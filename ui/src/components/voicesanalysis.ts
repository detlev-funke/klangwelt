// The analysis window in the polyphonic mode: how many voices the recording
// has (found or given), the notes of each voice in its color over the pitches
// found per moment, key and tempo – and the choice of the analysis mode, which
// the single-line window and the live view show too.

import { loadVoices, melodyToTrack } from "../actions.js";
import { fitCanvas, h, icon, noteName } from "../dom.js";
import { plural, t } from "../i18n.js";
import { highOnTop } from "../pianoroll.js";
import { SMOOTH_LEVELS, setSmoothLevel, smoothLabel, smoothLevel, smoothTitle } from "../smoothing.js";
import { VOICE_CHOICES, analysisMode, setAnalysisMode, setVoiceCount, voiceColor, voiceCount, voiceName } from "../voices.js";
import { noteAlpha } from "./performance.js";
import type { VoicesAnalysis } from "../types.js";

interface Panel {
  el: HTMLElement;
  update(): void;
}

function prop(label: string, value: Node | string, hint?: string, wide = false): HTMLElement {
  return h(
    "div",
    { class: wide ? "prop wide" : "prop" },
    h("label", {}, label),
    typeof value === "string" ? h("div", { class: "value" }, value) : value,
    hint ? h("div", { class: "hint" }, hint) : null,
  );
}

/** A row of option buttons that follows a value. */
function choice(cls: string, items: Array<[string, string, string]>, current: () => string, pick: (v: string) => void): { el: HTMLElement; update(): void } {
  const buttons = items.map(([value, label, title]) => h("button", { class: "seg-btn", "data-value": value, title, onclick: () => pick(value) }, label));
  const el = h("div", { class: `seg mini ${cls}`, role: "group" }, ...buttons);
  const update = () => {
    for (const b of buttons) {
      const on = b.dataset.value === current();
      b.classList.toggle("on", on);
      b.setAttribute("aria-pressed", String(on));
    }
  };
  update();
  return { el, update };
}

/** One line or several voices, and how many – where a recording is analyzed.
 *  `voicesRow`: also the voice count (shown in the polyphonic mode). */
export function modeChoice(): { el: HTMLElement; update(): void } {
  const mode = choice(
    "mode-choice",
    [
      ["mono", t("One line"), t("A melody: singing, whistling, one instrument – with dynamics, accents and articulation")],
      ["poly", t("Several voices"), t("Several voices at once: a choir, a piano, guitar with bass – one track per voice")],
    ],
    () => analysisMode.peek(),
    (v) => {
      setAnalysisMode(v as "mono" | "poly");
      if (v === "poly") void loadVoices();
    },
  );
  const count = choice(
    "voices-choice",
    VOICE_CHOICES.map((v) => [v, v === "auto" ? t("Auto") : v, v === "auto" ? t("Find out how many voices the recording has") : t("Exactly {n} voices", { n: v })]),
    () => voiceCount.peek(),
    (v) => {
      setVoiceCount(v);
      void loadVoices();
    },
  );
  const countRow = prop(t("Voices"), h("div", { class: "value" }, count.el));
  const el = h("div", { class: "prop wide analysis-mode" }, h("label", {}, t("Analysis")), h("div", { class: "value" }, mode.el), countRow);
  const update = () => {
    mode.update();
    count.update();
    countRow.style.display = analysisMode.peek() === "poly" ? "" : "none";
  };
  update();
  return { el, update };
}

