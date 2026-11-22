// Screenshots for the changelog: one picture per bigger entry, taken from the English
// browser version in a headless, muted Edge (same harness as the browser tests) and
// written to changelog/images/<id>.webp, where <id> is the changelog entry's id.
//
//   node tests/web/changelog-shots.mjs [id ...]      (no ids: every shot)
//
// Needs a current ui/dist-web (npm run build:web in ui/). Every shot starts its own
// browser with a fresh temporary profile, so the pictures don't depend on each other;
// notes (toasts), hover states and the text cursor are hidden. Nothing goes to the
// network: the harness serves the app locally and offers no score library.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT, sleep, startWeb } from "./harness.mjs";
import { makeTempDir, removeTempDir } from "../tempdirs.mjs";

const OUT = join(ROOT, "changelog", "images");
const LANGUAGES = "en-US,en";
/** Largest file we aim for; the quality steps down from 80 until a picture fits. */
const MAX_BYTES = 250 * 1024;

// ------------------------------------------------------------------ helpers

/** Emulated window: double pixel density, optionally a phone (touch, mobile). */
async function viewport(app, width, height, { mobile = false } = {}) {
  await app.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 2, mobile });
  await app.send("Emulation.setTouchEmulationEnabled", { enabled: mobile, maxTouchPoints: mobile ? 5 : 1 });
  app.size = { width, height };
  // the toolbar and the dock fit themselves in the next frames
  await sleep(400);
}

/** Hides what doesn't belong in a picture: notes (toasts), the text cursor, the mouse's hover. */
async function calm(app, park = { x: 1, y: 1 }) {
  await app.eval(() => {
    if (!document.getElementById("shot-calm")) {
      const s = document.createElement("style");
      s.id = "shot-calm";
      s.textContent = "kw-toast { display: none !important; } * { caret-color: transparent !important; }";
      document.head.append(s);
    }
    T.st.message.value = null;
    document.activeElement?.blur?.();
  });
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: park.x, y: park.y });
  await sleep(300);
}

/** Width and height of a WebP file (VP8, VP8L or VP8X). */
function webpSize(b) {
  const kind = b.toString("ascii", 12, 16);
  if (kind === "VP8 ") return { width: b.readUInt16LE(26) & 0x3fff, height: b.readUInt16LE(28) & 0x3fff };
  if (kind === "VP8L") {
    const v = b.readUInt32LE(21);
    return { width: (v & 0x3fff) + 1, height: ((v >> 14) & 0x3fff) + 1 };
  }
  return { width: b.readUIntLE(24, 3) + 1, height: b.readUIntLE(27, 3) + 1 };
}

/** Bounding box (CSS px) of the elements matching the selectors, with a margin, inside the window. */
async function box(app, selectors, { margin = 0 } = {}) {
  const r = await app.eval((list) => {
    const rects = list.flatMap((s) => [...document.querySelectorAll(s)]).map((e) => e.getBoundingClientRect()).filter((r) => r.width && r.height);
    if (!rects.length) return null;
    const x = Math.min(...rects.map((r) => r.left));
    const y = Math.min(...rects.map((r) => r.top));
    return { x, y, right: Math.max(...rects.map((r) => r.right)), bottom: Math.max(...rects.map((r) => r.bottom)) };
  }, [selectors].flat());
  if (!r) throw new Error(`not found: ${selectors}`);
  const { width, height } = app.size;
  const x = Math.max(0, Math.floor(r.x - margin));
  const y = Math.max(0, Math.floor(r.y - margin));
  return { x, y, width: Math.min(width, Math.ceil(r.right + margin)) - x, height: Math.min(height, Math.ceil(r.bottom + margin)) - y };
}

/** Writes changelog/images/<id>.webp – of `clip` (CSS px) or the whole window. */
async function shoot(app, id, clip, { park } = {}) {
  await calm(app, park);
  let quality = 80;
  let data;
  for (;;) {
    const r = await app.send("Page.captureScreenshot", { format: "webp", quality, ...(clip ? { clip: { ...clip, scale: 1 } } : {}) });
    data = Buffer.from(r.data, "base64");
    if (data.length <= MAX_BYTES || quality <= 50) break;
    quality -= 10;
  }
  const file = join(OUT, `${id}.webp`);
  writeFileSync(file, data);
  const { width, height } = webpSize(data);
  console.log(`${id}.webp  ${width}×${height} px  ${Math.round(data.length / 1024)} KB  (quality ${quality})`);
}

