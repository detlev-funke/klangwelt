// Shared geometry and state of the note view (track headers + timeline).

import { backend } from "./bridge.js";
import { type ExprDef, clampExpr, exprShown } from "./expression.js";
import { signal } from "./signals.js";
import { ROW_H } from "./store.js";
import type { Analysis, Note, Project, Track, TrackGroup } from "./types.js";

export const NOTES_ROW_H = 236;
/** Height of the expression lane below the rhythm strip (instrument tracks). */
export const EXPR_H = 48;
/** Band at the top of the note row for clip names (clips can be moved there). */
export const NOTES_TOP = 20;
/** Height of the rhythm strip at the bottom of the note row (unpitched beats like hi-hats). */
export const RHYTHM_H = 18;
/** Visible semitones: default, limits for zoom. */
export const DEFAULT_SPAN = 21;
const MIN_SPAN = 12;
const MAX_SPAN = 60;

// ---------- Row heights (individually draggable) ----------

/** Height while dragging the splitter (preview, not yet saved). */
export const heightDraft = signal<{ trackId: number; height: number } | null>(null);

export function defaultHeight(t: Track): number {
  return t.view === "notes" ? NOTES_ROW_H + (exprShown(t) ? EXPR_H : 0) : ROW_H;
}

/** Allowed heights per view (pixels). */
export function heightLimits(t: Track): [number, number] {
  return t.view === "notes" ? [140, 1200] : [48, 600];
}

export function rowHeight(t: Track): number {
  const d = heightDraft.peek();
  if (d && d.trackId === t.id) return d.height;
  const stored = t.view === "notes" ? t.notesHeight : t.compactHeight;
  return stored > 0 ? stored : defaultHeight(t);
}

// ---------- Rows: tracks and track groups ----------

/** Height of a group's row in the track list and the timeline. */
export const GROUP_H = 36;

export type Row =
  | { kind: "track"; track: Track; group: TrackGroup | null; top: number; h: number }
  | { kind: "group"; group: TrackGroup; tracks: Track[]; top: number; h: number };

type Layout = Pick<Project, "tracks" | "groups">;

/** The group a track belongs to, if it exists. */
export function groupOf(p: Layout, t: Track): TrackGroup | null {
  return t.group ? (p.groups ?? []).find((g) => g.id === t.group) ?? null : null;
}

/** Muted itself or by its group. */
export function isMuted(p: Layout, t: Track): boolean {
  return t.mute || !!groupOf(p, t)?.mute;
}

/**
 * Rows from top to bottom: a group's row comes before its first track, its other tracks
 * follow right after (even if the project order scattered them); the tracks of a folded
 * group are hidden. Groups without tracks come last.
 */
export function layoutRows(p: Layout): Row[] {
  const rows: Row[] = [];
  const groups = p.groups ?? [];
  const done = new Set<number>();
  let y = 0;
  const push = (r: Row) => {
    rows.push(r);
    y += r.h;
  };
  const pushGroup = (g: TrackGroup) => {
    done.add(g.id);
    const tracks = p.tracks.filter((t) => t.group === g.id);
    push({ kind: "group", group: g, tracks, top: y, h: GROUP_H });
    if (!g.collapsed) for (const t of tracks) push({ kind: "track", track: t, group: g, top: y, h: rowHeight(t) });
  };
  for (const t of p.tracks) {
    const g = groupOf(p, t);
    if (!g) push({ kind: "track", track: t, group: null, top: y, h: rowHeight(t) });
    else if (!done.has(g.id)) pushGroup(g);
  }
  for (const g of groups) if (!done.has(g.id)) pushGroup(g);
  return rows;
}

/** A project – or just tracks (without groups). */
function asLayout(src: Track[] | Layout): Layout {
  return Array.isArray(src) ? { tracks: src } : src;
}

/** Top of every track's row, in project order (a track hidden in a folded group: -1). */
export function rowTops(src: Track[] | Layout): number[] {
  const p = asLayout(src);
  const tops = new Map(layoutRows(p).flatMap((r) => (r.kind === "track" ? [[r.track.id, r.top] as const] : [])));
  return p.tracks.map((t) => tops.get(t.id) ?? -1);
}

export function contentHeight(src: Track[] | Layout): number {
  return layoutRows(asLayout(src)).reduce((s, r) => s + r.h, 0);
}

