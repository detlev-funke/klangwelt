// Note, chord and key names for display (ui/src/spelling.ts): the core spells them
// the German way; English shows letters with ♯/♭, Dutch its own spelling, German
// keeps them – and typed input goes back to the core in German.
import { test } from "node:test";
import assert from "node:assert/strict";
import { setLanguage } from "../../ui/dist/i18n.js";
import { chordName, chordsInput, chordsText, keyInput, keyText } from "../../ui/dist/spelling.js";
import { loadDict } from "../../ui/scripts/i18n-keys.mjs";

const use = async (code) => setLanguage(code, code === "en" ? await loadDict("en") : await loadDict(code), await loadDict("en"));

test("English: letters with ♯/♭, H is B, B is B♭, minor stays lower case", async () => {
  await use("en");
  assert.deepEqual(["Dis", "fis", "H", "h°7", "B7", "b", "Es", "as", "Eses", "Fisis", "Asus4", "D7/Fis", "cis4", "VII"].map(chordName),
    ["D♯", "f♯", "B", "b°7", "B♭7", "b♭", "E♭", "a♭", "E𝄫", "F𝄪", "Asus4", "D7/F♯", "c♯4", "VII"]);
  assert.equal(chordsText("a F C G7 h°"), "a F C G7 b°");
  assert.equal(keyText("a-Moll"), "A minor");
  assert.equal(keyText("Fis-Dur"), "F♯ major");
  assert.equal(keyText("b-Moll"), "B♭ minor");
});

test("English input goes back to the core in German spelling", async () => {
  await use("en");
  assert.equal(chordsInput("Am F C G7 B Bb F#m Eb7 b°"), "Am F C G7 H B Fism Es7 h°");
  assert.equal(chordsInput("a F C G"), "a F C G");
  assert.equal(keyInput("A minor"), "a-Moll");
  assert.equal(keyInput("B minor"), "h-Moll");
  assert.equal(keyInput("Bb major"), "B-Dur");
  assert.equal(keyInput("F♯ minor"), "fis-Moll");
  assert.equal(keyInput("a-Moll"), "a-Moll", "German input is still understood by the core");
});

test("Dutch: B is B natural, Bes is B♭", async () => {
  await use("nl");
  assert.deepEqual(["H", "B", "b", "Fis", "Es", "As7"].map(chordName), ["B", "Bes", "bes", "Fis", "Es", "As7"]);
  assert.equal(chordsInput("B Bes fis"), "H B fis");
  assert.equal(keyInput(keyText("h-Moll")), "h-Moll");
});

test("German and Polish keep the German names", async () => {
  await use("de");
  assert.equal(chordName("fis"), "fis");
  assert.equal(chordsInput("a F H"), "a F H");
  assert.equal(keyText("a-Moll"), "a-Moll");
  assert.equal(keyInput("fis-Moll"), "fis-Moll");
  await use("pl");
  assert.equal(chordName("H7"), "H7");
});
