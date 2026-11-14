//! Counterpoint: check voices for part-writing errors and generate counter-voices.
//!
//! **Checking** ([`check`]): parallel fifths and octaves, hidden parallels
//! between the outer voices, voice crossing, dissonance treatment (on strong beats
//! only prepared and resolved downward, on weak beats stepwise),
//! voice range, large and augmented leaps, wide spacing.
//!
//! **Generating** ([`generate_line`]): a new voice against existing ones –
//! dynamic programming over pitches with costs as in
//! counterpoint teaching (consonance on strong beats, passing tones on
//! weak ones, contrary motion, steps before leaps, no parallels). With
//! `invertible` fifths are avoided so that the voice stays usable in
//! double counterpoint at the octave (swapped).

use crate::theory::Key;
use klang_i18n::tr;
use serde::{Deserialize, Serialize};

/// Note of a voice; times in beats.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VNote {
    pub pitch: u8,
    pub start: f64,
    pub len: f64,
}

impl VNote {
    pub fn new(pitch: u8, start: f64, len: f64) -> Self {
        Self { pitch, start, len }
    }
    pub fn end(&self) -> f64 {
        self.start + self.len
    }
}

const EPS: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Fehler,
    Warnung,
    Hinweis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IssueKind {
    Quintparallele,
    Oktavparallele,
    VerdeckteParallele,
    Stimmkreuzung,
    Dissonanz,
    Umfang,
    Sprung,
    Lage,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub kind: IssueKind,
    pub severity: Severity,
    pub start: f64,
    pub end: f64,
    /// Voices involved (indices as passed).
    pub voices: Vec<usize>,
    pub message: String,
}

/// Usual range (MIDI) of a voice – by name, otherwise by position in the texture.
pub fn voice_range(name: &str, rank: usize, count: usize) -> (u8, u8) {
    let n = name.to_lowercase();
    if n.contains("sopran") || n.contains("ober") || n.contains("upper") {
        return (60, 81);
    }
    if n.contains("alt") {
        return (53, 74);
    }
    if n.contains("tenor") || n.contains("ténor") {
        return (48, 69);
    }
    if n.contains("bass") || n.contains("unter") || n.contains("lower") || n.contains("pedal") {
        return (36, 62);
    }
    match (count, rank) {
        (1, _) => (36, 84),
        (2, 0) => (55, 84),
        (2, _) => (36, 67),
        (3, 0) => (60, 81),
        (3, 1) => (50, 74),
        (3, _) => (36, 62),
        (_, 0) => (60, 81),
        (_, 1) => (53, 74),
        (_, 2) => (48, 69),
        _ => (36, 62),
    }
}

/// Rank by voice name (0 = top), None = unknown.
fn name_rank(name: &str) -> Option<u8> {
    let n = name.to_lowercase();
    if n.contains("sopran") || n.contains("ober") || n.contains("upper") {
        Some(0)
    } else if n.contains("alt") {
        Some(1)
    } else if n.contains("tenor") || n.contains("ténor") {
        Some(2)
    } else if n.contains("bass") || n.contains("unter") || n.contains("lower") || n.contains("pedal") {
        Some(3)
    } else {
        None
    }
}

const NAMES: [&str; 12] = ["c", "cis", "d", "es", "e", "f", "fis", "g", "gis", "a", "b", "h"];
/// Note name for messages ("cis4" in German, "c♯4" in English).
fn pname(p: u8) -> String {
    crate::theory::display_name(&format!("{}{}", NAMES[p as usize % 12], p as i32 / 12 - 1))
}

/// Sounding note of a voice (sorted by onset) at time t.
fn sounding(v: &[VNote], t: f64) -> Option<VNote> {
    let i = v.partition_point(|n| n.start <= t + EPS);
    v[..i].iter().rev().take(4).find(|n| n.end() > t + EPS).copied()
}

fn next_after(v: &[VNote], n: &VNote) -> Option<VNote> {
    let i = v.partition_point(|x| x.start <= n.start + EPS);
    v.get(i).copied().filter(|x| x.start < n.end() + 0.75)
}

fn prev_before(v: &[VNote], n: &VNote) -> Option<VNote> {
    let i = v.partition_point(|x| x.start < n.start - EPS);
    i.checked_sub(1).map(|j| v[j]).filter(|x| x.end() > n.start - 0.75)
}

