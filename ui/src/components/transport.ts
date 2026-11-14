import { backend } from "../bridge.js";
import { redo, stopToStart, toggleLoop, togglePlay, toggleRecord, undo } from "../actions.js";
import { LOGO, dropdown, fitCanvas, formatBarBeat, formatTime, h, icon } from "../dom.js";
import { effect } from "../signals.js";
import { windowMenu } from "../layout.js";
import { settingsMenu } from "./settings.js";
import { fileControls } from "./fileops.js";
import { act, agentPulse, chat, errorText, notify, project, run, showPanel, snapshot, status } from "../store.js";
import type { AudioDevices, AudioKind } from "../types.js";

/** Text der Agent-Anzeige oben rechts. */
export function agentLabel(serverRunning: boolean, connected: boolean, listening: boolean): string {
  if (!serverRunning) return "Agent aus";
  if (listening) return "Agent hört zu";
  return connected ? "Agent bereit" : "Kein Agent verbunden";
}

const KIND_WORD: Record<AudioKind, { title: string; short: string }> = {
  output: { title: "Ausgabegerät", short: "Ausgabe" },
  input: { title: "Aufnahmegerät", short: "Aufnahme" },
};

/** Gerät wählen (null = Systemstandard) – wirkt sofort und wird gemerkt. */
async function chooseDevice(kind: AudioKind, name: string | null): Promise<void> {
  if (await act(backend.setAudioDevice(kind, name))) notify(`${KIND_WORD[kind].short}: ${name ?? "Systemstandard"}`);
}

/** Inhalt des Geräte-Menüs: „Systemstandard“ oben, dann die Geräte des Systems. */
function fillDeviceMenu(menu: HTMLElement, kind: AudioKind, d: AudioDevices, close: () => void): void {
  const list = kind === "output" ? d.outputs : d.inputs;
  const chosen = kind === "output" ? d.output : d.input;
  const deflt = kind === "output" ? d.defaultOutput : d.defaultInput;
  const recording = status.peek().recording;
  const option = (name: string | null, label: string, sub: string | null) =>
    h(
      "button",
      {
        class: `dev-option${chosen === name ? " on" : ""}`,
        "data-device": name ?? "",
        disabled: recording,
        onclick: () => {
          close();
          void chooseDevice(kind, name);
        },
      },
      h("span", { class: "dev-check" }, chosen === name ? icon("check") : null),
      h("span", { class: "dev-text" }, h("b", {}, label), sub ? h("small", {}, sub) : null),
    );
  // Browser: das Gerät wählt das System (Ausgabe) bzw. die Freigabe im Browser (Mikrofon)
  if (backend.platform === "web") {
    menu.replaceChildren(
      h("div", { class: "menu-title" }, KIND_WORD[kind].title),
      option(null, "Systemstandard", deflt ? `zurzeit: ${deflt}` : "Standardgerät des Systems"),
      h("div", { class: "menu-hint" }, kind === "output" ? "Im Browser spielt Klangwelt über das Ausgabegerät des Systems." : "Im Browser nimmt Klangwelt über das Mikrofon auf, das du dem Browser freigibst."),
    );
    return;
  }
  const hints: Node[] = [];
  const e = snapshot.peek()?.engine;
  const notice = kind === "output" ? e?.outputNotice : e?.inputNotice;
  if (notice) hints.push(h("div", { class: "menu-hint dev-notice" }, notice));
  if (recording) hints.push(h("div", { class: "menu-hint" }, "Während einer Aufnahme lässt sich das Gerät nicht wechseln."));
  if (!list.length) hints.push(h("div", { class: "menu-hint" }, kind === "output" ? "Kein Ausgabegerät gefunden." : "Kein Mikrofon gefunden."));
  hints.push(h("div", { class: "menu-hint" }, `Wirkt sofort und gilt auch beim nächsten Start.${kind === "output" ? " Die Wiedergabe läuft auf dem neuen Gerät weiter." : ""}`));
  menu.replaceChildren(
    h("div", { class: "menu-title" }, KIND_WORD[kind].title),
    option(null, "Systemstandard", deflt ? `zurzeit: ${deflt}` : "Standardgerät des Systems"),
    h("div", { class: "menu-sep" }),
    ...list.map((n) => option(n, n, n === deflt ? "Standardgerät des Systems" : null)),
    ...hints,
  );
}

