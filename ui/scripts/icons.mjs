// App icons for installing the browser version (web app manifest, home screen):
// draws the logo as SVG and renders the PNG sizes with a windowless Edge
// (Chrome DevTools Protocol, Node built-ins only). Run it after changing the
// logo; the results in ui/static/icons/ are committed.
//
//   node scripts/icons.mjs        (Edge: KLANGWELT_EDGE or the usual path on Windows)
//
// icon.svg / icon-*.png   the logo as in the toolbar (rounded, transparent corners)
// maskable-*.png          full-bleed background, the sound bars inside the safe
//                         circle – Android cuts it into its own shape
// apple-touch-icon.png    180 px, no transparency (iOS rounds the corners itself)

import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "static", "icons");
const EDGE = process.env.KLANGWELT_EDGE ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";

const GRADIENT = `<linearGradient id="lg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ffb454"/><stop offset="1" stop-color="#6ee7d8"/></linearGradient>`;
const BARS = `<path d="M6 16h2.5M10.5 11v10M14.5 7v18M18.5 10v12M22.5 13v6M25.5 16h.5" stroke="url(#lg)" stroke-width="2.4" stroke-linecap="round" fill="none"/>`;
/** The logo of the toolbar (ui/src/dom.ts LOGO). */
const LOGO = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><defs>${GRADIENT}</defs><rect x="1" y="1" width="30" height="30" rx="9" fill="#171c27" stroke="url(#lg)" stroke-width="1.5"/>${BARS}</svg>`;
/** Full-bleed: the bars at 75 % around the middle stay inside the safe circle (80 % of the size). */
const FULL = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><defs>${GRADIENT}</defs><rect width="32" height="32" fill="#171c27"/><g transform="translate(16 16) scale(.75) translate(-16 -16)">${BARS}</g></svg>`;

const PNGS = [
  { file: "icon-192.png", svg: LOGO, size: 192 },
  { file: "icon-512.png", svg: LOGO, size: 512 },
  { file: "maskable-192.png", svg: FULL, size: 192 },
  { file: "maskable-512.png", svg: FULL, size: 512 },
  { file: "apple-touch-icon.png", svg: FULL, size: 180 },
];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function freePort() {
  return new Promise((res, rej) => {
    const s = createServer();
    s.on("error", rej);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => res(port));
    });
  });
}

async function pageSocket(port) {
  for (let i = 0; i < 100; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      const page = list.find((p) => p.type === "page" && p.webSocketDebuggerUrl);
      if (page) return page.webSocketDebuggerUrl;
    } catch {
      /* not ready yet */
    }
    await sleep(200);
  }
  throw new Error("Edge does not answer");
}

mkdirSync(OUT, { recursive: true });
writeFileSync(join(OUT, "icon.svg"), `${LOGO}\n`);

const port = await freePort();
const profile = mkdtempSync(join(tmpdir(), "klangwelt-icons-"));
const edge = spawn(EDGE, ["--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-first-run", "about:blank"], { stdio: "ignore" });
try {
  const ws = new WebSocket(await pageSocket(port));
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = rej;
  });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id) pending.get(m.id)?.(m);
  };
  const send = (method, params = {}) =>
    new Promise((res, rej) => {
      const i = ++id;
      pending.set(i, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result)));
      ws.send(JSON.stringify({ id: i, method, params }));
    });
  await send("Page.enable");
  await send("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } });
  for (const p of PNGS) {
    await send("Emulation.setDeviceMetricsOverride", { width: p.size, height: p.size, deviceScaleFactor: 1, mobile: false });
    const html = `<!doctype html><html><body style="margin:0;background:transparent">${p.svg.replace("<svg ", `<svg width="${p.size}" height="${p.size}" style="display:block" `)}</body></html>`;
    await send("Page.navigate", { url: `data:text/html;base64,${Buffer.from(html).toString("base64")}` });
    await sleep(300);
    const shot = await send("Page.captureScreenshot", { format: "png", clip: { x: 0, y: 0, width: p.size, height: p.size, scale: 1 } });
    writeFileSync(join(OUT, p.file), Buffer.from(shot.data, "base64"));
    console.log(`${p.file} (${p.size} px)`);
  }
  ws.close();
} finally {
  edge.kill();
  await sleep(500);
  try {
    rmSync(profile, { recursive: true, force: true });
  } catch {
    /* Edge may still hold the folder */
  }
}
console.log(`Icons → ${OUT}`);
