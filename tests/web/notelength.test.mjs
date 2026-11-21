// Note lengths in the browser version: the length of new notes chosen next to the piano
// (Auto, 1/1 … 1/32, dotted, triplet; digit keys while typing), the piano appending notes
// at the end of the piece, and the menu of selected notes – "Length" and "Longer / shorter"
// set the lengths, quantizing tells when there is nothing to change. On a phone (touch
// emulation) a box drawn with a finger stays selected while holding an empty spot, the menu's
// options are finger-sized and work by tap, and the length is one button with a menu.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { startWeb, makeTest, sleep } from "./harness.mjs";

let app;
const test = makeTest(() => app);
before(async () => {
  app = await startWeb();
});
after(async () => {
  await app?.close();
});

const MOD = { alt: 1, ctrl: 2, shift: 8 };
let trackId;

/** Emulate a viewport; `touch`: a phone (coarse pointer, touch events). */
async function viewport(width, height, { touch = false } = {}) {
  await app.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: touch ? 2 : 1, mobile: touch });
  await app.send("Emulation.setTouchEmulationEnabled", { enabled: touch, maxTouchPoints: touch ? 5 : 1 });
  await sleep(300);
}

/**
 * Empty project with track "Test" in the piano roll: one clip from `clipStart`, one bar long,
 * with `notes`; `ppb` pixels per beat, visible pitches 55–75. The length of new notes is Auto.
 */
async function setup({ clipStart = 4, notes = [{ pitch: 72, startBeats: 3, lengthBeats: 0.5, velocity: 100 }], ppb = 80 } = {}) {
  trackId = await app.eval(async (clipStart, notes, ppb) => {
    await T.st.act(T.backend.projectNew());
    await T.backend.transport("stop");
    await T.backend.transport("rewind");
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Test", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    if (t.view !== "notes") await T.st.run({ type: "setTrack", trackId: t.id, patch: { view: "notes" } });
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Noten", startBeats: clipStart, source: { type: "notes", lengthBeats: 4, notes } } });
    const pr = await import("./pianoroll.js");
    pr.pitchViews.update((m) => ({ ...m, [t.id]: { low: 55, span: 21 } }));
    pr.noteSel.value = null;
    (await import("./noteinput.js")).noteValue.value = { d: 0, mod: "" };
    T.st.viewport.value = { pxPerBeat: ppb, scrollX: 0, scrollY: 0 };
    T.st.selection.value = null;
    T.st.showPanel("spuren");
    await T.sleep(300);
    return t.id;
  }, clipStart, notes, ppb);
}

/** Screen geometry of the piano roll (window pixels). */
async function geo(id = trackId) {
  return app.eval(async (id) => {
    const pr = await import("./pianoroll.js");
    const p = T.st.snapshot.peek().project;
    const i = p.tracks.findIndex((t) => t.id === id);
    const g = pr.geoFor(p.tracks[i]);
    const v = T.st.viewport.peek();
    const c = document.querySelector("kw-timeline canvas").getBoundingClientRect();
    const lanes = {};
    for (let m = 0; m < 128; m++) if (pr.inView(g.view, m)) lanes[m] = pr.laneY(g, m);
    const keys = document.querySelector(`.track-row[data-track-id="${id}"] canvas.keys`)?.getBoundingClientRect();
    return {
      cx: c.x, cy: c.y, rowY: c.y + T.st.RULER_H + pr.rowTops(p.tracks)[i] - v.scrollY, laneH: g.laneH, lanes, ppb: v.pxPerBeat, scrollX: v.scrollX,
      notesTop: pr.NOTES_TOP, keys: keys ? { x: keys.x, y: keys.y } : null,
    };
  }, id);
}
const X = (g, beat) => g.cx + beat * g.ppb - g.scrollX;
const Y = (g, pitch) => {
  if (g.lanes[pitch] == null) throw new Error(`pitch ${pitch} not visible`);
  return g.rowY + g.lanes[pitch] + g.laneH / 2;
};
/** Window point of a piano key. */
const KEY = (g, pitch) => [g.keys.x + 8, g.keys.y + (g.lanes[pitch] - g.notesTop) + g.laneH / 2];

