// "Combine variants" in the History window. The core compares the chosen
// variants with their common origin (klang-core merge.rs): per track the bars
// each one changed, whether it changed the track itself (sound, mix, added,
// removed), tempo and time signature. Here they are laid out on a timeline like
// the Song window: one row per track, one field per changed section – a click
// chooses which version goes into the result; a click on the ruler chooses for
// all tracks at once. Fields that several variants changed differently are
// conflicts and have to be decided. Each track can be opened into lanes that
// show the notes of every version. "Listen" plays the combination without
// changing the project; "Combine" creates it as a new variant (one step).

import { backend } from "../bridge.js";
import { fitCanvas, h, icon } from "../dom.js";
import { plural, t } from "../i18n.js";
import { effect } from "../signals.js";
import { act, errorText, notify, status } from "../store.js";
import type { HistoryView, MergeChoices, MergeSegment, MergeTrack, MergeView } from "../types.js";

/** The common origin. */
const ORIGIN = 0;
const ORIGIN_COLOR = "#8a93a6";
/** Colors of the variants: the base first. */
const COLORS = ["#ffb454", "#6ee7d8", "#c4a2ff", "#f472b6", "#60a5fa", "#4ade80", "#fb923c", "#facc15"];

interface Option {
  source: number;
  label: string;
  hint?: string;
}

/** Small menu of options at an element; a click outside or Esc closes it. */
function choose(anchor: HTMLElement, title: string, options: Option[], current: number | null, color: (s: number) => string, pick: (s: number) => void): void {
  document.querySelector(".mg-pop")?.remove();
  const pop = h("div", { class: "menu mg-pop open", role: "menu" }, h("div", { class: "menu-title" }, title));
  const close = () => {
    pop.remove();
    window.removeEventListener("pointerdown", outside, true);
    window.removeEventListener("keydown", onKey, true);
  };
  const outside = (e: PointerEvent) => {
    if (!pop.contains(e.target as Node)) close();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      close();
    }
  };
  for (const o of options) {
    const b = h("button", { class: `menu-item mg-opt${o.source === current ? " on" : ""}`, role: "menuitemradio", "aria-checked": String(o.source === current) },
      h("i", { style: `background:${color(o.source)}` }), h("em", {}, o.label), o.hint ? h("small", {}, o.hint) : null);
    b.dataset.source = String(o.source);
    b.addEventListener("click", () => {
      close();
      pick(o.source);
    });
    pop.append(b);
  }
  document.body.append(pop);
  const r = anchor.getBoundingClientRect();
  const w = pop.offsetWidth;
  pop.style.left = `${Math.max(8, Math.min(window.innerWidth - w - 8, r.left + r.width / 2 - w / 2))}px`;
  pop.style.top = `${Math.max(8, Math.min(window.innerHeight - pop.offsetHeight - 8, r.bottom + 4))}px`;
  window.addEventListener("pointerdown", outside, true);
  window.addEventListener("keydown", onKey, true);
  (pop.querySelector(".mg-opt") as HTMLElement | null)?.focus();
}

