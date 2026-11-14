//! Klangwelt-Kern: Projektmodell, Befehle und Undo/Redo.
//!
//! Jede Änderung am Projekt läuft über einen [`Command`]. Die Oberfläche und
//! später der Agent (MCP) benutzen dieselbe, als JSON serialisierbare
//! Schnittstelle. `apply` liefert zu jedem Befehl seinen Umkehrbefehl –
//! daraus entsteht die Undo-Historie in [`Session`].

mod command;
pub mod edit;
mod model;
mod session;

pub use command::{apply, ClipPatch, Command, CoreError, ProjectPatch, TrackPatch};
pub use model::{
    Clip, ClipSource, Id, Instrument, InstrumentKind, Note, Project, Track, TrackKind, TrackView, TRACK_COLORS,
};
pub use session::Session;
