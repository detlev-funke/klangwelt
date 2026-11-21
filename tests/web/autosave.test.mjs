// Autosave and switching the language without losing the project (ui/src/autosave.ts,
// crates/klang-app/src/autosave.rs) – in the browser: projects in OPFS, the core starts
// anew with every page load. The interval is set in seconds through the module, so
// nothing waits for minutes; the harness switches autosave off for every other test.

import { after } from "node:test";
import assert from "node:assert/strict";
import { startWeb, makeTest, sleep } from "./harness.mjs";

let app;
const test = makeTest(() => app);

after(async () => {
  await app?.close();
});

const RECOVERY = "Wiederherstellung.klangwelt";

/** File, "unsaved" and track names of the open project. */
const state = () =>
  app.eval(() => {
    const s = T.st.snapshot.peek();
    return { file: s.file, dirty: s.dirty, tracks: s.project.tracks.map((t) => t.name) };
  });

const addTrack = (name) => app.eval((n) => T.st.run({ type: "addTrack", track: { id: 0, name: n, kind: "instrument" } }), name);
const setAutosave = (s) => app.eval(async (s) => (await import("./autosave.js")).setAutosaveSeconds(s), s);
const newProject = () => app.eval(() => T.st.act(T.backend.projectNew()));
/** Save under a name (in the browser's project folder); returns the file. */
const saveAs = (name) =>
  app.eval(async (n) => {
    await T.st.act(T.backend.projectSave(n));
    return T.st.snapshot.peek().file;
  }, name);

/** Is the file in the browser's storage (OPFS, folder Klangwelt)? Waits for pending writes first. */
const stored = (name) =>
  app.eval(async (n) => {
    await T.web.sync();
    const dir = await (await navigator.storage.getDirectory()).getDirectoryHandle("Klangwelt");
    try {
      await dir.getFileHandle(n);
      return true;
    } catch {
      return false;
    }
  }, name);

/** Size and time of a project file in the project folder. */
const fileInfo = (path) => app.eval(async (p) => (await T.backend.projectList())[1].find((f) => f.path === p) ?? null, path);

/** SHA-256 of a file in the browser's storage (path in the core's file tree, "/Klangwelt/…"). */
const digest = (path) =>
  app.eval(async (p) => {
    await T.web.sync();
    let dir = await navigator.storage.getDirectory();
    const parts = p.split("/").filter(Boolean);
    const name = parts.pop();
    for (const d of parts) dir = await dir.getDirectoryHandle(d);
    const bytes = await (await (await dir.getFileHandle(name)).getFile()).arrayBuffer();
    return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");
  }, path);

/** File → New project, as from the menu (asks about unsaved changes when it has to). */
const newFromMenu = () => app.eval(async () => void (await import("./components/fileops.js")).newProject());

/** The tracks in a project file – opened, then the project from before is opened again. */
const tracksInFile = (path) =>
  app.eval(async (p) => {
    const back = T.st.snapshot.peek().file;
    const opened = await T.backend.projectOpen(p);
    const names = opened.project.tracks.map((t) => t.name);
    if (back) await T.st.act(T.backend.projectOpen(back));
    return names;
  }, path);

async function openSettings() {
  await app.clickOn('[data-help="einstellungen"]');
  await app.waitFor(() => document.querySelectorAll(".set-menu.open [data-autosave]").length === 5, { what: "settings menu" });
}

async function chooseLanguage(code) {
  await openSettings();
  await app.clickOn(`.set-menu.open [data-lang="${code}"]`);
  await app.afterReload();
}

