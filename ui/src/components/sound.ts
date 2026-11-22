// Sound window: choose an instrument, shape the sound in fields (character,
// timbre, room – each with its own starting points that only set its own
// sliders), time course as a waveform (for the synth with drawn
// curves and curve presets), all sliders in fine-tuning, measurements.

import { backend } from "../bridge.js";
import { fitCanvas, h, icon, noteName, setRange } from "../dom.js";
import { language, plural, t } from "../i18n.js";
import { act, errorText, findTrack, instrumentSpecs, notify, project, run, savedSounds, showPanel, snapshot, specFor } from "../store.js";
import { KEYS_MODE, refitPitch } from "../pianoroll.js";
import { picked, pickedTracks } from "../groups.js";
import { effect } from "../signals.js";
import type { Command, CurvePoint, FieldSpec, Instrument, InstrumentSpec, ParamSpec, PreviewPattern, SavedSound, VerlaufSpec, WaveView } from "../types.js";

export interface SoundPanel {
  el: HTMLElement;
  update(): void;
}

/** Preview pattern per slider group for drums (click on the group heading). */
const GROUP_PREVIEW: Record<string, string> = {
  Bassdrum: "kick", Snare: "snare", Clap: "clap", "Hi-Hat": "hihat", Toms: "toms",
};

export function defaultParams(spec: InstrumentSpec): Record<string, number> {
  return Object.fromEntries(spec.params.map((p) => [p.key, p.default]));
}

/** Instrument with default values (synth: with the first curve preset). */
export function defaultInstrument(spec: InstrumentSpec): Instrument {
  const v = spec.verlauf[0];
  return { kind: spec.kind, preset: "standard", params: defaultParams(spec), ...(v ? { curves: { laut: v.laut, hell: v.hell } } : {}) };
}

// ---------------------------------------------------------------- View settings

const VIEW_KEY = "klangwelt.klang.ansicht";
/** What briefly sounds after a change: nothing, a single note (drums: bass drum) or the pattern. */
type PlayMode = "aus" | "ton" | "muster";
type ViewPrefs = { klangfarbe: boolean; raum: boolean; anspielen: PlayMode; feinschliff: boolean; zeitverlauf: boolean; klaenge: boolean };
function viewPrefs(): ViewPrefs {
  const d: ViewPrefs = { klangfarbe: true, raum: true, anspielen: "ton", feinschliff: false, zeitverlauf: true, klaenge: false };
  try {
    const v = { ...d, ...(JSON.parse(localStorage.getItem(VIEW_KEY) ?? "{}") as Partial<Record<keyof ViewPrefs, unknown>>) };
    // used to be a checkbox: on → single note, off → off
    const a = v.anspielen;
    return { ...(v as ViewPrefs), anspielen: a === false ? "aus" : a === "aus" || a === "muster" ? a : "ton" };
  } catch {
    return d;
  }
}

/** Preview pattern for auditioning after a change. */
export function playPattern(spec: InstrumentSpec, mode: PlayMode): PreviewPattern | null {
  if (mode === "aus") return null;
  if (mode === "muster") return spec.previews[0]?.key ?? null;
  const single = spec.kind === "drums" ? "kick" : "einzelton";
  return spec.previews.find((p) => p.key === single)?.key ?? spec.previews[0]?.key ?? null;
}
function setViewPref<K extends keyof ViewPrefs>(k: K, v: ViewPrefs[K]): void {
  try {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ ...viewPrefs(), [k]: v }));
  } catch {
    /* without storage it simply isn't remembered */
  }
}

const word = (words: readonly string[], v: number) => words[Math.min(3, Math.max(0, Math.floor(v * 4)))] ?? "";
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));

// ---------------------------------------------------------------- Field

interface FieldView {
  el: HTMLElement;
  set(x: number, y: number, active: string | null): void;
}

/** A field: starting points as markers (click sets them), a point to drag. */
function fieldView(f: FieldSpec, pick: (name: string) => void, move: (x: number, y: number, phase: "start" | "move" | "end") => void): FieldView {
  const dot = h("div", { class: "snd-dot", title: t("Drag") });
  const marks = f.presets.map((p) => {
    const b = h("button", { class: "snd-mark", title: t("Starting point \"{name}\" – only sets {field}", { name: t(p.name), field: f.name }) }, h("i"), h("span", {}, t(p.name)));
    b.dataset.name = p.name;
    b.dataset.edge = p.x > 0.86 ? "r" : p.x < 0.12 ? "l" : p.y > 0.9 ? "t" : "";
    b.style.left = `${p.x * 100}%`;
    b.style.top = `${(1 - p.y) * 100}%`;
    b.addEventListener("pointerdown", (e) => e.stopPropagation());
    b.addEventListener("click", () => pick(p.name));
    return b;
  });
  const plot = h("div", { class: "snd-plot" }, ...marks, dot);
  const words = h("span", { class: "muted" });
  const at = (e: PointerEvent): [number, number] => {
    const r = plot.getBoundingClientRect();
    return [clamp01((e.clientX - r.left) / r.width), clamp01(1 - (e.clientY - r.top) / r.height)];
  };
  let dragging = false;
  plot.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    plot.setPointerCapture(e.pointerId);
    dragging = true;
    const [x, y] = at(e);
    view.set(x, y, null);
    move(x, y, "start");
  });
  plot.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    const [x, y] = at(e);
    view.set(x, y, null);
    move(x, y, "move");
  });
  const end = (e: PointerEvent) => {
    if (!dragging) return;
    dragging = false;
    const [x, y] = at(e);
    move(x, y, "end");
  };
  plot.addEventListener("pointerup", end);
  plot.addEventListener("pointercancel", end);
  const el = h(
    "div",
    { class: "snd-field" },
    h("div", { class: "snd-field-head" }, h("b", {}, f.name), words),
    h("div", { class: "snd-field-body" },
      h("div", { class: "snd-ylab" }, `${f.y.words[0]} → ${f.y.words[3]}`),
      plot,
      h("div", { class: "snd-xwords" }, ...f.x.words.map((w) => h("span", {}, w)))),
  );
  el.dataset.field = f.key;
  const view: FieldView = {
    el,
    set(x, y, active) {
      dot.style.left = `${x * 100}%`;
      dot.style.top = `${(1 - y) * 100}%`;
      words.textContent = `${word(f.x.words, x)} · ${word(f.y.words, y)}`;
      for (const m of marks) m.classList.toggle("on", m.dataset.name === active);
    },
  };
  return view;
}

