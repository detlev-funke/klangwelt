// Export-Dialog: Format (WAV, FLAC, Opus, Stems, MIDI), Bittiefe bzw.
// Opus-Bitrate, ganzes Stück oder markierter Bereich, Dateiname. Der Dialog
// bleibt beim Exportieren offen und zeigt danach Datei, Größe und Länge – mit
// „Im Ordner zeigen“. Die letzte Wahl merkt sich der Browser-Speicher.

import { backend } from "../bridge.js";
import { formatTime, h, icon } from "../dom.js";
import { errorText, notify, snapshot } from "../store.js";
import type { ExportFormat, ExportInfo, ExportResult } from "../types.js";

const STORE = "klangwelt.export";

const FORMATS: { id: ExportFormat; label: string; ext: string; text: string }[] = [
  { id: "wav", label: "WAV", ext: ".wav", text: "Unkomprimiert und verlustfrei – zum Weiterbearbeiten und Mastern." },
  { id: "flac", label: "FLAC", ext: ".flac", text: "Verlustfrei gepackt: derselbe Klang wie WAV, kleiner – bei 16 Bit etwa halb so groß." },
  { id: "opus", label: "Opus", ext: ".opus", text: "Klein und klingt sehr gut – zum Anhören auf dem Handy und zum Verschicken." },
  { id: "stems", label: "Stems", ext: "", text: "Ein Ordner mit einer WAV je hörbarer Spur, alle gleich lang – zum Weitermischen in einem anderen Programm." },
  { id: "midi", label: "MIDI", ext: ".mid", text: "Nur die Noten der Instrumentspuren (mit Tempo und Takt) – für Notensatz und andere Musikprogramme." },
];

const BITRATE_HINT: Record<number, string> = {
  64: "Sprache, Skizzen",
  96: "unterwegs",
  128: "gut",
  160: "empfohlen – kaum vom Original zu unterscheiden",
  192: "sehr gut",
  256: "transparent",
  320: "Höchstwert",
};

interface Prefs {
  format: ExportFormat;
  bits: 16 | 24;
  bitrate: number;
}

function loadPrefs(def: number): Prefs {
  const p: Prefs = { format: "wav", bits: 24, bitrate: def };
  try {
    const raw = JSON.parse(localStorage.getItem(STORE) ?? "{}") as Partial<Prefs>;
    if (FORMATS.some((f) => f.id === raw.format)) p.format = raw.format!;
    if (raw.bits === 16 || raw.bits === 24) p.bits = raw.bits;
    if (typeof raw.bitrate === "number" && raw.bitrate >= 32) p.bitrate = raw.bitrate;
  } catch {
    /* ohne Speicher: Standardwerte */
  }
  return p;
}

function savePrefs(p: Prefs): void {
  try {
    localStorage.setItem(STORE, JSON.stringify(p));
  } catch {
    /* egal */
  }
}

export function formatBytes(b: number): string {
  if (b >= 1e9) return `${(b / 1e9).toLocaleString("de-DE", { maximumFractionDigits: 1 })} GB`;
  if (b >= 1e6) return `${(b / 1e6).toLocaleString("de-DE", { maximumFractionDigits: 1 })} MB`;
  return `${Math.max(1, Math.round(b / 1e3))} kB`;
}

let open = false;

