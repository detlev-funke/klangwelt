//! Live voices in the mixer: note clips are played by voices that produce
//! their sound sample by sample (`klang_dsp::live`) – nothing is rendered
//! beforehand, a new tempo or arrangement takes effect immediately.
//!
//! * Each track gets a running sound shaping chain ([`FxChain`]); all of its
//!   voices sound through it – also the ones of a fading state, so a
//!   transition does not interrupt the chain.
//! * Voices belong to a *context*: the current state or a fading one
//!   ([`crate::Renderer`]). They take its level, so they fade out with it.
//!   When playback starts or jumps into the middle of notes, those are
//!   picked up ([`Live::chase`]) and fade in briefly.
//! * Everything is allocated up front (voice slots, string delay lines,
//!   chains); a finished voice hands its clip back through the garbage
//!   channel, so nothing is freed in the mixer either.

use crate::{Garbage, NoteClip, MAX_BLOCK};
use klang_dsp::live::{arena_len, FxChain, LiveNotes, Voice};
use rtrb::Producer;
use std::sync::Arc;

/// Voices sounding at the same time.
pub(crate) const MAX_VOICES: usize = 512;
/// Of these with plucked strings (delay lines).
const MAX_ARENAS: usize = 96;
/// Tracks with live voices at the same time.
const MAX_BUSES: usize = 128;
/// Context of voices that fade out on their own (after stopping, at a loop jump).
pub(crate) const RELEASED: u32 = u32::MAX;
/// A voice picked up in the middle fades in over this time.
const PICKUP_FADE_SECS: f32 = 0.004;
/// After its last voice a chain keeps running this long (delay lines and filters ring out).
const BUS_TAIL_SECS: f32 = 0.1;

/// Levels of the contexts in the current block.
pub(crate) struct Levels<'a> {
    /// The current state.
    pub current: u32,
    /// Fading states: context and level at the start of the block.
    pub fading: &'a [(u32, f32)],
    /// Level drop per sample of a fading state.
    pub step: f32,
    /// Sum of the fading levels per sample (`None`: nothing fades).
    pub fade_sum: Option<&'a [f32]>,
}

/// How the level of a context runs through the block.
#[derive(Clone, Copy)]
enum Curve {
    Full,
    /// The current state while others fade: what they leave over.
    Rest,
    /// A fading state: from this level down.
    Falling(f32),
}

impl Levels<'_> {
    /// Level curve of context `ctx`; `None` if it no longer exists.
    fn curve(&self, ctx: u32) -> Option<Curve> {
        if ctx == RELEASED {
            return Some(Curve::Full);
        }
        if ctx == self.current {
            return Some(if self.fade_sum.is_some() { Curve::Rest } else { Curve::Full });
        }
        self.fading.iter().find(|(c, _)| *c == ctx).map(|&(_, g0)| Curve::Falling(g0))
    }

    #[inline]
    fn at(&self, c: Curve, i: usize) -> f32 {
        match c {
            Curve::Full => 1.0,
            Curve::Rest => self.fade_sum.map_or(1.0, |s| (1.0 - s[i]).max(0.0)),
            Curve::Falling(g0) => (g0 - i as f32 * self.step).max(0.0),
        }
    }
}

struct Slot {
    voice: Voice,
    clip: Arc<LiveNotes>,
    bus: usize,
    ctx: u32,
    arena: Option<usize>,
    /// Starts this many samples into the next block.
    delay: usize,
    /// Fade-in after a pickup: samples done, of how many.
    fade_in: (u32, u32),
    /// Own level once released (falls to 0, then the voice ends).
    level: f32,
    falling: bool,
}

struct Bus {
    track: Option<u64>,
    fx: FxChain,
    input: Box<[f32]>,
    voices: usize,
    /// Samples the chain still runs after its last voice.
    tail: usize,
    /// Left, right, reverb: current and target level (smoothed).
    gain: [f32; 3],
    target: [f32; 3],
    /// Block in which a voice last started (to reuse the longest unused chain).
    last_used: u64,
    /// Since this sample the chain rests (no voice, rung out); when it
    /// continues, it catches up on the pause.
    paused_at: Option<u64>,
    /// New target level from this sample of the block on.
    retarget: Option<(usize, [f32; 3])>,
}

