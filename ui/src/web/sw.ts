// Service Worker der Browser-Version: hält App-Dateien und WebAssembly im
// Cache, damit Klangwelt auch ohne Netz startet (und schneller lädt).
// `npm run build:web` legt ihn als `sw.js` in den Ausgabeordner und setzt
// Version und Dateiliste ein. Neue Version = neuer Cache; der alte wird beim
// Aktivieren gelöscht.

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

sw.addEventListener("install", (e) => {
  e.waitUntil(
    caches
      .open(CACHE)
      .then((c) => c.addAll(FILES.map((f) => new URL(f, sw.registration.scope).href)))
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

// Erst der Cache, sonst das Netz; Adressen mit Parametern (z. B. ?ohne-sw)
// gelten als die Seite selbst.
sw.addEventListener("fetch", (e) => {
  const req = e.request;
  if (req.method !== "GET" || new URL(req.url).origin !== location.origin) return;
  e.respondWith(
    caches.open(CACHE).then(async (c) => {
      const hit = (await c.match(req, { ignoreSearch: req.mode === "navigate" })) ?? (req.mode === "navigate" ? await c.match(new URL("index.html", sw.registration.scope).href) : undefined);
      return hit ?? fetch(req);
    }),
  );
});

// Modul (eigener Gültigkeitsbereich), geladen als ES-Modul.
export {};
