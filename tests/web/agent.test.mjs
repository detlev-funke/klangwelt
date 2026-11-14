// Agent in the browser version: without a port there is no MCP server - but the same
// tools as on the desktop (klang_app::agent) are available in the page
// (`window.__klangweltWeb.mcp`) and through the stdio bridge
// (ui/scripts/web-mcp.mjs), which talks to the tab via the DevTools protocol.
import { before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { startWeb, makeTest, sleep, ROOT } from "./harness.mjs";

let app;
const test = makeTest(() => app);
before(async () => {
  app = await startWeb();
});
after(async () => {
  await app?.close();
});

const rpc = (method, params = {}, id = 1) => app.eval((m) => T.web.mcp(m), { jsonrpc: "2.0", id, method, params });

test("in the page: same tools as on the desktop, changes appear immediately", async () => {
  const init = await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } });
  assert.equal(init.result.serverInfo.name, "klangwelt");
  const tools = (await rpc("tools/list")).result.tools.map((t) => t.name);
  for (const t of ["get_project", "execute", "create_instrument_track", "write_chords", "write_song", "analyze_harmony", "chat_poll"]) assert.ok(tools.includes(t), `Werkzeug ${t} fehlt`);
  const r = await rpc("tools/call", { name: "create_instrument_track", arguments: { kind: "synth" } });
  assert.ok(!r.result.isError, JSON.stringify(r.result));
  // "project-changed" from the core: the UI shows the track without having called anything itself
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.some((t) => t.instrument?.kind === "synth"), { what: "Spur in der Oberfläche" });
  // tools that do not exist in the browser say so
  const dl = await rpc("tools/call", { name: "download_library", arguments: { id: "x" } });
  assert.equal(dl.result.isError, true);
  assert.match(dl.result.content[0].text, /platform/);
});

test("agent window in the browser: prompt to copy with the page's address", async () => {
  // the Agent button opens the window (on the desktop only with a running MCP server)
  assert.match(await app.eval(() => T.$(".chip.agent").textContent), /Kein Agent verbunden/);
  await app.clickOn(".chip.agent");
  await app.waitFor(() => T.$(".chat .connect-prompt"), { what: "Prompt im Agent-Fenster" });
  const [text, href] = await app.eval(() => [T.$(".chat .connect-prompt").textContent, location.href]);
  assert.ok(text.includes(href), `Adresse ${href} eingesetzt`);
  assert.match(text, /window\.__klangweltWeb\.mcp/);
  assert.match(text, /chat_poll mit arguments \{timeoutSeconds: 20\}/);
  assert.match(text, /result\.structuredContent/);
  assert.ok(await app.eval(() => !!T.$(".chat .connect .btn.blue")), "Kopieren-Knopf");
});

test("stdio bridge: an MCP client talks to the tab over the DevTools protocol", async () => {
  const bridge = spawn(process.execPath, [join(ROOT, "ui", "scripts", "web-mcp.mjs"), `--port=${app.cdpPort}`], { stdio: ["pipe", "pipe", "inherit"] });
  const lines = createInterface({ input: bridge.stdout });
  const replies = [];
  const waiting = [];
  lines.on("line", (l) => {
    const m = JSON.parse(l);
    const w = waiting.shift();
    if (w) w(m);
    else replies.push(m);
  });
  const ask = (msg) =>
    new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error(`keine Antwort auf ${msg.method}`)), 20000);
      waiting.push((m) => {
        clearTimeout(t);
        resolve(m);
      });
      bridge.stdin.write(`${JSON.stringify(msg)}\n`);
    });
  try {
    const init = await ask({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } } });
    assert.equal(init.id, 1);
    assert.equal(init.result.serverInfo.name, "klangwelt");
    // notification: no response; the next response belongs to the next question
    bridge.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
    const p = await ask({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "get_project", arguments: {} } });
    assert.equal(p.id, 2);
    const project = p.result.structuredContent ?? JSON.parse(p.result.content[0].text);
    assert.equal(project.tracks.length, 1, JSON.stringify(project).slice(0, 300));
    assert.deepEqual(replies, []);
  } finally {
    bridge.kill();
  }
});

const poll = (timeoutSeconds, id) => ({ jsonrpc: "2.0", id, method: "tools/call", params: { name: "chat_poll", arguments: { timeoutSeconds } } });

test("chat_poll waits (in the worker) for a task – core and UI stay usable", async () => {
  await app.eval((msg) => {
    window.__poll = { done: false, t0: performance.now() };
    T.web.mcp(msg).then((r) => Object.assign(window.__poll, { done: true, r, ms: performance.now() - window.__poll.t0 }));
  }, poll(15, 50));
  await sleep(600);
  assert.equal(await app.eval(() => window.__poll.done), false, "wartet noch");
  // meanwhile other tools and commands respond immediately
  const t0 = Date.now();
  const p = await rpc("tools/call", { name: "get_project", arguments: {} }, 51);
  assert.ok(!p.result.isError);
  await app.eval(() => T.backend.getState());
  assert.ok(Date.now() - t0 < 3000, `${Date.now() - t0} ms`);
  // the UI shows that an agent is listening
  await app.waitFor(() => T.st.chat.peek().agentListening, { what: "Agent hört zu" });
  await app.waitFor(() => T.$(".chip.agent").textContent.includes("Agent hört zu"), { what: "Knopf: Agent hört zu" });
  // request from the UI chat → the waiting poll responds right away
  await app.eval(() => T.backend.chatSend("Mach die Kick tiefer"));
  await app.waitFor(() => window.__poll.done, { timeout: 5000, what: "chat_poll antwortet" });
  const { r, ms } = await app.eval(() => window.__poll);
  assert.equal(r.id, 50);
  const body = JSON.parse(r.result.content[0].text);
  assert.deepEqual(body.auftraege.map((a) => a.text), ["Mach die Kick tiefer"]);
  assert.ok(ms > 600 && ms < 10000, `${ms} ms`);
  // the agent's reply appears in the chat, the request is done
  const rep = await rpc("tools/call", { name: "chat_reply", arguments: { text: "Kick ist tiefer.", replyTo: body.auftraege[0].id } }, 52);
  assert.ok(!rep.result.isError, JSON.stringify(rep.result));
  await app.waitFor(() => T.st.chat.peek().messages.some((m) => m.from === "agent" && m.text === "Kick ist tiefer."), { what: "Antwort im Chat" });
  assert.equal(await app.eval(() => T.st.chat.peek().messages.find((m) => m.from === "user").status), "erledigt");
});

test("chat_poll without a task: empty list after the wait time; a task is picked up only once", async () => {
  const r = await app.eval(async (msg) => {
    const t0 = performance.now();
    const res = await T.web.mcp(msg);
    return { ms: performance.now() - t0, body: JSON.parse(res.result.content[0].text) };
  }, poll(1, 60));
  assert.deepEqual(r.body.auftraege, []);
  assert.ok(r.ms >= 950 && r.ms < 4000, `${r.ms} ms`);
  // two waiting polls, one request: exactly one gets it, the other waits until the end
  const both = await app.eval(async (a, b) => {
    const p = [T.web.mcp(a), T.web.mcp(b)];
    await T.sleep(300);
    await T.backend.chatSend("Nur einmal");
    return (await Promise.all(p)).map((x) => JSON.parse(x.result.content[0].text).auftraege.length);
  }, poll(2, 61), poll(2, 62));
  assert.deepEqual(both.sort(), [0, 1]);
});
