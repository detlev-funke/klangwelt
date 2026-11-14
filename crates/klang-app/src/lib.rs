//! Klangwelt-Kern: alles, was die App tut – ohne Plattform.
//!
//! Desktop (Tauri, `app/`) und Browser (WebAssembly, `crates/klang-web`) sind
//! nur dünne Hüllen um diesen Kern:
//!
//! * [`commands`] – die eine Befehlstabelle der Oberfläche: Name + JSON rein,
//!   JSON raus. Beide Hüllen leiten jeden Aufruf dorthin weiter.
//! * [`state::AppState`] – Sitzung (Projekt + Undo), Klänge, Engine, Bibliothek.
//! * [`Host`] – was der Kern von der Plattform braucht (Ereignisse, Ablageorte).
//! * [`agent`] – die Werkzeuge für den Agenten (MCP), ebenfalls gemeinsam.
//! * [`vfs`], [`clock`] – Dateien und Uhr (Desktop: `std`, Browser: im Speicher
//!   bzw. die Uhr der Seite).
//! * Die Audio-Engine ([`klang_engine::Engine`]) hat auf beiden Plattformen
//!   dieselbe Schnittstelle.
//!
//! Wer einen Befehl hinzufügt, trägt ihn in [`commands`] ein – dann gibt es
//! ihn auf beiden Plattformen.

pub mod agent;
pub mod analysis;
pub mod audiofile;
pub mod chat;
pub mod clock;
pub mod commands;
pub mod convert;
pub mod demo;
pub mod export;
pub mod flac;
pub mod fugue;
pub mod harmony;
pub mod host;
pub mod library;
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
