//! Application state: session (project + undo), audio assets, engine,
//! melody library and agent interface.

use crate::analysis::{self, Analysis, LiveAnalysis, LiveCursor, LiveView};
use crate::sound::{self, SoundReport};
use klang_core::{Clip, ClipSource, Command, Id, InstrumentKind, Note, Project, Session, Track, TrackKind, TrackPatch};
use klang_dsp::synth::NoteSpec;
use klang_dsp::{stretch, util};
use klang_engine::{Arrangement, Engine, LiveNote, LiveNotes, NoteClip, PlacedClip, StepRender};
use klang_i18n::tr;
use klang_music::diskindex::AnyIndex;
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub type CmdResult<T> = Result<T, String>;

pub fn lock<T>(m: &Mutex<T>) -> CmdResult<MutexGuard<'_, T>> {
    m.lock().map_err(|_| tr!("Internal error: state is locked"))
}

pub struct Asset {
    #[allow(dead_code)] // for the planned asset browser
    pub name: String,
    pub samples: Arc<[f32]>,
    pub sample_rate: u32,
}

/// Audio material lives here – the project only refers to it by ID.
#[derive(Default)]
pub struct AssetPool {
    assets: HashMap<String, Asset>,
    renders: HashMap<String, Arc<[f32]>>,
    /// Note clips prepared for the live mixer (they hold for every tempo).
    live: HashMap<String, Arc<LiveNotes>>,
    /// Note clips rendered because the live mixer couldn't keep up (mid,
    /// side) – key with the tempo, so a new tempo plays them live again.
    prerendered: HashMap<String, (Arc<[f32]>, Option<Arc<[f32]>>)>,
    analyses: HashMap<String, Analysis>,
    /// What the polyphonic analysis found per recording (see `voices`).
    pub(crate) voices: HashMap<String, Arc<crate::voices::PolyRaw>>,
    counter: u64,
}

impl AssetPool {
    /// Branch for the export: the same sounds (shared), its own
    /// cache – what the export renders displaces nothing of playback.
    fn fork(&self) -> AssetPool {
        AssetPool {
            assets: self.assets.iter().map(|(k, a)| (k.clone(), Asset { name: a.name.clone(), samples: a.samples.clone(), sample_rate: a.sample_rate })).collect(),
            renders: self.renders.clone(),
            live: self.live.clone(),
            prerendered: self.prerendered.clone(),
            analyses: HashMap::new(),
            voices: HashMap::new(),
            counter: self.counter,
        }
    }

    pub fn insert(&mut self, name: String, samples: Vec<f32>, sample_rate: u32) -> String {
        self.counter += 1;
        let id = format!("asset-{}", self.counter);
        self.assets.insert(id.clone(), Asset { name, samples: samples.into(), sample_rate });
        id
    }

    pub fn get(&self, id: &str) -> Option<&Asset> {
        self.assets.get(id)
    }

    /// Replaces all recordings (when opening a project); IDs are kept.
    pub fn replace_all(&mut self, audio: Vec<crate::project::AudioData>) {
        self.assets.clear();
        self.renders.clear();
        self.analyses.clear();
        for a in audio {
            if let Some(n) = a.id.strip_prefix("asset-").and_then(|n| n.parse::<u64>().ok()) {
                self.counter = self.counter.max(n);
            }
            self.assets.insert(a.id, Asset { name: a.name, samples: a.samples.into(), sample_rate: a.sample_rate });
        }
    }
}

/// The active library (see `libraries`): exactly one index file.
pub struct Library {
    /// Identifier in the library list (`bibliotheken.json`)
    pub id: String,
    pub name: String,
    /// Index file
    pub path: String,
    /// Index on disk (format 4) or older file in memory.
    pub index: AnyIndex,
    /// Contained packages from the catalog
    pub packs: Vec<String>,
    /// Genre/composer are available for all contained PDMX packages
    pub has_meta: bool,
}

#[derive(Default)]
pub struct AgentLink {
    /// Address of the agent server (MCP), if it is running.
    pub url: Option<String>,
    pub error: Option<String>,
    /// The running MCP server of the desktop (keeps it alive; counts
    /// connected clients). In the browser there is none – `None` there.
    pub server: Option<klang_mcp::ServerHandle>,
}

/// An agent counts as connected as long as it keeps a connection open
/// (event stream, running request such as a waiting `chat_poll`). Only one that
/// never opens a stream (scripts) counts for that long after its last request.
pub const AGENT_GRACE: std::time::Duration = std::time::Duration::from_secs(2 * 60);

/// Keys of the chosen audio devices in the settings (missing/null = system default).
pub const SETTING_OUTPUT: &str = "ausgabegeraet";
pub const SETTING_INPUT: &str = "eingabegeraet";

/// Why a remembered device wasn't used at start.
#[derive(Clone)]
enum DeviceProblem {
    /// Not connected (name).
    Missing(String),
    /// Couldn't be opened or chosen (name, error).
    Failed(String, String),
}

/// Notice when a remembered device was missing at start (the choice stays remembered).
/// The text is made only when asked for – at start the UI hasn't set its
/// language yet.
#[derive(Default, Clone)]
pub struct AudioNotice {
    output: Option<DeviceProblem>,
    input: Option<DeviceProblem>,
}

impl AudioNotice {
    fn output_text(&self) -> Option<String> {
        self.output.as_ref().map(|p| match p {
            DeviceProblem::Missing(n) => tr!("Output device \"{name}\" is not connected – Klangwelt plays through the system default.", name = n),
            DeviceProblem::Failed(n, e) => {
                tr!("Output device \"{name}\" couldn't be opened ({error}) – Klangwelt plays through the system default.", name = n, error = e)
            }
        })
    }

    fn input_text(&self) -> Option<String> {
        self.input.as_ref().map(|p| match p {
            DeviceProblem::Missing(n) => tr!("Input device \"{name}\" is not connected – Klangwelt records from the system default.", name = n),
            DeviceProblem::Failed(n, e) => {
                tr!("Input device \"{name}\" couldn't be selected ({error}) – Klangwelt records from the system default.", name = n, error = e)
            }
        })
    }
}

/// Audio devices for the picker (UI and agent).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevicesDto {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
    /// Current default devices of the system
    pub default_output: Option<String>,
    pub default_input: Option<String>,
    /// Chosen devices (null = system default)
    pub output: Option<String>,
    pub input: Option<String>,
    /// Output device that is playing right now, and its sample rate
    pub output_device: String,
    pub sample_rate: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfoDto {
    available: bool,
    error: Option<String>,
    output_device: Option<String>,
    /// Chosen output/recording device (null = system default).
    output_choice: Option<String>,
    input_choice: Option<String>,
    /// Remembered device was missing at start – Klangwelt uses the system default.
    output_notice: Option<String>,
    input_notice: Option<String>,
    sample_rate: u32,
    metronome: bool,
    /// Repeat playback (start over at the end).
    loop_on: bool,
    /// Selected time range in beats; with `loop_on` the loop, otherwise the whole piece.
    loop_range: Option<(f64, f64)>,
    /// End of the piece in beats (where the last note or audio clip ends): without
    /// a loop and a range, playback stops there and leaves the playhead at that spot.
    end_beats: f64,
}

