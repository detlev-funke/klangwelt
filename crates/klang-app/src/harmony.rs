//! Circle-of-fifths window and agent: key of the project, chord progression of a
//! track, continuations, cadences, previewing and writing chord progressions
//! (set in four parts, one undo step).

use crate::sound;
use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::{Clip, ClipPatch, ClipSource, Command, Id, InstrumentKind, Note, Project, Track, TrackKind};
use klang_dsp::synth::NoteSpec;
use klang_dsp::util;
use klang_i18n::tr;
use klang_music::chordstats::{Follow, Rel, GROUPS};
use klang_music::chordstats::ChordStats;
use klang_music::harmony::{self, CadencePath, CadenceTemplate, Category, Chord, ChordInfo, CircleCell, Suggestion, TimedPitch};
use klang_music::theory::{estimate_key_from_histogram, Key};
use serde::Serialize;

/// Track that chord progressions without an explicit target go to. Its name is
/// how core and UI find it: a new one is called "Chords" in the chosen language
/// ([`chord_track_name`]); older projects have "Akkorde".
pub const CHORD_TRACK: &str = "Akkorde";

/// Name of a new chord track (and its clips), in the chosen language.
pub fn chord_track_name() -> String {
    tr!("Chords")
}

/// Is this the chord track? Its German, English or current-language name.
pub fn is_chord_track(name: &str) -> bool {
    name == CHORD_TRACK || name == "Chords" || name == chord_track_name()
}
const MAX_CHORDS: usize = 64;

fn melodic(t: &Track) -> bool {
    t.kind == TrackKind::Instrument && sound::transposes(t.instrument.as_ref())
}

fn shift_of(t: &Track) -> i32 {
    if sound::transposes(t.instrument.as_ref()) { t.pitch_semitones.round() as i32 } else { 0 }
}

/// Sounding notes of a clip in project time (beats), transposed as played.
fn clip_pitches(t: &Track, c: &Clip) -> Vec<TimedPitch> {
    let shift = shift_of(t);
    let ClipSource::Notes { notes, length_beats } = &c.source else { return Vec::new() };
    notes
        .iter()
        .filter(|n| n.start_beats < *length_beats)
        .map(|n| TimedPitch {
            pitch: (n.pitch as i32 + shift).clamp(0, 127) as u8,
            start: c.start_beats + n.start_beats,
            length: n.length_beats.min(length_beats - n.start_beats),
        })
        .collect()
}

/// Key from all instrument tracks with pitch (duration-weighted).
pub fn project_key(p: &Project) -> Option<Key> {
    let mut hist = [0.0f64; 12];
    for t in p.tracks.iter().filter(|t| melodic(t) && !t.mute) {
        for c in &t.clips {
            for n in clip_pitches(t, c) {
                hist[(n.pitch % 12) as usize] += n.length;
            }
        }
    }
    estimate_key_from_histogram(&hist)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    #[serde(flatten)]
    pub key: Key,
    /// Name for display, in the chosen language.
    pub name: String,
    /// "gewaehlt" (by hand), "geschaetzt" (from the notes) or "standard" (C major, no notes).
    pub source: &'static str,
}

/// KeyInfo for a key with origin ("gewaehlt", "geschaetzt", "standard").
pub fn key_info(key: Key, source: &'static str) -> KeyInfo {
    let key = Key { tonic: key.tonic % 12, minor: key.minor, confidence: key.confidence };
    KeyInfo { key, name: key.name(), source }
}