/** Drags a dock gutter (the line between two windows) by dx/dy with the real mouse. */
async function dragGutter(app, selector, { dx = 0, dy = 0 }) {
  const g = await app.eval((s) => {
    const b = document.querySelector(s)?.getBoundingClientRect();
    return b && { x: b.x + b.width / 2, y: b.y + b.height / 2 };
  }, selector);
  if (!g) throw new Error(`no gutter: ${selector}`);
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: g.x, y: g.y });
  await app.send("Input.dispatchMouseEvent", { type: "mousePressed", x: g.x, y: g.y, button: "left", buttons: 1, clickCount: 1 });
  for (let i = 1; i <= 5; i++) await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: g.x + (dx * i) / 5, y: g.y + (dy * i) / 5, button: "left", buttons: 1 });
  await app.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: g.x + dx, y: g.y + dy, button: "left", buttons: 0, clickCount: 1 });
  await sleep(300);
}

/**
 * Resizes the docked window group (tabs and window) holding the element `inside` to `width` and/or
 * `height` CSS px by dragging the gutters next to it, as a user would.
 */
async function windowSize(app, inside, { width, height }) {
  for (const [dir, want] of [["row", width], ["col", height]]) {
    if (want == null) continue;
    for (let i = 0; i < 3; i++) {
      // the stack's size and the nearest gutter of this direction (before or after it)
      const g = await app.eval((p, dir) => {
        const stack = document.querySelector(p)?.closest(".dk-stack");
        if (!stack) return null;
        const r = stack.getBoundingClientRect();
        for (let el = stack; el?.parentElement; el = el.parentElement) {
          if (!el.parentElement.classList.contains("dk-split") || !el.parentElement.classList.contains(dir)) continue;
          const before = el.previousElementSibling?.classList.contains("dk-gutter");
          const after = el.nextElementSibling?.classList.contains("dk-gutter");
          if (!before && !after) continue;
          const gutter = after ? el.nextElementSibling : el.previousElementSibling;
          gutter.dataset.shotGutter = "1";
          return { now: dir === "row" ? r.width : r.height, sign: after ? 1 : -1 };
        }
        return null;
      }, inside, dir);
      if (!g) throw new Error(`no gutter next to ${inside} (${dir})`);
      const d = Math.round(want - g.now) * g.sign;
      if (Math.abs(d) >= 1) await dragGutter(app, "[data-shot-gutter]", dir === "row" ? { dx: d } : { dy: d });
      await app.eval(() => document.querySelector("[data-shot-gutter]")?.removeAttribute("data-shot-gutter"));
      if (Math.abs(d) < 2) break;
    }
  }
}

/**
 * Sizes the window group holding `inside` so that the scrolling element `scroller` in it
 * shows all of `content` (its child) – no scrollbars, no empty space below.
 */
async function fitContent(app, inside, scroller, content, { maxWidth = 1100, maxHeight = 700 } = {}) {
  const need = () =>
    app.eval((p, s, c) => {
      const stack = document.querySelector(p).closest(".dk-stack").getBoundingClientRect();
      const el = document.querySelector(s);
      return { w: stack.width + el.scrollWidth - el.clientWidth, h: stack.height + document.querySelector(c).offsetHeight - el.clientHeight };
    }, inside, scroller, content);
  // tall first (no vertical scrollbar), then the width, then the height without a horizontal one
  await windowSize(app, inside, { height: maxHeight });
  await windowSize(app, inside, { width: Math.min(maxWidth, Math.ceil((await need()).w) + 1) });
  await windowSize(app, inside, { height: Math.min(maxHeight, Math.ceil((await need()).h) + 1) });
}

/**
 * The core's wall clock (it runs in a web worker) on fixed times, so the History window
 * shows the same times on every run. Every new worker waits until its Date.now is moved
 * to today 14:02, then the page reloads so the core starts on that clock. `at(secs)`
 * moves it forward (never back) to that many seconds after 14:02 – the time of the next
 * step. The clock runs on at its normal rate in between.
 */
async function fixedClock(app) {
  const start = await app.eval(() => new Date().setHours(14, 2, 0, 0));
  let next = 1e9;
  const raw = (sessionId, method, params = {}) =>
    new Promise((res, rej) => {
      const id = ++next;
      app.page.pending.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result)));
      app.page.ws.send(JSON.stringify({ id, sessionId, method, params }));
    });
  const workers = new Set();
  const patch = `(() => {
    const real = Date.now.bind(Date);
    let off = ${start} - real();
    Date.now = () => real() + off;
    self.__shotClock = (ms) => { off = Math.max(off, ms - real()); };
  })()`;
  app.page.handlers.push((m) => {
    if (m.method === "Target.detachedFromTarget") workers.delete(m.params.sessionId);
    if (m.method !== "Target.attachedToTarget") return;
    const { sessionId, targetInfo, waitingForDebugger } = m.params;
    void (async () => {
      if (waitingForDebugger && targetInfo.type === "worker") {
        await raw(sessionId, "Runtime.evaluate", { expression: patch });
        workers.add(sessionId);
      }
      if (waitingForDebugger) await raw(sessionId, "Runtime.runIfWaitingForDebugger");
    })().catch((e) => console.error(`clock: ${e.message}`));
  });
  await app.send("Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: true, flatten: true });
  await app.reload();
  if (!workers.size) throw new Error("clock: no worker attached");
  return {
    start,
    /** Clock to `start` + `secs` seconds. */
    async at(secs) {
      for (const s of workers) await raw(s, "Runtime.evaluate", { expression: `self.__shotClock(${start + secs * 1000})` });
    },
  };
}

