// Einstiegspunkt der Oberfläche.

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
import { agentPulse, chat, chatUnread, errorText, instrumentSpecs, libraryBusy, notify, savedSounds, snapshot, status, visiblePanels } from "./store.js";

// Fenster (Spuren, Clip, Analyse, Klang, Bibliothek, Agent) im Docksystem.
initLayout(document.getElementById("dock")!);
initHelp();
// zuletzt benutzte Bibliothek gleich laden – nicht erst, wenn das Fenster „Bibliothek“ aufgeht
initLibrary();
// Audiodateien aufs Fenster ziehen: werden Spuren
initFileDrop();

function isTyping(e: KeyboardEvent): boolean {
  const t = e.target as HTMLElement | null;
  return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
}

// F1: Hilfe zum Element unter der Maus festhalten – vor allen anderen
// (Capture), damit es auch in Feldern wirkt, die Tasten selbst abfangen
window.addEventListener("keydown", (e) => {
  if (e.key !== "F1") return;
  e.preventDefault();
  e.stopPropagation();
  helpKey();
}, true);

window.addEventListener("keydown", (e) => {
  // Speichern/Öffnen/Neu auch beim Tippen in einem Feld
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
    // markierte Noten, sonst ein zuletzt markierter Zeitbereich (aus allen Spuren), sonst Clip/Spur
    deleteSelection();
  } else if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !e.altKey && !mod && noteSel.peek()?.keys.length) {
    // markierte Noten in Halbtönen (Umschalt: Oktave) verschieben
    e.preventDefault();
    transposeSelectedNotes((e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 12 : 1));
  } else if (e.key === "Escape") {
    noteSel.value = null;
  } else if (e.key === "Home" || e.key === "Enter") {
    void stopToStart();
  }
});

async function boot(): Promise<void> {
  try {
    snapshot.value = await backend.getState();
    // gemerktes Audiogerät fehlt: Klangwelt nimmt den Systemstandard – kurz Bescheid sagen
    const e = snapshot.peek()?.engine;
    const missing = [e?.outputNotice, e?.inputNotice].filter(Boolean).join(" ");
    if (missing) notify(missing);
  } catch (e) {
    notify(`Verbindung zum Kern fehlgeschlagen: ${errorText(e)}`, "error");
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
      notify(`Agent: ${last.length > 90 ? last.slice(0, 90) + " …" : last}`, "agent");
    }
  });
  backend.onStatus((s) => (status.value = s));
  handleCloseRequests();
  // Bibliotheksarbeit im Hintergrund (auch fortgesetzt nach einem Neustart): Beginn und Ende melden
  backend.onLibraryChanged((i) => {
    const busy = !!(i.building || i.download);
    if (busy !== libraryBusy.peek() && i.message) notify(`Bibliothek: ${i.message}`);
    libraryBusy.value = busy;
  });
  void backend.libraryInfo().then((i) => {
    libraryBusy.value = !!(i.building || i.download);
    if (i.message && /unterbrochen|fertiggestellt/.test(i.message)) notify(`Bibliothek: ${i.message}`);
  }).catch(() => {});
  backend.onProjectChanged((s) => (snapshot.value = s));
  // Doppelklick auf eine .klangwelt-Datei, während Klangwelt schon läuft (oder Agent)
  backend.onOpenFile((path) => void openPath(path));
  backend.onAgentActivity((a) => {
    agentPulse.update((n) => n + 1);
    notify(`Agent: ${a.summary}`, "agent");
  });
  if (!backend.native) notify("Browser-Vorschau: Oberfläche ohne Audio. Die volle App startet mit cargo run.");
  if (backend.platform === "web") webNotice();
}

void boot();

/** Browser-Version: einmal erklären, was anders ist als in der Desktop-App. */
function webNotice(): void {
  const key = "klangwelt.web.hinweis";
  try {
    if (localStorage.getItem(key)) return;
    localStorage.setItem(key, "1");
  } catch {
    /* ohne Speicher: jedes Mal */
  }
  notify("Klangwelt im Browser: Projekte bleiben in diesem Browser gespeichert, Exporte werden heruntergeladen. Ohne Agent und Bibliotheks-Downloads – Einzelheiten in docs/WEB.md.");
}
