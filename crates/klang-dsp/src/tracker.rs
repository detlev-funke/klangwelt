//! Live analysis of a running recording: the samples arrive in pieces every
//! few dozen milliseconds and are measured right away with the building
//! blocks of the finished analysis – pitch frame by frame ([`Yin`]), onsets
//! from the spectral flux ([`Flux`], [`is_peak`]) and the input level.
//!
//! The tracker keeps only the samples its next frames need; what it measured
//! (pitch frames, flux, onsets) grows with the recording – about 100 pitch
//! frames and 190 flux values per second. Cost per second of audio at
//! 48 kHz: ~100 YIN frames (three 4096-point FFTs each) and ~190 flux frames
//! (one 1024-point FFT each) – a few percent of one core, in the browser too.

use crate::onset::{estimate_tempo, is_peak, peak_time, Flux, OnsetConfig};
use crate::pitch::{PitchFrame, Yin, YinConfig};

/// Pitch frames per second (the hop follows the sample rate).
const PITCH_FRAMES_PER_SEC: f32 = 96.0;
/// The level is measured over this many seconds of the newest samples.
const LEVEL_SECS: f32 = 0.05;
/// Onsets only count where it isn't silent (RMS of the frame, as the
/// pitch detection's silence threshold).
const ONSET_GATE_RMS: f32 = 0.005;

pub struct Tracker {
    sample_rate: f32,
    yin: Option<Yin>,
    pitch_hop: usize,
    flux: Flux,
    onset_cfg: OnsetConfig,
    /// Samples from position `base` on. Positions count from `lead` zeros
    /// before the recording: the flux needs a "before" for a beat right at
    /// the start (as in [`crate::onset::detect_onsets`]).
    buf: Vec<f32>,
    base: usize,
    lead: usize,
    /// Position of the next pitch frame and the next flux frame.
    next_pitch: usize,
    next_flux: usize,
    received: usize,
    frames: Vec<PitchFrame>,
    /// Flux per onset hop (unnormalized) and the frame's RMS.
    envelope: Vec<f32>,
    envelope_rms: Vec<f32>,
    /// Loudest flux among the first `scaled` values.
    scale: f32,
    scaled: usize,
    /// Envelope values before this index have been checked for onsets.
    checked: usize,
    onsets: Vec<f32>,
    window: Vec<f32>,
}

