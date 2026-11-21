// Saving in the background, and keeping the open project through a reload of
// the UI (switching the language).
//
// Every `autosaveSeconds` (gear menu; one minute by default, 0 = off) the core
// saves the open project if it changed (`project_autosave`):
// * a project with a file into that file – the same atomic write as "Save";
//   earlier states stay reachable through the change history,
// * a project without a file into a recovery copy in the app's own folder: it
//   stays "unsaved" and comes back at the next start after a crash or a
//   closed browser tab.
// It waits while playing or recording (and during the intro tour) and saves
// right after. With autosave on, a project with a file is also saved without
// asking before closing, opening or creating another one (`confirmDiscard`).
//
// Switching the language reloads the UI (every module translates while it
// loads). Before that the project is saved the same way – with autosave off, a
// project with a file goes into the recovery copy as well and its file stays
// untouched – and afterwards it is opened again: the browser's core starts
// anew with the page, the desktop's core keeps running (its project is still
// open, undo included).
//
// Tests set any number of seconds with `setAutosaveSeconds` (the menu offers
// whole minutes).

import { backend } from "./bridge.js";
import { h, modal } from "./dom.js";
import { switchLanguage, t } from "./i18n.js";
import { signal } from "./signals.js";
import { errorText, notify, snapshot, status } from "./store.js";
import { tourRunning } from "./components/tour.js";
import type { AutosaveResult } from "./types.js";

/** localStorage: seconds between autosaves ("0" = off). */
export const AUTOSAVE_KEY = "klangwelt.autosave";
export const AUTOSAVE_DEFAULT = 60;
/** Choices in the gear menu (seconds; 0 = off). */
export const AUTOSAVE_CHOICES: readonly number[] = [0, 60, 120, 300, 600];
/** sessionStorage: a reload that should open the same project again (survives the reload only). */
export const RESUME_KEY = "klangwelt.resume";

function storedSeconds(): number {
  try {
    const v = localStorage.getItem(AUTOSAVE_KEY);
    if (v != null && /^\d+(\.\d+)?$/.test(v.trim())) return Number(v);
  } catch {
    /* no storage: the default */
  }
  return AUTOSAVE_DEFAULT;
}

/** Seconds between autosaves; 0 = off. */
export const autosaveSeconds = signal(storedSeconds());

export function autosaveOn(): boolean {
  return autosaveSeconds.peek() > 0;
}

/** Choose the interval (remembered; 0 = off) – the next autosave is that far away. */
export function setAutosaveSeconds(seconds: number): void {
  const s = Number.isFinite(seconds) && seconds > 0 ? seconds : 0;
  autosaveSeconds.value = s;
  try {
    localStorage.setItem(AUTOSAVE_KEY, String(s));
  } catch {
    /* without storage: for this session */
  }
  schedule();
}

let interval: number | undefined;
let retry: number | undefined;
/** An autosave is due but waits (playing, recording, tour, another save running). */
let due = false;
let running: Promise<AutosaveResult | null> | null = null;
/** Last error shown – the same one isn't repeated every minute. */
let lastError: string | null = null;

function schedule(): void {
  clearInterval(interval);
  clearTimeout(retry);
  interval = undefined;
  due = false;
  const s = autosaveSeconds.peek();
  if (s > 0) {
    interval = window.setInterval(() => {
      due = true;
      void tick();
    }, s * 1000);
  }
}

/** Playback or a recording must not be disturbed; the tour changes the project and takes it back. */
function busy(): boolean {
  const s = status.peek();
  return !!(s.playing || s.recording || tourRunning() || running);
}

async function tick(): Promise<void> {
  clearTimeout(retry);
  if (!due) return;
  if (busy()) {
    retry = window.setTimeout(() => void tick(), 1000);
    return;
  }
  due = false;
  await autosaveNow();
}

/** Start the timer (once, at start). */
export function initAutosave(): void {
  schedule();
}

/**
 * Save now as the autosave does (`recoveryOnly`: into the recovery copy even
 * with a file). Returns what happened, null on an error (shown once).
 */
export async function autosaveNow(recoveryOnly = false): Promise<AutosaveResult | null> {
  while (running) await running.catch(() => null);
  const before = snapshot.peek();
  if (!before) return null;
  const job = (async () => {
    try {
      const r = await backend.projectAutosave(recoveryOnly);
      lastError = null;
      // only if no other answer came in meanwhile – that one is newer than this
      if (snapshot.peek() === before && (r.dirty !== before.dirty || r.file !== before.file)) snapshot.value = { ...before, dirty: r.dirty, file: r.file };
      return r;
    } catch (e) {
      const text = errorText(e);
      if (text !== lastError) notify(t("Autosave failed: {error}", { error: text }), "error");
      lastError = text;
      return null;
    }
  })();
  running = job;
  try {
    return await job;
  } finally {
    running = null;
  }
}

/**
 * Switch the language: save the project first (see above), remember to open
 * it again, then reload. If saving fails, ask before changes could be lost.
 */
export async function switchLanguageKeepingProject(code: string): Promise<void> {
  const snap = snapshot.peek();
  const result = await autosaveNow(!autosaveOn());
  if (!result && snap?.dirty) {
    const choice = await modal(t("Unsaved changes"), [
      h("p", {}, t("The project couldn't be saved. Switch the language anyway? Unsaved changes may be lost.")),
    ], [
      { label: t("Switch anyway"), value: "switch", kind: "danger" },
      { label: t("Cancel"), value: "cancel", kind: "ghost" },
    ]);
    if (choice !== "switch") return;
  }
  try {
    await backend.syncStorage();
  } catch (e) {
    console.warn("sync:", e);
  }
  try {
    sessionStorage.setItem(RESUME_KEY, JSON.stringify({ file: snapshot.peek()?.file ?? null }));
  } catch {
    /* without storage: the recovery copy still comes back */
  }
  switchLanguage(code);
}

/**
 * At start: if the core starts empty (always in the browser, on the desktop
 * after a restart), open what was open before a language switch – the recovery
 * copy if there is one, otherwise the project's file. Without a language switch
 * a recovery copy means the app didn't close properly: it comes back with a
 * notice.
 */
export async function resumeProject(): Promise<void> {
  let resume: { file?: string | null } | null = null;
  try {
    resume = JSON.parse(sessionStorage.getItem(RESUME_KEY) ?? "null") as { file?: string | null } | null;
    sessionStorage.removeItem(RESUME_KEY);
  } catch {
    /* no storage or a broken entry: like a start */
  }
  const snap = snapshot.peek();
  // the desktop's core kept running through the reload: its project is still open
  if (!snap || snap.file || snap.dirty || snap.canUndo || snap.project.tracks.length) return;
  try {
    const recovered = await backend.projectRecover();
    if (recovered) {
      snapshot.value = recovered;
      if (!resume) notify(t("Restored the unsaved project from the last session."));
      return;
    }
    if (resume?.file) snapshot.value = await backend.projectOpen(resume.file);
  } catch (e) {
    notify(errorText(e), "error");
  }
}
