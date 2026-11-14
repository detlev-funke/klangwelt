//! Anwendungszustand: Sitzung (Projekt + Undo), Audio-Assets, Engine,
//! Melodie-Bibliothek und Agent-Schnittstelle.

use crate::analysis::{self, Analysis};
use crate::sound::{self, SoundReport};
use klang_core::{Clip, ClipSource, Command, Id, InstrumentKind, Note, Project, Session, Track, TrackKind, TrackPatch};
use klang_dsp::synth::NoteSpec;
use klang_dsp::{stretch, util};
use klang_engine::{Arrangement, Engine, PlacedClip};
use klang_i18n::tr;
use klang_music::diskindex::AnyIndex;
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub type CmdResult<T> = Result<T, String>;

pub fn lock<T>(m: &Mutex<T>) -> CmdResult<MutexGuard<'_, T>> {
    m.lock().map_err(|_| tr!("Internal error: state is locked"))
}

pub struct Asset {
    #[allow(dead_code)] // für den geplanten Asset-Browser
    pub name: String,
    pub samples: Arc<[f32]>,
    pub sample_rate: u32,
}

/// Audiomaterial liegt hier – das Projekt verweist nur per ID darauf.
#[derive(Default)]
pub struct AssetPool {
    assets: HashMap<String, Asset>,
    renders: HashMap<String, Arc<[f32]>>,
    analyses: HashMap<String, Analysis>,
    counter: u64,
}

impl AssetPool {
    /// Abzweigung für den Export: dieselben Klänge (geteilt), eigener
    /// Zwischenspeicher – was der Export rendert, verdrängt nichts der Wiedergabe.
    fn fork(&self) -> AssetPool {
        AssetPool {
            assets: self.assets.iter().map(|(k, a)| (k.clone(), Asset { name: a.name.clone(), samples: a.samples.clone(), sample_rate: a.sample_rate })).collect(),
            renders: self.renders.clone(),
            analyses: HashMap::new(),
            counter: self.counter,
        }
    }

    pub fn insert(&mut self, name: String, samples: Vec<f32>, sample_rate: u32) -> String {
        self.counter += 1;
        let id = format!("asset-{}", self.counter);
        self.assets.insert(id.clone(), Asset { name, samples: samples.into(), sample_rate });
        id
    }

    pub fn get(&self, id: &str) -> Option<&Asset> {
        self.assets.get(id)
    }

    /// Ersetzt alle Aufnahmen (beim Öffnen eines Projekts); IDs bleiben erhalten.
    pub fn replace_all(&mut self, audio: Vec<crate::project::AudioData>) {
        self.assets.clear();
        self.renders.clear();
        self.analyses.clear();
        for a in audio {
            if let Some(n) = a.id.strip_prefix("asset-").and_then(|n| n.parse::<u64>().ok()) {
                self.counter = self.counter.max(n);
            }
            self.assets.insert(a.id, Asset { name: a.name, samples: a.samples.into(), sample_rate: a.sample_rate });
        }
    }
}

/// Die aktive Bibliothek (siehe `libraries`): genau eine Indexdatei.
pub struct Library {
    /// Kennung in der Bibliotheksliste (`bibliotheken.json`)
    pub id: String,
    pub name: String,
    /// Indexdatei
    pub path: String,
    /// Index auf der Platte (Format 4) oder ältere Datei im Speicher.
    pub index: AnyIndex,
    /// Enthaltene Pakete aus dem Katalog
    pub packs: Vec<String>,
    /// Genre/Komponist liegen für alle enthaltenen PDMX-Pakete vor
    pub has_meta: bool,
}

#[derive(Default)]
pub struct AgentLink {
    /// Adresse des Agent-Servers (MCP), wenn er läuft.
    pub url: Option<String>,
    pub error: Option<String>,
    /// Der laufende MCP-Server des Desktops (hält ihn am Leben; zählt
    /// verbundene Clients). Im Browser gibt es keinen – dort `None`.
    pub server: Option<klang_mcp::ServerHandle>,
}

/// Ein Agent gilt als verbunden, solange er eine Verbindung offen hält
/// (Ereignisstrom, laufende Anfrage wie ein wartender `chat_poll`). Nur wer nie
/// einen Strom öffnet (Skripte), zählt so lange nach seiner letzten Anfrage.
pub const AGENT_GRACE: std::time::Duration = std::time::Duration::from_secs(2 * 60);

/// Schlüssel der gewählten Audiogeräte in den Einstellungen (fehlt/null = Systemstandard).
pub const SETTING_OUTPUT: &str = "ausgabegeraet";
pub const SETTING_INPUT: &str = "eingabegeraet";

/// Why a remembered device wasn't used at start.
#[derive(Clone)]
enum DeviceProblem {
    /// Nicht angeschlossen (Name).
    Missing(String),
    /// Couldn't be opened or chosen (name, error).
    Failed(String, String),
}

/// Hinweis, wenn ein gemerktes Gerät beim Start fehlte (die Wahl bleibt gemerkt).
/// The text is made only when asked for – at start the UI hasn't set its
/// language yet.
#[derive(Default, Clone)]
pub struct AudioNotice {
    output: Option<DeviceProblem>,
    input: Option<DeviceProblem>,
}

impl AudioNotice {
    fn output_text(&self) -> Option<String> {
        self.output.as_ref().map(|p| match p {
            DeviceProblem::Missing(n) => tr!("Output device \"{name}\" is not connected – Klangwelt plays through the system default.", name = n),
            DeviceProblem::Failed(n, e) => {
                tr!("Output device \"{name}\" couldn't be opened ({error}) – Klangwelt plays through the system default.", name = n, error = e)
            }
        })
    }

    fn input_text(&self) -> Option<String> {
        self.input.as_ref().map(|p| match p {
            DeviceProblem::Missing(n) => tr!("Input device \"{name}\" is not connected – Klangwelt records from the system default.", name = n),
            DeviceProblem::Failed(n, e) => {
                tr!("Input device \"{name}\" couldn't be selected ({error}) – Klangwelt records from the system default.", name = n, error = e)
            }
        })
    }
}

/// Audiogeräte für die Auswahl (Oberfläche und Agent).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevicesDto {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
    /// Aktuelle Standardgeräte des Systems
    pub default_output: Option<String>,
    pub default_input: Option<String>,
    /// Gewählte Geräte (null = Systemstandard)
    pub output: Option<String>,
    pub input: Option<String>,
    /// Ausgabegerät, das gerade spielt, und seine Abtastrate
    pub output_device: String,
    pub sample_rate: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfoDto {
    available: bool,
    error: Option<String>,
    output_device: Option<String>,
    /// Gewähltes Ausgabe-/Aufnahmegerät (null = Systemstandard).
    output_choice: Option<String>,
    input_choice: Option<String>,
    /// Gemerktes Gerät fehlte beim Start – Klangwelt nutzt den Systemstandard.
    output_notice: Option<String>,
    input_notice: Option<String>,
    sample_rate: u32,
    metronome: bool,
    /// Wiedergabe wiederholen (am Ende von vorn).
    loop_on: bool,
    /// Markierter Zeitbereich in Schlägen; mit `loop_on` die Schleife, sonst das ganze Stück.
    loop_range: Option<(f64, f64)>,
}

