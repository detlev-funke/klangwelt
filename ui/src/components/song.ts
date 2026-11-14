// Song-Fenster: Mini-Editor für den Aufbau eines Stücks. Spalten sind die
// Abschnitte (so breit wie ihre Takte), Zeilen die Rollen; jede Zelle hat eine
// Stufe – aus, sparsam, voll (Klick schaltet weiter, Umschalt+Klick zurück).
// „Song erzeugen“ schreibt daraus die Spuren und ersetzt dabei die zuletzt
// erzeugten. Vorlagen (Trance, Ambient …) sind Startpunkte.

import { backend } from "../bridge.js";
import { h, icon } from "../dom.js";
import { effect } from "../signals.js";
import { errorText, notify, project, showPanel, snapshot, status, viewport } from "../store.js";
import type { SongPlan, SongSection, SongSectionKind } from "../types.js";

const PLAN_KEY = "klangwelt.song.plan";
const BAR_PX = 9;

const KINDS: Array<[SongSectionKind, string, string]> = [
  ["intro", "Intro", "beginnt behutsam"],
  ["teil", "Teil", "gleichbleibend"],
  ["aufbau", "Aufbau", "wird lauter und heller, endet (mit Übergängen) im Snare-Wirbel"],
  ["break", "Break", "zurückgenommen"],
  ["hoehepunkt", "Höhepunkt", "voll, Becken zu Beginn"],
  ["outro", "Outro", "klingt aus"],
];
const KIND_MARK: Record<SongSectionKind, string> = { intro: "◔", teil: "", aufbau: "↗", break: "◦", hoehepunkt: "★", outro: "↘" };
const LEVEL_WORD = ["aus", "sparsam", "voll"];

let templatesPromise: Promise<SongPlan[]> | null = null;
export function loadTemplates(): Promise<SongPlan[]> {
  templatesPromise ??= backend.songTemplates().catch((e) => {
    templatesPromise = null;
    throw e;
  });
  return templatesPromise;
}

const clone = (p: SongPlan): SongPlan => JSON.parse(JSON.stringify(p)) as SongPlan;

/** Der Plan im Editor – gemerkt über Neustarts. */
let plan: SongPlan | null = (() => {
  try {
    const raw = localStorage.getItem(PLAN_KEY);
    return raw ? (JSON.parse(raw) as SongPlan) : null;
  } catch {
    return null;
  }
})();
function savePlan(): void {
  try {
    if (plan) localStorage.setItem(PLAN_KEY, JSON.stringify(plan));
  } catch {
    /* ohne Speicher */
  }
}

const bars = (p: SongPlan) => p.sections.reduce((a, s) => a + s.bars, 0);
const duration = (p: SongPlan) => {
  const secs = (bars(p) * 4 * 60) / p.tempo;
  return `${Math.floor(secs / 60)}:${String(Math.round(secs % 60)).padStart(2, "0")} min`;
};

