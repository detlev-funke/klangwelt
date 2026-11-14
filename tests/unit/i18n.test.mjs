// Translations (ui/src/i18n.ts) – detection, fallback, placeholders, plurals –
// and the dictionaries: every language knows every key from the UI and the core
// (ui/scripts/i18n-keys.mjs). Straight from the build (ui/dist).
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  LANGUAGES, SOURCE, FALLBACK, coreDictionary, detectLanguage, initI18n, language, plural, setLanguage, supported, t, tid, tk,
} from "../../ui/dist/i18n.js";
import { checkDict, collectKeys, loadDict } from "../../ui/scripts/i18n-keys.mjs";

test("detection: first supported language of the list, otherwise English", () => {
  assert.equal(detectLanguage(["de-AT", "en-US"]), "de");
  assert.equal(detectLanguage(["pt-BR", "fr-CA", "de"]), "fr");
  assert.equal(detectLanguage(["zh-CN", "ja"]), "en");
  assert.equal(detectLanguage([]), FALLBACK);
  assert.equal(detectLanguage(["PL_pl"]), "pl");
  assert.equal(supported("nl-BE"), "nl");
  assert.equal(supported("xx"), null);
  assert.equal(supported(null), null);
  assert.equal(SOURCE, "en");
  assert.deepEqual(LANGUAGES.map((l) => l.code), ["en", "de", "fr", "it", "es", "nl", "pl"]);
});

const dicts = {
  en: { Glocke: "Bell" },
  de: { Save: "Speichern", "Track {name}": "Spur {name}", "{n} notes": { one: "eine Note", other: "{n} Noten" } },
  fr: { Save: "Enregistrer", Glocke: "Cloche", "{n} notes": { one: "{n} note", other: "{n} notes" } },
  pl: { "{n} notes": { one: "{n} nuta", few: "{n} nuty", many: "{n} nut", other: "{n} nuty" } },
};
const load = async (code) => {
  if (!dicts[code]) throw new Error(`no dictionary ${code}`);
  return dicts[code];
};

test("a remembered choice wins over detection; fallback: English, then the key", async () => {
  assert.equal(await initI18n({ preferred: ["de-DE"], stored: "fr", load }), "fr");
  assert.equal(language(), "fr");
  assert.equal(t("Save"), "Enregistrer");
  assert.equal(t(tid("Glocke")), "Cloche");
  assert.equal(t("Nowhere"), "Nowhere");
  // Italian without entries: identifiers from English, everything else is the key
  assert.equal(await initI18n({ preferred: ["it", "de"], stored: null, load: async (c) => (c === "it" ? {} : load(c)) }), "it");
  assert.equal(t("Glocke"), "Bell");
  assert.equal(t("Save"), "Save");
  // missing dictionary: keys instead of a crash
  const warn = console.warn;
  console.warn = () => {};
  try {
    assert.equal(await initI18n({ preferred: ["nl"], stored: null, load }), "nl");
  } finally {
    console.warn = warn;
  }
  assert.equal(t("Save"), "Save");
  // German has a dictionary; English only its identifiers
  assert.equal(await initI18n({ preferred: ["de-CH"], stored: null, load }), "de");
  assert.equal(t("Save"), "Speichern");
  assert.equal(t("Glocke"), "Glocke");
  assert.equal(await initI18n({ preferred: ["en-GB"], stored: null, load }), "en");
  assert.equal(t("Save"), "Save");
  assert.equal(t("Glocke"), "Bell");
});

test("placeholders are filled, unknown ones stay", () => {
  setLanguage("de", dicts.de);
  assert.equal(t("Track {name}", { name: "Bass" }), "Spur Bass");
  assert.equal(t("Bar {n} of {m}", { n: 3 }), "Bar 3 of {m}");
  assert.equal(t("No placeholder {x}"), "No placeholder {x}");
  assert.equal(tk("Table"), "Table");
});

test("plurals follow the rules of the language", () => {
  setLanguage("en", dicts.en);
  assert.equal(plural(1, "one note", "{n} notes"), "one note");
  assert.equal(plural(4, "one note", "{n} notes"), "4 notes");
  setLanguage("de", dicts.de, dicts.en);
  assert.equal(plural(1, "one note", "{n} notes"), "eine Note");
  assert.equal(plural(7, "one note", "{n} notes"), "7 Noten");
  setLanguage("fr", dicts.fr, dicts.en);
  assert.equal(plural(0, "one note", "{n} notes"), "0 note");
  setLanguage("pl", dicts.pl, dicts.en);
  assert.deepEqual([1, 2, 5, 12, 22, 25].map((n) => plural(n, "one note", "{n} notes")), ["1 nuta", "2 nuty", "5 nut", "12 nut", "22 nuty", "25 nut"]);
  // without an entry: the forms from the code
  assert.equal(plural(1, "one bar", "{n} bars"), "one bar");
});

test("dictionary for the core: flat, chosen language over English, plural forms with a separator", () => {
  setLanguage("fr", dicts.fr, dicts.en);
  const d = coreDictionary();
  assert.equal(d.Save, "Enregistrer");
  assert.equal(d.Glocke, "Cloche");
  assert.equal(d["{n} notes"], "{n} notes");
  assert.equal(d["{n} notes\u001fone"], "{n} note");
  assert.ok(Object.values(d).every((v) => typeof v === "string"));
});

test("all keys can be read from the code", () => {
  const { keys, problems } = collectKeys();
  assert.deepEqual(problems, [], "calls without a string literal");
  assert.ok(keys.size > 1000, `only ${keys.size} keys found`);
  assert.ok(keys.has("Settings") && keys.has("Language"));
});

test("German leaves identifiers as they are (their values are German already)", async () => {
  const { keys } = collectKeys();
  const de = await loadDict("de");
  const changed = [...keys].filter(([k, e]) => e.id && k in de && de[k] !== k).map(([k]) => `${k} → ${de[k]}`);
  assert.deepEqual(changed, [], "a string used as identifier and as English key with different meanings");
});

// English needs only the identifiers (tid), German everything except them
// (they are German already), every other language all keys.
for (const { code } of LANGUAGES) {
  test(`dictionary ${code}: every key translated, no foreign placeholders`, async () => {
    const { keys: all } = collectKeys();
    const keys = new Map([...all].filter(([, e]) => (code === "en" ? e.id : code === "de" ? !e.id : true)));
    const { missing, badVars } = checkDict(await loadDict(code), keys);
    assert.deepEqual(missing.slice(0, 20), [], `${missing.length} keys missing in ${code} (node ui/scripts/i18n-keys.mjs --missing=${code})`);
    assert.deepEqual(badVars, []);
  });
}
