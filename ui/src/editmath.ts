// Reine Rechenhilfen der Zeitleiste (ohne DOM) – auch direkt in Node testbar.

/** Auf ein Raster runden; Raster 0 = frei. */
export const snapTo = (v: number, grid: number): number => (grid ? Math.round(v / grid) * grid : v);

/** Abstand in Schlägen lesbar: „+1 Takt 2 Schläge“, „−0,5 Schläge“, „±0“. */
export function fmtShift(beats: number, bpb: number): string {
  if (Math.abs(beats) < 1e-6) return "±0";
  const sign = beats > 0 ? "+" : "−";
  const a = Math.abs(beats);
  const bars = Math.floor(a / bpb + 1e-9);
  const rest = Math.round((a - bars * bpb) * 1000) / 1000;
  const parts: string[] = [];
  if (bars) parts.push(`${bars} ${bars === 1 ? "Takt" : "Takte"}`);
  if (rest > 1e-6) parts.push(`${rest.toLocaleString("de-DE")} ${rest === 1 ? "Schlag" : "Schläge"}`);
  return `${sign}${parts.join(" ")}`;
}

/** Raster für Zeitbereiche im Lineal: Schläge, bei engem Zoom ganze Takte; frei = 0. */
export function rangeGrid(pxPerBeat: number, beatsPerBar: number, free: boolean): number {
  if (free) return 0;
  return pxPerBeat >= 24 ? 1 : beatsPerBar;
}

/**
 * Bereich, den ein Noten-Clip für diese Noten braucht – relativ zu seinem
 * Anfang, in ganzen Takten: `lo` ≤ 0 (wie weit er nach links wachsen muss,
 * höchstens bis zum Projektanfang), `hi` ≥ bisherige Länge.
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

/** Takt.Schlag einer Position (1-basiert), Bruchteile mit Komma: „3.1“, „2.2,5“. */
export function barBeat(beats: number, beatsPerBar: number): string {
  const bpb = Math.max(1, beatsPerBar);
  const bar = Math.floor(beats / bpb + 1e-9);
  const beat = Math.round((beats - bar * bpb + 1) * 1000) / 1000;
  return `${bar + 1}.${beat.toLocaleString("de-DE")}`;
}

/** Zeitbereich lesbar: ganze Takte als „Takt 2“ / „Takt 2–3“, sonst „Takt 1.3 – 2.2“. */
export function rangeLabel(start: number, end: number, beatsPerBar: number): string {
  const bpb = Math.max(1, beatsPerBar);
  const whole = (b: number) => Math.abs(b / bpb - Math.round(b / bpb)) < 1e-9;
  if (whole(start) && whole(end)) {
    const a = Math.round(start / bpb) + 1;
    const z = Math.round(end / bpb);
    return z <= a ? `Takt ${a}` : `Takt ${a}–${z}`;
  }
  return `Takt ${barBeat(start, bpb)} – ${barBeat(end, bpb)}`;
}

/** Raster (in Schlägen) als Notenwert: „1/8“; Schlag-Einheit aus der Taktart (4 = Viertel). */
export function gridLabel(grid: number, beatUnit: number): string {
  const d = beatUnit / grid;
  if (d >= 1 && Math.abs(d - Math.round(d)) < 1e-6) return `1/${Math.round(d)}`;
  return `${grid.toLocaleString("de-DE")} Schläge`;
}

/** Raster eines Notenwerts 1/`d` in Schlägen (4/4: 1/4 = 1, 1/8 = 0,5; 6/8: 1/8 = 1). */
export function gridOf(d: number, beatUnit: number): number {
  return beatUnit / d;
}
