// The gear menu's display settings in the browser: the language as one entry that opens
// the list (mouse, keyboard, finger), the interface size (the page zoom of phones and
// tablets through the viewport – every tap still hits what is under it; a desktop browser
// keeps its own zoom), the spacing of the layout (tight, normal, airy), the link to the
// project page – and the installed app opening in full screen with the safe areas kept clear.
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

const PHONE = [390, 844];

/** Emulate a viewport; `touch`: a phone (coarse pointer, touch events, viewport meta). */
async function viewport(width, height, { touch = false } = {}) {
  await app.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: touch ? 2 : 1, mobile: touch });
  await app.send("Emulation.setTouchEmulationEnabled", { enabled: touch, maxTouchPoints: touch ? 5 : 1 });
  await sleep(250);
}

const touch = (type, points) =>
  app.send("Input.dispatchTouchEvent", { type, touchPoints: points.map(([x, y], id) => ({ x, y, id, radiusX: 3, radiusY: 3, force: 1 })) });

async function tap(x, y) {
  await touch("touchStart", [[x, y]]);
  await touch("touchEnd", []);
  await sleep(150);
}

async function center(selector) {
  const c = await app.eval((sel) => {
    const el = T.$$(sel).find((e) => e.getClientRects().length);
    if (!el) return null;
    el.scrollIntoView({ block: "nearest" });
    const b = el.getBoundingClientRect();
    return [b.x + b.width / 2, b.y + b.height / 2];
  }, selector);
  if (!c) throw new Error(`not found: ${selector}`);
  return c;
}
const tapOn = async (selector) => tap(...(await center(selector)));

async function openSettings() {
  if (!(await app.eval(() => !!T.$(".set-menu.open")))) await app.clickOn('[data-help="einstellungen"]');
  await app.waitFor(() => !!T.$(".set-menu.open .lang-entry"), { what: "settings menu" });
}

async function closeMenus() {
  await app.eval(() => {
    for (const m of T.$$(".menu.open")) m.classList.remove("open");
  });
}

/** Focus and page of the settings menu. */
const menuState = () =>
  app.eval(() => {
    const m = T.$(".set-menu");
    const a = document.activeElement;
    return {
      open: m.classList.contains("open"),
      page: m.dataset.page,
      languages: T.$$(".set-menu.open [data-lang]").length,
      focus: a?.dataset.lang ?? (a?.classList.contains("lang-entry") ? "entry" : a?.classList.contains("lang-back") ? "back" : a?.dataset.help ?? a?.tagName),
    };
  });

// ---------------------------------------------------------------- language

test("gear menu: the language is one entry with flag and name; it opens the list as a page of its own", async () => {
  await openSettings();
  const entry = await app.eval(() => {
    const e = T.$(".set-menu.open .lang-entry");
    return {
      flag: e.querySelector("svg.flag")?.dataset.flag,
      name: e.querySelector("b").textContent,
      sub: e.querySelector("small").textContent,
      popup: e.getAttribute("aria-haspopup"),
      languages: T.$$(".set-menu.open [data-lang]").length,
    };
  });
  assert.deepEqual(entry, { flag: "de", name: "Deutsch", sub: "Sprache · Language", popup: "true", languages: 0 });
  await app.clickOn(".set-menu.open .lang-entry");
  const list = await menuState();
  assert.equal(list.page, "languages");
  assert.equal(list.languages, 7);
  assert.equal(list.focus, "de", "the current language has the focus");
  assert.equal(await app.eval(() => T.$('.set-menu [data-lang="de"]').getAttribute("aria-current")), "true");
  // back: the main page again, the entry has the focus
  await app.clickOn(".set-menu.open .lang-back");
  const back = await menuState();
  assert.deepEqual([back.page, back.languages, back.focus, back.open], ["main", 0, "entry", true]);
  await closeMenus();
});

