import { h } from "../dom.js";
import { effect } from "../signals.js";
import { message } from "../store.js";

class KwToast extends HTMLElement {
  connectedCallback(): void {
    effect(() => {
      const m = message.value;
      this.replaceChildren(m ? h("div", { class: `toast ${m.kind}`, role: "status" }, m.text) : "");
    });
  }
}

customElements.define("kw-toast", KwToast);
