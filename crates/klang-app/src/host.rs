//! Was der Kern von der Plattform braucht: Ereignisse an die Oberfläche und
//! die Ablageorte. Desktop: Tauri (`app/src/lib.rs`), Browser: der Web-Kern
//! (`crates/klang-web`).

use serde::Serialize;
use std::path::PathBuf;

pub trait Host: Send + Sync {
    /// Ereignis an die Oberfläche (Desktop: Tauri-Event, Browser: `postMessage`);
    /// `json` ist der fertig serialisierte Inhalt (siehe [`Host::emit`]).
    fn emit_json(&self, event: &str, json: String);
    /// Ordner für Projekte (Desktop: Dokumente/Klangwelt/Projekte).
    fn projects_dir(&self) -> PathBuf;
    /// Wurzel der Bibliothek (Desktop: Musik/Klangwelt/Bibliothek). Exporte und
    /// Klänge liegen daneben (siehe [`crate::export::export_dir`]).
    fn library_root(&self) -> PathBuf;
}

/// Platzhalter, bis die Plattform ihren Host setzt: meldet nichts.
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
    /// Ereignis mit Inhalt an die Oberfläche. Einmal direkt serialisiert – wie
    /// eine Antwort auf einen Befehl (Zahlen erscheinen gleich, z. B. `f32`).
    pub fn emit<T: Serialize + ?Sized>(&self, event: &str, payload: &T) {
        self.emit_json(event, serde_json::to_string(payload).unwrap_or_else(|_| "null".into()));
    }
}

/// Ordner neben der Bibliothek (Exporte, Klänge). Mit `KLANGWELT_BIBLIOTHEK`
/// (Tests, eigene Ablage) im Bibliotheksordner selbst.
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
