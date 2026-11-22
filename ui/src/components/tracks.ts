import { addTrack, convertAudio, resetRowHeight, startRowResize } from "../actions.js";
import { audioDrag } from "../filedrop.js";
import { backend } from "../bridge.js";
import { fitCanvas, h, icon, noteName, setRange } from "../dom.js";
import {
  EXPR_H, NOTES_TOP, RHYTHM_H, type NoteGeo, type Row, geoFor, heightDraft, highOnTop, isBlack, layoutRows, maxScrollY, pitchAt, pitchOfLane, pitchViews,
  refitPitch, rowHeight, scrollPitch, viewFor, wheelPitchStep, zoomPitch, drumMode,
} from "../pianoroll.js";
import { EXPRS, type ExprKey, exprKeysFor, exprLanes, exprShown, laneKeyFor, setLaneKey, toggleExprLane } from "../expression.js";
import { plural, t } from "../i18n.js";
import { effect } from "../signals.js";
import { act, instrumentSpecs, run, selection, snapshot, specFor, viewport } from "../store.js";
import { NOTE_VALUES, entryLetters, entryTrack, keyLabelsVersion, noteValue, pianoKey, setNoteValue, toggleEntry, toggleValueMod, valueLabel } from "../noteinput.js";
import { openContextMenu } from "../ctxmenu.js";
import { harmonyLaneOn, setHarmonyLane } from "../harmonyline.js";
import { type DropPlace, groupMenu, groupTracks, listMenu, moveTrackTo, pickTrack, picked, setGroup, trackMenu } from "../groups.js";
import { HEADS_MAX, HEADS_MIN, headsWidth, setHeadsWidth, showPan, showPitch, toggleShowPan, toggleShowPitch } from "../trackview.js";
import type { InstrumentKind, Track, TrackGroup } from "../types.js";

/** "Harmony" button: show/hide the chord progression in the timeline. */
function harmonyToggle(): HTMLElement {
  // only the circle of fifths (like the Harmony window's tab) – the bar above the headers is narrow
  const b = h("button", { class: "btn small icon harmony-toggle", "aria-label": t("Harmony"), title: t("Show harmony in the timeline: chords in the color of their mood, breaks in red (also appears while you work in the Harmony window)"), onclick: () => setHarmonyLane(!harmonyLaneOn.peek()) }, icon("fifths"));
  effect(() => b.classList.toggle("on", harmonyLaneOn.value));
  return b;
}

/**
 * Keyboard at the right edge of the track header (note view only).
 * Like a real piano, played from the left: white keys across the full
 * width with the key front on the left, black keys shorter and at the back
 * (right, at the timeline). The key height follows zoom and row height.
 */
function drawKeys(canvas: HTMLCanvasElement, g: NoteGeo, drumKeys?: Map<number, string>, letters?: Map<number, string> | null): void {
  const { ctx, w, h: hh } = fitCanvas(canvas);
  ctx.clearRect(0, 0, w, hh);
  const LANE_SPAN = g.view.span;
  const LANE_H = g.laneH;
  const total = g.lanesH;
  const blackW = Math.round(w * 0.6);
  const blackX = w - blackW;
  const up = highOnTop.peek();
  const pitchOf = (k: number) => pitchOfLane(g.view, k);

  // White keys as a continuous surface, lighter toward the front (left).
  const white = ctx.createLinearGradient(0, 0, w, 0);
  white.addColorStop(0, "#e4e7ee");
  white.addColorStop(1, "#c3c8d2");
  ctx.fillStyle = white;
  ctx.fillRect(0, 0, w, total);

  // Gaps between white keys.
  ctx.fillStyle = "#8a919e";
  for (let k = 0; k < LANE_SPAN; k++) {
    const pitch = pitchOf(k);
    const y = k * LANE_H;
    if (isBlack(pitch)) {
      // two white keys meet before the black key
      ctx.fillRect(0, Math.round(y + LANE_H / 2), blackX, 1);
    } else if (!isBlack(pitch + 1)) {
      // E|F and B|C: white keys directly next to each other – gap on the side of the higher note
      ctx.fillRect(0, up ? y : y + LANE_H - 1, w, 1);
    }
  }

  // Black keys: shorter, attached at the back (right), front rounded on the left.
  for (let k = 0; k < LANE_SPAN; k++) {
    const pitch = pitchOf(k);
    if (!isBlack(pitch)) continue;
    const y = k * LANE_H + 0.5;
    ctx.fillStyle = "#171b23";
    ctx.beginPath();
    ctx.roundRect(blackX, y, blackW, Math.max(2, LANE_H - 1), [2, 0, 0, 2]);
    ctx.fill();
    if (LANE_H >= 6) {
      ctx.fillStyle = "#3a4252";
      ctx.fillRect(blackX + 2, y + 1, blackW - 3, 1);
    }
  }

  // Drum kit: keys without a drum are dimmed – they play tuned drums, not drums of the kit.
  if (drumKeys) {
    ctx.fillStyle = "rgba(14, 17, 23, 0.5)";
    for (let k = 0; k < LANE_SPAN; k++) if (!drumKeys.has(pitchOf(k))) ctx.fillRect(0, k * LANE_H, w, LANE_H);
  }

  // Label at the front (left) on the white key: C notes, or for drums the drum sounds.
  ctx.font = `600 ${Math.max(5.5, Math.min(7.5, LANE_H - 0.5)).toFixed(1)}px system-ui`;
  ctx.textAlign = "left";
  ctx.textBaseline = "middle";
  for (let k = 0; k < LANE_SPAN; k++) {
    const pitch = pitchOf(k);
    const y = k * LANE_H + LANE_H / 2 + 0.5;
    if (drumKeys) {
      const name = drumKeys.get(pitch);
      if (!name) continue;
      ctx.fillStyle = "rgba(255, 180, 84, 0.9)";
      ctx.fillRect(0, k * LANE_H + 1, 2, Math.max(2, LANE_H - 2));
      ctx.fillStyle = "#1b2130";
      ctx.fillText(name, 3, y);
    } else if (pitch % 12 === 0) {
      ctx.fillStyle = "#5b6271";
      ctx.fillText(noteName(pitch), 2, y);
    }
  }

  // Keyboard entry: the keys the letters play are tinted and show the letter (at the back, right).
  if (letters) {
    ctx.font = `700 ${Math.max(6, Math.min(8.5, LANE_H - 1)).toFixed(1)}px system-ui`;
    ctx.textAlign = "right";
    for (let k = 0; k < LANE_SPAN; k++) {
      const pitch = pitchOf(k);
      const letter = letters.get(pitch);
      if (!letter) continue;
      const y = k * LANE_H;
      const black = isBlack(pitch);
      ctx.fillStyle = black ? "rgba(199, 146, 255, 0.42)" : "rgba(199, 146, 255, 0.26)";
      if (black) ctx.fillRect(blackX, y + 0.5, blackW, Math.max(2, LANE_H - 1));
      else ctx.fillRect(0, y, w, LANE_H);
      if (LANE_H < 6) continue;
      ctx.fillStyle = black ? "#efe4ff" : "#4b2380";
      ctx.fillText(letter, w - 3, y + LANE_H / 2 + 0.5);
    }
  }
}

