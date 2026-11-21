//! Klangwelt core: project model, commands and undo/redo.
//!
//! Every change to the project goes through a [`Command`]. The UI and
//! later the agent (MCP) use the same interface, which is serializable as JSON.
//! `apply` returns the inverse command for every command -
//! this builds the history in [`Session`]: a tree of steps with checkpoints
//! and variants ([`history`]), key figures per step ([`Metrics`]) and the
//! comparison of two versions ([`diff`]).

mod command;
pub mod diff;
pub mod edit;
pub mod history;
pub mod merge;
mod metrics;
mod model;
mod session;
pub mod smooth;

pub use command::{apply, ClipPatch, Command, CoreError, ProjectPatch, TrackPatch};
pub use history::{Checkpoint, History, HistoryFile, NodeId, Scope, Variant};
pub use metrics::Metrics;
pub use model::{
    Clip, ClipSource, Id, Instrument, InstrumentKind, Note, Project, Track, TrackKind, TrackView, TRACK_COLORS,
};
pub use session::{MergeSources, Session, UndoMark};
