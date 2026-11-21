// Smoothing notes from recordings in the browser version: pitch detection breaks a sung
// line into fragments (an octave slip, a breathy voice break, a scoop); the strength chosen
// in the gear menu or the analysis window decides what converting the recording keeps.
// Afterwards the menu of selected notes and the menu of a note clip smooth them – one undo
// step each, and a message when there is nothing to smooth.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { startWeb, makeTest, sleep } from "./harness.mjs";

let app;
const test = makeTest(() => app);
before(async () => {
  app = await startWeb();
  await app.send("Emulation.setDeviceMetricsOverride", { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false });
});
after(async () => {
  await app?.close();
});

const project = () => app.eval(() => T.st.snapshot.peek().project);
const undoLabel = () => app.eval(() => T.st.snapshot.peek().undoLabel);
const toast = () => app.eval(() => document.querySelector("kw-toast")?.textContent ?? "");
const level = () => app.eval(async () => (await import("./smoothing.js")).smoothLevel.peek());
const undo = () => app.eval(async () => { await (await import("./actions.js")).undo(); });

async function rightClick(x, y) {
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await app.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "right", buttons: 2, clickCount: 1 });
  await app.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "right", buttons: 0, clickCount: 1 });
  await app.waitFor(() => !!document.querySelector(".ctx-menu"), { what: "context menu" });
}

async function fresh() {
  await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    await T.backend.transport("stop");
    (await import("./pianoroll.js")).noteSel.value = null;
    T.st.selection.value = null;
    T.st.viewport.value = { pxPerBeat: 60, scrollX: 0, scrollY: 0 };
    T.st.showPanel("spuren");
    await T.sleep(200);
  });
}

/**
 * A sung line dropped into the window (WAV, 44.1 kHz): c' with an octave slip, e' with a
 * breathy voice break, a scoop d' → g', d' d' re-struck after a breath, c' – two beats each
 * at 120 BPM. The same signal as in the core's test (convert.rs).
 */
async function dropBrokenRecording() {
  await app.eval(() => {
    const parts = [
      [60, 0.44, 0], [72, 0.13, 0], [60, 0.39, 0], [0, 0.04, 2],
      [64, 0.47, 0], [64, 0.05, 1], [64, 0.44, 0], [0, 0.04, 2],
      [62, 0.11, 0], [67, 0.85, 0], [0, 0.04, 2],
      [62, 0.44, 0], [0, 0.06, 2], [62, 0.44, 0], [0, 0.06, 2],
      [60, 0.9, 0], [0, 0.1, 2],
    ];
    const sr = 44100;
    const x = [];
    let phase = 0;
    let noise = 0x2545f491;
    let since = 0;
    for (const [midi, secs, kind] of parts) {
      const f = 440 * 2 ** ((midi - 69) / 12);
      for (let i = 0; i < Math.floor(secs * sr); i++) {
        const env = Math.min(1, since / 0.015);
        since += 1 / sr;
        if (kind === 0) {
          phase += (2 * Math.PI * f) / sr;
          x.push(env * (0.4 * Math.sin(phase) + 0.15 * Math.sin(2 * phase) + 0.06 * Math.sin(3 * phase)));
        } else if (kind === 1) {
          noise = (noise ^ (noise << 13)) >>> 0;
          noise = (noise ^ (noise >>> 17)) >>> 0;
          noise = (noise ^ (noise << 5)) >>> 0;
          x.push(0.3 * env * ((noise / 4294967295) * 2 - 1) * 1.7);
        } else {
          since = 0;
          x.push(0);
        }
      }
    }
    const buf = new ArrayBuffer(44 + 2 * x.length);
    const v = new DataView(buf);
    const tag = (o, s) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
    tag(0, "RIFF");
    v.setUint32(4, 36 + 2 * x.length, true);
    tag(8, "WAVE");
    tag(12, "fmt ");
    v.setUint32(16, 16, true);
    v.setUint16(20, 1, true);
    v.setUint16(22, 1, true);
    v.setUint32(24, sr, true);
    v.setUint32(28, 2 * sr, true);
    v.setUint16(32, 2, true);
    v.setUint16(34, 16, true);
    tag(36, "data");
    v.setUint32(40, 2 * x.length, true);
    x.forEach((s, i) => v.setInt16(44 + 2 * i, Math.round(Math.max(-1, Math.min(1, s)) * 32767), true));
    const dt = new DataTransfer();
    dt.items.add(new File([buf], "Gesang mit Brüchen.wav", { type: "audio/wav" }));
    window.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, clientX: 5, clientY: 5, bubbles: true, cancelable: true }));
  });
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.some((t) => t.kind === "audio"), { timeout: 15000, what: "audio track" });
  return app.eval(() => {
    const t = T.st.snapshot.peek().project.tracks.find((t) => t.kind === "audio");
    return { trackId: t.id, clipId: t.clips[0].id };
  });
}

