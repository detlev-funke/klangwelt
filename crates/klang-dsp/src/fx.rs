//! Sound shaping after sound generation – the same for all instruments:
//!
//! * **Warmth** – more fullness in the low mids, softer highs and a gentle
//!   tube saturation (even-order harmonics), level stays the same.
//! * **Shimmer** – two slightly detuned, slowly drifting copies (chorus):
//!   the tone becomes wider and livelier, like several strings or pipes
//!   that are never tuned quite the same.
//!
//! The **room** (reverb) lives in the engine, because it sounds in stereo and
//! rings out across all tracks.

use crate::synth::Biquad;
use std::f32::consts::PI;

/// Apply warmth 0…1 to a mono buffer.
pub fn warmth(buf: &mut [f32], amount: f32, sr: f32) {
    let a = amount.clamp(0.0, 1.0);
    if a < 1e-3 || buf.is_empty() {
        return;
    }
    let mut low = Biquad::lowshelf(220.0, 3.5 * a, sr);
    let mut body = Biquad::peaking(450.0, 0.8, 1.5 * a, sr);
    let mut high = Biquad::highshelf(5200.0, -6.0 * a, sr);
    // Saturation: asymmetric soft curve (tube) – compensate the level before/after
    let drive = 1.0 + 2.5 * a;
    let bias = 0.12 * a;
    let shape = |x: f32| ((x + bias) * drive).tanh() - (bias * drive).tanh();
    // Slope at the origin = 1: quiet passages stay equally loud
    let norm = drive * (1.0 - (bias * drive).tanh().powi(2));
    let mix = 0.35 + 0.4 * a;
    let mut dc_x = 0.0f32;
    let mut dc_y = 0.0f32;
    let r = 1.0 - 2.0 * PI * 12.0 / sr;
    for v in buf.iter_mut() {
        let x = high.process(body.process(low.process(*v)));
        let sat = shape(x) / norm;
        let y = x * (1.0 - mix) + sat * mix;
        // remove the DC offset of the asymmetric curve
        let out = y - dc_x + r * dc_y;
        dc_x = y;
        dc_y = out;
        *v = out;
    }
}

/// Apply shimmer 0…1 (chorus) to a mono buffer.
pub fn chorus(buf: &mut [f32], amount: f32, sr: f32) {
    let a = amount.clamp(0.0, 1.0);
    if a < 1e-3 || buf.is_empty() {
        return;
    }
    let dry: Vec<f32> = buf.to_vec();
    // two voices with different, non-matching speeds
    let voices = [(0.011, 0.47, 0.0f32), (0.017, 0.71, 1.9f32)];
    let depth = (0.0006 + 0.0024 * a) * sr;
    let wet = 0.22 + 0.33 * a;
    let at = |t: f32| -> f32 {
        if t < 0.0 {
            return 0.0;
        }
        let i = t as usize;
        let f = t - i as f32;
        let x0 = dry.get(i).copied().unwrap_or(0.0);
        let x1 = dry.get(i + 1).copied().unwrap_or(0.0);
        x0 + (x1 - x0) * f
    };
    for (n, v) in buf.iter_mut().enumerate() {
        let t = n as f32 / sr;
        let mut s = 0.0;
        for &(base, rate, phase) in &voices {
            let d = base * sr + depth * (0.5 + 0.5 * (2.0 * PI * rate * t + phase).sin());
            s += at(n as f32 - d);
        }
        // keep the level roughly the same
        *v = (dry[n] + wet * 0.5 * s) / (1.0 + 0.35 * wet);
    }
}

/// Color 0…1 (0.5 = neutral): tilt filter – dark lowers the highs (down to −14 dB)
/// and raises the lows somewhat, bright raises the highs (up to +8 dB) and takes
/// away some low end.
pub fn tone(buf: &mut [f32], farbe: f32, sr: f32) {
    let f = farbe.clamp(0.0, 1.0) - 0.5;
    if f.abs() < 0.01 || buf.is_empty() {
        return;
    }
    let hi_db = if f < 0.0 { 28.0 * f } else { 16.0 * f };
    let mut lo = Biquad::lowshelf(250.0, -6.0 * f, sr);
    let mut hi = Biquad::highshelf(2500.0, hi_db, sr);
    for v in buf.iter_mut() {
        *v = hi.process(lo.process(*v));
    }
}

