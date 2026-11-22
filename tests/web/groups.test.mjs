// Track groups in the track list (ui/src/groups.ts, components/tracks.ts): picking tracks with
// Ctrl+click and grouping them with Ctrl+G, the group's row (mute, solo, level, fold), the
// context menus of tracks and groups, dragging a track out of a group by its icon, and the
// view settings of the track headers (pan, pitch, width with the splitter).
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

const CTRL = 2;
const project = () => app.eval(() => T.st.snapshot.peek().project);
/** Middle of an element (CSS selector) in window coordinates. */
const center = (sel) =>
  app.eval((s) => {
    const el = document.querySelector(s);
    if (!el) return null;
    const b = el.getBoundingClientRect();
    return { x: b.x + b.width / 2, y: b.y + b.height / 2, top: b.top, bottom: b.bottom, left: b.left, right: b.right, width: b.width };
  }, sel);
async function mouse(type, x, y, { button = "left", mods = 0, clickCount = 1 } = {}) {
  const buttons = type === "mouseReleased" ? 0 : button === "right" ? 2 : 1;
  await app.send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" ? "none" : button, buttons: type === "mouseMoved" && !mods ? 0 : buttons, clickCount, modifiers: mods });
}
async function clickAt(sel, opts = {}) {
  const c = await center(sel);
  assert.ok(c, `element ${sel}`);
  await mouse("mouseMoved", c.x, c.y);
  await mouse("mousePressed", c.x, c.y, opts);
  await mouse("mouseReleased", c.x, c.y, opts);
  await sleep(150);
}
const row = (name) => app.eval((n) => {
  const p = T.st.snapshot.peek().project;
  const id = p.tracks.find((t) => t.name === n)?.id;
  return id ? `.track-row[data-track-id="${id}"]` : null;
}, name);
/** The rows of the track list from top to bottom: "G:name" for groups, the track names otherwise. */
const listed = () =>
  app.eval(() => {
    const p = T.st.snapshot.peek().project;
    return T.$$(".tracks-body > *").map((el) => (el.classList.contains("group-row") ? `G:${el.querySelector("input.text").value}` : p.tracks.find((t) => t.id === Number(el.dataset.trackId))?.name));
  });

test("Ctrl+click picks tracks, Ctrl+G groups them: a group row above them, the tracks indented", async () => {
  await app.eval(async () => {
    for (const name of ["Violin 1", "Viola", "Violin 2"]) await T.st.run({ type: "addTrack", track: { id: 0, name, kind: "instrument" } });
  });
  await app.waitFor(() => T.$$(".track-row").length === 3, { what: "three tracks" });
  // (on the colored stripe: the name field would take the keys)
  await clickAt(`${await row("Violin 1")} .stripe`);
  await clickAt(`${await row("Violin 2")} .stripe`, { mods: CTRL });
  assert.equal(await app.eval(() => T.$$(".track-row.picked").length), 2, "two tracks picked");
  await app.key("g", { code: "KeyG", vk: 71, mods: CTRL });
  await app.waitFor(() => (T.st.snapshot.peek().project.groups ?? []).length === 1, { what: "a group" });
  const p = await project();
  const g = p.groups[0];
  assert.equal(g.name, "Gruppe 1");
  assert.deepEqual(p.tracks.filter((t) => t.group === g.id).map((t) => t.name), ["Violin 1", "Violin 2"]);
  // the grouped tracks follow each other where the first one was
  assert.deepEqual(await listed(), ["G:Gruppe 1", "Violin 1", "Violin 2", "Viola"]);
  assert.equal(await app.eval(() => T.$(".group-row .group-count").textContent), "2");
  assert.equal(await app.eval(() => T.$$(".track-row.in-group").length), 2);
});

test("the group's mute, solo and level act on its tracks; folding hides them (no undo step)", async () => {
  await clickAt(".group-row .toggle.m");
  await app.waitFor(() => T.st.snapshot.peek().project.groups[0].mute, { what: "group muted" });
  assert.equal(await app.eval(() => T.$$(".track-row.group-muted").length), 2, "its tracks look muted");
  await clickAt(".group-row .toggle.m");
  await clickAt(".group-row .toggle.s");
  await app.waitFor(() => T.st.snapshot.peek().project.groups[0].solo, { what: "group solo" });
  await clickAt(".group-row .toggle.s");
  await app.eval(() => {
    const r = T.$(".group-row input[type=range]");
    r.value = "-6";
    r.dispatchEvent(new Event("input"));
    r.dispatchEvent(new Event("change"));
  });
  await app.waitFor(() => T.st.snapshot.peek().project.groups[0].gainDb === -6, { what: "group level" });
  assert.equal(await app.eval(() => T.$(".group-row .group-db").textContent), "-6.0 dB");

  await clickAt(".group-row .group-fold");
  await app.waitFor(() => T.st.snapshot.peek().project.groups[0].collapsed, { what: "folded" });
  assert.deepEqual(await listed(), ["G:Gruppe 1", "Viola"], "the group's tracks are hidden");
  await clickAt(".group-row .group-fold");
  await app.waitFor(() => T.$$(".track-row").length === 3, { what: "unfolded" });
});