/// Wiederholen und markierter Zeitbereich (nicht Teil des Projekts).
#[derive(Clone, Copy, Default)]
pub struct LoopState {
    pub on: bool,
    pub range: Option<(f64, f64)>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfoDto {
    url: Option<String>,
    error: Option<String>,
    library: Option<String>,
    library_voices: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Projektdatei (None = noch nicht gespeichert).
    file: Option<String>,
    /// Ungespeicherte Änderungen seit dem letzten Speichern/Öffnen.
    dirty: bool,
    project: Project,
    can_undo: bool,
    can_redo: bool,
    undo_label: Option<String>,
    redo_label: Option<String>,
    engine: EngineInfoDto,
    agent: AgentInfoDto,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub playing: bool,
    /// Ein Vorhören klingt gerade (Bibliothek, Akkorde …).
    pub previewing: bool,
    pub recording: bool,
    pub position_secs: f64,
    pub peak_out: f32,
    pub peak_in: f32,
}

pub struct AppState {
    /// Chat zwischen Oberfläche und Agent (Aufträge per `chat_poll`).
    pub chat: crate::chat::Chat,
    pub session: Mutex<Session>,
    pub pool: Mutex<AssetPool>,
    pub library: Mutex<Option<Library>>,
    pub library_status: Mutex<crate::library::LibStatus>,
    /// Laufenden Download/Indexlauf der Bibliothek abbrechen.
    pub library_cancel: AtomicBool,
    /// Laufender Download-Prozess (curl), damit „Abbrechen“ ihn beenden kann.
    pub download_child: Mutex<Option<std::process::Child>>,
    pub agent: Mutex<AgentLink>,
    engine: Option<Engine>,
    engine_error: Option<String>,
    metronome: AtomicBool,
    looping: Mutex<LoopState>,
    pub record_counter: AtomicU32,
    /// Datei des offenen Projekts (None = noch nie gespeichert).
    pub project_file: Mutex<Option<std::path::PathBuf>>,
    /// Prüfsumme des Projekts beim letzten Speichern/Öffnen (für „ungespeichert“).
    saved_hash: Mutex<u64>,
    /// Fenster trotz ungespeicherter Änderungen schließen (nach Rückfrage).
    pub force_close: AtomicBool,
    /// Zuletzt vom Song-Aufbau erzeugte Spuren (neu erzeugen ersetzt sie); gilt nur fürs offene Projekt.
    pub song_tracks: Mutex<Vec<Id>>,
    /// Datei der Einstellungen (beim Start gesetzt; vorher wird nichts gemerkt).
    pub settings_file: Mutex<Option<std::path::PathBuf>>,
    /// Gemerktes Audiogerät fehlte beim Start.
    audio_notice: Mutex<AudioNotice>,
    /// Die Plattform (Ereignisse, Ablageorte); beim Start gesetzt.
    host: std::sync::OnceLock<Box<dyn crate::Host>>,
}

/// Prüfsumme für „ungespeichert“: der Inhalt ohne den internen Zähler für neue
/// IDs – der wächst weiter, wenn ein Schritt zurückgenommen wird (z. B. die Demo
/// nach der Intro-Tour), und das Projekt ist dann trotzdem unverändert.
fn project_hash(p: &Project) -> u64 {
    let mut v = serde_json::to_value(p).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("nextId");
    }
    let mut h = DefaultHasher::new();
    v.to_string().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod hash_tests {
    use super::*;

    #[test]
    fn id_counter_does_not_count_as_a_change() {
        let a = Project::default();
        let mut b = a.clone();
        b.alloc_id();
        b.alloc_id();
        assert_eq!(project_hash(&a), project_hash(&b));
        b.tempo_bpm = 99.0;
        assert_ne!(project_hash(&a), project_hash(&b));
    }
}

impl AppState {
    pub fn new() -> Self {
        let (engine, engine_error) = match Engine::start() {
            Ok(e) => (Some(e), None),
            Err(e) => {
                eprintln!("Audio-Engine nicht verfügbar: {e}");
                (None, Some(e.to_string()))
            }
        };
        Self::with_engine(engine, engine_error)
    }

    /// Ohne Audiogerät (Tests): alles außer Wiedergabe und Aufnahme geht.
    pub fn without_audio() -> Self {
        Self::with_engine(None, Some("Ohne Audio gestartet".into()))
    }

    fn with_engine(engine: Option<Engine>, engine_error: Option<String>) -> Self {
        let state = Self {
            chat: crate::chat::Chat::default(),
            session: Mutex::new(Session::default()),
            pool: Mutex::new(AssetPool::default()),
            library: Mutex::new(None),
            library_status: Mutex::new(Default::default()),
            library_cancel: AtomicBool::new(false),
            download_child: Mutex::new(None),
            agent: Mutex::new(AgentLink::default()),
            engine,
            engine_error,
            metronome: AtomicBool::new(true),
            looping: Mutex::new(LoopState::default()),
            record_counter: AtomicU32::new(0),
            project_file: Mutex::new(None),
            saved_hash: Mutex::new(project_hash(&Project::default())),
            force_close: AtomicBool::new(false),
            song_tracks: Mutex::new(Vec::new()),
            settings_file: Mutex::new(None),
            audio_notice: Mutex::new(AudioNotice::default()),
            host: std::sync::OnceLock::new(),
        };
        if let Some(e) = &state.engine {
            e.set_metronome(true);
            // KLANGWELT_STUMM=1: nichts zu hören (automatische Tests), alles andere läuft wie sonst
            if std::env::var_os("KLANGWELT_STUMM").is_some_and(|v| !v.is_empty() && v != "0") {
                e.set_muted(true);
            }
        }
        state
    }

    /// Plattform setzen (einmal, beim Start).
    pub fn set_host(&self, host: Box<dyn crate::Host>) {
        let _ = self.host.set(host);
    }

    /// Die Plattform – vor dem Start ein Platzhalter, der nichts meldet.
    pub fn host(&self) -> &dyn crate::Host {
        match self.host.get() {
            Some(h) => h.as_ref(),
            None => &crate::host::NoHost,
        }
    }

    pub fn engine(&self) -> CmdResult<&Engine> {
        self.engine.as_ref().ok_or_else(|| self.engine_error_text().unwrap_or_else(|| tr!("Audio engine not available")))
    }

    /// Why the engine is missing. Made at start, before the UI has set its
    /// language – fixed messages of the engine are translated here.
    fn engine_error_text(&self) -> Option<String> {
        self.engine_error.as_deref().map(klang_i18n::translate)
    }

    pub fn snapshot(&self) -> CmdResult<Snapshot> {
        let agent = {
            let a = lock(&self.agent)?;
            let lib = lock(&self.library)?;
            AgentInfoDto {
                url: a.url.clone(),
                error: a.error.clone(),
                library: lib.as_ref().map(|l| l.name.clone()),
                library_voices: lib.as_ref().map(|l| l.index.doc_count()).unwrap_or(0),
            }
        };
        let file = lock(&self.project_file)?.as_ref().map(|p| p.display().to_string());
        let saved = *lock(&self.saved_hash)?;
        let looping = *lock(&self.looping)?;
        let info = self.engine.as_ref().map(|e| e.info());
        let notice = lock(&self.audio_notice)?.clone();
        let s = lock(&self.session)?;
        Ok(Snapshot {
            file,
            dirty: project_hash(&s.project) != saved,
            project: s.project.clone(),
            can_undo: s.undo_label().is_some(),
            can_redo: s.redo_label().is_some(),
            undo_label: s.undo_label().map(str::to_string),
            redo_label: s.redo_label().map(str::to_string),
            engine: EngineInfoDto {
                available: self.engine.is_some(),
                error: self.engine_error_text(),
                output_device: info.as_ref().map(|i| i.output_device.clone()),
                output_choice: info.as_ref().and_then(|i| i.output_choice.clone()),
                input_choice: info.as_ref().and_then(|i| i.input_choice.clone()),
                output_notice: notice.output_text(),
                input_notice: notice.input_text(),
                sample_rate: info.as_ref().map(|i| i.sample_rate).unwrap_or(48_000),
                metronome: self.metronome.load(Ordering::Relaxed),
                loop_on: looping.on,
                loop_range: looping.range,
            },
            agent,
        })
    }

    /// Ungespeicherte Änderungen?
    pub fn is_dirty(&self) -> bool {
        let saved = self.saved_hash.lock().map(|h| *h).unwrap_or(0);
        self.session.lock().map(|s| project_hash(&s.project) != saved).unwrap_or(false)
    }

