// Mirror of the Rust types (serde, camelCase).

export type Id = number;

export interface Note {
  pitch: number;
  startBeats: number;
  lengthBeats: number;
  /** Velocity 1–127. */
  velocity: number;
  /** Vibrato depth in semitones 0–1 (missing = 0). */
  vibrato?: number;
  /** Brightness −1…+1 (missing = 0). */
  brightness?: number;
  /** Glide-in in semitones −12…+12 (missing = 0). */
  bend?: number;
}

export type ClipSource =
  | { type: "audio"; assetId: string; sourceBpm: number; warp: boolean; durationSecs: number }
  | { type: "notes"; lengthBeats: number; notes: Note[] };

export interface Clip {
  id: Id;
  name: string;
  startBeats: number;
  source: ClipSource;
}

export type TrackKind = "audio" | "instrument";

export type InstrumentKind = "pluck" | "drums" | "bass" | "organ" | "harpsichord" | "synth";

/** Curve point: [seconds from note start, value 0…1]. */
export type CurvePoint = [number, number];

export interface Instrument {
  kind: InstrumentKind;
  preset: string;
  params: Record<string, number>;
  /** Drawn curves (synth: "loud", "bright"). */
  curves?: Record<string, CurvePoint[]>;
}

export interface ParamSpec {
  key: string;
  label: string;
  group: string;
  min: number;
  max: number;
  default: number;
  left: string;
  right: string;
  /** Value is a MIDI note */
  note: boolean;
  meaning: string;
  /** No control of its own (field position, curve). */
  hidden?: boolean;
}

/** Field in the Sound window: two axes, starting points, controls that have been set. */
export interface FieldSpec {
  key: "charakter" | "klangfarbe" | "raum";
  name: string;
  x: { param: string; label: string; words: [string, string, string, string] };
  y: { param: string; label: string; words: [string, string, string, string] };
  presets: Array<{ name: string; x: number; y: number }>;
  sets: string[];
}

/** Template for the synth's curve. */
export interface VerlaufSpec {
  name: string;
  beschreibung: string;
  laut: CurvePoint[];
  hell: CurvePoint[];
  ausklang: number;
  puls: number;
  pulstiefe: number;
}

/** Song structure: role of a row. */
export type SongRole = "pad" | "arp" | "bass" | "lead" | "kick" | "hats" | "clap" | "perc" | "fx";
export type SongSectionKind = "intro" | "teil" | "aufbau" | "break" | "hoehepunkt" | "outro";

export interface SongLayer {
  key: string;
  name: string;
  role: SongRole;
  kind?: InstrumentKind | null;
  sound?: string | null;
  verlauf?: string | null;
  raum?: string | null;
  gainDb?: number;
}

export interface SongSection {
  name: string;
  bars: number;
  kind: SongSectionKind;
  /** Level per role: 0 off, 1 sparse, 2 full */
  levels: Record<string, number>;
}

export interface SongPlan {
  template: string;
  name: string;
  description: string;
  tempo: number;
  key: string;
  chords: string[];
  chordBars: number;
  drums?: string | null;
  swing?: number;
  layers: SongLayer[];
  sections: SongSection[];
}

export interface SongWritten {
  snapshot: Snapshot;
  trackIds: number[];
  replaced: number;
  bars: number;
  seconds: number;
}

/** Rhythm: row of the step grid (one drum). */
export interface RhythmRow {
  key: string;
  label: string;
  midi: number;
  /** always visible (otherwise under "more drums") */
  main: boolean;
}

/** Step grid: length in sixteenths, per row the steps that are set (from 0). */
export interface RhythmGrid {
  steps: number;
  rows: Record<string, number[]>;
}

export interface RhythmPattern {
  name: string;
  bpm: number;
  swing: number;
  tip: string;
  /** one bar */
  grid: RhythmGrid;
}

