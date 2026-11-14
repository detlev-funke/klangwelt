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
| Multiple cores for rendering | Threads would need SharedArrayBuffer (COOP/COEP) and a custom-built standard library (nightly). | Rendering happens sequentially – long songs take somewhat longer. |
| Choosing a project folder | Projects live in the browser's storage, separate per address. Clearing the website's data deletes them. | Download/upload; later the File System Access API. |
| Double-clicking a `.klangwelt` file, command line | Not available for web pages. | Drag & drop, upload. |

Limits worth knowing:

* **Memory:** Rendered sounds are held in memory twice – in the core
  (worker) and in the mixer (worklet); without SharedArrayBuffer the two
  can't share memory. With very long songs with many tracks this adds up
  (WebAssembly: at most 4 GB per instance).
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
   browser extension such as Claude in Chrome, or the DevTools protocol).
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
runs in the page and supports `await`) or another agent with a browser tool.
The prompt explains the route via `window.__klangweltWeb.mcp` and the
listening loop with `chat_poll`/`chat_reply`.

**`chat_poll` waits**, as on the desktop, until a task arrives or
`timeoutSeconds` expires. In the browser the core has only one thread and
must not block; so the web worker keeps the response open and queries the
core again as soon as the UI sends a task (`chat_send`). Other commands and
tools keep running normally in the meantime; every 30 s the worker refreshes
"Agent is listening". A service worker would be the wrong place for this: it
can't see the core's state (that lives in the web worker), and the browser
terminates it when idle. The prompt uses a 20 s wait because the time limit
of the JavaScript tool is unknown. If a tool aborts the call before that, the
page doesn't notice – a task that arrives at exactly that moment counts as
picked up; so it's better to wait for a shorter time.

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
page and via the stdio bridge, agent window with prompt, waiting `chat_poll`
(a task from the chat wakes it, timeout, each task delivered only once).

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
| Generate song (trance, 3½ min, 26 clips) | 1.5–1.9 s | 5.9–6.2 s |
| WAV export of this song | 2.6–3.3 s | 7.0 s |

Small commands are faster in the browser (no IPC between processes); large
computations are slower because the core renders on a single thread there.

### How many voices in real time?

Klangwelt pre-renders notes; in real time the mixer only sums the finished
clips (panning, stereo width, reverb send, limiter). Measured with
simultaneously sounding stereo clips with reverb send, in blocks of 128
frames at 48 kHz (budget 2.67 ms per block):

| simultaneous clips | native | browser (WebAssembly) |
|---|---|---|
| 512 | 3.5% | 5.3% |
| 1024 | 7% | 10% |
| 4096 | 28% | 48% |
| 8192 | 56% | 83% |

In the real AudioWorklet, 1024 simultaneous tracks play in time
(playback rate 0.99–1.0). So for playback the browser is not a
bottleneck – 500 voices need around 5% of the budget. To measure:
`cargo test --release -p klang-engine -- --ignored --nocapture stimmen` and
`node tests/perf/voices.mjs`.

### SharedArrayBuffer and threads – worth it?

Not for playback (see above). It would help in two other places:

* **Rendering while editing:** A whole song takes 6 s in the browser on one
  thread, 1.5–1.9 s on the desktop with 8 threads. With WebAssembly threads
  it would be about 3–4 times faster. Small changes (one clip, the demo) are
  barely noticeable.
* **Memory:** Rendered sounds are currently held in memory twice (worker and
  worklet), for the song around 130–260 MB extra. With shared memory they
  would exist only once, and worker and worklet could communicate via a
  lock-free ring buffer as on the desktop.

The copying itself is the smaller part: measured at around 0.4 GB/s on the
path worker → worklet (in chunks, with `postMessage`), so for the song
0.3–0.6 s out of 6 s.

Cost: COOP/COEP must be set (the service worker can take care of that, so a
simple server is still sufficient); for shared memory and threads, Rust needs
either nightly with a rebuilt standard library or the target
`wasm32-wasip1-threads` with a small thread layer in JavaScript.
Recommendation: only once long songs become noticeably sluggish in the
browser or memory gets tight.
