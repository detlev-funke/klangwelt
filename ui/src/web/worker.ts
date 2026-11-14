// Web shell, worker: the core (`klang_web.wasm`) runs here – like the Rust
// process behind the Tauri IPC on desktop.
//
// * Commands from the page → `kw_invoke` (same command table as on desktop).
// * Events from the core → page (like Tauri events).
// * Status every 33 ms → page ("engine-status"; desktop: status thread).
// * Engine messages and sound buffers → worklet (MessageChannel);
//   status and microphone samples come back from there.
// * Files: at startup from OPFS into the core's file tree, changes after
//   each command back to OPFS. Exports stay in memory only (they are
//   downloaded).
// * Score library: the package on the server is read by range – a command
//   that misses data answers with status 2 and the missing ranges; this
//   worker fetches them (`remote.ts`, cache first), hands them to the core and
//   runs the command again. Own scores live in the core's file tree (OPFS).

import { cachedBytes, clearCache, configure, fetchNeeds, manifest, onWait, setPage, type Need } from "./remote.js";

interface Exports {
  memory: WebAssembly.Memory;
  kw_alloc(len: number): number;
  kw_free(ptr: number, len: number): void;
  kw_out_ptr(): number;
  kw_out_len(): number;
  kw_init(sampleRate: number, audio: number): void;
  kw_agent_hint(ptr: number, len: number): void;
  kw_invoke(cmd: number, cmdLen: number, args: number, argsLen: number): number;
  kw_lib_feed(file: number, fileLen: number, start: number, data: number, dataLen: number): number;
  kw_status(): void;
  kw_mcp(ptr: number, len: number): number;
  kw_tick(): void;
  kw_engine_report(playing: number, previewing: number, position: number, peakOut: number, peakIn: number, load: number): void;
  kw_record_input(ptr: number, len: number): void;
  kw_fs_put(path: number, pathLen: number, data: number, dataLen: number, modified: number): void;
  kw_fs_write(path: number, pathLen: number, data: number, dataLen: number): number;
  kw_fs_read(path: number, pathLen: number): number;
  kw_fs_changes(): number;
  kw_fs_change_data(i: number): number;
  kw_fs_changes_done(): void;
}

interface Init {
  sampleRate: number;
  audio: boolean;
  agentHint: string;
  port: MessagePort | null;
  /** Address of the page (for `?library=`). */
  page: string;
}

interface Request {
  id: number;
  cmd: string;
  args: string;
  binary: boolean;
  data?: ArrayBuffer;
}

/** Messages from the worklet. */
type FromWorklet = { s: [number, number, number, number, number, number?] } | { rec: Float32Array } | { flushed: number };

const scope = self as unknown as {
  postMessage(m: unknown, transfer?: Transferable[]): void;
  onmessage: ((e: MessageEvent) => void) | null;
};

/** Size of a buffer chunk sent to the worklet (samples) – small chunks so that the
 *  worklet is never busy for long at a stretch while copying. */
const CHUNK = 1 << 17;
/** In memory only (downloaded, not saved). */
const TRANSIENT = "/Klangwelt/Exporte/";
const OPFS_ROOT = "Klangwelt";

const enc = new TextEncoder();
const dec = new TextDecoder();
let wasm: Exports;
let port: MessagePort | null = null;
const flushes = new Map<number, () => void>();
let flushId = 0;

// ---------------------------------------------------------------- Memory

function view(): Uint8Array {
  return new Uint8Array(wasm.memory.buffer);
}

/** Bytes into linear memory; `fn` receives pointer and length. */
function withBytes<T>(bytes: Uint8Array, fn: (ptr: number, len: number) => T): T {
  const ptr = wasm.kw_alloc(bytes.length);
  view().set(bytes, ptr);
  try {
    return fn(ptr, bytes.length);
  } finally {
    wasm.kw_free(ptr, bytes.length);
  }
}

function withText<T>(s: string, fn: (ptr: number, len: number) => T): T {
  return withBytes(enc.encode(s), fn);
}

