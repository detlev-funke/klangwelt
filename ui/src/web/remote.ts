// Web shell, worker: the score library package on a server (Zenodo), read by
// HTTP range. The core asks for ranges it misses (`kw_invoke` → status 2,
// see `klang_music::source::remote`); this module fetches them – from the
// browser's cache if they were read before, otherwise from the server,
// several ranges per request – and keeps what it fetched in the cache. Only
// what a search or an opened score actually read is ever stored, so the app
// works offline with everything it has used.
//
// Configuration: `library.json` next to the app (`{"zenodoRecord": "…"}` or
// `{"baseUrl": "…/"}`), see docs/LIBRARY.md. `?library=<url>` in the address
// overrides it (development).

/** A range the core needs: file name (from the manifest), start, end (exclusive). */
export interface Need {
  file: string;
  start: number;
  end: number;
}

export interface LibrarySource {
  /** URL of a package file. */
  url(name: string): string;
  /** URL of the manifest. */
  manifestUrl: string;
  /** Page of the dataset (shown in the app). */
  page: string | null;
}

/** Name of the browser cache for the package's ranges. */
// not "klangwelt-…": the service worker deletes those on every update
const CACHE = "klangwelt.library";
/** Ranges closer than this are fetched as one. */
const GAP = 2048;
/** At most this many ranges in one request, and a Range header at most this
 *  long (Zenodo answers 400 ranges in one request; every request counts
 *  against its limit, the number of ranges doesn't). */
const MAX_RANGES = 250;
const MAX_RANGE_HEADER = 6000;
/** Parallel requests. */
const PARALLEL = 4;
const MANIFEST = "klangwelt-manifest.json";

let source: LibrarySource | null = null;
/** The server answered several ranges with the whole file: from now on one range per request. */
let singleOnly = false;

// ---------------------------------------------------------------- request limit
// Zenodo allows guests 60 requests per minute and 2000 per hour (documented;
// it reports 133 per minute in X-RateLimit-Limit), counted per client.
// A melody search takes about 4 requests, a text search 2, opening a score 1.
// A token bucket keeps below the limits: BURST requests at once, then
// PER_MINUTE on average (1,800 an hour). The server's own counter
// (X-RateLimit-Remaining/-Reset, readable through CORS) and a 429 answer
// make it wait for the server's next window – e.g. when several people share
// one address, as in a school network.

const BURST = 20;
const PER_MINUTE = 30;
/** Wait for the next window when the server has this many requests left. */
const RESERVE = 3;
let tokens = BURST;
let refilled = Date.now();
/** No request before this time (ms since 1970): the server's window is used up. */
let blockedUntil = 0;
let waitListener: ((seconds: number) => void) | null = null;

