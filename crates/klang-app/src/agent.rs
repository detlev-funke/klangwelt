//! Werkzeuge für den Agenten (MCP): Ein Agent (z. B. Claude) steuert das
//! Projekt über dieselben Befehle wie die Oberfläche. Jede Änderung landet in
//! der Undo-Historie und erscheint sofort in der Oberfläche.
//!
//! Die Werkzeuge gehören zum gemeinsamen Kern – wie die Befehlstabelle der
//! Oberfläche. Nur der Weg zum Agenten ist je Plattform anders:
//! * Desktop: HTTP-Server auf 127.0.0.1 (`app/src/mcp.rs`, `klang-mcp`).
//! * Browser: `window.__klangweltWeb.mcp(…)` in der Seite – für einen Agenten,
//!   der die Seite über das DevTools-Protokoll oder eine Browser-Erweiterung
//!   bedient (siehe docs/WEB.md, `tools/klangwelt-web-mcp.mjs`).
//!
//! Was nur eine Plattform kann (Fenster nach vorn holen, Bibliotheken laden
//! und verwalten), liefert sie über [`AgentShell`].

use crate::harmony::{self, KeyInfo};
use crate::sound::{self, note_name};
use crate::rhythm;
use crate::song;
use crate::sound_library;
use crate::state::{lock, AppState};
use klang_core::{ClipSource, Command, Id, Instrument, InstrumentKind, TrackPatch};
use klang_mcp::{Handler, Prompt, ServerInfo, Tool};
use klang_music::analysis::analyze_score;
use klang_music::chordstats::GROUPS;
use klang_music::counterpoint::VNote;
use klang_music::fugue::Transform;
use klang_music::harmony as kh;
use klang_music::theory::Key;
use klang_music::index::MIN_QUERY_NOTES;
use klang_music::melody::MelodyNote;
use klang_music::score::load_score;
use serde_json::{json, Value};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Auftrag für den Zuhör-Subagenten (auch in `.claude/agents/klangwelt-zuhoerer.md`).
pub const LISTENER_BRIEF: &str = "Du bist der Zuhörer der Klangwelt-App. Die Person gibt im Chat der Oberfläche Aufträge, \
du erledigst sie mit den Klangwelt-MCP-Werkzeugen.\n\
Schleife: 1. `chat_poll` aufrufen – er antwortet, sobald ein Auftrag kommt, sonst nach timeoutSeconds. Nimm 280; \
bricht der Aufruf mit einem Timeout-Fehler ab oder enthält die Antwort `abbruch`, nimm ab dann 50 (dann ist das \
Client-Limit kürzer; es geht dabei kein Auftrag verloren). 2. Kommt eine leere Liste, sofort wieder `chat_poll` – das ist normal, \
nicht nachfragen, nicht zusammenfassen. 3. Für jeden Auftrag: bei längeren Arbeiten zuerst ein kurzes `chat_reply` mit \
done=false („Mache ich …“), dann mit `get_project` und den übrigen Werkzeugen ausführen und mit `chat_reply` \
(replyTo = Auftrags-id, done=true) in ein, zwei Sätzen auf Deutsch sagen, was du geändert hast. Rückfragen stellst du \
ebenfalls per `chat_reply` und wartest dann per `chat_poll` auf die Antwort. 4. Weiter mit Schritt 1.\n\
Hör nicht von selbst auf. Nur wenn die Person im Chat ausdrücklich „Stopp“ oder „hör auf“ schreibt, verabschiede dich per \
`chat_reply` und beende die Schleife. Dein Abschlussbericht an den aufrufenden Agenten: welche Aufträge du erledigt hast.";

const INSTRUCTIONS: &str = "Klangwelt ist eine Musik-App (Mehrspur, Aufnahme, Melodie-Erkennung, Soundwerkstatt). \
Du steuerst das geöffnete Projekt; alles erscheint live in der Oberfläche und lässt sich mit `undo` zurücknehmen.\n\
Chat: Die Person gibt dir Aufträge im Chat der Oberfläche. Zum Zuhören startest du einen Subagenten \
(Agent-/Task-Werkzeug, möglichst im Hintergrund; in Claude Code gibt es dafür den Typ „klangwelt-zuhoerer“, falls vorhanden), \
damit du selbst ansprechbar bleibst. Der Subagent ruft `chat_poll` in einer Schleife auf, führt jeden Auftrag aus und \
antwortet kurz auf Deutsch mit `chat_reply` (replyTo = Auftrags-id). Endet er (z. B. weil sein Kontext voll ist), starte einen neuen. \
Nur wenn du keine Subagenten starten kannst, pollst du selbst. Der Prompt `zuhoeren` enthält den fertigen Auftrag für den Subagenten.\n\
Einheiten: Positionen und Längen in Schlägen (beats), Tempo in BPM, Lautstärke in dB (−60…+12), Panorama −1 (links) … +1 (rechts), Tonhöhe in Halbtönen (−24…+24), MIDI-Töne 0–127 (60 = c').\n\
Vorgehen: Zuerst `get_project` lesen, um Spur- und Clip-IDs zu kennen. Änderungen mit `execute` (mehrere Befehle = ein Undo-Schritt). Neue Spuren/Clips mit id 0 anlegen – die App vergibt IDs.\n\
Audio-Clips mit `warp: true` laufen im Projekttempo; `sourceBpm` ist das Tempo, in dem sie eingespielt wurden.\n\
Audio und Bereiche: `import_audio` fügt Audiodateien (WAV, AIFF, FLAC, Opus; kein MP3) als Spuren ein. `audio_to_notes` wandelt eine Aufnahme \
in eine Melodie-Spur (erkannte Töne) oder eine Rhythmus-Spur (Anschläge → Bassdrum/Snare/Hi-Hat) direkt darunter um. \
`delete_range` löscht einen Zeitbereich aus allen Spuren, alles danach rückt auf (Clips werden geteilt bzw. gekürzt). \
`quantize_notes` richtet Noten am Raster aus (grid in Schlägen: 1 = Viertel, 0.5 = Achtel, 0.25 = Sechzehntel).\n\
Noten bearbeiten: `get_project` mit includeNotes lesen, dann `setClipNotes` mit der vollständigen neuen Notenliste (Start/Länge in Schlägen relativ zum Clipanfang). \
Mit `setTrack` und patch {view:'notes'} zeigst du eine Spur als Pianorolle, {view:'compact'} wieder flach.\n\
Ausdruck einzelner Noten (in `setClipNotes`): `velocity` 1–127 (pp≈32, p≈48, mp≈64, mf≈80, f≈100, ff≈120; Crescendo = steigende Werte), \
`vibrato` 0–1 Halbton Tiefe (0,3 gesanglich), `brightness` −1…+1, `bend` −12…+12 Halbtöne Anschleifen (−2 = von unten hineingleiten). \
Staccato = Notenlänge etwa halbieren, Legato = bis zur nächsten Note verlängern, Akzent = Anschlag +25. \
Zupfer und Bass nutzen alles; das Schlagzeug nur Anschlag und Helligkeit. Neutrale Werte (0) kannst du weglassen.\n\
Soundwerkstatt (Instrumentspuren): Instrumente Zupfer, Orgel (Register subbass, prinzipal, floete, quinte, oktave, mixtur), Cembalo (achtfuss, vierfuss, laute), Schlagzeug (jede Taste eine Trommel: 36 Bassdrum, 37 Rim, 38 Snare, 39 Clap, 42/46 Hi-Hat zu/offen, 41/45/48 Toms, 49 Crash, 51 Ride, 56 Cowbell, 70 Shaker), Bass-Synth und Synth (Flächen, lange Klänge). \
`list_instruments` nennt alle Parameter, `create_instrument_track` legt eine Spur mit Muster an, `set_instrument` wechselt das Instrument, \
`describe_sound` misst den Klang, `set_sound` ändert Parameter, `preview_sound` spielt ihn vor. Jedes Instrument hat drei Felder (`set_sound_field`): \
Charakter (je Instrument, z. B. Schlagzeug Gewicht knackig ↔ mächtig × Komplexität; setzt viele Regler auf einmal), Klangfarbe (farbe dunkel ↔ hell × rauheit glatt ↔ rau) \
und Raum (breite schmal ↔ breit × raum nah ↔ fern), jedes mit benannten Startpunkten – ein Feld ändert nie die Regler der anderen. Dazu waerme und schwebung. \
Beim Synth zusätzlich der gezeichnete Verlauf: `set_sound` mit verlauf (Vorlage wie „Anschwellen“, „Glocke“, „Pulsieren“) oder curves (laut/hell als [Sekunde, Wert]). \
Rhythmus (Fenster „Rhythmus“): `list_drum_patterns` nennt Schlagzeug-Muster typischer Genres (House, Techno, Hip-Hop, Trap, Drum & Bass, Funk, Reggaeton, Latin …) \
mit Tempo und Tipp; `write_drum_pattern` schreibt eines (oder eigene Zeilen wie kick: „x...x...x...x...“) als Clip, `read_drum_pattern` liest einen Schlagzeug-Clip als Raster. \
Für „mach mir einen House-Beat“: write_drum_pattern mit genre „House“ (ohne trackId entsteht eine neue Schlagzeug-Spur mit passendem Klang). \
Song-Aufbau (Fenster „Song“): `list_song_templates` zeigt Vorlagen ganzer Stücke (Trance, Ambient, Deep House, Synthwave, Lo-Fi) – Abschnitte mit Takten und Art \
(intro, teil, aufbau, break, hoehepunkt, outro) und je Rolle (pad, arp, bass, lead, kick, hats, clap, perc, fx) eine Stufe 0 aus / 1 sparsam / 2 voll. \
`write_song` erzeugt daraus die Spuren (ein Rückgängig-Schritt; ersetzt die zuletzt erzeugten Song-Spuren), auch mit eigenen Abschnitten, Tempo, Tonart, Akkorden. \
Für „bau mir einen Trance-Song, der langsam aufbaut“: write_song mit template „trance“ (bei Bedarf sections anpassen). \
Eigene Klänge (Klang-Bibliothek, über alle Projekte): `save_sound` sichert den Klang einer Spur unter einem Namen, `list_saved_sounds` zeigt alle, \
`load_saved_sound` lädt einen in eine Spur (für „nimm meinen Acid-Bass“ erst list_saved_sounds); dazu `rename_saved_sound`, `delete_saved_sound`. \
Für Wünsche wie „Kick tiefer und punchiger“ oder „Bass wärmer“ in kleinen Schritten ändern und nach jedem Schritt die Messwerte prüfen; \
den Bassdrum-Grundton (kick.grundton) passend zur Tonart des Basses wählen.\n\
Melodiesuche: `search_melody` findet Stücke zu Noten oder zu einer Aufnahme in der aktiven Bibliothek. Eine Bibliothek ist \
genau ein Index; gesucht wird nur in der aktiven (normalerweise „Standard“). `list_libraries` zeigt alle, `switch_library` wechselt, \
`create_library` legt eine leere an (z. B. für Versuche, ohne „Standard“ zu verändern), `delete_library` löscht eine (nur den Index). \
`download_library` lädt ein Paket und übernimmt es in die aktive Bibliothek (PDMX: 'pdmx-kostprobe' mit 3 000 oder 'pdmx-auswahl' mit 30 000 Partituren; nach Epochen in \
Forschungsqualität 'kern-barock' (Bach), 'kern-klassik' (Mozart, Beethoven, Haydn), 'kern-romantik', 'kern-20jh', mit Harmonieanalyse 'dcml-…'; läuft im \
Hintergrund – mit `library_status` prüfen). `add_to_library` nimmt MusicXML/MXL-Dateien (oder Ordner) in die aktive Bibliothek auf. `browse_library` ordnet die Bibliothek nach Genre, Komponist, Stimmung, Besetzung, Tonart usw. \
(z. B. für „zeig mir ruhige Klavierstücke in Moll“).\n\
Harmonik (Quintenzirkel-Fenster „Harmonie“): `analyze_harmony` liest Tonart und Akkordfolge (Stufen, Funktionen), `suggest_chords` \
schlägt Fortsetzungen und Kadenzen vor, `preview_chords` spielt eine Folge vor, `write_chords` setzt sie vierstimmig und schreibt sie \
als Noten (Spur „Akkorde“ oder trackId). Akkorde mit deutschen Tonnamen (H = englisch B, B = englisch B♭; Moll klein: a, fis) oder als \
Stufen (I, ii6, V7, V7/V, I64, N6). Für „schreib eine Kadenz“ erst suggest_chords mit cadences:true, dann write_chords. \
Mit corpus 'barock' oder 'bach' kommen die Häufigkeiten nur aus diesen Stücken.\n\
Fuge und Kontrapunkt (Fenster „Fuge“): Das Thema ist ein Noten-Clip. `fugue_subject` zeigt Antwort (real/tonal), Engführungen und \
Verwandlungen; `transform_melody` schreibt Umkehrung, Krebs, Vergrößerung usw. als Spur; `countersubject` erzeugt einen umkehrbaren \
Gegensatz; `write_fugue` baut ein komplettes Gerüst (2–4 Stimmen, Orgel oder Cembalo); `check_counterpoint` findet Parallelen, \
Kreuzungen und unvorbereitete Dissonanzen – danach gezielt mit setClipNotes korrigieren und erneut prüfen. Instrumente für \
Fugen: organ (Orgel) oder harpsichord (Cembalo).";

