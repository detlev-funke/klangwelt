// Audiodateien aufs Fenster ziehen: je Datei eine Audiospur mit Clip. Über
// der Zeitleiste zählt die Stelle – auf eine Audiospur gezogen kommt die erste
// Datei in diese Spur, sonst entstehen neue Spuren (unter der Spur unter der
// Maus bzw. am Ende), ab dem Schlag unter der Maus. Über den Spurköpfen: in
// bzw. unter diese Spur, ab Anfang. MusicXML übernimmt die Bibliothek
// (library.ts) – beide hören auf dasselbe Tauri-Ereignis.

import { backend } from "./bridge.js";
import { libraryTakesDrops } from "./components/library.js";
import { baseName, isAudioPath, isScorePath } from "./files.js";
import { signal } from "./signals.js";
import { errorText, notify, selection, snapshot } from "./store.js";

/** Ziel einer Ablage: Spur (oder null = neue Spur am Ende) und Startschlag. */
export interface DropTarget {
  trackId: number | null;
  startBeats: number;
}

/** Audiodateien werden gerade über das Fenster gezogen (Stelle in CSS-Pixeln, Anzahl). */
export const audioDrag = signal<{ x: number; y: number; count: number } | null>(null);

/** Elemente, die eine Stelle in ein Ziel übersetzen (Zeitleiste). */
interface DropZone extends Element {
  dropTargetAt(x: number, y: number): DropTarget | null;
}

function isZone(el: Element | null): el is DropZone {
  return !!el && typeof (el as Partial<DropZone>).dropTargetAt === "function";
}

/** Ziel unter einer Stelle (Fensterkoordinaten). */
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
  if (s < 60) return `${s.toLocaleString("de-DE", { maximumFractionDigits: 1 })} s`;
  const m = Math.floor(s / 60);
  return `${m}:${String(Math.round(s - m * 60)).padStart(2, "0")} min`;
}

/** Dateien einfügen und Bescheid sagen (auch für den Agenten-freien Weg über Tests). */
export async function importAudio(paths: string[], target: DropTarget): Promise<boolean> {
  notify(paths.length === 1 ? `Lese „${baseName(paths[0]!)}“ …` : `Lese ${paths.length} Dateien …`);
  try {
    const r = await backend.audioImport(paths, target.trackId, target.startBeats);
    snapshot.value = r.snapshot;
    const first = r.imported[0];
    if (first) selection.value = { trackId: first.trackId, clipId: first.clipId };
    const what = r.imported.length === 1 && first
      ? `„${first.name}“ eingefügt (${first.format}, ${secs(first.seconds)}${first.channels > 1 ? ", zu mono gemittelt" : ""})`
      : `${r.imported.length} Audiodateien eingefügt`;
    notify(r.errors.length ? `${what} – nicht gelesen: ${r.errors.join(" · ")}` : what, r.errors.length ? "error" : "info");
    return true;
  } catch (e) {
    notify(errorText(e), "error");
    return false;
  }
}

let started = false;

/** Beim Start: auf gezogene Dateien hören. */
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
    // nichts für Klangwelt: MusicXML und Ordner nimmt die Bibliothek, wenn sie offen ist
    if (paths.length && !(paths.some(isScorePath) && libraryTakesDrops())) {
      notify(
        paths.some(isScorePath)
          ? "MusicXML-Dateien nimmt die Bibliothek auf – dafür das Fenster „Bibliothek“ öffnen."
          : "Diese Dateien kann Klangwelt nicht einfügen: Audiodateien (WAV, AIFF, FLAC, Opus) werden Spuren, MusicXML kommt in die Bibliothek.",
        "error",
      );
    }
  });
}
