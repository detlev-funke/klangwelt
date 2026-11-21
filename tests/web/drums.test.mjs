// Drums with pitch in the browser: notes off the drum keys play tuned drums
// (drum kit), the switch "Keys: drum kit | pitch" in the Sound window plays
// every key by pitch – heard through the real AudioWorklet and at the right
// pitch in the exported WAV; the track shows what its keys play, the rhythm
// window only offers drum tracks in the drum kit.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { startWeb, makeTest, sleep } from "./harness.mjs";

let app;
const test = makeTest(() => app);
before(async () => {
  app = await startWeb();
  // Second measurement of the level directly at the output of the worklet
  await app.eval(() => {
    const { ctx, node } = window.__klangweltAudio;
    const a = ctx.createAnalyser();
    a.fftSize = 2048;
    node.connect(a);
    window.__analyser = a;
  });
});
after(async () => {
  await app?.close();
});

const track = (id) => app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id), id);

/** Empty project with one drums track holding a melody (C4 E4 G4 C5, twice); returns its id. */
async function melodyOnDrums(pitches = [60, 64, 67, 72]) {
  return app.eval(async (pitches) => {
    await T.st.act(T.backend.projectNew());
    await T.backend.transport("stop");
    await T.backend.transport("rewind");
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Trommelmelodie", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    const notes = [...pitches, ...pitches].map((pitch, i) => ({ pitch, startBeats: i * 0.5, lengthBeats: 0.45, velocity: 110 }));
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Melodie", startBeats: 0, source: { type: "notes", lengthBeats: 4, notes } } });
    await T.st.run({ type: "setTrack", trackId: t.id, patch: { instrument: { kind: "drums", preset: "standard", params: {} } } });
    return t.id;
  }, pitches);
}

const setKeys = (id, v) => app.eval(async (id, v) => {
  const t = T.st.snapshot.peek().project.tracks.find((x) => x.id === id);
  await T.st.run({ type: "setTrack", trackId: id, patch: { instrument: { ...t.instrument, params: { ...t.instrument.params, tasten: v } } } });
}, id, v);

/** Plays from the start for `ms` and returns the highest level (status and worklet output). */
async function playLevel(ms = 1500) {
  await app.eval(async () => {
    await T.backend.seek(0);
    await T.backend.transport("play");
  });
  await app.waitFor(() => T.st.status.peek().playing, { what: "Wiedergabe" });
  let peak = 0;
  let out = 0;
  for (const end = Date.now() + ms; Date.now() < end; ) {
    await sleep(60);
    const m = await app.eval(() => {
      const buf = new Float32Array(window.__analyser.fftSize);
      window.__analyser.getFloatTimeDomainData(buf);
      return { peak: T.st.status.peek().peakOut, out: buf.reduce((m, v) => Math.max(m, Math.abs(v)), 0) };
    });
    peak = Math.max(peak, m.peak);
    out = Math.max(out, m.out);
  }
  await app.eval(() => T.backend.transport("stop"));
  await app.waitFor(() => !T.st.status.peek().playing, { what: "Stopp" });
  return { peak, out };
}

/** Waits for a downloaded file; returns its path. */
async function downloaded(name, timeout = 20000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (readdirSync(app.downloads).includes(name)) return join(app.downloads, name);
    await sleep(150);
  }
  throw new Error(`Download fehlt: ${name}`);
}

/** Exports the project as WAV through the dialog; returns the left channel as numbers. */
async function exportWav(name) {
  await app.clickOn("kw-transport .file-btn");
  await app.clickOn(".file-menu.open .mi-export");
  await app.waitFor(() => T.$(".export-modal"), { what: "Export-Dialog" });
  await app.clickOn(".ex-format [data-format=wav]");
  await app.eval((name) => {
    const el = T.$(".ex-name");
    el.value = name;
    el.dispatchEvent(new Event("input", { bubbles: true }));
  }, name);
  await app.clickOn(".ex-go");
  await app.waitFor(() => T.$(".ex-reveal"), { timeout: 60000, what: "Export fertig" });
  const file = await downloaded(await app.eval(() => T.$(".ex-done span").textContent.trim()));
  await app.key("Escape", { code: "Escape", vk: 27 });
  const b = readFileSync(file);
  const bits = b.readUInt16LE(34);
  const step = 2 * (bits / 8);
  const out = [];
  for (let i = 44; i + step <= b.length; i += step) out.push(bits === 24 ? b.readIntLE(i, 3) / 8388608 : b.readInt16LE(i) / 32768);
  return out;
}

/** Energy at frequency `f` in samples [from, from + 0.25 s) at 48 kHz (Goertzel). */
function energyAt(x, f, from) {
  let re = 0;
  let im = 0;
  for (let i = 0; i < 12000; i++) {
    const w = (2 * Math.PI * f * i) / 48000;
    re += (x[from + i] ?? 0) * Math.cos(w);
    im += (x[from + i] ?? 0) * Math.sin(w);
  }
  return Math.hypot(re, im);
}

test("a melody on a drums track is heard – in the drum kit and by pitch", async () => {
  const id = await melodyOnDrums();
  assert.equal((await track(id)).instrument.kind, "drums");
  // drum kit (default): the notes C4 … C5 have no drum and play tuned drums
  const kit = await playLevel();
  assert.ok(kit.peak > 0.02 && kit.out > 0.02, `Schlagzeug: Status ${kit.peak}, Ausgang ${kit.out}`);
  // by pitch: the switch travels to the worklet with the instrument
  await setKeys(id, 1);
  const pitched = await playLevel();
  assert.ok(pitched.peak > 0.02 && pitched.out > 0.02, `Tonhöhe: Status ${pitched.peak}, Ausgang ${pitched.out}`);
  assert.deepEqual(app.errors, []);
});

