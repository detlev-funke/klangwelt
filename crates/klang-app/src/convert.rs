//! Audio and notes across tracks:
//!
//! * **Insert audio files** (drag & drop, agent): one audio track with a clip
//!   per file – or, when dropped onto an audio track, the first file into it.
//! * **Recording → notes**: detected melody (instrument track) or detected
//!   hits (drums track) directly below the audio track.
//! * **Delete a time range** across all tracks with ripple delete; audio is cut
//!   in the process (new file in the sound store, the old one stays for undo).
//! * **Align notes to the grid** (quantize).
//!
//! Planning lives in the core (`klang_core::edit`); this module only holds what
//! needs audio data or the analysis. Every action is one undo step.

use crate::analysis::Analysis;
use crate::expression::{self, Articulation, Attack};
use crate::audiofile;
use crate::sound;
use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::edit::{self, AudioKeep};
use klang_core::smooth::{self as smoothing, Strength};
use klang_core::{Clip, ClipSource, Command, Id, InstrumentKind, Note, Track, TrackKind};
use klang_dsp::hits;
use klang_i18n::{tr, trn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Longer files are rejected (Klangwelt keeps audio in memory).
pub const MAX_IMPORT_SECS: f64 = 30.0 * 60.0;
/// Hits below this (relative to the loudest) count as noise.
const MIN_HIT_STRENGTH: f32 = 0.05;
/// Short fades at cut edges against clicks (seconds).
const CUT_FADE_SECS: f64 = 0.003;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub track_id: Id,
    pub clip_id: Id,
    pub name: String,
    /// Format in words ("WAV 24 Bit").
    pub format: String,
    pub seconds: f64,
    /// Channels of the file (Klangwelt averages them to mono).
    pub channels: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub snapshot: Snapshot,
    pub imported: Vec<Imported>,
    /// Files that could not be read (with reason).
    pub errors: Vec<String>,
}

/// What a recording is converted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// Detected notes as an instrument track (low registers: bass).
    #[serde(alias = "melody", alias = "noten", alias = "notes")]
    Melodie,
    /// Detected hits as a drums track (bass drum, snare, hi-hat).
    #[serde(alias = "rhythm", alias = "drums", alias = "schlagzeug")]
    Rhythmus,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Converted {
    pub snapshot: Snapshot,
    pub track_id: Id,
    /// Number of notes created.
    pub notes: usize,
    /// Detected notes that smoothing merged into a neighbour or removed.
    pub smoothed: usize,
    /// Every track created (several when converting voice by voice) and how many voices.
    pub track_ids: Vec<Id>,
    pub voices: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Smoothed {
    pub snapshot: Snapshot,
    /// New indices of the notes that came from the chosen ones (for the selection).
    pub selected: Vec<usize>,
    /// False: nothing to smooth – nothing was changed (no undo step).
    pub changed: bool,
    /// Notes that went into a neighbour, and soft blips removed.
    pub merged: usize,
    pub removed: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quantized {
    pub snapshot: Snapshot,
    /// New indices of the aligned notes (for the selection).
    pub selected: Vec<usize>,
    /// False: the notes already lay on the grid – nothing was changed (no undo step).
    pub changed: bool,
}

/// File name without folder and extension.
pub fn stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 => base[..i].to_string(),
        _ => base.to_string(),
    }
}

/// Joins parts (seconds) of a recording together; short fades at cut edges.
fn splice(x: &[f32], sr: u32, parts: &[(f64, f64)]) -> Vec<f32> {
    let fade = (sr as f64 * CUT_FADE_SECS) as usize;
    let mut out = Vec::new();
    for &(a, b) in parts {
        let i0 = ((a * sr as f64).round().max(0.0) as usize).min(x.len());
        let i1 = ((b * sr as f64).round().max(0.0) as usize).min(x.len()).max(i0);
        let at = out.len();
        out.extend_from_slice(&x[i0..i1]);
        let n = i1 - i0;
        let f = fade.min(n / 2);
        if f == 0 {
            continue;
        }
        if i0 > 0 {
            for k in 0..f {
                out[at + k] *= k as f32 / f as f32;
            }
        }
        if i1 < x.len() {
            for k in 0..f {
                out[at + n - 1 - k] *= k as f32 / f as f32;
            }
        }
    }
    out
}

/// Grid of a converted melody (beats): sixteenths. Smoothing measures against it, too.
const MELODY_GRID: f64 = 0.25;

/// Grid of notes smoothed in a clip (beats) – the one recordings are converted to.
const SMOOTH_GRID: f64 = MELODY_GRID;