/** Song in die Spuren schreiben (ersetzt die zuletzt erzeugten Song-Spuren). */
export async function writeSong(p: SongPlan): Promise<boolean> {
  try {
    const w = await backend.songWrite(p, true);
    snapshot.value = w.snapshot;
    // Zeitleiste auf den ganzen Song zoomen
    const tl = document.querySelector("kw-timeline") as HTMLElement | null;
    const width = (tl?.clientWidth ?? 900) - 40;
    viewport.value = { ...viewport.peek(), pxPerBeat: Math.min(400, Math.max(1.5, width / (w.bars * 4))), scrollX: 0 };
    notify(`Song „${p.name}“: ${w.bars} Takte, ${duration(p)}${w.replaced ? ` – ${w.replaced} alte Song-Spuren ersetzt` : ""} · Leertaste spielt ab`);
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

/** Menü „Mit Song starten“ (leeres Projekt): Vorlage wählen → Song erzeugen, Fenster „Song“ öffnen. */
export function fillSongMenu(menu: HTMLElement, close: () => void): void {
  const btn = menu.parentElement?.querySelector("button");
  if (btn) {
    const r = btn.getBoundingClientRect();
    const w = Math.min(520, window.innerWidth - 24);
    Object.assign(menu.style, { width: `${w}px`, left: `${Math.max(12, Math.min(window.innerWidth - w - 12, r.left + r.width / 2 - w / 2))}px`, top: `${r.bottom + 6}px` });
  }
  menu.replaceChildren(h("div", { class: "menu-title" }, "Song-Vorlagen – ganze Stücke zum Weiterbauen"));
  loadTemplates()
    .then((ts) => {
      menu.append(
        ...ts.map((t) => {
          const b = h("button", { class: "song-tpl-item", title: t.description },
            h("b", {}, t.name), h("span", {}, `${Math.round(t.tempo)} BPM · ${t.key} · ${bars(t)} Takte`), h("small", {}, t.description));
          b.dataset.template = t.template;
          b.addEventListener("click", () => {
            close();
            plan = clone(t);
            savePlan();
            notify(`Song „${t.name}“ wird erzeugt …`);
            void writeSong(plan).then((ok) => ok && showPanel("song"));
          });
          return b;
        }),
      );
    })
    .catch((e) => menu.append(h("div", { class: "muted" }, errorText(e))));
}

export function songPanel(): { el: HTMLElement; update(): void } {
  const el = h("div", { class: "song" }, h("div", { class: "placeholder" }, "Vorlagen werden geladen …"));
  let templates: SongPlan[] = [];

  const tplBar = h("div", { class: "song-templates" });
  const desc = h("div", { class: "song-desc" });
  const tempoIn = h("input", { type: "number", min: 40, max: 220, step: 1, class: "song-tempo", title: "Tempo (BPM)" }) as HTMLInputElement;
  const keyIn = h("input", { type: "text", class: "song-key", title: "Tonart, z. B. a-Moll, D-Dur", spellcheck: false }) as HTMLInputElement;
  const chordsIn = h("input", { type: "text", class: "song-chords", title: "Akkorde reihum – deutsche Tonnamen (a = a-Moll, B = englisch B♭) oder Stufen (i, VI, III, VII)" }) as HTMLInputElement;
  const chordBarsSeg = h("div", { class: "seg mini song-chordbars", title: "Takte je Akkord" });
  const grid = h("div", { class: "song-grid" });
  const scroll = h("div", { class: "song-scroll" }, grid);
  const summary = h("span", { class: "song-summary" });
  const writeBtn = h("button", { class: "btn primary song-write", title: "Spuren aus dem Aufbau erzeugen – ersetzt die zuletzt erzeugten Song-Spuren (Rückgängig holt sie zurück)" }, icon("sparkle"), "Song erzeugen") as HTMLButtonElement;
  const playBtn = h("button", { class: "btn song-play", title: "Von vorn abspielen" }, icon("play"), "Von vorn") as HTMLButtonElement;
  const headers: HTMLElement[] = [];

  const changed = () => {
    savePlan();
    render();
  };

  // ------------------------------------------------------------ Darstellung
  const renderTop = () => {
    if (!plan) return;
    for (const b of tplBar.children) (b as HTMLElement).classList.toggle("on", (b as HTMLElement).dataset.template === plan.template);
    const t = templates.find((x) => x.template === plan!.template);
    const edited = !!t && JSON.stringify(t) !== JSON.stringify(plan);
    desc.textContent = `${plan.description}${edited ? " · bearbeitet" : ""}`;
    if (document.activeElement !== tempoIn) tempoIn.value = String(Math.round(plan.tempo));
    if (document.activeElement !== keyIn) keyIn.value = plan.key;
    if (document.activeElement !== chordsIn) chordsIn.value = plan.chords.join(" ");
    for (const b of chordBarsSeg.children) (b as HTMLElement).classList.toggle("on", Number((b as HTMLElement).dataset.bars) === plan.chordBars);
    summary.textContent = `${plan.sections.length} Abschnitte · ${bars(plan)} Takte · ${duration(plan)}`;
  };

  const sectionHead = (s: SongSection, i: number) => {
    const name = h("input", { type: "text", class: "song-sec-name", title: "Name des Abschnitts" }) as HTMLInputElement;
    name.value = s.name;
    name.spellcheck = false;
    name.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") name.blur();
    });
    name.addEventListener("change", () => {
      s.name = name.value.trim() || s.name;
      changed();
    });
    const barsSel = h("select", { class: "song-sec-bars", title: "Takte" },
      ...[2, 4, 8, 12, 16, 24, 32].map((n) => h("option", { value: n }, `${n} T.`))) as HTMLSelectElement;
    if (![2, 4, 8, 12, 16, 24, 32].includes(s.bars)) barsSel.append(h("option", { value: s.bars }, `${s.bars} T.`));
    barsSel.value = String(s.bars);
    barsSel.addEventListener("change", () => {
      s.bars = Number(barsSel.value);
      changed();
    });
    const kindSel = h("select", { class: "song-sec-kind", title: KINDS.find((k) => k[0] === s.kind)?.[2] ?? "" },
      ...KINDS.map(([k, label, hint]) => h("option", { value: k, title: hint }, `${KIND_MARK[k]} ${label}`.trim()))) as HTMLSelectElement;
    kindSel.value = s.kind;
    kindSel.addEventListener("change", () => {
      s.kind = kindSel.value as SongSectionKind;
      changed();
    });
    const move = (d: number) => {
      const j = i + d;
      if (!plan || j < 0 || j >= plan.sections.length) return;
      [plan.sections[i], plan.sections[j]] = [plan.sections[j]!, plan.sections[i]!];
      changed();
    };
    const tools = h("div", { class: "song-sec-tools" },
      h("button", { class: "btn ghost small", title: "nach vorn", onclick: () => move(-1) }, "◀"),
      h("button", { class: "btn ghost small", title: "nach hinten", onclick: () => move(1) }, "▶"),
      h("button", { class: "btn ghost small", title: "verdoppeln", onclick: () => {
        plan!.sections.splice(i + 1, 0, { ...JSON.parse(JSON.stringify(s)), name: `${s.name} 2` });
        changed();
      } }, icon("copy")),
      h("button", { class: "btn ghost small", title: "entfernen", onclick: () => {
        if (plan!.sections.length <= 1) return;
        plan!.sections.splice(i, 1);
        changed();
      } }, icon("close")));
    const head = h("div", { class: `song-sec kind-${s.kind}` }, name, h("div", { class: "song-sec-row" }, barsSel, kindSel), tools);
    head.dataset.index = String(i);
    return head;
  };

  const render = () => {
    if (!plan) return;
    renderTop();
    const p = plan;
    const cols = p.sections.map((s) => `${Math.max(112, s.bars * BAR_PX)}px`).join(" ");
    grid.style.gridTemplateColumns = `150px ${cols} 92px`;
    headers.length = 0;
    const cells: HTMLElement[] = [h("div", { class: "song-corner" }, "Rolle ↓ · Abschnitt →")];
    p.sections.forEach((s, i) => {
      const hd = sectionHead(s, i);
      headers.push(hd);
      cells.push(hd);
    });
    cells.push(h("button", { class: "btn small song-add", title: "Abschnitt anhängen", onclick: () => {
      p.sections.push({ name: `Teil ${p.sections.length + 1}`, bars: 8, kind: "teil", levels: {} });
      changed();
    } }, icon("plus"), "Abschnitt"));
    for (const l of p.layers) {
      const lab = h("div", { class: `song-layer role-${l.role}`, title: [l.sound, l.verlauf].filter(Boolean).join(" · ") },
        h("i"), h("b", {}, l.name), h("span", {}, l.sound ?? (["kick", "hats", "clap", "perc", "fx"].includes(l.role) ? "Beat" : "")));
      cells.push(lab);
      p.sections.forEach((s, i) => {
        const lv = s.levels[l.key] ?? 0;
        const c = h("button", { class: `song-cell role-${l.role} l${lv}`, title: `${l.name} in „${s.name}“: ${LEVEL_WORD[lv]} – Klick: mehr · Umschalt+Klick: weniger` }, h("i"));
        c.dataset.layer = l.key;
        c.dataset.section = String(i);
        c.addEventListener("click", (e) => {
          const cur = s.levels[l.key] ?? 0;
          const next = (cur + (e.shiftKey ? 2 : 1)) % 3;
          if (next) s.levels[l.key] = next;
          else delete s.levels[l.key];
          changed();
        });
        c.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          const cur = s.levels[l.key] ?? 0;
          const next = (cur + 2) % 3;
          if (next) s.levels[l.key] = next;
          else delete s.levels[l.key];
          changed();
        });
        cells.push(c);
      });
      cells.push(h("div"));
    }
    grid.replaceChildren(...cells);
    lastRendered = JSON.stringify(p);
    markPlaying();
  };

  /** Abschnitt unter der Abspielposition hervorheben. */
  let lastNow = -2;
  const markPlaying = () => {
    if (!plan) return;
    const st = status.peek();
    let now = -1;
    if (st.playing) {
      const bar = (st.positionSecs * (project()?.tempoBpm ?? plan.tempo)) / 60 / 4;
      let acc = 0;
      now = plan.sections.findIndex((s) => {
        acc += s.bars;
        return bar < acc;
      });
    }
    if (now === lastNow && headers.every((hd) => hd.isConnected)) return;
    lastNow = now;
    headers.forEach((hd, i) => hd.classList.toggle("now", i === now));
  };
  effect(() => {
    status.value;
    markPlaying();
  });

  // ------------------------------------------------------------ Aufbau
  const build = () => {
    tplBar.replaceChildren(
      ...templates.map((t) => {
        const b = h("button", { class: "song-tpl", title: `${t.description}\nKlick: Vorlage laden (ersetzt den Aufbau im Editor)` }, t.name);
        b.dataset.template = t.template;
        b.addEventListener("click", () => {
          plan = clone(t);
          changed();
        });
        return b;
      }),
    );
    [1, 2, 4].forEach((n) => {
      const b = h("button", { class: "seg-btn", title: `${n} Takt${n > 1 ? "e" : ""} je Akkord` }, String(n));
      b.dataset.bars = String(n);
      b.addEventListener("click", () => {
        plan!.chordBars = n;
        changed();
      });
      chordBarsSeg.append(b);
    });
    for (const inp of [tempoIn, keyIn, chordsIn]) inp.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") inp.blur();
    });
    tempoIn.addEventListener("change", () => {
      const v = Number(tempoIn.value);
      if (v >= 40 && v <= 220) plan!.tempo = v;
      changed();
    });
    keyIn.addEventListener("change", () => {
      if (keyIn.value.trim()) plan!.key = keyIn.value.trim();
      changed();
    });
    chordsIn.addEventListener("change", () => {
      // getrennt durch Leerzeichen, Komma, Strich („a – F – C – G“)
      const list = chordsIn.value.split(/[\s,;|]+/).map((c) => c.trim()).filter((c) => c && !/^[-–—]+$/.test(c));
      if (list.length) plan!.chords = list;
      changed();
    });
    writeBtn.addEventListener("click", async () => {
      if (!plan || writeBtn.disabled) return;
      writeBtn.disabled = true;
      writeBtn.replaceChildren(icon("sparkle"), "Erzeuge …");
      await writeSong(plan);
      writeBtn.disabled = false;
      writeBtn.replaceChildren(icon("sparkle"), "Song erzeugen");
    });
    playBtn.addEventListener("click", async () => {
      try {
        await backend.seek(0);
        await backend.transport("play");
      } catch (e) {
        notify(errorText(e), "error");
      }
    });
    el.replaceChildren(
      h("div", { class: "song-top" },
        h("div", { class: "song-tpl-row" }, h("span", { class: "muted" }, "Vorlage:"), tplBar),
        desc,
        h("div", { class: "song-props" },
          h("label", {}, "Tempo", tempoIn),
          h("label", {}, "Tonart", keyIn),
          h("label", { class: "grow" }, "Akkorde", chordsIn),
          h("label", {}, "je", chordBarsSeg, "Takte"))),
      scroll,
      h("div", { class: "song-foot" },
        summary,
        h("span", { class: "rh-gap" }),
        h("span", { class: "hint" }, "Klick: aus → sparsam → voll · Umschalt+Klick: zurück"),
        playBtn, writeBtn),
    );
    render();
  };

  loadTemplates()
    .then((ts) => {
      templates = ts;
      if (!plan || !Array.isArray(plan.sections) || !Array.isArray(plan.layers)) plan = clone(ts[0]!);
      build();
    })
    .catch(() => el.replaceChildren(h("div", { class: "placeholder" }, "Song-Vorlagen gibt es nur in der Desktop-App.")));

  return {
    el,
    update() {
      // Vorlagen-Menü im leeren Projekt kann den Plan ersetzt haben
      if (templates.length && plan && el.isConnected) {
        const key = JSON.stringify(plan);
        if (key !== lastRendered) {
          lastRendered = key;
          render();
        }
      }
    },
  };
}

let lastRendered = "";