test("language list by keyboard: → opens, arrows move, Esc goes back, Esc again closes – the app's keys stay out", async () => {
  await openSettings();
  await app.eval(() => T.$(".set-menu.open .lang-entry").focus());
  await app.key("ArrowRight", { vk: 39 });
  assert.deepEqual(await menuState(), { open: true, page: "languages", languages: 7, focus: "de" });
  // a grid with two columns: right one step, down one row; up past the first row → the back button
  const order = await app.eval(() => T.$$(".set-menu [data-lang]").map((b) => b.dataset.lang));
  let i = order.indexOf("de");
  await app.key("ArrowRight", { vk: 39 });
  i += 1;
  assert.equal((await menuState()).focus, order[i]);
  await app.key("ArrowDown", { vk: 40 });
  i += 2;
  assert.equal((await menuState()).focus, order[i]);
  while (i >= 0) {
    await app.key("ArrowUp", { vk: 38 });
    i -= 2;
    assert.equal((await menuState()).focus, i >= 0 ? order[i] : "back");
  }
  await app.key("End", { vk: 35 });
  assert.equal((await menuState()).focus, order.at(-1));
  // Space on a menu entry doesn't start playback
  await app.key("Escape", { vk: 27 });
  assert.deepEqual(await menuState(), { open: true, page: "main", languages: 0, focus: "entry" });
  await app.key(" ", { code: "Space", vk: 32 });
  await sleep(200);
  assert.equal(await app.eval(() => T.st.status.peek().playing), false, "Space stayed in the menu");
  await app.eval(() => T.$(".set-menu").dataset.page === "languages" && T.$(".set-menu .lang-back").click());
  await app.eval(() => T.$(".set-menu.open .lang-entry")?.focus());
  await app.key("Escape", { vk: 27 });
  const closed = await menuState();
  assert.equal(closed.open, false, "Esc on the main page closes the menu");
  assert.equal(closed.focus, "einstellungen", "the gear has the focus again");
  assert.deepEqual(app.errors, []);
});

test("language list on a phone: a tap opens it, it fits the screen, a tap goes back", async () => {
  await viewport(...PHONE, { touch: true });
  await tapOn('kw-transport [data-help="einstellungen"]');
  await app.waitFor(() => !!T.$(".set-menu.open .lang-entry"), { what: "settings menu" });
  await tapOn(".set-menu.open .lang-entry");
  await app.waitFor(() => T.$$(".set-menu.open [data-lang]").length === 7, { what: "language list" });
  const r = await app.eval(() => {
    const boxes = [T.$(".set-menu.open"), ...T.$$(".set-menu.open [data-lang]")].map((e) => e.getBoundingClientRect());
    return { left: Math.min(...boxes.map((b) => b.left)), right: Math.max(...boxes.map((b) => b.right)), top: Math.min(...boxes.map((b) => b.top)), bottom: Math.max(...boxes.map((b) => b.bottom)), w: innerWidth, h: innerHeight, minH: Math.min(...boxes.slice(1).map((b) => b.height)) };
  });
  assert.ok(r.left >= 0 && r.right <= r.w && r.top >= 0 && r.bottom <= r.h, `fits: ${JSON.stringify(r)}`);
  assert.ok(r.minH >= 40, `finger-sized entries: ${r.minH} px`);
  await tapOn(".set-menu.open .lang-back");
  assert.equal((await menuState()).page, "main");
  await closeMenus();
  await viewport(1600, 1000);
});

// ---------------------------------------------------------------- interface size

/** Set the size slider the way a release of the thumb does (input, then change). */
async function slideTo(percent) {
  await openSettings();
  await app.eval((p) => {
    const s = T.$(".set-menu.open .scale-slider");
    s.value = String(p);
    s.dispatchEvent(new Event("input", { bubbles: true }));
    s.dispatchEvent(new Event("change", { bubbles: true }));
  }, percent);
}

/** Back to 100 % and a desktop window – also after a failed test, so the next one starts clean. */
async function resetSize() {
  await closeMenus();
  await app.eval(() => T.backend.setUiScale(1)).catch(() => {});
  await viewport(1600, 1000);
}

/** Before the next load: the window width at the moment the app first fills the toolbar. */
const watchFirstLayout = () =>
  app.send("Page.addScriptToEvaluateOnNewDocument", {
    source: `new MutationObserver((_, o) => {
      if (document.querySelector("kw-transport")?.childElementCount) {
        window.__firstLayout = { width: innerWidth, density: document.documentElement.dataset.density ?? "normal" };
        o.disconnect();
      }
    }).observe(document, { childList: true, subtree: true });`,
  });

/** A notes clip at beat 1 on a new track, its pitch lanes 60…71 and the timeline at 50 px per beat. */
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
    await T.sleep(300);
    return t.id;
  });
}

