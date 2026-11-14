// Timeline: ruler on top, the tracks below.
// Ruler: dragging pans the view, click sets the playhead,
// Shift/Ctrl+drag marks a time range (loop), edges can
// be dragged. Tracks: click selects notes, dragging on empty space draws
// a selection box, selected notes can be moved (with
// bar subdivision); only Alt+click places a note. Right-click opens a
// context menu (align/transpose notes, convert recording, delete
// time range). Audio clips carry the button "♪ → Notes"; dragged audio files
// show where they land (filedrop.ts).

import { startTour } from "./tour.js";
import { chordName } from "../spelling.js";
import { backend } from "../bridge.js";
import {
  addTrack, analyzeSelected, convertAudio, deleteRange, deleteSelectedNotes, detectTempo, loadDemo, quantizeSelectedNotes, resetRowHeight,
  seekBeats, startRowResize, toggleRecord, transposeSelectedNotes,
} from "../actions.js";
import { type MenuEntry, openContextMenu } from "../ctxmenu.js";
import { dropdown, fitCanvas, formatBarBeat, h, icon } from "../dom.js";
import { type DropTarget, audioDrag } from "../filedrop.js";
import { fillBeatMenu } from "./rhythm.js";
import { fillSongMenu } from "./song.js";
import { MOOD_COLORS, MOOD_LABELS, chordIndexAt, harmonyFocus, harmonyLaneOn, harmonyLine, watchHarmonyLine } from "../harmonyline.js";
import {
  NOTES_TOP, RHYTHM_H, type NoteGeo, analysisFor, analysisState, contentHeight, geoFor, heightDraft, highOnTop, inView, isBlack, laneY,
  maxScrollY, noteGrid, noteKey, noteSel, offViewAtTop, pitchAt, pitchDelta, pitchOfLane, pitchViews, rowHeight, rowTops,
  scrollPitch, viewFor, wheelPitchStep, exprAt, exprY,
} from "../pianoroll.js";
import { plural, t } from "../i18n.js";
import { effect } from "../signals.js";
import { RULER_H, act, clipLengthBeats, counterpointMarks, rangeFocus, run, selection, snapshot, specFor, status, viewport } from "../store.js";
import type { Clip, Command, Note, Project, Track } from "../types.js";
import { EXPRS, type ExprKey, exprKeysFor, exprLanes, exprValue, laneKeyFor, withExpr } from "../expression.js";
import { addNoteAt, entry, entryStep, entryTrack, keyLabel, keyLabelsVersion, setEntryPosition } from "../noteinput.js";
import { clipSpan, fmtShift, gridLabel, gridOf, rangeGrid, rangeLabel, snapTo } from "../editmath.js";

const WAVE_BUCKETS = 2400;
const waves = new Map<string, Float32Array | "loading">();
/** Length of the most recently placed note (new notes adopt it). */
let lastNoteLength = 1;

function rgba(hex: string, a: number): string {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}

type Drag =
  | {
      /** Drag ruler: pan the view (without movement: set playhead). */
      kind: "pan";
      startX: number;
      scrollX0: number;
      moved: boolean;
    }
  | {
      /** Mark a time range or drag an edge. */
      kind: "range";
      anchor: number;
      cur: number;
      moved: boolean;
      free: boolean;
    }
  | {
      /** Selection box over notes of a track. */
      kind: "marquee";
      trackId: number;
      /** Clip under the starting point (otherwise the one with the most hits). */
      clipId: number | null;
      x0: number;
      y0: number;
      x: number;
      y: number;
      /** Shift/Ctrl: add to the existing selection. */
      base: { clipId: number; keys: string[] } | null;
      moved: boolean;
    }
  | { kind: "clip"; trackId: number; clipId: number; startX: number; origBeats: number; beats: number; moved: boolean; curX: number; curY: number }
  | {
      kind: "notes";
      mode: "move" | "resize";
      trackId: number;
      clipId: number;
      startX: number;
      startY: number;
      /** Pointer position (for the position display while moving). */
      curX: number;
      curY: number;
      /** Notes before the gesture (for a new note including this one). */
      orig: Note[];
      /** Indices in `orig` that are moved or changed in length. */
      active: number[];
      /** Index of the grabbed note. */
      grabbed: number;
      draft: Note[];
      moved: boolean;
      created: boolean;
      lastPitch: number;
      /** Lane height at the start of the gesture (for the y → semitones conversion). */
      laneH: number;
    }
  | {
      /** Expression track: drag/paint values, with Shift a line (crescendo). */
      kind: "expr";
      trackId: number;
      key: ExprKey;
      line: boolean;
      startX: number;
      startY: number;
      lastX: number;
      curX: number;
      curY: number;
      /** Note lists per clip before or during the gesture. */
      orig: Map<number, Note[]>;
      draft: Map<number, Note[]>;
      /** Edit only these notes (selection); if a clip is missing, all are free. */
      only: Map<number, Set<number>> | null;
    }
  | {
      /** Alt+drag on a note: change velocity (with Shift: vibrato) directly. */
      kind: "noteExpr";
      trackId: number;
      clipId: number;
      key: ExprKey;
      startY: number;
      curX: number;
      curY: number;
      orig: Note[];
      active: number[];
      grabbed: number;
      draft: Note[];
      moved: boolean;
    };

interface Hit {
  track: Track;
  index: number;
  rowY: number;
  clip: Clip | null;
  /** Area within the note row. */
  zone: "band" | "lanes" | "rhythm" | "expr" | "compact";
  note: number; // index of the hit note or -1
  edge: boolean; // right edge of a note (change length)
  pitch: number;
  beatInClip: number;
  /** Position in the project (beats). */
  beat: number;
}

class KwTimeline extends HTMLElement {
  #canvas = h("canvas");
  #hint!: HTMLDivElement;
  /** Shortcut line, always visible right below the last track (keyboard entry: its keys). */
  #keysHint = h("div", { class: "keys-hint" });
  #keysHintMode = "";
  #raf = 0;
  #drag: Drag | null = null;
  #width = 0;
  /** "♪ → Notes" buttons of the audio clips (collected while drawing, timeline coordinates). */
  #noteButtons: Array<{ x: number; y: number; w: number; h: number; trackId: number; clipId: number }> = [];

  connectedCallback(): void {
    if (this.childElementCount) return;
    this.#hint = h(
      "div",
      { class: "empty-hint" },
      h("h2", {}, t("Empty project")),
      h("div", {}, t("Load the demo, start with a song or beat, create a track, record straight from the microphone – or drag audio files (WAV, AIFF, FLAC, Opus) here.")),
      h(
        "div",
        { class: "row" },
        h("button", { class: "btn intro-btn", title: t("Tour of all windows – with notes on the most important features"), onclick: () => startTour() }, icon("help"), t("Intro")),
        h("button", { class: "btn primary", title: t("Load an example project with beat, arpeggio and melody – to try things out"), onclick: () => loadDemo() }, icon("sparkle"), t("Load demo")),
        dropdown(h("button", { class: "btn song-start", title: t("Create a complete piece from a template (trance, ambient…) – keep shaping it in the \"Song\" window") }, icon("song"), `${t("Start with a song")} ▾`) as HTMLButtonElement, "beat-menu song-menu", fillSongMenu),
        dropdown(h("button", { class: "btn beat-start", title: t("Create a genre's beat template as a drum track") }, icon("grid"), `${t("Start with a beat")} ▾`) as HTMLButtonElement, "beat-menu", fillBeatMenu),
        h("button", { class: "btn", title: t("Create an empty instrument track (pluck) – add notes with Alt+click"), onclick: () => addTrack("instrument") }, icon("keys"), t("Instrument track")),
        h("button", { class: "btn rec", title: t("Record from the microphone (R) – creates an audio track"), onclick: () => toggleRecord() }, icon("mic"), t("Record")),
      ),
    );
    this.append(this.#canvas, this.#hint, this.#keysHint);

    new ResizeObserver(() => this.#invalidate()).observe(this);
    effect(() => {
      const p = snapshot.value?.project;
      selection.value;
      viewport.value;
      noteSel.value;
      pitchViews.value;
      heightDraft.value;
      highOnTop.value;
      exprLanes.value;
      counterpointMarks.value;
      harmonyLine.value;
      harmonyLaneOn.value;
      harmonyFocus.value;
      audioDrag.value;
      entry.value;
      this.#hint.style.display = p && p.tracks.length === 0 ? "" : "none";
      this.#updateKeysHint(entryTrack.value !== null, keyLabelsVersion.value);
      this.#invalidate();
    });
    effect(() => {
      const st = status.value;
      this.#follow(st.positionSecs, st.playing);
      this.#invalidate();
    });

    watchHarmonyLine();
    this.#canvas.addEventListener("pointerdown", (e) => this.#down(e));
    this.#canvas.addEventListener("pointermove", (e) => this.#move(e));
    this.#canvas.addEventListener("pointerup", (e) => this.#up(e));
    this.#canvas.addEventListener("pointercancel", () => (this.#drag = null));
    this.#canvas.addEventListener("contextmenu", (e) => this.#context(e));
    this.#canvas.addEventListener("dblclick", (e) => this.#dblclick(e));
    this.addEventListener("wheel", (e) => this.#wheel(e), { passive: false });
  }

  /** Text of the shortcut line: the usual shortcuts, or the keys of keyboard entry. */
  #updateKeysHint(typing: boolean, labels: number): void {
    const mode = `${typing}:${labels}`;
    if (mode === this.#keysHintMode) return;
    this.#keysHintMode = mode;
    this.#keysHint.classList.toggle("entry", typing);
    this.#keysHint.innerHTML = typing
      ? t("Keyboard entry: <kbd>{first}</kbd>…<kbd>{last}</kbd> notes · <kbd>Shift</kbd> chord · <kbd>{down}</kbd> <kbd>{up}</kbd> octave · <kbd>←</kbd> <kbd>→</kbd> step · <kbd>Backspace</kbd> back · <kbd>Esc</kbd> end", {
          first: keyLabel("KeyA"), last: keyLabel("KeyK"), down: keyLabel("KeyZ"), up: keyLabel("KeyX"),
        })
      : t("<kbd>Space</kbd> Play · <kbd>L</kbd> Loop · <kbd>R</kbd> Record · <kbd>Ctrl</kbd>+mouse wheel Zoom · <kbd>Alt</kbd>+click Add note · <kbd>Del</kbd> Delete");
  }

  /** Target for dragged audio files at a position (window coordinates); null outside. */
  dropTargetAt(cx: number, cy: number): DropTarget | null {
    const r = this.#canvas.getBoundingClientRect();
    if (cx < r.left || cx >= r.right || cy < r.top || cy >= r.bottom) return null;
    const x = cx - r.left;
    const y = cy - r.top;
    const startBeats = Math.max(0, snapTo(this.#xToBeat(x), this.#ppb() > 70 ? 0.25 : 1));
    const hit = y >= RULER_H ? this.#hit(x, y) : null;
    return { trackId: hit?.track.id ?? null, startBeats };
  }

  /** Position of the "♪ → Notes" button of an audio clip in window coordinates (null: not visible). */
  noteButtonRect(clipId: number): { x: number; y: number; w: number; h: number } | null {
    const b = this.#noteButtons.find((n) => n.clipId === clipId);
    if (!b) return null;
    const r = this.#canvas.getBoundingClientRect();
    return { x: r.left + b.x, y: r.top + b.y, w: b.w, h: b.h };
  }

  #noteButtonAt(x: number, y: number): { trackId: number; clipId: number } | null {
    for (let i = this.#noteButtons.length - 1; i >= 0; i--) {
      const b = this.#noteButtons[i]!;
      if (x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h) return b;
    }
    return null;
  }

  // ---------- Coordinates ----------
  #ppb(): number {
    return viewport.peek().pxPerBeat;
  }
  #beatToX(b: number): number {
    return b * this.#ppb() - viewport.peek().scrollX;
  }
  #xToBeat(x: number): number {
    return (x + viewport.peek().scrollX) / this.#ppb();
  }

