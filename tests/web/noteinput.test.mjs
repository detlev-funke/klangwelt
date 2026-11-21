// Note entry in the browser version: double-click in the piano roll adds a note,
// keyboard entry mode (switch next to the piano) adds notes with the letter keys and
// with clicks on the piano (Shift keeps the position: chords), and the shortcut line
// stays right below the last track. Playback that ends at the end of the piece leaves
// the mark there, so typing appends.
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

/**
 * Empty project with track "Test" in the piano roll: one clip from beat 4, one bar long,
 * with 72@3. 80 px per beat (grid ¼), visible pitches 55–75 (keyboard entry: C4 = A).
 */
async function setup() {
  trackId = await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    await T.backend.transport("stop");
    await T.backend.transport("rewind");
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Test", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    if (t.view !== "notes") await T.st.run({ type: "setTrack", trackId: t.id, patch: { view: "notes" } });
    const notes = [{ pitch: 72, startBeats: 3, lengthBeats: 0.5, velocity: 100 }];
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Noten", startBeats: 4, source: { type: "notes", lengthBeats: 4, notes } } });
    const pr = await import("./pianoroll.js");
    pr.pitchViews.update((m) => ({ ...m, [t.id]: { low: 55, span: 21 } }));
    T.st.viewport.value = { pxPerBeat: 80, scrollX: 0, scrollY: 0 };
    T.st.selection.value = null;
    T.st.showPanel("spuren");
    await T.sleep(250);
    return t.id;
  });
}

/** Screen geometry of a track's piano roll (window pixels), computed from the app's modules. */
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
      notesTop: pr.NOTES_TOP, rulerH: T.st.RULER_H, keys: keys ? { x: keys.x, y: keys.y } : null,
    };
  }, id);
}
const X = (g, beat) => g.cx + beat * g.ppb - g.scrollX;
const Y = (g, pitch) => {
  if (g.lanes[pitch] == null) throw new Error(`pitch ${pitch} not visible`);
  return g.rowY + g.lanes[pitch] + g.laneH / 2;
};

async function dblclick(x, y) {
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  for (const clickCount of [1, 2]) {
    await app.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount });
    await app.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount });
  }
  await sleep(250);
}

/** A letter key by its position (KeyboardEvent.code), optionally with modifiers. */
const letter = (l, mods = 0) => app.key(l, { code: `Key${l.toUpperCase()}`, vk: l.toUpperCase().charCodeAt(0), mods });

const notes = (id = trackId) => app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips
  .flatMap((c) => c.source.notes.map((n) => `${n.pitch}@${c.startBeats + n.startBeats}`)).sort(), id);
const entryNow = () => app.eval(async () => (await import("./noteinput.js")).entry.peek());
const idle = () => app.eval(async () => { await (await import("./noteinput.js")).entryIdle(); await T.sleep(100); });
const undo = () => app.eval(async () => { await (await import("./actions.js")).undo(); });
const sel = () => app.eval(async () => ((await import("./pianoroll.js")).noteSel.peek()?.keys ?? []).slice().sort());

async function switchOn(id = trackId) {
  await app.clickOn(`.track-row[data-track-id="${id}"] .entry-switch`);
  await app.waitFor(async (id) => (await import("./noteinput.js")).entry.peek()?.trackId === id, { what: "keyboard entry on" }, id);
}

test("double-click on an empty lane of the piano roll adds a note there – one undo step", async () => {
  await setup();
  const g = await geo();
  await dblclick(X(g, 6.3), Y(g, 64));
  await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.length === 2, { what: "new note" }, trackId);
  const c = await app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0], trackId);
  const n = c.source.notes.find((x) => x.pitch === 64);
  assert.equal(n.startBeats, 2.25, "start on the grid (¼) under the pointer");
  assert.equal(n.lengthBeats, 1, "length of the last set note (default one beat)");
  assert.match(await app.eval(() => T.st.snapshot.peek().undoLabel), /E4/);
  assert.deepEqual(await sel(), ["64@2.25"], "the new note is selected");
  // on an existing note a double-click adds nothing
  await dblclick(X(g, 6.5), Y(g, 64));
  assert.deepEqual(await notes(), ["64@6.25", "72@7"]);
  await undo();
  assert.deepEqual(await notes(), ["72@7"], "one undo step removes it");
});

