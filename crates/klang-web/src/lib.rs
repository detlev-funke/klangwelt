//! Web-Kern: Klangwelt im Browser – die Hülle um den gemeinsamen Kern
//! (`klang-app`), Gegenstück zur Tauri-Hülle (`app/src/lib.rs`).
//!
//! | Desktop (Tauri)                    | Browser (dieser Kern)                         |
//! |------------------------------------|-----------------------------------------------|
//! | Tauri-IPC → `commands::call`       | `kw_invoke` → `commands::call`                |
//! | `DesktopHost`: Tauri-Events, Ordner | [`WebHost`]: Ereignisse an das Skript, `/Klangwelt/…` |
//! | Engine am Gerät (cpal-Thread)      | Engine → AudioWorklet (`klang_engine::web`)   |
//! | Status-Thread alle 33 ms           | Worker-Skript ruft `kw_status` alle 33 ms     |
//! | Dateisystem                        | Dateibaum im Speicher, gesichert in OPFS      |
//!
//! Das Worker-Skript (`ui/src/web/worker.ts`) lädt dieses Modul. Die
//! Schnittstelle ist schlank und braucht kein Zusatzwerkzeug: Zahlen und
//! Zeiger in den linearen Speicher; Texte als UTF-8 (JSON wie beim Tauri-IPC).
//! Antworten liegen danach in einem Puffer (`kw_out_ptr`, `kw_out_len`).
//!
//! Auf anderen Zielen als wasm32 ist die Crate leer.

#![cfg(target_arch = "wasm32")]

use klang_app::{commands, settings, vfs, AppState, Host};
use klang_engine::wire::Kind;
use klang_music::source::remote;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Was das Worker-Skript bereitstellt (Importmodul „klangwelt“).
mod js {
    #[link(wasm_import_module = "klangwelt")]
    extern "C" {
        /// Ereignis an die Oberfläche (Name, JSON).
        pub fn emit(event: *const u8, event_len: usize, payload: *const u8, payload_len: usize);
        /// Nachricht an das Worklet (Art, Zahlen).
        pub fn engine_post(kind: u32, data: *const f64, len: usize);
        /// Klangpuffer an das Worklet.
        pub fn engine_buffer(id: u32, data: *const f32, len: usize);
        /// Date.now()
        pub fn now_ms() -> f64;
        /// performance.now()
        pub fn mono_ms() -> f64;
        /// Meldung auf der Konsole (z. B. bei einem Absturz).
        pub fn log(msg: *const u8, len: usize);
    }
}

/// Ablage im Dateibaum (gesichert im Speicher des Browsers).
const PROJECTS: &str = "/Klangwelt/Projekte";
const LIBRARY: &str = "/Klangwelt/Bibliothek";
const SETTINGS: &str = "/Klangwelt/einstellungen.json";

/// Die Plattform „Browser“ für den Kern.
struct WebHost;

impl Host for WebHost {
    fn emit_json(&self, event: &str, p: String) {
        // SAFETY: Zeiger und Längen gültiger Texte, das Skript liest sie sofort.
        unsafe { js::emit(event.as_ptr(), event.len(), p.as_ptr(), p.len()) }
    }
    fn projects_dir(&self) -> PathBuf {
        PathBuf::from(PROJECTS)
    }
    fn library_root(&self) -> PathBuf {
        PathBuf::from(LIBRARY)
    }
}

fn log(msg: &str) {
    // SAFETY: gültiger Text.
    unsafe { js::log(msg.as_ptr(), msg.len()) }
}

static STATE: OnceLock<AppState> = OnceLock::new();
/// Antwort des letzten Aufrufs.
static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Geänderte Dateien, bis das Skript sie gesichert hat.
type Change = (PathBuf, Option<Arc<Vec<u8>>>);
static CHANGES: Mutex<Vec<Change>> = Mutex::new(Vec::new());

fn out() -> MutexGuard<'static, Vec<u8>> {
    OUT.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_out(bytes: Vec<u8>) {
    *out() = bytes;
}

/// # Safety
/// `ptr` zeigt auf `len` Bytes im linearen Speicher.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

/// # Safety
/// wie [`bytes`]
unsafe fn text<'a>(ptr: *const u8, len: usize) -> &'a str {
    std::str::from_utf8(bytes(ptr, len)).unwrap_or("")
}

