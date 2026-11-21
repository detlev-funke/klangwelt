//! How the detected notes were played – beyond pitch and timing:
//!
//! * **Dynamics** – velocity 1–127 from the loudness of the note's body,
//!   relative to the recording: the median note is mf (80), every dB more or
//!   less moves it by 3.4 (pp … ff span about 26 dB). Microphone gain doesn't
//!   matter that way, only what is louder or softer than the rest.
//! * **Accent / sforzato** – an attack that stands out against the notes on
//!   *both* sides (so a sudden change of dynamics isn't one). A sforzato
//!   stands out further, or stands out and falls back right after the attack
//!   (sfp) – more than the instrument's notes usually do.
//! * **Articulation** – staccato: clearly shorter than the distance to the
//!   next note (and short), legato: runs into the next note without the level
//!   dipping in between (the pitch frames smear a short break away, the level
//!   doesn't).
//!
//! The finished analysis and the live analysis use it alike. Converting a
//! recording into notes carries it into velocity, brightness (a harder,
//! brighter attack for accents and sforzati) and length.

use klang_dsp::pitch::PitchFrame;
use klang_dsp::util::{gain_to_db, median};
use serde::Serialize;

/// How a note was struck.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Attack {
    #[default]
    Normal,
    Accent,
    Sforzato,
}

/// How long a note sounds compared with the way to the next one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Articulation {
    #[default]
    Normal,
    Staccato,
    Legato,
}

/// Loudness of one note (dBFS): its body (median after the attack) and the
/// loudest point of the attack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    pub body_db: f32,
    pub attack_db: f32,
}

/// A note as [`expression`] needs it (times in seconds).
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    pub levels: Levels,
    /// Quietest level (dBFS) between this note and the next (`None` for the last).
    pub dip_after_db: Option<f32>,
}

/// The performance of one note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Played {
    pub velocity: u8,
    pub attack: Attack,
    pub articulation: Articulation,
}

/// The attack: from a little before the detected start (pitch locks on a
/// moment after the onset) to this long after it.
const ATTACK_BEFORE_SECS: f64 = 0.03;
const ATTACK_AFTER_SECS: f64 = 0.08;
/// Velocity of the median note and per dB of difference.
const VELOCITY_MF: f32 = 80.0;
const VELOCITY_PER_DB: f32 = 3.4;
const VELOCITY_MIN: f32 = 16.0;
/// Notes on each side an attack is compared with.
const NEIGHBOURS: usize = 4;
/// How far (dB) an attack stands out on both sides for an accent, a sforzato,
/// or a sforzato that falls back (sfp) further than usual after the attack.
const ACCENT_DB: f32 = 4.5;
const SFORZATO_DB: f32 = 9.0;
const SFP_PROMINENCE_DB: f32 = 6.0;
const SFP_DROP_DB: f32 = 5.0;
/// Velocity above the surrounding notes' dynamics.
const ACCENT_BOOST: f32 = 18.0;
const SFORZATO_BOOST: f32 = 32.0;
/// Brightness for the harder attack of accents and sforzati (−1 … +1, as `Note::brightness`).
pub const ACCENT_BRIGHTNESS: f32 = 0.2;
pub const SFORZATO_BRIGHTNESS: f32 = 0.4;
/// Legato: at most this much silence before the next note, and the level
/// between them at most this far below the quieter of the two.
const LEGATO_GAP_SECS: f64 = 0.035;
const LEGATO_DIP_DB: f32 = 10.0;
/// Around the gap between two notes the level is looked at this far into them.
const DIP_MARGIN_SECS: f64 = 0.03;
/// Staccato: at most this share of the way to the next note, at least this
/// much silence after it, and short (this many beats, or seconds without a tempo).
const STACCATO_SHARE: f64 = 0.6;
const STACCATO_GAP_SECS: f64 = 0.06;
const STACCATO_BEATS: f64 = 0.6;
const STACCATO_SECS: f64 = 0.3;

/// A note starts with an attack of its own when the level dips this far
/// below the quieter of it and the note before in between …
const NEW_ATTACK_DIP_DB: f32 = 6.0;
/// … or an onset lies at its start (from a little before, where the pitch
/// hasn't locked on yet) and its attack is this much above the body of the note before.
const NEW_ATTACK_ONSET_BEFORE_SECS: f64 = 0.08;
const NEW_ATTACK_ONSET_AFTER_SECS: f64 = 0.03;
const NEW_ATTACK_JUMP_DB: f32 = 3.0;

