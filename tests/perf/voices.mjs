// How many clips (tracks, voices) does the browser version mix in real time?
// Counterpart to `stimmen_in_echtzeit` in crates/klang-engine (native).
//
// Klangwelt pre-renders notes; in real time the mixer only adds up the
// finished clips (with pan, stereo width, reverb amount, limiter).
//
// 1. Compute time: the same mixer as in the AudioWorklet (klang_worklet.wasm),
//    measured here on a page thread – N simultaneous clips with a side
//    signal and reverb amount, blocks of 128 frames, budget 2.67 ms.
// 2. Test in the real AudioWorklet: play K tracks from the core and measure
//    the playback rate (1.0 = the beat is held); the browser only counts
//    dropouts if it knows AudioContext.playoutStats.
//
//   cd ui && npm run build:web && cd .. && node tests/perf/voices.mjs
import { startWeb, sleep } from "../web/harness.mjs";

const app = await startWeb();
app.evalTimeout = 600000;
try {
  const rows = await app.eval(async () => {
    const bytes = await (await fetch("klang_worklet.wasm")).arrayBuffer();
    const w = (await WebAssembly.instantiate(bytes, {})).instance.exports;
    const BLOCK = 128;
    const budget = (BLOCK / 48000) * 1e6;
    w.kwl_init(48000);
    // 16 buffers of 4 s (mid + side), identifiers 1…32
    for (let id = 1; id <= 32; id++) {
      const len = 48000 * 4;
      const ptr = w.kwl_buffer_alloc(id, len);
      const view = new Float32Array(w.memory.buffer, ptr, len);
      for (let i = 0; i < len; i++) view[i] = (Math.random() - 0.5) * 0.01;
      w.kwl_buffer_done(id);
    }
    const msg = (kind, nums) => {
      const p = w.kwl_msg_buf(nums.length);
      new Float64Array(w.memory.buffer, p, nums.length).set(nums);
      w.kwl_msg(kind, nums.length);
    };
    msg(1, []); // Play
    const out = [];
    for (const n of [1, 8, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192]) {
      const clips = [];
      for (let i = 0; i < n; i++) {
        const b = (i % 16) * 2 + 1;
        clips.push(b, b + 1, i % 97, 0.8, 0.7, 0.3);
      }
      msg(8, clips); // Arrangement
      msg(3, [0]); // to the start
      for (let i = 0; i < 50; i++) w.kwl_process(BLOCK);
      const blocks = 1000;
      const t0 = performance.now();
      for (let i = 0; i < blocks; i++) w.kwl_process(BLOCK);
      const us = ((performance.now() - t0) * 1000) / blocks;
      out.push({ n, us: Math.round(us * 10) / 10, anteil: Math.round((us / budget) * 1000) / 10 });
    }
    return out;
  });
  console.log("Browser (WebAssembly, ein Thread der Seite) – Clips   µs/Block   Anteil am Budget");
  for (const r of rows) console.log(`  ${String(r.n).padStart(5)}  ${String(r.us).padStart(9)}   ${String(r.anteil).padStart(6)} %`);
  const ok = rows.filter((r) => r.anteil < 50).pop();
  console.log(`Bis ${ok?.n ?? 0} gleichzeitige Clips bleibt das Mischen unter der Hälfte des Echtzeit-Budgets.`);

  // 2. Test in the real worklet: K tracks from the core (six sounds, each a
  //    4-bar pattern; all created in one step), play and measure the
  //    playback rate.
  const stats = await app.eval(() => !!window.__klangweltAudio.ctx && "playoutStats" in window.__klangweltAudio.ctx);
  for (const k of [16, 64, 256, 1024]) {
    const r = await app.eval(async (k) => {
      const call = (cmd, args) => T.web.call(cmd, args);
      await call("project_new");
      await call("set_metronome", { enabled: false });
      const kinds = ["pluck", "bass", "synth", "organ", "drums", "harpsichord"];
      for (const kind of kinds) await call("create_instrument_track", { kind });
      const base = (await call("get_state")).project.tracks;
      const t0 = performance.now();
      const commands = [];
      for (let i = kinds.length; i < k; i++) {
        const t = base[i % kinds.length];
        commands.push({ type: "addTrack", track: { ...t, id: 0, name: `${t.name} ${i}`, clips: t.clips.map((c) => ({ ...c, id: 0 })) } });
      }
      if (commands.length) await call("execute", { command: { type: "batch", label: "Viele Spuren", commands } });
      const build = performance.now() - t0;
      const tracks = (await call("get_state")).project.tracks.length;
      await call("transport", { action: "rewind" });
      await T.sleep(300);
      const ctx = window.__klangweltAudio.ctx;
      const st0 = ctx.playoutStats?.toJSON?.() ?? null;
      await call("transport", { action: "play" });
      await T.waitFor(() => T.st.status.peek().positionSecs > 0.2, 10000);
      const p0 = T.st.status.peek().positionSecs;
      const t1 = performance.now();
      let peak = 0;
      while (performance.now() - t1 < 3000) {
        await T.sleep(100);
        peak = Math.max(peak, T.st.status.peek().peakOut);
      }
      const rate = (T.st.status.peek().positionSecs - p0) / ((performance.now() - t1) / 1000);
      await call("transport", { action: "stop" });
      const st1 = ctx.playoutStats?.toJSON?.() ?? null;
      return { tracks, build: Math.round(build), rate: Math.round(rate * 100) / 100, peak: Math.round(peak * 1000) / 1000, st0, st1 };
    }, k);
    const glitch = r.st0 && r.st1 ? `, Aussetzer laut Browser: ${(r.st1.fallbackFramesEvents ?? 0) - (r.st0.fallbackFramesEvents ?? 0)}` : "";
    console.log(`Worklet mit ${String(r.tracks).padStart(4)} Spuren (angelegt in ${r.build} ms): Abspielrate ${r.rate}, Pegel ${r.peak}${glitch}`);
    await sleep(200);
  }
  if (!stats) console.log("(AudioContext.playoutStats gibt es in diesem Browser nicht – die Abspielrate zeigt, ob das Worklet den Takt hält.)");
} finally {
  await app.close();
}
