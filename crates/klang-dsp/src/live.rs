//! Live synthesis: the instruments as voices that produce their sound sample
//! by sample while playing – nothing is rendered beforehand.
//!
//! * [`Patch`] – an instrument with its parameters, [`Shaping`] – the sound
//!   shaping after it (warmth, shimmer, roughness, color, room, width).
//! * [`LiveNotes`] – a note clip prepared for playing: notes in beats, plus
//!   what a voice needs from its neighbours ([`Plan`]: glide, choked hi-hat)
//!   and the organ's wavetables.
//! * [`Voice`] – one sounding note. Starting and rendering allocate nothing;
//!   plucked strings take their delay lines from an arena slot handed in
//!   ([`arena_len`]).
//! * [`FxChain`] – the sound shaping as a running chain (one per track in
//!   the mixer).
//! * [`render`] – the same offline: notes → buffer (previews, wave view,
//!   tests). It reproduces the instruments' offline functions sample for
//!   sample, so playback, export and previews sound alike.

use crate::bass::{poly_blep, BassParams, Svf};
use crate::drums::{self, square, DrumKit};
use crate::keys::{organ_partials, organ_table, HarpsichordParams, OrganParams, TABLE};
use crate::notes::midi_to_hz;
use crate::pad::{curve_at, gate, level_at, puls_period, SynthParams, Verlauf};
use crate::synth::{Biquad, NoteSpec, PluckParams, Rng};
use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::Arc;

/// An instrument with its parameters.
#[derive(Clone, Debug, PartialEq)]
pub enum Patch {
    Pluck(PluckParams),
    Drums(DrumKit),
    Bass(BassParams),
    Organ(OrganParams),
    Harpsichord(HarpsichordParams),
    Synth(SynthParams, Verlauf),
}

impl Patch {
    /// Length of the sound of a clip that is `total_secs` long: after its end
    /// the notes may still ring for a while (as many samples as the offline
    /// buffer of the instrument holds).
    pub fn buffer_len(&self, total_secs: f32, sr: f32) -> usize {
        let t = total_secs.max(0.0);
        match self {
            Patch::Pluck(_) => ((t + 0.1) * sr) as usize,
            Patch::Drums(_) => ((t + 1.2) * sr) as usize,
            Patch::Bass(_) => ((t + 0.2) * sr) as usize,
            Patch::Organ(_) | Patch::Harpsichord(_) => (t * sr) as usize,
            Patch::Synth(_, v) => ((t + v.ausklang.clamp(0.0, 2.0) + 0.05) * sr) as usize,
        }
    }

    /// Longest time a note sounds after its end (seconds) – to pick up notes
    /// that are already sounding when playback starts in their middle.
    pub fn release_secs(&self) -> f32 {
        match self {
            Patch::Pluck(_) => 0.08,
            // a crash cymbal rings longest
            Patch::Drums(_) => 2.9,
            Patch::Bass(_) => 0.04,
            Patch::Organ(_) => 0.14,
            Patch::Harpsichord(_) => 0.11,
            Patch::Synth(_, v) => v.ausklang.max(0.02),
        }
    }

    /// Whether a voice needs delay lines (plucked strings) from an arena slot.
    pub fn uses_arena(&self) -> bool {
        matches!(self, Patch::Pluck(_) | Patch::Harpsichord(_))
    }

    /// Appends the patch as numbers (for the trip to the AudioWorklet).
    pub fn encode(&self, out: &mut Vec<f64>) {
        let mut put = |kind: f64, v: &[f32]| {
            out.push(kind);
            out.extend(v.iter().map(|&x| x as f64));
        };
        match self {
            Patch::Pluck(p) => put(1.0, &[p.brightness, p.decay, p.body, p.attack]),
            Patch::Drums(k) => put(
                2.0,
                &[
                    k.kick.grundton,
                    k.kick.punch,
                    k.kick.laenge,
                    k.kick.klick,
                    k.kick.waerme,
                    k.snare.kessel,
                    k.snare.teppich,
                    k.snare.laenge,
                    k.snare.schaerfe,
                    k.clap.helligkeit,
                    k.clap.laenge,
                    k.hat.helligkeit,
                    k.hat.laenge,
                    k.hat.offen,
                    k.tom.grundton,
                    k.tom.laenge,
                    if k.pitched { 1.0 } else { 0.0 },
                ],
            ),
            Patch::Bass(p) => put(3.0, &[p.wellenform, p.filter, p.resonanz, p.anschlag, p.abklingen, p.sub, p.biss, p.glide]),
            Patch::Organ(p) => put(4.0, &[p.subbass, p.prinzipal, p.floete, p.quinte, p.oktave, p.mixtur, p.chiff, p.tremulant]),
            Patch::Harpsichord(p) => put(5.0, &[p.achtfuss, p.vierfuss, p.laute, p.kiel, p.nachklang]),
            Patch::Synth(p, v) => {
                put(6.0, &[p.stimmen, p.verstimmung, p.wellenform, p.sub, p.schimmer, p.filter, p.resonanz, p.bewegung, p.tempo, v.ausklang, v.puls as f32, v.pulstiefe]);
                for curve in [&v.laut, &v.hell] {
                    out.push(curve.len() as f64);
                    out.extend(curve.iter().flat_map(|p| [p[0] as f64, p[1] as f64]));
                }
            }
        }
    }

    /// Reads a patch from numbers ([`Patch::encode`]); returns it with the
    /// count of numbers read.
    pub fn decode(d: &[f64]) -> Option<(Patch, usize)> {
        let kind = *d.first()? as u32;
        let take = |n: usize| -> Option<Vec<f32>> { Some(d.get(1..1 + n)?.iter().map(|&x| x as f32).collect()) };
        Some(match kind {
            1 => {
                let v = take(4)?;
                (Patch::Pluck(PluckParams { brightness: v[0], decay: v[1], body: v[2], attack: v[3] }), 5)
            }
            2 => {
                use crate::drums::{ClapParams, HatParams, KickParams, SnareParams, TomParams};
                let v = take(17)?;
                let kit = DrumKit {
                    kick: KickParams { grundton: v[0], punch: v[1], laenge: v[2], klick: v[3], waerme: v[4] },
                    snare: SnareParams { kessel: v[5], teppich: v[6], laenge: v[7], schaerfe: v[8] },
                    clap: ClapParams { helligkeit: v[9], laenge: v[10] },
                    hat: HatParams { helligkeit: v[11], laenge: v[12], offen: v[13] },
                    tom: TomParams { grundton: v[14], laenge: v[15] },
                    pitched: v[16] >= 0.5,
                };
                (Patch::Drums(kit), 18)
            }
            3 => {
                let v = take(8)?;
                let p = BassParams { wellenform: v[0], filter: v[1], resonanz: v[2], anschlag: v[3], abklingen: v[4], sub: v[5], biss: v[6], glide: v[7] };
                (Patch::Bass(p), 9)
            }
            4 => {
                let v = take(8)?;
                let p = OrganParams { subbass: v[0], prinzipal: v[1], floete: v[2], quinte: v[3], oktave: v[4], mixtur: v[5], chiff: v[6], tremulant: v[7] };
                (Patch::Organ(p), 9)
            }
            5 => {
                let v = take(5)?;
                (Patch::Harpsichord(HarpsichordParams { achtfuss: v[0], vierfuss: v[1], laute: v[2], kiel: v[3], nachklang: v[4] }), 6)
            }
            6 => {
                let v = take(12)?;
                let p = SynthParams {
                    stimmen: v[0],
                    verstimmung: v[1],
                    wellenform: v[2],
                    sub: v[3],
                    schimmer: v[4],
                    filter: v[5],
                    resonanz: v[6],
                    bewegung: v[7],
                    tempo: v[8],
                };
                let mut at = 13;
                let mut curve = || -> Option<Vec<[f32; 2]>> {
                    let n = *d.get(at)? as usize;
                    let pts = d.get(at + 1..at + 1 + 2 * n)?.chunks_exact(2).map(|c| [c[0] as f32, c[1] as f32]).collect();
                    at += 1 + 2 * n;
                    Some(pts)
                };
                let (laut, hell) = (curve()?, curve()?);
                (Patch::Synth(p, Verlauf { laut, hell, ausklang: v[9], puls: v[10] as u8, pulstiefe: v[11] }), at)
            }
            _ => return None,
        })
    }
}

impl Shaping {
    pub fn encode(&self) -> [f64; 6] {
        [self.waerme, self.schwebung, self.rauheit, self.farbe, self.raum, self.breite].map(|v| v as f64)
    }

    pub fn decode(d: &[f64]) -> Option<Shaping> {
        let v = d.get(..6)?;
        Some(Shaping { waerme: v[0] as f32, schwebung: v[1] as f32, rauheit: v[2] as f32, farbe: v[3] as f32, raum: v[4] as f32, breite: v[5] as f32 })
    }
}

/// Sound shaping after the instrument, all 0…1 (color: 0.5 = neutral).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shaping {
    pub waerme: f32,
    pub schwebung: f32,
    pub rauheit: f32,
    pub farbe: f32,
    pub raum: f32,
    pub breite: f32,
}

/// What a note needs from its neighbours and from the preparation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Seed of the noise and detuning (as in the offline functions: from the
    /// note's position in the clip).
    pub seed: u32,
    /// Bass: frequency of the previous note (Hz; 0 = no glide) – it glides
    /// from there if the note follows within 30 ms (`gap`).
    pub glide_from: f32,
    /// Bass: from the end of the previous note to this start.
    pub gap: f32,
    /// Bass: the next note begins this long after the start; open hi-hat: the
    /// next hi-hat. Infinite = nothing.
    pub cut: f32,
    /// Organ: index of the wavetable in [`LiveNotes`].
    pub table: u32,
}

impl Default for Plan {
    fn default() -> Self {
        Self { seed: 0, glide_from: 0.0, gap: f32::INFINITY, cut: f32::INFINITY, table: 0 }
    }
}

