# Architecture

Klangwelt lives in two repos: **klangwelt** (this one: `crates/`, `ui/`, browser version,
`tests/unit`, `tests/web`) and **klangwelt-desktop** (Tauri shell `app/`, `tests/e2e`,
release), which includes this repo as the Git submodule `klangwelt/`. Paths such as
`app/src/…` and `tests/e2e/…` refer to klangwelt-desktop.

```
┌ ui/  TypeScript, Web Components, Canvas ─────────────────────────────────────┐
│  transport · tracks · timeline · panels              (signals, no framework) │
│  dock.ts: dock system – Tracks · Clip · Analysis · Sound · Library · Chat    │
│  bridge.ts: one method → command mapping (coreBackend) for both shells       │
│     ├─ Desktop: Tauri IPC (invoke + events)                                  │
│     ├─ Browser: web/host.ts → Web Worker → klang_web.wasm                    │
│     └─ mock.ts: mock backend for UI work (?attrappe)                         │
└─────────────┬──────────────────────────────────────────┬─────────────────────┘
┌ app/  Tauri 2 (shell) ──────────┐   ┌ crates/klang-web  WebAssembly (shell) ───┐
│  Host: Tauri events, folders    │   │  Host: postMessage, /Klangwelt/… (OPFS)  │
│  Desktop only: window, Explorer,│   │  kw_invoke · kw_mcp · files · status     │
│  loading/managing libraries,    │   │  Engine → AudioWorklet                   │
│  MCP server (HTTP), command line│   │  (crates/klang-worklet: mixer)           │
└─────────────┬───────────────────┘   └──────────────────┬───────────────────────┘
┌ crates/klang-app  shared core ─────────────────────────▼───────────────────────┐
│  commands.rs: the one command table (name + JSON) · agent.rs: MCP tools        │
│  AppState: Session (project+undo) · AssetPool (audio, caches) · Engine         │
│  sync_engine(): project → arrangement (audio + live notes) → Engine            │
│  Harmony · Fugue · Rhythm · Song · Sound · Analysis · Export · Projects        │
│  host.rs (platform) · vfs.rs (files) · clock.rs (clock)                        │
└────────────────────────────────────────────────────────────────────────────────┘
┌ crates/klang-core ┐ ┌ crates/klang-dsp ───┐ ┌ crates/klang-engine ┐ ┌ crates/klang-music ┐
│ Project model     │ │ FFT, YIN, notes,    │ │ Renderer (mixer,    │ │ MusicXML/MXL,      │
│ Command + apply   │ │ onsets, tempo,      │ │ reverb, metronome), │ │ melody index,      │
│ Undo/Redo, JSON   │ │ WSOLA, resampling   │ │ Engine: cpal thread │ │ theory, analysis   │
│                   │ │ plucker, drums,     │ │ (desktop) or        │ │ folder index       │
│                   │ │ bass, sound metrics │ │ worklet (browser)   │ │                    │
└───────────────────┘ └─────────────────────┘ └─────────────────────┘ └────────────────────┘
```

Desktop and browser share the core; only the shells differ
(details and what is missing in the browser: [WEB.md](WEB.md)). A new UI
command belongs in `crates/klang-app/src/commands.rs`, a new agent tool in
`crates/klang-app/src/agent.rs` – then it is available on both platforms.

## Principles

* **The project state lives in the Rust core.** The UI displays a snapshot
  and sends commands. Every command (`klang_core::Command`) is JSON, e.g.
  `{"type":"setTrack","trackId":3,"patch":{"gainDb":-6}}`. The agent gets
  exactly this interface via MCP.
* **Undo/redo** comes about automatically: for every command, `apply` returns
  the inverse command. Control gestures merge into a single step (`merge`).
* **Real-time rules:** Nothing is allocated, locked or freed in the audio
  callback. New arrangements arrive via a lock-free ring buffer; old ones go
  back via a second ring buffer to be freed – so do the clips of finished
  live voices (`Garbage`). Voice slots, string delay lines and sound shaping
  chains are allocated when the mixer is created.
  * On Windows the engine sets the audio thread to
    `THREAD_PRIORITY_TIME_CRITICAL` itself (cpal 0.15 tries to, but passes a
    thread ID instead of a handle). Without this, up to 170 ms of audio per 3 s
    went missing under full load (pre-rendering on all cores); measurement:
    `cargo test --release -p klang-engine --test audio_unter_last -- --ignored --nocapture`.
* **Jumps without clicks:** Every change during playback sends a new
  arrangement (note clips are played live – see below –, on a tempo change
  only audio clips are re-stretched). The renderer does not switch hard but crossfades linearly
  over 15 ms (`FADE_SECS`): the old state keeps sounding at its position and
  fades out, the new one fades in. Identical content stays unchanged
  (sum 1), different content does not jump.
  * Tempo and arrangement travel as **one** message
    (`RtMsg::Arrangement { arrangement, tempo }`, in the browser `wire::Kind::Arrangement`
    with a tempo header), so that no block runs with the new tempo but the old sound.
  * Jumping (`Seek`) during playback is crossfaded too; up to eight
    transitions can overlap. The arrangement that has faded out goes back via
    the return channel to be freed – here too without allocation in the mixer
    (unit test with a counting allocator).
  * Measured with the demo project (`tempo_sprung_messen` in `state.rs`):
    with hard switching the signal jumped by up to 0.84 per tempo step;
    with crossfading, every sample step stays within 0.002 of what the old
    or new signal does anyway.
  * Live voices belong to the state they were started in and fade out
    with it; the new state picks up the notes sounding at its position.
  * The tempo field does not queue up steps: while a step is running,
    only the newest value is remembered and sent afterwards.
* **Looping:** The engine keeps the loop in beats (`RtMsg::Loop`), so that it
  survives tempo changes.
  * The callback splits a block at the loop end and jumps to the start.
  * Starting past the end plays on unhindered.
  * The app (`AppState::apply_loop`) passes on the selected range, otherwise
    the whole piece (up to the end of the last clip, rounded up to whole bars).
    This is recalculated after every project change.
  * On/off and range are in the snapshot (`engine.loopOn/loopRange`),
    not in the project.
* **Preview** (library, chords, notes) runs alongside the transport. The
  status reports `previewing` while it sounds; this turns the library's listen
  button into a stop button, which switches back afterwards.
* **Expensive processing offline:** Tempo adjustment (WSOLA) and pitch are
  precomputed and cached when they change, not in the audio thread.
  * Whatever is missing after a change (stretched audio clips) is
    rendered in parallel by `prerender` on the desktop; large resampling jobs
    are split across several cores by `stretch::resample` itself (same result
    as in one piece). In the browser both run sequentially.
  * The resampler advances the sinc and window angles from filter tap to
    filter tap instead of computing `sin`/`cos` at every tap.
  * A tempo step with the demo project (two 48 kHz audio clips, one
    melody) took 335 ms at 44.1 kHz output, now about 40 ms; at
    48 kHz 65 ms, now 23 ms. Measurement:
    `cargo test --release -p klang-app -- --ignored --nocapture tempo_schritt`.
* **Note clips are played live** (`klang_dsp::live`, `klang_engine::live`):
  nothing is rendered beforehand – generating a song, a tempo change or an
  edit take effect immediately, also in the browser.
  * The core prepares each note clip once (`LiveNotes`: notes in beats,
    instrument as `Patch`, sound shaping as `Shaping`, what a note needs
    from its neighbours – bass glide, choked hi-hat –, the organ's
    wavetables). The preparation holds for every tempo and is cached per
    clip (`AssetPool::live`); `NoteClip` places it on a track.
  * The mixer starts a `Voice` per note in the block where it begins
    (positions in beats at the current tempo). Voices write into their
    track's running sound shaping chain (`FxChain`: warmth → shimmer →
    roughness → color → distance → width), which feeds the mix and the reverb.
    512 voices (96 of them with plucked strings), 128 chains; when they run
    out, the voice sounding longest makes room.
  * A track that pauses stops its chain; when it continues, the chain catches
    up on the pause (delay lines empty, modulation on).
  * Starting or jumping into the middle of notes picks them up
    (`Live::chase`): envelopes continue at their place, plucked strings lose
    the energy they would have lost meanwhile; picked-up voices fade in over
    4 ms. Stop, end of the piece and a loop jump let the voices fade out over 15 ms.
  * The voices are the instruments' offline functions sample for sample
    (tests compare them: difference below 10⁻⁶, the chain below 10⁻⁴);
    previews, the wave view and the export (`render_offline` runs the same
    `Renderer`) use them too.
  * In the browser a note clip travels once to the AudioWorklet
    (`wire::Kind::Notes`, organ tables as buffers); arrangements refer to
    it by its ID.
  * **When live playback can't keep up, clips are pre-rendered.** The mixer
    reports the share of its real-time budget it needs (`EngineStatus::load`;
    desktop: time of the audio callback, browser: the worklet measures its
    processing time with `Date.now` and whether its blocks come late over a
    second). Above 80 % for 300 ms while playing, `AppState::tick` (called by
    the shells every 20–33 ms) picks the most expensive note clip that sounds
    within the next 10 s and renders it step by step (`StepRender`: the same
    voices, 6 ms per tick, so commands and status keep flowing). Then it plays
    from its buffer like an audio clip (`AssetPool::prerendered`, keyed with
    the tempo). Below 60 % no further clip is taken. A new tempo or an edit of
    the clip plays it live again. The status shows `load` and `prerendered`;
    the command `simulate_load` lets tests trigger it.
  * Measured with the five song templates (`live_song_messen` in
    `state.rs`): generating takes 2–8 ms (before: 1.5–1.9 s on 8 cores);
    live mixing needs 2–5 % of the real-time budget, the longest
    block 18 % (native, blocks of 128 frames).
