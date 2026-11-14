//! What the core needs from the platform: events to the UI and the storage
//! locations. Desktop: Tauri (`app/src/lib.rs`), browser: the web core
//! (`crates/klang-web`).

use serde::Serialize;
use std::path::PathBuf;

pub trait Host: Send + Sync {
    /// Event to the UI (desktop: Tauri event, browser: `postMessage`);
    /// `json` is the fully serialized payload (see [`Host::emit`]).
    fn emit_json(&self, event: &str, json: String);
    /// Folder for projects (desktop: Documents/Klangwelt/Projekte).
    fn projects_dir(&self) -> PathBuf;
    /// Root of the library (desktop: Music/Klangwelt/Bibliothek). Exports and
    /// sounds live next to it (see [`crate::export::export_dir`]).
    fn library_root(&self) -> PathBuf;
}

/// Placeholder until the platform sets its host: reports nothing.
pub struct NoHost;

impl Host for NoHost {
    fn emit_json(&self, _event: &str, _json: String) {}
    fn projects_dir(&self) -> PathBuf {
        PathBuf::from("Projekte")
    }
    fn library_root(&self) -> PathBuf {
        PathBuf::from("Bibliothek")
    }
}

impl<'a> dyn Host + 'a {
    /// Event with payload to the UI. Serialized once, directly – like a reply
    /// to a command (numbers look the same, e.g. `f32`).
    pub fn emit<T: Serialize + ?Sized>(&self, event: &str, payload: &T) {
        self.emit_json(event, serde_json::to_string(payload).unwrap_or_else(|_| "null".into()));
    }
}

/// Folder next to the library (exports, sounds). With `KLANGWELT_BIBLIOTHEK`
/// (tests, custom location) inside the library folder itself.
pub fn beside_library(host: &dyn Host, name: &str) -> PathBuf {
    let root = host.library_root();
    let base = if std::env::var_os("KLANGWELT_BIBLIOTHEK").is_some_and(|r| !r.is_empty()) {
        root
    } else {
        // Musik/Klangwelt/Bibliothek → Musik/Klangwelt/<name>
        root.parent().map(std::path::Path::to_path_buf).unwrap_or(root)
    };
    base.join(name)
}
