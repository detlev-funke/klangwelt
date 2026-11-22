// What the track headers show (pan, pitch) and how wide they are: settings of the
// view, remembered in this browser / this app (not in the project).

import { signal } from "./signals.js";

const PAN_KEY = "klangwelt.spuren.pan";
const PITCH_KEY = "klangwelt.spuren.tonhoehe";
const WIDTH_KEY = "klangwelt.spuren.breite";

/** Width of the track headers: limits and default (pixels). */
export const HEADS_MIN = 220;
export const HEADS_MAX = 640;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    /* without storage */
  }
}

/** Pan slider in every track header. */
export const showPan = signal(read(PAN_KEY) !== "aus");
/** Pitch stepper (semitones) in every track header. */
export const showPitch = signal(read(PITCH_KEY) !== "aus");
/** Width of the track headers set with the splitter (null = default of the layout). */
export const headsWidth = signal<number | null>(((w) => (w >= HEADS_MIN && w <= HEADS_MAX ? w : null))(Number(read(WIDTH_KEY))));

export function toggleShowPan(): void {
  showPan.value = !showPan.peek();
  write(PAN_KEY, showPan.peek() ? "an" : "aus");
}

export function toggleShowPitch(): void {
  showPitch.value = !showPitch.peek();
  write(PITCH_KEY, showPitch.peek() ? "an" : "aus");
}

export function setHeadsWidth(w: number | null): void {
  const v = w === null ? null : Math.round(Math.max(HEADS_MIN, Math.min(HEADS_MAX, w)));
  headsWidth.value = v;
  write(WIDTH_KEY, v === null ? null : String(v));
}
