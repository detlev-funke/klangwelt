// Live analysis while recording in the browser: the substitute microphone plays
// a known melody (C major, a note every half second = 120 BPM, with a sforzato,
// two soft notes and a staccato note) instead of its beep. While recording, the analysis window shows what sounds right now, the
// level, key and tempo, the core hands out only what is new, and after
// stopping the finished analysis replaces the live view.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { startWeb, makeTest, sleep } from "./harness.mjs";
import { makeTempDir, removeTempDir } from "../tempdirs.mjs";

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

/** C major melody, one plucked note every 0.5 s (120 BPM), 6 s – loops seamlessly.
 *  C5 staccato (0.14 s), A4 sforzato (the attack 12 dB louder, falling back within
 *  about 30 ms), D4 and B3 soft (−12 dB) – each of those pitches occurs only there. */
const MELODY = [60, 64, 67, { midi: 72, secs: 0.14 }, 67, 64, 65, { midi: 69, accent: 4 }, 67, { midi: 62, amp: 0.25 }, { midi: 59, amp: 0.25 }, 60];
function melody(sr) {
  const out = [];
  for (const note of MELODY) {
    const { midi, amp = 1, accent = 1, secs = 0.46 } = typeof note === "number" ? { midi: note } : note;
    const f = 440 * 2 ** ((midi - 69) / 12);
    const n = Math.round(0.5 * sr);
    for (let i = 0; i < n; i++) {
      const t = i / sr;
      const gain = amp * (1 + (accent - 1) * Math.exp(-t / 0.03));
      const env = Math.min(1, t / 0.005) * Math.exp(-t * 2) * Math.max(0, Math.min(1, (secs - t) / 0.02));
      const w = 2 * Math.PI * f * t;
      out.push(gain * env * (0.15 * Math.sin(w) + 0.056 * Math.sin(2 * w) + 0.022 * Math.sin(3 * w)));
    }
  }
  return out;
}

let app;
let micDir;
const test = makeTest(() => app);
before(async () => {
  micDir = makeTempDir("klangwelt-mic-");
  const wav = join(micDir, "melody.wav");
  writeWav(wav, melody(48000), 48000);
  app = await startWeb({ fakeMic: wav });
});
after(async () => {
  await app?.close();
  await removeTempDir(micDir);
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

test("the analysis finds dynamics, a sforzato and staccato and carries them into the melody track", async () => {
  await app.eval(() => T.st.act(T.backend.projectNew()));
  await app.clickOn("kw-transport .btn.rec");
  await app.waitFor(() => !!document.querySelector(".analysis.live"), { what: "live view" });
  // live: the latest note's dynamics, the sforzato among the accents
  const dyn = await app.waitFor(
    () => {
      const p = [...T.$$(".analysis.live .prop")].find((p) => p.querySelector("label").textContent === "Dynamik");
      const v = p?.querySelector(".value").textContent ?? "";
      const hint = p?.querySelector(".hint").textContent ?? "";
      return /^(pp|p|mp|mf|f|ff)$/.test(v) && hint.includes("sfz") ? { v, hint } : null;
    },
    { timeout: 15000, what: "dynamics and the sforzato live" },
  );
  assert.match(dyn.hint, /sfz \d/);
  // a whole round of the melody (6 s) and a bit
  await app.waitFor(() => T.backend.recordLive({ take: 0, curve: 0, notes: 0, onsets: 0 }).then((v) => v && v.seconds >= 8), { timeout: 15000, what: "8 seconds recorded" });
  await app.clickOn("kw-transport .btn.rec");
  await app.waitFor(() => !T.st.status.peek().recording && !document.querySelector(".analysis.live") && !!document.querySelector(".analysis"), { timeout: 15000, what: "finished analysis" });

  await app.shot("analysis with dynamics and accents");
  const done = await facts(".analysis");
  assert.match(done.Akzente, /sfz \d/, done.Akzente);
  assert.match(done.Artikulation, /staccato \d/, done.Artikulation);
  assert.match(done.Dynamik, /^(pp|p|mp) – (f|ff)$/, done.Dynamik);
  const found = await app.eval(() => T.st.analysis.peek().notes.map((n) => ({ midi: n.midi, attack: n.attack, articulation: n.articulation, velocity: n.velocity })));
  const sfz = found.filter((n) => n.attack === "sforzato");
  assert.ok(sfz.length >= 1 && sfz.every((n) => n.midi === 69), `sforzato on A4 only: ${JSON.stringify(sfz)}`);
  assert.ok(found.some((n) => n.midi === 72 && n.articulation === "staccato"), "C5 is staccato");

  // into the notes: velocity, brighter attack, short staccato
  await app.clickOn(".analysis .btn", { text: "Melodie" });
  const notes = await app.waitFor(() => T.st.snapshot.peek().project.tracks.find((t) => t.kind === "instrument")?.clips.flatMap((c) => c.source.notes), { what: "melody track" });
  const of = (p) => notes.filter((n) => n.pitch === p);
  const median = (v) => [...v].sort((a, b) => a - b)[Math.floor(v.length / 2)];
  const usual = median([...of(64), ...of(67)].map((n) => n.velocity));
  const loudest = notes.reduce((a, b) => (b.velocity > a.velocity ? b : a));
  assert.equal(loudest.pitch, 69, `the sforzato is the loudest note: ${JSON.stringify(loudest)}`);
  assert.equal(loudest.brightness, 0.4, "with a brighter attack");
  assert.ok(loudest.velocity >= usual + 25, `sforzato ${loudest.velocity}, usual ${usual}`);
  for (const p of [62, 59]) assert.ok(of(p).length && of(p).every((n) => n.velocity < usual - 20), `soft ${p}: ${JSON.stringify(of(p))} (usual ${usual})`);
  // staccato: at most half the way to the next note (a beat), far shorter than the others
  assert.ok(of(72).some((n) => n.lengthBeats <= 0.5), `staccato C5: ${JSON.stringify(of(72))}`);
  assert.ok(median(of(67).map((n) => n.lengthBeats)) >= 0.75, `normal notes keep their length: ${JSON.stringify(of(67))}`);
  assert.deepEqual(app.errors, []);
});
