//! Several voices at once – the polyphonic analysis mode, next to the single
//! line of [`crate::analysis`] (which stays as it is).
//!
//! The pitches, notes and voices come from [`klang_dsp::poly`] (carried over
//! from the LED light organ's analysis and taken further to several voices):
//! drums separated off, several pitches per frame by harmonic subtraction,
//! per pitch the most likely path (Viterbi), then how many voices the
//! material has and which note belongs to which.
//!
//! * **Finished recording**: [`AppState::analyze_voices`] (cached per
//!   recording; the voice assignment is cheap and runs per call).
//! * **Converting**: [`AppState::audio_to_voices`] – one instrument track per
//!   voice below the recording, each voice smoothed (see `klang_core::smooth`)
//!   and on the sixteenth grid; the lowest becomes a bass track when it lies low.
//! * **While recording**: [`LiveVoices`] gets the same samples as the
//!   single-line live analysis; notes settle once nothing before them can
//!   change any more.

use crate::analysis::KeyInfo;
use crate::convert::Converted;
use crate::sound;
use crate::state::{lock, AppState, CmdResult};
use klang_core::smooth::{self as smoothing, Strength};
use klang_core::{edit, Clip, ClipSource, Command, Id, InstrumentKind, Note, Track, TrackKind};
use klang_dsp::onset::{detect_onsets, estimate_tempo, OnsetConfig};
use klang_dsp::poly::{self, PolyFrame, PolyNote, PolyStream, VoiceCount};
use klang_i18n::tr;
use klang_music::theory::{estimate_key, TimedNote};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Which analysis turns a recording into notes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalysisMode {
    /// One line (singing, a melody instrument) – [`crate::analysis`].
    #[default]
    #[serde(alias = "einstimmig", alias = "single")]
    Mono,
    /// Several voices at once – this module.
    #[serde(alias = "mehrstimmig", alias = "voices", alias = "polyphonic")]
    Poly,
}

/// How many voices, as the commands take it: "auto" (or missing) or 1–6.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceArg(pub VoiceCount);

impl<'de> Deserialize<'de> for VoiceArg {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        parse_voices(&v).map(VoiceArg).map_err(serde::de::Error::custom)
    }
}

/// "auto" / "automatisch" / null → automatic, 1–6 (number or text) → that many.
pub fn parse_voices(v: &Value) -> Result<VoiceCount, String> {
    let n = match v {
        Value::Null => return Ok(VoiceCount::Auto),
        Value::String(s) if matches!(s.trim().to_lowercase().as_str(), "auto" | "automatisch" | "automatic" | "") => return Ok(VoiceCount::Auto),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    };
    match n {
        Some(n) if (1..=poly::MAX_VOICES as u64).contains(&n) => Ok(VoiceCount::Fixed(n as u8)),
        _ => Err(tr!("\"voices\" must be \"auto\" or a number from 1 to {max}", max = poly::MAX_VOICES)),
    }
}

