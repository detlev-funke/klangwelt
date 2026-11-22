//! Tools for the agent (MCP): an agent (e.g. Claude) controls the project
//! through the same commands as the UI. Every change lands in the undo
//! history and shows up in the UI immediately.
//!
//! The tools belong to the shared core, like the UI's command table. Only
//! the route to the agent differs per platform:
//! * Desktop: HTTP server on 127.0.0.1 (`app/src/mcp.rs`, `klang-mcp`).
//! * Browser: `window.__klangweltWeb.mcp(…)` in the page, for an agent that
//!   drives the page through the DevTools protocol or a browser extension
//!   (see docs/WEB.md, `tools/klangwelt-web-mcp.mjs`).
//!
//! What only one platform can do (bring the window to the front, load and
//! manage libraries) it provides through [`AgentShell`].

use crate::harmony::{self, KeyInfo};
use crate::history::{MergeRequest, MergeView};
use crate::sound::{self, note_name};
use crate::rhythm;
use crate::song;
use crate::sound_library;
use crate::state::{lock, AppState};
use klang_core::merge::{Choices, SegmentChoice, Source, ORIGIN};
use klang_core::{ClipSource, Command, History, Id, Instrument, InstrumentKind, NodeId, TrackPatch};
use klang_i18n::{tr, translate, trn};
use klang_mcp::{Handler, Prompt, ServerInfo, Tool};
use klang_music::analysis::analyze_score;
use klang_music::chordstats::GROUPS;
use klang_music::counterpoint::VNote;
use klang_music::fugue::Transform;
use klang_music::harmony as kh;
use klang_music::theory::Key;
use klang_music::index::MIN_QUERY_NOTES;
use klang_music::melody::MelodyNote;
use klang_music::score::load_score;
use serde_json::{json, Value};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Task for the listening subagent (role "listener"; also in
/// `.claude/agents/klangwelt-zuhoerer.md` of the desktop) – MCP prompt `zuhoeren`.
pub const LISTENER_BRIEF: &str = "You are the listener of the Klangwelt app. The person gives tasks in the chat of the interface. \
You only listen and confirm – the work is done by workers that the main session (to: \"main\") starts.\n\
Loop: 1. Call `chat_poll` with client \"zuhoerer\" – it answers as soon as a task arrives, otherwise after timeoutSeconds. Use 280; \
if the call fails with a timeout error or the reply contains `abbruch`, use 50 from then on (the client's \
limit is shorter then; no task is lost). 2. If an empty list comes back, call `chat_poll` again right away – that is normal, \
don't ask, don't summarize. 3. Confirm EVERY task at once with `chat_reply` (replyTo = task id, done=false, a few words in the person's \
language, e.g. \"On it.\") – otherwise it's offered again after the lease. Questions you can answer yourself in one or two \
calls (\"what's going on?\", status: `chat_peek`, `get_project` with fields) you answer right away with done=true. \
Everything else you send to the main session with SendMessage (to: \"main\"): task id, text and kontext. Follow-ups to a \
running task and stop messages (field `cancels`) go there as well – the app already flags stopped tasks with cancelRequested. \
4. Continue with step 1 – don't do the work yourself, so that you stay available.\n\
Only if you can't send messages to a main session, carry out the tasks yourself with the tools and finish each with \
`chat_reply` (done=true, one or two sentences on what you changed).\n\
Don't stop on your own. A plain \"stop\" only stops running tasks. Only when the person explicitly asks you to stop listening, \
say goodbye via `chat_reply` and end the loop. Your final report to the calling agent: which tasks you handled.";

/// Task for a worker subagent (role "worker") – MCP prompt `arbeiten`;
/// the main session adds the task.
pub const WORKER_BRIEF: &str = "You are a worker of the Klangwelt app and carry out ONE task from its chat with the Klangwelt tools.\n\
- Read only what you need: `get_project` with trackIds/fields; remember `revision` and pass it to `execute` as ifRevision – \
if someone changed the project in the meantime, read again and retry.\n\
- Write in few calls: `execute` with placeholders (ref \"$bass\" for new tracks/clips, then trackId \"$bass\"), notes compact \
(\"C4 1 · E4 .5 v100 · G4 .5\"), several tools at once with `batch` (one undo step). Write responses return new IDs and the revision.\n\
- Only touch your track(s). Project-wide changes (tempo, delete_range, write_song) only if the task asks for them.\n\
- Before a bigger change (a whole section, several tracks, write_song) set a `checkpoint` named after the task, so the person can go back \
to it; to offer alternatives, build each in its own variant (`variant_new`) and say which one is active.\n\
- Report progress with `chat_reply` (replyTo = task id, done=false), finish with done=true: one or two sentences in the person's \
language on what you changed.\n\
- If a tool response contains cancelRequested with your task id, stop at once and reply done=true (\"Stopped.\"). Between steps \
`chat_peek` (sinceId) shows follow-ups without taking them from the listener.";

/// Short help on roles for the main session (dispatcher).
const ROLES: &str = "Roles (so that someone always listens while others work): listener = a subagent on a small, fast model \
that only calls `chat_poll` and confirms each task at once (task: prompt `zuhoeren`); dispatcher = you, the main session: \
for each task the listener sends you (SendMessage), start a worker subagent in the background (task: prompt `arbeiten` plus \
the task) and pass follow-ups and stops on to it with SendMessage – react only to messages, so you stay available; \
worker = carries out one task (larger model for composing). At most one worker per track; project-wide changes (tempo, \
delete_range, write_song) one after another. If you can't start subagents, listen and work yourself.";

const INSTRUCTIONS: &str = "Klangwelt is a music app (multitrack, recording, melody detection, sound workshop). \
You control the open project; everything appears live in the interface and can be reverted with `undo`.\n\
Chat: The person gives you tasks in the chat of the interface. To listen, start a subagent \
(Agent/Task tool, preferably in the background; in Claude Code there is the type \"klangwelt-zuhoerer\" for this, if available), \
so that you yourself stay available. The subagent calls `chat_poll` in a loop and confirms each task with `chat_reply` (replyTo = task id). \
If it ends (e.g. because its context is full), start a new one. \
The prompts `zuhoeren` (listener) and `arbeiten` (worker) contain the ready-made tasks (prompts/get).\n\
Chat details: `chat_poll` reserves tasks for its client (\"claimed by …\"); without a `chat_reply` within leaseSeconds (default 60) \
a task is offered again. `chat_peek` (sinceId) reads new messages without picking anything up. If the person writes \"stop\" \
(or \"stop #3\"), every tool response carries `cancelRequested: [ids]` – stop working on those tasks and confirm with chat_reply.\n\
Units: positions and lengths in beats, tempo in BPM, volume in dB (−60…+12), pan −1 (left) … +1 (right), pitch in semitones (−24…+24), MIDI notes 0–127 (60 = middle C).\n\
Workflow: first read `get_project` to learn the track and clip IDs (trackIds/fields read only a part; `revision` protects against \
overwriting: execute with ifRevision fails if someone changed the project in the meantime). Make changes with `execute` \
(several commands = one undo step). Create new tracks/clips with id 0 – the app assigns IDs; or give them a placeholder \
(\"ref\": \"$bass\") and use \"$bass\" as trackId/clipId in later commands – the response lists the IDs. `batch` runs several tools \
in a row as one undo step (stops and reverts at the first error; \"$0.trackId\" uses a result of an earlier call). \
Every writing tool returns the new `revision`, `change` and the IDs it created. Notes can also be written compactly: \
\"C4 1 · E4 .5 v100 · G4 .5\" (pitch, length in beats, v = velocity; + for chords, r = rest; German note names H = B, B = B♭).\n\
Change history (window \"History\"; saved in the project file): every change is a step in a tree. `history` shows the steps of the \
active variant compactly (step IDs, labels, where they changed something, key figures), `goto` jumps to any step or checkpoint – the later \
steps stay and can be redone. Before a bigger change (rewriting a section, write_song, delete_range, reworking several tracks) set a \
`checkpoint` with a telling name, so you and the person can go back to it. To try ideas side by side: `variant_new` (named after the idea) \
starts a branch from the current state or a step – work in it; `variant_switch` goes back and forth, `variant_list` lists them. `combine` \
(base, others) brings variants together into a NEW variant: per track and bar section it takes what only one variant changed; where \
several changed the same bars, sound or tempo differently, it changes nothing and returns `open` conflicts with their options (incl. the \
notes) – decide each with `choices` ({id: variant ID or name, 0 = origin}) and call it again. Tell the person which variant is active \
afterwards. These tools don't run inside batch.\n\
{ROLES}\n\
Audio clips with `warp: true` play at the project tempo; `sourceBpm` is the tempo they were recorded at.\n\
Audio and ranges: `import_audio` inserts audio files (WAV, AIFF, FLAC, Opus; no MP3) as tracks. `audio_to_notes` converts a recording \
into a melody track (detected pitches with their dynamics, accents/sforzati and staccato/legato) or a rhythm track (hits → kick/snare/hi-hat) directly below it. \
`delete_range` deletes a time range from all tracks, everything after it moves up (clips are split or shortened). \
`quantize_notes` snaps notes to the grid (grid in beats: 1 = quarter, 0.5 = eighth, 0.25 = sixteenth). \
`smooth_notes` cleans up notes that came from a recording: unusually short fragments go into a neighbour, repeated notes of the same pitch \
without a new attack become one (`audio_to_notes` and `melody_to_track` already smooth, `smooth` off … strong, default medium). \
For several voices at once (a choir, a piano, guitar with bass) use mode 'poly' in `analyze_clip` and `audio_to_notes`: it finds how many voices there are \
(voices 'auto', or a number) and makes one instrument track per voice.\n\
Editing notes: read `get_project` with includeNotes, then `setClipNotes` with the complete new note list (start/length in beats relative to the clip start). \
`setTrack` with patch {view:'notes'} shows a track as a piano roll, {view:'compact'} flat again.\n\
Track groups (e.g. all violins): `addGroup` {group:{ref:\"$strings\", name:\"Strings\"}}, then `setTrack` {patch:{group:\"$strings\"}} for each track (group 0 = none) and `moveTrack` so they follow each other. \
A group's `gainDb` adds to its tracks' levels, its `mute`/`solo` act on all of them (each track keeps its own); `setGroup` changes it, `collapsed` folds it (no undo step), \
`removeGroup` ungroups – the tracks stay. get_project lists `groups` and each track's `group`.\n\
Expression of individual notes (in `setClipNotes`): `velocity` 1–127 (pp≈32, p≈48, mp≈64, mf≈80, f≈100, ff≈120; crescendo = rising values), \
`vibrato` 0–1 semitone depth (0.3 = vocal), `brightness` −1…+1, `bend` −12…+12 semitones slide-in (−2 = glide in from below). \
Staccato = roughly halve the note length, legato = extend to the next note, accent = velocity +25. \
Pluck and bass use all of them; drums only velocity and brightness. You can omit neutral values (0).\n\
Sound workshop (instrument tracks): instruments pluck, organ (stops subbass, prinzipal, floete, quinte, oktave, mixtur), harpsichord (achtfuss, vierfuss, laute), drums (each key a drum: 36 kick, 37 rim, 38 snare, 39 clap, 42/46 hi-hat closed/open, 41/45/48 toms, 49 crash, 51 ride, 56 cowbell, 70 shaker; every other key plays a tuned drum at its pitch – kick below 48, tom 48–71, snare from 72; with tasten = 1 every key does, for melodies and bass lines on drums), bass synth and synth (pads, long sounds). \
`list_instruments` lists all parameters, `create_instrument_track` creates a track with a pattern, `set_instrument` switches the instrument, \
`describe_sound` measures the sound, `set_sound` changes parameters, `preview_sound` plays it. Every instrument has three fields (`set_sound_field`): \
charakter (character, per instrument, e.g. drums weight crisp ↔ powerful × complexity; sets many controls at once), klangfarbe (timbre: farbe dark ↔ bright × rauheit smooth ↔ rough) \
and raum (space: breite narrow ↔ wide × raum near ↔ far), each with named starting points – a field never changes the controls of the others. Plus waerme and schwebung. \
The synth also has the drawn envelope: `set_sound` with verlauf (template such as 'Anschwellen', 'Glocke', 'Pulsieren') or curves (laut/hell as [second, value]). \
Rhythm (window \"Rhythm\"): `list_drum_patterns` lists drum patterns of typical genres (House, Techno, Hip-Hop, Trap, Drum & Bass, Funk, Reggaeton, Latin …) \
with tempo and tip; `write_drum_pattern` writes one (or your own rows like kick: 'x...x...x...x...') as a clip, `read_drum_pattern` reads a drum clip as a grid. \
For \"make me a house beat\": write_drum_pattern with genre 'House' (without trackId a new drum track with a matching sound is created). \
Song structure (window \"Song\"): `list_song_templates` shows templates of complete pieces (Trance, Ambient, Deep House, Synthwave, Lo-Fi) – sections with bars and kind \
(intro, teil = part, aufbau = build-up, break, hoehepunkt = peak, outro) and per role (pad, arp, bass, lead, kick, hats, clap, perc, fx) a level 0 off / 1 sparse / 2 full. \
`write_song` creates the tracks from it (one undo step; replaces the most recently generated song tracks), also with your own sections, tempo, key, chords. \
For \"build me a trance song that builds up slowly\": write_song with template 'trance' (adjust sections if needed). \
My sounds (sound library, across all projects): `save_sound` saves a track's sound under a name, `list_saved_sounds` shows all of them, \
`load_saved_sound` loads one into a track (for \"use my acid bass\" first list_saved_sounds); also `rename_saved_sound`, `delete_saved_sound`. \
For requests like \"kick deeper and punchier\" or \"warmer bass\", change in small steps and check the measurements after each step; \
choose the kick's fundamental (kick.grundton) to match the key of the bass.\n\
Melody search: `search_melody` finds pieces matching notes or a recording in the active library. A library is \
exactly one index; only the active one is searched (normally \"Standard\"). `list_libraries` shows all, `switch_library` switches, \
`create_library` creates an empty one (e.g. for experiments without changing \"Standard\"), `delete_library` deletes one (only the index). \
`download_library` downloads a package and adds it to the active library (PDMX: 'pdmx-kostprobe' with 3,000 or 'pdmx-auswahl' with 30,000 scores; by era in \
research quality 'kern-barock' (Bach), 'kern-klassik' (Mozart, Beethoven, Haydn), 'kern-romantik', 'kern-20jh', with harmonic analysis 'dcml-…'; runs in the \
background – check with `library_status`). `add_to_library` adds MusicXML/MXL files (or folders) to the active library. `browse_library` organizes the library by genre, composer, mood, instrumentation, key, etc. \
(e.g. for \"show me calm piano pieces in minor\").\n\
Scores from the library – the person's own included, on the desktop and in the browser alike – are addressed by their `doc`, never by a file path: \
`search_library` finds them by title, composer or instruments (one hit per score; `browse_library` and `search_melody` give a doc too), \
`read_library_score` shows the voices (number, name, role, range), bars, key, tempo and – with fromBar/toBar – the notes of a bar range, \
`import_library_score` brings chosen parts or voices (and bars) into the project as instrument tracks in ONE undo step – one track per part \
(instrument) with all its voices, e.g. divisi; split 'voice' = one per voice. To put several movements one after another on the same tracks, import \
the first, then the next with trackIds (the first import's tracks, one per part in the order of voices – name the parts in voices to line them up when \
they are named or ordered differently) and startBeats = endBeats of the previous import; a part without a track of its own gets 0 = a new track. \
`analyze_score` takes a doc as well.\n\
Harmony (circle-of-fifths window \"Harmony\"): `analyze_harmony` reads the key and chord progression (scale degrees, functions), `suggest_chords` \
suggests continuations and cadences, `preview_chords` plays a progression, `write_chords` voices it in four parts and writes it \
as notes (track \"Chords\" or trackId). Chords with German note names (H = English B, B = English B♭; minor in lower case: a, fis) or as \
scale degrees (I, ii6, V7, V7/V, I64, N6). For \"write a cadence\": first suggest_chords with cadences:true, then write_chords. \
With corpus 'barock' or 'bach' the frequencies come only from those pieces.\n\
Fugue and counterpoint (window \"Fugue\"): the subject is a note clip. `fugue_subject` shows the answer (real/tonal), strettos and \
transformations; `transform_melody` writes inversion, retrograde, augmentation etc. as a track; `countersubject` creates an invertible \
countersubject; `write_fugue` builds a complete framework (2–4 voices, organ or harpsichord); `check_counterpoint` finds parallels, \
voice crossings and unprepared dissonances – then correct them specifically with setClipNotes and check again. Instruments for \
fugues: organ or harpsichord.";

/// What the tools need from the platform. Without its own implementation the
/// tool does not exist there (error text for the agent).
pub trait AgentShell: Send + Sync + 'static {
    /// The app state.
    fn state(&self) -> &AppState;
    /// Opens a project file in the window (asks about unsaved changes).
    fn open_in_window(&self, path: &Path) -> Result<(), String> {
        self.state().host().emit("open-file", &path.display().to_string());
        Ok(())
    }
    /// Identifiers of the packages that can be downloaded.
    fn pack_ids(&self) -> Vec<&'static str> {
        Vec::new()
    }
    /// Downloads a package and adds it to the active library (in the background);
    /// target folder and, if a job is already running, the position in the queue.
    fn library_download(&self, _id: &str) -> Result<(PathBuf, Option<usize>), String> {
        Err(NOT_HERE.into())
    }
    /// Adds files or folders to the active library (in the background); name of the library.
    fn library_add(&self, _paths: &[String]) -> Result<String, String> {
        Err(NOT_HERE.into())
    }
    /// All libraries.
    fn libraries(&self) -> Value {
        json!([])
    }
    /// Makes a library (name or identifier) active.
    fn library_switch(&self, _name: &str) -> Result<(), String> {
        Err(NOT_HERE.into())
    }
    /// New library, active right away if requested; returns (identifier, name).
    fn library_create(&self, _name: &str, _activate: bool) -> Result<(String, String), String> {
        Err(NOT_HERE.into())
    }
    /// Deletes a library; returns its identifier.
    fn library_delete(&self, _name: &str) -> Result<String, String> {
        Err(NOT_HERE.into())
    }
}

const NOT_HERE: &str = "Not available on this platform (the browser version can't download or index the library).";

/// The tools: a `klang_mcp::Handler` for both platforms.
pub struct Agent<S: AgentShell> {
    shell: S,
}

impl<S: AgentShell> Agent<S> {
    pub fn new(shell: S) -> Self {
        Self { shell }
    }
}

