import { backend } from "../bridge.js";
import { redo, stopToStart, toggleLoop, togglePlay, toggleRecord, undo } from "../actions.js";
import { LOGO, dropdown, fitCanvas, formatBarBeat, formatTime, h, icon } from "../dom.js";
import { t } from "../i18n.js";
import { effect } from "../signals.js";
import { windowMenu } from "../layout.js";
import { settingsMenu } from "./settings.js";
import { fileControls, projectLabel } from "./fileops.js";
import { act, agentPulse, chat, errorText, notify, project, run, showPanel, snapshot, status } from "../store.js";
import type { AudioDevices, AudioKind } from "../types.js";

/** Text der Agent-Anzeige oben rechts. */
export function agentLabel(serverRunning: boolean, connected: boolean, listening: boolean): string {
  if (!serverRunning) return t("Agent off");
  if (listening) return t("Agent listening");
  return connected ? t("Agent ready") : t("No agent connected");
}

const KIND_WORD: Record<AudioKind, { title: string; short: string }> = {
  output: { title: t("Output device"), short: t("Output") },
  input: { title: t("Recording device"), short: t("Recording") },
};

/** Gerät wählen (null = Systemstandard) – wirkt sofort und wird gemerkt. */
async function chooseDevice(kind: AudioKind, name: string | null): Promise<void> {
  if (await act(backend.setAudioDevice(kind, name))) notify(`${KIND_WORD[kind].short}: ${name ?? t("System default")}`);
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
      option(null, t("System default"), deflt ? t("currently: {device}", { device: deflt }) : t("System default device")),
      h("div", { class: "menu-hint" }, kind === "output" ? t("In the browser, Klangwelt plays through the system's output device.") : t("In the browser, Klangwelt records from the microphone you allow the browser to use.")),
    );
    return;
  }
  const hints: Node[] = [];
  const e = snapshot.peek()?.engine;
  const notice = kind === "output" ? e?.outputNotice : e?.inputNotice;
  if (notice) hints.push(h("div", { class: "menu-hint dev-notice" }, notice));
  if (recording) hints.push(h("div", { class: "menu-hint" }, t("The device can't be changed during a recording.")));
  if (!list.length) hints.push(h("div", { class: "menu-hint" }, kind === "output" ? t("No output device found.") : t("No microphone found.")));
  hints.push(h("div", { class: "menu-hint" }, kind === "output" ? t("Takes effect immediately and also applies the next time you start. Playback continues on the new device.") : t("Takes effect immediately and also applies the next time you start.")));
  menu.replaceChildren(
    h("div", { class: "menu-title" }, KIND_WORD[kind].title),
    option(null, t("System default"), deflt ? t("currently: {device}", { device: deflt }) : t("System default device")),
    h("div", { class: "menu-sep" }),
    ...list.map((n) => option(n, n, n === deflt ? t("System default device") : null)),
    ...hints,
  );
}

