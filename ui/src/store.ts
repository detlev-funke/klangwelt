// Gemeinsamer Zustand der Oberfläche. Die Wahrheit über das Projekt liegt im
// Rust-Kern – hier liegt nur der zuletzt empfangene Schnappschuss.

import { backend } from "./bridge.js";
import { effect, signal } from "./signals.js";
import type {
  Analysis, ChatView, Clip, Command, EngineStatus, Instrument, InstrumentSpec, Project, SavedSound, Selection, Snapshot, Track,
} from "./types.js";

/** Beschreibung der Instrumente (Regler, Presets, Tastenbelegung) – vom Kern geladen. */
export const instrumentSpecs = signal<InstrumentSpec[]>([]);

/** Klang-Bibliothek (eigene Klänge über alle Projekte); null = noch nicht geladen. */
export const savedSounds = signal<SavedSound[] | null>(null);

export function specFor(inst: Instrument | null | undefined): InstrumentSpec | undefined {
  return instrumentSpecs.peek().find((s) => s.kind === (inst?.kind ?? "pluck"));
}

export const RULER_H = 32;
export const ROW_H = 84;

export const snapshot = signal<Snapshot | null>(null);
export const status = signal<EngineStatus>({ playing: false, recording: false, positionSecs: 0, peakOut: 0, peakIn: 0 });
export const selection = signal<Selection | null>(null);
/**
 * Zuletzt markiert wurde der Zeitbereich im Lineal (true) – dann löscht Entf
 * ihn aus allen Spuren. Jede neue Auswahl von Clip, Spur oder Noten setzt das zurück.
 */
export const rangeFocus = signal(false);
effect(() => {
  selection.value;
  rangeFocus.value = false;
});
export const analysis = signal<Analysis | null>(null);
/** Fenster im Docksystem. */
export type PanelId = "spuren" | "clip" | "analyse" | "klang" | "rhythmus" | "song" | "agent" | "bibliothek" | "harmonie" | "fuge" | "hilfe";
/** Anfrage, ein Fenster zu zeigen (jede Anfrage ist ein neues Objekt, auch für dasselbe Fenster). */
export const panelRequest = signal<{ id: PanelId; seq: number } | null>(null);
let panelSeq = 0;
export function showPanel(id: PanelId): void {
  panelRequest.value = { id, seq: ++panelSeq };
}
/** Fenster, das vorübergehend mehr Platz bekommt (Intro-Tour); null = normale Größen. */
export const panelEmphasis = signal<{ id: PanelId; minWidth?: number } | null>(null);
/** Fenster, die gerade zu sehen sind (aktiver Reiter ihrer Gruppe). */
export const visiblePanels = signal<ReadonlySet<string>>(new Set());
/** Chat mit dem Agenten. */
export const chat = signal<ChatView>({ messages: [], agentListening: false, agentConnected: false });
/** Anzahl ungelesener Agent-Antworten (Punkt am Tab). */
export const chatUnread = signal(0);
/** Bibliothek wird heruntergeladen oder indiziert (Punkt am Reiter). */
export const libraryBusy = signal(false);
/** Stellen, die der Kontrapunkt-Prüfer markiert (Schläge, beteiligte Spuren). */
export const counterpointMarks = signal<Array<{ start: number; end: number; severity: "fehler" | "warnung" | "hinweis"; trackIds: number[]; message: string }>>([]);
export const viewport = signal({ pxPerBeat: 44, scrollX: 0, scrollY: 0 });
export type MessageKind = "info" | "error" | "agent";
export const message = signal<{ text: string; kind: MessageKind; id: number } | null>(null);
/** Zählt Projektänderungen durch den Agenten (für kurze Hervorhebungen). */
export const agentPulse = signal(0);

let msgId = 0;
let msgTimer: number | undefined;
export function notify(text: string, kind: MessageKind = "info"): void {
  message.value = { text, kind, id: ++msgId };
  clearTimeout(msgTimer);
  msgTimer = window.setTimeout(() => (message.value = null), kind === "error" ? 6000 : 3500);
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}

/** Wartet auf einen neuen Schnappschuss und übernimmt ihn; Fehler werden angezeigt. */
export async function act(p: Promise<Snapshot>): Promise<boolean> {
  try {
    snapshot.value = await p;
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

export function run(cmd: Command, merge = false): Promise<boolean> {
  return act(backend.execute(cmd, merge));
}

export function project(): Project | null {
  return snapshot.value?.project ?? null;
}

export function clipLengthBeats(clip: Clip, bpm: number): number {
  const s = clip.source;
  return s.type === "audio" ? (s.durationSecs * (s.warp ? s.sourceBpm : bpm)) / 60 : s.lengthBeats;
}

export function findTrack(p: Project | null, id: number | undefined): Track | undefined {
  return p?.tracks.find((t) => t.id === id);
}

export function findClip(p: Project | null, sel: Selection | null): { track: Track; clip: Clip } | null {
  if (!p || !sel || sel.clipId == null) return null;
  const track = findTrack(p, sel.trackId);
  const clip = track?.clips.find((c) => c.id === sel.clipId);
  return track && clip ? { track, clip } : null;
}

export function secondsToBeats(secs: number, bpm: number): number {
  return (secs * bpm) / 60;
}
