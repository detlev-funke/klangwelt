//! Several voices at once – polyphonic analysis.
//!
//! Carried over from the analysis built for the LED light organ (its "slow
//! path") and taken further to several voices:
//!
//! 1. **STFT** with a ≈ 90 ms Hann window and 1/8 hop, normalized to
//!    amplitudes (a sine of amplitude A shows as A in its bin).
//! 2. **HPSS** (Fitzgerald): a median over time keeps held tones, a median
//!    over frequency keeps hits; a soft Wiener mask leaves the harmonic part.
//!    The drums are gone before pitches are looked for – in the light organ
//!    the kick was what masked the bass.
//! 3. **Spectral whitening** in third-octave bands, so a soft voice isn't
//!    drowned by a loud one in another register.
//! 4. **Several pitches per frame** by iterative harmonic subtraction
//!    (Klapuri): a harmonic salience on a fifth-semitone grid sums the partials
//!    of every candidate (a low note is found through its overtones even where
//!    a kick covers its fundamental – the light organ's overtone band); the
//!    strongest one is taken, its partials are subtracted – smoothed over
//!    neighbouring harmonics, so a partial it shares with another note
//!    survives – and the search repeats. How many notes a frame holds follows
//!    from the accumulated salience: one more is taken while it still pays off.
//!    Each frame thus yields a list of candidates with their strength.
//! 5. **Notes**: per pitch a two-state model (sounding / silent) decoded with
//!    Viterbi – the light organ's lesson that a decision per frame fails where
//!    a decision over the whole path holds. An onset with a rise of that pitch
//!    splits a note (struck anew).
//! 6. **Voices**: how many there are (the polyphony most of the time) and which
//!    note belongs to which – by register and the smallest leaps.
//!
//! [`PolyStream`] does steps 1–4 piece by piece with bounded memory (a few
//! dozen frames), so a whole recording and a running one go the same way; it
//! lags about half the HPSS window (≈ 90 ms) behind the newest samples.

use crate::fft::{hann, Complex, Fft};
use crate::util::median;
use serde::Serialize;
use std::collections::VecDeque;

/// Window length (seconds, rounded to a power of two in samples) and hops per window.
const WINDOW_SECS: f32 = 0.093;
const HOPS_PER_WINDOW: usize = 8;
/// Pitch range searched (MIDI): E1 … C7.
pub const MIN_MIDI: u8 = 28;
pub const MAX_MIDI: u8 = 96;
/// Grid steps per semitone of the salience.
const STEPS: usize = 5;
/// Partials summed per candidate and the highest frequency used.
const HARMONICS: usize = 16;
const MAX_FREQ: f32 = 6000.0;
/// Salience weights of the partials (Klapuri 2006, for magnitudes).
const ALPHA: f32 = 27.0;
const BETA: f32 = 320.0;
/// HPSS medians: frames over time, bins over frequency.
const HPSS_FRAMES: usize = 33;
const HPSS_BINS: usize = 17;
/// Whitening: third-octave bands from this frequency; magnitudes are
/// compressed to the power `NU` within a band.
const WHITEN_FROM: f32 = 50.0;
const NU: f32 = 0.33;
/// Bands quieter than this share of the loudest one aren't lifted further (an
/// empty band would otherwise blow leakage up into false evidence).
const WHITEN_FLOOR: f32 = 0.1;
/// At most this many notes per frame; one more is taken while the
/// accumulated salience divided by count^GAMMA still grows, and only if it
/// reaches this share of the strongest one.
const MAX_PER_FRAME: usize = 6;
const GAMMA: f32 = 0.3;
const MIN_SHARE: f32 = 0.12;
/// Share of a partial's (smoothed) amplitude that is subtracted.
const CANCEL: f32 = 0.9;
/// A partial this many times above the mean of its two neighbours is shared with another note.
const SHARED: f32 = 2.5;
/// Frames quieter than this (RMS) hold no notes.
const SILENCE_RMS: f32 = 0.003;
/// Below this frequency a note needs at least two strong partials among its
/// first six (at least this share of the strongest) – a kick is a falling sine.
const LOW_HARMONIC_HZ: f32 = 120.0;
const HARMONIC_SHARE: f32 = 0.2;
/// A pitch within this many cents of a partial of a pitch already found, and
/// weaker than this share of it, is that partial's residue.
const RESIDUE_CENTS: f32 = 35.0;
const RESIDUE_SHARE: f32 = 0.6;
/// Octave check: even partials (2, 4, 6) at least this share of the fundamental,
/// odd ones (3, 5, 7) at most this share of the even ones.
const OCTAVE_EVEN: f32 = 0.6;
const OCTAVE_ODD: f32 = 0.2;
/// Above this frequency only spectral peaks count: a bin keeps its magnitude
/// in proportion to how far it stands above the local floor (the median over
/// frequency) – the noise of snares and cymbals has no peaks.
const TONAL_FROM_HZ: f32 = 800.0;
const TONAL_FLOOR: f32 = 2.0;
const TONAL_SPAN: f32 = 4.0;

/// One pitch found in a frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PolyPeak {
    /// MIDI with fraction.
    pub midi: f32,
    /// Salience on the whitened spectrum – what the search compared.
    pub salience: f32,
    /// Sum of the note's partial amplitudes in the (harmonic, unwhitened) spectrum.
    pub amp: f32,
    /// Share of the fundamental in `amp` – a subharmonic has next to none.
    pub fundamental: f32,
}

/// The pitches of one frame (strongest first).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PolyFrame {
    /// Center of the window (seconds from the start).
    pub time: f32,
    pub rms: f32,
    pub peaks: Vec<PolyPeak>,
}