/** Called with the seconds a request has to wait (only for waits of 2 s or more). */
export function onWait(cb: (seconds: number) => void): void {
  waitListener = cb;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Waits until a request may go out, and takes its token. */
async function slot(): Promise<void> {
  for (;;) {
    const now = Date.now();
    tokens = Math.min(BURST, tokens + ((now - refilled) / 60_000) * PER_MINUTE);
    refilled = now;
    const wait = Math.max(blockedUntil - now, tokens >= 1 ? 0 : ((1 - tokens) / PER_MINUTE) * 60_000);
    if (wait <= 0) {
      tokens -= 1;
      return;
    }
    if (wait >= 2000) waitListener?.(Math.ceil(wait / 1000));
    await sleep(wait);
  }
}

/** The server's counter: when its window is almost used up, wait for the next one. */
function noteLimit(res: Response): void {
  const left = res.headers.get("x-ratelimit-remaining");
  const reset = Number(res.headers.get("x-ratelimit-reset")); // seconds since 1970
  if (left !== null && Number(left) <= RESERVE && reset > 0) blockedUntil = Math.max(blockedUntil, reset * 1000);
}

/** A range request within the limits; a 429 answer waits for the server's window and tries again. */
async function request(url: string, range: string): Promise<Response> {
  for (let attempt = 0; ; attempt++) {
    await slot();
    const res = await fetch(url, { headers: { Range: range } });
    noteLimit(res);
    if (res.status !== 429 || attempt >= 3) return res;
    void res.body?.cancel();
    const reset = Number(res.headers.get("x-ratelimit-reset"));
    blockedUntil = Math.max(blockedUntil, reset > 0 ? reset * 1000 : Date.now() + 60_000);
  }
}

export async function configure(): Promise<LibrarySource | null> {
  let cfg: { zenodoRecord?: string; baseUrl?: string } = {};
  const appRoot = new URL("../", import.meta.url).href;
  let relativeTo = appRoot;
  if (libraryParam) {
    cfg = { baseUrl: libraryParam };
    relativeTo = pageUrl ?? appRoot;
  } else {
    try {
      cfg = await (await fetch(new URL("library.json", appRoot))).json();
    } catch {
      cfg = {};
    }
  }
  if (cfg.zenodoRecord) {
    const record = String(cfg.zenodoRecord).trim();
    source = {
      url: (name) => `https://zenodo.org/api/records/${record}/files/${encodeURIComponent(name)}/content`,
      manifestUrl: `https://zenodo.org/api/records/${record}/files/${MANIFEST}/content`,
      page: `https://zenodo.org/records/${record}`,
    };
  } else if (cfg.baseUrl) {
    const base = new URL(cfg.baseUrl.endsWith("/") ? cfg.baseUrl : `${cfg.baseUrl}/`, relativeTo).href;
    source = { url: (name) => new URL(encodeURIComponent(name), base).href, manifestUrl: new URL(MANIFEST, base).href, page: null };
  } else source = null;
  return source;
}

/** The page's address (the worker doesn't see the page's query) – set by the worker at start. */
let pageUrl: string | null = null;
let libraryParam: string | null = null;
export function setPage(href: string): void {
  pageUrl = href;
  libraryParam = new URL(href).searchParams.get("library");
}

async function cache(): Promise<Cache | null> {
  try {
    return await caches.open(CACHE);
  } catch {
    return null;
  }
}

const rangeKey = (url: string, start: number, end: number) => `${url}${url.includes("?") ? "&" : "?"}klangwelt-range=${start}-${end}`;

/** The manifest: from the server, or – offline – the copy from last time. */
export async function manifest(): Promise<{ json: unknown; offline: boolean } | null> {
  if (!source) return null;
  const c = await cache();
  try {
    const res = await fetch(source.manifestUrl, { cache: "no-cache" });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const text = await res.text();
    const json = JSON.parse(text);
    if (c) {
      // Another package at the same address (a new build on a mirror or the
      // development server; Zenodo gives every version new addresses): the
      // cached ranges belong to the old files, so forget them before reading.
      const old = await c.match(source.manifestUrl);
      const before = old ? await old.json().catch(() => null) : null;
      if (before && fingerprint(before) !== fingerprint(json)) {
        for (const k of await c.keys()) await c.delete(k);
      }
      await c.put(source.manifestUrl, new Response(text, { headers: { "content-type": "application/json" } }));
    }
    return { json, offline: false };
  } catch (e) {
    const hit = await c?.match(source.manifestUrl);
    if (hit) return { json: await hit.json(), offline: true };
    throw e;
  }
}

/** Fetches the needed ranges (cache first); returns them as (need, bytes). */
export async function fetchNeeds(needs: Need[]): Promise<Array<[Need, Uint8Array]>> {
  if (!source) throw new Error("no library configured");
  const c = await cache();
  const out: Array<[Need, Uint8Array]> = [];
  const missing: Need[] = [];
  await Promise.all(
    needs.map(async (n) => {
      const hit = await c?.match(rangeKey(source!.url(n.file), n.start, n.end));
      if (hit) out.push([n, new Uint8Array(await hit.arrayBuffer())]);
      else missing.push(n);
    }),
  );
  if (!missing.length) return out;
  // per file: ranges sorted and merged where they are close, in groups per request
  const byFile = new Map<string, Need[]>();
  for (const n of missing) byFile.set(n.file, [...(byFile.get(n.file) ?? []), n]);
  const jobs: Array<{ file: string; spans: Array<[number, number]>; needs: Need[] }> = [];
  for (const [file, list] of byFile) {
    list.sort((a, b) => a.start - b.start);
    const spans: Array<[number, number]> = [];
    for (const n of list) {
      const last = spans[spans.length - 1];
      if (last && n.start <= last[1] + GAP) last[1] = Math.max(last[1], n.end);
      else spans.push([n.start, n.end]);
    }
    for (const group of groups(spans)) {
      jobs.push({ file, spans: group, needs: list.filter((n) => group.some(([a, b]) => n.start >= a && n.end <= b)) });
    }
  }
  const fetched: Array<[Need, Uint8Array]> = [];
  let next = 0;
  const worker = async () => {
    while (next < jobs.length) {
      const job = jobs[next++]!;
      const blocks = await fetchSpans(source!.url(job.file), job.spans);
      for (const n of job.needs) {
        const block = blocks.find(([a, bytes]) => n.start >= a && n.end <= a + bytes.length);
        if (!block) throw new Error(`range ${n.start}-${n.end} of ${n.file} missing in the answer`);
        fetched.push([n, block[1].slice(n.start - block[0], n.end - block[0])]);
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(PARALLEL, jobs.length) }, worker));
  // keep what was read (in the background; a full cache must not break the search)
  if (c) {
    void Promise.all(
      fetched.map(([n, bytes]) =>
        c.put(rangeKey(source!.url(n.file), n.start, n.end), new Response(bytes as unknown as BodyInit, { headers: { "content-type": "application/octet-stream" } })).catch(() => {}),
      ),
    );
  }
  return out.concat(fetched);
}

/** Spans of one file in groups of one request each: at most MAX_RANGES and
 *  a Range header of at most MAX_RANGE_HEADER characters (one span per
 *  request once the server has shown it can't do several). */
export function groups(spans: Array<[number, number]>): Array<Array<[number, number]>> {
  const out: Array<Array<[number, number]>> = [];
  let group: Array<[number, number]> = [];
  let length = 0;
  for (const span of spans) {
    const part = `${span[0]}-${span[1] - 1}`.length + 1;
    if (group.length && (singleOnly || group.length >= MAX_RANGES || length + part > MAX_RANGE_HEADER)) {
      out.push(group);
      group = [];
      length = 0;
    }
    group.push(span);
    length += part;
  }
  if (group.length) out.push(group);
  return out;
}

/** Fetches ranges [start, end) of one URL: one request with several ranges, or one per range. */
async function fetchSpans(url: string, spans: Array<[number, number]>): Promise<Array<[number, Uint8Array]>> {
  if (spans.length > 1 && !singleOnly) {
    const res = await request(url, `bytes=${spans.map(([a, b]) => `${a}-${b - 1}`).join(",")}`);
    if (res.status === 206) {
      const body = new Uint8Array(await res.arrayBuffer());
      const parts = parseMultipart(body);
      if (parts) return parts;
      // one coalesced range (the server merged them): it starts at the first start
      const start = spans[0]![0];
      if (body.length === spans[spans.length - 1]![1] - start) return [[start, body]];
      throw new Error(`unreadable answer to a multi-range request for ${url}`);
    }
    void res.body?.cancel();
    // only the whole file instead of the ranges means the server can't do
    // several: then one range per request (any other answer is an error –
    // switching would multiply the requests, e.g. after a 429)
    if (res.status !== 200) throw new Error(`HTTP ${res.status} for ranges of ${url}`);
    singleOnly = true;
  }
  return Promise.all(
    spans.map(async ([a, b]): Promise<[number, Uint8Array]> => {
      const res = await request(url, `bytes=${a}-${b - 1}`);
      if (res.status !== 206) {
        void res.body?.cancel();
        throw new Error(`HTTP ${res.status} for a range of ${url}`);
      }
      const bytes = new Uint8Array(await res.arrayBuffer());
      if (bytes.length !== b - a) throw new Error(`short range from ${url}`);
      return [a, bytes];
    }),
  );
}

const ascii = new TextDecoder("latin1");

/** Parses a multipart/byteranges body. The boundary is taken from the body's
 *  first line (Zenodo sends it with a wrong Content-Type and LF line ends);
 *  each part's length comes from its Content-Range. */
export function parseMultipart(body: Uint8Array): Array<[number, Uint8Array]> | null {
  let i = 0;
  const line = (): string | null => {
    const nl = body.indexOf(10, i);
    if (nl < 0) return null;
    const s = ascii.decode(body.subarray(i, nl)).replace(/\r$/, "");
    i = nl + 1;
    return s;
  };
  let first = line();
  while (first === "") first = line();
  if (!first || !first.startsWith("--")) return null;
  const boundary = first;
  const out: Array<[number, Uint8Array]> = [];
  for (;;) {
    let range: [number, number] | null = null;
    for (let h = line(); h !== null && h !== ""; h = line()) {
      const m = /^content-range:\s*bytes\s+(\d+)-(\d+)\//i.exec(h);
      if (m) range = [Number(m[1]), Number(m[2])];
    }
    if (!range) return null;
    const len = range[1] - range[0] + 1;
    if (i + len > body.length) return null;
    out.push([range[0], body.slice(i, i + len)]);
    i += len;
    // to the next boundary line
    let next = line();
    while (next === "") next = line();
    if (next === null || next === `${boundary}--`) break;
    if (next !== boundary) return null;
  }
  return out;
}

/** Bytes the package's ranges take in the browser's cache (estimate). */
export async function cachedBytes(): Promise<{ entries: number; bytes: number }> {
  const c = await cache();
  if (!c) return { entries: 0, bytes: 0 };
  const keys = await c.keys();
  let bytes = 0;
  for (const k of keys) {
    const m = /klangwelt-range=(\d+)-(\d+)/.exec(k.url);
    if (m) bytes += Number(m[2]) - Number(m[1]);
  }
  return { entries: keys.length, bytes };
}

/** The package's files by name and checksum: equal for the same package. */
export function fingerprint(manifest: unknown): string {
  const files = (manifest as { files?: Record<string, { name?: string; size?: number; sha256?: string }> } | null)?.files ?? {};
  return JSON.stringify(Object.values(files).map((f) => [f.name, f.size, f.sha256]).sort());
}

/** Empties the cache of the package (the next searches read from the server again). */
export async function clearCache(): Promise<void> {
  try {
    await caches.delete(CACHE);
  } catch {
    /* no cache */
  }
}
