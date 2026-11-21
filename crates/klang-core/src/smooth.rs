//! Smoothing notes that come from recordings.
//!
//! Pitch detection breaks a played or sung line into more notes than were
//! played: a scoop or an attack transient becomes a short note of its own in
//! front of the real one, a few frames an octave off cut a held note in three,
//! a wide vibrato swing or a voice break splits it into pieces of the same or
//! almost the same pitch. Smoothing puts them back together:
//!
//! * **Unusually short notes** are absorbed into a neighbour they touch.
//!   "Short" is measured three ways at once – against the piece (its median
//!   note length), against the grid, and in seconds (attack transients,
//!   vibrato swings and voice breaks last about the same at any tempo) – and
//!   the smallest limit counts, so a fast run of real sixteenths stays. A note
//!   an octave off its neighbour, or between two notes of the same pitch, may
//!   be a little longer.
//! * **Consecutive notes of the same pitch** with no real gap between them
//!   become one – unless the second one is attacked anew: measured in the
//!   recording (an onset with a jump or a dip in level), or, without that, a
//!   clearly louder or accented note.
//! * **Isolated soft blips** – short, touching nothing and much softer than
//!   the notes around them (a breath, a noise) – are removed.
//!
//! The merged note keeps the pitch of its main note, its attack (velocity,
//! accent) from where it starts and ends where its last part ends. Only a
//! single line is smoothed: notes that overlap others (chords) and notes
//! outside the selection stay as they are, and nothing merges into them.

use crate::model::Note;
use serde::{Deserialize, Serialize};

const EPS: f64 = 1e-9;

/// How much is smoothed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strength {
    #[serde(alias = "aus", alias = "none")]
    Off,
    #[serde(alias = "leicht")]
    Light,
    #[default]
    #[serde(alias = "mittel")]
    Medium,
    #[serde(alias = "stark")]
    Strong,
}

impl Strength {
    pub const ALL: [Strength; 4] = [Strength::Off, Strength::Light, Strength::Medium, Strength::Strong];

    /// Name as in the commands ("off", "light", "medium", "strong").
    pub fn key(self) -> &'static str {
        match self {
            Strength::Off => "off",
            Strength::Light => "light",
            Strength::Medium => "medium",
            Strength::Strong => "strong",
        }
    }

    fn limits(self) -> Option<Limits> {
        Some(match self {
            Strength::Off => return None,
            Strength::Light => Limits { of_median: 0.3, grid: (0.5, 0.8), secs: 0.10, join_grid: 0.25, join_secs: 0.05, held_grid: 0.5, held_secs: 0.10, stick_secs: 0.03, wider: 1.5, louder: 10, softer: 45 },
            Strength::Medium => Limits { of_median: 0.4, grid: (0.6, 1.0), secs: 0.14, join_grid: 0.5, join_secs: 0.08, held_grid: 1.0, held_secs: 0.16, stick_secs: 0.04, wider: 1.6, louder: 14, softer: 30 },
            Strength::Strong => Limits { of_median: 0.55, grid: (0.7, 1.5), secs: 0.20, join_grid: 1.0, join_secs: 0.14, held_grid: 1.5, held_secs: 0.25, stick_secs: 0.05, wider: 1.8, louder: 20, softer: 20 },
        })
    }
}

/// The thresholds of one strength.
#[derive(Clone, Copy, Debug)]
struct Limits {
    /// Short: below this share of the median note length …
    of_median: f64,
    /// … but at least the first and at most the second share of the grid …
    grid: (f64, f64),
    /// … and below this many seconds.
    secs: f64,
    /// Notes touch when the gap between them is at most this share of the
    /// grid and at most this many seconds.
    join_grid: f64,
    join_secs: f64,
    /// Two notes of the same pitch the recording says sounded through (no new
    /// attack measured: the pitch was only lost for a moment) may be this far apart.
    held_grid: f64,
    held_secs: f64,
    /// A fragment sticks to a neighbour – pitch detection cut them apart, so
    /// there is hardly a gap – when at most this many seconds lie between them.
    stick_secs: f64,
    /// A note an octave off its neighbour or between two of the same pitch may be this much longer.
    wider: f64,
    /// Without a measured attack: a second note this much louder is struck anew.
    louder: u8,
    /// An isolated short note this much softer than the notes around it is a blip.
    softer: u8,
}