fn is_strong(t: f64, bpb: u32) -> bool {
    let bpb = bpb.max(1) as f64;
    let m = t.rem_euclid(bpb);
    m < EPS || (bpb - m) < EPS || (bpb >= 4.0 && (bpb as u32) % 2 == 0 && (m - bpb / 2.0).abs() < EPS)
}

fn ic(a: u8, b: u8) -> u8 {
    ((a as i32 - b as i32).unsigned_abs() % 12) as u8
}

/// Dissonant between two voices (fourth only against the bass).
fn dissonant(interval_class: u8, against_bass: bool) -> bool {
    matches!(interval_class, 1 | 2 | 6 | 10 | 11) || (interval_class == 5 && against_bass)
}

fn sorted(v: &[VNote]) -> Vec<VNote> {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.pitch.cmp(&a.pitch)));
    // per onset only the topmost note (one voice = one line)
    s.dedup_by(|b, a| (a.start - b.start).abs() < EPS);
    s
}

/// Check voices. `voices` in any order (the position is determined from the
/// mean pitch), `names` for range and messages.
pub fn check(voices: &[Vec<VNote>], names: &[String], key: Option<&Key>, beats_per_bar: u32) -> Vec<Issue> {
    let v: Vec<Vec<VNote>> = voices.iter().map(|x| sorted(x)).collect();
    let n = v.len();
    let mean = |x: &[VNote]| if x.is_empty() { 0.0 } else { x.iter().map(|n| n.pitch as f64).sum::<f64>() / x.len() as f64 };
    // Order from top to bottom: by voice names if all are named, otherwise by position
    let mut order: Vec<usize> = (0..n).filter(|&i| !v[i].is_empty()).collect();
    let by_name: Vec<Option<u8>> = (0..n).map(|i| names.get(i).and_then(|s| name_rank(s))).collect();
    if order.iter().all(|&i| by_name[i].is_some()) {
        order.sort_by(|&a, &b| by_name[a].cmp(&by_name[b]).then(mean(&v[b]).total_cmp(&mean(&v[a]))));
    } else {
        order.sort_by(|&a, &b| mean(&v[b]).total_cmp(&mean(&v[a])));
    }
    let rank = |i: usize| order.iter().position(|&x| x == i).unwrap_or(0);
    let label = |i: usize| names.get(i).cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| tr!("Voice {n}", n = i + 1));
    let mut out: Vec<Issue> = Vec::new();
    let push = |out: &mut Vec<Issue>, kind, severity, start: f64, end: f64, voices: Vec<usize>, message: String| {
        // merge the same message directly following
        if let Some(last) = out.iter_mut().rev().take(12).find(|x| x.kind == kind && x.voices == voices && (x.end - start).abs() < 1.0 + EPS && x.message == message) {
            last.end = last.end.max(end);
            return;
        }
        out.push(Issue { kind, severity, start, end, voices, message });
    };

    let mut events: Vec<f64> = v.iter().flatten().map(|n| n.start).collect();
    events.sort_by(f64::total_cmp);
    events.dedup_by(|b, a| (*a - *b).abs() < EPS);

    for w in 0..events.len() {
        let t = events[w];
        // Position of all voices at this point in time
        let now: Vec<Option<VNote>> = (0..n).map(|i| sounding(&v[i], t)).collect();
        let lowest = order.iter().copied().filter(|&i| now[i].is_some()).last();
        for (ai, &a) in order.iter().enumerate() {
            for &b in &order[ai + 1..] {
                let (Some(na), Some(nb)) = (now[a], now[b]) else { continue };
                // Voice crossing
                if na.pitch < nb.pitch {
                    push(&mut out, IssueKind::Stimmkreuzung, Severity::Warnung, t, na.end().min(nb.end()), vec![a, b],
                        tr!("{a} is below {b} ({pa} < {pb})", a = label(a), b = label(b), pa = pname(na.pitch), pb = pname(nb.pitch)));
                }
                // Parallels: comparison with the previous event
                if w > 0 {
                    let tp = events[w - 1];
                    if let (Some(pa), Some(pb)) = (sounding(&v[a], tp), sounding(&v[b], tp)) {
                        let moved_a = (pa.start - na.start).abs() > EPS && pa.pitch != na.pitch;
                        let moved_b = (pb.start - nb.start).abs() > EPS && pb.pitch != nb.pitch;
                        let ma = na.pitch as i32 - pa.pitch as i32;
                        let mb = nb.pitch as i32 - pb.pitch as i32;
                        let (i0, i1) = (pa.pitch as i32 - pb.pitch as i32, na.pitch as i32 - nb.pitch as i32);
                        if moved_a && moved_b && ma.signum() == mb.signum() {
                            if i0.rem_euclid(12) == 7 && i1.rem_euclid(12) == 7 && i0.abs() >= 7 && i1.abs() >= 7 {
                                push(&mut out, IssueKind::Quintparallele, Severity::Fehler, tp, na.end().min(nb.end()), vec![a, b],
                                    tr!("Parallel fifths {a} / {b}: {a1}–{b1} → {a2}–{b2}", a = label(a), b = label(b), a1 = pname(pa.pitch), b1 = pname(pb.pitch), a2 = pname(na.pitch), b2 = pname(nb.pitch)));
                            } else if i0.rem_euclid(12) == 0 && i1.rem_euclid(12) == 0 {
                                push(&mut out, IssueKind::Oktavparallele, Severity::Fehler, tp, na.end().min(nb.end()), vec![a, b],
                                    if i1 == 0 {
                                        tr!("Parallel unisons {a} / {b}: {a1}–{b1} → {a2}–{b2}", a = label(a), b = label(b), a1 = pname(pa.pitch), b1 = pname(pb.pitch), a2 = pname(na.pitch), b2 = pname(nb.pitch))
                                    } else {
                                        tr!("Parallel octaves {a} / {b}: {a1}–{b1} → {a2}–{b2}", a = label(a), b = label(b), a1 = pname(pa.pitch), b1 = pname(pb.pitch), a2 = pname(na.pitch), b2 = pname(nb.pitch))
                                    });
                            } else if ai == 0
                                && Some(b) == order.last().copied()
                                && matches!(i1.rem_euclid(12), 0 | 7)
                                && ma.abs() > 2
                                && i0.rem_euclid(12) != i1.rem_euclid(12)
                            {
                                let message = if i1.rem_euclid(12) == 0 {
                                    tr!("Hidden octave in the outer voices (leap in {voice} into the octave)", voice = label(a))
                                } else {
                                    tr!("Hidden fifth in the outer voices (leap in {voice} into the fifth)", voice = label(a))
                                };
                                push(&mut out, IssueKind::VerdeckteParallele, Severity::Warnung, tp, na.end().min(nb.end()), vec![a, b], message);
                            }
                        }
                    }
                }
                // Dissonances: only where one of the two notes starts here
                let starts_a = (na.start - t).abs() < EPS;
                let starts_b = (nb.start - t).abs() < EPS;
                if !(starts_a || starts_b) {
                    continue;
                }
                let interval = ic(na.pitch, nb.pitch);
                if !dissonant(interval, Some(b) == lowest) {
                    continue;
                }
                let iname = klang_i18n::translate(match interval {
                    1 => klang_i18n::tk!("minor second/ninth"),
                    2 => klang_i18n::tk!("major second/ninth"),
                    5 => klang_i18n::tk!("fourth against the bass"),
                    6 => klang_i18n::tk!("tritone"),
                    10 => klang_i18n::tk!("minor seventh"),
                    _ => klang_i18n::tk!("major seventh"),
                });
                if is_strong(t, beats_per_bar) {
                    // prepared: a note was already sounding (suspension) and resolves stepwise downward
                    let held = if !starts_a { Some((a, na)) } else if !starts_b { Some((b, nb)) } else { None };
                    let resolved = held.is_some_and(|(i, hn)| next_after(&v[i], &hn).is_some_and(|nx| (1..=2).contains(&(hn.pitch as i32 - nx.pitch as i32))));
                    // also: re-struck suspension (same pitch as before)
                    let restruck = [(a, na, starts_a), (b, nb, starts_b)].iter().any(|&(i, x, s)| {
                        s && prev_before(&v[i], &x).is_some_and(|p| p.pitch == x.pitch) && next_after(&v[i], &x).is_some_and(|nx| (1..=2).contains(&(x.pitch as i32 - nx.pitch as i32)))
                    });
                    if !(resolved || restruck) {
                        push(&mut out, IssueKind::Dissonanz, Severity::Warnung, t, na.end().min(nb.end()), vec![a, b],
                            tr!("Unprepared dissonance on a strong beat ({iname}) between {a} and {b}", iname = iname, a = label(a), b = label(b)));
                    }
                } else {
                    // weak beat: passing tone/neighbor tone – approached or left stepwise
                    for (i, x, s) in [(a, na, starts_a), (b, nb, starts_b)] {
                        if !s {
                            continue;
                        }
                        let step = |y: Option<VNote>| y.is_some_and(|y| (1..=2).contains(&(y.pitch as i32 - x.pitch as i32).abs()));
                        if !step(prev_before(&v[i], &x)) && !step(next_after(&v[i], &x)) {
                            push(&mut out, IssueKind::Dissonanz, Severity::Hinweis, t, x.end(), vec![a, b],
                                tr!("Free dissonance ({iname}) in {voice}: neither approached nor left by step", iname = iname, voice = label(i)));
                        }
                    }
                }
            }
        }
        // wide spacing between adjacent upper voices
        for pair in order.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if Some(b) == order.last().copied() {
                continue;
            }
            if let (Some(na), Some(nb)) = (now[a], now[b]) {
                if na.pitch as i32 - nb.pitch as i32 > 12 && ((na.start - t).abs() < EPS || (nb.start - t).abs() < EPS) {
                    push(&mut out, IssueKind::Lage, Severity::Hinweis, t, na.end().min(nb.end()), vec![a, b],
                        tr!("More than an octave between {a} and {b}", a = label(a), b = label(b)));
                }
            }
        }
    }

    // per voice: range and leaps
    for i in 0..n {
        // range by the name passed in (not translated)
        let (lo, hi) = voice_range(names.get(i).map(String::as_str).unwrap_or(""), rank(i), order.len());
        for (k, x) in v[i].iter().enumerate() {
            if x.pitch + 2 < lo || x.pitch > hi + 2 {
                let sev = if x.pitch + 5 < lo || x.pitch > hi + 5 { Severity::Warnung } else { Severity::Hinweis };
                push(&mut out, IssueKind::Umfang, sev, x.start, x.end(), vec![i],
                    tr!("{voice} {pitch} outside the range ({lo}–{hi})", voice = label(i), pitch = pname(x.pitch), lo = pname(lo), hi = pname(hi)));
            }
            if k == 0 {
                continue;
            }
            let p = v[i][k - 1];
            if x.start - p.end() > 0.5 {
                continue; // after a rest the leap does not count
            }
            let d = (x.pitch as i32 - p.pitch as i32).abs();
            let aug2 = key.is_some_and(|kk| {
                kk.minor && {
                    let (o1, o2) = ((p.pitch + 12 - kk.tonic % 12) % 12, (x.pitch + 12 - kk.tonic % 12) % 12);
                    (o1 == 8 && o2 == 11) || (o1 == 11 && o2 == 8)
                }
            });
            if d > 12 {
                push(&mut out, IssueKind::Sprung, Severity::Warnung, p.start, x.end(), vec![i], tr!("Leap of more than an octave in {voice} ({from} → {to})", voice = label(i), from = pname(p.pitch), to = pname(x.pitch)));
            } else if d == 6 {
                push(&mut out, IssueKind::Sprung, Severity::Hinweis, p.start, x.end(), vec![i], tr!("Tritone leap in {voice} ({from} → {to})", voice = label(i), from = pname(p.pitch), to = pname(x.pitch)));
            } else if aug2 && d == 3 {
                push(&mut out, IssueKind::Sprung, Severity::Hinweis, p.start, x.end(), vec![i], tr!("Augmented second in {voice} ({from} → {to})", voice = label(i), from = pname(p.pitch), to = pname(x.pitch)));
            }
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.severity.cmp(&b.severity)));
    out
}

