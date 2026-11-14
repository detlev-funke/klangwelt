//! Keyboard instruments for counterpoint and fugue: pipe organ and harpsichord.
//!
//! **Organ** - additive: for each note a wavetable with the matching partials is built
//! from the drawn stops (16′, 8′, 4′,
//! 2⅔′, 2′, mixture) (only below the Nyquist limit) and played with pipe speech, blowing noise
//! and tremulant. The organ sounds as long as the key is held.
//!
//! **Harpsichord** - two string choirs (8′ and 4′) via the Karplus-Strong pluck:
//! bright quill attack, little body, lute stop (felt) dampens, a soft damper noise
//! on release. Velocity changes hardly anything - as on a
//! real harpsichord.

use crate::notes::midi_to_hz;
use crate::synth::{mix_into, pluck_note, Biquad, NoteSpec, PluckParams, Rng};
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Organ stops, all 0…1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganParams {
    /// Subbass 16′ (one octave lower)
    pub subbass: f32,
    /// Principal 8′ (fundamental with overtones)
    pub prinzipal: f32,
    /// Flute 4′ (octave, almost free of overtones)
    pub floete: f32,
    /// Fifth 2⅔′ (third partial)
    pub quinte: f32,
    /// Octave 2′ (fourth partial)
    pub oktave: f32,
    /// Mixture (high partials)
    pub mixtur: f32,
    /// Blowing noise
    pub chiff: f32,
    /// Tremulant (wind fluctuation)
    pub tremulant: f32,
}

impl Default for OrganParams {
    fn default() -> Self {
        Self { subbass: 0.0, prinzipal: 0.8, floete: 0.5, quinte: 0.0, oktave: 0.3, mixtur: 0.0, chiff: 0.4, tremulant: 0.0 }
    }
}

const TABLE: usize = 2048;

/// Partials (ratio to the fundamental, strength) of the drawn stops.
fn organ_partials(p: &OrganParams) -> Vec<(f32, f32)> {
    let mut v = Vec::new();
    let mut add = |ratio: f32, amp: f32| {
        if amp > 1e-4 {
            v.push((ratio, amp));
        }
    };
    add(0.5, 0.9 * p.subbass);
    add(1.0, 0.25 * p.subbass);
    for (h, a) in [(1.0, 1.0), (2.0, 0.45), (3.0, 0.25), (4.0, 0.14), (5.0, 0.08), (6.0, 0.05), (8.0, 0.03)] {
        add(h, a * p.prinzipal);
    }
    add(2.0, 0.75 * p.floete);
    add(4.0, 0.06 * p.floete);
    add(3.0, 0.55 * p.quinte);
    add(4.0, 0.6 * p.oktave);
    add(8.0, 0.12 * p.oktave);
    for (h, a) in [(6.0, 0.3), (8.0, 0.28), (12.0, 0.22), (16.0, 0.16)] {
        add(h, a * p.mixtur);
    }
    v
}

/// Wavetable (unnormalized) from the partials for which `take` holds.
fn organ_table(partials: &[(f32, f32)], freq: f32, sr: f32, take: impl Fn(f32) -> bool) -> Vec<f32> {
    let mut t = vec![0f32; TABLE + 1];
    for (j, &(ratio, amp)) in partials.iter().enumerate() {
        // partials above 0.45·sr are dropped (no aliasing)
        if freq * ratio > sr * 0.45 || !take(ratio) {
            continue;
        }
        // fixed, different phases: less peaky waveform, fuller at the same peak level
        let phase = j as f32 * 2.399_963;
        // Half partials (16′) need a table spanning two fundamental periods: the 16′ fundamental becomes the period
        let k = ratio * 2.0;
        for (i, x) in t.iter_mut().enumerate().take(TABLE) {
            *x += amp * (2.0 * PI * k * i as f32 / TABLE as f32 + phase).sin();
        }
    }
    t[TABLE] = t[0];
    t
}

