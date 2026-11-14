//! Sound generation. Germ cell of the sound workshop: parameters carry
//! expressive meanings (brightness, decay, body, velocity) so that
//! people and an agent can control them through sound descriptions.

use crate::notes::midi_to_hz;
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Plucked instrument (extended Karplus-Strong). All values 0…1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluckParams {
    /// dark ↔ bright (damping of high partials in the string)
    pub brightness: f32,
    /// short ↔ long decay
    pub decay: f32,
    /// thin ↔ warm/full (body resonances)
    pub body: f32,
    /// soft ↔ hard strike (spectrum of the excitation)
    pub attack: f32,
}

impl Default for PluckParams {
    fn default() -> Self {
        Self { brightness: 0.55, decay: 0.5, body: 0.4, attack: 0.5 }
    }
}

pub(crate) struct Rng(pub(crate) u32);
impl Rng {
    pub(crate) fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Biquad (RBJ cookbook), transposed direct form II.
#[derive(Clone, Copy)]
pub(crate) struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }
    pub(crate) fn bandpass(f0: f32, q: f32, sr: f32) -> Self {
        let w0 = 2.0 * PI * f0 / sr;
        let alpha = w0.sin() / (2.0 * q);
        Self::new(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * w0.cos(), 1.0 - alpha)
    }
    pub(crate) fn highpass(f0: f32, q: f32, sr: f32) -> Self {
        let w0 = 2.0 * PI * f0 / sr;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        Self::new((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }
    pub(crate) fn lowpass(f0: f32, q: f32, sr: f32) -> Self {
        let w0 = 2.0 * PI * f0 / sr;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        Self::new((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }
    /// Low shelf filter (`db` boost/cut below `f0`).
    pub(crate) fn lowshelf(f0: f32, db: f32, sr: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let w0 = 2.0 * PI * f0 / sr;
        let (c, alpha) = (w0.cos(), w0.sin() / 2.0 * 2f32.sqrt());
        let sq = 2.0 * a.sqrt() * alpha;
        Self::new(
            a * ((a + 1.0) - (a - 1.0) * c + sq),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sq),
            (a + 1.0) + (a - 1.0) * c + sq,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sq,
        )
    }
    /// High shelf filter (`db` above `f0`).
    pub(crate) fn highshelf(f0: f32, db: f32, sr: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let w0 = 2.0 * PI * f0 / sr;
        let (c, alpha) = (w0.cos(), w0.sin() / 2.0 * 2f32.sqrt());
        let sq = 2.0 * a.sqrt() * alpha;
        Self::new(
            a * ((a + 1.0) + (a - 1.0) * c + sq),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sq),
            (a + 1.0) - (a - 1.0) * c + sq,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sq,
        )
    }
    /// Bell: `db` around `f0` with quality `q`.
    pub(crate) fn peaking(f0: f32, q: f32, db: f32, sr: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let w0 = 2.0 * PI * f0 / sr;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        Self::new(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }
    /// Forgets what passed through (silence from now on).
    pub(crate) fn clear(&mut self) {
        (self.z1, self.z2) = (0.0, 0.0);
    }

    /// Takes over the coefficients of `other`; the state keeps running.
    pub(crate) fn set_coeffs(&mut self, other: &Biquad) {
        (self.b0, self.b1, self.b2, self.a1, self.a2) = (other.b0, other.b1, other.b2, other.a1, other.a2);
    }

    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// A plucked note. Length = `duration_secs` plus a short decay.
pub fn pluck(freq: f32, duration_secs: f32, velocity: f32, sr: f32, p: &PluckParams, seed: u32) -> Vec<f32> {
    pluck_note(&NoteSpec::new(0.0, duration_secs, 69.0 + 12.0 * (freq / 440.0).log2(), velocity), sr, p, seed)
}

/// A vibrating string (Karplus-Strong): delay line with
/// all-pass fine tuning and loop filter.
struct Saite {
    line: Vec<f32>,
    w: usize,
    /// Loop filter: weight of the previous sample (brightness).
    s: f32,
    prev: f32,
    /// All-pass for the fraction of the period (accurate tuning without loss of highs).
    ap_x1: f32,
    ap_y1: f32,
    len: usize,
    ap: f32,
    g: f32,
}

impl Saite {
    fn new(excitation: &[f32], size: usize, s: f32) -> Self {
        let mut line = vec![0f32; size];
        for (i, v) in excitation.iter().take(size).enumerate() {
            line[i] = *v;
        }
        Self { line, w: excitation.len().min(size) % size, s, prev: 0.0, ap_x1: 0.0, ap_y1: 0.0, len: 2, ap: 0.0, g: 0.0 }
    }

    /// Sets the period (samples, incl. delay of the loop filter) and decay time.
    fn tune(&mut self, period_total: f32, t60: f32, freq: f32) {
        // Loop filter delays by s; remainder: whole samples + fraction in the range 0.5…1.5 (stable all-pass)
        let p = (period_total - self.s).clamp(2.5, (self.line.len() - 3) as f32);
        let len = (p - 0.5).floor().max(1.0);
        let d = p - len;
        self.len = len as usize;
        self.ap = (1.0 - d) / (1.0 + d);
        self.g = 10f32.powf(-3.0 / (t60 * freq));
    }

    #[inline]
    fn step(&mut self) -> f32 {
        let size = self.line.len();
        let x = self.line[(self.w + size - self.len) % size];
        // first-order all-pass (Thiran)
        let y = self.ap * x + self.ap_x1 - self.ap * self.ap_y1;
        self.ap_x1 = x;
        self.ap_y1 = y;
        let out = self.g * ((1.0 - self.s) * y + self.s * self.prev);
        self.prev = y;
        self.line[self.w] = out;
        self.w = (self.w + 1) % size;
        out
    }
}

/// A plucked note with expression: velocity (volume and hardness), brightness,
/// vibrato and scoop-in (delay line with variable length).
///
/// For a natural sound: pluck position (comb filter on the excitation –
/// near the bridge it sounds brighter and thinner), two slightly detuned
/// planes of vibration of the string (a gentle beating instead of a rigid
/// tone) and a body with several resonances.
pub fn pluck_note(n: &NoteSpec, sr: f32, p: &PluckParams, seed: u32) -> Vec<f32> {
    let release = 0.08;
    let duration_secs = n.dur_secs;
    let velocity = n.velocity;
    let total = ((duration_secs.max(0.01) + release) * sr) as usize;
    let mut out = vec![0f32; total];
    let freq_of = |offset: f32| (midi_to_hz(n.midi + offset)).clamp(20.0, sr * 0.45);
    let freq = freq_of(n.pitch_offset(0.0));

    // Loop filter: average of two samples, weighted by brightness;
    // high notes somewhat brighter, otherwise they sound dull
    let register = ((n.midi - 60.0) / 36.0).clamp(-0.5, 1.0);
    let s = (0.5 - 0.45 * (p.brightness + 0.45 * n.brightness + 0.12 * register).clamp(0.0, 1.0)).max(0.03);
    let t60 = 0.25 + 7.0 * p.decay.clamp(0.0, 1.0).powf(1.5);

    // Buffer for the longest period (lowest note during scoop-in/vibrato).
    let longest = sr / freq_of(n.lowest_offset().min(n.pitch_offset(0.0)));
    let size = longest.ceil() as usize + 4;
    let period0 = sr / freq;

    // Excitation: displacement of the string when plucked – a triangle with its peak at the
    // pluck point (hard strike near the bridge, softer further toward the middle), plus
    // filtered noise for hardness and liveliness (harder = more overtones)
    let mut rng = Rng(seed.wrapping_mul(2_654_435_761) | 1);
    let hardness = (p.attack + 0.3 * n.brightness + 0.25 * (velocity - 0.8)).clamp(0.0, 1.0);
    let a = 0.15 + 0.85 * hardness;
    let pos = 0.27 - 0.15 * hardness;
    let exc_len = (period0.round() as usize).clamp(2, size);
    let comb = ((exc_len as f32 * pos).round() as usize).max(1);
    let mut noise = vec![0f32; exc_len];
    let mut lp = 0.0;
    for v in noise.iter_mut() {
        lp += a * (rng.next() - lp);
        *v = lp;
    }
    let noise_amt = 0.3 + 0.9 * hardness + 0.6 * p.brightness.clamp(0.0, 1.0);
    let plucked: Vec<f32> = (0..exc_len)
        .map(|i| {
            let x = i as f32 / exc_len as f32;
            let tri = if x < pos { x / pos } else { (1.0 - x) / (1.0 - pos) };
            let nz = noise[i] - if i >= comb { noise[i - comb] } else { 0.0 };
            tri + noise_amt * nz
        })
        .collect();
    let mean = plucked.iter().sum::<f32>() / exc_len as f32;
    let peak = plucked.iter().fold(0.0f32, |m, v| m.max((v - mean).abs())).max(1e-6);
    let plucked: Vec<f32> = plucked.iter().map(|v| (v - mean) / peak).collect();

    // two planes of vibration: the second a good cent higher, sounds slightly shorter
    let detune = 1.0 + 0.0007 + 0.0004 * (seed % 7) as f32 / 7.0;
    let mut a_str = Saite::new(&plucked, size, s);
    let mut b_str = Saite::new(&plucked, size, s);
    a_str.tune(period0, t60, freq);
    b_str.tune(period0 / detune, t60 * 0.8, freq * detune);

    // Body: air resonance, two wood resonances, a little "air" in the highs
    let body = p.body.clamp(0.0, 1.0);
    let mut res = [
        (Biquad::bandpass(110.0, 1.3, sr), 1.0f32),
        (Biquad::bandpass(200.0, 1.3, sr), 0.9),
        (Biquad::bandpass(400.0, 1.4, sr), 0.55),
        (Biquad::bandpass(2600.0, 1.0, sr), 0.25),
    ];
    let note_end = (duration_secs * sr) as usize;
    let rel_len = (release * sr) as usize;
    let fade_in = (0.0015 * sr) as usize;

    let modulated = n.modulated();
    for (i, o) in out.iter_mut().enumerate() {
        if modulated && i % 8 == 0 {
            let f = freq_of(n.pitch_offset(i as f32 / sr));
            a_str.tune(sr / f, t60, f);
            b_str.tune(sr / (f * detune), t60 * 0.8, f * detune);
        }
        let y = 0.62 * a_str.step() + 0.38 * b_str.step();
        let resonance: f32 = res.iter_mut().map(|(bq, g)| *g * bq.process(y)).sum();
        let mut v = y * (1.0 - 0.3 * body) + body * 0.9 * resonance;
        if i < fade_in {
            v *= i as f32 / fade_in as f32;
        }
        if i >= note_end {
            let k = (i - note_end) as f32 / rel_len.max(1) as f32;
            v *= (1.0 - k).max(0.0).powi(2);
        }
        *o = v * 0.38 * velocity.clamp(0.0, 1.0);
    }
    out
}

/// Bass drum: falling sine frequency plus a short click.
pub fn kick(sr: f32) -> Vec<f32> {
    let n = (0.45 * sr) as usize;
    let mut phase = 0.0f32;
    let mut rng = Rng(77);
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let f = 45.0 + 85.0 * (-t * 28.0).exp();
            phase += 2.0 * PI * f / sr;
            let body = phase.sin() * (-t * 7.0).exp();
            let click = rng.next() * (-t * 300.0).exp() * 0.3;
            (body + click) * 0.9
        })
        .collect()
}

/// Snare: tone at 185 Hz plus band-pass-filtered noise.
pub fn snare(sr: f32, seed: u32) -> Vec<f32> {
    let n = (0.25 * sr) as usize;
    let mut rng = Rng(seed | 1);
    let mut bp = Biquad::bandpass(2500.0, 0.8, sr);
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let tone = (2.0 * PI * 185.0 * t).sin() * (-t * 25.0).exp() * 0.5;
            let noise = bp.process(rng.next()) * (-t * 16.0).exp() * 1.6;
            tone + noise
        })
        .collect()
}

