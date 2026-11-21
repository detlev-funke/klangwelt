// Several voices at once in the browser version: a three-voice chorale (soprano,
// alto, bass – harmonic tones of equal level) as a recording. The analysis
// window switches between one line and several voices and between Auto and a
// given voice count; converting makes one instrument track per voice (one undo
// step); the clip's context menu offers it, whatever the mode; while recording
// (substitute microphone) the live view shows the voices.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startWeb, makeTest, sleep } from "./harness.mjs";

const SR = 44_100;
const BEAT = 0.6; // 100 BPM
/** (MIDI, beat, beats) per voice. */
const VOICES = [
  [[76, 0, 1], [74, 1, 1], [72, 2, 1], [74, 3, 1], [76, 4, 1], [76, 5, 1], [76, 6, 2]],
  [[67, 0, 2], [65, 2, 2], [64, 4, 2], [67, 6, 2]],
  [[48, 0, 2], [43, 2, 2], [36, 4, 2], [43, 6, 2]],
];

/** The chorale: every voice a harmonic tone (partials 1/h), at the same level. */
function chorale() {
  const out = new Float32Array(Math.round((8 * BEAT + 0.4) * SR));
  for (const voice of VOICES) {
    for (const [midi, beat, beats] of voice) {
      const f = 440 * 2 ** ((midi - 69) / 12);
      const dur = beats * BEAT * 0.95;
      const a = Math.round(beat * BEAT * SR);
      for (let i = 0; i < dur * SR; i++) {
        const t = i / SR;
        const env = Math.min(1, t / 0.01) * Math.max(0, Math.min(1, (dur - t) / 0.03)) * Math.exp(-t * 0.8);
        let v = 0;
        for (let h = 1; h <= 8 && f * h < SR * 0.45; h++) v += Math.sin(2 * Math.PI * f * h * t) / h;
        out[a + i] += 0.07 * env * v;
      }
    }
  }
  return out;
}

function wavBytes(samples) {
  const data = Buffer.alloc(samples.length * 2);
  samples.forEach((v, i) => data.writeInt16LE(Math.round(Math.max(-1, Math.min(1, v)) * 32767), i * 2));
  const head = Buffer.alloc(44);
  head.write("RIFF", 0);
  head.writeUInt32LE(36 + data.length, 4);
  head.write("WAVE", 8);
  head.write("fmt ", 12);
  head.writeUInt32LE(16, 16);
  head.writeUInt16LE(1, 20);
  head.writeUInt16LE(1, 22);
  head.writeUInt32LE(SR, 24);
  head.writeUInt32LE(SR * 2, 28);
  head.writeUInt16LE(2, 32);
  head.writeUInt16LE(16, 34);
  head.write("data", 36);
  head.writeUInt32LE(data.length, 40);
  return Buffer.concat([head, data]);
}

let app;
let wav;
const test = makeTest(() => app);
before(async () => {
  wav = join(mkdtempSync(join(tmpdir(), "klangwelt-voices-")), "Satz.wav");
  writeFileSync(wav, wavBytes(chorale()));
  app = await startWeb({ fakeMic: wav });
  await app.send("Emulation.setDeviceMetricsOverride", { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false });
});
after(async () => {
  await app?.close();
});

const project = () => app.eval(() => T.st.snapshot.peek().project);
const undoLabel = () => app.eval(() => T.st.snapshot.peek().undoLabel);
const toast = () => app.eval(() => document.querySelector("kw-toast")?.textContent ?? "");

async function fresh(mode = "mono", voices = "auto") {
  await app.eval(async (mode, voices) => {
    await T.st.act(T.backend.projectNew());
    await T.backend.transport("stop");
    await T.st.run({ type: "setProject", patch: { tempoBpm: 100 } });
    const v = await import("./voices.js");
    v.setAnalysisMode(mode);
    v.setVoiceCount(voices);
    v.voicesAnalysis.value = null;
    T.st.analysis.value = null;
    T.st.selection.value = null;
    T.st.viewport.value = { pxPerBeat: 50, scrollX: 0, scrollY: 0 };
    T.st.showPanel("spuren");
    await T.sleep(200);
  }, mode, voices);
}