* **No third-party DSP libraries:** FFT, YIN, onset detection,
  time stretching, resampling and synthesis are written in-house and tested.

## Timeline (`ui/src/components/timeline.ts`)

One canvas for ruler and tracks; gestures are a `Drag` state:
* `pan`: dragging the ruler moves `viewport.scrollX`; without movement the
  click sets the playback position.
* `range`: Shift/Ctrl selects a time range; the grid is beats, or bars when
  zoomed far out. On release it goes to the core via `set_loop_range`.
* `marquee`: selection rectangle on an empty lane. `noteSel` only holds notes
  of one clip: the clip under the starting point, otherwise the one with the most hits.
* `notes`: moving or resizing. A plain click never places a note: Alt+click
  does (dragging sets the length), and so does a double-click on an empty lane.
  Placed next to a clip, it goes into the nearest note clip on the track, with
  a start position relative to that clip's beginning, negative if need be.

**Note entry** (`ui/src/noteinput.ts`): `addNoteAt` adds one note at a project
position – into the clip there, else into the nearest note clip (it grows like
below), else into a new clip – as one `batch` command named after the note.
The double-click and the keyboard entry mode use it. Keyboard entry (`entry`:
track and position, null = off) is switched on next to the piano of a piano
roll; `handleEntryKey` runs in app.ts before every other shortcut. Keys count
by position (`KeyboardEvent.code`), the piano labels them with the letters of
the keyboard layout (`navigator.keyboard.getLayoutMap`, else US letters). Typed
notes queue up so that each one is added to the snapshot of the one before.
The shortcut line (`.keys-hint`) sits right below the last track; while
typing it lists the mode's keys.

**The clip grows along:** `clipSpan` (in `ui/src/editmath.ts`, pure and tested
in Node) computes how far the clip has to grow in whole bars: to the left at
most to the project start, to the right up to the last note.
* While dragging, the timeline already draws the outline as a dashed line.
* On release, `#commitNotes` sets the clip start and the notes in one
  `batch` command, i.e. one undo step. If the clip grows to the left, the
  notes shift by the same amount.

**When moving** notes or clips, `#moving` describes range,
position and distance. From it the timeline draws:
* prominent bar and beat lines,
* the snap grid in the track,
* the new range in the ruler,
* a position label – additionally as the attribute `data-drag-info` on the element,
  for tests.

The library selects voices as a list (Ctrl+click, "All voices" checkbox).
`library_play` and `library_to_track` take pairs (part, voice) and create
one track per voice.

**Context menu** (`ui/src/ctxmenu.ts`): depending on the target, a right-click
builds a list of entries, subheadings and rows with radio buttons (grid
1/4 · 1/8 · 1/16 · view grid). Clicking an unselected note selects it first;
Esc, clicking elsewhere, the mouse wheel or switching windows closes the menu.
Entries carry `data-act` (tests, context help).

**"♪ → Notes" button** on audio clips: drawn onto the canvas; the position of
each button is collected while drawing (`#noteButtons`) and checked first on a
press. `noteButtonRect(clipId)` returns it in window coordinates
(for tests).

**What Delete removes:** selected notes; otherwise the time range, if it was
selected last (`rangeFocus` in `store.ts`; every new selection resets it, and
selecting a range clears the note selection) or nothing else is selected;
otherwise the clip or track.

**Dragged-in audio files** (`ui/src/filedrop.ts`): the timeline translates
a window position into a target (`dropTargetAt`: track or new track, beat
snapped to the clip grid) and draws it while dragging (line, label, also as
`data-drop-info`).

**Only swap icons in buttons when they change.** A redraw on press
(e.g. because the track gets selected at that moment) must not replace the
element under the mouse. Otherwise the browser delivers no `click` – the first
click would be lost.

## Track editing (`klang_core::edit`, `crates/klang-app/src/convert.rs`)

**Deleting a time range and closing the gap.** `edit::delete_range` plans the
deletion of `[start, end)` as a time mapping `collapse`: everything up to
`start` stays, everything inside the range falls onto `start`, everything after
it moves forward by the length. Clips and notes are converted with the same
mapping – clips entirely inside the range disappear, clips after it only get a
new start, notes across an edge get shorter (length < 1/128 beat is dropped).
* Audio clips: the core only computes which parts of the source (seconds,
  via `audio_beats_per_sec`: source tempo with warp, otherwise project tempo)
  remain (`AudioKeep`). The caller does the cutting (`AppState::delete_range`):
  it concatenates the parts as a **new file** in the AssetPool (3 ms fade at
  each cut edge) and replaces the clip (same ID) via `removeClip` +
  `addClip`. The old file stays in the pool – undo points back to it;
  only files in use are saved.
* Everything goes into one `batch` command → one undo step; afterwards
  the app clears the selection. Without closing the gap (leaving a hole) only
  part of this would be needed – deliberately not offered, see README.

**Quantize to the grid.** `edit::quantize_notes` computes in project time
(clip start + note start), so that clips that begin at an offset still land on
the beat. The start goes to the nearest grid point (never before the clip
start); with lengths, the end too (at least one grid step). Two notes of the
same pitch on the same start merge (the longer one stays). The function returns
the new indices of the edited notes so that the UI keeps the selection. The UI
(`quantize_notes`) and the agent use the same implementation.

**Recording → notes** (`AppState::audio_to_notes`): for each audio clip, one
note clip at the same position (length rounded to whole bars), in a new
instrument track directly below the audio track; the track's pitch shift is
taken into account.
* Melody: notes detected by the analysis, quantized to sixteenths; if the
  median lies below C (48), it becomes a bass track. `melody_to_track` (Clip
  panel, agent) goes through this.
* Rhythm (`klang_dsp::hits`): for each hit found by the analysis, it measures
  what is **added** in the low (< 150 Hz), mid and high (> 5 kHz) band compared
  with the equally long stretch before it – so a decaying kick drum does not
  color the next hi-hat. Lots of lows = kick drum, lots of highs without lows
  = hi-hat, otherwise snare. A kick drum with as many highs as the recording's
  other hi-hats gets the hi-hat added. Because kick drums get lost next to loud
  cymbals in the broadband flux of the general onset detection, `low_onsets`
  additionally looks for energy jumps (≥ 6 dB in 20 ms) in the low band. The
  grid is sixteenths, keeping the loudest hit per drum and step.

## Importing audio files (`crates/klang-app/src/audiofile.rs`, `ui/src/filedrop.ts`)

* **Decoders without third-party libraries** (except libopus, which is built
  for export anyway). The format is detected from the content: `RIFF/WAVE`,
  `FORM/AIFF|AIFC`, `fLaC` (also behind an ID3 tag), `OggS` with
  `OpusHead`. WAV: PCM 8–32 bit, floating point 32/64, WAVE_FORMAT_EXTENSIBLE;
  truncated data chunks are tolerated. AIFF: 80-bit sample rate, PCM
  big endian, `sowt`, `fl32`/`fl64`. FLAC: STREAMINFO, frame headers with all
  block sizes/rates, constant/verbatim/fixed/LPC subframes with wasted
  bits, Rice partitions with escape, all stereo modes; a damaged frame
  is skipped (search for the next sync code). Opus: in-house
  Ogg reader (first logical stream), pre-skip and output gain from
  OpusHead, length from the last granule position.
* Detected and rejected with a reason: MP3 (frame sync word, ID3), AAC/M4A
  (`ftyp`, ADTS), Ogg Vorbis, FLAC in Ogg, WMA (ASF GUID), compressed WAVs.
* `AppState::import_audio` creates one file in the AssetPool and one clip per
  file (warp off = original tempo) – all in one `batch`. Read errors of
  individual files come back as a list; only if no file was readable is the
  whole operation an error.