/// Counts errors and warnings (for ratings).
pub fn tally(issues: &[Issue]) -> (usize, usize) {
    (issues.iter().filter(|i| i.severity == Severity::Fehler).count(), issues.iter().filter(|i| i.severity == Severity::Warnung).count())
}

// ------------------------------------------------------------------ Rhythm

/// Rhythm of a counter-voice: moves where the reference voice holds, and
/// holds where it moves. Returns (onset, duration) in the range [from, to).
pub fn complementary_rhythm(reference: &[VNote], from: f64, to: f64, beats_per_bar: u32) -> Vec<(f64, f64)> {
    let mut slots: Vec<(f64, f64)> = Vec::new();
    let mut t = from;
    let bpb = beats_per_bar.max(1) as f64;
    while t < to - EPS {
        let beat_end = (t.floor() + 1.0).min(to);
        let onsets = reference.iter().filter(|n| n.start >= t - EPS && n.start < beat_end - EPS).count();
        let busy_next = {
            let ne = (beat_end + 1.0).min(to);
            reference.iter().filter(|n| n.start >= beat_end - EPS && n.start < ne - EPS).count() >= 2
        };
        let len = beat_end - t;
        if onsets >= 2 && busy_next && (t.rem_euclid(2.0) < EPS) && beat_end + 1.0 <= to + EPS && ((t + 2.0) % bpb > EPS || bpb <= 2.0) {
            slots.push((t, 2.0)); // both beats busy: half note
            t += 2.0;
            continue;
        }
        if onsets == 0 && len >= 1.0 - EPS {
            slots.push((t, 0.5)); // reference voice holds: eighth
            slots.push((t + 0.5, 0.5));
        } else {
            slots.push((t, len));
        }
        t = beat_end;
    }
    slots
}

