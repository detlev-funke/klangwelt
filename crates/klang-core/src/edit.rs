//! Edits across all tracks.
//!
//! **Delete time range** (Delete with a range selected): The range
//! `[start, end)` disappears from all tracks, and everything after it moves
//! forward by its length (ripple delete) – so no gap is left.
//! Mathematically this is a time mapping that collapses the range to a
//! point ([`collapse`]); clips and notes are mapped with it:
//!
//! * Clips entirely inside the range are dropped, clips after it move forward.
//! * A note clip that intersects the range is shortened; its notes inside the
//!   range are dropped, and notes crossing an edge become shorter by the part
//!   inside the range (a held note stays one note).
//! * For an audio clip only the material outside the range remains; the cutting
//!   itself is done by the caller (`cut`), which holds the audio data.
//!
//! Everything together is one command (`Batch`) – so one undo step.

use crate::command::{ClipPatch, Command, CoreError};
use crate::model::{Clip, ClipSource, Note, Project};
use klang_i18n::tr;

const EPS: f64 = 1e-9;
/// Shorter notes (in beats) are dropped after collapsing.
const MIN_NOTE: f64 = 1.0 / 128.0;

/// Position after deleting `[start, end)`.
pub fn collapse(x: f64, start: f64, end: f64) -> f64 {
    if x <= start {
        x
    } else if x >= end {
        x - (end - start)
    } else {
        start
    }
}

/// Parts of the source (seconds from the start of the audio file) that an
/// audio clip should keep – joined together in this order.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioKeep {
    pub asset_id: String,
    pub parts: Vec<(f64, f64)>,
}

/// Beats per second of source material of an audio clip.
pub fn audio_beats_per_sec(source: &ClipSource, project_bpm: f64) -> f64 {
    match source {
        ClipSource::Audio { source_bpm, warp, .. } => (if *warp { *source_bpm } else { project_bpm }) / 60.0,
        ClipSource::Notes { .. } => project_bpm / 60.0,
    }
}

/// Bar.beat of a position (1-based), e.g. "3.1" or "2.2,5".
fn bar_beat(beats: f64, bpb: u32) -> String {
    let bpb = bpb.max(1) as f64;
    let bar = (beats / bpb + EPS).floor();
    let beat = beats - bar * bpb + 1.0;
    let beat = (beat * 1000.0).round() / 1000.0;
    let beat = if (beat - beat.round()).abs() < 1e-6 { format!("{}", beat.round() as i64) } else { format!("{beat}").replace('.', ",") };
    format!("{}.{beat}", bar as i64 + 1)
}

fn map_notes(notes: &[Note], clip_start: f64, start: f64, end: f64) -> Vec<Note> {
    let new_start = collapse(clip_start, start, end);
    notes
        .iter()
        .filter_map(|n| {
            let a = collapse(clip_start + n.start_beats, start, end);
            let b = collapse(clip_start + n.start_beats + n.length_beats, start, end);
            (b - a >= MIN_NOTE).then(|| Note { start_beats: (a - new_start).max(0.0), length_beats: b - a, ..n.clone() })
        })
        .collect()
}