    /// Speichert das Projekt samt benutzter Aufnahmen nach `path`.
    pub fn save_project(&self, path: &std::path::Path) -> CmdResult<()> {
        let project = {
            let mut s = lock(&self.session)?;
            // Ein noch unbenanntes Projekt heißt danach wie seine Datei (ohne Undo-Schritt).
            if s.project.name == Project::default().name {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    s.project.name = stem.to_string();
                }
            }
            s.project.clone()
        };
        let audio: Vec<crate::project::AudioData> = {
            let pool = lock(&self.pool)?;
            crate::project::used_assets(&project)
                .into_iter()
                .filter_map(|id| {
                    pool.get(&id).map(|a| crate::project::AudioData { name: a.name.clone(), samples: a.samples.to_vec(), sample_rate: a.sample_rate, id })
                })
                .collect()
        };
        crate::project::save(path, &project, &audio).map_err(|e| tr!("Saving to {path} failed: {error}", path = path.display(), error = e))?;
        *lock(&self.saved_hash)? = project_hash(&project);
        *lock(&self.project_file)? = Some(path.to_path_buf());
        Ok(())
    }

    /// Öffnet ein Projekt (ersetzt das offene, Undo-Verlauf beginnt neu).
    pub fn open_project(&self, path: &std::path::Path) -> CmdResult<()> {
        let (project, audio) = crate::project::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
        lock(&self.pool)?.replace_all(audio);
        *lock(&self.saved_hash)? = project_hash(&project);
        *lock(&self.session)? = Session::new(project);
        *lock(&self.project_file)? = Some(path.to_path_buf());
        lock(&self.song_tracks)?.clear();
        self.sync_engine()
    }

    /// Leeres Projekt.
    pub fn new_project(&self) -> CmdResult<()> {
        lock(&self.pool)?.replace_all(Vec::new());
        *lock(&self.session)? = Session::default();
        *lock(&self.saved_hash)? = project_hash(&Project::default());
        *lock(&self.project_file)? = None;
        lock(&self.song_tracks)?.clear();
        self.sync_engine()
    }

    pub fn execute(&self, cmd: Command, merge: bool) -> CmdResult<Snapshot> {
        lock(&self.session)?.execute(cmd, merge).map_err(|e| e.to_string())?;
        self.sync_engine()?;
        self.snapshot()
    }

    pub fn undo(&self) -> CmdResult<Snapshot> {
        lock(&self.session)?.undo().map_err(|e| e.to_string())?;
        self.sync_engine()?;
        self.snapshot()
    }

    pub fn redo(&self) -> CmdResult<Snapshot> {
        lock(&self.session)?.redo().map_err(|e| e.to_string())?;
        self.sync_engine()?;
        self.snapshot()
    }

    /// Lage im Projekt in Worten – geht mit jeder Chat-Nachricht an den Agenten:
    /// hörbare, stumme und Solo-Spuren (mit Kennung) und der markierte Zeitraum.
    pub fn chat_context(&self) -> CmdResult<String> {
        let looping = *lock(&self.looping)?;
        let s = lock(&self.session)?;
        let p = &s.project;
        let list = |ts: Vec<&Track>| ts.iter().map(|t| format!("„{}“ ({})", t.name, t.id)).collect::<Vec<_>>().join(", ");
        let any_solo = p.tracks.iter().any(|t| t.solo);
        let audible: Vec<&Track> = p.tracks.iter().filter(|t| !t.mute && (!any_solo || t.solo)).collect();
        let muted: Vec<&Track> = p.tracks.iter().filter(|t| t.mute).collect();
        let solo: Vec<&Track> = p.tracks.iter().filter(|t| t.solo).collect();
        let mut parts = Vec::new();
        if p.tracks.is_empty() {
            parts.push(tr!("Tracks: none"));
        } else {
            parts.push(if audible.is_empty() { tr!("Audible tracks: none") } else { tr!("Audible tracks: {list}", list = list(audible)) });
            if !solo.is_empty() {
                parts.push(tr!("Solo: {list}", list = list(solo)));
            }
            if !muted.is_empty() {
                parts.push(tr!("Muted: {list}", list = list(muted)));
            }
        }
        let bpb = p.beats_per_bar.max(1) as f64;
        parts.push(match looping.range {
            Some((a, b)) => {
                let from = (a / bpb).floor() as i64 + 1;
                let to = ((b / bpb) - 1e-9).ceil().max(1.0) as i64;
                let (start, end) = ((a * 100.0).round() / 100.0, (b * 100.0).round() / 100.0);
                if looping.on {
                    tr!("Selected range: bars {from}–{to} (beats {start} to {end}), loop on", from = from, to = to, start = start, end = end)
                } else {
                    tr!("Selected range: bars {from}–{to} (beats {start} to {end})", from = from, to = to, start = start, end = end)
                }
            }
            None => tr!("No time range selected"),
        });
        Ok(parts.join(" · "))
    }

    pub fn set_metronome(&self, on: bool) -> CmdResult<Snapshot> {
        self.metronome.store(on, Ordering::Relaxed);
        if let Some(e) = &self.engine {
            e.set_metronome(on);
        }
        // für den nächsten Start merken (auch wenn der Agent umschaltet)
        if let Some(f) = lock(&self.settings_file)?.as_ref() {
            if let Err(e) = crate::settings::set(f, "metronom", serde_json::Value::Bool(on)) {
                eprintln!("Einstellungen: {e}");
            }
        }
        self.snapshot()
    }

    /// Chat für die Oberfläche, samt „Agent verbunden“.
    pub fn chat_view(&self) -> crate::chat::ChatView {
        let mut v = self.chat.view();
        v.agent_connected = v.agent_listening || self.agent_session_active();
        v
    }

    /// Ist ein Agent verbunden: hört er zu, oder hält er eine Verbindung zum
    /// MCP-Server offen (siehe [`AGENT_GRACE`] und `klang_mcp::Presence`)?
    pub fn agent_connected(&self) -> bool {
        self.chat.listening() || self.agent_session_active()
    }

    fn agent_session_active(&self) -> bool {
        lock(&self.agent).ok().and_then(|a| a.server.as_ref().map(|s| s.presence().connected(AGENT_GRACE) > 0)).unwrap_or(false)
    }

    /// Ausgabe- und Aufnahmegeräte samt Wahl.
    pub fn audio_devices(&self) -> CmdResult<AudioDevicesDto> {
        let engine = self.engine()?;
        let d = engine.devices().map_err(|e| e.to_string())?;
        let info = engine.info();
        Ok(AudioDevicesDto {
            outputs: d.outputs,
            inputs: d.inputs,
            default_output: d.default_output,
            default_input: d.default_input,
            output: info.output_choice,
            input: info.input_choice,
            output_device: info.output_device,
            sample_rate: info.sample_rate,
        })
    }