/// Whether each note (sorted, as from [`spans`]) is struck anew rather than
/// continuing the note before – smoothing merges notes of the same pitch only
/// without one. `onsets` sorted, in seconds. The first note always is.
pub fn new_attacks(notes: &[Span], onsets: &[f32]) -> Vec<bool> {
    (0..notes.len())
        .map(|i| {
            let Some(prev) = i.checked_sub(1).map(|j| &notes[j]) else { return true };
            let s = &notes[i];
            let quieter = prev.levels.body_db.min(s.levels.body_db);
            let dip = prev.dip_after_db.is_some_and(|d| d <= quieter - NEW_ATTACK_DIP_DB);
            let from = onsets.partition_point(|&t| (t as f64) < s.start - NEW_ATTACK_ONSET_BEFORE_SECS);
            let onset = onsets.get(from).is_some_and(|&t| (t as f64) <= s.start + NEW_ATTACK_ONSET_AFTER_SECS);
            let jump = s.levels.attack_db >= prev.levels.body_db + NEW_ATTACK_JUMP_DB;
            dip || (onset && jump)
        })
        .collect()
}

/// The note's loudness from the pitch frames (sorted by time) between
/// `start` and `end` (seconds).
pub fn levels(frames: &[PitchFrame], start: f64, end: f64) -> Levels {
    let from = frames.partition_point(|f| (f.time as f64) < start - ATTACK_BEFORE_SECS);
    let to = frames.partition_point(|f| (f.time as f64) <= end).max(from);
    let span = &frames[from..to];
    let attack = span.iter().filter(|f| (f.time as f64) <= start + ATTACK_AFTER_SECS).fold(0.0f32, |m, f| m.max(f.rms));
    let mut body: Vec<f32> = span.iter().filter(|f| (f.time as f64) > start + ATTACK_AFTER_SECS).map(|f| f.rms).collect();
    if body.is_empty() {
        // a short note is all attack
        body = span.iter().filter(|f| (f.time as f64) >= start).map(|f| f.rms).collect();
    }
    let body = if body.is_empty() { attack } else { median(&mut body) };
    Levels { body_db: gain_to_db(body), attack_db: gain_to_db(attack.max(body)) }
}

/// The notes (start, end in seconds, sorted) with their levels from the pitch frames.
pub fn spans(frames: &[PitchFrame], notes: &[(f64, f64)]) -> Vec<Span> {
    notes
        .iter()
        .enumerate()
        .map(|(i, &(start, end))| Span {
            start,
            end,
            levels: levels(frames, start, end),
            dip_after_db: notes.get(i + 1).map(|&(next, _)| {
                let from = frames.partition_point(|f| (f.time as f64) < end - DIP_MARGIN_SECS);
                let to = frames.partition_point(|f| (f.time as f64) <= next + DIP_MARGIN_SECS).max(from);
                gain_to_db(frames[from..to].iter().map(|f| f.rms).fold(f32::INFINITY, f32::min).min(1.0))
            }),
        })
        .collect()
}

/// The performance of every note (sorted by start), relative to the others.
/// `tempo` (BPM) sets what counts as short for staccato.
pub fn expression(notes: &[Span], tempo: Option<f32>) -> Vec<Played> {
    if notes.is_empty() {
        return Vec::new();
    }
    let median_body = median(&mut notes.iter().map(|s| s.levels.body_db).collect::<Vec<_>>());
    let median_attack = median(&mut notes.iter().map(|s| s.levels.attack_db).collect::<Vec<_>>());
    let usual_drop = median(&mut notes.iter().map(|s| s.levels.attack_db - s.levels.body_db).collect::<Vec<_>>());
    let velocity = |db: f32| VELOCITY_MF + VELOCITY_PER_DB * (db - median_body);
    let staccato_secs = tempo.filter(|t| *t > 0.0).map(|t| STACCATO_BEATS * 60.0 / t as f64).unwrap_or(STACCATO_SECS);
    (0..notes.len())
        .map(|i| {
            let s = &notes[i];
            let attack = attack_of(notes, i, usual_drop, median_attack);
            let own = velocity(s.levels.body_db);
            let boost = match attack {
                Attack::Normal => None,
                Attack::Accent => Some(ACCENT_BOOST),
                Attack::Sforzato => Some(SFORZATO_BOOST),
            };
            // accents lift the note above the dynamics around it
            let v = match boost {
                None => own,
                Some(b) => own.max(velocity(neighbour_median(notes, i, |n| n.levels.body_db).unwrap_or(s.levels.body_db)) + b),
            };
            Played { velocity: v.round().clamp(VELOCITY_MIN, 127.0) as u8, attack, articulation: articulation_of(notes, i, staccato_secs) }
        })
        .collect()
}