/// Plans deleting `[start, end)` from all tracks with ripple delete.
///
/// `cut` cuts audio: It gets the clip and the parts of the source to keep and
/// returns the new source (`ClipSource::Audio` with a new file and
/// duration). The result is a batch command; an error if nothing is affected.
pub fn delete_range(
    project: &Project,
    start: f64,
    end: f64,
    cut: &mut dyn FnMut(&Clip, &AudioKeep) -> Result<ClipSource, String>,
) -> Result<Command, CoreError> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end - start < 1e-6 {
        return Err(CoreError::Invalid(tr!("Invalid time range")));
    }
    let len = end - start;
    let bpm = project.tempo_bpm;
    let mut commands = Vec::new();
    for track in &project.tracks {
        for clip in &track.clips {
            let cs = clip.start_beats;
            let ce = cs + clip.length_beats(bpm);
            if ce <= start + EPS {
                continue; // entirely before
            }
            let (ns, ne) = (collapse(cs, start, end), collapse(ce, start, end));
            if cs >= end - EPS {
                // entirely after: just move forward
                let patch = ClipPatch { start_beats: Some((cs - len).max(0.0)), ..Default::default() };
                commands.push(Command::SetClip { track_id: track.id, clip_id: clip.id, patch });
                continue;
            }
            if ne - ns < 1e-6 {
                commands.push(Command::RemoveClip { track_id: track.id, clip_id: clip.id });
                continue;
            }
            match &clip.source {
                ClipSource::Notes { notes, .. } => {
                    commands.push(Command::SetClipNotes {
                        track_id: track.id,
                        clip_id: clip.id,
                        notes: map_notes(notes, cs, start, end),
                        length_beats: Some(ne - ns),
                    });
                    if (ns - cs).abs() > EPS {
                        let patch = ClipPatch { start_beats: Some(ns), ..Default::default() };
                        commands.push(Command::SetClip { track_id: track.id, clip_id: clip.id, patch });
                    }
                }
                ClipSource::Audio { asset_id, duration_secs, .. } => {
                    let bps = audio_beats_per_sec(&clip.source, bpm);
                    // deleted part in seconds of the source
                    let a = ((start.max(cs) - cs) / bps).clamp(0.0, *duration_secs);
                    let b = ((end.min(ce) - cs) / bps).clamp(0.0, *duration_secs);
                    let parts: Vec<(f64, f64)> = [(0.0, a), (b, *duration_secs)].into_iter().filter(|(x, y)| y - x > 1e-4).collect();
                    let keep = AudioKeep { asset_id: asset_id.clone(), parts };
                    let source = cut(clip, &keep).map_err(CoreError::Invalid)?;
                    // the same clip (same ID) with a new source at a new position
                    commands.push(Command::RemoveClip { track_id: track.id, clip_id: clip.id });
                    commands.push(Command::AddClip { track_id: track.id, clip: Clip { start_beats: ns, source, ..clip.clone() } });
                }
            }
        }
    }
    if commands.is_empty() {
        return Err(CoreError::Invalid(tr!("There's nothing to delete in or after the selected range")));
    }
    let bpb = project.beats_per_bar;
    let label = tr!("Delete time range (bars {from} – {to})", from = bar_beat(start, bpb), to = bar_beat(end, bpb));
    Ok(Command::Batch { label, commands })
}

/// Aligns notes to the grid ("align to bar"). Calculation is done in
/// project time (`clip_start` + note start), so that a clip that does not begin
/// on a bar line also sits cleanly on the bars. The start jumps to the
/// nearest grid point (never before the clip start); with `lengths` the end does
/// too, at least one grid step after the start. `only`: indices of the notes to
/// edit (None = all).
///
/// Result: notes in the order of `SetClipNotes` (start, pitch) and the
/// new indices of the edited notes. If two notes of the same pitch end up on
/// the same start, the longer one stays.
pub fn quantize_notes(notes: &[Note], only: Option<&[usize]>, clip_start: f64, grid: f64, lengths: bool) -> Result<(Vec<Note>, Vec<usize>), CoreError> {
    if !grid.is_finite() || grid < 1.0 / 64.0 - EPS {
        return Err(CoreError::Invalid(tr!("The grid must be at least 1/64 beat")));
    }
    let chosen = |i: usize| only.is_none_or(|o| o.contains(&i));
    let snap = |b: f64| (b / grid).round() * grid;
    let mut out: Vec<(Note, bool)> = notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if !chosen(i) {
                return (n.clone(), false);
            }
            let abs = clip_start + n.start_beats;
            let mut s = snap(abs);
            if s < clip_start - EPS {
                s += grid;
            }
            let mut m = n.clone();
            m.start_beats = (s - clip_start).max(0.0);
            if lengths {
                m.length_beats = snap(abs + n.length_beats).max(s + grid) - s;
            }
            (m, true)
        })
        .collect();
    out.sort_by(|(a, _), (b, _)| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
    let mut merged: Vec<(Note, bool)> = Vec::with_capacity(out.len());
    for (n, sel) in out {
        match merged.last_mut() {
            Some((m, msel)) if m.pitch == n.pitch && (m.start_beats - n.start_beats).abs() < EPS => {
                if n.length_beats > m.length_beats {
                    *m = n;
                }
                *msel |= sel;
            }
            _ => merged.push((n, sel)),
        }
    }
    let selected = merged.iter().enumerate().filter(|(_, (_, s))| *s).map(|(i, _)| i).collect();
    Ok((merged.into_iter().map(|(n, _)| n).collect(), selected))
}