/// Was die Werkzeuge von der Plattform brauchen. Ohne eigene Umsetzung gibt
/// es das Werkzeug dort nicht (Fehlertext für den Agenten).
pub trait AgentShell: Send + Sync + 'static {
    /// Der Zustand der App.
    fn state(&self) -> &AppState;
    /// Projektdatei im Fenster öffnen (das fragt bei ungespeicherten Änderungen).
    fn open_in_window(&self, path: &Path) -> Result<(), String> {
        self.state().host().emit("open-file", &path.display().to_string());
        Ok(())
    }
    /// Kennungen der Pakete, die sich herunterladen lassen.
    fn pack_ids(&self) -> Vec<&'static str> {
        Vec::new()
    }
    /// Paket herunterladen und in die aktive Bibliothek aufnehmen (im Hintergrund);
    /// Zielordner und – läuft schon ein Auftrag – der Platz in der Warteschlange.
    fn library_download(&self, _id: &str) -> Result<(PathBuf, Option<usize>), String> {
        Err(NOT_HERE.into())
    }
    /// Dateien oder Ordner in die aktive Bibliothek aufnehmen (im Hintergrund); Name der Bibliothek.
    fn library_add(&self, _paths: &[String]) -> Result<String, String> {
        Err(NOT_HERE.into())
    }
    /// Alle Bibliotheken.
    fn libraries(&self) -> Value {
        json!([])
    }
    /// Bibliothek (Name oder Kennung) aktiv schalten.
    fn library_switch(&self, _name: &str) -> Result<(), String> {
        Err(NOT_HERE.into())
    }
    /// Neue Bibliothek, auf Wunsch gleich aktiv; liefert (Kennung, Name).
    fn library_create(&self, _name: &str, _activate: bool) -> Result<(String, String), String> {
        Err(NOT_HERE.into())
    }
    /// Bibliothek löschen; liefert ihre Kennung.
    fn library_delete(&self, _name: &str) -> Result<String, String> {
        Err(NOT_HERE.into())
    }
}

const NOT_HERE: &str = "Das gibt es auf dieser Plattform nicht (im Browser fehlen Herunterladen und Indizieren der Bibliothek).";

/// Die Werkzeuge – ein `klang_mcp::Handler` für beide Plattformen.
pub struct Agent<S: AgentShell> {
    shell: S,
}

impl<S: AgentShell> Agent<S> {
    pub fn new(shell: S) -> Self {
        Self { shell }
    }
}