/// Repeat and selected time range (not part of the project).
#[derive(Clone, Copy, Default)]
pub struct LoopState {
    pub on: bool,
    pub range: Option<(f64, f64)>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfoDto {
    url: Option<String>,
    error: Option<String>,
    library: Option<String>,
    library_voices: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Project file (None = not saved yet).
    file: Option<String>,
    /// Unsaved changes since the last save/open.
    dirty: bool,
    project: Project,
    can_undo: bool,
    can_redo: bool,
    undo_label: Option<String>,
    redo_label: Option<String>,
    engine: EngineInfoDto,
    agent: AgentInfoDto,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub playing: bool,
    /// A preview is sounding right now (library, chords …).
    pub previewing: bool,
    pub recording: bool,
    pub position_secs: f64,
    pub peak_out: f32,
    pub peak_in: f32,
    /// Share of the real-time budget the live mixer needs (1 = all of it).
    pub load: f32,
    /// Note clips currently pre-rendered because the mixer couldn't keep up.
    pub prerendered: usize,
    /// A recording with live analysis is running (`record_live` has it) –
    /// not the library's sing-to-search.
    pub live_analysis: bool,
}

/// State of the agent indicator: listening, working, tasks in progress, connected.
type AgentKey = (bool, bool, Vec<u64>, bool);

pub struct AppState {
    /// Chat between UI and agent (requests via `chat_poll`).
    pub chat: crate::chat::Chat,
    pub session: Mutex<Session>,
    pub pool: Mutex<AssetPool>,
    pub library: Mutex<Option<Library>>,
    pub library_status: Mutex<crate::library::LibStatus>,
    /// Cancel the running download/index run of the library.
    pub library_cancel: AtomicBool,
    /// Running download process (curl), so "Cancel" can end it.
    pub download_child: Mutex<Option<std::process::Child>>,
    pub agent: Mutex<AgentLink>,
    engine: Option<Engine>,
    engine_error: Option<String>,
    /// Metronome on (the person's choice; off by default).
    metronome: AtomicBool,
    /// Silenced for the running recording, so it doesn't click into the
    /// microphone – until the person switches it on during the recording.
    metronome_muted: AtomicBool,
    looping: Mutex<LoopState>,
    pub record_counter: AtomicU32,
    /// Analysis of the running recording (fed in `tick`), with the number of
    /// the last recording.
    live: Mutex<Option<LiveAnalysis>>,
    /// The polyphonic live analysis, when the recording was started in that mode.
    live_voices: Mutex<Option<crate::voices::LiveVoices>>,
    live_take: AtomicU32,
    /// File of the open project (None = never saved).
    pub project_file: Mutex<Option<std::path::PathBuf>>,
    /// Checksum of the project at the last save/open (for "unsaved").
    pub(crate) saved_hash: Mutex<u64>,
    /// Close the window despite unsaved changes (after asking).
    pub force_close: AtomicBool,
    /// Tracks last created by the song builder (creating again replaces them); applies only to the open project.
    pub song_tracks: Mutex<Vec<Id>>,
    /// Settings file (set at start; nothing is remembered before that).
    pub settings_file: Mutex<Option<std::path::PathBuf>>,
    /// Remembered audio device was missing at start.
    audio_notice: Mutex<AudioNotice>,
    /// The platform (events, storage locations); set at start.
    host: std::sync::OnceLock<Box<dyn crate::Host>>,
    /// Pre-rendering when the live mixer can't keep up (see `tick`).
    prerender: Mutex<Prerender>,
    /// For tests: the mixer counts as at least this loaded (f32 bits).
    test_load: AtomicU32,
    /// Agent indicator: next check (Unix ms) and the state reported last.
    agent_watch: Mutex<(u64, Option<AgentKey>)>,
}

/// Checksum for "unsaved": the content without the internal counter for new
/// IDs – it keeps growing when a step is undone (e.g. the demo after the
/// intro tour), and the project is unchanged all the same.
pub(crate) fn project_hash(p: &Project) -> u64 {
    let mut v = serde_json::to_value(p).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("nextId");
    }
    let mut h = DefaultHasher::new();
    v.to_string().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod hash_tests {
    use super::*;

    #[test]
    fn id_counter_does_not_count_as_a_change() {
        let a = Project::default();
        let mut b = a.clone();
        b.alloc_id();
        b.alloc_id();
        assert_eq!(project_hash(&a), project_hash(&b));
        b.tempo_bpm = 99.0;
        assert_ne!(project_hash(&a), project_hash(&b));
    }

    #[test]
    fn metronome_is_off_until_switched_on() {
        let st = AppState::without_audio();
        assert!(!st.snapshot().unwrap().engine.metronome, "off by default");
        assert!(st.set_metronome(true).unwrap().engine.metronome);
        assert!(!st.set_metronome(false).unwrap().engine.metronome);
    }

    #[test]
    fn the_piece_ends_where_its_last_sound_ends() {
        let st = AppState::without_audio();
        let run = |cmd: serde_json::Value| st.execute(serde_json::from_value(cmd).unwrap(), false).unwrap();
        let end = |s: &Snapshot| (s.engine.end_beats, SongEnd::of(&s.project).clips);
        assert_eq!(end(&st.snapshot().unwrap()), (0.0, 0.0), "empty project");
        run(serde_json::json!({ "type": "addTrack", "track": { "id": 0, "name": "A", "kind": "instrument" } }));
        let track = st.snapshot().unwrap().project.tracks[0].id;
        // two bars of clip, the last note ends at beat 5.5
        let notes = [(0.0, 1.0), (4.5, 1.0), (2.0, 0.5)].map(|(s, l)| serde_json::json!({ "pitch": 60, "startBeats": s, "lengthBeats": l, "velocity": 100 }));
        let s = run(serde_json::json!({ "type": "addClip", "trackId": track, "clip": { "id": 0, "name": "N", "startBeats": 0.0, "source": { "type": "notes", "lengthBeats": 8.0, "notes": notes } } }));
        assert_eq!(end(&s), (5.5, 8.0), "the last note's end, not the clip's");
        // a note past the clip's end only sounds up to it
        let late = [serde_json::json!({ "pitch": 62, "startBeats": 3.0, "lengthBeats": 4.0, "velocity": 100 })];
        let s = run(serde_json::json!({ "type": "addClip", "trackId": track, "clip": { "id": 0, "name": "M", "startBeats": 2.0, "source": { "type": "notes", "lengthBeats": 4.0, "notes": late } } }));
        assert_eq!(end(&s), (6.0, 8.0));
        // an empty note clip counts with its whole length
        let s = run(serde_json::json!({ "type": "addClip", "trackId": track, "clip": { "id": 0, "name": "E", "startBeats": 8.0, "source": { "type": "notes", "lengthBeats": 4.0, "notes": [] } } }));
        assert_eq!(end(&s), (12.0, 12.0));
    }

    #[test]
    fn metronome_silent_while_recording_unless_switched_on_there() {
        let st = AppState::without_audio();
        st.set_metronome(true).unwrap();
        st.mute_metronome_for_recording();
        assert!(!st.snapshot().unwrap().engine.metronome, "silent once the recording starts");
        st.unmute_metronome_after_recording();
        assert!(st.snapshot().unwrap().engine.metronome, "the choice applies again afterwards");
        st.set_metronome(false).unwrap();
        st.mute_metronome_for_recording();
        assert!(st.set_metronome(true).unwrap().engine.metronome, "switched on during the recording: audible");
        st.unmute_metronome_after_recording();
        assert!(st.snapshot().unwrap().engine.metronome, "and still on afterwards");
    }

    #[test]
    fn an_overloaded_mixer_gets_clips_pre_rendered_until_it_is_relieved() {
        let st = AppState::without_audio();
        let plan = crate::song::templates().into_iter().next().unwrap();
        st.song_write(&plan, true, "").unwrap();
        let sr = 48_000;
        let note_clips = |st: &AppState| {
            let p = lock(&st.session).unwrap().project.clone();
            let arr = build_arrangement(&p, &mut lock(&st.pool).unwrap(), sr);
            (arr.notes.len(), arr.clips.len())
        };
        let (live_before, rendered_before) = note_clips(&st);
        assert_eq!(rendered_before, 0, "everything live at first");
        // calm: nothing happens
        assert!(!st.prerender_step(0, true, 0.0, 0.3, sr));
        assert!(lock(&st.prerender).unwrap().job.is_none());
        // overloaded, but only briefly: still nothing
        assert!(!st.prerender_step(1000, true, 0.0, 0.95, sr));
        assert!(!st.prerender_step(1100, true, 0.0, 0.95, sr));
        assert!(lock(&st.prerender).unwrap().job.is_none());
        // overloaded for a while: the most expensive clip near the playback position is rendered step by step
        let mut now = 1400;
        let mut finished = false;
        for _ in 0..10_000 {
            if st.prerender_step(now, true, 0.0, 0.95, sr) {
                finished = true;
                break;
            }
            now += 33;
        }
        assert!(finished, "a clip got rendered");
        assert_eq!(lock(&st.pool).unwrap().prerendered.len(), 1);
        {
            let pool = lock(&st.pool).unwrap();
            let (mid, _) = pool.prerendered.values().next().unwrap();
            let rms = (mid.iter().map(|v| v * v).sum::<f32>() / mid.len() as f32).sqrt();
            assert!(rms > 0.001, "the rendered clip sounds: rms {rms} over {} samples", mid.len());
        }
        let (live_after, rendered_after) = note_clips(&st);
        assert_eq!((live_after, rendered_after), (live_before - 1, 1), "it plays from its buffer now");
        // relieved: no further clip
        for k in 0..20 {
            assert!(!st.prerender_step(now + 33 * k, true, 0.0, 0.4, sr));
        }
        assert!(lock(&st.prerender).unwrap().job.is_none());
        // a new tempo plays it live again (its buffer belongs to the old tempo)
        st.execute(Command::SetProject { patch: klang_core::ProjectPatch { tempo_bpm: Some(plan.tempo + 7.0), ..Default::default() } }, false).unwrap();
        assert_eq!(note_clips(&st), (live_before, 0));
        assert!(lock(&st.pool).unwrap().prerendered.is_empty(), "the old buffer is freed");
    }
}

impl AppState {
    pub fn new() -> Self {
        let (engine, engine_error) = match Engine::start() {
            Ok(e) => (Some(e), None),
            Err(e) => {
                eprintln!("Audio-Engine nicht verfügbar: {e}");
                (None, Some(e.to_string()))
            }
        };
        Self::with_engine(engine, engine_error)
    }

    /// Without an audio device (tests): everything except playback and recording works.
    pub fn without_audio() -> Self {
        Self::with_engine(None, Some("Ohne Audio gestartet".into()))
    }

    fn with_engine(engine: Option<Engine>, engine_error: Option<String>) -> Self {
        // the history notes when each step happened
        klang_core::history::set_clock(crate::clock::now_ms);
        let state = Self {
            chat: crate::chat::Chat::default(),
            session: Mutex::new(Session::with_label(Project::default(), klang_i18n::tk!("New project").into())),
            pool: Mutex::new(AssetPool::default()),
            library: Mutex::new(None),
            library_status: Mutex::new(Default::default()),
            library_cancel: AtomicBool::new(false),
            download_child: Mutex::new(None),
            agent: Mutex::new(AgentLink::default()),
            engine,
            engine_error,
            metronome: AtomicBool::new(false),
            metronome_muted: AtomicBool::new(false),
            looping: Mutex::new(LoopState::default()),
            record_counter: AtomicU32::new(0),
            live: Mutex::new(None),
            live_voices: Mutex::new(None),
            live_take: AtomicU32::new(0),
            project_file: Mutex::new(None),
            saved_hash: Mutex::new(project_hash(&Project::default())),
            force_close: AtomicBool::new(false),
            song_tracks: Mutex::new(Vec::new()),
            settings_file: Mutex::new(None),
            audio_notice: Mutex::new(AudioNotice::default()),
            host: std::sync::OnceLock::new(),
            prerender: Mutex::new(Prerender::default()),
            test_load: AtomicU32::new(0),
            agent_watch: Mutex::new((0, None)),
        };
        if let Some(e) = &state.engine {
            e.set_metronome(false);
            // KLANGWELT_STUMM=1: nothing audible (automated tests), everything else runs as usual
            if std::env::var_os("KLANGWELT_STUMM").is_some_and(|v| !v.is_empty() && v != "0") {
                e.set_muted(true);
            }
            // KLANGWELT_FAKE_MIC=<audio file>: recordings take this file (in a loop) instead of a
            // microphone – automated tests must not listen to a real one
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(path) = std::env::var_os("KLANGWELT_FAKE_MIC").filter(|v| !v.is_empty()) {
                let path = std::path::PathBuf::from(path);
                match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| crate::audiofile::decode(&b)) {
                    Ok(d) => e.set_fake_input(Some(klang_engine::FakeInput {
                        samples: d.samples.into(),
                        sample_rate: d.sample_rate,
                        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    })),
                    Err(err) => eprintln!("KLANGWELT_FAKE_MIC {}: {err}", path.display()),
                }
            }
        }
        state
    }

    /// Set the platform (once, at start).
    pub fn set_host(&self, host: Box<dyn crate::Host>) {
        let _ = self.host.set(host);
    }

    /// The platform – before start a placeholder that reports nothing.
    pub fn host(&self) -> &dyn crate::Host {
        match self.host.get() {
            Some(h) => h.as_ref(),
            None => &crate::host::NoHost,
        }
    }

    pub fn engine(&self) -> CmdResult<&Engine> {
        self.engine.as_ref().ok_or_else(|| self.engine_error_text().unwrap_or_else(|| tr!("Audio engine not available")))
    }

    /// Why the engine is missing. Made at start, before the UI has set its
    /// language – fixed messages of the engine are translated here.
    fn engine_error_text(&self) -> Option<String> {
        self.engine_error.as_deref().map(klang_i18n::translate)
    }

