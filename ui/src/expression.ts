// Expression of individual notes: velocity, vibrato, brightness, bend (glide)
// and performance marks (dynamics, hairpins, accent, staccato, legato …).
// The values live in the note itself (see klang_core::Note); here are
// ranges, labels and the editing functions for the UI and the
// expression track.

import { t } from "./i18n.js";
import { signal } from "./signals.js";
import type { Note, Track } from "./types.js";

export type ExprKey = "velocity" | "vibrato" | "brightness" | "bend";

export interface ExprDef {
  key: ExprKey;
  label: string;
  /** Short hint on what the value does. */
  hint: string;
  min: number;
  max: number;
  /** Value without special expression. */
  neutral: number;
  /** Zero point in the middle (values upward and downward)? */
  bipolar: boolean;
  /** Quantization while dragging. */
  step: number;
  format(v: number): string;
}

/** Dynamic levels and their velocity. */
export const DYNAMICS: Array<[string, number]> = [["pp", 32], ["p", 48], ["mp", 64], ["mf", 80], ["f", 100], ["ff", 120]];

export function dynamicName(v: number): string {
  let best = DYNAMICS[0]!;
  for (const d of DYNAMICS) if (Math.abs(d[1] - v) < Math.abs(best[1] - v)) best = d;
  return best[0];
}

const signed = (v: number, digits = 0) => `${v > 0 ? "+" : v < 0 ? "−" : "±"}${Math.abs(v).toFixed(digits)}`;

export const EXPRS: Record<ExprKey, ExprDef> = {
  velocity: {
    key: "velocity", label: t("Velocity"), hint: t("Volume and hardness of the attack"),
    min: 1, max: 127, neutral: 100, bipolar: false, step: 1,
    format: (v) => `${Math.round(v)} (${dynamicName(v)})`,
  },
  vibrato: {
    key: "vibrato", label: t("Vibrato"), hint: t("Vibrato depth – starts with a delay, like in singing"),
    min: 0, max: 1, neutral: 0, bipolar: false, step: 0.01,
    format: (v) => (v > 0.005 ? t("± {cents} cents", { cents: Math.round(v * 100) }) : t("off")),
  },
  brightness: {
    key: "brightness", label: t("Brightness"), hint: t("darker ↔ brighter than the instrument's base sound"),
    min: -1, max: 1, neutral: 0, bipolar: true, step: 0.01,
    format: (v) => (Math.abs(v) < 0.005 ? t("neutral") : v > 0 ? t("{value}% brighter", { value: signed(v * 100) }) : t("{value}% darker", { value: signed(v * 100) })),
  },
  bend: {
    key: "bend", label: t("Bend"), hint: t("Scoop: the note starts offset and glides in"),
    min: -12, max: 12, neutral: 0, bipolar: true, step: 0.5,
    format: (v) => (Math.abs(v) < 0.05 ? t("off") : v < 0 ? t("{value} semitones from below", { value: signed(v, 1) }) : t("{value} semitones from above", { value: signed(v, 1) })),
  },
};

export const EXPR_ORDER: ExprKey[] = ["velocity", "vibrato", "brightness", "bend"];

/** Which expression forms the instrument of a track supports. */
export function exprKeysFor(t: Track | null | undefined): ExprKey[] {
  if (t?.instrument?.kind === "drums") return ["velocity", "brightness"];
  return EXPR_ORDER;
}

export function exprValue(n: Note, k: ExprKey): number {
  return k === "velocity" ? n.velocity : n[k] ?? 0;
}

export function clampExpr(k: ExprKey, v: number): number {
  const d = EXPRS[k];
  const x = Math.max(d.min, Math.min(d.max, v));
  return k === "velocity" ? Math.round(x) : Math.round(x / d.step) * d.step;
}

/** Note with a changed expression value (neutral values are dropped). */
export function withExpr(n: Note, k: ExprKey, v: number): Note {
  const x = clampExpr(k, v);
  if (k === "velocity") return { ...n, velocity: x };
  const out: Note = { ...n };
  if (Math.abs(x) < 1e-3) delete out[k];
  else out[k] = +x.toFixed(3);
  return out;
}

// ---------- Expression track per track (view only, stored in the browser) ----------

const LANE_KEY = "klangwelt.ausdruck";

interface LaneState {
  key: Record<number, ExprKey>;
  hidden: Record<number, boolean>;
}

function loadLanes(): LaneState {
  try {
    const raw = JSON.parse(localStorage.getItem(LANE_KEY) ?? "null") as Partial<LaneState> | null;
    return { key: raw?.key ?? {}, hidden: raw?.hidden ?? {} };
  } catch {
    return { key: {}, hidden: {} };
  }
}

/** Chosen expression form of the track and whether the expression track is hidden. */
export const exprLanes = signal<LaneState>(loadLanes());

