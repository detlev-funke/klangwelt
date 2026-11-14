use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Stable ID for tracks and clips. `Id(0)` in commands means "please assign".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub u64);

impl Id {
    pub const NEW: Id = Id(0);
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub const TRACK_COLORS: [&str; 8] = [
    "#ffb454", "#6ee7d8", "#a78bfa", "#f472b6", "#60a5fa", "#4ade80", "#fb923c", "#facc15",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    pub tempo_bpm: f64,
    pub beats_per_bar: u32,
    pub beat_unit: u32,
    pub tracks: Vec<Track>,
    pub next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            name: "Neues Projekt".into(),
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            beat_unit: 4,
            tracks: Vec::new(),
            next_id: 1,
        }
    }
}

impl Project {
    pub fn alloc_id(&mut self) -> Id {
        let id = Id(self.next_id);
        self.next_id += 1;
        id
    }

    pub(crate) fn reserve_id(&mut self, id: Id) {
        self.next_id = self.next_id.max(id.0 + 1);
    }

    pub fn seconds_per_beat(&self) -> f64 {
        60.0 / self.tempo_bpm
    }

    pub fn track(&self, id: Id) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: Id) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn track_index(&self, id: Id) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    pub fn clip(&self, track_id: Id, clip_id: Id) -> Option<&Clip> {
        self.track(track_id)?.clips.iter().find(|c| c.id == clip_id)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Projekt ist immer serialisierbar")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    #[default]
    Audio,
    Instrument,
}

/// Presentation of a track in the timeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackView {
    /// Flat row: waveform or hinted notes.
    #[default]
    Compact,
    /// Tall row with pitch axis (piano roll); editable for instrument tracks.
    Notes,
}

/// Kind of sound generator of an instrument track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstrumentKind {
    /// Plucked instrument (guitar, harp, fingered bass)
    #[default]
    Pluck,
    /// Drums: each key is a drum
    Drums,
    /// Bass synthesizer
    Bass,
    /// Pipe organ (stops 16′ … mixture)
    Organ,
    /// Harpsichord (8′/4′ choir, lute stop)
    Harpsichord,
    /// Synthesizer for pads and long sounds: oscillators with detuning,
    /// filter and drawn curves (volume, brightness, pulse)
    Synth,
}

/// Sound generator of an instrument track. The parameters have descriptive names
/// (e.g. "helligkeit", "kick.grundton") so that humans and the agent
/// understand them equally well.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instrument {
    #[serde(default)]
    pub kind: InstrumentKind,
    pub preset: String,
    pub params: BTreeMap<String, f32>,
    /// Drawn curves over time (synth: "laut", "hell"): points
    /// [seconds from note start, value 0…1], sorted by time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub curves: BTreeMap<String, Vec<[f32; 2]>>,
}

impl Instrument {
    pub fn pluck() -> Self {
        let params = [("helligkeit", 0.55), ("abklingen", 0.5), ("koerper", 0.4), ("anschlag", 0.5)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        Self { kind: InstrumentKind::Pluck, preset: "zupfer".into(), params, curves: BTreeMap::new() }
    }

    pub fn param(&self, name: &str, default: f32) -> f32 {
        self.params.get(name).copied().unwrap_or(default)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Track {
    pub id: Id,
    pub name: String,
    pub kind: TrackKind,
    pub color: String,
    pub gain_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub pitch_semitones: f32,
    pub instrument: Option<Instrument>,
    pub view: TrackView,
    /// Row height in the compact view (pixels, 0 = default).
    pub compact_height: f32,
    /// Row height in the score view (pixels, 0 = default).
    pub notes_height: f32,
    pub clips: Vec<Clip>,
}

impl Default for Track {
    fn default() -> Self {
        Self {
            id: Id::NEW,
            name: "Spur".into(),
            kind: TrackKind::Audio,
            color: String::new(),
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            pitch_semitones: 0.0,
            instrument: None,
            view: TrackView::Compact,
            compact_height: 0.0,
            notes_height: 0.0,
            clips: Vec::new(),
        }
    }
}

impl Track {
    /// Instrument tracks start in the score view, audio tracks compact.
    pub fn new(id: Id, name: impl Into<String>, kind: TrackKind) -> Self {
        let instrument = kind == TrackKind::Instrument;
        Self {
            id,
            name: name.into(),
            kind,
            instrument: instrument.then(Instrument::pluck),
            view: if instrument { TrackView::Notes } else { TrackView::Compact },
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: Id,
    pub name: String,
    pub start_beats: f64,
    pub source: ClipSource,
}

impl Clip {
    /// Length in beats at the current project tempo.
    pub fn length_beats(&self, project_bpm: f64) -> f64 {
        match &self.source {
            ClipSource::Audio { source_bpm, warp, duration_secs, .. } => {
                let bpm = if *warp { *source_bpm } else { project_bpm };
                duration_secs * bpm / 60.0
            }
            ClipSource::Notes { length_beats, .. } => *length_beats,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClipSource {
    /// Audio material. With `warp` it is stretched from the source tempo to the project tempo
    /// - this keeps rhythms of different tracks in sync.
    Audio {
        asset_id: String,
        source_bpm: f64,
        warp: bool,
        duration_secs: f64,
    },
    /// Notes in beats relative to the clip start.
    Notes { length_beats: f64, notes: Vec<Note> },
}

/// A note with expression. The expression values are 0 = neutral and are
/// then omitted from the JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub pitch: u8,
    pub start_beats: f64,
    pub length_beats: f64,
    /// Velocity 1–127.
    pub velocity: u8,
    /// Vibrato depth in semitones (0 = none, ≈ 0.3 as in singing, max. 1).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub vibrato: f32,
    /// Brightness relative to the instrument's sound (−1 darker … +1 brighter).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub brightness: f32,
    /// Scoop: the note starts this many semitones off and glides
    /// into pitch (−12 … +12; negative = from below).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bend: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl Note {
    /// Note without particular expression.
    pub fn new(pitch: u8, start_beats: f64, length_beats: f64, velocity: u8) -> Self {
        Self { pitch, start_beats, length_beats, velocity, ..Default::default() }
    }
}
