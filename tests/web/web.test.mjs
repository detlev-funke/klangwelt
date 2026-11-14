// Browser-Version: Klangwelt als statische Dateien (ui/dist-web) in Edge ohne
// Fenster – der echte Kern als WebAssembly im Worker, der Mischer im
// AudioWorklet. Geprüft: Laden, Demo, Wiedergabe mit Pegel (auch unabhängig
// am Ausgang des Worklets gemessen), Bearbeiten und Rückgängig, Speichern und
// Neuladen (OPFS), Export als Download, Analyse/Harmonie/Fuge, Aufnahme mit
// dem Ersatz-Mikrofon des Browsers und Start ohne Server (Service Worker).
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { startWeb, makeTest, sleep } from "./harness.mjs";

let app;
let wavSeconds = 0; // Länge der WAV aus dem Export-Test – Vergleich für Opus
const test = makeTest(() => app);
before(async () => {
  app = await startWeb();
});
after(async () => {
  await app?.close();
});

const snap = () => app.eval(() => T.st.snapshot.peek());

/** Leeres Projekt, Transport am Anfang. */
async function reset() {
  await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    await T.backend.transport("stop");
    await T.backend.transport("rewind");
  });
}

/** Wartet auf eine heruntergeladene Datei; gibt ihren Pfad zurück. */
async function downloaded(name, timeout = 20000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const files = readdirSync(app.downloads);
    if (files.includes(name)) return join(app.downloads, name);
    await sleep(150);
  }
  throw new Error(`Download fehlt: ${name} (da: ${readdirSync(app.downloads).join(", ") || "nichts"})`);
}

test("lädt mit dem echten Kern: Engine im Worklet, Agent erklärt, keine Fehler", async () => {
  const s = await snap();
  assert.equal(await app.eval(() => T.backend.platform), "web");
  assert.equal(await app.eval(() => T.backend.native), true);
  assert.equal(s.engine.available, true, `Engine: ${s.engine.error}`);
  assert.match(s.engine.outputDevice, /^Web Audio \(\d+ Hz\)$/);
  assert.equal(s.agent.url, null);
  assert.match(s.agent.error, /Agent-Server/);
  // Befehl aus dem Kern, den die Attrappe nicht kann: Quintenzirkel aus klang-music
  const view = await app.eval(() => T.backend.harmonyView(null));
  assert.equal(view.circle.length, 36, "Quintenzirkel: drei Ringe zu zwölf Feldern");
  // Bibliothek: ohne eingerichtetes Paket (library.json leer) nur die eigenen Noten
  const lib = await app.waitFor(async () => { const i = await T.backend.libraryInfo(); return i.web?.remote === "none" ? i : null; }, { what: "Bibliothek ohne Paket" });
  assert.equal(lib.id, "browser");
  assert.equal(lib.message, null);
  assert.deepEqual(app.errors, []);
});

test("„Demo laden“ (Klick) legt Beat, Arpeggio und Melodie an", async () => {
  await reset();
  await app.clickOn("kw-timeline .empty-hint button", { text: "Demo laden" });
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.length === 3, { what: "Demo" });
  const s = await snap();
  assert.deepEqual(s.project.tracks.map((t) => t.name), ["Beat", "Arpeggio", "Melodie"]);
  assert.equal(s.undoLabel, "Demo laden");
});

test("Wiedergabe: Position läuft, Pegel kommt aus dem Worklet – auch am Ausgang gemessen", async () => {
  // Zweite, unabhängige Messung: ein Analyser direkt am Ausgang des Worklets
  await app.eval(() => {
    const { ctx, node } = window.__klangweltAudio;
    const a = ctx.createAnalyser();
    a.fftSize = 2048;
    node.connect(a);
    window.__analyser = a;
  });
  await app.key(" ", { code: "Space", vk: 32 });
  await app.waitFor(() => T.st.status.peek().playing, { what: "Wiedergabe" });
  let peak = 0;
  let outPeak = 0;
  for (let i = 0; i < 20; i++) {
    await sleep(100);
    const m = await app.eval(() => {
      const buf = new Float32Array(window.__analyser.fftSize);
      window.__analyser.getFloatTimeDomainData(buf);
      return { peak: T.st.status.peek().peakOut, out: buf.reduce((m, v) => Math.max(m, Math.abs(v)), 0), pos: T.st.status.peek().positionSecs };
    });
    peak = Math.max(peak, m.peak);
    outPeak = Math.max(outPeak, m.out);
  }
  const pos = await app.eval(() => T.st.status.peek().positionSecs);
  assert.ok(pos > 1, `Position nach 2 s: ${pos}`);
  assert.ok(peak > 0.05, `Pegel im Status: ${peak}`);
  assert.ok(outPeak > 0.05, `Pegel am Ausgang des Worklets: ${outPeak}`);
  await app.key(" ", { code: "Space", vk: 32 });
  await app.waitFor(() => !T.st.status.peek().playing, { what: "Pause" });
});

