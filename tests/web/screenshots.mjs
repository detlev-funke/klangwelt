// Screenshots for the README: drives the browser version in a headless Edge
// (same harness as the browser tests, muted) and writes PNGs to docs/images.
//
//   cd ui && npm run build:web && cd .. && node tests/web/screenshots.mjs
//
// Rerun after UI changes – the pictures are generated, not drawn by hand.
// Language: English (KLANGWELT_SHOTS_LANG="de-DE,de" for another one).
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT, sleep, startWeb } from "./harness.mjs";

const OUT = process.argv[2] ?? join(ROOT, "docs", "images");
mkdirSync(OUT, { recursive: true });
const W = 1600;
const H = 1000;

const app = await startWeb({ languages: process.env.KLANGWELT_SHOTS_LANG ?? "en-US,en" });
app.evalTimeout = 120000;
try {
  // double pixel density: sharp on high-resolution screens and when GitHub scales the picture down
  await app.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 2, mobile: false });

  /**
   * Screenshot of the whole window, or of one element (CSS selector) with a
   * margin; `bottomAt` cuts the picture above the first element matching it.
   */
  const shoot = async (name, selector, { margin = 0, bottomAt, rows } = {}) => {
    let clip;
    if (selector) {
      const r = await app.eval(
        (s, b) => {
          const el = document.querySelector(s);
          if (!el) return null;
          const r = el.getBoundingClientRect();
          let bottom = r.bottom;
          if (b) {
            const cut = [...el.querySelectorAll(b)].map((e) => e.getBoundingClientRect().top).filter((t) => t > r.top);
            if (cut.length) bottom = Math.min(bottom, Math.min(...cut) - 6);
          }
          return { x: r.x, y: r.y, width: r.width, height: bottom - r.y };
        },
        selector,
        bottomAt ?? null,
      );
      if (!r) throw new Error(`not found: ${selector}`);
      const x = Math.max(0, r.x - margin);
      const y = Math.max(0, r.y - margin);
      clip = { x, y, width: Math.min(W, r.x + r.width + margin) - x, height: Math.min(H, r.y + r.height + margin) - y, scale: 1 };
      if (rows) Object.assign(clip, { y: rows.top, height: rows.height });
    }
    const res = await app.send("Page.captureScreenshot", { format: "png", ...(clip ? { clip } : {}) });
    const file = join(OUT, `${name}.png`);
    writeFileSync(file, Buffer.from(res.data, "base64"));
    console.log(file);
  };

  // A whole song from the first template: pad, arpeggio, bass, lead, beat
  const song = () =>
    app.eval(async () => {
      await T.st.act(T.backend.projectNew());
      const plan = (await T.backend.songTemplates())[0];
      const w = await T.backend.songWrite(plan, false);
      T.st.snapshot.value = w.snapshot;
      return w.snapshot.project.tracks.map((t) => ({
        id: t.id,
        name: t.name,
        kind: t.instrument?.kind ?? t.kind,
        clips: t.clips.filter((c) => c.source.type === "notes").map((c) => ({ id: c.id, name: c.name, start: c.startBeats, notes: c.source.notes?.length ?? 0 })),
      }));
    });
  await song();
  // the harmony band above the tracks (chords in the colour of their mood); remembered across the reload
  await app.eval(async () => (await import("./harmonyline.js")).setHarmonyLane(true));
  await sleep(1200);
  // The tracks window just as tall as its tracks, the windows below get the rest:
  // set the split in the saved layout, then reload (the layout is read at start).
  const split = await app.eval(() => {
    const ws = document.querySelector(".workspace");
    const col = ws?.closest(".dk-split.col");
    const rows = [...document.querySelectorAll("kw-tracks .track-row")];
    if (!col || !rows.length) return;
    const c = col.getBoundingClientRect();
    const top = col.firstElementChild.getBoundingClientRect().top;
    const bottom = Math.max(...rows.map((r) => r.getBoundingClientRect().bottom)) + 14;
    const share = Math.min(0.85, Math.max(0.3, (bottom - top) / c.height));
    const key = "klangwelt.dock.v1";
    const layout = JSON.parse(localStorage.getItem(key) ?? "null");
    const fix = (n) => {
      if (!n || n.kind !== "split") return false;
      if (n.dir === "col" && JSON.stringify(n.children[0]).includes('"spuren"')) {
        n.sizes = [share, 1 - share];
        return true;
      }
      return n.children.some(fix);
    };
    const ok = !!layout && fix(layout.root);
    if (ok) localStorage.setItem(key, JSON.stringify(layout));
    return { share, ok };
  });
  console.log("tracks window share:", JSON.stringify(split));
  await app.reload();
  await app.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 2, mobile: false });
  const tracks = await song();
  console.log(tracks.map((t) => `${t.name} (${t.kind})`).join(", "));
  const bass = tracks.find((t) => t.kind === "bass") ?? tracks.find((t) => /bass/i.test(t.name));
  const lead = tracks.find((t) => /lead|melod/i.test(t.name) && t.clips.length) ?? tracks.find((t) => t.clips.length && !["drums", "bass"].includes(t.kind));

  // 1. Overview: the whole song fills the timeline, its structure in the song
  //    window below, circle of fifths and agent chat on the right
  await app.eval(async (b) => {
    if (b) T.st.selection.value = { trackId: b.id, clipId: b.clips[0]?.id ?? null };
    T.st.showPanel("song");
    await T.sleep(300);
    const p = T.st.snapshot.peek().project;
    const end = Math.max(...p.tracks.flatMap((t) => t.clips.map((c) => c.startBeats + (c.source.lengthBeats ?? 0))), 16);
    const width = document.querySelector("kw-timeline")?.clientWidth ?? 1000;
    T.st.viewport.value = { pxPerBeat: (width - 24) / end, scrollX: 0, scrollY: 0 };
  }, bass ?? null);
  await sleep(1800);
  await shoot("overview");

  // 2. Pitches: the lead melody as a piano roll at the climax, where it plays
  //    most – moved to the top, so the harmony band above it shows its chords;
  //    the picture runs from the tracks toolbar down to the end of this track
  if (lead) {
    const row = await app.eval(async (l) => {
      await T.backend.execute({ type: "moveTrack", trackId: l.id, index: 0 });
      T.st.snapshot.value = await T.backend.execute({ type: "setTrack", trackId: l.id, patch: { view: "notes" } });
      // the clip where the melody plays most (the climax)
      const clip = [...l.clips].sort((a, b) => b.notes - a.notes)[0];
      T.st.selection.value = { trackId: l.id, clipId: clip?.id ?? null };
      T.st.panelEmphasis.value = { id: "spuren" };
      await T.sleep(600);
      // the clip's start at the left edge
      const ppb = 28;
      T.st.viewport.value = { pxPerBeat: ppb, scrollX: Math.max(0, (clip?.start ?? 0) * ppb - 2), scrollY: 0 };
      await T.sleep(800);
      const first = document.querySelector("kw-tracks .track-row").getBoundingClientRect();
      const ws = document.querySelector(".workspace").getBoundingClientRect();
      return { top: ws.top, height: Math.min(first.bottom, ws.bottom) - ws.top };
    }, lead);
    await sleep(800);
    await shoot("notes", ".workspace", { rows: row });
    await app.eval(() => (T.st.panelEmphasis.value = null));
  }

  // 3. Sound workshop for the bass: the three fields with their presets, large
  if (bass) {
    await app.eval(async (b) => {
      T.st.selection.value = { trackId: b.id, clipId: null };
      T.st.showPanel("klang");
      T.st.panelEmphasis.value = { id: "klang", minWidth: 1500 };
    }, bass);
    await sleep(1500);
    await shoot("sound-bass", ".snd-fields");
    await app.eval(() => (T.st.panelEmphasis.value = null));
  }

  // 4. Harmony: circle of fifths with the ways to a cadence
  await app.eval(() => {
    T.st.showPanel("harmonie");
    T.st.panelEmphasis.value = { id: "harmonie", minWidth: 900 };
  });
  await sleep(1500);
  await shoot("circle-of-fifths", "kw-panel[tab=harmonie]");
} finally {
  await app.close();
}