/// Time scale of the notes: seconds per beat and the grid (beats) the notes
/// are meant for.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub secs_per_beat: f64,
    pub grid_beats: f64,
}

/// One note as smoothing sees it (times in beats).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub velocity: u8,
    /// How accented the attack is (e.g. the note's brightness); higher = more.
    pub accent: f32,
    /// Whether the note starts with an attack of its own, as measured in the
    /// recording; `None` = unknown (then judged from velocity and accent).
    pub new_attack: Option<bool>,
}

/// Notes that become one: their indices (by time), the one whose pitch the
/// result keeps, the span and the attack.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub members: Vec<usize>,
    pub main: usize,
    pub start: f64,
    pub end: f64,
    /// Velocity and accent of the attack (from where the group starts).
    pub velocity: u8,
    pub accent: f32,
}

/// What smoothing does with a list of notes: every note that stays is in
/// exactly one group (most groups have one member), the others were removed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Sorted by start.
    pub groups: Vec<Group>,
    pub removed: Vec<usize>,
}

impl Plan {
    /// Notes that went into another one.
    pub fn merged(&self) -> usize {
        self.groups.iter().map(|g| g.members.len() - 1).sum()
    }

    pub fn changed(&self) -> bool {
        self.merged() > 0 || !self.removed.is_empty()
    }
}

/// A note or the notes merged so far.
#[derive(Clone, Debug)]
struct Cur {
    members: Vec<usize>,
    main: usize,
    start: f64,
    end: f64,
    pitch: u8,
    velocity: u8,
    accent: f32,
    new_attack: Option<bool>,
    /// Stays as it is (overlaps other notes or isn't selected).
    fixed: bool,
}

impl Cur {
    fn len(&self) -> f64 {
        self.end - self.start
    }
}