export interface RhythmGenre {
  name: string;
  blurb: string;
  /** Starting point in the character field for a new drum track */
  sound: string;
  patterns: RhythmPattern[];
}

export interface RhythmInfo {
  rows: RhythmRow[];
  genres: RhythmGenre[];
}

export interface ClipGrid {
  grid: RhythmGrid;
  swing: number;
  /** Notes that do not appear in the grid */
  skipped: number;
  name: string;
}

export interface RhythmWritten {
  snapshot: Snapshot;
  trackId: number;
  clipId: number;
}

/** Sound from the sound library (across all projects). */
export interface SavedSound {
  id: string;
  name: string;
  kind: InstrumentKind;
  kindName: string;
  /** Position in the fields in words, e.g. "Trap · dunkel, rau · breit, fern" */
  summary: string;
  created: number;
  modified: number;
  instrument: Instrument;
}

export interface SoundSaveResult {
  id: string;
  name: string;
  /** The name already existed for the instrument – replaced. */
  replaced: boolean;
  sounds: SavedSound[];
}

/** Waveform of the test sound (per column minimum, maximum). */
export interface WaveView {
  secs: number;
  peaks: Array<[number, number]>;
  held: number;
  testton: string;
}

export interface InstrumentSpec {
  kind: InstrumentKind;
  name: string;
  description: string;
  params: ParamSpec[];
  presets: Array<{ name: string; params: Record<string, number> }>;
  previews: Array<{ key: string; label: string }>;
  keys: Array<{ midi: number; short: string; name: string }>;
  fields: FieldSpec[];
  verlauf: VerlaufSpec[];
}

export type TrackView = "compact" | "notes";

export interface Track {
  id: Id;
  name: string;
  kind: TrackKind;
  view: TrackView;
  /** Row height compact (0 = default). */
  compactHeight: number;
  /** Row height note view (0 = default). */
  notesHeight: number;
  color: string;
  gainDb: number;
  pan: number;
  mute: boolean;
  solo: boolean;
  pitchSemitones: number;
  instrument: Instrument | null;
  clips: Clip[];
}

export interface Project {
  name: string;
  tempoBpm: number;
  beatsPerBar: number;
  beatUnit: number;
  tracks: Track[];
  nextId: number;
}

export interface EngineInfo {
  available: boolean;
  error: string | null;
  outputDevice: string | null;
  /** Selected output/recording device (null = system default). */
  outputChoice: string | null;
  inputChoice: string | null;
  /** The remembered device was missing at start – the system default plays/records. */
  outputNotice: string | null;
  inputNotice: string | null;
  sampleRate: number;
  metronome: boolean;
  /** Repeat playback (from the start again at the end). */
  loopOn: boolean;
  /** Marked time range in beats: with `loopOn` the loop. */
  loopRange: [number, number] | null;
  /** End of the piece in beats (where the last note or audio clip ends): without a loop
   * and a range, playback stops there and leaves the playhead at that spot. */
  endBeats: number;
}

/** Audio devices of the system including the choice (null = system default). */
export interface AudioDevices {
  outputs: string[];
  inputs: string[];
  defaultOutput: string | null;
  defaultInput: string | null;
  output: string | null;
  input: string | null;
  /** Output device that is currently playing, and its sample rate. */
  outputDevice: string;
  sampleRate: number;
}

export type AudioKind = "output" | "input";

export interface AgentInfo {
  url: string | null;
  error: string | null;
  library: string | null;
  libraryVoices: number;
}

export interface Snapshot {
  /** Project file (null = not saved yet). */
  file: string | null;
  /** Unsaved changes. */
  dirty: boolean;
  project: Project;
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  engine: EngineInfo;
  agent: AgentInfo;
}

// ---------------------------------------------------------------- Change history