// ------------------------------------------------------------------ Generating

#[derive(Clone, Debug)]
pub struct LineSpec {
    pub range: (u8, u8),
    pub slots: Vec<(f64, f64)>,
    /// Stay above (true) or below the reference voices; None = don't care.
    pub above: Option<bool>,
    /// Avoid fifths (double counterpoint at the octave).
    pub invertible: bool,
    /// Variant (slight, reproducible deviations for other suggestions).
    pub variant: u32,
    /// Neighboring voice above (not to exceed) and below (not to go under).
    pub ceiling: Vec<VNote>,
    pub floor: Vec<VNote>,
}

impl LineSpec {
    pub fn new(range: (u8, u8), slots: Vec<(f64, f64)>) -> Self {
        Self { range, slots, above: None, invertible: false, variant: 0, ceiling: Vec::new(), floor: Vec::new() }
    }
}

fn scale_set(key: &Key) -> Vec<u8> {
    let steps: &[u8] = if key.minor { &[0, 2, 3, 5, 7, 8, 9, 10, 11] } else { &[0, 2, 4, 5, 7, 9, 11] };
    steps.iter().map(|s| (key.tonic + s) % 12).collect()
}

fn jitter(variant: u32, slot: usize, pitch: u8) -> f32 {
    if variant == 0 {
        return 0.0;
    }
    let mut z = (variant as u64) << 40 ^ (slot as u64) << 16 ^ pitch as u64;
    z = z.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z ^= z >> 29;
    (z % 1000) as f32 / 1000.0 * 2.2
}

