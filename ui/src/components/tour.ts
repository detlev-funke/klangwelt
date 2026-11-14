// Intro-Tour: geht Schritt für Schritt durch alle Fenster. Das jeweilige
// Fenster ist ausgeschnitten (der Rest abgedunkelt), ein großer Zettel
// erklärt es, kleine Post-its kleben an einzelnen Funktionen. Weiter mit
// → / Enter, zurück mit ←, Esc beendet. Am Ende sind die Fenster wieder so
// aufgeschlagen wie vorher.
//
// Damit die Fenster etwas zeigen, lädt die Tour im leeren Projekt die Demo
// (ein Rückgängig-Schritt) und füllt jedes Fenster: Clip gewählt, Analyse der
// Arpeggio-Aufnahme, Klang der Melodie, Fugenthema, Harmonie an der
// Abspielposition. Danach verschwindet die Demo wieder – außer mit „Demo behalten“.

import { seekBeats, stopToStart } from "../actions.js";
import { backend } from "../bridge.js";
import { h } from "../dom.js";
import { t } from "../i18n.js";
import { act, analysis, type PanelId, panelEmphasis, selection, showPanel, snapshot, visiblePanels } from "../store.js";
import { helpShow } from "./help.js";

/** Was die Tour aus der Demo zeigt. */
interface Demo {
  loaded: boolean;
  melody: { trackId: number; clipId: number } | null;
  audio: { trackId: number; clipId: number; assetId: string } | null;
  analysed: boolean;
  /** Die Tour hat den Download-Katalog der Bibliothek aufgeklappt. */
  catalogOpened: boolean;
}

function findDemo(d: Demo): void {
  const tracks = snapshot.peek()?.project.tracks ?? [];
  // track name of the demo in the language it was loaded in (the core translates with the same dictionary)
  const arp = t("Arpeggio").toLowerCase();
  for (const track of tracks) {
    for (const c of track.clips) {
      if (!d.melody && track.kind === "instrument" && track.instrument?.kind !== "drums" && c.source.type === "notes" && c.source.notes.length >= 4) {
        d.melody = { trackId: track.id, clipId: c.id };
      }
      const isArp = /arpeggio/i.test(track.name) || track.name.toLowerCase().includes(arp);
      if (c.source.type === "audio" && (!d.audio || isArp)) d.audio = { trackId: track.id, clipId: c.id, assetId: c.source.assetId };
    }
  }
}

interface Note {
  sel: string;
  text: string;
}

interface Step {
  panel?: PanelId;
  /** Vor dem Zeigen: Fenster mit Demo-Inhalt füllen (Auswahl, Analyse …). */
  prepare?: (d: Demo) => Promise<void> | void;
  /** Wenn das Fenster zu sehen ist (z. B. Auswahllisten im Fenster setzen). */
  after?: (d: Demo) => void;
  /** Beim Weiterblättern oder Beenden: Aufgeklapptes wieder schließen. */
  leave?: (d: Demo) => void;
  /** Mindestbreite des Fensters in diesem Schritt (z. B. damit es zweispaltig wird). */
  minWidth?: number;
  /** Hervorgehobene Fläche: Selektor; bei Fenstern deren Stapel samt Reiter. */
  target?: string;
  title: string;
  text: string;
  notes?: Note[];
}

const panelArea = (id: PanelId) => (id === "spuren" ? "main.workspace" : id === "hilfe" ? "kw-help" : `kw-panel[tab="${id}"]`);

const select = (v: { trackId: number; clipId: number } | null) => {
  selection.value = v ? { trackId: v.trackId, clipId: v.clipId } : null;
};

