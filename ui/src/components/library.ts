// Musikbibliothek: Pakete herunterladen und Partituren in die aktive Bibliothek
// aufnehmen, Bibliotheken verwalten (je ein Index), nach Titel und Melodie
// suchen, Partitur mit Stimmrollen anzeigen, Stimme als Spur übernehmen.

import { backend } from "../bridge.js";
import { chordsInput, chordsText } from "../spelling.js";
import { isAudioPath } from "../files.js";
import { fitCanvas, h, icon, noteName } from "../dom.js";
import { language, plural, t } from "../i18n.js";
import { highOnTop } from "../pianoroll.js";
import { treeView } from "./libtree.js";
import { effect, signal } from "../signals.js";
import { act, errorText, findClip, notify, project, selection, showPanel, status as engineStatus } from "../store.js";
import type { CatalogEntry, LibEntry, LibHit, LibraryInfo, PlayInfo, ScoreView } from "../types.js";

/** Zustand bleibt erhalten, auch wenn der Tab gewechselt wird. */
const state = {
  info: signal<LibraryInfo | null>(null),
  hits: signal<LibHit[]>([]),
  score: signal<ScoreView | null>(null),
  /** Gewählte Stimmen (Indizes, nie leer bei offener Partitur): Klick wählt eine, Strg+Klick mehrere. */
  voices: signal<number[]>([]),
  /** Markiertes Zeitfenster (Viertel) in der Partitur. */
  window: signal<[number, number] | null>(null),
  /** Ab hier wird vorgespielt (Viertel); Klick in die Notenrolle setzt ihn. */
  cursor: signal(0),
  /** Gewähltes Tempo (Viertel je Minute) für Anhören und Übernehmen; null = das der Partitur. */
  tempo: signal<number | null>(null),
  text: "",
  melody: "",
};

/** Vorsing-Aufnahme (bleibt erhalten, falls das Fenster neu aufgebaut wird). */
const listen = { active: false, since: 0 };
/** Danach wird automatisch gestoppt (das Backend wertet höchstens 45 s aus). */
const LISTEN_MAX_SECS = 40;

/** Bibliotheksfenster, das gezogene Dateien annimmt. */
let dropTarget: HTMLElement | null = null;

/** Nimmt die Bibliothek gerade gezogene Dateien an (Fenster offen und sichtbar)? */
export function libraryTakesDrops(): boolean {
  return !!dropTarget && dropTarget.isConnected && !!dropTarget.offsetParent;
}

function addToLibrary(paths: string[]): void {
  if (!state.info.peek()?.id) {
    notify(t("The library is still loading – try again in a moment."), "error");
    return;
  }
  backend.libraryAdd(paths).then((i) => (state.info.value = i)).catch((e) => notify(errorText(e), "error"));
}

let listening = false;
/** Beim Programmstart aufrufen: hört auf Bibliotheksänderungen – unabhängig davon, ob
 *  das Fenster „Bibliothek“ offen ist. Welche Bibliothek aktiv ist, merkt sich das Programm. */
export function initLibrary(): void {
  ensureListening();
}
function ensureListening(): void {
  if (listening) return;
  listening = true;
  backend.onLibraryChanged((i) => (state.info.value = i));
  // MusicXML-Dateien aufs Fenster ziehen: bei offener Bibliothek in den Index aufnehmen.
  backend.onFileDrop((paths) => {
    const panel = dropTarget;
    if (!panel || !panel.isConnected || !panel.offsetParent) return;
    const scores = paths.filter((p) => /.(mxl|musicxml|xml)$/i.test(p) || !/.[a-z0-9]{1,8}$/i.test(p));
    if (!scores.length) {
      if (paths.some(isAudioPath)) return; // Audiodateien werden Spuren (filedrop.ts)
      notify(t("Only MusicXML files (.mxl, .musicxml, .xml) or folders can be added to the library."), "error");
      return;
    }
    addToLibrary(scores);
  });
  backend
    .libraryInfo()
    .then((i) => (state.info.value = i))
    .catch(() => {});
  // Früher merkte sich die Oberfläche den Pfad der Bibliothek – jetzt das Programm.
  try {
    localStorage.removeItem("klangwelt.bibliothek.pfad");
  } catch {
    /* ohne Speicher eben nicht */
  }
}

const MODE_KEY = "klangwelt.bibliothek.modus";
let libMode: "suchen" | "stoebern" = (() => {
  try {
    return localStorage.getItem(MODE_KEY) === "stoebern" ? "stoebern" : "suchen";
  } catch {
    return "suchen";
  }
})();

const fmt = (n: number) => Math.round(n).toLocaleString(language());
const mb = (bytes: number) => `${(bytes / 1e6).toLocaleString(language(), { maximumFractionDigits: bytes < 1e8 ? 1 : 0 })} MB`;

/** Katalog herunterladbarer Bibliotheken: je Sammlung ein aufklappbares Panel
 *  mit Karten (Größe, Format, Lizenz, Zielordner). */
