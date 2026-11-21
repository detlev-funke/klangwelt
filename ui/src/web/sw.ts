// Service worker of the browser version: keeps app files and WebAssembly in
// the cache so that Klangwelt starts even without a network (and loads faster).
// `npm run build:web` puts it into the output folder as `sw.js` and inserts
// version and file list. New version = new cache; the old one is deleted on
// activation.

const VERSION = "__KLANGWELT_VERSION__";
const FILES: string[] = JSON.parse('__KLANGWELT_FILES__') as string[];
const CACHE = `klangwelt-${VERSION}`;

interface ExtendableEvent extends Event {
  waitUntil(p: Promise<unknown>): void;
}
interface FetchEvent extends ExtendableEvent {
  readonly request: Request;
  respondWith(r: Promise<Response> | Response): void;
}

const sw = self as unknown as {
  addEventListener(type: "install" | "activate", cb: (e: ExtendableEvent) => void): void;
  addEventListener(type: "fetch", cb: (e: FetchEvent) => void): void;
  skipWaiting(): Promise<void>;
  clients: { claim(): Promise<void> };
  registration: { scope: string };
};

// Every file straight from the server ("reload"): GitHub Pages lets the browser keep
// files for ten minutes, and a new version must not cache an old page next to new code.
sw.addEventListener("install", (e) => {
  e.waitUntil(
    caches
      .open(CACHE)
      .then((c) => c.addAll(FILES.map((f) => new Request(new URL(f, sw.registration.scope).href, { cache: "reload" }))))
      .then(() => sw.skipWaiting()),
  );
});

sw.addEventListener("activate", (e) => {
  e.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith("klangwelt-") && k !== CACHE).map((k) => caches.delete(k))))
      .then(() => sw.clients.claim()),
  );
});

// Cache first, otherwise the network; addresses with parameters (e.g. ?ohne-sw)
// count as the page itself. The changelog's screenshots aren't in FILES: each one goes
// into the cache when it is first loaded.
sw.addEventListener("fetch", (e) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== location.origin) return;
  // Ranges of the score library: the worker caches them itself (web/remote.ts)
  if (req.headers.has("range")) return;
  const onDemand = url.pathname.startsWith(new URL("changelog/images/", sw.registration.scope).pathname);
  e.respondWith(
    caches.open(CACHE).then(async (c) => {
      const hit = (await c.match(req, { ignoreSearch: req.mode === "navigate" })) ?? (req.mode === "navigate" ? await c.match(new URL("index.html", sw.registration.scope).href) : undefined);
      if (hit) return hit;
      const res = await fetch(req);
      if (onDemand && res.ok) await c.put(req, res.clone());
      return res;
    }),
  );
});

// Module (own scope), loaded as an ES module.
export {};
