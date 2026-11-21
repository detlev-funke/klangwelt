// Temporary folders of the end-to-end tests – the browser harness (tests/web), the
// desktop app's harness (klangwelt-desktop tests/e2e, which imports this file from its
// klangwelt/ submodule) and the tests that write files of their own (a WAV for the
// substitute microphone, an isolated library): create them, remove them reliably and
// sweep what earlier runs left behind. Without third-party packages.
//
// On Windows a folder can only be removed once no process holds a file in it, and the
// child processes of Edge and WebView2 (renderer, GPU, crashpad) often outlive the main
// process by seconds. So `stopProcess` waits for the main process and kills its whole
// tree if needed, and `removeTempDir` retries for half a minute, killing every process
// whose command line still names the folder (e.g. `--user-data-dir=…`).
//
// A run that is killed or hangs never gets to remove its folders. `makeTempDir`
// therefore first sweeps – once per process and prefix – folders with its prefix that
// have not changed for a few hours and that no running process names.

import { execFile, execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

const WINDOWS = process.platform === "win32";
/** Every prefix starts with this – the process query only looks at such command lines. */
const ROOT_PREFIX = "klangwelt-";
/** A folder unchanged for this long belongs to a killed or hung run. */
export const STALE_MS = 3 * 60 * 60 * 1000;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const swept = new Set();

/** A new folder `<temp>/<prefix>XXXXXX`; sweeps stale folders with the same prefix first. */
export function makeTempDir(prefix) {
  if (!prefix.startsWith(ROOT_PREFIX)) throw new Error(`temp prefix must start with ${ROOT_PREFIX}: ${prefix}`);
  if (!swept.has(prefix)) {
    swept.add(prefix);
    sweepStale(prefix);
  }
  return mkdtempSync(join(tmpdir(), prefix));
}

/**
 * Removes the temp folders `<temp>/<prefix>*` that have not changed for `maxAge` ms
 * and that no running process names on its command line. Synchronous, so it also
 * works at a module's top level; it only asks for processes when there is a candidate.
 * Returns the removed folders.
 */
export function sweepStale(prefix, { maxAge = STALE_MS } = {}) {
  const root = tmpdir();
  const now = Date.now();
  let stale;
  try {
    stale = readdirSync(root, { withFileTypes: true })
      .filter((e) => e.isDirectory() && e.name.startsWith(prefix))
      .map((e) => join(root, e.name))
      .filter((dir) => now - lastChange(dir) > maxAge);
  } catch {
    return [];
  }
  if (!stale.length) return [];
  let procs = [];
  if (WINDOWS) {
    try {
      procs = parseProcesses(execFileSync("powershell.exe", psArgs(), { encoding: "utf8", windowsHide: true, timeout: 30000, stdio: ["ignore", "pipe", "ignore"] }));
    } catch {
      return []; // without the process list nobody can tell what is still in use
    }
  }
  const removed = [];
  for (const dir of stale) {
    if (procs.some((p) => names(p.cmd, dir))) continue;
    if (tryRemove(dir)) removed.push(dir);
  }
  return removed;
}

/**
 * Removes a temp folder for good: retries for `timeout` ms and, after `grace` ms,
 * kills the processes (with their trees) whose command line still names the folder.
 * Returns whether it is gone; warns instead of throwing – the next run sweeps it.
 */
export async function removeTempDir(dir, { timeout = 30000, grace = 2000 } = {}) {
  if (!dir) return true;
  const start = Date.now();
  let error;
  for (;;) {
    try {
      rmSync(dir, { recursive: true, force: true });
      if (!existsSync(dir)) return true;
    } catch (e) {
      error = e;
    }
    if (Date.now() - start > timeout) {
      console.warn(`temp folder not removed: ${dir} (${error?.code ?? error?.message ?? "still there"})`);
      return false;
    }
    if (Date.now() - start >= grace) for (const p of await processesNaming(dir)) await killTree(p.pid);
    await sleep(400);
  }
}

/** Running processes whose command line names `dir` (Windows only; elsewhere none). */
export async function processesNaming(dir) {
  if (!WINDOWS) return [];
  const out = await new Promise((res) =>
    execFile("powershell.exe", psArgs(), { encoding: "utf8", windowsHide: true, timeout: 30000 }, (err, stdout) => res(err ? "" : stdout)),
  );
  return parseProcesses(out).filter((p) => names(p.cmd, dir));
}

/** Kills a process with all its descendants (Windows: taskkill /T /F). */
export async function killTree(pid) {
  if (!pid) return;
  if (!WINDOWS) {
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      /* already exited */
    }
    return;
  }
  await new Promise((res) => execFile("taskkill", ["/PID", String(pid), "/T", "/F"], { windowsHide: true }, () => res()));
}

/** Waits up to `ms` for a child process to exit; true if it has. */
export function exited(child, ms) {
  if (child.exitCode != null || child.signalCode != null) return Promise.resolve(true);
  return new Promise((res) => {
    const timer = setTimeout(() => res(false), ms);
    child.once("exit", () => {
      clearTimeout(timer);
      res(true);
    });
  });
}

/**
 * Ends a child process with everything it started: gives it `grace` ms to exit by
 * itself (after a polite request elsewhere, e.g. CDP Browser.close), then kills its
 * whole tree – while it still runs, because Windows finds the descendants of a
 * process only through it – and waits for the exit.
 */
export async function stopProcess(child, { grace = 0 } = {}) {
  if (!child) return;
  if (grace && (await exited(child, grace))) return;
  if (child.exitCode == null && child.signalCode == null) await killTree(child.pid);
  await exited(child, 5000);
}

// ---------------------------------------------------------------- internals

/** Newest change of the folder and its direct entries (a running profile touches them). */
function lastChange(dir) {
  let t = statSync(dir).mtimeMs;
  for (const name of readdirSync(dir)) {
    try {
      t = Math.max(t, statSync(join(dir, name)).mtimeMs);
    } catch {
      /* vanished meanwhile */
    }
  }
  return t;
}

function tryRemove(dir) {
  try {
    rmSync(dir, { recursive: true, force: true });
    return !existsSync(dir);
  } catch {
    return false;
  }
}

/** PowerShell arguments listing "pid<TAB>command line" of the processes that mention a klangwelt- path. */
function psArgs() {
  const script = [
    "$ProgressPreference = 'SilentlyContinue'",
    "[Console]::OutputEncoding = [Text.Encoding]::UTF8",
    `Get-CimInstance Win32_Process -Filter "CommandLine like '%${ROOT_PREFIX}%'" | ForEach-Object { "$($_.ProcessId)\`t$($_.CommandLine)" }`,
  ].join("; ");
  // encoded, so no quoting survives the trip through the command line by accident
  return ["-NoProfile", "-NonInteractive", "-EncodedCommand", Buffer.from(script, "utf16le").toString("base64")];
}

function parseProcesses(out) {
  return out
    .split(/\r?\n/)
    .map((line) => {
      const tab = line.indexOf("\t");
      return { pid: Number(line.slice(0, tab)), cmd: line.slice(tab + 1) };
    })
    .filter((p) => p.pid > 0 && p.pid !== process.pid);
}

/** Does a command line name the folder (as a whole path component, any case)? */
function names(cmd, dir) {
  const c = cmd.toLowerCase();
  const b = basename(dir).toLowerCase();
  for (let i = c.indexOf(b); i >= 0; i = c.indexOf(b, i + 1)) {
    const before = c[i - 1];
    const after = c[i + b.length];
    if ((before === "\\" || before === "/") && (after === undefined || !/[\w-]/.test(after))) return true;
  }
  return false;
}