/// An organ note (sounds until key end, then a short decay).
///
/// As with real pipes: each pipe is tuned a cent or two differently
/// (chords shimmer slightly), the overtones speak faster than the
/// fundamental, and a little wind noise runs along during the note.
pub fn organ_note(n: &NoteSpec, sr: f32, p: &OrganParams, seed: u32) -> Vec<f32> {
    let partials = organ_partials(p);
    if partials.is_empty() {
        return vec![0.0; (0.1 * sr) as usize];
    }
    // tuning of the pipe: up to ±1.8 cents, fixed per note (not random from run to run)
    let cents = ((n.midi * 7.31).sin() * 1.8) / 100.0;
    let freq = midi_to_hz(n.midi + n.pitch_offset(0.0) + cents).clamp(15.0, sr * 0.45);
    // fundamental (up to 8′/4′) and overtones separate - own speech
    let low = organ_table(&partials, freq, sr, |r| r <= 2.0);
    let high = organ_table(&partials, freq, sr, |r| r > 2.0);
    let peak = low.iter().zip(&high).fold(0f32, |m, (a, b)| m.max((a + b).abs())).max(1e-6);
    let (low, high): (Vec<f32>, Vec<f32>) = (low.iter().map(|v| v / peak).collect(), high.iter().map(|v| v / peak).collect());
    let release = 0.14;
    let total = ((n.dur_secs.max(0.02) + release) * sr) as usize;
    let mut out = vec![0f32; total];
    // low pipes speak more slowly
    let attack = (0.035 + 0.03 * ((60.0 - n.midi) / 24.0)).clamp(0.015, 0.09);
    let note_end = (n.dur_secs * sr) as usize;
    let rel_n = (release * sr) as usize;
    let mut phase = 0f32;
    let mut rng = Rng(seed.wrapping_mul(747_796_405) | 1);
    let mut chiff_bp = Biquad::bandpass((freq * 4.0).min(7000.0), 2.5, sr);
    let mut wind_bp = Biquad::bandpass((freq * 2.5).clamp(300.0, 6000.0), 1.2, sr);
    let (att_low, att_high) = (((attack * 1.35) * sr) as usize, ((attack * 0.55) * sr) as usize);
    let modulated = n.modulated();
    let mut f = freq;
    // table contains two fundamental periods (because of 16′): half step size
    let mut inc = f / sr * TABLE as f32 * 0.5;
    let level = 0.16 * (0.75 + 0.25 * n.velocity.clamp(0.0, 1.0));
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / sr;
        if modulated && i % 16 == 0 {
            f = midi_to_hz(n.midi + n.pitch_offset(t)).clamp(15.0, sr * 0.45);
            inc = f / sr * TABLE as f32 * 0.5;
        }
        let trem = 1.0 + 0.12 * p.tremulant * (2.0 * PI * 5.6 * t).sin();
        phase += inc * (1.0 + 0.002 * p.tremulant * (2.0 * PI * 5.6 * t).sin());
        if phase >= TABLE as f32 {
            phase -= TABLE as f32;
        }
        let idx = phase as usize;
        let frac = phase - idx as f32;
        let lerp = |t: &[f32]| t[idx] * (1.0 - frac) + t[idx + 1] * frac;
        let rise = |len: usize| {
            if i >= len {
                return 1.0;
            }
            let x = i as f32 / len.max(1) as f32;
            x * x * (3.0 - 2.0 * x)
        };
        let fall = if i >= note_end { (1.0 - (i - note_end) as f32 / rel_n.max(1) as f32).max(0.0) } else { 1.0 };
        let s = lerp(&low) * rise(att_low) + lerp(&high) * rise(att_high);
        // blowing noise: filtered noise in the first 60 ms, afterwards quiet wind
        let noise = rng.next();
        let chiff = if t < 0.06 { chiff_bp.process(noise) * p.chiff * 0.9 * (1.0 - t / 0.06) } else { 0.0 };
        let wind = wind_bp.process(noise) * (0.012 + 0.02 * p.chiff) * rise(att_low);
        *o = ((s + wind) * fall * trem + chiff) * level;
    }
    out
}

