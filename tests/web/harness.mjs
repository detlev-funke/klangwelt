// Testgerüst für die Browser-Version – ohne Fremdpakete, wie tests/e2e/harness.mjs:
// node:test als Läufer; ein statischer Server (ui/scripts/serve.mjs --web)
// liefert ui/dist-web aus, ein Edge ohne Fenster (headless) lädt die Seite
// und wird über das Chrome-DevTools-Protokoll (CDP) bedient.
//
// Jede Testdatei startet ihren eigenen Server und Browser: freie Ports, eigenes
// Browser-Profil im Temp-Ordner (OPFS, Service Worker) und eigener Ordner für
// Downloads. Vorher: `npm run build:web` in ui/.
//
// Browser: KLANGWELT_EDGE oder der übliche Pfad von Edge unter Windows.
// Stumm (--mute-audio); KLANGWELT_TESTS_LAUT=1 macht die Tests hörbar.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
export const OUT = join(ROOT, "tests", "web", "out");
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const EDGE = process.env.KLANGWELT_EDGE ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";

function freePort() {
  return new Promise((res, rej) => {
    const s = createServer();
    s.unref();
    s.on("error", rej);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => res(port));
    });
  });
}

/** Eine CDP-Verbindung (Seite oder Browser). */
class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.handlers = [];
    ws.onmessage = (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id) {
        this.pending.get(m.id)?.(m);
        this.pending.delete(m.id);
      } else for (const h of this.handlers) h(m);
    };
  }

  static async open(url) {
    const ws = new WebSocket(url);
    await new Promise((res, rej) => {
      ws.onopen = res;
      ws.onerror = rej;
    });
    return new Cdp(ws);
  }

  send(method, params = {}, timeout = 60000) {
    return new Promise((res, rej) => {
      const i = ++this.id;
      const timer = setTimeout(() => rej(new Error(`CDP ${method}: keine Antwort`)), timeout);
      this.pending.set(i, (m) => {
        clearTimeout(timer);
        m.error ? rej(new Error(`CDP ${method}: ${m.error.message}`)) : res(m.result);
      });
      this.ws.send(JSON.stringify({ id: i, method, params }));
    });
  }

  close() {
    try {
      this.ws.close();
    } catch {
      /* egal */
    }
  }
}