  #rows(p: Project): Array<{ track: Track; y: number; h: number }> {
    const tops = rowTops(p.tracks);
    const sy = viewport.peek().scrollY;
    return p.tracks.map((track, i) => ({ track, y: RULER_H + tops[i]! - sy, h: rowHeight(track) }));
  }

  #clipNotes(c: Clip): Note[] {
    const d = this.#drag;
    if ((d?.kind === "notes" || d?.kind === "noteExpr") && d.clipId === c.id) return d.draft;
    if (d?.kind === "expr") {
      const draft = d.draft.get(c.id);
      if (draft) return draft;
    }
    return c.source.type === "notes" ? c.source.notes : [];
  }

  #hit(x: number, y: number): Hit | null {
    const p = snapshot.peek()?.project;
    if (!p || y < RULER_H) return null;
    const rows = this.#rows(p);
    const index = rows.findIndex((r) => y >= r.y && y < r.y + r.h);
    if (index < 0) return null;
    const { track, y: rowY } = rows[index]!;
    const beat = this.#xToBeat(x);
    let clip: Clip | null = null;
    for (let k = track.clips.length - 1; k >= 0; k--) {
      const c = track.clips[k]!;
      if (beat >= c.startBeats && beat <= c.startBeats + clipLengthBeats(c, p.tempoBpm)) {
        clip = c;
        break;
      }
    }
    const hit: Hit = { track, index, rowY, clip, zone: "compact", note: -1, edge: false, pitch: 0, beatInClip: 0, beat };
    if (track.view !== "notes") return hit;
    const rel = y - rowY;
    const g = geoFor(track);
    hit.zone = rel < NOTES_TOP ? "band" : g.exprH && rel >= g.exprTop ? "expr" : rel >= g.rhythmTop ? "rhythm" : "lanes";
    hit.pitch = Math.max(0, Math.min(127, pitchAt(g, rel)));
    if (clip && hit.zone === "lanes" && clip.source.type === "notes") {
      hit.beatInClip = beat - clip.startBeats;
      const notes = this.#clipNotes(clip);
      for (let n = notes.length - 1; n >= 0; n--) {
        const note = notes[n]!;
        if (note.pitch !== hit.pitch) continue;
        if (hit.beatInClip >= note.startBeats && hit.beatInClip <= note.startBeats + note.lengthBeats) {
          hit.note = n;
          const right = this.#beatToX(clip.startBeats + note.startBeats + note.lengthBeats);
          hit.edge = right - x <= 6 && note.lengthBeats * this.#ppb() > 10;
          break;
        }
      }
    }
    return hit;
  }

  /** Track whose bottom edge (splitter) is at this y position (±4 px). */
  #splitterAt(y: number): Track | null {
    const p = snapshot.peek()?.project;
    if (!p || y < RULER_H) return null;
    const row = this.#rows(p).find((r) => Math.abs(r.y + r.h - y) <= 4);
    return row?.track ?? null;
  }

  // ---------- Interaction ----------
  #down(e: PointerEvent): void {
    if (e.button === 2) return; // right-click: context action
    const split = this.#splitterAt(e.offsetY);
    if (split) {
      startRowResize(split, e, this.#canvas);
      return;
    }
    const btn = this.#noteButtonAt(e.offsetX, e.offsetY);
    if (btn) {
      selection.value = { trackId: btn.trackId, clipId: btn.clipId };
      void convertAudio(btn.trackId, btn.clipId, "melodie");
      return;
    }
    try {
      this.#canvas.setPointerCapture(e.pointerId);
    } catch {
      /* e.g. synthetic events – the gesture works anyway */
    }
    if (e.offsetY < RULER_H) {
      this.#rulerDown(e);
      return;
    }
    const hit = this.#hit(e.offsetX, e.offsetY);
    if (!hit) {
      selection.value = null;
      noteSel.value = null;
      return;
    }
    if (hit.zone === "expr") {
      this.#beginExprGesture(e, hit);
      return;
    }
    selection.value = { trackId: hit.track.id, clipId: hit.clip?.id ?? null };

    const editable = hit.track.kind === "instrument" && hit.zone === "lanes" && hit.clip?.source.type === "notes";
    if (editable && hit.clip && hit.note >= 0 && e.altKey) {
      this.#beginNoteExpr(e, hit, hit.clip);
      return;
    }
    if (editable && hit.clip && hit.note >= 0) {
      this.#beginNoteGesture(e, hit, hit.clip);
      return;
    }
    if (hit.track.kind === "instrument" && hit.zone === "lanes") {
      // Only Alt+click places a note, otherwise you draw a selection box.
      // Next to a clip the note goes into the nearest clip of the track, which
      // extends to whole bars on release; only without a clip is a new one created.
      if (e.altKey) {
        if (editable && hit.clip) this.#beginNoteCreate(e, hit, hit.clip);
        else if (!hit.clip) {
          const near = this.#nearestNotesClip(hit.track, hit.beat);
          if (near) {
            selection.value = { trackId: hit.track.id, clipId: near.id };
            this.#beginNoteCreate(e, { ...hit, clip: near, beatInClip: hit.beat - near.startBeats }, near);
          } else this.#createClipWithNote(hit);
        }
        return;
      }
      this.#beginMarquee(e, hit);
      return;
    }
    if (noteSel.peek()?.clipId !== hit.clip?.id) noteSel.value = null;
    if (hit.clip && (hit.zone === "band" || hit.zone === "compact" || hit.track.kind === "audio")) {
      this.#drag = {
        kind: "clip", trackId: hit.track.id, clipId: hit.clip.id, startX: e.offsetX,
        origBeats: hit.clip.startBeats, beats: hit.clip.startBeats, moved: false, curX: e.offsetX, curY: e.offsetY,
      };
    }
  }

  // ---------- Ruler ----------
  #rangeShown(): [number, number] | null {
    const d = this.#drag;
    if (d?.kind === "range") return d.moved && Math.abs(d.cur - d.anchor) > 1e-6 ? [Math.min(d.anchor, d.cur), Math.max(d.anchor, d.cur)] : null;
    return snapshot.peek()?.engine.loopRange ?? null;
  }

  /** Edge of the marked range under x (±5 px): 0 = start, 1 = end. */
  #rangeEdgeAt(x: number): 0 | 1 | null {
    const r = snapshot.peek()?.engine.loopRange;
    if (!r) return null;
    if (Math.abs(x - this.#beatToX(r[0])) <= 5) return 0;
    if (Math.abs(x - this.#beatToX(r[1])) <= 5) return 1;
    return null;
  }

  #rulerDown(e: PointerEvent): void {
    const range = snapshot.peek()?.engine.loopRange ?? null;
    const mod = e.shiftKey || e.ctrlKey || e.metaKey;
    const edge = mod ? null : this.#rangeEdgeAt(e.offsetX);
    if (range && edge != null) {
      // grab an edge: the other stays put
      this.#drag = { kind: "range", anchor: range[1 - edge]!, cur: range[edge]!, moved: true, free: e.altKey };
      return;
    }
    if (mod) {
      const bpb = snapshot.peek()?.project.beatsPerBar ?? 4;
      const b = Math.max(0, snapTo(this.#xToBeat(e.offsetX), rangeGrid(this.#ppb(), bpb, e.altKey)));
      this.#drag = { kind: "range", anchor: b, cur: b, moved: false, free: e.altKey };
      return;
    }
    this.#drag = { kind: "pan", startX: e.offsetX, scrollX0: viewport.peek().scrollX, moved: false };
  }

  // ---------- Notes ----------
  /** Selection box: all notes in the rectangle (time × pitch) of a clip of the track. */
  #beginMarquee(e: PointerEvent, hit: Hit): void {
    const additive = e.shiftKey || e.ctrlKey || e.metaKey;
    const sel = noteSel.peek();
    const base = additive && sel && sel.trackId === hit.track.id ? { clipId: sel.clipId, keys: sel.keys } : null;
    if (!additive) noteSel.value = null;
    this.#drag = {
      kind: "marquee", trackId: hit.track.id, clipId: hit.clip?.source.type === "notes" ? hit.clip.id : null,
      x0: e.offsetX, y0: e.offsetY, x: e.offsetX, y: e.offsetY, base, moved: false,
    };
  }

  #marqueeSelect(): void {
    const d = this.#drag;
    const p = snapshot.peek()?.project;
    if (d?.kind !== "marquee" || !p) return;
    const t = p.tracks.find((tr) => tr.id === d.trackId);
    const row = t ? this.#rows(p).find((r) => r.track.id === t.id) : undefined;
    if (!t || !row) return;
    const g = geoFor(t);
    const clampRel = (y: number) => Math.max(NOTES_TOP, Math.min(NOTES_TOP + g.lanesH - 1, y - row.y));
    const pa = pitchAt(g, clampRel(d.y0));
    const pb = pitchAt(g, clampRel(d.y));
    const [lo, hi] = [Math.min(pa, pb), Math.max(pa, pb)];
    const b0 = this.#xToBeat(Math.min(d.x0, d.x));
    const b1 = this.#xToBeat(Math.max(d.x0, d.x));
    let best: { clipId: number; keys: string[] } | null = null;
    for (const c of t.clips) {
      if (c.source.type !== "notes" || (d.clipId != null && c.id !== d.clipId)) continue;
      const keys = c.source.notes
        .filter((n) => n.pitch >= lo && n.pitch <= hi && c.startBeats + n.startBeats < b1 && c.startBeats + n.startBeats + n.lengthBeats > b0)
        .map(noteKey);
      if (!best || keys.length > best.keys.length) best = { clipId: c.id, keys };
    }
    if (!best) return;
    let keys = best.keys;
    if (d.base && d.base.clipId === best.clipId) keys = [...new Set([...d.base.keys, ...keys])];
    else if (d.base && !keys.length) {
      noteSel.value = { trackId: t.id, ...d.base };
      return;
    }
    noteSel.value = keys.length ? { trackId: t.id, clipId: best.clipId, keys } : null;
    selection.value = { trackId: t.id, clipId: best.clipId };
  }

  /** Nearest note clip of the track to a position (distance to the clip edge; on a tie the left one). */
  #nearestNotesClip(t: Track, beat: number): Clip | null {
    const p = snapshot.peek()?.project;
    if (!p) return null;
    let best: { c: Clip; d: number } | null = null;
    for (const c of t.clips) {
      if (c.source.type !== "notes") continue;
      const end = c.startBeats + clipLengthBeats(c, p.tempoBpm);
      const d = beat < c.startBeats ? c.startBeats - beat : beat > end ? beat - end : 0;
      if (!best || d < best.d - 1e-9) best = { c, d };
    }
    return best?.c ?? null;
  }

  /** Track without a note clip: Alt+click creates a clip (1 bar) with this note. */
  #createClipWithNote(hit: Hit): void {
    const p = snapshot.peek()?.project;
    if (!p) return;
    const bpb = p.beatsPerBar;
    const at = Math.max(0, hit.beat);
    const bar = Math.floor(at / bpb) * bpb;
    const grid = noteGrid(this.#ppb(), false);
    const start = grid ? Math.floor((at - bar) / grid) * grid : at - bar;
    const len = Math.max(lastNoteLength, grid || 0.0625);
    const note: Note = { pitch: hit.pitch, startBeats: start, lengthBeats: len, velocity: 100 };
    void backend.previewNote(hit.track.id, hit.pitch).catch(() => {});
    void run({
      type: "addClip",
      trackId: hit.track.id,
      clip: { id: 0, name: t("Notes"), startBeats: bar, source: { type: "notes", lengthBeats: Math.max(1, Math.ceil((start + len) / bpb)) * bpb, notes: [note] } },
    });
  }

  #beginNoteGesture(e: PointerEvent, hit: Hit, clip: Clip): void {
    const notes = clip.source.type === "notes" ? clip.source.notes : [];
    const sel = noteSel.peek();
    const selKeys = new Set(sel && sel.clipId === clip.id ? sel.keys : []);
    const base = {
      kind: "notes" as const, trackId: hit.track.id, clipId: clip.id, startX: e.offsetX, startY: e.offsetY, curX: e.offsetX, curY: e.offsetY,
      moved: false, laneH: geoFor(hit.track).laneH,
    };
    // Click selects the note, Shift/Ctrl+click adds it to or removes it from the selection;
    // dragging moves all selected notes, at the right edge it changes the length.
    const key = noteKey(notes[hit.note]!);
    if (e.shiftKey || e.ctrlKey || e.metaKey) {
      selKeys.has(key) ? selKeys.delete(key) : selKeys.add(key);
    } else if (!selKeys.has(key)) {
      selKeys.clear();
      selKeys.add(key);
    }
    noteSel.value = { trackId: hit.track.id, clipId: clip.id, keys: [...selKeys] };
    const active = hit.edge ? [hit.note] : notes.map((n, i) => (selKeys.has(noteKey(n)) ? i : -1)).filter((i) => i >= 0);
    this.#drag = { ...base, mode: hit.edge ? "resize" : "move", orig: notes, active, grabbed: hit.note, draft: notes, created: false, lastPitch: hit.pitch };
    void backend.previewNote(hit.track.id, notes[hit.note]!.pitch).catch(() => {});
  }

  /** Alt+click on an empty lane in the clip: new note, dragging to the right sets the length. */
  #beginNoteCreate(e: PointerEvent, hit: Hit, clip: Clip): void {
    const notes = clip.source.type === "notes" ? clip.source.notes : [];
    const base = {
      kind: "notes" as const, trackId: hit.track.id, clipId: clip.id, startX: e.offsetX, startY: e.offsetY, curX: e.offsetX, curY: e.offsetY,
      moved: false, laneH: geoFor(hit.track).laneH,
    };
    const grid = noteGrid(this.#ppb(), e.shiftKey);
    // also before the clip start (negative) – just not before the project start
    const start = Math.max(-clip.startBeats, grid ? Math.floor(hit.beatInClip / grid) * grid : hit.beatInClip);
    const created: Note = { pitch: hit.pitch, startBeats: start, lengthBeats: Math.max(lastNoteLength, grid || 0.0625), velocity: 100 };
    const orig = [...notes, created];
    const idx = orig.length - 1;
    noteSel.value = { trackId: hit.track.id, clipId: clip.id, keys: [noteKey(created)] };
    this.#drag = { ...base, mode: "resize", orig, active: [idx], grabbed: idx, draft: orig, created: true, lastPitch: hit.pitch };
    void backend.previewNote(hit.track.id, hit.pitch).catch(() => {});
    this.#invalidate();
  }

  /** Alt+drag on a note: velocity, with Shift vibrato – for the note or the whole selection. */
  #beginNoteExpr(e: PointerEvent, hit: Hit, clip: Clip): void {
    const notes = clip.source.type === "notes" ? clip.source.notes : [];
    const sel = noteSel.peek();
    const selKeys = new Set(sel && sel.clipId === clip.id ? sel.keys : []);
    const key = noteKey(notes[hit.note]!);
    if (!selKeys.has(key)) {
      selKeys.clear();
      selKeys.add(key);
      noteSel.value = { trackId: hit.track.id, clipId: clip.id, keys: [key] };
    }
    const active = notes.map((n, i) => (selKeys.has(noteKey(n)) ? i : -1)).filter((i) => i >= 0);
    const exprKey: ExprKey = e.shiftKey && exprKeysFor(hit.track).includes("vibrato") ? "vibrato" : "velocity";
    this.#drag = {
      kind: "noteExpr", trackId: hit.track.id, clipId: clip.id, key: exprKey, startY: e.offsetY, curX: e.offsetX, curY: e.offsetY,
      orig: notes, active, grabbed: hit.note, draft: notes, moved: false,
    };
    this.#invalidate();
  }

  /** Expression track: set/paint value; Shift: straight line across several notes (e.g. crescendo). */
  #beginExprGesture(e: PointerEvent, hit: Hit): void {
    const t = hit.track;
    selection.value = { trackId: t.id, clipId: hit.clip?.id ?? null };
    const orig = new Map<number, Note[]>();
    for (const c of t.clips) if (c.source.type === "notes") orig.set(c.id, c.source.notes);
    // With a note selection in this track: change only the selected notes.
    const sel = noteSel.peek();
    let only: Map<number, Set<number>> | null = null;
    if (sel && sel.trackId === t.id && sel.keys.length) {
      const notes = orig.get(sel.clipId) ?? [];
      const keys = new Set(sel.keys);
      only = new Map([[sel.clipId, new Set(notes.map((n, i) => (keys.has(noteKey(n)) ? i : -1)).filter((i) => i >= 0))]]);
    }
    this.#drag = {
      kind: "expr", trackId: t.id, key: laneKeyFor(t), line: e.shiftKey,
      startX: e.offsetX, startY: e.offsetY, lastX: e.offsetX, curX: e.offsetX, curY: e.offsetY,
      orig, draft: new Map(orig), only,
    };
    this.#paintExpr(e.offsetX, e.offsetY);
  }

  /** Notes of the track whose start lies between x0 and x1: [clip, index, x]. */
  #notesBetween(t: Track, x0: number, x1: number, only: Map<number, Set<number>> | null): Array<[Clip, number, number]> {
    const lo = Math.min(x0, x1);
    const hi = Math.max(x0, x1);
    const out: Array<[Clip, number, number]> = [];
    for (const c of t.clips) {
      if (c.source.type !== "notes") continue;
      if (only && !only.has(c.id)) continue;
      const allowed = only?.get(c.id);
      c.source.notes.forEach((n, i) => {
        if (allowed && !allowed.has(i)) return;
        const x = this.#beatToX(c.startBeats + n.startBeats);
        if (x >= lo && x <= hi) out.push([c, i, x]);
      });
    }
    return out;
  }

  #paintExpr(x: number, y: number): void {
    const d = this.#drag;
    if (d?.kind !== "expr") return;
    const p = snapshot.peek()?.project;
    const t = p?.tracks.find((tr) => tr.id === d.trackId);
    const row = p && t ? this.#rows(p).find((r) => r.track.id === t.id) : undefined;
    if (!t || !row) return;
    const g = geoFor(t);
    const def = EXPRS[d.key];
    const valueAt = (yy: number) => exprAt(g, def, yy - row.y);
    d.curX = x;
    d.curY = y;
    if (d.line) {
      // Straight line from the starting point to the pointer: notes in between follow the line.
      d.draft = new Map(d.orig);
      const v0 = valueAt(d.startY);
      const v1 = valueAt(y);
      // 5 px tolerance at the ends, so that notes exactly at the line start/end belong to it.
      const tol = x >= d.startX ? 5 : -5;
      for (const [c, i, nx] of this.#notesBetween(t, d.startX - tol, x + tol, d.only)) {
        const f = x === d.startX ? 0 : Math.max(0, Math.min(1, (nx - d.startX) / (x - d.startX)));
        const notes = d.draft.get(c.id) === d.orig.get(c.id) ? d.orig.get(c.id)!.slice() : d.draft.get(c.id)!;
        notes[i] = withExpr(notes[i]!, d.key, v0 + (v1 - v0) * f);
        d.draft.set(c.id, notes);
      }
    } else {
      // Painting: all notes whose start the pointer has swept over since the last step.
      const v = valueAt(y);
      const from = d.lastX === x ? x - 6 : d.lastX;
      const to = d.lastX === x ? x + 6 : x;
      for (const [c, i] of this.#notesBetween(t, from, to, d.only)) {
        const cur = d.draft.get(c.id)!;
        const notes = cur === d.orig.get(c.id) ? cur.slice() : cur;
        notes[i] = withExpr(notes[i]!, d.key, v);
        d.draft.set(c.id, notes);
      }
      d.lastX = x;
    }
    this.#invalidate();
  }

  #move(e: PointerEvent): void {
    const d = this.#drag;
    if (!d) {
      this.#canvas.style.cursor = this.#cursorFor(e);
      this.#canvas.title = this.#noteButtonAt(e.offsetX, e.offsetY)
        ? t("Convert to notes: create the detected pitches as a melody track below (right-click: also as a rhythm track)")
        : this.#harmonyTip(e);
      return;
    }
    if (d.kind === "pan") {
      // grab the ruler: time moves with the mouse
      const dx = e.offsetX - d.startX;
      if (Math.abs(dx) > 3) d.moved = true;
      if (!d.moved) return;
      viewport.update((v) => ({ ...v, scrollX: Math.max(0, d.scrollX0 - dx) }));
      this.#canvas.style.cursor = "grabbing";
      return;
    }
    if (d.kind === "range") {
      const bpb = snapshot.peek()?.project.beatsPerBar ?? 4;
      const b = Math.max(0, snapTo(this.#xToBeat(e.offsetX), rangeGrid(this.#ppb(), bpb, d.free || e.altKey)));
      if (Math.abs(b - d.anchor) > 1e-6) d.moved = true;
      d.cur = b;
      this.#canvas.style.cursor = "ew-resize";
      this.#invalidate();
      return;
    }
    if (d.kind === "marquee") {
      d.x = e.offsetX;
      d.y = e.offsetY;
      if (Math.abs(d.x - d.x0) > 3 || Math.abs(d.y - d.y0) > 3) d.moved = true;
      if (d.moved) this.#marqueeSelect();
      this.#invalidate();
      return;
    }
    if (d.kind === "notes" || d.kind === "clip") {
      d.curX = e.offsetX;
      d.curY = e.offsetY;
    }
    if (d.kind === "expr") {
      this.#paintExpr(e.offsetX, e.offsetY);
      return;
    }
    if (d.kind === "noteExpr") {
      const dy = d.startY - e.offsetY; // up = more
      if (Math.abs(dy) > 2) d.moved = true;
      d.curX = e.offsetX;
      d.curY = e.offsetY;
      if (!d.moved) return;
      const def = EXPRS[d.key];
      const perPx = (def.max - def.min) / 160; // 160 px = whole range
      d.draft = d.orig.slice();
      for (const i of d.active) d.draft[i] = withExpr(d.orig[i]!, d.key, exprValue(d.orig[i]!, d.key) + dy * perPx);
      this.#canvas.style.cursor = "ns-resize";
      this.#invalidate();
      return;
    }
    const dx = e.offsetX - d.startX;
    if (d.kind === "clip") {
      if (Math.abs(dx) > 3) d.moved = true;
      if (!d.moved) return;
      const raw = Math.max(0, d.origBeats + dx / this.#ppb());
      const grid = e.shiftKey ? 0 : this.#ppb() > 70 ? 0.25 : 1;
      d.beats = snapTo(raw, grid);
      this.#canvas.style.cursor = "grabbing";
      this.#invalidate();
      return;
    }

    // Notes
    const dy = e.offsetY - d.startY;
    if (Math.abs(dx) > 2 || Math.abs(dy) > 2) d.moved = true;
    if (!d.moved) return;
    const grid = noteGrid(this.#ppb(), e.shiftKey);
    const dBeatsRaw = dx / this.#ppb();
    if (d.mode === "move") {
      const dPitch = pitchDelta(d, dy); // direction follows the pitch axis setting
      let dBeats = snapTo(dBeatsRaw, grid);
      // also before the clip start (the clip then grows to the left), not before the project start
      const minStart = Math.min(...d.active.map((i) => d.orig[i]!.startBeats));
      const clipStart = snapshot.peek()?.project.tracks.find((t) => t.id === d.trackId)?.clips.find((c) => c.id === d.clipId)?.startBeats ?? 0;
      dBeats = Math.max(dBeats, -(clipStart + minStart));
      const activeSet = new Set(d.active);
      d.draft = d.orig.map((n, i) =>
        activeSet.has(i) ? { ...n, startBeats: n.startBeats + dBeats, pitch: Math.max(0, Math.min(127, n.pitch + dPitch)) } : n,
      );
      const p = d.draft[d.grabbed]!.pitch;
      if (p !== d.lastPitch) {
        d.lastPitch = p;
        void backend.previewNote(d.trackId, p).catch(() => {});
      }
      this.#canvas.style.cursor = "grabbing";
    } else {
      const n = d.orig[d.grabbed]!;
      // new note: ends at the pointer; existing: move the edge by the drag distance
      const clipStart = snapshot.peek()?.project.tracks.find((t) => t.id === d.trackId)?.clips.find((c) => c.id === d.clipId)?.startBeats ?? 0;
      const end = d.created ? snapTo(this.#xToBeat(e.offsetX) - clipStart, grid) : snapTo(n.startBeats + n.lengthBeats + dBeatsRaw, grid);
      const len = Math.max(grid || 0.0625, end - n.startBeats);
      d.draft = d.orig.map((x, i) => (i === d.grabbed ? { ...x, lengthBeats: len } : x));
      this.#canvas.style.cursor = "ew-resize";
    }
    this.#invalidate();
  }

  #up(e: PointerEvent): void {
    const d = this.#drag;
    this.#drag = null;
    try {
      if (this.#canvas.hasPointerCapture(e.pointerId)) this.#canvas.releasePointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
    if (d?.kind === "pan" && !d.moved) seekBeats(this.#xToBeat(e.offsetX));
    if (d?.kind === "range") {
      // Dragging sets the range, Shift/Ctrl+click without dragging clears it
      const r: [number, number] | null = d.moved && Math.abs(d.cur - d.anchor) > 1e-6 ? [Math.min(d.anchor, d.cur), Math.max(d.anchor, d.cur)] : null;
      void act(backend.setLoopRange(r));
      // last marked: the time range – Delete removes it (not the note or clip selection)
      rangeFocus.value = !!r;
      if (r) noteSel.value = null;
    }
    if (d?.kind === "marquee" && d.moved) this.#marqueeSelect();
    if (d?.kind === "marquee" && !d.moved && entry.peek()) {
      // keyboard entry: a click into the piano roll puts the entry position there
      const step = entryStep();
      setEntryPosition(Math.floor(this.#xToBeat(d.x0) / step + 1e-9) * step, d.trackId);
    }
    if (d?.kind === "clip" && d.moved && Math.abs(d.beats - d.origBeats) > 1e-9) {
      void run({ type: "setClip", trackId: d.trackId, clipId: d.clipId, patch: { startBeats: d.beats } });
    }
    if (d?.kind === "notes" && (d.moved || d.created)) {
      this.#commitNotes(d.trackId, d.clipId, d.draft, d.active.map((i) => d.draft[i]!));
      if (d.mode === "resize") lastNoteLength = d.draft[d.grabbed]!.lengthBeats;
    }
    if (d?.kind === "noteExpr" && d.moved) {
      void run({ type: "setClipNotes", trackId: d.trackId, clipId: d.clipId, notes: d.draft });
      noteSel.value = { trackId: d.trackId, clipId: d.clipId, keys: d.active.map((i) => noteKey(d.draft[i]!)) };
    }
    if (d?.kind === "expr") {
      const commands: Command[] = [];
      for (const [clipId, notes] of d.draft) {
        if (notes !== d.orig.get(clipId)) commands.push({ type: "setClipNotes", trackId: d.trackId, clipId, notes });
      }
      const label = d.line ? t("{expr} as a line", { expr: EXPRS[d.key].label }) : t("{expr} drawn", { expr: EXPRS[d.key].label });
      if (commands.length === 1) void run(commands[0]!);
      else if (commands.length > 1) void run({ type: "batch", label, commands });
    }
    this.#invalidate();
  }

  /**
   * Adopt a new note list. If notes lie outside the clip, it grows in
   * whole bars: to the right (length) or to the left (start earlier, notes
   * shift by the same amount) – all one undo step.
   */
  #commitNotes(trackId: number, clipId: number, notes: Note[], selected: Note[]): void {
    const p = snapshot.peek()?.project;
    const clip = p?.tracks.find((t) => t.id === trackId)?.clips.find((c) => c.id === clipId);
    if (!p || !clip || clip.source.type !== "notes") return;
    const span = clipSpan(notes, clip.source.lengthBeats, p.beatsPerBar, clip.startBeats);
    const shift = -span.lo;
    const moved = shift ? notes.map((n) => ({ ...n, startBeats: n.startBeats + shift })) : notes;
    const sel = shift ? selected.map((n) => ({ ...n, startBeats: n.startBeats + shift })) : selected;
    const lengthBeats = span.hi - span.lo > clip.source.lengthBeats + 1e-9 ? span.hi - span.lo : undefined;
    const setNotes = { type: "setClipNotes" as const, trackId, clipId, notes: moved, ...(lengthBeats ? { lengthBeats } : {}) };
    noteSel.value = sel.length ? { trackId, clipId, keys: sel.map(noteKey) } : null;
    if (shift) {
      void run({ type: "batch", label: t("Notes (clip extended to the left)"), commands: [{ type: "setClip", trackId, clipId, patch: { startBeats: clip.startBeats - shift } }, setNotes] });
    } else {
      void run(setNotes);
    }
  }

  /**
   * Right-click: context menu for whatever is under the mouse – selected
   * notes (an unselected note is selected first), an audio clip,
   * the marked time range; otherwise the clip.
   */
  #context(e: MouseEvent): void {
    e.preventDefault();
    const p = snapshot.peek()?.project;
    if (!p) return;
    const entries: MenuEntry[] = [];
    const add = (more: MenuEntry[]) => {
      if (entries.length && more.length) entries.push("sep");
      entries.push(...more);
    };
    const range = snapshot.peek()?.engine.loopRange ?? null;
    const beat = this.#xToBeat(e.offsetX);
    const hit = e.offsetY >= RULER_H ? this.#hit(e.offsetX, e.offsetY) : null;
    const clip = hit?.clip ?? null;
    if (hit && clip?.source.type === "notes" && hit.track.kind === "instrument" && hit.zone === "lanes") {
      if (hit.note >= 0) {
        const key = noteKey(clip.source.notes[hit.note]!);
        const sel = noteSel.peek();
        if (!(sel && sel.clipId === clip.id && sel.keys.includes(key))) noteSel.value = { trackId: hit.track.id, clipId: clip.id, keys: [key] };
        selection.value = { trackId: hit.track.id, clipId: clip.id };
      }
      const now = noteSel.peek();
      if (now && now.clipId === clip.id && now.keys.length) add(this.#notesMenu(now.keys.length, p));
    }
    if (hit && clip?.source.type === "audio") {
      selection.value = { trackId: hit.track.id, clipId: clip.id };
      add(this.#audioMenu(hit.track, clip));
    }
    if (range && (e.offsetY < RULER_H || (beat >= range[0] && beat <= range[1]))) add(this.#rangeMenu(range, p.beatsPerBar));
    if (hit && clip && !entries.length) {
      selection.value = { trackId: hit.track.id, clipId: clip.id };
      add([{ heading: clip.name }, { label: t("Delete clip"), hint: t("Del"), act: "clip-loeschen", action: () => void run({ type: "removeClip", trackId: hit.track.id, clipId: clip.id }) }]);
    }
    if (entries.length) openContextMenu(e.clientX, e.clientY, entries);
  }

  /** Menu for selected notes: align to bar (grid selectable), transpose, delete. */
  #notesMenu(count: number, p: Project): MenuEntry[] {
    const unit = p.beatUnit || 4;
    const view = noteGrid(this.#ppb(), false) || 0.25;
    const chips = (lengths: boolean) => [
      ...[4, 8, 16].map((d) => ({
        label: `1/${d}`,
        act: `raster-${d}${lengths ? "-laenge" : ""}`,
        title: d === 4 ? t("Snap to quarters") : d === 8 ? t("Snap to eighths") : t("Snap to sixteenths"),
        action: () => void quantizeSelectedNotes(gridOf(d, unit), lengths),
      })),
      {
        label: t("Grid"),
        act: `raster-ansicht${lengths ? "-laenge" : ""}`,
        title: t("View grid: {grid} (finer the further you zoom in)", { grid: gridLabel(view, unit) }),
        action: () => void quantizeSelectedNotes(view, lengths),
      },
    ];
    return [
      { heading: plural(count, "{n} note selected", "{n} notes selected") },
      { label: t("Quantize"), act: "ausrichten", title: t("Starts move to the nearest grid line – lengths stay"), chips: chips(false) },
      { label: t("Start and length"), act: "ausrichten-laenge", title: t("Start and end move to the grid (every note at least one grid step long)"), chips: chips(true) },
      {
        label: t("Transpose"),
        act: "transponieren",
        title: t("Shift in semitones – faster with ↑ / ↓ (Shift: octave)"),
        chips: [[-12, t("−Oct")], [-1, "−1"], [1, "+1"], [12, t("+Oct")]].map(([d, label]) => ({
          label: String(label),
          act: `transponieren${Number(d) > 0 ? "+" : ""}${d}`,
          title: Math.abs(Number(d)) === 12
            ? Number(d) > 0 ? t("One octave up (Shift+↑)") : t("One octave down (Shift+↓)")
            : Number(d) > 0 ? t("One semitone up (↑)") : t("One semitone down (↓)"),
          action: () => void transposeSelectedNotes(Number(d)),
        })),
      },
      { label: t("Delete"), hint: t("Del"), act: "noten-loeschen", action: () => void deleteSelectedNotes() },
    ];
  }

  /** Menu for an audio clip: convert to a melody or rhythm track, analysis, tempo, delete. */
  #audioMenu(track: Track, c: Clip): MenuEntry[] {
    return [
      { heading: c.name },
      { label: t("Convert to melody track"), hint: t("Pitches → notes"), act: "melodie", title: t("The detected pitches as an instrument track right below (quantized to sixteenths, low registers as bass)"), action: () => void convertAudio(track.id, c.id, "melodie") },
      { label: t("Convert to rhythm track"), hint: t("Hits → drums"), act: "rhythmus", title: t("The detected hits as a drum track right below: kick, snare and hi-hat by timbre"), action: () => void convertAudio(track.id, c.id, "rhythmus") },
      "sep",
      { label: t("Show analysis"), act: "analyse", action: () => void analyzeSelected() },
      { label: t("Detect tempo"), act: "tempo", title: t("Estimate the source tempo – the clip then follows the project tempo"), action: () => void detectTempo() },
      { label: t("Delete clip"), hint: t("Del"), act: "clip-loeschen", action: () => void run({ type: "removeClip", trackId: track.id, clipId: c.id }) },
    ];
  }

  /** Menu for the marked time range. */
  #rangeMenu(range: [number, number], bpb: number): MenuEntry[] {
    return [
      { heading: t("Selected: {range}", { range: rangeLabel(range[0], range[1], bpb) }) },
      { label: t("Delete time range"), hint: t("Del"), act: "bereich-loeschen", title: t("Delete from all tracks – everything after it moves up, clips are split or shortened"), action: () => void deleteRange() },
      { label: t("Clear selection"), act: "bereich-aufheben", action: () => void act(backend.setLoopRange(null)) },
    ];
  }

  #dblclick(e: MouseEvent): void {
    const split = this.#splitterAt(e.offsetY);
    if (split) {
      void resetRowHeight(split);
      return;
    }
    const hit = this.#hit(e.offsetX, e.offsetY);
    if (!hit) return;
    if (hit.zone === "expr") {
      // Double-click in the expression track: reset the value of the nearest note(s).
      const key = laneKeyFor(hit.track);
      const near = this.#notesBetween(hit.track, e.offsetX - 8, e.offsetX + 8, null);
      const byClip = new Map<number, Note[]>();
      for (const [c, i] of near) {
        const notes = byClip.get(c.id) ?? (c.source.type === "notes" ? c.source.notes.slice() : []);
        notes[i] = withExpr(notes[i]!, key, EXPRS[key].neutral);
        byClip.set(c.id, notes);
      }
      for (const [clipId, notes] of byClip) void run({ type: "setClipNotes", trackId: hit.track.id, clipId, notes });
      return;
    }
    if (hit.clip?.source.type === "audio") {
      void analyzeSelected();
      return;
    }
    // Double-click on an empty lane of the piano roll: a note at this spot – start on
    // the grid, length as with Alt+click; outside a clip like Alt+click, too.
    if (hit.zone === "lanes" && hit.track.kind === "instrument") {
      if (hit.note >= 0) return;
      const grid = noteGrid(this.#ppb(), e.shiftKey);
      const ref = hit.clip?.startBeats ?? 0;
      const beat = grid ? ref + Math.floor((hit.beat - ref) / grid + 1e-9) * grid : hit.beat;
      void backend.previewNote(hit.track.id, hit.pitch).catch(() => {});
      void addNoteAt(hit.track.id, beat, hit.pitch, Math.max(lastNoteLength, grid || 0.0625));
      return;
    }
    // Double-click on empty space of an instrument track: create a new clip (1 bar).
    const p = snapshot.peek()?.project;
    if (!hit.clip && hit.track.kind === "instrument" && p) {
      const bar = Math.floor(this.#xToBeat(e.offsetX) / p.beatsPerBar) * p.beatsPerBar;
      void run({
        type: "addClip",
        trackId: hit.track.id,
        clip: { id: 0, name: t("Notes"), startBeats: Math.max(0, bar), source: { type: "notes", lengthBeats: p.beatsPerBar, notes: [] } },
      });
    }
  }

  #cursorFor(e: PointerEvent): string {
    if (e.offsetY < RULER_H) {
      if (e.shiftKey || e.ctrlKey || e.metaKey) return "text";
      return this.#rangeEdgeAt(e.offsetX) != null ? "ew-resize" : "grab";
    }
    if (this.#splitterAt(e.offsetY)) return "row-resize";
    if (this.#noteButtonAt(e.offsetX, e.offsetY)) return "pointer";
    const hit = this.#hit(e.offsetX, e.offsetY);
    if (!hit) return "default";
    if (hit.zone === "expr") return "crosshair";
    if (hit.zone === "lanes" && hit.track.kind === "instrument") {
      if (hit.note >= 0) return e.altKey ? "ns-resize" : hit.edge ? "ew-resize" : "move";
      // empty lane: Alt = place note, otherwise selection box
      return e.altKey ? "copy" : "crosshair";
    }
    return hit.clip ? "grab" : "default";
  }

  #wheel(e: WheelEvent): void {
    e.preventDefault();
    const rect = this.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const p = snapshot.peek()?.project;
    // Alt + mouse wheel over a note row: shift the pitch range.
    if (e.altKey && p) {
      const hit = this.#hit(mx, my);
      if (hit?.track.view === "notes") {
        scrollPitch(hit.track.id, wheelPitchStep(e.deltaY), viewFor(hit.track));
        return;
      }
    }
    const max = maxScrollY(p?.tracks ?? []);
    viewport.update((v) => {
      if (e.ctrlKey || e.metaKey) {
        const ppb = Math.min(400, Math.max(1.5, v.pxPerBeat * Math.exp(-e.deltaY * 0.0015)));
        const beatAtMouse = (mx + v.scrollX) / v.pxPerBeat;
        return { ...v, pxPerBeat: ppb, scrollX: Math.max(0, beatAtMouse * ppb - mx) };
      }
      if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        return { ...v, scrollX: Math.max(0, v.scrollX + (e.deltaX || e.deltaY)) };
      }
      return { ...v, scrollY: Math.max(0, Math.min(max, v.scrollY + e.deltaY)) };
    });
  }

  #follow(secs: number, playing: boolean): void {
    if (!playing || this.#drag || !this.#width) return;
    const bpm = snapshot.peek()?.project.tempoBpm ?? 120;
    const x = this.#beatToX((secs * bpm) / 60);
    if (x > this.#width * 0.88 || x < 0) {
      viewport.update((v) => ({ ...v, scrollX: Math.max(0, v.scrollX + x - this.#width * 0.15) }));
    }
  }

  #invalidate(): void {
    if (this.#raf) return;
    this.#raf = requestAnimationFrame(() => {
      this.#raf = 0;
      this.#draw();
    });
  }

  #wave(assetId: string): Float32Array | null {
    const w = waves.get(assetId);
    if (w instanceof Float32Array) return w;
    if (!w) {
      waves.set(assetId, "loading");
      backend
        .getWaveform(assetId, WAVE_BUCKETS)
        .then((d) => {
          waves.set(assetId, Float32Array.from(d));
          this.#invalidate();
        })
        .catch(() => waves.delete(assetId));
    }
    return null;
  }

  /** What is currently being moved (notes or clip): range, new position, distance, grid. */
  #moving(p: Project): { start: number; end: number; at: number; shift: number; pitch: number; trackId: number; grid: number; x: number; y: number } | null {
    const d = this.#drag;
    if (d?.kind === "notes" && d.mode === "move" && d.moved) {
      const clip = p.tracks.find((t) => t.id === d.trackId)?.clips.find((c) => c.id === d.clipId);
      if (!clip) return null;
      const now = d.active.map((i) => d.draft[i]!);
      const g = d.draft[d.grabbed]!;
      const o = d.orig[d.grabbed]!;
      return {
        start: clip.startBeats + Math.min(...now.map((n) => n.startBeats)),
        end: clip.startBeats + Math.max(...now.map((n) => n.startBeats + n.lengthBeats)),
        at: clip.startBeats + g.startBeats, shift: g.startBeats - o.startBeats, pitch: g.pitch - o.pitch,
        trackId: d.trackId, grid: noteGrid(this.#ppb(), false), x: d.curX, y: d.curY,
      };
    }
    if (d?.kind === "clip" && d.moved) {
      const clip = p.tracks.find((t) => t.id === d.trackId)?.clips.find((c) => c.id === d.clipId);
      if (!clip) return null;
      return {
        start: d.beats, end: d.beats + clipLengthBeats(clip, p.tempoBpm), at: d.beats, shift: d.beats - d.origBeats, pitch: 0,
        trackId: d.trackId, grid: this.#ppb() > 70 ? 0.25 : 1, x: d.curX, y: d.curY,
      };
    }
    return null;
  }

  /** Small box with text at the pointer (value while dragging). */
  #tip(ctx: CanvasRenderingContext2D, x0: number, y0: number, text: string, border = "rgba(255, 180, 84, 0.7)"): void {
    ctx.font = "600 11.5px 'Segoe UI', system-ui, sans-serif";
    const tw = ctx.measureText(text).width;
    const x = Math.min(x0 + 14, this.#width - tw - 22);
    const y = Math.max(RULER_H + 4, y0 - 30);
    ctx.fillStyle = "rgba(20, 24, 33, 0.94)";
    ctx.strokeStyle = border;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.roundRect(x, y, tw + 16, 22, 6);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = "#f1e3cc";
    ctx.textBaseline = "middle";
    ctx.fillText(text, x + 8, y + 11.5);
  }

  // ---------- Drawing ----------
  #draw(): void {
    const { ctx, w, h: hh } = fitCanvas(this.#canvas);
    this.#width = w;
    ctx.fillStyle = "#0d0f14";
    ctx.fillRect(0, 0, w, hh);
    this.#noteButtons = [];
    const p = snapshot.peek()?.project;
    if (!p) return;
    const sel = selection.peek();
    const ppb = this.#ppb();
    const bpb = p.beatsPerBar;
    const rows = this.#rows(p);

    // shortcut line right below the last track – at the bottom edge when the tracks go further
    const below = RULER_H + contentHeight(p.tracks) - viewport.peek().scrollY + 8;
    const hintTop = `${Math.round(Math.max(RULER_H + 4, Math.min(below, hh - this.#keysHint.offsetHeight - 6)))}px`;
    if (this.#keysHint.style.top !== hintTop) this.#keysHint.style.top = hintTop;

    // Track rows (note rows with pitch lanes)
    rows.forEach(({ track: t, y, h: rh }, i) => {
      if (y > hh || y + rh < RULER_H) return;
      if (t.view === "notes") {
        this.#drawLanes(ctx, t, y, w);
      } else {
        ctx.fillStyle = sel?.trackId === t.id ? "#131824" : i % 2 ? "#0f1219" : "#0d1016";
        ctx.fillRect(0, y, w, rh);
      }
      ctx.fillStyle = "#1a202b";
      ctx.fillRect(0, y + rh - 1, w, 1);
    });

    // Grid
    const first = Math.max(0, Math.floor(this.#xToBeat(0)));
    const last = Math.ceil(this.#xToBeat(w));
    const barStep = ppb * bpb < 40 ? Math.ceil(40 / (ppb * bpb)) : 1;
    for (let b = first; b <= last; b++) {
      const isBar = b % bpb === 0;
      if (!isBar && ppb < 14) continue;
      // fully zoomed out: only every n-th bar line, otherwise the grid turns into a solid area
      if (isBar && ppb * bpb < 12 && (b / bpb) % barStep) continue;
      const x = Math.round(this.#beatToX(b)) + 0.5;
      ctx.fillStyle = isBar ? "#262d3b" : "#171c26";
      ctx.fillRect(x, RULER_H, 1, hh);
      if (ppb > 90) {
        ctx.fillStyle = "#12161e";
        for (let s = 1; s < 4; s++) ctx.fillRect(Math.round(this.#beatToX(b + s / 4)) + 0.5, RULER_H, 1, hh);
      }
    }

    // While moving: bar subdivision clearly visible – bars, beats and, in the
    // moved track, the snap grid; the new range slightly highlighted.
    const mv = this.#moving(p);
    if (mv) {
      const mvRow = rows.find((r) => r.track.id === mv.trackId);
      const xs = this.#beatToX(mv.start);
      const xe = this.#beatToX(mv.end);
      ctx.fillStyle = "rgba(255, 180, 84, 0.05)";
      ctx.fillRect(xs, RULER_H, xe - xs, hh - RULER_H);
      for (let b = first; b <= last; b++) {
        const isBar = b % bpb === 0;
        if (!isBar && ppb < 8) continue;
        const x = Math.round(this.#beatToX(b)) + 0.5;
        ctx.fillStyle = isBar ? "rgba(255, 255, 255, 0.30)" : "rgba(255, 255, 255, 0.11)";
        ctx.fillRect(x, RULER_H, isBar ? 1.5 : 1, hh);
        if (mvRow && mv.grid > 0 && mv.grid < 1 && ppb * mv.grid >= 6) {
          ctx.fillStyle = "rgba(255, 255, 255, 0.06)";
          for (let s = mv.grid; s < 1 - 1e-9; s += mv.grid) ctx.fillRect(Math.round(this.#beatToX(b + s)) + 0.5, mvRow.y, 1, mvRow.h);
        }
      }
    }

    // Clips
    for (const { track: t, y, h: rh } of rows) {
      if (y > hh || y + rh < RULER_H) continue;
      for (const c of t.clips) {
        if (t.view === "notes") this.#drawNotesClip(ctx, p, t, c, y, w, sel?.clipId === c.id);
        else this.#drawClip(ctx, p, t, c, y, w, sel?.clipId === c.id);
      }
    }

    this.#drawExprTip(ctx, p);
    this.#drawDrop(ctx, p, rows, w, hh);

    // Marked time range (teal: will be repeated, blue: only marked)
    const range = this.#rangeShown();
    const loopOn = !!snapshot.peek()?.engine.loopOn;
    const rangeX = range ? [this.#beatToX(range[0]), this.#beatToX(range[1])] as const : null;
    if (rangeX) {
      const [x0, x1] = rangeX;
      ctx.fillStyle = loopOn ? "rgba(110, 231, 216, 0.06)" : "rgba(122, 167, 255, 0.05)";
      ctx.fillRect(x0, RULER_H, x1 - x0, hh - RULER_H);
      ctx.fillStyle = loopOn ? "rgba(110, 231, 216, 0.55)" : "rgba(122, 167, 255, 0.45)";
      ctx.fillRect(Math.round(x0), RULER_H, 1, hh);
      ctx.fillRect(Math.round(x1), RULER_H, 1, hh);
    }

    // Selection box
    const dm = this.#drag;
    if (dm?.kind === "marquee" && dm.moved) {
      const row = rows.find((r) => r.track.id === dm.trackId);
      if (row) {
        const top = row.y + NOTES_TOP;
        const bottom = top + geoFor(row.track).lanesH;
        const cy = (y: number) => Math.max(top, Math.min(bottom, y));
        const x0 = Math.min(dm.x0, dm.x);
        const y0 = cy(Math.min(dm.y0, dm.y));
        const rw = Math.abs(dm.x - dm.x0);
        const rh = cy(Math.max(dm.y0, dm.y)) - y0;
        ctx.save();
        ctx.fillStyle = "rgba(110, 231, 216, 0.08)";
        ctx.fillRect(x0, y0, rw, rh);
        ctx.strokeStyle = "rgba(110, 231, 216, 0.85)";
        ctx.setLineDash([4, 3]);
        ctx.lineWidth = 1;
        ctx.strokeRect(x0 + 0.5, y0 + 0.5, rw, rh);
        ctx.restore();
      }
    }

    // Keyboard entry: where the next typed note goes (one grid step wide, line at its start)
    const en = entry.peek();
    const enRow = en ? rows.find((r) => r.track.id === en.trackId) : undefined;
    const enX = en ? this.#beatToX(en.beat) : 0;
    if (en && enRow) {
      const x1 = this.#beatToX(en.beat + entryStep());
      const top = Math.max(RULER_H, enRow.y);
      const bottom = enRow.y + enRow.h - 1;
      if (x1 >= 0 && enX <= w && bottom > top) {
        ctx.fillStyle = "rgba(199, 146, 255, 0.12)";
        ctx.fillRect(enX, top, x1 - enX, bottom - top);
        ctx.fillStyle = "rgba(199, 146, 255, 0.95)";
        ctx.fillRect(Math.round(enX) - 1, top, 2, bottom - top);
      }
    }

    // Position while moving (also as an attribute – for tests and screen readers)
    if (mv) {
      const pitch = mv.pitch ? ` · ${t("{delta} st", { delta: `${mv.pitch > 0 ? "+" : "−"}${Math.abs(mv.pitch)}` })}` : "";
      const label = `${t("Bar {bar}", { bar: formatBarBeat(mv.at, bpb) })}   ${fmtShift(mv.shift, bpb)}${pitch}`;
      this.#tip(ctx, mv.x, mv.y, label);
      this.dataset.dragInfo = label;
    } else if (this.dataset.dragInfo) {
      delete this.dataset.dragInfo;
    }

    // Spots found by the counterpoint checker: tint in the tracks involved
    const marks = counterpointMarks.peek();
    const markColor = (s: string, a: number) => (s === "fehler" ? `rgba(255, 93, 108, ${a})` : s === "warnung" ? `rgba(255, 180, 84, ${a})` : `rgba(122, 167, 255, ${a})`);
    for (const m of marks) {
      const x0 = this.#beatToX(m.start);
      const x1 = Math.max(x0 + 3, this.#beatToX(m.end));
      if (x1 < 0 || x0 > w) continue;
      for (const r of rows) {
        if (!m.trackIds.includes(r.track.id) || r.y > hh || r.y + r.h < RULER_H) continue;
        ctx.fillStyle = markColor(m.severity, m.severity === "hinweis" ? 0.06 : 0.12);
        ctx.fillRect(x0, Math.max(RULER_H, r.y), x1 - x0, r.h);
      }
    }

    // Ruler
    ctx.fillStyle = "#121620";
    ctx.fillRect(0, 0, w, RULER_H);
    ctx.fillStyle = "#262d3b";
    ctx.fillRect(0, RULER_H - 1, w, 1);
    if (rangeX) {
      const [x0, x1] = rangeX;
      ctx.fillStyle = loopOn ? "rgba(110, 231, 216, 0.22)" : "rgba(122, 167, 255, 0.16)";
      ctx.fillRect(x0, 0, x1 - x0, RULER_H - 1);
      ctx.fillStyle = loopOn ? "#6ee7d8" : "#7aa7ff";
      ctx.fillRect(x0, 0, x1 - x0, 3);
      ctx.fillRect(x0 - 1, 0, 2, RULER_H - 1);
      ctx.fillRect(x1 - 1, 0, 2, RULER_H - 1);
    }
    if (mv) {
      // new range of the moved notes or clip in the ruler
      const xs = this.#beatToX(mv.start);
      ctx.fillStyle = "rgba(255, 180, 84, 0.22)";
      ctx.fillRect(xs, 0, this.#beatToX(mv.end) - xs, RULER_H - 1);
      ctx.fillStyle = "#ffb454";
      ctx.fillRect(xs, RULER_H - 4, this.#beatToX(mv.end) - xs, 3);
    }
    ctx.font = "11px 'Segoe UI', system-ui, sans-serif";
    ctx.textBaseline = "middle";
    // Harmony band at the bottom of the ruler: bar numbers move up
    const lane = this.#harmonyLane();
    for (let b = first - (first % bpb); b <= last; b += bpb) {
      const bar = b / bpb;
      if (bar % barStep) continue;
      const x = Math.round(this.#beatToX(b)) + 0.5;
      ctx.fillStyle = "#3a4458";
      ctx.fillRect(x, lane ? 0 : RULER_H - 12, 1, lane ? 14 : 12);
      ctx.fillStyle = "#8a93a6";
      ctx.fillText(String(bar + 1), x + 5, lane ? 8 : RULER_H / 2 - 2);
      if (ppb > 24 && !lane) {
        for (let k = 1; k < bpb; k++) {
          ctx.fillStyle = "#2a3242";
          ctx.fillRect(Math.round(this.#beatToX(b + k)) + 0.5, RULER_H - 6, 1, 6);
        }
      }
    }

    if (lane) this.#drawHarmony(ctx, w, hh);

    // keyboard entry position in the ruler (small violet triangle at the bottom)
    if (en && enX >= -6 && enX <= w + 6) {
      ctx.fillStyle = "#c792ff";
      ctx.beginPath();
      ctx.moveTo(enX - 5, RULER_H - 1);
      ctx.lineTo(enX + 5, RULER_H - 1);
      ctx.lineTo(enX, RULER_H - 8);
      ctx.closePath();
      ctx.fill();
    }

    // Checker markers in the ruler (red error, orange warning, blue hint)
    for (const m of marks) {
      const x0 = this.#beatToX(m.start);
      const x1 = Math.max(x0 + 4, this.#beatToX(m.end));
      if (x1 < 0 || x0 > w) continue;
      ctx.fillStyle = markColor(m.severity, 0.95);
      ctx.fillRect(x0, RULER_H - 5, x1 - x0, 4);
    }

    // Playhead
    const st = status.peek();
    const px = this.#beatToX((st.positionSecs * p.tempoBpm) / 60);
    if (px >= -2 && px <= w + 2) {
      const col = st.recording ? "#ff5d6c" : "#ffb454";
      ctx.fillStyle = col;
      ctx.fillRect(Math.round(px), RULER_H, 1.5, hh);
      ctx.beginPath();
      ctx.moveTo(px - 6, 4);
      ctx.lineTo(px + 6, 4);
      ctx.lineTo(px, 14);
      ctx.closePath();
      ctx.fill();
    }
  }

  /** Dragged audio files: where they go (row, start line, label). */
  #drawDrop(ctx: CanvasRenderingContext2D, p: Project, rows: Array<{ track: Track; y: number; h: number }>, w: number, hh: number): void {
    const drag = audioDrag.peek();
    const target = drag ? this.dropTargetAt(drag.x, drag.y) : null;
    if (!drag || !target) {
      if (this.dataset.dropInfo) delete this.dataset.dropInfo;
      return;
    }
    const row = target.trackId != null ? rows.find((r) => r.track.id === target.trackId) : undefined;
    const into = row?.track.kind === "audio";
    const x = Math.round(this.#beatToX(target.startBeats));
    ctx.save();
    ctx.fillStyle = "rgba(110, 231, 216, 0.85)";
    if (row && into) {
      ctx.fillStyle = "rgba(110, 231, 216, 0.09)";
      ctx.fillRect(0, row.y, w, row.h);
      ctx.fillStyle = "rgba(110, 231, 216, 0.85)";
    } else {
      // new track: below the track under the mouse or below the last one
      const last = row ?? rows[rows.length - 1];
      ctx.fillRect(0, Math.max(RULER_H, last ? last.y + last.h - 1 : RULER_H), w, 2);
    }
    ctx.fillRect(x, RULER_H, 2, hh - RULER_H);
    ctx.restore();
    const n = drag.count;
    const files = plural(n, "Audio file", "{n} audio files");
    const where = into
      ? n > 1
        ? plural(n - 1, "into \"{name}\" (+{n} new track)", "into \"{name}\" (+{n} new tracks)", { name: row!.track.name })
        : t("into \"{name}\"", { name: row!.track.name })
      : plural(n, "new track", "new tracks");
    const label = t("{files} → bar {bar} · {where}", { files, bar: formatBarBeat(target.startBeats, p.beatsPerBar), where });
    this.#tip(ctx, x, drag.y - this.#canvas.getBoundingClientRect().top, label, "rgba(110, 231, 216, 0.8)");
    this.dataset.dropInfo = label;
  }

  /** "♪ → Notes" button in the header of an audio clip (omitted if the clip is too narrow). */
  #drawNoteButton(ctx: CanvasRenderingContext2D, track: Track, c: Clip, x0: number, x1: number, top: number, hgt: number, w: number): void {
    const text = `♪ → ${t("Notes")}`;
    ctx.save();
    ctx.font = "600 10.5px 'Segoe UI', system-ui, sans-serif";
    const bw = Math.ceil(ctx.measureText(text).width) + 12;
    const bh = Math.max(12, Math.min(15, hgt - 3));
    const right = Math.min(x1, w) - 5;
    if (right - Math.max(x0, 0) < bw + 40 || this.#drag?.kind === "clip") {
      ctx.restore();
      return;
    }
    const bx = right - bw;
    const by = top + (hgt - bh) / 2;
    ctx.globalAlpha = 1;
    ctx.fillStyle = "rgba(13, 15, 20, 0.78)";
    ctx.strokeStyle = "rgba(110, 231, 216, 0.8)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.roundRect(bx + 0.5, by + 0.5, bw, bh, 4);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = "#6ee7d8";
    ctx.textBaseline = "middle";
    ctx.fillText(text, bx + 6, by + bh / 2 + 0.5);
    ctx.restore();
    this.#noteButtons.push({ x: bx, y: by, w: bw, h: bh, trackId: track.id, clipId: c.id });
  }

  /** Harmony band visible (switched on or working in the Harmony window) and chords present? */
  #harmonyLane(): boolean {
    return (harmonyLaneOn.peek() || harmonyFocus.peek()) && !!harmonyLine.peek()?.chords.length;
  }

  /**
   * Chord progression at the bottom of the ruler: for each chord a box in the color of its
   * mood (as in the circle of fifths), with its name; harmonic breaks as a red
   * edge running through all tracks. The chord under the playhead lights up.
   */
  #drawHarmony(ctx: CanvasRenderingContext2D, w: number, hh: number): void {
    const line = harmonyLine.peek();
    const p = snapshot.peek()?.project;
    if (!line || !p) return;
    const top = 16;
    const bandH = RULER_H - 1 - top;
    const now = chordIndexAt(line, (status.peek().positionSecs * p.tempoBpm) / 60);
    ctx.font = "600 10.5px 'Segoe UI', system-ui, sans-serif";
    ctx.textBaseline = "middle";
    line.chords.forEach((c, i) => {
      const x0 = this.#beatToX(c.startBeats);
      const x1 = this.#beatToX(c.startBeats + c.lengthBeats);
      if (x1 < 0 || x0 > w) return;
      const col = MOOD_COLORS[c.mood];
      ctx.globalAlpha = i === now ? 0.5 : 0.24;
      ctx.fillStyle = col;
      ctx.fillRect(x0 + 0.5, top, Math.max(1, x1 - x0 - 1), bandH);
      ctx.globalAlpha = 1;
      ctx.fillRect(x0 + 0.5, top, Math.max(1, x1 - x0 - 1), 2);
      if (x1 - x0 > 16) {
        ctx.save();
        ctx.beginPath();
        ctx.rect(x0 + 1, top, x1 - x0 - 2, bandH);
        ctx.clip();
        ctx.fillStyle = "#eef1f6";
        ctx.fillText(chordName(c.name), x0 + 4, top + bandH / 2 + 1);
        ctx.restore();
      }
      if (c.bruch) {
        // break: red edge in the ruler and dashed through all tracks
        ctx.fillStyle = "#ff4d5e";
        ctx.fillRect(Math.round(x0) - 1, top - 3, 3, bandH + 3);
        ctx.beginPath();
        ctx.moveTo(x0 - 5, top - 3);
        ctx.lineTo(x0 + 5, top - 3);
        ctx.lineTo(x0, top + 4);
        ctx.closePath();
        ctx.fill();
        ctx.globalAlpha = 0.45;
        ctx.setLineDash([4, 4]);
        ctx.strokeStyle = "#ff4d5e";
        ctx.beginPath();
        ctx.moveTo(Math.round(x0) + 0.5, RULER_H);
        ctx.lineTo(Math.round(x0) + 0.5, hh);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.globalAlpha = 1;
      }
    });
  }

  /** Explanation of the chord under the mouse in the harmony band. */
  #harmonyTip(e: PointerEvent): string {
    if (e.offsetY < 14 || e.offsetY > RULER_H || !this.#harmonyLane()) return "";
    const line = harmonyLine.peek()!;
    const i = chordIndexAt(line, this.#xToBeat(e.offsetX));
    const c = line.chords[i];
    if (!c || this.#xToBeat(e.offsetX) > c.startBeats + c.lengthBeats) return "";
    const prev = line.chords[i - 1];
    return `${chordName(c.name)} · ${c.roman} · ${c.function} – ${c.role}`
      + `\n${prev ? `${chordName(prev.name)} → ${chordName(c.name)}: ` : ""}${MOOD_LABELS[c.mood]}`
      + (c.bruch ? `\n${t("Harmonic break: {reason}", { reason: c.bruch })}` : "")
      + `\n${t("Key {key}", { key: line.key.name })}`;
  }

  #drawLanes(ctx: CanvasRenderingContext2D, t: Track, y: number, w: number): void {
    const g = geoFor(t);
    const LANE_SPAN = g.view.span;
    const LANE_H = g.laneH;
    const LANES_H = g.lanesH;
    const RHYTHM_TOP = g.rhythmTop;
    ctx.fillStyle = "#0f1219";
    ctx.fillRect(0, y, w, NOTES_TOP);
    // Drums: highlight occupied keys (drums)
    const drumKeys = t.instrument?.kind === "drums" ? new Set(specFor(t.instrument)?.keys.map((k) => k.midi)) : null;
    const up = highOnTop.peek();
    for (let k = 0; k < LANE_SPAN; k++) {
      const pitch = pitchOfLane(g.view, k);
      const ly = y + NOTES_TOP + k * LANE_H;
      ctx.fillStyle = drumKeys?.has(pitch) ? "#181d28" : isBlack(pitch) ? "#0b0d12" : "#11141b";
      ctx.fillRect(0, ly, w, LANE_H);
      if (pitch % 12 === 0) {
        // Octave boundary between B and C (B is above or below depending on direction)
        ctx.fillStyle = "#283041";
        ctx.fillRect(0, up ? ly + LANE_H - 1 : ly, w, 1);
      }
    }
    // Rhythm strip below the pitch lanes.
    const lanesEnd = y + NOTES_TOP + LANES_H;
    ctx.fillStyle = "#0d1016";
    ctx.fillRect(0, lanesEnd, w, RHYTHM_TOP - NOTES_TOP - LANES_H);
    ctx.fillStyle = "#0b0d12";
    ctx.fillRect(0, y + RHYTHM_TOP, w, RHYTHM_H);
    ctx.fillStyle = "#1d2330";
    ctx.fillRect(0, y + RHYTHM_TOP, w, 1);
    // Expression track: guide lines at 0/50/100 %, for two-sided values the zero line emphasized.
    if (g.exprH) {
      const def = EXPRS[laneKeyFor(t)];
      ctx.fillStyle = "#0a0c11";
      ctx.fillRect(0, y + g.exprTop, w, g.exprH);
      ctx.fillStyle = "#1d2330";
      ctx.fillRect(0, y + g.exprTop, w, 1);
      for (const f of [0, 0.5, 1]) {
        const v = def.min + f * (def.max - def.min);
        ctx.fillStyle = f === 0.5 && def.bipolar ? "#2a3243" : "#151a24";
        ctx.fillRect(0, Math.round(y + exprY(g, def, v)), w, 1);
      }
    }
  }

  /** During expression gestures: line (Shift) and value at the pointer. */
  #drawExprTip(ctx: CanvasRenderingContext2D, p: Project): void {
    const d = this.#drag;
    if (d?.kind !== "expr" && d?.kind !== "noteExpr") return;
    const def = EXPRS[d.key];
    let value: number;
    if (d.kind === "expr") {
      const t = p.tracks.find((tr) => tr.id === d.trackId);
      const row = t ? this.#rows(p).find((r) => r.track.id === t.id) : undefined;
      if (!t || !row) return;
      value = exprAt(geoFor(t), def, d.curY - row.y);
      if (d.line) {
        ctx.save();
        ctx.strokeStyle = "rgba(255, 180, 84, 0.9)";
        ctx.setLineDash([5, 4]);
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.moveTo(d.startX, d.startY);
        ctx.lineTo(d.curX, d.curY);
        ctx.stroke();
        ctx.restore();
      }
    } else {
      if (!d.moved) return;
      value = exprValue(d.draft[d.grabbed]!, d.key);
    }
    const text = `${def.label}: ${def.format(value)}`;
    ctx.font = "600 11.5px 'Segoe UI', system-ui, sans-serif";
    const tw = ctx.measureText(text).width;
    const x = Math.min(d.curX + 14, this.#width - tw - 22);
    const y = Math.max(RULER_H + 4, d.curY - 30);
    ctx.fillStyle = "rgba(20, 24, 33, 0.94)";
    ctx.strokeStyle = "rgba(255, 180, 84, 0.7)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.roundRect(x, y, tw + 16, 22, 6);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = "#f1e3cc";
    ctx.textBaseline = "middle";
    ctx.fillText(text, x + 8, y + 11.5);
  }

  #clipStart(c: Clip): number {
    const d = this.#drag;
    return d?.kind === "clip" && d.clipId === c.id && d.moved ? d.beats : c.startBeats;
  }

  #drawNotesClip(ctx: CanvasRenderingContext2D, p: Project, track: Track, c: Clip, rowY: number, w: number, selected: boolean): void {
    const start = this.#clipStart(c);
    // x0: clip start (reference of the notes); wx0/wx1: frame of the clip – while
    // a note lies outside, it already grows along (whole bars)
    const x0 = this.#beatToX(start);
    const len = clipLengthBeats(c, p.tempoBpm);
    const dr = this.#drag;
    const span = (dr?.kind === "notes" || dr?.kind === "noteExpr") && dr.clipId === c.id && c.source.type === "notes"
      ? clipSpan(dr.draft, len, p.beatsPerBar, start)
      : { lo: 0, hi: len };
    const growing = span.lo < 0 || span.hi > len + 1e-9;
    const wx0 = x0 + span.lo * this.#ppb();
    const wx1 = x0 + span.hi * this.#ppb();
    if (wx1 < 0 || wx0 > w) return;
    const g = geoFor(track);
    const LANE_H = g.laneH;
    const RHYTHM_TOP = g.rhythmTop;
    const lanesTop = rowY + NOTES_TOP;
    const lanesH = g.lanesH;

    const bottom = g.exprH ? g.exprTop + g.exprH : RHYTHM_TOP + RHYTHM_H;
    ctx.save();
    ctx.globalAlpha = track.mute ? 0.45 : 1;
    ctx.fillStyle = rgba(track.color, 0.06);
    ctx.fillRect(wx0, lanesTop, wx1 - wx0, lanesH);
    ctx.strokeStyle = selected ? "rgba(255,255,255,0.85)" : rgba(track.color, 0.45);
    ctx.lineWidth = selected ? 1.5 : 1;
    if (growing) ctx.setLineDash([5, 3]);
    ctx.strokeRect(wx0 + 0.5, rowY + 2.5, wx1 - wx0 - 1, bottom - 3);
    ctx.setLineDash([]);
    ctx.fillStyle = rgba(track.color, 0.3);
    ctx.fillRect(wx0, rowY + 2, wx1 - wx0, NOTES_TOP - 3);
    ctx.beginPath();
    ctx.rect(wx0, rowY, wx1 - wx0, bottom);
    ctx.clip();
    ctx.fillStyle = "#e8ebf1";
    ctx.font = "600 11px 'Segoe UI', system-ui, sans-serif";
    ctx.textBaseline = "middle";
    ctx.fillText(growing ? t("{name} – extending", { name: c.name }) : c.name, Math.max(wx0, 0) + 7, rowY + NOTES_TOP / 2 + 1);

    if (c.source.type === "notes") {
      const sel = noteSel.peek();
      const selKeys = new Set(sel && sel.clipId === c.id ? sel.keys : []);
      const trackHasSel = !!sel && sel.trackId === track.id && sel.keys.length > 0;
      const d = this.#drag;
      const activeDraft = (d?.kind === "notes" || d?.kind === "noteExpr") && d.clipId === c.id ? new Set(d.active) : null;
      const exprDef = g.exprH ? EXPRS[laneKeyFor(track)] : null;
      this.#clipNotes(c).forEach((n, i) => {
        const nx = x0 + n.startBeats * this.#ppb();
        const nw = Math.max(3, n.lengthBeats * this.#ppb() - 1);
        const isSel = activeDraft ? activeDraft.has(i) : selKeys.has(noteKey(n));
        // Rhythm strip: note starts subtle
        ctx.fillStyle = rgba(track.color, 0.4);
        ctx.fillRect(Math.round(nx), rowY + RHYTHM_TOP + 4, 2, RHYTHM_H - 7);
        // Expression track: lollipop per note (stem from the baseline or zero line to the value)
        if (exprDef) {
          const v = exprValue(n, exprDef.key);
          const base = rowY + exprY(g, exprDef, exprDef.bipolar ? 0 : exprDef.min);
          const vy = rowY + exprY(g, exprDef, v);
          const dim = trackHasSel && !isSel;
          ctx.fillStyle = rgba(track.color, dim ? 0.25 : 0.75);
          ctx.fillRect(Math.round(nx), Math.min(base, vy), 1.5, Math.max(1, Math.abs(vy - base)));
          ctx.fillStyle = isSel ? "#ffffff" : rgba(track.color, dim ? 0.4 : 1);
          ctx.beginPath();
          ctx.arc(Math.round(nx) + 0.75, vy, isSel ? 3.2 : 2.6, 0, Math.PI * 2);
          ctx.fill();
        }
        if (!inView(g.view, n.pitch)) {
          // outside the visible range: marker at the edge
          ctx.fillStyle = rgba(track.color, 0.7);
          ctx.fillRect(nx, offViewAtTop(g.view, n.pitch) ? lanesTop : lanesTop + lanesH - 2, nw, 2);
          return;
        }
        const ny = rowY + laneY(g, n.pitch);
        const nh = Math.max(2, LANE_H - 1);
        // Slide: arc from the offset note to the note start
        if (n.bend) {
          const from = Math.max(g.view.low - 1, Math.min(g.view.low + g.view.span, n.pitch + n.bend));
          const fy = rowY + laneY(g, from) + LANE_H / 2;
          const reach = Math.min(nw * 0.45, 10);
          ctx.strokeStyle = rgba(track.color, 0.95);
          ctx.lineWidth = 1.6;
          ctx.beginPath();
          ctx.moveTo(nx - 7, fy);
          ctx.quadraticCurveTo(nx + reach * 0.2, ny + LANE_H / 2, nx + reach, ny + LANE_H / 2);
          ctx.stroke();
        }
        // velocity = opacity
        ctx.fillStyle = rgba(track.color, 0.28 + 0.72 * (n.velocity / 127));
        ctx.beginPath();
        ctx.roundRect(nx + 0.5, ny + 0.5, nw, nh, 2);
        ctx.fill();
        // Brightness: light edge on top or darkening
        const br = n.brightness ?? 0;
        if (br > 0.01) {
          ctx.fillStyle = `rgba(255,255,255,${(0.25 + 0.6 * br).toFixed(2)})`;
          ctx.fillRect(nx + 1.5, ny + 1, Math.max(1, nw - 2), Math.min(2, nh / 3));
        } else if (br < -0.01) {
          ctx.fillStyle = `rgba(5,7,10,${(0.5 * -br).toFixed(2)})`;
          ctx.fillRect(nx + 0.5, ny + 0.5, nw, nh);
        }
        // Vibrato: wavy line in the rear part of the note (sets in with a delay)
        const vib = n.vibrato ?? 0;
        if (vib > 0.005 && nw > 10 && LANE_H >= 4) {
          const amp = Math.min(nh / 2 - 0.5, 0.8 + vib * Math.max(3, LANE_H));
          const cy = ny + 0.5 + nh / 2;
          const xs = nx + Math.min(nw * 0.25, 10);
          ctx.strokeStyle = "rgba(255,255,255,0.85)";
          ctx.lineWidth = 1.1;
          ctx.beginPath();
          for (let x = xs; x <= nx + nw - 2; x += 1) {
            const ramp = Math.min(1, (x - xs) / 8);
            const yy = cy - Math.sin((x - xs) * 0.9) * amp * ramp;
            if (x === xs) ctx.moveTo(x, yy);
            else ctx.lineTo(x, yy);
          }
          ctx.stroke();
        }
        if (isSel) {
          ctx.strokeStyle = "#ffffff";
          ctx.lineWidth = 1.5;
          ctx.beginPath();
          ctx.roundRect(nx + 0.5, ny + 0.5, nw, nh, 2);
          ctx.stroke();
        }
        if (nw > 14) {
          ctx.fillStyle = "rgba(13,15,20,0.45)";
          ctx.fillRect(nx + nw - 3, ny + 2, 1.5, LANE_H - 4);
        }
      });
    } else {
      this.#drawAudioPitch(ctx, p, track, c, x0, rowY, g, w);
      this.#drawNoteButton(ctx, track, c, wx0, wx1, rowY + 2, NOTES_TOP - 3, w);
    }
    ctx.restore();
  }

  /** Hint box within a clip. */
  #clipNote(ctx: CanvasRenderingContext2D, x: number, y: number, title: string, sub: string): void {
    const titleFont = "600 12px 'Segoe UI', system-ui, sans-serif";
    const subFont = "11px 'Segoe UI', system-ui, sans-serif";
    ctx.font = titleFont;
    const titleW = ctx.measureText(title).width;
    ctx.font = subFont;
    const tw = Math.max(titleW, ctx.measureText(sub).width);
    ctx.fillStyle = "rgba(13, 15, 20, 0.82)";
    ctx.strokeStyle = "rgba(138, 147, 166, 0.35)";
    ctx.beginPath();
    ctx.roundRect(x, y, tw + 20, 42, 6);
    ctx.fill();
    ctx.stroke();
    ctx.textBaseline = "middle";
    ctx.fillStyle = "#e8ebf1";
    ctx.font = titleFont;
    ctx.fillText(title, x + 10, y + 14);
    ctx.fillStyle = "#8a93a6";
    ctx.font = subFont;
    ctx.fillText(sub, x + 10, y + 30);
  }

  /**
   * Audio recording in the note view (display only): detected melody notes with
   * pitch curve, hits with a fundamental (bass drum, snare, bass …) and unpitched
   * beats (hi-hat …) in the rhythm strip.
   */
  #drawAudioPitch(ctx: CanvasRenderingContext2D, p: Project, track: Track, c: Clip, x0: number, rowY: number, g: NoteGeo, w: number): void {
    if (c.source.type !== "audio") return;
    const view = g.view;
    const low = view.low;
    const LANE_H = g.laneH;
    const RHYTHM_TOP = g.rhythmTop;
    const lanesTop = rowY + NOTES_TOP;
    const lanesH = g.lanesH;
    const a = analysisFor(c.source.assetId, () => this.#invalidate());
    const noteX = Math.max(x0, 0) + 10;
    if (!a) {
      const none = analysisState(c.source.assetId) === "none";
      this.#clipNote(ctx, noteX, lanesTop + 10, none ? t("No analysis available") : t("Analyzing…"), none ? t("Desktop app only") : t("Detecting pitches and onsets"));
      return;
    }
    const beatsPerSec = (c.source.warp ? c.source.sourceBpm : p.tempoBpm) / 60;
    const ppb = this.#ppb();
    const xAt = (secs: number) => x0 + secs * beatsPerSec * ppb;

    const shift = track.pitchSemitones;
    const yAt = (midi: number) => rowY + laneY(g, midi) + LANE_H / 2;
    const diamond = Math.max(3, Math.min(5, LANE_H * 0.6));

    // Rhythm strip: unpitched beats (hi-hat, shaker …) – only time and strength.
    const stripTop = rowY + RHYTHM_TOP;
    const unpitched = a.hits.filter((hp) => hp.midi == null);
    for (const hp of unpitched) {
      const x = Math.round(xAt(hp.time));
      if (x < -3 || x > w + 3) continue;
      const bar = Math.max(4, (RHYTHM_H - 4) * Math.sqrt(hp.strength));
      ctx.fillStyle = rgba(track.color, 0.95);
      ctx.fillRect(x - 1, stripTop + RHYTHM_H - 2 - bar, 3, bar);
    }

    // Merge melody notes and hits: A short note that begins with a
    // hit of the same pitch (e.g. a decaying bass drum) shows only
    // the diamond; for longer notes the note box shows the hit.
    const hideHit = new Set<number>();
    const hideNote = new Set<number>();
    a.notes.forEach((n, ni) => {
      // The note may begin a little later (a falling kick pitch only settles down later).
      const hi = a.hits.findIndex(
        (h) => h.midi != null && n.startSecs >= h.time - 0.08 && n.startSecs <= h.end && Math.abs(Math.round(h.midi) - n.midi) <= 2,
      );
      if (hi < 0) return;
      if (n.endSecs - n.startSecs < 0.3) hideNote.add(ni);
      else hideHit.add(hi);
    });

    // Hits with a fundamental (bass drum, snare, toms, bass): diamond + short bar.
    for (const [hi, hp] of a.hits.entries()) {
      if (hp.midi == null || hideHit.has(hi)) continue;
      const m = Math.round(hp.midi + shift);
      const x = xAt(hp.time);
      if (x < -10 || x > w + 10) continue;
      const alpha = 0.45 + 0.55 * hp.strength;
      if (!inView(view, m)) {
        ctx.fillStyle = rgba(track.color, alpha);
        ctx.fillRect(x - 3, offViewAtTop(view, m) ? lanesTop : lanesTop + lanesH - 2, 7, 2);
        continue;
      }
      const y = yAt(m);
      const len = Math.max(6, xAt(Math.min(hp.end, hp.time + 0.25)) - x);
      ctx.fillStyle = rgba(track.color, alpha * 0.45);
      ctx.fillRect(x, y - Math.min(2, LANE_H / 3), len, Math.min(4, LANE_H * 0.66));
      ctx.fillStyle = rgba(track.color, alpha);
      ctx.beginPath();
      ctx.moveTo(x, y - diamond);
      ctx.lineTo(x + diamond, y);
      ctx.lineTo(x, y + diamond);
      ctx.lineTo(x - diamond, y);
      ctx.closePath();
      ctx.fill();
    }

    const pitchedHits = a.hits.length - unpitched.length;
    if (!a.notes.length && !pitchedHits) {
      const tempo = a.tempoEstimate ? ` · ${t("Tempo ≈ {bpm} BPM", { bpm: a.tempoEstimate.toFixed(1) })}` : "";
      this.#clipNote(ctx, noteX, lanesTop + 14, t("No pitch detected"), `${t("{n} hits in the rhythm lane", { n: unpitched.length })}${tempo}`);
      return;
    }
    for (const [ni, n] of a.notes.entries()) {
      const m = n.midi + Math.round(shift);
      if (hideNote.has(ni) || !inView(view, m)) continue;
      ctx.fillStyle = rgba(track.color, 0.35);
      ctx.strokeStyle = rgba(track.color, 0.9);
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.roundRect(xAt(n.startSecs), rowY + laneY(g, m) + 0.5, Math.max(2, xAt(n.endSecs) - xAt(n.startSecs)), Math.max(2, LANE_H - 1), 2);
      ctx.fill();
      ctx.stroke();
    }
    // Pitch curve only in held melody notes (≥ 0.3 s): there it shows intonation
    // and vibrato; for beats (falling kick pitch) it would be just noise.
    const held = a.notes.filter((n, ni) => !hideNote.has(ni) && n.endSecs - n.startSecs >= 0.3);
    const inNote = (secs: number) => held.some((n) => secs >= n.startSecs - 0.04 && secs <= n.endSecs + 0.04);
    ctx.strokeStyle = "#6ee7d8";
    ctx.lineWidth = 1.4;
    ctx.beginPath();
    let prev: number | null = null;
    for (const [secs, midi] of a.pitch) {
      const m = midi == null ? null : midi + shift;
      if (m == null || m < low - 1 || m > low + view.span || !inNote(secs)) {
        prev = null;
        continue;
      }
      const x = xAt(secs);
      const y = yAt(m);
      // Do not connect jumps (pitch changes, octave errors) as a vertical line.
      if (prev != null && Math.abs(m - prev) <= 1.5) ctx.lineTo(x, y);
      else ctx.moveTo(x, y);
      prev = m;
    }
    ctx.stroke();
  }

  #drawClip(ctx: CanvasRenderingContext2D, p: Project, track: Track, c: Clip, rowY: number, w: number, selected: boolean): void {
    const start = this.#clipStart(c);
    const len = clipLengthBeats(c, p.tempoBpm);
    const x0 = this.#beatToX(start);
    const x1 = x0 + len * this.#ppb();
    if (x1 < 0 || x0 > w) return;
    const rh = rowHeight(track);
    const y0 = rowY + 5;
    const ch = rh - 10;
    const header = 17;

    ctx.save();
    ctx.globalAlpha = track.mute ? 0.45 : 1;
    ctx.beginPath();
    ctx.roundRect(x0 + 0.5, y0 + 0.5, Math.max(4, x1 - x0 - 1), ch, 6);
    ctx.fillStyle = rgba(track.color, 0.13);
    ctx.fill();
    ctx.lineWidth = selected ? 2 : 1;
    ctx.strokeStyle = selected ? "#ffffff" : rgba(track.color, 0.55);
    ctx.stroke();
    ctx.clip();

    ctx.fillStyle = rgba(track.color, 0.3);
    ctx.fillRect(x0, y0, x1 - x0, header);
    ctx.fillStyle = "#e8ebf1";
    ctx.font = "600 11px 'Segoe UI', system-ui, sans-serif";
    ctx.textBaseline = "middle";
    let label = c.name;
    if (c.source.type === "audio" && c.source.warp && Math.abs(c.source.sourceBpm - p.tempoBpm) > 0.05) {
      label += `   ⇄ ${+c.source.sourceBpm.toFixed(1)} → ${+p.tempoBpm.toFixed(1)} BPM`;
    }
    if (track.pitchSemitones) label += `   ${t("{delta} st", { delta: `${track.pitchSemitones > 0 ? "+" : ""}${track.pitchSemitones}` })}`;
    ctx.fillText(label, Math.max(x0, 0) + 7, y0 + header / 2 + 0.5);

    const top = y0 + header + 3;
    const bodyH = ch - header - 6;
    if (c.source.type === "audio") {
      this.#drawNoteButton(ctx, track, c, x0, x1, y0, header, w);
      const wave = this.#wave(c.source.assetId);
      const mid = top + bodyH / 2;
      ctx.fillStyle = rgba(track.color, 0.85);
      if (wave) {
        const buckets = wave.length / 2;
        const from = Math.max(0, Math.floor(x0));
        const to = Math.min(w, Math.ceil(x1));
        for (let px = from; px < to; px++) {
          const a = Math.floor(((px - x0) / (x1 - x0)) * buckets);
          const b = Math.max(a + 1, Math.floor(((px + 1 - x0) / (x1 - x0)) * buckets));
          let lo = 0;
          let hi = 0;
          for (let k = a; k < b && k < buckets; k++) {
            lo = Math.min(lo, wave[2 * k]!);
            hi = Math.max(hi, wave[2 * k + 1]!);
          }
          const y1 = mid - hi * (bodyH / 2);
          const y2 = mid - lo * (bodyH / 2);
          ctx.fillRect(px, y1, 1, Math.max(1, y2 - y1));
        }
      } else {
        ctx.fillRect(x0, mid, x1 - x0, 1);
      }
    } else {
      const notes = c.source.notes;
      if (notes.length) {
        let lo = 127;
        let hi = 0;
        for (const n of notes) {
          lo = Math.min(lo, n.pitch);
          hi = Math.max(hi, n.pitch);
        }
        lo -= 2;
        hi += 2;
        const nh = Math.max(2, Math.min(7, bodyH / (hi - lo + 1)));
        ctx.fillStyle = rgba(track.color, 0.95);
        for (const n of notes) {
          const nx = x0 + n.startBeats * this.#ppb();
          const nw = Math.max(2, n.lengthBeats * this.#ppb() - 1.5);
          const ny = top + ((hi - n.pitch) / (hi - lo)) * (bodyH - nh);
          ctx.beginPath();
          ctx.roundRect(nx, ny, nw, nh, 2);
          ctx.fill();
        }
      }
    }
    ctx.restore();
  }
}

customElements.define("kw-timeline", KwTimeline);