/// Kurz warten, damit sich der Status der Engine gesetzt hat (im Browser
/// kann nichts warten).
fn settle() {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(std::time::Duration::from_millis(60));
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn tool(name: &'static str, title: &'static str, description: &'static str, input_schema: Value, read_only: bool) -> Tool {
    Tool { name, title, description, input_schema, read_only }
}

fn command_schema() -> Value {
    let id = json!({ "type": "integer", "minimum": 0 });
    let note = json!({ "type": "object", "properties": {
        "pitch": { "type": "integer", "minimum": 0, "maximum": 127 },
        "startBeats": { "type": "number", "minimum": 0 },
        "lengthBeats": { "type": "number", "exclusiveMinimum": 0 },
        "velocity": { "type": "integer", "minimum": 1, "maximum": 127, "description": "Anschlag: pp≈32, p≈48, mp≈64, mf≈80, f≈100, ff≈120" },
        "vibrato": { "type": "number", "minimum": 0, "maximum": 1, "description": "Vibrato-Tiefe in Halbtönen, setzt verzögert ein (0,3 ≈ gesanglich); weglassen = 0" },
        "brightness": { "type": "number", "minimum": -1, "maximum": 1, "description": "Helligkeit relativ zum Instrument (−1 dunkler … +1 heller); weglassen = 0" },
        "bend": { "type": "number", "minimum": -12, "maximum": 12, "description": "Anschleifen: Ton beginnt so viele Halbtöne versetzt und gleitet hinein (−2 = von unten); weglassen = 0" } },
        "required": ["pitch", "startBeats", "lengthBeats", "velocity"] });
    let clip = json!({ "type": "object", "description": "id 0 = neu. Instrument-Clips: source {type:'notes', lengthBeats, notes[]}",
        "properties": { "id": id, "name": { "type": "string" }, "startBeats": { "type": "number", "minimum": 0 },
            "source": { "type": "object", "properties": {
                "type": { "enum": ["notes", "audio"] }, "lengthBeats": { "type": "number" }, "notes": { "type": "array", "items": note },
                "assetId": { "type": "string" }, "sourceBpm": { "type": "number" }, "warp": { "type": "boolean" }, "durationSecs": { "type": "number" } },
                "required": ["type"] } },
        "required": ["id", "name", "startBeats", "source"] });
    let instrument = json!({ "type": "object", "properties": {
        "preset": { "type": "string" },
        "params": { "type": "object", "properties": {
            "helligkeit": { "type": "number", "minimum": 0, "maximum": 1 }, "abklingen": { "type": "number", "minimum": 0, "maximum": 1 },
            "koerper": { "type": "number", "minimum": 0, "maximum": 1 }, "anschlag": { "type": "number", "minimum": 0, "maximum": 1 } } } } });
    let track_patch = json!({ "type": "object", "properties": {
        "name": { "type": "string" }, "color": { "type": "string" }, "gainDb": { "type": "number", "minimum": -60, "maximum": 12 },
        "pan": { "type": "number", "minimum": -1, "maximum": 1 }, "mute": { "type": "boolean" }, "solo": { "type": "boolean" },
        "pitchSemitones": { "type": "number", "minimum": -24, "maximum": 24 }, "instrument": instrument,
        "view": { "enum": ["compact", "notes"], "description": "Darstellung: kompakt oder Notenansicht (Pianorolle); kein Undo-Schritt" },
        "compactHeight": { "type": "number", "description": "Zeilenhöhe kompakt in Pixel (0 = Standard, sonst 40–1200); kein Undo-Schritt" },
        "notesHeight": { "type": "number", "description": "Zeilenhöhe Notenansicht in Pixel (0 = Standard, sonst 40–1200); kein Undo-Schritt" } } });
    let variant = |t: &str, props: Value, req: &[&str]| {
        let mut p = props;
        p["type"] = json!({ "const": t });
        let mut r = vec!["type"];
        r.extend_from_slice(req);
        json!({ "type": "object", "properties": p, "required": r })
    };
    json!({ "oneOf": [
        variant("setProject", json!({ "patch": { "type": "object", "properties": {
            "name": { "type": "string" }, "tempoBpm": { "type": "number", "minimum": 20, "maximum": 400 },
            "beatsPerBar": { "type": "integer", "minimum": 1, "maximum": 32 }, "beatUnit": { "enum": [1, 2, 4, 8, 16, 32] } } } }), &["patch"]),
        variant("addTrack", json!({ "track": { "type": "object", "description": "id 0 = neu; kind 'audio' oder 'instrument'; weitere Felder wie bei setTrack, optional clips[]",
            "properties": { "id": id, "name": { "type": "string" }, "kind": { "enum": ["audio", "instrument"] }, "clips": { "type": "array", "items": clip } },
            "required": ["id", "name", "kind"] }, "index": { "type": "integer", "minimum": 0 } }), &["track"]),
        variant("removeTrack", json!({ "trackId": id }), &["trackId"]),
        variant("moveTrack", json!({ "trackId": id, "index": { "type": "integer", "minimum": 0 } }), &["trackId", "index"]),
        variant("setTrack", json!({ "trackId": id, "patch": track_patch }), &["trackId", "patch"]),
        variant("addClip", json!({ "trackId": id, "clip": clip }), &["trackId", "clip"]),
        variant("removeClip", json!({ "trackId": id, "clipId": id }), &["trackId", "clipId"]),
        variant("setClip", json!({ "trackId": id, "clipId": id, "patch": { "type": "object", "properties": {
            "name": { "type": "string" }, "startBeats": { "type": "number", "minimum": 0 },
            "sourceBpm": { "type": "number", "minimum": 20, "maximum": 400 }, "warp": { "type": "boolean" } } } }), &["trackId", "clipId", "patch"]),
        variant("setClipNotes", json!({ "trackId": id, "clipId": id,
            "notes": { "type": "array", "items": note, "description": "Vollständige neue Notenliste des Clips (ersetzt die alte)" },
            "lengthBeats": { "type": "number", "exclusiveMinimum": 0, "description": "Optional neue Cliplänge" } }), &["trackId", "clipId", "notes"])
    ] })
}

fn tools(packs: &[&'static str]) -> Vec<Tool> {
    let id = json!({ "type": "integer", "minimum": 1 });
    let pattern = json!({ "enum": sound::all_preview_keys(),
        "description": "Vorhör-Muster je Instrument – Zupfer: akkord, tonleiter, einzelton; Schlagzeug: beat, kick, snare, clap, hihat, hihat-offen, toms; Bass: basslinie, einzelton" });
    let kind = json!({ "enum": ["pluck", "drums", "bass", "organ", "harpsichord", "synth"],
        "description": "pluck = Zupfer, drums = Schlagzeug, bass = Bass-Synth, organ = Pfeifenorgel, harpsichord = Cembalo, synth = Synth für Flächen" });
    let key_arg = json!({ "type": "string", "description": "Tonart, z. B. „C-Dur“, „a-Moll“, „Es-Dur“, „fis-Moll“; weglassen = aus den Noten geschätzt" });
    let instrument_kind = json!({ "enum": ["organ", "harpsichord", "pluck"], "description": "Klang der neuen Spuren (Standard organ = Orgel)" });
    let chords_arg = json!({ "type": "array", "items": { "type": "string" }, "maxItems": 64,
        "description": "Akkorde als Symbole mit deutschen Tonnamen (C, a = a-Moll, G7, fis7, h°, Hø7, gis°7, Cmaj7, Dsus4, C/E; B = englisch B♭, H = englisch B) \
                        oder als Stufen in der Tonart (I, ii6, I64, V7, vii°7, V7/V, V7/vi, N6, ♭VI, iv6)" });
    vec![
        tool("get_project", "Projekt lesen",
            "Liefert das geöffnete Projekt: Tempo, Taktart, Spuren mit IDs, Pegel, Panorama, Tonhöhe, Instrument-Parametern und Clips (Start/Länge in Schlägen). Mit includeNotes auch die Noten der Instrument-Clips.",
            schema(json!({ "includeNotes": { "type": "boolean" } }), &[]), true),
        tool("execute", "Änderungen ausführen",
            "Führt einen oder mehrere Projektbefehle als einen Undo-Schritt aus (bei Fehler wird alles zurückgerollt). Beispiele: {\"type\":\"setProject\",\"patch\":{\"tempoBpm\":96}}, {\"type\":\"setTrack\",\"trackId\":3,\"patch\":{\"gainDb\":-6,\"pan\":-0.3}}, {\"type\":\"addTrack\",\"track\":{\"id\":0,\"name\":\"Bass\",\"kind\":\"instrument\",\"clips\":[{\"id\":0,\"name\":\"Riff\",\"startBeats\":0,\"source\":{\"type\":\"notes\",\"lengthBeats\":4,\"notes\":[{\"pitch\":45,\"startBeats\":0,\"lengthBeats\":1,\"velocity\":100}]}}]}}.",
            schema(json!({ "commands": { "type": "array", "minItems": 1, "items": command_schema() },
                "label": { "type": "string", "description": "Beschriftung des Undo-Schritts" } }), &["commands"]), false),
        tool("undo", "Rückgängig", "Nimmt den letzten Schritt zurück (auch Änderungen aus der Oberfläche).", schema(json!({}), &[]), false),
        tool("redo", "Wiederholen", "Stellt den zuletzt zurückgenommenen Schritt wieder her.", schema(json!({}), &[]), false),
        tool("transport", "Wiedergabe steuern", "Startet/stoppt die Wiedergabe oder springt an eine Position.",
            schema(json!({ "action": { "enum": ["play", "stop", "toggle", "rewind"] }, "positionBeats": { "type": "number", "minimum": 0 } }), &[]), false),
        tool("set_metronome", "Metronom", "Schaltet das Metronom ein oder aus.", schema(json!({ "enabled": { "type": "boolean" } }), &["enabled"]), false),
        tool("audio_devices", "Audiogeräte",
            "Listet die Ausgabegeräte (Lautsprecher, Kopfhörer, Interfaces) und Aufnahmegeräte (Mikrofone) des Systems, die Standardgeräte \
             und die aktuelle Wahl (null = Systemstandard) samt dem Gerät, das gerade spielt.",
            schema(json!({}), &[]), true),
        tool("set_audio_device", "Audiogerät wählen",
            "Wählt das Ausgabe- oder Aufnahmegerät – wirkt sofort (die Wiedergabe läuft auf dem neuen Gerät weiter) und gilt auch beim \
             nächsten Start. name: genau wie von audio_devices geliefert; null, leer oder „Systemstandard“ = Standardgerät des Systems. \
             Während einer Aufnahme nicht möglich.",
            schema(json!({ "kind": { "enum": ["ausgabe", "eingabe"] }, "name": { "type": ["string", "null"] } }), &["kind"]), false),
        tool("export_project", "Exportieren",
            "Exportiert das Projekt (oder mit range=true den in der Zeitleiste markierten Bereich) als Datei – so, wie es zu hören ist:              stumm geschaltete Spuren fehlen, Solo gilt. Formate: wav (unkomprimiert), flac (verlustfrei, kleiner als WAV),              opus (klein, für Handy und Weitergeben; bitrate in kbit/s, Standard 160), stems (ein Ordner mit einer WAV je Spur) und              midi (nur die Noten der Instrumentspuren). Ablage: Musik/Klangwelt/Exporte; name ohne Endung, oder ein vollständiger Pfad.              Eine vorhandene Datei gleichen Namens wird überschrieben.",
            schema(json!({
                "format": { "type": "string", "enum": ["wav", "flac", "opus", "stems", "midi"] },
                "name": { "type": "string", "description": "Dateiname ohne Endung oder vollständiger Pfad; weglassen = Name des Projekts" },
                "bitrate": { "type": "integer", "enum": [64, 96, 128, 160, 192, 256, 320], "description": "Nur opus: kbit/s (Standard 160)" },
                "bits": { "type": "integer", "enum": [16, 24], "description": "wav, flac, stems: Bittiefe (Standard 24)" },
                "range": { "type": "boolean", "description": "Nur den markierten Bereich (Standard: das ganze Stück)" }
            }), &["format"]), false),
        tool("open_project", "Projekt öffnen",
            "Öffnet eine .klangwelt-Datei im Klangwelt-Fenster und holt es nach vorn. Hat das offene Projekt ungespeicherte Änderungen, \
             fragt die Oberfläche erst nach (die Antwort kommt sofort, das Öffnen folgt im Fenster).",
            schema(json!({ "path": { "type": "string", "description": "Pfad zur Projektdatei" } }), &["path"]), false),
        tool("chat_poll", "Aufträge abholen (Chat)",
            "Wartet (Long-Poll) auf neue Aufträge, die im Chat der Klangwelt-Oberfläche eingegeben werden, und gibt sie zurück – oder eine leere Liste nach Ablauf der Wartezeit. Gedacht für einen Zuhör-Subagenten (siehe Prompt „zuhoeren“), damit der Hauptagent ansprechbar bleibt. Arbeitsweise: chat_poll aufrufen → Aufträge mit den anderen Werkzeugen ausführen → mit chat_reply antworten → wieder chat_poll aufrufen. Solange chat_poll wartet, zeigt die Oberfläche „Agent hört zu“.",
            schema(json!({ "timeoutSeconds": { "type": "number", "minimum": 1, "maximum": 590,
                "description": "Maximale Wartezeit; kommt vorher ein Auftrag, antwortet chat_poll sofort. Standard 50 s – passt unter das 60-s-Limit, das Claude Code ohne weitere Einstellung für HTTP-Aufrufe setzt. Mit \"timeout\": 330000 in der MCP-Konfiguration des Servers sind 280 s möglich (eine Anfrage alle ~5 Minuten)." } }), &[]), true),
        tool("chat_reply", "Im Chat antworten",
            "Schreibt eine Antwort in den Chat der Oberfläche. replyTo = id des Auftrags aus chat_poll; done = true (Standard) markiert ihn als erledigt, false für Zwischenstände.",
            schema(json!({ "text": { "type": "string" }, "replyTo": { "type": "integer" }, "done": { "type": "boolean" } }), &["text"]), false),
        tool("list_instruments", "Instrumente anzeigen",
            "Beschreibt die Instrumente der Soundwerkstatt: alle Parameter mit Bereich, Standardwert und Bedeutung, die drei Felder (Achsen, Startpunkte, welche Regler sie setzen), Presets (= Startpunkte des Charakter-Felds), Vorhör-Muster, beim Synth die Verlaufsvorlagen und beim Schlagzeug die Tastenbelegung.",
            schema(json!({}), &[]), true),
        tool("create_instrument_track", "Instrumentspur anlegen",
            "Legt eine Instrumentspur mit Standardklang und 4 Takten Beispielmuster an (Schlagzeug-Beat, Basslinie bzw. Arpeggio).",
            schema(json!({ "kind": kind }), &["kind"]), false),
        tool("set_instrument", "Instrument wechseln",
            "Setzt das Instrument einer Instrumentspur (Standardwerte, optional ein Preset aus list_instruments).",
            schema(json!({ "trackId": id, "kind": kind, "preset": { "type": "string" } }), &["trackId", "kind"]), false),
        tool("describe_sound", "Klang messen",
            "Misst den Klang einer Instrumentspur: Parameter und Messwerte mit Wörtern – Zupfer: Helligkeit, Wärme, Anschlag, Ausklingen; Schlagzeug: Bassdrum-Grundton (gemessen), Punch, Länge, Snare-Kessel, Teppich, Hi-Hat; Bass: Grundton, Helligkeit, Tiefe, Biss, Ausklingen; Synth: Helligkeit, Fülle, Bewegung, Einschwingen, Ausklang.",
            schema(json!({ "trackId": id }), &["trackId"]), true),
        tool("set_sound", "Klang einstellen",
            "Ändert Klangparameter einer Instrumentspur (nur angegebene Werte). Namen und Bereiche je Instrument siehe list_instruments – z. B. Zupfer: helligkeit, abklingen, koerper, anschlag (0…1); Schlagzeug: kick.grundton (MIDI-Ton 24–52), kick.punch, kick.laenge, snare.kessel (MIDI-Ton), hat.helligkeit …; Bass: wellenform, filter, resonanz, anschlag, abklingen, sub, biss, glide; Synth: stimmen, verstimmung, wellenform, sub, schimmer, filter, resonanz, bewegung, tempo. Bei allen: farbe (dunkel ↔ hell, 0,5 neutral), rauheit (glatt ↔ rau), breite (schmal ↔ breit), raum (nah ↔ fern), außer beim Schlagzeug auch waerme und schwebung. charakter.x/charakter.y verschieben den Punkt im Charakter-Feld (setzt die zugehörigen Regler). Synth: verlauf = Vorlage, curves = gezeichnete Kurven, verlauf.ausklang (Sekunden), verlauf.puls (0 aus, 1 Viertel, 2 Achtel, 3 Sechzehntel), verlauf.pulstiefe. Gibt danach die neuen Messwerte zurück; mit preview zusätzlich vorspielen.",
            schema(json!({ "trackId": id,
                "params": { "type": "object", "additionalProperties": { "type": "number" } },
                "verlauf": { "type": "string", "description": "Synth: Verlaufsvorlage, z. B. Anschwellen, Glocke, Atmen, Aufblühen, Pulsieren, Wellen, Gate, Stich" },
                "curves": { "type": "object", "description": "Synth: { laut: [[Sekunde, Wert 0–1], …], hell: […] } – Werte zwischen den Punkten linear, nach dem letzten gehalten",
                    "properties": { "laut": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } },
                        "hell": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } } } },
                "preset": { "type": "string", "description": "Name für diese Einstellung, z. B. 'warm-weich'" },
                "preview": pattern }), &["trackId"]), false),
        tool("set_sound_field", "Klangfeld einstellen",
            "Setzt im Klang-Fenster einen Punkt in einem der drei Felder – x/y je 0…1 oder einen benannten Startpunkt (list_instruments → fields). charakter: je Instrument (Schlagzeug: Gewicht knackig ↔ mächtig × Komplexität; Synth: Fülle × Komplexität …), klangfarbe: dunkel ↔ hell × glatt ↔ rau, raum: schmal ↔ breit × nah ↔ fern. Ändert nur die Regler dieses Felds; gibt die Messwerte zurück.",
            schema(json!({ "trackId": id, "field": { "enum": ["charakter", "klangfarbe", "raum"] },
                "x": { "type": "number", "minimum": 0, "maximum": 1 }, "y": { "type": "number", "minimum": 0, "maximum": 1 },
                "preset": { "type": "string", "description": "Startpunkt, z. B. 'Trap' (Charakter Schlagzeug), 'Staub' (Klangfarbe), 'Kirche' (Raum)" },
                "preview": pattern }), &["trackId", "field"]), false),
        tool("list_saved_sounds", "Gesicherte Klänge anzeigen",
            "Die Klang-Bibliothek: eigene Klänge, gesichert im Klang-Fenster („Meine Klänge“) oder mit save_sound – über alle Projekte hinweg. Je Klang Kennung, Name, Instrument und die Stellung in den Feldern in Worten. Mit kind nur die eines Instruments.",
            schema(json!({ "kind": kind }), &[]), true),
        tool("save_sound", "Klang sichern",
            "Sichert den aktuellen Klang einer Instrumentspur (alle Regler, Stellung in den Feldern, beim Synth der Verlauf) unter einem Namen in der Klang-Bibliothek. Gibt es den Namen beim selben Instrument schon, wird dieser Klang ersetzt (ersetzt: true).",
            schema(json!({ "trackId": id, "name": { "type": "string", "maxLength": 60 } }), &["trackId", "name"]), false),
        tool("load_saved_sound", "Gesicherten Klang laden",
            "Lädt einen Klang aus der Klang-Bibliothek (Name oder Kennung aus list_saved_sounds) in eine Instrumentspur – ein Rückgängig-Schritt; ein Klang eines anderen Instruments wechselt das Instrument der Spur. Gibt die Messwerte zurück; mit preview zusätzlich vorspielen.",
            schema(json!({ "trackId": id, "sound": { "type": "string", "description": "Name oder Kennung" }, "preview": pattern }), &["trackId", "sound"]), false),
        tool("rename_saved_sound", "Gesicherten Klang umbenennen",
            "Gibt einem Klang der Klang-Bibliothek einen neuen Namen (je Instrument eindeutig).",
            schema(json!({ "sound": { "type": "string", "description": "Name oder Kennung" }, "name": { "type": "string", "maxLength": 60 } }), &["sound", "name"]), false),
        tool("delete_saved_sound", "Gesicherten Klang löschen",
            "Löscht einen Klang aus der Klang-Bibliothek – endgültig, nur auf ausdrücklichen Wunsch der Person. Spuren, die ihn geladen haben, behalten ihren Klang.",
            schema(json!({ "sound": { "type": "string", "description": "Name oder Kennung" } }), &["sound"]), false),
        tool("list_drum_patterns", "Schlagzeug-Muster anzeigen",
            "Schlagzeug-Muster typischer Genres (nach dem Pocket-Operations-Heft): je Genre eine Beschreibung und Muster mit Tempo (BPM), empfohlenem Swing, Tipp und Zeilen als Text – ein Zeichen je Sechzehntel, x = Schlag (z. B. kick \"x...x...x...x...\"). Dazu die Trommeln (kick, snare, clap, rim, hhc = Hi-Hat zu, hho = offen, crash, ride, cow, tomh, tomm, toml, shk).",
            schema(json!({ "genre": { "type": "string", "description": "nur dieses Genre, z. B. „House“, „Trap“, „Latin“" } }), &[]), true),
        tool("write_drum_pattern", "Schlagzeug-Muster schreiben",
            "Schreibt ein Muster (genre + pattern, oder eigene rows) als Schlagzeug-Clip – ein Rückgängig-Schritt. Ohne trackId entsteht eine neue Schlagzeug-Spur mit dem Klang des Genres; mit trackId ein neuer Clip hinter dem letzten (oder ab startBeats); mit clipId werden dessen Noten ersetzt. Betonungen werden gesetzt (Backbeat laut, Zwischen-Snares leise), Swing verschiebt jede zweite Sechzehntel.",
            schema(json!({ "genre": { "type": "string" }, "pattern": { "type": "string", "description": "Name aus list_drum_patterns; leer = erstes Muster des Genres" },
                "rows": { "type": "object", "description": "eigene Zeilen statt eines Musters: { kick: \"x...x...x...x...\", snare: [5, 13], … } (Text je Sechzehntel oder Schritte ab 1)" },
                "bars": { "type": "integer", "enum": [1, 2, 3, 4], "description": "Länge in Takten (Muster werden wiederholt), Standard 4" },
                "swing": { "type": "number", "minimum": 0, "maximum": 100, "description": "Prozent; Standard = Empfehlung des Musters" },
                "trackId": id, "clipId": id, "startBeats": { "type": "number", "minimum": 0 },
                "setTempo": { "type": "boolean", "description": "Projekttempo auf das Tempo des Musters setzen" },
                "preview": { "type": "boolean", "description": "danach vorspielen" } }), &[]), false),
        tool("read_drum_pattern", "Schlagzeug-Clip lesen",
            "Liest einen Schlagzeug-Clip als Raster (bis 4 Takte): Zeilen als Text (x = Schlag je Sechzehntel), geschätzter Swing und wie viele Noten nicht ins Raster passen.",
            schema(json!({ "trackId": id, "clipId": id }), &["trackId", "clipId"]), true),
        tool("list_song_templates", "Song-Vorlagen anzeigen",
            "Vorlagen ganzer Stücke: Tempo, Tonart, Akkorde (Takte je Akkord), Rollen (key, Name, Instrument, Klang) und Abschnitte (Name, Takte, Art, Stufen je Rolle 0–2). Grundlage für write_song.",
            schema(json!({}), &[]), true),
        tool("write_song", "Song erzeugen",
            "Erzeugt einen ganzen Song als Spuren: je melodischer Rolle eine Spur, alle Trommeln zusammen eine Schlagzeug-Spur, ein Clip je Abschnitt. Setzt Tempo und 4/4. Aufbau-Abschnitte werden lauter und heller und enden (mit Rolle fx) im Snare-Wirbel, Outros klingen aus. Ein Rückgängig-Schritt; ersetzt die zuletzt erzeugten Song-Spuren (replace: false behält sie).",
            schema(json!({
                "template": { "type": "string", "description": "trance, ambient, deep-house, synthwave, lofi" },
                "tempo": { "type": "number", "minimum": 40, "maximum": 220 },
                "key": { "type": "string", "description": "z. B. „a-Moll“, „D-Dur“" },
                "chords": { "type": "array", "items": { "type": "string" }, "description": "Akkorde reihum, z. B. [\"a\", \"F\", \"C\", \"G\"] oder Stufen [\"i\", \"VI\", \"III\", \"VII\"]" },
                "chordBars": { "type": "integer", "minimum": 1, "maximum": 8 },
                "sections": { "type": "array", "description": "eigene Abschnitte statt der Vorlage: [{ name, bars, kind, levels: { pad: 2, kick: 1, … } }]",
                    "items": { "type": "object", "properties": {
                        "name": { "type": "string" }, "bars": { "type": "integer", "minimum": 1, "maximum": 64 },
                        "kind": { "enum": ["intro", "teil", "aufbau", "break", "hoehepunkt", "outro"] },
                        "levels": { "type": "object", "additionalProperties": { "type": "integer", "minimum": 0, "maximum": 2 } } }, "required": ["name", "bars"] } },
                "replace": { "type": "boolean" },
                "play": { "type": "boolean", "description": "danach von vorn abspielen" } }), &["template"]), false),
        tool("preview_sound", "Klang vorhören", "Spielt ein kurzes Vorhör-Muster mit dem Klang der Instrumentspur, unabhängig von der Wiedergabe.",
            schema(json!({ "trackId": id, "pattern": pattern }), &["trackId"]), false),
        tool("analyze_clip", "Aufnahme analysieren",
            "Analysiert einen Audio-Clip: erkannte Noten (bereinigt), Tonart, Tempo-Schätzung und Anschläge mit Grundton (auch Schlagzeug: Bassdrum, Snare, Toms; grundton null = tonlos wie Hi-Hat).",
            schema(json!({ "trackId": id, "clipId": id }), &["trackId", "clipId"]), true),
        tool("melody_to_track", "Melodie übernehmen",
            "Legt aus der erkannten Melodie eines Audio-Clips eine Instrumentspur an (auf Sechzehntel quantisiert).",
            schema(json!({ "trackId": id, "clipId": id }), &["trackId", "clipId"]), false),
        tool("audio_to_notes", "Aufnahme in Noten umwandeln",
            "Wandelt Audio-Clips in Noten um – als neue Instrumentspur direkt unter der Audiospur, ein Clip je Audio-Clip an derselben Stelle (ein Rückgängig-Schritt). \
             target 'melodie': erkannte Töne, auf Sechzehntel gerastert (tiefe Lage wird eine Bass-Spur); 'rhythmus': erkannte Anschläge als Schlagzeug-Spur \
             (Bassdrum, Snare, Hi-Hat nach Klangfarbe). Ohne clipId alle Audio-Clips der Spur.",
            schema(json!({ "trackId": id, "clipId": id, "target": { "enum": ["melodie", "rhythmus"] } }), &["trackId", "target"]), false),
        tool("import_audio", "Audiodateien einfügen",
            "Fügt Audiodateien als Spuren ein (ein Rückgängig-Schritt): je Datei eine neue Audiospur mit einem Clip ab startBeats. Mit trackId einer Audiospur \
             kommt die erste Datei in diese Spur, weitere als neue Spuren darunter. Formate: WAV (8/16/24/32 Bit, Gleitkomma), AIFF, FLAC, Opus; \
             MP3, AAC/M4A und Ogg Vorbis noch nicht. Stereo wird zu mono gemittelt. Die Clips laufen im Originaltempo (warp aus).",
            schema(json!({ "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "Vollständige Dateipfade" },
                "trackId": id, "startBeats": { "type": "number", "minimum": 0 } }), &["paths"]), false),
        tool("delete_range", "Zeitbereich löschen",
            "Löscht den Zeitbereich [startBeats, endBeats) aus allen Spuren; alles danach rückt um seine Länge auf. Clips im Bereich fallen weg, \
             Clips, die ihn schneiden, werden gekürzt (Noten im Bereich fallen weg, Audio wird geschnitten). Ein Rückgängig-Schritt; die Markierung ist danach aufgehoben.",
            schema(json!({ "startBeats": { "type": "number", "minimum": 0 }, "endBeats": { "type": "number", "minimum": 0 } }), &["startBeats", "endBeats"]), false),
        tool("quantize_notes", "Noten am Raster ausrichten",
            "Richtet Noten eines Noten-Clips am Raster aus (in Projektzeit): Anfang auf den nächsten Rasterpunkt, mit lengths auch das Ende. \
             grid in Schlägen (1 = Viertel, 0.5 = Achtel, 0.25 = Sechzehntel); notes = Indizes der Noten (sortiert wie in get_project), weglassen = alle.",
            schema(json!({ "trackId": id, "clipId": id, "grid": { "type": "number", "minimum": 0.015625, "maximum": 16 }, "lengths": { "type": "boolean" },
                "notes": { "type": "array", "items": { "type": "integer", "minimum": 0 } } }), &["trackId", "clipId", "grid"]), false),
        tool("list_libraries", "Bibliotheken auflisten",
            "Alle Bibliotheken (je ein Index) mit Partituren, enthaltenen Paketen und Ordnern; active = in ihr wird gesucht.",
            schema(json!({}), &[]), true),
        tool("switch_library", "Bibliothek wechseln",
            "Macht eine Bibliothek zur aktiven (Kennung oder Name aus list_libraries). Suche, Baum und Harmonie-Statistik kommen danach aus ihr; bleibt über einen Neustart aktiv.",
            schema(json!({ "library": { "type": "string" } }), &["library"]), false),
        tool("create_library", "Bibliothek anlegen",
            "Legt eine leere Bibliothek an und macht sie aktiv (activate: false lässt die aktuelle aktiv). Danach mit download_library oder add_to_library füllen.",
            schema(json!({ "name": { "type": "string" }, "activate": { "type": "boolean" } }), &["name"]), false),
        tool("delete_library", "Bibliothek löschen",
            "Löscht eine Bibliothek – nur ihren Index, nie Partituren. „Standard“ lässt sich nicht löschen; war die gelöschte aktiv, wird „Standard“ aktiv.",
            schema(json!({ "library": { "type": "string" } }), &["library"]), false),
        tool("download_library", "Bibliothek herunterladen",
            "Lädt ein Notenpaket herunter, indiziert es im Hintergrund und übernimmt es in die aktive Bibliothek (schon enthaltene Partituren einmal). \
             PDMX (gemischt, schwankende Qualität): 'pdmx-kostprobe' (3 000 Partituren, ≈ 10 s), 'pdmx-auswahl' (30 000, ≈ 1,5 min), 'pdmx-voll' \
             (≈ 254 000, 1,9 GB Download, ≈ 3,5 GB auf der Platte – nur auf ausdrücklichen Wunsch der Person). \
             OpenScore (CC0, sorgfältig gesetzt): 'openscore-lieder' (≈ 1 460 Kunstlieder), 'openscore-quartette' (≈ 200 Quartettsätze, u. a. Haydn, Mozart, Beethoven). \
             KernScores (Forschungsausgaben, je Epoche): 'kern-barock' (Bach: Choräle, Wohltemperiertes Klavier, Kunst der Fuge …; Scarlatti, Vivaldi), \
             'kern-klassik' (Mozart- und Beethoven-Klaviersonaten, Beethoven-Quartette, Haydn), 'kern-romantik' (Chopin, Schubert …), 'kern-20jh' (Joplin, Gershwin). \
             DCML (mit Akkordanalyse von Fachleuten, macht die Akkordstatistik verlässlicher): 'dcml-barock' (Bach-Suiten, Corelli, Monteverdi …), \
             'dcml-klassik' (Mozart, Beethoven, C. P. E. Bach …), 'dcml-romantik', 'dcml-20jh'. Die kleinen Sammlungen sind in Sekunden geladen. \
             Antwortet sofort; mit `library_status` den Fortschritt abfragen. Läuft schon ein Auftrag, kommt das Paket in die Warteschlange \
             und startet danach von selbst – mehrere Pakete lassen sich so nacheinander beauftragen (`library_status`: queue).",
            schema(json!({ "id": { "type": "string", "enum": packs } }), &["id"]), false),
        tool("add_to_library", "Zur Bibliothek hinzufügen",
            "Nimmt MusicXML/MXL-Dateien (oder alle eines Ordners) in die aktive Bibliothek auf: Sie werden indiziert und mit ihrem Index verschmolzen \
             (danach in Suche und Baum). Die Dateien bleiben, wo sie sind. Schon enthaltene werden übersprungen. Läuft im Hintergrund; \
             das Ergebnis steht danach in `library_status` (message, scores).",
            schema(json!({ "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1 } }), &["paths"]), false),
        tool("browse_library", "Bibliothek erkunden",
            "Ordnet die geladene Bibliothek wie einen Baum: levels = Reihenfolge der Ebenen, path = gewählte Werte der ersten Ebenen. \
             Antwortet mit den Zweigen der nächsten Ebene (Wert + Anzahl) bzw. – wenn path so lang ist wie levels – mit den Partituren \
             (doc, Titel, Komponist). Ebenen: genre, komponist, stimmung, faerbung, energie, tongeschlecht, tonart, taktart, takt, tempo, \
             besetzung, instrumente, satz, harmonik, laenge, schwierigkeit. Beispiel: levels ['genre','besetzung'], path ['Klassik'] → \
             Besetzungen klassischer Stücke. Genre und Instrumentengruppe können mehrfach zutreffen.",
            schema(json!({
                "levels": { "type": "array", "items": { "type": "string" } },
                "path": { "type": "array", "items": { "type": "string" } },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000 },
                "offset": { "type": "integer", "minimum": 0 }
            }), &["levels"]), true),
        tool("library_status", "Bibliothek: Stand",
            "Welche Bibliothek aktiv ist (name, Partituren, Stimmen) und ob gerade heruntergeladen, indiziert oder übernommen wird.",
            schema(json!({}), &[]), true),
        tool("search_melody", "Melodie suchen",
            "Sucht in der geladenen Bibliothek nach Stücken mit dieser Melodie – unabhängig von Tonart und Tempo. Entweder notes (mind. 5, Dauer in beliebiger Einheit) oder trackId+clipId eines Audio-Clips angeben.",
            schema(json!({ "notes": { "type": "array", "items": { "type": "object", "properties": {
                    "midi": { "type": "integer", "minimum": 0, "maximum": 127 }, "duration": { "type": "number", "exclusiveMinimum": 0 } },
                    "required": ["midi", "duration"] } },
                "trackId": id, "clipId": id, "limit": { "type": "integer", "minimum": 1, "maximum": 50 } }), &[]), true),
        tool("analyze_harmony", "Harmonik analysieren",
            "Tonart (aus den Noten geschätzt oder angegeben) und Akkordfolge mit Stufen (römisch) und Funktionen (Riemann) – einer Spur (trackId), \
             eines Clips (trackId + clipId) oder ohne Angabe aller Instrumentspuren zusammen (die Harmonik des ganzen Satzes). Raster: halber Takt.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone() }), &[]), true),
        tool("suggest_chords", "Akkordfortsetzungen",
            "Schlägt Fortsetzungen nach einer Akkordfolge vor, beste zuerst (Funktionsharmonik: Ganzschluss, Trugschluss, Plagalschluss, \
             Doppeldominante, Zwischendominanten mit Ausweichtonart, Quartsextvorhalt, Neapolitaner, Moll-Entlehnungen, Quintfall). \
             Ohne chords: womit man beginnt. Jeder Vorschlag hat eine stimmung (lösend, spannend, überraschend, hell, dunkel). \
             wegeZumSchluss: mehrschrittige Wege (bis 4 Akkorde) vom letzten Akkord zu Ganz-, Plagal-, Halb-, Trug- und phrygischem \
             Schluss, mit Stimmung je Übergang und – bei geladener Bibliothek – dem Anteil jedes Schritts dort. \
             cadences: true liefert zusätzlich die Kadenzvorlagen der Tonart (authentisch, plagal, \
             Halbschluss, phrygisch, Trugschluss, Picardische Terz, Lamentobass, Quintfallsequenz …).",
            schema(json!({ "chords": chords_arg.clone(), "key": key_arg.clone(), "cadences": { "type": "boolean" },
                "corpus": { "enum": ["alle", "barock", "bach"], "description": "Häufigkeiten aus allen Stücken, nur Barock oder nur Bach" } }), &[]), true),
        tool("write_chords", "Akkorde schreiben",
            "Setzt eine Akkordfolge vierstimmig (Choralsatz: kurze Wege, keine Quint- und Oktavparallelen, Leittöne steigen, Septimen fallen) \
             und schreibt sie als Noten – in trackId oder in die Spur „Akkorde“ (wird bei Bedarf angelegt). Liegt dort an startBeats schon \
             ein Noten-Clip, werden die Noten in diesem Bereich ersetzt, sonst entsteht ein neuer Clip. Mit mode „einfuegen“ wird an startBeats \
             Platz geschaffen: alles ab dort rückt in allen Spuren um die Länge der Akkorde nach hinten. Ein Undo-Schritt. \
             preview: true spielt die Folge zusätzlich vor.",
            schema(json!({ "chords": chords_arg.clone(), "key": key_arg.clone(),
                "startBeats": { "type": "number", "minimum": 0, "description": "Start in Schlägen (Standard 0)" },
                "beatsPerChord": { "type": "number", "minimum": 0.25, "maximum": 32, "description": "Dauer je Akkord in Schlägen (Standard: ein Takt)" },
                "trackId": id, "preview": { "type": "boolean" },
                "mode": { "enum": ["ersetzen", "einfuegen"], "description": "ersetzen (Standard): Noten im Bereich der Ziel-Spur ersetzen; einfuegen: Platz schaffen, Nachfolgendes rückt nach hinten" } }), &["chords"]), false),
        tool("preview_chords", "Akkorde vorhören",
            "Spielt eine Akkordfolge vierstimmig vor (Klang von trackId, sonst Zupfer), ohne das Projekt zu ändern.",
            schema(json!({ "chords": chords_arg, "key": key_arg,
                "beatsPerChord": { "type": "number", "minimum": 0.25, "maximum": 32 }, "trackId": id }), &["chords"]), false),
        tool("fugue_subject", "Fugenthema untersuchen",
            "Untersucht einen Noten-Clip als Fugenthema: Tonart, Länge, Antwort (real oder tonal, mit Begründung und veränderten Tönen), \
             mögliche Engführungen (Abstand, Intervall, Überlappung – nur fehlerfreie) und die Verwandlungen.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone() }), &["trackId", "clipId"]), true),
        tool("transform_melody", "Melodie verwandeln",
            "Verwandelt einen Noten-Clip (Thema): umkehrung (diatonisch gespiegelt), krebs, krebsumkehrung, vergroesserung (doppelte Werte), \
             verkleinerung, versetzung (steps Stufen in der Tonart), chromatisch (steps Halbtöne). write (Standard true) legt das Ergebnis \
             als neue Spur an derselben Stelle an.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone(),
                "transform": { "enum": ["umkehrung", "krebs", "krebsumkehrung", "vergroesserung", "verkleinerung", "versetzung", "chromatisch"] },
                "steps": { "type": "integer", "minimum": -24, "maximum": 24 }, "write": { "type": "boolean" },
                "instrument": instrument_kind.clone() }), &["trackId", "clipId", "transform"]), false),
        tool("countersubject", "Gegensatz erzeugen",
            "Schlägt Gegensätze (Kontrasubjekte) zum Thema vor – im doppelten Kontrapunkt der Oktave geprüft (umkehrbar = auch unter dem \
             Thema fehlerfrei). Mit write: true wird der beste als neue Spur unter/über das Thema gelegt.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone(), "write": { "type": "boolean" }, "instrument": instrument_kind.clone() }),
                &["trackId", "clipId"]), false),
        tool("write_fugue", "Fuge anlegen",
            "Baut aus einem Thema (Noten-Clip) ein Fugen-Gerüst und schreibt es als Spuren (je Stimme eine): Exposition mit Antwort und \
             Gegensatz, Zwischenspiele (Sequenz über Quintfall), Durchführungen in Parallel- und Subdominanttonart, Engführung, Orgelpunkt, \
             Schlusskadenz (Moll mit Picardischer Terz). Ein Undo-Schritt. Danach mit check_counterpoint prüfen und nachbessern.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone(),
                "voices": { "type": "integer", "minimum": 2, "maximum": 4, "description": "Standard 3" },
                "instrument": instrument_kind.clone(),
                "startBeats": { "type": "number", "minimum": 0, "description": "Standard: hinter dem letzten Clip" },
                "countersubject": { "type": "boolean" }, "stretto": { "type": "boolean" }, "picardy": { "type": "boolean" } }),
                &["trackId", "clipId"]), false),
        tool("check_counterpoint", "Kontrapunkt prüfen",
            "Prüft die Instrumentspuren (je Spur eine Stimme, Schlagzeug ausgenommen) auf Satzfehler: Quint- und Oktavparallelen, verdeckte \
             Parallelen der Außenstimmen, Stimmkreuzung, unvorbereitete Dissonanzen auf schwerer Zeit, freie Dissonanzen, Umfang, große \
             Sprünge. Liefert Stellen mit Takt, Spur-IDs und Erklärung – zum Beheben die Noten mit execute/setClipNotes ändern.",
            schema(json!({ "trackIds": { "type": "array", "items": id.clone() } }), &[]), true),
        tool("analyze_score", "Partitur analysieren",
            "Analysiert eine MusicXML/MXL-Datei: Tonart, Stimmung (Valenz/Erregung), Stimmfunktionen (Hauptthema, Gegenstimme, Mittelstimme, Begleitfigur, Ostinato, Bass) und Kadenzen.",
            schema(json!({ "path": { "type": "string" } }), &["path"]), true),
    ]
}