function outBytes(): Uint8Array {
  const ptr = wasm.kw_out_ptr();
  return view().slice(ptr, ptr + wasm.kw_out_len());
}

function outText(): string {
  return dec.decode(outBytes());
}

function text(ptr: number, len: number): string {
  return dec.decode(view().subarray(ptr, ptr + len));
}

// ---------------------------------------------------------------- Loading the core

const imports = {
  klangwelt: {
    emit(ev: number, evLen: number, payload: number, payloadLen: number) {
      scope.postMessage({ ev: text(ev, evLen), payload: text(payload, payloadLen) });
    },
    engine_post(kind: number, ptr: number, len: number) {
      const d = new Float64Array(wasm.memory.buffer, ptr, len).slice();
      port?.postMessage({ k: kind, d }, [d.buffer]);
    },
    engine_buffer(id: number, ptr: number, len: number) {
      const all = new Float32Array(wasm.memory.buffer, ptr, len);
      if (len === 0) port?.postMessage({ b: id, n: 0, off: 0, d: new Float32Array(0) });
      for (let off = 0; off < len; off += CHUNK) {
        const d = all.slice(off, Math.min(len, off + CHUNK));
        port?.postMessage({ b: id, n: len, off, d }, [d.buffer]);
      }
    },
    now_ms: () => Date.now(),
    mono_ms: () => performance.now(),
    log(ptr: number, len: number) {
      console.error(text(ptr, len));
    },
  },
};

async function start(init: Init): Promise<void> {
  const url = new URL("../klang_web.wasm", import.meta.url);
  // A server that does not send .wasm as application/wasm breaks
  // streaming load – then load it in one piece.
  const { instance } = await WebAssembly.instantiateStreaming(fetch(url), imports).catch(async () =>
    WebAssembly.instantiate(await (await fetch(url)).arrayBuffer(), imports),
  );
  wasm = instance.exports as unknown as Exports;
  await restoreFiles();
  port = init.port;
  if (port) port.onmessage = (e: MessageEvent<FromWorklet>) => fromWorklet(e.data);
  wasm.kw_init(init.sampleRate, init.audio ? 1 : 0);
  withText(init.agentHint, (p, l) => wasm.kw_agent_hint(p, l));
  setPage(init.page);
  // the request limit of the server: tell the page when a request has to wait
  onWait((seconds) => scope.postMessage({ ev: "library-wait", payload: JSON.stringify({ seconds }) }));
  void attachLibrary();
  // Status like the desktop's status thread; refill the preview like the engine thread
  // After an internal core error, do not repeat the same message every few milliseconds
  const timers = [
    setInterval(() => guard(() => {
      wasm.kw_status();
      scope.postMessage({ ev: "engine-status", payload: outText() });
    }), 33),
    setInterval(() => guard(() => wasm.kw_tick()), 20),
  ];
  const guard = (fn: () => void) => {
    try {
      fn();
    } catch (e) {
      timers.forEach(clearInterval);
      console.error("Klangwelt (Web-Kern): interner Fehler – bitte die Seite neu laden.", e);
    }
  };
}

function fromWorklet(m: FromWorklet): void {
  if ("s" in m) {
    const [playing, previewing, pos, peakOut, peakIn, load] = m.s;
    wasm.kw_engine_report(playing, previewing, pos, peakOut, peakIn, load ?? 0);
  } else if ("rec" in m) {
    const bytes = new Uint8Array(m.rec.buffer, m.rec.byteOffset, m.rec.byteLength);
    const ptr = wasm.kw_alloc(bytes.length);
    view().set(bytes, ptr);
    wasm.kw_record_input(ptr, m.rec.length);
    wasm.kw_free(ptr, bytes.length);
  } else if ("flushed" in m) {
    flushes.get(m.flushed)?.();
    flushes.delete(m.flushed);
  }
}

/** Fetch the latest microphone samples from the worklet (before the end of a
 *  recording – on desktop the engine thread drains the ring buffer). */