/// Median of `value` over the neighbours on both sides together.
fn neighbour_median(notes: &[Span], i: usize, value: impl Fn(&Span) -> f32) -> Option<f32> {
    let mut v: Vec<f32> = notes[i.saturating_sub(NEIGHBOURS)..i].iter().chain(notes.iter().skip(i + 1).take(NEIGHBOURS)).map(value).collect();
    (!v.is_empty()).then(|| median(&mut v))
}

fn attack_of(notes: &[Span], i: usize, usual_drop: f32, median_attack: f32) -> Attack {
    if notes.len() < 3 {
        return Attack::Normal;
    }
    let side = |range: &[Span]| -> Option<f32> {
        let mut v: Vec<f32> = range.iter().map(|n| n.levels.attack_db).collect();
        (!v.is_empty()).then(|| median(&mut v))
    };
    let left = side(&notes[i.saturating_sub(NEIGHBOURS)..i]);
    let right = side(&notes[(i + 1).min(notes.len())..(i + 1 + NEIGHBOURS).min(notes.len())]);
    let l = notes[i].levels;
    let prominence = match (left, right) {
        (Some(a), Some(b)) => (l.attack_db - a).min(l.attack_db - b),
        // at the edges: against the whole recording, too
        (Some(x), None) | (None, Some(x)) => (l.attack_db - x).min(l.attack_db - median_attack),
        (None, None) => return Attack::Normal,
    };
    let drop = (l.attack_db - l.body_db) - usual_drop;
    if prominence >= SFORZATO_DB || (prominence >= SFP_PROMINENCE_DB && drop >= SFP_DROP_DB) {
        Attack::Sforzato
    } else if prominence >= ACCENT_DB {
        Attack::Accent
    } else {
        Attack::Normal
    }
}

