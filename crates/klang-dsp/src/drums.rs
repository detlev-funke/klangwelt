//! Drum synthesis with expressive parameters.
//!
//! Each drum is a small physically inspired model:
//! * Bass drum – sine with falling pitch down to the **fundamental**, click, saturation
//! * Snare   – shell tone plus filtered noise (snare wires)
//! * Clap    – several short noise bursts plus decay
//! * Hi-hat  – high-pass-filtered noise with metallic band-passes
//! * Toms    – tuned sine drums with a slight pitch drop
//! * Rim     – short, woody double tone (side stick), tuned with the snare shell
//! * Cowbell – two square tones (540/800 Hz) with a band-pass, like the classic drum machine
//! * Ride/Crash – inharmonic partials plus noise, brightness and length like the hi-hat
//! * Shaker  – short noise swish with a soft attack
//!
//! The key mapping follows the usual drum mapping (General MIDI).
//!
//! Every note sounds: notes off the drum keys – and all notes once the kit
//! plays by pitch ([`DrumKit::pitched`]) – play a tuned drum at their own
//! pitch ([`voice_for`]): bass drum below C3, tom up to B4, snare from C5.

use crate::notes::midi_to_hz;
use crate::synth::{Biquad, NoteSpec, Rng};
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

pub const KICK: u8 = 36;
pub const SNARE: u8 = 38;
pub const CLAP: u8 = 39;
pub const TOM_LOW: u8 = 41;
pub const HAT_CLOSED: u8 = 42;
pub const TOM_MID: u8 = 45;
pub const HAT_OPEN: u8 = 46;
pub const TOM_HIGH: u8 = 48;
pub const RIM: u8 = 37;
pub const CRASH: u8 = 49;
pub const RIDE: u8 = 51;
pub const COWBELL: u8 = 56;
pub const SHAKER: u8 = 70;

/// Tuned drums: from this key on (C3) a note plays the tom, below it the bass drum.
pub const TUNED_TOM_FROM: u8 = 48;
/// Tuned drums: from this key on (C5) a note plays the snare.
pub const TUNED_SNARE_FROM: u8 = 72;
/// Tuned drums never go below A0 (27.5 Hz) – lower a bass drum is only a click.
const TUNED_LOWEST: f32 = 21.0;