pub fn midi_to_hz(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

/// Fixed tables of the salience search and the whitening for one sample rate.
struct Estimator {
    bins: usize,
    /// Per grid point: range into `parts`.
    grid: Vec<(u32, u32)>,
    /// (first bin, last bin, weight) per partial of every grid point.
    parts: Vec<(u16, u16, f32)>,
    /// Whitening: band centers (bins), per bin the band below and the fraction to the next.
    centers: Vec<f32>,
    bin_band: Vec<(usize, f32)>,
}

impl Estimator {
    fn new(sr: f32, n: usize) -> Self {
        let df = sr / n as f32;
        let fmax = MAX_FREQ.min(sr * 0.5 - df);
        let bins = ((fmax / df).ceil() as usize + 2).min(n / 2);
        let half_step = 2f32.powf(0.5 / (12.0 * STEPS as f32)) - 1.0;
        let mut grid = Vec::new();
        let mut parts = Vec::new();
        let n_grid = (MAX_MIDI - MIN_MIDI) as usize * STEPS + 1;
        for i in 0..n_grid {
            let f0 = midi_to_hz(MIN_MIDI as f32 + i as f32 / STEPS as f32);
            let from = parts.len() as u32;
            for h in 1..=HARMONICS {
                let f = f0 * h as f32;
                if f > fmax {
                    break;
                }
                let lo = ((f * (1.0 - half_step)) / df).floor().max(1.0) as usize;
                let hi = (((f * (1.0 + half_step)) / df).ceil() as usize).max(lo + 1).min(bins - 1);
                parts.push((lo as u16, hi as u16, (f0 + ALPHA) / (h as f32 * f0 + BETA)));
            }
            grid.push((from, parts.len() as u32));
        }
        let mut centers = Vec::new();
        let mut c = WHITEN_FROM;
        while c < fmax {
            centers.push(c / df);
            c *= 2f32.powf(1.0 / 3.0);
        }
        let bin_band = (0..bins)
            .map(|k| {
                let k = k as f32;
                match centers.iter().rposition(|&c| c <= k) {
                    None => (0, 0.0),
                    Some(b) if b + 1 >= centers.len() => (b, 0.0),
                    Some(b) => (b, (k - centers[b]) / (centers[b + 1] - centers[b])),
                }
            })
            .collect();
        Self { bins, grid, parts, centers, bin_band }
    }

    /// Whitened copy of `x` (harmonic magnitudes).
    fn whiten(&self, x: &[f32]) -> Vec<f32> {
        let nb = self.centers.len();
        if nb == 0 {
            return x.to_vec();
        }
        // triangular bands between the neighbouring centers
        let mut acc = vec![(0.0f32, 0.0f32); nb];
        for (k, &v) in x.iter().enumerate() {
            let (b, frac) = self.bin_band[k];
            let (k_f, c) = (k as f32, self.centers[b]);
            if k_f < c {
                acc[b].0 += v * v;
                acc[b].1 += 1.0;
                continue;
            }
            acc[b].0 += (1.0 - frac) * v * v;
            acc[b].1 += 1.0 - frac;
            if b + 1 < nb {
                acc[b + 1].0 += frac * v * v;
                acc[b + 1].1 += frac;
            }
        }
        let sigma: Vec<f32> = acc.iter().map(|&(e, w)| if w > 0.0 { (e / w).sqrt() } else { 0.0 }).collect();
        let floor = sigma.iter().fold(0.0f32, |m, &s| m.max(s)) * WHITEN_FLOOR + 1e-6;
        let gain: Vec<f32> = sigma.iter().map(|&s| (s + floor).powf(NU - 1.0)).collect();
        x.iter()
            .enumerate()
            .map(|(k, &v)| {
                let (b, frac) = self.bin_band[k];
                let g = if b + 1 < nb { gain[b] * (1.0 - frac) + gain[b + 1] * frac } else { gain[b] };
                v * g
            })
            .collect()
    }

    /// Salience of every grid point on `r`.
    fn salience(&self, r: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.extend(self.grid.iter().map(|&(a, b)| {
            self.parts[a as usize..b as usize]
                .iter()
                .map(|&(lo, hi, w)| w * r[lo as usize..=hi as usize].iter().fold(0.0f32, |m, &v| m.max(v)))
                .sum::<f32>()
        }));
    }

    /// The pitches of one frame: `x` harmonic magnitudes (unwhitened).
    fn detect(&self, x: &[f32]) -> Vec<PolyPeak> {
        let mut r = self.whiten(x);
        let mut s = Vec::with_capacity(self.grid.len());
        let mut peaks: Vec<PolyPeak> = Vec::new();
        let (mut sum, mut best) = (0.0f32, 0.0f32);
        let mut first = 0.0f32;
        for _ in 0..MAX_PER_FRAME + 3 {
            if peaks.len() >= MAX_PER_FRAME {
                break;
            }
            self.salience(&r, &mut s);
            let Some((i, &top)) = s.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else { break };
            if top <= 0.0 || top < MIN_SHARE * first {
                break;
            }
            first = first.max(top);
            let here = self.partial_peaks(&r, i);
            // strong even partials but hardly odd ones above the fundamental: the
            // pitch is an octave too low (e.g. a bass synth's sub-oscillator under
            // the real note) – the note is the octave above
            let up = i + 12 * STEPS;
            let odd: f32 = [2, 4, 6].iter().filter_map(|&h| here.get(h)).map(|p| p.1).sum();
            let even: f32 = [1, 3, 5].iter().filter_map(|&h| here.get(h)).map(|p| p.1).sum();
            let a1 = here.first().map_or(0.0, |p| p.1);
            let (i, top, found) = if up < s.len() && even >= OCTAVE_EVEN * a1 && odd <= OCTAVE_ODD * even {
                if peaks.iter().any(|q| (q.midi - (MIN_MIDI as f32 + up as f32 / STEPS as f32)).abs() < 0.5) {
                    // that note is found already: this was only its sub-octave
                    self.cancel(&mut r, x, &here);
                    continue;
                }
                (up, s[up], self.partial_peaks(&r, up))
            } else {
                (i, top, here)
            };
            // a low "note" without overtones is a drum (a kick is a falling sine), one
            // whose first partials are missing is a subharmonic: taken out of the
            // spectrum, but not a note
            if self.f0(i) < LOW_HARMONIC_HZ {
                let max = found.iter().take(6).fold(0.0f32, |m, p| m.max(p.1));
                if found.iter().take(4).filter(|p| p.1 >= HARMONIC_SHARE * max).count() < 2 {
                    self.cancel(&mut r, x, &found);
                    continue;
                }
            }
            // the same pitch again: what is left of a note found already
            let here_midi = MIN_MIDI as f32 + i as f32 / STEPS as f32;
            if peaks.iter().any(|q| (q.midi - here_midi).abs() < 0.5) {
                self.cancel(&mut r, x, &found);
                continue;
            }
            // a weak pitch right on a partial of a stronger one in this frame is what
            // is left of that partial, not a note
            let f = self.f0(i);
            let residue = peaks.iter().any(|q| {
                let ratio = f / midi_to_hz(q.midi);
                let h = ratio.round();
                h >= 2.0 && (1200.0 * (ratio / h).log2()).abs() < RESIDUE_CENTS && top < RESIDUE_SHARE * q.salience
            });
            if residue {
                self.cancel(&mut r, x, &found);
                continue;
            }
            let j = peaks.len() + 1;
            let score = (sum + top) / (j as f32).powf(GAMMA);
            if j > 1 && score <= best {
                break;
            }
            sum += top;
            best = score;
            // parabolic refinement between the grid points
            let frac = if i > 0 && i + 1 < s.len() {
                let (a, b, c) = (s[i - 1], s[i], s[i + 1]);
                let d = a - 2.0 * b + c;
                if d < 0.0 { (0.5 * (a - c) / d).clamp(-0.5, 0.5) } else { 0.0 }
            } else {
                0.0
            };
            let midi = MIN_MIDI as f32 + (i as f32 + frac) / STEPS as f32;
            let amp = self.cancel(&mut r, x, &found);
            let fundamental = found.first().map_or(0.0, |&(k, _)| x[k]) / amp.max(1e-12);
            peaks.push(PolyPeak { midi, salience: top, amp, fundamental });
        }
        peaks
    }

    /// Fundamental of grid point `i` (Hz).
    fn f0(&self, i: usize) -> f32 {
        midi_to_hz(MIN_MIDI as f32 + i as f32 / STEPS as f32)
    }

    /// Where the partials of grid point `i` lie in `r` (bin, magnitude).
    fn partial_peaks(&self, r: &[f32], i: usize) -> Vec<(usize, f32)> {
        let (a, b) = self.grid[i];
        self.parts[a as usize..b as usize]
            .iter()
            .map(|&(lo, hi, _)| {
                let (lo, hi) = (lo as usize, hi as usize);
                let k = (lo..=hi).max_by(|&p, &q| r[p].total_cmp(&r[q])).unwrap_or(lo);
                (k, r[k])
            })
            .collect()
    }

    /// Subtracts the partials `found` (see [`Self::partial_peaks`]) from `r`;
    /// returns the sum of their amplitudes in `x`.
    fn cancel(&self, r: &mut [f32], x: &[f32], found: &[(usize, f32)]) -> f32 {
        let mut amp = 0.0;
        for (h0, &(k, a_h)) in found.iter().enumerate() {
            amp += x[k];
            if a_h <= 0.0 {
                continue;
            }
            // spectral smoothness (Klapuri): the mean over about an octave of partials;
            // a partial standing far above its neighbours holds another note, too –
            // then only what its neighbours suggest is this note's
            let h = (h0 + 1) as f32;
            let (from, to) = ((h / std::f32::consts::SQRT_2).ceil() as usize, (h * std::f32::consts::SQRT_2).floor() as usize);
            let window: Vec<f32> = (from.max(1)..=to.min(found.len())).map(|m| found[m - 1].1).collect();
            let smooth = if window.is_empty() { a_h } else { window.iter().sum::<f32>() / window.len() as f32 };
            let around: Vec<f32> = [h0.checked_sub(1), Some(h0 + 1)].into_iter().flatten().filter_map(|m| found.get(m)).map(|p| p.1).collect();
            let neighbours = (!around.is_empty()).then(|| around.iter().sum::<f32>() / around.len() as f32);
            let own = match neighbours {
                Some(nb) if h0 > 0 && a_h > SHARED * nb => nb,
                _ => a_h.min(smooth),
            };
            let keep = 1.0 - CANCEL * own / a_h;
            for kk in k.saturating_sub(2)..=(k + 2).min(r.len() - 1) {
                r[kk] *= keep;
            }
        }
        amp
    }
}

/// Steps 1–4 piece by piece: samples in, frames with their pitches out.
pub struct PolyStream {
    sr: f32,
    n: usize,
    hop: usize,
    fft: Fft,
    window: Vec<f32>,
    norm: f32,
    est: Estimator,
    /// Samples from position `base` on; positions count from `lead` zeros
    /// before the recording (the first window is centered on its start).
    buf: Vec<f32>,
    base: usize,
    lead: usize,
    next: usize,
    /// The last frames' magnitudes (time, RMS, magnitudes) for the medians over time.
    recent: VecDeque<(f32, f32, Vec<f32>)>,
    /// Index into `recent` of the next frame to finish.
    pending: usize,
    frames: Vec<PolyFrame>,
}

impl PolyStream {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let n = 1usize << ((WINDOW_SECS * sr).log2().round() as u32).clamp(9, 14);
        let hop = n / HOPS_PER_WINDOW;
        let window = hann(n);
        let norm = 2.0 / window.iter().sum::<f32>();
        Self {
            sr,
            n,
            hop,
            fft: Fft::new(n),
            window,
            norm,
            est: Estimator::new(sr, n),
            buf: vec![0.0; n / 2],
            base: 0,
            lead: n / 2,
            next: 0,
            recent: VecDeque::new(),
            pending: 0,
            frames: Vec::new(),
        }
    }

    /// Seconds between two frames.
    pub fn hop_secs(&self) -> f32 {
        self.hop as f32 / self.sr
    }

    pub fn frames(&self) -> &[PolyFrame] {
        &self.frames
    }

    /// Hands over the frames finished so far (a running recording keeps only
    /// what it still needs).
    pub fn take_frames(&mut self) -> Vec<PolyFrame> {
        std::mem::take(&mut self.frames)
    }

    /// Measures new samples (mono, at the rate given at the start).
    pub fn push(&mut self, samples: &[f32]) {
        self.buf.extend_from_slice(samples);
        while self.next + self.n <= self.base + self.buf.len() {
            let s = self.next - self.base;
            let mut spec: Vec<Complex> = self.buf[s..s + self.n].iter().zip(&self.window).map(|(&v, &w)| Complex::new(v * w, 0.0)).collect();
            let rms = (self.buf[s..s + self.n].iter().map(|v| v * v).sum::<f32>() / self.n as f32).sqrt();
            self.fft.forward(&mut spec);
            let mag: Vec<f32> = spec[..self.est.bins].iter().map(|c| c.norm() * self.norm).collect();
            let time = ((self.next as f32 - self.lead as f32) + self.n as f32 * 0.5) / self.sr;
            self.recent.push_back((time, rms, mag));
            self.next += self.hop;
            self.finish(false);
        }
        let drop = self.next.saturating_sub(self.base);
        if drop >= 1 << 15 {
            self.buf.drain(..drop);
            self.base += drop;
        }
    }

    /// The end of a whole recording: the last frames are finished without
    /// the frames after them.
    pub fn flush(&mut self) {
        // the tail: the remaining samples padded with silence
        let tail = (self.next + self.n).saturating_sub(self.base + self.buf.len());
        if tail < self.n {
            self.push(&vec![0.0; tail]);
        }
        self.finish(true);
    }

    /// Finishes every frame whose median over time is complete (`all`: every one).
    fn finish(&mut self, all: bool) {
        let half = HPSS_FRAMES / 2;
        while self.pending < self.recent.len() && (all || self.pending + half < self.recent.len()) {
            let i = self.pending;
            let (time, rms, ref mag) = self.recent[i];
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(self.recent.len());
            let mut over_time = Vec::with_capacity(HPSS_FRAMES);
            let mut over_freq = Vec::with_capacity(HPSS_BINS);
            let bins = mag.len();
            let hb = HPSS_BINS / 2;
            let df = self.sr / self.n as f32;
            let harmonic: Vec<f32> = (0..bins)
                .map(|k| {
                    over_time.clear();
                    over_time.extend(self.recent.range(lo..hi).map(|f| f.2[k]));
                    let h = median(&mut over_time);
                    over_freq.clear();
                    over_freq.extend_from_slice(&mag[k.saturating_sub(hb)..(k + hb + 1).min(bins)]);
                    let p = median(&mut over_freq);
                    let (h2, p2) = (h * h, p * p);
                    let tonal = if k as f32 * df < TONAL_FROM_HZ { 1.0 } else { ((mag[k] / (p + 1e-12) - TONAL_FLOOR) / TONAL_SPAN).clamp(0.0, 1.0) };
                    mag[k] * h2 / (h2 + p2 + 1e-12) * tonal
                })
                .collect();
            let peaks = if rms < SILENCE_RMS { Vec::new() } else { self.est.detect(&harmonic) };
            self.frames.push(PolyFrame { time, rms, peaks });
            self.pending += 1;
            // keep only what the next medians need
            while self.pending > half {
                self.recent.pop_front();
                self.pending -= 1;
            }
        }
    }
}

