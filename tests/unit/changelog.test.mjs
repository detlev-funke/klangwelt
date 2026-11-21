// Changelog (ui/scripts/changelog.mjs, changelog/README.md): every entry complete in all
// seven languages, CHANGELOG.md generated from the release files, versions named after
// date and time, the app's data per language, and a release in a scratch git repo.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  LANGS,
  appData,
  build,
  check,
  entryProblems,
  imageSize,
  loadReleases,
  loadUnreleased,
  markdown,
  notes,
  release,
  releaseFileAt,
  versionAt,
  versionStamp,
} from "../../ui/scripts/changelog.mjs";

/** One value per language: `${prefix} (${lang})`. */
const perLang = (prefix) => Object.fromEntries(LANGS.map((l) => [l, `${prefix} (${l})`]));

/** Smallest headers that say a PNG's or WebP's size. */
function pngHeader(width, height) {
  const b = Buffer.alloc(33);
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(b, 0);
  b.writeUInt32BE(13, 8);
  b.write("IHDR", 12, "latin1");
  b.writeUInt32BE(width, 16);
  b.writeUInt32BE(height, 20);
  return b;
}
function webpHeader(width, height) {
  const b = Buffer.alloc(40);
  b.write("RIFF", 0, "latin1");
  b.writeUInt32LE(32, 4);
  b.write("WEBPVP8X", 8, "latin1");
  b.writeUInt32LE(10, 16);
  b.writeUIntLE(width - 1, 24, 3);
  b.writeUIntLE(height - 1, 27, 3);
  return b;
}

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

test("an entry needs a type, a known platform, a title and a text in all seven languages (internal ones only English)", () => {
  const title = perLang("Title");
  const text = perLang("text");
  assert.deepEqual(entryProblems({ id: "ok", type: "added", title, text }, "x"), []);
  assert.deepEqual(entryProblems({ id: "ok", type: "internal", title: { en: "Workflow" }, text: { en: "only English" } }, "x"), []);
  const { pl, ...six } = text;
  assert.match(entryProblems({ id: "ok", type: "fixed", title, text: six }, "x").join(), /text\.pl is missing/);
  assert.match(entryProblems({ id: "ok", type: "new", title, text }, "x").join(), /type "new"/);
  assert.match(entryProblems({ id: "ok", type: "added", platform: "phone", title, text }, "x").join(), /platform "phone"/);
  assert.match(entryProblems({ id: "Not OK", type: "added", title, text }, "x").join(), /id "Not OK"/);
  assert.match(entryProblems({ id: "ok", type: "added", title, text: { ...text, pt: "x" } }, "x").join(), /unknown language "pt"/);
});

test("the colon after a title follows each language's typography (French: a non-breaking space before it)", async () => {
  const { loadDict } = await import("../../ui/scripts/i18n-keys.mjs");
  assert.equal((await loadDict("fr"))["{title}:"], "{title} :");
  for (const l of ["de", "it", "es", "nl", "pl"]) assert.equal((await loadDict(l))["{title}:"], "{title}:", l);
});

test("titles: every language, short, without the colon the layout adds", () => {
  const text = perLang("text");
  assert.match(entryProblems({ id: "ok", type: "added", text }, "x").join(), /"title" must be an object/);
  const { fr, ...noFr } = perLang("Title");
  assert.match(entryProblems({ id: "ok", type: "added", title: noFr, text }, "x").join(), /title\.fr is missing/);
  assert.match(entryProblems({ id: "ok", type: "added", title: { ...perLang("Title"), de: "Titel:" }, text }, "x").join(), /title\.de – no colon/);
  assert.match(entryProblems({ id: "ok", type: "added", title: { ...perLang("Title"), en: "x".repeat(81) }, text }, "x").join(), /title\.en – a title is short/);
  assert.match(entryProblems({ id: "ok", type: "internal", title: { en: "" }, text: { en: "t" } }, "x").join(), /title\.en is missing/);
});

