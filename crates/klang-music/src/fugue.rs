//! Fuge: Thema und Antwort (real oder tonal), Verwandlungen (Umkehrung,
//! Krebs, Vergrößerung, Verkleinerung, Versetzung), Engführungen,
//! Gegensatz-Vorschläge im doppelten Kontrapunkt und ein Fugenplan als
//! Gerüst (Exposition, Zwischenspiele, Durchführungen, Engführung,
//! Orgelpunkt, Schlusskadenz).
//!
//! Alles arbeitet auf [`VNote`] (Tonhöhe, Einsatz, Dauer in Schlägen).
//! Freie Stimmen erzeugt [`generate_line`]; der Plan ist ein Ausgangspunkt
//! zum Weiterschreiben, kein fertiger Bach.

use crate::counterpoint::{check, complementary_rhythm, generate_line, tally, IssueKind, LineSpec, Severity, VNote};
use crate::harmony::{parse_roman, voice_progression};
use crate::theory::Key;
use serde::{Deserialize, Serialize};

const EPS: f64 = 1e-6;
const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];

fn key(tonic: u8, minor: bool) -> Key {
    Key { tonic: tonic % 12, minor, confidence: 1.0 }
}

/// Auf Beginn 0 verschieben und sortieren.
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

// ------------------------------------------------------------------ Stufen

/// Tonhöhe → (Stufe absolut = Oktave·7 + Stufe, Alteration).
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

/// In Moll: kleine Septime vor der Tonika (aufwärts) wird zum Leitton erhöht.
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

/// Diatonisch versetzen (Stufen innerhalb der Tonart).
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

// ------------------------------------------------------------------ Verwandlungen

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Transform {
    /// Spiegelung um den ersten Ton, innerhalb der Tonart.
    Umkehrung,
    /// Rückwärts.
    Krebs,
    Krebsumkehrung,
    /// Doppelte Notenwerte.
    Vergroesserung,
    /// Halbe Notenwerte.
    Verkleinerung,
    /// Um Stufen innerhalb der Tonart versetzen.
    Versetzung(i32),
    /// Um Halbtöne versetzen.
    Chromatisch(i32),
}

impl Transform {
    pub fn label(&self) -> String {
        match self {
            Self::Umkehrung => "Umkehrung".into(),
            Self::Krebs => "Krebs".into(),
            Self::Krebsumkehrung => "Krebsumkehrung".into(),
            Self::Vergroesserung => "Vergrößerung".into(),
            Self::Verkleinerung => "Verkleinerung".into(),
            Self::Versetzung(s) => format!("um {} Stufe{} {}", s.abs(), if s.abs() == 1 { "" } else { "n" }, if *s >= 0 { "höher" } else { "tiefer" }),
            Self::Chromatisch(s) => format!("um {} Halbt{} {}", s.abs(), if s.abs() == 1 { "on" } else { "öne" }, if *s >= 0 { "höher" } else { "tiefer" }),
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

// ------------------------------------------------------------------ Antwort

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
    /// Eine Quinte höher (bzw. bei tonaler Antwort im Kopf eine Quarte).
    pub notes: Vec<VNote>,
    /// Indizes der Töne, die gegenüber der realen Antwort verändert sind.
    pub mutated: Vec<usize>,
    pub reason: String,
}

/// Antwort (Comes) in der Dominante. Steht die Dominante im Kopf des Themas
/// (nur Grundton und Quinte am Anfang), wird tonal beantwortet: Quinte → Grundton.
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
        format!("Tonale Antwort: Das Thema beginnt im Raum von Grundton und Quinte; die Quinte {d} wird mit dem Grundton {t} beantwortet, damit die Tonart nicht kippt. Danach eine Quinte höher.")
    } else {
        "Reale Antwort: das ganze Thema eine Quinte höher (in der Dominante).".to_string()
    };
    Answer { kind: if tonal { AnswerKind::Tonal } else { AnswerKind::Real }, notes, mutated, reason }
}

// ------------------------------------------------------------------ Engführung

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stretto {
    /// Einsatzabstand in Schlägen.
    pub delay: f64,
    /// Intervall der zweiten Stimme in Halbtönen (nach Versetzung).
    pub semitones: i32,
    pub label: String,
    pub errors: usize,
    pub warnings: usize,
    /// Anteil, in dem beide Stimmen gleichzeitig das Thema spielen.
    pub overlap: f64,
    /// Die zweite Stimme (Comes) in Schlägen relativ zum Dux.
    pub follower: Vec<VNote>,
}