/// Detected melody → notes on the sixteenth grid; `spb` = seconds per beat of
/// the source. First smoothed (`smooth`, on the exact times – a fragment must
/// not grow to a grid step of its own), then how each note was played comes
/// along: velocity (dynamics, accents), a brighter attack for accents and
/// sforzati, and the length – staccato short (at most half the way to the
/// next note), legato up to it. Also returns how many detected notes
/// smoothing merged or removed.
fn melody_notes(a: &Analysis, spb: f64, shift: i32, smooth: Strength) -> (Vec<Note>, usize) {
    const GRID: f64 = MELODY_GRID;
    const FINE: f64 = 0.125;
    let snap = |b: f64, grid: f64| (b / grid).round() * grid;
    let rank = |k: Attack| match k {
        Attack::Normal => 0.0,
        Attack::Accent => 1.0,
        Attack::Sforzato => 2.0,
    };
    let items: Vec<smoothing::Item> = a
        .notes
        .iter()
        .map(|n| smoothing::Item {
            start: n.start_secs / spb,
            end: n.end_secs / spb,
            pitch: n.midi,
            velocity: n.velocity,
            accent: rank(n.attack),
            new_attack: Some(n.new_attack),
        })
        .collect();
    let plan = smoothing::plan(&items, None, smooth, smoothing::Scale { secs_per_beat: spb, grid_beats: GRID });
    let smoothed = plan.merged() + plan.removed.len();
    let mut notes: Vec<(Note, Articulation, f64)> = plan
        .groups
        .iter()
        .map(|g| {
            let main = &a.notes[g.main];
            let start = snap(g.start, GRID).max(0.0);
            let end = snap(g.end, GRID).max(start + GRID);
            let mut note = Note::new((main.midi as i32 + shift).clamp(0, 127) as u8, start, end - start, g.velocity.max(1));
            // the attack from where the note starts (accent rank, see above)
            note.brightness = match g.accent {
                r if r >= 2.0 => expression::SFORZATO_BRIGHTNESS,
                r if r >= 1.0 => expression::ACCENT_BRIGHTNESS,
                _ => 0.0,
            };
            // how it ends: the articulation of its last part
            let last = g.members.last().map_or(main, |&i| &a.notes[i]);
            (note, last.articulation, g.end - g.start)
        })
        .collect();
    notes.sort_by(|a, b| a.0.start_beats.total_cmp(&b.0.start_beats));
    // the same start position: keep the louder note
    notes.dedup_by(|b, a| {
        let same = (a.0.start_beats - b.0.start_beats).abs() < 1e-9;
        if same && b.0.velocity > a.0.velocity {
            std::mem::swap(a, b);
        }
        same
    });
    let starts: Vec<f64> = notes.iter().map(|n| n.0.start_beats).collect();
    let notes = notes
        .into_iter()
        .enumerate()
        .map(|(i, (mut note, articulation, sounding))| {
            let next = starts.get(i + 1).copied();
            match (articulation, next) {
                (Articulation::Legato, Some(nx)) => note.length_beats = nx - note.start_beats,
                (Articulation::Staccato, _) => {
                    let short = snap(sounding, FINE).max(FINE);
                    let half = next.map_or(short, |nx| ((nx - note.start_beats) * 0.5).max(FINE));
                    note.length_beats = short.min(half);
                }
                _ => {}
            }
            // never into the next note
            if let Some(nx) = next {
                note.length_beats = note.length_beats.min(nx - note.start_beats);
            }
            note
        })
        .collect();
    (notes, smoothed)
}

/// Undo label of smoothing with a strength.
pub fn smooth_label(s: Strength) -> String {
    match s {
        Strength::Off => tr!("Smooth notes"),
        Strength::Light => tr!("Smooth notes (light)"),
        Strength::Medium => tr!("Smooth notes (medium)"),
        Strength::Strong => tr!("Smooth notes (strong)"),
    }
}

/// Detected hits → drum notes (sixteenth notes); the strongest per drum and step.
/// Plus bass drums from the low band that the general onset detection overlooks.
fn rhythm_notes(a: &Analysis, samples: &[f32], sr: f32, spb: f64) -> Vec<Note> {
    let mut best: BTreeMap<(i64, u8), u8> = BTreeMap::new();
    let mut put = |time: f32, drum: hits::Drum, strength: f32| {
        let step = (time as f64 / spb / 0.25).round().max(0.0) as i64;
        let vel = (40.0 + 87.0 * strength.clamp(0.0, 1.0)) as u8;
        let e = best.entry((step, drum.midi())).or_insert(0);
        *e = (*e).max(vel);
    };
    let found: Vec<_> = a.hits.iter().filter(|h| h.strength >= MIN_HIT_STRENGTH).collect();
    let spans: Vec<(f32, f32, Option<f32>)> = found.iter().map(|h| (h.time, h.end, h.midi)).collect();
    let mut kicks = Vec::new();
    for (h, drums) in found.iter().zip(hits::classify_hits(samples, sr, &spans)) {
        for d in drums {
            if d == hits::Drum::Kick {
                kicks.push(h.time);
            }
            put(h.time, d, h.strength);
        }
    }
    for (t, strength) in hits::low_onsets(samples, sr) {
        if !kicks.iter().any(|k| (k - t).abs() < 0.05) {
            put(t, hits::Drum::Kick, strength);
        }
    }
    best.into_iter().map(|((step, pitch), vel)| Note::new(pitch, step as f64 * 0.25, 0.25, vel)).collect()
}