/// Roughness 0…1: fast fluttering of the volume (30–80 Hz – the ear hears it
/// as rough, rasping), saturation and graininess that follows the level.
pub fn roughness(buf: &mut [f32], amount: f32, sr: f32) {
    let a = amount.clamp(0.0, 1.0);
    if a < 0.01 || buf.is_empty() {
        return;
    }
    let (am_f, depth) = (30.0 + 50.0 * a, 0.55 * a);
    let drive = 1.0 + 6.0 * a * a;
    let mix = 0.7 * a;
    let makeup = 1.0 + 0.6 * a;
    let (att, rel) = ((-1.0 / (0.001 * sr)).exp(), (-1.0 / (0.03 * sr)).exp());
    let mut env = 0.0f32;
    let mut seed: u32 = 0x9E37_79B9;
    let mut prev_noise = 0.0f32;
    for (i, v) in buf.iter_mut().enumerate() {
        let x = *v;
        let level = x.abs();
        env = if level > env { att * env + (1.0 - att) * level } else { rel * env + (1.0 - rel) * level };
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
        // graininess without lows (difference = simple high-pass)
        let grit = (noise - prev_noise) * 0.5 * env * 0.35 * a;
        prev_noise = noise;
        let sat = (x * drive).tanh() / drive;
        let shaped = (x * (1.0 - mix) + sat * mix) * makeup + grit;
        let t = i as f32 / sr;
        *v = shaped * (1.0 - depth * 0.5 * (1.0 - (2.0 * PI * am_f * t).sin()));
    }
}

/// Distance from the "Room" control: from about a third on, the direct sound
/// gets quieter and duller (air swallows highs) – the reverb is added by the engine.
pub fn distance(buf: &mut [f32], raum: f32, sr: f32) {
    let d = ((raum.clamp(0.0, 1.0) - 0.3) / 0.7).max(0.0);
    if d < 0.01 || buf.is_empty() {
        return;
    }
    // stay below half the sample rate (otherwise the filter becomes unstable)
    let mut lp = Biquad::lowpass((16_000.0 * (1.0 - 0.75 * d)).min(sr * 0.45), 0.707, sr);
    let gain = 1.0 - 0.4 * d;
    for v in buf.iter_mut() {
        *v = lp.process(*v) * gain;
    }
}

