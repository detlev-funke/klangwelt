// Browser version: the "History" window – every edit step as a table with time,
// label, where and the project's key figures; a click on a row jumps to that
// state; checkpoints and variants; the history is saved in the project file
// (OPFS) and comes back after reloading the page.
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

const rows = () => app.eval(() => T.$$(".hist-row").map((r) => ({
  node: Number(r.dataset.node),
  cells: [...r.children].map((c) => c.textContent.trim()),
  current: r.classList.contains("current"),
  future: r.classList.contains("future"),
})));
const labels = () => app.eval(() => T.$$(".hist-row .hist-label").map((l) => l.textContent));
const names = () => app.eval(() => T.st.snapshot.peek().project.tracks.map((t) => t.name));
const variants = () => app.eval(() => T.$$(".hist-var").map((b) => ({ name: b.childNodes[1].textContent, on: b.classList.contains("on") })));

/** New project, the history window in front. */
async function fresh() {
  await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    T.st.showPanel("historie");
  });
  await app.waitFor(() => T.$$(".hist-row").length === 1 && T.$(".hist-row").textContent.includes("Neues Projekt"), { what: "leere Historie" });
}

/** A few steps: lead track, a clip with notes, a tempo change. */
async function someSteps() {
  await app.eval(async () => {
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Lead", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks.at(-1);
    const notes = [60, 64, 67, 72, 69, 65, 62, 60].map((p, i) => ({ pitch: p, startBeats: i, lengthBeats: 1, velocity: 90 }));
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Thema", startBeats: 8, source: { type: "notes", lengthBeats: 8, notes } } });
    await T.st.run({ type: "setProject", patch: { tempoBpm: 90 } });
  });
  await app.waitFor(() => T.$$(".hist-row").length === 4, { what: "vier Zeilen" });
}

test("table: every step with time, label, where and key figures; the current one marked", async () => {
  await fresh();
  await someSteps();
  const r = await rows();
  assert.deepEqual(await labels(), ["Neues Projekt", "Spur hinzufügen", "Clip hinzufügen", "Tempo ändern"]);
  // columns: # | time | step | where | notes | bars | duration | tracks | clips | tempo | key
  const clip = r[2].cells;
  assert.match(clip[1], /^\d\d:\d\d:\d\d$/, `Uhrzeit: ${clip[1]}`);
  assert.equal(clip[3], "Lead · Takt 3–4 · +8 Noten");
  assert.equal(clip[4], "8+8", "8 notes, 8 more than before");
  assert.deepEqual([clip[5], clip[6], clip[7], clip[8], clip[9], clip[10]], ["4", "0:08", "1", "1", "120", "C-Dur"]);
  assert.equal(r[3].cells[3], "120 → 90 BPM");
  assert.equal(r[3].cells[6], "0:11", "the same 4 bars last longer at 90 BPM");
  assert.deepEqual(r.map((x) => x.current), [false, false, false, true]);
  // changed figures are highlighted
  assert.ok(await app.eval(() => T.$$(".hist-row")[2].children[4].classList.contains("chg")));
  assert.deepEqual(await variants(), [{ name: "Hauptversion", on: true }]);
  assert.match(await app.eval(() => T.$(".hist-foot").textContent), /3 Schritte/);
  await app.shot("historie-tabelle");
  assert.deepEqual(app.errors, []);
});

test("click on a row goes back, Ctrl+Z / Ctrl+Y move along the same line", async () => {
  await fresh();
  await someSteps();
  await app.clickOn(".hist-row:nth-child(2)");
  await app.waitFor(() => T.$(".hist-row:nth-child(2)")?.classList.contains("current"), { what: "Zeile 2 aktuell" });
  assert.deepEqual(await names(), ["Lead"]);
  assert.equal((await app.eval(() => T.st.snapshot.peek().project.tracks[0].clips.length)), 0);
  assert.deepEqual((await rows()).map((x) => x.future), [false, false, true, true], "later steps stay, to redo");
  // Ctrl+Y redoes one step, Ctrl+Z undoes it
  await app.key("y", { code: "KeyY", vk: 89, mods: 2 });
  await app.waitFor(() => T.$(".hist-row:nth-child(3)")?.classList.contains("current"), { what: "Wiederherstellen" });
  await app.key("z", { code: "KeyZ", vk: 90, mods: 2 });
  await app.waitFor(() => T.$(".hist-row:nth-child(2)")?.classList.contains("current"), { what: "Rückgängig" });
  // a click on a later row redoes up to there
  await app.clickOn(".hist-row:nth-child(4)");
  await app.waitFor(() => T.st.snapshot.peek().project.tempoBpm === 90, { what: "bis zum Tempo" });
});

