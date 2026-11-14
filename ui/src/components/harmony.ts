// "Harmony" window: circle of fifths with continuations to try out.
// A click on a chord (in the circle or in the list) previews the transition
// from the last chord of the progression; double-click or "+" adopts it.
// Suggestions come from functional harmony (Rust core) and, if
// a library with chord statistics is loaded, carry their frequency there along with
// example passages to listen to and open.
// Transitions are colored by mood (resolving, tense, surprising, bright,
// dark - legend below the circle). "Paths to a cadence" shows multi-step
// cadences from the current chord; hovering over one draws the path numbered
// into the circle, a click pins it.
// The progression follows the playhead (during playback and when setting the cursor):
// it starts with the chords of the piece up to that point, new chords of one's own
// stay after it. "Write to track" places them after the chord at the cursor -
// inserting (everything after it moves back) or replacing what follows.

import { backend } from "../bridge.js";
import { chordName, keyText } from "../spelling.js";
import { h, icon } from "../dom.js";
import { act, errorText, findClip, notify, project, selection, snapshot, status } from "../store.js";
import { chordIndexAt, harmonyFocus, harmonyKey, harmonyLine, watchHarmonyLine } from "../harmonyline.js";
import { effect } from "../signals.js";
import { language, plural, t } from "../i18n.js";
import { openInLibrary } from "./library.js";
import type { CadencePath, Chord, ChordInfo, ChordSuggestion, HarmonyExample, HarmonyView, KeySpec, Mood, Snapshot, SuggestCategory, SuggestResult, Track } from "../types.js";

interface SketchItem extends ChordInfo {
  /** already in the track (read from the clip or written) */
  fixed?: boolean;
  /** only the beginning (tonic), not a chord of its own - drops out when the progression follows the playhead */
  seed?: boolean;
  startBeats?: number;
  lengthBeats?: number;
}

const MAJOR = ["C", "Des", "D", "Es", "E", "F", "Fis", "G", "As", "A", "B", "H"];
const MINOR = ["c", "cis", "d", "es", "e", "f", "fis", "g", "gis", "a", "b", "h"];
/** Key name in the language's spelling (the tables are German, like the core's names). */
const keyName = (k: KeySpec) => keyText(k.minor ? `${MINOR[k.tonic] ?? ""}-Moll` : `${MAJOR[k.tonic] ?? ""}-Dur`);
/** The chord track (core: harmony.rs, is_chord_track): new ones are called "Chords" in the
 *  chosen language, older projects have "Akkorde". */
const isChordTrack = (name: string) => ["Akkorde", "Chords", t("Chords")].includes(name);

const CATEGORIES: Array<[SuggestCategory, string, string]> = [
  ["schluss", t("Close"), t("Authentic cadence, plagal cadence, deceptive cadence")],
  ["spannung", t("Tension"), t("leads to the dominant")],
  ["weiter", t("Continue"), t("stays open without closing")],
  ["farbe", t("Color"), t("Secondary dominants, borrowed chords, Neapolitan")],
  ["ausweichen", t("Modulate"), t("opens a neighboring key")],
  ["haeufig", t("Common in the library"), t("not a rule, but often written this way")],
];

/** Moods of the transitions: color (CSS class m-…), name, explanation. */
const MOODS: Array<[Mood, string, string]> = [
  ["loesend", t("resolving"), t("Tension resolves: dominant → tonic (or the target of the secondary dominant), leading tone upward, plagal cadence")],
  ["spannend", t("tense"), t("builds tension: to the dominant, double or secondary dominant, cadential 6-4")],
  ["ueberraschend", t("surprising"), t("unexpected: deceptive cadence, chromatic mediant (e.g. C → A♭)")],
  ["hell", t("bright"), t("to a major chord of the key")],
  ["dunkel", t("dark"), t("to minor or diminished, or borrowed from the minor key")],
];
const moodLabel = (m: Mood) => MOODS.find((x) => x[0] === m)?.[1] ?? m;

/** What is additionally drawn in the circle: a path to a cadence or a cadence template. */
interface Overlay {
  kind: "path" | "cadence";
  id: string;
}
const pathId = (p: CadencePath) => `${p.goal}${p.longer ? "+" : ""}`;