/// Plans the smoothing of `items`; `only` = indices that may change (None = all).
pub fn plan(items: &[Item], only: Option<&[usize]>, strength: Strength, scale: Scale) -> Plan {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[a].start.total_cmp(&items[b].start).then(items[a].pitch.cmp(&items[b].pitch)));
    let single = |i: usize| Group { members: vec![i], main: i, start: items[i].start, end: items[i].end, velocity: items[i].velocity, accent: items[i].accent };
    let Some(lim) = strength.limits() else {
        return Plan { groups: order.into_iter().map(single).collect(), removed: Vec::new() };
    };
    let chosen = |i: usize| only.is_none_or(|o| o.contains(&i));
    // chord notes stay: a note overlapping another by more than half the shorter one
    let len = |i: usize| (items[i].end - items[i].start).max(0.0);
    let overlaps = |a: usize, b: usize| {
        let o = items[a].end.min(items[b].end) - items[a].start.max(items[b].start);
        o > 0.5 * len(a).min(len(b)) + EPS
    };
    let mut in_chord = vec![false; items.len()];
    for (k, &i) in order.iter().enumerate() {
        for &j in order[k + 1..].iter().take_while(|&&j| items[j].start < items[i].end - EPS) {
            if overlaps(i, j) {
                in_chord[i] = true;
                in_chord[j] = true;
            }
        }
    }

    let mut lens: Vec<f64> = order.iter().filter(|&&i| chosen(i) && !in_chord[i]).map(|&i| len(i)).collect();
    lens.sort_by(f64::total_cmp);
    let median = lens.get(lens.len() / 2).copied().unwrap_or(scale.grid_beats);
    let spb = scale.secs_per_beat.max(1e-6);
    let grid = scale.grid_beats.max(1e-6);
    let short = (lim.secs / spb).min((lim.of_median * median).clamp(lim.grid.0 * grid, lim.grid.1 * grid));
    let join = (lim.join_secs / spb).min(lim.join_grid * grid);
    let held = (lim.held_secs / spb).min(lim.held_grid * grid);
    let stick = (lim.stick_secs / spb).min(join);

    let mut cur: Vec<Cur> = order
        .iter()
        .map(|&i| {
            let it = &items[i];
            Cur { members: vec![i], main: i, start: it.start, end: it.end, pitch: it.pitch, velocity: it.velocity, accent: it.accent, new_attack: it.new_attack, fixed: !chosen(i) || in_chord[i] }
        })
        .collect();
    let within = |a: &Cur, b: &Cur, gap: f64| !a.fixed && !b.fixed && b.start - a.end <= gap + EPS;
    // the second note is struck anew (not a continuation of the first)
    let struck = |a: &Cur, b: &Cur| match b.new_attack {
        Some(s) => s,
        None => b.velocity >= a.velocity.saturating_add(lim.louder) || b.accent > a.accent + 0.05,
    };
    let octave = |a: u8, b: u8| a != b && (a as i32 - b as i32).abs() % 12 == 0;
    let distance = |a: u8, b: u8| if octave(a, b) { 0 } else { (a as i32 - b as i32).abs() };

    loop {
        // consecutive notes of the same pitch without a new attack
        let mut i = 1;
        while i < cur.len() {
            let gap = if cur[i].new_attack == Some(false) { held } else { join };
            if cur[i - 1].pitch == cur[i].pitch && within(&cur[i - 1], &cur[i], gap) && !struck(&cur[i - 1], &cur[i]) {
                let b = cur.remove(i);
                absorb(&mut cur[i - 1], b, items);
            } else {
                i += 1;
            }
        }
        // the shortest fragment goes into a neighbour it touches: a fragment struck
        // anew is never the end of the note before it; in front of a note struck
        // anew only a fragment with an attack of its own belongs to it (a scoop
        // that swells into the note, a transient), not the end of the one before
        let left = |k: usize| (k > 0 && within(&cur[k - 1], &cur[k], stick) && cur[k].new_attack != Some(true)).then(|| k - 1);
        let right = |k: usize| {
            let opens = cur[k + 1..].first().is_some_and(|r| r.new_attack != Some(true) || cur[k].new_attack == Some(true));
            (opens && within(&cur[k], &cur[k + 1], stick)).then_some(k + 1)
        };
        // a little longer when it is much shorter than the notes it sits in:
        // between two of the same pitch (a vibrato swing) or an octave off (a slip)
        let limit = |k: usize| {
            let x = &cur[k];
            let much_shorter = |j: usize| x.len() < 0.5 * cur[j].len();
            let between_same = matches!((left(k), right(k)), (Some(l), Some(r)) if cur[l].pitch == cur[r].pitch && much_shorter(l) && much_shorter(r));
            let octave_off = left(k).into_iter().chain(right(k)).any(|j| octave(cur[j].pitch, x.pitch) && much_shorter(j));
            if between_same || octave_off {
                short * lim.wider
            } else {
                short
            }
        };
        let fragment = (0..cur.len())
            .filter(|&k| !cur[k].fixed && (left(k).is_some() || right(k).is_some()) && cur[k].len() < limit(k) - EPS)
            .min_by(|&a, &b| cur[a].len().total_cmp(&cur[b].len()));
        let Some(k) = fragment else { break };
        let into = match (left(k), right(k)) {
            (Some(l), None) => l,
            (None, Some(r)) => r,
            (Some(l), Some(r)) => {
                let (x, lc, rc) = (&cur[k], &cur[l], &cur[r]);
                if lc.pitch == rc.pitch {
                    l
                } else {
                    // the side it clearly sits closer to, then the nearer pitch, the
                    // smaller gap, the longer note
                    let (gl, gr) = (x.start - lc.end, rc.start - x.end);
                    let (dl, dr) = (distance(x.pitch, lc.pitch), distance(x.pitch, rc.pitch));
                    if (gl - gr).abs() > 0.5 * stick {
                        if gl < gr { l } else { r }
                    } else if dl != dr {
                        if dl < dr { l } else { r }
                    } else if (gl - gr).abs() > EPS {
                        if gl < gr { l } else { r }
                    } else if rc.len() > lc.len() + EPS {
                        r
                    } else {
                        l
                    }
                }
            }
            (None, None) => unreachable!("a fragment touches a neighbour"),
        };
        let x = cur.remove(k);
        let into = if into > k { into - 1 } else { into };
        absorb(&mut cur[into], x, items);
    }

    // isolated soft blips
    let mut removed = Vec::new();
    let mut keep = vec![true; cur.len()];
    for k in 0..cur.len() {
        let c = &cur[k];
        let alone = (k == 0 || cur[k].start - cur[k - 1].end > stick + EPS) && (k + 1 == cur.len() || cur[k + 1].start - cur[k].end > stick + EPS);
        if c.fixed || !alone || c.len() >= short - EPS {
            continue;
        }
        let mut around: Vec<u8> = cur[k.saturating_sub(4)..k].iter().chain(cur.iter().skip(k + 1).take(4)).map(|n| n.velocity).collect();
        if around.len() < 2 {
            continue;
        }
        around.sort_unstable();
        if c.velocity.saturating_add(lim.softer) <= around[around.len() / 2] {
            keep[k] = false;
            removed.extend(&c.members);
        }
    }
    removed.sort_unstable();
    let groups = cur
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(c, _)| Group { members: c.members, main: c.main, start: c.start, end: c.end, velocity: c.velocity, accent: c.accent })
        .collect();
    Plan { groups, removed }
}

