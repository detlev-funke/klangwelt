//! Web core: Klangwelt in the browser – the shell around the shared core
//! (`klang-app`), counterpart to the Tauri shell (`app/src/lib.rs`).
//!
//! | Desktop (Tauri)                    | Browser (this core)                           |
//! |------------------------------------|-----------------------------------------------|
//! | Tauri IPC → `commands::call`       | `kw_invoke` → `commands::call`                |
//! | `DesktopHost`: Tauri events, folders | [`WebHost`]: events to the script, `/Klangwelt/…` |
//! | Engine on the device (cpal thread) | Engine → AudioWorklet (`klang_engine::web`)   |
//! | Status thread every 33 ms          | Worker script calls `kw_status` every 33 ms   |
//! | File system                        | File tree in memory, backed up in OPFS        |
//!
//! The worker script (`ui/src/web/worker.ts`) loads this module. The
//! interface is slim and needs no extra tooling: numbers and
//! pointers into linear memory; texts as UTF-8 (JSON as with Tauri IPC).
//! Answers then lie in a buffer (`kw_out_ptr`, `kw_out_len`).
//!
//! On targets other than wasm32 the crate is empty.

#![cfg(target_arch = "wasm32")]

use klang_app::{commands, settings, vfs, AppState, Host};
use klang_engine::wire::Kind;
use klang_music::source::remote;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// What the worker script provides (import module "klangwelt").
mod js {
    #[link(wasm_import_module = "klangwelt")]
    extern "C" {
        /// Event to the UI (name, JSON).
        pub fn emit(event: *const u8, event_len: usize, payload: *const u8, payload_len: usize);
        /// Message to the worklet (kind, numbers).
        pub fn engine_post(kind: u32, data: *const f64, len: usize);
        /// Sound buffer to the worklet.
        pub fn engine_buffer(id: u32, data: *const f32, len: usize);
        /// Date.now()
        pub fn now_ms() -> f64;
        /// performance.now()
        pub fn mono_ms() -> f64;
        /// Message on the console (e.g. on a crash).
        pub fn log(msg: *const u8, len: usize);
    }
}

/// Storage in the file tree (backed up in the browser's storage).
const PROJECTS: &str = "/Klangwelt/Projekte";
const LIBRARY: &str = "/Klangwelt/Bibliothek";
const SETTINGS: &str = "/Klangwelt/einstellungen.json";

/// The "browser" platform for the core.
struct WebHost;

impl Host for WebHost {
    fn emit_json(&self, event: &str, p: String) {
        // SAFETY: pointers and lengths of valid texts, the script reads them immediately.
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
    // SAFETY: valid text.
    unsafe { js::log(msg.as_ptr(), msg.len()) }
}

static STATE: OnceLock<AppState> = OnceLock::new();
/// Answer of the last call.
static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Changed files, until the script has backed them up.
type Change = (PathBuf, Option<Arc<Vec<u8>>>);
static CHANGES: Mutex<Vec<Change>> = Mutex::new(Vec::new());

fn out() -> MutexGuard<'static, Vec<u8>> {
    OUT.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_out(bytes: Vec<u8>) {
    *out() = bytes;
}

/// # Safety
/// `ptr` points to `len` bytes in linear memory.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

/// # Safety
/// like [`bytes`]
unsafe fn text<'a>(ptr: *const u8, len: usize) -> &'a str {
    std::str::from_utf8(bytes(ptr, len)).unwrap_or("")
}

// ---------------------------------------------------------------- Memory

/// Space for `len` bytes (arguments of the script).
#[no_mangle]
pub extern "C" fn kw_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// Only with pointer and length from [`kw_alloc`].
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
    // SAFETY: valid sequence of numbers, the script copies it immediately.
    unsafe { js::engine_post(kind as u32, d.as_ptr(), d.len()) }
}

fn engine_buffer(id: u32, d: &[f32]) {
    // SAFETY: valid buffer, the script copies it immediately.
    unsafe { js::engine_buffer(id, d.as_ptr(), d.len()) }
}

fn now_ms() -> f64 {
    // SAFETY: simple call without pointers.
    unsafe { js::now_ms() }
}

fn mono_ms() -> f64 {
    // SAFETY: simple call without pointers.
    unsafe { js::mono_ms() }
}

fn mono_secs() -> f64 {
    mono_ms() / 1000.0
}

/// Starts the core – after the script has put the backed-up files back with
/// [`kw_fs_put`]. `audio` = 1: the AudioWorklet runs at
/// `sample_rate`; 0: without sound (everything else works).
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
    // Apply remembered settings (metronome on/off from last time)
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

/// Why there is no agent server (shown in the UI the way a busy port
/// is on the desktop).
///
/// # Safety
/// Pointer and length of a valid UTF-8 text.
#[no_mangle]
pub unsafe extern "C" fn kw_agent_hint(ptr: *const u8, len: usize) {
    if let Some(st) = STATE.get() {
        if let Ok(mut a) = st.agent.lock() {
            a.error = Some(text(ptr, len).to_string());
        }
    }
}

// ---------------------------------------------------------------- Commands

