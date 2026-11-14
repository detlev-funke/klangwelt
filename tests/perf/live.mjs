// How many tracks does the browser version play live at the same time?
// Counterpart to `live_spuren_messen` in crates/klang-app (native).
//
// Note clips are synthesized while playing (voices in the AudioWorklet, one
// sound shaping chain per track). Here the trance song is stacked K times
// (5 tracks each, all notes at the same moments – the hardest case), played
// from its densest part, and the playback rate is measured: 1.0 = the
// worklet keeps time, below that it can't keep up and playback stutters.
//
//   cd ui && npm run build:web && cd .. && node tests/perf/live.mjs
import { startWeb, sleep } from "../web/harness.mjs";

const app = await startWeb();
app.evalTimeout = 600000;
try {
  for (const k of [1, 2, 4, 6, 8, 10, 12, 16]) {
    const r = await app.eval(async (k) => {
      const call = (cmd, args) => T.web.call(cmd, args);
      await call("project_new");
      await call("set_metronome", { enabled: false });
      const plan = (await call("song_templates"))[0];
      const t0 = performance.now();
      for (let i = 0; i < k; i++) await call("song_write", { plan, replace: false });
      const build = performance.now() - t0;
      const tracks = (await call("get_state")).project.tracks.length;
      // the chorus of the song, where all roles play
      await call("seek", { seconds: 100 });
      await T.sleep(300);
      await call("transport", { action: "play" });
      await T.waitFor(() => T.st.status.peek().positionSecs > 100.2, 10000);
      const p0 = T.st.status.peek().positionSecs;
      const t1 = performance.now();
      let peak = 0;
      while (performance.now() - t1 < 4000) {
        await T.sleep(100);
        peak = Math.max(peak, T.st.status.peek().peakOut);
      }
      const rate = (T.st.status.peek().positionSecs - p0) / ((performance.now() - t1) / 1000);
      await call("transport", { action: "stop" });
      return { tracks, build: Math.round(build), rate: Math.round(rate * 1000) / 1000, peak: Math.round(peak * 1000) / 1000 };
    }, k);
    console.log(`${String(r.tracks).padStart(3)} tracks live (generated in ${r.build} ms): playback rate ${r.rate}, level ${r.peak}`);
    await sleep(300);
  }
} finally {
  await app.close();
}
