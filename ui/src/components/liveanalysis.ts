// Live analysis while recording: what sounds right now (note and cents,
// tuner-style), the input level, key and tempo as soon as there is enough
// material, and the last seconds as a running pitch view with the notes as
// they are detected. The core analyses (klang-app `LiveAnalysis`, fed by
// both shells); this window fetches what is new a dozen times a second
// (`record_live` with a cursor) and draws it. After the recording the
// finished analysis replaces it. In the polyphonic mode (`voices.ts`) the
// voices come from `record_live_voices`: each in its color, and what sounds
// right now instead of the tuner.

import { backend } from "../bridge.js";
import { fitCanvas, h, noteName } from "../dom.js";
import { t } from "../i18n.js";
import { highOnTop } from "../pianoroll.js";
import { dynamicName } from "../expression.js";
import { accentsText, drawMarks, noteAlpha } from "./performance.js";
import { analysisMode, voiceColor } from "../voices.js";
import type { KeyInfo, LiveNote, LiveView, LiveVoicesView } from "../types.js";

/** Seconds the running view shows. */
const WINDOW_SECS = 10;
/** Pause between two requests for what is new (ms). */
const POLL_MS = 80;
/** Up to this deviation (cents) a note counts as in tune. */
const IN_TUNE_CENTS = 5;

interface Panel {
  el: HTMLElement;
  update(): void;
}

/** What has arrived of the running recording. */
interface LiveState {
  take: number;
  seconds: number;
  curve: Array<[number, number | null]>;
  notes: LiveNote[];
  pending: LiveNote[];
  onsets: number[];
  last: LiveView | null;
  /** Polyphonic mode: the voices so far (the last seconds). */
  voices: LiveVoicesView | null;
  /** Shown pitch range (changes only when the notes leave it). */
  range: [number, number] | null;
}

function emptyState(take = 0): LiveState {
  return { take, seconds: 0, curve: [], notes: [], pending: [], onsets: [], last: null, voices: null, range: null };
}

/** Takes over what is new; a new recording starts afresh. */
function merge(st: LiveState, v: LiveView): LiveState {
  const s = v.take === st.take ? st : emptyState(v.take);
  s.curve.length = Math.min(s.curve.length, v.curveFrom);
  s.curve.push(...v.curve);
  s.notes.length = Math.min(s.notes.length, v.notesFrom);
  s.notes.push(...v.notes);
  s.onsets.length = Math.min(s.onsets.length, v.onsetsFrom);
  s.onsets.push(...v.onsets);
  s.pending = v.pending;
  s.seconds = v.seconds;
  s.last = v;
  return s;
}

function fact(label: string, value: HTMLElement, hint: HTMLElement | null, wide = false): HTMLElement {
  return h("div", { class: wide ? "prop wide" : "prop" }, h("label", {}, label), value, hint);
}

function keyText(k: KeyInfo | null): [string, string] {
  return k ? [k.name, t("Confidence {percent}%", { percent: (k.confidence * 100).toFixed(0) })] : ["–", t("after a few notes")];
}