/// Runs a UI command (name, arguments as JSON). 0 = answer
/// (JSON) in the output buffer, 1 = error text in the output buffer, 2 = data
/// from the server is missing: the output buffer holds `[{file, start, end}, …]` – the script
/// fetches it, passes it in with [`kw_lib_feed`] and calls the command again
/// (see `klang_music::source::remote`).
///
/// # Safety
/// Pointers and lengths of valid UTF-8 texts.
#[no_mangle]
pub unsafe extern "C" fn kw_invoke(cmd: *const u8, cmd_len: usize, args: *const u8, args_len: usize) -> u32 {
    let Some(st) = STATE.get() else {
        set_out(b"The web core has not started yet.".to_vec());
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
/// Pointers and lengths of valid ranges.
#[no_mangle]
pub unsafe extern "C" fn kw_lib_feed(file: *const u8, file_len: usize, start: f64, data: *const u8, data_len: usize) -> u32 {
    u32::from(!remote::feed(text(file, file_len), start.max(0.0) as u64, bytes(data, data_len).to_vec()))
}

/// The "browser" platform for the agent tools: manage and download everything
/// except libraries (those don't exist here yet).
struct WebAgentShell;

impl klang_app::agent::AgentShell for WebAgentShell {
    fn state(&self) -> &AppState {
        STATE.get().expect("Web-Kern gestartet")
    }
}

/// An MCP message (JSON-RPC) to the agent tools – the same as the desktop's
/// MCP server. Answer as JSON in the output buffer (empty for
/// notifications). 0 = answer, 2 = data from the server is missing (as with
/// [`kw_invoke`]).
///
/// # Safety
/// Pointer and length of a valid UTF-8 text.
#[no_mangle]
pub unsafe extern "C" fn kw_mcp(msg: *const u8, len: usize) -> u32 {
    if STATE.get().is_none() {
        set_out(br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"The web core has not started yet."}}"#.to_vec());
        return 0;
    }
    remote::begin();
    let reply = match serde_json::from_slice::<Value>(bytes(msg, len)) {
        Ok(m) => klang_mcp::handle(&klang_app::agent::Agent::new(WebAgentShell), &m),
        Err(e) => Some(serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Invalid JSON: {e}") } })),
    };
    if let Some(need) = missing() {
        set_out(need);
        return 2;
    }
    set_out(reply.map(|r| r.to_string().into_bytes()).unwrap_or_default());
    0
}

/// Status for the UI ("engine-status") as JSON in the output buffer –
/// like the desktop's status thread, called by the script every 33 ms.
#[no_mangle]
pub extern "C" fn kw_status() {
    if let Some(st) = STATE.get() {
        set_out(serde_json::to_vec(&st.status()).unwrap_or_default());
    }
}

/// What the desktop's engine thread does on the side (refilling streamed
/// preview), and the core's work on the side (pre-rendering when the live
/// mixer can't keep up).
#[no_mangle]
pub extern "C" fn kw_tick() {
    if let Some(st) = STATE.get() {
        if let Ok(e) = st.engine() {
            e.tick();
        }
        st.tick();
    }
}

/// Status from the worklet.
#[no_mangle]
pub extern "C" fn kw_engine_report(playing: u32, previewing: u32, position_frames: f64, peak_out: f32, peak_in: f32, load: f32) {
    klang_engine::web::report(playing != 0, previewing != 0, position_frames.max(0.0) as u64, peak_out, peak_in, load);
}

/// Microphone samples of the running recording (mono, output rate) – `len` f32
/// as bytes (little-endian) from [`kw_alloc`], hence with no assumption about alignment.
///
/// # Safety
/// `ptr` points to `4 · len` bytes.
#[no_mangle]
pub unsafe extern "C" fn kw_record_input(ptr: *const u8, len: usize) {
    if len > 0 {
        let samples: Vec<f32> = bytes(ptr, len * 4).chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        klang_engine::web::record_input(&samples);
    }
}

// ---------------------------------------------------------------- Files

/// Puts a backed-up file back at startup (doesn't count as changed).
///
/// # Safety
/// Pointers and lengths of valid ranges.
#[no_mangle]
pub unsafe extern "C" fn kw_fs_put(path: *const u8, path_len: usize, data: *const u8, data_len: usize, modified_ms: f64) {
    vfs::put(&PathBuf::from(text(path, path_len)), bytes(data, data_len).to_vec(), modified_ms.max(0.0) as u64);
}

/// Writes a file (e.g. an imported project file); it is backed up.
///
/// # Safety
/// Pointers and lengths of valid ranges.
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

/// Reads a file (e.g. an export to download): 0 = contents in the
/// output buffer, 1 = error text.
///
/// # Safety
/// Pointer and length of a valid text.
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

/// Files changed since the last call: count; in the output buffer as
/// JSON `[[path, length or -1 (deleted)], …]`. Contents via
/// [`kw_fs_change_data`], then [`kw_fs_changes_done`].
#[no_mangle]
pub extern "C" fn kw_fs_changes() -> usize {
    let mut staged = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
    *staged = vfs::take_changes();
    let list: Vec<(String, i64)> = staged.iter().map(|(p, d)| (p.display().to_string(), d.as_ref().map(|d| d.len() as i64).unwrap_or(-1))).collect();
    set_out(serde_json::to_vec(&list).unwrap_or_default());
    staged.len()
}

/// Contents of the `i`-th changed file (length from the list).
#[no_mangle]
pub extern "C" fn kw_fs_change_data(i: usize) -> *const u8 {
    let staged = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
    staged.get(i).and_then(|(_, d)| d.as_ref()).map(|d| d.as_ptr()).unwrap_or(std::ptr::null())
}

#[no_mangle]
pub extern "C" fn kw_fs_changes_done() {
    CHANGES.lock().unwrap_or_else(|e| e.into_inner()).clear();
}