/** State is preserved when the window is rebuilt. */
const st = {
  keyChoice: null as KeySpec | null,
  view: null as HarmonyView | null,
  sketch: [] as SketchItem[],
  candidate: null as ChordInfo | null,
  result: null as SuggestResult | null,
  barsPerChord: 1,
  target: "akkorde" as "akkorde" | number,
  expanded: null as string | null,
  /** pinned path or pinned cadence (click / play) */
  pinned: null as Overlay | null,
  /** Statistics group: 0 = all pieces, 1 = Baroque, 2 = Bach */
  group: 0,
  /** Progression picks up the harmony at the playhead */
  follow: true,
  /** Writing: make room (everything after it moves back) or replace what follows */
  insertMode: "einfuegen" as "einfuegen" | "ersetzen",
};
/** currently shown with the mouse (takes precedence over the pinned one) */
let hover: Overlay | null = null;
const sameOverlay = (a: Overlay | null, b: Overlay | null) => !!a && !!b && a.kind === b.kind && a.id === b.id;

const same = (a: Chord, b: Chord) => a.root === b.root && a.quality === b.quality && (a.bass ?? -1) === (b.bass ?? -1);
const ckey = (c: Chord) => `${c.root}|${c.quality}|${c.bass ?? ""}`;
const pct = (x: number) => (x >= 0.095 ? Math.round(x * 100) : Math.round(x * 1000) / 10).toLocaleString(language());

function triadClass(c: Chord): 0 | 1 | 2 | null {
  switch (c.quality) {
    case "major": case "dom7": case "maj7": return 0;
    case "minor": case "min7": return 1;
    case "dim": case "halfDim7": case "dim7": return 2;
    default: return null;
  }
}

/** Place in the circle: [ring, position] - seventh chords with the triad. */
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

