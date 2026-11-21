// Temporary folders of the tests (tests/tempdirs.mjs): close() leaves no browser
// profile and no Edge process behind, and a folder that a killed run left behind
// disappears on the next start – unless a running process still names it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, utimesSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { startWeb } from "./harness.mjs";
import { STALE_MS, makeTempDir, processesNaming, removeTempDir, stopProcess, sweepStale } from "../tempdirs.mjs";

const WINDOWS = process.platform === "win32";

test("close() ends Edge with all its processes and removes the profile folder", async () => {
  const app = await startWeb();
  const { tmp } = app;
  try {
    assert.ok(existsSync(join(tmp, "profil")), "profile in the temp folder");
    if (WINDOWS) assert.ok((await processesNaming(tmp)).length > 0, "Edge names its profile on the command line");
  } finally {
    await app.close();
  }
  assert.equal(existsSync(tmp), false, `profile folder removed: ${tmp}`);
  assert.deepEqual(await processesNaming(tmp), [], "no Edge process left");
});

test("the next start sweeps stale folders of killed runs, but not fresh or used ones", { skip: !WINDOWS && "process check on Windows only" }, async () => {
  const prefix = "klangwelt-sweeptest-";
  const old = new Date(Date.now() - STALE_MS - 60_000);
  const [stale, used, fresh] = [makeTempDir(prefix), makeTempDir(prefix), makeTempDir(prefix)];
  for (const dir of [stale, used]) {
    writeFileSync(join(dir, "Local State"), "{}");
    utimesSync(join(dir, "Local State"), old, old);
    utimesSync(dir, old, old);
  }
  // still in use: a process names the folder on its command line, like Edge its --user-data-dir
  const user = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000)", join(used, "profil")], { stdio: "ignore" });
  await once(user, "spawn");
  try {
    assert.deepEqual(sweepStale(prefix), [stale], "only the stale folder nobody uses");
    assert.equal(existsSync(stale), false);
    assert.ok(existsSync(used), "used folder stays");
    assert.ok(existsSync(fresh), "fresh folder stays");
  } finally {
    await stopProcess(user);
  }
  assert.deepEqual(sweepStale(prefix), [used], "swept once its process has ended");
  assert.equal(await removeTempDir(fresh), true);
  assert.equal(existsSync(fresh), false);
});
