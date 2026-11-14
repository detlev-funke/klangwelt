use crate::model::{Clip, ClipSource, Id, Instrument, Note, Project, Track, TrackKind, TrackView, TRACK_COLORS};
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
            CoreError::TrackNotFound(id) => write!(f, "Spur {id} nicht gefunden"),
            CoreError::ClipNotFound(id) => write!(f, "Clip {id} nicht gefunden"),
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
                return invalid("Der Projektname darf nicht leer sein");
            }
        }
        if let Some(t) = self.tempo_bpm {
            if !(20.0..=400.0).contains(&t) {
                return invalid(format!("Tempo {t} BPM liegt außerhalb von 20–400"));
            }
        }
        if let Some(b) = self.beats_per_bar {
            if !(1..=32).contains(&b) {
                return invalid("Schläge pro Takt müssen zwischen 1 und 32 liegen");
            }
        }
        if let Some(u) = self.beat_unit {
            if ![1, 2, 4, 8, 16, 32].contains(&u) {
                return invalid("Ungültige Notenwert-Einheit für die Taktart");
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
        let check = |v: Option<f32>, lo: f32, hi: f32, what: &str| -> Result<(), CoreError> {
            match v {
                Some(x) if !x.is_finite() || x < lo || x > hi => {
                    invalid(format!("{what} muss zwischen {lo} und {hi} liegen"))
                }
                _ => Ok(()),
            }
        };
        check(self.gain_db, -60.0, 12.0, "Lautstärke (dB)")?;
        check(self.pan, -1.0, 1.0, "Panorama")?;
        check(self.pitch_semitones, -24.0, 24.0, "Tonhöhe (Halbtöne)")?;
        for (v, what) in [(self.compact_height, "Höhe (kompakt)"), (self.notes_height, "Höhe (Notenansicht)")] {
            if let Some(h) = v {
                if !h.is_finite() || (h != 0.0 && !(40.0..=1200.0).contains(&h)) {
                    return invalid(format!("{what} muss 0 (Standard) oder 40–1200 Pixel sein"));
                }
            }
        }
        if let Some(n) = &self.name {
            if n.trim().is_empty() {
                return invalid("Der Spurname darf nicht leer sein");
            }
        }
        if let Some(i) = &self.instrument {
            if i.params.values().any(|v| !v.is_finite()) {
                return invalid("Instrumentparameter müssen endliche Zahlen sein");
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
                return invalid("Clipstart muss ≥ 0 sein");
            }
        }
        let is_audio = matches!(clip.source, ClipSource::Audio { .. });
        if let Some(b) = self.source_bpm {
            if !is_audio {
                return invalid("Quelltempo gibt es nur bei Audio-Clips");
            }
            if !(20.0..=400.0).contains(&b) {
                return invalid(format!("Quelltempo {b} BPM liegt außerhalb von 20–400"));
            }
        }
        if self.warp.is_some() && !is_audio {
            return invalid("Warp gibt es nur bei Audio-Clips");
        }
        Ok(())
    }
}

impl Command {
    /// Beschriftung für die Undo-Historie.
    pub fn label(&self) -> String {
        match self {
            Command::SetProject { patch } if patch.tempo_bpm.is_some() => "Tempo ändern",
            Command::SetProject { .. } => "Projekt ändern",
            Command::AddTrack { .. } => "Spur hinzufügen",
            Command::RemoveTrack { .. } => "Spur löschen",
            Command::MoveTrack { .. } => "Spur verschieben",
            Command::SetTrack { patch, .. } => {
                if patch.gain_db.is_some() { "Lautstärke ändern" }
                else if patch.pan.is_some() { "Panorama ändern" }
                else if patch.pitch_semitones.is_some() { "Tonhöhe ändern" }
                else if patch.mute.is_some() { "Stummschalten" }
                else if patch.solo.is_some() { "Solo" }
                else if patch.instrument.is_some() { "Klang ändern" }
                else if patch.name.is_some() { "Spur umbenennen" }
                else if patch.view.is_some() || patch.compact_height.is_some() || patch.notes_height.is_some() { "Ansicht ändern" }
                else { "Spur ändern" }
            }
            Command::AddClip { .. } => "Clip hinzufügen",
            Command::RemoveClip { .. } => "Clip löschen",
            Command::SetClip { patch, .. } => {
                if patch.start_beats.is_some() { "Clip verschieben" }
                else if patch.source_bpm.is_some() || patch.warp.is_some() { "Clip-Sync ändern" }
                else { "Clip ändern" }
            }
            Command::SetClipNotes { .. } => "Noten bearbeiten",
            Command::Batch { label, .. } => return label.clone(),
        }
        .to_string()
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
        return invalid("Zu viele Noten in einem Clip (max. 20 000)");
    }
    for (i, n) in notes.iter().enumerate() {
        if n.pitch > 127 {
            return invalid(format!("Note {}: Tonhöhe {} liegt außerhalb von 0–127", i + 1, n.pitch));
        }
        if !n.start_beats.is_finite() || n.start_beats < 0.0 {
            return invalid(format!("Note {}: Start muss ≥ 0 sein", i + 1));
        }
        if !n.length_beats.is_finite() || n.length_beats <= 0.0 {
            return invalid(format!("Note {}: Länge muss > 0 sein", i + 1));
        }
        if n.velocity == 0 || n.velocity > 127 {
            return invalid(format!("Note {}: Anschlagstärke muss zwischen 1 und 127 liegen", i + 1));
        }
        let range = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        if !range(n.vibrato, 0.0, 1.0) {
            return invalid(format!("Note {}: Vibrato muss zwischen 0 und 1 (Halbton) liegen", i + 1));
        }
        if !range(n.brightness, -1.0, 1.0) {
            return invalid(format!("Note {}: Helligkeit muss zwischen −1 und +1 liegen", i + 1));
        }
        if !range(n.bend, -12.0, 12.0) {
            return invalid(format!("Note {}: Biegung muss zwischen −12 und +12 Halbtönen liegen", i + 1));
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
                return invalid(format!("Spur-ID {} ist bereits vergeben", track.id));
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
                return invalid("Clipstart muss ≥ 0 sein");
            }
            if clip.id == Id::NEW {
                clip.id = project.alloc_id();
            } else if project.tracks.iter().flat_map(|t| &t.clips).any(|c| c.id == clip.id) {
                return invalid(format!("Clip-ID {} ist bereits vergeben", clip.id));
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
                    return invalid("Cliplänge muss > 0 sein");
                }
            }
            let track = project.track_mut(track_id).ok_or(CoreError::TrackNotFound(track_id))?;
            let clip = track.clips.iter_mut().find(|c| c.id == clip_id).ok_or(CoreError::ClipNotFound(clip_id))?;
            let ClipSource::Notes { length_beats: len, notes: current } = &mut clip.source else {
                return invalid("Noten gibt es nur in Instrument-Clips");
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