class WebApp {
  constructor({ server, edge, page, browser, tmp, url, downloads }) {
    Object.assign(this, { server, edge, page, browser, tmp, url, downloads });
    /** Fehler in der Seite (Ausnahmen, console.error) – Tests prüfen, dass keine auftreten. */
    this.errors = [];
    /** Längste Wartezeit für eine Auswertung in der Seite (ms). */
    this.evalTimeout = 60000;
    /** Rückfragen des Browsers (z. B. „Seite verlassen?“ bei ungespeicherten Änderungen) – werden angenommen. */
    this.dialogs = [];
    page.handlers.push((m) => {
      if (m.method === "Page.javascriptDialogOpening") {
        this.dialogs.push(m.params.type);
        void page.send("Page.handleJavaScriptDialog", { accept: true }).catch(() => {});
      }
      if (m.method === "Runtime.exceptionThrown") {
        const d = m.params.exceptionDetails;
        this.errors.push(d.exception?.description ?? d.text);
      } else if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") {
        this.errors.push(m.params.args.map((a) => a.value ?? a.description).join(" "));
      }
    });
  }

  send(method, params, timeout) {
    return this.page.send(method, params, timeout);
  }

  /** In der Seite auswerten (Funktion mit Argumenten oder Ausdruck als Text). Im
   *  Fenster stehen bereit: `T.st` (store.js), `T.backend`, `T.web` (Web-Hülle),
   *  `T.sleep`, `T.waitFor`, `T.$`, `T.$$`. */
  async eval(fn, ...args) {
    const expression = typeof fn === "function" ? `(${fn.toString()})(...${JSON.stringify(args)})` : fn;
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, this.evalTimeout);
    if (r.exceptionDetails) {
      const d = r.exceptionDetails;
      throw new Error(`In der Seite: ${d.exception?.description ?? d.text}`);
    }
    return r.result.value;
  }

  async waitFor(fn, { timeout = 10000, interval = 80, what = "Bedingung" } = {}, ...args) {
    const end = Date.now() + timeout;
    let last;
    while (Date.now() < end) {
      last = await this.eval(fn, ...args).catch((e) => `Fehler: ${e.message}`);
      if (last && !(typeof last === "string" && last.startsWith("Fehler: "))) return last;
      await sleep(interval);
    }
    throw new Error(`Zeitüberschreitung beim Warten auf ${what} (zuletzt: ${JSON.stringify(last)})`);
  }

  async click(x, y) {
    await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await this.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 });
    await this.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount: 1 });
    await sleep(200);
  }

  /** Element (CSS-Selektor, optional mit Text) mit echter Maus anklicken. */
  async clickOn(selector, { text } = {}) {
    const c = await this.eval((sel, t) => {
      const el = [...document.querySelectorAll(sel)].find((e) => t == null || e.textContent.includes(t));
      if (!el) return null;
      el.scrollIntoView({ block: "center" });
      const b = el.getBoundingClientRect();
      return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
    }, selector, text ?? null);
    if (!c) throw new Error(`Element nicht gefunden: ${selector}${text ? ` mit „${text}“` : ""}`);
    await this.click(c.x, c.y);
  }

  async key(key, { code = key, vk = 0, mods = 0 } = {}) {
    const base = { key, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, modifiers: mods };
    await this.send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...base });
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
    await sleep(150);
  }

  async shot(name) {
    mkdirSync(OUT, { recursive: true });
    const r = await this.send("Page.captureScreenshot", { format: "png" });
    const file = join(OUT, `${name.replace(/[^\wäöüÄÖÜß-]+/g, "_").slice(0, 80)}.png`);
    writeFileSync(file, Buffer.from(r.data, "base64"));
    return file;
  }

  /** Seite neu laden und warten, bis die Oberfläche mit dem Kern steht. */
  async reload() {
    await this.send("Page.reload", { ignoreCache: false });
    await sleep(300);
    await ready(this);
  }

  /** The page reloads itself (e.g. when switching the language): wait until it's back. */
  async afterReload() {
    await this.waitFor(() => !window.T, { timeout: 15000, what: "Neuladen" });
    await ready(this);
  }

  /** Server beenden (z. B. um offline zu testen); `startServer` startet ihn neu. */
  stopServer() {
    this.server.kill();
  }

  async startServer() {
    this.server = await serve(this.port, this.serverArgs);
  }

  async close() {
    this.page.close();
    try {
      await this.browser.send("Browser.close", {}, 5000);
    } catch {
      /* schon zu */
    }
    this.browser.close();
    if (this.edge.exitCode == null) this.edge.kill();
    this.server.kill();
    for (let i = 0; i < 10; i++) {
      try {
        rmSync(this.tmp, { recursive: true, force: true });
        break;
      } catch {
        await sleep(300);
      }
    }
  }
}

/** Statischen Server starten (ui/dist-web); `args`: more options of serve.mjs (e.g. --library=…). */
async function serve(port, args = []) {
  const server = spawn(process.execPath, [join(ROOT, "ui", "scripts", "serve.mjs"), "--web", `--port=${port}`, ...args], { stdio: ["ignore", "pipe", "inherit"] });
  await new Promise((res, rej) => {
    const timer = setTimeout(() => rej(new Error("Server startet nicht")), 10000);
    server.stdout.on("data", (d) => {
      if (String(d).includes("http://")) {
        clearTimeout(timer);
        res();
      }
    });
    server.on("exit", (c) => rej(new Error(`Server beendet (${c})`)));
  });
  return server;
}

async function pageTarget(cdpPort, timeout = 20000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json`)).json();
      const page = list.find((p) => p.type === "page" && p.webSocketDebuggerUrl);
      const version = await (await fetch(`http://127.0.0.1:${cdpPort}/json/version`)).json();
      if (page) return { page: page.webSocketDebuggerUrl, browser: version.webSocketDebuggerUrl };
    } catch {
      /* noch nicht bereit */
    }
    await sleep(200);
  }
  throw new Error(`Edge nicht erreichbar (CDP-Port ${cdpPort})`);
}