/// New voice against `against` (other voices) in the rhythm slots of `spec`.
pub fn generate_line(against: &[&[VNote]], key: &Key, spec: &LineSpec, beats_per_bar: u32) -> Vec<VNote> {
    let others: Vec<Vec<VNote>> = against.iter().map(|v| sorted(v)).collect();
    let (ceiling, floor) = (sorted(&spec.ceiling), sorted(&spec.floor));
    let set = scale_set(key);
    let cands: Vec<u8> = (spec.range.0..=spec.range.1).filter(|p| set.contains(&(p % 12))).collect();
    let s_n = spec.slots.len();
    let c_n = cands.len();
    if s_n == 0 || c_n == 0 {
        return Vec::new();
    }
    let lowest_other = |t: f64| others.iter().filter_map(|v| sounding(v, t)).map(|n| n.pitch).min();
    let highest_other = |t: f64| others.iter().filter_map(|v| sounding(v, t)).map(|n| n.pitch).max();

    // vertical costs per slot and candidate (+ whether dissonant)
    let mut vert = vec![vec![0.0f32; c_n]; s_n];
    let mut diss = vec![vec![false; c_n]; s_n];
    for (s, &(t, len)) in spec.slots.iter().enumerate() {
        let strong = is_strong(t, beats_per_bar);
        // points in time within the slot at which other voices change
        let mut times = vec![t];
        for v in &others {
            for n in v.iter().filter(|n| n.start > t + EPS && n.start < t + len - EPS) {
                times.push(n.start);
            }
        }
        for (c, &p) in cands.iter().enumerate() {
            let mut cost = 0.0f32;
            let mut is_diss = false;
            for (k, &tt) in times.iter().enumerate() {
                let w = if k == 0 { 1.0 } else { 0.35 };
                let low = lowest_other(tt);
                for v in &others {
                    let Some(o) = sounding(v, tt) else { continue };
                    let against_bass = low.is_some_and(|l| l == o.pitch && o.pitch < p) || low.is_some_and(|l| p < l);
                    let i = ic(p, o.pitch);
                    if p == o.pitch {
                        cost += 6.0 * w; // unison with another voice
                    }
                    if dissonant(i, against_bass || others.len() == 1) {
                        if k == 0 {
                            is_diss = true;
                        }
                        cost += if strong && k == 0 { 30.0 } else { 3.0 } * w;
                    } else {
                        cost += match i {
                            3 | 4 | 8 | 9 => 0.0,
                            0 => 2.5,
                            7 => if spec.invertible { 7.0 } else { 1.5 },
                            _ => 1.0,
                        } * w;
                    }
                }
            }
            // stay above/below
            match spec.above {
                Some(true) => {
                    if let Some(h) = highest_other(t) {
                        if p <= h {
                            cost += 25.0;
                        }
                    }
                }
                Some(false) => {
                    if let Some(l) = lowest_other(t) {
                        if p >= l {
                            cost += 25.0;
                        }
                    }
                }
                None => {}
            }
            // not above the voice above, not below the one below
            for tt in [t, t + len * 0.5] {
                if sounding(&ceiling, tt).is_some_and(|o| p >= o.pitch) {
                    cost += 25.0;
                }
                if sounding(&floor, tt).is_some_and(|o| p <= o.pitch) {
                    cost += 25.0;
                }
            }
            // prefer the middle of the range
            let mid = (spec.range.0 as f32 + spec.range.1 as f32) / 2.0;
            cost += (p as f32 - mid).abs() * 0.06 + jitter(spec.variant, s, p);
            vert[s][c] = cost;
            diss[s][c] = is_diss;
        }
    }

    let horiz = |a: u8, b: u8| -> f32 {
        let d = (b as i32 - a as i32).abs();
        let mut c = match d {
            0 => 2.0,
            1 | 2 => 0.0,
            3 | 4 => 0.8,
            5 | 7 => 1.6,
            6 => 14.0,
            8 | 9 => 3.0,
            12 => 2.5,
            10 | 11 => 8.0,
            _ => 20.0,
        };
        if key.minor {
            let (o1, o2) = ((a + 12 - key.tonic % 12) % 12, (b + 12 - key.tonic % 12) % 12);
            if (o1 == 8 && o2 == 11) || (o1 == 11 && o2 == 8) || (o1 == 9 && o2 == 8) || (o1 == 10 && o2 == 11) {
                c += 12.0; // augmented second, cross-relation
            }
        }
        c
    };
    // Parallels between slot s-1 and s against other voices
    let parallel = |s: usize, a: u8, b: u8| -> f32 {
        let t1 = spec.slots[s].0;
        let mut c = 0.0;
        for v in &others {
            // like the checker: note immediately before the change against the one after
            let (Some(o0), Some(o1)) = (sounding(v, t1 - 0.01), sounding(v, t1)) else { continue };
            let (mo, ml) = (o1.pitch as i32 - o0.pitch as i32, b as i32 - a as i32);
            if mo == 0 || ml == 0 {
                continue;
            }
            let (i0, i1) = (ic(a, o0.pitch), ic(b, o1.pitch));
            if mo.signum() == ml.signum() {
                if i0 == i1 && (i0 == 0 || i0 == 7) {
                    c += 60.0;
                } else if i1 == 0 || i1 == 7 {
                    c += 2.0; // hidden
                }
            } else {
                c -= 0.4; // contrary motion
            }
        }
        c
    };

    // DP over (previous, current) pitch
    let none = c_n; // "no predecessor"
    let w = c_n + 1;
    let inf = f32::INFINITY;
    let mut cost = vec![inf; w * c_n];
    let mut back: Vec<Vec<u32>> = vec![vec![u32::MAX; w * c_n]; s_n];
    for c in 0..c_n {
        cost[none * c_n + c] = vert[0][c];
    }
    for s in 1..s_n {
        let mut next = vec![inf; w * c_n];
        for pp in 0..w {
            for p in 0..c_n {
                let base = cost[pp * c_n + p];
                if !base.is_finite() {
                    continue;
                }
                for q in 0..c_n {
                    let (a, b) = (cands[p], cands[q]);
                    let mut c = base + horiz(a, b) + vert[s][q] + parallel(s, a, b);
                    // Dissonance in the previous slot: on a weak beat stepwise in and out,
                    // on a strong beat only as a suspension (same note before, step down after)
                    if diss[s - 1][p] {
                        let step_in = pp != none && (1..=2).contains(&(a as i32 - cands[pp] as i32).abs());
                        let step_out = (1..=2).contains(&(b as i32 - a as i32).abs());
                        let strong = is_strong(spec.slots[s - 1].0, beats_per_bar);
                        if strong {
                            let prepared = pp != none && cands[pp] == a && (1..=2).contains(&(a as i32 - b as i32));
                            if prepared {
                                c -= 26.0;
                            }
                        } else if !(step_in && step_out) {
                            c += 9.0;
                        }
                    }
                    // after a leap, a step in the opposite direction
                    if pp != none {
                        let l1 = a as i32 - cands[pp] as i32;
                        let l2 = b as i32 - a as i32;
                        if l1.abs() >= 5 && (l2.signum() == l1.signum() || l2.abs() > 2) {
                            c += 2.5;
                        }
                    }
                    let idx = p * c_n + q;
                    if c < next[idx] {
                        next[idx] = c;
                        back[s][idx] = (pp * c_n + p) as u32;
                    }
                }
            }
        }
        cost = next;
    }
    // Ending: prefer a triad note of the tonic
    let tonic_triad = [key.tonic % 12, (key.tonic + if key.minor { 3 } else { 4 }) % 12, (key.tonic + 7) % 12];
    let (mut best, mut best_c) = (usize::MAX, inf);
    for (idx, &c) in cost.iter().enumerate() {
        let q = idx % c_n;
        let c = c + if tonic_triad.contains(&(cands[q] % 12)) { 0.0 } else { 1.5 };
        if c < best_c {
            best_c = c;
            best = idx;
        }
    }
    if best == usize::MAX {
        return Vec::new();
    }
    let mut pitches = vec![0u8; s_n];
    let mut idx = best;
    for s in (0..s_n).rev() {
        pitches[s] = cands[idx % c_n];
        if s > 0 {
            idx = back[s][idx] as usize;
        }
    }
    // tie consecutive equal pitches into longer notes (only within a beat)
    let mut out: Vec<VNote> = Vec::with_capacity(s_n);
    for (s, &(t, len)) in spec.slots.iter().enumerate() {
        if let Some(last) = out.last_mut() {
            if last.pitch == pitches[s] && (last.end() - t).abs() < EPS && t.fract() > EPS {
                last.len += len;
                continue;
            }
        }
        out.push(VNote::new(pitches[s], t, len));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(notes: &[(u8, f64)], start: f64) -> Vec<VNote> {
        let mut t = start;
        notes
            .iter()
            .map(|&(p, d)| {
                let n = VNote::new(p, t, d);
                t += d;
                n
            })
            .collect()
    }

    fn c_dur() -> Key {
        Key { tonic: 0, minor: false, confidence: 1.0 }
    }

    #[test]
    fn finds_parallels_crossing_and_dissonance() {
        // Soprano c'' d'' e'' / bass f g a: parallel fifths (f-c, g-d), then a third
        let s = line(&[(72, 1.0), (74, 1.0), (76, 1.0), (77, 1.0)], 0.0);
        let b = line(&[(53, 1.0), (55, 1.0), (60, 1.0), (50, 1.0)], 0.0);
        let names = vec!["Sopran".to_string(), "Bass".to_string()];
        let issues = check(&[s.clone(), b], &names, Some(&c_dur()), 4);
        assert!(issues.iter().any(|i| i.kind == IssueKind::Quintparallele && i.start == 0.0), "{issues:#?}");
        // Parallel octave
        let b2 = line(&[(60, 1.0), (62, 1.0), (52, 1.0)], 0.0);
        let issues = check(&[s.clone(), b2], &names, Some(&c_dur()), 4);
        assert!(issues.iter().any(|i| i.kind == IssueKind::Oktavparallele));
        // Contrary motion into the octave is allowed
        let s3 = line(&[(67, 1.0), (72, 1.0)], 0.0);
        let b3 = line(&[(55, 1.0), (48, 1.0)], 0.0);
        let issues = check(&[s3, b3], &names, None, 4);
        assert!(!issues.iter().any(|i| i.severity == Severity::Fehler), "{issues:#?}");
        // Voice crossing
        let a = line(&[(60, 2.0)], 0.0);
        let t = line(&[(64, 2.0)], 0.0);
        let names3 = vec!["Alt".to_string(), "Tenor".to_string()];
        let mut sop = line(&[(72, 4.0)], 0.0);
        sop.extend(line(&[(71, 1.0)], 4.0));
        let issues = check(&[sop, a, t], &["Sopran".into(), names3[0].clone(), names3[1].clone()], None, 4);
        assert!(issues.iter().any(|i| i.kind == IssueKind::Stimmkreuzung || i.kind == IssueKind::Umfang), "{issues:#?}");
    }

    #[test]
    fn suspension_is_fine_but_free_dissonance_is_flagged() {
        let names = vec!["Oben".to_string(), "Unten".to_string()];
        // Suspension 4-3: c'' sustains, bass changes to g (fourth), c'' → h' (resolution)
        let top = line(&[(72, 2.0), (72, 2.0), (71, 2.0)], 0.0);
        let mut top_tied = vec![VNote::new(72, 0.0, 4.0)];
        top_tied.push(VNote::new(71, 4.0, 2.0));
        let bass = line(&[(57, 2.0), (55, 4.0)], 0.0);
        let issues = check(&[top_tied, bass.clone()], &names, None, 4);
        assert!(!issues.iter().any(|i| i.kind == IssueKind::Dissonanz && i.severity == Severity::Warnung), "{issues:#?}");
        let _ = top;
        // same dissonance freely struck on a strong beat
        let top2 = line(&[(67, 2.0), (72, 2.0), (71, 2.0)], 0.0);
        let issues = check(&[top2, bass], &names, None, 4);
        assert!(issues.iter().any(|i| i.kind == IssueKind::Dissonanz && i.severity == Severity::Warnung), "{issues:#?}");
    }

    #[test]
    fn generated_line_is_clean_against_a_subject() {
        // Subject (C major), counter-voice above, invertible
        let subject = line(&[(60, 1.0), (64, 0.5), (62, 0.5), (65, 1.0), (64, 1.0), (67, 2.0), (65, 0.5), (64, 0.5), (62, 1.0), (60, 2.0)], 0.0);
        let key = c_dur();
        let slots = complementary_rhythm(&subject, 0.0, 10.0, 4);
        assert!(!slots.is_empty());
        for variant in 0..3 {
            let spec = LineSpec { above: Some(true), invertible: true, variant, ..LineSpec::new((67, 84), slots.clone()) };
            let cs = generate_line(&[&subject], &key, &spec, 4);
            assert!(!cs.is_empty());
            let issues = check(&[cs.clone(), subject.clone()], &["Oben".into(), "Unten".into()], Some(&key), 4);
            let (err, warn) = tally(&issues);
            assert_eq!(err, 0, "Variante {variant}: {issues:#?}\n{cs:?}");
            assert!(warn <= 1, "Variante {variant}: {issues:#?}");
            // rhythm fills the range without gaps
            let total: f64 = cs.iter().map(|n| n.len).sum();
            assert!((total - 10.0).abs() < 1e-6);
        }
    }
}
