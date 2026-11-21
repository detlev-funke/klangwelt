// Connection to the Rust core (`klang-app`). Desktop: via Tauri IPC. Browser:
// the same core as WebAssembly in a web worker (`web/host.ts`). Both
// use the same mapping (`coreBackend`) – only the transport differs.
// The mock (`mock.ts`, no audio, simplified) still exists for
// UI work: `?attrappe` in the address.

import type {
  AgentActivity, Analysis, AutosaveResult, CatalogEntry, CheckView, ExportInfo, ExportOptions, ExportResult, Chord, ChordInfo, ChatView, Command, Countersubject, Dimension, EngineStatus, FugueOptions,
  FuguePlan, HarmonyView, InstrumentKind, InstrumentSpec, SubjectView, TransformSpec, VNote,
  AddResult, ConvertResult, ConvertTarget, ImportResult, QuantizeResult, SmoothResult, SmoothStrength, KeySpec, LibEntry, LibHit, LibraryInfo, ListenResult, OwnScore, PlacedChord, PlayInfo, PreviewPattern, ProjectFile, RecordResult, LiveCursor, LiveView, HarmonyLine, SongPlan, SongWritten, ClipGrid, RhythmGrid, RhythmInfo, RhythmWritten, SavedSound, ScoreView, Snapshot, SoundReport, SoundSaveResult, SuggestResult, TreeLevel, WaveView,
} from "./types.js";
import type { AudioDevices, AudioKind, HistoryView, MergeChoices, MergeView } from "./types.js";
import { createMock } from "./mock.js";
import { createWebLink } from "./web/host.js";

interface TauriGlobal {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> };
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
  }
}

/** Transport to the core: call a command (name + arguments, as in the
 *  core's command table) and receive events. */
export interface CoreLink {
  call<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, cb: (payload: T) => void): void;
}

/** Where the UI runs: desktop app, browser with the real core, mock. */
export type Platform = "desktop" | "web" | "mock";

export type TransportAction = "play" | "stop" | "toggle" | "rewind";

