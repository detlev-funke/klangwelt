// Kleine DOM-Helfer und Icons (inline SVG).

type Child = Node | string | number | null | undefined | false;
type Props = Record<string, unknown>;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Props = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (v == null || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
    } else if (k === "class") {
      el.className = String(v);
    } else if (k === "style") {
      el.setAttribute("style", String(v));
    } else if (k === "html") {
      el.innerHTML = String(v);
    } else if (k in el) {
      (el as unknown as Record<string, unknown>)[k] = v;
    } else {
      el.setAttribute(k, v === true ? "" : String(v));
    }
  }
  for (const c of children) if (c != null && c !== false) el.append(typeof c === "number" ? String(c) : c);
  return el;
}

const PATHS: Record<string, string> = {
  play: '<path d="M7 4.5v15l12-7.5z" fill="currentColor"/>',
  pause: '<path d="M7 4h4v16H7zM13 4h4v16h-4z" fill="currentColor"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="1.5" fill="currentColor"/>',
  record: '<circle cx="12" cy="12" r="6.5" fill="currentColor"/>',
  rewind: '<path d="M6 5h2v14H6zM20 5v14L9 12z" fill="currentColor"/>',
  metronome: '<path d="M9 3h6l4 18H5zM12 15l5-9" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" stroke-linecap="round"/>',
  loop: '<path d="M4 12.5V11a4 4 0 014-4h11M16 4l3 3-3 3M20 11.5V13a4 4 0 01-4 4H5M8 20l-3-3 3-3" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/>',
  undo: '<path d="M9 7L4 12l5 5M4.5 12H14a5 5 0 010 10h-2" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>',
  redo: '<path d="M15 7l5 5-5 5M19.5 12H10a5 5 0 000 10h2" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>',
  plus: '<path d="M12 5v14M5 12h14" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>',
  song: '<path d="M3 19h18" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" opacity=".5"/><rect x="3.5" y="13" width="4" height="4" rx="1" fill="currentColor" opacity=".55"/><rect x="8.5" y="9" width="5" height="8" rx="1" fill="currentColor" opacity=".75"/><rect x="14.5" y="5" width="6" height="12" rx="1" fill="currentColor"/>',
  grid: '<rect x="3" y="4" width="18" height="16" rx="2" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M3 9.3h18M3 14.7h18M7.5 4v16M12 4v16M16.5 4v16" stroke="currentColor" stroke-width="1.2" opacity=".55"/><rect x="4.4" y="5.4" width="2.4" height="2.6" rx=".5" fill="currentColor"/><rect x="13" y="10.8" width="2.4" height="2.6" rx=".5" fill="currentColor"/><rect x="8.6" y="16.2" width="2.4" height="2.6" rx=".5" fill="currentColor"/>',
  pencil: '<path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>',
  trash: '<path d="M5 7h14M10 7V5h4v2M7 7l1 13h8l1-13" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>',
  sparkle: '<path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8zM18.5 15l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z" fill="currentColor"/>',
  wave: '<path d="M2 12h2M6 8v8M10 4v16M14 7v10M18 10v4M22 12h0" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>',
  keys: '<rect x="3" y="5" width="18" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M8 5v9M12 5v9M16 5v9" stroke="currentColor" stroke-width="1.8"/>',
  mic: '<rect x="9" y="3" width="6" height="11" rx="3" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M5.5 11a6.5 6.5 0 0013 0M12 17.5V21" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
  speaker: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M15.5 9a4.2 4.2 0 010 6M18 6.5a7.8 7.8 0 010 11" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
  clip: '<rect x="3" y="6" width="18" height="12" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M7 12h1M10 10v4M13 9v6M16 11v2" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
  chart: '<path d="M3 17l5-6 4 3 4-7 5 5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>',
  dial: '<circle cx="12" cy="12" r="8" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M12 12l4-4" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>',
  fugue: '<path d="M3 7h18M3 12h18M3 17h18" stroke="currentColor" stroke-width="1" opacity=".45"/><path d="M5 9.5l4-5 4 5M9 14.5l4-5 4 5M13 19.5l4-5 4 5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>',
  fifths: '<circle cx="12" cy="12" r="8.5" fill="none" stroke="currentColor" stroke-width="1.7"/><circle cx="12" cy="12" r="3.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M12 3.5v5M19.4 7.7l-4.3 2.5M19.4 16.3l-4.3-2.5M12 20.5v-5M4.6 16.3l4.3-2.5M4.6 7.7l4.3 2.5" stroke="currentColor" stroke-width="1.4"/>',
  book: '<path d="M4 5.5A2.5 2.5 0 016.5 3H20v15H6.5A2.5 2.5 0 004 20.5zM4 20.5A2.5 2.5 0 016.5 18H20v3H6.5A2.5 2.5 0 014 20.5z" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/>',
  save: '<path d="M5 4h11l3 3v12a1 1 0 01-1 1H6a1 1 0 01-1-1z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M8 4v5h7V4M8 20v-6h8v6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/>',
  folder: '<path d="M3 6.5A1.5 1.5 0 014.5 5H9l2 2.5h8.5A1.5 1.5 0 0121 9v8.5a1.5 1.5 0 01-1.5 1.5h-15A1.5 1.5 0 013 17.5z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/>',
  download: '<path d="M12 4v11M7 10.5l5 5 5-5M5 19.5h14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>',
  copy: '<rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2.2" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M15.5 8.5V6.2A2.2 2.2 0 0013.3 4H6.2A2.2 2.2 0 004 6.2v7.1a2.2 2.2 0 002.2 2.2h2.3" fill="none" stroke="currentColor" stroke-width="1.8"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>',
  expr: '<path d="M3 20h18" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><path d="M6 20v-6M12 20V7M18 20v-9" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="6" cy="13" r="2" fill="currentColor"/><circle cx="12" cy="6" r="2" fill="currentColor"/><circle cx="18" cy="10" r="2" fill="currentColor"/>',
  gear: '<circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M12 2.8v2.6M12 18.6v2.6M2.8 12h2.6M18.6 12h2.6M5.5 5.5l1.8 1.8M16.7 16.7l1.8 1.8M5.5 18.5l1.8-1.8M16.7 7.3l1.8-1.8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="12" cy="12" r="6.2" fill="none" stroke="currentColor" stroke-width="1.8"/>',
  layout: '<rect x="3" y="4" width="18" height="16" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M3 9.5h18M10 9.5V20" stroke="currentColor" stroke-width="1.8"/>',
  close: '<path d="M7 7l10 10M17 7L7 17" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"/>',
  help: '<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M9.6 9.4a2.5 2.5 0 014.9.7c0 1.8-2.5 2.1-2.5 4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="12" cy="17.2" r="1.15" fill="currentColor"/>',
  search: '<circle cx="11" cy="11" r="6.5" fill="none" stroke="currentColor" stroke-width="1.9"/><path d="M16 16l4.5 4.5" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>',
};

