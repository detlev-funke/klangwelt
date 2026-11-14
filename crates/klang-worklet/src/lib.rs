//! Mixer in the AudioWorklet (browser) – the counterpart to the audio callback of the
//! desktop (`klang-engine/src/device.rs`): the same [`Mixer`] with the same
//! messages, except that they arrive via `postMessage` from the web core
//! ([`klang_engine::wire`]).
//!
//! Interface to the worklet script (`ui/src/web/worklet.ts`), everything via
//! linear memory:
//! * `kwl_init(rate)` – create the mixer
//! * `kwl_msg_buf(n)` → pointer to `n` f64; then `kwl_msg(art, n)`
//! * `kwl_buffer_alloc(id, len)` → pointer to `len` f32 to fill;
//!   `kwl_buffer_done(id)` makes it usable
//! * `kwl_process(frames)` → pointer to `2·frames` f32 (left/right interleaved)
//! * `kwl_status()` → pointer to [playing, preview, position, peak level]
//!
//! On targets other than wasm32 the crate is empty.

#![cfg(target_arch = "wasm32")]

use klang_engine::wire::{self, Kind};
use klang_engine::Mixer;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

struct Worklet {
    mixer: Mixer,
    /// Buffers that messages may refer to.
    buffers: HashMap<u32, Arc<[f32]>>,
    /// Buffers that are currently being filled.
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

/// Space for a message with `n` numbers.
#[no_mangle]
pub extern "C" fn kwl_msg_buf(n: usize) -> *mut f64 {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return std::ptr::null_mut() };
    w.msg.clear();
    w.msg.resize(n, 0.0);
    w.msg.as_mut_ptr()
}

/// Message (kind, the first `n` numbers from `kwl_msg_buf`) to the mixer.
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
        // The worklet script collects the recording itself (microphone input)
        Kind::Record => {}
        _ => {
            if let Some(msg) = wire::decode(kind, d, &w.buffers) {
                w.mixer.send(msg);
            }
        }
    }
}

/// New buffer with `len` samples (zeros); the script fills it via the pointer.
#[no_mangle]
pub extern "C" fn kwl_buffer_alloc(id: u32, len: usize) -> *mut f32 {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return std::ptr::null_mut() };
    let mut buf: Arc<[f32]> = Arc::<[f32]>::new_zeroed_slice(len).into_init();
    let ptr = Arc::get_mut(&mut buf).map(|s| s.as_mut_ptr()).unwrap_or(std::ptr::null_mut());
    w.pending.insert(id, buf);
    ptr
}

/// Buffer is filled – from now on messages may refer to it.
#[no_mangle]
pub extern "C" fn kwl_buffer_done(id: u32) {
    let mut g = worklet();
    let Some(w) = g.as_mut() else { return };
    if let Some(b) = w.pending.remove(&id) {
        w.buffers.insert(id, b);
    }
}

/// Mixes `frames` frames; pointer to the interleaved samples (left, right).
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

/// [playing (0/1), preview (0/1), position (frames), peak level since the last call]
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
        // SAFETY: created with zeros (`new_zeroed_slice`) – 0.0 is a valid f32.
        unsafe { self.assume_init() }
    }
}
