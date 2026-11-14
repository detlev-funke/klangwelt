//! Fugue: subject and answer (real or tonal), transformations (inversion,
//! retrograde, augmentation, diminution, transposition), strettos,
//! countersubject suggestions in double counterpoint and a fugue plan as a
//! framework (exposition, episodes, middle entries, stretto,
//! pedal point, final cadence).
//!
//! Everything works on [`VNote`] (pitch, entry, duration in beats).
//! Free voices are produced by [`generate_line`]; the plan is a starting point
//! for further writing, not a finished Bach.

use crate::counterpoint::{check, complementary_rhythm, generate_line, tally, IssueKind, LineSpec, Severity, VNote};
use crate::harmony::{parse_roman, voice_progression};
use crate::theory::Key;
use klang_i18n::{tk, tr, trn};
use serde::{Deserialize, Serialize};

const EPS: f64 = 1e-6;
const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];

fn key(tonic: u8, minor: bool) -> Key {
    Key { tonic: tonic % 12, minor, confidence: 1.0 }
}

/// Shift to start 0 and sort.
pub fn normalize(s: &[VNote]) -> Vec<VNote> {
    let mut v = s.to_vec();
    v.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.pitch.cmp(&a.pitch)));
    v.dedup_by(|b, a| (a.start - b.start).abs() < EPS);
    let t0 = v.first().map(|n| n.start).unwrap_or(0.0);
    for n in &mut v {
        n.start -= t0;
    }
    v
}

pub fn length(s: &[VNote]) -> f64 {
    s.iter().map(|n| n.end()).fold(0.0, f64::max)
}

fn shift(s: &[VNote], dt: f64, semis: i32) -> Vec<VNote> {
    s.iter().map(|n| VNote::new((n.pitch as i32 + semis).clamp(0, 127) as u8, n.start + dt, n.len)).collect()
}

fn mean(s: &[VNote]) -> f64 {
    if s.is_empty() {
        return 60.0;
    }
    s.iter().map(|n| n.pitch as f64 * n.len).sum::<f64>() / s.iter().map(|n| n.len).sum::<f64>().max(EPS)
}

// ------------------------------------------------------------------ Scale degrees

/// Pitch → (absolute scale degree = octave·7 + degree, alteration).
fn to_degree(p: u8, k: &Key) -> (i32, i32) {
    let steps = if k.minor { MINOR } else { MAJOR };
    let rel = p as i32 - k.tonic as i32;
    let (oct, off) = (rel.div_euclid(12), rel.rem_euclid(12));
    if let Some(d) = steps.iter().position(|&s| s == off) {
        return (oct * 7 + d as i32, 0);
    }
    let d = steps.iter().rposition(|&s| s < off).unwrap_or(0);
    (oct * 7 + d as i32, off - steps[d])
}

fn from_degree(pos: i32, alt: i32, k: &Key) -> u8 {
    let steps = if k.minor { MINOR } else { MAJOR };
    let (oct, d) = (pos.div_euclid(7), pos.rem_euclid(7));
    (k.tonic as i32 + oct * 12 + steps[d as usize] + alt).clamp(0, 127) as u8
}

/// In minor: the minor seventh before the tonic (ascending) is raised to the leading tone.
fn fix_leading_tones(s: &mut [VNote], k: &Key) {
    if !k.minor {
        return;
    }
    for i in 0..s.len().saturating_sub(1) {
        let (a, b) = (s[i].pitch, s[i + 1].pitch);
        let off = (a as i32 - k.tonic as i32).rem_euclid(12);
        if off == 10 && (b as i32 - a as i32) == 2 && (b as i32 - k.tonic as i32).rem_euclid(12) == 0 {
            s[i].pitch += 1;
        }
    }
}

/// Transpose diatonically (scale degrees within the key).
pub fn transpose_diatonic(s: &[VNote], k: &Key, steps: i32) -> Vec<VNote> {
    let mut v: Vec<VNote> = s
        .iter()
        .map(|n| {
            let (pos, _) = to_degree(n.pitch, k);
            VNote::new(from_degree(pos + steps, 0, k), n.start, n.len)
        })
        .collect();
    fix_leading_tones(&mut v, k);
    v
}

// ------------------------------------------------------------------ Transformations

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Transform {
    /// Mirroring around the first note, within the key.
    Umkehrung,
    /// Backwards.
    Krebs,
    Krebsumkehrung,
    /// Doubled note values.
    Vergroesserung,
    /// Halved note values.
    Verkleinerung,
    /// Transpose by scale degrees within the key.
    Versetzung(i32),
    /// Transpose by semitones.
    Chromatisch(i32),
}