test("screenshots: a file name in changelog/images/ that exists, not for internal entries", () => {
  const root = mkdtempSync(join(tmpdir(), "klangwelt-changelog-img-"));
  try {
    mkdirSync(join(root, "changelog", "images"), { recursive: true });
    writeFileSync(join(root, "changelog", "images", "history-window.webp"), webpHeader(1200, 800));
    const e = { id: "ok", type: "added", title: perLang("Title"), text: perLang("text") };
    assert.deepEqual(entryProblems({ ...e, image: "history-window.webp" }, "x", root), []);
    assert.match(entryProblems({ ...e, image: "missing.webp" }, "x", root).join(), /changelog\/images\/missing\.webp does not exist/);
    assert.match(entryProblems({ ...e, image: "../secret.png" }, "x", root).join(), /image "\.\.\/secret\.png"/);
    assert.match(entryProblems({ ...e, image: "shot.gif" }, "x", root).join(), /image "shot\.gif"/);
    assert.match(entryProblems({ ...e, type: "internal", image: "history-window.webp" }, "x", root).join(), /internal entry has no screenshot/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("imageSize reads PNG and WebP headers", () => {
  const root = mkdtempSync(join(tmpdir(), "klangwelt-changelog-size-"));
  try {
    writeFileSync(join(root, "a.png"), pngHeader(1600, 900));
    writeFileSync(join(root, "b.webp"), webpHeader(780, 1688));
    writeFileSync(join(root, "c.txt"), "no picture");
    assert.deepEqual(imageSize(join(root, "a.png")), { width: 1600, height: 900 });
    assert.deepEqual(imageSize(join(root, "b.webp")), { width: 780, height: 1688 });
    assert.equal(imageSize(join(root, "c.txt")), null);
    assert.equal(imageSize(join(root, "missing.png")), null);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the app gets each language's texts, without internal entries", () => {
  const releases = loadReleases();
  for (const lang of LANGS) {
    const data = appData(lang);
    assert.equal(data.version, JSON.parse(readFileSync(new URL("../../ui/package.json", import.meta.url), "utf8")).version);
    assert.deepEqual(data.releases.map((r) => r.version), releases.filter((r) => r.entries.some((e) => e.type !== "internal")).map((r) => r.version));
    for (const r of data.releases) for (const e of r.entries) assert.notEqual(e.type, "internal");
    const first = releases[0].entries.find((e) => e.type !== "internal");
    assert.equal(data.releases[0].entries[0].title, first.title[lang]);
    assert.equal(data.releases[0].entries[0].text, first.text[lang]);
  }
  assert.equal(appData("de").unreleased.length, loadUnreleased().filter((e) => e.type !== "internal").length);
});

test("screenshots in changelog/images/: each one belongs to an entry, small enough to ship in the app", () => {
  const dir = new URL("../../changelog/images/", import.meta.url);
  const files = existsSync(dir) ? readdirSync(dir) : [];
  const used = new Set([...loadReleases().flatMap((r) => r.entries), ...loadUnreleased()].map((e) => e.image).filter(Boolean));
  assert.deepEqual(files.filter((f) => !used.has(f)), [], "pictures no entry names");
  for (const f of files) {
    const size = statSync(new URL(f, dir)).size;
    assert.ok(size < 300 * 1024, `${f}: ${Math.round(size / 1024)} KB – at most 300 KB`);
    assert.ok(imageSize(new URL(f, dir)), `${f}: a PNG or WebP`);
  }
});

test("the app's data carries title and screenshot; build copies the screenshots next to it", () => {
  const root = mkdtempSync(join(tmpdir(), "klangwelt-changelog-build-"));
  try {
    mkdirSync(join(root, "ui"), { recursive: true });
    mkdirSync(join(root, "changelog", "unreleased"), { recursive: true });
    mkdirSync(join(root, "changelog", "images"), { recursive: true });
    writeFileSync(join(root, "ui", "package.json"), `{ "name": "klangwelt-ui", "version": "0.1.0" }\n`);
    writeFileSync(join(root, "changelog", "images", "big-feature.webp"), webpHeader(1200, 800));
    writeFileSync(join(root, "changelog", "images", "not-used.webp"), webpHeader(10, 10));
    writeFileSync(join(root, "changelog", "unreleased", "big-feature.json"), JSON.stringify({ type: "added", title: perLang("Big"), text: perLang("does a lot."), image: "big-feature.webp" }));
    const data = appData("fr", root);
    assert.deepEqual(data.unreleased, [{ id: "big-feature", type: "added", platform: "all", title: "Big (fr)", text: "does a lot. (fr)", image: "changelog/images/big-feature.webp" }]);
    const out = build(root, join(root, "ui", "dist", "changelog"));
    assert.ok(existsSync(join(out, "images", "big-feature.webp")));
    assert.ok(!existsSync(join(out, "images", "not-used.webp")), "only pictures of shown entries");
    assert.match(readFileSync(join(out, "de.js"), "utf8"), /"title":"Big \(de\)"/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("CHANGELOG.md and the GitHub notes: browser-only entries marked, left out for the desktop app", () => {
  const r = {
    version: "2026.11.21+1430",
    date: "2026-11-21T14:30+01:00",
    entries: [
      { id: "a", type: "added", title: { en: "Everyone" }, text: { en: "for everyone." } },
      { id: "b", type: "fixed", platform: "browser", title: { en: "Browser" }, text: { en: "a browser fix." }, dev: "Detail." },
      { id: "c", type: "internal", title: { en: "CI" }, text: { en: "a workflow." } },
    ],
  };
  const md = markdown([r]);
  assert.match(md, /## \[2026\.11\.21\+1430\] - 2026-11-21 14:30\n\n### Added\n\n- \*\*Everyone:\*\* for everyone\.\n\n### Fixed\n\n- \*\*Browser:\*\* a browser fix\. \*\(browser only\)\*\n  - Technical: Detail\.\n\n### Internal\n\n- \*\*CI:\*\* a workflow\./);
  assert.match(md, /\[2026\.11\.21\+1430\]: https:\/\/github\.com\/detlev-funke\/klangwelt\/releases\/tag\/v2026\.11\.21%2B1430/);
  assert.match(markdown([{ ...r, tagged: false }]), /## 2026\.11\.21\+1430 - /, "no link for a release without a tag");
  assert.match(notes(r), /\*\*Browser:\*\* a browser fix/);
  assert.doesNotMatch(notes(r, "desktop"), /browser fix/);
  assert.match(notes(r, "desktop"), /\*\*Everyone:\*\* for everyone/);
});

test("a screenshot below its entry: in the repo for CHANGELOG.md, at the tag on GitHub, at its natural size up to 640 px", () => {
  const root = mkdtempSync(join(tmpdir(), "klangwelt-changelog-md-"));
  try {
    mkdirSync(join(root, "changelog", "images"), { recursive: true });
    writeFileSync(join(root, "changelog", "images", "wide.webp"), webpHeader(1800, 1000));
    writeFileSync(join(root, "changelog", "images", "phone.png"), pngHeader(780, 1688));
    const r = {
      version: "2026.11.21+1430",
      date: "2026-11-21T14:30+01:00",
      entries: [
        { id: "w", type: "added", title: { en: 'Wide "window"' }, text: { en: "a window." }, image: "wide.webp", dev: "Detail." },
        { id: "p", type: "added", title: { en: "Phone" }, text: { en: "on a phone." }, image: "phone.png" },
      ],
    };
    const md = markdown([r], root);
    assert.match(md, /- \*\*Wide "window":\*\* a window\.<br>\n  <img src="changelog\/images\/wide\.webp" alt="Wide &quot;window&quot;" width="640">\n  - Technical: Detail\./);
    assert.match(md, /- \*\*Phone:\*\* on a phone\.<br>\n  <img src="changelog\/images\/phone\.png" alt="Phone" width="390">/, "double density: half the pixels");
    assert.match(notes(r, "all", root), /<img src="https:\/\/raw\.githubusercontent\.com\/detlev-funke\/klangwelt\/v2026\.11\.21%2B1430\/changelog\/images\/wide\.webp"/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
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
  mkdirSync(join(root, "changelog", "images"), { recursive: true });
  const title = perLang("Something");
  const text = perLang("something new.");
  writeFileSync(join(root, "changelog", "images", "something-new.webp"), webpHeader(1200, 800));
  writeFileSync(join(root, "changelog", "unreleased", "something-new.json"), JSON.stringify({ type: "added", title, text, image: "something-new.webp" }));
  writeFileSync(join(root, "changelog", "unreleased", "a-fix.json"), JSON.stringify({ type: "fixed", platform: "desktop", title, text }));
  writeFileSync(join(root, "other.txt"), "other session\n");
  git("add", ".");
  git("commit", "--quiet", "-m", "start");
  return { root, git };
}

test("release: committed entries become a release with commit and tag; others' work stays untouched", () => {
  const { root, git } = scratchRepo();
  try {
    // another session: an entry with its screenshot not committed yet and a staged change of its own
    writeFileSync(join(root, "changelog", "images", "not-committed.webp"), webpHeader(800, 600));
    writeFileSync(join(root, "changelog", "unreleased", "not-committed.json"), JSON.stringify({ type: "added", title: perLang("Later"), text: perLang("later."), image: "not-committed.webp" }));
    writeFileSync(join(root, "other.txt"), "other session, staged\n");
    git("add", "other.txt");

    const r = release({ root, at: new Date(2026, 10, 21, 14, 30), trailers: ["Co-Authored-By: Test <test@example.invalid>"] });
    assert.deepEqual(r, { version: "2026.11.21+1430", file: "changelog/releases/2026-11-21-1430.json", entries: 2, tag: "v2026.11.21+1430" });

    const rel = JSON.parse(readFileSync(join(root, r.file), "utf8"));
    assert.equal(rel.date.slice(0, 16), "2026-11-21T14:30");
    assert.deepEqual(rel.entries.map((e) => [e.id, e.type, e.platform, e.title.de, e.image]), [["something-new", "added", undefined, "Something (de)", "something-new.webp"], ["a-fix", "fixed", "desktop", "Something (de)", undefined]]);
    assert.ok(existsSync(join(root, "changelog", "images", "something-new.webp")), "screenshots stay for CHANGELOG.md");
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

test("release of an entry whose screenshot isn't committed is refused: the notes point at the tag", () => {
  const { root, git } = scratchRepo();
  try {
    git("rm", "--quiet", "--cached", "changelog/images/something-new.webp");
    git("commit", "--quiet", "-m", "picture not committed");
    assert.throws(() => release({ root, at: new Date(2026, 10, 21, 17, 0) }), /changelog\/images\/something-new\.webp is not committed/);
    assert.equal(git("tag", "--list").trim(), "", "no tag");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