export interface ModalAction {
  label: string;
  value: string;
  kind?: "primary" | "blue" | "ghost" | "danger";
}

/**
 * Einfacher Dialog über der App. Liefert den `value` der gewählten Aktion,
 * `null` bei Esc oder Klick daneben. Enter löst die erste Aktion aus.
 */
export function modal(title: string, body: Node[], actions: ModalAction[], focus?: HTMLElement): Promise<string | null> {
  return new Promise((resolve) => {
    const back = h("div", { class: "modal-back" });
    const done = (v: string | null) => {
      window.removeEventListener("keydown", onKey, true);
      back.remove();
      resolve(v);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        done(null);
      } else if (e.key === "Enter" && !(e.target instanceof HTMLTextAreaElement)) {
        e.preventDefault();
        e.stopPropagation();
        if (actions[0]) done(actions[0].value);
      }
    };
    const box = h(
      "div",
      { class: "modal", role: "dialog", "aria-label": title },
      h("div", { class: "modal-title" }, title),
      h("div", { class: "modal-body" }, ...body),
      h("div", { class: "modal-actions" },
        ...actions.map((a) => h("button", { class: `btn ${a.kind ?? ""}`, onclick: () => done(a.value) }, a.label))),
    );
    back.append(box);
    back.addEventListener("pointerdown", (e) => {
      if (e.target === back) done(null);
    });
    window.addEventListener("keydown", onKey, true);
    document.body.append(back);
    (focus ?? (box.querySelector(".modal-actions .btn") as HTMLElement | null))?.focus();
  });
}

