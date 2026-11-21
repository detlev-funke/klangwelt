// Interface size and spacing of this device (gear menu). Both are per device:
// spacing in localStorage, size in the desktop shell's settings file or the
// browser's localStorage (bridge.ts). boot.ts applies the stored values before
// the first layout – same keys; keep them in step.

import { backend } from "./bridge.js";
import { signal } from "./signals.js";

/** Interface size: 40 … 200 % in steps of 5 (a stylus hits small controls, too). On a
 *  phone the top end is as far as a layout of 300 px allows (backend.uiScaleMax). */
export const SCALE_MIN = 0.4;
export const SCALE_MAX = 2;
export const SCALE_STEP = 0.05;

/** Spacing of the layout and the dock: gaps, gutters, tab bars, paddings. */
export type Density = "tight" | "normal" | "air";
export const DENSITIES: Density[] = ["tight", "normal", "air"];
const DENSITY_KEY = "klangwelt.density";

function storedDensity(): Density {
  try {
    const v = localStorage.getItem(DENSITY_KEY);
    return v === "tight" || v === "air" ? v : "normal";
  } catch {
    return "normal";
  }
}

export const density = signal<Density>(storedDensity());

/** Spacing at once (CSS custom properties under `html[data-density]`, styles.css). */
export function setDensity(d: Density): void {
  const root = document.documentElement;
  if (d === "normal") delete root.dataset.density;
  else root.dataset.density = d;
  try {
    if (d === "normal") localStorage.removeItem(DENSITY_KEY);
    else localStorage.setItem(DENSITY_KEY, d);
  } catch {
    // storage blocked: until the page is closed
  }
  density.value = d;
}

/** The interface size that applies right now (1 = 100 %). */
export const uiScale = signal(1);
void backend.uiScale().then((s) => (uiScale.value = s)).catch(() => {});

export async function setUiScale(scale: number): Promise<void> {
  const s = Math.min(SCALE_MAX, Math.max(SCALE_MIN, Math.round(scale / SCALE_STEP) * SCALE_STEP));
  uiScale.value = await backend.setUiScale(Math.round(s * 100) / 100);
}