/// The pitches of a whole recording, frame by frame.
pub fn analyze_frames(samples: &[f32], sample_rate: u32) -> (Vec<PolyFrame>, f32) {
    let mut s = PolyStream::new(sample_rate);
    s.push(samples);
    s.flush();
    let hop = s.hop_secs();
    (s.frames, hop)
}

// ---------------------------------------------------------------- Notes

/// A note found by the polyphonic analysis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolyNote {
    pub start: f32,
    pub end: f32,
    pub midi: u8,
    /// Mean deviation of the measured pitch from `midi`.
    pub cents: f32,
    /// Typical partial amplitude sum while it sounds (for dynamics).
    pub amp: f32,
    /// Salience relative to a clearly sounding note (median while it sounds).
    pub strength: f32,
    /// Typical share of the fundamental in its partials (see [`PolyPeak::fundamental`]).
    pub fundamental: f32,
    /// Struck anew at its start – not the same pitch going on.
    pub new_attack: bool,
}

/// Probabilities of the per-pitch model (per frame): switching on, off.
const P_ON: f32 = 0.005;
const P_OFF: f32 = 0.02;
/// Salience (relative to the reference) at which a frame is as likely on as off,
/// and how sharply the evidence turns around it.
const NEUTRAL: f32 = 0.15;
const SLOPE: f32 = 0.04;
/// Notes shorter than this are dropped.
const MIN_NOTE_SECS: f32 = 0.06;
/// A weak note (strength below this) must last at least this long – a filter
/// sweep or an attack noise passes in a few frames.
const WEAK_STRENGTH: f32 = 0.35;
const WEAK_MIN_SECS: f32 = 0.1;
/// A note shorter than this whose pitch spreads more than this (cents, RMS) is a sweep.
const SWEEP_MAX_SECS: f32 = 0.25;
const SWEEP_CENTS: f32 = 22.0;
/// A note shorter than this more than this many cents off the tuning is a resonance.
const DETUNED_MAX_SECS: f32 = 0.2;
const DETUNED_CENTS: f32 = 25.0;
/// Shadows: at most this share of the strength of the note above, within it
/// give or take this much; a fifth only counts as one below this pitch.
const SHADOW_SHARE: f32 = 0.45;
const SHADOW_SLACK_SECS: f32 = 0.06;
const SHADOW_FIFTH_BELOW: u8 = 45;
/// … and with a fundamental below this share of its partials (a real low note has one).
const SHADOW_FUNDAMENTAL: f32 = 0.12;
/// A re-attack: an onset this far inside a note, with the pitch's salience
/// rising by this factor from just before to just after it.
const REATTACK_MARGIN_SECS: f32 = 0.08;
const REATTACK_RISE: f32 = 1.35;
/// Gaps up to this long are bridged unless an onset lies this close to where the note resumes.
const BRIDGE_SECS: f32 = 0.2;
const BRIDGE_ONSET_SECS: f32 = 0.06;
const REATTACK_DIP: f32 = 0.9;
const REATTACK_BACK: f32 = 0.8;
/// A note whose fundamental holds at least this share of its partials is judged by it.
const REATTACK_FUNDAMENTAL: f32 = 0.15;

