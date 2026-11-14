// Note entry without dragging: adding a single note at a spot (double-click in the
// piano roll) and the keyboard entry mode.
//
// Keyboard entry mode (small switch next to the piano of an instrument track): the
// letter keys play the visible octave like a piano – middle letter row = white keys,
// the row above = black keys – and add the note at the entry position; a click on a
// key of the piano does the same. After each note the entry position moves on by one
// grid step, unless Shift is held: then it stays, for chords. Every note is
// its own undo step. The keys are taken by their position on the keyboard (KeyboardEvent.code),
// so the piano shape is the same on every layout; the piano shows each key's letter.

import { backend } from "./bridge.js";
import { noteName } from "./dom.js";
import { clipSpan, snapTo } from "./editmath.js";
import { t } from "./i18n.js";
import { type PitchView, noteGrid, noteKey, noteSel, scrollPitch, viewFor } from "./pianoroll.js";
import { effect, signal } from "./signals.js";
import { clipLengthBeats, run, secondsToBeats, selection, snapshot, status, viewport } from "./store.js";
import type { Clip, Command, Note, Project, Track } from "./types.js";

// ---------- adding a note ----------

/** Note clip of a track at a project position (start ≤ beat < end). */
function clipAt(t: Track, beat: number, bpm: number): Clip | null {
  for (let k = t.clips.length - 1; k >= 0; k--) {
    const c = t.clips[k]!;
    if (c.source.type === "notes" && beat >= c.startBeats - 1e-9 && beat < c.startBeats + clipLengthBeats(c, bpm) - 1e-9) return c;
  }
  return null;
}

/** Nearest note clip of a track (distance to the clip's edges; on a tie the left one). */
export function nearestNotesClip(t: Track, beat: number, bpm: number): Clip | null {
  let best: { c: Clip; d: number } | null = null;
  for (const c of t.clips) {
    if (c.source.type !== "notes") continue;
    const end = c.startBeats + clipLengthBeats(c, bpm);
    const d = beat < c.startBeats ? c.startBeats - beat : beat > end ? beat - end : 0;
    if (!best || d < best.d - 1e-9) best = { c, d };
  }
  return best?.c ?? null;
}

/** Selected notes (keys) of a clip after its notes moved by `shift` beats. */
function shiftedKeys(clip: Clip, keys: string[], shift: number): string[] {
  if (!shift || clip.source.type !== "notes") return keys;
  const set = new Set(keys);
  return clip.source.notes.filter((n) => set.has(noteKey(n))).map((n) => noteKey({ ...n, startBeats: n.startBeats + shift }));
}

/**
 * Adds one note to an instrument track at a project position (beats): into the note
 * clip there, else into the nearest note clip of the track (it grows by whole bars,
 * also to the left), else into a new clip starting at the bar – always one undo step.
 * The new note gets selected; with `addToSelection` it joins the selection (chords).
 */