export function voicesPanel(v: VoicesAnalysis): Panel {
  const canvas = h("canvas");
  const draw = () => drawVoices(canvas, v);
  new ResizeObserver(draw).observe(canvas);
  const mode = modeChoice();
  const smooth = choice("smooth-choice", SMOOTH_LEVELS.map((l) => [l, smoothLabel(l), smoothTitle(l)]), () => smoothLevel.peek(), (l) => setSmoothLevel(l as never));
  const perVoice = v.perVoice.map((info, i) =>
    prop(
      voiceName(i),
      h("div", { class: "value" }, h("i", { class: "voice-dot", style: `background:${voiceColor(i)}` }), plural(info.notes, "{n} note", "{n} notes")),
      info.low != null && info.high != null ? (info.low === info.high ? noteName(info.low) : `${noteName(info.low)} – ${noteName(info.high)}`) : undefined,
    ),
  );
  const facts = h(
    "div",
    { class: "facts voices-facts" },
    mode.el,
    prop(t("Voices"), String(v.voices), v.auto ? t("found in the recording") : t("as chosen"), true),
    ...perVoice,
    prop(t("Key"), v.key ? v.key.name : "–", v.key ? t("Confidence {percent}%", { percent: (v.key.confidence * 100).toFixed(0) }) : t("too few notes")),
    prop(t("Tempo estimate"), v.tempoEstimate ? `${v.tempoEstimate.toFixed(1)} BPM` : "–"),
    prop(t("Duration"), `${v.durationSecs.toFixed(2)} s`),
    prop(t("Smooth notes"), h("div", { class: "value" }, smooth.el), t("Per voice: short fragments and repeated notes of one pitch are merged"), true),
    h("button", { class: "btn voices-convert", onclick: () => melodyToTrack() }, icon("keys"), plural(v.voices, "Voice → instrument track", "Voices → instrument tracks")),
  );
  const legend = h(
    "div",
    { class: "legend" },
    ...Array.from({ length: v.voices }, (_, i) => h("span", {}, h("i", { style: `background:${voiceColor(i)}` }), voiceName(i))),
    h("span", {}, h("i", { style: "background:#5c6578" }), t("pitches found")),
  );
  const el = h("div", { class: "analysis voices" }, facts, h("div", { style: "display:grid;grid-template-rows:auto 1fr;gap:8px;min-height:0" }, legend, canvas));
  return {
    el,
    update() {
      draw();
      mode.update();
      // the smoothing may change in the gear menu
      smooth.update();
    },
  };
}

/** Pitches found per moment (grey, by strength) and the notes in their voice's color. */
function drawVoices(canvas: HTMLCanvasElement, v: VoicesAnalysis): void {
  const { ctx, w, h: hh } = fitCanvas(canvas);
  ctx.clearRect(0, 0, w, hh);
  const all = v.notes.map((n) => n.midi);
  if (!all.length) {
    ctx.fillStyle = "#5c6578";
    ctx.font = "13px system-ui";
    ctx.fillText(t("No notes detected."), 16, 28);
    return;
  }
  const lo = Math.min(...all) - 3;
  const hi = Math.max(...all) + 3;
  const padL = 40;
  const x = (s: number) => padL + (s / Math.max(v.durationSecs, 0.001)) * (w - padL - 8);
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
  for (const o of v.onsets) ctx.fillRect(x(o), hh - 14, 1.5, 10);
  // what each moment held (the evidence behind the notes)
  for (const [time, peaks] of v.candidates) {
    for (const [m, s] of peaks) {
      if (m < lo || m > hi) continue;
      ctx.fillStyle = `rgba(138, 147, 166, ${Math.min(0.5, 0.15 + s * 0.3).toFixed(2)})`;
      ctx.fillRect(x(time), y(m) - 1, 2, 2);
    }
  }
  const nh = Math.max(4, Math.min(12, (hh - 28) / (hi - lo)));
  for (const n of v.notes) {
    ctx.globalAlpha = noteAlpha(n.velocity);
    ctx.fillStyle = voiceColor(n.voice);
    ctx.beginPath();
    ctx.roundRect(x(n.startSecs), y(n.midi) - nh / 2, Math.max(2, x(n.endSecs) - x(n.startSecs)), nh, 3);
    ctx.fill();
  }
  ctx.globalAlpha = 1;
}