function flushRecording(): Promise<void> {
  if (!port) return Promise.resolve();
  const id = ++flushId;
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, 1000);
    flushes.set(id, () => {
      clearTimeout(timer);
      resolve();
    });
    port!.postMessage({ flush: id });
  });
}

// ---------------------------------------------------------------- Files (OPFS)

async function opfsRoot(): Promise<FileSystemDirectoryHandle | null> {
  try {
    const root = await navigator.storage.getDirectory();
    return await root.getDirectoryHandle(OPFS_ROOT, { create: true });
  } catch (e) {
    console.warn("Klangwelt: kein Speicher im Browser (OPFS) – nichts wird gesichert.", e);
    return null;
  }
}

/** All saved files into the core's file tree. */
async function restoreFiles(): Promise<void> {
  const root = await opfsRoot();
  if (!root) return;
  const walk = async (dir: FileSystemDirectoryHandle, path: string): Promise<void> => {
    for await (const [name, handle] of (dir as unknown as { entries(): AsyncIterable<[string, FileSystemHandle]> }).entries()) {
      const p = `${path}/${name}`;
      if (handle.kind === "directory") await walk(handle as FileSystemDirectoryHandle, p);
      else {
        const file = await (handle as FileSystemFileHandle).getFile();
        const bytes = new Uint8Array(await file.arrayBuffer());
        withText(p, (pp, pl) => withBytes(bytes, (dp, dl) => wasm.kw_fs_put(pp, pl, dp, dl, file.lastModified)));
      }
    }
  };
  await walk(root, `/${OPFS_ROOT}`);
}

let persisting: Promise<void> = Promise.resolve();

/** Changes of the core to OPFS (in order, in the background). */
function persistChanges(): void {
  const n = wasm.kw_fs_changes();
  if (!n) return;
  const list = JSON.parse(outText()) as Array<[string, number]>;
  const jobs = list.map(([path, len], i) => {
    if (len < 0) return { path, data: null as Uint8Array | null };
    const ptr = wasm.kw_fs_change_data(i);
    return { path, data: view().slice(ptr, ptr + len) };
  });
  wasm.kw_fs_changes_done();
  for (const job of jobs.filter((j) => !j.path.startsWith(TRANSIENT))) {
    persisting = persisting.then(() => store(job.path, job.data)).catch((e) => console.error(`Sichern von ${job.path}:`, e));
  }
}

async function store(path: string, data: Uint8Array | null): Promise<void> {
  const parts = path.split("/").filter(Boolean);
  if (parts.shift() !== OPFS_ROOT) return;
  let dir = await opfsRoot();
  if (!dir) return;
  const name = parts.pop()!;
  for (const p of parts) dir = await dir.getDirectoryHandle(p, { create: data != null });
  if (data == null) {
    await dir.removeEntry(name).catch(() => {});
    return;
  }
  const fh = await dir.getFileHandle(name, { create: true });
  const w = await (fh as unknown as { createWritable(): Promise<{ write(d: BufferSource): Promise<void>; close(): Promise<void> }> }).createWritable();
  await w.write(data as unknown as BufferSource);
  await w.close();
}

// ---------------------------------------------------------------- Score library (package on the server)

/** Error text for the page (translated there, see host.ts). */
export const LIBRARY_OFFLINE = "klangwelt:library-offline";
/** Rounds of data a command may need before it's given up. */
const MAX_ROUNDS = 16;

/** Hands fetched ranges to the core. */
async function feedNeeds(needs: Need[]): Promise<void> {
  const got = await fetchNeeds(needs);
  for (const [n, bytes] of got) withText(n.file, (fp, fl) => withBytes(bytes, (dp, dl) => wasm.kw_lib_feed(fp, fl, n.start, dp, dl)));
}

