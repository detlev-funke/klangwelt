import { analyzeSelected, convertAudio, detectTempo, melodyToTrack } from "../actions.js";
import { fitCanvas, formatBarBeat, h, icon, noteName, setRange } from "../dom.js";
import { highOnTop, noteKey, noteSel } from "../pianoroll.js";
import { plural, t } from "../i18n.js";
import { effect } from "../signals.js";
import { DYNAMICS, EXPRS, EXPR_ORDER, type ExprKey, type Mark, applyMark, exprKeysFor, exprValue, setExprAll } from "../expression.js";
import {
  analysis, chat, clipLengthBeats, findClip, findTrack, instrumentSpecs,
  project, run, savedSounds, selection, snapshot,
} from "../store.js";
import { chatPanel } from "./chat.js";
import { fuguePanel } from "./fugue.js";
import { harmonyPanel } from "./harmony.js";
import { rhythmPanel } from "./rhythm.js";
import { songPanel } from "./song.js";
import { libraryPanel, librarySignals } from "./library.js";
import { soundPanel } from "./sound.js";
import type { Analysis, Note, Track } from "../types.js";

type Tab = "clip" | "analyse" | "klang" | "rhythmus" | "song" | "agent" | "bibliothek" | "harmonie" | "fuge";

interface Panel {
  el: HTMLElement;
  update(): void;
}

const placeholder = (text: string): Panel => ({ el: h("div", { class: "placeholder" }, text), update() {} });

function prop(label: string, value: Node | string, hint?: string): HTMLElement {
  return h(
    "div",
    { class: "prop" },
    h("label", {}, label),
    typeof value === "string" ? h("div", { class: "value" }, value) : value,
    hint ? h("div", { class: "hint" }, hint) : null,
  );
}

// ---------------------------------------------------------------- Expression

/**
 * Expression of the selected notes (or all notes of the clip): articulation marks
 * as buttons and, per form of expression, a slider that shows the mean value.
 */
