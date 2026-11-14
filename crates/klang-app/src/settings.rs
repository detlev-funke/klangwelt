//! Kleine Einstellungen, die über Neustarts gelten (z. B. Metronom an/aus) –
//! vom Kern sofort in `einstellungen.json` geschrieben, damit nichts verloren
//! geht, auch wenn die App hart beendet wird. Wo die Datei liegt, bestimmt die
//! Plattform (Desktop: lokaler App-Datenordner, mit `KLANGWELT_BIBLIOTHEK` in
//! jenem Ordner; Browser: im Speicher des Browsers).

use serde_json::{Map, Value};
use std::path::Path;
use std::sync::Mutex;

pub const FILE: &str = "einstellungen.json";
static LOCK: Mutex<()> = Mutex::new(());

pub fn read(path: &Path) -> Map<String, Value> {
    crate::vfs::read(path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

/// Einen Wert setzen (Lesen–Ändern–Schreiben, atomar über eine Zwischendatei).
pub fn set(path: &Path, key: &str, value: Value) -> Result<(), String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = read(path);
    if map.get(key) == Some(&value) {
        return Ok(());
    }
    map.insert(key.to_string(), value);
    let data = serde_json::to_vec_pretty(&Value::Object(map)).map_err(|e| e.to_string())?;
    crate::vfs::write_atomic(path, &data, "json.neu").map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_survive_and_others_stay() {
        let dir = std::env::temp_dir().join(format!("klang-einstellungen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let f = dir.join(FILE);
        assert!(read(&f).is_empty(), "ohne Datei leer");
        set(&f, "metronom", Value::Bool(false)).unwrap();
        set(&f, "anderes", Value::from(3)).unwrap();
        set(&f, "metronom", Value::Bool(true)).unwrap();
        let m = read(&f);
        assert_eq!(m.get("metronom"), Some(&Value::Bool(true)));
        assert_eq!(m.get("anderes"), Some(&Value::from(3)));
        // kaputte Datei: gilt als leer, wird beim nächsten Setzen ersetzt
        std::fs::write(&f, b"{ kaputt").unwrap();
        assert!(read(&f).is_empty());
        set(&f, "metronom", Value::Bool(false)).unwrap();
        assert_eq!(read(&f).get("metronom"), Some(&Value::Bool(false)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