/** Window point of a pitch lane's middle at a beat (the timeline's own geometry). */
const pointOf = (id, beat, pitch) =>
  app.eval(async (id, beat, pitch) => {
    const pr = await import("./pianoroll.js");
    const track = T.st.snapshot.peek().project.tracks.find((t) => t.id === id);
    const g = pr.geoFor(track);
    const c = T.$("kw-timeline canvas").getBoundingClientRect();
    const { pxPerBeat, scrollX, scrollY } = T.st.viewport.peek();
    const top = pr.rowTops(T.st.snapshot.peek().project.tracks)[T.st.snapshot.peek().project.tracks.indexOf(track)];
    return [c.x + beat * pxPerBeat - scrollX, c.y + T.st.RULER_H + top - scrollY + pr.laneY(g, pitch) + g.laneH / 2];
  }, id, beat, pitch);

const notesOf = (id) => app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.map((n) => ({ pitch: n.pitch, start: n.startBeats })), id);

test("interface size on a phone: 40 … 130 % upright, up to 200 % sideways – the layout is the page zoom, set before the first layout after a reload", async (t) => {
  t.after(resetSize);
  await viewport(...PHONE, { touch: true });
  await openSettings();
  const s = await app.eval(() => ({ works: T.backend.uiScaleWorks(), disabled: T.$(".set-menu.open .scale-slider").disabled, value: T.$(".set-menu.open .scale-value").textContent }));
  assert.deepEqual(s, { works: true, disabled: false, value: "100 %" });
  // upright, the slider goes as far as a layout of 300 px: 130 % on a phone 390 px wide
  assert.deepEqual(await app.eval(() => [T.$(".set-menu.open .scale-slider").min, T.$(".set-menu.open .scale-slider").max]), ["40", "130"]);
  for (const [p, width] of [[40, 390 / 0.4], [70, 390 / 0.7], [130, 390 / 1.3]]) {
    await slideTo(p);
    await app.waitFor((w) => Math.abs(innerWidth - w) <= 2, { what: `layout width at ${p} %` }, width);
    const v = await app.eval(() => ({ scale: visualViewport.scale, stored: localStorage.getItem("klangwelt.uiScale"), shown: T.$(".set-menu .scale-value").textContent }));
    assert.ok(Math.abs(v.scale - p / 100) < 0.01, `page scale ${v.scale}`);
    assert.equal(v.stored, String(p / 100));
    assert.equal(v.shown, `${p} %`);
    // the toolbar and the dock fit themselves within a frame or two
    await app.waitFor((w) => Math.max(document.documentElement.scrollWidth, document.body.scrollWidth) <= Math.ceil(w) + 1, { what: `nothing sticking out sideways at ${p} %` }, width);
  }
  // sideways there is room for 200 %; turned upright again, the layout stays 300 px wide
  await closeMenus();
  await viewport(844, 390, { touch: true });
  await openSettings();
  assert.equal(await app.eval(() => T.$(".set-menu.open .scale-slider").max), "200");
  await slideTo(200);
  await app.waitFor(() => Math.abs(innerWidth - 844 / 2) <= 2, { what: "200 % sideways" });
  await closeMenus();
  await viewport(...PHONE, { touch: true });
  await app.waitFor(() => Math.abs(innerWidth - 300) <= 2, { what: "upright: no narrower than 300 px" });
  assert.equal(await app.eval(() => localStorage.getItem("klangwelt.uiScale")), "2", "the chosen size stays");
  // after a reload the stored size is there before the app lays out anything
  await slideTo(80);
  await app.waitFor(() => Math.abs(innerWidth - 390 / 0.8) <= 2, { what: "80 %" });
  await watchFirstLayout();
  await app.reload();
  const first = await app.eval(() => window.__firstLayout);
  assert.ok(Math.abs(first.width - 390 / 0.8) <= 2, `first layout at ${first.width} px`);
  // turned sideways: the layout width follows the new device width
  await viewport(844, 390, { touch: true });
  await app.waitFor(() => Math.abs(innerWidth - 844 / 0.8) <= 2, { what: "sideways at 80 %" });
  await viewport(...PHONE, { touch: true });
  await app.waitFor(() => Math.abs(innerWidth - 390 / 0.8) <= 2, { what: "upright again" });
  // back to 100 %: the button next to the slider
  await openSettings();
  await app.eval(() => T.$(".set-menu.open .scale-reset").click());
  await app.waitFor(() => innerWidth === 390 && localStorage.getItem("klangwelt.uiScale") === null, { what: "100 %" });
  assert.equal(await app.eval(() => T.$(".set-menu .scale-reset").disabled), true);
  await closeMenus();
  assert.deepEqual(app.errors, []);
});

