//! Klangwelt core: project model, commands and undo/redo.
//!
//! Every change to the project goes through a [`Command`]. The UI and
//! later the agent (MCP) use the same interface, which is serializable as JSON.
//! `apply` returns the inverse command for every command -
//! this builds the undo history in [`Session`].

mod command;
pub mod edit;
mod model;
mod session;

pub use command::{apply, ClipPatch, Command, CoreError, ProjectPatch, TrackPatch};
pub use model::{
    Clip, ClipSource, Id, Instrument, InstrumentKind, Note, Project, Track, TrackKind, TrackView, TRACK_COLORS,
};
pub use session::Session;
