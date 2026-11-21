// Settings (gear in the transport bar). Stored per device: localStorage, the interface size
// in the desktop shell's settings file (uiprefs.ts).

import { AUTOSAVE_CHOICES, autosaveSeconds, setAutosaveSeconds, switchLanguageKeepingProject } from "../autosave.js";
import { backend } from "../bridge.js";
import { dropdown, h, icon, setRange } from "../dom.js";
import { flag } from "../flags.js";
import { AUTOSAVE_HELP, SMOOTH_HELP } from "../help.js";
import { LANGUAGES, language, t } from "../i18n.js";
import { highOnTop, setHighOnTop } from "../pianoroll.js";
import { SMOOTH_LEVELS, setSmoothLevel, smoothLabel, smoothLevel, smoothTitle } from "../smoothing.js";
import { effect } from "../signals.js";
import { errorText, notify } from "../store.js";
import { DENSITIES, type Density, SCALE_MAX, SCALE_MIN, SCALE_STEP, density, setDensity, setUiScale, uiScale } from "../uiprefs.js";

/** The project page; the desktop app opens it in the system's browser. */
export const PROJECT_URL = "https://github.com/detlev-funke/klangwelt";
import { whatsNewMenuItem, whatsNewUnread } from "./whatsnew.js";

/** Small sketch of a keyboard with the chosen direction (h = high notes on top). */
function axisSketch(high: boolean): SVGSVGElement {
  // 7 keys stacked, black keys on the right; labels low/high.
  const keys = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
  const black = [1, 3, 6, 8, 10];
  const lane = 3;
  const order = high ? [...keys].reverse() : keys;
  const rects = order
    .map((k, i) => (black.includes(k) ? `<rect x="11" y="${2 + i * lane}" width="9" height="${lane}" rx="0.8" fill="#171b23"/>` : ""))
    .join("");
  const svg = `<svg viewBox="0 0 42 40" aria-hidden="true"><rect x="2" y="2" width="18" height="${keys.length * lane}" rx="1.5" fill="#d9dde5"/>${rects}
    <text x="23" y="8" font-size="6.5" fill="currentColor"></text>
    <text x="23" y="37" font-size="6.5" fill="currentColor"></text></svg>`;
  const tpl = document.createElement("template");
  tpl.innerHTML = svg;
  const el = tpl.content.firstElementChild as SVGSVGElement;
  const [top, bottom] = el.querySelectorAll("text");
  top!.textContent = high ? t("high") : t("low");
  bottom!.textContent = high ? t("low") : t("high");
  return el;
}

/** Autosave interval: off or every few minutes (see autosave.ts). */
function autosaveOptions(close: () => void): HTMLElement {
  const now = autosaveSeconds.peek();
  return h(
    "div",
    { class: "autosave-grid", role: "group", "aria-label": t("Save automatically") },
    ...AUTOSAVE_CHOICES.map((s) =>
      h(
        "button",
        {
          class: `dev-option autosave-option${s === now ? " on" : ""}`,
          "data-autosave": String(s),
          "aria-pressed": s === now ? "true" : "false",
          onclick: () => {
            close();
            setAutosaveSeconds(s);
          },
        },
        s === 0 ? t("Off") : t("{n} min", { n: s / 60 }),
      ),
    ),
  );
}

/** Smoothing when a recording becomes notes: off, light, medium, strong (smoothing.ts). */
function smoothOptions(close: () => void): HTMLElement {
  const now = smoothLevel.peek();
  return h(
    "div",
    { class: "autosave-grid smooth-grid", role: "group", "aria-label": t("Smooth notes from recordings") },
    ...SMOOTH_LEVELS.map((level) =>
      h(
        "button",
        {
          class: `dev-option autosave-option smooth-option${level === now ? " on" : ""}`,
          "data-smooth": level,
          "aria-pressed": level === now ? "true" : "false",
          title: smoothTitle(level),
          onclick: () => {
            close();
            setSmoothLevel(level);
          },
        },
        smoothLabel(level),
      ),
    ),
  );
}

