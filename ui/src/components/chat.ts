// Chat with the agent: enter tasks, read replies.
// The agent picks up tasks with the MCP tool `chat_poll` and replies with `chat_reply`.

import { backend } from "../bridge.js";
import { h, icon } from "../dom.js";
import { language, t } from "../i18n.js";
import { chat, errorText, notify, snapshot } from "../store.js";
import type { ChatMessage } from "../types.js";
import { webAgentPrompt } from "../web/agent-prompt.js";

const STATUS: Record<string, string> = { neu: t("waiting"), inArbeit: t("in progress"), erledigt: t("done"), abgebrochen: t("cancelled") };

function time(ms: number): string {
  return new Date(ms).toLocaleTimeString(language(), { hour: "2-digit", minute: "2-digit" });
}

/** Stops a task: cancelled if nobody picked it up yet, otherwise the agent is told
 *  with every tool response (cancelRequested) until it finishes the task. */
async function stopTask(id: number): Promise<void> {
  try {
    chat.value = await backend.chatStop(id, t("Stop #{id}", { id }));
  } catch (e) {
    notify(errorText(e), "error");
  }
}

function bubble(m: ChatMessage): HTMLElement {
  const open = m.from === "user" && (m.status === "neu" || m.status === "inArbeit") && !m.cancels?.length;
  const stopping = m.status === "inArbeit" && m.cancelRequested;
  return h(
    "div",
    { class: `msg ${m.from}`, "data-id": String(m.id) },
    h("div", { class: "msg-text" }, m.text),
    m.kontext ? h("div", { class: "msg-context", title: t("Sent to the agent along with the message") }, m.kontext) : null,
    h(
      "div",
      { class: "meta" },
      m.from === "user" ? `#${m.id}` : null,
      time(m.time),
      m.claimedBy && m.status === "inArbeit"
        ? h("span", { class: "claimed", title: m.confirmed ? t("The agent confirmed the task") : t("Reserved – offered again if the agent doesn't reply in time") }, t("claimed by {client}", { client: m.claimedBy }))
        : null,
      stopping
        ? h("span", { class: "state stopping" }, t("stop requested"))
        : m.status ? h("span", { class: `state ${m.status}` }, STATUS[m.status] ?? m.status) : null,
      open && !stopping
        ? h("button", { class: "btn small ghost stop-task", title: t("Stop this task – the agent is told with its next tool call"), onclick: () => void stopTask(m.id) }, t("Stop"))
        : null,
    ),
  );
}

/**
 * The one prompt for connecting: with it a Claude Code session drives the
 * app's HTTP endpoint directly (curl) – without setting it up as an MCP server –
 * and starts a subagent that listens in the chat.
 */
export function agentPrompt(url: string): string {
  const init = `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude","version":"1"}}}`;
  const poll = `{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"chat_poll","arguments":{"timeoutSeconds":280,"client":"zuhoerer"}}}`;
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
    t("3. Listening (roles in result.instructions): start a general-purpose subagent in the background as the listener, on a small, fast model (not one that requires configured MCP tools). Give it these complete instructions plus the listener's task from prompts/get with name \"zuhoeren\", so you stay available yourself. It calls chat_poll in a loop, with a shell timeout of 300000 ms:"),
    `curl -s --max-time 295 ${url} -H "Content-Type: application/json" -d '${poll}'`,
    t("The call waits until a task arrives in the app's chat. Empty list: poll again right away. The listener confirms each task at once with chat_reply (replyTo = id, done=false) and sends it to you with SendMessage. You start a worker subagent for it with the task from prompts/get with name \"arbeiten\" plus the task – at most one per track, project-wide changes one after another – and pass follow-ups and stop messages on to it. The worker finishes with chat_reply (done=true), briefly in English. If a response reports a cancellation, continue with the shorter wait time it gives. If the listener ends, start a new one. If you can't start subagents, listen and work yourself."),
    t("Saving rounds: get_project with trackIds/fields, execute with placeholders (\"ref\": \"$bass\") and compact notes (\"C4 1 · E4 .5 v100\"), several tools in one batch call."),
    t("4. If the server responds with 401, ask me for the token and send it as the header Authorization: Bearer <token>."),
    "",
    t("Let me know once you're connected and listening."),
  ].join("\n");
}

