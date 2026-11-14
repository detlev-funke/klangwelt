// Chat mit dem Agenten: Aufträge eingeben, Antworten lesen.
// Der Agent holt Aufträge per MCP-Werkzeug `chat_poll` ab und antwortet mit `chat_reply`.

import { backend } from "../bridge.js";
import { h, icon } from "../dom.js";
import { chat, errorText, notify, snapshot } from "../store.js";
import type { ChatMessage } from "../types.js";
import { webAgentPrompt } from "../web/agent-prompt.js";

const STATUS: Record<string, string> = { neu: "wartet", inArbeit: "in Arbeit", erledigt: "erledigt" };

function time(ms: number): string {
  return new Date(ms).toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit" });
}

function bubble(m: ChatMessage): HTMLElement {
  return h(
    "div",
    { class: `msg ${m.from}` },
    h("div", { class: "msg-text" }, m.text),
    m.kontext ? h("div", { class: "msg-context", title: "Geht mit der Nachricht an den Agenten" }, m.kontext) : null,
    h(
      "div",
      { class: "meta" },
      time(m.time),
      m.status ? h("span", { class: `state ${m.status}` }, STATUS[m.status] ?? m.status) : null,
    ),
  );
}

/**
 * Der eine Prompt zum Verbinden: Eine Claude-Code-Session bedient damit den
 * HTTP-Endpunkt der App direkt (curl) – ohne ihn als MCP-Server einzurichten –
 * und startet einen Subagenten, der im Chat zuhört.
 */
export function agentPrompt(url: string): string {
  const init = `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude","version":"1"}}}`;
  const poll = `{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"chat_poll","arguments":{"timeoutSeconds":280}}}`;
  return `Klangwelt (eine Musik-App) läuft auf diesem Rechner. Steuere sie über ihren HTTP-Endpunkt ${url} – ohne ihn als MCP-Server einzurichten. Wo die Anweisungen der App von Werkzeugen sprechen, rufst du sie so auf wie hier beschrieben.

So geht ein Aufruf: HTTP-POST an ${url} mit einer JSON-RPC-2.0-Nachricht im Body und dem Header Content-Type: application/json; die Antwort ist JSON. Am einfachsten mit curl in Bash (unter Windows Git Bash; in PowerShell curl.exe):
curl -s ${url} -H "Content-Type: application/json" -d '${init}'
Texte mit Umlauten (z. B. für chat_reply) – und in PowerShell jede Nachricht – als UTF-8-Datei schreiben und mit --data-binary @datei.json schicken. Den Kopf Mcp-Session-Id brauchst du nicht.

1. Schick initialize (wie oben) und halte dich an result.instructions aus der Antwort.
2. tools/list nennt alle Werkzeuge mit Beschreibung und Parametern. Aufruf eines Werkzeugs: {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_project","arguments":{}}} – das Ergebnis steht in result.structuredContent (als Text auch in result.content[0].text), result.isError meldet Fehler.
3. Zuhören: Starte einen allgemeinen Subagenten im Hintergrund (keinen, der eingerichtete MCP-Werkzeuge voraussetzt) und gib ihm diese ganze Anleitung mit, damit du selbst ansprechbar bleibst. Er ruft in einer Schleife chat_poll auf, mit Shell-Timeout 300000 ms:
curl -s --max-time 295 ${url} -H "Content-Type: application/json" -d '${poll}'
Der Aufruf wartet, bis im Chat der App ein Auftrag kommt. Leere Liste: sofort wieder pollen. Jeden Auftrag mit den Werkzeugen ausführen, kurz auf Deutsch mit chat_reply antworten (replyTo = id des Auftrags), dann wieder pollen. Meldet die Antwort einen Abbruch, mit der genannten kürzeren Wartezeit weitermachen. Endet der Subagent, starte einen neuen.
4. Antwortet der Server mit 401, frag mich nach dem Token und schick ihn als Header Authorization: Bearer <Token>.

Sag kurz Bescheid, sobald du verbunden bist und zuhörst.`;
}

/** Ersatzweg, falls die Zwischenablage-API ablehnt (z. B. Fenster noch ohne Fokus). */
function copyFallback(text: string): boolean {
  const ta = h("textarea", { style: "position:fixed;left:-9999px;top:0;opacity:0" });
  ta.value = text;
  document.body.append(ta);
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  ta.remove();
  return ok;
}

/** Blauer Knopf, der `text` (oder den aktuellen Wert von `text()`) kopiert und kurz „Kopiert“ zeigt. */
function copyButton(source: string | (() => string)): HTMLButtonElement {
  const get = typeof source === "string" ? () => source : source;
  const btn = h("button", { class: "btn small blue", title: "In die Zwischenablage kopieren" }, icon("copy"), "Kopieren");
  const done = () => {
    btn.classList.add("done");
    btn.replaceChildren(icon("check"), "Kopiert");
    setTimeout(() => {
      btn.classList.remove("done");
      btn.replaceChildren(icon("copy"), "Kopieren");
    }, 1600);
  };
  btn.addEventListener("click", () => {
    const text = get();
    navigator.clipboard
      .writeText(text)
      .then(done)
      .catch(() => (copyFallback(text) ? done() : notify(`Kopieren nicht möglich – bitte markieren: ${text}`, "error")));
  });
  return btn;
}

