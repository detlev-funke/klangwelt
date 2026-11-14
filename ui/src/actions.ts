// Benutzeraktionen, die von mehreren Stellen (Knöpfe, Tastatur) ausgelöst werden.

import { backend } from "./bridge.js";
import {
  act, analysis, errorText, findClip, notify, project, rangeFocus, run, selection, showPanel, snapshot, status, viewport,
} from "./store.js";
import { heightDraft, heightLimits, noteKey, noteSel, rowHeight } from "./pianoroll.js";
import { gridLabel, rangeLabel } from "./editmath.js";
import { plural, t } from "./i18n.js";
import type { ConvertTarget, Note, Track, TrackKind } from "./types.js";

// ---------- Spurhöhe (Splitter) ----------

function heightPatch(t: Track, height: number) {
  return t.view === "notes" ? { notesHeight: height } : { compactHeight: height };
}

/**
 * Beginnt das Ziehen am Splitter unter einer Spur. Während des Ziehens wird nur
 * die Vorschau-Höhe gesetzt; beim Loslassen geht ein Befehl an den Kern
 * (reine Ansichtsänderung, kein Undo-Schritt).
 */
export function startRowResize(t: Track, e: PointerEvent, el: Element): void {
  e.preventDefault();
  const startY = e.clientY;
  const startH = rowHeight(t);
  const [min, max] = heightLimits(t);
  try {
    el.setPointerCapture(e.pointerId);
  } catch {
    /* synthetische Ereignisse */
  }
  const prevCursor = document.body.style.cursor;
  document.body.style.cursor = "row-resize";
  const move = (ev: PointerEvent) => {
    const height = Math.round(Math.max(min, Math.min(max, startH + ev.clientY - startY)));
    heightDraft.value = { trackId: t.id, height };
  };
  const up = async () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", up);
    document.body.style.cursor = prevCursor;
    const d = heightDraft.peek();
    if (d && d.trackId === t.id && Math.abs(d.height - startH) >= 1) {
      await run({ type: "setTrack", trackId: t.id, patch: heightPatch(t, d.height) });
    }
    heightDraft.value = null;
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", up);
}

/** Standardhöhe der aktuellen Ansicht wiederherstellen. */
export async function resetRowHeight(t: Track): Promise<void> {
  heightDraft.value = null;
  await run({ type: "setTrack", trackId: t.id, patch: heightPatch(t, 0) });
}

/** Noten des Clips mit der aktuellen Notenauswahl. */
export function selectedNotesClip(): { trackId: number; clipId: number; notes: Note[]; keys: Set<string> } | null {
  const sel = noteSel.peek();
  if (!sel || !sel.keys.length) return null;
  const clip = project()?.tracks.find((t) => t.id === sel.trackId)?.clips.find((c) => c.id === sel.clipId);
  if (!clip || clip.source.type !== "notes") return null;
  return { trackId: sel.trackId, clipId: sel.clipId, notes: clip.source.notes, keys: new Set(sel.keys) };
}

export function deleteSelectedNotes(): boolean {
  const s = selectedNotesClip();
  if (!s) return false;
  noteSel.value = null;
  void run({ type: "setClipNotes", trackId: s.trackId, clipId: s.clipId, notes: s.notes.filter((n) => !s.keys.has(noteKey(n))) });
  return true;
}

/** Markierte Noten um `delta` Halbtöne verschieben (↑/↓, mit Umschalt eine Oktave) – ein Rückgängig-Schritt je Druck. */
export function transposeSelectedNotes(delta: number): boolean {
  const s = selectedNotesClip();
  if (!s) return false;
  const moved: Note[] = [];
  const notes = s.notes.map((n) => {
    if (!s.keys.has(noteKey(n))) return n;
    const m = { ...n, pitch: Math.max(0, Math.min(127, n.pitch + delta)) };
    moved.push(m);
    return m;
  });
  noteSel.value = { trackId: s.trackId, clipId: s.clipId, keys: moved.map(noteKey) };
  if (moved[0]) void backend.previewNote(s.trackId, moved[0].pitch).catch(() => {});
  const label = Math.abs(delta) === 12
    ? delta > 0 ? t("Notes up one octave") : t("Notes down one octave")
    : t("Transpose notes ({delta} semitones)", { delta: `${delta > 0 ? "+" : "−"}${Math.abs(delta)}` });
  void run({ type: "batch", label, commands: [{ type: "setClipNotes", trackId: s.trackId, clipId: s.clipId, notes }] });
  return true;
}

