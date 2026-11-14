import { addTrack, convertAudio, resetRowHeight, startRowResize } from "../actions.js";
import { audioDrag } from "../filedrop.js";
import { backend } from "../bridge.js";
import { fitCanvas, h, icon, noteName, setRange } from "../dom.js";
import {
  EXPR_H, NOTES_TOP, RHYTHM_H, type NoteGeo, geoFor, heightDraft, highOnTop, isBlack, maxScrollY, pitchAt, pitchOfLane, pitchViews,
  refitPitch, rowHeight, scrollPitch, viewFor, wheelPitchStep, zoomPitch,
} from "../pianoroll.js";
import { EXPRS, type ExprKey, exprKeysFor, exprLanes, exprShown, laneKeyFor, setLaneKey, toggleExprLane } from "../expression.js";
import { t } from "../i18n.js";
import { effect } from "../signals.js";
import { act, instrumentSpecs, run, selection, snapshot, specFor, viewport } from "../store.js";
import { enterNote, entry, entryLetters, entryTrack, keyLabelsVersion, setEntryPosition, toggleEntry } from "../noteinput.js";
import { harmonyLaneOn, setHarmonyLane } from "../harmonyline.js";
import type { InstrumentKind, Track } from "../types.js";

/** "Harmony" button: show/hide the chord progression in the timeline. */
function harmonyToggle(): HTMLElement {
  const b = h("button", { class: "btn small harmony-toggle", title: t("Show harmony in the timeline: chords in the color of their mood, breaks in red (also appears while you work in the Harmony window)"), onclick: () => setHarmonyLane(!harmonyLaneOn.peek()) }, icon("fifths"), t("Harmony"));
  effect(() => b.classList.toggle("on", harmonyLaneOn.value));
  return b;
}

/**
 * Keyboard at the right edge of the track header (note view only).
 * Like a real piano, played from the left: white keys across the full
 * width with the key front on the left, black keys shorter and at the back
 * (right, at the timeline). The key height follows zoom and row height.
 */
