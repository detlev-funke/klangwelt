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

test("the history survives a reload after view changes, and a checkpoint alone is saved too", async () => {
  await fresh();
  await someSteps();
  const lead = await app.eval(() => T.st.snapshot.peek().project.tracks[0].id);
  // no undo steps: compact view, a row height – the history used to be refused after these
  await app.eval(async (id) => {
    await T.st.run({ type: "setTrack", trackId: id, patch: { view: "compact" } });
    await T.st.run({ type: "setTrack", trackId: id, patch: { compactHeight: 110 } });
  }, lead);
  await app.clickOn(".hist-checkpoint");
  // without a file: the recovery copy, as the autosave writes it – then reloaded like a crash
  await app.eval(async () => {
    await (await import("./autosave.js")).autosaveNow();
    await T.web.sync();
  });
  const before = await rows();
  await app.reload();
  await app.eval(() => T.st.showPanel("historie"));
  await app.waitFor(() => T.$$(".hist-row").length === 4, { what: "history after the reload" });
  assert.deepEqual((await rows()).map((r) => r.cells), before.map((r) => r.cells));
  assert.equal(await app.eval(() => T.$(".hist-row.current .hist-flag")?.textContent.trim()), "Checkpoint 1");
  assert.equal(await app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).compactHeight, lead), 110, "the view came along");

  // saved under a name, then only a checkpoint: that is unsaved, and the autosave keeps it
  await app.eval(async () => {
    await T.st.act(T.backend.projectSave("Historie Checkpoint"));
    await T.backend.historyCheckpoint("Nach dem Speichern");
  });
  assert.equal(await app.eval(async () => (await T.backend.getState()).dirty), true, "a checkpoint is an unsaved change");
  await app.eval(async () => {
    await (await import("./autosave.js")).autosaveNow();
    await T.web.sync();
  });
  assert.equal(await app.eval(() => T.st.snapshot.peek().dirty), false, "saved into the file");
  await app.reload();
  await app.eval(async () => {
    await T.st.act(T.backend.projectOpen("Historie Checkpoint"));
    T.st.showPanel("historie");
  });
  await app.waitFor(() => T.$$(".hist-row .hist-flag").length === 2, { what: "both checkpoints after the reload" });
  assert.deepEqual(await app.eval(() => T.$$(".hist-row .hist-flag").map((f) => f.textContent.trim())), ["Checkpoint 1", "Nach dem Speichern"]);
  assert.deepEqual(app.errors, []);
});

// ---------------------------------------------------------------- Combining variants

/** Lead with one note per bar (8 bars) and a checkpoint; the main version
 *  changes bars 5–6, the variant "Refrain" bars 6–8 and the lead's sound and adds a pad. */
async function twoVariants() {
  await fresh();
  await app.eval(async () => {
    const notes = (pitches) => pitches.map((p, i) => ({ pitch: p, startBeats: i * 4, lengthBeats: 1, velocity: 90 }));
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Lead", kind: "instrument" } });
    const t = T.st.snapshot.peek().project.tracks[0];
    await T.st.run({ type: "addClip", trackId: t.id, clip: { id: 0, name: "Thema", startBeats: 0, source: { type: "notes", lengthBeats: 32, notes: notes([60, 60, 60, 60, 60, 60, 60, 60]) } } });
    const v = await T.backend.historyCheckpoint("Rohfassung");
    const base = v.current;
    const clip = () => T.st.snapshot.peek().project.tracks[0].clips[0].id;
    await T.st.run({ type: "setClipNotes", trackId: t.id, clipId: clip(), notes: notes([60, 60, 60, 60, 62, 62, 60, 60]) });
    await T.st.act(T.backend.historyVariantNew("Refrain", base));
    await T.st.run({ type: "setClipNotes", trackId: t.id, clipId: clip(), notes: notes([60, 60, 60, 60, 60, 67, 67, 67]) });
    const inst = { ...T.st.snapshot.peek().project.tracks[0].instrument, preset: "dunkel" };
    await T.st.run({ type: "setTrack", trackId: t.id, patch: { instrument: inst } });
    await T.st.run({ type: "addTrack", track: { id: 0, name: "Pad", kind: "instrument", clips: [{ id: 0, name: "P", startBeats: 16, source: { type: "notes", lengthBeats: 16, notes: [{ pitch: 48, startBeats: 0, lengthBeats: 8, velocity: 80 }] } }] } });
  });
  await app.waitFor(() => T.$$(".hist-var").length === 2 && !T.$(".hist-combine").disabled, { what: "zwei Varianten" });
}

const cells = () => app.eval(() => T.$$(".mg-row[data-track] .mg-cell").map((c) => ({
  track: c.dataset.track,
  start: Number(c.dataset.start),
  text: c.querySelector(".mg-cell-text").textContent,
  conflict: c.classList.contains("conflict") && !c.classList.contains("resolved"),
})));
async function pick(selector, source) {
  await app.clickOn(selector);
  await app.waitFor(() => !!T.$(".mg-pop"), { what: "Auswahl" });
  await app.clickOn(`.mg-pop .mg-opt[data-source="${source}"]`);
}