const STRETTO_INTERVALS: [(&str, i32, bool); 9] = [
    ("Oktave höher", 12, false),
    ("Oktave tiefer", -12, false),
    ("Quinte höher", 4, true),
    ("Quinte tiefer", -4, true),
    ("Quarte höher", 3, true),
    ("Quarte tiefer", -3, true),
    ("Terz tiefer", -2, true),
    ("Sexte tiefer", -5, true),
    ("Duodezime tiefer", -11, true),
];

/// Engführungen: Einsatzabstände und Intervalle, bei denen das Thema mit
/// sich selbst ohne Satzfehler zusammenklingt – engste zuerst.
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
            // freie Dissonanzen auf leichter Zeit zählen mit: höchstens eine je vier gemeinsame Schläge
            let free = vertical.iter().filter(|i| i.kind == IssueKind::Dissonanz && i.severity == Severity::Hinweis).count();
            let together = (len - d).max(0.0);
            if errors == 0 && warnings <= 1 && (free as f64) <= (together / 4.0).max(1.0) {
                out.push(Stretto {
                    delay: d,
                    semitones: (follower.first().map(|n| n.pitch as i32).unwrap_or(0)) - s.first().map(|n| n.pitch as i32).unwrap_or(0),
                    label: label.to_string(),
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

// ------------------------------------------------------------------ Gegensatz

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Countersubject {
    pub notes: Vec<VNote>,
    /// Oberhalb des Themas erzeugt.
    pub above: bool,
    pub errors: usize,
    pub warnings: usize,
    /// Mit Oktavtausch (Gegensatz auf der anderen Seite) noch fehlerfrei.
    pub invertible: bool,
    pub inverted_errors: usize,
    pub inverted_warnings: usize,
}

/// Auf die andere Seite des Themas versetzen (Oktaven).
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

/// Gegensatz-Vorschläge zum Thema, die beste zuerst.
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
    /// 2 bis 4 Stimmen.
    pub voices: usize,
    #[serde(default)]
    pub beats_per_bar: u32,
    #[serde(default = "yes")]
    pub countersubject: bool,
    #[serde(default = "yes")]
    pub stretto: bool,
    /// In Moll mit Dur-Schlussakkord.
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
        VoiceDef { name: "Sopran", range: (62, 79), center: 70.0 },
        VoiceDef { name: "Alt", range: (55, 72), center: 64.0 },
        VoiceDef { name: "Tenor", range: (48, 67), center: 57.0 },
        VoiceDef { name: "Bass", range: (38, 58), center: 48.0 },
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
                (2, 0) => "Oberstimme",
                (2, 3) => "Unterstimme",
                _ => d.name,
            };
            VoiceDef { name, range: d.range, center: d.center }
        })
        .collect()
}

/// In die Lage einer Stimme oktavieren.
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

/// Oberhalb bzw. unterhalb einer anderen Stimme in die eigene Lage oktavieren.
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

