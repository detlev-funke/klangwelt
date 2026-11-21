// Changelog (ui/scripts/changelog.mjs, changelog/README.md): every entry complete in all
// seven languages, CHANGELOG.md generated from the release files, versions named after
// date and time, the app's data per language, and a release in a scratch git repo.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  LANGS,
  appData,
  check,
  entryProblems,
  loadReleases,
  loadUnreleased,
  markdown,
  notes,
  release,
  releaseFileAt,
  versionAt,
  versionStamp,
} from "../../ui/scripts/changelog.mjs";

test("every entry and release in changelog/ is complete, CHANGELOG.md is up to date", () => {
  assert.deepEqual(check(), []);
  assert.ok(loadReleases().length >= 3, "the releases of the first filling");
});

test("versions: the date as major.minor.patch, the time as build metadata", () => {
  assert.equal(versionAt(new Date(2026, 10, 21, 14, 30)), "2026.11.21+1430");
  assert.equal(versionAt(new Date(2027, 0, 5, 9, 5)), "2027.1.5+0905", "SemVer: no leading zeros before the +, the time keeps them");
  assert.equal(releaseFileAt(new Date(2027, 0, 5, 9, 5)), "2027-01-05-0905.json");
  assert.equal(versionStamp("2027.1.5+0905"), "2027-01-05 09:05");
  assert.equal(versionStamp("0.1.0"), null);
  for (const r of loadReleases()) assert.match(r.version, /^\d{4}\.\d{1,2}\.\d{1,2}\+\d{4}$/);
});

test("an entry needs a type, a known platform and all seven languages (internal ones only English)", () => {
  const text = Object.fromEntries(LANGS.map((l) => [l, `text ${l}`]));
  assert.deepEqual(entryProblems({ id: "ok", type: "added", text }, "x"), []);
  assert.deepEqual(entryProblems({ id: "ok", type: "internal", text: { en: "only English" } }, "x"), []);
  const { pl, ...six } = text;
  assert.match(entryProblems({ id: "ok", type: "fixed", text: six }, "x").join(), /text\.pl is missing/);
  assert.match(entryProblems({ id: "ok", type: "new", text }, "x").join(), /type "new"/);
  assert.match(entryProblems({ id: "ok", type: "added", platform: "phone", text }, "x").join(), /platform "phone"/);
  assert.match(entryProblems({ id: "Not OK", type: "added", text }, "x").join(), /id "Not OK"/);
  assert.match(entryProblems({ id: "ok", type: "added", text: { ...text, pt: "x" } }, "x").join(), /unknown language "pt"/);
});

test("the app gets each language's texts, without internal entries", () => {
  const releases = loadReleases();
  for (const lang of LANGS) {
    const data = appData(lang);
    assert.equal(data.version, JSON.parse(readFileSync(new URL("../../ui/package.json", import.meta.url), "utf8")).version);
    assert.deepEqual(data.releases.map((r) => r.version), releases.filter((r) => r.entries.some((e) => e.type !== "internal")).map((r) => r.version));
    for (const r of data.releases) for (const e of r.entries) assert.notEqual(e.type, "internal");
    const first = releases[0].entries.find((e) => e.type !== "internal");
    assert.equal(data.releases[0].entries[0].text, first.text[lang]);
  }
  assert.equal(appData("de").unreleased.length, loadUnreleased().filter((e) => e.type !== "internal").length);
});