/** Knopf mit Menü zur Wahl des Ausgabe- oder Aufnahmegeräts. */
function deviceMenu(kind: AudioKind, btn: HTMLButtonElement): HTMLElement {
  const wrap = dropdown(btn, "dev-menu", (menu, close) => {
    menu.dataset.kind = kind;
    menu.replaceChildren(h("div", { class: "menu-title" }, KIND_WORD[kind].title), h("div", { class: "menu-hint" }, "Geräte werden gesucht …"));
    backend
      .audioDevices()
      .then((d) => {
        if (menu.classList.contains("open")) fillDeviceMenu(menu, kind, d, close);
      })
      .catch((err) => menu.replaceChildren(h("div", { class: "menu-title" }, KIND_WORD[kind].title), h("div", { class: "menu-hint dev-notice" }, errorText(err))));
  });
  wrap.classList.add(kind === "output" ? "dev-out" : "dev-in");
  return wrap;
}

/** Öffnet das Fenster mit dem Agent-Chat. */
function openAgentChat(): void {
  const a = snapshot.peek()?.agent;
  // Browser: kein MCP-Server, aber das Agent-Fenster mit dem Prompt für einen Agenten mit Browser-Werkzeug
  if (!a?.url && backend.platform !== "web") {
    notify(a?.error ?? "Agent-Schnittstelle nicht verfügbar", "error");
    return;
  }
  showPanel("agent");
}

class KwTransport extends HTMLElement {
  #hold = { out: 0, inp: 0 };