// ---------------------------------------------------------------- Speicher

/// Platz für `len` Bytes (Argumente des Skripts).
#[no_mangle]
pub extern "C" fn kw_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// Nur mit Zeiger und Länge aus [`kw_alloc`].
#[no_mangle]
pub unsafe extern "C" fn kw_free(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
}

#[no_mangle]
pub extern "C" fn kw_out_ptr() -> *const u8 {
    out().as_ptr()
}

#[no_mangle]
pub extern "C" fn kw_out_len() -> usize {
    out().len()
}

// ---------------------------------------------------------------- Start

fn engine_post(kind: Kind, d: &[f64]) {
    // SAFETY: gültige Zahlenfolge, das Skript kopiert sie sofort.
    unsafe { js::engine_post(kind as u32, d.as_ptr(), d.len()) }
}

fn engine_buffer(id: u32, d: &[f32]) {
    // SAFETY: gültiger Puffer, das Skript kopiert ihn sofort.
    unsafe { js::engine_buffer(id, d.as_ptr(), d.len()) }
}

fn now_ms() -> f64 {
    // SAFETY: einfacher Aufruf ohne Zeiger.
    unsafe { js::now_ms() }
}

fn mono_ms() -> f64 {
    // SAFETY: einfacher Aufruf ohne Zeiger.
    unsafe { js::mono_ms() }
}

fn mono_secs() -> f64 {
    mono_ms() / 1000.0
}

/// Kern starten – nachdem das Skript die gesicherten Dateien mit
/// [`kw_fs_put`] zurückgelegt hat. `audio` = 1: das AudioWorklet läuft mit
/// `sample_rate`; 0: ohne Ton (alles andere geht).
#[no_mangle]
pub extern "C" fn kw_init(sample_rate: u32, audio: u32) {
    std::panic::set_hook(Box::new(|info| log(&format!("Klangwelt (Web-Kern): {info}"))));
    klang_app::clock::set(now_ms, mono_ms);
    if audio != 0 {
        klang_engine::web::connect(klang_engine::web::WebPort {
            sample_rate,
            device: format!("Web Audio ({sample_rate} Hz)"),
            post: engine_post,
            buffer: engine_buffer,
            now: mono_secs,
        });
    }
    let st = AppState::new();
    st.set_host(Box::new(WebHost));
    // Gemerkte Einstellungen anwenden (Metronom an/aus vom letzten Mal)
    let file = PathBuf::from(SETTINGS);
    if let Some(on) = settings::read(&file).get("metronom").and_then(Value::as_bool) {
        let _ = st.set_metronome(on);
    }
    *st.settings_file.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    // Library: own scores now (from the file tree), the package on the server
    // when the worker script attaches it (`library_web_attach`).
    klang_app::weblib::init(&st);
    let _ = STATE.set(st);
}

/// Warum es keinen Agent-Server gibt (erscheint in der Oberfläche wie auf dem
/// Desktop ein belegter Port).
///
/// # Safety
/// Zeiger und Länge eines gültigen UTF-8-Texts.
#[no_mangle]
pub unsafe extern "C" fn kw_agent_hint(ptr: *const u8, len: usize) {
    if let Some(st) = STATE.get() {
        if let Ok(mut a) = st.agent.lock() {
            a.error = Some(text(ptr, len).to_string());
        }
    }
}

// ---------------------------------------------------------------- Befehle

/// Befehl der Oberfläche ausführen (Name, Argumente als JSON). 0 = Antwort
/// (JSON) im Ausgabepuffer, 1 = Fehlertext im Ausgabepuffer, 2 = es fehlen
/// Daten vom Server: im Ausgabepuffer `[{file, start, end}, …]` – das Skript
/// holt sie, übergibt sie mit [`kw_lib_feed`] und ruft den Befehl erneut auf
/// (siehe `klang_music::source::remote`).
///
/// # Safety
/// Zeiger und Längen gültiger UTF-8-Texte.
#[no_mangle]
pub unsafe extern "C" fn kw_invoke(cmd: *const u8, cmd_len: usize, args: *const u8, args_len: usize) -> u32 {
    let Some(st) = STATE.get() else {
        set_out(b"Der Web-Kern ist noch nicht gestartet.".to_vec());
        return 1;
    };
    let args: Value = match bytes(args, args_len) {
        [] => Value::Null,
        b => match serde_json::from_slice(b) {
            Ok(v) => v,
            Err(e) => {
                set_out(klang_i18n::tr!("Invalid input: {error}", error = e).into_bytes());
                return 1;
            }
        },
    };
    remote::begin();
    let result = match web_command(st, text(cmd, cmd_len), &args) {
        Some(r) => r,
        None => commands::call(st, text(cmd, cmd_len), &args),
    };
    if let Some(need) = missing() {
        set_out(need);
        return 2;
    }
    match result {
        Ok(json) => {
            set_out(json.into_bytes());
            0
        }
        Err(e) => {
            set_out(e.into_bytes());
            1
        }
    }
}

