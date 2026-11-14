// Web-Hülle, Worker: hier läuft der Kern (`klang_web.wasm`) – wie auf dem
// Desktop der Rust-Prozess hinter dem Tauri-IPC.
//
// * Befehle der Seite → `kw_invoke` (dieselbe Befehlstabelle wie auf dem Desktop).
// * Ereignisse des Kerns → Seite (wie Tauri-Events).
// * Status alle 33 ms → Seite („engine-status“; Desktop: Status-Thread).
// * Engine-Nachrichten und Klangpuffer → Worklet (MessageChannel);
//   Status und Mikrofon-Samples kommen von dort zurück.
// * Dateien: beim Start aus OPFS in den Dateibaum des Kerns, Änderungen nach
//   jedem Befehl zurück nach OPFS. Exporte bleiben nur im Speicher (sie werden
//   heruntergeladen).

interface Exports {
  memory: WebAssembly.Memory;
  kw_alloc(len: number): number;
  kw_free(ptr: number, len: number): void;
  kw_out_ptr(): number;
  kw_out_len(): number;
  kw_init(sampleRate: number, audio: number): void;
  kw_agent_hint(ptr: number, len: number): void;
  kw_invoke(cmd: number, cmdLen: number, args: number, argsLen: number): number;
  kw_status(): void;
  kw_mcp(ptr: number, len: number): void;
  kw_tick(): void;
  kw_engine_report(playing: number, previewing: number, position: number, peakOut: number, peakIn: number): void;
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
}

interface Request {
  id: number;
  cmd: string;
  args: string;
  binary: boolean;
  data?: ArrayBuffer;
}

/** Nachrichten des Worklets. */
type FromWorklet = { s: [number, number, number, number, number] } | { rec: Float32Array } | { flushed: number };

const scope = self as unknown as {
  postMessage(m: unknown, transfer?: Transferable[]): void;
  onmessage: ((e: MessageEvent) => void) | null;
};

/** Größe eines Pufferstücks ans Worklet (Samples) – kleine Stücke, damit das
 *  Worklet beim Kopieren nie lange am Stück beschäftigt ist. */
const CHUNK = 1 << 17;
/** Nur im Speicher (werden heruntergeladen, nicht gesichert). */
const TRANSIENT = "/Klangwelt/Exporte/";
const OPFS_ROOT = "Klangwelt";

const enc = new TextEncoder();
const dec = new TextDecoder();
let wasm: Exports;
let port: MessagePort | null = null;
const flushes = new Map<number, () => void>();
let flushId = 0;

// ---------------------------------------------------------------- Speicher

function view(): Uint8Array {
  return new Uint8Array(wasm.memory.buffer);
}

/** Bytes in den linearen Speicher; `fn` bekommt Zeiger und Länge. */
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

// ---------------------------------------------------------------- Kern laden

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
  // Ein Server, der .wasm nicht als application/wasm schickt, bricht das
  // Laden im Strom ab – dann eben am Stück.
  const { instance } = await WebAssembly.instantiateStreaming(fetch(url), imports).catch(async () =>
    WebAssembly.instantiate(await (await fetch(url)).arrayBuffer(), imports),
  );
  wasm = instance.exports as unknown as Exports;
  await restoreFiles();
  port = init.port;
  if (port) port.onmessage = (e: MessageEvent<FromWorklet>) => fromWorklet(e.data);
  wasm.kw_init(init.sampleRate, init.audio ? 1 : 0);
  withText(init.agentHint, (p, l) => wasm.kw_agent_hint(p, l));
  // Status wie der Status-Thread des Desktops; Vorhören nachfüllen wie der Engine-Thread
  // Nach einem internen Fehler des Kerns nicht alle paar Millisekunden dieselbe Meldung
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
    const [playing, previewing, pos, peakOut, peakIn] = m.s;
    wasm.kw_engine_report(playing, previewing, pos, peakOut, peakIn);
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

/** Letzte Mikrofon-Samples aus dem Worklet abholen (vor dem Ende einer
 *  Aufnahme – auf dem Desktop leert der Engine-Thread den Ringpuffer). */
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

// ---------------------------------------------------------------- Dateien (OPFS)

async function opfsRoot(): Promise<FileSystemDirectoryHandle | null> {
  try {
    const root = await navigator.storage.getDirectory();
    return await root.getDirectoryHandle(OPFS_ROOT, { create: true });
  } catch (e) {
    console.warn("Klangwelt: kein Speicher im Browser (OPFS) – nichts wird gesichert.", e);
    return null;
  }
}

