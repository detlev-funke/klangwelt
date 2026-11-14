//! Fugen-Fenster und Agent: Thema aus einem Clip, Antwort, Verwandlungen,
//! Engführungen, Gegensatz und Fugenplan; Material als Spuren schreiben und
//! mehrstimmig vorspielen; Kontrapunkt-Prüfer über alle Instrumentspuren.

use crate::harmony::{key_info, project_key, KeyInfo};
use crate::sound;
use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::{Clip, ClipSource, Command, Id, InstrumentKind, Note, Project, Track, TrackKind};
use klang_dsp::synth::NoteSpec;
use klang_dsp::util;
use klang_i18n::tr;
use klang_music::counterpoint::{check, IssueKind, Severity, VNote};
use klang_music::fugue::{self, Answer, Stretto, Transform};
use klang_music::theory::{estimate_key_from_histogram, Key};
use serde::{Deserialize, Serialize};

fn melodic(t: &Track) -> bool {
    t.kind == TrackKind::Instrument && sound::transposes(t.instrument.as_ref())
}

fn shift_of(t: &Track) -> i32 {
    if sound::transposes(t.instrument.as_ref()) { t.pitch_semitones.round() as i32 } else { 0 }
}

/// Noten eines Clips in Projektzeit, je Einsatz der oberste Ton.
fn clip_vnotes(t: &Track, c: &Clip) -> Vec<VNote> {
    let ClipSource::Notes { notes, length_beats } = &c.source else { return Vec::new() };
    let shift = shift_of(t);
    let mut v: Vec<VNote> = notes
        .iter()
        .filter(|n| n.start_beats < *length_beats)
        .map(|n| VNote::new((n.pitch as i32 + shift).clamp(0, 127) as u8, c.start_beats + n.start_beats, n.length_beats.min(length_beats - n.start_beats)))
        .collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.pitch.cmp(&a.pitch)));
    v.dedup_by(|b, a| (a.start - b.start).abs() < 1e-6);
    v
}

/// Thema einer Fuge: Noten eines Clips mit Herkunft.
pub struct Subject {
    pub track_name: String,
    pub clip_name: String,
    pub start: f64,
    pub notes: Vec<VNote>,
}

pub fn subject_of(st: &AppState, track_id: Id, clip_id: Id) -> CmdResult<Subject> {
    let s = lock(&st.session)?;
    let t = s.project.track(track_id).ok_or_else(|| tr!("Track {id} not found", id = track_id))?;
    if !melodic(t) {
        return Err(tr!("\"{name}\" is not a pitched instrument track", name = t.name));
    }
    let c = t.clips.iter().find(|c| c.id == clip_id).ok_or_else(|| tr!("Clip not found"))?;
    let notes = clip_vnotes(t, c);
    if notes.len() < 4 {
        return Err(tr!("The subject needs at least 4 notes – select a note clip containing the fugue subject."));
    }
    let start = notes[0].start;
    Ok(Subject { track_name: t.name.clone(), clip_name: c.name.clone(), start, notes: fugue::normalize(&notes) })
}

/// Tonart: gewählt, sonst aus dem Thema geschätzt (Grundton am Anfang/Ende zählt mehr), sonst aus dem Projekt.
fn subject_key(st: &AppState, notes: &[VNote], chosen: Option<Key>) -> CmdResult<KeyInfo> {
    if let Some(k) = chosen {
        return Ok(key_info(k, "gewaehlt"));
    }
    let mut hist = [0.0f64; 12];
    for (i, n) in notes.iter().enumerate() {
        let w = if i == 0 || i + 1 == notes.len() { 2.0 } else { 1.0 };
        hist[(n.pitch % 12) as usize] += n.len * w;
    }
    if let Some(k) = estimate_key_from_histogram(&hist) {
        return Ok(key_info(k, "geschaetzt"));
    }
    let p = project_key(&lock(&st.session)?.project);
    Ok(key_info(p.unwrap_or(Key { tonic: 0, minor: false, confidence: 0.0 }), if p.is_some() { "geschaetzt" } else { "standard" }))
}