fn parse_kind(a: &Value) -> Result<InstrumentKind, String> {
    let k = a.get("kind").cloned().ok_or("Argument „kind“ fehlt (pluck, drums, bass, organ, harpsichord oder synth)")?;
    serde_json::from_value(k).map_err(|_| "„kind“ muss pluck, drums, bass, organ, harpsichord oder synth sein".to_string())
}

fn u64_arg(a: &Value, k: &str) -> Result<u64, String> {
    a.get(k).and_then(Value::as_u64).ok_or_else(|| format!("Argument „{k}“ fehlt oder ist keine Zahl"))
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(Value::as_str).ok_or_else(|| format!("Argument „{k}“ fehlt"))
}

/// Akkorde aus `chords` (Symbole oder Stufen) einlesen.
fn chords_of(a: &Value, key: &Key) -> Result<Vec<kh::Chord>, String> {
    let Some(list) = a.get("chords").and_then(Value::as_array) else { return Ok(Vec::new()) };
    list.iter()
        .map(|v| {
            let s = v.as_str().ok_or("chords: nur Texte wie „G7“ oder „V7“")?;
            kh::parse_chord(s, key).ok_or_else(|| {
                format!("Akkord „{s}“ nicht verstanden – Symbole mit deutschen Tonnamen (C, a, G7, fis°, Hø7, C/E) oder Stufen (I, ii6, V7, V7/V, N6)")
            })
        })
        .collect()
}

