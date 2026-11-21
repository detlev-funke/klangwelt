// Changelog: one JSON file per change in changelog/unreleased/, one per release
// in changelog/releases/. From them come CHANGELOG.md (English), the release notes
// on GitHub and the data of the app's "What's new" window. Node built-ins only.
//
//   npm run changelog -- check      check every entry (the unit tests do it too)
//   npm run changelog -- release    the committed entries become a release named after
//                                   the current time (2026.11.21+1430): release file,
//                                   CHANGELOG.md, version in ui/package.json, commit, tag
//   npm run changelog -- markdown   rewrite CHANGELOG.md from the release files
//   npm run changelog -- notes <version> [--platform=desktop|browser]
//                                   release notes (Markdown) for GitHub, on stdout
//   npm run changelog -- build      data for the app → ui/dist/changelog/<lang>.js
//                                   (part of `npm run build`)
//
// Format and rules: changelog/README.md.

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Root of the klangwelt repo. */
export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
export const LANGS = ["en", "de", "fr", "it", "es", "nl", "pl"];
export const TYPES = ["added", "changed", "fixed", "removed", "internal"];
export const PLATFORMS = ["all", "browser", "desktop"];
/** Paths a release writes (relative to the root, with forward slashes). */
export const RELEASE_PATHS = ["CHANGELOG.md", "changelog", "ui/package.json", "ui/package-lock.json"];

const HEADINGS = { added: "Added", changed: "Changed", fixed: "Fixed", removed: "Removed", internal: "Internal" };
const ONLY = { browser: "browser only", desktop: "desktop only" };
const REPO = "https://github.com/detlev-funke/klangwelt";
const RAW = "https://raw.githubusercontent.com/detlev-funke/klangwelt";
const WEB = "https://detlev-funke.github.io/klangwelt/";
const ID_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
/** Screenshots live in changelog/images/; an entry names its file. */
const IMAGE_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*\.(?:webp|png)$/;
/** Widest a screenshot is shown in CHANGELOG.md and on GitHub (CSS pixels). */
const IMAGE_WIDTH = 640;
const TITLE_MAX = 80;
/** 2026.11.21+1430: the date as major.minor.patch, the time as build metadata (valid SemVer). */
export const VERSION_RE = /^(\d{4})\.([1-9]\d?)\.([1-9]\d?)\+(\d{2})(\d{2})$/;
const ISO_RE = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(Z|[+-]\d{2}:\d{2})$/;

const pad = (n) => String(n).padStart(2, "0");
const paths = (root) => ({
  unreleased: join(root, "changelog", "unreleased"),
  releases: join(root, "changelog", "releases"),
  images: join(root, "changelog", "images"),
  markdown: join(root, "CHANGELOG.md"),
  pkg: join(root, "ui", "package.json"),
  lock: join(root, "ui", "package-lock.json"),
});

// ------------------------------------------------------------------ Versions

/** Version of a release made at local time `d`: 2026.11.21+1430. */
export function versionAt(d) {
  return `${d.getFullYear()}.${d.getMonth() + 1}.${d.getDate()}+${pad(d.getHours())}${pad(d.getMinutes())}`;
}

/** Local time `d` as ISO 8601 with its offset, to the minute: 2026-11-21T14:30+01:00. */
export function isoAt(d) {
  const off = -d.getTimezoneOffset();
  const a = Math.abs(off);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}${off >= 0 ? "+" : "-"}${pad(Math.floor(a / 60))}:${pad(a % 60)}`;
}

/** File name of the release made at local time `d`: 2026-11-21-1430.json (sorts by time). */
export function releaseFileAt(d) {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}.json`;
}

/** "2026-11-21 14:30" – the date and time a version names (null if it is no release version). */
export function versionStamp(v) {
  const m = VERSION_RE.exec(v ?? "");
  return m ? `${m[1]}-${pad(m[2])}-${pad(m[3])} ${m[4]}:${m[5]}` : null;
}

/** "2026-11-21 14:30" from an ISO time as written (its own offset, not converted). */
function isoStamp(iso) {
  const m = ISO_RE.exec(iso ?? "");
  return m ? `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}` : null;
}

/** "2026-11-21 14:30" → local Date. */
export function parseStamp(s) {
  const m = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})$/.exec(s ?? "");
  if (!m) throw new Error(`time "${s}": expected YYYY-MM-DD HH:MM`);
  return new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5]);
}

