// Fenster der Klangwelt und ihr Standardlayout im Docksystem.

import "./components/tracks.js";
import "./components/timeline.js";
import "./components/inspector.js";
import "./components/help.js";
import { Dock, type Layout, type PanelDef, split, stack } from "./dock.js";
import { dropdown, h, icon } from "./dom.js";
import { effect } from "./signals.js";
import { chatUnread, libraryBusy, panelEmphasis, panelRequest, visiblePanels } from "./store.js";

function panel(tab: string): HTMLElement {
  const el = document.createElement("kw-panel");
  el.setAttribute("tab", tab);
  return el;
}

const PANELS: PanelDef[] = [
  {
    id: "spuren", title: "Spuren", icon: "wave", closable: false, minWidth: 520, minHeight: 200,
    create: () => h("main", { class: "workspace" }, document.createElement("kw-tracks"), document.createElement("kw-timeline")),
  },
  { id: "clip", title: "Clip", icon: "clip", closable: true, create: () => panel("clip") },
  { id: "analyse", title: "Analyse", icon: "chart", closable: true, create: () => panel("analyse") },
  { id: "klang", title: "Klang", icon: "dial", closable: true, create: () => panel("klang") },
  {
    id: "rhythmus", title: "Rhythmus", icon: "grid", closable: true, minWidth: 360, minHeight: 180,
    place: { near: "klang", zone: "center" }, create: () => panel("rhythmus"),
  },
  {
    id: "song", title: "Song", icon: "song", closable: true, minWidth: 420, minHeight: 200,
    place: { near: "klang", zone: "center" }, create: () => panel("song"),
  },
  { id: "bibliothek", title: "Bibliothek", icon: "book", closable: true, create: () => panel("bibliothek") },
  { id: "agent", title: "Agent", icon: "sparkle", closable: true, create: () => panel("agent") },
  {
    id: "harmonie", title: "Harmonie", icon: "fifths", closable: true, minWidth: 260, minHeight: 240,
    place: { near: "agent", zone: "top" }, create: () => panel("harmonie"),
  },
  {
    id: "fuge", title: "Fuge", icon: "fugue", closable: true, minWidth: 320, minHeight: 200,
    place: { near: "clip", zone: "center" }, create: () => panel("fuge"),
  },
  {
    id: "hilfe", title: "Hilfe", icon: "help", closable: true, minWidth: 220, minHeight: 120, startClosed: true,
    float: (w, h) => ({ w: 340, h: 260, x: Math.max(0, w - 356), y: Math.max(0, h - 350) }),
    create: () => document.createElement("kw-help"),
  },
];

/** Spuren oben, Werkzeuge darunter als Reiter; rechts Quintenzirkel und Agent-Chat. */
function defaults(): Layout {
  return {
    root: split("row", [
      split("col", [stack(["spuren"]), stack(["clip", "analyse", "klang", "rhythmus", "song", "bibliothek", "fuge"])], [0.68, 0.32]),
      split("col", [stack(["harmonie"]), stack(["agent"])], [0.6, 0.4]),
    ], [0.72, 0.28]),
    floats: [],
    closed: ["hilfe"],
    homes: {},
  };
}

let dock: Dock | null = null;

export function initLayout(host: HTMLElement): void {
  dock = new Dock(host, {
    panels: PANELS,
    defaults,
    storageKey: "klangwelt.dock.v1",
    onVisible: (ids) => (visiblePanels.value = ids),
  });
  // Außerhalb des Effekts ausführen, damit er nur von der Anfrage abhängt.
  effect(() => {
    const r = panelRequest.value;
    if (r) queueMicrotask(() => dock?.show(r.id));
  });
  // nach dem Zeigen (gleiche Reihenfolge der Mikrotasks): erst öffnen, dann vergrößern
  effect(() => {
    const e = panelEmphasis.value;
    queueMicrotask(() => dock?.emphasize(e?.id ?? null, { minWidth: e?.minWidth }));
  });
  effect(() => dock?.setBadge("agent", chatUnread.value > 0));
  effect(() => dock?.setBadge("bibliothek", libraryBusy.value));
  effect(() => {
    if (visiblePanels.value.has("agent")) chatUnread.value = 0;
  });
}

/** Knopf „Fenster ▾“: Fenster ein-/ausblenden, Layout zurücksetzen. */
export function windowMenu(): HTMLElement {
  const btn = h("button", { class: "btn", "data-help": "fenster", title: "Fenster ein-/ausblenden, Layout zurücksetzen" }, icon("layout"), "Fenster ▾");
  return dropdown(btn, "win-menu", (menu, close) => {
    if (!dock) return;
    const d = dock;
    menu.replaceChildren(
      ...d.list().map((p) =>
        h(
          "button",
          {
            class: `menu-item win${p.open ? " open" : ""}`,
            title: p.closable ? (p.open ? "Ausblenden" : "Einblenden") : "Hauptfenster – lässt sich nicht schließen",
            onclick: () => {
              if (p.closable) d.toggle(p.id);
              else d.show(p.id);
              close();
            },
          },
          h("b", { class: "check" }, p.open ? "✓" : ""),
          icon(p.icon),
          h("em", {}, p.title),
        ),
      ),
      h("div", { class: "menu-sep" }),
      h("button", { class: "menu-item", onclick: () => { d.reset(); close(); } }, h("b", { class: "check" }), icon("layout"), h("em", {}, "Layout zurücksetzen")),
      h("div", { class: "menu-hint" }, "Reiter ziehen: andocken, als Reiter gruppieren oder frei schweben lassen."),
    );
  });
}
