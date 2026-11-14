// Web shell, page side: Klangwelt in the browser without a server process. Counterpart to the
// Tauri shell (`app/src/lib.rs`):
//
// * The core (`klang_web.wasm`) runs in a web worker (`worker.ts`) – like the
//   Rust process behind the IPC on the desktop.
// * The engine mixes in an AudioWorklet (`worklet.ts`, `klang_worklet.wasm`);
//   worker and worklet talk directly over a MessageChannel.
// * What only the browser can do or needs is here: releasing the microphone,
//   downloading and dropping in files, "Leave page?" for
//   unsaved changes, service worker (offline).
//
// Commands that exist on the desktop only in the Tauri shell are answered by
// this shell itself (`SHELL`).

import type { CoreLink } from "../bridge.js";
import { t } from "../i18n.js";

/** Answer of the worker to a command. */
type Reply = { id: number; ok: true; json?: string; bytes?: ArrayBuffer } | { id: number; ok: false; error: string };
type FromWorker = Reply | { ev: string; payload: string } | { ready: true } | { failed: string };

/** Shared with the tests: audio nodes of the page (for level measurements). */
export interface WebAudioInfo {
  ctx: AudioContext | null;
  node: AudioWorkletNode | null;
  error: string | null;
}

declare global {
  interface Window {
    __klangweltAudio?: WebAudioInfo;
    /** For tests and tools: command to the core, wait for saving. */
    __klangweltWeb?: {
      call<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
      sync(): Promise<void>;
      /** Put a file from the computer into the core's file tree; returns the path there. */
      importFile(f: File): Promise<string>;
      /** Agent: an MCP message (JSON-RPC) to the core's tools – the same as on the desktop. */
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

/** Folder in the core's file tree where dropped-in files go. */
const IMPORT_DIR = "/Klangwelt/Import";
const PROJECTS_DIR = "/Klangwelt/Projekte";

export function createWebLink(): CoreLink {
  const listeners = new Map<string, Array<(p: unknown) => void>>();
  const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: unknown) => void; binary: boolean }>();
  let nextId = 0;
  /** Unsaved changes (from the last answer with project state). */
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
          // changes from outside (agent) can also be unsaved
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
    // "Leave page?" needs the state immediately (beforeunload does not wait)
    const snap = value && typeof value === "object" ? ((value as { snapshot?: unknown }).snapshot ?? value) : null;
    if (snap && typeof snap === "object" && "dirty" in snap) dirty = !!(snap as { dirty: boolean }).dirty;
    p.resolve(value);
  }

  /** Command to the worker (core or worker shell). `binary`: answer as
   *  bytes; `data`: send bytes along (e.g. a dropped-in file). */
  async function send<T>(cmd: string, args: Record<string, unknown> | undefined, opts: { binary?: boolean; data?: ArrayBuffer } = {}): Promise<T> {
    await ready;
    const id = ++nextId;
    const binary = !!opts.binary;
    return new Promise<T>((resolve, reject) => {
      pending.set(id, { resolve: resolve as (v: unknown) => void, reject, binary });
      worker!.postMessage({ id, cmd, args: args === undefined ? "" : JSON.stringify(args), binary, data: opts.data }, opts.data ? [opts.data] : []);
    });
  }

  /** Download a file from the core's file tree. */
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

  /** Microphone to the worklet's input (release once, then it stays connected). */
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

  /** What the Tauri shell does on the desktop, the page does here. */
  const SHELL: Record<string, (args: Record<string, unknown>) => Promise<unknown>> = {
    // No window to close: the page stays open.
    app_close: async () => null,
    // Instead of "Show in folder": download
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
    // Export: in the browser, download right away (the core puts the file in its file tree)
    if (cmd === "export_run") {
      const r = result as unknown as { files: string[] };
      for (const f of r.files) await download(f);
    }
    return result;
  };

  // Playing needs a gesture by the user (autoplay rule)
  const resume = () => {
    if (audio.ctx && audio.ctx.state !== "running") void audio.ctx.resume();
  };
  window.addEventListener("pointerdown", resume, true);
  window.addEventListener("keydown", resume, true);

  // Unsaved changes: the browser asks when leaving
  window.addEventListener("beforeunload", (e) => {
    if (dirty) e.preventDefault();
  });

  /** Put a file from the computer into the core's file tree (projects into the
   *  project folder, everything else under "Import"); returns the path there. */
  async function importFile(f: File): Promise<string> {
    const project = /\.klangwelt$/i.test(f.name);
    const path = `${project ? PROJECTS_DIR : IMPORT_DIR}/${f.name.replace(/[\\/]/g, "_")}`;
    await send("web_write_file", { path }, { data: await f.arrayBuffer() });
    return path;
  }

  // Dropping in files: open project files, report everything else as paths like on the
  // desktop (in the core's file tree) – including position and
  // drag events ("file-drag") for the timeline's insertion marker.
  // While dragging, the browser does not reveal names, only the kind; from that a
  // placeholder with a fitting extension (for detecting "audio file").
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

/** Start AudioContext and worklet; returns the port for the worker (or
 *  null without audio – then everything else runs anyway). */
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

/** Service worker: app files and WebAssembly in the cache – then it also runs
 *  offline. `?ohne-sw` in the address turns it off (for development). */
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
