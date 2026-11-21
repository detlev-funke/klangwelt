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

// Runs first: every tool call counts as "agent working" for a minute.
test("agent window in the browser: prompt to copy with the page's address, roles and Playwright MCP", async () => {
  // the Agent button opens the window (on the desktop only with a running MCP server)
  assert.match(await app.eval(() => T.$(".chip.agent").textContent), /Kein Agent verbunden/);
  assert.match(await app.eval(() => T.$(".chip.agent").title), /Playwright MCP Bridge/);
  await app.clickOn(".chip.agent");
  await app.waitFor(() => T.$(".chat .connect-prompt"), { what: "Prompt im Agent-Fenster" });
  const [text, href, hint] = await app.eval(() => [T.$(".chat .connect-prompt").textContent, location.href, T.$(".chat .connect-hint").textContent]);
  assert.ok(text.includes(href), `Adresse ${href} eingesetzt`);
  assert.match(text, /window\.__klangweltWeb\.mcp/);
  assert.match(text, /chat_poll mit arguments \{timeoutSeconds: 20, client: "zuhoerer"\}/);
  assert.match(text, /result\.structuredContent/);
  // roles and the Playwright variant
  for (const w of ["Zuhörer", "Arbeiter", "\"zuhoeren\"", "\"arbeiten\"", "browser_evaluate", "Playwright MCP Bridge", "batch"]) assert.ok(text.includes(w), `${w} fehlt im Prompt`);
  assert.match(hint, /Playwright MCP Bridge/);
  assert.ok(await app.eval(() => !!T.$(".chat .connect .btn.blue")), "Kopieren-Knopf");
});

test("in the page: same tools as on the desktop, changes appear immediately", async () => {
  const init = await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } });
  assert.equal(init.result.serverInfo.name, "klangwelt");
  const tools = (await rpc("tools/list")).result.tools.map((t) => t.name);
  for (const t of ["get_project", "execute", "create_instrument_track", "write_chords", "write_song", "analyze_harmony", "chat_poll",
    "history", "checkpoint", "goto", "variant_new", "variant_switch", "variant_list", "combine"]) assert.ok(tools.includes(t), `Werkzeug ${t} fehlt`);
  const r = await rpc("tools/call", { name: "create_instrument_track", arguments: { kind: "synth" } });
  assert.ok(!r.result.isError, JSON.stringify(r.result));
  // "project-changed" from the core: the UI shows the track without having called anything itself
  await app.waitFor(() => T.st.snapshot.peek().project.tracks.some((t) => t.instrument?.kind === "synth"), { what: "Spur in der Oberfläche" });
  // tools that do not exist in the browser say so
  const dl = await rpc("tools/call", { name: "download_library", arguments: { id: "x" } });
  assert.equal(dl.result.isError, true);
  assert.match(dl.result.content[0].text, /platform/);
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
    return (await Promise.all(p)).map((x) => JSON.parse(x.result.content[0].text).auftraege);
  }, poll(2, 61), poll(2, 62));
  assert.deepEqual(both.map((a) => a.length).sort(), [0, 1]);
  // done, so it isn't offered again in the next tests
  await call("chat_reply", { text: "Erledigt.", replyTo: both.flat()[0].id });
});

/** Tool call in the page; result object (structuredContent), throws on tool errors. */
async function call(name, args = {}) {
  const r = await rpc("tools/call", { name, arguments: args }, Math.floor(Math.random() * 1e6));
  if (r.result?.isError) throw new Error(`${name}: ${r.result.content[0].text}`);
  return r.result.structuredContent ?? JSON.parse(r.result.content[0].text);
}
const callError = async (name, args = {}) => {
  const r = await rpc("tools/call", { name, arguments: args }, 7);
  assert.equal(r.result.isError, true, `${name} sollte fehlschlagen`);
  return r.result.content[0].text;
};
const chip = () => app.eval(() => T.$(".chip.agent").textContent);
const bubbleMeta = (id) => app.eval((i) => T.$(`.chat-list .msg[data-id="${i}"] .meta`)?.textContent ?? "", id);

