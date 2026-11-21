// Shared state of the UI. The truth about the project lives in the
// Rust core – only the most recently received snapshot lives here.

import { backend } from "./bridge.js";
import { effect, signal } from "./signals.js";
import type {
  Analysis, ChatView, Clip, Command, EngineStatus, Instrument, InstrumentSpec, Project, SavedSound, Selection, Snapshot, Track,
} from "./types.js";

/** Description of the instruments (sliders, presets, key mapping) – loaded from the core. */
export const instrumentSpecs = signal<InstrumentSpec[]>([]);

/** Sound library (own sounds across all projects); null = not loaded yet. */
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
 * The time range in the ruler was marked last (true) – then Delete removes
 * it from all tracks. Every new selection of clip, track or notes resets this.
 */
export const rangeFocus = signal(false);
effect(() => {
  selection.value;
  rangeFocus.value = false;
});
export const analysis = signal<Analysis | null>(null);
/** A recording with live analysis is running – the analysis window shows it. */
export const liveRecording = signal(false);
let liveHeld = false;
/** Follows the engine status – except while a recording is being finished
 *  ([`holdLive`]): the live view then stays until the finished analysis replaces it. */
export function followLive(s: EngineStatus): void {
  if (s.liveAnalysis || !liveHeld) liveRecording.value = !!s.liveAnalysis;
}
export function holdLive(on: boolean): void {
  liveHeld = on;
  if (!on) followLive(status.peek());
}
/** Windows in the docking system. */
export type PanelId = "spuren" | "clip" | "analyse" | "klang" | "rhythmus" | "song" | "historie" | "agent" | "bibliothek" | "harmonie" | "fuge" | "hilfe";
/** Request to show a window (each request is a new object, even for the same window). */
export const panelRequest = signal<{ id: PanelId; seq: number } | null>(null);
let panelSeq = 0;
export function showPanel(id: PanelId): void {
  panelRequest.value = { id, seq: ++panelSeq };
}
/** Window that temporarily gets more space (intro tour); null = normal sizes. */
export const panelEmphasis = signal<{ id: PanelId; minWidth?: number } | null>(null);
/** Windows that are currently visible (active tab of their group). */
export const visiblePanels = signal<ReadonlySet<string>>(new Set());
/** Chat with the agent. */
export const chat = signal<ChatView>({ messages: [], agentListening: false, agentConnected: false, agentWorking: false, workingOn: [] });
/** Number of unread agent replies (dot on the tab). */
export const chatUnread = signal(0);
/** Library is being downloaded or indexed (dot on the tab). */
export const libraryBusy = signal(false);
/** Spots that the counterpoint checker marks (beats, tracks involved). */
export const counterpointMarks = signal<Array<{ start: number; end: number; severity: "fehler" | "warnung" | "hinweis"; trackIds: number[]; message: string }>>([]);
export const viewport = signal({ pxPerBeat: 44, scrollX: 0, scrollY: 0 });
export type MessageKind = "info" | "error" | "agent";
export const message = signal<{ text: string; kind: MessageKind; id: number } | null>(null);
/** Counts project changes made by the agent (for brief highlights). */
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

/** Waits for a new snapshot and adopts it; errors are displayed. */
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