impl AppState {
    /// Inserts audio files (name, content or read error). Onto an audio track
    /// (`track_id`) the first file goes into it, every further one gets its own
    /// track directly below; otherwise (instrument track: below, no track: at
    /// the end) a new audio track per file. All from `start_beats`.
    pub fn import_audio(&self, files: Vec<(String, Result<Vec<u8>, String>)>, track_id: Option<Id>, start_beats: f64) -> CmdResult<ImportResult> {
        let mut decoded = Vec::new();
        let mut errors = Vec::new();
        for (name, bytes) in files {
            let shown = name.rsplit(['/', '\\']).next().unwrap_or(&name).to_string();
            match bytes.and_then(|b| audiofile::decode(&b)) {
                Ok(d) if d.duration_secs() > MAX_IMPORT_SECS => errors.push(tr!("\"{name}\": longer than 30 minutes", name = shown)),
                Ok(d) if d.samples.len() < 64 => errors.push(tr!("\"{name}\": too short", name = shown)),
                Ok(d) => decoded.push((stem(&name), d)),
                Err(e) => errors.push(tr!("\"{name}\": {error}", name = shown, error = e)),
            }
        }
        if decoded.is_empty() {
            return Err(if errors.is_empty() { tr!("No files") } else { errors.join("\n") });
        }
        let start = if start_beats.is_finite() { start_beats.max(0.0) } else { 0.0 };
        let (cmd, imported) = {
            let mut s = lock(&self.session)?;
            let mut pool = lock(&self.pool)?;
            let p = &mut s.project;
            let bpm = p.tempo_bpm;
            let target = track_id.and_then(|id| p.track_index(id)).map(|i| (i, p.tracks[i].id, p.tracks[i].kind == TrackKind::Audio));
            let mut insert_at = target.map(|(i, ..)| i + 1).unwrap_or(p.tracks.len());
            let (mut commands, mut imported) = (Vec::new(), Vec::new());
            for (k, (name, d)) in decoded.into_iter().enumerate() {
                let seconds = d.duration_secs();
                let (format, channels) = (d.format.clone(), d.channels);
                let asset_id = pool.insert(name.clone(), d.samples, d.sample_rate);
                let clip_id = p.alloc_id();
                // Original tempo: the clip is not stretched when the project tempo changes
                // ("Detect tempo" in the clip window turns on following it).
                let clip = Clip { id: clip_id, name: name.clone(), start_beats: start, source: ClipSource::Audio { asset_id, source_bpm: bpm, warp: false, duration_secs: seconds } };
                let track_id = match target {
                    Some((_, tid, true)) if k == 0 => {
                        commands.push(Command::AddClip { track_id: tid, clip });
                        tid
                    }
                    _ => {
                        let tid = p.alloc_id();
                        let mut t = Track::new(tid, name.clone(), TrackKind::Audio);
                        t.clips.push(clip);
                        commands.push(Command::AddTrack { track: t, index: Some(insert_at) });
                        insert_at += 1;
                        tid
                    }
                };
                imported.push(Imported { track_id, clip_id, name, format, seconds, channels });
            }
            let label = match imported.as_slice() {
                [one] => tr!("Insert audio: {name}", name = one.name),
                many => trn!(many.len(), "Insert {n} audio file", "Insert {n} audio files"),
            };
            (Command::Batch { label, commands }, imported)
        };
        let snapshot = self.execute(cmd, false)?;
        Ok(ImportResult { snapshot, imported, errors })
    }