test("Tempo während der Wiedergabe ändern: Worklet spielt weiter, Schlagposition bleibt", async () => {
  await reset();
  // Demo (Audio wird neu gestreckt, Noten neu gerendert), dahinter Noten bis Schlag 28
  await app.eval(async () => {
    await T.st.act(T.backend.createDemo());
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Takte", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    const notes = [0, 1, 2, 3].map((s) => ({ pitch: 60 + s * 2, startBeats: s, lengthBeats: 0.9, velocity: 100 }));
    for (const start of [16, 20, 24]) {
      await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Takt", startBeats: start, source: { type: "notes", lengthBeats: 4, notes } } });
    }
  });
  let bpm = (await snap()).project.tempoBpm;
  // ab Schlag 8: Ginge die Position in Schlägen verloren, spränge sie um mehrere Schläge
  await app.eval(async (bpm) => {
    await T.backend.seek((8 * 60) / bpm);
    await T.backend.transport("play");
  }, bpm);
  await app.waitFor(() => T.st.status.peek().playing, { what: "Wiedergabe" });
  const beat = () => app.eval(() => (T.st.status.peek().positionSecs * T.st.snapshot.peek().project.tempoBpm) / 60);
  /** Größter Pegel über `ms`: im Status und am Ausgang des Worklets. */
  const peaks = async (ms) => {
    let peak = 0;
    let out = 0;
    for (const end = Date.now() + ms; Date.now() < end; ) {
      await sleep(50);
      const m = await app.eval(() => {
        const buf = new Float32Array(window.__analyser.fftSize);
        window.__analyser.getFloatTimeDomainData(buf);
        return { peak: T.st.status.peek().peakOut, out: buf.reduce((m, v) => Math.max(m, Math.abs(v)), 0) };
      });
      peak = Math.max(peak, m.peak);
      out = Math.max(out, m.out);
    }
    return { peak, out };
  };
  await sleep(300);
  let last = { beat: await beat(), t: Date.now() };
  for (const next of [132, 100, 140, 90]) {
    // wie ein Schritt im Tempofeld
    await app.eval((bpm) => {
      const i = T.$("kw-transport input.num");
      i.value = String(bpm);
      i.dispatchEvent(new Event("change"));
    }, next);
    await app.waitFor((bpm) => T.st.snapshot.peek().project.tempoBpm === bpm, { what: `Tempo ${next}` }, next);
    const p = await peaks(300);
    assert.ok(await app.eval(() => T.st.status.peek().playing), `spielt weiter nach ${next} BPM`);
    assert.ok(p.peak > 0.01 && p.out > 0.01, `klingt weiter nach ${next} BPM: Status ${p.peak}, Ausgang ${p.out}`);
    // vorangekommen um die vergangene Zeit, zwischen altem und neuem Tempo
    const now = { beat: await beat(), t: Date.now() };
    const secs = (now.t - last.t) / 1000;
    const [lo, hi] = [(secs * Math.min(bpm, next)) / 60 - 0.5, (secs * Math.max(bpm, next)) / 60 + 0.5];
    const d = now.beat - last.beat;
    assert.ok(d >= lo && d <= hi, `${bpm} → ${next} BPM: +${d.toFixed(2)} Schläge in ${secs.toFixed(2)} s (erwartet ${lo.toFixed(2)} bis ${hi.toFixed(2)})`);
    [last, bpm] = [now, next];
  }
  // die folgenden Tests arbeiten mit dem reinen Demo weiter
  await reset();
  await app.eval(() => T.st.act(T.backend.createDemo()));
});

