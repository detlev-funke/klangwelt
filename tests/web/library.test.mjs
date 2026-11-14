// Score library in the browser: a package (built here with klang-libbuild from
// tests/web/fixtures/library) is served like Zenodo – by HTTP range, with
// CORS – and read in ranges; own scores are added by upload, indexed in the
// core and kept in OPFS. Checked: text and melody search find a piece in the
// package and one of the own scores (mixed hits with source and license), a
// score opens from the package's ZIP, the tree covers both, and what was read
// once works offline.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { startWeb, makeTest, sleep, ROOT, OUT } from "./harness.mjs";

const FIXTURES = join(ROOT, "tests", "web", "fixtures", "library");
const PACKAGE = join(OUT, "library-package");
let app;
const test = makeTest(() => app);

before(async () => {
  // the sample package: six traditional tunes
  execFileSync("cargo", ["run", "--quiet", "--release", "-p", "klang-libbuild", "--", "build", "--sources", join(FIXTURES, "sources.json"), "--out", PACKAGE, "--cache", join(OUT, "library-cache"), "--date", "2026-01-01"], {
    cwd: ROOT,
    stdio: ["ignore", "ignore", "inherit"],
  });
  app = await startWeb({ library: PACKAGE });
});
after(async () => {
  await app?.close();
});

const info = () => app.eval(() => T.backend.libraryInfo());
const search = (q) => app.eval((q) => T.backend.librarySearch(q), q);
/** Melody of "Ode an die Freude" and of the own tune (MIDI numbers – independent of note spelling). */
const ODE = "64 64 65 67 67 65 64 62 60 60 62 64";
const OWN = "74 71 67 71 69 66 67 69 71 72";

test("the package from the server is attached, nothing but its header is read", async () => {
  const i = await app.waitFor(async () => {
    const i = await T.backend.libraryInfo();
    return i.web?.remote === "ready" ? i : null;
  }, { timeout: 20000, what: "package attached" });
  assert.equal(i.web.remoteTitle, "Klangwelt test library");
  assert.equal(i.web.remoteScores, 6);
  assert.equal(i.scores, 6);
  assert.match(i.web.remoteSources[0], /CC0/);
  const manifest = JSON.parse(readFileSync(join(PACKAGE, "klangwelt-manifest.json"), "utf8"));
  assert.equal(manifest.scores, 6);
  assert.deepEqual(app.errors, []);
});

test("text and melody search find a piece in the package (source and license with every hit)", async () => {
  const text = await search({ text: "jakob" });
  assert.equal(text.length, 1);
  assert.equal(text[0].title, "Bruder Jakob");
  assert.equal(text[0].origin, "remote");
  assert.equal(text[0].collection, "Klangwelt test scores");
  assert.equal(text[0].license, "CC0-1.0");
  const mel = await search({ melody: ODE });
  assert.equal(mel[0].title, "Ode an die Freude");
  assert.ok(mel[0].score > 0.95, `score ${mel[0].score}`);
  assert.equal(mel[0].origin, "remote");
});

test("own scores: upload, indexed in the core, found together with the package", async () => {
  const xml = readFileSync(join(FIXTURES, "own", "mein-testlied.musicxml"), "utf8");
  const r = await app.eval(async (xml) => {
    const paths = await T.backend.importFiles([new File([xml], "mein-testlied.musicxml")]);
    return T.backend.libraryAdd(paths);
  }, xml);
  assert.equal(r.added, 1, JSON.stringify(r));
  assert.equal((await info()).web.ownScores, 1);
  // melody: the own tune first, marked as own
  const mel = await search({ melody: OWN });
  assert.equal(mel[0].title, "Mein Testlied");
  assert.equal(mel[0].origin, "own");
  // text: own and package hits mixed
  const text = await search({ text: "lied" });
  assert.ok(text.some((h) => h.origin === "own" && h.title === "Mein Testlied"), JSON.stringify(text));
  const both = await search({ text: "o" });
  assert.ok(both.some((h) => h.origin === "own") && both.some((h) => h.origin === "remote"), "mixed hits");
  // tree: both parts, own scores listed first
  const top = await app.eval(() => T.backend.libraryTree(["genre"], [], 0, 50));
  assert.equal(top.matching, 7);
  const own = await app.eval(() => T.backend.libraryOwnList());
  assert.equal(own.length, 1);
  assert.equal(own[0].file, "mein-testlied.musicxml");
});