test("double-click left of the clip: the clip grows to the left; without a clip one is created", async () => {
  await setup();
  let g = await geo();
  await dblclick(X(g, 1.1), Y(g, 62));
  await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.length === 2, { what: "note before the clip" }, trackId);
  const c = await app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips.map((c) => [c.startBeats, c.source.lengthBeats]), trackId);
  assert.deepEqual(c, [[0, 8]], "clip starts a bar earlier");
  assert.deepEqual(await notes(), ["62@1", "72@7"]);

  const empty = await app.eval(async () => {
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Leer", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    if (t.view !== "notes") await T.st.run({ type: "setTrack", trackId: t.id, patch: { view: "notes" } });
    (await import("./pianoroll.js")).pitchViews.update((m) => ({ ...m, [t.id]: { low: 55, span: 21 } }));
    await T.sleep(200);
    return t.id;
  });
  g = await geo(empty);
  await dblclick(X(g, 5.6), Y(g, 67));
  await app.waitFor(async (id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips.length === 1, { what: "new clip" }, empty);
  const made = await app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips.map((c) => ({ start: c.startBeats, len: c.source.lengthBeats, notes: c.source.notes.map((n) => `${n.pitch}@${n.startBeats}`) })), empty);
  assert.deepEqual(made, [{ start: 4, len: 4, notes: ["67@1.5"] }]);
});

test("keyboard entry: letters add notes of the visible octave, one grid step further; Shift stays (chord); Ctrl keeps its shortcuts; one undo step each", async () => {
  await setup();
  await app.eval(() => T.st.act(T.backend.setLoopRange([4, 8])));
  await switchOn();
  assert.deepEqual(await entryNow(), { trackId, beat: 4 }, "starts at the selected time range");
  assert.equal(await app.eval(() => document.querySelector(`.entry-switch.on`)?.getAttribute("aria-checked")), "true");
  await letter("a");
  await letter("s");
  await letter("d", MOD.shift);
  await letter("g", MOD.shift);
  await letter("j");
  await idle();
  assert.deepEqual(await notes(), ["60@4", "62@4.25", "64@4.5", "67@4.5", "71@4.5", "72@7"]);
  assert.deepEqual(await entryNow(), { trackId, beat: 4.75 });
  assert.deepEqual(await sel(), ["64@0.5", "67@0.5", "71@0.5"], "the chord is selected");
  // each note is its own undo step – Ctrl+Z undoes while typing
  await letter("z", MOD.ctrl);
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.at(-1).clips[0].source.notes.length === 5, { what: "Ctrl+Z undoes the last note" });
  assert.deepEqual(await notes(), ["60@4", "62@4.25", "64@4.5", "67@4.5", "72@7"]);
  // Ctrl + a note key is no note: Ctrl+Y (the G♯ key) redoes
  await letter("y", MOD.ctrl);
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.at(-1).clips[0].source.notes.length === 6, { what: "Ctrl+Y redoes" });
  await idle();
  assert.deepEqual(await notes(), ["60@4", "62@4.25", "64@4.5", "67@4.5", "71@4.5", "72@7"], "no G♯ from Ctrl+Y");
  assert.deepEqual(await entryNow(), { trackId, beat: 4.75 }, "Ctrl+letter doesn't move the mark");
  await undo();
  await undo();
  await undo();
  assert.deepEqual(await notes(), ["60@4", "62@4.25", "72@7"]);
  assert.deepEqual(app.errors, []);
});

