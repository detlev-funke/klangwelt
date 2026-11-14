// Dragging audio files onto the window: one audio track with a clip per file. Over
// the timeline the spot counts – dragged onto an audio track, the first
// file goes into that track, otherwise new tracks are created (below the track under
// the mouse or at the end), starting at the beat under the mouse. Over the track
// headers: into or below that track, from the start. MusicXML is taken by the library
// (library.ts) – both listen to the same Tauri event.

import { backend } from "./bridge.js";
import { libraryTakesDrops } from "./components/library.js";
import { baseName, isAudioPath, isScorePath } from "./files.js";
import { language, t } from "./i18n.js";
import { signal } from "./signals.js";
import { errorText, notify, selection, snapshot } from "./store.js";

/** Target of a drop: track (or null = new track at the end) and start beat. */
export interface DropTarget {
  trackId: number | null;
  startBeats: number;
}

/** Audio files are currently being dragged over the window (spot in CSS pixels, count). */
export const audioDrag = signal<{ x: number; y: number; count: number } | null>(null);

/** Elements that translate a spot into a target (timeline). */
interface DropZone extends Element {
  dropTargetAt(x: number, y: number): DropTarget | null;
}

function isZone(el: Element | null): el is DropZone {
  return !!el && typeof (el as Partial<DropZone>).dropTargetAt === "function";
}

/** Target under a spot (window coordinates). */
export function dropTargetAt(x: number, y: number): DropTarget {
  const el = x >= 0 && y >= 0 ? document.elementFromPoint(x, y) : null;
  const zone = el?.closest("kw-timeline") ?? null;
  if (isZone(zone)) {
    const t = zone.dropTargetAt(x, y);
    if (t) return t;
  }
  const row = el?.closest<HTMLElement>("[data-track-id]");
  if (row?.dataset.trackId) return { trackId: Number(row.dataset.trackId), startBeats: 0 };
  return { trackId: null, startBeats: 0 };
}

function secs(s: number): string {
  if (s < 60) return `${s.toLocaleString(language(), { maximumFractionDigits: 1 })} s`;
  const m = Math.floor(s / 60);
  return `${m}:${String(Math.round(s - m * 60)).padStart(2, "0")} min`;
}

/** Inserts files and notifies (also for the agent-free path via tests). */
export async function importAudio(paths: string[], target: DropTarget): Promise<boolean> {
  notify(paths.length === 1 ? t("Reading \"{name}\"…", { name: baseName(paths[0]!) }) : t("Reading {n} files…", { n: paths.length }));
  try {
    const r = await backend.audioImport(paths, target.trackId, target.startBeats);
    snapshot.value = r.snapshot;
    const first = r.imported[0];
    if (first) selection.value = { trackId: first.trackId, clipId: first.clipId };
    const what = r.imported.length === 1 && first
      ? first.channels > 1
        ? t("Inserted \"{name}\" ({format}, {duration}, mixed down to mono)", { name: first.name, format: first.format, duration: secs(first.seconds) })
        : t("Inserted \"{name}\" ({format}, {duration})", { name: first.name, format: first.format, duration: secs(first.seconds) })
      : t("Inserted {n} audio files", { n: r.imported.length });
    notify(r.errors.length ? t("{what} – couldn't read: {errors}", { what, errors: r.errors.join(" · ") }) : what, r.errors.length ? "error" : "info");
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

let started = false;

/** At startup: listen for dragged files. */
export function initFileDrop(): void {
  if (started) return;
  started = true;
  backend.onFileDrag((d) => {
    const count = d ? d.paths.filter(isAudioPath).length : 0;
    audioDrag.value = d && count ? { x: d.x, y: d.y, count } : null;
  });
  backend.onFileDrop((paths, at) => {
    audioDrag.value = null;
    const audio = paths.filter(isAudioPath);
    if (audio.length) {
      void importAudio(audio, at ? dropTargetAt(at.x, at.y) : { trackId: null, startBeats: 0 });
      return;
    }
    // nothing for Klangwelt: MusicXML and folders are taken by the library when it is open
    if (paths.length && !(paths.some(isScorePath) && libraryTakesDrops())) {
      notify(
        paths.some(isScorePath)
          ? t("MusicXML files go into the library – open the \"Library\" window for that.")
          : t("Klangwelt can't insert these files: audio files (WAV, AIFF, FLAC, Opus) become tracks, MusicXML goes into the library."),
        "error",
      );
    }
  });
}