function exprSection(trackId: number, clipId: number): Panel {
  const current = () => {
    const f = findClip(project(), { trackId, clipId });
    return f && f.clip.source.type === "notes" ? { track: f.track, notes: f.clip.source.notes } : null;
  };
  const selectedIdx = (notes: Note[]): number[] => {
    const sel = noteSel.peek();
    if (!sel || sel.clipId !== clipId || !sel.keys.length) return [];
    const keys = new Set(sel.keys);
    return notes.map((n, i) => (keys.has(noteKey(n)) ? i : -1)).filter((i) => i >= 0);
  };
  const targets = (notes: Note[]) => {
    const idx = selectedIdx(notes);
    return idx.length ? idx : notes.map((_, i) => i);
  };
  const apply = (fn: (notes: Note[], idx: number[]) => Note[], merge = false) => {
    const c = current();
    if (!c || !c.notes.length) return;
    void run({ type: "setClipNotes", trackId, clipId, notes: fn(c.notes, targets(c.notes)) }, merge);
  };

  const markButtons: Array<{ el: HTMLButtonElement; needs?: ExprKey }> = [];
  const mark = (label: string, title: string, m: Mark, cls = "", needs?: ExprKey) => {
    const el = h("button", { class: `btn small mark ${cls}`, title, onclick: () => apply((n, i) => applyMark(n, i, m)) }, label);
    markButtons.push({ el, needs });
    return el;
  };
  const marks = h(
    "div",
    { class: "marks-wrap" },
    h("div", { class: "marks" }, ...DYNAMICS.map(([name, v]) => mark(name, t("Dynamics {name}: velocity {velocity}", { name, velocity: v }), { kind: "dyn", velocity: v }, "dyn"))),
    h(
      "div",
      { class: "marks" },
      mark("cresc.", t("Crescendo: velocity rises by 40 from the first to the last note"), { kind: "cresc" }),
      mark("decresc.", t("Decrescendo: velocity falls by 40 from the first to the last note"), { kind: "decresc" }),
      mark(">", t("Accent: velocity +25"), { kind: "accent" }, "sym"),
      mark("·", t("Staccato: notes half as long"), { kind: "staccato" }, "sym"),
      mark("⌒", t("Legato: extend each note up to the next one (on bass: glide)"), { kind: "legato" }, "sym"),
    ),
    h(
      "div",
      { class: "marks" },
      mark(`〜 ${t("Vibrato")}`, t("Vibrato on/off (± 30 cents, delayed onset)"), { kind: "vibrato" }, "", "vibrato"),
      mark(t("Brighter"), t("Brightness +40%"), { kind: "brighter" }, "", "brightness"),
      mark(t("Darker"), t("Brightness −40%"), { kind: "darker" }, "", "brightness"),
      mark(`↗ ${t("Scoop")}`, t("Scoop into the note from 2 semitones below (on/off)"), { kind: "scoop" }, "", "bend"),
      mark(t("Neutral"), t("Reset expression: velocity 100, no vibrato, no bend, base sound"), { kind: "reset" }, "ghost"),
    ),
  );

  // Sliders: show the mean value of the target notes; dragging sets all of them to the value.
  const sliders = EXPR_ORDER.map((k) => {
    const d = EXPRS[k];
    const input = h("input", { type: "range", min: d.min, max: d.max, step: d.step });
    const val = h("b");
    let merging = false;
    input.addEventListener("pointerdown", () => (merging = false));
    input.addEventListener("input", () => {
      apply((n, i) => setExprAll(n, i, k, Number(input.value)), merging);
      merging = true;
      val.textContent = d.format(Number(input.value));
    });
    input.addEventListener("change", () => (merging = false));
    input.addEventListener("keydown", (e) => e.stopPropagation());
    input.addEventListener("dblclick", () => apply((n, i) => setExprAll(n, i, k, d.neutral)));
    const row = h("div", { class: "slider", title: `${d.hint} · ${t("Double-click: neutral")}` }, h("label", {}, h("span", {}, d.label), val), input);
    return { k, d, input, val, row };
  });

  const scope = h("span", { class: "muted" });
  const el = h(
    "div",
    { class: "expr-section" },
    h("div", { class: "prop" }, h("label", {}, `${t("Expression")} · `, scope), marks),
    h("div", { class: "sliders expr-sliders" }, ...sliders.map((s) => s.row)),
  );

  return {
    el,
    update() {
      const c = current();
      if (!c) return;
      const chosen = selectedIdx(c.notes);
      const idx = chosen.length ? chosen : c.notes.map((_, i) => i);
      scope.textContent = !c.notes.length
        ? t("no notes yet")
        : chosen.length
          ? plural(chosen.length, "for {n} selected note", "for {n} selected notes")
          : t("for all {n} notes (select notes to change only some of them)", { n: c.notes.length });
      const supported = exprKeysFor(c.track);
      for (const b of markButtons) b.el.hidden = !!b.needs && !supported.includes(b.needs);
      const color = c.track.color;
      for (const s of sliders) {
        s.row.hidden = !supported.includes(s.k);
        s.input.style.setProperty("--c", color);
        if (document.activeElement === s.input || !idx.length) continue;
        const vals = idx.map((i) => exprValue(c.notes[i]!, s.k));
        const avg = vals.reduce((a, b) => a + b, 0) / vals.length;
        const mixed = Math.max(...vals) - Math.min(...vals) > s.d.step * 1.5;
        setRange(s.input, avg);
        s.val.textContent = `${s.d.format(avg)}${mixed ? ` · ${t("mixed")}` : ""}`;
      }
    },
  };
}

// ---------------------------------------------------------------- Clip