    /// Audiogerät wählen – `kind` „output“/„ausgabe“ oder „input“/„eingabe“,
    /// `name` None (oder leer/„Systemstandard“) = Systemstandard. Wirkt sofort
    /// und gilt auch beim nächsten Start.
    pub fn set_audio_device(&self, kind: &str, name: Option<String>) -> CmdResult<Snapshot> {
        // "system default" as the agent reads it: German, English or in the chosen language
        let default = tr!("System default");
        let name = name.map(|n| n.trim().to_string()).filter(|n| {
            !n.is_empty() && !n.eq_ignore_ascii_case("Systemstandard") && !n.eq_ignore_ascii_case("System default") && !n.eq_ignore_ascii_case(&default)
        });
        let engine = self.engine()?;
        let output = match kind {
            "output" | "ausgabe" => true,
            "input" | "eingabe" => false,
            other => return Err(tr!("Unknown device type \"{kind}\" (allowed: output, input)", kind = other)),
        };
        if output {
            engine.set_output_device(name.clone()).map_err(|e| e.to_string())?;
            // andere Abtastrate: alles für das neue Gerät rendern
            self.sync_engine()?;
            lock(&self.audio_notice)?.output = None;
        } else {
            engine.set_input_device(name.clone()).map_err(|e| e.to_string())?;
            lock(&self.audio_notice)?.input = None;
        }
        if let Some(f) = lock(&self.settings_file)?.as_ref() {
            let key = if output { SETTING_OUTPUT } else { SETTING_INPUT };
            if let Err(e) = crate::settings::set(f, key, name.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)) {
                eprintln!("Einstellungen: {e}");
            }
        }
        self.snapshot()
    }

    /// Beim Start: gemerkte Audiogeräte nehmen. Fehlt eines, bleibt der
    /// Systemstandard und ein Hinweis erscheint (die Wahl bleibt gemerkt –
    /// steckt das Gerät beim nächsten Start wieder, gilt es wieder).
    pub fn apply_audio_settings(&self, settings: &serde_json::Map<String, serde_json::Value>) {
        let Some(engine) = &self.engine else { return };
        let want = |k: &str| settings.get(k).and_then(serde_json::Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let (out, inp) = (want(SETTING_OUTPUT), want(SETTING_INPUT));
        if out.is_none() && inp.is_none() {
            return;
        }
        let devices = engine.devices().unwrap_or_default();
        let mut notice = AudioNotice::default();
        if let Some(n) = out {
            let res = if devices.outputs.contains(&n) { engine.set_output_device(Some(n.clone())).map(|_| ()).map_err(|e| e.to_string()) } else { Err(String::new()) };
            match res {
                Ok(()) => {
                    let _ = self.sync_engine();
                }
                Err(e) if e.is_empty() => notice.output = Some(DeviceProblem::Missing(n)),
                Err(e) => notice.output = Some(DeviceProblem::Failed(n, e)),
            }
        }
        if let Some(n) = inp {
            let res = if devices.inputs.contains(&n) { engine.set_input_device(Some(n.clone())).map(|_| ()).map_err(|e| e.to_string()) } else { Err(String::new()) };
            match res {
                Ok(()) => {}
                Err(e) if e.is_empty() => notice.input = Some(DeviceProblem::Missing(n)),
                Err(e) => notice.input = Some(DeviceProblem::Failed(n, e)),
            }
        }
        for n in [notice.output_text(), notice.input_text()].into_iter().flatten() {
            eprintln!("{n}");
        }
        if let Ok(mut g) = self.audio_notice.lock() {
            *g = notice;
        }
    }

    /// Wiederholen ein/aus und/oder Zeitbereich setzen (`range`: Some(None) hebt ihn auf).
    pub fn set_loop(&self, on: Option<bool>, range: Option<Option<(f64, f64)>>) -> CmdResult<Snapshot> {
        {
            let mut l = lock(&self.looping)?;
            if let Some(on) = on {
                l.on = on;
            }
            if let Some(r) = range {
                l.range = r.map(|(a, b)| (a.min(b).max(0.0), a.max(b))).filter(|(a, b)| b - a > 1e-3);
            }
        }
        self.apply_loop()?;
        self.snapshot()
    }

    /// Ende des Stücks (letzter Clip) und Länge eines Takts, in Schlägen.
    fn song_end(&self) -> CmdResult<(f64, f64)> {
        let s = lock(&self.session)?;
        let p = &s.project;
        let end = p.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.start_beats + c.length_beats(p.tempo_bpm)).fold(0.0, f64::max);
        Ok((end, p.beats_per_bar.max(1) as f64))
    }

    /// Schleife und Haltepunkt an die Engine. Mit Wiederholen: der markierte
    /// Bereich, sonst das ganze Stück (auf ganze Takte aufgerundet). Ohne:
    /// Halt am Ende des markierten Bereichs (zurück an seinen Anfang), sonst
    /// einen Takt nach dem Ende des Stücks.
    fn apply_loop(&self) -> CmdResult<()> {
        let Some(engine) = &self.engine else { return Ok(()) };
        let l = *lock(&self.looping)?;
        let (end, bar) = self.song_end()?;
        let (region, halt) = if l.on {
            (l.range.or_else(|| (end > 0.0).then(|| (0.0, (end / bar - 1e-9).ceil() * bar))), None)
        } else {
            (None, match l.range {
                Some((a, b)) => Some((b, a)),
                None => (end > 0.0).then_some((end + bar, end + bar)),
            })
        };
        engine.set_loop(region);
        engine.set_stop_at(halt);
        Ok(())
    }

    /// Vor dem Start: liegt der Abspielkopf außerhalb des markierten Bereichs,
    /// beginnt es an dessen Anfang; steht er (ohne Bereich) hinter dem
    /// Haltepunkt am Ende, wieder vorn.
    fn place_for_play(&self, engine: &Engine) -> CmdResult<()> {
        let l = *lock(&self.looping)?;
        let spb = engine.info().sample_rate as f64 * 60.0 / self.tempo();
        let pos = engine.status().position_frames as f64 / spb;
        let (end, bar) = self.song_end()?;
        let to = match l.range {
            Some((a, b)) if pos < a - 1e-6 || pos >= b - 1e-6 => Some(a),
            None if !l.on && end > 0.0 && pos >= end + bar - 1e-6 => Some(0.0),
            _ => None,
        };
        if let Some(beat) = to {
            engine.seek_frames((beat * spb).round().max(0.0) as u64);
        }
        Ok(())
    }

    pub fn transport(&self, action: &str) -> CmdResult<()> {
        let engine = self.engine()?;
        match action {
            "play" => {
                if !engine.status().playing {
                    self.place_for_play(engine)?;
                }
                engine.play()
            }
            "stop" => engine.stop(),
            "toggle" => {
                if engine.status().playing {
                    engine.stop()
                } else {
                    self.place_for_play(engine)?;
                    engine.play()
                }
            }
            "rewind" => engine.seek_frames(0),
            other => return Err(tr!("Unknown transport action: {action} (allowed: play, stop, toggle, rewind)", action = other)),
        }
        Ok(())
    }

    pub fn seek_seconds(&self, seconds: f64) -> CmdResult<()> {
        let engine = self.engine()?;
        engine.seek_frames((seconds.max(0.0) * engine.info().sample_rate as f64) as u64);
        Ok(())
    }

    /// Was der Export braucht: Projekt, markierter Bereich, eigener Klangspeicher.
    pub fn export_source(&self) -> CmdResult<ExportSource> {
        let project = lock(&self.session)?.project.clone();
        let range = lock(&self.looping)?.range;
        let pool = lock(&self.pool)?.fork();
        Ok(ExportSource { project, range, pool })
    }

    pub fn tempo(&self) -> f64 {
        lock(&self.session).map(|s| s.project.tempo_bpm).unwrap_or(120.0)
    }

    /// Rendert das Projekt in ein Arrangement und übergibt es der Engine.
    pub fn sync_engine(&self) -> CmdResult<()> {
        let Some(engine) = &self.engine else { return Ok(()) };
        let project = lock(&self.session)?.project.clone();
        let arrangement = {
            let mut pool = lock(&self.pool)?;
            build_arrangement(&project, &mut pool, engine.info().sample_rate)
        };
        // eine Nachricht: Tempo und neu gerendertes Arrangement wechseln im selben Block
        engine.set_arrangement_with_tempo(arrangement, project.tempo_bpm, project.beats_per_bar);
        // Ende des Stücks kann sich geändert haben
        self.apply_loop()
    }

    pub fn status(&self) -> StatusDto {
        match &self.engine {
            Some(e) => {
                let s = e.status();
                StatusDto {
                    playing: s.playing,
                    previewing: s.previewing,
                    recording: s.recording,
                    position_secs: s.position_frames as f64 / s.sample_rate.max(1) as f64,
                    peak_out: s.peak_out,
                    peak_in: s.peak_in,
                }
            }
            None => StatusDto { playing: false, previewing: false, recording: false, position_secs: 0.0, peak_out: 0.0, peak_in: 0.0 },
        }
    }

    /// Analyse eines Assets (zwischengespeichert).
    pub fn analysis_for(&self, asset_id: &str) -> CmdResult<Analysis> {
        let (samples, sr) = {
            let pool = lock(&self.pool)?;
            if let Some(a) = pool.analyses.get(asset_id) {
                return Ok(a.clone());
            }
            let asset = pool.get(asset_id).ok_or_else(|| tr!("Audio file not found"))?;
            (asset.samples.clone(), asset.sample_rate)
        };
        let a = analysis::analyze(asset_id, &samples, sr);
        lock(&self.pool)?.analyses.insert(asset_id.to_string(), a.clone());
        Ok(a)
    }

    pub fn store_analysis(&self, a: Analysis) -> CmdResult<()> {
        lock(&self.pool)?.analyses.insert(a.asset_id.clone(), a);
        Ok(())
    }

    pub fn clip(&self, track_id: Id, clip_id: Id) -> CmdResult<Clip> {
        lock(&self.session)?
            .project
            .clip(track_id, clip_id)
            .cloned()
            .ok_or_else(|| tr!("Clip {clip} not found on track {track}", clip = clip_id, track = track_id))
    }

    /// Analyse eines Audio-Clips.
    pub fn clip_analysis(&self, track_id: Id, clip_id: Id) -> CmdResult<(Clip, Analysis)> {
        let clip = self.clip(track_id, clip_id)?;
        let ClipSource::Audio { asset_id, .. } = &clip.source else {
            return Err(tr!("Only audio clips can be analyzed."));
        };
        let a = self.analysis_for(asset_id)?;
        Ok((clip, a))
    }

    /// Erkannte Melodie eines Audio-Clips als Instrumentspur anlegen (Sechzehntel-Raster,
    /// direkt unter der Audiospur) – siehe `convert::audio_to_notes`.
    pub fn melody_to_track(&self, track_id: Id, clip_id: Id, label: &str) -> CmdResult<(Snapshot, Id)> {
        let r = self.audio_to_notes(track_id, Some(clip_id), crate::convert::Target::Melodie, label)?;
        Ok((r.snapshot, r.track_id))
    }

    pub fn instrument_track(&self, track_id: Id) -> CmdResult<Track> {
        let s = lock(&self.session)?;
        let t = s.project.track(track_id).ok_or_else(|| tr!("Track {id} not found", id = track_id))?;
        if t.kind != TrackKind::Instrument {
            return Err(tr!("\"{name}\" is an audio track – sound design is only available for instrument tracks.", name = t.name));
        }
        Ok(t.clone())
    }

    /// Spielt ein Vorhör-Muster mit dem Klang der Spur (Transport läuft unabhängig weiter).
    pub fn preview_track(&self, track_id: Id, pattern: &str) -> CmdResult<f32> {
        let track = self.instrument_track(track_id)?;
        let engine = self.engine()?;
        let sr = engine.info().sample_rate as f32;
        let inst = track.instrument.as_ref();
        let shift = if sound::transposes(inst) { track.pitch_semitones } else { 0.0 };
        let (notes, secs) = sound::preview_notes(inst, pattern, self.tempo(), shift);
        let r = sound::render_track(inst, &notes, secs, sr, (60.0 / self.tempo()) as f32);
        let secs = r.mid.len() as f32 / sr;
        engine.preview_stereo(r.mid.into(), r.side.map(Into::into), util::db_to_gain(track.gain_db), sound::reverb_send(inst));
        Ok(secs)
    }

    /// Einzelnen Ton anspielen (beim Setzen/Verschieben von Noten im Editor).
    pub fn preview_note(&self, track_id: Id, midi: u8) -> CmdResult<()> {
        let track = self.instrument_track(track_id)?;
        let engine = self.engine()?;
        let sr = engine.info().sample_rate as f32;
        let inst = track.instrument.as_ref();
        let shift = if sound::transposes(inst) { track.pitch_semitones } else { 0.0 };
        let note = NoteSpec::new(0.0, 0.45, midi as f32 + shift, 0.85);
        let r = sound::render_track(inst, &[note], 0.6, sr, (60.0 / self.tempo()) as f32);
        engine.preview_stereo(r.mid.into(), r.side.map(Into::into), util::db_to_gain(track.gain_db), sound::reverb_send(inst));
        Ok(())
    }

    /// Wellenform des Testklangs der Spur (für das Klang-Fenster).
    pub fn sound_wave(&self, track_id: Id) -> CmdResult<sound::WaveView> {
        let track = self.instrument_track(track_id)?;
        let inst = track.instrument.unwrap_or_else(|| sound::default_instrument(InstrumentKind::Pluck));
        Ok(sound::wave_view(&inst, (60.0 / self.tempo()) as f32, 480))
    }

    /// Setzt einen Punkt in einem Feld (Charakter, Klangfarbe, Raum) – oder,
    /// mit `preset`, dessen Startpunkt. Nur die Regler dieses Felds ändern sich.
    /// `merge`: Teil einer Ziehbewegung (ein Rückgängig-Schritt).
    pub fn sound_field(&self, track_id: Id, field: &str, xy: Option<(f32, f32)>, preset: Option<&str>, merge: bool) -> CmdResult<Snapshot> {
        let track = self.instrument_track(track_id)?;
        let mut inst = track.instrument.clone().unwrap_or_else(|| sound::default_instrument(InstrumentKind::Pluck));
        let (x, y) = match (preset, xy) {
            (Some(name), _) => sound::field_preset(inst.kind, field, name)?,
            (None, Some(xy)) => xy,
            (None, None) => return Err(tr!("Point (x, y) or starting point missing")),
        };
        sound::apply_field(&mut inst, field, x, y)?;
        inst.preset = match (field, preset) {
            ("charakter", Some(name)) => name.to_string(),
            ("charakter", None) => "eigen".into(),
            _ => inst.preset,
        };
        let label = match field {
            "charakter" => tr!("Sound \"{name}\": Character", name = track.name),
            "klangfarbe" => tr!("Sound \"{name}\": Timbre", name = track.name),
            _ => tr!("Sound \"{name}\": Space", name = track.name),
        };
        self.execute(
            Command::Batch { label, commands: vec![Command::SetTrack { track_id, patch: TrackPatch { instrument: Some(inst), ..Default::default() } }] },
            merge,
        )
    }

    pub fn describe_track(&self, track_id: Id) -> CmdResult<SoundReport> {
        let track = self.instrument_track(track_id)?;
        let inst = track.instrument.unwrap_or_else(|| sound::default_instrument(InstrumentKind::Pluck));
        Ok(sound::report(&inst))
    }

    /// Neue Instrumentspur mit Standardklang und einem 4-taktigen Muster, damit sofort etwas klingt.
    pub fn create_instrument_track(&self, kind: InstrumentKind, label: &str) -> CmdResult<(Snapshot, Id)> {
        let (cmd, id) = {
            let mut s = lock(&self.session)?;
            let p = &mut s.project;
            let bpb = p.beats_per_bar as f64;
            let count = p.tracks.iter().filter(|t| t.instrument.as_ref().map(|i| i.kind) == Some(kind)).count();
            let name = match count {
                0 => sound::kind_name(kind).to_string(),
                n => format!("{} {}", sound::kind_name(kind), n + 1),
            };
            let id = p.alloc_id();
            let mut track = Track::new(id, name, TrackKind::Instrument);
            track.instrument = Some(sound::default_instrument(kind));
            track.clips.push(Clip {
                id: p.alloc_id(),
                name: tr!("Pattern"),
                start_beats: 0.0,
                source: ClipSource::Notes { length_beats: 4.0 * bpb, notes: sound::pattern_bars(kind, 4, bpb) },
            });
            (Command::Batch { label: label.into(), commands: vec![Command::AddTrack { track, index: None }] }, id)
        };
        Ok((self.execute(cmd, false)?, id))
    }

    /// Fügt einen Takt des Beispielmusters hinter dem letzten Clip der Spur ein.
    pub fn insert_pattern(&self, track_id: Id) -> CmdResult<Snapshot> {
        let track = self.instrument_track(track_id)?;
        let kind = sound::kind_of(track.instrument.as_ref());
        let (bpb, bpm) = {
            let s = lock(&self.session)?;
            (s.project.beats_per_bar as f64, s.project.tempo_bpm)
        };
        let end = track.clips.iter().map(|c| c.start_beats + c.length_beats(bpm)).fold(0.0, f64::max);
        let start = (end / bpb).ceil() * bpb;
        let clip = Clip {
            id: Id::NEW,
            name: tr!("Pattern"),
            start_beats: start,
            source: ClipSource::Notes { length_beats: 4.0, notes: sound::pattern(kind) },
        };
        self.execute(Command::AddClip { track_id, clip }, false)
    }

    /// Aufnahme starten (die Wiedergabe läuft mit).
    pub fn record_start(&self) -> CmdResult<()> {
        if crate::library::LISTENING.load(Ordering::SeqCst) {
            return Err(tr!("Sing-to-search for the library is currently running – stop it there first."));
        }
        let engine = self.engine()?;
        engine.start_recording().map_err(|e| e.to_string())?;
        engine.play();
        Ok(())
    }

    /// Aufnahme beenden: als Audiospur ins Projekt (ein Rückgängig-Schritt), mit Analyse.
    pub fn record_stop(&self) -> CmdResult<RecordResult> {
        if crate::library::LISTENING.load(Ordering::SeqCst) {
            return Err(tr!("This is the library's sing-to-search recording – stop it there with \"Stop\"."));
        }
        let engine = self.engine()?;
        let rec = engine.stop_recording().map_err(|e| e.to_string())?;
        engine.stop();
        let out_sr = engine.info().sample_rate as f64;
        if rec.samples.len() < (rec.sample_rate as usize) / 4 {
            return Err(tr!("The recording was too short."));
        }

        let n = self.record_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let name = tr!("Recording {n}", n = n);
        let duration_secs = rec.samples.len() as f64 / rec.sample_rate as f64;
        let mut samples = rec.samples;
        // Leise Aufnahmen auf brauchbaren Pegel bringen (nur anheben, nie absenken).
        let peak = util::peak(&samples);
        if peak > 1e-4 && peak < 0.5 {
            util::normalize(&mut samples, 0.5);
        }

        let asset_id = lock(&self.pool)?.insert(name.clone(), samples.clone(), rec.sample_rate);
        let analysis = analysis::analyze(&asset_id, &samples, rec.sample_rate);
        self.store_analysis(analysis.clone())?;

        let (cmd, track_id, clip_id) = {
            let mut session = lock(&self.session)?;
            let p = &mut session.project;
            let bpm = p.tempo_bpm;
            let track_id = p.alloc_id();
            let clip_id = p.alloc_id();
            let mut track = Track::new(track_id, name.clone(), TrackKind::Audio);
            track.clips.push(Clip {
                id: clip_id,
                name: name.clone(),
                start_beats: rec.start_frame as f64 / out_sr * bpm / 60.0,
                source: ClipSource::Audio { asset_id, source_bpm: bpm, warp: true, duration_secs },
            });
            (Command::Batch { label: tr!("Recording"), commands: vec![Command::AddTrack { track, index: None }] }, track_id, clip_id)
        };
        let snapshot = self.execute(cmd, false)?;
        Ok(RecordResult { snapshot, analysis, track_id, clip_id })
    }

    /// Übersicht einer Aufnahme für die Wellenform (`buckets` Werte).
    pub fn waveform(&self, asset_id: &str, buckets: usize) -> CmdResult<Vec<f32>> {
        let pool = lock(&self.pool)?;
        let asset = pool.get(asset_id).ok_or_else(|| tr!("Audio file not found"))?;
        Ok(util::overview(&asset.samples, buckets.clamp(16, 20_000)))
    }

    /// Demo-Projekt einfügen (ein Rückgängig-Schritt).
    pub fn create_demo(&self) -> CmdResult<Snapshot> {
        let cmd = {
            let mut s = lock(&self.session)?;
            let mut pool = lock(&self.pool)?;
            crate::demo::demo_command(&mut s.project, &mut pool)
        };
        self.execute(cmd, false)
    }

    /// Laufendes Vorhören (Vorschau, Bibliotheks-Ausschnitt …) beenden.
    pub fn preview_stop(&self) -> CmdResult<()> {
        self.engine()?.preview(Vec::<f32>::new().into(), 0.0);
        Ok(())
    }
}

