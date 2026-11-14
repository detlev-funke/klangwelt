// Export-Dialog: Format (WAV, FLAC, Opus, Stems, MIDI), Bittiefe bzw.
// Opus-Bitrate, ganzes Stück oder markierter Bereich, Dateiname. Der Dialog
// bleibt beim Exportieren offen und zeigt danach Datei, Größe und Länge – mit
// „Im Ordner zeigen“. Die letzte Wahl merkt sich der Browser-Speicher.

import { backend } from "../bridge.js";
import { formatTime, h, icon } from "../dom.js";
import { language, plural, t } from "../i18n.js";
import { errorText, notify, snapshot } from "../store.js";
import type { ExportFormat, ExportInfo, ExportResult } from "../types.js";

const STORE = "klangwelt.export";

const FORMATS: { id: ExportFormat; label: string; ext: string; text: string }[] = [
  { id: "wav", label: "WAV", ext: ".wav", text: t("Uncompressed and lossless – for further editing and mastering.") },
  { id: "flac", label: "FLAC", ext: ".flac", text: t("Losslessly compressed: same sound as WAV, but smaller – about half the size at 16 bit.") },
  { id: "opus", label: "Opus", ext: ".opus", text: t("Small and sounds very good – for listening on your phone and sharing.") },
  { id: "stems", label: "Stems", ext: "", text: t("A folder with one WAV per audible track, all the same length – for further mixing in another program.") },
  { id: "midi", label: "MIDI", ext: ".mid", text: t("Only the notes of the instrument tracks (with tempo and time signature) – for notation and other music software.") },
];