/// What the expensive part found in a recording (cached per recording).
pub struct PolyRaw {
    pub duration_secs: f64,
    pub notes: Vec<PolyNote>,
    /// For the display: every other frame – time and the pitches found
    /// (MIDI, strength relative to a clearly sounding note).
    pub candidates: Vec<(f32, Vec<(f32, f32)>)>,
    pub onsets: Vec<f32>,
    pub tempo_estimate: Option<f32>,
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

fn candidates(frames: &[PolyFrame], reference: f32) -> Vec<(f32, Vec<(f32, f32)>)> {
    frames.iter().step_by(2).map(|f| (round3(f.time), f.peaks.iter().map(|p| (round2(p.midi), round2(p.salience / reference))).collect())).collect()
}

pub fn analyze_raw(samples: &[f32], sample_rate: u32) -> PolyRaw {
    let sr = sample_rate as f32;
    let (frames, hop) = poly::analyze_frames(samples, sample_rate);
    let onsets = detect_onsets(samples, sr, &OnsetConfig::default());
    let tempo_estimate = estimate_tempo(&onsets.envelope, onsets.hop_secs, 60.0, 200.0);
    let reference = poly::reference(&frames);
    let notes = poly::track_notes(&frames, hop, &onsets.onsets, reference);
    PolyRaw {
        duration_secs: samples.len() as f64 / sample_rate.max(1) as f64,
        candidates: candidates(&frames, reference),
        notes,
        onsets: onsets.onsets,
        tempo_estimate,
    }
}

/// A note with its voice (0 = the highest).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceNote {
    pub start_secs: f64,
    pub end_secs: f64,
    pub midi: u8,
    pub cents: f32,
    /// 1–127: dynamics relative to the recording (the median note is mf = 80).
    pub velocity: u8,
    pub voice: usize,
    /// Struck anew (not the same pitch going on).
    pub new_attack: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceInfo {
    pub notes: usize,
    pub low: Option<u8>,
    pub high: Option<u8>,
}

/// The polyphonic analysis of a recording.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicesAnalysis {
    pub asset_id: String,
    pub duration_secs: f64,
    /// How many voices, and whether that was found from the material.
    pub voices: usize,
    pub auto: bool,
    pub per_voice: Vec<VoiceInfo>,
    /// Sorted by start.
    pub notes: Vec<VoiceNote>,
    pub candidates: Vec<(f32, Vec<(f32, f32)>)>,
    pub onsets: Vec<f32>,
    pub tempo_estimate: Option<f32>,
    pub key: Option<KeyInfo>,
}

/// Velocity 1–127 per note: dynamics relative to the median note (mf = 80),
/// as in the single-line analysis (3.4 per dB).
fn velocities(notes: &[PolyNote]) -> Vec<u8> {
    let mut amps: Vec<f32> = notes.iter().map(|n| n.amp).filter(|a| *a > 0.0).collect();
    if amps.is_empty() {
        return vec![80; notes.len()];
    }
    let med = klang_dsp::util::median(&mut amps).max(1e-9);
    notes.iter().map(|n| (80.0 + 3.4 * klang_dsp::util::gain_to_db(n.amp.max(1e-9) / med)).round().clamp(16.0, 127.0) as u8).collect()
}

fn key_of(notes: &[PolyNote]) -> Option<KeyInfo> {
    let timed: Vec<TimedNote> = notes.iter().map(|n| TimedNote { start: n.start as f64, end: n.end as f64, midi: n.midi, cents: n.cents, velocity: 0.8 }).collect();
    estimate_key(&timed).map(|k| KeyInfo { name: k.name(), tonic: k.tonic, minor: k.minor, confidence: k.confidence })
}

/// Notes with voices from what a recording holds.
pub fn with_voices(notes: &[PolyNote], count: VoiceCount) -> (Vec<VoiceNote>, usize) {
    let (voice, n) = poly::assign_voices(notes, count);
    let vel = velocities(notes);
    let out = notes
        .iter()
        .zip(voice)
        .zip(vel)
        .map(|((p, v), velocity)| VoiceNote {
            start_secs: round3(p.start) as f64,
            end_secs: round3(p.end) as f64,
            midi: p.midi,
            cents: p.cents.round(),
            velocity,
            voice: v,
            new_attack: p.new_attack,
        })
        .collect();
    (out, n)
}

fn per_voice(notes: &[VoiceNote], n: usize) -> Vec<VoiceInfo> {
    (0..n)
        .map(|v| {
            let mine: Vec<u8> = notes.iter().filter(|x| x.voice == v).map(|x| x.midi).collect();
            VoiceInfo { notes: mine.len(), low: mine.iter().min().copied(), high: mine.iter().max().copied() }
        })
        .collect()
}

pub fn voices_analysis(asset_id: &str, raw: &PolyRaw, count: VoiceCount) -> VoicesAnalysis {
    let (notes, n) = with_voices(&raw.notes, count);
    VoicesAnalysis {
        asset_id: asset_id.to_string(),
        duration_secs: raw.duration_secs,
        voices: n,
        auto: count == VoiceCount::Auto,
        per_voice: per_voice(&notes, n),
        notes,
        candidates: raw.candidates.clone(),
        onsets: raw.onsets.clone(),
        tempo_estimate: raw.tempo_estimate,
        key: key_of(&raw.notes),
    }
}