/** Runs a core function that may miss data (status 2) until it has everything. */
async function withData(run: () => number): Promise<{ status: number; out: string }> {
  for (let round = 0; ; round++) {
    const status = run();
    const out = outText();
    if (status !== 2) return { status, out };
    if (round >= MAX_ROUNDS) return { status: 1, out: "Score library: no progress while reading" };
    try {
      await feedNeeds(JSON.parse(out) as Need[]);
    } catch (e) {
      console.warn("Klangwelt (score library):", e);
      return { status: 1, out: LIBRARY_OFFLINE };
    }
  }
}

function invokeOnce(cmd: string, args: string): number {
  return withText(cmd, (cp, cl) => withText(args, (ap, al) => wasm.kw_invoke(cp, cl, ap, al)));
}

const invoke = (cmd: string, args: unknown) => withData(() => invokeOnce(cmd, JSON.stringify(args)));

/** At start: attach the package from the configuration (library.json). */
async function attachLibrary(): Promise<void> {
  try {
    const src = await configure();
    if (!src) {
      await invoke("library_web_state", { state: "none" });
      return;
    }
    await invoke("library_web_state", { state: "loading" });
    let m: Awaited<ReturnType<typeof manifest>>;
    try {
      m = await manifest();
    } catch (e) {
      await invoke("library_web_state", { state: "offline", error: String((e as Error).message ?? e) });
      return;
    }
    const json = m!.json as Record<string, unknown>;
    if (src.page && !json.url) json.url = src.page;
    const r = await invoke("library_web_attach", { manifest: json });
    if (r.status !== 0) await invoke("library_web_state", { state: r.out === LIBRARY_OFFLINE ? "offline" : "error", error: r.out === LIBRARY_OFFLINE ? null : r.out });
  } catch (e) {
    console.error("Klangwelt (score library):", e);
  } finally {
    persistChanges();
  }
}

// ---------------------------------------------------------------- Agent: waiting chat_poll
//
// On desktop `chat_poll` waits in the core (one thread per connection). Here
// the core has only one thread and must not block – so the worker waits:
// it keeps the answer open until the UI sends a job (`chat_send`) or the
// wait time runs out, and then asks the core again.
// All other commands continue normally in the meantime. (A service worker
// would not work for this: it cannot see the core's state, and the browser
// terminates it when idle.)

interface HeldPoll {
  retry(last: boolean): void;
}
const heldPolls = new Set<HeldPoll>();
/** "Agent is listening" holds in the core for 30 s after the last poll – as long as one
 *  is waiting, the worker refreshes it. Each refresh also asks again, so a task whose
 *  lease ran out (no chat_reply in time) is offered again within these few seconds,
 *  like on the desktop. */
const POLL_REFRESH_MS = 5_000;

function mcpCore(message: string): string {
  withText(message, (p, l) => wasm.kw_mcp(p, l));
  return outText() || "null";
}

/** Like `mcpCore`, with the data a library tool may need from the server. */
async function mcpWithData(message: string): Promise<string> {
  const r = await withData(() => withText(message, (p, l) => wasm.kw_mcp(p, l)));
  if (r.status === 2 || r.out === LIBRARY_OFFLINE) return JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32603, message: "score library not reachable" } });
  return r.out || "null";
}

/** Number of jobs in a response to chat_poll; −1 on errors and for other
 *  responses (`peek`) – then do not wait. */
function pollTasks(json: string): number {
  try {
    const r = JSON.parse(json) as { result?: { isError?: boolean; content?: Array<{ text?: string }> } };
    const text = r.result?.content?.[0]?.text;
    if (r.result?.isError || typeof text !== "string") return -1;
    const tasks = (JSON.parse(text) as { auftraege?: unknown[] }).auftraege;
    return Array.isArray(tasks) ? tasks.length : -1;
  } catch {
    return -1;
  }
}