function saveLanes(s: LaneState): void {
  exprLanes.value = s;
  try {
    localStorage.setItem(LANE_KEY, JSON.stringify(s));
  } catch {
    /* without storage */
  }
}

export function laneKeyFor(t: Track): ExprKey {
  const k = exprLanes.peek().key[t.id];
  return k && exprKeysFor(t).includes(k) ? k : "velocity";
}

export function setLaneKey(trackId: number, k: ExprKey): void {
  const s = exprLanes.peek();
  saveLanes({ ...s, key: { ...s.key, [trackId]: k } });
}

export function exprShown(t: Track): boolean {
  return t.kind === "instrument" && t.view === "notes" && !exprLanes.peek().hidden[t.id];
}

export function toggleExprLane(trackId: number): void {
  const s = exprLanes.peek();
  const hidden = { ...s.hidden };
  if (hidden[trackId]) delete hidden[trackId];
  else hidden[trackId] = true;
  saveLanes({ ...s, hidden });
}

// ---------- Performance marks ----------

export type Mark =
  | { kind: "dyn"; velocity: number }
  | { kind: "cresc" }
  | { kind: "decresc" }
  | { kind: "accent" }
  | { kind: "staccato" }
  | { kind: "legato" }
  | { kind: "vibrato" }
  | { kind: "brighter" }
  | { kind: "darker" }
  | { kind: "scoop" }
  | { kind: "reset" };

/** Target indices sorted by start time. */
function byStart(notes: Note[], idx: number[]): number[] {
  return [...idx].sort((a, b) => notes[a]!.startBeats - notes[b]!.startBeats || notes[a]!.pitch - notes[b]!.pitch);
}

/**
 * Applies a performance mark to the notes `idx` and returns the new
 * note list (same order, indices stay valid).
 */
export function applyMark(notes: Note[], idx: number[], mark: Mark): Note[] {
  const out = notes.slice();
  const set = (i: number, n: Note) => (out[i] = n);
  const targets = byStart(notes, idx);
  switch (mark.kind) {
    case "dyn":
      for (const i of targets) set(i, withExpr(out[i]!, "velocity", mark.velocity));
      break;
    case "cresc":
    case "decresc": {
      // From the volume of the first note linearly by ±40 upward or downward;
      // simultaneous notes (chords) get the same value.
      if (targets.length < 2) break;
      const starts = [...new Set(targets.map((i) => notes[i]!.startBeats))].sort((a, b) => a - b);
      const first = notes[targets[0]!]!.velocity;
      const last = Math.max(1, Math.min(127, first + (mark.kind === "cresc" ? 40 : -40)));
      const span = starts[starts.length - 1]! - starts[0]! || 1;
      for (const i of targets) {
        const f = (notes[i]!.startBeats - starts[0]!) / span;
        set(i, withExpr(out[i]!, "velocity", first + (last - first) * f));
      }
      break;
    }
    case "accent":
      for (const i of targets) set(i, withExpr(out[i]!, "velocity", out[i]!.velocity + 25));
      break;
    case "staccato":
      for (const i of targets) set(i, { ...out[i]!, lengthBeats: Math.max(0.0625, +(out[i]!.lengthBeats * 0.5).toFixed(4)) });
      break;
    case "legato": {
      // Extend each note to the start of the next (later) note of the clip.
      const starts = [...new Set(notes.map((n) => n.startBeats))].sort((a, b) => a - b);
      for (const i of targets) {
        const n = out[i]!;
        const next = starts.find((s) => s > n.startBeats + 1e-9);
        if (next !== undefined) set(i, { ...n, lengthBeats: +(next - n.startBeats).toFixed(4) });
      }
      break;
    }
    case "vibrato": {
      const allOn = targets.every((i) => (notes[i]!.vibrato ?? 0) > 0);
      for (const i of targets) set(i, withExpr(out[i]!, "vibrato", allOn ? 0 : 0.3));
      break;
    }
    case "brighter":
    case "darker":
      for (const i of targets) set(i, withExpr(out[i]!, "brightness", (out[i]!.brightness ?? 0) + (mark.kind === "brighter" ? 0.4 : -0.4)));
      break;
    case "scoop": {
      const allOn = targets.every((i) => (notes[i]!.bend ?? 0) === -2);
      for (const i of targets) set(i, withExpr(out[i]!, "bend", allOn ? 0 : -2));
      break;
    }
    case "reset":
      for (const i of targets) {
        const { vibrato: _v, brightness: _b, bend: _g, ...rest } = out[i]!;
        set(i, { ...rest, velocity: 100 });
      }
      break;
  }
  return out;
}

/** Sets an expression value for all target notes. */
export function setExprAll(notes: Note[], idx: number[], k: ExprKey, v: number): Note[] {
  const out = notes.slice();
  for (const i of idx) out[i] = withExpr(out[i]!, k, v);
  return out;
}