function clipPanel(trackId: number, clipId: number): Panel {
  const name = h("input", { class: "num", style: "width:100%;font-family:inherit", title: t("Clip name (Enter applies)") });
  const bpm = h("input", { class: "num", type: "number", min: 20, max: 400, step: 0.1 });
  const warp = h("input", { type: "checkbox" });
  const startV = h("div", { class: "value" });
  const lenV = h("div", { class: "value" });
  const trackV = h("div", { class: "value" });
  const speedHint = h("div", { class: "hint" });
  const extra = h("div", { class: "props", style: "margin-top:14px" });

  name.addEventListener("change", () => name.value.trim() && run({ type: "setClip", trackId, clipId, patch: { name: name.value.trim() } }));
  bpm.addEventListener("change", () => {
    const v = Number(bpm.value);
    if (v >= 20 && v <= 400) void run({ type: "setClip", trackId, clipId, patch: { sourceBpm: v } });
  });
  warp.addEventListener("change", () => run({ type: "setClip", trackId, clipId, patch: { warp: warp.checked } }));
  for (const i of [name, bpm]) i.addEventListener("keydown", (e) => { e.stopPropagation(); if (e.key === "Enter") i.blur(); });

  const audioProps = h(
    "div",
    { class: "props" },
    prop(t("Source tempo (BPM)"), bpm, t("Tempo the material was recorded at")),
    prop(t("Sync"), h("label", { class: "check" }, warp, t("match project tempo")), undefined),
    h("div", { class: "prop" }, h("label", {}, t("Playback")), speedHint),
  );
  const actions = h(
    "div",
    { class: "actions" },
    h("button", { class: "btn", title: t("Estimate the recording's tempo from its onsets and set it as the source tempo"), onclick: () => detectTempo() }, icon("metronome"), t("Detect tempo")),
    h("button", { class: "btn", title: t("Determine the recording's pitches, notes, tempo and key – in the \"Analysis\" window"), onclick: () => analyzeSelected() }, icon("chart"), t("Analyze")),
    h("button", { class: "btn", title: t("Turn the detected melody into notes on a new instrument track right below"), onclick: () => melodyToTrack() }, icon("keys"), t("Melody → instrument track")),
    h("button", { class: "btn", title: t("The detected hits as a drum track right below: kick, snare and hi-hat by timbre"), onclick: () => void convertAudio(trackId, clipId, "rhythmus") }, icon("grid"), t("Rhythm → drums")),
    h("button", { class: "btn ghost", title: t("Remove this clip (undo with Ctrl+Z)"), onclick: () => { void run({ type: "removeClip", trackId, clipId }); selection.value = null; } }, icon("trash"), t("Delete clip")),
  );

  const expr = exprSection(trackId, clipId);
  const el = h(
    "div",
    {},
    h("div", { class: "props" }, prop(t("Name"), name), h("div", { class: "prop" }, h("label", {}, t("Track")), trackV), h("div", { class: "prop" }, h("label", {}, t("Start (bar.beat)")), startV), h("div", { class: "prop" }, h("label", {}, t("Length")), lenV)),
    h("div", { style: "height:14px" }),
    audioProps,
    expr.el,
    extra,
    actions,
  );

  return {
    el,
    update() {
      const p = project();
      const found = findClip(p, { trackId, clipId });
      if (!p || !found) return;
      const { track, clip } = found;
      if (document.activeElement !== name) name.value = clip.name;
      trackV.textContent = track.name;
      startV.textContent = formatBarBeat(clip.startBeats, p.beatsPerBar);
      const len = clipLengthBeats(clip, p.tempoBpm);
      lenV.textContent = t("{bars} bars · {beats} beats", { bars: +(len / p.beatsPerBar).toFixed(2), beats: +len.toFixed(2) });
      const isAudio = clip.source.type === "audio";
      audioProps.style.display = isAudio ? "" : "none";
      expr.el.style.display = isAudio ? "none" : "";
      if (!isAudio) expr.update();
      actions.children[0]!.toggleAttribute("hidden", !isAudio);
      actions.children[1]!.toggleAttribute("hidden", !isAudio);
      actions.children[2]!.toggleAttribute("hidden", !isAudio);
      if (clip.source.type === "audio") {
        const s = clip.source;
        if (document.activeElement !== bpm) bpm.value = String(s.sourceBpm);
        warp.checked = s.warp;
        const speed = s.warp ? p.tempoBpm / s.sourceBpm : 1;
        speedHint.textContent = s.warp
          ? t("{percent}% speed, pitch unchanged", { percent: (speed * 100).toFixed(1) })
          : t("Original speed (not synced)");
        extra.replaceChildren(prop(t("Length of the recording"), `${s.durationSecs.toFixed(2)} s`));
      } else {
        const notes = clip.source.notes;
        const lo = Math.min(...notes.map((n) => n.pitch));
        const hi = Math.max(...notes.map((n) => n.pitch));
        extra.replaceChildren(
          prop(t("Notes"), String(notes.length)),
          prop(t("Pitch range"), notes.length ? `${noteName(lo)} – ${noteName(hi)}` : "–"),
          prop(t("Sound"), track.instrument?.preset ? t(track.instrument.preset) : "–", t("Design in the \"Sound\" tab")),
          h(
            "div",
            { class: "prop", style: "grid-column: span 2" },
            h("label", {}, t("Edit notes (piano roll)")),
            h("div", { class: "hint" }, t("Alt+click adds a note (also next to the clip – it grows with it) · click selects, Shift/Ctrl+click selects several, dragging on empty space draws a selection box · dragging selected notes moves them (Shift: off the grid) · the right edge changes the length · Del deletes · ↑/↓ transposes (Shift: by octave) · right-click: quantize, transpose, delete · double-click on empty space creates a new clip")),
            h("div", { class: "hint" }, t("Expression: Alt+drag on a note changes the velocity (Alt+Shift: vibrato) · in the expression lane below the notes, drag or paint values, Shift draws a straight line (crescendo), double-click resets")),
          ),
        );
      }
    },
  };
}