/// Plans for notes given as (start, length, pitch) – in the order of
/// `notes`. Positions in any unit (seconds offline, beats live): the rules in
/// seconds are applied when a voice starts, so a plan holds for every tempo.
fn plan(patch: &Patch, notes: &[(f32, f32, f32)]) -> Vec<Plan> {
    let seeded = |f: &dyn Fn(u32) -> u32| (0..notes.len() as u32).map(|i| Plan { seed: f(i), ..Plan::default() }).collect::<Vec<_>>();
    match patch {
        Patch::Pluck(_) => seeded(&|i| i * 7919 + 17),
        Patch::Organ(_) => seeded(&|i| i + 1),
        Patch::Harpsichord(_) => seeded(&|i| i * 31 + 5),
        Patch::Synth(..) => seeded(&|i| i * 7919 + 31),
        Patch::Drums(kit) => {
            let mut plans = seeded(&|i| i * 7919 + 31);
            let hat_times: Vec<f32> = notes.iter().filter(|n| drums::is_hat(kit, n.2)).map(|n| n.0).collect();
            for (p, n) in plans.iter_mut().zip(notes) {
                if drums::voice_for(kit, n.2).0 == drums::HAT_OPEN {
                    if let Some(&next) = hat_times.iter().find(|&&t| t > n.0 + 1e-4) {
                        p.cut = next - n.0;
                    }
                }
            }
            plans
        }
        Patch::Bass(b) => {
            let mut plans = vec![Plan::default(); notes.len()];
            let mut order: Vec<usize> = (0..notes.len()).collect();
            order.sort_by(|&a, &b| notes[a].0.total_cmp(&notes[b].0));
            let mut prev: Option<(f32, f32)> = None; // (end, frequency)
            for (j, &i) in order.iter().enumerate() {
                let (start, len, midi) = notes[i];
                let freq = midi_to_hz(midi);
                if let Some(&next) = order.get(j + 1) {
                    plans[i].cut = notes[next].0 - start;
                }
                if let Some((end, f)) = prev {
                    plans[i].gap = start - end;
                    if b.glide > 0.01 {
                        plans[i].glide_from = f;
                    }
                }
                prev = Some((start + len, freq));
            }
            plans
        }
    }
}

/// Wavetables of an organ note (fundamental and overtones, normalized together).
#[derive(Clone, Debug)]
pub struct OrganTable {
    pub low: Arc<[f32]>,
    pub high: Arc<[f32]>,
}

/// Frequency an organ pipe sounds at for a note (its own small mistuning included).
fn organ_freq(n: &NoteSpec, sr: f32) -> f32 {
    let cents = ((n.midi * 7.31).sin() * 1.8) / 100.0;
    midi_to_hz(n.midi + n.pitch_offset(0.0) + cents).clamp(15.0, sr * 0.45)
}

fn organ_tables(partials: &[(f32, f32)], freq: f32, sr: f32) -> OrganTable {
    let low = organ_table(partials, freq, sr, |r| r <= 2.0);
    let high = organ_table(partials, freq, sr, |r| r > 2.0);
    let peak = low.iter().zip(&high).fold(0f32, |m, (a, b)| m.max((a + b).abs())).max(1e-6);
    OrganTable { low: low.iter().map(|v| v / peak).collect::<Vec<f32>>().into(), high: high.iter().map(|v| v / peak).collect::<Vec<f32>>().into() }
}

/// A note of a clip: position and length in beats, pitch and expression.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveNote {
    pub start: f64,
    pub len: f64,
    /// May be fractional (transposition by cents).
    pub midi: f32,
    pub velocity: f32,
    pub vibrato: f32,
    pub brightness: f32,
    pub bend: f32,
    pub plan: Plan,
}

impl LiveNote {
    pub fn new(start: f64, len: f64, midi: f32, velocity: f32) -> Self {
        Self { start, len, midi, velocity, vibrato: 0.0, brightness: 0.0, bend: 0.0, plan: Plan::default() }
    }

    /// The note to play at `spb` seconds per beat.
    pub fn spec(&self, spb: f64) -> NoteSpec {
        NoteSpec {
            start_secs: (self.start * spb) as f32,
            dur_secs: (self.len * spb) as f32,
            midi: self.midi,
            velocity: self.velocity,
            vibrato: self.vibrato,
            brightness: self.brightness,
            bend: self.bend,
        }
    }
}

/// A note clip prepared for playing.
#[derive(Debug)]
pub struct LiveNotes {
    pub patch: Patch,
    pub shaping: Shaping,
    /// Sorted by start.
    pub notes: Vec<LiveNote>,
    /// Length of the clip in beats.
    pub len_beats: f64,
    /// Longest note (beats).
    pub max_len: f64,
    organ: Vec<OrganTable>,
    /// Organ without stops: silent.
    silent: bool,
}

impl LiveNotes {
    /// Prepares the notes of a clip `len_beats` long for output rate `sr`:
    /// notes from its end on are dropped, longer ones shortened. Holds for
    /// every tempo.
    pub fn new(patch: Patch, shaping: Shaping, notes: &[LiveNote], len_beats: f64, sr: f32) -> Self {
        let mut notes: Vec<LiveNote> =
            notes.iter().filter(|n| n.start < len_beats).map(|n| LiveNote { len: n.len.min(len_beats - n.start), ..*n }).collect();
        let positions: Vec<(f32, f32, f32)> = notes.iter().map(|n| (n.start as f32, n.len as f32, n.midi)).collect();
        for (n, p) in notes.iter_mut().zip(plan(&patch, &positions)) {
            n.plan = p;
        }
        let mut organ = Vec::new();
        if let Patch::Organ(p) = &patch {
            let partials = organ_partials(p);
            let mut index: HashMap<u32, u32> = HashMap::new();
            for n in notes.iter_mut().filter(|_| !partials.is_empty()) {
                // the pitch at the start does not depend on the tempo
                let freq = organ_freq(&n.spec(1.0), sr);
                n.plan.table = *index.entry(freq.to_bits()).or_insert_with(|| {
                    organ.push(organ_tables(&partials, freq, sr));
                    organ.len() as u32 - 1
                });
            }
        }
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        Self::from_parts(patch, shaping, notes, len_beats, organ)
    }

    /// From already prepared parts (notes with plans, sorted by start; organ
    /// tables) – e.g. after the trip to the AudioWorklet.
    pub fn from_parts(patch: Patch, shaping: Shaping, notes: Vec<LiveNote>, len_beats: f64, organ: Vec<OrganTable>) -> Self {
        let silent = matches!(&patch, Patch::Organ(p) if organ_partials(p).is_empty());
        let max_len = notes.iter().map(|n| n.len).fold(0.0, f64::max);
        Self { patch, shaping, notes, len_beats, max_len, organ, silent }
    }

    /// Wavetables of the organ (see [`Plan::table`]).
    pub fn tables(&self) -> &[OrganTable] {
        &self.organ
    }

    /// Starts note `k` (`spb` = current seconds per beat). `limit` = samples
    /// from the note start on which nothing may sound any more (end of the
    /// clip's sound). `None` if the note stays silent.
    pub fn start_voice(&self, k: usize, spb: f64, sr: f32, limit: usize, arena: &mut [f32]) -> Option<Voice> {
        let n = self.notes.get(k)?;
        if self.silent || limit == 0 {
            return None;
        }
        let table = self.organ.get(n.plan.table as usize);
        Some(Voice::start(&self.patch, table, &n.spec(spb), &n.plan, spb as f32, spb as f32, sr, limit, arena))
    }

    /// Renders voice `v` (started from this clip) on: see [`Voice::render`].
    pub fn render_voice(&self, v: &mut Voice, arena: &mut [f32], out: &mut [f32]) -> usize {
        v.render(&self.patch, self.organ.get(v.table as usize), arena, out)
    }
}

// ---------------------------------------------------------------- Voices

/// Floats of an arena slot (delay lines of plucked strings) at rate `sr`.
pub fn arena_len(sr: f32) -> usize {
    ARENA_LINES * line_capacity(sr)
}

/// Strings per voice: the harpsichord has two choirs of two planes of vibration.
const ARENA_LINES: usize = 4;

/// Longest delay line: the string sounds at least at 20 Hz.
fn line_capacity(sr: f32) -> usize {
    (sr / 20.0).ceil() as usize + 8
}

/// One sounding note.
pub struct Voice {
    /// Sample since the note start.
    i: usize,
    /// From here on nothing sounds (natural end or end of the clip's sound).
    end: usize,
    /// Organ table index.
    table: u32,
    kind: Kind,
}

enum Kind {
    Pluck(Pluck),
    Harps(Harps),
    Bass(Bass),
    Organ(Organ),
    Pad(Pad),
    Drum(Drum),
}

impl Voice {
    /// Starts note `n` of `patch`. `unit_secs` = seconds per unit of
    /// `plan.cut`; `spb` = seconds per beat (pulse of the synth); `limit` =
    /// samples until the end of the clip's sound; `arena` = a slot of
    /// [`arena_len`] floats if the patch [uses one](Patch::uses_arena).
    #[allow(clippy::too_many_arguments)]
    pub fn start(patch: &Patch, organ: Option<&OrganTable>, n: &NoteSpec, plan: &Plan, unit_secs: f32, spb: f32, sr: f32, limit: usize, arena: &mut [f32]) -> Voice {
        let cap = line_capacity(sr);
        let cut_secs = plan.cut * unit_secs;
        let (len, kind) = match patch {
            Patch::Pluck(p) => {
                let v = Pluck::start(n, sr, p, plan.seed, arena, 0, cap);
                (v.total, Kind::Pluck(v))
            }
            Patch::Harpsichord(p) => {
                let v = Harps::start(n, sr, p, plan.seed, arena, cap);
                (v.len, Kind::Harps(v))
            }
            Patch::Bass(p) => {
                let dur = n.dur_secs.min(cut_secs.max(0.02));
                // tied notes (the next one at most 30 ms after the end of the previous one) glide
                let legato = plan.gap * unit_secs <= 0.03;
                let from = (legato && plan.glide_from > 0.0).then_some(plan.glide_from);
                let v = Bass::start(midi_to_hz(n.midi), from, dur, n, sr, p);
                (v.n, Kind::Bass(v))
            }
            Patch::Organ(p) => {
                let v = Organ::start(n, sr, p, plan.seed, organ.is_some());
                (v.total, Kind::Organ(v))
            }
            Patch::Synth(p, v) => {
                let pad = Pad::start(n, sr, p, v, spb, plan.seed, limit);
                (pad.len, Kind::Pad(pad))
            }
            Patch::Drums(kit) => {
                let d = Drum::start(kit, n, sr, plan.seed, cut_secs);
                (d.n, Kind::Drum(d))
            }
        };
        Voice { i: 0, end: len.min(limit), table: plan.table, kind }
    }

