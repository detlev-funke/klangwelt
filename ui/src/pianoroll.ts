// Gemeinsame Geometrie und Zustand der Notenansicht (Spurköpfe + Zeitleiste).

import { backend } from "./bridge.js";
import { type ExprDef, clampExpr, exprShown } from "./expression.js";
import { signal } from "./signals.js";
import { ROW_H } from "./store.js";
import type { Analysis, Note, Track } from "./types.js";

export const NOTES_ROW_H = 236;
/** Höhe der Ausdrucksspur unter der Rhythmus-Leiste (Instrumentspuren). */
export const EXPR_H = 48;
/** Band oben in der Notenzeile für Clipnamen (dort lassen sich Clips verschieben). */
export const NOTES_TOP = 20;
/** Höhe der Rhythmus-Leiste unten in der Notenzeile (tonlose Schläge wie Hi-Hats). */
export const RHYTHM_H = 18;
/** Sichtbare Halbtöne: Standard, Grenzen für den Zoom. */
export const DEFAULT_SPAN = 21;
const MIN_SPAN = 12;
const MAX_SPAN = 60;

// ---------- Zeilenhöhen (einzeln ziehbar) ----------

/** Höhe während des Ziehens am Splitter (Vorschau, noch nicht gespeichert). */
export const heightDraft = signal<{ trackId: number; height: number } | null>(null);

export function defaultHeight(t: Track): number {
  return t.view === "notes" ? NOTES_ROW_H + (exprShown(t) ? EXPR_H : 0) : ROW_H;
}

/** Erlaubte Höhen je Ansicht (Pixel). */
export function heightLimits(t: Track): [number, number] {
  return t.view === "notes" ? [140, 1200] : [48, 600];
}

export function rowHeight(t: Track): number {
  const d = heightDraft.peek();
  if (d && d.trackId === t.id) return d.height;
  const stored = t.view === "notes" ? t.notesHeight : t.compactHeight;
  return stored > 0 ? stored : defaultHeight(t);
}

export function rowTops(tracks: Track[]): number[] {
  const tops: number[] = [];
  let y = 0;
  for (const t of tracks) {
    tops.push(y);
    y += rowHeight(t);
  }
  return tops;
}

export function contentHeight(tracks: Track[]): number {
  return tracks.reduce((s, t) => s + rowHeight(t), 0);
}

export function maxScrollY(tracks: Track[]): number {
  return Math.max(0, contentHeight(tracks) - 120);
}

export const isBlack = (pitch: number) => [1, 3, 6, 8, 10].includes(((pitch % 12) + 12) % 12);

// ---------- sichtbarer Tonbereich pro Spur ----------

/** Sichtbarer Tonbereich einer Notenzeile: tiefster Ton und Anzahl Halbtöne. */
export interface PitchView {
  low: number;
  span: number;
}

/** Tonbereich je Spur (vertikale Lage und Zoom). */
export const pitchViews = signal<Record<number, PitchView>>({});

/** Geometrie einer Notenzeile: Tonbereich plus daraus folgende Bahn- und Leistenmaße. */
export interface NoteGeo {
  view: PitchView;
  /** Höhe des Bereichs mit den Tonbahnen. */
  lanesH: number;
  /** Höhe einer Tonbahn (wächst mit der Zeilenhöhe). */
  laneH: number;
  /** Oberkante der Rhythmus-Leiste relativ zur Zeilenoberkante. */
  rhythmTop: number;
  /** Ausdrucksspur (Höhe 0 = ausgeblendet). */
  exprTop: number;
  exprH: number;
}

export function geoFor(t: Track): NoteGeo {
  const view = viewFor(t);
  const exprH = exprShown(t) ? EXPR_H : 0;
  const lanesH = Math.max(40, rowHeight(t) - NOTES_TOP - RHYTHM_H - 6 - (exprH ? exprH + 2 : 0));
  const rhythmTop = NOTES_TOP + lanesH + 2;
  return { view, lanesH, laneH: lanesH / view.span, rhythmTop, exprTop: rhythmTop + RHYTHM_H + 2, exprH };
}

