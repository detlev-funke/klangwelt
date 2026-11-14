//! Engine in the browser – the same interface as on the desktop
//! (`device.rs`), but without a device and without threads:
//!
//! * Mixing happens in an AudioWorklet: the same [`Renderer`] runs there
//!   as [`crate::Mixer`] (its own WebAssembly instance).
//! * The engine sends it the same messages ([`RtMsg`]) as a sequence of numbers
//!   ([`crate::wire`]); sound buffers are transferred once per buffer and are
//!   afterwards referred to only by their id.
//! * Status (position, level) and microphone samples are reported back by the worklet;
//!   the web core passes them in with [`report`] and [`record_input`].
//! * What the engine thread does on the side on the desktop (refilling streamed
//!   previews) is done here by [`Engine::tick`], called by the web core
//!   regularly.
//!
//! The web core sets the connection to the worklet ([`WebPort`]) with
//! [`connect`] before it starts the engine.

use crate::wire::{self, Kind};
use klang_i18n::tr;
use crate::{update_peak, Arrangement, AudioDevices, EngineError, EngineInfo, EngineStatus, LiveNotes, PlacedClip, PreviewSource, PreviewStream, Recording, RtMsg, Shared, STREAM_AHEAD_SECS};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Connection to the audio output in the browser.
#[derive(Clone)]
pub struct WebPort {
    pub sample_rate: u32,
    /// Name for the UI ("Web Audio …").
    pub device: String,
    /// Message to the worklet.
    pub post: fn(Kind, &[f64]),
    /// Sound buffer to the worklet (id, samples).
    pub buffer: fn(u32, &[f32]),
    /// Monotonic clock in seconds.
    pub now: fn() -> f64,
}

static PORT: Mutex<Option<WebPort>> = Mutex::new(None);

static SHARED: Shared = Shared {
    playing: AtomicBool::new(false),
    previewing: AtomicBool::new(false),
    recording: AtomicBool::new(false),
    position: AtomicU64::new(0),
    peak_out: AtomicU32::new(0),
    peak_in: AtomicU32::new(0),
    muted: AtomicBool::new(false),
    sample_rate: AtomicU32::new(0),
    load: AtomicU32::new(0),
};

/// Microphone samples of the current recording.
static REC: Mutex<Option<Vec<f32>>> = Mutex::new(None);

/// Establish the connection to the worklet (before [`Engine::start`]).
pub fn connect(port: WebPort) {
    *PORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(port);
}

