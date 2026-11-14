// Hilfefenster: zeigt den ausführlichen Hilfetext zum Element unter der Maus.
// F1 hält den Text fest (Schloss-Symbol) und öffnet das Fenster in jedem Fall,
// auch wenn es geschlossen war; F1 auf demselben Element löst es wieder.

import { h, icon } from "../dom.js";
import { helpFor, type HelpTopic, type HelpView } from "../help.js";
import { showPanel, visiblePanels } from "../store.js";

let pointer: { el: Element; x: number; y: number } | null = null;
let current: HelpView | null = null;
let frozen: HelpView | null = null;
const views = new Set<KwHelp>();

const inHelp = (el: Element | null) => !!el?.closest("kw-help, .dk-tab[data-panel=hilfe]");

function same(a: HelpView | null, b: HelpView | null): boolean {
  return a?.topic === b?.topic && a?.tip === b?.tip;
}

function refresh(): void {
  for (const v of views) v.render();
}

let queued = false;
function track(): void {
  queued = false;
  if (frozen || !pointer || !visiblePanels.peek().has("hilfe")) return;
  if (inHelp(pointer.el)) return; // die Hilfe erklärt nicht sich selbst
  const v = helpFor(pointer.el, pointer.x, pointer.y);
  if (!v || same(v, current)) return;
  current = v;
  refresh();
}

/** Mausbewegung verfolgen (einmal beim Start). */
export function initHelp(): void {
  document.addEventListener(
    "pointermove",
    (e) => {
      const el = e.target instanceof Element ? e.target : null;
      if (!el) return;
      pointer = { el, x: e.clientX, y: e.clientY };
      if (!queued) {
        queued = true;
        requestAnimationFrame(track);
      }
    },
    { capture: true, passive: true },
  );
}

/** F1: Fenster öffnen und den Text zum Element unter der Maus festhalten (noch einmal: lösen). */
export function helpKey(): void {
  if (pointer && !inHelp(pointer.el)) {
    const v = helpFor(pointer.el, pointer.x, pointer.y);
    if (frozen && v && frozen.el === v.el && same(frozen, v)) {
      frozen = null;
      current = v;
    } else {
      frozen = v ?? current;
      if (v) current = v;
    }
  } else {
    frozen = frozen ? null : current;
  }
  showPanel("hilfe");
  refresh();
}

/** Text zu einem Element zeigen, ohne festzuhalten (z. B. in der Intro-Tour). */
export function helpShow(el: Element): void {
  const v = helpFor(el);
  if (!v) return;
  frozen = null;
  current = v;
  refresh();
}

export function isHelpFrozen(): boolean {
  return !!frozen;
}

/** Text mit **fett** als Knoten (ohne HTML aus den Texten). */
function rich(text: string): Node[] {
  return text.split(/\*\*(.+?)\*\*/g).map((part, i) => (i % 2 ? h("b", {}, part) : document.createTextNode(part)));
}

const LOCK = '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="10.5" width="14" height="10" rx="2" fill="currentColor"/><path d="M8 10.5V8a4 4 0 018 0v2.5" fill="none" stroke="currentColor" stroke-width="2"/></svg>';
const UNLOCK = '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="10.5" width="14" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M8 10.5V8a4 4 0 017.6-1.7" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>';

const WELCOME: HelpTopic = {
  title: "Hilfe",
  text: [
    "Zeig mit der Maus auf etwas – hier steht, was es tut und welche Tasten es gibt.",
    "**F1** hält den Text fest, damit du in Ruhe lesen kannst (Schloss oben rechts). Noch einmal F1 auf demselben Element – oder ein Klick aufs Schloss – löst ihn wieder.",
  ],
};

class KwHelp extends HTMLElement {
  #title = h("div", { class: "help-title" });
  #lock = h("button", { class: "help-lock" }) as HTMLButtonElement;
  #tip = h("div", { class: "help-tip" });
  #body = h("div", { class: "help-text" });
  #keys = h("div", { class: "help-keys" });
  #foot = h("div", { class: "help-foot" });

  connectedCallback(): void {
    if (!this.childElementCount) {
      this.#lock.addEventListener("click", () => {
        frozen = frozen ? null : current;
        refresh();
      });
      this.append(
        h("div", { class: "help-head" }, icon("help"), this.#title, this.#lock),
        h("div", { class: "help-scroll" }, this.#tip, this.#body, this.#keys),
        this.#foot,
      );
    }
    views.add(this);
    this.render();
  }

  disconnectedCallback(): void {
    views.delete(this);
  }

  render(): void {
    const v = frozen ?? current;
    const topic = v?.topic ?? WELCOME;
    this.#title.textContent = topic.title;
    this.#tip.textContent = v?.tip ?? "";
    this.#tip.hidden = !v?.tip;
    this.#body.replaceChildren(...topic.text.map((t) => h("p", {}, ...rich(t))));
    this.#keys.replaceChildren(...(topic.keys ?? []).flatMap(([k, what]) => [h("kbd", {}, k), h("span", {}, what)]));
    this.#keys.hidden = !topic.keys?.length;
    this.classList.toggle("frozen", !!frozen);
    this.#lock.innerHTML = frozen ? LOCK : UNLOCK;
    this.#lock.classList.toggle("on", !!frozen);
    this.#lock.title = frozen ? "Festgehalten – Klick oder F1 löst den Text wieder" : "Folgt der Maus – F1 oder Klick hält den Text fest";
    this.#foot.textContent = frozen ? "Festgehalten · F1 auf demselben Element löst" : "F1 hält den Text fest";
  }
}

customElements.define("kw-help", KwHelp);