* **Dragging:** Tauri reports `tauri://drag-enter` (paths, position),
  `tauri://drag-over` (position), `tauri://drag-drop` and `tauri://drag-leave`;
  the positions are physical pixels (`bridge.ts` divides by
  `devicePixelRatio`). `filedrop.ts` accepts audio files, the library
  (`library.ts`) MusicXML and folders – both listen to the same event.
  Without a suitable target, the message says what belongs where.

## Export (`crates/klang-app/src/export.rs`, `crates/klang-app/src/flac.rs`, `crates/klang-opus`)

* **Rendering:** `AppState::export_source` returns the project, the selected
  range and a *fork* of the AssetPool (sounds shared, separate
  render cache) – the export evicts nothing that playback currently
  needs. `klang_engine::render_offline` runs the same `Renderer` as
  the real-time engine block by block (live voices, mixing, reverb, soft
  limiter) – so offline it sounds exactly like during playback. Always 48 kHz.
* **Length:** the selected range exactly; otherwise up to the last clip, plus
  the tail until it drops below −80 dB (at most one bar + 6 s).
* **WAV:** 44-byte header, 24 bit or 16 bit with triangular dither.
* **FLAC:** blocks of 4096 frames; per channel the shortest of constant,
  raw or fixed predictor (order 0–4) with Rice partitions
  (order up to 8, parameter estimated from the sums, escape for outliers);
  per block the best stereo decomposition (L/R, L/S, R/S, M/S). STREAMINFO with
  MD5 (in-house implementation), VORBIS_COMMENT with title. A verification
  decoder in the tests reads everything back bit-exactly.
