// Fenster „Fuge“: Thema (ein Noten-Clip) → Antwort, Verwandlungen, Gegensatz,
// Engführungen, Fugenplan; Kontrapunkt-Prüfer über alle Instrumentspuren mit
// Markierungen in der Zeitleiste; ähnliche Themen in der Bibliothek.

import { backend } from "../bridge.js";
import { keyText } from "../spelling.js";
import { fitCanvas, h, icon, noteName } from "../dom.js";
import { plural, t } from "../i18n.js";
import { act, counterpointMarks, errorText, findClip, notify, project, selection, snapshot } from "../store.js";
import { searchLibraryFor } from "./library.js";
import type { CheckView, Countersubject, FuguePlan, InstrumentKind, KeySpec, Snapshot, SubjectView, TransformSpec, VNote } from "../types.js";

const MAJOR = ["C", "Des", "D", "Es", "E", "F", "Fis", "G", "As", "A", "B", "H"];
const MINOR = ["c", "cis", "d", "es", "e", "f", "fis", "g", "gis", "a", "b", "h"];
/** Key name in the language's spelling (the tables are German, like the core's names). */
const keyName = (k: KeySpec) => keyText(k.minor ? `${MINOR[k.tonic] ?? ""}-Moll` : `${MAJOR[k.tonic] ?? ""}-Dur`);

// by the kind of section (FugueSection.kind, untranslated)
const SECTION_COLORS: Record<string, string> = {
  exposition: "#6ee7d8",
  zwischenspiel: "#7aa7ff",
  durchfuehrung: "#c3a6ff",
  engfuehrung: "#ffb454",
  schlussdurchfuehrung: "#ffb454",
  orgelpunkt: "#ff8a95",
  schlusskadenz: "#8fe3a8",
};
const sectionColor = (kind: string) => SECTION_COLORS[kind] ?? "#8a93a6";

/** Zustand bleibt erhalten, wenn das Fenster neu aufgebaut wird. */
const st = {
  subject: null as { trackId: number; clipId: number } | null,
  keyChoice: null as KeySpec | null,
  voices: 3,
  kind: "organ" as InstrumentKind,
  view: null as SubjectView | null,
  shift: 4,
  cs: null as Countersubject[] | null,
  plan: null as FuguePlan | null,
  check: null as CheckView | null,
  live: false,
  hints: false,
  onlyFugues: true,
};

function projectEnd(): number {
  const p = project();
  if (!p) return 0;
  const bpb = p.beatsPerBar || 4;
  let end = 0;
  for (const t of p.tracks) {
    for (const c of t.clips) {
      const len = c.source.type === "notes" ? c.source.lengthBeats : (c.source.durationSecs * (c.source.warp ? c.source.sourceBpm : p.tempoBpm)) / 60;
      end = Math.max(end, c.startBeats + len);
    }
  }
  return Math.ceil(end / bpb) * bpb;
}

/** Kleine Notenrolle: mehrere Lagen (Farbe), optional hervorgehobene Noten. */
function drawRoll(canvas: HTMLCanvasElement, layers: Array<{ notes: VNote[]; color: string; marks?: Set<number> }>, beatsPerBar = 4): void {
  const { ctx, w, h: hh } = fitCanvas(canvas);
  ctx.clearRect(0, 0, w, hh);
  const all = layers.flatMap((l) => l.notes);
  if (!all.length) return;
  const lo = Math.min(...all.map((n) => n.pitch)) - 1;
  const hi = Math.max(...all.map((n) => n.pitch)) + 1;
  const t0 = Math.min(...all.map((n) => n.start));
  const t1 = Math.max(...all.map((n) => n.start + n.len));
  const qw = (w - 4) / Math.max(1, t1 - t0);
  const lane = (hh - 4) / (hi - lo + 1);
  for (let b = Math.ceil(t0 / beatsPerBar) * beatsPerBar; b <= t1; b += beatsPerBar) {
    ctx.fillStyle = "#1d2330";
    ctx.fillRect(2 + (b - t0) * qw, 0, 1, hh);
  }
  for (const l of layers) {
    l.notes.forEach((n, i) => {
      ctx.fillStyle = l.marks?.has(i) ? "#ff8a95" : l.color;
      ctx.fillRect(2 + (n.start - t0) * qw, 2 + (hi - n.pitch) * lane, Math.max(2, n.len * qw - 1.5), Math.max(2, lane - 0.5));
    });
  }
}