fn port() -> Option<WebPort> {
    PORT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Status from the worklet (like the atomics that the audio callback
/// sets on the desktop). Peak levels accumulate until the next read; `load`
/// = share of the real-time budget the worklet needs (measured there).
pub fn report(playing: bool, previewing: bool, position_frames: u64, peak_out: f32, peak_in: f32, load: f32) {
    SHARED.load.store(load.max(0.0).to_bits(), Ordering::Relaxed);
    SHARED.playing.store(playing, Ordering::Relaxed);
    SHARED.previewing.store(previewing, Ordering::Relaxed);
    SHARED.position.store(position_frames, Ordering::Relaxed);
    update_peak(&SHARED.peak_out, peak_out);
    update_peak(&SHARED.peak_in, peak_in);
}

/// Microphone samples (mono, output rate) from the worklet.
pub fn record_input(samples: &[f32]) {
    if let Some(rec) = REC.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        rec.extend_from_slice(samples);
    }
}

#[derive(Default)]
struct Inner {
    /// Buffers that the worklet has: address → (id, buffer). The buffer
    /// is kept so that its address is not reassigned.
    known: HashMap<usize, (u32, Arc<[f32]>)>,
    next_id: u32,
    /// Addresses of the buffers in the current arrangement and preview respectively.
    in_arrangement: HashSet<usize>,
    in_preview: HashSet<usize>,
    /// Note clips the worklet has (address → id, clip) and the buffers they use (organ tables).
    notes: HashMap<usize, (u32, Arc<LiveNotes>)>,
    next_notes_id: u32,
    in_notes: HashSet<usize>,
    stream: Option<PreviewStream>,
}

fn addr(b: &Arc<[f32]>) -> usize {
    Arc::as_ptr(b) as *const f32 as usize
}

fn notes_addr(n: &Arc<LiveNotes>) -> usize {
    Arc::as_ptr(n) as usize
}

impl Inner {
    /// Id of a buffer; a new one goes to the worklet in the process.
    fn id_for(&mut self, port: &WebPort, b: &Arc<[f32]>) -> u32 {
        let a = addr(b);
        if let Some((id, _)) = self.known.get(&a) {
            return *id;
        }
        self.next_id += 1;
        let id = self.next_id;
        (port.buffer)(id, b);
        self.known.insert(a, (id, b.clone()));
        id
    }

    /// Clips to the worklet (after the head `head`); returns the addresses of the
    /// buffers used.
    fn send_clips(&mut self, port: &WebPort, kind: Kind, head: &[f64], arr: &Arrangement) -> HashSet<usize> {
        let mut used = HashSet::new();
        let mut d = head.to_vec();
        d.extend(wire::encode_clips(arr, |b| {
            used.insert(addr(b));
            self.id_for(port, b)
        }));
        (port.post)(kind, &d);
        used
    }

    /// Id of a note clip; a new one goes to the worklet in the process (its
    /// organ tables as buffers beforehand).
    fn notes_id(&mut self, port: &WebPort, n: &Arc<LiveNotes>) -> u32 {
        let a = notes_addr(n);
        if let Some((id, _)) = self.notes.get(&a) {
            return *id;
        }
        self.next_notes_id += 1;
        let id = self.next_notes_id;
        let mut tables = Vec::new();
        let d = wire::encode_notes(id, n, |b| {
            tables.push(addr(b));
            self.id_for(port, b)
        });
        self.in_notes.extend(tables);
        (port.post)(Kind::Notes, &d);
        self.notes.insert(a, (id, n.clone()));
        id
    }

    /// Release note clips the arrangement no longer uses.
    fn collect_notes(&mut self, port: &WebPort, used: &HashSet<usize>) {
        let gone: Vec<usize> = self.notes.keys().copied().filter(|a| !used.contains(a)).collect();
        if gone.is_empty() {
            return;
        }
        let ids: Vec<f64> = gone.iter().filter_map(|a| self.notes.remove(a)).map(|(id, _)| id as f64).collect();
        (port.post)(Kind::FreeNotes, &ids);
        self.in_notes = self.notes.values().flat_map(|(_, n)| n.tables().iter().flat_map(|t| [addr(&t.low), addr(&t.high)])).collect();
    }

    /// Release buffers that neither the arrangement nor the preview needs.
    fn collect(&mut self, port: &WebPort) {
        let keep = |a: &usize| self.in_arrangement.contains(a) || self.in_preview.contains(a) || self.in_notes.contains(a);
        let gone: Vec<usize> = self.known.keys().copied().filter(|a| !keep(a)).collect();
        if gone.is_empty() {
            return;
        }
        let ids: Vec<f64> = gone.iter().filter_map(|a| self.known.remove(a)).map(|(id, _)| id as f64).collect();
        (port.post)(Kind::Free, &ids);
    }
}

pub struct Engine {
    info: EngineInfo,
    port: WebPort,
    inner: Mutex<Inner>,
}

impl Engine {
    /// Starts the engine on the output set with [`connect`].
    pub fn start() -> Result<Self, EngineError> {
        let port = port().ok_or_else(|| EngineError(tr!("No audio output in the browser (AudioWorklet not available)")))?;
        let info = EngineInfo { output_device: port.device.clone(), sample_rate: port.sample_rate, channels: 2, ..Default::default() };
        Ok(Self { info, port, inner: Mutex::new(Inner::default()) })
    }

    pub fn info(&self) -> EngineInfo {
        self.info.clone()
    }

    /// In the browser the system chooses the devices (output of the AudioContext,
    /// microphone via the permission) – only this one pair is listed.
    pub fn devices(&self) -> Result<AudioDevices, EngineError> {
        Ok(AudioDevices {
            outputs: vec![self.info.output_device.clone()],
            inputs: vec!["Mikrofon".into()],
            default_output: Some(self.info.output_device.clone()),
            default_input: Some("Mikrofon".into()),
        })
    }

    /// There is no device selection in the browser; only the system default works.
    pub fn set_output_device(&self, name: Option<String>) -> Result<EngineInfo, EngineError> {
        match name {
            None => Ok(self.info()),
            Some(_) => Err(EngineError(tr!("In the browser, the system chooses the output device."))),
        }
    }

    pub fn set_input_device(&self, name: Option<String>) -> Result<EngineInfo, EngineError> {
        match name {
            None => Ok(self.info()),
            Some(_) => Err(EngineError(tr!("In the browser, the system chooses the microphone (grant access in the browser)."))),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, msg: RtMsg) {
        if let Some((k, d)) = wire::encode(&msg) {
            (self.port.post)(k, &d);
        }
    }

    pub fn play(&self) {
        self.send(RtMsg::Play);
    }

    pub fn stop(&self) {
        self.send(RtMsg::Stop);
    }

    pub fn seek_frames(&self, frame: u64) {
        self.send(RtMsg::Seek(frame));
    }

    /// During playback the position in beats is preserved.
    pub fn set_tempo(&self, bpm: f64, beats_per_bar: u32) {
        self.send(RtMsg::Tempo { bpm, beats_per_bar });
    }

    pub fn set_metronome(&self, on: bool) {
        self.send(RtMsg::Metronome(on));
    }

    /// Mute the output: playback, position and level keep running.
    pub fn set_muted(&self, on: bool) {
        SHARED.muted.store(on, Ordering::Relaxed);
        (self.port.post)(Kind::Mute, &[if on { 1.0 } else { 0.0 }]);
    }

    pub fn set_loop(&self, region: Option<(f64, f64)>) {
        self.send(RtMsg::Loop(region.filter(|(a, b)| b > a)));
    }

    pub fn set_stop_at(&self, at: Option<(f64, f64)>) {
        self.send(RtMsg::StopAt(at.filter(|(s, _)| *s > 0.0)));
    }

    /// New arrangement; during playback it is crossfaded.
    pub fn set_arrangement(&self, arrangement: Arrangement) {
        self.send_arrangement(arrangement, None);
    }

    /// New arrangement with a new tempo in one step (one message to the
    /// worklet): position in beats and sound change in the same block.
    pub fn set_arrangement_with_tempo(&self, arrangement: Arrangement, bpm: f64, beats_per_bar: u32) {
        self.send_arrangement(arrangement, Some((bpm, beats_per_bar)));
    }

    fn send_arrangement(&self, arrangement: Arrangement, tempo: Option<(f64, u32)>) {
        let mut g = self.inner();
        let g = &mut *g;
        // note clips first (they travel once), then the arrangement referring to them
        let notes: Vec<u32> = arrangement.notes.iter().map(|n| g.notes_id(&self.port, &n.notes)).collect();
        let mut ids = notes.into_iter();
        let mut used = HashSet::new();
        let d = wire::encode_arrangement(
            &arrangement,
            tempo,
            |b| {
                used.insert(addr(b));
                g.id_for(&self.port, b)
            },
            |_| ids.next().unwrap_or(0),
        );
        (self.port.post)(Kind::Arrangement, &d);
        g.in_arrangement = used;
        let used_notes: HashSet<usize> = arrangement.notes.iter().map(|n| notes_addr(&n.notes)).collect();
        g.collect_notes(&self.port, &used_notes);
        g.collect(&self.port);
    }

    /// Plays a mono buffer (output rate) immediately; replaces a running preview.
    pub fn preview(&self, data: Arc<[f32]>, gain: f32) {
        self.preview_send(data, gain, 0.0);
    }

    pub fn preview_send(&self, data: Arc<[f32]>, gain: f32, send: f32) {
        self.preview_stereo(data, None, gain, send);
    }

    pub fn preview_stereo(&self, data: Arc<[f32]>, side: Option<Arc<[f32]>>, gain: f32, send: f32) {
        let arr = Arrangement { clips: vec![PlacedClip { start_frame: 0, data, side, gain_l: gain, gain_r: gain, send }], notes: Vec::new() };
        let mut g = self.inner();
        g.stream = None;
        g.in_preview = g.send_clips(&self.port, Kind::Preview, &[], &arr);
        g.collect(&self.port);
    }

    /// Plays a preview that `source` delivers piece by piece (refilled
    /// in [`Engine::tick`]).
    pub fn preview_stream(&self, source: Box<dyn PreviewSource>) {
        let ahead = (STREAM_AHEAD_SECS * self.info.sample_rate as f64) as u64;
        let mut st = PreviewStream::new(source);
        st.fill(0, ahead);
        st.started = (self.port.now)();
        let mut g = self.inner();
        g.in_preview = g.send_clips(&self.port, Kind::Preview, &[], &st.arrangement());
        g.stream = Some(st);
        g.collect(&self.port);
    }

    /// What the engine thread does on the side on the desktop: refill streamed
    /// previews. Called by the web core every few milliseconds.
    pub fn tick(&self) {
        let sr = self.info.sample_rate as f64;
        let ahead = (STREAM_AHEAD_SECS * sr) as u64;
        let mut g = self.inner();
        let Some(st) = g.stream.as_mut() else { return };
        let now = (((self.port.now)() - st.started).max(0.0) * sr) as u64;
        if st.fill(now.saturating_sub((sr * 0.5) as u64), ahead) {
            let arr = st.arrangement();
            let done = st.done;
            g.in_preview = g.send_clips(&self.port, Kind::PreviewMore, &[], &arr);
            if done {
                g.stream = None;
            }
            g.collect(&self.port);
        } else if st.done {
            g.stream = None;
        }
    }

    /// Start recording: from now on the worklet sends the microphone samples
    /// (the web core connects the microphone beforehand).
    pub fn start_recording(&self) -> Result<u32, EngineError> {
        *REC.lock().unwrap_or_else(|e| e.into_inner()) = Some(Vec::with_capacity(self.info.sample_rate as usize * 30));
        SHARED.recording.store(true, Ordering::Relaxed);
        REC_START.store(SHARED.position.load(Ordering::Relaxed), Ordering::Relaxed);
        (self.port.post)(Kind::Record, &[1.0]);
        Ok(self.info.sample_rate)
    }

    /// Stop recording. Beforehand the web core fetches the last samples from the
    /// worklet (on the desktop: draining the ring buffer).
    pub fn stop_recording(&self) -> Result<Recording, EngineError> {
        (self.port.post)(Kind::Record, &[0.0]);
        SHARED.recording.store(false, Ordering::Relaxed);
        let samples = REC.lock().unwrap_or_else(|e| e.into_inner()).take().ok_or_else(|| EngineError(tr!("No recording is running")))?;
        Ok(Recording { samples, sample_rate: self.info.sample_rate, start_frame: REC_START.load(Ordering::Relaxed), device: "Mikrofon".into() })
    }

    /// Reads the status; peak levels are reset in the process.
    pub fn status(&self) -> EngineStatus {
        SHARED.status(self.info.sample_rate)
    }

    /// The status without resetting the peak levels (for work on the side;
    /// the levels belong to the UI).
    pub fn peek_status(&self) -> EngineStatus {
        SHARED.peek(self.info.sample_rate)
    }
}

/// Playback position at the start of the recording.
static REC_START: AtomicU64 = AtomicU64::new(0);