    pub fn snapshot(&self) -> CmdResult<Snapshot> {
        let agent = {
            let a = lock(&self.agent)?;
            let lib = lock(&self.library)?;
            AgentInfoDto {
                url: a.url.clone(),
                error: a.error.clone(),
                library: lib.as_ref().map(|l| l.name.clone()),
                library_voices: lib.as_ref().map(|l| l.index.doc_count()).unwrap_or(0),
            }
        };
        let file = lock(&self.project_file)?.as_ref().map(|p| p.display().to_string());
        let saved = *lock(&self.saved_hash)?;
        let looping = *lock(&self.looping)?;
        let info = self.engine.as_ref().map(|e| e.info());
        let notice = lock(&self.audio_notice)?.clone();
        let s = lock(&self.session)?;
        Ok(Snapshot {
            file,
            dirty: project_hash(&s.project) != saved,
            project: s.project.clone(),
            can_undo: s.undo_label().is_some(),
            can_redo: s.redo_label().is_some(),
            undo_label: s.undo_label().map(str::to_string),
            redo_label: s.redo_label().map(str::to_string),
            engine: EngineInfoDto {
                available: self.engine.is_some(),
                error: self.engine_error_text(),
                output_device: info.as_ref().map(|i| i.output_device.clone()),
                output_choice: info.as_ref().and_then(|i| i.output_choice.clone()),
                input_choice: info.as_ref().and_then(|i| i.input_choice.clone()),
                output_notice: notice.output_text(),
                input_notice: notice.input_text(),
                sample_rate: info.as_ref().map(|i| i.sample_rate).unwrap_or(48_000),
                // what is heard: silenced for a recording shows as off
                metronome: self.metronome.load(Ordering::Relaxed) && !self.metronome_muted.load(Ordering::Relaxed),
                loop_on: looping.on,
                loop_range: looping.range,
                end_beats: SongEnd::of(&s.project).sound,
            },
            agent,
        })
    }

    /// Unsaved changes?
    pub fn is_dirty(&self) -> bool {
        let saved = self.saved_hash.lock().map(|h| *h).unwrap_or(0);
        self.session.lock().map(|s| project_hash(&s.project) != saved).unwrap_or(false)
    }