  connectedCallback(): void {
    if (this.childElementCount) return;

    const play = h("button", { class: "btn big primary", title: "Wiedergabe / Pause (Leertaste)", onclick: () => togglePlay() }, icon("play"));
    const stop = h("button", { class: "btn big", title: "Stopp – zurück an den Anfang, auch die Ansicht (Pos1)", onclick: () => stopToStart() }, icon("stop"));
    const loop = h("button", { class: "btn icon", "data-help": "wiederholen", onclick: () => void toggleLoop() }, icon("loop"));
    const rec = h("button", { class: "btn big rec", title: "Aufnehmen über das Mikrofon (R)", onclick: () => toggleRecord() }, icon("record"));

    const pos = h("div", { class: "big" }, "1.1.1");
    const time = h("div", { class: "small" }, "0:00.00");

    const tempo = h("input", { class: "num", type: "number", min: 20, max: 400, step: 1, title: "Projekttempo in BPM" });
    // Jeder Tempo-Schritt rendert das ganze Stück neu. Kommen Schritte schneller
    // (Pfeiltaste gehalten), geht nur der jeweils neueste Wert hinterher, sobald
    // der vorige angekommen ist – statt eines Staus aus Zwischenwerten.
    let tempoBusy = false;
    let tempoNext: number | null = null;
    const sendTempo = (v: number): void => {
      if (tempoBusy) {
        tempoNext = v;
        return;
      }
      tempoBusy = true;
      void run({ type: "setProject", patch: { tempoBpm: v } }).then(() => {
        tempoBusy = false;
        const next = tempoNext;
        tempoNext = null;
        if (next !== null && next !== project()?.tempoBpm) sendTempo(next);
      });
    };
    tempo.addEventListener("change", () => {
      const v = Number(tempo.value);
      if (v >= 20 && v <= 400) sendTempo(v);
    });
    tempo.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") tempo.blur();
    });
    const sig = h("div", { class: "num", style: "display:flex;align-items:center;justify-content:center;width:52px" }, "4/4");

    const metro = h("button", { class: "btn icon", "data-help": "metronom", title: "Metronom", onclick: () => act(backend.setMetronome(!snapshot.peek()?.engine.metronome)) }, icon("metronome"));
    const undoBtn = h("button", { class: "btn icon", "data-help": "rueckgaengig", onclick: () => undo() }, icon("undo"));
    const redoBtn = h("button", { class: "btn icon", "data-help": "wiederherstellen", onclick: () => redo() }, icon("redo"));

    const meter = h("canvas", { class: "meter", title: "Pegel: oben Ausgang, unten Mikrofon" });
    // Lautsprecher: zeigt das Ausgabegerät, Klick öffnet die Gerätewahl; daneben das Mikrofon
    const outLabel = h("span", { class: "dev-label" });
    const chip = h("button", { class: "chip audio-out", "data-help": "ausgabegeraet" }, icon("speaker"), outLabel);
    const micBtn = h("button", { class: "btn icon audio-in", "data-help": "aufnahmegeraet" }, icon("mic"));
    const agent = h("button", { class: "chip agent", onclick: () => openAgentChat() });

    const projName = h("div", { class: "proj-name" });
    effect(() => {
      const s = snapshot.value;
      const file = s?.file?.split(/[\\/]/).pop()?.replace(/\.klangwelt$/i, "");
      projName.textContent = `${file ?? s?.project.name ?? ""}${s?.dirty ? " •" : ""}`;
      projName.title = s?.file ?? "Noch nicht gespeichert";
    });
    this.append(
      h("div", { class: "brand", html: `${LOGO}<span>Klangwelt</span>` }),
      fileControls(),
      projName,
      h("div", { class: "sep" }),
      h("div", { class: "group" }, stop, play, rec),
      h("div", { class: "lcd", title: "Abspielposition: Takt.Schlag.Sechzehntel und Zeit" }, pos, time),
      h("div", { class: "field", "data-help": "tempo" }, h("label", {}, "Tempo"), tempo),
      h("div", { class: "field", "data-help": "takt", title: "Taktart des Projekts" }, h("label", {}, "Takt"), sig),
      metro,
      loop,
      h("div", { class: "sep" }),
      undoBtn,
      redoBtn,
      h("div", { class: "spacer" }),
      windowMenu(),
      settingsMenu(),
      meter,
      agent,
      deviceMenu("output", chip),
      deviceMenu("input", micBtn),
    );

    effect(() => {
      const a = snapshot.value?.agent;
      // Browser: kein Endpunkt, aber der MCP-Server in der Seite ist immer da
      const web = backend.platform === "web";
      const ok = !!a?.url || web;
      const { agentListening, agentConnected } = chat.value;
      const listening = ok && agentListening;
      const connected = ok && (agentConnected || agentListening);
      agent.classList.toggle("warn", !connected);
      agent.classList.toggle("listening", listening);
      agent.classList.toggle("connected", connected);
      agent.replaceChildren(h("span", { class: "dot" }), agentLabel(ok, connected, listening));
      if (!ok) agent.title = a?.error ?? "Agent-Endpunkt nicht gestartet";
      else if (web && listening) agent.title = "Ein Agent hört zu – Aufträge im Chat werden sofort abgeholt.\nKlick öffnet den Agent-Chat.";
      else if (web) agent.title = "MCP-Server in der Seite (ohne Netzwerk) – für einen Agenten mit Browser-Werkzeug wie „Claude in Chrome“.\nKlick öffnet den Agent-Chat mit dem Prompt zum Kopieren.";
      else if (listening) agent.title = `Ein Agent hört zu – Aufträge im Chat werden sofort abgeholt.\nEndpunkt: ${a!.url}\nKlick öffnet den Agent-Chat.`;
      else if (connected) agent.title = `Ein Agent ist verbunden, hört aber gerade nicht im Chat zu.\nEndpunkt: ${a!.url}\nKlick öffnet den Agent-Chat – dort steht, wie er zuhört.`;
      else agent.title = `Noch kein Agent verbunden – der Endpunkt wartet unter ${a!.url}.\nKlick öffnet den Agent-Chat: dort den Prompt kopieren und in Claude Code einfügen.`;
    });
    effect(() => {
      if (!agentPulse.value) return;
      agent.classList.remove("pulse");
      void agent.offsetWidth;
      agent.classList.add("pulse");
    });

    let lastPlaying: boolean | null = null;
    effect(() => {
      const st = status.value;
      const p = project();
      if (st.playing !== lastPlaying) {
        lastPlaying = st.playing;
        play.replaceChildren(icon(st.playing ? "pause" : "play"));
      }
      rec.classList.toggle("on", st.recording);
      const bpm = p?.tempoBpm ?? 120;
      pos.textContent = formatBarBeat((st.positionSecs * bpm) / 60, p?.beatsPerBar ?? 4);
      time.textContent = formatTime(st.positionSecs);
      this.#drawMeter(meter, st.peakOut, st.peakIn);
    });

    effect(() => {
      const snap = snapshot.value;
      if (!snap) return;
      const p = snap.project;
      if (document.activeElement !== tempo) tempo.value = String(Math.round(p.tempoBpm * 10) / 10);
      sig.textContent = `${p.beatsPerBar}/${p.beatUnit}`;
      undoBtn.disabled = !snap.canUndo;
      redoBtn.disabled = !snap.canRedo;
      undoBtn.title = snap.undoLabel ? `Rückgängig: ${snap.undoLabel} (Strg+Z)` : "Nichts rückgängig zu machen";
      redoBtn.title = snap.redoLabel ? `Wiederholen: ${snap.redoLabel} (Strg+Y)` : "Nichts zu wiederholen";
      metro.classList.toggle("on", snap.engine.metronome);
      loop.classList.toggle("on", !!snap.engine.loopOn);
      const r = snap.engine.loopRange;
      // Takt.Schlag (Sechzehntel nur, wenn nötig): 4–8 Schläge in 4/4 = „Takt 2.1 bis 3.1“
      const at = (b: number) => formatBarBeat(b, p.beatsPerBar).replace(/\.1$/, "");
      loop.title = `Wiederholen ${snap.engine.loopOn ? "an" : "aus"} (L) – ${r ? `markierter Bereich, Takt ${at(r[0])} bis ${at(r[1])}` : "am Ende des Stücks wieder von vorn"}\nBereich markieren: im Lineal mit Umschalt oder Strg ziehen`;
      const e = snap.engine;
      chip.classList.toggle("warn", !e.available || !!e.outputNotice);
      const outWhich = e.outputChoice ? "gewählt" : "Systemstandard";
      chip.title = e.available
        ? `Ausgabegerät: ${e.outputDevice} (${outWhich}) · ${e.sampleRate} Hz${e.outputNotice ? `\n${e.outputNotice}` : ""}\nKlick wählt ein anderes Gerät.`
        : `${e.error ?? "Kein Audio"}\nKlick zeigt die Ausgabegeräte.`;
      outLabel.textContent = e.available ? `${e.outputDevice ?? "Audio"} · ${(e.sampleRate / 1000).toFixed(1)} kHz` : e.error ?? "kein Audio";
      micBtn.classList.toggle("warn", !!e.inputNotice);
      micBtn.title = `Aufnahmegerät: ${e.inputChoice ?? "Systemstandard"}${e.inputNotice ? `\n${e.inputNotice}` : ""}\nKlick wählt ein anderes Mikrofon.`;
    });
  }

  #drawMeter(canvas: HTMLCanvasElement, out: number, inp: number): void {
    this.#hold.out = Math.max(out, this.#hold.out * 0.88);
    this.#hold.inp = Math.max(inp, this.#hold.inp * 0.88);
    const { ctx, w, h: hh } = fitCanvas(canvas);
    ctx.clearRect(0, 0, w, hh);
    const bar = (v: number, y: number, color: string) => {
      const db = 20 * Math.log10(Math.max(v, 1e-5));
      const frac = Math.min(1, Math.max(0, (db + 60) / 60));
      const g = ctx.createLinearGradient(0, 0, w, 0);
      g.addColorStop(0, color);
      g.addColorStop(0.8, color);
      g.addColorStop(1, "#ff5d6c");
      ctx.fillStyle = g;
      ctx.fillRect(2, y, (w - 4) * frac, hh / 2 - 4);
    };
    bar(this.#hold.out, 3, "#6ee7d8");
    bar(this.#hold.inp, hh / 2 + 1, "#ffb454");
  }
}

customElements.define("kw-transport", KwTransport);