/// Name of the drum on a key (short form for the keyboard).
pub fn piece_name(midi: u8) -> Option<(&'static str, &'static str)> {
    Some(match midi {
        KICK => ("BD", klang_i18n::tk!("Kick drum")),
        SNARE => ("SD", klang_i18n::tk!("Snare")),
        CLAP => ("CP", klang_i18n::tk!("Clap")),
        TOM_LOW => ("T1", klang_i18n::tk!("Low tom")),
        HAT_CLOSED => ("HH", klang_i18n::tk!("Closed hi-hat")),
        TOM_MID => ("T2", klang_i18n::tk!("Mid tom")),
        HAT_OPEN => ("OH", klang_i18n::tk!("Open hi-hat")),
        TOM_HIGH => ("T3", klang_i18n::tk!("High tom")),
        RIM => ("RS", klang_i18n::tk!("Rim")),
        CRASH => ("CR", klang_i18n::tk!("Crash")),
        RIDE => ("RD", klang_i18n::tk!("Ride")),
        COWBELL => ("CB", klang_i18n::tk!("Cowbell")),
        SHAKER => ("SH", klang_i18n::tk!("Shaker")),
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct KickParams {
    /// Fundamental as a MIDI note (end of the pitch drop), e.g. 33 = A1 = 55 Hz.
    pub grundton: f32,
    /// soft ↔ punchy: strength and speed of the pitch drop
    pub punch: f32,
    /// short ↔ long
    pub laenge: f32,
    /// round ↔ clicky: impact noise of the beater
    pub klick: f32,
    /// clean ↔ warm/distorted
    pub waerme: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnareParams {
    /// Shell tone as a MIDI note
    pub kessel: f32,
    /// little ↔ much rattle
    pub teppich: f32,
    pub laenge: f32,
    /// dull ↔ sharp
    pub schaerfe: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClapParams {
    pub helligkeit: f32,
    pub laenge: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HatParams {
    pub helligkeit: f32,
    /// closed hi-hat: short ↔ long
    pub laenge: f32,
    /// open hi-hat: short ↔ long
    pub offen: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TomParams {
    /// Fundamental of the low tom (mid +5, high +10 semitones)
    pub grundton: f32,
    pub laenge: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumKit {
    pub kick: KickParams,
    pub snare: SnareParams,
    pub clap: ClapParams,
    pub hat: HatParams,
    pub tom: TomParams,
    /// Keys play by pitch: every note is a tuned drum at its pitch, also on
    /// the drum keys. Off (the drum kit): a drum key plays its drum, only the
    /// other keys play tuned drums.
    #[serde(default)]
    pub pitched: bool,
}

impl Default for DrumKit {
    fn default() -> Self {
        Self {
            kick: KickParams { grundton: 33.0, punch: 0.6, laenge: 0.5, klick: 0.4, waerme: 0.3 },
            snare: SnareParams { kessel: 54.0, teppich: 0.6, laenge: 0.4, schaerfe: 0.5 },
            clap: ClapParams { helligkeit: 0.5, laenge: 0.4 },
            hat: HatParams { helligkeit: 0.6, laenge: 0.3, offen: 0.5 },
            tom: TomParams { grundton: 43.0, laenge: 0.5 },
            pitched: false,
        }
    }
}

/// The drum a note plays and the kit tuned for it – never nothing. In the
/// drum kit a drum key plays its drum unchanged; every other note (and every
/// note when the kit plays by pitch) plays a tuned drum that settles on the
/// note's pitch: bass drum below C3, tom up to B4, snare from C5. The tuned
/// drum keeps the kit's character (punch, length, click, snare wires …).
pub fn voice_for(kit: &DrumKit, midi: f32) -> (u8, DrumKit) {
    let key = midi.round().clamp(0.0, 127.0) as u8;
    if !kit.pitched && piece_name(key).is_some() {
        return (key, *kit);
    }
    let pitch = midi.max(TUNED_LOWEST);
    let mut k = *kit;
    if key < TUNED_TOM_FROM {
        k.kick.grundton = pitch;
        (KICK, k)
    } else if key < TUNED_SNARE_FROM {
        k.tom.grundton = pitch;
        (TOM_LOW, k)
    } else {
        k.snare.kessel = pitch;
        (SNARE, k)
    }
}

pub(crate) fn soft_clip(x: f32, drive: f32) -> f32 {
    (x * drive).tanh() / drive.tanh()
}

pub fn kick(sr: f32, p: &KickParams, velocity: f32, seed: u32) -> Vec<f32> {
    let f_end = midi_to_hz(p.grundton);
    let punch = p.punch.clamp(0.0, 1.0);
    let f_start = f_end * (1.5 + 4.0 * punch);
    // Fast decay, so the fundamental audibly (and measurably) determines the pitch.
    let sweep = 30.0 + 50.0 * punch;
    let decay = 0.12 + 1.1 * p.laenge.clamp(0.0, 1.0);
    let n = ((decay + 0.02) * sr) as usize;
    let drive = 1.0 + 4.0 * p.waerme.clamp(0.0, 1.0);
    let mut rng = Rng(seed | 1);
    let mut click_hp = Biquad::highpass(2500.0, 0.7, sr);
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let f = f_end + (f_start - f_end) * (-t * sweep).exp();
            phase += 2.0 * PI * f / sr;
            let amp = (-t * 6.9 / decay).exp();
            let body = soft_clip(phase.sin() * amp, drive);
            let click = click_hp.process(rng.next()) * (-t * 400.0).exp() * p.klick.clamp(0.0, 1.0) * 0.7;
            let fade = (t * 2000.0).min(1.0);
            (body + click) * fade * 0.9 * velocity
        })
        .collect()
}

pub fn snare(sr: f32, p: &SnareParams, velocity: f32, seed: u32) -> Vec<f32> {
    let f = midi_to_hz(p.kessel);
    let laenge = p.laenge.clamp(0.0, 1.0);
    let teppich = p.teppich.clamp(0.0, 1.0);
    let n = ((0.12 + 0.5 * laenge) * sr) as usize;
    let mut rng = Rng(seed | 1);
    let mut bp = Biquad::bandpass(1500.0 + 5000.0 * p.schaerfe.clamp(0.0, 1.0), 0.8, sr);
    let tone_decay = 30.0 - 15.0 * laenge;
    let noise_decay = 30.0 - 22.0 * laenge;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            phase += 2.0 * PI * f * (1.0 + 0.15 * (-t * 40.0).exp()) / sr;
            let tone = phase.sin() * (-t * tone_decay).exp() * (0.8 - 0.4 * teppich);
            let noise = bp.process(rng.next()) * (-t * noise_decay).exp() * teppich * 1.8;
            (tone + noise) * velocity
        })
        .collect()
}

pub fn clap(sr: f32, p: &ClapParams, velocity: f32, seed: u32) -> Vec<f32> {
    let laenge = p.laenge.clamp(0.0, 1.0);
    let n = ((0.1 + 0.4 * laenge) * sr) as usize;
    let mut rng = Rng(seed | 1);
    let mut bp = Biquad::bandpass(900.0 + 2200.0 * p.helligkeit.clamp(0.0, 1.0), 1.2, sr);
    let tail = 18.0 - 12.0 * laenge;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            // three short bursts ~10 ms apart, then decay
            let bursts: f32 = [0.0f32, 0.011, 0.022].iter().map(|&o| if t >= o { (-(t - o) * 180.0).exp() } else { 0.0 }).sum();
            let env = bursts * 0.7 + if t >= 0.022 { (-(t - 0.022) * tail).exp() * 0.6 } else { 0.0 };
            bp.process(rng.next()) * env * 2.2 * velocity
        })
        .collect()
}

pub fn hat(sr: f32, p: &HatParams, open: bool, velocity: f32, seed: u32) -> Vec<f32> {
    let len = if open { 0.15 + 0.7 * p.offen.clamp(0.0, 1.0) } else { 0.03 + 0.15 * p.laenge.clamp(0.0, 1.0) };
    let n = ((len + 0.01) * sr) as usize;
    let h = p.helligkeit.clamp(0.0, 1.0);
    let mut rng = Rng(seed | 1);
    let mut hp = Biquad::highpass(4500.0 + 5000.0 * h, 0.7, sr);
    // metallic coloring: two narrow resonances
    let mut m1 = Biquad::bandpass(6200.0 + 2500.0 * h, 6.0, sr);
    let mut m2 = Biquad::bandpass(8300.0 + 2800.0 * h, 6.0, sr);
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let x = hp.process(rng.next());
            let metal = m1.process(x) + m2.process(x);
            let env = (-t * 6.9 / len).exp();
            (x * 0.7 + metal * 0.9) * env * 0.8 * velocity
        })
        .collect()
}