pub(crate) struct Live {
    sr: f32,
    slots: Vec<Option<Slot>>,
    arenas: Vec<Box<[f32]>>,
    arena_used: Vec<bool>,
    buses: Vec<Bus>,
    scratch: Box<[f32]>,
    pickup_len: u32,
    bus_tail: usize,
    /// Smoothing of the track levels per sample.
    glide: f32,
    /// Fade-out per sample of a released voice.
    step: f32,
    blocks: u64,
    /// Samples rendered so far.
    clock: u64,
}

/// First sample of a position in beats.
#[inline]
fn frame(beats: f64, spf: f64) -> u64 {
    (beats * spf).max(0.0).floor() as u64
}

impl Live {
    pub(crate) fn new(sr: f32, fade_step: f32) -> Self {
        let a = arena_len(sr);
        Self {
            sr,
            slots: (0..MAX_VOICES).map(|_| None).collect(),
            // filled with -0.0 instead of zeros: the memory is committed now, not on first use in
            // the audio thread (a voice overwrites what it reads; -0.0 counts as silence anyway)
            arenas: (0..MAX_ARENAS).map(|_| vec![-0.0f32; a].into_boxed_slice()).collect(),
            arena_used: vec![false; MAX_ARENAS],
            buses: (0..MAX_BUSES)
                .map(|_| Bus {
                    track: None,
                    fx: FxChain::new(sr),
                    input: vec![-0.0f32; MAX_BLOCK].into_boxed_slice(),
                    voices: 0,
                    tail: 0,
                    gain: [0.0; 3],
                    target: [0.0; 3],
                    last_used: 0,
                    paused_at: None,
                    retarget: None,
                })
                .collect(),
            scratch: vec![-0.0f32; MAX_BLOCK].into_boxed_slice(),
            pickup_len: ((PICKUP_FADE_SECS * sr) as u32).max(1),
            bus_tail: (BUS_TAIL_SECS * sr) as usize,
            glide: 1.0 - (-1.0 / (0.005 * sr)).exp(),
            step: fade_step,
            blocks: 0,
            clock: 0,
        }
    }