// ---------------------------------------------------------------- Analysis

function analysisPanel(a: Analysis): Panel {
  const canvas = h("canvas");
  const draw = () => drawAnalysis(canvas, a);
  new ResizeObserver(draw).observe(canvas);
  const corrected = a.notes.filter((n) => n.midi !== n.rawMidi).length;
  const facts = h(
    "div",
    { class: "facts" },
    prop(t("Key"), a.key ? a.key.name : "–", a.key ? t("Confidence {percent}%", { percent: (a.key.confidence * 100).toFixed(0) }) : t("too few notes")),
    prop(t("Tempo estimate"), a.tempoEstimate ? `${a.tempoEstimate.toFixed(1)} BPM` : "–"),
    prop(t("Notes"), String(a.notes.length), corrected ? t("Corrected {n} out-of-tune notes", { n: corrected }) : t("no correction needed")),
    prop(t("Duration"), `${a.durationSecs.toFixed(2)} s`),
    h("button", { class: "btn", onclick: () => melodyToTrack() }, icon("keys"), t("Melody → instrument track")),
  );
  const legend = h(
    "div",
    { class: "legend" },
    h("span", {}, h("i", { style: "background:#6ee7d8" }), t("Pitch curve")),
    h("span", {}, h("i", { style: "background:#ffb454" }), t("detected notes")),
    h("span", {}, h("i", { style: "background:#ff5d6c" }), t("corrected (before)")),
    h("span", {}, h("i", { style: "background:#8a93a6" }), t("Onsets")),
  );
  const el = h("div", { class: "analysis" }, facts, h("div", { style: "display:grid;grid-template-rows:auto 1fr;gap:8px;min-height:0" }, legend, canvas));
  return { el, update: draw };
}

function drawAnalysis(canvas: HTMLCanvasElement, a: Analysis): void {
  const { ctx, w, h: hh } = fitCanvas(canvas);
  ctx.clearRect(0, 0, w, hh);
  const pitches = a.pitch.map((p) => p[1]).filter((v): v is number => v != null);
  const all = [...pitches, ...a.notes.map((n) => n.midi), ...a.notes.map((n) => n.rawMidi)];
  if (!all.length) {
    ctx.fillStyle = "#5c6578";
    ctx.font = "13px system-ui";
    ctx.fillText(t("No pitch detected – sing or hum louder or closer to the microphone."), 16, 28);
    return;
  }
  const lo = Math.floor(Math.min(...all)) - 2;
  const hi = Math.ceil(Math.max(...all)) + 2;
  const padL = 40;
  const x = (t: number) => padL + (t / Math.max(a.durationSecs, 0.001)) * (w - padL - 8);
  // Direction as in the note view ("Pitch axis" setting)
  const up = highOnTop.peek();
  const y = (m: number) => 8 + ((up ? hi - m : m - lo) / (hi - lo)) * (hh - 28);

  ctx.font = "10px system-ui";
  ctx.textBaseline = "middle";
  for (let m = Math.ceil(lo); m <= hi; m++) {
    const isC = m % 12 === 0;
    ctx.fillStyle = isC ? "#222937" : "#141821";
    ctx.fillRect(padL, y(m), w - padL, 1);
    if (isC || hi - lo < 16) {
      ctx.fillStyle = "#5c6578";
      ctx.fillText(noteName(m), 4, y(m));
    }
  }
  ctx.fillStyle = "#8a93a6";
  for (const o of a.onsets) ctx.fillRect(x(o), hh - 14, 1.5, 10);

  const nh = Math.max(4, Math.min(12, (hh - 28) / (hi - lo)));
  for (const n of a.notes) {
    if (n.rawMidi !== n.midi) {
      ctx.strokeStyle = "#ff5d6c";
      ctx.setLineDash([3, 3]);
      ctx.strokeRect(x(n.startSecs), y(n.rawMidi) - nh / 2, x(n.endSecs) - x(n.startSecs), nh);
      ctx.setLineDash([]);
    }
    ctx.fillStyle = "rgba(255, 180, 84, 0.85)";
    ctx.beginPath();
    ctx.roundRect(x(n.startSecs), y(n.midi) - nh / 2, Math.max(2, x(n.endSecs) - x(n.startSecs)), nh, 3);
    ctx.fill();
  }

  ctx.strokeStyle = "#6ee7d8";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  let pen = false;
  for (const [t, m] of a.pitch) {
    if (m == null) {
      pen = false;
      continue;
    }
    if (pen) ctx.lineTo(x(t), y(m));
    else ctx.moveTo(x(t), y(m));
    pen = true;
  }
  ctx.stroke();
}