/// The notes of one voice on the sixteenth grid: smoothed first (on the
/// exact times), then snapped; a note never reaches into the next one of
/// the same voice (notes that start together stay a chord). Also returns
/// how many notes smoothing merged or removed.
fn voice_notes(notes: &[&VoiceNote], spb: f64, shift: i32, smooth: Strength) -> (Vec<Note>, usize) {
    const GRID: f64 = 0.25;
    let snap = |b: f64| (b / GRID).round() * GRID;
    let items: Vec<smoothing::Item> = notes
        .iter()
        .map(|n| smoothing::Item { start: n.start_secs / spb, end: n.end_secs / spb, pitch: n.midi, velocity: n.velocity, accent: 0.0, new_attack: Some(n.new_attack) })
        .collect();
    let plan = smoothing::plan(&items, None, smooth, smoothing::Scale { secs_per_beat: spb, grid_beats: GRID });
    let smoothed = plan.merged() + plan.removed.len();
    let mut out: Vec<Note> = plan
        .groups
        .iter()
        .map(|g| {
            let start = snap(g.start).max(0.0);
            let end = snap(g.end).max(start + GRID);
            Note::new((notes[g.main].midi as i32 + shift).clamp(0, 127) as u8, start, end - start, g.velocity.max(1))
        })
        .collect();
    out.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
    out.dedup_by(|b, a| a.pitch == b.pitch && (a.start_beats - b.start_beats).abs() < 1e-9);
    let starts: Vec<f64> = out.iter().map(|n| n.start_beats).collect();
    for (i, n) in out.iter_mut().enumerate() {
        if let Some(&next) = starts[i + 1..].iter().find(|&&s| s > n.start_beats + 1e-9) {
            n.length_beats = n.length_beats.min(next - n.start_beats);
        }
    }
    (out, smoothed)
}

impl AppState {
    /// The polyphonic analysis' expensive part for a recording (cached).
    pub fn voices_raw(&self, asset_id: &str) -> CmdResult<Arc<PolyRaw>> {
        let (samples, sr) = {
            let pool = lock(&self.pool)?;
            if let Some(r) = pool.voices.get(asset_id) {
                return Ok(r.clone());
            }
            let asset = pool.get(asset_id).ok_or_else(|| tr!("Audio file not found"))?;
            (asset.samples.clone(), asset.sample_rate)
        };
        let raw = Arc::new(analyze_raw(&samples, sr));
        lock(&self.pool)?.voices.insert(asset_id.to_string(), raw.clone());
        Ok(raw)
    }

    /// The polyphonic analysis of a recording with `count` voices.
    pub fn analyze_voices(&self, asset_id: &str, count: VoiceCount) -> CmdResult<VoicesAnalysis> {
        Ok(voices_analysis(asset_id, &*self.voices_raw(asset_id)?, count))
    }