pub fn bpb(st: &AppState) -> CmdResult<u32> {
    Ok(lock(&st.session)?.project.beats_per_bar.max(1))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformView {
    pub label: String,
    pub transform: Transform,
    pub notes: Vec<VNote>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectView {
    pub key: KeyInfo,
    pub track_name: String,
    pub clip_name: String,
    /// Einsatz des Themas im Projekt (Schläge).
    pub start: f64,
    /// Thema ab 0.
    pub notes: Vec<VNote>,
    pub length: f64,
    pub range: (u8, u8),
    pub answer: Answer,
    pub transforms: Vec<TransformView>,
    pub strettos: Vec<Stretto>,
    pub beats_per_bar: u32,
}

pub fn subject_view(st: &AppState, track_id: Id, clip_id: Id, key: Option<Key>) -> CmdResult<SubjectView> {
    let s = subject_of(st, track_id, clip_id)?;
    let key = subject_key(st, &s.notes, key)?;
    let bpb = bpb(st)?;
    let k = key.key;
    let transforms = [Transform::Umkehrung, Transform::Krebs, Transform::Krebsumkehrung, Transform::Vergroesserung, Transform::Verkleinerung]
        .into_iter()
        .map(|t| TransformView { label: t.label(), transform: t, notes: fugue::transform(&s.notes, &k, t) })
        .collect();
    let lo = s.notes.iter().map(|n| n.pitch).min().unwrap_or(60);
    let hi = s.notes.iter().map(|n| n.pitch).max().unwrap_or(60);
    let mut strettos = fugue::strettos(&s.notes, &k, bpb);
    strettos.truncate(10);
    Ok(SubjectView {
        answer: fugue::answer(&s.notes, &k),
        key,
        track_name: s.track_name,
        clip_name: s.clip_name,
        start: s.start,
        length: fugue::length(&s.notes),
        range: (lo, hi),
        notes: s.notes,
        transforms,
        strettos,
        beats_per_bar: bpb,
    })
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VoiceIn {
    pub name: String,
    pub notes: Vec<VNote>,
}

/// Stimmen als neue Instrumentspuren ab `start` schreiben (ein Undo-Schritt).
pub fn add_voices(st: &AppState, voices: &[VoiceIn], start: f64, kind: InstrumentKind, label: &str) -> CmdResult<(Snapshot, Vec<Id>)> {
    if voices.iter().all(|v| v.notes.is_empty()) {
        return Err(tr!("No notes to write"));
    }
    let (cmd, ids) = {
        let mut s = lock(&st.session)?;
        let p: &mut Project = &mut s.project;
        let bar = p.beats_per_bar.max(1) as f64;
        let spec = sound::spec(kind);
        let mut commands = Vec::new();
        let mut ids = Vec::new();
        for v in voices.iter().filter(|v| !v.notes.is_empty()) {
            let id = p.alloc_id();
            let mut track = Track::new(id, v.name.clone(), TrackKind::Instrument);
            let mut inst = sound::default_instrument(kind);
            // Pedal für den Bass der Orgel
            let low = v.notes.iter().map(|n| n.pitch as f64).sum::<f64>() / v.notes.len() as f64 <= 52.0;
            if kind == InstrumentKind::Organ && low {
                if let Some(pre) = spec.presets.iter().find(|x| x.name.starts_with("Pedal")) {
                    for (k, val) in &pre.params {
                        inst.params.insert(k.to_string(), *val);
                    }
                    inst.preset = pre.name.to_string();
                }
            }
            track.instrument = Some(inst);
            track.gain_db = if voices.len() >= 3 { -4.0 } else { -2.0 };
            let first = v.notes.iter().map(|n| n.start).fold(f64::MAX, f64::min).max(0.0);
            let clip_start = start + (first / bar).floor() * bar;
            let notes: Vec<Note> = v
                .notes
                .iter()
                .map(|n| Note::new(n.pitch, (start + n.start - clip_start).max(0.0), n.len.max(0.05), 84))
                .collect();
            let end = notes.iter().map(|n| n.start_beats + n.length_beats).fold(0.0, f64::max);
            track.clips.push(Clip {
                id: p.alloc_id(),
                name: v.name.clone(),
                start_beats: clip_start,
                source: ClipSource::Notes { length_beats: (end / bar).ceil().max(1.0) * bar, notes },
            });
            ids.push(id);
            commands.push(Command::AddTrack { track, index: None });
        }
        (Command::Batch { label: label.to_string(), commands }, ids)
    };
    Ok((st.execute(cmd, false)?, ids))
}

/// Mehrere Stimmen zusammen vorspielen (Klang des gewählten Instruments).
pub fn preview_voices(st: &AppState, voices: &[Vec<VNote>], kind: InstrumentKind) -> CmdResult<f32> {
    let engine = st.engine()?;
    let sr = engine.info().sample_rate as f32;
    let spb = (60.0 / st.tempo()) as f32;
    let t0 = voices.iter().flatten().map(|n| n.start).fold(f64::MAX, f64::min);
    if !t0.is_finite() {
        return Err(tr!("No notes"));
    }
    let inst = sound::default_instrument(kind);
    let end = voices.iter().flatten().map(|n| n.end()).fold(0.0, f64::max);
    let total = (((end - t0) as f32 * spb) + 1.5).min(240.0);
    let mut mix = vec![0f32; (total * sr) as usize];
    for v in voices {
        let notes: Vec<NoteSpec> = v.iter().map(|n| NoteSpec::new((n.start - t0) as f32 * spb, n.len as f32 * spb * 0.97, n.pitch as f32, 0.75)).collect();
        let x = sound::render_instrument(Some(&inst), &notes, total, sr);
        klang_dsp::synth::mix_into(&mut mix, &x, 0, 1.0 / (voices.len() as f32).sqrt().max(1.0));
    }
    let secs = mix.len() as f32 / sr;
    engine.preview_send(mix.into(), util::db_to_gain(-1.0), sound::reverb_send(Some(&inst)));
    Ok(secs)
}

// ------------------------------------------------------------------ Prüfer

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRef {
    pub track_id: Id,
    pub name: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IssueView {
    pub kind: IssueKind,
    pub severity: Severity,
    pub start: f64,
    pub end: f64,
    pub bar: u32,
    pub track_ids: Vec<Id>,
    pub message: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CheckView {
    pub voices: Vec<VoiceRef>,
    pub issues: Vec<IssueView>,
    pub errors: usize,
    pub warnings: usize,
    pub hints: usize,
}

/// Satzfehler in den Instrumentspuren mit Tonhöhe (je Spur eine Stimme).
pub fn check_tracks(st: &AppState, only: Option<&[Id]>) -> CmdResult<CheckView> {
    let s = lock(&st.session)?;
    let p = &s.project;
    let bpb = p.beats_per_bar.max(1);
    let mut refs = Vec::new();
    let mut voices = Vec::new();
    for t in p.tracks.iter().filter(|t| melodic(t) && !t.mute && only.is_none_or(|o| o.contains(&t.id))) {
        let notes: Vec<VNote> = t.clips.iter().flat_map(|c| clip_vnotes(t, c)).collect();
        if notes.is_empty() {
            continue;
        }
        refs.push(VoiceRef { track_id: t.id, name: t.name.clone() });
        voices.push(notes);
    }
    let names: Vec<String> = refs.iter().map(|r| r.name.clone()).collect();
    let key = project_key(p);
    let issues = check(&voices, &names, key.as_ref(), bpb);
    let view: Vec<IssueView> = issues
        .into_iter()
        .map(|i| IssueView {
            kind: i.kind,
            severity: i.severity,
            start: i.start,
            end: i.end,
            bar: (i.start / bpb as f64).floor() as u32 + 1,
            track_ids: i.voices.iter().filter_map(|&v| refs.get(v).map(|r| r.track_id)).collect(),
            message: i.message,
        })
        .collect();
    let count = |s: Severity| view.iter().filter(|i| i.severity == s).count();
    Ok(CheckView { errors: count(Severity::Fehler), warnings: count(Severity::Warnung), hints: count(Severity::Hinweis), voices: refs, issues: view })
}

