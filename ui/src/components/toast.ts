import { h } from "../dom.js";
import { effect } from "../signals.js";
import { message } from "../store.js";

class KwToast extends HTMLElement {
  connectedCallback(): void {
    effect(() => {
      const m = message.value;
      const a = m?.action;
      const button = a
        ? h("button", { class: "btn toast-action", onclick: () => { message.value = null; a.run(); } }, a.label)
        : null;
      this.replaceChildren(m ? h("div", { class: `toast ${m.kind}${a ? " has-action" : ""}`, role: "status" }, m.text, button) : "");
    });
  }
}

customElements.define("kw-toast", KwToast);
