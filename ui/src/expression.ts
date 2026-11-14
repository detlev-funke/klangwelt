// Ausdruck einzelner Noten: Anschlag, Vibrato, Helligkeit, Biegung (Anschleifen)
// und Vortragszeichen (Dynamik, Gabeln, Akzent, Staccato, Legato …).
// Die Werte liegen in der Note selbst (siehe klang_core::Note); hier stehen
// Bereiche, Beschriftungen und die Bearbeitungsfunktionen für Oberfläche und
// Ausdrucksspur.

import { signal } from "./signals.js";
import type { Note, Track } from "./types.js";

export type ExprKey = "velocity" | "vibrato" | "brightness" | "bend";

export interface ExprDef {
  key: ExprKey;
  label: string;
  /** Kurzer Hinweis, was der Wert bewirkt. */
  hint: string;
  min: number;
  max: number;
  /** Wert ohne besonderen Ausdruck. */
  neutral: number;
  /** Nullpunkt in der Mitte (Werte nach oben und unten)? */
  bipolar: boolean;
  /** Rasterung beim Ziehen. */
  step: number;
  format(v: number): string;
}

/** Dynamikstufen und ihr Anschlag. */
export const DYNAMICS: Array<[string, number]> = [["pp", 32], ["p", 48], ["mp", 64], ["mf", 80], ["f", 100], ["ff", 120]];

export function dynamicName(v: number): string {
  let best = DYNAMICS[0]!;
  for (const d of DYNAMICS) if (Math.abs(d[1] - v) < Math.abs(best[1] - v)) best = d;
  return best[0];
}

const signed = (v: number, digits = 0) => `${v > 0 ? "+" : v < 0 ? "−" : "±"}${Math.abs(v).toFixed(digits)}`;

export const EXPRS: Record<ExprKey, ExprDef> = {
  velocity: {
    key: "velocity", label: "Anschlag", hint: "Lautstärke und Härte des Anschlags",
    min: 1, max: 127, neutral: 100, bipolar: false, step: 1,
    format: (v) => `${Math.round(v)} (${dynamicName(v)})`,
  },
  vibrato: {
    key: "vibrato", label: "Vibrato", hint: "Tiefe des Vibratos – setzt verzögert ein wie beim Singen",
    min: 0, max: 1, neutral: 0, bipolar: false, step: 0.01,
    format: (v) => (v > 0.005 ? `± ${Math.round(v * 100)} Cent` : "aus"),
  },
  brightness: {
    key: "brightness", label: "Helligkeit", hint: "dunkler ↔ heller als der Grundklang des Instruments",
    min: -1, max: 1, neutral: 0, bipolar: true, step: 0.01,
    format: (v) => (Math.abs(v) < 0.005 ? "neutral" : `${signed(v * 100)} % ${v > 0 ? "heller" : "dunkler"}`),
  },
  bend: {
    key: "bend", label: "Biegung", hint: "Anschleifen: Ton beginnt versetzt und gleitet hinein",
    min: -12, max: 12, neutral: 0, bipolar: true, step: 0.5,
    format: (v) => (Math.abs(v) < 0.05 ? "aus" : `${signed(v, 1)} Halbtöne ${v < 0 ? "von unten" : "von oben"}`),
  },
};

export const EXPR_ORDER: ExprKey[] = ["velocity", "vibrato", "brightness", "bend"];

/** Welche Ausdrucksformen das Instrument einer Spur umsetzt. */
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

/** Note mit geändertem Ausdruckswert (neutrale Werte entfallen). */
export function withExpr(n: Note, k: ExprKey, v: number): Note {
  const x = clampExpr(k, v);
  if (k === "velocity") return { ...n, velocity: x };
  const out: Note = { ...n };
  if (Math.abs(x) < 1e-3) delete out[k];
  else out[k] = +x.toFixed(3);
  return out;
}

// ---------- Ausdrucksspur je Spur (nur Ansicht, im Browser gespeichert) ----------

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

/** Gewählte Ausdrucksform der Spur und ob die Ausdrucksspur ausgeblendet ist. */
export const exprLanes = signal<LaneState>(loadLanes());

function saveLanes(s: LaneState): void {
  exprLanes.value = s;
  try {
    localStorage.setItem(LANE_KEY, JSON.stringify(s));
  } catch {
    /* ohne Speicher */
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

// ---------- Vortragszeichen ----------

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

/** Ziel-Indizes nach Startzeit sortiert. */
function byStart(notes: Note[], idx: number[]): number[] {
  return [...idx].sort((a, b) => notes[a]!.startBeats - notes[b]!.startBeats || notes[a]!.pitch - notes[b]!.pitch);
}

/**
 * Wendet ein Vortragszeichen auf die Noten `idx` an und gibt die neue
 * Notenliste zurück (gleiche Reihenfolge, Indizes bleiben gültig).
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
      // Von der Lautstärke der ersten Note linear um ±40 nach oben bzw. unten;
      // gleichzeitige Töne (Akkorde) bekommen denselben Wert.
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
      // Jede Note bis zum Beginn der nächsten (späteren) Note des Clips verlängern.
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

/** Setzt einen Ausdruckswert für alle Ziel-Noten. */
export function setExprAll(notes: Note[], idx: number[], k: ExprKey, v: number): Note[] {
  const out = notes.slice();
  for (const i of idx) out[i] = withExpr(out[i]!, k, v);
  return out;
}