/// Ergebnis einer Aufnahme.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordResult {
    pub snapshot: Snapshot,
    pub analysis: Analysis,
    pub track_id: Id,
    pub clip_id: Id,
}

/// Schlüssel eines gerenderten Noten-Clips (Noten, Klang, Tempo, Tonhöhe, Länge).
fn notes_key(track: &Track, notes: &[Note], length_beats: f64, bpm: f64, out_sr: u32) -> String {
    let mut h = DefaultHasher::new();
    serde_json::to_string(notes).unwrap_or_default().hash(&mut h);
    serde_json::to_string(&track.instrument.as_ref()).unwrap_or_default().hash(&mut h);
    format!("notes|{:x}|{bpm:.4}|{:.3}|{out_sr}|{length_beats}", h.finish(), track.pitch_semitones)
}

fn notes_specs(track: &Track, notes: &[Note], length_beats: f64, spb: f64) -> Vec<NoteSpec> {
    let shift = if sound::transposes(track.instrument.as_ref()) { track.pitch_semitones } else { 0.0 };
    notes
        .iter()
        .filter(|n| n.start_beats < length_beats)
        .map(|n| NoteSpec { dur_secs: (n.length_beats.min(length_beats - n.start_beats) * spb) as f32, ..sound::note_spec(n, spb as f32, shift) })
        .collect()
}