    /// Jumps `offset` samples into the note (playback starts in its middle):
    /// envelopes continue at the right place; plucked strings lose as much
    /// energy as they would have meanwhile.
    pub fn skip(&mut self, offset: usize, arena: &mut [f32]) {
        if offset == 0 {
            return;
        }
        match &mut self.kind {
            Kind::Pluck(p) => p.decay_by(offset, arena),
            Kind::Harps(h) => {
                if let Some(p) = h.p8.as_mut() {
                    p.decay_by(offset, arena);
                }
                if let Some(p) = h.p4.as_mut() {
                    p.decay_by(offset, arena);
                }
            }
            _ => {}
        }
        self.i = offset;
    }

    /// Whether the voice has ended.
    pub fn done(&self) -> bool {
        self.i >= self.end
    }

    /// Samples since the note start.
    pub fn position(&self) -> usize {
        self.i
    }

    /// Writes the next samples into `out` (overwriting) and returns how many –
    /// fewer than `out.len()` means the voice has ended.
    pub fn render(&mut self, patch: &Patch, organ: Option<&OrganTable>, arena: &mut [f32], out: &mut [f32]) -> usize {
        let n = out.len().min(self.end.saturating_sub(self.i));
        let i0 = self.i;
        let out = &mut out[..n];
        match (&mut self.kind, patch) {
            (Kind::Pluck(v), _) => {
                for (j, o) in out.iter_mut().enumerate() {
                    *o = v.sample(i0 + j, arena);
                }
            }
            (Kind::Harps(v), _) => {
                for (j, o) in out.iter_mut().enumerate() {
                    *o = v.sample(i0 + j, arena);
                }
            }
            (Kind::Bass(v), _) => {
                for (j, o) in out.iter_mut().enumerate() {
                    *o = v.sample(i0 + j);
                }
            }
            (Kind::Organ(v), Patch::Organ(p)) => match organ {
                Some(t) => {
                    for (j, o) in out.iter_mut().enumerate() {
                        *o = v.sample(i0 + j, p, t);
                    }
                }
                None => out.fill(0.0),
            },
            (Kind::Pad(v), Patch::Synth(p, verlauf)) => {
                for (j, o) in out.iter_mut().enumerate() {
                    *o = v.sample(i0 + j, p, verlauf);
                }
            }
            (Kind::Drum(v), _) => {
                for (j, o) in out.iter_mut().enumerate() {
                    *o = v.sample(i0 + j);
                }
            }
            _ => out.fill(0.0),
        }
        self.i += n;
        n
    }
}

/// A vibrating string (Karplus-Strong): delay line in an arena with
/// all-pass fine tuning and loop filter.
struct Saite {
    off: usize,
    size: usize,
    w: usize,
    /// Loop filter: weight of the previous sample (brightness).
    s: f32,
    prev: f32,
    ap_x1: f32,
    ap_y1: f32,
    len: usize,
    ap: f32,
    g: f32,
}

impl Saite {
    fn new(off: usize, size: usize, exc_len: usize, s: f32) -> Self {
        Self { off, size, w: exc_len.min(size) % size, s, prev: 0.0, ap_x1: 0.0, ap_y1: 0.0, len: 2, ap: 0.0, g: 0.0 }
    }

    /// Sets the period (samples, incl. delay of the loop filter) and decay time.
    fn tune(&mut self, period_total: f32, t60: f32, freq: f32) {
        let p = (period_total - self.s).clamp(2.5, (self.size - 3) as f32);
        let len = (p - 0.5).floor().max(1.0);
        let d = p - len;
        self.len = len as usize;
        self.ap = (1.0 - d) / (1.0 + d);
        self.g = 10f32.powf(-3.0 / (t60 * freq));
    }

    #[inline]
    fn step(&mut self, a: &mut [f32]) -> f32 {
        let size = self.size;
        let x = a[self.off + (self.w + size - self.len) % size];
        let y = self.ap * x + self.ap_x1 - self.ap * self.ap_y1;
        self.ap_x1 = x;
        self.ap_y1 = y;
        let out = self.g * ((1.0 - self.s) * y + self.s * self.prev);
        self.prev = y;
        a[self.off + self.w] = out;
        self.w = (self.w + 1) % size;
        out
    }

    /// Scales what vibrates in the string.
    fn scale(&mut self, a: &mut [f32], k: f32) {
        for v in &mut a[self.off..self.off + self.size] {
            *v *= k;
        }
        self.prev *= k;
        self.ap_x1 *= k;
        self.ap_y1 *= k;
    }
}

/// Plucked note (see `synth::pluck_note`).
struct Pluck {
    a: Saite,
    b: Saite,
    res: [(Biquad, f32); 4],
    body: f32,
    detune: f32,
    t60: f32,
    freq: f32,
    total: usize,
    note_end: usize,
    rel_len: usize,
    fade_in: usize,
    modulated: bool,
    note: NoteSpec,
    velocity: f32,
    sr: f32,
}

impl Pluck {
    /// Strings in lines `line` and `line + 1` of the arena.
    fn start(n: &NoteSpec, sr: f32, p: &PluckParams, seed: u32, arena: &mut [f32], line: usize, cap: usize) -> Pluck {
        let release = 0.08;
        let duration_secs = n.dur_secs;
        let velocity = n.velocity;
        let total = ((duration_secs.max(0.01) + release) * sr) as usize;
        let freq_of = |offset: f32| (midi_to_hz(n.midi + offset)).clamp(20.0, sr * 0.45);
        let freq = freq_of(n.pitch_offset(0.0));

        let register = ((n.midi - 60.0) / 36.0).clamp(-0.5, 1.0);
        let s = (0.5 - 0.45 * (p.brightness + 0.45 * n.brightness + 0.12 * register).clamp(0.0, 1.0)).max(0.03);
        let t60 = 0.25 + 7.0 * p.decay.clamp(0.0, 1.0).powf(1.5);

        let longest = sr / freq_of(n.lowest_offset().min(n.pitch_offset(0.0)));
        let size = (longest.ceil() as usize + 4).min(cap);
        let period0 = sr / freq;

        // Excitation: noise first into the second line, the plucked shape into the first
        let (a_off, b_off) = (line * cap, (line + 1) * cap);
        let mut rng = Rng(seed.wrapping_mul(2_654_435_761) | 1);
        let hardness = (p.attack + 0.3 * n.brightness + 0.25 * (velocity - 0.8)).clamp(0.0, 1.0);
        let a = 0.15 + 0.85 * hardness;
        let pos = 0.27 - 0.15 * hardness;
        let exc_len = (period0.round() as usize).clamp(2, size);
        let comb = ((exc_len as f32 * pos).round() as usize).max(1);
        let mut lp = 0.0;
        for k in 0..exc_len {
            lp += a * (rng.next() - lp);
            arena[b_off + k] = lp;
        }
        let noise_amt = 0.3 + 0.9 * hardness + 0.6 * p.brightness.clamp(0.0, 1.0);
        for i in 0..exc_len {
            let x = i as f32 / exc_len as f32;
            let tri = if x < pos { x / pos } else { (1.0 - x) / (1.0 - pos) };
            let nz = arena[b_off + i] - if i >= comb { arena[b_off + i - comb] } else { 0.0 };
            arena[a_off + i] = tri + noise_amt * nz;
        }
        let exc = a_off..a_off + exc_len;
        let mean = arena[exc.clone()].iter().sum::<f32>() / exc_len as f32;
        let peak = arena[exc.clone()].iter().fold(0.0f32, |m, v| m.max((v - mean).abs())).max(1e-6);
        for v in &mut arena[exc] {
            *v = (*v - mean) / peak;
        }
        arena[a_off + exc_len..a_off + size].fill(0.0);
        arena.copy_within(a_off..a_off + size, b_off);

        let detune = 1.0 + 0.0007 + 0.0004 * (seed % 7) as f32 / 7.0;
        let mut a_str = Saite::new(a_off, size, exc_len, s);
        let mut b_str = Saite::new(b_off, size, exc_len, s);
        a_str.tune(period0, t60, freq);
        b_str.tune(period0 / detune, t60 * 0.8, freq * detune);

        let body = p.body.clamp(0.0, 1.0);
        let res = [
            (Biquad::bandpass(110.0, 1.3, sr), 1.0f32),
            (Biquad::bandpass(200.0, 1.3, sr), 0.9),
            (Biquad::bandpass(400.0, 1.4, sr), 0.55),
            (Biquad::bandpass(2600.0, 1.0, sr), 0.25),
        ];
        Pluck {
            a: a_str,
            b: b_str,
            res,
            body,
            detune,
            t60,
            freq,
            total,
            note_end: (duration_secs * sr) as usize,
            rel_len: (release * sr) as usize,
            fade_in: (0.0015 * sr) as usize,
            modulated: n.modulated(),
            note: *n,
            velocity: velocity.clamp(0.0, 1.0),
            sr,
        }
    }

    /// The strings lose what they would have lost in `samples` samples.
    fn decay_by(&mut self, samples: usize, arena: &mut [f32]) {
        let periods = samples as f32 * self.freq / self.sr;
        let (ga, gb) = (self.a.g.powf(periods), self.b.g.powf(periods * self.detune));
        self.a.scale(arena, ga);
        self.b.scale(arena, gb);
    }

    #[inline]
    fn sample(&mut self, i: usize, arena: &mut [f32]) -> f32 {
        let sr = self.sr;
        if self.modulated && i % 8 == 0 {
            let f = midi_to_hz(self.note.midi + self.note.pitch_offset(i as f32 / sr)).clamp(20.0, sr * 0.45);
            self.a.tune(sr / f, self.t60, f);
            self.b.tune(sr / (f * self.detune), self.t60 * 0.8, f * self.detune);
        }
        let y = 0.62 * self.a.step(arena) + 0.38 * self.b.step(arena);
        let resonance: f32 = self.res.iter_mut().map(|(bq, g)| *g * bq.process(y)).sum();
        let mut v = y * (1.0 - 0.3 * self.body) + self.body * 0.9 * resonance;
        if i < self.fade_in {
            v *= i as f32 / self.fade_in as f32;
        }
        if i >= self.note_end {
            let k = (i - self.note_end) as f32 / self.rel_len.max(1) as f32;
            v *= (1.0 - k).max(0.0).powi(2);
        }
        v * 0.38 * self.velocity
    }
}