function catalogView(onClose: () => void): { el: HTMLElement; refresh(): void; sync(info: LibraryInfo | null): void } {
  const panels = h("div", { class: "lib-panels" });
  /** Knöpfe der Karten nach dem Stand (lädt gerade, wartet, Einreihen). */
  let syncs: ((info: LibraryInfo | null) => void)[] = [];
  let lastInfo: LibraryInfo | null = null;
  const el = h(
    "div",
    { class: "lib-catalog" },
    h("div", { class: "lib-catalog-head" },
      h("b", {}, t("Download music library")),
      h("span", { class: "muted" }, t("Once downloaded, it's added to the active library. For Bach, Mozart and Beethoven in good quality: KernScores, OpenScore and DCML.")),
      h("button", { class: "btn small ghost", title: t("Close"), onclick: onClose }, icon("close"))),
    panels,
  );
  const sizeText = (v: number) =>
    v >= 1000 ? `${(v / 1000).toLocaleString(language(), { maximumFractionDigits: 1 })} GB` : v < 1 ? "< 1 MB" : `${v} MB`;

  const card = (c: CatalogEntry) => {
    const folder = h("input", { class: "lib-input mono small", value: c.folder, title: t("Folder for the scores – can be changed") });
    folder.addEventListener("keydown", (e) => e.stopPropagation());
    const isPdmx = c.group === "pdmx";
    /** Platz in der Warteschlange (für die aktive Bibliothek), -1 = wartet nicht. */
    const queuePos = (info: LibraryInfo | null) => (info?.queue ?? []).findIndex((q) => q.id === c.id && q.library === info?.id);
    const go = () => {
      if (c.inLibrary) return;
      const info = state.info.peek();
      // Wartet schon: Klick nimmt es wieder heraus.
      if (queuePos(info) >= 0) {
        backend.libraryDequeue(c.id, info?.id ?? null).then((i) => (state.info.value = i)).catch((e) => notify(errorText(e), "error"));
        return;
      }
      const f = folder.value.trim();
      // Große Pakete: erst nachfragen (zweiter Klick startet).
      if (!c.installed && c.downloadMb >= 1000 && !confirmArmed) {
        confirmArmed = true;
        goBtn.classList.add("warn");
        goBtn.replaceChildren(icon("download"), t("Really download {size}?", { size: sizeText(c.downloadMb) }));
        setTimeout(() => {
          confirmArmed = false;
          goBtn.classList.remove("warn");
          shown = "";
          sync(lastInfo);
        }, 6000);
        return;
      }
      backend
        .libraryDownload(c.id, f || null, isPdmx && withMeta.checked)
        .then((i) => {
          state.info.value = i;
          const pos = queuePos(i);
          if (pos >= 0) notify(t("\"{name}\" is waiting in the queue (position {pos}) – it starts as soon as the current job is done.", { name: c.name, pos: pos + 1 }));
        })
        .catch((e) => notify(errorText(e), "error"));
    };
    let confirmArmed = false;
    const withMeta = h("input", { type: "checkbox", checked: true });
    const metaRow = h("label", { class: "check lib-meta", title: t("Reads PDMX.csv (225 MB) and keeps only the rows for the downloaded scores – for the tree view by genre, composer and difficulty.") },
      withMeta, t("Also download genre & composer (+225 MB)"));
    const [label, tip, ico] = c.inLibrary
      ? [t("In \"{library}\"", { library: c.inLibrary }), t("Already in the library \"{library}\"", { library: c.inLibrary }), "check"]
      : c.installed
        ? [t("Import"), t("The scores are already in the folder – just add them to the active library"), "book"]
        : [t("Download"), t("Download and add to the active library"), "download"];
    const goBtn = h("button", { class: `btn small ${c.inLibrary || c.installed ? "" : "blue"}`, title: tip, disabled: !!c.inLibrary, onclick: go }, icon(ico), label);
    // Läuft gerade ein Auftrag, wird das Paket eingereiht statt gestartet.
    let shown = "";
    const sync = (info: LibraryInfo | null) => {
      if (c.inLibrary || confirmArmed) return;
      const pos = queuePos(info);
      const running = !!info && info.currentPack === c.id;
      const busy = !!info && (!!info.download || !!info.building || info.merging != null || info.adding || !!info.currentPack || !!info.queue?.length);
      const key = running ? "run" : pos >= 0 ? `q${pos}` : busy ? "busy" : "idle";
      if (key === shown) return;
      shown = key;
      goBtn.classList.remove("queued", "blue");
      goBtn.disabled = running;
      if (running) {
        goBtn.replaceChildren(icon("download"), t("Loading…"));
        goBtn.title = t("This pack is being downloaded or added right now");
      } else if (pos >= 0) {
        goBtn.classList.add("queued");
        goBtn.replaceChildren(icon("close"), t("Waiting · position {pos}", { pos: pos + 1 }));
        goBtn.title = t("In the queue – starts as soon as the packs ahead of it are done. Click to remove it.");
      } else if (busy) {
        if (!c.installed) goBtn.classList.add("blue");
        goBtn.replaceChildren(icon("plus"), c.installed ? t("Queue import") : t("Add to queue"));
        goBtn.title = t("Another job is running – the pack goes into the queue and will be downloaded and added afterwards");
      } else {
        if (!c.installed) goBtn.classList.add("blue");
        goBtn.replaceChildren(icon(ico), label);
        goBtn.title = tip;
      }
    };
    syncs.push(sync);
    return h(
      "div",
      { class: `lib-card${c.inLibrary ? " installed" : ""}`, "data-id": c.id },
      h("div", { class: "lib-card-title" }, c.name,
        c.inLibrary ? h("span", { class: "tag" }, t("in \"{library}\"", { library: c.inLibrary })) : c.installed ? h("span", { class: "tag local", title: t("The scores are already in the folder") }, t("in folder")) : null),
      h("div", { class: "lib-card-tags" },
        h("span", { class: "chip" }, c.format),
        c.expertHarmony ? h("span", { class: "chip expert", title: t("Expert chord analysis for every bar – used in the chord statistics instead of automatic detection") }, t("Harmonic analysis")) : null),
      h("div", { class: "lib-card-facts" },
        plural(c.scores, "≈ {n} score · {download} download · {disk} on disk", "≈ {n} scores · {download} download · {disk} on disk", { n: fmt(c.scores), download: sizeText(c.downloadMb), disk: sizeText(c.diskMb) })),
      h("div", { class: "lib-card-desc" }, c.description),
      c.inLibrary ? null : folder,
      c.inLibrary || c.installed || !isPdmx ? null : metaRow,
      h("div", { class: "lib-card-foot" },
        h("a", { class: "lib-license", href: c.sourceUrl, target: "_blank", rel: "noreferrer", title: c.sourceUrl }, c.license),
        goBtn),
    );
  };

  const refresh = () => {
    backend
      .libraryCatalog()
      .then((list) => {
        const groups: { id: string; name: string; description: string; items: CatalogEntry[] }[] = [];
        for (const c of list) {
          const id = c.group ?? "pdmx";
          let g = groups.find((x) => x.id === id);
          if (!g) groups.push((g = { id, name: c.groupName ?? id, description: c.groupDescription ?? "", items: [] }));
          g.items.push(c);
        }
        syncs = [];
        panels.replaceChildren(
          ...groups.map((g) => {
            const contained = g.items.filter((c) => c.inLibrary).length;
            const libName = g.items.find((c) => c.inLibrary)?.inLibrary ?? "";
            const panel = h(
              "details",
              { class: `lib-panel lib-panel-${g.id}`, open: panelOpen(g.id) },
              h("summary", { class: "lib-panel-head" },
                h("b", {}, g.name),
                h("span", { class: "muted" }, plural(g.items.length, "{n} pack", "{n} packs")),
                contained ? h("span", { class: "tag" }, t("{n} in \"{library}\"", { n: contained, library: libName })) : null),
              h("div", { class: "lib-panel-desc" }, g.description),
              h("div", { class: "lib-cards" }, ...g.items.map(card)),
            ) as HTMLDetailsElement;
            panel.addEventListener("toggle", () => setPanelOpen(g.id, panel.open));
            return panel;
          }),
        );
        for (const s of syncs) s(lastInfo);
      })
      .catch(() => panels.replaceChildren(h("div", { class: "lib-empty" }, t("Downloading is only available in the desktop app."))));
  };
  const sync = (info: LibraryInfo | null) => {
    lastInfo = info;
    for (const s of syncs) s(info);
  };
  return { el, refresh, sync };
}

/** Bibliotheken verwalten: Liste (Name, Partituren, Pakete), wechseln, neu
 *  anlegen, umbenennen, löschen (nur den Index, nie Partituren). */
function manageView(onClose: () => void): { el: HTMLElement; refresh(): void } {
  const list = h("div", { class: "lib-libs" });
  const newName = h("input", { class: "lib-input", placeholder: t("Name of the new library") });
  const err = (e: unknown) => notify(errorText(e), "error");
  let current: LibEntry[] = [];

  const create = () => {
    const name = newName.value.trim();
    if (!name) return notify(t("Please enter a name."), "error");
    backend
      .libraryCreate(name)
      .then((libs) => {
        newName.value = "";
        // neu angelegt = gleich aktiv (sie soll ja gefüllt werden)
        const made = libs.find((l) => l.name === name);
        return made ? backend.librarySwitch(made.id) : libs;
      })
      .then(render)
      .catch(err);
  };
  newName.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") create();
  });

  const row = (l: LibEntry) => {
    const name = h("span", { class: "n" }, l.name);
    const title = h("div", { class: "lib-lib-name" }, name, l.active ? h("span", { class: "tag" }, t("active")) : null);
    const facts = [
      plural(l.scores, "{n} score", "{n} scores", { n: fmt(l.scores) }),
      l.packs.length ? plural(l.packs.length, "Pack: {packs}", "Packs: {packs}", { packs: l.packs.join("; ") }) : "",
      l.paths.length ? plural(l.paths.length, "{n} folder/file", "{n} folders/files") : "",
      l.bytes >= 1e5 ? mb(l.bytes) : l.bytes ? t("< 0.1 MB") : "",
    ].filter(Boolean).join(" · ");
    const actions = h("div", { class: "lib-lib-actions" });
    if (!l.active) {
      actions.append(h("button", { class: "btn small blue", title: t("Search this library – downloads and added scores will then go here"), onclick: () => backend.librarySwitch(l.id).then(render).catch(err) }, t("Switch")));
    }
    const rename = () => {
      const input = h("input", { class: "lib-input small", value: l.name });
      const commit = () => {
        const v = input.value.trim();
        if (!v || v === l.name) return render(current);
        backend.libraryRename(l.id, v).then(render).catch(err);
      };
      input.addEventListener("keydown", (e) => {
        e.stopPropagation();
        if (e.key === "Enter") commit();
        if (e.key === "Escape") render(current);
      });
      name.replaceWith(input, h("button", { class: "btn small", onclick: commit }, icon("check"), "OK"));
      input.focus();
      input.select();
    };
    actions.append(h("button", { class: "btn small", onclick: rename }, t("Rename")));
    if (!l.standard) {
      let armed = false;
      const del = h("button", {
        class: "btn small",
        title: t("Deletes only this library's index – the scores stay on disk."),
        onclick: () => {
          if (!armed) {
            armed = true;
            del.classList.add("warn");
            del.replaceChildren(icon("trash"), t("Really delete?"));
            setTimeout(() => {
              armed = false;
              del.classList.remove("warn");
              del.replaceChildren(icon("trash"), t("Delete"));
            }, 5000);
            return;
          }
          backend.libraryDelete(l.id).then(render).catch(err);
        },
      }, icon("trash"), t("Delete"));
      actions.append(del);
    }
    const el = h("div", { class: `lib-lib${l.active ? " active" : ""}` }, title, h("div", { class: "lib-lib-facts" }, facts), actions);
    el.dataset.id = l.id;
    return el;
  };

  const render = (libs: LibEntry[]) => {
    current = libs;
    list.replaceChildren(...(libs.length ? libs.map(row) : [h("div", { class: "lib-empty" }, t("Libraries are only available in the desktop app."))]));
  };
  const el = h(
    "div",
    { class: "lib-manage" },
    h("div", { class: "lib-catalog-head" },
      h("b", {}, t("Libraries")),
      h("span", { class: "muted" }, t("Each library is a separate index – only the active one is searched. Downloaded and added scores go into the active one.")),
      h("button", { class: "btn small ghost", title: t("Close"), onclick: onClose }, icon("close"))),
    h("div", { class: "lib-manage-body" },
      list,
      h("div", { class: "lib-new" }, newName, h("button", { class: "btn small blue", onclick: create }, icon("plus"), t("Create new")))),
  );
  const refresh = () => {
    backend.libraryList().then(render).catch(() => render([]));
  };
  return { el, refresh };
}

