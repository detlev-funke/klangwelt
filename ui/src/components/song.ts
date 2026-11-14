// Song window: mini editor for building a piece. Columns are the
// sections (as wide as their bars), rows the roles; each cell has a
// level – off, sparse, full (click advances, Shift+click goes back).
// "Generate song" writes the tracks from it and replaces the most recently
// generated ones. Templates (trance, ambient …) are starting points.

import { backend } from "../bridge.js";
import { chordsInput, chordsText, keyInput, keyText } from "../spelling.js";
import { h, icon } from "../dom.js";
import { plural, t } from "../i18n.js";
import { effect } from "../signals.js";
import { errorText, notify, project, showPanel, snapshot, status, viewport } from "../store.js";
import type { SongPlan, SongSection, SongSectionKind } from "../types.js";

const PLAN_KEY = "klangwelt.song.plan";
const BAR_PX = 9;

const KINDS: Array<[SongSectionKind, string, string]> = [
  ["intro", t("Intro"), t("starts gently")],
  ["teil", t("Part"), t("steady")],
  ["aufbau", t("Build-up"), t("gets louder and brighter, ends (with transitions) in a snare roll")],
  ["break", t("Break"), t("stripped back")],
  ["hoehepunkt", t("Peak"), t("full, cymbal at the start")],
  ["outro", t("Outro"), t("fades out")],
];
const KIND_MARK: Record<SongSectionKind, string> = { intro: "◔", teil: "", aufbau: "↗", break: "◦", hoehepunkt: "★", outro: "↘" };
const LEVEL_WORD = [t("off"), t("sparse"), t("full")];

let templatesPromise: Promise<SongPlan[]> | null = null;
export function loadTemplates(): Promise<SongPlan[]> {
  templatesPromise ??= backend.songTemplates().catch((e) => {
    templatesPromise = null;
    throw e;
  });
  return templatesPromise;
}

const clone = (p: SongPlan): SongPlan => JSON.parse(JSON.stringify(p)) as SongPlan;

/** The plan in the editor – remembered across restarts. */
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
    /* without storage */
  }
}

const bars = (p: SongPlan) => p.sections.reduce((a, s) => a + s.bars, 0);
const duration = (p: SongPlan) => {
  const secs = (bars(p) * 4 * 60) / p.tempo;
  return `${Math.floor(secs / 60)}:${String(Math.round(secs % 60)).padStart(2, "0")} min`;
};