/** Language list (a page of its own in the gear menu): every language with its flag and in its
 *  own name; switching reloads the UI – the open project is saved first and opened again (autosave.ts). */
function languageOptions(close: () => void): HTMLElement {
  const now = language();
  return h(
    "div",
    { class: "lang-grid", role: "group", "aria-label": "Language" },
    ...LANGUAGES.map((l) =>
      h(
        "button",
        {
          class: `dev-option lang-option${l.code === now ? " on" : ""}`,
          lang: l.code,
          "data-lang": l.code,
          "aria-current": l.code === now ? "true" : null,
          onclick: () => {
            close();
            if (l.code !== now) void switchLanguageKeepingProject(l.code);
          },
        },
        h("span", { class: "dev-check" }, l.code === now ? icon("check") : null),
        flag(l.code),
        h("span", { class: "dev-text" }, h("b", {}, l.name)),
      ),
    ),
  );
}

/** An ⓘ button and the explanation it unfolds below a setting: the text is also the button's
 *  tooltip, but tooltips never show for a finger. `help`: the help window's entry (F1, help.ts). */
function infoToggle(help: string, full: string): { info: HTMLButtonElement; more: HTMLElement } {
  const more = h("div", { class: "menu-hint menu-more", hidden: true, "data-help": help }, full);
  const info: HTMLButtonElement = h(
    "button",
    {
      class: "info-btn",
      title: full,
      "aria-label": t("More about this"),
      "aria-expanded": "false",
      onclick: () => {
        more.hidden = !more.hidden;
        info.setAttribute("aria-expanded", more.hidden ? "false" : "true");
      },
    },
    icon("help"),
  );
  return { info, more };
}

/** A setting without a visible description: its title with an ⓘ, the options (the explanation as their
 *  tooltip) and the explanation to unfold. */
function explained(help: string, title: string, options: HTMLElement | HTMLElement[], full: string): HTMLElement[] {
  const { info, more } = infoToggle(help, full);
  const list = [options].flat();
  for (const o of list) {
    o.dataset.help = help;
    if (!o.title) o.title = full;
  }
  return [h("div", { class: "menu-title with-info", "data-help": help }, title, info), ...list, more];
}

/** "Language" is always shown in English, too – whoever ended up in a foreign language finds the way back. */
const languageTitle = () => (language() === "en" ? "Language" : `${t("Language")} · Language`);

/** The entry in the gear menu: the current language with its flag; opens the list. */
function languageEntry(open: () => void): HTMLElement {
  const now = LANGUAGES.find((l) => l.code === language()) ?? LANGUAGES[0]!;
  return h(
    "button",
    { class: "set-option lang-entry", "aria-haspopup": "true", "aria-expanded": "false", onclick: open },
    flag(now.code),
    h("span", {}, h("b", { lang: now.code }, now.name), h("small", {}, languageTitle())),
    h("span", { class: "lang-next", "aria-hidden": "true" }, icon("forward")),
  );
}

/** Interface size (slider) and spacing (tight, normal, airy) – per device, applied at once (uiprefs.ts).
 *  The size only where it can change: the desktop app, phones and tablets – not in a browser on a
 *  computer, whose own zoom (Ctrl + / Ctrl −) does that (CSS zoom breaks hit tests, docs/ARCHITECTURE.md). */