/** The notes of the track: "pitch@start/length" (project beats), sorted. */
const notes = (id = trackId) => app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips
  .flatMap((c) => c.source.notes.map((n) => `${n.pitch}@${+(c.startBeats + n.startBeats).toFixed(4)}/${+n.lengthBeats.toFixed(4)}`)).sort(), id);
const idle = () => app.eval(async () => { await (await import("./noteinput.js")).entryIdle(); await T.sleep(150); });
const value = () => app.eval(async () => (await import("./noteinput.js")).valueLabel());
const sel = () => app.eval(async () => ((await import("./pianoroll.js")).noteSel.peek()?.keys ?? []).slice().sort());
const undoLabel = () => app.eval(() => T.st.snapshot.peek().undoLabel);
const toast = () => app.eval(() => document.querySelector("kw-toast")?.textContent ?? "");
const select = (keys, id = trackId) => app.eval(async (id, keys) => {
  const t = T.st.snapshot.peek().project.tracks.find((t) => t.id === id);
  (await import("./pianoroll.js")).noteSel.value = { trackId: id, clipId: t.clips[0].id, keys };
  T.st.selection.value = { trackId: id, clipId: t.clips[0].id };
}, id, keys);
const playheadBeat = () => app.eval(() => (T.st.status.peek().positionSecs * T.st.snapshot.peek().project.tempoBpm) / 60);

async function dblclick(x, y) {
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  for (const clickCount of [1, 2]) {
    await app.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount });
    await app.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount });
  }
  await sleep(250);
}
async function click(x, y, modifiers = 0) {
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, modifiers });
  await app.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1, modifiers });
  await app.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount: 1, modifiers });
  await sleep(200);
}
async function rightClick(x, y) {
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await app.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "right", buttons: 2, clickCount: 1 });
  await app.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "right", buttons: 0, clickCount: 1 });
  await app.waitFor(() => !!document.querySelector(".ctx-menu"), { what: "context menu" });
}
/** A key by its position (KeyboardEvent.code). */
const press = (key, code, vk, mods = 0) => app.key(key, { code, vk, mods });
const digit = (n) => press(String(n), `Digit${n}`, 48 + n);
const letter = (l, mods = 0) => press(l, `Key${l.toUpperCase()}`, l.toUpperCase().charCodeAt(0), mods);

/** Center of an element (window pixels); null if it isn't shown. */
const center = (selector) => app.eval((s) => {
  const el = [...document.querySelectorAll(s)].find((e) => e.getClientRects().length);
  if (!el) return null;
  const b = el.getBoundingClientRect();
  return [b.x + b.width / 2, b.y + b.height / 2];
}, selector);
async function clickOn(selector) {
  const c = await center(selector);
  if (!c) throw new Error(`not shown: ${selector}`);
  await click(...c);
}

// ---------------------------------------------------------------- touch (CDP touch events)

const touch = (type, points) =>
  app.send("Input.dispatchTouchEvent", { type, touchPoints: points.map(([x, y], id) => ({ x, y, id, radiusX: 3, radiusY: 3, force: 1 })) });
async function tap(x, y) {
  await touch("touchStart", [[x, y]]);
  await touch("touchEnd", []);
  await sleep(150);
}
async function tapOn(selector) {
  const c = await center(selector);
  if (!c) throw new Error(`not shown: ${selector}`);
  await tap(...c);
}
async function hold(x, y) {
  await touch("touchStart", [[x, y]]);
  await sleep(750);
  await touch("touchEnd", []);
  await app.waitFor(() => !!document.querySelector(".ctx-menu"), { what: "menu from holding" });
}
async function drag([x0, y0], [x1, y1], steps = 8) {
  await touch("touchStart", [[x0, y0]]);
  for (let i = 1; i <= steps; i++) {
    await touch("touchMove", [[x0 + ((x1 - x0) * i) / steps, y0 + ((y1 - y0) * i) / steps]]);
    await sleep(16);
  }
  await touch("touchEnd", []);
  await sleep(200);
}

// ---------------------------------------------------------------- length of new notes