test("Bearbeiten und Rückgängig: Tempo ändern, Strg+Z, Strg+Y", async () => {
  const before = (await snap()).project.tempoBpm;
  await app.eval(() => T.st.run({ type: "setProject", patch: { tempoBpm: 133 } }));
  assert.equal((await snap()).project.tempoBpm, 133);
  await app.key("z", { code: "KeyZ", vk: 90, mods: 2 });
  await app.waitFor((b) => T.st.snapshot.peek().project.tempoBpm === b, { what: "Rückgängig" }, before);
  await app.key("y", { code: "KeyY", vk: 89, mods: 2 });
  await app.waitFor(() => T.st.snapshot.peek().project.tempoBpm === 133, { what: "Wiederholen" });
});

test("Speichern und Neuladen: das Projekt liegt danach wieder in der Liste und öffnet sich", async () => {
  const saved = await app.eval(async () => {
    await T.st.act(T.backend.projectSave("E2E Lied"));
    await T.web.sync();
    return T.st.snapshot.peek();
  });
  assert.equal(saved.file, "/Klangwelt/Projekte/E2E Lied.klangwelt");
  assert.equal(saved.dirty, false);
  const tracks = saved.project.tracks.map((t) => t.name);
  await app.reload();
  // nach dem Neuladen: leeres Projekt, aber die Datei ist da
  assert.equal((await snap()).project.tracks.length, 0);
  const [dir, list] = await app.eval(() => T.backend.projectList());
  assert.equal(dir, "/Klangwelt/Projekte");
  assert.ok(list.some((p) => p.name === "E2E Lied" && p.bytes > 1000), JSON.stringify(list));
  const opened = await app.eval(async () => {
    await T.st.act(T.backend.projectOpen("E2E Lied"));
    return T.st.snapshot.peek();
  });
  assert.deepEqual(opened.project.tracks.map((t) => t.name), tracks);
  assert.equal(opened.project.tempoBpm, 133);
  // die Aufnahme (Beat) kam mit: Wellenform aus dem gespeicherten Klang
  const beat = opened.project.tracks.find((t) => t.kind === "audio");
  const wave = await app.eval((id) => T.backend.getWaveform(id, 64), beat.clips[0].source.assetId);
  assert.ok(wave.some((v) => v > 0.05), "Aufnahme mitgespeichert");
});

test("Projektdatei herunterladen (Menü) und wieder hochladen", async () => {
  await app.clickOn(".file-controls .btn", { text: "Datei" });
  await app.clickOn(".file-menu .menu-item", { text: "Projektdatei herunterladen" });
  const file = await downloaded("E2E Lied.klangwelt");
  const bytes = readFileSync(file);
  assert.equal(bytes.toString("ascii", 0, 2), "PK", "ZIP-Container");
  // hochladen wie über die Dateiauswahl: unter neuem Namen in den Speicher des Browsers, dann öffnen
  const opened = await app.eval(async (b64) => {
    const data = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const path = await T.web.importFile(new File([data], "Hochgeladen.klangwelt"));
    await (await import("./components/fileops.js")).openPath(path);
    await T.waitFor(() => T.st.snapshot.peek().file === path);
    return T.st.snapshot.peek();
  }, bytes.toString("base64"));
  assert.equal(opened.file, "/Klangwelt/Projekte/Hochgeladen.klangwelt");
  assert.deepEqual(opened.project.tracks.map((t) => t.name), ["Beat", "Arpeggio", "Melodie"]);
});

