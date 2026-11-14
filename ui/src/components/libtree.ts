// Bibliothek stöbern: Baum mit frei wählbaren Ebenen (Genre, Komponist,
// Stimmung, Besetzung, Tonart …). Zweige werden erst beim Aufklappen geladen,
// große Listen seitenweise. Mehrwertige Ebenen (Genre, Instrumentengruppe)
// legen eine Partitur in mehrere Zweige.

import { backend } from "../bridge.js";
import { dropdown, h, icon } from "../dom.js";
import { errorText, notify } from "../store.js";
import type { Dimension, LibraryInfo, TreeLevel, TreeNode, TreeScore } from "../types.js";

const LEVELS_KEY = "klangwelt.bibliothek.ebenen";
const PAGE = 200;

const PRESETS: Array<[string, string[]]> = [
  ["Genre › Komponist", ["genre", "komponist"]],
  ["Komponist › Genre", ["komponist", "genre"]],
  ["Genre › Stimmung › Besetzung", ["genre", "stimmung", "besetzung"]],
  ["Stimmung › Besetzung › Tonart", ["stimmung", "besetzung", "tonart"]],
  ["Besetzung › Instrumentengruppe › Schwierigkeit", ["besetzung", "instrumente", "schwierigkeit"]],
  ["Dur/Moll › Tempo › Taktart", ["tongeschlecht", "tempo", "taktart"]],
  ["Färbung › Energie › Harmonik", ["faerbung", "energie", "harmonik"]],
];

function loadLevels(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(LEVELS_KEY) ?? "null");
    if (Array.isArray(v) && v.length && v.every((x) => typeof x === "string")) return v;
  } catch {
    /* Standard */
  }
  return ["genre", "komponist"];
}

interface Branch {
  open: boolean;
  loading: boolean;
  dimension: string | null;
  nodes: TreeNode[];
  scores: TreeScore[];
  total: number;
  matching: number;
}

const keyOf = (path: string[]) => path.join("\u0001");

export interface TreeView {
  /** Ebenen-Leiste (kommt in die Werkzeugzeile des Fensters). */
  bar: HTMLElement;
  /** Baum (linke Spalte). */
  el: HTMLElement;
  setInfo(info: LibraryInfo | null): void;
}