test("next to the piano: the length of new notes – double-click and Alt+click take it, dotted and triplet modify it, Auto is as before", async () => {
  await viewport(1400, 900);
  await setup();
  const row = `.track-row[data-track-id="${trackId}"]`;
  assert.deepEqual(await app.eval((row) => [...document.querySelectorAll(`${row} .note-values button`)].map((b) => b.textContent), row),
    ["Auto", "1/1", "1/2", "1/4", "1/8", "1/16", "1/32", "·", "3"]);
  assert.equal(await app.eval((row) => document.querySelector(`${row} .note-values .on`)?.textContent, row), "Auto");
  assert.equal(await app.eval((row) => document.querySelector(`${row} .note-values [data-mod="dot"]`).disabled, row), true, "dotted needs a note value");
  // the row of values fits next to the piano in one line
  const fit = await app.eval((row) => {
    const tops = new Set([...document.querySelectorAll(`${row} .note-values button`)].map((b) => Math.round(b.getBoundingClientRect().top)));
    const box = document.querySelector(`${row} .note-values`).getBoundingClientRect();
    return { lines: tops.size, right: box.right, keys: document.querySelector(`${row} canvas.keys`).getBoundingClientRect().left };
  }, row);
  assert.equal(fit.lines, 1, "one line");
  assert.ok(fit.right <= fit.keys, `left of the piano: ${JSON.stringify(fit)}`);

  let g = await geo();
  // Auto: a double-click takes the last length (one beat at first)
  await dblclick(X(g, 4.5), Y(g, 60));
  await idle();
  assert.ok((await notes()).includes("60@4.5/1"), "Auto: one beat");
  // 1/8: a double-click makes an eighth (half a beat in 4/4)
  await clickOn(`${row} .note-values [data-value="8"]`);
  assert.equal(await value(), "1/8");
  await dblclick(X(g, 5.5), Y(g, 62));
  await idle();
  assert.ok((await notes()).includes("62@5.5/0.5"), `1/8: ${await notes()}`);
  // dotted: Alt+click (without dragging) – a dotted eighth
  await clickOn(`${row} .note-values [data-mod="dot"]`);
  assert.equal(await value(), "1/8.");
  g = await geo();
  await click(X(g, 6.1), Y(g, 64), MOD.alt);
  await idle();
  assert.ok((await notes()).includes("64@6/0.75"), `dotted: ${await notes()}`);
  // triplet replaces dotted: a third of a beat for an eighth triplet
  await clickOn(`${row} .note-values [data-mod="tri"]`);
  assert.equal(await value(), "1/8³");
  assert.deepEqual(await app.eval((row) => [...document.querySelectorAll(`${row} .note-values .on`)].map((b) => b.textContent), row), ["1/8", "3"]);
  await dblclick(X(g, 6.5), Y(g, 66));
  await idle();
  assert.ok((await notes()).some((n) => /^66@6\.5\/0\.333/.test(n)), `triplet: ${await notes()}`);
  // Auto again: dotted and triplet are switched off with it
  await clickOn(`${row} .note-values [data-value="0"]`);
  assert.equal(await value(), "Auto");
  assert.equal(await app.eval((row) => document.querySelector(`${row} .note-values [data-mod="tri"]`).disabled, row), true);
});

test("keyboard entry: digit keys 1–6 choose the length, \".\" dotted, \",\" triplet, 0 Auto – the mark moves on by the note's length", async () => {
  await viewport(1400, 900);
  await setup();
  await app.eval(() => T.st.act(T.backend.setLoopRange([4, 8])));
  await clickOn(`.track-row[data-track-id="${trackId}"] .entry-switch`);
  await app.waitFor(async () => !!(await import("./noteinput.js")).entry.peek(), { what: "keyboard entry on" });
  const entryBeat = () => app.eval(async () => (await import("./noteinput.js")).entry.peek().beat);
  await digit(4); // eighth
  assert.equal(await value(), "1/8");
  assert.match(await app.eval(() => document.querySelector("kw-timeline .keys-hint").textContent), /1\/8/, "the hint line shows the length");
  await letter("a"); // C4
  await idle();
  assert.equal(await entryBeat(), 4.5);
  await press(".", "Period", 190); // dotted
  assert.equal(await value(), "1/8.");
  await letter("s"); // D4
  await idle();
  assert.equal(await entryBeat(), 5.25);
  await press(",", "Comma", 188); // triplet
  assert.equal(await value(), "1/8³");
  await digit(3); // quarter, the triplet stays
  assert.equal(await value(), "1/4³");
  await letter("d"); // E4
  await idle();
  assert.ok(Math.abs((await entryBeat()) - (5.25 + 2 / 3)) < 1e-6, `after a quarter triplet: ${await entryBeat()}`);
  await digit(0); // Auto: one grid step (¼ beat at this zoom)
  assert.equal(await value(), "Auto");
  await letter("f"); // F4
  await idle();
  const n = await notes();
  assert.ok(n.includes("60@4/0.5") && n.includes("62@4.5/0.75"), JSON.stringify(n));
  assert.ok(n.some((x) => /^64@5\.25\/0\.666/.test(x)), JSON.stringify(n));
  assert.ok(n.some((x) => /^65@5\.916\d*\/0\.25$/.test(x)), JSON.stringify(n));
  await app.key("Escape", { code: "Escape", vk: 27 });
  await app.eval(() => T.st.act(T.backend.setLoopRange(null)));
});