pub fn tom(sr: f32, p: &TomParams, offset: f32, velocity: f32, seed: u32) -> Vec<f32> {
    let f = midi_to_hz(p.grundton + offset);
    let decay = 0.2 + 0.8 * p.laenge.clamp(0.0, 1.0);
    let n = ((decay + 0.02) * sr) as usize;
    let mut rng = Rng(seed | 1);
    let mut lp = Biquad::lowpass(3000.0, 0.7, sr);
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            phase += 2.0 * PI * f * (1.0 + 0.35 * (-t * 25.0).exp()) / sr;
            let body = phase.sin() * (-t * 6.9 / decay).exp();
            let stick = lp.process(rng.next()) * (-t * 120.0).exp() * 0.25;
            (body + stick) * 0.85 * velocity
        })
        .collect()
}

/// Rim (side stick): two short, tuned tones and a click – tuned with the snare shell.
pub fn rim(sr: f32, p: &SnareParams, velocity: f32, seed: u32) -> Vec<f32> {
    let k = midi_to_hz(p.kessel) / midi_to_hz(54.0);
    let (f1, f2) = (455.0 * k, 1670.0 * k * (0.9 + 0.2 * p.schaerfe.clamp(0.0, 1.0)));
    let n = (0.07 * sr) as usize;
    let mut rng = Rng(seed | 1);
    let mut hp = Biquad::highpass(3000.0, 0.7, sr);
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let tone = (2.0 * PI * f1 * t).sin() * (-t * 90.0).exp() * 0.6 + (2.0 * PI * f2 * t).sin() * (-t * 140.0).exp() * 0.5;
            let click = hp.process(rng.next()) * (-t * 600.0).exp() * 0.6;
            (tone + click) * velocity
        })
        .collect()
}

/// Square wave from the first odd harmonics (without aliasing).
pub(crate) fn square(phase: f32) -> f32 {
    phase.sin() + (3.0 * phase).sin() / 3.0 + (5.0 * phase).sin() / 5.0 + (7.0 * phase).sin() / 7.0
}