test("export: tuned drums sound at the pitch of their notes", async () => {
  // first bar C4, second bar G4 – toms by pitch (tempo 120: one bar = 2 s)
  const id = await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Toms", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    const notes = [0, 1, 2, 3].flatMap((b) => [{ pitch: 60, startBeats: b, lengthBeats: 0.5, velocity: 110 }, { pitch: 67, startBeats: 4 + b, lengthBeats: 0.5, velocity: 110 }]);
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Toms", startBeats: 0, source: { type: "notes", lengthBeats: 8, notes } } });
    await T.st.run({ type: "setProject", patch: { tempoBpm: 120 } });
    await T.st.run({ type: "setTrack", trackId: t.id, patch: { instrument: { kind: "drums", preset: "standard", params: { tasten: 1 } } } });
    return t.id;
  });
  assert.equal((await track(id)).instrument.params.tasten, 1);
  const x = await exportWav("e2e-toms");
  const [c4, g4] = [261.63, 392.0];
  const first = 0.5 * 48000; // second hit of bar 1 (C4)
  const second = 2.5 * 48000; // second hit of bar 2 (G4)
  assert.ok(energyAt(x, c4, first) > 2 * energyAt(x, g4, first), `C4: ${energyAt(x, c4, first)} vs ${energyAt(x, g4, first)}`);
  assert.ok(energyAt(x, g4, second) > 2 * energyAt(x, c4, second), `G4: ${energyAt(x, g4, second)} vs ${energyAt(x, c4, second)}`);
});

test("Sound window: “Keys” switches drum kit ↔ pitch – one undo step, the track follows", async () => {
  const id = await melodyOnDrums();
  await app.eval((id) => {
    T.st.selection.value = { trackId: id, clipId: null };
    T.st.showPanel("klang");
  }, id);
  await app.waitFor(() => T.$(".snd-keys .seg-btn"), { what: "Tasten-Schalter" });
  // no auditioning after changes – the tests stay quiet
  await app.eval(() => T.$(".snd-play .seg-btn[data-mode=aus]").click());
  const ui = () => app.eval(async (id) => {
    const pr = await import("./pianoroll.js");
    const tracks = await import("./components/tracks.js");
    const t = T.st.snapshot.peek().project.tracks.find((x) => x.id === id);
    return {
      buttons: T.$$(".snd-keys .seg-btn").map((b) => b.textContent),
      on: T.$(".snd-keys .seg-btn.on")?.textContent,
      hint: T.$(".snd-keys-hint").textContent,
      clapIdle: T.$('.snd-fine .pgroup[data-group="Clap"]').classList.contains("idle"),
      mode: pr.drumMode(t),
      view: pr.viewFor(t),
      drumKeys: tracks.drumKeyNames(t)?.size ?? 0,
    };
  }, id);
  let s = await ui();
  assert.deepEqual(s.buttons, ["Schlagzeug", "Tonhöhe"]);
  assert.equal(s.on, "Schlagzeug");
  assert.match(s.hint, /gestimmte Trommel/);
  assert.equal(s.mode, "kit");
  assert.equal(s.clapIdle, false);
  // drum kit: the keyboard names the drums, the view shows them (kick drum at 36) together with the notes
  assert.ok(s.drumKeys >= 13, `Trommelnamen: ${s.drumKeys}`);
  assert.ok(s.view.low <= 36 && s.view.low + s.view.span > 72, `Ansicht Schlagzeug: ${JSON.stringify(s.view)}`);

  await app.clickOn(".snd-keys .seg-btn", { text: "Tonhöhe" });
  await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).instrument.params.tasten === 1, { what: "tasten = 1" }, id);
  s = await ui();
  assert.equal(s.on, "Tonhöhe");
  assert.equal(s.mode, "pitch");
  assert.equal(s.clapIdle, true, "Clap ruht nach Tonhöhe");
  assert.match(s.hint, /C3/);
  // by pitch: a piano roll around the notes, no drum names
  assert.equal(s.drumKeys, 0);
  assert.ok(s.view.low > 40 && s.view.low <= 60 && s.view.low + s.view.span > 72, `Ansicht Tonhöhe: ${JSON.stringify(s.view)}`);
  // the rest of the sound stays (one parameter changed)
  assert.equal((await track(id)).instrument.kind, "drums");

  // one undo step back to the drum kit – the view follows again
  await app.eval(() => T.st.act(T.backend.undo()));
  await app.waitFor((id) => (T.st.snapshot.peek().project.tracks.find((t) => t.id === id).instrument.params.tasten ?? 0) === 0, { what: "Undo" }, id);
  await app.waitFor(() => T.$(".snd-keys .seg-btn.on")?.textContent === "Schlagzeug", { what: "Schalter nach Undo" });
  s = await ui();
  assert.equal(s.mode, "kit");
  assert.ok(s.view.low <= 36, `Ansicht nach Undo: ${JSON.stringify(s.view)}`);
  assert.deepEqual(app.errors, []);
});

test("Rhythm window offers only drum tracks in the drum kit", async () => {
  const id = await melodyOnDrums();
  const options = async () => {
    await app.eval(() => T.st.showPanel("rhythmus"));
    await app.waitFor(() => T.$(".rh-target option"), { what: "Rhythmus-Ziel" });
    return app.eval(() => T.$$(".rh-target option").map((o) => o.value));
  };
  assert.ok((await options()).includes(String(id)), "Schlagzeug: Ziel im Rhythmus-Fenster");
  await setKeys(id, 1);
  await app.waitFor((id) => !T.$$(".rh-target option").some((o) => o.value === String(id)), { what: "nach Tonhöhe kein Ziel" }, id);
  assert.deepEqual(await options(), ["neu"]);
});