/// Ranges the last command missed (JSON), if any.
fn missing() -> Option<Vec<u8>> {
    let need = remote::take_missing();
    (!need.is_empty()).then(|| serde_json::to_vec(&need).unwrap_or_default())
}

/// Commands that only the browser has – on the desktop its shell handles
/// libraries (`app/src/library.rs`). The work is done in the core (`weblib`).
fn web_command(st: &AppState, cmd: &str, args: &Value) -> Option<Result<String, String>> {
    use klang_app::weblib;
    let str_arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    fn json<T: serde::Serialize>(r: Result<T, String>) -> Result<String, String> {
        r.and_then(|v| serde_json::to_string(&v).map_err(|e| e.to_string()))
    }
    Some(match cmd {
        "library_add" => {
            let paths: Vec<String> = args.get("paths").and_then(|p| serde_json::from_value(p.clone()).ok()).unwrap_or_default();
            json(weblib::add(st, &paths))
        }
        "library_own_list" => json(weblib::list(st)),
        "library_own_remove" => json(weblib::remove(st, &str_arg("path").unwrap_or_default())),
        "library_web_attach" => json(weblib::attach(st, args.get("manifest").unwrap_or(&Value::Null))),
        "library_web_state" => json(weblib::set_remote_state(st, &str_arg("state").unwrap_or_default(), str_arg("error"))),
        _ => return None,
    })
}

/// Bytes of a file of the library package (`file`, from `start`) – the
/// answer to status 2 of [`kw_invoke`]. 0 = taken, 1 = unknown file.
///
/// # Safety
/// Zeiger und Längen gültiger Bereiche.
#[no_mangle]
pub unsafe extern "C" fn kw_lib_feed(file: *const u8, file_len: usize, start: f64, data: *const u8, data_len: usize) -> u32 {
    u32::from(!remote::feed(text(file, file_len), start.max(0.0) as u64, bytes(data, data_len).to_vec()))
}

/// Die Plattform „Browser“ für die Agent-Werkzeuge: alles außer Bibliotheken
/// verwalten und herunterladen (das gibt es hier noch nicht).
struct WebAgentShell;

impl klang_app::agent::AgentShell for WebAgentShell {
    fn state(&self) -> &AppState {
        STATE.get().expect("Web-Kern gestartet")
    }
}

