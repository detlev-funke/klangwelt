// Narrow windows and phones: the toolbar shrinks in steps (labels go, then
// controls move into the File menu and "⋯"), the gear always stays, and nothing
// sticks out sideways. On a phone the windows become a compact view (Tracks on
// top, the rest as tabs – or one window at a time), the timeline and the track
// headers work with fingers, and menus and dialogs fit the screen.
// Viewports are emulated over CDP (device metrics, touch emulation, touch events).
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

/** Emulate a viewport; `touch`: a phone (coarse pointer, touch events). */
async function viewport(width, height, { touch = false } = {}) {
  await app.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: touch ? 2 : 1, mobile: touch });
  await app.send("Emulation.setTouchEmulationEnabled", { enabled: touch, maxTouchPoints: touch ? 5 : 1 });
  // the bar fits itself in the next frame after a resize
  await sleep(250);
}

/** State of the toolbar: step, gear and the page width. */
const toolbar = () =>
  app.eval(() => {
    const tr = T.$("kw-transport");
    const box = (el) => {
      const r = el?.getBoundingClientRect();
      return r && r.width ? { left: r.left, right: r.right, top: r.top, bottom: r.bottom } : null;
    };
    return {
      fit: Number(tr.dataset.fit),
      gear: box(T.$('kw-transport [data-help="einstellungen"]')),
      save: box(T.$("kw-transport .save-btn")),
      more: box(T.$("kw-transport .more-btn")),
      // widest the page or the bar gets – both must stay within the window
      pageWidth: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth),
      barRight: Math.max(...[...tr.children].map((c) => c.getBoundingClientRect()).filter((b) => b.width).map((b) => b.right)),
      width: innerWidth,
    };
  });

/** Close the open drop-down menus. */
async function closeMenus() {
  await app.eval(() => {
    for (const m of T.$$(".menu.open")) m.classList.remove("open");
  });
}

test("toolbar: from 1920 down to 320 px nothing sticks out and the gear stays visible", async () => {
  let last = -1;
  for (const w of [1920, 1600, 1440, 1280, 1100, 1000, 900, 800, 700, 600, 500, 430, 390, 360, 320]) {
    await viewport(w, 800, { touch: w < 700 });
    const s = await toolbar();
    assert.ok(s.pageWidth <= w, `${w} px: page is ${s.pageWidth} px wide`);
    assert.ok(s.barRight <= w, `${w} px: a control ends at ${Math.round(s.barRight)} px`);
    assert.ok(s.gear, `${w} px: gear hidden`);
    assert.ok(s.gear.left >= 0 && s.gear.right <= w, `${w} px: gear at ${s.gear.left}–${s.gear.right}`);
    assert.ok(s.fit >= last, `${w} px: step ${s.fit} after ${last} – narrower never shows more`);
    last = s.fit;
  }
  assert.ok(last >= 7, `at 320 px Save and Export are in the File menu (step ${last})`);
});

test("toolbar at desktop width: File and Windows with labels, Save as a disk with its dot, Export only in the File menu, no \"⋯\"", async () => {
  await viewport(1600, 1000);
  const s = await toolbar();
  assert.equal(s.fit, 0);
  assert.ok(s.save, "Save in the bar");
  assert.equal(await app.eval(() => T.$("kw-transport .export-btn")), null, "no Export button – it is in the File menu");
  assert.equal(s.more, null, "no \"⋯\" at full width");
  const labels = await app.eval(() => [".file-btn", ".win-btn"].map((sel) => T.$(`kw-transport ${sel} .lbl`).getClientRects().length > 0));
  assert.deepEqual(labels, [true, true], "File and Windows with their labels");
  const save = await app.eval(() => {
    const b = T.$("kw-transport .save-btn");
    return { text: b.textContent.trim(), label: b.getAttribute("aria-label"), width: b.getBoundingClientRect().width };
  });
  assert.deepEqual({ text: save.text, label: save.label }, { text: "", label: "Speichern" }, "only the disk (named for screen readers)");
  assert.ok(save.width <= 40, `Save is ${save.width} px wide`);
  // unsaved changes: the dot on the disk
  await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Punkt", kind: "instrument" } });
  });
  await app.waitFor(() => T.$("kw-transport .save-btn .dirty-dot").getClientRects().length > 0, { what: "dot on the disk" });
  // the project name has more room: it keeps 240 px while logo text, level meter, device name and
  // agent status can still make way (it used to stop at 160 px), and takes what is free up to 320 px
  await app.eval(() => T.st.run({ type: "setProject", patch: { name: "Ein ziemlich langer Projekttitel für die Leiste" } }));
  for (const w of [1600, 1440]) {
    await viewport(w, 900);
    const n = await app.eval(() => ({ w: T.$("kw-transport .proj-name").getBoundingClientRect().width, fit: Number(T.$("kw-transport").dataset.fit) }));
    assert.ok(n.w >= 239, `${w} px: project name ${n.w} px wide`);
    assert.ok(n.fit <= 2, `${w} px: only extras made way (step ${n.fit})`);
    assert.ok((await toolbar()).barRight <= w, `${w} px: nothing sticks out`);
  }
  // a short name changes nothing: everything as before
  await app.eval(() => T.st.run({ type: "setProject", patch: { name: "Demo" } }));
  await viewport(1600, 1000);
  assert.equal((await toolbar()).fit, 0);
});