// ------------------------------------------------------------------ Reading and checking

/** Problems of a per-language field ("title" or "text"): every needed language, one line each. */
function languageProblems(e, field, where, check) {
  const v = e[field];
  if (!v || typeof v !== "object" || Array.isArray(v)) return [`${where}: "${field}" must be an object with one ${field} per language`];
  const p = [];
  for (const k of Object.keys(v)) if (!LANGS.includes(k)) p.push(`${where}: unknown language "${k}" in ${field}`);
  for (const l of e.type === "internal" ? ["en"] : LANGS) {
    const s = v[l];
    if (typeof s !== "string" || !s.trim()) p.push(`${where}: ${field}.${l} is missing`);
    else if (s !== s.trim() || /\n/.test(s)) p.push(`${where}: ${field}.${l} – one line, no surrounding spaces`);
    else p.push(...check(s, `${where}: ${field}.${l}`));
  }
  return p;
}

/**
 * Problems of one entry (`where` names it in the messages). With `root`, a screenshot
 * the entry names must exist in changelog/images/.
 */
export function entryProblems(e, where, root = null) {
  const p = [];
  if (!e || typeof e !== "object" || Array.isArray(e)) return [`${where}: not an object`];
  for (const k of Object.keys(e)) if (!["id", "type", "platform", "title", "text", "image", "dev"].includes(k)) p.push(`${where}: unknown field "${k}"`);
  if (typeof e.id !== "string" || !ID_RE.test(e.id)) p.push(`${where}: id "${e.id}" – lowercase letters, digits and hyphens`);
  if (!TYPES.includes(e.type)) p.push(`${where}: type "${e.type}" – one of ${TYPES.join(", ")}`);
  if (e.platform !== undefined && !PLATFORMS.includes(e.platform)) p.push(`${where}: platform "${e.platform}" – one of ${PLATFORMS.join(", ")}`);
  p.push(
    ...languageProblems(e, "title", where, (s, at) => [
      ...(s.length > TITLE_MAX ? [`${at} – a title is short (at most ${TITLE_MAX} characters)`] : []),
      ...(/[:.]$/.test(s) ? [`${at} – no colon or full stop at the end, the colon comes with the layout`] : []),
    ]),
  );
  p.push(...languageProblems(e, "text", where, () => []));
  if (e.image !== undefined) {
    if (typeof e.image !== "string" || !IMAGE_RE.test(e.image)) p.push(`${where}: image "${e.image}" – a file name in changelog/images/ like history-window.webp`);
    else if (e.type === "internal") p.push(`${where}: an internal entry has no screenshot`);
    else if (root && !existsSync(join(paths(root).images, e.image))) p.push(`${where}: changelog/images/${e.image} does not exist`);
  }
  if (e.dev !== undefined && (typeof e.dev !== "string" || !e.dev.trim())) p.push(`${where}: "dev" must be a non-empty text`);
  return p;
}

function readJson(file) {
  return JSON.parse(readFileSync(file, "utf8"));
}

