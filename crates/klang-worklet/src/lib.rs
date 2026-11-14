//! Mischer im AudioWorklet (Browser) – das Gegenstück zum Audio-Callback des
//! Desktops (`klang-engine/src/device.rs`): derselbe [`Mixer`] mit denselben
//! Nachrichten, nur dass sie per `postMessage` aus dem Web-Kern kommen
//! ([`klang_engine::wire`]).
//!
//! Schnittstelle zum Worklet-Skript (`ui/src/web/worklet.ts`), alles über den
//! linearen Speicher:
//! * `kwl_init(rate)` – Mischer anlegen
//! * `kwl_msg_buf(n)` → Zeiger auf `n` f64; danach `kwl_msg(art, n)`
//! * `kwl_buffer_alloc(id, len)` → Zeiger auf `len` f32 zum Befüllen;
//!   `kwl_buffer_done(id)` macht ihn benutzbar
//! * `kwl_process(frames)` → Zeiger auf `2·frames` f32 (links/rechts verschränkt)
//! * `kwl_status()` → Zeiger auf [spielt, Vorhören, Position, Spitzenpegel]
//!
//! Auf anderen Zielen als wasm32 ist die Crate leer.

#![cfg(target_arch = "wasm32")]

use klang_engine::wire::{self, Kind};
use klang_engine::Mixer;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

struct Worklet {
    mixer: Mixer,
    /// Puffer, auf die Nachrichten verweisen dürfen.
    buffers: HashMap<u32, Arc<[f32]>>,
    /// Puffer, die gerade befüllt werden.
    pending: HashMap<u32, Arc<[f32]>>,
    msg: Vec<f64>,
    out: Vec<f32>,
    status: [f64; 4],
}

static WORKLET: Mutex<Option<Worklet>> = Mutex::new(None);

fn worklet() -> MutexGuard<'static, Option<Worklet>> {
    WORKLET.lock().unwrap_or_else(|e| e.into_inner())
}

#[no_mangle]
pub extern "C" fn kwl_init(sample_rate: f32) {
    *worklet() = Some(Worklet {
        mixer: Mixer::new(sample_rate, 2),
        buffers: HashMap::new(),
        pending: HashMap::new(),
        msg: Vec::new(),
        out: Vec::new(),
        status: [0.0; 4],
    });
}

/// Platz für eine Nachricht mit `n` Zahlen.
#[no_mangle]
pub extern "C" fn kwl_msg_buf(n: usize) -> *mut f64 {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return std::ptr::null_mut() };
    w.msg.clear();
    w.msg.resize(n, 0.0);
    w.msg.as_mut_ptr()
}

/// Nachricht (Art, die ersten `n` Zahlen aus `kwl_msg_buf`) an den Mischer.
#[no_mangle]
pub extern "C" fn kwl_msg(kind: u32, n: usize) {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return };
    let Some(kind) = Kind::from_u32(kind) else { return };
    let d = &w.msg[..n.min(w.msg.len())];
    match kind {
        Kind::Mute => w.mixer.set_muted(d.first().copied().unwrap_or(0.0) != 0.0),
        Kind::Free => {
            for id in d {
                w.buffers.remove(&(*id as u32));
            }
        }
        // Aufnahme sammelt das Worklet-Skript selbst (Mikrofon-Eingang)
        Kind::Record => {}
        _ => {
            if let Some(msg) = wire::decode(kind, d, &w.buffers) {
                w.mixer.send(msg);
            }
        }
    }
}

/// Neuer Puffer mit `len` Samples (Nullen); das Skript füllt ihn über den Zeiger.
#[no_mangle]
pub extern "C" fn kwl_buffer_alloc(id: u32, len: usize) -> *mut f32 {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return std::ptr::null_mut() };
    let mut buf: Arc<[f32]> = Arc::<[f32]>::new_zeroed_slice(len).into_init();
    let ptr = Arc::get_mut(&mut buf).map(|s| s.as_mut_ptr()).unwrap_or(std::ptr::null_mut());
    w.pending.insert(id, buf);
    ptr
}

/// Puffer ist befüllt – ab jetzt dürfen Nachrichten auf ihn verweisen.
#[no_mangle]
pub extern "C" fn kwl_buffer_done(id: u32) {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return };
    if let Some(b) = w.pending.remove(&id) {
        w.buffers.insert(id, b);
    }
}

/// Mischt `frames` Frames; Zeiger auf die verschränkten Samples (links, rechts).
#[no_mangle]
pub extern "C" fn kwl_process(frames: usize) -> *const f32 {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return std::ptr::null() };
    if w.out.len() != frames * 2 {
        w.out.resize(frames * 2, 0.0);
    }
    w.mixer.process(&mut w.out);
    w.out.as_ptr()
}

/// [spielt (0/1), Vorhören (0/1), Position (Frames), Spitzenpegel seit dem letzten Aufruf]
#[no_mangle]
pub extern "C" fn kwl_status() -> *const f64 {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return std::ptr::null() };
    let s = w.mixer.status();
    w.status = [s.playing as u8 as f64, s.previewing as u8 as f64, s.position_frames as f64, s.peak_out as f64];
    w.status.as_ptr()
}

trait IntoInit {
    fn into_init(self) -> Arc<[f32]>;
}

impl IntoInit for Arc<[std::mem::MaybeUninit<f32>]> {
    fn into_init(self) -> Arc<[f32]> {
        // SAFETY: mit Nullen angelegt (`new_zeroed_slice`) – 0.0 ist ein gültiges f32.
        unsafe { self.assume_init() }
    }
}