/// The salience a clearly sounding note has in these frames: the upper
/// quartile of each frame's strongest pitch.
pub fn reference(frames: &[PolyFrame]) -> f32 {
    let mut top: Vec<f32> = frames.iter().filter_map(|f| f.peaks.first().map(|p| p.salience)).collect();
    if top.is_empty() {
        return 1.0;
    }
    top.sort_by(f32::total_cmp);
    top[(top.len() * 3 / 4).min(top.len() - 1)].max(1e-9)
}

/// Notes from the frames: per pitch the most likely on/off path (Viterbi),
/// split where an onset (seconds, sorted) brings a rise of that pitch.
/// `reference`: salience of a clearly sounding note (see [`reference`]).
pub fn track_notes(frames: &[PolyFrame], hop: f32, onsets: &[f32], reference: f32) -> Vec<PolyNote> {
    let range = (MAX_MIDI - MIN_MIDI) as usize + 1;
    let t_n = frames.len();
    if t_n == 0 {
        return Vec::new();
    }
    // salience, deviation and amplitude per frame and pitch
    let mut act = vec![0.0f32; t_n * range];
    let mut dev = vec![0.0f32; t_n * range];
    let mut amp = vec![0.0f32; t_n * range];
    let mut fund = vec![0.0f32; t_n * range];
    for (t, f) in frames.iter().enumerate() {
        for p in &f.peaks {
            let m = p.midi.round();
            if m < MIN_MIDI as f32 || m > MAX_MIDI as f32 {
                continue;
            }
            let i = t * range + (m as usize - MIN_MIDI as usize);
            if p.salience > act[i] {
                act[i] = p.salience;
                dev[i] = (p.midi - m) * 100.0;
                amp[i] = p.amp;
                fund[i] = p.fundamental;
            }
        }
    }
    let (l_on, l_stay_on, l_off, l_stay_off) = (P_ON.ln(), (1.0 - P_OFF).ln(), P_OFF.ln(), (1.0 - P_ON).ln());
    let emission = |a: f32| {
        let e = 0.12 + 0.76 / (1.0 + (-(a / reference - NEUTRAL) / SLOPE).exp());
        (e.ln(), (1.0 - e).ln())
    };
    let mut notes = Vec::new();
    let mut back = vec![(false, false); t_n];
    for p in 0..range {
        let a = |t: usize| act[t * range + p];
        if (0..t_n).all(|t| a(t) <= 0.0) {
            continue;
        }
        // Viterbi over two states; back[t] = (best predecessor of "on", of "off") is on
        let (e_on, e_off) = emission(a(0));
        let (mut v_on, mut v_off) = (l_on + e_on, l_stay_off + e_off);
        for (t, from) in back.iter_mut().enumerate().take(t_n).skip(1) {
            let (e_on, e_off) = emission(a(t));
            let (on_from_on, on_from_off) = (v_on + l_stay_on, v_off + l_on);
            let (off_from_on, off_from_off) = (v_on + l_off, v_off + l_stay_off);
            *from = (on_from_on >= on_from_off, off_from_on > off_from_off);
            v_on = on_from_on.max(on_from_off) + e_on;
            v_off = off_from_on.max(off_from_off) + e_off;
        }
        let mut state = v_on > v_off;
        let mut on = vec![false; t_n];
        for t in (0..t_n).rev() {
            on[t] = state;
            if t > 0 {
                state = if state { back[t].0 } else { back[t].1 };
            }
        }
        // a short gap without a new attack: the note went on (a partial beating
        // away, a moment covered by another note)
        let bridge = ((BRIDGE_SECS / hop).round() as usize).max(1);
        let struck_at = |time: f32| {
            let i = onsets.partition_point(|&o| o < time - BRIDGE_ONSET_SECS);
            onsets.get(i).is_some_and(|&o| o <= time + BRIDGE_ONSET_SECS)
        };
        let mut last_on: Option<usize> = None;
        for t in 0..t_n {
            if !on[t] {
                continue;
            }
            if let Some(l) = last_on {
                if t > l + 1 && t - l - 1 <= bridge && !struck_at(frames[t].time) {
                    on[l + 1..t].iter_mut().for_each(|v| *v = true);
                }
            }
            last_on = Some(t);
        }
        // sounding stretches → notes, split at re-attacks
        let mut t = 0;
        while t < t_n {
            if !on[t] {
                t += 1;
                continue;
            }
            let first = t;
            while t < t_n && on[t] {
                t += 1;
            }
            let last = t - 1;
            let mut cuts = vec![first];
            let start_s = frames[first].time;
            let end_s = frames[last].time;
            let from = onsets.partition_point(|&o| o < start_s + REATTACK_MARGIN_SECS);
            // the level a re-attack is judged by: the fundamental where the note has a
            // strong one (another voice may share its upper partials, never that), else
            // all its partials
            let mut shares: Vec<f32> = (first..=last).map(|t| fund[t * range + p]).filter(|f| *f > 0.0).collect();
            let strong_fundamental = !shares.is_empty() && median(&mut shares) >= REATTACK_FUNDAMENTAL;
            let level = |t: usize| if strong_fundamental { amp[t * range + p] * fund[t * range + p] } else { amp[t * range + p] };
            for &o in onsets[from..].iter().take_while(|&&o| o <= end_s - REATTACK_MARGIN_SECS) {
                let k = first + frames[first..=last].partition_point(|f| f.time < o);
                if k <= *cuts.last().unwrap_or(&first) + 2 {
                    continue;
                }
                let span = |secs: f32| ((secs / hop).round() as usize).max(1);
                let before = (k.saturating_sub(span(0.05))..k).map(&level).fold(f32::INFINITY, f32::min);
                let after = (k..(k + span(0.08)).min(last + 1)).map(&level).fold(0.0f32, f32::max);
                // a re-attack comes after a dip (not while the note is still swelling)
                // and brings the level back up to its attack
                let peak = (first..k).map(&level).fold(0.0f32, f32::max);
                if after >= REATTACK_RISE * before.max(1e-9) && before <= REATTACK_DIP * peak && after >= REATTACK_BACK * peak {
                    cuts.push(k);
                }
            }
            cuts.push(last + 1);
            for (ci, w) in cuts.windows(2).enumerate() {
                let (s, e) = (w[0], w[1] - 1);
                let start = frames[s].time - hop * 0.5;
                let end = frames[e].time + hop * 0.5;
                if end - start < MIN_NOTE_SECS {
                    continue;
                }
                let idx = |t: usize| t * range + p;
                let held: Vec<usize> = (s..=e).filter(|&t| act[idx(t)] > 0.0).collect();
                let mut amps: Vec<f32> = held.iter().map(|&t| amp[idx(t)]).collect();
                let cents = if held.is_empty() { 0.0 } else { held.iter().map(|&t| dev[idx(t)]).sum::<f32>() / held.len() as f32 };
                let strength = {
                    let mut sal: Vec<f32> = held.iter().map(|&t| act[idx(t)] / reference).collect();
                    if sal.is_empty() { 0.0 } else { median(&mut sal) }
                };
                if strength < WEAK_STRENGTH && end - start < WEAK_MIN_SECS {
                    continue;
                }
                // a short note far off the tuning is a resonance or an inharmonic partial
                if end - start < DETUNED_MAX_SECS && cents.abs() > DETUNED_CENTS {
                    continue;
                }
                // a short note whose pitch wanders is a sweep (a resonant filter), not a note
                if end - start < SWEEP_MAX_SECS && held.len() >= 3 {
                    let spread = (held.iter().map(|&t| (dev[idx(t)] - cents).powi(2)).sum::<f32>() / held.len() as f32).sqrt();
                    if spread > SWEEP_CENTS {
                        continue;
                    }
                }
                notes.push(PolyNote {
                    start: start.max(0.0),
                    end,
                    midi: MIN_MIDI + p as u8,
                    cents,
                    amp: if amps.is_empty() { 0.0 } else { median(&mut amps) },
                    strength,
                    fundamental: {
                        let mut fs: Vec<f32> = held.iter().map(|&t| fund[idx(t)]).collect();
                        if fs.is_empty() { 0.0 } else { median(&mut fs) }
                    },
                    // a cut at a re-attack, or a stretch that starts on its own
                    new_attack: ci > 0 || s == 0 || !on[s - 1],
                });
            }
        }
    }
    drop_shadows(&mut notes, onsets);
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.midi.cmp(&a.midi)));
    notes
}

