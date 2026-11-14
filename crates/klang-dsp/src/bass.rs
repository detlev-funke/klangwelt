//! Bass-Synthesizer (subtraktiv, einstimmig) mit sprechenden Parametern.
//!
//! Oszillator (Sinus ↔ Sägezahn ↔ Rechteck, bandbegrenzt per PolyBLEP),
//! Sub-Oszillator eine Oktave tiefer, resonanter Tiefpass (2 × TPT-SVF) mit
//! Hüllkurve, Sättigung („Biss“) und Glide zwischen gebundenen Noten.

use crate::notes::midi_to_hz;
use crate::synth::NoteSpec;
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Alle Werte 0…1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BassParams {
    /// rund (Sinus) ↔ Sägezahn ↔ hohl/scharf (Rechteck)
    pub wellenform: f32,
    /// dunkel ↔ hell (Grenzfrequenz)
    pub filter: f32,
    /// weich ↔ quäkig (Resonanz)
    pub resonanz: f32,
    /// weich ↔ knackig (Filter-Hüllkurve und Einschwingen)
    pub anschlag: f32,
    /// kurz ↔ lang (Ausklingen, Haltepegel)
    pub abklingen: f32,
    /// dünn ↔ tief (Sub-Oszillator)
    pub sub: f32,
    /// sauber ↔ verzerrt (Sättigung)
    pub biss: f32,
    /// aus ↔ gleitend (Portamento bei gebundenen Noten)
    pub glide: f32,
}

impl Default for BassParams {
    fn default() -> Self {
        Self { wellenform: 0.5, filter: 0.35, resonanz: 0.3, anschlag: 0.5, abklingen: 0.5, sub: 0.4, biss: 0.2, glide: 0.0 }
    }
}

#[inline]
pub(crate) fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// Zustandsvariablen-Filter (topologieerhaltend, Zavalishin) – Tiefpass-Ausgang.
#[derive(Default, Clone, Copy)]
pub(crate) struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    pub(crate) fn lowpass(&mut self, x: f32, g: f32, k: f32) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }
}

/// Ein Bass-Ton. `from_hz` startet ein Glide von dieser Frequenz aus.
/// Ausdruck aus `note`: Anschlag (Lautstärke, Filterhüllkurve), Helligkeit
/// (Filter), Vibrato und Anschleifen (Tonhöhe).
fn voice(freq: f32, from_hz: Option<f32>, dur: f32, note: &NoteSpec, sr: f32, p: &BassParams) -> Vec<f32> {
    let velocity = note.velocity;
    let modulated = note.modulated();
    let release = 0.04;
    let n = ((dur.max(0.02) + release) * sr) as usize;
    let note_end = (dur * sr) as usize;
    let w = p.wellenform.clamp(0.0, 1.0);
    let anschlag = p.anschlag.clamp(0.0, 1.0);
    let abklingen = p.abklingen.clamp(0.0, 1.0);
    let attack = 0.002 + 0.02 * (1.0 - anschlag);
    let decay = 0.05 + 0.8 * abklingen;
    let sustain = 0.3 + 0.6 * abklingen;
    // Helligkeit der Note verschiebt den Filter um bis zu ±1,5 Oktaven.
    let base_cut = 40.0 * 2f32.powf(p.filter.clamp(0.0, 1.0) * 8.0 + 1.5 * note.brightness.clamp(-1.0, 1.0));
    // Fester angeschlagen = Filter öffnet weiter.
    let env_amt = anschlag * 4.0 * (0.55 + 0.5 * velocity.clamp(0.0, 1.0)); // Oktaven
    let f_decay = 0.05 + 0.6 * abklingen;
    let k = 2.0 - 1.9 * p.resonanz.clamp(0.0, 1.0);
    let drive = 1.0 + 8.0 * p.biss.clamp(0.0, 1.0);
    // Gleitzeit (Zeitkonstante) bis ca. 125 ms – typisch für Bass-Portamento.
    let tau = 0.005 + 0.12 * p.glide.clamp(0.0, 1.0);
    let (mut ph, mut sub_ph) = (0.0f32, 0.0f32);
    let (mut f1, mut f2) = (Svf::default(), Svf::default());
    let mut g = 0.0;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let mut f = match from_hz {
            Some(f0) => freq + (f0 - freq) * (-t / tau).exp(),
            None => freq,
        };
        if modulated {
            f *= 2f32.powf(note.pitch_offset(t) / 12.0);
        }
        let dt = (f / sr).min(0.45);
        ph += dt;
        if ph >= 1.0 {
            ph -= 1.0;
        }
        sub_ph += dt * 0.5;
        if sub_ph >= 1.0 {
            sub_ph -= 1.0;
        }
        let sine = (2.0 * PI * ph).sin();
        let saw = 2.0 * ph - 1.0 - poly_blep(ph, dt);
        let square = (if ph < 0.5 { 1.0 } else { -1.0 }) + poly_blep(ph, dt) - poly_blep((ph + 0.5) % 1.0, dt);
        let osc = if w < 0.5 {
            let a = w / 0.5;
            (1.0 - a) * sine + a * saw
        } else {
            let b = (w - 0.5) / 0.5;
            (1.0 - b) * saw + b * square * 0.8
        };
        let sub = (2.0 * PI * sub_ph).sin() * p.sub.clamp(0.0, 1.0) * 0.8;

        if i % 16 == 0 {
            let cut = (base_cut * 2f32.powf(env_amt * (-t / f_decay).exp())).min(sr * 0.45);
            g = (PI * cut / sr).tan();
        }
        let filtered = f2.lowpass(f1.lowpass(osc, g, k), g, 1.414);

        let amp = if t < attack {
            t / attack
        } else {
            sustain + (1.0 - sustain) * (-(t - attack) / decay).exp()
        };
        let rel = if i >= note_end { (1.0 - (i - note_end) as f32 / (release * sr)).max(0.0) } else { 1.0 };
        let y = ((filtered + sub) * drive).tanh() / drive.tanh();
        out.push(y * amp * rel * 0.5 * velocity);
    }
    out
}