/** Short names of the drums of a drum track in the drum kit (key → "BD", "SD" …). */
export function drumKeyNames(t: Track): Map<number, string> | undefined {
  if (drumMode(t) !== "kit") return undefined;
  const keys = specFor(t.instrument)?.keys ?? [];
  return new Map(keys.map((k) => [k.midi, k.short]));
}

/** Control whose intermediate values merge into one undo step while dragging. */
function gestureRange(min: number, max: number, step: number, onValue: (v: number, merge: boolean) => void, reset?: number) {
  const input = h("input", { type: "range", min, max, step });
  let merging = false;
  input.addEventListener("pointerdown", () => (merging = false));
  input.addEventListener("input", () => {
    onValue(Number(input.value), merging);
    merging = true;
  });
  input.addEventListener("change", () => (merging = false));
  input.addEventListener("keydown", (e) => e.stopPropagation());
  if (reset !== undefined) {
    input.addEventListener("dblclick", () => {
      input.value = String(reset);
      onValue(reset, false);
    });
  }
  return input;
}

function panText(p: number): string {
  if (Math.abs(p) < 0.01) return "C";
  return `${p < 0 ? "L" : "R"} ${Math.round(Math.abs(p) * 100)}`;
}

interface NoteValuePicker {
  /** The row of note values (wide header). */
  el: HTMLElement;
  /** One button with the chosen value that opens the choice (slim column on a phone). */
  compact: HTMLButtonElement;
  update(): void;
}

/** Names of the note values 1/1 … 1/32 with their digit key (keyboard entry). */
function valueTitle(d: number): string {
  const key = NOTE_VALUES.indexOf(d as (typeof NOTE_VALUES)[number]) + 1;
  const name = d === 1 ? t("Whole note") : d === 2 ? t("Half note") : d === 4 ? t("Quarter note") : d === 8 ? t("Eighth note") : d === 16 ? t("Sixteenth note") : t("32nd note");
  return `${name} (${key})`;
}
const AUTO_TITLE = () => t("Automatic: typed notes one grid step long (it follows the zoom), drawn notes as long as the last one you drew (0)");
const DOT_TITLE = () => t("Dotted: one and a half times as long (.)");
const TRIPLET_TITLE = () => t("Triplet: two thirds as long – three in the time of two (,)");

/**
 * Length of new notes next to the piano: Auto, 1/1 … 1/32, dotted, triplet. New notes from the
 * piano, the letter keys, double-click/double tap and Alt+click get it; digit keys choose it while typing.
 */
function noteValuePicker(): NoteValuePicker {
  const values = [0, ...NOTE_VALUES].map((d) => {
    // (a redraw only switches classes – the buttons stay, so a press may select the track and start a swipe)
    const b = h("button", { class: "nv", "data-value": String(d), title: d ? valueTitle(d) : AUTO_TITLE(), onclick: () => setNoteValue(d) }, d ? `1/${d}` : t("Auto"));
    return { b, d };
  });
  const mods = ([["dot", "·", DOT_TITLE()], ["tri", "3", TRIPLET_TITLE()]] as const).map(([mod, label, title]) => {
    const b = h("button", { class: "nv mod", "data-mod": mod, title, "aria-label": title, onclick: () => toggleValueMod(mod) }, label);
    return { b, mod };
  });
  const el = h(
    "div",
    { class: "note-values", role: "group", "aria-label": t("Length of new notes"), "data-help": "notenlaenge" },
    ...values.map((v) => v.b),
    h("span", { class: "nv-sep" }),
    ...mods.map((m) => m.b),
  );
  const compact = h("button", { class: "nv-compact", "data-help": "notenlaenge" });
  compact.addEventListener("click", () => {
    const r = compact.getBoundingClientRect();
    const v = noteValue.peek();
    openContextMenu(r.left, r.bottom + 4, [
      { heading: t("Length of new notes") },
      {
        label: t("Note value"),
        act: "notenwert",
        chips: [0, ...NOTE_VALUES].map((d) => ({ label: d ? `1/${d}` : t("Auto"), act: `notenwert-${d}`, title: d ? valueTitle(d) : AUTO_TITLE(), on: v.d === d, action: () => setNoteValue(d) })),
      },
      {
        label: t("Variant"),
        act: "notenwert-art",
        chips: [
          { label: t("plain"), act: "notenwert-normal", on: !!v.d && !v.mod, action: () => v.mod && toggleValueMod(v.mod) },
          { label: t("dotted"), act: "notenwert-punktiert", title: DOT_TITLE(), on: v.mod === "dot", disabled: !v.d, action: () => v.mod !== "dot" && toggleValueMod("dot") },
          { label: t("triplet"), act: "notenwert-triole", title: TRIPLET_TITLE(), on: v.mod === "tri", disabled: !v.d, action: () => v.mod !== "tri" && toggleValueMod("tri") },
        ],
      },
    ]);
  });
  return {
    el,
    compact,
    update() {
      const v = noteValue.peek();
      for (const { b, d } of values) {
        b.classList.toggle("on", v.d === d);
        b.setAttribute("aria-pressed", String(v.d === d));
      }
      for (const { b, mod } of mods) {
        b.classList.toggle("on", v.mod === mod);
        b.setAttribute("aria-pressed", String(v.mod === mod));
        b.disabled = !v.d;
      }
      compact.textContent = `♪ ${valueLabel(v)}`;
      compact.title = t("Length of new notes: {value} – tap to change", { value: valueLabel(v) });
    },
  };
}

