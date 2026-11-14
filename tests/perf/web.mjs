// Leistungsmessung der Browser-Version (ui/dist-web in Edge ohne Fenster) –
// dieselben Abläufe wie tests/perf/desktop.mjs, zum Vergleich der Plattformen.
// Kein Test mit Grenzwerten, sondern eine Tabelle mit Medianen.
//
//   cd ui && npm run build:web && cd .. && node tests/perf/web.mjs [Durchläufe]
import { startWeb } from "../web/harness.mjs";

const runs = Number(process.argv[2] ?? 3);
const app = await startWeb();
// Ohne Threads rendert ein ganzer Song im Browser deutlich länger
app.evalTimeout = 600000;
const result = {};

async function measure(name, body, { n = runs, prep = "" } = {}) {
  const xs = [];
  for (let i = 0; i < n; i++) {
    xs.push(
      await app.eval(`(async () => {
        const call = (cmd, args) => T.web.call(cmd, args);
        ${prep}
        const t0 = performance.now();
        ${body}
        return performance.now() - t0;
      })()`),
    );
  }
  xs.sort((a, b) => a - b);
  result[name] = Math.round(xs[Math.floor(xs.length / 2)] * 100) / 100;
}

const run = (code) => app.eval(`(async () => { const call = (cmd, args) => T.web.call(cmd, args); ${code} })()`);

try {
  await run(`await call("project_new"); await call("set_metronome", { enabled: false });`);
  await measure("get_state (leer) ×100", `for (let i = 0; i < 100; i++) await call("get_state");`);
  await run(`await call("create_demo");`);
  await measure("get_state (Demo) ×100", `for (let i = 0; i < 100; i++) await call("get_state");`);
  const tid = await run(`return (await call("get_state")).project.tracks.find((t) => t.kind === "instrument").id;`);
  await measure(
    "execute Lautstärke (merge) ×50",
    `for (let i = 0; i < 50; i++) await call("execute", { command: { type: "setTrack", trackId: ${tid}, patch: { gainDb: -(i % 6) } }, merge: i > 0 });`,
  );
  await measure("undo+redo ×20", `for (let i = 0; i < 20; i++) { await call("undo"); await call("redo"); }`);
  await measure("create_demo", `await call("create_demo");`, { prep: `await call("project_new");` });
  await measure("song_write (Trance)", `await call("song_write", { plan, replace: false });`, { prep: `await call("project_new"); const plan = (await call("song_templates"))[0];` });
  await measure("harmony_timeline (Song)", `await call("harmony_timeline", { key: null });`);
  // Export samt Download (der Kern rendert, die Seite lädt die Datei herunter)
  await measure("export WAV 16 Bit (Song)", `await call("export_run", { options: { format: "wav", bits: 16, name: "leistung" } });`, { n: Math.min(runs, 3) });
  const width = Math.max(...Object.keys(result).map((k) => k.length));
  console.log(`Browser, Median über ${runs} Durchläufe (ms):`);
  for (const [k, v] of Object.entries(result)) console.log(`  ${k.padEnd(width)}  ${String(v).padStart(9)}`);
  console.log(JSON.stringify(result));
} finally {
  await app.close();
}