impl Transform {
    pub fn label(&self) -> String {
        match self {
            Self::Umkehrung => tr!("Inversion"),
            Self::Krebs => tr!("Retrograde"),
            Self::Krebsumkehrung => tr!("Retrograde inversion"),
            Self::Vergroesserung => tr!("Augmentation"),
            Self::Verkleinerung => tr!("Diminution"),
            Self::Versetzung(s) if *s >= 0 => trn!(s.abs(), "{n} scale step higher", "{n} scale steps higher"),
            Self::Versetzung(s) => trn!(s.abs(), "{n} scale step lower", "{n} scale steps lower"),
            Self::Chromatisch(s) if *s >= 0 => trn!(s.abs(), "{n} semitone higher", "{n} semitones higher"),
            Self::Chromatisch(s) => trn!(s.abs(), "{n} semitone lower", "{n} semitones lower"),
        }
    }
}

pub fn transform(s: &[VNote], k: &Key, t: Transform) -> Vec<VNote> {
    let s = normalize(s);
    let Some(first) = s.first().copied() else { return s };
    let invert = |v: &[VNote]| -> Vec<VNote> {
        let (axis, _) = to_degree(first.pitch, k);
        let mut out: Vec<VNote> = v
            .iter()
            .map(|n| {
                let (pos, _) = to_degree(n.pitch, k);
                VNote::new(from_degree(2 * axis - pos, 0, k), n.start, n.len)
            })
            .collect();
        fix_leading_tones(&mut out, k);
        out
    };
    let retro = |v: &[VNote]| -> Vec<VNote> {
        let total = length(v);
        let mut out: Vec<VNote> = v.iter().map(|n| VNote::new(n.pitch, total - n.end(), n.len)).collect();
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        out
    };
    match t {
        Transform::Umkehrung => invert(&s),
        Transform::Krebs => retro(&s),
        Transform::Krebsumkehrung => retro(&invert(&s)),
        Transform::Vergroesserung => s.iter().map(|n| VNote::new(n.pitch, n.start * 2.0, n.len * 2.0)).collect(),
        Transform::Verkleinerung => s.iter().map(|n| VNote::new(n.pitch, n.start / 2.0, n.len / 2.0)).collect(),
        Transform::Versetzung(st) => transpose_diatonic(&s, k, st),
        Transform::Chromatisch(semi) => shift(&s, 0.0, semi),
    }
}

// ------------------------------------------------------------------ Answer

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnswerKind {
    Real,
    Tonal,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub kind: AnswerKind,
    /// A fifth higher (or, with a tonal answer, a fourth in the head).
    pub notes: Vec<VNote>,
    /// Indices of the notes that are altered compared to the real answer.
    pub mutated: Vec<usize>,
    pub reason: String,
}

/// Answer (comes) in the dominant. If the dominant is in the head of the subject
/// (only root and fifth at the start), the answer is tonal: fifth → root.
pub fn answer(subject: &[VNote], k: &Key) -> Answer {
    let s = normalize(subject);
    let off = |p: u8| (p as i32 - k.tonic as i32).rem_euclid(12);
    let head = s.iter().take_while(|n| off(n.pitch) == 0 || off(n.pitch) == 7).count();
    let tonal = s[..head].iter().any(|n| off(n.pitch) == 7);
    let mut mutated = Vec::new();
    let notes: Vec<VNote> = s
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let semis = if tonal && i < head && off(n.pitch) == 7 {
                mutated.push(i);
                5
            } else {
                7
            };
            VNote::new((n.pitch as i32 + semis).clamp(0, 127) as u8, n.start, n.len)
        })
        .collect();
    let names = ["c", "cis", "d", "es", "e", "f", "fis", "g", "gis", "a", "b", "h"];
    let (t, d) = (names[k.tonic as usize % 12], names[(k.tonic as usize + 7) % 12]);
    let reason = if tonal {
        tr!("Tonal answer: the subject starts within the range of tonic and fifth; the fifth {d} is answered by the tonic {t} so the key doesn't tip over. After that, a fifth higher.", d = d, t = t)
    } else {
        tr!("Real answer: the whole subject a fifth higher (in the dominant).")
    };
    Answer { kind: if tonal { AnswerKind::Tonal } else { AnswerKind::Real }, notes, mutated, reason }
}

// ------------------------------------------------------------------ Stretto

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stretto {
    /// Entry distance in beats.
    pub delay: f64,
    /// Interval of the second voice in semitones (after transposition).
    pub semitones: i32,
    pub label: String,
    pub errors: usize,
    pub warnings: usize,
    /// Portion in which both voices play the subject at the same time.
    pub overlap: f64,
    /// The second voice (comes) in beats relative to the dux.
    pub follower: Vec<VNote>,
}

const STRETTO_INTERVALS: [(&str, i32, bool); 9] = [
    (tk!("Octave higher"), 12, false),
    (tk!("Octave lower"), -12, false),
    (tk!("Fifth higher"), 4, true),
    (tk!("Fifth lower"), -4, true),
    (tk!("Fourth higher"), 3, true),
    (tk!("Fourth lower"), -3, true),
    (tk!("Third lower"), -2, true),
    (tk!("Sixth lower"), -5, true),
    (tk!("Twelfth lower"), -11, true),
];