/// Klang neuer Fugenstimmen (Standard Orgel).
fn instrument_of(a: &Value) -> InstrumentKind {
    match a.get("instrument").and_then(Value::as_str) {
        Some("harpsichord") => InstrumentKind::Harpsichord,
        Some("pluck") => InstrumentKind::Pluck,
        _ => InstrumentKind::Organ,
    }
}

fn chord_json(i: &kh::ChordInfo) -> Value {
    json!({ "name": i.name, "stufe": i.roman, "funktion": i.function, "rolle": i.role })
}


impl<S: AgentShell> Agent<S> {
    fn st(&self) -> &AppState {
        self.shell.state()
    }

    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        self.st().host().emit(event, payload);
    }

    /// Tonart aus dem Argument `key`, sonst aus den Noten geschätzt.
    fn key_of(&self, args: &Value) -> Result<KeyInfo, String> {
        let chosen = match args.get("key").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            Some(s) => Some(kh::parse_key(s).ok_or_else(|| format!("Tonart „{s}“ nicht verstanden – z. B. „C-Dur“, „a-Moll“, „Es-Dur“"))?),
            None => None,
        };
        harmony::resolve_key(&self.st(), chosen)
    }

    /// Oberfläche über Änderungen und Aktivität des Agenten informieren.
    fn changed(&self, tool: &str, summary: impl Into<String>) {
        if let Ok(s) = self.st().snapshot() {
            self.emit("project-changed", &s);
        }
        self.activity(tool, summary);
    }

    fn activity(&self, tool: &str, summary: impl Into<String>) {
        self.emit("agent-activity", &json!({ "tool": tool, "summary": summary.into() }));
    }

    fn project_summary(&self, include_notes: bool) -> Result<Value, String> {
        let st = self.st();
        let status = st.status();
        let s = lock(&st.session)?;
        let p = &s.project;
        let mut v = serde_json::to_value(p).map_err(|e| e.to_string())?;
        if let Some(obj) = v.as_object_mut() {
            obj.remove("nextId");
        }
        for (ti, t) in p.tracks.iter().enumerate() {
            for (ci, c) in t.clips.iter().enumerate() {
                let cv = &mut v["tracks"][ti]["clips"][ci];
                cv["lengthBeats"] = json!(c.length_beats(p.tempo_bpm));
                if let ClipSource::Notes { notes, .. } = &c.source {
                    cv["source"]["noteCount"] = json!(notes.len());
                    if !include_notes {
                        cv["source"].as_object_mut().map(|o| o.remove("notes"));
                    }
                }
            }
        }
        v["undo"] = json!(s.undo_label());
        v["redo"] = json!(s.redo_label());
        v["playing"] = json!(status.playing);
        v["positionBeats"] = json!((status.position_secs * p.tempo_bpm / 60.0 * 100.0).round() / 100.0);
        Ok(v)
    }

    fn execute(&self, args: &Value) -> Result<Value, String> {
        let list = args.get("commands").and_then(Value::as_array).ok_or("Argument „commands“ (Liste) fehlt")?;
        if list.is_empty() {
            return Err("„commands“ ist leer".into());
        }
        let mut commands = Vec::with_capacity(list.len());
        for (i, c) in list.iter().enumerate() {
            let cmd: Command = serde_json::from_value(c.clone()).map_err(|e| format!("Befehl {}: {e}", i + 1))?;
            commands.push(cmd);
        }
        if commands.iter().all(Command::is_view_only) {
            // Reine Ansichtsänderungen: einzeln ausführen, kein Undo-Schritt.
            for c in commands {
                self.st().execute(c, false)?;
            }
            self.changed("execute", "Ansicht geändert");
            return Ok(json!({ "ok": true, "undoLabel": null, "project": self.project_summary(false)? }));
        }
        let label = match args.get("label").and_then(Value::as_str) {
            Some(l) if !l.trim().is_empty() => l.trim().to_string(),
            _ if commands.len() == 1 => commands[0].label(),
            _ => format!("{} Änderungen", commands.len()),
        };
        let label = format!("Agent · {label}");
        let snap = self.st().execute(Command::Batch { label: label.clone(), commands }, false)?;
        drop(snap);
        self.changed("execute", label.clone());
        Ok(json!({ "ok": true, "undoLabel": label, "project": self.project_summary(false)? }))
    }

    fn set_sound(&self, args: &Value) -> Result<Value, String> {
        let track_id = Id(u64_arg(args, "trackId")?);
        let track = self.st().instrument_track(track_id)?;
        let empty = serde_json::Map::new();
        let given = args.get("params").and_then(Value::as_object).unwrap_or(&empty);
        let mut inst = track.instrument.clone().unwrap_or_else(Instrument::pluck);
        let mut values = std::collections::BTreeMap::new();
        for (k, v) in given {
            let x = v.as_f64().ok_or_else(|| format!("„{k}“ muss eine Zahl sein"))? as f32;
            values.insert(k.clone(), x);
        }
        let mut values = sound::validate_params(inst.kind, &values)?;
        // Punkt im Charakter-Feld: erst die Zuordnung, dann einzeln genannte Regler darüber
        let cx = values.remove("charakter.x");
        let cy = values.remove("charakter.y");
        if cx.is_some() || cy.is_some() {
            let x = cx.unwrap_or_else(|| inst.param("charakter.x", 0.5));
            let y = cy.unwrap_or_else(|| inst.param("charakter.y", 0.5));
            sound::apply_field(&mut inst, "charakter", x, y)?;
        }
        if let Some(name) = args.get("verlauf").and_then(Value::as_str) {
            sound::apply_verlauf(&mut inst, name)?;
        }
        if let Some(c) = args.get("curves") {
            if inst.kind != InstrumentKind::Synth {
                return Err("Kurven gibt es nur beim Synth.".into());
            }
            let curves: std::collections::BTreeMap<String, Vec<[f32; 2]>> =
                serde_json::from_value(c.clone()).map_err(|_| "curves: { laut: [[Sekunde, Wert], …], hell: […] }".to_string())?;
            inst.curves.extend(sound::validate_curves(&curves)?);
        }
        if values.is_empty() && cx.is_none() && cy.is_none() && args.get("verlauf").is_none() && args.get("curves").is_none() {
            return Err("Nichts zu ändern – params, verlauf oder curves angeben.".into());
        }
        inst.params.extend(values);
        inst.preset = args.get("preset").and_then(Value::as_str).unwrap_or("agent").to_string();
        let label = "Agent · Klang ändern".to_string();
        let cmd = Command::Batch {
            label: label.clone(),
            commands: vec![Command::SetTrack { track_id, patch: TrackPatch { instrument: Some(inst), ..Default::default() } }],
        };
        self.st().execute(cmd, false)?;
        let report = self.st().describe_track(track_id)?;
        let words: Vec<String> = report.anzeige.iter().map(|m| format!("{} {}", m.label, m.wort)).collect();
        self.changed("set_sound", format!("Klang „{}“: {}", track.name, words.join(", ")));
        let mut out = serde_json::to_value(&report).map_err(|e| e.to_string())?;
        if let Some(p) = args.get("preview").and_then(Value::as_str) {
            out["vorgespielt"] = json!(self.st().preview_track(track_id, p)?);
        }
        Ok(out)
    }

    fn search(&self, args: &Value) -> Result<Value, String> {
        let st = self.st();
        let query: Vec<MelodyNote> = if let Some(notes) = args.get("notes").and_then(Value::as_array) {
            let mut t = 0.0;
            notes
                .iter()
                .map(|n| {
                    let midi = n.get("midi").and_then(Value::as_u64).ok_or("Jede Note braucht „midi“")? as u8;
                    let d = n.get("duration").and_then(Value::as_f64).ok_or("Jede Note braucht „duration“")?;
                    let note = MelodyNote { onset: t, duration: d, midi };
                    t += d;
                    Ok(note)
                })
                .collect::<Result<_, &str>>()?
        } else {
            let (_, a) = st.clip_analysis(Id(u64_arg(args, "trackId")?), Id(u64_arg(args, "clipId")?))?;
            a.notes.iter().map(|n| MelodyNote { onset: n.start_secs, duration: n.end_secs - n.start_secs, midi: n.midi }).collect()
        };
        if query.len() < MIN_QUERY_NOTES {
            return Err(format!("Mindestens {MIN_QUERY_NOTES} Noten nötig (erhalten: {}).", query.len()));
        }
        let lib = lock(&st.library)?;
        let lib = lib.as_ref().ok_or("Keine Bibliothek geladen – sie wird beim Start geladen; mit library_status nachsehen.")?;
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
        let hits = lib.index.search(&query, limit);
        self.activity("search_melody", format!("Melodiesuche: {} Treffer", hits.len()));
        Ok(json!({ "query": query.iter().map(|n| note_name(n.midi)).collect::<Vec<_>>(), "hits": hits }))
    }
}

impl<S: AgentShell> Handler for Agent<S> {
    fn info(&self) -> ServerInfo {
        ServerInfo {
            name: "klangwelt".into(),
            title: "Klangwelt".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            instructions: INSTRUCTIONS.into(),
        }
    }

    fn tools(&self) -> Vec<Tool> {
        tools(&self.shell.pack_ids())
    }

    fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        let mut v = self.dispatch(name, args)?;
        tidy(&mut v);
        Ok(v)
    }

    fn prompts(&self) -> Vec<Prompt> {
        vec![Prompt {
            name: "zuhoeren",
            title: "Im Klangwelt-Chat zuhören",
            description: "Startet einen Subagenten, der Aufträge aus dem Chat der Klangwelt-Oberfläche abholt und erledigt.",
            text: format!(
                "Starte einen Subagenten, der im Klangwelt-Chat zuhört – möglichst im Hintergrund, damit wir weiter reden können \
                 (Typ „klangwelt-zuhoerer“, falls vorhanden, sonst ein allgemeiner Subagent mit Zugriff auf die Klangwelt-Werkzeuge). \
                 Gib ihm diesen Auftrag wörtlich mit:\n\n{LISTENER_BRIEF}\n\n\
                 Sag mir kurz Bescheid, wenn er läuft. Endet er, ohne dass im Chat „Stopp“ stand, starte einen neuen."
            ),
        }]
    }
}