    /// Saves the project together with the recordings it uses and its
    /// history to `path`.
    pub fn save_project(&self, path: &std::path::Path) -> CmdResult<()> {
        {
            let mut s = lock(&self.session)?;
            // A still unnamed project is afterwards named like its file (without an undo step).
            if s.project.name == Project::default().name {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    s.project.name = stem.to_string();
                }
            }
        }
        let hash = self.write_project(path)?;
        *lock(&self.saved_hash)? = hash;
        *lock(&self.project_file)? = Some(path.to_path_buf());
        Ok(())
    }

    /// Writes the open project with its recordings and history to `path`
    /// without making it the project's file (Save, and the autosave's recovery
    /// copy); returns the checksum of what was written.
    pub fn write_project(&self, path: &std::path::Path) -> CmdResult<u64> {
        let (project, history) = {
            let s = lock(&self.session)?;
            (s.project.clone(), s.history_file().ok())
        };
        let audio: Vec<crate::project::AudioData> = {
            let pool = lock(&self.pool)?;
            // recordings of earlier steps too, so undo still finds them after opening
            let mut ids = crate::project::used_assets(&project);
            if let Some(h) = &history {
                ids.extend(crate::project::history_assets(h));
                ids.sort();
                ids.dedup();
            }
            ids.into_iter()
                .filter_map(|id| {
                    pool.get(&id).map(|a| crate::project::AudioData { name: a.name.clone(), samples: a.samples.to_vec(), sample_rate: a.sample_rate, id })
                })
                .collect()
        };
        crate::project::save_with_history(path, &project, &audio, history.as_ref()).map_err(|e| tr!("Saving to {path} failed: {error}", path = path.display(), error = e))?;
        Ok(project_hash(&project))
    }

    /// Opens a project (replaces the open one) with the history saved in it –
    /// without one (older file) the history starts anew.
    pub fn open_project(&self, path: &std::path::Path) -> CmdResult<()> {
        let loaded = crate::project::load_with_history(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let project = loaded.project;
        lock(&self.pool)?.replace_all(loaded.audio);
        *lock(&self.saved_hash)? = project_hash(&project);
        let session = match loaded.history.map(|h| Session::restore(project.clone(), h)) {
            Some(Ok(s)) => s,
            other => {
                if let Some(Err(e)) = other {
                    eprintln!("{}: {e}", path.display());
                }
                Session::with_label(project, klang_i18n::tk!("Project opened").into())
            }
        };
        *lock(&self.session)? = session;
        *lock(&self.project_file)? = Some(path.to_path_buf());
        lock(&self.song_tracks)?.clear();
        self.sync_engine()
    }

    /// Empty project.
    pub fn new_project(&self) -> CmdResult<()> {
        lock(&self.pool)?.replace_all(Vec::new());
        *lock(&self.session)? = Session::with_label(Project::default(), klang_i18n::tk!("New project").into());
        *lock(&self.saved_hash)? = project_hash(&Project::default());
        *lock(&self.project_file)? = None;
        lock(&self.song_tracks)?.clear();
        self.sync_engine()
    }

    pub fn execute(&self, cmd: Command, merge: bool) -> CmdResult<Snapshot> {
        lock(&self.session)?.execute(cmd, merge).map_err(|e| e.to_string())?;
        self.sync_engine()?;
        self.snapshot()
    }

    /// Like [`AppState::execute`], but only if the project is still at revision
    /// `if_revision` (nobody changed it in the meantime) – checked under the same lock.
    pub fn execute_if(&self, cmd: Command, if_revision: Option<u64>) -> CmdResult<Snapshot> {
        {
            let mut s = lock(&self.session)?;
            if let Some(want) = if_revision {
                if s.revision() != want {
                    // for the agent (not translated, like the other agent texts)
                    return Err(format!("The project was changed in the meantime (revision {} instead of {want}) – read it again with get_project and retry.", s.revision()));
                }
            }
            s.execute(cmd, false).map_err(|e| e.to_string())?;
        }
        self.sync_engine()?;
        self.snapshot()
    }

    /// Revision of the project (changes with every change, undo and redo).
    pub fn revision(&self) -> CmdResult<u64> {
        Ok(lock(&self.session)?.revision())
    }

    /// Mark in the undo history for [`AppState::squash_steps`] / [`AppState::rollback_steps`].
    pub fn undo_mark(&self) -> CmdResult<klang_core::UndoMark> {
        Ok(lock(&self.session)?.mark())
    }

    /// All steps since `mark` become one undo step; false if there were none.
    pub fn squash_steps(&self, mark: &klang_core::UndoMark, label: String) -> CmdResult<bool> {
        let mut s = lock(&self.session)?;
        if s.steps_since(mark) == 0 {
            return Ok(false);
        }
        s.squash(mark, label);
        Ok(true)
    }

    /// Reverts all steps since `mark` – the history (undo and redo) is as it was then.
    pub fn rollback_steps(&self, mark: klang_core::UndoMark) -> CmdResult<Snapshot> {
        lock(&self.session)?.rollback(mark).map_err(|e| e.to_string())?;
        self.sync_engine()?;
        self.snapshot()
    }

    pub fn undo(&self) -> CmdResult<Snapshot> {
        lock(&self.session)?.undo().map_err(|e| e.to_string())?;
        self.sync_engine()?;
        self.snapshot()
    }

    pub fn redo(&self) -> CmdResult<Snapshot> {
        lock(&self.session)?.redo().map_err(|e| e.to_string())?;
        self.sync_engine()?;
        self.snapshot()
    }

    /// Situation in the project in words – goes to the agent with every chat message:
    /// audible, muted and solo tracks (with id) and the selected time range.
    pub fn chat_context(&self) -> CmdResult<String> {
        let looping = *lock(&self.looping)?;
        let s = lock(&self.session)?;
        let p = &s.project;
        let list = |ts: Vec<&Track>| ts.iter().map(|t| format!("„{}“ ({})", t.name, t.id)).collect::<Vec<_>>().join(", ");
        let audible: Vec<&Track> = p.audible_tracks().collect();
        let muted: Vec<&Track> = p.tracks.iter().filter(|t| p.is_muted(t)).collect();
        let solo: Vec<&Track> = p.tracks.iter().filter(|t| p.is_solo(t)).collect();
        let mut parts = Vec::new();
        if p.tracks.is_empty() {
            parts.push(tr!("Tracks: none"));
        } else {
            parts.push(if audible.is_empty() { tr!("Audible tracks: none") } else { tr!("Audible tracks: {list}", list = list(audible)) });
            if !solo.is_empty() {
                parts.push(tr!("Solo: {list}", list = list(solo)));
            }
            if !muted.is_empty() {
                parts.push(tr!("Muted: {list}", list = list(muted)));
            }
        }
        let bpb = p.beats_per_bar.max(1) as f64;
        parts.push(match looping.range {
            Some((a, b)) => {
                let from = (a / bpb).floor() as i64 + 1;
                let to = ((b / bpb) - 1e-9).ceil().max(1.0) as i64;
                let (start, end) = ((a * 100.0).round() / 100.0, (b * 100.0).round() / 100.0);
                if looping.on {
                    tr!("Selected range: bars {from}–{to} (beats {start} to {end}), loop on", from = from, to = to, start = start, end = end)
                } else {
                    tr!("Selected range: bars {from}–{to} (beats {start} to {end})", from = from, to = to, start = start, end = end)
                }
            }
            None => tr!("No time range selected"),
        });
        Ok(parts.join(" · "))
    }

    pub fn set_metronome(&self, on: bool) -> CmdResult<Snapshot> {
        self.metronome.store(on, Ordering::Relaxed);
        // switched during a recording, too: from now on it's heard as chosen
        self.metronome_muted.store(false, Ordering::Relaxed);
        if let Some(e) = &self.engine {
            e.set_metronome(on);
        }
        // remember for the next start (even if the agent toggles it)
        if let Some(f) = lock(&self.settings_file)?.as_ref() {
            if let Err(e) = crate::settings::set(f, "metronom", serde_json::Value::Bool(on)) {
                eprintln!("Einstellungen: {e}");
            }
        }
        self.snapshot()
    }

    /// Chat for the UI, including "agent connected".
    pub fn chat_view(&self) -> crate::chat::ChatView {
        let mut v = self.chat.view();
        v.agent_connected = v.agent_listening || v.agent_working || self.agent_session_active();
        v
    }

    /// Reports changes of the agent indicator to the UI ("chat-changed") – also
    /// those that come without an event: a lease or the minute after the last
    /// tool call running out, an MCP client going away. Called by `tick`,
    /// checks twice a second.
    fn watch_agent(&self) {
        let now = crate::clock::now_ms();
        let Ok(mut w) = self.agent_watch.lock() else { return };
        if now < w.0 {
            return;
        }
        w.0 = now + 500;
        let (listening, working, on) = self.chat.agent_key();
        let key = (listening, working, on, listening || working || self.agent_session_active());
        if w.1.as_ref() != Some(&key) {
            let first = w.1.is_none();
            w.1 = Some(key);
            drop(w);
            if !first {
                self.host().emit("chat-changed", &self.chat_view());
            }
        }
    }

    /// Is an agent connected: is it listening, or does it keep a connection to
    /// the MCP server open (see [`AGENT_GRACE`] and `klang_mcp::Presence`)?
    pub fn agent_connected(&self) -> bool {
        let (listening, working, _) = self.chat.agent_key();
        listening || working || self.agent_session_active()
    }

    fn agent_session_active(&self) -> bool {
        lock(&self.agent).ok().and_then(|a| a.server.as_ref().map(|s| s.presence().connected(AGENT_GRACE) > 0)).unwrap_or(false)
    }

    /// Output and recording devices with the current choice.
    pub fn audio_devices(&self) -> CmdResult<AudioDevicesDto> {
        let engine = self.engine()?;
        let d = engine.devices().map_err(|e| e.to_string())?;
        let info = engine.info();
        Ok(AudioDevicesDto {
            outputs: d.outputs,
            inputs: d.inputs,
            default_output: d.default_output,
            default_input: d.default_input,
            output: info.output_choice,
            input: info.input_choice,
            output_device: info.output_device,
            sample_rate: info.sample_rate,
        })
    }

    /// Choose an audio device – `kind` "output"/"ausgabe" or "input"/"eingabe",
    /// `name` None (or empty/"Systemstandard") = system default. Takes effect
    /// immediately and also applies at the next start.
    pub fn set_audio_device(&self, kind: &str, name: Option<String>) -> CmdResult<Snapshot> {
        // "system default" as the agent reads it: German, English or in the chosen language
        let default = tr!("System default");
        let name = name.map(|n| n.trim().to_string()).filter(|n| {
            !n.is_empty() && !n.eq_ignore_ascii_case("Systemstandard") && !n.eq_ignore_ascii_case("System default") && !n.eq_ignore_ascii_case(&default)
        });
        let engine = self.engine()?;
        let output = match kind {
            "output" | "ausgabe" => true,
            "input" | "eingabe" => false,
            other => return Err(tr!("Unknown device type \"{kind}\" (allowed: output, input)", kind = other)),
        };
        if output {
            engine.set_output_device(name.clone()).map_err(|e| e.to_string())?;
            // different sample rate: render everything for the new device
            self.sync_engine()?;
            lock(&self.audio_notice)?.output = None;
        } else {
            engine.set_input_device(name.clone()).map_err(|e| e.to_string())?;
            lock(&self.audio_notice)?.input = None;
        }
        if let Some(f) = lock(&self.settings_file)?.as_ref() {
            let key = if output { SETTING_OUTPUT } else { SETTING_INPUT };
            if let Err(e) = crate::settings::set(f, key, name.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)) {
                eprintln!("Einstellungen: {e}");
            }
        }
        self.snapshot()
    }

    /// At start: take the remembered audio devices. If one is missing, the
    /// system default stays and a notice appears (the choice stays remembered –
    /// if the device is plugged in again at the next start, it applies again).
    pub fn apply_audio_settings(&self, settings: &serde_json::Map<String, serde_json::Value>) {
        let Some(engine) = &self.engine else { return };
        let want = |k: &str| settings.get(k).and_then(serde_json::Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let (out, inp) = (want(SETTING_OUTPUT), want(SETTING_INPUT));
        if out.is_none() && inp.is_none() {
            return;
        }
        let devices = engine.devices().unwrap_or_default();
        let mut notice = AudioNotice::default();
        if let Some(n) = out {
            let res = if devices.outputs.contains(&n) { engine.set_output_device(Some(n.clone())).map(|_| ()).map_err(|e| e.to_string()) } else { Err(String::new()) };
            match res {
                Ok(()) => {
                    let _ = self.sync_engine();
                }
                Err(e) if e.is_empty() => notice.output = Some(DeviceProblem::Missing(n)),
                Err(e) => notice.output = Some(DeviceProblem::Failed(n, e)),
            }
        }
        if let Some(n) = inp {
            let res = if devices.inputs.contains(&n) { engine.set_input_device(Some(n.clone())).map(|_| ()).map_err(|e| e.to_string()) } else { Err(String::new()) };
            match res {
                Ok(()) => {}
                Err(e) if e.is_empty() => notice.input = Some(DeviceProblem::Missing(n)),
                Err(e) => notice.input = Some(DeviceProblem::Failed(n, e)),
            }
        }
        for n in [notice.output_text(), notice.input_text()].into_iter().flatten() {
            eprintln!("{n}");
        }
        if let Ok(mut g) = self.audio_notice.lock() {
            *g = notice;
        }
    }

    /// Repeat on/off and/or set the time range (`range`: Some(None) clears it).
    pub fn set_loop(&self, on: Option<bool>, range: Option<Option<(f64, f64)>>) -> CmdResult<Snapshot> {
        {
            let mut l = lock(&self.looping)?;
            if let Some(on) = on {
                l.on = on;
            }
            if let Some(r) = range {
                l.range = r.map(|(a, b)| (a.min(b).max(0.0), a.max(b))).filter(|(a, b)| b - a > 1e-3);
            }
        }
        self.apply_loop()?;
        self.snapshot()
    }

    fn song_end(&self) -> CmdResult<SongEnd> {
        Ok(SongEnd::of(&lock(&self.session)?.project))
    }

    /// Loop and stop point to the engine. With repeat: the selected range,
    /// otherwise the whole piece (rounded up to whole bars). Without: stop
    /// at the end of the selected range (back to its start), otherwise
    /// right at the end of the piece – the playhead stays there, so typing
    /// can go on at that spot.
    fn apply_loop(&self) -> CmdResult<()> {
        let Some(engine) = &self.engine else { return Ok(()) };
        let l = *lock(&self.looping)?;
        let end = self.song_end()?;
        let (region, halt) = if l.on {
            (l.range.or_else(|| (end.clips > 0.0).then(|| (0.0, (end.clips / end.bar - 1e-9).ceil() * end.bar))), None)
        } else {
            (None, match l.range {
                Some((a, b)) => Some((b, a)),
                None => (end.sound > 0.0).then_some((end.sound, end.sound)),
            })
        };
        engine.set_loop(region);
        engine.set_stop_at(halt);
        Ok(())
    }

    /// Before the start: if the playhead is outside the selected range,
    /// it begins at the range's start. (At or past the stop point the engine
    /// itself starts over – at the end of the piece from the beginning.)
    fn place_for_play(&self, engine: &Engine) -> CmdResult<()> {
        let l = *lock(&self.looping)?;
        let spb = engine.info().sample_rate as f64 * 60.0 / self.tempo();
        let pos = engine.status().position_frames as f64 / spb;
        let to = match l.range {
            Some((a, b)) if pos < a - 1e-6 || pos >= b - 1e-6 => Some(a),
            _ => None,
        };
        if let Some(beat) = to {
            engine.seek_frames((beat * spb).round().max(0.0) as u64);
        }
        Ok(())
    }

    pub fn transport(&self, action: &str) -> CmdResult<()> {
        let engine = self.engine()?;
        match action {
            "play" => {
                if !engine.status().playing {
                    self.place_for_play(engine)?;
                }
                engine.play()
            }
            "stop" => engine.stop(),
            "toggle" => {
                if engine.status().playing {
                    engine.stop()
                } else {
                    self.place_for_play(engine)?;
                    engine.play()
                }
            }
            "rewind" => engine.seek_frames(0),
            other => return Err(tr!("Unknown transport action: {action} (allowed: play, stop, toggle, rewind)", action = other)),
        }
        Ok(())
    }

    pub fn seek_seconds(&self, seconds: f64) -> CmdResult<()> {
        let engine = self.engine()?;
        engine.seek_frames((seconds.max(0.0) * engine.info().sample_rate as f64) as u64);
        Ok(())
    }

    /// What the export needs: project, selected range, its own sound store.
    pub fn export_source(&self) -> CmdResult<ExportSource> {
        let project = lock(&self.session)?.project.clone();
        let range = lock(&self.looping)?.range;
        let pool = lock(&self.pool)?.fork();
        Ok(ExportSource { project, range, pool })
    }

    pub fn tempo(&self) -> f64 {
        lock(&self.session).map(|s| s.project.tempo_bpm).unwrap_or(120.0)
    }

    /// Renders the project into an arrangement and hands it to the engine.
    pub fn sync_engine(&self) -> CmdResult<()> {
        let Some(engine) = &self.engine else { return Ok(()) };
        let project = lock(&self.session)?.project.clone();
        let arrangement = {
            let mut pool = lock(&self.pool)?;
            build_arrangement(&project, &mut pool, engine.info().sample_rate)
        };
        // one message: tempo and newly rendered arrangement switch in the same block
        engine.set_arrangement_with_tempo(arrangement, project.tempo_bpm, project.beats_per_bar);
        // end of the piece may have changed
        self.apply_loop()
    }

/// Regular work on the side, called by the shells every few dozen
    /// milliseconds (desktop: status thread, browser: worker timer): when the
    /// live mixer can't keep up, note clips around the playback position are
    /// rendered step by step and then play from their buffer.
    pub fn tick(&self) {
        self.watch_agent();
        let Some(engine) = &self.engine else { return };
        self.feed_live(engine);
        // without taking the peak levels: they belong to the UI's status
        let s = engine.peek_status();
        let load = s.load.max(f32::from_bits(self.test_load.load(Ordering::Relaxed)));
        let pos = s.position_frames as f64 / s.sample_rate.max(1) as f64;
        if self.prerender_step(crate::clock::now_ms(), s.playing, pos, load, engine.info().sample_rate) {
            if let Err(e) = self.sync_engine() {
                eprintln!("pre-rendering: {e}");
            }
        }
    }

    /// The running recording's new samples into the live analysis.
    fn feed_live(&self, engine: &Engine) {
        let Ok(mut live) = lock(&self.live) else { return };
        if let Some(l) = live.as_mut() {
            let samples = engine.take_input_tap();
            l.push(&samples);
            if let Ok(mut voices) = lock(&self.live_voices) {
                if let Some(v) = voices.as_mut() {
                    v.push(&samples, l.onsets());
                }
            }
        }
    }

    /// The running recording's polyphonic analysis (`None`: no recording, or
    /// it wasn't started in that mode).
    pub fn record_live_voices(&self) -> CmdResult<Option<crate::voices::LiveVoicesView>> {
        Ok(lock(&self.live_voices)?.as_ref().map(|v| v.view()))
    }

    /// The running recording's analysis – what is new since `since` (see
    /// [`LiveAnalysis::view`]); `None` when no recording is running.
    pub fn record_live(&self, since: LiveCursor) -> CmdResult<Option<LiveView>> {
        Ok(lock(&self.live)?.as_ref().map(|l| l.view(since)))
    }

    /// For tests: the live mixer counts as at least `load` loaded (0 = off).
    pub fn simulate_load(&self, load: f32) {
        self.test_load.store(load.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
    }

    /// One step of pre-rendering at time `now` (ms) with the mixer at `load`.
    /// Returns `true` when a clip is finished – the arrangement then has to be
    /// built anew.
    fn prerender_step(&self, now: u64, playing: bool, pos_secs: f64, load: f32, sr: u32) -> bool {
        let Ok(mut p) = lock(&self.prerender) else { return false };
        if playing && load > LOAD_HIGH {
            p.over_since.get_or_insert(now);
        } else if !playing || load < LOAD_CALM {
            p.over_since = None;
        }
        if let Some((_, job)) = p.job.as_mut() {
            let t = crate::clock::Instant::now();
            let mut done = false;
            while !done && t.elapsed().as_secs_f64() * 1000.0 < PRERENDER_SLICE_MS {
                done = job.step(4096);
            }
            if !done {
                return false;
            }
            let Some((key, job)) = p.job.take() else { return false };
            if let Ok(mut pool) = lock(&self.pool) {
                pool.prerendered.insert(key, job.finish());
            }
            // the load needs a moment to show the effect before the next clip
            if p.over_since.is_some() {
                p.over_since = Some(now);
            }
            return true;
        }
        match p.over_since {
            Some(since) if now.saturating_sub(since) >= LOAD_HOLD_MS => {}
            _ => return false,
        }
        if let Some((key, clip, bpm)) = self.prerender_candidate(pos_secs, sr) {
            p.job = Some((key, StepRender::new(clip, bpm, sr)));
        }
        false
    }

    /// The most expensive note clip that sounds in the next seconds and is
    /// not yet rendered: its key (with the tempo), the clip and the tempo.
    fn prerender_candidate(&self, pos_secs: f64, sr: u32) -> Option<(String, NoteClip, f64)> {
        let project = lock(&self.session).ok()?.project.clone();
        let bpm = project.tempo_bpm;
        let spb = 60.0 / bpm;
        let (from, to) = (pos_secs / spb, (pos_secs + PRERENDER_AHEAD_SECS) / spb);
        let mut pool = lock(&self.pool).ok()?;
        let mut best: Option<(f64, String, String, &Track, &Clip)> = None;
        for track in project.audible_tracks() {
            let tail = sound::patch(track.instrument.as_ref()).release_secs() as f64 / spb + 1.0;
            for clip in &track.clips {
                let ClipSource::Notes { length_beats, notes } = &clip.source else { continue };
                if clip.start_beats + length_beats + tail < from || clip.start_beats > to || notes.is_empty() {
                    continue;
                }
                let key = notes_key(track, notes, *length_beats, sr);
                let pre = prerendered_key(&key, bpm);
                if pool.prerendered.contains_key(&pre) {
                    continue;
                }
                let cost = clip_cost(track, notes, *length_beats);
                if best.as_ref().is_none_or(|b| cost > b.0) {
                    best = Some((cost, key, pre, track, clip));
                }
            }
        }
        let (_, key, pre, track, clip) = best?;
        let ClipSource::Notes { length_beats, notes } = &clip.source else { return None };
        let prepared = pool.live.entry(key).or_insert_with(|| Arc::new(live_notes(track, notes, *length_beats, sr))).clone();
        Some((pre, NoteClip { track: track.id.0, start_beats: clip.start_beats, notes: prepared, gain_l: 1.0, gain_r: 1.0, send: 0.0 }, bpm))
    }

    pub fn status(&self) -> StatusDto {
        match &self.engine {
            Some(e) => {
                let s = e.status();
                StatusDto {
                    playing: s.playing,
                    previewing: s.previewing,
                    recording: s.recording,
                    position_secs: s.position_frames as f64 / s.sample_rate.max(1) as f64,
                    peak_out: s.peak_out,
                    peak_in: s.peak_in,
                    load: s.load,
                    prerendered: lock(&self.pool).map(|p| p.prerendered.len()).unwrap_or(0),
                    live_analysis: lock(&self.live).map(|l| l.is_some()).unwrap_or(false),
                }
            }
            None => StatusDto { playing: false, previewing: false, recording: false, position_secs: 0.0, peak_out: 0.0, peak_in: 0.0, load: 0.0, prerendered: 0, live_analysis: false },
        }
    }

    /// Analysis of an asset (cached).
    pub fn analysis_for(&self, asset_id: &str) -> CmdResult<Analysis> {
        let (samples, sr) = {
            let pool = lock(&self.pool)?;
            if let Some(a) = pool.analyses.get(asset_id) {
                return Ok(a.clone());
            }
            let asset = pool.get(asset_id).ok_or_else(|| tr!("Audio file not found"))?;
            (asset.samples.clone(), asset.sample_rate)
        };
        let a = analysis::analyze(asset_id, &samples, sr);
        lock(&self.pool)?.analyses.insert(asset_id.to_string(), a.clone());
        Ok(a)
    }

    pub fn store_analysis(&self, a: Analysis) -> CmdResult<()> {
        lock(&self.pool)?.analyses.insert(a.asset_id.clone(), a);
        Ok(())
    }

    pub fn clip(&self, track_id: Id, clip_id: Id) -> CmdResult<Clip> {
        lock(&self.session)?
            .project
            .clip(track_id, clip_id)
            .cloned()
            .ok_or_else(|| tr!("Clip {clip} not found on track {track}", clip = clip_id, track = track_id))
    }

    /// Analysis of an audio clip.
    pub fn clip_analysis(&self, track_id: Id, clip_id: Id) -> CmdResult<(Clip, Analysis)> {
        let clip = self.clip(track_id, clip_id)?;
        let ClipSource::Audio { asset_id, .. } = &clip.source else {
            return Err(tr!("Only audio clips can be analyzed."));
        };
        let a = self.analysis_for(asset_id)?;
        Ok((clip, a))
    }

    /// Create the detected melody of an audio clip as an instrument track (sixteenth-note grid,
    /// directly below the audio track) – see `convert::audio_to_notes`.
    pub fn melody_to_track(&self, track_id: Id, clip_id: Id, label: &str, smooth: klang_core::smooth::Strength) -> CmdResult<(Snapshot, Id)> {
        let r = self.audio_to_notes(track_id, Some(clip_id), crate::convert::Target::Melodie, label, smooth)?;
        Ok((r.snapshot, r.track_id))
    }

    pub fn instrument_track(&self, track_id: Id) -> CmdResult<Track> {
        let s = lock(&self.session)?;
        let t = s.project.track(track_id).ok_or_else(|| tr!("Track {id} not found", id = track_id))?;
        if t.kind != TrackKind::Instrument {
            return Err(tr!("\"{name}\" is an audio track – sound design is only available for instrument tracks.", name = t.name));
        }
        Ok(t.clone())
    }

    /// Level of a track in dB as it sounds in the project: its own plus its group's.
    pub fn heard_gain_db(&self, track: &Track) -> f32 {
        lock(&self.session).map(|s| s.project.track_gain_db(track)).unwrap_or(track.gain_db)
    }

    /// Plays a preview pattern with the track's sound (transport keeps running independently).
    pub fn preview_track(&self, track_id: Id, pattern: &str) -> CmdResult<f32> {
        let track = self.instrument_track(track_id)?;
        let engine = self.engine()?;
        let sr = engine.info().sample_rate as f32;
        let inst = track.instrument.as_ref();
        let shift = if sound::transposes(inst) { track.pitch_semitones } else { 0.0 };
        let (notes, secs) = sound::preview_notes(inst, pattern, self.tempo(), shift);
        let r = sound::render_track(inst, &notes, secs, sr, (60.0 / self.tempo()) as f32);
        let secs = r.mid.len() as f32 / sr;
        engine.preview_stereo(r.mid.into(), r.side.map(Into::into), util::db_to_gain(self.heard_gain_db(&track)), sound::reverb_send(inst));
        Ok(secs)
    }

    /// Play a single note (when placing/moving notes in the editor).
    pub fn preview_note(&self, track_id: Id, midi: u8) -> CmdResult<()> {
        let track = self.instrument_track(track_id)?;
        let engine = self.engine()?;
        let sr = engine.info().sample_rate as f32;
        let inst = track.instrument.as_ref();
        let shift = if sound::transposes(inst) { track.pitch_semitones } else { 0.0 };
        let note = NoteSpec::new(0.0, 0.45, midi as f32 + shift, 0.85);
        let r = sound::render_track(inst, &[note], 0.6, sr, (60.0 / self.tempo()) as f32);
        engine.preview_stereo(r.mid.into(), r.side.map(Into::into), util::db_to_gain(self.heard_gain_db(&track)), sound::reverb_send(inst));
        Ok(())
    }

    /// Waveform of the track's test sound (for the sound window).
    pub fn sound_wave(&self, track_id: Id) -> CmdResult<sound::WaveView> {
        let track = self.instrument_track(track_id)?;
        let inst = track.instrument.unwrap_or_else(|| sound::default_instrument(InstrumentKind::Pluck));
        Ok(sound::wave_view(&inst, (60.0 / self.tempo()) as f32, 480))
    }

    /// Sets a point in a field (character, timbre, room) – or, with `preset`,
    /// its starting point. Only the controls of this field change.
    /// `merge`: part of a drag gesture (one undo step).
    pub fn sound_field(&self, track_id: Id, field: &str, xy: Option<(f32, f32)>, preset: Option<&str>, merge: bool) -> CmdResult<Snapshot> {
        self.sound_field_on(track_id, &[], field, xy, preset, merge)
    }

    /// [`sound_field`](Self::sound_field) on several tracks at once (`also`: further picked
    /// tracks): each one with the same instrument gets the same point in the field and keeps
    /// its other settings – one undo step.
    pub fn sound_field_on(&self, track_id: Id, also: &[Id], field: &str, xy: Option<(f32, f32)>, preset: Option<&str>, merge: bool) -> CmdResult<Snapshot> {
        let track = self.instrument_track(track_id)?;
        let mut inst = track.instrument.clone().unwrap_or_else(|| sound::default_instrument(InstrumentKind::Pluck));
        let (x, y) = match (preset, xy) {
            (Some(name), _) => sound::field_preset(inst.kind, field, name)?,
            (None, Some(xy)) => xy,
            (None, None) => return Err(tr!("Point (x, y) or starting point missing")),
        };
        sound::apply_field(&mut inst, field, x, y)?;
        inst.preset = match (field, preset) {
            ("charakter", Some(name)) => name.to_string(),
            ("charakter", None) => "eigen".into(),
            _ => inst.preset,
        };
        let label = match field {
            "charakter" => tr!("Sound \"{name}\": Character", name = track.name),
            "klangfarbe" => tr!("Sound \"{name}\": Timbre", name = track.name),
            _ => tr!("Sound \"{name}\": Space", name = track.name),
        };
        let mut commands = vec![Command::SetTrack { track_id, patch: TrackPatch { instrument: Some(inst.clone()), ..Default::default() } }];
        for &id in also.iter().filter(|id| **id != track_id) {
            let Ok(other) = self.instrument_track(id) else { continue };
            let Some(mut oi) = other.instrument.filter(|i| i.kind == inst.kind) else { continue };
            sound::apply_field(&mut oi, field, x, y)?;
            if field == "charakter" {
                oi.preset = inst.preset.clone();
            }
            commands.push(Command::SetTrack { track_id: id, patch: TrackPatch { instrument: Some(oi), ..Default::default() } });
        }
        self.execute(Command::Batch { label, commands }, merge)
    }

    pub fn describe_track(&self, track_id: Id) -> CmdResult<SoundReport> {
        let track = self.instrument_track(track_id)?;
        let inst = track.instrument.unwrap_or_else(|| sound::default_instrument(InstrumentKind::Pluck));
        Ok(sound::report(&inst))
    }

    /// New instrument track with the default sound and a 4-bar pattern so something sounds right away.
    pub fn create_instrument_track(&self, kind: InstrumentKind, label: &str) -> CmdResult<(Snapshot, Id)> {
        let (cmd, id) = {
            let mut s = lock(&self.session)?;
            let p = &mut s.project;
            let bpb = p.beats_per_bar as f64;
            let count = p.tracks.iter().filter(|t| t.instrument.as_ref().map(|i| i.kind) == Some(kind)).count();
            let name = match count {
                0 => sound::kind_name(kind).to_string(),
                n => format!("{} {}", sound::kind_name(kind), n + 1),
            };
            let id = p.alloc_id();
            let mut track = Track::new(id, name, TrackKind::Instrument);
            track.instrument = Some(sound::default_instrument(kind));
            track.clips.push(Clip {
                id: p.alloc_id(),
                name: tr!("Pattern"),
                start_beats: 0.0,
                source: ClipSource::Notes { length_beats: 4.0 * bpb, notes: sound::pattern_bars(kind, 4, bpb) },
            });
            (Command::Batch { label: label.into(), commands: vec![Command::AddTrack { track, index: None }] }, id)
        };
        Ok((self.execute(cmd, false)?, id))
    }

    /// Inserts one bar of the sample pattern after the track's last clip.
    pub fn insert_pattern(&self, track_id: Id) -> CmdResult<Snapshot> {
        let track = self.instrument_track(track_id)?;
        let inst = track.instrument.as_ref();
        let (bpb, bpm) = {
            let s = lock(&self.session)?;
            (s.project.beats_per_bar as f64, s.project.tempo_bpm)
        };
        let end = track.clips.iter().map(|c| c.start_beats + c.length_beats(bpm)).fold(0.0, f64::max);
        let start = (end / bpb).ceil() * bpb;
        let clip = Clip {
            id: Id::NEW,
            name: tr!("Pattern"),
            start_beats: start,
            source: ClipSource::Notes { length_beats: 4.0, notes: sound::pattern_for(inst) },
        };
        self.execute(Command::AddClip { track_id, clip }, false)
    }

    /// Start recording (playback runs along). `voices`: also analyze it
    /// polyphonically while it runs, with that many voices.
    pub fn record_start(&self, voices: Option<klang_dsp::poly::VoiceCount>) -> CmdResult<()> {
        if crate::library::LISTENING.load(Ordering::SeqCst) {
            return Err(tr!("Sing-to-search for the library is currently running – stop it there first."));
        }
        let engine = self.engine()?;
        // the samples go to the live analysis from the very first one
        engine.set_input_tap(true);
        let sample_rate = match engine.start_recording() {
            Ok(sr) => sr,
            Err(e) => {
                engine.set_input_tap(false);
                return Err(e.to_string());
            }
        };
        let take = self.live_take.fetch_add(1, Ordering::Relaxed) + 1;
        *lock(&self.live)? = Some(LiveAnalysis::new(take, sample_rate));
        *lock(&self.live_voices)? = voices.map(|count| crate::voices::LiveVoices::new(take, sample_rate, count));
        self.mute_metronome_for_recording();
        engine.play();
        Ok(())
    }

    /// A recording starts without a click in the microphone, even if the
    /// metronome is switched on; switching it on now makes it heard.
    fn mute_metronome_for_recording(&self) {
        self.metronome_muted.store(true, Ordering::Relaxed);
        if let Some(e) = &self.engine {
            e.set_metronome(false);
        }
    }

    /// After the recording the person's choice applies again.
    fn unmute_metronome_after_recording(&self) {
        self.metronome_muted.store(false, Ordering::Relaxed);
        if let Some(e) = &self.engine {
            e.set_metronome(self.metronome.load(Ordering::Relaxed));
        }
    }

    /// End recording: as an audio track into the project (one undo step), with analysis.
    pub fn record_stop(&self) -> CmdResult<RecordResult> {
        if crate::library::LISTENING.load(Ordering::SeqCst) {
            return Err(tr!("This is the library's sing-to-search recording – stop it there with \"Stop\"."));
        }
        let engine = self.engine()?;
        // the finished analysis below replaces the live one
        engine.set_input_tap(false);
        *lock(&self.live)? = None;
        *lock(&self.live_voices)? = None;
        let rec = engine.stop_recording().map_err(|e| e.to_string());
        self.unmute_metronome_after_recording();
        let rec = rec?;
        engine.stop();
        let out_sr = engine.info().sample_rate as f64;
        if rec.samples.len() < (rec.sample_rate as usize) / 4 {
            return Err(tr!("The recording was too short."));
        }

        let n = self.record_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let name = tr!("Recording {n}", n = n);
        let duration_secs = rec.samples.len() as f64 / rec.sample_rate as f64;
        let mut samples = rec.samples;
        // Bring quiet recordings to a usable level (only raise, never lower).
        let peak = util::peak(&samples);
        if peak > 1e-4 && peak < 0.5 {
            util::normalize(&mut samples, 0.5);
        }

        let asset_id = lock(&self.pool)?.insert(name.clone(), samples.clone(), rec.sample_rate);
        let analysis = analysis::analyze(&asset_id, &samples, rec.sample_rate);
        self.store_analysis(analysis.clone())?;

        let (cmd, track_id, clip_id) = {
            let mut session = lock(&self.session)?;
            let p = &mut session.project;
            let bpm = p.tempo_bpm;
            let track_id = p.alloc_id();
            let clip_id = p.alloc_id();
            let mut track = Track::new(track_id, name.clone(), TrackKind::Audio);
            track.clips.push(Clip {
                id: clip_id,
                name: name.clone(),
                start_beats: rec.start_frame as f64 / out_sr * bpm / 60.0,
                source: ClipSource::Audio { asset_id, source_bpm: bpm, warp: true, duration_secs },
            });
            (Command::Batch { label: tr!("Recording"), commands: vec![Command::AddTrack { track, index: None }] }, track_id, clip_id)
        };
        let snapshot = self.execute(cmd, false)?;
        Ok(RecordResult { snapshot, analysis, track_id, clip_id })
    }

    /// Overview of a recording for the waveform (`buckets` values).
    pub fn waveform(&self, asset_id: &str, buckets: usize) -> CmdResult<Vec<f32>> {
        let pool = lock(&self.pool)?;
        let asset = pool.get(asset_id).ok_or_else(|| tr!("Audio file not found"))?;
        Ok(util::overview(&asset.samples, buckets.clamp(16, 20_000)))
    }

    /// Insert the demo project (one undo step).
    pub fn create_demo(&self) -> CmdResult<Snapshot> {
        let cmd = {
            let mut s = lock(&self.session)?;
            let mut pool = lock(&self.pool)?;
            crate::demo::demo_command(&mut s.project, &mut pool)
        };
        self.execute(cmd, false)
    }

    /// End the running preview (preview, library excerpt …).
    pub fn preview_stop(&self) -> CmdResult<()> {
        self.engine()?.preview(Vec::<f32>::new().into(), 0.0);
        Ok(())
    }
}

/// Result of a recording.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordResult {
    pub snapshot: Snapshot,
    pub analysis: Analysis,
    pub track_id: Id,
    pub clip_id: Id,
}

