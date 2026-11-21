//! Differences between two versions of a project: which tracks changed, in
//! which time ranges, and whether their sound or mix changed. The history uses
//! it for the "where" of every step and to find what a variant changed.

use crate::model::{ClipSource, Id, Project, Track};
use serde::{Deserialize, Serialize};

/// Time range in beats, `[start, end)`.
pub type Range = (f64, f64);

/// What differs between two versions of a project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDiff {
    pub tempo: bool,
    /// Time signature (beats per bar, beat unit).
    pub signature: bool,
    pub name: bool,
    /// Changed tracks: in the order of the newer version, removed ones last.
    pub tracks: Vec<TrackDiff>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackChange {
    Added,
    Removed,
    Changed,
}

/// How one track differs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDiff {
    pub track_id: Id,
    pub change: TrackChange,
    /// The sound changed (instrument, pitch).
    pub sound: bool,
    /// The mix changed (volume, pan, mute, solo).
    pub mix: bool,
    /// Name or color changed.
    pub meta: bool,
    /// Time ranges whose content changed (sorted, without overlaps).
    pub ranges: Vec<Range>,
    pub notes_added: u32,
    pub notes_removed: u32,
}

impl TrackDiff {
    /// Only name, color or row changes – nothing you can hear.
    pub fn is_silent(&self) -> bool {
        self.change == TrackChange::Changed && !self.sound && !self.mix && self.ranges.is_empty()
    }
}

