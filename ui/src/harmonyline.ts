// Harmony in the timeline: the chord progression of the whole piece (detected by
// the Rust core) with the mood of each transition – the same colors as in the
// circle of fifths – and marked harmonic breaks. Visible when switched on
// or while working in the Harmony window.

import { backend } from "./bridge.js";
import { t } from "./i18n.js";
import { effect, signal } from "./signals.js";
import { snapshot } from "./store.js";
import type { HarmonyLine, KeySpec, Mood } from "./types.js";

const LANE_KEY = "klangwelt.harmonie.zeitleiste";

function readOn(): boolean {
  try {
    return localStorage.getItem(LANE_KEY) === "an";
  } catch {
    return false;
  }
}

/** Harmony band in the timeline permanently on. */
export const harmonyLaneOn = signal(readOn());
/** Mouse or keyboard focus in the Harmony window: the band appears by itself. */
export const harmonyFocus = signal(false);
/** Key chosen in the Harmony window (null = automatic from the notes). */
export const harmonyKey = signal<KeySpec | null>(null);
/** Detected chord progression of the project (null = not loaded yet). */
export const harmonyLine = signal<HarmonyLine | null>(null);

export function setHarmonyLane(on: boolean): void {
  harmonyLaneOn.value = on;
  try {
    localStorage.setItem(LANE_KEY, on ? "an" : "aus");
  } catch {
    /* without storage */
  }
}

/** Colors of the moods – as in the circle of fifths (CSS --mood-…). */
export const MOOD_COLORS: Record<Mood, string> = {
  loesend: "#5ee08f",
  spannend: "#ffb454",
  ueberraschend: "#ff7ac6",
  hell: "#ffe066",
  dunkel: "#6f9bff",
};
export const MOOD_LABELS: Record<Mood, string> = {
  loesend: t("resolving"),
  spannend: t("tense"),
  ueberraschend: t("surprising"),
  hell: t("bright"),
  dunkel: t("dark"),
};

let installed = false;
/** Load the chord progression and keep it up to date with the project (throttled – editing creates many snapshots). */
export function watchHarmonyLine(): void {
  if (installed || !backend.native) return;
  installed = true;
  let timer = 0;
  let seq = 0;
  effect(() => {
    snapshot.value;
    const key = harmonyKey.value;
    clearTimeout(timer);
    timer = window.setTimeout(async () => {
      const my = ++seq;
      try {
        const line = await backend.harmonyTimeline(key);
        if (my === seq) harmonyLine.value = line;
      } catch {
        /* no harmony, no band */
      }
    }, 300);
  });
}

/** Index of the chord at beat `beat` (the last one that began before it); -1 = before the first. */
export function chordIndexAt(line: HarmonyLine | null, beat: number): number {
  if (!line) return -1;
  let i = -1;
  for (let k = 0; k < line.chords.length; k++) {
    if (line.chords[k]!.startBeats <= beat + 1e-6) i = k;
    else break;
  }
  return i;
}