/// Key of a prepared note clip (notes, sound, pitch, length, output rate) –
/// independent of the tempo: the mixer plays it live at any tempo.
fn notes_key(track: &Track, notes: &[Note], length_beats: f64, out_sr: u32) -> String {
    let mut h = DefaultHasher::new();
    serde_json::to_string(notes).unwrap_or_default().hash(&mut h);
    serde_json::to_string(&track.instrument.as_ref()).unwrap_or_default().hash(&mut h);
    format!("notes|{:x}|{:.3}|{out_sr}|{length_beats}", h.finish(), track.pitch_semitones)
}

/// The notes of a clip for the live mixer (beats, pitch with the track's transposition).
fn live_notes(track: &Track, notes: &[Note], length_beats: f64, out_sr: u32) -> LiveNotes {
    let inst = track.instrument.as_ref();
    let shift = if sound::transposes(inst) { track.pitch_semitones } else { 0.0 };
    let notes: Vec<LiveNote> = notes
        .iter()
        .map(|n| LiveNote {
            vibrato: n.vibrato,
            brightness: n.brightness,
            bend: n.bend,
            ..LiveNote::new(n.start_beats, n.length_beats, n.pitch as f32 + shift, n.velocity as f32 / 127.0)
        })
        .collect();
    LiveNotes::new(sound::patch(inst), sound::shaping_of(inst), &notes, length_beats, out_sr as f32)
}