function drawKeys(canvas: HTMLCanvasElement, g: NoteGeo, drumKeys?: Map<number, string>, letters?: Map<number, string> | null): void {
  const { ctx, w, h: hh } = fitCanvas(canvas);
  ctx.clearRect(0, 0, w, hh);
  const LANE_SPAN = g.view.span;
  const LANE_H = g.laneH;
  const total = g.lanesH;
  const blackW = Math.round(w * 0.6);
  const blackX = w - blackW;
  const up = highOnTop.peek();
  const pitchOf = (k: number) => pitchOfLane(g.view, k);

  // White keys as a continuous surface, lighter toward the front (left).
  const white = ctx.createLinearGradient(0, 0, w, 0);
  white.addColorStop(0, "#e4e7ee");
  white.addColorStop(1, "#c3c8d2");
  ctx.fillStyle = white;
  ctx.fillRect(0, 0, w, total);

  // Gaps between white keys.
  ctx.fillStyle = "#8a919e";
  for (let k = 0; k < LANE_SPAN; k++) {
    const pitch = pitchOf(k);
    const y = k * LANE_H;
    if (isBlack(pitch)) {
      // two white keys meet before the black key
      ctx.fillRect(0, Math.round(y + LANE_H / 2), blackX, 1);
    } else if (!isBlack(pitch + 1)) {
      // E|F and B|C: white keys directly next to each other – gap on the side of the higher note
      ctx.fillRect(0, up ? y : y + LANE_H - 1, w, 1);
    }
  }

  // Black keys: shorter, attached at the back (right), front rounded on the left.
  for (let k = 0; k < LANE_SPAN; k++) {
    const pitch = pitchOf(k);
    if (!isBlack(pitch)) continue;
    const y = k * LANE_H + 0.5;
    ctx.fillStyle = "#171b23";
    ctx.beginPath();
    ctx.roundRect(blackX, y, blackW, Math.max(2, LANE_H - 1), [2, 0, 0, 2]);
    ctx.fill();
    if (LANE_H >= 6) {
      ctx.fillStyle = "#3a4252";
      ctx.fillRect(blackX + 2, y + 1, blackW - 3, 1);
    }
  }

  // Label at the front (left) on the white key: C notes, or for drums the drum sounds.
  ctx.font = `600 ${Math.max(5.5, Math.min(7.5, LANE_H - 0.5)).toFixed(1)}px system-ui`;
  ctx.textAlign = "left";
  ctx.textBaseline = "middle";
  for (let k = 0; k < LANE_SPAN; k++) {
    const pitch = pitchOf(k);
    const y = k * LANE_H + LANE_H / 2 + 0.5;
    if (drumKeys) {
      const name = drumKeys.get(pitch);
      if (!name) continue;
      ctx.fillStyle = "rgba(255, 180, 84, 0.9)";
      ctx.fillRect(0, k * LANE_H + 1, 2, Math.max(2, LANE_H - 2));
      ctx.fillStyle = "#1b2130";
      ctx.fillText(name, 3, y);
    } else if (pitch % 12 === 0) {
      ctx.fillStyle = "#5b6271";
      ctx.fillText(noteName(pitch), 2, y);
    }
  }

  // Keyboard entry: the keys the letters play are tinted and show the letter (at the back, right).
  if (letters) {
    ctx.font = `700 ${Math.max(6, Math.min(8.5, LANE_H - 1)).toFixed(1)}px system-ui`;
    ctx.textAlign = "right";
    for (let k = 0; k < LANE_SPAN; k++) {
      const pitch = pitchOf(k);
      const letter = letters.get(pitch);
      if (!letter) continue;
      const y = k * LANE_H;
      const black = isBlack(pitch);
      ctx.fillStyle = black ? "rgba(199, 146, 255, 0.42)" : "rgba(199, 146, 255, 0.26)";
      if (black) ctx.fillRect(blackX, y + 0.5, blackW, Math.max(2, LANE_H - 1));
      else ctx.fillRect(0, y, w, LANE_H);
      if (LANE_H < 6) continue;
      ctx.fillStyle = black ? "#efe4ff" : "#4b2380";
      ctx.fillText(letter, w - 3, y + LANE_H / 2 + 0.5);
    }
  }
}

/** Short names of the drums of a drum track (key → "BD", "SD" …). */
export function drumKeyNames(t: Track): Map<number, string> | undefined {
  if (t.instrument?.kind !== "drums") return undefined;
  const keys = specFor(t.instrument)?.keys ?? [];
  return new Map(keys.map((k) => [k.midi, k.short]));
}

/** Control whose intermediate values merge into one undo step while dragging. */
function gestureRange(min: number, max: number, step: number, onValue: (v: number, merge: boolean) => void, reset?: number) {
  const input = h("input", { type: "range", min, max, step });
  let merging = false;
  input.addEventListener("pointerdown", () => (merging = false));
  input.addEventListener("input", () => {
    onValue(Number(input.value), merging);
    merging = true;
  });
  input.addEventListener("change", () => (merging = false));
  input.addEventListener("keydown", (e) => e.stopPropagation());
  if (reset !== undefined) {
    input.addEventListener("dblclick", () => {
      input.value = String(reset);
      onValue(reset, false);
    });
  }
  return input;
}

function panText(p: number): string {
  if (Math.abs(p) < 0.01) return "C";
  return `${p < 0 ? "L" : "R"} ${Math.round(Math.abs(p) * 100)}`;
}

