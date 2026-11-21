// Installing the browser version as an app (Android "Install app", desktop
// browsers): the page links a web app manifest – name, start, full-screen
// display, colors, icons 192/512 px plus maskable ones – the icons are real PNGs
// of the stated size, a service worker controls the page, and Chrome's own
// check (Page.getInstallabilityErrors) finds nothing missing.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { startWeb, makeTest } from "./harness.mjs";

let app;
const test = makeTest(() => app);
before(async () => {
  app = await startWeb();
});
after(async () => {
  await app?.close();
});

/** Fetch images in the page: is it a PNG, and how large is it (IHDR)? */
const pngSizes = (urls) =>
  app.eval(async (urls) =>
    Promise.all(urls.map(async (url) => {
      const res = await fetch(url);
      const b = new Uint8Array(await res.arrayBuffer());
      const dv = new DataView(b.buffer);
      const png = b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47;
      return { url, status: res.status, type: res.headers.get("content-type"), png, w: png ? dv.getUint32(16) : 0, h: png ? dv.getUint32(20) : 0 };
    })), urls);

test("manifest: name, start, full-screen display, colors – and icons that are PNGs of their size", async () => {
  const r = await app.send("Page.getAppManifest");
  assert.deepEqual(r.errors, [], "the manifest parses without errors");
  assert.match(r.url, /\/manifest\.webmanifest$/);
  const m = JSON.parse(r.data);
  assert.equal(m.name, "Klangwelt");
  assert.equal(m.short_name, "Klangwelt");
  assert.equal(m.start_url, "./");
  // installed, the app opens in full screen (no status and navigation bar on Android);
  // where that isn't possible (iOS, desktop), browsers fall back to a window of its own
  assert.equal(m.display, "fullscreen");
  assert.equal(m.background_color, "#0d0f14");
  assert.equal(m.theme_color, "#0d0f14");
  const png = m.icons.filter((i) => i.type === "image/png");
  for (const [size, purpose] of [["192x192", "any"], ["512x512", "any"], ["192x192", "maskable"], ["512x512", "maskable"]]) {
    assert.ok(png.some((i) => i.sizes === size && i.purpose === purpose), `icon ${size} ${purpose}`);
  }
  const base = new URL(r.url);
  const got = await pngSizes(png.map((i) => new URL(i.src, base).href));
  png.forEach((icon, k) => {
    const [w, h] = icon.sizes.split("x").map(Number);
    const g = got[k];
    assert.equal(g.status, 200, `${icon.src} loads`);
    assert.ok(g.png, `${icon.src} is a PNG`);
    assert.deepEqual([g.w, g.h], [w, h], `${icon.src} is ${icon.sizes}`);
  });
  // the server says what the manifest is
  const type = await app.eval(async (u) => (await fetch(u)).headers.get("content-type"), r.url);
  assert.match(type, /application\/manifest\+json/);
});

test("Chrome finds the app installable: service worker in control, no installability errors", async () => {
  await app.waitFor(() => !!navigator.serviceWorker?.controller, { timeout: 20000, what: "service worker controls the page" });
  const { installabilityErrors } = await app.send("Page.getInstallabilityErrors");
  assert.deepEqual(installabilityErrors, []);
});

test("page head: favicon and the icon for iPhones load", async () => {
  const links = await app.eval(() => ({
    svg: T.$('link[rel="icon"][type="image/svg+xml"]')?.href,
    png: T.$('link[rel="icon"][type="image/png"]')?.href,
    apple: T.$('link[rel="apple-touch-icon"]')?.href,
  }));
  const svg = await app.eval(async (u) => { const r = await fetch(u); return { status: r.status, type: r.headers.get("content-type"), text: await r.text() }; }, links.svg);
  assert.equal(svg.status, 200);
  assert.match(svg.type, /image\/svg\+xml/);
  assert.match(svg.text, /^<svg /);
  const [png, apple] = await pngSizes([links.png, links.apple]);
  assert.deepEqual([png.png, png.w, png.h], [true, 192, 192]);
  assert.deepEqual([apple.png, apple.w, apple.h], [true, 180, 180], "apple-touch-icon 180 px");
  assert.deepEqual(app.errors, []);
});