    /// The voices of audio clips as instrument tracks directly below the
    /// audio track – one per voice (the highest first), one clip per audio
    /// clip at the same place; the lowest becomes a bass track when it lies
    /// low. One undo step. `clip_id`: None = all audio clips of the track.
    pub fn audio_to_voices(&self, track_id: Id, clip_id: Option<Id>, label: &str, smooth: Strength, count: VoiceCount) -> CmdResult<Converted> {
        let (track, index, bpm, bpb) = {
            let s = lock(&self.session)?;
            let p = &s.project;
            let i = p.track_index(track_id).ok_or_else(|| tr!("Track {id} not found", id = track_id))?;
            (p.tracks[i].clone(), i, p.tempo_bpm, p.beats_per_bar.max(1) as f64)
        };
        let mut clips: Vec<&Clip> = track.clips.iter().filter(|c| matches!(c.source, ClipSource::Audio { .. }) && clip_id.is_none_or(|id| id == c.id)).collect();
        if clips.is_empty() {
            return Err(match clip_id {
                Some(id) => tr!("Clip {id} in \"{name}\" is not an audio clip – only recordings and inserted audio files can be converted.", id = id, name = track.name),
                None => tr!("\"{name}\" contains no audio clips.", name = track.name),
            });
        }
        clips.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats));
        let shift = track.pitch_semitones.round() as i32;
        // per voice: (clip start, clip length, notes)
        let mut voices: Vec<Vec<(f64, f64, Vec<Note>)>> = Vec::new();
        let mut smoothed = 0;
        for clip in &clips {
            let ClipSource::Audio { asset_id, .. } = &clip.source else { continue };
            let raw = self.voices_raw(asset_id)?;
            let (notes, n) = with_voices(&raw.notes, count);
            let spb = 1.0 / edit::audio_beats_per_sec(&clip.source, bpm);
            let length_beats = (clip.length_beats(bpm) / bpb - 1e-9).ceil().max(1.0) * bpb;
            if voices.len() < n {
                voices.resize_with(n, Vec::new);
            }
            for (v, list) in voices.iter_mut().enumerate().take(n) {
                let mine: Vec<&VoiceNote> = notes.iter().filter(|x| x.voice == v).collect();
                let (vn, k) = voice_notes(&mine, spb, shift, smooth);
                smoothed += k;
                if !vn.is_empty() {
                    list.push((clip.start_beats, length_beats, vn));
                }
            }
        }
        voices.retain(|v| !v.is_empty());
        if voices.is_empty() {
            return Err(tr!("No notes were detected in it – for drums, use \"Convert to rhythm track\"."));
        }
        let base = if clips.len() == 1 { clips[0].name.clone() } else { track.name.clone() };
        let total = voices.len();
        let notes: usize = voices.iter().flatten().map(|c| c.2.len()).sum();
        let (cmd, ids) = {
            let mut s = lock(&self.session)?;
            let p = &mut s.project;
            let mut commands = Vec::new();
            let mut ids = Vec::new();
            for (v, list) in voices.into_iter().enumerate() {
                let mut pitches: Vec<u8> = list.iter().flat_map(|c| c.2.iter().map(|n| n.pitch)).collect();
                pitches.sort_unstable();
                let median = pitches[pitches.len() / 2];
                let bass = v + 1 == total && median < 48;
                let name = if total == 1 {
                    tr!("Melody from {name}", name = base)
                } else if bass {
                    tr!("Bass from {name}", name = base)
                } else {
                    tr!("Voice {n} from {name}", n = v + 1, name = base)
                };
                let id = p.alloc_id();
                let mut t = Track::new(id, name, TrackKind::Instrument);
                t.instrument = Some(sound::default_instrument(if bass { InstrumentKind::Bass } else { InstrumentKind::Pluck }));
                for (start, length, notes) in list {
                    let name = tr!("Voice {n}", n = v + 1);
                    t.clips.push(Clip { id: p.alloc_id(), name, start_beats: start, source: ClipSource::Notes { length_beats: length, notes } });
                }
                commands.push(Command::AddTrack { track: t, index: Some(index + 1 + v) });
                ids.push(id);
            }
            (Command::Batch { label: label.into(), commands }, ids)
        };
        let snapshot = self.execute(cmd, false)?;
        Ok(Converted { snapshot, track_id: ids[0], notes, smoothed, track_ids: ids, voices: total })
    }
}

// ---------------------------------------------------------------- While recording

/// Notes settle once they ended this long before the newest frame (the
/// per-pitch paths have long converged by then).
const SETTLE_SECS: f32 = 0.6;
/// The live view sends the notes of this many seconds before the newest frame.
const VIEW_SECS: f64 = 12.0;
/// The reference of a clearly sounding note looks at this many newest frames.
const REFERENCE_FRAMES: usize = 3000;
/// Frames kept before the earliest open note (its Viterbi path needs a little before).
const CUT_MARGIN_SECS: f32 = 0.15;

