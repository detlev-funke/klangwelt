// Web-Hülle, Seite: Klangwelt im Browser ohne Server-Prozess. Gegenstück zur
// Tauri-Hülle (`app/src/lib.rs`):
//
// * Der Kern (`klang_web.wasm`) läuft in einem Web Worker (`worker.ts`) – wie
//   auf dem Desktop der Rust-Prozess hinter dem IPC.
// * Die Engine mischt in einem AudioWorklet (`worklet.ts`, `klang_worklet.wasm`);
//   Worker und Worklet sprechen direkt über einen MessageChannel.
// * Was nur der Browser kann oder braucht, steht hier: Mikrofon freigeben,
//   Dateien herunterladen und hereinziehen, „Seite verlassen?“ bei
//   ungespeicherten Änderungen, Service Worker (offline).
//
// Befehle, die es auf dem Desktop nur in der Tauri-Hülle gibt, beantwortet
// diese Hülle selbst (`SHELL`).

import type { CoreLink } from "../bridge.js";
import { t } from "../i18n.js";

/** Antwort des Workers auf einen Befehl. */
type Reply = { id: number; ok: true; json?: string; bytes?: ArrayBuffer } | { id: number; ok: false; error: string };
type FromWorker = Reply | { ev: string; payload: string } | { ready: true } | { failed: string };

/** Gemeinsam mit den Tests: Audio-Knoten der Seite (für Pegelmessungen). */
export interface WebAudioInfo {
  ctx: AudioContext | null;
  node: AudioWorkletNode | null;
  error: string | null;
}

declare global {
  interface Window {
    __klangweltAudio?: WebAudioInfo;
    /** Für Tests und Werkzeuge: Befehl an den Kern, Sichern abwarten. */
    __klangweltWeb?: {
      call<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
      sync(): Promise<void>;
      /** Datei des Rechners in den Dateibaum des Kerns; liefert den Pfad dort. */
      importFile(f: File): Promise<string>;
      /** Agent: eine MCP-Nachricht (JSON-RPC) an die Werkzeuge des Kerns – dieselben wie auf dem Desktop. */
      mcp(message: Record<string, unknown>): Promise<unknown>;
    };
  }
}

const AGENT_HINT = t(
  "There's no agent server (MCP) in the browser – a web page can't open a port. An agent can still operate Klangwelt: via a browser extension or the Chrome DevTools Protocol, see docs/WEB.md.",
);

const NOT_IN_BROWSER = t("Not yet available in the browser – see docs/WEB.md.");

/** The worker's code for "the package on the server can't be reached" (web/worker.ts). */
const LIBRARY_OFFLINE = "klangwelt:library-offline";
const LIBRARY_OFFLINE_TEXT = t("The score library on the server can't be reached right now – offline, only scores you've used before are available.");

/** Ordner im Dateibaum des Kerns, in den hereingezogene Dateien kommen. */
const IMPORT_DIR = "/Klangwelt/Import";
const PROJECTS_DIR = "/Klangwelt/Projekte";