/// Strettos: entry distances and intervals at which the subject sounds together
/// with itself without voice-leading errors – tightest first.
pub fn strettos(subject: &[VNote], k: &Key, beats_per_bar: u32) -> Vec<Stretto> {
    let s = normalize(subject);
    let len = length(&s);
    let mut out = Vec::new();
    let mut d = 1.0;
    while d <= len - 1.0 + EPS {
        for &(label, amount, diatonic) in &STRETTO_INTERVALS {
            let f0 = if diatonic { transpose_diatonic(&s, k, amount) } else { shift(&s, 0.0, amount) };
            let follower = shift(&f0, d, 0);
            let above = mean(&follower) > mean(&s);
            let (top, bottom) = if above { (follower.clone(), s.clone()) } else { (s.clone(), follower.clone()) };
            let issues = check(&[top, bottom], &["Oben".into(), "Unten".into()], Some(k), beats_per_bar);
            let vertical: Vec<_> = issues.into_iter().filter(|i| i.voices.len() == 2).collect();
            let (errors, warnings) = tally(&vertical);
            // free dissonances on weak beats count too: at most one per four shared beats
            let free = vertical.iter().filter(|i| i.kind == IssueKind::Dissonanz && i.severity == Severity::Hinweis).count();
            let together = (len - d).max(0.0);
            if errors == 0 && warnings <= 1 && (free as f64) <= (together / 4.0).max(1.0) {
                out.push(Stretto {
                    delay: d,
                    semitones: (follower.first().map(|n| n.pitch as i32).unwrap_or(0)) - s.first().map(|n| n.pitch as i32).unwrap_or(0),
                    label: klang_i18n::translate(label),
                    errors,
                    warnings,
                    overlap: ((len - d) / len).max(0.0),
                    follower,
                });
            }
        }
        d += 1.0;
    }
    out.sort_by(|a, b| a.delay.total_cmp(&b.delay).then(a.warnings.cmp(&b.warnings)));
    out
}

// ------------------------------------------------------------------ Countersubject

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Countersubject {
    pub notes: Vec<VNote>,
    /// Generated above the subject.
    pub above: bool,
    pub errors: usize,
    pub warnings: usize,
    /// Still error-free with octave swap (countersubject on the other side).
    pub invertible: bool,
    pub inverted_errors: usize,
    pub inverted_warnings: usize,
}

/// Move to the other side of the subject (octaves).
fn swap_side(cs: &[VNote], subject: &[VNote], above: bool) -> Vec<VNote> {
    let (mc, ms) = (mean(cs), mean(subject));
    let mut k = 0;
    if above {
        while mc + 12.0 * k as f64 > ms - 3.0 {
            k -= 1;
        }
    } else {
        while mc + 12.0 * (k as f64) < ms + 3.0 {
            k += 1;
        }
    }
    shift(cs, 0.0, 12 * k)
}