const STEPS: Step[] = [
  {
    title: t("Welcome to Klangwelt"),
    text: t("A short tour of all windows – one is highlighted at a time, and notes explain the most important features. The demo is loaded so there's something to see. Continue with → or Enter, go back with ←, Esc ends the tour at any time."),
  },
  {
    target: "kw-transport",
    title: t("The top bar"),
    text: t("Project and playback: save, export, play, record, tempo and time signature, metronome and loop, undo. On the right: windows, settings, levels and the connection to the agent."),
    notes: [
      { sel: ".file-controls", text: t("File, save and export – as WAV, FLAC, Opus, stems or MIDI (Ctrl+E).") },
      { sel: "kw-transport .btn.big.primary", text: t("Space: play. If a time range is selected, exactly that.") },
      { sel: "[data-help=tempo]", text: t("Tempo in BPM – everything follows instantly.") },
      { sel: "[data-help=fenster]", text: t("Show and hide windows, reset the layout.") },
    ],
  },
  {
    panel: "spuren",
    target: panelArea("spuren"),
    prepare: () => select(null),
    title: t("Tracks"),
    text: t("The main window: track headers on the left (name, mute/solo, level, view), the timeline with the clips on the right. The demo has two recordings (beat, arpeggio) and a melody as notes – Alt+click adds a note."),
    notes: [
      { sel: "[data-help=neue-instrumentspur]", text: t("New track: pluck, organ, harpsichord, bass, synth or drums.") },
      { sel: ".track-row .toggle.m", text: t("Mute and solo – also apply to the export.") },
      { sel: ".track-row.notes canvas.keys", text: t("Keyboard: click plays a note, mouse wheel scrolls.") },
      { sel: ".empty-hint .row", text: t("Quick start: demo, complete song, beat or recording.") },
    ],
  },
  {
    panel: "clip",
    target: panelArea("clip"),
    prepare: (d) => select(d.melody),
    title: t("Clip"),
    text: t("Properties of the selected clip – here the melody clip \"Thema\" with its performance markings: pp … ff, crescendo, staccato, vibrato. For recordings: detect tempo, analyze, turn the melody into notes."),
    notes: [{ sel: "kw-panel[tab=clip] .mark.dyn", text: t("Dynamics for the selected notes – with nothing selected, for all of them.") }],
  },
  {
    panel: "analyse",
    target: panelArea("analyse"),
    prepare: async (d) => {
      if (!d.audio) return;
      select(d.audio);
      if (d.analysed) return;
      try {
        analysis.value = await backend.analyzeAsset(d.audio.assetId);
        d.analysed = true;
      } catch {
        /* ohne Analyse zeigt das Fenster seinen Hinweis */
      }
    },
    title: t("Analysis"),
    text: t("What's inside a recording – here the demo's arpeggio: pitch curve, detected notes, tempo and key. Double-click an audio clip to open it."),
  },
  {
    panel: "klang",
    target: panelArea("klang"),
    prepare: (d) => select(d.melody),
    title: t("Sound – the sound workshop"),
    text: t("The melody's sound (a pluck) in three fields: character, timbre and space. Drag the dot or click a named starting point; fine-tuning has all the controls. Under \"My sounds\" you save settings for all projects."),
    notes: [
      { sel: "kw-panel[tab=klang] .snd-kinds", text: t("Switch instrument – the notes stay.") },
      { sel: "kw-panel[tab=klang] .snd-field[data-field=charakter] .snd-mark", text: t("Starting points: one click sets the whole field.") },
    ],
  },
  {
    panel: "rhythmus",
    target: panelArea("rhythmus"),
    prepare: () => select(null),
    title: t("Rhythm"),
    text: t("Beats for a quick start: patterns from 14 genres and a step grid – one row per drum, a click sets a hit. \"Write to track\" creates the beat."),
    notes: [
      { sel: ".rh-genre", text: t("Genres from house to dubstep – each with a tempo and a tip.") },
      { sel: ".rh-step", text: t("Click sets or clears a hit.") },
    ],
  },
  {
    panel: "song",
    target: panelArea("song"),
    title: t("Song"),
    text: t("Complete pieces from templates. Columns are sections (intro, build-up, peak…), rows are roles (pad, bass, kick…), each cell off, sparse or full. \"Generate song\" writes all the tracks."),
    notes: [
      { sel: ".song-tpl", text: t("Templates: Trance, Ambient, Deep House, Synthwave, Lo-Fi.") },
      { sel: ".song-cell", text: t("Click: more · Shift+click: less.") },
    ],
  },
  {
    panel: "bibliothek",
    target: panelArea("bibliothek"),
    after: (d) => {
      // die Download-Übersicht aufklappen (danach wieder zu, falls sie zu war)
      const cat = document.querySelector<HTMLElement>("kw-panel[tab=bibliothek] .lib-catalog");
      const btn = document.querySelector<HTMLButtonElement>("kw-panel[tab=bibliothek] [data-help=lib-herunterladen]");
      if (cat?.hidden && btn) {
        btn.click();
        d.catalogOpened = true;
      }
    },
    leave: (d) => {
      if (!d.catalogOpened) return;
      d.catalogOpened = false;
      const cat = document.querySelector<HTMLElement>("kw-panel[tab=bibliothek] .lib-catalog");
      if (cat && !cat.hidden) document.querySelector<HTMLButtonElement>("kw-panel[tab=bibliothek] [data-help=lib-herunterladen]")?.click();
    },
    title: t("Library"),
    text: t("Scores to search, listen to and add. Under \"Download\" you'll find the packs: Bach chorales, OpenScore, KernScores, PDMX with over 250,000 pieces – each with its size and license. Melody search finds pieces from notes or a sung recording."),
    notes: [
      { sel: "kw-panel[tab=bibliothek] [data-help=lib-herunterladen]", text: t("Expand and collapse the catalog of music libraries.") },
      { sel: "kw-panel[tab=bibliothek] .lib-cards", text: t("Clicking a pack downloads it – in the background.") },
    ],
  },
  {
    panel: "fuge",
    target: panelArea("fuge"),
    prepare: (d) => select(d.melody),
    after: (d) => {
      // das Thema der Demo als Fugenthema wählen (nur, wenn noch keins gewählt ist)
      const sel = document.querySelector<HTMLSelectElement>("kw-panel[tab=fuge] .fg-subject");
      const v = d.melody ? `${d.melody.trackId}:${d.melody.clipId}` : "";
      if (sel && v && !sel.value && [...sel.options].some((o) => o.value === v)) {
        sel.value = v;
        sel.dispatchEvent(new Event("change"));
      }
    },
    title: t("Fugue"),
    text: t("Counterpoint tools, here with the demo clip \"Thema\": answer, inversion, retrograde, strettos, countersubjects and a complete fugue plan. The checker marks parallels and dissonances in the timeline."),
    notes: [{ sel: "kw-panel[tab=fuge] .fg-subject", text: t("The subject: a note clip of 2–4 bars.") }],
  },
  {
    panel: "harmonie",
    target: panelArea("harmonie"),
    prepare: () => seekBeats(4.5),
    minWidth: 780,
    after: () => {
      // nach oben: links der Zirkel, rechts Verlauf und Wege zum Schluss
      const body = document.querySelector<HTMLElement>("kw-panel[tab=harmonie] .tab-body");
      if (body) body.scrollTop = 0;
    },
    title: t("Harmony"),
    text: t("The circle of fifths follows the playback position – here bar 2 of the demo. On the right, the paths to a cadence: multi-step cadences from the current chord; the number under each arrow is the transition probability – how often exactly this step follows in the library's pieces. Below that, the continuations by rank, also with their probability. Click plays a transition, double-click adds it."),
    notes: [
      { sel: "kw-panel[tab=harmonie] .hc-paths", text: t("Paths to a cadence – under the arrows: transition probability from the library.") },
      { sel: "kw-panel[tab=harmonie] .hc-pick", text: t("Continuation with rank – the bar shows how often it follows in the library.") },
    ],
  },
  {
    panel: "agent",
    target: panelArea("agent"),
    title: t("Agent"),
    text: t("Tasks for an AI agent like Claude – \"Make the kick deeper\", \"Write a bass line for the selected bars\". It sees which tracks are audible and which time range is selected."),
    notes: [{ sel: ".chat-input", text: t("Type a task, Enter sends it.") }],
  },
  {
    panel: "hilfe",
    target: panelArea("hilfe"),
    after: () => {
      const play = document.querySelector("kw-transport .btn.big.primary");
      if (play) helpShow(play);
    },
    title: t("Help"),
    text: t("This small window explains what the mouse is pointing at – here the play button. F1 pins the text and brings the window back if you've closed it."),
  },
  {
    title: t("Have fun!"),
    text: t("Windows can be moved, docked and floated by their tabs; \"Windows ▾\" resets the layout. \"Done\" removes the demo again – or keep it to experiment with."),
  },
];