/** Auf- und zugeklappte Panels merken (nur Bequemlichkeit, darf fehlen). */
const PANEL_KEY = "klangwelt.bibliothek.panels";
function panelState(): Record<string, boolean> {
  try {
    return JSON.parse(localStorage.getItem(PANEL_KEY) ?? "{}") as Record<string, boolean>;
  } catch {
    return {};
  }
}
function panelOpen(id: string): boolean {
  // Standard: die kuratierten Sammlungen offen, PDMX zu (lang und gemischt)
  return panelState()[id] ?? id !== "pdmx";
}
function setPanelOpen(id: string, open: boolean) {
  try {
    localStorage.setItem(PANEL_KEY, JSON.stringify({ ...panelState(), [id]: open }));
  } catch {
    /* ohne Speicher eben nicht */
  }
}

const VOICE_COLORS = ["#ffb454", "#6ee7d8", "#a78bfa", "#f472b6", "#60a5fa", "#4ade80", "#fb923c", "#facc15"];

/** Anfangsauswahl einer Partitur: die Treffer-Stimme, ohne Treffer alle Stimmen. */
function defaultVoices(s: ScoreView): number[] {
  const hit = s.voices.findIndex((v) => v.isHit);
  return hit >= 0 ? [hit] : s.voices.map((_, i) => i);
}

/** Partitur zeigen und die Stimmenauswahl zurücksetzen. */
function showScore(s: ScoreView): void {
  state.voices.value = defaultVoices(s);
  // Cursor an die Fundstelle (bzw. den Anfang), Tempo wieder wie in der Partitur
  const hit = s.voices.find((v) => v.highlight)?.highlight ?? s.range ?? null;
  if (s.doc !== state.score.peek()?.doc) state.tempo.value = null;
  state.cursor.value = hit ? hit[0] : 0;
  state.score.value = s;
}

/** Tempo der Partitur (Viertel je Minute) – wie im Backend: ohne brauchbare Angabe 90. */
const scoreTempo = (s: ScoreView) => (s.tempoBpm && s.tempoBpm >= 30 && s.tempoBpm <= 240 ? s.tempoBpm : 90);
const TEMPO_MIN = 10;
const TEMPO_MAX = 400;

const voicePairs = (s: ScoreView, idx: number[]): Array<[string, string]> =>
  idx.map((i) => s.voices[i]).filter((v): v is ScoreView["voices"][number] => !!v).map((v) => [v.part, v.label]);

/** Viertel je Takt der Partitur. */
const barQuarters = (s: ScoreView) => (s.timeSignature ? (s.timeSignature[0] * 4) / s.timeSignature[1] : 4) || 4;
const barLabel = (s: ScoreView, [a, b]: [number, number]) =>
  t("Bars {from}–{to}", { from: Math.floor(a / barQuarters(s)) + 1, to: Math.ceil(b / barQuarters(s) - 1e-6) });

/** Sichtbarer Ausschnitt der Notenrolle: Pixel je Viertel und linker Rand (Inhaltspixel). */
interface RollView {
  qw: number;
  ox: number;
}

/** Zeitleiste über der Notenrolle: Taktzahlen, Zählzeiten, markiertes Zeitfenster. */
function drawRuler(canvas: HTMLCanvasElement, s: ScoreView, view: RollView, win: [number, number] | null): void {
  const { ctx, w, h: hh } = fitCanvas(canvas);
  const { qw, ox } = view;
  ctx.fillStyle = "#121620";
  ctx.fillRect(0, 0, w, hh);
  ctx.fillStyle = "#262d3b";
  ctx.fillRect(0, hh - 1, w, 1);
  if (win) {
    const x0 = win[0] * qw - ox;
    const x1 = win[1] * qw - ox;
    ctx.fillStyle = "rgba(122, 167, 255, 0.16)";
    ctx.fillRect(x0, 0, x1 - x0, hh - 1);
    ctx.fillStyle = "#7aa7ff";
    ctx.fillRect(x0, 0, x1 - x0, 3);
  }
  const bq = barQuarters(s);
  // Taktzahlen mindestens ~40 px auseinander
  const step = [1, 2, 4, 8, 16, 32, 64].find((n) => n * bq * qw >= 40) ?? 128;
  const first = Math.max(0, Math.floor(ox / qw / bq));
  const last = Math.ceil((ox + w) / qw / bq);
  ctx.font = "11px 'Segoe UI', system-ui, sans-serif";
  ctx.textBaseline = "middle";
  for (let bar = first - (first % step); bar <= last; bar += step) {
    const x = Math.round(bar * bq * qw - ox) + 0.5;
    ctx.fillStyle = "#3a4458";
    ctx.fillRect(x, hh - 10, 1, 10);
    ctx.fillStyle = "#8a93a6";
    ctx.fillText(String(bar + 1), x + 5, hh / 2 - 1);
    if (step === 1 && qw > 14) {
      for (let k = 1; k < bq; k++) {
        ctx.fillStyle = "#2a3242";
        ctx.fillRect(Math.round((bar * bq + k) * qw - ox) + 0.5, hh - 5, 1, 5);
      }
    }
  }
}

function drawScore(canvas: HTMLCanvasElement, s: ScoreView, selected: number[], win: [number, number] | null, view: RollView): void {
  const { ctx, w, h: hh } = fitCanvas(canvas);
  ctx.clearRect(0, 0, w, hh);
  const all = s.voices.flatMap((v) => v.notes.map((n) => n[2]));
  if (!all.length) return;
  const lo = Math.min(...all) - 2;
  const hi = Math.max(...all) + 2;
  const { qw, ox } = view;
  // nur der sichtbare Ausschnitt: alles um den linken Rand verschoben
  ctx.save();
  ctx.translate(-ox, 0);
  const lane = (hh - 16) / (hi - lo + 1);
  // Richtung wie in der Notenansicht (Einstellung „Tonachse“)
  const up = highOnTop.peek();
  const y = (m: number) => 8 + (up ? hi - m : m - lo) * lane;
  const q0 = ox / qw;
  const q1 = (ox + w) / qw;
  const bq = barQuarters(s);
  for (let q = Math.floor(q0 / bq) * bq; q <= Math.min(s.durationQuarters, q1); q += bq) {
    ctx.fillStyle = "#1d2330";
    ctx.fillRect(Math.round(q * qw) + 0.5, 0, 1, hh);
  }
  for (let m = Math.ceil(lo); m <= hi; m++) {
    if (m % 12 === 0) {
      ctx.fillStyle = "#252c3a";
      ctx.fillRect(ox, up ? y(m) + lane : y(m), w, 1);
      ctx.fillStyle = "#5c6578";
      ctx.font = "9px system-ui";
      ctx.fillText(noteName(m), ox + 2, y(m) + 9);
    }
  }
  s.voices.forEach((v, i) => {
    if (v.highlight) {
      ctx.fillStyle = "rgba(255, 180, 84, 0.12)";
      ctx.fillRect(v.highlight[0] * qw, 0, (v.highlight[1] - v.highlight[0]) * qw, hh);
    }
    const color = VOICE_COLORS[i % VOICE_COLORS.length]!;
    ctx.globalAlpha = selected.includes(i) ? 1 : 0.3;
    ctx.fillStyle = color;
    for (const [on, dur, m] of v.notes) {
      if (on > q1 || on + dur < q0) continue;
      ctx.fillRect(on * qw, y(m), Math.max(1.5, dur * qw - 1), Math.max(1.5, lane - 0.5));
    }
  });
  ctx.globalAlpha = 1;
  if (s.range) {
    // Stelle aus dem Quintenzirkel (Akkordwechsel): Rahmen über alle Stimmen
    const [a, b] = s.range;
    ctx.fillStyle = "rgba(110, 231, 216, 0.10)";
    ctx.fillRect(a * qw, 0, (b - a) * qw, hh);
    ctx.strokeStyle = "rgba(110, 231, 216, 0.85)";
    ctx.lineWidth = 1.5;
    ctx.strokeRect(a * qw + 0.75, 0.75, (b - a) * qw - 1.5, hh - 1.5);
  }
  for (const [on, label] of s.cadences) {
    ctx.fillStyle = "#8a93a6";
    ctx.font = "9px system-ui";
    ctx.fillText(label === "Ganzschluss" ? "‖" : "|", on * qw - 2, hh - 3);
  }
  ctx.restore();
  if (win) {
    // markiertes Zeitfenster: nur das wird übernommen bzw. vorgespielt
    const [a, b] = win;
    const x0 = a * qw - ox;
    const x1 = b * qw - ox;
    ctx.fillStyle = "rgba(15, 18, 26, 0.55)";
    ctx.fillRect(0, 0, x0, hh);
    ctx.fillRect(x1, 0, w - x1, hh);
    ctx.fillStyle = "rgba(122, 167, 255, 0.10)";
    ctx.fillRect(x0, 0, x1 - x0, hh);
    ctx.fillStyle = "#7aa7ff";
    ctx.fillRect(x0 - 1, 0, 2, hh);
    ctx.fillRect(x1 - 1, 0, 2, hh);
    for (const x of [x0, x1]) ctx.fillRect(x - 3, hh / 2 - 12, 6, 24);
    ctx.font = "600 10px system-ui";
    const label = barLabel(s, win);
    ctx.fillStyle = "#0d0f14";
    ctx.fillRect(x0 + 3, 3, ctx.measureText(label).width + 8, 15);
    ctx.fillStyle = "#a9c4ff";
    ctx.fillText(label, x0 + 7, 14);
  }
}