// ---------------------------------------------------------------- Time course

type Lane = "laut" | "hell";

/** Time axis, waveform and – for the synth – the drawn curves. */
function timeView(isSynth: boolean, edit: (lane: Lane, pts: CurvePoint[], merge: boolean, done: boolean) => void) {
  const canvas = h("canvas", { class: `snd-wave${isSynth ? " synth" : ""}`, title: isSynth ? t("Envelope of a note: dragging points changes volume (loud) and brightness (bright) over time – double-click adds or removes a point") : t("Waveform of a note with the current sound") }) as HTMLCanvasElement;
  let wave: WaveView | null = null;
  let curves: Record<Lane, CurvePoint[]> = { laut: [], hell: [] };
  let geo = { L: 34, R: 0, T: 1, lanes: {} as Record<Lane, [number, number]> };
  let drag: { lane: Lane; i: number; started: boolean } | null = null;

  const css = (n: string, d: string) => getComputedStyle(canvas).getPropertyValue(n).trim() || d;
  const xOf = (t: number) => geo.L + (t / geo.T) * (geo.R - geo.L);
  const tOf = (x: number) => ((x - geo.L) / (geo.R - geo.L)) * geo.T;
  const yOf = (lane: Lane, v: number) => {
    const [top, bot] = geo.lanes[lane];
    return bot - v * (bot - top);
  };
  const vOf = (lane: Lane, y: number) => {
    const [top, bot] = geo.lanes[lane];
    return clamp01((bot - y) / (bot - top));
  };

  const draw = () => {
    const { ctx, w, h: hh } = fitCanvas(canvas);
    ctx.clearRect(0, 0, w, hh);
    const lastCurve = Math.max(curves.laut.at(-1)?.[0] ?? 0, curves.hell.at(-1)?.[0] ?? 0);
    geo.T = Math.max(wave?.secs ?? 1, isSynth ? lastCurve + 0.6 : 0.2, 0.2);
    geo.R = w - 8;
    const axis = hh - 16;
    geo.lanes = isSynth ? { laut: [8, axis * 0.6], hell: [axis * 0.6 + 10, axis - 4] } : { laut: [6, axis - 4], hell: [6, axis - 4] };
    const line = css("--line-2", "#2a3140");
    const muted = css("--muted", "#8a93a6");
    ctx.font = "10px system-ui";
    ctx.textAlign = "center";
    const step = geo.T > 6 ? 1 : geo.T > 2.5 ? 0.5 : geo.T > 1 ? 0.25 : 0.1;
    for (let t = 0; t <= geo.T + 1e-6; t += step) {
      ctx.fillStyle = line;
      ctx.fillRect(Math.round(xOf(t)), 4, 1, axis - 4);
      ctx.fillStyle = muted;
      ctx.fillText(geo.T > 1.2 ? `${t.toLocaleString(language(), { maximumFractionDigits: 2 })} s` : `${Math.round(t * 1000)} ms`, xOf(t), hh - 4);
    }
    // waveform (in the volume lane)
    if (wave?.peaks.length) {
      const [top, bot] = geo.lanes.laut;
      const mid = (top + bot) / 2;
      const amp = (bot - top) / 2;
      const peak = Math.max(0.05, ...wave.peaks.map((p) => Math.max(-p[0], p[1])));
      ctx.fillStyle = isSynth ? "rgba(255, 180, 84, 0.28)" : "rgba(255, 180, 84, 0.85)";
      const per = wave.secs / wave.peaks.length;
      wave.peaks.forEach(([lo, hi], i) => {
        const x0 = xOf(i * per);
        const x1 = xOf((i + 1) * per);
        ctx.fillRect(x0, mid - (hi / peak) * amp, Math.max(1, x1 - x0), Math.max(1, ((hi - lo) / peak) * amp));
      });
      if (isSynth && wave.held < geo.T) {
        ctx.strokeStyle = muted;
        ctx.setLineDash([3, 3]);
        ctx.beginPath();
        ctx.moveTo(xOf(wave.held) + 0.5, 4);
        ctx.lineTo(xOf(wave.held) + 0.5, axis);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.fillStyle = muted;
        ctx.textAlign = "left";
        ctx.fillText(t("release"), xOf(wave.held) + 4, 14);
      }
    }
    if (!isSynth) return;
    for (const [lane, color, label] of [["laut", "#ffb454", t("Volume")], ["hell", "#6ee7d8", t("Brightness")]] as const) {
      const pts = curves[lane];
      const [top, bot] = geo.lanes[lane];
      ctx.fillStyle = "rgba(255,255,255,0.025)";
      ctx.fillRect(geo.L, top, geo.R - geo.L, bot - top);
      ctx.fillStyle = muted;
      ctx.textAlign = "left";
      ctx.fillText(label, geo.L + 4, top + 11);
      if (!pts.length) continue;
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.beginPath();
      pts.forEach(([t, v], i) => (i ? ctx.lineTo(xOf(t), yOf(lane, v)) : ctx.moveTo(xOf(t), yOf(lane, v))));
      ctx.lineTo(geo.R, yOf(lane, pts.at(-1)![1]));
      ctx.stroke();
      ctx.fillStyle = color;
      for (const [t, v] of pts) {
        ctx.beginPath();
        ctx.arc(xOf(t), yOf(lane, v), 4.5, 0, Math.PI * 2);
        ctx.fill();
      }
    }
  };

  const local = (e: MouseEvent): [number, number] => {
    const r = canvas.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  };
  const hit = (x: number, y: number): { lane: Lane; i: number } | null => {
    let best: { lane: Lane; i: number; d: number } | null = null;
    for (const lane of ["laut", "hell"] as const) {
      curves[lane].forEach(([t, v], i) => {
        const d = Math.hypot(xOf(t) - x, yOf(lane, v) - y);
        if (d < 10 && (!best || d < best.d)) best = { lane, i, d };
      });
    }
    return best;
  };
  const laneAt = (y: number): Lane | null => {
    for (const lane of ["laut", "hell"] as const) {
      const [top, bot] = geo.lanes[lane];
      if (y >= top - 4 && y <= bot + 4) return lane;
    }
    return null;
  };
  if (isSynth) {
    canvas.addEventListener("pointerdown", (e) => {
      const [x, y] = local(e);
      const target = hit(x, y);
      if (!target) return;
      canvas.setPointerCapture(e.pointerId);
      drag = { ...target, started: false };
    });
    canvas.addEventListener("pointermove", (e) => {
      const [x, y] = local(e);
      if (!drag) {
        canvas.style.cursor = hit(x, y) ? "grab" : laneAt(y) ? "copy" : "default";
        return;
      }
      const pts = curves[drag.lane].map((p) => [...p] as CurvePoint);
      const prev = pts[drag.i - 1];
      const next = pts[drag.i + 1];
      const t = drag.i === 0 ? 0 : Math.min(next ? next[0] - 0.01 : 12, Math.max(prev ? prev[0] + 0.01 : 0, tOf(x)));
      pts[drag.i] = [Math.round(t * 1000) / 1000, Math.round(vOf(drag.lane, y) * 1000) / 1000];
      curves = { ...curves, [drag.lane]: pts };
      draw();
      edit(drag.lane, pts, drag.started, false);
      drag.started = true;
    });
    const up = () => {
      if (drag?.started) edit(drag.lane, curves[drag.lane], true, true);
      drag = null;
    };
    canvas.addEventListener("pointerup", up);
    canvas.addEventListener("pointercancel", up);
    // double-click: add a point or (on a point) remove it
    canvas.addEventListener("dblclick", (e) => {
      const [x, y] = local(e);
      const target = hit(x, y);
      if (target) {
        const pts = curves[target.lane];
        if (target.i === 0 || pts.length <= 2) return;
        const next = pts.filter((_, i) => i !== target.i);
        curves = { ...curves, [target.lane]: next };
        edit(target.lane, next, false, true);
      } else {
        const lane = laneAt(y);
        if (!lane) return;
        const t = Math.max(0.01, tOf(x));
        const next = [...curves[lane], [Math.round(t * 1000) / 1000, Math.round(vOf(lane, y) * 1000) / 1000] as CurvePoint].sort((a, b) => a[0] - b[0]);
        if (next.length > 24) return;
        curves = { ...curves, [lane]: next };
        edit(lane, next, false, true);
      }
      draw();
    });
  }
  return {
    el: canvas,
    setWave(w: WaveView) {
      wave = w;
      draw();
    },
    setCurves(c: Record<Lane, CurvePoint[]>) {
      if (drag) return;
      curves = c;
      draw();
    },
    draw,
  };
}

