//! Music library, the platform-neutral part: state, browsing, search by
//! title and melody (including sung), showing scores with voice analysis,
//! previewing them and taking voices over as tracks.
//!
//! Downloading, indexing and managing several libraries is done by the
//! platform shell (desktop: `app/src/library.rs`, `libraries.rs`).

use crate::state::{lock, AppState, CmdResult, Snapshot};
use klang_core::{Clip, ClipSource, Command, Id, Note, Track, TrackKind};
use klang_dsp::synth::NoteSpec;
use klang_i18n::{tr, trn};
use klang_music::analysis::analyze_score;
use klang_music::diskindex::AnyIndex;
use klang_music::facets::{dimension, Dimension, TreeLevel, DIMENSIONS};
use klang_music::index::MIN_QUERY_NOTES;
use klang_music::library::voice_label;
use klang_music::melody::{skyline, MelodyNote};
use klang_music::score::{load_score, load_score_bytes, Part, Score, Voice};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Reports the library state to the UI ("library-changed").
pub fn emit_info(st: &AppState) {
    if let Ok(i) = info(st) {
        st.host().emit("library-changed", &i);
    }
}

/// Progress and last message of the library.
#[derive(Default)]
pub struct LibStatus {
    pub building: Option<(usize, usize)>,
    /// Individual files are being added to the index.
    pub adding: bool,
    pub download: Option<DownloadProgress>,
    pub message: Option<String>,
    /// New scores are being merged into the library (fraction 0…1).
    pub merging: Option<f32>,
    /// At startup: the active library is being loaded (or set up).
    pub loading: bool,
    /// A download job is running – from its start until the import finishes.
    /// Also covers the short gaps between loading, categories and indexing.
    pub pipeline: Option<QueuedDownload>,
    /// Queued packages waiting until the running job is done.
    pub queue: Vec<QueuedDownload>,
    /// The thread that works through the queue is running.
    pub queue_runner: bool,
    /// Browser: state of the two parts of the library (package on the server, own scores).
    pub web: Option<WebLibInfo>,
}

/// Browser: the library consists of a package on a server (Zenodo), read by
/// range, and the person's own scores (in the browser's storage).
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebLibInfo {
    /// The package: "none" (not configured), "loading", "ready", "offline" (unreachable, nothing cached), "error"
    pub remote: String,
    /// Title, version and number of scores of the package (from its manifest).
    pub remote_title: Option<String>,
    pub remote_version: Option<String>,
    pub remote_scores: usize,
    /// Page of the package (e.g. the Zenodo record or DOI).
    pub remote_url: Option<String>,
    /// License of the package and of its scores (from the manifest, shown as is).
    pub remote_license: Option<String>,
    /// Sources with attribution (from the manifest).
    pub remote_sources: Vec<String>,
    pub own_scores: usize,
    /// Why the package isn't available (if so).
    pub error: Option<String>,
}

/// A queued package (running now or waiting in the queue).
/// Queued and processed in the desktop shell (`app/src/library.rs`).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueuedDownload {
    /// Package from the catalog (empty for old jobs resumed at startup)
    pub id: String,
    pub name: String,
    /// Target library: the one active when queued (id, name)
    pub library: String,
    pub library_name: String,
    #[serde(skip)]
    pub folder: Option<String>,
    #[serde(skip)]
    pub with_meta: bool,
}

/// Progress of a library download.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub name: String,
    /// "Partituren" or "Kategorien" (genre/composer from PDMX.csv) – identifiers,
    /// the UI compares them (don't translate)
    pub phase: String,
    pub folder: String,
    /// Scores in the target folder
    pub files: usize,
    /// requested count (None = all)
    pub target: Option<usize>,
    /// downloaded (compressed) bytes and total size of the archive
    pub bytes: u64,
    pub total_bytes: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LibraryInfo {
    /// Active library (id and name); None while it is being loaded
    id: Option<String>,
    name: Option<String>,
    voices: usize,
    scores: usize,
    /// (done, total) while indexing
    building: Option<(usize, usize)>,
    download: Option<DownloadProgress>,
    message: Option<String>,
    /// Scores with features (tree view); 0 = index without features (reindex)
    tree_scores: usize,
    /// Genre/composer are available for all contained PDMX packages
    has_meta: bool,
    /// Scores with chord progression [major, minor]; None = index without chord statistics (reindex)
    chord_scores: Option<[u32; 2]>,
    /// Files are being added
    adding: bool,
    /// Currently importing into the library (fraction 0…1)
    merging: Option<f32>,
    /// The library is being loaded at startup
    loading: bool,
    /// Package currently being loaded or imported
    current_pack: Option<String>,
    /// Waiting packages in the order in which their turn comes
    queue: Vec<QueuedDownload>,
    /// Browser only: package on the server and own scores.
    #[serde(skip_serializing_if = "Option::is_none")]
    web: Option<WebLibInfo>,
}

pub fn info(st: &AppState) -> CmdResult<LibraryInfo> {
    let status = lock(&st.library_status)?;
    let lib = lock(&st.library)?;
    Ok(LibraryInfo {
        id: lib.as_ref().map(|l| l.id.clone()),
        // the browser library has no name of its own: in the chosen language
        name: lib.as_ref().map(|l| if l.id == "browser" { tr!("Library") } else { l.name.clone() }),
        voices: lib.as_ref().map(|l| l.index.doc_count()).unwrap_or(0),
        scores: lib.as_ref().map(|l| l.index.score_count()).unwrap_or(0),
        building: status.building,
        download: status.download.clone(),
        // fixed messages (e.g. the web core's, `tk!`) are made before the language is chosen – translate them here
        message: status.message.as_deref().map(klang_i18n::translate),
        tree_scores: lib.as_ref().map(|l| l.index.tree_score_count()).unwrap_or(0),
        has_meta: lib.as_ref().map(|l| l.has_meta).unwrap_or(true),
        chord_scores: lib.as_ref().and_then(|l| l.index.chord_counts()),
        adding: status.adding,
        merging: status.merging,
        loading: status.loading,
        current_pack: status.pipeline.as_ref().map(|p| p.id.clone()).filter(|id| !id.is_empty()),
        queue: status.queue.clone(),
        web: status.web.clone(),
    })
}

/// What is currently running in the background (for the prompt on closing).
pub fn busy(st: &AppState) -> Option<String> {
    let s = st.library_status.lock().ok()?;
    let now = if let Some(d) = &s.download {
        Some(if d.phase == "Kategorien" {
            tr!("{name}: loading genre and composer", name = d.name)
        } else {
            tr!("Downloading {name}", name = d.name)
        })
    } else if s.merging.is_some() {
        Some(tr!("Adding scores to the library"))
    } else {
        s.building.map(|(d, t)| {
            if t > 0 {
                tr!("Indexing the library ({done} of {total} files)", done = d, total = t)
            } else {
                tr!("Indexing the library")
            }
        })
    };
    // Waiting packages are lost on closing – mention that.
    let waiting = match s.queue.len() {
        0 => None,
        1 => Some(tr!("\"{name}\" is still waiting in the queue", name = s.queue[0].name)),
        n => Some(trn!(n, "{n} package is still waiting in the queue", "{n} packages are still waiting in the queue")),
    };
    match (now, waiting) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    }
}

// ---------------------------------------------------------------- Browsing (tree)

/// Levels by which the library can be organized.
pub fn dimensions() -> CmdResult<Vec<Dimension>> {
    Ok(DIMENSIONS.to_vec())
}

/// Content of a tree node: branches of the next level or scores.