/** Alle gesicherten Dateien in den Dateibaum des Kerns. */
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

/** Änderungen des Kerns nach OPFS (der Reihe nach, im Hintergrund). */
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

// ---------------------------------------------------------------- Agent: wartender chat_poll
//
// Auf dem Desktop wartet `chat_poll` im Kern (ein Thread je Verbindung). Hier
// hat der Kern nur einen Thread und darf nicht blockieren – also wartet der
// Worker: Er hält die Antwort offen, bis die Oberfläche einen Auftrag schickt
// (`chat_send`) oder die Wartezeit abläuft, und fragt dann den Kern erneut.
// Alle anderen Befehle laufen in der Zeit normal weiter. (Ein Service Worker
// ginge dafür nicht: Er sieht den Zustand des Kerns nicht, und der Browser
// beendet ihn im Leerlauf.)

interface HeldPoll {
  retry(last: boolean): void;
}
const heldPolls = new Set<HeldPoll>();
/** „Agent hört zu“ gilt im Kern 90 s nach dem letzten Poll – solange einer
 *  wartet, frischt der Worker das auf. */
const POLL_REFRESH_MS = 30_000;

function mcpCore(message: string): string {
  withText(message, (p, l) => wasm.kw_mcp(p, l));
  return outText() || "null";
}

/** Zahl der Aufträge in einer Antwort auf chat_poll; −1 bei Fehlern (dann nicht warten). */
function pollTasks(json: string): number {
  try {
    const r = JSON.parse(json) as { result?: { isError?: boolean; content?: Array<{ text?: string }> } };
    const text = r.result?.content?.[0]?.text;
    if (r.result?.isError || typeof text !== "string") return -1;
    return ((JSON.parse(text) as { auftraege?: unknown[] }).auftraege ?? []).length;
  } catch {
    return -1;
  }
}

function mcp(message: string): Promise<string> {
  let req: { method?: string; params?: { name?: string; arguments?: { timeoutSeconds?: unknown } } } | null = null;
  try {
    req = JSON.parse(message);
  } catch {
    // ungültiges JSON beantwortet der Kern (Parse-Fehler)
  }
  const first = mcpCore(message);
  if (req?.method !== "tools/call" || req.params?.name !== "chat_poll" || pollTasks(first) !== 0) return Promise.resolve(first);
  // wie im Kern: Standard 50 s, höchstens 590 s
  const secs = Math.min(590, Math.max(1, Number(req.params.arguments?.timeoutSeconds) || 50));
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

/** Ein Auftrag kam herein: wartende Polls fragen den Kern erneut. */
function wakePolls(): void {
  for (const held of [...heldPolls]) held.retry(false);
}

// ---------------------------------------------------------------- Befehle

function reply(m: unknown, transfer: Transferable[] = []): void {
  scope.postMessage(m, transfer);
}

async function handle(r: Request): Promise<void> {
  try {
    switch (r.cmd) {
      // Hülle: Datei des Dateibaums lesen (Download) bzw. schreiben (Import)
      case "web_read_file": {
        const { path } = JSON.parse(r.args) as { path: string };
        const status = withText(path, (p, l) => wasm.kw_fs_read(p, l));
        if (status !== 0) throw outText();
        const bytes = outBytes();
        reply({ id: r.id, ok: true, bytes: bytes.buffer }, [bytes.buffer]);
        return;
      }
      // Agent: MCP-Nachricht (JSON-RPC) an die Werkzeuge des Kerns
      case "web_mcp": {
        reply({ id: r.id, ok: true, json: await mcp(r.args) });
        break;
      }
      // Hülle: warten, bis alle Änderungen in OPFS gesichert sind
      case "web_sync": {
        persistChanges();
        await persisting;
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
        const status = withText(r.cmd, (cp, cl) => withText(r.args, (ap, al) => wasm.kw_invoke(cp, cl, ap, al)));
        if (status === 0) reply({ id: r.id, ok: true, json: outText() });
        else reply({ id: r.id, ok: false, error: outText() });
        if (status === 0 && r.cmd === "chat_send") wakePolls();
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

// Modul (eigener Gültigkeitsbereich), geladen als ES-Modul.
export {};