impl ProjectDiff {
    pub fn between(a: &Project, b: &Project) -> Self {
        let mut tracks = Vec::new();
        for tb in &b.tracks {
            if let Some(d) = diff_track(a.track(tb.id), Some(tb), a.tempo_bpm, b.tempo_bpm) {
                tracks.push(d);
            }
        }
        for ta in a.tracks.iter().filter(|t| b.track(t.id).is_none()) {
            tracks.extend(diff_track(Some(ta), None, a.tempo_bpm, b.tempo_bpm));
        }
        ProjectDiff {
            tempo: a.tempo_bpm != b.tempo_bpm,
            signature: a.beats_per_bar != b.beats_per_bar || a.beat_unit != b.beat_unit,
            name: a.name != b.name,
            tracks,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.tempo && !self.signature && !self.name && self.tracks.is_empty()
    }

    /// All changed time ranges of all tracks together.
    pub fn span(&self) -> Option<Range> {
        span_of(self.tracks.iter().flat_map(|t| t.ranges.iter().copied()))
    }

    pub fn track(&self, id: Id) -> Option<&TrackDiff> {
        self.tracks.iter().find(|t| t.track_id == id)
    }
}

/// From the first start to the last end.
pub fn span_of(ranges: impl IntoIterator<Item = Range>) -> Option<Range> {
    ranges.into_iter().fold(None, |acc, (a, b)| Some(match acc {
        Some((x, y)) => (f64::min(x, a), f64::max(y, b)),
        None => (a, b),
    }))
}

/// Compares two versions of a track (`None` = not there). `None` when nothing changed.
pub fn diff_track(a: Option<&Track>, b: Option<&Track>, bpm_a: f64, bpm_b: f64) -> Option<TrackDiff> {
    match (a, b) {
        (None, None) => None,
        (None, Some(t)) | (Some(t), None) => {
            let added = a.is_none();
            let ev = events(t, if added { bpm_b } else { bpm_a });
            let notes = ev.iter().filter(|e| e.audio.is_none()).count() as u32;
            Some(TrackDiff {
                track_id: t.id,
                change: if added { TrackChange::Added } else { TrackChange::Removed },
                sound: false,
                mix: false,
                meta: false,
                ranges: merge_ranges(ev.iter().map(Ev::range).collect()),
                notes_added: if added { notes } else { 0 },
                notes_removed: if added { 0 } else { notes },
            })
        }
        (Some(a), Some(b)) => {
            let (only_a, only_b) = sym_diff(events(a, bpm_a), events(b, bpm_b));
            let d = TrackDiff {
                track_id: b.id,
                change: TrackChange::Changed,
                sound: a.instrument != b.instrument || a.pitch_semitones != b.pitch_semitones || a.kind != b.kind,
                mix: a.gain_db != b.gain_db || a.pan != b.pan || a.mute != b.mute || a.solo != b.solo,
                meta: a.name != b.name || a.color != b.color,
                ranges: merge_ranges(only_a.iter().chain(&only_b).map(Ev::range).collect()),
                notes_added: only_b.iter().filter(|e| e.audio.is_none()).count() as u32,
                notes_removed: only_a.iter().filter(|e| e.audio.is_none()).count() as u32,
            };
            (d.sound || d.mix || d.meta || !d.ranges.is_empty()).then_some(d)
        }
    }
}

/// Sorts ranges and joins the ones that overlap or touch.
pub fn merge_ranges(mut r: Vec<Range>) -> Vec<Range> {
    r.retain(|(a, b)| a.is_finite() && b.is_finite());
    r.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out: Vec<Range> = Vec::with_capacity(r.len());
    for (a, b) in r {
        match out.last_mut() {
            Some(last) if a <= last.1 + 1e-6 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// What sounds in a track, comparable across versions: notes and recordings at
/// their place in the song (clip boundaries don't matter, only what you hear).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Ev {
    start: i64,
    len: i64,
    pitch: i16,
    velocity: u8,
    expr: [i32; 3],
    /// Recording: asset, source tempo, warp, duration.
    audio: Option<(String, i64, bool, i64)>,
}

impl Ev {
    fn range(&self) -> Range {
        let s = self.start as f64 / 1e6;
        (s, s + (self.len.max(1) as f64 / 1e6))
    }
}

fn q(x: f64) -> i64 {
    (x * 1e6).round() as i64
}

fn events(t: &Track, bpm: f64) -> Vec<Ev> {
    let mut out = Vec::new();
    for c in &t.clips {
        match &c.source {
            ClipSource::Notes { notes, .. } => out.extend(notes.iter().map(|n| Ev {
                start: q(c.start_beats + n.start_beats),
                len: q(n.length_beats),
                pitch: n.pitch as i16,
                velocity: n.velocity,
                expr: [q(n.vibrato as f64) as i32, q(n.brightness as f64) as i32, q(n.bend as f64) as i32],
                audio: None,
            })),
            ClipSource::Audio { asset_id, source_bpm, warp, duration_secs } => out.push(Ev {
                start: q(c.start_beats),
                len: q(c.length_beats(bpm)),
                pitch: -1,
                velocity: 0,
                expr: [0; 3],
                audio: Some((asset_id.clone(), q(*source_bpm), *warp, q(*duration_secs))),
            }),
        }
    }
    out
}

/// Elements only in `a` and only in `b` (as multisets).
fn sym_diff(mut a: Vec<Ev>, mut b: Vec<Ev>) -> (Vec<Ev>, Vec<Ev>) {
    a.sort();
    b.sort();
    let (mut only_a, mut only_b) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                only_a.push(a[i].clone());
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                only_b.push(b[j].clone());
                j += 1;
            }
        }
    }
    only_a.extend_from_slice(&a[i..]);
    only_b.extend_from_slice(&b[j..]);
    (only_a, only_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Note, TrackKind};

    fn lead(notes: Vec<Note>, start: f64) -> Track {
        let mut t = Track::new(Id(1), "Lead", TrackKind::Instrument);
        t.clips.push(Clip { id: Id(2), name: "A".into(), start_beats: start, source: ClipSource::Notes { length_beats: 16.0, notes } });
        t
    }

    fn project(tracks: Vec<Track>) -> Project {
        Project { tracks, next_id: 10, ..Default::default() }
    }

    #[test]
    fn same_content_is_no_change_even_with_other_clip_boundaries() {
        let a = project(vec![lead(vec![Note::new(60, 0.0, 1.0, 90), Note::new(62, 8.0, 1.0, 90)], 0.0)]);
        assert!(ProjectDiff::between(&a, &a).is_empty());
        // the same notes, split over two clips: sounds the same
        let mut b = a.clone();
        b.tracks[0].clips = vec![
            Clip { id: Id(2), name: "A".into(), start_beats: 0.0, source: ClipSource::Notes { length_beats: 8.0, notes: vec![Note::new(60, 0.0, 1.0, 90)] } },
            Clip { id: Id(3), name: "B".into(), start_beats: 8.0, source: ClipSource::Notes { length_beats: 8.0, notes: vec![Note::new(62, 0.0, 1.0, 90)] } },
        ];
        assert!(ProjectDiff::between(&a, &b).is_empty());
    }

