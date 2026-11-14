//! Klang-Bibliothek: eigene Klänge – ein Instrument mit allen Reglern, der
//! Stellung in den Feldern und (beim Synth) den gezeichneten Kurven – unter
//! einem Namen sichern und in jede Instrumentspur laden, über alle Projekte
//! hinweg. Alles steht in einer Datei `klaenge.json` im Ordner `Klänge` neben
//! dem Bibliotheksordner (Musik/Klangwelt/Klänge); mit `KLANGWELT_BIBLIOTHEK`
//! liegt sie in diesem Ordner, damit Tests nie die echten Klänge berühren.
//!
//! Namen sind je Instrument eindeutig: Sichern unter einem vorhandenen Namen
//! ersetzt diesen Klang. Gesichert wird immer das vollständige Instrument
//! (fehlende Regler mit Standardwert), damit spätere neue Standardwerte einen
//! gesicherten Klang nicht verändern.

use crate::sound;
use crate::state::{AppState, CmdResult, Snapshot};
use klang_core::{Command, Id, Instrument, InstrumentKind, TrackPatch};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE: &str = "klaenge.json";
const DIR: &str = "Klänge";
const MAX_NAME: usize = 60;

/// Lesen–Ändern–Schreiben der Datei nur nacheinander (Oberfläche und Agent).
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SavedSound {
    pub id: String,
    pub name: String,
    pub instrument: Instrument,
    /// Unix-Zeit in Millisekunden
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub modified: u64,
}

#[derive(Serialize, Deserialize)]
struct Store {
    version: u32,
    #[serde(default)]
    sounds: Vec<SavedSound>,
}

impl Default for Store {
    fn default() -> Self {
        Self { version: 1, sounds: Vec::new() }
    }
}

/// Eintrag für Oberfläche und Agent: mit Instrumentname und kurzer Beschreibung.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SoundEntry {
    pub id: String,
    pub name: String,
    pub kind: InstrumentKind,
    pub kind_name: &'static str,
    /// Stellung in den Feldern in Worten, z. B. „Trap · dunkel, rau · breit, fern“
    pub summary: String,
    pub created: u64,
    pub modified: u64,
    pub instrument: Instrument,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub id: String,
    pub name: String,
    /// Es gab den Namen beim Instrument schon – der Klang wurde ersetzt.
    pub replaced: bool,
    pub sounds: Vec<SoundEntry>,
}

/// Datei der Klang-Bibliothek (neben dem Bibliotheksordner).
pub fn file(st: &AppState) -> PathBuf {
    crate::host::beside_library(st.host(), DIR).join(FILE)
}