/** Innerer Wertebereich der Ausdrucksspur (relativ zur Zeilenoberkante). */
function exprBox(g: NoteGeo): { top: number; h: number } {
  return { top: g.exprTop + 7, h: Math.max(4, g.exprH - 13) };
}

/** y-Position (relativ zur Zeile) eines Ausdruckswerts in der Ausdrucksspur. */
export function exprY(g: NoteGeo, d: ExprDef, v: number): number {
  const b = exprBox(g);
  return b.top + (1 - (v - d.min) / (d.max - d.min)) * b.h;
}

/** Ausdruckswert an einer y-Position (relativ zur Zeile). */
export function exprAt(g: NoteGeo, d: ExprDef, yRel: number): number {
  const b = exprBox(g);
  const f = 1 - (yRel - b.top) / b.h;
  return clampExpr(d.key, d.min + Math.max(0, Math.min(1, f)) * (d.max - d.min));
}

function clampView(low: number, span: number): PitchView {
  const s = Math.max(MIN_SPAN, Math.min(MAX_SPAN, Math.round(span)));
  return { low: Math.max(0, Math.min(128 - s, Math.round(low))), span: s };
}

/** Passt den Bereich an die Töne an; großer Umfang (z. B. Bassdrum bis Snare) wird herausgezoomt. */
function fitView(pitches: number[]): PitchView {
  if (!pitches.length) return clampView(60 - DEFAULT_SPAN / 2, DEFAULT_SPAN);
  const lo = Math.min(...pitches);
  const hi = Math.max(...pitches);
  const span = Math.max(DEFAULT_SPAN, hi - lo + 1 + 4);
  return clampView((lo + hi) / 2 - span / 2 + 0.5, span);
}

/** Töne, an denen sich die Ansicht ausrichtet (Noten bzw. erkannte Noten). */
function contentPitches(t: Track): number[] | null {
  if (t.kind === "instrument") {
    const pitches = t.clips.flatMap((c) => (c.source.type === "notes" ? c.source.notes.map((n) => n.pitch) : []));
    if (t.instrument?.kind === "drums") return [...pitches, 35, 49]; // alle Trommeln sichtbar
    if (!pitches.length && t.instrument?.kind === "bass") return [28, 48];
    return pitches;
  }
  const out: number[] = [];
  for (const c of t.clips) {
    if (c.source.type !== "audio") continue;
    const a = analyses.get(c.source.assetId);
    if (a === undefined || a === "loading") return null; // noch nicht bereit
    if (a !== "none") {
      const shift = Math.round(t.pitchSemitones);
      out.push(...a.notes.map((n) => n.midi + shift));
      for (const h of a.hits) if (h.midi != null) out.push(Math.round(h.midi) + shift);
    }
  }
  return out;
}

/**
 * Sichtbarer Tonbereich einer Notenzeile. Beim ersten Aufruf wird er eingepasst
 * und festgehalten, damit die Ansicht beim Bearbeiten nicht springt – egal ob die
 * erste Abfrage vom Zeichnen, von der Klaviatur oder von einem Mausklick kommt.
 */
export function viewFor(t: Track): PitchView {
  const stored = pitchViews.peek()[t.id];
  if (stored) return stored;
  const pitches = contentPitches(t);
  const view = fitView(pitches ?? []);
  if (pitches !== null && t.view === "notes") {
    pitchViews.update((m) => ({ ...m, [t.id]: view }));
  }
  return view;
}

/** Tonbereich verschieben (Halbtöne, positiv = höher). */
export function scrollPitch(trackId: number, delta: number, current: PitchView): void {
  pitchViews.update((m) => ({ ...m, [trackId]: clampView(current.low + delta, current.span) }));
}

/** Zoomen um die Mitte (positiv = mehr Halbtöne sichtbar). */
export function zoomPitch(trackId: number, delta: number, current: PitchView): void {
  const center = current.low + current.span / 2;
  const span = current.span + delta;
  pitchViews.update((m) => ({ ...m, [trackId]: clampView(center - span / 2, span) }));
}

export function refitPitch(trackId: number): void {
  pitchViews.update((m) => {
    const next = { ...m };
    delete next[trackId];
    return next;
  });
}

