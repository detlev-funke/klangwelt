//! Tonhöhenerkennung nach YIN (de Cheveigné & Kawahara, 2002).
//!
//! Die Differenzfunktion wird über eine FFT-Kreuzkorrelation berechnet,
//! dadurch kostet ein Analyseframe O(N log N) statt O(N²).

use crate::fft::{Complex, Fft};
use crate::synth::Biquad;
use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct YinConfig {
    /// Gesamtlänge eines Analyseframes in Samples (Integrationsfenster = Hälfte).
    pub frame_len: usize,
    pub hop: usize,
    pub min_freq: f32,
    pub max_freq: f32,
    /// Schwelle der normierten Differenzfunktion (kleiner = strenger).
    pub threshold: f32,
    /// Frames unterhalb dieses RMS-Pegels gelten als Stille.
    pub silence_rms: f32,
}

impl Default for YinConfig {
    /// Für Melodien: Gesang, Melodieinstrumente und Bass (bis E1 ≈ 41 Hz).
    fn default() -> Self {
        Self {
            frame_len: 2560,
            hop: 256,
            min_freq: 40.0,
            max_freq: 1400.0,
            threshold: 0.15,
            silence_rms: 0.005,
        }
    }
}

impl YinConfig {
    /// Für vorgesungene oder gepfiffene Melodien: Männerstimme bis Pfiff
    /// (≈ 60 Hz … 3,2 kHz – Pfeifen liegt meist bei 0,9–2,8 kHz).
    pub fn voice_and_whistle() -> Self {
        Self { min_freq: 60.0, max_freq: 3200.0, ..Self::default() }
    }

    /// Für Anschläge (Schlagzeug, gezupfte Bässe): tiefer, toleranter, längeres Fenster.
    pub fn percussive() -> Self {
        Self {
            frame_len: 4096,
            hop: 512,
            min_freq: 28.0,
            max_freq: 2000.0,
            threshold: 0.3,
            silence_rms: 0.002,
        }
    }
}

/// Grundtonhöhe eines einzelnen Anschlags.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitPitch {
    /// Einsatz in Sekunden.
    pub time: f32,
    /// Ende des ausgewerteten Abschnitts (nächster Einsatz, höchstens 0,4 s später).
    pub end: f32,
    /// Grundton als MIDI-Wert (gebrochen), `None` bei reinem Geräusch (z. B. Hi-Hat).
    pub midi: Option<f32>,
    /// 0…1 – wie eindeutig die Tonhöhe ist.
    pub clarity: f32,
    /// 0…1 – Lautstärke relativ zum lautesten Anschlag.
    pub strength: f32,
}

/// Bestimmt für jeden Einsatz den Grundton des folgenden Abschnitts.
///
/// Anders als die Melodie-Erkennung verlangt das keinen lange gleichbleibenden
/// Ton: Bassdrum (mit fallender Tonhöhe), Snare-Kessel, Toms oder gezupfte
/// Basstöne bekommen so ihren Grundton – energiegewichtet über den Abschnitt.
pub fn hit_pitches(samples: &[f32], sample_rate: f32, onsets: &[f32]) -> Vec<HitPitch> {
    let cfg = YinConfig::percussive();
    let len = samples.len();
    let max_peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    let total_secs = len as f32 / sample_rate;
    onsets
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            let end = onsets.get(i + 1).copied().unwrap_or(f32::MAX).min(t + 0.4).min(total_secs);
            // Den ersten Knack (Schlägel, Stick) überspringen.
            let s = (((t + 0.008) * sample_rate) as usize).min(len);
            let e = ((end * sample_rate) as usize).clamp(s, len);
            let seg = &samples[s..e];
            let strength = seg.iter().fold(0.0f32, |m, v| m.max(v.abs())) / max_peak;
            // Tiefpass (4. Ordnung, 1,2 kHz): Teppich, Becken und Stick-Geräusch
            // überdecken sonst den Grundton von Kessel, Bassdrum oder Bass.
            let mut lp1 = Biquad::lowpass(1200.0, 0.707, sample_rate);
            let mut lp2 = Biquad::lowpass(1200.0, 0.707, sample_rate);
            let mut buf: Vec<f32> = seg.iter().map(|&v| lp2.process(lp1.process(v))).collect();
            if buf.len() < cfg.frame_len {
                buf.resize(cfg.frame_len, 0.0);
            }
            let frames = detect_pitch(&buf, sample_rate, &cfg);
            let mut voiced: Vec<(f32, f32)> = frames
                .iter()
                .filter_map(|f| f.freq.map(|hz| (crate::notes::hz_to_midi(hz), f.rms * f.clarity)))
                .collect();
            let clarity = if voiced.is_empty() {
                0.0
            } else {
                frames.iter().filter(|f| f.freq.is_some()).map(|f| f.clarity).sum::<f32>() / voiced.len() as f32
            };
            let midi = weighted_median(&mut voiced);
            HitPitch { time: t, end, midi, clarity, strength }
        })
        .collect()
}