pub fn render_organ(notes: &[NoteSpec], total_secs: f32, sr: f32, p: &OrganParams) -> Vec<f32> {
    let mut out = vec![0f32; (total_secs.max(0.0) * sr) as usize];
    for (i, n) in notes.iter().enumerate() {
        let x = organ_note(n, sr, p, i as u32 + 1);
        mix_into(&mut out, &x, (n.start_secs.max(0.0) * sr) as usize, 1.0);
    }
    out
}

/// Harpsichord stops, all 0…1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarpsichordParams {
    /// 8′ string choir
    pub achtfuss: f32,
    /// 4′ string choir (one octave higher)
    pub vierfuss: f32,
    /// Lute stop (felt dampens)
    pub laute: f32,
    /// Quill: soft ↔ sharp
    pub kiel: f32,
    /// Ring-out: short ↔ long
    pub nachklang: f32,
}

impl Default for HarpsichordParams {
    fn default() -> Self {
        Self { achtfuss: 1.0, vierfuss: 0.0, laute: 0.0, kiel: 0.6, nachklang: 0.5 }
    }
}

pub fn harpsichord_note(n: &NoteSpec, sr: f32, p: &HarpsichordParams, seed: u32) -> Vec<f32> {
    let pp = PluckParams {
        brightness: (0.58 + 0.28 * p.kiel - 0.5 * p.laute).clamp(0.0, 1.0),
        decay: (0.25 + 0.6 * p.nachklang) * (1.0 - 0.75 * p.laute),
        body: 0.15,
        attack: (0.55 + 0.35 * p.kiel - 0.4 * p.laute).clamp(0.0, 1.0),
    };
    // hardly any dynamics
    let vel = 0.82 + 0.1 * n.velocity.clamp(0.0, 1.0);
    let base = NoteSpec { velocity: vel, ..*n };
    let mut out = if p.achtfuss > 0.0 {
        let mut v = pluck_note(&base, sr, &pp, seed);
        for x in &mut v {
            *x *= p.achtfuss;
        }
        v
    } else {
        Vec::new()
    };
    if p.vierfuss > 0.0 {
        // 4′: one octave higher, slightly detuned (two strings shimmer slightly)
        let four = NoteSpec { midi: n.midi + 12.03, ..base };
        let v = pluck_note(&four, sr, &PluckParams { brightness: (pp.brightness + 0.1).min(1.0), ..pp }, seed.wrapping_add(7919));
        if out.len() < v.len() {
            out.resize(v.len(), 0.0);
        }
        mix_into(&mut out, &v, 0, 0.8 * p.vierfuss);
    }
    // damper noise on release
    let end = (n.dur_secs * sr) as usize;
    let thump_len = (0.03 * sr) as usize;
    if end + thump_len < out.len() + thump_len {
        let mut rng = Rng(seed.wrapping_mul(2_891_336_453) | 1);
        let mut bp = Biquad::bandpass(midi_to_hz(n.midi).clamp(80.0, 2000.0) * 1.5, 1.5, sr);
        for k in 0..thump_len {
            if let Some(o) = out.get_mut(end + k) {
                *o += bp.process(rng.next()) * 0.025 * (1.0 - k as f32 / thump_len as f32);
            }
        }
    }
    out
}