pub fn tree(st: &AppState, levels: &[String], path: &[String], offset: usize, limit: usize) -> CmdResult<TreeLevel> {
    let lib = lock(&st.library)?;
    let lib = lib.as_ref().ok_or_else(|| tr!("No library loaded."))?;
    if lib.index.tree_score_count() == 0 {
        return Err(tr!("This library has no features yet – please reindex."));
    }
    let mut lv: Vec<&str> = Vec::new();
    for l in levels {
        let d = dimension(l)
            .ok_or_else(|| tr!("Unknown level: {level} (available: {list})", level = l, list = DIMENSIONS.map(|d| d.key).join(", ")))?;
        lv.push(d.key);
    }
    Ok(lib.index.tree(&lv, path, offset, limit.clamp(1, 1000)))
}

/// Where a voice entry comes from: "own" (the person's scores in the browser,
/// layered over the package), "remote" (the package on the server) or "" (a
/// library on disk).
fn origin(index: &AnyIndex, doc: u32) -> &'static str {
    match index.extra_offset() {
        Some(o) if doc >= o && index.is_remote() => "own",
        _ if index.is_remote() => "remote",
        // browser without a package: everything is own
        _ if matches!(index, AnyIndex::Mem(_)) && cfg!(target_arch = "wasm32") => "own",
        _ => "",
    }
}

// ---------------------------------------------------------------- Search

/// Melody from text: note names (German: H = h, B = b; Cis/Es/Fis …, also C#/Eb)
/// with octave or MIDI numbers, optionally with duration: "C4:1 D4:0.5 E4" or "60 62 64".
pub fn parse_melody(s: &str) -> Result<Vec<MelodyNote>, String> {
    let mut t = 0.0;
    let mut out = Vec::new();
    // Split at spaces/semicolons; commas only separate when there is no duration
    // ("60,62,64"), otherwise they are decimal commas ("D4:0,5").
    let tokens = s.split(|c: char| c.is_whitespace() || c == ';').flat_map(|t| {
        let t = t.trim_end_matches(',');
        if t.contains(':') { vec![t] } else { t.split(',').collect() }
    });
    for tok in tokens.filter(|p| !p.is_empty()) {
        let (name, dur) = tok.split_once(':').unwrap_or((tok, "1"));
        let duration: f64 = dur.replace(',', ".").parse().map_err(|_| tr!("Invalid duration \"{duration}\"", duration = dur))?;
        let midi = parse_note(name).ok_or_else(|| tr!("Unknown note \"{note}\" (e.g. C4, F#3, Bb2 or 60)", note = name))?;
        out.push(MelodyNote { onset: t, duration, midi });
        t += duration.max(0.01);
    }
    Ok(out)
}

pub(crate) fn parse_note(s: &str) -> Option<u8> {
    if let Ok(n) = s.parse::<u8>() {
        return (n <= 127).then_some(n);
    }
    let lower = s.to_lowercase();
    let mut chars = lower.chars().peekable();
    let letter = chars.next()?;
    let mut pc: i32 = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'h' => 11,
        'b' => 10, // German: B = B♭
        _ => return None,
    };
    let rest: String = chars.collect();
    let (acc, oct) = rest.split_at(rest.find(|c: char| c.is_ascii_digit() || c == '-').unwrap_or(rest.len()));
    pc += match acc {
        "" => 0,
        "#" | "♯" | "is" => 1,
        "##" | "isis" => 2,
        "b" | "♭" if letter != 'b' => -1,
        "b" | "♭" => 0, // „Bb“ = B♭
        "es" | "s" => -1,
        "eses" => -2,
        _ => return None,
    };
    let octave: i32 = oct.parse().ok()?;
    let midi = (octave + 1) * 12 + pc;
    (0..=127).contains(&midi).then_some(midi as u8)
}