/// Whether `l` sits an octave, an octave and a fifth or two octaves below `u`
/// (low down also a fifth – two thirds of its frequency).
fn below_in_series(l: &PolyNote, u: &PolyNote) -> bool {
    let iv = u.midi as i32 - l.midi as i32;
    matches!(iv, 12 | 19 | 24) || (iv == 7 && l.midi < SHADOW_FIFTH_BELOW)
}

/// Notes in the series below another one that aren't notes of their own:
///
/// * a **shadow** – much weaker and lying within it: a sub-oscillator or a
///   subharmonic the frames couldn't tell from the note;
/// * a **hand-over** – starting without an onset while it still sounds and
///   going on after it: the note's low part outlasting the rest (a bass
///   synth's sub-oscillator, a decaying string). The note above takes its end.
///
/// Notes of the same pitch that then overlap become one.
fn drop_shadows(notes: &mut Vec<PolyNote>, onsets: &[f32]) {
    let struck = |t: f32| {
        let i = onsets.partition_point(|&o| o < t - SHADOW_SLACK_SECS);
        onsets.get(i).is_some_and(|&o| o <= t + SHADOW_SLACK_SECS)
    };
    let mut gone = vec![false; notes.len()];
    let mut extend: Vec<f32> = notes.iter().map(|n| n.end).collect();
    for (li, l) in notes.iter().enumerate() {
        for (ui, u) in notes.iter().enumerate() {
            if ui == li || gone[ui] || !below_in_series(l, u) {
                continue;
            }
            let inside = l.start >= u.start - SHADOW_SLACK_SECS && l.end <= u.end + 2.0 * SHADOW_SLACK_SECS;
            if inside && l.strength < SHADOW_SHARE * u.strength && l.fundamental < SHADOW_FUNDAMENTAL {
                gone[li] = true;
                break;
            }
            if l.start > u.start + SHADOW_SLACK_SECS && l.start < u.end && l.end > u.end && !struck(l.start) {
                gone[li] = true;
                extend[ui] = extend[ui].max(l.end);
                break;
            }
        }
    }
    let mut kept: Vec<PolyNote> = notes.iter().zip(&extend).zip(&gone).filter(|(_, g)| !**g).map(|((n, &e), _)| PolyNote { end: e, ..*n }).collect();
    // the same pitch overlapping now: one note
    kept.sort_by(|a, b| a.midi.cmp(&b.midi).then(a.start.total_cmp(&b.start)));
    let mut merged: Vec<PolyNote> = Vec::with_capacity(kept.len());
    for n in kept {
        match merged.last_mut() {
            Some(m) if m.midi == n.midi && n.start < m.end - SHADOW_SLACK_SECS => m.end = m.end.max(n.end),
            _ => merged.push(n),
        }
    }
    *notes = merged;
}

