// Spiegel der Rust-Typen (serde, camelCase).

export type Id = number;

export interface Note {
  pitch: number;
  startBeats: number;
  lengthBeats: number;
  /** Anschlag 1–127. */
  velocity: number;
  /** Vibrato-Tiefe in Halbtönen 0–1 (fehlt = 0). */
  vibrato?: number;
  /** Helligkeit −1…+1 (fehlt = 0). */
  brightness?: number;
  /** Anschleifen in Halbtönen −12…+12 (fehlt = 0). */
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

/** Kurvenpunkt: [Sekunden ab Tonbeginn, Wert 0…1]. */
export type CurvePoint = [number, number];

export interface Instrument {
  kind: InstrumentKind;
  preset: string;
  params: Record<string, number>;
  /** Gezeichnete Verläufe (Synth: „laut“, „hell“). */
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
  /** Wert ist ein MIDI-Ton */
  note: boolean;
  meaning: string;
  /** Kein eigener Regler (Feld-Stellung, Verlauf). */
  hidden?: boolean;
}

/** Feld im Klang-Fenster: zwei Achsen, Startpunkte, gesetzte Regler. */
export interface FieldSpec {
  key: "charakter" | "klangfarbe" | "raum";
  name: string;
  x: { param: string; label: string; words: [string, string, string, string] };
  y: { param: string; label: string; words: [string, string, string, string] };
  presets: Array<{ name: string; x: number; y: number }>;
  sets: string[];
}

/** Vorlage für den Verlauf des Synths. */
export interface VerlaufSpec {
  name: string;
  beschreibung: string;
  laut: CurvePoint[];
  hell: CurvePoint[];
  ausklang: number;
  puls: number;
  pulstiefe: number;
}

/** Song-Aufbau: Rolle einer Zeile. */
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
  /** Stufe je Rolle: 0 aus, 1 sparsam, 2 voll */
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

/** Rhythmus: Zeile des Schritt-Rasters (eine Trommel). */
export interface RhythmRow {
  key: string;
  label: string;
  midi: number;
  /** immer sichtbar (sonst unter „weitere Trommeln“) */
  main: boolean;
}

/** Schritt-Raster: Länge in Sechzehnteln, je Zeile die gesetzten Schritte (ab 0). */
export interface RhythmGrid {
  steps: number;
  rows: Record<string, number[]>;
}

export interface RhythmPattern {
  name: string;
  bpm: number;
  swing: number;
  tip: string;
  /** ein Takt */
  grid: RhythmGrid;
}

export interface RhythmGenre {
  name: string;
  blurb: string;
  /** Startpunkt im Charakter-Feld für eine neue Schlagzeug-Spur */
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
  /** Noten, die nicht im Raster erscheinen */
  skipped: number;
  name: string;
}

export interface RhythmWritten {
  snapshot: Snapshot;
  trackId: number;
  clipId: number;
}

/** Klang aus der Klang-Bibliothek (über alle Projekte). */
export interface SavedSound {
  id: string;
  name: string;
  kind: InstrumentKind;
  kindName: string;
  /** Stellung in den Feldern in Worten, z. B. „Trap · dunkel, rau · breit, fern“ */
  summary: string;
  created: number;
  modified: number;
  instrument: Instrument;
}

export interface SoundSaveResult {
  id: string;
  name: string;
  /** Den Namen gab es beim Instrument schon – ersetzt. */
  replaced: boolean;
  sounds: SavedSound[];
}

/** Wellenform des Testklangs (je Spalte Minimum, Maximum). */
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
  /** Zeilenhöhe kompakt (0 = Standard). */
  compactHeight: number;
  /** Zeilenhöhe Notenansicht (0 = Standard). */
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
  /** Gewähltes Ausgabe-/Aufnahmegerät (null = Systemstandard). */
  outputChoice: string | null;
  inputChoice: string | null;
  /** Gemerktes Gerät fehlte beim Start – es spielt/nimmt der Systemstandard. */
  outputNotice: string | null;
  inputNotice: string | null;
  sampleRate: number;
  metronome: boolean;
  /** Wiedergabe wiederholen (am Ende von vorn). */
  loopOn: boolean;
  /** Markierter Zeitbereich in Schlägen: mit `loopOn` die Schleife. */
  loopRange: [number, number] | null;
}

/** Audiogeräte des Systems samt Wahl (null = Systemstandard). */
export interface AudioDevices {
  outputs: string[];
  inputs: string[];
  defaultOutput: string | null;
  defaultInput: string | null;
  output: string | null;
  input: string | null;
  /** Ausgabegerät, das gerade spielt, und seine Abtastrate. */
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
  /** Projektdatei (null = noch nicht gespeichert). */
  file: string | null;
  /** Ungespeicherte Änderungen. */
  dirty: boolean;
  project: Project;
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  engine: EngineInfo;
  agent: AgentInfo;
}