function rollCanvas(height = 54): HTMLCanvasElement {
  const c = h("canvas", { class: "fg-roll" });
  c.style.height = `${height}px`;
  return c;
}

const noteList = (notes: VNote[]) => notes.map((n) => noteName(n.pitch)).join(" ");

export function fuguePanel(): { el: HTMLElement; update(): void } {
  const body = h("div", { class: "fg-body" });
  const keySel = h("select", { class: "hc-select", title: t("Key of the subject – estimated automatically from the subject or set manually") });
  keySel.addEventListener("change", () => {
    const v = keySel.value;
    st.keyChoice = v === "auto" ? null : { tonic: Number(v.split(":")[0]), minor: v.endsWith(":m") };
    void load();
  });
  const seg = <T extends string | number>(values: Array<[T, string]>, get: () => T, set: (v: T) => void, title: string) => {
    const s = h("div", { class: "seg mini", title });
    const paint = () => {
      for (const b of s.children) b.classList.toggle("on", (b as HTMLElement).dataset.v === String(get()));
    };
    for (const [v, label] of values) {
      const b = h("button", { class: "seg-btn", onclick: () => { set(v); paint(); } }, label);
      b.dataset.v = String(v);
      s.append(b);
    }
    paint();
    return s;
  };
  const voicesSeg = seg<number>([[2, "2"], [3, "3"], [4, "4"]], () => st.voices, (v) => { st.voices = v; st.plan = null; render(); }, t("Voices in the fugue"));
  const kindSeg = seg<InstrumentKind>([["organ", t("Organ")], ["harpsichord", t("Harpsichord")], ["pluck", t("Pluck")]], () => st.kind, (v) => (st.kind = v), t("Sound for listening and for new tracks"));
  // Thema: Auswahlliste aller Noten-Clips (oder der in der Zeitleiste gewählte)
  const subjectSel = h("select", { class: "hc-select fg-subject", title: t("Which note clip is the fugue subject") });
  subjectSel.addEventListener("change", () => {
    const [t, c] = subjectSel.value.split(":").map(Number);
    if (t == null || c == null || Number.isNaN(c)) return;
    st.subject = { trackId: t, clipId: c };
    st.cs = null;
    st.plan = null;
    lastSig = subjectSig();
    void load();
  });
  const takeBtn = h("button", { class: "btn small", title: t("Use the note clip selected in the timeline as the subject"), onclick: () => takeSelection(true) }, icon("clip"), t("Selection"));
  const field = (label: string, c: HTMLElement) => h("span", { class: "hc-field" }, h("label", {}, label), c);
  const head = h("div", { class: "hc-head" }, field(t("Subject"), subjectSel), takeBtn, field(t("Key"), keySel), field(t("Voices"), voicesSeg), field(t("Sound"), kindSeg));

  let clipsSig = "";
  function fillSubjects(): void {
    const p = project();
    const bpb = p?.beatsPerBar || 4;
    const items = (p?.tracks ?? [])
      .filter((tr) => tr.kind === "instrument" && tr.instrument?.kind !== "drums")
      .flatMap((tr) => tr.clips.filter((c) => c.source.type === "notes" && c.source.notes.length >= 4)
        .map((c) => ({ value: `${tr.id}:${c.id}`, label: `${tr.name} · ${c.name || t("Clip")} (${t("Bar {bar}", { bar: Math.floor(c.startBeats / bpb) + 1 })})` })));
    const sig = JSON.stringify(items) + (st.subject ? `${st.subject.trackId}:${st.subject.clipId}` : "");
    if (sig === clipsSig) return;
    clipsSig = sig;
    subjectSel.replaceChildren(
      h("option", { value: "" }, items.length ? t("– Choose a clip –") : t("no note clip in the project")),
      ...items.map((i) => h("option", { value: i.value }, i.label)));
    subjectSel.value = st.subject ? `${st.subject.trackId}:${st.subject.clipId}` : "";
  }
  const el = h("div", { class: "fugue" }, head, body);

  function fillKeys(): void {
    const v = st.view;
    const auto = v && v.key.source !== "gewaehlt" ? t("Automatic ({key})", { key: v.key.name }) : t("Automatic");
    const opts = [h("option", { value: "auto" }, auto)];
    for (const minor of [false, true]) {
      for (let i = 0; i < 12; i++) {
        const tonic = (i * 7 + (minor ? 9 : 0)) % 12;
        opts.push(h("option", { value: `${tonic}:${minor ? "m" : "d"}` }, keyName({ tonic, minor })));
      }
    }
    keySel.replaceChildren(...opts);
    keySel.value = st.keyChoice ? `${st.keyChoice.tonic}:${st.keyChoice.minor ? "m" : "d"}` : "auto";
  }

  const key = (): KeySpec => (st.view ? { tonic: st.view.key.tonic, minor: st.view.key.minor } : st.keyChoice ?? { tonic: 0, minor: false });

  function takeSelection(explicit: boolean): void {
    const found = findClip(project(), selection.peek());
    if (!found || found.clip.source.type !== "notes") {
      if (explicit) notify(t("Please click a note clip containing the subject in the timeline first."));
      return;
    }
    st.subject = { trackId: found.track.id, clipId: found.clip.id };
    st.cs = null;
    st.plan = null;
    lastSig = subjectSig();
    void load();
  }

  async function load(): Promise<void> {
    if (!st.subject) {
      render();
      return;
    }
    try {
      st.view = await backend.fugueSubject(st.subject.trackId, st.subject.clipId, st.keyChoice);
    } catch (e) {
      st.view = null;
      notify(errorText(e), "error");
    }
    render();
  }

  const play = (voices: VNote[][]) => void backend.fuguePreview(voices, st.kind).catch((e) => notify(errorText(e), "error"));
  const write = async (voices: Array<{ name: string; notes: VNote[] }>, start: number, label: string) => {
    if (await act(backend.fugueAddVoices(voices, start, st.kind, label))) notify(plural(voices.length, "{label}: created {n} track", "{label}: created {n} tracks", { label }));
  };

  // ------------------------------------------------------------ Karten

  function card(title: string, sub: string | null, ...kids: Array<Node | null>): HTMLElement {
    return h("section", { class: "fg-card" }, h("div", { class: "fg-title" }, title, sub ? h("span", { class: "muted" }, ` – ${sub}`) : null), ...kids);
  }

  function subjectCard(v: SubjectView): HTMLElement {
    const canvas = rollCanvas(70);
    const marks = new Set(v.answer.mutated);
    requestAnimationFrame(() =>
      drawRoll(canvas, [
        { notes: v.notes, color: "#6ee7d8" },
        { notes: v.answer.notes.map((n) => ({ ...n, start: n.start + Math.ceil(v.length / v.beatsPerBar) * v.beatsPerBar })), color: "#ffb454", marks },
      ], v.beatsPerBar));
    const gap = Math.ceil(v.length / v.beatsPerBar) * v.beatsPerBar;
    return card(t("Subject and answer"), t("{key}, {beats} beats, {low}–{high}", { key: v.key.name, beats: Math.round(v.length * 10) / 10, low: noteName(v.range[0]), high: noteName(v.range[1]) }),
      canvas,
      h("div", { class: "fg-legend" }, h("i", { style: "background:#6ee7d8" }), t("Subject (dux)"), h("i", { style: "background:#ffb454" }),
        v.answer.kind === "tonal" ? t("Answer (comes, tonal)") : t("Answer (comes, real)"), v.answer.mutated.length ? h("i", { style: "background:#ff8a95" }) : null, v.answer.mutated.length ? t("altered notes") : null),
      h("p", { class: "fg-text" }, v.answer.reason),
      h("div", { class: "fg-actions" },
        h("button", { class: "btn small", onclick: () => play([v.notes]) }, icon("play"), t("Subject")),
        h("button", { class: "btn small", onclick: () => play([v.answer.notes]) }, icon("play"), t("Answer")),
        h("button", { class: "btn small", title: t("Subject, then answer with the first countersubject suggestion (if computed)"),
          onclick: () => play([v.notes, v.answer.notes.map((n) => ({ ...n, start: n.start + gap })), ...(st.cs?.[0] ? [st.cs[0].notes.map((n) => ({ ...n, start: n.start + gap, pitch: n.pitch + 7 }))] : [])]) },
          icon("play"), t("Subject → answer")),
        h("button", { class: "btn small blue", title: t("Answer as a new track right after the subject"), onclick: () => void write([{ name: t("Answer"), notes: v.answer.notes }], v.start + gap, t("Answer")) }, t("+ Answer as track")),
        h("button", { class: "btn small ghost", title: t("Search the library for similar subjects"), onclick: () => st.subject && void searchLibraryFor({ ...st.subject, text: st.onlyFugues ? "fug" : "" }) },
          icon("book"), t("Similar subjects")),
        h("label", { class: "fg-check", title: t("Only pieces with \"Fuge/Fugue/Fuga\" in the title") },
          (() => { const c = h("input", { type: "checkbox", checked: st.onlyFugues }); c.addEventListener("change", () => (st.onlyFugues = c.checked)); return c; })(), t("fugues only"))));
  }

  function transformCard(v: SubjectView): HTMLElement {
    const rows = v.transforms.map((tf) => {
      const c = rollCanvas(36);
      requestAnimationFrame(() => drawRoll(c, [{ notes: tf.notes, color: "#c3a6ff" }], v.beatsPerBar));
      return h("div", { class: "fg-row" }, h("b", {}, tf.label), c,
        h("button", { class: "btn small", title: noteList(tf.notes), onclick: () => play([tf.notes]) }, icon("play")),
        h("button", { class: "btn small blue", title: t("As a new track at the position of the subject"), onclick: () => void write([{ name: `${v.clipName} – ${tf.label}`, notes: tf.notes }], v.start, tf.label) }, t("+ Track")));
    });
    const stepInput = h("input", { class: "num fg-num", type: "number", min: -14, max: 14, step: 1, value: String(st.shift), title: t("Scale steps in the key (negative = lower)") });
    stepInput.addEventListener("change", () => (st.shift = Number(stepInput.value) || 0));
    stepInput.addEventListener("keydown", (e) => e.stopPropagation());
    const shifted = async () => {
      if (!st.subject) return null;
      const spec: TransformSpec = { type: "versetzung", value: st.shift };
      return backend.fugueTransform(st.subject.trackId, st.subject.clipId, key(), spec).catch((e) => (notify(errorText(e), "error"), null));
    };
    rows.push(h("div", { class: "fg-row" }, h("b", {}, t("Shift by")), h("span", { class: "fg-inline" }, stepInput, ` ${t("steps")}`),
      h("button", { class: "btn small", onclick: async () => { const n = await shifted(); if (n) play([n]); } }, icon("play")),
      h("button", { class: "btn small blue", onclick: async () => {
        const n = await shifted();
        if (n) void write([{ name: plural(Math.abs(st.shift), "{clip} – {shift} step", "{clip} – {shift} steps", { clip: v.clipName, shift: `${st.shift > 0 ? "+" : ""}${st.shift}` }), notes: n }], v.start, t("Transposition"));
      } }, t("+ Track"))));
    return card(t("Transformations"), t("Diatonic inversion around the first note"), ...rows);
  }

  function csCard(v: SubjectView): HTMLElement {
    const list = h("div", { class: "fg-list" });
    if (st.cs) {
      if (!st.cs.length) list.append(h("div", { class: "lib-empty" }, t("No countersubject found.")));
      st.cs.forEach((c, i) => {
        const canvas = rollCanvas(48);
        requestAnimationFrame(() => drawRoll(canvas, [{ notes: v.notes, color: "#6ee7d8" }, { notes: c.notes, color: "#ffb454" }], v.beatsPerBar));
        list.append(h("div", { class: "fg-row cs" },
          h("b", {}, c.above ? t("#{nr} above the subject", { nr: i + 1 }) : t("#{nr} below the subject", { nr: i + 1 })),
          canvas,
          h("span", { class: `fg-badge ${c.errors ? "bad" : "ok"}` }, c.errors ? plural(c.errors, "{n} error", "{n} errors") : t("no errors")),
          h("span", { class: `fg-badge ${c.invertible ? "ok" : "warn"}`, title: t("Swapped (octave inversion): {errors} errors, {warnings} warnings", { errors: c.invertedErrors, warnings: c.invertedWarnings }) }, c.invertible ? t("invertible") : t("not invertible")),
          h("button", { class: "btn small", title: t("Listen together with the subject"), onclick: () => play([v.notes, c.notes]) }, icon("play")),
          h("button", { class: "btn small blue", title: t("As a new track alongside the subject"), onclick: () => void write([{ name: t("Countersubject"), notes: c.notes }], v.start, t("Countersubject")) }, t("+ Track"))));
      });
    }
    return card(t("Countersubject"), t("checked in double counterpoint at the octave"),
      h("div", { class: "fg-actions" }, h("button", { class: "btn small primary", onclick: async () => {
        if (!st.subject) return;
        try {
          st.cs = await backend.fugueCountersubjects(st.subject.trackId, st.subject.clipId, key());
        } catch (e) {
          notify(errorText(e), "error");
        }
        render();
      } }, st.cs ? t("Recalculate") : t("Calculate suggestions"))), list);
  }

  function strettoCard(v: SubjectView): HTMLElement {
    if (!v.strettos.length) return card(t("Strettos"), null, h("div", { class: "lib-empty" }, t("No error-free stretto found on the beat grid – the subject is better suited to regular entries.")));
    return card(t("Strettos"), t("Subject against itself, entering staggered – tightest first"),
      ...v.strettos.map((s) => {
        const canvas = rollCanvas(40);
        requestAnimationFrame(() => drawRoll(canvas, [{ notes: v.notes, color: "#6ee7d8" }, { notes: s.follower, color: "#ffb454" }], v.beatsPerBar));
        return h("div", { class: "fg-row st" },
          h("b", {}, t("{label}, after {delay} beats", { label: s.label, delay: s.delay })),
          canvas,
          h("span", { class: "fg-badge ok", title: t("Share of the time in which both voices play the subject simultaneously") }, t("{pct}% overlap", { pct: Math.round(s.overlap * 100) })),
          h("button", { class: "btn small", onclick: () => play([v.notes, s.follower]) }, icon("play")),
          h("button", { class: "btn small blue", title: t("Both voices as new tracks after the end of the project"), onclick: () => void write([{ name: t("Stretto – dux"), notes: v.notes }, { name: t("Stretto – comes"), notes: s.follower }], projectEnd(), t("Stretto")) }, t("+ Tracks")));
      }));
  }

  function planCard(): HTMLElement {
    const p = st.plan;
    const kids: Node[] = [
      h("div", { class: "fg-actions" },
        h("button", { class: "btn small primary", onclick: async () => {
          if (!st.subject) return;
          try {
            st.plan = await backend.fuguePlan(st.subject.trackId, st.subject.clipId, key(), { voices: st.voices, countersubject: true, stretto: true, picardy: true });
          } catch (e) {
            notify(errorText(e), "error");
          }
          render();
        } }, p ? t("Recalculate") : t("Calculate plan")),
        p ? h("button", { class: "btn small", onclick: () => play(p.voices.map((x) => x.notes)) }, icon("play"), t("Listen")) : null,
        p ? h("button", { class: "btn small blue", title: t("Create one track per voice after the end of the project"), onclick: () => void write(p.voices.map((x) => ({ name: t("Fugue – {voice}", { voice: x.name }), notes: x.notes })), projectEnd(), t("Create fugue")) }, icon("plus"), t("Create fugue")) : null),
    ];
    if (p) {
      const bpb = st.view?.beatsPerBar ?? 4;
      const bar = h("div", { class: "fg-sections" },
        ...p.sections.map((s) => {
          const seg = h("div", { class: "fg-sec", title: `${t("{name} ({key}): bars {from}–{to}", { name: s.name, key: s.key, from: Math.floor(s.start / bpb) + 1, to: Math.ceil(s.end / bpb) })}\n${s.description}` }, s.name);
          seg.style.flex = String(s.end - s.start);
          seg.style.setProperty("--c", sectionColor(s.kind));
          return seg;
        }));
      const canvas = rollCanvas(110);
      const colors = ["#6ee7d8", "#ffb454", "#c3a6ff", "#7aa7ff"];
      requestAnimationFrame(() => drawRoll(canvas, p.voices.map((v, i) => ({ notes: v.notes, color: colors[i % colors.length]! })), bpb));
      kids.push(bar, canvas,
        h("div", { class: "fg-legend" }, ...p.voices.flatMap((v, i) => [h("i", { style: `background:${colors[i % colors.length]}` }), v.name])),
        h("ul", { class: "fg-seclist" }, ...p.sections.map((s) => h("li", {}, h("i", { style: `background:${sectionColor(s.kind)}` }),
          h("b", {}, s.name), ` · ${t("Bars {from}–{to}", { from: Math.floor(s.start / bpb) + 1, to: Math.ceil(s.end / bpb) })} · ${s.key} – `, h("span", { class: "muted" }, s.description)))),
        h("p", { class: "fg-text muted" }, `${[
          plural(Math.ceil(p.length / bpb), "{n} bar", "{n} bars"),
          p.answer === "tonal" ? t("tonal answer") : t("real answer"),
          p.countersubject ? t("with countersubject") : null,
          p.stretto ? t("Stretto: {stretto}", { stretto: p.stretto }) : null,
        ].filter(Boolean).join(", ")}. ${t("A skeleton to build on – then go over it with the checker.")}`));
    }
    return card(t("Fugue plan"), plural(st.voices, "{n} voice", "{n} voices"), ...kids);
  }

  function checkCard(): HTMLElement {
    const c = st.check;
    const run = async () => {
      try {
        st.check = await backend.counterpointCheck(null);
        applyMarks();
      } catch (e) {
        notify(errorText(e), "error");
      }
      renderCheck();
    };
    const live = h("input", { type: "checkbox", checked: st.live });
    live.addEventListener("change", () => {
      st.live = live.checked;
      if (st.live) void run();
    });
    const hints = h("input", { type: "checkbox", checked: st.hints });
    hints.addEventListener("change", () => {
      st.hints = hints.checked;
      applyMarks();
      renderCheck();
    });
    const issues = (c?.issues ?? []).filter((i) => st.hints || i.severity !== "hinweis");
    const names = new Map((c?.voices ?? []).map((v) => [v.trackId, v.name]));
    return card(t("Counterpoint checker"), t("all instrument tracks, one voice per track"),
      h("div", { class: "fg-actions" },
        h("button", { class: "btn small primary", title: t("Check all instrument tracks as voices against the rules of counterpoint (parallels, voice crossings, dissonances)"), onclick: () => void run() }, t("Check")),
        h("label", { class: "fg-check", title: t("Check again after every change") }, live, t("live")),
        h("label", { class: "fg-check", title: t("Also show minor hints, not just errors and warnings") }, hints, t("Show hints")),
        c ? h("button", { class: "btn small ghost", title: t("Remove markers from the timeline"), onclick: () => { st.check = null; applyMarks(); renderCheck(); } }, t("Markers off")) : null),
      c ? h("div", { class: "fg-sum" },
        h("span", { class: `fg-badge ${c.errors ? "bad" : "ok"}` }, plural(c.errors, "{n} error", "{n} errors")),
        h("span", { class: `fg-badge ${c.warnings ? "warn" : "ok"}` }, plural(c.warnings, "{n} warning", "{n} warnings")),
        h("span", { class: "fg-badge" }, plural(c.hints, "{n} hint", "{n} hints")),
        h("span", { class: "muted" }, ` · ${c.voices.map((v) => v.name).join(", ") || t("no voices")}`)) : h("div", { class: "lib-empty" }, t("Checks parallel fifths and octaves, hidden parallels, voice crossing, dissonance treatment, range and leaps.")),
      c ? h("div", { class: "fg-issues" }, ...(issues.length ? issues.slice(0, 200).map((i) =>
        h("button", { class: `fg-issue ${i.severity}`, title: t("Jump to this spot"), onclick: () => {
          const bpm = project()?.tempoBpm ?? 120;
          void backend.seek((i.start * 60) / bpm);
          if (i.trackIds[0] != null) selection.value = { trackId: i.trackIds[0], clipId: null };
        } }, h("b", {}, t("Bar {bar}", { bar: i.bar })), h("span", {}, i.message), h("em", {}, i.trackIds.map((id) => names.get(id) ?? "").join(" · ")))) : [h("div", { class: "lib-empty" }, t("No part-writing errors found."))])) : null);
  }

  function applyMarks(): void {
    const c = st.check;
    counterpointMarks.value = c ? c.issues.filter((i) => st.hints || i.severity !== "hinweis").map((i) => ({ start: i.start, end: i.end, severity: i.severity, trackIds: i.trackIds, message: i.message })) : [];
  }

  let checkEl: HTMLElement | null = null;
  function renderCheck(): void {
    const fresh = checkCard();
    checkEl?.replaceWith(fresh);
    checkEl = fresh;
  }

  function render(): void {
    fillKeys();
    fillSubjects();
    const v = st.view;
    subjectSel.title = v ? t("Subject: {notes}", { notes: noteList(v.notes) }) : t("Which note clip is the fugue subject");
    checkEl = checkCard();
    if (!v) {
      body.replaceChildren(
        h("div", { class: "fg-empty" },
          h("b", {}, t("How it works")),
          h("ol", {},
            h("li", {}, t("Write a fugue subject as notes in an instrument track (2–4 bars, e.g. with the piano roll or by singing it into the library).")),
            h("li", {}, t("Choose the clip under \"Subject\" above (or click it in the timeline and press \"Selection\").")),
            h("li", {}, t("Listen to the answer, transformations, countersubject and strettos – or calculate a fugue plan right away and create it as tracks.")),
            h("li", {}, t("Use the counterpoint checker to find parallels and dissonances; the spots are marked in the timeline.")))),
        checkEl);
      return;
    }
    body.replaceChildren(subjectCard(v), h("div", { class: "fg-cols" }, transformCard(v), strettoCard(v)), csCard(v), planCard(), checkEl);
  }

  // ------------------------------------------------------------ Lebenszyklus

  let lastSnap: Snapshot | null = null;
  let timer = 0;
  /** Fingerabdruck des Themen-Clips: nur bei Änderung neu analysieren. */
  const subjectSig = () => {
    const f = st.subject ? findClip(project(), { trackId: st.subject.trackId, clipId: st.subject.clipId }) : null;
    return f ? JSON.stringify([f.clip.source, f.clip.startBeats, f.track.pitchSemitones]) : "";
  };
  let lastSig = subjectSig();
  if (!st.subject) takeSelection(false);
  if (st.subject) void load();
  else render();

  return {
    el,
    update() {
      const snap = snapshot.peek();
      if (snap === lastSnap) return;
      const first = lastSnap === null;
      lastSnap = snap;
      fillSubjects();
      if (first) return;
      // Thema geändert oder gelöscht?
      if (st.subject && !findClip(project(), { trackId: st.subject.trackId, clipId: st.subject.clipId })) {
        st.subject = null;
        st.view = null;
        render();
      }
      clearTimeout(timer);
      timer = window.setTimeout(() => {
        const sig = subjectSig();
        if (st.subject && sig !== lastSig) {
          lastSig = sig;
          void load();
        }
        if (st.live) {
          void backend.counterpointCheck(null).then((c) => {
            st.check = c;
            applyMarks();
            renderCheck();
          }).catch(() => {});
        }
      }, 700);
    },
  };
}