class TrackRow {
  readonly el: HTMLDivElement;
  #track!: Track;
  #name: HTMLInputElement;
  #mute: HTMLButtonElement;
  #solo: HTMLButtonElement;
  #gain: HTMLInputElement;
  #gainVal: HTMLElement;
  #pan: HTMLInputElement;
  #panVal: HTMLElement;
  #pitchVal: HTMLElement;
  #kind: HTMLElement;
  #viewBtn: HTMLButtonElement;
  #keys: HTMLCanvasElement;
  #rhythm: HTMLDivElement;
  #exprBtn: HTMLButtonElement;
  #convertBtn: HTMLButtonElement;
  #exprHead: HTMLDivElement;
  #exprSelect: HTMLSelectElement;
  #exprHint: HTMLElement;
  #entryBtn: HTMLButtonElement;
  #values: NoteValuePicker;

  constructor(readonly id: number) {
    const set = (patch: object, merge = false) => run({ type: "setTrack", trackId: this.id, patch }, merge);

    this.#name = h("input", { class: "text", spellcheck: false, title: t("Track name – click and type to rename") });
    this.#name.addEventListener("change", () => {
      const v = this.#name.value.trim();
      if (v) void set({ name: v });
      else this.#name.value = this.#track.name;
    });
    this.#name.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter" || e.key === "Escape") this.#name.blur();
    });

    this.#mute = h("button", { class: "toggle m", title: t("Mute"), onclick: () => set({ mute: !this.#track.mute }) }, "M");
    this.#solo = h("button", { class: "toggle s", title: t("Solo"), onclick: () => set({ solo: !this.#track.solo }) }, "S");
    const del = h("button", { class: "btn ghost small", title: t("Delete track"), onclick: () => run({ type: "removeTrack", trackId: this.id }) }, icon("trash"));

    this.#gain = gestureRange(-60, 6, 0.5, (v, m) => set({ gainDb: v }, m), 0);
    this.#gain.title = t("Level in dB – double-click: 0 dB");
    this.#gain.dataset.help = "pegel";
    this.#gainVal = h("b");
    this.#pan = gestureRange(-1, 1, 0.01, (v, m) => set({ pan: v }, m), 0);
    this.#pan.title = t("Pan: left ↔ right – double-click: center");
    this.#pan.dataset.help = "panorama";
    this.#panVal = h("b");

    this.#pitchVal = h("span", { class: "val", title: t("Pitch in semitones") });
    const pitch = (d: number) => {
      const v = Math.max(-24, Math.min(24, Math.round(this.#track.pitchSemitones) + d));
      void set({ pitchSemitones: v });
    };
    const stepper = h(
      "div",
      { class: "stepper pitch", title: t("Pitch (semitones) – tempo stays the same") },
      h("button", { title: t("One semitone down"), onclick: () => pitch(-1) }, "−"),
      this.#pitchVal,
      h("button", { title: t("One semitone up"), onclick: () => pitch(1) }, "+"),
    );

    this.#kind = h("span", { class: "kind" });
    this.#viewBtn = h("button", {
      class: "toggle v",
      onclick: () => set({ view: this.#track.view === "notes" ? "compact" : "notes" }),
    });

    this.#keys = h("canvas", {
      class: "keys",
      title: t("Mouse wheel: shift pitch range · Ctrl+mouse wheel: zoom · Double-click: fit · Click: play note – at the end of the piece or with keyboard entry it is added"),
    });
    this.#keys.style.top = `${NOTES_TOP}px`;
    this.#keys.addEventListener("wheel", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const view = viewFor(this.#track);
      if (e.ctrlKey || e.metaKey) zoomPitch(this.id, e.deltaY > 0 ? 3 : -3, view);
      else scrollPitch(this.id, wheelPitchStep(e.deltaY), view); // up = what lies above
    }, { passive: false });
    this.#keys.addEventListener("dblclick", () => refitPitch(this.id));

    this.#rhythm = h("div", {
      class: "rhythm-label",
      title: t("Rhythm lane: unpitched hits like hi-hat or shaker (on instrument tracks, the note onsets)"),
    }, t("Rhythm"));
    this.#rhythm.style.height = `${RHYTHM_H}px`;

    // Expression track: which form of expression is shown and edited.
    this.#exprSelect = h("select", { class: "expr-select", title: t("Expression type shown in the expression lane") });
    this.#exprSelect.addEventListener("change", () => setLaneKey(this.id, this.#exprSelect.value as ExprKey));
    this.#exprSelect.addEventListener("pointerdown", (e) => e.stopPropagation());
    this.#exprHint = h("span", { class: "expr-range" });
    this.#exprHead = h("div", {
      class: "expr-head",
      title: t("Expression lane: drag values or paint across several notes · Shift: straight line (e.g. crescendo) · Double-click: reset"),
    }, this.#exprSelect, this.#exprHint);
    this.#exprHead.style.height = `${EXPR_H}px`;
    this.#exprBtn = h("button", {
      class: "toggle v x",
      onclick: () => {
        const t = this.#track;
        const shown = exprShown(t);
        toggleExprLane(this.id);
        // Let the stored row height grow/shrink along, so that the note lanes stay the same size.
        if (t.notesHeight > 0) void set({ notesHeight: Math.max(140, t.notesHeight + (shown ? -EXPR_H : EXPR_H)) });
      },
    }, icon("expr"));

    // Audio track in the pitch view: convert detected notes of all clips into a melody track.
    this.#convertBtn = h("button", {
      class: "toggle convert",
      title: t("Convert to notes: create the detected pitches of all clips as a melody track below (right-click a clip: also as a rhythm track)"),
      onclick: () => void convertAudio(this.id, null, "melodie"),
    }, icon("keys"), t("Notes"));

    // Keyboard entry mode: letters and clicks on the piano add notes to this track.
    this.#entryBtn = h("button", {
      class: "entry-switch",
      role: "switch",
      title: t("Keyboard entry: letter keys and clicks on the piano add notes at the violet mark – each time one note length further, with Shift in the same place (chords)"),
      onclick: () => {
        toggleEntry(this.id);
        this.#entryBtn.blur(); // Enter (stop) must not toggle it again
      },
    }, h("span", { class: "switch" }), icon("keyboard"), h("span", { class: "lbl" }, t("Keyboard entry")));
    this.#entryBtn.setAttribute("aria-label", t("Keyboard entry"));
    this.#entryBtn.addEventListener("pointerdown", (e) => e.stopPropagation());
    // Length of new notes, right next to the piano
    this.#values = noteValuePicker();

    // Splitter at the bottom edge: drag a row larger/smaller individually, double-click = default height.
    const splitter = h("div", { class: "splitter", title: t("Drag: change track height · Double-click: default height") });
    splitter.addEventListener("pointerdown", (e) => {
      e.stopPropagation();
      startRowResize(this.#track, e, splitter);
    });
    splitter.addEventListener("dblclick", (e) => {
      e.stopPropagation();
      void resetRowHeight(this.#track);
    });
    // a key adds its note with keyboard entry on or the playhead at the end of the piece, else it sounds
    const pressKey = (offsetY: number, shift: boolean) => {
      if (this.#track.kind !== "instrument") return;
      pianoKey(this.id, pitchAt(geoFor(this.#track), offsetY + NOTES_TOP), shift);
    };
    // Touch: dragging along the keys shifts the pitch range (the mouse wheel's job), a tap plays the key
    let slide: { id: number; y0: number; low: number; moved: boolean } | null = null;
    this.#keys.addEventListener("pointerdown", (e) => {
      if (e.pointerType !== "touch") return pressKey(e.offsetY, e.shiftKey);
      e.stopPropagation();
      slide = { id: e.pointerId, y0: e.clientY, low: viewFor(this.#track).low, moved: false };
      try {
        this.#keys.setPointerCapture(e.pointerId);
      } catch {
        /* synthetic events */
      }
    });
    this.#keys.addEventListener("pointermove", (e) => {
      if (!slide || slide.id !== e.pointerId) return;
      const dy = e.clientY - slide.y0;
      if (Math.abs(dy) > 8) slide.moved = true;
      if (!slide.moved) return;
      const view = viewFor(this.#track);
      const lanes = Math.round(dy / geoFor(this.#track).laneH);
      // the keys follow the finger: high notes on top → dragging down shows higher notes
      scrollPitch(this.id, slide.low + (highOnTop.peek() ? lanes : -lanes) - view.low, view);
    });
    const slideEnd = (e: PointerEvent) => {
      if (!slide || slide.id !== e.pointerId) return;
      const tap = !slide.moved && e.type === "pointerup";
      slide = null;
      if (tap) pressKey(e.offsetY, false);
    };
    this.#keys.addEventListener("pointerup", slideEnd);
    this.#keys.addEventListener("pointercancel", slideEnd);

    this.el = h(
      "div",
      { class: "track-row" },
      h("div", { class: "stripe" }),
      h(
        "div",
        { class: "inner" },
        h("div", { class: "track-top" }, this.#kind, this.#name, this.#viewBtn, this.#exprBtn, this.#convertBtn, this.#mute, this.#solo, del),
        h(
          "div",
          { class: "track-controls" },
          h("div", { class: "knob" }, h("span", {}, t("Level"), this.#gainVal), this.#gain),
          h("div", { class: "knob pan" }, h("span", {}, t("Pan"), this.#panVal), this.#pan),
          stepper,
        ),
        h("div", { class: "track-entry" }, this.#entryBtn, this.#values.el, this.#values.compact),
      ),
      this.#keys,
      this.#rhythm,
      this.#exprHead,
      splitter,
    );
    this.el.dataset.trackId = String(id);
    this.el.addEventListener("pointerdown", (e) => {
      // right-click on a picked track keeps the pick (the menu acts on all of them),
      // and so does a control of it (its own M, S, level … stay individual)
      const mine = picked.peek().includes(this.id);
      if (e.button === 2 && mine) return;
      const control = (e.target as Element).closest("button, input, select, canvas.keys, .splitter");
      if (!(control && mine && !e.ctrlKey && !e.metaKey && !e.shiftKey)) pickTrack(this.id, e);
      if (e.ctrlKey || e.metaKey) return;
      const sel = selection.peek();
      if (sel?.trackId !== this.id) selection.value = { trackId: this.id, clipId: null };
    });
    this.el.addEventListener("contextmenu", (e) => {
      if ((e.target as Element).closest("input.text")) return; // (the name field keeps the browser's menu)
      e.preventDefault();
      e.stopPropagation();
      trackMenu(e.clientX, e.clientY, this.#track, () => this.rename());
    });
  }

  rename(): void {
    this.#name.focus();
    this.#name.select();
  }

  update(track: Track, selected: boolean, group: TrackGroup | null = null, pick = false): void {
    const kindChanged = !this.#track || this.#track.kind !== track.kind;
    this.#track = track;
    this.el.style.setProperty("--c", track.color);
    this.el.style.height = `${rowHeight(track)}px`;
    this.el.classList.toggle("selected", selected);
    this.el.classList.toggle("picked", pick);
    this.el.classList.toggle("in-group", !!group);
    this.el.classList.toggle("group-muted", !!group?.mute);
    if (group) this.el.style.setProperty("--gc", group.color);
    const notes = track.view === "notes";
    this.el.classList.toggle("notes", notes);
    // Swap the icon only on change: a redraw on press (track gets
    // selected) would otherwise replace the element under the mouse – the click would never arrive.
    const viewIcon = notes ? "wave" : "keys";
    if (this.#viewBtn.dataset.icon !== viewIcon) {
      this.#viewBtn.dataset.icon = viewIcon;
      this.#viewBtn.replaceChildren(icon(viewIcon));
    }
    this.#viewBtn.classList.toggle("on", notes);
    this.#viewBtn.title = notes ? t("Compact view") : track.kind === "instrument" ? t("Piano roll (editable)") : t("Show pitches (from the analysis)");
    const canExpr = notes && track.kind === "instrument";
    this.#exprBtn.hidden = !canExpr;
    this.#convertBtn.hidden = !(notes && track.kind === "audio" && track.clips.some((c) => c.source.type === "audio"));
    const typing = entryTrack.peek() === track.id;
    this.#entryBtn.hidden = !canExpr;
    this.#entryBtn.classList.toggle("on", typing);
    this.#entryBtn.setAttribute("aria-checked", String(typing));
    if (canExpr) this.#values.update();
    const exprOn = canExpr && exprShown(track);
    this.#exprBtn.classList.toggle("on", exprOn);
    this.#exprBtn.title = exprOn ? t("Hide expression lane") : t("Show expression lane (velocity, vibrato, brightness, bend per note)");
    this.el.classList.toggle("expr", exprOn);
    if (notes) {
      const g = geoFor(track);
      this.#keys.style.height = `${g.lanesH}px`;
      this.#rhythm.style.top = `${g.rhythmTop}px`;
      drawKeys(this.#keys, g, drumKeyNames(track), entryLetters(track));
      if (exprOn) {
        this.#exprHead.style.top = `${g.exprTop}px`;
        const keys = exprKeysFor(track);
        const current = laneKeyFor(track);
        const sig = keys.join(",");
        if (this.#exprSelect.dataset.keys !== sig) {
          this.#exprSelect.dataset.keys = sig;
          this.#exprSelect.replaceChildren(...keys.map((k) => h("option", { value: k }, EXPRS[k].label)));
        }
        this.#exprSelect.value = current;
        const d = EXPRS[current];
        this.#exprHint.textContent = d.bipolar ? "− · 0 · +" : current === "velocity" ? "pp … ff" : "0 … max";
        this.#exprHead.title = `${d.label}: ${d.hint}. ${t("Drag values or paint across several notes · Shift: straight line (e.g. crescendo) · Double-click: reset")}`;
      }
    }
    if (kindChanged) {
      this.#kind.replaceChildren(icon(track.kind === "audio" ? "wave" : "keys"));
      const kind = track.kind === "audio" ? t("Audio track") : t("Instrument track");
      this.#kind.title = `${kind} – ${t("drag to move it, also into or out of a group")}`;
    }
    if (document.activeElement !== this.#name) this.#name.value = track.name;
    this.#mute.classList.toggle("on", track.mute);
    this.#solo.classList.toggle("on", track.solo);
    setRange(this.#gain, track.gainDb);
    this.#gainVal.textContent = `${track.gainDb > 0 ? "+" : ""}${track.gainDb.toFixed(1)} dB`;
    setRange(this.#pan, track.pan);
    this.#panVal.textContent = panText(track.pan);
    const st = Math.round(track.pitchSemitones);
    this.#pitchVal.textContent = st === 0 ? "±0" : `${st > 0 ? "+" : ""}${st}`;
  }
}

/** A group's row: fold, name, number of tracks, level, mute and solo for all its tracks. */
class GroupRow {
  readonly el: HTMLDivElement;
  #group!: TrackGroup;
  #fold: HTMLButtonElement;
  #name: HTMLInputElement;
  #count: HTMLElement;
  #mute: HTMLButtonElement;
  #solo: HTMLButtonElement;
  #gain: HTMLInputElement;
  #gainVal: HTMLElement;

  constructor(readonly id: number) {
    const set = (patch: Partial<Omit<TrackGroup, "id">>, merge = false) => void setGroup(this.#group, patch, merge);
    this.#fold = h("button", { class: "btn ghost small icon group-fold", onclick: () => set({ collapsed: !this.#group.collapsed }) }, icon("chevron"));
    this.#name = h("input", { class: "text", spellcheck: false, title: t("Group name – click and type to rename") });
    this.#name.addEventListener("change", () => {
      const v = this.#name.value.trim();
      if (v) set({ name: v });
      else this.#name.value = this.#group.name;
    });
    this.#name.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter" || e.key === "Escape") this.#name.blur();
    });
    this.#count = h("span", { class: "group-count" });
    this.#mute = h("button", { class: "toggle m", title: t("Mute the group"), onclick: () => set({ mute: !this.#group.mute }) }, "M");
    this.#solo = h("button", { class: "toggle s", title: t("Solo the group"), onclick: () => set({ solo: !this.#group.solo }) }, "S");
    this.#gain = gestureRange(-60, 6, 0.5, (v, m) => set({ gainDb: v }, m), 0);
    this.#gain.title = t("Group level in dB – added to the level of each of its tracks · double-click: 0 dB");
    this.#gainVal = h("b", { class: "group-db" });
    this.el = h(
      "div",
      { class: "group-row" },
      h("div", { class: "stripe" }),
      h("div", { class: "inner" }, this.#fold, h("span", { class: "kind" }, icon("folder")), this.#name, this.#count, this.#gain, this.#gainVal, this.#mute, this.#solo),
    );
    this.el.dataset.groupId = String(id);
    this.el.addEventListener("contextmenu", (e) => {
      if ((e.target as Element).closest("input.text")) return;
      e.preventDefault();
      e.stopPropagation();
      groupMenu(e.clientX, e.clientY, this.#group, () => this.rename());
    });
    // double-click beside the controls: fold or unfold
    this.el.addEventListener("dblclick", (e) => {
      if ((e.target as Element).closest("input, button")) return;
      set({ collapsed: !this.#group.collapsed });
    });
  }

  rename(): void {
    this.#name.focus();
    this.#name.select();
  }

  update(g: TrackGroup, count: number, height: number): void {
    this.#group = g;
    this.el.style.setProperty("--c", g.color);
    this.el.style.height = `${height}px`;
    this.el.classList.toggle("collapsed", g.collapsed);
    this.el.classList.toggle("muted", g.mute);
    this.#fold.title = g.collapsed ? t("Unfold the group") : t("Fold the group");
    this.#fold.setAttribute("aria-expanded", String(!g.collapsed));
    if (document.activeElement !== this.#name) this.#name.value = g.name;
    this.#count.textContent = String(count);
    this.#count.title = plural(count, "{n} track", "{n} tracks");
    this.#mute.classList.toggle("on", g.mute);
    this.#solo.classList.toggle("on", g.solo);
    setRange(this.#gain, g.gainDb);
    this.#gainVal.textContent = `${g.gainDb > 0 ? "+" : ""}${g.gainDb.toFixed(1)} dB`;
  }
}

class KwTracks extends HTMLElement {
  #rows = new Map<number, TrackRow>();
  #groups = new Map<number, GroupRow>();
  #body = h("div", { class: "tracks-body" });
  #menu!: HTMLDivElement;

  connectedCallback(): void {
    if (this.childElementCount) return;
    // Narrow window (phone): the headers are a slim column; this unfolds them over the timeline
    const unfold = h("button", { class: "btn small icon heads-toggle", onclick: () => this.#unfold(!this.classList.contains("open")) }, icon("chevron"));
    this.#unfoldBtn = unfold;
    this.#unfold(false);
    // "+ label"; headers too narrow for the labels: an icon of its own instead (class "tight")
    const plusLabel = (alt: string, label: string) => {
      const plus = icon("plus");
      plus.classList.add("ic-plus");
      const other = icon(alt);
      other.classList.add("ic-alt");
      return [plus, other, h("span", { class: "lbl" }, label)];
    };
    const head = h(
      "div",
      { class: "tracks-head" },
      unfold,
      // (no "Tracks" label: the window's tab right above says it – the buttons need the room)
      h(
        "div",
        { class: "group" },
        harmonyToggle(),
        h("button", { class: "btn small", "data-help": "neue-audiospur", title: t("Add audio track"), onclick: () => addTrack("audio") }, ...plusLabel("wave", t("Audio"))),
        h("button", {
          class: "btn small",
          "data-help": "neue-instrumentspur",
          title: t("Add instrument track: pluck, organ, harpsichord, bass, synth or drums"),
          onclick: (e: MouseEvent) => {
            e.stopPropagation();
            // slim column: unfold first, so that the menu has room
            if (unfold.getClientRects().length) this.#unfold(true);
            this.#menu.classList.toggle("open");
          },
        }, ...plusLabel("keys", t("Instrument")), " ▾"),
        h("button", {
          class: "btn small",
          "data-help": "neue-gruppe",
          title: t("Group: puts the picked tracks (Ctrl+click, Shift+click) into a new group – without any, an empty one · Ctrl+G"),
          onclick: () => void groupTracks(),
        }, ...plusLabel("folder", t("Group"))),
        h("button", {
          class: "btn small icon view-toggle",
          "aria-label": t("What the track headers show"),
          title: t("What the track headers show: pan, pitch"),
          onclick: (e: MouseEvent) => {
            const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
            openContextMenu(r.left, r.bottom + 4, [
              { heading: t("Show in the track headers") },
              { label: t("Pan"), act: "zeige-pan", checked: showPan.peek(), action: toggleShowPan },
              { label: t("Pitch"), act: "zeige-pitch", checked: showPitch.peek(), action: toggleShowPitch },
            ]);
          },
        }, icon("layout")),
      ),
    );
    // labels only while they fit (width measured once with them)
    const buttons = head.querySelector<HTMLElement>(".group")!;
    let full = 0;
    new ResizeObserver(() => {
      if (!this.classList.contains("tight")) full = buttons.scrollWidth;
      const room = head.clientWidth - 18;
      this.classList.toggle("tight", full > 0 && full > room);
    }).observe(head);
    this.#menu = h("div", { class: "menu" });
    effect(() => {
      const specs = instrumentSpecs.value;
      const pick = (kind: InstrumentKind) => {
        this.#menu.classList.remove("open");
        void act(backend.createInstrumentTrack(kind));
      };
      this.#menu.replaceChildren(
        ...(specs.length
          ? specs.map((s) => h("button", { class: "menu-item", title: s.description, onclick: () => pick(s.kind) },
              h("b", {}, s.name), h("span", {}, s.kind === "drums" ? t("with beat") : s.kind === "bass" ? t("with bass line") : s.kind === "synth" ? t("with chord progression") : t("with arpeggio"))))
          : [h("button", { class: "menu-item", onclick: () => { this.#menu.classList.remove("open"); addTrack("instrument"); } }, h("b", {}, t("Instrument")), h("span", {}, t("Pluck")))]),
      );
    });
    document.addEventListener("click", () => this.#menu.classList.remove("open"));
    // a tap beside the unfolded headers folds them again
    document.addEventListener("pointerdown", (e) => {
      if (this.classList.contains("open") && !this.contains(e.target as Node)) this.#unfold(false);
    }, true);
    this.append(head, this.#menu, this.#body);

    effect(() => {
      instrumentSpecs.value; // drum names on the keyboard
      pitchViews.value; // keyboard follows pitch range and zoom
      heightDraft.value; // … and the row height while dragging
      highOnTop.value; // … and the direction of the pitch axis
      exprLanes.value; // … and the expression track
      entryTrack.value; // keyboard entry: switch and letters on the piano
      noteValue.value; // length of new notes next to the piano
      keyLabelsVersion.value;
      picked.value; // tracks picked for a group
      const p = snapshot.value?.project;
      this.#render(p ? layoutRows(p) : [], selection.value?.trackId);
    });
    // a track selected elsewhere (timeline) is the pick
    effect(() => {
      const s = selection.value?.trackId;
      if (s && !picked.peek().includes(s)) picked.value = [s];
    });
    effect(() => {
      this.classList.toggle("no-pan", !showPan.value);
      this.classList.toggle("no-pitch", !showPitch.value);
    });
    // width of the headers: a splitter at their right edge
    const splitter = h("div", { class: "heads-splitter", title: t("Drag: width of the track headers · Double-click: default width") });
    splitter.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      const x0 = e.clientX;
      const w0 = this.getBoundingClientRect().width;
      const move = (ev: PointerEvent) => setHeadsWidth(w0 + ev.clientX - x0);
      const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        document.body.classList.remove("col-resizing");
      };
      document.body.classList.add("col-resizing");
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
    });
    splitter.addEventListener("dblclick", () => setHeadsWidth(null));
    this.append(splitter);
    effect(() => {
      const w = headsWidth.value;
      const ws = this.closest<HTMLElement>(".workspace");
      if (!ws) return;
      if (w) ws.style.setProperty("--heads-w", `${Math.max(HEADS_MIN, Math.min(HEADS_MAX, w))}px`);
      else ws.style.removeProperty("--heads-w");
    });
    // drag a track by its icon (a finger swipes the list instead – the context menu moves it)
    this.#body.addEventListener("pointerdown", (e) => {
      const grip = (e.target as Element).closest(".track-row .kind");
      if (!grip || e.button !== 0 || e.pointerType === "touch") return;
      const id = Number((grip.closest(".track-row") as HTMLElement).dataset.trackId);
      e.preventDefault();
      this.#startDrag(id, e);
    });
    // right-click beside the tracks: add tracks, group, what the headers show
    this.addEventListener("contextmenu", (e) => {
      if ((e.target as Element).closest(".track-row, .group-row, .menu, input, select")) return;
      e.preventDefault();
      listMenu(e.clientX, e.clientY);
    });
    // Ctrl+G: group the picked tracks · Ctrl+Shift+G: dissolve the selected track's group
    window.addEventListener("keydown", (e) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.key.toLowerCase() !== "g") return;
      if ((e.target as Element | null)?.closest?.("input, textarea, select, [contenteditable]")) return;
      e.preventDefault();
      if (!e.shiftKey) return void groupTracks();
      const p = snapshot.peek()?.project;
      const tr = p?.tracks.find((x) => x.id === selection.peek()?.trackId);
      const g = tr?.group ? p?.groups?.find((x) => x.id === tr.group) : undefined;
      if (g) void run({ type: "removeGroup", groupId: g.id });
    });
    effect(() => {
      this.#body.style.transform = `translateY(${-viewport.value.scrollY}px)`;
    });
    // dragged audio files over a track header: highlight this track
    effect(() => {
      const d = audioDrag.value;
      const over = d ? document.elementFromPoint(d.x, d.y)?.closest(".track-row") : null;
      for (const row of this.#rows.values()) row.el.classList.toggle("drop-target", row.el === over);
    });
    this.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const max = maxScrollY(snapshot.peek()?.project);
        viewport.update((v) => ({ ...v, scrollY: Math.max(0, Math.min(max, v.scrollY + e.deltaY)) }));
      },
      { passive: false },
    );
    // Touch: a finger on the track headers scrolls them – and the timeline – up and down
    let swipe: { id: number; y0: number; scrollY0: number; moved: boolean } | null = null;
    this.addEventListener("pointerdown", (e) => {
      if (e.pointerType !== "touch" || swipe) return;
      // (sliders, menus and the keys keep the finger; a swipe may start on a name or a button)
      if ((e.target as Element).closest('input[type="range"], select, .splitter, .heads-splitter, canvas.keys, .menu')) return;
      swipe = { id: e.pointerId, y0: e.clientY, scrollY0: viewport.peek().scrollY, moved: false };
    }, true); // (capture: also from buttons that keep their press to themselves, like the keyboard switch)
    this.addEventListener("pointermove", (e) => {
      if (!swipe || swipe.id !== e.pointerId) return;
      const dy = e.clientY - swipe.y0;
      if (Math.abs(dy) > 8) swipe.moved = true;
      if (!swipe.moved) return;
      const max = maxScrollY(snapshot.peek()?.project);
      const y = Math.max(0, Math.min(max, swipe.scrollY0 - dy));
      viewport.update((v) => ({ ...v, scrollY: y }));
    });
    const swipeEnd = (e: PointerEvent) => {
      if (swipe?.id !== e.pointerId) return;
      if (swipe.moved) {
        // the button the swipe started on is not pressed
        const swallow = (c: Event) => {
          c.stopPropagation();
          c.preventDefault();
        };
        this.addEventListener("click", swallow, { capture: true, once: true });
        setTimeout(() => this.removeEventListener("click", swallow, { capture: true }), 400);
      }
      swipe = null;
    };
    this.addEventListener("pointerup", swipeEnd);
    this.addEventListener("pointercancel", swipeEnd);
  }

  #unfoldBtn: HTMLButtonElement | null = null;

  #unfold(open: boolean): void {
    this.classList.toggle("open", open);
    if (this.#unfoldBtn) this.#unfoldBtn.title = open ? t("Fold the track headers") : t("Unfold the track headers: level, pan and all buttons");
  }

  #render(rows: Row[], selectedId: number | undefined): void {
    const pick = new Set(picked.peek());
    const seenTracks = new Set<number>();
    const seenGroups = new Set<number>();
    rows.forEach((r, i) => {
      let el: HTMLElement;
      if (r.kind === "group") {
        let row = this.#groups.get(r.group.id);
        if (!row) {
          row = new GroupRow(r.group.id);
          this.#groups.set(r.group.id, row);
        }
        row.update(r.group, r.tracks.length, r.h);
        seenGroups.add(r.group.id);
        el = row.el;
      } else {
        let row = this.#rows.get(r.track.id);
        if (!row) {
          row = new TrackRow(r.track.id);
          this.#rows.set(r.track.id, row);
        }
        row.update(r.track, r.track.id === selectedId, r.group, pick.has(r.track.id));
        seenTracks.add(r.track.id);
        el = row.el;
      }
      if (this.#body.children[i] !== el) this.#body.insertBefore(el, this.#body.children[i] ?? null);
    });
    // (tracks of a folded group are taken out too – they come back when it unfolds)
    for (const [id, row] of this.#rows) {
      if (!seenTracks.has(id)) {
        row.el.remove();
        if (!snapshot.peek()?.project.tracks.some((tr) => tr.id === id)) this.#rows.delete(id);
      }
    }
    for (const [id, row] of this.#groups) {
      if (!seenGroups.has(id)) {
        row.el.remove();
        this.#groups.delete(id);
      }
    }
  }

  /** Where a track dragged to `clientY` would land, and where the drop line goes (body coordinates). */
  #dropAt(clientY: number): { place: DropPlace; y: number; inside: boolean } | null {
    const p = snapshot.peek()?.project;
    if (!p) return null;
    const y = clientY - this.#body.getBoundingClientRect().top;
    const rows = layoutRows(p);
    const r = rows.find((row) => y >= row.top && y < row.top + row.h);
    if (!r) {
      const end = rows.reduce((s, row) => s + row.h, 0);
      return { place: { before: null, after: null, group: 0 }, y: y < 0 ? 0 : end, inside: false };
    }
    const lower = y >= r.top + r.h / 2;
    if (r.kind === "track") {
      const group = r.group?.id ?? 0;
      return lower
        ? { place: { before: null, after: r.track.id, group }, y: r.top + r.h, inside: !!group }
        : { place: { before: r.track.id, after: null, group }, y: r.top, inside: !!group };
    }
    const first = r.tracks[0];
    if (!lower) return { place: { before: first?.id ?? null, after: null, group: 0 }, y: r.top, inside: false };
    // lower half of a group's row: into the group (at the top; folded: at the end)
    return r.group.collapsed || !first
      ? { place: { before: null, after: r.tracks.at(-1)?.id ?? null, group: r.group.id }, y: r.top + r.h, inside: true }
      : { place: { before: first.id, after: null, group: r.group.id }, y: r.top + r.h, inside: true };
  }

  /** Dragging a track by its icon: a line shows where it goes; dropping moves it (into or out of groups). */
  #startDrag(trackId: number, e: PointerEvent): void {
    const y0 = e.clientY;
    let moving = false;
    let place: DropPlace | null = null;
    const line = h("div", { class: "drop-line" });
    const move = (ev: PointerEvent) => {
      if (!moving && Math.abs(ev.clientY - y0) < 4) return;
      if (!moving) {
        moving = true;
        this.#body.append(line);
        this.classList.add("dragging");
      }
      const d = this.#dropAt(ev.clientY);
      place = d?.place ?? null;
      if (d) {
        line.style.top = `${d.y}px`;
        line.classList.toggle("inside", d.inside);
      }
    };
    const end = (drop: boolean) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      line.remove();
      this.classList.remove("dragging");
      if (drop && moving && place) void moveTrackTo(trackId, place);
    };
    const onUp = () => end(true);
    const onCancel = () => end(false);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
  }
}

customElements.define("kw-tracks", KwTracks);