/** Window point inside the clip's row at `beat`. */
const rowPoint = (trackId, beat) => app.eval(async (id, beat) => {
  const pr = await import("./pianoroll.js");
  const p = T.st.snapshot.peek().project;
  const i = p.tracks.findIndex((t) => t.id === id);
  const v = T.st.viewport.peek();
  const c = document.querySelector("kw-timeline canvas").getBoundingClientRect();
  return [c.x + beat * v.pxPerBeat - v.scrollX, c.y + T.st.RULER_H + pr.rowTops(p.tracks)[i] - v.scrollY + 30];
}, trackId, beat);

/** Window point of a pitch lane at `beat` in a track shown as a piano roll. */
const notePoint = (trackId, beat, pitch) => app.eval(async (id, beat, pitch) => {
  const pr = await import("./pianoroll.js");
  const p = T.st.snapshot.peek().project;
  const i = p.tracks.findIndex((t) => t.id === id);
  const g = pr.geoFor(p.tracks[i]);
  const v = T.st.viewport.peek();
  const c = document.querySelector("kw-timeline canvas").getBoundingClientRect();
  return [c.x + beat * v.pxPerBeat - v.scrollX, c.y + T.st.RULER_H + pr.rowTops(p.tracks)[i] - v.scrollY + pr.laneY(g, pitch) + g.laneH / 2];
}, trackId, beat, pitch);

/** Converts the recording with the clip's context menu; returns the new track's pitches. */
async function convertByMenu(rec) {
  const before = (await project()).tracks.length;
  await rightClick(...(await rowPoint(rec.trackId, 1.5)));
  await app.clickOn('.ctx-menu [data-act="melodie"]');
  await app.waitFor((n) => T.st.snapshot.peek().project.tracks.length === n + 1, { what: "melody track", timeout: 20000 }, before);
  const t = (await project()).tracks.find((t) => t.name.startsWith("Melodie aus"));
  return t.clips[0].source.notes.map((n) => n.pitch);
}

test("gear menu and analysis window choose how much a recording is smoothed – converting uses it", async () => {
  await fresh();
  assert.equal(await level(), "medium", "medium by default");
  const rec = await dropBrokenRecording();

  // gear menu: off
  await app.clickOn('[data-help="einstellungen"]');
  await app.waitFor(() => document.querySelectorAll(".set-menu.open [data-smooth]").length === 4, { what: "smoothing in the settings" });
  const menu = await app.eval(() => ({
    titles: T.$$(".set-menu.open .menu-title").map((e) => e.textContent),
    options: T.$$(".set-menu.open [data-smooth]").map((b) => [b.textContent, b.getAttribute("aria-pressed")]),
  }));
  assert.ok(menu.titles.includes("Noten aus Aufnahmen glätten"), JSON.stringify(menu.titles));
  assert.deepEqual(menu.options, [["Aus", "false"], ["Leicht", "false"], ["Mittel", "true"], ["Stark", "false"]]);
  await app.clickOn('.set-menu.open [data-smooth="off"]');
  assert.equal(await level(), "off");
  assert.equal(await app.eval(() => localStorage.getItem("klangwelt.glaetten")), "off", "remembered");

  // off: the fragments stay notes of their own
  const raw = await convertByMenu(rec);
  assert.ok(raw.length >= 9, `broken up: ${raw}`);
  assert.ok(raw.includes(72), `the octave slip: ${raw}`);
  assert.doesNotMatch(await toast(), /geglättet/);
  await undo();

  // analysis window: medium
  await app.eval(async (sel) => {
    T.st.selection.value = sel;
    await (await import("./actions.js")).analyzeSelected();
  }, rec);
  await app.waitFor(() => document.querySelectorAll("kw-panel[tab=analyse] .smooth-choice [data-smooth]").length === 4, { what: "smoothing in the analysis window", timeout: 20000 });
  assert.equal(await app.eval(() => T.$('kw-panel[tab=analyse] .smooth-choice [data-smooth="off"]').getAttribute("aria-pressed")), "true", "shows the choice from the gear menu");
  await app.clickOn('kw-panel[tab=analyse] .smooth-choice [data-smooth="medium"]');
  await app.waitFor(() => T.$('kw-panel[tab=analyse] .smooth-choice [data-smooth="medium"]').classList.contains("on"), { what: "medium marked" });
  assert.equal(await level(), "medium");
  await app.eval(() => T.st.showPanel("spuren"));
  await sleep(200);

  const smooth = await convertByMenu(rec);
  assert.deepEqual(smooth, [60, 64, 67, 62, 62, 60], "the line as it was sung");
  assert.match(await toast(), /6 Noten \(auf Sechzehntel gerastert, \d+ Bruchstücke geglättet\)/);
  const t = (await project()).tracks.find((t) => t.name.startsWith("Melodie aus"));
  assert.deepEqual(t.clips[0].source.notes.slice(0, 3).map((n) => n.startBeats), [0, 2, 4], "each held note from its start");
  assert.equal(await undoLabel(), "In Melodie-Spur umwandeln", "one undo step");
});

