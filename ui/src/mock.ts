// Stand-in for the Rust core for the browser preview: same interface,
// simplified logic, no audio.

import type { Backend } from "./bridge.js";
import { setWebUiScale, webUiScale, webUiScaleMax, webUiScaleWorks } from "./web/viewport.js";
import { t } from "./i18n.js";
import type { ChatMessage, Command, EngineStatus, Project, Snapshot, Track } from "./types.js";

/** Collections in the preview's download catalog (texts shortened as in the app). */
const MOCK_GROUPS = {
  pdmx: { group: "pdmx", groupName: "PDMX – gemeinfreie MuseScore-Partituren", groupDescription: "Riesige, bunt gemischte Sammlung – gut für die Melodiesuche, die Qualität schwankt." },
  openscore: { group: "openscore", groupName: "OpenScore – sorgfältig gesetzte Partituren", groupDescription: "Von Freiwilligen nach Erstausgaben gesetzt und geprüft, gemeinfrei (CC0)." },
  kernscores: { group: "kernscores", groupName: "KernScores – Forschungsausgaben im Humdrum-Format", groupDescription: "Der akademische Standard des CCARH (Stanford), je ein Paket pro Epoche." },
  dcml: { group: "dcml", groupName: "DCML – mit Harmonieanalyse von Fachleuten", groupDescription: "Zu jedem Takt eine geprüfte Akkordanalyse (EPFL Lausanne)." },
};

const COLORS = ["#ffb454", "#6ee7d8", "#a78bfa", "#f472b6", "#60a5fa", "#4ade80", "#fb923c", "#facc15"];

function demoProject(): Project {
  const melody = [
    [69, 0, 1], [72, 1, 0.5], [71, 1.5, 0.5], [69, 2, 1], [64, 3, 1], [65, 4, 1], [69, 5, 1], [67, 6, 1.5],
    [72, 8, 1], [76, 9, 0.5], [74, 9.5, 0.5], [72, 10, 1], [67, 11, 1], [71, 12, 1], [74, 13, 0.5], [72, 13.5, 0.5], [71, 14, 1], [69, 15, 1],
  ].map(([pitch, startBeats, lengthBeats]) => ({ pitch: pitch!, startBeats: startBeats!, lengthBeats: lengthBeats!, velocity: 96 }));
  const base = { mute: false, solo: false, pitchSemitones: 0, instrument: null, view: "compact" as const, compactHeight: 0, notesHeight: 0 };
  return {
    name: "Demo (Browser-Vorschau)",
    tempoBpm: 110,
    beatsPerBar: 4,
    beatUnit: 4,
    nextId: 7,
    tracks: [
      { ...base, id: 1, name: "Beat", kind: "audio", color: COLORS[0]!, gainDb: -2, pan: 0,
        clips: [{ id: 2, name: "Beat · 96 BPM", startBeats: 0, source: { type: "audio", assetId: "demo-beat", sourceBpm: 96, warp: true, durationSecs: 10 } }] },
      { ...base, id: 3, name: "Arpeggio", kind: "audio", color: COLORS[2]!, gainDb: -6, pan: 0.35,
        clips: [{ id: 4, name: "Arpeggio · 128 BPM", startBeats: 0, source: { type: "audio", assetId: "demo-arp", sourceBpm: 128, warp: true, durationSecs: 7.5 } }] },
      { ...base, id: 5, name: "Melodie", kind: "instrument", color: COLORS[4]!, gainDb: -3, pan: -0.2, view: "notes",
        instrument: { kind: "pluck", preset: "warm", params: { helligkeit: 0.4, abklingen: 0.7, koerper: 0.75, anschlag: 0.35 } },
        clips: [{ id: 6, name: "Thema", startBeats: 0, source: { type: "notes", lengthBeats: 16, notes: melody } }] },
    ],
  };
}

