// Smoothing notes that come from recordings (core: klang_core::smooth): unusually
// short fragments go into a neighbour, repeated notes of one pitch into one.
// The strength chosen here applies whenever a recording becomes notes;
// the menu of selected notes smooths them afterwards.

import { t } from "./i18n.js";
import { signal } from "./signals.js";
import type { SmoothStrength } from "./types.js";

/** localStorage: strength used when a recording becomes notes. */
export const SMOOTH_KEY = "klangwelt.glaetten";
export const SMOOTH_DEFAULT: SmoothStrength = "medium";
export const SMOOTH_LEVELS: readonly SmoothStrength[] = ["off", "light", "medium", "strong"];

function stored(): SmoothStrength {
  try {
    const v = localStorage.getItem(SMOOTH_KEY);
    if (v && (SMOOTH_LEVELS as readonly string[]).includes(v)) return v as SmoothStrength;
  } catch {
    /* no storage: the default */
  }
  return SMOOTH_DEFAULT;
}

/** Strength for converting recordings into notes. */
export const smoothLevel = signal<SmoothStrength>(stored());

/** Choose the strength (remembered). */
export function setSmoothLevel(level: SmoothStrength): void {
  smoothLevel.value = level;
  try {
    localStorage.setItem(SMOOTH_KEY, level);
  } catch {
    /* without storage: for this session */
  }
}

/** Name of a strength on a button. */
export function smoothLabel(level: SmoothStrength): string {
  switch (level) {
    case "off":
      return t("Off");
    case "light":
      return t("Light");
    case "medium":
      return t("Medium");
    case "strong":
      return t("Strong");
  }
}

/** What a strength does (tooltip). */
export function smoothTitle(level: SmoothStrength): string {
  switch (level) {
    case "off":
      return t("Keep every detected note");
    case "light":
      return t("Only the clearest fragments: very short notes and notes of one pitch that touch");
    case "medium":
      return t("Short fragments go into the note next to them, a held note split in two becomes one again");
    case "strong":
      return t("Also longer fragments and small gaps between notes of one pitch");
  }
}
