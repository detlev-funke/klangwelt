// Projekte speichern, öffnen, neu anlegen – Knopf „Speichern“ (mit Punkt bei
// ungespeicherten Änderungen) und Menü „Datei“. Die Dialoge sind eigene
// (keine Systemdialoge): Projekte liegen in Dokumente/Klangwelt/Projekte,
// andere Orte gehen über einen Pfad.

import { backend } from "../bridge.js";
import { loadDemo } from "../actions.js";
import { exportDialog } from "./export.js";
import { endTour, tourRunning } from "./tour.js";
import { dropdown, h, icon, modal } from "../dom.js";
import { language, t, tk } from "../i18n.js";
import { effect } from "../signals.js";
import { errorText, notify, snapshot } from "../store.js";
import type { ProjectFile } from "../types.js";

/** The core's name for a new project (an identifier – German for historical reasons). */
export const DEFAULT_PROJECT = "Neues Projekt";

/** Project name for display: the default name in the UI language. */
export function projectLabel(name: string): string {
  return name === DEFAULT_PROJECT ? t("New project") : name;
}

const fileName = (path: string | null | undefined) => (path ? path.split(/[\\/]/).pop()!.replace(/\.klangwelt$/i, "") : null);

function stopKeys(input: HTMLInputElement): HTMLInputElement {
  input.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== "Escape") e.stopPropagation();
  });
  return input;
}