/// Key of a note clip rendered at `bpm` (see `AssetPool::prerendered`).
fn prerendered_key(notes_key: &str, bpm: f64) -> String {
    format!("{notes_key}|{bpm:.4}")
}

/// Rough cost of a note clip in the live mixer: average number of sounding
/// notes times the effort of the instrument's voice.
fn clip_cost(track: &Track, notes: &[Note], length_beats: f64) -> f64 {
    let inst = track.instrument.as_ref();
    let weight = match sound::kind_of(inst) {
        // one oscillator per unison voice
        InstrumentKind::Synth => 1.0 + 6.0 * inst.and_then(|i| i.params.get("stimmen").copied()).unwrap_or(0.5) as f64,
        InstrumentKind::Harpsichord => 2.5,
        InstrumentKind::Pluck => 1.5,
        InstrumentKind::Organ | InstrumentKind::Bass => 1.0,
        InstrumentKind::Drums => 0.6,
    };
    let sounding: f64 = notes.iter().filter(|n| n.start_beats < length_beats).map(|n| n.length_beats.min(length_beats - n.start_beats)).sum();
    weight * sounding / length_beats.max(1e-9)
}

/// Key of an audio clip at project tempo, pitch and output rate.
fn audio_key(asset_id: &str, time_ratio: f64, pitch_semitones: f64, out_sr: u32) -> String {
    format!("audio|{asset_id}|{time_ratio:.6}|{pitch_semitones:.3}|{out_sr}")
}