test("editing after going back keeps the later steps – they come back as a variant", async () => {
  await fresh();
  await someSteps();
  await app.clickOn(".hist-row:nth-child(2)");
  await app.waitFor(() => T.$(".hist-row:nth-child(2)")?.classList.contains("current"), { what: "zurück" });
  await app.eval(async () => {
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Bass", kind: "instrument" } });
  });
  await app.waitFor(() => T.$$(".hist-row").length === 3 && !!T.$(".hist-fork.abandoned"), { what: "beiseitegelegte Schritte" });
  assert.equal(await app.eval(() => T.$(".hist-fork.abandoned").textContent.trim()), "2 Schritte");
  assert.deepEqual(await names(), ["Lead", "Bass"]);
  await app.clickOn(".hist-fork.abandoned");
  await app.waitFor(() => T.$$(".hist-var").length === 2, { what: "neue Variante" });
  assert.deepEqual(await variants(), [{ name: "Hauptversion", on: false }, { name: "Variante 2", on: true }]);
  assert.equal(await app.eval(() => T.st.snapshot.peek().project.tempoBpm), 90);
  assert.deepEqual(await labels(), ["Neues Projekt", "Spur hinzufügen", "Clip hinzufügen", "Tempo ändern"]);
  // the other variant branches off in row 2
  assert.equal(await app.eval(() => T.$(".hist-row:nth-child(2) .hist-fork").textContent.trim()), "Hauptversion");
  await app.clickOn(".hist-var", { text: "Hauptversion" });
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.map((t) => t.name).join() === "Lead,Bass", { what: "Hauptversion" });
});

test("checkpoint and new variant with the buttons; rename by double-click", async () => {
  await fresh();
  await someSteps();
  await app.clickOn(".hist-checkpoint");
  await app.waitFor(() => T.$(".hist-row.current .hist-flag")?.textContent.trim() === "Checkpoint 1", { what: "Checkpoint-Fahne" });
  // rename the checkpoint over its flag
  await app.clickOn(".hist-row.current .hist-flag");
  await app.waitFor(() => !!T.$(".modal .hist-name"), { what: "Dialog" });
  await app.eval(() => {
    const i = T.$(".modal .hist-name");
    i.value = "Thema fertig";
  });
  await app.clickOn(".modal .btn.primary");
  await app.waitFor(() => T.$(".hist-row.current .hist-flag")?.textContent.trim() === "Thema fertig", { what: "umbenannt" });
  // variant from row 2 (⑂ in the row)
  await app.eval(() => T.$(".hist-row:nth-child(2) .hist-branch-here").click());
  await app.waitFor(() => !!T.$(".modal .hist-name"), { what: "Varianten-Dialog" });
  await app.eval(() => {
    T.$(".modal .hist-name").value = "Ohne Melodie";
  });
  await app.clickOn(".modal .btn.primary");
  await app.waitFor(() => T.$$(".hist-var").length === 2, { what: "Variante" });
  assert.deepEqual(await variants(), [{ name: "Hauptversion", on: false }, { name: "Ohne Melodie", on: true }]);
  assert.equal(await app.eval(() => T.st.snapshot.peek().project.tracks[0].clips.length), 0);
  await app.eval(() => T.st.run({ type: "setTrack", trackId: T.st.snapshot.peek().project.tracks[0].id, patch: { gainDb: -6 } }));
  await app.waitFor(() => T.$$(".hist-row").length === 3, { what: "Schritt in der Variante" });
  assert.equal((await labels())[2], "Lautstärke ändern");
  // the checkpoint is listed in the menu even though it lies on the other variant
  await app.clickOn(".hist-more");
  assert.match(await app.eval(() => T.$(".hist-menu").textContent), /Thema fertig/);
  await app.clickOn(".hist-cp-item");
  await app.waitFor(() => T.$$(".hist-var.on")[0]?.textContent.includes("Hauptversion"), { what: "zum Checkpoint" });
  assert.equal(await app.eval(() => T.st.snapshot.peek().project.tempoBpm), 90);
  // rename a variant: double-click
  await app.eval(() => T.$$(".hist-var")[1].dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
  await app.waitFor(() => !!T.$(".modal .hist-name"), { what: "Umbenennen" });
  await app.eval(() => {
    T.$(".modal .hist-name").value = "Leise";
  });
  await app.clickOn(".modal .btn.primary");
  await app.waitFor(() => T.$$(".hist-var")[1]?.textContent.includes("Leise"), { what: "Variante umbenannt" });
  await app.shot("historie-varianten");
});

test("the history is saved in the project file and comes back after a reload", async () => {
  await fresh();
  await someSteps();
  await app.clickOn(".hist-checkpoint");
  await app.eval(async () => {
    const first = T.$$(".hist-row")[1].dataset.node;
    await T.st.act(T.backend.historyVariantNew("Zweite", Number(first)));
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Pad", kind: "instrument" } });
    await T.st.act(T.backend.projectSave("Historie E2E"));
    await T.web.sync();
  });
  const before = await rows();
  await app.reload();
  await app.eval(async () => {
    await T.st.act(T.backend.projectOpen("Historie E2E"));
    T.st.showPanel("historie");
  });
  await app.waitFor(() => T.$$(".hist-var").length === 2, { what: "Varianten nach dem Neuladen" });
  assert.deepEqual(await variants(), [{ name: "Hauptversion", on: false }, { name: "Zweite", on: true }]);
  assert.deepEqual((await rows()).map((r) => r.cells), before.map((r) => r.cells));
  await app.clickOn(".hist-var", { text: "Hauptversion" });
  await app.waitFor(() => T.$(".hist-row.current .hist-flag")?.textContent.trim() === "Checkpoint 1", { what: "Checkpoint nach dem Neuladen" });
  assert.equal(await app.eval(() => T.st.snapshot.peek().project.tempoBpm), 90);
  // undo works across the reload
  await app.key("z", { code: "KeyZ", vk: 90, mods: 2 });
  await app.waitFor(() => T.st.snapshot.peek().project.tempoBpm === 120, { what: "Rückgängig nach dem Neuladen" });
  await sleep(100);
  assert.deepEqual(app.errors, []);
});