export function createWebLink(): CoreLink {
  const listeners = new Map<string, Array<(p: unknown) => void>>();
  const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: unknown) => void; binary: boolean }>();
  let nextId = 0;
  /** Ungespeicherte Änderungen (aus der letzten Antwort mit Projektstand). */
  let dirty = false;

  const emit = (ev: string, payload: unknown) => {
    for (const cb of listeners.get(ev) ?? []) cb(payload);
  };

  const audio: WebAudioInfo = { ctx: null, node: null, error: null };
  window.__klangweltAudio = audio;
  let worker: Worker | null = null;
  let mic: MediaStream | null = null;

  const ready = (async () => {
    const port = await startAudio(audio);
    worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
    const started = new Promise<void>((resolve, reject) => {
      worker!.onmessage = (e: MessageEvent<FromWorker>) => {
        const m = e.data;
        if ("ready" in m) resolve();
        else if ("failed" in m) reject(new Error(m.failed));
        else if ("ev" in m) {
          const payload = JSON.parse(m.payload);
          // auch Änderungen von außen (Agent) können ungespeichert sein
          if (m.ev === "project-changed" && payload && typeof payload.dirty === "boolean") dirty = payload.dirty;
          emit(m.ev, payload);
        }
        else onReply(m);
      };
      worker!.onerror = (e) => reject(new Error(t("Web core: {message}", { message: e.message })));
    });
    const init = { sampleRate: audio.ctx?.sampleRate ?? 48000, audio: !!port, agentHint: AGENT_HINT, port, page: location.href };
    worker.postMessage({ init }, port ? [port] : []);
    await started;
  })();
  ready.catch((e) => console.error("Klangwelt (Browser):", e));

  function onReply(m: Reply): void {
    const p = pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    if (!m.ok) {
      p.reject(m.error === LIBRARY_OFFLINE ? LIBRARY_OFFLINE_TEXT : m.error);
      return;
    }
    if (p.binary) {
      p.resolve(m.bytes);
      return;
    }
    const value = m.json == null ? null : JSON.parse(m.json);
    // „Seite verlassen?“ braucht den Stand sofort (beforeunload wartet nicht)
    const snap = value && typeof value === "object" ? ((value as { snapshot?: unknown }).snapshot ?? value) : null;
    if (snap && typeof snap === "object" && "dirty" in snap) dirty = !!(snap as { dirty: boolean }).dirty;
    p.resolve(value);
  }

  /** Befehl an den Worker (Kern oder Worker-Hülle). `binary`: Antwort als
   *  Bytes; `data`: Bytes mitschicken (z. B. eine hereingezogene Datei). */
  async function send<T>(cmd: string, args: Record<string, unknown> | undefined, opts: { binary?: boolean; data?: ArrayBuffer } = {}): Promise<T> {
    await ready;
    const id = ++nextId;
    const binary = !!opts.binary;
    return new Promise<T>((resolve, reject) => {
      pending.set(id, { resolve: resolve as (v: unknown) => void, reject, binary });
      worker!.postMessage({ id, cmd, args: args === undefined ? "" : JSON.stringify(args), binary, data: opts.data }, opts.data ? [opts.data] : []);
    });
  }

  /** Datei aus dem Dateibaum des Kerns herunterladen. */
  async function download(path: string): Promise<void> {
    const bytes = await send<ArrayBuffer>("web_read_file", { path }, { binary: true });
    const name = path.split("/").pop() || "Klangwelt";
    const url = URL.createObjectURL(new Blob([bytes]));
    const a = document.createElement("a");
    a.href = url;
    a.download = name;
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
  }

  /** Mikrofon an den Eingang des Worklets (einmal freigeben, dann bleibt es verbunden). */
  async function ensureMic(): Promise<void> {
    if (mic) return;
    if (!audio.ctx || !audio.node) throw audio.error ?? t("No audio in the browser");
    if (!navigator.mediaDevices?.getUserMedia) throw t("This browser doesn't allow microphone access (only via https or localhost).");
    try {
      mic = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false } });
    } catch (e) {
      throw t("Microphone access denied: {error}", { error: String((e as Error).message ?? e) });
    }
    audio.ctx.createMediaStreamSource(mic).connect(audio.node);
    await audio.ctx.resume();
  }

  /** Was auf dem Desktop die Tauri-Hülle tut, macht hier die Seite. */
  const SHELL: Record<string, (args: Record<string, unknown>) => Promise<unknown>> = {
    // Kein Fenster zum Schließen: die Seite bleibt offen.
    app_close: async () => null,
    // Statt „Im Ordner zeigen“: herunterladen
    export_reveal: async (a) => download(String(a.path)),
    // files of the computer into the core's file tree (e.g. for the own scores)
    web_import_files: async (a) => {
      const paths: string[] = [];
      for (const f of (a.files as File[]) ?? []) paths.push(await importFile(f));
      return paths;
    },
    library_list: async () => [],
    library_create: async () => Promise.reject(NOT_IN_BROWSER),
    library_switch: async () => Promise.reject(NOT_IN_BROWSER),
    library_rename: async () => Promise.reject(NOT_IN_BROWSER),
    library_delete: async () => Promise.reject(NOT_IN_BROWSER),
    library_catalog: async () => [],
    library_download: async () => Promise.reject(NOT_IN_BROWSER),
    library_cancel: async () => null,
    library_enrich: async () => Promise.reject(NOT_IN_BROWSER),
    library_reindex: async () => Promise.reject(NOT_IN_BROWSER),
  };

  const call = async <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
    const shell = SHELL[cmd];
    if (shell) return (await shell(args ?? {})) as T;
    if (cmd === "record_start" || cmd === "library_listen_start") await ensureMic();
    const result = await send<T>(cmd, args);
    // Export: im Browser gleich herunterladen (der Kern legt die Datei in seinen Dateibaum)
    if (cmd === "export_run") {
      const r = result as unknown as { files: string[] };
      for (const f of r.files) await download(f);
    }
    return result;
  };

  // Abspielen braucht eine Geste der Nutzerin / des Nutzers (Autoplay-Regel)
  const resume = () => {
    if (audio.ctx && audio.ctx.state !== "running") void audio.ctx.resume();
  };
  window.addEventListener("pointerdown", resume, true);
  window.addEventListener("keydown", resume, true);

  // Ungespeicherte Änderungen: der Browser fragt beim Verlassen nach
  window.addEventListener("beforeunload", (e) => {
    if (dirty) e.preventDefault();
  });

  /** Datei des Rechners in den Dateibaum des Kerns legen (Projekte in den
   *  Projektordner, alles andere nach „Import“); liefert den Pfad dort. */
  async function importFile(f: File): Promise<string> {
    const project = /\.klangwelt$/i.test(f.name);
    const path = `${project ? PROJECTS_DIR : IMPORT_DIR}/${f.name.replace(/[\\/]/g, "_")}`;
    await send("web_write_file", { path }, { data: await f.arrayBuffer() });
    return path;
  }

  // Dateien hereinziehen: Projektdateien öffnen, alles andere wie auf dem
  // Desktop als Pfade melden (im Dateibaum des Kerns) – samt Stelle und
  // Zieh-Ereignissen („file-drag“) für die Einfügemarke der Zeitleiste.
  // Beim Ziehen verrät der Browser keine Namen, nur die Art; daraus ein
  // Platzhalter mit passender Endung (für die Erkennung „Audiodatei“).
  const EXT: Record<string, string> = { "audio/wav": ".wav", "audio/x-wav": ".wav", "audio/wave": ".wav", "audio/aiff": ".aiff", "audio/x-aiff": ".aiff", "audio/flac": ".flac", "audio/x-flac": ".flac", "audio/ogg": ".opus", "audio/opus": ".opus" };
  const dragNames = (dt: DataTransfer | null) => [...(dt?.items ?? [])].filter((i) => i.kind === "file").map((i, n) => `ziehen-${n}${EXT[i.type] ?? ""}`);
  window.addEventListener("dragover", (e) => {
    if (!e.dataTransfer?.types.includes("Files")) return;
    e.preventDefault();
    emit("file-drag", { paths: dragNames(e.dataTransfer), x: e.clientX, y: e.clientY });
  });
  window.addEventListener("dragleave", (e) => {
    if (e.relatedTarget === null) emit("file-drag", null);
  });
  window.addEventListener("drop", (e) => {
    const files = [...(e.dataTransfer?.files ?? [])];
    emit("file-drag", null);
    if (!files.length) return;
    e.preventDefault();
    const position = { x: e.clientX, y: e.clientY };
    void (async () => {
      const paths: string[] = [];
      for (const f of files) {
        let path: string;
        try {
          path = await importFile(f);
        } catch (err) {
          console.error(`${f.name}:`, err);
          continue;
        }
        if (path.startsWith(PROJECTS_DIR)) emit("open-file", path);
        else paths.push(path);
      }
      if (paths.length) emit("file-drop", { paths, position });
    })();
  });

  registerServiceWorker();
  window.__klangweltWeb = { call, importFile, sync: () => send("web_sync", undefined), mcp: (message) => send("web_mcp", message) };

  return {
    call,
    listen: (event, cb) => {
      const l = listeners.get(event) ?? [];
      l.push(cb as (p: unknown) => void);
      listeners.set(event, l);
    },
  };
}

