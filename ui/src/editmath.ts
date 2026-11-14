// Pure calculation helpers of the timeline (without DOM) – also directly testable in Node.

import { language, plural, t } from "./i18n.js";

/** Rounds to a grid; grid 0 = free. */
export const snapTo = (v: number, grid: number): number => (grid ? Math.round(v / grid) * grid : v);

/** Distance in beats, readable: "+1 bar 2 beats", "−0.5 beats", "±0". */
export function fmtShift(beats: number, bpb: number): string {
  if (Math.abs(beats) < 1e-6) return "±0";
  const sign = beats > 0 ? "+" : "−";
  const a = Math.abs(beats);
  const bars = Math.floor(a / bpb + 1e-9);
  const rest = Math.round((a - bars * bpb) * 1000) / 1000;
  const parts: string[] = [];
  if (bars) parts.push(plural(bars, "{n} bar", "{n} bars"));
  if (rest > 1e-6) parts.push(plural(rest, "{n} beat", "{n} beats", { n: rest.toLocaleString(language()) }));
  return `${sign}${parts.join(" ")}`;
}

/** Grid for time ranges in the ruler: beats, whole bars when zoomed out; free = 0. */
export function rangeGrid(pxPerBeat: number, beatsPerBar: number, free: boolean): number {
  if (free) return 0;
  return pxPerBeat >= 24 ? 1 : beatsPerBar;
}

/**
 * Range a note clip needs for these notes – relative to its
 * start, in whole bars: `lo` ≤ 0 (how far it must grow to the left,
 * at most to the project start), `hi` ≥ the previous length.
 */
export function clipSpan(
  notes: ReadonlyArray<{ startBeats: number; lengthBeats: number }>,
  lengthBeats: number,
  beatsPerBar: number,
  clipStart: number,
): { lo: number; hi: number } {
  const bpb = Math.max(1, beatsPerBar);
  const minStart = Math.min(0, ...notes.map((n) => n.startBeats));
  const lo = minStart < -1e-9 ? -Math.min(clipStart, Math.ceil(-minStart / bpb - 1e-9) * bpb) : 0;
  const maxEnd = Math.max(0, ...notes.map((n) => n.startBeats + n.lengthBeats));
  const needed = Math.ceil((maxEnd - lo) / bpb - 1e-9) * bpb;
  return { lo, hi: lo + Math.max(lengthBeats - lo, needed) };
}

/** Bar.beat of a position (1-based), fractions with a comma: "3.1", "2.2,5". */
export function barBeat(beats: number, beatsPerBar: number): string {
  const bpb = Math.max(1, beatsPerBar);
  const bar = Math.floor(beats / bpb + 1e-9);
  const beat = Math.round((beats - bar * bpb + 1) * 1000) / 1000;
  return `${bar + 1}.${beat.toLocaleString(language())}`;
}

/** Time range, readable: whole bars as "Bar 2" / "bars 2–3", otherwise "bars 1.3 – 2.2". */
export function rangeLabel(start: number, end: number, beatsPerBar: number): string {
  const bpb = Math.max(1, beatsPerBar);
  const whole = (b: number) => Math.abs(b / bpb - Math.round(b / bpb)) < 1e-9;
  if (whole(start) && whole(end)) {
    const a = Math.round(start / bpb) + 1;
    const z = Math.round(end / bpb);
    return z <= a ? t("Bar {bar}", { bar: a }) : t("bars {from}–{to}", { from: a, to: z });
  }
  return t("bars {from} – {to}", { from: barBeat(start, bpb), to: barBeat(end, bpb) });
}

/** Grid (in beats) as a note value: "1/8"; beat unit from the time signature (4 = quarter). */
export function gridLabel(grid: number, beatUnit: number): string {
  const d = beatUnit / grid;
  if (d >= 1 && Math.abs(d - Math.round(d)) < 1e-6) return `1/${Math.round(d)}`;
  return plural(grid, "{n} beat", "{n} beats", { n: grid.toLocaleString(language()) });
}

/** Grid of a note value 1/`d` in beats (4/4: 1/4 = 1, 1/8 = 0.5; 6/8: 1/8 = 1). */
export function gridOf(d: number, beatUnit: number): number {
  return beatUnit / d;
}
