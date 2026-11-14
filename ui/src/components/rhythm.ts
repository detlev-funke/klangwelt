// Rhythmus-Fenster: Schlagzeug-Muster typischer Genres als Schnelleinstieg und
// ein Schritt-Raster (16 Sechzehntel je Takt) als Eingabehilfe.
//
// Ist ein Schlagzeug-Clip ausgewählt, zeigt und bearbeitet das Raster diesen
// Clip – jeder Klick ein Rückgängig-Schritt, ein Muster ersetzt den Inhalt.
// Sonst ist das Raster ein Entwurf, den „In Spur schreiben“ als Clip anlegt
// (ohne Schlagzeug-Spur in einer neuen, mit dem Klang des Genres).

import { backend } from "../bridge.js";
import { h, icon, setRange } from "../dom.js";
import { plural, t } from "../i18n.js";
import { act, errorText, notify, project, run, selection, showPanel, snapshot, status } from "../store.js";
import type { Clip, RhythmGenre, RhythmGrid, RhythmInfo, RhythmPattern, RhythmRow, Track } from "../types.js";

const STEPS = 16;
const PREF_KEY = "klangwelt.rhythmus";

type Prefs = { more: boolean; genre: number; bars: number };
function prefs(): Prefs {
  const d: Prefs = { more: false, genre: 0, bars: 1 };
  try {
    return { ...d, ...(JSON.parse(localStorage.getItem(PREF_KEY) ?? "{}") as Partial<Prefs>) };
  } catch {
    return d;
  }
}
function setPref<K extends keyof Prefs>(k: K, v: Prefs[K]): void {
  try {
    localStorage.setItem(PREF_KEY, JSON.stringify({ ...prefs(), [k]: v }));
  } catch {
    /* ohne Speicher */
  }
}

let infoPromise: Promise<RhythmInfo> | null = null;
function loadInfo(): Promise<RhythmInfo> {
  infoPromise ??= backend.rhythmInfo().catch((e) => {
    infoPromise = null;
    throw e;
  });
  return infoPromise;
}

/** Wiederholt bzw. kürzt ein Raster auf `steps`. */
export function resizeGrid(g: RhythmGrid, steps: number): RhythmGrid {
  const rows: Record<string, number[]> = {};
  for (const [k, v] of Object.entries(g.rows)) {
    const out = Array.from({ length: steps }, (_, s) => s).filter((s) => v.includes(s % g.steps));
    if (out.length) rows[k] = out;
  }
  return { steps, rows };
}

function sameGrid(a: RhythmGrid, b: RhythmGrid): boolean {
  if (a.steps !== b.steps) return false;
  const keys = new Set([...Object.keys(a.rows), ...Object.keys(b.rows)]);
  for (const k of keys) {
    const x = a.rows[k] ?? [];
    const y = b.rows[k] ?? [];
    if (x.length !== y.length || x.some((v, i) => v !== y[i])) return false;
  }
  return true;
}

const isDrums = (t: Track | undefined): t is Track => t?.kind === "instrument" && t.instrument?.kind === "drums";

/**
 * Schnelleinstieg „Mit Beat starten“: legt eine Schlagzeug-Spur mit vier Takten
 * der Vorlage an (Klang des Genres), übernimmt das Tempo – ein Rückgängig-Schritt –
 * und öffnet den Clip im Fenster „Rhythmus“.
 */
export async function startWithBeat(g: RhythmGenre, p: RhythmPattern, bars = 4): Promise<void> {
  try {
    const w = await backend.rhythmWrite(resizeGrid(p.grid, bars * STEPS), p.swing, null, null, 0, p.name, g.sound, p.bpm);
    snapshot.value = w.snapshot;
    selection.value = { trackId: w.trackId, clipId: w.clipId };
    showPanel("rhythmus");
    notify(t("\"{name}\" · {bpm} BPM – Space plays it, edit the beat in the \"Rhythm\" window", { name: p.name, bpm: p.bpm }));
  } catch (e) {
    notify(errorText(e), "error");
  }
}

