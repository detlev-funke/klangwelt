// Copy prompt for the agent window of the browser version. An agent with a
// browser tool (e.g. Claude Desktop with the "Claude in
// Chrome" extension) operates Klangwelt through the MCP server in the page
// (`window.__klangweltWeb.mcp`, see docs/WEB.md) – without network, without a port.
// Counterpart to `agentPrompt()` of the desktop (HTTP endpoint via curl).
//
// A module of its own, so that the desktop part of the agent window (chat.ts)
// stays untouched by it.

import { t } from "../i18n.js";

/** Wait time for `chat_poll` in the prompt: short enough for the time limit of a
 *  JavaScript tool that we do not know (if a call ends before that,
 *  the prompt says what to do then). */
export const WEB_POLL_SECONDS = 20;

/** The prompt with the address under which the page is currently running. */
export function webAgentPrompt(address: string): string {
  // paragraphs translated one by one (placeholders: address and waiting time)
  const seconds = WEB_POLL_SECONDS;
  return [
    t("Klangwelt (a music app) runs as a web page at {address}. Operate it via Claude in Chrome (or another tool that runs JavaScript in a browser tab) – without setting it up as an MCP server. Wherever the app's instructions mention tools, call them as described here.", { address }),
    "",
    t("1. Open the address in a tab of your tab group (if it's already open, use that tab) and wait until `window.__klangweltWeb` exists."),
    t("2. The page contains an MCP server without a network: `await window.__klangweltWeb.mcp(message)` takes a JSON-RPC 2.0 message (as an object) and returns the response. Operate Klangwelt only through it (with the JavaScript tool), not by clicking in the UI."),
    t("3. Initialize first: JSON.stringify(await window.__klangweltWeb.mcp({jsonrpc:\"2.0\", id:1, method:\"initialize\", params:{protocolVersion:\"2025-06-18\", capabilities:{}, clientInfo:{name:\"claude-in-chrome\", version:\"1\"}}})). Follow result.instructions from the response."),
    t("4. tools/list lists all tools with their description and parameters. Calling a tool: {jsonrpc:\"2.0\", id:<sequential>, method:\"tools/call\", params:{name:\"get_project\", arguments:{}}} – the result is in result.structuredContent (as text also in result.content[0].text); result.isError reports errors."),
    t("5. Return responses as JSON.stringify(...); for very large ones, only the parts you need."),
    t("6. Tools that don't exist in the browser say so – then don't keep trying."),
    t("7. Listening: the person gives you tasks in the page's chat. If you can start a subagent that also has the browser tool, use a general-purpose one (not one that requires configured MCP tools) and pass it these complete instructions so that you stay available yourself; otherwise, listen yourself. The loop: call chat_poll with arguments {timeoutSeconds: {seconds}} – the call waits until a task arrives, at most {seconds} s; the page stays usable meanwhile. If the JavaScript tool aborts earlier with a timeout, use 10 from then on. Empty list: call chat_poll again right away – that's normal. Carry out each task with the tools and use chat_reply (replyTo = the task's id) to say in one or two sentences in English what you changed; for longer work, send a short chat_reply with done=false first. Then call chat_poll again. Don't stop on your own – only when the person explicitly writes \"Stop\" in the chat, say goodbye via chat_reply.", { seconds }),
    "",
    t("Let me know once you're connected and listening."),
  ].join("\n");
}