impl Tracker {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f32;
        let pitch_hop = ((sr / PITCH_FRAMES_PER_SEC).round() as usize).max(32);
        let yin = Yin::new(sr, &YinConfig { hop: pitch_hop, ..YinConfig::default() });
        let onset_cfg = OnsetConfig::default();
        let flux = Flux::new(onset_cfg.frame_len);
        let lead = flux.frame_len();
        Self {
            sample_rate: sr,
            yin,
            pitch_hop,
            flux,
            onset_cfg,
            buf: vec![0.0; lead],
            base: 0,
            lead,
            next_pitch: lead,
            next_flux: 0,
            received: 0,
            frames: Vec::new(),
            envelope: Vec::new(),
            envelope_rms: Vec::new(),
            scale: 0.0,
            scaled: 0,
            checked: 1,
            onsets: Vec::new(),
            window: Vec::new(),
        }
    }

    /// Measures the new samples (mono, at the tracker's rate).
    pub fn push(&mut self, samples: &[f32]) {
        self.buf.extend_from_slice(samples);
        self.received += samples.len();
        let end = self.base + self.buf.len();
        let sr = self.sample_rate;

        if let Some(yin) = self.yin.as_mut() {
            let span = yin.span();
            while self.next_pitch + span <= end {
                let s = self.next_pitch - self.base;
                let time = ((self.next_pitch - self.lead) as f32 + span as f32 * 0.5) / sr;
                self.frames.push(yin.frame(&self.buf[s..s + span], time));
                self.next_pitch += self.pitch_hop;
            }
        }

        let n = self.flux.frame_len();
        while self.next_flux + n <= end {
            let s = self.next_flux - self.base;
            let x = &self.buf[s..s + n];
            let v = self.flux.frame(x);
            self.envelope.push(v);
            self.envelope_rms.push((x.iter().map(|v| v * v).sum::<f32>() / n as f32).sqrt());
            self.next_flux += self.onset_cfg.hop;
        }
        self.find_onsets();
        self.trim();
    }

    /// Checks the flux values whose look-ahead is complete for onsets –
    /// against the loudest flux up to the look-ahead (the finished analysis
    /// normalizes to the loudest of the whole recording). Independent of how
    /// the samples were split into pieces.
    fn find_onsets(&mut self) {
        let cfg = self.onset_cfg;
        let n = self.flux.frame_len();
        while self.checked + cfg.median_radius < self.envelope.len() {
            let i = self.checked;
            self.checked += 1;
            while self.scaled <= i + cfg.median_radius {
                self.scale = self.scale.max(self.envelope[self.scaled]);
                self.scaled += 1;
            }
            if !is_peak(&self.envelope, i, self.scale, &cfg, &mut self.window) {
                continue;
            }
            // the attack lies in the peak frame or right after it
            let loud = self.envelope_rms[i..(i + 3).min(self.envelope_rms.len())].iter().any(|&r| r >= ONSET_GATE_RMS);
            let t = peak_time(i, n, self.sample_rate, &cfg);
            if loud && self.onsets.last().is_none_or(|&last| t - last >= cfg.min_interval_secs) {
                self.onsets.push(t);
            }
        }
    }

    /// Drops samples that no frame needs any more (in larger pieces).
    fn trim(&mut self) {
        let level = (LEVEL_SECS * self.sample_rate) as usize;
        let keep_from = self.next_pitch.min(self.next_flux).min((self.base + self.buf.len()).saturating_sub(level));
        let drop = keep_from.saturating_sub(self.base);
        if drop >= 16_384 {
            self.buf.drain(..drop);
            self.base += drop;
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Recorded seconds so far.
    pub fn seconds(&self) -> f64 {
        self.received as f64 / self.sample_rate as f64
    }

    /// Pitch frames so far (time = center of the frame in recording seconds).
    pub fn frames(&self) -> &[PitchFrame] {
        &self.frames
    }

    /// Onset times so far (seconds).
    pub fn onsets(&self) -> &[f32] {
        &self.onsets
    }

    /// RMS and peak of the newest samples (about 50 ms).
    pub fn level(&self) -> (f32, f32) {
        let len = ((LEVEL_SECS * self.sample_rate) as usize).min(self.received).max(1);
        let tail = &self.buf[self.buf.len().saturating_sub(len)..];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / len as f32).sqrt();
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        (rms, peak)
    }

    /// Tempo of the last `window_secs` seconds (as
    /// [`crate::onset::estimate_tempo`] in the finished analysis) – `None`
    /// while that holds fewer than `min_onsets` onsets.
    pub fn tempo(&self, window_secs: f32, min_onsets: usize) -> Option<f32> {
        let now = self.seconds() as f32;
        let from = now - window_secs;
        if self.onsets.iter().filter(|&&t| t >= from).count() < min_onsets {
            return None;
        }
        let hop_secs = self.onset_cfg.hop as f32 / self.sample_rate;
        let lead = self.lead / self.onset_cfg.hop;
        let skip = (lead + (from.max(0.0) / hop_secs) as usize).min(self.envelope.len());
        estimate_tempo(&self.envelope[skip..], hop_secs, 60.0, 200.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::hz_to_midi;
    use crate::onset::detect_onsets;
    use crate::pitch::detect_pitch;
    use crate::testutil::sine;

    /// Feeds `x` in pieces of `chunk` samples, as the shells do.
    fn feed(x: &[f32], sr: u32, chunk: usize) -> Tracker {
        let mut t = Tracker::new(sr);
        for c in x.chunks(chunk) {
            t.push(c);
        }
        t
    }

    /// Plucked notes: a decaying harmonic tone per note, at 120 BPM.
    fn plucked(midis: &[f32], beat_secs: f32, sr: f32) -> Vec<f32> {
        let mut out = Vec::new();
        for &m in midis {
            let f = 440.0 * 2f32.powf((m - 69.0) / 12.0);
            let n = (beat_secs * sr) as usize;
            for i in 0..n {
                let t = i as f32 / sr;
                let w = 2.0 * std::f32::consts::PI * f * t;
                let env = (t / 0.005).min(1.0) * (-t * 3.0).exp() * ((beat_secs - t) / 0.02).clamp(0.0, 1.0);
                out.push(env * (0.4 * w.sin() + 0.15 * (2.0 * w).sin()));
            }
        }
        out
    }

    #[test]
    fn pitch_frames_match_the_finished_detection() {
        let sr = 48_000;
        let x = sine(220.0, 1.0, sr as f32, 0.4);
        let live = feed(&x, sr, 2048);
        let hop = live.pitch_hop;
        let whole = detect_pitch(&x, sr as f32, &YinConfig { hop, ..YinConfig::default() });
        assert_eq!(live.frames().len(), whole.len());
        for (a, b) in live.frames().iter().zip(&whole) {
            assert_eq!(a, b, "the same frames, piece by piece or in one go");
        }
        let f = live.frames().last().unwrap().freq.unwrap();
        assert!((hz_to_midi(f) - hz_to_midi(220.0)).abs() < 0.05, "{f}");
    }

    #[test]
    fn chunk_size_does_not_change_the_result() {
        let sr = 44_100;
        let x = plucked(&[60.0, 64.0, 67.0, 72.0, 67.0, 64.0], 0.5, sr as f32);
        let a = feed(&x, sr, 128);
        let b = feed(&x, sr, 4096);
        assert_eq!(a.frames(), b.frames());
        assert_eq!(a.onsets(), b.onsets());
        assert!((a.seconds() - x.len() as f64 / sr as f64).abs() < 1e-9);
    }

    #[test]
    fn onsets_and_tempo_follow_the_finished_analysis() {
        let sr = 48_000;
        let notes: Vec<f32> = (0..16).map(|i| [60.0, 64.0, 67.0, 72.0][i % 4]).collect();
        let x = plucked(&notes, 0.5, sr as f32);
        let live = feed(&x, sr, 1024);
        let whole = detect_onsets(&x, sr as f32, &OnsetConfig::default());
        // the last onsets lack the look-ahead while recording; the rest match
        assert!(live.onsets().len() + 1 >= whole.onsets.len() && live.onsets().len() <= whole.onsets.len(), "{:?} / {:?}", live.onsets(), whole.onsets);
        for (a, b) in live.onsets().iter().zip(&whole.onsets) {
            assert!((a - b).abs() < 1e-4, "{a} / {b}");
        }
        let bpm = live.tempo(20.0, 4).expect("tempo");
        assert!((bpm - 120.0).abs() < 3.0, "{bpm}");
        assert!(live.tempo(20.0, 100).is_none(), "too few onsets: no tempo");
    }

    #[test]
    fn quiet_noise_has_no_onsets_and_low_level() {
        let sr = 48_000;
        let mut seed = 99u32;
        let x: Vec<f32> = (0..sr as usize * 2)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0) * 0.001
            })
            .collect();
        let live = feed(&x, sr, 2048);
        assert!(live.onsets().is_empty(), "{:?}", live.onsets());
        let (rms, peak) = live.level();
        assert!(rms < 0.001 && peak <= 0.001, "{rms} {peak}");
        assert!(live.frames().iter().all(|f| f.freq.is_none()));
    }

    #[test]
    fn keeps_only_a_short_window_of_samples() {
        let sr = 48_000;
        let x = sine(330.0, 10.0, sr as f32, 0.3);
        let live = feed(&x, sr, 2048);
        assert!(live.buf.len() < 48_000, "{} samples kept", live.buf.len());
        let (rms, peak) = live.level();
        assert!((rms - 0.3 / 2f32.sqrt()).abs() < 0.01 && (peak - 0.3).abs() < 0.01, "{rms} {peak}");
        assert!((live.frames().len() as f64 - 10.0 * PITCH_FRAMES_PER_SEC as f64).abs() < 20.0, "{}", live.frames().len());
    }
}