/// The polyphonic analysis of a running recording.
pub struct LiveVoices {
    take: u32,
    count: VoiceCount,
    stream: PolyStream,
    hop: f32,
    /// Frames from the cut on (earlier ones only left their notes).
    frames: Vec<PolyFrame>,
    /// The strongest salience of the newest frames (for the reference).
    tops: std::collections::VecDeque<f32>,
    settled: Vec<PolyNote>,
    pending: Vec<PolyNote>,
}

/// What the UI shows of it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveVoicesView {
    pub take: u32,
    /// Analyzed so far (seconds; about 0.2 s behind the recording).
    pub seconds: f64,
    pub voices: usize,
    pub auto: bool,
    /// The notes of the last few seconds (settled and still open), with voices.
    pub notes: Vec<VoiceNote>,
    /// What sounds right now (MIDI, lowest first).
    pub sounding: Vec<u8>,
}

impl LiveVoices {
    pub fn new(take: u32, sample_rate: u32, count: VoiceCount) -> Self {
        let stream = PolyStream::new(sample_rate);
        let hop = stream.hop_secs();
        Self { take, count, stream, hop, frames: Vec::new(), tops: Default::default(), settled: Vec::new(), pending: Vec::new() }
    }

    pub fn take(&self) -> u32 {
        self.take
    }

    fn reference(&self) -> f32 {
        let mut v: Vec<f32> = self.tops.iter().copied().collect();
        if v.is_empty() {
            return 1.0;
        }
        v.sort_by(f32::total_cmp);
        v[(v.len() * 3 / 4).min(v.len() - 1)].max(1e-9)
    }

    /// Measures new samples; `onsets`: those of the single-line live analysis (seconds).
    pub fn push(&mut self, samples: &[f32], onsets: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.stream.push(samples);
        let new = self.stream.take_frames();
        if new.is_empty() {
            return;
        }
        for f in &new {
            if let Some(p) = f.peaks.first() {
                self.tops.push_back(p.salience);
                if self.tops.len() > REFERENCE_FRAMES {
                    self.tops.pop_front();
                }
            }
        }
        self.frames.extend(new);
        let Some(newest) = self.frames.last().map(|f| f.time) else { return };
        let from = onsets.partition_point(|&t| t < self.frames[0].time);
        let notes = poly::track_notes(&self.frames, self.hop, &onsets[from..], self.reference());
        // a note is settled once it ended well before the newest frame; one already
        // settled comes again while frames of it are left (its tail): skipped
        let limit = newest - SETTLE_SECS;
        let hop = self.hop;
        let mut open = Vec::new();
        for n in notes {
            let known = self.settled.iter().rev().take(64).any(|s| s.midi == n.midi && n.start >= s.start - 2.0 * hop && n.start <= s.end + hop);
            if known {
                continue;
            }
            if n.end < limit {
                self.settled.push(n);
            } else {
                open.push(n);
            }
        }
        self.pending = open;
        // the frames before every open note (and before the margin) are done with
        let keep_from = self.pending.iter().map(|n| n.start).fold(limit, f32::min) - CUT_MARGIN_SECS;
        let cut = self.frames.partition_point(|f| f.time < keep_from);
        if cut > 0 {
            self.frames.drain(..cut);
        }
    }