test("interface size on a phone: at 40 %, 70 % and 130 % a finger still hits the lane and the beat under it", async (t) => {
  t.after(resetSize);
  await viewport(...PHONE, { touch: true });
  for (const p of [40, 70, 130, 100]) {
    await slideTo(p);
    await app.waitFor((w) => Math.abs(innerWidth - w) <= 2, { what: `${p} %` }, 390 / (p / 100));
    await closeMenus();
    const id = await pianoRoll();
    // double tap on the lane of E4 (64), a bit after beat 3: a note E4 starting on beat 3
    // (2 beats into the clip, which starts on beat 1)
    const [x, y] = await pointOf(id, 3.1, 64);
    await tap(x, y);
    await tap(x, y);
    await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.length === 1, { what: `note at ${p} %` }, id);
    assert.deepEqual(await notesOf(id), [{ pitch: 64, start: 2 }], `${p} %`);
    // hold in the ruler and drag from beat 2 to beat 4: that range
    const rulerY = await app.eval(() => T.$("kw-timeline canvas").getBoundingClientRect().y + 12);
    const [x2] = await pointOf(id, 2, 64);
    const [x4] = await pointOf(id, 4, 64);
    await touch("touchStart", [[x2, rulerY]]);
    await sleep(700);
    for (let i = 1; i <= 6; i++) {
      await touch("touchMove", [[x2 + ((x4 - x2) * i) / 6, rulerY]]);
      await sleep(16);
    }
    await touch("touchEnd", []);
    await app.waitFor(() => !!T.st.snapshot.peek().engine.loopRange, { what: `range at ${p} %` });
    const range = await app.eval(() => T.st.snapshot.peek().engine.loopRange);
    assert.ok(Math.abs(range[0] - 2) <= 0.25 && Math.abs(range[1] - 4) <= 0.25, `range ${range} at ${p} %`);
    await app.eval(() => T.st.act(T.backend.setLoopRange(null)));
  }
  await viewport(1600, 1000);
  assert.deepEqual(app.errors, []);
});

test("interface size in a desktop browser: the slider stays at 100 % and points to the browser's own zoom", async () => {
  await viewport(1600, 1000);
  await openSettings();
  const s = await app.eval(() => ({ works: T.backend.uiScaleWorks(), disabled: T.$(".set-menu.open .scale-slider").disabled, hint: T.$(".set-menu.open .scale-hint").textContent }));
  assert.equal(s.works, false);
  assert.equal(s.disabled, true);
  assert.match(s.hint, /Strg \+/);
  // asked anyway: the page doesn't follow, the size stays 100 %
  assert.equal(await app.eval(() => T.backend.setUiScale(0.8)), 1);
  assert.equal(await app.eval(() => [innerWidth, localStorage.getItem("klangwelt.uiScale")].join()), "1600,");
  assert.equal(await app.eval(() => T.backend.uiScaleWorks()), false);
  await closeMenus();
  // a fresh page: the next tests start without what this one learned about the browser
  await app.reload();
});

// ---------------------------------------------------------------- spacing

/** Measured spacing: margin around the layout, gutter, tab bar, toolbar, panel padding. */
const spacing = () =>
  app.eval(() => {
    const area = T.$("#dock .dk-area").getBoundingClientRect();
    const host = T.$("#dock").getBoundingClientRect();
    const gutter = T.$("#dock .dk-gutter.row");
    return {
      density: document.documentElement.dataset.density ?? "normal",
      inset: Math.round(area.left - host.left),
      gutter: gutter ? Math.round(gutter.getBoundingClientRect().width) : null,
      tabs: Math.round(T.$("#dock .dk-tabs").getBoundingClientRect().height),
      bar: Math.round(T.$("kw-transport").getBoundingClientRect().height),
      pressed: T.$$(".set-menu.open [data-density]").filter((b) => b.getAttribute("aria-pressed") === "true").map((b) => b.dataset.density),
    };
  });