/** Wartet auf Oberfläche und Kern; stellt `T` in der Seite bereit. */
async function ready(app) {
  await app.waitFor(() => !!document.querySelector("kw-transport") && !!window.__klangweltWeb, { timeout: 30000, what: "Oberfläche" });
  await app.eval(async () => {
    const st = await import("./store.js");
    const { backend } = await import("./bridge.js");
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    window.T = {
      st,
      backend,
      web: window.__klangweltWeb,
      sleep,
      $: (s) => document.querySelector(s),
      $$: (s) => [...document.querySelectorAll(s)],
      async waitFor(fn, timeout = 8000) {
        const end = performance.now() + timeout;
        while (performance.now() < end) {
          const v = await fn();
          if (v) return v;
          await sleep(40);
        }
        throw new Error("waitFor: Zeitüberschreitung");
      },
    };
    await T.waitFor(() => !!st.snapshot.peek(), 30000);
  });
}

/** Start the server and Edge (without a window), load the page. `languages`: the
 *  browser's preferred languages (navigator.languages) – German by default, because the
 *  tests check German texts; the UI detects its language from them (ui/src/i18n.ts).
 *  `library`: folder of a score library package (klang-libbuild), served under
 *  /library/ like Zenodo (ranges, CORS) – see docs/LIBRARY.md. Without it the
 *  app gets no package (never the Zenodo record from ui/static/library.json). */
export async function startWeb({ languages = "de-DE,de", library = null } = {}) {
  if (!existsSync(join(ROOT, "ui", "dist-web", "klang_web.wasm"))) throw new Error("ui/dist-web fehlt – erst „npm run build:web“ in ui/");
  if (!existsSync(EDGE)) throw new Error(`Edge nicht gefunden: ${EDGE} (KLANGWELT_EDGE setzen)`);
  const tmp = mkdtempSync(join(tmpdir(), "klangwelt-web-"));
  const downloads = join(tmp, "downloads");
  mkdirSync(downloads, { recursive: true });
  const [port, cdpPort] = [await freePort(), await freePort()];
  const serverArgs = library ? [`--library=${library}`] : ["--no-library"];
  const server = await serve(port, serverArgs);
  const edge = spawn(
    EDGE,
    [
      "--headless=new",
      `--remote-debugging-port=${cdpPort}`,
      `--user-data-dir=${join(tmp, "profil")}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-extensions",
      "--disable-features=msEdgeSidebarV2,EdgeCollections,msImplicitSignin",
      // Stumm wie die Desktop-Tests (KLANGWELT_STUMM): alles läuft und wird gemessen, zu hören ist nichts.
      // KLANGWELT_TESTS_LAUT=1 macht es hörbar.
      ...(process.env.KLANGWELT_TESTS_LAUT ? [] : ["--mute-audio"]),
      // Ton ohne Geste und ein Ersatz-Mikrofon (Piepton) ohne Rückfrage
      "--autoplay-policy=no-user-gesture-required",
      "--use-fake-ui-for-media-stream",
      "--use-fake-device-for-media-stream",
      "--window-size=1600,1000",
      `--accept-lang=${languages}`,
      "about:blank",
    ],
    { stdio: "ignore" },
  );
  let targets;
  try {
    targets = await pageTarget(cdpPort);
  } catch (e) {
    edge.kill();
    server.kill();
    throw e;
  }
  const page = await Cdp.open(targets.page);
  const browser = await Cdp.open(targets.browser);
  const url = `http://127.0.0.1:${port}/`;
  const app = new WebApp({ server, edge, page, browser, tmp, url, downloads });
  app.port = port;
  app.serverArgs = serverArgs;
  app.cdpPort = cdpPort;
  await browser.send("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: downloads, eventsEnabled: true });
  await app.send("Runtime.enable");
  await app.send("Page.enable");
  await app.send("Emulation.setFocusEmulationEnabled", { enabled: true });
  await app.send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
  await app.send("Page.navigate", { url });
  await ready(app);
  return app;
}

/** Test mit Bildschirmfoto bei Fehlschlag. */
export function makeTest(getApp) {
  return (name, fn, opts = {}) =>
    test(name, opts, async (t) => {
      try {
        await fn(t);
      } catch (e) {
        const app = getApp();
        if (app) {
          try {
            e.message += `\n  Bildschirmfoto: ${await app.shot(name)}`;
          } catch {
            /* ohne Foto */
          }
        }
        throw e;
      }
    });
}