/// Rendert eine Basslinie einstimmig. Gebundene Noten (die nächste beginnt,
/// bevor die vorherige endet bzw. höchstens 30 ms danach) gleiten bei `glide > 0`.
pub fn render_bass(notes: &[NoteSpec], total_secs: f32, sr: f32, p: &BassParams) -> Vec<f32> {
    let mut sorted: Vec<&NoteSpec> = notes.iter().collect();
    sorted.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    let mut out = vec![0f32; ((total_secs.max(0.0) + 0.2) * sr) as usize];
    let mut prev: Option<(f32, f32)> = None; // (Ende, Frequenz)
    for (i, n) in sorted.iter().enumerate() {
        let freq = midi_to_hz(n.midi);
        // einstimmig: Note endet spätestens, wenn die nächste beginnt
        let next_start = sorted.get(i + 1).map(|m| m.start_secs).unwrap_or(f32::MAX);
        let dur = n.dur_secs.min((next_start - n.start_secs).max(0.02));
        let legato = prev.map(|(end, _)| n.start_secs <= end + 0.03).unwrap_or(false);
        let from = if legato && p.glide > 0.01 { prev.map(|(_, f)| f) } else { None };
        let v = voice(freq, from, dur, n, sr, p);
        let start = (n.start_secs.max(0.0) * sr) as usize;
        for (j, s) in v.iter().enumerate() {
            match out.get_mut(start + j) {
                Some(o) => *o += s,
                None => break,
            }
        }
        prev = Some((n.start_secs + n.dur_secs, freq));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::analyze_sound;
    use crate::pitch::{detect_pitch, YinConfig};
    use crate::util::median;

    const SR: f32 = 48_000.0;

    fn note(midi: f32, start: f32, dur: f32) -> NoteSpec {
        NoteSpec::new(start, dur, midi, 0.9)
    }

    #[test]
    fn expression_changes_pitch_and_tone() {
        let p = BassParams { sub: 0.0, ..Default::default() };
        let mid = |x: &[f32], a: f32, b: f32| {
            let mut est: Vec<f32> = detect_pitch(&x[(a * SR) as usize..(b * SR) as usize], SR, &YinConfig::default())
                .iter()
                .filter_map(|q| q.freq)
                .collect();
            median(&mut est)
        };
        // Anschleifen von 3 Halbtönen unten: Anfang tiefer, Ende auf dem Ton (A1 = 55 Hz).
        let bent = render_bass(&[NoteSpec { bend: -3.0, ..note(33.0, 0.0, 1.0) }], 1.0, SR, &p);
        let early = mid(&bent, 0.0, 0.08);
        let late = mid(&bent, 0.5, 0.9);
        assert!(early < 52.0, "Anfang {early} Hz");
        assert!((late - 55.0).abs() < 1.5, "Ende {late} Hz");
        // Helligkeit öffnet den Filter.
        let one = |b: f32| analyze_sound(&render_bass(&[NoteSpec { brightness: b, ..note(33.0, 0.0, 1.0) }], 1.0, SR, &p), SR);
        assert!(one(0.8).centroid_hz > one(-0.8).centroid_hz * 1.3);
        // Vibrato bleibt beim Ton und schwankt hörbar.
        let vib = render_bass(&[NoteSpec { vibrato: 0.6, ..note(45.0, 0.0, 1.5) }], 1.5, SR, &p);
        let est: Vec<f32> = detect_pitch(&vib[(0.6 * SR) as usize..(1.4 * SR) as usize], SR, &YinConfig::default())
            .iter()
            .filter_map(|q| q.freq)
            .collect();
        let (lo, hi) = est.iter().fold((f32::MAX, 0.0f32), |(l, h), &f| (l.min(f), h.max(f)));
        assert!(hi / lo > 1.03, "Schwankung {lo}–{hi} Hz");
    }

    fn pitch_of(x: &[f32]) -> f32 {
        let mut f: Vec<f32> = detect_pitch(x, SR, &YinConfig::default()).iter().filter_map(|p| p.freq).collect();
        median(&mut f)
    }

    #[test]
    fn plays_the_right_note() {
        for (midi, hz) in [(28.0f32, 41.2f32), (33.0, 55.0), (40.0, 82.4)] {
            let x = render_bass(&[note(midi, 0.0, 1.0)], 1.0, SR, &BassParams { sub: 0.0, ..Default::default() });
            let f = pitch_of(&x[..(0.9 * SR) as usize]);
            assert!((f - hz).abs() / hz < 0.02, "MIDI {midi}: {f} Hz statt {hz}");
        }
    }

    #[test]
    fn filter_and_sub_move_the_sound() {
        let one = |p: BassParams| analyze_sound(&render_bass(&[note(33.0, 0.0, 1.0)], 1.0, SR, &p), SR);
        let dark = one(BassParams { filter: 0.1, ..Default::default() });
        let bright = one(BassParams { filter: 0.8, ..Default::default() });
        assert!(bright.centroid_hz > 1.5 * dark.centroid_hz, "{} / {}", dark.centroid_hz, bright.centroid_hz);
        let thin = one(BassParams { sub: 0.0, filter: 0.6, ..Default::default() });
        let deep = one(BassParams { sub: 1.0, filter: 0.6, ..Default::default() });
        assert!(deep.low_share > thin.low_share);
    }

    /// Frequenz über aufsteigende Nulldurchgänge (für reine Sinus-Signale).
    fn zero_cross_hz(x: &[f32]) -> f32 {
        let n = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        n as f32 / (x.len() as f32 / SR)
    }

    #[test]
    fn glide_bends_between_tied_notes() {
        let p = BassParams { glide: 0.8, wellenform: 0.0, sub: 0.0, filter: 0.9, biss: 0.0, ..Default::default() };
        let x = render_bass(&[note(33.0, 0.0, 0.5), note(45.0, 0.5, 0.5)], 1.0, SR, &p);
        assert!(x.iter().all(|v| v.is_finite()));
        // stabil vorher (A1 = 55 Hz) und nachher (A2 = 110 Hz) …
        assert!((zero_cross_hz(&x[(0.2 * SR) as usize..(0.45 * SR) as usize]) - 55.0).abs() < 5.0);
        assert!((zero_cross_hz(&x[(0.85 * SR) as usize..(0.98 * SR) as usize]) - 110.0).abs() < 9.0);
        // … dazwischen gleitet sie
        let f = zero_cross_hz(&x[(0.52 * SR) as usize..(0.6 * SR) as usize]);
        assert!(f > 60.0 && f < 100.0, "{f}");
        // ohne Glide springt sie sofort (Fenster nach dem 40-ms-Ausklang des Vortons)
        let hard = render_bass(&[note(33.0, 0.0, 0.5), note(45.0, 0.5, 0.5)], 1.0, SR, &BassParams { glide: 0.0, ..p });
        assert!(zero_cross_hz(&hard[(0.55 * SR) as usize..(0.63 * SR) as usize]) > 100.0);
    }
}