/** Markierte Noten am Raster ausrichten (`grid` in Schlägen); mit `lengths` auch die Enden. */
export async function quantizeSelectedNotes(grid: number, lengths: boolean): Promise<boolean> {
  const s = selectedNotesClip();
  if (!s) return false;
  const indices = s.notes.map((n, i) => (s.keys.has(noteKey(n)) ? i : -1)).filter((i) => i >= 0);
  try {
    const r = await backend.quantizeNotes(s.trackId, s.clipId, indices, grid, lengths);
    snapshot.value = r.snapshot;
    const clip = r.snapshot.project.tracks.find((t) => t.id === s.trackId)?.clips.find((c) => c.id === s.clipId);
    const notes = clip?.source.type === "notes" ? clip.source.notes : [];
    noteSel.value = { trackId: s.trackId, clipId: s.clipId, keys: r.selected.flatMap((i) => (notes[i] ? [noteKey(notes[i])] : [])) };
    const unit = project()?.beatUnit ?? 4;
    const vars = { grid: gridLabel(grid, unit) };
    notify(lengths
      ? plural(indices.length, "Snapped {n} note to the {grid} grid (start and length)", "Snapped {n} notes to the {grid} grid (start and length)", vars)
      : plural(indices.length, "Snapped {n} note to the {grid} grid", "Snapped {n} notes to the {grid} grid", vars));
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

/** Markierten Zeitbereich aus allen Spuren löschen; alles danach rückt auf. */
export async function deleteRange(): Promise<boolean> {
  const r = snapshot.peek()?.engine.loopRange;
  if (!r) return false;
  const bpb = project()?.beatsPerBar ?? 4;
  if (!(await act(backend.deleteRange(r[0], r[1])))) return false;
  rangeFocus.value = false;
  noteSel.value = null;
  // gewählter Clip kann weggefallen sein
  if (!findClip(project(), selection.peek()) && selection.peek()?.clipId != null) selection.value = null;
  notify(t("Deleted {range} – everything after it moved up to close the gap (Ctrl+Z brings it back)", { range: rangeLabel(r[0], r[1], bpb) }));
  return true;
}

/** Aufnahme in eine Melodie- oder Rhythmus-Spur umwandeln (`clipId` null: alle Audio-Clips der Spur). */
export async function convertAudio(trackId: number, clipId: number | null, target: ConvertTarget): Promise<boolean> {
  notify(target === "melodie" ? t("Detecting pitches…") : t("Detecting hits…"));
  try {
    const r = await backend.audioToNotes(trackId, clipId, target);
    snapshot.value = r.snapshot;
    selection.value = { trackId: r.trackId, clipId: null };
    const name = r.snapshot.project.tracks.find((t) => t.id === r.trackId)?.name ?? t("New track");
    const how = target === "melodie" ? t("quantized to sixteenths") : t("kick, snare and hi-hat");
    notify(plural(r.notes, "Created \"{name}\": {n} note ({how})", "Created \"{name}\": {n} notes ({how})", { name, how }));
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

async function guarded(fn: () => Promise<unknown>): Promise<void> {
  try {
    await fn();
  } catch (e) {
    notify(errorText(e), "error");
  }
}

export const togglePlay = () => guarded(() => backend.transport("toggle"));

/** Stopp: anhalten und an den Anfang – Abspielposition und Ansicht. */
export const stopToStart = () =>
  guarded(async () => {
    if (status.peek().playing) await backend.transport("stop");
    await backend.transport("rewind");
    viewport.update((v) => ({ ...v, scrollX: 0 }));
  });

/** Wiederholen: am Ende des Stücks (bzw. des markierten Bereichs) wieder von vorn. */
export async function toggleLoop(): Promise<void> {
  const on = !snapshot.peek()?.engine.loopOn;
  if (await act(backend.setLoop(on))) {
    const range = snapshot.peek()?.engine.loopRange;
    notify(on ? (range ? t("Loop: selected range") : t("Loop: back to the start at the end")) : t("Loop off"));
  }
}

export function seekBeats(beats: number): void {
  const bpm = project()?.tempoBpm ?? 120;
  void guarded(() => backend.seek((Math.max(0, beats) * 60) / bpm));
}

export async function toggleRecord(): Promise<void> {
  if (!status.peek().recording) {
    await guarded(async () => {
      await backend.recordStart();
      // the metronome falls silent for the recording: show that on its button
      snapshot.value = await backend.getState();
      notify(t("Recording – press again (or R) to stop"));
    });
    return;
  }
  await guarded(async () => {
    const r = await backend.recordStop();
    snapshot.value = r.snapshot;
    analysis.value = r.analysis;
    selection.value = { trackId: r.trackId, clipId: r.clipId };
    showPanel("analyse");
    const key = r.analysis.key ? ` · ${r.analysis.key.name}` : "";
    notify(`${t("Detected {n} notes", { n: r.analysis.notes.length })}${key}`);
  });
}

export const undo = () => act(backend.undo());
export const redo = () => act(backend.redo());

export function addTrack(kind: TrackKind): void {
  const count = (project()?.tracks.filter((t) => t.kind === kind).length ?? 0) + 1;
  const name = kind === "audio" ? t("Audio {n}", { n: count }) : t("Instrument {n}", { n: count });
  void run({ type: "addTrack", track: { id: 0, name, kind } });
}

/**
 * Entf: markierte Noten, sonst – wenn zuletzt ein Zeitbereich markiert wurde
 * oder nichts anderes gewählt ist – den Zeitbereich aus allen Spuren, sonst
 * den gewählten Clip bzw. die gewählte Spur.
 */
export function deleteSelection(): void {
  if (deleteSelectedNotes()) return;
  const sel = selection.peek();
  if (snapshot.peek()?.engine.loopRange && (rangeFocus.peek() || !sel)) {
    void deleteRange();
    return;
  }
  if (!sel) return;
  if (sel.clipId != null) {
    void run({ type: "removeClip", trackId: sel.trackId, clipId: sel.clipId });
  } else {
    void run({ type: "removeTrack", trackId: sel.trackId });
  }
  selection.value = null;
}

export async function loadDemo(): Promise<void> {
  if (await act(backend.createDemo())) {
    notify(t("Demo loaded: beat (96 BPM) and arpeggio (128 BPM) play in sync at 110 BPM"));
  }
}

export async function analyzeSelected(): Promise<void> {
  const found = findClip(project(), selection.peek());
  if (!found || found.clip.source.type !== "audio") {
    notify(t("Please select an audio clip first."));
    return;
  }
  const assetId = found.clip.source.assetId;
  await guarded(async () => {
    analysis.value = await backend.analyzeAsset(assetId);
    showPanel("analyse");
  });
}

export async function detectTempo(): Promise<void> {
  const found = findClip(project(), selection.peek());
  if (!found || found.clip.source.type !== "audio") return;
  const { track, clip } = found;
  const assetId = found.clip.source.assetId;
  await guarded(async () => {
    const a = await backend.analyzeAsset(assetId);
    analysis.value = a;
    if (!a.tempoEstimate) {
      notify(t("No clear tempo detected."));
      return;
    }
    const bpm = Math.round(a.tempoEstimate * 10) / 10;
    if (await run({ type: "setClip", trackId: track.id, clipId: clip.id, patch: { sourceBpm: bpm, warp: true } })) {
      notify(t("Source tempo set to {bpm} BPM – the clip now plays at the project tempo", { bpm }));
    }
  });
}

export async function melodyToTrack(): Promise<void> {
  const sel = selection.peek();
  let found = findClip(project(), sel);
  const a = analysis.peek();
  // Falls nichts gewählt ist: den Clip zur aktuellen Analyse suchen.
  if ((!found || found.clip.source.type !== "audio") && a) {
    for (const t of project()?.tracks ?? []) {
      const c = t.clips.find((c) => c.source.type === "audio" && c.source.assetId === a.assetId);
      if (c) found = { track: t, clip: c };
    }
  }
  if (!found || found.clip.source.type !== "audio") {
    notify(t("Please select an audio clip."));
    return;
  }
  if (await act(backend.createMelodyTrack(found.track.id, found.clip.id))) {
    notify(t("Melody created as an instrument track (quantized to sixteenths)"));
  }
}