/// Countersubject suggestions for the subject, best first.
pub fn countersubjects(subject: &[VNote], k: &Key, beats_per_bar: u32, count: usize) -> Vec<Countersubject> {
    let s = normalize(subject);
    let len = length(&s);
    let m = mean(&s).round() as i32;
    let slots = complementary_rhythm(&s, 0.0, len, beats_per_bar);
    let mut out: Vec<(f32, Countersubject)> = Vec::new();
    for above in [true, false] {
        let range = if above { ((m + 3).clamp(30, 90) as u8, (m + 17).clamp(40, 100) as u8) } else { ((m - 17).clamp(24, 80) as u8, (m - 3).clamp(30, 90) as u8) };
        for variant in 0..5 {
            let spec = LineSpec { above: Some(above), invertible: true, variant, ..LineSpec::new(range, slots.clone()) };
            let cs = generate_line(&[&s], k, &spec, beats_per_bar);
            if cs.is_empty() || out.iter().any(|(_, c)| c.notes == cs) {
                continue;
            }
            let names = ["Oben".to_string(), "Unten".to_string()];
            let pair = |a: &[VNote], b: &[VNote], a_above: bool| {
                let v = if a_above { vec![a.to_vec(), b.to_vec()] } else { vec![b.to_vec(), a.to_vec()] };
                let issues: Vec<_> = check(&v, &names, Some(k), beats_per_bar).into_iter().filter(|i| i.voices.len() == 2).collect();
                tally(&issues)
            };
            let (errors, warnings) = pair(&cs, &s, above);
            let inv = swap_side(&cs, &s, above);
            let (ie, iw) = pair(&inv, &s, !above);
            let invertible = ie == 0 && iw <= 1;
            let score = errors as f32 * 10.0 + warnings as f32 * 2.0 + ie as f32 * 4.0 + iw as f32 + if invertible { 0.0 } else { 3.0 } + variant as f32 * 0.1;
            out.push((score, Countersubject { notes: cs, above, errors, warnings, invertible, inverted_errors: ie, inverted_warnings: iw }));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().take(count).map(|(_, c)| c).collect()
}

// ------------------------------------------------------------------ Plan

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    /// Kind of section (untranslated): exposition, zwischenspiel,
    /// durchfuehrung, engfuehrung, schlussdurchfuehrung, orgelpunkt, schlusskadenz.
    pub kind: &'static str,
    /// Name for display (translated).
    pub name: String,
    pub start: f64,
    pub end: f64,
    pub key: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedVoice {
    pub name: String,
    pub notes: Vec<VNote>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FuguePlan {
    pub sections: Vec<Section>,
    pub voices: Vec<PlannedVoice>,
    pub length: f64,
    pub answer: AnswerKind,
    pub stretto: Option<String>,
    pub countersubject: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FugueOptions {
    /// 2 to 4 voices.
    pub voices: usize,
    #[serde(default)]
    pub beats_per_bar: u32,
    #[serde(default = "yes")]
    pub countersubject: bool,
    #[serde(default = "yes")]
    pub stretto: bool,
    /// In minor with a major final chord.
    #[serde(default = "yes")]
    pub picardy: bool,
}

fn yes() -> bool {
    true
}

struct VoiceDef {
    name: &'static str,
    range: (u8, u8),
    center: f64,
}

fn voice_defs(n: usize) -> Vec<VoiceDef> {
    let all = [
        VoiceDef { name: tk!("Soprano"), range: (62, 79), center: 70.0 },
        VoiceDef { name: tk!("Alto"), range: (55, 72), center: 64.0 },
        VoiceDef { name: tk!("Tenor"), range: (48, 67), center: 57.0 },
        VoiceDef { name: tk!("Bass"), range: (38, 58), center: 48.0 },
    ];
    let pick: &[usize] = match n {
        2 => &[0, 3],
        3 => &[0, 1, 3],
        _ => &[0, 1, 2, 3],
    };
    pick.iter()
        .map(|&i| {
            let d = &all[i];
            let name = match (n, i) {
                (2, 0) => tk!("Upper voice"),
                (2, 3) => tk!("Lower voice"),
                _ => d.name,
            };
            VoiceDef { name, range: d.range, center: d.center }
        })
        .collect()
}

/// Name of a voice for display.
fn voice_name(d: &VoiceDef) -> String {
    klang_i18n::translate(d.name)
}

/// Octave-shift into the range of a voice.
fn fit_octave(s: &[VNote], range: (u8, u8), center: f64) -> Vec<VNote> {
    let m = mean(s);
    let best = (-4..=4)
        .map(|k| {
            let out = s.iter().filter(|n| (n.pitch as i32 + 12 * k) < range.0 as i32 - 2 || (n.pitch as i32 + 12 * k) > range.1 as i32 + 2).count();
            (k, (m + 12.0 * k as f64 - center).abs() + 4.0 * out as f64)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|x| x.0)
        .unwrap_or(0);
    shift(s, 0.0, 12 * best)
}

/// Octave-shift into its own range above or below another voice.
fn fit_side(s: &[VNote], other: &[VNote], above: bool, range: (u8, u8), center: f64) -> Vec<VNote> {
    let (m, mo) = (mean(s), mean(other));
    let best = (-4..=4)
        .filter(|&k| if above { m + 12.0 * k as f64 > mo + 1.0 } else { m + 12.0 * (k as f64) < mo - 1.0 })
        .map(|k| {
            let out = s.iter().filter(|n| (n.pitch as i32 + 12 * k) < range.0 as i32 - 3 || (n.pitch as i32 + 12 * k) > range.1 as i32 + 3).count();
            (k, (m + 12.0 * k as f64 - center).abs() + 3.0 * out as f64)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|x| x.0);
    match best {
        Some(k) => shift(s, 0.0, 12 * k),
        None => fit_octave(s, range, center),
    }
}

/// Calm rhythm for free voices: quarters, halves where the main voice is very busy.
fn calm_rhythm(reference: &[VNote], from: f64, to: f64) -> Vec<(f64, f64)> {
    let mut slots = Vec::new();
    let mut t = from;
    while t < to - EPS {
        let busy = |a: f64, b: f64| reference.iter().filter(|n| n.start >= a - EPS && n.start < b - EPS).count() >= 2;
        if t + 2.0 <= to + EPS && (t - from).rem_euclid(2.0) < EPS && busy(t, t + 1.0) && busy(t + 1.0, t + 2.0) {
            slots.push((t, 2.0));
            t += 2.0;
        } else {
            let len = (to - t).min(1.0);
            slots.push((t, len));
            t += len;
        }
    }
    slots
}

struct Builder<'a> {
    defs: Vec<VoiceDef>,
    voices: Vec<Vec<VNote>>,
    bpb: u32,
    key: &'a Key,
}

impl Builder<'_> {
    fn put(&mut self, v: usize, notes: Vec<VNote>) {
        self.voices[v].extend(notes);
    }

    fn window(&self, v: usize, a: f64, b: f64) -> Vec<VNote> {
        self.voices[v].iter().filter(|n| n.end() > a + EPS && n.start < b - EPS).copied().collect()
    }

    /// Free voice in [a, b) against everything already there.
    fn free(&mut self, v: usize, a: f64, b: f64, k: &Key, reference: &[VNote], calm: bool) {
        let others: Vec<Vec<VNote>> = (0..self.voices.len()).filter(|&o| o != v).map(|o| self.window(o, a, b)).collect();
        let refs: Vec<&[VNote]> = others.iter().map(|x| x.as_slice()).collect();
        let slots = if calm { calm_rhythm(reference, a, b) } else { complementary_rhythm(reference, a, b, self.bpb) };
        // neighboring voices (smaller number = higher) limit the range
        let ceiling = if v > 0 { self.window(v - 1, a, b) } else { Vec::new() };
        let floor = if v + 1 < self.voices.len() { self.window(v + 1, a, b) } else { Vec::new() };
        let spec = LineSpec { variant: (a as u32) % 7, ceiling, floor, ..LineSpec::new(self.defs[v].range, slots) };
        let line = generate_line(&refs, k, &spec, self.bpb);
        self.put(v, line);
    }

    /// Entry: bring the material into the range of the voice and place it from t.
    fn entry(&mut self, v: usize, material: &[VNote], t: f64) -> Vec<VNote> {
        let fitted = shift(&fit_octave(material, self.defs[v].range, self.defs[v].center), t, 0);
        self.put(v, fitted.clone());
        fitted
    }

    fn finish(mut self) -> Vec<PlannedVoice> {
        for v in &mut self.voices {
            v.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.pitch.cmp(&a.pitch)));
            v.dedup_by(|b, a| (a.start - b.start).abs() < EPS);
            // shorten overlaps within a voice
            for i in 0..v.len().saturating_sub(1) {
                let next = v[i + 1].start;
                if v[i].end() > next + EPS {
                    v[i].len = (next - v[i].start).max(0.05);
                }
            }
            v.retain(|n| n.len >= 0.05);
        }
        let _ = self.key;
        self.defs.iter().zip(self.voices).map(|(d, notes)| PlannedVoice { name: voice_name(d), notes }).collect()
    }
}

/// Fugue plan: from a subject, a framework for 2–4 voices.
pub fn plan_fugue(subject: &[VNote], k: &Key, opt: &FugueOptions) -> FuguePlan {
    let n = opt.voices.clamp(2, 4);
    let bpb = opt.beats_per_bar.max(1);
    let bar = bpb as f64;
    let s = normalize(subject);
    let len = length(&s).max(1.0);
    let entry_gap = (len / bar - EPS).ceil().max(1.0) * bar;
    let ans = answer(&s, k);
    let cs = if opt.countersubject { countersubjects(&s, k, bpb, 1).into_iter().next() } else { None };
    let dom = key(k.tonic + 7, k.minor);
    let rel = if k.minor { key(k.tonic + 3, false) } else { key(k.tonic + 9, true) };
    let sub = key(k.tonic + 5, k.minor);
    let defs = voice_defs(n);
    let order: Vec<usize> = match n {
        2 => vec![0, 1],
        3 => vec![1, 0, 2],
        _ => vec![1, 0, 2, 3],
    };
    let mut b = Builder { defs, voices: vec![Vec::new(); n], bpb, key: k };
    let mut sections: Vec<Section> = Vec::new();
    let names = |kk: &Key| kk.name();

    // ---------------- Exposition
    for (i, &v) in order.iter().enumerate() {
        let t = i as f64 * entry_gap;
        let is_answer = i % 2 == 1;
        let (material, kk) = if is_answer { (ans.notes.clone(), dom) } else { (s.clone(), *k) };
        let entered = b.entry(v, &material, t);
        if i == 0 {
            continue;
        }
        let prev = order[i - 1];
        let mut handled = vec![v];
        if let Some(c) = &cs {
            let mat = if is_answer { shift(&c.notes, 0.0, 7) } else { c.notes.clone() };
            let above = prev < v; // smaller number = higher voice
            let placed = fit_side(&shift(&mat, t, 0), &entered, above, b.defs[prev].range, b.defs[prev].center);
            b.put(prev, placed);
            handled.push(prev);
        }
        for &o in &order[..i] {
            if !handled.contains(&o) {
                b.free(o, t, t + entry_gap, &kk, &entered, true);
            }
        }
    }
    let expo_end = n as f64 * entry_gap;
    sections.push(Section {
        kind: "exposition",
        name: tr!("Exposition"),
        start: 0.0,
        end: expo_end,
        key: names(k),
        description: match (ans.kind == AnswerKind::Tonal, cs.is_some()) {
            (true, true) => tr!("{n} entries: subject and tonal answer alternating, plus the countersubject in the voice that entered before", n = n),
            (true, false) => tr!("{n} entries: subject and tonal answer alternating", n = n),
            (false, true) => tr!("{n} entries: subject and real answer alternating, plus the countersubject in the voice that entered before", n = n),
            (false, false) => tr!("{n} entries: subject and real answer alternating", n = n),
        },
    });

    // Episode: sequence of the subject head on top, descending fifths in the bass, middle free
    let head_len = len.min(bar);
    let head: Vec<VNote> = s.iter().filter(|x| x.start < head_len - EPS).map(|x| VNote::new(x.pitch, x.start, x.len.min(head_len - x.start))).collect();
    let episode = |b: &mut Builder, t0: f64, target: &Key, sections: &mut Vec<Section>, nr: usize| -> f64 {
        let dur = 2.0 * bar;
        let top = 0;
        let bass = n - 1;
        let reps = (dur / head_len).floor().max(1.0) as i32;
        let mut seq = Vec::new();
        for r in 0..reps {
            seq.extend(shift(&transpose_diatonic(&head, k, -r), t0 + r as f64 * head_len, 0));
        }
        let seq = fit_octave(&seq, b.defs[top].range, b.defs[top].center);
        b.put(top, seq.clone());
        // descending-fifth roots in the bass, half a bar each, finally the dominant of the target key
        let half = (bar / 2.0).max(1.0);
        let m = (dur / half).round() as i32;
        let mut bassline = Vec::new();
        for j in 0..m {
            let root = (target.tonic as i32 + 7 + 7 * (m - 1 - j)).rem_euclid(12);
            let mut p = 36 + root;
            while p < 41 {
                p += 12;
            }
            bassline.push(VNote::new(p as u8, t0 + j as f64 * half, half));
        }
        b.put(bass, bassline);
        for v in 1..n - 1 {
            b.free(v, t0, t0 + dur, target, &seq, true);
        }
        sections.push(Section {
            kind: "zwischenspiel",
            name: tr!("Episode {nr}", nr = nr),
            start: t0,
            end: t0 + dur,
            key: format!("→ {}", target.name()),
            description: tr!("Sequence of the subject's head (stepping down) over falling fifths in the bass"),
        });
        t0 + dur
    };

    // Middle entry: subject in one voice, countersubject in another, the rest free
    let middle_entry = |b: &mut Builder, t0: f64, material: &[VNote], cs_mat: Option<Vec<VNote>>, v: usize, kk: &Key, sections: &mut Vec<Section>, (kind, name): (&'static str, String)| -> f64 {
        let entered = b.entry(v, material, t0);
        let mut handled = vec![v];
        if let Some(c) = cs_mat {
            let cv = if v == 0 { 1.min(n - 1) } else { 0 };
            let placed = fit_side(&shift(&c, t0, 0), &entered, cv < v, b.defs[cv].range, b.defs[cv].center);
            b.put(cv, placed);
            handled.push(cv);
        }
        for o in 0..n {
            if !handled.contains(&o) {
                b.free(o, t0, t0 + entry_gap, kk, &entered, true);
            }
        }
        let voice = voice_name(&b.defs[v]);
        sections.push(Section {
            kind,
            name,
            start: t0,
            end: t0 + entry_gap,
            key: kk.name(),
            description: if handled.len() > 1 { tr!("Subject in {voice}, with countersubject", voice = voice) } else { tr!("Subject in {voice}", voice = voice) },
        });
        t0 + entry_gap
    };

    let mut t = expo_end;
    t = episode(&mut b, t, &rel, &mut sections, 1);
    // Relative key: diatonically two scale degrees (major → minor downwards, minor → major upwards)
    let steps = if k.minor { 2 } else { -2 };
    let mut rel_subject = transpose_diatonic(&s, k, steps);
    fix_leading_tones(&mut rel_subject, &rel);
    let rel_cs = cs.as_ref().map(|c| {
        let mut v = transpose_diatonic(&c.notes, k, steps);
        fix_leading_tones(&mut v, &rel);
        v
    });
    let v1 = if n >= 3 { n - 2 } else { 1 };
    t = middle_entry(&mut b, t, &rel_subject, rel_cs, v1, &rel, &mut sections, ("durchfuehrung", tr!("Middle entries (relative key)")));
    t = episode(&mut b, t, &sub, &mut sections, 2);
    let sub_subject = shift(&s, 0.0, 5);
    let sub_cs = cs.as_ref().map(|c| shift(&c.notes, 0.0, 5));
    t = middle_entry(&mut b, t, &sub_subject, sub_cs, 0, &sub, &mut sections, ("durchfuehrung", tr!("Middle entries (subdominant)")));
    t = episode(&mut b, t, k, &mut sections, 3);

    // ---------------- Stretto (or final middle entry)
    let st = if opt.stretto { strettos(&s, k, bpb).into_iter().next() } else { None };
    let stretto_name = st.as_ref().map(|x| tr!("{label}, {delay} beats apart", label = x.label, delay = x.delay));
    match &st {
        Some(x) => {
            let lead_v = n - 1;
            let follow_v = 0;
            let lead = b.entry(lead_v, &s, t);
            let follower = fit_side(&shift(&x.follower, t, 0), &lead, true, b.defs[follow_v].range, b.defs[follow_v].center);
            b.put(follow_v, follower.clone());
            let end = t + ((x.delay + len) / bar - EPS).ceil() * bar;
            for o in 1..n - 1 {
                b.free(o, t, end, k, &lead, true);
            }
            // fill the gap of the following voice before its entry
            b.free(follow_v, t, t + x.delay, k, &lead, true);
            sections.push(Section {
                kind: "engfuehrung",
                name: tr!("Stretto"),
                start: t,
                end,
                key: k.name(),
                description: tr!("Subject in {lead} and – {interval} – in {follower}", lead = voice_name(&b.defs[lead_v]), interval = x.label, follower = voice_name(&b.defs[follow_v])),
            });
            t = end;
        }
        None => {
            t = middle_entry(&mut b, t, &s, cs.as_ref().map(|c| c.notes.clone()), n - 1, k, &mut sections, ("schlussdurchfuehrung", tr!("Final entries")));
        }
    }

    // ---------------- Pedal point on the dominant
    let pedal_len = 2.0 * bar;
    let mut dp = 36 + ((k.tonic as i32 + 7) % 12);
    while dp < 40 {
        dp += 12;
    }
    b.put(n - 1, vec![VNote::new(dp as u8, t, pedal_len)]);
    let top_head = fit_octave(&shift(&head, t, 0), b.defs[0].range, b.defs[0].center);
    b.put(0, top_head.clone());
    b.free(0, t + head_len, t + pedal_len, k, &[], true);
    for v in 1..n - 1 {
        b.free(v, t, t + pedal_len, k, &top_head, true);
    }
    sections.push(Section {
        kind: "orgelpunkt",
        name: tr!("Pedal point"),
        start: t,
        end: t + pedal_len,
        key: k.name(),
        description: tr!("Bass holds the dominant, the subject's head on top"),
    });
    t += pedal_len;

    // ---------------- Final cadence
    let tonic_id = if k.minor && opt.picardy { "I" } else if k.minor { "i" } else { "I" };
    let six_four = if k.minor { "i64" } else { "I64" };
    let chords: Vec<_> = [six_four, "V7", tonic_id].iter().filter_map(|id| parse_roman(id, k)).collect();
    let voicing = voice_progression(&chords, Some(k), None);
    let durs = [bar / 2.0, bar / 2.0, bar];
    let mut tt = t;
    for (ci, v4) in voicing.iter().enumerate() {
        // v4 = [bass, tenor, alto, soprano]
        let pick: Vec<u8> = match n {
            2 => vec![v4[3], v4[0]],
            3 => vec![v4[3], v4[2], v4[0]],
            _ => vec![v4[3], v4[2], v4[1], v4[0]],
        };
        for (vi, &p) in pick.iter().enumerate() {
            let fitted = fit_octave(&[VNote::new(p, 0.0, 1.0)], b.defs[vi].range, b.defs[vi].center)[0].pitch;
            b.put(vi, vec![VNote::new(fitted, tt, durs[ci])]);
        }
        tt += durs[ci];
    }
    sections.push(Section {
        kind: "schlusskadenz",
        name: tr!("Final cadence"),
        start: t,
        end: tt,
        key: k.name(),
        description: if k.minor && opt.picardy { tr!("Cadential six-four – D⁷ – tonic with Picardy third") } else { tr!("Cadential six-four – D⁷ – tonic") },
    });

    FuguePlan { sections, voices: b.finish(), length: tt, answer: ans.kind, stretto: stretto_name, countersubject: cs.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(notes: &[(u8, f64)]) -> Vec<VNote> {
        let mut t = 0.0;
        notes
            .iter()
            .map(|&(p, d)| {
                let n = VNote::new(p, t, d);
                t += d;
                n
            })
            .collect()
    }

    fn c() -> Key {
        key(0, false)
    }

    /// Subject in the style of a fugue subject in C major (2 bars).
    fn subject() -> Vec<VNote> {
        line(&[(60, 1.0), (62, 0.5), (64, 0.5), (65, 1.5), (64, 0.5), (62, 0.5), (64, 0.5), (60, 1.0), (67, 1.0), (65, 0.5), (64, 0.5), (62, 1.0)])
    }

    #[test]
    fn real_and_tonal_answers() {
        let k = c();
        let real = answer(&subject(), &k);
        assert_eq!(real.kind, AnswerKind::Real);
        assert_eq!(real.notes[0].pitch, 67);
        assert!(real.mutated.is_empty());
        // begins with g–c: tonal, g is answered with c
        let s = line(&[(67, 1.0), (60, 1.0), (62, 1.0), (64, 1.0), (65, 1.0), (64, 2.0)]);
        let t = answer(&s, &k);
        assert_eq!(t.kind, AnswerKind::Tonal);
        assert_eq!(t.notes[0].pitch, 72, "g → c");
        assert_eq!(t.notes[1].pitch, 67, "c → g");
        assert_eq!(t.notes[2].pitch, 69, "danach real: d → a");
        assert_eq!(t.mutated, vec![0]);
    }

    #[test]
    fn transformations() {
        let k = c();
        let s = line(&[(60, 1.0), (62, 1.0), (64, 2.0)]);
        let inv: Vec<u8> = transform(&s, &k, Transform::Umkehrung).iter().map(|n| n.pitch).collect();
        assert_eq!(inv, vec![60, 59, 57], "c d e → c h a");
        let krebs = transform(&s, &k, Transform::Krebs);
        assert_eq!(krebs.iter().map(|n| (n.pitch, n.start, n.len)).collect::<Vec<_>>(), vec![(64, 0.0, 2.0), (62, 2.0, 1.0), (60, 3.0, 1.0)]);
        let aug = transform(&s, &k, Transform::Vergroesserung);
        assert_eq!(length(&aug), 8.0);
        assert_eq!(transform(&s, &k, Transform::Versetzung(1)).iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![62, 64, 65]);
        // in minor: leading tone before the tonic
        let am = key(9, true);
        let m = line(&[(64, 1.0), (62, 1.0), (60, 1.0), (59, 1.0)]); // e d c b → one scale degree lower: d c b a?
        let down = transform(&m, &am, Transform::Versetzung(-1));
        assert_eq!(down.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![62, 60, 59, 57]);
        // e g a: g rises stepwise to the tonic a → leading tone g-sharp (melodic minor ascending)
        let up = line(&[(64, 1.0), (67, 1.0), (69, 1.0)]);
        assert_eq!(transform(&up, &am, Transform::Versetzung(0))[1].pitch, 68);
        // descending, g stays
        let down2 = line(&[(69, 1.0), (67, 1.0), (65, 1.0)]);
        assert_eq!(transform(&down2, &am, Transform::Versetzung(0))[1].pitch, 67);
    }

    #[test]
    fn finds_a_canonic_stretto() {
        // Frère Jacques is a canon at a distance of two bars
        let s = line(&[
            (60, 1.0), (62, 1.0), (64, 1.0), (60, 1.0), (60, 1.0), (62, 1.0), (64, 1.0), (60, 1.0),
            (64, 1.0), (65, 1.0), (67, 2.0), (64, 1.0), (65, 1.0), (67, 2.0),
        ]);
        let st = strettos(&s, &c(), 4);
        assert!(st.iter().any(|x| x.delay == 8.0 && x.label.starts_with("Octave")), "{:?}", st.iter().map(|x| (x.delay, &x.label)).collect::<Vec<_>>());
        assert!(st.iter().all(|x| x.errors == 0));
    }

    #[test]
    fn countersubject_is_clean_and_mostly_invertible() {
        let cs = countersubjects(&subject(), &c(), 4, 3);
        assert!(!cs.is_empty());
        assert_eq!(cs[0].errors, 0, "{:?}", cs[0]);
        assert!(cs.iter().any(|x| x.invertible), "{:?}", cs.iter().map(|x| (x.errors, x.warnings, x.inverted_errors, x.inverted_warnings)).collect::<Vec<_>>());
        assert!((length(&cs[0].notes) - length(&subject())).abs() < 1e-6);
    }

    #[test]
    fn plan_has_all_sections_and_few_errors() {
        for (n, k) in [(3usize, c()), (4, c()), (2, key(9, true))] {
            let s = if k.minor { transform(&subject(), &c(), Transform::Chromatisch(-3)) } else { subject() };
            let plan = plan_fugue(&s, &k, &FugueOptions { voices: n, beats_per_bar: 4, countersubject: true, stretto: true, picardy: true });
            assert_eq!(plan.voices.len(), n);
            assert!(plan.voices.iter().all(|v| !v.notes.is_empty()));
            let names: Vec<&str> = plan.sections.iter().map(|x| x.name.as_str()).collect();
            assert!(names.contains(&"Exposition") && names.contains(&"Pedal point") && names.contains(&"Final cadence"), "{names:?}");
            assert!(plan.sections.windows(2).all(|w| (w[0].end - w[1].start).abs() < 1e-6), "lückenlos");
            assert!((plan.length - plan.sections.last().unwrap().end).abs() < 1e-6);
            // count voice-leading errors (framework: a few allowed)
            let voices: Vec<Vec<VNote>> = plan.voices.iter().map(|v| v.notes.clone()).collect();
            let names: Vec<String> = plan.voices.iter().map(|v| v.name.clone()).collect();
            let issues = check(&voices, &names, Some(&k), 4);
            let errors = issues.iter().filter(|i| i.severity == Severity::Fehler).count();
            let mut kinds: Vec<(String, usize)> = Vec::new();
            for i in issues.iter().filter(|i| i.severity == Severity::Warnung) {
                let k = format!("{:?}", i.kind);
                match kinds.iter_mut().find(|x| x.0 == k) {
                    Some(x) => x.1 += 1,
                    None => kinds.push((k, 1)),
                }
            }
            eprintln!("{n} Stimmen {}: {} Takte, {errors} Fehler, Warnungen {kinds:?}", k.name_de(), plan.length / 4.0);
            assert!(errors <= 8, "{:#?}", issues.iter().filter(|i| i.severity == Severity::Fehler).collect::<Vec<_>>());
        }
    }
}
