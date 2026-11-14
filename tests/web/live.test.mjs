// Live playback in the browser: note clips are synthesized in the AudioWorklet
// while playing. The worklet reports how much of its real-time budget it needs;
// when it can't keep up, the core pre-renders the clips around the playback
// position step by step, and they play from their buffers.
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

/** Generates the first song template and starts playing it. */
async function playSong() {
  await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    const plans = await T.backend.songTemplates();
    await T.backend.songWrite(plans[0], true);
    await T.st.act(T.backend.getState());
    await T.backend.transport("play");
  });
  await app.waitFor(() => T.st.status.peek().playing, { what: "playing" });
}

/** Output level of the worklet since the last status. */
const level = () => app.eval(() => T.st.status.peek().peakOut);

test("The worklet reports how much of its budget live playback needs", async () => {
  await playSong();
  await sleep(1500);
  const load = await app.eval(() => T.st.status.peek().load);
  assert.ok(load > 0 && load < 1, `load while playing a song: ${load}`);
  assert.equal(await app.eval(() => T.st.status.peek().prerendered), 0, "nothing pre-rendered while it keeps up");
  await app.eval(() => T.backend.transport("stop"));
});

test("When live playback can't keep up, clips are pre-rendered and play on; a new tempo plays them live again", async () => {
  await playSong();
  // as if the worklet needed 95 % of its budget
  await app.eval(() => T.web.call("simulate_load", { load: 0.95 }));
  await app.waitFor(() => T.st.status.peek().prerendered >= 2, { timeout: 30000, what: "two clips pre-rendered" });
  assert.ok(await app.eval(() => T.st.status.peek().playing), "keeps playing");
  let peak = 0;
  for (let i = 0; i < 6; i++) {
    await sleep(100);
    peak = Math.max(peak, await level());
  }
  assert.ok(peak > 0.005, `sounds: ${peak}`);
  // relieved: no further clips (one already being rendered may still finish)
  await app.eval(() => T.web.call("simulate_load", { load: 0 }));
  await sleep(500);
  const settled = await app.eval(() => T.st.status.peek().prerendered);
  await sleep(1500);
  assert.ok((await app.eval(() => T.st.status.peek().prerendered)) <= settled, "no more pre-rendering once relieved");
  // a new tempo: the buffers belong to the old one, the clips play live again
  await app.eval(async () => {
    const bpm = T.st.snapshot.peek().project.tempoBpm;
    await T.st.run({ type: "setProject", patch: { tempoBpm: bpm + 6 } });
  });
  await app.waitFor(() => T.st.status.peek().prerendered === 0, { timeout: 5000, what: "live again" });
  assert.ok(await app.eval(() => T.st.status.peek().playing));
  await app.eval(() => T.backend.transport("stop"));
});