/// Note value of a grid in words: "1/8" (with a quarter as the beat) or "0.3 beats".
pub fn grid_name(grid: f64, beat_unit: u32) -> String {
    let d = beat_unit as f64 / grid;
    if d >= 1.0 && (d - d.round()).abs() < 1e-6 {
        format!("1/{}", d.round() as i64)
    } else {
        // never exactly one beat (that would give "1/unit")
        tr!("{n} beats", n = klang_i18n::decimal(grid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Id, Session, Track, TrackKind};

    #[test]
    fn quantize_snaps_starts_and_optionally_lengths() {
        let notes: Vec<Note> = [(60, 0.1, 0.8), (62, 1.15, 0.3), (64, 1.6, 1.1), (65, 3.0, 0.5)].iter().map(|&(p, s, l)| Note::new(p, s, l, 90)).collect();
        // only the first three, eighth (0.5), clip from beat 4
        let (q, sel) = quantize_notes(&notes, Some(&[0, 1, 2]), 4.0, 0.5, false).unwrap();
        let v: Vec<(u8, f64, f64)> = q.iter().map(|n| (n.pitch, n.start_beats, n.length_beats)).collect();
        assert_eq!(v, vec![(60, 0.0, 0.8), (62, 1.0, 0.3), (64, 1.5, 1.1), (65, 3.0, 0.5)]);
        assert_eq!(sel, vec![0, 1, 2]);
        // with lengths: end on the grid, at least one grid step
        let (q, _) = quantize_notes(&notes, None, 4.0, 0.5, true).unwrap();
        let v: Vec<(u8, f64, f64)> = q.iter().map(|n| (n.pitch, n.start_beats, n.length_beats)).collect();
        assert_eq!(v, vec![(60, 0.0, 1.0), (62, 1.0, 0.5), (64, 1.5, 1.0), (65, 3.0, 0.5)]);
    }

    #[test]
    fn quantize_uses_project_time_and_never_leaves_the_clip() {
        // Clip starts at 4.2: note at 0.1 (project 4.3) → quarter grid 4.0 lies before the clip → 5.0
        let notes = vec![Note::new(60, 0.1, 0.5, 90), Note::new(60, 0.9, 0.5, 90)];
        let (q, sel) = quantize_notes(&notes, None, 4.2, 1.0, false).unwrap();
        // both land on project beat 5 → merged
        assert_eq!(q.len(), 1);
        assert!((q[0].start_beats - 0.8).abs() < 1e-9);
        assert_eq!(sel, vec![0]);
        assert!(quantize_notes(&notes, None, 0.0, 0.0, false).is_err());
        assert_eq!(grid_name(0.25, 4), "1/16");
        assert_eq!(grid_name(1.0, 4), "1/4");
        assert_eq!(grid_name(0.5, 8), "1/16");
        assert_eq!(grid_name(0.3, 4), "0.3 beats");
    }

    fn notes_clip(start: f64, len: f64, notes: &[(u8, f64, f64)]) -> Clip {
        Clip {
            id: Id::NEW,
            name: "N".into(),
            start_beats: start,
            source: ClipSource::Notes { length_beats: len, notes: notes.iter().map(|&(p, s, l)| Note::new(p, s, l, 100)).collect() },
        }
    }

    fn audio_clip(start: f64, secs: f64) -> Clip {
        // 120 BPM without warp: 2 beats per second
        Clip { id: Id::NEW, name: "A".into(), start_beats: start, source: ClipSource::Audio { asset_id: "a1".into(), source_bpm: 90.0, warp: false, duration_secs: secs } }
    }

    fn session(clips: Vec<(TrackKind, Vec<Clip>)>) -> Session {
        let mut s = Session::default();
        for (kind, cs) in clips {
            let mut t = Track::new(Id::NEW, "T", kind);
            t.clips = cs;
            s.execute(Command::AddTrack { track: t, index: None }, false).unwrap();
        }
        s
    }

    fn no_audio(_: &Clip, _: &AudioKeep) -> Result<ClipSource, String> {
        Err("kein Audio erwartet".into())
    }

    fn summary(c: &Clip) -> (f64, f64, Vec<(u8, f64, f64)>) {
        match &c.source {
            ClipSource::Notes { length_beats, notes } => (c.start_beats, *length_beats, notes.iter().map(|n| (n.pitch, n.start_beats, n.length_beats)).collect()),
            ClipSource::Audio { duration_secs, .. } => (c.start_beats, *duration_secs, vec![]),
        }
    }

    #[test]
    fn collapse_maps_range_to_a_point() {
        assert_eq!(collapse(1.0, 2.0, 4.0), 1.0);
        assert_eq!(collapse(3.0, 2.0, 4.0), 2.0);
        assert_eq!(collapse(4.0, 2.0, 4.0), 2.0);
        assert_eq!(collapse(7.0, 2.0, 4.0), 5.0);
    }

    #[test]
    fn notes_clips_shrink_and_later_clips_move_up() {
        // Track 1: clip 0–8 with notes; track 2: a clip entirely inside the range and one after it
        let mut s = session(vec![
            (TrackKind::Instrument, vec![notes_clip(0.0, 8.0, &[(60, 0.0, 1.0), (62, 2.5, 1.0), (64, 3.5, 2.0), (65, 5.0, 1.0), (67, 6.0, 1.0)])]),
            (TrackKind::Instrument, vec![notes_clip(3.0, 1.0, &[(50, 0.0, 1.0)]), notes_clip(8.0, 4.0, &[(52, 1.0, 1.0)])]),
        ]);
        let before = s.project.clone();
        let cmd = delete_range(&s.project, 2.0, 5.0, &mut no_audio).unwrap();
        assert_eq!(cmd.label(), "Delete time range (bars 1.3 – 2.2)");
        s.execute(cmd, false).unwrap();
        let t1 = &s.project.tracks[0].clips;
        // 62 inside the range: gone; 64 (3.5–5.5) extends beyond it: stays with the remaining 0.5 from beat 2;
        // 65 (starts at the end edge) moves to 2; 67 to 3
        assert_eq!(summary(&t1[0]), (0.0, 5.0, vec![(60, 0.0, 1.0), (64, 2.0, 0.5), (65, 2.0, 1.0), (67, 3.0, 1.0)]));
        let t2 = &s.project.tracks[1].clips;
        assert_eq!(t2.len(), 1, "Clip im Bereich ist weg");
        assert_eq!(summary(&t2[0]), (5.0, 4.0, vec![(52, 1.0, 1.0)]));
        // one undo step (a deleted clip comes back at the end of the list)
        assert!(s.undo().unwrap());
        let sorted = |p: &Project| {
            let mut t = p.tracks.clone();
            t.iter_mut().for_each(|t| t.clips.sort_by_key(|c| c.id));
            t
        };
        assert_eq!(sorted(&s.project), sorted(&before));
    }

    #[test]
    fn held_note_across_the_range_stays_one_note() {
        let mut s = session(vec![(TrackKind::Instrument, vec![notes_clip(0.0, 8.0, &[(60, 1.0, 6.0)])])]);
        s.execute(delete_range(&s.project, 2.0, 4.0, &mut no_audio).unwrap(), false).unwrap();
        assert_eq!(summary(&s.project.tracks[0].clips[0]), (0.0, 6.0, vec![(60, 1.0, 4.0)]));
    }

    #[test]
    fn clip_starting_inside_loses_its_head() {
        let mut s = session(vec![(TrackKind::Instrument, vec![notes_clip(3.0, 4.0, &[(60, 0.0, 1.0), (62, 2.0, 1.0)])])]);
        s.execute(delete_range(&s.project, 2.0, 4.0, &mut no_audio).unwrap(), false).unwrap();
        // Clip 3–7 → 2–5; note at 3 (inside the range) gone, note at 5 → 3 (relative 1)
        assert_eq!(summary(&s.project.tracks[0].clips[0]), (2.0, 3.0, vec![(62, 1.0, 1.0)]));
    }

    #[test]
    fn audio_clip_keeps_material_outside_the_range() {
        // Audio 4 s from beat 1 → beats 1–9 (120 BPM, without warp)
        let mut s = session(vec![(TrackKind::Audio, vec![audio_clip(1.0, 4.0), audio_clip(10.0, 1.0)])]);
        let first_id = s.project.tracks[0].clips[0].id;
        let mut seen = Vec::new();
        let mut cut = |c: &Clip, k: &AudioKeep| {
            seen.push((c.id, k.clone()));
            let dur: f64 = k.parts.iter().map(|(a, b)| b - a).sum();
            Ok(ClipSource::Audio { asset_id: "neu".into(), source_bpm: 90.0, warp: false, duration_secs: dur })
        };
        let cmd = delete_range(&s.project, 3.0, 5.0, &mut cut).unwrap();
        s.execute(cmd, false).unwrap();
        assert_eq!(seen, vec![(first_id, AudioKeep { asset_id: "a1".into(), parts: vec![(0.0, 1.0), (2.0, 4.0)] })]);
        let clips = &s.project.tracks[0].clips;
        let first = clips.iter().find(|c| c.id == first_id).expect("gleiche ID");
        assert_eq!(summary(first), (1.0, 3.0, vec![]));
        assert!(matches!(&first.source, ClipSource::Audio { asset_id, .. } if asset_id == "neu"));
        let second = clips.iter().find(|c| c.id != first_id).unwrap();
        assert_eq!(second.start_beats, 8.0, "rückt um 2 Schläge auf");
        s.undo().unwrap();
        let back = s.project.tracks[0].clips.iter().find(|c| c.id == first_id).unwrap();
        assert!(matches!(&back.source, ClipSource::Audio { asset_id, duration_secs, .. } if asset_id == "a1" && *duration_secs == 4.0));
    }

    #[test]
    fn warped_audio_uses_its_source_tempo() {
        let clip = Clip { id: Id::NEW, name: "W".into(), start_beats: 0.0, source: ClipSource::Audio { asset_id: "w".into(), source_bpm: 60.0, warp: true, duration_secs: 8.0 } };
        let s = session(vec![(TrackKind::Audio, vec![clip])]);
        let mut parts = None;
        let mut cut = |_: &Clip, k: &AudioKeep| {
            parts = Some(k.parts.clone());
            Ok(ClipSource::Audio { asset_id: "x".into(), source_bpm: 60.0, warp: true, duration_secs: 6.0 })
        };
        // 60 BPM source: one beat = one second
        delete_range(&s.project, 2.0, 4.0, &mut cut).unwrap();
        assert_eq!(parts, Some(vec![(0.0, 2.0), (4.0, 8.0)]));
    }

    #[test]
    fn nothing_to_delete_is_an_error() {
        let s = session(vec![(TrackKind::Instrument, vec![notes_clip(0.0, 2.0, &[(60, 0.0, 1.0)])])]);
        assert!(delete_range(&s.project, 4.0, 8.0, &mut no_audio).is_err());
        assert!(delete_range(&s.project, 3.0, 3.0, &mut no_audio).is_err());
    }
}