test("context menu of a track: move it into the group; of the group: ungroup – the tracks stay", async () => {
  const viola = await row("Viola");
  const c = await center(`${viola} .stripe`);
  await mouse("mousePressed", c.x, c.y, { button: "right" });
  await mouse("mouseReleased", c.x, c.y, { button: "right" });
  await app.waitFor(() => !!T.$(".ctx-menu"), { what: "track menu" });
  const g = (await project()).groups[0];
  const acts = await app.eval(() => T.$$(".ctx-menu [data-act]").map((b) => b.dataset.act));
  for (const a of ["umbenennen", "farbe", "gruppieren", `in-gruppe-${g.id}`, "ansicht", "stumm", "solo", "zeige-pan", "zeige-pitch", "loeschen"]) assert.ok(acts.includes(a), `${a} in ${acts}`);
  await clickAt(`.ctx-menu [data-act="in-gruppe-${g.id}"]`);
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.filter((t) => t.group).length === 3, { what: "viola in the group" });
  assert.deepEqual(await listed(), ["G:Gruppe 1", "Violin 1", "Violin 2", "Viola"]);

  // drag the viola by its icon above the group's row: out of the group, in front of it
  const grip = await center(`${viola} .kind`);
  const head = await center(".group-row");
  await mouse("mousePressed", grip.x, grip.y);
  for (let k = 1; k <= 6; k++) await mouse("mouseMoved", grip.x, grip.y + ((head.top + 4 - grip.y) * k) / 6, { mods: 0 });
  assert.ok(await app.eval(() => !!T.$(".drop-line")), "a line shows where it goes");
  await mouse("mouseReleased", grip.x, head.top + 4);
  await app.waitFor(() => !T.st.snapshot.peek().project.tracks.find((t) => t.name === "Viola").group, { what: "viola out of the group" });
  assert.deepEqual(await listed(), ["Viola", "G:Gruppe 1", "Violin 1", "Violin 2"]);

  const h = await center(".group-row .kind");
  await mouse("mousePressed", h.x, h.y, { button: "right" });
  await mouse("mouseReleased", h.x, h.y, { button: "right" });
  await app.waitFor(() => !!T.$('.ctx-menu [data-act="aufloesen"]'), { what: "group menu" });
  await clickAt('.ctx-menu [data-act="aufloesen"]');
  await app.waitFor(() => !(T.st.snapshot.peek().project.groups ?? []).length, { what: "ungrouped" });
  assert.equal((await project()).tracks.length, 3, "the tracks stay");
  assert.equal(await app.eval(() => T.$$(".group-row").length), 0);
  // undo brings the group back with its tracks
  await app.key("z", { code: "KeyZ", vk: 90, mods: CTRL });
  await app.waitFor(() => (T.st.snapshot.peek().project.groups ?? []).length === 1, { what: "group back" });
  assert.equal((await project()).tracks.filter((t) => t.group).length, 2);
  assert.deepEqual(app.errors, []);
});