test("the piano: at the end of the piece a key appends its note and the playhead moves behind it (Shift: chord); elsewhere it only sounds", async () => {
  await viewport(1400, 900);
  await setup();
  const g = await geo();
  // the piece: 72 over beat 7–7.5 – it ends at 7.5
  await clickOn(`.track-row[data-track-id="${trackId}"] .note-values [data-value="4"]`);
  // playhead at the start: a key only sounds
  await click(...KEY(g, 60));
  await idle();
  assert.deepEqual(await notes(), ["72@7/0.5"], "not at the end: nothing added");
  // playhead at the end of the piece
  await app.eval(async () => (await import("./actions.js")).seekBeats(7.5));
  await app.waitFor(async () => (await import("./noteinput.js")).atPieceEnd(), { what: "playhead at the end" });
  await click(...KEY(g, 60));
  await idle();
  await click(...KEY(g, 62));
  await click(...KEY(g, 65), MOD.shift); // chord with D
  await click(...KEY(g, 64));
  await idle();
  assert.deepEqual(await notes(), ["60@7.5/1", "62@8.5/1", "64@9.5/1", "65@8.5/1", "72@7/0.5"]);
  assert.ok(Math.abs((await playheadBeat()) - 10.5) < 1e-3, `playhead behind the last note: ${await playheadBeat()}`);
  const clip = await app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips.map((c) => [c.startBeats, c.source.lengthBeats]), trackId);
  assert.deepEqual(clip, [[4, 8]], "the clip grew by a bar");
  assert.match(await undoLabel(), /E4/, "every note its own undo step");
  // the playhead elsewhere: only sound again
  await app.eval(async () => (await import("./actions.js")).seekBeats(2));
  await sleep(200);
  await click(...KEY(g, 67));
  await idle();
  assert.equal((await notes()).length, 5, "not at the end: nothing added");
});

// ---------------------------------------------------------------- menu of selected notes