// ---------------------------------------------------------------- My sounds

/** The same sound? All sliders (missing = default value), for the synth also the curves. */
function sameSound(spec: InstrumentSpec, a: Instrument, b: Instrument): boolean {
  if (a.kind !== b.kind) return false;
  for (const p of spec.params) {
    if (Math.abs((a.params[p.key] ?? p.default) - (b.params[p.key] ?? p.default)) > 1e-4) return false;
  }
  if (spec.kind !== "synth") return true;
  const v = spec.verlauf[0];
  const c = (i: Instrument, k: Lane) => JSON.stringify(i.curves?.[k] ?? v?.[k] ?? []);
  return c(a, "laut") === c(b, "laut") && c(a, "hell") === c(b, "hell");
}

/**
 * Sound library in the Sound window: save the current setting under a
 * name (same name for the same instrument = replace), load saved
 * sounds, rename, delete. What is currently set is highlighted.
 */
function soundLibrary(spec: InstrumentSpec, trackId: number, loaded: (s: SavedSound) => void) {
  const nameInput = h("input", { type: "text", class: "snd-lib-name", maxLength: 60, title: t("Sound name") }) as HTMLInputElement;
  nameInput.spellcheck = false;
  const saveBtn = h("button", { class: "btn small primary snd-lib-save" }) as HTMLButtonElement;
  const list = h("div", { class: "snd-lib-list" });
  const others = h("details", { class: "snd-lib-others" }) as HTMLDetailsElement;
  let othersOpen = false;
  others.addEventListener("toggle", () => (othersOpen = others.open));
  let inst: Instrument | null = null;
  let key = "";
  let editing = false;

  const clean = (n: string) => n.trim().replace(/\s+/g, " ");
  const mine = () => (savedSounds.peek() ?? []).filter((s) => s.kind === spec.kind);
  const existing = (name: string) => mine().find((s) => s.name.toLowerCase() === clean(name).toLowerCase());
  /** Suggestion: the chosen character starting point, otherwise the instrument name – made unique with a number. */
  const suggestion = () => {
    const charakter = spec.fields.find((f) => f.key === "charakter");
    const base = inst && charakter?.presets.some((p) => p.name === inst!.preset) ? t(inst.preset) : spec.name;
    if (!existing(base)) return base;
    for (let n = 2; ; n++) if (!existing(`${base} ${n}`)) return `${base} ${n}`;
  };
  const refreshSave = () => {
    const name = clean(nameInput.value) || suggestion();
    const ex = existing(name);
    nameInput.placeholder = suggestion();
    saveBtn.replaceChildren(icon("save"), ex ? t("Replace") : t("Save sound"));
    saveBtn.title = ex ? t("Overwrite \"{name}\" with the current settings", { name: ex.name }) : t("Save the current settings as \"{name}\"", { name });
  };
  const save = async () => {
    const name = clean(nameInput.value) || suggestion();
    try {
      const r = await backend.soundSave(trackId, name);
      savedSounds.value = r.sounds;
      nameInput.value = "";
      refreshSave();
      notify(r.replaced ? t("Sound \"{name}\" replaced", { name: r.name }) : t("Sound \"{name}\" saved", { name: r.name }));
    } catch (e) {
      notify(errorText(e), "error");
    }
  };
  saveBtn.addEventListener("click", () => void save());
  nameInput.addEventListener("input", refreshSave);
  nameInput.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") void save();
    if (e.key === "Escape") {
      nameInput.value = "";
      refreshSave();
      nameInput.blur();
    }
  });

  const row = (s: SavedSound, active: boolean, other: boolean) => {
    const label = h(
      "button",
      { class: "snd-lib-load", title: other ? t("Load \"{name}\" – the track becomes {kind}", { name: s.name, kind: s.kindName }) : t("Load \"{name}\"", { name: s.name }) },
      h("b", {}, s.name),
      h("span", {}, other ? `${s.kindName} · ${s.summary}` : s.summary),
    );
    label.addEventListener("click", () => {
      void act(backend.soundLoad(trackId, s.id)).then((ok) => ok && loaded(s));
    });
    const ren = h("button", { class: "btn ghost small snd-lib-ren", title: t("Rename") }, icon("pencil"));
    ren.addEventListener("click", () => {
      editing = true;
      const input = h("input", { type: "text", class: "snd-lib-rename", maxLength: 60 }) as HTMLInputElement;
      input.value = s.name;
      input.spellcheck = false;
      let done = false;
      const finish = (commit: boolean) => {
        if (done) return;
        done = true;
        editing = false;
        const v = clean(input.value);
        if (!commit || !v || v === s.name) return render(true);
        backend.soundRename(s.id, v).then((l) => (savedSounds.value = l)).catch((e) => {
          notify(errorText(e), "error");
          render(true);
        });
      };
      input.addEventListener("keydown", (e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(true);
        if (e.key === "Escape") finish(false);
      });
      input.addEventListener("blur", () => finish(true));
      label.replaceWith(input);
      input.focus();
      input.select();
    });
    // delete with a second click (as with the note libraries)
    let armed = false;
    const del = h("button", { class: "btn ghost small snd-lib-del", title: t("Delete from \"My sounds\"") }, icon("trash"));
    del.addEventListener("click", () => {
      if (!armed) {
        armed = true;
        del.classList.add("warn");
        del.replaceChildren(icon("trash"), t("Delete?"));
        setTimeout(() => {
          armed = false;
          del.classList.remove("warn");
          del.replaceChildren(icon("trash"));
        }, 4000);
        return;
      }
      backend.soundDelete(s.id).then((l) => {
        savedSounds.value = l;
        notify(t("Sound \"{name}\" deleted", { name: s.name }));
      }).catch((e) => notify(errorText(e), "error"));
    });
    const el = h("div", { class: `snd-lib-item${active ? " on" : ""}` }, label, ren, del);
    el.dataset.id = s.id;
    el.dataset.name = s.name;
    return el;
  };

  const render = (force = false) => {
    const all = savedSounds.peek();
    const own = mine();
    const active = new Set(inst ? own.filter((s) => sameSound(spec, s.instrument, inst!)).map((s) => s.id) : []);
    const k = JSON.stringify([all?.map((s) => [s.id, s.name, s.summary, s.modified]), [...active]]);
    if (editing || (!force && k === key)) return;
    key = k;
    const rest = (all ?? []).filter((s) => s.kind !== spec.kind);
    list.replaceChildren(
      ...(own.length
        ? own.map((s) => row(s, active.has(s.id), false))
        : [h("div", { class: "snd-lib-empty" }, all === null ? t("Loading…") : t("No sounds of your own for {instrument} yet – enter a name and save.", { instrument: spec.name }))]),
    );
    others.hidden = rest.length === 0;
    others.replaceChildren(h("summary", {}, t("Other instruments ({n})", { n: rest.length })), ...rest.map((s) => row(s, false, true)));
    others.open = othersOpen;
    // Summary for the collapsed state: count and the currently set sound
    const on = own.find((s) => active.has(s.id));
    badge.textContent = all === null ? "" : `${own.length}${on ? ` · ${on.name}` : ""}`;
    badge.title = on
      ? t("Current: \"{name}\"", { name: on.name })
      : plural(own.length, "{n} custom sound for {instrument}", "{n} custom sounds for {instrument}", { instrument: spec.name });
    refreshSave();
  };

  const badge = h("span", { class: "snd-lib-badge" });
  const el = h("div", { class: "snd-lib" }, h("div", { class: "snd-lib-saverow" }, nameInput, saveBtn), list, others);
  return {
    el,
    badge,
    update(i: Instrument) {
      inst = i;
      render();
      refreshSave(); // suggestion follows the character starting point
    },
  };
}