/** Mono 16-bit WAV. */
function wav(samples, sr) {
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
  return Buffer.concat([head, data]);
}

/**
 * A three-voice chorale in C major (soprano, alto, bass; 4 bars at 100 BPM), every voice
 * a harmonic tone at the same level – like the recording in voices.test.mjs, a little longer.
 */
function chorale() {
  const SR = 44_100;
  const BEAT = 0.6;
  // (MIDI, beat, beats) per voice
  const voices = [
    [[76, 0, 1], [74, 1, 1], [72, 2, 1], [74, 3, 1], [76, 4, 1], [76, 5, 1], [76, 6, 2], [74, 8, 1], [74, 9, 1], [74, 10, 1], [71, 11, 1], [72, 12, 4]],
    [[67, 0, 2], [65, 2, 2], [67, 4, 2], [64, 6, 2], [65, 8, 2], [67, 10, 2], [64, 12, 4]],
    [[48, 0, 2], [41, 2, 2], [48, 4, 2], [45, 6, 2], [41, 8, 2], [43, 10, 2], [48, 12, 2], [36, 14, 2]],
  ];
  const out = new Float32Array(Math.round((16 * BEAT + 0.4) * SR));
  for (const voice of voices) {
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
  return wav(out, SR);
}

/**
 * A sung line that pitch detection breaks into fragments (as in smoothing.test.mjs): c' with
 * an octave slip, e' with a breathy voice break, a scoop d' → g', d' d' re-struck after a
 * breath, c' – two beats each at 120 BPM.
 */
function brokenLine() {
  const parts = [
    [60, 0.44, 0], [72, 0.13, 0], [60, 0.39, 0], [0, 0.04, 2],
    [64, 0.47, 0], [64, 0.05, 1], [64, 0.44, 0], [0, 0.04, 2],
    [62, 0.11, 0], [67, 0.85, 0], [0, 0.04, 2],
    [62, 0.44, 0], [0, 0.06, 2], [62, 0.44, 0], [0, 0.06, 2],
    [60, 0.9, 0], [0, 0.1, 2],
  ];
  const sr = 44100;
  const x = [];
  let phase = 0;
  let noise = 0x2545f491;
  let since = 0;
  for (const [midi, secs, kind] of parts) {
    const f = 440 * 2 ** ((midi - 69) / 12);
    for (let i = 0; i < Math.floor(secs * sr); i++) {
      const env = Math.min(1, since / 0.015);
      since += 1 / sr;
      if (kind === 0) {
        phase += (2 * Math.PI * f) / sr;
        x.push(env * (0.4 * Math.sin(phase) + 0.15 * Math.sin(2 * phase) + 0.06 * Math.sin(3 * phase)));
      } else if (kind === 1) {
        noise = (noise ^ (noise << 13)) >>> 0;
        noise = (noise ^ (noise >>> 17)) >>> 0;
        noise = (noise ^ (noise << 5)) >>> 0;
        x.push(0.3 * env * ((noise / 4294967295) * 2 - 1) * 1.7);
      } else {
        since = 0;
        x.push(0);
      }
    }
  }
  return wav(x, sr);
}

/**
 * What the substitute microphone plays for the live analysis (liveanalysis.test.mjs): a
 * C major melody, one plucked note every 0.5 s (120 BPM), with a staccato C5, a
 * sforzato A4 and two soft notes; 6 s, it loops seamlessly.
 */
function liveMelody() {
  const sr = 48000;
  const melody = [60, 64, 67, { midi: 72, secs: 0.14 }, 67, 64, 65, { midi: 69, accent: 4 }, 67, { midi: 62, amp: 0.25 }, { midi: 59, amp: 0.25 }, 60];
  const out = [];
  for (const note of melody) {
    const { midi, amp = 1, accent = 1, secs = 0.46 } = typeof note === "number" ? { midi: note } : note;
    const f = 440 * 2 ** ((midi - 69) / 12);
    for (let i = 0; i < Math.round(0.5 * sr); i++) {
      const t = i / sr;
      const gain = amp * (1 + (accent - 1) * Math.exp(-t / 0.03));
      const env = Math.min(1, t / 0.005) * Math.exp(-t * 2) * Math.max(0, Math.min(1, (secs - t) / 0.02));
      const w = 2 * Math.PI * f * t;
      out.push(gain * env * (0.15 * Math.sin(w) + 0.056 * Math.sin(2 * w) + 0.022 * Math.sin(3 * w)));
    }
  }
  return wav(out, sr);
}

/** Drops a WAV file into the window (as from the file manager); returns the new audio track and clip. */
async function dropWav(app, name, bytes) {
  await app.eval((name, b64) => {
    const data = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([data], name, { type: "audio/wav" }));
    window.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, clientX: 5, clientY: 5, bubbles: true, cancelable: true }));
  }, name, bytes.toString("base64"));
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.some((t) => t.kind === "audio"), { timeout: 15000, what: "audio track" });
  return app.eval(() => {
    const t = T.st.snapshot.peek().project.tracks.find((t) => t.kind === "audio");
    return { trackId: t.id, clipId: t.clips[0].id };
  });
}

