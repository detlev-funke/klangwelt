use crate::model::{Clip, ClipSource, Id, Instrument, Note, Project, Track, TrackGroup, TrackKind, TrackView, TRACK_COLORS};
use klang_i18n::tr;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum CoreError {
    TrackNotFound(Id),
    ClipNotFound(Id),
    Invalid(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreError::TrackNotFound(id) => f.write_str(&tr!("Track {id} not found", id = id)),
            CoreError::ClipNotFound(id) => f.write_str(&tr!("Clip {id} not found", id = id)),
            CoreError::Invalid(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for CoreError {}

fn invalid<T>(msg: impl Into<String>) -> Result<T, CoreError> {
    Err(CoreError::Invalid(msg.into()))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo_bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beats_per_bar: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beat_unit: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pan: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solo: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_semitones: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<Instrument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<TrackView>,
    /// Row height compact (pixels, 0 = default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact_height: Option<f32>,
    /// Row height score view (pixels, 0 = default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes_height: Option<f32>,
    /// Group of the track (0 = none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<Id>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solo: Option<bool>,
    /// Folded or unfolded (presentation only, no undo step).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClipPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_beats: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warp: Option<bool>,
}

/// All changes to the project. JSON form e.g.
/// `{"type":"setTrack","trackId":3,"patch":{"gainDb":-6}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Command {
    SetProject { patch: ProjectPatch },
    AddTrack {
        track: Track,
        #[serde(default)]
        index: Option<usize>,
    },
    RemoveTrack { track_id: Id },
    MoveTrack { track_id: Id, index: usize },
    SetTrack { track_id: Id, patch: TrackPatch },
    AddClip { track_id: Id, clip: Clip },
    RemoveClip { track_id: Id, clip_id: Id },
    SetClip { track_id: Id, clip_id: Id, patch: ClipPatch },
    /// Replaces the notes of an instrument clip (optionally its length too).
    SetClipNotes {
        track_id: Id,
        clip_id: Id,
        notes: Vec<Note>,
        #[serde(default)]
        length_beats: Option<f64>,
    },
    /// A new track group (tracks join it with `setTrack` and `group`).
    AddGroup {
        group: TrackGroup,
        #[serde(default)]
        index: Option<usize>,
    },
    /// Removes a group; its tracks stay, without a group.
    RemoveGroup { group_id: Id },
    SetGroup { group_id: Id, patch: GroupPatch },
    Batch { label: String, commands: Vec<Command> },
}

impl ProjectPatch {
    fn fields(&self) -> String {
        let mut f = Vec::new();
        if self.name.is_some() { f.push("name") }
        if self.tempo_bpm.is_some() { f.push("tempo") }
        if self.beats_per_bar.is_some() { f.push("bpb") }
        if self.beat_unit.is_some() { f.push("unit") }
        f.join(",")
    }

    fn validate(&self) -> Result<(), CoreError> {
        if let Some(n) = &self.name {
            if n.trim().is_empty() {
                return invalid(tr!("The project name can't be empty"));
            }
        }
        if let Some(t) = self.tempo_bpm {
            if !(20.0..=400.0).contains(&t) {
                return invalid(tr!("Tempo {t} BPM is outside 20–400", t = t));
            }
        }
        if let Some(b) = self.beats_per_bar {
            if !(1..=32).contains(&b) {
                return invalid(tr!("Beats per bar must be between 1 and 32"));
            }
        }
        if let Some(u) = self.beat_unit {
            if ![1, 2, 4, 8, 16, 32].contains(&u) {
                return invalid(tr!("Invalid beat unit for the time signature"));
            }
        }
        Ok(())
    }
}

impl TrackPatch {
    fn fields(&self) -> String {
        let mut f = Vec::new();
        if self.name.is_some() { f.push("name") }
        if self.color.is_some() { f.push("color") }
        if self.gain_db.is_some() { f.push("gain") }
        if self.pan.is_some() { f.push("pan") }
        if self.mute.is_some() { f.push("mute") }
        if self.solo.is_some() { f.push("solo") }
        if self.pitch_semitones.is_some() { f.push("pitch") }
        if self.instrument.is_some() { f.push("instrument") }
        if self.view.is_some() { f.push("view") }
        if self.compact_height.is_some() { f.push("compactHeight") }
        if self.notes_height.is_some() { f.push("notesHeight") }
        if self.group.is_some() { f.push("group") }
        f.join(",")
    }

    /// Changes only the presentation - view and/or row heights (no undo step).
    pub fn is_view_only(&self) -> bool {
        let display = TrackPatch {
            view: self.view,
            compact_height: self.compact_height,
            notes_height: self.notes_height,
            ..Default::default()
        };
        *self != TrackPatch::default() && *self == display
    }

    fn validate(&self) -> Result<(), CoreError> {
        let outside = |v: Option<f32>, lo: f32, hi: f32| matches!(v, Some(x) if !x.is_finite() || x < lo || x > hi);
        let (lo, hi) = (-60.0f32, 12.0f32);
        if outside(self.gain_db, lo, hi) {
            return invalid(tr!("Volume (dB) must be between {lo} and {hi}", lo = lo, hi = hi));
        }
        let (lo, hi) = (-1.0f32, 1.0f32);
        if outside(self.pan, lo, hi) {
            return invalid(tr!("Pan must be between {lo} and {hi}", lo = lo, hi = hi));
        }
        let (lo, hi) = (-24.0f32, 24.0f32);
        if outside(self.pitch_semitones, lo, hi) {
            return invalid(tr!("Pitch (semitones) must be between {lo} and {hi}", lo = lo, hi = hi));
        }
        for (v, compact) in [(self.compact_height, true), (self.notes_height, false)] {
            if let Some(h) = v {
                if !h.is_finite() || (h != 0.0 && !(40.0..=1200.0).contains(&h)) {
                    return invalid(if compact {
                        tr!("Height (compact) must be 0 (default) or 40–1200 pixels")
                    } else {
                        tr!("Height (piano roll) must be 0 (default) or 40–1200 pixels")
                    });
                }
            }
        }
        if let Some(n) = &self.name {
            if n.trim().is_empty() {
                return invalid(tr!("The track name can't be empty"));
            }
        }
        if let Some(i) = &self.instrument {
            if i.params.values().any(|v| !v.is_finite()) {
                return invalid(tr!("Instrument parameters must be finite numbers"));
            }
        }
        Ok(())
    }
}

impl GroupPatch {
    fn fields(&self) -> String {
        let mut f = Vec::new();
        if self.name.is_some() { f.push("name") }
        if self.color.is_some() { f.push("color") }
        if self.gain_db.is_some() { f.push("gain") }
        if self.mute.is_some() { f.push("mute") }
        if self.solo.is_some() { f.push("solo") }
        if self.collapsed.is_some() { f.push("collapsed") }
        f.join(",")
    }

    /// Only folds or unfolds the group (no undo step).
    pub fn is_view_only(&self) -> bool {
        *self != GroupPatch::default() && *self == GroupPatch { collapsed: self.collapsed, ..Default::default() }
    }

    fn validate(&self) -> Result<(), CoreError> {
        if matches!(self.gain_db, Some(x) if !x.is_finite() || !(-60.0..=12.0).contains(&x)) {
            return invalid(tr!("Volume (dB) must be between {lo} and {hi}", lo = -60.0f32, hi = 12.0f32));
        }
        if let Some(n) = &self.name {
            if n.trim().is_empty() {
                return invalid(tr!("The group name can't be empty"));
            }
        }
        Ok(())
    }
}

fn group_missing<T>(id: Id) -> Result<T, CoreError> {
    invalid(tr!("Group {id} not found", id = id))
}

impl ClipPatch {
    fn fields(&self) -> String {
        let mut f = Vec::new();
        if self.name.is_some() { f.push("name") }
        if self.start_beats.is_some() { f.push("start") }
        if self.source_bpm.is_some() { f.push("bpm") }
        if self.warp.is_some() { f.push("warp") }
        f.join(",")
    }

    fn validate(&self, clip: &Clip) -> Result<(), CoreError> {
        if let Some(s) = self.start_beats {
            if !s.is_finite() || s < 0.0 {
                return invalid(tr!("Clip start must be ≥ 0"));
            }
        }
        let is_audio = matches!(clip.source, ClipSource::Audio { .. });
        if let Some(b) = self.source_bpm {
            if !is_audio {
                return invalid(tr!("Source tempo only exists for audio clips"));
            }
            if !(20.0..=400.0).contains(&b) {
                return invalid(tr!("Source tempo {b} BPM is outside 20–400", b = b));
            }
        }
        if self.warp.is_some() && !is_audio {
            return invalid(tr!("Warp only exists for audio clips"));
        }
        Ok(())
    }
}

impl Command {
    /// Label for the undo history.
    pub fn label(&self) -> String {
        match self {
            Command::SetProject { patch } if patch.tempo_bpm.is_some() => tr!("Change tempo"),
            Command::SetProject { .. } => tr!("Change project"),
            Command::AddTrack { .. } => tr!("Add track"),
            Command::RemoveTrack { .. } => tr!("Delete track"),
            Command::MoveTrack { .. } => tr!("Move track"),
            Command::SetTrack { patch, .. } => {
                if patch.gain_db.is_some() { tr!("Change volume") }
                else if patch.pan.is_some() { tr!("Change pan") }
                else if patch.pitch_semitones.is_some() { tr!("Change pitch") }
                else if patch.mute.is_some() { tr!("Mute") }
                else if patch.solo.is_some() { tr!("Solo") }
                else if patch.instrument.is_some() { tr!("Change sound") }
                else if patch.name.is_some() { tr!("Rename track") }
                else if patch.group.is_some() { tr!("Change group") }
                else if patch.view.is_some() || patch.compact_height.is_some() || patch.notes_height.is_some() { tr!("Change view") }
                else { tr!("Change track") }
            }
            Command::AddGroup { .. } => tr!("Add group"),
            Command::RemoveGroup { .. } => tr!("Ungroup"),
            Command::SetGroup { patch, .. } => {
                if patch.gain_db.is_some() { tr!("Change group volume") }
                else if patch.mute.is_some() { tr!("Mute group") }
                else if patch.solo.is_some() { tr!("Solo group") }
                else if patch.name.is_some() { tr!("Rename group") }
                else if patch.collapsed.is_some() { tr!("Change view") }
                else { tr!("Change group") }
            }
            Command::AddClip { .. } => tr!("Add clip"),
            Command::RemoveClip { .. } => tr!("Delete clip"),
            Command::SetClip { patch, .. } => {
                if patch.start_beats.is_some() { tr!("Move clip") }
                else if patch.source_bpm.is_some() || patch.warp.is_some() { tr!("Change clip sync") }
                else { tr!("Change clip") }
            }
            Command::SetClipNotes { .. } => tr!("Edit notes"),
            Command::Batch { label, .. } => label.clone(),
        }
    }

    /// Equal keys may merge into one undo step
    /// (e.g. many intermediate values while dragging a slider).
    pub fn merge_key(&self) -> Option<String> {
        match self {
            Command::SetProject { patch } => Some(format!("project:{}", patch.fields())),
            Command::SetTrack { track_id, patch } => Some(format!("track:{track_id}:{}", patch.fields())),
            Command::SetClip { track_id, clip_id, patch } => {
                Some(format!("clip:{track_id}:{clip_id}:{}", patch.fields()))
            }
            Command::SetClipNotes { track_id, clip_id, .. } => Some(format!("notes:{track_id}:{clip_id}")),
            Command::SetGroup { group_id, patch } => Some(format!("group:{group_id}:{}", patch.fields())),
            // a batch of exactly one command (only for its own label) merges like that command
            Command::Batch { commands, .. } if commands.len() == 1 => commands[0].merge_key(),
            // the same change on several tracks at once (a slider of several picked tracks)
            Command::Batch { commands, .. } if !commands.is_empty() => {
                commands.iter().map(Command::merge_key).collect::<Option<Vec<_>>>().map(|keys| keys.join("|"))
            }
            _ => None,
        }
    }

    /// Pure view change - is executed but not recorded in the undo history.
    pub fn is_view_only(&self) -> bool {
        match self {
            Command::SetTrack { patch, .. } => patch.is_view_only(),
            Command::SetGroup { patch, .. } => patch.is_view_only(),
            _ => false,
        }
    }

    /// Tracks the command touches (for the history's "where" column).
    /// Changes of the project itself (tempo, time signature) are not tracks.
    pub fn touched_tracks(&self, out: &mut Vec<Id>) {
        let id = match self {
            // a group's own settings (its tracks change with setTrack)
            Command::SetProject { .. } | Command::AddGroup { .. } | Command::RemoveGroup { .. } | Command::SetGroup { .. } => return,
            Command::AddTrack { track, .. } => track.id,
            Command::RemoveTrack { track_id }
            | Command::MoveTrack { track_id, .. }
            | Command::SetTrack { track_id, .. }
            | Command::AddClip { track_id, .. }
            | Command::RemoveClip { track_id, .. }
            | Command::SetClip { track_id, .. }
            | Command::SetClipNotes { track_id, .. } => *track_id,
            Command::Batch { commands, .. } => {
                for c in commands {
                    c.touched_tracks(out);
                }
                return;
            }
        };
        if !out.contains(&id) {
            out.push(id);
        }
    }

    /// Does the command change the project itself (name, tempo, time signature)?
    pub fn touches_project(&self) -> bool {
        match self {
            Command::SetProject { .. } => true,
            Command::Batch { commands, .. } => commands.iter().any(Command::touches_project),
            _ => false,
        }
    }

    /// Gives new tracks and clips (`Id::NEW`) their IDs before they are applied.
    /// The command then replays exactly the same way – the history needs that to go
    /// forward again from any earlier state.
    pub fn assign_ids(&mut self, project: &mut Project) {
        match self {
            Command::AddTrack { track, .. } => {
                if track.id == Id::NEW {
                    track.id = project.alloc_id();
                }
                for clip in &mut track.clips {
                    if clip.id == Id::NEW {
                        clip.id = project.alloc_id();
                    }
                }
            }
            Command::AddClip { clip, .. } => {
                if clip.id == Id::NEW {
                    clip.id = project.alloc_id();
                }
            }
            Command::AddGroup { group, .. } => {
                if group.id == Id::NEW {
                    group.id = project.alloc_id();
                }
            }
            Command::Batch { commands, .. } => {
                for c in commands {
                    c.assign_ids(project);
                }
            }
            _ => {}
        }
    }
}

fn validate_notes(notes: &[Note]) -> Result<(), CoreError> {
    if notes.len() > 20_000 {
        return invalid(tr!("Too many notes in one clip (max. 20,000)"));
    }
    for (i, n) in notes.iter().enumerate() {
        let nr = i + 1;
        if n.pitch > 127 {
            return invalid(tr!("Note {nr}: pitch {pitch} is outside 0–127", nr = nr, pitch = n.pitch));
        }
        if !n.start_beats.is_finite() || n.start_beats < 0.0 {
            return invalid(tr!("Note {nr}: start must be ≥ 0", nr = nr));
        }
        if !n.length_beats.is_finite() || n.length_beats <= 0.0 {
            return invalid(tr!("Note {nr}: length must be > 0", nr = nr));
        }
        if n.velocity == 0 || n.velocity > 127 {
            return invalid(tr!("Note {nr}: velocity must be between 1 and 127", nr = nr));
        }
        let range = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        if !range(n.vibrato, 0.0, 1.0) {
            return invalid(tr!("Note {nr}: vibrato must be between 0 and 1 (semitone)", nr = nr));
        }
        if !range(n.brightness, -1.0, 1.0) {
            return invalid(tr!("Note {nr}: brightness must be between −1 and +1", nr = nr));
        }
        if !range(n.bend, -12.0, 12.0) {
            return invalid(tr!("Note {nr}: bend must be between −12 and +12 semitones", nr = nr));
        }
    }
    Ok(())
}

macro_rules! swap_fields {
    ($target:expr, $patch:expr, $inv:expr; $($f:ident),*) => {
        $( if let Some(v) = $patch.$f { $inv.$f = Some(std::mem::replace(&mut $target.$f, v)); } )*
    };
}

/// Applies `cmd` and returns the command that undoes the change.
/// On an error the project remains unchanged.
pub fn apply(project: &mut Project, cmd: Command) -> Result<Command, CoreError> {
    match cmd {
        Command::SetProject { patch } => {
            patch.validate()?;
            let mut inv = ProjectPatch::default();
            swap_fields!(project, patch, inv; name, tempo_bpm, beats_per_bar, beat_unit);
            Ok(Command::SetProject { patch: inv })
        }

        Command::AddTrack { mut track, index } => {
            if track.id == Id::NEW {
                track.id = project.alloc_id();
            } else if project.track(track.id).is_some() {
                return invalid(tr!("Track ID {id} is already taken", id = track.id));
            } else {
                project.reserve_id(track.id);
            }
            for clip in &mut track.clips {
                if clip.id == Id::NEW {
                    clip.id = project.alloc_id();
                } else {
                    project.reserve_id(clip.id);
                }
            }
            if track.color.is_empty() {
                track.color = TRACK_COLORS[(track.id.0 as usize).saturating_sub(1) % TRACK_COLORS.len()].into();
            }
            if track.kind == TrackKind::Instrument && track.instrument.is_none() {
                track.instrument = Some(Instrument::pluck());
            }
            if track.group != Id::NEW && project.group(track.group).is_none() {
                return group_missing(track.group);
            }
            let idx = index.unwrap_or(project.tracks.len()).min(project.tracks.len());
            let id = track.id;
            project.tracks.insert(idx, track);
            Ok(Command::RemoveTrack { track_id: id })
        }

        Command::RemoveTrack { track_id } => {
            let idx = project.track_index(track_id).ok_or(CoreError::TrackNotFound(track_id))?;
            let track = project.tracks.remove(idx);
            Ok(Command::AddTrack { track, index: Some(idx) })
        }

        Command::MoveTrack { track_id, index } => {
            let from = project.track_index(track_id).ok_or(CoreError::TrackNotFound(track_id))?;
            let track = project.tracks.remove(from);
            let to = index.min(project.tracks.len());
            project.tracks.insert(to, track);
            Ok(Command::MoveTrack { track_id, index: from })
        }

        Command::SetTrack { track_id, patch } => {
            patch.validate()?;
            if let Some(g) = patch.group.filter(|g| *g != Id::NEW) {
                if project.group(g).is_none() {
                    return group_missing(g);
                }
            }
            let track = project.track_mut(track_id).ok_or(CoreError::TrackNotFound(track_id))?;
            let mut inv = TrackPatch::default();
            swap_fields!(track, patch, inv; name, color, gain_db, pan, mute, solo, pitch_semitones, view, compact_height, notes_height, group);
            if let Some(v) = patch.instrument {
                inv.instrument = Some(track.instrument.replace(v).unwrap_or_default());
            }
            Ok(Command::SetTrack { track_id, patch: inv })
        }

        Command::AddClip { track_id, mut clip } => {
            if project.track(track_id).is_none() {
                return Err(CoreError::TrackNotFound(track_id));
            }
            if !clip.start_beats.is_finite() || clip.start_beats < 0.0 {
                return invalid(tr!("Clip start must be ≥ 0"));
            }
            if clip.id == Id::NEW {
                clip.id = project.alloc_id();
            } else if project.tracks.iter().flat_map(|t| &t.clips).any(|c| c.id == clip.id) {
                return invalid(tr!("Clip ID {id} is already taken", id = clip.id));
            } else {
                project.reserve_id(clip.id);
            }
            let clip_id = clip.id;
            project.track_mut(track_id).expect("geprüft").clips.push(clip);
            Ok(Command::RemoveClip { track_id, clip_id })
        }

        Command::RemoveClip { track_id, clip_id } => {
            let track = project.track_mut(track_id).ok_or(CoreError::TrackNotFound(track_id))?;
            let idx = track.clips.iter().position(|c| c.id == clip_id).ok_or(CoreError::ClipNotFound(clip_id))?;
            let clip = track.clips.remove(idx);
            Ok(Command::AddClip { track_id, clip })
        }

        Command::SetClip { track_id, clip_id, patch } => {
            let track = project.track_mut(track_id).ok_or(CoreError::TrackNotFound(track_id))?;
            let clip = track.clips.iter_mut().find(|c| c.id == clip_id).ok_or(CoreError::ClipNotFound(clip_id))?;
            patch.validate(clip)?;
            let mut inv = ClipPatch::default();
            swap_fields!(clip, patch, inv; name, start_beats);
            if let ClipSource::Audio { source_bpm, warp, .. } = &mut clip.source {
                if let Some(v) = patch.source_bpm {
                    inv.source_bpm = Some(std::mem::replace(source_bpm, v));
                }
                if let Some(v) = patch.warp {
                    inv.warp = Some(std::mem::replace(warp, v));
                }
            }
            Ok(Command::SetClip { track_id, clip_id, patch: inv })
        }

        Command::SetClipNotes { track_id, clip_id, mut notes, length_beats } => {
            validate_notes(&notes)?;
            if let Some(l) = length_beats {
                if !l.is_finite() || l <= 0.0 {
                    return invalid(tr!("Clip length must be > 0"));
                }
            }
            let track = project.track_mut(track_id).ok_or(CoreError::TrackNotFound(track_id))?;
            let clip = track.clips.iter_mut().find(|c| c.id == clip_id).ok_or(CoreError::ClipNotFound(clip_id))?;
            let ClipSource::Notes { length_beats: len, notes: current } = &mut clip.source else {
                return invalid(tr!("Only instrument clips contain notes"));
            };
            notes.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
            let old_len = *len;
            if let Some(l) = length_beats {
                *len = l;
            }
            let old_notes = std::mem::replace(current, notes);
            Ok(Command::SetClipNotes { track_id, clip_id, notes: old_notes, length_beats: Some(old_len) })
        }

        Command::AddGroup { mut group, index } => {
            if group.id == Id::NEW {
                group.id = project.alloc_id();
            } else if project.group(group.id).is_some() || project.track(group.id).is_some() {
                return invalid(tr!("Group ID {id} is already taken", id = group.id));
            } else {
                project.reserve_id(group.id);
            }
            if group.name.trim().is_empty() {
                group.name = tr!("Group");
            }
            GroupPatch { gain_db: Some(group.gain_db), ..Default::default() }.validate()?;
            if group.color.is_empty() {
                group.color = TRACK_COLORS[(project.groups.len() + 1) % TRACK_COLORS.len()].into();
            }
            let idx = index.unwrap_or(project.groups.len()).min(project.groups.len());
            let id = group.id;
            project.groups.insert(idx, group);
            Ok(Command::RemoveGroup { group_id: id })
        }

        Command::RemoveGroup { group_id } => {
            let idx = project.groups.iter().position(|g| g.id == group_id).ok_or(CoreError::Invalid(tr!("Group {id} not found", id = group_id)))?;
            let group = project.groups.remove(idx);
            let label = tr!("Ungroup");
            // undo: the group first, then its tracks join it again
            let mut back = vec![Command::AddGroup { group, index: Some(idx) }];
            for t in project.tracks.iter_mut().filter(|t| t.group == group_id) {
                t.group = Id::NEW;
                back.push(Command::SetTrack { track_id: t.id, patch: TrackPatch { group: Some(group_id), ..Default::default() } });
            }
            Ok(if back.len() == 1 { back.remove(0) } else { Command::Batch { label, commands: back } })
        }

        Command::SetGroup { group_id, patch } => {
            patch.validate()?;
            let group = project.group_mut(group_id).ok_or(CoreError::Invalid(tr!("Group {id} not found", id = group_id)))?;
            let mut inv = GroupPatch::default();
            swap_fields!(group, patch, inv; name, color, gain_db, mute, solo, collapsed);
            Ok(Command::SetGroup { group_id, patch: inv })
        }

        Command::Batch { label, commands } => {
            let mut inverses = Vec::with_capacity(commands.len());
            for c in commands {
                match apply(project, c) {
                    Ok(inv) => inverses.push(inv),
                    Err(e) => {
                        for inv in inverses.into_iter().rev() {
                            let _ = apply(project, inv);
                        }
                        return Err(e);
                    }
                }
            }
            inverses.reverse();
            Ok(Command::Batch { label, commands: inverses })
        }
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::merge::commands_between;
    use crate::Session;

    fn session_with(names: &[&str]) -> (Session, Vec<Id>) {
        let mut s = Session::default();
        for n in names {
            s.execute(Command::AddTrack { track: Track::new(Id::NEW, *n, TrackKind::Instrument), index: None }, false).unwrap();
        }
        let ids = s.project.tracks.iter().map(|t| t.id).collect();
        (s, ids)
    }

    fn join(track_id: Id, group: Id) -> Command {
        Command::SetTrack { track_id, patch: TrackPatch { group: Some(group), ..Default::default() } }
    }

    fn group_up(s: &mut Session, name: &str, members: &[Id]) -> Id {
        let id = s.project.alloc_id();
        let mut commands = vec![Command::AddGroup { group: TrackGroup { id, name: name.into(), ..Default::default() }, index: None }];
        commands.extend(members.iter().map(|t| join(*t, id)));
        s.execute(Command::Batch { label: "Group tracks".into(), commands }, false).unwrap();
        id
    }

    fn audible(s: &Session) -> Vec<String> {
        s.project.audible_tracks().map(|t| t.name.clone()).collect()
    }

    #[test]
    fn group_level_mute_and_solo_act_on_its_tracks() {
        let (mut s, ids) = session_with(&["Violin 1", "Violin 2", "Viola"]);
        let g = group_up(&mut s, "Violins", &ids[..2]);
        let set = |patch: GroupPatch| Command::SetGroup { group_id: g, patch };
        s.execute(set(GroupPatch { gain_db: Some(-3.0), ..Default::default() }), false).unwrap();
        s.execute(Command::SetTrack { track_id: ids[1], patch: TrackPatch { gain_db: Some(-2.0), ..Default::default() } }, false).unwrap();
        let p = &s.project;
        assert_eq!(p.track_gain_db(p.track(ids[1]).unwrap()), -5.0, "the group's level adds to the track's");
        assert_eq!(p.track_gain_db(p.track(ids[2]).unwrap()), 0.0, "a track outside isn't touched");

        s.execute(set(GroupPatch { mute: Some(true), ..Default::default() }), false).unwrap();
        assert_eq!(audible(&s), ["Viola"]);
        assert_eq!(s.undo_label(), Some("Mute group"));
        s.execute(set(GroupPatch { mute: Some(false), solo: Some(true), ..Default::default() }), false).unwrap();
        assert_eq!(audible(&s), ["Violin 1", "Violin 2"]);
        // a track muted itself stays silent in a soloed group
        s.execute(Command::SetTrack { track_id: ids[0], patch: TrackPatch { mute: Some(true), ..Default::default() } }, false).unwrap();
        assert_eq!(audible(&s), ["Violin 2"]);
    }

    #[test]
    fn ungrouping_keeps_the_tracks_and_undo_brings_the_group_back() {
        let (mut s, ids) = session_with(&["Cello 1", "Cello 2"]);
        let g = group_up(&mut s, "Celli", &ids);
        s.execute(Command::RemoveGroup { group_id: g }, false).unwrap();
        assert!(s.project.groups.is_empty());
        assert_eq!(s.project.tracks.len(), 2);
        assert!(s.project.tracks.iter().all(|t| t.group == Id::NEW));
        assert_eq!(s.undo_label(), Some("Ungroup"));
        s.undo().unwrap();
        assert_eq!(s.project.groups.len(), 1);
        assert_eq!(s.project.group_tracks(g).count(), 2);
        s.redo().unwrap();
        assert!(s.project.groups.is_empty() && s.project.tracks.iter().all(|t| t.group == Id::NEW));
        s.undo().unwrap();
        s.undo().unwrap();
        assert!(s.project.groups.is_empty(), "grouping undone");
    }

    #[test]
    fn folding_is_no_undo_step_and_unknown_groups_are_refused() {
        let (mut s, ids) = session_with(&["A"]);
        let g = group_up(&mut s, "G", &ids);
        s.execute(Command::SetGroup { group_id: g, patch: GroupPatch { collapsed: Some(true), ..Default::default() } }, false).unwrap();
        assert!(s.project.groups[0].collapsed);
        assert_eq!(s.undo_label(), Some("Group tracks"));
        let e = s.execute(join(ids[0], Id(999)), false).unwrap_err();
        assert!(e.to_string().contains("999"), "{e}");
        let e = s.execute(Command::SetGroup { group_id: g, patch: GroupPatch { gain_db: Some(40.0), ..Default::default() } }, false).unwrap_err();
        assert!(e.to_string().contains("dB"), "{e}");
    }

    #[test]
    fn projects_without_groups_stay_as_they_were_in_json() {
        let (mut s, ids) = session_with(&["A"]);
        let json = s.project.to_json();
        assert!(!json.contains("\"groups\"") && !json.contains("\"group\""), "{json}");
        let g = group_up(&mut s, "G", &ids);
        let back = Project::from_json(&s.project.to_json()).unwrap();
        assert_eq!(back, s.project);
        assert_eq!(back.tracks[0].group, g);
    }

    #[test]
    fn commands_between_carry_groups_along() {
        let (mut s, ids) = session_with(&["A", "B", "C"]);
        let from = s.project.clone();
        group_up(&mut s, "AB", &ids[..2]);
        let g = s.project.groups[0].id;
        s.execute(Command::SetGroup { group_id: g, patch: GroupPatch { gain_db: Some(-6.0), ..Default::default() } }, false).unwrap();
        let to = s.project.clone();
        let mut p = from.clone();
        apply(&mut p, Command::Batch { label: "x".into(), commands: commands_between(&from, &to) }).unwrap();
        assert_eq!((p.groups.clone(), p.tracks.clone()), (to.groups.clone(), to.tracks.clone()));
        let mut back = to.clone();
        apply(&mut back, Command::Batch { label: "y".into(), commands: commands_between(&to, &from) }).unwrap();
        assert_eq!((back.groups, back.tracks), (from.groups, from.tracks));
    }
}