test("track header bar: the harmony switch, + Audio and + Instrument fit inside the header column", async () => {
  await viewport(1600, 1000);
  const r = await app.eval(() => {
    const head = T.$(".tracks-head").getBoundingClientRect();
    return T.$$(".tracks-head .group > *").map((b) => {
      const x = b.getBoundingClientRect();
      return { what: b.textContent.trim() || b.getAttribute("aria-label"), inside: x.left >= head.left && x.right <= head.right };
    });
  });
  assert.deepEqual(r.filter((b) => !b.inside), [], "nothing cut off");
  assert.deepEqual(r.map((b) => b.what), ["Harmonie", "Audio", "Instrument ▾"]);
});

test("toolbar on a phone: Save and Export are in the File menu and work there", async () => {
  await viewport(390, 844, { touch: true });
  const s = await toolbar();
  assert.equal(s.save, null, "Save left the bar");
  // unsaved changes: the dot moves to the File button
  await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Handy", kind: "instrument" } });
  });
  await app.waitFor(() => T.$("kw-transport .file-btn .dirty-dot").getClientRects().length > 0, { what: "dot on File" });
  await app.clickOn("kw-transport .file-btn");
  const items = await app.eval(() =>
    T.$$(".file-menu.open .menu-item").map((b) => {
      const r = b.getBoundingClientRect();
      return { text: b.firstChild.textContent, inside: r.left >= 0 && r.right <= innerWidth && r.bottom <= innerHeight };
    }),
  );
  for (const want of ["Speichern", "Exportieren …"]) {
    const it = items.find((i) => i.text === want);
    assert.ok(it, `${want} in the File menu (has: ${items.map((i) => i.text).join(", ")})`);
    assert.ok(it.inside, `${want} fits on the screen`);
  }
  await app.clickOn(".file-menu.open .menu-item", { text: "Exportieren" });
  await app.waitFor(() => !!T.$(".modal-back .export-modal, .modal-back .modal"), { what: "export dialog" });
  const dlg = await app.eval(() => {
    const r = T.$(".modal-back .modal").getBoundingClientRect();
    return { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
  });
  assert.ok(dlg.left >= 0 && dlg.right <= 390 && dlg.top >= 0 && dlg.bottom <= 844, `export dialog fits: ${JSON.stringify(dlg)}`);
  await app.key("Escape", { vk: 27 });
  await app.waitFor(() => !T.$(".modal-back"), { what: "dialog closed" });
});