/** Key figures of the project after a step. */
export interface HistoryMetrics {
  notes: number;
  tracks: number;
  clips: number;
  audioClips: number;
  /** End of the last clip. */
  lengthBeats: number;
  lengthSecs: number;
  tempo: number;
  beatsPerBar: number;
  beatUnit: number;
  /** Estimated key ("A minor"), only with enough notes. */
  key: string | null;
  /** Lowest and highest note (MIDI). */
  pitchRange: [number, number] | null;
}

export interface HistoryRef {
  id: number;
  name: string;
}

/** A branch that leaves the line at a row. */
export interface HistoryFork {
  /** Its first step. */
  node: number;
  /** Its newest step. */
  tip: number;
  steps: number;
  /** The variant that continues there – null: steps abandoned after an undo. */
  variant: HistoryRef | null;
}

/** A step on the active variant's line. */
export interface HistoryRow {
  id: number;
  index: number;
  label: string;
  /** Where, in words: tracks, bars, notes, tempo … */
  detail: string;
  /** Unix ms (0: unknown). */
  time: number;
  metrics: HistoryMetrics;
  /** After the current step: can be redone. */
  future: boolean;
  current: boolean;
  checkpoints: HistoryRef[];
  forks: HistoryFork[];
}

export interface HistoryVariant {
  id: number;
  name: string;
  head: number;
  active: boolean;
  /** Step where it leaves the other variants. */
  fork: number;
  /** Its own steps since then. */
  steps: number;
  time: number;
  /** Label of its newest step. */
  last: string;
}

export interface HistoryCheckpoint {
  id: number;
  name: string;
  node: number;
  time: number;
  /** On the active variant's line. */
  onLine: boolean;
  /** Variant it lies on otherwise. */
  variant: string | null;
}

export interface HistoryView {
  rows: HistoryRow[];
  current: number;
  /** Active variant. */
  active: number;
  variants: HistoryVariant[];
  checkpoints: HistoryCheckpoint[];
  /** Steps in the whole tree (all variants, abandoned branches). */
  total: number;
  /** Steps kept at most. */
  limit: number;
}

/** A section of a track that variants changed (beats, on bar lines). */
export interface MergeSegment {
  start: number;
  end: number;
  /** Variants that changed it. */
  by: number[];
  /** Changed differently by several – has to be decided. */
  conflict: boolean;
}

/** One track in "Combine": what each variant changed. */
export interface MergeTrack {
  /** Key of the decisions ("12", or "12@3" for a new track whose ID another variant used too). */
  key: string;
  trackId: number;
  name: string;
  color: string;
  inOrigin: boolean;
  /** Variants that have the track. */
  presentIn: number[];
  /** Variants that changed the track itself (sound, mix, name, whether it exists). */
  settingsBy: number[];
  settingsConflict: boolean;
  /** What each of them changed, in words. */
  changes: { source: number; what: string }[];
  segments: MergeSegment[];
  /** Notes of each version for the lanes view: the origin (0) and the variants that changed the track. */
  lanes: { source: number; present: boolean; notes: [number, number, number][]; audio: [number, number][] }[];
}

/** Everything "Combine" shows. Sources: a variant's ID, 0 = the common origin. */
export interface MergeView {
  origin: { node: number; index: number; label: string; time: number; checkpoints: string[] };
  /** The variant the result builds on. */
  target: number;
  /** Taking part, the base first. */
  variants: HistoryRef[];
  beatsPerBar: number;
  lengthBeats: number;
  project: { by: number[]; conflict: boolean; options: { source: number; tempo: number; beatsPerBar: number; beatUnit: number }[] };
  tracks: MergeTrack[];
  /** Name of the combined variant. */
  name: string;
}

/** Choices in "Combine" (missing ones take the obvious default). */
export interface MergeChoices {
  project: number | null;
  tracks: Record<string, number>;
  segments: { key: string; start: number; source: number }[];
}