/// Waits briefly so the engine's status has settled (in the browser nothing
/// can wait).
fn settle() {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(std::time::Duration::from_millis(60));
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn tool(name: &'static str, title: &'static str, description: &'static str, input_schema: Value, read_only: bool) -> Tool {
    Tool { name, title, description, input_schema, read_only }
}

/// Placeholder for an ID in `execute` and `batch`: `$` plus letters, digits, `_`.
const PLACEHOLDER_PATTERN: &str = r"^\$[A-Za-z_][A-Za-z0-9_]*$";

fn command_schema() -> Value {
    let id = json!({ "type": ["integer", "string"], "minimum": 0, "pattern": PLACEHOLDER_PATTERN,
        "description": "ID, or a placeholder like \"$bass\" declared with ref in an earlier command" });
    let reference = json!({ "type": "string", "pattern": PLACEHOLDER_PATTERN, "description": "Placeholder for the new ID, e.g. \"$bass\" – usable as trackId/clipId in later commands (and later calls of a batch)" });
    let note = json!({ "type": "object", "properties": {
        "pitch": { "type": "integer", "minimum": 0, "maximum": 127 },
        "startBeats": { "type": "number", "minimum": 0 },
        "lengthBeats": { "type": "number", "exclusiveMinimum": 0 },
        "velocity": { "type": "integer", "minimum": 1, "maximum": 127, "description": "Velocity: pp≈32, p≈48, mp≈64, mf≈80, f≈100, ff≈120" },
        "vibrato": { "type": "number", "minimum": 0, "maximum": 1, "description": "Vibrato depth in semitones, sets in with a delay (0.3 ≈ vocal); omit = 0" },
        "brightness": { "type": "number", "minimum": -1, "maximum": 1, "description": "Brightness relative to the instrument (−1 darker … +1 brighter); omit = 0" },
        "bend": { "type": "number", "minimum": -12, "maximum": 12, "description": "Slide-in: the note starts this many semitones off and glides in (−2 = from below); omit = 0" } },
        "required": ["pitch", "startBeats", "lengthBeats", "velocity"] });
    let notes = json!({ "oneOf": [
        { "type": "array", "items": note },
        { "type": "string", "description": "Compact: \"C4 1 · E4 .5 v100 · G4 .5\" – pitch (German names: H = B, B = B♭; or MIDI number), length in beats (omitted = previous), v<velocity> (stays until the next v, start 100), @<start>; C4+E4+G4 = chord, r = rest; separators · , ; | or line break. Notes follow each other unless @ is given." }
    ] });
    let clip = json!({ "type": "object", "description": "id 0 (or omitted) = new. Instrument clips: source {type:'notes', lengthBeats, notes}",
        "properties": { "id": id, "ref": reference, "name": { "type": "string" }, "startBeats": { "type": "number", "minimum": 0 },
            "source": { "type": "object", "properties": {
                "type": { "enum": ["notes", "audio"] }, "lengthBeats": { "type": "number" }, "notes": notes,
                "assetId": { "type": "string" }, "sourceBpm": { "type": "number" }, "warp": { "type": "boolean" }, "durationSecs": { "type": "number" } },
                "required": ["type"] } },
        "required": ["startBeats", "source"] });
    let instrument = json!({ "type": "object", "properties": {
        "preset": { "type": "string" },
        "params": { "type": "object", "properties": {
            "helligkeit": { "type": "number", "minimum": 0, "maximum": 1 }, "abklingen": { "type": "number", "minimum": 0, "maximum": 1 },
            "koerper": { "type": "number", "minimum": 0, "maximum": 1 }, "anschlag": { "type": "number", "minimum": 0, "maximum": 1 } } } } });
    let track_patch = json!({ "type": "object", "properties": {
        "name": { "type": "string" }, "color": { "type": "string" }, "gainDb": { "type": "number", "minimum": -60, "maximum": 12 },
        "pan": { "type": "number", "minimum": -1, "maximum": 1 }, "mute": { "type": "boolean" }, "solo": { "type": "boolean" },
        "pitchSemitones": { "type": "number", "minimum": -24, "maximum": 24 }, "instrument": instrument,
        "view": { "enum": ["compact", "notes"], "description": "Display: compact or note view (piano roll); no undo step" },
        "compactHeight": { "type": "number", "description": "Row height in compact view in pixels (0 = default, otherwise 40–1200); no undo step" },
        "notesHeight": { "type": "number", "description": "Row height in note view in pixels (0 = default, otherwise 40–1200); no undo step" },
        "group": { "type": ["integer", "string"], "description": "Group of the track: its id or placeholder, 0 = no group" } } });
    let group_patch = json!({ "type": "object", "properties": {
        "name": { "type": "string" }, "color": { "type": "string" }, "gainDb": { "type": "number", "minimum": -60, "maximum": 12, "description": "Added to the level of every track in the group" },
        "mute": { "type": "boolean" }, "solo": { "type": "boolean" }, "collapsed": { "type": "boolean", "description": "Folded in the track list; no undo step" } } });
    let variant = |t: &str, props: Value, req: &[&str]| {
        let mut p = props;
        p["type"] = json!({ "const": t });
        let mut r = vec!["type"];
        r.extend_from_slice(req);
        json!({ "type": "object", "properties": p, "required": r })
    };
    json!({ "oneOf": [
        variant("setProject", json!({ "patch": { "type": "object", "properties": {
            "name": { "type": "string" }, "tempoBpm": { "type": "number", "minimum": 20, "maximum": 400 },
            "beatsPerBar": { "type": "integer", "minimum": 1, "maximum": 32 }, "beatUnit": { "enum": [1, 2, 4, 8, 16, 32] } } } }), &["patch"]),
        variant("addTrack", json!({ "track": { "type": "object", "description": "id 0 (or omitted) = new; kind 'audio' or 'instrument'; other fields as in setTrack, optional clips[]",
            "properties": { "id": id, "ref": reference, "name": { "type": "string" }, "kind": { "enum": ["audio", "instrument"] }, "clips": { "type": "array", "items": clip } },
            "required": ["name", "kind"] }, "index": { "type": "integer", "minimum": 0 } }), &["track"]),
        variant("removeTrack", json!({ "trackId": id }), &["trackId"]),
        variant("moveTrack", json!({ "trackId": id, "index": { "type": "integer", "minimum": 0 } }), &["trackId", "index"]),
        variant("setTrack", json!({ "trackId": id, "patch": track_patch }), &["trackId", "patch"]),
        variant("addClip", json!({ "trackId": id, "clip": clip }), &["trackId", "clip"]),
        variant("removeClip", json!({ "trackId": id, "clipId": id }), &["trackId", "clipId"]),
        variant("setClip", json!({ "trackId": id, "clipId": id, "patch": { "type": "object", "properties": {
            "name": { "type": "string" }, "startBeats": { "type": "number", "minimum": 0 },
            "sourceBpm": { "type": "number", "minimum": 20, "maximum": 400 }, "warp": { "type": "boolean" } } } }), &["trackId", "clipId", "patch"]),
        variant("setClipNotes", json!({ "trackId": id, "clipId": id,
            "notes": notes.clone(),
            "lengthBeats": { "type": "number", "exclusiveMinimum": 0, "description": "Optional new clip length" } }), &["trackId", "clipId", "notes"]),
        variant("addGroup", json!({ "group": { "type": "object", "description": "id 0 (or omitted) = new; tracks join it with setTrack {group}",
            "properties": { "id": id, "ref": reference, "name": { "type": "string" }, "color": { "type": "string" }, "gainDb": { "type": "number", "minimum": -60, "maximum": 12 },
                "mute": { "type": "boolean" }, "solo": { "type": "boolean" }, "collapsed": { "type": "boolean" } } } }), &["group"]),
        variant("setGroup", json!({ "groupId": id, "patch": group_patch }), &["groupId", "patch"]),
        variant("removeGroup", json!({ "groupId": id }), &["groupId"])
    ] })
}

fn tools(packs: &[&'static str]) -> Vec<Tool> {
    let id = json!({ "type": "integer", "minimum": 1 });
    // how much a detected melody is smoothed when it becomes notes
    let smooth = json!({ "enum": ["off", "light", "medium", "strong"] });
    // one line or several voices at once, and how many
    let mode = json!({ "enum": ["mono", "poly"], "description": "mono = one line (default), poly = several voices at once" });
    let voices = json!({ "type": ["string", "integer"], "description": "'auto' (default: found from the recording) or 1–6" });
    let pattern = json!({ "enum": sound::all_preview_keys(),
        "description": "Preview pattern per instrument – pluck: akkord (chord), tonleiter (scale), einzelton (single note); drums: beat, kick, snare, clap, hihat, hihat-offen (open), toms; bass: basslinie (bass line), einzelton" });
    let kind = json!({ "enum": ["pluck", "drums", "bass", "organ", "harpsichord", "synth"],
        "description": "pluck = plucked strings, drums = drum kit, bass = bass synth, organ = pipe organ, harpsichord, synth = synth for pads" });
    let key_arg = json!({ "type": "string", "description": "Key in German notation, e.g. \"C-Dur\" (C major), \"a-Moll\" (A minor), \"Es-Dur\" (E♭ major), \"fis-Moll\" (F♯ minor); omit = estimated from the notes" });
    let instrument_kind = json!({ "enum": ["organ", "harpsichord", "pluck"], "description": "Sound of the new tracks (default organ)" });
    let doc = json!({ "type": "integer", "minimum": 0, "description": "A score of the library: the doc of any of its voices (search_library, browse_library, search_melody)" });
    let bar = json!({ "type": "integer", "minimum": 0, "description": "Bar number as printed (0 = pickup)" });
    let voice_picks = json!({ "type": "array", "items": { "type": ["integer", "string"] }, "minItems": 1,
        "description": "Voices by number (from read_library_score) or name (part, e.g. \"Violin 1\"; a part name takes all its voices)" });
    let chords_arg = json!({ "type": "array", "items": { "type": "string" }, "maxItems": 64,
        "description": "Chords as symbols with German note names (C, a = A minor, G7, fis7, h°, Hø7, gis°7, Cmaj7, Dsus4, C/E; lower case = minor, -is = ♯, -es = ♭, B = English B♭, H = English B) \
                        or as scale degrees in the key (I, ii6, I64, V7, vii°7, V7/V, V7/vi, N6, ♭VI, iv6)" });
    vec![
        tool("get_project", "Read project",
            "Returns the open project: tempo, time signature, tracks with IDs, level, pan, pitch, instrument parameters and clips (start/length in beats). With includeNotes also the notes of the instrument clips. \
             revision changes with every change – pass it to execute as ifRevision so you don't overwrite someone else's change. \
             trackIds: only these tracks; fields: only these fields, e.g. [\"tempoBpm\", \"tracks.name\", \"tracks.clips\"] (\"tracks.x\" = field x of every track; track id and revision always come along).",
            schema(json!({ "includeNotes": { "type": "boolean" },
                "trackIds": { "type": "array", "items": { "type": "integer", "minimum": 1 } },
                "fields": { "type": "array", "items": { "type": "string" } } }), &[]), true),
        tool("execute", "Apply changes",
            "Runs one or more project commands as one undo step (on error everything is rolled back). Examples: {\"type\":\"setProject\",\"patch\":{\"tempoBpm\":96}}, {\"type\":\"setTrack\",\"trackId\":3,\"patch\":{\"gainDb\":-6,\"pan\":-0.3}}, {\"type\":\"addTrack\",\"track\":{\"id\":0,\"name\":\"Bass\",\"kind\":\"instrument\",\"clips\":[{\"id\":0,\"name\":\"Riff\",\"startBeats\":0,\"source\":{\"type\":\"notes\",\"lengthBeats\":4,\"notes\":[{\"pitch\":45,\"startBeats\":0,\"lengthBeats\":1,\"velocity\":100}]}}]}}.\n\
             Placeholders instead of reading IDs afterwards: {\"type\":\"addTrack\",\"track\":{\"ref\":\"$bass\",\"name\":\"Bass\",\"kind\":\"instrument\"}} then {\"type\":\"addClip\",\"trackId\":\"$bass\",\"clip\":{\"ref\":\"$riff\",\"startBeats\":0,\"source\":{\"type\":\"notes\",\"lengthBeats\":4,\"notes\":\"A2 1 · C3 · E3 · G3\"}}} – \
             the response contains ids {\"$bass\": 7, \"$riff\": 8}. Notes as a list or compact text (see notes). ifRevision: only if the project is still at this revision (from get_project or the last write response).",
            schema(json!({ "commands": { "type": "array", "minItems": 1, "items": command_schema() },
                "label": { "type": "string", "description": "Label of the undo step" },
                "ifRevision": { "type": "integer", "description": "Fails (nothing changed) if the project's revision differs – someone else changed it in the meantime" } }), &["commands"]), false),
        tool("batch", "Several tools at once",
            "Runs several tool calls in a row as ONE undo step and stops at the first error – then everything before it is reverted too \
             (except effects outside the project such as saved sounds, exports or chat messages). Saves a round per call. \
             In args, \"$0.trackId\" uses a field of the result of call 0 (\"$2.trackIds.1\" = second entry), \"$bass\" a placeholder declared with ref in an execute before. \
             Not allowed inside: batch, chat_poll, undo, redo, open_project and the history tools that move or mark the history (checkpoint, goto, \
             variant_new, variant_switch, combine). Results come back in order (execute without the project).",
            schema(json!({
                "calls": { "type": "array", "minItems": 1, "maxItems": 50, "items": { "type": "object", "properties": {
                    "tool": { "type": "string" }, "args": { "type": "object" } }, "required": ["tool"] } },
                "label": { "type": "string", "description": "Label of the undo step" },
                "ifRevision": { "type": "integer", "description": "Only if the project is still at this revision" } }), &["calls"]), false),
        tool("undo", "Undo", "Reverts the last step (including changes made in the interface).", schema(json!({}), &[]), false),
        tool("redo", "Redo", "Restores the most recently reverted step.", schema(json!({}), &[]), false),
        tool("history", "Change history",
            "The change history as a compact table: the steps of the active variant, oldest first – step (ID for goto), label, detail (where: tracks, \
             bars, notes, tempo), key figures after the step (tracks, notes, length in beats; tempo, meter and key only where they change), \
             current = the project's state now, redo = steps after it that can be redone, checkpoints on the step, branches = other variants or \
             set-aside steps that leave the line there. Plus the variants and checkpoints. limit: rows at most (default 40, around the current step).",
            schema(json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 1000 } }), &[]), true),
        tool("checkpoint", "Set checkpoint",
            "Names the current state – before a bigger change, so you and the person can find it and go back to it (goto with checkpoint). \
             Doesn't change the project; appears as a flag in the History window. Without a name: \"Checkpoint 1\", \"Checkpoint 2\" …",
            schema(json!({ "name": { "type": "string", "minLength": 1, "maxLength": 80 } }), &[]), false),
        tool("goto", "Go to a state",
            "Puts the project to the state after a step – like undo/redo across many steps at once; the later steps stay and can be redone. \
             step: ID from history; or checkpoint: its ID or name. A step on another variant switches to that variant; a step among set-aside \
             steps (edited after an undo) brings them back as a new variant.",
            schema(json!({ "step": { "type": "integer", "minimum": 1 }, "checkpoint": { "type": ["integer", "string"], "description": "Checkpoint ID or name" } }), &[]), false),
        tool("variant_new", "New variant",
            "Starts a variant – a branch of the history – at the current state, or at step/checkpoint (the project goes there). The following \
             changes go into the new variant, the other variants stay as they are. To try an idea without losing the current version: variant_new \
             named after the idea, change, compare with variant_switch, bring the good parts together with combine.",
            schema(json!({ "name": { "type": "string", "minLength": 1, "maxLength": 80, "description": "Default \"Variant 2\" …" },
                "step": { "type": "integer", "minimum": 1 }, "checkpoint": { "type": ["integer", "string"], "description": "Checkpoint ID or name" } }), &[]), false),
        tool("variant_switch", "Switch variant",
            "Switches to a variant (ID or name): the project goes to its newest state, new changes go into it.",
            schema(json!({ "variant": { "type": ["integer", "string"], "description": "Variant ID or name" } }), &["variant"]), false),
        tool("variant_list", "List variants",
            "All variants: ID, name, active, newest step (head) and its label, where it branches off the others (fork) and its own steps since then.",
            schema(json!({}), &[]), true),
        tool("combine", "Combine variants",
            "Combines variants into a NEW variant (one step; the variants themselves stay) and switches to it. Three-way merge against their \
             common origin: per track the bars each variant changed (rounded to whole bars), the track itself (sound, mix, name, added or \
             removed) and tempo/time signature. What only one variant changed is taken from it; where several changed the same thing \
             differently there is a conflict. If a conflict has no choice, nothing changes: the response lists the decisions (id, by, take) \
             and in open the conflicts, each with its options (variant, name, the notes in those bars as \"pitch length @beat\"). Then call again \
             with choices {id: variant ID or name, 0 = origin}; \"*\": variant decides every conflict that variant is part of. A choice may also \
             override a decision without conflict (0 = keep the origin there). base: the variant the result builds on (track order, name; default \
             the active one); others: the variants to bring in. dryRun: only show the decisions.",
            schema(json!({
                "base": { "type": ["integer", "string"], "description": "Variant ID or name; default the active variant" },
                "others": { "type": "array", "minItems": 1, "items": { "type": ["integer", "string"] }, "description": "Variant IDs or names" },
                "choices": { "type": "object", "additionalProperties": { "type": ["integer", "string"] },
                    "description": "Decision id (from open/decisions) → variant ID or name, 0 or \"origin\" = as in the common origin; \"*\" = every conflict that variant is part of" },
                "name": { "type": "string", "maxLength": 80, "description": "Name of the new variant; default \"A + B\"" },
                "dryRun": { "type": "boolean", "description": "Only show the decisions, change nothing" } }), &["others"]), false),
        tool("transport", "Control playback", "Starts/stops playback or jumps to a position.",
            schema(json!({ "action": { "enum": ["play", "stop", "toggle", "rewind"] }, "positionBeats": { "type": "number", "minimum": 0 } }), &[]), false),
        tool("set_metronome", "Metronome", "Turns the metronome on or off.", schema(json!({ "enabled": { "type": "boolean" } }), &["enabled"]), false),
        tool("audio_devices", "Audio devices",
            "Lists the system's output devices (speakers, headphones, interfaces) and recording devices (microphones), the default devices \
             and the current choice (null = system default) along with the device that is currently playing.",
            schema(json!({}), &[]), true),
        tool("set_audio_device", "Select audio device",
            "Selects the output (ausgabe) or recording (eingabe) device – takes effect immediately (playback continues on the new device) and also applies at the \
             next start. name: exactly as returned by audio_devices; null, empty or \"System default\" = the system's default device. \
             Not possible during a recording.",
            schema(json!({ "kind": { "enum": ["ausgabe", "eingabe"] }, "name": { "type": ["string", "null"] } }), &["kind"]), false),
        tool("export_project", "Export",
            "Exports the project (or with range=true the range selected in the timeline) as a file – the way it sounds: \
             muted tracks are left out, solo applies. Formats: wav (uncompressed), flac (lossless, smaller than WAV), \
             opus (small, for phones and sharing; bitrate in kbit/s, default 160), stems (a folder with one WAV per track) and \
             midi (only the notes of the instrument tracks). Location: Music/Klangwelt/Exporte; name without extension, or a full path. \
             An existing file with the same name is overwritten.",
            schema(json!({
                "format": { "type": "string", "enum": ["wav", "flac", "opus", "stems", "midi"] },
                "name": { "type": "string", "description": "File name without extension or full path; omit = project name" },
                "bitrate": { "type": "integer", "enum": [64, 96, 128, 160, 192, 256, 320], "description": "opus only: kbit/s (default 160)" },
                "bits": { "type": "integer", "enum": [16, 24], "description": "wav, flac, stems: bit depth (default 24)" },
                "range": { "type": "boolean", "description": "Only the selected range (default: the whole piece)" }
            }), &["format"]), false),
        tool("open_project", "Open project",
            "Opens a .klangwelt file in the Klangwelt window and brings it to the front. If the open project has unsaved changes, \
             the interface asks first (the reply comes immediately, the opening follows in the window).",
            schema(json!({ "path": { "type": "string", "description": "Path to the project file" } }), &["path"]), false),
        tool("chat_poll", "Fetch tasks (chat)",
            "Waits (long poll) for new tasks typed into the chat of the Klangwelt interface and returns them – or an empty list when the wait time is up. Meant for a listening subagent (see prompt 'zuhoeren') so that the main agent stays available. Workflow: call chat_poll → confirm each task with chat_reply (done=false) → have it carried out → finish with chat_reply (done=true) → call chat_poll again. \
             Picked-up tasks are reserved for client (shown as \"claimed by …\"); without a chat_reply within leaseSeconds they are offered again (redelivered counts how often). \
             A task with cancels is a stop request for those task ids. While chat_poll is waiting, the interface shows \"Agent listening\". peek: true = like chat_peek.",
            schema(json!({ "timeoutSeconds": { "type": "number", "minimum": 1, "maximum": 590,
                "description": "Maximum wait time; if a task arrives earlier, chat_poll answers immediately. Default 50 s – fits under the 60 s limit that Claude Code sets for HTTP calls without further configuration. With \"timeout\": 330000 in the server's MCP configuration, 280 s are possible (one request every ~5 minutes)." },
                "client": { "type": "string", "maxLength": 40, "description": "Name of the poller, e.g. \"zuhoerer\" (default \"agent\")" },
                "leaseSeconds": { "type": "number", "minimum": 10, "maximum": 3600, "description": "How long a picked-up task stays reserved without a chat_reply (default 60)" },
                "peek": { "type": "boolean", "description": "Only look (= chat_peek), pick up nothing" },
                "sinceId": { "type": "integer", "minimum": 0, "description": "With peek: messages after this id" } }), &[]), true),
        tool("chat_peek", "Look at the chat",
            "Returns the chat messages after sinceId (tasks with status and \"claimed by\", replies) without picking anything up – for workers between steps \
             (follow-ups, stop requests) and for status questions. Without sinceId the last 20. lastId = sinceId for the next call.",
            schema(json!({ "sinceId": { "type": "integer", "minimum": 0 } }), &[]), true),
        tool("chat_reply", "Reply in chat",
            "Writes a reply into the chat of the interface. replyTo = id of the task from chat_poll – a reply confirms the task (it's no longer offered again); done = true (default) marks it as done, false for confirmations and progress updates. client: as in chat_poll.",
            schema(json!({ "text": { "type": "string" }, "replyTo": { "type": "integer" }, "done": { "type": "boolean" },
                "client": { "type": "string", "maxLength": 40 } }), &["text"]), false),
        tool("list_instruments", "Show instruments",
            "Describes the instruments of the sound workshop: all parameters with range, default value and meaning, the three fields (axes, starting points, which controls they set), presets (= starting points of the character field), preview patterns, for the synth the envelope templates (verlauf) and for the drums the key mapping.",
            schema(json!({}), &[]), true),
        tool("create_instrument_track", "Create instrument track",
            "Creates an instrument track with the default sound and a 4-bar example pattern (drum beat, bass line or arpeggio).",
            schema(json!({ "kind": kind }), &["kind"]), false),
        tool("set_instrument", "Switch instrument",
            "Sets the instrument of an instrument track (default values, optionally a preset from list_instruments).",
            schema(json!({ "trackId": id, "kind": kind, "preset": { "type": "string" } }), &["trackId", "kind"]), false),
        tool("describe_sound", "Measure sound",
            "Measures the sound of an instrument track: parameters and measurements in words – pluck: brightness, warmth, velocity, decay; drums: kick pitch (measured), punch, length, snare shell, snare wires, hi-hat; bass: fundamental, brightness, depth, bite, decay; synth: brightness, fullness, motion, attack, release.",
            schema(json!({ "trackId": id }), &["trackId"]), true),
        tool("set_sound", "Set sound",
            "Changes sound parameters of an instrument track (only the given values). Names and ranges per instrument: see list_instruments – e.g. pluck: helligkeit, abklingen, koerper, anschlag (0…1); drums: kick.grundton (MIDI note 24–52), kick.punch, kick.laenge, snare.kessel (MIDI note), hat.helligkeit, tasten (0 drum kit, 1 every key a tuned drum by pitch) …; bass: wellenform, filter, resonanz, anschlag, abklingen, sub, biss, glide; synth: stimmen, verstimmung, wellenform, sub, schimmer, filter, resonanz, bewegung, tempo. For all: farbe (dark ↔ bright, 0.5 neutral), rauheit (smooth ↔ rough), breite (narrow ↔ wide), raum (near ↔ far), and except for the drums also waerme and schwebung. charakter.x/charakter.y move the dot in the character field (sets the associated controls). Synth: verlauf = template, curves = drawn curves, verlauf.ausklang (seconds), verlauf.puls (0 off, 1 quarter, 2 eighth, 3 sixteenth), verlauf.pulstiefe. Returns the new measurements afterwards; with preview also plays it.",
            schema(json!({ "trackId": id,
                "params": { "type": "object", "additionalProperties": { "type": "number" } },
                "verlauf": { "type": "string", "description": "Synth: envelope template, e.g. Anschwellen, Glocke, Atmen, Aufblühen, Pulsieren, Wellen, Gate, Stich" },
                "curves": { "type": "object", "description": "Synth: { laut: [[second, value 0–1], …], hell: […] } (laut = loudness, hell = brightness) – values linear between the points, held after the last one",
                    "properties": { "laut": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } },
                        "hell": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } } } },
                "preset": { "type": "string", "description": "Name for this setting, e.g. 'warm-weich'" },
                "preview": pattern }), &["trackId"]), false),
        tool("set_sound_field", "Set sound field",
            "Sets a dot in one of the three fields of the Sound window – x/y each 0…1 or a named starting point (list_instruments → fields). charakter: per instrument (drums: weight crisp ↔ powerful × complexity; synth: fullness × complexity …), klangfarbe: dark ↔ bright × smooth ↔ rough, raum: narrow ↔ wide × near ↔ far. Changes only this field's controls; returns the measurements.",
            schema(json!({ "trackId": id, "field": { "enum": ["charakter", "klangfarbe", "raum"] },
                "x": { "type": "number", "minimum": 0, "maximum": 1 }, "y": { "type": "number", "minimum": 0, "maximum": 1 },
                "preset": { "type": "string", "description": "Starting point, e.g. 'Trap' (drums charakter), 'Staub' (klangfarbe), 'Kirche' (raum)" },
                "preview": pattern }), &["trackId", "field"]), false),
        tool("list_saved_sounds", "Show saved sounds",
            "The sound library: your own sounds, saved in the Sound window (\"My sounds\") or with save_sound – across all projects. Per sound: identifier, name, instrument and the position in the fields in words. With kind only those of one instrument.",
            schema(json!({ "kind": kind }), &[]), true),
        tool("save_sound", "Save sound",
            "Saves the current sound of an instrument track (all controls, position in the fields, for the synth the envelope) under a name in the sound library. If the name already exists for the same instrument, that sound is replaced (ersetzt: true).",
            schema(json!({ "trackId": id, "name": { "type": "string", "maxLength": 60 } }), &["trackId", "name"]), false),
        tool("load_saved_sound", "Load saved sound",
            "Loads a sound from the sound library (name or identifier from list_saved_sounds) into an instrument track – one undo step; a sound of another instrument switches the track's instrument. Returns the measurements; with preview also plays it.",
            schema(json!({ "trackId": id, "sound": { "type": "string", "description": "Name or identifier" }, "preview": pattern }), &["trackId", "sound"]), false),
        tool("rename_saved_sound", "Rename saved sound",
            "Gives a sound in the sound library a new name (unique per instrument).",
            schema(json!({ "sound": { "type": "string", "description": "Name or identifier" }, "name": { "type": "string", "maxLength": 60 } }), &["sound", "name"]), false),
        tool("delete_saved_sound", "Delete saved sound",
            "Deletes a sound from the sound library – permanently, only if the person explicitly asks for it. Tracks that loaded it keep their sound.",
            schema(json!({ "sound": { "type": "string", "description": "Name or identifier" } }), &["sound"]), false),
        tool("list_drum_patterns", "Show drum patterns",
            "Drum patterns of typical genres (based on the Pocket Operations booklet): per genre a description and patterns with tempo (BPM), recommended swing, tip and rows as text – one character per sixteenth, x = hit (e.g. kick \"x...x...x...x...\"). Plus the drums (kick, snare, clap, rim, hhc = hi-hat closed, hho = open, crash, ride, cow, tomh, tomm, toml, shk).",
            schema(json!({ "genre": { "type": "string", "description": "only this genre, e.g. 'House', 'Trap', 'Latin'" } }), &[]), true),
        tool("write_drum_pattern", "Write drum pattern",
            "Writes a pattern (genre + pattern, or your own rows) as a drum clip – one undo step. Without trackId a new drum track with the genre's sound is created; with trackId a new clip after the last one (or at startBeats); with clipId its notes are replaced. Accents are set (backbeat loud, ghost snares quiet), swing shifts every second sixteenth.",
            schema(json!({ "genre": { "type": "string" }, "pattern": { "type": "string", "description": "Name from list_drum_patterns; empty = first pattern of the genre" },
                "rows": { "type": "object", "description": "your own rows instead of a pattern: { kick: \"x...x...x...x...\", snare: [5, 13], … } (text per sixteenth or steps from 1)" },
                "bars": { "type": "integer", "enum": [1, 2, 3, 4], "description": "Length in bars (patterns are repeated), default 4" },
                "swing": { "type": "number", "minimum": 0, "maximum": 100, "description": "Percent; default = the pattern's recommendation" },
                "trackId": id, "clipId": id, "startBeats": { "type": "number", "minimum": 0 },
                "setTempo": { "type": "boolean", "description": "Set the project tempo to the pattern's tempo" },
                "preview": { "type": "boolean", "description": "play it afterwards" } }), &[]), false),
        tool("read_drum_pattern", "Read drum clip",
            "Reads a drum clip as a grid (up to 4 bars): rows as text (x = hit per sixteenth), estimated swing and how many notes don't fit the grid.",
            schema(json!({ "trackId": id, "clipId": id }), &["trackId", "clipId"]), true),
        tool("list_song_templates", "Show song templates",
            "Templates of complete pieces: tempo, key, chords (bars per chord), roles (key, name, instrument, sound) and sections (name, bars, kind, level per role 0–2). Basis for write_song.",
            schema(json!({}), &[]), true),
        tool("write_song", "Generate song",
            "Generates a complete song as tracks: one track per melodic role, all drums together in one drum track, one clip per section. Sets the tempo and 4/4. Build-up sections (aufbau) get louder and brighter and end (with role fx) in a snare roll, outros fade out. One undo step; replaces the most recently generated song tracks (replace: false keeps them).",
            schema(json!({
                "template": { "type": "string", "description": "trance, ambient, deep-house, synthwave, lofi" },
                "tempo": { "type": "number", "minimum": 40, "maximum": 220 },
                "key": { "type": "string", "description": "e.g. \"a-Moll\" (A minor), \"D-Dur\" (D major)" },
                "chords": { "type": "array", "items": { "type": "string" }, "description": "Chords in rotation, e.g. [\"a\", \"F\", \"C\", \"G\"] or degrees [\"i\", \"VI\", \"III\", \"VII\"]" },
                "chordBars": { "type": "integer", "minimum": 1, "maximum": 8 },
                "sections": { "type": "array", "description": "your own sections instead of the template's: [{ name, bars, kind, levels: { pad: 2, kick: 1, … } }]",
                    "items": { "type": "object", "properties": {
                        "name": { "type": "string" }, "bars": { "type": "integer", "minimum": 1, "maximum": 64 },
                        "kind": { "enum": ["intro", "teil", "aufbau", "break", "hoehepunkt", "outro"] },
                        "levels": { "type": "object", "additionalProperties": { "type": "integer", "minimum": 0, "maximum": 2 } } }, "required": ["name", "bars"] } },
                "replace": { "type": "boolean" },
                "play": { "type": "boolean", "description": "play from the start afterwards" } }), &["template"]), false),
        tool("preview_sound", "Preview sound", "Plays a short preview pattern with the instrument track's sound, independent of playback.",
            schema(json!({ "trackId": id, "pattern": pattern }), &["trackId"]), false),
        tool("analyze_clip", "Analyze recording",
            "Analyzes an audio clip: detected notes (cleaned up) with how they were played – velocity 1–127 (dynamics relative to the recording, the median note is mf = 80), attack (normal, accent, sforzato) and articulation (normal, staccato, legato) – key, tempo estimate and hits with fundamental (also drums: kick, snare, toms; grundton null = unpitched like a hi-hat). \
             mode 'poly': several voices at once instead of one line – how many voices (voices 'auto' or 1–6) and the notes of each voice (stimme 1 = the highest).",
            schema(json!({ "trackId": id, "clipId": id, "mode": mode, "voices": voices }), &["trackId", "clipId"]), true),
        tool("melody_to_track", "Import melody",
            "Creates an instrument track from the detected melody of an audio clip (smoothed, quantized to sixteenths) – with the detected dynamics as velocity, a brighter attack for accents and sforzati, staccato notes short and legato notes up to the next one. smooth: see audio_to_notes.",
            schema(json!({ "trackId": id, "clipId": id, "smooth": smooth }), &["trackId", "clipId"]), false),
        tool("audio_to_notes", "Convert recording to notes",
            "Converts audio clips into notes – as a new instrument track directly below the audio track, one clip per audio clip at the same position (one undo step). \
             target 'melodie': detected pitches, snapped to sixteenths, with the played dynamics, accents/sforzati (velocity, brighter attack) and staccato/legato lengths (a low register becomes a bass track); 'rhythmus': detected hits as a drum track \
             (kick, snare, hi-hat by timbre). Without clipId all audio clips of the track. \
             smooth (melody only, default 'medium'): first merges what pitch detection split up – unusually short fragments (scoops, octave slips, vibrato swings) \
             into a neighbour, a held note split into notes of the same pitch back into one; 'off' keeps every detected note. \
             mode 'poly' (melody only): several voices at once – one instrument track per voice directly below (the highest first, a low lowest voice becomes a bass track); voices 'auto' or 1–6.",
            schema(json!({ "trackId": id, "clipId": id, "target": { "enum": ["melodie", "rhythmus"] }, "smooth": smooth, "mode": mode, "voices": voices }), &["trackId", "target"]), false),
        tool("import_audio", "Insert audio files",
            "Inserts audio files as tracks (one undo step): one new audio track per file with a clip at startBeats. With the trackId of an audio track \
             the first file goes into that track, further ones as new tracks below. Formats: WAV (8/16/24/32 bit, floating point), AIFF, FLAC, Opus; \
             not yet MP3, AAC/M4A and Ogg Vorbis. Stereo is averaged to mono. The clips play at their original tempo (warp off).",
            schema(json!({ "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "Full file paths" },
                "trackId": id, "startBeats": { "type": "number", "minimum": 0 } }), &["paths"]), false),
        tool("delete_range", "Delete time range",
            "Deletes the time range [startBeats, endBeats) from all tracks; everything after it moves up by its length. Clips inside the range are removed, \
             clips that overlap it are shortened (notes in the range are removed, audio is cut). One undo step; the selection is cleared afterwards.",
            schema(json!({ "startBeats": { "type": "number", "minimum": 0 }, "endBeats": { "type": "number", "minimum": 0 } }), &["startBeats", "endBeats"]), false),
        tool("quantize_notes", "Quantize notes",
            "Snaps notes of a note clip to the grid (in project time): start to the nearest grid point, with lengths also the end. \
             grid in beats (1 = quarter, 0.5 = eighth, 0.25 = sixteenth); notes = indices of the notes (sorted as in get_project), omit = all.",
            schema(json!({ "trackId": id, "clipId": id, "grid": { "type": "number", "minimum": 0.015625, "maximum": 16 }, "lengths": { "type": "boolean" },
                "notes": { "type": "array", "items": { "type": "integer", "minimum": 0 } } }), &["trackId", "clipId", "grid"]), false),
        tool("smooth_notes", "Smooth notes",
            "Smooths the notes of a note clip, e.g. a melody converted from a recording (one undo step): notes unusually short for the piece, the grid and in seconds \
             go into a neighbour they touch (keeping its pitch); consecutive notes of the same pitch without a gap become one unless the second is clearly louder \
             or accented; isolated, much softer blips are removed. The merged note keeps the attack (velocity, accent) of where it starts. Chords and drum tracks \
             are left alone. strength 'light' | 'medium' (default) | 'strong'; notes = indices (sorted as in get_project), omit = all.",
            schema(json!({ "trackId": id, "clipId": id, "strength": { "enum": ["light", "medium", "strong"] },
                "notes": { "type": "array", "items": { "type": "integer", "minimum": 0 } } }), &["trackId", "clipId"]), false),
        tool("list_libraries", "List libraries",
            "All libraries (one index each) with scores, included packages and folders; active = the one that is searched.",
            schema(json!({}), &[]), true),
        tool("switch_library", "Switch library",
            "Makes a library the active one (identifier or name from list_libraries). Search, tree and harmony statistics then come from it; stays active across a restart.",
            schema(json!({ "library": { "type": "string" } }), &["library"]), false),
        tool("create_library", "Create library",
            "Creates an empty library and makes it active (activate: false keeps the current one active). Then fill it with download_library or add_to_library.",
            schema(json!({ "name": { "type": "string" }, "activate": { "type": "boolean" } }), &["name"]), false),
        tool("delete_library", "Delete library",
            "Deletes a library – only its index, never scores. \"Standard\" can't be deleted; if the deleted one was active, \"Standard\" becomes active.",
            schema(json!({ "library": { "type": "string" } }), &["library"]), false),
        tool("download_library", "Download library",
            "Downloads a score package, indexes it in the background and adds it to the active library (scores already present only once). \
             PDMX (mixed, varying quality): 'pdmx-kostprobe' (3,000 scores, ≈ 10 s), 'pdmx-auswahl' (30,000, ≈ 1.5 min), 'pdmx-voll' \
             (≈ 254,000, 1.9 GB download, ≈ 3.5 GB on disk – only if the person explicitly asks for it). \
             OpenScore (CC0, carefully engraved): 'openscore-lieder' (≈ 1,460 art songs), 'openscore-quartette' (≈ 200 quartet movements, incl. Haydn, Mozart, Beethoven). \
             KernScores (research editions, per era): 'kern-barock' (Bach: chorales, Well-Tempered Clavier, Art of Fugue …; Scarlatti, Vivaldi), \
             'kern-klassik' (Mozart and Beethoven piano sonatas, Beethoven quartets, Haydn), 'kern-romantik' (Chopin, Schubert …), 'kern-20jh' (Joplin, Gershwin). \
             DCML (with chord analysis by experts, makes the chord statistics more reliable): 'dcml-barock' (Bach suites, Corelli, Monteverdi …), \
             'dcml-klassik' (Mozart, Beethoven, C. P. E. Bach …), 'dcml-romantik', 'dcml-20jh'. The small collections load in seconds. \
             Answers immediately; query the progress with `library_status`. If a job is already running, the package goes into the queue \
             and starts on its own afterwards – so several packages can be requested one after another (`library_status`: queue).",
            schema(json!({ "id": { "type": "string", "enum": packs } }), &["id"]), false),
        tool("add_to_library", "Add to library",
            "Adds MusicXML/MXL files (or all of a folder) to the active library: they are indexed and merged into its index \
             (then they appear in search and tree). The files stay where they are. Ones already present are skipped. Runs in the background; \
             the result then appears in `library_status` (message, scores).",
            schema(json!({ "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1 } }), &["paths"]), false),
        tool("browse_library", "Browse library",
            "Organizes the loaded library like a tree: levels = order of the levels, path = chosen values of the first levels. \
             Answers with the branches of the next level (value + count) or – when path is as long as levels – with the scores \
             (doc, title, composer). Levels: genre, komponist, stimmung, faerbung, energie, tongeschlecht, tonart, taktart, takt, tempo, \
             besetzung, instrumente, satz, harmonik, laenge, schwierigkeit. Example: levels ['genre','besetzung'], path ['Klassik'] → \
             instrumentations of classical pieces. Genre and instrument group can apply more than once.",
            schema(json!({
                "levels": { "type": "array", "items": { "type": "string" } },
                "path": { "type": "array", "items": { "type": "string" } },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000 },
                "offset": { "type": "integer", "minimum": 0 }
            }), &["levels"]), true),
        tool("library_status", "Library: status",
            "Which library is active (name, scores, voices) and whether something is currently being downloaded, indexed or added.",
            schema(json!({}), &[]), true),
        tool("search_melody", "Search melody",
            "Searches the loaded library for pieces with this melody – independent of key and tempo. Give either notes (at least 5, duration in any unit) or trackId+clipId of an audio clip.",
            schema(json!({ "notes": { "type": "array", "items": { "type": "object", "properties": {
                    "midi": { "type": "integer", "minimum": 0, "maximum": 127 }, "duration": { "type": "number", "exclusiveMinimum": 0 } },
                    "required": ["midi", "duration"] } },
                "trackId": id, "clipId": id, "limit": { "type": "integer", "minimum": 1, "maximum": 50 } }), &[]), true),
        tool("analyze_harmony", "Analyze harmony",
            "Key (estimated from the notes or given) and chord progression with scale degrees (Roman numerals) and functions (Riemann) – of a track (trackId), \
             a clip (trackId + clipId) or, without either, of all instrument tracks together (the harmony of the whole texture). Grid: half a bar.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone() }), &[]), true),
        tool("suggest_chords", "Chord continuations",
            "Suggests continuations after a chord progression, best first (functional harmony: authentic cadence, deceptive cadence, plagal cadence, \
             double dominant, secondary dominants with the key they lead to, cadential 6-4, Neapolitan, borrowings from minor, circle of fifths). \
             Without chords: what to start with. Each suggestion has a stimmung (resolving, tense, surprising, bright, dark). \
             wegeZumSchluss: multi-step paths (up to 4 chords) from the last chord to an authentic, plagal, half, deceptive and Phrygian \
             cadence, with the mood per transition and – with a library loaded – each step's share there. \
             cadences: true also returns the key's cadence templates (authentic, plagal, \
             half cadence, Phrygian, deceptive, Picardy third, lament bass, circle-of-fifths sequence …).",
            schema(json!({ "chords": chords_arg.clone(), "key": key_arg.clone(), "cadences": { "type": "boolean" },
                "corpus": { "enum": ["alle", "barock", "bach"], "description": "Frequencies from all pieces, Baroque only or Bach only" } }), &[]), true),
        tool("write_chords", "Write chords",
            "Voices a chord progression in four parts (chorale style: smooth voice leading, no parallel fifths and octaves, leading tones rise, sevenths fall) \
             and writes it as notes – into trackId or into the track \"Chords\" (created if needed). If a note clip already lies there at startBeats, \
             the notes in this range are replaced, otherwise a new clip is created. With mode 'einfuegen' (insert), room is made at startBeats: \
             everything from there on moves back by the length of the chords in all tracks. One undo step. \
             preview: true also plays the progression.",
            schema(json!({ "chords": chords_arg.clone(), "key": key_arg.clone(),
                "startBeats": { "type": "number", "minimum": 0, "description": "Start in beats (default 0)" },
                "beatsPerChord": { "type": "number", "minimum": 0.25, "maximum": 32, "description": "Duration per chord in beats (default: one bar)" },
                "trackId": id, "preview": { "type": "boolean" },
                "mode": { "enum": ["ersetzen", "einfuegen"], "description": "ersetzen (default): replace the notes in the range of the target track; einfuegen: make room, everything after it moves back" } }), &["chords"]), false),
        tool("preview_chords", "Preview chords",
            "Plays a chord progression in four parts (sound of trackId, otherwise pluck) without changing the project.",
            schema(json!({ "chords": chords_arg, "key": key_arg,
                "beatsPerChord": { "type": "number", "minimum": 0.25, "maximum": 32 }, "trackId": id }), &["chords"]), false),
        tool("fugue_subject", "Examine fugue subject",
            "Examines a note clip as a fugue subject: key, length, answer (real or tonal, with the reason and altered notes), \
             possible strettos (distance, interval, overlap – only error-free ones) and the transformations.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone() }), &["trackId", "clipId"]), true),
        tool("transform_melody", "Transform melody",
            "Transforms a note clip (subject): umkehrung (inversion, mirrored diatonically), krebs (retrograde), krebsumkehrung (retrograde inversion), vergroesserung (augmentation, doubled values), \
             verkleinerung (diminution), versetzung (steps scale degrees within the key), chromatisch (steps semitones). write (default true) creates the result \
             as a new track at the same position.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone(),
                "transform": { "enum": ["umkehrung", "krebs", "krebsumkehrung", "vergroesserung", "verkleinerung", "versetzung", "chromatisch"] },
                "steps": { "type": "integer", "minimum": -24, "maximum": 24 }, "write": { "type": "boolean" },
                "instrument": instrument_kind.clone() }), &["trackId", "clipId", "transform"]), false),
        tool("countersubject", "Create countersubject",
            "Suggests countersubjects for the subject – checked in double counterpoint at the octave (invertible = also error-free below the \
             subject). With write: true the best one is placed as a new track below/above the subject.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone(), "write": { "type": "boolean" }, "instrument": instrument_kind.clone() }),
                &["trackId", "clipId"]), false),
        tool("write_fugue", "Create fugue",
            "Builds a fugue framework from a subject (note clip) and writes it as tracks (one per voice): exposition with answer and \
             countersubject, episodes (sequence over the circle of fifths), middle entries in the relative and subdominant keys, stretto, pedal point, \
             final cadence (minor with a Picardy third). One undo step. Then check with check_counterpoint and refine.",
            schema(json!({ "trackId": id, "clipId": id, "key": key_arg.clone(),
                "voices": { "type": "integer", "minimum": 2, "maximum": 4, "description": "Default 3" },
                "instrument": instrument_kind.clone(),
                "startBeats": { "type": "number", "minimum": 0, "description": "Default: after the last clip" },
                "countersubject": { "type": "boolean" }, "stretto": { "type": "boolean" }, "picardy": { "type": "boolean" } }),
                &["trackId", "clipId"]), false),
        tool("check_counterpoint", "Check counterpoint",
            "Checks the instrument tracks (one voice per track, drums excluded) for part-writing errors: parallel fifths and octaves, hidden \
             parallels in the outer voices, voice crossing, unprepared dissonances on strong beats, free dissonances, range, large \
             leaps. Returns the spots with bar, track IDs and explanation – to fix them, change the notes with execute/setClipNotes.",
            schema(json!({ "trackIds": { "type": "array", "items": id.clone() } }), &[]), true),
        tool("analyze_score", "Analyze score",
            "Analyzes a score: key, mood (valence/arousal), voice functions (main theme, counter-melody, inner voice, accompaniment figure, ostinato, bass) and cadences. \
             doc = a score of the library (from search_library, browse_library or search_melody – works in the browser too and for the person's own scores), \
             or path = a MusicXML/MXL file (desktop only).",
            schema(json!({ "doc": doc.clone(), "path": { "type": "string", "description": "MusicXML/MXL file (desktop)" } }), &[]), true),
        tool("search_library", "Search library",
            "Finds scores in the active library by words from title, composer, instruments, genre, key or mood (every word must occur, any case) – \
             the person's own scores included (origin 'own' in the browser, 'remote' = the package on the server). One hit per score: doc, title, composer, file. \
             Pass the doc to read_library_score, import_library_score or analyze_score. Composers are spelled as in the score, often in English \
             (Tchaikovsky, Mussorgsky) – try that spelling or a word of the title if nothing comes up. For a melody use search_melody, to browse by genre or \
             composer browse_library; their doc works the same.",
            schema(json!({ "text": { "type": "string", "minLength": 1 }, "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Default 20" } }), &["text"]), true),
        tool("read_library_score", "Read library score",
            "Reads a score of the library by its doc (from search_library, browse_library or search_melody) – no file path needed, works in the browser and for \
             the person's own scores. Returns title, key (German notation as in the other tools), time signature, tempo (quarters per minute), number of bars \
             (as printed; pickup = bar 0), cadences and the voices: number, name, part (instrument; a part can have several voices, e.g. divisi), role, note count, \
             range and the first/last bar in which each plays. \
             With fromBar/toBar also the notes of those bars for the chosen voices (voices: numbers or names, default all) in compact notation \
             \"pitch length @start\" – lengths and starts in quarters from the start of fromBar, exactly what import_library_score would bring in at the default \
             tempo (one quarter = one beat). At most 400 notes per voice – read long pieces a few dozen bars at a time. notes: true without bars = the whole piece.",
            schema(json!({ "doc": doc.clone(), "fromBar": bar.clone(), "toBar": bar.clone(), "voices": voice_picks.clone(),
                "notes": { "type": "boolean", "description": "Notes without fromBar/toBar: of the whole piece" } }), &["doc"]), true),
        tool("import_library_score", "Import from library",
            "Brings parts of a library score into the project as instrument tracks – one track per part (instrument) with one note clip holding all its voices \
             (divisi, piano staves), all ONE undo step (undo reverts it); split 'voice' = one track per voice instead. doc from search_library, browse_library or \
             search_melody – no file path needed, works in the browser and for the person's own scores. voices: numbers or names from read_library_score (a part \
             name takes all its voices; default all). fromBar/toBar: only those bars (as printed, 0 = pickup; one of them alone = from the start / to the end). \
             startBeats: where the clips begin (default 0). trackIds: per track to be made (part, or voice with split 'voice'; in the order of voices) an existing \
             instrument track that gets it as an additional clip, 0 = new track – e.g. the next movement after the first on the same tracks, with startBeats = \
             endBeats of the earlier import. tempo: quarters per minute it should sound at under the project tempo (the notes are stretched); default one quarter \
             = one beat (x/8: one eighth = one beat). Parts without notes in the range are left out (skipped). Returns per track the voices in it, trackId, \
             clipId, the track's name and the note count, and endBeats.",
            schema(json!({ "doc": doc, "voices": voice_picks, "fromBar": bar.clone(), "toBar": bar,
                "split": { "enum": ["part", "voice"], "description": "part (default): one track per part with all its voices; voice: one track per voice" },
                "startBeats": { "type": "number", "minimum": 0, "description": "Start of the clips in beats (default 0)" },
                "trackIds": { "type": "array", "items": { "type": "integer", "minimum": 0 }, "description": "Per track to be made (part or voice, in the order of voices) an existing instrument track, 0 = new track" },
                "tempo": { "type": "number", "minimum": 10, "maximum": 400, "description": "Quarters per minute at the project tempo; default quarter = beat" },
                "ifRevision": { "type": "integer", "description": "Only if the project is still at this revision" } }), &["doc"]), false),
    ]
}

