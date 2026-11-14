//! Audio und Noten über Spuren hinweg:
//!
//! * **Audiodateien einfügen** (Drag & Drop, Agent): je Datei eine Audiospur
//!   mit Clip – oder, auf eine Audiospur gezogen, die erste Datei dort hinein.
//! * **Aufnahme → Noten**: erkannte Melodie (Instrumentspur) oder erkannte
//!   Anschläge (Schlagzeug-Spur) direkt unter der Audiospur.
//! * **Zeitbereich löschen** über alle Spuren mit Aufrücken; Audio wird dabei
//!   geschnitten (neue Datei im Klangspeicher, die alte bleibt für Rückgängig).
//! * **Noten am Raster ausrichten** (Quantisieren).
//!
//! Die Planung liegt im Kern (`klang_core::edit`), hier nur, was Audiodaten
//! oder die Analyse braucht. Jede Aktion ist ein Rückgängig-Schritt.

use crate::analysis::Analysis;
use crate::audiofile;
use crate::sound;
use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::edit::{self, AudioKeep};
use klang_core::{Clip, ClipSource, Command, Id, InstrumentKind, Note, Track, TrackKind};
use klang_dsp::hits;
use klang_i18n::{tr, trn};
use klang_music::theory::{quantize, TimedNote};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Längere Dateien werden abgelehnt (Klangwelt hält Audio im Speicher).
pub const MAX_IMPORT_SECS: f64 = 30.0 * 60.0;
/// Anschläge darunter (relativ zum lautesten) gelten als Rauschen.
const MIN_HIT_STRENGTH: f32 = 0.05;
/// Kurze Blenden an Schnittkanten gegen Knacken (Sekunden).
const CUT_FADE_SECS: f64 = 0.003;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub track_id: Id,
    pub clip_id: Id,
    pub name: String,
    /// Format in Worten („WAV 24 Bit“).
    pub format: String,
    pub seconds: f64,
    /// Kanäle der Datei (Klangwelt mittelt zu mono).
    pub channels: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub snapshot: Snapshot,
    pub imported: Vec<Imported>,
    /// Dateien, die sich nicht lesen ließen (mit Grund).
    pub errors: Vec<String>,
}

/// Wohin eine Aufnahme umgewandelt wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// Erkannte Töne als Instrumentspur (tiefe Lagen: Bass).
    #[serde(alias = "melody", alias = "noten", alias = "notes")]
    Melodie,
    /// Erkannte Anschläge als Schlagzeug-Spur (Bassdrum, Snare, Hi-Hat).
    #[serde(alias = "rhythm", alias = "drums", alias = "schlagzeug")]
    Rhythmus,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Converted {
    pub snapshot: Snapshot,
    pub track_id: Id,
    /// Anzahl erzeugter Noten.
    pub notes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quantized {
    pub snapshot: Snapshot,
    /// Neue Indizes der ausgerichteten Noten (für die Auswahl).
    pub selected: Vec<usize>,
}

/// Dateiname ohne Ordner und Endung.
pub fn stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 => base[..i].to_string(),
        _ => base.to_string(),
    }
}

/// Teile (Sekunden) einer Aufnahme aneinanderhängen; an Schnittkanten kurz blenden.
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

/// Erkannte Melodie → Noten (Sechzehntel-Raster); `spb` = Sekunden je Schlag der Quelle.
fn melody_notes(a: &Analysis, spb: f64, shift: i32) -> Vec<Note> {
    let timed: Vec<TimedNote> = a
        .notes
        .iter()
        .map(|n| TimedNote { start: n.start_secs, end: n.end_secs, midi: n.midi, cents: 0.0, velocity: n.velocity })
        .collect();
    quantize(&timed, spb, 0.25)
        .iter()
        .map(|n| Note::new((n.midi as i32 + shift).clamp(0, 127) as u8, n.start_beats, n.length_beats, (40.0 + 87.0 * n.velocity.clamp(0.0, 1.0)) as u8))
        .collect()
}

/// Erkannte Anschläge → Schlagzeug-Noten (Sechzehntel); je Trommel und Schritt der stärkste.
/// Dazu Bassdrums aus dem tiefen Band, die die allgemeine Einsatzerkennung übersieht.
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
    /// Audiodateien (Name, Inhalt oder Lesefehler) einfügen. Auf eine Audiospur
    /// (`track_id`) kommt die erste Datei dort hinein, jede weitere bekommt eine
    /// eigene Spur direkt darunter; sonst (Instrumentspur: darunter, keine
    /// Spur: am Ende) je Datei eine neue Audiospur. Alle ab `start_beats`.
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
                // Originaltempo: der Clip wird nicht gedehnt, wenn sich das Projekttempo ändert
                // („Tempo erkennen“ im Clip-Fenster schaltet das Mitlaufen ein).
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

    /// Erkannte Melodie bzw. Anschläge von Audio-Clips als neue Instrumentspur
    /// direkt unter der Audiospur (ein Clip je Audio-Clip, an derselben Stelle,
    /// Länge auf ganze Takte). `clip_id`: None = alle Audio-Clips der Spur.
    pub fn audio_to_notes(&self, track_id: Id, clip_id: Option<Id>, target: Target, label: &str) -> CmdResult<Converted> {
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
        for clip in &clips {
            let ClipSource::Audio { asset_id, .. } = &clip.source else { continue };
            let a = self.analysis_for(asset_id)?;
            let spb = 1.0 / edit::audio_beats_per_sec(&clip.source, bpm);
            let notes = match target {
                Target::Melodie => melody_notes(&a, spb, shift),
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
        Ok(Converted { snapshot, track_id: new_id, notes })
    }

    /// Zeitbereich `[start, end)` (Schläge) aus allen Spuren löschen; alles
    /// danach rückt auf. Die Markierung ist danach aufgehoben.
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

    /// Noten eines Clips am Raster ausrichten (`indices`: None = alle).
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
        let label = label.map(str::to_string).unwrap_or_else(|| {
            let grid = edit::grid_name(grid, beat_unit);
            if lengths {
                tr!("Quantize notes ({grid}, start and length)", grid = grid)
            } else {
                tr!("Quantize notes ({grid}, starts)", grid = grid)
            }
        });
        let cmd = Command::Batch { label, commands: vec![Command::SetClipNotes { track_id, clip_id, notes: quantized, length_beats: None }] };
        Ok(Quantized { snapshot: self.execute(cmd, false)?, selected })
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
        // zwei Takte bei 120 BPM: Bassdrum 1/3, Snare 2/4, Hi-Hat auf den Achteln dazwischen
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
        let sr = 1000; // 3 ms Blende = 3 Samples
        let x = vec![1.0f32; 100];
        let y = splice(&x, sr, &[(0.0, 0.02), (0.05, 0.1)]);
        assert_eq!(y.len(), 20 + 50);
        assert_eq!(y[0], 1.0, "Anfang der Datei bleibt");
        assert!(y[19] < 0.5 && y[20] < 0.5, "Schnittkante geblendet");
        assert_eq!(y[69], 1.0, "Ende der Datei bleibt");
    }
}