/// Harpsichord note (see `keys::harpsichord_note`): 8′ and 4′ choir, damper noise.
struct Harps {
    p8: Option<Pluck>,
    p4: Option<Pluck>,
    achtfuss: f32,
    gain4: f32,
    thump: (Biquad, Rng),
    end: usize,
    thump_len: usize,
    len: usize,
}

impl Harps {
    fn start(n: &NoteSpec, sr: f32, p: &HarpsichordParams, seed: u32, arena: &mut [f32], cap: usize) -> Harps {
        let pp = PluckParams {
            brightness: (0.58 + 0.28 * p.kiel - 0.5 * p.laute).clamp(0.0, 1.0),
            decay: (0.25 + 0.6 * p.nachklang) * (1.0 - 0.75 * p.laute),
            body: 0.15,
            attack: (0.55 + 0.35 * p.kiel - 0.4 * p.laute).clamp(0.0, 1.0),
        };
        let vel = 0.82 + 0.1 * n.velocity.clamp(0.0, 1.0);
        let base = NoteSpec { velocity: vel, ..*n };
        let p8 = (p.achtfuss > 0.0).then(|| Pluck::start(&base, sr, &pp, seed, arena, 0, cap));
        let p4 = (p.vierfuss > 0.0).then(|| {
            let four = NoteSpec { midi: n.midi + 12.03, ..base };
            Pluck::start(&four, sr, &PluckParams { brightness: (pp.brightness + 0.1).min(1.0), ..pp }, seed.wrapping_add(7919), arena, 2, cap)
        });
        let len = p8.as_ref().map(|v| v.total).max(p4.as_ref().map(|v| v.total)).unwrap_or(0);
        Harps {
            p8,
            p4,
            achtfuss: p.achtfuss,
            gain4: 0.8 * p.vierfuss,
            thump: (Biquad::bandpass(midi_to_hz(n.midi).clamp(80.0, 2000.0) * 1.5, 1.5, sr), Rng(seed.wrapping_mul(2_891_336_453) | 1)),
            end: (n.dur_secs * sr) as usize,
            thump_len: (0.03 * sr) as usize,
            len,
        }
    }

    #[inline]
    fn sample(&mut self, i: usize, arena: &mut [f32]) -> f32 {
        let mut y = match self.p8.as_mut() {
            Some(p) if i < p.total => p.sample(i, arena) * self.achtfuss,
            _ => 0.0,
        };
        if let Some(p) = self.p4.as_mut() {
            if i < p.total {
                y += p.sample(i, arena) * self.gain4;
            }
        }
        if i >= self.end && i < self.end + self.thump_len {
            let k = i - self.end;
            let (bp, rng) = &mut self.thump;
            y += bp.process(rng.next()) * 0.025 * (1.0 - k as f32 / self.thump_len as f32);
        }
        y
    }
}

/// Bass note (see `bass::voice`).
struct Bass {
    freq: f32,
    from_hz: Option<f32>,
    note: NoteSpec,
    modulated: bool,
    velocity: f32,
    n: usize,
    note_end: usize,
    release: f32,
    w: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    base_cut: f32,
    env_amt: f32,
    f_decay: f32,
    k: f32,
    drive: f32,
    tau: f32,
    sub: f32,
    ph: f32,
    sub_ph: f32,
    f1: Svf,
    f2: Svf,
    g: f32,
    sr: f32,
}

impl Bass {
    fn start(freq: f32, from_hz: Option<f32>, dur: f32, note: &NoteSpec, sr: f32, p: &BassParams) -> Bass {
        let velocity = note.velocity;
        let release = 0.04;
        let anschlag = p.anschlag.clamp(0.0, 1.0);
        let abklingen = p.abklingen.clamp(0.0, 1.0);
        Bass {
            freq,
            from_hz,
            note: *note,
            modulated: note.modulated(),
            velocity,
            n: ((dur.max(0.02) + release) * sr) as usize,
            note_end: (dur * sr) as usize,
            release,
            w: p.wellenform.clamp(0.0, 1.0),
            attack: 0.002 + 0.02 * (1.0 - anschlag),
            decay: 0.05 + 0.8 * abklingen,
            sustain: 0.3 + 0.6 * abklingen,
            base_cut: 40.0 * 2f32.powf(p.filter.clamp(0.0, 1.0) * 8.0 + 1.5 * note.brightness.clamp(-1.0, 1.0)),
            env_amt: anschlag * 4.0 * (0.55 + 0.5 * velocity.clamp(0.0, 1.0)),
            f_decay: 0.05 + 0.6 * abklingen,
            k: 2.0 - 1.9 * p.resonanz.clamp(0.0, 1.0),
            drive: 1.0 + 8.0 * p.biss.clamp(0.0, 1.0),
            tau: 0.005 + 0.12 * p.glide.clamp(0.0, 1.0),
            sub: p.sub.clamp(0.0, 1.0),
            ph: 0.0,
            sub_ph: 0.0,
            f1: Svf::default(),
            f2: Svf::default(),
            g: 0.0,
            sr,
        }
    }

    #[inline]
    fn sample(&mut self, i: usize) -> f32 {
        let sr = self.sr;
        let t = i as f32 / sr;
        let mut f = match self.from_hz {
            Some(f0) => self.freq + (f0 - self.freq) * (-t / self.tau).exp(),
            None => self.freq,
        };
        if self.modulated {
            f *= 2f32.powf(self.note.pitch_offset(t) / 12.0);
        }
        let dt = (f / sr).min(0.45);
        self.ph += dt;
        if self.ph >= 1.0 {
            self.ph -= 1.0;
        }
        self.sub_ph += dt * 0.5;
        if self.sub_ph >= 1.0 {
            self.sub_ph -= 1.0;
        }
        let ph = self.ph;
        let w = self.w;
        let sine = (2.0 * PI * ph).sin();
        let saw = 2.0 * ph - 1.0 - poly_blep(ph, dt);
        let square = (if ph < 0.5 { 1.0 } else { -1.0 }) + poly_blep(ph, dt) - poly_blep((ph + 0.5) % 1.0, dt);
        let osc = if w < 0.5 {
            let a = w / 0.5;
            (1.0 - a) * sine + a * saw
        } else {
            let b = (w - 0.5) / 0.5;
            (1.0 - b) * saw + b * square * 0.8
        };
        let sub = (2.0 * PI * self.sub_ph).sin() * self.sub * 0.8;
        if i % 16 == 0 {
            let cut = (self.base_cut * 2f32.powf(self.env_amt * (-t / self.f_decay).exp())).min(sr * 0.45);
            self.g = (PI * cut / sr).tan();
        }
        let filtered = self.f2.lowpass(self.f1.lowpass(osc, self.g, self.k), self.g, 1.414);
        let amp = if t < self.attack { t / self.attack } else { self.sustain + (1.0 - self.sustain) * (-(t - self.attack) / self.decay).exp() };
        let rel = if i >= self.note_end { (1.0 - (i - self.note_end) as f32 / (self.release * sr)).max(0.0) } else { 1.0 };
        let y = ((filtered + sub) * self.drive).tanh() / self.drive.tanh();
        y * amp * rel * 0.5 * self.velocity
    }
}

/// Organ note (see `keys::organ_note`); the wavetables come prepared.
struct Organ {
    note: NoteSpec,
    total: usize,
    note_end: usize,
    rel_n: usize,
    phase: f32,
    rng: Rng,
    chiff_bp: Biquad,
    wind_bp: Biquad,
    att_low: usize,
    att_high: usize,
    modulated: bool,
    inc: f32,
    level: f32,
    sr: f32,
}

impl Organ {
    fn start(n: &NoteSpec, sr: f32, _p: &OrganParams, seed: u32, sounding: bool) -> Organ {
        let freq = organ_freq(n, sr);
        let release = 0.14;
        let total = if sounding { ((n.dur_secs.max(0.02) + release) * sr) as usize } else { 0 };
        let attack = (0.035 + 0.03 * ((60.0 - n.midi) / 24.0)).clamp(0.015, 0.09);
        Organ {
            note: *n,
            total,
            note_end: (n.dur_secs * sr) as usize,
            rel_n: (release * sr) as usize,
            phase: 0.0,
            rng: Rng(seed.wrapping_mul(747_796_405) | 1),
            chiff_bp: Biquad::bandpass((freq * 4.0).min(7000.0), 2.5, sr),
            wind_bp: Biquad::bandpass((freq * 2.5).clamp(300.0, 6000.0), 1.2, sr),
            att_low: ((attack * 1.35) * sr) as usize,
            att_high: ((attack * 0.55) * sr) as usize,
            modulated: n.modulated(),
            inc: freq / sr * TABLE as f32 * 0.5,
            level: 0.16 * (0.75 + 0.25 * n.velocity.clamp(0.0, 1.0)),
            sr,
        }
    }

    #[inline]
    fn sample(&mut self, i: usize, p: &OrganParams, tab: &OrganTable) -> f32 {
        let sr = self.sr;
        let t = i as f32 / sr;
        if self.modulated && i % 16 == 0 {
            let f = midi_to_hz(self.note.midi + self.note.pitch_offset(t)).clamp(15.0, sr * 0.45);
            self.inc = f / sr * TABLE as f32 * 0.5;
        }
        let trem = 1.0 + 0.12 * p.tremulant * (2.0 * PI * 5.6 * t).sin();
        self.phase += self.inc * (1.0 + 0.002 * p.tremulant * (2.0 * PI * 5.6 * t).sin());
        if self.phase >= TABLE as f32 {
            self.phase -= TABLE as f32;
        }
        let idx = self.phase as usize;
        let frac = self.phase - idx as f32;
        let lerp = |t: &[f32]| t[idx] * (1.0 - frac) + t[idx + 1] * frac;
        let rise = |len: usize| {
            if i >= len {
                return 1.0;
            }
            let x = i as f32 / len.max(1) as f32;
            x * x * (3.0 - 2.0 * x)
        };
        let fall = if i >= self.note_end { (1.0 - (i - self.note_end) as f32 / self.rel_n.max(1) as f32).max(0.0) } else { 1.0 };
        let s = lerp(&tab.low) * rise(self.att_low) + lerp(&tab.high) * rise(self.att_high);
        let noise = self.rng.next();
        let chiff = if t < 0.06 { self.chiff_bp.process(noise) * p.chiff * 0.9 * (1.0 - t / 0.06) } else { 0.0 };
        let wind = self.wind_bp.process(noise) * (0.012 + 0.02 * p.chiff) * rise(self.att_low);
        ((s + wind) * fall * trem + chiff) * self.level
    }
}

