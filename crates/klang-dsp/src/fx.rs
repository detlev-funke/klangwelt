//! Klangformung nach der Klangerzeugung – für alle Instrumente gleich:
//!
//! * **Wärme** – mehr Fülle in den unteren Mitten, weichere Höhen und eine
//!   sanfte Röhren-Sättigung (geradzahlige Obertöne), Pegel bleibt gleich.
//! * **Schwebung** – zwei leicht verstimmte, langsam wandernde Kopien
//!   (Chorus): der Ton wird breiter und lebendiger, wie mehrere Saiten oder
//!   Pfeifen, die nie ganz gleich gestimmt sind.
//!
//! Der **Raum** (Hall) liegt in der Engine, weil er in Stereo klingt und
//! über alle Spuren hinweg ausklingt.

use crate::synth::Biquad;
use std::f32::consts::PI;

/// Wärme 0…1 auf einen Mono-Puffer anwenden.
pub fn warmth(buf: &mut [f32], amount: f32, sr: f32) {
    let a = amount.clamp(0.0, 1.0);
    if a < 1e-3 || buf.is_empty() {
        return;
    }
    let mut low = Biquad::lowshelf(220.0, 3.5 * a, sr);
    let mut body = Biquad::peaking(450.0, 0.8, 1.5 * a, sr);
    let mut high = Biquad::highshelf(5200.0, -6.0 * a, sr);
    // Sättigung: asymmetrische weiche Kennlinie (Röhre) – Pegel davor/danach ausgleichen
    let drive = 1.0 + 2.5 * a;
    let bias = 0.12 * a;
    let shape = |x: f32| ((x + bias) * drive).tanh() - (bias * drive).tanh();
    // Steigung im Nullpunkt = 1: leise Stellen bleiben gleich laut
    let norm = drive * (1.0 - (bias * drive).tanh().powi(2));
    let mix = 0.35 + 0.4 * a;
    let mut dc_x = 0.0f32;
    let mut dc_y = 0.0f32;
    let r = 1.0 - 2.0 * PI * 12.0 / sr;
    for v in buf.iter_mut() {
        let x = high.process(body.process(low.process(*v)));
        let sat = shape(x) / norm;
        let y = x * (1.0 - mix) + sat * mix;
        // Gleichanteil der asymmetrischen Kennlinie entfernen
        let out = y - dc_x + r * dc_y;
        dc_x = y;
        dc_y = out;
        *v = out;
    }
}

/// Schwebung 0…1 (Chorus) auf einen Mono-Puffer anwenden.
pub fn chorus(buf: &mut [f32], amount: f32, sr: f32) {
    let a = amount.clamp(0.0, 1.0);
    if a < 1e-3 || buf.is_empty() {
        return;
    }
    let dry: Vec<f32> = buf.to_vec();
    // zwei Stimmen mit verschiedenen, nicht zusammenpassenden Geschwindigkeiten
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
        // Pegel etwa gleich lassen
        *v = (dry[n] + wet * 0.5 * s) / (1.0 + 0.35 * wet);
    }
}

/// Farbe 0…1 (0,5 = neutral): Kippfilter – dunkel senkt die Höhen (bis −14 dB)
/// und hebt die Tiefen etwas an, hell hebt die Höhen (bis +8 dB) und nimmt
/// etwas Tiefe weg.
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

/// Rauheit 0…1: schnelles Flattern der Lautstärke (30–80 Hz – das Ohr hört es
/// als rau, schnarrend), Sättigung und Körnung, die dem Pegel folgt.
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
        // Körnung ohne Tiefen (Differenz = einfacher Hochpass)
        let grit = (noise - prev_noise) * 0.5 * env * 0.35 * a;
        prev_noise = noise;
        let sat = (x * drive).tanh() / drive;
        let shaped = (x * (1.0 - mix) + sat * mix) * makeup + grit;
        let t = i as f32 / sr;
        *v = shaped * (1.0 - depth * 0.5 * (1.0 - (2.0 * PI * am_f * t).sin()));
    }
}

/// Entfernung aus dem Regler „Raum“: ab etwa einem Drittel klingt der direkte
/// Schall leiser und dumpfer (Luft schluckt Höhen) – den Hall fügt die Engine hinzu.
pub fn distance(buf: &mut [f32], raum: f32, sr: f32) {
    let d = ((raum.clamp(0.0, 1.0) - 0.3) / 0.7).max(0.0);
    if d < 0.01 || buf.is_empty() {
        return;
    }
    // unter der halben Abtastrate bleiben (sonst wird das Filter instabil)
    let mut lp = Biquad::lowpass((16_000.0 * (1.0 - 0.75 * d)).min(sr * 0.45), 0.707, sr);
    let gain = 1.0 - 0.4 * d;
    for v in buf.iter_mut() {
        *v = lp.process(*v) * gain;
    }
}

/// Breite 0…1: Seitensignal (Stereo) aus einer kurz verzögerten Kopie, ohne
/// Tiefen – Bassdrum und Bass bleiben in der Mitte, in Mono hebt sich die
/// Seite auf (links = Mitte + Seite, rechts = Mitte − Seite). `None` = schmal.
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
        // auch bei niedriger Abtastrate stabil
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

    /// Stärke eines Teiltons (DFT an einer Frequenz).
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
        // Sättigung: zweiter Teilton entsteht
        let pure = sine(200.0, 1.0, sr, 0.5);
        let mut sat = pure.clone();
        warmth(&mut sat, 1.0, sr);
        assert!(amp(&sat[s..e], 400.0, sr) > 10.0 * amp(&pure[s..e], 400.0, sr) + 1e-4);
        // aus: unverändert
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
        // Grundton bleibt, Pegel ähnlich – aber der Ton schwankt (Hüllkurve nicht mehr konstant)
        assert!(amp(&ch[s..e], 440.0, sr) > 0.5 * amp(&tone[s..e], 440.0, sr));
        let env = |x: &[f32]| -> Vec<f32> { x[s..e].chunks(2400).map(rms).collect() };
        let spread = |v: Vec<f32>| v.iter().cloned().fold(0.0f32, f32::max) - v.iter().cloned().fold(1.0f32, f32::min);
        assert!(spread(env(&ch)) > spread(env(&tone)) + 0.01, "lebendige Schwebung");
        assert!(ch.iter().all(|v| v.abs() < 1.0));
    }
}
