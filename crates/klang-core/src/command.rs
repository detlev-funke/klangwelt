use crate::model::{Clip, ClipSource, Id, Instrument, Note, Project, Track, TrackKind, TrackView, TRACK_COLORS};
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
    /// Zeilenhöhe kompakt (Pixel, 0 = Standard).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact_height: Option<f32>,
    /// Zeilenhöhe Notenansicht (Pixel, 0 = Standard).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes_height: Option<f32>,
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

/// Alle Änderungen am Projekt. JSON-Form z. B.
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
    /// Ersetzt die Noten eines Instrument-Clips (optional auch seine Länge).
    SetClipNotes {
        track_id: Id,
        clip_id: Id,
        notes: Vec<Note>,
        #[serde(default)]
        length_beats: Option<f64>,
    },
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
        f.join(",")
    }

    /// Ändert nur die Darstellung – Ansicht und/oder Zeilenhöhen (kein Undo-Schritt).
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
    /// Beschriftung für die Undo-Historie.
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
                else if patch.view.is_some() || patch.compact_height.is_some() || patch.notes_height.is_some() { tr!("Change view") }
                else { tr!("Change track") }
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

    /// Gleiche Schlüssel dürfen zu einem Undo-Schritt verschmelzen
    /// (z. B. viele Zwischenwerte beim Ziehen eines Reglers).
    pub fn merge_key(&self) -> Option<String> {
        match self {
            Command::SetProject { patch } => Some(format!("project:{}", patch.fields())),
            Command::SetTrack { track_id, patch } => Some(format!("track:{track_id}:{}", patch.fields())),
            Command::SetClip { track_id, clip_id, patch } => {
                Some(format!("clip:{track_id}:{clip_id}:{}", patch.fields()))
            }
            Command::SetClipNotes { track_id, clip_id, .. } => Some(format!("notes:{track_id}:{clip_id}")),
            // ein Stapel aus genau einem Befehl (nur für eine eigene Beschriftung) verschmilzt wie dieser
            Command::Batch { commands, .. } if commands.len() == 1 => commands[0].merge_key(),
            _ => None,
        }
    }

    /// Reine Ansichtsänderung – wird ausgeführt, aber nicht in die Undo-Historie aufgenommen.
    pub fn is_view_only(&self) -> bool {
        matches!(self, Command::SetTrack { patch, .. } if patch.is_view_only())
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

/// Wendet `cmd` an und liefert den Befehl, der die Änderung rückgängig macht.
/// Bei einem Fehler bleibt das Projekt unverändert.
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
            let track = project.track_mut(track_id).ok_or(CoreError::TrackNotFound(track_id))?;
            let mut inv = TrackPatch::default();
            swap_fields!(track, patch, inv; name, color, gain_db, pan, mute, solo, pitch_semitones, view, compact_height, notes_height);
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