    #[test]
    fn changed_notes_give_their_time_ranges() {
        let a = project(vec![lead(vec![Note::new(60, 0.0, 1.0, 90), Note::new(62, 8.0, 1.0, 90), Note::new(64, 12.0, 2.0, 90)], 4.0)]);
        let mut b = a.clone();
        // move the second note up, louder third note, new note at the end
        b.tracks[0].clips[0].source = ClipSource::Notes {
            length_beats: 16.0,
            notes: vec![Note::new(60, 0.0, 1.0, 90), Note::new(63, 8.0, 1.0, 90), Note::new(64, 12.0, 2.0, 120), Note::new(67, 15.0, 1.0, 90)],
        };
        let d = ProjectDiff::between(&a, &b);
        let t = d.track(Id(1)).unwrap();
        assert_eq!(t.change, TrackChange::Changed);
        assert!(!t.sound && !t.mix);
        // absolute beats: clip at 4 → second note 12..13, third 16..18, new one 19..20
        assert_eq!(t.ranges, vec![(12.0, 13.0), (16.0, 18.0), (19.0, 20.0)]);
        assert_eq!((t.notes_added, t.notes_removed), (3, 2));
        assert_eq!(d.span(), Some((12.0, 20.0)));
    }

    #[test]
    fn moving_a_clip_changes_old_and_new_place() {
        let a = project(vec![lead(vec![Note::new(60, 0.0, 4.0, 90)], 0.0)]);
        let mut b = a.clone();
        b.tracks[0].clips[0].start_beats = 16.0;
        assert_eq!(ProjectDiff::between(&a, &b).tracks[0].ranges, vec![(0.0, 4.0), (16.0, 20.0)]);
    }

    #[test]
    fn sound_mix_added_removed_and_project() {
        let a = project(vec![lead(vec![Note::new(60, 0.0, 1.0, 90)], 0.0)]);
        let mut b = a.clone();
        b.tracks[0].instrument.as_mut().unwrap().params.insert("helligkeit".into(), 0.9);
        b.tracks[0].gain_db = -3.0;
        let mut bass = Track::new(Id(5), "Bass", TrackKind::Instrument);
        bass.clips.push(Clip { id: Id(6), name: "B".into(), start_beats: 8.0, source: ClipSource::Notes { length_beats: 4.0, notes: vec![Note::new(36, 0.0, 2.0, 90), Note::new(38, 2.0, 2.0, 90)] } });
        b.tracks.push(bass);
        b.tempo_bpm = 100.0;
        let d = ProjectDiff::between(&a, &b);
        assert!(d.tempo && !d.signature && !d.name);
        let t = d.track(Id(1)).unwrap();
        assert!(t.sound && t.mix && t.ranges.is_empty());
        let added = d.track(Id(5)).unwrap();
        assert_eq!((added.change, added.ranges.clone(), added.notes_added), (TrackChange::Added, vec![(8.0, 12.0)], 2));
        // the other way round the bass is removed
        let back = ProjectDiff::between(&b, &a);
        let removed = back.track(Id(5)).unwrap();
        assert_eq!((removed.change, removed.notes_removed), (TrackChange::Removed, 2));
        assert_eq!(back.tracks.last().unwrap().track_id, Id(5), "removed tracks come last");
        // renaming only: no audible change
        let mut c = a.clone();
        c.tracks[0].name = "Melodie".into();
        let d = ProjectDiff::between(&a, &c);
        assert!(d.tracks[0].meta && d.tracks[0].is_silent());
    }

    #[test]
    fn merge_ranges_joins_touching_and_overlapping() {
        assert_eq!(merge_ranges(vec![(4.0, 6.0), (0.0, 1.0), (1.0, 2.0), (5.0, 8.0), (10.0, 11.0)]), vec![(0.0, 2.0), (4.0, 8.0), (10.0, 11.0)]);
        assert_eq!(span_of(vec![(4.0, 6.0), (0.0, 1.0)]), Some((0.0, 6.0)));
        assert_eq!(span_of(Vec::<Range>::new()), None);
    }
}