/** Inhalt des Menüs „Mit Beat starten“: je Genre eine Zeile mit seinen Vorlagen. */
export function fillBeatMenu(menu: HTMLElement, close: () => void): void {
  // fest über allem, damit kein Fensterrand das Menü abschneidet: unter dem Knopf, sonst darüber
  const btn = menu.parentElement?.querySelector("button");
  if (btn) {
    const r = btn.getBoundingClientRect();
    const w = Math.min(640, window.innerWidth - 24);
    const below = window.innerHeight - r.bottom - 18;
    const above = r.top - 18;
    Object.assign(menu.style, {
      width: `${w}px`,
      left: `${Math.max(12, Math.min(window.innerWidth - w - 12, r.left + r.width / 2 - w / 2))}px`,
      ...(below >= 360 || below >= above
        ? { top: `${r.bottom + 6}px`, bottom: "auto", maxHeight: `${below}px` }
        : { top: "auto", bottom: `${window.innerHeight - r.top + 6}px`, maxHeight: `${above}px` }),
    });
  }
  menu.replaceChildren(h("div", { class: "menu-title" }, t("Beat templates – new drum track, template tempo")));
  loadInfo()
    .then((info) => {
      menu.append(
        ...info.genres.map((g) => {
          const row = h(
            "div",
            { class: "beat-row", title: g.blurb },
            h("span", { class: "beat-genre" }, g.name),
            h("div", { class: "beat-chips" },
              ...g.patterns.map((p) => {
                const b = h("button", { class: "beat-chip", title: `${p.tip} (${p.bpm} BPM)` }, p.name, h("small", {}, String(p.bpm)));
                b.dataset.name = p.name;
                b.addEventListener("click", () => {
                  close();
                  void startWithBeat(g, p);
                });
                return b;
              })),
          );
          row.dataset.genre = g.name;
          return row;
        }),
      );
    })
    .catch((e) => menu.append(h("div", { class: "muted" }, errorText(e))));
}

/** Entwurf – bleibt erhalten, wenn das Fenster neu aufgebaut wird. */
const draft: { grid: RhythmGrid | null; swing: number; pattern: string | null; sound: string | null; bpm: number | null } = {
  grid: null,
  swing: 0,
  pattern: null,
  sound: null,
  bpm: null,
};