/// Stretch an audio clip (tempo), shift its pitch and bring it to the output rate.
fn render_audio(samples: &[f32], sample_rate: u32, time_ratio: f64, pitch_semitones: f64, out_sr: u32) -> Arc<[f32]> {
    let rate = out_sr as f64 / sample_rate as f64;
    stretch::render_warp(samples, sample_rate as f32, time_ratio, pitch_semitones, rate).into()
}

/// There are no threads in the browser (wasm32-unknown-unknown): the loop in
/// `arrange` stretches the audio clips one after another.
#[cfg(target_arch = "wasm32")]
fn prerender(_project: &Project, _pool: &mut AssetPool, _out_sr: u32) {}

/// Stretch what is not yet rendered (audio clips at the project tempo) on
/// several cores and put it in the cache – e.g. after a tempo change. Note
/// clips are not rendered at all: the mixer plays them live.
#[cfg(not(target_arch = "wasm32"))]
fn prerender(project: &Project, pool: &mut AssetPool, out_sr: u32) {
    struct Job {
        samples: Arc<[f32]>,
        sample_rate: u32,
        time_ratio: f64,
        pitch: f64,
    }
    let bpm = project.tempo_bpm;
    let mut jobs: Vec<(String, Job)> = Vec::new();
    let mut seen = HashSet::new();
    for track in project.audible_tracks() {
        for clip in &track.clips {
            let ClipSource::Audio { asset_id, source_bpm, warp, .. } = &clip.source else { continue };
            let Some(asset) = pool.assets.get(asset_id) else { continue };
            let time_ratio = if *warp { source_bpm / bpm } else { 1.0 };
            let pitch = track.pitch_semitones as f64;
            let key = audio_key(asset_id, time_ratio, pitch, out_sr);
            if !pool.renders.contains_key(&key) && seen.insert(key.clone()) {
                jobs.push((key, Job { samples: asset.samples.clone(), sample_rate: asset.sample_rate, time_ratio, pitch }));
            }
        }
    }
    if jobs.len() < 2 {
        return; // one alone: rendered by the loop in `arrange`
    }
    // long ones first, so the cores are evenly busy
    jobs.sort_by_key(|(_, j)| std::cmp::Reverse(j.samples.len()));
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8).min(jobs.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done: Mutex<Vec<(String, Arc<[f32]>)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((key, job)) = jobs.get(i) else { break };
                let data = render_audio(&job.samples, job.sample_rate, job.time_ratio, job.pitch, out_sr);
                if let Ok(mut d) = done.lock() {
                    d.push((key.clone(), data));
                }
            });
        }
    });
    for (key, data) in done.into_inner().unwrap_or_default() {
        pool.renders.insert(key, data);
    }
}

/// The mixer counts as overloaded from this share of its budget on …
const LOAD_HIGH: f32 = 0.8;
/// … and as relieved below this one.
const LOAD_CALM: f32 = 0.6;
/// This long overloaded (ms) before the next clip is pre-rendered.
const LOAD_HOLD_MS: u64 = 300;
/// Rendering per tick (ms), so commands and status keep flowing.
const PRERENDER_SLICE_MS: f64 = 6.0;
/// Clips sounding within this many seconds from the playback position are candidates.
const PRERENDER_AHEAD_SECS: f64 = 10.0;

/// Pre-rendering when the live mixer can't keep up (see `AppState::tick`).
#[derive(Default)]
struct Prerender {
    /// The clip being rendered: key (with the tempo) and renderer.
    job: Option<(String, StepRender)>,
    /// Since when (ms) the mixer has needed more than `LOAD_HIGH` of its budget.
    over_since: Option<u64>,
}

/// Ends of the piece in beats.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SongEnd {
    /// End of the last clip (a repeat without a range loops up to there, in whole bars).
    clips: f64,
    /// End of what sounds last: a note (at most up to its clip's end), an
    /// audio clip, or an empty note clip in its full length – playback without
    /// a loop stops right there.
    sound: f64,
    /// Length of a bar.
    bar: f64,
}

impl SongEnd {
    fn of(p: &Project) -> Self {
        let mut end = SongEnd { clips: 0.0, sound: 0.0, bar: p.beats_per_bar.max(1) as f64 };
        for c in p.tracks.iter().flat_map(|t| t.clips.iter()) {
            let len = c.length_beats(p.tempo_bpm);
            let sounding = match &c.source {
                ClipSource::Notes { notes, .. } if !notes.is_empty() => {
                    notes.iter().map(|n| n.start_beats + n.length_beats).fold(0.0, f64::max).min(len)
                }
                _ => len,
            };
            end.clips = end.clips.max(c.start_beats + len);
            end.sound = end.sound.max(c.start_beats + sounding);
        }
        end
    }
}

/// For the export (see `AppState::export_source`).
pub struct ExportSource {
    pub project: Project,
    pub range: Option<(f64, f64)>,
    pool: AssetPool,
}

impl ExportSource {
    /// Arrangement of a (partial) project; what is rendered stays for further calls.
    pub fn arrangement(&mut self, project: &Project, out_sr: u32) -> Arrangement {
        arrange(project, &mut self.pool, out_sr).0
    }
}

/// The project as the engine plays it (also the preview of combined variants).
pub(crate) fn build_arrangement(project: &Project, pool: &mut AssetPool, out_sr: u32) -> Arrangement {
    let (arrangement, used) = arrange(project, pool, out_sr);
    pool.renders.retain(|k, _| used.contains(k));
    pool.live.retain(|k, _| used.contains(k));
    pool.prerendered.retain(|k, _| used.contains(k));
    arrangement
}

/// Arrangement plus the keys of the cache used.
fn arrange(project: &Project, pool: &mut AssetPool, out_sr: u32) -> (Arrangement, HashSet<String>) {
    prerender(project, pool, out_sr);
    let bpm = project.tempo_bpm;
    let spb = 60.0 / bpm;
    let mut used = HashSet::new();
    let mut clips = Vec::new();
    let mut notes = Vec::new();

    for track in project.audible_tracks() {
        let gain = util::db_to_gain(project.track_gain_db(track));
        let angle = (track.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gain_l, gain_r) = (
            gain * angle.cos() * std::f32::consts::SQRT_2,
            gain * angle.sin() * std::f32::consts::SQRT_2,
        );

        let send = if track.instrument.is_some() { sound::reverb_send(track.instrument.as_ref()) } else { 0.0 };
        for clip in &track.clips {
            match &clip.source {
                ClipSource::Audio { asset_id, source_bpm, warp, .. } => {
                    let Some(asset) = pool.assets.get(asset_id) else { continue };
                    let time_ratio = if *warp { source_bpm / bpm } else { 1.0 };
                    let pitch = track.pitch_semitones as f64;
                    let key = audio_key(asset_id, time_ratio, pitch, out_sr);
                    used.insert(key.clone());
                    let data = match pool.renders.get(&key) {
                        Some(d) => d.clone(),
                        None => {
                            let rendered = render_audio(&asset.samples, asset.sample_rate, time_ratio, pitch, out_sr);
                            pool.renders.insert(key, rendered.clone());
                            rendered
                        }
                    };
                    clips.push(PlacedClip {
                        start_frame: (clip.start_beats * spb * out_sr as f64).round().max(0.0) as u64,
                        data,
                        side: None,
                        gain_l,
                        gain_r,
                        send: 0.0,
                    });
                }
                ClipSource::Notes { length_beats, notes: list } => {
                    let key = notes_key(track, list, *length_beats, out_sr);
                    used.insert(key.clone());
                    // rendered because the live mixer couldn't keep up: plays like an audio clip
                    let pre = prerendered_key(&key, bpm);
                    if let Some((data, side)) = pool.prerendered.get(&pre) {
                        clips.push(PlacedClip {
                            start_frame: (clip.start_beats * spb * out_sr as f64).round().max(0.0) as u64,
                            data: data.clone(),
                            side: side.clone(),
                            gain_l,
                            gain_r,
                            send,
                        });
                        used.insert(pre);
                        continue;
                    }
                    let prepared = pool.live.entry(key).or_insert_with(|| Arc::new(live_notes(track, list, *length_beats, out_sr))).clone();
                    notes.push(NoteClip { track: track.id.0, start_beats: clip.start_beats, notes: prepared, gain_l, gain_r, send });
                }
            }
        }
    }
    (Arrangement { clips, notes }, used)
}


#[cfg(test)]
mod tempo_tests {
    use super::*;
    use klang_engine::{Mixer, RtMsg, FADE_SECS};

    const SR: u32 = 48_000;
    const BLOCK: usize = 128;

    fn demo_project() -> (Project, AssetPool) {
        let mut project = Project::default();
        let mut pool = AssetPool::default();
        let cmd = crate::demo::demo_command(&mut project, &mut pool);
        let mut s = Session::new(project);
        s.execute(cmd, false).expect("Demo");
        (s.project, pool)
    }

    /// How the mixer switches to the new arrangement.
    #[derive(Clone, Copy, PartialEq)]
    enum Switch {
        /// as before the crossfade: immediately (emulated with stop, switch
        /// and start in the same block – without playback there is no transition)
        Hard,
        /// like `sync_engine`: one message during playback
        Faded,
    }

    fn mixer(arr: &Arc<Arrangement>, bpm: f64, seek: u64) -> Mixer {
        let mut m = Mixer::new(SR as f32, 2);
        m.send(RtMsg::Tempo { bpm, beats_per_bar: 4 });
        m.send(RtMsg::Arrangement { arrangement: arr.clone(), tempo: None });
        m.send(RtMsg::Seek(seek));
        m.send(RtMsg::Play);
        m
    }

    /// Left channel, up to at least `len` frames; before every block `before` may send messages.
    fn run(m: &mut Mixer, len: usize, mut before: impl FnMut(&mut Mixer, usize)) -> Vec<f32> {
        let mut out = vec![0f32; BLOCK * 2];
        let mut left = Vec::with_capacity(len + BLOCK);
        while left.len() < len {
            before(m, left.len());
            m.process(&mut out);
            left.extend(out.chunks(2).map(|f| f[0]));
        }
        left
    }

