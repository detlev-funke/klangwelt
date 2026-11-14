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
  for (const t of tracks) {
    for (const c of t.clips) {
      if (!d.melody && t.kind === "instrument" && t.instrument?.kind !== "drums" && c.source.type === "notes" && c.source.notes.length >= 4) {
        d.melody = { trackId: t.id, clipId: c.id };
      }
      if (c.source.type === "audio" && (!d.audio || /arpeggio/i.test(t.name))) d.audio = { trackId: t.id, clipId: c.id, assetId: c.source.assetId };
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
    title: "Willkommen in Klangwelt",
    text: "Ein kurzer Rundgang durch alle Fenster – jedes Mal ist eins hervorgehoben, Zettel erklären die wichtigsten Funktionen. Damit es etwas zu sehen gibt, ist die Demo geladen. Weiter mit → oder Enter, zurück mit ←, Esc beendet die Tour jederzeit.",
  },
  {
    target: "kw-transport",
    title: "Die Leiste oben",
    text: "Projekt und Wiedergabe: Speichern, Exportieren, Abspielen, Aufnehmen, Tempo und Takt, Metronom und Wiederholen, Rückgängig. Rechts Fenster, Einstellungen, Pegel und die Verbindung zum Agenten.",
    notes: [
      { sel: ".file-controls", text: "Datei, Speichern und Exportieren – als WAV, FLAC, Opus, Stems oder MIDI (Strg+E)." },
      { sel: "kw-transport .btn.big.primary", text: "Leertaste: abspielen. Ist ein Zeitraum markiert, genau den." },
      { sel: "[data-help=tempo]", text: "Tempo in BPM – alles folgt sofort." },
      { sel: "[data-help=fenster]", text: "Fenster ein- und ausblenden, Layout zurücksetzen." },
    ],
  },
  {
    panel: "spuren",
    target: panelArea("spuren"),
    prepare: () => select(null),
    title: "Spuren",
    text: "Das Hauptfenster: links die Spurköpfe (Name, Stumm/Solo, Pegel, Ansicht), rechts die Zeitleiste mit den Clips. Die Demo hat zwei Aufnahmen (Beat, Arpeggio) und eine Melodie als Noten – Alt+Klick setzt eine Note.",
    notes: [
      { sel: "[data-help=neue-instrumentspur]", text: "Neue Spur: Zupfer, Orgel, Cembalo, Bass, Synth oder Schlagzeug." },
      { sel: ".track-row .toggle.m", text: "Stumm und Solo – gilt auch für den Export." },
      { sel: ".track-row.notes canvas.keys", text: "Klaviatur: Klick spielt einen Ton, Mausrad verschiebt." },
      { sel: ".empty-hint .row", text: "Schnellstart: Demo, ganzer Song, Beat oder Aufnahme." },
    ],
  },
  {
    panel: "clip",
    target: panelArea("clip"),
    prepare: (d) => select(d.melody),
    title: "Clip",
    text: "Eigenschaften des gewählten Clips – hier das „Thema“ der Melodie mit seinen Vortragszeichen: pp … ff, Crescendo, Staccato, Vibrato. Bei Aufnahmen: Tempo erkennen, analysieren, die Melodie als Noten übernehmen.",
    notes: [{ sel: "kw-panel[tab=clip] .mark.dyn", text: "Dynamik für die gewählten Noten – ohne Auswahl für alle." }],
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
    title: "Analyse",
    text: "Was in einer Aufnahme steckt – hier im Arpeggio der Demo: Tonhöhenkurve, erkannte Noten, Tempo und Tonart. Doppelklick auf einen Audio-Clip öffnet sie.",
  },
  {
    panel: "klang",
    target: panelArea("klang"),
    prepare: (d) => select(d.melody),
    title: "Klang – die Soundwerkstatt",
    text: "Der Klang der Melodie (ein Zupfer) in drei Feldern: Charakter, Klangfarbe und Raum. Punkt ziehen oder einen benannten Startpunkt anklicken; im Feinschliff stehen alle Regler. Unter „Meine Klänge“ sicherst du Einstellungen für alle Projekte.",
    notes: [
      { sel: "kw-panel[tab=klang] .snd-kinds", text: "Instrument wechseln – die Noten bleiben." },
      { sel: "kw-panel[tab=klang] .snd-field[data-field=charakter] .snd-mark", text: "Startpunkte: ein Klick setzt das ganze Feld." },
    ],
  },
  {
    panel: "rhythmus",
    target: panelArea("rhythmus"),
    prepare: () => select(null),
    title: "Rhythmus",
    text: "Beats zum Schnelleinstieg: Muster aus 14 Genres und ein Schritt-Raster – eine Zeile je Trommel, ein Klick setzt einen Schlag. „In Spur schreiben“ legt den Beat an.",
    notes: [
      { sel: ".rh-genre", text: "Genres von House bis Dubstep – jedes mit Tempo und Tipp." },
      { sel: ".rh-step", text: "Klick setzt oder löscht einen Schlag." },
    ],
  },
  {
    panel: "song",
    target: panelArea("song"),
    title: "Song",
    text: "Ganze Stücke aus Vorlagen. Spalten sind Abschnitte (Intro, Aufbau, Höhepunkt …), Zeilen Rollen (Fläche, Bass, Kick …), jede Zelle aus, sparsam oder voll. „Song erzeugen“ schreibt alle Spuren.",
    notes: [
      { sel: ".song-tpl", text: "Vorlagen: Trance, Ambient, Deep House, Synthwave, Lo-Fi." },
      { sel: ".song-cell", text: "Klick: mehr · Umschalt+Klick: weniger." },
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
    title: "Bibliothek",
    text: "Partituren zum Suchen, Anhören und Übernehmen. Unter „Herunterladen“ stehen die Pakete: Bach-Choräle, OpenScore, KernScores, PDMX mit über 250 000 Stücken – jedes mit Umfang und Lizenz. Die Melodiesuche findet Stücke zu Noten oder einer gesungenen Aufnahme.",
    notes: [
      { sel: "kw-panel[tab=bibliothek] [data-help=lib-herunterladen]", text: "Katalog der Musikbibliotheken auf- und zuklappen." },
      { sel: "kw-panel[tab=bibliothek] .lib-cards", text: "Ein Klick auf ein Paket lädt es herunter – im Hintergrund." },
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
    title: "Fuge",
    text: "Kontrapunkt-Werkzeuge, hier mit dem „Thema“ der Demo: Antwort, Umkehrung, Krebs, Engführungen, Gegensätze und ein ganzer Fugenplan. Der Prüfer markiert Parallelen und Dissonanzen in der Zeitleiste.",
    notes: [{ sel: "kw-panel[tab=fuge] .fg-subject", text: "Das Thema: ein Noten-Clip mit 2–4 Takten." }],
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
    title: "Harmonie",
    text: "Der Quintenzirkel folgt der Abspielposition – hier Takt 2 der Demo. Rechts die Wege zum Schluss: mehrschrittige Kadenzen vom aktuellen Akkord aus; die Zahl unter jedem Pfeil ist die Übergangswahrscheinlichkeit – wie oft genau dieser Schritt in den Stücken der Bibliothek folgt. Darunter die Fortsetzungen nach Rang, ebenfalls mit Wahrscheinlichkeit. Klick spielt einen Übergang, Doppelklick übernimmt ihn.",
    notes: [
      { sel: "kw-panel[tab=harmonie] .hc-paths", text: "Wege zum Schluss – unter den Pfeilen: Übergangswahrscheinlichkeit aus der Bibliothek." },
      { sel: "kw-panel[tab=harmonie] .hc-pick", text: "Fortsetzung mit Rang – der Balken zeigt, wie oft sie in der Bibliothek folgt." },
    ],
  },
  {
    panel: "agent",
    target: panelArea("agent"),
    title: "Agent",
    text: "Aufträge an einen KI-Agenten wie Claude – „Mach die Kick tiefer“, „Schreib eine Basslinie für die markierten Takte“. Er sieht, welche Spuren hörbar sind und welcher Zeitraum markiert ist.",
    notes: [{ sel: ".chat-input", text: "Auftrag eintippen, Enter sendet." }],
  },
  {
    panel: "hilfe",
    target: panelArea("hilfe"),
    after: () => {
      const play = document.querySelector("kw-transport .btn.big.primary");
      if (play) helpShow(play);
    },
    title: "Hilfe",
    text: "Dieses kleine Fenster erklärt, worauf die Maus gerade zeigt – hier den Abspielknopf. F1 hält den Text fest und holt das Fenster zurück, wenn du es geschlossen hast.",
  },
  {
    title: "Viel Spaß!",
    text: "Fenster lassen sich an ihren Reitern verschieben, andocken und schweben lassen; „Fenster ▾“ setzt das Layout zurück. Mit „Fertig“ verschwindet die Demo wieder – oder du behältst sie zum Ausprobieren.",
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
  const root = h("div", { class: "tour", role: "dialog", "aria-label": "Intro-Tour" });
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
        if (snapshot.peek()?.undoLabel === "Demo laden") await act(backend.undo());
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

  /** Demo laden (nur im leeren Projekt), dann der erste Schritt. */
  const begin = async () => {
    if (!snapshot.peek()?.project.tracks.length) {
      root.append(h("div", { class: "tour-shade" }));
      demo.loaded = await act(backend.createDemo());
      if (!running) {
        // während des Ladens beendet: die Demo gleich wieder zurücknehmen
        if (demo.loaded && snapshot.peek()?.undoLabel === "Demo laden") await act(backend.undo());
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
    const back = h("button", { class: "tour-btn ghost", onclick: () => void go(index - 1) }, "← Zurück");
    back.disabled = index === 0;
    const last = index === STEPS.length - 1;
    const next = h("button", { class: "tour-btn primary", onclick: () => void go(index + 1) }, last ? "Fertig" : "Weiter →");
    const keep = last && demo.loaded ? h("button", { class: "tour-btn ghost keep", title: "Die Demo-Spuren bleiben im Projekt", onclick: () => void stop(true) }, "Demo behalten") : null;
    const card = h("div", { class: "tour-card" },
      h("div", { class: "tour-count" }, `${index + 1} / ${STEPS.length}`),
      h("div", { class: "tour-title" }, s.title),
      h("div", { class: "tour-text" }, s.text),
      h("div", { class: "tour-actions" },
        h("button", { class: "tour-btn ghost end", onclick: () => void stop() }, "Beenden"),
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