export const inView = (v: PitchView, pitch: number) => pitch >= v.low && pitch < v.low + v.span;

// ---------- Richtung der Tonachse (Einstellung) ----------
//
// „tief oben“: wie eine um 90° gedrehte Klaviatur, an der man links sitzt.
// „hoch oben“: wie in Noten und den meisten Pianorollen.

const AXIS_KEY = "klangwelt.tonachse";

function loadHighOnTop(): boolean {
  try {
    return localStorage.getItem(AXIS_KEY) === "hoch-oben";
  } catch {
    return false;
  }
}

/** true: hohe Töne oben, false: tiefe Töne oben (Standard). */
export const highOnTop = signal(loadHighOnTop());

export function setHighOnTop(on: boolean): void {
  highOnTop.value = on;
  try {
    localStorage.setItem(AXIS_KEY, on ? "hoch-oben" : "tief-oben");
  } catch {
    /* ohne Speicher */
  }
}

/** Ton der Bahn `k` (0 = oberste Bahn). */
export function pitchOfLane(v: PitchView, k: number): number {
  return highOnTop.peek() ? v.low + v.span - 1 - k : v.low + k;
}

/** Bahn-Position eines Tons (0 = oberste Bahn; Bruchteile für Tonhöhenkurven). */
export function laneOf(v: PitchView, pitch: number): number {
  return highOnTop.peek() ? v.low + v.span - 1 - pitch : pitch - v.low;
}

/** Oberkante der Bahn eines Tons, relativ zur Zeilenoberkante. */
export function laneY(g: NoteGeo, pitch: number): number {
  return NOTES_TOP + laneOf(g.view, pitch) * g.laneH;
}

/** Ton an einer y-Position relativ zur Zeilenoberkante. */
export function pitchAt(g: NoteGeo, yRel: number): number {
  return pitchOfLane(g.view, Math.floor((yRel - NOTES_TOP) / g.laneH));
}

/** Halbtöne, um die eine senkrechte Bewegung um `dy` Pixel die Tonhöhe ändert. */
export function pitchDelta(g: { laneH: number }, dy: number): number {
  const lanes = Math.round(dy / g.laneH);
  return highOnTop.peek() ? -lanes : lanes;
}

/** Tonbereich-Verschiebung für eine Mausrad-Bewegung (nach oben = was oben liegt). */
export function wheelPitchStep(deltaY: number): number {
  const towardTop = deltaY < 0;
  return towardTop === highOnTop.peek() ? 2 : -2;
}

/** Liegt ein Ton unterhalb/oberhalb des sichtbaren Bereichs am oberen Rand? */
export function offViewAtTop(v: PitchView, pitch: number): boolean {
  return (pitch < v.low) !== highOnTop.peek();
}

// ---------- Notenauswahl ----------

export const noteKey = (n: Note) => `${n.pitch}@${n.startBeats}`;

export const noteSel = signal<{ trackId: number; clipId: number; keys: string[] } | null>(null);

/** Rasterweite für Noten abhängig vom Zoom; mit Shift frei. */
export function noteGrid(pxPerBeat: number, free: boolean): number {
  if (free) return 0;
  if (pxPerBeat >= 160) return 0.125;
  if (pxPerBeat >= 70) return 0.25;
  if (pxPerBeat >= 30) return 0.5;
  return 1;
}

// ---------- Analysen für Audiospuren ----------

export const analyses = new Map<string, Analysis | "loading" | "none">();

export function analysisFor(assetId: string, onReady: () => void): Analysis | null {
  const a = analyses.get(assetId);
  if (a && a !== "loading" && a !== "none") return a;
  if (a === undefined) {
    analyses.set(assetId, "loading");
    backend
      .analyzeAsset(assetId)
      .then((r) => analyses.set(assetId, r))
      .catch(() => analyses.set(assetId, "none"))
      .finally(onReady);
  }
  return null;
}

export function analysisState(assetId: string): "loading" | "none" | "ready" | "missing" {
  const a = analyses.get(assetId);
  if (a === undefined) return "missing";
  return a === "loading" || a === "none" ? a : "ready";
}