test("combine: changed bars per track from both variants, the conflict has to be decided", async () => {
  await twoVariants();
  await app.clickOn(".hist-combine");
  await app.waitFor(() => T.$$(".mg-row[data-track]").length === 2, { what: "Kombinieren-Ansicht" });
  // the base is the active variant (the refrain), the main version takes part
  const ids = await app.eval(() => {
    const id = (name) => Number([...T.$(".mg-target").options].find((o) => o.textContent === name).value);
    return { main: id("Hauptversion"), refrain: id("Refrain"), base: Number(T.$(".mg-target").value), pad: T.$$(".mg-row[data-track]")[1].dataset.track };
  });
  assert.equal(ids.base, ids.refrain);
  assert.equal(await app.eval(() => T.$(".mg-chip.on").textContent), "Hauptversion");
  assert.match(await app.eval(() => T.$(".mg-origin").textContent), /Ursprung: Schritt 2 · Clip hinzufügen.*Rohfassung/);
  assert.deepEqual(await app.eval(() => T.$$(".mg-row[data-track] .mg-name").map((n) => n.textContent)), ["Lead", "Pad"]);
  assert.deepEqual(await cells(), [
    { track: "1", start: 16, text: "Hauptversion", conflict: false },
    { track: "1", start: 20, text: "Entscheiden", conflict: true },
    { track: "1", start: 24, text: "Refrain", conflict: false },
    { track: ids.pad, start: 16, text: "Refrain", conflict: false },
  ]);
  // the lead's sound comes from the refrain (only it changed it); the pad is new there
  assert.match(await app.eval(() => T.$('.mg-track-chip[data-decision="track:1"]').textContent), /Refrain · Klang/);
  assert.equal(await app.eval(() => T.$(".mg-apply").disabled), true);
  assert.match(await app.eval(() => T.$(".mg-summary").textContent), /1 Konflikt offen/);
  // lanes: the notes of every version; picking there works like the fields
  await app.clickOn('.mg-row[data-track="1"] .mg-expand');
  await app.waitFor(() => T.$$(".mg-sub").length === 3, { what: "Lanes" });
  assert.deepEqual(await app.eval(() => T.$$(".mg-sub .mg-name").map((n) => n.textContent)), ["Ursprung", "Hauptversion", "Refrain"]);
  await app.clickOn(`.mg-pick.can[data-start="20"][data-source="${ids.refrain}"]`);
  await app.waitFor(() => !T.$(".mg-apply").disabled, { what: "Konflikt entschieden" });
  assert.match(await app.eval(() => T.$(".mg-summary").textContent), /alles entschieden/);
  // the ruler sets a section for all tracks: bars 5–6 from the origin (lead and pad)
  await pick('.mg-rseg[data-start="16"]', 0);
  assert.deepEqual((await cells()).map((c) => c.text), ["unverändert", "Refrain", "Refrain", "weggelassen"]);
  await pick('.mg-row[data-track="1"] .mg-cell[data-start="16"]', ids.main);
  await pick(`.mg-row[data-track="${ids.pad}"] .mg-cell`, ids.refrain);
  assert.deepEqual((await cells()).map((c) => c.text), ["Hauptversion", "Refrain", "Refrain", "Refrain"]);
  await app.shot("historie-kombinieren");
  // listen: plays the combination, stopping returns to the project
  await app.clickOn(".mg-play");
  await app.waitFor(() => T.st.status.peek().playing, { what: "Probehören" });
  await app.waitFor(() => !T.$(".mg-head").hidden, { what: "Laufbalken" });
  await app.clickOn(".mg-play");
  await app.waitFor(() => !T.st.status.peek().playing, { what: "Stopp" });
  // combine: a new variant with one step, the table is back
  await app.clickOn(".mg-apply");
  await app.waitFor(() => T.$$(".hist-var").length === 3 && !!T.$(".hist-table"), { what: "kombinierte Variante" });
  assert.deepEqual(await variants(), [{ name: "Hauptversion", on: false }, { name: "Refrain", on: false }, { name: "Refrain + Hauptversion", on: true }]);
  assert.equal((await labels()).at(-1), "Kombiniert: Hauptversion");
  const p = await app.eval(() => T.st.snapshot.peek().project);
  assert.deepEqual(p.tracks.map((t) => t.name), ["Lead", "Pad"]);
  assert.deepEqual(p.tracks[0].clips[0].source.notes.map((n) => n.pitch), [60, 60, 60, 60, 62, 67, 67, 67]);
  assert.equal(p.tracks[0].instrument.preset, "dunkel");
  assert.deepEqual(app.errors, []);
});

test("combine: another base, at least one more variant, cancel changes nothing", async () => {
  await twoVariants();
  const before = await app.eval(() => JSON.stringify(T.st.snapshot.peek().project));
  await app.clickOn(".hist-combine");
  await app.waitFor(() => T.$$(".mg-row[data-track]").length === 2, { what: "Kombinieren-Ansicht" });
  // base: the main version – the refrain is the one added now
  await app.eval(() => {
    const s = T.$(".mg-target");
    s.value = [...s.options].find((o) => o.textContent === "Hauptversion").value;
    s.dispatchEvent(new Event("change"));
  });
  await app.waitFor(() => T.$(".mg-chip")?.textContent.includes("Refrain") && T.$$(".mg-row[data-track] .mg-cell").length === 4, { what: "Basis Hauptversion" });
  assert.match(await app.eval(() => T.$(".mg-apply").title), /rot markierten Felder/);
  assert.match(await app.eval(() => T.$(".mg-summary").textContent), /1 Konflikt offen/);
  // the only other variant can't be left out
  await app.clickOn(".mg-chip");
  assert.equal(await app.eval(() => T.$$(".mg-chip.on").length), 1);
  await app.clickOn(".mg-back");
  await app.waitFor(() => !!T.$(".hist-table"), { what: "Tabelle" });
  assert.equal(await app.eval(() => JSON.stringify(T.st.snapshot.peek().project)), before);
  assert.equal(await app.eval(() => T.$$(".hist-var").length), 2);
});