fn weighted_median(values: &mut [(f32, f32)]) -> Option<f32> {
    let total: f32 = values.iter().map(|v| v.1).sum();
    if values.is_empty() || total <= 0.0 {
        return None;
    }
    values.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut acc = 0.0;
    for &(v, w) in values.iter() {
        acc += w;
        if acc >= total * 0.5 {
            return Some(v);
        }
    }
    values.last().map(|v| v.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PitchFrame {
    /// Mittelpunkt des Frames in Sekunden.
    pub time: f32,
    /// Grundfrequenz in Hz, `None` bei Stille oder unsicherer Erkennung.
    pub freq: Option<f32>,
    /// 0…1 – wie periodisch das Signal ist (1 − normierte Differenz).
    pub clarity: f32,
    pub rms: f32,
}

pub fn detect_pitch(samples: &[f32], sample_rate: f32, cfg: &YinConfig) -> Vec<PitchFrame> {
    let w = cfg.frame_len / 2;
    let tau_max = ((sample_rate / cfg.min_freq).ceil() as usize).min(w).max(4);
    let tau_min = ((sample_rate / cfg.max_freq).floor() as usize).max(2);
    if tau_min + 2 >= tau_max {
        return Vec::new();
    }
    let span = w + tau_max;
    if samples.len() < span || cfg.hop == 0 {
        return Vec::new();
    }

    let n = span.next_power_of_two();
    let fft = Fft::new(n);
    let mut a = vec![Complex::ZERO; n];
    let mut b = vec![Complex::ZERO; n];
    let mut d = vec![0f32; tau_max + 1];
    let mut cm = vec![0f32; tau_max + 1];
    let mut prefix = vec![0f64; span + 1];
    let mut frames = Vec::with_capacity(samples.len() / cfg.hop + 1);

    let mut start = 0;
    while start + span <= samples.len() {
        let x = &samples[start..start + span];
        let time = (start as f32 + span as f32 * 0.5) / sample_rate;

        for i in 0..span {
            prefix[i + 1] = prefix[i] + (x[i] as f64) * (x[i] as f64);
        }
        let rms = (prefix[w] / w as f64).sqrt() as f32;
        if rms < cfg.silence_rms {
            frames.push(PitchFrame { time, freq: None, clarity: 0.0, rms });
            start += cfg.hop;
            continue;
        }

        // r(τ) = Σ_j x[j]·x[j+τ] per FFT-Kreuzkorrelation.
        for i in 0..n {
            a[i] = if i < w { Complex::new(x[i], 0.0) } else { Complex::ZERO };
            b[i] = if i < span { Complex::new(x[i], 0.0) } else { Complex::ZERO };
        }
        fft.forward(&mut a);
        fft.forward(&mut b);
        for i in 0..n {
            a[i] = a[i].conj() * b[i];
        }
        fft.inverse(&mut a);

        // d(τ) = E(0) + E(τ) − 2·r(τ)
        let e0 = prefix[w];
        for tau in 0..=tau_max {
            let et = prefix[tau + w] - prefix[tau];
            d[tau] = (e0 + et - 2.0 * a[tau].re as f64).max(0.0) as f32;
        }

        // Kumulativ normierte Differenzfunktion.
        cm[0] = 1.0;
        let mut running = 0f64;
        for tau in 1..=tau_max {
            running += d[tau] as f64;
            cm[tau] = if running > 0.0 {
                (d[tau] as f64 * tau as f64 / running) as f32
            } else {
                1.0
            };
        }

        // Erstes Minimum unter der Schwelle suchen.
        let mut found = None;
        let mut tau = tau_min;
        while tau < tau_max {
            if cm[tau] < cfg.threshold {
                while tau + 1 < tau_max && cm[tau + 1] < cm[tau] {
                    tau += 1;
                }
                found = Some(tau);
                break;
            }
            tau += 1;
        }
        let (best, voiced) = match found {
            Some(t) => (t, true),
            None => {
                let t = (tau_min..tau_max)
                    .min_by(|&p, &q| cm[p].total_cmp(&cm[q]))
                    .unwrap_or(tau_min);
                (t, false)
            }
        };

        let refined = parabolic(&cm, best);
        let clarity = (1.0 - cm[best]).clamp(0.0, 1.0);
        let freq = if voiced && refined > 0.0 {
            Some(sample_rate / refined)
        } else {
            None
        };
        frames.push(PitchFrame { time, freq, clarity, rms });
        start += cfg.hop;
    }
    frames
}

fn parabolic(v: &[f32], i: usize) -> f32 {
    if i == 0 || i + 1 >= v.len() {
        return i as f32;
    }
    let (a, b, c) = (v[i - 1], v[i], v[i + 1]);
    let den = a - 2.0 * b + c;
    if den.abs() < 1e-12 {
        i as f32
    } else {
        i as f32 + 0.5 * (a - c) / den
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::sine;
    use crate::util::median;

    fn median_freq(frames: &[PitchFrame]) -> f32 {
        let mut f: Vec<f32> = frames.iter().filter_map(|f| f.freq).collect();
        assert!(!f.is_empty(), "keine Tonhöhe erkannt");
        median(&mut f)
    }

    #[test]
    fn detects_sine_frequencies() {
        for &freq in &[82.4f32, 220.0, 440.0, 987.8] {
            let x = sine(freq, 0.5, 44_100.0, 0.5);
            let frames = detect_pitch(&x, 44_100.0, &YinConfig::default());
            let est = median_freq(&frames);
            assert!((est - freq).abs() / freq < 0.005, "{freq} Hz → {est} Hz");
        }
    }

    #[test]
    fn harmonic_tone_reports_fundamental() {
        let sr = 48_000.0;
        let f0 = 196.0;
        let x: Vec<f32> = (0..(sr * 0.5) as usize)
            .map(|i| {
                let t = i as f32 / sr;
                let w = 2.0 * std::f32::consts::PI * f0 * t;
                0.3 * w.sin() + 0.5 * (2.0 * w).sin() + 0.3 * (3.0 * w).sin()
            })
            .collect();
        let est = median_freq(&detect_pitch(&x, sr, &YinConfig::default()));
        assert!((est - f0).abs() / f0 < 0.01, "{est}");
    }

    #[test]
    fn bass_down_to_low_e() {
        let x = sine(41.2, 0.6, 44_100.0, 0.5);
        let est = median_freq(&detect_pitch(&x, 44_100.0, &YinConfig::default()));
        assert!((est - 41.2).abs() / 41.2 < 0.01, "{est}");
    }

    #[test]
    fn drum_hits_get_their_fundamental() {
        use crate::synth::{hat, kick, mix_into, snare};
        let sr = 48_000.0;
        let mut x = vec![0.0f32; (2.0 * sr) as usize];
        mix_into(&mut x, &kick(sr), 0, 1.0);
        mix_into(&mut x, &snare(sr, 3), (0.5 * sr) as usize, 0.8);
        mix_into(&mut x, &hat(sr, 5, false), (1.0 * sr) as usize, 0.5);
        let hits = hit_pitches(&x, sr, &[0.0, 0.5, 1.0]);
        let kick_midi = hits[0].midi.expect("Bassdrum hat einen Grundton");
        // Synth-Kick fällt von 130 auf 45 Hz und klingt dort aus – Grundton ≈ 45 Hz (MIDI ≈ 30,3).
        assert!((29.0..34.0).contains(&kick_midi), "Bassdrum {kick_midi}");
        let snare_midi = hits[1].midi.expect("Snare hat einen Kesselton");
        assert!((snare_midi - 54.4).abs() < 1.0, "Snare {snare_midi} (185 Hz ≈ 54,4)");
        assert!(hits[2].midi.is_none() || hits[2].clarity < 0.6, "Hi-Hat sollte tonlos sein: {:?}", hits[2]);
        assert!(hits[0].strength > hits[2].strength);
    }

    #[test]
    fn silence_is_unvoiced() {
        let x = vec![0.0f32; 44_100];
        let frames = detect_pitch(&x, 44_100.0, &YinConfig::default());
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|f| f.freq.is_none()));
    }
}
