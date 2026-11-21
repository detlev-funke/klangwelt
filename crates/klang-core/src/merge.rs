//! Combining variants.
//!
//! Every variant is compared with the variants' common origin in the history
//! (a three-way merge). Per track it is clear which time ranges a variant
//! changed (rounded out to whole bars), whether it changed the track itself
//! (sound, mix, name – or added or removed it); for the project, whether it
//! changed tempo or time signature. Where several variants changed the same
//! thing differently there is a **conflict** that needs a choice; everything
//! else has an obvious default. [`combine`] builds the combined project from one
//! choice per decision, [`commands_between`] turns it into one undoable step.

use crate::command::{Command, ProjectPatch};
use crate::diff::{diff_track, merge_ranges, same_content_in, Range};
use crate::model::{Clip, ClipSource, Id, Note, Project, Track};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Where a part of the result comes from: a variant's ID or [`ORIGIN`].
pub type Source = u32;
/// The common origin (as it was before any of the variants changed it).
pub const ORIGIN: Source = 0;

/// A time range of a track that variants changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// Beats, on bar lines.
    pub start: f64,
    pub end: f64,
    /// Variants that changed it.
    pub by: Vec<Source>,
    /// Changed differently by several variants – needs a choice.
    pub conflict: bool,
}

/// One track across the versions and what has to be decided for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPlan {
    /// Key of the decisions: the track ID ("12"); "12@3" when variant 3 gave
    /// a new track an ID another variant used for a different track.
    pub key: String,
    pub track_id: Id,
    pub in_origin: bool,
    /// Variants that have the track.
    pub present_in: Vec<Source>,
    /// Variants that changed the track itself: sound, mix, name, color – or
    /// whether it exists.
    pub settings_by: Vec<Source>,
    pub settings_conflict: bool,
    /// Of those, the ones that changed the sound (instrument, pitch).
    pub sound_by: Vec<Source>,
    pub segments: Vec<Segment>,
}

/// Everything to decide when combining.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePlan {
    /// The variant the result builds on (track order, project name).
    pub target: Source,
    pub variants: Vec<Source>,
    /// The ranges are rounded to bars of this length (the origin's).
    pub beats_per_bar: u32,
    /// Variants that changed tempo or time signature.
    pub project_by: Vec<Source>,
    pub project_conflict: bool,
    pub tracks: Vec<TrackPlan>,
}

/// The choices of the person; whatever is missing takes its default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Choices {
    /// Tempo and time signature.
    pub project: Option<Source>,
    /// Track key → where the track itself comes from (sound, mix, whether it exists).
    pub tracks: BTreeMap<String, Source>,
    pub segments: Vec<SegmentChoice>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentChoice {
    pub key: String,
    pub start: f64,
    pub source: Source,
}

impl Choices {
    fn segment(&self, key: &str, start: f64) -> Option<Source> {
        self.segments.iter().find(|c| c.key == key && (c.start - start).abs() < 1e-6).map(|c| c.source)
    }
}

/// The versions to combine: the origin and the project of every variant.
pub struct Versions<'a> {
    pub origin: &'a Project,
    pub variants: Vec<(Source, &'a Project)>,
}

impl<'a> Versions<'a> {
    pub fn project(&self, s: Source) -> &'a Project {
        self.variants.iter().find(|v| v.0 == s).map(|v| v.1).unwrap_or(self.origin)
    }
}

/// A track across the versions.
struct Lane<'a> {
    key: String,
    id: Id,
    origin: Option<&'a Track>,
    versions: Vec<(Source, Option<&'a Track>)>,
}

impl<'a> Lane<'a> {
    fn track(&self, s: Source) -> Option<&'a Track> {
        if s == ORIGIN {
            return self.origin;
        }
        self.versions.iter().find(|v| v.0 == s).and_then(|v| v.1)
    }
}