test("the settings menu offers autosave: off or every few minutes – every minute by default", async () => {
  app = await startWeb({ autosave: null });
  assert.equal(await app.eval(async () => (await import("./autosave.js")).autosaveSeconds.peek()), 60);
  await openSettings();
  const menu = await app.eval(() => ({
    titles: T.$$(".set-menu.open .menu-title").map((e) => e.textContent),
    options: T.$$(".set-menu.open [data-autosave]").map((b) => ({ text: b.textContent, on: b.classList.contains("on"), pressed: b.getAttribute("aria-pressed") })),
  }));
  assert.ok(menu.titles.includes("Automatisch speichern"), JSON.stringify(menu.titles));
  assert.deepEqual(menu.options.map((o) => o.text), ["Aus", "1 Min.", "2 Min.", "5 Min.", "10 Min."]);
  assert.deepEqual(menu.options.map((o) => o.on), [false, true, false, false, false], "one minute is marked");
  assert.equal(menu.options[1].pressed, "true");
  await app.clickOn('.set-menu.open [data-autosave="0"]');
  assert.equal(await app.eval(() => !!T.$(".set-menu.open")), false, "the menu closes");
  assert.equal(await app.eval(() => localStorage.getItem("klangwelt.autosave")), "0");
  // remembered across a reload
  await app.reload();
  await openSettings();
  assert.deepEqual(await app.eval(() => T.$$(".set-menu.open [data-autosave].on").map((b) => b.dataset.autosave)), ["0"]);
  await app.clickOn('.set-menu.open [data-autosave="300"]');
  assert.equal(await app.eval(async () => (await import("./autosave.js")).autosaveSeconds.peek()), 300);
  assert.deepEqual(app.errors, []);
});

test("a project with a file is saved into it in the background – only when it changed", async () => {
  await setAutosave(1);
  await newProject();
  await addTrack("Eins");
  const file = await saveAs("Autosave");
  assert.match(file, /Autosave\.klangwelt$/);
  const saved = await fileInfo(file);
  // nothing changed: the file isn't written again
  await sleep(2500);
  assert.deepEqual(await fileInfo(file), saved);
  await addTrack("Zwei");
  assert.equal((await state()).dirty, true);
  await app.waitFor(() => !T.st.snapshot.peek().dirty, { timeout: 6000, what: "saved by the autosave" });
  assert.equal((await state()).file, file, "into its own file");
  assert.equal(await app.eval(() => T.$("kw-transport .save-btn").classList.contains("dirty")), false, "no dot on the disk");
  assert.deepEqual(await tracksInFile(file), ["Eins", "Zwei"]);
  assert.equal(await stored(RECOVERY), false, "no recovery copy for a project with a file");
  assert.deepEqual(app.errors, []);
});

test("the autosave waits while playing and saves right after", async () => {
  await app.eval(async () => {
    await T.st.act(T.backend.projectNew());
    const plans = await T.backend.songTemplates();
    await T.backend.songWrite(plans[0], true);
    await T.st.act(T.backend.getState());
  });
  const file = await saveAs("Abspielen");
  await app.eval(async () => {
    await T.st.act(T.backend.setLoop(true));
    await T.backend.transport("play");
  });
  await app.waitFor(() => T.st.status.peek().playing, { what: "playing" });
  await addTrack("Während");
  await sleep(2500);
  const playing = await app.eval(() => ({ playing: T.st.status.peek().playing, dirty: T.st.snapshot.peek().dirty }));
  assert.deepEqual(playing, { playing: true, dirty: true }, "not saved while playing");
  await app.eval(async () => {
    await T.backend.transport("stop");
    await T.st.act(T.backend.setLoop(false));
  });
  await app.waitFor(() => !T.st.snapshot.peek().dirty, { timeout: 5000, what: "saved after stopping" });
  assert.ok((await tracksInFile(file)).includes("Während"));
  assert.deepEqual(app.errors, []);
});