export function mergeView(history: HistoryView, onClose: () => void): { el: HTMLElement; refresh(): void; destroy(): void; variants(): number[] } {
  const active = history.variants.find((v) => v.active) ?? history.variants[0]!;
  let target = active.id;
  let others = new Set(history.variants.filter((v) => v.id !== target).map((v) => v.id));
  let plan: MergeView | null = null;
  let loading = 0;
  const trackChoice = new Map<string, number>();
  const segChoice = new Map<string, number>();
  let projectChoice: number | null = null;
  const expanded = new Set<string>();
  let preview: { tempo: number; end: number } | null = null;

  const segKey = (track: string, start: number) => `${track}|${start}`;
  const colorOf = (s: number) => {
    if (s === ORIGIN) return ORIGIN_COLOR;
    const i = plan ? plan.variants.findIndex((v) => v.id === s) : -1;
    return COLORS[(i < 0 ? 0 : i) % COLORS.length]!;
  };
  const nameOf = (s: number) => (s === ORIGIN ? t("Origin") : plan?.variants.find((v) => v.id === s)?.name ?? history.variants.find((v) => v.id === s)?.name ?? "?");

  /** Chosen (or default) source of a segment; null: open conflict. */
  const segSource = (tr: MergeTrack, s: MergeSegment): number | null => segChoice.get(segKey(tr.key, s.start)) ?? (s.conflict ? null : s.by[0]!);
  const trackSource = (tr: MergeTrack): number | null => trackChoice.get(tr.key) ?? (tr.settingsConflict ? null : tr.settingsBy[0] ?? ORIGIN);
  const projectSource = (): number | null => projectChoice ?? (plan?.project.conflict ? null : plan?.project.by[0] ?? ORIGIN);
  /** Is the track in the result (its own source has it)? */
  const included = (tr: MergeTrack) => {
    const s = trackSource(tr);
    return s == null || (s === ORIGIN ? tr.inOrigin : tr.presentIn.includes(s));
  };

  const choices = (): MergeChoices => ({
    project: projectChoice,
    tracks: Object.fromEntries(trackChoice),
    segments: [...segChoice].map(([k, source]) => {
      const i = k.lastIndexOf("|");
      return { key: k.slice(0, i), start: Number(k.slice(i + 1)), source };
    }),
  });

  // ------------------------------------------------------------ Structure
  const targetSel = h("select", { class: "mg-target", title: t("The variant the result builds on (its track order and name)") }) as HTMLSelectElement;
  const chips = h("div", { class: "mg-chips" });
  const origin = h("div", { class: "mg-origin" });
  const grid = h("div", { class: "mg-grid" });
  const scroll = h("div", { class: "mg-scroll" }, grid);
  const summary = h("div", { class: "mg-summary" });
  const playBtn = h("button", { class: "btn small mg-play", title: t("Listen to the combination from the start (or from the section last clicked on the ruler) – the project stays as it is") }, icon("play"), h("span", {}, t("Listen"))) as HTMLButtonElement;
  const applyBtn = h("button", { class: "btn small primary mg-apply" }, icon("check"), t("Combine → new variant")) as HTMLButtonElement;
  const backBtn = h("button", { class: "btn small ghost mg-back", title: t("Back to the table – nothing is changed") }, t("Cancel"));
  const el = h("div", { class: "merge" },
    h("div", { class: "mg-top" },
      h("span", { class: "muted" }, t("Base:")), targetSel,
      h("span", { class: "muted" }, t("plus:")), chips,
      h("span", { class: "rh-gap" }),
      h("span", { class: "hint" }, t("Click a field: choose the version · click the ruler: for all tracks"))),
    origin,
    scroll,
    h("div", { class: "mg-foot" }, summary, playBtn, backBtn, applyBtn));

  let fromBeat = 0;

  // ------------------------------------------------------------ Display
  const pct = (beats: number) => `${(beats / Math.max(1, plan!.lengthBeats)) * 100}%`;
  const span = (a: number, b: number) => `left:calc(${pct(a)} + 2px);width:calc(${(((b - a) / Math.max(1, plan!.lengthBeats)) * 100).toFixed(4)}% - 4px)`;
  const barsText = (a: number, b: number) => {
    const bpb = plan!.beatsPerBar || 4;
    const from = Math.floor(a / bpb + 1e-9) + 1;
    const to = Math.max(from, Math.ceil(b / bpb - 1e-9));
    return from === to ? t("bar {n}", { n: from }) : t("bars {from}–{to}", { from, to });
  };

  const gridlines = (lane: HTMLElement) => {
    const bpb = plan!.beatsPerBar || 4;
    const bars = Math.ceil(plan!.lengthBeats / bpb);
    const every = bars > 64 ? 8 : bars > 24 ? 4 : 1;
    for (let b = 0; b < bars; b += every) lane.append(h("div", { class: `mg-gl${b % (every * 4) === 0 ? " strong" : ""}`, style: `left:${pct(b * bpb)}` }));
  };

  const ruler = (): HTMLElement => {
    const p = plan!;
    const lane = h("div", { class: "mg-lane mg-ruler" });
    const bpb = p.beatsPerBar || 4;
    const bars = Math.ceil(p.lengthBeats / bpb);
    const every = bars > 64 ? 8 : bars > 24 ? 4 : bars > 12 ? 2 : 1;
    for (let b = 0; b < bars; b += every) lane.append(h("div", { class: "mg-tick", style: `left:${pct(b * bpb)}` }, String(b + 1)));
    // one button per section between the boundaries of all fields
    const cuts = new Set<number>();
    for (const tr of p.tracks) for (const s of tr.segments) {
      cuts.add(s.start);
      cuts.add(s.end);
    }
    const list = [...cuts].sort((a, b) => a - b);
    for (let i = 0; i + 1 < list.length; i++) {
      const [a, b] = [list[i]!, list[i + 1]!];
      const by = new Set<number>();
      for (const tr of p.tracks) for (const s of tr.segments) if (s.start < b && s.end > a) s.by.forEach((x) => by.add(x));
      if (!by.size) continue;
      const btn = h("button", { class: "mg-rseg", style: span(a, b), title: t("{bars}: choose for all tracks", { bars: barsText(a, b) }) });
      btn.dataset.start = String(a);
      btn.addEventListener("click", () => {
        fromBeat = a;
        choose(btn, t("{bars} · all tracks", { bars: barsText(a, b) }), [...by, ORIGIN].map((source) => ({ source, label: source === ORIGIN ? t("Origin (unchanged)") : nameOf(source) })), null, colorOf, (src) => {
          for (const tr of p.tracks) for (const s of tr.segments) {
            if (s.start < b && s.end > a && (src === ORIGIN || s.by.includes(src))) segChoice.set(segKey(tr.key, s.start), src);
          }
          render();
        });
      });
      lane.append(btn);
    }
    return h("div", { class: "mg-row" }, h("div", { class: "mg-label" }, h("span", { class: "muted" }, t("Bar"))), lane);
  };

  const projectRow = (): HTMLElement | null => {
    const p = plan!;
    if (!p.project.by.length) return null;
    const lane = h("div", { class: "mg-lane" });
    const cur = projectSource();
    const meter = (s: number) => {
      const m = p.project.options.find((o) => o.source === s) ?? p.project.options[0]!;
      return `${Math.round(m.tempo * 100) / 100} BPM · ${m.beatsPerBar}/${m.beatUnit}`;
    };
    const open = cur == null;
    const cell = h("button", {
      class: `mg-cell${p.project.conflict ? " conflict" : ""}${p.project.conflict && !open ? " resolved" : ""}${cur === ORIGIN ? " off" : ""}`,
      style: `left:2px;right:2px;--c:${cur == null ? "var(--rec)" : colorOf(cur)}`,
      title: t("Tempo and time signature: changed in {names}", { names: p.project.by.map(nameOf).join(", ") }),
    }, open ? h("b", { class: "warn" }, "!") : null, open ? t("Decide") : `${nameOf(cur!)} · ${meter(cur!)}`);
    cell.dataset.decision = "project";
    cell.addEventListener("click", () => choose(cell, t("Tempo and time signature"), [...p.project.by, ORIGIN].map((source) => ({ source, label: source === ORIGIN ? t("Origin (unchanged)") : nameOf(source), hint: meter(source) })), cur, colorOf, (s) => {
      projectChoice = s;
      render();
    }));
    lane.append(cell);
    return h("div", { class: "mg-row" }, h("div", { class: "mg-label" }, h("i", { class: "mg-dot", style: "background:var(--faint)" }), h("span", { class: "mg-name" }, t("Project"))), lane);
  };

  const trackLabel = (tr: MergeTrack): HTMLElement => {
    const kids: Node[] = [];
    const toggle = h("button", { class: `mg-expand${expanded.has(tr.key) ? " open" : ""}`, title: t("Show the notes of every version (lanes)"), disabled: !tr.segments.length }, icon("chevron"));
    toggle.addEventListener("click", () => {
      if (expanded.has(tr.key)) expanded.delete(tr.key);
      else expanded.add(tr.key);
      render();
    });
    kids.push(toggle, h("i", { class: "mg-dot", style: `background:${tr.color || "var(--faint)"}` }), h("span", { class: "mg-name", title: tr.name }, tr.name));
    if (tr.settingsBy.length) {
      const cur = trackSource(tr);
      const what = (s: number) => tr.changes.find((c) => c.source === s)?.what ?? "";
      const label = cur == null ? t("Decide") : cur === ORIGIN ? (tr.inOrigin ? t("as before") : t("leave out")) : `${nameOf(cur)}${what(cur) ? ` · ${what(cur)}` : ""}`;
      const chip = h("button", {
        class: `mg-track-chip${tr.settingsConflict ? " conflict" : ""}${tr.settingsConflict && cur != null ? " resolved" : ""}`,
        style: `--c:${cur == null ? "var(--rec)" : colorOf(cur)}`,
        title: t("The track itself (sound, mix, name, whether it exists): {changes}", { changes: tr.changes.map((c) => `${nameOf(c.source)}: ${c.what}`).join(" · ") }),
      }, cur == null ? h("b", { class: "warn" }, "!") : null, label);
      chip.dataset.decision = `track:${tr.key}`;
      chip.addEventListener("click", () => choose(chip, t("{name} · the track itself", { name: tr.name }),
        [...tr.settingsBy.map((source) => ({ source, label: nameOf(source), hint: what(source) })), { source: ORIGIN, label: tr.inOrigin ? t("Origin (unchanged)") : t("Leave the track out") }],
        cur, colorOf, (s) => {
          trackChoice.set(tr.key, s);
          render();
        }));
      kids.push(chip);
    }
    return h("div", { class: "mg-label" }, ...kids);
  };

  const segCell = (tr: MergeTrack, s: MergeSegment): HTMLElement => {
    const cur = segSource(tr, s);
    const open = cur == null;
    const off = cur === ORIGIN;
    const text = open ? t("Decide") : off ? (tr.inOrigin ? t("unchanged") : t("left out")) : nameOf(cur!);
    const cell = h("button", {
      class: `mg-cell${s.conflict ? " conflict" : ""}${s.conflict && !open ? " resolved" : ""}${off ? " off" : ""}`,
      style: `${span(s.start, s.end)};--c:${open ? "var(--rec)" : colorOf(cur!)}`,
      title: t("{track} · {bars}: changed in {names}", { track: tr.name, bars: barsText(s.start, s.end), names: s.by.map(nameOf).join(", ") }) + (s.conflict ? ` – ${t("differently: please choose")}` : ""),
    }, open ? h("b", { class: "warn" }, "!") : null, h("span", { class: "mg-cell-text" }, text),
      h("span", { class: "mg-alts" }, ...s.by.map((x) => h("i", { class: x === cur ? "sel" : "", style: `background:${colorOf(x)}` }))));
    cell.dataset.track = tr.key;
    cell.dataset.start = String(s.start);
    cell.addEventListener("click", () => {
      fromBeat = s.start;
      choose(cell, `${tr.name} · ${barsText(s.start, s.end)}`,
        [...s.by.map((source) => ({ source, label: nameOf(source) })), { source: ORIGIN, label: tr.inOrigin ? t("Origin (unchanged)") : t("Leave out") }],
        cur, colorOf, (src) => {
          segChoice.set(segKey(tr.key, s.start), src);
          render();
        });
    });
    return cell;
  };

  /** The notes of one version as small bars (lanes view). */
  const laneCanvas = (tr: MergeTrack, notes: [number, number, number][], audio: [number, number][], color: string): HTMLCanvasElement => {
    const canvas = h("canvas", { class: "mg-notes" }) as HTMLCanvasElement;
    const all = tr.lanes.flatMap((l) => l.notes.map((n) => n[2]));
    const lo = Math.min(...all, 127);
    const hi = Math.max(...all, 0);
    const draw = () => {
      if (!canvas.isConnected) return;
      const { ctx, w, h: hh } = fitCanvas(canvas);
      ctx.clearRect(0, 0, w, hh);
      const x = (b: number) => (b / Math.max(1, plan!.lengthBeats)) * w;
      ctx.fillStyle = color;
      for (const [a, b] of audio) {
        ctx.globalAlpha = 0.35;
        ctx.fillRect(x(a), 3, Math.max(2, x(b) - x(a)), hh - 6);
      }
      ctx.globalAlpha = 0.9;
      const range = Math.max(1, hi - lo);
      for (const [start, len, pitch] of notes) {
        const y = 2 + (1 - (pitch - lo) / range) * (hh - 6);
        ctx.fillRect(x(start), y, Math.max(1.5, x(start + len) - x(start) - 0.5), 2);
      }
    };
    new ResizeObserver(draw).observe(canvas);
    requestAnimationFrame(draw);
    return canvas;
  };

  const lanes = (tr: MergeTrack): HTMLElement[] =>
    tr.lanes.map((l) => {
      const lane = h("div", { class: "mg-lane mg-sublane" });
      gridlines(lane);
      lane.append(laneCanvas(tr, l.notes, l.audio, colorOf(l.source)));
      for (const s of tr.segments) {
        const can = l.source === ORIGIN || s.by.includes(l.source);
        const cur = segSource(tr, s);
        const pick = h("button", {
          class: `mg-pick${can ? " can" : ""}${cur === l.source ? " chosen" : ""}`,
          style: `${span(s.start, s.end)};--c:${colorOf(l.source)}`,
          title: can ? t("{bars} from \"{name}\"", { bars: barsText(s.start, s.end), name: nameOf(l.source) }) : t("unchanged in \"{name}\"", { name: nameOf(l.source) }),
          disabled: !can,
        });
        pick.dataset.track = tr.key;
        pick.dataset.start = String(s.start);
        pick.dataset.source = String(l.source);
        pick.addEventListener("click", () => {
          fromBeat = s.start;
          segChoice.set(segKey(tr.key, s.start), l.source);
          render();
        });
        lane.append(pick);
      }
      const label = l.source === ORIGIN ? t("Origin") : nameOf(l.source);
      return h("div", { class: "mg-row mg-sub" }, h("div", { class: "mg-label" }, h("i", { class: "mg-dot", style: `background:${colorOf(l.source)}` }), h("span", { class: "mg-name" }, l.present ? label : `${label} · ${t("no track")}`)), lane);
    });

  const trackRows = (tr: MergeTrack): HTMLElement[] => {
    const lane = h("div", { class: "mg-lane" });
    gridlines(lane);
    if (!included(tr)) {
      lane.append(h("div", { class: "mg-gone" }, tr.inOrigin ? t("removed") : t("left out")));
    } else if (!tr.segments.length) {
      lane.append(h("div", { class: "mg-same", title: t("The same in every version") }));
    } else {
      for (const s of tr.segments) lane.append(segCell(tr, s));
    }
    const row = h("div", { class: `mg-row${included(tr) ? "" : " excluded"}` }, trackLabel(tr), lane);
    row.dataset.track = tr.key;
    return [row, ...(expanded.has(tr.key) && included(tr) ? lanes(tr) : [])];
  };

  let head: HTMLElement | null = null;

  const renderSummary = () => {
    const p = plan!;
    const from = new Map<number, string[]>();
    const add = (s: number, what: string) => {
      if (s === ORIGIN) return;
      from.set(s, [...(from.get(s) ?? []), what]);
    };
    let open = 0;
    const ps = projectSource();
    if (p.project.by.length) {
      if (ps == null) open++;
      else add(ps, t("tempo"));
    }
    for (const tr of p.tracks) {
      const ts = trackSource(tr);
      if (tr.settingsBy.length) {
        if (ts == null) open++;
        else add(ts, `${tr.name} (${tr.changes.find((c) => c.source === ts)?.what ?? ""})`);
      }
      if (!included(tr)) continue;
      for (const s of tr.segments) {
        const cur = segSource(tr, s);
        if (cur == null) open++;
        else add(cur, `${tr.name} ${barsText(s.start, s.end)}`);
      }
    }
    const parts = p.variants.filter((v) => from.has(v.id)).map((v) => h("span", { class: "mg-from" }, h("b", { style: `color:${colorOf(v.id)}` }, v.name), `: ${from.get(v.id)!.join(", ")}`));
    summary.replaceChildren(
      ...(parts.length ? parts.flatMap((x, i) => (i ? [" · ", x] : [x])) : [h("span", {}, t("No differences to combine"))]),
      open ? h("span", { class: "mg-open" }, ` · ${plural(open, "{n} conflict open", "{n} conflicts open")}`) : h("span", { class: "mg-done" }, ` · ${t("all decided")}`),
    );
    applyBtn.disabled = open > 0;
    applyBtn.title = open ? t("Decide the fields marked red first") : t("Creates \"{name}\" as a new variant with one step – the others stay as they are", { name: p.name });
  };

  const render = () => {
    const p = plan;
    if (!p) return;
    origin.replaceChildren(
      t("Compared with their common origin: step {index} · {label}", { index: p.origin.index, label: p.origin.label }),
      ...p.origin.checkpoints.map((c) => h("span", { class: "hist-flag mg-origin-cp" }, icon("flag"), c)),
      ` · ${t("sections rounded to whole bars")}`,
    );
    const rows: HTMLElement[] = [ruler()];
    const pr = projectRow();
    if (pr) rows.push(pr);
    for (const tr of p.tracks) rows.push(...trackRows(tr));
    head = h("div", { class: "mg-head" });
    grid.replaceChildren(...rows, head);
    renderSummary();
    movePlayhead();
  };

  const renderTop = () => {
    targetSel.replaceChildren(...history.variants.map((v) => h("option", { value: String(v.id) }, v.name)));
    targetSel.value = String(target);
    chips.replaceChildren(...history.variants.filter((v) => v.id !== target).map((v) => {
      const on = others.has(v.id);
      const b = h("button", { class: `hist-var mg-chip${on ? " on" : ""}`, "aria-pressed": String(on), title: on ? t("Taking part – click: leave out") : t("Click: take part") }, h("i"), v.name);
      b.dataset.variant = String(v.id);
      b.addEventListener("click", () => {
        if (on && others.size === 1) return notify(t("Combine needs at least one more variant"));
        if (on) others.delete(v.id);
        else others.add(v.id);
        renderTop();
        void load();
      });
      return b;
    }));
  };

  // ------------------------------------------------------------ Loading
  const load = async () => {
    const n = ++loading;
    try {
      const p = await backend.historyMergePlan(target, [...others]);
      if (n !== loading) return;
      plan = p;
      // keep the choices that still fit
      const segs = new Set(p.tracks.flatMap((tr) => tr.segments.map((s) => segKey(tr.key, s.start))));
      for (const k of [...segChoice.keys()]) if (!segs.has(k)) segChoice.delete(k);
      const keys = new Set(p.tracks.map((tr) => tr.key));
      for (const k of [...trackChoice.keys()]) if (!keys.has(k)) trackChoice.delete(k);
      if (projectChoice != null && projectChoice !== ORIGIN && !p.project.by.includes(projectChoice)) projectChoice = null;
      // colors of the chips like the fields
      for (const c of chips.querySelectorAll<HTMLElement>(".mg-chip")) c.style.setProperty("--c", colorOf(Number(c.dataset.variant)));
      render();
    } catch (e) {
      if (n === loading) grid.replaceChildren(h("div", { class: "placeholder" }, errorText(e)));
    }
  };

  targetSel.addEventListener("change", () => {
    target = Number(targetSel.value);
    others = new Set(history.variants.filter((v) => v.id !== target).map((v) => v.id));
    trackChoice.clear();
    segChoice.clear();
    projectChoice = null;
    renderTop();
    void load();
  });

  // ------------------------------------------------------------ Listening
  const movePlayhead = () => {
    if (!head || !plan) return;
    if (!preview) {
      head.hidden = true;
      return;
    }
    const beat = (status.peek().positionSecs * preview.tempo) / 60;
    const label = (grid.querySelector(".mg-label") as HTMLElement | null)?.offsetWidth ?? 0;
    head.hidden = false;
    head.style.left = `calc(${label}px + (100% - ${label}px) * ${Math.min(1, beat / Math.max(1, plan.lengthBeats))})`;
  };
  const stopPreview = async () => {
    if (!preview) return;
    preview = null;
    playBtn.replaceChildren(icon("play"), h("span", {}, t("Listen")));
    playBtn.classList.remove("on");
    movePlayhead();
    try {
      await backend.historyMergePreviewStop();
    } catch {
      /* without engine */
    }
  };
  let sawPlaying = false;
  const stopWatch = effect(() => {
    const s = status.value;
    if (!preview) return;
    if (s.playing) sawPlaying = true;
    else if (sawPlaying) void stopPreview();
    movePlayhead();
  });
  playBtn.addEventListener("click", async () => {
    if (preview) return void stopPreview();
    try {
      const r = await backend.historyMergePreview(target, [...others], choices(), fromBeat);
      preview = { tempo: r.tempo, end: r.endBeats };
      sawPlaying = false;
      playBtn.replaceChildren(icon("stop"), h("span", {}, t("Stop")));
      playBtn.classList.add("on");
    } catch (e) {
      notify(errorText(e), "error");
    }
  });

  applyBtn.addEventListener("click", async () => {
    if (!plan || applyBtn.disabled) return;
    await stopPreview();
    const name = plan.name;
    if (await act(backend.historyMergeApply(target, [...others], choices(), null))) {
      notify(t("Variant \"{name}\" created – the other variants stay as they are", { name }));
      onClose();
    }
  });
  backBtn.addEventListener("click", () => {
    void stopPreview();
    onClose();
  });

  renderTop();
  void load();

  return {
    el,
    refresh: () => void load(),
    variants: () => [target, ...others],
    destroy: () => {
      void stopPreview();
      stopWatch();
      document.querySelector(".mg-pop")?.remove();
    },
  };
}