export interface LibraryInfo {
  /** Aktive Bibliothek (null, solange sie beim Start geladen wird). */
  id: string | null;
  name: string | null;
  voices: number;
  scores: number;
  building: [number, number] | null;
  download: DownloadProgress | null;
  message: string | null;
  /** Partituren mit Merkmalen (0 = neu indizieren für die Baumansicht). */
  treeScores: number;
  /** Genre/Komponist liegen für alle enthaltenen PDMX-Pakete vor. */
  hasMeta: boolean;
  /** Partituren mit Akkordfolge [Dur, Moll]; null = Index ohne Akkordstatistik. */
  chordScores: [number, number] | null;
  /** Gerade werden Dateien hinzugefügt. */
  adding: boolean;
  /** Gerade wird in die Bibliothek übernommen (Anteil 0…1). */
  merging: number | null;
  /** Die Bibliothek wird beim Start geladen (bzw. eingerichtet). */
  loading: boolean;
  /** Paket, das gerade geladen bzw. übernommen wird. */
  currentPack?: string | null;
  /** Wartende Pakete, in der Reihenfolge, in der sie drankommen. */
  queue?: QueuedDownload[];
}

/** Ein beauftragtes Paket in der Download-Warteschlange. */
export interface QueuedDownload {
  id: string;
  name: string;
  /** Zielbibliothek: die beim Beauftragen aktive */
  library: string;
  libraryName: string;
}

/** Eine Bibliothek (genau ein Index) in der Verwaltung. */
export interface LibEntry {
  id: string;
  name: string;
  scores: number;
  voices: number;
  /** Namen der enthaltenen Pakete */
  packs: string[];
  /** hinzugefügte Ordner und Dateien */
  paths: string[];
  active: boolean;
  /** „Standard“ (lässt sich nicht löschen) */
  standard: boolean;
  /** Größe der Indexdatei */
  bytes: number;
}

/** Projektdatei im Projektordner. */
export interface ProjectFile {
  name: string;
  path: string;
  /** Unix-Zeit in ms */
  modified: number;
  bytes: number;
}

/** Ebene der Baumansicht. */
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
  /** „Partituren“ oder „Kategorien“ */
  phase: string;
  folder: string;
  files: number;
  target: number | null;
  bytes: number;
  totalBytes: number;
}

/** Herunterladbare Musikbibliothek. */
export interface CatalogEntry {
  id: string;
  name: string;
  description: string;
  scores: number;
  downloadMb: number;
  diskMb: number;
  license: string;
  sourceUrl: string;
  /** Sammlung (Panel): pdmx, openscore, kernscores, dcml */
  group?: string;
  groupName?: string;
  groupDescription?: string;
  /** Dateiformat, z. B. MusicXML, **kern */
  format: string;
  /** bringt eine Akkordanalyse von Fachleuten mit */
  expertHarmony?: boolean;
  folder: string;
  /** Die Partituren liegen schon im Ordner (Übernehmen ohne Download). */
  installed: boolean;
  /** Name der aktiven Bibliothek, wenn sie das Paket schon enthält. */
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
}

/** Ergebnis von „Vorsingen“: erkannte Töne und Treffer. */
export interface ListenResult {
  /** Töne im Format des Melodiefelds („A5:1 Fis5:0,5 …“) */
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
  /** [Einsatz, Dauer, MIDI] in Vierteln */
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
  /** Hervorgehobene Stelle über alle Stimmen (Viertel), z. B. ein Akkordwechsel. */
  range: [number, number] | null;
}

/** Laufendes Vorspielen einer Partitur (Viertel; Tempo in Vierteln je Minute). */
export interface PlayInfo {
  tempo: number;
  start: number;
  end: number;
  loopStart: number;
  looping: boolean;
}

// ---------------------------------------------------------------- Harmonik

export type Quality = "major" | "minor" | "dim" | "aug" | "dom7" | "maj7" | "min7" | "halfDim7" | "dim7" | "sus4";

export interface Chord {
  /** Grundton als Tonhöhenklasse (0 = C). */
  root: number;
  quality: Quality;
  /** Basston, wenn nicht der Grundton (Umkehrung). */
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
  /** Akkordsymbol mit deutschen Tonnamen („G7“, „fis°“, „C/E“). */
  name: string;
  /** Stufe („V⁷“, „ii⁶“, „V⁷/vi“). */
  roman: string;
  /** Funktion nach Riemann („D⁷“, „Sp“, „DD“). */
  function: string;
  role: string;
  diatonic: boolean;
}

export interface CircleCell extends ChordInfo {
  /** 0 = Dur außen, 1 = Moll, 2 = vermindert innen */
  ring: 0 | 1 | 2;
  pos: number;
}

