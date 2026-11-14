//! Real-time audio engine.
//!
//! Structure (the same on desktop and in the browser):
//! * The app talks to an [`Engine`]: playback, arrangement, preview,
//!   recording, status. The methods are the same on both platforms.
//! * Mixing happens in the [`Renderer`] (clips, reverb, metronome, loop). It
//!   receives messages ([`RtMsg`]) through a lock-free ring buffer. In the
//!   mixer **nothing is allocated, nothing is locked and nothing is freed**:
//!   a replaced arrangement goes back through a second ring buffer and
//!   is freed outside.
//! * Jumps during playback (new arrangement, tempo, seeking)
//!   are crossfaded briefly ([`FADE_SECS`]): the old one keeps sounding at its
//!   position and fades out, the new one fades in – no clicks.
//! * Desktop (`device.rs`): an engine thread owns the cpal streams, the
//!   renderer runs in the audio callback.
//! * Browser (`web.rs`): the engine sends the same messages to an
//!   AudioWorklet; there the same renderer runs as a [`Mixer`] (WebAssembly).
//! * Position, level and status are available in atomics for the UI.
//! * Choosing devices: the output and recording device can be switched
//!   while running ([`Engine::set_output_device`]). The engine thread then
//!   rebuilds the output stream and brings the new callback up to the
//!   state of the old one (tempo, metronome, loop, arrangement, position).

mod live;
mod reverb;
pub mod wire;

pub use klang_dsp::live::{LiveNote, LiveNotes, OrganTable, Patch, Plan, Shaping};

#[cfg(not(target_arch = "wasm32"))]
mod device;
#[cfg(not(target_arch = "wasm32"))]
pub use device::Engine;
#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(target_arch = "wasm32")]
pub use web::Engine;

pub use reverb::soft_limit;
use rtrb::{Consumer, Producer, RingBuffer};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct EngineError(pub String);

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

/// A fully rendered clip at a fixed position (output rate): mid
/// (`data`) and optionally side (`side`, stereo width) – left = mid + side,
/// right = mid − side. In mono the side drops out, nothing cancels.
pub struct PlacedClip {
    pub start_frame: u64,
    pub data: Arc<[f32]>,
    pub side: Option<Arc<[f32]>>,
    pub gain_l: f32,
    pub gain_r: f32,
    /// Share that goes into the reverb (room), 0…1.
    pub send: f32,
}

/// A note clip, played live: instrument and notes in beats – nothing is
/// rendered, a new tempo takes effect immediately.
pub struct NoteClip {
    /// Track: its clips sound through one sound shaping chain.
    pub track: u64,
    pub start_beats: f64,
    pub notes: Arc<LiveNotes>,
    pub gain_l: f32,
    pub gain_r: f32,
    /// Share that goes into the reverb (room), 0…1.
    pub send: f32,
}

#[derive(Default)]
pub struct Arrangement {
    /// Rendered clips (audio).
    pub clips: Vec<PlacedClip>,
    /// Note clips (played live).
    pub notes: Vec<NoteClip>,
}

/// What the mixer hands back to be freed outside of it.
pub enum Garbage {
    Arrangement(Arc<Arrangement>),
    /// The clip of a finished voice.
    Notes(Arc<LiveNotes>),
}

/// Supplies a preview piece by piece (e.g. a long score) so that it can
/// run for any length without rendering everything beforehand.
pub trait PreviewSource: Send {
    /// The next piece: the clip (position from the start of the preview) and up
    /// to which frame the preview is covered by it. `None` = finished.
    fn next(&mut self) -> Option<(PlacedClip, u64)>;
}

/// This far ahead (seconds) the engine thread renders a streamed preview.
const STREAM_AHEAD_SECS: f64 = 3.0;

/// A running, streamed preview in the engine thread.
struct PreviewStream {
    source: Box<dyn PreviewSource>,
    clips: Vec<PlacedClip>,
    /// Rendered up to here (frames from the start).
    covered: u64,
    /// Start (seconds on the engine's clock).
    started: f64,
    done: bool,
}

impl PreviewStream {
    fn new(source: Box<dyn PreviewSource>) -> Self {
        Self { source, clips: Vec::new(), covered: 0, started: 0.0, done: false }
    }

    /// Fetches pieces until `ahead` frames beyond `now` are rendered; reports
    /// whether new ones were added. Clips that have been played are dropped.
    fn fill(&mut self, now: u64, ahead: u64) -> bool {
        let mut added = false;
        while !self.done && self.covered < now + ahead {
            match self.source.next() {
                Some((clip, until)) => {
                    self.covered = until.max(self.covered + 1);
                    if !clip.data.is_empty() {
                        self.clips.push(clip);
                    }
                    added = true;
                }
                None => self.done = true,
            }
        }
        if added {
            self.clips.retain(|c| c.start_frame + c.data.len() as u64 > now);
        }
        added
    }