fn clip_melody(st: &AppState, track_id: Id, clip_id: Id) -> CmdResult<Vec<MelodyNote>> {
    let clip = st.clip(track_id, clip_id)?;
    match &clip.source {
        ClipSource::Notes { notes, .. } => {
            let mut v: Vec<MelodyNote> = notes.iter().map(|n| MelodyNote { onset: n.start_beats, duration: n.length_beats, midi: n.pitch }).collect();
            v.sort_by(|a, b| a.onset.total_cmp(&b.onset).then(b.midi.cmp(&a.midi)));
            v.dedup_by(|b, a| (a.onset - b.onset).abs() < 1e-6); // top note per onset
            Ok(v)
        }
        ClipSource::Audio { .. } => {
            let (_, a) = st.clip_analysis(track_id, clip_id)?;
            Ok(a.notes.iter().map(|n| MelodyNote { onset: n.start_secs, duration: n.end_secs - n.start_secs, midi: n.midi }).collect())
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibHit {
    doc: u32,
    title: String,
    composer: String,
    part: String,
    voice: String,
    source: String,
    note_count: u32,
    /// Melody match 0…1 (melody search only)
    score: Option<f32>,
    note_index: Option<usize>,
    query_len: usize,
    /// Collection and license of the score (packages; empty = unknown)
    #[serde(skip_serializing_if = "String::is_empty")]
    collection: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    license: String,
    /// Browser: "own" (own scores) or "remote" (package on the server) – an identifier, see `origin`
    #[serde(skip_serializing_if = "str::is_empty")]
    origin: &'static str,
}

pub fn search(
    st: &AppState,
    text: Option<String>,
    melody: Option<String>,
    track_id: Option<u64>,
    clip_id: Option<u64>,
    limit: Option<usize>,
) -> CmdResult<Vec<LibHit>> {
    let limit = limit.unwrap_or(40).clamp(1, 500);
    let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let query = match (melody.as_deref().map(str::trim).filter(|m| !m.is_empty()), track_id, clip_id) {
        (Some(m), _, _) => Some(parse_melody(m)?),
        (None, Some(t), Some(c)) => Some(clip_melody(st, Id(t), Id(c))?),
        _ => None,
    };
    let lib = lock(&st.library)?;
    let lib = lib.as_ref().ok_or_else(no_library)?;
    match query {
        Some(q) => melody_hits(&lib.index, &q, text.as_deref(), limit),
        None if text.is_some() => Ok(lib
            .index
            .search_text(text.as_deref().unwrap_or(""), limit)
            .into_iter()
            .map(|(doc, i)| LibHit {
                doc,
                title: i.title.clone(),
                composer: i.composer.clone(),
                part: i.part.clone(),
                voice: i.voice.clone(),
                source: i.source.clone(),
                note_count: i.note_count,
                score: None,
                note_index: None,
                query_len: 0,
                collection: i.collection,
                license: i.license,
                origin: origin(&lib.index, doc),
            })
            .collect()),
        None => Err(tr!("Enter a search text or a melody (or select a clip).")),
    }
}

fn no_library() -> String {
    tr!("No library loaded – it's loaded at startup (\"Library\" window).")
}

/// Melody search, optionally restricted to pieces whose title/composer/voice contains all words of `text`.
fn melody_hits(index: &AnyIndex, q: &[MelodyNote], text: Option<&str>, limit: usize) -> CmdResult<Vec<LibHit>> {
    if q.len() < MIN_QUERY_NOTES {
        return Err(tr!("Enter at least {min} notes for the melody search (detected: {found}).", min = MIN_QUERY_NOTES, found = q.len()));
    }
    let words: Vec<String> = text.unwrap_or("").split_whitespace().map(str::to_lowercase).collect();
    let matches_text = |i: &klang_music::index::DocInfo| {
        let hay = format!("{} {} {} {}", i.title, i.composer, i.part, i.source).to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    };
    Ok(index
        .search(q, 200)
        .into_iter()
        .filter(|h| matches_text(&h.info))
        .take(limit)
        .map(|h| LibHit {
            origin: origin(index, h.doc),
            doc: h.doc,
            title: h.info.title,
            composer: h.info.composer,
            part: h.info.part,
            voice: h.info.voice,
            source: h.info.source,
            note_count: h.info.note_count,
            score: Some(h.score),
            note_index: Some(h.note_index),
            query_len: q.len(),
            collection: h.info.collection,
            license: h.info.license,
        })
        .collect())
}

// ---------------------------------------------------------------- Singing a query

/// Is a sung-query recording for the search running? (Then the normal recording stays locked.)
pub static LISTENING: AtomicBool = AtomicBool::new(false);
/// Longest sung-query recording that is analyzed.
const LISTEN_MAX_SECS: f64 = 45.0;

const NOTE_NAMES_DE: [&str; 12] = ["C", "Cis", "D", "Es", "E", "F", "Fis", "G", "As", "A", "B", "H"];

/// Notes for the melody field: "E5:1 D5:0,5 …" – durations relative to the mean
/// note length (rounded to quarters), German note names, c' = C4.
pub fn format_melody(notes: &[MelodyNote]) -> String {
    let mut d: Vec<f64> = notes.iter().map(|n| n.duration).filter(|d| *d > 0.0).collect();
    d.sort_by(f64::total_cmp);
    let unit = d.get(d.len() / 2).copied().unwrap_or(1.0).max(1e-3);
    notes
        .iter()
        .map(|n| {
            let beats = ((n.duration / unit) * 4.0).round().max(1.0) / 4.0;
            let len = if (beats - beats.round()).abs() < 1e-9 { format!("{}", beats as i64) } else { klang_i18n::decimal(beats) };
            format!("{}{}:{len}", NOTE_NAMES_DE[n.midi as usize % 12], n.midi as i32 / 12 - 1)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenResult {
    /// Detected notes in the format of the melody field.
    melody: String,
    notes: usize,
    key: Option<String>,
    seconds: f64,
    hits: Vec<LibHit>,
}

/// Records the microphone without putting anything into the project (for melody search).
pub fn listen_start(st: &AppState) -> CmdResult<()> {
    if lock(&st.library)?.is_none() {
        return Err(no_library());
    }
    let engine = st.engine()?;
    if engine.status().recording {
        return Err(tr!("A recording is already running."));
    }
    LISTENING.store(true, Ordering::SeqCst);
    if let Err(e) = engine.start_recording() {
        LISTENING.store(false, Ordering::SeqCst);
        return Err(tr!("Microphone: {error}", error = e));
    }
    Ok(())
}

/// Ends the sung-query recording, recognizes the melody (singing or whistling) and searches.
/// `cancel` only discards the recording.
pub fn listen_stop(st: &AppState, text: Option<String>, cancel: Option<bool>) -> CmdResult<ListenResult> {
    if !LISTENING.swap(false, Ordering::SeqCst) {
        // Browser: the search waited for data from the server and the command
        // runs again – with the melody detected the first time.
        let waiting = PENDING_LISTEN.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((q, key, seconds)) = waiting {
            return listen_search(st, text, q, key, seconds);
        }
        return Err(tr!("No sing-to-search recording is running."));
    }
    let rec = st.engine()?.stop_recording().map_err(|e| e.to_string())?;
    let seconds = rec.samples.len() as f64 / rec.sample_rate.max(1) as f64;
    if cancel.unwrap_or(false) {
        return Ok(ListenResult { melody: String::new(), notes: 0, key: None, seconds, hits: Vec::new() });
    }
    if seconds < 1.0 {
        return Err(tr!("The recording was too short – sing or whistle a few bars."));
    }
    let mut samples = rec.samples;
    samples.truncate((LISTEN_MAX_SECS * rec.sample_rate as f64) as usize);
    let peak = klang_dsp::util::peak(&samples);
    if peak < 0.003 {
        return Err(tr!("Nothing came through – is the microphone on and allowed?"));
    }
    if peak < 0.5 {
        klang_dsp::util::normalize(&mut samples, 0.5);
    }
    let (q, key) = crate::analysis::query_melody(&samples, rec.sample_rate);
    if q.len() < MIN_QUERY_NOTES {
        return Err(match q.len() {
            0 => tr!(
                "No notes detected – please sing or whistle at least {min} notes clearly and separately (ideally on \"da\" or \"doo\").",
                min = MIN_QUERY_NOTES
            ),
            n => trn!(
                n,
                "Only 1 note detected – please sing or whistle at least {min} notes clearly and separately (ideally on \"da\" or \"doo\").",
                "Only {n} notes detected – please sing or whistle at least {min} notes clearly and separately (ideally on \"da\" or \"doo\").",
                min = MIN_QUERY_NOTES
            ),
        });
    }
    listen_search(st, text, q, key, seconds)
}

/// Detected melody of a sing-to-search whose search still waits for data (browser).
type ListenQuery = (Vec<MelodyNote>, Option<String>, f64);
static PENDING_LISTEN: std::sync::Mutex<Option<ListenQuery>> = std::sync::Mutex::new(None);

fn listen_search(st: &AppState, text: Option<String>, q: Vec<MelodyNote>, key: Option<String>, seconds: f64) -> CmdResult<ListenResult> {
    let lib = lock(&st.library)?;
    let lib = lib.as_ref().ok_or_else(no_library)?;
    let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let hits = melody_hits(&lib.index, &q, text.as_deref(), 40)?;
    if klang_music::source::remote::pending() {
        *PENDING_LISTEN.lock().unwrap_or_else(|e| e.into_inner()) = Some((q.clone(), key.clone(), seconds));
    }
    Ok(ListenResult { melody: format_melody(&q), notes: q.len(), key, seconds, hits })
}

// ---------------------------------------------------------------- Score

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceView {
    part: String,
    label: String,
    role: Option<String>,
    /// All notes: [onset, duration, MIDI] in quarters
    notes: Vec<(f64, f64, u8)>,
    is_hit: bool,
    /// Match range (quarters) in this voice
    highlight: Option<(f64, f64)>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreView {
    doc: u32,
    title: String,
    composer: String,
    source: String,
    key_name: Option<String>,
    tempo_bpm: Option<f64>,
    time_signature: Option<(u32, u32)>,
    mood: String,
    duration_quarters: f64,
    voices: Vec<VoiceView>,
    cadences: Vec<(f64, String)>,
    /// Highlighted spot across all voices (e.g. a chord change), in quarters.
    range: Option<(f64, f64)>,
    /// Collection, license and origin, as in [`LibHit`]
    #[serde(skip_serializing_if = "String::is_empty")]
    collection: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    license: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    origin: &'static str,
}

pub fn doc_and_score(st: &AppState, doc: u32) -> CmdResult<(klang_music::index::DocInfo, Score)> {
    let (info, bytes) = {
        let lib = lock(&st.library)?;
        let lib = lib.as_ref().ok_or_else(|| tr!("No library loaded"))?;
        let info = lib.index.doc(doc).ok_or_else(|| tr!("Entry {doc} not found", doc = doc))?;
        // in a package's ZIP (read by range in the browser) or a file of its own
        let bytes = lib.index.score_bytes(&info).transpose().map_err(|e| format!("{}: {e}", info.source))?;
        (info, bytes)
    };
    let score = match bytes {
        Some((name, data)) => load_score_bytes(&name, &data),
        // the browser keeps files in the core's file tree, the desktop on disk
        None if cfg!(target_arch = "wasm32") => crate::vfs::read(Path::new(&info.source)).map_err(Into::into).and_then(|data| load_score_bytes(&info.source, &data)),
        None => load_score(Path::new(&info.source)),
    }
    .map_err(|e| format!("{}: {e}", info.source))?;
    Ok((info, score))
}

pub fn open(
    st: &AppState,
    doc: u32,
    note_index: Option<usize>,
    query_len: Option<usize>,
    range: Option<(f64, f64)>,
) -> CmdResult<ScoreView> {
    let (info, score) = doc_and_score(st, doc)?;
    let analysis = analyze_score(&score);
    let mut voices = Vec::new();
    for part in &score.parts {
        for v in &part.voices {
            let label = voice_label(v);
            let role = analysis
                .voices
                .iter()
                .find(|p| p.part == part.name && p.staff == v.staff && p.voice == v.voice)
                .map(|p| p.role_label.to_string());
            let is_hit = part.name == info.part && label == info.voice;
            let highlight = match (is_hit, note_index) {
                (true, Some(ni)) => {
                    let line = skyline(&v.notes);
                    line.get(ni).map(|start| {
                        let last = line.get((ni + query_len.unwrap_or(1)).saturating_sub(1).min(line.len().saturating_sub(1))).unwrap_or(start);
                        (start.onset, last.onset + last.duration)
                    })
                }
                _ => None,
            };
            voices.push(VoiceView {
                part: part.name.clone(),
                label,
                role,
                notes: v.notes.iter().map(|n| (n.onset, n.duration, n.midi)).collect(),
                is_hit,
                highlight,
            });
        }
    }
    Ok(ScoreView {
        doc,
        title: if info.title.is_empty() { tr!("(untitled)") } else { info.title },
        composer: info.composer,
        source: info.source,
        key_name: analysis.key.map(|k| k.name()),
        tempo_bpm: score.tempo_bpm,
        time_signature: score.time_signature,
        mood: klang_i18n::translate(analysis.mood.label),
        duration_quarters: analysis.duration_quarters,
        voices,
        cadences: analysis.cadences.iter().map(|c| (c.onset, c.label.to_string())).collect(),
        range,
        collection: info.collection,
        license: info.license,
        origin: lock(&st.library)?.as_ref().map(|l| origin(&l.index, doc)).unwrap_or(""),
    })
}

/// Longest excerpt that is previewed (quarters).
const PLAY_MAX_QUARTERS: f64 = 128.0;

/// Tempo of a score (quarters per minute); 90 without a usable value.
pub fn score_tempo(score: &Score) -> f64 {
    score.tempo_bpm.filter(|b| (30.0..=240.0).contains(b)).unwrap_or(90.0)
}

/// Requested tempo (quarters per minute) within sensible limits; without: that of the score.
fn chosen_tempo(tempo: Option<f64>, score: &Score) -> f64 {
    tempo.filter(|t| t.is_finite() && *t > 0.0).map(|t| t.clamp(TEMPO_MIN, TEMPO_MAX)).unwrap_or_else(|| score_tempo(score))
}

const TEMPO_MIN: f64 = 10.0;
const TEMPO_MAX: f64 = 400.0;

/// Notes of the chosen voices (all for `None`) as (start, duration, MIDI) in quarters, sorted by start.
fn score_notes(score: &Score, only: Option<&[(String, String)]>) -> Vec<(f64, f64, u8)> {
    let mut notes = Vec::new();
    for p in &score.parts {
        for v in &p.voices {
            if only.is_some_and(|sel| !sel.iter().any(|(part, label)| p.name == *part && voice_label(v) == *label)) {
                continue;
            }
            notes.extend(v.notes.iter().filter(|n| n.duration > 0.0).map(|n| (n.onset, n.duration, n.midi)));
        }
    }
    notes.sort_by(|a, b| a.0.total_cmp(&b.0));
    notes
}

/// Plays the excerpt [start, end) of a score (quarters) – all voices or
/// only the chosen ones (`only` = pairs of part and voice label) – at the score's tempo.
pub fn play_range(st: &AppState, doc: u32, start: f64, end: f64, only: Option<&[(String, String)]>) -> CmdResult<f32> {
    let (_, score) = doc_and_score(st, doc)?;
    let from = start.max(0.0);
    let to = end.max(from + 0.25).min(from + PLAY_MAX_QUARTERS);
    let spq = (60.0 / score_tempo(&score)) as f32;
    let notes: Vec<NoteSpec> = score_notes(&score, only)
        .into_iter()
        .filter_map(|(on, dur, midi)| {
            let (a, b) = (on.max(from), (on + dur).min(to));
            (b - a > 1e-6).then(|| NoteSpec::new((a - from) as f32 * spq, (b - a) as f32 * spq * 0.97, midi as f32, 0.7))
        })
        .collect();
    if notes.is_empty() {
        return Err(tr!("Nothing plays at this point"));
    }
    let engine = st.engine()?;
    let sr = engine.info().sample_rate as f32;
    let total = (to - from) as f32 * spq + 1.2;
    let inst = crate::sound::default_instrument(klang_core::InstrumentKind::Pluck);
    let data = crate::sound::render_instrument(Some(&inst), &notes, total, sr);
    let secs = data.len() as f32 / sr;
    engine.preview_send(data.into(), 0.8, crate::sound::reverb_send(Some(&inst)));
    Ok(secs)
}

/// This many quarters the score player renders at a time.
const PIECE_QUARTERS: f64 = 4.0;

/// Plays a score piece by piece (for the engine, see [`PreviewSource`]):
/// from `pos` to `to`; with `looping` afterwards again and again from `from`.
struct ScorePlayer {
    notes: Vec<(f64, f64, u8)>,
    pos: f64,
    from: f64,
    to: f64,
    looping: bool,
    /// Seconds per quarter.
    spq: f64,
    sr: f64,
    inst: klang_core::Instrument,
    /// Seconds from the start of the preview at which the next piece begins.
    t: f64,
    /// First piece of a pass: also notes that were struck earlier and are still sounding.
    fresh: bool,
}

impl ScorePlayer {
    /// Notes of the next piece (seconds from its start) and its length in quarters; `None` = finished.
    fn next_piece(&mut self) -> Option<(Vec<NoteSpec>, f64)> {
        if self.pos >= self.to - 1e-9 {
            if !self.looping {
                return None;
            }
            self.pos = self.from;
            self.fresh = true;
        }
        let (a, b) = (self.pos, (self.pos + PIECE_QUARTERS).min(self.to));
        let fresh = std::mem::take(&mut self.fresh);
        let spq = self.spq;
        let to = self.to;
        let specs = self
            .notes
            .iter()
            .filter(|&&(on, dur, _)| (on >= a - 1e-9 && on < b - 1e-9) || (fresh && on < a - 1e-9 && on + dur > a + 1e-6))
            .filter_map(|&(on, dur, midi)| {
                let (s, e) = (on.max(a), (on + dur).min(to));
                (e - s > 1e-6).then(|| NoteSpec::new(((s - a) * spq) as f32, ((e - s) * spq * 0.97) as f32, midi as f32, 0.7))
            })
            .collect();
        self.pos = b;
        Some((specs, b - a))
    }
}

impl klang_engine::PreviewSource for ScorePlayer {
    fn next(&mut self) -> Option<(klang_engine::PlacedClip, u64)> {
        let (specs, len) = self.next_piece()?;
        let start = (self.t * self.sr).round() as u64;
        self.t += len * self.spq;
        let until = (self.t * self.sr).round() as u64;
        let data: Arc<[f32]> = if specs.is_empty() {
            Vec::new().into()
        } else {
            let total = specs.iter().map(|n| n.start_secs + n.dur_secs).fold(0.0f32, f32::max) + 1.2;
            crate::sound::render_instrument(Some(&self.inst), &specs, total, self.sr as f32).into()
        };
        let send = crate::sound::reverb_send(Some(&self.inst));
        Some((klang_engine::PlacedClip { start_frame: start, data, side: None, gain_l: 0.8, gain_r: 0.8, send }, until))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayInfo {
    /// Tempo at which it plays (quarters per minute).
    pub tempo: f64,
    /// From here (quarters) …
    pub start: f64,
    /// … to here; with `looping` afterwards again from `loopStart`.
    pub end: f64,
    pub loop_start: f64,
    pub looping: bool,
}

/// Plays a score until stopped (`preview_stop`): from `start` (quarters)
/// to the end – with `region` only that range, repeatedly. `voices` = pairs
/// (part, voice), without: all voices; `tempo` in quarters per minute, without: that of the score.
pub fn play(
    st: &AppState,
    doc: u32,
    start: f64,
    region: Option<(f64, f64)>,
    voices: Option<Vec<(String, String)>>,
    tempo: Option<f64>,
) -> CmdResult<PlayInfo> {
    let (_, score) = doc_and_score(st, doc)?;
    let only = voices.as_deref().filter(|v| !v.is_empty());
    let notes = score_notes(&score, only);
    let piece_end = notes.iter().map(|n| n.0 + n.1).fold(0.0, f64::max);
    let (from, to, looping) = match region {
        Some((a, b)) if b - a > 1e-6 => (a.max(0.0), b, true),
        _ => (0.0, piece_end, false),
    };
    // Cursor outside the range: at its start
    let start = if start.is_finite() && start >= from && start < to { start } else { from };
    if !notes.iter().any(|&(on, dur, _)| on < to && on + dur > if looping { from } else { start }) {
        return Err(if looping { tr!("Nothing plays in the selected range") } else { tr!("Nothing plays from here on") });
    }
    let tempo = chosen_tempo(tempo, &score);
    let engine = st.engine()?;
    let player = ScorePlayer {
        notes,
        pos: start,
        from,
        to,
        looping,
        spq: 60.0 / tempo,
        sr: engine.info().sample_rate as f64,
        inst: crate::sound::default_instrument(klang_core::InstrumentKind::Pluck),
        t: 0.0,
        fresh: true,
    };
    engine.preview_stream(Box::new(player));
    Ok(PlayInfo { tempo, start, end: to, loop_start: from, looping })
}

/// Beats of the project per quarter of the score: without `tempo` one beat per
/// quarter beat (two beats for x/8 bars), otherwise such that it sounds
/// like `tempo` (quarters per minute) at the project tempo.
fn beats_per_quarter(tempo: Option<f64>, project_bpm: f64, beat_unit: f64) -> f64 {
    match tempo.filter(|t| t.is_finite() && *t > 0.0) {
        Some(t) => project_bpm.max(1.0) / t.clamp(TEMPO_MIN, TEMPO_MAX),
        None => beat_unit / 4.0,
    }
}

/// Takes voices over as instrument tracks – one track per voice, all one
/// undo step; with `range` (quarters) only that excerpt, from beat 0.
/// With `tempo` (quarters per minute) the notes are stretched so that they
/// sound at that tempo at the project tempo; without, quarter = beat.
pub fn to_track(
    st: &AppState,
    doc: u32,
    voices: Vec<(String, String)>,
    range: Option<(f64, f64)>,
    tempo: Option<f64>,
) -> CmdResult<Snapshot> {
    let (info, score) = doc_and_score(st, doc)?;
    let all = score_voices(&score);
    let picked = voices
        .iter()
        .map(|(part, label)| {
            all.iter()
                .position(|(p, v)| p.name == *part && voice_label(v) == *label)
                .ok_or_else(|| tr!("Voice \"{part} {voice}\" not found", part = part, voice = label))
        })
        .collect::<CmdResult<Vec<usize>>>()?;
    let req = ImportRequest { voices: picked, range, tempo, ..Default::default() };
    let (cmd, _) = plan_import(st, &info, &score, &req)?;
    st.execute(cmd, false)
}

// ---------------------------------------------------------------- Scores for the agent
//
// The agent addresses a score by the `doc` of any of its voice entries (from
// `search_library`, `browse_library` or a melody hit) instead of a file path –
// so the same works on the desktop, for the package the browser reads by range
// and for the person's own scores. Texts here are for the agent (English, not
// translated), like the other agent texts.

/// The voices of a score in its order (part by part).
pub fn score_voices(score: &Score) -> Vec<(&Part, &Voice)> {
    score.parts.iter().flat_map(|p| p.voices.iter().map(move |v| (p, v))).collect()
}

/// Name of a voice: its part – with the voice label if the part has several
/// voices (piano staves, divisi).
pub fn voice_name(part: &Part, v: &Voice) -> String {
    match (part.name.trim(), part.voices.len()) {
        ("", _) => voice_label(v),
        (name, 1) => name.to_string(),
        (name, _) => format!("{name} {}", voice_label(v)),
    }
}

/// A voice chosen by its number (1-based, as `read_library_score` lists them) or its name.
#[derive(Clone, Debug, PartialEq)]
pub enum VoicePick {
    Number(usize),
    Name(String),
}

/// Indices of the chosen voices in the order given, each once; nothing chosen = all.
/// A name matches a voice's name or its part (then all voices of the part),
/// ignoring case; failing that, every voice whose name contains it.
pub fn pick_voices(score: &Score, picks: &[VoicePick]) -> Result<Vec<usize>, String> {
    let all = score_voices(score);
    if picks.is_empty() {
        return Ok((0..all.len()).collect());
    }
    let listing = || all.iter().enumerate().map(|(i, (p, v))| format!("{} {}", i + 1, voice_name(p, v))).collect::<Vec<_>>().join(", ");
    let matching = |f: &dyn Fn(&Part, &Voice) -> bool| all.iter().enumerate().filter(|(_, (p, v))| f(p, v)).map(|(i, _)| i).collect::<Vec<_>>();
    let mut out = Vec::new();
    for pick in picks {
        let found = match pick {
            VoicePick::Number(n) if (1..=all.len()).contains(n) => vec![n - 1],
            VoicePick::Number(n) => return Err(format!("There is no voice {n} – the voices are: {}", listing())),
            VoicePick::Name(name) => {
                let want = name.trim().to_lowercase();
                let mut f = matching(&|p, v| voice_name(p, v).to_lowercase() == want);
                if f.is_empty() {
                    f = matching(&|p, _| p.name.trim().to_lowercase() == want);
                }
                if f.is_empty() && !want.is_empty() {
                    f = matching(&|p, v| voice_name(p, v).to_lowercase().contains(&want));
                }
                if f.is_empty() {
                    return Err(format!("No voice \"{name}\" – the voices are: {}", listing()));
                }
                f
            }
        };
        for i in found {
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
    Ok(out)
}

/// Bars `from`…`to` (inclusive, as printed; 0 = pickup) as quarters [start, end).
pub fn bars_range(score: &Score, from: u32, to: u32) -> Result<(f64, f64), String> {
    let count = score.bar_count();
    let first = if score.has_pickup() { 0 } else { 1 };
    if from > to {
        return Err(format!("fromBar ({from}) is after toBar ({to})"));
    }
    if from > count || to < first {
        return Err(format!("The score has bars {first}–{count}"));
    }
    let (from, to) = (from.max(first), to.min(count));
    Ok((score.bar_start(from), score.bar_start(to + 1)))
}

fn file_name(source: &str) -> String {
    source.rsplit(['/', '\\']).next().unwrap_or(source).to_string()
}

/// A score found by `search_library`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreHit {
    /// A voice entry of the score – any of its voices stands for the score.
    doc: u32,
    title: String,
    composer: String,
    file: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    origin: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    collection: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    license: String,
}

/// Text search for scores (title, composer, instruments, genre …) – one hit per score.
pub fn find_scores(st: &AppState, text: &str, limit: usize) -> Result<Vec<ScoreHit>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Give a search text (words from the title, composer or instruments).".into());
    }
    let limit = limit.clamp(1, 200);
    let lib = lock(&st.library)?;
    let lib = lib.as_ref().ok_or("No library loaded – it is loaded at startup; check with library_status.")?;
    // depending on the index a hit is a score or a single voice: one per score
    let group = |raw: Vec<(u32, klang_music::index::DocInfo)>| {
        let mut seen = std::collections::HashSet::new();
        raw.into_iter()
            .filter(|(_, i)| seen.insert((i.source.clone(), i.zip)))
            .map(|(doc, i)| ScoreHit {
                doc,
                title: if i.title.trim().is_empty() { "(untitled)".into() } else { i.title.trim().to_string() },
                composer: i.composer,
                file: file_name(&i.source),
                origin: origin(&lib.index, doc),
                collection: i.collection,
                license: i.license,
            })
            .collect::<Vec<_>>()
    };
    let raw = lib.index.search_text(text, limit);
    let more = raw.len() >= limit;
    let mut hits = group(raw);
    if more && hits.len() < limit {
        hits = group(lib.index.search_text(text, limit * 8));
    }
    hits.truncate(limit);
    Ok(hits)
}

/// A voice as `read_library_score` lists it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRead {
    /// Number for choosing it (1-based).
    voice: usize,
    name: String,
    /// The part (instrument) it belongs to – a part can have several voices (divisi, piano staves).
    part: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    notes: usize,
    /// Lowest – highest note.
    range: String,
    /// First and last bar in which it plays.
    bars: (u32, u32),
    /// Notes of the excerpt, compact; start positions in quarters from the excerpt's start.
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcerptRead {
    from_bar: u32,
    to_bar: u32,
    /// Where the excerpt lies in the score (quarters).
    start_quarter: f64,
    end_quarter: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CadenceRead {
    bar: u32,
    kind: String,
}

/// A library score for the agent: voices, bars, key – and the notes of an excerpt.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreRead {
    doc: u32,
    title: String,
    composer: String,
    file: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    origin: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    collection: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    license: String,
    /// Estimated key in the German notation of the other tools ("Es-Dur", "a-Moll").
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_signature: Option<String>,
    /// Notated tempo in quarters per minute.
    #[serde(skip_serializing_if = "Option::is_none")]
    tempo: Option<f64>,
    /// Number of bars (as printed) and whether a pickup (bar 0) comes before bar 1.
    bars: u32,
    pickup: bool,
    quarters: f64,
    mood: String,
    voices: Vec<VoiceRead>,
    cadences: Vec<CadenceRead>,
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<ExcerptRead>,
}

/// At most this many notes per voice in an excerpt.
const READ_MAX_NOTES: usize = 400;

/// Notes of a voice in [from, to), trimmed at the edges, from `from`: (start, length, MIDI) in quarters.
fn excerpt_notes(v: &Voice, from: f64, to: f64) -> Vec<(f64, f64, u8)> {
    v.notes
        .iter()
        .filter_map(|n| {
            let (a, b) = (n.onset.max(from), (n.onset + n.duration).min(to));
            (b - a > 1e-6).then_some((a - from, b - a, n.midi))
        })
        .collect()
}

/// Reads a score: overview of all voices and – with `bars` or `notes` – the
/// notes of the chosen voices (`picks`, nothing = all) in that bar range.
pub fn read_score(st: &AppState, doc: u32, bars: Option<(u32, u32)>, picks: &[VoicePick], notes: bool) -> Result<ScoreRead, String> {
    let (info, score) = doc_and_score(st, doc)?;
    let range = bars.map(|(a, b)| bars_range(&score, a, b)).transpose()?;
    let chosen = pick_voices(&score, picks)?;
    let analysis = analyze_score(&score);
    let with_notes = notes || bars.is_some();
    let (from, to) = range.unwrap_or((0.0, f64::MAX));
    let voices = score_voices(&score)
        .into_iter()
        .enumerate()
        .map(|(i, (p, v))| {
            let low = v.notes.iter().map(|n| n.midi).min().unwrap_or(0);
            let high = v.notes.iter().map(|n| n.midi).max().unwrap_or(0);
            let first = v.notes.first().map(|n| score.bar_at(n.onset)).unwrap_or(0);
            let last = v.notes.iter().map(|n| n.onset).fold(0.0, f64::max);
            let excerpt = (with_notes && chosen.contains(&i)).then(|| {
                let all = excerpt_notes(v, from, to);
                if all.is_empty() {
                    return "(rests)".to_string();
                }
                let mut text = crate::notation::format(&all[..all.len().min(READ_MAX_NOTES)]);
                if all.len() > READ_MAX_NOTES {
                    text += &format!(" · … +{} more notes – read fewer bars", all.len() - READ_MAX_NOTES);
                }
                text
            });
            VoiceRead {
                voice: i + 1,
                name: voice_name(p, v),
                part: p.name.trim().to_string(),
                role: analysis.voices.iter().find(|r| r.part == p.name && r.staff == v.staff && r.voice == v.voice).map(|r| r.role_label.clone()),
                notes: v.notes.len(),
                range: format!("{}–{}", crate::sound::note_name(low), crate::sound::note_name(high)),
                bars: (first, score.bar_at(last)),
                excerpt,
            }
        })
        .collect();
    let excerpt = match (with_notes, range) {
        (true, Some((a, b))) => Some(ExcerptRead { from_bar: score.bar_at(a), to_bar: score.bar_at((b - 1e-3).max(a)), start_quarter: a, end_quarter: b }),
        (true, None) => Some(ExcerptRead {
            from_bar: if score.has_pickup() { 0 } else { 1 },
            to_bar: score.bar_count(),
            start_quarter: 0.0,
            end_quarter: score.duration_quarters(),
        }),
        _ => None,
    };
    let origin = lock(&st.library)?.as_ref().map(|l| origin(&l.index, doc)).unwrap_or("");
    Ok(ScoreRead {
        doc,
        title: if info.title.trim().is_empty() { "(untitled)".into() } else { info.title.trim().to_string() },
        composer: info.composer,
        file: file_name(&info.source),
        origin,
        collection: info.collection,
        license: info.license,
        key: analysis.key_name.clone(),
        time_signature: score.time_signature.map(|(n, d)| format!("{n}/{d}")),
        tempo: score.tempo_bpm,
        bars: score.bar_count(),
        pickup: score.has_pickup(),
        quarters: analysis.duration_quarters,
        mood: klang_i18n::translate(analysis.mood.label),
        voices,
        cadences: analysis.cadences.iter().take(60).map(|c| CadenceRead { bar: score.bar_at(c.onset), kind: c.label.to_string() }).collect(),
        excerpt,
    })
}

/// Which voices of a score go into the project, and how.
#[derive(Clone, Debug, Default)]
pub struct ImportRequest {
    /// Indices into [`score_voices`].
    pub voices: Vec<usize>,
    /// One track per part with all its chosen voices (divisi, piano staves) instead of one per voice.
    pub by_part: bool,
    /// Only this excerpt (quarters), from its start.
    pub range: Option<(f64, f64)>,
    /// Quarters per minute it should sound at the project tempo; `None` = quarter = beat.
    pub tempo: Option<f64>,
    /// Where the clips begin (beats).
    pub start_beats: f64,
    /// Per track to be made (part or voice, in the order of `voices`) an existing
    /// instrument track that gets the clip instead (`None` = a new track).
    pub targets: Vec<Option<Id>>,
}

/// A part or voice that `import_voices` brought into the project.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedTrack {
    /// The score's voices in it (numbers as `read_library_score` lists them).
    voices: Vec<usize>,
    name: String,
    track_id: u64,
    clip_id: u64,
    track: String,
    notes: usize,
    new_track: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    title: String,
    tracks: Vec<ImportedTrack>,
    /// Chosen voices without notes in the range (not imported).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skipped: Vec<String>,
    start_beats: f64,
    end_beats: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    bars: Option<(u32, u32)>,
    /// Project beats per quarter of the score.
    beats_per_quarter: f64,
}

/// The command that takes the chosen voices over (one undo step) and what it brings in.
fn plan_import(st: &AppState, info: &klang_music::index::DocInfo, score: &Score, req: &ImportRequest) -> CmdResult<(Command, Imported)> {
    if req.voices.is_empty() {
        return Err(tr!("No voice selected."));
    }
    let all = score_voices(score);
    let (bpb, per_quarter) = {
        let s = lock(&st.session)?;
        let p = &s.project;
        (p.beats_per_bar as f64, beats_per_quarter(req.tempo, p.tempo_bpm, p.beat_unit as f64))
    };
    // Excerpt: notes in the range, trimmed at the edges, from 0
    let (from, to) = match req.range {
        Some((a, b)) if b > a => (a.max(0.0), b),
        _ => (0.0, f64::MAX),
    };
    let bars = req.range.map(|(a, b)| (score.bar_at(a), score.bar_at((b - 1e-3).max(a))));
    let suffix = bars.map(|(a, b)| format!(" {}", tr!("(bars {from}–{to})", from = a, to = b))).unwrap_or_default();
    let title = if info.title.is_empty() { tr!("Library") } else { info.title.clone() };
    // One track per voice – or per part with its voices together
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &i in &req.voices {
        let (p, _) = all.get(i).ok_or_else(|| format!("There is no voice {}", i + 1))?;
        match groups.iter_mut().find(|g| req.by_part && std::ptr::eq(all[g[0]].0, *p)) {
            Some(g) => g.push(i),
            None => groups.push(vec![i]),
        }
    }
    if !req.targets.is_empty() && req.targets.len() != groups.len() {
        // agent only
        let per = if req.by_part { "part" } else { "voice" };
        return Err(format!("trackIds has {} entries for {} tracks – one per chosen {per}, in their order (0 = new track)", req.targets.len(), groups.len()));
    }
    // Several voices of the same part on tracks of their own (e.g. piano upper/lower): include the voice label in the name
    let shared_part = |part: &str| req.voices.iter().filter(|&&i| all.get(i).is_some_and(|(p, _)| p.name == part)).count() > 1;
    let start = req.start_beats.max(0.0);
    // Voices without notes in the range are skipped
    let mut planned = Vec::new();
    let mut skipped = Vec::new();
    for (k, group) in groups.iter().enumerate() {
        let (p, v) = all[group[0]];
        let shown = if req.by_part && !p.name.trim().is_empty() { p.name.trim().to_string() } else { voice_name(p, v) };
        let mut notes: Vec<(f64, f64, u8)> = group.iter().flat_map(|&i| excerpt_notes(all[i].1, from, to)).collect();
        notes.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.2.cmp(&a.2)).then(a.1.total_cmp(&b.1)));
        // voices of one part in unison: each note once
        notes.dedup_by(|b, a| a.2 == b.2 && (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
        let notes: Vec<Note> = notes.into_iter().map(|(a, len, midi)| Note::new(midi, a * per_quarter, (len * per_quarter).max(0.05), 96)).collect();
        if notes.is_empty() {
            skipped.push(shown);
            continue;
        }
        let end = notes.iter().map(|n| n.start_beats + n.length_beats).fold(0.0, f64::max);
        let mean = notes.iter().map(|n| n.pitch as f64).sum::<f64>() / notes.len() as f64;
        let kind = if mean < 48.0 { klang_core::InstrumentKind::Bass } else { klang_core::InstrumentKind::Pluck };
        let name = match p.name.trim() {
            "" => voice_label(v),
            n if !req.by_part && shared_part(&p.name) => format!("{n} {}", voice_label(v)),
            n => n.to_string(),
        };
        planned.push((group.iter().map(|i| i + 1).collect::<Vec<_>>(), shown, name, kind, end, notes, req.targets.get(k).copied().flatten()));
    }
    if planned.is_empty() {
        return Err(match (req.voices.len() == 1, req.range.is_some()) {
            (true, true) => tr!("This voice has no notes in the selected range."),
            (true, false) => tr!("This voice has no notes."),
            (false, true) => tr!("The selected voices have no notes in the selected range."),
            (false, false) => tr!("The selected voices have no notes."),
        });
    }
    let mut s = lock(&st.session)?;
    let p = &mut s.project;
    let mut commands = Vec::new();
    let mut tracks = Vec::new();
    let mut end_beats = start;
    for (voices, shown, name, kind, end, notes, target) in planned {
        let length = (end / bpb).ceil().max(1.0) * bpb;
        end_beats = end_beats.max(start + length);
        let count = notes.len();
        let clip = Clip { id: p.alloc_id(), name: format!("{title}{suffix}"), start_beats: start, source: ClipSource::Notes { length_beats: length, notes } };
        let clip_id = clip.id.0;
        match target {
            // agent only: an existing track gets the clip
            Some(track_id) => {
                let t = p.tracks.iter().find(|t| t.id == track_id).ok_or_else(|| format!("Track {} doesn't exist", track_id.0))?;
                if t.kind != TrackKind::Instrument {
                    return Err(format!("Track {} (\"{}\") is not an instrument track", track_id.0, t.name));
                }
                tracks.push(ImportedTrack { voices, name: shown, track_id: track_id.0, clip_id, track: t.name.clone(), notes: count, new_track: false });
                commands.push(Command::AddClip { track_id, clip });
            }
            None => {
                let mut track = Track::new(p.alloc_id(), format!("{title} – {name}{suffix}"), TrackKind::Instrument);
                track.instrument = Some(crate::sound::default_instrument(kind));
                tracks.push(ImportedTrack { voices, name: shown, track_id: track.id.0, clip_id, track: track.name.clone(), notes: count, new_track: true });
                track.clips.push(clip);
                commands.push(Command::AddTrack { track, index: None });
            }
        }
    }
    let n = tracks.len();
    let label = if n > 1 { trn!(n, "Import {n} voice from library", "Import {n} voices from library") } else { tr!("Import from library") };
    let out = Imported { title, tracks, skipped, start_beats: start, end_beats, bars, beats_per_quarter: per_quarter };
    Ok((Command::Batch { label, commands }, out))
}

/// Takes the chosen voices of a library score (from [`doc_and_score`]) into the
/// project – one undo step; with `if_revision` only if nobody changed the
/// project in the meantime.
pub fn import_voices(st: &AppState, info: &klang_music::index::DocInfo, score: &Score, req: &ImportRequest, if_revision: Option<u64>) -> CmdResult<Imported> {
    let (cmd, out) = plan_import(st, info, score, req)?;
    st.execute_if(cmd, if_revision)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(notes: Vec<(f64, f64, u8)>, pos: f64, from: f64, to: f64, looping: bool) -> ScorePlayer {
        ScorePlayer {
            notes,
            pos,
            from,
            to,
            looping,
            spq: 0.5,
            sr: 1000.0,
            inst: crate::sound::default_instrument(klang_core::InstrumentKind::Pluck),
            t: 0.0,
            fresh: true,
        }
    }

    #[test]
    fn score_player_starts_at_the_cursor_with_sounding_notes() {
        // long note from 0 is still sounding at the cursor (quarter 2): it is played from there
        let mut p = player(vec![(0.0, 4.0, 60), (3.0, 1.0, 62), (6.5, 1.0, 64)], 2.0, 0.0, 7.5, false);
        let (first, len) = p.next_piece().unwrap();
        assert_eq!(len, 4.0);
        let got: Vec<_> = first.iter().map(|n| (n.start_secs, n.midi)).collect();
        assert_eq!(got, vec![(0.0, 60.0), (0.5, 62.0)]);
        assert!((first[0].dur_secs - 2.0 * 0.5 * 0.97).abs() < 1e-6, "nur der Rest der Note");
        let (second, len) = p.next_piece().unwrap();
        assert_eq!((len, second.len()), (1.5, 1), "letztes Stück bis zum Ende");
        assert!(p.next_piece().is_none(), "ohne Schleife ist am Ende Schluss");
    }

    #[test]
    fn score_player_loops_the_region_until_stopped() {
        let mut p = player(vec![(0.0, 1.0, 60), (1.0, 2.0, 62), (8.0, 1.0, 64)], 1.5, 1.0, 2.0, true);
        let mut rounds = Vec::new();
        for _ in 0..4 {
            let (n, len) = p.next_piece().unwrap();
            rounds.push((n.iter().map(|n| n.midi as u8).collect::<Vec<_>>(), len));
        }
        // first the rest from the cursor, then the whole range again and again – the note trimmed to its end
        assert_eq!(rounds, vec![(vec![62], 0.5), (vec![62], 1.0), (vec![62], 1.0), (vec![62], 1.0)]);
        assert!((klang_engine::PreviewSource::next(&mut p).unwrap().1) > 0);
    }

    #[test]
    fn insert_tempo_maps_onto_the_project_tempo() {
        // without a value: quarter = beat (two beats for x/8)
        assert_eq!(beats_per_quarter(None, 120.0, 4.0), 1.0);
        assert_eq!(beats_per_quarter(None, 120.0, 8.0), 2.0);
        // score at 60 in a project at 120: each quarter becomes two beats long
        assert_eq!(beats_per_quarter(Some(60.0), 120.0, 4.0), 2.0);
        assert_eq!(beats_per_quarter(Some(240.0), 120.0, 4.0), 0.5);
    }

    #[test]
    fn parses_german_and_english_note_names() {
        let n = |s: &str| parse_melody(s).unwrap().iter().map(|m| m.midi).collect::<Vec<_>>();
        assert_eq!(n("C4 D4 E4"), vec![60, 62, 64]);
        assert_eq!(n("Cis4 Es4 Fis3 As3"), vec![61, 63, 54, 56]);
        assert_eq!(n("H3 B3 Bb3 C#4 Eb4"), vec![59, 58, 58, 61, 63]);
        assert_eq!(n("60, 62; 64"), vec![60, 62, 64]);
        let d = parse_melody("C4:2 D4:0,5 E4").unwrap();
        assert_eq!((d[1].onset, d[2].onset), (2.0, 2.5));
        assert!(parse_melody("X4").is_err());
        assert!(parse_melody("C4:x").is_err());
    }

    /// (MIDI, beats) → melody in beats
    fn tune(n: &[(u8, f64)]) -> Vec<MelodyNote> {
        let mut t = 0.0;
        n.iter()
            .map(|&(midi, d)| {
                let m = MelodyNote { onset: t, duration: d, midi };
                t += d;
                m
            })
            .collect()
    }

    const HAENSCHEN: [(u8, f64); 13] =
        [(67, 1.), (64, 1.), (64, 2.), (65, 1.), (62, 1.), (62, 2.), (60, 1.), (62, 1.), (64, 1.), (65, 1.), (67, 1.), (67, 1.), (67, 2.)];
    const ENTCHEN: [(u8, f64); 16] = [(60, 1.), (62, 1.), (64, 1.), (65, 1.), (67, 2.), (67, 2.), (69, 1.), (69, 1.), (69, 1.), (69, 1.), (67, 4.), (69, 1.), (69, 1.), (69, 1.), (69, 1.), (67, 4.)];
    const JAKOB: [(u8, f64); 14] = [(60, 1.), (62, 1.), (64, 1.), (60, 1.), (60, 1.), (62, 1.), (64, 1.), (60, 1.), (64, 1.), (65, 1.), (67, 2.), (64, 1.), (65, 1.), (67, 2.)];

    /// Whistled: nearly pure sine with slight vibrato and breath noise, short gaps.
    fn whistle(notes: &[(u8, f64)], transpose: i32, secs_per_beat: f32, sr: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let (mut phase, mut seed) = (0.0f32, 12345u32);
        for &(midi, beats) in notes {
            let secs = beats as f32 * secs_per_beat - 0.04;
            for i in 0..(secs * sr) as usize {
                let t = i as f32 / sr;
                let m = (midi as i32 + transpose) as f32 + 0.1 * (2.0 * std::f32::consts::PI * 6.0 * t).sin();
                phase += 2.0 * std::f32::consts::PI * 440.0 * 2f32.powf((m - 69.0) / 12.0) / sr;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                let env = (t / 0.015).min(1.0) * ((secs - t) / 0.025).clamp(0.0, 1.0);
                out.push(env * (0.35 * phase.sin() + 0.02 * noise));
            }
            out.extend(std::iter::repeat(0.0).take((0.04 * sr) as usize));
        }
        out
    }

    #[test]
    fn whistled_melody_above_1400_hz_is_found() {
        let sr = 44_100.0;
        // Hänschen klein, 26 semitones higher (a''' ≈ 1760 Hz) and whistled faster
        let x = whistle(&HAENSCHEN[..10], 26, 0.3, sr);
        let (q, key) = crate::analysis::query_melody(&x, sr as u32);
        let got: Vec<i32> = q.windows(2).map(|w| w[1].midi as i32 - w[0].midi as i32).collect();
        let want: Vec<i32> = HAENSCHEN[..10].windows(2).map(|w| w[1].0 as i32 - w[0].0 as i32).collect();
        assert_eq!(got, want, "Intervalle: {q:?}");
        assert_eq!(q[0].midi, 93);
        // From ten notes, D major and E minor are equally plausible
        assert!(matches!(key.as_deref(), Some("D major" | "E minor")), "{key:?}");
        assert!(format_melody(&q).starts_with("A6:1 Fis6:1 Fis6:2 G6:1"), "{}", format_melody(&q));

        let mut idx = klang_music::index::MelodyIndex::new();
        for (title, t) in [("Alle meine Entchen", &ENTCHEN[..]), ("Hänschen klein", &HAENSCHEN[..]), ("Bruder Jakob", &JAKOB[..])] {
            let info = klang_music::index::DocInfo { title: title.into(), ..Default::default() };
            idx.add_melody(info, &tune(t));
        }
        let index = AnyIndex::Mem(idx);
        let hits = melody_hits(&index, &q, None, 5).unwrap();
        assert_eq!(hits[0].title, "Hänschen klein", "{:?}", hits.iter().map(|h| (&h.title, h.score)).collect::<Vec<_>>());
        // Text filter restricts
        assert!(melody_hits(&index, &q, Some("jakob"), 5).unwrap().iter().all(|h| h.title == "Bruder Jakob"));
        // too few notes
        assert!(melody_hits(&index, &q[..3], None, 5).is_err());
    }

    #[test]
    fn sung_melody_in_low_voice_is_found() {
        // Male voice: Bruder Jakob an octave lower, with overtones (as sung)
        let sr = 44_100.0;
        let mut x = Vec::new();
        let mut phase = 0.0f32;
        for &(midi, beats) in &JAKOB {
            let secs = beats as f32 * 0.35 - 0.04;
            for i in 0..(secs * sr) as usize {
                let t = i as f32 / sr;
                let f = 440.0 * 2f32.powf((midi as f32 - 12.0 + 0.15 * (2.0 * std::f32::consts::PI * 5.5 * t).sin() - 69.0) / 12.0);
                phase += 2.0 * std::f32::consts::PI * f / sr;
                let env = (t / 0.02).min(1.0) * ((secs - t) / 0.03).clamp(0.0, 1.0);
                x.push(env * (0.4 * phase.sin() + 0.2 * (2.0 * phase).sin() + 0.1 * (3.0 * phase).sin()));
            }
            x.extend(std::iter::repeat(0.0).take((0.04 * sr) as usize));
        }
        let (q, _) = crate::analysis::query_melody(&x, sr as u32);
        assert_eq!(q.iter().map(|n| n.midi).collect::<Vec<_>>(), JAKOB.iter().map(|n| n.0 - 12).collect::<Vec<_>>());
        let mut idx = klang_music::index::MelodyIndex::new();
        for (title, t) in [("Alle meine Entchen", &ENTCHEN[..]), ("Hänschen klein", &HAENSCHEN[..]), ("Bruder Jakob", &JAKOB[..])] {
            idx.add_melody(klang_music::index::DocInfo { title: title.into(), ..Default::default() }, &tune(t));
        }
        let hits = melody_hits(&AnyIndex::Mem(idx), &q, None, 5).unwrap();
        assert_eq!(hits[0].title, "Bruder Jakob");
    }
}