test("chat_poll reserves tasks for its client; without a reply they are offered again", async () => {
  await app.eval(() => T.st.showPanel("agent"));
  const task = (await app.eval(() => T.backend.chatSend("Bass wärmer"))).messages.at(-1);
  const got = await call("chat_poll", { timeoutSeconds: 2, client: "test", leaseSeconds: 10 });
  assert.deepEqual(got.auftraege.map((a) => a.id), [task.id]);
  assert.equal(got.leaseSeconds, 10);
  // the bubble names who took it, the indicator shows the task in progress
  await app.waitFor((i) => T.$(`.chat-list .msg[data-id="${i}"] .meta`)?.textContent.includes("angenommen von test"), { what: "angenommen von test" }, task.id);
  await app.waitFor((i) => T.$(".chip.agent").textContent.includes(`arbeitet an #${i}`), { what: "Agent arbeitet an #n" }, task.id);
  // a second poller doesn't get it while it is reserved
  assert.deepEqual((await call("chat_poll", { timeoutSeconds: 1, client: "zuhoerer" })).auftraege, []);
  // lease over without a chat_reply → offered again (the worker asks again every few seconds)
  const again = await call("chat_poll", { timeoutSeconds: 20, client: "zuhoerer" });
  assert.deepEqual(again.auftraege.map((a) => [a.id, a.redelivered]), [[task.id, 1]]);
  assert.match(await bubbleMeta(task.id), /angenommen von zuhoerer/);
  // a reply confirms it: never offered again
  await call("chat_reply", { text: "Bin dran.", replyTo: task.id, done: false, client: "zuhoerer" });
  await sleep(300);
  assert.deepEqual((await call("chat_poll", { timeoutSeconds: 1, client: "test" })).auftraege, []);
  await call("chat_reply", { text: "Bass ist wärmer.", replyTo: task.id });
  await app.waitFor((i) => T.st.chat.peek().messages.find((m) => m.id === i)?.status === "erledigt", { what: "erledigt" }, task.id);
});

test("chat_peek reads new messages without taking them from the listener", async () => {
  const since = (await call("chat_peek", {})).lastId;
  const task = (await app.eval(() => T.backend.chatSend("Was geht ab?"))).messages.at(-1);
  const peek = await call("chat_peek", { sinceId: since });
  assert.deepEqual(peek.messages.map((m) => [m.id, m.status]), [[task.id, "new"]]);
  assert.equal(peek.lastId, task.id);
  // same with chat_poll {peek: true} – answers at once, the worker doesn't hold it
  const t0 = Date.now();
  assert.deepEqual((await call("chat_poll", { peek: true, sinceId: since })).messages.map((m) => m.id), [task.id]);
  assert.ok(Date.now() - t0 < 2000, `peek took ${Date.now() - t0} ms`);
  // still there for the listener
  assert.deepEqual((await call("chat_poll", { timeoutSeconds: 1, client: "zuhoerer" })).auftraege.map((a) => a.id), [task.id]);
  await call("chat_reply", { text: "Ruhig hier.", replyTo: task.id });
});

