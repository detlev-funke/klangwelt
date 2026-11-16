// Help window: shows the detailed help text for the element under the mouse.
// F1 pins the text (lock icon) and opens the window in any case,
// even if it was closed; F1 on the same element unpins it again.

import { h, icon } from "../dom.js";
import { helpFor, type HelpTopic, type HelpView } from "../help.js";
import { t } from "../i18n.js";
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
  if (inHelp(pointer.el)) return; // the help does not explain itself
  const v = helpFor(pointer.el, pointer.x, pointer.y);
  if (!v || same(v, current)) return;
  current = v;
  refresh();
}

/** Track mouse movement (once at startup). */
export function initHelp(): void {
  const follow = (e: PointerEvent) => {
    const el = e.target instanceof Element ? e.target : null;
    if (!el) return;
    pointer = { el, x: e.clientX, y: e.clientY };
    if (!queued) {
      queued = true;
      requestAnimationFrame(track);
    }
  };
  document.addEventListener("pointermove", follow, { capture: true, passive: true });
  // touch has no hovering: a tap explains what was tapped
  document.addEventListener("pointerdown", (e) => {
    if (e.pointerType !== "mouse") follow(e);
  }, { capture: true, passive: true });
}

/** F1: open the window and pin the text for the element under the mouse (again: unpin). */
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

/** Show the text for an element without pinning (e.g. in the intro tour). */
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

/** Text with **bold** as nodes (without HTML from the texts). */
function rich(text: string): Node[] {
  return text.split(/\*\*(.+?)\*\*/g).map((part, i) => (i % 2 ? h("b", {}, part) : document.createTextNode(part)));
}

const LOCK = '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="10.5" width="14" height="10" rx="2" fill="currentColor"/><path d="M8 10.5V8a4 4 0 018 0v2.5" fill="none" stroke="currentColor" stroke-width="2"/></svg>';
const UNLOCK = '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="10.5" width="14" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M8 10.5V8a4 4 0 017.6-1.7" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>';

const WELCOME: HelpTopic = {
  title: t("Help"),
  text: [
    t("Point at something with the mouse – this shows what it does and which keys it uses."),
    t("**F1** pins the text so you can read it at your leisure (lock at top right). Press F1 again on the same element – or click the lock – to release it."),
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
    this.#body.replaceChildren(...topic.text.map((p) => h("p", {}, ...rich(p))));
    this.#keys.replaceChildren(...(topic.keys ?? []).flatMap(([k, what]) => [h("kbd", {}, k), h("span", {}, what)]));
    this.#keys.hidden = !topic.keys?.length;
    this.classList.toggle("frozen", !!frozen);
    this.#lock.innerHTML = frozen ? LOCK : UNLOCK;
    this.#lock.classList.toggle("on", !!frozen);
    this.#lock.title = frozen ? t("Pinned – click or F1 releases the text") : t("Follows the mouse – F1 or click pins the text");
    this.#foot.textContent = frozen ? t("Pinned · F1 on the same element releases it") : t("F1 pins the text");
  }
}

customElements.define("kw-help", KwHelp);