    /// Plays `a` at `from` BPM, switches at the first block start from `at` to
    /// `b` at `to` BPM and keeps playing. Returns the signal from frame 0, the
    /// switch point and the new position (equal in beats).
    fn play_with_switch(a: &Arc<Arrangement>, b: &Arc<Arrangement>, from: f64, to: f64, at: usize, len: usize, how: Switch) -> (Vec<f32>, usize, u64) {
        let mut switched = None;
        let sig = run(&mut mixer(a, from, 0), len, |m, done| {
            if switched.is_none() && done >= at {
                switched = Some(done);
                let msg = RtMsg::Arrangement { arrangement: b.clone(), tempo: Some((to, 4)) };
                if how == Switch::Hard {
                    m.send(RtMsg::Stop);
                    m.send(msg);
                    m.send(RtMsg::Play);
                } else {
                    m.send(msg);
                }
            }
        });
        let s = switched.unwrap_or(len);
        (sig, s, (s as f64 * from / to).round() as u64)
    }

    /// Largest excess of a sample step in the output (from the switch point
    /// `s`, `span` samples) over what the old signal (`old`, from frame 0) and
    /// the new one (`new`, from `s`) do there anyway. A click shows up as a
    /// large excess.
    fn excess(sig: &[f32], s: usize, old: &[f32], new: &[f32], span: usize) -> f32 {
        (0..span)
            .map(|j| {
                let i = s + j;
                let dx = (sig[i] - sig[i - 1]).abs();
                let da = (old[i] - old[i - 1]).abs();
                let db = if j > 0 { (new[j] - new[j - 1]).abs() } else { 0.0 };
                dx - da.max(db)
            })
            .fold(0.0, f32::max)
    }

    /// Largest excess over `points` tempo changes (hard, crossfaded).
    fn worst_excess(p: &mut Project, pool: &mut AssetPool, from: f64, to: f64, points: usize) -> (f32, f32) {
        p.tempo_bpm = from;
        let a = Arc::new(arrange(p, pool, SR).0);
        p.tempo_bpm = to;
        let b = Arc::new(arrange(p, pool, SR).0);
        let span = (FADE_SECS * SR as f32) as usize + 2;
        let mut worst = (0f32, 0f32);
        let last = SR as usize / 2 + points * SR as usize / 4 + 2 * span;
        let old = run(&mut mixer(&a, from, 0), last, |_, _| {});
        for k in 0..points {
            let at = SR as usize / 2 + k * SR as usize / 4;
            for how in [Switch::Hard, Switch::Faded] {
                let (sig, s, pos) = play_with_switch(&a, &b, from, to, at, at + 2 * span, how);
                let new = run(&mut mixer(&b, to, pos), span, |_, _| {});
                let e = excess(&sig, s, &old, &new, span);
                if how == Switch::Hard {
                    worst.0 = worst.0.max(e);
                } else {
                    worst.1 = worst.1.max(e);
                }
            }
        }
        worst
    }

    /// Tempo change during playback with the demo project (audio clips
    /// are stretched anew, notes rendered anew): crossfaded, every sample
    /// step stays as large as the old or new signal would have it anyway.
    #[test]
    fn tempo_change_in_the_demo_does_not_click() {
        let (mut p, mut pool) = demo_project();
        let (hard, faded) = worst_excess(&mut p, &mut pool, 110.0, 111.0, 8);
        assert!(hard > 0.05, "Testaufbau: hart umgeschaltet knackt es ({hard})");
        assert!(faded < 0.01, "übergeblendet: Überschuss {faded} (hart {hard})");
    }

    /// Measurement (output only): jump in the signal on a tempo change with the
    /// demo project, hard versus crossfaded, 32 switch points each.
    /// `cargo test --release -p klang-app -- --ignored --nocapture tempo_sprung`
    #[test]
    #[ignore = "Messung"]
    fn tempo_sprung_messen() {
        let (mut p, mut pool) = demo_project();
        for (from, to) in [(110.0, 111.0), (110.0, 120.0), (120.0, 90.0)] {
            let t = std::time::Instant::now();
            p.tempo_bpm = to;
            arrange(&p, &mut pool, SR);
            let render_ms = t.elapsed().as_secs_f64() * 1e3;
            let (hard, faded) = worst_excess(&mut p, &mut pool, from, to, 32);
            println!("{from} → {to} BPM: größter Überschuss hart {hard:.4}, übergeblendet {faded:.4} (Neu-Rendern {render_ms:.0} ms)");
        }
    }

    /// Measurement (output only): how many tracks play live at the same time?
    /// The trance song stacked 1, 2, 4 times (5 tracks each), its first
    /// 90 seconds; per block the shortest of three runs.
    /// `cargo test --release -p klang-app -- --ignored --nocapture live_spuren`
    #[test]
    #[ignore = "Messung"]
    fn live_spuren_messen() {
        let plan = crate::song::templates().into_iter().next().expect("template");
        let budget = BLOCK as f64 / SR as f64;
        for stacks in [1usize, 2, 4] {
            let st = AppState::without_audio();
            for _ in 0..stacks {
                st.song_write(&plan, false, "").expect("song");
            }
            let project = lock(&st.session).unwrap().project.clone();
            let arr = Arc::new(build_arrangement(&project, &mut AssetPool::default(), SR));
            let frames = 90 * SR as usize;
            let mut out = vec![0f32; BLOCK * 2];
            let mut blocks = vec![f64::MAX; frames / BLOCK];
            for _ in 0..3 {
                let mut m = mixer(&arr, project.tempo_bpm, 0);
                for b in blocks.iter_mut() {
                    let t = std::time::Instant::now();
                    m.process(&mut out);
                    *b = b.min(t.elapsed().as_secs_f64());
                }
            }
            let avg = blocks.iter().sum::<f64>() / blocks.len() as f64 / budget;
            let mut sorted = blocks.clone();
            sorted.sort_by(f64::total_cmp);
            let q = |p: f64| 100.0 * sorted[((sorted.len() - 1) as f64 * p) as usize] / budget;
            let tracks = project.tracks.len();
            println!(
                "{tracks} tracks: average {:.1} % of the budget (per track {:.2} %), 99 % of the blocks under {:.0} %, 99.9 % under {:.0} %, longest {:.0} %",
                100.0 * avg,
                100.0 * avg / tracks as f64,
                q(0.99),
                q(0.999),
                q(1.0)
            );
        }
    }

    /// Measurement (output only): generated songs played live – how long does
    /// generating take, and how much of the real-time budget does the mixer
    /// need (blocks of 128 frames as in the AudioWorklet)?
    /// `cargo test --release -p klang-app -- --ignored --nocapture live_song`
    #[test]
    #[ignore = "Messung"]
    fn live_song_messen() {
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        for plan in crate::song::templates() {
            let st = AppState::without_audio();
            let t = std::time::Instant::now();
            let written = st.song_write(&plan, true, "").expect("song");
            let write_ms = ms(t);
            let project = lock(&st.session).unwrap().project.clone();
            let mut pool = AssetPool::default();
            let t = std::time::Instant::now();
            let arr = Arc::new(build_arrangement(&project, &mut pool, SR));
            let build_ms = ms(t);
            let frames = (written.seconds * SR as f64) as usize;
            let mut out = vec![0f32; BLOCK * 2];
            // three times through: per block the shortest time – what the
            // mixer needs, without the moments the system took the core away
            let mut blocks: Vec<(f64, usize)> = (0..frames.div_ceil(BLOCK)).map(|k| (f64::MAX, k * BLOCK)).collect();
            for _ in 0..3 {
                let mut m = mixer(&arr, project.tempo_bpm, 0);
                for b in blocks.iter_mut() {
                    let t = std::time::Instant::now();
                    m.process(&mut out);
                    b.0 = b.0.min(t.elapsed().as_secs_f64());
                }
            }
            let total: f64 = blocks.iter().map(|b| b.0).sum();
            blocks.sort_by(|a, b| b.0.total_cmp(&a.0));
            let budget = BLOCK as f64 / SR as f64;
            let worst: Vec<String> = blocks.iter().take(4).map(|(e, at)| format!("{:.0} % at {:.2} s", 100.0 * e / budget, *at as f64 / SR as f64)).collect();
            println!(
                "{}: {} tracks, {} clips, {:.0} s – generating {write_ms:.0} ms (arrangement {build_ms:.1} ms), live mixing {:.1} % of the real-time budget, longest blocks {}",
                plan.name,
                project.tracks.len(),
                arr.notes.len() + arr.clips.len(),
                written.seconds,
                100.0 * total / (frames as f64 / SR as f64),
                worst.join(", ")
            );
        }
    }

    /// Measurement (output only): how long does a tempo step take (as in the
    /// app: render anew, trim the cache to what is used), and where does the
    /// time go? At 44.1 kHz the demo's 48 kHz clips also have to be
    /// converted.
    /// `cargo test --release -p klang-app -- --ignored --nocapture tempo_schritt`
    #[test]
    #[ignore = "Messung"]
    fn tempo_schritt_zeit_messen() {
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        for out_sr in [48_000u32, 44_100] {
            let (mut p, mut pool) = demo_project();
            build_arrangement(&p, &mut pool, out_sr);
            let mut steps = Vec::new();
            for bpm in [111.0, 112.0, 113.0, 114.0] {
                p.tempo_bpm = bpm;
                let t = std::time::Instant::now();
                build_arrangement(&p, &mut pool, out_sr);
                steps.push(format!("{:.0}", ms(t)));
            }
            println!("{out_sr} Hz: Tempo-Schritt {} ms", steps.join(", "));
            // breakdown per audio clip: stretching and sample-rate conversion
            for track in &p.tracks {
                for clip in &track.clips {
                    let ClipSource::Audio { asset_id, source_bpm, .. } = &clip.source else { continue };
                    let a = &pool.assets[asset_id];
                    let t = std::time::Instant::now();
                    let st = stretch::time_stretch(&a.samples, source_bpm / 115.0, a.sample_rate as f32);
                    let t_stretch = ms(t);
                    let t = std::time::Instant::now();
                    let rs = stretch::resample(&st, out_sr as f64 / a.sample_rate as f64);
                    println!(
                        "  {}: {:.1} s, Dehnen {:.0} ms, Umrechnen {:.0} ms ({} Samples)",
                        track.name,
                        a.samples.len() as f64 / a.sample_rate as f64,
                        t_stretch,
                        ms(t),
                        rs.len()
                    );
                }
            }
        }
    }
}
