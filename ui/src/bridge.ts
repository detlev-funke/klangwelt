// Verbindung zum Rust-Kern (`klang-app`). Desktop: über Tauri-IPC. Browser:
// derselbe Kern als WebAssembly in einem Web Worker (`web/host.ts`). Beide
// benutzen dieselbe Zuordnung (`coreBackend`) – nur der Transport ist anders.
// Die Attrappe (`mock.ts`, ohne Audio, vereinfacht) gibt es weiter für die
// UI-Arbeit: `?attrappe` an der Adresse.

import type {
  AgentActivity, Analysis, CatalogEntry, CheckView, ExportInfo, ExportOptions, ExportResult, Chord, ChordInfo, ChatView, Command, Countersubject, Dimension, EngineStatus, FugueOptions,
  FuguePlan, HarmonyView, InstrumentKind, InstrumentSpec, SubjectView, TransformSpec, VNote,
  ConvertResult, ConvertTarget, ImportResult, QuantizeResult, KeySpec, LibEntry, LibHit, LibraryInfo, ListenResult, PlacedChord, PlayInfo, PreviewPattern, ProjectFile, RecordResult, HarmonyLine, SongPlan, SongWritten, ClipGrid, RhythmGrid, RhythmInfo, RhythmWritten, SavedSound, ScoreView, Snapshot, SoundReport, SoundSaveResult, SuggestResult, TreeLevel, WaveView,
} from "./types.js";
import type { AudioDevices, AudioKind } from "./types.js";
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

/** Transport zum Kern: Befehl aufrufen (Name + Argumente, wie in der
 *  Befehlstabelle des Kerns) und Ereignisse empfangen. */
export interface CoreLink {
  call<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, cb: (payload: T) => void): void;
}

/** Wo die Oberfläche läuft: Desktop-App, Browser mit echtem Kern, Attrappe. */
export type Platform = "desktop" | "web" | "mock";

export type TransportAction = "play" | "stop" | "toggle" | "rewind";