/**
 * Knopf mit Aufklappmenü. `fill` baut den Inhalt bei jedem Öffnen neu und
 * bekommt `close`; ein Klick außerhalb schließt das Menü.
 */
export function dropdown(btn: HTMLButtonElement, menuClass: string, fill: (menu: HTMLElement, close: () => void) => void): HTMLElement {
  const menu = h("div", { class: `menu ${menuClass}` });
  const wrap = h("div", { class: "menu-wrap" }, btn, menu);
  const outside = (e: PointerEvent) => {
    if (!wrap.contains(e.target as Node)) close();
  };
  const close = () => {
    menu.classList.remove("open");
    window.removeEventListener("pointerdown", outside, true);
  };
  btn.addEventListener("click", () => {
    if (menu.classList.contains("open")) return close();
    fill(menu, close);
    menu.classList.add("open");
    window.addEventListener("pointerdown", outside, true);
  });
  return wrap;
}

export function icon(name: keyof typeof PATHS | string): SVGSVGElement {
  const tpl = document.createElement("template");
  tpl.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${PATHS[name] ?? ""}</svg>`;
  return tpl.content.firstElementChild as SVGSVGElement;
}

export const LOGO = `<svg viewBox="0 0 32 32"><defs><linearGradient id="lg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ffb454"/><stop offset="1" stop-color="#6ee7d8"/></linearGradient></defs><rect x="1" y="1" width="30" height="30" rx="9" fill="#171c27" stroke="url(#lg)" stroke-width="1.5"/><path d="M6 16h2.5M10.5 11v10M14.5 7v18M18.5 10v12M22.5 13v6M25.5 16h.5" stroke="url(#lg)" stroke-width="2.4" stroke-linecap="round"/></svg>`;

/** Hängt Canvas-Auflösung an die Elementgröße (inkl. devicePixelRatio) an. */
export function fitCanvas(canvas: HTMLCanvasElement): { ctx: CanvasRenderingContext2D; w: number; h: number } {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth;
  const hgt = canvas.clientHeight;
  const bw = Math.max(1, Math.round(w * dpr));
  const bh = Math.max(1, Math.round(hgt * dpr));
  if (canvas.width !== bw || canvas.height !== bh) {
    canvas.width = bw;
    canvas.height = bh;
  }
  const ctx = canvas.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h: hgt };
}

export function setRange(input: HTMLInputElement, value: number): void {
  if (document.activeElement !== input) input.value = String(value);
  const min = Number(input.min);
  const max = Number(input.max);
  input.style.setProperty("--p", `${((Number(input.value) - min) / (max - min)) * 100}%`);
}

export function formatTime(secs: number): string {
  const s = Math.max(0, secs);
  const m = Math.floor(s / 60);
  return `${m}:${(s % 60).toFixed(2).padStart(5, "0")}`;
}

export function formatBarBeat(beats: number, beatsPerBar: number): string {
  const b = Math.max(0, beats);
  const bar = Math.floor(b / beatsPerBar) + 1;
  const beat = Math.floor(b % beatsPerBar) + 1;
  const sixteenth = Math.floor((b % 1) * 4) + 1;
  return `${bar}.${beat}.${sixteenth}`;
}

const NOTE_NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "H"];
export function noteName(midi: number): string {
  return `${NOTE_NAMES[((midi % 12) + 12) % 12]}${Math.floor(midi / 12) - 1}`;
}