    fn arrangement(&self) -> Arrangement {
        Arrangement {
            clips: self
                .clips
                .iter()
                .map(|c| PlacedClip { start_frame: c.start_frame, data: c.data.clone(), side: c.side.clone(), gain_l: c.gain_l, gain_r: c.gain_r, send: c.send })
                .collect(),
            notes: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Shared {
    playing: AtomicBool,
    /// A preview (library, chords, note) is currently sounding.
    previewing: AtomicBool,
    recording: AtomicBool,
    position: AtomicU64,
    peak_out: AtomicU32,
    peak_in: AtomicU32,
    /// Output muted (everything runs and is measured, nothing is audible) – for tests.
    muted: AtomicBool,
    /// Sample rate of the output device that `position` belongs to (desktop only,
    /// in the browser the web engine knows it itself).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    sample_rate: AtomicU32,
    /// Share of the real-time budget the mixer needs (f32 bits, smoothed).
    load: AtomicU32,
}

impl Shared {
    /// Reads the status; peak levels are reset in the process.
    fn status(&self, sample_rate: u32) -> EngineStatus {
        EngineStatus {
            peak_out: f32::from_bits(self.peak_out.swap(0, Ordering::Relaxed)),
            peak_in: f32::from_bits(self.peak_in.swap(0, Ordering::Relaxed)),
            ..self.peek(sample_rate)
        }
    }

    /// Like [`Shared::status`], but the peak levels stay for the next read.
    fn peek(&self, sample_rate: u32) -> EngineStatus {
        EngineStatus {
            playing: self.playing.load(Ordering::Relaxed),
            previewing: self.previewing.load(Ordering::Relaxed),
            recording: self.recording.load(Ordering::Relaxed),
            position_frames: self.position.load(Ordering::Relaxed),
            sample_rate,
            peak_out: f32::from_bits(self.peak_out.load(Ordering::Relaxed)),
            peak_in: f32::from_bits(self.peak_in.load(Ordering::Relaxed)),
            load: f32::from_bits(self.load.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct EngineInfo {
    /// Output device that is currently playing.
    pub output_device: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Selected output device; `None` = system default.
    pub output_choice: Option<String>,
    /// Selected recording device; `None` = system default.
    pub input_choice: Option<String>,
}

/// Audio devices of the system (names as the operating system calls them).
#[derive(Clone, Debug, Default)]
pub struct AudioDevices {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
    /// Current default devices of the system.
    pub default_output: Option<String>,
    pub default_input: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EngineStatus {
    pub playing: bool,
    pub previewing: bool,
    pub recording: bool,
    pub position_frames: u64,
    pub sample_rate: u32,
    pub peak_out: f32,
    pub peak_in: f32,
    /// Share of the real-time budget the mixer needs (1 = all of it; above
    /// that it can't keep up), smoothed over about 0.3 s.
    pub load: f32,
}

pub struct Recording {
    /// Mono, sample rate of the input device.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Playback position (output frames) at the start of the recording.
    pub start_frame: u64,
    pub device: String,
}

/// Message to the mixer (through the ring buffer or, in the browser, via
/// `postMessage` to the worklet, see [`wire`]).
pub enum RtMsg {
    Play,
    Stop,
    Seek(u64),
    Tempo { bpm: f64, beats_per_bar: u32 },
    Metronome(bool),
    /// Loop [start, end) in beats; None = off.
    Loop(Option<(f64, f64)>),
    /// Without a loop: stop at beat `.0` and jump to beat `.1`; None = play through.
    StopAt(Option<(f64, f64)>),
    /// New arrangement, optionally with a new tempo (BPM, beats per bar) –
    /// both in one step, so that no block sounds with the new tempo but the old
    /// arrangement (or vice versa). During playback it is
    /// crossfaded.
    Arrangement { arrangement: Arc<Arrangement>, tempo: Option<(f64, u32)> },
    /// Play once, independent of the transport (preview).
    Preview(Arc<Arrangement>),
    /// Replace a running preview without restarting from the beginning (streamed).
    PreviewMore(Arc<Arrangement>),
}

fn update_peak(a: &AtomicU32, v: f32) {
    let cur = f32::from_bits(a.load(Ordering::Relaxed));
    if v > cur {
        a.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// Output sample of the mixer. Desktop: all formats cpal delivers (with
/// its conversion); browser: `f32` (Web Audio).
pub trait OutSample: Copy {
    fn from_f32(v: f32) -> Self;
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: cpal::SizedSample + cpal::FromSample<f32>> OutSample for T {
    #[inline]
    fn from_f32(v: f32) -> Self {
        <T as cpal::Sample>::from_sample(v)
    }
}

#[cfg(target_arch = "wasm32")]
impl OutSample for f32 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        v
    }
}

// ------------------------------------------------------------------ Audio callback

const MAX_BLOCK: usize = 1024;

/// Crossfade time for a jump during playback (new arrangement,
/// new tempo, seeking). Linear: if the content stays the same (e.g. only one
/// track changed), the sum stays unchanged.
pub const FADE_SECS: f32 = 0.015;

/// This many transitions may overlap (rapid succession of changes);
/// at most one begins per audio block.
const MAX_FADES: usize = 8;

/// A fading state: arrangement and position keep running, the level
/// falls linearly to 0.
struct Fade {
    arrangement: Arc<Arrangement>,
    pos: u64,
    /// Level at the next sample.
    gain: f32,
    /// Context of its live voices (they fade out with it).
    ctx: u32,
}

struct Renderer {
    sr: f32,
    channels: usize,
    rx: Consumer<RtMsg>,
    garbage: Producer<Garbage>,
    /// Live voices of the note clips.
    live: live::Live,
    /// Context of the current state (see `live`); a new one after every jump.
    ctx: u32,
    /// Pick up sounding notes before the next block (start, jump).
    chase: bool,
    /// Without reverb: one part of a parallel export (the reverb runs on the sum).
    dry: bool,
    shared: Arc<Shared>,
    playing: bool,
    pos: u64,
    bpm: f64,
    beats_per_bar: u32,
    metronome: bool,
    next_click: f64,
    click_pos: usize,
    click_len: usize,
    click_freq: f32,
    click_gain: f32,
    arrangement: Arc<Arrangement>,
    preview: Arc<Arrangement>,
    preview_pos: u64,
    /// End of the preview (frames from its start).
    preview_len: u64,
    /// Smoothed share of the real-time budget (see `note_time`).
    load: f32,
    /// Loop in beats.
    loop_beats: Option<(f64, f64)>,
    /// Stop (beat) and return jump (beat) when no loop is running.
    stop_at: Option<(f64, f64)>,
    mix_l: [f32; MAX_BLOCK],
    mix_r: [f32; MAX_BLOCK],
    /// Sum of the reverb sends in the block.
    mix_send: [f32; MAX_BLOCK],
    reverb: reverb::Reverb,
    /// Fading states after jumps (see [`FADE_SECS`]).
    fades: [Option<Fade>; MAX_FADES],
    /// Level decrease per sample while fading out.
    fade_step: f32,
    /// Intermediate buffer for mixing a state with its own level curve.
    tmp_l: [f32; MAX_BLOCK],
    tmp_r: [f32; MAX_BLOCK],
    tmp_send: [f32; MAX_BLOCK],
    /// Sum of the levels of all fading states per sample.
    fade_sum: [f32; MAX_BLOCK],
}

impl Renderer {
    fn new(sr: f32, channels: usize, rx: Consumer<RtMsg>, garbage: Producer<Garbage>, shared: Arc<Shared>) -> Self {
        let fade_step = 1.0 / (FADE_SECS * sr).max(1.0);
        Self {
            sr,
            channels: channels.max(1),
            rx,
            garbage,
            live: live::Live::new(sr, fade_step),
            ctx: 0,
            chase: false,
            dry: false,
            shared,
            playing: false,
            pos: 0,
            bpm: 120.0,
            beats_per_bar: 4,
            metronome: false,
            next_click: 0.0,
            click_pos: usize::MAX,
            click_len: (0.03 * sr) as usize,
            click_freq: 1000.0,
            click_gain: 0.0,
            arrangement: Arc::new(Arrangement::default()),
            preview: Arc::new(Arrangement::default()),
            preview_pos: 0,
            preview_len: 0,
            load: 0.0,
            loop_beats: None,
            stop_at: None,
            mix_l: [0.0; MAX_BLOCK],
            mix_r: [0.0; MAX_BLOCK],
            mix_send: [0.0; MAX_BLOCK],
            reverb: reverb::Reverb::new(sr),
            fades: std::array::from_fn(|_| None),
            fade_step: 1.0 / (FADE_SECS * sr).max(1.0),
            tmp_l: [0.0; MAX_BLOCK],
            tmp_r: [0.0; MAX_BLOCK],
            tmp_send: [0.0; MAX_BLOCK],
            fade_sum: [0.0; MAX_BLOCK],
        }
    }

    /// Level of the current state: what the fading ones leave over.
    fn main_gain(&self) -> f32 {
        1.0 - self.fades.iter().flatten().map(|f| f.gain).sum::<f32>()
    }

    /// Before a jump during playback: the previous state
    /// (arrangement, position) keeps sounding in place and fades out;
    /// the new one starts at level 0. Afterwards `arrangement`, `pos`
    /// and `bpm` may change. Without playback – or if the current state has only
    /// just begun (level 0) – there is nothing to fade out.
    /// Allocates nothing: the arrangement is only shared (`Arc`). Returns
    /// whether a fading state was created.
    fn begin_fade(&mut self) -> bool {
        if !self.playing || self.main_gain() <= 1e-4 {
            return false;
        }
        let slot = match self.fades.iter().position(Option::is_none) {
            Some(i) => i,
            // all occupied: replace the quietest (almost faded out) one
            None => {
                let i = (0..MAX_FADES)
                    .min_by(|&a, &b| {
                        let g = |i: usize| self.fades[i].as_ref().map_or(0.0, |f| f.gain);
                        g(a).total_cmp(&g(b))
                    })
                    .unwrap_or(0);
                if let Some(old) = self.fades[i].take() {
                    self.live.kill(old.ctx, &mut self.garbage);
                    let _ = self.garbage.push(Garbage::Arrangement(old.arrangement));
                }
                i
            }
        };
        // (after the replacement including its share – the sum stays 1)
        let gain = self.main_gain();
        self.fades[slot] = Some(Fade { arrangement: self.arrangement.clone(), pos: self.pos, gain, ctx: self.ctx });
        true
    }

    /// A jump during playback (new arrangement, tempo, position): the
    /// current state fades out with its voices, a new one begins and picks
    /// up the notes sounding at its position.
    fn transition(&mut self) {
        if !self.playing {
            return;
        }
        if !self.begin_fade() {
            // the current state had only just begun: replaced without a trace
            self.live.kill(self.ctx, &mut self.garbage);
        }
        self.ctx = self.ctx.wrapping_add(1) % live::RELEASED;
        self.chase = true;
    }

    /// End fading states immediately (stop); their voices end, those of the
    /// current state fade out briefly.
    fn drop_fades(&mut self) {
        let level = self.main_gain();
        for slot in self.fades.iter_mut() {
            if let Some(f) = slot.take() {
                let _ = self.garbage.push(Garbage::Arrangement(f.arrangement));
            }
        }
        self.live.kill_others(self.ctx, &mut self.garbage);
        self.live.release(self.ctx, level);
    }

    /// New tempo; during playback the position in beats is
    /// preserved (the jump in the signal is crossfaded).
    fn set_tempo(&mut self, bpm: f64, beats_per_bar: u32) {
        if bpm > 0.0 && (bpm - self.bpm).abs() > 1e-9 {
            self.transition();
            self.pos = (self.pos as f64 * self.bpm / bpm).round() as u64;
            self.bpm = bpm;
        }
        self.beats_per_bar = beats_per_bar.max(1);
        self.resync_click();
    }

    fn samples_per_beat(&self) -> f64 {
        self.sr as f64 * 60.0 / self.bpm
    }

    fn resync_click(&mut self) {
        let spb = self.samples_per_beat();
        self.next_click = (self.pos as f64 / spb).ceil() * spb;
    }

    /// Loop in frames at the current tempo.
    fn loop_frames(&self) -> Option<(u64, u64)> {
        let (a, b) = self.loop_beats?;
        let spb = self.samples_per_beat();
        let (s, e) = ((a.max(0.0) * spb).round() as u64, (b * spb).round() as u64);
        (e > s + 64).then_some((s, e))
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.pop() {
            match msg {
                RtMsg::Play => {
                    if !self.playing {
                        self.chase = true;
                    }
                    self.playing = true;
                    self.resync_click();
                }
                RtMsg::Stop => {
                    self.drop_fades();
                    self.playing = false;
                }
                RtMsg::Seek(f) => {
                    if f != self.pos {
                        self.transition();
                    }
                    self.pos = f;
                    self.resync_click();
                }
                RtMsg::Tempo { bpm, beats_per_bar } => self.set_tempo(bpm, beats_per_bar),
                RtMsg::Metronome(on) => {
                    self.metronome = on;
                    self.resync_click();
                }
                RtMsg::Loop(region) => self.loop_beats = region,
                RtMsg::StopAt(at) => self.stop_at = at,
                RtMsg::Arrangement { arrangement, tempo } => {
                    // one transition for both (set_tempo does not start a second one afterwards)
                    self.transition();
                    if let Some((bpm, beats_per_bar)) = tempo {
                        self.set_tempo(bpm, beats_per_bar);
                    }
                    let old = std::mem::replace(&mut self.arrangement, arrangement);
                    // Freed outside the callback; only if the return channel is full, here.
                    let _ = self.garbage.push(Garbage::Arrangement(old));
                }
                RtMsg::Preview(p) => {
                    self.preview_len = p.clips.iter().map(|c| c.start_frame + c.data.len() as u64).max().unwrap_or(0);
                    let old = std::mem::replace(&mut self.preview, p);
                    self.preview_pos = 0;
                    let _ = self.garbage.push(Garbage::Arrangement(old));
                }
                RtMsg::PreviewMore(p) => {
                    self.preview_len = p.clips.iter().map(|c| c.start_frame + c.data.len() as u64).max().unwrap_or(0);
                    let old = std::mem::replace(&mut self.preview, p);
                    let _ = self.garbage.push(Garbage::Arrangement(old));
                }
            }
        }
    }

    /// Mixes all clips of `arr` that sound in [pos, pos+n) into the buffers.
    fn mix_clips(arr: &Arrangement, pos: u64, n: usize, mix_l: &mut [f32], mix_r: &mut [f32], send: &mut [f32]) {
        let block_end = pos + n as u64;
        for clip in arr.clips.iter() {
            let cs = clip.start_frame;
            let ce = cs + clip.data.len() as u64;
            if ce <= pos || cs >= block_end {
                continue;
            }
            // reverb share with the track's average volume
            let to_send = clip.send * 0.5 * (clip.gain_l + clip.gain_r);
            for f in pos.max(cs)..block_end.min(ce) {
                let j = (f - cs) as usize;
                let s = clip.data[j];
                let d = clip.side.as_ref().and_then(|x| x.get(j)).copied().unwrap_or(0.0);
                let i = (f - pos) as usize;
                mix_l[i] += (s + d) * clip.gain_l;
                mix_r[i] += (s - d) * clip.gain_r;
                send[i] += s * to_send;
            }
        }
    }

    /// Mixes a state into the intermediate buffers and from there with level `gain(i)`
    /// into the sum.
    fn mix_weighted(
        arr: &Arrangement,
        pos: u64,
        n: usize,
        tmp: (&mut [f32], &mut [f32], &mut [f32]),
        mix: (&mut [f32], &mut [f32], &mut [f32]),
        gain: impl Fn(usize) -> f32,
    ) {
        let (tl, tr, ts) = tmp;
        let (ml, mr, ms) = mix;
        tl[..n].fill(0.0);
        tr[..n].fill(0.0);
        ts[..n].fill(0.0);
        Self::mix_clips(arr, pos, n, &mut tl[..n], &mut tr[..n], &mut ts[..n]);
        for i in 0..n {
            let g = gain(i);
            ml[i] += tl[i] * g;
            mr[i] += tr[i] * g;
            ms[i] += ts[i] * g;
        }
    }

    /// Playback during a transition: fading states with falling
    /// level, the current one with the remainder up to 1.
    fn mix_fading(&mut self, n: usize) {
        let step = self.fade_step;
        self.fade_sum[..n].fill(0.0);
        for slot in self.fades.iter_mut() {
            let Some(f) = slot else { continue };
            let g0 = f.gain;
            Self::mix_weighted(
                &f.arrangement,
                f.pos,
                n,
                (&mut self.tmp_l, &mut self.tmp_r, &mut self.tmp_send),
                (&mut self.mix_l, &mut self.mix_r, &mut self.mix_send),
                |i| (g0 - i as f32 * step).max(0.0),
            );
            for (i, s) in self.fade_sum[..n].iter_mut().enumerate() {
                *s += (g0 - i as f32 * step).max(0.0);
            }
            f.gain = (g0 - n as f32 * step).max(0.0);
            f.pos += n as u64;
            if f.gain <= 0.0 {
                // faded out: freed outside the callback (its voices end in the next block)
                if let Some(done) = slot.take() {
                    let _ = self.garbage.push(Garbage::Arrangement(done.arrangement));
                }
            }
        }
        let sum = &self.fade_sum;
        Self::mix_weighted(
            &self.arrangement,
            self.pos,
            n,
            (&mut self.tmp_l, &mut self.tmp_r, &mut self.tmp_send),
            (&mut self.mix_l, &mut self.mix_r, &mut self.mix_send),
            |i| (1.0 - sum[i]).max(0.0),
        );
    }

    fn render_block(&mut self, n: usize) {
        self.mix_l[..n].fill(0.0);
        self.mix_r[..n].fill(0.0);
        self.mix_send[..n].fill(0.0);
        let block_start = self.pos;
        // fading states at the start of the block: their voices fade along
        let mut fading = [(0u32, 0f32); MAX_FADES];
        let mut n_fading = 0;
        for f in self.fades.iter().flatten() {
            fading[n_fading] = (f.ctx, f.gain);
            n_fading += 1;
        }
        if self.playing {
            let spf = self.samples_per_beat();
            if self.chase {
                self.chase = false;
                self.live.chase(&self.arrangement.notes, block_start, spf, self.ctx, &mut self.garbage);
            }
            self.live.schedule(&self.arrangement.notes, block_start, n, spf, self.ctx, &mut self.garbage);
            if n_fading > 0 {
                self.mix_fading(n);
            } else {
                Self::mix_clips(&self.arrangement, block_start, n, &mut self.mix_l[..n], &mut self.mix_r[..n], &mut self.mix_send[..n]);
            }
        }
        let levels = live::Levels {
            current: self.ctx,
            fading: &fading[..n_fading],
            step: self.fade_step,
            fade_sum: (self.playing && n_fading > 0).then_some(&self.fade_sum[..n]),
        };
        self.live.render(n, &levels, (&mut self.mix_l[..n], &mut self.mix_r[..n], &mut self.mix_send[..n]), &mut self.garbage);
        if !self.preview.clips.is_empty() {
            Self::mix_clips(&self.preview, self.preview_pos, n, &mut self.mix_l[..n], &mut self.mix_r[..n], &mut self.mix_send[..n]);
            self.preview_pos += n as u64;
        }
        // room: the reverb keeps ringing out even after stopping
        for i in (0..n).filter(|_| !self.dry) {
            let (l, r) = self.reverb.process(self.mix_send[i]);
            self.mix_l[i] += l;
            self.mix_r[i] += r;
        }
        let spb = self.samples_per_beat();
        for i in 0..n {
            if self.playing && (block_start + i as u64) as f64 >= self.next_click {
                if self.metronome {
                    let beat = (self.next_click / spb).round() as u64;
                    let accent = beat % self.beats_per_bar as u64 == 0;
                    self.click_freq = if accent { 1760.0 } else { 1175.0 };
                    self.click_gain = if accent { 0.45 } else { 0.28 };
                    self.click_pos = 0;
                }
                self.next_click += spb;
            }
            if self.click_pos < self.click_len {
                let t = self.click_pos as f32 / self.sr;
                let env = 1.0 - self.click_pos as f32 / self.click_len as f32;
                let s = (2.0 * std::f32::consts::PI * self.click_freq * t).sin() * env * env * env * self.click_gain;
                self.mix_l[i] += s;
                self.mix_r[i] += s;
                self.click_pos += 1;
            }
        }
        if self.playing {
            self.pos = block_start + n as u64;
        }
    }

    /// The last callback took `secs` for `frames` frames: the share of the
    /// real-time budget, smoothed over about 0.3 s, goes into the status.
    fn note_time(&mut self, secs: f32, frames: usize) {
        if frames == 0 {
            return;
        }
        let dt = frames as f32 / self.sr;
        let a = 1.0 - (-dt / 0.3).exp();
        self.load += (secs / dt - self.load) * a;
        self.shared.load.store(self.load.to_bits(), Ordering::Relaxed);
    }

    fn process<T: OutSample>(&mut self, out: &mut [T]) {
        self.handle_messages();
        let ch = self.channels;
        let frames = out.len() / ch;
        let mut done = 0;
        let mut peak = 0f32;
        let muted = self.shared.muted.load(Ordering::Relaxed);
        while done < frames {
            let mut n = (frames - done).min(MAX_BLOCK);
            // loop: split the block at the loop end and jump to the start there
            let lp = if self.playing { self.loop_frames() } else { None };
            if let Some((_, e)) = lp {
                if self.pos < e {
                    n = n.min((e - self.pos) as usize);
                }
            }
            // without a loop: stop at the stop point (end of the range or one bar after the piece)
            let spb = self.samples_per_beat();
            let halt = if self.playing && lp.is_none() {
                self.stop_at.map(|(s, b)| ((s * spb).round() as u64, (b.max(0.0) * spb).round() as u64))
            } else {
                None
            };
            if let Some((s, _)) = halt {
                if self.pos < s {
                    n = n.min(((s - self.pos) as usize).max(1));
                }
            }
            let before = self.pos;
            self.render_block(n);
            if let Some((s, e)) = lp {
                if before < e && self.pos >= e {
                    self.pos = s;
                    self.resync_click();
                    // the voices from before the jump fade out, the loop start picks up its notes
                    self.live.release(self.ctx, 1.0);
                    self.chase = true;
                }
            }
            if let Some((s, back)) = halt {
                if before < s && self.pos >= s {
                    self.drop_fades();
                    self.playing = false;
                    self.pos = back;
                    self.resync_click();
                }
            }
            for i in 0..n {
                let l = soft_limit(self.mix_l[i]);
                let r = soft_limit(self.mix_r[i]);
                peak = peak.max(l.abs()).max(r.abs());
                let (l, r) = if muted { (0.0, 0.0) } else { (l, r) };
                let base = (done + i) * ch;
                if ch == 1 {
                    out[base] = T::from_f32((l + r) * 0.5);
                } else {
                    out[base] = T::from_f32(l);
                    out[base + 1] = T::from_f32(r);
                    for c in 2..ch {
                        out[base + c] = T::from_f32(0.0f32);
                    }
                }
            }
            done += n;
        }
        self.shared.position.store(self.pos, Ordering::Relaxed);
        self.shared.playing.store(self.playing, Ordering::Relaxed);
        self.shared.previewing.store(self.preview_pos < self.preview_len, Ordering::Relaxed);
        update_peak(&self.shared.peak_out, peak);
    }
}

/// Mix offline (export): exactly like playback – clips with level and
/// pan, side signal, reverb bus, soft limiter –, but without a device and
/// without metronome. Returns `frames` frames left/right at `sample_rate`.
pub fn render_offline(arrangement: Arrangement, sample_rate: u32, frames: u64) -> (Vec<f32>, Vec<f32>) {
    // the tracks are independent up to the reverb: on the desktop each core takes some
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
        if threads > 1 && arrangement.notes.len() > 1 {
            return render_parallel(arrangement, sample_rate, frames, threads);
        }
    }
    render_serial(arrangement, sample_rate, frames)
}

/// [`render_offline`] in one renderer.
fn render_serial(arrangement: Arrangement, sample_rate: u32, frames: u64) -> (Vec<f32>, Vec<f32>) {
    let (mut r, mut grx) = offline_renderer(arrangement, sample_rate, false);
    let mut left = Vec::with_capacity(frames as usize);
    let mut right = Vec::with_capacity(frames as usize);
    let mut done = 0u64;
    while done < frames {
        let n = ((frames - done) as usize).min(MAX_BLOCK);
        r.render_block(n);
        for i in 0..n {
            left.push(soft_limit(r.mix_l[i]));
            right.push(soft_limit(r.mix_r[i]));
        }
        done += n as u64;
        while let Ok(g) = grx.pop() {
            drop(g);
        }
    }
    (left, right)
}

/// Renders a note clip step by step – e.g. in the browser between other
/// work – into mid and side, like a rendered clip: the same voices and sound
/// shaping as during playback (for clips that the live mixer can't keep up with).
pub struct StepRender {
    r: Renderer,
    garbage: Consumer<Garbage>,
    frames: usize,
    mid: Vec<f32>,
    side: Vec<f32>,
}

impl StepRender {
    /// `clip` from its start at `bpm` (its position, level and reverb do not
    /// matter: they belong to the clip that plays the result).
    pub fn new(clip: NoteClip, bpm: f64, sample_rate: u32) -> Self {
        let clip = NoteClip { start_beats: 0.0, gain_l: 1.0, gain_r: 1.0, send: 0.0, ..clip };
        let frames = clip.notes.patch.buffer_len((clip.notes.len_beats * 60.0 / bpm) as f32, sample_rate as f32);
        let (mut r, garbage) = offline_renderer(Arrangement { clips: Vec::new(), notes: vec![clip] }, sample_rate, true);
        r.bpm = bpm;
        Self { r, garbage, frames, mid: Vec::with_capacity(frames), side: Vec::with_capacity(frames) }
    }

    /// Renders up to `max` more frames; `true` once the clip is complete.
    pub fn step(&mut self, max: usize) -> bool {
        let mut left = (self.frames - self.mid.len()).min(max);
        while left > 0 {
            let n = left.min(MAX_BLOCK);
            self.r.render_block(n);
            for i in 0..n {
                let (l, r) = (self.r.mix_l[i], self.r.mix_r[i]);
                self.mid.push((l + r) * 0.5);
                self.side.push((l - r) * 0.5);
            }
            while let Ok(g) = self.garbage.pop() {
                drop(g);
            }
            left -= n;
        }
        self.mid.len() >= self.frames
    }

    /// The rendered sound: mid and – if the clip is wide – side.
    pub fn finish(self) -> (Arc<[f32]>, Option<Arc<[f32]>>) {
        let wide = self.side.iter().any(|v| v.abs() > 1e-9);
        (self.mid.into(), wide.then(|| self.side.into()))
    }
}

/// A renderer that plays `arrangement` from the start, without a device.
fn offline_renderer(arrangement: Arrangement, sample_rate: u32, dry: bool) -> (Renderer, Consumer<Garbage>) {
    let (_tx, rx) = RingBuffer::new(1);
    let (gtx, grx) = RingBuffer::new(GARBAGE);
    let mut r = Renderer::new(sample_rate as f32, 2, rx, gtx, Arc::new(Shared::default()));
    r.arrangement = Arc::new(arrangement);
    r.playing = true;
    r.chase = true;
    r.dry = dry;
    (r, grx)
}

/// [`render_offline`] on several cores: the tracks are spread over `threads`
/// renderers without reverb, which render one second at a time side by
/// side; the sum then goes through the reverb and the limiter – the same as
/// in one renderer (a track's clips stay together, they share one chain).
#[cfg(not(target_arch = "wasm32"))]
fn render_parallel(arrangement: Arrangement, sample_rate: u32, frames: u64, threads: usize) -> (Vec<f32>, Vec<f32>) {
    let Arrangement { clips, notes } = arrangement;
    let mut tracks: Vec<u64> = notes.iter().map(|n| n.track).collect();
    tracks.sort_unstable();
    tracks.dedup();
    let groups = threads.min(tracks.len()).max(1);
    let mut parts: Vec<Arrangement> = (0..groups).map(|_| Arrangement::default()).collect();
    parts[0].clips = clips;
    for n in notes {
        let g = tracks.binary_search(&n.track).unwrap_or(0) % groups;
        parts[g].notes.push(n);
    }
    struct Part {
        r: Renderer,
        garbage: Consumer<Garbage>,
        out: [Vec<f32>; 3],
    }
    let mut parts: Vec<Part> = parts
        .into_iter()
        .map(|a| {
            let (r, garbage) = offline_renderer(a, sample_rate, true);
            Part { r, garbage, out: [Vec::new(), Vec::new(), Vec::new()] }
        })
        .collect();
    let chunk = sample_rate as usize;
    let mut reverb = reverb::Reverb::new(sample_rate as f32);
    let mut left = Vec::with_capacity(frames as usize);
    let mut right = Vec::with_capacity(frames as usize);
    let mut done = 0u64;
    while done < frames {
        let n = ((frames - done) as usize).min(chunk);
        std::thread::scope(|s| {
            for p in parts.iter_mut() {
                s.spawn(move || {
                    for o in p.out.iter_mut() {
                        o.clear();
                    }
                    let mut k = 0;
                    while k < n {
                        let m = (n - k).min(MAX_BLOCK);
                        p.r.render_block(m);
                        p.out[0].extend_from_slice(&p.r.mix_l[..m]);
                        p.out[1].extend_from_slice(&p.r.mix_r[..m]);
                        p.out[2].extend_from_slice(&p.r.mix_send[..m]);
                        while let Ok(g) = p.garbage.pop() {
                            drop(g);
                        }
                        k += m;
                    }
                });
            }
        });
        for i in 0..n {
            let (mut l, mut r, mut send) = (0f32, 0f32, 0f32);
            for p in &parts {
                l += p.out[0][i];
                r += p.out[1][i];
                send += p.out[2][i];
            }
            let (rl, rr) = reverb.process(send);
            left.push(soft_limit(l + rl));
            right.push(soft_limit(r + rr));
        }
        done += n as u64;
    }
    (left, right)
}

/// Capacity of the channel through which the mixer hands back what is to be freed.
pub(crate) const GARBAGE: usize = 1024;

/// The engine's mixer without a device of its own: for an output that itself
/// requests blocks – in the browser the AudioWorklet. The same [`Renderer`] as
/// in the desktop's audio callback, with the same messages ([`RtMsg`]).
pub struct Mixer {
    renderer: Renderer,
    tx: Producer<RtMsg>,
    garbage: Consumer<Garbage>,
}

impl Mixer {
    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let (tx, rx) = RingBuffer::<RtMsg>::new(512);
        let (gtx, garbage) = RingBuffer::<Garbage>::new(GARBAGE);
        Self { renderer: Renderer::new(sample_rate, channels, rx, gtx, Arc::new(Shared::default())), tx, garbage }
    }

    /// Queues a message; it takes effect from the next block.
    pub fn send(&mut self, msg: RtMsg) {
        let mut m = msg;
        while let Err(rtrb::PushError::Full(back)) = self.tx.push(m) {
            m = back;
            self.renderer.handle_messages();
            self.collect();
        }
    }

    /// Mutes the output (status and level keep running).
    pub fn set_muted(&mut self, on: bool) {
        self.renderer.shared.muted.store(on, Ordering::Relaxed);
    }

    /// Fills `out` (interleaved, `channels` channels per frame).
    pub fn process(&mut self, out: &mut [f32]) {
        self.renderer.process(out);
        self.collect();
    }

    /// Frees replaced arrangements (outside of mixing).
    fn collect(&mut self) {
        while let Ok(old) = self.garbage.pop() {
            drop(old);
        }
    }

    /// Status like [`Engine::status`]; peak levels are reset in the process.
    pub fn status(&self) -> EngineStatus {
        self.renderer.shared.status(self.renderer.sr as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_plays_like_the_device_renderer() {
        let mut m = Mixer::new(48_000.0, 2);
        let data: Arc<[f32]> = vec![0.5f32; 100].into();
        m.send(arrangement_msg(Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 10, data, side: None, gain_l: 1.0, gain_r: 0.5, send: 0.0 }] }));
        m.send(RtMsg::Play);
        let mut out = vec![0f32; 2 * 64];
        m.process(&mut out);
        assert_eq!(out[2 * 9], 0.0);
        assert!((out[2 * 10] - soft_limit(0.5)).abs() < 1e-6);
        let s = m.status();
        assert!(s.playing);
        assert_eq!(s.position_frames, 64);
        assert!(s.peak_out > 0.4);
        assert_eq!(m.status().peak_out, 0.0, "Spitzenpegel beim Lesen zurückgesetzt");
    }

    fn renderer() -> (Renderer, Producer<RtMsg>, Consumer<Garbage>) {
        let (tx, rx) = RingBuffer::new(64);
        let (gtx, grx) = RingBuffer::new(GARBAGE);
        (Renderer::new(48_000.0, 2, rx, gtx, Arc::new(Shared::default())), tx, grx)
    }

    fn arrangement_msg(arr: Arrangement) -> RtMsg {
        RtMsg::Arrangement { arrangement: Arc::new(arr), tempo: None }
    }

    // ------------------------------------------------ Crossfading at jumps

    /// Counts allocations and frees per thread (tests run in parallel).
    mod counting {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        pub struct Counting;

        thread_local! {
            static OPS: Cell<usize> = const { Cell::new(0) };
        }

        fn bump() {
            let _ = OPS.try_with(|c| c.set(c.get() + 1));
        }

        /// Allocations + frees of this thread so far.
        pub fn ops() -> usize {
            OPS.with(Cell::get)
        }

        unsafe impl GlobalAlloc for Counting {
            unsafe fn alloc(&self, l: Layout) -> *mut u8 {
                bump();
                unsafe { System.alloc(l) }
            }
            unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
                bump();
                unsafe { System.alloc_zeroed(l) }
            }
            unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
                bump();
                unsafe { System.dealloc(p, l) }
            }
            unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
                bump();
                unsafe { System.realloc(p, l, n) }
            }
        }
    }

    #[global_allocator]
    static ALLOC: counting::Counting = counting::Counting;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 128;

    /// A sine (frequency, phase, amplitude) as a clip from frame 0.
    fn sine(len: usize, freq: f32, phase: f32, amp: f32) -> Arrangement {
        let data: Arc<[f32]> = (0..len).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR + phase).sin()).collect::<Vec<f32>>().into();
        Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 0, data, side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 }] }
    }

    /// Value of the sine at frame `f` (as in [`sine`]).
    fn sine_at(f: u64, freq: f32, phase: f32, amp: f32) -> f32 {
        amp * (std::f32::consts::TAU * freq * f as f32 / SR + phase).sin()
    }

    /// Plays `blocks` blocks and appends the left channel.
    fn play(r: &mut Renderer, blocks: usize, left: &mut Vec<f32>) {
        let mut out = [0f32; 2 * BLOCK];
        for _ in 0..blocks {
            r.process(&mut out);
            left.extend(out.chunks(2).map(|f| f[0]));
        }
    }

    /// Largest difference between consecutive samples.
    fn max_step(x: &[f32]) -> f32 {
        x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    /// Steepest rise of a sine at 220 Hz and amplitude 0.5 per sample
    /// (≈ 0.0144) – the signal may change this much from sample to sample anyway.
    fn sine_slope() -> f32 {
        0.5 * std::f32::consts::TAU * 220.0 / SR
    }

    #[test]
    fn arrangement_change_while_playing_crossfades_without_a_jump() {
        let (mut r, mut tx, mut grx) = renderer();
        let _ = tx.push(arrangement_msg(sine(96_000, 220.0, 0.0, 0.5)));
        let _ = tx.push(RtMsg::Play);
        let mut left = Vec::new();
        play(&mut r, 10, &mut left);
        let s = r.pos;
        // same note, phase rotated: switched hard, that would be a jump
        let hard = (sine_at(s, 220.0, 0.0, 0.5) - sine_at(s, 220.0, std::f32::consts::PI, 0.5)).abs();
        assert!(hard > 0.5, "Testaufbau: harter Wechsel springt um {hard}");
        while grx.pop().is_ok() {}
        let _ = tx.push(arrangement_msg(sine(96_000, 220.0, std::f32::consts::PI, 0.5)));
        play(&mut r, 20, &mut left);
        let fade_len = (FADE_SECS * SR) as f32;
        let bound = sine_slope() + 1.0 / fade_len + 1e-3;
        assert!(max_step(&left) < bound, "größter Schritt {} (erlaubt {bound}, hart wären {hard})", max_step(&left));
        // after the crossfade: only the new arrangement, the old one back for freeing
        assert!(r.fades.iter().all(Option::is_none));
        let last = *left.last().unwrap();
        assert!((last - sine_at(r.pos - 1, 220.0, std::f32::consts::PI, 0.5)).abs() < 1e-5);
        assert_eq!(std::iter::from_fn(|| grx.pop().ok()).count(), 2, "ersetztes Arrangement (und sein Übergang) zurück");
    }

    #[test]
    fn arrangement_with_tempo_keeps_the_beat_position_without_a_jump() {
        let (mut r, mut tx, _g) = renderer();
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(arrangement_msg(sine(4 * 48_000, 220.0, 0.0, 0.5)));
        let _ = tx.push(RtMsg::Play);
        let mut left = Vec::new();
        play(&mut r, 100, &mut left);
        assert_eq!(r.pos, 12_800);
        // faster: the same spot in beats lies earlier – the read head jumps
        let new_pos = 12_800 * 120 / 150;
        let hard = (sine_at(12_800, 220.0, 0.0, 0.5) - sine_at(new_pos, 220.0, 0.0, 0.5)).abs();
        assert!(hard > 0.1, "Testaufbau: harter Wechsel springt um {hard}");
        let _ = tx.push(RtMsg::Arrangement { arrangement: Arc::new(sine(4 * 48_000, 220.0, 0.0, 0.5)), tempo: Some((150.0, 3)) });
        play(&mut r, 20, &mut left);
        assert_eq!(r.pos, new_pos + 20 * BLOCK as u64, "Position in Schlägen erhalten");
        assert_eq!((r.bpm, r.beats_per_bar), (150.0, 3));
        let bound = sine_slope() + 1.0 / (FADE_SECS * SR) + 1e-3;
        assert!(max_step(&left) < bound, "größter Schritt {} (erlaubt {bound}, hart wären {hard})", max_step(&left));
    }

    #[test]
    fn tempo_seek_and_quick_changes_overlap_without_jumps() {
        let (mut r, mut tx, mut grx) = renderer();
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(arrangement_msg(sine(4 * 48_000, 220.0, 0.0, 0.5)));
        let _ = tx.push(RtMsg::Play);
        let mut left = Vec::new();
        play(&mut r, 50, &mut left);
        // tempo alone (old engine sequence: tempo, the arrangement one block later)
        let _ = tx.push(RtMsg::Tempo { bpm: 133.0, beats_per_bar: 4 });
        play(&mut r, 1, &mut left);
        let _ = tx.push(arrangement_msg(sine(4 * 48_000, 220.0, 1.0, 0.5)));
        play(&mut r, 10, &mut left);
        // seeking during playback
        let _ = tx.push(RtMsg::Seek(70_001));
        play(&mut r, 10, &mut left);
        // lots of changes, one per block (more than can be crossfaded)
        for k in 0..40 {
            while grx.pop().is_ok() {}
            let msg = if k % 3 == 0 {
                RtMsg::Arrangement { arrangement: Arc::new(sine(4 * 48_000, 220.0, k as f32, 0.5)), tempo: Some((100.0 + k as f64, 4)) }
            } else {
                arrangement_msg(sine(4 * 48_000, 220.0, k as f32 * 2.0, 0.5))
            };
            let _ = tx.push(msg);
            play(&mut r, 1, &mut left);
        }
        play(&mut r, 20, &mut left);
        // Each running crossfade adds at most its slope (Δ/length).
        let fades_at_once = ((FADE_SECS * SR) / BLOCK as f32).ceil();
        let bound = sine_slope() + fades_at_once / (FADE_SECS * SR) + 1e-3;
        assert!(max_step(&left) < bound, "größter Schritt {} (erlaubt {bound})", max_step(&left));
        assert!(r.fades.iter().all(Option::is_none), "alles ausgeklungen");
    }

    #[test]
    fn stop_ends_the_crossfade_and_returns_its_arrangement() {
        let (mut r, mut tx, mut grx) = renderer();
        let _ = tx.push(arrangement_msg(sine(48_000, 220.0, 0.0, 0.5)));
        let _ = tx.push(RtMsg::Play);
        let mut left = Vec::new();
        play(&mut r, 4, &mut left);
        let _ = tx.push(arrangement_msg(sine(48_000, 220.0, 2.0, 0.5)));
        play(&mut r, 1, &mut left);
        assert!(r.fades.iter().any(Option::is_some), "Übergang läuft");
        while grx.pop().is_ok() {}
        let _ = tx.push(RtMsg::Stop);
        play(&mut r, 1, &mut left);
        assert!(r.fades.iter().all(Option::is_none));
        assert_eq!(std::iter::from_fn(|| grx.pop().ok()).count(), 1, "ausklingendes Arrangement zur Freigabe zurück");
        // stopped: a new arrangement is adopted without a transition
        let _ = tx.push(arrangement_msg(sine(48_000, 220.0, 1.0, 0.5)));
        play(&mut r, 1, &mut left);
        assert!(r.fades.iter().all(Option::is_none));
    }

    #[test]
    fn crossfades_neither_allocate_nor_free_in_the_mixer() {
        let (tx_rt, rx) = RingBuffer::new(64);
        let (gtx, mut grx) = RingBuffer::new(64);
        let mut r = Renderer::new(SR, 2, rx, gtx, Arc::new(Shared::default()));
        let mut tx = tx_rt;
        // Arrangements only in the messages (no copy here): if the
        // mixer freed one, that would count as a free.
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(arrangement_msg(sine(4 * 48_000, 220.0, 0.0, 0.5)));
        let _ = tx.push(RtMsg::Play);
        let mut out = [0f32; 2 * BLOCK];
        let mut ops = 0;
        let mut returned = 0;
        for k in 0..120 {
            // messages are built outside (as in the app)
            match k % 9 {
                0 => drop(tx.push(arrangement_msg(sine(4 * 48_000, 220.0, k as f32, 0.5)))),
                3 => drop(tx.push(RtMsg::Arrangement { arrangement: Arc::new(sine(4 * 48_000, 220.0, 1.0, 0.5)), tempo: Some((90.0 + k as f64, 4)) })),
                5 => drop(tx.push(RtMsg::Tempo { bpm: 140.0 - k as f64 * 0.1, beats_per_bar: 4 })),
                7 => drop(tx.push(RtMsg::Seek(10_000 + k as u64 * 100))),
                _ => {}
            }
            let before = counting::ops();
            r.process(&mut out);
            ops += counting::ops() - before;
            while let Ok(old) = grx.pop() {
                drop(old);
                returned += 1;
            }
        }
        assert_eq!(ops, 0, "Allokationen/Freigaben im Mischer");
        // 27 arrangement messages: each replaced one comes back (plus the faded-out transitions)
        assert!(returned >= 27, "ersetzte Arrangements und Übergänge zurückgegeben: {returned}");
    }

    #[test]
    fn mixes_clips_at_their_position() {
        let (mut r, mut tx, mut grx) = renderer();
        let data: Arc<[f32]> = vec![0.5f32; 100].into();
        let arr = Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 10, data, side: None, gain_l: 1.0, gain_r: 0.5, send: 0.0 }] };
        let _ = tx.push(arrangement_msg(arr));
        let _ = tx.push(RtMsg::Play);
        let mut out = vec![0f32; 2 * 64];
        r.process(&mut out);
        assert_eq!(out[2 * 9], 0.0);
        assert_eq!(out[2 * 10], 0.5);
        assert_eq!(out[2 * 10 + 1], 0.25);
        assert_eq!(r.pos, 64);
        // The old (empty) arrangement is ready for freeing.
        assert!(grx.pop().is_ok());
    }

    #[test]
    fn side_signal_widens_left_and_right() {
        let (mut r, mut tx, _g) = renderer();
        let data: Arc<[f32]> = vec![0.5f32; 64].into();
        let side: Arc<[f32]> = vec![0.25f32; 64].into();
        let arr = Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 0, data, side: Some(side), gain_l: 1.0, gain_r: 1.0, send: 0.0 }] };
        let _ = tx.push(arrangement_msg(arr));
        let _ = tx.push(RtMsg::Play);
        let mut out = vec![0f32; 2 * 32];
        r.process(&mut out);
        // (the soft limiter at the output squashes 0.75 slightly)
        assert!((out[4] - 0.75).abs() < 0.002 && (out[5] - 0.25).abs() < 1e-6, "links Mitte + Seite, rechts Mitte − Seite: {:?}", &out[4..6]);
        assert!((out[4] + out[5] - 1.0).abs() < 0.002, "Mono-Summe unverändert");
    }

    #[test]
    fn metronome_clicks_on_beats_and_tempo_change_keeps_beat_position() {
        let (mut r, mut tx, _g) = renderer();
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(RtMsg::Metronome(true));
        let _ = tx.push(RtMsg::Play);
        let mut out = vec![0f32; 2 * 1024];
        r.process(&mut out);
        assert!(out.iter().any(|v| v.abs() > 0.01), "Klick auf Schlag 1 fehlt");
        // 24000 frames = 1 beat at 120 BPM. Half tempo → position doubled.
        r.pos = 24_000;
        let _ = tx.push(RtMsg::Tempo { bpm: 60.0, beats_per_bar: 4 });
        r.process(&mut out);
        assert_eq!(r.pos, 48_000 + 1024);
    }

    #[test]
    fn preview_plays_while_transport_is_stopped() {
        let (mut r, mut tx, _g) = renderer();
        let data: Arc<[f32]> = vec![0.25f32; 10].into();
        let clip = PlacedClip { start_frame: 0, data, side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 };
        let _ = tx.push(RtMsg::Preview(Arc::new(Arrangement { notes: Vec::new(), clips: vec![clip] })));
        let mut out = vec![0f32; 2 * 16];
        r.process(&mut out);
        assert_eq!(out[0], 0.25);
        assert_eq!(out[2 * 9], 0.25);
        assert_eq!(out[2 * 10], 0.0);
        assert_eq!(r.pos, 0, "Transport bleibt stehen");
    }

    #[test]
    fn loop_jumps_back_at_its_end_and_not_when_started_behind_it() {
        let (mut r, mut tx, _g) = renderer();
        // 120 BPM at 48 kHz: 1 beat = 24,000 frames; loop beat 1–2
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(RtMsg::Loop(Some((1.0, 2.0))));
        let _ = tx.push(RtMsg::Seek(47_900));
        let _ = tx.push(RtMsg::Play);
        let mut out = vec![0f32; 2 * 256];
        r.process(&mut out);
        // 100 frames to the end, then on from 24,000: 156 frames
        assert_eq!(r.pos, 24_000 + 156);
        // started past the end: no loop
        r.pos = 60_000;
        r.process(&mut out);
        assert_eq!(r.pos, 60_256);
        // off: no jump
        let _ = tx.push(RtMsg::Loop(None));
        r.pos = 47_900;
        r.process(&mut out);
        assert_eq!(r.pos, 48_156);
    }

    #[test]
    fn offline_render_matches_the_clip_mix() {
        let data: Arc<[f32]> = vec![0.25f32; 100].into();
        let arr = Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 10, data, side: None, gain_l: 1.0, gain_r: 0.5, send: 0.0 }] };
        let (l, r) = render_offline(arr, 48_000, 2_000);
        assert_eq!(l.len(), 2_000);
        assert_eq!(l[5], 0.0);
        assert!((l[50] - soft_limit(0.25)).abs() < 1e-6);
        assert!((r[50] - soft_limit(0.125)).abs() < 1e-6);
        assert_eq!(l[500], 0.0, "nach dem Clip still (ohne Hall-Send)");
    }

    #[test]
    fn stops_at_the_halt_point_and_jumps_back() {
        let (mut r, mut tx, _g) = renderer();
        // 1 beat = 24,000 frames: stop at beat 2, back to beat 1
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(RtMsg::StopAt(Some((2.0, 1.0))));
        let _ = tx.push(RtMsg::Seek(47_900));
        let _ = tx.push(RtMsg::Play);
        let mut out = vec![0f32; 2 * 256];
        r.process(&mut out);
        assert!(!r.playing, "angehalten");
        assert_eq!(r.pos, 24_000, "zurück auf den Anfang des Bereichs");
        // a loop takes precedence
        let _ = tx.push(RtMsg::Loop(Some((1.0, 2.0))));
        let _ = tx.push(RtMsg::Seek(47_900));
        let _ = tx.push(RtMsg::Play);
        r.process(&mut out);
        assert!(r.playing);
        assert_eq!(r.pos, 24_000 + 156);
    }

    #[test]
    fn reports_previewing_until_the_preview_has_played() {
        let (mut r, mut tx, _g) = renderer();
        let data: Arc<[f32]> = vec![0.1f32; 300].into();
        let _ = tx.push(RtMsg::Preview(Arc::new(Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 0, data, side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 }] })));
        let mut out = vec![0f32; 2 * 256];
        r.process(&mut out);
        assert!(r.shared.previewing.load(Ordering::Relaxed));
        r.process(&mut out);
        assert!(!r.shared.previewing.load(Ordering::Relaxed), "nach 512 von 300 Frames vorbei");
        // stop = empty preview
        let data: Arc<[f32]> = vec![0.1f32; 3000].into();
        let _ = tx.push(RtMsg::Preview(Arc::new(Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 0, data, side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 }] })));
        r.process(&mut out);
        assert!(r.shared.previewing.load(Ordering::Relaxed));
        let _ = tx.push(RtMsg::Preview(Arc::new(Arrangement::default())));
        r.process(&mut out);
        assert!(!r.shared.previewing.load(Ordering::Relaxed));
    }

    #[test]
    fn preview_more_continues_where_the_preview_is() {
        let (mut r, mut tx, _g) = renderer();
        let a: Arc<[f32]> = vec![0.1f32; 256].into();
        let _ = tx.push(RtMsg::Preview(Arc::new(Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 0, data: a, side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 }] })));
        let mut out = vec![0f32; 2 * 256];
        r.process(&mut out);
        // second piece right behind it: keeps running without restarting from the beginning
        let b: Arc<[f32]> = vec![0.3f32; 256].into();
        let _ = tx.push(RtMsg::PreviewMore(Arc::new(Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 256, data: b, side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 }] })));
        r.process(&mut out);
        assert_eq!(out[0], 0.3);
        r.process(&mut out);
        assert!(!r.shared.previewing.load(Ordering::Relaxed));
    }

    struct Pieces(u64);
    impl PreviewSource for Pieces {
        fn next(&mut self) -> Option<(PlacedClip, u64)> {
            if self.0 >= 10 {
                return None;
            }
            let start = self.0 * 100;
            self.0 += 1;
            Some((PlacedClip { start_frame: start, data: vec![0.5f32; 100].into(), side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 }, start + 100))
        }
    }

    #[test]
    fn stream_renders_ahead_and_drops_played_clips() {
        let mut st = PreviewStream { source: Box::new(Pieces(0)), clips: Vec::new(), covered: 0, started: 0.0, done: false };
        assert!(st.fill(0, 250));
        assert_eq!(st.covered, 300, "bis über den Vorlauf hinaus");
        assert!(!st.fill(10, 250), "noch genug Vorlauf");
        assert!(st.fill(150, 250));
        assert_eq!(st.covered, 400);
        assert_eq!(st.clips.first().map(|c| c.start_frame), Some(100), "Abgespieltes fällt weg");
        st.fill(2000, 250);
        assert!(st.done);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn mirror_brings_a_fresh_callback_to_the_same_state() {
        // device change (desktop): the new callback knows tempo, metronome, loop and arrangement
        let mut m = crate::device::Mirror::default();
        let data: Arc<[f32]> = vec![0.5f32; 100].into();
        let arr = Arc::new(Arrangement { notes: Vec::new(), clips: vec![PlacedClip { start_frame: 0, data, side: None, gain_l: 1.0, gain_r: 1.0, send: 0.0 }] });
        for msg in [RtMsg::Tempo { bpm: 90.0, beats_per_bar: 3 }, RtMsg::Metronome(true), RtMsg::Loop(Some((1.0, 2.0))), RtMsg::Arrangement { arrangement: arr, tempo: None }, RtMsg::Play] {
            m.note(&msg, 48_000);
        }
        let (mut r, mut tx, _g) = renderer();
        for msg in m.replay(48_000) {
            let _ = tx.push(msg);
        }
        let mut out = vec![0f32; 2 * 16];
        r.process(&mut out);
        assert_eq!((r.bpm, r.beats_per_bar), (90.0, 3));
        assert!(r.metronome);
        assert_eq!(r.loop_beats, Some((1.0, 2.0)));
        assert_eq!(r.arrangement.clips.len(), 1);
        assert!(!r.playing, "Abspielen entscheidet der Wechsel selbst");
        // different sample rate: the arrangement no longer fits, the app renders again
        assert!(!m.replay(44_100).iter().any(|x| matches!(x, RtMsg::Arrangement { .. })));
    }

    #[test]
    fn stopped_engine_outputs_silence_and_keeps_position() {
        let (mut r, _tx, _g) = renderer();
        r.pos = 500;
        let mut out = vec![1f32; 2 * 256];
        r.process(&mut out);
        assert!(out.iter().all(|&v| v == 0.0));
        assert_eq!(r.pos, 500);
    }

    /// How many clips (tracks, voices) does the renderer mix in real time? All
    /// sound at once, with side signal (stereo width) and reverb share –
    /// the most expensive case. 16 different buffers, distributed across all clips
    /// with offsets (memory stays small, but every clip is still computed). Computed in blocks of 128 frames at 48 kHz (as in the
    /// AudioWorklet), budget per block 2.67 ms. Counterpart in the browser:
    /// `tests/perf/voices.mjs`. Measurement only:
    /// `cargo test --release -p klang-engine -- --ignored --nocapture stimmen`
    #[test]
    #[ignore = "Messung"]
    fn stimmen_in_echtzeit() {
        const BLOCK: usize = 128;
        let budget_us = BLOCK as f64 / 48_000.0 * 1e6;
        let mut seed = 1u32;
        let mut noise = |n: usize| -> Arc<[f32]> {
            (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.01
                })
                .collect::<Vec<f32>>()
                .into()
        };
        println!("Clips   µs/Block   Anteil am Budget");
        let mut last_ok = 0;
        let bufs: Vec<(Arc<[f32]>, Arc<[f32]>)> = (0..16).map(|_| (noise(48_000 * 4), noise(48_000 * 4))).collect();
        for n in [1usize, 8, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192] {
            let mut m = Mixer::new(48_000.0, 2);
            let clips = (0..n)
                .map(|i| {
                    let (d, sd) = &bufs[i % 16];
                    PlacedClip { start_frame: (i % 97) as u64, data: d.clone(), side: Some(sd.clone()), gain_l: 0.8, gain_r: 0.7, send: 0.3 }
                })
                .collect();
            m.send(arrangement_msg(Arrangement { clips, notes: Vec::new() }));
            m.send(RtMsg::Play);
            let mut out = vec![0f32; BLOCK * 2];
            for _ in 0..50 {
                m.process(&mut out);
            }
            let blocks = 1000;
            let t = std::time::Instant::now();
            for _ in 0..blocks {
                m.process(&mut out);
            }
            let us = t.elapsed().as_secs_f64() * 1e6 / blocks as f64;
            println!("{n:>5}  {us:>9.1}   {:>6.1} %", us / budget_us * 100.0);
            if us < budget_us * 0.5 {
                last_ok = n;
            }
        }
        println!("Bis {last_ok} gleichzeitige Clips bleibt das Mischen unter der Hälfte des Echtzeit-Budgets.");
    }

    // ------------------------------------------------ Live note clips

    fn note_clip(track: u64, start_beats: f64, patch: Patch, shaping: Shaping, notes: &[(f64, f64, f32)], len_beats: f64) -> NoteClip {
        let notes: Vec<LiveNote> = notes.iter().map(|&(s, l, m)| LiveNote::new(s, l, m, 0.5)).collect();
        NoteClip { track, start_beats, notes: Arc::new(LiveNotes::new(patch, shaping, &notes, len_beats, SR)), gain_l: 1.0, gain_r: 1.0, send: 0.0 }
    }

    fn live(clips: Vec<NoteClip>) -> Arrangement {
        Arrangement { clips: Vec::new(), notes: clips }
    }

    fn pad() -> Patch {
        Patch::Synth(klang_dsp::pad::SynthParams::default(), klang_dsp::pad::Verlauf::default())
    }

    fn pluck() -> Patch {
        Patch::Pluck(klang_dsp::synth::PluckParams::default())
    }

    /// Plays `blocks` blocks (messages first) and appends the left channel before the limiter.
    fn play_raw(r: &mut Renderer, grx: &mut Consumer<Garbage>, blocks: usize, left: &mut Vec<f32>) {
        for _ in 0..blocks {
            r.handle_messages();
            r.render_block(BLOCK);
            left.extend_from_slice(&r.mix_l[..BLOCK]);
            while let Ok(g) = grx.pop() {
                drop(g);
            }
        }
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn note_clips_play_live_like_the_offline_render() {
        let shaping = Shaping { waerme: 0.4, schwebung: 0.3, farbe: 0.6, ..Default::default() };
        let melody = [(0.0, 0.5, 60.0), (0.5, 0.5, 64.0), (1.0, 1.0, 67.0), (1.0, 1.0, 72.0), (2.5, 0.5, 62.0), (3.75, 1.0, 55.0)];
        let beat = [(0.0, 0.25, 36.0), (0.5, 0.25, 38.0), (1.0, 0.25, 42.0), (1.25, 0.25, 46.0), (1.5, 0.25, 42.0), (3.0, 0.25, 49.0)];
        for (patch, notes) in [(pad(), &melody), (Patch::Bass(Default::default()), &melody), (Patch::Drums(Default::default()), &beat), (pluck(), &melody)] {
            let clip = note_clip(1, 0.0, patch.clone(), shaping, notes, 4.0);
            let specs: Vec<klang_dsp::synth::NoteSpec> = clip.notes.notes.iter().map(|n| n.spec(0.5)).collect();
            let (offline, _) = klang_dsp::live::render(&patch, &shaping, &specs, 2.0, SR, 0.5);
            let (mut r, mut tx, mut grx) = renderer();
            let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
            let _ = tx.push(arrangement_msg(live(vec![clip])));
            let _ = tx.push(RtMsg::Play);
            let mut left = Vec::new();
            play_raw(&mut r, &mut grx, offline.len() / BLOCK + 1, &mut left);
            let diff = offline.iter().zip(&left).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(rms(&offline) > 0.001, "{patch:?} sounds");
            assert!(diff < 1e-4, "{patch:?}: live differs from offline by {diff}");
        }
    }

    #[test]
    fn a_tempo_change_keeps_long_notes_sounding_without_a_gap() {
        let clip = note_clip(1, 0.0, pad(), Shaping::default(), &[(0.0, 16.0, 57.0), (0.0, 16.0, 64.0)], 16.0);
        let (mut r, mut tx, mut grx) = renderer();
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(arrangement_msg(live(vec![clip])));
        let _ = tx.push(RtMsg::Play);
        let mut left = Vec::new();
        play_raw(&mut r, &mut grx, 200, &mut left);
        let before = rms(&left[150 * BLOCK..]);
        let beat = r.pos as f64 / r.samples_per_beat();
        let _ = tx.push(RtMsg::Tempo { bpm: 80.0, beats_per_bar: 4 });
        let mut after = Vec::new();
        play_raw(&mut r, &mut grx, 60, &mut after);
        assert!((r.pos as f64 / r.samples_per_beat() - beat - 60.0 * BLOCK as f64 / r.samples_per_beat()).abs() < 0.01, "beat position kept");
        for (k, block) in after.chunks(BLOCK).enumerate() {
            assert!(rms(block) > before * 0.3, "block {k} after the tempo change: {} vs {before}", rms(block));
        }
        assert!(max_step(&[&left[left.len() - BLOCK..], &after[..4 * BLOCK]].concat()) < 0.05, "no click");
    }

    #[test]
    fn starting_in_the_middle_of_a_note_picks_it_up() {
        for patch in [pad(), pluck(), Patch::Organ(Default::default()), Patch::Bass(Default::default())] {
            let clip = || note_clip(1, 0.0, patch.clone(), Shaping::default(), &[(0.0, 8.0, 52.0)], 8.0);
            // the note played from its start: how loud is it at beat 2?
            let (mut r, mut tx, mut grx) = renderer();
            let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
            let _ = tx.push(arrangement_msg(live(vec![clip()])));
            let _ = tx.push(RtMsg::Play);
            let mut whole = Vec::new();
            play_raw(&mut r, &mut grx, (2 * 24_000 + 20 * BLOCK) / BLOCK, &mut whole);
            let want = rms(&whole[2 * 24_000 + 2 * BLOCK..]);
            // stopped: jump to beat 2, then play
            let (mut r, mut tx, mut grx) = renderer();
            let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
            let _ = tx.push(arrangement_msg(live(vec![clip()])));
            let _ = tx.push(RtMsg::Seek(2 * 24_000));
            let _ = tx.push(RtMsg::Play);
            let mut left = Vec::new();
            play_raw(&mut r, &mut grx, 20, &mut left);
            let have = rms(&left[2 * BLOCK..]);
            assert!(want > 1e-4, "{patch:?} still sounds at beat 2");
            assert!(have > want * 0.5 && have < want * 2.0, "{patch:?} picked up: {have} vs {want}");
            assert!(left[0].abs() < 0.01, "{patch:?} fades in");
        }
    }

    #[test]
    fn stopping_fades_the_voices_out() {
        let clip = note_clip(1, 0.0, pad(), Shaping { breite: 0.5, schwebung: 0.4, ..Default::default() }, &[(0.0, 16.0, 57.0)], 16.0);
        let (mut r, mut tx, mut grx) = renderer();
        let _ = tx.push(arrangement_msg(live(vec![clip])));
        let _ = tx.push(RtMsg::Play);
        let mut left = Vec::new();
        play_raw(&mut r, &mut grx, 100, &mut left);
        assert!(r.live.sounding());
        let _ = tx.push(RtMsg::Stop);
        let mut after = Vec::new();
        play_raw(&mut r, &mut grx, 60, &mut after);
        assert!(max_step(&[&left[left.len() - BLOCK..], &after[..8 * BLOCK]].concat()) < 0.05, "no click at the stop");
        assert!(rms(&after[40 * BLOCK..]) < 1e-4, "silent after the fade");
        assert!(!r.live.sounding(), "voices and chains done");
    }

    #[test]
    fn notes_at_the_loop_start_sound_again_after_the_jump() {
        let clip = note_clip(1, 0.0, pluck(), Shaping::default(), &[(0.0, 0.5, 64.0)], 2.0);
        let (mut r, mut tx, mut grx) = renderer();
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(arrangement_msg(live(vec![clip])));
        let _ = tx.push(RtMsg::Loop(Some((0.0, 1.0))));
        let _ = tx.push(RtMsg::Play);
        // three times through the loop (one beat = 24 000 frames)
        let mut out = vec![0f32; 2 * BLOCK];
        let mut left = Vec::new();
        for _ in 0..(3 * 24_000 / BLOCK) {
            r.process(&mut out);
            left.extend(out.chunks(2).map(|f| f[0]));
            while let Ok(g) = grx.pop() {
                drop(g);
            }
        }
        for k in 0..3 {
            let at = k * 24_000;
            assert!(rms(&left[at + 200..at + 4000]) > 0.02, "note at loop pass {k}");
            assert!(rms(&left[at + 15_000..at + 23_000]) < rms(&left[at + 200..at + 4000]) * 0.5, "it decays before the next pass");
        }
    }

    #[test]
    fn many_notes_at_once_reuse_the_oldest_voices() {
        let chord: Vec<(f64, f64, f32)> = (0..400).map(|k| (k as f64 * 0.001, 4.0, 40.0 + (k % 40) as f32)).collect();
        let strings: Vec<(f64, f64, f32)> = (0..100).map(|k| (k as f64 * 0.002, 4.0, 50.0 + (k % 20) as f32)).collect();
        let (mut r, mut tx, mut grx) = renderer();
        let _ = tx.push(arrangement_msg(live(vec![note_clip(1, 0.0, pad(), Shaping::default(), &chord, 4.0), note_clip(2, 0.0, pluck(), Shaping::default(), &strings, 4.0)])));
        let _ = tx.push(RtMsg::Play);
        let mut left = Vec::new();
        play_raw(&mut r, &mut grx, 60, &mut left);
        assert!(left.iter().all(|v| v.is_finite()));
        assert!(rms(&left) > 0.01);
    }

    #[test]
    fn live_voices_neither_allocate_nor_free_in_the_mixer() {
        let (tx_rt, rx) = RingBuffer::new(64);
        let (gtx, mut grx) = RingBuffer::new(GARBAGE);
        let mut r = Renderer::new(SR, 2, rx, gtx, Arc::new(Shared::default()));
        let mut tx = tx_rt;
        let melody: Vec<(f64, f64, f32)> = (0..64).map(|k| (k as f64 * 0.25, 0.6, 48.0 + (k * 5 % 24) as f32)).collect();
        let beat: Vec<(f64, f64, f32)> = (0..64).map(|k| (k as f64 * 0.25, 0.2, [36.0, 42.0, 38.0, 46.0][k % 4])).collect();
        let shaping = Shaping { waerme: 0.5, schwebung: 0.5, rauheit: 0.3, farbe: 0.3, raum: 0.7, breite: 0.6 };
        let make = |k: usize| {
            live(vec![
                note_clip(1, 0.0, pluck(), shaping, &melody, 16.0),
                note_clip(2, 0.0, pad(), Shaping { breite: (k % 3) as f32 * 0.3, ..shaping }, &melody, 16.0),
                note_clip(3, 0.0, Patch::Drums(Default::default()), Shaping::default(), &beat, 16.0),
                note_clip(4, 0.0, Patch::Organ(Default::default()), shaping, &melody, 16.0),
                note_clip(5, 0.0, Patch::Harpsichord(klang_dsp::keys::HarpsichordParams { vierfuss: 0.6, ..Default::default() }), shaping, &melody, 16.0),
            ])
        };
        let _ = tx.push(RtMsg::Tempo { bpm: 120.0, beats_per_bar: 4 });
        let _ = tx.push(arrangement_msg(make(0)));
        let _ = tx.push(RtMsg::Play);
        let mut out = [0f32; 2 * BLOCK];
        let mut ops = 0;
        for k in 0..400 {
            // messages are built outside (as in the app)
            match k % 37 {
                3 => drop(tx.push(arrangement_msg(make(k)))),
                11 => drop(tx.push(RtMsg::Tempo { bpm: 100.0 + (k % 50) as f64, beats_per_bar: 4 })),
                17 => drop(tx.push(RtMsg::Seek(24_000 + k as u64 * 300))),
                23 => drop(tx.push(RtMsg::Loop(Some((1.0, 3.0))))),
                29 => drop(tx.push(RtMsg::Stop)),
                31 => drop(tx.push(RtMsg::Play)),
                _ => {}
            }
            let before = counting::ops();
            r.process(&mut out);
            ops += counting::ops() - before;
            while let Ok(old) = grx.pop() {
                drop(old);
            }
        }
        assert_eq!(ops, 0, "allocations/frees in the mixer");
    }

    #[test]
    fn a_parallel_export_sounds_like_one_renderer() {
        let melody: Vec<(f64, f64, f32)> = (0..24).map(|k| (k as f64 * 0.5, 0.7, 48.0 + (k * 7 % 24) as f32)).collect();
        let beat: Vec<(f64, f64, f32)> = (0..24).map(|k| (k as f64 * 0.5, 0.2, [36.0, 42.0, 38.0, 42.0][k % 4])).collect();
        let shaping = Shaping { waerme: 0.4, schwebung: 0.5, breite: 0.6, raum: 0.6, ..Default::default() };
        let make = || {
            let mut a = live(vec![
                note_clip(1, 0.0, pluck(), shaping, &melody, 6.0),
                note_clip(1, 6.0, pluck(), shaping, &melody, 6.0),
                note_clip(2, 0.0, pad(), shaping, &melody[..8], 12.0),
                note_clip(3, 0.0, Patch::Drums(Default::default()), Shaping::default(), &beat, 12.0),
                note_clip(4, 2.0, Patch::Bass(Default::default()), shaping, &melody, 8.0),
            ]);
            for (k, n) in a.notes.iter_mut().enumerate() {
                n.send = 0.1 * k as f32;
                n.gain_l = 1.0 - 0.1 * k as f32;
            }
            a.clips.push(PlacedClip { start_frame: 1000, data: vec![0.2f32; 30_000].into(), side: None, gain_l: 0.5, gain_r: 0.7, send: 0.3 });
            a
        };
        let frames = 7 * 48_000;
        let (sl, sr) = render_serial(make(), 48_000, frames);
        for threads in [2, 3, 8] {
            let (pl, pr) = render_parallel(make(), 48_000, frames, threads);
            let d = sl.iter().zip(&pl).chain(sr.iter().zip(&pr)).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert_eq!(pl.len(), frames as usize);
            assert!(d < 1e-4, "{threads} threads: difference {d}");
        }
        assert!(rms(&sl) > 0.01);
    }

    #[test]
    fn a_clip_rendered_step_by_step_sounds_like_it_does_live() {
        let melody: Vec<(f64, f64, f32)> = (0..16).map(|k| (k as f64 * 0.5, 0.8, 50.0 + (k * 5 % 19) as f32)).collect();
        let shaping = Shaping { waerme: 0.3, schwebung: 0.4, breite: 0.7, raum: 0.5, ..Default::default() };
        for patch in [pad(), pluck()] {
            let clip = || note_clip(7, 0.0, patch.clone(), shaping, &melody, 8.0);
            let frames = clip().notes.patch.buffer_len(8.0 * 60.0 / 100.0, SR) as u64;
            let mut live = live(vec![clip()]);
            live.notes[0].send = 0.0;
            let (mut r, mut grx) = offline_renderer(live, 48_000, true);
            r.bpm = 100.0;
            let (mut l, mut rr) = (Vec::new(), Vec::new());
            while (l.len() as u64) < frames {
                let n = ((frames - l.len() as u64) as usize).min(MAX_BLOCK);
                r.render_block(n);
                l.extend_from_slice(&r.mix_l[..n]);
                rr.extend_from_slice(&r.mix_r[..n]);
                while let Ok(g) = grx.pop() {
                    drop(g);
                }
            }
            // in odd steps, as between other work
            let mut s = StepRender::new(clip(), 100.0, 48_000);
            let mut steps = 0;
            while !s.step(1000 + 37 * steps) {
                steps += 1;
            }
            let (mid, side) = s.finish();
            let side = side.expect("wide clip has a side");
            assert_eq!(mid.len(), l.len());
            let d = (0..mid.len()).map(|i| (mid[i] + side[i] - l[i]).abs().max((mid[i] - side[i] - rr[i]).abs())).fold(0.0, f32::max);
            assert!(d < 1e-5, "{patch:?}: difference {d}");
            assert!(rms(&l) > 0.001);
        }
    }
}