/// Schlüssel eines Audio-Clips auf Projekttempo, Tonhöhe und Ausgaberate.
fn audio_key(asset_id: &str, time_ratio: f64, pitch_semitones: f64, out_sr: u32) -> String {
    format!("audio|{asset_id}|{time_ratio:.6}|{pitch_semitones:.3}|{out_sr}")
}

/// Audio-Clip dehnen (Tempo), Tonhöhe verschieben und auf die Ausgaberate bringen.
fn render_audio(samples: &[f32], sample_rate: u32, time_ratio: f64, pitch_semitones: f64, out_sr: u32) -> Arc<[f32]> {
    let rate = out_sr as f64 / sample_rate as f64;
    stretch::render_warp(samples, sample_rate as f32, time_ratio, pitch_semitones, rate).into()
}

/// Im Browser gibt es keine Threads (wasm32-unknown-unknown): die Schleife in
/// `arrange` rendert die Clips der Reihe nach.
#[cfg(target_arch = "wasm32")]
fn prerender(_project: &Project, _pool: &mut AssetPool, _out_sr: u32) {}

/// Was noch nicht gerendert ist (Noten-Clips, gedehnte Audio-Clips), auf
/// mehreren Kernen rendern und in den Zwischenspeicher legen – z. B. nach
/// einer Tempoänderung (alles neu) oder für einen ganzen erzeugten Song.
#[cfg(not(target_arch = "wasm32"))]
fn prerender(project: &Project, pool: &mut AssetPool, out_sr: u32) {
    enum Job<'a> {
        Notes { track: &'a Track, specs: Vec<NoteSpec>, secs: f32 },
        Audio { samples: Arc<[f32]>, sample_rate: u32, time_ratio: f64, pitch: f64 },
    }
    impl Job<'_> {
        /// Grob geschätzter Aufwand, um lange Arbeiten zuerst zu verteilen.
        fn cost(&self) -> f64 {
            match self {
                Job::Notes { specs, secs, .. } => *secs as f64 * specs.len() as f64,
                // Dehnen und Umrechnen kosten je Sekunde etwa so viel wie zwei Dutzend Noten
                Job::Audio { samples, sample_rate, .. } => samples.len() as f64 / *sample_rate as f64 * 24.0,
            }
        }
    }
    let bpm = project.tempo_bpm;
    let spb = 60.0 / bpm;
    let any_solo = project.tracks.iter().any(|t| t.solo);
    let mut jobs: Vec<(String, Job)> = Vec::new();
    let mut seen = HashSet::new();
    for track in project.tracks.iter().filter(|t| !t.mute && (!any_solo || t.solo)) {
        for clip in &track.clips {
            match &clip.source {
                ClipSource::Notes { length_beats, notes } => {
                    let key = notes_key(track, notes, *length_beats, bpm, out_sr);
                    if !pool.renders.contains_key(&key) && seen.insert(key.clone()) {
                        jobs.push((key, Job::Notes { track, specs: notes_specs(track, notes, *length_beats, spb), secs: (length_beats * spb) as f32 }));
                    }
                }
                ClipSource::Audio { asset_id, source_bpm, warp, .. } => {
                    let Some(asset) = pool.assets.get(asset_id) else { continue };
                    let time_ratio = if *warp { source_bpm / bpm } else { 1.0 };
                    let pitch = track.pitch_semitones as f64;
                    let key = audio_key(asset_id, time_ratio, pitch, out_sr);
                    if !pool.renders.contains_key(&key) && seen.insert(key.clone()) {
                        jobs.push((key, Job::Audio { samples: asset.samples.clone(), sample_rate: asset.sample_rate, time_ratio, pitch }));
                    }
                }
            }
        }
    }
    if jobs.len() < 2 {
        return; // einer allein: rendert die Schleife in `arrange`
    }
    // lange Arbeiten zuerst, damit die Kerne gleichmäßig zu tun haben
    jobs.sort_by(|a, b| b.1.cost().total_cmp(&a.1.cost()));
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8).min(jobs.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done: Mutex<Vec<(String, Arc<[f32]>, Option<Arc<[f32]>>)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((key, job)) = jobs.get(i) else { break };
                let (mid, side) = match job {
                    Job::Notes { track, specs, secs } => {
                        let r = sound::render_track(track.instrument.as_ref(), specs, *secs, out_sr as f32, spb as f32);
                        (r.mid.into(), r.side.map(Into::into))
                    }
                    Job::Audio { samples, sample_rate, time_ratio, pitch } => (render_audio(samples, *sample_rate, *time_ratio, *pitch, out_sr), None),
                };
                if let Ok(mut d) = done.lock() {
                    d.push((key.clone(), mid, side));
                }
            });
        }
    });
    for (key, mid, side) in done.into_inner().unwrap_or_default() {
        if let Some(sd) = side {
            pool.renders.insert(format!("{key}|seite"), sd);
        }
        pool.renders.insert(key, mid);
    }
}

