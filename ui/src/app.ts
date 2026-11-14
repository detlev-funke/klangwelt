// The app: windows, shortcuts, start. Loaded by main.ts once the language is set.

import "./components/transport.js";
import "./components/toast.js";
import { initLayout } from "./layout.js";
import { fileShortcut, handleCloseRequests, openPath } from "./components/fileops.js";
import { helpKey, initHelp } from "./components/help.js";
import { deleteSelection, redo, stopToStart, toggleLoop, togglePlay, toggleRecord, transposeSelectedNotes, undo } from "./actions.js";
import { noteSel } from "./pianoroll.js";
import { backend } from "./bridge.js";
import { initLibrary } from "./components/library.js";
import { initFileDrop } from "./filedrop.js";
import { t } from "./i18n.js";
import { agentPulse, chat, chatUnread, errorText, instrumentSpecs, libraryBusy, notify, savedSounds, snapshot, status, visiblePanels } from "./store.js";

// Windows (tracks, clip, analysis, sound, library, agent) in the dock system.
initLayout(document.getElementById("dock")!);
initHelp();
// load the most recently used library right away – not only when the "Library" window opens
initLibrary();
// Drag audio files onto the window: they become tracks
initFileDrop();

function isTyping(e: KeyboardEvent): boolean {
  const el = e.target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
}

// F1: capture help for the element under the mouse – before all others
// (capture), so it also works in fields that intercept keys themselves
window.addEventListener("keydown", (e) => {
  if (e.key !== "F1") return;
  e.preventDefault();
  e.stopPropagation();
  helpKey();
}, true);

window.addEventListener("keydown", (e) => {
  // Save/Open/New also while typing in a field
  if (fileShortcut(e)) return;
  if (isTyping(e)) return;
  const mod = e.ctrlKey || e.metaKey;
  if (e.code === "Space") {
    e.preventDefault();
    void togglePlay();
  } else if (mod && e.key.toLowerCase() === "z") {
    e.preventDefault();
    void (e.shiftKey ? redo() : undo());
  } else if (mod && e.key.toLowerCase() === "y") {
    e.preventDefault();
    void redo();
  } else if (!mod && e.key.toLowerCase() === "r") {
    void toggleRecord();
  } else if (!mod && !e.altKey && e.key.toLowerCase() === "l") {
    void toggleLoop();
  } else if (e.key === "Delete" || e.key === "Backspace") {
    // selected notes, otherwise a most recently selected time range (from all tracks), otherwise clip/track
    deleteSelection();
  } else if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !e.altKey && !mod && noteSel.peek()?.keys.length) {
    // move selected notes by semitones (Shift: octave)
    e.preventDefault();
    transposeSelectedNotes((e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 12 : 1));
  } else if (e.key === "Escape") {
    noteSel.value = null;
  } else if (e.key === "Home" || e.key === "Enter") {
    void stopToStart();
  }
});

/** Does a library message (desktop shell, translated by the core with the same
 *  dictionary) report work that was interrupted last time? Then it's shown at start.
 *  Matched against the translated templates, with the placeholders as wildcards. */
function resumedWork(message: string): boolean {
  const templates = [
    t('{name}: the download was interrupted – "Download" resumes it (scores already there are skipped).'),
    t("{name}: finishing (interrupted last time) …"),
    t("{name} (interrupted last time):"),
  ];
  const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return templates.some((tpl) => new RegExp(tpl.split(/\{\w+\}/).map(escape).join("[\\s\\S]*")).test(message));
}

async function boot(): Promise<void> {
  try {
    // (the core got the UI's language in main.ts, before the layout was built)
    snapshot.value = await backend.getState();
    // remembered audio device is missing: Klangwelt takes the system default – give a short notice
    const e = snapshot.peek()?.engine;
    const missing = [e?.outputNotice, e?.inputNotice].filter(Boolean).join(" ");
    if (missing) notify(missing);
  } catch (e) {
    notify(t("Connection to the core failed: {error}", { error: errorText(e) }), "error");
  }
  backend.instrumentSpecs().then((s) => (instrumentSpecs.value = s)).catch(() => {});
  backend.soundsList().then((s) => (savedSounds.value = s)).catch(() => (savedSounds.value = []));
  backend.onSoundsChanged((s) => (savedSounds.value = s));
  backend.chatHistory().then((c) => (chat.value = c)).catch(() => {});
  backend.onChatChanged((c) => {
    const before = new Set(chat.peek().messages.map((m) => m.id));
    const fresh = c.messages.filter((m) => m.from === "agent" && !before.has(m.id));
    chat.value = c;
    if (fresh.length && !visiblePanels.peek().has("agent")) {
      chatUnread.update((n) => n + fresh.length);
      const last = fresh[fresh.length - 1]!.text;
      notify(t("Agent: {text}", { text: last.length > 90 ? last.slice(0, 90) + " …" : last }), "agent");
    }
  });
  backend.onStatus((s) => (status.value = s));
  handleCloseRequests();
  // Library work in the background (also resumed after a restart): report start and end
  backend.onLibraryChanged((i) => {
    const busy = !!(i.building || i.download);
    if (busy !== libraryBusy.peek() && i.message) notify(t("Library: {message}", { message: i.message }));
    libraryBusy.value = busy;
  });
  void backend.libraryInfo().then((i) => {
    libraryBusy.value = !!(i.building || i.download);
    if (i.message && resumedWork(i.message)) notify(t("Library: {message}", { message: i.message }));
  }).catch(() => {});
  backend.onProjectChanged((s) => (snapshot.value = s));
  // Double-click on a .klangwelt file while Klangwelt is already running (or agent)
  backend.onOpenFile((path) => void openPath(path));
  backend.onAgentActivity((a) => {
    agentPulse.update((n) => n + 1);
    notify(t("Agent: {text}", { text: a.summary }), "agent");
  });
  if (!backend.native) notify(t("Browser preview: interface without audio. The full app starts with cargo run."));
  if (backend.platform === "web") webNotice();
}

void boot();

/** Browser version: explain once what is different from the desktop app. */
function webNotice(): void {
  const key = "klangwelt.web.hinweis";
  try {
    if (localStorage.getItem(key)) return;
    localStorage.setItem(key, "1");
  } catch {
    /* without storage: every time */
  }
  notify(t("Klangwelt in the browser: projects are stored in this browser, exports are downloaded. No agent and no library downloads – details in docs/WEB.md."));
}