test("keyboard entry: Backspace steps back and deletes, arrows and clicks move the mark, X shifts the octave", async () => {
  await setup();
  await app.eval(() => T.st.act(T.backend.setLoopRange([4, 8])));
  await switchOn();
  await letter("f");
  await letter("h");
  await idle();
  assert.deepEqual(await notes(), ["65@4", "69@4.25", "72@7"]);
  await app.key("Backspace", { code: "Backspace", vk: 8 });
  await idle();
  assert.deepEqual(await notes(), ["65@4", "72@7"]);
  assert.equal((await entryNow()).beat, 4.25);
  await app.key("ArrowRight", { code: "ArrowRight", vk: 39 });
  assert.equal((await entryNow()).beat, 4.5);
  await app.key("ArrowLeft", { code: "ArrowLeft", vk: 37, mods: MOD.shift });
  assert.equal((await entryNow()).beat, 0.5, "Shift: one bar");
  // a click into the piano roll puts the mark there (on the grid)
  const g = await geo();
  await app.click(X(g, 6.6), Y(g, 58));
  assert.deepEqual(await entryNow(), { trackId, beat: 6.5 });
  // a click into the ruler moves the playhead – and the mark
  await app.click(X(g, 2), g.cy + g.rulerH / 2);
  assert.equal((await entryNow()).beat, 2);
  // X: one octave up – the same key plays an octave higher
  await app.click(X(g, 6.1), Y(g, 58));
  await app.key("x", { code: "KeyX", vk: 88 });
  await letter("a");
  await idle();
  assert.deepEqual(await notes(), ["65@4", "72@6", "72@7"]);
  assert.deepEqual(await app.eval(async (id) => (await import("./pianoroll.js")).pitchViews.peek()[id], trackId), { low: 67, span: 21 });
});

test("keyboard entry: a click on the piano adds that note; L and R do nothing until Esc ends the mode", async () => {
  await setup();
  await app.eval(() => T.st.act(T.backend.setLoopRange([4, 8])));
  await switchOn();
  const g = await geo();
  // the piano shows the letters of the keys
  assert.equal(await app.eval(async (id) => {
    const { entryLetters } = await import("./noteinput.js");
    return entryLetters(T.st.snapshot.peek().project.tracks.find((t) => t.id === id)).get(60);
  }, trackId), "A");
  await app.click(g.keys.x + 6, g.keys.y + (g.lanes[65] - g.notesTop) + g.laneH / 2);
  await idle();
  assert.deepEqual(await notes(), ["65@4", "72@7"]);
  assert.equal((await entryNow()).beat, 4.25);

  const loopBefore = await app.eval(() => T.st.snapshot.peek().engine.loopOn);
  await letter("l"); // L = D in the next octave
  await letter("r"); // no key: nothing happens (no recording)
  await idle();
  assert.equal(await app.eval(() => T.st.snapshot.peek().engine.loopOn), loopBefore, "L doesn't toggle loop");
  assert.equal(await app.eval(() => T.st.status.peek().recording), false, "R doesn't record");
  assert.deepEqual(await notes(), ["65@4", "74@4.25", "72@7"].sort());

  await app.key("Escape", { code: "Escape", vk: 27 });
  assert.equal(await entryNow(), null, "Esc ends keyboard entry");
  assert.equal(await app.eval(() => !!document.querySelector(".entry-switch.on")), false);
  await letter("l");
  await app.waitFor((b) => T.st.snapshot.peek().engine.loopOn !== b, { what: "L toggles loop again" }, loopBefore);
  await letter("l");
});