export function createMock(): Backend {
  let project: Project = { name: "Neues Projekt", tempoBpm: 120, beatsPerBar: 4, beatUnit: 4, tracks: [], nextId: 1 };
  const undo: Project[] = [];
  const redo: Project[] = [];
  let lastMergeKey: string | null = null;
  const chatMessages: ChatMessage[] = [];
  let metronome = true;
  let loopOn = false;
  let loopRange: [number, number] | null = null;
  let playing = false;
  let pos = 0;
  let last = performance.now();
  // Sample devices so the device selection can be used in the preview
  const MOCK_OUTPUTS = ["Lautsprecher (Realtek(R) Audio)", "Kopfhörer (USB Audio)", "Focusrite USB (Scarlett 2i2)"];
  const MOCK_INPUTS = ["Mikrofon (Realtek(R) Audio)", "Headset-Mikrofon (USB Audio)", "Focusrite USB (Scarlett 2i2)"];
  let outputChoice: string | null = null;
  let inputChoice: string | null = null;

  const snap = (): Snapshot => ({
    file: null,
    dirty: undo.length > 0,
    project: structuredClone(project),
    canUndo: undo.length > 0,
    canRedo: redo.length > 0,
    undoLabel: undo.length ? t("Last change") : null,
    redoLabel: redo.length ? t("Last change") : null,
    engine: {
      available: false, error: t("Browser preview – no audio"), outputDevice: null, outputChoice, inputChoice, outputNotice: null, inputNotice: null,
      sampleRate: 48000, metronome, loopOn, loopRange,
      endBeats: Math.max(0, ...project.tracks.flatMap((tr) => tr.clips.map((c) =>
        c.startBeats + (c.source.type === "notes" ? Math.max(0, ...c.source.notes.map((n) => n.startBeats + n.lengthBeats)) : 0)))),
    },
    agent: { url: null, error: t("desktop app only"), library: null, libraryVoices: 0 },
  });

  const track = (p: Project, id: number): Track => {
    const found = p.tracks.find((x) => x.id === id);
    if (!found) throw new Error(t("Track {id} not found", { id }));
    return found;
  };

  function apply(p: Project, c: Command): void {
    switch (c.type) {
      case "setProject": Object.assign(p, c.patch); break;
      case "setTrack": Object.assign(track(p, c.trackId), c.patch); break;
      case "removeTrack": p.tracks = p.tracks.filter((t) => t.id !== c.trackId); break;
      case "moveTrack": {
        const t = track(p, c.trackId);
        p.tracks = p.tracks.filter((x) => x !== t);
        p.tracks.splice(c.index, 0, t);
        break;
      }
      case "addTrack": {
        const id = c.track.id || p.nextId++;
        const t: Track = {
          color: COLORS[(id - 1) % COLORS.length]!, gainDb: 0, pan: 0, mute: false, solo: false, pitchSemitones: 0,
          instrument: c.track.kind === "instrument" ? { kind: "pluck", preset: "zupfer", params: { helligkeit: 0.55, abklingen: 0.5, koerper: 0.4, anschlag: 0.5 } } : null,
          view: c.track.kind === "instrument" ? "notes" : "compact",
          clips: [], ...c.track, id,
          compactHeight: c.track.compactHeight ?? 0,
          notesHeight: c.track.notesHeight ?? 0,
        };
        p.tracks.splice(c.index ?? p.tracks.length, 0, t);
        break;
      }
      case "addClip": track(p, c.trackId).clips.push({ ...c.clip, id: c.clip.id || p.nextId++ }); break;
      case "removeClip": { const t = track(p, c.trackId); t.clips = t.clips.filter((x) => x.id !== c.clipId); break; }
      case "setClip": {
        const clip = track(p, c.trackId).clips.find((x) => x.id === c.clipId);
        if (!clip) throw new Error(t("Clip not found"));
        if (c.patch.name !== undefined) clip.name = c.patch.name;
        if (c.patch.startBeats !== undefined) clip.startBeats = c.patch.startBeats;
        if (clip.source.type === "audio") {
          if (c.patch.sourceBpm !== undefined) clip.source.sourceBpm = c.patch.sourceBpm;
          if (c.patch.warp !== undefined) clip.source.warp = c.patch.warp;
        }
        break;
      }
      case "setClipNotes": {
        const clip = track(p, c.trackId).clips.find((x) => x.id === c.clipId);
        if (!clip || clip.source.type !== "notes") throw new Error(t("Only instrument clips contain notes"));
        clip.source.notes = [...c.notes].sort((a, b) => a.startBeats - b.startBeats || a.pitch - b.pitch);
        if (c.lengthBeats !== undefined) clip.source.lengthBeats = c.lengthBeats;
        break;
      }
      case "batch": c.commands.forEach((x) => apply(p, x)); break;
    }
  }

  const later = <T>(v: T) => Promise.resolve(v);
  const noAudio = () => Promise.reject(t("Recording and analysis are only available in the desktop app."));

  return {
    native: false,
    platform: "mock",
    setLanguage: () => later(undefined),
    getState: () => later(snap()),
    execute: (c, merge = false) => {
      const display = ["view", "compactHeight", "notesHeight"];
      if (c.type === "setTrack" && Object.keys(c.patch).length && Object.keys(c.patch).every((k) => display.includes(k))) {
        apply(project, c); // View/height: no undo step
        return later(snap());
      }
      const key = c.type === "setTrack" || c.type === "setClip" || c.type === "setProject" ? JSON.stringify([c.type, Object.keys(c.patch), "trackId" in c ? c.trackId : 0]) : null;
      const next = structuredClone(project);
      try {
        apply(next, c);
      } catch (e) {
        return Promise.reject(String(e));
      }
      if (!(merge && key && key === lastMergeKey)) undo.push(project);
      lastMergeKey = key;
      redo.length = 0;
      project = next;
      return later(snap());
    },
    undo: () => { const p = undo.pop(); if (p) { redo.push(project); project = p; } lastMergeKey = null; return later(snap()); },
    redo: () => { const p = redo.pop(); if (p) { undo.push(project); project = p; } lastMergeKey = null; return later(snap()); },
    // change history: the stand-in keeps none – one row with the current state
    historyView: () => {
      const notes = project.tracks.flatMap((tr) => tr.clips).reduce((n, c) => n + (c.source.type === "notes" ? c.source.notes.length : 0), 0);
      const metrics = {
        notes, tracks: project.tracks.length, clips: project.tracks.reduce((n, tr) => n + tr.clips.length, 0), audioClips: 0,
        lengthBeats: 0, lengthSecs: 0, tempo: project.tempoBpm, beatsPerBar: project.beatsPerBar, beatUnit: project.beatUnit, key: null, pitchRange: null,
      };
      return later({
        rows: [{ id: 1, index: 0, label: t("Start"), detail: "", time: Date.now(), metrics, future: false, current: true, checkpoints: [], forks: [] }],
        current: 1, active: 1, checkpoints: [], total: 1, limit: 1000,
        variants: [{ id: 1, name: t("Main version"), head: 1, active: true, fork: 1, steps: 0, time: 0, last: t("Start") }],
      });
    },
    historyGoto: () => later(snap()),
    historyCheckpoint: () => Promise.reject(t("Only available in the desktop app")),
    historyCheckpointRename: () => Promise.reject(t("Only available in the desktop app")),
    historyCheckpointDelete: () => Promise.reject(t("Only available in the desktop app")),
    historyVariantNew: () => Promise.reject(t("Only available in the desktop app")),
    historyVariantSwitch: () => later(snap()),
    historyVariantRename: () => Promise.reject(t("Only available in the desktop app")),
    historyVariantDelete: () => later(snap()),
    historyClear: () => later(snap()),
    historyMergePlan: () => Promise.reject(t("Only available in the desktop app")),
    historyMergePreview: () => Promise.reject(t("Only available in the desktop app")),
    historyMergePreviewStop: () => later(null),
    historyMergeApply: () => Promise.reject(t("Only available in the desktop app")),
    transport: (action) => {
      if (action === "play") playing = true;
      else if (action === "stop") playing = false;
      else if (action === "toggle") playing = !playing;
      else pos = 0;
      last = performance.now();
      return later(undefined);
    },
    seek: (s) => { pos = s; return later(undefined); },
    setMetronome: (on) => { metronome = on; return later(snap()); },
    setLoop: (on) => { loopOn = on; return later(snap()); },
    setLoopRange: (r) => { loopRange = r; return later(snap()); },
    audioDevices: () => later({
      outputs: [...MOCK_OUTPUTS], inputs: [...MOCK_INPUTS], defaultOutput: MOCK_OUTPUTS[0]!, defaultInput: MOCK_INPUTS[0]!,
      output: outputChoice, input: inputChoice, outputDevice: outputChoice ?? MOCK_OUTPUTS[0]!, sampleRate: 48000,
    }),
    setAudioDevice: (kind, name) => {
      const list = kind === "output" ? MOCK_OUTPUTS : MOCK_INPUTS;
      if (name && !list.includes(name)) return Promise.reject(t("Device \"{name}\" not found – is it connected?", { name }));
      if (kind === "output") outputChoice = name;
      else inputChoice = name;
      return later(snap());
    },
    recordStart: noAudio,
    recordStop: noAudio,
    recordLive: () => later(null),
    analyzeAsset: noAudio,
    createMelodyTrack: noAudio,
    audioImport: noAudio,
    audioToNotes: noAudio,
    deleteRange: noAudio,
    quantizeNotes: noAudio,
    smoothNotes: noAudio,
    analyzeVoices: noAudio,
    recordLiveVoices: async () => null,
    getWaveform: (assetId, buckets) => {
      let seed = [...assetId].reduce((a, ch) => a * 31 + ch.charCodeAt(0), 7) >>> 0;
      const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
      const out: number[] = [];
      for (let i = 0; i < buckets; i++) {
        const beat = assetId.includes("beat") ? Math.exp(-((i * 16) % buckets / buckets) * 9) : 0.35 + 0.25 * Math.sin(i / 9);
        const a = Math.min(1, beat * (0.6 + 0.4 * rnd()));
        out.push(-a, a);
      }
      return later(out);
    },
    createDemo: () => {
      undo.push(project);
      redo.length = 0;
      project = demoProject();
      return later(snap());
    },
    previewTrack: noAudio,
    previewNote: () => later(undefined),
    describeSound: (trackId) => {
      const p = project.tracks.find((t) => t.id === trackId)?.instrument?.params ?? {};
      const word = (x: number) => (x < 0.33 ? "niedrig" : x < 0.66 ? "mittel" : "hoch");
      const anzeige = Object.entries(p).slice(0, 4).map(([k, v]) => ({ label: k, wort: word(v), wert: v, detail: "Attrappe" }));
      return later({
        instrument: "Attrappe",
        params: { ...p },
        anzeige,
        beschreibung: Object.fromEntries(anzeige.map((m) => [m.label, m.wort])),
        testton: t("Browser preview – no measurement"),
      });
    },
    libraryInfo: () => later({ id: null, name: null, voices: 0, scores: 0, building: null, download: null, message: t("Library is only available in the desktop app"), treeScores: 0, hasMeta: true, chordScores: null, adding: false, merging: null, loading: false }),
    libraryAdd: noAudio,
    libraryOwnList: async () => [],
    libraryOwnRemove: async () => [],
    libraryCacheInfo: async () => ({ entries: 0, bytes: 0 }),
    libraryCacheClear: async () => {},
    importFiles: async () => [],
    libraryList: async () => [],
    libraryCreate: noAudio,
    librarySwitch: noAudio,
    libraryRename: noAudio,
    libraryDelete: noAudio,
    // Preview: show the catalog, download only in the desktop app.
    libraryCatalog: async () => [
      { id: "pdmx-kostprobe", name: "PDMX – Kostprobe", description: "3 000 gemischte Partituren zum Ausprobieren.", scores: 3000, downloadMb: 21, diskMb: 30, license: "gemeinfrei/CC0 · Datensatz CC-BY 4.0", sourceUrl: "https://zenodo.org/records/15571083", folder: "Musik/Klangwelt/Bibliothek/PDMX-Kostprobe", installed: false, inLibrary: null, ...MOCK_GROUPS.pdmx, format: "MusicXML" },
      { id: "pdmx-auswahl", name: "PDMX – Auswahl", description: "30 000 Partituren.", scores: 30000, downloadMb: 220, diskMb: 380, license: "gemeinfrei/CC0 · Datensatz CC-BY 4.0", sourceUrl: "https://zenodo.org/records/15571083", folder: "Musik/Klangwelt/Bibliothek/PDMX-Auswahl", installed: false, inLibrary: null, ...MOCK_GROUPS.pdmx, format: "MusicXML" },
      { id: "pdmx-voll", name: "PDMX – vollständig", description: "Rund 254 000 Partituren.", scores: 254000, downloadMb: 1895, diskMb: 3500, license: "gemeinfrei/CC0 · Datensatz CC-BY 4.0", sourceUrl: "https://zenodo.org/records/15571083", folder: "Musik/Klangwelt/Bibliothek/PDMX", installed: false, inLibrary: null, ...MOCK_GROUPS.pdmx, format: "MusicXML" },
      { id: "openscore-lieder", name: "OpenScore – Lieder", description: "Rund 1 460 Kunstlieder für Singstimme und Klavier.", scores: 1460, downloadMb: 19, diskMb: 30, license: "CC0 1.0 (gemeinfrei) · OpenScore", sourceUrl: "https://github.com/OpenScore/Lieder", folder: "Musik/Klangwelt/Bibliothek/OpenScore-Lieder", installed: false, inLibrary: null, ...MOCK_GROUPS.openscore, format: "MusicXML" },
      { id: "kern-barock", name: "Barock", description: "Bach: alle 370 Choräle, Wohltemperiertes Klavier, Kunst der Fuge, Inventionen · Scarlatti · Vivaldi.", scores: 605, downloadMb: 7, diskMb: 12, license: "KernScores/CCARH: frei für Forschung, Lehre und private Nutzung", sourceUrl: "https://github.com/craigsapp", folder: "Musik/Klangwelt/Bibliothek/KernScores-Barock", installed: true, inLibrary: null, ...MOCK_GROUPS.kernscores, format: "**kern" },
      { id: "kern-klassik", name: "Klassik", description: "Mozart- und Beethoven-Klaviersonaten, Beethoven-Streichquartette, Haydn-Klaviersonaten.", scores: 268, downloadMb: 9, diskMb: 15, license: "KernScores/CCARH: frei für Forschung, Lehre und private Nutzung", sourceUrl: "https://github.com/craigsapp", folder: "Musik/Klangwelt/Bibliothek/KernScores-Klassik", installed: false, inLibrary: null, ...MOCK_GROUPS.kernscores, format: "**kern" },
      { id: "kern-romantik", name: "Romantik", description: "Chopin: Mazurken und Préludes · Hummel: Präludien · Schubert: Lieder · Stephen Foster: Songs · Erks Deutscher Liederschatz.", scores: 405, downloadMb: 6, diskMb: 10, license: "KernScores/CCARH: frei für Forschung, Lehre und private Nutzung", sourceUrl: "https://github.com/craigsapp/chopin-mazurkas", folder: "Musik/Klangwelt/Bibliothek/KernScores-Romantik", installed: false, inLibrary: null, ...MOCK_GROUPS.kernscores, format: "**kern" },
      { id: "dcml-klassik", name: "Klassik, analysiert", description: "Mozart und Beethoven (Klaviersonaten, Streichquartette), C. P. E. Bach, J. C. Bach, Koželuch, Pleyel – mit Akkordanalyse für jeden Takt.", scores: 365, downloadMb: 62, diskMb: 85, license: "CC BY-NC-SA 4.0 · DCML, EPFL Lausanne", sourceUrl: "https://github.com/DCMLab/dcml_corpora", folder: "Musik/Klangwelt/Bibliothek/DCML-Klassik", installed: false, inLibrary: null, ...MOCK_GROUPS.dcml, format: "DCML-Tabellen", expertHarmony: true },
    ],
    libraryDownload: noAudio,
    libraryDequeue: noAudio,
    libraryCancel: async () => {},
    libraryEnrich: noAudio,
    libraryDimensions: async () => [
      { key: "genre", label: "Genre", multi: true }, { key: "komponist", label: "Komponist", multi: false },
      { key: "stimmung", label: "Stimmung", multi: false }, { key: "besetzung", label: "Besetzung", multi: false },
      { key: "tonart", label: "Tonart", multi: false }, { key: "instrumente", label: "Instrumentengruppe", multi: true },
    ],
    libraryTree: noAudio,
    librarySearch: noAudio,
    libraryOpen: noAudio,
    libraryToTrack: noAudio,
    libraryPlay: noAudio,
    previewStop: () => later(undefined),
    libraryReindex: noAudio,
    libraryListenStart: noAudio,
    libraryListenStop: noAudio,
    // Harmony is computed in the Rust core – the preview shows only an empty circle.
    harmonyView: () => later({ key: { tonic: 0, minor: false, name: "C-Dur", source: "standard" as const }, circle: [], cadences: [] }),
    harmonySuggest: () => later({ suggestions: [], library: null, hint: t("Harmony analysis is only available in the desktop app"), reindex: false, paths: [] }),
    harmonyDescribe: () => later([]),
    harmonyChords: () => later([]),
    harmonyPreview: noAudio,
    harmonyWrite: noAudio,
    harmonyTimeline: () => later({ key: { tonic: 0, minor: false, name: "C-Dur", source: "standard" as const }, chords: [] }),
    harmonyPlayExample: noAudio,
    fugueSubject: noAudio,
    fugueTransform: noAudio,
    fugueCountersubjects: noAudio,
    fuguePlan: noAudio,
    fugueAddVoices: noAudio,
    fuguePreview: noAudio,
    counterpointCheck: () => later({ voices: [], issues: [], errors: 0, warnings: 0, hints: 0 }),
    onLibraryChanged: () => {},
    onLibraryWait: () => {},
    onFileDrop: () => {},
    onFileDrag: () => {},
    chatSend: (text) => {
      chatMessages.push({ id: chatMessages.length + 1, from: "user", text, time: Date.now(), status: "neu", replyTo: null });
      return later({ messages: [...chatMessages], agentListening: false, agentConnected: false, agentWorking: false, workingOn: [] });
    },
    chatHistory: () => later({ messages: [...chatMessages], agentListening: false, agentConnected: false, agentWorking: false, workingOn: [] }),
    chatStop: (id, text) => {
      chatMessages.push({ id: chatMessages.length + 1, from: "user", text, time: Date.now(), status: "neu", replyTo: null, cancels: [id] });
      return later({ messages: [...chatMessages], agentListening: false, agentConnected: false, agentWorking: false, workingOn: [] });
    },
    onChatChanged: () => {},
    instrumentSpecs: () => Promise.reject(t("Instruments are only available in the desktop app.")),
    soundWave: () => Promise.reject(t("Sound editing is only available in the desktop app.")),
    soundField: noAudio,
    songTemplates: () => Promise.reject(t("Song templates are only available in the desktop app.")),
    songWrite: noAudio,
    rhythmInfo: () => Promise.reject(t("Rhythm is only available in the desktop app.")),
    rhythmRead: noAudio,
    rhythmToggle: noAudio,
    rhythmSwing: noAudio,
    rhythmWrite: noAudio,
    rhythmPreview: noAudio,
    soundsList: async () => [],
    soundSave: noAudio,
    soundLoad: noAudio,
    soundRename: noAudio,
    soundDelete: noAudio,
    onSoundsChanged: () => {},
    createInstrumentTrack: noAudio,
    insertPattern: noAudio,
    onProjectChanged: () => {},
    projectSave: () => Promise.reject(t("Saving is only available in the desktop app.")),
    projectOpen: () => Promise.reject(t("Opening files is only available in the desktop app.")),
    projectPick: () => Promise.reject(t("Opening files is only available in the desktop app.")),
    projectAutosave: async () => ({ saved: null, file: null, dirty: undo.length > 0 }),
    projectRecover: async () => null,
    syncStorage: async () => {},
    projectNew: noAudio,
    projectList: async () => ["Dokumente/Klangwelt/Projekte", []],
    exportInfo: async () => ({ dir: "Musik/Klangwelt/Exporte", name: "Export", range: null, rangeLabel: null, seconds: 0, hasNotes: false, bitrates: [64, 96, 128, 160, 192, 256, 320], defaultBitrate: 160, existing: [] }),
    exportRun: async () => { throw new Error(t("Export is only available in the app.")); },
    exportReveal: async () => {},
    appClose: async () => {},
    uiScale: async () => webUiScale(),
    setUiScale: (scale: number) => setWebUiScale(scale),
    uiScaleWorks: () => webUiScaleWorks(),
    uiScaleMax: () => webUiScaleMax(),
    openUrl: async (url: string) => {
      window.open(url, "_blank", "noopener,noreferrer");
    },
    onCloseRequested: () => {},
    onAgentActivity: () => {},
    onOpenFile: () => {},
    onStatus: (cb: (s: EngineStatus) => void) => {
      setInterval(() => {
        const now = performance.now();
        if (playing) pos += (now - last) / 1000;
        last = now;
        cb({ playing, recording: false, positionSecs: pos, peakOut: playing ? 0.3 + 0.3 * Math.random() : 0, peakIn: 0 });
      }, 33);
    },
  };
}