/// Eine MCP-Nachricht (JSON-RPC) an die Agent-Werkzeuge – dieselben wie der
/// MCP-Server des Desktops. Antwort als JSON im Ausgabepuffer (leer bei
/// Benachrichtigungen). 0 = Antwort, 2 = es fehlen Daten vom Server (wie bei
/// [`kw_invoke`]).
///
/// # Safety
/// Zeiger und Länge eines gültigen UTF-8-Texts.
#[no_mangle]
pub unsafe extern "C" fn kw_mcp(msg: *const u8, len: usize) -> u32 {
    if STATE.get().is_none() {
        set_out(br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Der Web-Kern ist noch nicht gestartet."}}"#.to_vec());
        return 0;
    }
    remote::begin();
    let reply = match serde_json::from_slice::<Value>(bytes(msg, len)) {
        Ok(m) => klang_mcp::handle(&klang_app::agent::Agent::new(WebAgentShell), &m),
        Err(e) => Some(serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Ungültiges JSON: {e}") } })),
    };
    if let Some(need) = missing() {
        set_out(need);
        return 2;
    }
    set_out(reply.map(|r| r.to_string().into_bytes()).unwrap_or_default());
    0
}

/// Status für die Oberfläche („engine-status“) als JSON im Ausgabepuffer –
/// wie der Status-Thread des Desktops, aufgerufen vom Skript alle 33 ms.
#[no_mangle]
pub extern "C" fn kw_status() {
    if let Some(st) = STATE.get() {
        set_out(serde_json::to_vec(&st.status()).unwrap_or_default());
    }
}

/// Was der Engine-Thread des Desktops nebenbei tut (gestreamtes Vorhören nachfüllen).
#[no_mangle]
pub extern "C" fn kw_tick() {
    if let Some(e) = STATE.get().and_then(|st| st.engine().ok()) {
        e.tick();
    }
}

/// Status aus dem Worklet.
#[no_mangle]
pub extern "C" fn kw_engine_report(playing: u32, previewing: u32, position_frames: f64, peak_out: f32, peak_in: f32) {
    klang_engine::web::report(playing != 0, previewing != 0, position_frames.max(0.0) as u64, peak_out, peak_in);
}

/// Mikrofon-Samples der laufenden Aufnahme (mono, Ausgaberate) – `len` f32
/// als Bytes (Little Endian) aus [`kw_alloc`], daher ohne Annahme über die Ausrichtung.
///
/// # Safety
/// `ptr` zeigt auf `4 · len` Bytes.
#[no_mangle]
pub unsafe extern "C" fn kw_record_input(ptr: *const u8, len: usize) {
    if len > 0 {
        let samples: Vec<f32> = bytes(ptr, len * 4).chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        klang_engine::web::record_input(&samples);
    }
}

// ---------------------------------------------------------------- Dateien

/// Gesicherte Datei beim Start zurücklegen (gilt nicht als geändert).
///
/// # Safety
/// Zeiger und Längen gültiger Bereiche.
#[no_mangle]
pub unsafe extern "C" fn kw_fs_put(path: *const u8, path_len: usize, data: *const u8, data_len: usize, modified_ms: f64) {
    vfs::put(&PathBuf::from(text(path, path_len)), bytes(data, data_len).to_vec(), modified_ms.max(0.0) as u64);
}

/// Datei schreiben (z. B. eine importierte Projektdatei); wird gesichert.
///
/// # Safety
/// Zeiger und Längen gültiger Bereiche.
#[no_mangle]
pub unsafe extern "C" fn kw_fs_write(path: *const u8, path_len: usize, data: *const u8, data_len: usize) -> u32 {
    let path = PathBuf::from(text(path, path_len));
    match vfs::write(&path, bytes(data, data_len)) {
        Ok(()) => 0,
        Err(e) => {
            set_out(e.to_string().into_bytes());
            1
        }
    }
}

/// Datei lesen (z. B. ein Export zum Herunterladen): 0 = Inhalt im
/// Ausgabepuffer, 1 = Fehlertext.
///
/// # Safety
/// Zeiger und Länge eines gültigen Texts.
#[no_mangle]
pub unsafe extern "C" fn kw_fs_read(path: *const u8, path_len: usize) -> u32 {
    match vfs::read(&PathBuf::from(text(path, path_len))) {
        Ok(b) => {
            set_out(b);
            0
        }
        Err(e) => {
            set_out(e.to_string().into_bytes());
            1
        }
    }
}

/// Geänderte Dateien seit dem letzten Aufruf: Anzahl; im Ausgabepuffer als
/// JSON `[[Pfad, Länge oder -1 (gelöscht)], …]`. Inhalte mit
/// [`kw_fs_change_data`], danach [`kw_fs_changes_done`].
#[no_mangle]
pub extern "C" fn kw_fs_changes() -> usize {
    let mut staged = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
    *staged = vfs::take_changes();
    let list: Vec<(String, i64)> = staged.iter().map(|(p, d)| (p.display().to_string(), d.as_ref().map(|d| d.len() as i64).unwrap_or(-1))).collect();
    set_out(serde_json::to_vec(&list).unwrap_or_default());
    staged.len()
}

/// Inhalt der `i`-ten geänderten Datei (Länge aus der Liste).
#[no_mangle]
pub extern "C" fn kw_fs_change_data(i: usize) -> *const u8 {
    let staged = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
    staged.get(i).and_then(|(_, d)| d.as_ref()).map(|d| d.as_ptr()).unwrap_or(std::ptr::null())
}

#[no_mangle]
pub extern "C" fn kw_fs_changes_done() {
    CHANGES.lock().unwrap_or_else(|e| e.into_inner()).clear();
}