test("CHANGELOG.md and the GitHub notes: browser-only entries marked, left out for the desktop app", () => {
  const r = {
    version: "2026.11.21+1430",
    date: "2026-11-21T14:30+01:00",
    entries: [
      { id: "a", type: "added", text: { en: "For everyone." } },
      { id: "b", type: "fixed", platform: "browser", text: { en: "Browser fix." }, dev: "Detail." },
      { id: "c", type: "internal", text: { en: "Workflow." } },
    ],
  };
  const md = markdown([r]);
  assert.match(md, /## \[2026\.11\.21\+1430\] - 2026-11-21 14:30\n\n### Added\n\n- For everyone\.\n\n### Fixed\n\n- Browser fix\. \*\(browser only\)\*\n  - Technical: Detail\.\n\n### Internal\n\n- Workflow\./);
  assert.match(md, /\[2026\.11\.21\+1430\]: https:\/\/github\.com\/detlev-funke\/klangwelt\/releases\/tag\/v2026\.11\.21%2B1430/);
  assert.match(markdown([{ ...r, tagged: false }]), /## 2026\.11\.21\+1430 - /, "no link for a release without a tag");
  assert.match(notes(r), /Browser fix/);
  assert.doesNotMatch(notes(r, "desktop"), /Browser fix/);
  assert.match(notes(r, "desktop"), /For everyone/);
});

/** Scratch repo with the files a release touches. */
function scratchRepo() {
  const root = mkdtempSync(join(tmpdir(), "klangwelt-changelog-"));
  const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  git("init", "--quiet", "--initial-branch=main");
  git("config", "user.name", "Changelog Test");
  git("config", "user.email", "changelog-test@example.invalid");
  mkdirSync(join(root, "ui"), { recursive: true });
  mkdirSync(join(root, "changelog", "unreleased"), { recursive: true });
  writeFileSync(join(root, "ui", "package.json"), `{\n  "name": "klangwelt-ui",\n  "private": true,\n  "version": "0.1.0"\n}\n`);
  writeFileSync(join(root, "ui", "package-lock.json"), `{\n  "name": "klangwelt-ui",\n  "version": "0.1.0",\n  "lockfileVersion": 3,\n  "packages": {\n    "": {\n      "name": "klangwelt-ui",\n      "version": "0.1.0",\n      "devDependencies": { "typescript": "^7.0.2" }\n    },\n    "node_modules/typescript": {\n      "version": "7.0.2"\n    }\n  }\n}\n`);
  const text = Object.fromEntries(LANGS.map((l) => [l, `Something new (${l}).`]));
  writeFileSync(join(root, "changelog", "unreleased", "something-new.json"), JSON.stringify({ type: "added", text }));
  writeFileSync(join(root, "changelog", "unreleased", "a-fix.json"), JSON.stringify({ type: "fixed", platform: "desktop", text }));
  writeFileSync(join(root, "other.txt"), "other session\n");
  git("add", ".");
  git("commit", "--quiet", "-m", "start");
  return { root, git };
}

test("release: committed entries become a release with commit and tag; others' work stays untouched", () => {
  const { root, git } = scratchRepo();
  try {
    // another session: an entry not committed yet and a staged change of its own
    const text = Object.fromEntries(LANGS.map((l) => [l, `Later (${l}).`]));
    writeFileSync(join(root, "changelog", "unreleased", "not-committed.json"), JSON.stringify({ type: "added", text }));
    writeFileSync(join(root, "other.txt"), "other session, staged\n");
    git("add", "other.txt");

    const r = release({ root, at: new Date(2026, 10, 21, 14, 30), trailers: ["Co-Authored-By: Test <test@example.invalid>"] });
    assert.deepEqual(r, { version: "2026.11.21+1430", file: "changelog/releases/2026-11-21-1430.json", entries: 2, tag: "v2026.11.21+1430" });

    const rel = JSON.parse(readFileSync(join(root, r.file), "utf8"));
    assert.equal(rel.date.slice(0, 16), "2026-11-21T14:30");
    assert.deepEqual(rel.entries.map((e) => [e.id, e.type, e.platform]), [["something-new", "added", undefined], ["a-fix", "fixed", "desktop"]]);
    assert.ok(!existsSync(join(root, "changelog", "unreleased", "something-new.json")));
    assert.ok(existsSync(join(root, "changelog", "unreleased", "not-committed.json")), "an entry not committed yet stays");
    assert.match(readFileSync(join(root, "CHANGELOG.md"), "utf8"), /## \[2026\.11\.21\+1430\] - 2026-11-21 14:30/);
    assert.equal(JSON.parse(readFileSync(join(root, "ui", "package.json"), "utf8")).version, "2026.11.21+1430");
    const lock = JSON.parse(readFileSync(join(root, "ui", "package-lock.json"), "utf8"));
    assert.equal(lock.version, "2026.11.21+1430");
    assert.equal(lock.packages[""].version, "2026.11.21+1430");
    assert.equal(lock.packages["node_modules/typescript"].version, "7.0.2", "dependencies keep their versions");

    // the commit holds exactly the release; the other session's staged change is still only staged
    const files = git("show", "--name-status", "--format=", "HEAD").trim().split("\n").sort();
    assert.deepEqual(files, ["A\tCHANGELOG.md", "A\tchangelog/releases/2026-11-21-1430.json", "D\tchangelog/unreleased/a-fix.json", "D\tchangelog/unreleased/something-new.json", "M\tui/package-lock.json", "M\tui/package.json"]);
    assert.match(git("log", "-1", "--format=%B"), /^Release 2026\.11\.21\+1430\n\n1 added, 1 fixed – see CHANGELOG\.md\.\n\nCo-Authored-By: Test/);
    assert.equal(git("diff", "--cached", "--name-only").trim(), "other.txt");
    assert.equal(git("rev-parse", "v2026.11.21+1430^{commit}").trim(), git("rev-parse", "HEAD").trim());

    // the same minute again: refused; an uncommitted change in a file a release writes: refused
    assert.throws(() => release({ root, at: new Date(2026, 10, 21, 14, 30) }), /exists already/);
    writeFileSync(join(root, "CHANGELOG.md"), "edited by hand\n");
    assert.throws(() => release({ root, at: new Date(2026, 10, 21, 15, 0) }), /uncommitted changes/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("release without committed entries is refused", () => {
  const { root, git } = scratchRepo();
  try {
    git("rm", "--quiet", "changelog/unreleased/something-new.json", "changelog/unreleased/a-fix.json");
    git("commit", "--quiet", "-m", "no entries");
    assert.throws(() => release({ root, at: new Date(2026, 10, 21, 16, 0) }), /nothing to release/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