    /// Whether something still sounds (voices or chains ringing out).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn sounding(&self) -> bool {
        self.buses.iter().any(|b| b.voices > 0 || b.tail > 0)
    }

    /// Starts the notes of `clips` that begin in [`pos`, `pos + n`)
    /// (`spf` samples per beat) in context `ctx`.
    pub(crate) fn schedule(&mut self, clips: &[NoteClip], pos: u64, n: usize, spf: f64, ctx: u32, g: &mut Producer<Garbage>) {
        let end = pos + n as u64;
        for clip in clips {
            let notes = &clip.notes.notes;
            let (first, last) = match (notes.first(), notes.last()) {
                (Some(a), Some(b)) => (frame(clip.start_beats + a.start, spf), frame(clip.start_beats + b.start, spf)),
                _ => continue,
            };
            if first >= end || last < pos {
                continue;
            }
            let lo = notes.partition_point(|nt| frame(clip.start_beats + nt.start, spf) < pos);
            for k in lo..notes.len() {
                let f = frame(clip.start_beats + notes[k].start, spf);
                if f >= end {
                    break;
                }
                self.start(clip, k, f, (f - pos) as usize, 0, spf, ctx, g);
            }
        }
    }

    /// Picks up the notes of `clips` that already sound at `pos` (playback
    /// starts or jumps into them): they continue at their place and fade in.
    pub(crate) fn chase(&mut self, clips: &[NoteClip], pos: u64, spf: f64, ctx: u32, g: &mut Producer<Garbage>) {
        let spb = spf / self.sr as f64;
        for clip in clips {
            let notes = &clip.notes.notes;
            let rel = pos as f64 / spf - clip.start_beats;
            let window = clip.notes.max_len + clip.notes.patch.release_secs() as f64 / spb;
            if rel <= 0.0 || rel > clip.notes.len_beats + window {
                continue;
            }
            let hi = notes.partition_point(|nt| frame(clip.start_beats + nt.start, spf) < pos);
            for k in (0..hi).rev() {
                if notes[k].start < rel - window {
                    break;
                }
                let f = frame(clip.start_beats + notes[k].start, spf);
                self.start(clip, k, f, 0, (pos - f) as usize, spf, ctx, g);
            }
        }
    }

    /// Starts note `k` of `clip` (first sample `note_frame`), `delay`
    /// samples into the block – or `offset` samples into the note (pickup).
    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, clip: &NoteClip, k: usize, note_frame: u64, delay: usize, offset: usize, spf: f64, ctx: u32, g: &mut Producer<Garbage>) {
        let data = &clip.notes;
        let spb = spf / self.sr as f64;
        let sound_end = frame(clip.start_beats, spf) + data.patch.buffer_len((data.len_beats * spb) as f32, self.sr) as u64;
        let limit = sound_end.saturating_sub(note_frame) as usize;
        if limit <= offset {
            return;
        }
        let Some(bus) = self.bus_for(clip.track) else { return };
        let slot = match self.slots.iter().position(Option::is_none) {
            Some(s) => s,
            None => self.steal(false, g),
        };
        let arena = if data.patch.uses_arena() {
            match self.arena_used.iter().position(|u| !u) {
                Some(a) => Some(a),
                None => {
                    let s = self.steal(true, g);
                    self.slots[s].is_none().then(|| self.arena_used.iter().position(|u| !u)).flatten()
                }
            }
        } else {
            None
        };
        if data.patch.uses_arena() && arena.is_none() {
            return;
        }
        let mut empty: [f32; 0] = [];
        let lines: &mut [f32] = match arena {
            Some(a) => &mut self.arenas[a],
            None => &mut empty,
        };
        let Some(mut voice) = data.start_voice(k, spb, self.sr, limit, lines) else { return };
        voice.skip(offset, lines);
        if voice.done() {
            return;
        }
        if let Some(a) = arena {
            self.arena_used[a] = true;
        }
        let b = &mut self.buses[bus];
        if let Some(p) = b.paused_at.take() {
            b.fx.skip(self.clock.saturating_sub(p) as usize);
        }
        b.fx.set(&data.shaping);
        let target = [clip.gain_l, clip.gain_r, clip.send];
        if b.voices == 0 && b.tail == 0 {
            // a resting chain takes the level at once (nothing sounds through it yet)
            (b.gain, b.target, b.retarget) = (target, target, None);
        } else if target != b.target {
            // from the note's first sample on, not before (independent of where the block begins)
            b.retarget = Some((delay, target));
        }
        b.voices += 1;
        b.last_used = self.blocks;
        self.slots[slot] = Some(Slot {
            voice,
            clip: data.clone(),
            bus,
            ctx,
            arena,
            delay,
            fade_in: if offset > 0 { (0, self.pickup_len) } else { (0, 0) },
            level: 1.0,
            falling: false,
        });
    }

    /// Chain of a track; a free one (or the longest unused) is taken over.
    fn bus_for(&mut self, track: u64) -> Option<usize> {
        if let Some(b) = self.buses.iter().position(|b| b.track == Some(track)) {
            return Some(b);
        }
        let b = self
            .buses
            .iter()
            .enumerate()
            .filter(|(_, b)| b.voices == 0 && b.tail == 0)
            .min_by_key(|(_, b)| (b.track.is_some(), b.last_used))
            .map(|(i, _)| i)?;
        let bus = &mut self.buses[b];
        bus.fx.reset(self.clock);
        bus.track = Some(track);
        bus.paused_at = None;
        Some(b)
    }

    /// Ends the voice that has been sounding longest (with a string arena if
    /// `arena`) to make room; returns its slot.
    fn steal(&mut self, arena: bool, g: &mut Producer<Garbage>) -> usize {
        let victim = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().filter(|s| !arena || s.arena.is_some()).map(|s| (i, s.voice.position())))
            .max_by_key(|&(_, p)| p)
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.end(victim, g);
        victim
    }

    fn end(&mut self, i: usize, g: &mut Producer<Garbage>) {
        if let Some(s) = self.slots[i].take() {
            if let Some(a) = s.arena {
                self.arena_used[a] = false;
            }
            let b = &mut self.buses[s.bus];
            b.voices = b.voices.saturating_sub(1);
            b.tail = self.bus_tail;
            let _ = g.push(Garbage::Notes(s.clip));
        }
    }

    /// Voices of context `ctx` fade out on their own from level `from`.
    pub(crate) fn release(&mut self, ctx: u32, from: f32) {
        for s in self.slots.iter_mut().flatten().filter(|s| s.ctx == ctx) {
            s.ctx = RELEASED;
            s.falling = true;
            s.level = s.level.min(from);
        }
    }

    /// Ends the voices of context `ctx` immediately.
    pub(crate) fn kill(&mut self, ctx: u32, g: &mut Producer<Garbage>) {
        for i in 0..self.slots.len() {
            if self.slots[i].as_ref().is_some_and(|s| s.ctx == ctx) {
                self.end(i, g);
            }
        }
    }

    /// Ends all voices that belong to a context other than `keep` (stop:
    /// the fading states are gone).
    pub(crate) fn kill_others(&mut self, keep: u32, g: &mut Producer<Garbage>) {
        for i in 0..self.slots.len() {
            if self.slots[i].as_ref().is_some_and(|s| s.ctx != keep && s.ctx != RELEASED) {
                self.end(i, g);
            }
        }
    }

    /// Renders the voices into their chains and the chains into the mix (left,
    /// right, reverb send) for `n` samples.
    pub(crate) fn render(&mut self, n: usize, levels: &Levels, mix: (&mut [f32], &mut [f32], &mut [f32]), g: &mut Producer<Garbage>) {
        self.blocks += 1;
        for b in self.buses.iter_mut().filter(|b| b.voices > 0 || b.tail > 0) {
            b.input[..n].fill(0.0);
        }
        for i in 0..self.slots.len() {
            let ctx = match &self.slots[i] {
                Some(s) => s.ctx,
                None => continue,
            };
            let Some(curve) = levels.curve(ctx) else {
                self.end(i, g);
                continue;
            };
            let Some(s) = self.slots[i].as_mut() else { continue };
            let start = s.delay.min(n);
            s.delay = 0;
            let lines: &mut [f32] = match s.arena {
                Some(a) => &mut self.arenas[a],
                None => &mut [],
            };
            let got = s.clip.render_voice(&mut s.voice, lines, &mut self.scratch[start..n]);
            let input = &mut self.buses[s.bus].input;
            let mut finished = got < n - start;
            for j in 0..got {
                let at = start + j;
                let mut v = self.scratch[at] * levels.at(curve, at);
                if s.fade_in.0 < s.fade_in.1 {
                    v *= s.fade_in.0 as f32 / s.fade_in.1 as f32;
                    s.fade_in.0 += 1;
                }
                if s.falling {
                    v *= s.level.max(0.0);
                    s.level -= self.step;
                }
                input[at] += v;
                if s.falling && s.level <= 0.0 {
                    finished = true;
                    break;
                }
            }
            if finished || s.voice.done() {
                self.end(i, g);
            }
        }
        let (ml, mr, ms) = mix;
        let glide = self.glide;
        for b in self.buses.iter_mut().filter(|b| b.voices > 0 || b.tail > 0) {
            for i in 0..n {
                if let Some((at, t)) = b.retarget {
                    if i >= at {
                        (b.target, b.retarget) = (t, None);
                    }
                }
                let (m, d) = b.fx.process(b.input[i]);
                for (c, t) in b.gain.iter_mut().zip(&b.target) {
                    *c += (t - *c) * glide;
                }
                let [gl, gr, gs] = b.gain;
                ml[i] += (m + d) * gl;
                mr[i] += (m - d) * gr;
                ms[i] += m * gs * 0.5 * (gl + gr);
            }
            if b.voices == 0 {
                b.tail = b.tail.saturating_sub(n);
                if b.tail == 0 {
                    b.paused_at = Some(self.clock + n as u64);
                }
            }
        }
        self.clock += n as u64;
    }
}