fn parse_kind(a: &Value) -> Result<InstrumentKind, String> {
    let k = a.get("kind").cloned().ok_or("Argument \"kind\" is missing (pluck, drums, bass, organ, harpsichord or synth)")?;
    serde_json::from_value(k).map_err(|_| "\"kind\" must be pluck, drums, bass, organ, harpsichord or synth".to_string())
}

/// Analysis mode (mono, poly – also German); missing = mono.
fn mode_arg(a: &Value) -> Result<crate::voices::AnalysisMode, String> {
    match a.get("mode") {
        None | Some(Value::Null) => Ok(Default::default()),
        Some(v) => serde_json::from_value(v.clone()).map_err(|_| "\"mode\" must be 'mono' or 'poly'".to_string()),
    }
}

/// Voices: "auto" (or missing) or 1–6.
fn voices_arg(a: &Value) -> Result<klang_dsp::poly::VoiceCount, String> {
    crate::voices::parse_voices(a.get("voices").unwrap_or(&Value::Null))
}

/// Smoothing strength (off, light, medium, strong – also German); missing = medium.
fn strength_arg(a: &Value, k: &str) -> Result<klang_core::smooth::Strength, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(Default::default()),
        Some(v) => serde_json::from_value(v.clone()).map_err(|_| format!("\"{k}\" must be 'off', 'light', 'medium' or 'strong'")),
    }
}

fn u64_arg(a: &Value, k: &str) -> Result<u64, String> {
    a.get(k).and_then(Value::as_u64).ok_or_else(|| format!("Argument \"{k}\" is missing or not a number"))
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(Value::as_str).ok_or_else(|| format!("Argument \"{k}\" is missing"))
}

/// Reads the chords from `chords` (symbols or scale degrees).
fn chords_of(a: &Value, key: &Key) -> Result<Vec<kh::Chord>, String> {
    let Some(list) = a.get("chords").and_then(Value::as_array) else { return Ok(Vec::new()) };
    list.iter()
        .map(|v| {
            let s = v.as_str().ok_or("chords: only strings like \"G7\" or \"V7\"")?;
            kh::parse_chord(s, key).ok_or_else(|| {
                format!("Chord \"{s}\" not understood – use symbols with German note names (C, a, G7, fis°, Hø7, C/E) or scale degrees (I, ii6, V7, V7/V, N6)")
            })
        })
        .collect()
}

/// Sound of new fugue voices (default organ).
fn instrument_of(a: &Value) -> InstrumentKind {
    match a.get("instrument").and_then(Value::as_str) {
        Some("harpsichord") => InstrumentKind::Harpsichord,
        Some("pluck") => InstrumentKind::Pluck,
        _ => InstrumentKind::Organ,
    }
}

fn chord_json(i: &kh::ChordInfo) -> Value {
    json!({ "name": i.name, "stufe": i.roman, "funktion": i.function, "rolle": i.role })
}

thread_local! {
    /// Short descriptions of the changes of the running tool call (`change` in
    /// the response) – per thread, because the desktop serves several clients at once.
    static CHANGES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Track and clip IDs and the revision before a tool call.
struct IdState {
    revision: u64,
    tracks: Vec<u64>,
    clips: Vec<(u64, u64)>,
}

/// `client` of `chat_poll`/`chat_reply`: short, printable, default "agent".
fn client_arg(a: &Value) -> String {
    let c: String = a.get("client").and_then(Value::as_str).unwrap_or("").trim().chars().filter(|c| !c.is_control()).take(40).collect();
    if c.is_empty() { "agent".into() } else { c }
}

fn is_placeholder(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next() == Some('$')
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replaces placeholders (`"$bass"`) in ID fields of a command by the real IDs.
fn resolve_ids(v: &mut Value, refs: &BTreeMap<String, u64>) -> Result<(), String> {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                match x {
                    Value::String(s) if matches!(k.as_str(), "id" | "trackId" | "clipId" | "groupId" | "group") => {
                        let id = refs.get(s.as_str()).ok_or_else(|| {
                            if s.starts_with('$') { format!("Unknown placeholder {s} – declare it first with \"ref\": \"{s}\" in an addTrack/addClip/addGroup") } else { format!("{k} must be a number or a placeholder like \"$bass\"") }
                        })?;
                        *x = json!(id);
                    }
                    _ => resolve_ids(x, refs)?,
                }
            }
            Ok(())
        }
        Value::Array(a) => a.iter_mut().try_for_each(|x| resolve_ids(x, refs)),
        _ => Ok(()),
    }
}