/// Width 0…1: side signal (stereo) from a briefly delayed copy, without
/// lows – bass drum and bass stay in the center, in mono the side
/// cancels out (left = mid + side, right = mid − side). `None` = narrow.
pub fn widen(mid: &[f32], breite: f32, sr: f32) -> Option<Vec<f32>> {
    let b = breite.clamp(0.0, 1.0);
    if b < 0.02 || mid.is_empty() {
        return None;
    }
    let d = (0.013 * sr) as usize;
    let mut hp = Biquad::highpass(220.0, 0.707, sr);
    let mut hp2 = Biquad::highpass(220.0, 0.707, sr);
    let gain = 0.6 * b;
    Some((0..mid.len()).map(|i| hp2.process(hp.process(if i >= d { mid[i - d] } else { 0.0 })) * gain).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::sine;

    #[test]
    fn colour_tilts_the_spectrum_and_neutral_keeps_it() {
        let sr = 48_000.0;
        let mix: Vec<f32> = sine(150.0, 1.0, sr, 0.25).iter().zip(sine(8000.0, 1.0, sr, 0.25)).map(|(a, b)| a + b).collect();
        let (s, e) = ((0.2 * sr) as usize, (0.9 * sr) as usize);
        let ratio = |x: &[f32]| amp(&x[s..e], 8000.0, sr) / amp(&x[s..e], 150.0, sr);
        let (mut dark, mut bright, mut neutral) = (mix.clone(), mix.clone(), mix.clone());
        tone(&mut dark, 0.0, sr);
        tone(&mut bright, 1.0, sr);
        tone(&mut neutral, 0.5, sr);
        assert!(ratio(&dark) < ratio(&mix) * 0.3, "dunkel");
        assert!(ratio(&bright) > ratio(&mix) * 2.0, "hell");
        assert_eq!(neutral, mix);
    }

    #[test]
    fn roughness_makes_the_level_flutter() {
        let sr = 48_000.0;
        let tone_in = sine(220.0, 1.0, sr, 0.4);
        let mut rough = tone_in.clone();
        roughness(&mut rough, 1.0, sr);
        let blocks = |x: &[f32]| -> f32 {
            let b: Vec<f32> = x[4800..43_200].chunks(120).map(rms).collect();
            b.iter().cloned().fold(0.0f32, f32::max) - b.iter().cloned().fold(1.0f32, f32::min)
        };
        assert!(blocks(&rough) > blocks(&tone_in) + 0.1, "Flattern");
        assert!(rough.iter().all(|v| v.is_finite() && v.abs() < 1.2));
        let mut off = tone_in.clone();
        roughness(&mut off, 0.0, sr);
        assert_eq!(off, tone_in);
    }

    #[test]
    fn distance_is_quieter_and_duller_only_when_far() {
        let sr = 48_000.0;
        let mix: Vec<f32> = sine(200.0, 1.0, sr, 0.25).iter().zip(sine(9000.0, 1.0, sr, 0.25)).map(|(a, b)| a + b).collect();
        let (mut near, mut far) = (mix.clone(), mix.clone());
        distance(&mut near, 0.2, sr);
        distance(&mut far, 1.0, sr);
        assert_eq!(near, mix);
        // stable even at a low sample rate
        let mut low = sine(300.0, 1.0, 16_000.0, 0.3);
        distance(&mut low, 0.5, 16_000.0);
        assert!(low.iter().all(|v| v.is_finite() && v.abs() < 1.0));
        let (s, e) = ((0.2 * sr) as usize, (0.9 * sr) as usize);
        assert!(rms(&far[s..e]) < rms(&mix[s..e]) * 0.8);
        assert!(amp(&far[s..e], 9000.0, sr) < 0.3 * amp(&mix[s..e], 9000.0, sr));
    }

    #[test]
    fn width_leaves_the_lows_in_the_middle() {
        let sr = 48_000.0;
        assert!(widen(&sine(500.0, 0.5, sr, 0.3), 0.0, sr).is_none());
        let low = widen(&sine(60.0, 1.0, sr, 0.3), 1.0, sr).unwrap();
        let high = widen(&sine(3000.0, 1.0, sr, 0.3), 1.0, sr).unwrap();
        let (s, e) = ((0.2 * sr) as usize, (0.9 * sr) as usize);
        assert!(rms(&low[s..e]) < 0.03, "Tiefen bleiben mittig: {}", rms(&low[s..e]));
        assert!(rms(&high[s..e]) > 0.08, "Höhen werden breit: {}", rms(&high[s..e]));
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Strength of a partial (DFT at one frequency).
    fn amp(x: &[f32], f: f32, sr: f32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, v) in x.iter().enumerate() {
            let w = 2.0 * PI * f * i as f32 / sr;
            re += v * w.cos();
            im -= v * w.sin();
        }
        (re * re + im * im).sqrt() / x.len() as f32
    }

    #[test]
    fn warmth_fills_the_lows_softens_the_highs_and_keeps_the_level() {
        let sr = 48_000.0;
        let mix: Vec<f32> = sine(150.0, 1.0, sr, 0.25).iter().zip(sine(8000.0, 1.0, sr, 0.25)).map(|(a, b)| a + b).collect();
        let mut warm = mix.clone();
        warmth(&mut warm, 1.0, sr);
        let (s, e) = ((0.2 * sr) as usize, (0.9 * sr) as usize);
        let ratio = |x: &[f32]| amp(&x[s..e], 150.0, sr) / amp(&x[s..e], 8000.0, sr);
        assert!(ratio(&warm) > ratio(&mix) * 2.0, "Tiefen gegenüber Höhen: {} → {}", ratio(&mix), ratio(&warm));
        let (r0, r1) = (rms(&mix[s..e]), rms(&warm[s..e]));
        assert!((r1 / r0 - 1.0).abs() < 0.35, "Pegel etwa gleich: {r0} → {r1}");
        // saturation: a second partial appears
        let pure = sine(200.0, 1.0, sr, 0.5);
        let mut sat = pure.clone();
        warmth(&mut sat, 1.0, sr);
        assert!(amp(&sat[s..e], 400.0, sr) > 10.0 * amp(&pure[s..e], 400.0, sr) + 1e-4);
        // off: unchanged
        let mut off = mix.clone();
        warmth(&mut off, 0.0, sr);
        assert_eq!(off, mix);
    }

    #[test]
    fn chorus_widens_the_spectrum_around_the_tone() {
        let sr = 48_000.0;
        let tone = sine(440.0, 2.0, sr, 0.3);
        let mut ch = tone.clone();
        chorus(&mut ch, 1.0, sr);
        let (s, e) = ((0.3 * sr) as usize, (1.9 * sr) as usize);
        // fundamental stays, level similar – but the tone wavers (envelope no longer constant)
        assert!(amp(&ch[s..e], 440.0, sr) > 0.5 * amp(&tone[s..e], 440.0, sr));
        let env = |x: &[f32]| -> Vec<f32> { x[s..e].chunks(2400).map(rms).collect() };
        let spread = |v: Vec<f32>| v.iter().cloned().fold(0.0f32, f32::max) - v.iter().cloned().fold(1.0f32, f32::min);
        assert!(spread(env(&ch)) > spread(env(&tone)) + 0.01, "lebendige Schwebung");
        assert!(ch.iter().all(|v| v.abs() < 1.0));
    }
}