export interface Backend {
  /** Echter Kern (Desktop oder Browser) – nicht die Attrappe. */
  readonly native: boolean;
  readonly platform: Platform;
  getState(): Promise<Snapshot>;
  execute(command: Command, merge?: boolean): Promise<Snapshot>;
  undo(): Promise<Snapshot>;
  redo(): Promise<Snapshot>;
  transport(action: TransportAction): Promise<void>;
  seek(seconds: number): Promise<void>;
  setMetronome(enabled: boolean): Promise<Snapshot>;
  /** Wiederholen ein/aus. */
  setLoop(enabled: boolean): Promise<Snapshot>;
  /** Zeitbereich in Schlägen markieren; null hebt ihn auf. */
  setLoopRange(range: [number, number] | null): Promise<Snapshot>;
  /** Ausgabe- und Aufnahmegeräte des Systems samt Wahl. */
  audioDevices(): Promise<AudioDevices>;
  /** Gerät wählen (null = Systemstandard) – wirkt sofort und gilt beim nächsten Start. */
  setAudioDevice(kind: AudioKind, name: string | null): Promise<Snapshot>;
  recordStart(): Promise<void>;
  recordStop(): Promise<RecordResult>;
  getWaveform(assetId: string, buckets: number): Promise<number[]>;
  analyzeAsset(assetId: string): Promise<Analysis>;
  createMelodyTrack(trackId: number, clipId: number): Promise<Snapshot>;
  /** Audiodateien einfügen: auf eine Audiospur die erste Datei hinein, sonst je Datei eine neue Spur (unter `trackId`), ab `startBeats`. */
  audioImport(paths: string[], trackId: number | null, startBeats: number): Promise<ImportResult>;
  /** Aufnahme in eine Melodie- oder Rhythmus-Spur umwandeln (`clipId` null: alle Audio-Clips der Spur). */
  audioToNotes(trackId: number, clipId: number | null, target: ConvertTarget): Promise<ConvertResult>;
  /** Zeitbereich aus allen Spuren löschen, alles danach rückt auf (hebt die Markierung auf). */
  deleteRange(startBeats: number, endBeats: number): Promise<Snapshot>;
  /** Noten am Raster ausrichten (`indices` null: alle Noten des Clips); `grid` in Schlägen. */
  quantizeNotes(trackId: number, clipId: number, indices: number[] | null, grid: number, lengths: boolean): Promise<QuantizeResult>;
  createDemo(): Promise<Snapshot>;
  previewTrack(trackId: number, pattern: PreviewPattern): Promise<number>;
  previewNote(trackId: number, midi: number): Promise<void>;
  describeSound(trackId: number): Promise<SoundReport>;
  /** Wellenform des Testklangs der Spur (Klang-Fenster). */
  soundWave(trackId: number): Promise<WaveView>;
  /** Punkt (x, y je 0…1) oder Startpunkt in einem Feld setzen; `merge` = Teil einer Ziehbewegung. */
  soundField(trackId: number, field: string, x: number | null, y: number | null, preset: string | null, merge: boolean): Promise<Snapshot>;
  /** Song-Vorlagen (ganze Stücke aus Abschnitten und Rollen). */
  songTemplates(): Promise<SongPlan[]>;
  /** Song erzeugen; `replace` ersetzt die zuletzt erzeugten Song-Spuren. */
  songWrite(plan: SongPlan, replace: boolean): Promise<SongWritten>;
  /** Rhythmus: Genre-Muster und Zeilen des Schritt-Rasters. */
  rhythmInfo(): Promise<RhythmInfo>;
  /** Schlagzeug-Clip als Raster (bis 4 Takte). */
  rhythmRead(trackId: number, clipId: number): Promise<ClipGrid>;
  /** Einen Schritt im Clip setzen oder löschen (ein Rückgängig-Schritt). */
  rhythmToggle(trackId: number, clipId: number, row: string, step: number, on: boolean, swing: number): Promise<Snapshot>;
  /** Swing des Clips ändern; `merge` = Teil einer Reglerbewegung. */
  rhythmSwing(trackId: number, clipId: number, swing: number, merge: boolean): Promise<Snapshot>;
  /** Raster schreiben: in einen Clip (ersetzt), als neuen Clip in eine Spur oder in eine neue Schlagzeug-Spur. */
  rhythmWrite(grid: RhythmGrid, swing: number, trackId: number | null, clipId: number | null, startBeats: number, name: string, sound: string | null, tempo: number | null): Promise<RhythmWritten>;
  /** Raster oder Clip im Projekttempo vorspielen; liefert die Dauer in Sekunden. */
  rhythmPreview(trackId: number | null, clipId: number | null, grid: RhythmGrid | null, swing: number, sound: string | null): Promise<number>;
  /** Klang-Bibliothek: eigene Klänge über alle Projekte. */
  soundsList(): Promise<SavedSound[]>;
  /** Klang der Spur unter einem Namen sichern (gleicher Name beim selben Instrument: ersetzen). */
  soundSave(trackId: number, name: string): Promise<SoundSaveResult>;
  /** Gesicherten Klang in die Spur laden (ein Rückgängig-Schritt). */
  soundLoad(trackId: number, id: string): Promise<Snapshot>;
  soundRename(id: string, name: string): Promise<SavedSound[]>;
  soundDelete(id: string): Promise<SavedSound[]>;
  /** Klang-Bibliothek geändert (auch durch den Agent). */
  onSoundsChanged(cb: (s: SavedSound[]) => void): void;
  instrumentSpecs(): Promise<InstrumentSpec[]>;
  createInstrumentTrack(kind: InstrumentKind): Promise<Snapshot>;
  insertPattern(trackId: number): Promise<Snapshot>;
  libraryInfo(): Promise<LibraryInfo>;
  /** Dateien oder Ordner in die aktive Bibliothek aufnehmen (im Hintergrund). */
  libraryAdd(paths: string[]): Promise<LibraryInfo>;
  /** Bibliotheken verwalten (je ein Index; gesucht wird nur in der aktiven). */
  libraryList(): Promise<LibEntry[]>;
  libraryCreate(name: string): Promise<LibEntry[]>;
  librarySwitch(id: string): Promise<LibEntry[]>;
  libraryRename(id: string, name: string): Promise<LibEntry[]>;
  libraryDelete(id: string): Promise<LibEntry[]>;
  /** Dateien, die auf das Fenster gezogen werden (Pfade und Stelle in CSS-Pixeln; nur Desktop-App). */
  onFileDrop(cb: (paths: string[], at: { x: number; y: number } | null) => void): void;
  /** Dateien werden über das Fenster gezogen (null: weggezogen oder abgelegt). */
  onFileDrag(cb: (drag: { paths: string[]; x: number; y: number } | null) => void): void;
  libraryCatalog(): Promise<CatalogEntry[]>;
  /** Lädt ein Paket – oder reiht es ein, wenn gerade ein Auftrag läuft. */
  libraryDownload(id: string, folder: string | null, withMeta: boolean): Promise<LibraryInfo>;
  /** Nimmt ein wartendes Paket aus der Warteschlange (ohne `id`: alle). */
  libraryDequeue(id: string | null, library: string | null): Promise<LibraryInfo>;
  libraryCancel(): Promise<void>;
  libraryEnrich(): Promise<LibraryInfo>;
  libraryDimensions(): Promise<Dimension[]>;
  libraryTree(levels: string[], path: string[], offset: number, limit: number): Promise<TreeLevel>;
  librarySearch(q: { text?: string; melody?: string; trackId?: number; clipId?: number; limit?: number }): Promise<LibHit[]>;
  libraryOpen(doc: number, noteIndex: number | null, queryLen: number | null, range?: [number, number] | null): Promise<ScoreView>;
  /** Stimmen [Teil, Stimme] als Spuren (je Stimme eine); mit `range` (Viertel) nur diesen Ausschnitt.
   *  `tempo` (Viertel je Minute): so gestreckt, dass es im Projekttempo so schnell klingt. */
  libraryToTrack(doc: number, voices: Array<[string, string]>, range?: [number, number] | null, tempo?: number | null): Promise<Snapshot>;
  /** Vorspielen ab `start` (Viertel) bis zum Ende oder – mit `region` – diesen Bereich immer wieder,
   *  bis `previewStop`; die gewählten Stimmen [Teil, Stimme], ohne: alle; `tempo` ohne: das der Partitur. */
  libraryPlay(doc: number, start: number, region: [number, number] | null, voices?: Array<[string, string]> | null, tempo?: number | null): Promise<PlayInfo>;
  /** Laufendes Vorhören beenden. */
  previewStop(): Promise<void>;
  /** Aktive Bibliothek aus ihren Quellen (Pakete, Ordner) neu aufbauen. */
  libraryReindex(): Promise<LibraryInfo>;
  /** Mikrofon für die Melodiesuche aufnehmen (kommt nicht ins Projekt). */
  libraryListenStart(): Promise<void>;
  /** Aufnahme beenden, Melodie erkennen und suchen (`cancel`: nur verwerfen). */
  libraryListenStop(text: string | null, cancel?: boolean): Promise<ListenResult>;
  /** Tonart (null = aus den Noten geschätzt), Quintenzirkel und Kadenzvorlagen. */
  harmonyView(key: KeySpec | null): Promise<HarmonyView>;
  harmonySuggest(key: KeySpec, chords: Chord[], group?: number): Promise<SuggestResult>;
  fugueSubject(trackId: number, clipId: number, key: KeySpec | null): Promise<SubjectView>;
  fugueTransform(trackId: number, clipId: number, key: KeySpec, transform: TransformSpec): Promise<VNote[]>;
  fugueCountersubjects(trackId: number, clipId: number, key: KeySpec): Promise<Countersubject[]>;
  fuguePlan(trackId: number, clipId: number, key: KeySpec, options: FugueOptions): Promise<FuguePlan>;
  /** Stimmen als neue Instrumentspuren ab startBeats (ein Undo-Schritt). */
  fugueAddVoices(voices: Array<{ name: string; notes: VNote[] }>, startBeats: number, kind: InstrumentKind, label: string): Promise<Snapshot>;
  fuguePreview(voices: VNote[][], kind: InstrumentKind): Promise<number>;
  counterpointCheck(trackIds: number[] | null): Promise<CheckView>;
  harmonyDescribe(key: KeySpec, chords: Chord[]): Promise<ChordInfo[]>;
  harmonyChords(key: KeySpec, trackId: number | null, clipId: number | null): Promise<PlacedChord[]>;
  harmonyPreview(key: KeySpec, chords: Chord[], beatsPerChord: number, trackId: number | null, before: Chord | null): Promise<number>;
  /** Akkorde schreiben; `mode` „einfuegen“ schafft Platz (alles danach rückt in allen Spuren nach hinten), „ersetzen“ überschreibt. */
  harmonyWrite(key: KeySpec, chords: Chord[], beatsPerChord: number, startBeats: number, trackId: number | null, before: Chord | null, mode?: "einfuegen" | "ersetzen"): Promise<Snapshot>;
  /** Akkordfolge des ganzen Satzes mit Stimmung und Brüchen (Harmonie-Spur der Zeitleiste). */
  harmonyTimeline(key: KeySpec | null): Promise<HarmonyLine>;
  harmonyPlayExample(doc: number, start: number, end: number): Promise<number>;
  onLibraryChanged(cb: (i: LibraryInfo) => void): void;
  chatSend(text: string): Promise<ChatView>;
  chatHistory(): Promise<ChatView>;
  onChatChanged(cb: (c: ChatView) => void): void;
  onStatus(cb: (s: EngineStatus) => void): void;
  /** Projekt wurde von außen (Agent) geändert. */
  onProjectChanged(cb: (s: Snapshot) => void): void;
  /** Speichern: ohne Pfad in die aktuelle Datei; ein Name landet im Projektordner. */
  projectSave(path: string | null): Promise<Snapshot>;
  projectOpen(path: string): Promise<Snapshot>;
  projectNew(): Promise<Snapshot>;
  /** [Projektordner, Projekte darin (neueste zuerst)] */
  projectList(): Promise<[string, ProjectFile[]]>;
  exportInfo(): Promise<ExportInfo>;
  exportRun(options: ExportOptions): Promise<ExportResult>;
  /** Datei oder Ordner im Explorer zeigen. */
  exportReveal(path: string): Promise<void>;
  /** App schließen, auch mit ungespeicherten Änderungen. */
  appClose(): Promise<void>;
  /** Fenster soll schließen, es gibt aber ungespeicherte Änderungen. */
  /** Schließen angehalten: ungespeicherte Änderungen und/oder laufende Bibliotheksarbeit. */
  onCloseRequested(cb: (info: { dirty: boolean; busy: string | null }) => void): void;
  onAgentActivity(cb: (a: AgentActivity) => void): void;
  /** Eine Projektdatei soll geöffnet werden (Doppelklick, während Klangwelt läuft, oder Agent). */
  onOpenFile(cb: (path: string) => void): void;
}