    /// Detected melody or hits of audio clips as a new instrument track
    /// directly below the audio track (one clip per audio clip, in the same
    /// place, length rounded to whole bars). `clip_id`: None = all audio clips of the track.
    /// `smooth`: how much a detected melody is smoothed first (drums aren't).
    pub fn audio_to_notes(&self, track_id: Id, clip_id: Option<Id>, target: Target, label: &str, smooth: Strength) -> CmdResult<Converted> {
        let (track, index, bpm, bpb) = {
            let s = lock(&self.session)?;
            let p = &s.project;
            let i = p.track_index(track_id).ok_or_else(|| tr!("Track {id} not found", id = track_id))?;
            (p.tracks[i].clone(), i, p.tempo_bpm, p.beats_per_bar.max(1) as f64)
        };
        let mut clips: Vec<&Clip> = track
            .clips
            .iter()
            .filter(|c| matches!(c.source, ClipSource::Audio { .. }) && clip_id.is_none_or(|id| id == c.id))
            .collect();
        if clips.is_empty() {
            return Err(match clip_id {
                Some(id) => tr!(
                    "Clip {id} in \"{name}\" is not an audio clip – only recordings and inserted audio files can be converted.",
                    id = id,
                    name = track.name
                ),
                None => tr!("\"{name}\" contains no audio clips.", name = track.name),
            });
        }
        clips.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats));
        let shift = track.pitch_semitones.round() as i32;
        let mut out = Vec::new();
        let mut pitches = Vec::new();
        let mut smoothed = 0;
        for clip in &clips {
            let ClipSource::Audio { asset_id, .. } = &clip.source else { continue };
            let a = self.analysis_for(asset_id)?;
            let spb = 1.0 / edit::audio_beats_per_sec(&clip.source, bpm);
            let notes = match target {
                Target::Melodie => {
                    let (notes, n) = melody_notes(&a, spb, shift, smooth);
                    smoothed += n;
                    notes
                }
                Target::Rhythmus => {
                    let (samples, sr) = {
                        let pool = lock(&self.pool)?;
                        let asset = pool.get(asset_id).ok_or_else(|| tr!("Audio file not found"))?;
                        (asset.samples.clone(), asset.sample_rate)
                    };
                    rhythm_notes(&a, &samples, sr as f32, spb)
                }
            };
            if notes.is_empty() {
                continue;
            }
            pitches.extend(notes.iter().map(|n| n.pitch));
            let length_beats = (clip.length_beats(bpm) / bpb - 1e-9).ceil().max(1.0) * bpb;
            let name = match (target, &a.key) {
                (Target::Melodie, Some(k)) => tr!("Melody · {key}", key = k.name),
                (Target::Melodie, None) => tr!("Melody"),
                (Target::Rhythmus, _) => match a.tempo_estimate {
                    Some(t) => tr!("Rhythm · ≈ {bpm} BPM", bpm = t.round()),
                    None => tr!("Rhythm"),
                },
            };
            out.push(Clip { id: Id::NEW, name, start_beats: clip.start_beats, source: ClipSource::Notes { length_beats, notes } });
        }
        if out.is_empty() {
            return Err(match target {
                Target::Melodie => tr!("No melody was detected in it – for drums, use \"Convert to rhythm track\"."),
                Target::Rhythmus => tr!("No hits were detected in it."),
            });
        }
        pitches.sort_unstable();
        let median = pitches[pitches.len() / 2];
        let kind = match target {
            Target::Rhythmus => InstrumentKind::Drums,
            Target::Melodie if median < 48 => InstrumentKind::Bass,
            Target::Melodie => InstrumentKind::Pluck,
        };
        let base = if clips.len() == 1 { clips[0].name.clone() } else { track.name.clone() };
        let track_name = match (target, kind) {
            (Target::Rhythmus, _) => tr!("Rhythm from {name}", name = base),
            (_, InstrumentKind::Bass) => tr!("Bass from {name}", name = base),
            _ => tr!("Melody from {name}", name = base),
        };
        let notes: usize = out.iter().map(|c| match &c.source { ClipSource::Notes { notes, .. } => notes.len(), _ => 0 }).sum();
        let (cmd, new_id) = {
            let mut s = lock(&self.session)?;
            let p = &mut s.project;
            let new_id = p.alloc_id();
            let mut t = Track::new(new_id, track_name, TrackKind::Instrument);
            t.instrument = Some(sound::default_instrument(kind));
            for mut c in out {
                c.id = p.alloc_id();
                t.clips.push(c);
            }
            (Command::Batch { label: label.into(), commands: vec![Command::AddTrack { track: t, index: Some(index + 1) }] }, new_id)
        };
        let snapshot = self.execute(cmd, false)?;
        Ok(Converted { snapshot, track_id: new_id, notes, smoothed, track_ids: vec![new_id], voices: 1 })
    }

    /// Smooths the notes of a clip (`indices`: None = all): unusually short
    /// notes go into a neighbour, consecutive notes of the same pitch become
    /// one unless the second is louder or accented (see `klang_core::smooth`).
    /// Nothing to smooth: no undo step.
    pub fn smooth_notes(&self, track_id: Id, clip_id: Id, indices: Option<Vec<usize>>, strength: Strength, label: Option<&str>) -> CmdResult<Smoothed> {
        let (clip, drums, bpm) = {
            let s = lock(&self.session)?;
            let p = &s.project;
            let clip = p.clip(track_id, clip_id).cloned().ok_or_else(|| tr!("Clip {clip} not found on track {track}", clip = clip_id, track = track_id))?;
            let drums = p.track(track_id).and_then(|t| t.instrument.as_ref()).is_some_and(|i| i.kind == InstrumentKind::Drums);
            (clip, drums, p.tempo_bpm)
        };
        let ClipSource::Notes { notes, .. } = &clip.source else {
            return Err(tr!("Smoothing only works on note clips."));
        };
        if drums {
            return Err(tr!("Drum notes are single hits – smoothing is meant for melodies, e.g. from a recording."));
        }
        if let Some(bad) = indices.as_ref().and_then(|ix| ix.iter().find(|&&i| i >= notes.len())) {
            return Err(trn!(
                notes.len(),
                "Note {nr} doesn't exist in this clip (it has {n} note).",
                "Note {nr} doesn't exist in this clip (it has {n} notes).",
                nr = bad
            ));
        }
        let scale = smoothing::Scale { secs_per_beat: 60.0 / bpm.max(1.0), grid_beats: SMOOTH_GRID };
        let (smoothed, selected, plan) = smoothing::smooth_notes(notes, indices.as_deref(), strength, scale);
        if !plan.changed() {
            let selected = indices.unwrap_or_else(|| (0..notes.len()).collect());
            return Ok(Smoothed { snapshot: self.snapshot()?, selected, changed: false, merged: 0, removed: 0 });
        }
        let label = label.map(str::to_string).unwrap_or_else(|| smooth_label(strength));
        let cmd = Command::Batch { label, commands: vec![Command::SetClipNotes { track_id, clip_id, notes: smoothed, length_beats: None }] };
        Ok(Smoothed { snapshot: self.execute(cmd, false)?, selected, changed: true, merged: plan.merged(), removed: plan.removed.len() })
    }

    /// Deletes the time range `[start, end)` (beats) from all tracks; everything
    /// after it moves up. The selection is cleared afterwards.
    pub fn delete_range(&self, start: f64, end: f64) -> CmdResult<Snapshot> {
        let (start, end) = (start.min(end), start.max(end));
        let project = lock(&self.session)?.project.clone();
        let mut cut = |clip: &Clip, keep: &AudioKeep| -> Result<ClipSource, String> {
            let ClipSource::Audio { source_bpm, warp, .. } = &clip.source else {
                return Err(tr!("No audio clip"));
            };
            let mut pool = lock(&self.pool)?;
            let asset = pool.get(&keep.asset_id).ok_or_else(|| tr!("Audio file not found"))?;
            let sr = asset.sample_rate;
            let samples = splice(&asset.samples, sr, &keep.parts);
            let name = asset.name.clone();
            let secs = samples.len() as f64 / sr.max(1) as f64;
            let asset_id = pool.insert(name, samples, sr);
            Ok(ClipSource::Audio { asset_id, source_bpm: *source_bpm, warp: *warp, duration_secs: secs })
        };
        let cmd = edit::delete_range(&project, start, end, &mut cut).map_err(|e| e.to_string())?;
        self.execute(cmd, false)?;
        self.set_loop(None, Some(None))
    }

    /// Aligns the notes of a clip to the grid (`indices`: None = all).
    pub fn quantize_notes(&self, track_id: Id, clip_id: Id, indices: Option<Vec<usize>>, grid: f64, lengths: bool, label: Option<&str>) -> CmdResult<Quantized> {
        let (clip, beat_unit) = {
            let s = lock(&self.session)?;
            let clip = s.project.clip(track_id, clip_id).cloned();
            (clip.ok_or_else(|| tr!("Clip {clip} not found on track {track}", clip = clip_id, track = track_id))?, s.project.beat_unit)
        };
        let ClipSource::Notes { notes, .. } = &clip.source else {
            return Err(tr!("Quantizing only works on note clips."));
        };
        if let Some(bad) = indices.as_ref().and_then(|ix| ix.iter().find(|&&i| i >= notes.len())) {
            return Err(trn!(
                notes.len(),
                "Note {nr} doesn't exist in this clip (it has {n} note).",
                "Note {nr} doesn't exist in this clip (it has {n} notes).",
                nr = bad
            ));
        }
        let (quantized, selected) = edit::quantize_notes(notes, indices.as_deref(), clip.start_beats, grid, lengths).map_err(|e| e.to_string())?;
        // already on the grid (the result is only sorted): no empty undo step
        let mut before = notes.clone();
        before.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
        let same = |a: &Note, b: &Note| a.pitch == b.pitch && (a.start_beats - b.start_beats).abs() < 1e-9 && (a.length_beats - b.length_beats).abs() < 1e-9;
        if before.len() == quantized.len() && before.iter().zip(&quantized).all(|(a, b)| same(a, b)) {
            let selected = indices.unwrap_or_else(|| (0..notes.len()).collect());
            return Ok(Quantized { snapshot: self.snapshot()?, selected, changed: false });
        }
        let label = label.map(str::to_string).unwrap_or_else(|| {
            let grid = edit::grid_name(grid, beat_unit);
            if lengths {
                tr!("Quantize notes ({grid}, start and length)", grid = grid)
            } else {
                tr!("Quantize notes ({grid}, starts)", grid = grid)
            }
        });
        let cmd = Command::Batch { label, commands: vec![Command::SetClipNotes { track_id, clip_id, notes: quantized, length_beats: None }] };
        Ok(Quantized { snapshot: self.execute(cmd, false)?, selected, changed: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_strips_folders_and_extension() {
        assert_eq!(stem(r"C:\Musik\Mein Lied.final.wav"), "Mein Lied.final");
        assert_eq!(stem("/tmp/loop.flac"), "loop");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(stem("ohne_endung"), "ohne_endung");
    }

    #[test]
    fn drum_recording_becomes_kick_snare_hat_on_their_steps() {
        use klang_dsp::drums::{self, DrumKit};
        // two bars at 120 BPM: bass drum 1/3, snare 2/4, hi-hat on the eighths in between
        let sr = 44_100.0f32;
        let kit = DrumKit::default();
        let step = 0.125f32;
        let mut x = vec![0.0f32; (4.5 * sr) as usize];
        let mut put = |midi: u8, steps: &[usize]| {
            for (k, &s) in steps.iter().enumerate() {
                let hit = drums::render_hit(&kit, midi, 0.9, sr, k as u32 + midi as u32).unwrap();
                let a = (s as f32 * step * sr) as usize;
                for (i, v) in hit.iter().enumerate() {
                    if let Some(o) = x.get_mut(a + i) {
                        *o += v;
                    }
                }
            }
        };
        put(drums::KICK, &[0, 8, 16, 24]);
        put(drums::SNARE, &[4, 12, 20, 28]);
        put(drums::HAT_CLOSED, &[2, 6, 10, 14, 18, 22, 26, 30]);
        let a = crate::analysis::analyze("beat", &x, sr as u32);
        let notes = rhythm_notes(&a, &x, sr, 0.5);
        let steps = |p: u8| notes.iter().filter(|n| n.pitch == p).map(|n| (n.start_beats * 4.0).round() as usize).collect::<Vec<_>>();
        assert_eq!(steps(drums::KICK), vec![0, 8, 16, 24], "{notes:?}");
        assert_eq!(steps(drums::SNARE), vec![4, 12, 20, 28]);
        assert_eq!(steps(drums::HAT_CLOSED), vec![2, 6, 10, 14, 18, 22, 26, 30]);
    }

    #[test]
    fn splice_joins_parts_and_fades_only_at_cuts() {
        let sr = 1000; // 3 ms fade = 3 samples
        let x = vec![1.0f32; 100];
        let y = splice(&x, sr, &[(0.0, 0.02), (0.05, 0.1)]);
        assert_eq!(y.len(), 20 + 50);
        assert_eq!(y[0], 1.0, "Anfang der Datei bleibt");
        assert!(y[19] < 0.5 && y[20] < 0.5, "Schnittkante geblendet");
        assert_eq!(y[69], 1.0, "Ende der Datei bleibt");
    }

    /// A played melody: (MIDI, seconds sounding, amplitude, attack gain, silence after).
    /// The attack gain falls back to 1 within about 30 ms (sforzato: 4 = +12 dB);
    /// without silence after it, the next note follows phase-continuously (legato).
    fn perform(notes: &[(f32, f32, f32, f32, f32)], sr: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let mut phase = 0.0f32;
        for &(midi, secs, amp, accent, rest) in notes {
            let f = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
            for i in 0..(secs * sr) as usize {
                let t = i as f32 / sr;
                phase += 2.0 * std::f32::consts::PI * f / sr;
                let fade_in = if out.last().is_some_and(|v: &f32| v.abs() > 0.0) && rest == 0.0 { 1.0 } else { (t / 0.01).min(1.0) };
                let fade_out = if rest > 0.0 { ((secs - t) / 0.015).clamp(0.0, 1.0) } else { 1.0 };
                let gain = 1.0 + (accent - 1.0) * (-t / 0.03).exp();
                out.push(amp * gain * fade_in * fade_out * (0.6 * phase.sin() + 0.25 * (2.0 * phase).sin() + 0.1 * (3.0 * phase).sin()));
            }
            out.extend(std::iter::repeat(0.0).take((rest * sr) as usize));
        }
        out
    }

    #[test]
    fn played_dynamics_accents_and_articulation_carry_into_the_notes() {
        let sr = 44_100.0;
        // 120 BPM, a note per beat: legato pair, sforzato, two soft notes (−12 dB), staccato
        let x = perform(
            &[
                (60.0, 0.5, 0.3, 1.0, 0.0),   // legato into the next
                (64.0, 0.44, 0.3, 1.0, 0.06),
                (67.0, 0.44, 0.3, 4.0, 0.06), // sforzato
                (72.0, 0.44, 0.3, 1.0, 0.06),
                (67.0, 0.44, 0.075, 1.0, 0.06), // soft
                (64.0, 0.44, 0.075, 1.0, 0.06), // soft
                (62.0, 0.12, 0.3, 1.0, 0.38), // staccato
                (60.0, 0.44, 0.3, 1.0, 0.06),
                (64.0, 0.44, 0.3, 1.0, 0.06),
                (60.0, 0.8, 0.3, 1.0, 0.2),
            ],
            sr,
        );
        let a = crate::analysis::analyze("t", &x, sr as u32);
        let midis: Vec<u8> = a.notes.iter().map(|n| n.midi).collect();
        assert_eq!(midis, vec![60, 64, 67, 72, 67, 64, 62, 60, 64, 60], "{:?}", a.notes);
        let attacks: Vec<Attack> = a.notes.iter().map(|n| n.attack).collect();
        assert_eq!(attacks[2], Attack::Sforzato, "{attacks:?}");
        assert!(attacks.iter().enumerate().all(|(i, k)| i == 2 || *k == Attack::Normal), "{attacks:?}");
        assert_eq!(a.notes[0].articulation, Articulation::Legato);
        assert_eq!(a.notes[6].articulation, Articulation::Staccato);
        assert!([1, 2, 3, 4, 5, 7, 8].iter().all(|&i| a.notes[i].articulation == Articulation::Normal), "{:?}", a.notes.iter().map(|n| n.articulation).collect::<Vec<_>>());
        let vel: Vec<u8> = a.notes.iter().map(|n| n.velocity).collect();
        assert!((74..=86).contains(&vel[3]), "a normal note is about mf: {vel:?}");
        assert!(vel[2] >= 108, "sforzato: {vel:?}");
        assert!(vel[4] <= 50 && vel[5] <= 50, "soft notes about p/pp: {vel:?}");

        let (notes, _) = melody_notes(&a, 0.5, 0, Strength::Medium);
        assert_eq!(notes.len(), 10);
        assert_eq!(notes.iter().map(|n| n.start_beats).collect::<Vec<_>>(), (0..10).map(|i| i as f64).collect::<Vec<_>>());
        assert_eq!(notes[2].brightness, expression::SFORZATO_BRIGHTNESS, "harder, brighter attack");
        assert!(notes.iter().enumerate().all(|(i, n)| i == 2 || n.brightness == 0.0));
        assert_eq!(notes[2].velocity, vel[2]);
        assert_eq!(notes[0].length_beats, 1.0, "legato: up to the next note");
        assert_eq!(notes[6].length_beats, 0.25, "staccato: short");
        assert!(notes[3].length_beats >= 0.75 && notes[3].length_beats <= 1.0, "{}", notes[3].length_beats);
        // a clean recording: smoothing changes nothing
        assert_eq!(melody_notes(&a, 0.5, 0, Strength::Strong), (notes.clone(), 0));
        assert_eq!(melody_notes(&a, 0.5, 0, Strength::Off).0, notes);
    }

    /// A sung line the way pitch detection breaks it up: each part is
    /// (MIDI, seconds, kind) – kinds: 0 tone, 1 noise at the same level (a
    /// breathy voice break), 2 silence. Phase-continuous between tone parts.
    fn sung_with_flaws(parts: &[(f32, f32, u8)], sr: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let mut phase = 0.0f32;
        let mut noise = 0x2545_f491u32;
        let mut since_rest = 0.0f32;
        for &(midi, secs, kind) in parts {
            let f = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
            for _ in 0..(secs * sr) as usize {
                let env = (since_rest / 0.015).min(1.0);
                since_rest += 1.0 / sr;
                out.push(match kind {
                    0 => {
                        phase += 2.0 * std::f32::consts::PI * f / sr;
                        env * (0.4 * phase.sin() + 0.15 * (2.0 * phase).sin() + 0.06 * (3.0 * phase).sin())
                    }
                    1 => {
                        noise ^= noise << 13;
                        noise ^= noise >> 17;
                        noise ^= noise << 5;
                        0.3 * env * (noise as f32 / u32::MAX as f32 * 2.0 - 1.0) * 1.7
                    }
                    _ => {
                        since_rest = 0.0;
                        0.0
                    }
                });
            }
        }
        out
    }

    #[test]
    fn smoothing_puts_a_broken_up_recording_back_together() {
        let sr = 44_100.0;
        // 120 BPM, two beats per note: c' with an octave slip, e' with a breathy
        // voice break, a scoop d' → g', then d' d' re-struck after a breath, c'
        let x = sung_with_flaws(
            &[
                (60.0, 0.44, 0), (72.0, 0.13, 0), (60.0, 0.39, 0), (0.0, 0.04, 2),
                (64.0, 0.47, 0), (64.0, 0.05, 1), (64.0, 0.44, 0), (0.0, 0.04, 2),
                (62.0, 0.11, 0), (67.0, 0.85, 0), (0.0, 0.04, 2),
                (62.0, 0.44, 0), (0.0, 0.06, 2), (62.0, 0.44, 0), (0.0, 0.06, 2),
                (60.0, 0.9, 0), (0.0, 0.1, 2),
            ],
            sr,
        );
        let a = crate::analysis::analyze("t", &x, sr as u32);
        let (raw, none) = melody_notes(&a, 0.5, 0, Strength::Off);
        let pitches = |n: &[Note]| n.iter().map(|n| n.pitch).collect::<Vec<_>>();
        assert_eq!(none, 0);
        assert!(raw.len() >= 9, "broken up without smoothing: {:?}", pitches(&raw));
        assert!(pitches(&raw).contains(&72), "the octave slip is a note of its own: {:?}", pitches(&raw));

        let (smooth, merged) = melody_notes(&a, 0.5, 0, Strength::Medium);
        assert_eq!(pitches(&smooth), vec![60, 64, 67, 62, 62, 60], "{:?}", a.notes.iter().map(|n| (n.midi, n.start_secs, n.end_secs, n.new_attack)).collect::<Vec<_>>());
        assert_eq!(merged, raw.len() - smooth.len());
        let starts: Vec<f64> = smooth.iter().map(|n| n.start_beats).collect();
        assert_eq!(starts[..3], [0.0, 2.0, 4.0], "each held note from its start: {smooth:?}");
        assert!(smooth[0].length_beats >= 1.75 && smooth[1].length_beats >= 1.75, "held through: {smooth:?}");
        assert!(smooth[2].length_beats >= 1.75, "the scoop belongs to the g': {smooth:?}");
    }

    #[test]
    fn smoothing_a_clip_is_one_undo_step_and_leaves_drums_alone() {
        let st = AppState::without_audio();
        let notes = vec![Note::new(60, 0.0, 0.9, 80), Note::new(72, 0.9, 0.18, 80), Note::new(60, 1.08, 0.92, 80), Note::new(64, 2.0, 1.0, 80)];
        let clip = Clip { id: Id::NEW, name: "m".into(), start_beats: 0.0, source: ClipSource::Notes { length_beats: 4.0, notes: notes.clone() } };
        let mut track = Track::new(Id::NEW, "Melody", TrackKind::Instrument);
        track.clips.push(clip);
        st.execute(Command::AddTrack { track, index: None }, false).unwrap();
        let (tid, cid) = {
            let s = lock(&st.session).unwrap();
            let t = s.project.tracks.last().unwrap();
            (t.id, t.clips[0].id)
        };
        let clip_notes = |st: &AppState| match &st.clip(tid, cid).unwrap().source {
            ClipSource::Notes { notes, .. } => notes.clone(),
            _ => unreachable!(),
        };
        let r = st.smooth_notes(tid, cid, None, Strength::Medium, None).unwrap();
        assert!(r.changed);
        assert_eq!((r.merged, r.removed), (2, 0));
        let got = clip_notes(&st);
        assert_eq!(got.iter().map(|n| (n.pitch, n.start_beats, n.length_beats)).collect::<Vec<_>>(), vec![(60, 0.0, 2.0), (64, 2.0, 1.0)]);
        let undo_label = |st: &AppState| lock(&st.session).unwrap().undo_label().map(str::to_string);
        assert_eq!(undo_label(&st).as_deref(), Some("Smooth notes (medium)"));
        // once more: nothing left to smooth, no empty undo step
        let again = st.smooth_notes(tid, cid, None, Strength::Strong, None).unwrap();
        assert!(!again.changed);
        assert_eq!(undo_label(&st).as_deref(), Some("Smooth notes (medium)"));
        st.undo().unwrap();
        assert_eq!(clip_notes(&st), notes, "undo brings the fragments back");
        // drum notes are hits, not a melody
        let mut kit = Track::new(Id::NEW, "Drums", TrackKind::Instrument);
        kit.instrument = Some(sound::default_instrument(InstrumentKind::Drums));
        kit.clips.push(Clip { id: Id::NEW, name: "d".into(), start_beats: 0.0, source: ClipSource::Notes { length_beats: 4.0, notes: vec![Note::new(42, 0.0, 0.25, 90), Note::new(42, 0.25, 0.25, 90)] } });
        st.execute(Command::AddTrack { track: kit, index: None }, false).unwrap();
        let (dt, dc) = {
            let s = lock(&st.session).unwrap();
            let t = s.project.tracks.last().unwrap();
            (t.id, t.clips[0].id)
        };
        assert!(st.smooth_notes(dt, dc, None, Strength::Medium, None).is_err());
        assert!(st.smooth_notes(tid, cid, Some(vec![9]), Strength::Medium, None).is_err(), "a note that isn't there");
    }

    #[test]
    fn plucked_notes_decay_without_counting_as_accents() {
        // the melody of the end-to-end tests, twice: (MIDI, amplitude, attack gain, sounding seconds)
        let sr = 48_000.0f32;
        let melody = [
            (60.0, 1.0, 1.0, 0.46), (64.0, 1.0, 1.0, 0.46), (67.0, 1.0, 1.0, 0.46), (72.0, 1.0, 1.0, 0.14),
            (67.0, 1.0, 1.0, 0.46), (64.0, 1.0, 1.0, 0.46), (65.0, 1.0, 1.0, 0.46), (69.0, 1.0, 4.0, 0.46),
            (67.0, 1.0, 1.0, 0.46), (62.0, 0.25, 1.0, 0.46), (59.0, 0.25, 1.0, 0.46), (60.0, 1.0, 1.0, 0.46),
        ];
        let mut x = Vec::new();
        for _ in 0..2 {
            for &(midi, amp, accent, secs) in &melody {
                let f = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
                for i in 0..(0.5 * sr) as usize {
                    let t = i as f32 / sr;
                    let gain = amp * (1.0 + (accent - 1.0) * (-t / 0.03).exp());
                    let env = (t / 0.005).min(1.0) * (-t * 2.0).exp() * ((secs - t) / 0.02).clamp(0.0, 1.0);
                    let w = 2.0 * std::f32::consts::PI * f * t;
                    x.push(gain * env * (0.15 * w.sin() + 0.056 * (2.0 * w).sin() + 0.022 * (3.0 * w).sin()));
                }
            }
        }
        let a = crate::analysis::analyze("t", &x, sr as u32);
        let got: Vec<(u8, Attack, Articulation, u8)> = a.notes.iter().map(|n| (n.midi, n.attack, n.articulation, n.velocity)).collect();
        assert_eq!(a.notes.len(), 24, "{got:?}");
        for (i, n) in a.notes.iter().enumerate() {
            let k = i % 12;
            assert_eq!(n.attack == Attack::Sforzato, k == 7, "{i}: {got:?}");
            assert!(n.attack != Attack::Accent, "{i}: {got:?}");
            assert_eq!(n.articulation == Articulation::Staccato, k == 3, "{i}: {got:?}");
            assert_ne!(n.articulation, Articulation::Legato, "40 ms breaks between plucked notes: {i}: {got:?}");
            if k == 9 || k == 10 {
                assert!(n.velocity < 50, "{i}: {got:?}");
            }
        }
    }
}