/** A whole song from the first template (pad, arpeggio, bass, lead, beat), as in screenshots.mjs. */
const song = (app) =>
  app.eval(async () => {
    const plan = (await T.backend.songTemplates())[0];
    const w = await T.backend.songWrite(plan, false);
    T.st.snapshot.value = w.snapshot;
    return w.snapshot.project.tracks.map((t) => ({ id: t.id, name: t.name, kind: t.instrument?.kind ?? t.kind, clips: t.clips.map((c) => c.id) }));
  });

// ------------------------------------------------------------------ shots

const SHOTS = {
  /** History window: edit steps with their key figures, a named checkpoint and a variant. */
  "history-window": {
    async run(app) {
      await viewport(app, 1400, 1000);
      const clock = await fixedClock(app);
      await clock.at(8);
      await app.eval(() => T.st.act(T.backend.projectNew()));
      await clock.at(95);
      const tracks = await song(app);
      const lead = tracks.find((t) => t.name === "Lead");
      const bass = tracks.find((t) => t.name === "Bass");
      const arp = tracks.find((t) => t.name === "Arpeggio");
      await clock.at(4 * 60 + 12);
      await app.eval(() => T.st.run({ type: "setProject", patch: { tempoBpm: 132 } }));
      await clock.at(7 * 60 + 31);
      // the lead's busiest clip thinned out: every fourth note goes
      await app.eval(async (id) => {
        const t = T.st.snapshot.peek().project.tracks.find((t) => t.id === id);
        const c = [...t.clips].sort((a, b) => (b.source.notes?.length ?? 0) - (a.source.notes?.length ?? 0))[0];
        await T.st.run({ type: "setClipNotes", trackId: id, clipId: c.id, notes: c.source.notes.filter((_, i) => i % 4 !== 3) });
      }, lead.id);
      await clock.at(10 * 60 + 47);
      await app.eval((id) => T.st.run({ type: "setTrack", trackId: id, patch: { gainDb: -3 } }), bass.id);
      await clock.at(11 * 60 + 5);
      await app.eval(() => T.backend.historyCheckpoint("First draft"));
      await clock.at(13 * 60 + 20);
      await app.eval((id) => T.st.run({ type: "setTrack", trackId: id, patch: { pan: -0.4 } }), arp.id);
      // a variant from the generated song: slower, the lead as a choir
      await clock.at(16 * 60 + 2);
      await app.eval(async () => {
        const v = await T.backend.historyView();
        const node = v.rows.find((r) => r.index === 1).id;
        await T.st.act(T.backend.historyVariantNew("Slower", node));
        await T.st.run({ type: "setProject", patch: { tempoBpm: 112 } });
      });
      await clock.at(18 * 60 + 40);
      await app.eval(async (leadId) => {
        const t = T.st.snapshot.peek().project.tracks.find((t) => t.id === leadId);
        await T.st.run({ type: "setTrack", trackId: leadId, patch: { instrument: { ...t.instrument, preset: "Chor" } } });
      }, lead.id);
      // back to the main version
      await app.eval(async () => {
        const v = await T.backend.historyView();
        const main = v.variants.find((x) => !/Slower/.test(x.name));
        await T.st.act(T.backend.historyVariantSwitch(main.id));
        T.st.showPanel("historie");
      });
      await app.waitFor(() => T.$$(".hist-var").length === 2 && !!T.$(".hist-flag") && !!T.$(".hist-fork"), { what: "history with checkpoint and variant" });
      await sleep(500);
      // the window just big enough for the whole table; the picture shows its tabs and the window
      await fitContent(app, "kw-panel[tab=historie]", ".hist-scroll", ".hist-table");
      await shoot(app, "history-window", await box(app, ".dk-stack:has(kw-panel[tab=historie])"));
    },
  },

  /** Combine dialog: per track the bars each variant changed, one conflict, the lead opened into lanes. */
  "combine-variants": {
    async run(app) {
      await viewport(app, 1300, 1000);
      await app.eval(() => T.st.act(T.backend.projectNew()));
      await song(app);
      await app.eval(async () => {
        const track = (name) => T.st.snapshot.peek().project.tracks.find((t) => t.name === name);
        const clip = (name, section) => track(name).clips.find((c) => c.name === section);
        const notes = (name, section, fn) => {
          const c = clip(name, section);
          return T.st.run({ type: "setClipNotes", trackId: track(name).id, clipId: c.id, notes: fn(c.source.notes) });
        };
        const base = (await T.backend.historyCheckpoint("Draft")).current;
        // main version: a lighter lead in the peak, the bass of both peaks an octave up
        await notes("Lead", "Peak", (n) => n.filter((_, i) => i % 4 !== 3));
        await notes("Bass", "Peak", (n) => n.map((x) => ({ ...x, pitch: x.pitch + 12 })));
        await notes("Bass", "Peak 2", (n) => n.map((x) => ({ ...x, pitch: x.pitch + 12 })));
        // variant "Brighter" from the draft: the lead's peak an octave up (the conflict),
        // a busier pad in the break and a brighter arpeggio sound
        await T.st.act(T.backend.historyVariantNew("Brighter", base));
        await notes("Lead", "Peak", (n) => n.map((x) => ({ ...x, pitch: x.pitch + 12 })));
        await notes("Pad", "Break", (n) => n.flatMap((x) => [x, { ...x, pitch: x.pitch + 7, velocity: Math.max(1, x.velocity - 20) }]));
        const arp = track("Arpeggio");
        await T.st.run({ type: "setTrack", trackId: arp.id, patch: { instrument: { ...arp.instrument, preset: "Puls" } } });
        T.st.showPanel("historie");
      });
      await app.waitFor(() => T.$$(".hist-var").length === 2 && T.$(".hist-combine") && !T.$(".hist-combine").disabled, { what: "two variants" });
      await windowSize(app, "kw-panel[tab=historie]", { width: 900 });
      await app.clickOn(".hist-combine");
      await app.waitFor(() => T.$$(".mg-row[data-track]").length >= 3, { what: "combine view" });
      // the lead (with the conflict) opened into lanes with the notes of every version
      await app.eval(() => {
        const row = T.$$(".mg-row[data-track]").find((r) => r.querySelector(".mg-name")?.textContent === "Lead");
        row?.querySelector(".mg-expand")?.click();
      });
      await app.waitFor(() => T.$$(".mg-sub").length === 3, { what: "lanes" });
      await sleep(500);
      await fitContent(app, "kw-panel[tab=historie]", ".mg-scroll", ".mg-grid", { maxWidth: 900 });
      await sleep(1000);
      const conflicts = await app.eval(() => T.$$(".mg-cell.conflict:not(.resolved)").length);
      if (conflicts !== 1) throw new Error(`expected one conflict, found ${conflicts}`);
      await shoot(app, "combine-variants", await box(app, ".dk-stack:has(kw-panel[tab=historie])"));
    },
  },

  /** Analysis in "Several voices" mode: a three-voice chorale, every voice in its colour. */
  "several-voices-analysis": {
    async run(app) {
      await viewport(app, 1300, 1000);
      await app.eval(async () => {
        await T.st.act(T.backend.projectNew());
        await T.st.run({ type: "setProject", patch: { tempoBpm: 100 } });
        const v = await import("./voices.js");
        v.setAnalysisMode("poly");
        v.setVoiceCount("auto");
      });
      const rec = await dropWav(app, "Chorale.wav", chorale());
      await app.eval(async (sel) => {
        T.st.selection.value = sel;
        await (await import("./actions.js")).analyzeSelected();
      }, rec);
      await app.waitFor(() => !!document.querySelector("kw-panel[tab=analyse] .analysis.voices .legend"), { what: "voices analysis", timeout: 30000 });
      await windowSize(app, "kw-panel[tab=analyse]", { width: 900, height: 560 });
      await sleep(800);
      const voices = await app.eval(async () => new Set((await import("./voices.js")).voicesAnalysis.peek().notes.map((n) => n.voice)).size);
      if (voices !== 3) throw new Error(`expected three voices, found ${voices}`);
      await shoot(app, "several-voices-analysis", await box(app, ".dk-stack:has(kw-panel[tab=analyse])"));
    },
  },

  /** Smoothing: fragments of a sung line as notes, the note clip's menu offers Smooth notes. */
  "smooth-notes-from-recordings": {
    async run(app) {
      await viewport(app, 1600, 900);
      // the tracks window wide enough for the clip and the menu next to it
      await windowSize(app, "kw-timeline", { width: 1240 });
      await app.eval(async () => {
        await T.st.act(T.backend.projectNew());
        (await import("./smoothing.js")).setSmoothLevel("off");
        T.st.selection.value = null;
        T.st.viewport.value = { pxPerBeat: 50, scrollX: 0, scrollY: 0 };
        T.st.showPanel("spuren");
      });
      const rec = await dropWav(app, "Sung line.wav", brokenLine());
      // converted without smoothing: the slip, the voice break and the scoop stay notes of their own
      const melody = await app.eval(async (rec) => {
        await (await import("./actions.js")).convertAudio(rec.trackId, rec.clipId, "melodie");
        const t = T.st.snapshot.peek().project.tracks.find((t) => t.kind !== "audio");
        if (t.view !== "notes") await T.st.run({ type: "setTrack", trackId: t.id, patch: { view: "notes" } });
        (await import("./smoothing.js")).setSmoothLevel("medium");
        T.st.selection.value = null;
        await T.sleep(400);
        return { id: t.id, notes: t.clips[0].source.notes.map((n) => n.pitch) };
      }, rec);
      if (!melody.notes.includes(72)) throw new Error(`expected the octave slip among the notes: ${melody.notes.join(" ")}`);
      // right-click on the note clip, behind its last note
      const at = await app.eval(async (id) => {
        const pr = await import("./pianoroll.js");
        const p = T.st.snapshot.peek().project;
        const i = p.tracks.findIndex((t) => t.id === id);
        const v = T.st.viewport.peek();
        const c = document.querySelector("kw-timeline canvas").getBoundingClientRect();
        const clip = p.tracks[i].clips[0];
        const end = clip.startBeats + clip.source.lengthBeats;
        const top = c.y + T.st.RULER_H + pr.rowTops(p.tracks)[i] - v.scrollY;
        const bottom = top + pr.rowHeight(p.tracks[i]);
        return { x: c.x + (end - 0.3) * v.pxPerBeat - v.scrollX, y: top + 30, left: c.x, top: c.y, bottom: Math.min(bottom, c.bottom) };
      }, melody.id);
      await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: at.x, y: at.y });
      await app.send("Input.dispatchMouseEvent", { type: "mousePressed", x: at.x, y: at.y, button: "right", buttons: 2, clickCount: 1 });
      await app.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: at.x, y: at.y, button: "right", buttons: 0, clickCount: 1 });
      await app.waitFor(() => !!document.querySelector('.ctx-menu [data-act^="glaetten-clip"]'), { what: "Smooth notes in the menu" });
      await sleep(300);
      // the timeline from the ruler down to the end of the melody track, and the menu
      const menu = await box(app, ".ctx-menu");
      const right = menu.x + menu.width + 14;
      await shoot(app, "smooth-notes-from-recordings", { x: Math.floor(at.left), y: Math.floor(at.top), width: Math.ceil(right - at.left), height: Math.ceil(Math.max(at.bottom, menu.y + menu.height + 14) - at.top) });
    },
  },

  /** A drum track playing a melody on tuned drums; the Sound window with "Keys: Drum kit | Pitch". */
  "tuned-drums": {
    async run(app) {
      await viewport(app, 1250, 1000);
      const id = await app.eval(async () => {
        await T.st.act(T.backend.projectNew());
        await T.st.run({ type: "setProject", patch: { tempoBpm: 110 } });
        await T.st.run({ type: "addTrack", track: { id: 0, name: "Toms", kind: "instrument" } });
        const t = T.st.snapshot.peek().project.tracks.at(-1);
        // two bars of eighths in C minor, a few rests
        const line = [48, null, 48, 51, null, 55, 53, 51, 48, null, 48, 51, 53, 55, 58, 60];
        const notes = line.flatMap((pitch, i) => (pitch == null ? [] : [{ pitch, startBeats: i * 0.5, lengthBeats: 0.45, velocity: i % 4 === 0 ? 112 : 92 }]));
        await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Toms", startBeats: 0, source: { type: "notes", lengthBeats: 8, notes } } });
        await T.st.run({ type: "setTrack", trackId: t.id, patch: { instrument: { kind: "drums", preset: "standard", params: {} } } });
        await T.st.run({ type: "setTrack", trackId: t.id, patch: { view: "notes" } });
        T.st.selection.value = { trackId: t.id, clipId: null };
        T.st.viewport.value = { pxPerBeat: 68, scrollX: 0, scrollY: 0 };
        T.st.showPanel("klang");
        return t.id;
      });
      await app.waitFor(() => T.$(".snd-keys .seg-btn"), { what: "keys switch" });
      await app.clickOn(".snd-keys .seg-btn", { text: "Pitch" });
      await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).instrument.params.tasten === 1, { what: "keys by pitch" }, id);
      await sleep(500);
      // the tracks window just as tall as the drum track (and the hint below it), the Sound
      // window below it 420 px (it scrolls): the emulated window gets the height the two need
      const now = await app.eval(() => {
        const stack = (s) => document.querySelector(`.dk-stack:has(${s})`).getBoundingClientRect();
        const row = document.querySelector("kw-tracks .track-row").getBoundingClientRect();
        return { tracks: Math.ceil(row.bottom - stack("kw-timeline").top + 44), sum: stack("kw-timeline").height + stack("kw-panel[tab=klang]").height };
      });
      await viewport(app, 1250, app.size.height + now.tracks + 420 - Math.round(now.sum));
      await windowSize(app, "kw-timeline", { height: now.tracks });
      await sleep(800);
      await shoot(app, "tuned-drums", await box(app, [".dk-stack:has(kw-timeline)", ".dk-stack:has(kw-panel[tab=klang])"]));
    },
  },

  /** The whole app on a phone (390 × 844, touch) with a song: compact view, tracks on top, tools as tabs. */
  "phones-and-tablets": {
    async run(app) {
      await viewport(app, 390, 844, { mobile: true });
      // load-time checks (touch, small screen) see the phone
      await app.reload();
      await viewport(app, 390, 844, { mobile: true });
      await app.eval(() => T.st.act(T.backend.projectNew()));
      const tracks = await song(app);
      await app.eval(async (bass) => {
        T.st.selection.value = { trackId: bass.id, clipId: bass.clips[0] ?? null };
        T.st.showPanel("song");
        await T.sleep(300);
        // the whole song across the timeline
        const p = T.st.snapshot.peek().project;
        const end = Math.max(...p.tracks.flatMap((t) => t.clips.map((c) => c.startBeats + (c.source.lengthBeats ?? 0))), 16);
        const width = document.querySelector("kw-timeline")?.clientWidth ?? 300;
        T.st.viewport.value = { pxPerBeat: (width - 12) / end, scrollX: 0, scrollY: 0 };
      }, tracks.find((t) => t.kind === "bass"));
      await sleep(800);
      // the tracks part as tall as all five tracks and the touch hint below them (drag the
      // handle between the tracks and the tool tabs), the Song window below
      const need = await app.eval(() => {
        const stack = document.querySelector(".dk-stack:has(kw-timeline)").getBoundingClientRect();
        const rows = [...document.querySelectorAll("kw-tracks .track-row")].map((r) => r.getBoundingClientRect().bottom);
        return Math.ceil(Math.max(...rows) - stack.top + 40);
      });
      await windowSize(app, "kw-timeline", { height: need });
      await sleep(1200);
      await shoot(app, "phones-and-tablets", null, { park: { x: 389, y: 843 } });
    },
  },

  /** Live analysis while recording: the substitute microphone plays a melody (as in liveanalysis.test.mjs). */
  "live-analysis": {
    files: (dir) => {
      const file = join(dir, "melody.wav");
      writeFileSync(file, liveMelody());
      return { fakeMic: file };
    },
    async run(app) {
      await viewport(app, 1300, 1000);
      await app.eval(async () => {
        await T.st.act(T.backend.projectNew());
        T.st.showPanel("analyse");
      });
      await windowSize(app, "kw-panel[tab=analyse]", { width: 900, height: 520 });
      await app.clickOn("kw-transport .btn.rec");
      await app.waitFor(() => T.st.status.peek().liveAnalysis && !!document.querySelector(".analysis.live"), { what: "live view" });
      // key and tempo found, more than one round of the melody in the curve, and one of
      // its louder notes (E4, G4) sounding right now
      await app.waitFor(
        () => {
          const f = Object.fromEntries(T.$$(".analysis.live .prop").map((p) => [p.querySelector("label")?.textContent, p.querySelector(".value")?.textContent.trim()]));
          return f.Key === "C major" && Math.abs(parseFloat(f["Tempo estimate"]) - 120) < 5 && Number(f.Notes) >= 14 && /^(E4|G4)$/.test(T.$(".live-note").textContent.trim());
        },
        { timeout: 40000, interval: 40, what: "key, tempo and a note live" },
      );
      await shoot(app, "live-analysis", await box(app, ".dk-stack:has(kw-panel[tab=analyse])"));
      await app.clickOn("kw-transport .btn.rec");
      await app.waitFor(() => !T.st.status.peek().recording, { timeout: 15000, what: "recording stopped" });
    },
  },

  /** A phone (412 × 915, touch) at 60 % interface size with a song: the gear menu open at Display. */
  "interface-size": {
    async run(app) {
      await viewport(app, 412, 915, { mobile: true });
      // load-time checks (touch, small screen) see the phone
      await app.reload();
      await viewport(app, 412, 915, { mobile: true });
      await app.eval(async () => (await import("./uiprefs.js")).setUiScale(0.6));
      await app.waitFor(() => Math.abs(innerWidth - 412 / 0.6) <= 2, { what: "60 %" });
      await app.eval(() => T.st.act(T.backend.projectNew()));
      await song(app);
      await app.eval(async () => {
        T.st.showPanel("song");
        await T.sleep(300);
        const p = T.st.snapshot.peek().project;
        const end = Math.max(...p.tracks.flatMap((t) => t.clips.map((c) => c.startBeats + (c.source.lengthBeats ?? 0))), 16);
        const width = document.querySelector("kw-timeline")?.clientWidth ?? 300;
        T.st.viewport.value = { pxPerBeat: (width - 12) / end, scrollX: 0, scrollY: 0 };
      });
      await sleep(600);
      await app.clickOn('[data-help="einstellungen"]');
      await app.waitFor(() => !!document.querySelector(".set-menu.open .scale-slider"), { what: "gear menu" });
      await sleep(400);
      await shoot(app, "interface-size", null, { park: { x: 1, y: 900 } });
    },
  },

  /** The airy spacing on a tablet held sideways (1280 × 800, touch) with a song, the gear menu open at Display. */
  "layout-spacing": {
    async run(app) {
      await viewport(app, 1280, 800, { mobile: true });
      await app.reload();
      await viewport(app, 1280, 800, { mobile: true });
      await app.eval(() => T.st.act(T.backend.projectNew()));
      await song(app);
      await app.eval(async () => {
        T.st.showPanel("song");
        await T.sleep(300);
      });
      await app.clickOn('[data-help="einstellungen"]');
      await app.clickOn('.set-menu.open [data-density="air"]');
      await sleep(800);
      await shoot(app, "layout-spacing", null, { park: { x: 1, y: 799 } });
    },
  },

  /** A string sextet in groups: violins unfolded (−3 dB), violas folded, celli folded and muted. */
  "track-groups": {
    async run(app) {
      await viewport(app, 1250, 760);
      await app.eval(async () => {
        await T.st.act(T.backend.projectNew());
        const parts = [["Violin 1", 76, "#f472b6"], ["Violin 2", 69, "#a78bfa"], ["Viola 1", 62, "#4ade80"], ["Viola 2", 57, "#facc15"], ["Cello 1", 50, "#60a5fa"], ["Cello 2", 43, "#6ee7d8"]];
        for (const [name, base, color] of parts) {
          const notes = Array.from({ length: 14 }, (_, i) => ({ pitch: base + [0, 2, 4, 5, 7, 5, 4][i % 7], startBeats: i * 1.5, lengthBeats: 1.3, velocity: 92 }));
          const clip = { id: 0, name: "Allegro con spirito", startBeats: 0, source: { type: "notes", lengthBeats: 22, notes } };
          await T.st.run({ type: "addTrack", track: { id: 0, name, kind: "instrument", color, view: "compact", clips: [clip] } });
        }
        const g = await import("./groups.js");
        const tracks = () => T.st.snapshot.peek().project.tracks;
        for (const prefix of ["Violin", "Viola", "Cello"]) await g.groupTracks(tracks().filter((t) => t.name.startsWith(prefix)));
        const [violins, violas, celli] = T.st.snapshot.peek().project.groups;
        await T.st.run({ type: "setGroup", groupId: violins.id, patch: { name: "Violins", gainDb: -3 } });
        await T.st.run({ type: "setGroup", groupId: violas.id, patch: { name: "Violas", collapsed: true } });
        await T.st.run({ type: "setGroup", groupId: celli.id, patch: { name: "Celli", mute: true, collapsed: true } });
        T.st.selection.value = { trackId: tracks()[0].id, clipId: null };
        T.st.viewport.value = { pxPerBeat: 34, scrollX: 0, scrollY: 0 };
      });
      await sleep(800);
      await shoot(app, "track-groups", await box(app, [".dk-stack:has(kw-timeline)"]));
    },
  },

  /** What's new (gear menu → What's new), at the top. */
  "whats-new": {
    async run(app) {
      await viewport(app, 1200, 820);
      await app.clickOn('[data-help="einstellungen"]');
      await app.waitFor(() => !!document.querySelector(".set-menu.open [data-whatsnew]"), { what: "What's new in the gear menu" });
      await app.clickOn(".set-menu.open [data-whatsnew]");
      await app.waitFor(() => !!document.querySelector(".modal .whatsnew-list .whatsnew-entry"), { what: "What's new window" });
      await app.eval(() => {
        for (const el of T.$$(".modal, .modal *")) if (el.scrollTop) el.scrollTop = 0;
      });
      await sleep(500);
      await shoot(app, "whats-new", await box(app, ".modal:has(.whatsnew-list)"));
    },
  },
};

// ------------------------------------------------------------------ run

const wanted = process.argv.slice(2);
const unknown = wanted.filter((id) => !SHOTS[id]);
if (unknown.length) {
  console.error(`unknown shot: ${unknown.join(", ")} – known: ${Object.keys(SHOTS).join(", ")}`);
  process.exit(2);
}
mkdirSync(OUT, { recursive: true });
let failed = 0;
for (const id of wanted.length ? wanted : Object.keys(SHOTS)) {
  const shot = SHOTS[id];
  const tmp = shot.files ? makeTempDir("klangwelt-shot-") : null;
  let app;
  try {
    const options = shot.files ? shot.files(tmp) : {};
    app = await startWeb({ languages: LANGUAGES, ...options });
    app.evalTimeout = 120000;
    await shot.run(app);
    if (app.errors.length) console.warn(`${id}: errors in the page:\n  ${app.errors.join("\n  ")}`);
  } catch (e) {
    failed++;
    console.error(`${id}: ${e.stack ?? e.message}`);
  } finally {
    await app?.close();
    await removeTempDir(tmp);
  }
}
process.exit(failed ? 1 : 0);