/** The chorale dropped into the window: an audio track. */
async function dropChorale() {
  const b64 = readFileSync(wav).toString("base64");
  await app.eval((b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], "Satz.wav", { type: "audio/wav" }));
    window.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, clientX: 5, clientY: 5, bubbles: true, cancelable: true }));
  }, b64);
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.some((t) => t.kind === "audio"), { timeout: 15000, what: "audio track" });
  return app.eval(() => {
    const t = T.st.snapshot.peek().project.tracks.find((t) => t.kind === "audio");
    return { trackId: t.id, clipId: t.clips[0].id };
  });
}

async function analyze(rec) {
  await app.eval(async (sel) => {
    T.st.selection.value = sel;
    await (await import("./actions.js")).analyzeSelected();
  }, rec);
}

const facts = () => app.eval(() => Object.fromEntries([...document.querySelectorAll("kw-panel[tab=analyse] .facts > .prop:not(.analysis-mode)")].map((p) => [p.querySelector("label").textContent, [p.querySelector(".value")?.textContent.trim(), p.querySelector(".hint")?.textContent ?? ""]])));

test("analysis window: one line or several voices – Auto finds three in the chorale, a given count is taken", async () => {
  await fresh("mono");
  const rec = await dropChorale();
  await analyze(rec);
  await app.waitFor(() => !!document.querySelector("kw-panel[tab=analyse] .mode-choice"), { what: "mode choice", timeout: 20000 });
  assert.deepEqual(await app.eval(() => T.$$("kw-panel[tab=analyse] .mode-choice .seg-btn").map((b) => [b.textContent, b.getAttribute("aria-pressed")])), [["Einstimmig", "true"], ["Mehrstimmig", "false"]]);
  assert.equal(await app.eval(() => getComputedStyle(T.$("kw-panel[tab=analyse] .voices-choice").closest(".prop")).display), "none", "the voice count only with several voices");

  await app.clickOn('kw-panel[tab=analyse] .mode-choice [data-value="poly"]');
  await app.waitFor(() => !!document.querySelector("kw-panel[tab=analyse] .analysis.voices"), { what: "voices panel", timeout: 20000 });
  assert.equal(await app.eval(() => localStorage.getItem("klangwelt.analysemodus")), "poly", "remembered");
  const f = await facts();
  assert.deepEqual(f["Stimmen"], ["3", "in der Aufnahme erkannt"], JSON.stringify(f));
  assert.deepEqual(await app.eval(() => T.$$("kw-panel[tab=analyse] .legend span").map((s) => s.textContent)), ["Stimme 1", "Stimme 2", "Stimme 3", "gefundene Tonhöhen"]);
  const v = await app.eval(async () => (await import("./voices.js")).voicesAnalysis.peek());
  const voice = (i) => v.notes.filter((n) => n.voice === i).map((n) => n.midi);
  assert.deepEqual(voice(2), [48, 43, 36, 43], JSON.stringify(v.notes));
  assert.ok(voice(1).filter((m) => [67, 65, 64].includes(m)).length >= 3, `alto: ${voice(1)}`);
  assert.ok(voice(0).filter((m) => [76, 74, 72].includes(m)).length >= 5, `soprano: ${voice(0)}`);
  assert.equal(v.key?.name, "C-Dur");

  // a given count
  await app.clickOn('kw-panel[tab=analyse] .voices-choice [data-value="2"]');
  await app.waitFor(() => T.$("kw-panel[tab=analyse] .legend")?.textContent.includes("Stimme 2") && !T.$("kw-panel[tab=analyse] .legend").textContent.includes("Stimme 3"), { what: "two voices" });
  assert.deepEqual((await facts())["Stimmen"], ["2", "wie gewählt"]);
  await app.clickOn('kw-panel[tab=analyse] .voices-choice [data-value="auto"]');
  await app.waitFor(() => T.$("kw-panel[tab=analyse] .legend")?.textContent.includes("Stimme 3"), { what: "three voices again" });
});