* **Opus:** `crates/klang-opus` builds libopus 1.5 (sources in `opus/`,
  file list from its `*_sources.mk`) with the `cc` crate – floating point,
  without CMake, without SIMD and neural networks. On top of that, an in-house
  Ogg container following RFC 7845: OpusHead (pre-skip = encoder lookahead),
  OpusTags, pages with CRC-32, granule of the last page = pre-skip + length, so
  that players play exactly up to the last sample. 20 ms packets, VBR,
  complexity 10, signal "music". In the browser the same libopus, compiled to
  WebAssembly with clang (without libc; details in
  [WEB.md](WEB.md#libopus-in-the-browser)).
* **Stems:** for each audible track, a sub-project with only that track – same
  start, same length as the mix.
* **MIDI:** type 1, 480 PPQ; conductor track (name, tempo, time signature),
  then for each instrument track its name, GM program and notes (track pitch
  shift included, notes truncated at the clip end).
* e2e (`tests/e2e/export.test.mjs`) checks the files themselves and has the
  browser's decoder open FLAC and Opus – an independent cross-check.

## Audio devices (`crates/klang-engine`, `AppState::set_audio_device`)

Output and input device can be chosen while running – in the top bar
(speaker chip, microphone button), via Tauri (`audio_devices`,
`audio_set_device`) and via MCP (`audio_devices`, `set_audio_device`).
* **Devices** are known to cpal only by name; that is what Klangwelt stores.
  `None` means system default. Listing and opening run in the engine thread
  (`Control::Devices`, `SetOutput`, `SetInput`) – all cpal streams live there
  anyway, because they are not `Send`.
* **Switching output** rebuilds the stream: open the new device (with a
  fresh callback and fresh ring buffers), stop the old stream, give the new
  callback the state of the old one, start playing. The engine thread keeps
  track of the state in a `Mirror`: it sees every message to the callback
  (tempo, metronome, loop, stop point, arrangement). Position and
  "playing" come from the atomics; the position is converted to the new
  sample rate, so it stays the same in beats. A running preview ends.
* **Different sample rate:** the arrangement is rendered for one rate; the
  `Mirror` only passes it on if the rate is the same. `set_audio_device` then
  calls `sync_engine()`; the render keys contain the rate, so everything is
  re-rendered for the new device. `EngineInfo` (device, rate, choice) is
  maintained by the engine thread behind a `Mutex`; the rate that goes with the
  position is additionally kept in the atomic `Shared::sample_rate`.
* If the new device does not play, the engine returns to the previous one.
  Switching is blocked during a recording. The input device is only opened at
  the next recording start (`InputCapture::start`).
* **Remembered** in `einstellungen.json` (`ausgabegeraet`, `eingabegeraet`;
  `null` = system default). At startup (`AppState::apply_audio_settings`, in
  `setup` before the first snapshot) a remembered device is only used
  if it is connected. Otherwise the system default stays,
  `engine.outputNotice`/`inputNotice` in the snapshot explain why, and the
  UI briefly reports it at startup. The choice stays remembered: if the
  device is plugged in again at the next start, it applies again.
* e2e: `tests/e2e/audiogeraete.test.mjs` (menus, choice during playback,
  restart, missing device, MCP); the mock backend (`ui/src/mock.ts`) supplies
  sample devices for the browser preview.

## Tests

* **Rust:** `cargo test --workspace` tests core, DSP, engine (loop,
  preview), music theory and library.
* **UI, pure:** `tests/unit/*.test.mjs` tests modules from `ui/dist` without
  a DOM, using `node:test`.
* **End to end:** `tests/e2e/*.test.mjs` tests the real app.
  * `harness.mjs` starts it in isolation for each file: temp folders for
    `KLANGWELT_BIBLIOTHEK` and the WebView2 profile, free ports for CDP and
    MCP. Then it connects via the Chrome DevTools Protocol.
  * In the window it provides `T`: `store.js`/`pianoroll.js` of the same
    module instance (dynamic `import`), `waitFor`, `$`.
  * It sends real mouse and keyboard events (`Input.dispatch*`) with
    modifiers, and JSON-RPC to the MCP server.
  * `noteGeo` computes screen coordinates from the app's geometry functions,
    so that tests do not depend on pixels.
  * Every file ends with "no errors in the window" (exceptions and
    `console.error`).
  * **Background:** The main window starts invisible and unfocused
    (`visible`/`focus: false` in `tauri.conf.json`). `setup` normally shows it
    and brings it to the front.
    * With `KLANGWELT_HINTERGRUND=1`, `setup` places it off-screen,
      without a taskbar entry, and shows it with `SW_SHOWNOACTIVATE`.
    * `open_project` then does not bring it to the front either.
    * So that WebView2 still paints, the test harness disables occlusion
      detection and background throttling
      (browser switches).
  * No third-party packages – Node ships with a test runner and WebSocket.
  * Dropped files: `audioimport.test.mjs` writes WAV files (sung
    melody, drum bar; `audiofiles.mjs`) and fires the same
    Tauri events as the operating system (`tauri://drag-enter` …
    `tauri://drag-drop` via `event.emit`).
* **Browser version:** `tests/web/*.test.mjs` (same harness, `tests/web/harness.mjs`):
  static server for `ui/dist-web` and headless Edge, muted
  (`--mute-audio`), with its own profile – see [WEB.md](WEB.md#tests).
* **Desktop performance:** `tests/perf/desktop.mjs` measures commands over the
  real IPC (medians), for comparison before and after reworking the core.

## Languages (i18n)

The app speaks English, German, French, Italian, Spanish, Dutch and Polish.

* **Keys are the English texts in the code.** `t("Save")` returns the text in
  the current language. Placeholders: `t("Track {name}", { name })`. Plurals:
  `plural(n, "one note", "{n} notes")` – the key is the plural form, `{n}` is
  always set, and a dictionary entry may be an object with the forms of
  `Intl.PluralRules` (`{ one, few, many, other }`, e.g. for Polish). `tk("…")`
  marks texts in tables as keys; they are translated when shown, with
  `t(value)`.
* **Identifiers that are shown** – preset names, envelope names, facet values
  of the score index, rhythm genres … – are marked with `tid("…")` / `tid!("…")`.
  Their value never changes, because it is persisted (projects, saved sounds,
  the index on disk), compared, or sent back by the UI or the agent; for
  historical reasons these values are German. Every dictionary except German
  translates them for display, so `i18n/en.ts` contains only identifiers.
* **`ui/src/i18n.ts`:** language list (`LANGUAGES`), detection (first entry of
  `navigator.languages` that is supported, otherwise English), the remembered
  choice (`localStorage["klangwelt.language"]`; only an explicit choice is
  stored, not the detected language), `t`, `plural`, `tk`, `tid`. Fallback
  chain: chosen language → English → the key itself.
* **Start order:** `main.ts` awaits `initI18n()` (loads only the chosen
  dictionary, plus English as fallback – not for German, whose identifiers are
  German already – with a dynamic `import()`), hands the dictionary to the core
  (`set_language`) and only then imports `app.ts`. So every module may call
  `t()` at load time, also in module-level tables, and the core already answers
  in that language while the panels are built. `document.documentElement.lang`
  follows the language. A `load` listener added during the app's start may never
  fire (the event is over by then).
* **Before the UI starts** the core has no dictionary: messages it creates at
  that moment and stores (desktop: the one-time library migration notice, an
  MCP port error) stay English. Notices that are built when they are read
  (missing audio device, engine errors) follow the language.
* **Dictionaries:** `ui/src/i18n/<code>.ts`, one object per language. The UI
  owns them for the core as well: at start it sends the merged dictionary
  (flat, plural forms as `key\u001fone` …) to the core with the command
  `set_language` (same command table on desktop and in the browser).
* **Core:** crate `klang-i18n` – `tr!("Track {id} not found", id = id)`,
  `trn!(n, "one bar", "{n} bars")`, `tk!("…")`/`tid!("…")` + `translate(…)`.
  A global dictionary, set by `set_language`; without one (Rust tests, command
  line) everything is English. Error messages, undo labels, chord/key names,
  explanations and notices go through it. Tool descriptions for the agent
  (`agent.rs`) are not translated – they are read by the AI, not by the user.
* **UI code that compares texts from the core** compares with `t(key)` – the
  core uses the same dictionary, so both sides agree in every language.
  Better still, the core sends a stable field (e.g. `FugueSection.kind`,
  `SuggestResult.reindex`).
* **Note and chord names:** the core spells them the German way (B = B♭,
  H = B, Cis/Es, minor in lower case: "fis", "h°7"; keys like "a-Moll") – that
  is its data and input format (song chords, melody search, the agent). For
  display, `ui/src/spelling.ts` (names from the core's data) and
  `theory::display_name` (names inside the core's messages) convert: letters
  with ♯/♭ in English, French, Italian and Spanish, Dutch spelling in Dutch
  (B = B natural, Bes = B♭), unchanged in German and Polish. Names typed into
  the song's key and chords and the melody search go back to German
  (`keyInput`, `chordsInput`). The chord track is created as "Chords" in the
  chosen language; "Akkorde" (older projects) is recognised as well.
* **Switching** (settings menu, gear in the transport bar): store the choice
  and reload the UI. Desktop: the core keeps its state (only labels created
  earlier, e.g. old undo steps, stay in the previous language). Browser: the
  core lives in the web worker and is restarted; with unsaved changes the
  existing `beforeunload` question appears first.
* **Key list:** `ui/scripts/i18n-keys.mjs` reads every `t`/`plural`/`tk`/`tid`
  call in `ui/src` and every `tr!`/`trn!`/`tk!`/`tid!` in `crates/` (keys must
  be string literals). `--missing=fr --out=todo.json` lists what a language
  lacks, `--merge=fr --from=done.json` writes translations into
  `ui/src/i18n/fr.ts`; `--root=../klangwelt-desktop/app/src` adds the desktop
  shell's keys. `tests/unit/i18n.test.mjs` fails if a dictionary lacks a key or
  uses a placeholder the call doesn't pass.
* **Adding a language:** an entry in `LANGUAGES`, then `--missing=xx`,
  translate, `--merge=xx` – and a branch in `klang-i18n`'s `category` if its
  plural rules differ from English.
* **Tests:** Rust tests run without a dictionary and assert English texts. The
  browser and desktop test harnesses start the browser/WebView with
  `--accept-lang=de-DE,de`, so the end-to-end tests see the German UI they
  were written for; `tests/web/i18n.test.mjs` covers detection, switching,
  reloading and the core's messages.

## Context help and intro tour

* **`ui/src/help.ts`:** directory of `[selector, topic]`. For an element, the
  entry that matches the element or its nearest ancestor applies (the
  deepest wins; with several on the same element, the first). A topic
  can be a function that chooses by mouse position – that is how the
  timeline tells ruler and tracks apart on the same canvas. Elements without a
  distinguishing class carry `data-help="…"`. A deeper tooltip without its
  own entry appears above the text of the surrounding panel.
* **`components/help.ts`** (`<kw-help>`, dock panel "hilfe"): tracks
  `pointermove` (capture, once per frame) only while the panel is visible
  and not pinned, and never over itself. F1 hooks into the
  capture phase on `window`, so that fields that intercept keys do not
  swallow it.
* **Dock:** `PanelDef.startClosed` keeps a new panel closed in saved
  layouts; `PanelDef.float` opens it as a floating panel if it has no
  remembered place. `emphasize(id)` temporarily gives a stack at least
  62% of its split (tour) – what gets saved is still the actual sizes.
* **`components/tour.ts`:** steps with panel, target and sticky notes. The hole
  is a frame with a huge shadow; first the sticky notes are placed
  (avoiding each other), then the large note looks for the first position
  without overlap (beside, below, above, inside the target, center). Panel
  requests are batched by the signal system – so when restoring, they happen
  one after the other. In an empty project the tour loads the demo; steps have
  `prepare` (selection, analysis, playback position – before showing, aborted
  if the user has paged on in the meantime) and `after` (selection lists in the
  visible panel, help text). At the end it takes the demo back via undo – only
  if the last step is still called "Load demo".

## UI: dock system

`ui/src/dock.ts` is a standalone dock system (without app knowledge);
`ui/src/layout.ts` registers Klangwelt's panels and defines the
default layout.

* **Layout = tree:** splits (`split`, row/column, size fractions) and
  stacks (`stack`, tab groups), plus floating panels. After every change
  the tree is normalized (empty stacks removed, single children pulled up,
  splits in the same direction merged) and saved as JSON in `localStorage`
  (`klangwelt.dock.v1`, validated on load).
* **Contents are preserved:** every panel is created once
  (`<kw-panel tab="…">` or tracks + timeline) and only moved when the layout
  changes; switching tabs only toggles visibility.
* **Dragging:** target cross in the panel under the pointer, edge markers,
  insertion marker in tab bars, otherwise floating. A preview rectangle shows
  the result.
* **Close/open:** a closed panel remembers its tab neighbors, its neighbor
  and the side (or the edge or the floating position) and returns there.
* **Wiring:** `showPanel(id)` (store) brings a panel to the front – e.g.
  Analysis after recording, Agent via the agent button. `visiblePanels`
  says which panels are currently visible (unread dot on the Agent tab).
* Narrow panels: container queries switch Sound, Analysis and Library
  to one column and make the chat bubbles wider.

## Project files

* `crates/klang-app/src/project.rs`: `save` / `load` for `*.klangwelt` (ZIP via the
  `zip` crate). `projekt.json` holds `{format: "klangwelt-projekt", version: 1,
  project, assets}`; `audio/<asset-id>.wav` contains only the recordings that
  a clip still uses (in-house WAV reader/writer: float32 or PCM16,
  multichannel is averaged down to mono). Audio is stored uncompressed, JSON
  compressed. Writing goes to `*.teil`, then `sync_all` and a rename.
* `AppState` remembers `project_file` and a hash of the project JSON at the
  last save/open; `dirty` in the snapshot is the comparison with it. Undo
  therefore stays honest: undoing back to the saved state removes the
  dot again.
* Opening replaces the asset pool (`AssetPool::replace_all`, waveforms and
  analyses are recomputed) and starts a fresh undo history.
* Closing: `on_window_event(CloseRequested)` prevents closing when there are
  unsaved changes and sends `close-requested` to the UI;
  the UI asks and calls `app_close` (sets `force_close`, quits the app).
* Dialogs (`ui/src/components/fileops.ts`, `modal()` in `dom.ts`) are custom
  HTML dialogs, not system dialogs, and therefore need no dialog plugin.

## The four areas

### a) Multitrack, tempo, pitch, sync
* Positions and lengths in **beats**, not seconds → a tempo change
  shifts everything consistently.
* Audio clips have a **source tempo**. With `warp` they are stretched to
  the project tempo via WSOLA – so rhythms of different tracks run in sync
  (demo: 96 BPM and 128 BPM both run at 110 BPM).
* "Detect tempo" estimates the source tempo via onset autocorrelation.
* Pitch per track in semitones (stretching + resampling, tempo stays).

### Note view (piano roll)
* Stored per track (`Track.view`: `compact` | `notes`). Switching is a
  pure view change and deliberately creates no undo step.
* Editing via **one** command: `setClipNotes` replaces a clip's note list
  (sorted, validated, undoable). During a mouse gesture, drawing happens
  only locally; only on release does a command go to the core – this keeps
  the undo history clean, and the engine renders only once.
* `ui/src/pianoroll.ts` holds the geometry, the visible pitch range per track
  and the note selection; track headers (keyboard) and timeline share this state.
* When placing and moving, the note is played via the engine's preview voice
  (`preview_note`).
* Pitch axis: setting `highOnTop` (gear icon; default: low notes on top like
  a rotated keyboard). All conversions go through `pitchOfLane`,
  `laneOf`/`laneY`, `pitchAt`, `pitchDelta` and `wheelPitchStep` in
  `ui/src/pianoroll.ts`; keyboard, timeline, analysis and library redraw
  when it changes. Per track
  a `PitchView` (lowest note + number of semitones); the lane height follows
  from it. On first display the range is fitted to the content (also kick drum
  to snare); after that it stays stable.
* Audio in the note view: melody notes (YIN, from 40 Hz), hits with a
  pitch (`klang_dsp::pitch::hit_pitches`: per onset a 1.2 kHz low-pass,
  YIN down to 28 Hz, energy-weighted median) and unpitched hits in the
  rhythm strip. Short notes that coincide with a hit are
  only shown as a hit.

### Expression of individual notes
* `klang_core::Note` carries `velocity`, `vibrato` (semitones 0–1), `brightness`
  (−1…+1) and `bend` (−12…+12 semitones of slide-in); neutral values are
  omitted from the JSON. `setClipNotes` checks the limits.
* `klang_dsp::synth::NoteSpec::pitch_offset(t)` returns the pitch curve
  (slide-in with a soft landing, 5.5 Hz vibrato with delayed onset).
  The plucker reads its delay line with a variable length
  (buffer sized for the lowest note), the bass modulates the oscillator frequency;
  brightness shifts the loop filter or the low-pass respectively, and for drums
  the click, sharpness and sheen. Velocity also makes the sound harder
  (excitation or filter envelope respectively).
* UI: `ui/src/expression.ts` (ranges, labels,
  expression marks), expression lane and Alt gestures in the timeline,
  expression marks and controls in the Clip panel. Gestures are drawn locally
  and committed on release as one `setClipNotes` (or a `batch` across several
  clips).

### b) Recording and cleanup
1. Microphone → ring buffer → the engine thread collects the samples.
2. YIN pitch curve, spectral-flux onsets.
3. Segmentation into notes (median smoothing, splitting at jumps/onsets).
4. Key (Krumhansl-Kessler); out-of-scale notes that were sung imprecisely
   are pulled onto the scale.
5. "Melody → instrument track" or "♪ → Notes": quantization to
   sixteenths; "Convert to rhythm track": hits as drum notes
   (see "Track editing").

**Live analysis while recording** (`analysis::LiveAnalysis`): the same steps,
measured as the samples come in, so the Analysis window can show them while
recording.
* Both engines copy the recording's samples into a tap (`Engine::set_input_tap`;
  desktop: the engine thread while draining the ring buffer, browser:
  `record_input`). `AppState::tick` (desktop: status thread, browser: worker
  timer) hands them to the live analysis – no extra thread, one place for both
  shells.
* `klang_dsp::tracker::Tracker` measures frame by frame with the building
  blocks of the finished analysis (`pitch::Yin`, `onset::Flux`,
  `onset::is_peak`), ~96 pitch frames per second; it keeps only the samples the
  next frames need.
* Notes are segmented again after the last silence on every update; notes
  before it are settled. Key after six notes; tempo from the last 20 s once
  there are six onsets in them, shown as the median of the last three
  estimates (twice a second), so a single octave error doesn't show.
* The UI polls `record_live` with a cursor (take, curve points, settled notes,
  onsets) and gets only what is new. `StatusDto::live_analysis` says when to
  show it (not for the library's sing-to-search).
* After stopping, the finished analysis (unchanged) replaces the live view.
* Tests: `KLANGWELT_FAKE_MIC=<wav>` makes the desktop engine record from a
  file (`FakeInput`: in real time, looped); the browser tests use Chromium's
  `--use-file-for-fake-audio-capture`.

### c) Melody recognition and classification
* **Features:** interval (semitones) and duration ratio of consecutive
  notes → independent of key and tempo.
* **Index:** n-grams on three levels (exact / intervals only / contour only).
  Search: voting over (piece, offset), then fine scoring via alignment
  with tolerance for wrong notes.
* **Analysis (stage 1, heuristic):** main theme, countermelody, inner voice,
  accompaniment figure, ostinato, bass; authentic and half cadences; mood via
  valence/arousal.
* **Scaling to PDMX (~250,000 scores), next steps:**
  postings sorted + delta/varint-compressed, mapped in via mmap;
  index only melody-carrying voices; classification by a model
  (Rust: `burn`/`candle`), trained on heuristic and agent labels.
* Licenses: PDMX comes with license information per file – store it on
  import and show it in the results.

* **Libraries** (`app/src/libraries.rs`): a library is exactly one
  index file in `Indizes\` of the library folder; searching only happens in
  the active one (`AppState::library`). `bibliotheken.json` holds the list,
  contents (packs, folders/files, counts) and the active one; `startup` loads
  it in the background at startup (beforehand: `migrate` builds "Standard" from
  the old per-pack-folder indexes). New scores: `build_files_to` into a
  temporary file, then `merge_into` → `klang_music::diskindex::merge` into the
  library's other file (`X.kwix` ↔ `X.neu.kwix`); `install` puts it in place
  (active index swapped, old file deleted) – the search keeps running
  meanwhile.
* **Merging** (`diskindex::merge`): voice records and feature sequences are
  copied one after the other (numbers shifted), postings by merging the
  sorted key tables (only in the first source: bytes unchanged),
  features renumbered, chord statistics added (`ChordStats::absorb`,
  examples by place ÷ frequency). With `dedupe`, every score that an earlier
  source already has is dropped (`source_key`: PDMX by IPFS ID,
  otherwise by path). 1 GB + a small pack: a few seconds.
* **Library in the app** (`app/src/library.rs` – downloading/indexing –, `crates/klang-app/src/library.rs` – searching, opening, playing –, `crates/klang-music/src/library.rs`):
  `gather` collects files and folders (without duplicates, with the folders'
  genre/composer), `build_files_to` reads them in parallel (`std::thread::scope`)
  and reports progress, throttled, as the event `library-changed`. Search by text
  (title, composer, voice) or melody (note names/MIDI, or the notes of a
  clip); the detail view analyzes the score (`analyze_score`) and
  highlights the match.

* **Downloading** (`klang_music::fetch`, `app/src/library.rs`): the operating
  system's `curl` delivers PDMX `mxl.tar.gz` as a data stream → `flate2` (gzip) →
  `tar` → scores only, safe paths, `.teil` file + rename; existing
  files of the same size are skipped. Sample/selection packs stop after N
  files (start of the archive). Then `build_index_cancellable` runs in the same
  background thread; cancelling stops curl or the index threads respectively.
  `fetch::ResumingDownload` (for `mxl.tar.gz` and `PDMX.csv`) is the `Read`
  behind it: if curl does not end successfully (e.g. 18 "end of response", 28 too
  slow, 56 connection reset), it restarts after a short pause with
  `--continue-at <bytes read>`; if the server then does not deliver exactly this
  range (33) or returns a 4xx error, it gives up, likewise after five
  connections without a new byte. The stream reports the abort as
  `ConnectionAborted` (not `Interrupted` – `read_exact`/
  `read_to_end` treat that as "read again"); `extract_scores` turns it back into
  `Interrupted`. `KLANGWELT_PDMX_URL` redirects the download to a
  local server for tests (`tests/e2e/library-resume.test.mjs`).
* **Browsing** (`klang_music::facets`, `klang_music::pdmx`): during indexing,
  every score gets `ScoreFacets` (analysis without cadence search – 30,000
  pieces ≈ 20 s – plus genre/composer/difficulty from
  `pdmx-metadaten.tsv`). Index format `KWIX02` appends them as JSON
  (`KWIX01` stays readable, then without the tree). `tree_children(levels, path)`
  filters by the path and counts the values of the next level (fixed
  order for ordered levels, otherwise by count, remainder groups last);
  multi-valued levels count a score in every branch. The UI loads
  branches on expand (`library_tree`), the agent uses `browse_library`.
  `PDMX.csv` is filtered as a data stream with an in-house RFC 4180 reader
  and stops as soon as all of the library's own pieces have been found.
* **Jobs** (`app/src/library.rs`): the chain download → genre/composer →
  indexing → install writes its state to `klangwelt-auftrag.json` in the
  pack folder (`laden`, `metadaten`, `indizieren`, `abgebrochen`; plus the target
  library and pack). The folders of open jobs are additionally listed in
  `bibliothek-auftraege.txt` in the app data folder. `resume_pending` (after the
  library has loaded) continues open jobs at startup
  (`finish_pipeline`); interrupted downloads are only reported. After three
  failed attempts it gives up. `library::busy` holds up closing
  (`CloseRequested` → event `close-requested` with `{dirty, busy}`), and the
  UI asks.
* **Queue** (`app/src/library.rs`): `start_download` starts immediately
  if nothing is running; otherwise the pack is added as a `QueuedDownload` (pack,
  target library = the currently active one, folder, genre/composer) to the end of
  `LibStatus::queue` (`Started::Queued`). Anything already running or
  waiting is rejected (same library, `covers` – a larger PDMX pack covers smaller
  ones). `LibStatus::pipeline` stays set from the start until the end of
  installing (`PipelineGuard`, also when resuming after startup) and counts in
  `check_idle` – so nothing slips into the short gaps between downloading,
  categories and indexing. The thread `klang-warteschlange` (runs only
  while something is waiting) checks every 250 ms; if things are idle twice in
  a row, it starts the next pack (`start_queued`: first checks again whether it
  is now already contained in the target library – if so, it is skipped with a
  message). `library_dequeue(id?, library?)` removes one or all;
  `LibraryInfo` reports `queue` and `currentPack`. The queue lives only
  in memory.
* **On-disk index** (`klang_music::diskindex`, format `KWIX04`, also reads `KWIX03`):
  `IndexWriter` writes voice records and feature sequences immediately to
  temporary files and collects postings in blocks (8 million entries), which
  go to disk sorted as runs; `finish` merges the runs
  (k-way), drops n-grams with more than 20,000 postings (the search
  skips them anyway) and writes lists delta/varint-encoded.
  `DiskIndex` keeps only offsets, a sparse table of contents (every 64th
  key) and the features in memory and reads lists and
  feature sequences on demand (`seek_read`/`read_exact_at`). The same
  search function (`search_in`) works on both kinds of index; a test
  compares postings and hits with the in-memory index. `AnyIndex`
  also opens older files (format 1/2, in memory).
  Measured with 30,000 scores: build 12–31 s at ≈ 0.2 GB of memory,
  index file 157 MB (previously 241 MB in the in-memory format, ≈ 0.6 GB RAM).
  `DiskIndex` reads through `klang_music::source::ReadAt` – a file, bytes in
  memory, or a file on a web server read by range (`source::remote`: a read
  that isn't there yet fails with `WouldBlock` and is noted; the browser
  fetches the noted ranges and runs the command again). `IndexWriter::set_provenance`
  writes a JSON block between header and voice entries (sources, versions,
  checksums); all readers skip it, since every section is found by its offset.
  The browser version reads a prebuilt package like this (index, scores ZIP,
  text index, facet table, chord statistics – `klang_music::webpack`,
  `scorezip`; built by `crates/klang-libbuild`) and layers the own scores over
  it (`klang_app::weblib`) – see [LIBRARY.md](LIBRARY.md).
* **Chord progression statistics** (`klang_music::chordstats`, part of `KWIX04`):
  during indexing, `score_progression` detects the chord progression of each
  score (all voices except drums, grid of half a bar, or a whole bar in
  odd meters) and stores it relative to the piece's key
  (`Rel`: distance from the tonic × chord quality × inversion). `ChordStats`
  counts transitions separately for major and minor and keeps up to five
  example passages per transition from different pieces (voice, bar, range).
  The selection is pseudo-random but reproducible, and prefers pieces with a
  title and composer. Query `follow(prev, next)`: share among all
  continuations of `prev`; with fewer than 40 transitions of the exact form,
  the basic chords (without seventh/inversion) are counted. The section is
  stored in binary after the table of contents; the last header value points
  to it. Modulations count relative to the main key. 30,000 scores: build
  51–66 s (release build), ≈ 24,000 major and 5,800 minor pieces with a chord
  progression. The statistics come in three groups (`GROUPS`: all, Baroque,
  Bach), assigned by composer (`composer_groups`: word list of Baroque
  composers, "Bach" as a separate word, so not Offenbach). Section
  `KWH2` = several `KWH1` blocks; `KWH1` on its own is still read.

## Harmony (circle of fifths)

* **Harmony in the timeline** (`crates/klang-app/src/harmony.rs::line`,
  `ui/src/harmonyline.ts`): chord progression of the whole piece (`chords_in`
  over all instrument tracks), for each chord the mood of the transition
  (`transition_mood`) and, where applicable, a break
  (`klang_music::harmony::harmonic_break`: not explicable as diatonic, as a
  secondary dominant with a reachable target, as borrowing from the parallel key
  or as a Neapolitan – or a tritone leap without a dominant). The UI loads it,
  throttled, after every change; the timeline draws it at the bottom of the
  ruler, and the Harmony panel picks up the chords at the playback position
  from it.
* **Inserting** (`harmony::write_mode` with `insert`): a batch of
  "make room" (all clips from that point onward move, note clips spanning the
  point get longer, notes after it move along, notes spanning the point end
  there) and the chords in the gap – one undo step.

* **`klang_music::harmony`** – theory in the core, shared by UI and
  agent:
  * `Chord` (root, quality, bass).
  * German spelling relative to the key (natural-note degree + alteration, so
    As (A♭) instead of Gis (G♯) in C major, B = English B♭).
  * `describe` returns the degree with figured-bass figures (V⁶₅, I⁶₄, N⁶, V⁷/vi),
    the Riemannian function (T, Sp, D⁷, DD, (D⁷) Tp, s⁶₅, sN, Đ) and the role in
    words.
  * `parse_chord` accepts symbols (`G7`, `fis°`, `C/E`, `Es`) and degrees (`ii6`,
    `V7/V`, `♭VI`, `N6`).
  * `suggest` follows rule tables per degree (major/minor, with category and
    reason); for chords outside the key it follows root movements (falling
    fifth first) and common tones.
  * `transition_mood` assigns a mood to every transition (`Mood`: resolving,
    tense, surprising, bright, dark). The order of the checks:
    cadential six-four → deceptive cadence (dominant one step up) →
    chromatic mediant relationship → resolution (falling fifth from the dominant,
    leading tone, plagal) → to the dominant → sound of the target. Suggestions
    and cadence templates carry it too.
  * `cadence_paths` searches for multi-step paths to the cadences (`goal_of`:
    authentic, plagal, half, deceptive, Phrygian half cadence). The search:
    - Depth-first search over the best 8 rule continuations (no tonicizations,
      no repetition, no return to the starting chord along the way, ending on
      the tonic).
    - A cache keyed by (last, second-to-last) chord, ≈ 1–2 ms for 4
      steps.
    - Scoring: geometric mean of the step weights × 0.93 per additional
      chord. The weight comes from outside (`weight`), so that the app can
      factor in the library.
    - Per cadence: the best path and the best longer one (`longer`), if it
      reaches at least 75% of the score.
  * `cadences` holds the templates; `detect_chords` detects chords in notes
    (weight per pitch class, the seventh counts half, the bass note favors the
    root).
  * `voice_chord`/`voice_progression` voice in four parts via a search over all
    positions: costs for voice movement, parallel fifths/octaves, doublings,
    leading tones and sevenths.
* **`crates/klang-app/src/harmony.rs`** – key of the project (histogram of the
  instrument tracks), chords of a track, preview (sound of the target track,
  otherwise the plucker), writing as `AddTrack`/`AddClip`/`SetClipNotes` in one
  `Command::Batch`, suggestions with frequencies from the loaded index
  (`suggest_with_library`, plus frequent continuations without a rule) and
  playing example passages. `library_open` takes a range (`range`)
  that the score view highlights.
  * Paths to the cadence (`paths_for`): weight per step =
    ½ · rule value + ½ · min(1, 2 · share in the library), share via
    `ChordStats::share` (like `follow`, without examples). Every path carries the
    shares of its steps (`shares`).
* **`ui/src/components/harmony.ts`** – circle as SVG:
  * three rings, rank badges, arrows colored by mood;
  * a separate arrowhead marker per mood, because markers do not reliably
    inherit the stroke color;
  * overlay for a path or a cadence: arcs with a dark halo,
    numbered steps; the same chord twice yields a loop and
    numbers side by side.

  The overlay has two states: hovering (`hover`) takes precedence over the
  pinned one (`st.pinned`). While it is visible, suggestion arrows and badges
  are dimmed.

  Also in the panel: legend (hovering over it uses `data-mood` to fade out the
  other moods), "Paths to the cadence" below the circle, history, grouped
  suggestions with a mood dot, share bars and examples, cadence cards.

  New panels find their place via `PanelDef.place` even in an already
  saved layout (here: above the agent chat).

## Counterpoint and fugue

* **`klang_music::counterpoint`** – works on `VNote` (pitch, onset,
  duration in beats).
  * `check(voices, names, key, bpb)` goes through all onset times:
    * Parallels: two voices move in the same direction from fifth to fifth
      or from octave/unison to octave.
    * Hidden parallels: outer voices only, leap in the upper voice.
    * Voice crossing.
    * Dissonance on a strong beat: only allowed as a suspension, i.e. held
      over or re-struck and resolved stepwise downward.
    * Dissonance on a weak beat: approached or left stepwise.
    * Range per voice name, leaps (beyond an octave, tritone, augmented second
      in minor), spacing.

    The order of the voices comes from the names, otherwise from the register.
  * `generate_line` generates a voice against existing ones via dynamic
    programming over pitch pairs (previous, current). The costs
    follow the rules of voice leading: consonance on strong beats, passing tones,
    suspensions, contrary motion, a step in the opposite direction after a leap,
    no parallels, ceiling and floor set by the neighboring voices. `invertible`
    avoids fifths (double counterpoint at the octave). `complementary_rhythm`
    moves where the reference voice holds.
* **`klang_music::fugue`**:
  * `answer`: real or tonal (the fifth in the head becomes the tonic).
  * `transform`: diatonic inversion, retrograde, augmentation, etc.; in minor,
    the minor seventh before the tonic becomes the leading tone.
  * `strettos`: all distances × intervals, filtered with `check`. Free
    dissonances are counted too.
  * `countersubjects`: variants above/below the subject, also checked with the
    voices exchanged at the octave.
  * `plan_fugue`: the framework. Exposition (entry order alto–soprano–tenor–bass),
    episodes (sequence of the subject head over falling fifths to the target key),
    middle entries (relative key transposed diatonically, subdominant),
    stretto, pedal point, cadence via `voice_progression`. Free voices
    are generated by `generate_line` against everything already in the section.
    Measured: 0 voice-leading errors in the test plans.
* **`crates/klang-app/src/fugue.rs`** – subject from a clip (key estimated from
  the subject), creating the voices as tracks (organ in the bass with a pedal stop),
  multi-voice preview and `check_tracks` over all instrument tracks. Plus
  five MCP tools: `fugue_subject`, `transform_melody`, `countersubject`,
  `write_fugue`, `check_counterpoint`.
* **`ui/src/components/fugue.ts`** – cards for subject/answer, transformations,
  strettos, countersubject, plan (section strip), checker. The findings go
  to the timeline via the signal `counterpointMarks`: tinting of the tracks
  and a strip in the ruler.

### d) Sound workshop
* Six instruments (`InstrumentKind`), each with self-explanatory parameters,
  described by `crates/klang-app/src/sound.rs::spec(kind)` – groups, limits,
  words at the ends of the controls, fields, preview patterns. The UI
  (Sound tab, `ui/src/components/sound.ts`) and the agent
  (`list_instruments`) are built from the same description; parameters
  are validated in the core (`validate_params`).
* **Fields** (`sound::fields`): Character, Timbre, Space – two axes each
  and named starting points. Timbre and Space are the controls themselves
  (`farbe`/`rauheit`, `breite`/`raum`); Character is a fixed
  mapping per instrument (`charakter_params(kind, x, y)`), the position
  is stored in `charakter.x/y`. The controls of the fields are disjoint (test), so
  `apply_field` never changes another field. The default values of the
  Character controls are the mapping at the home position. Dragging goes through
  `sound_field` (one request after another, the newest wins) and is
  merged into one undo step (a batch of one command
  merges like that command).
* **Sound library** (`crates/klang-app/src/sound_library.rs`, "My sounds"): a
  file `klaenge.json` in `Musik/Klangwelt/Klänge` (with
  `KLANGWELT_BIBLIOTHEK` set, in that folder – tests never touch the real
  sounds). The complete instrument is saved (`normalize`: missing
  controls get their default value, unknown ones are removed, curves are
  checked), so that new default values do not change a saved sound. Names are
  unique per instrument; saving under an existing name replaces the sound
  (the name stays). Writing is atomic (`.neu` + rename); an unreadable
  file is never overwritten. Loading = one `SetTrack` (one
  undo step). Every change reports `sounds-changed`, so that the
  UI also shows changes made by the agent; the UI itself determines which
  sound is "on" by comparison (all controls, plus the curves for the synth).
* **Sound shaping after generation** (`sound::shaping_of`, `klang_dsp::live::FxChain`; offline `klang_dsp::fx`):
  warmth → beating → roughness → color → distance → width. Width is a
  side signal (delayed copy without lows); `PlacedClip` carries mid and
  optionally side (left = M + S, right = M − S); in mono the side is dropped.
  * **Plucker** (`synth::pluck`, extended Karplus-Strong): `helligkeit`,
    `abklingen`, `koerper`, `anschlag`.
  * **Drums** (`klang_dsp::drums`): kick drum (sine sweep down to the
    fundamental, click, warmth), snare (tuned shell + filtered noise),
    clap, hi-hats (the open one is choked by the next hi-hat), toms – each
    with a tunable fundamental. Tests measure the fundamental via `hit_pitches`.
    Plus rim (two short tuned tones, follows the snare shell), cowbell
    (two square tones at 540/800 Hz with band-pass), ride and crash (six
    inharmonic partials plus noise) and shaker (noise swish) – without
    their own controls, derived from the snare and hi-hat controls.
  * **Bass** (`klang_dsp::bass`): PolyBLEP saw/square, TPT low-pass with
    envelope, sub-octave, saturation, monophonic glide on tied notes.
  * **Organ** (`klang_dsp::keys::organ_note`): additive. The partials of the
    drawn stops are stored in a wavetable per note. It is two
    fundamental periods long so that 16′ (half frequency) fits into it; partials
    above 0.45·sr are dropped. Pipe speech 15–90 ms depending on register, wind
    noise as filtered noise, tremulant.
  * **Harpsichord** (`keys::harpsichord_note`): two Karplus-Strong choirs (8′ and
    4′, slightly detuned), bright quill pluck, lute stop damps,
    damper noise on release.
  * **Synth** (`klang_dsp::pad`): 1–7 voices per note (PolyBLEP, sine ↔
    saw ↔ square) with up to 30 cents of detune, sub, shimmer, 24 dB
    low-pass (2 × SVF) with LFO. The envelope is drawn
    (`Instrument::curves`: "laut" (loudness), "hell" (brightness) as [second, value]);
    after the last point the value holds, after release it fades out over
    `ausklang` seconds. The pulse follows the project tempo (`spb`). Presets in
    `VERLAUF_PRESETS`.
* The sound report (`sound::report`) measures the rendered sound
  (fundamental of the kick drum, snare shell, hi-hat brightness, bass pitch
  without sub) and returns a display + words for UI and agent.
* **Song structure** (`crates/klang-app/src/song.rs`, `ui/src/components/song.ts`): a
  `SongPlan` consists of roles (`Layer`: role, instrument, sound starting points,
  gain) and sections (`Section`: bars, kind, intensity 0–2 per role) plus
  tempo, key and chords (`kh::parse_chord`). `generate` deterministically
  creates one track per melodic role and one drum track from
  all drum roles (via the grid from `rhythm`), one clip per section.
  The kind shapes volume and note brightness (`shape`); a build-up with
  transitions ends in a snare roll. `AppState::song_write` writes everything in
  one batch and remembers the generated tracks (`song_tracks`, cleared on
  open/new), so that regenerating replaces them.
* **No rendering:** `build_arrangement` only prepares the note clips for the
  live mixer (cached per clip) – a generated song is there in a few milliseconds.
* **Rhythm** (`crates/klang-app/src/rhythm.rs`, `ui/src/components/rhythm.ts`): 33
  genre patterns (steps 1–16 per drum, after "Pocket Operations") and the
  step grid as a view onto a drum clip. `grid_notes` turns
  the grid into notes with accents (backbeat loud; in-between snares quiet,
  unless the bar has no snare on a quarter note – then the pattern itself is
  syncopated, like the dembow) and swing (odd sixteenths up to 2/3 of a
  step later). `grid_from_notes` reads back; because swing only pushes notes
  later, a note up to 0.67 steps after an odd sixteenth belongs to that
  sixteenth. `toggle` only changes notes of the drum on that one
  step (expression and other keys stay), `apply_swing` moves drum
  notes to step + swing. A cymbal on at most two steps is a
  crash, otherwise a ride. New tracks get the genre's Character starting
  point.
* Planned: modal synthesis (bells, wood, metal), waveguides (wind
  instruments), effects.

## Agent via MCP

```
Claude Code ──HTTP──┐                       ┌─ project-changed ─▶ UI (live)
                    ▼                       │
Claude Desktop ─stdio─▶ klangwelt --mcp ─HTTP─▶ klang-mcp (127.0.0.1:7419) ─▶ app/src/mcp.rs ─▶ klang_app::agent ─▶ AppState
                       (bridge)                  JSON-RPC, written in-house        tools           Session/Engine
```

* **`crates/klang-mcp`** – an in-house, lean MCP: JSON-RPC 2.0 (`initialize`,
  `ping`, `tools/list`, `tools/call`), transport "Streamable HTTP": responses to
  `POST` as JSON, plus the event stream `GET /mcp` (`text/event-stream`,
  chunked). It stays open until the client closes it, and carries only
  keep-alives (an SSE comment every 20 s; otherwise e.g. Node/undici aborts after
  300 s of silence). Blocking, with threads instead of an async runtime.
  Security: loopback only, Origin check against DNS rebinding, optional token.
  Protocol versions 2024-11-05 to 2025-11-25.
* **Who is connected?** (`klang_mcp::Presence`) Open connections count:
  * Event streams: an MCP client such as Claude Code with a configured server
    (MCP SDK) opens its stream right after `notifications/initialized` and keeps
    it until it exits.
  * Requests that are still running – such as a waiting `chat_poll` (long poll).
  * The server notices a close immediately: the stream thread checks every 250 ms
    via `peek`; running requests end with their response. After the
    last stream has closed, the client still counts as connected for `STREAM_GRACE` (3 s),
    so that a brief reconnect does not flicker.
  * Clients that never open a stream (scripts that call tools directly)
    count for `AGENT_GRACE` (2 min) after their last request. `DELETE` ends
    a session immediately. Requests without a session together count as one client.
  * `AppState::agent_connected`: an agent is listening, working or connected in this way.
    `AppState::tick` (both shells call it periodically) checks the agent state twice per
    second (`watch_agent`: listening, working, tasks in progress, connected) and sends
    `chat-changed` when it changes – also for changes without an event of their own (a
    lease or the minute after the last tool call running out, a client going away).
    The bar shows "Agent listening", "Agent working on #n" (or both), "Agent ready",
    "No agent connected" (only after more than a minute without a sign) or "Agent off"
    without a server.
  * The stdio bridge keeps a stream open for its client (in its own thread,
    reopening it after an abort) and ends the session when the
    client stops.
* **Connecting via prompt** (`agentPrompt(url)` in `ui/src/components/chat.ts`):
  the agent chat offers exactly one text to copy. In it, a Claude Code session
  reads how to operate the endpoint via HTTP without any MCP setup (curl:
  `initialize` → `result.instructions`, `tools/list`, `tools/call`) and how to
  start a listening subagent. The server needs nothing special for this:
  requests without `Mcp-Session-Id` and without an `Accept` header are answered
  and count as an anonymous client. The e2e test runs the curl lines from the prompt
  (`initialize`, `chat_poll`) verbatim.
* **`klangwelt --mcp`** (`klang_mcp::bridge`) – stdio bridge for clients that
  start servers as a process.
  * If the app is not reachable, `app/src/cli.rs` starts it once itself:
    the same program without arguments, without inherited pipes, detached from the
    client's job object, so that the window survives the end of the bridge.
  * After that, the bridge waits until the port responds.

## One program (`app/src/main.rs`, `app/src/cli.rs`)

At startup, `klangwelt.exe` decides from its arguments what to do:
* **Window:** without arguments or with a `.klangwelt` file.
* **`--mcp`:** the bridge.
* **`index …`:** the library; this used to be the separate program `klang-index`.
* **`--help`, `--version`.**

Details:
* **No console window:** the release version is a GUI program
  (`windows_subsystem`). For output it attaches to the calling console via
  `AttachConsole` – only if the output is not redirected anyway.
* **Project file while the app is running:** with a project file, the program
  first asks a running instance via the MCP tool `open_project`. That
  instance reports `open-file` to the window, which opens it the normal way (asking
  first if there are unsaved changes). Only if no instance responds does
  a new window open.
* **No library next to the program:** `crate-type = ["rlib"]` – the code
  lives in the exe, no `klangwelt_lib.dll`. For Android/iOS, Tauri would need
  `cdylib`/`staticlib` again.

**Release without an installer:** `npm run release` in `ui/` builds the UI and
then runs `cargo build --release -p klangwelt --features tauri/custom-protocol`.
* The feature switches Tauri into production mode; the UI is
  embedded.
* The result is a single file, `target/release/klangwelt.exe`.
* Publisher, copyright and description from `bundle` in
  `app/tauri.conf.json` end up in the exe's file properties.
* **`crates/klang-app/src/agent.rs`** (shared by desktop and browser; the HTTP server lives in `app/src/mcp.rs`) – over 30 tools. Changes run as a `Command::Batch`
  labeled "Agent · …" through the same session as the UI; afterwards
  the app sends `project-changed` (new snapshot) and `agent-activity`
  (message in the UI). Pure view changes (note view,
  track heights) create no undo step.
  * `Agent::call` wraps every tool: it marks the agent as working (`Chat::touch`, except a
    waiting `chat_poll`), compares track/clip IDs and the revision before and after and adds
    `revision`, `change` (the summaries passed to `changed`, collected per thread) and
    `created` / `removed` to the response, and adds `cancelRequested` (also to error texts).
  * **Revision** (`Session::revision`, klang-core): a process-wide counter that moves with
    every content change, undo and redo (not with view changes; a newly opened project never
    repeats one). `get_project` returns it, `execute {ifRevision}` checks it under the same
    lock as the change (`AppState::execute_if`). `get_project {trackIds, fields}` filters
    (`"tracks.name"` = field of every track).
  * **Placeholders** in `execute`: an object with `"ref": "$bass"` gets a real ID from
    `Project::alloc_id` before the commands are parsed; `"$bass"` in `id`/`trackId`/`clipId`
    is replaced by it. New tracks/clips may omit `id` (= 0, new).
  * **Compact notes** (`crates/klang-app/src/notation.rs`): wherever a command has
    `"notes": "<text>"`, the text (`C4 1 · E4 .5 v100`, chords with `+`, rests `r`, `@start`,
    German note names) becomes the JSON note list.
  * **`batch`**: runs tool calls in a row; `Session::mark` marks the start (with what can be redone),
    `Session::squash` merges all undo steps since then into one, `Session::rollback` reverts
    them at the first error, redo included. `"$0.trackId"` takes a field of an earlier result; placeholders
    of an `execute` stay valid for the following calls.
* **Chat** (`crates/klang-app/src/chat.rs`): the UI writes tasks
  (`chat_send`), the agent picks them up with `chat_poll` – a long poll over
  `Mutex` + `Condvar` that waits up to `timeoutSeconds` and returns
  immediately as soon as something new arrives. Picking up **reserves** a task for the
  poller's `client` ("claimed by …") for `leaseSeconds` (default 60): without a
  `chat_reply` in that time it is offered again (`redelivered`). The poll checks for leases
  running out in its 1 s slices; in the browser the web worker asks again every 5 s. A
  reply confirms the task (never offered again); with `done` it is finished.
  `chat_peek {sinceId}` reads messages without picking up. A short message of stop words
  ("Stopp", "stop #3", all UI languages) or the stop button (`chat_stop`) stops tasks:
  not picked up yet → cancelled, in progress → `cancel_requested` until the agent finishes it.
  While a poll is waiting (plus a 30 s grace period), the UI shows "Agent listening"; a task
  in progress or a tool call in the last 60 s shows "Agent working on #n".
  **Roles:** a listener subagent (small model) only polls and confirms, the main session
  starts a worker subagent per task and passes follow-ups on (subagents can't start
  subagents in Claude Code, so the listener sends tasks to the main session). The connect
  prompt, the server instructions (`ROLES`) and the MCP prompts `zuhoeren` (`LISTENER_BRIEF`,
  worded identically in `.claude/agents/klangwelt-zuhoerer.md` of the desktop) and `arbeiten`
  (`WORKER_BRIEF`) describe them; in Claude Code `/mcp__klangwelt__zuhoeren`.
  Empty poll responses are deliberately minimal so that the subagent runs for a long time.
  The poll waits in 1 s slices and in between checks via
  `klang_mcp::caller_gone()` (thread-local connection, non-blocking
  `peek`) whether the client has hung up. If so, it picks up nothing, and the next
  poll reports the abort (`abbruch`) with a suitable wait time.
* **Prompts:** besides `tools/*`, `crates/klang-mcp` also answers
  `prompts/list` and `prompts/get`.
  Event: `chat-changed`.
* **The agent's "ears":** `klang_dsp::features` measures the spectral
  centroid, low/high share, attack centroid and decay time (T60).
  `crates/klang-app/src/sound.rs` translates the measurements into words (dark/bright, thin/warm,
  soft/hard, short/long). The thresholds are calibrated and secured by a test
  (`cargo test -p klang-dsp print_calibration_grid -- --ignored --nocapture`).
  This lets the agent implement "warmer and softer" step by step and check the
  result – the Sound tab shows the same measurements.
