// Kontextmenü (Rechtsklick): eine Liste von Einträgen an der Mausposition.
// Einträge sind Knöpfe, Zwischentitel, Trennlinien oder Zeilen mit kleinen
// Wahlknöpfen („Chips“, z. B. das Raster 1/4 · 1/8 · 1/16). Ein Klick
// außerhalb, Esc, Mausrad oder Fensterwechsel schließt das Menü.

import { h } from "./dom.js";

export interface MenuChip {
  label: string;
  title?: string;
  /** Kennung für Tests und Hilfe (data-act). */
  act?: string;
  action: () => void;
}

export interface MenuItem {
  label: string;
  /** Rechts daneben, z. B. die Taste („Entf“). */
  hint?: string;
  title?: string;
  /** Kennung für Tests und Hilfe (data-act). */
  act?: string;
  action?: () => void;
  disabled?: boolean;
  /** Wahlknöpfe in der Zeile statt einer eigenen Aktion. */
  chips?: MenuChip[];
}

export type MenuEntry = MenuItem | "sep" | { heading: string };

let open: { el: HTMLElement; close: () => void } | null = null;

export function closeContextMenu(): void {
  open?.close();
}

/** Menü an der Stelle (Fensterkoordinaten) öffnen; ein offenes schließt sich vorher. */
export function openContextMenu(x: number, y: number, entries: MenuEntry[]): HTMLElement {
  closeContextMenu();
  const el = h("div", { class: "menu ctx-menu open", role: "menu" });
  const run = (fn: () => void) => {
    close();
    fn();
  };
  for (const e of entries) {
    if (e === "sep") {
      el.append(h("div", { class: "menu-sep" }));
    } else if ("heading" in e) {
      el.append(h("div", { class: "menu-title" }, e.heading));
    } else if (e.chips) {
      const row = h("div", { class: "ctx-row", title: e.title ?? "" }, h("span", { class: "ctx-label" }, e.label));
      const chips = h("div", { class: "ctx-chips" });
      for (const c of e.chips) {
        const b = h("button", { class: "ctx-chip", role: "menuitem", title: c.title ?? "" }, c.label);
        if (c.act) b.dataset.act = c.act;
        b.addEventListener("click", () => run(c.action));
        chips.append(b);
      }
      row.append(chips);
      if (e.act) row.dataset.act = e.act;
      el.append(row);
    } else {
      const b = h("button", { class: "menu-item", role: "menuitem", title: e.title ?? "" }, h("b", {}, e.label), e.hint ? h("span", {}, e.hint) : "");
      if (e.act) b.dataset.act = e.act;
      if (e.disabled) b.disabled = true;
      const action = e.action;
      if (action) b.addEventListener("click", () => run(action));
      el.append(b);
    }
  }
  const outside = (ev: Event) => {
    if (!el.contains(ev.target as Node)) close();
  };
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key !== "Escape") return;
    ev.preventDefault();
    ev.stopPropagation();
    close();
  };
  const close = () => {
    if (open?.el !== el) return;
    open = null;
    el.remove();
    window.removeEventListener("pointerdown", outside, true);
    window.removeEventListener("wheel", outside, true);
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("blur", close);
    window.removeEventListener("resize", close);
  };
  open = { el, close };
  document.body.append(el);
  // im Fenster halten
  const r = el.getBoundingClientRect();
  el.style.left = `${Math.max(4, Math.min(x, window.innerWidth - r.width - 4))}px`;
  el.style.top = `${Math.max(4, Math.min(y, window.innerHeight - r.height - 4))}px`;
  window.addEventListener("pointerdown", outside, true);
  window.addEventListener("wheel", outside, true);
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("blur", close);
  window.addEventListener("resize", close);
  return el;
}
