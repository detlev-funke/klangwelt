// stdio MCP bridge to the browser version – the counterpart to `klangwelt --mcp`
// on the desktop. An MCP client (e.g. Claude Code) starts this script;
// it forwards every JSON-RPC message via the Chrome DevTools Protocol to the
// open Klangwelt tab (`window.__klangweltWeb.mcp`). There the same tools
// answer as on the desktop (`klang_app::agent`).
//
// Beforehand: start Edge or Chrome with an open debug port and load Klangwelt,
//   msedge --remote-debugging-port=9222 http://localhost:1420/
// MCP configuration (Claude Code):
//   { "command": "node", "args": ["<path>/ui/scripts/web-mcp.mjs", "--port=9222"] }
//
// Node built-ins only (WebSocket and fetch are built in).
import { createInterface } from "node:readline";

const portArg = process.argv.find((a) => a.startsWith("--port="));
const port = Number(portArg?.slice(7) ?? process.env.KLANGWELT_CDP_PORT ?? 9222);

let ws = null;
let nextId = 0;
const pending = new Map();

/** Connection to the first tab in which Klangwelt is running. */
async function connect() {
  const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const pages = list.filter((p) => p.type === "page" && p.webSocketDebuggerUrl);
  for (const p of pages.sort((a, b) => Number(b.title === "Klangwelt") - Number(a.title === "Klangwelt"))) {
    const sock = new WebSocket(p.webSocketDebuggerUrl);
    await new Promise((res, rej) => {
      sock.onopen = res;
      sock.onerror = () => rej(new Error(`Tab nicht erreichbar: ${p.url}`));
    });
    sock.onmessage = (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id && pending.has(m.id)) {
        pending.get(m.id)(m);
        pending.delete(m.id);
      }
    };
    sock.onclose = () => {
      if (ws === sock) ws = null;
    };
    ws = sock;
    const ok = await evaluate("!!window.__klangweltWeb").catch(() => false);
    if (ok) return;
    sock.close();
    ws = null;
  }
  throw new Error(`Kein Klangwelt-Tab am Debug-Port ${port} – Browser mit --remote-debugging-port=${port} starten und Klangwelt öffnen.`);
}

function evaluate(expression) {
  return new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, (m) => {
      if (m.error) reject(new Error(m.error.message));
      else if (m.result.exceptionDetails) reject(new Error(m.result.exceptionDetails.exception?.description ?? m.result.exceptionDetails.text));
      else resolve(m.result.result.value);
    });
    ws.send(JSON.stringify({ id, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true } }));
  });
}

function write(msg) {
  process.stdout.write(`${JSON.stringify(msg)}\n`);
}

const queue = [];
let busy = false;

async function handle(line) {
  let msg;
  try {
    msg = JSON.parse(line.replace(/^﻿/, ""));
  } catch (e) {
    write({ jsonrpc: "2.0", id: null, error: { code: -32700, message: `Ungültiges JSON: ${e.message}` } });
    return;
  }
  try {
    if (!ws) await connect();
    const reply = await evaluate(`window.__klangweltWeb.mcp(${JSON.stringify(msg)})`);
    if (reply != null) write(reply);
  } catch (e) {
    if (msg.id != null) write({ jsonrpc: "2.0", id: msg.id, error: { code: -32603, message: String(e.message ?? e) } });
  }
}

// Messages in order (like the stdio bridge of the desktop)
createInterface({ input: process.stdin }).on("line", (line) => {
  if (!line.trim()) return;
  queue.push(line);
  if (busy) return;
  busy = true;
  void (async () => {
    while (queue.length) await handle(queue.shift());
    busy = false;
  })();
});
