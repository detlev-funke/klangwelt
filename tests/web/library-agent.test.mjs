// The agent's library tools in the browser: a score is addressed by its doc,
// never by a file path – so the agent can read and import the person's own
// scores (kept in OPFS) and scores of the package on the server (read by range)
// just like on the desktop. Two movements of a test sextet are uploaded as own
// scores; the agent finds them (search_library), reads voices, bars and notes
// (read_library_score), takes all voices over as tracks and puts the second
// movement after the first on the same tracks (import_library_score) – each one
// undo step.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { startWeb, makeTest, ROOT, OUT } from "./harness.mjs";
import { SEXTET, SEXTET_PARTS, expectedExcerpt, scoreXml } from "../library-scores.mjs";

const FIXTURES = join(ROOT, "tests", "web", "fixtures", "library");
const PACKAGE = join(OUT, "library-package");
let app;
const test = makeTest(() => app);

before(async () => {
  // the sample package of the library tests (six traditional tunes)
  execFileSync("cargo", ["run", "--quiet", "--release", "-p", "klang-libbuild", "--", "build", "--sources", join(FIXTURES, "sources.json"), "--out", PACKAGE, "--cache", join(OUT, "library-cache"), "--date", "2026-01-01"], {
    cwd: ROOT,
    stdio: ["ignore", "ignore", "inherit"],
  });
  app = await startWeb({ library: PACKAGE });
});
after(async () => {
  await app?.close();
});

let rpcId = 0;
/** Calls an agent tool through the page's MCP server; errors are thrown. */
async function tool(name, args = {}) {
  const r = await app.eval((m) => T.web.mcp(m), { jsonrpc: "2.0", id: ++rpcId, method: "tools/call", params: { name, arguments: args } });
  if (!r.result) throw new Error(`${name}: ${JSON.stringify(r.error ?? r)}`);
  if (r.result.isError) throw new Error(`${name}: ${r.result.content?.[0]?.text}`);
  return r.result.structuredContent ?? JSON.parse(r.result.content[0].text);
}
const tracks = () => app.eval(() => T.st.snapshot.peek().project.tracks.map((t) => ({ id: t.id, name: t.name, clips: t.clips.map((c) => ({ start: c.startBeats, notes: c.source.notes?.length ?? 0 })) })));

test("own movements uploaded into the library next to the package", async () => {
  await app.waitFor(async () => (await T.backend.libraryInfo()).web?.remote === "ready", { timeout: 20000, what: "package attached" });
  const files = SEXTET.map((m) => ({ name: m.file, xml: scoreXml(m) }));
  const r = await app.eval(async (files) => {
    const paths = await T.backend.importFiles(files.map((f) => new File([f.xml], f.name)));
    return T.backend.libraryAdd(paths);
  }, files);
  assert.equal(r.added, 2, JSON.stringify(r));
  assert.equal((await app.eval(() => T.backend.libraryInfo())).web.ownScores, 2);
});

let docs;
test("agent: search_library finds both movements by title words – marked as own", async () => {
  const tools = (await app.eval((m) => T.web.mcp(m), { jsonrpc: "2.0", id: ++rpcId, method: "tools/list", params: {} })).result.tools;
  for (const t of ["search_library", "read_library_score", "import_library_score"]) assert.ok(tools.some((x) => x.name === t), `tool ${t}`);
  assert.match(tools.find((t) => t.name === "analyze_score").description, /doc/);
  const r = await tool("search_library", { text: "testsextett satz" });
  assert.equal(r.hits.length, 2, JSON.stringify(r));
  assert.ok(r.hits.every((h) => h.origin === "own" && h.composer === "Pjotr Test"), JSON.stringify(r.hits));
  docs = SEXTET.map((m) => r.hits.find((h) => h.title === m.title)?.doc);
  assert.ok(docs.every((d) => Number.isInteger(d)), JSON.stringify(r.hits));
  assert.deepEqual(r.hits.map((h) => h.file).sort(), SEXTET.map((m) => m.file).sort());
});

