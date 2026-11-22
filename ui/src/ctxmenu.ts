// Context menu (right click): a list of entries at the mouse position.
// Entries are buttons, subheadings, separators or rows with small
// option buttons ("chips", e.g. the grid 1/4 · 1/8 · 1/16). A click
// outside, Esc, mouse wheel or window switch closes the menu.

import { h } from "./dom.js";

export interface MenuChip {
  label: string;
  title?: string;
  /** Identifier for tests and help (data-act). */
  act?: string;
  /** Shown as chosen (e.g. the current value). */
  on?: boolean;
  disabled?: boolean;
  action: () => void;
}

export interface MenuItem {
  label: string;
  /** On the right, e.g. the key ("Delete"). */
  hint?: string;
  title?: string;
  /** Identifier for tests and help (data-act). */
  act?: string;
  action?: () => void;
  disabled?: boolean;
  /** A switch: shown with a check mark when on. */
  checked?: boolean;
  /** Option buttons in the row instead of an action of its own. */
  chips?: MenuChip[];
}

export type MenuEntry = MenuItem | "sep" | { heading: string };

let open: { el: HTMLElement; close: () => void } | null = null;

export function closeContextMenu(): void {
  open?.close();
}

/** Opens a menu at the position (window coordinates); an open one closes first. */
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
        const b = h("button", { class: c.on ? "ctx-chip on" : "ctx-chip", role: c.on === undefined ? "menuitem" : "menuitemradio", title: c.title ?? "" }, c.label);
        if (c.act) b.dataset.act = c.act;
        if (c.on !== undefined) b.setAttribute("aria-checked", String(c.on));
        if (c.disabled) b.disabled = true;
        b.addEventListener("click", () => run(c.action));
        chips.append(b);
      }
      row.append(chips);
      if (e.act) row.dataset.act = e.act;
      el.append(row);
    } else {
      const check = e.checked === undefined ? "" : h("i", { class: "ctx-check" }, e.checked ? "✓" : "");
      const b = h("button", { class: "menu-item", role: e.checked === undefined ? "menuitem" : "menuitemcheckbox", title: e.title ?? "" }, check, h("b", {}, e.label), e.hint ? h("span", {}, e.hint) : "");
      if (e.checked !== undefined) b.setAttribute("aria-checked", String(e.checked));
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
  // keep within the window
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