/// Key: chosen, otherwise estimated from the notes, otherwise C major.
pub fn resolve_key(st: &AppState, chosen: Option<Key>) -> CmdResult<KeyInfo> {
    let (key, source) = match chosen {
        Some(k) => (Key { tonic: k.tonic % 12, minor: k.minor, confidence: 1.0 }, "gewaehlt"),
        None => match project_key(&lock(&st.session)?.project) {
            Some(k) => (k, "geschaetzt"),
            None => (Key { tonic: 0, minor: false, confidence: 0.0 }, "standard"),
        },
    };
    Ok(KeyInfo { key, name: key.name(), source })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placed {
    pub start_beats: f64,
    pub length_beats: f64,
    #[serde(flatten)]
    pub info: ChordInfo,
}

/// Chord progression of a track (or a clip); without a track, all instrument tracks
/// with pitch together – i.e. the harmony of the whole piece.
pub fn chords_in(st: &AppState, track_id: Option<Id>, clip_id: Option<Id>, key: &Key) -> CmdResult<Vec<Placed>> {
    let s = lock(&st.session)?;
    let p = &s.project;
    let mut notes = Vec::new();
    match track_id {
        Some(tid) => {
            let t = p.track(tid).ok_or_else(|| tr!("Track {id} not found", id = tid))?;
            if !melodic(t) {
                return Err(tr!("\"{name}\" is not a pitched instrument track", name = t.name));
            }
            for c in t.clips.iter().filter(|c| clip_id.is_none_or(|id| c.id == id)) {
                notes.extend(clip_pitches(t, c));
            }
        }
        None => {
            for t in p.tracks.iter().filter(|t| melodic(t) && !t.mute) {
                for c in &t.clips {
                    notes.extend(clip_pitches(t, c));
                }
            }
        }
    }
    let window = (p.beats_per_bar as f64 / 2.0).max(1.0);
    Ok(harmony::detect_chords(&notes, window, Some(key))
        .into_iter()
        .map(|d| Placed { start_beats: d.start, length_beats: d.length, info: harmony::describe(&d.chord, key) })
        .collect())
}

/// Sets chords in four parts; `before` = the chord before (for the connection, not output).
fn voiced(chords: &[Chord], key: &Key, before: Option<Chord>) -> Vec<Vec<u8>> {
    match before {
        Some(b) => {
            let mut all = vec![b];
            all.extend_from_slice(chords);
            harmony::voice_progression(&all, Some(key), None).split_off(1)
        }
        None => harmony::voice_progression(chords, Some(key), None),
    }
}

fn check_len(chords: &[Chord], bpc: f64) -> CmdResult<f64> {
    if chords.is_empty() {
        return Err(tr!("No chords given"));
    }
    if chords.len() > MAX_CHORDS {
        return Err(tr!("At most {max} chords at once", max = MAX_CHORDS));
    }
    if !(0.25..=32.0).contains(&bpc) {
        return Err(tr!("Duration per chord: 0.25 to 32 beats"));
    }
    Ok(bpc)
}

/// Plays a chord progression (sound of the track, otherwise plucked). Returns the duration in seconds.
pub fn preview(st: &AppState, track_id: Option<Id>, chords: &[Chord], key: &Key, beats_per_chord: f64, before: Option<Chord>) -> CmdResult<f32> {
    let bpc = check_len(chords, beats_per_chord)?;
    let track = match track_id {
        Some(id) => {
            let t = st.instrument_track(id)?;
            melodic(&t).then_some(t)
        }
        None => None,
    };
    let inst = track.as_ref().and_then(|t| t.instrument.clone()).unwrap_or_else(|| sound::default_instrument(InstrumentKind::Pluck));
    let shift = track.as_ref().map(|t| t.pitch_semitones).unwrap_or(0.0);
    let gain = track.as_ref().map(|t| util::db_to_gain(t.gain_db)).unwrap_or(1.0);
    let engine = st.engine()?;
    let sr = engine.info().sample_rate as f32;
    let secs_per_chord = (bpc * 60.0 / st.tempo()) as f32;
    let mut notes = Vec::new();
    for (i, v) in voiced(chords, key, before).iter().enumerate() {
        for (j, &m) in v.iter().enumerate() {
            let vel = if j == 0 { 0.85 } else { 0.72 };
            // Pianissimo velocity staggered: slightly broken as if plucked
            let t0 = i as f32 * secs_per_chord + j as f32 * 0.012;
            notes.push(NoteSpec::new(t0, secs_per_chord * 0.96, m as f32 + shift, vel));
        }
    }
    let total = chords.len() as f32 * secs_per_chord + 1.2;
    let data = sound::render_instrument(Some(&inst), &notes, total, sr);
    let secs = data.len() as f32 / sr;
    engine.preview_send(data.into(), gain, sound::reverb_send(Some(&inst)));
    Ok(secs)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Written {
    pub track_id: Id,
    pub clip_id: Id,
    pub start_beats: f64,
    pub length_beats: f64,
    /// Voices set for each chord (MIDI, bass first).
    pub voicings: Vec<Vec<u8>>,
}

/// Notes from `rel` (beats in the clip) shifted back by `by`; notes across that point end there.
fn shifted_notes(notes: &[Note], rel: f64, by: f64) -> Vec<Note> {
    notes
        .iter()
        .map(|n| {
            if n.start_beats >= rel - 1e-9 {
                Note { start_beats: n.start_beats + by, ..n.clone() }
            } else if n.start_beats + n.length_beats > rel + 1e-9 {
                Note { length_beats: rel - n.start_beats, ..n.clone() }
            } else {
                n.clone()
            }
        })
        .collect()
}

/// Writes a chord progression as notes – into the track `track_id` or into the track
/// "Akkorde" (created if needed). If a note clip already lies there at
/// that position, the chords go into it (notes in the range are replaced),
/// otherwise a new clip is created.
///
/// With `insert`, room is made at that position: everything from
/// `start_beats` moves back by the length of the chords in all tracks
/// (clips that extend across the position get longer, their notes after it move
/// along). Without `insert`, the notes in the range of the target track are replaced.
#[allow(clippy::too_many_arguments)]
pub fn write_mode(
    st: &AppState,
    track_id: Option<Id>,
    start_beats: f64,
    chords: &[Chord],
    key: &Key,
    beats_per_chord: f64,
    before: Option<Chord>,
    label: &str,
    insert: bool,
) -> CmdResult<(Snapshot, Written)> {
    let bpc = check_len(chords, beats_per_chord)?;
    let start = start_beats.max(0.0);
    let len = chords.len() as f64 * bpc;
    let voicings = voiced(chords, key, before);
    let mut commands = Vec::new();
    let (cmd, written) = {
        let mut s = lock(&st.session)?;
        let p = &mut s.project;
        let target = match track_id {
            Some(id) => {
                let t = p.track(id).ok_or_else(|| tr!("Track {id} not found", id = id))?;
                if !melodic(t) {
                    return Err(tr!("\"{name}\" is not a pitched instrument track – chords can't go on drums", name = t.name));
                }
                Some(id)
            }
            None => p.tracks.iter().find(|t| is_chord_track(&t.name) && melodic(t)).map(|t| t.id),
        };
        let shift = target.and_then(|id| p.track(id)).map(shift_of).unwrap_or(0);
        // Insert: make room – everything from the position moves back in all tracks
        let mut spanning: Option<(Id, Vec<Note>, f64)> = None;
        if insert {
            for t in &p.tracks {
                for c in &t.clips {
                    let length = match &c.source {
                        ClipSource::Notes { length_beats, .. } => *length_beats,
                        _ => 0.0,
                    };
                    if c.start_beats >= start - 1e-9 {
                        commands.push(Command::SetClip { track_id: t.id, clip_id: c.id, patch: ClipPatch { start_beats: Some(c.start_beats + len), ..Default::default() } });
                    } else if let ClipSource::Notes { notes, .. } = &c.source {
                        if c.start_beats + length > start + 1e-9 {
                            let moved = shifted_notes(notes, start - c.start_beats, len);
                            if Some(t.id) == target && spanning.is_none() {
                                spanning = Some((c.id, moved, length + len));
                            } else {
                                commands.push(Command::SetClipNotes { track_id: t.id, clip_id: c.id, notes: moved, length_beats: Some(length + len) });
                            }
                        }
                    }
                }
            }
        }
        let notes_at = |offset: f64| -> Vec<Note> {
            voicings
                .iter()
                .enumerate()
                .flat_map(|(i, v)| {
                    v.iter().enumerate().map(move |(j, &m)| Note {
                        pitch: (m as i32 - shift).clamp(0, 127) as u8,
                        start_beats: offset + i as f64 * bpc,
                        length_beats: bpc * 0.98,
                        velocity: if j == 0 { 88 } else { 76 },
                        ..Default::default()
                    })
                })
                .collect()
        };
        match target {
            // Insert into a clip that extends across the position: chords into the gap
            Some(tid) if spanning.is_some() => {
                let (cid, mut notes, new_len) = spanning.take().expect("geprüft");
                let t = p.track(tid).expect("geprüft");
                let rel = start - t.clips.iter().find(|c| c.id == cid).map(|c| c.start_beats).unwrap_or(start);
                notes.extend(notes_at(rel));
                notes.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
                let w = Written { track_id: tid, clip_id: cid, start_beats: start, length_beats: len, voicings: voicings.clone() };
                (Command::SetClipNotes { track_id: tid, clip_id: cid, notes, length_beats: Some(new_len) }, w)
            }
            // Insert without a clip at the position: its own clip in the gap
            Some(tid) if insert => {
                let clip = Clip { id: p.alloc_id(), name: chord_track_name(), start_beats: start, source: ClipSource::Notes { length_beats: len, notes: notes_at(0.0) } };
                let w = Written { track_id: tid, clip_id: clip.id, start_beats: start, length_beats: len, voicings: voicings.clone() };
                (Command::AddClip { track_id: tid, clip }, w)
            }
            Some(tid) => {
                let t = p.track(tid).expect("geprüft");
                let host = t.clips.iter().find(|c| match &c.source {
                    ClipSource::Notes { length_beats, .. } => c.start_beats <= start + 1e-9 && start < c.start_beats + length_beats - 1e-9,
                    _ => false,
                });
                match host {
                    Some(c) => {
                        let ClipSource::Notes { length_beats, notes } = &c.source else { unreachable!() };
                        let rel = start - c.start_beats;
                        let mut kept: Vec<Note> = notes
                            .iter()
                            .filter(|n| !(n.start_beats < rel + len - 1e-9 && n.start_beats + n.length_beats > rel + 1e-9))
                            .cloned()
                            .collect();
                        kept.extend(notes_at(rel));
                        kept.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats).then(a.pitch.cmp(&b.pitch)));
                        let new_len = length_beats.max(rel + len);
                        let w = Written { track_id: tid, clip_id: c.id, start_beats: start, length_beats: len, voicings: voicings.clone() };
                        (Command::SetClipNotes { track_id: tid, clip_id: c.id, notes: kept, length_beats: Some(new_len) }, w)
                    }
                    None => {
                        let clip = Clip { id: p.alloc_id(), name: chord_track_name(), start_beats: start, source: ClipSource::Notes { length_beats: len, notes: notes_at(0.0) } };
                        let w = Written { track_id: tid, clip_id: clip.id, start_beats: start, length_beats: len, voicings: voicings.clone() };
                        (Command::AddClip { track_id: tid, clip }, w)
                    }
                }
            }
            None => {
                let id = p.alloc_id();
                let mut track = Track::new(id, chord_track_name(), TrackKind::Instrument);
                track.instrument = Some(sound::default_instrument(InstrumentKind::Pluck));
                track.gain_db = -4.0;
                let clip = Clip { id: p.alloc_id(), name: chord_track_name(), start_beats: start, source: ClipSource::Notes { length_beats: len, notes: notes_at(0.0) } };
                let w = Written { track_id: id, clip_id: clip.id, start_beats: start, length_beats: len, voicings: voicings.clone() };
                track.clips.push(clip);
                (Command::AddTrack { track, index: None }, w)
            }
        }
    };
    commands.push(cmd);
    let snap = st.execute(Command::Batch { label: label.to_string(), commands }, false)?;
    Ok((snap, written))
}