fn lanes<'a>(v: &Versions<'a>) -> Vec<Lane<'a>> {
    let mut out: Vec<Lane<'a>> = v
        .origin
        .tracks
        .iter()
        .map(|t| Lane { key: t.id.to_string(), id: t.id, origin: Some(t), versions: v.variants.iter().map(|(s, p)| (*s, p.track(t.id))).collect() })
        .collect();
    // new tracks, by ID; the same ID for a different track (other name or kind) stays apart
    let mut groups: Vec<(Id, Vec<(Source, &'a Track)>)> = Vec::new();
    for (s, p) in &v.variants {
        for t in p.tracks.iter().filter(|t| v.origin.track(t.id).is_none()) {
            match groups.iter_mut().find(|(id, m)| *id == t.id && m.iter().all(|(_, x)| x.name == t.name && x.kind == t.kind)) {
                Some(g) => g.1.push((*s, t)),
                None => groups.push((t.id, vec![(*s, t)])),
            }
        }
    }
    for (id, members) in &groups {
        let twice = groups.iter().filter(|g| g.0 == *id).count() > 1;
        out.push(Lane {
            key: if twice { format!("{id}@{}", members[0].0) } else { id.to_string() },
            id: *id,
            origin: None,
            versions: v.variants.iter().map(|(s, _)| (*s, members.iter().find(|m| m.0 == *s).map(|m| m.1))).collect(),
        });
    }
    out
}

fn same_settings(a: Option<&Track>, b: Option<&Track>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.kind == b.kind
                && a.name == b.name
                && a.color == b.color
                && a.gain_db == b.gain_db
                && a.pan == b.pan
                && a.mute == b.mute
                && a.solo == b.solo
                && a.pitch_semitones == b.pitch_semitones
                && a.instrument == b.instrument
        }
        _ => false,
    }
}

/// Splits the changed ranges of all variants into segments: wherever the set of
/// variants that changed something is the same, it is one segment.
fn split(changed: &[(Source, Vec<Range>)]) -> Vec<(f64, f64, Vec<Source>)> {
    let mut cuts: Vec<f64> = changed.iter().flat_map(|(_, r)| r.iter().flat_map(|&(a, b)| [a, b])).collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let mut out: Vec<(f64, f64, Vec<Source>)> = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mid = (a + b) / 2.0;
        let by: Vec<Source> = changed.iter().filter(|(_, r)| r.iter().any(|&(x, y)| x <= mid && mid < y)).map(|(s, _)| *s).collect();
        if by.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if (last.1 - a).abs() < 1e-6 && last.2 == by => last.1 = b,
            _ => out.push((a, b, by)),
        }
    }
    out
}

/// What has to be decided to combine `v` onto `target`.
pub fn plan(v: &Versions, target: Source) -> MergePlan {
    let bpb = v.origin.beats_per_bar.max(1) as f64;
    let bars = |(a, b): Range| -> Range {
        let from = (a / bpb + 1e-9).floor();
        let to = ((b / bpb) - 1e-9).ceil().max(from + 1.0);
        (from * bpb, to * bpb)
    };
    let bpm = |s: Source| v.project(s).tempo_bpm;
    let mut tracks = Vec::new();
    for lane in lanes(v) {
        let present_in: Vec<Source> = lane.versions.iter().filter(|(_, t)| t.is_some()).map(|(s, _)| *s).collect();
        let settings_by: Vec<Source> = lane.versions.iter().filter(|(_, t)| !same_settings(lane.origin, *t)).map(|(s, _)| *s).collect();
        let settings_conflict = settings_by.iter().skip(1).any(|s| !same_settings(lane.track(settings_by[0]), lane.track(*s)));
        let sound_by = lane
            .versions
            .iter()
            .filter(|(_, t)| match (lane.origin, t) {
                (Some(o), Some(t)) => o.instrument != t.instrument || o.pitch_semitones != t.pitch_semitones,
                _ => false,
            })
            .map(|(s, _)| *s)
            .collect();
        // content changes of the versions that have the track (removing it is a decision about the track itself)
        let changed: Vec<(Source, Vec<Range>)> = lane
            .versions
            .iter()
            .filter(|(_, t)| t.is_some())
            .filter_map(|(s, t)| {
                let d = diff_track(lane.origin, *t, v.origin.tempo_bpm, bpm(*s))?;
                let r = merge_ranges(d.ranges.iter().map(|&r| bars(r)).collect());
                (!r.is_empty()).then_some((*s, r))
            })
            .collect();
        let segments: Vec<Segment> = split(&changed)
            .into_iter()
            .map(|(start, end, by)| {
                let first = lane.track(by[0]);
                let conflict = by.iter().skip(1).any(|s| !same_content_in(first, lane.track(*s), bpm(by[0]), bpm(*s), (start, end)));
                Segment { start, end, by, conflict }
            })
            .collect();
        // removed in one variant, edited in another: that has to be decided too
        let removed = lane.origin.is_some() && present_in.len() < lane.versions.len();
        let settings_conflict = settings_conflict || (removed && !segments.is_empty());
        tracks.push(TrackPlan { key: lane.key, track_id: lane.id, in_origin: lane.origin.is_some(), present_in, settings_by, settings_conflict, sound_by, segments });
    }
    let meter = |p: &Project| (p.tempo_bpm, p.beats_per_bar, p.beat_unit);
    let project_by: Vec<Source> = v.variants.iter().filter(|(_, p)| meter(p) != meter(v.origin)).map(|(s, _)| *s).collect();
    let project_conflict = project_by.iter().skip(1).any(|s| meter(v.project(*s)) != meter(v.project(project_by[0])));
    MergePlan { target, variants: v.variants.iter().map(|(s, _)| *s).collect(), beats_per_bar: v.origin.beats_per_bar, project_by, project_conflict, tracks }
}