export interface LibraryInfo {
  /** Active library (null while it is being loaded at start). */
  id: string | null;
  name: string | null;
  voices: number;
  scores: number;
  building: [number, number] | null;
  download: DownloadProgress | null;
  message: string | null;
  /** Scores with features (0 = re-index for the tree view). */
  treeScores: number;
  /** Genre/composer are available for all contained PDMX packages. */
  hasMeta: boolean;
  /** Scores with chord progression [major, minor]; null = index without chord statistics. */
  chordScores: [number, number] | null;
  /** Files are currently being added. */
  adding: boolean;
  /** Currently being imported into the library (fraction 0…1). */
  merging: number | null;
  /** The library is being loaded (or set up) at start. */
  loading: boolean;
  /** Package that is currently being loaded or imported. */
  currentPack?: string | null;
  /** Waiting packages, in the order in which they come up. */
  queue?: QueuedDownload[];
  /** Browser: the package on the server and the own scores. */
  web?: WebLibInfo;
}

/** Browser: the two parts of the library (see crates/klang-app/src/weblib.rs). */
export interface WebLibInfo {
  /** "none" (not configured), "loading", "ready", "offline", "error" */
  remote: "none" | "loading" | "ready" | "offline" | "error";
  remoteTitle: string | null;
  remoteVersion: string | null;
  remoteScores: number;
  remoteUrl: string | null;
  remoteLicense: string | null;
  remoteSources: string[];
  ownScores: number;
  error: string | null;
}

/** An own score in the browser. */
export interface OwnScore {
  file: string;
  path: string;
  doc: number;
  title: string;
  composer: string;
  bytes: number;
}

/** Result of adding own scores. */
export interface AddResult {
  added: number;
  skipped: string[];
  message: string;
}

/** A requested package in the download queue. */
export interface QueuedDownload {
  id: string;
  name: string;
  /** Target library: the one that was active when it was requested */
  library: string;
  libraryName: string;
}

/** A library (exactly one index) in the management view. */
export interface LibEntry {
  id: string;
  name: string;
  scores: number;
  voices: number;
  /** Names of the contained packages */
  packs: string[];
  /** added folders and files */
  paths: string[];
  active: boolean;
  /** "default" (cannot be deleted) */
  standard: boolean;
  /** Size of the index file */
  bytes: number;
}

/** Project file in the project folder. */
export interface ProjectFile {
  name: string;
  path: string;
  /** Unix time in ms */
  modified: number;
  bytes: number;
}

/** What an autosave did: into the project's file, into the recovery copy (a
 *  project without a file, or kept apart for a reload), or nothing (no changes). */
export interface AutosaveResult {
  saved: "file" | "recovery" | null;
  /** The project's file afterwards (a recovery copy doesn't change it). */
  file: string | null;
  /** Changes not yet in the project's file (a recovery copy keeps the project unsaved). */
  dirty: boolean;
}

/** Level of the tree view. */
export interface Dimension {
  key: string;
  label: string;
  multi: boolean;
}

export interface TreeNode {
  /** Value as in the facets (German) – the path for the next query. */
  value: string;
  /** Value for display (translated). */
  label: string;
  count: number;
}

export interface TreeScore {
  doc: number;
  title: string;
  composer: string;
  facts: string;
}

export interface TreeLevel {
  dimension: string | null;
  nodes: TreeNode[];
  scores: TreeScore[];
  total: number;
  matching: number;
}

export interface DownloadProgress {
  name: string;
  /** "Scores" or "Categories" */
  phase: string;
  folder: string;
  files: number;
  target: number | null;
  bytes: number;
  totalBytes: number;
}

/** Downloadable music library. */
export interface CatalogEntry {
  id: string;
  name: string;
  description: string;
  scores: number;
  downloadMb: number;
  diskMb: number;
  license: string;
  sourceUrl: string;
  /** Collection (panel): pdmx, openscore, kernscores, dcml */
  group?: string;
  groupName?: string;
  groupDescription?: string;
  /** File format, e.g. MusicXML, **kern */
  format: string;
  /** comes with a chord analysis by experts */
  expertHarmony?: boolean;
  folder: string;
  /** The scores are already in the folder (import without download). */
  installed: boolean;
  /** Name of the active library if it already contains the package. */
  inLibrary: string | null;
}