test("Export: WAV wird heruntergeladen (Kopf, Länge, Pegel)", async () => {
  await app.clickOn(".export-btn");
  await app.waitFor(() => T.$(".export-modal"), { what: "Export-Dialog" });
  // alle Formate wie auf dem Desktop – auch Opus (libopus als WebAssembly)
  assert.deepEqual(await app.eval(() => T.$$(".ex-format .seg-btn").filter((b) => !b.disabled).map((b) => b.dataset.format)), ["wav", "flac", "opus", "stems", "midi"]);
  await app.clickOn(".ex-format [data-format=wav]");
  await app.eval(() => {
    const name = T.$(".ex-name");
    if (name) {
      name.value = "e2e-export";
      name.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
  await app.clickOn(".ex-go");
  await app.waitFor(() => T.$(".ex-reveal"), { timeout: 60000, what: "Export fertig" });
  assert.equal(await app.eval(() => T.$(".ex-reveal").textContent.trim()), "Nochmal herunterladen");
  const name = await app.eval(() => T.$(".ex-done span").textContent.trim());
  const file = await downloaded(name);
  const b = readFileSync(file);
  assert.equal(b.toString("ascii", 0, 4), "RIFF");
  assert.equal(b.toString("ascii", 8, 12), "WAVE");
  assert.equal(b.readUInt16LE(22), 2, "Stereo");
  assert.equal(b.readUInt32LE(24), 48000);
  const bits = b.readUInt16LE(34);
  const frames = b.readUInt32LE(40) / (2 * (bits / 8));
  assert.ok(frames > 48000 * 4, `Länge: ${frames} Frames`);
  wavSeconds = frames / 48000;
  let peak = 0;
  for (let i = 44; i + 2 <= b.length; i += 2 * 97) peak = Math.max(peak, Math.abs(bits === 24 ? b.readIntLE(i, 3) / 8388608 : b.readInt16LE(i) / 32768));
  assert.ok(peak > 0.05, `Pegel im Export: ${peak}`);
  await app.key("Escape", { code: "Escape", vk: 27 });
});

test("Export: Opus (libopus als WebAssembly) – der Browser dekodiert die Datei, Länge passt zur WAV", async () => {
  await app.clickOn(".export-btn");
  await app.waitFor(() => T.$(".export-modal"), { what: "Export-Dialog" });
  await app.clickOn(".ex-format [data-format=opus]");
  // Bitrate wählen wie im Dialog: 96 kbit/s
  await app.eval(() => {
    const rate = T.$(".ex-bitrate");
    rate.value = "96";
    rate.dispatchEvent(new Event("change", { bubbles: true }));
    const name = T.$(".ex-name");
    name.value = "e2e-opus";
    name.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await app.clickOn(".ex-go");
  await app.waitFor(() => T.$(".ex-reveal"), { timeout: 60000, what: "Opus-Export fertig" });
  const file = await downloaded("e2e-opus.opus");
  const o = readFileSync(file);
  assert.equal(o.toString("ascii", 0, 4), "OggS");
  assert.ok(o.includes(Buffer.from("OpusHead")) && o.includes(Buffer.from("OpusTags")));
  assert.ok(o.includes(Buffer.from("libopus")), "Kennung der eingebauten libopus in den Kommentaren");
  // 96 kbit/s ≈ 12 kB je Sekunde (variable Rate: großzügig) – die Bitrate kam also an
  assert.ok(o.length > wavSeconds * 12000 * 0.4 && o.length < wavSeconds * 12000 * 2 + 4000, `${o.length} Bytes für ${wavSeconds} s`);
  const d = await app.eval(async (b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const buf = await new OfflineAudioContext(2, 48000, 48000).decodeAudioData(bytes.buffer);
    let peak = 0;
    const ch = buf.getChannelData(0);
    for (let i = 0; i < ch.length; i += 7) peak = Math.max(peak, Math.abs(ch[i]));
    return { channels: buf.numberOfChannels, duration: buf.duration, peak };
  }, o.toString("base64"));
  assert.equal(d.channels, 2);
  assert.ok(Math.abs(d.duration - wavSeconds) < 0.05, `Opus ${d.duration} s, WAV ${wavSeconds} s`);
  assert.ok(d.peak > 0.05, `hörbar: ${d.peak}`);
  await app.key("Escape", { code: "Escape", vk: 27 });
});

test("Analyse, Harmonie, Fuge und Kontrapunkt rechnen im Browser", async () => {
  const s = await snap();
  const beat = s.project.tracks.find((t) => t.kind === "audio");
  const analysis = await app.eval((id) => T.backend.analyzeAsset(id), beat.clips[0].source.assetId);
  assert.ok(analysis.tempoEstimate > 60 && analysis.tempoEstimate < 200, `Tempo: ${analysis.tempoEstimate}`);
  assert.ok(analysis.onsets.length > 4, `Einsätze: ${analysis.onsets.length}`);
  const line = await app.eval(() => T.backend.harmonyTimeline(null));
  assert.equal(line.key.name, "a-Moll");
  assert.ok(line.chords.length >= 4, `Akkorde: ${line.chords.length}`);
  const sugg = await app.eval((k, c) => T.backend.harmonySuggest(k, c), line.key, line.chords.slice(0, 2).map((c) => c.chord));
  assert.ok(sugg.suggestions.length > 0, "Vorschläge ohne Bibliothek");
  const mel = s.project.tracks.find((t) => t.name === "Melodie");
  const subject = await app.eval((t, c) => T.backend.fugueSubject(t, c, null), mel.id, mel.clips[0].id);
  assert.ok(subject.notes.length >= 8, `Thema: ${subject.notes.length} Noten`);
  const cs = await app.eval((t, c, k) => T.backend.fugueCountersubjects(t, c, k), mel.id, mel.clips[0].id, subject.key);
  assert.ok(cs.length > 0, "Gegensätze");
  const check = await app.eval(() => T.backend.counterpointCheck(null));
  assert.ok(Array.isArray(check.issues));
});

test("Aufnahme mit dem Mikrofon des Browsers (Ersatzgerät) wird eine Audiospur", async () => {
  await reset();
  await app.eval(() => T.backend.recordStart());
  await app.waitFor(() => T.st.status.peek().recording, { what: "Aufnahme läuft" });
  await sleep(1800);
  const r = await app.eval(() => T.backend.recordStop());
  const t = r.snapshot.project.tracks.find((x) => x.id === r.trackId);
  assert.equal(t.kind, "audio");
  assert.equal(t.name, "Aufnahme 1");
  assert.ok(t.clips[0].source.durationSecs > 1, `Dauer: ${t.clips[0].source.durationSecs}`);
  assert.ok(r.analysis.durationSecs > 1);
});

test("Audiodatei ins Fenster ziehen: landet im Dateibaum des Kerns und wird eine Audiospur", async () => {
  await reset();
  const before = (await snap()).project.tracks.length;
  // eine WAV (1 s, 440 Hz, 16 Bit mono) wie vom Rechner hereingezogen: echtes drop-Ereignis mit DataTransfer
  await app.eval(() => {
    const sr = 44100;
    const n = sr;
    const buf = new ArrayBuffer(44 + 2 * n);
    const v = new DataView(buf);
    const tag = (o, s) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
    tag(0, "RIFF");
    v.setUint32(4, 36 + 2 * n, true);
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
    v.setUint32(40, 2 * n, true);
    for (let i = 0; i < n; i++) v.setInt16(44 + 2 * i, Math.round(12000 * Math.sin((2 * Math.PI * 440 * i) / sr)), true);
    const dt = new DataTransfer();
    dt.items.add(new File([buf], "Sinus 440.wav", { type: "audio/wav" }));
    window.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, clientX: 5, clientY: 5, bubbles: true, cancelable: true }));
  });
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.some((t) => t.kind === "audio"), { timeout: 15000, what: "neue Audiospur" });
  const s = await snap();
  const t = s.project.tracks[s.project.tracks.length - 1];
  assert.equal(t.kind, "audio");
  assert.ok(Math.abs(t.clips[0].source.durationSecs - 1) < 0.05, `Dauer: ${t.clips[0].source.durationSecs}`);
  // die Datei liegt im Dateibaum des Kerns (Ordner „Import“) – über den Pfad lässt sie sich
  // noch einmal einfügen, wie auf dem Desktop (Oberfläche und Agent nutzen denselben Befehl)
  const again = await app.eval(() => T.backend.audioImport(["/Klangwelt/Import/Sinus 440.wav"], null, 4));
  assert.equal(again.imported.length, 1, JSON.stringify(again.errors));
  assert.equal(again.snapshot.project.tracks.length, before + 2);
});

test("ohne Server: der Service Worker liefert die App aus dem Cache", async () => {
  await app.waitFor(() => !!navigator.serviceWorker.controller, { timeout: 15000, what: "Service Worker aktiv" });
  // ungespeicherte Änderungen (die Aufnahme): der Browser fragt beim Verlassen nach
  assert.equal((await app.eval(() => T.backend.getState())).dirty, true);
  app.stopServer();
  await sleep(300);
  await app.reload();
  assert.deepEqual(app.dialogs, ["beforeunload"], "Rückfrage „Seite verlassen?“");
  assert.equal((await snap()).engine.available, true);
  const list = await app.eval(() => T.backend.projectList());
  assert.ok(list[1].some((p) => p.name === "E2E Lied"), "gespeichertes Projekt auch offline");
  await app.startServer();
  assert.ok(existsSync(app.downloads));
  assert.deepEqual(app.errors, []);
});
