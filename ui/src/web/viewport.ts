// Interface size in the browser (thin shell part, like the desktop's zoom in
// lib.rs): phones and tablets zoom the page through the viewport meta – a layout
// width of device width / scale at an initial scale of `scale`. That is the
// browser's own page zoom, so pointer events, element boxes and canvas sizes stay
// in one coordinate system and every hit test keeps working; CSS `zoom` or a
// transform would not. boot.ts applies the stored size before the first layout
// (same key, same rule).
//
// Desktop browsers ignore the viewport meta; there the browser's zoom
// (Ctrl + / Ctrl −) does the same, and the slider says so.

const KEY = "klangwelt.uiScale";
const REST = "viewport-fit=cover, interactive-widget=resizes-content";
/** Sizes the slider offers (uiprefs.ts SCALE_MIN … SCALE_MAX). */
const LOWEST = 0.4;
const HIGHEST = 2;
/** The narrowest layout the toolbar and the dock are made for: a larger size doesn't go
 *  further on this device (a phone held upright), whatever is stored. Same in boot.ts. */
const MIN_LAYOUT = 300;

const stored = (): number => {
  try {
    const v = Number(localStorage.getItem(KEY) ?? "1");
    return v >= LOWEST && v <= HIGHEST ? v : 1;
  } catch {
    return 1;
  }
};

let current = stored();
/** Does this browser follow the viewport meta? Known once a size other than 100 % was applied (false: never again on this page). */
let follows: boolean | null = null;

/** Width of the window in device-independent pixels – the same at any page zoom (rounded:
 *  the page scale is a float, 975 × 0.40007 must still be 390). */
function deviceWidth(): number {
  const vv = window.visualViewport;
  return Math.round(vv ? vv.width * vv.scale : window.innerWidth);
}

/** The largest size that still leaves a layout of MIN_LAYOUT px here (in steps of 5 %, at least 100 %). */
export function webUiScaleMax(): number {
  return Math.min(HIGHEST, Math.max(1, Math.floor((deviceWidth() / MIN_LAYOUT) * 20 + 1e-9) / 20));
}

/** The size the page is laid out at for a chosen one. */
const effective = (scale: number): number => Math.min(scale, webUiScaleMax());

function apply(scale: number): void {
  const s = effective(scale);
  const meta = document.querySelector('meta[name="viewport"]');
  meta?.setAttribute(
    "content",
    s === 1 ? `width=device-width, initial-scale=1, ${REST}` : `width=${(deviceWidth() / s).toFixed(2)}, initial-scale=${s}, ${REST}`,
  );
}

/** Is the page laid out at `scale` right now? */
const laidOutAt = (scale: number): boolean => Math.abs(window.innerWidth - deviceWidth() / effective(scale)) <= 2;

export function webUiScale(): number {
  return current;
}

/** Can the interface size change here? Phones and tablets: yes; a desktop browser: no (its own zoom does it). */
export function webUiScaleWorks(): boolean {
  if (follows === false) return false;
  if (current !== 1) return laidOutAt(current);
  return matchMedia("(pointer: coarse)").matches && navigator.maxTouchPoints > 0;
}

/** Sets the size and answers the one that applies: 1 where the browser ignores the viewport meta. */
export async function setWebUiScale(scale: number): Promise<number> {
  const s = Math.min(HIGHEST, Math.max(LOWEST, Math.round(scale * 100) / 100));
  apply(s);
  if (effective(s) !== 1) {
    // the browser lays the page out anew within a frame or two
    for (let i = 0; i < 20 && !laidOutAt(s); i++) await new Promise((r) => setTimeout(r, 16));
    follows = laidOutAt(s);
  }
  current = follows === false ? 1 : s;
  if (current !== s) apply(1);
  try {
    if (current === 1) localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, String(current));
  } catch {
    // storage blocked: the size lasts until the page is closed
  }
  return current;
}

// phone turned, split screen: the layout width follows the new device width
window.addEventListener("resize", () => {
  if (current !== 1 && !laidOutAt(current)) apply(current);
});