export function rhythmPanel(): { el: HTMLElement; update(): void } {
  const el = h("div", { class: "rh" }, h("div", { class: "placeholder" }, t("Loading patterns…")));
  let info: RhythmInfo | null = null;
  const pf = prefs();
  let genreIx = Math.min(pf.genre, 99);
  let more = pf.more;
  let page = 0;
  let targetChoice: string | null = null;
  let tipPattern: RhythmPattern | null = null;

  // gebunden an einen Schlagzeug-Clip?
  let bound: { trackId: number; clipId: number; track: Track; clip: Clip } | null = null;
  let boundKey = "";
  let clipGrid: RhythmGrid | null = null;
  let clipSwing = 0;
  let clipSkipped = 0;
  let clipName = "";
  let swingDragging = false;
  let swingMerge = false;

  // Vorhören mit mitlaufender Spalte
  let playing: { t0: number; secs: number; stepSecs: number; steps: number } | null = null;
  let raf = 0;

  // ------------------------------------------------------------ Elemente
  const genreBar = h("div", { class: "rh-genres" });
  const patternBar = h("div", { class: "rh-patterns" });
  const about = h("div", { class: "rh-about" });
  const mode = h("div", { class: "rh-mode" });
  const barsSeg = h("div", { class: "seg mini rh-bars" });
  const swingIn = h("input", { type: "range", min: 0, max: 75, step: 1, class: "rh-swing", title: t("Swing: every second sixteenth comes later") }) as HTMLInputElement;
  const swingVal = h("b", { class: "rh-swing-val" });
  const tempoInfo = h("span", { class: "muted rh-tempo" });
  const tempoBtn = h("button", { class: "btn small rh-tempo-btn" }) as HTMLButtonElement;
  const targetSel = h("select", { class: "hc-select rh-target", title: t("Where \"Write to track\" puts the beat") }) as HTMLSelectElement;
  const playBtn = h("button", { class: "btn small primary rh-play", title: t("Listen to the draft in a loop – click again to stop") }) as HTMLButtonElement;
  const writeBtn = h("button", { class: "btn small blue rh-write", title: t("Create the draft as a clip – the grid then edits that clip") }, icon("plus"), t("Write to track")) as HTMLButtonElement;
  const clearBtn = h("button", { class: "btn small ghost rh-clear", title: t("Clear all steps") }, icon("trash"), t("Clear")) as HTMLButtonElement;
  const pages = h("div", { class: "seg mini rh-pages" });
  const grid = h("div", { class: "rh-grid" });
  const moreBtn = h("button", { class: "btn ghost small rh-more", title: t("Show or hide more drums (toms, cymbals, percussion)") }) as HTMLButtonElement;

  const bars = () => Math.max(1, Math.round(current().steps / STEPS));
  const current = (): RhythmGrid => (bound ? clipGrid : draft.grid) ?? { steps: STEPS, rows: {} };
  const swing = () => (bound ? clipSwing : draft.swing);
  const genre = (): RhythmGenre | null => info?.genres[genreIx] ?? null;
  const tempo = () => project()?.tempoBpm ?? 120;

  /** Muster, dem das Raster gerade entspricht (in irgendeinem Genre). */
  const matching = (): { g: number; p: RhythmPattern } | null => {
    if (!info) return null;
    const cur = current();
    const order = [genreIx, ...info.genres.map((_, i) => i).filter((i) => i !== genreIx)];
    for (const gi of order) {
      const p = info.genres[gi]!.patterns.find((p) => sameGrid(resizeGrid(p.grid, cur.steps), cur));
      if (p) return { g: gi, p };
    }
    return null;
  };

  // ------------------------------------------------------------ Aktionen
  const loadPattern = (g: RhythmGenre, p: RhythmPattern) => {
    tipPattern = p;
    const n = bars() * STEPS;
    if (bound) {
      const b = bound;
      void act(backend.rhythmWrite(resizeGrid(p.grid, n), p.swing, b.trackId, b.clipId, 0, p.name, null, null).then((w) => w.snapshot)).then((ok) => {
        if (ok) notify(t("Clip \"{clip}\" is now \"{pattern}\" – undo with Ctrl+Z", { clip: b.clip.name, pattern: p.name }));
      });
    } else {
      draft.grid = resizeGrid(p.grid, n);
      draft.swing = p.swing;
      draft.pattern = p.name;
      draft.sound = g.sound;
      draft.bpm = p.bpm;
      render();
    }
  };

  const toggle = (row: string, step: number) => {
    const g = current();
    const list = g.rows[row] ?? [];
    const on = !list.includes(step);
    const next: RhythmGrid = { steps: g.steps, rows: { ...g.rows, [row]: on ? [...list, step].sort((a, b) => a - b) : list.filter((s) => s !== step) } };
    if (!next.rows[row]!.length) delete next.rows[row];
    if (bound) {
      clipGrid = next; // gleich zeigen, der Kern bestätigt
      render();
      void act(backend.rhythmToggle(bound.trackId, bound.clipId, row, step, on, clipSwing));
    } else {
      draft.grid = next;
      render();
    }
  };

  const setBars = (n: number) => {
    setPref("bars", n);
    const g = resizeGrid(current(), n * STEPS);
    page = Math.min(page, n - 1);
    if (bound) {
      void act(backend.rhythmWrite(g, clipSwing, bound.trackId, bound.clipId, 0, bound.clip.name, null, null).then((w) => w.snapshot));
    } else {
      draft.grid = g;
      render();
    }
  };

  /** Spuren, in die geschrieben werden kann. */
  const drumTracks = () => (project()?.tracks ?? []).filter(isDrums);
  const targetTrack = (): Track | null => {
    const v = targetSel.value;
    return v && v !== "neu" ? drumTracks().find((t) => String(t.id) === v) ?? null : null;
  };

  const write = async () => {
    const p = project();
    if (!p) return;
    const target = targetTrack();
    const bpb = p.beatsPerBar || 4;
    let start: number;
    if (target) {
      const end = Math.max(0, ...target.clips.map((c) => c.startBeats + (c.source.type === "notes" ? c.source.lengthBeats : 0)));
      start = Math.ceil(end / bpb - 1e-9) * bpb;
    } else {
      start = Math.floor((status.peek().positionSecs * p.tempoBpm) / 60 / bpb) * bpb;
    }
    const name = matching()?.p.name ?? draft.pattern ?? t("Beat");
    try {
      const w = await backend.rhythmWrite(current(), draft.swing, target?.id ?? null, null, start, name, target ? null : draft.sound, null);
      snapshot.value = w.snapshot;
      selection.value = { trackId: w.trackId, clipId: w.clipId };
      notify(t("Wrote \"{name}\" – the grid now edits this clip", { name }));
    } catch (e) {
      notify(errorText(e), "error");
    }
  };

  const stopPlay = () => {
    playing = null;
    cancelAnimationFrame(raf);
    renderPlayhead();
    playBtn.replaceChildren(icon("play"), t("Listen"));
  };
  const play = async () => {
    if (playing) {
      void backend.previewStop();
      stopPlay();
      return;
    }
    const g = current();
    try {
      const secs = bound
        ? await backend.rhythmPreview(bound.trackId, bound.clipId, null, clipSwing, null)
        : await backend.rhythmPreview(targetTrack()?.id ?? null, null, g, draft.swing, draft.sound);
      playing = { t0: performance.now(), secs, stepSecs: 60 / tempo() / 4, steps: g.steps };
      playBtn.replaceChildren(icon("stop"), t("Stop"));
      const tick = () => {
        if (!playing) return;
        if ((performance.now() - playing.t0) / 1000 >= playing.secs) return stopPlay();
        renderPlayhead();
        raf = requestAnimationFrame(tick);
      };
      tick();
    } catch (e) {
      notify(errorText(e), "error");
    }
  };

  // ------------------------------------------------------------ Darstellung
  let gridKey = "";
  let cells: Array<{ row: string; step: number; el: HTMLElement }> = [];

  const visibleRows = (): RhythmRow[] => {
    const g = current();
    return (info?.rows ?? []).filter((r) => r.main || more || (g.rows[r.key]?.length ?? 0) > 0);
  };

  const renderGrid = () => {
    const g = current();
    const rows = visibleRows();
    const nBars = Math.max(1, Math.round(g.steps / STEPS));
    page = Math.min(page, nBars - 1);
    const key = `${rows.map((r) => r.key).join(",")}|${page}|${g.steps}`;
    if (key !== gridKey) {
      gridKey = key;
      cells = [];
      grid.replaceChildren(
        ...rows.map((r) => {
          const label = h("button", { class: "rh-label", title: t("Play {name}", { name: r.label }) }, r.label);
          label.dataset.row = r.key;
          label.addEventListener("click", () => {
            const tid = bound?.trackId ?? targetTrack()?.id;
            if (tid != null) void backend.previewNote(tid, r.midi).catch(() => {});
          });
          const steps = Array.from({ length: STEPS }, (_, i) => {
            const step = page * STEPS + i;
            const c = h("button", { class: `rh-step${i % 4 === 0 ? " beat" : ""}`, title: t("{row} · step {step}", { row: r.label, step: i + 1 }) });
            c.dataset.row = r.key;
            c.dataset.step = String(step);
            c.addEventListener("click", () => toggle(r.key, step));
            cells.push({ row: r.key, step, el: c });
            return c;
          });
          const line = h("div", { class: "rh-row" }, label, ...steps);
          line.dataset.row = r.key;
          return line;
        }),
      );
      pages.replaceChildren(
        ...Array.from({ length: nBars }, (_, b) => {
          const btn = h("button", { class: "seg-btn", title: t("Show bar {bar}", { bar: b + 1 }) }, t("Bar {bar}", { bar: b + 1 }));
          btn.dataset.page = String(b);
          btn.addEventListener("click", () => {
            page = b;
            render();
          });
          return btn;
        }),
      );
    }
    pages.hidden = nBars < 2;
    for (const b of pages.children) (b as HTMLElement).classList.toggle("on", Number((b as HTMLElement).dataset.page) === page);
    for (const c of cells) c.el.classList.toggle("on", (g.rows[c.row] ?? []).includes(c.step));
    const hidden = (info?.rows ?? []).filter((r) => !r.main && !(g.rows[r.key]?.length)).length;
    moreBtn.textContent = more ? t("fewer drums") : t("more drums ({n})", { n: hidden });
    moreBtn.hidden = !more && hidden === 0;
    renderPlayhead();
  };

  const renderPlayhead = () => {
    let col = -1;
    if (playing) {
      const s = Math.floor((performance.now() - playing.t0) / 1000 / playing.stepSecs) % playing.steps;
      const want = Math.floor(s / STEPS);
      if (want !== page) {
        page = want;
        renderGrid();
        return;
      }
      col = s;
    }
    for (const c of cells) c.el.classList.toggle("now", c.step === col);
  };

  const renderGenres = () => {
    if (!info) return;
    genreBar.replaceChildren(
      ...info.genres.map((g, i) => {
        const b = h("button", { class: `rh-genre${i === genreIx ? " on" : ""}`, title: g.blurb }, g.name);
        b.addEventListener("click", () => {
          genreIx = i;
          setPref("genre", i);
          tipPattern = null;
          renderGenres();
          render();
        });
        return b;
      }),
    );
    const g = genre();
    patternBar.replaceChildren(
      ...(g?.patterns ?? []).map((p) => {
        const b = h("button", { class: "rh-pattern", title: p.tip }, h("b", {}, p.name), h("span", {}, `${p.bpm} BPM${p.swing ? ` · ${t("Swing {swing}%", { swing: p.swing })}` : ""}`));
        b.dataset.name = p.name;
        b.addEventListener("click", () => loadPattern(g!, p));
        b.addEventListener("mouseenter", () => {
          tipPattern = p;
          renderAbout();
        });
        return b;
      }),
    );
  };

  const renderAbout = () => {
    const g = genre();
    const m = matching();
    const p = tipPattern ?? (m && m.g === genreIx ? m.p : null);
    about.replaceChildren(
      h("span", {}, g?.blurb ?? ""),
      ...(p ? [h("span", { class: "rh-tip" }, h("b", {}, `${p.name}: `), p.tip)] : []),
    );
  };

  const render = () => {
    if (!info) return;
    const g = current();
    const m = matching();
    for (const b of patternBar.children) {
      const el = b as HTMLElement;
      el.classList.toggle("on", !!m && m.g === genreIx && m.p.name === el.dataset.name);
    }
    renderAbout();
    // Modus
    if (bound) {
      mode.replaceChildren(
        h("span", { class: "rh-badge" }, t("Clip")),
        h("span", {}, t("editing \"{clip}\" in \"{track}\" – every click goes straight into the clip, patterns replace its content", { clip: clipName || bound.clip.name, track: bound.track.name })),
        ...(clipSkipped
          ? [h("span", { class: "muted" }, ` · ${plural(clipSkipped, "{n} note outside the grid stays unchanged", "{n} notes outside the grid stay unchanged")}`)]
          : []),
      );
    } else {
      mode.replaceChildren(
        h("span", { class: "rh-badge draft" }, t("Draft")),
        h("span", {}, t("choose a pattern or tap in steps – \"Write to track\" creates a clip (or select a drum clip to edit it)")),
      );
    }
    // Takte
    const n = bars();
    for (const b of barsSeg.children) (b as HTMLElement).classList.toggle("on", Number((b as HTMLElement).dataset.bars) === n);
    // Swing
    if (!swingDragging) setRange(swingIn, swing());
    swingVal.textContent = `${Math.round(swing())} %`;
    // Tempo
    const bpm = m?.p.bpm ?? (bound ? null : draft.bpm);
    tempoInfo.textContent = t("Project {bpm} BPM", { bpm: Math.round(tempo()) });
    tempoBtn.hidden = !bpm || Math.round(bpm) === Math.round(tempo());
    if (bpm) {
      tempoBtn.replaceChildren(t("Use tempo {bpm}", { bpm }));
      tempoBtn.dataset.bpm = String(bpm);
      tempoBtn.title = t("Set the project tempo to {bpm} BPM – typical for this pattern", { bpm });
    }
    // Ziel (nur im Entwurf)
    const tracks = drumTracks();
    const opts = [...tracks.map((tr) => [String(tr.id), t("New clip in \"{track}\"", { track: tr.name })] as const), ["neu", t("New drum track")] as const];
    const sig = opts.map((o) => o.join(":")).join("|");
    if (targetSel.dataset.sig !== sig) {
      targetSel.dataset.sig = sig;
      targetSel.replaceChildren(...opts.map(([v, l]) => h("option", { value: v }, l)));
    }
    const sel = selection.peek();
    const want = targetChoice && opts.some(([v]) => v === targetChoice) ? targetChoice
      : sel && tracks.some((t) => t.id === sel.trackId) ? String(sel.trackId)
      : opts[0]![0];
    if (targetSel.value !== want) targetSel.value = want;
    targetSel.hidden = !!bound;
    writeBtn.hidden = !!bound;
    writeBtn.disabled = !Object.keys(g.rows).length;
    renderGrid();
  };

  // ------------------------------------------------------------ Aufbau nach dem Laden
  const build = () => {
    [1, 2, 4].forEach((n) => {
      const b = h(
        "button",
        { class: "seg-btn", title: plural(n, "{n} bar ({steps} steps)", "{n} bars ({steps} steps)", { steps: n * STEPS }) },
        plural(n, "{n} bar", "{n} bars"),
      );
      b.dataset.bars = String(n);
      b.addEventListener("click", () => setBars(n));
      barsSeg.append(b);
    });
    swingIn.addEventListener("pointerdown", () => {
      swingDragging = true;
      swingMerge = false;
    });
    swingIn.addEventListener("input", () => {
      const v = Number(swingIn.value);
      swingVal.textContent = `${v} %`;
      if (bound) {
        clipSwing = v;
        void act(backend.rhythmSwing(bound.trackId, bound.clipId, v, swingMerge));
        swingMerge = true;
      } else {
        draft.swing = v;
      }
    });
    swingIn.addEventListener("change", () => {
      swingDragging = false;
      swingMerge = false;
    });
    swingIn.addEventListener("keydown", (e) => e.stopPropagation());
    tempoBtn.addEventListener("click", () => {
      const bpm = Number(tempoBtn.dataset.bpm);
      if (bpm) void run({ type: "setProject", patch: { tempoBpm: bpm } });
    });
    targetSel.addEventListener("change", () => (targetChoice = targetSel.value));
    playBtn.replaceChildren(icon("play"), t("Listen"));
    playBtn.addEventListener("click", () => void play());
    writeBtn.addEventListener("click", () => void write());
    clearBtn.addEventListener("click", () => {
      const empty = { steps: current().steps, rows: {} };
      if (bound) void act(backend.rhythmWrite(empty, clipSwing, bound.trackId, bound.clipId, 0, bound.clip.name, null, null).then((w) => w.snapshot));
      else {
        draft.grid = empty;
        draft.pattern = null;
        render();
      }
    });
    moreBtn.addEventListener("click", () => {
      more = !more;
      setPref("more", more);
      render();
    });
    if (!draft.grid) {
      const g0 = info!.genres[0]!;
      const p0 = g0.patterns[0]!;
      draft.grid = resizeGrid(p0.grid, Math.max(1, pf.bars) * STEPS);
      draft.pattern = p0.name;
      draft.sound = g0.sound;
      draft.bpm = p0.bpm;
    }
    el.replaceChildren(
      h("div", { class: "rh-top" }, genreBar, patternBar, about),
      mode,
      h("div", { class: "rh-ctl" },
        barsSeg,
        h("label", { class: "rh-swing-box" }, t("Swing"), swingIn, swingVal),
        tempoInfo, tempoBtn,
        h("span", { class: "rh-gap" }),
        targetSel, writeBtn, playBtn, clearBtn),
      pages,
      grid,
      h("div", { class: "rh-foot" }, moreBtn, h("span", { class: "hint" }, t("Click sets or clears a step · bright columns = beats 1 2 3 4 · click a drum to hear it"))),
    );
    renderGenres();
    update();
  };

  loadInfo().then((i) => {
    info = i;
    genreIx = Math.min(genreIx, i.genres.length - 1);
    build();
  }).catch(() => el.replaceChildren(h("div", { class: "placeholder" }, t("Rhythm is only available in the desktop app."))));

  // ------------------------------------------------------------ Auswahl verfolgen
  const update = () => {
    if (!info) return;
    const p = project();
    const sel = selection.peek();
    const t = p?.tracks.find((x) => x.id === sel?.trackId);
    const c = t?.clips.find((x) => x.id === sel?.clipId);
    const next = isDrums(t) && c && c.source.type === "notes" ? { trackId: t.id, clipId: c.id, track: t, clip: c } : null;
    if (!next) {
      if (bound) {
        bound = null;
        boundKey = "";
        clipGrid = null;
        page = 0;
      }
      render();
      return;
    }
    const src = c!.source as Extract<Clip["source"], { type: "notes" }>;
    const key = `${next.trackId}|${next.clipId}|${src.lengthBeats}|${JSON.stringify(src.notes)}`;
    const switched = !bound || bound.clipId !== next.clipId;
    bound = next;
    if (switched) page = 0;
    if (key === boundKey) {
      render();
      return;
    }
    boundKey = key;
    backend.rhythmRead(next.trackId, next.clipId).then((r) => {
      if (bound?.clipId !== next.clipId) return;
      clipGrid = r.grid;
      if (!swingDragging) clipSwing = r.swing;
      clipSkipped = r.skipped;
      clipName = r.name;
      render();
    }).catch((e) => notify(errorText(e), "error"));
  };

  return { el, update };
}