export function treeView(onOpen: (doc: number) => void): TreeView {
  let levels = loadLevels();
  let dims: Dimension[] = [];
  const branches = new Map<string, Branch>();
  let generation = 0; // verwirft Antworten nach einem Neuaufbau
  let info: LibraryInfo | null = null;
  let infoSig = "";
  let selectedDoc = -1;

  const label = (k: string) => dims.find((d) => d.key === k)?.label ?? k;
  const saveLevels = () => {
    try {
      localStorage.setItem(LEVELS_KEY, JSON.stringify(levels));
    } catch {
      /* ohne Speicher */
    }
  };

  // ---------- Ebenen-Leiste: Chips (ziehen zum Umsortieren, × entfernt), + Ebene, Vorlagen
  const chips = h("div", { class: "tree-chips" });
  const addBtn = h("button", { class: "btn small", title: "Ebene hinzufügen" }, icon("plus"), "Ebene ▾");
  const add = dropdown(addBtn, "tree-menu", (menu, close) => {
    const rest = dims.filter((d) => !levels.includes(d.key));
    menu.replaceChildren(
      ...(rest.length
        ? rest.map((d) => h("button", { class: "menu-item", onclick: () => { setLevels([...levels, d.key]); close(); } }, d.label, d.multi ? h("span", {}, "mehrfach") : null))
        : [h("div", { class: "menu-hint" }, "Alle Ebenen sind schon dabei.")]),
    );
  });
  const presetBtn = h("button", { class: "btn small", title: "Fertige Anordnungen" }, "Vorlagen ▾");
  const presets = dropdown(presetBtn, "tree-menu", (menu, close) => {
    menu.replaceChildren(...PRESETS.map(([name, lv]) => h("button", { class: "menu-item", onclick: () => { setLevels(lv); close(); } }, name)));
  });
  const bar = h("div", { class: "tree-bar" }, h("span", { class: "tree-bar-label" }, "Ebenen:"), chips, add, presets);

  let dragFrom = -1;
  const renderChips = () => {
    chips.replaceChildren(
      ...levels.map((k, i) => {
        const chip = h("span", { class: "tree-chip", draggable: "true", title: "Ziehen zum Umsortieren" },
          h("b", {}, String(i + 1)), label(k),
          h("button", { class: "tree-chip-x", title: "Ebene entfernen", onclick: () => setLevels(levels.filter((_, j) => j !== i)) }, "×"));
        chip.addEventListener("dragstart", (e) => {
          dragFrom = i;
          e.dataTransfer?.setData("text/plain", k);
          chip.classList.add("dragging");
        });
        chip.addEventListener("dragend", () => chip.classList.remove("dragging"));
        chip.addEventListener("dragover", (e) => {
          e.preventDefault();
          chip.classList.add("over");
        });
        chip.addEventListener("dragleave", () => chip.classList.remove("over"));
        chip.addEventListener("drop", (e) => {
          e.preventDefault();
          chip.classList.remove("over");
          if (dragFrom < 0 || dragFrom === i) return;
          const next = levels.slice();
          const [moved] = next.splice(dragFrom, 1);
          next.splice(i, 0, moved!);
          dragFrom = -1;
          setLevels(next);
        });
        return i < levels.length - 1 ? [chip, h("span", { class: "tree-sep" }, "›")] : [chip];
      }).flat(),
    );
  };

  // ---------- Baum
  const banner = h("div", { class: "tree-banner" });
  const list = h("div", { class: "tree-list" });
  const el = h("div", { class: "tree" }, banner, list);

  const load = async (path: string[], more = false) => {
    const k = keyOf(path);
    const b = branches.get(k) ?? { open: true, loading: false, dimension: null, nodes: [], scores: [], total: 0, matching: 0 };
    branches.set(k, b);
    if (b.loading) return;
    b.loading = true;
    render();
    const gen = generation;
    try {
      const offset = more ? (b.dimension ? b.nodes.length : b.scores.length) : 0;
      const r: TreeLevel = await backend.libraryTree(levels, path, offset, PAGE);
      if (gen !== generation) return;
      b.dimension = r.dimension;
      b.nodes = more ? [...b.nodes, ...r.nodes] : r.nodes;
      b.scores = more ? [...b.scores, ...r.scores] : r.scores;
      b.total = r.total;
      b.matching = r.matching;
    } catch (e) {
      if (gen === generation) notify(errorText(e), "error");
    } finally {
      b.loading = false;
      if (gen === generation) render();
    }
  };

  const toggle = (path: string[]) => {
    const b = branches.get(keyOf(path));
    if (b && (b.nodes.length || b.scores.length)) {
      b.open = !b.open;
      render();
    } else {
      void load(path);
    }
  };

  const rows = (path: string[], depth: number): HTMLElement[] => {
    const b = branches.get(keyOf(path));
    if (!b || !b.open) return [];
    const pad = (d: number) => `padding-left:${8 + d * 16}px`;
    const out: HTMLElement[] = [];
    if (b.dimension) {
      const max = Math.max(1, ...b.nodes.map((n) => n.count));
      for (const n of b.nodes) {
        const childPath = [...path, n.value];
        const child = branches.get(keyOf(childPath));
        const isOpen = !!child?.open && (child.nodes.length > 0 || child.scores.length > 0 || child.loading);
        const row = h("div", { class: `tree-node${isOpen ? " open" : ""}`, style: pad(depth), title: `${label(b.dimension)}: ${n.value}` },
          h("span", { class: "caret" }, child?.loading ? "…" : isOpen ? "▾" : "▸"),
          h("span", { class: "name" }, n.value),
          h("span", { class: "bar" }, h("i", { style: `width:${Math.max(2, Math.round((n.count / max) * 100))}%` })),
          h("span", { class: "count" }, n.count.toLocaleString("de-DE")));
        row.addEventListener("click", () => toggle(childPath));
        out.push(row, ...rows(childPath, depth + 1));
      }
      if (b.nodes.length < b.total) {
        out.push(h("button", { class: "tree-more", style: pad(depth), onclick: () => void load(path, true) },
          `weitere ${(b.total - b.nodes.length).toLocaleString("de-DE")} ${label(b.dimension)}-Einträge …`));
      }
    } else {
      for (const s of b.scores) {
        const row = h("div", { class: `tree-score${s.doc === selectedDoc ? " on" : ""}`, style: pad(depth), title: "Partitur öffnen" },
          icon("clip"),
          h("span", { class: "name" }, s.title || "(ohne Titel)", s.composer ? h("span", { class: "muted" }, ` – ${s.composer}`) : null),
          h("span", { class: "facts" }, s.facts));
        row.addEventListener("click", () => {
          selectedDoc = s.doc;
          onOpen(s.doc);
          render();
        });
        out.push(row);
      }
      if (b.scores.length < b.total) {
        out.push(h("button", { class: "tree-more", style: pad(depth), onclick: () => void load(path, true) },
          `weitere ${(b.total - b.scores.length).toLocaleString("de-DE")} Partituren …`));
      }
    }
    return out;
  };

  function render(): void {
    const root = branches.get("");
    const head = root && root.dimension
      ? [h("div", { class: "tree-head" }, `${root.matching.toLocaleString("de-DE")} Partituren · ${levels.map(label).join(" › ")}`)]
      : [];
    list.replaceChildren(...head, ...rows([], 0));
    if (root?.loading && !root.nodes.length) list.append(h("div", { class: "lib-empty" }, "Lade …"));
  }

  const rebuild = () => {
    generation++;
    branches.clear();
    renderChips();
    list.replaceChildren();
    if (!info?.id || !info.treeScores) return;
    void load([]);
  };

  function setLevels(next: string[]): void {
    levels = next.length ? next : ["genre"];
    saveLevels();
    rebuild();
  }

  const showBanner = () => {
    const parts: Node[] = [];
    if (!info?.id) {
      parts.push(h("span", {}, info?.loading ? "Die Bibliothek wird geladen …" : "Keine Bibliothek geladen."));
    } else if (!info.scores) {
      parts.push(h("span", {}, `„${info.name}“ ist noch leer – oben „Herunterladen“ oder Partituren hinzufügen.`));
    } else if (!info.treeScores) {
      parts.push(h("span", {}, "Diese Bibliothek wurde vor der Baumansicht indiziert. "),
        h("button", { class: "btn small blue", title: "Aus ihren Paketen und Ordnern neu aufbauen", onclick: () => backend.libraryReindex().catch((e) => notify(errorText(e), "error")) }, "Neu aufbauen"));
    } else if (!info.hasMeta) {
      parts.push(h("span", {}, "Genre und Komponist aus PDMX fehlen noch (nur für PDMX-Bibliotheken). "),
        h("button", { class: "btn small blue", title: "PDMX.csv (225 MB) lesen, passende Zeilen behalten, neu indizieren", onclick: () => backend.libraryEnrich().catch((e) => notify(errorText(e), "error")) },
          icon("download"), "Genre & Komponist nachladen (225 MB)"));
    }
    banner.replaceChildren(...parts);
    banner.hidden = parts.length === 0;
  };

  backend
    .libraryDimensions()
    .then((d) => {
      dims = d;
      levels = levels.filter((k) => dims.some((x) => x.key === k));
      if (!levels.length) levels = ["genre", "komponist"];
      renderChips();
      render();
    })
    .catch(() => {});
  renderChips();

  return {
    bar,
    el,
    setInfo(next) {
      info = next;
      const sig = `${next?.id ?? ""}|${next?.treeScores ?? 0}|${next?.hasMeta ?? false}`;
      showBanner();
      if (sig !== infoSig) {
        infoSig = sig;
        rebuild();
      }
    },
  };
}