/** Arc from field a to field b (curved toward the center, ends shortened); same field: loop. */
function segPath(a: [number, number], b: [number, number]): string {
  const [x0, y0] = centerOf(a[0], a[1]);
  const [x1, y1] = centerOf(b[0], b[1]);
  if (a[0] === b[0] && a[1] === b[1]) {
    // loop: inward in the outer ring, otherwise outward
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
 * Path in the circle: arcs colored by mood (moods[i] for chords[i] → chords[i+1])
 * and numbered steps. skipFirst: chords[0] is the current chord (without a number).
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
    // same chord several times in the path: numbers side by side
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
  const keySel = h("select", { class: "hc-select", title: t("Key – estimated automatically from the notes or set manually") });
  keySel.addEventListener("change", () => {
    const v = keySel.value;
    st.keyChoice = v === "auto" ? null : { tonic: Number(v.split(":")[0]), minor: v.endsWith(":m") };
    harmonyKey.value = st.keyChoice;
    void loadView();
  });
  const durSeg = h("div", { class: "seg mini", title: t("Duration per chord when playing and writing") });
  for (const [bars, label] of [[0.5, t("½ bar")], [1, t("1 bar")], [2, t("2 bars")]] as const) {
    const b = h("button", { class: "seg-btn" }, label);
    b.dataset.v = String(bars);
    b.addEventListener("click", () => {
      st.barsPerChord = bars;
      for (const x of durSeg.children) x.classList.toggle("on", (x as HTMLElement).dataset.v === String(bars));
    });
    b.classList.toggle("on", bars === st.barsPerChord);
    durSeg.append(b);
  }
  const targetSel = h("select", { class: "hc-select", title: t("Where \"Write to track\" puts the chords") });
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

  /** Legend: hovering over a mood highlights its transitions. */
  const focusMood = (m: Mood | null) => {
    for (const e of el.querySelectorAll<HTMLElement | SVGElement>("[data-mood]")) e.classList.toggle("faded", m !== null && e.dataset.mood !== m);
  };
  const legend = h("div", { class: "hc-legend" },
    h("span", { class: "hc-lg-title" }, t("Mood")),
    ...MOODS.map(([m, label, desc]) => {
      const item = h("span", { class: `hc-lg m-${m}`, title: `${label}: ${desc}` }, h("i"), label);
      item.addEventListener("mouseenter", () => focusMood(m));
      item.addEventListener("mouseleave", () => focusMood(null));
      return item;
    }));

  const field = (label: string, control: HTMLElement) => h("span", { class: "hc-field" }, h("label", {}, label), control);
  const pathSection = h("div", { class: "hc-pathsec" },
    h("div", { class: "hc-title", title: t("Multi-step cadences from the current chord – the best path for each cadence type, plus a longer one where available.\nHover: show in the circle · Click: pin") }, t("Paths to a cadence"), pathFrom),
    pathBox);
  const left = h("div", { class: "hc-left" }, circleBox, legend, pathSection);
  const side = h("div", { class: "hc-side" },
    sketchBox,
    h("div", { class: "hc-title" }, t("Continuations"), libLine),
    suggBox,
    h("div", { class: "hc-title" }, t("Cadences")),
    cadBox);
  const bodyEl = h("div", { class: "hc-body" }, left, side);
  const el = h(
    "div",
    { class: "harmony" },
    h("div", { class: "hc-head" }, field(t("Key"), keySel), field(t("Duration"), durSeg), field(t("Target"), targetSel)),
    bodyEl,
  );
  // Wide (two columns): the paths to a cadence on the right below the progression - on the left
  // circle and legend stay alone and stay put while scrolling. Narrow: below the circle.
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

  // ------------------------------------------------------------ Data

  let loadSeq = 0;
  async function loadView(): Promise<void> {
    const seq = ++loadSeq;
    try {
      const v = await backend.harmonyView(st.keyChoice);
      if (seq !== loadSeq) return;
      const keyChanged = !st.view || st.view.key.tonic !== v.key.tonic || st.view.key.minor !== v.key.minor;
      st.view = v;
      // Only the initial tonic in the progression: start over with the new key
      if (keyChanged && st.sketch.length === 1 && !st.sketch[0]!.fixed) st.sketch = [];
      if (keyChanged && st.sketch.length) {
        const infos = await backend.harmonyDescribe(currentKey(), st.sketch.map((s) => s.chord));
        st.sketch = st.sketch.map((s, i) => ({ ...s, ...infos[i]! }));
      }
      if (!st.sketch.length) {
        // beginning: tonic
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

  // ------------------------------------------------------------ Sound

  const preview = (chords: Chord[], bars: number) =>
    backend.harmonyPreview(currentKey(), chords, Math.max(1, bars * beatsPerBar()), targetTrackId(), null).catch((e) => notify(errorText(e), "error"));

  /** Choose a candidate and preview the transition from the last chord. */
  function tryChord(info: ChordInfo): void {
    st.candidate = info;
    const last = st.sketch.at(-1);
    void preview(last ? [last.chord, info.chord] : [info.chord], 0.5);
    renderCircle();
    renderSketch();
    renderSuggestions();
  }

  /**
   * As soon as one works on the progression, the key stays fixed - otherwise the
   * estimate would jump after every chord written and reinterpret everything.
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
        notify(t("No chords detected – single-voice passages don't form a chord."));
        return;
      }
      pinKey();
      st.sketch = placed.map((p) => ({ ...p, fixed: true }));
      st.candidate = null;
      notify(track
        ? plural(placed.length, "Read {n} chord from \"{track}\"", "Read {n} chords from \"{track}\"", { track: track.name })
        : plural(placed.length, "Read {n} chord from all instrument tracks", "Read {n} chords from all instrument tracks"));
      await loadSuggestions();
    } catch (e) {
      notify(errorText(e), "error");
    }
  }

  async function write(): Promise<void> {
    pinKey();
    const fresh = st.sketch.filter((s) => !s.fixed);
    if (!fresh.length) {
      notify(t("Nothing new to write – add chords first (double-click or \"+\")."));
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
    let pos = start;
    st.sketch = st.sketch.map((s) => {
      if (s.fixed) return s;
      const placed = { ...s, fixed: true, startBeats: pos, lengthBeats: bpc };
      pos += bpc;
      return placed;
    });
    const where = targetTrackId() != null ? melodicTracks().find((x) => x.id === targetTrackId())?.name ?? "" : t("Chords");
    const vars = { track: where, bar: Math.floor(start / bpb) + 1 };
    notify(mode === "einfuegen"
      ? plural(fresh.length, "Inserted {n} chord into \"{track}\" from bar {bar} (everything after it shifts later)", "Inserted {n} chords into \"{track}\" from bar {bar} (everything after it shifts later)", vars)
      : plural(fresh.length, "Wrote {n} chord to \"{track}\" from bar {bar}", "Wrote {n} chords to \"{track}\" from bar {bar}", vars));
    // cursor on the last new chord: the progression continues from there
    const lastStart = pos - bpc;
    followIx = -2;
    void backend.seek((lastStart * 60) / bpm).catch(() => {});
    renderSketch();
  }

  // ------------------------------------------------------------ Rendering

  function renderHead(): void {
    const v = st.view;
    const auto = v && v.key.source !== "gewaehlt"
      ? v.key.source === "standard" ? t("Automatic ({key}, no notes)", { key: v.key.name }) : t("Automatic ({key}, estimated)", { key: v.key.name })
      : t("Automatic");
    const opts = [h("option", { value: "auto" }, auto)];
    for (const minor of [false, true]) {
      for (let i = 0; i < 12; i++) {
        const tonic = (i * 7 + (minor ? 9 : 0)) % 12; // sorted by circle of fifths
        opts.push(h("option", { value: `${tonic}:${minor ? "m" : "d"}` }, keyName({ tonic, minor })));
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
      h("option", { value: "akkorde" }, t("Track \"{name}\"", { name: t("Chords") })),
      ...tracks.filter((tr) => !isChordTrack(tr.name)).map((tr) => h("option", { value: String(tr.id) }, t("Track \"{name}\"", { name: tr.name }))),
    );
    if (typeof st.target === "number" && !tracks.some((t) => t.id === st.target)) st.target = "akkorde";
    targetSel.value = String(st.target);
  }

  function renderCircle(): void {
    const v = st.view;
    if (!v || !v.circle.length) {
      circleBox.replaceChildren(h("div", { class: "lib-empty" }, t("Loading circle of fifths…")));
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

    const root = svg("svg", { viewBox: "-212 -212 424 424", class: "hc-svg", role: "img", "aria-label": t("Circle of fifths in {key}", { key: v.key.name }) });
    // arrowheads per mood (markers do not reliably inherit the stroke color)
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
      const label = svg("text", { x: cx, y: cy + (inKey || s ? -2 : 4), class: "hc-name" }, chordName(cell.name));
      g.append(path, label);
      if (inKey || (s && s.rank <= 8)) g.append(svg("text", { x: cx, y: cy + 11, class: "hc-roman" }, cell.roman));
      const tip = `${chordName(cell.name)} · ${cell.roman} · ${cell.function} – ${cell.role}${s ? `\n${t("Suggestion #{rank}: {reason}", { rank: s.rank, reason: s.s.reason })}` : ""}${s?.s.library ? `\n${t("Library: {pct}%", { pct: pct(s.s.library.share) })}` : ""}\n${t("Click: hear the transition · Double-click: add")}`;
      g.append(svg("title", {}, tip));
      g.addEventListener("click", () => tryChord(cell));
      g.addEventListener("dblclick", () => accept(cell));
      root.append(g);
    }

    // arrows from the current chord to the best suggestions, colored by mood
    // (faded while a path or a cadence is shown)
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
    // rank badges
    for (const [key, { rank }] of rankOf) {
      if (rank > 8) continue;
      const [ring, pos] = key.split(":").map(Number) as [number, number];
      const [bx, by] = pt(RINGS[ring]![1] - 9, pos * 30 - 90 + 10);
      root.append(svg("g", { class: `hc-badge${cadence || path ? " dim" : ""}` }, svg("circle", { cx: bx, cy: by, r: 7.5 }), svg("text", { x: bx, y: by + 3.2 }, String(rank))));
    }
    // path to a cadence (from the current chord) or cadence template, numbered
    if (path && last) root.append(route([last, ...path.steps.map((s) => s.chord)], path.moods, true));
    else if (cadence) root.append(route(cadence.steps.map((s) => s.chord), cadence.moods ?? [], false));
    // center: key and current chord
    const cur = st.candidate ?? st.sketch.at(-1) ?? null;
    root.append(
      svg("circle", { r: 62, class: "hc-center" }),
      svg("text", { y: -22, class: "hc-key" }, v.key.name),
      svg("text", { y: -8, class: "hc-keysrc" }, v.key.source === "gewaehlt" ? t("selected") : v.key.source === "geschaetzt" ? t("from the notes") : t("default")),
      svg("text", { y: 16, class: "hc-cur" }, cur ? chordName(cur.name) : "–"),
      svg("text", { y: 31, class: "hc-curfn" }, cur ? `${cur.roman} · ${cur.function}` : ""),
    );
    circleBox.replaceChildren(root);
  }

  /** From this index on, the chords in the progression briefly light up (just appended). */
  let flashFrom = -1;
  let flashTimer = 0;

  function chip(item: SketchItem, i: number, isLast: boolean): HTMLElement {
    const b = h("button", {
      class: `hc-chip${item.fixed ? " fixed" : ""}${isLast ? " last" : ""}${flashFrom >= 0 && i >= flashFrom ? " new" : ""}`,
      title: `${chordName(item.name)} · ${item.roman} · ${item.function} – ${item.role}${item.fixed ? `\n${t("already in the track")}` : ""}\n${t("Click: listen")}`,
      onclick: () => void preview([item.chord], 0.5),
    }, h("b", {}, chordName(item.name)), h("span", {}, item.roman));
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
        h("span", { class: "hc-chip cand", title: `${chordName(c.name)} · ${c.roman} · ${c.function} – ${c.role}` }, h("b", {}, chordName(c.name)), h("span", {}, c.roman),
          h("button", { class: "btn small blue", title: t("Add to the progression (or double-click)"), onclick: () => accept(c) }, t("+ Add"))));
    }
    const canBack = !!st.sketch.length && !st.sketch.at(-1)!.fixed;
    sketchBox.replaceChildren(
      h("div", { class: "hc-title" }, t("Progression"),
        h("span", { class: "muted" }, ` – ${t("Click a chord: hear the transition, double-click: add")}`)),
      h("div", { class: "hc-chips" }, ...arrows),
      h("div", { class: "hc-actions" },
        h("button", { class: "btn small", title: t("Play the whole progression"), onclick: () => st.sketch.length && void preview(st.sketch.map((s) => s.chord), st.barsPerChord) }, icon("play"), t("Progression")),
        h("button", { class: "btn small", title: t("Remove last chord"), disabled: !canBack, onclick: back }, icon("undo"), t("Back")),
        h("button", { class: "btn small ghost", title: t("Start over (with the tonic)"), onclick: () => { st.sketch = []; st.candidate = null; holdFollow(); void loadView(); } }, t("Clear")),
        h("button", { class: "btn small", title: t("Read chords from the selected clip (with nothing selected: from all instrument tracks)"), onclick: () => void fromTracks() }, icon("clip"), t("Read from track")),
        modeSeg(),
        h("button", { class: "btn small primary", title: st.insertMode === "einfuegen"
          ? t("Insert new chords after the chord at the cursor – everything after it shifts later in all tracks")
          : t("Write new chords after the chord at the cursor – whatever is there in the target track gets replaced"), onclick: () => void write() }, icon("plus"), t("Write to track"))),
      h("label", { class: "check small hc-follow", title: t("During playback and when you set the cursor, the progression picks up the chords at that position (your own new chords stay)") },
        followBox(), t("follows playback position")),
    );
  }

  function modeSeg(): HTMLElement {
    const seg = h("div", { class: "seg mini hc-mode", title: t("How \"Write to track\" places the new chords") });
    for (const [m, label, tip] of [
      ["einfuegen", t("Insert"), t("Make room: everything after it shifts later in all tracks")],
      ["ersetzen", t("Replace"), t("overwrite what follows in the target track")],
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

  // ------------------------------------------------------------ Playhead

  /** Index and content of the chord picked up last (-2 = pick up again). */
  let followIx = -2;
  let followSig = "";
  let followTimer = 0;
  /** After "Clear": follow again only once the cursor moves (position in seconds at clearing). */
  let followHold: number | null = null;
  function holdFollow(): void {
    followHold = status.peek().positionSecs;
  }
  /**
   * Adjust the progression to the playhead: the chords of the piece up to the
   * point (the chord there and up to three before it) - new chords of one's own stay after it.
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
    const name = `${who}${e.title || t("(untitled)")}`;
    return h("div", { class: "hc-ex" },
      h("span", { class: "hc-ex-name", title: t("{title}, bar {bar}", { title: name, bar: e.bar }) }, name, h("em", {}, ` · ${t("Bar {bar}", { bar: e.bar })}`)),
      h("button", { class: "btn small ghost", title: t("Listen to this passage"), onclick: () => void backend.harmonyPlayExample(e.doc, e.start, e.end).catch((x) => notify(errorText(x), "error")) }, icon("play")),
      h("button", { class: "btn small ghost", title: t("Open the score at this spot in the \"Library\" window"), onclick: () => void openInLibrary(e.doc, [e.start, e.end]) }, icon("book"), t("Open")));
  }

  function suggestionRow(s: ChordSuggestion, rank: number, maxShare: number): HTMLElement {
    const k = ckey(s.chord);
    const lib = s.library;
    const isCand = !!st.candidate && same(st.candidate.chord, s.chord);
    const prevName = chordName(st.sketch.at(-1)?.name ?? "");
    const share = lib
      ? h("div", {
          class: "hc-share",
          title: lib.exact
            ? t("{count} of {total} continuations after {chord} in the library", { count: lib.count.toLocaleString(language()), total: lib.total.toLocaleString(language()), chord: prevName })
            : t("{count} of {total} continuations after {chord} in the library (counted as root position – there's too little data for this form)", { count: lib.count.toLocaleString(language()), total: lib.total.toLocaleString(language()), chord: prevName }),
        },
          h("i", { style: `width:${maxShare > 0 ? Math.max(2, (lib.share / maxShare) * 100) : 0}%` }),
          h("b", {}, `${pct(lib.share)} %`))
      : null;
    const m = s.mood ?? "hell";
    const row = h("div", { class: `hc-row${isCand ? " cand" : ""}` },
      h("span", { class: "hc-rank" },
        h("i", { class: `hc-mood m-${m}`, "data-mood": m, title: `${prevName} → ${chordName(s.name)}: ${moodLabel(m)} – ${MOODS.find((x) => x[0] === m)?.[2] ?? ""}` }),
        String(rank)),
      h("button", { class: "hc-pick", title: t("Hear the transition (double-click: add)"), onclick: () => tryChord(s), ondblclick: () => accept(s) },
        h("b", {}, chordName(s.name)),
        h("span", { class: "hc-fn" }, `${s.roman} · ${s.function}`),
        h("span", { class: "hc-why" }, s.reason, s.toKey ? h("em", {}, ` → ${s.toKey}`) : null)),
      share,
      lib && lib.examples.length
        ? h("button", { class: `btn small ghost hc-exbtn${st.expanded === k ? " on" : ""}`, title: t("Examples from the library"),
            onclick: () => { st.expanded = st.expanded === k ? null : k; renderSuggestions(); } }, `${t("{n} ex.", { n: lib.examples.length })} ${st.expanded === k ? "▴" : "▾"}`)
        : h("span"),
      h("button", { class: "btn small blue", title: t("Add to the progression"), onclick: () => accept(s) }, "+"),
    );
    if (st.expanded !== k || !lib) return row;
    return h("div", { class: "hc-rowwrap" }, row, h("div", { class: "hc-exlist" }, ...lib.examples.map(exampleRow)));
  }

  function renderSuggestions(): void {
    const r = st.result;
    if (r?.library) {
      const lib = r.library;
      const groups = lib.groups.length > 1
        ? h("span", { class: "seg mini hc-groups", title: t("Where the frequencies come from") },
            ...lib.groups.map((g) => {
              const count = g.scores.toLocaleString(language());
              const b = h("button", { class: `seg-btn${g.id === lib.group ? " on" : ""}`, title: currentKey().minor
                ? plural(g.scores, "{count} minor-key piece with a chord progression", "{count} minor-key pieces with chord progressions", { count })
                : plural(g.scores, "{count} major-key piece with a chord progression", "{count} major-key pieces with chord progressions", { count }),
                onclick: () => { st.group = g.id; void loadSuggestions(); } }, g.label);
              return b;
            }))
        : null;
      const libVars = { name: lib.name, count: lib.scores.toLocaleString(language()) };
      libLine.replaceChildren(
        h("span", { class: "muted" }, ` · ${currentKey().minor
          ? plural(lib.scores, "Frequency in \"{name}\" ({count} minor-key piece)", "Frequency in \"{name}\" ({count} minor-key pieces)", libVars)
          : plural(lib.scores, "Frequency in \"{name}\" ({count} major-key piece)", "Frequency in \"{name}\" ({count} major-key pieces)", libVars)}`),
        groups ?? "",
        r.hint ? h("span", { class: "muted" }, ` · ${r.hint}`) : "");
    } else if (r?.hint) {
      const reindex = r.reindex
        ? h("button", { class: "btn small blue", onclick: () => void backend.libraryReindex().then(() => notify(t("Library is being re-indexed – progress in the \"Library\" window"))).catch((e) => notify(errorText(e), "error")) }, t("Re-index"))
        : null;
      libLine.replaceChildren(h("span", { class: "muted" }, ` · ${r.hint}`), reindex ?? "");
    } else {
      libLine.replaceChildren();
    }
    const list = r?.suggestions ?? [];
    if (!list.length) {
      suggBox.replaceChildren(h("div", { class: "lib-empty" }, st.view ? t("No suggestions.") : "…"));
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

  /** Chord progression with arrows in the mood's color (moods[i] leads into chords[i + 1]). */
  function moodSeq(chords: ChordInfo[], moods: Mood[], opt: { from?: boolean; shares?: Array<number | null> | null } = {}): Node[] {
    const out: Node[] = [];
    chords.forEach((s, i) => {
      if (i) {
        const m = moods[i - 1] ?? "hell";
        const sh = opt.shares?.[i - 1];
        const prev = chordName(chords[i - 1]!.name);
        // with a library: the transition probability right below the arrow
        out.push(h("i", { class: `hc-pto m-${m}${sh != null ? " has-pct" : ""}`, "data-mood": m, title: `${prev} → ${chordName(s.name)}: ${moodLabel(m)}${sh != null ? `\n${t("Library: {pct}% of continuations after {chord}", { pct: pct(sh), chord: prev })}` : ""}` },
          ...(sh != null ? [h("span", {}, "→"), h("small", { class: "hc-pct" }, `${pct(sh)} %`)] : ["→"])));
      }
      const from = i === 0 && opt.from;
      out.push(h("span", { class: `hc-pchip${from ? " from" : ""}`, title: `${chordName(s.name)} · ${s.roman} · ${s.function} – ${s.role}${from ? `\n${t("(current chord)")}` : ""}` }, h("b", {}, chordName(s.name)), h("span", {}, s.roman)));
    });
    return out;
  }

  /** Pin or release the overlay and redraw everything it shows. */
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
   * Append chords to the progression - visibly: the progression scrolls into view, the
   * new chords light up briefly. `write`: write straight into the target track.
   */
  function appendSteps(steps: ChordInfo[], what: string, toTrack = false): void {
    if (!steps.length) {
      notify(t("The progression already ends that way – nothing to append."));
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
    if (toTrack) void write();
    else notify(plural(steps.length, "{what}: appended {n} chord to the progression – \"Write to track\" puts it in the track", "{what}: appended {n} chords to the progression – \"Write to track\" puts them in the track", { what }));
  }

  function renderPaths(): void {
    const cur = st.sketch.at(-1);
    pathFrom.textContent = cur ? ` · ${t("from {chord}", { chord: chordName(cur.name) })}` : "";
    const paths = st.result?.paths ?? [];
    if (!paths.length) {
      pathBox.replaceChildren(h("div", { class: "lib-empty" }, st.result ? t("No cadence within reach.") : "…"));
      return;
    }
    pathBox.replaceChildren(...paths.map((p) => {
      const o: Overlay = { kind: "path", id: pathId(p) };
      const pinned = sameOverlay(st.pinned, o);
      const libAvg = p.shares && p.shares.length ? p.shares.reduce<number>((a, x) => a + (x ?? 0), 0) / p.shares.length : null;
      const tip = `${p.longer ? t("{label} (longer path)", { label: p.label }) : p.label}: ${[chordName(cur?.name ?? ""), ...p.steps.map((s) => chordName(s.name))].join(" → ")}`
        + `\n${t("Rating {score}", { score: p.score.toLocaleString(language()) })}${libAvg != null ? ` · ${t("Steps average {pct}% in the library", { pct: pct(libAvg) })}` : ""}`
        + `\n${t("Hover: show in the circle · Click: pin")}`;
      const row = h("div", { class: `hc-path${p.longer ? " longer" : ""}${pinned ? " on" : ""}`, title: tip },
        h("span", { class: "hc-plabel" }, p.longer ? t("longer") : p.label),
        h("span", { class: "hc-pseq" }, ...(cur
          ? moodSeq([cur, ...p.steps], p.moods, { from: true, shares: p.shares })
          : moodSeq(p.steps, p.moods.slice(1), { shares: p.shares?.slice(1) }))),
        h("span", { class: "hc-pbtns" },
          h("button", { class: "btn small ghost", title: t("Play from the current chord to the cadence and pin it in the circle"), onclick: (e: Event) => {
            e.stopPropagation();
            togglePin(o, true);
            void preview([...(cur ? [cur.chord] : []), ...p.steps.map((s) => s.chord)], st.barsPerChord);
          } }, icon("play")),
          h("button", { class: "btn small blue", title: t("Append these chords to the progression (to keep experimenting)"), onclick: (e: Event) => {
            e.stopPropagation();
            st.pinned = null;
            hover = null;
            appendSteps(p.steps, p.label);
          } }, "+"),
          h("button", { class: "btn small", title: t("Append and write straight to the target track in four parts"), onclick: (e: Event) => {
            e.stopPropagation();
            st.pinned = null;
            hover = null;
            appendSteps(p.steps, p.label, true);
          } }, t("To track"))));
      row.addEventListener("mouseenter", () => showHover(o));
      row.addEventListener("mouseleave", () => showHover(null));
      row.addEventListener("click", () => togglePin(o));
      return row;
    }));
  }

  /** Cadence without its first chord, if the progression already ends on it. */
  function cadenceTail(steps: ChordInfo[]): ChordInfo[] {
    const last = st.sketch.at(-1);
    return last && steps[0] && same(last.chord, steps[0].chord) ? steps.slice(1) : steps;
  }

  function renderCadences(): void {
    const v = st.view;
    if (!v) return;
    cadBox.replaceChildren(...v.cadences.map((c) => {
      const o: Overlay = { kind: "cadence", id: c.id };
      const card = h("div", { class: `hc-cadcard${sameOverlay(st.pinned, o) ? " on" : ""}`, title: `${c.description}\n${c.steps.map((s) => `${chordName(s.name)} (${s.roman})`).join(" – ")}\n${t("Hover: show in the circle · Click: pin")}` },
        h("b", {}, c.name), h("span", { class: "hc-pseq" }, ...moodSeq(c.steps, c.moods ?? [])),
        h("div", {},
          h("button", { class: "btn small", title: t("Play and pin in the circle"), onclick: (e: Event) => {
            e.stopPropagation();
            togglePin(o, true);
            void preview(c.steps.map((s) => s.chord), st.barsPerChord);
          } }, icon("play")),
          h("button", { class: "btn small blue", title: t("Append to the progression (above) – to keep experimenting"), onclick: (e: Event) => {
            e.stopPropagation();
            st.pinned = o;
            appendSteps(cadenceTail(c.steps), c.name);
          } }, t("+ Append")),
          h("button", { class: "btn small", title: t("Append and write straight to the target track in four parts"), onclick: (e: Event) => {
            e.stopPropagation();
            st.pinned = o;
            appendSteps(cadenceTail(c.steps), c.name, true);
          } }, t("To track"))));
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

  // ------------------------------------------------------------ Lifecycle

  let lastSnap: Snapshot | null = null;
  let timer = 0;
  renderSketch();
  renderCircle();
  void loadView();
  // Load the chord progression of the piece; the progression follows the playhead
  watchHarmonyLine();
  harmonyKey.value = st.keyChoice;
  effect(() => {
    status.value;
    harmonyLine.value;
    followCursor();
  });
  // Working in the window: show the harmony band in the timeline
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
  // Library loaded or re-indexed: catch up on frequencies
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
        // Estimated key follows the notes (throttled, editing produces many snapshots)
        if (!first && !st.keyChoice) {
          clearTimeout(timer);
          timer = window.setTimeout(() => void loadView(), 600);
        }
      }
    },
  };
}