/// Cowbell: two square tones (540 and 800 Hz) through a band-pass, fast decay with a tail.
pub fn cowbell(sr: f32, p: &HatParams, velocity: f32, _seed: u32) -> Vec<f32> {
    let n = (0.45 * sr) as usize;
    let mut bp = Biquad::bandpass(900.0 + 900.0 * p.helligkeit.clamp(0.0, 1.0), 1.4, sr);
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let x = square(2.0 * PI * 540.0 * t) + square(2.0 * PI * 800.0 * t);
            let env = 0.65 * (-t * 45.0).exp() + 0.35 * (-t * 8.0).exp();
            bp.process(x) * env * 0.9 * velocity
        })
        .collect()
}

/// Cymbal: six inharmonic square partials (like the classic drum machine) plus noise.
/// Ride: bright bell portion, medium decay; crash: more noise, long decay.
pub fn cymbal(sr: f32, p: &HatParams, crash: bool, velocity: f32, seed: u32) -> Vec<f32> {
    let h = p.helligkeit.clamp(0.0, 1.0);
    let len = if crash { 1.2 + 1.6 * p.offen.clamp(0.0, 1.0) } else { 0.8 + 1.2 * p.offen.clamp(0.0, 1.0) };
    let n = (len * sr) as usize;
    let mut rng = Rng(seed | 1);
    let mut hp = Biquad::highpass(if crash { 3000.0 } else { 5000.0 } + 2500.0 * h, 0.7, sr);
    let mut ping = Biquad::bandpass(if crash { 5200.0 } else { 3600.0 } + 1500.0 * h, 3.0, sr);
    const PARTIALS: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
    let noise_amt = if crash { 0.9 } else { 0.35 };
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let metal: f32 = PARTIALS.iter().map(|&f| square(2.0 * PI * f * 2.0 * t)).sum::<f32>() / 6.0;
            let x = hp.process(metal + rng.next() * noise_amt);
            let bell = ping.process(metal) * (-t * 12.0).exp() * if crash { 0.3 } else { 0.9 };
            let attack = if crash { (t * 400.0).min(1.0) } else { 1.0 };
            let env = (-t * 6.9 / len).exp();
            (x * 0.8 + bell) * env * attack * 0.6 * velocity
        })
        .collect()
}

/// Shaker: short, high noise with a soft attack.
pub fn shaker(sr: f32, p: &HatParams, velocity: f32, seed: u32) -> Vec<f32> {
    let len = 0.06 + 0.1 * p.laenge.clamp(0.0, 1.0);
    let n = ((len + 0.02) * sr) as usize;
    let mut rng = Rng(seed | 1);
    let mut bp = Biquad::bandpass(5500.0 + 3500.0 * p.helligkeit.clamp(0.0, 1.0), 1.1, sr);
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let env = (t / 0.012).min(1.0) * (-(t - 0.012).max(0.0) * 6.9 / len).exp();
            bp.process(rng.next()) * env * 1.1 * velocity
        })
        .collect()
}

/// A single hit on a key; `None` for unmapped keys.
pub fn render_hit(kit: &DrumKit, midi: u8, velocity: f32, sr: f32, seed: u32) -> Option<Vec<f32>> {
    Some(match midi {
        KICK => kick(sr, &kit.kick, velocity, seed),
        SNARE => snare(sr, &kit.snare, velocity, seed),
        CLAP => clap(sr, &kit.clap, velocity, seed),
        HAT_CLOSED => hat(sr, &kit.hat, false, velocity, seed),
        HAT_OPEN => hat(sr, &kit.hat, true, velocity, seed),
        TOM_LOW => tom(sr, &kit.tom, 0.0, velocity, seed),
        TOM_MID => tom(sr, &kit.tom, 5.0, velocity, seed),
        TOM_HIGH => tom(sr, &kit.tom, 10.0, velocity, seed),
        RIM => rim(sr, &kit.snare, velocity, seed),
        COWBELL => cowbell(sr, &kit.hat, velocity, seed),
        RIDE => cymbal(sr, &kit.hat, false, velocity, seed),
        CRASH => cymbal(sr, &kit.hat, true, velocity, seed),
        SHAKER => shaker(sr, &kit.hat, velocity, seed),
        _ => return None,
    })
}