test("spacing: tight, normal and airy change margins, gutters, tab bars and toolbar at once; it stays after a reload", async () => {
  await viewport(1600, 1000);
  await openSettings();
  const normal = await spacing();
  assert.deepEqual(normal, { density: "normal", inset: 5, gutter: 5, tabs: 33, bar: 64, pressed: ["normal"] }, "normal is the look from before");
  await app.clickOn('.set-menu.open [data-density="tight"]');
  assert.deepEqual(await spacing(), { density: "tight", inset: 2, gutter: 2, tabs: 29, bar: 54, pressed: ["tight"] });
  assert.equal(await app.eval(() => T.$(".set-menu").classList.contains("open")), true, "the menu stays open to compare");
  await app.clickOn('.set-menu.open [data-density="air"]');
  assert.deepEqual(await spacing(), { density: "air", inset: 12, gutter: 10, tabs: 38, bar: 72, pressed: ["air"] });
  // after a reload: airy before the app lays out anything
  await watchFirstLayout();
  await app.reload();
  assert.equal(await app.eval(() => window.__firstLayout.density), "air");
  await openSettings();
  assert.deepEqual(await spacing(), { density: "air", inset: 12, gutter: 10, tabs: 38, bar: 72, pressed: ["air"] });
  await app.clickOn('.set-menu.open [data-density="normal"]');
  assert.deepEqual(await spacing(), normal);
  assert.equal(await app.eval(() => localStorage.getItem("klangwelt.density")), null);
  await closeMenus();
  assert.deepEqual(app.errors, []);
});

test("spacing on a phone: the compact view follows tight and airy too", async () => {
  await viewport(...PHONE, { touch: true });
  await openSettings();
  for (const [d, inset] of [["tight", 1], ["air", 6], ["normal", 3]]) {
    await app.eval((d) => T.$(`.set-menu.open [data-density="${d}"]`).click(), d);
    const s = await app.eval(() => ({ mode: T.$("#dock").dataset.mode, inset: Math.round(T.$("#dock .dk-area").getBoundingClientRect().left - T.$("#dock").getBoundingClientRect().left), page: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth) }));
    assert.notEqual(s.mode, "dock", `compact view (${d})`);
    assert.equal(s.inset, inset, d);
    assert.ok(s.page <= 390, `nothing sticks out (${d}): ${s.page}`);
  }
  await closeMenus();
  await viewport(1600, 1000);
});

// ---------------------------------------------------------------- short hints, full text behind ⓘ

/** Title, short hint, tooltip and the unfolded explanation of a gear-menu setting (its help id). */
const explained = (help) =>
  app.eval((help) => {
    const at = (sel) => T.$(`.set-menu.open ${sel}[data-help="${help}"]`);
    const info = at(".menu-title").querySelector(".info-btn");
    const more = at(".menu-more");
    return {
      title: at(".menu-title").textContent,
      hint: at(".menu-hint:not(.menu-more)").textContent,
      tooltip: info.title,
      hintTooltip: at(".menu-hint:not(.menu-more)").title,
      optionsTooltip: at("[role=group]").title,
      expanded: info.getAttribute("aria-expanded"),
      more: more.hidden ? null : more.textContent,
      infoSize: Math.round(info.getBoundingClientRect().height),
    };
  }, help);

const AUTOSAVE_FULL = "Nur wenn sich etwas geändert hat, und erst nach Wiedergabe oder Aufnahme. Ein Projekt ohne Namen kommt in eine Wiederherstellungskopie. Frühere Stände bleiben in der Änderungshistorie.";