export interface Backend {
  /** Real core (desktop or browser) – not the mock. */
  readonly native: boolean;
  readonly platform: Platform;
  /** Set the core's language: code and dictionary (flat, see `coreDictionary` in i18n.ts). */
  setLanguage(lang: string, dict: Record<string, string>): Promise<void>;
  getState(): Promise<Snapshot>;
  execute(command: Command, merge?: boolean): Promise<Snapshot>;
  undo(): Promise<Snapshot>;
  redo(): Promise<Snapshot>;
  /** Change history: the active variant's steps with key figures, the variants and checkpoints. */
  historyView(): Promise<HistoryView>;
  /** Jump to the state after a step (any number of undo/redo steps, also onto another variant). */
  historyGoto(node: number): Promise<Snapshot>;
  /** Name the current state (null: "Checkpoint n"). */
  historyCheckpoint(name: string | null): Promise<HistoryView>;
  historyCheckpointRename(id: number, name: string): Promise<HistoryView>;
  /** Remove a checkpoint (its steps stay). */
  historyCheckpointDelete(id: number): Promise<HistoryView>;
  /** Start a variant at a step (null: the current one) – the project goes there, new steps go into the variant. */
  historyVariantNew(name: string | null, node: number | null): Promise<Snapshot>;
  historyVariantSwitch(id: number): Promise<Snapshot>;
  historyVariantRename(id: number, name: string): Promise<HistoryView>;
  /** Delete a variant with the steps only it has. */
  historyVariantDelete(id: number): Promise<Snapshot>;
  /** Forget all steps; the project stays as it is. */
  historyClear(): Promise<Snapshot>;
  /** Combining variants: what each changed compared with their common origin. */
  historyMergePlan(target: number, others: number[]): Promise<MergeView>;
  /** Play the combination from `startBeats` without changing the project. */
  historyMergePreview(target: number, others: number[], choices: MergeChoices, startBeats: number): Promise<{ endBeats: number; tempo: number }>;
  /** End the preview: the project plays again. */
  historyMergePreviewStop(): Promise<null>;
  /** Combine into a new variant (one step) and switch to it. */
  historyMergeApply(target: number, others: number[], choices: MergeChoices, name: string | null): Promise<Snapshot>;
  transport(action: TransportAction): Promise<void>;
  seek(seconds: number): Promise<void>;
  setMetronome(enabled: boolean): Promise<Snapshot>;
  /** Repeat on/off. */
  setLoop(enabled: boolean): Promise<Snapshot>;
  /** Mark a time range in beats; null clears it. */
  setLoopRange(range: [number, number] | null): Promise<Snapshot>;
  /** Output and recording devices of the system including the choice. */
  audioDevices(): Promise<AudioDevices>;
  /** Choose a device (null = system default) – takes effect immediately and applies at the next start. */
  setAudioDevice(kind: AudioKind, name: string | null): Promise<Snapshot>;
  recordStart(): Promise<void>;
  recordStop(): Promise<RecordResult>;
  /** What the running recording's live analysis found since `since` (null: no recording running). */
  recordLive(since: LiveCursor): Promise<LiveView | null>;
  getWaveform(assetId: string, buckets: number): Promise<number[]>;
  analyzeAsset(assetId: string): Promise<Analysis>;
  createMelodyTrack(trackId: number, clipId: number, smooth?: SmoothStrength): Promise<Snapshot>;
  /** Insert audio files: on an audio track the first file goes in, otherwise one new track per file (under `trackId`), from `startBeats`. */
  audioImport(paths: string[], trackId: number | null, startBeats: number): Promise<ImportResult>;
  /** Convert a recording into a melody or rhythm track (`clipId` null: all audio clips of the track). */
  audioToNotes(trackId: number, clipId: number | null, target: ConvertTarget, smooth?: SmoothStrength): Promise<ConvertResult>;
  /** Delete a time range from all tracks, everything after it moves up (clears the selection). */
  deleteRange(startBeats: number, endBeats: number): Promise<Snapshot>;
  /** Align notes to the grid (`indices` null: all notes of the clip); `grid` in beats. */
  quantizeNotes(trackId: number, clipId: number, indices: number[] | null, grid: number, lengths: boolean): Promise<QuantizeResult>;
  smoothNotes(trackId: number, clipId: number, indices: number[] | null, strength: SmoothStrength): Promise<SmoothResult>;
  createDemo(): Promise<Snapshot>;
  previewTrack(trackId: number, pattern: PreviewPattern): Promise<number>;
  previewNote(trackId: number, midi: number): Promise<void>;
  describeSound(trackId: number): Promise<SoundReport>;
  /** Waveform of the track's test sound (sound window). */
  soundWave(trackId: number): Promise<WaveView>;
  /** Set a point (x, y each 0…1) or start point in a field; `merge` = part of a drag. */
  soundField(trackId: number, field: string, x: number | null, y: number | null, preset: string | null, merge: boolean): Promise<Snapshot>;
  /** Song templates (whole pieces made of sections and roles). */
  songTemplates(): Promise<SongPlan[]>;
  /** Create a song; `replace` replaces the song tracks created last. */
  songWrite(plan: SongPlan, replace: boolean): Promise<SongWritten>;
  /** Rhythm: genre patterns and rows of the step grid. */
  rhythmInfo(): Promise<RhythmInfo>;
  /** Drum clip as a grid (up to 4 bars). */
  rhythmRead(trackId: number, clipId: number): Promise<ClipGrid>;
  /** Set or clear a step in the clip (one undo step). */
  rhythmToggle(trackId: number, clipId: number, row: string, step: number, on: boolean, swing: number): Promise<Snapshot>;
  /** Change the clip's swing; `merge` = part of a slider drag. */
  rhythmSwing(trackId: number, clipId: number, swing: number, merge: boolean): Promise<Snapshot>;
  /** Write a grid: into a clip (replaces), as a new clip in a track or into a new drum track. */
  rhythmWrite(grid: RhythmGrid, swing: number, trackId: number | null, clipId: number | null, startBeats: number, name: string, sound: string | null, tempo: number | null): Promise<RhythmWritten>;
  /** Preview a grid or clip at the project tempo; returns the duration in seconds. */
  rhythmPreview(trackId: number | null, clipId: number | null, grid: RhythmGrid | null, swing: number, sound: string | null): Promise<number>;
  /** Sound library: own sounds across all projects. */
  soundsList(): Promise<SavedSound[]>;
  /** Save the track's sound under a name (same name for the same instrument: replace). */
  soundSave(trackId: number, name: string): Promise<SoundSaveResult>;
  /** Load a saved sound into the track (one undo step). */
  soundLoad(trackId: number, id: string): Promise<Snapshot>;
  soundRename(id: string, name: string): Promise<SavedSound[]>;
  soundDelete(id: string): Promise<SavedSound[]>;
  /** Sound library changed (also by the agent). */
  onSoundsChanged(cb: (s: SavedSound[]) => void): void;
  instrumentSpecs(): Promise<InstrumentSpec[]>;
  createInstrumentTrack(kind: InstrumentKind): Promise<Snapshot>;
  insertPattern(trackId: number): Promise<Snapshot>;
  libraryInfo(): Promise<LibraryInfo>;
  /** Add files or folders to the active library (in the background). Browser: into the own scores, answers with an `AddResult`. */
  libraryAdd(paths: string[]): Promise<LibraryInfo | AddResult>;
  /** Browser: the own scores, remove one. */
  libraryOwnList(): Promise<OwnScore[]>;
  libraryOwnRemove(path: string): Promise<OwnScore[]>;
  /** Browser: what the browser cache holds of the package on the server; empty it. */
  libraryCacheInfo(): Promise<{ entries: number; bytes: number }>;
  libraryCacheClear(): Promise<void>;
  /** Browser: files of the computer into the core's file tree (paths there). */
  importFiles(files: File[]): Promise<string[]>;
  /** Manage libraries (one index each; only the active one is searched). */
  libraryList(): Promise<LibEntry[]>;
  libraryCreate(name: string): Promise<LibEntry[]>;
  librarySwitch(id: string): Promise<LibEntry[]>;
  libraryRename(id: string, name: string): Promise<LibEntry[]>;
  libraryDelete(id: string): Promise<LibEntry[]>;
  /** Files dragged onto the window (paths and position in CSS pixels; desktop app only). */
  onFileDrop(cb: (paths: string[], at: { x: number; y: number } | null) => void): void;
  /** Files are being dragged over the window (null: dragged away or dropped). */
  onFileDrag(cb: (drag: { paths: string[]; x: number; y: number } | null) => void): void;
  libraryCatalog(): Promise<CatalogEntry[]>;
  /** Loads a package – or queues it if a job is currently running. */
  libraryDownload(id: string, folder: string | null, withMeta: boolean): Promise<LibraryInfo>;
  /** Removes a waiting package from the queue (without `id`: all). */
  libraryDequeue(id: string | null, library: string | null): Promise<LibraryInfo>;
  libraryCancel(): Promise<void>;
  libraryEnrich(): Promise<LibraryInfo>;
  libraryDimensions(): Promise<Dimension[]>;
  libraryTree(levels: string[], path: string[], offset: number, limit: number): Promise<TreeLevel>;
  librarySearch(q: { text?: string; melody?: string; trackId?: number; clipId?: number; limit?: number }): Promise<LibHit[]>;
  libraryOpen(doc: number, noteIndex: number | null, queryLen: number | null, range?: [number, number] | null): Promise<ScoreView>;
  /** Voices [part, voice] as tracks (one per voice); with `range` (quarters) only that excerpt.
   *  `tempo` (quarters per minute): stretched so that it sounds that fast at the project tempo. */
  libraryToTrack(doc: number, voices: Array<[string, string]>, range?: [number, number] | null, tempo?: number | null): Promise<Snapshot>;
  /** Preview from `start` (quarters) to the end or – with `region` – that range repeatedly,
   *  until `previewStop`; the chosen voices [part, voice], without: all; `tempo` without: that of the score. */
  libraryPlay(doc: number, start: number, region: [number, number] | null, voices?: Array<[string, string]> | null, tempo?: number | null): Promise<PlayInfo>;
  /** End the running preview. */
  previewStop(): Promise<void>;
  /** Rebuild the active library from its sources (packages, folders). */
  libraryReindex(): Promise<LibraryInfo>;
  /** Record the microphone for melody search (does not go into the project). */
  libraryListenStart(): Promise<void>;
  /** End the recording, recognize the melody and search (`cancel`: only discard). */
  libraryListenStop(text: string | null, cancel?: boolean): Promise<ListenResult>;
  /** Key (null = estimated from the notes), circle of fifths and cadence templates. */
  harmonyView(key: KeySpec | null): Promise<HarmonyView>;
  harmonySuggest(key: KeySpec, chords: Chord[], group?: number): Promise<SuggestResult>;
  fugueSubject(trackId: number, clipId: number, key: KeySpec | null): Promise<SubjectView>;
  fugueTransform(trackId: number, clipId: number, key: KeySpec, transform: TransformSpec): Promise<VNote[]>;
  fugueCountersubjects(trackId: number, clipId: number, key: KeySpec): Promise<Countersubject[]>;
  fuguePlan(trackId: number, clipId: number, key: KeySpec, options: FugueOptions): Promise<FuguePlan>;
  /** Voices as new instrument tracks from startBeats (one undo step). */
  fugueAddVoices(voices: Array<{ name: string; notes: VNote[] }>, startBeats: number, kind: InstrumentKind, label: string): Promise<Snapshot>;
  fuguePreview(voices: VNote[][], kind: InstrumentKind): Promise<number>;
  counterpointCheck(trackIds: number[] | null): Promise<CheckView>;
  harmonyDescribe(key: KeySpec, chords: Chord[]): Promise<ChordInfo[]>;
  harmonyChords(key: KeySpec, trackId: number | null, clipId: number | null): Promise<PlacedChord[]>;
  harmonyPreview(key: KeySpec, chords: Chord[], beatsPerChord: number, trackId: number | null, before: Chord | null): Promise<number>;
  /** Write chords; `mode` "einfuegen" makes room (everything after it moves back in all tracks), "ersetzen" overwrites. */
  harmonyWrite(key: KeySpec, chords: Chord[], beatsPerChord: number, startBeats: number, trackId: number | null, before: Chord | null, mode?: "einfuegen" | "ersetzen"): Promise<Snapshot>;
  /** Chord progression of the whole piece with mood and breaks (harmony track of the timeline). */
  harmonyTimeline(key: KeySpec | null): Promise<HarmonyLine>;
  harmonyPlayExample(doc: number, start: number, end: number): Promise<number>;
  onLibraryChanged(cb: (i: LibraryInfo) => void): void;
  /** Browser: a request to the score library server waits for its request limit (seconds). */
  onLibraryWait(cb: (seconds: number) => void): void;
  chatSend(text: string): Promise<ChatView>;
  /** The person stops task `id`; `text` appears in the chat and goes to the agent. */
  chatStop(id: number, text: string): Promise<ChatView>;
  chatHistory(): Promise<ChatView>;
  onChatChanged(cb: (c: ChatView) => void): void;
  onStatus(cb: (s: EngineStatus) => void): void;
  /** Project was changed from outside (agent). */
  onProjectChanged(cb: (s: Snapshot) => void): void;
  /** Save: without a path into the current file; a name ends up in the project folder. */
  projectSave(path: string | null): Promise<Snapshot>;
  projectOpen(path: string): Promise<Snapshot>;
  /** The system's file picker for a .klangwelt file; the path to open it with (null: cancelled).
   *  Desktop: the native dialog; browser: the file goes into the browser's storage first. */
  projectPick(): Promise<string | null>;
  /** Autosave: changes into the project's file, a project without a file into the
   *  recovery copy (`recoveryOnly`: a project with a file, too – its file stays
   *  untouched); without changes an old recovery copy is removed. */
  projectAutosave(recoveryOnly: boolean): Promise<AutosaveResult>;
  /** Open the recovery copy (unsaved, with the file it belongs to, if any); null: there is none. */
  projectRecover(): Promise<Snapshot | null>;
  /** Wait until every file the core wrote is stored for good (browser: OPFS; desktop: at once). */
  syncStorage(): Promise<void>;
  projectNew(): Promise<Snapshot>;
  /** [project folder, projects in it (newest first)] */
  projectList(): Promise<[string, ProjectFile[]]>;
  exportInfo(): Promise<ExportInfo>;
  exportRun(options: ExportOptions): Promise<ExportResult>;
  /** Show a file or folder in Explorer. */
  exportReveal(path: string): Promise<void>;
  /** Close the app, even with unsaved changes. */
  appClose(): Promise<void>;
  /** The window is about to close, but there are unsaved changes. */
  /** Closing halted: unsaved changes and/or running library work. */
  onCloseRequested(cb: (info: { dirty: boolean; busy: string | null }) => void): void;
  onAgentActivity(cb: (a: AgentActivity) => void): void;
  /** A project file is to be opened (double-click while Klangwelt is running, or agent). */
  onOpenFile(cb: (path: string) => void): void;
}

