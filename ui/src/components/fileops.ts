// Projekte speichern, öffnen, neu anlegen – Knopf „Speichern“ (mit Punkt bei
// ungespeicherten Änderungen) und Menü „Datei“. Die Dialoge sind eigene
// (keine Systemdialoge): Projekte liegen in Dokumente/Klangwelt/Projekte,
// andere Orte gehen über einen Pfad.

import { backend } from "../bridge.js";
import { loadDemo } from "../actions.js";
import { exportDialog } from "./export.js";
import { endTour, tourRunning } from "./tour.js";
import { dropdown, h, icon, modal } from "../dom.js";
import { effect } from "../signals.js";
import { errorText, notify, snapshot } from "../store.js";
import type { ProjectFile } from "../types.js";

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
    notify(`Gespeichert: ${fileName(snapshot.peek()?.file)}`);
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
    value: fileName(snap?.file) ?? (snap?.project.name && snap.project.name !== "Neues Projekt" ? snap.project.name : ""),
    placeholder: "Name des Projekts (oder vollständiger Pfad)",
  }));
  const hint = h("div", { class: "modal-hint" });
  const update = () => {
    const v = name.value.trim();
    const clash = existing.find((p) => p.name.toLowerCase() === v.toLowerCase());
    hint.textContent = /[\\/:]/.test(v)
      ? "Wird unter diesem Pfad gespeichert."
      : clash
        ? `„${clash.name}“ gibt es schon – wird überschrieben.`
        : `Wird in ${dir || "Dokumente/Klangwelt/Projekte"} gespeichert.`;
    hint.classList.toggle("warn", !!clash);
  };
  name.addEventListener("input", update);
  update();
  setTimeout(() => name.select(), 0);
  const choice = await modal("Projekt speichern", [name, hint], [
    { label: "Speichern", value: "save", kind: "blue" },
    { label: "Abbrechen", value: "cancel", kind: "ghost" },
  ], name);
  if (choice !== "save") return false;
  const target = name.value.trim();
  if (!target) {
    notify("Bitte einen Namen eingeben.", "error");
    return saveAs();
  }
  try {
    snapshot.value = await backend.projectSave(target);
    notify(`Gespeichert: ${fileName(snapshot.peek()?.file)}`);
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

/**
 * Bei ungespeicherten Änderungen nachfragen. Liefert true, wenn es weitergehen
 * darf (gespeichert oder verworfen).
 */
export async function confirmDiscard(action: string): Promise<boolean> {
  const snap = snapshot.peek();
  if (!snap?.dirty) return true;
  const name = fileName(snap.file) ?? snap.project.name;
  const choice = await modal("Ungespeicherte Änderungen", [
    h("p", {}, `„${name}“ hat ungespeicherte Änderungen. Vor dem ${action} speichern?`),
  ], [
    { label: "Speichern", value: "save", kind: "blue" },
    { label: "Nicht speichern", value: "discard", kind: "danger" },
    { label: "Abbrechen", value: "cancel", kind: "ghost" },
  ]);
  if (choice === "save") return saveProject();
  return choice === "discard";
}

export async function newProject(): Promise<void> {
  if (!(await confirmDiscard("Neuanlegen"))) return;
  try {
    snapshot.value = await backend.projectNew();
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** Neues Projekt mit den Demo-Spuren (Beat, Arpeggio, Melodie). */
async function demoProject(): Promise<void> {
  if (!(await confirmDiscard("Laden der Demo"))) return;
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
  if (!(await confirmDiscard("Öffnen"))) return;
  try {
    snapshot.value = await backend.projectOpen(path);
    notify(`Geöffnet: ${fileName(snapshot.peek()?.file)}`);
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
  const fmtDate = (ms: number) => new Date(ms).toLocaleString("de-DE", { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit" });
  const size = (b: number) => (b > 1e6 ? `${(b / 1e6).toLocaleString("de-DE", { maximumFractionDigits: 1 })} MB` : `${Math.max(1, Math.round(b / 1e3))} kB`);
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
      : [h("div", { class: "lib-empty" }, `Noch keine Projekte in ${dir}.`)]));
  const path = stopKeys(h("input", { class: "lib-input mono modal-input", placeholder: "… oder Pfad zu einer .klangwelt-Datei" }));
  const choice = await modal("Projekt öffnen", [h("div", { class: "modal-hint" }, dir), list, path], [
    { label: "Öffnen", value: "open", kind: "blue" },
    { label: "Abbrechen", value: "cancel", kind: "ghost" },
  ]);
  const target = path.value.trim() || picked;
  if (choice !== "open") return;
  if (!target) {
    notify("Bitte ein Projekt auswählen.", "error");
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
  const saveBtn = h("button", { class: "btn save-btn", onclick: () => void saveProject() }, icon("save"), "Speichern", h("i", { class: "dirty-dot" }));
  const menuBtn = h("button", { class: "btn", "data-help": "datei", title: "Neu, Öffnen, Speichern unter, Exportieren" }, icon("folder"), "Datei ▾");
  const menu = dropdown(menuBtn, "file-menu", (m, close) => {
    const item = (label: string, keys: string, fn: () => void) =>
      h("button", { class: "menu-item", onclick: () => { close(); fn(); } }, label, h("span", {}, keys));
    m.replaceChildren(
      item("Neues Projekt", "Strg+N", () => void newProject()),
      item("Öffnen …", "Strg+O", () => void openDialog()),
      h("div", { class: "menu-sep" }),
      item("Speichern", "Strg+S", () => void saveProject()),
      item("Speichern unter …", "Strg+Umschalt+S", () => void saveAs()),
      item("Exportieren …", "Strg+E", () => void exportDialog()),
      // Browser: Projekte liegen im Speicher des Browsers – als Datei mitnehmen bzw. hereinholen
      ...(backend.platform === "web"
        ? [
            h("div", { class: "menu-sep" }),
            item("Projektdatei herunterladen", "", () => void downloadProject()),
            item("Projektdatei hochladen …", "", () => uploadProject()),
          ]
        : []),
      h("div", { class: "menu-sep" }),
      item("Demo-Projekt laden", "", () => void demoProject()),
    );
  });
  effect(() => {
    const s = snapshot.value;
    const name = fileName(s?.file);
    saveBtn.classList.toggle("dirty", !!s?.dirty);
    saveBtn.title = name
      ? `${name}${s?.dirty ? " – ungespeicherte Änderungen" : " – gespeichert"} (Strg+S)`
      : "Projekt speichern (Strg+S)";
  });
  const exportBtn = h("button", { class: "btn export-btn", title: "Als WAV, FLAC, Opus, Stems oder MIDI exportieren (Strg+E)", onclick: () => void exportDialog() }, icon("download"), "Exportieren");
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
      const choice = await modal("Bibliothek wird noch bearbeitet", [
        h("p", {}, `${info.busy}.`),
        h("p", { class: "modal-hint" }, "Beim nächsten Start geht es automatisch an dieser Stelle weiter – trotzdem jetzt schließen?"),
      ], [
        { label: "Weiterlaufen lassen", value: "stay", kind: "blue" },
        { label: "Trotzdem schließen", value: "close", kind: "danger" },
      ]);
      if (choice !== "close") return;
    }
    if (!info.dirty || (await confirmDiscard("Schließen"))) await backend.appClose();
  });
}