export async function addNoteAt(trackId: number, beat: number, pitch: number, lengthBeats: number, addToSelection = false): Promise<boolean> {
  const p = snapshot.peek()?.project;
  const track = p?.tracks.find((tr) => tr.id === trackId);
  if (!p || !track || track.kind !== "instrument" || pitch < 0 || pitch > 127) return false;
  const at = Math.max(0, beat);
  const label = t("Add note {note}", { note: noteName(pitch) });
  const clip = clipAt(track, at, p.tempoBpm) ?? nearestNotesClip(track, at, p.tempoBpm);
  if (!clip || clip.source.type !== "notes") return addClipWithNote(p, track, at, pitch, lengthBeats, label);

  const note: Note = { pitch, startBeats: at - clip.startBeats, lengthBeats, velocity: 100 };
  const sel = noteSel.peek();
  const keep = addToSelection && sel?.clipId === clip.id ? sel.keys : [];
  if (clip.source.notes.some((n) => noteKey(n) === noteKey(note))) {
    // the same note is already there: just select it (no second copy, no undo step)
    noteSel.value = { trackId, clipId: clip.id, keys: [...new Set([...keep, noteKey(note)])] };
    selection.value = { trackId, clipId: clip.id };
    return true;
  }
  const notes = [...clip.source.notes, note];
  const span = clipSpan(notes, clip.source.lengthBeats, p.beatsPerBar, clip.startBeats);
  const shift = -span.lo;
  const moved = shift ? notes.map((n) => ({ ...n, startBeats: n.startBeats + shift })) : notes;
  const grown = span.hi - span.lo > clip.source.lengthBeats + 1e-9 ? span.hi - span.lo : undefined;
  const commands: Command[] = [];
  if (shift) commands.push({ type: "setClip", trackId, clipId: clip.id, patch: { startBeats: clip.startBeats - shift } });
  commands.push({ type: "setClipNotes", trackId, clipId: clip.id, notes: moved, ...(grown ? { lengthBeats: grown } : {}) });
  const keys = [...shiftedKeys(clip, keep, shift), noteKey({ ...note, startBeats: note.startBeats + shift })];
  const ok = await run({ type: "batch", label, commands });
  if (ok) {
    noteSel.value = { trackId, clipId: clip.id, keys: [...new Set(keys)] };
    selection.value = { trackId, clipId: clip.id };
  }
  return ok;
}

/** Track without a note clip: a new clip from the bar's start, in whole bars, with the note. */
async function addClipWithNote(p: Project, track: Track, at: number, pitch: number, lengthBeats: number, label: string): Promise<boolean> {
  const bpb = Math.max(1, p.beatsPerBar);
  const bar = Math.floor(at / bpb + 1e-9) * bpb;
  const note: Note = { pitch, startBeats: at - bar, lengthBeats, velocity: 100 };
  const before = new Set(track.clips.map((c) => c.id));
  const length = Math.max(1, Math.ceil((note.startBeats + lengthBeats) / bpb - 1e-9)) * bpb;
  const ok = await run({
    type: "batch",
    label,
    commands: [{ type: "addClip", trackId: track.id, clip: { id: 0, name: t("Notes"), startBeats: bar, source: { type: "notes", lengthBeats: length, notes: [note] } } }],
  });
  const created = snapshot.peek()?.project.tracks.find((tr) => tr.id === track.id)?.clips.find((c) => !before.has(c.id));
  if (ok && created) {
    noteSel.value = { trackId: track.id, clipId: created.id, keys: [noteKey(note)] };
    selection.value = { trackId: track.id, clipId: created.id };
  }
  return ok;
}

// ---------- keyboard entry mode ----------

/** Where typed notes go: track and position (beats); null = mode off. */
export const entry = signal<{ trackId: number; beat: number } | null>(null);
/** Track that typing goes to (changes less often than `entry`: the piano follows it). */
export const entryTrack = signal<number | null>(null);
effect(() => {
  const id = entry.value?.trackId ?? null;
  if (entryTrack.peek() !== id) entryTrack.value = id;
});
/** Last typed note's place – the next note at the same spot joins the selection (chord). */
let lastEntered: { trackId: number; beat: number } | null = null;
/** Typed notes are added one after another, each on the snapshot of the one before. */
let queue: Promise<unknown> = Promise.resolve();

/** Semitones above the visible octave's C, by key position (US names). */
const NOTE_KEYS: Record<string, number> = {
  KeyA: 0, KeyW: 1, KeyS: 2, KeyE: 3, KeyD: 4, KeyF: 5, KeyT: 6, KeyG: 7, KeyY: 8, KeyH: 9, KeyU: 10, KeyJ: 11,
  KeyK: 12, KeyO: 13, KeyL: 14, KeyP: 15, Semicolon: 16, Quote: 17,
};
/** Octave down / up (shifts the visible range of the piano roll). */
const OCTAVE_KEYS: Record<string, number> = { KeyZ: -12, KeyX: 12 };
const US_LABELS: Record<string, string> = { Semicolon: ";", Quote: "'" };