export function liveAnalysisPanel(): Panel {
  let st = emptyState();

  // Tuner: note name, cents, a needle between −50 and +50 cents
  const note = h("div", { class: "live-note" }, "–");
  const cents = h("div", { class: "live-cents" }, "");
  const needle = h("i", { class: "needle" });
  const scale = h("div", { class: "cents-scale", title: t("−50 to +50 cents") }, h("b"), needle);
  const tuneHint = h("span", { class: "hint" }, t("Sing or play a note"));
  const tuner = h("div", { class: "value live-tuner" }, h("div", { class: "live-note-row" }, note, cents, tuneHint), scale);
  // polyphonic mode: what sounds right now, how many voices
  const poly = analysisMode.peek() === "poly";
  const soundingValue = h("div", { class: "value live-sounding" }, "–");
  const voicesValue = h("div", { class: "value" }, "–");
  const voicesHint = h("div", { class: "hint" }, "");

  // Level: bar from −60 to 0 dBFS
  const levelBar = h("i");
  const levelDb = h("span", { class: "live-db" }, "");
  const level = h("div", { class: "value live-level" }, h("div", { class: "level-bar" }, levelBar), levelDb);
  const levelHint = h("div", { class: "hint" }, "");

  const keyValue = h("div", { class: "value" }, "–");
  const keyHint = h("div", { class: "hint" }, t("after a few notes"));
  const tempoValue = h("div", { class: "value" }, "–");
  const tempoHint = h("div", { class: "hint" }, t("needs a steady beat"));
  const dynValue = h("div", { class: "value" }, "–");
  const dynHint = h("div", { class: "hint" }, "");
  const notesValue = h("div", { class: "value" }, "0");
  const durationValue = h("div", { class: "value" }, "0.0 s");

  const facts = h(
    "div",
    { class: "facts" },
    h("div", { class: "live-badge wide" }, h("i"), poly ? t("Live analysis · several voices") : t("Live analysis")),
    poly ? fact(t("Sounding now"), soundingValue, null, true) : fact(t("Current note"), tuner, null, true),
    poly ? fact(t("Voices"), voicesValue, voicesHint) : null,
    fact(t("Input level"), level, levelHint, true),
    fact(t("Key"), keyValue, keyHint),
    fact(t("Tempo estimate"), tempoValue, tempoHint),
    poly ? null : fact(t("Dynamics"), dynValue, dynHint),
    fact(t("Notes"), notesValue, null),
    fact(t("Duration"), durationValue, null),
  );
  const canvas = h("canvas", { class: "live-canvas" });
  const legend = h(
    "div",
    { class: "legend" },
    ...(poly
      ? [h("span", {}, h("i", { style: `background:linear-gradient(90deg,${voiceColor(0)},${voiceColor(1)},${voiceColor(2)})` }), t("Voices, each in its color"))]
      : [h("span", {}, h("i", { style: "background:#6ee7d8" }), t("Pitch curve")), h("span", {}, h("i", { style: "background:#ffb454" }), t("detected notes"))]),
    h("span", {}, h("i", { style: "background:#8a93a6" }), t("Onsets")),
  );
  const el = h("div", { class: "analysis live" }, facts, h("div", { style: "display:grid;grid-template-rows:auto 1fr;gap:8px;min-height:0" }, legend, canvas));

  const showFacts = (): void => {
    const v = st.last;
    if (!v) return;
    const sounding = v.note != null && v.cents != null;
    note.textContent = sounding ? noteName(v.note!) : "–";
    cents.textContent = sounding ? `${v.cents! > 0 ? "+" : v.cents! < 0 ? "−" : "±"}${Math.abs(v.cents!)} ¢` : "";
    const c = v.cents ?? 0;
    const tune = !sounding ? "" : Math.abs(c) <= IN_TUNE_CENTS ? "in" : Math.abs(c) <= 25 ? "near" : "off";
    tuner.dataset.tune = tune;
    needle.style.left = `${50 + Math.max(-50, Math.min(50, c))}%`;
    needle.style.visibility = sounding ? "visible" : "hidden";
    tuneHint.textContent = !sounding ? t("Sing or play a note") : Math.abs(c) <= IN_TUNE_CENTS ? t("in tune") : c > 0 ? t("too sharp") : t("too flat");

    const frac = Math.min(1, Math.max(0, (v.levelDb + 60) / 60));
    levelBar.style.width = `${(frac * 100).toFixed(1)}%`;
    level.dataset.clip = String(v.peak >= 0.99);
    levelDb.textContent = v.levelDb <= -119 ? "−∞ dB" : `${v.levelDb.toFixed(0).replace("-", "−")} dB`;
    levelHint.textContent = v.peak >= 0.99 ? t("Clipping – move back or turn the input down") : v.levelDb < -50 ? t("Very quiet – move closer to the microphone") : "";

    const [kName, kHint] = keyText(v.key);
    keyValue.textContent = kName;
    keyHint.textContent = kHint;
    tempoValue.textContent = v.tempoEstimate ? `${v.tempoEstimate.toFixed(1)} BPM` : "–";
    tempoHint.textContent = v.tempoEstimate ? "" : t("needs a steady beat");
    notesValue.textContent = String(st.notes.length + st.pending.length);
    const vv = st.voices;
    if (poly) {
      soundingValue.textContent = vv?.sounding.length ? vv.sounding.map((m) => noteName(m)).join(" · ") : "–";
      voicesValue.textContent = vv && vv.voices ? String(vv.voices) : "–";
      voicesHint.textContent = vv && vv.voices ? (vv.auto ? t("found so far") : t("as chosen")) : t("after a few notes");
      notesValue.textContent = String(vv?.notes.length ?? 0);
    }
    // dynamics of the latest note, accents and sforzati so far
    const all = [...st.notes, ...st.pending];
    const latest = all[all.length - 1];
    dynValue.textContent = latest ? dynamicName(latest.velocity) : "–";
    dynHint.textContent = accentsText(all);
    durationValue.textContent = `${v.seconds.toFixed(1)} s`;
  };

  let frame = 0;
  const redraw = (): void => {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      drawLive(canvas, st);
    });
  };
  new ResizeObserver(redraw).observe(canvas);

  // Fetch what is new while the window is shown; stop once it was removed.
  let busy = false;
  let shown = false;
  const timer = setInterval(async () => {
    if (!el.isConnected) {
      if (shown) clearInterval(timer);
      return;
    }
    shown = true;
    if (busy) return;
    busy = true;
    try {
      const v = await backend.recordLive({ take: st.take, curve: st.curve.length, notes: st.notes.length, onsets: st.onsets.length });
      if (v) {
        st = merge(st, v);
        if (poly) st.voices = await backend.recordLiveVoices();
        showFacts();
        redraw();
      }
    } catch {
      // the next request tries again
    } finally {
      busy = false;
    }
  }, POLL_MS);

  return { el, update: redraw };
}