test("stop at a task: every tool response carries cancelRequested until the agent finishes it", async () => {
  await app.eval(() => T.st.showPanel("agent"));
  const task = (await app.eval(() => T.backend.chatSend("Bau ein Jazz-Stück"))).messages.at(-1);
  await call("chat_poll", { timeoutSeconds: 2, client: "zuhoerer" });
  await call("chat_reply", { text: "Bin dran.", replyTo: task.id, done: false });
  // the person clicks "Stopp" at the task
  await app.waitFor((i) => T.$(`.chat-list .msg[data-id="${i}"] .stop-task`), { what: "Stopp-Knopf" }, task.id);
  await app.eval((i) => T.$(`.chat-list .msg[data-id="${i}"] .stop-task`).click(), task.id);
  await app.waitFor((i) => T.$(`.chat-list .msg[data-id="${i}"] .meta`)?.textContent.includes("Stopp angefordert"), { what: "Stopp angefordert" }, task.id);
  const p = await call("get_project", { fields: ["name"] });
  assert.deepEqual(p.cancelRequested, [task.id]);
  assert.match(await callError("get_project", { trackIds: [987654] }), /cancelRequested/);
  // the stop message goes to the listener, with the stopped task
  const stop = (await call("chat_poll", { timeoutSeconds: 2, client: "zuhoerer" })).auftraege;
  assert.equal(stop.length, 1);
  assert.deepEqual(stop[0].cancels, [task.id]);
  assert.match(stop[0].text, /Stopp #/);
  await call("chat_reply", { text: "Gestoppt.", replyTo: task.id });
  await call("chat_reply", { text: "Erledigt.", replyTo: stop[0].id });
  assert.equal((await call("get_project", { fields: ["name"] })).cancelRequested, undefined);
  // a typed "Stopp" cancels a task nobody picked up yet
  const waiting = (await app.eval(() => T.backend.chatSend("Danach Tempo 90"))).messages.at(-1);
  const typed = (await app.eval(() => T.backend.chatSend("Stopp"))).messages.at(-1);
  assert.deepEqual(typed.cancels, [waiting.id]);
  await app.waitFor((i) => T.st.chat.peek().messages.find((m) => m.id === i)?.status === "abgebrochen", { what: "abgebrochen" }, waiting.id);
  assert.match(await bubbleMeta(waiting.id), /abgebrochen/);
  const left = (await call("chat_poll", { timeoutSeconds: 1, client: "zuhoerer" })).auftraege.map((a) => a.id);
  assert.deepEqual(left, [typed.id], "the cancelled task isn't offered");
  await call("chat_reply", { text: "Nichts mehr offen.", replyTo: typed.id });
});

test("agent indicator: listening and working at the same time", async () => {
  const task = (await app.eval(() => T.backend.chatSend("Hi-Hat leiser"))).messages.at(-1);
  await call("chat_poll", { timeoutSeconds: 2, client: "zuhoerer" });
  await call("chat_reply", { text: "Bin dran.", replyTo: task.id, done: false });
  // a listener waits again while the task is in progress
  await app.eval((msg) => void T.web.mcp(msg), poll(4, 90));
  await app.waitFor((i) => T.$(".chip.agent").textContent === `Agent hört zu · arbeitet an #${i}`, { what: "hört zu · arbeitet an #n" }, task.id);
  assert.match(await app.eval(() => T.$(".chat-status .status-text").textContent), /Arbeitet an #/);
  await call("chat_reply", { text: "Leiser.", replyTo: task.id });
  await app.waitFor(() => T.$(".chip.agent").textContent === "Agent hört zu", { what: "Agent hört zu", timeout: 4000 });
  await sleep(4500); // the held poll ends
  // no poll, but a tool call within the last minute: "Agent arbeitet" (not "Kein Agent verbunden")
  await app.waitFor(() => T.$(".chip.agent").textContent === "Agent arbeitet" || T.$(".chip.agent").textContent === "Agent hört zu", { what: "Agent arbeitet" });
  assert.notEqual(await chip(), "Kein Agent verbunden");
});

test("write tools: revision and ifRevision, placeholders, compact notes, filters, IDs in the response", async () => {
  const rev = (await call("get_project", { fields: ["tempoBpm"] })).revision;
  const r = await call("execute", { ifRevision: rev, label: "Bass", commands: [
    { type: "addTrack", track: { ref: "$bass", name: "Bass", kind: "instrument" } },
    { type: "addClip", trackId: "$bass", clip: { ref: "$riff", startBeats: 0, source: { type: "notes", lengthBeats: 4, notes: "A2 1 · C3 .5 v90 · E3 .5 · G3 2" } } },
  ] });
  const bass = r.ids.$bass;
  assert.deepEqual(r.created.tracks, [bass]);
  assert.deepEqual(r.created.clips, [{ trackId: bass, clipId: r.ids.$riff }]);
  assert.ok(r.revision > rev);
  assert.match(r.change, /Agent · Bass/);
  // the UI shows the track with the notes at once
  await app.waitFor((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id)?.clips[0]?.source.notes?.length === 4, { what: "Bass-Spur mit 4 Noten" }, bass);
  const notes = await app.eval((id) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id).clips[0].source.notes.map((n) => [n.pitch, n.startBeats, n.velocity]), bass);
  assert.deepEqual(notes, [[45, 0, 100], [48, 1, 90], [52, 1.5, 90], [55, 2, 90]]);
  // the old revision is stale now: nothing is changed
  const before = await app.eval(() => T.st.snapshot.peek().project.tracks.length);
  assert.match(await callError("execute", { ifRevision: rev, commands: [{ type: "removeTrack", trackId: bass }] }), /changed in the meantime/);
  assert.equal(await app.eval(() => T.st.snapshot.peek().project.tracks.length), before);
  // filters: only this track, only some fields
  const p = await call("get_project", { trackIds: [bass], fields: ["tempoBpm", "tracks.name"] });
  assert.deepEqual(p.tracks, [{ id: bass, name: "Bass" }]);
  assert.deepEqual(Object.keys(p).sort(), ["revision", "tempoBpm", "tracks"]);
});

test("batch: several tools as one undo step, reverted at the first error", async () => {
  const before = await app.eval(() => T.st.snapshot.peek().project.tracks.length);
  const r = await call("batch", { label: "Schlagzeug", calls: [
    { tool: "create_instrument_track", args: { kind: "drums" } },
    { tool: "execute", args: { commands: [{ type: "setTrack", trackId: "$0.trackId", patch: { gainDb: -5, name: "Drums" } }] } },
    { tool: "get_project", args: { trackIds: ["$0.trackId"], fields: ["tracks.gainDb"] } },
  ] });
  assert.equal(r.undoLabel, "Agent · Schlagzeug");
  assert.equal(r.results[2].tracks[0].gainDb, -5);
  assert.deepEqual(r.created.tracks, [r.results[0].trackId]);
  await app.waitFor((n) => T.st.snapshot.peek().project.tracks.length === n + 1, { what: "neue Spur" }, before);
  assert.equal(await app.eval(() => T.st.snapshot.peek().undoLabel), "Agent · Schlagzeug");
  // one Ctrl+Z reverts all of it
  await call("undo");
  await app.waitFor((n) => T.st.snapshot.peek().project.tracks.length === n, { what: "rückgängig" }, before);
  // error in the second call: the first is reverted too
  const e = await callError("batch", { calls: [
    { tool: "create_instrument_track", args: { kind: "synth" } },
    { tool: "execute", args: { commands: [{ type: "setTrack", trackId: 99999, patch: { mute: true } }] } },
  ] });
  assert.match(e, /Call 2 \(execute\)/);
  await sleep(200);
  assert.equal(await app.eval(() => T.st.snapshot.peek().project.tracks.length), before);
});

test("change history: checkpoint, variant, combine with a conflict to decide, goto – the History window follows", async () => {
  // one note per bar (8 bars); `changed` replaces some of them
  const bars = (changed = {}) => [0, 1, 2, 3, 4, 5, 6, 7].map((i) => `${changed[i] ?? "C4"} 1 @${i * 4}`).join(" · ");
  const r = await call("execute", { label: "Lead", commands: [{ type: "addTrack", track: { ref: "$lead", name: "Lead", kind: "instrument",
    clips: [{ ref: "$a", startBeats: 0, source: { type: "notes", lengthBeats: 32, notes: bars() } }] } }] });
  const [lead, clip] = [r.ids.$lead, r.ids.$a];
  const setNotes = (changed) => call("execute", { commands: [{ type: "setClipNotes", trackId: lead, clipId: clip, notes: bars(changed) }] });
  const pitches = (want) => app.waitFor((id, w) => T.st.snapshot.peek().project.tracks.find((t) => t.id === id)?.clips[0].source.notes.map((n) => n.pitch).join() === w,
    { what: `Lead-Noten ${want}` }, lead, want);
  const activeVariant = (name) => app.waitFor((n) => T.$(".hist-var.on")?.childNodes[1].textContent === n, { what: `Variante „${name}“ aktiv` }, name);

  await app.eval(() => T.st.showPanel("historie"));
  const cp = await call("checkpoint", { name: "Vor dem Refrain" });
  await app.waitFor(() => T.$(".hist-row.current .hist-flag")?.textContent.trim() === "Vor dem Refrain", { what: "Checkpoint des Agenten in der Historie" });
  await setNotes({ 4: "D4", 5: "D4" });
  const main = (await call("history")).variant;
  const v = await call("variant_new", { name: "Refrain", checkpoint: cp.checkpoint.id });
  assert.equal(v.step, cp.step);
  await activeVariant("Refrain");
  await pitches("60,60,60,60,60,60,60,60");
  await setNotes({ 5: "G4", 6: "G4", 7: "G4" });

  // the conflict in bar 6 comes back open; "*" decides it for the main version
  const c = await call("combine", { others: [main.id] });
  assert.deepEqual([c.combined, c.open], [false, [`track ${lead} bar 6`]]);
  const done = await call("combine", { others: [main.name], choices: { "*": main.id } });
  assert.deepEqual([done.combined, done.variant.name], [true, `Refrain + ${main.name}`]);
  await activeVariant(done.variant.name);
  await pitches("60,60,60,60,62,62,67,67");

  await call("goto", { checkpoint: "Vor dem Refrain" });
  await pitches("60,60,60,60,60,60,60,60");
  await call("goto", { step: done.step });
  await pitches("60,60,60,60,62,62,67,67");
  assert.match(await callError("batch", { calls: [{ tool: "combine", args: { others: [main.id] } }] }), /not allowed/);
});