test("a project without a name goes into a recovery copy and comes back after the tab was closed", async () => {
  await newProject();
  assert.equal(await stored(RECOVERY), false, "a new project removes an old copy");
  await addTrack("Skizze");
  await app.waitFor(async () => {
    await T.web.sync();
    const dir = await (await navigator.storage.getDirectory()).getDirectoryHandle("Klangwelt");
    return dir.getFileHandle("Wiederherstellung.klangwelt").then(() => true, () => false);
  }, { timeout: 6000, what: "recovery copy" });
  assert.deepEqual(await state(), { file: null, dirty: true, tracks: ["Skizze"] }, "still unsaved, still without a file");
  // like closing the tab: the browser asks (the harness confirms), the next start brings it back
  app.dialogs.length = 0;
  await app.reload();
  assert.deepEqual(app.dialogs, ["beforeunload"]);
  const notice = await app.waitFor(() => T.st.snapshot.peek().project.tracks.length && (T.st.message.peek()?.text ?? "-"), { what: "project back" });
  assert.match(notice, /ungespeicherte Projekt der letzten Sitzung wurde wiederhergestellt/);
  assert.deepEqual(await state(), { file: null, dirty: true, tracks: ["Skizze"] });
  // saved under a name: the copy is gone
  await saveAs("Skizze");
  assert.equal(await stored(RECOVERY), false);
  assert.deepEqual(app.errors, []);
});

test("switching the language keeps a project with a file: saved first, opened again", async () => {
  await setAutosave(600);
  await newProject();
  await addTrack("Datei");
  const file = await saveAs("Sprache");
  await addTrack("Ungespeichert");
  app.dialogs.length = 0;
  await chooseLanguage("en");
  assert.equal(await app.eval(async () => (await import("./i18n.js")).language()), "en");
  assert.deepEqual(app.dialogs, [], "no \"Leave page?\"");
  await app.waitFor((f) => T.st.snapshot.peek().file === f, { what: "the project again" }, file);
  assert.deepEqual(await state(), { file, dirty: false, tracks: ["Datei", "Ungespeichert"] }, "saved into its file before the reload");
  assert.equal(await app.eval(() => sessionStorage.getItem("klangwelt.resume")), null, "the note for the reload is used up");
  assert.deepEqual(app.errors, []);
});

test("with autosave off: nothing is saved in the background, a language switch leaves the file untouched and keeps the changes unsaved", async () => {
  await setAutosave(0);
  await newProject();
  await addTrack("A");
  const file = await saveAs("Unberührt");
  const before = await digest(file);
  await addTrack("B");
  await sleep(2500);
  assert.equal((await state()).dirty, true, "no autosave");
  app.dialogs.length = 0;
  await chooseLanguage("de");
  assert.deepEqual(app.dialogs, []);
  await app.waitFor((f) => T.st.snapshot.peek().file === f, { what: "the project again" }, file);
  assert.deepEqual(await state(), { file, dirty: true, tracks: ["A", "B"] }, "same file, changes still unsaved");
  assert.equal(await digest(file), before, "the file is as it was");
  assert.equal(await app.eval(() => T.$(".proj-name").textContent), "Unberührt •");
  // the question about unsaved changes is there as before
  await newFromMenu();
  await app.waitFor(() => T.$(".modal-title")?.textContent === "Ungespeicherte Änderungen", { what: "question" });
  await app.clickOn(".modal-actions .btn", { text: "Speichern" });
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.length === 0, { what: "new project" });
  assert.deepEqual(await tracksInFile(file), ["A", "B"], "saved on request");
  assert.equal(await stored(RECOVERY), false);
  assert.deepEqual(app.errors, []);
});

test("with autosave on, a project with a file is saved without asking before a new one", async () => {
  await setAutosave(600);
  await newProject();
  await addTrack("Erst");
  const file = await saveAs("Ohne Frage");
  await addTrack("Dann");
  await newFromMenu();
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.length === 0, { what: "new project" });
  assert.equal(await app.eval(() => !!T.$(".modal")), false, "no question");
  assert.deepEqual(await tracksInFile(file), ["Erst", "Dann"]);
  // a project without a name is still asked about – it needs one
  await newProject();
  await addTrack("Namenlos");
  await newFromMenu();
  await app.waitFor(() => T.$(".modal-title")?.textContent === "Ungespeicherte Änderungen", { what: "question" });
  await app.clickOn(".modal-actions .btn", { text: "Nicht speichern" });
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.length === 0, { what: "discarded" });
  assert.equal(await stored(RECOVERY), false);
  assert.deepEqual(app.errors, []);
});
