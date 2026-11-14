// Bibliothek stöbern: Baum mit frei wählbaren Ebenen (Genre, Komponist,
// Stimmung, Besetzung, Tonart …). Zweige werden erst beim Aufklappen geladen,
// große Listen seitenweise. Mehrwertige Ebenen (Genre, Instrumentengruppe)
// legen eine Partitur in mehrere Zweige.

import { backend } from "../bridge.js";
import { dropdown, h, icon } from "../dom.js";
import { language, plural, t } from "../i18n.js";
import { errorText, notify } from "../store.js";
import type { Dimension, LibraryInfo, TreeLevel, TreeNode, TreeScore } from "../types.js";

const LEVELS_KEY = "klangwelt.bibliothek.ebenen";
const PAGE = 200;

const PRESETS: Array<[string, string[]]> = [
  [t("Genre › Composer"), ["genre", "komponist"]],
  [t("Composer › Genre"), ["komponist", "genre"]],
  [t("Genre › Mood › Instrumentation"), ["genre", "stimmung", "besetzung"]],
  [t("Mood › Instrumentation › Key"), ["stimmung", "besetzung", "tonart"]],
  [t("Instrumentation › Instrument group › Difficulty"), ["besetzung", "instrumente", "schwierigkeit"]],
  [t("Major/minor › Tempo › Time signature"), ["tongeschlecht", "tempo", "taktart"]],
  [t("Color › Energy › Harmony"), ["faerbung", "energie", "harmonik"]],
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
  const addBtn = h("button", { class: "btn small", title: t("Add level") }, icon("plus"), `${t("Grouping")} ▾`);
  const add = dropdown(addBtn, "tree-menu", (menu, close) => {
    const rest = dims.filter((d) => !levels.includes(d.key));
    menu.replaceChildren(
      ...(rest.length
        ? rest.map((d) => h("button", { class: "menu-item", onclick: () => { setLevels([...levels, d.key]); close(); } }, d.label, d.multi ? h("span", {}, t("multiple")) : null))
        : [h("div", { class: "menu-hint" }, t("All levels are already included."))]),
    );
  });
  const presetBtn = h("button", { class: "btn small", title: t("Ready-made arrangements") }, `${t("Presets")} ▾`);
  const presets = dropdown(presetBtn, "tree-menu", (menu, close) => {
    menu.replaceChildren(...PRESETS.map(([name, lv]) => h("button", { class: "menu-item", onclick: () => { setLevels(lv); close(); } }, name)));
  });
  const bar = h("div", { class: "tree-bar" }, h("span", { class: "tree-bar-label" }, t("Levels:")), chips, add, presets);

  let dragFrom = -1;
  const renderChips = () => {
    chips.replaceChildren(
      ...levels.map((k, i) => {
        const chip = h("span", { class: "tree-chip", draggable: "true", title: t("Drag to reorder") },
          h("b", {}, String(i + 1)), label(k),
          h("button", { class: "tree-chip-x", title: t("Remove level"), onclick: () => setLevels(levels.filter((_, j) => j !== i)) }, "×"));
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
        const row = h("div", { class: `tree-node${isOpen ? " open" : ""}`, style: pad(depth), title: `${label(b.dimension)}: ${n.label || n.value}` },
          h("span", { class: "caret" }, child?.loading ? "…" : isOpen ? "▾" : "▸"),
          h("span", { class: "name" }, n.label || n.value),
          h("span", { class: "bar" }, h("i", { style: `width:${Math.max(2, Math.round((n.count / max) * 100))}%` })),
          h("span", { class: "count" }, n.count.toLocaleString(language())));
        row.addEventListener("click", () => toggle(childPath));
        out.push(row, ...rows(childPath, depth + 1));
      }
      if (b.nodes.length < b.total) {
        const more = b.total - b.nodes.length;
        out.push(h("button", { class: "tree-more", style: pad(depth), onclick: () => void load(path, true) },
          plural(more, "one more {dimension} entry…", "{n} more {dimension} entries…", { n: more.toLocaleString(language()), dimension: label(b.dimension) })));
      }
    } else {
      for (const s of b.scores) {
        const row = h("div", { class: `tree-score${s.doc === selectedDoc ? " on" : ""}`, style: pad(depth), title: t("Open score") },
          icon("clip"),
          h("span", { class: "name" }, s.title || t("(untitled)"), s.composer ? h("span", { class: "muted" }, ` – ${s.composer}`) : null),
          h("span", { class: "facts" }, s.facts));
        row.addEventListener("click", () => {
          selectedDoc = s.doc;
          onOpen(s.doc);
          render();
        });
        out.push(row);
      }
      if (b.scores.length < b.total) {
        const more = b.total - b.scores.length;
        out.push(h("button", { class: "tree-more", style: pad(depth), onclick: () => void load(path, true) },
          plural(more, "one more score…", "{n} more scores…", { n: more.toLocaleString(language()) })));
      }
    }
    return out;
  };

  function render(): void {
    const root = branches.get("");
    const head = root && root.dimension
      ? [h("div", { class: "tree-head" }, plural(root.matching, "{n} score · {levels}", "{n} scores · {levels}", { n: root.matching.toLocaleString(language()), levels: levels.map(label).join(" › ") }))]
      : [];
    list.replaceChildren(...head, ...rows([], 0));
    if (root?.loading && !root.nodes.length) list.append(h("div", { class: "lib-empty" }, t("Loading…")));
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
      parts.push(h("span", {}, info?.loading ? t("The library is loading…") : t("No library loaded.")));
    } else if (!info.scores) {
      parts.push(h("span", {}, t("\"{name}\" is still empty – use \"Download\" above or add scores.", { name: info.name ?? "" })));
    } else if (!info.treeScores) {
      parts.push(h("span", {}, `${t("This library was indexed before the tree view existed.")} `),
        h("button", { class: "btn small blue", title: t("Rebuild from its packs and folders"), onclick: () => backend.libraryReindex().catch((e) => notify(errorText(e), "error")) }, t("Rebuild")));
    } else if (!info.hasMeta) {
      parts.push(h("span", {}, `${t("Genre and composer from PDMX are still missing (PDMX libraries only).")} `),
        h("button", { class: "btn small blue", title: t("Read PDMX.csv (225 MB), keep the matching rows, re-index"), onclick: () => backend.libraryEnrich().catch((e) => notify(errorText(e), "error")) },
          icon("download"), t("Download genre & composer (225 MB)")));
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