test("converting with several voices: one instrument track per voice below the recording, the bass as a bass track – one undo step", async () => {
  await fresh("poly");
  const rec = await dropChorale();
  await analyze(rec);
  await app.waitFor(() => !!document.querySelector("kw-panel[tab=analyse] .voices-convert"), { what: "convert button", timeout: 20000 });
  assert.equal(await app.eval(() => T.$("kw-panel[tab=analyse] .voices-convert").textContent), "Stimmen → Instrumentspuren");
  await app.clickOn("kw-panel[tab=analyse] .voices-convert");
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.length === 4, { what: "voice tracks", timeout: 20000 });
  const p = await project();
  assert.deepEqual(p.tracks.map((t) => t.name), ["Satz", "Stimme 1 aus Satz", "Stimme 2 aus Satz", "Bass aus Satz"]);
  assert.equal(p.tracks[3].instrument.kind, "bass");
  assert.deepEqual(p.tracks[3].clips[0].source.notes.map((n) => [n.pitch, n.startBeats]), [[48, 0], [43, 2], [36, 4], [43, 6]]);
  assert.match(await toast(), /3 Spuren angelegt, eine je Stimme/);
  assert.equal(await undoLabel(), "In Stimmen-Spuren umwandeln");
  await app.eval(async () => { await (await import("./actions.js")).undo(); });
  assert.equal((await project()).tracks.length, 1, "one undo step");
});

test("context menu of a recording: \"Convert to voice tracks\" works in the single-line mode, too", async () => {
  await fresh("mono");
  const rec = await dropChorale();
  const pt = await app.eval(async (id) => {
    const pr = await import("./pianoroll.js");
    const p = T.st.snapshot.peek().project;
    const i = p.tracks.findIndex((t) => t.id === id);
    const v = T.st.viewport.peek();
    const c = document.querySelector("kw-timeline canvas").getBoundingClientRect();
    return [c.x + 1.5 * v.pxPerBeat - v.scrollX, c.y + T.st.RULER_H + pr.rowTops(p.tracks)[i] - v.scrollY + 30];
  }, rec.trackId);
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: pt[0], y: pt[1] });
  await app.send("Input.dispatchMouseEvent", { type: "mousePressed", x: pt[0], y: pt[1], button: "right", buttons: 2, clickCount: 1 });
  await app.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: pt[0], y: pt[1], button: "right", buttons: 0, clickCount: 1 });
  await app.waitFor(() => !!document.querySelector('.ctx-menu [data-act="stimmen"]'), { what: "menu entry" });
  assert.equal(await app.eval(() => T.$('.ctx-menu [data-act="stimmen"] b').textContent), "In Stimmen-Spuren umwandeln");
  await app.clickOn('.ctx-menu [data-act="stimmen"]');
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.length === 4, { what: "voice tracks", timeout: 20000 });
  assert.equal(await undoLabel(), "In Stimmen-Spuren umwandeln");
  assert.equal(await app.eval(async () => (await import("./voices.js")).analysisMode.peek()), "mono", "the mode stays");
});

test("recording with several voices: the live view shows the voices and what sounds now", async () => {
  await fresh("poly");
  await app.eval(async () => {
    await (await import("./actions.js")).toggleRecord();
  });
  await app.waitFor(() => !!document.querySelector(".analysis.live"), { what: "live view" });
  assert.match(await app.eval(() => T.$(".analysis.live .live-badge").textContent), /mehrstimmig/);
  await app.waitFor(async () => {
    const p = [...document.querySelectorAll(".analysis.live .prop")].find((e) => e.querySelector("label")?.textContent === "Stimmen");
    const v = await T.backend.recordLiveVoices();
    return p && Number(p.querySelector(".value").textContent) >= 2 && v && v.notes.length >= 6;
  }, { what: "voices found live", timeout: 20000 });
  const view = await app.eval(() => T.backend.recordLiveVoices());
  assert.ok(view && view.notes.length >= 6, JSON.stringify(view));
  assert.ok(view.sounding.length >= 2 || view.notes.length >= 6, "what sounds now");
  assert.ok(view.notes.some((n) => n.voice > 0), "more than one voice");
  await app.eval(async () => {
    await (await import("./actions.js")).toggleRecord();
  });
  await app.waitFor(() => !T.st.status.peek().recording && !!document.querySelector("kw-panel[tab=analyse] .analysis.voices"), { what: "finished voices analysis", timeout: 20000 });
  assert.equal(await app.eval(() => T.backend.recordLiveVoices()), null, "no recording, nothing live");
  await sleep(100);
});

test("no errors in the page", () => {
  assert.deepEqual(app.errors, []);
});