test("menu of selected notes and of a note clip: Smooth – fragments merged, one undo step; nothing left says so", async () => {
  await fresh();
  const notes0 = [
    { pitch: 60, startBeats: 0, lengthBeats: 0.9, velocity: 100 },
    { pitch: 72, startBeats: 0.9, lengthBeats: 0.18, velocity: 100 },
    { pitch: 60, startBeats: 1.08, lengthBeats: 0.92, velocity: 100 },
    { pitch: 63, startBeats: 2, lengthBeats: 0.12, velocity: 110 },
    { pitch: 64, startBeats: 2.12, lengthBeats: 0.88, velocity: 90 },
    { pitch: 67, startBeats: 3, lengthBeats: 1, velocity: 100 },
  ];
  const trackId = await app.eval(async (notes) => {
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Gesungen", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    if (t.view !== "notes") await T.st.run({ type: "setTrack", trackId: t.id, patch: { view: "notes" } });
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Noten", startBeats: 0, source: { type: "notes", lengthBeats: 4, notes } } });
    const pr = await import("./pianoroll.js");
    pr.pitchViews.update((m) => ({ ...m, [t.id]: { low: 55, span: 21 } }));
    await T.sleep(250);
    return t.id;
  }, notes0);
  const notes = () => app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes
    .map((n) => `${n.pitch}@${+n.startBeats.toFixed(4)}/${+n.lengthBeats.toFixed(4)} v${n.velocity}`), trackId);
  const all = notes0.map((n) => `${n.pitch}@${n.startBeats}`);
  await app.eval(async (id, keys) => {
    const t = T.st.snapshot.peek().project.tracks.find((t) => t.id === id);
    (await import("./pianoroll.js")).noteSel.value = { trackId: id, clipId: t.clips[0].id, keys };
    T.st.selection.value = { trackId: id, clipId: t.clips[0].id };
  }, trackId, all);

  // selected notes: the menu row with three strengths
  await rightClick(...(await notePoint(trackId, 3.5, 67)));
  const row = await app.eval(() => {
    const r = T.$('.ctx-menu .ctx-row[data-act="glaetten"]');
    return r && { label: r.querySelector(".ctx-label").textContent, chips: [...r.querySelectorAll(".ctx-chip")].map((c) => [c.dataset.act, c.textContent]) };
  });
  assert.deepEqual(row, { label: "Glätten", chips: [["glaetten-light", "Leicht"], ["glaetten-medium", "Mittel"], ["glaetten-strong", "Stark"]] });
  await app.clickOn('.ctx-menu [data-act="glaetten-medium"]');
  await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.length === 3, { what: "smoothed" }, trackId);
  assert.deepEqual(await notes(), ["60@0/2 v100", "64@2/1 v110", "67@3/1 v100"], "slip and scoop gone; the scoop's attack stays");
  assert.equal(await undoLabel(), "Noten glätten (mittel)");
  assert.match(await toast(), /Geglättet \(Mittel\): 3 Bruchstücke zusammengelegt – Strg\+Z holt sie zurück/);
  assert.deepEqual((await app.eval(async () => (await import("./pianoroll.js")).noteSel.peek().keys)).length, 3, "the merged notes stay selected");
  await undo();
  assert.equal((await notes()).length, 6, "undo brings the fragments back");

  // no selection: the clip's menu smooths all its notes
  await app.eval(async () => { (await import("./pianoroll.js")).noteSel.value = null; });
  await rightClick(...(await notePoint(trackId, 3.5, 72)));
  assert.ok(await app.eval(() => !!T.$('.ctx-menu .ctx-row[data-act="glaetten-clip"]')), "Smooth notes in the clip's menu");
  await app.clickOn('.ctx-menu [data-act="glaetten-clip-strong"]');
  await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.length === 3, { what: "clip smoothed" }, trackId);
  assert.equal(await undoLabel(), "Noten glätten (stark)");

  // once more: nothing left to smooth – a message, no undo step
  await rightClick(...(await notePoint(trackId, 3.5, 72)));
  await app.clickOn('.ctx-menu [data-act="glaetten-clip-strong"]');
  await app.waitFor(() => /nichts zu glätten/.test(document.querySelector("kw-toast")?.textContent ?? ""), { what: "nothing to smooth" });
  assert.equal(await undoLabel(), "Noten glätten (stark)", "no empty undo step");

  // drums: no smoothing in the menu
  await app.eval(async () => {
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Drums", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    await T.st.run({ type: "setTrack", trackId: t.id, patch: { instrument: { kind: "drums", preset: "standard", params: {} } } });
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Beat", startBeats: 0, source: { type: "notes", lengthBeats: 4, notes: [
      { pitch: 42, startBeats: 0, lengthBeats: 0.25, velocity: 90 }, { pitch: 42, startBeats: 0.25, lengthBeats: 0.25, velocity: 90 },
    ] } } });
    await T.sleep(250);
  });
  const drums = await app.eval(() => T.st.snapshot.peek().project.tracks.at(-1).id);
  await rightClick(...(await rowPoint(drums, 0.1)));
  assert.equal(await app.eval(() => !!T.$('.ctx-menu [data-act^="glaetten"]')), false, "drum hits aren't smoothed");
  await app.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  await app.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
});

test("no errors in the window", () => {
  assert.deepEqual(app.errors, []);
});