test("agent: read_library_score lists voices and bars, and the notes of a bar range", async () => {
  const r = await tool("read_library_score", { doc: docs[0] });
  assert.deepEqual(r.voices.map((v) => v.name), SEXTET_PARTS.map(([n]) => n));
  assert.deepEqual([r.bars, r.pickup, r.timeSignature, r.tempo], [4, false, "3/4", 132]);
  assert.ok(r.voices.every((v) => v.notes === 12 && v.bars[0] === 1 && v.bars[1] === 4), JSON.stringify(r.voices));
  assert.equal(r.excerpt, undefined, "no notes without bars");
  const ex = await tool("read_library_score", { doc: docs[0], fromBar: 2, toBar: 3, voices: ["violin 1", 6] });
  assert.deepEqual(ex.excerpt, { fromBar: 2, toBar: 3, startQuarter: 3, endQuarter: 9 });
  assert.equal(ex.voices[0].excerpt, expectedExcerpt(SEXTET[0], 0, 2, 3));
  assert.equal(ex.voices[5].excerpt, expectedExcerpt(SEXTET[0], 5, 2, 3));
  assert.equal(ex.voices[1].excerpt, undefined, "only the chosen voices");
  await assert.rejects(tool("read_library_score", { doc: docs[0], voices: ["Kontrabass"] }), /the voices are: 1 Violin 1, 2 Violin 2/);
});

test("agent: import_library_score takes all voices over as tracks – one undo step", async () => {
  const r = await tool("import_library_score", { doc: docs[0] });
  assert.equal(r.tracks.length, 6, JSON.stringify(r));
  assert.deepEqual([r.startBeats, r.endBeats], [0, 12]);
  assert.equal(r.created.tracks.length, 6);
  // the interface shows them at once
  const shown = await app.waitFor(() => {
    const ts = T.st.snapshot.peek().project.tracks;
    return ts.length === 6 ? ts.map((t) => t.name) : null;
  }, { what: "six tracks" });
  assert.deepEqual(shown, SEXTET_PARTS.map(([n]) => `${SEXTET[0].title} – ${n}`));
  assert.equal(await app.eval(() => T.st.snapshot.peek().undoLabel), "6 Stimmen aus Bibliothek übernehmen");
});

test("agent: the second movement goes after the first on the same tracks", async () => {
  const before = await tracks();
  const r = await tool("import_library_score", { doc: docs[1], trackIds: before.map((t) => t.id), startBeats: 12 });
  assert.ok(r.tracks.every((t) => !t.newTrack), JSON.stringify(r));
  assert.equal(r.endBeats, 24);
  const after = await app.waitFor(async () => {
    const ts = T.st.snapshot.peek().project.tracks;
    return ts.every((t) => t.clips.length === 2) ? ts.map((t) => t.clips.map((c) => [c.startBeats, c.source.notes.length])) : null;
  }, { what: "second clip on every track" });
  assert.equal(after.length, 6, "no new tracks");
  assert.ok(after.every((c) => JSON.stringify(c) === JSON.stringify([[0, 12], [12, 12]])), JSON.stringify(after));
  // undo: first the second movement, then the first
  await tool("undo");
  assert.ok((await tracks()).every((t) => t.clips.length === 1));
  await tool("undo");
  assert.equal((await tracks()).length, 0);
});

test("agent: a score of the package on the server is read by range and imported too", async () => {
  const r = await tool("search_library", { text: "jakob" });
  assert.equal(r.hits[0].title, "Bruder Jakob");
  assert.equal(r.hits[0].origin, "remote");
  const read = await tool("read_library_score", { doc: r.hits[0].doc, fromBar: 1, toBar: 2 });
  assert.equal(read.origin, "remote");
  assert.ok(read.voices.length >= 1 && read.voices[0].excerpt.length > 0, JSON.stringify(read));
  const imp = await tool("import_library_score", { doc: r.hits[0].doc, voices: [1], fromBar: 1, toBar: 2 });
  assert.equal(imp.tracks.length, 1);
  assert.match(imp.tracks[0].track, /^Bruder Jakob – .* \(T\. 1–2\)$/);
  const analysis = await tool("analyze_score", { doc: r.hits[0].doc });
  assert.equal(analysis.titel, "Bruder Jakob");
  await tool("undo");
  assert.equal((await tracks()).length, 0);
  assert.deepEqual(app.errors, []);
});
