use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Stabile ID für Spuren und Clips. `Id(0)` bedeutet in Befehlen „bitte vergeben“.
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

/// Darstellung einer Spur in der Zeitleiste.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackView {
    /// Flache Zeile: Wellenform bzw. angedeutete Noten.
    #[default]
    Compact,
    /// Hohe Zeile mit Tonhöhenachse (Pianorolle); bei Instrumentspuren bearbeitbar.
    Notes,
}

/// Art des Klangerzeugers einer Instrumentspur.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstrumentKind {
    /// Zupf-Instrument (Gitarre, Harfe, Fingerbass)
    #[default]
    Pluck,
    /// Schlagzeug: jede Taste eine Trommel
    Drums,
    /// Bass-Synthesizer
    Bass,
    /// Pfeifenorgel (Register 16′ … Mixtur)
    Organ,
    /// Cembalo (8′/4′-Chor, Lautenzug)
    Harpsichord,
    /// Synthesizer für Flächen und lange Klänge: Oszillatoren mit Verstimmung,
    /// Filter und gezeichnetem Verlauf (Lautstärke, Helligkeit, Puls)
    Synth,
}

/// Klangerzeuger einer Instrumentspur. Die Parameter tragen sprechende Namen
/// (z. B. „helligkeit“, „kick.grundton“), damit Menschen und Agent sie
/// gleichermaßen verstehen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instrument {
    #[serde(default)]
    pub kind: InstrumentKind,
    pub preset: String,
    pub params: BTreeMap<String, f32>,
    /// Gezeichnete Verläufe über die Zeit (Synth: „laut“, „hell“): Punkte
    /// [Sekunden ab Tonbeginn, Wert 0…1], nach der Zeit sortiert.
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
    /// Zeilenhöhe in der kompakten Ansicht (Pixel, 0 = Standard).
    pub compact_height: f32,
    /// Zeilenhöhe in der Notenansicht (Pixel, 0 = Standard).
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
    /// Instrumentspuren starten in der Notenansicht, Audiospuren kompakt.
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
    /// Länge in Schlägen beim aktuellen Projekttempo.
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
    /// Audiomaterial. Mit `warp` wird es vom Quelltempo auf das Projekttempo
    /// gedehnt – so laufen Rhythmen verschiedener Spuren synchron.
    Audio {
        asset_id: String,
        source_bpm: f64,
        warp: bool,
        duration_secs: f64,
    },
    /// Noten in Schlägen relativ zum Clipanfang.
    Notes { length_beats: f64, notes: Vec<Note> },
}

/// Eine Note mit Ausdruck. Die Ausdruckswerte sind 0 = neutral und werden
/// dann im JSON weggelassen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub pitch: u8,
    pub start_beats: f64,
    pub length_beats: f64,
    /// Anschlagstärke 1–127.
    pub velocity: u8,
    /// Vibrato-Tiefe in Halbtönen (0 = keins, ≈ 0,3 wie beim Singen, max. 1).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub vibrato: f32,
    /// Helligkeit relativ zum Klang des Instruments (−1 dunkler … +1 heller).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub brightness: f32,
    /// Anschleifen: Der Ton beginnt so viele Halbtöne versetzt und gleitet
    /// hinein (−12 … +12; negativ = von unten).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bend: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl Note {
    /// Note ohne besonderen Ausdruck.
    pub fn new(pitch: u8, start_beats: f64, length_beats: f64, velocity: u8) -> Self {
        Self { pitch, start_beats, length_beats, velocity, ..Default::default() }
    }
}