function displayOptions(): HTMLElement[] {
  const percent = (s: number) => t("{n}%", { n: Math.round(s * 100) });
  const value = h("span", { class: "scale-value" }, percent(uiScale.peek()));
  const slider = h("input", {
    type: "range",
    class: "scale-slider",
    min: String(Math.round(SCALE_MIN * 100)),
    max: String(Math.round(Math.min(SCALE_MAX, backend.uiScaleMax()) * 100)),
    step: String(Math.round(SCALE_STEP * 100)),
    "aria-label": t("Interface size"),
  });
  const reset = h("button", { class: "btn small scale-reset", title: t("Back to 100%"), onclick: () => void apply(1) }, percent(1));
  const about = t("Smaller shows more at once – handy on a phone. Remembered on this device.");
  const { info, more } = infoToggle("einstellungen-groesse", about);
  slider.title = about;
  const sizeRows = [
    h("div", { class: "scale-row size-row" }, h("span", { class: "scale-label with-info" }, t("Interface size"), info), value),
    h("div", { class: "scale-row size-row" }, slider, reset),
    more,
  ];
  const show = () => {
    const s = uiScale.peek();
    reset.disabled = s === 1;
    setRange(slider, Math.round(s * 100));
    value.textContent = percent(s);
    slider.setAttribute("aria-valuetext", percent(s));
  };
  const apply = async (s: number) => {
    try {
      await setUiScale(s);
    } catch (e) {
      notify(errorText(e), "error");
    }
    // the browser turned out not to follow (a touch screen on a computer): no slider here
    if (!backend.uiScaleWorks()) for (const el of sizeRows) el.remove();
    show();
  };
  // live number while dragging; the size changes on release – zooming under the finger would move the slider
  slider.addEventListener("input", () => {
    value.textContent = percent(Number(slider.value) / 100);
    setRange(slider, Number(slider.value));
  });
  slider.addEventListener("change", () => void apply(Number(slider.value) / 100));
  show();
  const now = density.peek();
  const titles: Record<Density, string> = {
    tight: t("Narrow gaps and margins: the most room for the content"),
    normal: t("The usual gaps and margins"),
    air: t("Wide gaps and margins: calmer, and edges are easier to grab"),
  };
  const labels: Record<Density, string> = { tight: t("Tight"), normal: t("Normal"), air: t("Airy") };
  const densityGrid = h(
    "div",
    { class: "autosave-grid density-grid", role: "group", "aria-label": t("Spacing") },
    ...DENSITIES.map((d) =>
      h(
        "button",
        {
          class: `dev-option autosave-option density-option${d === now ? " on" : ""}`,
          "data-density": d,
          "aria-pressed": d === now ? "true" : "false",
          title: titles[d],
          onclick: () => {
            setDensity(d);
            for (const b of densityGrid.querySelectorAll<HTMLElement>("[data-density]")) {
              const on = b.dataset.density === d;
              b.classList.toggle("on", on);
              b.setAttribute("aria-pressed", on ? "true" : "false");
            }
          },
        },
        labels[d],
      ),
    ),
  );
  return [
    h("div", { class: "menu-title" }, t("Display")),
    ...(backend.uiScaleWorks() ? sizeRows : []),
    h("div", { class: "scale-row density-row" }, h("span", { class: "scale-label" }, t("Spacing")), densityGrid),
  ];
}

/** The project page on GitHub: a link in the browser, the system's browser in the desktop app. */
function projectLink(close: () => void): HTMLElement {
  return h(
    "a",
    {
      class: "set-option github-item",
      href: PROJECT_URL,
      target: "_blank",
      rel: "noreferrer",
      "data-github": "",
      onclick: (e: MouseEvent) => {
        close();
        if (backend.platform !== "desktop") return;
        e.preventDefault();
        backend.openUrl(PROJECT_URL).catch((err) => notify(t("The page couldn't be opened: {error}", { error: errorText(err) }), "error"));
      },
    },
    h("span", { class: "github-icon", "aria-hidden": "true" }, icon("external")),
    h("span", {}, h("b", {}, t("Klangwelt on GitHub")), h("small", {}, PROJECT_URL.replace("https://", ""))),
  );
}

/** Arrow keys in the language list: left/right one step, up/down one row; above the first row the back button. */
function moveInLanguages(menu: HTMLElement, key: string): void {
  const back = menu.querySelector<HTMLElement>(".lang-back");
  const opts = [...menu.querySelectorAll<HTMLElement>(".lang-option")];
  if (!back || !opts.length) return;
  const cols = Math.max(1, opts.filter((o) => o.offsetTop === opts[0]!.offsetTop).length);
  const at = opts.indexOf(document.activeElement as HTMLElement);
  let next: HTMLElement | undefined;
  if (at < 0) next = key === "ArrowUp" || key === "End" ? opts.at(-1) : (opts.find((o) => o.classList.contains("on")) ?? opts[0]);
  else if (key === "ArrowLeft") next = opts[Math.max(0, at - 1)];
  else if (key === "ArrowRight") next = opts[Math.min(opts.length - 1, at + 1)];
  else if (key === "ArrowUp") next = at - cols < 0 ? back : opts[at - cols];
  else if (key === "ArrowDown") next = opts[Math.min(opts.length - 1, at + cols)];
  else if (key === "Home") next = opts[0];
  else if (key === "End") next = opts.at(-1);
  next?.focus();
}

