// Fenster „Harmonie“: Quintenzirkel mit Fortsetzungen zum Ausprobieren.
// Ein Klick auf einen Akkord (im Zirkel oder in der Liste) spielt den Übergang
// vom letzten Akkord des Verlaufs vor, Doppelklick bzw. „+“ übernimmt ihn.
// Vorschläge kommen aus der Funktionsharmonik (Rust-Kern) und tragen, wenn
// eine Bibliothek mit Akkordstatistik geladen ist, ihre Häufigkeit dort samt
// Beispielstellen zum Anhören und Öffnen.
// Übergänge sind nach Stimmung gefärbt (lösend, spannend, überraschend, hell,
// dunkel – Legende unter dem Zirkel). „Wege zum Schluss“ zeigt mehrschrittige
// Kadenzen vom aktuellen Akkord aus; Maus darüber zeichnet den Weg nummeriert
// in den Zirkel, ein Klick hält ihn fest.
// Der Verlauf folgt der Abspielposition (beim Abspielen und Cursor-Setzen):
// Er beginnt mit den Akkorden des Satzes bis zur Stelle, eigene neue Akkorde
// bleiben dahinter. „In Spur schreiben“ setzt sie hinter den Akkord am Cursor –
// einfügen (alles danach rückt nach hinten) oder das Nachfolgende ersetzen.

import { backend } from "../bridge.js";
import { h, icon } from "../dom.js";
import { act, errorText, findClip, notify, project, selection, snapshot, status } from "../store.js";
import { chordIndexAt, harmonyFocus, harmonyKey, harmonyLine, watchHarmonyLine } from "../harmonyline.js";
import { effect } from "../signals.js";
import { openInLibrary } from "./library.js";
import type { CadencePath, Chord, ChordInfo, ChordSuggestion, HarmonyExample, HarmonyView, KeySpec, Mood, Snapshot, SuggestCategory, SuggestResult, Track } from "../types.js";

interface SketchItem extends ChordInfo {
  /** schon in der Spur (aus dem Clip gelesen oder geschrieben) */
  fixed?: boolean;
  /** nur der Anfang (Tonika), kein eigener Akkord – fällt weg, wenn der Verlauf der Abspielposition folgt */
  seed?: boolean;
  startBeats?: number;
  lengthBeats?: number;
}

const MAJOR = ["C", "Des", "D", "Es", "E", "F", "Fis", "G", "As", "A", "B", "H"];
const MINOR = ["c", "cis", "d", "es", "e", "f", "fis", "g", "gis", "a", "b", "h"];
const keyName = (k: KeySpec) => (k.minor ? `${MINOR[k.tonic]}-Moll` : `${MAJOR[k.tonic]}-Dur`);

const CATEGORIES: Array<[SuggestCategory, string, string]> = [
  ["schluss", "Schließen", "Ganzschluss, Plagalschluss, Trugschluss"],
  ["spannung", "Spannung", "führt zur Dominante"],
  ["weiter", "Weiterführen", "bleibt offen, ohne zu schließen"],
  ["farbe", "Farbe", "Zwischendominanten, entlehnte Akkorde, Neapolitaner"],
  ["ausweichen", "Ausweichen", "öffnet eine Nachbartonart"],
  ["haeufig", "Häufig in der Bibliothek", "keine Regel, aber oft so gesetzt"],
];

/** Stimmungen der Übergänge: Farbe (CSS-Klasse m-…), Name, Erklärung. */
const MOODS: Array<[Mood, string, string]> = [
  ["loesend", "lösend", "Spannung löst sich: Dominante → Tonika (oder Ziel der Zwischendominante), Leitton aufwärts, Plagalschluss"],
  ["spannend", "spannend", "baut Spannung auf: zur Dominante, Doppel- oder Zwischendominante, Quartsextvorhalt"],
  ["ueberraschend", "überraschend", "unerwartet: Trugschluss, chromatische Terzverwandtschaft (z. B. C → As)"],
  ["hell", "hell", "zu einem Durakkord der Tonart"],
  ["dunkel", "dunkel", "zu Moll oder vermindert, oder aus der Molltonart entlehnt"],
];
const moodLabel = (m: Mood) => MOODS.find((x) => x[0] === m)?.[1] ?? m;

/** Was zusätzlich im Zirkel gezeichnet wird: ein Weg zum Schluss oder eine Kadenzvorlage. */
interface Overlay {
  kind: "path" | "cadence";
  id: string;
}
const pathId = (p: CadencePath) => `${p.goal}${p.longer ? "+" : ""}`;

/** Zustand bleibt erhalten, wenn das Fenster neu aufgebaut wird. */
const st = {
  keyChoice: null as KeySpec | null,
  view: null as HarmonyView | null,
  sketch: [] as SketchItem[],
  candidate: null as ChordInfo | null,
  result: null as SuggestResult | null,
  barsPerChord: 1,
  target: "akkorde" as "akkorde" | number,
  expanded: null as string | null,
  /** festgehaltener Weg bzw. festgehaltene Kadenz (Klick / Abspielen) */
  pinned: null as Overlay | null,
  /** Statistik-Gruppe: 0 = alle Stücke, 1 = Barock, 2 = Bach */
  group: 0,
  /** Verlauf greift die Harmonie an der Abspielposition auf */
  follow: true,
  /** Schreiben: Platz schaffen (alles danach rückt nach hinten) oder Nachfolgendes ersetzen */
  insertMode: "einfuegen" as "einfuegen" | "ersetzen",
};
/** gerade mit der Maus gezeigt (hat Vorrang vor dem festgehaltenen) */
let hover: Overlay | null = null;
const sameOverlay = (a: Overlay | null, b: Overlay | null) => !!a && !!b && a.kind === b.kind && a.id === b.id;

const same = (a: Chord, b: Chord) => a.root === b.root && a.quality === b.quality && (a.bass ?? -1) === (b.bass ?? -1);
const ckey = (c: Chord) => `${c.root}|${c.quality}|${c.bass ?? ""}`;
const pct = (x: number) => (x >= 0.095 ? Math.round(x * 100) : Math.round(x * 1000) / 10).toLocaleString("de-DE");

function triadClass(c: Chord): 0 | 1 | 2 | null {
  switch (c.quality) {
    case "major": case "dom7": case "maj7": return 0;
    case "minor": case "min7": return 1;
    case "dim": case "halfDim7": case "dim7": return 2;
    default: return null;
  }
}

/** Platz im Zirkel: [Ring, Position] – Septakkorde beim Dreiklang. */
function cellOf(c: Chord): [number, number] | null {
  const ring = triadClass(c);
  const posOf = (r: number) => (r * 7) % 12;
  if (ring === 0) return [0, posOf(c.root)];
  if (ring === 1) return [1, posOf((c.root + 3) % 12)];
  if (ring === 2) return [2, posOf((c.root + 1) % 12)];
  return null;
}