test("toolbar on a phone: \"⋯\" holds tempo, metronome, loop, undo/redo and the devices", async () => {
  await viewport(390, 844, { touch: true });
  await closeMenus();
  await app.clickOn("kw-transport .more-btn");
  const menu = await app.eval(() => {
    const m = T.$(".more-menu.open");
    const r = m.getBoundingClientRect();
    return {
      items: T.$$(".more-menu.open .menu-item em").map((e) => e.textContent),
      tempo: !!m.querySelector(".more-tempo input.num"),
      inside: r.left >= 0 && r.right <= innerWidth && r.bottom <= innerHeight,
    };
  });
  assert.ok(menu.tempo, "tempo field in \"⋯\"");
  for (const want of ["Metronom", "Wiederholen", "Rückgängig", "Wiederherstellen", "Ausgabegerät", "Aufnahmegerät"]) {
    assert.ok(menu.items.includes(want), `${want} in "⋯" (has: ${menu.items.join(", ")})`);
  }
  assert.ok(menu.inside, "\"⋯\" fits on the screen");
  // tempo +1 from the menu
  const bpm = await app.eval(() => T.st.snapshot.peek().project.tempoBpm);
  await app.clickOn(".more-menu.open .more-tempo .btn", { text: "+" });
  await app.waitFor((b) => T.st.snapshot.peek().project.tempoBpm === b + 1, { what: "tempo +1" }, bpm);
  // metronome from the menu
  const metro = await app.eval(() => T.st.snapshot.peek().engine.metronome);
  await app.clickOn(".more-menu.open .mi-metro");
  await app.waitFor((m) => T.st.snapshot.peek().engine.metronome === !m, { what: "metronome toggled" }, metro);
  // back: undo from the menu takes the tempo back
  await app.eval(() => T.st.act(T.backend.setMetronome(false)));
  await app.clickOn("kw-transport .more-btn");
  await app.clickOn(".more-menu.open .mi-undo");
  await app.waitFor((b) => T.st.snapshot.peek().project.tempoBpm === b, { what: "undo tempo" }, bpm);
  // devices open inside the menu
  await app.clickOn("kw-transport .more-btn");
  await app.clickOn(".more-menu.open .mi-out");
  await app.waitFor(() => T.$(".more-menu.open .dev-option"), { what: "output devices in \"⋯\"" });
  await closeMenus();
  assert.deepEqual(app.errors, []);
});

// ---------------------------------------------------------------- phones and tablets

/** Touch points (window pixels) – every call lists all fingers still down. */
const touch = (type, points) =>
  app.send("Input.dispatchTouchEvent", { type, touchPoints: points.map(([x, y], id) => ({ x, y, id, radiusX: 3, radiusY: 3, force: 1 })) });

async function tap(x, y) {
  await touch("touchStart", [[x, y]]);
  await touch("touchEnd", []);
  await sleep(120);
}

/** One finger from a to b in a few steps. */
async function swipe([x0, y0], [x1, y1], steps = 8) {
  await touch("touchStart", [[x0, y0]]);
  for (let i = 1; i <= steps; i++) {
    await touch("touchMove", [[x0 + ((x1 - x0) * i) / steps, y0 + ((y1 - y0) * i) / steps]]);
    await sleep(16);
  }
  await touch("touchEnd", []);
  await sleep(150);
}

/** Center of an element (CSS selector, optionally containing a text). */
async function center(selector, text = null) {
  const c = await app.eval((sel, t) => {
    const el = T.$$(sel).find((e) => e.getClientRects().length && (t == null || e.textContent.includes(t)));
    if (!el) return null;
    // e.g. a tab further along a tab bar that scrolls sideways
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
    const b = el.getBoundingClientRect();
    return [b.x + b.width / 2, b.y + b.height / 2];
  }, selector, text);
  if (!c) throw new Error(`not found: ${selector}${text ? ` with "${text}"` : ""}`);
  return c;
}
const tapOn = async (selector, text) => tap(...(await center(selector, text)));

/** The compact view: mode and the tabs of each tab group. */
const dock = () =>
  app.eval(() => ({
    mode: T.$("#dock").dataset.mode,
    stacks: T.$$("#dock .dk-stack").map((s) => [...s.querySelectorAll(".dk-tab")].map((t) => t.dataset.panel)),
  }));

