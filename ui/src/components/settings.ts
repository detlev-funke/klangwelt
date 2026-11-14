// Einstellungen (Zahnrad in der Transportleiste). Gespeichert im localStorage.

import { dropdown, h, icon } from "../dom.js";
import { highOnTop, setHighOnTop } from "../pianoroll.js";

/** Kleine Skizze einer Klaviatur mit der gewählten Richtung (h = hohe Töne oben). */
function axisSketch(high: boolean): SVGSVGElement {
  // 7 Tasten untereinander, schwarze Tasten rechts; Beschriftung tief/hoch.
  const keys = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
  const black = [1, 3, 6, 8, 10];
  const lane = 3;
  const order = high ? [...keys].reverse() : keys;
  const rects = order
    .map((k, i) => (black.includes(k) ? `<rect x="11" y="${2 + i * lane}" width="9" height="${lane}" rx="0.8" fill="#171b23"/>` : ""))
    .join("");
  const svg = `<svg viewBox="0 0 42 40" aria-hidden="true"><rect x="2" y="2" width="18" height="${keys.length * lane}" rx="1.5" fill="#d9dde5"/>${rects}
    <text x="23" y="8" font-size="6.5" fill="currentColor">${high ? "hoch" : "tief"}</text>
    <text x="23" y="37" font-size="6.5" fill="currentColor">${high ? "tief" : "hoch"}</text></svg>`;
  const tpl = document.createElement("template");
  tpl.innerHTML = svg;
  return tpl.content.firstElementChild as SVGSVGElement;
}

export function settingsMenu(): HTMLElement {
  const btn = h("button", { class: "btn icon", "data-help": "einstellungen", title: "Einstellungen" }, icon("gear"));
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
    menu.replaceChildren(
      h("div", { class: "menu-title" }, "Tonachse"),
      option(false, "Tiefe Töne oben", "wie eine gedrehte Klaviatur, von links gespielt"),
      option(true, "Hohe Töne oben", "wie in Noten und üblichen Pianorollen"),
      h("div", { class: "menu-hint" }, "Gilt für Notenansicht, Klaviatur, Analyse und Bibliothek."),
    );
  });
}