/// Für den Export (siehe `AppState::export_source`).
pub struct ExportSource {
    pub project: Project,
    pub range: Option<(f64, f64)>,
    pool: AssetPool,
}

impl ExportSource {
    /// Arrangement eines (Teil-)Projekts; Gerendertes bleibt für weitere Aufrufe liegen.
    pub fn arrangement(&mut self, project: &Project, out_sr: u32) -> Arrangement {
        arrange(project, &mut self.pool, out_sr).0
    }
}

fn build_arrangement(project: &Project, pool: &mut AssetPool, out_sr: u32) -> Arrangement {
    let (arrangement, used) = arrange(project, pool, out_sr);
    pool.renders.retain(|k, _| used.contains(k));
    arrangement
}

/// Arrangement plus die Schlüssel des benutzten Zwischenspeichers.
fn arrange(project: &Project, pool: &mut AssetPool, out_sr: u32) -> (Arrangement, HashSet<String>) {
    prerender(project, pool, out_sr);
    let bpm = project.tempo_bpm;
    let spb = 60.0 / bpm;
    let any_solo = project.tracks.iter().any(|t| t.solo);
    let mut used = HashSet::new();
    let mut clips = Vec::new();

    for track in &project.tracks {
        if track.mute || (any_solo && !track.solo) {
            continue;
        }
        let gain = util::db_to_gain(track.gain_db);
        let angle = (track.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gain_l, gain_r) = (
            gain * angle.cos() * std::f32::consts::SQRT_2,
            gain * angle.sin() * std::f32::consts::SQRT_2,
        );

        let send = if track.instrument.is_some() { sound::reverb_send(track.instrument.as_ref()) } else { 0.0 };
        for clip in &track.clips {
            let send = if matches!(clip.source, ClipSource::Audio { .. }) { 0.0 } else { send };
            let (data, side): (Arc<[f32]>, Option<Arc<[f32]>>) = match &clip.source {
                ClipSource::Audio { asset_id, source_bpm, warp, .. } => {
                    let Some(asset) = pool.assets.get(asset_id) else { continue };
                    let time_ratio = if *warp { source_bpm / bpm } else { 1.0 };
                    let pitch = track.pitch_semitones as f64;
                    let key = audio_key(asset_id, time_ratio, pitch, out_sr);
                    used.insert(key.clone());
                    let rendered = match pool.renders.get(&key) {
                        Some(d) => d.clone(),
                        None => {
                            let rendered = render_audio(&asset.samples, asset.sample_rate, time_ratio, pitch, out_sr);
                            pool.renders.insert(key, rendered.clone());
                            rendered
                        }
                    };
                    (rendered, None)
                }
                ClipSource::Notes { length_beats, notes } => {
                    let inst = track.instrument.as_ref();
                    let key = notes_key(track, notes, *length_beats, bpm, out_sr);
                    let side_key = format!("{key}|seite");
                    used.insert(key.clone());
                    used.insert(side_key.clone());
                    match pool.renders.get(&key) {
                        Some(d) => (d.clone(), pool.renders.get(&side_key).cloned()),
                        None => {
                            let specs = notes_specs(track, notes, *length_beats, spb);
                            let r = sound::render_track(inst, &specs, (length_beats * spb) as f32, out_sr as f32, spb as f32);
                            let rendered: Arc<[f32]> = r.mid.into();
                            let side: Option<Arc<[f32]>> = r.side.map(Into::into);
                            pool.renders.insert(key, rendered.clone());
                            if let Some(sd) = &side {
                                pool.renders.insert(side_key, sd.clone());
                            }
                            (rendered, side)
                        }
                    }
                }
            };
            clips.push(PlacedClip {
                start_frame: (clip.start_beats * spb * out_sr as f64).round().max(0.0) as u64,
                data,
                side,
                gain_l,
                gain_r,
                send,
            });
        }
    }
    (Arrangement { clips }, used)
}


#[cfg(test)]
mod tempo_tests {
    use super::*;
    use klang_engine::{Mixer, RtMsg, FADE_SECS};

    const SR: u32 = 48_000;
    const BLOCK: usize = 128;

    fn demo_project() -> (Project, AssetPool) {
        let mut project = Project::default();
        let mut pool = AssetPool::default();
        let cmd = crate::demo::demo_command(&mut project, &mut pool);
        let mut s = Session::new(project);
        s.execute(cmd, false).expect("Demo");
        (s.project, pool)
    }

    /// Wie der Mischer auf das neue Arrangement umschaltet.
    #[derive(Clone, Copy, PartialEq)]
    enum Switch {
        /// wie vor der Überblendung: sofort (nachgestellt mit Stopp, Wechsel
        /// und Start im selben Block – ohne Wiedergabe gibt es keinen Übergang)
        Hard,
        /// wie `sync_engine`: eine Nachricht während der Wiedergabe
        Faded,
    }