/// A single note: the drum on its key or a tuned drum at its pitch (see [`voice_for`]).
pub fn render_note(kit: &DrumKit, midi: f32, velocity: f32, sr: f32, seed: u32) -> Vec<f32> {
    let (key, k) = voice_for(kit, midi);
    render_hit(&k, key, velocity, sr, seed).unwrap_or_default()
}

/// Whether a note is a hi-hat (chokes an open hi-hat before it).
pub(crate) fn is_hat(kit: &DrumKit, midi: f32) -> bool {
    matches!(voice_for(kit, midi).0, HAT_CLOSED | HAT_OPEN)
}

/// Brightness of a note: more click, sharpness and gloss (or less).
pub(crate) fn brighten(k: &mut DrumKit, brightness: f32) {
    if brightness == 0.0 {
        return;
    }
    let b = 0.35 * brightness.clamp(-1.0, 1.0);
    let c = |v: f32| v.clamp(0.0, 1.0);
    k.kick.klick = c(k.kick.klick + b);
    k.kick.waerme = c(k.kick.waerme - 0.5 * b);
    k.snare.schaerfe = c(k.snare.schaerfe + b);
    k.clap.helligkeit = c(k.clap.helligkeit + b);
    k.hat.helligkeit = c(k.hat.helligkeit + b);
}