/** Is a window visible, inside the window width, and does nothing stick out sideways (own scroll areas excepted)? */
const panelFits = (id) =>
  app.eval((id) => {
    const p = T.$(`.dk-panel[data-panel="${id}"]`);
    if (!p || p.hidden || !p.getClientRects().length) return `${id} not visible`;
    const r = p.getBoundingClientRect();
    if (r.left < 0 || r.right > innerWidth) return `${id} at ${r.left}–${r.right}`;
    // areas meant to scroll sideways: the step grid, the song's sections, the timeline
    const own = ".rh-grid, .song-scroll, .lib-scroll, kw-timeline, kw-tracks, .tracks-body";
    const wide = [...p.querySelectorAll("*")].filter((e) => {
      const cs = getComputedStyle(e);
      return (cs.overflowX === "auto" || cs.overflowX === "scroll") && e.scrollWidth > e.clientWidth + 1 && !e.matches(own);
    });
    if (wide.length) return `${id}: ${wide.map((e) => `${e.className} ${e.scrollWidth}>${e.clientWidth}`).join(", ")} scrolls sideways`;
    const page = Math.max(document.documentElement.scrollWidth, document.body.scrollWidth);
    return page <= innerWidth ? true : `${id}: page ${page} px wide`;
  }, id);

async function demo() {
  await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    await (await import("./actions.js")).loadDemo();
    T.st.selection.value = null;
  });
  await sleep(300);
}

test("phone upright: Tracks on top, every other window as a tab below – each one fits the width", async () => {
  await viewport(390, 844, { touch: true });
  await demo();
  const d = await dock();
  assert.equal(d.mode, "sheet");
  assert.equal(d.stacks.length, 2, "two parts");
  assert.deepEqual(d.stacks[0], ["spuren"], "Tracks alone on top");
  for (const id of ["clip", "analyse", "klang", "rhythmus", "song", "bibliothek", "agent", "harmonie", "fuge"]) {
    assert.ok(d.stacks[1].includes(id), `${id} as a tab below (has: ${d.stacks[1].join(", ")})`);
  }
  // the tab bar below scrolls sideways with a finger
  const bar = await app.eval(() => {
    const b = T.$$("#dock .dk-tabs")[1];
    b.scrollLeft = 0;
    const r = b.getBoundingClientRect();
    return { x: r.x, y: r.y + r.height / 2, w: r.width, more: b.scrollWidth > b.clientWidth };
  });
  assert.ok(bar.more, "more tabs than fit");
  await swipe([bar.x + bar.w - 30, bar.y], [bar.x + 30, bar.y]);
  assert.ok((await app.eval(() => T.$$("#dock .dk-tabs")[1].scrollLeft)) > 50, "tab bar scrolled");
  // a clip selected, so that Clip and Sound show their content
  await app.eval(() => {
    const t = T.st.snapshot.peek().project.tracks.find((x) => x.kind === "instrument" && x.clips.length);
    T.st.selection.value = { trackId: t.id, clipId: t.clips[0].id };
  });
  for (const id of ["spuren", ...d.stacks[1]]) {
    if (id !== "spuren") await tapOn(`.dk-tab[data-panel="${id}"]`);
    await app.waitFor((id) => T.st.visiblePanels.peek().has(id), { what: `${id} in front` }, id);
    await sleep(150);
    assert.equal(await panelFits(id), true);
  }
  assert.deepEqual(app.errors, []);
});

test("phone sideways: one window at a time, a tap on a tab switches", async () => {
  await viewport(844, 390, { touch: true });
  const d = await dock();
  assert.equal(d.mode, "single");
  assert.equal(d.stacks.length, 1);
  assert.equal(d.stacks[0][0], "spuren", "Tracks first");
  await tapOn('.dk-tab[data-panel="harmonie"]');
  await app.waitFor(() => T.st.visiblePanels.peek().has("harmonie") && !T.st.visiblePanels.peek().has("spuren"), { what: "harmony in front" });
  assert.equal(await panelFits("harmonie"), true);
  // the toolbar is lower when the phone is held sideways
  assert.ok((await app.eval(() => T.$("kw-transport").getBoundingClientRect().height)) <= 52);
  await tapOn('.dk-tab[data-panel="spuren"]');
  await app.waitFor(() => T.st.visiblePanels.peek().has("spuren"), { what: "tracks in front" });
});

