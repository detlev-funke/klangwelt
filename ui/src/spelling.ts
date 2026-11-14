// Note, chord and key names for display. The core spells them the German way –
// B = B♭, H = B, Cis/Es, minor chords in lower case ("fis", "h°7"), keys like
// "a-Moll" – and that is also its input format (song chords, the agent). For
// display, English, French, Italian and Spanish use letters with ♯/♭, Dutch its
// own spelling (B = B natural, Bes = B♭); German and Polish keep the German
// names. Input typed in the current spelling goes back to the core in German.
// The core does the same for names inside its messages (theory::display_name).

import { language, t } from "./i18n.js";

export type Spelling = "german" | "dutch" | "letters";

export function spelling(lang = language()): Spelling {
  return lang === "de" || lang === "pl" ? "german" : lang === "nl" ? "dutch" : "letters";
}

interface Root {
  /** C D E F G A B (B is B natural). */
  letter: string;
  /** −2 … +2 semitones. */
  acc: number;
  lower: boolean;
  rest: string;
}

/** German root at the start of a name ("fis°" → F♯, lower, "°"), or null. */
function parseGerman(name: string): Root | null {
  const m = /^([A-Ha-h])([\s\S]*)$/.exec(name);
  if (!m) return null;
  const l = m[1]!;
  let rest = m[2]!;
  const lower = l === l.toLowerCase();
  let letter = l.toUpperCase();
  if (letter === "B") return { letter: "B", acc: -1, lower, rest }; // German B = B♭
  if (letter === "H") letter = "B";
  let acc = 0;
  if (!rest.startsWith("sus")) {
    const flatS = letter === "A" || letter === "E"; // "As", "Es"
    const m2 = /^(isis|is|eses|es|ses|s)/.exec(rest);
    const suffix = m2?.[1];
    if (suffix === "isis") acc = 2;
    else if (suffix === "is") acc = 1;
    else if (suffix === "eses" || (suffix === "ses" && flatS)) acc = -2;
    else if (suffix === "es" || (suffix === "s" && flatS)) acc = -1;
    if (acc !== 0) rest = rest.slice(suffix!.length);
  }
  return { letter, acc, lower, rest };
}

/** Root as typed in a spelling ("F#m", "Bb", "Fis", "Bes"), or null. */
function parseInput(name: string, sp: Spelling): Root | null {
  if (sp === "german") return parseGerman(name);
  if (sp === "dutch") {
    const m = /^([A-Ga-g])(isis|is|eses|es|ses|s)?([\s\S]*)$/.exec(name);
    if (!m) return null;
    const l = m[1]!;
    const letter = l.toUpperCase();
    const suffix = m[2];
    const flatS = letter === "A" || letter === "E";
    let acc = 0;
    let rest = m[3]!;
    if (suffix === "isis") acc = 2;
    else if (suffix === "is") acc = 1;
    else if (suffix === "eses" || (suffix === "ses" && flatS)) acc = -2;
    else if (suffix === "es" || (suffix === "s" && flatS)) acc = -1;
    else if (suffix) rest = suffix + rest; // "s…" after other letters belongs to the rest ("Csus4")
    return { letter, acc, lower: l === l.toLowerCase(), rest };
  }
  const m = /^([A-Ga-g])(𝄪|##|x|♯|#|𝄫|bb|♭|b)?([\s\S]*)$/.exec(name);
  if (!m) return null;
  const l = m[1]!;
  const acc = { "𝄪": 2, "##": 2, x: 2, "♯": 1, "#": 1, "𝄫": -2, bb: -2, "♭": -1, b: -1 }[m[2] ?? ""] ?? 0;
  return { letter: l.toUpperCase(), acc, lower: l === l.toLowerCase(), rest: m[3]! };
}

function formatRoot(r: Root, sp: Spelling): string {
  const cased = (s: string) => (r.lower ? s.toLowerCase() : s);
  if (sp === "letters") return cased(r.letter) + ["𝄫", "♭", "", "♯", "𝄪"][r.acc + 2]!;
  const flatS = r.letter === "A" || r.letter === "E";
  const suffix = ["eses", "es", "", "is", "isis"][r.acc + 2]!;
  if (sp === "dutch") {
    if (r.acc < 0 && flatS) return cased(r.letter) + suffix.slice(1); // As, Es, Ases, Eses
    return cased(r.letter) + suffix; // B = B natural, Bes = B♭
  }
  // German: B natural is H, B♭ is B
  if (r.letter === "B") {
    if (r.acc === -1) return cased("B");
    return cased("H") + suffix;
  }
  if (r.acc < 0 && flatS) return cased(r.letter) + suffix.slice(1);
  return cased(r.letter) + suffix;
}

function convert(name: string, parse: (s: string) => Root | null, sp: Spelling): string {
  return name
    .split("/")
    .map((part) => {
      const r = parse(part);
      return r ? formatRoot(r, sp) + r.rest : part;
    })
    .join("/");
}

/** A note or chord name from the core (German spelling) as shown in the current language. */
export function chordName(name: string): string {
  const sp = spelling();
  return sp === "german" ? name : convert(name, parseGerman, sp);
}

/** Space-separated chord names (a song's chords) for display. */
export function chordsText(text: string): string {
  return spelling() === "german" ? text : text.split(/(\s+)/).map((p) => (/\s/.test(p) || !p ? p : chordName(p))).join("");
}

/** Chord names typed in the current spelling, in the core's German spelling. */
export function chordsInput(text: string): string {
  const sp = spelling();
  if (sp === "german") return text;
  return text
    .split(/(\s+)/)
    .map((p) => (/\s/.test(p) || !p ? p : convert(p, (s) => parseInput(s, sp), "german")))
    .join("");
}

/** A key name from the core's data ("fis-Moll", "Es-Dur") for display. */
export function keyText(german: string): string {
  const m = /^\s*([A-Ha-h][a-z]*)-(Dur|Moll)\s*$/i.exec(german);
  if (!m) return german;
  const root = m[1]!;
  const note = chordName(root.charAt(0).toUpperCase() + root.slice(1));
  if (m[2]!.toLowerCase() === "dur") return t("{note} major", { note });
  return t("{note} minor", { note, low: chordName(root.toLowerCase()) });
}

/** A key typed in the current language ("F♯ minor", "fis-Moll", "Am") for the core
 *  (German: "fis-Moll"); unrecognised text goes through unchanged. */
export function keyInput(text: string): string {
  const sp = spelling();
  const trimmed = text.trim();
  const toGerman = (root: string, minor: boolean) => {
    const r = parseInput(root, sp);
    if (!r || r.rest) return null;
    const g = formatRoot({ ...r, lower: minor, rest: "" }, "german");
    return minor ? `${g}-Moll` : `${g.charAt(0).toUpperCase()}${g.slice(1)}-Dur`;
  };
  const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  for (const [template, minor] of [[t("{note} minor", { note: "\u0000", low: "\u0000" }), true], [t("{note} major", { note: "\u0000" }), false]] as const) {
    const re = new RegExp(`^${template.split("\u0000").map(escape).join("(\\S+?)")}$`, "i");
    const m = re.exec(trimmed);
    if (m?.[1]) {
      const g = toGerman(m[1], minor);
      if (g) return g;
    }
  }
  return text;
}