/** The mapping method → core command – one for desktop and browser.
 *  Both cores have the same command table (`crates/klang-app/src/commands.rs`). */
function coreBackend(link: CoreLink, platform: "desktop" | "web"): Backend {
  const call = <T>(cmd: string, args?: Record<string, unknown>) => link.call<T>(cmd, args);
  const listen = <T>(event: string, cb: (payload: T) => void) => link.listen<T>(event, cb);
  return {
    native: true,
    platform,
    setLanguage: (lang, dict) => call("set_language", { lang, dict }),
    getState: () => call("get_state"),
    execute: (command, merge = false) => call("execute", { command, merge }),
    undo: () => call("undo"),
    redo: () => call("redo"),
    historyView: () => call("history_view"),
    historyGoto: (node) => call("history_goto", { node }),
    historyCheckpoint: (name) => call("history_checkpoint", { name }),
    historyCheckpointRename: (id, name) => call("history_checkpoint_rename", { id, name }),
    historyCheckpointDelete: (id) => call("history_checkpoint_delete", { id }),
    historyVariantNew: (name, node) => call("history_variant_new", { name, node }),
    historyVariantSwitch: (id) => call("history_variant_switch", { id }),
    historyVariantRename: (id, name) => call("history_variant_rename", { id, name }),
    historyVariantDelete: (id) => call("history_variant_delete", { id }),
    historyClear: () => call("history_clear"),
    historyMergePlan: (target, others) => call("history_merge_plan", { target, others }),
    historyMergePreview: (target, others, choices, startBeats) => call("history_merge_preview", { target, others, choices, startBeats }),
    historyMergePreviewStop: () => call("history_merge_preview_stop"),
    historyMergeApply: (target, others, choices, name) => call("history_merge_apply", { target, others, choices, name }),
    transport: (action) => call("transport", { action }),
    seek: (seconds) => call("seek", { seconds }),
    setMetronome: (enabled) => call("set_metronome", { enabled }),
    setLoop: (enabled) => call("set_loop", { enabled }),
    setLoopRange: (range) => call("set_loop_range", { range }),
    audioDevices: () => call("audio_devices"),
    setAudioDevice: (kind, name) => call("audio_set_device", { kind, name }),
    recordStart: () => call("record_start"),
    recordStop: () => call("record_stop"),
    recordLive: (since) => call("record_live", { since }),
    getWaveform: (assetId, buckets) => call("get_waveform", { assetId, buckets }),
    analyzeAsset: (assetId) => call("analyze_asset", { assetId }),
    createMelodyTrack: (trackId, clipId, smooth) => call("create_melody_track", { trackId, clipId, smooth }),
    audioImport: (paths, trackId, startBeats) => call("audio_import", { paths, trackId, startBeats }),
    audioToNotes: (trackId, clipId, target, smooth) => call("audio_to_notes", { trackId, clipId, target, smooth }),
    deleteRange: (startBeats, endBeats) => call("delete_range", { startBeats, endBeats }),
    quantizeNotes: (trackId, clipId, indices, grid, lengths) => call("quantize_notes", { trackId, clipId, indices, grid, lengths }),
    smoothNotes: (trackId, clipId, indices, strength) => call("smooth_notes", { trackId, clipId, indices, strength }),
    createDemo: () => call("create_demo"),
    previewTrack: (trackId, pattern) => call("preview_track", { trackId, pattern }),
    previewNote: (trackId, midi) => call("preview_note", { trackId, midi }),
    describeSound: (trackId) => call("describe_sound", { trackId }),
    soundWave: (trackId) => call("sound_wave", { trackId }),
    soundField: (trackId, field, x, y, preset, merge) => call("sound_field", { trackId, field, x, y, preset, merge }),
    songTemplates: () => call("song_templates"),
    songWrite: (plan, replace) => call("song_write", { plan, replace }),
    rhythmInfo: () => call("rhythm_info"),
    rhythmRead: (trackId, clipId) => call("rhythm_read", { trackId, clipId }),
    rhythmToggle: (trackId, clipId, row, step, on, swing) => call("rhythm_toggle", { trackId, clipId, row, step, on, swing }),
    rhythmSwing: (trackId, clipId, swing, merge) => call("rhythm_swing", { trackId, clipId, swing, merge }),
    rhythmWrite: (grid, swing, trackId, clipId, startBeats, name, sound, tempo) => call("rhythm_write", { grid, swing, trackId, clipId, startBeats, name, sound, tempo }),
    rhythmPreview: (trackId, clipId, grid, swing, sound) => call("rhythm_preview", { trackId, clipId, grid, swing, sound }),
    soundsList: () => call("sounds_list"),
    soundSave: (trackId, name) => call("sound_save", { trackId, name }),
    soundLoad: (trackId, id) => call("sound_load", { trackId, id }),
    soundRename: (id, name) => call("sound_rename", { id, name }),
    soundDelete: (id) => call("sound_delete", { id }),
    onSoundsChanged: (cb) => {
      listen<SavedSound[]>("sounds-changed", (p) => cb(p));
    },
    instrumentSpecs: () => call("instrument_specs"),
    createInstrumentTrack: (kind) => call("create_instrument_track", { kind }),
    insertPattern: (trackId) => call("insert_pattern", { trackId }),
    libraryInfo: () => call("library_info"),
    libraryAdd: (paths) => call("library_add", { paths }),
    libraryOwnList: () => call("library_own_list"),
    libraryOwnRemove: (path) => call("library_own_remove", { path }),
    libraryCacheInfo: () => call("web_library_cache"),
    libraryCacheClear: () => call("web_library_clear"),
    importFiles: (files) => call("web_import_files", { files }),
    libraryList: () => call("library_list"),
    libraryCreate: (name) => call("library_create", { name }),
    librarySwitch: (id) => call("library_switch", { id }),
    libraryRename: (id, name) => call("library_rename", { id, name }),
    libraryDelete: (id) => call("library_delete", { id }),
    onFileDrop: (cb) => {
      // Dragged files including position (CSS pixels); desktop: Tauri, browser: web/host.ts
      listen<{ paths: string[]; position?: { x: number; y: number } | null }>("file-drop", (p) => cb(p.paths ?? [], p.position ?? null));
    },
    onFileDrag: (cb) => {
      listen<{ paths: string[]; x: number; y: number } | null>("file-drag", (p) => cb(p));
    },
    libraryCatalog: () => call("library_catalog"),
    libraryDownload: (id, folder, withMeta) => call("library_download", { id, folder, withMeta }),
    libraryDequeue: (id, library) => call("library_dequeue", { id, library }),
    libraryCancel: () => call("library_cancel"),
    libraryEnrich: () => call("library_enrich"),
    libraryDimensions: () => call("library_dimensions"),
    libraryTree: (levels, path, offset, limit) => call("library_tree", { levels, path, offset, limit }),
    librarySearch: (q) => call("library_search", q),
    libraryOpen: (doc, noteIndex, queryLen, range = null) => call("library_open", { doc, noteIndex, queryLen, range }),
    libraryToTrack: (doc, voices, range = null, tempo = null) => call("library_to_track", { doc, voices, range, tempo }),
    libraryPlay: (doc, start, region, voices = null, tempo = null) => call("library_play", { doc, start, region, voices, tempo }),
    previewStop: () => call("preview_stop"),
    libraryReindex: () => call("library_reindex"),
    libraryListenStart: () => call("library_listen_start"),
    libraryListenStop: (text, cancel = false) => call("library_listen_stop", { text, cancel }),
    harmonyView: (key) => call("harmony_view", { key }),
    harmonySuggest: (key, chords, group = 0) => call("harmony_suggest", { key, chords, group }),
    fugueSubject: (trackId, clipId, key) => call("fugue_subject", { trackId, clipId, key }),
    fugueTransform: (trackId, clipId, key, transform) => call("fugue_transform", { trackId, clipId, key, transform }),
    fugueCountersubjects: (trackId, clipId, key) => call("fugue_countersubjects", { trackId, clipId, key }),
    fuguePlan: (trackId, clipId, key, options) => call("fugue_plan", { trackId, clipId, key, options }),
    fugueAddVoices: (voices, startBeats, kind, label) => call("fugue_add_voices", { voices, startBeats, kind, label }),
    fuguePreview: (voices, kind) => call("fugue_preview", { voices, kind }),
    counterpointCheck: (trackIds) => call("counterpoint_check", { trackIds }),
    harmonyDescribe: (key, chords) => call("harmony_describe", { key, chords }),
    harmonyChords: (key, trackId, clipId) => call("harmony_chords", { key, trackId, clipId }),
    harmonyPreview: (key, chords, beatsPerChord, trackId, before) => call("harmony_preview", { key, chords, beatsPerChord, trackId, before }),
    harmonyWrite: (key, chords, beatsPerChord, startBeats, trackId, before, mode = "ersetzen") =>
      call("harmony_write", { key, chords, beatsPerChord, startBeats, trackId, before, mode }),
    harmonyTimeline: (key) => call("harmony_timeline", { key }),
    harmonyPlayExample: (doc, start, end) => call("harmony_play_example", { doc, start, end }),
    onLibraryChanged: (cb) => {
      listen<LibraryInfo>("library-changed", (p) => cb(p));
    },
    onLibraryWait: (cb) => {
      listen<{ seconds: number }>("library-wait", (p) => cb(p.seconds));
    },
    chatSend: (text) => call("chat_send", { text }),
    chatStop: (id, text) => call("chat_stop", { id, text }),
    chatHistory: () => call("chat_history"),
    onChatChanged: (cb) => {
      listen<ChatView>("chat-changed", (p) => cb(p));
    },
    onStatus: (cb) => {
      listen<EngineStatus>("engine-status", (p) => cb(p));
    },
    onProjectChanged: (cb) => {
      listen<Snapshot>("project-changed", (p) => cb(p));
    },
    projectSave: (path) => call("project_save", { path }),
    projectOpen: (path) => call("project_open", { path }),
    projectPick: () => call("project_pick"),
    projectAutosave: (recoveryOnly) => call("project_autosave", { recoveryOnly }),
    projectRecover: () => call("project_recover"),
    syncStorage: async () => {
      if (platform === "web") await call("web_sync");
    },
    projectNew: () => call("project_new"),
    projectList: () => call("project_list"),
    exportInfo: () => call("export_info"),
    exportRun: (options) => call("export_run", { options }),
    exportReveal: (path) => call("export_reveal", { path }),
    appClose: () => call("app_close"),
    onCloseRequested: (cb) => {
      listen<{ dirty: boolean; busy: string | null } | null>("close-requested", (p) => cb(p ?? { dirty: true, busy: null }));
    },
    onAgentActivity: (cb) => {
      listen<AgentActivity>("agent-activity", (p) => cb(p));
    },
    onOpenFile: (cb) => {
      listen<string>("open-file", (p) => cb(p));
    },
  };
}

