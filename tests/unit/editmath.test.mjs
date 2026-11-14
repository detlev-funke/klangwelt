// Reine Rechenhilfen der Zeitleiste (ui/src/editmath.ts) – direkt aus dem Build (ui/dist).
import { test } from "node:test";
import assert from "node:assert/strict";
import { clipSpan, fmtShift, rangeGrid, snapTo } from "../../ui/dist/editmath.js";

const n = (startBeats, lengthBeats = 1) => ({ startBeats, lengthBeats });

test("snapTo rundet aufs Raster, 0 = frei", () => {
  assert.equal(snapTo(1.13, 0.25), 1.25);
  assert.equal(snapTo(1.12, 0.25), 1);
  assert.equal(snapTo(1.13, 0), 1.13);
});

test("fmtShift: Takte und Schläge lesbar", () => {
  assert.equal(fmtShift(0, 4), "±0");
  assert.equal(fmtShift(1, 4), "+1 Schlag");
  assert.equal(fmtShift(-2, 4), "−2 Schläge");
  assert.equal(fmtShift(4, 4), "+1 Takt");
  assert.equal(fmtShift(9.5, 4), "+2 Takte 1,5 Schläge");
  assert.equal(fmtShift(-0.25, 3), "−0,25 Schläge");
});

test("rangeGrid: Schläge, bei engem Zoom Takte, frei mit Alt", () => {
  assert.equal(rangeGrid(60, 4, false), 1);
  assert.equal(rangeGrid(12, 4, false), 4);
  assert.equal(rangeGrid(12, 3, true), 0);
});

test("clipSpan: Noten im Clip ändern nichts", () => {
  assert.deepEqual(clipSpan([n(0), n(3)], 4, 4, 8), { lo: 0, hi: 4 });
});

test("clipSpan: Note hinter dem Ende verlängert auf ganze Takte", () => {
  assert.deepEqual(clipSpan([n(0), n(4.5)], 4, 4, 0), { lo: 0, hi: 8 });
  assert.deepEqual(clipSpan([n(7.5, 1)], 4, 4, 0), { lo: 0, hi: 12 }, "Ende 8,5 → drei Takte");
  assert.deepEqual(clipSpan([n(0), n(4)], 3, 3, 0), { lo: 0, hi: 6 }, "Dreiertakt");
});

test("clipSpan: Note vor dem Anfang erweitert um ganze Takte nach links", () => {
  assert.deepEqual(clipSpan([n(-1), n(2)], 4, 4, 8), { lo: -4, hi: 4 });
  assert.deepEqual(clipSpan([n(-5)], 4, 4, 8), { lo: -8, hi: 4 });
});

test("clipSpan: nach links höchstens bis zum Projektanfang", () => {
  assert.deepEqual(clipSpan([n(-1.5)], 4, 4, 2), { lo: -2, hi: 4 });
});

test("clipSpan: beides zugleich", () => {
  assert.deepEqual(clipSpan([n(-1), n(5)], 4, 4, 4), { lo: -4, hi: 8 });
});

test("barBeat und rangeLabel: Takte lesbar", async () => {
  const { barBeat, rangeLabel } = await import("../../ui/dist/editmath.js");
  assert.equal(barBeat(0, 4), "1.1");
  assert.equal(barBeat(6.5, 4), "2.3,5");
  assert.equal(rangeLabel(4, 8, 4), "Takt 2");
  assert.equal(rangeLabel(4, 12, 4), "Takt 2–3");
  assert.equal(rangeLabel(2, 5, 4), "Takt 1.3 – 2.2");
});

test("gridLabel/gridOf: Raster als Notenwert", async () => {
  const { gridLabel, gridOf } = await import("../../ui/dist/editmath.js");
  assert.equal(gridLabel(0.5, 4), "1/8");
  assert.equal(gridLabel(1, 4), "1/4");
  assert.equal(gridLabel(0.25, 4), "1/16");
  assert.equal(gridLabel(1, 8), "1/8");
  assert.equal(gridOf(16, 4), 0.25);
  assert.equal(gridOf(8, 8), 1);
});

test("files: Audio, Partituren, Ordner", async () => {
  const { isAudioPath, isScorePath, baseName } = await import("../../ui/dist/files.js");
  assert.ok(isAudioPath("C:\\Musik\\Lied.WAV"));
  assert.ok(isAudioPath("/a/b.flac") && isAudioPath("x.mp3") && isAudioPath("x.opus"));
  assert.ok(!isAudioPath("x.mxl") && !isAudioPath("ordner"));
  assert.ok(isScorePath("x.musicxml") && isScorePath("C:\\Noten") && !isScorePath("x.wav"));
  assert.equal(baseName("C:\\Musik\\Lied.wav"), "Lied.wav");
});