pub fn render_harpsichord(notes: &[NoteSpec], total_secs: f32, sr: f32, p: &HarpsichordParams) -> Vec<f32> {
    let mut out = vec![0f32; (total_secs.max(0.0) * sr) as usize];
    for (i, n) in notes.iter().enumerate() {
        let x = harpsichord_note(n, sr, p, i as u32 * 31 + 5);
        mix_into(&mut out, &x, (n.start_secs.max(0.0) * sr) as usize, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{detect_pitch, YinConfig};

    fn pitch_of(x: &[f32], sr: f32) -> f32 {
        let frames = detect_pitch(&x[(0.15 * sr) as usize..(0.6 * sr) as usize], sr, &YinConfig::default());
        let mut f: Vec<f32> = frames.iter().filter_map(|f| f.freq).collect();
        f.sort_by(f32::total_cmp);
        f[f.len() / 2]
    }

    #[test]
    fn organ_sounds_at_the_right_pitch_and_sustains() {
        let sr = 48_000.0;
        let n = NoteSpec::new(0.0, 1.0, 57.0, 0.8); // A3
        let x = organ_note(&n, sr, &OrganParams { prinzipal: 1.0, floete: 0.0, oktave: 0.0, ..Default::default() }, 1);
        assert!((pitch_of(&x, sr) - 220.0).abs() < 3.0, "{}", pitch_of(&x, sr));
        // sustains: level at 0.8 s similar to that at 0.3 s
        let rms = |a: f32, b: f32| {
            let s = &x[(a * sr) as usize..(b * sr) as usize];
            (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
        };
        assert!((rms(0.75, 0.85) / rms(0.25, 0.35) - 1.0).abs() < 0.15);
        assert!(rms(1.12, 1.14) < 0.2 * rms(0.5, 0.6), "klingt nach dem Loslassen kurz aus");
        // 16′ alone: one octave lower
        let sub = organ_note(&n, sr, &OrganParams { subbass: 1.0, prinzipal: 0.0, floete: 0.0, oktave: 0.0, chiff: 0.0, ..Default::default() }, 2);
        assert!((pitch_of(&sub, sr) - 110.0).abs() < 2.0, "{}", pitch_of(&sub, sr));
        // no values outside ±1
        assert!(x.iter().all(|v| v.abs() < 1.0));
    }

    #[test]
    fn harpsichord_plucks_and_four_foot_brightens() {
        let sr = 48_000.0;
        let n = NoteSpec::new(0.0, 0.8, 57.0, 0.8);
        let eight = harpsichord_note(&n, sr, &HarpsichordParams::default(), 3);
        assert!((pitch_of(&eight, sr) - 220.0).abs() < 4.0);
        let both = harpsichord_note(&n, sr, &HarpsichordParams { vierfuss: 1.0, ..Default::default() }, 3);
        // strength of a partial (DFT at one frequency) between 0.1 and 0.5 s
        let amp = |x: &[f32], f: f32| {
            let (a, b) = ((0.1 * sr) as usize, (0.5 * sr) as usize);
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, v) in x[a..b].iter().enumerate() {
                let w = 2.0 * PI * f * i as f32 / sr;
                re += v * w.cos();
                im -= v * w.sin();
            }
            (re * re + im * im).sqrt()
        };
        // 4′ alone sounds one octave higher; together more energy (the choirs shimmer slightly)
        let four = harpsichord_note(&n, sr, &HarpsichordParams { achtfuss: 0.0, vierfuss: 1.0, ..Default::default() }, 3);
        assert!(amp(&four, 880.0) > amp(&four, 220.0) * 3.0, "4′ ohne 8′-Grundton");
        let energy = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
        assert!(energy(&both) > energy(&eight) * 1.15, "4′ füllt: {} → {}", energy(&eight), energy(&both));
        assert!(amp(&eight, 220.0) > 0.2 * amp(&eight, 440.0), "Grundton nicht verschwunden");
        let lute = harpsichord_note(&n, sr, &HarpsichordParams { laute: 1.0, ..Default::default() }, 3);
        let tail = |x: &[f32]| x[(0.5 * sr) as usize..(0.7 * sr) as usize].iter().map(|v| v.abs()).sum::<f32>();
        assert!(tail(&lute) < tail(&eight) * 0.6, "Lautenzug dämpft");
    }
}
