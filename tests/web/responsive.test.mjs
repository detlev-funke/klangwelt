// Narrow windows and phones: the toolbar shrinks in steps (labels go, then
// controls move into the File menu and "⋯"), the gear always stays, and nothing
// sticks out sideways. Viewports are emulated over CDP (device metrics, touch).
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
      export: box(T.$("kw-transport .export-btn")),
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

test("toolbar: at desktop width everything stays as it was – labels, Save, Export, no \"⋯\"", async () => {
  await viewport(1600, 1000);
  const s = await toolbar();
  assert.equal(s.fit, 0);
  assert.ok(s.save && s.export, "Save and Export in the bar");
  assert.equal(s.more, null, "no \"⋯\" at full width");
  const labels = await app.eval(() => [".file-btn", ".save-btn", ".export-btn", ".win-btn"].map((sel) => T.$(`kw-transport ${sel} .lbl`).getClientRects().length > 0));
  assert.deepEqual(labels, [true, true, true, true], "File, Save, Export and Windows with their labels");
});

test("toolbar on a phone: Save and Export are in the File menu and work there", async () => {
  await viewport(390, 844, { touch: true });
  const s = await toolbar();
  assert.equal(s.save, null, "Save left the bar");
  assert.equal(s.export, null, "Export left the bar");
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