    pub fn view(&self) -> LiveVoicesView {
        let mut all: Vec<PolyNote> = self.settled.iter().chain(&self.pending).copied().collect();
        all.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.midi.cmp(&a.midi)));
        let (notes, n) = with_voices(&all, self.count);
        let seconds = self.frames.last().map_or(0.0, |f| f.time as f64);
        let reference = self.reference();
        let mut sounding: Vec<u8> = self
            .frames
            .last()
            .map(|f| f.peaks.iter().filter(|p| p.salience >= 0.25 * reference).map(|p| p.midi.round().clamp(0.0, 127.0) as u8).collect())
            .unwrap_or_default();
        sounding.sort_unstable();
        sounding.dedup();
        LiveVoicesView {
            take: self.take,
            seconds: round3(seconds as f32) as f64,
            voices: n,
            auto: self.count == VoiceCount::Auto,
            notes: notes.into_iter().filter(|x| x.end_secs >= seconds - VIEW_SECS).collect(),
            sounding,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klang_dsp::synth::NoteSpec;

    /// A three-voice chorale played with Klangwelt's own instruments: soprano
    /// and alto on the plucked instrument, the bass on the bass synth –
    /// (MIDI, beat, beats) per voice at 100 BPM.
    fn chorale() -> (Vec<f32>, Vec<Vec<(u8, f64, f64)>>) {
        let sr = 44_100.0;
        let spb = 0.6;
        let soprano = [(76, 0.0, 1.0), (74, 1.0, 1.0), (72, 2.0, 1.0), (74, 3.0, 1.0), (76, 4.0, 1.0), (76, 5.0, 1.0), (76, 6.0, 2.0)];
        let alto = [(67, 0.0, 2.0), (65, 2.0, 2.0), (64, 4.0, 2.0), (67, 6.0, 2.0)];
        let bass = [(48, 0.0, 2.0), (43, 2.0, 2.0), (36, 4.0, 2.0), (43, 6.0, 2.0)];
        let voices: Vec<Vec<(u8, f64, f64)>> = vec![soprano.to_vec(), alto.to_vec(), bass.to_vec()];
        let total = 8.0 * spb as f32 + 0.5;
        let mut mix = vec![0.0f32; (total * sr) as usize];
        for (v, notes) in voices.iter().enumerate() {
            let kind = if v == 2 { InstrumentKind::Bass } else { InstrumentKind::Pluck };
            let inst = sound::default_instrument(kind);
            let specs: Vec<NoteSpec> = notes.iter().map(|&(m, b, l)| NoteSpec::new((b * spb) as f32, (l * spb) as f32 * 0.95, m as f32, 0.8)).collect();
            let r = sound::render_track(Some(&inst), &specs, total, sr, spb as f32);
            // balanced like an ensemble: every voice at the same level
            let gain = 0.1 / klang_dsp::util::rms(&r.mid).max(1e-6);
            for (o, s) in mix.iter_mut().zip(r.mid.iter()) {
                *o += s * gain;
            }
        }
        (mix, voices)
    }

    #[test]
    fn a_chorale_on_our_instruments_comes_back_voice_by_voice() {
        let (x, truth) = chorale();
        let t = std::time::Instant::now();
        let raw = analyze_raw(&x, 44_100);
        let secs = t.elapsed().as_secs_f32();
        eprintln!("polyphonic analysis of {:.1} s audio: {secs:.2} s", x.len() as f32 / 44_100.0);
        let a = voices_analysis("t", &raw, VoiceCount::Auto);
        let got: Vec<(u8, f64, usize)> = a.notes.iter().map(|n| (n.midi, n.start_secs, n.voice)).collect();
        assert_eq!(a.voices, 3, "{got:?}");
        // per voice: the notes with the right pitch and an onset within 80 ms
        for (v, notes) in truth.iter().enumerate() {
            let mine: Vec<&VoiceNote> = a.notes.iter().filter(|n| n.voice == v).collect();
            let hit = notes.iter().filter(|&&(m, b, _)| mine.iter().any(|n| n.midi == m && (n.start_secs - b * 0.6).abs() < 0.08)).count();
            assert!(hit as f32 >= 0.75 * notes.len() as f32, "voice {v}: {hit}/{} – {got:?}", notes.len());
            assert!(mine.len() <= notes.len() + 2, "voice {v}: {} notes for {} – {got:?}", mine.len(), notes.len());
        }
        assert!(secs < 2.5, "5 s of audio analyzed in {secs:.2} s");
        assert_eq!(a.key.as_ref().map(|k| k.name.as_str()), Some("C major"));
    }

    #[test]
    fn converting_makes_a_track_per_voice_in_one_undo_step() {
        let (x, _) = chorale();
        let st = AppState::without_audio();
        let asset = lock(&st.pool).unwrap().insert("Satz".into(), x, 44_100);
        let mut track = Track::new(Id::NEW, "Satz", TrackKind::Audio);
        // recorded at 100 BPM; the project too
        st.execute(Command::SetProject { patch: klang_core::ProjectPatch { tempo_bpm: Some(100.0), ..Default::default() } }, false).unwrap();
        track.clips.push(Clip { id: Id::NEW, name: "Satz".into(), start_beats: 0.0, source: ClipSource::Audio { asset_id: asset, source_bpm: 100.0, warp: false, duration_secs: 5.3 } });
        st.execute(Command::AddTrack { track, index: None }, false).unwrap();
        let tid = lock(&st.session).unwrap().project.tracks[0].id;
        let r = st.audio_to_voices(tid, None, "Convert", Strength::Medium, VoiceCount::Auto).unwrap();
        assert_eq!(r.voices, 3);
        let p = lock(&st.session).unwrap().project.clone();
        let names: Vec<&str> = p.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Satz", "Voice 1 from Satz", "Voice 2 from Satz", "Bass from Satz"]);
        assert_eq!(p.tracks[3].instrument.as_ref().map(|i| i.kind), Some(InstrumentKind::Bass));
        let first = |i: usize| match &p.tracks[i].clips[0].source {
            ClipSource::Notes { notes, .. } => notes.iter().map(|n| (n.pitch, n.start_beats)).collect::<Vec<_>>(),
            _ => vec![],
        };
        // the bass synth's sub-oscillator may leave the octave below its last note (real energy,
        // as an octave bass would be – kept rather than losing real low notes)
        let bass: Vec<u8> = first(3).iter().map(|n| n.0).collect();
        assert!(bass.starts_with(&[48, 43, 36, 43]) && bass.len() <= 5, "{bass:?}");
        assert_eq!(first(2).iter().map(|n| n.0).collect::<Vec<_>>(), vec![67, 65, 64, 67], "{:?}", first(2));
        assert_eq!(first(1).first().map(|n| n.1), Some(0.0));
        st.undo().unwrap();
        assert_eq!(lock(&st.session).unwrap().project.tracks.len(), 1, "one undo step");
        // a given count
        let r = st.audio_to_voices(tid, None, "Convert", Strength::Medium, VoiceCount::Fixed(2)).unwrap();
        assert_eq!(r.voices, 2);
    }

    #[test]
    fn live_finds_the_same_voices_piece_by_piece() {
        let (x, truth) = chorale();
        let onsets = detect_onsets(&x, 44_100.0, &OnsetConfig::default()).onsets;
        let mut l = LiveVoices::new(3, 44_100, VoiceCount::Auto);
        for c in x.chunks(2048) {
            l.push(c, &onsets);
        }
        let v = l.view();
        assert_eq!(v.voices, 3, "{:?}", v.notes);
        let total: usize = truth.iter().map(|t| t.len()).sum();
        assert!(v.notes.len() + 2 >= total && v.notes.len() <= total + 3, "{} notes for {total}: {:?}", v.notes.len(), v.notes);
        assert!(l.settled.len() >= total - 4, "most notes settled: {}", l.settled.len());
        // the frames before the cut are gone
        assert!(l.frames.len() < 400, "{}", l.frames.len());
    }

    #[test]
    fn voices_argument_reads_auto_and_numbers() {
        assert_eq!(parse_voices(&Value::Null), Ok(VoiceCount::Auto));
        assert_eq!(parse_voices(&serde_json::json!("auto")), Ok(VoiceCount::Auto));
        assert_eq!(parse_voices(&serde_json::json!(3)), Ok(VoiceCount::Fixed(3)));
        assert_eq!(parse_voices(&serde_json::json!("4")), Ok(VoiceCount::Fixed(4)));
        assert!(parse_voices(&serde_json::json!(9)).is_err());
        let m: AnalysisMode = serde_json::from_value(serde_json::json!("mehrstimmig")).unwrap();
        assert_eq!(m, AnalysisMode::Poly);
    }
}