const MAX_UNISON: usize = 7;

/// Synth note (see `pad::voice`).
struct Pad {
    note: NoteSpec,
    freq: f32,
    held: f32,
    len: usize,
    u: usize,
    ratios: [f32; MAX_UNISON],
    phases: [f32; MAX_UNISON],
    sub_ph: f32,
    shim_ph: f32,
    w: f32,
    norm: f32,
    k_res: f32,
    lfo_rate: f32,
    lfo_phase: f32,
    period: Option<f32>,
    velocity: f32,
    modulated: bool,
    f1: Svf,
    f2: Svf,
    g: f32,
    sr: f32,
}

impl Pad {
    fn start(n: &NoteSpec, sr: f32, p: &SynthParams, v: &Verlauf, spb: f32, seed: u32, limit: usize) -> Pad {
        let held = n.dur_secs.max(0.02);
        let len = (((held + v.ausklang.max(0.02)) * sr) as usize).min(limit);
        let u = 1 + (p.stimmen.clamp(0.0, 1.0) * 6.0).round() as usize;
        let spread = p.verstimmung.clamp(0.0, 1.0) * 30.0;
        let mut rng = Rng(seed.wrapping_mul(2_654_435_761) | 1);
        let mut ratios = [1.0f32; MAX_UNISON];
        for (k, r) in ratios.iter_mut().enumerate().take(u) {
            let cents = if u == 1 { 0.0 } else { (k as f32 / (u - 1) as f32 * 2.0 - 1.0) * spread };
            *r = 2f32.powf(cents / 1200.0);
        }
        let mut phases = [0.0f32; MAX_UNISON];
        for ph in phases.iter_mut().take(u) {
            *ph = rng.next().abs();
        }
        let shim_ph = rng.next().abs();
        let lfo_rate = 0.05 + 5.95 * p.tempo.clamp(0.0, 1.0).powi(2);
        let lfo_phase = rng.next().abs() * 2.0 * PI;
        Pad {
            note: *n,
            freq: midi_to_hz(n.midi),
            held,
            len,
            u,
            ratios,
            phases,
            sub_ph: 0.0,
            shim_ph,
            w: p.wellenform.clamp(0.0, 1.0),
            norm: 0.62 / (u as f32).sqrt(),
            k_res: 2.0 - 1.85 * p.resonanz.clamp(0.0, 1.0),
            lfo_rate,
            lfo_phase,
            period: puls_period(v.puls, spb),
            velocity: n.velocity.clamp(0.0, 1.0),
            modulated: n.modulated(),
            f1: Svf::default(),
            f2: Svf::default(),
            g: 0.0,
            sr,
        }
    }

    #[inline]
    fn sample(&mut self, i: usize, p: &SynthParams, v: &Verlauf) -> f32 {
        let sr = self.sr;
        let n = &self.note;
        let t = i as f32 / sr;
        if i % 16 == 0 {
            let hb = curve_at(&v.hell, t.min(self.held));
            let lfo = p.bewegung.clamp(0.0, 1.0) * 1.4 * (2.0 * PI * self.lfo_rate * t + self.lfo_phase).sin();
            let oct = 4.3 + 5.2 * p.filter.clamp(0.0, 1.0) + 6.0 * (hb - 0.5) + 1.2 * n.brightness.clamp(-1.0, 1.0) + lfo;
            let cut = (20.0 * 2f32.powf(oct)).clamp(30.0, sr * 0.45);
            self.g = (PI * cut / sr).tan();
        }
        let f = if self.modulated { self.freq * 2f32.powf(n.pitch_offset(t) / 12.0) } else { self.freq };
        let w = self.w;
        let mut sum = 0.0;
        for (ph, r) in self.phases.iter_mut().zip(&self.ratios).take(self.u) {
            let dt = (f * r / sr).min(0.45);
            *ph += dt;
            if *ph >= 1.0 {
                *ph -= 1.0;
            }
            let saw = 2.0 * *ph - 1.0 - poly_blep(*ph, dt);
            sum += if w < 0.5 {
                let a = w / 0.5;
                (1.0 - a) * (2.0 * PI * *ph).sin() + a * saw
            } else {
                let b = (w - 0.5) / 0.5;
                let square = (if *ph < 0.5 { 1.0 } else { -1.0 }) + poly_blep(*ph, dt) - poly_blep((*ph + 0.5) % 1.0, dt);
                (1.0 - b) * saw + b * 0.8 * square
            };
        }
        let dt = f / sr;
        self.sub_ph = (self.sub_ph + 0.5 * dt) % 1.0;
        let sdt = (2.0 * dt).min(0.45);
        self.shim_ph += sdt;
        if self.shim_ph >= 1.0 {
            self.shim_ph -= 1.0;
        }
        let shimmer = 2.0 * self.shim_ph - 1.0 - poly_blep(self.shim_ph, sdt);
        let x = sum * self.norm + p.sub.clamp(0.0, 1.0) * 0.55 * (2.0 * PI * self.sub_ph).sin() + p.schimmer.clamp(0.0, 1.0) * 0.3 * shimmer;
        let y = self.f2.lowpass(self.f1.lowpass(x, self.g, self.k_res), self.g, 1.414);
        let pulse = self.period.map(|per| gate((t % per) / per, v.pulstiefe)).unwrap_or(1.0);
        let edge = (t / 0.002).min(1.0) * ((self.len - i) as f32 / (0.003 * sr)).min(1.0);
        y * level_at(v, t, self.held) * pulse * edge * self.velocity * 0.3
    }
}

/// One drum hit (see `drums::render_note`), an open hi-hat choked by the next one.
struct Drum {
    piece: Piece,
    n: usize,
    velocity: f32,
    /// Choke: from sample `.0` on it fades out over `.1` samples.
    choke: Option<(usize, usize)>,
    sr: f32,
}

enum Piece {
    Kick { f_end: f32, f_start: f32, sweep: f32, decay: f32, drive: f32, klick: f32, rng: Rng, hp: Biquad, phase: f32 },
    Snare { f: f32, teppich: f32, tone_decay: f32, noise_decay: f32, rng: Rng, bp: Biquad, phase: f32 },
    Clap { tail: f32, rng: Rng, bp: Biquad },
    Hat { len: f32, rng: Rng, hp: Biquad, m1: Biquad, m2: Biquad },
    Tom { f: f32, decay: f32, rng: Rng, lp: Biquad, phase: f32 },
    Rim { f1: f32, f2: f32, rng: Rng, hp: Biquad },
    Cowbell { bp: Biquad },
    Cymbal { crash: bool, len: f32, noise_amt: f32, rng: Rng, hp: Biquad, ping: Biquad },
    Shaker { len: f32, rng: Rng, bp: Biquad },
    None,
}

const CYMBAL_PARTIALS: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

impl Drum {
    fn start(kit: &DrumKit, n: &NoteSpec, sr: f32, seed: u32, cut_secs: f32) -> Drum {
        // the drum on the key, or a tuned drum at the note's pitch
        let (midi, mut k) = drums::voice_for(kit, n.midi);
        drums::brighten(&mut k, n.brightness);
        let rng = Rng(seed | 1);
        let (piece, n_len) = match midi {
            drums::KICK => {
                let p = &k.kick;
                let f_end = midi_to_hz(p.grundton);
                let punch = p.punch.clamp(0.0, 1.0);
                let decay = 0.12 + 1.1 * p.laenge.clamp(0.0, 1.0);
                let piece = Piece::Kick {
                    f_end,
                    f_start: f_end * (1.5 + 4.0 * punch),
                    sweep: 30.0 + 50.0 * punch,
                    decay,
                    drive: 1.0 + 4.0 * p.waerme.clamp(0.0, 1.0),
                    klick: p.klick.clamp(0.0, 1.0),
                    rng,
                    hp: Biquad::highpass(2500.0, 0.7, sr),
                    phase: 0.0,
                };
                (piece, ((decay + 0.02) * sr) as usize)
            }
            drums::SNARE => {
                let p = &k.snare;
                let laenge = p.laenge.clamp(0.0, 1.0);
                let piece = Piece::Snare {
                    f: midi_to_hz(p.kessel),
                    teppich: p.teppich.clamp(0.0, 1.0),
                    tone_decay: 30.0 - 15.0 * laenge,
                    noise_decay: 30.0 - 22.0 * laenge,
                    rng,
                    bp: Biquad::bandpass(1500.0 + 5000.0 * p.schaerfe.clamp(0.0, 1.0), 0.8, sr),
                    phase: 0.0,
                };
                (piece, ((0.12 + 0.5 * laenge) * sr) as usize)
            }
            drums::CLAP => {
                let p = &k.clap;
                let laenge = p.laenge.clamp(0.0, 1.0);
                let piece = Piece::Clap { tail: 18.0 - 12.0 * laenge, rng, bp: Biquad::bandpass(900.0 + 2200.0 * p.helligkeit.clamp(0.0, 1.0), 1.2, sr) };
                (piece, ((0.1 + 0.4 * laenge) * sr) as usize)
            }
            drums::HAT_CLOSED | drums::HAT_OPEN => {
                let p = &k.hat;
                let open = midi == drums::HAT_OPEN;
                let len = if open { 0.15 + 0.7 * p.offen.clamp(0.0, 1.0) } else { 0.03 + 0.15 * p.laenge.clamp(0.0, 1.0) };
                let h = p.helligkeit.clamp(0.0, 1.0);
                let piece = Piece::Hat {
                    len,
                    rng,
                    hp: Biquad::highpass(4500.0 + 5000.0 * h, 0.7, sr),
                    m1: Biquad::bandpass(6200.0 + 2500.0 * h, 6.0, sr),
                    m2: Biquad::bandpass(8300.0 + 2800.0 * h, 6.0, sr),
                };
                (piece, ((len + 0.01) * sr) as usize)
            }
            drums::TOM_LOW | drums::TOM_MID | drums::TOM_HIGH => {
                let p = &k.tom;
                let offset = match midi {
                    drums::TOM_LOW => 0.0,
                    drums::TOM_MID => 5.0,
                    _ => 10.0,
                };
                let decay = 0.2 + 0.8 * p.laenge.clamp(0.0, 1.0);
                let piece = Piece::Tom { f: midi_to_hz(p.grundton + offset), decay, rng, lp: Biquad::lowpass(3000.0, 0.7, sr), phase: 0.0 };
                (piece, ((decay + 0.02) * sr) as usize)
            }
            drums::RIM => {
                let p = &k.snare;
                let kk = midi_to_hz(p.kessel) / midi_to_hz(54.0);
                let piece = Piece::Rim { f1: 455.0 * kk, f2: 1670.0 * kk * (0.9 + 0.2 * p.schaerfe.clamp(0.0, 1.0)), rng, hp: Biquad::highpass(3000.0, 0.7, sr) };
                (piece, (0.07 * sr) as usize)
            }
            drums::COWBELL => (Piece::Cowbell { bp: Biquad::bandpass(900.0 + 900.0 * k.hat.helligkeit.clamp(0.0, 1.0), 1.4, sr) }, (0.45 * sr) as usize),
            drums::RIDE | drums::CRASH => {
                let p = &k.hat;
                let crash = midi == drums::CRASH;
                let h = p.helligkeit.clamp(0.0, 1.0);
                let len = if crash { 1.2 + 1.6 * p.offen.clamp(0.0, 1.0) } else { 0.8 + 1.2 * p.offen.clamp(0.0, 1.0) };
                let piece = Piece::Cymbal {
                    crash,
                    len,
                    noise_amt: if crash { 0.9 } else { 0.35 },
                    rng,
                    hp: Biquad::highpass(if crash { 3000.0 } else { 5000.0 } + 2500.0 * h, 0.7, sr),
                    ping: Biquad::bandpass(if crash { 5200.0 } else { 3600.0 } + 1500.0 * h, 3.0, sr),
                };
                (piece, (len * sr) as usize)
            }
            drums::SHAKER => {
                let p = &k.hat;
                let len = 0.06 + 0.1 * p.laenge.clamp(0.0, 1.0);
                let piece = Piece::Shaker { len, rng, bp: Biquad::bandpass(5500.0 + 3500.0 * p.helligkeit.clamp(0.0, 1.0), 1.1, sr) };
                (piece, ((len + 0.02) * sr) as usize)
            }
            _ => (Piece::None, 0),
        };
        let choke = (midi == drums::HAT_OPEN && cut_secs.is_finite()).then(|| ((cut_secs * sr) as usize, (0.008 * sr) as usize));
        Drum { piece, n: n_len, velocity: n.velocity, choke, sr }
    }