/** Stimmung eines Übergangs (Farbe im Quintenzirkel). */
export type Mood = "loesend" | "spannend" | "ueberraschend" | "hell" | "dunkel";

export interface CadenceTemplate {
  id: string;
  name: string;
  description: string;
  steps: ChordInfo[];
  /** Stimmung je Übergang (steps[i] → steps[i+1]). */
  moods: Mood[];
}

/** Mehrschrittiger Weg vom letzten Akkord zu einem Schluss. */
export interface CadencePath {
  goal: "ganzschluss" | "plagalschluss" | "halbschluss" | "trugschluss" | "phrygisch";
  label: string;
  steps: ChordInfo[];
  /** Stimmung je Übergang (in steps[i] hinein, der erste vom aktuellen Akkord). */
  moods: Mood[];
  score: number;
  /** Längere Variante desselben Schlusses. */
  longer: boolean;
  /** Anteil jedes Schritts an den Fortsetzungen in der Bibliothek; null = keine Bibliothek. */
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
  /** Bereich in Vierteln */
  start: number;
  end: number;
}

export interface LibFollow {
  share: number;
  count: number;
  total: number;
  /** false = als Grundakkord gezählt */
  exact: boolean;
  examples: HarmonyExample[];
}

export interface ChordSuggestion extends ChordInfo {
  score: number;
  category: SuggestCategory;
  reason: string;
  toKey?: string;
  /** Stimmung des Übergangs vom letzten Akkord hierher. */
  mood: Mood;
  library: LibFollow | null;
}

export interface SuggestResult {
  suggestions: ChordSuggestion[];
  library: {
    name: string;
    scores: number;
    /** 0 = alle, 1 = Barock, 2 = Bach */
    group: number;
    groups: Array<{ id: number; key: string; label: string; scores: number }>;
  } | null;
  hint: string | null;
  /** The hint suggests re-indexing the library (button "Re-index"). */
  reindex: boolean;
  /** Wege zum Schluss: je Schlussart der beste, ggf. dahinter eine längere Variante. */
  paths: CadencePath[];
}

// ---------------------------------------------------------------- Fuge

/** Note einer Stimme (Schläge). */
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
  /** Indizes der gegenüber der realen Antwort veränderten Töne */
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

/** Akkord der Harmonie-Spur: mit Stimmung des Übergangs und ggf. Bruch. */
export interface LineChord extends PlacedChord {
  mood: Mood;
  /** harmonischer Bruch: „tonartfremd“ oder „Tritonus-Sprung“ */
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
  /** Unix-Zeit in ms */
  time: number;
  status: "neu" | "inArbeit" | "erledigt" | null;
  replyTo: number | null;
  /** Lage im Projekt beim Absenden (geht mit an den Agenten) */
  kontext?: string;
}

export interface ChatView {
  messages: ChatMessage[];
  agentListening: boolean;
  /** Ein Agent ist über MCP verbunden (aktive Sitzung) oder hört zu. */
  agentConnected: boolean;
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
  /** Ein Vorhören klingt gerade (Bibliothek, Akkorde …). */
  previewing?: boolean;
  recording: boolean;
  positionSecs: number;
  peakOut: number;
  peakIn: number;
}

export interface AnalyzedNote {
  startSecs: number;
  endSecs: number;
  midi: number;
  rawMidi: number;
  cents: number;
  velocity: number;
}

export interface HitPitch {
  time: number;
  end: number;
  /** Grundton (gebrochen) oder null = tonlos (Hi-Hat, Shaker …). */
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
  /** Anschläge mit Grundton (Schlagzeug, Bass); tonlose landen in der Rhythmus-Leiste. */
  hits: HitPitch[];
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

/** Eingefügte Audiodatei (Drag & Drop). */
export interface ImportedAudio {
  trackId: Id;
  clipId: Id;
  name: string;
  /** Format in Worten, z. B. „WAV 24 Bit“. */
  format: string;
  seconds: number;
  /** Kanäle der Datei (Klangwelt mittelt zu mono). */
  channels: number;
}

export interface ImportResult {
  snapshot: Snapshot;
  imported: ImportedAudio[];
  /** Dateien, die sich nicht lesen ließen (mit Grund). */
  errors: string[];
}

/** Aufnahme umwandeln: erkannte Töne oder erkannte Anschläge. */
export type ConvertTarget = "melodie" | "rhythmus";

export interface ConvertResult {
  snapshot: Snapshot;
  trackId: Id;
  /** Anzahl erzeugter Noten. */
  notes: number;
}

export interface QuantizeResult {
  snapshot: Snapshot;
  /** Neue Indizes der ausgerichteten Noten (für die Auswahl). */
  selected: number[];
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