// ---------------------------------------------------------------- Voices

/// How many voices: found from the material or given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VoiceCount {
    #[default]
    Auto,
    Fixed(u8),
}

/// Most voices the analysis assigns.
pub const MAX_VOICES: usize = 6;
/// The voice count is the polyphony that this share of the sounding time stays within.
const VOICE_QUANTILE: f32 = 0.9;
/// Notes starting this close together are placed together.
const CLUSTER_SECS: f32 = 0.04;
/// A voice is free again this long before its note ends (legato overlap).
const OVERLAP_SECS: f32 = 0.03;

/// How many notes sound at once – the count most of the sounding time stays within.
pub fn voice_count(notes: &[PolyNote]) -> usize {
    if notes.is_empty() {
        return 0;
    }
    // sweep over starts and ends: how long each polyphony lasts
    let mut events: Vec<(f32, i32)> = notes.iter().flat_map(|n| [(n.start, 1), (n.end, -1)]).collect();
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut time_at = [0.0f32; MAX_VOICES * 4 + 2];
    let (mut level, mut last) = (0i32, events[0].0);
    for (t, d) in events {
        if level > 0 {
            let top = time_at.len() - 1;
            time_at[(level as usize).min(top)] += t - last;
        }
        level += d;
        last = t;
    }
    let total: f32 = time_at.iter().sum();
    if total <= 0.0 {
        return 1;
    }
    let mut acc = 0.0;
    for (k, &secs) in time_at.iter().enumerate().skip(1) {
        acc += secs;
        if acc >= VOICE_QUANTILE * total - 1e-6 {
            return k.clamp(1, MAX_VOICES);
        }
    }
    MAX_VOICES
}