/// Ruhiger Rhythmus für freie Stimmen: Viertel, Halbe wo die Hauptstimme sehr bewegt ist.
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

    /// Freie Stimme in [a, b) gegen alles, was dort schon steht.
    fn free(&mut self, v: usize, a: f64, b: f64, k: &Key, reference: &[VNote], calm: bool) {
        let others: Vec<Vec<VNote>> = (0..self.voices.len()).filter(|&o| o != v).map(|o| self.window(o, a, b)).collect();
        let refs: Vec<&[VNote]> = others.iter().map(|x| x.as_slice()).collect();
        let slots = if calm { calm_rhythm(reference, a, b) } else { complementary_rhythm(reference, a, b, self.bpb) };
        // Nachbarstimmen (kleinere Nummer = höher) begrenzen die Lage
        let ceiling = if v > 0 { self.window(v - 1, a, b) } else { Vec::new() };
        let floor = if v + 1 < self.voices.len() { self.window(v + 1, a, b) } else { Vec::new() };
        let spec = LineSpec { variant: (a as u32) % 7, ceiling, floor, ..LineSpec::new(self.defs[v].range, slots) };
        let line = generate_line(&refs, k, &spec, self.bpb);
        self.put(v, line);
    }

    /// Einsatz: Material in die Lage der Stimme bringen und ab t setzen.
    fn entry(&mut self, v: usize, material: &[VNote], t: f64) -> Vec<VNote> {
        let fitted = shift(&fit_octave(material, self.defs[v].range, self.defs[v].center), t, 0);
        self.put(v, fitted.clone());
        fitted
    }

    fn finish(mut self) -> Vec<PlannedVoice> {
        for v in &mut self.voices {
            v.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.pitch.cmp(&a.pitch)));
            v.dedup_by(|b, a| (a.start - b.start).abs() < EPS);
            // Überlappungen innerhalb einer Stimme kürzen
            for i in 0..v.len().saturating_sub(1) {
                let next = v[i + 1].start;
                if v[i].end() > next + EPS {
                    v[i].len = (next - v[i].start).max(0.05);
                }
            }
            v.retain(|n| n.len >= 0.05);
        }
        let _ = self.key;
        self.defs.iter().zip(self.voices).map(|(d, notes)| PlannedVoice { name: d.name.to_string(), notes }).collect()
    }
}