export interface LibHit {
  doc: number;
  title: string;
  composer: string;
  part: string;
  voice: string;
  source: string;
  noteCount: number;
  score: number | null;
  noteIndex: number | null;
  queryLen: number;
  /** Collection and license of the score (library packages). */
  collection?: string;
  license?: string;
  /** Browser: "own" (own scores) or "remote" (the package on the server). */
  origin?: "own" | "remote";
}

/** Result of "Sing it": recognized notes and matches. */
export interface ListenResult {
  /** Notes in the format of the melody field ("A5:1 Fis5:0,5 …") */
  melody: string;
  notes: number;
  key: string | null;
  seconds: number;
  hits: LibHit[];
}

export interface VoiceView {
  part: string;
  label: string;
  role: string | null;
  /** [onset, duration, MIDI] in quarters */
  notes: Array<[number, number, number]>;
  isHit: boolean;
  highlight: [number, number] | null;
}

export interface ScoreView {
  doc: number;
  title: string;
  composer: string;
  source: string;
  keyName: string | null;
  tempoBpm: number | null;
  timeSignature: [number, number] | null;
  mood: string;
  durationQuarters: number;
  voices: VoiceView[];
  cadences: Array<[number, string]>;
  /** Highlighted location across all parts (quarters), e.g. a chord change. */
  range: [number, number] | null;
  collection?: string;
  license?: string;
  origin?: "own" | "remote";
}

/** Running preview of a score (quarters; tempo in quarters per minute). */
export interface PlayInfo {
  tempo: number;
  start: number;
  end: number;
  loopStart: number;
  looping: boolean;
}

// ---------------------------------------------------------------- Harmony

export type Quality = "major" | "minor" | "dim" | "aug" | "dom7" | "maj7" | "min7" | "halfDim7" | "dim7" | "sus4";

export interface Chord {
  /** Root as pitch class (0 = C). */
  root: number;
  quality: Quality;
  /** Bass note if not the root (inversion). */
  bass?: number;
}

export interface KeySpec {
  tonic: number;
  minor: boolean;
  confidence?: number;
}

export interface HarmonyKey extends KeySpec {
  name: string;
  source: "gewaehlt" | "geschaetzt" | "standard";
}

export interface ChordInfo {
  chord: Chord;
  /** Chord symbol with German note names ("G7", "fis°", "C/E"). */
  name: string;
  /** Scale degree ("V⁷", "ii⁶", "V⁷/vi"). */
  roman: string;
  /** Function according to Riemann ("D⁷", "Sp", "DD"). */
  function: string;
  role: string;
  diatonic: boolean;
}

export interface CircleCell extends ChordInfo {
  /** 0 = major outside, 1 = minor, 2 = diminished inside */
  ring: 0 | 1 | 2;
  pos: number;
}

/** Mood of a transition (color in the circle of fifths). */
export type Mood = "loesend" | "spannend" | "ueberraschend" | "hell" | "dunkel";

export interface CadenceTemplate {
  id: string;
  name: string;
  description: string;
  steps: ChordInfo[];
  /** Mood per transition (steps[i] → steps[i+1]). */
  moods: Mood[];
}

/** Multi-step path from the last chord to a closing. */
export interface CadencePath {
  goal: "ganzschluss" | "plagalschluss" | "halbschluss" | "trugschluss" | "phrygisch";
  label: string;
  steps: ChordInfo[];
  /** Mood per transition (into steps[i], the first from the current chord). */
  moods: Mood[];
  score: number;
  /** Longer variant of the same closing. */
  longer: boolean;
  /** Share of each step among the continuations in the library; null = no library. */
  shares: Array<number | null> | null;
}