// ---------------------------------------------------------------- Window

/**
 * What changed from `old` to `next` (another instrument, parameters, curves, starting point),
 * applied to `other`: a picked track follows the change and keeps everything else.
 */
export function followChange(old: Instrument, next: Instrument, other: Instrument): Instrument {
  if (next.kind !== old.kind) return next;
  const params = { ...other.params };
  for (const [k, v] of Object.entries(next.params)) if (old.params[k] !== v) params[k] = v;
  const curves = { ...(other.curves ?? {}) };
  for (const [k, v] of Object.entries(next.curves ?? {})) if (JSON.stringify(old.curves?.[k]) !== JSON.stringify(v)) curves[k] = v;
  return { ...other, preset: next.preset !== old.preset ? next.preset : other.preset, params, ...(Object.keys(curves).length ? { curves } : {}) };
}

export function soundPanel(trackId: number): SoundPanel {
  const spec = specFor(findTrack(project(), trackId)?.instrument);
  if (!spec) return { el: h("div", { class: "placeholder" }, t("Loading instruments… (sound design is only available in the desktop app)")), update() {} };
  const isSynth = spec.kind === "synth";
  const prefs = viewPrefs();
  const current = (): Instrument | null => findTrack(project(), trackId)?.instrument ?? null;
  // Several tracks picked (Ctrl/Shift+click in the track list): those with the same instrument
  // follow every change – each keeps what wasn't changed. One undo step.
  const others = (): number[] => pickedTracks().filter((tr) => tr.id !== trackId && tr.instrument?.kind === spec.kind).map((tr) => tr.id);
  const setInstrument = (inst: Instrument, merge = false) => {
    const old = current();
    const more = old ? others() : [];
    if (!more.length) return run({ type: "setTrack", trackId, patch: { instrument: inst } }, merge);
    const commands: Command[] = [{ type: "setTrack", trackId, patch: { instrument: inst } }];
    for (const id of more) {
      const o = findTrack(project(), id)?.instrument;
      if (o) commands.push({ type: "setTrack", trackId: id, patch: { instrument: followChange(old!, inst, o) } });
    }
    return run({ type: "batch", label: t("Change sound"), commands }, merge);
  };
  const base = (): Instrument => {
    const cur = current();
    return { kind: spec.kind, preset: cur?.preset ?? "standard", params: { ...defaultParams(spec), ...(cur?.params ?? {}) }, ...(cur?.curves ? { curves: cur.curves } : {}) };
  };
  const setParams = (params: Record<string, number>, preset: string, merge: boolean) => setInstrument({ ...base(), preset, params: { ...base().params, ...params } }, merge);
  const preview = (pattern: PreviewPattern) => backend.previewTrack(trackId, pattern).catch((e) => notify(errorText(e), "error"));
  // Auditioning after a change: off · single note · pattern (remembered)
  let playMode: PlayMode = prefs.anspielen;
  const autoPreview = (s: InstrumentSpec = spec) => {
    const p = playPattern(s, playMode);
    if (p) void preview(p);
  };

  // Instrument selection
  const kinds = h(
    "div",
    { class: "seg snd-kinds" },
    ...instrumentSpecs.peek().map((s) =>
      h("button", {
        class: `seg-btn${s.kind === spec.kind ? " on" : ""}`,
        title: s.description,
        onclick: () => s.kind !== spec.kind && setInstrument(defaultInstrument(s)),
      }, s.name),
    ),
  );
  const listen = h(
    "div",
    { class: "presets" },
    ...spec.previews.map((pv, i) =>
      h("button", { class: `btn small${i === 0 ? " primary" : ""}`, title: t("Play the sound with \"{label}\"", { label: pv.label }), onclick: () => preview(pv.key) }, i === 0 ? icon("play") : null, pv.label),
    ),
  );
  const insert = h("button", { class: "btn small", title: t("Insert one bar of an example pattern after the last clip"), onclick: () => act(backend.insertPattern(trackId)) }, icon("plus"), t("Insert pattern"));

  // Fields (character always, timbre and room switchable)
  let dragging: string | null = null;
  let pending: { field: string; x: number; y: number } | null = null;
  let fresh = false;
  let inflight = false;
  let playAfter = false;
  const pump = async () => {
    if (inflight || !pending) return;
    const p = pending;
    pending = null;
    const merge = !fresh;
    fresh = false;
    inflight = true;
    await act(backend.soundField(trackId, p.field, p.x, p.y, null, merge, others()));
    inflight = false;
    if (pending) void pump();
    else if (playAfter) {
      playAfter = false;
      autoPreview();
    }
  };
  const fieldViews = spec.fields.map((f) => {
    const view = fieldView(
      f,
      (name) => {
        void act(backend.soundField(trackId, f.key, null, null, name, false, others())).then((ok) => {
          if (ok) autoPreview();
        });
      },
      (x, y, phase) => {
        if (phase === "start") {
          dragging = f.key;
          fresh = true;
        }
        pending = { field: f.key, x, y };
        if (phase === "end") {
          dragging = null;
          playAfter = true;
        }
        void pump();
      },
    );
    return { f, view };
  });
  const fieldBox = h("div", { class: "snd-fields" }, ...fieldViews.map((v) => v.view.el));
  /** Side length of the fields: equally wide side by side, but never taller than the visible space. */
  const fitPads = () => {
    const body = fieldBox.closest(".tab-body") as HTMLElement | null;
    const shown = fieldViews.filter((v) => !v.view.el.hidden).length;
    if (!body || !shown || !fieldBox.clientWidth) return;
    const GAP = 14;
    const YLAB = 20; // vertical label left of the field
    const perW = (fieldBox.clientWidth - GAP * (shown - 1)) / shown - YLAB;
    const top = fieldBox.getBoundingClientRect().top - body.getBoundingClientRect().top + body.scrollTop;
    const perH = body.clientHeight - top - 22 - 18 - 14; // header row, axis words, margin
    const size = Math.round(Math.max(120, Math.min(perW, perH, 640)));
    if (fieldBox.style.getPropertyValue("--pad") !== `${size}px`) fieldBox.style.setProperty("--pad", `${size}px`);
  };
  const toggles = (["klangfarbe", "raum"] as const).map((key) => {
    const name = spec.fields.find((f) => f.key === key)?.name ?? key;
    const b = h("button", { class: `btn small snd-toggle${prefs[key] ? " on" : ""}`, title: t("Show or hide the \"{name}\" field (the setting is kept)", { name }) }, name);
    b.dataset.field = key;
    const apply = (on: boolean) => {
      b.classList.toggle("on", on);
      const fv = fieldViews.find((v) => v.f.key === key);
      if (fv) fv.view.el.hidden = !on;
      fitPads();
    };
    b.addEventListener("click", () => {
      const on = !b.classList.contains("on");
      setViewPref(key, on);
      apply(on);
    });
    apply(prefs[key]);
    return b;
  });
  const single = spec.kind === "drums" ? t("Single hit") : t("Single note");
  const patternLabel = spec.previews[0]?.label ?? t("Pattern");
  const playBtns = ([
    ["aus", t("off"), t("Don't play anything after changes")],
    ["ton", single, spec.kind === "drums" ? t("Play a kick hit after releasing") : t("Play a single note after releasing")],
    ["muster", patternLabel, t("Play \"{label}\" after releasing", { label: patternLabel })],
  ] as const).map(([mode, label, title]) => {
    const b = h("button", { class: `seg-btn${mode === playMode ? " on" : ""}`, title }, label);
    b.dataset.mode = mode;
    b.addEventListener("click", () => {
      playMode = mode;
      setViewPref("anspielen", mode);
      for (const x of playBtns) x.classList.toggle("on", x.dataset.mode === mode);
      autoPreview();
    });
    return b;
  });
  const playSwitch = h("div", { class: "seg mini snd-play" }, ...playBtns);

  // Time course
  const time = timeView(isSynth, (lane, pts, merge) => {
    const cur = base();
    void setInstrument({ ...cur, preset: cur.preset, curves: { ...(cur.curves ?? {}), [lane]: pts } }, merge);
  });
  const testton = h("span", { class: "muted" });
  let verlaufBox: HTMLElement | null = null;
  const vchips: Array<{ v: VerlaufSpec; b: HTMLElement }> = [];
  const pulsBtns: HTMLElement[] = [];
  const pulsDepth = h("input", { type: "range", min: 0, max: 1, step: 0.01, title: t("How far the pulse lowers the volume") }) as HTMLInputElement;
  const release = h("input", { type: "range", min: 0.05, max: 6, step: 0.05, title: t("Ring-out after the note is released") }) as HTMLInputElement;
  const releaseVal = h("b");
  if (isSynth) {
    for (const v of spec.verlauf) {
      const b = h("button", { class: "btn small snd-vchip", title: v.beschreibung, onclick: () => {
        const cur = base();
        void setInstrument({ ...cur, curves: { ...(cur.curves ?? {}), laut: v.laut, hell: v.hell },
          params: { ...cur.params, "verlauf.ausklang": v.ausklang, "verlauf.puls": v.puls, "verlauf.pulstiefe": v.pulstiefe } }).then((ok) => {
          if (ok) autoPreview();
        });
      } }, t(v.name));
      vchips.push({ v, b });
    }
    [[t("off"), 0], ["¼", 1], ["⅛", 2], ["1/16", 3]].forEach(([label, n]) => {
      const b = h("button", { class: "seg-btn", title: n ? t("Pulse in {label} notes, in time with the beat", { label: label as string }) : t("No pulse"), onclick: () => {
        const cur = base().params;
        const depth = n && (cur["verlauf.pulstiefe"] ?? 0) < 0.05 ? 0.8 : cur["verlauf.pulstiefe"] ?? 0;
        void setParams({ "verlauf.puls": n as number, "verlauf.pulstiefe": depth }, base().preset, false);
      } }, label as string);
      pulsBtns.push(b);
    });
    let merging = false;
    for (const [input, key] of [[pulsDepth, "verlauf.pulstiefe"], [release, "verlauf.ausklang"]] as const) {
      input.addEventListener("pointerdown", () => (merging = false));
      input.addEventListener("input", () => {
        void setParams({ [key]: Number(input.value) }, base().preset, merging);
        merging = true;
      });
      input.addEventListener("change", () => (merging = false));
      input.addEventListener("keydown", (e) => e.stopPropagation());
    }
    verlaufBox = h(
      "div",
      { class: "snd-verlauf" },
      h("div", { class: "snd-vchips" }, h("span", { class: "muted" }, t("Envelope:")), ...vchips.map((c) => c.b)),
      h("div", { class: "snd-vctl" },
        h("label", {}, t("Pulse")), h("div", { class: "seg" }, ...pulsBtns),
        h("label", {}, t("Depth")), pulsDepth,
        h("label", {}, t("Release")), release, releaseVal),
      h("div", { class: "hint" }, t("Drag points · double-click adds or removes a point")),
    );
  }

  // Drums: how the keys play – drum kit (a drum per drum key) or every key by pitch
  const keysBtns = spec.kind === "drums"
    ? ([
        [0, t("Drum kit"), t("Each drum key plays its drum (C2 kick drum, D2 snare, F♯2 hi-hat …); every other key plays a tuned drum")],
        [1, t("Pitch"), t("Every key plays a tuned drum at its pitch – for melodies and bass lines")],
      ] as const).map(([v, label, title]) => {
        const b = h("button", { class: "seg-btn", title }, label);
        b.dataset.keys = String(v);
        b.addEventListener("click", () => {
          if (b.classList.contains("on")) return;
          void setParams({ [KEYS_MODE]: v }, base().preset, false).then((ok) => {
            if (!ok) return;
            refitPitch(trackId); // the track shows its drums or its notes
            autoPreview();
          });
        });
        return b;
      })
    : [];
  const keysHint = h("div", { class: "hint snd-keys-hint" });
  const keysBox = keysBtns.length ? h("div", { class: "prop snd-keys" }, h("label", {}, t("Keys")), h("div", { class: "seg mini" }, ...keysBtns), keysHint) : null;

  // Fine-tuning: all sliders
  const sliders: Array<{ p: ParamSpec; input: HTMLInputElement; val: HTMLElement; row: HTMLElement }> = [];
  const makeSlider = (p: ParamSpec) => {
    const input = h("input", { type: "range", min: p.min, max: p.max, step: p.note ? 1 : 0.01 }) as HTMLInputElement;
    const val = h("b");
    let merging = false;
    input.addEventListener("pointerdown", () => (merging = false));
    input.addEventListener("input", () => {
      void setParams({ [p.key]: Number(input.value) }, "eigen", merging);
      merging = true;
    });
    input.addEventListener("change", () => {
      merging = false;
      const pv = GROUP_PREVIEW[p.group];
      if (spec.kind === "drums" && pv) setTimeout(() => void preview(pv), 120);
    });
    input.addEventListener("keydown", (e) => e.stopPropagation());
    const row = h(
      "div",
      { class: "slider", title: p.meaning },
      h("label", {}, h("span", {}, p.label), val),
      input,
      h("div", { class: "scale" }, h("span", {}, p.note ? noteName(p.min) : p.left), h("span", {}, p.note ? noteName(p.max) : p.right)),
    );
    sliders.push({ p, input, val, row });
    return row;
  };
  const groups = new Map<string, ParamSpec[]>();
  for (const p of spec.params.filter((p) => !p.hidden)) groups.set(p.group, [...(groups.get(p.group) ?? []), p]);
  const groupEls = [...groups].map(([name, ps]) => {
    const pv = spec.kind === "drums" ? GROUP_PREVIEW[name] : undefined;
    const head = h("h4", {}, t(name), pv ? h("button", { class: "btn ghost small", title: t("Play {name}", { name: t(name) }), onclick: () => preview(pv) }, icon("play")) : null);
    const g = h("div", { class: "pgroup" }, head, h("div", { class: "sliders" }, ...ps.map(makeSlider)));
    g.dataset.group = name;
    return g;
  });
  const fine = h("details", { class: "snd-fine", open: prefs.feinschliff }, h("summary", {}, t("Fine-tuning – all controls")), ...groupEls) as HTMLDetailsElement;
  fine.addEventListener("toggle", () => setViewPref("feinschliff", fine.open));

  // Measurements – the agent sees the same numbers and words.
  const measures = h("div", { class: "measures" });
  const measureTon = h("span", { class: "muted" });
  let lastKey = "";
  let timer: number | undefined;
  const refresh = () => {
    const inst = current();
    const key = JSON.stringify(inst ?? {});
    if (!inst || key === lastKey) return;
    lastKey = key;
    clearTimeout(timer);
    timer = window.setTimeout(async () => {
      try {
        const [r, w] = await Promise.all([backend.describeSound(trackId), backend.soundWave(trackId)]);
        measureTon.textContent = r.testton;
        testton.textContent = w.testton;
        time.setWave(w);
        measures.replaceChildren(
          ...r.anzeige.map((m) => {
            const fill = h("i");
            fill.style.width = `${Math.round(Math.max(0.03, Math.min(1, m.wert)) * 100)}%`;
            return h("div", { class: "measure" }, h("span", { class: "name" }, m.label), h("b", {}, m.wort), h("div", { class: "bar" }, fill), h("span", { class: "detail" }, m.detail));
          }),
        );
      } catch {
        /* measurement is optional */
      }
    }, 180);
  };

  // My sounds (sound library across all projects)
  const lib = soundLibrary(spec, trackId, (s) => autoPreview(instrumentSpecs.peek().find((x) => x.kind === s.kind) ?? spec));

  // My sounds collapsible (closed at first); collapsed, the row shows the count and the current sound
  const libBox = h("details", { class: "prop snd-lib-box", open: prefs.klaenge },
    h("summary", { title: t("Your own sounds – available in every project") }, h("span", { class: "snd-lib-title" }, t("My sounds")), lib.badge),
    lib.el) as HTMLDetailsElement;
  libBox.addEventListener("toggle", () => setViewPref("klaenge", libBox.open));

  // Time course collapsible
  const timeBox = h("details", { class: "snd-time", open: prefs.zeitverlauf },
    h("summary", { class: "snd-time-head" }, h("b", {}, t("Over time")), testton),
    time.el,
    verlaufBox) as HTMLDetailsElement;
  timeBox.addEventListener("toggle", () => {
    setViewPref("zeitverlauf", timeBox.open);
    fitPads();
  });

  const agentNote = h("div", { class: "agent-note" });
  // several tracks picked: which ones follow the changes
  const multiNote = h("div", { class: "snd-multi" });
  const refreshMulti = () => {
    const names = others().map((id) => findTrack(project(), id)?.name ?? "");
    multiNote.hidden = !names.length;
    const self = findTrack(project(), trackId)?.name ?? "";
    multiNote.textContent = names.length
      ? plural(names.length + 1, "Changes apply to {n} track: {names}", "Changes apply to {n} tracks: {names}", { names: [self, ...names].join(", ") })
      : "";
  };
  const el = h(
    "div",
    { class: "sound snd" },
    h("div", { class: "snd-main" },
      h("div", { class: "snd-head" },
        kinds,
        h("div", { class: "snd-head-right" },
          h("span", { class: "muted" }, t("Fields:")), ...toggles,
          h("span", { class: "muted snd-play-label", title: t("What plays briefly after a change") }, t("play:")), playSwitch)),
      multiNote,
      fieldBox,
      timeBox,
      fine),
    h("div", { class: "sound-side" },
      libBox,
      keysBox,
      h("div", { class: "prop" }, h("label", {}, t("Preview")), listen),
      h("div", { class: "prop snd-insert" }, insert,
        spec.kind === "drums" ? h("button", { class: "btn small", title: t("Genre patterns and step grid in the \"Rhythm\" window"), onclick: () => showPanel("rhythmus") }, icon("grid"), t("Genre patterns…")) : null),
      h("div", { class: "prop" }, h("label", {}, t("Measured"), " · ", measureTon), measures),
      agentNote),
  );
  new ResizeObserver(() => time.draw()).observe(time.el);
  // Fields grow with the space: as large as the window's width and visible height allow
  let bodySeen = false;
  const padObserver = new ResizeObserver(() => {
    if (!el.isConnected) {
      if (bodySeen) padObserver.disconnect();
      return;
    }
    const body = el.closest(".tab-body");
    if (body && !bodySeen) {
      bodySeen = true;
      padObserver.observe(body);
    }
    fitPads();
  });
  padObserver.observe(fieldBox);
  // (the pick changes without the project changing; the effect ends with the panel)
  const stopMulti = effect(() => {
    picked.value;
    if (!el.isConnected && multiNote.dataset.seen) return stopMulti?.();
    if (el.isConnected) multiNote.dataset.seen = "1";
    refreshMulti();
  });

  return {
    el,
    update() {
      refreshMulti();
      const inst = current();
      if (!inst) return;
      const color = findTrack(project(), trackId)?.color ?? "#ffb454";
      el.style.setProperty("--c", color);
      const prm = (k: string, d = 0.5) => inst.params[k] ?? spec.params.find((p) => p.key === k)?.default ?? d;
      for (const { f, view } of fieldViews) {
        if (dragging === f.key) continue;
        const x = prm(f.x.param);
        const y = prm(f.y.param);
        const active = f.key === "charakter"
          ? (f.presets.some((p) => p.name === inst.preset) ? inst.preset : null)
          : f.presets.find((p) => Math.abs(p.x - x) < 0.005 && Math.abs(p.y - y) < 0.005)?.name ?? null;
        view.set(x, y, active);
      }
      // By pitch the note gives the pitch, and there are no claps and hi-hats: those controls rest.
      const byPitch = spec.kind === "drums" && prm(KEYS_MODE, 0) >= 0.5;
      for (const b of keysBtns) b.classList.toggle("on", Number(b.dataset.keys) === (byPitch ? 1 : 0));
      keysHint.textContent = byPitch
        ? t("Every key is a tuned drum: below C3 kick drum, C3 to B4 tom, from C5 snare.")
        : t("The drum keys play the kit; every other key plays a tuned drum (below C3 kick drum, C3 to B4 tom, from C5 snare).");
      for (const g of groupEls) {
        const idle = byPitch && (g.dataset.group === "Clap" || g.dataset.group === "Hi-Hat");
        g.classList.toggle("idle", idle);
        g.title = idle ? t("By pitch there are no claps and hi-hats") : "";
      }
      for (const s of sliders) {
        s.row.classList.toggle("idle", byPitch && s.p.note);
        if (byPitch && s.p.note) s.row.title = t("By pitch the note sets the pitch");
        else s.row.title = s.p.meaning;
        const v = inst.params[s.p.key] ?? s.p.default;
        s.input.style.setProperty("--c", color);
        setRange(s.input, v);
        s.val.textContent = s.p.note ? noteName(Math.round(v)) : `${Math.round(((v - s.p.min) / (s.p.max - s.p.min)) * 100)}`;
      }
      if (isSynth) {
        const laut = inst.curves?.laut ?? spec.verlauf[0]?.laut ?? [];
        const hell = inst.curves?.hell ?? spec.verlauf[0]?.hell ?? [];
        time.setCurves({ laut, hell });
        const puls = Math.round(prm("verlauf.puls", 0));
        pulsBtns.forEach((b, i) => b.classList.toggle("on", i === puls));
        setRange(pulsDepth, prm("verlauf.pulstiefe", 0));
        pulsDepth.disabled = puls === 0;
        setRange(release, prm("verlauf.ausklang", 1.6));
        releaseVal.textContent = `${prm("verlauf.ausklang", 1.6).toLocaleString(language(), { maximumFractionDigits: 2 })} s`;
        const same = (a: CurvePoint[], b: CurvePoint[]) => JSON.stringify(a) === JSON.stringify(b);
        for (const { v, b } of vchips) {
          b.classList.toggle("on", same(laut, v.laut) && same(hell, v.hell) && Math.round(prm("verlauf.puls", 0)) === v.puls);
        }
      }
      const url = snapshot.peek()?.agent.url;
      agentNote.textContent = url
        ? t("Agent: e.g. \"Make the kick bigger\", \"pad darker and rougher\", \"let the synth breathe\" or \"save this as My Beat\" – in the agent chat or via Claude Code ({url}).", { url })
        : t("Agent interface (MCP) is not active.");
      lib.update(inst);
      refresh();
    },
  };
}
