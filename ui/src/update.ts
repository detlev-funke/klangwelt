// A new version of the browser app. The service worker serves the page from its
// cache, so the first start after an update still shows the old version while the
// new worker caches the new one in the background; when it takes over, the web
// shell says so (web/host.ts, UPDATE_READY). Then:
//
// * nothing touched yet (no click, tap, key or wheel since the page loaded,
//   nothing playing or recording, no agent at work, no window open): reload right
//   away – the open project comes back as after a language switch (autosave.ts),
//   and the What's new note follows the new version,
// * otherwise a note with "Reload": the work isn't interrupted, and the next start
//   is the new version anyway.
//
// The desktop app updates as a whole (a new exe) – nothing to do there. The same
// holds for the installed app on a phone: it is this page with this worker.

import { reloadKeepingProject } from "./autosave.js";
import { backend } from "./bridge.js";
import { t } from "./i18n.js";
import { chat, notify, status } from "./store.js";

/** Same name as UPDATE_READY in web/host.ts (not imported: that is the browser's shell). */
const UPDATE_READY = "klangwelt:update-ready";
/** sessionStorage: time of the last reload for an update – never two in a row. */
const RELOADED_KEY = "klangwelt.updateReloaded";
const AGAIN_MS = 60_000;

let touched = false;
let handled = false;

/** Something on the page is in use: reloading now would interrupt it. */
function busy(): boolean {
  const s = status.peek();
  const c = chat.peek();
  return touched || s.playing || s.recording || !!s.previewing || c.agentWorking || c.agentListening || !!document.querySelector(".modal");
}

function reloadedJustNow(): boolean {
  try {
    return Date.now() - Number(sessionStorage.getItem(RELOADED_KEY) ?? 0) < AGAIN_MS;
  } catch {
    return false;
  }
}

function onUpdate(): void {
  if (handled) return;
  handled = true;
  if (!busy() && !reloadedJustNow()) {
    try {
      sessionStorage.setItem(RELOADED_KEY, String(Date.now()));
    } catch {
      /* without storage the guard is this page's `handled` */
    }
    void reloadKeepingProject();
    return;
  }
  notify(t("A new version of Klangwelt is ready."), "info", { label: t("Reload"), run: () => void reloadKeepingProject() });
}

/** At start (browser only): watch for a newer version taking over. */
export function watchForUpdates(): void {
  if (backend.platform === "desktop") return;
  const touch = () => {
    touched = true;
  };
  for (const type of ["pointerdown", "keydown", "wheel"]) window.addEventListener(type, touch, { capture: true, passive: true, once: true });
  window.addEventListener(UPDATE_READY, onUpdate);
  // it may have happened before the app was up
  if ("updateReady" in document.documentElement.dataset) onUpdate();
}