test("menu of selected notes: \"Length\" sets 1/1 … 1/16, \"Longer / shorter\" multiplies – starts stay, one undo step, the clip grows", async () => {
  await viewport(1400, 900);
  await setup({ notes: [
    { pitch: 60, startBeats: 0, lengthBeats: 1, velocity: 100 },
    { pitch: 64, startBeats: 1, lengthBeats: 0.25, velocity: 100 },
    { pitch: 67, startBeats: 3, lengthBeats: 0.5, velocity: 100 },
  ] });
  const g = await geo();
  await select(["60@0", "64@1"]);
  await rightClick(X(g, 4.5), Y(g, 60));
  const rows = await app.eval(() => [...document.querySelectorAll(".ctx-menu .ctx-row")].map((r) => [r.dataset.act, [...r.querySelectorAll(".ctx-chip")].map((c) => c.textContent)]));
  assert.deepEqual(rows.find(([a]) => a === "laenge")?.[1], ["1/1", "1/2", "1/4", "1/8", "1/16"]);
  assert.deepEqual(rows.find(([a]) => a === "laenge-mal")?.[1], ["½×", "1½×", "2×"]);
  await clickOn('.ctx-menu [data-act="laenge-8"]');
  await app.waitFor(async (id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.filter((n) => n.lengthBeats === 0.5).length === 3, { what: "eighths" }, trackId);
  assert.deepEqual(await notes(), ["60@4/0.5", "64@5/0.5", "67@7/0.5"], "only the selected notes, starts unchanged");
  assert.deepEqual(await sel(), ["60@0", "64@1"], "the selection stays");
  assert.match(await toast(), /2 Noten jetzt 1\/8 lang/);
  assert.match(await undoLabel(), /1\/8/);
  // twice as long
  await rightClick(X(g, 4.2), Y(g, 60));
  await clickOn('.ctx-menu [data-act="laenge-mal-2"]');
  await app.waitFor(async (id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes[0].lengthBeats === 1, { what: "twice as long" }, trackId);
  assert.deepEqual(await notes(), ["60@4/1", "64@5/1", "67@7/0.5"]);
  // undo: one step back
  await app.eval(async () => { await (await import("./actions.js")).undo(); });
  assert.deepEqual(await notes(), ["60@4/0.5", "64@5/0.5", "67@7/0.5"]);
  // a whole note at the end of the clip: the clip grows by a bar
  await select(["67@3"]);
  await rightClick(X(g, 7.2), Y(g, 67));
  await clickOn('.ctx-menu [data-act="laenge-1"]');
  await app.waitFor(async (id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.lengthBeats === 8, { what: "clip grown" }, trackId);
  assert.ok((await notes()).includes("67@7/4"));
});

test("quantize: notes already on the grid – a message, no undo step; off the grid they move as before", async () => {
  await viewport(1400, 900);
  await setup({ notes: [
    { pitch: 60, startBeats: 0, lengthBeats: 1, velocity: 100 },
    { pitch: 62, startBeats: 1.1, lengthBeats: 0.3, velocity: 100 },
  ] });
  const g = await geo();
  await select(["60@0"]);
  const before = await undoLabel();
  await rightClick(X(g, 4.5), Y(g, 60));
  await clickOn('.ctx-menu [data-act="raster-4"]');
  await app.waitFor(() => /nichts zu ändern/.test(document.querySelector("kw-toast")?.textContent ?? ""), { what: "message: nothing to change" });
  assert.match(await toast(), /beginnt schon auf dem Raster 1\/4/);
  assert.equal(await undoLabel(), before, "no undo step");
  await rightClick(X(g, 4.5), Y(g, 60));
  await clickOn('.ctx-menu [data-act="raster-4-laenge"]');
  await app.waitFor(() => /Anfang und Ende schon auf dem Raster/.test(document.querySelector("kw-toast")?.textContent ?? ""), { what: "message: start and end on the grid" });
  assert.deepEqual(await sel(), ["60@0"], "the selection stays");
  // off the grid: it moves (start and length on 1/4)
  await select(["62@1.1"]);
  await rightClick(X(g, 5.2), Y(g, 62));
  await clickOn('.ctx-menu [data-act="raster-4-laenge"]');
  await app.waitFor(async (id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.some((n) => n.pitch === 62 && n.startBeats === 1), { what: "quantized" }, trackId);
  assert.ok((await notes()).includes("62@5/1"), JSON.stringify(await notes()));
  assert.match(await toast(), /ausgerichtet/);
  // without a selection: a hint instead of nothing
  await app.eval(async () => {
    (await import("./pianoroll.js")).noteSel.value = null;
    await (await import("./actions.js")).quantizeSelectedNotes(1, false);
  });
  assert.match(await toast(), /Erst Noten markieren/);
});

// ---------------------------------------------------------------- phone

test("phone: a box drawn with a finger stays selected while holding an empty spot – quantize and length work by tap, the options are finger-sized", async () => {
  await viewport(390, 844, { touch: true });
  await setup({ clipStart: 0, ppb: 50, notes: [
    { pitch: 60, startBeats: 0.1, lengthBeats: 0.4, velocity: 100 },
    { pitch: 62, startBeats: 1.3, lengthBeats: 0.4, velocity: 100 },
    { pitch: 70, startBeats: 3, lengthBeats: 0.5, velocity: 100 },
  ] });
  const g = await geo();
  // a box around the first two notes, drawn from an empty spot inside the clip
  await drag([X(g, 2.4), Y(g, 58)], [X(g, 0.05), Y(g, 64)]);
  assert.deepEqual(await sel(), ["60@0.1", "62@1.3"], "selected by the box");
  // hold an empty spot: the menu is for the selected notes
  await hold(X(g, 2.6), Y(g, 66));
  assert.deepEqual(await sel(), ["60@0.1", "62@1.3"], "holding keeps the selection");
  assert.ok(await center('.ctx-menu [data-act="ausrichten"]'), "the menu offers quantize");
  const sizes = await app.eval(() => [...document.querySelectorAll(".ctx-menu .ctx-chip")].map((c) => { const b = c.getBoundingClientRect(); return [Math.round(b.width), Math.round(b.height), b.left >= 0 && b.right <= innerWidth]; }));
  assert.ok(sizes.every(([w, h, inside]) => w >= 40 && h >= 36 && inside), `finger-sized chips inside the screen: ${JSON.stringify(sizes)}`);
  await tapOn('.ctx-menu [data-act="raster-4"]');
  await app.waitFor(async (id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.some((n) => n.pitch === 62 && n.startBeats === 1), { what: "quantized by tap" }, trackId);
  assert.deepEqual(await notes(), ["60@0/0.4", "62@1/0.4", "70@3/0.5"]);
  // hold a selected note: its menu – "Length" by tap
  await hold(X(g, 1.15), Y(g, 62));
  await tapOn('.ctx-menu [data-act="laenge-8"]');
  await app.waitFor(async (id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.filter((n) => n.lengthBeats === 0.5).length === 3, { what: "eighths by tap" }, trackId);
  assert.deepEqual(await notes(), ["60@0/0.5", "62@1/0.5", "70@3/0.5"]);
  // a tap on an empty spot clears the selection
  await tap(X(g, 2.6), Y(g, 66));
  await sleep(450); // (no double tap)
  assert.deepEqual(await sel(), []);
  assert.deepEqual(app.errors, []);
});

test("phone: the slim header shows the length as one button – its menu by tap; a double tap and the piano at the end use it", async () => {
  await viewport(390, 844, { touch: true });
  await setup({ clipStart: 0, ppb: 50, notes: [{ pitch: 60, startBeats: 0, lengthBeats: 1, velocity: 100 }] });
  const row = `.track-row[data-track-id="${trackId}"]`;
  await app.waitFor(() => document.querySelector(".workspace.narrow"), { what: "slim headers" });
  const shown = await app.eval((row) => ({
    values: document.querySelector(`${row} .note-values`).getClientRects().length,
    compact: document.querySelector(`${row} .nv-compact`)?.textContent,
    switch: document.querySelector(`${row} .entry-switch`).getClientRects().length,
  }), row);
  assert.deepEqual(shown, { values: 0, compact: "♪ Auto", switch: 1 }, "one button instead of the row; the keyboard switch stays");
  await tapOn(`${row} .nv-compact`);
  await app.waitFor(() => !!document.querySelector('.ctx-menu [data-act="notenwert-16"]'), { what: "choice of the length" });
  assert.equal(await app.eval(() => document.querySelector('.ctx-menu [data-act="notenwert-0"]').getAttribute("aria-checked")), "true", "Auto chosen");
  await tapOn('.ctx-menu [data-act="notenwert-16"]');
  await app.waitFor((row) => document.querySelector(`${row} .nv-compact`).textContent === "♪ 1/16", { what: "button shows 1/16" }, row);
  // a double tap on an empty lane: a sixteenth
  const g = await geo();
  await tap(X(g, 2.1), Y(g, 64));
  await tap(X(g, 2.1), Y(g, 64));
  await app.waitFor(async (id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.length === 2, { what: "note from a double tap" }, trackId);
  assert.ok((await notes()).includes("64@2/0.25"), JSON.stringify(await notes()));
  // the piano at the end of the piece: appends, also by finger
  await app.eval(async () => (await import("./actions.js")).seekBeats(T.st.snapshot.peek().engine.endBeats));
  await app.waitFor(async () => (await import("./noteinput.js")).atPieceEnd(), { what: "playhead at the end" });
  await tap(...KEY(await geo(), 67));
  await idle();
  assert.ok((await notes()).includes("67@2.25/0.25"), JSON.stringify(await notes()));
  assert.deepEqual(app.errors, []);
  await viewport(1400, 900);
});