/// The voice of every note (0 = the highest), and how many voices there are.
/// Notes are placed in groups that start together: the free voices take them
/// top to bottom, each the note nearest its last pitch; a note left over
/// (more notes than free voices) joins the voice nearest to it, as a chord.
pub fn assign_voices(notes: &[PolyNote], count: VoiceCount) -> (Vec<usize>, usize) {
    let v_n = match count {
        VoiceCount::Auto => voice_count(notes),
        VoiceCount::Fixed(n) => (n as usize).clamp(1, MAX_VOICES),
    }
    .min(notes.len().max(1));
    let mut voice = vec![0usize; notes.len()];
    if notes.is_empty() || v_n <= 1 {
        return (voice, if notes.is_empty() { 0 } else { 1 });
    }
    // starting points: the register quantiles, highest first
    let mut pitches: Vec<f32> = notes.iter().map(|n| n.midi as f32).collect();
    pitches.sort_by(f32::total_cmp);
    let mut last: Vec<f32> = (0..v_n).map(|v| pitches[(((1.0 - (v as f32 + 0.5) / v_n as f32) * pitches.len() as f32) as usize).min(pitches.len() - 1)]).collect();
    let mut busy_until = vec![f32::NEG_INFINITY; v_n];
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|&a, &b| notes[a].start.total_cmp(&notes[b].start).then(notes[b].midi.cmp(&notes[a].midi)));
    let mut i = 0;
    while i < order.len() {
        let t0 = notes[order[i]].start;
        let mut group: Vec<usize> = Vec::new();
        while i < order.len() && notes[order[i]].start - t0 <= CLUSTER_SECS {
            group.push(order[i]);
            i += 1;
        }
        group.sort_by(|&a, &b| notes[b].midi.cmp(&notes[a].midi));
        let free: Vec<usize> = (0..v_n).filter(|&v| busy_until[v] <= t0 + OVERLAP_SECS).collect();
        // order-preserving matching of the group (high → low) to the free voices
        // (top → bottom) with the smallest leaps; a note may stay unmatched
        const SKIP: f32 = 48.0;
        let (g, f) = (group.len(), free.len());
        let mut cost = vec![vec![f32::INFINITY; f + 1]; g + 1];
        let mut pick = vec![vec![0u8; f + 1]; g + 1];
        cost[0].fill(0.0);
        for a in 1..=g {
            cost[a][0] = a as f32 * SKIP;
            pick[a][0] = 1;
            for b in 1..=f {
                let leap = (notes[group[a - 1]].midi as f32 - last[free[b - 1]]).abs();
                let options = [(cost[a - 1][b - 1] + leap, 0u8), (cost[a - 1][b] + SKIP, 1), (cost[a][b - 1], 2)];
                let (c, k) = options.iter().copied().fold((f32::INFINITY, 0), |m, o| if o.0 < m.0 { o } else { m });
                cost[a][b] = c;
                pick[a][b] = k;
            }
        }
        let (mut a, mut b) = (g, f);
        let mut leftover = Vec::new();
        while a > 0 {
            match pick[a][b] {
                0 => {
                    let n = group[a - 1];
                    voice[n] = free[b - 1];
                    a -= 1;
                    b -= 1;
                }
                1 => {
                    leftover.push(group[a - 1]);
                    a -= 1;
                }
                _ => b -= 1,
            }
        }
        for &n in &group {
            if leftover.contains(&n) {
                let v = (0..v_n).min_by(|&x, &y| (notes[n].midi as f32 - last[x]).abs().total_cmp(&(notes[n].midi as f32 - last[y]).abs())).unwrap_or(0);
                voice[n] = v;
            }
        }
        for &n in &group {
            let v = voice[n];
            last[v] = notes[n].midi as f32;
            busy_until[v] = busy_until[v].max(notes[n].end);
        }
    }
    (voice, v_n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    /// A harmonic tone (partials 1/h, slight decay) added into `x`.
    fn tone(x: &mut Vec<f32>, midi: f32, start: f32, dur: f32, amp: f32) {
        let sr = SR as f32;
        let f = midi_to_hz(midi);
        let (a, n) = ((start * sr) as usize, (dur * sr) as usize);
        if x.len() < a + n {
            x.resize(a + n, 0.0);
        }
        for i in 0..n {
            let t = i as f32 / sr;
            let env = (t / 0.01).min(1.0) * ((dur - t) / 0.03).clamp(0.0, 1.0) * (-t * 0.8).exp();
            let mut v = 0.0;
            for h in 1..=10 {
                if f * h as f32 > sr * 0.45 {
                    break;
                }
                v += (2.0 * std::f32::consts::PI * f * h as f32 * t).sin() / h as f32;
            }
            x[a + i] += amp * env * v;
        }
    }

    /// (midi, start, end) of the notes, sorted.
    fn found(notes: &[PolyNote]) -> Vec<(u8, f32, f32, f32)> {
        notes.iter().map(|n| (n.midi, (n.start * 100.0).round() / 100.0, (n.end * 100.0).round() / 100.0, (n.strength * 100.0).round() / 100.0)).collect()
    }

    /// Notes of `truth` (midi, start, end) found with the right pitch and an
    /// onset within 70 ms – and how many extra notes there are.
    fn score(notes: &[PolyNote], truth: &[(u8, f32, f32)]) -> (usize, usize) {
        let mut used = vec![false; notes.len()];
        let mut hit = 0;
        for &(m, s, _) in truth {
            if let Some(i) = (0..notes.len()).find(|&i| !used[i] && notes[i].midi == m && (notes[i].start - s).abs() < 0.07) {
                used[i] = true;
                hit += 1;
            }
        }
        (hit, used.iter().filter(|u| !**u).count())
    }

    fn analyze(x: &[f32]) -> Vec<PolyNote> {
        let (frames, hop) = analyze_frames(x, SR);
        let onsets = crate::onset::detect_onsets(x, SR as f32, &crate::onset::OnsetConfig::default()).onsets;
        track_notes(&frames, hop, &onsets, reference(&frames))
    }

    #[test]
    fn a_chord_shows_all_its_notes_in_one_frame() {
        let mut x = Vec::new();
        for m in [48.0, 60.0, 64.0, 67.0] {
            tone(&mut x, m, 0.0, 1.5, 0.15);
        }
        let (frames, _) = analyze_frames(&x, SR);
        let mid = &frames[frames.len() / 2];
        let mut got: Vec<u8> = mid.peaks.iter().map(|p| p.midi.round() as u8).collect();
        got.sort_unstable();
        assert_eq!(got, vec![48, 60, 64, 67], "{:?}", mid.peaks);
        let notes = analyze(&x);
        assert_eq!(notes.iter().map(|n| n.midi).collect::<Vec<_>>().len(), 4, "{:?}", found(&notes));
        assert_eq!(voice_count(&notes), 4);
    }

    #[test]
    fn bass_and_melody_become_two_voices() {
        // a walking bass in half notes under a melody in quarters (120 BPM)
        let mut x = Vec::new();
        let mut truth = Vec::new();
        for (k, m) in [36u8, 43, 41, 38].iter().enumerate() {
            let s = k as f32;
            tone(&mut x, *m as f32, s, 0.95, 0.2);
            truth.push((*m, s, s + 0.95));
        }
        for (k, m) in [72u8, 74, 76, 77, 79, 77, 76, 72].iter().enumerate() {
            let s = k as f32 * 0.5;
            tone(&mut x, *m as f32, s, 0.45, 0.12);
            truth.push((*m, s, s + 0.45));
        }
        let notes = analyze(&x);
        let (hit, extra) = score(&notes, &truth);
        assert!(hit >= 11 && extra <= 2, "{hit}/12 found, {extra} extra: {:?}", found(&notes));
        let (voice, n) = assign_voices(&notes, VoiceCount::Auto);
        assert_eq!(n, 2);
        for (note, v) in notes.iter().zip(&voice) {
            assert_eq!(*v, if note.midi >= 60 { 0 } else { 1 }, "{note:?}");
        }
    }

    #[test]
    fn a_single_line_is_one_voice() {
        let mut x = Vec::new();
        for (k, m) in [60.0, 62.0, 64.0, 65.0, 67.0].iter().enumerate() {
            tone(&mut x, *m, k as f32 * 0.4, 0.38, 0.2);
        }
        let notes = analyze(&x);
        assert_eq!(notes.iter().map(|n| n.midi).collect::<Vec<_>>(), vec![60, 62, 64, 65, 67], "{:?}", found(&notes));
        assert_eq!(assign_voices(&notes, VoiceCount::Auto).1, 1);
        // given: as many voices as asked, never more than notes
        assert_eq!(assign_voices(&notes, VoiceCount::Fixed(3)).1, 3);
    }

    #[test]
    fn drums_dont_make_notes_and_dont_hide_the_bass() {
        use crate::synth;
        let sr = SR as f32;
        let mut x = Vec::new();
        let mut truth = Vec::new();
        for (k, m) in [40u8, 45, 43, 38].iter().enumerate() {
            tone(&mut x, *m as f32, k as f32, 0.95, 0.2);
            truth.push((*m, k as f32, k as f32 + 0.95));
        }
        x.resize((4.2 * sr) as usize, 0.0);
        let (kick, snare) = (synth::kick(sr), synth::snare(sr, 3));
        for b in 0..8 {
            let at = (b as f32 * 0.5 * sr) as usize;
            synth::mix_into(&mut x, if b % 2 == 0 { &kick } else { &snare }, at, 0.6);
        }
        let notes = analyze(&x);
        let (hit, extra) = score(&notes, &truth);
        assert!(hit >= 3 && extra <= 2, "{hit}/4, {extra} extra: {:?}", found(&notes));
    }

    #[test]
    fn a_struck_again_note_is_two_notes() {
        let mut x = Vec::new();
        tone(&mut x, 64.0, 0.0, 0.5, 0.2);
        tone(&mut x, 64.0, 0.5, 0.5, 0.2);
        let notes = analyze(&x);
        assert_eq!(notes.len(), 2, "{:?}", found(&notes));
        assert!(notes.iter().all(|n| n.midi == 64 && n.new_attack));
    }

    #[test]
    fn piece_by_piece_gives_the_same_frames() {
        let mut x = Vec::new();
        tone(&mut x, 57.0, 0.0, 0.6, 0.2);
        tone(&mut x, 64.0, 0.2, 0.6, 0.15);
        let (whole, _) = analyze_frames(&x, SR);
        let mut s = PolyStream::new(SR);
        for c in x.chunks(1500) {
            s.push(c);
        }
        s.flush();
        assert_eq!(s.frames(), &whole[..]);
    }

    #[test]
    fn voices_follow_their_register_when_a_voice_rests() {
        // three voices; the middle one pauses in the second half
        let n = |m: u8, s: f32, e: f32| PolyNote { start: s, end: e, midi: m, cents: 0.0, amp: 1.0, strength: 1.0, fundamental: 0.5, new_attack: true };
        let notes = vec![
            n(72, 0.0, 1.0), n(64, 0.0, 1.0), n(48, 0.0, 1.0),
            n(74, 1.0, 2.0), n(65, 1.0, 2.0), n(47, 1.0, 2.0),
            n(76, 2.0, 3.0), n(48, 2.0, 3.0),
            n(77, 3.0, 4.0), n(45, 3.0, 4.0),
        ];
        let (voice, count) = assign_voices(&notes, VoiceCount::Auto);
        assert_eq!(count, 3);
        assert_eq!(voice, vec![0, 1, 2, 0, 1, 2, 0, 2, 0, 2]);
    }
}