/** Speichern; ohne Datei geht es zu „Speichern unter“. Liefert true bei Erfolg. */
export async function saveProject(): Promise<boolean> {
  if (!snapshot.peek()?.file) return saveAs();
  try {
    snapshot.value = await backend.projectSave(null);
    notify(t("Saved: {name}", { name: fileName(snapshot.peek()?.file) ?? "" }));
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

/** „Speichern unter“: Name (oder voller Pfad) im eigenen Dialog. */
export async function saveAs(): Promise<boolean> {
  const snap = snapshot.peek();
  let dir = "";
  let existing: ProjectFile[] = [];
  try {
    [dir, existing] = await backend.projectList();
  } catch {
    /* Liste ist nur ein Hinweis */
  }
  const name = stopKeys(h("input", {
    class: "lib-input modal-input",
    value: fileName(snap?.file) ?? (snap?.project.name && snap.project.name !== DEFAULT_PROJECT ? snap.project.name : ""),
    placeholder: t("Project name (or full path)"),
  }));
  const hint = h("div", { class: "modal-hint" });
  const update = () => {
    const v = name.value.trim();
    const clash = existing.find((p) => p.name.toLowerCase() === v.toLowerCase());
    hint.textContent = /[\\/:]/.test(v)
      ? t("Will be saved to this path.")
      : clash
        ? t("\"{name}\" already exists – it will be overwritten.", { name: clash.name })
        : t("Will be saved in {dir}.", { dir: dir || t("Documents/Klangwelt/Projekte") });
    hint.classList.toggle("warn", !!clash);
  };
  name.addEventListener("input", update);
  update();
  setTimeout(() => name.select(), 0);
  const choice = await modal(t("Save project"), [name, hint], [
    { label: t("Save"), value: "save", kind: "blue" },
    { label: t("Cancel"), value: "cancel", kind: "ghost" },
  ], name);
  if (choice !== "save") return false;
  const target = name.value.trim();
  if (!target) {
    notify(t("Please enter a name."), "error");
    return saveAs();
  }
  try {
    snapshot.value = await backend.projectSave(target);
    notify(t("Saved: {name}", { name: fileName(snapshot.peek()?.file) ?? "" }));
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

/** Question per occasion – whole sentences, so they can be translated.
 *  The table keys are internal ids (not shown, not translated). */
const DISCARD_QUESTION = {
  new: tk("\"{name}\" has unsaved changes. Save before creating a new project?"),
  demo: tk("\"{name}\" has unsaved changes. Save before loading the demo?"),
  open: tk("\"{name}\" has unsaved changes. Save before opening?"),
  close: tk("\"{name}\" has unsaved changes. Save before closing?"),
};

/**
 * Bei ungespeicherten Änderungen nachfragen. Liefert true, wenn es weitergehen
 * darf (gespeichert oder verworfen).
 */
export async function confirmDiscard(action: keyof typeof DISCARD_QUESTION): Promise<boolean> {
  const snap = snapshot.peek();
  if (!snap?.dirty) return true;
  const name = fileName(snap.file) ?? projectLabel(snap.project.name);
  const choice = await modal(t("Unsaved changes"), [
    h("p", {}, t(DISCARD_QUESTION[action], { name })),
  ], [
    { label: t("Save"), value: "save", kind: "blue" },
    { label: t("Don't save"), value: "discard", kind: "danger" },
    { label: t("Cancel"), value: "cancel", kind: "ghost" },
  ]);
  if (choice === "save") return saveProject();
  return choice === "discard";
}

export async function newProject(): Promise<void> {
  if (!(await confirmDiscard("new"))) return;
  try {
    snapshot.value = await backend.projectNew();
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** Neues Projekt mit den Demo-Spuren (Beat, Arpeggio, Melodie). */
async function demoProject(): Promise<void> {
  if (!(await confirmDiscard("demo"))) return;
  try {
    snapshot.value = await backend.projectNew();
  } catch (e) {
    notify(errorText(e), "error");
    return;
  }
  await loadDemo();
}

/** Projekt öffnen – fragt bei ungespeicherten Änderungen (auch für Doppelklick auf eine Datei). */
export async function openPath(path: string): Promise<void> {
  if (!(await confirmDiscard("open"))) return;
  try {
    snapshot.value = await backend.projectOpen(path);
    notify(t("Opened: {name}", { name: fileName(snapshot.peek()?.file) ?? "" }));
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** Projekt öffnen: Liste aus dem Projektordner, oder ein Pfad. */
export async function openDialog(): Promise<void> {
  let dir = "";
  let files: ProjectFile[] = [];
  try {
    [dir, files] = await backend.projectList();
  } catch (e) {
    notify(errorText(e), "error");
    return;
  }
  let picked: string | null = null;
  const fmtDate = (ms: number) => new Date(ms).toLocaleString(language(), { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit" });
  const size = (b: number) => (b > 1e6 ? `${(b / 1e6).toLocaleString(language(), { maximumFractionDigits: 1 })} MB` : `${Math.max(1, Math.round(b / 1e3))} kB`);
  const list = h("div", { class: "proj-list" },
    ...(files.length
      ? files.map((f) => {
          const row = h("button", { class: "proj-row", title: f.path }, icon("clip"), h("span", { class: "name" }, f.name), h("span", { class: "meta" }, `${fmtDate(f.modified)} · ${size(f.bytes)}`));
          row.addEventListener("click", () => {
            picked = f.path;
            for (const r of list.children) r.classList.toggle("on", r === row);
            path.value = "";
          });
          row.addEventListener("dblclick", () => {
            picked = f.path;
            (document.querySelector(".modal-actions .btn") as HTMLButtonElement | null)?.click();
          });
          return row;
        })
      : [h("div", { class: "lib-empty" }, t("No projects in {dir} yet.", { dir }))]));
  const path = stopKeys(h("input", { class: "lib-input mono modal-input", placeholder: t("… or path to a .klangwelt file") }));
  const choice = await modal(t("Open project"), [h("div", { class: "modal-hint" }, dir), list, path], [
    { label: t("Open"), value: "open", kind: "blue" },
    { label: t("Cancel"), value: "cancel", kind: "ghost" },
  ]);
  const target = path.value.trim() || picked;
  if (choice !== "open") return;
  if (!target) {
    notify(t("Please select a project."), "error");
    return;
  }
  await openPath(target);
}

/** Browser: gespeichertes Projekt als .klangwelt-Datei herunterladen (vorher speichern). */
async function downloadProject(): Promise<void> {
  if (!(await saveProject())) return;
  const file = snapshot.peek()?.file;
  if (file) await backend.exportReveal(file).catch((e) => notify(errorText(e), "error"));
}

/** Browser: .klangwelt-Datei auswählen, in den Speicher des Browsers legen und öffnen. */
function uploadProject(): void {
  const input = h("input", { type: "file", accept: ".klangwelt" }) as HTMLInputElement;
  input.addEventListener("change", () => {
    const f = input.files?.[0];
    if (!f || !window.__klangweltWeb) return;
    void window.__klangweltWeb.importFile(f).then((path) => openPath(path)).catch((e) => notify(errorText(e), "error"));
  });
  input.click();
}

/** Knopf „Speichern“ + Menü „Datei“ für die Transportleiste. */
export function fileControls(): HTMLElement {
  const saveBtn = h("button", { class: "btn save-btn", onclick: () => void saveProject() }, icon("save"), t("Save"), h("i", { class: "dirty-dot" }));
  const menuBtn = h("button", { class: "btn", "data-help": "datei", title: t("New, Open, Save as, Export") }, icon("folder"), `${t("File")} ▾`);
  const menu = dropdown(menuBtn, "file-menu", (m, close) => {
    const item = (label: string, keys: string, fn: () => void) =>
      h("button", { class: "menu-item", onclick: () => { close(); fn(); } }, label, h("span", {}, keys));
    m.replaceChildren(
      item(t("New project"), t("Ctrl+N"), () => void newProject()),
      item(t("Open…"), t("Ctrl+O"), () => void openDialog()),
      h("div", { class: "menu-sep" }),
      item(t("Save"), t("Ctrl+S"), () => void saveProject()),
      item(t("Save as…"), t("Ctrl+Shift+S"), () => void saveAs()),
      item(t("Export…"), t("Ctrl+E"), () => void exportDialog()),
      // Browser: Projekte liegen im Speicher des Browsers – als Datei mitnehmen bzw. hereinholen
      ...(backend.platform === "web"
        ? [
            h("div", { class: "menu-sep" }),
            item(t("Download project file"), "", () => void downloadProject()),
            item(t("Upload project file…"), "", () => uploadProject()),
          ]
        : []),
      h("div", { class: "menu-sep" }),
      item(t("Load demo project"), "", () => void demoProject()),
    );
  });
  effect(() => {
    const s = snapshot.value;
    const name = fileName(s?.file);
    saveBtn.classList.toggle("dirty", !!s?.dirty);
    saveBtn.title = name
      ? s?.dirty
        ? t("{name} – unsaved changes (Ctrl+S)", { name })
        : t("{name} – saved (Ctrl+S)", { name })
      : t("Save project (Ctrl+S)");
  });
  const exportBtn = h("button", { class: "btn export-btn", title: t("Export as WAV, FLAC, Opus, stems or MIDI (Ctrl+E)"), onclick: () => void exportDialog() }, icon("download"), t("Export"));
  return h("div", { class: "file-controls" }, menu, saveBtn, exportBtn);
}

/** Tastenkürzel; gibt true zurück, wenn die Taste verarbeitet wurde. */
export function fileShortcut(e: KeyboardEvent): boolean {
  if (!(e.ctrlKey || e.metaKey)) return false;
  const k = e.key.toLowerCase();
  if (k === "s") {
    e.preventDefault();
    void (e.shiftKey ? saveAs() : saveProject());
    return true;
  }
  if (k === "o") {
    e.preventDefault();
    void openDialog();
    return true;
  }
  if (k === "n") {
    e.preventDefault();
    void newProject();
    return true;
  }
  if (k === "e" && !e.shiftKey) {
    e.preventDefault();
    void exportDialog();
    return true;
  }
  return false;
}

/** Fenster schließen mit ungespeicherten Änderungen. */
export function handleCloseRequests(): void {
  backend.onCloseRequested(async (info) => {
    // Intro-Tour läuft: erst beenden – sie nimmt die Demo zurück, danach ist das
    // Projekt meist wieder unverändert, und ein Dialog läge nicht unter der Tour
    if (tourRunning()) {
      await endTour();
      info = { ...info, dirty: !!snapshot.peek()?.dirty };
    }
    if (info.busy) {
      const choice = await modal(t("Library is still being processed"), [
        h("p", {}, `${info.busy}.`),
        h("p", { class: "modal-hint" }, t("Next time you start the app, it will pick up from here automatically – close anyway?")),
      ], [
        { label: t("Keep running"), value: "stay", kind: "blue" },
        { label: t("Close anyway"), value: "close", kind: "danger" },
      ]);
      if (choice !== "close") return;
    }
    if (!info.dirty || (await confirmDiscard("close"))) await backend.appClose();
  });
}