test("phone: the divider between tracks and tabs follows the finger and is remembered", async () => {
  await viewport(390, 844, { touch: true });
  const top = () => app.eval(() => T.$$("#dock .dk-stack")[0].getBoundingClientRect().height);
  const before = await top();
  const [x, y] = await center("#dock .dk-gutter.col");
  await swipe([x, y], [x, y - 120]);
  const after = await top();
  assert.ok(after < before - 80, `tracks part ${before} → ${after} px`);
  const saved = await app.eval(() => JSON.parse(localStorage.getItem("klangwelt.dock.v1.compact")).sizes);
  assert.ok(saved[0] < 0.45, `remembered share ${saved}`);
  await app.reload();
  await viewport(390, 844, { touch: true });
  assert.ok(Math.abs((await top()) - after) < 4, "same height after reloading");
  // back to where it was
  const [x2, y2] = await center("#dock .dk-gutter.col");
  await swipe([x2, y2], [x2, y2 + 120]);
  assert.ok(Math.abs((await top()) - before) < 10, "dragged back");
});

test("phone: \"Dock windows freely\" in the Windows menu brings back the docking layout", async () => {
  await viewport(390, 844, { touch: true });
  await tapOn("kw-transport .win-btn");
  await app.waitFor(() => T.$(".win-menu.open .free-layout"), { what: "switch in the Windows menu" });
  assert.match(await app.eval(() => T.$(".win-menu.open .menu-hint").textContent), /Kleiner Bildschirm/);
  await tapOn(".win-menu.open .free-layout");
  await app.waitFor(() => T.$("#dock").dataset.mode === "dock", { what: "docking layout" });
  assert.ok((await dock()).stacks.length >= 3, "the windows side by side again");
  // remembered – and back to the compact view
  assert.equal(await app.eval(() => JSON.parse(localStorage.getItem("klangwelt.dock.v1.compact")).free), true);
  await tapOn("kw-transport .win-btn");
  await tapOn(".win-menu.open .free-layout");
  await app.waitFor(() => T.$("#dock").dataset.mode === "sheet", { what: "compact view again" });
});

test("tablet sideways: the full docking layout, resizable with a finger", async () => {
  await viewport(1024, 768, { touch: true });
  assert.equal((await dock()).mode, "dock");
  const width = () => app.eval(() => T.$('.dk-panel[data-panel="spuren"]').getBoundingClientRect().width);
  const before = await width();
  const [x, y] = await center("#dock .dk-gutter.row");
  await swipe([x, y], [x - 80, y]);
  const after = await width();
  assert.ok(after < before - 50, `tracks ${before} → ${after} px wide`);
  // and back
  await swipe([x - 80, y], [x, y]);
  // upright tablet: too narrow for the layout – compact view
  await viewport(768, 1024, { touch: true });
  assert.equal((await dock()).mode, "sheet");
});

test("phone: toolbar buttons and window tabs are at least 40 px for a finger", async () => {
  await viewport(390, 844, { touch: true });
  const small = await app.eval(() =>
    [...T.$$("kw-transport button"), ...T.$$("#dock .dk-tab")]
      .filter((b) => b.getClientRects().length && !b.closest(".menu"))
      .map((b) => ({ what: b.className || b.title, r: b.getBoundingClientRect() }))
      .filter(({ r }) => r.height < 39.5 || r.width < 39.5)
      .map(({ what, r }) => `${what} ${Math.round(r.width)}×${Math.round(r.height)}`),
  );
  assert.deepEqual(small, []);
});

test("phone: the menus and the Open dialog fit on the screen", async () => {
  await viewport(360, 740, { touch: true });
  const inside = (sel) => app.eval((sel) => {
    const r = T.$(sel).getBoundingClientRect();
    return r.left >= 0 && r.top >= 0 && r.right <= innerWidth && r.bottom <= innerHeight ? true : `${sel}: ${Math.round(r.left)},${Math.round(r.top)} – ${Math.round(r.right)},${Math.round(r.bottom)}`;
  }, sel);
  for (const [btn, menu] of [["kw-transport .file-btn", ".file-menu.open"], ["kw-transport .win-btn", ".win-menu.open"], ['kw-transport [data-help="einstellungen"]', ".set-menu.open"], ["kw-transport .more-btn", ".more-menu.open"]]) {
    await closeMenus();
    await tapOn(btn);
    await app.waitFor((m) => !!T.$(m), { what: menu }, menu);
    assert.equal(await inside(menu), true);
  }
  await closeMenus();
  await tapOn("kw-transport .file-btn");
  await tapOn(".file-menu.open .menu-item", "Öffnen");
  await app.waitFor(() => !!T.$(".modal-back .modal"), { what: "Open dialog" });
  assert.equal(await inside(".modal-back .modal"), true);
  await app.key("Escape", { vk: 27 });
  await app.waitFor(() => !T.$(".modal-back"), { what: "dialog closed" });
});

