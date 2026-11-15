// Live analysis while recording in the browser: the substitute microphone plays
// a known melody (C major, a note every half second = 120 BPM) instead of its
// beep. While recording, the analysis window shows what sounds right now, the
// level, key and tempo, the core hands out only what is new, and after
// stopping the finished analysis replaces the live view.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startWeb, makeTest, sleep } from "./harness.mjs";

/** Mono 16-bit WAV. */
function writeWav(path, samples, sr) {
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
  head.writeUInt32LE(sr, 24);
  head.writeUInt32LE(sr * 2, 28);
  head.writeUInt16LE(2, 32);
  head.writeUInt16LE(16, 34);
  head.write("data", 36);
  head.writeUInt32LE(data.length, 40);
  writeFileSync(path, Buffer.concat([head, data]));
}

/** C major melody, one plucked note every 0.5 s (120 BPM), 6 s – loops seamlessly. */
const MELODY = [60, 64, 67, 72, 67, 64, 65, 69, 67, 62, 59, 60];
function melody(sr) {
  const out = [];
  for (const m of MELODY) {
    const f = 440 * 2 ** ((m - 69) / 12);
    const n = Math.round(0.5 * sr);
    for (let i = 0; i < n; i++) {
      const t = i / sr;
      const env = Math.min(1, t / 0.005) * Math.exp(-t * 2) * Math.max(0, Math.min(1, (0.46 - t) / 0.02));
      const w = 2 * Math.PI * f * t;
      out.push(env * (0.4 * Math.sin(w) + 0.15 * Math.sin(2 * w) + 0.06 * Math.sin(3 * w)));
    }
  }
  return out;
}

let app;
const test = makeTest(() => app);
before(async () => {
  const wav = join(mkdtempSync(join(tmpdir(), "klangwelt-mic-")), "melody.wav");
  writeWav(wav, melody(48000), 48000);
  app = await startWeb({ fakeMic: wav });
});
after(async () => {
  await app?.close();
});

/** Label → value of the facts in the analysis window. */
const facts = (sel) =>
  app.eval((s) => Object.fromEntries([...document.querySelectorAll(`${s} .prop`)].map((p) => [p.querySelector("label").textContent, p.querySelector(".value").textContent.trim()])), sel);

test("while recording, the analysis window shows the live analysis: note, level, key and tempo", async () => {
  await app.eval(() => T.st.act(T.backend.projectNew()));
  await app.clickOn("kw-transport .btn.rec");
  await app.waitFor(() => T.st.status.peek().liveAnalysis && !!document.querySelector(".analysis.live"), { what: "live view" });

  // tuner: a note of the melody with its deviation
  const note = await app.waitFor(() => (/^[A-H]/.test(T.$(".live-note").textContent) ? T.$(".live-note").textContent : null), { timeout: 8000, what: "current note" });
  assert.match(note, /^(C|D|E|F|G|A|H)[3-5]$/, note);
  // key and tempo once enough has been played (key after six notes, tempo after four onsets)
  const live = await app.waitFor(
    (sel) => {
      const f = Object.fromEntries([...document.querySelectorAll(`${sel} .prop`)].map((p) => [p.querySelector("label").textContent, p.querySelector(".value").textContent.trim()]));
      return f.Tonart === "C-Dur" && Math.abs(parseFloat(f["Tempo-Schätzung"]) - 120) < 5 ? f : null;
    },
    { timeout: 20000, what: "key C major and tempo about 120 BPM" },
    ".analysis.live",
  );
  await app.shot("live analysis while recording");
  const bpm = parseFloat(live["Tempo-Schätzung"]);
  assert.ok(Math.abs(bpm - 120) < 5, `tempo ${bpm}`);
  assert.ok(Number(live.Noten) >= 6, `notes: ${live.Noten}`);
  assert.match(live.Eingangspegel, /^−\d+ dB$/, live.Eingangspegel);
  assert.ok(await app.eval(() => T.$(".level-bar i").style.width !== "0%"), "the level bar moves");

  // the core: everything for a new cursor, then only what is new
  const a = await app.eval(() => T.backend.recordLive({ take: 0, curve: 0, notes: 0, onsets: 0 }));
  assert.equal(a.curveFrom, 0);
  assert.ok(a.curve.length > 200 && a.notes.length + a.pending.length >= 6 && a.onsets.length >= 4, `${a.curve.length} points, ${a.notes.length}+${a.pending.length} notes, ${a.onsets.length} onsets`);
  const scale = new Set([0, 2, 4, 5, 7, 9, 11]);
  const off = [...a.notes, ...a.pending].filter((n) => !scale.has(n.midi % 12));
  assert.ok(off.length <= 1, `notes outside C major: ${JSON.stringify(off)}`);
  await sleep(300);
  const b = await app.eval((c) => T.backend.recordLive(c), { take: a.take, curve: a.curve.length, notes: a.notes.length, onsets: a.onsets.length });
  assert.equal(b.curveFrom, a.curve.length);
  assert.ok(b.curve.length > 0 && b.curve.length < 100, `new points: ${b.curve.length}`);
  assert.ok(b.seconds > a.seconds);

  // stop: the finished analysis replaces the live view
  await app.clickOn("kw-transport .btn.rec");
  await app.waitFor(() => !T.st.status.peek().recording && T.st.analysis.peek() && !document.querySelector(".analysis.live") && !!document.querySelector(".analysis"), { timeout: 15000, what: "finished analysis" });
  const done = await facts(".analysis");
  assert.equal(done.Tonart, "C-Dur");
  assert.ok(Number(done.Noten) >= 6, `notes: ${done.Noten}`);
  assert.equal(await app.eval(() => T.backend.recordLive({ take: 0, curve: 0, notes: 0, onsets: 0 })), null, "no recording, no live analysis");
  assert.equal(await app.eval(() => T.st.status.peek().liveAnalysis), false);
  assert.deepEqual(app.errors, []);
});

test("a second recording starts the live view afresh", async () => {
  if (await app.eval(() => T.st.status.peek().recording)) await app.clickOn("kw-transport .btn.rec");
  await app.waitFor(() => !T.st.status.peek().recording, { timeout: 15000, what: "no recording running" });
  await app.clickOn("kw-transport .btn.rec");
  await app.waitFor(() => !!document.querySelector(".analysis.live"), { what: "live view" });
  const first = await app.waitFor(() => T.backend.recordLive({ take: 0, curve: 0, notes: 0, onsets: 0 }).then((v) => (v && v.seconds > 0.5 ? v : null)), { what: "running analysis" });
  assert.ok(first.take >= 2, `take ${first.take}`);
  assert.ok(first.seconds < 5, `starts at zero: ${first.seconds} s`);
  await app.clickOn("kw-transport .btn.rec");
  await app.waitFor(() => !T.st.status.peek().recording && !document.querySelector(".analysis.live"), { timeout: 15000, what: "stopped" });
  assert.deepEqual(app.errors, []);
});