/// Decisions without a choice that need one (conflicts), in words for the agent and tests.
pub fn open_decisions(plan: &MergePlan, choices: &Choices) -> Vec<String> {
    let mut open = Vec::new();
    if plan.project_conflict && choices.project.is_none() {
        open.push("tempo/time signature".to_string());
    }
    for t in &plan.tracks {
        if t.settings_conflict && !choices.tracks.contains_key(&t.key) {
            open.push(format!("track {}", t.key));
        }
        for s in t.segments.iter().filter(|s| s.conflict && choices.segment(&t.key, s.start).is_none()) {
            open.push(format!("track {} beats {}–{}", t.key, s.start, s.end));
        }
    }
    open
}

/// The default for a decision: the only variant that changed it (or the
/// first of several equal changes); for an open conflict the target if it
/// is among them (preview), otherwise the first one.
fn pick(by: &[Source], conflict: bool, target: Source) -> Source {
    match by {
        [] => ORIGIN,
        _ if conflict && by.contains(&target) => target,
        _ => by[0],
    }
}

/// The combined project. Open conflicts are an error (their list); see [`preview`].
pub fn combine(v: &Versions, plan: &MergePlan, choices: &Choices) -> Result<Project, Vec<String>> {
    let open = open_decisions(plan, choices);
    if !open.is_empty() {
        return Err(open);
    }
    Ok(build(v, plan, choices))
}

/// The combined project for listening: open conflicts take the target's version
/// (or the first variant's).
pub fn preview(v: &Versions, plan: &MergePlan, choices: &Choices) -> Project {
    build(v, plan, choices)
}

/// Part of a clip in a time range, before the parts are joined into clips again.
struct Frag {
    id: Id,
    name: String,
    start: f64,
    end: f64,
    /// Where the clip began and how long it was (to keep the numbers exact when it stays whole).
    orig: (f64, f64),
    /// Notes with the start of their clip.
    notes: Vec<(f64, Note)>,
    audio: Option<ClipSource>,
}

fn fragments(t: &Track, bpm: f64, (a, b): Range, out: &mut Vec<Frag>) {
    for c in &t.clips {
        match &c.source {
            ClipSource::Audio { .. } => {
                if c.start_beats >= a && c.start_beats < b {
                    let len = c.length_beats(bpm);
                    out.push(Frag { id: c.id, name: c.name.clone(), start: c.start_beats, end: c.start_beats + len, orig: (c.start_beats, len), notes: Vec::new(), audio: Some(c.source.clone()) });
                }
            }
            ClipSource::Notes { length_beats, notes } => {
                let (cs, ce) = (c.start_beats, c.start_beats + length_beats);
                let (s, e) = (cs.max(a), ce.min(b));
                if e - s <= 1e-9 {
                    continue;
                }
                let inside = notes.iter().filter(|n| {
                    let x = cs + n.start_beats;
                    x >= s - 1e-9 && x < e - 1e-9
                });
                out.push(Frag { id: c.id, name: c.name.clone(), start: s, end: e, orig: (cs, *length_beats), notes: inside.map(|n| (cs, n.clone())).collect(), audio: None });
            }
        }
    }
}