/// In the args of a `batch` call: `"$0.trackId"` → field of the result of call 0,
/// `"$bass"` → placeholder of an earlier execute (inside an execute it resolves itself,
/// it may declare it only now).
fn resolve_results(v: &mut Value, results: &[Value], refs: &BTreeMap<String, u64>, in_execute: bool) -> Result<(), String> {
    match v {
        Value::String(s) if s.starts_with('$') => {
            let (head, path) = s[1..].split_once('.').map_or((&s[1..], None), |(h, p)| (h, Some(p)));
            if let (Ok(n), Some(path)) = (head.parse::<usize>(), path) {
                let mut cur = results.get(n).ok_or_else(|| format!("{s}: there is no result {n} yet (calls count from 0)"))?;
                for part in path.split('.') {
                    cur = match part.parse::<usize>() {
                        Ok(i) => cur.get(i),
                        Err(_) => cur.get(part),
                    }
                    .ok_or_else(|| format!("{s}: the result of call {n} has no \"{part}\""))?;
                }
                *v = cur.clone();
            } else if let Some(id) = refs.get(s.as_str()) {
                *v = json!(id);
            } else if !in_execute && is_placeholder(s) {
                return Err(format!("Unknown placeholder {s}"));
            }
            Ok(())
        }
        Value::Object(o) => o.values_mut().try_for_each(|x| resolve_results(x, results, refs, in_execute)),
        Value::Array(a) => a.iter_mut().try_for_each(|x| resolve_results(x, results, refs, in_execute)),
        _ => Ok(()),
    }
}


impl<S: AgentShell> Agent<S> {
    fn st(&self) -> &AppState {
        self.shell.state()
    }

    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        self.st().host().emit(event, payload);
    }

    /// Key from the `key` argument, otherwise estimated from the notes.
    fn key_of(&self, args: &Value) -> Result<KeyInfo, String> {
        let chosen = match args.get("key").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            Some(s) => Some(kh::parse_key(s).ok_or_else(|| format!("Key \"{s}\" not understood – e.g. \"C-Dur\" (C major), \"a-Moll\" (A minor), \"Es-Dur\" (E♭ major)"))?),
            None => None,
        };
        harmony::resolve_key(&self.st(), chosen)
    }

    /// Informs the UI about changes and activity of the agent; `summary` also
    /// goes into the tool's response (`change`).
    fn changed(&self, tool: &str, summary: impl Into<String>) {
        if let Ok(s) = self.st().snapshot() {
            self.emit("project-changed", &s);
        }
        let summary = summary.into();
        CHANGES.with(|c| c.borrow_mut().push(summary.clone()));
        self.activity(tool, summary);
    }

    fn activity(&self, tool: &str, summary: impl Into<String>) {
        self.emit("agent-activity", &json!({ "tool": tool, "summary": summary.into() }));
    }

    fn project_summary(&self, include_notes: bool) -> Result<Value, String> {
        self.project_view(include_notes, None, None)
    }

    /// The project for the agent; `track_ids` / `fields` read only a part (see `get_project`).
    fn project_view(&self, include_notes: bool, track_ids: Option<&[u64]>, fields: Option<&[String]>) -> Result<Value, String> {
        let st = self.st();
        let status = st.status();
        let s = lock(&st.session)?;
        let p = &s.project;
        if let Some(ids) = track_ids {
            if let Some(missing) = ids.iter().find(|id| p.track(Id(**id)).is_none()) {
                return Err(format!("Track {missing} doesn't exist"));
            }
        }
        let mut v = serde_json::to_value(p).map_err(|e| e.to_string())?;
        if let Some(obj) = v.as_object_mut() {
            obj.remove("nextId");
        }
        for (ti, t) in p.tracks.iter().enumerate() {
            for (ci, c) in t.clips.iter().enumerate() {
                let cv = &mut v["tracks"][ti]["clips"][ci];
                cv["lengthBeats"] = json!(c.length_beats(p.tempo_bpm));
                if let ClipSource::Notes { notes, .. } = &c.source {
                    cv["source"]["noteCount"] = json!(notes.len());
                    if !include_notes {
                        cv["source"].as_object_mut().map(|o| o.remove("notes"));
                    }
                }
            }
        }
        v["undo"] = json!(s.undo_label());
        v["redo"] = json!(s.redo_label());
        v["playing"] = json!(status.playing);
        v["positionBeats"] = json!((status.position_secs * p.tempo_bpm / 60.0 * 100.0).round() / 100.0);
        v["revision"] = json!(s.revision());
        drop(s);
        if let (Some(ids), Some(tracks)) = (track_ids, v.get_mut("tracks").and_then(Value::as_array_mut)) {
            tracks.retain(|t| t["id"].as_u64().is_some_and(|id| ids.contains(&id)));
        }
        if let (Some(fields), Some(obj)) = (fields, v.as_object_mut()) {
            let track_fields: Vec<&str> = fields.iter().filter_map(|f| f.strip_prefix("tracks.")).collect();
            let keep_tracks = !track_fields.is_empty() || fields.iter().any(|f| f == "tracks");
            obj.retain(|k, _| k == "revision" || (k == "tracks" && keep_tracks) || fields.iter().any(|f| f == k));
            if !track_fields.is_empty() && !fields.iter().any(|f| f == "tracks") {
                for t in obj.get_mut("tracks").and_then(Value::as_array_mut).into_iter().flatten() {
                    if let Some(t) = t.as_object_mut() {
                        t.retain(|k, _| k == "id" || track_fields.contains(&k.as_str()));
                    }
                }
            }
        }
        Ok(v)
    }

    /// `execute`: commands as one undo step. Placeholders (`ref`) are given real
    /// IDs before; `refs` collects them (shared by the calls of a `batch`).
    fn execute(&self, args: &Value, refs: &mut BTreeMap<String, u64>) -> Result<Value, String> {
        let list = args.get("commands").and_then(Value::as_array).ok_or("Argument \"commands\" (list) is missing")?;
        if list.is_empty() {
            return Err("\"commands\" is empty".into());
        }
        let if_revision = args.get("ifRevision").and_then(Value::as_u64);
        let declared_before: Vec<String> = refs.keys().cloned().collect();
        let mut commands = Vec::with_capacity(list.len());
        for (i, c) in list.iter().enumerate() {
            let mut c = c.clone();
            let err = |e: String| format!("Command {}: {e}", i + 1);
            crate::notation::expand(&mut c).map_err(err)?;
            self.declare_refs(&mut c, refs).map_err(err)?;
            resolve_ids(&mut c, refs).map_err(err)?;
            let cmd: Command = serde_json::from_value(c).map_err(|e| err(e.to_string()))?;
            commands.push(cmd);
        }
        let new_ids: BTreeMap<&String, &u64> = refs.iter().filter(|(k, _)| !declared_before.contains(k)).collect();
        if commands.iter().all(Command::is_view_only) {
            // Pure view changes: run individually, no undo step.
            if let Some(want) = if_revision {
                let now = self.st().revision()?;
                if now != want {
                    return Err(format!("The project was changed in the meantime (revision {now} instead of {want}) – read it again with get_project and retry."));
                }
            }
            for c in commands {
                self.st().execute(c, false)?;
            }
            self.changed("execute", tr!("View changed"));
            return Ok(json!({ "ok": true, "undoLabel": null, "project": self.project_summary(false)? }));
        }
        let label = match args.get("label").and_then(Value::as_str) {
            Some(l) if !l.trim().is_empty() => l.trim().to_string(),
            _ if commands.len() == 1 => commands[0].label(),
            _ => trn!(commands.len(), "{n} change", "{n} changes"),
        };
        let label = tr!("Agent · {label}", label = label);
        let snap = self.st().execute_if(Command::Batch { label: label.clone(), commands }, if_revision)?;
        drop(snap);
        self.changed("execute", label.clone());
        let mut out = json!({ "ok": true, "undoLabel": label, "project": self.project_summary(false)? });
        if !new_ids.is_empty() {
            out["ids"] = json!(new_ids);
        }
        Ok(out)
    }

    /// Gives every object with `"ref": "$name"` (new track or clip) a real ID
    /// and remembers it in `refs`; new tracks and clips without `id` get 0 (= new).
    fn declare_refs(&self, v: &mut Value, refs: &mut BTreeMap<String, u64>) -> Result<(), String> {
        let new_object = |o: &mut serde_json::Map<String, Value>| {
            o.entry("id").or_insert(json!(0));
        };
        if let Some(o) = v.as_object_mut() {
            match o.get("type").and_then(Value::as_str) {
                Some("addTrack") => {
                    if let Some(t) = o.get_mut("track").and_then(Value::as_object_mut) {
                        new_object(t);
                        for c in t.get_mut("clips").and_then(Value::as_array_mut).into_iter().flatten().filter_map(Value::as_object_mut) {
                            new_object(c);
                            c.entry("name").or_insert(json!("Clip"));
                        }
                    }
                }
                Some("addClip") => {
                    if let Some(c) = o.get_mut("clip").and_then(Value::as_object_mut) {
                        new_object(c);
                        c.entry("name").or_insert(json!("Clip"));
                    }
                }
                Some("addGroup") => {
                    if let Some(g) = o.get_mut("group").and_then(Value::as_object_mut) {
                        new_object(g);
                    }
                }
                _ => {}
            }
        }
        self.declare_in(v, refs)
    }

    fn declare_in(&self, v: &mut Value, refs: &mut BTreeMap<String, u64>) -> Result<(), String> {
        match v {
            Value::Object(o) => {
                if let Some(r) = o.remove("ref") {
                    let name = r.as_str().filter(|s| is_placeholder(s)).ok_or_else(|| format!("ref must look like \"$bass\" (got {r})"))?.to_string();
                    if refs.contains_key(&name) {
                        return Err(format!("Placeholder {name} is already used"));
                    }
                    let id = lock(&self.st().session)?.project.alloc_id();
                    o.insert("id".into(), json!(id.0));
                    refs.insert(name, id.0);
                }
                o.values_mut().try_for_each(|x| self.declare_in(x, refs))
            }
            Value::Array(a) => a.iter_mut().try_for_each(|x| self.declare_in(x, refs)),
            _ => Ok(()),
        }
    }

    /// `batch`: several tool calls as one undo step; at the first error
    /// everything before it is reverted.
    fn batch(&self, args: &Value) -> Result<Value, String> {
        let calls = args.get("calls").and_then(Value::as_array).ok_or("Argument \"calls\" (list of {tool, args}) is missing")?;
        if calls.is_empty() || calls.len() > 50 {
            return Err("\"calls\" needs 1 to 50 entries".into());
        }
        let st = self.st();
        if let Some(want) = args.get("ifRevision").and_then(Value::as_u64) {
            let now = st.revision()?;
            if now != want {
                return Err(format!("The project was changed in the meantime (revision {now} instead of {want}) – read it again with get_project and retry."));
            }
        }
        let mark = st.undo_mark()?;
        let mut refs = BTreeMap::new();
        let mut results: Vec<Value> = Vec::with_capacity(calls.len());
        for (i, call) in calls.iter().enumerate() {
            let tool = call.get("tool").and_then(Value::as_str).unwrap_or_default();
            let mut run = || -> Result<Value, String> {
                if matches!(tool, "batch" | "chat_poll" | "undo" | "redo" | "open_project") || HISTORY_MOVES.contains(&tool) {
                    return Err(format!("{tool} is not allowed inside batch"));
                }
                if !Handler::tools(self).iter().any(|t| t.name == tool) {
                    return Err(format!("Unknown tool \"{tool}\""));
                }
                let mut a = call.get("args").cloned().unwrap_or_else(|| json!({}));
                if !a.is_object() {
                    return Err("args must be an object".into());
                }
                resolve_results(&mut a, &results, &refs, tool == "execute")?;
                let mut r = if tool == "execute" { self.execute(&a, &mut refs)? } else { self.dispatch(tool, a)? };
                if let Some(o) = r.as_object_mut() {
                    o.remove("project");
                }
                Ok(r)
            };
            match run() {
                Ok(r) => results.push(r),
                Err(e) => {
                    let reverted = st.rollback_steps(mark.clone()).map(|s| self.emit("project-changed", &s)).is_ok();
                    CHANGES.with(|c| c.borrow_mut().clear());
                    return Err(if reverted {
                        format!("Call {} ({tool}) failed: {e}\nThe project is unchanged – the calls before it were reverted.", i + 1)
                    } else {
                        format!("Call {} ({tool}) failed: {e}\nReverting the calls before it failed as well – check with get_project.", i + 1)
                    });
                }
            }
        }
        let label = match args.get("label").and_then(Value::as_str) {
            Some(l) if !l.trim().is_empty() => l.trim().to_string(),
            _ => trn!(calls.len(), "{n} step", "{n} steps"),
        };
        let label = tr!("Agent · {label}", label = label);
        if st.squash_steps(&mark, label.clone())? {
            if let Ok(s) = st.snapshot() {
                self.emit("project-changed", &s);
            }
        }
        let mut out = json!({ "ok": true, "undoLabel": label, "results": results });
        if !refs.is_empty() {
            out["ids"] = json!(refs);
        }
        Ok(out)
    }

    /// Track and clip IDs and the revision – to report what a tool changed.
    fn id_state(&self) -> Option<IdState> {
        let s = lock(&self.st().session).ok()?;
        Some(IdState {
            revision: s.revision(),
            tracks: s.project.tracks.iter().map(|t| t.id.0).collect(),
            clips: s.project.tracks.iter().flat_map(|t| t.clips.iter().map(move |c| (t.id.0, c.id.0))).collect(),
        })
    }

    /// Adds to a write tool's response what changed: new `revision`, `change`
    /// (short description), `created` / `removed` IDs.
    fn report_changes(&self, v: &mut Value, before: &IdState) {
        let Some(after) = self.id_state() else { return };
        let summary = CHANGES.with(|c| std::mem::take(&mut *c.borrow_mut()));
        let Some(o) = v.as_object_mut() else { return };
        if after.revision == before.revision {
            return;
        }
        o.insert("revision".into(), json!(after.revision));
        if !summary.is_empty() {
            o.insert("change".into(), json!(summary.join("; ")));
        }
        let diff = |a: &IdState, b: &IdState| {
            let tracks: Vec<u64> = b.tracks.iter().filter(|t| !a.tracks.contains(t)).copied().collect();
            let clips: Vec<Value> = b.clips.iter().filter(|c| !a.clips.contains(c)).map(|(t, c)| json!({ "trackId": t, "clipId": c })).collect();
            let mut m = serde_json::Map::new();
            if !tracks.is_empty() {
                m.insert("tracks".into(), json!(tracks));
            }
            if !clips.is_empty() {
                m.insert("clips".into(), json!(clips));
            }
            m
        };
        let created = diff(before, &after);
        if !created.is_empty() {
            o.insert("created".into(), Value::Object(created));
        }
        let removed = diff(&after, before);
        if !removed.is_empty() {
            o.insert("removed".into(), Value::Object(removed));
        }
    }

    fn chat_peek(&self, args: &Value) -> Value {
        let chat = &self.st().chat;
        let since = args.get("sinceId").and_then(Value::as_u64).unwrap_or(0);
        let mut msgs = chat.peek(since);
        let keep = if since == 0 { 20 } else { 50 };
        if msgs.len() > keep {
            msgs.drain(..msgs.len() - keep);
        }
        let status = |s: Option<crate::chat::Status>| match s {
            Some(crate::chat::Status::Neu) => Some("new"),
            Some(crate::chat::Status::InArbeit) => Some("inProgress"),
            Some(crate::chat::Status::Erledigt) => Some("done"),
            Some(crate::chat::Status::Abgebrochen) => Some("cancelled"),
            None => None,
        };
        let messages: Vec<Value> = msgs
            .iter()
            .map(|m| {
                let mut j = json!({ "id": m.id, "von": if m.from == crate::chat::Author::User { "user" } else { "agent" }, "text": m.text });
                if let Some(s) = status(m.status) {
                    j["status"] = json!(s);
                }
                if let Some(c) = &m.claimed_by {
                    j["claimedBy"] = json!(c);
                }
                if let Some(r) = m.reply_to {
                    j["replyTo"] = json!(r);
                }
                if m.cancel_requested {
                    j["cancelRequested"] = json!(true);
                }
                if !m.cancels.is_empty() {
                    j["cancels"] = json!(m.cancels);
                }
                j
            })
            .collect();
        json!({ "messages": messages, "lastId": chat.last_id() })
    }

    fn set_sound(&self, args: &Value) -> Result<Value, String> {
        let track_id = Id(u64_arg(args, "trackId")?);
        let track = self.st().instrument_track(track_id)?;
        let empty = serde_json::Map::new();
        let given = args.get("params").and_then(Value::as_object).unwrap_or(&empty);
        let mut inst = track.instrument.clone().unwrap_or_else(Instrument::pluck);
        let mut values = std::collections::BTreeMap::new();
        for (k, v) in given {
            let x = v.as_f64().ok_or_else(|| format!("\"{k}\" must be a number"))? as f32;
            values.insert(k.clone(), x);
        }
        let mut values = sound::validate_params(inst.kind, &values)?;
        // Dot in the character field: first the mapping, then individually named controls on top
        let cx = values.remove("charakter.x");
        let cy = values.remove("charakter.y");
        if cx.is_some() || cy.is_some() {
            let x = cx.unwrap_or_else(|| inst.param("charakter.x", 0.5));
            let y = cy.unwrap_or_else(|| inst.param("charakter.y", 0.5));
            sound::apply_field(&mut inst, "charakter", x, y)?;
        }
        if let Some(name) = args.get("verlauf").and_then(Value::as_str) {
            sound::apply_verlauf(&mut inst, name)?;
        }
        if let Some(c) = args.get("curves") {
            if inst.kind != InstrumentKind::Synth {
                return Err("Curves only exist for the synth.".into());
            }
            let curves: std::collections::BTreeMap<String, Vec<[f32; 2]>> =
                serde_json::from_value(c.clone()).map_err(|_| "curves: { laut: [[second, value], …], hell: […] }".to_string())?;
            inst.curves.extend(sound::validate_curves(&curves)?);
        }
        if values.is_empty() && cx.is_none() && cy.is_none() && args.get("verlauf").is_none() && args.get("curves").is_none() {
            return Err("Nothing to change – give params, verlauf or curves.".into());
        }
        inst.params.extend(values);
        inst.preset = args.get("preset").and_then(Value::as_str).unwrap_or("agent").to_string();
        let label = tr!("Agent · {label}", label = tr!("Change sound"));
        let cmd = Command::Batch {
            label: label.clone(),
            commands: vec![Command::SetTrack { track_id, patch: TrackPatch { instrument: Some(inst), ..Default::default() } }],
        };
        self.st().execute(cmd, false)?;
        let report = self.st().describe_track(track_id)?;
        let words: Vec<String> = report.anzeige.iter().map(|m| format!("{} {}", m.label, m.wort)).collect();
        self.changed("set_sound", tr!("Sound \"{name}\": {words}", name = track.name, words = words.join(", ")));
        let mut out = serde_json::to_value(&report).map_err(|e| e.to_string())?;
        if let Some(p) = args.get("preview").and_then(Value::as_str) {
            out["vorgespielt"] = json!(self.st().preview_track(track_id, p)?);
        }
        Ok(out)
    }

    fn search(&self, args: &Value) -> Result<Value, String> {
        let st = self.st();
        let query: Vec<MelodyNote> = if let Some(notes) = args.get("notes").and_then(Value::as_array) {
            let mut t = 0.0;
            notes
                .iter()
                .map(|n| {
                    let midi = n.get("midi").and_then(Value::as_u64).ok_or("Every note needs \"midi\"")? as u8;
                    let d = n.get("duration").and_then(Value::as_f64).ok_or("Every note needs \"duration\"")?;
                    let note = MelodyNote { onset: t, duration: d, midi };
                    t += d;
                    Ok(note)
                })
                .collect::<Result<_, &str>>()?
        } else {
            let (_, a) = st.clip_analysis(Id(u64_arg(args, "trackId")?), Id(u64_arg(args, "clipId")?))?;
            a.notes.iter().map(|n| MelodyNote { onset: n.start_secs, duration: n.end_secs - n.start_secs, midi: n.midi }).collect()
        };
        if query.len() < MIN_QUERY_NOTES {
            return Err(format!("At least {MIN_QUERY_NOTES} notes needed (got: {}).", query.len()));
        }
        let lib = lock(&st.library)?;
        let lib = lib.as_ref().ok_or("No library loaded – it is loaded at startup; check with library_status.")?;
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
        let hits = lib.index.search(&query, limit);
        self.activity("search_melody", trn!(hits.len(), "Melody search: {n} match", "Melody search: {n} matches"));
        Ok(json!({ "query": query.iter().map(|n| note_name(n.midi)).collect::<Vec<_>>(), "hits": hits }))
    }
}

// ---------------------------------------------------------------- Library scores

/// A doc (voice entry of the library) from an argument.
fn doc_arg(doc: u64) -> Result<u32, String> {
    u32::try_from(doc).map_err(|_| format!("doc {doc} doesn't exist"))
}

/// fromBar/toBar: one of them alone = from the start / to the end.
fn bars_arg(a: &Value) -> Result<Option<(u32, u32)>, String> {
    let bar = |k: &str| match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().and_then(|n| u32::try_from(n).ok()).map(Some).ok_or_else(|| format!("\"{k}\" must be a bar number (0 = pickup)")),
    };
    Ok(match (bar("fromBar")?, bar("toBar")?) {
        (None, None) => None,
        (from, to) => Some((from.unwrap_or(0), to.unwrap_or(u32::MAX))),
    })
}

/// voices: numbers (1-based) or names.
fn voice_picks(a: &Value) -> Result<Vec<crate::library::VoicePick>, String> {
    use crate::library::VoicePick;
    let Some(list) = a.get("voices").filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let list = list.as_array().ok_or("\"voices\" must be a list of voice numbers or names")?;
    list.iter()
        .map(|v| match v {
            Value::Number(n) => n.as_u64().map(|n| VoicePick::Number(n as usize)).ok_or_else(|| format!("voice {n}: a number from 1")),
            Value::String(s) => Ok(s.trim().parse::<usize>().map(VoicePick::Number).unwrap_or_else(|_| VoicePick::Name(s.clone()))),
            other => Err(format!("voice {other}: a number or a name")),
        })
        .collect()
}

impl<S: AgentShell> Agent<S> {
    fn import_library_score(&self, args: &Value) -> Result<Value, String> {
        let st = self.st();
        let doc = doc_arg(u64_arg(args, "doc")?)?;
        let (info, score) = crate::library::doc_and_score(st, doc)?;
        let voices = crate::library::pick_voices(&score, &voice_picks(args)?)?;
        let range = bars_arg(args)?.map(|(a, b)| crate::library::bars_range(&score, a, b)).transpose()?;
        let targets: Vec<Option<Id>> = match args.get("trackIds").filter(|v| !v.is_null()) {
            None => Vec::new(),
            Some(t) => t.as_array().ok_or("\"trackIds\" must be a list of track IDs")?.iter().map(|v| v.as_u64().filter(|&id| id > 0).map(Id)).collect(),
        };
        let by_part = match args.get("split").and_then(Value::as_str) {
            None | Some("part") => true,
            Some("voice") => false,
            Some(other) => return Err(format!("\"split\" must be 'part' or 'voice', not '{other}'")),
        };
        let req = crate::library::ImportRequest {
            voices,
            by_part,
            range,
            tempo: args.get("tempo").and_then(Value::as_f64),
            start_beats: args.get("startBeats").and_then(Value::as_f64).unwrap_or(0.0),
            targets,
        };
        let done = crate::library::import_voices(st, &info, &score, &req, args.get("ifRevision").and_then(Value::as_u64))?;
        let v = serde_json::to_value(&done).map_err(|e| e.to_string())?;
        let n = v["tracks"].as_array().map_or(0, Vec::len);
        self.changed("import_library_score", if n > 1 { trn!(n, "Import {n} voice from library", "Import {n} voices from library") } else { tr!("Import from library") });
        Ok(v)
    }
}

// ---------------------------------------------------------------- Change history

/// History tools that move or mark the history – not inside `batch`, whose
/// squash and rollback only know the steps the batch made itself.
const HISTORY_MOVES: [&str; 5] = ["checkpoint", "goto", "variant_new", "variant_switch", "combine"];

/// Same text, ignoring case and surrounding spaces.
fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn variant_names(h: &History) -> String {
    h.variants().iter().map(|x| format!("{} \"{}\"", x.id, translate(&x.name))).collect::<Vec<_>>().join(", ")
}

/// A variant from an argument: its ID or its name (as shown or as stored).
fn variant_arg(h: &History, v: &Value) -> Result<u32, String> {
    let id = match v {
        Value::Number(n) => n.as_u64().and_then(|x| u32::try_from(x).ok()),
        Value::String(s) => s.trim().parse::<u32>().ok().or_else(|| h.variants().iter().find(|x| same_name(&translate(&x.name), s) || same_name(&x.name, s)).map(|x| x.id)),
        _ => None,
    };
    match id {
        Some(id) if h.variant(id).is_some() => Ok(id),
        _ => Err(format!("Variant {v} doesn't exist – the variants are: {}", variant_names(h))),
    }
}

