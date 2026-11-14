// Languages (ui/src/i18n.ts): the UI detects its language from the browser's
// preferred languages (navigator.languages), can be switched in the settings
// menu and remembers the choice across reloads. The core speaks the same
// language (command set_language).
import { after } from "node:test";
import assert from "node:assert/strict";
import { startWeb, makeTest } from "./harness.mjs";

let app;
const test = makeTest(() => app);
after(async () => {
  await app?.close();
});

/** Language and a few UI texts; `expected` = what the dictionary gives for them. */
const ui = () =>
  app.eval(async () => {
    const i18n = await import("./i18n.js");
    let stored = null;
    try {
      stored = localStorage.getItem(i18n.STORAGE_KEY);
    } catch {
      /* no storage */
    }
    return {
      lang: document.documentElement.lang,
      current: i18n.language(),
      settings: document.querySelector('[data-help="einstellungen"]')?.title,
      expected: i18n.t("Settings"),
      stored,
    };
  });

/** Error message of the core (unknown track) and what the dictionary says for it. */
const coreError = () =>
  app.eval(async () => {
    const i18n = await import("./i18n.js");
    let text = null;
    try {
      await T.backend.execute({ type: "removeTrack", trackId: 424242 });
    } catch (e) {
      text = T.st.errorText(e);
    }
    return { text, expected: i18n.t("Track {id} not found", { id: 424242 }) };
  });

/** Choose a language in the settings menu. */
async function choose(code) {
  await app.clickOn('[data-help="einstellungen"]');
  await app.waitFor(() => document.querySelectorAll(".set-menu.open [data-lang]").length === 7, { what: "language menu" });
  await app.clickOn(`.set-menu.open [data-lang="${code}"]`);
}

test("French browser: UI and core in French, the detected language isn't stored", async () => {
  app = await startWeb({ languages: "fr-FR,fr,en" });
  const u = await ui();
  assert.equal(u.lang, "fr");
  assert.equal(u.current, "fr");
  assert.notEqual(u.settings, "Settings");
  assert.equal(u.settings, u.expected);
  assert.equal(u.stored, null, "only a choice in the menu is stored, not the detected language");
  const err = await coreError();
  assert.equal(err.text, err.expected);
  assert.notEqual(err.text, "Track 424242 not found");
  assert.deepEqual(app.errors, []);
  await app.close();
  app = null;
});

test("unsupported language: English", async () => {
  app = await startWeb({ languages: "ja-JP,ja" });
  const u = await ui();
  assert.equal(u.lang, "en");
  assert.equal(u.settings, "Settings");
  assert.equal((await coreError()).text, "Track 424242 not found");
  // identifiers (German values) are translated for display in English, too
  const preset = await app.eval(async () => (await import("./i18n.js")).t("Glocke"));
  assert.equal(preset, "Bell");
  await app.close();
  app = null;
});

test("English: note names, keys, chord track and sounds without German leftovers", async () => {
  app = await startWeb({ languages: "en-US,en" });
  await app.eval(async () => {
    await T.st.act(T.backend.createDemo());
    T.st.showPanel("harmonie");
    T.st.showPanel("song");
  });
  const r = await app.waitFor(() => {
    const names = T.$$(".hc-name").map((e) => e.textContent);
    const key = T.$(".song-key")?.value;
    if (!names.length || !key) return null;
    return {
      names,
      key,
      chords: T.$(".song-chords")?.value,
      sounds: T.$$(".song-layer span").map((e) => e.textContent),
      targets: T.$$(".hc-select option").map((o) => o.textContent),
    };
  }, { timeout: 20000, what: "harmony and song windows" });
  // circle of fifths: letters with ♯/♭, no German H, Dis, fis …
  assert.ok(r.names.some((n) => /[♯♭]/.test(n)), JSON.stringify(r.names));
  assert.deepEqual(r.names.filter((n) => /^[Hh]|is|^[A-Ga-g]es/.test(n)), [], JSON.stringify(r.names));
  assert.match(r.key, / (major|minor)$/);
  assert.ok(!/\bH\b/.test(r.chords), r.chords);
  assert.deepEqual(r.sounds.filter((s) => ["Ozean", "Glas", "Streicher", "Pluck-Bass"].includes(s)), [], JSON.stringify(r.sounds));
  assert.ok(r.targets.includes('Track "Chords"'), JSON.stringify(r.targets));
  assert.deepEqual(app.errors, []);
  await app.close();
  app = null;
});

test("switch the language in the settings menu; the choice survives a reload", async () => {
  app = await startWeb();
  assert.equal((await ui()).settings, "Einstellungen");
  // menu: every language in its own name, the current one marked
  await app.clickOn('[data-help="einstellungen"]');
  const menu = await app.waitFor(() => {
    const items = [...document.querySelectorAll(".set-menu.open [data-lang]")];
    return items.length && { names: items.map((b) => b.textContent.trim()), on: items.filter((b) => b.classList.contains("on")).map((b) => b.dataset.lang) };
  }, { what: "language menu" });
  assert.deepEqual(menu.names, ["English", "Deutsch", "Français", "Italiano", "Español", "Nederlands", "Polski"]);
  assert.deepEqual(menu.on, ["de"]);
  await app.key("Escape");
  await app.click(5, 500);

  // unsaved change: the browser asks before reloading (the harness confirms)
  await app.eval(async () => {
    await T.st.act(T.backend.createDemo());
  });
  await choose("en");
  await app.afterReload();
  let u = await ui();
  assert.equal(u.lang, "en");
  assert.equal(u.settings, "Settings");
  assert.equal(u.stored, "en");
  assert.ok(app.dialogs.includes("beforeunload"), `expected a question before reloading, got: ${JSON.stringify(app.dialogs)}`);
  assert.equal((await coreError()).text, "Track 424242 not found");

  // remembered: English after an ordinary reload, too
  await app.reload();
  u = await ui();
  assert.equal(u.lang, "en");
  assert.equal(u.settings, "Settings");

  // and back
  await choose("de");
  await app.afterReload();
  u = await ui();
  assert.equal(u.lang, "de");
  assert.equal(u.settings, "Einstellungen");
  assert.equal(u.stored, "de");
  assert.equal((await coreError()).text, "Spur 424242 nicht gefunden");
  // German identifiers stay as they are
  assert.equal(await app.eval(async () => (await import("./i18n.js")).t("Glocke")), "Glocke");
  assert.deepEqual(app.errors, []);
});