export interface HarmonyView {
  key: HarmonyKey;
  circle: CircleCell[];
  cadences: CadenceTemplate[];
}

export type SuggestCategory = "schluss" | "spannung" | "weiter" | "farbe" | "ausweichen" | "haeufig";

export interface HarmonyExample {
  doc: number;
  title: string;
  composer: string;
  bar: number;
  /** Range in quarters */
  start: number;
  end: number;
}

export interface LibFollow {
  share: number;
  count: number;
  total: number;
  /** false = counted as a root chord */
  exact: boolean;
  examples: HarmonyExample[];
}

export interface ChordSuggestion extends ChordInfo {
  score: number;
  category: SuggestCategory;
  reason: string;
  toKey?: string;
  /** Mood of the transition from the last chord to here. */
  mood: Mood;
  library: LibFollow | null;
}

export interface SuggestResult {
  suggestions: ChordSuggestion[];
  library: {
    name: string;
    scores: number;
    /** 0 = all, 1 = Baroque, 2 = Bach */
    group: number;
    groups: Array<{ id: number; key: string; label: string; scores: number }>;
  } | null;
  hint: string | null;
  /** The hint suggests re-indexing the library (button "Re-index"). */
  reindex: boolean;
  /** Paths to the closing: the best per closing type, optionally followed by a longer variant. */
  paths: CadencePath[];
}

// ---------------------------------------------------------------- Fugue

/** Note of a voice (beats). */
export interface VNote {
  pitch: number;
  start: number;
  len: number;
}

export type TransformSpec =
  | { type: "umkehrung" | "krebs" | "krebsumkehrung" | "vergroesserung" | "verkleinerung" }
  | { type: "versetzung" | "chromatisch"; value: number };

export interface FugueAnswer {
  kind: "real" | "tonal";
  notes: VNote[];
  /** Indices of the notes changed relative to the real answer */
  mutated: number[];
  reason: string;
}

export interface Stretto {
  delay: number;
  semitones: number;
  label: string;
  errors: number;
  warnings: number;
  overlap: number;
  follower: VNote[];
}

export interface SubjectView {
  key: HarmonyKey;
  trackName: string;
  clipName: string;
  start: number;
  notes: VNote[];
  length: number;
  range: [number, number];
  answer: FugueAnswer;
  transforms: Array<{ label: string; transform: TransformSpec; notes: VNote[] }>;
  strettos: Stretto[];
  beatsPerBar: number;
}

export interface Countersubject {
  notes: VNote[];
  above: boolean;
  errors: number;
  warnings: number;
  invertible: boolean;
  invertedErrors: number;
  invertedWarnings: number;
}

export interface FugueSection {
  /** Kind (untranslated): exposition, zwischenspiel, durchfuehrung, engfuehrung, schlussdurchfuehrung, orgelpunkt, schlusskadenz. */
  kind: string;
  /** Name for display (translated). */
  name: string;
  start: number;
  end: number;
  key: string;
  description: string;
}

export interface FuguePlan {
  sections: FugueSection[];
  voices: Array<{ name: string; notes: VNote[] }>;
  length: number;
  answer: "real" | "tonal";
  stretto: string | null;
  countersubject: boolean;
}

export interface FugueOptions {
  voices: number;
  countersubject: boolean;
  stretto: boolean;
  picardy: boolean;
}

export interface CheckIssue {
  kind: string;
  severity: "fehler" | "warnung" | "hinweis";
  start: number;
  end: number;
  bar: number;
  trackIds: number[];
  message: string;
}

export interface CheckView {
  voices: Array<{ trackId: number; name: string }>;
  issues: CheckIssue[];
  errors: number;
  warnings: number;
  hints: number;
}

export interface PlacedChord extends ChordInfo {
  startBeats: number;
  lengthBeats: number;
}