/** One instrument track in the piano roll with an empty clip from beat 1 (4 beats), 50 px per beat. */
async function pianoRoll() {
  return app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Finger", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    if (t.view !== "notes") await T.st.run({ type: "setTrack", trackId: t.id, patch: { view: "notes" } });
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Noten", startBeats: 1, source: { type: "notes", lengthBeats: 4, notes: [] } } });
    const pr = await import("./pianoroll.js");
    pr.pitchViews.update((m) => ({ ...m, [t.id]: { low: 60, span: 12 } }));
    T.st.viewport.value = { pxPerBeat: 50, scrollX: 0, scrollY: 0 };
    T.st.selection.value = null;
    T.st.showPanel("spuren");
    await T.sleep(250);
    const g = pr.geoFor(T.st.snapshot.peek().project.tracks[0]);
    const c = T.$("kw-timeline canvas").getBoundingClientRect();
    const rowY = c.y + T.st.RULER_H;
    return { id: t.id, cx: c.x, cy: c.y, cw: c.width, ch: c.height, rowY, lanesMid: rowY + pr.NOTES_TOP + g.lanesH / 2, rowBottom: rowY + pr.rowHeight(T.st.snapshot.peek().project.tracks[0]) };
  });
}
const notesOf = (id) => app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.length, id);

test("timeline with fingers: two fingers zoom and pan, one finger beside the clips moves the view", async () => {
  await viewport(844, 390, { touch: true });
  const g = await pianoRoll();
  const v0 = await app.eval(() => T.st.viewport.peek());
  // spread two fingers: zoom in around their middle
  const mx = g.cx + 200;
  const my = g.lanesMid;
  await touch("touchStart", [[mx - 40, my]]);
  await touch("touchStart", [[mx - 40, my], [mx + 40, my]]);
  for (let i = 1; i <= 6; i++) {
    await touch("touchMove", [[mx - 40 - i * 10, my], [mx + 40 + i * 10, my]]);
    await sleep(16);
  }
  // … and move both to the left: the time follows
  for (let i = 1; i <= 4; i++) {
    await touch("touchMove", [[mx - 100 - i * 15, my], [mx + 100 - i * 15, my]]);
    await sleep(16);
  }
  await touch("touchEnd", []);
  await sleep(150);
  const v1 = await app.eval(() => T.st.viewport.peek());
  assert.ok(v1.pxPerBeat > v0.pxPerBeat * 2, `zoom ${v0.pxPerBeat} → ${v1.pxPerBeat}`);
  assert.ok(v1.scrollX > 0, "panned");
  assert.equal(await notesOf(g.id), 0, "two fingers add nothing");
  // one finger on an empty spot of the track (after the clip): the view follows
  await app.eval(() => (T.st.viewport.value = { pxPerBeat: 50, scrollX: 0, scrollY: 0 }));
  await sleep(100);
  const x = g.cx + 7 * 50;
  await swipe([x + 100, g.lanesMid], [x, g.lanesMid]);
  const v2 = await app.eval(() => T.st.viewport.peek());
  assert.ok(v2.scrollX > 60, `swiped to ${v2.scrollX}`);
  assert.equal(await notesOf(g.id), 0);
});