/** Öffnet den Export-Dialog. */
export async function exportDialog(): Promise<void> {
  if (open) return;
  let info: ExportInfo;
  try {
    info = await backend.exportInfo();
  } catch (e) {
    notify(errorText(e), "error");
    return;
  }
  open = true;
  const prefs = loadPrefs(info.defaultBitrate);
  if (prefs.format === "midi" && !info.hasNotes) prefs.format = "wav";
  // Ohne Bitraten gibt es kein Opus (Kern ohne libopus gebaut)
  const noOpus = !info.bitrates.length;
  if (prefs.format === "opus" && noOpus) prefs.format = "wav";
  let useRange = false;
  let busy = false;

  const back = h("div", { class: "modal-back" });
  const box = h("div", { class: "modal export-modal", role: "dialog", "aria-label": "Exportieren" });
  const close = () => {
    if (busy) return;
    open = false;
    window.removeEventListener("keydown", onKey, true);
    back.remove();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "Enter" && !busy && go.isConnected && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      e.stopPropagation();
      void run();
    } else {
      e.stopPropagation(); // solange der Dialog offen ist: keine Tastenkürzel (Leertaste, Entf …)
    }
  };

  // --- Format
  const fmtSeg = h("div", { class: "seg ex-format" });
  const fmtText = h("div", { class: "ex-text" });
  const fmtButtons = FORMATS.map((f) => {
    const b = h("button", { class: "seg-btn", "data-format": f.id, title: f.text, onclick: () => { prefs.format = f.id; update(); } }, f.label);
    if (f.id === "midi" && !info.hasNotes) {
      b.disabled = true;
      b.title = "Keine Noten in hörbaren Instrumentspuren – MIDI enthält nur Noten, keine Aufnahmen.";
    }
    if (f.id === "opus" && noOpus) {
      b.disabled = true;
      b.title = "Opus gibt es in dieser Version nicht (ohne libopus gebaut) – WAV oder FLAC wählen.";
    }
    fmtSeg.append(b);
    return b;
  });

  // --- Bittiefe
  const bitsSeg = h("div", { class: "seg mini ex-bits" });
  const bitButtons = ([16, 24] as const).map((b) => {
    const btn = h("button", { class: "seg-btn", "data-bits": String(b), onclick: () => { prefs.bits = b; update(); } }, `${b} Bit`);
    btn.title = b === 24 ? "Volle Auflösung – für Weiterbearbeitung" : "CD-Qualität, kleiner (mit Dither)";
    bitsSeg.append(btn);
    return btn;
  });
  const bitsRow = h("label", { class: "ex-row" }, h("span", { class: "ex-label" }, "Auflösung"), bitsSeg);

  // --- Bitrate
  const rate = h("select", { class: "lib-input ex-bitrate", title: "Datenrate: höher = besser und größer" },
    ...info.bitrates.map((r) => h("option", { value: String(r) }, `${r} kbit/s – ${BITRATE_HINT[r] ?? ""}`)));
  if (!info.bitrates.includes(prefs.bitrate)) prefs.bitrate = info.defaultBitrate;
  rate.value = String(prefs.bitrate);
  rate.addEventListener("change", () => { prefs.bitrate = Number(rate.value); update(); });
  const rateRow = h("label", { class: "ex-row" }, h("span", { class: "ex-label" }, "Bitrate"), rate);

  // --- Bereich
  const rangeBox = h("input", { type: "checkbox", class: "ex-range" });
  rangeBox.disabled = !info.range;
  rangeBox.addEventListener("change", () => { useRange = rangeBox.checked; update(); });
  const rangeRow = h("label", { class: "ex-row ex-check", title: info.range ? "Nur den in der Zeitleiste markierten Zeitraum exportieren" : "In der Zeitleiste einen Zeitraum aufziehen, um nur ihn zu exportieren" },
    rangeBox, h("span", {}, info.rangeLabel ? `Nur markierter Bereich (${info.rangeLabel})` : "Nur markierter Bereich (keiner markiert)"));

  // --- Name
  const name = h("input", { type: "text", class: "lib-input modal-input ex-name", value: info.name, placeholder: "Dateiname (oder vollständiger Pfad)" });
  name.addEventListener("input", () => update());
  const hint = h("div", { class: "modal-hint ex-hint" });
  const size = h("div", { class: "ex-size" });

  const body = h("div", { class: "modal-body" },
    fmtSeg, fmtText, bitsRow, rateRow, rangeRow, name, hint, size);
  const go = h("button", { class: "btn blue ex-go", onclick: () => void run() }, icon("download"), "Exportieren");
  const cancel = h("button", { class: "btn ghost", onclick: () => close() }, "Abbrechen");
  const actions = h("div", { class: "modal-actions" }, cancel, go);
  box.append(h("div", { class: "modal-title" }, "Exportieren"), body, actions);

  // Für die Größenschätzung: Tempo und hörbare Spuren
  const proj = snapshot.peek()?.project;
  const bpm = proj?.tempoBpm ?? 120;
  const anySolo = !!proj?.tracks.some((t) => t.solo);
  const audibleTracks = proj?.tracks.filter((t) => !t.mute && (!anySolo || t.solo)).length ?? 1;
  const seconds = () => (useRange && info.range ? (info.range[1] - info.range[0]) * 60 / bpm : info.seconds);

  function update(): void {
    const f = FORMATS.find((x) => x.id === prefs.format)!;
    fmtButtons.forEach((b) => b.classList.toggle("on", b.dataset.format === f.id));
    bitButtons.forEach((b) => b.classList.toggle("on", Number(b.dataset.bits) === prefs.bits));
    fmtText.textContent = f.text;
    bitsRow.hidden = !["wav", "flac", "stems"].includes(f.id);
    rateRow.hidden = f.id !== "opus";
    const v = name.value.trim() || info.name;
    const isPath = /[\\/:]/.test(v);
    const file = f.id === "stems" ? (isPath ? v : `${v} – Stems`) : v.toLowerCase().endsWith(f.ext) ? v : `${v}${f.ext}`;
    const shown = isPath ? file : file.replace(/[<>:"/\\|?*]/g, "_");
    const clash = !isPath && info.existing.some((e) => e.toLowerCase() === shown.toLowerCase());
    hint.textContent = isPath
      ? `Wird unter ${shown} gespeichert.`
      : clash
        ? `„${shown}“ gibt es in ${info.dir} schon – wird überschrieben.`
        : `Wird als „${shown}“ in ${info.dir} gespeichert.`;
    hint.classList.toggle("warn", clash);
    const secs = seconds();
    const empty = secs <= 0 && !(useRange && info.range);
    if (empty) {
      size.textContent = "Das Projekt ist leer – es gibt nichts zu exportieren.";
    } else {
      const pcm = secs * 48000 * 2 * (prefs.bits / 8);
      const est = f.id === "wav" ? pcm : f.id === "flac" ? pcm * (prefs.bits === 24 ? 0.8 : 0.6) : f.id === "opus" ? secs * prefs.bitrate * 125 : f.id === "stems" ? pcm * Math.max(1, audibleTracks) : 0;
      size.textContent = f.id === "midi"
        ? `${useRange ? "Bereich" : "Länge"}: ${formatTime(secs)} · nur Noten`
        : `${useRange ? "Bereich" : "Länge"}: ${formatTime(secs)} (plus Nachklang) · 48 kHz Stereo · etwa ${formatBytes(est)}${f.id === "stems" ? ` (${audibleTracks} Spuren)` : ""}`;
    }
    go.disabled = empty || busy;
  }

  async function run(): Promise<void> {
    if (busy || go.disabled) return;
    busy = true;
    savePrefs(prefs);
    for (const el of box.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>("button, input, select")) el.disabled = true;
    go.replaceChildren(h("span", { class: "ex-spin" }), "Exportiere …");
    hint.classList.remove("warn", "error");
    let result: ExportResult;
    try {
      result = await backend.exportRun({
        format: prefs.format,
        name: name.value.trim() || undefined,
        bits: prefs.bits,
        bitrate: prefs.bitrate,
        range: useRange,
      });
    } catch (e) {
      busy = false;
      for (const el of box.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>("button, input, select")) el.disabled = false;
      fmtButtons.forEach((b) => { if (b.dataset.format === "midi") b.disabled = !info.hasNotes; if (b.dataset.format === "opus") b.disabled = noOpus; });
      rangeBox.disabled = !info.range;
      go.replaceChildren(icon("download"), "Exportieren");
      hint.textContent = errorText(e);
      hint.classList.add("error");
      update();
      return;
    }
    busy = false;
    showDone(result);
  }

  function showDone(r: ExportResult): void {
    const f = FORMATS.find((x) => x.id === r.format);
    const file = r.path.split(/[\\/]/).pop() ?? r.path;
    const what = r.format === "stems" ? `${r.files.length} Spuren` : f?.label ?? r.format;
    body.replaceChildren(
      h("div", { class: "ex-done" }, icon("check"), h("span", {}, `${file}`)),
      h("div", { class: "ex-text" }, `${what} · ${formatBytes(r.bytes)} · ${formatTime(r.seconds)}`),
      h("div", { class: "modal-hint mono ex-path" }, r.path),
    );
    // Browser: die Datei wurde schon heruntergeladen – der Knopf lädt sie nochmal
    const web = backend.platform === "web";
    const reveal = h("button", { class: "btn blue ex-reveal", onclick: () => { void backend.exportReveal(r.path).catch((e) => notify(errorText(e), "error")); } }, icon(web ? "download" : "folder"), web ? "Nochmal herunterladen" : "Im Ordner zeigen");
    const done = h("button", { class: "btn ghost ex-close", onclick: () => close() }, "Fertig");
    actions.replaceChildren(done, reveal);
    reveal.focus();
    notify(`Exportiert: ${file}`);
  }

  back.append(box);
  back.addEventListener("pointerdown", (e) => {
    if (e.target === back) close();
  });
  window.addEventListener("keydown", onKey, true);
  document.body.append(back);
  update();
  setTimeout(() => name.select(), 0);
}