/// Hi-hat: high-pass-filtered noise, `open` sounds longer.
pub fn hat(sr: f32, seed: u32, open: bool) -> Vec<f32> {
    let dur = if open { 0.3 } else { 0.07 };
    let decay = if open { 12.0 } else { 60.0 };
    let n = (dur * sr) as usize;
    let mut rng = Rng(seed | 1);
    let mut hp = Biquad::highpass(7000.0, 0.7, sr);
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            hp.process(rng.next()) * (-t * decay).exp()
        })
        .collect()
}

/// Vibrato rate in Hz (typical for singing and strings).
pub const VIBRATO_HZ: f32 = 5.5;

/// A note to be played, with expression.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoteSpec {
    pub start_secs: f32,
    pub dur_secs: f32,
    /// May be fractional (e.g. after transposing by cent amounts).
    pub midi: f32,
    /// Velocity 0…1.
    pub velocity: f32,
    /// Vibrato depth in semitones (0…1).
    pub vibrato: f32,
    /// Brightness relative to the instrument (−1…+1).
    pub brightness: f32,
    /// Scoop-in: start offset in semitones, glides to the note (−12…+12).
    pub bend: f32,
}

impl NoteSpec {
    pub fn new(start_secs: f32, dur_secs: f32, midi: f32, velocity: f32) -> Self {
        Self { start_secs, dur_secs, midi, velocity, ..Default::default() }
    }