    #[inline]
    fn sample(&mut self, i: usize) -> f32 {
        let t = i as f32 / self.sr;
        let sr = self.sr;
        let velocity = self.velocity;
        let mut v = match &mut self.piece {
            Piece::Kick { f_end, f_start, sweep, decay, drive, klick, rng, hp, phase } => {
                let f = *f_end + (*f_start - *f_end) * (-t * *sweep).exp();
                *phase += 2.0 * PI * f / sr;
                let amp = (-t * 6.9 / *decay).exp();
                let body = drums::soft_clip(phase.sin() * amp, *drive);
                let click = hp.process(rng.next()) * (-t * 400.0).exp() * *klick * 0.7;
                let fade = (t * 2000.0).min(1.0);
                (body + click) * fade * 0.9 * velocity
            }
            Piece::Snare { f, teppich, tone_decay, noise_decay, rng, bp, phase } => {
                *phase += 2.0 * PI * *f * (1.0 + 0.15 * (-t * 40.0).exp()) / sr;
                let tone = phase.sin() * (-t * *tone_decay).exp() * (0.8 - 0.4 * *teppich);
                let noise = bp.process(rng.next()) * (-t * *noise_decay).exp() * *teppich * 1.8;
                (tone + noise) * velocity
            }
            Piece::Clap { tail, rng, bp } => {
                let bursts: f32 = [0.0f32, 0.011, 0.022].iter().map(|&o| if t >= o { (-(t - o) * 180.0).exp() } else { 0.0 }).sum();
                let env = bursts * 0.7 + if t >= 0.022 { (-(t - 0.022) * *tail).exp() * 0.6 } else { 0.0 };
                bp.process(rng.next()) * env * 2.2 * velocity
            }
            Piece::Hat { len, rng, hp, m1, m2 } => {
                let x = hp.process(rng.next());
                let metal = m1.process(x) + m2.process(x);
                let env = (-t * 6.9 / *len).exp();
                (x * 0.7 + metal * 0.9) * env * 0.8 * velocity
            }
            Piece::Tom { f, decay, rng, lp, phase } => {
                *phase += 2.0 * PI * *f * (1.0 + 0.35 * (-t * 25.0).exp()) / sr;
                let body = phase.sin() * (-t * 6.9 / *decay).exp();
                let stick = lp.process(rng.next()) * (-t * 120.0).exp() * 0.25;
                (body + stick) * 0.85 * velocity
            }
            Piece::Rim { f1, f2, rng, hp } => {
                let tone = (2.0 * PI * *f1 * t).sin() * (-t * 90.0).exp() * 0.6 + (2.0 * PI * *f2 * t).sin() * (-t * 140.0).exp() * 0.5;
                let click = hp.process(rng.next()) * (-t * 600.0).exp() * 0.6;
                (tone + click) * velocity
            }
            Piece::Cowbell { bp } => {
                let x = square(2.0 * PI * 540.0 * t) + square(2.0 * PI * 800.0 * t);
                let env = 0.65 * (-t * 45.0).exp() + 0.35 * (-t * 8.0).exp();
                bp.process(x) * env * 0.9 * velocity
            }
            Piece::Cymbal { crash, len, noise_amt, rng, hp, ping } => {
                let metal: f32 = CYMBAL_PARTIALS.iter().map(|&f| square(2.0 * PI * f * 2.0 * t)).sum::<f32>() / 6.0;
                let x = hp.process(metal + rng.next() * *noise_amt);
                let bell = ping.process(metal) * (-t * 12.0).exp() * if *crash { 0.3 } else { 0.9 };
                let attack = if *crash { (t * 400.0).min(1.0) } else { 1.0 };
                let env = (-t * 6.9 / *len).exp();
                (x * 0.8 + bell) * env * attack * 0.6 * velocity
            }
            Piece::Shaker { len, rng, bp } => {
                let env = (t / 0.012).min(1.0) * (-(t - 0.012).max(0.0) * 6.9 / *len).exp();
                bp.process(rng.next()) * env * 1.1 * velocity
            }
            Piece::None => 0.0,
        };
        if let Some((keep, fade)) = self.choke {
            if i >= keep {
                let k = (i - keep) as f32 / fade.max(1) as f32;
                v *= (1.0 - k).max(0.0);
            }
        }
        v
    }
}

// ---------------------------------------------------------------- Sound shaping

/// Warmth (see `fx::warmth`).
struct Warm {
    low: Biquad,
    body: Biquad,
    high: Biquad,
    drive: f32,
    bias: f32,
    norm: f32,
    mix: f32,
    dc_x: f32,
    dc_y: f32,
    r: f32,
}

/// Shimmer (see `fx::chorus`).
struct Chorus {
    depth: f32,
    wet: f32,
}

/// Roughness (see `fx::roughness`).
struct Rough {
    am_f: f32,
    depth: f32,
    drive: f32,
    mix: f32,
    makeup: f32,
    a: f32,
    att: f32,
    rel: f32,
    env: f32,
    seed: u32,
    prev_noise: f32,
}

/// Width (see `fx::widen`).
struct Widen {
    d: usize,
    hp: Biquad,
    hp2: Biquad,
    gain: f32,
}

/// Sound shaping after the instrument as a running chain: warmth →
/// shimmer → roughness → color → distance → width. Mono in, mid and side
/// out. Created once (allocates its delay lines), afterwards allocation-free.
pub struct FxChain {
    sr: f32,
    shaping: Option<Shaping>,
    /// Samples since the start (up to the longest delay is enough).
    count: usize,
    /// Samples since the time origin (see [`FxChain::reset`]).
    time: u64,
    /// Phases (0…1) of the two shimmer voices and of the roughness flutter.
    ch_ph: [f64; 2],
    am_ph: f64,
    warm: Option<Warm>,
    chorus: Option<Chorus>,
    rough: Option<Rough>,
    tone: Option<(Biquad, Biquad)>,
    dist: Option<(Biquad, f32)>,
    widen: Option<Widen>,
    /// History of the shimmer's input and of the mid for the width (ring
    /// buffers, `w`/`wm` = slot of the current sample).
    dry: Box<[f32]>,
    w: usize,
    mid: Box<[f32]>,
    wm: usize,
}

/// The two shimmer voices: base delay (s), modulation rate (Hz), phase.
const CHORUS_VOICES: [(f32, f64, f32); 2] = [(0.011, 0.47, 0.0), (0.017, 0.71, 1.9)];

/// Retunes a filter whose state keeps running (parameter change while playing).
fn retune(slot: &mut Option<Biquad>, new: Biquad) {
    match slot {
        Some(b) => b.set_coeffs(&new),
        None => *slot = Some(new),
    }
}

impl FxChain {
    pub fn new(sr: f32) -> Self {
        // longest shimmer delay: 17 ms + 3 ms modulation
        // -0.0 (silence) instead of zeros: the memory is committed now, not in the audio thread
        let dry = vec![-0.0; (0.021 * sr) as usize + 4].into_boxed_slice();
        let mid = vec![-0.0; (0.013 * sr) as usize + 2].into_boxed_slice();
        Self {
            sr,
            shaping: None,
            count: 0,
            time: 0,
            ch_ph: [0.0; 2],
            am_ph: 0.0,
            warm: None,
            chorus: None,
            rough: None,
            tone: None,
            dist: None,
            widen: None,
            dry,
            w: 0,
            mid,
            wm: 0,
        }
    }

    /// Starts over with silence – for a new track on this chain. `time` =
    /// samples since the common time origin: the modulation stands where it
    /// would have stood had the chain run from there (so its phase does not
    /// depend on when exactly the chain was taken).
    pub fn reset(&mut self, time: u64) {
        self.shaping = None;
        self.count = 0;
        self.time = time;
        let sr = self.sr as f64;
        for (k, &(_, rate, _)) in CHORUS_VOICES.iter().enumerate() {
            self.ch_ph[k] = (rate * time as f64 / sr).fract();
        }
        self.am_ph = 0.0;
        self.w = 0;
        self.wm = 0;
        self.warm = None;
        self.chorus = None;
        self.rough = None;
        self.tone = None;
        self.dist = None;
        self.widen = None;
        self.dry.fill(0.0);
        self.mid.fill(0.0);
    }