/// Joins neighbouring parts of the same clip again and makes clips of them.
fn rejoin(mut frags: Vec<Frag>) -> Vec<Clip> {
    frags.sort_by(|x, y| x.start.total_cmp(&y.start).then(x.id.cmp(&y.id)));
    let mut out: Vec<Frag> = Vec::new();
    for f in frags {
        let joined = out.iter_mut().rev().find(|l| l.id == f.id && l.audio.is_none() && f.audio.is_none() && (l.end - f.start).abs() < 1e-6);
        match joined {
            Some(l) => {
                l.end = f.end;
                l.notes.extend(f.notes);
            }
            None => out.push(f),
        }
    }
    out.into_iter()
        .map(|f| {
            let source = match f.audio {
                Some(a) => a,
                None => {
                    let whole = f.start == f.orig.0 && f.end == f.orig.0 + f.orig.1;
                    let length_beats = if whole { f.orig.1 } else { f.end - f.start };
                    let mut notes: Vec<Note> = f
                        .notes
                        .into_iter()
                        .map(|(cs, n)| if cs == f.start { n } else { Note { start_beats: cs + n.start_beats - f.start, ..n } })
                        .collect();
                    notes.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
                    ClipSource::Notes { length_beats, notes }
                }
            };
            Clip { id: f.id, name: f.name, start_beats: f.start, source }
        })
        .collect()
}

fn build(v: &Versions, plan: &MergePlan, choices: &Choices) -> Project {
    let target = v.project(plan.target);
    let mut out = target.clone();
    let ps = choices.project.unwrap_or_else(|| pick(&plan.project_by, plan.project_conflict, plan.target));
    let pp = v.project(ps);
    (out.tempo_bpm, out.beats_per_bar, out.beat_unit) = (pp.tempo_bpm, pp.beats_per_bar, pp.beat_unit);

    let lanes = lanes(v);
    let mut built: BTreeMap<String, Track> = BTreeMap::new();
    for lane in &lanes {
        let Some(tp) = plan.tracks.iter().find(|t| t.key == lane.key) else { continue };
        let src = choices.tracks.get(&tp.key).copied().unwrap_or_else(|| pick(&tp.settings_by, tp.settings_conflict, plan.target));
        let Some(base) = lane.track(src) else { continue };
        // the changed segments from their chosen version, everything else from the origin
        let mut pieces: Vec<(Range, Source)> = Vec::new();
        let mut at = f64::NEG_INFINITY;
        for s in &tp.segments {
            let from = choices.segment(&tp.key, s.start).unwrap_or_else(|| pick(&s.by, s.conflict, plan.target));
            if s.start > at {
                pieces.push(((at, s.start), ORIGIN));
            }
            pieces.push(((s.start, s.end), from));
            at = s.end;
        }
        pieces.push(((at, f64::INFINITY), ORIGIN));
        let mut frags = Vec::new();
        for (range, from) in pieces {
            if let Some(t) = lane.track(from) {
                fragments(t, v.project(from).tempo_bpm, range, &mut frags);
            }
        }
        let mut track = base.clone();
        track.clips = rejoin(frags);
        built.insert(tp.key.clone(), track);
    }

    // order: the target's; a track it doesn't have goes after its neighbour in the
    // version it comes from (past tracks that version doesn't know)
    let key_of = |s: Source, id: Id| lanes.iter().find(|l| l.track(s).is_some_and(|t| t.id == id)).map(|l| l.key.clone());
    let mut order: Vec<String> = Vec::new();
    let sources = std::iter::once(plan.target).chain(std::iter::once(ORIGIN)).chain(plan.variants.iter().copied().filter(|s| *s != plan.target));
    for s in sources {
        let known: Vec<String> = v.project(s).tracks.iter().filter_map(|t| key_of(s, t.id)).collect();
        let mut prev: Option<usize> = None;
        for k in &known {
            match order.iter().position(|x| x == k) {
                Some(i) => prev = Some(i),
                None => {
                    let mut i = prev.map(|i| i + 1).unwrap_or(0);
                    while prev.is_some() && i < order.len() && !known.contains(&order[i]) {
                        i += 1;
                    }
                    order.insert(i, k.clone());
                    prev = Some(i);
                }
            }
        }
    }
    let mut tracks: Vec<Track> = order.iter().filter_map(|k| built.remove(k)).collect();

    // IDs: variants hand out the same new IDs independently – later duplicates get fresh ones
    let max_id = std::iter::once(v.origin)
        .chain(v.variants.iter().map(|(_, p)| *p))
        .flat_map(|p| std::iter::once(p.next_id.saturating_sub(1)).chain(p.tracks.iter().flat_map(|t| std::iter::once(t.id.0).chain(t.clips.iter().map(|c| c.id.0)))))
        .max()
        .unwrap_or(0);
    let mut next = max_id + 1;
    let mut seen = BTreeSet::new();
    let mut fresh = |id: &mut Id| {
        if !seen.insert(*id) {
            *id = Id(next);
            next += 1;
            seen.insert(*id);
        }
    };
    for t in &mut tracks {
        fresh(&mut t.id);
        for c in &mut t.clips {
            fresh(&mut c.id);
        }
    }
    out.tracks = tracks;
    out.next_id = next;
    out
}