test("several picked tracks: the sound window changes all of them, each keeps the rest", async () => {
  const ids = await app.eval(() => ["Violin 1", "Viola", "Violin 2"].map((n) => T.st.snapshot.peek().project.tracks.find((t) => t.name === n).id));
  // Violin 2 sounds different before (its own brightness)
  await app.eval(async (id) => {
    const t = T.st.snapshot.peek().project.tracks.find((x) => x.id === id);
    await T.st.run({ type: "setTrack", trackId: id, patch: { instrument: { ...t.instrument, params: { ...t.instrument.params, helligkeit: 0.9 } } } });
  }, ids[2]);
  await clickAt(`${await row("Violin 1")} .stripe`);
  await clickAt(`${await row("Viola")} .stripe`, { mods: CTRL });
  await clickAt(`${await row("Violin 2")} .stripe`, { mods: CTRL });
  await app.eval(() => T.st.showPanel("klang"));
  await app.waitFor(() => !!T.$(".snd-multi") && !T.$(".snd-multi").hidden, { what: "note: several tracks" });
  assert.match(await app.eval(() => T.$(".snd-multi").textContent), /3 Spuren/);
  // a field of the sound window (through the shell, as the window calls it)
  const before = await project();
  await app.eval(async (all) => {
    const { backend } = await import("./bridge.js");
    T.st.snapshot.value = await backend.soundField(all[0], "charakter", 0.15, 0.85, null, false, all.slice(1));
  }, [ids[0], ids[1], ids[2]]);
  const after = await project();
  const params = (p, i) => p.tracks.find((t) => t.id === ids[i]).instrument.params;
  for (const i of [0, 1, 2]) assert.notDeepEqual(params(after, i), params(before, i), `track ${i} changed`);
  // a fine-tuning control: the same value everywhere, only that one
  await app.eval(() => {
    T.$(".snd-fine").open = true;
    const r = T.$(".snd-fine input[type=range]");
    r.value = String((Number(r.min) + Number(r.max)) / 2);
    r.dispatchEvent(new Event("input"));
    r.dispatchEvent(new Event("change"));
  });
  await sleep(300);
  const fine = await project();
  const changed = Object.keys(params(fine, 0)).filter((k) => params(fine, 0)[k] !== params(after, 0)[k]);
  assert.ok(changed.length > 0, "the control changed something");
  for (const i of [1, 2]) {
    for (const k of Object.keys(params(fine, i))) {
      if (changed.includes(k)) assert.equal(params(fine, i)[k], params(fine, 0)[k], `track ${i} follows ${k}`);
      else assert.equal(params(fine, i)[k], params(after, i)[k], `track ${i} keeps its own ${k}`);
    }
  }
  // the track headers' own controls stay individual
  await clickAt(`${await row("Viola")} .toggle.m`);
  const p = await project();
  assert.deepEqual(p.tracks.map((t) => t.mute), p.tracks.map((t) => t.name === "Viola"));
  assert.equal(await app.eval(() => T.$$(".track-row.picked").length), 3, "clicking M keeps the pick");
  await clickAt(`${await row("Viola")} .toggle.m`);
});

test("view of the track headers: pan and pitch can be hidden, the width set with the splitter", async () => {
  // the buttons above the tracks sit on the left
  const tracks = await center("kw-tracks");
  const group = await center(".tracks-head .group");
  assert.ok(group.left - tracks.left < 60, `buttons on the left (${group.left - tracks.left}px)`);

  await clickAt(".tracks-head .view-toggle");
  await clickAt('.ctx-menu [data-act="zeige-pan"]');
  assert.ok(await app.eval(() => T.$("kw-tracks").classList.contains("no-pan")));
  assert.equal(await app.eval(() => T.$$(".track-row .knob.pan").filter((k) => k.offsetParent).length), 0, "no pan slider");
  await clickAt(".tracks-head .view-toggle");
  await clickAt('.ctx-menu [data-act="zeige-pitch"]');
  assert.equal(await app.eval(() => T.$$(".track-row .stepper.pitch").filter((k) => k.offsetParent).length), 0, "no pitch stepper");
  assert.equal(await app.eval(() => localStorage.getItem("klangwelt.spuren.pan")), "aus", "remembered");
  await clickAt(".tracks-head .view-toggle");
  assert.equal(await app.eval(() => T.$('.ctx-menu [data-act="zeige-pan"]').getAttribute("aria-checked")), "false");
  await clickAt('.ctx-menu [data-act="zeige-pan"]');
  await clickAt(".tracks-head .view-toggle");
  await clickAt('.ctx-menu [data-act="zeige-pitch"]');
  assert.equal(await app.eval(() => T.$$(".track-row .knob.pan").filter((k) => k.offsetParent).length), 3, "pan back");

  const s = await center(".heads-splitter");
  await mouse("mousePressed", s.x, s.y);
  for (let k = 1; k <= 5; k++) await mouse("mouseMoved", s.x + 20 * k, s.y, { mods: 0 });
  await mouse("mouseReleased", s.x + 100, s.y);
  const wide = await center("kw-tracks");
  assert.ok(Math.abs(wide.width - tracks.width - 100) <= 2, `100 px wider (${tracks.width} → ${wide.width})`);
  assert.equal(await app.eval(() => localStorage.getItem("klangwelt.spuren.breite")), String(Math.round(wide.width)));
  await clickAt(".heads-splitter", { clickCount: 1 });
  const s2 = await center(".heads-splitter");
  await mouse("mousePressed", s2.x, s2.y, { clickCount: 2 });
  await mouse("mouseReleased", s2.x, s2.y, { clickCount: 2 });
  await sleep(150);
  assert.equal((await center("kw-tracks")).width, tracks.width, "double-click: default width");
  assert.deepEqual(app.errors, []);
});