export function settingsMenu(): HTMLElement {
  const btn = h("button", { class: "btn icon", "data-help": "einstellungen", title: t("Settings") }, icon("gear"));
  // a dot after an update, until "What's new" was opened
  effect(() => btn.classList.toggle("unread", whatsNewUnread.value));
  return dropdown(btn, "set-menu", (menu, close) => {
    // the language list is a page of its own inside the menu: on a phone there is no room beside it
    const languages = () => {
      menu.dataset.page = "languages";
      menu.replaceChildren(
        h(
          "button",
          { class: "set-option lang-back", "aria-label": t("Back"), onclick: () => main(true) },
          h("span", { class: "lang-next", "aria-hidden": "true" }, icon("back")),
          h("b", {}, languageTitle()),
        ),
        languageOptions(close),
      );
      menu.scrollTop = 0;
      menu.querySelector<HTMLElement>('.lang-option[aria-current="true"]')?.focus();
    };
    menu.onkeydown = (e: KeyboardEvent) => {
      const inLanguages = menu.dataset.page === "languages";
      if (e.key === "Escape") {
        if (inLanguages) main(true);
        else {
          close();
          btn.focus();
        }
      } else if (inLanguages && e.key === "ArrowLeft" && document.activeElement === menu.querySelector(".lang-back")) {
        main(true);
      } else if (inLanguages && ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) {
        moveInLanguages(menu, e.key);
      } else if (!inLanguages && e.key === "ArrowRight" && (document.activeElement as HTMLElement | null)?.classList.contains("lang-entry")) {
        languages();
      } else if (e.key !== "Enter" && e.key !== " " && e.key !== "Tab") {
        return;
      }
      // keys meant for the menu don't reach the app's shortcuts (Space plays, Enter goes back to the start)
      if (e.key !== "Enter" && e.key !== " " && e.key !== "Tab") e.preventDefault();
      e.stopPropagation();
    };
    const main = (focusEntry = false) => {
      menu.dataset.page = "main";
      fillMain();
      if (focusEntry) menu.querySelector<HTMLElement>(".lang-entry")?.focus();
    };
    const fillMain = () => {
      const option = (high: boolean, title: string, sub: string) =>
        h(
          "button",
          {
            class: `set-option${highOnTop.peek() === high ? " on" : ""}`,
            onclick: () => {
              setHighOnTop(high);
              close();
            },
          },
          axisSketch(high),
          h("span", {}, h("b", {}, title), h("small", {}, sub)),
        );
      menu.replaceChildren(
        languageEntry(languages),
        h("div", { class: "menu-sep" }),
        ...displayOptions(),
        h("div", { class: "menu-sep" }),
        ...explained(
          "einstellungen-tonachse",
          t("Pitch axis"),
          [
            option(false, t("Low notes at the top"), t("like a keyboard turned sideways, played from the left")),
            option(true, t("High notes at the top"), t("as in sheet music and common piano rolls")),
          ],
          t("Applies to the piano roll, keyboard, analysis and library."),
        ),
        h("div", { class: "menu-sep" }),
        ...explained("einstellungen-autosave", t("Save automatically"), autosaveOptions(close), AUTOSAVE_HELP()),
        h("div", { class: "menu-sep" }),
        ...explained("einstellungen-glaetten", t("Smooth notes from recordings"), smoothOptions(close), SMOOTH_HELP()),
        h("div", { class: "menu-sep" }),
        projectLink(close),
        whatsNewMenuItem(close),
      );
    };
    main();
  });
}
