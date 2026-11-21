// A new version of the browser app (ui/src/update.ts, web/host.ts, web/sw.ts). The
// service worker serves the page from its cache, so the first start after an update is
// still the old version – that page had no manifest link once, and installing it as an
// app failed. When the new worker takes over, the page reloads into the new version
// right away if nothing was touched, keeping the open project; otherwise a note with
// "Neu laden" offers it. The test serves a copy of ui/dist-web and publishes "new
// versions" into it: a new worker version and a marker in index.html.
import { after } from "node:test";
import assert from "node:assert/strict";
import { cpSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT, sleep, startWeb, makeTest } from "./harness.mjs";
import { makeTempDir, removeTempDir } from "../tempdirs.mjs";

const dist = makeTempDir("klangwelt-update-");
cpSync(join(ROOT, "ui", "dist-web"), dist, { recursive: true });

/** Stand for version `n` on the server: another worker version, a marker in the page. */
function publish(n) {
  const sw = join(dist, "sw.js");
  const code = readFileSync(sw, "utf8");
  assert.match(code, /const VERSION = "[^"]*"/, "sw.js names its version");
  writeFileSync(sw, code.replace(/const VERSION = "[^"]*"/, `const VERSION = "test-${n}"`));
  const page = join(dist, "index.html");
  const html = readFileSync(page, "utf8").replace(/\s*<meta name="klangwelt-test-version"[^>]*>/, "");
  writeFileSync(page, html.replace("<head>", `<head>\n    <meta name="klangwelt-test-version" content="${n}" />`));
}

let app;
const test = makeTest(() => app);
after(async () => {
  await app?.close();
  await removeTempDir(dist);
});

const version = () => app.eval(() => document.querySelector('meta[name="klangwelt-test-version"]')?.content ?? "start");
const tracks = () => app.eval(() => T.st.snapshot.peek().project.tracks.map((t) => t.name));
const addTrack = (name) => app.eval((n) => T.st.run({ type: "addTrack", track: { id: 0, name: n, kind: "instrument" } }), name);
/** Ask the browser to look for a new worker now (it does so by itself on navigations). */
const checkForUpdate = () => app.eval(async () => void (await (await navigator.serviceWorker.getRegistration())?.update()));
/** A mark on this page: gone after a reload. */
const mark = () => app.eval(() => (window.__samePage = true));
const samePage = () => app.eval(() => window.__samePage === true);
const note = () => app.eval(() => document.querySelector("kw-toast .toast.has-action")?.textContent ?? null);

test("first visit: the page comes from the network – the worker taking over reloads nothing", async () => {
  app = await startWeb({ dist });
  await mark();
  await app.waitFor(() => !!navigator.serviceWorker.controller, { timeout: 20000, what: "service worker active" });
  await sleep(1500);
  assert.equal(await samePage(), true, "no reload");
  assert.equal(await note(), null, "no note");
  assert.equal(await version(), "start");
});

test("a new version while nothing was touched: the page reloads into it and keeps the project", async () => {
  // from now on the page comes from the worker's cache
  await app.reload();
  await addTrack("Before the update");
  assert.deepEqual(await tracks(), ["Before the update"]);
  await mark();

  publish(2);
  await checkForUpdate();
  await app.afterReload();
  assert.equal(await version(), "2", "the new page");
  assert.equal(await samePage(), false);
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.length > 0, { timeout: 10000, what: "project back" });
  assert.deepEqual(await tracks(), ["Before the update"], "the open project came along");
  assert.equal(await note(), null, "no note needed");
});

test("a new version while working: a note with \"Neu laden\" instead – the work isn't interrupted", async () => {
  await addTrack("While working");
  await mark();
  // a key press counts as working (so does a click, a tap or the wheel)
  await app.key("Shift", { code: "ShiftLeft", vk: 16 });

  publish(3);
  await checkForUpdate();
  await app.waitFor(() => !!document.querySelector("kw-toast .toast.has-action"), { timeout: 20000, what: "update note" });
  assert.equal(await note(), "Eine neue Version von Klangwelt ist da.Neu laden");
  await sleep(1000);
  assert.equal(await samePage(), true, "no reload by itself");
  assert.equal(await version(), "2", "still the page it was");

  await app.clickOn("kw-toast .toast-action");
  await app.afterReload();
  assert.equal(await version(), "3");
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.length > 1, { timeout: 10000, what: "project back" });
  assert.deepEqual(await tracks(), ["Before the update", "While working"]);
  assert.deepEqual(app.errors, []);
});