async function mcp(message: string): Promise<string> {
  let req: { method?: string; params?: { name?: string; arguments?: { timeoutSeconds?: unknown; peek?: unknown } } } | null = null;
  try {
    req = JSON.parse(message);
  } catch {
    // the core answers invalid JSON (parse error)
  }
  const first = await mcpWithData(message);
  const waits = req?.method === "tools/call" && req.params?.name === "chat_poll" && req.params.arguments?.peek !== true;
  if (!waits || pollTasks(first) !== 0) return Promise.resolve(first);
  // as in the core: default 50 s, at most 590 s
  const secs = Math.min(590, Math.max(1, Number(req?.params?.arguments?.timeoutSeconds) || 50));
  return new Promise((resolve) => {
    const held: HeldPoll = {
      retry(last) {
        const json = mcpCore(message);
        if (!last && pollTasks(json) === 0) return;
        clearTimeout(timer);
        clearInterval(refresh);
        heldPolls.delete(held);
        resolve(json);
      },
    };
    const timer = setTimeout(() => held.retry(true), secs * 1000);
    const refresh = setInterval(() => held.retry(false), POLL_REFRESH_MS);
    heldPolls.add(held);
  });
}

/** A job came in: waiting polls ask the core again. */
function wakePolls(): void {
  for (const held of [...heldPolls]) held.retry(false);
}

// ---------------------------------------------------------------- Commands

function reply(m: unknown, transfer: Transferable[] = []): void {
  scope.postMessage(m, transfer);
}

async function handle(r: Request): Promise<void> {
  try {
    switch (r.cmd) {
      // Shell: read (download) or write (import) a file of the file tree
      case "web_read_file": {
        const { path } = JSON.parse(r.args) as { path: string };
        const status = withText(path, (p, l) => wasm.kw_fs_read(p, l));
        if (status !== 0) throw outText();
        const bytes = outBytes();
        reply({ id: r.id, ok: true, bytes: bytes.buffer }, [bytes.buffer]);
        return;
      }
      // Agent: MCP message (JSON-RPC) to the core's tools
      case "web_mcp": {
        reply({ id: r.id, ok: true, json: await mcp(r.args) });
        break;
      }
      // Shell: wait until all changes are saved to OPFS
      case "web_sync": {
        persistChanges();
        await persisting;
        reply({ id: r.id, ok: true, json: "null" });
        return;
      }
      // Score library: what the browser cache holds, emptying it
      case "web_library_cache": {
        reply({ id: r.id, ok: true, json: JSON.stringify(await cachedBytes()) });
        return;
      }
      case "web_library_clear": {
        await clearCache();
        reply({ id: r.id, ok: true, json: "null" });
        return;
      }
      case "web_write_file": {
        const { path } = JSON.parse(r.args) as { path: string };
        const data = new Uint8Array(r.data ?? new ArrayBuffer(0));
        const status = withText(path, (p, l) => withBytes(data, (dp, dl) => wasm.kw_fs_write(p, l, dp, dl)));
        if (status !== 0) throw outText();
        reply({ id: r.id, ok: true, json: "null" });
        break;
      }
      default: {
        if (r.cmd === "record_stop" || r.cmd === "library_listen_stop") await flushRecording();
        const { status, out } = await withData(() => invokeOnce(r.cmd, r.args));
        if (status === 0) reply({ id: r.id, ok: true, json: out });
        else reply({ id: r.id, ok: false, error: out });
        if (status === 0 && (r.cmd === "chat_send" || r.cmd === "chat_stop")) wakePolls();
      }
    }
  } catch (e) {
    const msg = typeof e === "string" ? e : `Interner Fehler im Web-Kern: ${(e as Error).message ?? e}`;
    reply({ id: r.id, ok: false, error: msg });
  }
  persistChanges();
}

scope.onmessage = (e: MessageEvent) => {
  const m = e.data as { init?: Init } & Partial<Request>;
  if (m.init) {
    start(m.init).then(
      () => scope.postMessage({ ready: true }),
      (err) => scope.postMessage({ failed: `Web-Kern startet nicht: ${(err as Error).message ?? err}` }),
    );
    return;
  }
  void handle(m as Request);
};

// Module (own scope), loaded as an ES module.
export {};
