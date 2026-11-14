use crate::command::{apply, Command, CoreError};
use crate::model::Project;

#[derive(Clone, Debug)]
struct Entry {
    label: String,
    inverse: Command,
    merge_key: Option<String>,
}

/// An open project together with its undo/redo history.
#[derive(Clone, Debug)]
pub struct Session {
    pub project: Project,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    limit: usize,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Project::default())
    }
}

impl Session {
    pub fn new(project: Project) -> Self {
        Self { project, undo: Vec::new(), redo: Vec::new(), limit: 500 }
    }

    /// Runs a command. With `merge = true` it merges with the previous
    /// step if both affect the same field (control gestures).
    pub fn execute(&mut self, cmd: Command, merge: bool) -> Result<(), CoreError> {
        if cmd.is_view_only() {
            apply(&mut self.project, cmd)?;
            return Ok(());
        }
        let label = cmd.label();
        let key = cmd.merge_key();
        let inverse = apply(&mut self.project, cmd)?;
        self.redo.clear();
        if merge {
            if let (Some(k), Some(last)) = (&key, self.undo.last()) {
                if last.merge_key.as_ref() == Some(k) {
                    return Ok(());
                }
            }
        }
        self.undo.push(Entry { label, inverse, merge_key: key });
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool, CoreError> {
        let Some(entry) = self.undo.pop() else {
            return Ok(false);
        };
        match apply(&mut self.project, entry.inverse.clone()) {
            Ok(redo) => {
                self.redo.push(Entry { label: entry.label, inverse: redo, merge_key: None });
                Ok(true)
            }
            Err(e) => {
                self.undo.push(entry);
                Err(e)
            }
        }
    }

    pub fn redo(&mut self) -> Result<bool, CoreError> {
        let Some(entry) = self.redo.pop() else {
            return Ok(false);
        };
        match apply(&mut self.project, entry.inverse.clone()) {
            Ok(undo) => {
                self.undo.push(Entry { label: entry.label, inverse: undo, merge_key: None });
                Ok(true)
            }
            Err(e) => {
                self.redo.push(entry);
                Err(e)
            }
        }
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClipPatch, ClipSource, Clip, Id, Note, ProjectPatch, Track, TrackKind, TrackPatch, TrackView};

    fn audio_clip(start: f64) -> Clip {
        Clip {
            id: Id::NEW,
            name: "Loop".into(),
            start_beats: start,
            source: ClipSource::Audio { asset_id: "a1".into(), source_bpm: 100.0, warp: true, duration_secs: 2.4 },
        }
    }

    #[test]
    fn add_undo_redo_roundtrip() {
        let mut s = Session::default();
        let before = s.project.clone();
        s.execute(Command::AddTrack { track: Track::new(Id::NEW, "Beat", TrackKind::Audio), index: None }, false).unwrap();
        let tid = s.project.tracks[0].id;
        s.execute(Command::AddClip { track_id: tid, clip: audio_clip(4.0) }, false).unwrap();
        let after = s.project.clone();
        assert_eq!(s.undo_label(), Some("Add clip"));

        assert!(s.undo().unwrap());
        assert!(s.undo().unwrap());
        assert_eq!(s.project.tracks, before.tracks);
        assert!(!s.undo().unwrap());

        assert!(s.redo().unwrap());
        assert!(s.redo().unwrap());
        assert_eq!(s.project.tracks, after.tracks);
    }

    #[test]
    fn slider_gesture_merges_into_one_step() {
        let mut s = Session::default();
        s.execute(Command::AddTrack { track: Track::new(Id::NEW, "Bass", TrackKind::Audio), index: None }, false).unwrap();
        let tid = s.project.tracks[0].id;
        for (i, db) in [-1.0, -2.0, -3.0, -6.0].into_iter().enumerate() {
            let patch = TrackPatch { gain_db: Some(db), ..Default::default() };
            s.execute(Command::SetTrack { track_id: tid, patch }, i > 0).unwrap();
        }
        assert_eq!(s.project.tracks[0].gain_db, -6.0);
        s.undo().unwrap();
        assert_eq!(s.project.tracks[0].gain_db, 0.0);
        assert_eq!(s.undo_label(), Some("Add track"));
        // the same with its own label (batch of one command)
        for (i, db) in [-1.0, -4.0].into_iter().enumerate() {
            let patch = TrackPatch { gain_db: Some(db), ..Default::default() };
            let one = Command::Batch { label: "Pegel ziehen".into(), commands: vec![Command::SetTrack { track_id: tid, patch }] };
            s.execute(one, i > 0).unwrap();
        }
        assert_eq!(s.undo_label(), Some("Pegel ziehen"));
        s.undo().unwrap();
        assert_eq!(s.project.tracks[0].gain_db, 0.0, "ein Schritt");
    }

    #[test]
    fn invalid_values_leave_project_untouched() {
        let mut s = Session::default();
        let err = s.execute(Command::SetProject { patch: ProjectPatch { tempo_bpm: Some(1000.0), ..Default::default() } }, false);
        assert!(err.is_err());
        assert_eq!(s.project.tempo_bpm, 120.0);
        assert!(s.undo_label().is_none());
    }

    #[test]
    fn failing_batch_rolls_back() {
        let mut s = Session::default();
        let batch = Command::Batch {
            label: "Test".into(),
            commands: vec![
                Command::AddTrack { track: Track::new(Id::NEW, "A", TrackKind::Audio), index: None },
                Command::RemoveTrack { track_id: Id(999) },
            ],
        };
        assert!(s.execute(batch, false).is_err());
        assert!(s.project.tracks.is_empty());
    }

    #[test]
    fn clip_sync_settings_and_length() {
        let mut s = Session::default();
        s.execute(Command::AddTrack { track: Track::new(Id::NEW, "Loop", TrackKind::Audio), index: None }, false).unwrap();
        let tid = s.project.tracks[0].id;
        s.execute(Command::AddClip { track_id: tid, clip: audio_clip(0.0) }, false).unwrap();
        let cid = s.project.tracks[0].clips[0].id;
        // 2.4 s at 100 BPM = 4 beats – independent of the project tempo.
        assert!((s.project.tracks[0].clips[0].length_beats(140.0) - 4.0).abs() < 1e-9);
        s.execute(Command::SetClip { track_id: tid, clip_id: cid, patch: ClipPatch { warp: Some(false), ..Default::default() } }, false).unwrap();
        assert!((s.project.tracks[0].clips[0].length_beats(120.0) - 4.8).abs() < 1e-9);
    }

    fn notes_clip_session() -> (Session, Id, Id) {
        let mut s = Session::default();
        let mut t = Track::new(Id::NEW, "Lead", TrackKind::Instrument);
        t.clips.push(Clip {
            id: Id::NEW,
            name: "Thema".into(),
            start_beats: 0.0,
            source: ClipSource::Notes { length_beats: 4.0, notes: vec![] },
        });
        s.execute(Command::AddTrack { track: t, index: None }, false).unwrap();
        let (tid, cid) = (s.project.tracks[0].id, s.project.tracks[0].clips[0].id);
        (s, tid, cid)
    }

    fn note(pitch: u8, start: f64, len: f64) -> Note {
        Note::new(pitch, start, len, 100)
    }

    #[test]
    fn note_expression_is_compact_and_validated() {
        // neutral expression values are missing from the JSON, set ones are there
        let plain = serde_json::to_value(note(60, 0.0, 1.0)).unwrap();
        assert!(plain.get("vibrato").is_none() && plain.get("bend").is_none());
        let expressive = Note { vibrato: 0.3, bend: -2.0, ..note(60, 0.0, 1.0) };
        let v = serde_json::to_value(&expressive).unwrap();
        assert_eq!(v["bend"], -2.0);
        let back: Note = serde_json::from_value(serde_json::json!({ "pitch": 60, "startBeats": 0, "lengthBeats": 1, "velocity": 90 })).unwrap();
        assert_eq!(back.vibrato, 0.0);
        // outside the limits → rejected
        assert!(validate_test(&[Note { vibrato: 1.5, ..note(60, 0.0, 1.0) }]).is_err());
        assert!(validate_test(&[Note { bend: -13.0, ..note(60, 0.0, 1.0) }]).is_err());
        assert!(validate_test(&[Note { brightness: 0.5, ..note(60, 0.0, 1.0) }]).is_ok());
    }

    fn validate_test(notes: &[Note]) -> Result<(), crate::CoreError> {
        let (mut s, tid, cid) = notes_clip_session();
        s.execute(Command::SetClipNotes { track_id: tid, clip_id: cid, notes: notes.to_vec(), length_beats: None }, false).map(|_| ())
    }

    fn clip_notes(s: &Session) -> (Vec<Note>, f64) {
        match &s.project.tracks[0].clips[0].source {
            ClipSource::Notes { notes, length_beats } => (notes.clone(), *length_beats),
            _ => unreachable!(),
        }
    }

    #[test]
    fn set_clip_notes_sorts_validates_and_undoes() {
        let (mut s, tid, cid) = notes_clip_session();
        let cmd = Command::SetClipNotes { track_id: tid, clip_id: cid, notes: vec![note(67, 2.0, 1.0), note(60, 0.0, 1.0), note(64, 0.0, 0.5)], length_beats: Some(8.0) };
        s.execute(cmd, false).unwrap();
        let (notes, len) = clip_notes(&s);
        assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 64, 67]);
        assert_eq!(len, 8.0);
        assert_eq!(s.undo_label(), Some("Edit notes"));