/** Chord of the harmony track: with the mood of the transition and possibly a break. */
export interface LineChord extends PlacedChord {
  mood: Mood;
  /** harmonic break: "tonartfremd" or "Tritonus-Sprung" */
  bruch: string | null;
}

export type ExportFormat = "wav" | "flac" | "opus" | "stems" | "midi";

export interface ExportOptions {
  format: ExportFormat;
  name?: string;
  bitrate?: number;
  bits?: 16 | 24;
  range?: boolean;
}

export interface ExportInfo {
  dir: string;
  name: string;
  range: [number, number] | null;
  rangeLabel: string | null;
  seconds: number;
  hasNotes: boolean;
  bitrates: number[];
  defaultBitrate: number;
  existing: string[];
}

export interface ExportResult {
  format: ExportFormat;
  path: string;
  files: string[];
  bytes: number;
  seconds: number;
}

export interface HarmonyLine {
  key: HarmonyKey;
  chords: LineChord[];
}

export interface ChatMessage {
  id: number;
  from: "user" | "agent";
  text: string;
  /** Unix time in ms */
  time: number;
  status: "neu" | "inArbeit" | "erledigt" | "abgebrochen" | null;
  replyTo: number | null;
  /** Position in the project when sending (goes along to the agent) */
  kontext?: string;
  /** Client that picked up the task (`client` of chat_poll) */
  claimedBy?: string;
  /** The agent replied to the task – it's no longer offered again */
  confirmed?: boolean;
  /** The person asked to stop this task */
  cancelRequested?: boolean;
  /** Stop message: the tasks it stops */
  cancels?: number[];
  /** Offered again this often because nobody replied in time */
  redelivered?: number;
}

export interface ChatView {
  messages: ChatMessage[];
  agentListening: boolean;
  /** An agent is connected via MCP (active session), listening or working. */
  agentConnected: boolean;
  /** An agent is working: a tool call within the last minute or a task in progress. */
  agentWorking: boolean;
  /** Tasks in progress (picked up, not done yet). */
  workingOn: number[];
}

export interface AgentActivity {
  tool: string;
  summary: string;
}

export type PreviewPattern = string;

export interface SoundReport {
  instrument: string;
  params: Record<string, number>;
  anzeige: Array<{ label: string; wort: string; wert: number; detail: string }>;
  beschreibung: Record<string, string>;
  testton: string;
}

export interface EngineStatus {
  playing: boolean;
  /** A preview is currently sounding (library, chords …). */
  previewing?: boolean;
  recording: boolean;
  positionSecs: number;
  peakOut: number;
  peakIn: number;
  /** A recording with live analysis is running (`recordLive` has it) – not the library's sing-to-search. */
  liveAnalysis?: boolean;
}

/** How a detected note was struck / how long it sounds against the way to the next. */
export type Attack = "normal" | "accent" | "sforzato";
export type Articulation = "normal" | "staccato" | "legato";

export interface AnalyzedNote {
  startSecs: number;
  endSecs: number;
  midi: number;
  rawMidi: number;
  cents: number;
  /** 1–127 as it goes into the notes: dynamics relative to the recording (median note = mf 80), raised for accents. */
  velocity: number;
  /** Loudness of the note's body (dBFS). */
  levelDb: number;
  attack: Attack;
  articulation: Articulation;
}

export interface HitPitch {
  time: number;
  end: number;
  /** Root (fractional) or null = unpitched (hi-hat, shaker …). */
  midi: number | null;
  clarity: number;
  strength: number;
}

export interface KeyInfo {
  name: string;
  tonic: number;
  minor: boolean;
  confidence: number;
}

export interface Analysis {
  assetId: string;
  durationSecs: number;
  sampleRate: number;
  pitch: Array<[number, number | null]>;
  notes: AnalyzedNote[];
  /** Hits with a root (drums, bass); unpitched ones end up in the rhythm strip. */
  hits: HitPitch[];
  onsets: number[];
  tempoEstimate: number | null;
  key: KeyInfo | null;
}