fn read(path: &Path) -> Result<Store, String> {
    match crate::vfs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Die Klang-Bibliothek {} ist nicht lesbar ({e}) – sie bleibt unverändert.", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write(path: &Path, store: &Store) -> Result<(), String> {
    let dir = path.parent().ok_or("Kein Ordner für die Klang-Bibliothek")?;
    crate::vfs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = path.with_extension("json.neu");
    crate::vfs::write(&tmp, &serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", tmp.display()))?;
    crate::vfs::rename_retry(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn now_ms() -> u64 {
    crate::clock::now_ms()
}

fn clean_name(name: &str) -> Result<String, String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        return Err("Der Klang braucht einen Namen.".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("Der Name ist zu lang (höchstens {MAX_NAME} Zeichen)."));
    }
    Ok(n)
}

fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Reihenfolge wie in der Instrumentwahl.
fn kind_order(kind: InstrumentKind) -> usize {
    sound::specs().iter().position(|s| s.kind == kind).unwrap_or(usize::MAX)
}

/// Vollständiges, gültiges Instrument: alle Regler (fehlende mit Standardwert,
/// unbekannte entfernt, Werte begrenzt), beim Synth beide Kurven geprüft.
pub fn normalize(inst: &Instrument) -> Instrument {
    let spec = sound::spec(inst.kind);
    let params = spec
        .params
        .iter()
        .map(|p| {
            let v = inst.params.get(p.key).copied().filter(|v| v.is_finite()).unwrap_or(p.default);
            (p.key.to_string(), v.clamp(p.min, p.max))
        })
        .collect();
    let mut curves = BTreeMap::new();
    if inst.kind == InstrumentKind::Synth {
        let fallback = sound::default_instrument(InstrumentKind::Synth).curves;
        for k in ["laut", "hell"] {
            let checked = inst
                .curves
                .get(k)
                .and_then(|c| sound::validate_curves(&BTreeMap::from([(k.to_string(), c.clone())])).ok())
                .and_then(|mut m| m.remove(k));
            curves.insert(k.to_string(), checked.or_else(|| fallback.get(k).cloned()).unwrap_or_default());
        }
    }
    Instrument { kind: inst.kind, preset: inst.preset.clone(), params, curves }
}

/// Stellung in den drei Feldern in Worten – beim Charakter der Startpunkt,
/// wenn er gewählt ist; beim Synth dazu die Verlaufsvorlage, wenn sie passt.
pub fn summary(inst: &Instrument) -> String {
    let inst = normalize(inst);
    fn word(words: &[&'static str; 4], v: f32) -> &'static str {
        words[((v * 4.0).floor() as i64).clamp(0, 3) as usize]
    }
    let mut parts: Vec<String> = sound::fields(inst.kind)
        .iter()
        .map(|f| match f.presets.iter().find(|p| f.key == "charakter" && p.name == inst.preset) {
            Some(p) => p.name.to_string(),
            None => format!("{}, {}", word(&f.x.words, inst.param(f.x.param, 0.5)), word(&f.y.words, inst.param(f.y.param, 0.5))),
        })
        .collect();
    if inst.kind == InstrumentKind::Synth {
        let spec = sound::spec(inst.kind);
        if let Some(v) = spec.verlauf.iter().find(|v| inst.curves.get("laut") == Some(&v.laut) && inst.curves.get("hell") == Some(&v.hell)) {
            parts.push(format!("Verlauf {}", v.name));
        }
    }
    parts.join(" · ")
}

fn entry(s: &SavedSound) -> SoundEntry {
    SoundEntry {
        id: s.id.clone(),
        name: s.name.clone(),
        kind: s.instrument.kind,
        kind_name: sound::kind_name(s.instrument.kind),
        summary: summary(&s.instrument),
        created: s.created,
        modified: s.modified,
        instrument: s.instrument.clone(),
    }
}

fn entries(store: &Store) -> Vec<SoundEntry> {
    let mut v: Vec<SoundEntry> = store.sounds.iter().map(entry).collect();
    v.sort_by(|a, b| kind_order(a.kind).cmp(&kind_order(b.kind)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    v
}

fn guard() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Alle Klänge, nach Instrument (wie in der Auswahl) und Name geordnet.
pub fn list(path: &Path) -> Result<Vec<SoundEntry>, String> {
    let _g = guard();
    Ok(entries(&read(path)?))
}

/// Sichert einen Klang. Gibt es den Namen beim selben Instrument schon, wird
/// dieser Klang ersetzt.
pub fn save(path: &Path, name: &str, inst: &Instrument) -> Result<SaveResult, String> {
    let name = clean_name(name)?;
    let inst = normalize(inst);
    let _g = guard();
    let mut store = read(path)?;
    let now = now_ms();
    let (id, replaced) = match store.sounds.iter_mut().find(|s| s.instrument.kind == inst.kind && same_name(&s.name, &name)) {
        // Ersetzen ändert nur den Klang, der Name bleibt wie er war
        Some(s) => {
            s.instrument = inst;
            s.modified = now;
            (s.id.clone(), true)
        }
        None => {
            let mut n = now;
            while store.sounds.iter().any(|s| s.id == format!("k{n:x}")) {
                n += 1;
            }
            let id = format!("k{n:x}");
            store.sounds.push(SavedSound { id: id.clone(), name: name.clone(), instrument: inst, created: now, modified: now });
            (id, false)
        }
    };
    write(path, &store)?;
    let name = store.sounds.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or(name);
    Ok(SaveResult { id, name, replaced, sounds: entries(&store) })
}

pub fn rename(path: &Path, id: &str, name: &str) -> Result<Vec<SoundEntry>, String> {
    let name = clean_name(name)?;
    let _g = guard();
    let mut store = read(path)?;
    let kind = store.sounds.iter().find(|s| s.id == id).map(|s| s.instrument.kind).ok_or_else(|| format!("Klang „{id}“ nicht gefunden"))?;
    if store.sounds.iter().any(|s| s.id != id && s.instrument.kind == kind && same_name(&s.name, &name)) {
        return Err(format!("Es gibt schon einen Klang „{name}“ für {}.", sound::kind_name(kind)));
    }
    let s = store.sounds.iter_mut().find(|s| s.id == id).expect("eben gefunden");
    s.name = name;
    s.modified = now_ms();
    write(path, &store)?;
    Ok(entries(&store))
}

pub fn delete(path: &Path, id: &str) -> Result<Vec<SoundEntry>, String> {
    let _g = guard();
    let mut store = read(path)?;
    let before = store.sounds.len();
    store.sounds.retain(|s| s.id != id);
    if store.sounds.len() == before {
        return Err(format!("Klang „{id}“ nicht gefunden"));
    }
    write(path, &store)?;
    Ok(entries(&store))
}

/// Klang nach Kennung oder Name (ohne Groß-/Kleinschreibung). Gibt es den
/// Namen bei mehreren Instrumenten, entscheidet `prefer` (z. B. das der Spur).
pub fn find(path: &Path, key: &str, prefer: Option<InstrumentKind>) -> Result<SavedSound, String> {
    let _g = guard();
    let store = read(path)?;
    let key = key.trim();
    if let Some(s) = store.sounds.iter().find(|s| s.id == key) {
        return Ok(s.clone());
    }
    let named: Vec<&SavedSound> = store.sounds.iter().filter(|s| same_name(&s.name, key)).collect();
    match named.as_slice() {
        [] => {
            let names: Vec<String> = entries(&store).iter().map(|e| format!("{} ({})", e.name, e.kind_name)).collect();
            Err(if names.is_empty() {
                "Die Klang-Bibliothek ist leer – erst einen Klang sichern.".into()
            } else {
                format!("Kein Klang „{key}“. Vorhanden: {}", names.join(", "))
            })
        }
        [one] => Ok((*one).clone()),
        many => match many.iter().find(|s| Some(s.instrument.kind) == prefer) {
            Some(s) => Ok((*s).clone()),
            None => Err(format!(
                "„{key}“ gibt es bei mehreren Instrumenten ({}) – bitte die Kennung angeben.",
                many.iter().map(|s| format!("{}: {}", sound::kind_name(s.instrument.kind), s.id)).collect::<Vec<_>>().join(", ")
            )),
        },
    }
}

impl AppState {
    /// Lädt einen gesicherten Klang in eine Instrumentspur (ein Rückgängig-Schritt);
    /// ein Klang eines anderen Instruments wechselt dabei das Instrument.
    pub fn load_sound(&self, track_id: Id, saved: &SavedSound, label_prefix: &str) -> CmdResult<Snapshot> {
        let track = self.instrument_track(track_id)?;
        let label = format!("{label_prefix}Klang „{}“: „{}“ laden", track.name, saved.name);
        self.execute(
            Command::Batch {
                label,
                commands: vec![Command::SetTrack { track_id, patch: TrackPatch { instrument: Some(normalize(&saved.instrument)), ..Default::default() } }],
            },
            false,
        )
    }

    /// Instrument der Spur, so wie es gesichert würde.
    pub fn track_sound(&self, track_id: Id) -> CmdResult<Instrument> {
        let track = self.instrument_track(track_id)?;
        Ok(track.instrument.unwrap_or_else(|| sound::default_instrument(InstrumentKind::Pluck)))
    }
}

/// Meldet die neue Liste an die Oberfläche (auch wenn der Agent etwas geändert hat).
pub fn announce(st: &AppState, sounds: &[SoundEntry]) {
    st.host().emit("sounds-changed", sounds);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("klang-klaenge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(DIR).join(FILE)
    }

    fn drums(preset: &str) -> Instrument {
        let mut i = sound::default_instrument(InstrumentKind::Drums);
        let (x, y) = sound::field_preset(i.kind, "charakter", preset).unwrap();
        sound::apply_field(&mut i, "charakter", x, y).unwrap();
        i.preset = preset.into();
        i
    }

    #[test]
    fn save_replace_rename_delete_and_survive_a_reread() {
        let f = tmp("ablauf");
        assert!(list(&f).unwrap().is_empty(), "ohne Datei leer");
        let a = save(&f, "  Mein   Beat ", &drums("Trap")).unwrap();
        assert_eq!(a.name, "Mein Beat");
        assert!(!a.replaced);
        // derselbe Name bei einem anderen Instrument ist ein eigener Klang
        let b = save(&f, "mein beat", &sound::default_instrument(InstrumentKind::Bass)).unwrap();
        assert!(!b.replaced);
        assert_ne!(a.id, b.id);
        // beim selben Instrument ersetzt er (Kennung bleibt)
        let c = save(&f, "MEIN BEAT", &drums("House")).unwrap();
        assert!(c.replaced);
        assert_eq!(c.id, a.id);
        assert_eq!(c.name, "Mein Beat", "Ersetzen behält den Namen");
        let l = list(&f).unwrap();
        assert_eq!(l.len(), 2);
        // Reihenfolge wie in der Instrumentwahl: Bass vor Schlagzeug
        assert_eq!(l[0].kind, InstrumentKind::Bass);
        assert_eq!(l[1].instrument.preset, "House");
        assert!(l[1].summary.starts_with("House"), "{}", l[1].summary);
        // umbenennen; ein Name, den das Instrument schon hat, geht nicht
        save(&f, "Zweiter", &drums("Rock")).unwrap();
        assert!(rename(&f, &a.id, "zweiter").unwrap_err().contains("schon"));
        let l = rename(&f, &a.id, "Disco-Groove").unwrap();
        assert!(l.iter().any(|e| e.name == "Disco-Groove"));
        // finden nach Name (mehrdeutig → Instrument der Spur entscheidet)
        save(&f, "Disco-Groove", &sound::default_instrument(InstrumentKind::Bass)).unwrap();
        assert!(find(&f, "disco-groove", None).unwrap_err().contains("mehreren"));
        assert_eq!(find(&f, "disco-groove", Some(InstrumentKind::Drums)).unwrap().id, a.id);
        assert!(find(&f, "Nichts", None).unwrap_err().contains("Vorhanden"));
        // löschen
        let l = delete(&f, &a.id).unwrap();
        assert!(l.iter().all(|e| e.id != a.id));
        assert!(delete(&f, &a.id).is_err());
        assert!(save(&f, "   ", &drums("Trap")).is_err());
        let _ = std::fs::remove_dir_all(f.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn saved_sounds_are_complete_and_valid() {
        let mut i = sound::default_instrument(InstrumentKind::Synth);
        i.params.remove("filter");
        i.params.insert("gibt-es-nicht".into(), 3.0);
        i.params.insert("farbe".into(), 7.0);
        i.curves.insert("laut".into(), vec![[0.0, 0.5]]); // zu wenige Punkte
        let n = normalize(&i);
        let spec = sound::spec(InstrumentKind::Synth);
        assert_eq!(n.params.len(), spec.params.len());
        assert!(!n.params.contains_key("gibt-es-nicht"));
        assert_eq!(n.params["farbe"], 1.0, "begrenzt");
        assert_eq!(n.params["filter"], spec.params.iter().find(|p| p.key == "filter").unwrap().default);
        assert!(n.curves["laut"].len() >= 2, "ungültige Kurve → Standardverlauf");
        // andere Instrumente haben keine Kurven
        let mut d = drums("Trap");
        d.curves.insert("laut".into(), vec![[0.0, 1.0], [1.0, 0.0]]);
        assert!(normalize(&d).curves.is_empty());
    }

    #[test]
    fn summary_names_the_fields_in_words() {
        let mut i = drums("Trap");
        sound::apply_field(&mut i, "klangfarbe", 0.1, 0.9).unwrap();
        sound::apply_field(&mut i, "raum", 0.6, 0.95).unwrap();
        assert_eq!(summary(&i), "Trap · dunkel, rau · breit, fern");
        // ohne Startpunkt die Wörter der Achsen (Trap liegt bei x 0,95, y 0,55)
        i.preset = "eigen".into();
        let f = &sound::fields(InstrumentKind::Drums)[0];
        assert_eq!(summary(&i), format!("{}, {} · dunkel, rau · breit, fern", f.x.words[3], f.y.words[2]));
        let mut s = sound::default_instrument(InstrumentKind::Synth);
        sound::apply_verlauf(&mut s, "Glocke").unwrap();
        assert!(summary(&s).ends_with("Verlauf Glocke"), "{}", summary(&s));
    }

    #[test]
    fn an_unreadable_file_is_never_overwritten() {
        let f = tmp("kaputt");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, b"{ kaputt").unwrap();
        assert!(save(&f, "Neu", &drums("Trap")).unwrap_err().contains("nicht lesbar"));
        assert_eq!(std::fs::read(&f).unwrap(), b"{ kaputt");
        let _ = std::fs::remove_dir_all(f.parent().unwrap().parent().unwrap());
    }
}