test("timeline with fingers: holding opens the context menu, a double tap adds a note", async () => {
  await viewport(844, 390, { touch: true });
  const g = await pianoRoll();
  // double tap on an empty lane inside the clip: a note
  const x = g.cx + 2.5 * 50;
  await tap(x, g.lanesMid);
  await tap(x, g.lanesMid);
  await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.length === 1, { what: "note from a double tap" }, g.id);
  await sleep(500);
  assert.equal(await notesOf(g.id), 1, "exactly one note (not also from the browser's dblclick)");
  // hold the finger on the clip: its context menu
  await touch("touchStart", [[x, g.lanesMid]]);
  await sleep(750);
  await touch("touchEnd", []);
  await app.waitFor(() => !!T.$(".ctx-menu"), { what: "context menu" });
  const r = await app.eval(() => { const b = T.$(".ctx-menu").getBoundingClientRect(); return { left: b.left, right: b.right, top: b.top, bottom: b.bottom }; });
  assert.ok(r.left >= 0 && r.right <= 844 && r.top >= 0 && r.bottom <= 390, `menu fits: ${JSON.stringify(r)}`);
  assert.equal(await notesOf(g.id), 1, "holding changed nothing");
  await app.key("Escape", { vk: 27 });
  // hold in the ruler and drag: marks a range to loop
  await touch("touchStart", [[g.cx + 50, g.cy + 12]]);
  await sleep(700);
  for (let i = 1; i <= 6; i++) {
    await touch("touchMove", [[g.cx + 50 + i * 25, g.cy + 12]]);
    await sleep(16);
  }
  await touch("touchEnd", []);
  await app.waitFor(() => !!T.st.snapshot.peek().engine.loopRange, { what: "range from the ruler" });
  const range = await app.eval(() => T.st.snapshot.peek().engine.loopRange);
  assert.ok(range[1] - range[0] >= 2, `range ${range}`);
  await app.eval(() => T.st.act(T.backend.setLoopRange(null)));
  assert.deepEqual(app.errors, []);
});

test("track headers on a phone: a slim column that unfolds over the timeline, a swipe scrolls them", async () => {
  await viewport(390, 844, { touch: true });
  await demo();
  const head = () => app.eval(() => {
    const k = T.$("kw-tracks");
    return { w: k.getBoundingClientRect().width, open: k.classList.contains("open"), level: !!T.$$(".track-controls").find((e) => e.getClientRects().length) };
  });
  const slim = await head();
  assert.ok(slim.w < 140 && !slim.open && !slim.level, `slim column: ${JSON.stringify(slim)}`);
  await tapOn("kw-tracks .heads-toggle");
  const open = await head();
  assert.ok(open.open && open.w > 280 && open.level, `unfolded: ${JSON.stringify(open)}`);
  // a tap on the timeline folds them again
  const c = await center("kw-timeline canvas");
  await tap(c[0] + 100, c[1]);
  assert.equal((await head()).open, false);
  // more tracks than fit: a swipe on the headers scrolls them (and the timeline)
  await app.eval(async () => {
    for (let i = 0; i < 6; i++) await T.st.run({ type: "addTrack", track: { id: 0, name: `Mehr ${i}`, kind: "audio" } });
  });
  await sleep(200);
  const y0 = await app.eval(() => T.st.viewport.peek().scrollY);
  // start low in the headers, on a name or button – not on a row's splitter (resizes the row)
  // and not near the divider below (a finger close to it lands on it)
  const k = await app.eval(() => {
    const b = T.$("kw-tracks").getBoundingClientRect();
    const x = b.x + 30;
    let y = b.bottom - 28;
    for (; y > b.y + 60; y -= 4) {
      const el = document.elementFromPoint(x, y);
      if (el?.closest("kw-tracks") && !el.closest(".splitter, input[type=range], canvas.keys")) break;
    }
    return { x, from: y, to: y - 120 };
  });
  await swipe([k.x, k.from], [k.x, k.to]);
  const y1 = await app.eval(() => T.st.viewport.peek().scrollY);
  assert.ok(y1 > y0 + 100, `scrolled ${y0} → ${y1}`);
});

test("help on a phone: a tap explains the tapped control", async () => {
  await viewport(390, 844, { touch: true });
  await app.eval(() => T.st.showPanel("hilfe"));
  await app.waitFor(() => T.st.visiblePanels.peek().has("hilfe"), { what: "help in front" });
  await tapOn("kw-transport .win-btn");
  await app.waitFor(() => /Fenster/.test(T.$("kw-help .help-title")?.textContent ?? ""), { what: "help for the Windows menu" });
  await closeMenus();
  assert.deepEqual(app.errors, []);
});
