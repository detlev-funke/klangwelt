//! Einsatzerkennung (Spectral Flux) und Tempo-Schätzung (Autokorrelation).

use crate::fft::{hann, Complex, Fft};
use crate::util::median;
use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct OnsetConfig {
    pub frame_len: usize,
    pub hop: usize,
    /// Abstand über dem lokalen Median, ab dem ein Peak ein Einsatz ist.
    pub delta: f32,
    pub median_radius: usize,
    pub min_interval_secs: f32,
}

impl Default for OnsetConfig {
    fn default() -> Self {
        Self {
            frame_len: 1024,
            hop: 256,
            delta: 0.08,
            median_radius: 6,
            min_interval_secs: 0.07,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnsetAnalysis {
    pub hop_secs: f32,
    /// Normierte Novelty-Kurve (0…1), ein Wert pro Hop.
    pub envelope: Vec<f32>,
    /// Einsatzzeiten in Sekunden.
    pub onsets: Vec<f32>,
}

pub fn detect_onsets(samples: &[f32], sample_rate: f32, cfg: &OnsetConfig) -> OnsetAnalysis {
    let n = cfg.frame_len.next_power_of_two().max(2);
    let hop_secs = cfg.hop as f32 / sample_rate;
    if samples.is_empty() || cfg.hop == 0 {
        return OnsetAnalysis { hop_secs, ..Default::default() };
    }
    // Stille voranstellen: Ein Schlag ganz am Anfang braucht ein „Vorher“ zum Vergleich.
    let lead = n;
    let mut padded = vec![0.0f32; lead + samples.len().max(n)];
    padded[lead..lead + samples.len()].copy_from_slice(samples);
    let samples = &padded[..];
    let lead_secs = lead as f32 / sample_rate;
    let fft = Fft::new(n);
    let win = hann(n);
    let bins = n / 2 + 1;
    let mut buf = vec![Complex::ZERO; n];
    let mut prev = vec![0f32; bins];
    let mut cur = vec![0f32; bins];
    let mut env = Vec::with_capacity(samples.len() / cfg.hop + 1);
    let mut first = true;

    let mut start = 0;
    while start + n <= samples.len() {
        for i in 0..n {
            buf[i] = Complex::new(samples[start + i] * win[i], 0.0);
        }
        fft.forward(&mut buf);
        let mut flux = 0.0;
        for k in 0..bins {
            cur[k] = (1.0 + 10.0 * buf[k].norm()).ln();
            if !first {
                let d = cur[k] - prev[k];
                if d > 0.0 {
                    flux += d;
                }
            }
        }
        env.push(flux);
        std::mem::swap(&mut prev, &mut cur);
        first = false;
        start += cfg.hop;
    }

    let max = env.iter().copied().fold(0.0f32, f32::max);
    if max > 0.0 {
        for v in env.iter_mut() {
            *v /= max;
        }
    }

    let mut onsets = Vec::new();
    let mut last = f32::NEG_INFINITY;
    let mut window = Vec::with_capacity(2 * cfg.median_radius + 1);
    for i in 1..env.len().saturating_sub(1) {
        let v = env[i];
        if !(v > env[i - 1] && v >= env[i + 1]) {
            continue;
        }
        let lo = i.saturating_sub(cfg.median_radius);
        let hi = (i + cfg.median_radius + 1).min(env.len());
        window.clear();
        window.extend_from_slice(&env[lo..hi]);
        let threshold = (median(&mut window) + cfg.delta).max(0.12);
        if v <= threshold {
            continue;
        }
        // Der Flux-Peak liegt etwa ein Viertel Frame nach dem Einsatz.
        let t = (((i * cfg.hop) as f32 + n as f32 * 0.25) / sample_rate - lead_secs).max(0.0);
        if t - last < cfg.min_interval_secs {
            continue;
        }
        onsets.push(t);
        last = t;
    }

    // Hüllkurve wieder auf den Originalbeginn ausrichten.
    let skip = (lead / cfg.hop).min(env.len());
    OnsetAnalysis { hop_secs, envelope: env.split_off(skip), onsets }
}

/// Schätzt das Tempo aus der Novelty-Kurve. Bevorzugt Werte um 120 BPM,
/// um Oktavfehler (halbes/doppeltes Tempo) abzumildern.
pub fn estimate_tempo(envelope: &[f32], hop_secs: f32, min_bpm: f32, max_bpm: f32) -> Option<f32> {
    if envelope.len() < 16 || hop_secs <= 0.0 {
        return None;
    }
    let mean = envelope.iter().sum::<f32>() / envelope.len() as f32;
    let e: Vec<f32> = envelope.iter().map(|v| v - mean).collect();
    let lag_min = ((60.0 / (max_bpm * hop_secs)).floor() as usize).max(1);
    let lag_max = ((60.0 / (min_bpm * hop_secs)).ceil() as usize).min(e.len() / 2);
    if lag_min + 2 >= lag_max {
        return None;
    }

    let ac: Vec<f32> = (0..=lag_max + 1)
        .map(|lag| {
            if lag >= e.len() {
                return 0.0;
            }
            let n = e.len() - lag;
            (0..n).map(|i| e[i] * e[i + lag]).sum::<f32>() / n as f32
        })
        .collect();

    let mut best = None;
    let mut best_score = 0.0f32;
    for lag in lag_min..=lag_max {
        let bpm = 60.0 / (lag as f32 * hop_secs);
        let octaves = (bpm / 120.0).log2();
        let prior = (-0.5 * (octaves / 0.9).powi(2)).exp();
        let score = ac[lag] * prior;
        if score > best_score {
            best_score = score;
            best = Some(lag);
        }
    }
    let lag = best?;
    let (a, b, c) = (ac[lag - 1], ac[lag], ac[lag + 1]);
    let den = a - 2.0 * b + c;
    let refined = if den.abs() > 1e-12 {
        lag as f32 + 0.5 * (a - c) / den
    } else {
        lag as f32
    };
    Some(60.0 / (refined * hop_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_train(bpm: f32, secs: f32, sr: f32) -> (Vec<f32>, Vec<f32>) {
        let mut x = vec![0.0f32; (secs * sr) as usize];
        let period = 60.0 / bpm;
        let mut times = Vec::new();
        let mut seed = 12345u32;
        let mut t = 0.25;
        while t + 0.1 < secs {
            let s = (t * sr) as usize;
            for i in 0..(0.04 * sr) as usize {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                x[s + i] += noise * (-(i as f32) / (0.006 * sr)).exp() * 0.8;
            }
            times.push(t);
            t += period;
        }
        (x, times)
    }

    #[test]
    fn finds_clicks_and_tempo() {
        let sr = 44_100.0;
        let (x, truth) = click_train(120.0, 8.0, sr);
        let a = detect_onsets(&x, sr, &OnsetConfig::default());
        assert_eq!(a.onsets.len(), truth.len(), "{:?}", a.onsets);
        for (o, t) in a.onsets.iter().zip(&truth) {
            assert!((o - t).abs() < 0.03, "Einsatz {o} statt {t}");
        }
        let bpm = estimate_tempo(&a.envelope, a.hop_secs, 60.0, 200.0).unwrap();
        assert!((bpm - 120.0).abs() < 2.0, "{bpm}");
    }

    #[test]
    fn hit_at_the_very_start_is_found() {
        let sr = 44_100.0;
        let (mut x, _) = click_train(120.0, 4.0, sr);
        // zusätzlicher Schlag genau bei 0 s
        for i in 0..(0.03 * sr) as usize {
            x[i] += (i as f32 * 0.37).sin() * (-(i as f32) / (0.006 * sr)).exp() * 0.8;
        }
        let a = detect_onsets(&x, sr, &OnsetConfig::default());
        assert!(a.onsets.first().map(|&t| t < 0.03).unwrap_or(false), "{:?}", &a.onsets[..3.min(a.onsets.len())]);
    }

    #[test]
    fn tempo_of_slower_pulse() {
        let sr = 44_100.0;
        let (x, _) = click_train(92.0, 10.0, sr);
        let a = detect_onsets(&x, sr, &OnsetConfig::default());
        let bpm = estimate_tempo(&a.envelope, a.hop_secs, 60.0, 200.0).unwrap();
        assert!((bpm - 92.0).abs() < 2.0, "{bpm}");
    }
}
