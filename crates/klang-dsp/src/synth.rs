//! Klangerzeugung. Keimzelle der Soundwerkstatt: Parameter tragen
//! sprechende Bedeutungen (Helligkeit, Abklingen, Körper, Anschlag), damit
//! Menschen und ein Agent sie über Klangbeschreibungen steuern können.

use crate::notes::midi_to_hz;
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Zupf-Instrument (erweitertes Karplus-Strong). Alle Werte 0…1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluckParams {
    /// dunkel ↔ hell (Dämpfung hoher Teiltöne in der Saite)
    pub brightness: f32,
    /// kurz ↔ lang ausklingend
    pub decay: f32,
    /// dünn ↔ warm/voll (Korpusresonanzen)
    pub body: f32,
    /// weich ↔ hart angeschlagen (Spektrum der Anregung)
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

/// Biquad (RBJ-Cookbook), transponierte Direktform II.
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
    /// Kuhschwanz-Filter für die Tiefen (`db` Anhebung/Absenkung unterhalb `f0`).
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
    /// Kuhschwanz-Filter für die Höhen (`db` oberhalb `f0`).
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
    /// Glocke: `db` um `f0` mit Güte `q`.
    pub(crate) fn peaking(f0: f32, q: f32, db: f32, sr: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let w0 = 2.0 * PI * f0 / sr;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        Self::new(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Ein gezupfter Ton. Länge = `duration_secs` plus kurzes Ausklingen.
pub fn pluck(freq: f32, duration_secs: f32, velocity: f32, sr: f32, p: &PluckParams, seed: u32) -> Vec<f32> {
    pluck_note(&NoteSpec::new(0.0, duration_secs, 69.0 + 12.0 * (freq / 440.0).log2(), velocity), sr, p, seed)
}

/// Eine schwingende Saite (Karplus-Strong): Verzögerungsleitung mit
/// Allpass-Feinstimmung und Schleifenfilter.
struct Saite {
    line: Vec<f32>,
    w: usize,
    /// Schleifenfilter: Gewicht des vorigen Samples (Helligkeit).
    s: f32,
    prev: f32,
    /// Allpass für den Bruchteil der Periode (genaue Stimmung ohne Höhenverlust).
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

    /// Periode (Samples, inkl. Verzögerung des Schleifenfilters) und Abklingzeit setzen.
    fn tune(&mut self, period_total: f32, t60: f32, freq: f32) {
        // Schleifenfilter verzögert um s; Rest: ganze Samples + Bruchteil im Bereich 0,5…1,5 (stabiler Allpass)
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
        // Allpass erster Ordnung (Thiran)
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

/// Ein gezupfter Ton mit Ausdruck: Anschlag (Lautstärke und Härte), Helligkeit,
/// Vibrato und Anschleifen (Verzögerungsleitung mit veränderlicher Länge).
///
/// Für einen natürlichen Klang: Anrissposition (Kammfilter auf der Anregung –
/// nah am Steg klingt es heller und dünner), zwei minimal verschieden
/// gestimmte Schwingungsebenen der Saite (leichtes Schweben statt starrem
/// Ton) und ein Korpus mit mehreren Resonanzen.
pub fn pluck_note(n: &NoteSpec, sr: f32, p: &PluckParams, seed: u32) -> Vec<f32> {
    let release = 0.08;
    let duration_secs = n.dur_secs;
    let velocity = n.velocity;
    let total = ((duration_secs.max(0.01) + release) * sr) as usize;
    let mut out = vec![0f32; total];
    let freq_of = |offset: f32| (midi_to_hz(n.midi + offset)).clamp(20.0, sr * 0.45);
    let freq = freq_of(n.pitch_offset(0.0));

    // Schleifenfilter: Mittelwert aus zwei Samples, gewichtet nach Helligkeit;
    // hohe Töne etwas heller, sonst klingen sie stumpf
    let register = ((n.midi - 60.0) / 36.0).clamp(-0.5, 1.0);
    let s = (0.5 - 0.45 * (p.brightness + 0.45 * n.brightness + 0.12 * register).clamp(0.0, 1.0)).max(0.03);
    let t60 = 0.25 + 7.0 * p.decay.clamp(0.0, 1.0).powf(1.5);

    // Puffer für die längste Periode (tiefster Ton beim Anschleifen/Vibrato).
    let longest = sr / freq_of(n.lowest_offset().min(n.pitch_offset(0.0)));
    let size = longest.ceil() as usize + 4;
    let period0 = sr / freq;

    // Anregung: Auslenkung der Saite beim Anreißen – ein Dreieck mit der Spitze an der
    // Anrissstelle (harter Anschlag nah am Steg, weicher weiter zur Mitte), dazu
    // gefiltertes Rauschen für Härte und Lebendigkeit (fester = mehr Obertöne)
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

    // zwei Schwingungsebenen: die zweite um gut einen Cent höher, klingt etwas kürzer
    let detune = 1.0 + 0.0007 + 0.0004 * (seed % 7) as f32 / 7.0;
    let mut a_str = Saite::new(&plucked, size, s);
    let mut b_str = Saite::new(&plucked, size, s);
    a_str.tune(period0, t60, freq);
    b_str.tune(period0 / detune, t60 * 0.8, freq * detune);

    // Korpus: Luftresonanz, zwei Holzresonanzen, etwas „Luft“ in den Höhen
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

/// Bassdrum: abfallende Sinus-Frequenz plus kurzer Klick.
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

/// Snare: Ton bei 185 Hz plus bandpassgefiltertes Rauschen.
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

/// Hi-Hat: hochpassgefiltertes Rauschen, `open` klingt länger.
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

/// Vibrato-Tempo in Hz (typisch für Gesang und Streicher).
pub const VIBRATO_HZ: f32 = 5.5;

/// Ein zu spielender Ton mit Ausdruck.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoteSpec {
    pub start_secs: f32,
    pub dur_secs: f32,
    /// Darf gebrochen sein (z. B. nach Transposition um Cent-Beträge).
    pub midi: f32,
    /// Anschlagstärke 0…1.
    pub velocity: f32,
    /// Vibrato-Tiefe in Halbtönen (0…1).
    pub vibrato: f32,
    /// Helligkeit relativ zum Instrument (−1…+1).
    pub brightness: f32,
    /// Anschleifen: Startversatz in Halbtönen, gleitet zum Ton (−12…+12).
    pub bend: f32,
}

impl NoteSpec {
    pub fn new(start_secs: f32, dur_secs: f32, midi: f32, velocity: f32) -> Self {
        Self { start_secs, dur_secs, midi, velocity, ..Default::default() }
    }

    /// Ändert sich die Tonhöhe während der Note (Vibrato, Anschleifen)?
    pub fn modulated(&self) -> bool {
        self.vibrato != 0.0 || self.bend != 0.0
    }

    /// Tonhöhenversatz in Halbtönen `t` Sekunden nach Notenbeginn.
    /// Anschleifen: vom Versatz weich (quadratisch) in den Ton gleiten.
    /// Vibrato: setzt verzögert ein und schwillt an – wie bei Sängern.
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

    /// Tiefster Versatz, den die Note erreicht (für Puffergrößen).
    pub fn lowest_offset(&self) -> f32 {
        self.bend.min(0.0) - self.vibrato.abs()
    }
}

/// Rendert eine Notenliste mit dem Zupf-Instrument in einen Mono-Puffer.
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

/// Mischt `src` mit Verstärkung `gain` ab Position `at` in `dst` (ohne Überlauf).
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
        // ohne Ausdruck unverändert gestimmt
        let plain = pluck_note(&NoteSpec::new(0.0, 1.2, 57.0, 0.9), sr, &p, 3);
        let m = median(&mut freqs(&plain, 0.1, 1.0));
        assert!((m - 220.0).abs() < 3.0, "{m}");
        // Anschleifen von unten: Anfang deutlich tiefer, Ende auf 220 Hz
        let bent = pluck_note(&NoteSpec { bend: -4.0, ..NoteSpec::new(0.0, 1.2, 57.0, 0.9) }, sr, &p, 3);
        let early = median(&mut freqs(&bent, 0.0, 0.07));
        let late = median(&mut freqs(&bent, 0.5, 1.0));
        assert!(early < 205.0, "Anfang {early}");
        assert!((late - 220.0).abs() < 3.0, "Ende {late}");
        // Vibrato: Tonhöhe schwankt um den Ton
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

