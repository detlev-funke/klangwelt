//! Pitch detection after YIN (de Cheveigné & Kawahara, 2002).
//!
//! The difference function is computed via an FFT cross-correlation,
//! so an analysis frame costs O(N log N) instead of O(N²).

use crate::fft::{Complex, Fft};
use crate::synth::Biquad;
use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct YinConfig {
    /// Total length of an analysis frame in samples (integration window = half).
    pub frame_len: usize,
    pub hop: usize,
    pub min_freq: f32,
    pub max_freq: f32,
    /// Threshold of the normalized difference function (smaller = stricter).
    pub threshold: f32,
    /// Frames below this RMS level count as silence.
    pub silence_rms: f32,
}

impl Default for YinConfig {
    /// For melodies: singing, melody instruments and bass (down to E1 ≈ 41 Hz).
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
    /// For sung or whistled melodies: from a male voice up to a whistle
    /// (≈ 60 Hz … 3.2 kHz – whistling usually lies at 0.9–2.8 kHz).
    pub fn voice_and_whistle() -> Self {
        Self { min_freq: 60.0, max_freq: 3200.0, ..Self::default() }
    }

    /// For attacks (drums, plucked basses): lower, more tolerant, longer window.
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

/// Fundamental pitch of a single attack.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitPitch {
    /// Onset in seconds.
    pub time: f32,
    /// End of the analyzed section (next onset, at most 0.4 s later).
    pub end: f32,
    /// Fundamental as a MIDI value (fractional), `None` for pure noise (e.g. hi-hat).
    pub midi: Option<f32>,
    /// 0…1 – how unambiguous the pitch is.
    pub clarity: f32,
    /// 0…1 – volume relative to the loudest attack.
    pub strength: f32,
}

/// Determines the fundamental of the following section for each onset.
///
/// Unlike melody detection, this does not require a long, steady
/// note: bass drum (with falling pitch), snare shell, toms or plucked
/// bass notes get their fundamental this way – energy-weighted over the section.
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
            // Skip the first click (beater, stick).
            let s = (((t + 0.008) * sample_rate) as usize).min(len);
            let e = ((end * sample_rate) as usize).clamp(s, len);
            let seg = &samples[s..e];
            let strength = seg.iter().fold(0.0f32, |m, v| m.max(v.abs())) / max_peak;
            // Low-pass (4th order, 1.2 kHz): snare wires, cymbals and stick noise
            // would otherwise mask the fundamental of shell, bass drum or bass.
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
    /// Center of the frame in seconds.
    pub time: f32,
    /// Fundamental frequency in Hz, `None` for silence or uncertain detection.
    pub freq: Option<f32>,
    /// 0…1 – how periodic the signal is (1 − normalized difference).
    pub clarity: f32,
    pub rms: f32,
}

pub fn detect_pitch(samples: &[f32], sample_rate: f32, cfg: &YinConfig) -> Vec<PitchFrame> {
    let Some(mut yin) = Yin::new(sample_rate, cfg) else {
        return Vec::new();
    };
    let span = yin.span();
    if samples.len() < span || cfg.hop == 0 {
        return Vec::new();
    }
    let mut frames = Vec::with_capacity(samples.len() / cfg.hop + 1);
    let mut start = 0;
    while start + span <= samples.len() {
        let time = (start as f32 + span as f32 * 0.5) / sample_rate;
        frames.push(yin.frame(&samples[start..start + span], time));
        start += cfg.hop;
    }
    frames
}

/// YIN for one frame at a time, with its buffers – [`detect_pitch`] walks a
/// whole recording with it, the live analysis ([`crate::tracker`]) one frame
/// whenever enough new samples have arrived. Both measure alike.
pub struct Yin {
    cfg: YinConfig,
    sample_rate: f32,
    w: usize,
    tau_min: usize,
    tau_max: usize,
    span: usize,
    n: usize,
    fft: Fft,
    a: Vec<Complex>,
    b: Vec<Complex>,
    d: Vec<f32>,
    cm: Vec<f32>,
    prefix: Vec<f64>,
}

impl Yin {
    /// `None` when the frequency range doesn't fit into the frame at this rate.
    pub fn new(sample_rate: f32, cfg: &YinConfig) -> Option<Self> {
        let w = cfg.frame_len / 2;
        let tau_max = ((sample_rate / cfg.min_freq).ceil() as usize).min(w).max(4);
        let tau_min = ((sample_rate / cfg.max_freq).floor() as usize).max(2);
        if tau_min + 2 >= tau_max {
            return None;
        }
        let span = w + tau_max;
        let n = span.next_power_of_two();
        Some(Self {
            cfg: *cfg,
            sample_rate,
            w,
            tau_min,
            tau_max,
            span,
            n,
            fft: Fft::new(n),
            a: vec![Complex::ZERO; n],
            b: vec![Complex::ZERO; n],
            d: vec![0f32; tau_max + 1],
            cm: vec![0f32; tau_max + 1],
            prefix: vec![0f64; span + 1],
        })
    }

    /// Samples one frame needs.
    pub fn span(&self) -> usize {
        self.span
    }

    /// Measures one frame: `x` holds at least [`Yin::span`] samples (only
    /// those are used), `time` is its center in seconds.
    pub fn frame(&mut self, x: &[f32], time: f32) -> PitchFrame {
        let (w, span, n, tau_min, tau_max) = (self.w, self.span, self.n, self.tau_min, self.tau_max);
        let Self { cfg, sample_rate, fft, a, b, d, cm, prefix, .. } = self;
        let x = &x[..span];

        for i in 0..span {
            prefix[i + 1] = prefix[i] + (x[i] as f64) * (x[i] as f64);
        }
        let rms = (prefix[w] / w as f64).sqrt() as f32;
        if rms < cfg.silence_rms {
            return PitchFrame { time, freq: None, clarity: 0.0, rms };
        }

        // r(τ) = Σ_j x[j]·x[j+τ] via FFT cross-correlation.
        for i in 0..n {
            a[i] = if i < w { Complex::new(x[i], 0.0) } else { Complex::ZERO };
            b[i] = if i < span { Complex::new(x[i], 0.0) } else { Complex::ZERO };
        }
        fft.forward(a);
        fft.forward(b);
        for i in 0..n {
            a[i] = a[i].conj() * b[i];
        }
        fft.inverse(a);

        // d(τ) = E(0) + E(τ) − 2·r(τ)
        let e0 = prefix[w];
        for tau in 0..=tau_max {
            let et = prefix[tau + w] - prefix[tau];
            d[tau] = (e0 + et - 2.0 * a[tau].re as f64).max(0.0) as f32;
        }

        // Cumulative mean normalized difference function.
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

        // Search for the first minimum below the threshold.
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

        let refined = parabolic(cm, best);
        let clarity = (1.0 - cm[best]).clamp(0.0, 1.0);
        let freq = if voiced && refined > 0.0 {
            Some(*sample_rate / refined)
        } else {
            None
        };
        PitchFrame { time, freq, clarity, rms }
    }
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
        // Synth kick falls from 130 to 45 Hz and decays there – fundamental ≈ 45 Hz (MIDI ≈ 30.3).
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