/** Position from a Tauri drag event (physical pixels) in CSS pixels. */
function cssPoint(p?: { x: number; y: number } | null): { x: number; y: number } | null {
  if (!p || !Number.isFinite(p.x) || !Number.isFinite(p.y)) return null;
  const dpr = window.devicePixelRatio || 1;
  return { x: p.x / dpr, y: p.y / dpr };
}

type DragPayload = { paths?: string[]; position?: { x: number; y: number } };

/** Tauri IPC: commands as `invoke`, events as Tauri events. */
function tauriLink(t: TauriGlobal): CoreLink {
  return {
    call: (cmd, args) => t.core.invoke(cmd, args),
    listen: (event, cb) => {
      // Tauri reports dragged files under its own names – translate into
      // "file-drop" { paths, position } and "file-drag" { paths, x, y } | null
      const emit = cb as (p: unknown) => void;
      if (event === "file-drop") {
        void t.event.listen<DragPayload>("tauri://drag-drop", (e) => emit({ paths: e.payload.paths ?? [], position: cssPoint(e.payload.position) }));
      } else if (event === "file-drag") {
        // Tauri reports the paths only on drag-in, afterwards only the position
        let paths: string[] = [];
        const at = (p?: { x: number; y: number }) => cssPoint(p) ?? { x: -1, y: -1 };
        void t.event.listen<DragPayload>("tauri://drag-enter", (e) => {
          paths = e.payload.paths ?? [];
          emit({ paths, ...at(e.payload.position) });
        });
        void t.event.listen<DragPayload>("tauri://drag-over", (e) => emit({ paths, ...at(e.payload.position) }));
        void t.event.listen("tauri://drag-leave", () => emit(null));
        void t.event.listen("tauri://drag-drop", () => emit(null));
      } else {
        void t.event.listen(event, (e) => cb(e.payload as never));
      }
    },
  };
}

/** Mock explicitly requested (`?attrappe`)? */
function mockWanted(): boolean {
  try {
    return new URLSearchParams(location.search).has("attrappe");
  } catch {
    return false;
  }
}

export const backend: Backend = window.__TAURI__
  ? coreBackend(tauriLink(window.__TAURI__), "desktop")
  : mockWanted()
    ? createMock()
    : coreBackend(createWebLink(), "web");
