// Test scores for the agent's library tools (search_library, read_library_score,
// import_library_score): two movements of a string sextet as separate MusicXML
// files – like a piece kept in the library as single movements. Used by the
// browser tests (tests/web) and the desktop tests (klangwelt-desktop, via the
// submodule).

const STEPS = ["C", "C", "D", "D", "E", "F", "F", "G", "G", "A", "A", "B"];
const ALTERS = [0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 0];

/** One note (MIDI, length in quarters; divisions = 2). */
function noteXml(midi, quarters) {
  const step = STEPS[midi % 12];
  const alter = ALTERS[midi % 12] ? "<alter>1</alter>" : "";
  return `<note><pitch><step>${step}</step>${alter}<octave>${Math.floor(midi / 12) - 1}</octave></pitch><duration>${quarters * 2}</duration><voice>1</voice></note>`;
}

/** MusicXML of a score: parts = [{ name, notes: [[midi, quarters], …] }], bars of `beats` quarters. */
export function scoreXml({ title, composer, beats, tempo, parts }) {
  const list = parts.map((p, i) => `<score-part id="P${i + 1}"><part-name>${p.name}</part-name></score-part>`).join("");
  const body = parts
    .map((p, i) => {
      const bars = [];
      let bar = [];
      let fill = 0;
      for (const [midi, q] of p.notes) {
        bar.push(noteXml(midi, q));
        fill += q;
        if (fill >= beats) {
          bars.push(bar.join(""));
          bar = [];
          fill = 0;
        }
      }
      if (bar.length) bars.push(bar.join(""));
      return `<part id="P${i + 1}">${bars
        .map((m, k) => {
          const attrs = k === 0 ? `<attributes><divisions>2</divisions><key><fifths>0</fifths></key><time><beats>${beats}</beats><beat-type>4</beat-type></time></attributes>` : "";
          const sound = k === 0 && i === 0 ? `<direction><sound tempo="${tempo}"/></direction>` : "";
          return `<measure number="${k + 1}">${attrs}${sound}${m}</measure>`;
        })
        .join("")}</part>`;
    })
    .join("");
  return `<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0"><work><work-title>${title}</work-title></work><identification><creator type="composer">${composer}</creator></identification><part-list>${list}</part-list>${body}</score-partwise>`;
}

/** The six parts with their lowest note. */
export const SEXTET_PARTS = [["Violin 1", 76], ["Violin 2", 72], ["Viola 1", 67], ["Viola 2", 64], ["Violoncello 1", 55], ["Violoncello 2", 48]];
const SCALE = [0, 2, 4, 5, 7, 9, 11, 12];

/** A movement: every part plays 12 quarters up its scale (shifted per part). */
function movement(n, beats, tempo, heading) {
  return {
    file: `Testsextett op. 1 - Satz ${n}.musicxml`,
    title: `Testsextett op. 1 – ${n}. Satz: ${heading}`,
    composer: "Pjotr Test",
    beats,
    tempo,
    parts: SEXTET_PARTS.map(([name, base], i) => ({ name, notes: Array.from({ length: 12 }, (_, k) => [base + SCALE[(k + i + n) % 8], 1]) })),
  };
}

/** Movement 1 in 3/4 (four bars), movement 2 in 4/4 (three bars). */
export const SEXTET = [movement(1, 3, 132, "Allegro"), movement(2, 4, 60, "Adagio")];

/** Compact notes of a part in bars `from`…`to` as read_library_score writes them (all quarters). */
export function expectedExcerpt(mov, part, from, to) {
  const notes = mov.parts[part].notes.slice((from - 1) * mov.beats, to * mov.beats);
  const names = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "H"];
  return notes.map(([m], i) => `${names[m % 12]}${Math.floor(m / 12) - 1}${i === 0 ? " 1" : ""}`).join(" · ");
}