// ------------------------------------------------------------------ Library

/// An example passage with title and composer.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleView {
    pub doc: u32,
    pub title: String,
    pub composer: String,
    pub bar: u32,
    pub start: f32,
    pub end: f32,
}

/// How often this continuation occurs in the library.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibFollow {
    pub share: f32,
    pub count: u32,
    pub total: u32,
    /// true: exactly this form; false: counted as a root chord (too little data for the exact form)
    pub exact: bool,
    pub examples: Vec<ExampleView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionView {
    #[serde(flatten)]
    pub suggestion: Suggestion,
    pub library: Option<LibFollow>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibSummary {
    pub name: String,
    /// Scores with a chord progression in this mode (major or minor) in the chosen group.
    pub scores: u32,
    /// Chosen group (0 = all, 1 = Baroque, 2 = Bach).
    pub group: usize,
    pub groups: Vec<GroupInfo>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfo {
    pub id: usize,
    pub key: &'static str,
    /// Display name (translated; the names in `GROUPS` are keys).
    pub label: String,
    pub scores: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestResult {
    pub suggestions: Vec<SuggestionView>,
    /// None: no library, or index without chord statistics.
    pub library: Option<LibSummary>,
    /// Note on why frequencies are missing.
    pub hint: Option<String>,
    /// The hint suggests re-indexing the library (index without chord statistics or without groups).
    pub reindex: bool,
    /// Multi-step paths from the last chord to the closings, best first.
    pub paths: Vec<PathView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathView {
    #[serde(flatten)]
    pub path: CadencePath,
    /// Share of each step among the continuations in the library (None: no data).
    pub shares: Option<Vec<Option<f32>>>,
}

/// Longest paths searched for the closing.
const PATH_STEPS: usize = 4;

/// Paths to the closing; with chord statistics, the frequency in the library counts
/// half (a step that half of the pieces take counts as full).
fn paths_for(key: &Key, chords: &[Chord], stats: Option<&ChordStats>) -> Vec<PathView> {
    let Some(stats) = stats else {
        return harmony::cadence_paths(key, chords, PATH_STEPS, &|_, _, s| s).into_iter().map(|path| PathView { path, shares: None }).collect();
    };
    let share = |a: &Chord, b: &Chord| stats.share(key.minor, Rel::of(a, key), Rel::of(b, key));
    let weight = |a: &Chord, b: &Chord, s: f32| 0.5 * s + 0.5 * (share(a, b).unwrap_or(0.0) * 2.0).min(1.0);
    let tonic = Chord::new(key.tonic, if key.minor { harmony::Quality::Minor } else { harmony::Quality::Major });
    let start = *chords.last().unwrap_or(&tonic);
    harmony::cadence_paths(key, chords, PATH_STEPS, &weight)
        .into_iter()
        .map(|path| {
            let mut prev = start;
            let shares = path
                .steps
                .iter()
                .map(|s| {
                    let v = share(&prev, &s.chord).map(|x| (x * 1000.0).round() / 1000.0);
                    prev = s.chord;
                    v
                })
                .collect();
            PathView { path, shares: Some(shares) }
        })
        .collect()
}

fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Suggestions by functional harmony, supplemented by the frequencies from the
/// loaded library and by common continuations that no rule names.
pub fn suggest_with_library(st: &AppState, key: &Key, chords: &[Chord], group: usize) -> CmdResult<SuggestResult> {
    let base = harmony::suggest(key, chords, 12);
    let lib = lock(&st.library)?;
    let Some(lib) = lib.as_ref() else {
        return Ok(SuggestResult {
            suggestions: base.into_iter().map(|s| SuggestionView { suggestion: s, library: None }).collect(),
            library: None,
            hint: Some(tr!("No library loaded – the frequencies come from the music library.")),
            reindex: false,
            paths: paths_for(key, chords, None),
        });
    };
    if lib.index.chords().is_none() {
        return Ok(SuggestResult {
            suggestions: base.into_iter().map(|s| SuggestionView { suggestion: s, library: None }).collect(),
            library: None,
            hint: Some(tr!("The index has no chord statistics yet – reindex the library.")),
            reindex: true,
            paths: paths_for(key, chords, None),
        });
    }
    let mode = key.minor as usize;
    let groups: Vec<GroupInfo> = GROUPS
        .iter()
        .enumerate()
        .filter_map(|(id, (k, label))| {
            lib.index.chords_group(id).map(|s| GroupInfo { id, key: k, label: klang_i18n::translate(label), scores: s.scores[mode] })
        })
        .collect();
    // Group with too few pieces in this mode: fall back to all
    let mut hint = None;
    let mut reindex = false;
    let group = match groups.iter().find(|g| g.id == group) {
        Some(g) if g.scores >= 20 => group,
        Some(g) => {
            hint = Some(if key.minor {
                tr!("\"{group}\": only {n} minor-key pieces with a chord progression – showing all pieces.", group = g.label, n = g.scores)
            } else {
                tr!("\"{group}\": only {n} major-key pieces with a chord progression – showing all pieces.", group = g.label, n = g.scores)
            });
            0
        }
        None => {
            if group != 0 {
                hint = Some(tr!("This index doesn't know any groups yet (Baroque/Bach) – reindex the library."));
                reindex = true;
            }
            0
        }
    };
    let stats = lib.index.chords_group(group).or(lib.index.chords()).expect("geprüft");
    let name = lib.name.clone();
    let summary = LibSummary { name, scores: stats.scores[mode], group, groups };
    let view = |f: Follow| LibFollow {
        share: (f.share * 1000.0).round() / 1000.0,
        count: f.count,
        total: f.total,
        exact: f.exact,
        examples: f
            .examples
            .iter()
            .filter_map(|e| {
                let d = lib.index.doc(e.doc)?;
                Some(ExampleView { doc: e.doc, title: tidy(&d.title), composer: tidy(&d.composer), bar: e.bar, start: e.start, end: e.end })
            })
            .collect(),
    };
    let paths = paths_for(key, chords, Some(stats));
    let Some(prev) = chords.last() else {
        return Ok(SuggestResult { suggestions: base.into_iter().map(|s| SuggestionView { suggestion: s, library: None }).collect(), library: Some(summary), hint, reindex, paths });
    };
    let prev_rel = Rel::of(prev, key);
    let mut out: Vec<SuggestionView> = base
        .into_iter()
        .map(|s| {
            let f = stats.follow(key.minor, prev_rel, Rel::of(&s.info.chord, key)).map(&view);
            SuggestionView { suggestion: s, library: f }
        })
        .collect();
    // Common continuations that the rules do not name
    let mut extra = 0;
    for (rel, f) in stats.top_after(key.minor, prev_rel, 10) {
        let ch = rel.chord(key);
        let known = out.iter().any(|v| if f.exact { v.suggestion.info.chord == ch } else { v.suggestion.info.chord.same_harmony(&ch) });
        if known || ch == *prev || f.share < 0.02 || extra >= 5 {
            continue;
        }
        extra += 1;
        let pct = (f.share * 100.0).round();
        out.push(SuggestionView {
            suggestion: Suggestion {
                info: harmony::describe(&ch, key),
                score: ((0.2 + 0.6 * f.share) * 100.0).round() / 100.0,
                category: Category::Haeufig,
                reason: tr!("common in the library: {pct}% of continuations", pct = pct),
                to_key: None,
                mood: harmony::transition_mood(Some(prev), &ch, key),
            },
            library: Some(view(f)),
        });
    }
    Ok(SuggestResult { suggestions: out, library: Some(summary), hint, reindex, paths })
}

/// Plays an example passage from the library (all voices, tempo of the score).
pub fn play_example(st: &AppState, doc: u32, start: f64, end: f64) -> CmdResult<f32> {
    crate::library::play_range(st, doc, start, end.min(start + 32.0), None)
}

// ------------------------------------------------------------------ Tauri commands

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarmonyView {
    pub key: KeyInfo,
    pub circle: Vec<CircleCell>,
    pub cadences: Vec<CadenceTemplate>,
}

// ------------------------------------------------------------------ Timeline

/// A chord of the harmony track in the timeline: with the mood of the transition
/// from the chord before (as in the circle of fifths) and – if it breaks – the reason.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineChord {
    pub start_beats: f64,
    pub length_beats: f64,
    #[serde(flatten)]
    pub info: ChordInfo,
    pub mood: harmony::Mood,
    /// Harmonic break: "tonartfremd" or "Tritonus-Sprung"
    pub bruch: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarmonyLine {
    pub key: KeyInfo,
    pub chords: Vec<LineChord>,
}

/// Chord progression of the whole piece (all instrument tracks with pitch) with mood and breaks.
pub fn line(st: &AppState, chosen: Option<Key>) -> CmdResult<HarmonyLine> {
    let info = resolve_key(st, chosen)?;
    let placed = chords_in(st, None, None, &info.key)?;
    let mut prev: Option<Chord> = None;
    let chords = placed
        .into_iter()
        .map(|p| {
            let ch = p.info.chord;
            let mood = harmony::transition_mood(prev.as_ref(), &ch, &info.key);
            let bruch = harmony::harmonic_break(prev.as_ref(), &ch, &info.key).map(klang_i18n::translate);
            prev = Some(ch);
            LineChord { start_beats: p.start_beats, length_beats: p.length_beats, info: p.info, mood, bruch }
        })
        .collect();
    Ok(HarmonyLine { key: info, chords })
}