    /// Whether a side signal comes out (width).
    pub fn widens(&self) -> bool {
        self.widen.is_some()
    }

    /// `n` samples of silence without computing them – a track pauses after
    /// its sound has rung out: delay lines and filters fall silent (what was
    /// still in them was long inaudible), the modulation runs on. Afterwards
    /// it continues as if every sample had been processed (only the roughness
    /// noise takes up where it stopped; for noise that makes no difference).
    pub fn skip(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        let sr = self.sr as f64;
        for (k, &(_, rate, _)) in CHORUS_VOICES.iter().enumerate() {
            self.ch_ph[k] = (self.ch_ph[k] + rate * n as f64 / sr).fract();
        }
        if let Some(r) = self.rough.as_mut() {
            self.am_ph = (self.am_ph + r.am_f as f64 * n as f64 / sr).fract();
            r.env = 0.0;
        }
        if let Some(w) = self.warm.as_mut() {
            w.low.clear();
            w.body.clear();
            w.high.clear();
            (w.dc_x, w.dc_y) = (0.0, 0.0);
        }
        if let Some((lo, hi)) = self.tone.as_mut() {
            lo.clear();
            hi.clear();
        }
        if let Some((lp, _)) = self.dist.as_mut() {
            lp.clear();
        }
        if let Some(wd) = self.widen.as_mut() {
            wd.hp.clear();
            wd.hp2.clear();
        }
        let (cap, mcap) = (self.dry.len(), self.mid.len());
        self.dry.fill(0.0);
        self.w = (self.w + n) % cap;
        self.mid.fill(0.0);
        self.wm = (self.wm + n) % mcap;
        self.count = (self.count + n).min(1 << 20);
        self.time += n as u64;
    }

    /// Sets the controls; running filters keep their state.
    pub fn set(&mut self, s: &Shaping) {
        if self.shaping.as_ref() == Some(s) {
            return;
        }
        self.shaping = Some(*s);
        let sr = self.sr;
        // warmth
        let a = s.waerme.clamp(0.0, 1.0);
        if a < 1e-3 {
            self.warm = None;
        } else {
            let (low, body, high) = (Biquad::lowshelf(220.0, 3.5 * a, sr), Biquad::peaking(450.0, 0.8, 1.5 * a, sr), Biquad::highshelf(5200.0, -6.0 * a, sr));
            let drive = 1.0 + 2.5 * a;
            let bias = 0.12 * a;
            let norm = drive * (1.0 - (bias * drive).tanh().powi(2));
            let mix = 0.35 + 0.4 * a;
            match self.warm.as_mut() {
                Some(w) => {
                    w.low.set_coeffs(&low);
                    w.body.set_coeffs(&body);
                    w.high.set_coeffs(&high);
                    (w.drive, w.bias, w.norm, w.mix) = (drive, bias, norm, mix);
                }
                None => self.warm = Some(Warm { low, body, high, drive, bias, norm, mix, dc_x: 0.0, dc_y: 0.0, r: 1.0 - 2.0 * PI * 12.0 / sr }),
            }
        }
        // shimmer
        let a = s.schwebung.clamp(0.0, 1.0);
        self.chorus = (a >= 1e-3).then(|| Chorus { depth: (0.0006 + 0.0024 * a) * sr, wet: 0.22 + 0.33 * a });
        // roughness
        let a = s.rauheit.clamp(0.0, 1.0);
        if a < 0.01 {
            self.rough = None;
        } else {
            let (env, seed, prev_noise) = self.rough.as_ref().map(|r| (r.env, r.seed, r.prev_noise)).unwrap_or((0.0, 0x9E37_79B9, 0.0));
            if self.rough.is_none() {
                // the flutter stands where it would stand had it run since the time origin
                self.am_ph = ((30.0 + 50.0 * a) as f64 * self.time as f64 / sr as f64).fract();
            }
            self.rough = Some(Rough {
                am_f: 30.0 + 50.0 * a,
                depth: 0.55 * a,
                drive: 1.0 + 6.0 * a * a,
                mix: 0.7 * a,
                makeup: 1.0 + 0.6 * a,
                a,
                att: (-1.0 / (0.001 * sr)).exp(),
                rel: (-1.0 / (0.03 * sr)).exp(),
                env,
                seed,
                prev_noise,
            });
        }
        // color
        let f = s.farbe.clamp(0.0, 1.0) - 0.5;
        if f.abs() < 0.01 {
            self.tone = None;
        } else {
            let hi_db = if f < 0.0 { 28.0 * f } else { 16.0 * f };
            let (lo, hi) = (Biquad::lowshelf(250.0, -6.0 * f, sr), Biquad::highshelf(2500.0, hi_db, sr));
            match self.tone.as_mut() {
                Some((a, b)) => {
                    a.set_coeffs(&lo);
                    b.set_coeffs(&hi);
                }
                None => self.tone = Some((lo, hi)),
            }
        }
        // distance
        let d = ((s.raum.clamp(0.0, 1.0) - 0.3) / 0.7).max(0.0);
        if d < 0.01 {
            self.dist = None;
        } else {
            let lp = Biquad::lowpass((16_000.0 * (1.0 - 0.75 * d)).min(sr * 0.45), 0.707, sr);
            let gain = 1.0 - 0.4 * d;
            let mut slot = self.dist.take().map(|(b, _)| b);
            retune(&mut slot, lp);
            self.dist = slot.map(|b| (b, gain));
        }
        // width
        let b = s.breite.clamp(0.0, 1.0);
        if b < 0.02 {
            self.widen = None;
        } else {
            let gain = 0.6 * b;
            if let Some(w) = self.widen.as_mut() {
                w.gain = gain;
            } else {
                self.widen = Some(Widen { d: (0.013 * sr) as usize, hp: Biquad::highpass(220.0, 0.707, sr), hp2: Biquad::highpass(220.0, 0.707, sr), gain });
            }
        }
    }

    /// One sample: mono in → (mid, side).
    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        let n = self.count;
        let sr = self.sr;
        let tau = std::f64::consts::TAU;
        let mut v = x;
        if let Some(w) = self.warm.as_mut() {
            let x = w.high.process(w.body.process(w.low.process(v)));
            let (bias, drive) = (w.bias, w.drive);
            let shape = |x: f32| ((x + bias) * drive).tanh() - (bias * drive).tanh();
            let sat = shape(x) / w.norm;
            let y = x * (1.0 - w.mix) + sat * w.mix;
            let out = y - w.dc_x + w.r * w.dc_y;
            w.dc_x = y;
            w.dc_y = out;
            v = out;
        }
        // the shimmer's history always runs along, so it can start any time
        let cap = self.dry.len();
        let w = self.w;
        self.dry[w] = v;
        if let Some(c) = &self.chorus {
            let mut s = 0.0;
            for (k, &(base, _, phase)) in CHORUS_VOICES.iter().enumerate() {
                let d = base * sr + c.depth * (0.5 + 0.5 * ((tau * self.ch_ph[k]) as f32 + phase).sin());
                // between the samples `back` and `back − 1` before the current one
                let back = d.ceil();
                s += if (n as f32) < d {
                    0.0
                } else {
                    let f = back - d;
                    let back = back as usize;
                    let x0 = self.dry[(w + cap - back) % cap];
                    let x1 = self.dry[(w + cap - back + 1) % cap];
                    x0 + (x1 - x0) * f
                };
            }
            v = (v + c.wet * 0.5 * s) / (1.0 + 0.35 * c.wet);
        }
        for (k, &(_, rate, _)) in CHORUS_VOICES.iter().enumerate() {
            self.ch_ph[k] = (self.ch_ph[k] + rate / sr as f64).fract();
        }
        self.w = (w + 1) % cap;
        if let Some(r) = self.rough.as_mut() {
            let x = v;
            let level = x.abs();
            r.env = if level > r.env { r.att * r.env + (1.0 - r.att) * level } else { r.rel * r.env + (1.0 - r.rel) * level };
            r.seed ^= r.seed << 13;
            r.seed ^= r.seed >> 17;
            r.seed ^= r.seed << 5;
            let noise = (r.seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let grit = (noise - r.prev_noise) * 0.5 * r.env * 0.35 * r.a;
            r.prev_noise = noise;
            let sat = (x * r.drive).tanh() / r.drive;
            let shaped = (x * (1.0 - r.mix) + sat * r.mix) * r.makeup + grit;
            v = shaped * (1.0 - r.depth * 0.5 * (1.0 - ((tau * self.am_ph) as f32).sin()));
            self.am_ph = (self.am_ph + r.am_f as f64 / sr as f64).fract();
        }
        if let Some((lo, hi)) = self.tone.as_mut() {
            v = hi.process(lo.process(v));
        }
        if let Some((lp, gain)) = self.dist.as_mut() {
            v = lp.process(v) * *gain;
        }
        let mcap = self.mid.len();
        let wm = self.wm;
        let side = match self.widen.as_mut() {
            Some(wd) => {
                let delayed = if n >= wd.d { self.mid[(wm + mcap - wd.d) % mcap] } else { 0.0 };
                wd.hp2.process(wd.hp.process(delayed)) * wd.gain
            }
            None => 0.0,
        };
        self.mid[wm] = v;
        self.wm = (wm + 1) % mcap;
        // only "since when" matters (longer than every delay): no need to count further
        self.count = (n + 1).min(1 << 20);
        self.time += 1;
        (v, side)
    }
}

// ---------------------------------------------------------------- Offline

