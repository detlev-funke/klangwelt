// Agent in der Browser-Version: Ohne Port gibt es keinen MCP-Server – dieselben
// Werkzeuge wie auf dem Desktop (klang_app::agent) stehen aber in der Seite
// bereit (`window.__klangweltWeb.mcp`) und über die stdio-Brücke
// (ui/scripts/web-mcp.mjs), die per DevTools-Protokoll mit dem Tab spricht.
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

test("in der Seite: dieselben Werkzeuge wie auf dem Desktop, Änderungen erscheinen sofort", async () => {
  const init = await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } });
  assert.equal(init.result.serverInfo.name, "klangwelt");
  const tools = (await rpc("tools/list")).result.tools.map((t) => t.name);
  for (const t of ["get_project", "execute", "create_instrument_track", "write_chords", "write_song", "analyze_harmony", "chat_poll"]) assert.ok(tools.includes(t), `Werkzeug ${t} fehlt`);
  const r = await rpc("tools/call", { name: "create_instrument_track", arguments: { kind: "synth" } });
  assert.ok(!r.result.isError, JSON.stringify(r.result));
  // „project-changed“ vom Kern: die Oberfläche zeigt die Spur, ohne dass sie selbst etwas aufgerufen hat
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.some((t) => t.instrument?.kind === "synth"), { what: "Spur in der Oberfläche" });
  // Werkzeuge, die es im Browser nicht gibt, sagen das
  const dl = await rpc("tools/call", { name: "download_library", arguments: { id: "x" } });
  assert.equal(dl.result.isError, true);
  assert.match(dl.result.content[0].text, /Plattform/);
});

test("Agent-Fenster im Browser: Prompt zum Kopieren mit der Adresse der Seite", async () => {
  // der Agent-Knopf öffnet das Fenster (auf dem Desktop nur mit laufendem MCP-Server)
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

test("stdio-Brücke: ein MCP-Client spricht über das DevTools-Protokoll mit dem Tab", async () => {
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
    // Benachrichtigung: keine Antwort; die nächste Antwort gehört zur nächsten Frage
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

test("chat_poll wartet (im Worker) auf einen Auftrag – Kern und Oberfläche bleiben bedienbar", async () => {
  await app.eval((msg) => {
    window.__poll = { done: false, t0: performance.now() };
    T.web.mcp(msg).then((r) => Object.assign(window.__poll, { done: true, r, ms: performance.now() - window.__poll.t0 }));
  }, poll(15, 50));
  await sleep(600);
  assert.equal(await app.eval(() => window.__poll.done), false, "wartet noch");
  // derweil antworten andere Werkzeuge und Befehle sofort
  const t0 = Date.now();
  const p = await rpc("tools/call", { name: "get_project", arguments: {} }, 51);
  assert.ok(!p.result.isError);
  await app.eval(() => T.backend.getState());
  assert.ok(Date.now() - t0 < 3000, `${Date.now() - t0} ms`);
  // die Oberfläche zeigt, dass ein Agent zuhört
  await app.waitFor(() => T.st.chat.peek().agentListening, { what: "Agent hört zu" });
  await app.waitFor(() => T.$(".chip.agent").textContent.includes("Agent hört zu"), { what: "Knopf: Agent hört zu" });
  // Auftrag aus dem Chat der Oberfläche → der wartende Poll antwortet gleich
  await app.eval(() => T.backend.chatSend("Mach die Kick tiefer"));
  await app.waitFor(() => window.__poll.done, { timeout: 5000, what: "chat_poll antwortet" });
  const { r, ms } = await app.eval(() => window.__poll);
  assert.equal(r.id, 50);
  const body = JSON.parse(r.result.content[0].text);
  assert.deepEqual(body.auftraege.map((a) => a.text), ["Mach die Kick tiefer"]);
  assert.ok(ms > 600 && ms < 10000, `${ms} ms`);
  // die Antwort des Agenten erscheint im Chat, der Auftrag ist erledigt
  const rep = await rpc("tools/call", { name: "chat_reply", arguments: { text: "Kick ist tiefer.", replyTo: body.auftraege[0].id } }, 52);
  assert.ok(!rep.result.isError, JSON.stringify(rep.result));
  await app.waitFor(() => T.st.chat.peek().messages.some((m) => m.from === "agent" && m.text === "Kick ist tiefer."), { what: "Antwort im Chat" });
  assert.equal(await app.eval(() => T.st.chat.peek().messages.find((m) => m.from === "user").status), "erledigt");
});

test("chat_poll ohne Auftrag: leere Liste nach der Wartezeit; ein Auftrag wird nur einmal abgeholt", async () => {
  const r = await app.eval(async (msg) => {
    const t0 = performance.now();
    const res = await T.web.mcp(msg);
    return { ms: performance.now() - t0, body: JSON.parse(res.result.content[0].text) };
  }, poll(1, 60));
  assert.deepEqual(r.body.auftraege, []);
  assert.ok(r.ms >= 950 && r.ms < 4000, `${r.ms} ms`);
  // zwei wartende Polls, ein Auftrag: genau einer bekommt ihn, der andere wartet bis zum Ende
  const both = await app.eval(async (a, b) => {
    const p = [T.web.mcp(a), T.web.mcp(b)];
    await T.sleep(300);
    await T.backend.chatSend("Nur einmal");
    return (await Promise.all(p)).map((x) => JSON.parse(x.result.content[0].text).auftraege.length);
  }, poll(2, 61), poll(2, 62));
  assert.deepEqual(both.sort(), [0, 1]);
});