/// The step of a checkpoint given by ID or name.
fn checkpoint_arg(h: &History, v: &Value) -> Result<NodeId, String> {
    let by_id = |id: u64| h.checkpoints().iter().find(|c| u64::from(c.id) == id);
    let found = match v {
        Value::Number(n) => n.as_u64().and_then(by_id),
        Value::String(s) => h.checkpoints().iter().find(|c| same_name(&c.name, s)).or_else(|| s.trim().parse::<u64>().ok().and_then(by_id)),
        _ => None,
    };
    found.map(|c| c.node).ok_or_else(|| {
        let all: Vec<String> = h.checkpoints().iter().map(|c| format!("{} \"{}\"", c.id, c.name)).collect();
        if all.is_empty() { format!("Checkpoint {v} doesn't exist – there are none yet") } else { format!("Checkpoint {v} doesn't exist – the checkpoints are: {}", all.join(", ")) }
    })
}

/// `step` or `checkpoint` from the arguments; `None` if neither is given.
fn step_arg(h: &History, a: &Value) -> Result<Option<NodeId>, String> {
    match (a.get("step").filter(|v| !v.is_null()), a.get("checkpoint").filter(|v| !v.is_null())) {
        (Some(_), Some(_)) => Err("Give step or checkpoint, not both".into()),
        (Some(s), None) => {
            let id = s.as_u64().and_then(|x| NodeId::try_from(x).ok()).ok_or("step must be a step ID from history")?;
            if h.node(id).is_none() {
                return Err(format!("Step {id} is not in the history – history lists the steps"));
            }
            Ok(Some(id))
        }
        (None, Some(c)) => checkpoint_arg(h, c).map(Some),
        (None, None) => Ok(None),
    }
}

/// Where the project stands in the history: step, its label, the active variant.
fn position(h: &History) -> Value {
    let cur = h.current();
    let v = h.active();
    let mut o = json!({ "step": cur, "variant": { "id": v.id, "name": translate(&v.name) } });
    if let Some(n) = h.node(cur) {
        o["label"] = json!(translate(&n.label));
        o["tracks"] = json!(n.metrics.tracks);
        o["notes"] = json!(n.metrics.notes);
    }
    o
}

/// The history table for the agent: the rows around the current step; tempo,
/// meter and key only where they change, flags only when set.
fn history_table(h: &History, limit: usize) -> Value {
    let v = crate::history::view(h);
    let n = v.rows.len();
    let limit = limit.clamp(1, 1000);
    let cur = v.rows.iter().position(|r| r.current).unwrap_or(n.saturating_sub(1));
    let (start, end) = if n <= limit {
        (0, n)
    } else if cur + limit / 2 >= n {
        (n - limit, n)
    } else {
        let s = cur.saturating_sub(limit / 2);
        (s, (s + limit).min(n))
    };
    let mut last: Option<(f64, String, Option<String>)> = None;
    let steps: Vec<Value> = v.rows[start..end]
        .iter()
        .map(|r| {
            let m = &r.metrics;
            let mut o = json!({ "step": r.id, "label": r.label, "tracks": m.tracks, "notes": m.notes, "beats": m.length_beats });
            if !r.detail.is_empty() {
                o["detail"] = json!(r.detail);
            }
            let meter = format!("{}/{}", m.beats_per_bar, m.beat_unit);
            let (tempo_new, meter_new, key_new) = match &last {
                None => (true, true, m.key.is_some()),
                Some(p) => (p.0 != m.tempo, p.1 != meter, m.key.is_some() && p.2 != m.key),
            };
            if tempo_new {
                o["tempo"] = json!(m.tempo);
            }
            if meter_new {
                o["meter"] = json!(meter);
            }
            if key_new {
                o["key"] = json!(m.key);
            }
            last = Some((m.tempo, meter, m.key.clone()));
            if r.current {
                o["current"] = json!(true);
            }
            if r.future {
                o["redo"] = json!(true);
            }
            if !r.checkpoints.is_empty() {
                o["checkpoints"] = json!(r.checkpoints.iter().map(|c| &c.name).collect::<Vec<_>>());
            }
            if !r.forks.is_empty() {
                let branches: Vec<Value> = r
                    .forks
                    .iter()
                    .map(|f| match &f.variant {
                        Some(x) => json!({ "step": f.node, "steps": f.steps, "variant": x.id, "name": x.name }),
                        // goto its tip brings the set-aside steps back as a variant
                        None => json!({ "step": f.node, "steps": f.steps, "tip": f.tip, "setAside": true }),
                    })
                    .collect();
                o["branches"] = json!(branches);
            }
            o
        })
        .collect();
    let mut out = json!({
        "current": v.current,
        "variant": v.variants.iter().find(|x| x.active).map(|x| json!({ "id": x.id, "name": x.name })),
        "steps": steps,
        "variants": v.variants.iter().map(|x| json!({ "id": x.id, "name": x.name, "head": x.head, "active": x.active })).collect::<Vec<_>>(),
        "checkpoints": v.checkpoints.iter().map(|c| {
            let mut o = json!({ "id": c.id, "name": c.name, "step": c.node });
            if !c.on_line {
                o["onLine"] = json!(false);
                if let Some(name) = &c.variant {
                    o["variant"] = json!(name);
                }
            }
            o
        }).collect::<Vec<_>>(),
        "total": v.total,
    });
    if start > 0 || end < n {
        out["omitted"] = json!({ "before": start, "after": n - end });
    }
    out
}

fn variant_list(h: &History) -> Value {
    let v = crate::history::view(h);
    let variants: Vec<Value> = v
        .variants
        .iter()
        .map(|x| json!({ "id": x.id, "name": x.name, "active": x.active, "head": x.head, "last": x.last, "fork": x.fork, "steps": x.steps }))
        .collect();
    json!({ "active": v.active, "current": v.current, "variants": variants })
}

/// Where a choice of `combine` goes.
enum Pick {
    Project,
    Track(String),
    Segment(String, f64),
}

/// One decision of combining, for the agent.
struct Decision {
    /// Key in `choices`: "tempo", "track 12", "track 12 bars 5-6".
    id: String,
    pick: Pick,
    by: Vec<Source>,
    conflict: bool,
    /// Track, bars, what changed.
    info: Value,
    /// The versions to choose from.
    options: Vec<Value>,
}

fn source_name(view: &MergeView, s: Source) -> String {
    if s == ORIGIN {
        return "origin".into();
    }
    view.variants.iter().find(|v| v.id == s).map(|v| v.name.clone()).unwrap_or_else(|| s.to_string())
}

fn fmt_beats(x: f64) -> String {
    let r = (x * 1000.0).round() / 1000.0;
    if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") }
}

/// The notes of one version of a track in [a, b), compact: "E4 1 @20 · G4 0.5 @21"
/// (pitch, length, beat in the song).
fn lane_notes(lane: Option<&crate::history::LaneDto>, a: f64, b: f64) -> String {
    const MAX: usize = 32;
    let Some(l) = lane else { return String::new() };
    if !l.present {
        return "(no track)".into();
    }
    let mut notes: Vec<&[f32; 3]> = l.notes.iter().filter(|n| f64::from(n[0]) >= a - 1e-6 && f64::from(n[0]) < b - 1e-6).collect();
    notes.sort_by(|x, y| x[0].total_cmp(&y[0]).then(x[2].total_cmp(&y[2])));
    let mut parts: Vec<String> = notes.iter().take(MAX).map(|n| format!("{} {} @{}", note_name(n[2] as u8), fmt_beats(f64::from(n[1])), fmt_beats(f64::from(n[0])))).collect();
    if notes.len() > MAX {
        parts.push(format!("… +{} more", notes.len() - MAX));
    }
    let audio = l.audio.iter().filter(|r| f64::from(r[0]) < b && f64::from(r[1]) > a).count();
    if audio > 0 {
        parts.push(format!("{audio} recording(s)"));
    }
    if parts.is_empty() { "(empty)".into() } else { parts.join(" · ") }
}

/// Every decision of a combination: tempo/time signature, per track the track
/// itself and each changed bar section.
fn decisions(view: &MergeView) -> Vec<Decision> {
    let name = |s: Source| source_name(view, s);
    let mut out = Vec::new();
    let p = &view.project;
    if !p.by.is_empty() {
        out.push(Decision {
            id: "tempo".into(),
            pick: Pick::Project,
            by: p.by.clone(),
            conflict: p.conflict,
            info: json!({ "what": "tempo/time signature" }),
            options: p.options.iter().map(|m| json!({ "variant": m.source, "name": name(m.source), "tempo": m.tempo, "meter": format!("{}/{}", m.beats_per_bar, m.beat_unit) })).collect(),
        });
    }
    let bpb = f64::from(view.beats_per_bar.max(1));
    for t in &view.tracks {
        let track = json!({ "track": t.name, "trackId": t.track_id });
        if !t.settings_by.is_empty() {
            let mut info = track.clone();
            info["what"] = json!(t.changes.iter().map(|c| format!("{}: {}", name(c.source), c.what)).collect::<Vec<_>>().join("; "));
            let mut options = vec![json!({ "variant": ORIGIN, "name": name(ORIGIN), "what": if t.in_origin { "as before" } else { "no track" } })];
            options.extend(t.changes.iter().map(|c| json!({ "variant": c.source, "name": name(c.source), "what": c.what })));
            out.push(Decision { id: format!("track {}", t.key), pick: Pick::Track(t.key.clone()), by: t.settings_by.clone(), conflict: t.settings_conflict, info, options });
        }
        for s in &t.segments {
            let (from, to) = ((s.start / bpb).round() as i64 + 1, (s.end / bpb).round() as i64);
            let bars = if to <= from { format!("bar {from}") } else { format!("bars {from}-{to}") };
            let mut info = track.clone();
            info["bars"] = json!(bars);
            info["beats"] = json!([s.start, s.end]);
            let options = std::iter::once(ORIGIN)
                .chain(s.by.iter().copied())
                .map(|src| json!({ "variant": src, "name": name(src), "notes": lane_notes(t.lanes.iter().find(|l| l.source == src), s.start, s.end) }))
                .collect();
            out.push(Decision { id: format!("track {} {bars}", t.key), pick: Pick::Segment(t.key.clone(), s.start), by: s.by.clone(), conflict: s.conflict, info, options });
        }
    }
    out
}