/** Letters of the keys on this keyboard layout (where the browser tells; else US). */
const layoutLabels = new Map<string, string>();
/** Bumped when the layout's letters arrived (the piano redraws its labels). */
export const keyLabelsVersion = signal(0);
try {
  const kb = (navigator as unknown as { keyboard?: { getLayoutMap?: () => Promise<Map<string, string>> } }).keyboard;
  void kb?.getLayoutMap?.().then((m) => {
    for (const code of [...Object.keys(NOTE_KEYS), ...Object.keys(OCTAVE_KEYS)]) {
      const k = m.get(code);
      if (k) layoutLabels.set(code, k.toUpperCase());
    }
    keyLabelsVersion.value++;
  }).catch(() => {});
} catch {
  /* no layout map: US letters */
}

export function keyLabel(code: string): string {
  return layoutLabels.get(code) ?? US_LABELS[code] ?? code.replace(/^Key/, "");
}

/** C of the visible octave: the octave C…B that fits best into the visible range. */
export function entryBase(v: PitchView): number {
  const lo = Math.floor(v.low / 12) * 12;
  const visible = (c: number) => Math.max(0, Math.min(c + 11, v.low + v.span - 1) - Math.max(c, v.low) + 1);
  const center = v.low + v.span / 2;
  let best = lo;
  for (const c of [lo + 12, lo + 24]) {
    const better = visible(c) > visible(best) || (visible(c) === visible(best) && Math.abs(c + 6 - center) < Math.abs(best + 6 - center));
    if (better) best = c;
  }
  return Math.min(best, 120);
}

/** Pitch → letter for the keys of the piano while typing goes to this track. */
export function entryLetters(track: Track): Map<number, string> | null {
  const e = entry.peek();
  if (!e || e.trackId !== track.id) return null;
  const base = entryBase(viewFor(track));
  const out = new Map<number, string>();
  for (const [code, off] of Object.entries(NOTE_KEYS)) if (base + off <= 127) out.set(base + off, keyLabel(code));
  return out;
}

/** Entry step and note length: the grid of the current zoom (like drawing notes). */
export function entryStep(): number {
  return noteGrid(viewport.peek().pxPerBeat, false) || 0.25;
}

/** Start of a new entry: the start of the selected time range, else the playhead – on the grid. */
function startBeat(): number {
  const snap = snapshot.peek();
  const range = snap?.engine.loopRange;
  const at = range ? range[0] : secondsToBeats(status.peek().positionSecs, snap?.project.tempoBpm ?? 120);
  return Math.max(0, snapTo(at, entryStep()));
}

/** Switch next to the piano: typing goes to this track (again: off). */
export function toggleEntry(trackId: number): void {
  const e = entry.peek();
  if (e?.trackId === trackId) entry.value = null;
  else entry.value = { trackId, beat: e ? e.beat : startBeat() };
  lastEntered = null;
}

/** Puts the entry position somewhere (click into the piano roll, playhead moved). */
export function setEntryPosition(beat: number, trackId?: number): void {
  const e = entry.peek();
  if (!e) return;
  entry.value = { trackId: trackId ?? e.trackId, beat: Math.max(0, beat) };
  lastEntered = null;
}

/** The playhead was moved on purpose (ruler click, stop): the entry position follows. */
export function playheadMoved(beat: number): void {
  if (entry.peek()) setEntryPosition(snapTo(beat, entryStep()));
}

/** Plays and adds a note at the entry position; moves on by one step unless `stay`. */
export function enterNote(pitch: number, stay: boolean): void {
  const e = entry.peek();
  if (!e || pitch < 0 || pitch > 127) return;
  const step = entryStep();
  const chord = !!lastEntered && lastEntered.trackId === e.trackId && Math.abs(lastEntered.beat - e.beat) < 1e-9;
  lastEntered = { trackId: e.trackId, beat: e.beat };
  void backend.previewNote(e.trackId, pitch).catch(() => {});
  if (!stay) {
    entry.value = { ...e, beat: e.beat + step };
    lastEntered = null;
  }
  queue = queue.then(() => addNoteAt(e.trackId, e.beat, pitch, step, chord)).catch(() => {});
}