class TrackRow {
  readonly el: HTMLDivElement;
  #track!: Track;
  #name: HTMLInputElement;
  #mute: HTMLButtonElement;
  #solo: HTMLButtonElement;
  #gain: HTMLInputElement;
  #gainVal: HTMLElement;
  #pan: HTMLInputElement;
  #panVal: HTMLElement;
  #pitchVal: HTMLElement;
  #kind: HTMLElement;
  #viewBtn: HTMLButtonElement;
  #keys: HTMLCanvasElement;
  #rhythm: HTMLDivElement;
  #exprBtn: HTMLButtonElement;
  #convertBtn: HTMLButtonElement;
  #exprHead: HTMLDivElement;
  #exprSelect: HTMLSelectElement;
  #exprHint: HTMLElement;
  #entryBtn: HTMLButtonElement;

  constructor(readonly id: number) {
    const set = (patch: object, merge = false) => run({ type: "setTrack", trackId: this.id, patch }, merge);

    this.#name = h("input", { class: "text", spellcheck: false, title: t("Track name – click and type to rename") });
    this.#name.addEventListener("change", () => {
      const v = this.#name.value.trim();
      if (v) void set({ name: v });
      else this.#name.value = this.#track.name;
    });
    this.#name.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter" || e.key === "Escape") this.#name.blur();
    });

    this.#mute = h("button", { class: "toggle m", title: t("Mute"), onclick: () => set({ mute: !this.#track.mute }) }, "M");
    this.#solo = h("button", { class: "toggle s", title: t("Solo"), onclick: () => set({ solo: !this.#track.solo }) }, "S");
    const del = h("button", { class: "btn ghost small", title: t("Delete track"), onclick: () => run({ type: "removeTrack", trackId: this.id }) }, icon("trash"));

    this.#gain = gestureRange(-60, 6, 0.5, (v, m) => set({ gainDb: v }, m), 0);
    this.#gain.title = t("Level in dB – double-click: 0 dB");
    this.#gain.dataset.help = "pegel";
    this.#gainVal = h("b");
    this.#pan = gestureRange(-1, 1, 0.01, (v, m) => set({ pan: v }, m), 0);
    this.#pan.title = t("Pan: left ↔ right – double-click: center");
    this.#pan.dataset.help = "panorama";
    this.#panVal = h("b");

    this.#pitchVal = h("span", { class: "val", title: t("Pitch in semitones") });
    const pitch = (d: number) => {
      const v = Math.max(-24, Math.min(24, Math.round(this.#track.pitchSemitones) + d));
      void set({ pitchSemitones: v });
    };
    const stepper = h(
      "div",
      { class: "stepper", title: t("Pitch (semitones) – tempo stays the same") },
      h("button", { title: t("One semitone down"), onclick: () => pitch(-1) }, "−"),
      this.#pitchVal,
      h("button", { title: t("One semitone up"), onclick: () => pitch(1) }, "+"),
    );

    this.#kind = h("span", { class: "kind" });
    this.#viewBtn = h("button", {
      class: "toggle v",
      onclick: () => set({ view: this.#track.view === "notes" ? "compact" : "notes" }),
    });

    this.#keys = h("canvas", {
      class: "keys",
      title: t("Mouse wheel: shift pitch range · Ctrl+mouse wheel: zoom · Double-click: fit · Click: play note"),
    });
    this.#keys.style.top = `${NOTES_TOP}px`;
    this.#keys.addEventListener("wheel", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const view = viewFor(this.#track);
      if (e.ctrlKey || e.metaKey) zoomPitch(this.id, e.deltaY > 0 ? 3 : -3, view);
      else scrollPitch(this.id, wheelPitchStep(e.deltaY), view); // up = what lies above
    }, { passive: false });
    this.#keys.addEventListener("dblclick", () => refitPitch(this.id));

    this.#rhythm = h("div", {
      class: "rhythm-label",
      title: t("Rhythm lane: unpitched hits like hi-hat or shaker (on instrument tracks, the note onsets)"),
    }, t("Rhythm"));
    this.#rhythm.style.height = `${RHYTHM_H}px`;

    // Expression track: which form of expression is shown and edited.
    this.#exprSelect = h("select", { class: "expr-select", title: t("Expression type shown in the expression lane") });
    this.#exprSelect.addEventListener("change", () => setLaneKey(this.id, this.#exprSelect.value as ExprKey));
    this.#exprSelect.addEventListener("pointerdown", (e) => e.stopPropagation());
    this.#exprHint = h("span", { class: "expr-range" });
    this.#exprHead = h("div", {
      class: "expr-head",
      title: t("Expression lane: drag values or paint across several notes · Shift: straight line (e.g. crescendo) · Double-click: reset"),
    }, this.#exprSelect, this.#exprHint);
    this.#exprHead.style.height = `${EXPR_H}px`;
    this.#exprBtn = h("button", {
      class: "toggle v x",
      onclick: () => {
        const t = this.#track;
        const shown = exprShown(t);
        toggleExprLane(this.id);
        // Let the stored row height grow/shrink along, so that the note lanes stay the same size.
        if (t.notesHeight > 0) void set({ notesHeight: Math.max(140, t.notesHeight + (shown ? -EXPR_H : EXPR_H)) });
      },
    }, icon("expr"));

    // Audio track in the pitch view: convert detected notes of all clips into a melody track.
    this.#convertBtn = h("button", {
      class: "toggle convert",
      title: t("Convert to notes: create the detected pitches of all clips as a melody track below (right-click a clip: also as a rhythm track)"),
      onclick: () => void convertAudio(this.id, null, "melodie"),
    }, icon("keys"), t("Notes"));

    // Keyboard entry mode: letters and clicks on the piano add notes to this track.
    this.#entryBtn = h("button", {
      class: "entry-switch",
      role: "switch",
      title: t("Keyboard entry: letter keys and clicks on the piano add notes at the violet mark – one grid step further each time, with Shift in the same place (chords)"),
      onclick: () => {
        toggleEntry(this.id);
        this.#entryBtn.blur(); // Enter (stop) must not toggle it again
      },
    }, h("span", { class: "switch" }), icon("keyboard"), t("Keyboard entry"));
    this.#entryBtn.addEventListener("pointerdown", (e) => e.stopPropagation());

    // Splitter at the bottom edge: drag a row larger/smaller individually, double-click = default height.
    const splitter = h("div", { class: "splitter", title: t("Drag: change track height · Double-click: default height") });
    splitter.addEventListener("pointerdown", (e) => {
      e.stopPropagation();
      startRowResize(this.#track, e, splitter);
    });
    splitter.addEventListener("dblclick", (e) => {
      e.stopPropagation();
      void resetRowHeight(this.#track);
    });
    this.#keys.addEventListener("pointerdown", (e) => {
      if (this.#track.kind !== "instrument") return;
      const pitch = pitchAt(geoFor(this.#track), e.offsetY + NOTES_TOP);
      const en = entry.peek();
      if (en) {
        // keyboard entry: the key adds its note (Shift: stay in place)
        if (en.trackId !== this.id) setEntryPosition(en.beat, this.id);
        enterNote(pitch, e.shiftKey);
        return;
      }
      void backend.previewNote(this.id, pitch).catch(() => {});
    });

    this.el = h(
      "div",
      { class: "track-row" },
      h("div", { class: "stripe" }),
      h(
        "div",
        { class: "inner" },
        h("div", { class: "track-top" }, this.#kind, this.#name, this.#viewBtn, this.#exprBtn, this.#convertBtn, this.#mute, this.#solo, del),
        h(
          "div",
          { class: "track-controls" },
          h("div", { class: "knob" }, h("span", {}, t("Level"), this.#gainVal), this.#gain),
          h("div", { class: "knob" }, h("span", {}, t("Pan"), this.#panVal), this.#pan),
          stepper,
        ),
        h("div", { class: "track-entry" }, this.#entryBtn),
      ),
      this.#keys,
      this.#rhythm,
      this.#exprHead,
      splitter,
    );
    this.el.dataset.trackId = String(id);
    this.el.addEventListener("pointerdown", () => {
      const sel = selection.peek();
      if (sel?.trackId !== this.id) selection.value = { trackId: this.id, clipId: null };
    });
  }

  update(track: Track, selected: boolean): void {
    const kindChanged = !this.#track || this.#track.kind !== track.kind;
    this.#track = track;
    this.el.style.setProperty("--c", track.color);
    this.el.style.height = `${rowHeight(track)}px`;
    this.el.classList.toggle("selected", selected);
    const notes = track.view === "notes";
    this.el.classList.toggle("notes", notes);
    // Swap the icon only on change: a redraw on press (track gets
    // selected) would otherwise replace the element under the mouse – the click would never arrive.
    const viewIcon = notes ? "wave" : "keys";
    if (this.#viewBtn.dataset.icon !== viewIcon) {
      this.#viewBtn.dataset.icon = viewIcon;
      this.#viewBtn.replaceChildren(icon(viewIcon));
    }
    this.#viewBtn.classList.toggle("on", notes);
    this.#viewBtn.title = notes ? t("Compact view") : track.kind === "instrument" ? t("Piano roll (editable)") : t("Show pitches (from the analysis)");
    const canExpr = notes && track.kind === "instrument";
    this.#exprBtn.hidden = !canExpr;
    this.#convertBtn.hidden = !(notes && track.kind === "audio" && track.clips.some((c) => c.source.type === "audio"));
    const typing = entryTrack.peek() === track.id;
    this.#entryBtn.hidden = !canExpr;
    this.#entryBtn.classList.toggle("on", typing);
    this.#entryBtn.setAttribute("aria-checked", String(typing));
    const exprOn = canExpr && exprShown(track);
    this.#exprBtn.classList.toggle("on", exprOn);
    this.#exprBtn.title = exprOn ? t("Hide expression lane") : t("Show expression lane (velocity, vibrato, brightness, bend per note)");
    this.el.classList.toggle("expr", exprOn);
    if (notes) {
      const g = geoFor(track);
      this.#keys.style.height = `${g.lanesH}px`;
      this.#rhythm.style.top = `${g.rhythmTop}px`;
      drawKeys(this.#keys, g, drumKeyNames(track), entryLetters(track));
      if (exprOn) {
        this.#exprHead.style.top = `${g.exprTop}px`;
        const keys = exprKeysFor(track);
        const current = laneKeyFor(track);
        const sig = keys.join(",");
        if (this.#exprSelect.dataset.keys !== sig) {
          this.#exprSelect.dataset.keys = sig;
          this.#exprSelect.replaceChildren(...keys.map((k) => h("option", { value: k }, EXPRS[k].label)));
        }
        this.#exprSelect.value = current;
        const d = EXPRS[current];
        this.#exprHint.textContent = d.bipolar ? "− · 0 · +" : current === "velocity" ? "pp … ff" : "0 … max";
        this.#exprHead.title = `${d.label}: ${d.hint}. ${t("Drag values or paint across several notes · Shift: straight line (e.g. crescendo) · Double-click: reset")}`;
      }
    }
    if (kindChanged) {
      this.#kind.replaceChildren(icon(track.kind === "audio" ? "wave" : "keys"));
      this.#kind.title = track.kind === "audio" ? t("Audio track") : t("Instrument track");
    }
    if (document.activeElement !== this.#name) this.#name.value = track.name;
    this.#mute.classList.toggle("on", track.mute);
    this.#solo.classList.toggle("on", track.solo);
    setRange(this.#gain, track.gainDb);
    this.#gainVal.textContent = `${track.gainDb > 0 ? "+" : ""}${track.gainDb.toFixed(1)} dB`;
    setRange(this.#pan, track.pan);
    this.#panVal.textContent = panText(track.pan);
    const st = Math.round(track.pitchSemitones);
    this.#pitchVal.textContent = st === 0 ? "±0" : `${st > 0 ? "+" : ""}${st}`;
  }
}

class KwTracks extends HTMLElement {
  #rows = new Map<number, TrackRow>();
  #body = h("div", { class: "tracks-body" });
  #menu!: HTMLDivElement;

  connectedCallback(): void {
    if (this.childElementCount) return;
    const head = h(
      "div",
      { class: "tracks-head" },
      h("span", {}, t("Tracks")),
      h(
        "div",
        { class: "group" },
        harmonyToggle(),
        h("button", { class: "btn small", "data-help": "neue-audiospur", title: t("Add audio track"), onclick: () => addTrack("audio") }, icon("plus"), t("Audio")),
        h("button", {
          class: "btn small",
          "data-help": "neue-instrumentspur",
          title: t("Add instrument track: pluck, organ, harpsichord, bass, synth or drums"),
          onclick: (e: MouseEvent) => {
            e.stopPropagation();
            this.#menu.classList.toggle("open");
          },
        }, icon("plus"), `${t("Instrument")} ▾`),
      ),
    );
    this.#menu = h("div", { class: "menu" });
    effect(() => {
      const specs = instrumentSpecs.value;
      const pick = (kind: InstrumentKind) => {
        this.#menu.classList.remove("open");
        void act(backend.createInstrumentTrack(kind));
      };
      this.#menu.replaceChildren(
        ...(specs.length
          ? specs.map((s) => h("button", { class: "menu-item", title: s.description, onclick: () => pick(s.kind) },
              h("b", {}, s.name), h("span", {}, s.kind === "drums" ? t("with beat") : s.kind === "bass" ? t("with bass line") : s.kind === "synth" ? t("with chord progression") : t("with arpeggio"))))
          : [h("button", { class: "menu-item", onclick: () => { this.#menu.classList.remove("open"); addTrack("instrument"); } }, h("b", {}, t("Instrument")), h("span", {}, t("Pluck")))]),
      );
    });
    document.addEventListener("click", () => this.#menu.classList.remove("open"));
    this.append(head, this.#menu, this.#body);

    effect(() => {
      instrumentSpecs.value; // drum names on the keyboard
      pitchViews.value; // keyboard follows pitch range and zoom
      heightDraft.value; // … and the row height while dragging
      highOnTop.value; // … and the direction of the pitch axis
      exprLanes.value; // … and the expression track
      entryTrack.value; // keyboard entry: switch and letters on the piano
      keyLabelsVersion.value;
      this.#render(snapshot.value?.project.tracks ?? [], selection.value?.trackId);
    });
    effect(() => {
      this.#body.style.transform = `translateY(${-viewport.value.scrollY}px)`;
    });
    // dragged audio files over a track header: highlight this track
    effect(() => {
      const d = audioDrag.value;
      const over = d ? document.elementFromPoint(d.x, d.y)?.closest(".track-row") : null;
      for (const row of this.#rows.values()) row.el.classList.toggle("drop-target", row.el === over);
    });
    this.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const max = maxScrollY(snapshot.peek()?.project.tracks ?? []);
        viewport.update((v) => ({ ...v, scrollY: Math.max(0, Math.min(max, v.scrollY + e.deltaY)) }));
      },
      { passive: false },
    );
  }

  #render(tracks: Track[], selectedId: number | undefined): void {
    const seen = new Set<number>();
    tracks.forEach((t, i) => {
      let row = this.#rows.get(t.id);
      if (!row) {
        row = new TrackRow(t.id);
        this.#rows.set(t.id, row);
      }
      row.update(t, t.id === selectedId);
      if (this.#body.children[i] !== row.el) this.#body.insertBefore(row.el, this.#body.children[i] ?? null);
      seen.add(t.id);
    });
    for (const [id, row] of this.#rows) {
      if (!seen.has(id)) {
        row.el.remove();
        this.#rows.delete(id);
      }
    }
  }
}

customElements.define("kw-tracks", KwTracks);
