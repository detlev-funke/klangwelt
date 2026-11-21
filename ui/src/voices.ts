// Analysis mode for recordings: one line (the analysis as before) or several
// voices at once (core: klang-app `voices`, klang-dsp `poly`). The choice is
// remembered and applies live while recording, in the analysis window and when
// a recording becomes notes.

import { t } from "./i18n.js";
import { signal } from "./signals.js";
import type { VoicesAnalysis } from "./types.js";

export type AnalysisModeName = "mono" | "poly";

/** localStorage: analysis mode and the voice count ("auto" or 1–6). */
export const MODE_KEY = "klangwelt.analysemodus";
export const VOICES_KEY = "klangwelt.stimmen";
/** Voice counts offered (plus "auto"). */
export const VOICE_CHOICES: readonly string[] = ["auto", "2", "3", "4"];

function read(key: string, ok: (v: string) => boolean, fallback: string): string {
  try {
    const v = localStorage.getItem(key);
    if (v != null && ok(v)) return v;
  } catch {
    /* no storage */
  }
  return fallback;
}

function write(key: string, v: string): void {
  try {
    localStorage.setItem(key, v);
  } catch {
    /* without storage: for this session */
  }
}

export const analysisMode = signal<AnalysisModeName>(read(MODE_KEY, (v) => v === "mono" || v === "poly", "mono") as AnalysisModeName);
export const voiceCount = signal<string>(read(VOICES_KEY, (v) => v === "auto" || /^[1-6]$/.test(v), "auto"));
/** The polyphonic analysis of the recording shown in the analysis window. */
export const voicesAnalysis = signal<VoicesAnalysis | null>(null);

export function setAnalysisMode(m: AnalysisModeName): void {
  analysisMode.value = m;
  write(MODE_KEY, m);
}

export function setVoiceCount(v: string): void {
  voiceCount.value = v;
  write(VOICES_KEY, v);
}

/** The voice count as the core takes it. */
export function voicesArg(): string | number {
  const v = voiceCount.peek();
  return v === "auto" ? "auto" : Number(v);
}

/** Colors of the voices (the highest first). */
export const VOICE_COLORS: readonly string[] = ["#ffb454", "#6ee7d8", "#c792ea", "#82aaff", "#f07178", "#c3e88d"];

export function voiceColor(i: number): string {
  return VOICE_COLORS[i % VOICE_COLORS.length]!;
}

/** "Voice 1" … for the legend and the facts. */
export function voiceName(i: number): string {
  return t("Voice {n}", { n: i + 1 });
}