/** Fallback in case the clipboard API refuses (e.g. window still without focus). */
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

/** Blue button that copies `text` (or the current value of `text()`) and briefly shows "Copied". */
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

/** Connect box: the one prompt with a copy button. */
function connectBox(url: string): HTMLElement {
  const text = agentPrompt(url);
  return h(
    "div",
    { class: "connect" },
    h("div", { class: "connect-row" }, h("span", { class: "connect-label" }, t("Prompt for Claude Code (just paste it):")), copyButton(text)),
    h("code", { class: "connect-cmd prose prompt", title: t("Paste this text into Claude Code") }, text),
    h("div", { class: "connect-hint" }, t("With this, Claude Code talks to the app's endpoint directly over HTTP – no MCP setup. A listener on a small model picks up your tasks in the background and confirms them at once, workers carry them out – so someone always listens, and \"Stop\" reaches the running task. Claude Code stays free.")),
  );
}

/** Browser: the same box with the prompt for an agent with a
 *  browser tool (text in `web/agent-prompt.ts`). */
function webConnectBox(): HTMLElement {
  const text = webAgentPrompt(location.href);
  return h(
    "div",
    { class: "connect" },
    h("div", { class: "connect-row" }, h("span", { class: "connect-label" }, t("Prompt for your agent (just paste it):")), copyButton(text)),
    h("code", { class: "connect-cmd prose prompt connect-prompt", title: t("Give this text to your agent") }, text),
    h("div", { class: "connect-hint" }, t("For Claude Desktop with the \"Claude in Chrome\" extension, for Playwright MCP with the \"Playwright MCP Bridge\" extension (also in Vivaldi, Brave or Edge, without a debug port) – or any other agent that can run JavaScript in a browser tab. It talks to the MCP server inside this page, without any network.")),
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
    e.stopPropagation(); // do not trigger keyboard shortcuts (Space, R …)
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
      const tasks = c.workingOn.map((id) => `#${id}`).join(", ");
      if (c.agentListening) {
        status.className = "chat-status on";
        const text = c.workingOn.length
          ? t("Agent is listening – tasks are picked up right away. Working on {tasks}.", { tasks })
          : t("Agent is listening – tasks are picked up right away.");
        status.replaceChildren(h("span", { class: "dot" }), h("span", { class: "status-text" }, text));
      } else {
        status.className = "chat-status";
        const text = c.agentWorking
          ? c.workingOn.length
            ? t("An agent is working on {tasks}, but nobody is listening right now. New tasks wait until a listener picks them up – the prompt below starts one.", { tasks })
            : t("An agent is working, but nobody is listening right now. New tasks wait until a listener picks them up – the prompt below starts one.")
          : c.agentConnected
          ? t("An agent is connected but not listening. Tasks wait until it listens – the prompt below starts listening.")
          : t("No agent connected. Tasks wait until one picks them up.");
        const parts: Node[] = [h("span", { class: "dot" }), h("span", { class: "status-text" }, text)];
        // Desktop: prompt for the HTTP endpoint; browser (no port): prompt for an agent with a browser tool
        if (url) parts.push(connectBox(url));
        else if (backend.platform === "web") parts.push(webConnectBox());
        status.replaceChildren(...parts);
      }
      // Redraw messages only when something has changed.
      // (also status changes new → in progress → done)
      const sig = c.messages.map((m) => `${m.id}:${m.status ?? ""}:${m.claimedBy ?? ""}:${m.confirmed ? 1 : 0}:${m.cancelRequested ? 1 : 0}`).join(",");
      if (sig !== rendered) {
        rendered = sig;
        list.replaceChildren(...(c.messages.length ? c.messages.map(bubble) : [h("div", { class: "chat-empty" }, t("No messages yet. Suggestions:"), suggestions)]));
        list.scrollTop = list.scrollHeight;
      }
    },
  };
}
