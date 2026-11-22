// The Sound window while the song plays (ui/src/components/sound.ts): a change is heard on
// the playing track itself – no test sound on top. Stopped, a change plays the short test
// sound as before.
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

const previews = () => app.eval(() => window.__previews);
/** Clicks a starting point in the first field of the Sound window (not the one it is at). */
const pickMark = () =>
  app.eval(() => {
    const marks = T.$$(".snd-field .snd-mark");
    const next = marks[(window.__mark = ((window.__mark ?? 0) + 1) % marks.length)];
    next.click();
  });
const instrument = () => app.eval(() => JSON.stringify(T.st.snapshot.peek().project.tracks[0].instrument));

test("while playing, a change in the Sound window changes the playing track – no test sound", async () => {
  await app.eval(async () => {
    const notes = Array.from({ length: 16 }, (_, i) => ({ pitch: 60 + (i % 8), startBeats: i, lengthBeats: 0.9, velocity: 90 }));
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Lead", kind: "instrument", clips: [{ id: 0, name: "Thema", startBeats: 0, source: { type: "notes", lengthBeats: 16, notes } }] } });
    T.st.selection.value = { trackId: T.st.snapshot.peek().project.tracks[0].id, clipId: null };
    T.st.showPanel("klang");
    // count the test sounds the window asks for
    const { backend } = await import("./bridge.js");
    window.__previews = 0;
    const play = backend.previewTrack;
    backend.previewTrack = (...a) => {
      window.__previews++;
      return play(...a);
    };
  });
  await app.waitFor(() => T.$$(".snd-field .snd-mark").length > 1, { what: "sound window" });

  // stopped: the short test sound, as before
  await pickMark();
  await app.waitFor(() => window.__previews === 1, { what: "test sound when stopped" });

  await app.eval(() => T.backend.transport("play"));
  await app.waitFor(() => T.st.status.peek().playing, { what: "playing" });
  const before = await instrument();
  await pickMark();
  await app.waitFor((b) => JSON.stringify(T.st.snapshot.peek().project.tracks[0].instrument) !== b, { what: "the track's sound changed" }, before);
  await sleep(400);
  assert.equal(await previews(), 1, "no test sound while playing");
  assert.equal(await app.eval(() => T.st.status.peek().playing), true, "still playing");

  await app.eval(() => T.backend.transport("stop"));
  await app.waitFor(() => !T.st.status.peek().playing, { what: "stopped" });
  await pickMark();
  await app.waitFor(() => window.__previews === 2, { what: "test sound again once stopped" });
  assert.deepEqual(app.errors, []);
});