/** The last seconds: semitone grid, onsets, notes, pitch curve and what sounds now. */
function drawLive(canvas: HTMLCanvasElement, st: LiveState): void {
  const { ctx, w, h: hh } = fitCanvas(canvas);
  ctx.clearRect(0, 0, w, hh);
  const end = Math.max(st.seconds, WINDOW_SECS);
  const start = end - WINDOW_SECS;
  const padL = 40;
  const x = (s: number) => padL + ((s - start) / WINDOW_SECS) * (w - padL - 8);

  let from = st.curve.length;
  while (from > 0 && st.curve[from - 1]![0] >= start) from--;
  const curve = st.curve.slice(from);
  const voiced = st.voices;
  const notes = voiced ? [] : [...st.notes.filter((n) => n.endSecs >= start), ...st.pending];
  const voiceNotes = voiced ? voiced.notes.filter((n) => n.endSecs >= start) : [];
  const now = voiced ? null : (st.last?.pitch ?? null);
  const values = voiced
    ? [...voiceNotes.map((n) => n.midi), ...voiced.sounding]
    : [...curve.map((p) => p[1]).filter((v): v is number => v != null), ...notes.map((n) => n.midi), ...(now != null ? [now] : [])];

  // "now" at the right edge of what was recorded
  ctx.fillStyle = "rgba(255, 93, 108, 0.5)";
  ctx.fillRect(x(st.seconds), 0, 1, hh);
  if (!values.length) {
    ctx.fillStyle = "#5c6578";
    ctx.font = "13px system-ui";
    ctx.textBaseline = "alphabetic";
    ctx.fillText(t("Listening… sing, hum or play something."), padL, 28);
    return;
  }

  // Pitch range: follows the notes, but only changes when they leave it
  const lo0 = Math.floor(Math.min(...values)) - 2;
  const hi0 = Math.ceil(Math.max(...values)) + 2;
  let [lo, hi] = st.range ?? [lo0, hi0];
  if (lo0 < lo || hi0 > hi || hi - lo > hi0 - lo0 + 12) [lo, hi] = [lo0, hi0];
  if (hi - lo < 12) {
    const mid = (lo + hi) / 2;
    [lo, hi] = [Math.floor(mid - 6), Math.ceil(mid + 6)];
  }
  st.range = [lo, hi];
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
  for (const o of st.onsets) if (o >= start) ctx.fillRect(x(o), hh - 14, 1.5, 10);

  const nh = Math.max(4, Math.min(12, (hh - 28) / (hi - lo)));
  notes.forEach((n, i) => {
    // dynamics as opacity; the running phrase (may still change) fainter
    const alpha = noteAlpha(n.velocity) * (st.pending.includes(n) ? 0.55 : 1);
    ctx.fillStyle = `rgba(255, 180, 84, ${alpha.toFixed(2)})`;
    const x0 = x(Math.max(n.startSecs, start));
    ctx.beginPath();
    ctx.roundRect(x0, y(n.midi) - nh / 2, Math.max(2, x(n.endSecs) - x0), nh, 3);
    ctx.fill();
    const next = notes[i + 1];
    if (n.startSecs >= start) drawMarks(ctx, n, x0, x(n.endSecs), y(n.midi), nh, next ? x(next.startSecs) : null, next ? y(next.midi) : null);
  });

  // polyphonic: every voice in its color, what sounds now at the right edge
  for (const n of voiceNotes) {
    ctx.globalAlpha = noteAlpha(n.velocity);
    ctx.fillStyle = voiceColor(n.voice);
    const x0 = x(Math.max(n.startSecs, start));
    ctx.beginPath();
    ctx.roundRect(x0, y(n.midi) - nh / 2, Math.max(2, x(n.endSecs) - x0), nh, 3);
    ctx.fill();
  }
  ctx.globalAlpha = 1;
  if (voiced) {
    ctx.fillStyle = "#e6e9ef";
    for (const m of voiced.sounding) {
      ctx.beginPath();
      ctx.arc(x(st.seconds), y(m), 4, 0, Math.PI * 2);
      ctx.fill();
    }
    return;
  }

  ctx.strokeStyle = "#6ee7d8";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  let pen = false;
  let prev = -1;
  for (const [s, m] of curve) {
    // gaps (silence, a slow update) break the line
    if (m == null || s - prev > 0.06) pen = false;
    prev = s;
    if (m == null) continue;
    if (pen) ctx.lineTo(x(s), y(m));
    else ctx.moveTo(x(s), y(m));
    pen = true;
  }
  ctx.stroke();

  if (now != null) {
    ctx.fillStyle = "#6ee7d8";
    ctx.beginPath();
    ctx.arc(x(st.seconds), y(now), 4, 0, Math.PI * 2);
    ctx.fill();
  }
}