/// Renders drum notes (single hits, note length does not matter).
/// An open hi-hat is choked by the next hi-hat – like with a real cymbal.
pub fn render_drums(notes: &[NoteSpec], total_secs: f32, sr: f32, kit: &DrumKit) -> Vec<f32> {
    let mut out = vec![0f32; ((total_secs.max(0.0) + 1.2) * sr) as usize];
    let hat_times: Vec<f32> = notes.iter().filter(|n| is_hat(kit, n.midi)).map(|n| n.start_secs).collect();
    for (i, n) in notes.iter().enumerate() {
        let (midi, mut k) = voice_for(kit, n.midi);
        brighten(&mut k, n.brightness);
        let Some(mut hit) = render_hit(&k, midi, n.velocity, sr, i as u32 * 7919 + 31) else { continue };
        if midi == HAT_OPEN {
            if let Some(&next) = hat_times.iter().find(|&&t| t > n.start_secs + 1e-4) {
                let keep = ((next - n.start_secs) * sr) as usize;
                let fade = (0.008 * sr) as usize;
                for (j, v) in hit.iter_mut().enumerate().skip(keep) {
                    let k = (j - keep) as f32 / fade.max(1) as f32;
                    *v *= (1.0 - k).max(0.0);
                }
            }
        }
        let start = (n.start_secs.max(0.0) * sr) as usize;
        for (j, v) in hit.iter().enumerate() {
            match out.get_mut(start + j) {
                Some(o) => *o += v,
                None => break,
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::hit_pitches;
    use crate::util::peak;

    const SR: f32 = 48_000.0;

    #[test]
    fn kick_sounds_at_its_fundamental() {
        for g in [28.0f32, 33.0, 38.0, 43.0] {
            let p = KickParams { grundton: g, ..DrumKit::default().kick };
            let x = kick(SR, &p, 1.0, 3);
            let m = hit_pitches(&x, SR, &[0.0])[0].midi.expect("Grundton messbar");
            assert!((m - g).abs() < 1.0, "Grundton {g} → gemessen {m}");
        }
    }

    #[test]
    fn snare_shell_is_tuned() {
        let p = SnareParams { kessel: 52.0, teppich: 0.5, ..DrumKit::default().snare };
        let x = snare(SR, &p, 1.0, 9);
        let m = hit_pitches(&x, SR, &[0.0])[0].midi.expect("Kesselton messbar");
        assert!((m - 52.0).abs() < 1.0, "{m}");
    }

    #[test]
    fn length_parameters_change_duration() {
        let short = kick(SR, &KickParams { laenge: 0.0, ..DrumKit::default().kick }, 1.0, 1);
        let long = kick(SR, &KickParams { laenge: 1.0, ..DrumKit::default().kick }, 1.0, 1);
        assert!(long.len() > 3 * short.len());
        let closed = hat(SR, &DrumKit::default().hat, false, 1.0, 1);
        let open = hat(SR, &DrumKit::default().hat, true, 1.0, 1);
        assert!(open.len() > 2 * closed.len());
    }

    #[test]
    fn kit_renders_mapped_keys_and_tunes_the_others() {
        let kit = DrumKit::default();
        let n = |midi: u8, t: f32| NoteSpec::new(t, 0.1, midi as f32, 0.9);
        let x = render_drums(&[n(KICK, 0.0), n(SNARE, 0.5), n(HAT_CLOSED, 0.25), n(60, 0.75)], 1.0, SR, &kit);
        assert!(x.iter().all(|v| v.is_finite()));
        assert!(peak(&x[..(0.2 * SR) as usize]) > 0.3);
        // Key 60 has no drum: it plays a tuned tom instead of staying silent.
        assert!(peak(&x[(0.78 * SR) as usize..(1.0 * SR) as usize]) > 0.2);
        for m in [CLAP, TOM_LOW, TOM_MID, TOM_HIGH, HAT_OPEN, RIM, COWBELL, RIDE, CRASH, SHAKER] {
            assert!(render_hit(&kit, m, 1.0, SR, 1).map(|h| peak(&h) > 0.05).unwrap_or(false), "Taste {m}");
        }
    }

    /// Energy of `x` at frequency `f` (Goertzel over the first 0.25 s).
    fn energy_at(x: &[f32], f: f32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, v) in x.iter().enumerate().take((0.25 * SR) as usize) {
            let w = 2.0 * PI * f * i as f32 / SR;
            re += v * w.cos();
            im += v * w.sin();
        }
        (re * re + im * im).sqrt()
    }

    #[test]
    fn drum_keys_of_the_kit_sound_exactly_as_before() {
        // Rhythm grid and drum tracks: a drum key plays its drum with the kit untouched.
        let kit = DrumKit { kick: KickParams { grundton: 30.0, ..DrumKit::default().kick }, ..DrumKit::default() };
        for m in [KICK, RIM, SNARE, CLAP, TOM_LOW, HAT_CLOSED, TOM_MID, HAT_OPEN, TOM_HIGH, CRASH, RIDE, COWBELL, SHAKER] {
            assert_eq!(voice_for(&kit, m as f32), (m, kit), "Taste {m}");
            assert_eq!(render_note(&kit, m as f32, 0.8, SR, 5), render_hit(&kit, m, 0.8, SR, 5).unwrap(), "Taste {m}");
        }
    }

    #[test]
    fn every_key_sounds_in_both_modes() {
        for pitched in [false, true] {
            let kit = DrumKit { pitched, ..DrumKit::default() };
            for m in 0..=127u8 {
                let x = render_note(&kit, m as f32, 0.8, SR, 7);
                assert!(x.iter().all(|v| v.is_finite()), "Taste {m}");
                assert!(peak(&x) > 0.1 && peak(&x) < 2.5, "Taste {m} (nach Tonhöhe: {pitched}): {}", peak(&x));
            }
        }
    }

    #[test]
    fn tuned_drums_split_the_keyboard_into_kick_tom_and_snare() {
        let kit = DrumKit { pitched: true, ..DrumKit::default() };
        let piece = |m: f32| voice_for(&kit, m).0;
        assert_eq!(piece(28.0), KICK);
        assert_eq!(piece(47.0), KICK);
        assert_eq!(piece(48.0), TOM_LOW);
        assert_eq!(piece(71.0), TOM_LOW);
        assert_eq!(piece(72.0), SNARE);
        assert_eq!(piece(96.0), SNARE);
        // By pitch the drum keys are tuned drums too: no hi-hat, no cymbal.
        assert_eq!(voice_for(&kit, HAT_OPEN as f32), (KICK, DrumKit { kick: KickParams { grundton: 46.0, ..kit.kick }, ..kit }));
        assert_eq!(piece(CRASH as f32), TOM_LOW);
        // The tuned drum keeps the kit's character, only its pitch follows the note.
        let (_, k) = voice_for(&kit, 64.0);
        assert_eq!(k.tom, TomParams { grundton: 64.0, ..kit.tom });
        // In the drum kit the free keys play the same tuned drums.
        let drum_kit = DrumKit::default();
        assert_eq!(voice_for(&drum_kit, 60.0), (TOM_LOW, DrumKit { tom: TomParams { grundton: 60.0, ..drum_kit.tom }, ..drum_kit }));
        assert_eq!(voice_for(&drum_kit, 30.0).0, KICK);
        assert_eq!(voice_for(&drum_kit, 76.0).0, SNARE);
    }

    #[test]
    fn tuned_drums_sound_at_the_pitch_of_the_note() {
        let kit = DrumKit { pitched: true, ..DrumKit::default() };
        for m in [28.0f32, 36.0, 43.0, 50.0, 57.0, 60.0, 64.0, 69.0, 74.0, 79.0] {
            let x = render_note(&kit, m, 1.0, SR, 3);
            let got = hit_pitches(&x, SR, &[0.0])[0].midi.unwrap_or(f32::NAN);
            assert!((got - m).abs() < 1.0, "Ton {m} → gemessen {got}");
        }
    }

    #[test]
    fn pitch_moves_the_spectrum_of_tuned_drums() {
        let kit = DrumKit { pitched: true, ..DrumKit::default() };
        // One drum per zone, a fifth apart: each has its energy at its own pitch.
        for (a, b) in [(31.0f32, 38.0f32), (55.0, 62.0), (74.0, 81.0)] {
            let (xa, xb) = (render_note(&kit, a, 1.0, SR, 3), render_note(&kit, b, 1.0, SR, 3));
            let (fa, fb) = (midi_to_hz(a), midi_to_hz(b));
            assert!(energy_at(&xa, fa) > 2.0 * energy_at(&xa, fb), "{a}: {} vs {}", energy_at(&xa, fa), energy_at(&xa, fb));
            assert!(energy_at(&xb, fb) > 2.0 * energy_at(&xb, fa), "{b}: {} vs {}", energy_at(&xb, fb), energy_at(&xb, fa));
        }
    }

    #[test]
    fn hats_by_pitch_are_not_choked() {
        // By pitch keys 46/42 are tuned drums: the first one rings out.
        let kit = DrumKit { pitched: true, ..DrumKit::default() };
        let n = |midi: u8, t: f32| NoteSpec::new(t, 0.1, midi as f32, 1.0);
        let alone = render_drums(&[n(HAT_OPEN, 0.0)], 1.0, SR, &kit);
        let both = render_drums(&[n(HAT_OPEN, 0.0), n(HAT_CLOSED, 0.2)], 1.0, SR, &kit);
        let tail = |x: &[f32]| peak(&x[(0.3 * SR) as usize..(0.4 * SR) as usize]);
        assert!(tail(&both) > 0.5 * tail(&alone), "{} {}", tail(&both), tail(&alone));
    }

    #[test]
    fn new_pieces_have_their_character() {
        let kit = DrumKit::default();
        let len = |m: u8| render_hit(&kit, m, 1.0, SR, 3).unwrap().len() as f32 / SR;
        // Rim and shaker short, cymbals long, crash longer than ride
        assert!(len(RIM) < 0.1 && len(SHAKER) < 0.2);
        assert!(len(RIDE) > 0.7 && len(CRASH) > len(RIDE));
        for m in [RIM, COWBELL, RIDE, CRASH, SHAKER] {
            let x = render_hit(&kit, m, 1.0, SR, 3).unwrap();
            assert!(x.iter().all(|v| v.is_finite()) && peak(&x) < 2.5, "Taste {m}");
        }
        // Cowbell: energy at 540 and 800 Hz, hardly any in between
        let cb = cowbell(SR, &kit.hat, 1.0, 1);
        let mag = |f: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, v) in cb.iter().enumerate().take((0.2 * SR) as usize) {
                let w = 2.0 * PI * f * i as f32 / SR;
                re += v * w.cos();
                im += v * w.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(mag(540.0) > 5.0 * mag(670.0) && mag(800.0) > 5.0 * mag(670.0), "{} {} {}", mag(540.0), mag(800.0), mag(670.0));
        assert!(piece_name(RIM).is_some() && piece_name(SHAKER).is_some());
    }

    #[test]
    fn open_hat_is_choked_by_next_hat() {
        let kit = DrumKit { hat: HatParams { offen: 1.0, ..DrumKit::default().hat }, ..DrumKit::default() };
        let n = |midi: u8, t: f32| NoteSpec::new(t, 0.1, midi as f32, 1.0);
        let alone = render_drums(&[n(HAT_OPEN, 0.0)], 1.0, SR, &kit);
        let choked = render_drums(&[n(HAT_OPEN, 0.0), n(HAT_CLOSED, 0.2)], 1.0, SR, &kit);
        let tail = |x: &[f32]| peak(&x[(0.5 * SR) as usize..(0.6 * SR) as usize]);
        assert!(tail(&choked) < 0.2 * tail(&alone));
    }
}