/// Fugenplan: aus einem Thema ein Gerüst für 2–4 Stimmen.
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
    let names = |kk: &Key| kk.name_de();

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
            let above = prev < v; // kleinere Nummer = höhere Stimme
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
        name: "Exposition".into(),
        start: 0.0,
        end: expo_end,
        key: names(k),
        description: format!(
            "{} Einsätze: Thema und {} Antwort im Wechsel{}",
            n,
            if ans.kind == AnswerKind::Tonal { "tonale" } else { "reale" },
            if cs.is_some() { ", dazu der Gegensatz in der zuvor eingetretenen Stimme" } else { "" }
        ),
    });

    // Zwischenspiel: Sequenz des Themenkopfs oben, Quintfall im Bass, Mitte frei
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
        // Quintfall-Grundtöne im Bass, je halber Takt, zuletzt die Dominante der Zieltonart
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
            name: format!("Zwischenspiel {nr}"),
            start: t0,
            end: t0 + dur,
            key: format!("→ {}", target.name_de()),
            description: "Sequenz des Themenkopfs (stufenweise abwärts) über Quintfall im Bass".into(),
        });
        t0 + dur
    };

    // Durchführung: Thema in einer Stimme, Gegensatz in einer anderen, Rest frei
    let middle_entry = |b: &mut Builder, t0: f64, material: &[VNote], cs_mat: Option<Vec<VNote>>, v: usize, kk: &Key, sections: &mut Vec<Section>, name: &str| -> f64 {
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
        sections.push(Section {
            name: name.to_string(),
            start: t0,
            end: t0 + entry_gap,
            key: kk.name_de(),
            description: format!("Thema in {}{}", b.defs[v].name, if handled.len() > 1 { ", Gegensatz dazu" } else { "" }),
        });
        t0 + entry_gap
    };

    let mut t = expo_end;
    t = episode(&mut b, t, &rel, &mut sections, 1);
    // Paralleltonart: diatonisch zwei Stufen (Dur → Moll abwärts, Moll → Dur aufwärts)
    let steps = if k.minor { 2 } else { -2 };
    let mut rel_subject = transpose_diatonic(&s, k, steps);
    fix_leading_tones(&mut rel_subject, &rel);
    let rel_cs = cs.as_ref().map(|c| {
        let mut v = transpose_diatonic(&c.notes, k, steps);
        fix_leading_tones(&mut v, &rel);
        v
    });
    let v1 = if n >= 3 { n - 2 } else { 1 };
    t = middle_entry(&mut b, t, &rel_subject, rel_cs, v1, &rel, &mut sections, "Durchführung (Paralleltonart)");
    t = episode(&mut b, t, &sub, &mut sections, 2);
    let sub_subject = shift(&s, 0.0, 5);
    let sub_cs = cs.as_ref().map(|c| shift(&c.notes, 0.0, 5));
    t = middle_entry(&mut b, t, &sub_subject, sub_cs, 0, &sub, &mut sections, "Durchführung (Subdominante)");
    t = episode(&mut b, t, k, &mut sections, 3);

    // ---------------- Engführung (oder Schlussdurchführung)
    let st = if opt.stretto { strettos(&s, k, bpb).into_iter().next() } else { None };
    let stretto_name = st.as_ref().map(|x| format!("{}, Abstand {} Schl.", x.label, x.delay));
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
            // Lücke der folgenden Stimme vor ihrem Einsatz füllen
            b.free(follow_v, t, t + x.delay, k, &lead, true);
            sections.push(Section {
                name: "Engführung".into(),
                start: t,
                end,
                key: k.name_de(),
                description: format!("Thema in {} und – {} – in {}", b.defs[lead_v].name, x.label, b.defs[follow_v].name),
            });
            t = end;
        }
        None => {
            t = middle_entry(&mut b, t, &s, cs.as_ref().map(|c| c.notes.clone()), n - 1, k, &mut sections, "Schlussdurchführung");
        }
    }

    // ---------------- Orgelpunkt auf der Dominante
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
    sections.push(Section { name: "Orgelpunkt".into(), start: t, end: t + pedal_len, key: k.name_de(), description: "Bass hält die Dominante, oben der Themenkopf".into() });
    t += pedal_len;

    // ---------------- Schlusskadenz
    let tonic_id = if k.minor && opt.picardy { "I" } else if k.minor { "i" } else { "I" };
    let six_four = if k.minor { "i64" } else { "I64" };
    let chords: Vec<_> = [six_four, "V7", tonic_id].iter().filter_map(|id| parse_roman(id, k)).collect();
    let voicing = voice_progression(&chords, Some(k), None);
    let durs = [bar / 2.0, bar / 2.0, bar];
    let mut tt = t;
    for (ci, v4) in voicing.iter().enumerate() {
        // v4 = [Bass, Tenor, Alt, Sopran]
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
        name: "Schlusskadenz".into(),
        start: t,
        end: tt,
        key: k.name_de(),
        description: if k.minor && opt.picardy { "Quartsextvorhalt – D⁷ – Tonika mit Picardischer Terz".into() } else { "Quartsextvorhalt – D⁷ – Tonika".into() },
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

    /// Thema im Stil eines Fugenthemas in C-Dur (2 Takte).
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
        // beginnt mit g–c: tonal, g wird mit c beantwortet
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
        // in Moll: Leitton vor der Tonika
        let am = key(9, true);
        let m = line(&[(64, 1.0), (62, 1.0), (60, 1.0), (59, 1.0)]); // e d c h → um 1 Stufe tiefer: d c h a?
        let down = transform(&m, &am, Transform::Versetzung(-1));
        assert_eq!(down.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![62, 60, 59, 57]);
        // e g a: g steigt stufenweise zur Tonika a → Leitton gis (melodisch Moll aufwärts)
        let up = line(&[(64, 1.0), (67, 1.0), (69, 1.0)]);
        assert_eq!(transform(&up, &am, Transform::Versetzung(0))[1].pitch, 68);
        // abwärts bleibt g
        let down2 = line(&[(69, 1.0), (67, 1.0), (65, 1.0)]);
        assert_eq!(transform(&down2, &am, Transform::Versetzung(0))[1].pitch, 67);
    }

    #[test]
    fn finds_a_canonic_stretto() {
        // Bruder Jakob ist ein Kanon im Abstand von zwei Takten
        let s = line(&[
            (60, 1.0), (62, 1.0), (64, 1.0), (60, 1.0), (60, 1.0), (62, 1.0), (64, 1.0), (60, 1.0),
            (64, 1.0), (65, 1.0), (67, 2.0), (64, 1.0), (65, 1.0), (67, 2.0),
        ]);
        let st = strettos(&s, &c(), 4);
        assert!(st.iter().any(|x| x.delay == 8.0 && x.label.starts_with("Oktave")), "{:?}", st.iter().map(|x| (x.delay, &x.label)).collect::<Vec<_>>());
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
            assert!(names.contains(&"Exposition") && names.contains(&"Orgelpunkt") && names.contains(&"Schlusskadenz"), "{names:?}");
            assert!(plan.sections.windows(2).all(|w| (w[0].end - w[1].start).abs() < 1e-6), "lückenlos");
            assert!((plan.length - plan.sections.last().unwrap().end).abs() < 1e-6);
            // Satzfehler zählen (Gerüst: wenige erlaubt)
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
