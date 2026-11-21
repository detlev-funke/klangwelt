//! Klangwelt core: everything the app does – without the platform.
//!
//! Desktop (Tauri, `app/`) and browser (WebAssembly, `crates/klang-web`) are
//! just thin shells around this core:
//!
//! * [`commands`] – the one command table of the UI: name + JSON in,
//!   JSON out. Both shells forward every call there.
//! * [`state::AppState`] – session (project + undo), sounds, engine, library.
//! * [`Host`] – what the core needs from the platform (events, storage locations).
//! * [`agent`] – the tools for the agent (MCP), also shared.
//! * [`vfs`], [`clock`] – files and clock (desktop: `std`, browser: in memory
//!   and the page's clock respectively).
//! * The audio engine ([`klang_engine::Engine`]) has the same interface on
//!   both platforms.
//!
//! Whoever adds a command registers it in [`commands`] – then it exists
//! on both platforms.

pub mod agent;
pub mod analysis;
pub mod audiofile;
pub mod autosave;
pub mod chat;
pub mod clock;
pub mod commands;
pub mod convert;
pub mod demo;
pub mod export;
pub mod expression;
pub mod flac;
pub mod fugue;
pub mod harmony;
pub mod history;
pub mod host;
pub mod library;
pub mod notation;
pub mod project;
pub mod rhythm;
pub mod settings;
pub mod song;
pub mod sound;
pub mod sound_library;
pub mod state;
pub mod vfs;
pub mod weblib;

pub use host::Host;
pub use state::{lock, AppState, CmdResult, Snapshot};