let running: { stop(): Promise<void> } | null = null;

export function tourRunning(): boolean {
  return !!running;
}

/** Tour beenden (nimmt die Demo zurück); fertig, wenn alles aufgeräumt ist. */
export function endTour(): Promise<void> {
  return running?.stop() ?? Promise.resolve();
}

/** Startet die Tour (ein zweiter Aufruf während sie läuft, tut nichts). */
export function startTour(): void {
  if (running) return;
  const before = [...visiblePanels.peek()] as PanelId[];
  const demo: Demo = { loaded: false, melody: null, audio: null, analysed: false, catalogOpened: false };
  let index = 0;
  let seq = 0;
  const root = h("div", { class: "tour", role: "dialog", "aria-label": t("Intro tour") });
  document.body.append(root);

  /** Ende der Tour; `keep`: die Demo bleibt geladen. */
  const stop = (keep = false): Promise<void> => {
    seq++;
    STEPS[index]?.leave?.(demo);
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("resize", layout);
    root.remove();
    running = null;
    panelEmphasis.value = null;
    return (async () => {
      if (demo.loaded && !keep) {
        // nur die eigene Demo zurücknehmen – hat sich inzwischen etwas anderes getan, bleibt alles
        if (isDemoUndo()) await act(backend.undo());
        if (demo.analysed) analysis.value = null;
        selection.value = null;
        await stopToStart();
      }
      // vorher aufgeschlagene Reiter wieder nach vorn (die Hilfe bleibt offen);
      // Fensterwünsche werden gebündelt – also nacheinander
      for (const id of before) {
        if (id === "hilfe") continue;
        showPanel(id);
        await new Promise((r) => setTimeout(r, 0));
      }
    })();
  };
  running = { stop: () => stop() };

  const frames = () => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r())));
  const go = async (i: number) => {
    if (i < 0) return;
    if (i >= STEPS.length) return stop();
    const my = ++seq;
    if (i !== index) STEPS[index]?.leave?.(demo);
    index = i;
    const s = STEPS[i]!;
    await s.prepare?.(demo);
    if (my !== seq) return; // inzwischen weitergeblättert oder beendet
    if (s.panel) showPanel(s.panel);
    // kleine Fenster (die unteren Werkzeuge, der Agent) bekommen solange mehr Platz
    panelEmphasis.value = s.panel && s.panel !== "spuren" && s.panel !== "hilfe" ? { id: s.panel, minWidth: s.minWidth } : null;
    // erst nach dem Umbau des Layouts messen
    await frames();
    if (my !== seq) return;
    s.after?.(demo);
    layout();
    // Fenster ordnen sich nach dem Vergrößern teils noch um (z. B. die Harmonie
    // auf zwei Spalten) – kurz danach noch einmal messen
    setTimeout(() => {
      if (my === seq && running) layout();
    }, 350);
  };

  /** Is the last step loading the demo? (label from the core, translated with the same dictionary) */
