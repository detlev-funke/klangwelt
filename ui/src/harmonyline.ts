// Harmonie in der Zeitleiste: die Akkordfolge des ganzen Satzes (vom Rust-Kern
// erkannt) mit der Stimmung jedes Übergangs – dieselben Farben wie im
// Quintenzirkel – und markierten harmonischen Brüchen. Sichtbar, wenn
// eingeschaltet oder solange man im Harmonie-Fenster arbeitet.

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

/** Harmonie-Band in der Zeitleiste dauerhaft an. */
export const harmonyLaneOn = signal(readOn());
/** Maus oder Tastaturfokus im Harmonie-Fenster: Band erscheint von selbst. */
export const harmonyFocus = signal(false);
/** Im Harmonie-Fenster gewählte Tonart (null = automatisch aus den Noten). */
export const harmonyKey = signal<KeySpec | null>(null);
/** Erkannte Akkordfolge des Projekts (null = noch nicht geladen). */
export const harmonyLine = signal<HarmonyLine | null>(null);

export function setHarmonyLane(on: boolean): void {
  harmonyLaneOn.value = on;
  try {
    localStorage.setItem(LANE_KEY, on ? "an" : "aus");
  } catch {
    /* ohne Speicher */
  }
}

/** Farben der Stimmungen – wie im Quintenzirkel (CSS --mood-…). */
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
/** Akkordfolge laden und mit dem Projekt aktuell halten (gebremst – Bearbeiten erzeugt viele Schnappschüsse). */
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
        /* ohne Harmonie eben kein Band */
      }
    }, 300);
  });
}

/** Index des Akkords an Schlag `beat` (der letzte, der davor begonnen hat); -1 = vor dem ersten. */
export function chordIndexAt(line: HarmonyLine | null, beat: number): number {
  if (!line) return -1;
  let i = -1;
  for (let k = 0; k < line.chords.length; k++) {
    if (line.chords[k]!.startBeats <= beat + 1e-6) i = k;
    else break;
  }
  return i;
}
