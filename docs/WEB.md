# Klangwelt in the browser

The browser version is an **experiment**: Klangwelt as static files that any
simple HTTP server can serve – without a Rust server process, without Tauri.
The computation still happens in Rust: the same core as in the desktop app is
compiled to WebAssembly (target `wasm32-unknown-unknown`) and runs in the
browser.

## Building and serving

Prerequisites: Rust with the target `wasm32-unknown-unknown`
(`rustup target add wasm32-unknown-unknown`), Node, and clang from LLVM for
libopus (Windows: `winget install LLVM.LLVM`; the build finds clang on the
`PATH` or under `C:\Program Files\LLVM\bin`, otherwise set
`CC_wasm32_unknown_unknown`). Nothing else: no Emscripten, no wasm-bindgen,
no wasm-pack, no bundler.

```bash
cd ui
npm ci
npm run build:web     # → ui/dist-web
npm run serve:web     # → http://localhost:1420/
```

* `ui/dist-web` is a ready-to-serve folder: the UI (the same as in the
  desktop app), `klang_web.wasm` (core with libopus, ~4.4 MB),
  `klang_worklet.wasm` (mixer, ~0.1 MB) and `sw.js` (service worker with file
  list and version).
* Any static server works. `.wasm` should be served as `application/wasm`
  (otherwise the core loads somewhat more slowly, in one piece instead of
  streamed).
* **No COOP/COEP needed** – SharedArrayBuffer is not required.
* The browser needs a "secure context": `localhost` or `https`. Over
  `http://<IP address>` on the network, AudioWorklet, storage (OPFS), service
  worker and microphone don't work.
* Tested with Edge (also headless in the tests). Chrome should work the same;
  Firefox and Safari have not been tried.
* `?attrappe` appended to the address loads the old mock (`mock.ts`, without
  audio) instead of the core – for UI work. `npm run serve` (without `:web`)
  serves `ui/dist` for that.
* `?ohne-sw` disables the service worker (during development).

## Publishing on GitHub Pages

The browser version runs at **https://devfunke.github.io/klangwelt/**.
All paths in the app are relative, so it also runs in a subfolder.

* **Setup (once):** Create the repo on GitHub and push, then under
  *Settings → Pages → Build and deployment* choose "GitHub Actions" as the
  *Source*.
* **Build and publish:** `.github/workflows/pages.yml` runs on every push to
  `main` (or manually under *Actions*): Rust with the target
  `wasm32-unknown-unknown`, clang and llvm-ar for libopus, Node, then
  `npm ci && npm run build:web`; `ui/dist-web` goes out as the Pages artifact.
  After a minute or two the new version is online; the service worker fetches
  it on the next visit (version = checksum of the files).
* **Custom domain** (later, e.g. klangwelt.io): enter it under *Settings →
  Pages → Custom domain* and, at the domain provider, set a CNAME pointing to
  `devfunke.github.io`; GitHub issues the HTTPS certificate itself.
* **Good to know:** The browser's storage (OPFS) is per origin – under
  `devfunke.github.io` all project pages of this account share one storage.
  Klangwelt puts everything into its own folder `Klangwelt`.

## Structure: one core, two shells

So that desktop and browser stay the same in the long run, the logic exists
only once (`crates/klang-app`). The desktop shell (`app/`) lives in the
klangwelt-desktop repo, which includes this repo as a submodule. Desktop and
browser are thin shells that fill in the same slots:

| Slot | Desktop (Tauri) | Browser |
|---|---|---|
| UI commands | Tauri IPC → `klang_app::commands::call` (`app/src/lib.rs`) | Web Worker → `kw_invoke` → `klang_app::commands::call` (`crates/klang-web`) |
| Method → command mapping | `ui/src/bridge.ts`, `coreBackend` | the same |
| Events to the UI | Tauri events | `postMessage` (worker → page) |
| Platform for the core (`Host`) | Tauri events, system folders | events to the script, `/Klangwelt/…` |
| Audio engine (`klang_engine::Engine`) | cpal thread, renderer in the audio callback (`device.rs`) | messages to the AudioWorklet (`web.rs`), renderer there as WebAssembly (`crates/klang-worklet`) |
| Status every 33 ms | status thread | timer in the worker |
| Files (`klang_app::vfs`) | file system | in-memory file tree, persisted in OPFS |
| Clock (`klang_app::clock`) | `std::time` | `Date.now()`, `performance.now()` |
| Agent (MCP) | HTTP server on 127.0.0.1 (`app/src/mcp.rs`) | `window.__klangweltWeb.mcp(…)`, stdio bridge via the DevTools protocol |
| Agent tools | `klang_app::agent` | the same |

