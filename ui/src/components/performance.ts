// How detected notes were played, drawn and summed up the same way in the
// finished analysis and the live view: dynamics as the note's opacity,
// accents (>) and sforzati (sfz) above the note, a staccato dot, a slur for
// legato. Musical marks (pp … ff, >, sfz, staccato, legato) read the same in
// every language.

import { dynamicName } from "../expression.js";
import type { Articulation, Attack } from "../types.js";

export interface PlayedNote {
  startSecs: number;
  endSecs: number;
  midi: number;
  velocity: number;
  attack: Attack;
  articulation: Articulation;
}

/** Opacity of a note bar: soft notes fainter, loud ones solid. */
export function noteAlpha(velocity: number): number {
  return 0.3 + 0.7 * Math.min(1, Math.max(0, (velocity - 16) / (120 - 16)));
}

/**
 * Marks above/at a note bar from `x0` to `x1` around `y` (height `nh`);
 * `nextX` is where the next note starts (for the legato slur).
 */
export function drawMarks(ctx: CanvasRenderingContext2D, n: PlayedNote, x0: number, x1: number, y: number, nh: number, nextX: number | null, nextY: number | null): void {
  ctx.save();
  ctx.fillStyle = "#e8ebf1";
  ctx.strokeStyle = "#e8ebf1";
  ctx.lineWidth = 1.2;
  const top = y - nh / 2 - 3;
  if (n.attack === "accent") {
    // >
    ctx.beginPath();
    ctx.moveTo(x0, top - 7);
    ctx.lineTo(x0 + 7, top - 4);
    ctx.lineTo(x0, top - 1);
    ctx.stroke();
  } else if (n.attack === "sforzato") {
    ctx.font = "italic bold 10px Georgia, serif";
    ctx.textBaseline = "alphabetic";
    ctx.fillStyle = "#ff8a5c";
    ctx.fillText("sfz", x0, top - 1);
  }
  if (n.articulation === "staccato") {
    ctx.beginPath();
    ctx.arc((x0 + x1) / 2, y + nh / 2 + 4, 1.6, 0, Math.PI * 2);
    ctx.fill();
  } else if (n.articulation === "legato" && nextX != null && nextY != null) {
    // slur from this note to the next, below the bars
    const below = Math.max(y, nextY) + nh / 2 + 3;
    ctx.globalAlpha = 0.6;
    ctx.beginPath();
    ctx.moveTo((x0 + x1) / 2, below);
    ctx.quadraticCurveTo((x1 + nextX) / 2, below + 6, nextX + 4, below);
    ctx.stroke();
  }
  ctx.restore();
}

/** "p – f": the range of dynamics (null without notes). */
export function dynamicsRange(notes: PlayedNote[]): { text: string; from: number; to: number } | null {
  if (!notes.length) return null;
  const v = notes.map((n) => n.velocity);
  const from = Math.min(...v);
  const to = Math.max(...v);
  const a = dynamicName(from);
  const b = dynamicName(to);
  return { text: a === b ? a : `${a} – ${b}`, from, to };
}

/** "> 2 · sfz 1" (empty when no attack stands out). */
export function accentsText(notes: PlayedNote[]): string {
  const acc = notes.filter((n) => n.attack === "accent").length;
  const sfz = notes.filter((n) => n.attack === "sforzato").length;
  return [acc ? `> ${acc}` : "", sfz ? `sfz ${sfz}` : ""].filter(Boolean).join(" · ");
}

/** "staccato 3 · legato 5" (empty when all notes are of normal length). */
export function articulationText(notes: PlayedNote[]): string {
  const st = notes.filter((n) => n.articulation === "staccato").length;
  const lg = notes.filter((n) => n.articulation === "legato").length;
  return [st ? `staccato ${st}` : "", lg ? `legato ${lg}` : ""].filter(Boolean).join(" · ");
}