/** AudioContext und Worklet starten; liefert den Port für den Worker (oder
 *  null ohne Audio – dann läuft alles andere trotzdem). */
async function startAudio(audio: WebAudioInfo): Promise<MessagePort | null> {
  try {
    const ctx = new AudioContext({ latencyHint: "interactive" });
    await ctx.audioWorklet.addModule(new URL("./worklet.js", import.meta.url));
    const wasm = await (await fetch(new URL("../klang_worklet.wasm", import.meta.url))).arrayBuffer();
    const node = new AudioWorkletNode(ctx, "klangwelt", {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      processorOptions: { wasm },
    });
    node.connect(ctx.destination);
    const channel = new MessageChannel();
    node.port.postMessage({ port: channel.port1 }, [channel.port1]);
    audio.ctx = ctx;
    audio.node = node;
    return channel.port2;
  } catch (e) {
    audio.error = t("No audio in the browser: {error}", { error: String((e as Error).message ?? e) });
    console.warn(audio.error);
    return null;
  }
}

/** Service Worker: App-Dateien und WebAssembly im Cache – läuft dann auch
 *  offline. `?ohne-sw` an der Adresse schaltet ihn ab (zum Entwickeln). */
function registerServiceWorker(): void {
  if (!("serviceWorker" in navigator)) return;
  const off = new URLSearchParams(location.search).has("ohne-sw");
  if (off) {
    void navigator.serviceWorker.getRegistrations().then((rs) => rs.forEach((r) => void r.unregister()));
    return;
  }
  const register = () =>
    void navigator.serviceWorker.register(new URL("../sw.js", import.meta.url), { scope: "./", type: "module" }).catch((e) => console.warn("Service Worker:", e));
  // the app loads after the dictionary (main.ts awaits it) – "load" may have fired already
  if (document.readyState === "complete") register();
  else window.addEventListener("load", register);
}