    /// Does the pitch change during the note (vibrato, scoop-in)?
    pub fn modulated(&self) -> bool {
        self.vibrato != 0.0 || self.bend != 0.0
    }

    /// Pitch offset in semitones `t` seconds after the note starts.
    /// Scoop-in: glides smoothly (quadratically) from the offset into the note.
    /// Vibrato: starts delayed and swells – like singers do.
    pub fn pitch_offset(&self, t: f32) -> f32 {
        let dur = self.dur_secs.max(0.05);
        let mut off = 0.0;
        if self.bend != 0.0 {
            let glide = (0.07 + 0.02 * self.bend.abs()).min(0.6 * dur);
            let rest = 1.0 - (t / glide).clamp(0.0, 1.0);
            off += self.bend * rest * rest;
        }
        if self.vibrato != 0.0 {
            let delay = (0.25 * dur).min(0.18);
            let ramp = (0.3 * dur).min(0.3).max(0.01);
            let amount = ((t - delay) / ramp).clamp(0.0, 1.0);
            off += self.vibrato * amount * (2.0 * PI * VIBRATO_HZ * t).sin();
        }
        off
    }

    /// Lowest offset the note reaches (for buffer sizes).
    pub fn lowest_offset(&self) -> f32 {
        self.bend.min(0.0) - self.vibrato.abs()
    }
}