/// Rundet Kommazahlen auf 4 Stellen (f32-Werte erscheinen sonst als 0.550000011920929).
fn tidy(v: &mut Value) {
    match v {
        Value::Number(n) if n.is_f64() => {
            let x = n.as_f64().unwrap_or(0.0);
            if let Some(r) = serde_json::Number::from_f64((x * 10_000.0).round() / 10_000.0) {
                *n = r;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(tidy),
        Value::Object(o) => o.values_mut().for_each(tidy),
        _ => {}
    }
}

impl<S: AgentShell> Agent<S> {
    fn dispatch(&self, name: &str, args: Value) -> Result<Value, String> {
        let st = self.st();
        match name {
            "get_project" => self.project_summary(args.get("includeNotes").and_then(Value::as_bool).unwrap_or(false)),
            "execute" => self.execute(&args),
            "undo" | "redo" => {
                let label = { let s = lock(&st.session)?; if name == "undo" { s.undo_label() } else { s.redo_label() }.map(str::to_string) };
                let Some(label) = label else { return Ok(json!({ "ok": false, "info": "Nichts zu tun" })) };
                let snap = if name == "undo" { st.undo()? } else { st.redo()? };
                drop(snap);
                self.changed(name, format!("{}: {label}", if name == "undo" { "Rückgängig" } else { "Wiederholt" }));
                Ok(json!({ "ok": true, "schritt": label }))
            }
            "open_project" => {
                let path = std::path::PathBuf::from(str_arg(&args, "path")?.trim().trim_matches('"'));
                if !crate::vfs::exists(&path) || crate::vfs::is_dir(&path) {
                    return Err(format!("Datei nicht gefunden: {}", path.display()));
                }
                // Das Fenster öffnet über seinen normalen Weg (fragt bei ungespeicherten Änderungen)
                self.shell.open_in_window(&path)?;
                self.activity("open_project", format!("Projekt öffnen: {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
                Ok(json!({ "ok": true, "hinweis": "Im Fenster geöffnet – bei ungespeicherten Änderungen fragt Klangwelt erst nach." }))
            }
            "transport" => {
                if let Some(b) = args.get("positionBeats").and_then(Value::as_f64) {
                    st.seek_seconds(b * 60.0 / st.tempo())?;
                }
                if let Some(a) = args.get("action").and_then(Value::as_str) {
                    st.transport(a)?;
                    self.activity("transport", match a { "play" => "Wiedergabe", "stop" => "Stopp", "rewind" => "Zum Anfang", _ => "Wiedergabe umgeschaltet" });
                }
                settle();
                let s = st.status();
                Ok(json!({ "playing": s.playing, "positionBeats": s.position_secs * st.tempo() / 60.0 }))
            }
            "set_metronome" => {
                let on = args.get("enabled").and_then(Value::as_bool).ok_or("Argument „enabled“ fehlt")?;
                st.set_metronome(on)?;
                self.changed("set_metronome", if on { "Metronom an" } else { "Metronom aus" });
                Ok(json!({ "metronome": on }))
            }
            "audio_devices" => serde_json::to_value(st.audio_devices()?).map_err(|e| e.to_string()),
            "set_audio_device" => {
                let kind = str_arg(&args, "kind")?;
                let name = args.get("name").and_then(Value::as_str).map(str::to_string);
                st.set_audio_device(kind, name)?;
                let d = st.audio_devices()?;
                let (gewaehlt, wort) = if matches!(kind, "eingabe" | "input") { (d.input.clone(), "Aufnahme") } else { (d.output.clone(), "Ausgabe") };
                self.changed("set_audio_device", format!("{wort}: {}", gewaehlt.as_deref().unwrap_or("Systemstandard")));
                serde_json::to_value(d).map_err(|e| e.to_string())
            }
            "export_project" => {
                let opts: crate::export::ExportOptions = serde_json::from_value(args.clone()).map_err(|e| format!("Argumente: {e}"))?;
                let r = crate::export::run(&crate::export::export_dir(&st), &st, &opts)?;
                self.activity("export_project", format!("Exportiert: {}", r.path));
                serde_json::to_value(r).map_err(|e| e.to_string())
            }
            "describe_sound" => serde_json::to_value(st.describe_track(Id(u64_arg(&args, "trackId")?))?).map_err(|e| e.to_string()),
            "set_sound" => self.set_sound(&args),
            "set_sound_field" => {
                let track_id = Id(u64_arg(&args, "trackId")?);
                let field = str_arg(&args, "field")?;
                let preset = args.get("preset").and_then(Value::as_str);
                let xy = match (args.get("x").and_then(Value::as_f64), args.get("y").and_then(Value::as_f64)) {
                    (Some(x), Some(y)) => Some((x as f32, y as f32)),
                    (None, None) => None,
                    _ => return Err("x und y zusammen angeben (oder einen Startpunkt)".into()),
                };
                st.sound_field(track_id, field, xy, preset, false)?;
                let report = st.describe_track(track_id)?;
                let words: Vec<String> = report.anzeige.iter().map(|m| format!("{} {}", m.label, m.wort)).collect();
                self.changed("set_sound_field", format!("Feld {field}: {}", words.join(", ")));
                let mut out = serde_json::to_value(&report).map_err(|e| e.to_string())?;
                if let Some(p) = args.get("preview").and_then(Value::as_str) {
                    out["vorgespielt"] = json!(st.preview_track(track_id, p)?);
                }
                Ok(out)
            }
            "list_song_templates" => Ok(json!({ "vorlagen": song::templates().iter().map(|t| json!({
                "template": t.template, "name": t.name, "beschreibung": t.description, "tempo": t.tempo, "tonart": t.key,
                "akkorde": t.chords, "taktJeAkkord": t.chord_bars, "takte": t.total_bars(),
                "rollen": t.layers.iter().map(|l| json!({ "key": l.key, "name": l.name, "rolle": l.role, "klang": l.sound })).collect::<Vec<_>>(),
                "abschnitte": t.sections.iter().map(|s| json!({ "name": s.name, "takte": s.bars, "art": s.kind, "levels": s.levels, "stufen": song::levels_text(s, t) })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>() })),
            "write_song" => {
                let mut plan = song::template(str_arg(&args, "template")?)?;
                if let Some(t) = args.get("tempo").and_then(Value::as_f64) {
                    plan.tempo = t;
                }
                if let Some(k) = args.get("key").and_then(Value::as_str) {
                    plan.key = k.to_string();
                }
                if let Some(c) = args.get("chords") {
                    plan.chords = serde_json::from_value(c.clone()).map_err(|_| "chords: Liste von Texten".to_string())?;
                }
                if let Some(n) = args.get("chordBars").and_then(Value::as_u64) {
                    plan.chord_bars = n.clamp(1, 8) as u32;
                }
                if let Some(s) = args.get("sections") {
                    plan.sections = serde_json::from_value(s.clone()).map_err(|e| format!("sections: [{{ name, bars, kind, levels }}] – {e}"))?;
                }
                let replace = args.get("replace").and_then(Value::as_bool).unwrap_or(true);
                let w = st.song_write(&plan, replace, "Agent · ")?;
                self.changed("write_song", format!("Song „{}“: {} Takte", plan.name, w.bars));
                if args.get("play").and_then(Value::as_bool).unwrap_or(false) {
                    st.seek_seconds(0.0)?;
                    st.transport("play")?;
                }
                Ok(json!({ "ok": true, "spuren": w.track_ids, "ersetzt": w.replaced, "takte": w.bars, "sekunden": (w.seconds * 10.0).round() / 10.0,
                    "abschnitte": plan.sections.iter().map(|s| format!("{} ({} T.): {}", s.name, s.bars, song::levels_text(s, &plan))).collect::<Vec<_>>() }))
            }
            "list_drum_patterns" => {
                let info = rhythm::info();
                let want = args.get("genre").and_then(Value::as_str).map(str::to_lowercase);
                let genres: Vec<Value> = info
                    .genres
                    .iter()
                    .filter(|g| want.as_ref().map_or(true, |w| g.name.to_lowercase().contains(w.as_str())))
                    .map(|g| json!({
                        "name": g.name, "beschreibung": g.blurb, "klang": g.sound,
                        "muster": g.patterns.iter().map(|p| json!({
                            "name": p.name, "bpm": p.bpm, "swing": p.swing, "tipp": p.tip,
                            "zeilen": p.grid.rows.iter().map(|(k, v)| (k.clone(), json!(rhythm::row_text(v, p.grid.steps)))).collect::<serde_json::Map<_, _>>(),
                        })).collect::<Vec<_>>(),
                    }))
                    .collect();
                if genres.is_empty() {
                    return Err(format!("Kein Genre „{}“ (vorhanden: {})", want.unwrap_or_default(), info.genres.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", ")));
                }
                Ok(json!({ "trommeln": info.rows.iter().map(|r| json!({ "key": r.key, "name": r.label, "taste": r.midi })).collect::<Vec<_>>(), "genres": genres }))
            }
            "write_drum_pattern" => {
                let bars = args.get("bars").and_then(Value::as_u64).unwrap_or(4).clamp(1, rhythm::MAX_BARS as u64) as usize;
                let steps = bars * rhythm::STEPS_PER_BAR;
                let track_id = args.get("trackId").and_then(Value::as_u64).map(Id);
                let clip_id = args.get("clipId").and_then(Value::as_u64).map(Id);
                let (grid, name, bpm, swing_default, sound) = match args.get("rows") {
                    Some(rows) => (rhythm::grid_from_json(rows, steps)?, "Beat".to_string(), None, 0.0, None),
                    None => {
                        let genre = args.get("genre").and_then(Value::as_str);
                        let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or("");
                        if genre.is_none() && pattern.is_empty() {
                            return Err("genre/pattern oder rows angeben (list_drum_patterns zeigt alle Muster)".into());
                        }
                        let (g, p) = rhythm::find_pattern(genre, pattern)?;
                        (p.grid.resized(steps), p.name.clone(), Some(p.bpm as f64), p.swing as f64, Some(g.sound.clone()))
                    }
                };
                let swing = args.get("swing").and_then(Value::as_f64).unwrap_or(swing_default).clamp(0.0, 100.0);
                let tempo = if args.get("setTempo").and_then(Value::as_bool).unwrap_or(false) { bpm } else { None };
                let start = match args.get("startBeats").and_then(Value::as_f64) {
                    Some(b) => b,
                    None => rhythm::free_start(&st, track_id),
                };
                let w = st.rhythm_write(grid.clone(), swing, track_id, clip_id, start, &name, sound.as_deref(), tempo, "Agent · ")?;
                self.changed("write_drum_pattern", format!("Rhythmus „{name}“ geschrieben"));
                let mut out = json!({ "ok": true, "trackId": w.track_id, "clipId": w.clip_id, "name": name, "startBeats": start, "takte": bars, "swing": swing,
                    "zeilen": grid.rows.iter().map(|(k, v)| (k.clone(), json!(rhythm::row_text(v, steps)))).collect::<serde_json::Map<_, _>>() });
                if let Some(t) = tempo {
                    out["tempo"] = json!(t);
                }
                if args.get("preview").and_then(Value::as_bool).unwrap_or(false) {
                    out["vorgespielt"] = json!(st.rhythm_preview(Some(w.track_id), Some(w.clip_id), None, swing, None)?);
                }
                Ok(out)
            }
            "read_drum_pattern" => {
                let r = st.rhythm_read(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?))?;
                Ok(json!({ "name": r.name, "schritte": r.grid.steps, "takte": r.grid.steps / rhythm::STEPS_PER_BAR, "swing": r.swing, "nichtImRaster": r.skipped,
                    "zeilen": r.grid.rows.iter().map(|(k, v)| (k.clone(), json!(rhythm::row_text(v, r.grid.steps)))).collect::<serde_json::Map<_, _>>() }))
            }
            "list_saved_sounds" => {
                let path = sound_library::file(&self.st());
                let want = if args.get("kind").is_some() { Some(parse_kind(&args)?) } else { None };
                let klaenge: Vec<Value> = sound_library::list(&path)?
                    .iter()
                    .filter(|e| want.map_or(true, |k| e.kind == k))
                    .map(|e| json!({ "id": e.id, "name": e.name, "kind": e.kind, "instrument": e.kind_name, "beschreibung": e.summary }))
                    .collect();
                Ok(json!({ "klaenge": klaenge, "datei": path.display().to_string() }))
            }
            "save_sound" => {
                let inst = st.track_sound(Id(u64_arg(&args, "trackId")?))?;
                let r = sound_library::save(&sound_library::file(&self.st()), str_arg(&args, "name")?, &inst)?;
                sound_library::announce(&self.st(), &r.sounds);
                self.activity("save_sound", format!("Klang „{}“ {}", r.name, if r.replaced { "ersetzt" } else { "gesichert" }));
                let summary = r.sounds.iter().find(|e| e.id == r.id).map(|e| e.summary.clone());
                Ok(json!({ "ok": true, "id": r.id, "name": r.name, "ersetzt": r.replaced, "beschreibung": summary }))
            }
            "load_saved_sound" => {
                let track_id = Id(u64_arg(&args, "trackId")?);
                let prefer = st.track_sound(track_id)?.kind;
                let saved = sound_library::find(&sound_library::file(&self.st()), str_arg(&args, "sound")?, Some(prefer))?;
                st.load_sound(track_id, &saved, "Agent · ")?;
                self.changed("load_saved_sound", format!("Klang „{}“ geladen", saved.name));
                let mut out = serde_json::to_value(st.describe_track(track_id)?).map_err(|e| e.to_string())?;
                out["geladen"] = json!(saved.name);
                if let Some(p) = args.get("preview").and_then(Value::as_str) {
                    out["vorgespielt"] = json!(st.preview_track(track_id, p)?);
                }
                Ok(out)
            }
            "rename_saved_sound" => {
                let path = sound_library::file(&self.st());
                let saved = sound_library::find(&path, str_arg(&args, "sound")?, None)?;
                let list = sound_library::rename(&path, &saved.id, str_arg(&args, "name")?)?;
                sound_library::announce(&self.st(), &list);
                let name = list.iter().find(|e| e.id == saved.id).map(|e| e.name.clone()).unwrap_or_default();
                self.activity("rename_saved_sound", format!("Klang „{}“ heißt jetzt „{name}“", saved.name));
                Ok(json!({ "ok": true, "id": saved.id, "name": name }))
            }
            "delete_saved_sound" => {
                let path = sound_library::file(&self.st());
                let saved = sound_library::find(&path, str_arg(&args, "sound")?, None)?;
                let list = sound_library::delete(&path, &saved.id)?;
                sound_library::announce(&self.st(), &list);
                self.activity("delete_saved_sound", format!("Klang „{}“ gelöscht", saved.name));
                Ok(json!({ "ok": true, "geloescht": saved.name, "uebrig": list.len() }))
            }
            "chat_poll" => {
                // Höchstens 590 s: die stdio-Brücke wartet bis zu 600 s auf eine Antwort.
                let secs = args.get("timeoutSeconds").and_then(Value::as_f64).unwrap_or(50.0).clamp(1.0, 590.0);
                let notify = || self.emit("chat-changed", &self.st().chat_view());
                let polled = st.chat.poll(std::time::Duration::from_secs_f64(secs), notify, klang_mcp::caller_gone);
                let tasks = polled.tasks;
                // Hat der Client einen früheren Poll abgebrochen, ist sein Timeout kürzer als die Wartezeit.
                let abbruch = polled.abandoned_before.map(|s| {
                    format!(
                        "Ein früherer chat_poll wurde nach {s:.0} s vom Client abgebrochen (Client-Timeout). \
                         Nimm timeoutSeconds {} oder erhöhe den Timeout des Klangwelt-Servers (\"timeout\": 330000 in der MCP-Konfiguration).",
                        ((s - 10.0).floor() as i64).clamp(10, 50)
                    )
                });
                // Leere Antworten knapp halten: ein zuhörender Subagent pollt stundenlang.
                if tasks.is_empty() {
                    let mut r = json!({ "auftraege": [], "hinweis": "Keine neuen Aufträge – chat_poll erneut aufrufen." });
                    if let Some(a) = abbruch {
                        r["abbruch"] = json!(a);
                    }
                    return Ok(r);
                }
                let verlauf: Vec<Value> = st
                    .chat
                    .recent(12)
                    .iter()
                    .map(|m| json!({ "id": m.id, "von": if m.from == crate::chat::Author::User { "nutzer" } else { "agent" }, "text": m.text }))
                    .collect();
                let mut r = json!({
                    "auftraege": tasks.iter().map(|m| json!({ "id": m.id, "text": m.text, "kontext": m.kontext })).collect::<Vec<_>>(),
                    "hinweis": "Aufträge ausführen, dann mit chat_reply (replyTo = id) antworten und wieder chat_poll aufrufen.",
                    "verlauf": verlauf,
                });
                if let Some(a) = abbruch {
                    r["abbruch"] = json!(a);
                }
                Ok(r)
            }
            "chat_reply" => {
                let text = str_arg(&args, "text")?;
                let reply_to = args.get("replyTo").and_then(Value::as_u64);
                let done = args.get("done").and_then(Value::as_bool).unwrap_or(true);
                let m = st.chat.reply(text, reply_to, done)?;
                self.emit("chat-changed", &st.chat_view());
                Ok(json!({ "ok": true, "id": m.id }))
            }
            "list_instruments" => serde_json::to_value(sound::specs()).map(|v| json!({ "instrumente": v })).map_err(|e| e.to_string()),
            "create_instrument_track" => {
                let kind = parse_kind(&args)?;
                let (_, id) = st.create_instrument_track(kind, &format!("Agent · {} anlegen", sound::kind_name(kind)))?;
                self.changed("create_instrument_track", format!("{}-Spur mit Muster angelegt", sound::kind_name(kind)));
                Ok(json!({ "ok": true, "trackId": id, "instrument": sound::kind_name(kind) }))
            }
            "set_instrument" => {
                let track_id = Id(u64_arg(&args, "trackId")?);
                let track = st.instrument_track(track_id)?;
                let kind = parse_kind(&args)?;
                let mut inst = sound::default_instrument(kind);
                if let Some(name) = args.get("preset").and_then(Value::as_str) {
                    let spec = sound::spec(kind);
                    let preset = spec.presets.iter().find(|p| p.name.eq_ignore_ascii_case(name)).ok_or_else(|| {
                        let names: Vec<&str> = spec.presets.iter().map(|p| p.name).collect();
                        format!("Unbekanntes Preset „{name}“. Vorhanden: {}", names.join(", "))
                    })?;
                    for (k, v) in &preset.params {
                        inst.params.insert(k.to_string(), *v);
                    }
                    inst.preset = preset.name.to_string();
                }
                let label = format!("Agent · {} für „{}“", sound::kind_name(kind), track.name);
                st.execute(
                    Command::Batch { label: label.clone(), commands: vec![Command::SetTrack { track_id, patch: TrackPatch { instrument: Some(inst), ..Default::default() } }] },
                    false,
                )?;
                self.changed("set_instrument", label);
                serde_json::to_value(st.describe_track(track_id)?).map_err(|e| e.to_string())
            }
            "preview_sound" => {
                let track_id = Id(u64_arg(&args, "trackId")?);
                let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or("akkord");
                let secs = st.preview_track(track_id, pattern)?;
                self.activity("preview_sound", format!("Vorhören: {pattern}"));
                Ok(json!({ "spielt": pattern, "sekunden": secs }))
            }
            "analyze_clip" => {
                let (clip, a) = st.clip_analysis(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?))?;
                Ok(json!({
                    "clip": clip.name,
                    "dauerSekunden": a.duration_secs,
                    "tonart": a.key.as_ref().map(|k| &k.name),
                    "tempoSchaetzung": a.tempo_estimate,
                    "noten": a.notes.iter().map(|n| json!({
                        "midi": n.midi, "name": note_name(n.midi), "start": (n.start_secs * 1000.0).round() / 1000.0,
                        "ende": (n.end_secs * 1000.0).round() / 1000.0, "korrigiert": n.midi != n.raw_midi })).collect::<Vec<_>>(),
                    "anschlaege": a.hits.iter().map(|h| json!({
                        "start": (h.time * 1000.0).round() / 1000.0,
                        "grundton": h.midi.map(|m| note_name(m.round().clamp(0.0, 127.0) as u8)),
                        "midi": h.midi, "staerke": h.strength })).collect::<Vec<_>>()
                }))
            }
            "melody_to_track" => {
                let (_, new_id) = st.melody_to_track(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?), "Agent · Melodie übernehmen")?;
                self.changed("melody_to_track", "Melodie als Instrumentspur angelegt");
                Ok(json!({ "ok": true, "trackId": new_id }))
            }
            "audio_to_notes" => {
                let target: crate::convert::Target = serde_json::from_value(args.get("target").cloned().unwrap_or(Value::Null))
                    .map_err(|_| "„target“ muss 'melodie' oder 'rhythmus' sein".to_string())?;
                let clip = args.get("clipId").and_then(Value::as_u64).map(Id);
                let label = match target {
                    crate::convert::Target::Melodie => "Agent · In Melodie-Spur umwandeln",
                    crate::convert::Target::Rhythmus => "Agent · In Rhythmus-Spur umwandeln",
                };
                let r = st.audio_to_notes(Id(u64_arg(&args, "trackId")?), clip, target, label)?;
                self.changed("audio_to_notes", format!("Aufnahme umgewandelt: {} Noten", r.notes));
                Ok(json!({ "ok": true, "trackId": r.track_id, "noten": r.notes }))
            }
            "import_audio" => {
                let paths: Vec<String> = args.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
                if paths.is_empty() {
                    return Err("Argument „paths“ fehlt (Liste von Dateipfaden)".into());
                }
                // über das Dateisystem des Kerns: Desktop die Platte, Browser hereingezogene/hochgeladene Dateien
                let files = paths
                    .into_iter()
                    .map(|p| {
                        let p = p.trim().trim_matches('"').to_string();
                        let bytes = crate::vfs::read(Path::new(&p)).map_err(|e| format!("Datei lässt sich nicht lesen ({e})"));
                        (p, bytes)
                    })
                    .collect();
                let track = args.get("trackId").and_then(Value::as_u64).map(Id);
                let start = args.get("startBeats").and_then(Value::as_f64).unwrap_or(0.0);
                let r = st.import_audio(files, track, start)?;
                self.changed("import_audio", format!("{} Audiodatei(en) eingefügt", r.imported.len()));
                Ok(json!({ "ok": true, "eingefuegt": r.imported, "fehler": r.errors }))
            }
            "delete_range" => {
                let num = |k: &str| args.get(k).and_then(Value::as_f64).ok_or_else(|| format!("Argument „{k}“ fehlt"));
                let (a, b) = (num("startBeats")?, num("endBeats")?);
                drop(st.delete_range(a, b)?);
                self.changed("delete_range", format!("Zeitbereich {a}–{b} gelöscht, Rest aufgerückt"));
                Ok(json!({ "ok": true, "geloescht": [a.min(b), a.max(b)], "aufgerueckt": (a - b).abs() }))
            }
            "quantize_notes" => {
                let grid = args.get("grid").and_then(Value::as_f64).ok_or("Argument „grid“ fehlt (Schläge, z. B. 0.5 = Achtel)")?;
                let lengths = args.get("lengths").and_then(Value::as_bool).unwrap_or(false);
                let indices = args.get("notes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|i| i as usize).collect());
                let r = st.quantize_notes(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?), indices, grid, lengths, Some("Agent · Noten am Raster ausrichten"))?;
                self.changed("quantize_notes", "Noten am Raster ausgerichtet");
                Ok(json!({ "ok": true, "ausgerichtet": r.selected.len() }))
            }
            "download_library" => {
                let id = str_arg(&args, "id")?;
                match self.shell.library_download(id)? {
                    (dir, None) => {
                        self.activity("download_library", format!("Paket wird geladen: {id}"));
                        Ok(json!({ "ok": true, "ordner": dir.display().to_string(), "hinweis": "Läuft im Hintergrund und landet in der aktiven Bibliothek – mit library_status nachsehen; danach search_melody." }))
                    }
                    (dir, Some(platz)) => {
                        self.activity("download_library", format!("Paket eingereiht: {id} (Platz {platz})"));
                        Ok(json!({ "ok": true, "warteschlange": platz, "ordner": dir.display().to_string(), "hinweis": "Gerade läuft ein anderer Auftrag – das Paket startet danach von selbst und landet in der jetzt aktiven Bibliothek. Mit library_status (queue) nachsehen." }))
                    }
                }
            }
            "add_to_library" => {
                let paths: Vec<String> = args.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let name = self.shell.library_add(&paths)?;
                self.activity("add_to_library", format!("Füge {} Datei(en) zu „{name}“ hinzu", paths.len()));
                Ok(json!({ "ok": true, "bibliothek": name, "hinweis": "Läuft im Hintergrund (wenige Sekunden) – mit library_status nachsehen." }))
            }
            "library_status" => serde_json::to_value(crate::library::info(&st)?).map_err(|e| e.to_string()),
            "browse_library" => {
                let strings = |key: &str| -> Vec<String> {
                    args.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
                };
                let levels = strings("levels");
                let path = strings("path");
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
                let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                serde_json::to_value(crate::library::tree(&st, &levels, &path, offset, limit)?).map_err(|e| e.to_string())
            }
            "list_libraries" => Ok(self.shell.libraries()),
            "switch_library" => {
                self.shell.library_switch(str_arg(&args, "library")?)?;
                crate::library::emit_info(st);
                let (name, scores) = lock(&st.library)?.as_ref().map(|l| (l.name.clone(), l.index.score_count())).unwrap_or_default();
                self.changed("switch_library", format!("Bibliothek „{name}“ aktiv"));
                Ok(json!({ "ok": true, "aktiv": name, "partituren": scores }))
            }
            "create_library" => {
                let activate = args.get("activate").and_then(Value::as_bool).unwrap_or(true);
                let (id, name) = self.shell.library_create(str_arg(&args, "name")?, activate)?;
                crate::library::emit_info(st);
                self.changed("create_library", format!("Bibliothek „{name}“ angelegt"));
                Ok(json!({ "ok": true, "id": id, "name": name }))
            }
            "delete_library" => {
                let id = self.shell.library_delete(str_arg(&args, "library")?)?;
                crate::library::emit_info(st);
                self.changed("delete_library", format!("Bibliothek {id} gelöscht"));
                Ok(json!({ "ok": true, "geloescht": id }))
            }
            "search_melody" => self.search(&args),
            "analyze_harmony" => {
                let key = self.key_of(&args)?;
                let track = args.get("trackId").and_then(Value::as_u64).map(Id);
                let clip = args.get("clipId").and_then(Value::as_u64).map(Id);
                let chords = harmony::chords_in(&st, track, clip, &key.key)?;
                let n = chords.len();
                let mut prev: Option<kh::Chord> = None;
                let akkorde: Vec<Value> = chords.iter().take(128).map(|p| {
                    let ch = p.info.chord;
                    let mood = kh::transition_mood(prev.as_ref(), &ch, &key.key);
                    let bruch = kh::harmonic_break(prev.as_ref(), &ch, &key.key);
                    prev = Some(ch);
                    let mut j = json!({ "startBeats": p.start_beats, "lengthBeats": p.length_beats,
                        "name": p.info.name, "stufe": p.info.roman, "funktion": p.info.function, "stimmung": mood.label() });
                    if let Some(b) = bruch {
                        j["bruch"] = json!(b);
                    }
                    j
                }).collect();
                Ok(json!({ "tonart": key.name, "quelle": key.source, "akkorde": akkorde, "anzahl": n }))
            }
            "suggest_chords" => {
                let key = self.key_of(&args)?;
                let chords = chords_of(&args, &key.key)?;
                let group = match args.get("corpus").and_then(Value::as_str) {
                    Some("barock") => 1,
                    Some("bach") => 2,
                    _ => 0,
                };
                let res = harmony::suggest_with_library(&st, &key.key, &chords, group)?;
                let mut r = json!({
                    "tonart": key.name,
                    "folge": chords.iter().map(|c| chord_json(&kh::describe(c, &key.key))).collect::<Vec<_>>(),
                    "vorschlaege": res.suggestions.iter().map(|sv| {
                        let s = &sv.suggestion;
                        let mut v = chord_json(&s.info);
                        v["art"] = json!(s.category);
                        v["warum"] = json!(s.reason);
                        v["wertung"] = json!(s.score);
                        v["stimmung"] = json!(s.mood.label());
                        if let Some(k) = &s.to_key {
                            v["ausweichenNach"] = json!(k);
                        }
                        if let Some(f) = &sv.library {
                            v["bibliothek"] = json!({
                                "anteil": f.share, "anzahl": f.count, "von": f.total,
                                "form": if f.exact { "genau" } else { "Grundakkord" },
                                "beispiele": f.examples.iter().take(3).map(|e| json!({
                                    "titel": e.title, "komponist": e.composer, "takt": e.bar, "doc": e.doc })).collect::<Vec<_>>(),
                            });
                        }
                        v
                    }).collect::<Vec<_>>(),
                    // mehrschrittige Wege vom letzten Akkord zu den Schlüssen
                    "wegeZumSchluss": res.paths.iter().map(|p| {
                        let mut v = json!({
                            "schluss": p.path.label,
                            "akkorde": p.path.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                            "stufen": p.path.steps.iter().map(|s| s.roman.as_str()).collect::<Vec<_>>(),
                            "stimmungen": p.path.moods.iter().map(|m| m.label()).collect::<Vec<_>>(),
                            "wertung": p.path.score,
                        });
                        if let Some(sh) = &p.shares {
                            v["anteileBibliothek"] = json!(sh);
                        }
                        v
                    }).collect::<Vec<_>>(),
                });
                match (&res.library, &res.hint) {
                    (Some(l), h) => {
                        r["bibliothek"] = json!({ "name": l.name, "partituren": l.scores, "gruppe": GROUPS[l.group].1 });
                        if let Some(h) = h {
                            r["hinweis"] = json!(h);
                        }
                    }
                    (None, Some(h)) => r["hinweis"] = json!(h),
                    _ => {}
                }
                if args.get("cadences").and_then(Value::as_bool).unwrap_or(false) {
                    r["kadenzen"] = json!(kh::cadences(&key.key).iter().map(|t| json!({
                        "id": t.id, "name": t.name, "beschreibung": t.description,
                        "akkorde": t.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                        "stufen": t.steps.iter().map(|s| s.roman.as_str()).collect::<Vec<_>>() })).collect::<Vec<_>>());
                }
                Ok(r)
            }
            "write_chords" | "preview_chords" => {
                let key = self.key_of(&args)?;
                let chords = chords_of(&args, &key.key)?;
                let bpb = lock(&st.session)?.project.beats_per_bar as f64;
                let bpc = args.get("beatsPerChord").and_then(Value::as_f64).unwrap_or(bpb);
                let track = args.get("trackId").and_then(Value::as_u64).map(Id);
                let names: Vec<String> = chords.iter().map(|c| kh::chord_name(c, &key.key)).collect();
                if name == "preview_chords" {
                    let secs = harmony::preview(&st, track, &chords, &key.key, bpc, None)?;
                    self.activity("preview_chords", format!("Vorhören: {}", names.join(" – ")));
                    return Ok(json!({ "spielt": names, "sekunden": secs }));
                }
                let start = args.get("startBeats").and_then(Value::as_f64).unwrap_or(0.0);
                let label = format!("Agent · Akkorde {}", names.join(" – "));
                let insert = args.get("mode").and_then(Value::as_str) == Some("einfuegen");
                let (_, w) = harmony::write_mode(&st, track, start, &chords, &key.key, bpc, None, &label, insert)?;
                self.changed("write_chords", label);
                if args.get("preview").and_then(Value::as_bool).unwrap_or(false) {
                    harmony::preview(&st, Some(w.track_id), &chords, &key.key, bpc, None)?;
                }
                Ok(json!({
                    "ok": true, "tonart": key.name, "trackId": w.track_id, "clipId": w.clip_id,
                    "startBeats": w.start_beats, "lengthBeats": w.length_beats,
                    "akkorde": chords.iter().zip(&w.voicings).map(|(c, v)| {
                        let mut j = chord_json(&kh::describe(c, &key.key));
                        j["noten"] = json!(v.iter().map(|m| note_name(*m)).collect::<Vec<_>>());
                        j
                    }).collect::<Vec<_>>(),
                }))
            }
            "fugue_subject" => {
                let (tid, cid) = (Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?));
                let chosen = self.key_of(&args).ok().filter(|k| k.source == "gewaehlt").map(|k| k.key);
                let v = crate::fugue::subject_view(&st, tid, cid, chosen)?;
                let names = |n: &[VNote]| n.iter().map(|x| note_name(x.pitch)).collect::<Vec<_>>();
                Ok(json!({
                    "tonart": v.key.name, "laengeSchlaege": v.length, "umfang": [note_name(v.range.0), note_name(v.range.1)],
                    "thema": names(&v.notes),
                    "antwort": { "art": v.answer.kind, "grund": v.answer.reason, "noten": names(&v.answer.notes), "veraendert": v.answer.mutated },
                    "engfuehrungen": v.strettos.iter().map(|s| json!({ "intervall": s.label, "abstandSchlaege": s.delay,
                        "ueberlappung": (s.overlap * 100.0).round() / 100.0, "warnungen": s.warnings })).collect::<Vec<_>>(),
                    "verwandlungen": v.transforms.iter().map(|t| t.label.clone()).collect::<Vec<_>>(),
                }))
            }
            "transform_melody" => {
                let (tid, cid) = (Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?));
                let chosen = self.key_of(&args).ok().filter(|k| k.source == "gewaehlt").map(|k| k.key);
                let v = crate::fugue::subject_view(&st, tid, cid, chosen)?;
                let steps = args.get("steps").and_then(Value::as_i64).unwrap_or(0) as i32;
                let t = match str_arg(&args, "transform")? {
                    "umkehrung" => Transform::Umkehrung,
                    "krebs" => Transform::Krebs,
                    "krebsumkehrung" => Transform::Krebsumkehrung,
                    "vergroesserung" => Transform::Vergroesserung,
                    "verkleinerung" => Transform::Verkleinerung,
                    "versetzung" => Transform::Versetzung(steps),
                    "chromatisch" => Transform::Chromatisch(steps),
                    o => return Err(format!("Unbekannte Verwandlung „{o}“")),
                };
                let notes = klang_music::fugue::transform(&v.notes, &v.key.key, t);
                let mut r = json!({ "verwandlung": t.label(), "noten": notes.iter().map(|n| note_name(n.pitch)).collect::<Vec<_>>() });
                if args.get("write").and_then(Value::as_bool).unwrap_or(true) {
                    let name = format!("{} – {}", v.clip_name, t.label());
                    let label = format!("Agent · {name}");
                    let (_, ids) = crate::fugue::add_voices(&st, &[crate::fugue::VoiceIn { name, notes }], v.start, instrument_of(&args), &label)?;
                    self.changed("transform_melody", label);
                    r["trackId"] = json!(ids.first());
                }
                Ok(r)
            }
            "countersubject" => {
                let (tid, cid) = (Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?));
                let chosen = self.key_of(&args).ok().filter(|k| k.source == "gewaehlt").map(|k| k.key);
                let v = crate::fugue::subject_view(&st, tid, cid, chosen)?;
                let cs = klang_music::fugue::countersubjects(&v.notes, &v.key.key, v.beats_per_bar, 4);
                let mut r = json!({
                    "tonart": v.key.name,
                    "vorschlaege": cs.iter().map(|c| json!({ "lage": if c.above { "über dem Thema" } else { "unter dem Thema" },
                        "umkehrbar": c.invertible, "fehler": c.errors, "warnungen": c.warnings,
                        "noten": c.notes.iter().map(|n| note_name(n.pitch)).collect::<Vec<_>>() })).collect::<Vec<_>>(),
                });
                if args.get("write").and_then(Value::as_bool).unwrap_or(false) {
                    let best = cs.first().ok_or("Kein Gegensatz gefunden")?;
                    let label = "Agent · Gegensatz".to_string();
                    let (_, ids) = crate::fugue::add_voices(&st, &[crate::fugue::VoiceIn { name: "Gegensatz".into(), notes: best.notes.clone() }], v.start, instrument_of(&args), &label)?;
                    self.changed("countersubject", label);
                    r["trackId"] = json!(ids.first());
                }
                Ok(r)
            }
            "write_fugue" => {
                let (tid, cid) = (Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?));
                let chosen = self.key_of(&args).ok().filter(|k| k.source == "gewaehlt").map(|k| k.key);
                let v = crate::fugue::subject_view(&st, tid, cid, chosen)?;
                let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(true);
                let opt = klang_music::fugue::FugueOptions {
                    voices: args.get("voices").and_then(Value::as_u64).unwrap_or(3) as usize,
                    beats_per_bar: v.beats_per_bar,
                    countersubject: flag("countersubject"),
                    stretto: flag("stretto"),
                    picardy: flag("picardy"),
                };
                let plan = klang_music::fugue::plan_fugue(&v.notes, &v.key.key, &opt);
                let start = match args.get("startBeats").and_then(Value::as_f64) {
                    Some(s) => s,
                    None => {
                        let s = lock(&st.session)?;
                        let bar = s.project.beats_per_bar.max(1) as f64;
                        let end = s.project.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.start_beats + c.length_beats(s.project.tempo_bpm))).fold(0.0, f64::max);
                        (end / bar).ceil() * bar
                    }
                };
                let voices: Vec<crate::fugue::VoiceIn> = plan.voices.iter().map(|pv| crate::fugue::VoiceIn { name: format!("Fuge – {}", pv.name), notes: pv.notes.clone() }).collect();
                let label = format!("Agent · Fuge über „{}“", v.clip_name);
                let (_, ids) = crate::fugue::add_voices(&st, &voices, start, instrument_of(&args), &label)?;
                self.changed("write_fugue", label);
                let bar = v.beats_per_bar as f64;
                Ok(json!({
                    "ok": true, "tonart": v.key.name, "trackIds": ids, "startBeats": start, "takte": (plan.length / bar).ceil(),
                    "antwort": plan.answer, "engfuehrung": plan.stretto, "gegensatz": plan.countersubject,
                    "abschnitte": plan.sections.iter().map(|s| json!({ "name": s.name, "tonart": s.key,
                        "abTakt": ((start + s.start) / bar).floor() + 1.0, "bisTakt": ((start + s.end) / bar).ceil(), "beschreibung": s.description })).collect::<Vec<_>>(),
                }))
            }
            "check_counterpoint" => {
                let only: Option<Vec<Id>> = args.get("trackIds").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(Id).collect());
                let c = crate::fugue::check_tracks(&st, only.as_deref())?;
                Ok(json!({
                    "stimmen": c.voices.iter().map(|v| json!({ "trackId": v.track_id, "name": v.name })).collect::<Vec<_>>(),
                    "fehler": c.errors, "warnungen": c.warnings, "hinweise": c.hints,
                    "stellen": c.issues.iter().take(120).map(|i| json!({ "art": i.kind, "schwere": i.severity, "takt": i.bar,
                        "startBeats": i.start, "endBeats": i.end, "trackIds": i.track_ids, "text": i.message })).collect::<Vec<_>>(),
                }))
            }
            "analyze_score" => {
                let path = str_arg(&args, "path")?;
                let score = load_score(Path::new(path)).map_err(|e| e.to_string())?;
                let mut v = serde_json::to_value(analyze_score(&score)).map_err(|e| e.to_string())?;
                v["titel"] = json!(score.title);
                v["komponist"] = json!(score.composer);
                Ok(v)
            }
            other => Err(format!("Unbekanntes Werkzeug: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_rounds_float_noise() {
        let mut v = json!({ "a": 0.55f32 as f64, "b": [0.3499999940395355, 2], "c": "x" });
        tidy(&mut v);
        assert_eq!(v, json!({ "a": 0.55, "b": [0.35, 2], "c": "x" }));
    }

    #[test]
    fn tool_schemas_are_objects_and_names_unique() {
        let t = tools(&["pdmx-kostprobe"]);
        let mut names: Vec<_> = t.iter().map(|x| x.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), t.len());
        for x in &t {
            assert_eq!(x.input_schema["type"], "object", "{}", x.name);
        }
    }

    #[test]
    fn schema_examples_parse_as_commands() {
        // Die Beispiele aus der Werkzeugbeschreibung müssen gültige Befehle sein.
        let desc = tools(&[]).into_iter().find(|t| t.name == "execute").unwrap().description;
        let mut found = 0;
        for part in desc.split(", ") {
            let start = part.find('{').unwrap();
            let json_text = part[start..].trim_end_matches('.');
            let cmd: Result<Command, _> = serde_json::from_str(json_text);
            assert!(cmd.is_ok(), "{json_text}: {cmd:?}");
            found += 1;
        }
        assert_eq!(found, 3);
    }
}