Rules to keep it that way:

* **New UI command:** register it in `crates/klang-app/src/commands.rs`
  (name, arguments in camelCase) and map it in `bridge.ts` – then it exists
  on both platforms. A test in the core checks that every registered command
  is also handled.
* **New agent tool:** in `crates/klang-app/src/agent.rs`.
* Only what a platform really can't do belongs in a shell:
  desktop `app/src/lib.rs`, `library.rs`, `libraries.rs`, `mcp.rs`; browser
  `crates/klang-web`, `ui/src/web/`.
* In the core, access files only via `klang_app::vfs` and time only via
  `klang_app::clock` (`std::fs`, `std::time::Instant`/`SystemTime` and threads
  don't exist on `wasm32-unknown-unknown` – they fail or abort).

### Runtime in the browser

```
Page (ui/src/web/host.ts)               Web Worker (worker.ts)              AudioWorklet (worklet.ts)
  UI, bridge.ts            ──command──▶ klang_web.wasm (core)  ──message──▶ klang_worklet.wasm (mixer)
  microphone, downloads    ◀───event─── OPFS backup            ◀──status─── microphone samples
  drag & drop, sw.js                                          MessageChannel
```

* Sound buffers go to the worklet once per buffer (in chunks of 128 k
  samples, so the worklet never spends long copying); after that, messages
  only refer to their ID (`klang_engine::wire`). The engine frees buffers that
  are no longer needed.
* Files: on startup the worker loads everything from OPFS into the core's
  file tree; after every command it writes the changed files back. Exports
  stay in memory only (they are downloaded).
* The service worker keeps all files in the cache – the app also starts
  without a server or without a network.

### libopus in the browser

The Opus export uses the same libopus 1.5 on both platforms
(`crates/klang-opus/opus`), the same Rust part (`crates/klang-opus`, Ogg)
and the same place in the core (`klang_app::export`). For the browser,
`crates/klang-opus/build.rs` compiles the C sources with clang for
`wasm32-unknown-unknown`; Rust links them into `klang_web.wasm` (around
260 KB more). There is no C standard library there, and hardly any of one is
needed:

* Header files: the few that libopus includes live, slimmed down, under
  `crates/klang-opus/wasm/include` (the rest comes from clang itself).
* Math (`sin`, `cos`, `exp`, `log`, `log10`, `pow`) and `memcpy` & co. are
  provided by Rust's `compiler_builtins`; square root, absolute value and
  rounding are WebAssembly instructions.
* Memory: `malloc`/`free` via Rust's allocator (`src/wasm_libc.rs`).
* Scratch buffers as C99 variable-length arrays instead of `alloca`.

`build:web` checks that `klang_web.wasm` and `klang_worklet.wasm` import
only what the worker and worklet provide – a missing C function would show up
at build time, not only in the browser. Emscripten is not needed.

## What works

* Editing projects as on the desktop: tracks, clips, notes, expression,
  sound window, rhythm, song, harmony (circle of fifths, suggestions,
  timeline), fugue and counterpoint, analysis of recordings – all with the
  same logic and the same undo.
* Playback, metronome, loop, preview – mixed by the same engine
  (reverb, limiter) in the AudioWorklet.
* Recording via the microphone (`getUserMedia`, after permission is granted
  in the browser).
* Saving and opening projects; they live in the browser's storage (OPFS)
  and survive reloads and browser restarts.
* Score library: a prebuilt package on Zenodo, read by HTTP range (only
  what a search or an opened score reads is cached – no "download the whole
  index"), plus own MusicXML/MXL files (drag & drop or "Add", indexed in the
  core, kept in OPFS). Text search, melody search and the tree cover both;
  hits show source and license. See [LIBRARY.md](LIBRARY.md).
* Downloading and uploading the project file ("File" menu), `.klangwelt`
  also via drag & drop onto the window.
* Export as WAV, FLAC, Opus, stems and MIDI – as a download. Opus is encoded
  by the same libopus as on the desktop (see "libopus in the browser").
* Unsaved changes: the browser asks for confirmation when leaving the page.
* Offline after the first load (service worker).
* Agent via the core's tools (see below).

## What's missing – and why

| What | Why | Replacement / next step |
|---|---|---|
| MCP server over HTTP | A web page can't open a port. | The same tools in the page and via a stdio bridge (see below). |
| Downloading library packs and managing several libraries (the desktop's catalog) | The desktop downloads sources and indexes them on disk; some of them have non-commercial licenses (KernScores, DCML) and must not be hosted. | The browser reads the prebuilt package by range and adds own files ([LIBRARY.md](LIBRARY.md)). |
| "Show in folder" | A web page can't see folders. | "Download again". |
| Multiple cores for rendering | Threads would need SharedArrayBuffer (COOP/COEP) and a custom-built standard library (nightly). | Note clips are played live, nothing waits for them; only stretching audio clips and the export run on one core. |
| Choosing a project folder | Projects live in the browser's storage, separate per address. Clearing the website's data deletes them. | Download/upload; later the File System Access API. |
| Double-clicking a `.klangwelt` file, command line | Not available for web pages. | Drag & drop, upload. |

Limits worth knowing:

* **Memory:** Audio clips (recordings, imports) are held in memory twice –
  in the core (worker) and in the mixer (worklet); without SharedArrayBuffer the two
  can't share memory. Note clips are only notes – they cost next to nothing.
* **Latency:** AudioWorklet processes in blocks of 128 frames; on top of that
  comes the browser's output latency. That's enough for playback and preview,
  but hardly for monitoring while recording.
* **Playback** requires a gesture (click, key press) – the browsers' autoplay
  rule. The page starts audio on the first click.
* **Internal error:** If the core aborts (in the browser, a Rust `panic`
  aborts the WebAssembly instance), the message appears in the console;
  then reload the page. Saved projects are preserved.

## Agent in the browser

The desktop's MCP server is an HTTP server – a web page can't open one. The
tools themselves (`klang_app::agent`), however, belong to the core and run in
the browser just the same. There are two ways to reach them:

1. **In the page:** `window.__klangweltWeb.mcp(message)` takes an MCP message
   (JSON-RPC) and returns the response – e.g.
   `{ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "get_project", arguments: {} } }`.
   Any agent that is allowed to run JavaScript in the page can do this (a
   browser extension such as Claude in Chrome or the Playwright MCP Bridge – see
   [Variant: Playwright MCP](#variant-playwright-mcp-vivaldi-brave-edge-chrome) – or the DevTools protocol).
2. **stdio bridge** (`ui/scripts/web-mcp.mjs`, `npm run mcp:web`): an
   ordinary MCP server for clients such as Claude Code that forwards each
   message to the open Klangwelt tab via the DevTools protocol.

   ```bash
   msedge --remote-debugging-port=9222 http://localhost:1420/
   ```

   MCP configuration:

   ```json
   { "mcpServers": { "klangwelt-web": { "command": "node", "args": ["<path>/ui/scripts/web-mcp.mjs", "--port=9222"] } } }
   ```

   To the agent this looks the same as on the desktop: the same tools, the
   same responses. Tools that don't exist in the browser (downloading and
   managing the library) report that.

Changes made by the agent appear immediately in the UI (as on the desktop via
the `project-changed` event).

**Agent window:** The agent button opens the chat in the browser as well. As
long as no agent is listening, it shows a prompt to copy
(`ui/src/web/agent-prompt.ts`) containing the page's address – intended for
Claude Desktop with the "Claude in Chrome" extension (whose JavaScript tool
runs in the page and supports `await`), for Playwright MCP with the
"Playwright MCP Bridge" extension, or another agent with a browser tool.
The prompt explains the route via `window.__klangweltWeb.mcp`, the Playwright
variant and the roles: a listener (small model) that only calls `chat_poll` and
confirms each task at once, the main session that starts a worker per task, the
workers (at most one per track, project-wide changes one after another). The tools
are the same as on the desktop – reserving tasks, `chat_peek`, `cancelRequested`,
`revision`/`ifRevision`, placeholders, `batch`, compact notes (see
[ARCHITECTURE.md](ARCHITECTURE.md#agent-via-mcp)).

**`chat_poll` waits**, as on the desktop, until a task arrives or
`timeoutSeconds` expires. In the browser the core has only one thread and
must not block; so the web worker keeps the response open and queries the
core again as soon as the UI sends a task (`chat_send`, `chat_stop`). Other commands and
tools keep running normally in the meantime; every 5 s the worker asks the core again –
that refreshes "Agent is listening" and hands out a task whose lease ran out (no
`chat_reply` in time). `chat_poll {peek: true}` is not held. A service worker would be the wrong place for this: it
can't see the core's state (that lives in the web worker), and the browser
terminates it when idle. The prompt uses a 20 s wait because the time limit
of the JavaScript tool is unknown. If a tool aborts the call before that, the
page doesn't notice – a task that arrives at exactly that moment is reserved for
nobody who listens; since it is only reserved, not used up, it is offered again
after its lease (60 s) and nothing is lost.

### Variant: Playwright MCP (Vivaldi, Brave, Edge, Chrome)

Without Claude in Chrome and without a debug port, an agent reaches the Klangwelt tab in
the person's own browser through Microsoft's Playwright MCP server in extension mode and
the browser extension **"Playwright MCP Bridge"** ([Chrome Web Store](https://chromewebstore.google.com/detail/mmlmfjhmonkocbjadbfplnigmagldckm), ID `mmlmfjhmonkocbjadbfplnigmagldckm`; works in Chromium browsers such as Vivaldi, Brave and Edge too). Tested with
Vivaldi 8.3 and `@playwright/mcp` 0.0.85. The `--remote-debugging-port` route no longer
works for the default profile since Chrome 136 (and so presumably in Vivaldi, too).

```
agent ─▶ Playwright MCP (stdio) ─▶ relay ws://127.0.0.1:<port>/extension/<uuid> ─▶ extension (chrome.debugger) ─▶ Klangwelt tab: window.__klangweltWeb.mcp()
```

1. If the agent has Playwright MCP tools (`browser_tabs`, `browser_evaluate`), it uses them.
   Otherwise it starts the server itself and talks to it over stdio (`initialize`,
   `notifications/initialized`, `tools/call`):

   ```bash
   npx @playwright/mcp@latest --extension --executable-path "<path to the browser exe>"
   ```

   Without `--executable-path` Playwright looks for Google Chrome.
2. The first tool call opens the extension's connect page as a new tab in the running
   browser (no second window, no new profile). The person allows the Klangwelt tab there –
   once per start of the server. With the token from the extension
   (`PLAYWRIGHT_MCP_EXTENSION_TOKEN`) this step should be skipped (according to the
   Playwright source; not tested).
3. `browser_tabs {action: "list"}` lists the tabs, e.g. `- 0: (current) [Klangwelt](http://127.0.0.1:1477/)`;
   select the Klangwelt tab with `browser_tabs {action: "select", index}` if needed.
4. Every message is one `browser_evaluate` with
   `function: async () => JSON.stringify(await window.__klangweltWeb.mcp(<message>))`.
   Playwright wraps the result in Markdown (`### Result`, then `### Ran Playwright code`);
   a script that parses it reliably returns the JSON base64-encoded between fixed markers
   (e.g. `KW<…>KW`). `page.evaluate` has no time limit of its own, so `chat_poll` with 20 s
   works.
5. Several Klangwelt messages fit into one `browser_evaluate` – an empty
   `browser_evaluate` costs about 0.57 s (measured: a tool call through such a bridge
   ≈ 630 ms in total, the Klangwelt part is small). Fewer calls matter more: `batch`,
   placeholders in `execute`, compact notes and `get_project` with filters save model
   rounds, and every round takes seconds.
6. Only one process listens with `chat_poll` (client `"zuhoerer"`). Tasks are reserved, not
   used up – a second poller (a test, say) no longer snatches them for good, but they stay
   with it until its lease runs out; workers look with `chat_peek`.

If Playwright MCP is registered as an MCP server (with `--extension`, `--executable-path`
and the token), the agent calls `browser_evaluate` directly and needs no bridge of its own.

## Tests

```bash
cd ui
npm run build:web
npm run test:web
```

`tests/web/` – node:test and the DevTools protocol, no Playwright (like
`tests/e2e/` for the desktop). Each test file starts the static server and
a headless Edge with its own profile (OPFS, service worker) on free ports.
`KLANGWELT_EDGE` selects a different browser.

Covered: loading with the real core, "Load demo", playback with level (in
the status and independently with an analyser at the worklet's output),
editing and undo (keyboard), saving and reloading, downloading and uploading
the project file, export as WAV and Opus download (the browser decodes the
Opus file itself – a cross-check of the libopus in the core),
analysis/harmony/fugue/counterpoint, recording with the browser's fake
microphone, starting without a server via the service worker, agent in the
page and via the stdio bridge, agent window with prompt (roles, Playwright variant), waiting `chat_poll`
(a task from the chat wakes it, timeout, each task delivered only once), reserving and
offering again after the lease, `chat_peek`, stop button and `cancelRequested`, the agent
indicator (listening and working), `revision`/`ifRevision`, placeholders, compact notes,
`get_project` filters and `batch` as one undo step.

## Performance

Measurements on the development machine (8 logical cores), Windows, Edge.

### Desktop: unchanged

On the desktop, the restructuring only changes the path a call takes: Tauri
passes it on to the command table, which returns the response as ready-made
JSON (as the generated commands did before; events likewise go out already
serialized via `emit_str`). The engine – the renderer in the audio callback –
is the same code.

* Overhead of the command table itself (Rust, `aufwand_der_befehlstabelle`):
  under 0.1 µs per call – an IPC round trip of the desktop app takes around
  1200 µs.
* Desktop app measured before/after in alternation (`tests/perf/desktop.mjs`,
  median of 5): small commands over IPC equal or faster; demo, song and
  export within the noise (the values of the same exe vary by up to 30%
  between two rounds; in the calmer round, before and after were within
  ±3%).

### Browser compared to desktop

| Operation | Desktop (8 threads) | Browser (one thread) |
|---|---|---|
| 100× `get_state` | 110–160 ms (IPC) | 12–20 ms (`postMessage`) |
| Load demo | 0.4–0.6 s | 0.6 s |
| Generate song (trance, 3½ min, 26 clips) | under 10 ms (core) | 55–125 ms |
| Tempo change while this song plays | – | 12–96 ms |

Small commands are faster in the browser (no IPC between processes).
Generating and changing the tempo are fast on both, because note clips are
not rendered but played live (before: 1.5–1.9 s on the desktop, 6–15 s in
the browser, and the browser's display stood still meanwhile). The single
thread only matters for stretching audio clips and for the export.

### How many tracks live at once?

Note clips are synthesized while playing: voices in the mixer, one sound
shaping chain per track. Measured with the trance song stacked several times
(5 tracks each, all notes at the same moments – the hardest case), blocks of
128 frames at 48 kHz (budget 2.67 ms):

| tracks | native: average | native: 99.9 % of the blocks under | browser (AudioWorklet): playback rate |
|---|---|---|---|
| 5 | 4 % | 8 % | 0.998 |
| 10 | 8 % | 15 % | 0.997 |
| 20 | 15 % | 30 % | 0.997 |
| 40 | – | – | 0.998 |
| 60 | – | – | 0.998 |
| 80 | – | – | 0.994 |

Per track that is about 0.8 % of the budget on average. In the browser
60 such tracks keep time (playback rate 1.0 = the beat is held); at 80 the
worklet starts to fall behind. Before that happens on a weaker machine, the
core pre-renders the most expensive clips around the playback position in
the background and plays them from buffers (see ARCHITECTURE.md, "Note clips
are played live"); the worklet reports its load for that. The mixer has room for 128 tracks and 512
voices at once. To measure: `cargo test --release -p klang-app --
--ignored --nocapture live_spuren` and `node tests/perf/live.mjs`.

Audio clips arrive as finished buffers and only need adding up: 512
simultaneous clips take 3.5 % (native) or 5.3 % (WebAssembly) of the budget,
1024 take 7 % or 10 % (`cargo test --release -p klang-engine -- --ignored
--nocapture stimmen`, `node tests/perf/voices.mjs`).

### SharedArrayBuffer and threads – worth it?

Not for playback (see above), and no longer for editing: note clips are not
rendered. It would help in two other places:

* **Export:** the desktop spreads the tracks over its cores; the browser
  synthesizes them one after another.
* **Memory:** Audio clips are held in memory twice (worker and worklet).
  With shared memory they would exist only once, and worker and worklet
  could communicate via a lock-free ring buffer as on the desktop.

Cost: COOP/COEP must be set (the service worker can take care of that, so a
simple server is still sufficient); for shared memory and threads, Rust needs
either nightly with a rebuilt standard library or the target
`wasm32-wasip1-threads` with a small thread layer in JavaScript.
Recommendation: only once exports in the browser take too long or memory
gets tight.