    fn mixer(arr: &Arc<Arrangement>, bpm: f64, seek: u64) -> Mixer {
        let mut m = Mixer::new(SR as f32, 2);
        m.send(RtMsg::Tempo { bpm, beats_per_bar: 4 });
        m.send(RtMsg::Arrangement { arrangement: arr.clone(), tempo: None });
        m.send(RtMsg::Seek(seek));
        m.send(RtMsg::Play);
        m
    }

    /// Linker Kanal, bis mindestens `len` Frames; vor jedem Block darf `before` Nachrichten schicken.
    fn run(m: &mut Mixer, len: usize, mut before: impl FnMut(&mut Mixer, usize)) -> Vec<f32> {
        let mut out = vec![0f32; BLOCK * 2];
        let mut left = Vec::with_capacity(len + BLOCK);
        while left.len() < len {
            before(m, left.len());
            m.process(&mut out);
            left.extend(out.chunks(2).map(|f| f[0]));
        }
        left
    }

    /// Spielt `a` bei `from` BPM, wechselt am ersten Blockanfang ab `at` auf
    /// `b` bei `to` BPM und spielt weiter. Liefert das Signal ab Frame 0, die
    /// Umschaltstelle und die neue Position (in Schlägen gleich).
    fn play_with_switch(a: &Arc<Arrangement>, b: &Arc<Arrangement>, from: f64, to: f64, at: usize, len: usize, how: Switch) -> (Vec<f32>, usize, u64) {
        let mut switched = None;
        let sig = run(&mut mixer(a, from, 0), len, |m, done| {
            if switched.is_none() && done >= at {
                switched = Some(done);
                let msg = RtMsg::Arrangement { arrangement: b.clone(), tempo: Some((to, 4)) };
                if how == Switch::Hard {
                    m.send(RtMsg::Stop);
                    m.send(msg);
                    m.send(RtMsg::Play);
                } else {
                    m.send(msg);
                }
            }
        });
        let s = switched.unwrap_or(len);
        (sig, s, (s as f64 * from / to).round() as u64)
    }

    /// Größter Überschuss eines Sample-Schritts im Ausgang (ab der
    /// Umschaltstelle `s`, `span` Samples) über das, was altes (`old`, ab
    /// Frame 0) und neues Signal (`new`, ab `s`) dort ohnehin tun. Ein Knack
    /// zeigt sich als großer Überschuss.
    fn excess(sig: &[f32], s: usize, old: &[f32], new: &[f32], span: usize) -> f32 {
        (0..span)
            .map(|j| {
                let i = s + j;
                let dx = (sig[i] - sig[i - 1]).abs();
                let da = (old[i] - old[i - 1]).abs();
                let db = if j > 0 { (new[j] - new[j - 1]).abs() } else { 0.0 };
                dx - da.max(db)
            })
            .fold(0.0, f32::max)
    }

    /// Größter Überschuss über `points` Tempowechsel (hart, übergeblendet).
    fn worst_excess(p: &mut Project, pool: &mut AssetPool, from: f64, to: f64, points: usize) -> (f32, f32) {
        p.tempo_bpm = from;
        let a = Arc::new(arrange(p, pool, SR).0);
        p.tempo_bpm = to;
        let b = Arc::new(arrange(p, pool, SR).0);
        let span = (FADE_SECS * SR as f32) as usize + 2;
        let mut worst = (0f32, 0f32);
        let last = SR as usize / 2 + points * SR as usize / 4 + 2 * span;
        let old = run(&mut mixer(&a, from, 0), last, |_, _| {});
        for k in 0..points {
            let at = SR as usize / 2 + k * SR as usize / 4;
            for how in [Switch::Hard, Switch::Faded] {
                let (sig, s, pos) = play_with_switch(&a, &b, from, to, at, at + 2 * span, how);
                let new = run(&mut mixer(&b, to, pos), span, |_, _| {});
                let e = excess(&sig, s, &old, &new, span);
                if how == Switch::Hard {
                    worst.0 = worst.0.max(e);
                } else {
                    worst.1 = worst.1.max(e);
                }
            }
        }
        worst
    }

    /// Tempowechsel während der Wiedergabe mit dem Demo-Projekt (Audio-Clips
    /// werden neu gestreckt, Noten neu gerendert): übergeblendet bleibt jeder
    /// Sample-Schritt so groß, wie ihn altes oder neues Signal ohnehin hätten.
    #[test]
    fn tempo_change_in_the_demo_does_not_click() {
        let (mut p, mut pool) = demo_project();
        let (hard, faded) = worst_excess(&mut p, &mut pool, 110.0, 111.0, 8);
        assert!(hard > 0.05, "Testaufbau: hart umgeschaltet knackt es ({hard})");
        assert!(faded < 0.01, "übergeblendet: Überschuss {faded} (hart {hard})");
    }

    /// Messung (nur Ausgabe): Sprung im Signal beim Tempowechsel mit dem
    /// Demo-Projekt, hart gegen übergeblendet, je 32 Umschaltstellen.
    /// `cargo test --release -p klang-app -- --ignored --nocapture tempo_sprung`
    #[test]
    #[ignore = "Messung"]
    fn tempo_sprung_messen() {
        let (mut p, mut pool) = demo_project();
        for (from, to) in [(110.0, 111.0), (110.0, 120.0), (120.0, 90.0)] {
            let t = std::time::Instant::now();
            p.tempo_bpm = to;
            arrange(&p, &mut pool, SR);
            let render_ms = t.elapsed().as_secs_f64() * 1e3;
            let (hard, faded) = worst_excess(&mut p, &mut pool, from, to, 32);
            println!("{from} → {to} BPM: größter Überschuss hart {hard:.4}, übergeblendet {faded:.4} (Neu-Rendern {render_ms:.0} ms)");
        }
    }

    /// Messung (nur Ausgabe): Wie lange dauert ein Tempo-Schritt (wie in der
    /// App: neu rendern, Zwischenspeicher auf das Benutzte kürzen), und wohin
    /// geht die Zeit? Bei 44,1 kHz müssen die 48-kHz-Clips des Demos auch
    /// umgerechnet werden.
    /// `cargo test --release -p klang-app -- --ignored --nocapture tempo_schritt`
    #[test]
    #[ignore = "Messung"]
    fn tempo_schritt_zeit_messen() {
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        for out_sr in [48_000u32, 44_100] {
            let (mut p, mut pool) = demo_project();
            build_arrangement(&p, &mut pool, out_sr);
            let mut steps = Vec::new();
            for bpm in [111.0, 112.0, 113.0, 114.0] {
                p.tempo_bpm = bpm;
                let t = std::time::Instant::now();
                build_arrangement(&p, &mut pool, out_sr);
                steps.push(format!("{:.0}", ms(t)));
            }
            println!("{out_sr} Hz: Tempo-Schritt {} ms", steps.join(", "));
            // Aufteilung je Audio-Clip: Dehnen und Umrechnen der Abtastrate
            for track in &p.tracks {
                for clip in &track.clips {
                    let ClipSource::Audio { asset_id, source_bpm, .. } = &clip.source else { continue };
                    let a = &pool.assets[asset_id];
                    let t = std::time::Instant::now();
                    let st = stretch::time_stretch(&a.samples, source_bpm / 115.0, a.sample_rate as f32);
                    let t_stretch = ms(t);
                    let t = std::time::Instant::now();
                    let rs = stretch::resample(&st, out_sr as f64 / a.sample_rate as f64);
                    println!(
                        "  {}: {:.1} s, Dehnen {:.0} ms, Umrechnen {:.0} ms ({} Samples)",
                        track.name,
                        a.samples.len() as f64 / a.sample_rate as f64,
                        t_stretch,
                        ms(t),
                        rs.len()
                    );
                }
            }
        }
    }
}
