// Chat mit dem Agenten: Aufträge eingeben, Antworten lesen.
// Der Agent holt Aufträge per MCP-Werkzeug `chat_poll` ab und antwortet mit `chat_reply`.

import { backend } from "../bridge.js";
import { h, icon } from "../dom.js";
import { language, t } from "../i18n.js";
import { chat, errorText, notify, snapshot } from "../store.js";
import type { ChatMessage } from "../types.js";
import { webAgentPrompt } from "../web/agent-prompt.js";

const STATUS: Record<string, string> = { neu: t("waiting"), inArbeit: t("in progress"), erledigt: t("done") };

function time(ms: number): string {
  return new Date(ms).toLocaleTimeString(language(), { hour: "2-digit", minute: "2-digit" });
}

function bubble(m: ChatMessage): HTMLElement {
  return h(
    "div",
    { class: `msg ${m.from}` },
    h("div", { class: "msg-text" }, m.text),
    m.kontext ? h("div", { class: "msg-context", title: t("Sent to the agent along with the message") }, m.kontext) : null,
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
  // paragraphs translated one by one; the curl lines are code and stay as they are
  return [
    t("Klangwelt (a music app) is running on this computer. Control it through its HTTP endpoint {url} – without setting it up as an MCP server. Wherever the app's instructions mention tools, call them as described here.", { url }),
    "",
    t("How to make a call: HTTP POST to {url} with a JSON-RPC 2.0 message in the body and the header Content-Type: application/json; the response is JSON. Easiest with curl in Bash (on Windows Git Bash; in PowerShell curl.exe):", { url }),
    `curl -s ${url} -H "Content-Type: application/json" -d '${init}'`,
    t("Write texts with non-ASCII characters (e.g. for chat_reply) – and in PowerShell every message – to a UTF-8 file and send it with --data-binary @file.json. You don't need the Mcp-Session-Id header."),
    "",
    t("1. Send initialize (as above) and follow result.instructions from the response."),
    t("2. tools/list lists all tools with their descriptions and parameters. Calling a tool: {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_project\",\"arguments\":{}}} – the result is in result.structuredContent (as text also in result.content[0].text), result.isError reports errors."),
    t("3. Listening: start a general-purpose subagent in the background (not one that requires configured MCP tools) and give it these complete instructions, so you stay available yourself. It calls chat_poll in a loop, with a shell timeout of 300000 ms:"),
    `curl -s --max-time 295 ${url} -H "Content-Type: application/json" -d '${poll}'`,
    t("The call waits until a task arrives in the app's chat. Empty list: poll again right away. Carry out each task with the tools, reply briefly in English with chat_reply (replyTo = id of the task), then poll again. If the response reports a cancellation, continue with the shorter wait time it gives. If the subagent ends, start a new one."),
    t("4. If the server responds with 401, ask me for the token and send it as the header Authorization: Bearer <token>."),
    "",
    t("Let me know once you're connected and listening."),
  ].join("\n");
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
  const btn = h("button", { class: "btn small blue", title: t("Copy to clipboard") }, icon("copy"), t("Copy"));
  const done = () => {
    btn.classList.add("done");
    btn.replaceChildren(icon("check"), t("Copied"));
    setTimeout(() => {
      btn.classList.remove("done");
      btn.replaceChildren(icon("copy"), t("Copy"));
    }, 1600);
  };
  btn.addEventListener("click", () => {
    const text = get();
    navigator.clipboard
      .writeText(text)
      .then(done)
      .catch(() => (copyFallback(text) ? done() : notify(t("Can't copy – please select it: {text}", { text }), "error")));
  });
  return btn;
}

/** Verbinden-Kasten: der eine Prompt mit Kopieren-Knopf. */
function connectBox(url: string): HTMLElement {
  const text = agentPrompt(url);
  return h(
    "div",
    { class: "connect" },
    h("div", { class: "connect-row" }, h("span", { class: "connect-label" }, t("Prompt for Claude Code (just paste it):")), copyButton(text)),
    h("code", { class: "connect-cmd prose prompt", title: t("Paste this text into Claude Code") }, text),
    h("div", { class: "connect-hint" }, t("With this, Claude Code talks to the app's endpoint directly over HTTP – no MCP setup – and starts a subagent that listens in the background. Claude Code stays free.")),
  );
}

/** Browser: derselbe Kasten mit dem Prompt für einen Agenten mit
 *  Browser-Werkzeug (Text in `web/agent-prompt.ts`). */
function webConnectBox(): HTMLElement {
  const text = webAgentPrompt(location.href);
  return h(
    "div",
    { class: "connect" },
    h("div", { class: "connect-row" }, h("span", { class: "connect-label" }, t("Prompt for your agent (just paste it):")), copyButton(text)),
    h("code", { class: "connect-cmd prose prompt connect-prompt", title: t("Give this text to your agent") }, text),
    h("div", { class: "connect-hint" }, t("For Claude Desktop with the \"Claude in Chrome\" extension – or any other agent that can run JavaScript in a browser tab. It talks to the MCP server inside this page, without any network.")),
  );
}

const SUGGESTIONS = [
  t("Make the kick deeper and punchier"),
  t("Tune the kick to the bass's root note"),
  t("Make the bass warmer with more sub"),
  t("Write a 4-bar bass line in A minor"),
];

export function chatPanel(): { el: HTMLElement; update(): void } {
  const list = h("div", { class: "chat-list" });
  const status = h("div", { class: "chat-status" });
  const input = h("textarea", {
    class: "chat-input",
    title: t("Task for the agent – it knows which tracks are audible and which time range is selected"),
    rows: 2,
    placeholder: t("Task for the agent – e.g. \"Make the kick deeper and the bass warmer\" (Enter sends, Shift+Enter adds a new line)"),
  });
  const send = async (text = input.value) => {
    const task = text.trim();
    if (!task) return;
    input.value = "";
    try {
      chat.value = await backend.chatSend(task);
    } catch (e) {
      notify(errorText(e), "error");
      input.value = task;
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
    ...SUGGESTIONS.map((s) => h("button", { class: "btn small ghost", title: t("Send this task to the agent"), onclick: () => void send(s) }, s)),
  );

  const el = h(
    "div",
    { class: "chat" },
    status,
    list,
    h("div", { class: "chat-compose" }, input, h("button", { class: "btn primary", title: t("Send task (Enter)"), onclick: () => void send() }, t("Send"))),
  );

  let rendered = "\u0000";
  return {
    el,
    update() {
      const c = chat.peek();
      const url = snapshot.peek()?.agent.url;
      if (c.agentListening) {
        status.className = "chat-status on";
        status.replaceChildren(h("span", { class: "dot" }), h("span", { class: "status-text" }, t("Agent is listening – tasks are picked up right away.")));
      } else {
        status.className = "chat-status";
        const text = c.agentConnected
          ? t("An agent is connected but not listening. Tasks wait until it listens – the prompt below starts listening.")
          : t("No agent connected. Tasks wait until one picks them up.");
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
        list.replaceChildren(...(c.messages.length ? c.messages.map(bubble) : [h("div", { class: "chat-empty" }, t("No messages yet. Suggestions:"), suggestions)]));
        list.scrollTop = list.scrollHeight;
      }
    },
  };
}