/** Verbinden-Kasten: der eine Prompt mit Kopieren-Knopf. */
function connectBox(url: string): HTMLElement {
  const text = agentPrompt(url);
  return h(
    "div",
    { class: "connect" },
    h("div", { class: "connect-row" }, h("span", { class: "connect-label" }, "Prompt für Claude Code (einfügen, fertig):"), copyButton(text)),
    h("code", { class: "connect-cmd prose prompt", title: "Diesen Text in Claude Code einfügen" }, text),
    h("div", { class: "connect-hint" }, "Claude Code bedient damit den Endpunkt der App direkt per HTTP – ohne MCP-Einrichtung – und startet einen Subagenten, der im Hintergrund zuhört. Claude Code bleibt frei."),
  );
}

/** Browser: derselbe Kasten mit dem Prompt für einen Agenten mit
 *  Browser-Werkzeug (Text in `web/agent-prompt.ts`). */
function webConnectBox(): HTMLElement {
  const text = webAgentPrompt(location.href);
  return h(
    "div",
    { class: "connect" },
    h("div", { class: "connect-row" }, h("span", { class: "connect-label" }, "Prompt für deinen Agenten (einfügen, fertig):"), copyButton(text)),
    h("code", { class: "connect-cmd prose prompt connect-prompt", title: "Diesen Text deinem Agenten geben" }, text),
    h("div", { class: "connect-hint" }, "Für Claude Desktop mit der Erweiterung „Claude in Chrome“ – oder einen anderen Agenten, der JavaScript in einem Browser-Tab ausführen kann. Er spricht mit dem MCP-Server in dieser Seite, ohne Netzwerk."),
  );
}

const SUGGESTIONS = [
  "Mach die Kick tiefer und punchiger",
  "Stimm die Kick auf den Grundton des Basses",
  "Mach den Bass wärmer mit mehr Sub",
  "Schreib eine Basslinie in a-Moll für 4 Takte",
];

export function chatPanel(): { el: HTMLElement; update(): void } {
  const list = h("div", { class: "chat-list" });
  const status = h("div", { class: "chat-status" });
  const input = h("textarea", {
    class: "chat-input",
    title: "Auftrag an den Agenten – er bekommt mit, welche Spuren zu hören sind und welcher Zeitraum markiert ist",
    rows: 2,
    placeholder: "Auftrag an den Agenten – z. B. „Mach die Kick tiefer und den Bass wärmer“ (Enter sendet, Shift+Enter neue Zeile)",
  });
  const send = async (text = input.value) => {
    const t = text.trim();
    if (!t) return;
    input.value = "";
    try {
      chat.value = await backend.chatSend(t);
    } catch (e) {
      notify(errorText(e), "error");
      input.value = t;
    }
  };
  input.addEventListener("keydown", (e) => {
    e.stopPropagation(); // Tastenkürzel (Leertaste, R …) nicht auslösen
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  });
  const suggestions = h(
    "div",
    { class: "chat-suggest" },
    ...SUGGESTIONS.map((s) => h("button", { class: "btn small ghost", title: "Diesen Auftrag an den Agenten schicken", onclick: () => void send(s) }, s)),
  );

  const el = h(
    "div",
    { class: "chat" },
    status,
    list,
    h("div", { class: "chat-compose" }, input, h("button", { class: "btn primary", title: "Auftrag senden (Enter)", onclick: () => void send() }, "Senden")),
  );

  let rendered = "\u0000";
  return {
    el,
    update() {
      const c = chat.peek();
      const url = snapshot.peek()?.agent.url;
      if (c.agentListening) {
        status.className = "chat-status on";
        status.replaceChildren(h("span", { class: "dot" }), h("span", { class: "status-text" }, "Agent hört zu – Aufträge werden sofort abgeholt."));
      } else {
        status.className = "chat-status";
        const text = c.agentConnected
          ? "Ein Agent ist verbunden, hört aber nicht zu. Aufträge warten, bis er zuhört – der Prompt unten startet das Zuhören."
          : "Kein Agent verbunden. Aufträge warten, bis einer sie abholt.";
        const parts: Node[] = [h("span", { class: "dot" }), h("span", { class: "status-text" }, text)];
        // Desktop: Prompt für den HTTP-Endpunkt; Browser (kein Port): Prompt für einen Agenten mit Browser-Werkzeug
        if (url) parts.push(connectBox(url));
        else if (backend.platform === "web") parts.push(webConnectBox());
        status.replaceChildren(...parts);
      }
      // Nachrichten nur neu zeichnen, wenn sich etwas geändert hat.
      // (auch Statuswechsel neu → in Arbeit → erledigt)
      const sig = c.messages.map((m) => `${m.id}:${m.status ?? ""}`).join(",");
      if (sig !== rendered) {
        rendered = sig;
        list.replaceChildren(...(c.messages.length ? c.messages.map(bubble) : [h("div", { class: "chat-empty" }, "Noch keine Nachrichten. Vorschläge:", suggestions)]));
        list.scrollTop = list.scrollHeight;
      }
    },
  };
}