const isDemoUndo = () => [t("Load demo"), "Load demo"].includes(snapshot.peek()?.undoLabel ?? "");

/** Load the demo (only in an empty project), then the first step. */
  const begin = async () => {
    if (!snapshot.peek()?.project.tracks.length) {
      root.append(h("div", { class: "tour-shade" }));
      demo.loaded = await act(backend.createDemo());
      if (!running) {
        // während des Ladens beendet: die Demo gleich wieder zurücknehmen
        if (demo.loaded && isDemoUndo()) await act(backend.undo());
        return;
      }
    }
    findDemo(demo);
    await go(0);
  };

  function onKey(e: KeyboardEvent): void {
    if (document.querySelector(".modal-back")) return; // ein Dialog hat Vorrang
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      stop();
    } else if (e.key === "ArrowRight" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      void go(index + 1);
    } else if (e.key === "ArrowLeft" || e.key === "Backspace") {
      e.preventDefault();
      void go(index - 1);
    }
  }

  function rectOf(sel: string | undefined, stackOf = false): DOMRect | null {
    if (!sel) return null;
    let el = document.querySelector(sel);
    if (stackOf) el = el?.closest(".dk-stack") ?? el;
    if (!el) return null;
    let r = el.getBoundingClientRect();
    // nur der Teil, der im eigenen Fenster zu sehen ist (weggescrollt = nicht da)
    const clip = stackOf ? null : el.closest(".tab-body")?.getBoundingClientRect();
    if (clip) {
      const top = Math.max(r.top, clip.top);
      const bottom = Math.min(r.bottom, clip.bottom);
      const left = Math.max(r.left, clip.left);
      const right = Math.min(r.right, clip.right);
      r = new DOMRect(left, top, right - left, bottom - top);
    }
    return r.width > 4 && r.height > 4 && r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth ? r : null;
  }

  function layout(): void {
    const s = STEPS[index]!;
    const W = innerWidth;
    const H = innerHeight;
    const pad = 6;
    const target = rectOf(s.target, !!s.panel);
    root.replaceChildren();
    // Abdunkeln: mit Ziel ein ausgeschnittenes Loch (Schatten), sonst alles
    if (target) {
      root.append(h("div", {
        class: "tour-hole",
        style: `left:${target.left - pad}px;top:${target.top - pad}px;width:${target.width + 2 * pad}px;height:${target.height + 2 * pad}px`,
      }));
    } else {
      root.append(h("div", { class: "tour-shade" }));
    }

    const placed: DOMRect[] = [];
    const overlap = (a: DOMRect) => placed.reduce((sum, o) => sum + Math.max(0, Math.min(a.right, o.right) - Math.max(a.left, o.left)) * Math.max(0, Math.min(a.bottom, o.bottom) - Math.max(a.top, o.top)), 0);
    const clampBox = (x: number, y: number, w: number, hh: number) => new DOMRect(Math.min(Math.max(12, x), W - w - 12), Math.min(Math.max(12, y), H - hh - 12), w, hh);
    /** Erste Lage ohne Überdeckung – sonst die mit der kleinsten. */
    const best = (tries: DOMRect[]) => tries.reduce((m, t) => (overlap(t) < overlap(m) ? t : m), tries.find((t) => overlap(t) === 0) ?? tries[0]!);

    // Post-its an einzelnen Funktionen (zuerst, damit der große Zettel ihnen ausweicht)
    let tilt = 0;
    for (const n of s.notes ?? []) {
      const r = rectOf(n.sel);
      if (!r) continue;
      root.append(h("div", { class: "tour-mark", style: `left:${r.left - 3}px;top:${r.top - 3}px;width:${r.width + 6}px;height:${r.height + 6}px` }));
      const note = h("div", { class: "tour-note" }, n.text);
      note.style.setProperty("--tilt", `${[-2, 1.5, -1, 2.2][tilt++ % 4]}deg`);
      root.append(note);
      const nw = note.offsetWidth;
      const nh = note.offsetHeight;
      const b = best([
        clampBox(r.left, r.bottom + 10, nw, nh),
        clampBox(r.left + r.width / 2 - nw / 2, r.bottom + 10, nw, nh),
        clampBox(r.right - nw, r.bottom + 10, nw, nh),
        clampBox(r.left, r.top - nh - 10, nw, nh),
        clampBox(r.right + 10, r.top, nw, nh),
        clampBox(r.left - nw - 10, r.top, nw, nh),
      ]);
      note.style.left = `${b.left}px`;
      note.style.top = `${b.top}px`;
      placed.push(b);
    }

    // Hauptzettel
    const back = h("button", { class: "tour-btn ghost", onclick: () => void go(index - 1) }, `← ${t("Back")}`);
    back.disabled = index === 0;
    const last = index === STEPS.length - 1;
    const next = h("button", { class: "tour-btn primary", onclick: () => void go(index + 1) }, last ? t("Done") : `${t("Next")} →`);
    const keep = last && demo.loaded ? h("button", { class: "tour-btn ghost keep", title: t("The demo tracks stay in the project"), onclick: () => void stop(true) }, t("Keep demo")) : null;
    const card = h("div", { class: "tour-card" },
      h("div", { class: "tour-count" }, `${index + 1} / ${STEPS.length}`),
      h("div", { class: "tour-title" }, s.title),
      h("div", { class: "tour-text" }, s.text),
      h("div", { class: "tour-actions" },
        h("button", { class: "tour-btn ghost end", onclick: () => void stop() }, t("End tour")),
        back,
        keep,
        next));
    root.append(card);
    const cw = card.offsetWidth;
    const ch = card.offsetHeight;
    const center = clampBox((W - cw) / 2, (H - ch) / 2, cw, ch);
    let box = center;
    if (target) {
      const gap = 18;
      const tries: DOMRect[] = [];
      // daneben, wenn Platz ist; sonst darunter/darüber (mittig, notfalls tiefer)
      if (target.right + gap + cw <= W - 12) tries.push(clampBox(target.right + gap, target.top + 10, cw, ch));
      if (target.left - gap - cw >= 12) tries.push(clampBox(target.left - gap - cw, target.top + 10, cw, ch));
      for (const dy of [0, 120, 240]) {
        if (target.bottom + gap + dy + ch <= H - 12) tries.push(clampBox(target.left + target.width / 2 - cw / 2, target.bottom + gap + dy, cw, ch));
      }
      if (target.top - gap - ch >= 12) tries.push(clampBox(target.left + target.width / 2 - cw / 2, target.top - gap - ch, cw, ch));
      // großes Ziel (z. B. die Spuren): in das Ziel, unten rechts
      tries.push(clampBox(target.right - cw - 24, target.bottom - ch - 24, cw, ch), center);
      box = best(tries);
    }
    card.style.left = `${box.left}px`;
    card.style.top = `${box.top}px`;
    next.focus();
  }

  window.addEventListener("keydown", onKey, true);
  window.addEventListener("resize", layout);
  void begin();
}