test("playback stops right at the end of the piece: the mark follows and typing appends there; Play from the end starts at the beginning; loop goes on as before", async () => {
  // the piece: 72 over beat 7–7.5 (clip 4–8) – it ends at 7.5
  await setup();
  await app.eval(async () => {
    await T.st.act(T.backend.setLoopRange(null));
    await T.st.act(T.backend.setLoop(false));
  });
  assert.equal(await app.eval(() => T.st.snapshot.peek().engine.endBeats), 7.5, "end of the piece in the snapshot");
  await switchOn();
  assert.equal((await entryNow()).beat, 0, "starts at the playhead");
  const bpm = await app.eval(() => T.st.snapshot.peek().project.tempoBpm);
  const beatNow = () => app.eval((bpm) => (T.st.status.peek().positionSecs * bpm) / 60, bpm);
  const space = () => app.key(" ", { code: "Space", vk: 32 });
  await app.eval((bpm) => T.backend.seek((6 * 60) / bpm), bpm);
  await space();
  await app.waitFor(() => T.st.status.peek().playing, { what: "playing", timeout: 3000 });
  await app.waitFor(() => !T.st.status.peek().playing, { what: "stopped at the end", timeout: 5000 });
  const end = await beatNow();
  assert.ok(Math.abs(end - 7.5) < 0.001, `the playhead stays at the end: ${end}`);
  await sleep(400);
  assert.equal(await beatNow(), end, "and doesn't move on");
  assert.ok(await app.eval(async () => (await import("./dom.js")).icon("play").innerHTML === document.querySelector("kw-transport .btn.big.primary svg").innerHTML), "the transport shows Play again");
  assert.deepEqual(await entryNow(), { trackId, beat: 7.5 }, "the mark follows to the end");
  await letter("d");
  await letter("f");
  await idle();
  assert.deepEqual(await notes(), ["64@7.5", "65@7.75", "72@7"], "the typed notes append to the piece");
  assert.equal(await app.eval(() => T.st.snapshot.peek().engine.endBeats), 8, "the piece got longer");
  // Play from the old end: plays what was typed and stops at the new end
  await space();
  await app.waitFor((bpm) => !T.st.status.peek().playing && Math.abs((T.st.status.peek().positionSecs * bpm) / 60 - 8) < 0.001, { what: "stopped at the new end", timeout: 5000 }, bpm);
  assert.deepEqual(await entryNow(), { trackId, beat: 8 });
  // Play at the end starts at the beginning; pausing in the middle leaves the mark where it is
  await space();
  await app.waitFor((bpm) => T.st.status.peek().playing && (T.st.status.peek().positionSecs * bpm) / 60 > 0.5, { what: "playing from the front", timeout: 4000 }, bpm);
  assert.ok((await beatNow()) < 4, "from the front");
  await space();
  await app.waitFor(() => !T.st.status.peek().playing, { what: "paused" });
  assert.deepEqual(await entryNow(), { trackId, beat: 8 }, "a pause doesn't move the mark");
  // loop on: at the end of the piece (whole bars: beat 8) back to the start, no stop
  await app.eval(async (bpm) => {
    await T.st.act(T.backend.setLoop(true));
    await T.backend.seek((7 * 60) / bpm);
    await T.backend.transport("play");
  }, bpm);
  const seen = [];
  for (let k = 0; k < 14; k++) {
    await sleep(100);
    seen.push(await beatNow());
  }
  const playing = await app.eval(() => T.st.status.peek().playing);
  await app.eval(async () => {
    await T.backend.transport("stop");
    await T.st.act(T.backend.setLoop(false));
  });
  assert.ok(playing, "still playing after the end");
  assert.ok(seen.every((b) => b < 8.1) && seen.some((b) => b < 2), `back to the start: ${seen.map((b) => b.toFixed(2))}`);
  assert.deepEqual(app.errors, []);
});

test("the shortcut line stays visible right below the last track – at the bottom edge when the tracks go further", async () => {
  await setup();
  const where = () => app.eval(async () => {
    const pr = await import("./pianoroll.js");
    const c = document.querySelector("kw-timeline canvas").getBoundingClientRect();
    const h = document.querySelector("kw-timeline .keys-hint");
    const r = h.getBoundingClientRect();
    const tracks = T.st.snapshot.peek().project.tracks;
    return {
      top: r.top, bottom: r.bottom, visible: getComputedStyle(h).display !== "none" && r.height > 0,
      lastBottom: c.top + T.st.RULER_H + pr.contentHeight(tracks) - T.st.viewport.peek().scrollY, canvasBottom: c.bottom, text: h.textContent,
    };
  });
  let w = await where();
  assert.ok(w.visible, "visible");
  assert.ok(w.top >= w.lastBottom && w.top - w.lastBottom < 16, `right below the last track: ${JSON.stringify(w)}`);
  assert.match(w.text, /Leertaste/);
  // in keyboard entry mode it shows the mode's keys
  await switchOn();
  w = await where();
  assert.match(w.text, /Tastatureingabe/);
  await app.key("Escape", { code: "Escape", vk: 27 });
  // many tall tracks: the line sits at the bottom edge of the timeline
  await app.eval(async () => {
    for (let i = 0; i < 6; i++) {
      await T.st.run({ type: "addTrack", track: { id: 0, name: `Spur ${i}`, kind: "instrument" } });
      const t = T.st.snapshot.peek().project.tracks.at(-1);
      if (t.view !== "notes") await T.st.run({ type: "setTrack", trackId: t.id, patch: { view: "notes" } });
    }
    await T.sleep(250);
  });
  w = await where();
  assert.ok(w.lastBottom > w.canvasBottom, "tracks go further than the view");
  assert.ok(w.visible && w.bottom <= w.canvasBottom && w.canvasBottom - w.bottom < 16, `at the bottom edge: ${JSON.stringify(w)}`);
});

test("no errors in the page", () => {
  assert.deepEqual(app.errors, []);
});