/// Commands that turn `from` into `to` (as one batch: one undo step). Changed
/// tracks are replaced as a whole; the order is set at the end.
pub fn commands_between(from: &Project, to: &Project) -> Vec<Command> {
    let mut cmds = Vec::new();
    let mut patch = ProjectPatch::default();
    if from.name != to.name {
        patch.name = Some(to.name.clone());
    }
    if from.tempo_bpm != to.tempo_bpm {
        patch.tempo_bpm = Some(to.tempo_bpm);
    }
    if from.beats_per_bar != to.beats_per_bar {
        patch.beats_per_bar = Some(to.beats_per_bar);
    }
    if from.beat_unit != to.beat_unit {
        patch.beat_unit = Some(to.beat_unit);
    }
    if patch != ProjectPatch::default() {
        cmds.push(Command::SetProject { patch });
    }
    let same = |t: &Track| to.track(t.id) == Some(t);
    for t in from.tracks.iter().filter(|t| !same(t)) {
        cmds.push(Command::RemoveTrack { track_id: t.id });
    }
    let mut order: Vec<Id> = from.tracks.iter().filter(|t| same(t)).map(|t| t.id).collect();
    for (i, t) in to.tracks.iter().enumerate() {
        if !order.contains(&t.id) {
            let at = i.min(order.len());
            cmds.push(Command::AddTrack { track: t.clone(), index: Some(at) });
            order.insert(at, t.id);
        }
    }
    for (i, t) in to.tracks.iter().enumerate() {
        if order[i] != t.id {
            let from_i = order.iter().position(|x| *x == t.id).expect("added above");
            let id = order.remove(from_i);
            order.insert(i, id);
            cmds.push(Command::MoveTrack { track_id: t.id, index: i });
        }
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{apply, Instrument, TrackKind};

    fn note(pitch: u8, start: f64) -> Note {
        Note::new(pitch, start, 1.0, 90)
    }

    fn track(id: u64, name: &str, clip: u64, start: f64, len: f64, notes: Vec<Note>) -> Track {
        let mut t = Track::new(Id(id), name, TrackKind::Instrument);
        t.color = "#ffb454".into();
        t.clips.push(Clip { id: Id(clip), name: "A".into(), start_beats: start, source: ClipSource::Notes { length_beats: len, notes } });
        t
    }

    /// Origin: Lead with one note per bar over 12 bars (clip 0..48), Bass with a clip 0..16.
    fn origin() -> Project {
        let lead = track(1, "Lead", 2, 0.0, 48.0, (0..12).map(|i| note(60, i as f64 * 4.0)).collect());
        let bass = track(3, "Bass", 4, 0.0, 16.0, vec![note(36, 0.0), note(36, 8.0)]);
        Project { tracks: vec![lead, bass], next_id: 5, ..Default::default() }
    }

    fn set_lead(p: &mut Project, f: impl Fn(&mut Vec<Note>)) {
        if let ClipSource::Notes { notes, .. } = &mut p.tracks[0].clips[0].source {
            f(notes);
        }
    }

    fn lead_notes(p: &Project) -> Vec<(u8, f64)> {
        let t = p.tracks.iter().find(|t| t.name == "Lead").unwrap();
        let mut v: Vec<(u8, f64)> = t
            .clips
            .iter()
            .flat_map(|c| match &c.source {
                ClipSource::Notes { notes, .. } => notes.iter().map(|n| (n.pitch, c.start_beats + n.start_beats)).collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
        v
    }

    #[test]
    fn segments_per_track_with_conflicts_where_both_changed_differently() {
        let o = origin();
        // B changes bars 5–8 (beats 16..32), C bars 7–10 (beats 24..40)
        let mut b = o.clone();
        set_lead(&mut b, |n| (4..8).for_each(|i| n[i].pitch = 62));
        let mut c = o.clone();
        set_lead(&mut c, |n| (6..10).for_each(|i| n[i].pitch = 64));
        let v = Versions { origin: &o, variants: vec![(1, &o), (2, &b), (3, &c)] };
        let p = plan(&v, 1);
        let lead = &p.tracks[0];
        assert_eq!(lead.key, "1");
        let segs: Vec<(f64, f64, Vec<Source>, bool)> = lead.segments.iter().map(|s| (s.start, s.end, s.by.clone(), s.conflict)).collect();
        assert_eq!(segs, vec![(16.0, 24.0, vec![2], false), (24.0, 32.0, vec![2, 3], true), (32.0, 40.0, vec![3], false)]);
        assert!(p.tracks[1].segments.is_empty(), "bass unchanged");
        assert!(p.project_by.is_empty() && !p.project_conflict);
        // without a choice for the conflict: no result
        let open = combine(&v, &p, &Choices::default()).unwrap_err();
        assert_eq!(open, vec!["track 1 beats 24–32".to_string()]);
        // C for the conflict: B in bars 5–6, C in 7–10
        let choices = Choices { segments: vec![SegmentChoice { key: "1".into(), start: 24.0, source: 3 }], ..Default::default() };
        let r = combine(&v, &p, &choices).unwrap();
        let pitches: Vec<u8> = lead_notes(&r).iter().map(|n| n.0).collect();
        assert_eq!(pitches, vec![60, 60, 60, 60, 62, 62, 64, 64, 64, 64, 60, 60]);
        // the lead clip stays one clip (its parts are joined again), bass untouched
        assert_eq!(r.tracks[0].clips.len(), 1);
        assert_eq!(r.tracks[0].clips[0].id, Id(2));
        assert_eq!(r.tracks[1], o.tracks[1]);
        // the preview takes the target for an open conflict if it changed it, otherwise the first variant (B)
        let pv = preview(&v, &p, &Choices::default());
        assert_eq!(lead_notes(&pv).iter().map(|n| n.0).collect::<Vec<_>>(), vec![60, 60, 60, 60, 62, 62, 62, 62, 64, 64, 60, 60]);
    }

    #[test]
    fn equal_changes_are_no_conflict_and_the_target_s_own_changes_count() {
        let o = origin();
        let mut t = o.clone();
        set_lead(&mut t, |n| n[0].velocity = 120); // bar 1 in the target itself
        let mut b = o.clone();
        set_lead(&mut b, |n| n[5].pitch = 67);
        let mut c = b.clone(); // the same change in both
        set_lead(&mut c, |n| n[11].pitch = 72);
        let v = Versions { origin: &o, variants: vec![(1, &t), (2, &b), (3, &c)] };
        let p = plan(&v, 1);
        let segs: Vec<(f64, Vec<Source>, bool)> = p.tracks[0].segments.iter().map(|s| (s.start, s.by.clone(), s.conflict)).collect();
        assert_eq!(segs, vec![(0.0, vec![1], false), (20.0, vec![2, 3], false), (44.0, vec![3], false)]);
        let r = combine(&v, &p, &Choices::default()).unwrap();
        let notes = lead_notes(&r);
        assert_eq!((notes[5].0, notes[11].0), (67, 72));
        assert_eq!(r.tracks[0].clips[0].source.clone(), {
            let mut x = o.tracks[0].clips[0].source.clone();
            if let ClipSource::Notes { notes, .. } = &mut x {
                notes[0].velocity = 120;
                notes[5].pitch = 67;
                notes[11].pitch = 72;
            }
            x
        });
        // choosing the origin undoes a change
        let back = Choices { segments: vec![SegmentChoice { key: "1".into(), start: 0.0, source: ORIGIN }], ..Default::default() };
        assert_eq!(lead_notes(&combine(&v, &p, &back).unwrap())[0], (60, 0.0));
        assert_eq!(match &combine(&v, &p, &back).unwrap().tracks[0].clips[0].source { ClipSource::Notes { notes, .. } => notes[0].velocity, _ => 0 }, 90);
    }

    #[test]
    fn sound_tracks_added_removed_and_tempo() {
        let o = origin();
        // B: darker lead sound, bass removed, tempo 100
        let mut b = o.clone();
        b.tracks[0].instrument = Some(Instrument { preset: "dunkel".into(), ..Instrument::pluck() });
        b.tracks.remove(1);
        b.tempo_bpm = 100.0;
        // C: a pad added (ID 5 – B might have used it too), the lead louder
        let mut c = o.clone();
        c.tracks[0].gain_db = -3.0;
        c.tracks.push(track(5, "Pad", 6, 32.0, 16.0, vec![note(48, 0.0)]));
        c.next_id = 7;
        let v = Versions { origin: &o, variants: vec![(1, &o), (2, &b), (3, &c)] };
        let p = plan(&v, 1);
        let lead = &p.tracks[0];
        assert_eq!((lead.settings_by.clone(), lead.settings_conflict, lead.sound_by.clone()), (vec![2, 3], true, vec![2]));
        // removing is a decision about the track, not a change in its bars
        let bass = &p.tracks[1];
        assert_eq!((bass.settings_by.clone(), bass.present_in.clone(), bass.segments.len(), bass.settings_conflict), (vec![2], vec![1, 3], 0, false));
        let pad = &p.tracks[2];
        assert_eq!((pad.key.as_str(), pad.in_origin, pad.settings_by.clone()), ("5", false, vec![3]));
        assert_eq!(pad.segments, vec![Segment { start: 32.0, end: 36.0, by: vec![3], conflict: false }]);
        assert_eq!((p.project_by.clone(), p.project_conflict), (vec![2], false));
        assert_eq!(open_decisions(&p, &Choices::default()), vec!["track 1"]);
        // the lead from B (sound), the rest by default: bass removed, pad added, tempo 100
        let mut ch = Choices::default();
        ch.tracks.insert("1".into(), 2);
        let r = combine(&v, &p, &ch).unwrap();
        assert_eq!(r.tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Lead", "Pad"]);
        assert_eq!(r.tracks[0].instrument.as_ref().unwrap().preset, "dunkel");
        assert_eq!(r.tracks[0].gain_db, 0.0);
        assert_eq!(r.tempo_bpm, 100.0);
        assert!(r.next_id >= 7);
        // keep the bass (origin) and the old tempo, leave the pad out
        ch.tracks.insert("3".into(), ORIGIN);
        ch.tracks.insert("5".into(), ORIGIN);
        ch.project = Some(ORIGIN);
        let r = combine(&v, &p, &ch).unwrap();
        assert_eq!(r.tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Lead", "Bass"]);
        assert_eq!(r.tracks[1], o.tracks[1]);
        assert_eq!(r.tempo_bpm, 120.0);
    }

    #[test]
    fn removed_in_one_variant_and_edited_in_another_is_a_conflict() {
        let o = origin();
        let mut b = o.clone();
        b.tracks.remove(1);
        let mut c = o.clone();
        if let ClipSource::Notes { notes, .. } = &mut c.tracks[1].clips[0].source {
            notes[1].pitch = 38;
        }
        let v = Versions { origin: &o, variants: vec![(1, &o), (2, &b), (3, &c)] };
        let p = plan(&v, 1);
        let bass = &p.tracks[1];
        assert!(bass.settings_conflict);
        assert_eq!(bass.segments, vec![Segment { start: 8.0, end: 12.0, by: vec![3], conflict: false }]);
        let mut keep = Choices::default();
        keep.tracks.insert("3".into(), ORIGIN);
        let r = combine(&v, &p, &keep).unwrap();
        assert_eq!(match &r.tracks[1].clips[0].source { ClipSource::Notes { notes, .. } => notes[1].pitch, _ => 0 }, 38, "kept, with C's edit");
        keep.tracks.insert("3".into(), 2);
        assert_eq!(combine(&v, &p, &keep).unwrap().tracks.len(), 1, "removed as in B");
    }

    #[test]
    fn the_same_new_id_for_different_tracks_stays_apart_and_gets_a_fresh_id() {
        let o = origin();
        let mut b = o.clone();
        b.tracks.push(track(5, "Strings", 6, 0.0, 4.0, vec![note(55, 0.0)]));
        let mut c = o.clone();
        c.tracks.push(track(5, "Choir", 6, 4.0, 4.0, vec![note(67, 0.0)]));
        let v = Versions { origin: &o, variants: vec![(1, &o), (2, &b), (3, &c)] };
        let p = plan(&v, 1);
        assert_eq!(p.tracks.iter().map(|t| t.key.as_str()).collect::<Vec<_>>(), ["1", "3", "5@2", "5@3"]);
        let r = combine(&v, &p, &Choices::default()).unwrap();
        let names: Vec<&str> = r.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Lead", "Bass", "Strings", "Choir"]);
        let ids: BTreeSet<u64> = r.tracks.iter().flat_map(|t| std::iter::once(t.id.0).chain(t.clips.iter().map(|c| c.id.0))).collect();
        assert_eq!(ids.len(), 8, "every track and clip has its own ID: {ids:?}");
        assert!(r.next_id > *ids.iter().max().unwrap());
    }

    #[test]
    fn a_clip_split_between_two_versions_keeps_both_halves() {
        let o = origin();
        // B moves the second half of the lead into a clip of its own (ID 9) and changes it
        let mut b = o.clone();
        b.tracks[0].clips[0].source = ClipSource::Notes { length_beats: 24.0, notes: (0..6).map(|i| note(60, i as f64 * 4.0)).collect() };
        b.tracks[0].clips.push(Clip { id: Id(9), name: "B".into(), start_beats: 24.0, source: ClipSource::Notes { length_beats: 24.0, notes: (0..6).map(|i| note(65, i as f64 * 4.0)).collect() } });
        let v = Versions { origin: &o, variants: vec![(1, &o), (2, &b)] };
        let p = plan(&v, 1);
        assert_eq!(p.tracks[0].segments.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(), vec![(24.0, 48.0)]);
        let r = combine(&v, &p, &Choices::default()).unwrap();
        assert_eq!(lead_notes(&r).iter().map(|n| n.0).collect::<Vec<_>>(), vec![60, 60, 60, 60, 60, 60, 65, 65, 65, 65, 65, 65]);
        assert_eq!(r.tracks[0].clips.iter().map(|c| (c.id.0, c.start_beats)).collect::<Vec<_>>(), vec![(2, 0.0), (9, 24.0)]);
    }

    #[test]
    fn commands_between_turn_one_project_into_the_other_in_one_step() {
        let o = origin();
        let mut to = o.clone();
        to.tempo_bpm = 90.0;
        to.tracks.swap(0, 1);
        to.tracks[0].gain_db = -6.0; // bass changed
        to.tracks.push(track(7, "Pad", 8, 0.0, 4.0, vec![note(48, 0.0)]));
        to.next_id = 9;
        let cmds = commands_between(&o, &to);
        let mut p = o.clone();
        let inverse = apply(&mut p, Command::Batch { label: "x".into(), commands: cmds }).unwrap();
        p.next_id = to.next_id;
        assert_eq!(p, to);
        apply(&mut p, inverse).unwrap();
        p.next_id = o.next_id;
        assert_eq!(p, o, "and back");
        assert!(commands_between(&o, &o).is_empty());
    }
}