/** Backspace: one step back, the notes starting there are deleted (one undo step). */
function stepBack(trackId: number, beat: number): void {
  const from = Math.max(0, beat - entryStep());
  setEntryPosition(from);
  queue = queue.then(async () => {
    const p = snapshot.peek()?.project;
    const track = p?.tracks.find((tr) => tr.id === trackId);
    if (!track) return;
    const commands: Command[] = [];
    for (const c of track.clips) {
      if (c.source.type !== "notes") continue;
      const hit = (n: Note) => c.startBeats + n.startBeats >= from - 1e-9 && c.startBeats + n.startBeats < beat - 1e-9;
      if (c.source.notes.some(hit)) commands.push({ type: "setClipNotes", trackId, clipId: c.id, notes: c.source.notes.filter((n) => !hit(n)) });
    }
    if (!commands.length) return;
    noteSel.value = null;
    await run({ type: "batch", label: t("Delete notes"), commands });
  }).catch(() => {});
}

/** Waits until all typed notes are in the project (for tests). */
export function entryIdle(): Promise<unknown> {
  return queue;
}

/**
 * Keys while the mode is on (called before the other shortcuts). Letters enter notes –
 * with Shift the position stays (chords); Ctrl shortcuts (undo, save …) work as usual.
 * Z / X shift the octave, ← / → move the entry position (Shift: a bar), Backspace steps
 * back and deletes the notes there, Esc ends the mode; other letters do nothing.
 */
export function handleEntryKey(e: KeyboardEvent): boolean {
  const en = entry.peek();
  if (!en || e.altKey || e.metaKey || e.ctrlKey) return false;
  const track = snapshot.peek()?.project.tracks.find((tr) => tr.id === en.trackId);
  if (!track) return false;
  const off = NOTE_KEYS[e.code];
  if (off !== undefined) {
    e.preventDefault();
    if (!e.repeat) enterNote(entryBase(viewFor(track)) + off, e.shiftKey);
    return true;
  }
  const octave = OCTAVE_KEYS[e.code];
  if (octave !== undefined && !e.shiftKey) {
    e.preventDefault();
    scrollPitch(track.id, octave, viewFor(track));
    return true;
  }
  if (e.key === "Backspace") {
    e.preventDefault();
    stepBack(track.id, en.beat);
    return true;
  }
  if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
    e.preventDefault();
    const bpb = snapshot.peek()?.project.beatsPerBar ?? 4;
    const step = e.shiftKey ? bpb : entryStep();
    setEntryPosition(en.beat + (e.key === "ArrowRight" ? step : -step));
    return true;
  }
  if (e.key === "Escape") {
    entry.value = null;
    return false; // Esc also clears the note selection as usual
  }
  // the other letters (R between E and T, I between U and O …) do nothing – no recording by accident
  if (/^Key[A-Z]$/.test(e.code)) {
    e.preventDefault();
    return true;
  }
  return false;
}

// Typing follows the selected piano roll; the mode ends when its track is gone or folded up.
const pianoRoll = (tr: Track | undefined) => tr?.kind === "instrument" && tr.view === "notes";
effect(() => {
  const sel = selection.value;
  const tracks = snapshot.value?.project.tracks ?? [];
  const e = entry.peek();
  if (!e) return;
  if (!pianoRoll(tracks.find((tr) => tr.id === e.trackId))) {
    entry.value = null;
    return;
  }
  const chosen = sel ? tracks.find((tr) => tr.id === sel.trackId) : undefined;
  if (chosen && pianoRoll(chosen) && chosen.id !== e.trackId) entry.value = { ...e, trackId: chosen.id };
});

// A newly selected time range: the entry position jumps to its start.
let lastRange: string | null = null;
effect(() => {
  const range = snapshot.value?.engine.loopRange ?? null;
  const key = range ? `${range[0]}` : null;
  if (key !== lastRange && range) playheadMoved(range[0]);
  lastRange = key;
});