function melodicTracks(): Track[] {
  return (project()?.tracks ?? []).filter((t) => t.kind === "instrument" && t.instrument?.kind !== "drums");
}

function beatsPerBar(): number {
  return project()?.beatsPerBar ?? 4;
}

function currentKey(): KeySpec {
  const k = st.view?.key;
  return k ? { tonic: k.tonic, minor: k.minor } : { tonic: 0, minor: false };
}

function targetTrackId(): number | null {
  return typeof st.target === "number" && melodicTracks().some((t) => t.id === st.target) ? st.target : null;
}

// ---------------------------------------------------------------- SVG

const NS = "http://www.w3.org/2000/svg";
function svg<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}, ...kids: Array<Node | string>): SVGElementTagNameMap[K] {
  const el = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  for (const c of kids) el.append(c);
  return el;
}

const RINGS: Array<[number, number]> = [[150, 206], [100, 150], [64, 100]];
const pt = (r: number, deg: number): [number, number] => {
  const t = (deg * Math.PI) / 180;
  return [Math.round(r * Math.cos(t) * 100) / 100, Math.round(r * Math.sin(t) * 100) / 100];
};
function sector(ring: number, pos: number): string {
  const [r0, r1] = RINGS[ring]!;
  const a0 = pos * 30 - 105;
  const a1 = a0 + 30;
  const [x0, y0] = pt(r1, a0), [x1, y1] = pt(r1, a1), [x2, y2] = pt(r0, a1), [x3, y3] = pt(r0, a0);
  return `M${x0} ${y0}A${r1} ${r1} 0 0 1 ${x1} ${y1}L${x2} ${y2}A${r0} ${r0} 0 0 0 ${x3} ${y3}Z`;
}
const centerOf = (ring: number, pos: number) => pt((RINGS[ring]![0] + RINGS[ring]![1]) / 2, pos * 30 - 90);
const r2 = (x: number) => Math.round(x * 10) / 10;

/** Bogen von Feld a nach Feld b (zur Mitte gekrümmt, Enden eingekürzt); gleiches Feld: Schleife. */
function segPath(a: [number, number], b: [number, number]): string {
  const [x0, y0] = centerOf(a[0], a[1]);
  const [x1, y1] = centerOf(b[0], b[1]);
  if (a[0] === b[0] && a[1] === b[1]) {
    // Schleife: im äußeren Ring nach innen, sonst nach außen
    const len = Math.hypot(x0, y0) || 1;
    const dir = a[0] === 0 ? -1 : 1;
    const ux = (x0 / len) * dir, uy = (y0 / len) * dir;
    const px = -uy, py = ux;
    const p = (u: number, v: number) => `${r2(x0 + ux * u + px * v)} ${r2(y0 + uy * u + py * v)}`;
    return `M${p(8, 6)}C${p(34, 20)} ${p(34, -20)} ${p(10, -7)}`;
  }
  const cx = ((x0 + x1) / 2) * 0.62, cy = ((y0 + y1) / 2) * 0.62;
  const toward = (x: number, y: number, by: number): [number, number] => {
    const dx = cx - x, dy = cy - y;
    const l = Math.hypot(dx, dy) || 1;
    return [x + (dx / l) * by, y + (dy / l) * by];
  };
  const [sx, sy] = toward(x0, y0, 7);
  const [ex, ey] = toward(x1, y1, 12);
  return `M${r2(sx)} ${r2(sy)}Q${r2(cx)} ${r2(cy)} ${r2(ex)} ${r2(ey)}`;
}

/**
 * Weg im Zirkel: Bögen nach Stimmung gefärbt (moods[i] für chords[i] → chords[i+1])
 * und nummerierte Schritte. skipFirst: chords[0] ist der aktuelle Akkord (ohne Nummer).
 */
function route(chords: Chord[], moods: Mood[], skipFirst: boolean): SVGGElement {
  const g = svg("g", { class: "hc-route" });
  const cells = chords.map(cellOf);
  for (let i = 0; i + 1 < cells.length; i++) {
    const a = cells[i], b = cells[i + 1];
    if (!a || !b) continue;
    const m = moods[i] ?? "hell";
    const d = segPath(a, b);
    g.append(svg("path", { d, class: "halo" }), svg("path", { d, class: `seg m-${m}`, "data-mood": m, "marker-end": `url(#hc-mk-${m})` }));
  }
  const used = new Map<string, number>();
  cells.forEach((c, i) => {
    if (!c || (skipFirst && i === 0)) return;
    const k = `${c[0]}:${c[1]}`;
    const n = used.get(k) ?? 0;
    used.set(k, n + 1);
    const [x, y] = centerOf(c[0], c[1]);
    const m = i > 0 ? moods[i - 1] : undefined;
    // derselbe Akkord mehrmals im Weg: Nummern nebeneinander
    const bx = x + n * 15, by = y - 17;
    g.append(svg("g", { class: `step${m ? ` m-${m}` : ""}`, ...(m ? { "data-mood": m } : {}) },
      svg("circle", { cx: r2(bx), cy: r2(by), r: 7.5 }), svg("text", { x: r2(bx), y: r2(by + 3.3) }, String(skipFirst ? i : i + 1))));
  });
  return g;
}

// ---------------------------------------------------------------- Panel

let libraryListening = false;
let onLibrary: () => void = () => {};