/** Write the song into the tracks (replaces the most recently generated song tracks). */
export async function writeSong(p: SongPlan): Promise<boolean> {
  try {
    const w = await backend.songWrite(p, true);
    snapshot.value = w.snapshot;
    // Zoom the timeline to the whole song
    const tl = document.querySelector("kw-timeline") as HTMLElement | null;
    const width = (tl?.clientWidth ?? 900) - 40;
    viewport.value = { ...viewport.peek(), pxPerBeat: Math.min(400, Math.max(1.5, width / (w.bars * 4))), scrollX: 0 };
    const vars = { name: p.name, bars: w.bars, duration: duration(p) };
    notify(
      w.replaced
        ? plural(w.replaced, "Song \"{name}\": {bars} bars, {duration} – replaced {n} old song track · Space plays", "Song \"{name}\": {bars} bars, {duration} – replaced {n} old song tracks · Space plays", vars)
        : t("Song \"{name}\": {bars} bars, {duration} · Space plays", vars),
    );
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

/** "Start with a song" menu (empty project): choose a template → generate song, open the "Song" window. */
export function fillSongMenu(menu: HTMLElement, close: () => void): void {
  const btn = menu.parentElement?.querySelector("button");
  if (btn) {
    const r = btn.getBoundingClientRect();
    const w = Math.min(520, window.innerWidth - 24);
    Object.assign(menu.style, { width: `${w}px`, left: `${Math.max(12, Math.min(window.innerWidth - w - 12, r.left + r.width / 2 - w / 2))}px`, top: `${r.bottom + 6}px` });
  }
  menu.replaceChildren(h("div", { class: "menu-title" }, t("Song templates – complete pieces to build on")));
  loadTemplates()
    .then((ts) => {
      menu.append(
        ...ts.map((tpl) => {
          const b = h("button", { class: "song-tpl-item", title: tpl.description },
            h("b", {}, tpl.name), h("span", {}, `${Math.round(tpl.tempo)} BPM · ${keyText(tpl.key)} · ${plural(bars(tpl), "{n} bar", "{n} bars")}`), h("small", {}, tpl.description));
          b.dataset.template = tpl.template;
          b.addEventListener("click", () => {
            close();
            plan = clone(tpl);
            savePlan();
            notify(t("Generating song \"{name}\"…", { name: tpl.name }));
            void writeSong(plan).then((ok) => ok && showPanel("song"));
          });
          return b;
        }),
      );
    })
    .catch((e) => menu.append(h("div", { class: "muted" }, errorText(e))));
}

export function songPanel(): { el: HTMLElement; update(): void } {
  const el = h("div", { class: "song" }, h("div", { class: "placeholder" }, t("Loading templates…")));
  let templates: SongPlan[] = [];

  const tplBar = h("div", { class: "song-templates" });
  const desc = h("div", { class: "song-desc" });
  const tempoIn = h("input", { type: "number", min: 40, max: 220, step: 1, class: "song-tempo", title: t("Tempo (BPM)") }) as HTMLInputElement;
  const keyIn = h("input", { type: "text", class: "song-key", title: t("Key, e.g. A minor, D major"), spellcheck: false }) as HTMLInputElement;
  const chordsIn = h("input", { type: "text", class: "song-chords", title: t("Chords in turn – lower case is minor (a = A minor; F♯ or F#, B♭ or Bb) – or degrees (i, VI, III, VII)") }) as HTMLInputElement;
  const chordBarsSeg = h("div", { class: "seg mini song-chordbars", title: t("Bars per chord") });
  const grid = h("div", { class: "song-grid" });
  const scroll = h("div", { class: "song-scroll" }, grid);
  const summary = h("span", { class: "song-summary" });
  const writeBtn = h("button", { class: "btn primary song-write", title: t("Generate tracks from the structure – replaces the most recently generated song tracks (undo brings them back)") }, icon("sparkle"), t("Generate song")) as HTMLButtonElement;
  const playBtn = h("button", { class: "btn song-play", title: t("Play from the start") }, icon("play"), t("From start")) as HTMLButtonElement;
  const headers: HTMLElement[] = [];

  const changed = () => {
    savePlan();
    render();
  };

  // ------------------------------------------------------------ Display
  const renderTop = () => {
    if (!plan) return;
    for (const b of tplBar.children) (b as HTMLElement).classList.toggle("on", (b as HTMLElement).dataset.template === plan.template);
    const tpl = templates.find((x) => x.template === plan!.template);
    const edited = !!tpl && JSON.stringify(tpl) !== JSON.stringify(plan);
    desc.textContent = `${plan.description}${edited ? ` · ${t("edited")}` : ""}`;
    if (document.activeElement !== tempoIn) tempoIn.value = String(Math.round(plan.tempo));
    if (document.activeElement !== keyIn) keyIn.value = keyText(plan.key);
    if (document.activeElement !== chordsIn) chordsIn.value = chordsText(plan.chords.join(" "));
    for (const b of chordBarsSeg.children) (b as HTMLElement).classList.toggle("on", Number((b as HTMLElement).dataset.bars) === plan.chordBars);
    summary.textContent = [
      plural(plan.sections.length, "{n} section", "{n} sections"),
      plural(bars(plan), "{n} bar", "{n} bars"),
      duration(plan),
    ].join(" · ");
  };

  const sectionHead = (s: SongSection, i: number) => {
    const name = h("input", { type: "text", class: "song-sec-name", title: t("Section name") }) as HTMLInputElement;
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
    const barsSel = h("select", { class: "song-sec-bars", title: t("Bars") },
      ...[2, 4, 8, 12, 16, 24, 32].map((n) => h("option", { value: n }, t("{n} bars", { n })))) as HTMLSelectElement;
    if (![2, 4, 8, 12, 16, 24, 32].includes(s.bars)) barsSel.append(h("option", { value: s.bars }, t("{n} bars", { n: s.bars })));
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
      h("button", { class: "btn ghost small", title: t("move earlier"), onclick: () => move(-1) }, "◀"),
      h("button", { class: "btn ghost small", title: t("move later"), onclick: () => move(1) }, "▶"),
      h("button", { class: "btn ghost small", title: t("duplicate"), onclick: () => {
        plan!.sections.splice(i + 1, 0, { ...JSON.parse(JSON.stringify(s)), name: `${s.name} 2` });
        changed();
      } }, icon("copy")),
      h("button", { class: "btn ghost small", title: t("remove"), onclick: () => {
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
    const cells: HTMLElement[] = [h("div", { class: "song-corner" }, t("Role ↓ · Section →"))];
    p.sections.forEach((s, i) => {
      const hd = sectionHead(s, i);
      headers.push(hd);
      cells.push(hd);
    });
    cells.push(h("button", { class: "btn small song-add", title: t("Add section"), onclick: () => {
      p.sections.push({ name: t("Part {n}", { n: p.sections.length + 1 }), bars: 8, kind: "teil", levels: {} });
      changed();
    } }, icon("plus"), t("Section")));
    for (const l of p.layers) {
      const lab = h("div", { class: `song-layer role-${l.role}`, title: [l.sound, l.verlauf].filter((x): x is string => !!x).map((x) => t(x)).join(" · ") },
        h("i"), h("b", {}, l.name), h("span", {}, l.sound ? t(l.sound) : (["kick", "hats", "clap", "perc", "fx"].includes(l.role) ? t("Beat") : "")));
      cells.push(lab);
      p.sections.forEach((s, i) => {
        const lv = s.levels[l.key] ?? 0;
        const c = h("button", {
          class: `song-cell role-${l.role} l${lv}`,
          title: t("{layer} in \"{section}\": {level} – click: more · Shift+click: less", { layer: l.name, section: s.name, level: LEVEL_WORD[lv] ?? "" }),
        }, h("i"));
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

  /** Highlight the section under the playhead. */
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

  // ------------------------------------------------------------ Structure
  const build = () => {
    tplBar.replaceChildren(
      ...templates.map((tpl) => {
        const b = h("button", { class: "song-tpl", title: `${tpl.description}\n${t("Click: load template (replaces the structure in the editor)")}` }, tpl.name);
        b.dataset.template = tpl.template;
        b.addEventListener("click", () => {
          plan = clone(tpl);
          changed();
        });
        return b;
      }),
    );
    [1, 2, 4].forEach((n) => {
      const b = h("button", { class: "seg-btn", title: plural(n, "{n} bar per chord", "{n} bars per chord") }, String(n));
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
      if (keyIn.value.trim()) plan!.key = keyInput(keyIn.value.trim());
      changed();
    });
    chordsIn.addEventListener("change", () => {
      // separated by space, comma, dash ("a – F – C – G")
      const list = chordsIn.value.split(/[\s,;|]+/).map((c) => c.trim()).filter((c) => c && !/^[-–—]+$/.test(c)).map(chordsInput);
      if (list.length) plan!.chords = list;
      changed();
    });
    writeBtn.addEventListener("click", async () => {
      if (!plan || writeBtn.disabled) return;
      writeBtn.disabled = true;
      writeBtn.replaceChildren(icon("sparkle"), t("Generating…"));
      await writeSong(plan);
      writeBtn.disabled = false;
      writeBtn.replaceChildren(icon("sparkle"), t("Generate song"));
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
        h("div", { class: "song-tpl-row" }, h("span", { class: "muted" }, t("Template:")), tplBar),
        desc,
        h("div", { class: "song-props" },
          h("label", {}, t("Tempo"), tempoIn),
          h("label", {}, t("Key"), keyIn),
          h("label", { class: "grow" }, t("Chords"), chordsIn),
          h("label", {}, t("per"), chordBarsSeg, t("Bars")))),
      scroll,
      h("div", { class: "song-foot" },
        summary,
        h("span", { class: "rh-gap" }),
        h("span", { class: "hint" }, t("Click: off → sparse → full · Shift+click: back")),
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
    .catch(() => el.replaceChildren(h("div", { class: "placeholder" }, t("Song templates are only available in the desktop app."))));

  return {
    el,
    update() {
      // the templates menu in the empty project may have replaced the plan
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