test("autosave and smoothing: a few words under each, the full explanation as tooltip, behind ⓘ and in the help window", async () => {
  await viewport(1600, 1000);
  await openSettings();
  const a = await explained("einstellungen-autosave");
  assert.deepEqual(
    { title: a.title, hint: a.hint, expanded: a.expanded, more: a.more },
    { title: "Automatisch speichern", hint: "Nur wenn sich etwas geändert hat.", expanded: "false", more: null },
  );
  assert.equal(a.tooltip, AUTOSAVE_FULL);
  assert.equal(a.hintTooltip, AUTOSAVE_FULL);
  assert.equal(a.optionsTooltip, AUTOSAVE_FULL);
  const s = await explained("einstellungen-glaetten");
  assert.equal(s.title, "Noten aus Aufnahmen glätten");
  assert.equal(s.hint, "Fügt kurze Bruchstücke zu Noten zusammen.");
  assert.match(s.tooltip, /^Wird eine Aufnahme zu Noten/);
  // a click on ⓘ unfolds the explanation (the menu stays open), another one folds it again
  await app.clickOn('.set-menu.open .menu-title[data-help="einstellungen-autosave"] .info-btn');
  const open = await explained("einstellungen-autosave");
  assert.deepEqual([open.expanded, open.more], ["true", AUTOSAVE_FULL]);
  assert.equal(await app.eval(() => T.$(".set-menu").classList.contains("open")), true);
  await app.clickOn('.set-menu.open .menu-title[data-help="einstellungen-autosave"] .info-btn');
  assert.equal((await explained("einstellungen-autosave")).more, null);
  // F1 over the setting: the help window explains it
  const [hx, hy] = await center('.set-menu.open .menu-hint[data-help="einstellungen-glaetten"]');
  await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: hx, y: hy });
  await app.key("F1", { vk: 112 });
  await app.waitFor(() => T.$("kw-help .help-title")?.textContent === "Noten aus Aufnahmen glätten", { what: "help for smoothing" });
  assert.match(await app.eval(() => T.$("kw-help .help-text").textContent), /^Wird eine Aufnahme zu Noten/);
  await app.key("F1", { vk: 112 });
  await closeMenus();
  assert.deepEqual(app.errors, []);
});

test("on a phone a tap on ⓘ unfolds the explanation – a finger-sized button", async () => {
  await viewport(...PHONE, { touch: true });
  await tapOn('kw-transport [data-help="einstellungen"]');
  await app.waitFor(() => !!T.$(".set-menu.open .lang-entry"), { what: "settings menu" });
  await tapOn('.set-menu.open .menu-title[data-help="einstellungen-glaetten"] .info-btn');
  const s = await explained("einstellungen-glaetten");
  assert.equal(s.expanded, "true");
  assert.match(s.more, /^Wird eine Aufnahme zu Noten/);
  assert.ok(s.infoSize >= 36, `ⓘ is ${s.infoSize} px high`);
  assert.equal(await app.eval(() => T.$(".set-menu").classList.contains("open")), true, "the menu stays open");
  await closeMenus();
  await viewport(1600, 1000);
});

// ---------------------------------------------------------------- project page, full screen

test("gear menu: Klangwelt on GitHub opens the project page in a new tab and closes the menu", async () => {
  await openSettings();
  const link = await app.eval(() => {
    const a = T.$(".set-menu.open [data-github]");
    return { href: a.href, target: a.target, rel: a.rel, text: a.querySelector("b").textContent };
  });
  assert.deepEqual(link, { href: "https://github.com/detlev-funke/klangwelt", target: "_blank", rel: "noreferrer", text: "Klangwelt auf GitHub" });
  // the test keeps the browser from really leaving (after the menu's own handler ran)
  await app.eval(() => document.addEventListener("click", (e) => e.target.closest?.("[data-github]") && e.preventDefault(), { once: true }));
  await app.clickOn(".set-menu.open [data-github]");
  assert.equal(await app.eval(() => T.$(".set-menu").classList.contains("open")), false);
});

test("installed app: the manifest asks for full screen; the toolbar keeps clear of a notch (safe area)", async () => {
  const m = await app.eval(async () => (await fetch("manifest.webmanifest")).json());
  assert.equal(m.display, "fullscreen");
  const meta = await app.eval(() => document.querySelector('meta[name="viewport"]').content);
  assert.match(meta, /viewport-fit=cover/);
  // a notch of 30 px on top, rounded corners left and right (if this Edge can emulate them)
  const can = await app
    .send("Emulation.setSafeAreaInsetsOverride", { insets: { top: 30, left: 12, right: 12, bottom: 20 } })
    .then(() => true, () => false);
  if (!can) return;
  await viewport(...PHONE, { touch: true });
  const box = await app.eval(() => {
    const r = T.$("kw-transport").getBoundingClientRect();
    return { top: r.top, left: r.left, right: innerWidth - r.right };
  });
  assert.deepEqual(box, { top: 30, left: 12, right: 12 });
  await app.send("Emulation.setSafeAreaInsetsOverride", { insets: {} });
  await viewport(1600, 1000);
});
