// Settings (gear in the transport bar). Stored in localStorage.

import { dropdown, h, icon } from "../dom.js";
import { flag } from "../flags.js";
import { LANGUAGES, language, switchLanguage, t } from "../i18n.js";
import { highOnTop, setHighOnTop } from "../pianoroll.js";

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

/** Language choice: every language with its flag and in its own name; switching reloads the UI. */
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
            if (l.code !== now) switchLanguage(l.code);
          },
        },
        h("span", { class: "dev-check" }, l.code === now ? icon("check") : null),
        flag(l.code),
        h("span", { class: "dev-text" }, h("b", {}, l.name)),
      ),
    ),
  );
}

export function settingsMenu(): HTMLElement {
  const btn = h("button", { class: "btn icon", "data-help": "einstellungen", title: t("Settings") }, icon("gear"));
  return dropdown(btn, "set-menu", (menu, close) => {
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
    // "Language" is always shown in English, too – whoever ended up in a foreign language finds the way back.
    const langTitle = language() === "en" ? "Language" : `${t("Language")} · Language`;
    menu.replaceChildren(
      h("div", { class: "menu-title" }, langTitle),
      languageOptions(close),
      h("div", { class: "menu-sep" }),
      h("div", { class: "menu-title" }, t("Pitch axis")),
      option(false, t("Low notes at the top"), t("like a keyboard turned sideways, played from the left")),
      option(true, t("High notes at the top"), t("as in sheet music and common piano rolls")),
      h("div", { class: "menu-hint" }, t("Applies to the piano roll, keyboard, analysis and library.")),
    );
  });
}