/// Decision IDs compare without case, extra spaces or the kind of dash.
fn norm_id(s: &str) -> String {
    s.to_lowercase().replace(['–', '—'], "-").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A version in `choices`: variant ID or name, 0 or "origin".
fn choice_source(view: &MergeView, v: &Value) -> Result<Source, String> {
    let s = match v {
        Value::Number(n) => n.as_u64().and_then(|x| u32::try_from(x).ok()),
        Value::String(s) if same_name(s, "origin") => Some(ORIGIN),
        Value::String(s) => s.trim().parse::<u32>().ok().or_else(|| view.variants.iter().find(|x| same_name(&x.name, s)).map(|x| x.id)),
        _ => None,
    };
    match s {
        Some(s) if s == ORIGIN || view.variants.iter().any(|x| x.id == s) => Ok(s),
        _ => Err(format!(
            "choices: {v} is not one of the variants being combined ({}) – or 0 = origin",
            view.variants.iter().map(|x| format!("{} \"{}\"", x.id, x.name)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The agent's `choices` as [`Choices`], and per decision the version chosen for it.
fn parse_choices(view: &MergeView, ds: &[Decision], given: Option<&Value>) -> Result<(Choices, Vec<Option<Source>>), String> {
    let mut picked: Vec<Option<Source>> = vec![None; ds.len()];
    let mut c = Choices::default();
    let Some(given) = given.filter(|v| !v.is_null()) else { return Ok((c, picked)) };
    let map = given.as_object().ok_or("choices must be an object {decision id: variant}")?;
    let mut all = None;
    for (k, v) in map {
        let s = choice_source(view, v)?;
        if k.trim() == "*" {
            all = Some(s);
            continue;
        }
        let i = ds.iter().position(|d| norm_id(&d.id) == norm_id(k)).ok_or_else(|| {
            let ids: Vec<&str> = ds.iter().map(|d| d.id.as_str()).collect();
            if ids.is_empty() { format!("choices: there is nothing to decide (\"{k}\")") } else { format!("choices: unknown decision \"{k}\" – the decisions are: {}", ids.join(", ")) }
        })?;
        picked[i] = Some(s);
    }
    if let Some(s) = all {
        for (i, d) in ds.iter().enumerate() {
            if picked[i].is_none() && d.conflict && (s == ORIGIN || d.by.contains(&s)) {
                picked[i] = Some(s);
            }
        }
    }
    for (d, s) in ds.iter().zip(&picked) {
        let Some(s) = *s else { continue };
        match &d.pick {
            Pick::Project => c.project = Some(s),
            Pick::Track(k) => {
                c.tracks.insert(k.clone(), s);
            }
            Pick::Segment(k, start) => c.segments.push(SegmentChoice { key: k.clone(), start: *start, source: s }),
        }
    }
    Ok((c, picked))
}

impl<S: AgentShell> Agent<S> {
    fn with_history<T>(&self, f: impl FnOnce(&History) -> T) -> Result<T, String> {
        Ok(f(lock(&self.st().session)?.history()))
    }

    /// `combine`: what combining means; with a choice for every conflict
    /// (and without dryRun) the new variant.
    fn combine(&self, args: &Value) -> Result<Value, String> {
        let st = self.st();
        let (base, others) = self.with_history(|h| -> Result<(u32, Vec<u32>), String> {
            let base = match args.get("base").filter(|v| !v.is_null()) {
                Some(v) => variant_arg(h, v)?,
                None => h.active().id,
            };
            let given = match args.get("others") {
                Some(Value::Array(a)) => a.clone(),
                Some(v) if !v.is_null() => vec![v.clone()],
                _ => Vec::new(),
            };
            let mut others = Vec::new();
            for v in &given {
                let id = variant_arg(h, v)?;
                if id != base && !others.contains(&id) {
                    others.push(id);
                }
            }
            if others.is_empty() {
                return Err(format!("Give others: the variants to bring into variant {base} – the variants are: {}", variant_names(h)));
            }
            Ok((base, others))
        })??;
        let view = st.history_merge_plan(base, &others)?;
        let ds = decisions(&view);
        let (choices, picked) = parse_choices(&view, &ds, args.get("choices"))?;
        let name = |s: Source| source_name(&view, s);
        let open: Vec<&str> = ds.iter().zip(&picked).filter(|(d, p)| d.conflict && p.is_none()).map(|(d, _)| d.id.as_str()).collect();
        let decided: Vec<Value> = ds
            .iter()
            .zip(&picked)
            .map(|(d, p)| {
                let mut o = d.info.clone();
                o["id"] = json!(d.id);
                o["by"] = json!(d.by.iter().map(|s| name(*s)).collect::<Vec<_>>());
                o["take"] = json!(p.or_else(|| (!d.conflict).then(|| d.by[0])).map(name));
                if d.conflict {
                    o["conflict"] = json!(true);
                    if p.is_none() {
                        o["options"] = json!(d.options);
                    }
                }
                if p.is_some() {
                    o["chosen"] = json!(true);
                }
                o
            })
            .collect();
        let mut out = json!({
            "base": { "id": base, "name": name(base) },
            "variants": view.variants.iter().map(|v| json!({ "id": v.id, "name": v.name })).collect::<Vec<_>>(),
            "origin": { "step": view.origin.node, "label": view.origin.label, "checkpoints": view.origin.checkpoints },
            "decisions": decided,
        });
        let dry = args.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
        if dry || !open.is_empty() {
            out["combined"] = json!(false);
            out["open"] = json!(open);
            out["hint"] = json!(if open.is_empty() {
                "dryRun – nothing changed. Call again without dryRun to create the combined variant."
            } else {
                "Nothing changed yet. Decide every id in open with choices – e.g. {\"<id>\": <variant ID or name>}, 0 = keep the origin; \"*\": <variant> = that variant in every conflict it is part of – and call combine again."
            });
            return Ok(out);
        }
        let label = tr!("Combined: {names}", names = view.variants.iter().skip(1).map(|v| v.name.as_str()).collect::<Vec<_>>().join(", "));
        let new_name = args.get("name").and_then(Value::as_str).map(str::to_string);
        st.history_merge_apply(&MergeRequest { target: base, others, choices }, new_name)?;
        self.changed("combine", label);
        let pos = self.with_history(position)?;
        if let (Some(o), Some(p)) = (out.as_object_mut(), pos.as_object()) {
            o.extend(p.clone());
        }
        out["ok"] = json!(true);
        out["combined"] = json!(true);
        Ok(out)
    }

    /// Runs a history change that moves the project; answers with where it stands.
    fn history_moved(&self, tool: &str, summary: impl FnOnce(&Value) -> String) -> Result<Value, String> {
        let mut pos = self.with_history(position)?;
        self.changed(tool, summary(&pos));
        pos["ok"] = json!(true);
        Ok(pos)
    }

    fn history_tool(&self, name: &str, args: &Value) -> Result<Value, String> {
        let st = self.st();
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        match name {
            "history" => {
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(40) as usize;
                self.with_history(|h| history_table(h, limit))
            }
            "variant_list" => self.with_history(variant_list),
            "checkpoint" => {
                let name = args.get("name").and_then(Value::as_str).map(str::to_string);
                let v = st.history_checkpoint(name)?;
                let c = v.checkpoints.iter().max_by_key(|c| c.id).ok_or("No checkpoint was set")?;
                self.changed("checkpoint", tr!("Checkpoint \"{name}\"", name = c.name));
                let label = v.rows.iter().find(|r| r.id == c.node).map(|r| r.label.clone());
                Ok(json!({ "ok": true, "checkpoint": { "id": c.id, "name": c.name }, "step": c.node, "label": label }))
            }
            "goto" => {
                let (target, before) = self.with_history(|h| (step_arg(h, args), h.variants().len()))?;
                let target = target?.ok_or("Give step (an ID from history) or checkpoint (its ID or name)")?;
                st.history_goto(target)?;
                let mut out = self.history_moved("goto", |p| format!("{} → {}", tr!("History"), text(&p["label"])))?;
                if self.with_history(|h| h.variants().len())? > before {
                    out["variantCreated"] = json!(true);
                }
                Ok(out)
            }
            "variant_new" => {
                let at = self.with_history(|h| step_arg(h, args))??;
                st.history_variant_new(args.get("name").and_then(Value::as_str).map(str::to_string), at)?;
                self.history_moved("variant_new", |p| format!("{}: {}", tr!("New variant"), text(&p["variant"]["name"])))
            }
            "variant_switch" => {
                let v = args.get("variant").filter(|v| !v.is_null()).ok_or("Argument \"variant\" (ID or name) is missing")?;
                let id = self.with_history(|h| variant_arg(h, v))??;
                st.history_variant_switch(id)?;
                self.history_moved("variant_switch", |p| tr!("Variant \"{name}\"", name = text(&p["variant"]["name"])))
            }
            "combine" => self.combine(args),
            other => Err(format!("Unknown tool: {other}")),
        }
    }
}

impl<S: AgentShell> Handler for Agent<S> {
    fn info(&self) -> ServerInfo {
        ServerInfo {
            name: "klangwelt".into(),
            title: "Klangwelt".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            instructions: INSTRUCTIONS.replace("{ROLES}", ROLES),
        }
    }

    fn tools(&self) -> Vec<Tool> {
        tools(&self.shell.pack_ids())
    }

    /// Every tool call: counts as "agent working" (except the waiting
    /// `chat_poll`), reports what changed and carries `cancelRequested`.
    fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        let st = self.st();
        let polling = name == "chat_poll" && !args.get("peek").and_then(Value::as_bool).unwrap_or(false);
        if !polling {
            st.chat.touch();
        }
        // a long poll may wait minutes – changes in the meantime aren't its own
        let before = if polling { None } else { self.id_state() };
        CHANGES.with(|c| c.borrow_mut().clear());
        let out = self.dispatch(name, args);
        let cancel = st.chat.cancel_requested();
        match out {
            Ok(mut v) => {
                if let Some(b) = &before {
                    self.report_changes(&mut v, b);
                }
                if !cancel.is_empty() {
                    match v.as_object_mut() {
                        Some(o) => {
                            o.insert("cancelRequested".into(), json!(cancel));
                        }
                        None => v = json!({ "result": v, "cancelRequested": cancel }),
                    }
                }
                tidy(&mut v);
                Ok(v)
            }
            Err(e) if !cancel.is_empty() => Err(format!("{e}\n(cancelRequested: {cancel:?} – the person asked to stop these tasks)")),
            Err(e) => Err(e),
        }
    }

    fn prompts(&self) -> Vec<Prompt> {
        vec![
            Prompt {
                name: "zuhoeren",
                title: "Listen in the Klangwelt chat",
                description: "Starts a listener subagent that picks up tasks from the chat of the Klangwelt interface and confirms them; you start workers for them.",
                text: format!(
                    "Start a subagent that listens in the Klangwelt chat – in the background so we can keep talking, on a small, fast model \
                     (type \"klangwelt-zuhoerer\" if available, otherwise a general-purpose subagent with access to the Klangwelt tools). \
                     Give it this task word for word:\n\n{LISTENER_BRIEF}\n\n\
                     {ROLES}\n\
                     Let me know briefly when it is running. If it ends without the person having asked it to stop listening, start a new one."
                ),
            },
            Prompt {
                name: "arbeiten",
                title: "Worker for a Klangwelt task",
                description: "Task for a worker subagent that carries out one task from the chat (add the task id, text and context).",
                text: format!("{WORKER_BRIEF}\n\nYour task (id, text, context):"),
            },
        ]
    }
}

/// Rounds decimal numbers to 4 places (f32 values otherwise show up as 0.550000011920929).
fn tidy(v: &mut Value) {
    match v {
        Value::Number(n) if n.is_f64() => {
            let x = n.as_f64().unwrap_or(0.0);
            if let Some(r) = serde_json::Number::from_f64((x * 10_000.0).round() / 10_000.0) {
                *n = r;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(tidy),
        Value::Object(o) => o.values_mut().for_each(tidy),
        _ => {}
    }
}

impl<S: AgentShell> Agent<S> {
    fn dispatch(&self, name: &str, args: Value) -> Result<Value, String> {
        let st = self.st();
        match name {
            "get_project" => {
                let ids: Option<Vec<u64>> = args.get("trackIds").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect());
                let fields: Option<Vec<String>> = args.get("fields").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
                self.project_view(args.get("includeNotes").and_then(Value::as_bool).unwrap_or(false), ids.as_deref(), fields.as_deref())
            }
            "execute" => self.execute(&args, &mut BTreeMap::new()),
            "batch" => self.batch(&args),
            "chat_peek" => Ok(self.chat_peek(&args)),
            "history" | "variant_list" | "checkpoint" | "goto" | "variant_new" | "variant_switch" | "combine" => self.history_tool(name, &args),
            "undo" | "redo" => {
                let label = { let s = lock(&st.session)?; if name == "undo" { s.undo_label() } else { s.redo_label() }.map(str::to_string) };
                let Some(label) = label else { return Ok(json!({ "ok": false, "info": "Nothing to do" })) };
                let snap = if name == "undo" { st.undo()? } else { st.redo()? };
                drop(snap);
                self.changed(name, if name == "undo" { tr!("Undone: {label}", label = label) } else { tr!("Redone: {label}", label = label) });
                Ok(json!({ "ok": true, "schritt": label }))
            }
            "open_project" => {
                let path = std::path::PathBuf::from(str_arg(&args, "path")?.trim().trim_matches('"'));
                if !crate::vfs::exists(&path) || crate::vfs::is_dir(&path) {
                    return Err(format!("File not found: {}", path.display()));
                }
                // The window opens through its normal path (asks about unsaved changes)
                self.shell.open_in_window(&path)?;
                self.activity("open_project", tr!("Open project: {file}", file = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
                Ok(json!({ "ok": true, "hinweis": "Opened in the window – if there are unsaved changes, Klangwelt asks first." }))
            }
            "transport" => {
                if let Some(b) = args.get("positionBeats").and_then(Value::as_f64) {
                    st.seek_seconds(b * 60.0 / st.tempo())?;
                }
                if let Some(a) = args.get("action").and_then(Value::as_str) {
                    st.transport(a)?;
                    self.activity("transport", match a {
                        "play" => tr!("Playback"),
                        "stop" => tr!("Stop"),
                        "rewind" => tr!("Go to start"),
                        _ => tr!("Playback toggled"),
                    });
                }
                settle();
                let s = st.status();
                Ok(json!({ "playing": s.playing, "positionBeats": s.position_secs * st.tempo() / 60.0 }))
            }
            "set_metronome" => {
                let on = args.get("enabled").and_then(Value::as_bool).ok_or("Argument \"enabled\" is missing")?;
                st.set_metronome(on)?;
                self.changed("set_metronome", if on { tr!("Metronome on") } else { tr!("Metronome off") });
                Ok(json!({ "metronome": on }))
            }
            "audio_devices" => serde_json::to_value(st.audio_devices()?).map_err(|e| e.to_string()),
            "set_audio_device" => {
                let kind = str_arg(&args, "kind")?;
                let name = args.get("name").and_then(Value::as_str).map(str::to_string);
                st.set_audio_device(kind, name)?;
                let d = st.audio_devices()?;
                let input = matches!(kind, "eingabe" | "input");
                let device = if input { d.input.clone() } else { d.output.clone() }.unwrap_or_else(|| tr!("System default"));
                self.changed("set_audio_device", if input { tr!("Input: {device}", device = device) } else { tr!("Output: {device}", device = device) });
                serde_json::to_value(d).map_err(|e| e.to_string())
            }
            "export_project" => {
                let opts: crate::export::ExportOptions = serde_json::from_value(args.clone()).map_err(|e| format!("Arguments: {e}"))?;
                let r = crate::export::run(&crate::export::export_dir(&st), &st, &opts)?;
                self.activity("export_project", tr!("Exported: {path}", path = r.path));
                serde_json::to_value(r).map_err(|e| e.to_string())
            }
            "describe_sound" => serde_json::to_value(st.describe_track(Id(u64_arg(&args, "trackId")?))?).map_err(|e| e.to_string()),
            "set_sound" => self.set_sound(&args),
            "set_sound_field" => {
                let track_id = Id(u64_arg(&args, "trackId")?);
                let field = str_arg(&args, "field")?;
                let preset = args.get("preset").and_then(Value::as_str);
                let xy = match (args.get("x").and_then(Value::as_f64), args.get("y").and_then(Value::as_f64)) {
                    (Some(x), Some(y)) => Some((x as f32, y as f32)),
                    (None, None) => None,
                    _ => return Err("Give x and y together (or a starting point)".into()),
                };
                st.sound_field(track_id, field, xy, preset, false)?;
                let report = st.describe_track(track_id)?;
                let words: Vec<String> = report.anzeige.iter().map(|m| format!("{} {}", m.label, m.wort)).collect();
                self.changed("set_sound_field", tr!("Field {field}: {words}", field = field, words = words.join(", ")));
                let mut out = serde_json::to_value(&report).map_err(|e| e.to_string())?;
                if let Some(p) = args.get("preview").and_then(Value::as_str) {
                    out["vorgespielt"] = json!(st.preview_track(track_id, p)?);
                }
                Ok(out)
            }
            "list_song_templates" => Ok(json!({ "vorlagen": song::templates().iter().map(|t| json!({
                "template": t.template, "name": t.name, "beschreibung": t.description, "tempo": t.tempo, "tonart": t.key,
                "akkorde": t.chords, "taktJeAkkord": t.chord_bars, "takte": t.total_bars(),
                "rollen": t.layers.iter().map(|l| json!({ "key": l.key, "name": l.name, "rolle": l.role, "klang": l.sound })).collect::<Vec<_>>(),
                "abschnitte": t.sections.iter().map(|s| json!({ "name": s.name, "takte": s.bars, "art": s.kind, "levels": s.levels, "stufen": song::levels_text(s, t) })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>() })),
            "write_song" => {
                let mut plan = song::template(str_arg(&args, "template")?)?;
                if let Some(t) = args.get("tempo").and_then(Value::as_f64) {
                    plan.tempo = t;
                }
                if let Some(k) = args.get("key").and_then(Value::as_str) {
                    plan.key = k.to_string();
                }
                if let Some(c) = args.get("chords") {
                    plan.chords = serde_json::from_value(c.clone()).map_err(|_| "chords: list of strings".to_string())?;
                }
                if let Some(n) = args.get("chordBars").and_then(Value::as_u64) {
                    plan.chord_bars = n.clamp(1, 8) as u32;
                }
                if let Some(s) = args.get("sections") {
                    plan.sections = serde_json::from_value(s.clone()).map_err(|e| format!("sections: [{{ name, bars, kind, levels }}] – {e}"))?;
                }
                let replace = args.get("replace").and_then(Value::as_bool).unwrap_or(true);
                let w = st.song_write(&plan, replace, "Agent · ")?;
                self.changed("write_song", trn!(w.bars, "Song \"{name}\": {n} bar", "Song \"{name}\": {n} bars", name = plan.name));
                if args.get("play").and_then(Value::as_bool).unwrap_or(false) {
                    st.seek_seconds(0.0)?;
                    st.transport("play")?;
                }
                Ok(json!({ "ok": true, "spuren": w.track_ids, "ersetzt": w.replaced, "takte": w.bars, "sekunden": (w.seconds * 10.0).round() / 10.0,
                    "abschnitte": plan.sections.iter().map(|s| format!("{} ({} bars): {}", s.name, s.bars, song::levels_text(s, &plan))).collect::<Vec<_>>() }))
            }
            "list_drum_patterns" => {
                let info = rhythm::info();
                let want = args.get("genre").and_then(Value::as_str).map(str::to_lowercase);
                let genres: Vec<Value> = info
                    .genres
                    .iter()
                    .filter(|g| want.as_ref().map_or(true, |w| g.name.to_lowercase().contains(w.as_str())))
                    .map(|g| json!({
                        "name": g.name, "beschreibung": g.blurb, "klang": g.sound,
                        "muster": g.patterns.iter().map(|p| json!({
                            "name": p.name, "bpm": p.bpm, "swing": p.swing, "tipp": p.tip,
                            "zeilen": p.grid.rows.iter().map(|(k, v)| (k.clone(), json!(rhythm::row_text(v, p.grid.steps)))).collect::<serde_json::Map<_, _>>(),
                        })).collect::<Vec<_>>(),
                    }))
                    .collect();
                if genres.is_empty() {
                    return Err(format!("No genre \"{}\" (available: {})", want.unwrap_or_default(), info.genres.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", ")));
                }
                Ok(json!({ "trommeln": info.rows.iter().map(|r| json!({ "key": r.key, "name": r.label, "taste": r.midi })).collect::<Vec<_>>(), "genres": genres }))
            }
            "write_drum_pattern" => {
                let bars = args.get("bars").and_then(Value::as_u64).unwrap_or(4).clamp(1, rhythm::MAX_BARS as u64) as usize;
                let steps = bars * rhythm::STEPS_PER_BAR;
                let track_id = args.get("trackId").and_then(Value::as_u64).map(Id);
                let clip_id = args.get("clipId").and_then(Value::as_u64).map(Id);
                let (grid, name, bpm, swing_default, sound) = match args.get("rows") {
                    Some(rows) => (rhythm::grid_from_json(rows, steps)?, "Beat".to_string(), None, 0.0, None),
                    None => {
                        let genre = args.get("genre").and_then(Value::as_str);
                        let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or("");
                        if genre.is_none() && pattern.is_empty() {
                            return Err("Give genre/pattern or rows (list_drum_patterns shows all patterns)".into());
                        }
                        let (g, p) = rhythm::find_pattern(genre, pattern)?;
                        (p.grid.resized(steps), p.name.clone(), Some(p.bpm as f64), p.swing as f64, Some(g.sound.clone()))
                    }
                };
                let swing = args.get("swing").and_then(Value::as_f64).unwrap_or(swing_default).clamp(0.0, 100.0);
                let tempo = if args.get("setTempo").and_then(Value::as_bool).unwrap_or(false) { bpm } else { None };
                let start = match args.get("startBeats").and_then(Value::as_f64) {
                    Some(b) => b,
                    None => rhythm::free_start(&st, track_id),
                };
                let w = st.rhythm_write(grid.clone(), swing, track_id, clip_id, start, &name, sound.as_deref(), tempo, "Agent · ")?;
                self.changed("write_drum_pattern", tr!("Rhythm \"{name}\" written", name = name));
                let mut out = json!({ "ok": true, "trackId": w.track_id, "clipId": w.clip_id, "name": name, "startBeats": start, "takte": bars, "swing": swing,
                    "zeilen": grid.rows.iter().map(|(k, v)| (k.clone(), json!(rhythm::row_text(v, steps)))).collect::<serde_json::Map<_, _>>() });
                if let Some(t) = tempo {
                    out["tempo"] = json!(t);
                }
                if args.get("preview").and_then(Value::as_bool).unwrap_or(false) {
                    out["vorgespielt"] = json!(st.rhythm_preview(Some(w.track_id), Some(w.clip_id), None, swing, None)?);
                }
                Ok(out)
            }
            "read_drum_pattern" => {
                let r = st.rhythm_read(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?))?;
                Ok(json!({ "name": r.name, "schritte": r.grid.steps, "takte": r.grid.steps / rhythm::STEPS_PER_BAR, "swing": r.swing, "nichtImRaster": r.skipped,
                    "zeilen": r.grid.rows.iter().map(|(k, v)| (k.clone(), json!(rhythm::row_text(v, r.grid.steps)))).collect::<serde_json::Map<_, _>>() }))
            }
            "list_saved_sounds" => {
                let path = sound_library::file(&self.st());
                let want = if args.get("kind").is_some() { Some(parse_kind(&args)?) } else { None };
                let klaenge: Vec<Value> = sound_library::list(&path)?
                    .iter()
                    .filter(|e| want.map_or(true, |k| e.kind == k))
                    .map(|e| json!({ "id": e.id, "name": e.name, "kind": e.kind, "instrument": e.kind_name, "beschreibung": e.summary }))
                    .collect();
                Ok(json!({ "klaenge": klaenge, "datei": path.display().to_string() }))
            }
            "save_sound" => {
                let inst = st.track_sound(Id(u64_arg(&args, "trackId")?))?;
                let r = sound_library::save(&sound_library::file(&self.st()), str_arg(&args, "name")?, &inst)?;
                sound_library::announce(&self.st(), &r.sounds);
                self.activity("save_sound", if r.replaced { tr!("Sound \"{name}\" replaced", name = r.name) } else { tr!("Sound \"{name}\" saved", name = r.name) });
                let summary = r.sounds.iter().find(|e| e.id == r.id).map(|e| e.summary.clone());
                Ok(json!({ "ok": true, "id": r.id, "name": r.name, "ersetzt": r.replaced, "beschreibung": summary }))
            }
            "load_saved_sound" => {
                let track_id = Id(u64_arg(&args, "trackId")?);
                let prefer = st.track_sound(track_id)?.kind;
                let saved = sound_library::find(&sound_library::file(&self.st()), str_arg(&args, "sound")?, Some(prefer))?;
                st.load_sound(track_id, &saved, "Agent · ")?;
                self.changed("load_saved_sound", tr!("Sound \"{name}\" loaded", name = saved.name));
                let mut out = serde_json::to_value(st.describe_track(track_id)?).map_err(|e| e.to_string())?;
                out["geladen"] = json!(saved.name);
                if let Some(p) = args.get("preview").and_then(Value::as_str) {
                    out["vorgespielt"] = json!(st.preview_track(track_id, p)?);
                }
                Ok(out)
            }
            "rename_saved_sound" => {
                let path = sound_library::file(&self.st());
                let saved = sound_library::find(&path, str_arg(&args, "sound")?, None)?;
                let list = sound_library::rename(&path, &saved.id, str_arg(&args, "name")?)?;
                sound_library::announce(&self.st(), &list);
                let name = list.iter().find(|e| e.id == saved.id).map(|e| e.name.clone()).unwrap_or_default();
                self.activity("rename_saved_sound", tr!("Sound \"{old}\" renamed to \"{name}\"", old = saved.name, name = name));
                Ok(json!({ "ok": true, "id": saved.id, "name": name }))
            }
            "delete_saved_sound" => {
                let path = sound_library::file(&self.st());
                let saved = sound_library::find(&path, str_arg(&args, "sound")?, None)?;
                let list = sound_library::delete(&path, &saved.id)?;
                sound_library::announce(&self.st(), &list);
                self.activity("delete_saved_sound", tr!("Sound \"{name}\" deleted", name = saved.name));
                Ok(json!({ "ok": true, "geloescht": saved.name, "uebrig": list.len() }))
            }
            "chat_poll" if args.get("peek").and_then(Value::as_bool).unwrap_or(false) => Ok(self.chat_peek(&args)),
            "chat_poll" => {
                // At most 590 s: the stdio bridge waits up to 600 s for a reply.
                let secs = args.get("timeoutSeconds").and_then(Value::as_f64).unwrap_or(50.0).clamp(1.0, 590.0);
                let lease = args.get("leaseSeconds").and_then(Value::as_f64).unwrap_or(crate::chat::DEFAULT_LEASE.as_secs_f64()).clamp(10.0, 3600.0);
                let client = client_arg(&args);
                let notify = || self.emit("chat-changed", &self.st().chat_view());
                let polled = st.chat.poll(&client, std::time::Duration::from_secs_f64(lease), std::time::Duration::from_secs_f64(secs), notify, klang_mcp::caller_gone);
                let tasks = polled.tasks;
                // If the client aborted an earlier poll, its timeout is shorter than the wait time.
                let abbruch = polled.abandoned_before.map(|s| {
                    format!(
                        "An earlier chat_poll was aborted by the client after {s:.0} s (client timeout). \
                         Use timeoutSeconds {} or raise the Klangwelt server's timeout (\"timeout\": 330000 in the MCP configuration).",
                        ((s - 10.0).floor() as i64).clamp(10, 50)
                    )
                });
                // Keep empty replies short: a listening subagent polls for hours.
                if tasks.is_empty() {
                    let mut r = json!({ "auftraege": [], "hinweis": "No new tasks – call chat_poll again." });
                    if let Some(a) = abbruch {
                        r["abbruch"] = json!(a);
                    }
                    return Ok(r);
                }
                let verlauf: Vec<Value> = st
                    .chat
                    .recent(12)
                    .iter()
                    .map(|m| json!({ "id": m.id, "von": if m.from == crate::chat::Author::User { "user" } else { "agent" }, "text": m.text }))
                    .collect();
                let auftraege: Vec<Value> = tasks
                    .iter()
                    .map(|m| {
                        let mut j = json!({ "id": m.id, "text": m.text, "kontext": m.kontext });
                        if !m.cancels.is_empty() {
                            j["cancels"] = json!(m.cancels);
                        }
                        if m.redelivered > 0 {
                            j["redelivered"] = json!(m.redelivered);
                        }
                        j
                    })
                    .collect();
                let mut r = json!({
                    "auftraege": auftraege,
                    "hinweis": format!("Confirm each task right away with chat_reply (replyTo = id, done=false) – without a reply within {lease:.0} s it's offered again. \
                                        Have it carried out, finish with chat_reply (done=true), then call chat_poll again. A task with cancels asks to stop those tasks."),
                    "leaseSeconds": lease,
                    "verlauf": verlauf,
                });
                if let Some(a) = abbruch {
                    r["abbruch"] = json!(a);
                }
                Ok(r)
            }
            "chat_reply" => {
                let text = str_arg(&args, "text")?;
                let reply_to = args.get("replyTo").and_then(Value::as_u64);
                let done = args.get("done").and_then(Value::as_bool).unwrap_or(true);
                let client = args.get("client").and_then(Value::as_str).map(|_| client_arg(&args));
                let m = st.chat.reply(text, reply_to, done, client.as_deref())?;
                self.emit("chat-changed", &st.chat_view());
                Ok(json!({ "ok": true, "id": m.id }))
            }
            "list_instruments" => serde_json::to_value(sound::specs()).map(|v| json!({ "instrumente": v })).map_err(|e| e.to_string()),
            "create_instrument_track" => {
                let kind = parse_kind(&args)?;
                let (_, id) = st.create_instrument_track(kind, &tr!("Agent · Create {kind}", kind = sound::kind_name(kind)))?;
                self.changed("create_instrument_track", tr!("{kind} track with pattern created", kind = sound::kind_name(kind)));
                Ok(json!({ "ok": true, "trackId": id, "instrument": sound::kind_name(kind) }))
            }
            "set_instrument" => {
                let track_id = Id(u64_arg(&args, "trackId")?);
                let track = st.instrument_track(track_id)?;
                let kind = parse_kind(&args)?;
                let mut inst = sound::default_instrument(kind);
                if let Some(name) = args.get("preset").and_then(Value::as_str) {
                    let spec = sound::spec(kind);
                    let preset = spec.presets.iter().find(|p| p.name.eq_ignore_ascii_case(name)).ok_or_else(|| {
                        let names: Vec<&str> = spec.presets.iter().map(|p| p.name).collect();
                        format!("Unknown preset \"{name}\". Available: {}", names.join(", "))
                    })?;
                    for (k, v) in &preset.params {
                        inst.params.insert(k.to_string(), *v);
                    }
                    inst.preset = preset.name.to_string();
                }
                let label = tr!("Agent · {kind} for \"{name}\"", kind = sound::kind_name(kind), name = track.name);
                st.execute(
                    Command::Batch { label: label.clone(), commands: vec![Command::SetTrack { track_id, patch: TrackPatch { instrument: Some(inst), ..Default::default() } }] },
                    false,
                )?;
                self.changed("set_instrument", label);
                serde_json::to_value(st.describe_track(track_id)?).map_err(|e| e.to_string())
            }
            "preview_sound" => {
                let track_id = Id(u64_arg(&args, "trackId")?);
                let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or("akkord");
                let secs = st.preview_track(track_id, pattern)?;
                self.activity("preview_sound", tr!("Preview: {what}", what = pattern));
                Ok(json!({ "spielt": pattern, "sekunden": secs }))
            }
            "analyze_clip" if mode_arg(&args)? == crate::voices::AnalysisMode::Poly => {
                let clip = st.clip(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?))?;
                let ClipSource::Audio { asset_id, .. } = &clip.source else {
                    return Err(tr!("\"{name}\" is not an audio clip.", name = clip.name));
                };
                let a = st.analyze_voices(asset_id, voices_arg(&args)?)?;
                Ok(json!({
                    "clip": clip.name,
                    "dauerSekunden": a.duration_secs,
                    "stimmen": a.voices,
                    "automatisch": a.auto,
                    "tonart": a.key.as_ref().map(|k| &k.name),
                    "tempoSchaetzung": a.tempo_estimate,
                    "proStimme": (0..a.voices).map(|v| json!({
                        "stimme": v + 1,
                        "noten": a.notes.iter().filter(|n| n.voice == v).map(|n| json!({
                            "midi": n.midi, "name": note_name(n.midi), "start": n.start_secs, "ende": n.end_secs, "velocity": n.velocity })).collect::<Vec<_>>()
                    })).collect::<Vec<_>>()
                }))
            }
            "analyze_clip" => {
                let (clip, a) = st.clip_analysis(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?))?;
                Ok(json!({
                    "clip": clip.name,
                    "dauerSekunden": a.duration_secs,
                    "tonart": a.key.as_ref().map(|k| &k.name),
                    "tempoSchaetzung": a.tempo_estimate,
                    "noten": a.notes.iter().map(|n| json!({
                        "midi": n.midi, "name": note_name(n.midi), "start": (n.start_secs * 1000.0).round() / 1000.0,
                        "ende": (n.end_secs * 1000.0).round() / 1000.0, "korrigiert": n.midi != n.raw_midi,
                        "velocity": n.velocity, "attack": n.attack, "articulation": n.articulation })).collect::<Vec<_>>(),
                    "anschlaege": a.hits.iter().map(|h| json!({
                        "start": (h.time * 1000.0).round() / 1000.0,
                        "grundton": h.midi.map(|m| note_name(m.round().clamp(0.0, 127.0) as u8)),
                        "midi": h.midi, "staerke": h.strength })).collect::<Vec<_>>()
                }))
            }
            "melody_to_track" => {
                let label = tr!("Agent · {label}", label = tr!("Import melody"));
                let (_, new_id) = st.melody_to_track(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?), &label, strength_arg(&args, "smooth")?)?;
                self.changed("melody_to_track", tr!("Melody created as instrument track"));
                Ok(json!({ "ok": true, "trackId": new_id }))
            }
            "audio_to_notes" => {
                let target: crate::convert::Target = serde_json::from_value(args.get("target").cloned().unwrap_or(Value::Null))
                    .map_err(|_| "\"target\" must be 'melodie' or 'rhythmus'".to_string())?;
                let clip = args.get("clipId").and_then(Value::as_u64).map(Id);
                let label = tr!("Agent · {label}", label = match target {
                    crate::convert::Target::Melodie => tr!("Convert to melody track"),
                    crate::convert::Target::Rhythmus => tr!("Convert to rhythm track"),
                });
                if target == crate::convert::Target::Melodie && mode_arg(&args)? == crate::voices::AnalysisMode::Poly {
                    let label = tr!("Agent · {label}", label = tr!("Convert to voice tracks"));
                    let r = st.audio_to_voices(Id(u64_arg(&args, "trackId")?), clip, &label, strength_arg(&args, "smooth")?, voices_arg(&args)?)?;
                    self.changed("audio_to_notes", tr!("Recording converted: {n} voices", n = r.voices));
                    return Ok(json!({ "ok": true, "trackId": r.track_id, "trackIds": r.track_ids, "stimmen": r.voices, "noten": r.notes, "geglaettet": r.smoothed }));
                }
                let r = st.audio_to_notes(Id(u64_arg(&args, "trackId")?), clip, target, &label, strength_arg(&args, "smooth")?)?;
                self.changed("audio_to_notes", trn!(r.notes, "Recording converted: {n} note", "Recording converted: {n} notes"));
                Ok(json!({ "ok": true, "trackId": r.track_id, "noten": r.notes, "geglaettet": r.smoothed }))
            }
            "import_audio" => {
                let paths: Vec<String> = args.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
                if paths.is_empty() {
                    return Err("Argument \"paths\" is missing (list of file paths)".into());
                }
                // via the core's file system: on desktop the disk, in the browser dropped/uploaded files
                let files = paths
                    .into_iter()
                    .map(|p| {
                        let p = p.trim().trim_matches('"').to_string();
                        let bytes = crate::vfs::read(Path::new(&p)).map_err(|e| format!("Can't read the file ({e})"));
                        (p, bytes)
                    })
                    .collect();
                let track = args.get("trackId").and_then(Value::as_u64).map(Id);
                let start = args.get("startBeats").and_then(Value::as_f64).unwrap_or(0.0);
                let r = st.import_audio(files, track, start)?;
                self.changed("import_audio", tr!("{n} audio file(s) inserted", n = r.imported.len()));
                Ok(json!({ "ok": true, "eingefuegt": r.imported, "fehler": r.errors }))
            }
            "delete_range" => {
                let num = |k: &str| args.get(k).and_then(Value::as_f64).ok_or_else(|| format!("Argument \"{k}\" is missing"));
                let (a, b) = (num("startBeats")?, num("endBeats")?);
                drop(st.delete_range(a, b)?);
                self.changed("delete_range", tr!("Time range {from}–{to} deleted, rest moved up", from = a, to = b));
                Ok(json!({ "ok": true, "geloescht": [a.min(b), a.max(b)], "aufgerueckt": (a - b).abs() }))
            }
            "quantize_notes" => {
                let grid = args.get("grid").and_then(Value::as_f64).ok_or("Argument \"grid\" is missing (beats, e.g. 0.5 = eighth)")?;
                let lengths = args.get("lengths").and_then(Value::as_bool).unwrap_or(false);
                let indices = args.get("notes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|i| i as usize).collect());
                let label = tr!("Agent · {label}", label = tr!("Quantize notes"));
                let r = st.quantize_notes(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?), indices, grid, lengths, Some(&label))?;
                self.changed("quantize_notes", tr!("Notes quantized"));
                Ok(json!({ "ok": true, "ausgerichtet": r.selected.len(), "geaendert": r.changed }))
            }
            "smooth_notes" => {
                let strength = strength_arg(&args, "strength")?;
                let indices = args.get("notes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|i| i as usize).collect());
                let label = tr!("Agent · {label}", label = crate::convert::smooth_label(strength));
                let r = st.smooth_notes(Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?), indices, strength, Some(&label))?;
                if r.changed {
                    self.changed("smooth_notes", trn!(r.merged + r.removed, "Notes smoothed: {n} fewer", "Notes smoothed: {n} fewer"));
                }
                Ok(json!({ "ok": true, "geaendert": r.changed, "zusammengelegt": r.merged, "entfernt": r.removed, "noten": r.selected }))
            }
            "download_library" => {
                let id = str_arg(&args, "id")?;
                match self.shell.library_download(id)? {
                    (dir, None) => {
                        self.activity("download_library", tr!("Downloading package: {id}", id = id));
                        Ok(json!({ "ok": true, "ordner": dir.display().to_string(), "hinweis": "Runs in the background and ends up in the active library – check with library_status; then search_melody." }))
                    }
                    (dir, Some(platz)) => {
                        self.activity("download_library", tr!("Package queued: {id} (position {place})", id = id, place = platz));
                        Ok(json!({ "ok": true, "warteschlange": platz, "ordner": dir.display().to_string(), "hinweis": "Another job is running right now – the package starts on its own afterwards and ends up in the currently active library. Check with library_status (queue)." }))
                    }
                }
            }
            "add_to_library" => {
                let paths: Vec<String> = args.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let name = self.shell.library_add(&paths)?;
                self.activity("add_to_library", tr!("Adding {n} file(s) to \"{name}\"", n = paths.len(), name = name));
                Ok(json!({ "ok": true, "bibliothek": name, "hinweis": "Runs in the background (a few seconds) – check with library_status." }))
            }
            "library_status" => serde_json::to_value(crate::library::info(&st)?).map_err(|e| e.to_string()),
            "browse_library" => {
                let strings = |key: &str| -> Vec<String> {
                    args.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
                };
                let levels = strings("levels");
                let path = strings("path");
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
                let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                serde_json::to_value(crate::library::tree(&st, &levels, &path, offset, limit)?).map_err(|e| e.to_string())
            }
            "list_libraries" => Ok(self.shell.libraries()),
            "switch_library" => {
                self.shell.library_switch(str_arg(&args, "library")?)?;
                crate::library::emit_info(st);
                let (name, scores) = lock(&st.library)?.as_ref().map(|l| (l.name.clone(), l.index.score_count())).unwrap_or_default();
                self.changed("switch_library", tr!("Library \"{name}\" active", name = name));
                Ok(json!({ "ok": true, "aktiv": name, "partituren": scores }))
            }
            "create_library" => {
                let activate = args.get("activate").and_then(Value::as_bool).unwrap_or(true);
                let (id, name) = self.shell.library_create(str_arg(&args, "name")?, activate)?;
                crate::library::emit_info(st);
                self.changed("create_library", tr!("Library \"{name}\" created", name = name));
                Ok(json!({ "ok": true, "id": id, "name": name }))
            }
            "delete_library" => {
                let id = self.shell.library_delete(str_arg(&args, "library")?)?;
                crate::library::emit_info(st);
                self.changed("delete_library", tr!("Library {id} deleted", id = id));
                Ok(json!({ "ok": true, "geloescht": id }))
            }
            "search_melody" => self.search(&args),
            "analyze_harmony" => {
                let key = self.key_of(&args)?;
                let track = args.get("trackId").and_then(Value::as_u64).map(Id);
                let clip = args.get("clipId").and_then(Value::as_u64).map(Id);
                let chords = harmony::chords_in(&st, track, clip, &key.key)?;
                let n = chords.len();
                let mut prev: Option<kh::Chord> = None;
                let akkorde: Vec<Value> = chords.iter().take(128).map(|p| {
                    let ch = p.info.chord;
                    let mood = kh::transition_mood(prev.as_ref(), &ch, &key.key);
                    let bruch = kh::harmonic_break(prev.as_ref(), &ch, &key.key);
                    prev = Some(ch);
                    let mut j = json!({ "startBeats": p.start_beats, "lengthBeats": p.length_beats,
                        "name": p.info.name, "stufe": p.info.roman, "funktion": p.info.function, "stimmung": mood.label() });
                    if let Some(b) = bruch {
                        j["bruch"] = json!(b);
                    }
                    j
                }).collect();
                Ok(json!({ "tonart": key.name, "quelle": key.source, "akkorde": akkorde, "anzahl": n }))
            }
            "suggest_chords" => {
                let key = self.key_of(&args)?;
                let chords = chords_of(&args, &key.key)?;
                let group = match args.get("corpus").and_then(Value::as_str) {
                    Some("barock") => 1,
                    Some("bach") => 2,
                    _ => 0,
                };
                let res = harmony::suggest_with_library(&st, &key.key, &chords, group)?;
                let mut r = json!({
                    "tonart": key.name,
                    "folge": chords.iter().map(|c| chord_json(&kh::describe(c, &key.key))).collect::<Vec<_>>(),
                    "vorschlaege": res.suggestions.iter().map(|sv| {
                        let s = &sv.suggestion;
                        let mut v = chord_json(&s.info);
                        v["art"] = json!(s.category);
                        v["warum"] = json!(s.reason);
                        v["wertung"] = json!(s.score);
                        v["stimmung"] = json!(s.mood.label());
                        if let Some(k) = &s.to_key {
                            v["ausweichenNach"] = json!(k);
                        }
                        if let Some(f) = &sv.library {
                            v["bibliothek"] = json!({
                                "anteil": f.share, "anzahl": f.count, "von": f.total,
                                "form": if f.exact { "exact" } else { "basic chord" },
                                "beispiele": f.examples.iter().take(3).map(|e| json!({
                                    "titel": e.title, "komponist": e.composer, "takt": e.bar, "doc": e.doc })).collect::<Vec<_>>(),
                            });
                        }
                        v
                    }).collect::<Vec<_>>(),
                    // multi-step paths from the last chord to the endings
                    "wegeZumSchluss": res.paths.iter().map(|p| {
                        let mut v = json!({
                            "schluss": p.path.label,
                            "akkorde": p.path.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                            "stufen": p.path.steps.iter().map(|s| s.roman.as_str()).collect::<Vec<_>>(),
                            "stimmungen": p.path.moods.iter().map(|m| m.label()).collect::<Vec<_>>(),
                            "wertung": p.path.score,
                        });
                        if let Some(sh) = &p.shares {
                            v["anteileBibliothek"] = json!(sh);
                        }
                        v
                    }).collect::<Vec<_>>(),
                });
                match (&res.library, &res.hint) {
                    (Some(l), h) => {
                        r["bibliothek"] = json!({ "name": l.name, "partituren": l.scores, "gruppe": GROUPS[l.group].1 });
                        if let Some(h) = h {
                            r["hinweis"] = json!(h);
                        }
                    }
                    (None, Some(h)) => r["hinweis"] = json!(h),
                    _ => {}
                }
                if args.get("cadences").and_then(Value::as_bool).unwrap_or(false) {
                    r["kadenzen"] = json!(kh::cadences(&key.key).iter().map(|t| json!({
                        "id": t.id, "name": t.name, "beschreibung": t.description,
                        "akkorde": t.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                        "stufen": t.steps.iter().map(|s| s.roman.as_str()).collect::<Vec<_>>() })).collect::<Vec<_>>());
                }
                Ok(r)
            }
            "write_chords" | "preview_chords" => {
                let key = self.key_of(&args)?;
                let chords = chords_of(&args, &key.key)?;
                let bpb = lock(&st.session)?.project.beats_per_bar as f64;
                let bpc = args.get("beatsPerChord").and_then(Value::as_f64).unwrap_or(bpb);
                let track = args.get("trackId").and_then(Value::as_u64).map(Id);
                let names: Vec<String> = chords.iter().map(|c| kh::chord_name(c, &key.key)).collect();
                if name == "preview_chords" {
                    let secs = harmony::preview(&st, track, &chords, &key.key, bpc, None)?;
                    self.activity("preview_chords", tr!("Preview: {what}", what = names.join(" – ")));
                    return Ok(json!({ "spielt": names, "sekunden": secs }));
                }
                let start = args.get("startBeats").and_then(Value::as_f64).unwrap_or(0.0);
                let label = tr!("Agent · Chords {chords}", chords = names.join(" – "));
                let insert = args.get("mode").and_then(Value::as_str) == Some("einfuegen");
                let (_, w) = harmony::write_mode(&st, track, start, &chords, &key.key, bpc, None, &label, insert)?;
                self.changed("write_chords", label);
                if args.get("preview").and_then(Value::as_bool).unwrap_or(false) {
                    harmony::preview(&st, Some(w.track_id), &chords, &key.key, bpc, None)?;
                }
                Ok(json!({
                    "ok": true, "tonart": key.name, "trackId": w.track_id, "clipId": w.clip_id,
                    "startBeats": w.start_beats, "lengthBeats": w.length_beats,
                    "akkorde": chords.iter().zip(&w.voicings).map(|(c, v)| {
                        let mut j = chord_json(&kh::describe(c, &key.key));
                        j["noten"] = json!(v.iter().map(|m| note_name(*m)).collect::<Vec<_>>());
                        j
                    }).collect::<Vec<_>>(),
                }))
            }
            "fugue_subject" => {
                let (tid, cid) = (Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?));
                let chosen = self.key_of(&args).ok().filter(|k| k.source == "gewaehlt").map(|k| k.key);
                let v = crate::fugue::subject_view(&st, tid, cid, chosen)?;
                let names = |n: &[VNote]| n.iter().map(|x| note_name(x.pitch)).collect::<Vec<_>>();
                Ok(json!({
                    "tonart": v.key.name, "laengeSchlaege": v.length, "umfang": [note_name(v.range.0), note_name(v.range.1)],
                    "thema": names(&v.notes),
                    "antwort": { "art": v.answer.kind, "grund": v.answer.reason, "noten": names(&v.answer.notes), "veraendert": v.answer.mutated },
                    "engfuehrungen": v.strettos.iter().map(|s| json!({ "intervall": s.label, "abstandSchlaege": s.delay,
                        "ueberlappung": (s.overlap * 100.0).round() / 100.0, "warnungen": s.warnings })).collect::<Vec<_>>(),
                    "verwandlungen": v.transforms.iter().map(|t| t.label.clone()).collect::<Vec<_>>(),
                }))
            }
            "transform_melody" => {
                let (tid, cid) = (Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?));
                let chosen = self.key_of(&args).ok().filter(|k| k.source == "gewaehlt").map(|k| k.key);
                let v = crate::fugue::subject_view(&st, tid, cid, chosen)?;
                let steps = args.get("steps").and_then(Value::as_i64).unwrap_or(0) as i32;
                let t = match str_arg(&args, "transform")? {
                    "umkehrung" => Transform::Umkehrung,
                    "krebs" => Transform::Krebs,
                    "krebsumkehrung" => Transform::Krebsumkehrung,
                    "vergroesserung" => Transform::Vergroesserung,
                    "verkleinerung" => Transform::Verkleinerung,
                    "versetzung" => Transform::Versetzung(steps),
                    "chromatisch" => Transform::Chromatisch(steps),
                    o => return Err(format!("Unknown transformation \"{o}\"")),
                };
                let notes = klang_music::fugue::transform(&v.notes, &v.key.key, t);
                let mut r = json!({ "verwandlung": t.label(), "noten": notes.iter().map(|n| note_name(n.pitch)).collect::<Vec<_>>() });
                if args.get("write").and_then(Value::as_bool).unwrap_or(true) {
                    let name = format!("{} – {}", v.clip_name, t.label());
                    let label = tr!("Agent · {label}", label = name);
                    let (_, ids) = crate::fugue::add_voices(&st, &[crate::fugue::VoiceIn { name, notes }], v.start, instrument_of(&args), &label)?;
                    self.changed("transform_melody", label);
                    r["trackId"] = json!(ids.first());
                }
                Ok(r)
            }
            "countersubject" => {
                let (tid, cid) = (Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?));
                let chosen = self.key_of(&args).ok().filter(|k| k.source == "gewaehlt").map(|k| k.key);
                let v = crate::fugue::subject_view(&st, tid, cid, chosen)?;
                let cs = klang_music::fugue::countersubjects(&v.notes, &v.key.key, v.beats_per_bar, 4);
                let mut r = json!({
                    "tonart": v.key.name,
                    "vorschlaege": cs.iter().map(|c| json!({ "lage": if c.above { "above the subject" } else { "below the subject" },
                        "umkehrbar": c.invertible, "fehler": c.errors, "warnungen": c.warnings,
                        "noten": c.notes.iter().map(|n| note_name(n.pitch)).collect::<Vec<_>>() })).collect::<Vec<_>>(),
                });
                if args.get("write").and_then(Value::as_bool).unwrap_or(false) {
                    let best = cs.first().ok_or("No countersubject found")?;
                    let label = tr!("Agent · {label}", label = tr!("Countersubject"));
                    let (_, ids) = crate::fugue::add_voices(&st, &[crate::fugue::VoiceIn { name: "Gegensatz".into(), notes: best.notes.clone() }], v.start, instrument_of(&args), &label)?;
                    self.changed("countersubject", label);
                    r["trackId"] = json!(ids.first());
                }
                Ok(r)
            }
            "write_fugue" => {
                let (tid, cid) = (Id(u64_arg(&args, "trackId")?), Id(u64_arg(&args, "clipId")?));
                let chosen = self.key_of(&args).ok().filter(|k| k.source == "gewaehlt").map(|k| k.key);
                let v = crate::fugue::subject_view(&st, tid, cid, chosen)?;
                let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(true);
                let opt = klang_music::fugue::FugueOptions {
                    voices: args.get("voices").and_then(Value::as_u64).unwrap_or(3) as usize,
                    beats_per_bar: v.beats_per_bar,
                    countersubject: flag("countersubject"),
                    stretto: flag("stretto"),
                    picardy: flag("picardy"),
                };
                let plan = klang_music::fugue::plan_fugue(&v.notes, &v.key.key, &opt);
                let start = match args.get("startBeats").and_then(Value::as_f64) {
                    Some(s) => s,
                    None => {
                        let s = lock(&st.session)?;
                        let bar = s.project.beats_per_bar.max(1) as f64;
                        let end = s.project.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.start_beats + c.length_beats(s.project.tempo_bpm))).fold(0.0, f64::max);
                        (end / bar).ceil() * bar
                    }
                };
                let voices: Vec<crate::fugue::VoiceIn> = plan.voices.iter().map(|pv| crate::fugue::VoiceIn { name: format!("Fuge – {}", pv.name), notes: pv.notes.clone() }).collect();
                let label = tr!("Agent · Fugue on \"{clip}\"", clip = v.clip_name);
                let (_, ids) = crate::fugue::add_voices(&st, &voices, start, instrument_of(&args), &label)?;
                self.changed("write_fugue", label);
                let bar = v.beats_per_bar as f64;
                Ok(json!({
                    "ok": true, "tonart": v.key.name, "trackIds": ids, "startBeats": start, "takte": (plan.length / bar).ceil(),
                    "antwort": plan.answer, "engfuehrung": plan.stretto, "gegensatz": plan.countersubject,
                    "abschnitte": plan.sections.iter().map(|s| json!({ "name": s.name, "tonart": s.key,
                        "abTakt": ((start + s.start) / bar).floor() + 1.0, "bisTakt": ((start + s.end) / bar).ceil(), "beschreibung": s.description })).collect::<Vec<_>>(),
                }))
            }
            "check_counterpoint" => {
                let only: Option<Vec<Id>> = args.get("trackIds").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(Id).collect());
                let c = crate::fugue::check_tracks(&st, only.as_deref())?;
                Ok(json!({
                    "stimmen": c.voices.iter().map(|v| json!({ "trackId": v.track_id, "name": v.name })).collect::<Vec<_>>(),
                    "fehler": c.errors, "warnungen": c.warnings, "hinweise": c.hints,
                    "stellen": c.issues.iter().take(120).map(|i| json!({ "art": i.kind, "schwere": i.severity, "takt": i.bar,
                        "startBeats": i.start, "endBeats": i.end, "trackIds": i.track_ids, "text": i.message })).collect::<Vec<_>>(),
                }))
            }
            "analyze_score" => {
                let score = match (args.get("doc").and_then(Value::as_u64), args.get("path").and_then(Value::as_str)) {
                    (Some(doc), _) => crate::library::doc_and_score(st, doc_arg(doc)?)?.1,
                    (None, Some(path)) if cfg!(target_arch = "wasm32") => {
                        let data = crate::vfs::read(Path::new(path)).map_err(|e| format!("{path}: {e} – in the browser give the doc of a library score"))?;
                        klang_music::score::load_score_bytes(path, &data).map_err(|e| e.to_string())?
                    }
                    (None, Some(path)) => load_score(Path::new(path)).map_err(|e| e.to_string())?,
                    (None, None) => return Err("Give doc (a library score from search_library) or path (a MusicXML/MXL file)".into()),
                };
                let mut v = serde_json::to_value(analyze_score(&score)).map_err(|e| e.to_string())?;
                v["titel"] = json!(score.title);
                v["komponist"] = json!(score.composer);
                Ok(v)
            }
            "search_library" => {
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
                let hits = crate::library::find_scores(st, str_arg(&args, "text")?, limit.min(100))?;
                self.activity("search_library", trn!(hits.len(), "Library search: {n} scores", "Library search: {n} scores"));
                Ok(json!({ "hits": hits }))
            }
            "read_library_score" => {
                let doc = doc_arg(u64_arg(&args, "doc")?)?;
                let bars = bars_arg(&args)?;
                let picks = voice_picks(&args)?;
                let read = crate::library::read_score(st, doc, bars, &picks, args.get("notes").and_then(Value::as_bool).unwrap_or(false))?;
                let v = serde_json::to_value(&read).map_err(|e| e.to_string())?;
                self.activity("read_library_score", tr!("Reading score: {title}", title = v["title"].as_str().unwrap_or_default()));
                Ok(v)
            }
            "import_library_score" => self.import_library_score(&args),
            other => Err(format!("Unknown tool: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_rounds_float_noise() {
        let mut v = json!({ "a": 0.55f32 as f64, "b": [0.3499999940395355, 2], "c": "x" });
        tidy(&mut v);
        assert_eq!(v, json!({ "a": 0.55, "b": [0.35, 2], "c": "x" }));
    }

    #[test]
    fn tool_schemas_are_objects_and_names_unique() {
        let t = tools(&["pdmx-kostprobe"]);
        let mut names: Vec<_> = t.iter().map(|x| x.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), t.len());
        for x in &t {
            assert_eq!(x.input_schema["type"], "object", "{}", x.name);
        }
    }

    #[test]
    fn schema_examples_parse_as_commands() {
        // The examples from the tool description (first paragraph) must be valid commands.
        let desc = tools(&[]).into_iter().find(|t| t.name == "execute").unwrap().description;
        let mut found = 0;
        for part in desc.lines().next().unwrap().split(", ") {
            let start = part.find('{').unwrap();
            let json_text = part[start..].trim_end_matches('.');
            let cmd: Result<Command, _> = serde_json::from_str(json_text);
            assert!(cmd.is_ok(), "{json_text}: {cmd:?}");
            found += 1;
        }
        assert_eq!(found, 3);
    }

    struct TestShell(AppState);

    impl AgentShell for TestShell {
        fn state(&self) -> &AppState {
            &self.0
        }
    }

    fn agent() -> Agent<TestShell> {
        Agent::new(TestShell(AppState::without_audio()))
    }

    fn call(a: &Agent<TestShell>, name: &str, args: Value) -> Value {
        Handler::call(a, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn tracks(a: &Agent<TestShell>) -> usize {
        lock(&a.st().session).unwrap().project.tracks.len()
    }

    #[test]
    fn execute_with_placeholders_and_compact_notes_reports_ids() {
        let a = agent();
        let r = call(&a, "execute", json!({ "commands": [
            { "type": "addTrack", "track": { "ref": "$bass", "name": "Bass", "kind": "instrument" } },
            { "type": "addClip", "trackId": "$bass", "clip": { "ref": "$riff", "startBeats": 0, "source": { "type": "notes", "lengthBeats": 4, "notes": "A2 1 · C3 · E3 v80 · G3" } } },
            { "type": "setTrack", "trackId": "$bass", "patch": { "gainDb": -4 } }
        ] }));
        let bass = r["ids"]["$bass"].as_u64().unwrap();
        let riff = r["ids"]["$riff"].as_u64().unwrap();
        assert_eq!(r["created"]["tracks"], json!([bass]));
        assert_eq!(r["created"]["clips"], json!([{ "trackId": bass, "clipId": riff }]));
        assert!(r["revision"].as_u64().is_some());
        assert!(r["change"].as_str().unwrap().starts_with("Agent · "));
        let s = lock(&a.st().session).unwrap();
        let t = s.project.track(Id(bass)).unwrap();
        assert_eq!(t.gain_db, -4.0);
        let ClipSource::Notes { notes, .. } = &t.clips[0].source else { panic!() };
        assert_eq!(notes.iter().map(|n| (n.pitch, n.start_beats, n.velocity)).collect::<Vec<_>>(), vec![(45, 0.0, 100), (48, 1.0, 100), (52, 2.0, 80), (55, 3.0, 80)]);
        drop(s);
        // unknown placeholder: nothing changes
        let before = tracks(&a);
        let e = Handler::call(&a, "execute", json!({ "commands": [{ "type": "setTrack", "trackId": "$drums", "patch": { "mute": true } }] })).unwrap_err();
        assert!(e.contains("$drums"), "{e}");
        assert_eq!(tracks(&a), before);
    }

    #[test]
    fn if_revision_refuses_after_someone_else_changed_the_project() {
        let a = agent();
        let rev = call(&a, "get_project", json!({}))["revision"].as_u64().unwrap();
        let add = json!({ "type": "addTrack", "track": { "name": "A", "kind": "instrument" } });
        let r = call(&a, "execute", json!({ "commands": [add], "ifRevision": rev }));
        let rev2 = r["revision"].as_u64().unwrap();
        assert_ne!(rev, rev2);
        let e = Handler::call(&a, "execute", json!({ "commands": [add], "ifRevision": rev })).unwrap_err();
        assert!(e.contains("changed in the meantime"), "{e}");
        assert_eq!(tracks(&a), 1);
        let e = Handler::call(&a, "batch", json!({ "calls": [{ "tool": "execute", "args": { "commands": [add] } }], "ifRevision": rev })).unwrap_err();
        assert!(e.contains("changed in the meantime"), "{e}");
        // read-only tools don't report a revision of their own
        assert!(call(&a, "list_song_templates", json!({})).get("revision").is_none());
    }

    #[test]
    fn execute_groups_tracks_with_placeholders_and_mutes_the_group() {
        let a = agent();
        let r = call(&a, "execute", json!({ "commands": [
            { "type": "addTrack", "track": { "ref": "$v1", "name": "Violin 1", "kind": "instrument" } },
            { "type": "addTrack", "track": { "ref": "$v2", "name": "Violin 2", "kind": "instrument" } },
            { "type": "addTrack", "track": { "ref": "$va", "name": "Viola", "kind": "instrument" } },
            { "type": "addGroup", "group": { "ref": "$violins", "name": "Violins", "gainDb": -3 } },
            { "type": "setTrack", "trackId": "$v1", "patch": { "group": "$violins" } },
            { "type": "setTrack", "trackId": "$v2", "patch": { "group": "$violins" } }
        ] }));
        let g = r["ids"]["$violins"].as_u64().unwrap();
        call(&a, "execute", json!({ "commands": [{ "type": "setGroup", "groupId": g, "patch": { "mute": true } }] }));
        let project = call(&a, "get_project", json!({}));
        assert_eq!(project["groups"][0]["name"], "Violins");
        let s = lock(&a.st().session).unwrap();
        let p = &s.project;
        assert_eq!(p.audible_tracks().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Viola"]);
        assert_eq!(p.track_gain_db(p.tracks.iter().find(|t| t.name == "Violin 1").unwrap()), -3.0);
        drop(s);
        // ungroup: the tracks stay
        call(&a, "execute", json!({ "commands": [{ "type": "removeGroup", "groupId": g }] }));
        assert_eq!(tracks(&a), 3);
        assert!(lock(&a.st().session).unwrap().project.groups.is_empty());
    }

    #[test]
    fn get_project_reads_only_the_asked_parts() {
        let a = agent();
        let r = call(&a, "execute", json!({ "commands": [
            { "type": "addTrack", "track": { "ref": "$a", "name": "A", "kind": "instrument" } },
            { "type": "addTrack", "track": { "ref": "$b", "name": "B", "kind": "audio" } }
        ] }));
        let b = r["ids"]["$b"].as_u64().unwrap();
        let p = call(&a, "get_project", json!({ "trackIds": [b], "fields": ["tempoBpm", "tracks.name"] }));
        let keys: Vec<&String> = p.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 3, "{keys:?}");
        assert!(p["revision"].is_u64() && p["tempoBpm"].is_number());
        assert_eq!(p["tracks"], json!([{ "id": b, "name": "B" }]));
        assert!(Handler::call(&a, "get_project", json!({ "trackIds": [999] })).unwrap_err().contains("999"));
    }

    #[test]
    fn batch_is_one_undo_step_and_reverts_at_the_first_error() {
        let a = agent();
        let r = call(&a, "batch", json!({ "label": "Bass", "calls": [
            { "tool": "create_instrument_track", "args": { "kind": "bass" } },
            { "tool": "execute", "args": { "commands": [{ "type": "setTrack", "trackId": "$0.trackId", "patch": { "gainDb": -6 } }] } },
            { "tool": "execute", "args": { "commands": [{ "type": "addClip", "trackId": "$0.trackId", "clip": { "ref": "$end", "startBeats": 16, "source": { "type": "notes", "lengthBeats": 4, "notes": "E2 2 · G2" } } }] } },
            { "tool": "execute", "args": { "commands": [{ "type": "setClip", "trackId": "$0.trackId", "clipId": "$end", "patch": { "name": "Ende" } }] } }
        ] }));
        assert_eq!(r["undoLabel"], "Agent · Bass");
        assert_eq!(r["results"].as_array().unwrap().len(), 4);
        assert!(r["results"][1].get("project").is_none(), "execute inside a batch without the project");
        assert!(r["ids"]["$end"].is_u64());
        let track = r["results"][0]["trackId"].as_u64().unwrap();
        assert_eq!(r["created"]["tracks"], json!([track]));
        {
            let s = lock(&a.st().session).unwrap();
            let t = s.project.track(Id(track)).unwrap();
            assert_eq!((t.gain_db, t.clips.last().unwrap().name.as_str()), (-6.0, "Ende"));
            assert_eq!(s.undo_label(), Some("Agent · Bass"));
        }
        call(&a, "undo", json!({}));
        assert_eq!(tracks(&a), 0, "one undo step reverts the whole batch");

        // error in call 2: call 1 is reverted, the undo history stays as it was
        let e = Handler::call(&a, "batch", json!({ "calls": [
            { "tool": "create_instrument_track", "args": { "kind": "synth" } },
            { "tool": "execute", "args": { "commands": [{ "type": "setTrack", "trackId": 9999, "patch": { "mute": true } }] } }
        ] })).unwrap_err();
        assert!(e.contains("Call 2 (execute)") && e.contains("unchanged"), "{e}");
        assert_eq!(tracks(&a), 0);
        {
            let s = lock(&a.st().session).unwrap();
            assert!(s.undo_label().is_none());
            assert_eq!(s.redo_label(), Some("Agent · Bass"), "the undone batch stays redoable");
        }
        let e = Handler::call(&a, "batch", json!({ "calls": [{ "tool": "undo" }] })).unwrap_err();
        assert!(e.contains("not allowed"), "{e}");
    }

    #[test]
    fn chat_tools_reserve_peek_and_carry_cancel_requests() {
        let a = agent();
        let st = a.st();
        let task = st.chat.send_user("Bau ein Jazz-Stück", None).unwrap();
        let r = call(&a, "chat_poll", json!({ "timeoutSeconds": 1, "client": "zuhoerer", "leaseSeconds": 30 }));
        assert_eq!(r["auftraege"][0]["id"], task.id);
        assert_eq!(r["leaseSeconds"], 30.0);
        // a second poller doesn't get the reserved task
        assert_eq!(call(&a, "chat_poll", json!({ "timeoutSeconds": 1, "client": "test" }))["auftraege"], json!([]));
        let peek = call(&a, "chat_peek", json!({}));
        assert_eq!(peek["messages"][0]["claimedBy"], "zuhoerer");
        assert_eq!(peek["messages"][0]["status"], "inProgress");
        let since = peek["lastId"].as_u64().unwrap();
        call(&a, "chat_reply", json!({ "text": "Bin dran", "replyTo": task.id, "done": false, "client": "arbeiter" }));
        // the person writes "Stopp": every tool response carries it
        st.chat.send_user("Stopp", None).unwrap();
        let p = call(&a, "get_project", json!({ "fields": ["name"] }));
        assert_eq!(p["cancelRequested"], json!([task.id]));
        let peek = call(&a, "chat_poll", json!({ "peek": true, "sinceId": since }));
        let msgs = peek["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2, "reply and stop request: {msgs:?}");
        assert_eq!(msgs[1]["cancels"], json!([task.id]));
        let e = Handler::call(&a, "get_project", json!({ "trackIds": [12345] })).unwrap_err();
        assert!(e.contains("cancelRequested"), "{e}");
        call(&a, "chat_reply", json!({ "text": "Gestoppt.", "replyTo": task.id }));
        assert!(call(&a, "get_project", json!({})).get("cancelRequested").is_none());
    }

    /// Pitches of the first clip of a track.
    fn pitches(a: &Agent<TestShell>, track: u64) -> Vec<u8> {
        let s = lock(&a.st().session).unwrap();
        match &s.project.track(Id(track)).unwrap().clips[0].source {
            ClipSource::Notes { notes, .. } => notes.iter().map(|n| n.pitch).collect(),
            _ => vec![],
        }
    }

    /// One note per bar (8 bars), the ones at `changed` (bar index → pitch name) replaced.
    fn bars_of_notes(changed: &[(usize, &str)]) -> String {
        (0..8).map(|i| format!("{} 1 @{}", changed.iter().find(|c| c.0 == i).map_or("C4", |c| c.1), i * 4)).collect::<Vec<_>>().join(" · ")
    }

    #[test]
    fn history_tools_checkpoint_variants_goto_and_combine() {
        let a = agent();
        let r = call(&a, "execute", json!({ "commands": [{ "type": "addTrack", "track": { "ref": "$lead", "name": "Lead", "kind": "instrument",
            "clips": [{ "ref": "$a", "startBeats": 0, "source": { "type": "notes", "lengthBeats": 32, "notes": bars_of_notes(&[]) } }] } }] }));
        let (lead, clip) = (r["ids"]["$lead"].as_u64().unwrap(), r["ids"]["$a"].as_u64().unwrap());
        let set_notes = |changed: &[(usize, &str)]| {
            call(&a, "execute", json!({ "commands": [{ "type": "setClipNotes", "trackId": lead, "clipId": clip, "notes": bars_of_notes(changed) }] }));
        };

        // a checkpoint names the current state without changing the project
        let rev = call(&a, "get_project", json!({ "fields": ["name"] }))["revision"].clone();
        let cp = call(&a, "checkpoint", json!({ "name": "Rohfassung" }));
        assert_eq!(cp["checkpoint"]["name"], "Rohfassung");
        assert!(cp.get("revision").is_none(), "no new revision: {cp}");
        assert_eq!(call(&a, "get_project", json!({ "fields": ["name"] }))["revision"], rev);
        let base = cp["step"].as_u64().unwrap();
        let h = call(&a, "history", json!({}));
        let steps = h["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!((steps[0]["label"].as_str(), steps[0]["tempo"].as_f64(), steps[0]["meter"].as_str()), (Some("New project"), Some(120.0), Some("4/4")));
        assert_eq!(steps[1]["step"].as_u64(), Some(base));
        assert_eq!(steps[1]["checkpoints"], json!(["Rohfassung"]));
        assert_eq!((steps[1]["current"].as_bool(), steps[1]["tracks"].as_u64(), steps[1]["notes"].as_u64()), (Some(true), Some(1), Some(8)));
        assert!(steps[1].get("tempo").is_none(), "tempo only where it changes");
        assert_eq!(h["checkpoints"][0]["step"].as_u64(), Some(base));
        assert_eq!(h["variant"]["name"], "Main version");

        // the main version changes bars 5–6 and adds a pad
        set_notes(&[(4, "D4"), (5, "D4")]);
        let pad = call(&a, "execute", json!({ "commands": [{ "type": "addTrack", "track": { "ref": "$pad", "name": "Pad", "kind": "instrument",
            "clips": [{ "startBeats": 16, "source": { "type": "notes", "lengthBeats": 8, "notes": "C3 8" } }] } }] }))["ids"]["$pad"].as_u64().unwrap();
        // a variant from the checkpoint: back to the rough version, the main version stays
        let v = call(&a, "variant_new", json!({ "name": "Refrain", "checkpoint": "rohfassung" }));
        assert_eq!((v["variant"]["name"].as_str(), v["step"].as_u64()), (Some("Refrain"), Some(base)));
        assert!(v["revision"].is_u64(), "the project moved: {v}");
        assert_eq!(pitches(&a, lead), vec![60; 8]);
        assert_eq!(tracks(&a), 1);
        set_notes(&[(5, "G4"), (6, "G4"), (7, "G4")]);
        let list = call(&a, "variant_list", json!({}));
        let vs = list["variants"].as_array().unwrap();
        assert_eq!(vs.iter().map(|x| (x["name"].as_str().unwrap(), x["active"].as_bool().unwrap(), x["steps"].as_u64().unwrap())).collect::<Vec<_>>(), [("Main version", false, 2), ("Refrain", true, 1)]);
        let (main, refrain) = (vs[0]["id"].as_u64().unwrap(), vs[1]["id"].as_u64().unwrap());
        assert_eq!(call(&a, "variant_switch", json!({ "variant": "main version" }))["variant"]["id"].as_u64(), Some(main));
        assert_eq!(tracks(&a), 2);
        call(&a, "variant_switch", json!({ "variant": refrain }));

        // combining onto the active variant: the conflict in bar 6 comes back open, nothing changes
        let rev = call(&a, "get_project", json!({ "fields": ["name"] }))["revision"].clone();
        let c = call(&a, "combine", json!({ "others": ["Main version"] }));
        assert_eq!(c["combined"], false);
        assert_eq!(c["base"]["name"], "Refrain");
        assert_eq!(c["origin"]["checkpoints"], json!(["Rohfassung"]));
        let conflict = format!("track {lead} bar 6");
        assert_eq!(c["open"], json!([conflict]));
        let ds = c["decisions"].as_array().unwrap();
        let by_id = |id: &str| ds.iter().find(|d| d["id"] == id).unwrap_or_else(|| panic!("{id} in {ds:?}")).clone();
        assert_eq!((by_id(&format!("track {lead} bar 5"))["take"].as_str(), by_id(&format!("track {lead} bars 7-8"))["take"].as_str()), (Some("Main version"), Some("Refrain")));
        assert_eq!(by_id(&format!("track {pad}"))["take"], "Main version");
        let open = by_id(&conflict);
        assert_eq!((open["conflict"].as_bool(), open["take"].is_null(), open["bars"].as_str()), (Some(true), true, Some("bar 6")));
        let options: Vec<(u64, &str)> = open["options"].as_array().unwrap().iter().map(|o| (o["variant"].as_u64().unwrap(), o["notes"].as_str().unwrap())).collect();
        assert_eq!(options, [(0, "C4 1 @20"), (refrain, "G4 1 @20"), (main, "D4 1 @20")]);
        assert_eq!(call(&a, "get_project", json!({ "fields": ["name"] }))["revision"], rev, "an open conflict changes nothing");
        assert_eq!(call(&a, "variant_list", json!({}))["variants"].as_array().unwrap().len(), 2);

        // "*" decides every conflict a variant is part of; dryRun only shows it
        let dry = call(&a, "combine", json!({ "others": [main], "choices": { "*": "Refrain" }, "dryRun": true }));
        assert_eq!((dry["combined"].as_bool(), dry["open"].clone()), (Some(false), json!([])));
        assert_eq!(dry["decisions"].as_array().unwrap().iter().find(|d| d["id"] == conflict.as_str()).unwrap()["take"], "Refrain");
        // wrong ids and variants are refused with the valid ones
        let e = Handler::call(&a, "combine", json!({ "others": [main], "choices": { "track 999": 1 } })).unwrap_err();
        assert!(e.contains("unknown decision") && e.contains(&conflict), "{e}");
        let e = Handler::call(&a, "combine", json!({ "others": [main], "choices": { conflict.as_str(): 77 } })).unwrap_err();
        assert!(e.contains("not one of the variants"), "{e}");
        assert!(Handler::call(&a, "combine", json!({ "others": ["Refrain"] })).unwrap_err().contains("Give others"));

        // with a choice: the new variant, the project at it
        // ids compare without case and extra spaces
        let loose = format!("Track  {}", &conflict[6..]);
        let c = call(&a, "combine", json!({ "others": ["Main version"], "choices": { loose.as_str(): main }, "name": "Beides" }));
        assert_eq!((c["combined"].as_bool(), c["variant"]["name"].as_str()), (Some(true), Some("Beides")));
        assert_eq!(c["label"], "Combined: Main version");
        assert_eq!(c["created"]["tracks"], json!([pad]));
        assert_eq!(pitches(&a, lead), vec![60, 60, 60, 60, 62, 62, 67, 67]);
        assert_eq!(tracks(&a), 2);
        let combined = c["step"].as_u64().unwrap();

        // goto: to a checkpoint, to a step; the later steps stay
        let g = call(&a, "goto", json!({ "checkpoint": "Rohfassung" }));
        assert_eq!((g["step"].as_u64(), g["variant"]["name"].as_str()), (Some(base), Some("Beides")));
        assert_eq!(tracks(&a), 1);
        let h = call(&a, "history", json!({}));
        assert!(h["steps"].as_array().unwrap().iter().any(|s| s["step"].as_u64() == Some(combined) && s["redo"] == true), "{h}");
        call(&a, "goto", json!({ "step": combined }));
        assert_eq!(pitches(&a, lead), vec![60, 60, 60, 60, 62, 62, 67, 67]);
        // editing after an undo sets the combined step aside; goto brings it back as a variant
        call(&a, "undo", json!({}));
        call(&a, "execute", json!({ "commands": [{ "type": "setTrack", "trackId": lead, "patch": { "gainDb": -3 } }] }));
        let h = call(&a, "history", json!({}));
        let branches = h["steps"].as_array().unwrap().iter().filter_map(|s| s.get("branches")?.as_array()).flatten();
        let aside = branches.into_iter().find(|x| x["setAside"] == true).cloned().unwrap_or_else(|| panic!("{h}"));
        let g = call(&a, "goto", json!({ "step": aside["tip"] }));
        assert_eq!(g["variantCreated"], true);
        assert_eq!(g["step"].as_u64(), Some(combined));
        // a short table keeps the current step
        let h = call(&a, "history", json!({ "limit": 2 }));
        assert_eq!(h["steps"].as_array().unwrap().len(), 2);
        assert!(h["omitted"]["before"].as_u64().unwrap() > 0);
        assert!(h["steps"].as_array().unwrap().iter().any(|s| s["current"] == true));

        // mistakes say what exists; history moves don't run inside batch
        assert!(Handler::call(&a, "goto", json!({ "step": 9999 })).unwrap_err().contains("not in the history"));
        assert!(Handler::call(&a, "goto", json!({ "checkpoint": "Nope" })).unwrap_err().contains("Rohfassung"));
        assert!(Handler::call(&a, "variant_switch", json!({ "variant": "Nope" })).unwrap_err().contains("Refrain"));
        let e = Handler::call(&a, "batch", json!({ "calls": [{ "tool": "checkpoint", "args": {} }] })).unwrap_err();
        assert!(e.contains("not allowed"), "{e}");
        assert_eq!(call(&a, "batch", json!({ "calls": [{ "tool": "history", "args": { "limit": 1 } }] }))["results"][0]["steps"].as_array().unwrap().len(), 1);
    }

    /// A trio with a pickup (Violin, Viola with two voices, Cello) as a score file and a library of its voices.
    fn agent_with_library() -> (Agent<TestShell>, std::path::PathBuf) {
        let n = |step: &str, oct: u8, d: u32, voice: u32| format!("<note><pitch><step>{step}</step><octave>{oct}</octave></pitch><duration>{d}</duration><voice>{voice}</voice></note>");
        let xml = format!(
            "<score-partwise version=\"4.0\"><work><work-title>Kleines Trio</work-title></work><identification><creator type=\"composer\">Test Composer</creator></identification>\
             <part-list><score-part id=\"P1\"><part-name>Violin</part-name></score-part><score-part id=\"P2\"><part-name>Viola</part-name></score-part>\
             <score-part id=\"P3\"><part-name>Cello</part-name></score-part></part-list>\
             <part id=\"P1\"><measure number=\"0\" implicit=\"yes\"><attributes><divisions>1</divisions><time><beats>3</beats><beat-type>4</beat-type></time></attributes>{p}</measure>\
             <measure number=\"1\">{a}{b}{c}</measure><measure number=\"2\">{d}{e}</measure><measure number=\"3\">{f}{f}{f}</measure><measure number=\"4\">{g}</measure></part>\
             <part id=\"P2\"><measure number=\"0\" implicit=\"yes\"><attributes><divisions>1</divisions></attributes><forward><duration>1</duration></forward></measure>\
             <measure number=\"1\">{v1}<backup><duration>3</duration></backup>{v2}</measure><measure number=\"2\">{v3}</measure></part>\
             <part id=\"P3\"><measure number=\"0\" implicit=\"yes\"><attributes><divisions>1</divisions></attributes><forward><duration>1</duration></forward></measure>\
             <measure number=\"1\">{c1}</measure><measure number=\"2\">{c2}</measure></part></score-partwise>",
            p = n("G", 4, 1, 1),
            a = n("C", 5, 1, 1),
            b = n("E", 5, 1, 1),
            c = n("G", 5, 1, 1),
            d = n("F", 5, 2, 1),
            e = n("E", 5, 1, 1),
            f = n("D", 5, 1, 1),
            g = n("C", 5, 3, 1),
            v1 = n("E", 4, 3, 1),
            v2 = n("C", 4, 3, 2),
            v3 = n("D", 4, 3, 1),
            c1 = n("C", 3, 3, 1),
            c2 = n("G", 2, 3, 1)
        );
        let dir = std::env::temp_dir().join(format!("klangwelt-agent-lib-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kleines-trio.musicxml");
        std::fs::write(&path, xml).unwrap();
        let score = load_score(&path).unwrap();
        let mut idx = klang_music::index::MelodyIndex::new();
        // only the violin is long enough for the melody index – one voice entry stands for the score
        assert_eq!(idx.add_score(&score, &path.display().to_string()), 1);
        let a = agent();
        *lock(&a.st().library).unwrap() = Some(crate::state::Library {
            id: "test".into(),
            name: "Test".into(),
            path: String::new(),
            index: klang_music::diskindex::AnyIndex::Mem(idx),
            packs: Vec::new(),
            has_meta: true,
        });
        (a, dir)
    }

    #[test]
    fn library_scores_are_searched_read_and_imported_by_doc() {
        let (a, dir) = agent_with_library();
        // search: one hit per score, by words of the title
        let hits = call(&a, "search_library", json!({ "text": "trio kleines" }))["hits"].clone();
        assert_eq!(hits.as_array().unwrap().len(), 1, "{hits}");
        assert_eq!(hits[0]["title"], "Kleines Trio");
        assert_eq!(hits[0]["file"], "kleines-trio.musicxml");
        let doc = hits[0]["doc"].as_u64().unwrap();
        assert!(call(&a, "search_library", json!({ "text": "Quartett" }))["hits"].as_array().unwrap().is_empty());

        // overview: voices with numbers and names, bars as printed (pickup = bar 0)
        let r = call(&a, "read_library_score", json!({ "doc": doc }));
        let names: Vec<&str> = r["voices"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Violin", "Viola System 1 · Stimme 1", "Viola System 1 · Stimme 2", "Cello"]);
        assert_eq!((r["bars"].as_u64(), r["pickup"].as_bool(), r["timeSignature"].as_str()), (Some(4), Some(true), Some("3/4")));
        assert_eq!(r["voices"][0]["bars"], json!([0, 4]));
        assert_eq!(r["voices"][3]["bars"], json!([1, 2]));
        assert_eq!(r["voices"][3]["range"], "G2–C3");
        assert_eq!((r["voices"][2]["part"].as_str(), r["voices"][3]["part"].as_str()), (Some("Viola"), Some("Cello")));
        assert!(r["voices"][0].get("excerpt").is_none() && r.get("excerpt").is_none(), "no notes without bars");

        // a bar range: the notes in compact notation, quarters from the start of the bar
        let r = call(&a, "read_library_score", json!({ "doc": doc, "fromBar": 2, "toBar": 2, "voices": ["violin", 4] }));
        assert_eq!(r["excerpt"], json!({ "fromBar": 2, "toBar": 2, "startQuarter": 4.0, "endQuarter": 7.0 }));
        assert_eq!(r["voices"][0]["excerpt"], "F5 2 · E5 1");
        assert_eq!(r["voices"][3]["excerpt"], "G2 3");
        assert!(r["voices"][1].get("excerpt").is_none(), "only the chosen voices");
        let e = Handler::call(&a, "read_library_score", json!({ "doc": doc, "voices": ["Tuba"] })).unwrap_err();
        assert!(e.contains("1 Violin") && e.contains("4 Cello"), "{e}");
        assert!(Handler::call(&a, "read_library_score", json!({ "doc": doc, "fromBar": 5 })).unwrap_err().contains("bars 0–4"));

        // import: one track per part (the viola's two voices together), one undo step
        let r = call(&a, "import_library_score", json!({ "doc": doc, "voices": ["Viola", "Cello"], "fromBar": 1 }));
        let tracks = r["tracks"].as_array().unwrap().clone();
        assert_eq!(tracks.len(), 2, "{r}");
        assert_eq!((tracks[0]["track"].as_str(), tracks[0]["voices"].clone()), (Some("Kleines Trio – Viola (bars 1–4)"), json!([2, 3])));
        assert_eq!(tracks[1]["track"], "Kleines Trio – Cello (bars 1–4)");
        assert_eq!((r["startBeats"].as_f64(), r["endBeats"].as_f64(), r["bars"].clone()), (Some(0.0), Some(8.0), json!([1, 4])));
        assert_eq!(r["created"]["tracks"].as_array().unwrap().len(), 2);
        assert!(r["change"].as_str().unwrap().contains("2 voices"), "{r}");
        let (viola, cello) = (tracks[0]["trackId"].as_u64().unwrap(), tracks[1]["trackId"].as_u64().unwrap());
        {
            let s = lock(&a.st().session).unwrap();
            assert_eq!(s.undo_label(), Some("Import 2 voices from library"));
            let notes = |id: u64| {
                let t = s.project.tracks.iter().find(|t| t.id.0 == id).unwrap();
                let ClipSource::Notes { notes, .. } = &t.clips[0].source else { panic!() };
                notes.iter().map(|n| (n.pitch, n.start_beats, n.length_beats)).collect::<Vec<_>>()
            };
            assert_eq!(notes(cello), vec![(48, 0.0, 3.0), (43, 3.0, 3.0)]);
            assert_eq!(notes(viola), vec![(64, 0.0, 3.0), (60, 0.0, 3.0), (62, 3.0, 3.0)], "both viola voices in one clip");
        }
        // one track per voice on request
        let r = call(&a, "import_library_score", json!({ "doc": doc, "voices": ["Viola"], "split": "voice" }));
        assert_eq!(r["tracks"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), vec!["Viola System 1 · Stimme 1", "Viola System 1 · Stimme 2"]);
        call(&a, "undo", json!({}));
        // the next "movement" after it on the same tracks: one trackId per part
        let ids = json!([viola, cello]);
        let r = call(&a, "import_library_score", json!({ "doc": doc, "voices": [2, 3, 4], "trackIds": ids, "startBeats": 8, "toBar": 1 }));
        assert!(r["tracks"].as_array().unwrap().iter().all(|t| t["newTrack"] == false), "{r}");
        assert!(r.get("created").and_then(|c| c.get("tracks")).is_none(), "no new tracks: {r}");
        assert_eq!(tracks_of(&a), 2);
        {
            let s = lock(&a.st().session).unwrap();
            let t = s.project.tracks.iter().find(|t| t.id.0 == cello).unwrap();
            assert_eq!((t.clips.len(), t.clips[1].start_beats), (2, 8.0));
        }
        call(&a, "undo", json!({}));
        call(&a, "undo", json!({}));
        assert_eq!(tracks_of(&a), 0);
        // mistakes say what's wrong
        assert!(Handler::call(&a, "import_library_score", json!({ "doc": doc, "voices": [1], "trackIds": [1, 2] })).unwrap_err().contains("one per chosen part"));
        assert!(Handler::call(&a, "import_library_score", json!({ "doc": doc, "split": "staff" })).unwrap_err().contains("'part' or 'voice'"));
        assert!(Handler::call(&a, "import_library_score", json!({ "doc": doc, "voices": [1], "trackIds": [999] })).unwrap_err().contains("999"));
        assert!(Handler::call(&a, "read_library_score", json!({ "doc": 4242 })).is_err());

        // analyze_score takes the doc too
        let v = call(&a, "analyze_score", json!({ "doc": doc }));
        assert_eq!(v["titel"], "Kleines Trio");
        assert_eq!(v["voices"].as_array().unwrap().len(), 4);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn tracks_of(a: &Agent<TestShell>) -> usize {
        tracks(a)
    }

    #[test]
    fn tool_calls_mark_the_agent_as_working_but_waiting_polls_do_not() {
        let a = agent();
        call(&a, "chat_poll", json!({ "timeoutSeconds": 1 }));
        let v = a.st().chat_view();
        assert!(v.agent_listening && !v.agent_working);
        call(&a, "get_project", json!({}));
        let v = a.st().chat_view();
        assert!(v.agent_working && v.agent_connected);
    }
}