export function libraryPanel(): { el: HTMLElement; update(): void } {
  ensureListening();

  // Aktive Bibliothek: Name, Stand, Fortschritt
  const libName = h("button", { class: "btn small ghost lib-name", title: t("Manage libraries: switch, create, rename, delete") }, icon("book"), h("b", {}, "…"));
  const status = h("div", { class: "lib-status" });
  const progress = h("div", { class: "lib-progress" }, h("i"));

  // Hinzufügen: Dateien oder Ordner in die aktive Bibliothek (auch per Ziehen aufs Fenster)
  const addInput = h("input", { class: "lib-input mono", placeholder: t("Path to a MusicXML/MXL file or a folder – separate several with ;") });
  const addBox = h("div", { class: "lib-addbox" }) as HTMLDivElement;
  const doAdd = () => {
    const paths = addInput.value.split(/[;\n]/).map((p) => p.trim().replace(/^"|"$/g, "")).filter(Boolean);
    if (!paths.length) return notify(t("Enter the path of a MusicXML file (or a folder) – or drag the files onto the window."), "error");
    addToLibrary(paths);
    addInput.value = "";
    setAddBox(false);
  };
  addInput.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") doAdd();
    if (e.key === "Escape") setAddBox(false);
  });
  const addTarget = h("span", { class: "muted" });
  addBox.append(
    addTarget,
    addInput,
    h("button", { class: "btn small blue", onclick: doAdd }, icon("plus"), t("Add")),
    h("button", { class: "btn small ghost", title: t("Close"), onclick: () => setAddBox(false) }, icon("close")),
  );
  const setAddBox = (open: boolean) => {
    addBox.hidden = !open;
    addBtn.classList.toggle("on", open);
    if (open) {
      addTarget.textContent = t("Add to \"{library}\" (or drag files onto the window):", { library: state.info.peek()?.name ?? "…" });
      addInput.focus();
    }
  };
  const addBtn = h("button", {
    class: "btn small",
    title: t("Add MusicXML/MXL files or whole folders to the active library – or just drag them onto the window."),
    onclick: () => setAddBox(!!addBox.hidden),
  }, icon("plus"), t("Add"));
  setAddBox(false);

  // Bibliotheken verwalten (Panel an Stelle der Suche, wie der Katalog)
  const manage = manageView(() => setManage(false));
  const setManage = (open: boolean) => {
    manage.el.hidden = !open;
    libName.classList.toggle("on", open);
    if (open) {
      setCatalog(false);
      setAddBox(false);
      manage.refresh();
    }
  };
  libName.addEventListener("click", () => setManage(!!manage.el.hidden));
  setManage(false);

  // Suche
  const text = h("input", { class: "lib-input", placeholder: t("Title, composer, instrument…"), value: state.text });
  const melody = h("input", { class: "lib-input mono", placeholder: t("Melody: C4 D4 E4 F4 G4 (F♯ or F#, B♭ or Bb …) or 60 62 64 – duration e.g. G4:2"), value: state.melody });
  for (const i of [text, melody]) {
    i.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") void search();
    });
  }
  const search = async (fromSelection = false) => {
    state.text = text.value;
    state.melody = melody.value;
    const q: { text?: string; melody?: string; trackId?: number; clipId?: number } = { text: text.value };
    if (fromSelection) {
      const sel = selection.peek();
      if (!findClip(project(), sel)) {
        notify(t("Please select a clip in the timeline first."));
        return;
      }
      q.trackId = sel!.trackId;
      q.clipId = sel!.clipId!;
    } else {
      q.melody = chordsInput(melody.value); // the core reads German note names
    }
    try {
      state.hits.value = await backend.librarySearch(q);
      if (!state.hits.peek().length) notify(t("No matches."));
    } catch (e) {
      notify(errorText(e), "error");
    }
  };

  const results = h("div", { class: "lib-results" });
  const detail = h("div", { class: "lib-detail" });
  // Notenrolle mit Zeitleiste: eine breite Inhaltsfläche sorgt für den Bildlauf,
  // Zeitleiste und Rolle bleiben als sichtbarer Ausschnitt stehen (position: sticky)
  // und zeichnen nur, was zu sehen ist – auch sehr lange Stücke in jedem Zoom.
  const ruler = h("canvas", { class: "lib-ruler", title: t("Drag: move through time · Click: set cursor · Shift/Ctrl+drag: select a time range · Ctrl+mouse wheel: zoom") });
  const canvas = h("canvas", { class: "lib-roll", title: t("Notes of the selected score – mouse wheel: scroll · Ctrl+mouse wheel: zoom · middle mouse button: drag") });
  const cursorEl = h("div", { class: "lib-cursor", title: t("Playback starts here") });
  const headEl = h("div", { class: "lib-playhead" });
  headEl.hidden = true;
  const content = h("div", { class: "lib-roll-content" }, ruler, canvas, cursorEl, headEl);
  const scroller = h("div", { class: "lib-roll-wrap" }, content);
  /** Zoom: Pixel je Viertel (kurze Stücke füllen mindestens die Breite). */
  let zoom = 14;
  const qwOf = (s: ScoreView) => Math.max(zoom, scroller.clientWidth / Math.max(1, s.durationQuarters));
  const view = (s: ScoreView): RollView => ({ qw: qwOf(s), ox: scroller.scrollLeft });
  /** Neu zeichnen: Breite der Inhaltsfläche, Zeitleiste, Rolle, Cursor. */
  const redraw = () => {
    const s = state.score.peek();
    if (!s) {
      content.style.width = "0px";
      return;
    }
    content.style.width = `${Math.ceil(qwOf(s) * s.durationQuarters)}px`;
    // die beiden Leinwände sind so breit wie der sichtbare Bereich
    ruler.style.width = canvas.style.width = `${scroller.clientWidth}px`;
    const v = view(s);
    drawRuler(ruler, s, v, state.window.peek());
    drawScore(canvas, s, state.voices.peek(), state.window.peek(), v);
    placeAt(cursorEl, state.cursor.peek());
  };
  let redrawQueued = false;
  const queueRedraw = () => {
    if (redrawQueued) return;
    redrawQueued = true;
    requestAnimationFrame(() => {
      redrawQueued = false;
      redraw();
    });
  };
  scroller.addEventListener("scroll", queueRedraw);
  /** Linie an Viertel `q` der Partitur ausrichten. */
  const placeAt = (el: HTMLElement, q: number) => {
    const s = state.score.peek();
    el.style.left = `${s ? q * qwOf(s) : 0}px`;
  };
  effect(() => {
    const q = state.cursor.value;
    state.score.value;
    cursorEl.hidden = !state.score.peek();
    requestAnimationFrame(() => placeAt(cursorEl, q));
  });
  new ResizeObserver(() => {
    if (state.score.peek()) redraw();
  }).observe(scroller);
  // Mausrad: in der Zeit blättern; Strg+Mausrad: Zoom um die Mausposition
  scroller.addEventListener("wheel", (e) => {
    const s = state.score.peek();
    if (!s) return;
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) {
      const mx = e.clientX - scroller.getBoundingClientRect().left;
      const q = (scroller.scrollLeft + mx) / qwOf(s);
      zoom = Math.min(160, Math.max(1, zoom * Math.exp(-e.deltaY * 0.0015)));
      content.style.width = `${Math.ceil(qwOf(s) * s.durationQuarters)}px`;
      scroller.scrollLeft = Math.max(0, q * qwOf(s) - mx);
      redraw();
    } else {
      scroller.scrollLeft += Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    }
  }, { passive: false });

  // Anhören: der gedrückte Knopf wird zum Stopp-Knopf, solange das Vorhören klingt.
  // `seen`: die Engine hat es schon gemeldet; `started`: Antwort ist da (Rendern fertig).
  const libPlay = { which: null as "sel" | null, seen: false, started: 0, info: null as PlayInfo | null, t0: 0, gen: 0 };
  let updatePlay = () => {};
  /** Aktuelle Abspielposition (Viertel) aus Startzeit und Tempo; null = (noch) nichts. */
  const playPos = (): number | null => {
    const i = libPlay.info;
    if (!libPlay.which || !i || !libPlay.t0) return null;
    const q = ((performance.now() - libPlay.t0) / 1000) * (i.tempo / 60);
    const first = i.end - i.start;
    if (q < first) return i.start + q;
    if (!i.looping) return null;
    const len = i.end - i.loopStart;
    return len > 0 ? i.loopStart + ((q - first) % len) : null;
  };
  const animate = () => {
    const q = playPos();
    headEl.hidden = q == null;
    if (q != null) {
      placeAt(headEl, q);
      // mitrollen, wenn die Linie aus dem Bild läuft
      const x = headEl.offsetLeft;
      if (x < scroller.scrollLeft || x > scroller.scrollLeft + scroller.clientWidth - 24) scroller.scrollLeft = Math.max(0, x - 40);
    }
    if (libPlay.which) requestAnimationFrame(animate);
  };
  /** Vorspielen der offenen Partitur ab `from` (Viertel) starten bzw. neu starten. */
  let startPlay = (_from: number) => {};
  const stopPlay = () => {
    libPlay.which = null;
    libPlay.info = null;
    headEl.hidden = true;
    updatePlay();
    void backend.previewStop();
  };
  effect(() => {
    const pv = !!engineStatus.value.previewing;
    if (!libPlay.which) return;
    if (pv) {
      libPlay.seen = true;
      return;
    }
    // vorbei – oder nie angekommen (z. B. gleich wieder ersetzt)
    if (libPlay.seen || (libPlay.started && performance.now() - libPlay.started > 800)) {
      libPlay.which = null;
      libPlay.seen = false;
      libPlay.started = 0;
      libPlay.info = null;
      headEl.hidden = true;
      updatePlay();
    }
  });

  // Zeitfenster markieren: in der Notenrolle ziehen (Schlagraster, mit Umschalt ganze Takte),
  // Kanten lassen sich nachziehen, Klick außerhalb hebt die Markierung auf.
  let updateSel = () => {};
  const setWindow = (win: [number, number] | null) => {
    state.window.value = win;
    redraw();
    updateSel();
  };
  const geometry = (e: PointerEvent) => {
    const s = state.score.peek()!;
    const r = canvas.getBoundingClientRect();
    const qw = qwOf(s);
    const px = e.clientX - r.left + scroller.scrollLeft;
    const step = e.shiftKey ? barQuarters(s) : 1;
    const q = Math.max(0, Math.min(s.durationQuarters, Math.round(px / qw / step) * step));
    const win = state.window.peek();
    const edge = win ? (Math.abs(px - win[0] * qw) < 7 ? 0 : Math.abs(px - win[1] * qw) < 7 ? 1 : -1) : -1;
    return { q, raw: px / qw, win, edge };
  };
  // Zeitleiste: Ziehen wandert in der Zeit, Klick setzt den Cursor,
  // Umschalt/Strg+Ziehen markiert ein Zeitfenster (wie in den Spuren)
  let pan: { x0: number; left0: number; moved: boolean } | null = null;
  const setCursor = (e: PointerEvent) => {
    const s = state.score.peek()!;
    const g = geometry(e);
    const step = e.shiftKey ? barQuarters(s) : 1;
    state.cursor.value = Math.max(0, Math.min(s.durationQuarters, Math.floor(g.raw / step) * step));
    updateSel();
    if (libPlay.which) startPlay(state.cursor.peek());
  };
  ruler.addEventListener("pointerdown", (e) => {
    if (!state.score.peek() || e.button !== 0) return;
    ruler.setPointerCapture(e.pointerId);
    if (e.shiftKey || e.ctrlKey || e.metaKey) {
      const g = geometry(e);
      drag = { anchor: g.q, moved: false };
      return;
    }
    pan = { x0: e.clientX, left0: scroller.scrollLeft, moved: false };
    ruler.style.cursor = "grabbing";
  });
  ruler.addEventListener("pointermove", (e) => {
    if (!state.score.peek()) return;
    if (pan) {
      const dx = e.clientX - pan.x0;
      if (Math.abs(dx) > 3) pan.moved = true;
      if (pan.moved) scroller.scrollLeft = Math.max(0, pan.left0 - dx);
    } else if (drag) {
      const g = geometry(e);
      if (g.q !== drag.anchor) drag.moved = true;
      if (drag.moved) setWindow(g.q === drag.anchor ? null : [Math.min(g.q, drag.anchor), Math.max(g.q, drag.anchor)]);
    }
  });
  ruler.addEventListener("pointerup", (e) => {
    if (state.score.peek() && ((pan && !pan.moved) || (drag && !drag.moved))) setCursor(e);
    pan = null;
    drag = null;
    ruler.style.cursor = "";
  });
  ruler.addEventListener("pointercancel", () => {
    pan = null;
    drag = null;
    ruler.style.cursor = "";
  });

  let drag: { anchor: number; moved: boolean } | null = null;
  canvas.addEventListener("pointerdown", (e) => {
    if (!state.score.peek()) return;
    // mittlere Maustaste: auch in der Rolle in der Zeit wandern
    if (e.button === 1) {
      e.preventDefault();
      canvas.setPointerCapture(e.pointerId);
      pan = { x0: e.clientX, left0: scroller.scrollLeft, moved: true };
      return;
    }
    if (e.button !== 0) return;
    const g = geometry(e);
    // an einer Kante gepackt: die andere bleibt stehen
    const anchor = g.win && g.edge >= 0 ? g.win[1 - g.edge]! : g.q;
    drag = { anchor, moved: g.edge >= 0 };
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!state.score.peek()) return;
    if (pan) {
      scroller.scrollLeft = Math.max(0, pan.left0 - (e.clientX - pan.x0));
      return;
    }
    const g = geometry(e);
    if (!drag) {
      canvas.style.cursor = g.edge >= 0 ? "col-resize" : "crosshair";
      return;
    }
    if (g.q !== drag.anchor) drag.moved = true;
    if (drag.moved) setWindow(g.q === drag.anchor ? null : [Math.min(g.q, drag.anchor), Math.max(g.q, drag.anchor)]);
  });
  canvas.addEventListener("pointerup", (e) => {
    if (pan) {
      pan = null;
      return;
    }
    if (drag && !drag.moved && state.score.peek()) {
      const g = geometry(e);
      if (!g.win || g.raw < g.win[0] || g.raw > g.win[1]) setWindow(null);
      // Cursor setzen (Schlagraster, mit Umschalt Taktanfang); läuft es gerade, von dort weiter
      setCursor(e);
    }
    drag = null;
  });
  effect(() => {
    highOnTop.value; // neu zeichnen, wenn die Richtung der Tonachse wechselt
    if (state.score.peek()) redraw();
  });

  const open = async (hit: LibHit) => {
    try {
      showScore(await backend.libraryOpen(hit.doc, hit.noteIndex, hit.queryLen || null));
    } catch (e) {
      notify(errorText(e), "error");
    }
  };

  // Herunterladen: Katalog auf- und zuklappen; ohne Bibliothek von selbst offen.
  let catalogOpen = false;
  let catalogTouched = false;
  const catalog = catalogView(() => {
    catalogTouched = true;
    setCatalog(false);
  });
  const setCatalog = (open: boolean) => {
    catalogOpen = open;
    catalog.el.hidden = !open;
    downloadBtn.classList.toggle("on", open);
    if (open) {
      setManage(false);
      setAddBox(false);
      catalog.refresh();
    }
  };
  const downloadBtn = h("button", {
    class: "btn small blue",
    "data-help": "lib-herunterladen",
    title: t("Download music library – PDMX, OpenScore, KernScores (Bach, Mozart, Beethoven) and DCML with harmonic analysis"),
    onclick: () => {
      catalogTouched = true;
      setCatalog(!catalogOpen);
    },
  }, icon("download"), t("Download"));
  const cancel = h("button", { class: "btn small", title: t("Cancel download or indexing"), onclick: () => void backend.libraryCancel() }, t("Cancel"));
  setCatalog(false);

  // Warteschlange: beauftragte Pakete, die nach dem laufenden Auftrag drankommen.
  const queueRow = h("div", { class: "lib-queue", hidden: true, "data-help": "lib-warteschlange" });
  const dequeue = (id: string | null, library: string | null) =>
    backend.libraryDequeue(id, library).then((i) => (state.info.value = i)).catch((e) => notify(errorText(e), "error"));
  let queueSig = "";
  const renderQueue = (info: LibraryInfo | null) => {
    const q = info?.queue ?? [];
    const sig = JSON.stringify([info?.id ?? null, q.map((x) => [x.id, x.library])]);
    if (sig === queueSig) return;
    queueSig = sig;
    queueRow.hidden = !q.length;
    cancel.title = q.length
      ? t("Cancel the current job – the queue continues afterwards")
      : t("Cancel download or indexing");
    queueRow.replaceChildren(
      ...(q.length
        ? [
            h("span", { class: "lib-queue-label" }, t("Queue ({n})", { n: q.length })),
            ...q.map((x, i) =>
              h("span", {
                class: "lib-queue-item",
                "data-id": x.id,
                title: t("Position {pos}: will be downloaded after the current job and added to \"{library}\"", { pos: i + 1, library: x.libraryName }),
              },
              h("b", {}, `${i + 1}.`),
              x.name,
              x.library !== info?.id ? h("span", { class: "muted" }, `→ ${x.libraryName}`) : null,
              h("button", { class: "lib-queue-x", title: t("Remove from queue"), onclick: () => void dequeue(x.id, x.library) }, icon("close"))),
            ),
            ...(q.length > 1
              ? [h("button", { class: "btn small ghost lib-queue-clear", title: t("Remove all waiting packs – the current job keeps running"), onclick: () => void dequeue(null, null) }, t("Remove all"))]
              : []),
          ]
        : []),
    );
  };

  // Stöbern: Baum nach Genre, Komponist, Stimmung … (Blatt öffnet die Partitur)
  const tree = treeView((doc) => {
    backend
      .libraryOpen(doc, null, null)
      .then(showScore)
      .catch((e) => notify(errorText(e), "error"));
  });
  // Vorsingen: Mikrofon aufnehmen (nicht ins Projekt), Melodie erkennen, suchen
  const micLabel = h("span", {}, t("Sing it"));
  const micLevel = h("i", { class: "mic-level" });
  const mic = h("button", {
    class: "btn small mic-btn",
    title: t("Sing or whistle a melody – after \"Stop\" it gets searched for (key and tempo don't matter)"),
    onclick: () => void toggleListen(),
  }, icon("mic"), micLabel, micLevel);
  const micCancel = h("button", { class: "btn small ghost", title: t("Discard recording"), hidden: true, onclick: () => void stopListen(true) }, icon("close"));
  let tick = 0;
  const showListening = () => {
    const on = listen.active;
    mic.classList.toggle("on", on);
    micCancel.hidden = !on;
    clearInterval(tick);
    if (!on) {
      micLevel.style.width = "0";
      return;
    }
    tick = window.setInterval(() => {
      if (!mic.isConnected || !listen.active) return clearInterval(tick);
      const secs = (performance.now() - listen.since) / 1000;
      micLabel.textContent = t("Stop · {secs} s", { secs: Math.floor(secs) });
      // Pegel des Mikrofons (Spitzenwert seit dem letzten Statusbericht)
      micLevel.style.width = `${Math.round(Math.min(1, Math.sqrt(engineStatus.peek().peakIn) * 1.4) * 100)}%`;
      if (secs >= LISTEN_MAX_SECS) void stopListen(false);
    }, 120);
  };
  const toggleListen = async () => {
    if (listen.active) return stopListen(false);
    try {
      await backend.libraryListenStart();
    } catch (e) {
      notify(errorText(e), "error");
      return;
    }
    listen.active = true;
    listen.since = performance.now();
    showListening();
    notify(t("Sing or whistle the melody (at least 5 notes) – then \"Stop\"."));
  };
  const stopListen = async (cancel: boolean) => {
    if (!listen.active) return;
    listen.active = false;
    showListening();
    micLabel.textContent = cancel ? t("Sing it") : t("Analyzing…");
    mic.disabled = true;
    try {
      const r = await backend.libraryListenStop(text.value.trim() || null, cancel);
      if (!cancel) {
        melody.value = chordsText(r.melody);
        state.melody = melody.value;
        state.hits.value = r.hits;
        const key = r.key ?? "";
        notify(r.key
          ? r.hits.length
            ? plural(r.notes, "Detected {n} note ({key}). You can correct it in the melody field.", "Detected {n} notes ({key}). You can correct them in the melody field.", { key })
            : plural(r.notes, "Detected {n} note ({key}) – no matches. You can correct it in the melody field.", "Detected {n} notes ({key}) – no matches. You can correct them in the melody field.", { key })
          : r.hits.length
            ? plural(r.notes, "Detected {n} note. You can correct it in the melody field.", "Detected {n} notes. You can correct them in the melody field.")
            : plural(r.notes, "Detected {n} note – no matches. You can correct it in the melody field.", "Detected {n} notes – no matches. You can correct them in the melody field."));
      }
    } catch (e) {
      notify(errorText(e), "error");
    } finally {
      mic.disabled = false;
      micLabel.textContent = t("Sing it");
    }
  };
  if (listen.active) showListening();

  const searchGroup = h("div", { class: "lib-search" }, icon("search"), text, melody,
    h("button", { class: "btn small primary", onclick: () => void search() }, t("Search")),
    h("button", { class: "btn small", title: t("Search for the melody of the selected clip (notes or recording)"), onclick: () => void search(true) }, t("Search selection")),
    mic, micCancel);
  const modeSeg = h("div", { class: "seg mini lib-mode" });
  const left = h("div", { class: "lib-left" }, results, tree.el);
  const setMode = (m: "suchen" | "stoebern") => {
    libMode = m;
    try {
      localStorage.setItem(MODE_KEY, m);
    } catch {
      /* ohne Speicher */
    }
    for (const b of modeSeg.children) b.classList.toggle("on", (b as HTMLElement).dataset.m === m);
    searchGroup.hidden = m !== "suchen";
    tree.bar.hidden = m !== "stoebern";
    results.hidden = m !== "suchen";
    tree.el.hidden = m !== "stoebern";
  };
  for (const [m, name, tip] of [["suchen", t("Search"), t("Search by title or melody")], ["stoebern", t("Browse"), t("Tree by genre, composer, mood, instrumentation…")]] as const) {
    const b = h("button", { class: "seg-btn", title: tip, onclick: () => setMode(m) }, name);
    b.dataset.m = m;
    modeSeg.append(b);
  }

  const el = h(
    "div",
    { class: "library" },
    h("div", { class: "lib-bar" }, libName, status, progress, cancel, addBtn, downloadBtn),
    queueRow,
    addBox,
    catalog.el,
    manage.el,
    h("div", { class: "lib-bar" }, modeSeg, searchGroup, tree.bar),
    h("div", { class: "lib-body" }, left, h("div", { class: "lib-right" }, detail, scroller)),
  );
  setMode(libMode);
  dropTarget = el;

  let lastSig: string | null = null;
  let renderedHits: LibHit[] | null = null;
  let renderedScore: ScoreView | null = null;
  let windowDoc: number | null = state.score.peek()?.doc ?? null;
  let renderedVoices: number[] | null = null;
  return {
    el,
    update() {
      const info = state.info.value;
      const dl = info?.download ?? null;
      let frac: number | null = null;
      tree.setInfo(info);
      const name = info?.name ?? null;
      const nameEl = libName.lastElementChild as HTMLElement;
      if (nameEl.textContent !== (name ?? "…")) nameEl.textContent = name ?? "…";
      const merging = info?.merging ?? null;
      if (dl && dl.phase === "Kategorien") {
        frac = dl.bytes / dl.totalBytes;
        status.textContent = info?.message ?? t("{name}: genre & composer… {done} of {total}", { name: dl.name, done: mb(dl.bytes), total: mb(dl.totalBytes) });
      } else if (dl) {
        // Pakete: Fortschritt nach Partituren; ohne Grenze nach geladenen Bytes.
        frac = dl.target ? dl.files / dl.target : dl.bytes / dl.totalBytes;
        status.textContent = info?.message
          ?? (dl.target
            ? t("{name}: {files} / {target} scores · {bytes}", { name: dl.name, files: fmt(dl.files), target: fmt(dl.target), bytes: mb(dl.bytes) })
            : t("{name}: {files} scores · {bytes} of {total}", { name: dl.name, files: fmt(dl.files), bytes: mb(dl.bytes), total: mb(dl.totalBytes) }));
      } else if (merging != null) {
        frac = merging;
        status.textContent = info?.message ?? t("Adding to \"{library}\"… {percent}%", { library: name ?? "…", percent: Math.round(merging * 100) });
      } else if (info?.building) {
        const [done, total] = info.building;
        frac = total ? done / total : 0.02;
        // message of the desktop shell: "<prefix>Indexing {what}…" – keep the prefix, append the progress here
        const marker = t("Indexing {what}…").split("{what}")[0]!.trim();
        const at = info.message && marker ? info.message.indexOf(marker) : -1;
        const lead = at >= 0 ? info.message!.slice(0, at) : "";
        status.textContent = `${lead}${t("Indexing… {done} / {total} files", { done: fmt(done), total: fmt(total) })}`;
      } else if (info?.adding) {
        frac = 0.3;
        status.textContent = info.message ?? t("Adding…");
      } else if (info?.loading) {
        frac = 0.05;
        status.textContent = info.message ?? t("Loading the library…");
      } else {
        status.textContent = info
          ? info.message ?? (info.id
            ? info.scores
              ? t("{scores} scores · {voices} voices", { scores: fmt(info.scores), voices: fmt(info.voices) })
              : t("Still empty – \"Download\" or add scores.")
            : t("No library loaded."))
          : "";
      }
      status.title = dl?.folder ?? "";
      renderQueue(info);
      catalog.sync(info);
      progress.style.display = frac != null ? "" : "none";
      cancel.hidden = frac == null || !!info?.loading;
      if (frac != null) (progress.firstElementChild as HTMLElement).style.width = `${Math.max(2, Math.min(100, Math.round(frac * 100)))}%`;
      // Gewechselt oder gewachsen: Treffer/Partitur gehören ggf. zur alten Bibliothek, Katalog und Liste neu
      const sig = `${info?.id ?? ""}|${info?.scores ?? 0}`;
      if (info && sig !== lastSig) {
        const switched = lastSig !== null && lastSig.split("|")[0] !== (info.id ?? "");
        lastSig = sig;
        if (switched) {
          state.hits.value = [];
          state.score.value = null;
        }
        if (catalogOpen) catalog.refresh();
        if (!manage.el.hidden) manage.refresh();
      }
      // Leere Bibliothek: den Katalog von selbst zeigen (bis man ihn schließt).
      if (info?.id && !info.loading && !info.scores && !dl && !info.building && merging == null && !info.adding && !catalogOpen && !catalogTouched && manage.el.hidden) setCatalog(true);

      const hits = state.hits.value;
      if (hits !== renderedHits) {
        renderedHits = hits;
        results.replaceChildren(
          ...(hits.length
            ? hits.map((hit) =>
                h(
                  "button",
                  { class: "lib-hit", onclick: () => void open(hit), title: hit.source },
                  h("div", { class: "t" }, hit.title || t("(untitled)"), hit.composer ? h("span", {}, ` – ${hit.composer}`) : null),
                  h("div", { class: "s" }, `${hit.part || t("Voice")} · ${hit.voice}`,
                    hit.score != null ? h("b", {}, `${Math.round(hit.score * 100)} %`) : h("b", {}, plural(hit.noteCount, "{n} note", "{n} notes"))),
                  hit.score != null ? h("i", { style: `width:${Math.round(hit.score * 100)}%` }) : null,
                ),
              )
            : [h("div", { class: "lib-empty" }, t("Search by title or melody – or select a clip and click \"Search selection\"."))]),
        );
      }

      const s = state.score.value;
      const sel = state.voices.value;
      if (s !== renderedScore || sel !== renderedVoices) {
        const newScore = s !== renderedScore;
        if (s?.doc !== windowDoc) {
          // andere Partitur: Markierung gilt nicht mehr
          windowDoc = s?.doc ?? null;
          state.window.value = null;
        }
        renderedScore = s;
        renderedVoices = sel;
        if (!s) {
          detail.replaceChildren(h("div", { class: "lib-empty" }, t("Click a match to see the score.")));
          content.style.width = "0px";
          if (libPlay.which) stopPlay();
          updateSel = () => {};
        } else {
          const n = s.voices.length;
          const allIdx = s.voices.map((_, i) => i);
          const all = sel.length === n;
          const names = (idx: number[]) => idx.map((i) => s.voices[i]?.part || s.voices[i]?.label || t("Voice"));
          const whoLabel = all ? (n > 1 ? t("all voices") : names(sel)[0]!) : sel.length === 1 ? names(sel)[0]! : plural(sel.length, "{n} voice", "{n} voices");
          const takeLabel = h("span", {}, "");
          const take = h("button", {
            class: "btn small primary",
            disabled: !sel.length,
            onclick: () => {
              const win = state.window.peek();
              const bpm = tempoNow();
              const vars = { voice: names(sel)[0] ?? "", n: sel.length, bars: win ? barLabel(s, win) : "", bpm: Math.round(bpm) };
              const msg = sel.length === 1
                ? win
                  ? t("Added \"{voice}\" ({bars}) as a track – at the project tempo it sounds like ♩ = {bpm}", vars)
                  : t("Added \"{voice}\" as a track – at the project tempo it sounds like ♩ = {bpm}", vars)
                : win
                  ? t("Added {n} voices ({bars}) as tracks – at the project tempo they sound like ♩ = {bpm}", vars)
                  : t("Added {n} voices as tracks – at the project tempo they sound like ♩ = {bpm}", vars);
              void act(backend.libraryToTrack(s.doc, voicePairs(s, sel), win, bpm)).then((ok) => ok && notify(msg));
            },
          }, icon("plus"), takeLabel);
          // Tempo: gewählt oder das der Partitur – gilt fürs Anhören und fürs Übernehmen
          const tempoNow = () => state.tempo.peek() ?? scoreTempo(s);
          // Anhören ab dem Cursor bis zum Ende – mit Markierung diese immer wieder –, bis „Stopp“
          startPlay = (from: number) => {
            const gen = ++libPlay.gen;
            const first = !libPlay.which;
            Object.assign(libPlay, { which: "sel", seen: first ? false : libPlay.seen, started: first ? 0 : libPlay.started });
            updatePlay();
            backend
              .libraryPlay(s.doc, from, state.window.peek(), all ? null : voicePairs(s, sel), tempoNow())
              .then((info) => {
                if (!libPlay.which || gen !== libPlay.gen) return;
                libPlay.info = info;
                libPlay.started = libPlay.t0 = performance.now();
                requestAnimationFrame(animate);
              })
              .catch((e) => {
                if (gen !== libPlay.gen) return;
                notify(errorText(e), "error");
                libPlay.which = null;
                libPlay.info = null;
                updatePlay();
              });
          };
          // Klick spielt die gewählten Stimmen, nochmal Klick (dann „Stopp“) beendet
          const toggle = () => (libPlay.which ? stopPlay() : startPlay(state.cursor.peek()));
          // Tempo ändern: läuft es, an der aktuellen Stelle im neuen Tempo weiter
          const setTempo = (bpm: number | null) => {
            state.tempo.value = bpm == null ? null : Math.max(TEMPO_MIN, Math.min(TEMPO_MAX, Math.round(bpm)));
            updateTempo();
            const pos = playPos();
            if (libPlay.which) startPlay(pos ?? state.cursor.peek());
          };
          const tempoIn = h("input", {
            type: "number", class: "lib-tempo-in", min: String(TEMPO_MIN), max: String(TEMPO_MAX), step: "1",
            title: t("Tempo in quarter notes per minute – for listening and for adding"),
          }) as HTMLInputElement;
          tempoIn.addEventListener("keydown", (e) => {
            e.stopPropagation();
            if (e.key === "Enter") tempoIn.blur();
          });
          tempoIn.addEventListener("change", () => {
            const v = Number(tempoIn.value);
            if (Number.isFinite(v) && v > 0) setTempo(v);
            else updateTempo();
          });
          const step = (d: number) => h("button", {
            class: "btn small ghost", title: d > 0 ? t("Faster (+{pct}%)", { pct: Math.abs(d) }) : t("Slower (−{pct}%)", { pct: Math.abs(d) }),
            onclick: () => setTempo(tempoNow() * (1 + d / 100)),
          }, d > 0 ? "+" : "−");
          const original = h("button", { class: "btn small ghost lib-tempo-orig", onclick: () => setTempo(null) }, t("Original"));
          const toProject = h("button", {
            class: "btn small ghost lib-tempo-proj",
            title: t("Use the project's tempo – then each quarter note becomes exactly one beat when adding"),
            onclick: () => setTempo(project()?.tempoBpm ?? 120),
          }, t("Project"));
          const tempoBox = h("span", { class: "lib-tempo" }, h("span", { class: "lbl" }, "♩ ="), step(-10), tempoIn, step(10), original, toProject);
          const updateTempo = () => {
            const bpm = tempoNow();
            if (document.activeElement !== tempoIn) tempoIn.value = String(Math.round(bpm));
            const orig = scoreTempo(s);
            original.hidden = state.tempo.peek() == null || Math.round(bpm) === Math.round(orig);
            original.title = s.tempoBpm
              ? t("Back to the score's tempo (♩ = {bpm})", { bpm: Math.round(orig) })
              : t("Back to the score's tempo (♩ = {bpm}, not specified)", { bpm: Math.round(orig) });
            const pb = project()?.tempoBpm ?? 120;
            tempoBox.classList.toggle("changed", Math.round(bpm) !== Math.round(orig));
            tempoBox.title = Math.round(bpm) === Math.round(pb)
              ? t("Listen at ♩ = {bpm}; when added, it's stretched so it sounds just as fast at the project tempo ({projectBpm} BPM) – one quarter note per beat",
                { bpm: Math.round(bpm), projectBpm: Math.round(pb) })
              : t("Listen at ♩ = {bpm}; when added, it's stretched so it sounds just as fast at the project tempo ({projectBpm} BPM) – one quarter note = {beats} beats",
                { bpm: Math.round(bpm), projectBpm: Math.round(pb), beats: Math.round((pb / bpm) * 100) / 100 });
          };
          updateTempo();
          let playText = t("Listen");
          const playIcon = h("span", { class: "ico" }, icon("play"));
          const playLabel = h("span", {}, playText);
          const play = h("button", { class: "btn small", onclick: toggle }, playIcon, playLabel);
          updatePlay = () => {
            const on = !!libPlay.which;
            playIcon.replaceChildren(icon(on ? "stop" : "play"));
            playLabel.textContent = on ? t("Stop") : playText;
            play.classList.toggle("playing", on);
          };
          const selInfo = h("span", { class: "lib-selinfo" });
          const clear = h("button", { class: "btn small ghost", title: t("Clear selection"), onclick: () => setWindow(null) }, icon("close"));
          updateSel = () => {
            const win = state.window.peek();
            const bar = Math.floor(state.cursor.peek() / barQuarters(s)) + 1;
            playText = `${win ? t("Marked range") : t("From bar {bar}", { bar })} · ${whoLabel}`;
            updatePlay();
            const voices = all ? t("all voices") : names(sel).join(", ");
            play.title = win
              ? t("Loop {bars} – {voices}\nClick in the note roll to set the cursor · Click here while playing: stop", { bars: barLabel(s, win), voices })
              : t("Play from the cursor (bar {bar}) to the end – {voices}\nClick in the note roll to set the cursor · Click here while playing: stop", { bar, voices });
            selInfo.textContent = win
              ? t("Selected: {bars} · {quarters} quarter notes", { bars: barLabel(s, win), quarters: Math.round((win[1] - win[0]) * 100) / 100 })
              : t("Drag the timeline: move through time · Ctrl+mouse wheel: zoom · Note roll: click sets the cursor, dragging selects a range to loop (Shift: whole bars)");
            selInfo.classList.toggle("on", !!win);
            clear.hidden = !win;
            const many = sel.length > 1;
            takeLabel.textContent = win
              ? many ? t("Section as tracks") : t("Section as track")
              : many ? t("{n} voices as tracks", { n: sel.length }) : t("Voice as track");
            const pickVars = { bars: win ? barLabel(s, win) : "", voices: names(sel).join(", "), voice: names(sel)[0] ?? "" };
            take.title = win
              ? many ? t("Only {bars} – one track per voice: {voices}", pickVars) : t("Only {bars} – add \"{voice}\" as a track", pickVars)
              : many ? t("one track per voice: {voices}", pickVars) : t("Add \"{voice}\" as a track", pickVars);
          };
          updateSel();
          // Klick: nur diese Stimme · Strg/Umschalt+Klick: hinzunehmen bzw. abwählen (mindestens eine bleibt)
          const pick = (i: number, e: MouseEvent) => {
            const cur = state.voices.peek();
            if (e.ctrlKey || e.metaKey || e.shiftKey) {
              const next = cur.includes(i) ? cur.filter((x) => x !== i) : [...cur, i].sort((a, b) => a - b);
              if (next.length) state.voices.value = next;
            } else {
              state.voices.value = [i];
            }
          };
          detail.replaceChildren(
            h("div", { class: "lib-title" }, s.title, s.composer ? h("span", {}, ` – ${s.composer}`) : null),
            h("div", { class: "lib-facts" },
              [s.keyName, s.timeSignature ? `${s.timeSignature[0]}/${s.timeSignature[1]}` : null, s.tempoBpm ? `${Math.round(s.tempoBpm)} BPM` : null, s.mood]
                .filter(Boolean).join(" · ")),
            h("div", { class: "lib-voices", title: t("Click: one voice · Ctrl+click: select several voices") },
              // Alle Stimmen auf einmal wählen – Anhören und Übernehmen gelten dann für den ganzen Satz
              n > 1
                ? h("button", {
                    class: `btn small lib-allvoices${all ? " on" : ""}`,
                    title: all ? t("All voices are selected – click: back to the matching voice") : t("Select all voices (for listening and adding)"),
                    onclick: () => {
                      const hitIdx = s.voices.findIndex((v) => v.isHit);
                      state.voices.value = all ? [Math.max(0, hitIdx)] : allIdx;
                    },
                  }, h("i"), t("All voices"))
                : null,
              ...s.voices.map((v, i) =>
                h("button", {
                  class: `lib-voice${sel.includes(i) ? " on" : ""}`,
                  style: `--vc:${VOICE_COLORS[i % VOICE_COLORS.length]}`,
                  onclick: (e: MouseEvent) => pick(i, e),
                  title: `${v.label}\n${t("Click: only this voice · Ctrl+click: add/remove")}`,
                }, h("i"), v.part || v.label, v.role ? h("span", {}, v.role) : null, v.isHit ? h("b", {}, t("Match")) : null),
              ),
              take),
            h("div", { class: "lib-selbar" }, play, tempoBox, selInfo, clear),
          );
          // Läuft gerade die Wiedergabe: andere Stimmen an derselben Stelle weiterspielen,
          // eine andere Partitur beendet sie
          if (libPlay.which) {
            if (newScore) stopPlay();
            else startPlay(playPos() ?? state.cursor.peek());
          }
          if (newScore) zoom = 14;
          requestAnimationFrame(() => {
            redraw();
            // nur bei einer neuen Partitur zur Fundstelle springen, nicht beim Umwählen der Stimmen
            const hl = s.range ?? s.voices.find((v) => v.highlight)?.highlight;
            if (newScore) scroller.scrollLeft = hl ? Math.max(0, hl[0] * qwOf(s) - 40) : 0;
            redraw();
          });
        }
      }
    },
  };
}

/** Partitur im Bibliotheksfenster öffnen und eine Stelle hervorheben (z. B. ein Beispiel aus dem Quintenzirkel). */
export async function openInLibrary(doc: number, range: [number, number] | null): Promise<void> {
  try {
    showScore(await backend.libraryOpen(doc, null, null, range));
    showPanel("bibliothek");
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** Melodiesuche von außen (z. B. Fugenthema) – Treffer erscheinen im Fenster „Bibliothek“. */
export async function searchLibraryFor(q: { trackId: number; clipId: number; text?: string }): Promise<void> {
  try {
    state.hits.value = await backend.librarySearch({ trackId: q.trackId, clipId: q.clipId, text: q.text ?? "" });
    state.text = q.text ?? "";
    showPanel("bibliothek");
    const found = state.hits.peek().length;
    notify(found ? plural(found, "Found {n} similar passage", "Found {n} similar passages") : t("No similar subjects found."));
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** Signale, auf die der Inspektor reagieren muss. */
export function librarySignals(): void {
  state.info.value;
  state.hits.value;
  state.score.value;
  state.voices.value;
}