export function maxScrollY(p: Layout | null | undefined): number {
  return p ? Math.max(0, contentHeight(p) - 120) : 0;
}

export const isBlack = (pitch: number) => [1, 3, 6, 8, 10].includes(((pitch % 12) + 12) % 12);

// ---------- Drums: drum kit or by pitch ----------

/** Control of the drums: 0 = drum kit (each drum key its drum), 1 = every key by pitch. */
export const KEYS_MODE = "tasten";
/** Tuned drums: from C3 the tom, from C5 the snare, below C3 the kick drum (as in the core). */
export const TUNED_TOM_FROM = 48;
export const TUNED_SNARE_FROM = 72;

/** How a track's keys play: "kit" = drum per drum key, "pitch" = tuned drums, null = no drums. */
export function drumMode(t: Track | undefined): "kit" | "pitch" | null {
  if (t?.kind !== "instrument" || t.instrument?.kind !== "drums") return null;
  return (t.instrument.params[KEYS_MODE] ?? 0) >= 0.5 ? "pitch" : "kit";
}

// ---------- visible pitch range per track ----------

/** Visible pitch range of a note row: lowest note and number of semitones. */
export interface PitchView {
  low: number;
  span: number;
}

/** Pitch range per track (vertical position and zoom). */
export const pitchViews = signal<Record<number, PitchView>>({});