/** Die Zuordnung Methode → Befehl des Kerns – eine für Desktop und Browser.
 *  Beide Kerne haben dieselbe Befehlstabelle (`crates/klang-app/src/commands.rs`). */
function coreBackend(link: CoreLink, platform: "desktop" | "web"): Backend {
  const call = <T>(cmd: string, args?: Record<string, unknown>) => link.call<T>(cmd, args);
  const listen = <T>(event: string, cb: (payload: T) => void) => link.listen<T>(event, cb);
  return {
    native: true,
    platform,
    getState: () => call("get_state"),
    execute: (command, merge = false) => call("execute", { command, merge }),
    undo: () => call("undo"),
    redo: () => call("redo"),
    transport: (action) => call("transport", { action }),
    seek: (seconds) => call("seek", { seconds }),
    setMetronome: (enabled) => call("set_metronome", { enabled }),
    setLoop: (enabled) => call("set_loop", { enabled }),
    setLoopRange: (range) => call("set_loop_range", { range }),
    audioDevices: () => call("audio_devices"),
    setAudioDevice: (kind, name) => call("audio_set_device", { kind, name }),
    recordStart: () => call("record_start"),
    recordStop: () => call("record_stop"),
    getWaveform: (assetId, buckets) => call("get_waveform", { assetId, buckets }),
    analyzeAsset: (assetId) => call("analyze_asset", { assetId }),
    createMelodyTrack: (trackId, clipId) => call("create_melody_track", { trackId, clipId }),
    audioImport: (paths, trackId, startBeats) => call("audio_import", { paths, trackId, startBeats }),
    audioToNotes: (trackId, clipId, target) => call("audio_to_notes", { trackId, clipId, target }),
    deleteRange: (startBeats, endBeats) => call("delete_range", { startBeats, endBeats }),
    quantizeNotes: (trackId, clipId, indices, grid, lengths) => call("quantize_notes", { trackId, clipId, indices, grid, lengths }),
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
    libraryList: () => call("library_list"),
    libraryCreate: (name) => call("library_create", { name }),
    librarySwitch: (id) => call("library_switch", { id }),
    libraryRename: (id, name) => call("library_rename", { id, name }),
    libraryDelete: (id) => call("library_delete", { id }),
    onFileDrop: (cb) => {
      // Gezogene Dateien samt Stelle (CSS-Pixel); Desktop: Tauri, Browser: web/host.ts
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
    chatSend: (text) => call("chat_send", { text }),
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

/** Stelle aus einem Tauri-Zieh-Ereignis (physische Pixel) in CSS-Pixeln. */
function cssPoint(p?: { x: number; y: number } | null): { x: number; y: number } | null {
  if (!p || !Number.isFinite(p.x) || !Number.isFinite(p.y)) return null;
  const dpr = window.devicePixelRatio || 1;
  return { x: p.x / dpr, y: p.y / dpr };
}

type DragPayload = { paths?: string[]; position?: { x: number; y: number } };

/** Tauri-IPC: Befehle als `invoke`, Ereignisse als Tauri-Events. */
function tauriLink(t: TauriGlobal): CoreLink {
  return {
    call: (cmd, args) => t.core.invoke(cmd, args),
    listen: (event, cb) => {
      // Gezogene Dateien meldet Tauri unter eigenen Namen – übersetzen in
      // „file-drop“ { paths, position } und „file-drag“ { paths, x, y } | null
      const emit = cb as (p: unknown) => void;
      if (event === "file-drop") {
        void t.event.listen<DragPayload>("tauri://drag-drop", (e) => emit({ paths: e.payload.paths ?? [], position: cssPoint(e.payload.position) }));
      } else if (event === "file-drag") {
        // Tauri meldet die Pfade nur beim Hereinziehen, danach nur noch die Stelle
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

/** Attrappe ausdrücklich verlangt (`?attrappe`)? */
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