fn articulation_of(notes: &[Span], i: usize, staccato_secs: f64) -> Articulation {
    let Some(next) = notes.get(i + 1) else { return Articulation::Normal };
    let s = &notes[i];
    let (dur, way, gap) = (s.end - s.start, next.start - s.start, next.start - s.end);
    let quieter = s.levels.body_db.min(next.levels.body_db);
    let held = s.dip_after_db.is_none_or(|dip| dip >= quieter - LEGATO_DIP_DB);
    if gap <= LEGATO_GAP_SECS && held {
        Articulation::Legato
    } else if dur <= STACCATO_SHARE * way && gap >= STACCATO_GAP_SECS && dur <= staccato_secs {
        Articulation::Staccato
    } else {
        Articulation::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, end: f64, body_db: f32, attack_db: f32) -> Span {
        Span { start, end, levels: Levels { body_db, attack_db }, dip_after_db: None }
    }

    /// Eight notes of a beat each, all alike (attack 3 dB above the body).
    fn even() -> Vec<Span> {
        (0..8).map(|i| span(i as f64 * 0.5, i as f64 * 0.5 + 0.4, -20.0, -17.0)).collect()
    }

    #[test]
    fn even_notes_are_mf_without_accents() {
        let p = expression(&even(), Some(120.0));
        assert!(p.iter().all(|p| p.velocity == 80 && p.attack == Attack::Normal && p.articulation == Articulation::Normal), "{p:?}");
    }

    #[test]
    fn dynamics_follow_the_level_relative_to_the_recording() {
        let mut n = even();
        for s in &mut n[..4] {
            s.levels = Levels { body_db: -30.0, attack_db: -27.0 };
        }
        // the whole recording 20 dB quieter: the same dynamics
        let quiet: Vec<Span> = n.iter().map(|s| span(s.start, s.end, s.levels.body_db - 20.0, s.levels.attack_db - 20.0)).collect();
        let p = expression(&n, None);
        assert_eq!(p.iter().map(|p| p.velocity).collect::<Vec<_>>(), expression(&quiet, None).iter().map(|p| p.velocity).collect::<Vec<_>>());
        // a sudden change of dynamics (subito) isn't an accent
        assert!(p.iter().all(|p| p.attack == Attack::Normal), "{p:?}");
        assert!(p[0].velocity < p[7].velocity && p[7].velocity - p[0].velocity >= 30, "{p:?}");
    }

    #[test]
    fn standing_out_attacks_are_accents_and_sforzati() {
        let mut n = even();
        n[2].levels = Levels { body_db: -18.0, attack_db: -11.0 }; // 6 dB above the others' attacks
        n[5].levels = Levels { body_db: -20.0, attack_db: -6.0 }; // 11 dB above: sforzato
        let p = expression(&n, Some(120.0));
        assert_eq!(p[2].attack, Attack::Accent);
        assert_eq!(p[5].attack, Attack::Sforzato);
        assert!(p.iter().enumerate().filter(|(i, _)| *i != 2 && *i != 5).all(|(_, p)| p.attack == Attack::Normal));
        assert!(p[5].velocity > p[2].velocity && p[2].velocity > p[0].velocity, "{p:?}");
        assert!(p[5].velocity >= 80 + 32 && p[2].velocity >= 80 + 18);
        // sfp: stands out by 7 dB and falls back to a soft body
        let mut sfp = even();
        sfp[4].levels = Levels { body_db: -30.0, attack_db: -10.0 };
        assert_eq!(expression(&sfp, None)[4].attack, Attack::Sforzato);
    }

    #[test]
    fn staccato_and_legato_from_the_way_to_the_next_note() {
        let mut n = even();
        n[1].end = n[1].start + 0.15; // short, then silence
        n[3].end = n[4].start; // runs into the next note
        n[5].end = n[6].start + 0.01;
        let p = expression(&n, Some(120.0));
        assert_eq!(p[1].articulation, Articulation::Staccato);
        assert_eq!(p[3].articulation, Articulation::Legato);
        assert_eq!(p[5].articulation, Articulation::Legato);
        assert_eq!(p[0].articulation, Articulation::Normal);
        assert_eq!(p[7].articulation, Articulation::Normal, "the last note has nothing to compare with");
        // touching, but the level dips in between (a break the pitch frames smeared away): not legato
        let mut dip = even();
        dip[3].end = dip[4].start;
        dip[3].dip_after_db = Some(-45.0);
        assert_eq!(expression(&dip, Some(120.0))[3].articulation, Articulation::Normal);
        dip[3].dip_after_db = Some(-24.0);
        assert_eq!(expression(&dip, Some(120.0))[3].articulation, Articulation::Legato);
        // a short note before a long rest at a slow tempo: still staccato; a long one isn't
        let rest = vec![span(0.0, 0.2, -20.0, -17.0), span(2.0, 2.4, -20.0, -17.0), span(2.5, 3.4, -20.0, -17.0), span(5.0, 5.4, -20.0, -17.0)];
        let p = expression(&rest, Some(60.0));
        assert_eq!(p[0].articulation, Articulation::Staccato);
        assert_eq!(p[2].articulation, Articulation::Normal);
    }

    #[test]
    fn a_new_attack_needs_a_dip_or_an_onset_with_a_jump() {
        let mut n = even();
        for (i, s) in n.iter_mut().enumerate() {
            s.dip_after_db = (i < 7).then_some(-60.0); // silence between the notes
        }
        assert!(new_attacks(&n, &[]).iter().all(|&a| a), "a gap of silence: every note struck");
        // held through: the level stays up between notes 2 and 3
        n[2].dip_after_db = Some(-21.0);
        let a = new_attacks(&n, &[]);
        assert!(!a[3] && a[2] && a[4], "{a:?}");
        // an onset at note 3, but no jump: a timbre change, not an attack
        n[3].levels.attack_db = -18.5;
        assert!(!new_attacks(&n, &[1.49])[3]);
        // an onset with an attack 3 dB above the body before: struck anew
        n[3].levels.attack_db = -16.5;
        assert!(new_attacks(&n, &[1.49])[3]);
        assert!(!new_attacks(&n, &[1.3])[3], "the onset is too far before");
    }

    #[test]
    fn levels_from_the_frames() {
        let frame = |t: f32, rms: f32| PitchFrame { time: t, freq: Some(220.0), clarity: 0.9, rms };
        // loud attack (0.4) falling back to 0.1 over the note
        let frames: Vec<PitchFrame> = (0..60).map(|i| frame(i as f32 * 0.01, if i < 5 { 0.4 } else { 0.1 })).collect();
        let l = levels(&frames, 0.0, 0.59);
        assert!((l.attack_db - gain_to_db(0.4)).abs() < 0.01 && (l.body_db - gain_to_db(0.1)).abs() < 0.01, "{l:?}");
        // a very short note: body = its own frames
        let s = levels(&frames, 0.3, 0.33);
        assert!((s.body_db - gain_to_db(0.1)).abs() < 0.01);
    }
}