/** Knopf mit Menü zur Wahl des Ausgabe- oder Aufnahmegeräts. */
function deviceMenu(kind: AudioKind, btn: HTMLButtonElement): HTMLElement {
  const wrap = dropdown(btn, "dev-menu", (menu, close) => {
    menu.dataset.kind = kind;
    menu.replaceChildren(h("div", { class: "menu-title" }, KIND_WORD[kind].title), h("div", { class: "menu-hint" }, t("Searching for devices…")));
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
    notify(a?.error ?? t("Agent interface not available"), "error");
    return;
  }
  showPanel("agent");
}

class KwTransport extends HTMLElement {
  #hold = { out: 0, inp: 0 };

  connectedCallback(): void {
    if (this.childElementCount) return;

    const play = h("button", { class: "btn big primary", title: t("Play / pause (Space)"), onclick: () => togglePlay() }, icon("play"));
    const stop = h("button", { class: "btn big", title: t("Stop – back to the start, including the view (Home)"), onclick: () => stopToStart() }, icon("stop"));
    const loop = h("button", { class: "btn icon", "data-help": "wiederholen", onclick: () => void toggleLoop() }, icon("loop"));
    const rec = h("button", { class: "btn big rec", title: t("Record from the microphone (R)"), onclick: () => toggleRecord() }, icon("record"));

    const pos = h("div", { class: "big" }, "1.1.1");
    const time = h("div", { class: "small" }, "0:00.00");

    const tempo = h("input", { class: "num", type: "number", min: 20, max: 400, step: 1, title: t("Project tempo in BPM") });
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

    const metro = h("button", { class: "btn icon", "data-help": "metronom", title: t("Metronome"), onclick: () => act(backend.setMetronome(!snapshot.peek()?.engine.metronome)) }, icon("metronome"));
    const undoBtn = h("button", { class: "btn icon", "data-help": "rueckgaengig", onclick: () => undo() }, icon("undo"));
    const redoBtn = h("button", { class: "btn icon", "data-help": "wiederherstellen", onclick: () => redo() }, icon("redo"));

    const meter = h("canvas", { class: "meter", title: t("Levels: output on top, microphone below") });
    // Lautsprecher: zeigt das Ausgabegerät, Klick öffnet die Gerätewahl; daneben das Mikrofon
    const outLabel = h("span", { class: "dev-label" });
    const chip = h("button", { class: "chip audio-out", "data-help": "ausgabegeraet" }, icon("speaker"), outLabel);
    const micBtn = h("button", { class: "btn icon audio-in", "data-help": "aufnahmegeraet" }, icon("mic"));
    const agent = h("button", { class: "chip agent", onclick: () => openAgentChat() });

    const projName = h("div", { class: "proj-name" });
    effect(() => {
      const s = snapshot.value;
      const file = s?.file?.split(/[\\/]/).pop()?.replace(/\.klangwelt$/i, "");
      projName.textContent = `${file ?? projectLabel(s?.project.name ?? "")}${s?.dirty ? " •" : ""}`;
      projName.title = s?.file ?? t("Not saved yet");
    });
    this.append(
      h("div", { class: "brand", html: `${LOGO}<span>Klangwelt</span>` }),
      fileControls(),
      projName,
      h("div", { class: "sep" }),
      h("div", { class: "group" }, stop, play, rec),
      h("div", { class: "lcd", title: t("Playback position: bar.beat.sixteenth and time") }, pos, time),
      h("div", { class: "field", "data-help": "tempo" }, h("label", {}, t("Tempo")), tempo),
      h("div", { class: "field", "data-help": "takt", title: t("Time signature of the project") }, h("label", {}, t("Time signature")), sig),
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
      if (!ok) agent.title = a?.error ?? t("Agent endpoint not started");
      else if (web && listening) agent.title = t("An agent is listening – tasks in the chat are picked up right away.\nClick to open the agent chat.");
      else if (web) agent.title = t("MCP server inside the page (no network) – for an agent with a browser tool like \"Claude in Chrome\".\nClick to open the agent chat with the prompt to copy.");
      else if (listening) agent.title = t("An agent is listening – tasks in the chat are picked up right away.\nEndpoint: {url}\nClick to open the agent chat.", { url: String(a!.url) });
      else if (connected) agent.title = t("An agent is connected but isn't listening in the chat right now.\nEndpoint: {url}\nClick to open the agent chat – it explains how to make it listen.", { url: String(a!.url) });
      else agent.title = t("No agent connected yet – the endpoint is waiting at {url}.\nClick to open the agent chat: copy the prompt there and paste it into Claude Code.", { url: String(a!.url) });
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
      undoBtn.title = snap.undoLabel ? t("Undo: {label} (Ctrl+Z)", { label: snap.undoLabel }) : t("Nothing to undo");
      redoBtn.title = snap.redoLabel ? t("Redo: {label} (Ctrl+Y)", { label: snap.redoLabel }) : t("Nothing to redo");
      metro.classList.toggle("on", snap.engine.metronome);
      loop.classList.toggle("on", !!snap.engine.loopOn);
      const r = snap.engine.loopRange;
      // Takt.Schlag (Sechzehntel nur, wenn nötig): 4–8 Schläge in 4/4 = „Takt 2.1 bis 3.1“
      const at = (b: number) => formatBarBeat(b, p.beatsPerBar).replace(/\.1$/, "");
      const loopState = r
        ? snap.engine.loopOn
          ? t("Loop on (L) – selected range, bars {from} to {to}", { from: at(r[0]), to: at(r[1]) })
          : t("Loop off (L) – selected range, bars {from} to {to}", { from: at(r[0]), to: at(r[1]) })
        : snap.engine.loopOn
          ? t("Loop on (L) – back to the start at the end of the piece")
          : t("Loop off (L) – back to the start at the end of the piece");
      loop.title = `${loopState}\n${t("Select a range: drag in the ruler with Shift or Ctrl")}`;
      const e = snap.engine;
      chip.classList.toggle("warn", !e.available || !!e.outputNotice);
      const outWhich = e.outputChoice ? t("selected") : t("System default");
      chip.title = e.available
        ? `${t("Output device: {device} ({which}) · {rate} Hz", { device: String(e.outputDevice), which: outWhich, rate: e.sampleRate })}${e.outputNotice ? `\n${e.outputNotice}` : ""}\n${t("Click to choose a different device.")}`
        : `${e.error ?? t("No audio")}\n${t("Click to show the output devices.")}`;
      outLabel.textContent = e.available ? `${e.outputDevice ?? t("Audio")} · ${(e.sampleRate / 1000).toFixed(1)} kHz` : e.error ?? t("no audio");
      micBtn.classList.toggle("warn", !!e.inputNotice);
      micBtn.title = `${t("Recording device: {device}", { device: e.inputChoice ?? t("System default") })}${e.inputNotice ? `\n${e.inputNotice}` : ""}\n${t("Click to choose a different microphone.")}`;
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