/// Renders notes (seconds) with an instrument into a mono buffer, without
/// sound shaping – exactly like the instrument's offline function.
pub fn render_mono(patch: &Patch, notes: &[NoteSpec], total_secs: f32, sr: f32, spb: f32) -> Vec<f32> {
    let len = patch.buffer_len(total_secs, sr);
    let mut out = vec![0f32; len];
    let positions: Vec<(f32, f32, f32)> = notes.iter().map(|n| (n.start_secs, n.dur_secs, n.midi)).collect();
    let plans = plan(patch, &positions);
    // the bass adds its notes in temporal order, the others in the given order
    let mut order: Vec<usize> = (0..notes.len()).collect();
    if matches!(patch, Patch::Bass(_)) {
        order.sort_by(|&a, &b| notes[a].start_secs.total_cmp(&notes[b].start_secs));
    }
    let partials = match patch {
        Patch::Organ(p) => Some(organ_partials(p)),
        _ => None,
    };
    if partials.as_ref().is_some_and(|p| p.is_empty()) {
        return out;
    }
    let mut tables: HashMap<u32, OrganTable> = HashMap::new();
    let mut arena = if patch.uses_arena() { vec![0f32; arena_len(sr)] } else { Vec::new() };
    let mut block = [0f32; 256];
    for &i in &order {
        let n = &notes[i];
        let start = (n.start_secs.max(0.0) * sr) as usize;
        if start >= len {
            continue;
        }
        let table = partials.as_ref().map(|p| {
            let freq = organ_freq(n, sr);
            tables.entry(freq.to_bits()).or_insert_with(|| organ_tables(p, freq, sr)).clone()
        });
        let mut v = Voice::start(patch, table.as_ref(), n, &plans[i], 1.0, spb, sr, len - start, &mut arena);
        let mut at = start;
        loop {
            let got = v.render(patch, table.as_ref(), &mut arena, &mut block);
            for (o, s) in out[at..at + got].iter_mut().zip(&block[..got]) {
                *o += s;
            }
            at += got;
            if got < block.len() {
                break;
            }
        }
    }
    out
}

/// Applies the sound shaping to a buffer; returns the side signal (width).
pub fn shape(buf: &mut [f32], shaping: &Shaping, sr: f32) -> Option<Vec<f32>> {
    let mut fx = FxChain::new(sr);
    fx.set(shaping);
    let wide = fx.widens() && !buf.is_empty();
    let mut side = if wide { vec![0f32; buf.len()] } else { Vec::new() };
    for (i, v) in buf.iter_mut().enumerate() {
        let (m, s) = fx.process(*v);
        *v = m;
        if wide {
            side[i] = s;
        }
    }
    wide.then_some(side)
}

/// Renders notes with an instrument and its sound shaping: mid and – with
/// width – side.
pub fn render(patch: &Patch, shaping: &Shaping, notes: &[NoteSpec], total_secs: f32, sr: f32, spb: f32) -> (Vec<f32>, Option<Vec<f32>>) {
    let mut mid = render_mono(patch, notes, total_secs, sr, spb);
    let side = shape(&mut mid, shaping, sr);
    (mid, side)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bass, fx, keys, pad, synth};

    const SR: f32 = 48_000.0;

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len(), "length");
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
    }

    fn with_expression(mut n: NoteSpec, k: usize) -> NoteSpec {
        match k % 4 {
            1 => n.vibrato = 0.4,
            2 => n.bend = -2.0,
            3 => n.brightness = 0.6,
            _ => {}
        }
        n
    }

    /// A little melody with chords, ties, expression and a note past the end.
    fn melody() -> Vec<NoteSpec> {
        let mut v = Vec::new();
        for (k, &(start, dur, midi)) in [(0.0, 0.4, 48.0), (0.0, 0.9, 55.0), (0.38, 0.3, 50.0), (0.68, 0.5, 52.0), (1.2, 1.5, 36.0), (1.25, 0.4, 64.0), (2.9, 0.6, 60.0), (3.5, 1.0, 62.0)]
            .iter()
            .enumerate()
        {
            v.push(with_expression(NoteSpec::new(start, dur, midi, 0.6 + 0.05 * k as f32), k));
        }
        v
    }

    fn beat() -> Vec<NoteSpec> {
        let pieces = [36u8, 38, 39, 42, 46, 42, 41, 45, 48, 37, 56, 51, 49, 70, 46, 44];
        pieces.iter().enumerate().map(|(k, &m)| with_expression(NoteSpec::new(k as f32 * 0.18, 0.1, m as f32, 0.9), k)).collect()
    }

    #[test]
    fn voices_sound_like_the_offline_instruments() {
        let notes = melody();
        let total = 3.2;
        let pluck = PluckParams::default();
        assert!(max_diff(&render_mono(&Patch::Pluck(pluck), &notes, total, SR, 0.5), &synth::render_notes(&notes, total, SR, &pluck)) < 1e-6);
        let b = bass::BassParams { glide: 0.6, ..Default::default() };
        assert!(max_diff(&render_mono(&Patch::Bass(b), &notes, total, SR, 0.5), &bass::render_bass(&notes, total, SR, &b)) < 1e-6);
        let o = keys::OrganParams { subbass: 0.4, tremulant: 0.5, mixtur: 0.3, ..Default::default() };
        assert!(max_diff(&render_mono(&Patch::Organ(o), &notes, total, SR, 0.5), &keys::render_organ(&notes, total, SR, &o)) < 1e-6);
        let h = keys::HarpsichordParams { vierfuss: 0.7, ..Default::default() };
        assert!(max_diff(&render_mono(&Patch::Harpsichord(h), &notes, total, SR, 0.5), &keys::render_harpsichord(&notes, total, SR, &h)) < 1e-6);
        for puls in [0u8, 2] {
            let s = pad::SynthParams::default();
            let v = pad::Verlauf { puls, pulstiefe: 0.6, ..Default::default() };
            let live = render_mono(&Patch::Synth(s, v.clone()), &notes, total, SR, 0.4);
            assert!(max_diff(&live, &pad::render_synth(&notes, total, SR, &s, &v, 0.4)) < 1e-6, "synth, pulse {puls}");
        }
        let kit = DrumKit::default();
        let beat = beat();
        assert!(max_diff(&render_mono(&Patch::Drums(kit), &beat, 3.0, SR, 0.5), &drums::render_drums(&beat, 3.0, SR, &kit)) < 1e-6);
        // tuned drums: a melody across all three zones (also on drum keys and between them), in both modes
        let tune: Vec<NoteSpec> = [30.0f32, 38.0, 42.0, 46.0, 49.0, 55.0, 60.0, 64.5, 70.0, 74.0, 79.0]
            .iter()
            .enumerate()
            .map(|(k, &m)| with_expression(NoteSpec::new(k as f32 * 0.22, 0.2, m, 0.8), k))
            .collect();
        for pitched in [false, true] {
            let kit = DrumKit { pitched, ..DrumKit::default() };
            let live = render_mono(&Patch::Drums(kit), &tune, 3.0, SR, 0.5);
            assert!(max_diff(&live, &drums::render_drums(&tune, 3.0, SR, &kit)) < 1e-6, "tuned drums, by pitch: {pitched}");
        }
    }

    #[test]
    fn the_chain_shapes_like_the_offline_effects() {
        let x: Vec<f32> = (0..30_000).map(|i| (i as f32 * 0.031).sin() * 0.5 + (i as f32 * 0.0071).sin() * 0.3).collect();
        let s = Shaping { waerme: 0.6, schwebung: 0.5, rauheit: 0.4, farbe: 0.3, raum: 0.8, breite: 0.7 };
        let mut offline = x.clone();
        fx::warmth(&mut offline, s.waerme, SR);
        fx::chorus(&mut offline, s.schwebung, SR);
        fx::roughness(&mut offline, s.rauheit, SR);
        fx::tone(&mut offline, s.farbe, SR);
        fx::distance(&mut offline, s.raum, SR);
        let side = fx::widen(&offline, s.breite, SR).unwrap();
        let mut live = x.clone();
        let live_side = shape(&mut live, &s, SR).unwrap();
        assert!(max_diff(&live, &offline) < 1e-4, "mid {}", max_diff(&live, &offline));
        assert!(max_diff(&live_side, &side) < 1e-4);
        // neutral: unchanged, no side
        let mut same = x.clone();
        assert!(shape(&mut same, &Shaping { farbe: 0.5, ..Default::default() }, SR).is_none());
        assert_eq!(same, x);
    }

    #[test]
    fn live_notes_play_like_the_offline_clip() {
        // the same clip once offline (seconds), once as prepared notes (beats) voice by voice
        let spb = 0.5f64;
        let beats: Vec<LiveNote> = [(0.0, 1.0, 45.0), (0.5, 2.0, 52.0), (2.5, 0.5, 47.0), (3.0, 4.0, 40.0)]
            .iter()
            .map(|&(s, l, m)| LiveNote::new(s, l, m, 0.8))
            .collect();
        let len_beats = 4.0;
        let patch = Patch::Bass(bass::BassParams { glide: 0.5, ..Default::default() });
        let clip = LiveNotes::new(patch.clone(), Shaping::default(), &beats, len_beats, SR);
        assert_eq!(clip.notes.len(), 4);
        assert!((clip.notes[3].len - 1.0).abs() < 1e-12, "shortened at the clip end");
        let specs: Vec<NoteSpec> = clip.notes.iter().map(|n| n.spec(spb)).collect();
        let offline = render_mono(&patch, &specs, (len_beats * spb) as f32, SR, spb as f32);
        let mut live = vec![0f32; offline.len()];
        for k in 0..clip.notes.len() {
            let start = (clip.notes[k].start * spb * SR as f64) as usize;
            let mut v = clip.start_voice(k, spb, SR, live.len() - start, &mut []).unwrap();
            let mut buf = vec![0f32; live.len() - start];
            let got = clip.render_voice(&mut v, &mut [], &mut buf);
            assert!(v.done());
            for (o, s) in live[start..].iter_mut().zip(&buf[..got]) {
                *o += s;
            }
        }
        assert!(max_diff(&live, &offline) < 1e-5);
    }

    #[test]
    fn a_voice_can_start_in_the_middle_of_its_note() {
        let n = NoteSpec::new(0.0, 2.0, 57.0, 0.8);
        let mut arena = vec![0f32; arena_len(SR)];
        for patch in [Patch::Pluck(PluckParams::default()), Patch::Synth(pad::SynthParams::default(), pad::Verlauf::default()), Patch::Bass(Default::default())] {
            let mut whole = Voice::start(&patch, None, &n, &Plan::default(), 1.0, 0.5, SR, usize::MAX, &mut arena);
            let mut a = vec![0f32; 48_000];
            whole.render(&patch, None, &mut arena, &mut a);
            let late = &a[24_000..];
            let mut v = Voice::start(&patch, None, &n, &Plan::default(), 1.0, 0.5, SR, usize::MAX, &mut arena);
            v.skip(24_000, &mut arena);
            let mut b = vec![0f32; 24_000];
            let got = v.render(&patch, None, &mut arena, &mut b);
            assert_eq!(got, late.len().min(got));
            // about as loud as the note at that point
            let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            let (want, have) = (rms(&late[2000..10_000]), rms(&b[2000..10_000]));
            assert!(have > want * 0.5 && have < want * 2.0, "{patch:?}: {have} vs {want}");
        }
    }
}