// ---------------------------------------------------------------- Sound

// ---------------------------------------------------------------- Window

/**
 * Content of a tool window in the docking system: `<kw-panel tab="klang">`.
 * Each window follows the selection itself and rebuilds when its
 * subject changes (different clip, different track, new analysis).
 */
class KwPanel extends HTMLElement {
  #body = h("div", { class: "tab-body" });
  #panel: Panel | null = null;
  #key = "";
  #analysis: Analysis | null = null;

  connectedCallback(): void {
    if (this.childElementCount) return;
    const tab = (this.getAttribute("tab") ?? "clip") as Tab;
    this.append(this.#body);

    effect(() => {
      const sel = selection.value;
      const snap = snapshot.value;
      const a = tab === "analyse" ? analysis.value : null;
      if (tab === "agent") chat.value; // redraw chat
      if (tab === "analyse") highOnTop.value; // redraw curve when the pitch axis changes
      if (tab === "clip") noteSel.value; // expression follows the note selection
      if (tab === "bibliothek") librarySignals();
      if (tab === "klang") savedSounds.value; // redraw My sounds
      const specsReady = tab === "klang" ? instrumentSpecs.value.length : 0;
      const p = snap?.project ?? null;
      const track = findTrack(p, sel?.trackId);
      const clip = findClip(p, sel);
      const key =
        tab === "clip" ? `${track?.id ?? ""}|${clip?.clip.id ?? ""}`
        : tab === "klang" ? `${track?.id ?? ""}|${track?.kind ?? ""}|${track?.instrument?.kind ?? ""}|${specsReady}`
        : tab;
      if (key !== this.#key || a !== this.#analysis) {
        this.#key = key;
        this.#analysis = a;
        this.#panel = this.#build(tab, track ?? null, clip ? clip.clip.id : null, a);
        this.#body.replaceChildren(this.#panel.el);
      }
      this.#panel?.update();
    });
  }

  #build(tab: Tab, track: Track | null, clipId: number | null, a: Analysis | null): Panel {
    if (tab === "agent") return chatPanel();
    if (tab === "bibliothek") return libraryPanel();
    if (tab === "harmonie") return harmonyPanel();
    if (tab === "rhythmus") return rhythmPanel();
    if (tab === "song") return songPanel();
    if (tab === "fuge") return fuguePanel();
    if (tab === "clip") {
      if (track && clipId != null) return clipPanel(track.id, clipId);
      if (track) return placeholder(t("Track \"{name}\" selected – click a clip in the timeline.", { name: track.name }));
      return placeholder(t("Select a clip in the timeline."));
    }
    if (tab === "analyse") {
      if (a) return analysisPanel(a);
      return placeholder(t("Record something (R) or select an audio clip and click \"Analyze\" (or double-click the clip)."));
    }
    if (track?.kind === "instrument") return soundPanel(track.id);
    return placeholder(t("Select an instrument track to design its sound."));
  }
}

customElements.define("kw-panel", KwPanel);