/// Smooths the notes of a clip (`only`: indices that may change, None = all).
/// Without a recording behind them, a new attack is judged from velocity and
/// accent (brightness).
///
/// Result: the notes in the order of `SetClipNotes` (start, pitch), the new
/// indices of the notes that came from the chosen ones, and the plan.
pub fn smooth_notes(notes: &[Note], only: Option<&[usize]>, strength: Strength, scale: Scale) -> (Vec<Note>, Vec<usize>, Plan) {
    let items: Vec<Item> = notes
        .iter()
        .map(|n| Item { start: n.start_beats, end: n.start_beats + n.length_beats, pitch: n.pitch, velocity: n.velocity, accent: n.brightness, new_attack: None })
        .collect();
    let plan = plan(&items, only, strength, scale);
    let chosen = |i: usize| only.is_none_or(|o| o.contains(&i));
    let mut out: Vec<(Note, bool)> = plan
        .groups
        .iter()
        .map(|g| {
            let mut n = notes[g.main].clone();
            if g.members.len() > 1 {
                n.start_beats = g.start;
                n.length_beats = g.end - g.start;
                n.velocity = g.velocity;
                n.brightness = g.accent;
            }
            (n, g.members.iter().any(|&i| chosen(i)))
        })
        .collect();
    out.sort_by(|(a, _), (b, _)| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
    let selected = out.iter().enumerate().filter(|(_, (_, s))| *s).map(|(i, _)| i).collect();
    (out.into_iter().map(|(n, _)| n).collect(), selected, plan)
}

/// Merges `b` into `a`. The attack comes from where the merged note starts:
/// is `b` earlier (a fragment in front), the louder and more accented of the
/// two counts; the pitch stays that of `a`'s main note.
fn absorb(a: &mut Cur, b: Cur, items: &[Item]) {
    if b.start < a.start - EPS {
        a.velocity = a.velocity.max(b.velocity);
        a.accent = a.accent.max(b.accent);
        a.new_attack = match (a.new_attack, b.new_attack) {
            (Some(x), Some(y)) => Some(x || y),
            (x, y) => x.or(y),
        };
    }
    // two notes of the same pitch: the longer one is the main note
    if b.pitch == a.pitch && items[b.main].end - items[b.main].start > items[a.main].end - items[a.main].start {
        a.main = b.main;
    }
    a.start = a.start.min(b.start);
    a.end = a.end.max(b.end);
    a.members.extend(b.members);
    a.members.sort_by(|&x, &y| items[x].start.total_cmp(&items[y].start));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 BPM, sixteenth grid.
    const SCALE: Scale = Scale { secs_per_beat: 0.5, grid_beats: 0.25 };

    fn n(start: f64, end: f64, pitch: u8) -> Item {
        Item { start, end, pitch, velocity: 80, accent: 0.0, new_attack: None }
    }

    fn with_attack(mut i: Item, a: bool) -> Item {
        i.new_attack = Some(a);
        i
    }

    /// (start, end, pitch) of the planned notes.
    fn result(items: &[Item], strength: Strength) -> Vec<(f64, f64, u8)> {
        let p = plan(items, None, strength, SCALE);
        p.groups.iter().map(|g| (g.start, g.end, items[g.main].pitch)).collect()
    }

    /// A clean melody in quarters and eighths with breaths between some notes.
    fn melody() -> Vec<Item> {
        vec![n(0.0, 0.95, 60), n(1.0, 1.95, 62), n(2.0, 2.45, 64), n(2.5, 2.95, 65), n(3.0, 3.9, 67), n(4.0, 5.9, 64)]
    }

    #[test]
    fn a_clean_melody_stays_as_it_is_at_every_strength() {
        for s in Strength::ALL {
            let p = plan(&melody(), None, s, SCALE);
            assert!(!p.changed(), "{s:?}: {p:?}");
            assert_eq!(p.groups.len(), 6);
        }
    }

    #[test]
    fn an_octave_slip_inside_a_held_note_disappears() {
        // c' held for two beats, a few frames an octave up in the middle
        let items = [n(0.0, 0.9, 60), n(0.9, 1.08, 72), n(1.08, 2.0, 60), n(2.0, 3.0, 64)];
        assert_eq!(result(&items, Strength::Medium), vec![(0.0, 2.0, 60), (2.0, 3.0, 64)]);
        // light still catches it (an octave off may be a little longer)
        assert_eq!(result(&items, Strength::Light), vec![(0.0, 2.0, 60), (2.0, 3.0, 64)]);
    }

    #[test]
    fn a_scoop_in_front_of_a_note_becomes_its_start_with_the_attack() {
        // a short d' sliding into e', the attack (and the accent) on the scoop
        let mut scoop = n(1.0, 1.15, 62);
        scoop.velocity = 96;
        scoop.accent = 0.2;
        let items = [n(0.0, 0.9, 60), scoop, n(1.15, 1.95, 64), n(2.0, 2.9, 67)];
        let p = plan(&items, None, Strength::Medium, SCALE);
        assert_eq!(p.groups.len(), 3, "{p:?}");
        let g = &p.groups[1];
        assert_eq!((g.start, g.end, items[g.main].pitch), (1.0, 1.95, 64));
        assert_eq!((g.velocity, g.accent), (96, 0.2), "the attack of the merged note");
        assert_eq!(g.members, vec![1, 2]);
    }

    #[test]
    fn a_fall_off_at_the_end_goes_into_the_note_before() {
        let items = [n(0.0, 0.85, 67), n(0.85, 0.97, 65), n(1.5, 2.4, 64)];
        assert_eq!(result(&items, Strength::Medium), vec![(0.0, 0.97, 67), (1.5, 2.4, 64)]);
        // measured: the note after it is struck anew right away – the fragment still ends the one before
        let items = [with_attack(n(0.0, 0.85, 67), true), with_attack(n(0.85, 0.97, 65), false), with_attack(n(0.97, 1.9, 64), true)];
        assert_eq!(result(&items, Strength::Medium), vec![(0.0, 0.97, 67), (0.97, 1.9, 64)]);
    }

    #[test]
    fn a_scoop_that_swells_into_its_note_belongs_to_it() {
        // a soft scoop after a rest: the note it slides into looks struck anew (its
        // attack jumps above the scoop), yet the scoop is where that note begins
        let items = [with_attack(n(0.0, 0.8, 60), true), with_attack(n(1.0, 1.16, 62), true), with_attack(n(1.16, 1.95, 64), true)];
        assert_eq!(result(&items, Strength::Medium), vec![(0.0, 0.8, 60), (1.0, 1.95, 64)]);
    }

    #[test]
    fn a_held_note_split_by_a_voice_break_becomes_one() {
        // same pitch, a 30 ms gap (0.06 beats), no new attack measured
        let items = [with_attack(n(0.0, 0.7, 64), true), with_attack(n(0.76, 1.9, 64), false), with_attack(n(2.0, 2.9, 62), true)];
        assert_eq!(result(&items, Strength::Medium), vec![(0.0, 1.9, 64), (2.0, 2.9, 62)]);
        // the attack of the merged note is the first one's, the longer part is the main note
        let p = plan(&items, None, Strength::Medium, SCALE);
        assert_eq!(p.groups[0].main, 1);
        assert_eq!(p.groups[0].members, vec![0, 1]);
        // the pitch lost for 80 ms (0.16 beats): bridged when the recording says the
        // note sounded through, not when nothing is known about it
        let lost = [with_attack(n(0.0, 0.7, 64), true), with_attack(n(0.86, 1.9, 64), false)];
        assert_eq!(plan(&lost, None, Strength::Medium, SCALE).groups.len(), 1);
        assert_eq!(plan(&lost, None, Strength::Light, SCALE).groups.len(), 2);
        let unknown = [n(0.0, 0.7, 64), n(0.86, 1.9, 64)];
        assert_eq!(plan(&unknown, None, Strength::Medium, SCALE).groups.len(), 2);
    }

    #[test]
    fn a_real_reattack_stays_two_notes() {
        // measured: an onset with a level jump at the second note
        let items = [with_attack(n(0.0, 0.9, 64), true), with_attack(n(0.9, 1.9, 64), true)];
        assert_eq!(plan(&items, None, Strength::Strong, SCALE).groups.len(), 2);
        // without a measurement: a clearly louder or accented second note
        let mut louder = n(1.0, 2.0, 64);
        louder.velocity = 100;
        assert_eq!(plan(&[n(0.0, 1.0, 64), louder], None, Strength::Medium, SCALE).groups.len(), 2);
        let mut accented = n(1.0, 2.0, 64);
        accented.accent = 0.4;
        assert_eq!(plan(&[n(0.0, 1.0, 64), accented], None, Strength::Medium, SCALE).groups.len(), 2);
        // evenly played touching notes of the same pitch become one
        assert_eq!(result(&[n(0.0, 1.0, 64), n(1.0, 2.0, 64)], Strength::Medium), vec![(0.0, 2.0, 64)]);
    }

    #[test]
    fn a_real_gap_keeps_repeated_notes_apart() {
        // detached repeated notes: a sixteenth rest between them
        let items = [n(0.0, 0.75, 64), n(1.0, 1.75, 64), n(2.0, 2.75, 64)];
        assert_eq!(plan(&items, None, Strength::Medium, SCALE).groups.len(), 3);
        // strong joins gaps up to a grid step (here 70 ms)
        let close = [n(0.0, 0.86, 64), n(1.0, 1.86, 64)];
        assert_eq!(plan(&close, None, Strength::Medium, SCALE).groups.len(), 2);
        assert_eq!(plan(&close, None, Strength::Strong, SCALE).groups.len(), 1);
    }

    #[test]
    fn vibrato_swings_fold_back_into_the_held_note() {
        // e' with a wide vibrato: short pieces a semitone up
        let items = [n(0.0, 0.5, 64), n(0.5, 0.62, 65), n(0.62, 1.0, 64), n(1.0, 1.12, 65), n(1.12, 1.6, 64), n(2.0, 3.0, 60)];
        assert_eq!(result(&items, Strength::Medium), vec![(0.0, 1.6, 64), (2.0, 3.0, 60)]);
    }

    #[test]
    fn a_fast_run_of_real_sixteenths_stays() {
        // 120 BPM: sixteenths of 110 ms each, touching, all different
        let items: Vec<Item> = (0..16).map(|i| n(i as f64 * 0.25, i as f64 * 0.25 + 0.22, [60, 62, 64, 65, 67, 65, 64, 62][i % 8])).collect();
        for s in [Strength::Light, Strength::Medium, Strength::Strong] {
            assert!(!plan(&items, None, s, SCALE).changed(), "{s:?}");
        }
    }

    #[test]
    fn short_depends_on_the_tempo_and_the_piece() {
        // a 0.2-beat note between two others: at 120 BPM 100 ms (a fragment) …
        let items = [n(0.0, 1.0, 60), n(1.0, 1.2, 62), n(1.2, 2.2, 64)];
        assert_eq!(plan(&items, None, Strength::Medium, SCALE).groups.len(), 2);
        // … at 40 BPM 300 ms: a real note
        let slow = Scale { secs_per_beat: 1.5, grid_beats: 0.25 };
        assert_eq!(plan(&items, None, Strength::Medium, slow).groups.len(), 3);
        // in a piece of short notes the same note is ordinary
        let busy: Vec<Item> = (0..12).map(|i| n(i as f64 * 0.25, i as f64 * 0.25 + 0.2, 60 + (i % 5) as u8 * 2)).collect();
        assert!(!plan(&busy, None, Strength::Medium, SCALE).changed());
    }

    #[test]
    fn stronger_smoothing_takes_longer_fragments() {
        // a 0.3-beat (150 ms) note between two long ones
        let items = [n(0.0, 2.0, 60), n(2.0, 2.3, 62), n(2.3, 4.3, 64)];
        assert_eq!(plan(&items, None, Strength::Light, SCALE).groups.len(), 3);
        assert_eq!(plan(&items, None, Strength::Medium, SCALE).groups.len(), 3);
        assert_eq!(plan(&items, None, Strength::Strong, SCALE).groups.len(), 2);
        assert!(!plan(&items, None, Strength::Off, SCALE).changed());
    }

    #[test]
    fn isolated_short_notes_stay_unless_they_are_soft_blips() {
        // a short loud note with rests around it is staccato
        let mut items = vec![n(0.0, 0.9, 60), n(1.5, 1.65, 67), n(2.5, 3.4, 64), n(3.5, 4.4, 62)];
        assert!(!plan(&items, None, Strength::Strong, SCALE).changed());
        // even right after a note, when it is struck anew after a short breath (60 ms)
        let staccato = [with_attack(n(0.0, 0.88, 64), true), with_attack(n(1.0, 1.24, 62), true), with_attack(n(2.0, 2.9, 60), true)];
        assert!(!plan(&staccato, None, Strength::Strong, SCALE).changed());
        // the same note much softer than the ones around it: a breath, a noise
        items[1].velocity = 40;
        let p = plan(&items, None, Strength::Medium, SCALE);
        assert_eq!(p.removed, vec![1]);
        assert_eq!(p.groups.len(), 3);
        // light only drops what is softer still
        assert!(!plan(&items, None, Strength::Light, SCALE).changed());
    }

    #[test]
    fn chords_and_unselected_notes_stay() {
        // a chord with a short note on top: nothing changes
        let chord = [n(0.0, 1.0, 60), n(0.0, 1.0, 64), n(0.0, 0.1, 67), n(1.0, 2.0, 60), n(1.0, 2.0, 64)];
        assert!(!plan(&chord, None, Strength::Strong, SCALE).changed());
        // only the selected notes: the octave slip outside the selection stays
        let items = [n(0.0, 0.9, 60), n(0.9, 1.08, 72), n(1.08, 2.0, 60), n(2.0, 2.12, 62), n(2.12, 3.0, 64)];
        let p = plan(&items, Some(&[3, 4]), Strength::Medium, SCALE);
        let got: Vec<(f64, f64, u8)> = p.groups.iter().map(|g| (g.start, g.end, items[g.main].pitch)).collect();
        assert_eq!(got, vec![(0.0, 0.9, 60), (0.9, 1.08, 72), (1.08, 2.0, 60), (2.0, 3.0, 64)]);
    }

    #[test]
    fn strength_reads_both_languages() {
        let s: Strength = serde_json::from_value(serde_json::json!("stark")).unwrap();
        assert_eq!(s, Strength::Strong);
        let s: Strength = serde_json::from_value(serde_json::json!("off")).unwrap();
        assert_eq!(s, Strength::Off);
        assert_eq!(Strength::default(), Strength::Medium);
    }

    #[test]
    fn clip_notes_keep_the_main_notes_expression_and_the_attack() {
        let note = |p: u8, s: f64, l: f64, v: u8| Note::new(p, s, l, v);
        let mut held = note(64, 1.2, 1.05, 70);
        held.vibrato = 0.3;
        let mut scoop = note(62, 1.0, 0.2, 92);
        scoop.brightness = 0.2;
        // converted from a recording: a scoop, a held note split in two, a clean note
        let notes = vec![note(60, 0.0, 1.0, 80), scoop, held, note(64, 2.25, 0.75, 72), note(67, 3.0, 1.0, 80)];
        let (out, selected, plan) = smooth_notes(&notes, None, Strength::Medium, SCALE);
        assert_eq!(plan.merged(), 2);
        assert_eq!(out.len(), 3, "{out:?}");
        let e = &out[1];
        assert_eq!((e.pitch, e.start_beats, e.length_beats), (64, 1.0, 2.0));
        assert_eq!((e.velocity, e.brightness, e.vibrato), (92, 0.2, 0.3), "attack of the scoop, vibrato of the held note");
        assert_eq!(out[0], notes[0], "untouched notes stay exactly as they were");
        assert_eq!(selected, vec![0, 1, 2]);
        // a selection: only those notes are reported
        let (out, selected, _) = smooth_notes(&notes, Some(&[1, 2, 3]), Strength::Medium, SCALE);
        assert_eq!(out.len(), 3);
        assert_eq!(selected, vec![1]);
        // nothing to smooth: the same notes
        let clean = vec![note(60, 0.0, 1.0, 80), note(62, 1.0, 1.0, 80)];
        let (out, _, plan) = smooth_notes(&clean, None, Strength::Strong, SCALE);
        assert!(!plan.changed());
        assert_eq!(out, clean);
    }
}