/** Times (Unix seconds) at which git first saw each file under `dir` (relative paths, "/"). */
function addedTimes(root, dir) {
  const times = new Map();
  try {
    const out = execFileSync("git", ["log", "--diff-filter=A", "--format=@%at", "--name-only", "--", dir], { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
    let t = 0;
    for (const line of out.split("\n")) {
      if (line.startsWith("@")) t = Number(line.slice(1));
      else if (line.trim()) times.set(line.trim(), t); // log runs newest first: the oldest wins
    }
  } catch {
    /* no git: by name */
  }
  return times;
}

/**
 * Unreleased entries, in the order of a release: by type, then oldest first (when git
 * added the file; files git doesn't know yet come last), then by name.
 * Each entry: the file's fields plus `id` (file name) and `file`; `problems` lists
 * what's wrong (an unreadable file gives an entry with only `id`, `file` and `problems`).
 */
export function loadUnreleased(root = ROOT) {
  const dir = paths(root).unreleased;
  if (!existsSync(dir)) return [];
  const times = addedTimes(root, "changelog/unreleased");
  const entries = readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .map((f) => {
      const file = join(dir, f);
      const id = f.slice(0, -5);
      const rel = `changelog/unreleased/${f}`;
      let e;
      try {
        const json = readJson(file);
        if (json && typeof json === "object" && "id" in json) return { id, file, rel, problems: [`${rel}: no "id" in a fragment – the file name is the id`] };
        e = { id, ...json };
      } catch (err) {
        return { id, file, rel, problems: [`${rel}: ${err.message}`] };
      }
      return { ...e, file, rel, added: times.get(rel) ?? Infinity, problems: entryProblems(e, rel, root) };
    });
  return entries.sort((a, b) => TYPES.indexOf(a.type) - TYPES.indexOf(b.type) || (a.added ?? Infinity) - (b.added ?? Infinity) || a.id.localeCompare(b.id));
}

/** Releases, newest first. Each: the file's fields plus `file` and `problems`. */
export function loadReleases(root = ROOT) {
  const dir = paths(root).releases;
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .map((f) => {
      const rel = `changelog/releases/${f}`;
      let r;
      try {
        r = readJson(join(dir, f));
      } catch (err) {
        return { file: f, rel, entries: [], problems: [`${rel}: ${err.message}`] };
      }
      return { ...r, file: f, rel, problems: releaseProblems(r, f, root) };
    })
    .sort((a, b) => (Date.parse(b.date) || 0) - (Date.parse(a.date) || 0));
}

function releaseProblems(r, f, root) {
  const where = `changelog/releases/${f}`;
  const p = [];
  if (!r || typeof r !== "object") return [`${where}: not an object`];
  for (const k of Object.keys(r)) if (!["version", "date", "tagged", "entries"].includes(k)) p.push(`${where}: unknown field "${k}"`);
  const stamp = versionStamp(r.version);
  if (!stamp) p.push(`${where}: version "${r.version}" – like 2026.11.21+1430`);
  if (!isoStamp(r.date)) p.push(`${where}: date "${r.date}" – like 2026-11-21T14:30+01:00`);
  else if (stamp && stamp !== isoStamp(r.date)) p.push(`${where}: version ${r.version} doesn't match date ${r.date}`);
  const name = stamp && `${stamp.slice(0, 10)}-${stamp.slice(11).replace(":", "")}.json`;
  if (name && f !== name) p.push(`${where}: file name should be ${name}`);
  if (r.tagged !== undefined && typeof r.tagged !== "boolean") p.push(`${where}: "tagged" must be true or false`);
  if (!Array.isArray(r.entries) || !r.entries.length) p.push(`${where}: no entries`);
  else {
    const ids = new Set();
    r.entries.forEach((e, i) => {
      p.push(...entryProblems(e, `${where} entry ${i + 1}`, root));
      if (ids.has(e?.id)) p.push(`${where}: id "${e.id}" twice`);
      ids.add(e?.id);
    });
  }
  return p;
}

/** Everything that's wrong with the changelog (empty: all fine). */
export function check(root = ROOT) {
  const releases = loadReleases(root);
  const unreleased = loadUnreleased(root);
  const problems = [...releases.flatMap((r) => r.problems), ...unreleased.flatMap((e) => e.problems)];
  const versions = new Set();
  for (const r of releases) {
    if (versions.has(r.version)) problems.push(`version ${r.version} twice`);
    versions.add(r.version);
  }
  const pkg = readJson(paths(root).pkg).version;
  if (releases.some((r) => r.tagged !== false) && pkg !== releases.find((r) => r.tagged !== false).version)
    problems.push(`ui/package.json has version ${pkg}, the newest release is ${releases.find((r) => r.tagged !== false).version}`);
  if (!problems.length && existsSync(paths(root).markdown) && readFileSync(paths(root).markdown, "utf8").replaceAll("\r\n", "\n") !== markdown(releases, root))
    problems.push("CHANGELOG.md is not what the release files give – npm run changelog -- markdown");
  return problems;
}

// ------------------------------------------------------------------ Markdown

/** Pixel size of a PNG or WebP file (null if unknown). */
export function imageSize(file) {
  let b;
  try {
    b = readFileSync(file);
  } catch {
    return null;
  }
  if (b.length > 24 && b.toString("latin1", 1, 4) === "PNG") return { width: b.readUInt32BE(16), height: b.readUInt32BE(20) };
  if (b.length > 30 && b.toString("latin1", 0, 4) === "RIFF" && b.toString("latin1", 8, 12) === "WEBP") {
    const chunk = b.toString("latin1", 12, 16);
    if (chunk === "VP8X") return { width: 1 + b.readUIntLE(24, 3), height: 1 + b.readUIntLE(27, 3) };
    if (chunk === "VP8 ") return { width: b.readUInt16LE(26) & 0x3fff, height: b.readUInt16LE(28) & 0x3fff };
    if (chunk === "VP8L") {
      const bits = b.readUInt32LE(21);
      return { width: 1 + (bits & 0x3fff), height: 1 + ((bits >> 14) & 0x3fff) };
    }
  }
  return null;
}

/**
 * The screenshot below an entry's text: at its natural size (screenshots are taken at
 * double pixel density), at most IMAGE_WIDTH wide. `src` is where Markdown finds it.
 */
function imageTag(e, src, root) {
  const size = root ? imageSize(join(paths(root).images, e.image)) : null;
  const width = size ? Math.min(IMAGE_WIDTH, Math.round(size.width / 2)) : IMAGE_WIDTH;
  const alt = e.title.en.replaceAll('"', "&quot;");
  return `<img src="${src}" alt="${alt}" width="${width}">`;
}

/**
 * Lines of one entry: "**Title:** text", the screenshot right below it (a <br> keeps
 * the list tight), then the technical line. `imageSrc(file)` gives the screenshot's URL.
 */
function entryLines(e, imageSrc, root) {
  const only = ONLY[e.platform] ? ` *(${ONLY[e.platform]})*` : "";
  const first = `- **${e.title.en}:** ${e.text.en}${only}`;
  const lines = e.image ? [`${first}<br>`, `  ${imageTag(e, imageSrc(e.image), root)}`] : [first];
  if (e.dev) lines.push(`  - Technical: ${e.dev}`);
  return lines;
}

function sections(entries, imageSrc, root) {
  const out = [];
  for (const type of TYPES) {
    const of = entries.filter((e) => e.type === type);
    if (!of.length) continue;
    out.push("", `### ${HEADINGS[type]}`, "", ...of.flatMap((e) => entryLines(e, imageSrc, root)));
  }
  return out;
}

const tagUrl = (v) => `${REPO}/releases/tag/v${encodeURIComponent(v)}`;

/** CHANGELOG.md from the releases (newest first); screenshots as paths in the repo. */
export function markdown(releases, root = ROOT) {
  const out = [
    "# Changelog",
    "",
    "All notable changes to Klangwelt. The browser version and the desktop app share",
    "one version number: the date and time of the release, so `2026.11.21+1430` is",
    "21 November 2026, 14:30. The format follows",
    "[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).",
    "",
    "This file is generated from the JSON files in [changelog/](changelog/README.md),",
    "which also says how to add an entry. The app shows the same entries in its own",
    "language under the gear menu → What's new.",
  ];
  for (const r of releases) {
    const name = r.tagged === false ? r.version : `[${r.version}]`;
    out.push("", `## ${name} - ${versionStamp(r.version)}`, ...sections(r.entries, (f) => `changelog/images/${f}`, root));
  }
  const links = releases.filter((r) => r.tagged !== false).map((r) => `[${r.version}]: ${tagUrl(r.version)}`);
  if (links.length) out.push("", ...links);
  return `${out.join("\n")}\n`;
}

/**
 * Release notes for GitHub: the entries of one release (for the desktop app without
 * browser-only ones and vice versa). A release page has no repo around it, so
 * screenshots are addressed at the release's tag.
 */
export function notes(release, platform = "all", root = ROOT) {
  const entries = release.entries.filter((e) => platform === "all" || !e.platform || e.platform === "all" || e.platform === platform);
  const intro =
    platform === "desktop"
      ? `Released ${versionStamp(release.version)}. The desktop app for Windows is the .exe below – no installer, just start it. The same version runs in the browser: ${WEB}`
      : `Released ${versionStamp(release.version)}. Open Klangwelt in your browser: ${WEB} – the desktop app: https://github.com/detlev-funke/klangwelt-desktop/releases`;
  const imageSrc = (f) => `${RAW}/v${encodeURIComponent(release.version)}/changelog/images/${f}`;
  return `${[intro, ...sections(entries, imageSrc, root)].join("\n")}\n`;
}

// ------------------------------------------------------------------ Data for the app

function gitShort(root) {
  try {
    return execFileSync("git", ["rev-parse", "--short", "HEAD"], { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim() || null;
  } catch {
    return null;
  }
}

/**
 * What the app shows in language `lang`: version, build, releases and unreleased
 * entries without internal ones, each title and text in that language (English if
 * missing), a screenshot as its path next to index.html (`build` copies it there).
 * Faulty entries are left out (`check` reports them).
 */
export function appData(lang, root = ROOT, { releases = loadReleases(root), unreleased = loadUnreleased(root) } = {}) {
  const shown = (e) => !e.problems?.length && e.type !== "internal";
  const view = (e) => ({
    id: e.id,
    type: e.type,
    platform: e.platform ?? "all",
    title: e.title[lang] || e.title.en,
    text: e.text[lang] || e.text.en,
    ...(e.image ? { image: `changelog/images/${e.image}` } : {}),
  });
  return {
    version: readJson(paths(root).pkg).version,
    build: gitShort(root),
    releases: releases
      .filter((r) => !r.problems.length)
      .map((r) => ({ version: r.version, date: r.date, entries: r.entries.filter(shown).map(view) }))
      .filter((r) => r.entries.length),
    unreleased: unreleased.filter(shown).map(view),
  };
}

/**
 * ui/dist/changelog/<lang>.js for every language (ES modules: the desktop's CSP allows
 * no fetch), and the screenshots the shown entries name in ui/dist/changelog/images/.
 */
export function build(root = ROOT, out = join(root, "ui", "dist", "changelog")) {
  const releases = loadReleases(root);
  const unreleased = loadUnreleased(root);
  for (const p of [...releases.flatMap((r) => r.problems), ...unreleased.flatMap((e) => e.problems)]) console.warn(`changelog: ${p} (left out)`);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(join(out, "images"), { recursive: true });
  const images = new Set();
  for (const lang of LANGS) {
    const data = appData(lang, root, { releases, unreleased });
    for (const e of [...data.releases.flatMap((r) => r.entries), ...data.unreleased]) if (e.image) images.add(e.image.slice("changelog/images/".length));
    writeFileSync(join(out, `${lang}.js`), `// Generated by ui/scripts/changelog.mjs from changelog/ – do not edit.\nexport default ${JSON.stringify(data)};\n`);
  }
  for (const f of images) copyFileSync(join(paths(root).images, f), join(out, "images", f));
  return out;
}

// ------------------------------------------------------------------ Release

function git(root, args, opts = {}) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: [opts.input == null ? "ignore" : "pipe", "pipe", "pipe"], ...opts });
}

/** Set the version in ui/package.json and ui/package-lock.json (only the package's own fields). */
export function setVersion(root, version) {
  const { pkg, lock } = paths(root);
  const p = readFileSync(pkg, "utf8");
  writeFileSync(pkg, p.replace(/("version"\s*:\s*")[^"]*(")/, `$1${version}$2`));
  if (existsSync(lock)) {
    const l = readFileSync(lock, "utf8");
    // the root's "version" and that of packages[""]: each right after "name": "klangwelt-ui"
    writeFileSync(lock, l.replace(/("name"\s*:\s*"klangwelt-ui",\s*"version"\s*:\s*")[^"]*(")/g, `$1${version}$2`));
  }
}

/**
 * Make a release of the committed entries in changelog/unreleased/ at local time `at`:
 * changelog/releases/<date>-<time>.json, the fragments removed, CHANGELOG.md, the version
 * in ui/package.json (and the lock file); then (with `commit`) a commit of exactly
 * these paths and the tag v<version>. Entries other sessions haven't committed yet stay.
 * Returns { version, file, entries, tag }.
 */
export function release({ root = ROOT, at = new Date(), commit = true, trailers = [] } = {}) {
  const version = versionAt(at);
  const tag = `v${version}`;
  const file = releaseFileAt(at);
  const rel = `changelog/releases/${file}`;
  if (commit) {
    if (git(root, ["tag", "--list", tag]).trim()) throw new Error(`tag ${tag} exists already`);
    const status = git(root, ["status", "--porcelain", "--untracked-files=all", "--", ...RELEASE_PATHS]).split("\n").filter(Boolean);
    // others' entries and screenshots not committed yet ("?? changelog/unreleased/…") stay where they are
    const blocking = status.filter((l) => !/^\?\? changelog\/(?:unreleased\/[^/]+\.json|images\/[^/]+)$/.test(l));
    if (blocking.length) throw new Error(`uncommitted changes in files a release writes – commit or wait for them first:\n${blocking.join("\n")}`);
  }
  if (existsSync(join(paths(root).releases, file))) throw new Error(`${rel} exists already`);
  const committed = commit ? new Set(git(root, ["ls-files", "--", "changelog/unreleased"]).split("\n").filter(Boolean)) : null;
  const entries = loadUnreleased(root).filter((e) => !committed || committed.has(e.rel));
  if (!entries.length) throw new Error("no committed entries in changelog/unreleased/ – nothing to release");
  const problems = entries.flatMap((e) => e.problems);
  if (commit) {
    // the release notes point at the screenshots in the tagged commit
    const images = new Set(git(root, ["ls-files", "--", "changelog/images"]).split("\n").filter(Boolean));
    for (const e of entries) if (e.image && !images.has(`changelog/images/${e.image}`)) problems.push(`${e.rel}: changelog/images/${e.image} is not committed`);
  }
  if (problems.length) throw new Error(`faulty entries:\n${problems.join("\n")}`);

  const data = {
    version,
    date: isoAt(at),
    entries: entries.map(({ id, type, platform, title, text, image, dev }) => ({
      id,
      type,
      ...(platform && platform !== "all" ? { platform } : {}),
      title,
      text,
      ...(image ? { image } : {}),
      ...(dev ? { dev } : {}),
    })),
  };
  mkdirSync(paths(root).releases, { recursive: true });
  writeFileSync(join(paths(root).releases, file), `${JSON.stringify(data, null, 2)}\n`);
  writeFileSync(paths(root).markdown, markdown(loadReleases(root)));
  setVersion(root, version);
  if (!commit) {
    for (const e of entries) rmSync(e.file);
    return { version, file: rel, entries: entries.length, tag: null };
  }
  git(root, ["rm", "--quiet", "--", ...entries.map((e) => e.rel)]);
  git(root, ["add", "--", rel, "CHANGELOG.md"]);
  const counts = TYPES.map((t) => [t, entries.filter((e) => e.type === t).length]).filter(([, n]) => n).map(([t, n]) => `${n} ${HEADINGS[t].toLowerCase()}`);
  const message = [`Release ${version}`, "", `${counts.join(", ")} – see CHANGELOG.md.`, ...(trailers.length ? ["", ...trailers] : [])].join("\n");
  git(root, ["commit", "--quiet", "--only", "-F", "-", "--", "CHANGELOG.md", rel, "ui/package.json", ...(existsSync(paths(root).lock) ? ["ui/package-lock.json"] : []), ...entries.map((e) => e.rel)], { input: message });
  git(root, ["tag", "-a", tag, "-m", `Klangwelt ${version}`]);
  return { version, file: rel, entries: entries.length, tag };
}

// ------------------------------------------------------------------ Command line

function option(args, name) {
  const a = args.find((x) => x.startsWith(`--${name}=`));
  return a ? a.slice(name.length + 3) : undefined;
}

function main(args) {
  const cmd = args.find((a) => !a.startsWith("--")) ?? "check";
  if (cmd === "check") {
    const problems = check();
    const n = loadUnreleased().length;
    if (problems.length) {
      console.error(problems.join("\n"));
      process.exit(1);
    }
    console.log(`changelog in order (${loadReleases().length} releases, ${n} unreleased ${n === 1 ? "entry" : "entries"})`);
  } else if (cmd === "build") {
    build();
    console.log("changelog → dist/changelog/");
  } else if (cmd === "markdown") {
    writeFileSync(paths(ROOT).markdown, markdown(loadReleases()));
    console.log("CHANGELOG.md rewritten");
  } else if (cmd === "notes") {
    const v = args.filter((a) => !a.startsWith("--"))[1];
    const r = loadReleases().find((x) => x.version === v);
    if (!r) throw new Error(`no release ${v} in changelog/releases/`);
    process.stdout.write(notes(r, option(args, "platform") ?? "all"));
  } else if (cmd === "release") {
    const at = option(args, "at") ? parseStamp(option(args, "at")) : new Date();
    const trailers = args.filter((a) => a.startsWith("--trailer=")).map((a) => a.slice(10));
    const r = release({ at, trailers, commit: !args.includes("--no-commit") });
    console.log(`Release ${r.version}: ${r.entries} entries → ${r.file}, CHANGELOG.md, ui/package.json`);
    if (r.tag) {
      console.log(`Committed and tagged ${r.tag} locally – nothing was pushed. Publishing: changelog/README.md.`);
    }
  } else {
    throw new Error(`unknown command "${cmd}" – check, build, markdown, notes <version>, release`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`changelog: ${e.message}`);
    process.exit(1);
  }
}