export function harmonyPanel(): { el: HTMLElement; update(): void } {
  const keySel = h("select", { class: "hc-select", title: "Tonart – automatisch aus den Noten geschätzt oder fest gewählt" });
  keySel.addEventListener("change", () => {
    const v = keySel.value;
    st.keyChoice = v === "auto" ? null : { tonic: Number(v.split(":")[0]), minor: v.endsWith(":m") };
    harmonyKey.value = st.keyChoice;
    void loadView();
  });
  const durSeg = h("div", { class: "seg mini", title: "Dauer je Akkord beim Abspielen und Schreiben" });
  for (const [bars, label] of [[0.5, "½ Takt"], [1, "1 Takt"], [2, "2 Takte"]] as const) {
    const b = h("button", { class: "seg-btn" }, label);
    b.dataset.v = String(bars);
    b.addEventListener("click", () => {
      st.barsPerChord = bars;
      for (const x of durSeg.children) x.classList.toggle("on", (x as HTMLElement).dataset.v === String(bars));
    });
    b.classList.toggle("on", bars === st.barsPerChord);
    durSeg.append(b);
  }
  const targetSel = h("select", { class: "hc-select", title: "Wohin „In Spur schreiben“ die Akkorde setzt" });
  targetSel.addEventListener("change", () => {
    st.target = targetSel.value === "akkorde" ? "akkorde" : Number(targetSel.value);
  });

  const circleBox = h("div", { class: "hc-circle" });
  const sketchBox = h("div", { class: "hc-sketch" });
  const libLine = h("div", { class: "hc-lib" });
  const suggBox = h("div", { class: "hc-sugg" });
  const cadBox = h("div", { class: "hc-cad" });
  const pathBox = h("div", { class: "hc-paths" });
  const pathFrom = h("span", { class: "muted" });

  /** Legende: Maus über eine Stimmung hebt ihre Übergänge hervor. */
  const focusMood = (m: Mood | null) => {
    for (const e of el.querySelectorAll<HTMLElement | SVGElement>("[data-mood]")) e.classList.toggle("faded", m !== null && e.dataset.mood !== m);
  };
  const legend = h("div", { class: "hc-legend" },
    h("span", { class: "hc-lg-title" }, "Stimmung"),
    ...MOODS.map(([m, label, desc]) => {
      const item = h("span", { class: `hc-lg m-${m}`, title: `${label}: ${desc}` }, h("i"), label);
      item.addEventListener("mouseenter", () => focusMood(m));
      item.addEventListener("mouseleave", () => focusMood(null));
      return item;
    }));

  const field = (label: string, control: HTMLElement) => h("span", { class: "hc-field" }, h("label", {}, label), control);
  const pathSection = h("div", { class: "hc-pathsec" },
    h("div", { class: "hc-title", title: "Mehrschrittige Kadenzen vom aktuellen Akkord aus – je Schlussart der beste Weg, darunter ggf. ein längerer.\nMaus darüber: im Zirkel zeigen · Klick: festhalten" }, "Wege zum Schluss", pathFrom),
    pathBox);
  const left = h("div", { class: "hc-left" }, circleBox, legend, pathSection);
  const side = h("div", { class: "hc-side" },
    sketchBox,
    h("div", { class: "hc-title" }, "Fortsetzungen", libLine),
    suggBox,
    h("div", { class: "hc-title" }, "Kadenzen"),
    cadBox);
  const bodyEl = h("div", { class: "hc-body" }, left, side);
  const el = h(
    "div",
    { class: "harmony" },
    h("div", { class: "hc-head" }, field("Tonart", keySel), field("Dauer", durSeg), field("Ziel", targetSel)),
    bodyEl,
  );
  // Breit (zwei Spalten): die Wege zum Schluss rechts unter den Verlauf – links
  // bleiben Zirkel und Legende allein und beim Scrollen stehen. Schmal: unter dem Zirkel.
  const placePaths = () => {
    const wide = getComputedStyle(bodyEl).gridTemplateColumns.trim().split(/\s+/).length > 1;
    if (wide && pathSection.parentElement !== side) side.insertBefore(pathSection, sketchBox.nextSibling);
    else if (!wide && pathSection.parentElement !== left) left.append(pathSection);
    el.classList.toggle("hc-wide", wide);
  };
  let placeQueued = false;
  new ResizeObserver(() => {
    if (placeQueued) return;
    placeQueued = true;
    requestAnimationFrame(() => {
      placeQueued = false;
      placePaths();
    });
  }).observe(el);

  // ------------------------------------------------------------ Daten

  let loadSeq = 0;
  async function loadView(): Promise<void> {
    const seq = ++loadSeq;
    try {
      const v = await backend.harmonyView(st.keyChoice);
      if (seq !== loadSeq) return;
      const keyChanged = !st.view || st.view.key.tonic !== v.key.tonic || st.view.key.minor !== v.key.minor;
      st.view = v;
      // Nur die Anfangs-Tonika im Verlauf: mit der neuen Tonart neu beginnen
      if (keyChanged && st.sketch.length === 1 && !st.sketch[0]!.fixed) st.sketch = [];
      if (keyChanged && st.sketch.length) {
        const infos = await backend.harmonyDescribe(currentKey(), st.sketch.map((s) => s.chord));
        st.sketch = st.sketch.map((s, i) => ({ ...s, ...infos[i]! }));
      }
      if (!st.sketch.length) {
        // Anfang: Tonika
        const tonic = v.circle.find((c) => c.chord.root === v.key.tonic && c.chord.quality === (v.key.minor ? "minor" : "major"));
        if (tonic) st.sketch = [{ chord: tonic.chord, name: tonic.name, roman: tonic.roman, function: tonic.function, role: tonic.role, diatonic: tonic.diatonic, seed: true }];
      }
      renderHead();
      renderCadences();
      await loadSuggestions();
    } catch (e) {
      notify(errorText(e), "error");
    }
  }

  async function loadSuggestions(): Promise<void> {
    try {
      st.result = await backend.harmonySuggest(currentKey(), st.sketch.map((s) => s.chord), st.group);
    } catch (e) {
      st.result = null;
      notify(errorText(e), "error");
    }
    renderAll();
  }

  // ------------------------------------------------------------ Klang

  const preview = (chords: Chord[], bars: number) =>
    backend.harmonyPreview(currentKey(), chords, Math.max(1, bars * beatsPerBar()), targetTrackId(), null).catch((e) => notify(errorText(e), "error"));

  /** Kandidat wählen und den Übergang vom letzten Akkord vorspielen. */
  function tryChord(info: ChordInfo): void {
    st.candidate = info;
    const last = st.sketch.at(-1);
    void preview(last ? [last.chord, info.chord] : [info.chord], 0.5);
    renderCircle();
    renderSketch();
    renderSuggestions();
  }

  /**
   * Sobald man am Verlauf arbeitet, bleibt die Tonart stehen – sonst würde die
   * Schätzung nach jedem geschriebenen Akkord springen und alles umdeuten.
   */
  function pinKey(): void {
    if (st.keyChoice || !st.view) return;
    st.keyChoice = currentKey();
    harmonyKey.value = st.keyChoice;
    st.view = { ...st.view, key: { ...st.view.key, source: "gewaehlt" } };
    renderHead();
    renderCircle();
  }

  function accept(info: ChordInfo): void {
    pinKey();
    st.sketch = [...st.sketch, { ...info }];
    st.candidate = null;
    void loadSuggestions();
  }

  function back(): void {
    const last = st.sketch.at(-1);
    if (!last || last.fixed) return;
    st.sketch = st.sketch.slice(0, -1);
    st.candidate = null;
    void loadSuggestions();
  }

  async function fromTracks(): Promise<void> {
    const sel = selection.peek();
    const found = findClip(project(), sel);
    const track = sel ? melodicTracks().find((t) => t.id === sel.trackId) : undefined;
    try {
      const placed = await backend.harmonyChords(currentKey(), track?.id ?? null, track && found ? found.clip.id : null);
      if (!placed.length) {
        notify("Keine Akkorde erkannt – einstimmige Stellen ergeben keinen Akkord.");
        return;
      }
      pinKey();
      st.sketch = placed.map((p) => ({ ...p, fixed: true }));
      st.candidate = null;
      notify(`${placed.length} Akkorde gelesen${track ? ` aus „${track.name}“` : " aus allen Instrumentspuren"}`);
      await loadSuggestions();
    } catch (e) {
      notify(errorText(e), "error");
    }
  }

  async function write(): Promise<void> {
    pinKey();
    const fresh = st.sketch.filter((s) => !s.fixed);
    if (!fresh.length) {
      notify("Nichts Neues zu schreiben – erst Akkorde übernehmen (Doppelklick oder „+“).");
      return;
    }
    const bpb = beatsPerBar();
    const bpc = Math.max(1, st.barsPerChord * bpb);
    const fixed = st.sketch.filter((s) => s.fixed && s.startBeats != null);
    const lastFixed = fixed.at(-1);
    const bpm = project()?.tempoBpm ?? 120;
    const start = lastFixed ? lastFixed.startBeats! + lastFixed.lengthBeats! : Math.floor((status.peek().positionSecs * bpm) / 60 / bpb) * bpb;
    const before = st.sketch.filter((s) => s.fixed).at(-1)?.chord ?? null;
    const mode = st.insertMode;
    const ok = await act(backend.harmonyWrite(currentKey(), fresh.map((s) => s.chord), bpc, start, targetTrackId(), before, mode));
    if (!ok) return;
    let t = start;
    st.sketch = st.sketch.map((s) => {
      if (s.fixed) return s;
      const placed = { ...s, fixed: true, startBeats: t, lengthBeats: bpc };
      t += bpc;
      return placed;
    });
    const where = targetTrackId() != null ? `„${melodicTracks().find((x) => x.id === targetTrackId())?.name}“` : "„Akkorde“";
    const how = mode === "einfuegen" ? "eingefügt (alles danach rückt nach hinten)" : "geschrieben";
    notify(`${fresh.length} Akkord${fresh.length === 1 ? "" : "e"} in ${where} ab Takt ${Math.floor(start / bpb) + 1} ${how}`);
    // Cursor auf den letzten neuen Akkord: der Verlauf geht von dort weiter
    const lastStart = t - bpc;
    followIx = -2;
    void backend.seek((lastStart * 60) / bpm).catch(() => {});
    renderSketch();
  }

  // ------------------------------------------------------------ Darstellung

  function renderHead(): void {
    const v = st.view;
    const auto = v && v.key.source !== "gewaehlt" ? `Automatisch (${v.key.name}${v.key.source === "standard" ? ", keine Noten" : ", geschätzt"})` : "Automatisch";
    const opts = [h("option", { value: "auto" }, auto)];
    for (const minor of [false, true]) {
      for (let i = 0; i < 12; i++) {
        const t = (i * 7 + (minor ? 9 : 0)) % 12; // im Quintenzirkel sortiert
        opts.push(h("option", { value: `${t}:${minor ? "m" : "d"}` }, keyName({ tonic: t, minor })));
      }
    }
    keySel.replaceChildren(...opts);
    keySel.value = st.keyChoice ? `${st.keyChoice.tonic}:${st.keyChoice.minor ? "m" : "d"}` : "auto";
    renderTargets();
  }

  let targetSig = "";
  function renderTargets(): void {
    const tracks = melodicTracks();
    const sig = tracks.map((t) => `${t.id}:${t.name}`).join(",");
    if (sig === targetSig && targetSel.childElementCount) return;
    targetSig = sig;
    targetSel.replaceChildren(
      h("option", { value: "akkorde" }, "Spur „Akkorde“"),
      ...tracks.filter((t) => t.name !== "Akkorde").map((t) => h("option", { value: String(t.id) }, `Spur „${t.name}“`)),
    );
    if (typeof st.target === "number" && !tracks.some((t) => t.id === st.target)) st.target = "akkorde";
    targetSel.value = String(st.target);
  }

  function renderCircle(): void {
    const v = st.view;
    if (!v || !v.circle.length) {
      circleBox.replaceChildren(h("div", { class: "lib-empty" }, "Quintenzirkel lädt …"));
      return;
    }
    const last = st.sketch.at(-1)?.chord ?? null;
    const lastCell = last ? cellOf(last) : null;
    const cand = st.candidate ? cellOf(st.candidate.chord) : null;
    const sugg = (st.result?.suggestions ?? []).slice().sort((a, b) => b.score - a.score);
    const rankOf = new Map<string, { rank: number; s: ChordSuggestion }>();
    sugg.forEach((s, i) => {
      const c = cellOf(s.chord);
      if (c && !rankOf.has(`${c[0]}:${c[1]}`)) rankOf.set(`${c[0]}:${c[1]}`, { rank: i + 1, s });
    });
    const shown = hover ?? st.pinned;
    const cadence = shown?.kind === "cadence" ? v.cadences.find((c) => c.id === shown.id) ?? null : null;
    const path = shown?.kind === "path" ? (st.result?.paths ?? []).find((p) => pathId(p) === shown.id) ?? null : null;

    const root = svg("svg", { viewBox: "-212 -212 424 424", class: "hc-svg", role: "img", "aria-label": `Quintenzirkel in ${v.key.name}` });
    // Pfeilspitzen je Stimmung (Marker erben die Strichfarbe nicht zuverlässig)
    const defs = svg("defs", {},
      ...MOODS.map(([m]) => svg("marker", { id: `hc-mk-${m}`, viewBox: "0 0 10 10", refX: "7", refY: "5", markerWidth: "4.2", markerHeight: "4.2", orient: "auto-start-reverse" },
        svg("path", { d: "M0 0L10 5L0 10z", class: `hc-mk m-${m}` }))));
    root.append(defs);

    for (const cell of v.circle) {
      const key = `${cell.ring}:${cell.pos}`;
      const s = rankOf.get(key);
      const inKey = cell.diatonic && cell.function !== "–";
      const related = !cell.diatonic && cell.function !== "–";
      const cls = ["hc-cell", `ring${cell.ring}`];
      if (inKey) cls.push("in-key");
      else if (related) cls.push("related");
      if (lastCell && lastCell[0] === cell.ring && lastCell[1] === cell.pos) cls.push("current");
      if (cand && cand[0] === cell.ring && cand[1] === cell.pos) cls.push("cand");
      if (s && s.rank <= 8) cls.push("sugg");
      const g = svg("g", { class: cls.join(" ") });
      const path = svg("path", { d: sector(cell.ring, cell.pos) });
      if (s && s.rank <= 8) path.style.setProperty("--s", String(Math.max(0.15, s.s.score)));
      const [cx, cy] = centerOf(cell.ring, cell.pos);
      const label = svg("text", { x: cx, y: cy + (inKey || s ? -2 : 4), class: "hc-name" }, cell.name);
      g.append(path, label);
      if (inKey || (s && s.rank <= 8)) g.append(svg("text", { x: cx, y: cy + 11, class: "hc-roman" }, cell.roman));
      const tip = `${cell.name} · ${cell.roman} · ${cell.function} – ${cell.role}${s ? `\n${s.rank}. Vorschlag: ${s.s.reason}` : ""}${s?.s.library ? `\nBibliothek: ${pct(s.s.library.share)} %` : ""}\nKlick: Übergang anhören · Doppelklick: übernehmen`;
      g.append(svg("title", {}, tip));
      g.addEventListener("click", () => tryChord(cell));
      g.addEventListener("dblclick", () => accept(cell));
      root.append(g);
    }

    // Pfeile vom aktuellen Akkord zu den besten Vorschlägen, gefärbt nach Stimmung
    // (blass, solange ein Weg oder eine Kadenz gezeigt wird)
    if (lastCell) {
      const [x0, y0] = centerOf(lastCell[0], lastCell[1]);
      const arrows = svg("g", { class: `hc-arrows${cadence || path ? " dim" : ""}` });
      for (const { rank, s } of [...rankOf.values()].filter((r) => r.rank <= 5)) {
        const c = cellOf(s.chord)!;
        if (c[0] === lastCell[0] && c[1] === lastCell[1]) continue;
        const [x1, y1] = centerOf(c[0], c[1]);
        const mx = (x0 + x1) / 2 * 0.45, my = (y0 + y1) / 2 * 0.45;
        const d = `M${x0} ${y0}Q${mx} ${my} ${x1} ${y1}`;
        const m = s.mood ?? "hell";
        arrows.append(svg("path", { d, class: `hc-arrow r${rank} m-${m}`, "data-mood": m, "stroke-width": 1 + 2.5 * s.score, "marker-end": `url(#hc-mk-${m})` }));
      }
      root.append(arrows);
    }
    // Rang-Plaketten
    for (const [key, { rank }] of rankOf) {
      if (rank > 8) continue;
      const [ring, pos] = key.split(":").map(Number) as [number, number];
      const [bx, by] = pt(RINGS[ring]![1] - 9, pos * 30 - 90 + 10);
      root.append(svg("g", { class: `hc-badge${cadence || path ? " dim" : ""}` }, svg("circle", { cx: bx, cy: by, r: 7.5 }), svg("text", { x: bx, y: by + 3.2 }, String(rank))));
    }
    // Weg zum Schluss (vom aktuellen Akkord aus) bzw. Kadenzvorlage, nummeriert
    if (path && last) root.append(route([last, ...path.steps.map((s) => s.chord)], path.moods, true));
    else if (cadence) root.append(route(cadence.steps.map((s) => s.chord), cadence.moods ?? [], false));
    // Mitte: Tonart und aktueller Akkord
    const cur = st.candidate ?? st.sketch.at(-1) ?? null;
    root.append(
      svg("circle", { r: 62, class: "hc-center" }),
      svg("text", { y: -22, class: "hc-key" }, v.key.name),
      svg("text", { y: -8, class: "hc-keysrc" }, v.key.source === "gewaehlt" ? "gewählt" : v.key.source === "geschaetzt" ? "aus den Noten" : "Standard"),
      svg("text", { y: 16, class: "hc-cur" }, cur ? cur.name : "–"),
      svg("text", { y: 31, class: "hc-curfn" }, cur ? `${cur.roman} · ${cur.function}` : ""),
    );
    circleBox.replaceChildren(root);
  }

  /** Ab diesem Index leuchten die Akkorde im Verlauf kurz auf (gerade angehängt). */
  let flashFrom = -1;
  let flashTimer = 0;

  function chip(item: SketchItem, i: number, isLast: boolean): HTMLElement {
    const b = h("button", {
      class: `hc-chip${item.fixed ? " fixed" : ""}${isLast ? " last" : ""}${flashFrom >= 0 && i >= flashFrom ? " new" : ""}`,
      title: `${item.name} · ${item.roman} · ${item.function} – ${item.role}${item.fixed ? "\nsteht schon in der Spur" : ""}\nKlick: anhören`,
      onclick: () => void preview([item.chord], 0.5),
    }, h("b", {}, item.name), h("span", {}, item.roman));
    b.dataset.i = String(i);
    return b;
  }

  function renderSketch(): void {
    const items = st.sketch.map((s, i) => chip(s, i, i === st.sketch.length - 1));
    const arrows: Node[] = [];
    items.forEach((c, i) => {
      if (i) arrows.push(h("i", { class: "hc-to" }, "→"));
      arrows.push(c);
    });
    if (st.candidate) {
      const c = st.candidate;
      arrows.push(h("i", { class: "hc-to" }, "→"),
        h("span", { class: "hc-chip cand", title: `${c.name} · ${c.roman} · ${c.function} – ${c.role}` }, h("b", {}, c.name), h("span", {}, c.roman),
          h("button", { class: "btn small blue", title: "In den Verlauf übernehmen (oder Doppelklick)", onclick: () => accept(c) }, "+ übernehmen")));
    }
    const canBack = !!st.sketch.length && !st.sketch.at(-1)!.fixed;
    sketchBox.replaceChildren(
      h("div", { class: "hc-title" }, "Verlauf",
        h("span", { class: "muted" }, " – Klick auf einen Akkord: Übergang anhören, Doppelklick: übernehmen")),
      h("div", { class: "hc-chips" }, ...arrows),
      h("div", { class: "hc-actions" },
        h("button", { class: "btn small", title: "Ganzen Verlauf vorspielen", onclick: () => st.sketch.length && void preview(st.sketch.map((s) => s.chord), st.barsPerChord) }, icon("play"), "Verlauf"),
        h("button", { class: "btn small", title: "Letzten Akkord entfernen", disabled: !canBack, onclick: back }, icon("undo"), "Zurück"),
        h("button", { class: "btn small ghost", title: "Neu beginnen (mit der Tonika)", onclick: () => { st.sketch = []; st.candidate = null; holdFollow(); void loadView(); } }, "Leeren"),
        h("button", { class: "btn small", title: "Akkorde aus dem gewählten Clip lesen (ohne Auswahl: aus allen Instrumentspuren)", onclick: () => void fromTracks() }, icon("clip"), "Aus Spur lesen"),
        modeSeg(),
        h("button", { class: "btn small primary", title: st.insertMode === "einfuegen"
          ? "Neue Akkorde hinter den Akkord am Cursor einfügen – alles danach rückt in allen Spuren nach hinten"
          : "Neue Akkorde hinter den Akkord am Cursor schreiben – was dort in der Ziel-Spur steht, wird ersetzt", onclick: () => void write() }, icon("plus"), "In Spur schreiben")),
      h("label", { class: "check small hc-follow", title: "Beim Abspielen und Cursor-Setzen greift der Verlauf die Akkorde an dieser Stelle auf (eigene neue Akkorde bleiben)" },
        followBox(), "folgt der Abspielposition"),
    );
  }

  function modeSeg(): HTMLElement {
    const seg = h("div", { class: "seg mini hc-mode", title: "Wie „In Spur schreiben“ die neuen Akkorde setzt" });
    for (const [m, label, tip] of [
      ["einfuegen", "Einfügen", "Platz schaffen: alles danach rückt in allen Spuren nach hinten"],
      ["ersetzen", "Ersetzen", "das Nachfolgende in der Ziel-Spur überschreiben"],
    ] as const) {
      const b = h("button", { class: `seg-btn${st.insertMode === m ? " on" : ""}`, title: tip }, label);
      b.dataset.mode = m;
      b.addEventListener("click", () => {
        st.insertMode = m;
        renderSketch();
      });
      seg.append(b);
    }
    return seg;
  }

  function followBox(): HTMLInputElement {
    const c = h("input", { type: "checkbox" }) as HTMLInputElement;
    c.checked = st.follow;
    c.addEventListener("change", () => {
      st.follow = c.checked;
      followIx = -2;
      if (st.follow) followCursor();
    });
    return c;
  }

  // ------------------------------------------------------------ Abspielposition

  /** Index und Inhalt des zuletzt aufgegriffenen Akkords (-2 = neu aufgreifen). */
  let followIx = -2;
  let followSig = "";
  let followTimer = 0;
  /** Nach „Leeren“: erst wieder folgen, wenn sich der Cursor bewegt (Position in Sekunden beim Leeren). */
  let followHold: number | null = null;
  function holdFollow(): void {
    followHold = status.peek().positionSecs;
  }
  /**
   * Verlauf an die Abspielposition anpassen: die Akkorde des Satzes bis zur
   * Stelle (der Akkord dort und bis zu drei davor) – eigene neue Akkorde bleiben dahinter.
   */
  function followCursor(): void {
    if (!st.follow) return;
    const line = harmonyLine.peek();
    const p = project();
    if (!line || !line.chords.length || !p) return;
    if (line.key.tonic !== currentKey().tonic || line.key.minor !== currentKey().minor) return;
    const i = Math.max(0, chordIndexAt(line, (status.peek().positionSecs * p.tempoBpm) / 60));
    if (followHold !== null) {
      if (Math.abs(status.peek().positionSecs - followHold) < 0.02) return;
      followHold = null;
      followIx = -2;
    }
    const ctx = line.chords.slice(Math.max(0, i - 3), i + 1);
    const sig = `${i}|${ctx.map((c) => `${c.startBeats}:${c.name}`).join(",")}`;
    if (i === followIx && sig === followSig) return;
    followIx = i;
    followSig = sig;
    const fresh = st.sketch.filter((s) => !s.fixed && !s.seed);
    st.sketch = [
      ...ctx.map((c) => ({ chord: c.chord, name: c.name, roman: c.roman, function: c.function, role: c.role, diatonic: c.diatonic, fixed: true, startBeats: c.startBeats, lengthBeats: c.lengthBeats })),
      ...fresh,
    ];
    st.candidate = null;
    renderSketch();
    renderCircle();
    clearTimeout(followTimer);
    followTimer = window.setTimeout(() => void loadSuggestions(), 200);
  }

  function exampleRow(e: HarmonyExample): HTMLElement {
    const who = e.composer ? `${e.composer} – ` : "";
    return h("div", { class: "hc-ex" },
      h("span", { class: "hc-ex-name", title: `${who}${e.title || "(ohne Titel)"}, Takt ${e.bar}` }, `${who}${e.title || "(ohne Titel)"}`, h("em", {}, ` · T. ${e.bar}`)),
      h("button", { class: "btn small ghost", title: "Diese Stelle anhören", onclick: () => void backend.harmonyPlayExample(e.doc, e.start, e.end).catch((x) => notify(errorText(x), "error")) }, icon("play")),
      h("button", { class: "btn small ghost", title: "Partitur im Fenster „Bibliothek“ an dieser Stelle öffnen", onclick: () => void openInLibrary(e.doc, [e.start, e.end]) }, icon("book"), "Öffnen"));
  }

  function suggestionRow(s: ChordSuggestion, rank: number, maxShare: number): HTMLElement {
    const k = ckey(s.chord);
    const lib = s.library;
    const isCand = !!st.candidate && same(st.candidate.chord, s.chord);
    const prevName = st.sketch.at(-1)?.name ?? "";
    const share = lib
      ? h("div", {
          class: "hc-share",
          title: `${lib.count.toLocaleString("de-DE")} von ${lib.total.toLocaleString("de-DE")} Fortsetzungen nach ${prevName} in der Bibliothek${lib.exact ? "" : " (als Grundakkord gezählt – für diese Form gibt es zu wenig Daten)"}`,
        },
          h("i", { style: `width:${maxShare > 0 ? Math.max(2, (lib.share / maxShare) * 100) : 0}%` }),
          h("b", {}, `${pct(lib.share)} %`))
      : null;
    const m = s.mood ?? "hell";
    const row = h("div", { class: `hc-row${isCand ? " cand" : ""}` },
      h("span", { class: "hc-rank" },
        h("i", { class: `hc-mood m-${m}`, "data-mood": m, title: `${prevName} → ${s.name}: ${moodLabel(m)} – ${MOODS.find((x) => x[0] === m)?.[2] ?? ""}` }),
        String(rank)),
      h("button", { class: "hc-pick", title: "Übergang anhören (Doppelklick: übernehmen)", onclick: () => tryChord(s), ondblclick: () => accept(s) },
        h("b", {}, s.name),
        h("span", { class: "hc-fn" }, `${s.roman} · ${s.function}`),
        h("span", { class: "hc-why" }, s.reason, s.toKey ? h("em", {}, ` → ${s.toKey}`) : null)),
      share,
      lib && lib.examples.length
        ? h("button", { class: `btn small ghost hc-exbtn${st.expanded === k ? " on" : ""}`, title: "Beispiele aus der Bibliothek",
            onclick: () => { st.expanded = st.expanded === k ? null : k; renderSuggestions(); } }, `${lib.examples.length} Beisp. ${st.expanded === k ? "▴" : "▾"}`)
        : h("span"),
      h("button", { class: "btn small blue", title: "In den Verlauf übernehmen", onclick: () => accept(s) }, "+"),
    );
    if (st.expanded !== k || !lib) return row;
    return h("div", { class: "hc-rowwrap" }, row, h("div", { class: "hc-exlist" }, ...lib.examples.map(exampleRow)));
  }

  function renderSuggestions(): void {
    const r = st.result;
    if (r?.library) {
      const lib = r.library;
      const groups = lib.groups.length > 1
        ? h("span", { class: "seg mini hc-groups", title: "Woher die Häufigkeiten kommen" },
            ...lib.groups.map((g) => {
              const b = h("button", { class: `seg-btn${g.id === lib.group ? " on" : ""}`, title: `${g.scores.toLocaleString("de-DE")} ${currentKey().minor ? "Moll" : "Dur"}stücke mit Akkordfolge`,
                onclick: () => { st.group = g.id; void loadSuggestions(); } }, g.label);
              return b;
            }))
        : null;
      libLine.replaceChildren(
        h("span", { class: "muted" }, ` · Häufigkeit in „${lib.name}“ (${lib.scores.toLocaleString("de-DE")} ${currentKey().minor ? "Moll" : "Dur"}stücke)`),
        groups ?? "",
        r.hint ? h("span", { class: "muted" }, ` · ${r.hint}`) : "");
    } else if (r?.hint) {
      const reindex = r.hint.includes("neu indizieren")
        ? h("button", { class: "btn small blue", onclick: () => void backend.libraryReindex().then(() => notify("Bibliothek wird neu indiziert – Fortschritt im Fenster „Bibliothek“")).catch((e) => notify(errorText(e), "error")) }, "Neu indizieren")
        : null;
      libLine.replaceChildren(h("span", { class: "muted" }, ` · ${r.hint}`), reindex ?? "");
    } else {
      libLine.replaceChildren();
    }
    const list = r?.suggestions ?? [];
    if (!list.length) {
      suggBox.replaceChildren(h("div", { class: "lib-empty" }, st.view ? "Keine Vorschläge." : "…"));
      return;
    }
    const ranked = list.slice().sort((a, b) => b.score - a.score);
    const rankOf = new Map(ranked.map((s, i) => [ckey(s.chord), i + 1]));
    const maxShare = Math.max(0, ...list.map((s) => s.library?.share ?? 0));
    const groups: Node[] = [];
    for (const [cat, label, hint] of CATEGORIES) {
      const items = ranked.filter((s) => s.category === cat);
      if (!items.length) continue;
      groups.push(h("div", { class: `hc-group ${cat}` }, h("div", { class: "hc-gtitle", title: hint }, label),
        ...items.map((s) => suggestionRow(s, rankOf.get(ckey(s.chord)) ?? 0, maxShare))));
    }
    suggBox.replaceChildren(...groups);
  }

  /** Akkordfolge mit Pfeilen in der Farbe der Stimmung (moods[i] führt in chords[i + 1]). */
  function moodSeq(chords: ChordInfo[], moods: Mood[], opt: { from?: boolean; shares?: Array<number | null> | null } = {}): Node[] {
    const out: Node[] = [];
    chords.forEach((s, i) => {
      if (i) {
        const m = moods[i - 1] ?? "hell";
        const sh = opt.shares?.[i - 1];
        const prev = chords[i - 1]!.name;
        // mit Bibliothek: die Übergangswahrscheinlichkeit gleich unter dem Pfeil
        out.push(h("i", { class: `hc-pto m-${m}${sh != null ? " has-pct" : ""}`, "data-mood": m, title: `${prev} → ${s.name}: ${moodLabel(m)}${sh != null ? `\nBibliothek: ${pct(sh)} % der Fortsetzungen nach ${prev}` : ""}` },
          ...(sh != null ? [h("span", {}, "→"), h("small", { class: "hc-pct" }, `${pct(sh)} %`)] : ["→"])));
      }
      const from = i === 0 && opt.from;
      out.push(h("span", { class: `hc-pchip${from ? " from" : ""}`, title: `${s.name} · ${s.roman} · ${s.function} – ${s.role}${from ? "\n(aktueller Akkord)" : ""}` }, h("b", {}, s.name), h("span", {}, s.roman)));
    });
    return out;
  }

  /** Overlay festhalten bzw. lösen und alles, was es zeigt, neu zeichnen. */
  function togglePin(o: Overlay, force?: boolean): void {
    st.pinned = force || !sameOverlay(st.pinned, o) ? o : null;
    renderPaths();
    renderCadences();
    renderCircle();
  }

  function showHover(o: Overlay | null): void {
    if (sameOverlay(hover, o) || (!hover && !o)) return;
    hover = o;
    renderCircle();
  }

  /**
   * Akkorde an den Verlauf hängen – sichtbar: der Verlauf rückt ins Bild, die
   * neuen Akkorde leuchten kurz auf. `write`: gleich in die Ziel-Spur schreiben.
   */
  function appendSteps(steps: ChordInfo[], what: string, toTrack = false): void {
    if (!steps.length) {
      notify("Der Verlauf endet schon so – nichts anzuhängen.");
      return;
    }
    pinKey();
    flashFrom = st.sketch.length;
    st.sketch = [...st.sketch, ...steps.map((s) => ({ ...s }))];
    st.candidate = null;
    renderSketch();
    sketchBox.scrollIntoView({ block: "nearest", behavior: "smooth" });
    clearTimeout(flashTimer);
    flashTimer = window.setTimeout(() => {
      flashFrom = -1;
      renderSketch();
    }, 1600);
    void loadSuggestions();
    const n = `${steps.length} Akkord${steps.length === 1 ? "" : "e"}`;
    if (toTrack) void write();
    else notify(`${what}: ${n} an den Verlauf angehängt – „In Spur schreiben“ setzt sie in die Spur`);
  }

  function renderPaths(): void {
    const cur = st.sketch.at(-1);
    pathFrom.textContent = cur ? ` · von ${cur.name}` : "";
    const paths = st.result?.paths ?? [];
    if (!paths.length) {
      pathBox.replaceChildren(h("div", { class: "lib-empty" }, st.result ? "Kein Schluss in Reichweite." : "…"));
      return;
    }
    pathBox.replaceChildren(...paths.map((p) => {
      const o: Overlay = { kind: "path", id: pathId(p) };
      const pinned = sameOverlay(st.pinned, o);
      const libAvg = p.shares && p.shares.length ? p.shares.reduce<number>((a, x) => a + (x ?? 0), 0) / p.shares.length : null;
      const tip = `${p.label}${p.longer ? " (längerer Weg)" : ""}: ${[cur?.name ?? "", ...p.steps.map((s) => s.name)].join(" → ")}`
        + `\nWertung ${p.score.toLocaleString("de-DE")}${libAvg != null ? ` · Schritte im Mittel ${pct(libAvg)} % in der Bibliothek` : ""}`
        + "\nMaus darüber: im Zirkel zeigen · Klick: festhalten";
      const row = h("div", { class: `hc-path${p.longer ? " longer" : ""}${pinned ? " on" : ""}`, title: tip },
        h("span", { class: "hc-plabel" }, p.longer ? "länger" : p.label),
        h("span", { class: "hc-pseq" }, ...(cur
          ? moodSeq([cur, ...p.steps], p.moods, { from: true, shares: p.shares })
          : moodSeq(p.steps, p.moods.slice(1), { shares: p.shares?.slice(1) }))),
        h("span", { class: "hc-pbtns" },
          h("button", { class: "btn small ghost", title: "Vom aktuellen Akkord bis zum Schluss vorspielen und im Zirkel festhalten", onclick: (e: Event) => {
            e.stopPropagation();
            togglePin(o, true);
            void preview([...(cur ? [cur.chord] : []), ...p.steps.map((s) => s.chord)], st.barsPerChord);
          } }, icon("play")),
          h("button", { class: "btn small blue", title: "Diese Akkorde an den Verlauf anhängen (zum Weiterprobieren)", onclick: (e: Event) => {
            e.stopPropagation();
            st.pinned = null;
            hover = null;
            appendSteps(p.steps, p.label);
          } }, "+"),
          h("button", { class: "btn small", title: "Anhängen und gleich vierstimmig in die Ziel-Spur schreiben", onclick: (e: Event) => {
            e.stopPropagation();
            st.pinned = null;
            hover = null;
            appendSteps(p.steps, p.label, true);
          } }, "In Spur")));
      row.addEventListener("mouseenter", () => showHover(o));
      row.addEventListener("mouseleave", () => showHover(null));
      row.addEventListener("click", () => togglePin(o));
      return row;
    }));
  }

  /** Kadenz ohne ihren ersten Akkord, wenn der Verlauf schon darauf endet. */
  function cadenceTail(steps: ChordInfo[]): ChordInfo[] {
    const last = st.sketch.at(-1);
    return last && steps[0] && same(last.chord, steps[0].chord) ? steps.slice(1) : steps;
  }

  function renderCadences(): void {
    const v = st.view;
    if (!v) return;
    cadBox.replaceChildren(...v.cadences.map((c) => {
      const o: Overlay = { kind: "cadence", id: c.id };
      const card = h("div", { class: `hc-cadcard${sameOverlay(st.pinned, o) ? " on" : ""}`, title: `${c.description}\n${c.steps.map((s) => `${s.name} (${s.roman})`).join(" – ")}\nMaus darüber: im Zirkel zeigen · Klick: festhalten` },
        h("b", {}, c.name), h("span", { class: "hc-pseq" }, ...moodSeq(c.steps, c.moods ?? [])),
        h("div", {},
          h("button", { class: "btn small", title: "Vorspielen und im Zirkel festhalten", onclick: (e: Event) => {
            e.stopPropagation();
            togglePin(o, true);
            void preview(c.steps.map((s) => s.chord), st.barsPerChord);
          } }, icon("play")),
          h("button", { class: "btn small blue", title: "An den Verlauf (oben) anhängen – zum Weiterprobieren", onclick: (e: Event) => {
            e.stopPropagation();
            st.pinned = o;
            appendSteps(cadenceTail(c.steps), c.name);
          } }, "+ anhängen"),
          h("button", { class: "btn small", title: "Anhängen und gleich vierstimmig in die Ziel-Spur schreiben", onclick: (e: Event) => {
            e.stopPropagation();
            st.pinned = o;
            appendSteps(cadenceTail(c.steps), c.name, true);
          } }, "In Spur")));
      card.addEventListener("mouseenter", () => showHover(o));
      card.addEventListener("mouseleave", () => showHover(null));
      card.addEventListener("click", () => togglePin(o));
      return card;
    }));
  }

  function renderAll(): void {
    renderCircle();
    renderSketch();
    renderSuggestions();
    renderPaths();
  }

  // ------------------------------------------------------------ Lebenszyklus

  let lastSnap: Snapshot | null = null;
  let timer = 0;
  renderSketch();
  renderCircle();
  void loadView();
  // Akkordfolge des Satzes laden; der Verlauf folgt der Abspielposition
  watchHarmonyLine();
  harmonyKey.value = st.keyChoice;
  effect(() => {
    status.value;
    harmonyLine.value;
    followCursor();
  });
  // Arbeit im Fenster: Harmonie-Band in der Zeitleiste zeigen
  let leaveTimer = 0;
  const focusOn = () => {
    clearTimeout(leaveTimer);
    harmonyFocus.value = true;
  };
  const focusOff = () => {
    clearTimeout(leaveTimer);
    leaveTimer = window.setTimeout(() => {
      if (!el.matches(":hover") && !el.contains(document.activeElement)) harmonyFocus.value = false;
    }, 500);
  };
  el.addEventListener("pointerenter", focusOn);
  el.addEventListener("focusin", focusOn);
  el.addEventListener("pointerleave", focusOff);
  el.addEventListener("focusout", focusOff);
  // Bibliothek geladen oder neu indiziert: Häufigkeiten nachholen
  onLibrary = () => {
    if (st.view) void loadSuggestions();
  };
  if (!libraryListening) {
    libraryListening = true;
    let lastLib: string | null = null;
    let lastChords = "";
    backend.onLibraryChanged((i) => {
      const sig = JSON.stringify(i.chordScores);
      if (i.id !== lastLib || sig !== lastChords) {
        lastLib = i.id;
        lastChords = sig;
        onLibrary();
      }
    });
  }

  return {
    el,
    update() {
      const snap = snapshot.peek();
      renderTargets();
      if (snap !== lastSnap) {
        const first = lastSnap === null;
        lastSnap = snap;
        // Geschätzte Tonart folgt den Noten (gebremst, Bearbeiten erzeugt viele Schnappschüsse)
        if (!first && !st.keyChoice) {
          clearTimeout(timer);
          timer = window.setTimeout(() => void loadView(), 600);
        }
      }
    },
  };
}