const BITRATE_HINT: Record<number, string> = {
  64: t("speech, sketches"),
  96: t("on the go"),
  128: t("good"),
  160: t("recommended – barely distinguishable from the original"),
  192: t("very good"),
  256: t("transparent"),
  320: t("maximum"),
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
  if (b >= 1e9) return `${(b / 1e9).toLocaleString(language(), { maximumFractionDigits: 1 })} GB`;
  if (b >= 1e6) return `${(b / 1e6).toLocaleString(language(), { maximumFractionDigits: 1 })} MB`;
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
  const box = h("div", { class: "modal export-modal", role: "dialog", "aria-label": t("Export") });
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
      b.title = t("No notes in audible instrument tracks – MIDI contains only notes, no recordings.");
    }
    if (f.id === "opus" && noOpus) {
      b.disabled = true;
      b.title = t("Opus isn't available in this version (built without libopus) – choose WAV or FLAC.");
    }
    fmtSeg.append(b);
    return b;
  });

  // --- Bittiefe
  const bitsSeg = h("div", { class: "seg mini ex-bits" });
  const bitButtons = ([16, 24] as const).map((b) => {
    const btn = h("button", { class: "seg-btn", "data-bits": String(b), onclick: () => { prefs.bits = b; update(); } }, t("{bits}-bit", { bits: b }));
    btn.title = b === 24 ? t("Full resolution – for further editing") : t("CD quality, smaller (with dither)");
    bitsSeg.append(btn);
    return btn;
  });
  const bitsRow = h("label", { class: "ex-row" }, h("span", { class: "ex-label" }, t("Bit depth")), bitsSeg);

  // --- Bitrate
  const rate = h("select", { class: "lib-input ex-bitrate", title: t("Data rate: higher = better and larger") },
    ...info.bitrates.map((r) => h("option", { value: String(r) }, `${r} kbit/s – ${BITRATE_HINT[r] ?? ""}`)));
  if (!info.bitrates.includes(prefs.bitrate)) prefs.bitrate = info.defaultBitrate;
  rate.value = String(prefs.bitrate);
  rate.addEventListener("change", () => { prefs.bitrate = Number(rate.value); update(); });
  const rateRow = h("label", { class: "ex-row" }, h("span", { class: "ex-label" }, t("Bitrate")), rate);

  // --- Bereich
  const rangeBox = h("input", { type: "checkbox", class: "ex-range" });
  rangeBox.disabled = !info.range;
  rangeBox.addEventListener("change", () => { useRange = rangeBox.checked; update(); });
  const rangeRow = h("label", { class: "ex-row ex-check", title: info.range ? t("Export only the time range selected in the timeline") : t("Drag out a time range in the timeline to export only that") },
    rangeBox, h("span", {}, info.rangeLabel ? t("Selected range only ({range})", { range: info.rangeLabel }) : t("Selected range only (nothing selected)")));

  // --- Name
  const name = h("input", { type: "text", class: "lib-input modal-input ex-name", value: info.name, placeholder: t("File name (or full path)") });
  name.addEventListener("input", () => update());
  const hint = h("div", { class: "modal-hint ex-hint" });
  const size = h("div", { class: "ex-size" });

  const body = h("div", { class: "modal-body" },
    fmtSeg, fmtText, bitsRow, rateRow, rangeRow, name, hint, size);
  const go = h("button", { class: "btn blue ex-go", onclick: () => void run() }, icon("download"), t("Export"));
  const cancel = h("button", { class: "btn ghost", onclick: () => close() }, t("Cancel"));
  const actions = h("div", { class: "modal-actions" }, cancel, go);
  box.append(h("div", { class: "modal-title" }, t("Export")), body, actions);

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
      ? t("Will be saved to {path}.", { path: shown })
      : clash
        ? t("\"{name}\" already exists in {dir} – it will be overwritten.", { name: shown, dir: info.dir })
        : t("Will be saved as \"{name}\" in {dir}.", { name: shown, dir: info.dir });
    hint.classList.toggle("warn", clash);
    const secs = seconds();
    const empty = secs <= 0 && !(useRange && info.range);
    if (empty) {
      size.textContent = t("The project is empty – there's nothing to export.");
    } else {
      const pcm = secs * 48000 * 2 * (prefs.bits / 8);
      const est = f.id === "wav" ? pcm : f.id === "flac" ? pcm * (prefs.bits === 24 ? 0.8 : 0.6) : f.id === "opus" ? secs * prefs.bitrate * 125 : f.id === "stems" ? pcm * Math.max(1, audibleTracks) : 0;
      const time = formatTime(secs);
      if (f.id === "midi") {
        size.textContent = `${useRange ? t("Range: {time}", { time }) : t("Length: {time}", { time })} · ${t("notes only")}`;
      } else {
        const len = useRange ? t("Range: {time} (plus tail)", { time }) : t("Length: {time} (plus tail)", { time });
        const tracks = f.id === "stems" ? ` ${plural(audibleTracks, "({n} track)", "({n} tracks)")}` : "";
        size.textContent = `${len} · ${t("48 kHz stereo")} · ${t("about {size}", { size: formatBytes(est) })}${tracks}`;
      }
    }
    go.disabled = empty || busy;
  }

  async function run(): Promise<void> {
    if (busy || go.disabled) return;
    busy = true;
    savePrefs(prefs);
    for (const el of box.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>("button, input, select")) el.disabled = true;
    go.replaceChildren(h("span", { class: "ex-spin" }), t("Exporting…"));
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
      go.replaceChildren(icon("download"), t("Export"));
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
    const what = r.format === "stems" ? plural(r.files.length, "{n} track", "{n} tracks") : f?.label ?? r.format;
    body.replaceChildren(
      h("div", { class: "ex-done" }, icon("check"), h("span", {}, `${file}`)),
      h("div", { class: "ex-text" }, `${what} · ${formatBytes(r.bytes)} · ${formatTime(r.seconds)}`),
      h("div", { class: "modal-hint mono ex-path" }, r.path),
    );
    // Browser: die Datei wurde schon heruntergeladen – der Knopf lädt sie nochmal
    const web = backend.platform === "web";
    const reveal = h("button", { class: "btn blue ex-reveal", onclick: () => { void backend.exportReveal(r.path).catch((e) => notify(errorText(e), "error")); } }, icon(web ? "download" : "folder"), web ? t("Download again") : t("Show in folder"));
    const done = h("button", { class: "btn ghost ex-close", onclick: () => close() }, t("Done"));
    actions.replaceChildren(done, reveal);
    reveal.focus();
    notify(t("Exported: {file}", { file }));
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