/** Geometry of a note row: pitch range plus the resulting lane and strip dimensions. */
export interface NoteGeo {
  view: PitchView;
  /** Height of the area with the pitch lanes. */
  lanesH: number;
  /** Height of a pitch lane (grows with the row height). */
  laneH: number;
  /** Top edge of the rhythm strip relative to the top edge of the row. */
  rhythmTop: number;
  /** Expression lane (height 0 = hidden). */
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

/** Inner value range of the expression lane (relative to the top edge of the row). */
function exprBox(g: NoteGeo): { top: number; h: number } {
  return { top: g.exprTop + 7, h: Math.max(4, g.exprH - 13) };
}

/** y position (relative to the row) of an expression value in the expression lane. */
export function exprY(g: NoteGeo, d: ExprDef, v: number): number {
  const b = exprBox(g);
  return b.top + (1 - (v - d.min) / (d.max - d.min)) * b.h;
}

/** Expression value at a y position (relative to the row). */
export function exprAt(g: NoteGeo, d: ExprDef, yRel: number): number {
  const b = exprBox(g);
  const f = 1 - (yRel - b.top) / b.h;
  return clampExpr(d.key, d.min + Math.max(0, Math.min(1, f)) * (d.max - d.min));
}

function clampView(low: number, span: number): PitchView {
  const s = Math.max(MIN_SPAN, Math.min(MAX_SPAN, Math.round(span)));
  return { low: Math.max(0, Math.min(128 - s, Math.round(low))), span: s };
}

/** Fits the range to the notes; a large span (e.g. bass drum to snare) is zoomed out. */
function fitView(pitches: number[]): PitchView {
  if (!pitches.length) return clampView(60 - DEFAULT_SPAN / 2, DEFAULT_SPAN);
  const lo = Math.min(...pitches);
  const hi = Math.max(...pitches);
  const span = Math.max(DEFAULT_SPAN, hi - lo + 1 + 4);
  return clampView((lo + hi) / 2 - span / 2 + 0.5, span);
}

/** Pitches the view aligns to (notes or detected notes). */
function contentPitches(t: Track): number[] | null {
  if (t.kind === "instrument") {
    const pitches = t.clips.flatMap((c) => (c.source.type === "notes" ? c.source.notes.map((n) => n.pitch) : []));
    if (drumMode(t) === "kit") return [...pitches, 35, 49]; // all drums visible
    if (!pitches.length && t.instrument?.kind === "bass") return [28, 48];
    return pitches;
  }
  const out: number[] = [];
  for (const c of t.clips) {
    if (c.source.type !== "audio") continue;
    const a = analyses.get(c.source.assetId);
    if (a === undefined || a === "loading") return null; // not ready yet
    if (a !== "none") {
      const shift = Math.round(t.pitchSemitones);
      out.push(...a.notes.map((n) => n.midi + shift));
      for (const h of a.hits) if (h.midi != null) out.push(Math.round(h.midi) + shift);
    }
  }
  return out;
}

/**
 * Visible pitch range of a note row. On the first call it is fitted
 * and pinned so that the view does not jump while editing – regardless of whether the
 * first query comes from drawing, from the keyboard or from a mouse click.
 */
export function viewFor(t: Track): PitchView {
  // Drums switched between drum kit and pitch (or became/stopped being drums): fit again –
  // the drum kit shows its drums, by pitch the notes like any melody.
  const mode = drumMode(t);
  if (fittedFor.has(t.id) && fittedFor.get(t.id) !== mode) {
    fittedFor.delete(t.id);
    if (pitchViews.peek()[t.id]) refitPitch(t.id);
  }
  const stored = pitchViews.peek()[t.id];
  if (stored) return stored;
  const pitches = contentPitches(t);
  const view = fitView(pitches ?? []);
  if (pitches !== null && t.view === "notes") {
    fittedFor.set(t.id, mode);
    pitchViews.update((m) => ({ ...m, [t.id]: view }));
  }
  return view;
}

/** Drum mode a track's pinned pitch range was fitted for. */
const fittedFor = new Map<number, "kit" | "pitch" | null>();

/** Shifts the pitch range (semitones, positive = higher). */
export function scrollPitch(trackId: number, delta: number, current: PitchView): void {
  pitchViews.update((m) => ({ ...m, [trackId]: clampView(current.low + delta, current.span) }));
}

/** Zooms around the center (positive = more semitones visible). */
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

// ---------- Direction of the pitch axis (setting) ----------
//
// "low on top": like a keyboard rotated by 90°, with you sitting at the left.
// "high on top": like in sheet music and most piano rolls.

const AXIS_KEY = "klangwelt.tonachse";

function loadHighOnTop(): boolean {
  try {
    return localStorage.getItem(AXIS_KEY) === "hoch-oben";
  } catch {
    return false;
  }
}

/** true: high notes on top, false: low notes on top (default). */
export const highOnTop = signal(loadHighOnTop());

export function setHighOnTop(on: boolean): void {
  highOnTop.value = on;
  try {
    localStorage.setItem(AXIS_KEY, on ? "hoch-oben" : "tief-oben");
  } catch {
    /* without storage */
  }
}

/** Pitch of lane `k` (0 = topmost lane). */
export function pitchOfLane(v: PitchView, k: number): number {
  return highOnTop.peek() ? v.low + v.span - 1 - k : v.low + k;
}

/** Lane position of a pitch (0 = topmost lane; fractions for pitch curves). */
export function laneOf(v: PitchView, pitch: number): number {
  return highOnTop.peek() ? v.low + v.span - 1 - pitch : pitch - v.low;
}

/** Top edge of a pitch's lane, relative to the top edge of the row. */
export function laneY(g: NoteGeo, pitch: number): number {
  return NOTES_TOP + laneOf(g.view, pitch) * g.laneH;
}

/** Pitch at a y position relative to the top edge of the row. */
export function pitchAt(g: NoteGeo, yRel: number): number {
  return pitchOfLane(g.view, Math.floor((yRel - NOTES_TOP) / g.laneH));
}

/** Semitones by which a vertical movement of `dy` pixels changes the pitch. */
export function pitchDelta(g: { laneH: number }, dy: number): number {
  const lanes = Math.round(dy / g.laneH);
  return highOnTop.peek() ? -lanes : lanes;
}

/** Pitch range shift for a mouse wheel movement (up = whatever lies on top). */
export function wheelPitchStep(deltaY: number): number {
  const towardTop = deltaY < 0;
  return towardTop === highOnTop.peek() ? 2 : -2;
}

/** Does a pitch lie below/above the visible range at the top edge? */
export function offViewAtTop(v: PitchView, pitch: number): boolean {
  return (pitch < v.low) !== highOnTop.peek();
}

// ---------- Note selection ----------

export const noteKey = (n: Note) => `${n.pitch}@${n.startBeats}`;

export const noteSel = signal<{ trackId: number; clipId: number; keys: string[] } | null>(null);

/** Grid width for notes depending on zoom; free with Shift. */
export function noteGrid(pxPerBeat: number, free: boolean): number {
  if (free) return 0;
  if (pxPerBeat >= 160) return 0.125;
  if (pxPerBeat >= 70) return 0.25;
  if (pxPerBeat >= 30) return 0.5;
  return 1;
}

// ---------- Analyses for audio tracks ----------

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