        let bad = Command::SetClipNotes { track_id: tid, clip_id: cid, notes: vec![note(60, -1.0, 1.0)], length_beats: None };
        assert!(s.execute(bad, false).is_err());
        assert_eq!(clip_notes(&s).0.len(), 3);

        s.undo().unwrap();
        assert_eq!(clip_notes(&s), (vec![], 4.0));
        s.redo().unwrap();
        assert_eq!(clip_notes(&s).1, 8.0);
    }

    #[test]
    fn view_switch_is_not_an_undo_step() {
        let (mut s, tid, _) = notes_clip_session();
        assert_eq!(s.project.tracks[0].view, TrackView::Notes, "Instrumentspuren starten in der Notenansicht");
        let before = s.undo_label().map(str::to_string);
        let view = |v| Command::SetTrack { track_id: tid, patch: TrackPatch { view: Some(v), ..Default::default() } };
        s.execute(view(TrackView::Compact), false).unwrap();
        assert_eq!(s.project.tracks[0].view, TrackView::Compact);
        assert_eq!(s.undo_label().map(str::to_string), before);
        // A view together with another change, on the other hand, is a normal step.
        let mixed = Command::SetTrack { track_id: tid, patch: TrackPatch { view: Some(TrackView::Notes), gain_db: Some(-3.0), ..Default::default() } };
        s.execute(mixed, false).unwrap();
        assert_eq!(s.undo_label(), Some("Change volume"));
        let json: Command = serde_json::from_str(r#"{"type":"setTrack","trackId":1,"patch":{"view":"notes"}}"#).unwrap();
        assert!(json.is_view_only());

        // Dragging the row height: likewise no undo step, limits are checked.
        let height = Command::SetTrack { track_id: tid, patch: TrackPatch { notes_height: Some(420.0), ..Default::default() } };
        assert!(height.is_view_only());
        s.execute(height, false).unwrap();
        assert_eq!(s.project.tracks[0].notes_height, 420.0);
        assert_eq!(s.undo_label(), Some("Change volume"));
        let too_small = Command::SetTrack { track_id: tid, patch: TrackPatch { compact_height: Some(5.0), ..Default::default() } };
        assert!(s.execute(too_small, false).is_err());
        let reset = Command::SetTrack { track_id: tid, patch: TrackPatch { notes_height: Some(0.0), ..Default::default() } };
        s.execute(reset, false).unwrap();
        assert_eq!(s.project.tracks[0].notes_height, 0.0);
        assert!(!Command::SetTrack { track_id: tid, patch: TrackPatch::default() }.is_view_only());
    }

    #[test]
    fn json_shape_for_ui_and_agent() {
        let json = r#"{"type":"setTrack","trackId":3,"patch":{"gainDb":-6.5,"mute":true}}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        assert_eq!(
            cmd,
            Command::SetTrack { track_id: Id(3), patch: TrackPatch { gain_db: Some(-6.5), mute: Some(true), ..Default::default() } }
        );
        let add: Command = serde_json::from_str(r#"{"type":"addTrack","track":{"id":0,"name":"Neu","kind":"instrument"}}"#).unwrap();
        let mut s = Session::default();
        s.execute(add, false).unwrap();
        assert!(s.project.tracks[0].instrument.is_some());
        assert!(!s.project.tracks[0].color.is_empty());
        let round = Project::from_json(&s.project.to_json()).unwrap();
        assert_eq!(round, s.project);
    }
}
