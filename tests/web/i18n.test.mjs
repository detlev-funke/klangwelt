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

/** Colors at points of each flag (viewBox 18×12), the same as in ui/src/flags.ts. */
const FLAG_COLORS = {
  en: [[5, 0.6, "#012169"], [9, 6, "#c8102e"], [9, 1, "#c8102e"]],
  de: [[9, 2, "#000000"], [9, 6, "#dd0000"], [9, 10, "#ffce00"]],
  fr: [[3, 6, "#0055a4"], [9, 6, "#ffffff"], [15, 6, "#ef4135"]],
  it: [[3, 6, "#009246"], [9, 6, "#ffffff"], [15, 6, "#ce2b37"]],
  es: [[9, 1.5, "#aa151b"], [9, 6, "#f1bf00"], [9, 10.5, "#aa151b"]],
  nl: [[9, 2, "#ae1c28"], [9, 6, "#ffffff"], [9, 10, "#21468b"]],
  pl: [[9, 3, "#ffffff"], [9, 9, "#dc143c"]],
};

/** Entries of the open language menu with their flags: size on screen and the
 *  colors the flag paints at the points of FLAG_COLORS (drawn into a canvas). */
const languageMenu = () =>
  app.eval(async (points) => {
    const hex = (d) => `#${[...d.slice(0, 3)].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
    const paint = async (svg) => {
      const copy = svg.cloneNode(true);
      copy.setAttribute("width", "180");
      copy.setAttribute("height", "120");
      const img = new Image();
      img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(new XMLSerializer().serializeToString(copy))}`;
      await img.decode();
      const c = document.createElement("canvas");
      c.width = 180;
      c.height = 120;
      const ctx = c.getContext("2d");
      ctx.drawImage(img, 0, 0, 180, 120);
      return (points[svg.dataset.flag] ?? []).map(([x, y]) => hex(ctx.getImageData(Math.round(x * 10), Math.round(y * 10), 1, 1).data));
    };
    const items = [...document.querySelectorAll(".set-menu.open [data-lang]")];
    const ids = [...document.querySelectorAll("[id]")].map((e) => e.id);
    return {
      duplicateIds: ids.filter((id, i) => ids.indexOf(id) !== i),
      items: await Promise.all(
        items.map(async (b) => {
          const svg = b.querySelector("svg.flag");
          const r = svg?.getBoundingClientRect();
          const name = b.querySelector(".dev-text b");
          return {
            lang: b.dataset.lang,
            name: b.textContent.trim(),
            on: b.classList.contains("on"),
            current: b.getAttribute("aria-current"),
            flag: svg?.dataset.flag ?? null,
            hidden: svg?.getAttribute("aria-hidden"),
            size: r ? [Math.round(r.width), Math.round(r.height)] : null,
            beforeName: !!svg && !!(svg.compareDocumentPosition(name) & Node.DOCUMENT_POSITION_FOLLOWING),
            colors: svg ? await paint(svg) : [],
          };
        }),
      ),
    };
  }, FLAG_COLORS);

/** Every entry shows its own flag (18×12, before the name, painted in its colors);
 *  only `current` is marked. */
function assertFlags(menu, current) {
  assert.deepEqual(menu.duplicateIds, []);
  for (const it of menu.items) {
    assert.equal(it.flag, it.lang, `flag of ${it.lang}`);
    assert.equal(it.hidden, "true", `flag of ${it.lang} is decoration (the name is beside it)`);
    assert.deepEqual(it.size, [18, 12], `size of the flag of ${it.lang}`);
    assert.ok(it.beforeName, `flag of ${it.lang} before the name`);
    assert.deepEqual(it.colors, FLAG_COLORS[it.lang].map((p) => p[2]), `colors of the flag of ${it.lang}`);
    assert.equal(it.on, it.lang === current, `${it.lang} marked`);
    assert.equal(it.current, it.lang === current ? "true" : null, `aria-current of ${it.lang}`);
  }
}

/** Open the settings menu and read the language entries. */
async function openLanguageMenu() {
  await app.clickOn('[data-help="einstellungen"]');
  await app.waitFor(() => document.querySelectorAll(".set-menu.open [data-lang]").length === 7, { what: "language menu" });
  return languageMenu();
}

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
  // menu: every language with its flag and in its own name, the current one marked
  const menu = await openLanguageMenu();
  assert.deepEqual(menu.items.map((i) => i.name), ["English", "Deutsch", "Français", "Italiano", "Español", "Nederlands", "Polski"]);
  assertFlags(menu, "de");
  await app.key("Escape");
  await app.click(5, 500);
  // opened again: the menu is rebuilt, still no id twice (the Union Jack's clip path)
  assertFlags(await openLanguageMenu(), "de");
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
  // the menu now marks English, with the Union Jack
  assertFlags(await openLanguageMenu(), "en");
  await app.click(5, 500);

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