/// Renders a note list with the plucked instrument into a mono buffer.
pub fn render_notes(notes: &[NoteSpec], total_secs: f32, sr: f32, p: &PluckParams) -> Vec<f32> {
    let mut out = vec![0f32; ((total_secs.max(0.0) + 0.1) * sr) as usize];
    for (i, n) in notes.iter().enumerate() {
        let start = (n.start_secs.max(0.0) * sr) as usize;
        if start >= out.len() {
            continue;
        }
        let tone = pluck_note(n, sr, p, i as u32 * 7919 + 17);
        for (j, v) in tone.iter().enumerate() {
            match out.get_mut(start + j) {
                Some(o) => *o += v,
                None => break,
            }
        }
    }
    out
}

/// Mixes `src` with gain `gain` at position `at` into `dst` (without overflow).
pub fn mix_into(dst: &mut [f32], src: &[f32], at: usize, gain: f32) {
    for (j, v) in src.iter().enumerate() {
        match dst.get_mut(at + j) {
            Some(o) => *o += v * gain,
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{detect_pitch, YinConfig};
    use crate::util::{median, peak};

    #[test]
    fn pluck_is_in_tune() {
        let sr = 48_000.0;
        for &f in &[110.0f32, 220.0, 440.0] {
            let x = pluck(f, 0.8, 1.0, sr, &PluckParams::default(), 3);
            let mut est: Vec<f32> = detect_pitch(&x[..(0.6 * sr) as usize], sr, &YinConfig::default())
                .iter()
                .filter_map(|p| p.freq)
                .collect();
            let m = median(&mut est);
            assert!((m - f).abs() / f < 0.015, "{f} Hz → {m} Hz");
        }
    }

    #[test]
    fn pluck_vibrato_and_bend_move_the_pitch() {
        let sr = 48_000.0;
        let p = PluckParams { decay: 0.9, ..Default::default() };
        let freqs = |x: &[f32], a: f32, b: f32| -> Vec<f32> {
            detect_pitch(&x[(a * sr) as usize..(b * sr) as usize], sr, &YinConfig::default()).iter().filter_map(|q| q.freq).collect()
        };
        // without expression, tuned unchanged
        let plain = pluck_note(&NoteSpec::new(0.0, 1.2, 57.0, 0.9), sr, &p, 3);
        let m = median(&mut freqs(&plain, 0.1, 1.0));
        assert!((m - 220.0).abs() < 3.0, "{m}");
        // Scoop-in from below: start clearly lower, end at 220 Hz
        let bent = pluck_note(&NoteSpec { bend: -4.0, ..NoteSpec::new(0.0, 1.2, 57.0, 0.9) }, sr, &p, 3);
        let early = median(&mut freqs(&bent, 0.0, 0.07));
        let late = median(&mut freqs(&bent, 0.5, 1.0));
        assert!(early < 205.0, "Anfang {early}");
        assert!((late - 220.0).abs() < 3.0, "Ende {late}");
        // Vibrato: pitch wavers around the note
        let vib = pluck_note(&NoteSpec { vibrato: 0.5, ..NoteSpec::new(0.0, 1.5, 57.0, 0.9) }, sr, &p, 3);
        let f = freqs(&vib, 0.5, 1.4);
        let (lo, hi) = f.iter().fold((f32::MAX, 0.0f32), |(l, h), &v| (l.min(v), h.max(v)));
        assert!(hi / lo > 1.03 && lo < 220.0 && hi > 220.0, "{lo}–{hi}");
    }

    #[test]
    fn decay_parameter_controls_sustain() {
        let sr = 48_000.0;
        let short = pluck(220.0, 1.5, 1.0, sr, &PluckParams { decay: 0.05, ..Default::default() }, 1);
        let long = pluck(220.0, 1.5, 1.0, sr, &PluckParams { decay: 0.95, ..Default::default() }, 1);
        let tail = |x: &[f32]| peak(&x[(1.0 * sr) as usize..(1.2 * sr) as usize]);
        assert!(tail(&long) > 4.0 * tail(&short));
    }

    #[test]
    fn drums_are_finite_and_bounded() {
        for x in [kick(48_000.0), snare(48_000.0, 5), hat(48_000.0, 9, true)] {
            assert!(x.iter().all(|v| v.is_finite()));
            assert!(peak(&x) < 2.0 && peak(&x) > 0.05);
        }
    }
}