/** Note found while recording. */
export interface LiveNote {
  startSecs: number;
  endSecs: number;
  midi: number;
  cents: number;
  velocity: number;
  attack: Attack;
  articulation: Articulation;
}

/** What the UI already has of the running live analysis. */
export interface LiveCursor {
  /** Number of the recording – another one starts from the beginning. */
  take: number;
  /** Curve points, settled notes and onsets already received. */
  curve: number;
  notes: number;
  onsets: number;
}

/** The running recording's analysis; the growing parts from the cursor on. */
export interface LiveView {
  take: number;
  seconds: number;
  /** Level of the newest samples: RMS in dBFS, peak 0…1. */
  levelDb: number;
  peak: number;
  /** What sounds right now (MIDI with fraction), its nearest note and the deviation – null when nothing clear sounds. */
  pitch: number | null;
  note: number | null;
  cents: number | null;
  curveFrom: number;
  curve: Array<[number, number | null]>;
  /** Notes that won't change any more, from number `notesFrom` on. */
  notesFrom: number;
  notes: LiveNote[];
  /** Notes of the running phrase (may still change) – always all of them. */
  pending: LiveNote[];
  onsetsFrom: number;
  onsets: number[];
  tempoEstimate: number | null;
  key: KeyInfo | null;
}

export interface RecordResult {
  snapshot: Snapshot;
  analysis: Analysis;
  trackId: Id;
  clipId: Id;
}

/** Inserted audio file (drag & drop). */
export interface ImportedAudio {
  trackId: Id;
  clipId: Id;
  name: string;
  /** Format in words, e.g. "WAV 24 Bit". */
  format: string;
  seconds: number;
  /** Channels of the file (Klangwelt averages to mono). */
  channels: number;
}

export interface ImportResult {
  snapshot: Snapshot;
  imported: ImportedAudio[];
  /** Files that could not be read (with reason). */
  errors: string[];
}

/** Convert a recording: recognized notes or recognized hits. */
export type ConvertTarget = "melodie" | "rhythmus";

export interface ConvertResult {
  snapshot: Snapshot;
  trackId: Id;
  /** Number of notes generated. */
  notes: number;
}

export interface QuantizeResult {
  snapshot: Snapshot;
  /** New indices of the aligned notes (for the selection). */
  selected: number[];
  /** False: the notes already lay on the grid – nothing changed (no undo step). */
  changed: boolean;
}

export interface ProjectPatch { name?: string; tempoBpm?: number; beatsPerBar?: number; beatUnit?: number }
export interface TrackPatch {
  name?: string; color?: string; gainDb?: number; pan?: number; mute?: boolean; solo?: boolean;
  pitchSemitones?: number; instrument?: Instrument; view?: TrackView; compactHeight?: number; notesHeight?: number;
}
export interface ClipPatch { name?: string; startBeats?: number; sourceBpm?: number; warp?: boolean }

export type Command =
  | { type: "setProject"; patch: ProjectPatch }
  | { type: "addTrack"; track: Partial<Track> & { id: Id; name: string; kind: TrackKind }; index?: number | null }
  | { type: "removeTrack"; trackId: Id }
  | { type: "moveTrack"; trackId: Id; index: number }
  | { type: "setTrack"; trackId: Id; patch: TrackPatch }
  | { type: "addClip"; trackId: Id; clip: Clip }
  | { type: "removeClip"; trackId: Id; clipId: Id }
  | { type: "setClip"; trackId: Id; clipId: Id; patch: ClipPatch }
  | { type: "setClipNotes"; trackId: Id; clipId: Id; notes: Note[]; lengthBeats?: number }
  | { type: "batch"; label: string; commands: Command[] };

export interface Selection {
  trackId: Id;
  clipId: Id | null;
}