test("the library window shows mixed hits with their source; a score opens from the package's ZIP", async () => {
  await app.eval(async (melody) => {
    T.st.showPanel("bibliothek");
    await T.sleep(300);
    const inputs = T.$$(".lib-search input");
    inputs[1].value = melody;
    inputs[1].dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
  }, ODE);
  await app.waitFor(() => T.$(".lib-hit .src")?.textContent.includes("CC0-1.0"), { what: "hit with source" });
  await app.clickOn(".lib-hit", { text: "Ode an die Freude" });
  await app.waitFor(() => T.$(".lib-title")?.textContent.includes("Ode an die Freude"), { timeout: 15000, what: "score open" });
  const detail = await app.eval(() => T.$(".lib-source")?.textContent);
  assert.match(detail, /Klangwelt test scores · CC0-1.0/);
  const view = await app.eval(async () => {
    const hits = await T.backend.librarySearch({ text: "kuckuck" });
    return T.backend.libraryOpen(hits[0].doc, null, null);
  });
  assert.match(view.title, /Kuckuck/);
  assert.equal(view.origin, "remote");
  assert.ok(view.voices[0].notes.length > 10);
  // own score opens from OPFS
  const own = await app.eval(async () => {
    const hits = await T.backend.librarySearch({ text: "testlied" });
    return T.backend.libraryOpen(hits[0].doc, null, null);
  });
  assert.equal(own.title, "Mein Testlied");
  assert.equal(own.origin, "own");
  await app.shot("bibliothek-web");
  assert.deepEqual(app.errors, []);
});

test("only what was used is cached; offline it still works", async () => {
  const cached = await app.eval(() => T.backend.libraryCacheInfo());
  const size = readFileSync(join(PACKAGE, "klangwelt-index.kwix")).length + readFileSync(join(PACKAGE, "klangwelt-scores.zip")).length;
  assert.ok(cached.entries > 0 && cached.bytes > 0, JSON.stringify(cached));
  assert.ok(cached.bytes < size, `cached ${cached.bytes} of ${size} bytes`);
  await app.waitFor(() => !!navigator.serviceWorker.controller, { timeout: 15000, what: "service worker" });
  await app.eval(() => T.web.sync());
  app.stopServer();
  await sleep(300);
  await app.reload();
  // the package attaches from the cache (manifest, header) – searches that ran before run again
  await app.waitFor(async () => (await T.backend.libraryInfo()).web?.remote === "ready", { timeout: 20000, what: "package from the cache" });
  const mel = await search({ melody: ODE });
  assert.equal(mel[0].title, "Ode an die Freude");
  const text = await search({ text: "jakob" });
  assert.equal(text[0].title, "Bruder Jakob");
  const view = await app.eval(async (doc) => T.backend.libraryOpen(doc, null, null), mel[0].doc);
  assert.equal(view.title, "Ode an die Freude");
  // own scores are kept in OPFS
  const own = await search({ melody: OWN });
  assert.equal(own[0].title, "Mein Testlied");
  // what was never read isn't there: a clear message, no crash
  const err = await app.eval(async () => {
    try {
      const hits = await T.backend.librarySearch({ melody: "60 67 60 67 60 67 60 67 72" });
      return hits.length ? "hits" : "empty";
    } catch (e) {
      return String(e);
    }
  });
  // (nothing cached for it: an error that says so – or no hits, if all it needed was cached)
  assert.ok(err === "empty" || /offline/i.test(err), err);
  await app.startServer();
  app.errors = app.errors.filter((e) => !/Failed to fetch|ERR_CONNECTION_REFUSED|net::/.test(e));
  assert.deepEqual(app.errors, []);
});

test("removing an own score", async () => {
  const own = await app.eval(() => T.backend.libraryOwnList());
  const rest = await app.eval((p) => T.backend.libraryOwnRemove(p), own[0].path);
  assert.equal(rest.length, 0);
  assert.equal((await info()).web.ownScores, 0);
  const hits = await search({ text: "testlied" });
  assert.equal(hits.length, 0);
});
