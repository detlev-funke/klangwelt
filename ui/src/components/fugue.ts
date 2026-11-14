// Fenster „Fuge“: Thema (ein Noten-Clip) → Antwort, Verwandlungen, Gegensatz,
// Engführungen, Fugenplan; Kontrapunkt-Prüfer über alle Instrumentspuren mit
// Markierungen in der Zeitleiste; ähnliche Themen in der Bibliothek.

import { backend } from "../bridge.js";
import { fitCanvas, h, icon, noteName } from "../dom.js";
import { act, counterpointMarks, errorText, findClip, notify, project, selection, snapshot } from "../store.js";
import { searchLibraryFor } from "./library.js";
import type { CheckView, Countersubject, FuguePlan, InstrumentKind, KeySpec, Snapshot, SubjectView, TransformSpec, VNote } from "../types.js";

const MAJOR = ["C", "Des", "D", "Es", "E", "F", "Fis", "G", "As", "A", "B", "H"];
const MINOR = ["c", "cis", "d", "es", "e", "f", "fis", "g", "gis", "a", "b", "h"];
const keyName = (k: KeySpec) => (k.minor ? `${MINOR[k.tonic]}-Moll` : `${MAJOR[k.tonic]}-Dur`);

const SECTION_COLORS: Record<string, string> = {
  Exposition: "#6ee7d8",
  Zwischenspiel: "#7aa7ff",
  Durchführung: "#c3a6ff",
  Engführung: "#ffb454",
  Schlussdurchführung: "#ffb454",
  Orgelpunkt: "#ff8a95",
  Schlusskadenz: "#8fe3a8",
};
const sectionColor = (name: string) => SECTION_COLORS[Object.keys(SECTION_COLORS).find((k) => name.startsWith(k)) ?? ""] ?? "#8a93a6";

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
  const keySel = h("select", { class: "hc-select", title: "Tonart des Themas – automatisch aus dem Thema geschätzt oder fest gewählt" });
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
  const voicesSeg = seg<number>([[2, "2"], [3, "3"], [4, "4"]], () => st.voices, (v) => { st.voices = v; st.plan = null; render(); }, "Stimmen der Fuge");
  const kindSeg = seg<InstrumentKind>([["organ", "Orgel"], ["harpsichord", "Cembalo"], ["pluck", "Zupfer"]], () => st.kind, (v) => (st.kind = v), "Klang beim Anhören und für neue Spuren");
  // Thema: Auswahlliste aller Noten-Clips (oder der in der Zeitleiste gewählte)
  const subjectSel = h("select", { class: "hc-select fg-subject", title: "Welcher Noten-Clip das Fugenthema ist" });
  subjectSel.addEventListener("change", () => {
    const [t, c] = subjectSel.value.split(":").map(Number);
    if (t == null || c == null || Number.isNaN(c)) return;
    st.subject = { trackId: t, clipId: c };
    st.cs = null;
    st.plan = null;
    lastSig = subjectSig();
    void load();
  });
  const takeBtn = h("button", { class: "btn small", title: "Den in der Zeitleiste gewählten Noten-Clip als Thema nehmen", onclick: () => takeSelection(true) }, icon("clip"), "Auswahl");
  const field = (label: string, c: HTMLElement) => h("span", { class: "hc-field" }, h("label", {}, label), c);
  const head = h("div", { class: "hc-head" }, field("Thema", subjectSel), takeBtn, field("Tonart", keySel), field("Stimmen", voicesSeg), field("Klang", kindSeg));

  let clipsSig = "";
  function fillSubjects(): void {
    const p = project();
    const bpb = p?.beatsPerBar || 4;
    const items = (p?.tracks ?? [])
      .filter((t) => t.kind === "instrument" && t.instrument?.kind !== "drums")
      .flatMap((t) => t.clips.filter((c) => c.source.type === "notes" && c.source.notes.length >= 4)
        .map((c) => ({ value: `${t.id}:${c.id}`, label: `${t.name} · ${c.name || "Clip"} (T. ${Math.floor(c.startBeats / bpb) + 1})` })));
    const sig = JSON.stringify(items) + (st.subject ? `${st.subject.trackId}:${st.subject.clipId}` : "");
    if (sig === clipsSig) return;
    clipsSig = sig;
    subjectSel.replaceChildren(
      h("option", { value: "" }, items.length ? "– Clip wählen –" : "kein Noten-Clip im Projekt"),
      ...items.map((i) => h("option", { value: i.value }, i.label)));
    subjectSel.value = st.subject ? `${st.subject.trackId}:${st.subject.clipId}` : "";
  }
  const el = h("div", { class: "fugue" }, head, body);

  function fillKeys(): void {
    const v = st.view;
    const auto = v && v.key.source !== "gewaehlt" ? `Automatisch (${v.key.name})` : "Automatisch";
    const opts = [h("option", { value: "auto" }, auto)];
    for (const minor of [false, true]) {
      for (let i = 0; i < 12; i++) {
        const t = (i * 7 + (minor ? 9 : 0)) % 12;
        opts.push(h("option", { value: `${t}:${minor ? "m" : "d"}` }, keyName({ tonic: t, minor })));
      }
    }
    keySel.replaceChildren(...opts);
    keySel.value = st.keyChoice ? `${st.keyChoice.tonic}:${st.keyChoice.minor ? "m" : "d"}` : "auto";
  }

  const key = (): KeySpec => (st.view ? { tonic: st.view.key.tonic, minor: st.view.key.minor } : st.keyChoice ?? { tonic: 0, minor: false });

  function takeSelection(explicit: boolean): void {
    const found = findClip(project(), selection.peek());
    if (!found || found.clip.source.type !== "notes") {
      if (explicit) notify("Bitte zuerst einen Noten-Clip mit dem Thema in der Zeitleiste anklicken.");
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
    if (await act(backend.fugueAddVoices(voices, start, st.kind, label))) notify(`${label}: ${voices.length} Spur${voices.length === 1 ? "" : "en"} angelegt`);
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
    return card("Thema und Antwort", `${v.key.name}, ${Math.round(v.length * 10) / 10} Schläge, ${noteName(v.range[0])}–${noteName(v.range[1])}`,
      canvas,
      h("div", { class: "fg-legend" }, h("i", { style: "background:#6ee7d8" }), "Thema (Dux)", h("i", { style: "background:#ffb454" }),
        `Antwort (Comes, ${v.answer.kind === "tonal" ? "tonal" : "real"})`, v.answer.mutated.length ? h("i", { style: "background:#ff8a95" }) : null, v.answer.mutated.length ? "veränderte Töne" : null),
      h("p", { class: "fg-text" }, v.answer.reason),
      h("div", { class: "fg-actions" },
        h("button", { class: "btn small", onclick: () => play([v.notes]) }, icon("play"), "Thema"),
        h("button", { class: "btn small", onclick: () => play([v.answer.notes]) }, icon("play"), "Antwort"),
        h("button", { class: "btn small", title: "Thema, dann Antwort mit dem ersten Gegensatz-Vorschlag (falls berechnet)",
          onclick: () => play([v.notes, v.answer.notes.map((n) => ({ ...n, start: n.start + gap })), ...(st.cs?.[0] ? [st.cs[0].notes.map((n) => ({ ...n, start: n.start + gap, pitch: n.pitch + 7 }))] : [])]) },
          icon("play"), "Thema → Antwort"),
        h("button", { class: "btn small blue", title: "Antwort als neue Spur direkt nach dem Thema", onclick: () => void write([{ name: "Antwort", notes: v.answer.notes }], v.start + gap, "Antwort") }, "+ Antwort als Spur"),
        h("button", { class: "btn small ghost", title: "Ähnliche Themen in der Bibliothek suchen", onclick: () => st.subject && void searchLibraryFor({ ...st.subject, text: st.onlyFugues ? "fug" : "" }) },
          icon("book"), "Ähnliche Themen"),
        h("label", { class: "fg-check", title: "Nur Stücke mit „Fuge/Fugue/Fuga“ im Titel" },
          (() => { const c = h("input", { type: "checkbox", checked: st.onlyFugues }); c.addEventListener("change", () => (st.onlyFugues = c.checked)); return c; })(), "nur Fugen")));
  }

  function transformCard(v: SubjectView): HTMLElement {
    const rows = v.transforms.map((t) => {
      const c = rollCanvas(36);
      requestAnimationFrame(() => drawRoll(c, [{ notes: t.notes, color: "#c3a6ff" }], v.beatsPerBar));
      return h("div", { class: "fg-row" }, h("b", {}, t.label), c,
        h("button", { class: "btn small", title: noteList(t.notes), onclick: () => play([t.notes]) }, icon("play")),
        h("button", { class: "btn small blue", title: "Als neue Spur an der Stelle des Themas", onclick: () => void write([{ name: `${v.clipName} – ${t.label}`, notes: t.notes }], v.start, t.label) }, "+ Spur"));
    });
    const stepInput = h("input", { class: "num fg-num", type: "number", min: -14, max: 14, step: 1, value: String(st.shift), title: "Stufen in der Tonart (negativ = tiefer)" });
    stepInput.addEventListener("change", () => (st.shift = Number(stepInput.value) || 0));
    stepInput.addEventListener("keydown", (e) => e.stopPropagation());
    const shifted = async () => {
      if (!st.subject) return null;
      const t: TransformSpec = { type: "versetzung", value: st.shift };
      return backend.fugueTransform(st.subject.trackId, st.subject.clipId, key(), t).catch((e) => (notify(errorText(e), "error"), null));
    };
    rows.push(h("div", { class: "fg-row" }, h("b", {}, "Versetzen um"), h("span", { class: "fg-inline" }, stepInput, " Stufen"),
      h("button", { class: "btn small", onclick: async () => { const n = await shifted(); if (n) play([n]); } }, icon("play")),
      h("button", { class: "btn small blue", onclick: async () => { const n = await shifted(); if (n) void write([{ name: `${v.clipName} – ${st.shift > 0 ? "+" : ""}${st.shift} Stufen`, notes: n }], v.start, "Versetzung"); } }, "+ Spur")));
    return card("Verwandlungen", "Umkehrung diatonisch um den ersten Ton", ...rows);
  }

  function csCard(v: SubjectView): HTMLElement {
    const list = h("div", { class: "fg-list" });
    if (st.cs) {
      if (!st.cs.length) list.append(h("div", { class: "lib-empty" }, "Kein Gegensatz gefunden."));
      st.cs.forEach((c, i) => {
        const canvas = rollCanvas(48);
        requestAnimationFrame(() => drawRoll(canvas, [{ notes: v.notes, color: "#6ee7d8" }, { notes: c.notes, color: "#ffb454" }], v.beatsPerBar));
        list.append(h("div", { class: "fg-row cs" },
          h("b", {}, `${i + 1}. ${c.above ? "über" : "unter"} dem Thema`),
          canvas,
          h("span", { class: `fg-badge ${c.errors ? "bad" : "ok"}` }, c.errors ? `${c.errors} Fehler` : "fehlerfrei"),
          h("span", { class: `fg-badge ${c.invertible ? "ok" : "warn"}`, title: `Vertauscht (Oktavtausch): ${c.invertedErrors} Fehler, ${c.invertedWarnings} Warnungen` }, c.invertible ? "umkehrbar" : "nicht umkehrbar"),
          h("button", { class: "btn small", title: "Mit dem Thema zusammen anhören", onclick: () => play([v.notes, c.notes]) }, icon("play")),
          h("button", { class: "btn small blue", title: "Als neue Spur zum Thema", onclick: () => void write([{ name: "Gegensatz", notes: c.notes }], v.start, "Gegensatz") }, "+ Spur")));
      });
    }
    return card("Gegensatz", "im doppelten Kontrapunkt der Oktave geprüft",
      h("div", { class: "fg-actions" }, h("button", { class: "btn small primary", onclick: async () => {
        if (!st.subject) return;
        try {
          st.cs = await backend.fugueCountersubjects(st.subject.trackId, st.subject.clipId, key());
        } catch (e) {
          notify(errorText(e), "error");
        }
        render();
      } }, st.cs ? "Neu berechnen" : "Vorschläge berechnen")), list);
  }

  function strettoCard(v: SubjectView): HTMLElement {
    if (!v.strettos.length) return card("Engführungen", null, h("div", { class: "lib-empty" }, "Keine fehlerfreie Engführung im Schlagraster gefunden – das Thema eignet sich eher für Durchführungen."));
    return card("Engführungen", "Thema mit sich selbst, versetzt einsetzend – engste zuerst",
      ...v.strettos.map((s) => {
        const canvas = rollCanvas(40);
        requestAnimationFrame(() => drawRoll(canvas, [{ notes: v.notes, color: "#6ee7d8" }, { notes: s.follower, color: "#ffb454" }], v.beatsPerBar));
        return h("div", { class: "fg-row st" },
          h("b", {}, `${s.label}, nach ${s.delay} Schl.`),
          canvas,
          h("span", { class: "fg-badge ok", title: "Anteil, in dem beide Stimmen gleichzeitig das Thema spielen" }, `${Math.round(s.overlap * 100)} % eng`),
          h("button", { class: "btn small", onclick: () => play([v.notes, s.follower]) }, icon("play")),
          h("button", { class: "btn small blue", title: "Beide Stimmen als neue Spuren hinter dem Projektende", onclick: () => void write([{ name: "Engführung – Dux", notes: v.notes }, { name: "Engführung – Comes", notes: s.follower }], projectEnd(), "Engführung") }, "+ Spuren"));
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
        } }, p ? "Neu berechnen" : "Plan berechnen"),
        p ? h("button", { class: "btn small", onclick: () => play(p.voices.map((x) => x.notes)) }, icon("play"), "Anhören") : null,
        p ? h("button", { class: "btn small blue", title: "Je Stimme eine Spur hinter dem Projektende anlegen", onclick: () => void write(p.voices.map((x) => ({ name: `Fuge – ${x.name}`, notes: x.notes })), projectEnd(), "Fuge anlegen") }, icon("plus"), "Fuge anlegen") : null),
    ];
    if (p) {
      const bpb = st.view?.beatsPerBar ?? 4;
      const bar = h("div", { class: "fg-sections" },
        ...p.sections.map((s) => {
          const seg = h("div", { class: "fg-sec", title: `${s.name} (${s.key}): Takt ${Math.floor(s.start / bpb) + 1}–${Math.ceil(s.end / bpb)}\n${s.description}` }, s.name);
          seg.style.flex = String(s.end - s.start);
          seg.style.setProperty("--c", sectionColor(s.name));
          return seg;
        }));
      const canvas = rollCanvas(110);
      const colors = ["#6ee7d8", "#ffb454", "#c3a6ff", "#7aa7ff"];
      requestAnimationFrame(() => drawRoll(canvas, p.voices.map((v, i) => ({ notes: v.notes, color: colors[i % colors.length]! })), bpb));
      kids.push(bar, canvas,
        h("div", { class: "fg-legend" }, ...p.voices.flatMap((v, i) => [h("i", { style: `background:${colors[i % colors.length]}` }), v.name])),
        h("ul", { class: "fg-seclist" }, ...p.sections.map((s) => h("li", {}, h("i", { style: `background:${sectionColor(s.name)}` }),
          h("b", {}, s.name), ` · T. ${Math.floor(s.start / bpb) + 1}–${Math.ceil(s.end / bpb)} · ${s.key} – `, h("span", { class: "muted" }, s.description)))),
        h("p", { class: "fg-text muted" }, `${Math.ceil(p.length / bpb)} Takte, ${p.answer === "tonal" ? "tonale" : "reale"} Antwort${p.countersubject ? ", mit Gegensatz" : ""}${p.stretto ? `, Engführung: ${p.stretto}` : ""}. Ein Gerüst zum Weiterschreiben – danach mit dem Prüfer durchsehen.`));
    }
    return card("Fugenplan", `${st.voices} Stimmen`, ...kids);
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
    return card("Kontrapunkt-Prüfer", "alle Instrumentspuren, je Spur eine Stimme",
      h("div", { class: "fg-actions" },
        h("button", { class: "btn small primary", title: "Alle Instrumentspuren als Stimmen nach den Regeln des Kontrapunkts prüfen (Parallelen, Stimmkreuzungen, Dissonanzen)", onclick: () => void run() }, "Prüfen"),
        h("label", { class: "fg-check", title: "Nach jeder Änderung neu prüfen" }, live, "laufend"),
        h("label", { class: "fg-check", title: "Auch leichte Hinweise zeigen, nicht nur Fehler und Warnungen" }, hints, "Hinweise zeigen"),
        c ? h("button", { class: "btn small ghost", title: "Markierungen in der Zeitleiste entfernen", onclick: () => { st.check = null; applyMarks(); renderCheck(); } }, "Markierungen aus") : null),
      c ? h("div", { class: "fg-sum" },
        h("span", { class: `fg-badge ${c.errors ? "bad" : "ok"}` }, `${c.errors} Fehler`),
        h("span", { class: `fg-badge ${c.warnings ? "warn" : "ok"}` }, `${c.warnings} Warnungen`),
        h("span", { class: "fg-badge" }, `${c.hints} Hinweise`),
        h("span", { class: "muted" }, ` · ${c.voices.map((v) => v.name).join(", ") || "keine Stimmen"}`)) : h("div", { class: "lib-empty" }, "Prüft Quint- und Oktavparallelen, verdeckte Parallelen, Stimmkreuzung, Dissonanzbehandlung, Umfang und Sprünge."),
      c ? h("div", { class: "fg-issues" }, ...(issues.length ? issues.slice(0, 200).map((i) =>
        h("button", { class: `fg-issue ${i.severity}`, title: "Zur Stelle springen", onclick: () => {
          const bpm = project()?.tempoBpm ?? 120;
          void backend.seek((i.start * 60) / bpm);
          if (i.trackIds[0] != null) selection.value = { trackId: i.trackIds[0], clipId: null };
        } }, h("b", {}, `T. ${i.bar}`), h("span", {}, i.message), h("em", {}, i.trackIds.map((t) => names.get(t) ?? "").join(" · ")))) : [h("div", { class: "lib-empty" }, "Keine Satzfehler gefunden.")])) : null);
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
    subjectSel.title = v ? `Thema: ${noteList(v.notes)}` : "Welcher Noten-Clip das Fugenthema ist";
    checkEl = checkCard();
    if (!v) {
      body.replaceChildren(
        h("div", { class: "fg-empty" },
          h("b", {}, "So geht’s"),
          h("ol", {},
            h("li", {}, "Ein Fugenthema als Noten in eine Instrumentspur schreiben (2–4 Takte, z. B. mit der Pianorolle oder durch Vorsingen in der Bibliothek)."),
            h("li", {}, "Oben unter „Thema“ den Clip wählen (oder ihn in der Zeitleiste anklicken und „Auswahl“ drücken)."),
            h("li", {}, "Antwort, Verwandlungen, Gegensatz und Engführungen anhören – oder gleich einen Fugenplan berechnen und als Spuren anlegen."),
            h("li", {}, "Mit dem Kontrapunkt-Prüfer Parallelen und Dissonanzen finden; die Stellen sind in der Zeitleiste markiert."))),
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
