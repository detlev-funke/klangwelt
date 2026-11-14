// Key list of the translations: collects every text from `t("…")`,
// `plural(n, "…", "…")` and `tk("…")` in ui/src, and from `tr!("…")`,
// `trn!(n, "…", "…")` and `tk!("…")` in the Rust core (crates/). Node only.
//
//   node scripts/i18n-keys.mjs                 summary: keys, missing per language
//   node scripts/i18n-keys.mjs --json=file     keys with their locations as JSON
//   node scripts/i18n-keys.mjs --missing=fr [--out=file]   keys a language lacks, as JSON
//   node scripts/i18n-keys.mjs --merge=fr --from=a.json,b.json [--prune]
//                                              merge translations ({ key: text }) into
//                                              ui/src/i18n/fr.ts (--prune: drop keys that
//                                              no longer occur in the sources)
//   node scripts/i18n-keys.mjs --root=path     more sources (e.g. the desktop shell,
//                                              ../klangwelt-desktop/app/src)
//
// New language: an entry in LANGUAGES (ui/src/i18n.ts), then --missing=xx --out=todo.json,
// translate, --merge=xx --from=done.json.
//
// Dictionaries: ui/src/i18n/<code>.ts (read directly, no build needed).
// Also usable as a module (tests/unit/i18n.test.mjs): `collectKeys`, `checkDict`, `loadDict`.

import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const UI = join(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = join(UI, "..");

/** Default sources: the UI (without the dictionaries) and the core (without klang-i18n). */
export const DEFAULT_ROOTS = [join(UI, "src"), join(REPO, "crates")];
const SKIP = [join(UI, "src", "i18n"), join(UI, "src", "i18n.ts"), join(REPO, "crates", "klang-i18n")];

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (SKIP.some((s) => p === s) || name === "target" || name === "node_modules") continue;
    if (statSync(p).isDirectory()) walk(p, out);
    else if (/\.(ts|rs)$/.test(name) && !name.endsWith(".d.ts")) out.push(p);
  }
  return out;
}

// ---------------------------------------------------------------- scanner
// A small tokenizer: skips comments, reads strings (JS: "…" '…' `…` with ${…};
// Rust: "…", r#"…"#, b"…", chars and lifetimes) and reports calls of the
// translation functions with their string arguments.

const JS_CALLS = Object.assign(Object.create(null), { t: 1, tk: 1, tid: 1, plural: 2 });
const RS_CALLS = Object.assign(Object.create(null), { "tr!": 1, "tk!": 1, "tid!": 1, "trn!": 2 });

function unescape(raw, rust) {
  let s = "";
  for (let i = 0; i < raw.length; i++) {
    const c = raw[i];
    if (c !== "\\") {
      s += c;
      continue;
    }
    const d = raw[++i];
    if (d === "n") s += "\n";
    else if (d === "t") s += "\t";
    else if (d === "r") s += "\r";
    else if (d === "0") s += "\0";
    else if (d === "u") {
      if (raw[i + 1] === "{") {
        const end = raw.indexOf("}", i);
        s += String.fromCodePoint(parseInt(raw.slice(i + 2, end).replaceAll("_", ""), 16));
        i = end;
      } else {
        s += String.fromCharCode(parseInt(raw.slice(i + 1, i + 5), 16));
        i += 4;
      }
    } else if (d === "x") {
      s += String.fromCharCode(parseInt(raw.slice(i + 1, i + 3), 16));
      i += 2;
    } else if (d === "\n" || d === "\r") {
      // line continuation: in Rust the next line's indentation is skipped, too
      if (d === "\r" && raw[i + 1] === "\n") i++;
      if (rust) while (/\s/.test(raw[i + 1] ?? "")) i++;
    } else s += d;
  }
  return s;
}

function scan(src, rust) {
  const calls = [];
  const n = src.length;

  // read a string literal at `at` → { value, end, plain } or null (not a literal)
  const literal = (at) => {
    let j = at;
    if (rust) {
      const m = /^b?r(#*)"/.exec(src.slice(j, j + 40));
      if (m) {
        const close = `"${m[1]}`;
        const start = j + m[0].length;
        const end = src.indexOf(close, start);
        return { value: src.slice(start, end), start: at, end: end + close.length, plain: true };
      }
      if (src[j] === "b" && src[j + 1] === '"') j++;
    }
    const q = src[j];
    if (q !== '"' && !(!rust && (q === "'" || q === "`"))) return null;
    let k = j + 1;
    let raw = "";
    let plain = true;
    while (k < n && src[k] !== q) {
      if (src[k] === "\\") {
        raw += src[k] + src[k + 1];
        k += 2;
        continue;
      }
      if (q === "`" && src[k] === "$" && src[k + 1] === "{") {
        plain = false;
        k = code(k + 2, "}");
        raw += "${…}";
        continue;
      }
      raw += src[k++];
    }
    return { value: unescape(raw, rust), start: at, end: k + 1, plain };
  };

  const skipSpace = (j) => {
    for (;;) {
      while (j < n && /\s/.test(src[j])) j++;
      if (src.startsWith("//", j)) j = src.indexOf("\n", j) === -1 ? n : src.indexOf("\n", j);
      else if (src.startsWith("/*", j)) j = src.indexOf("*/", j + 2) + 2;
      else return j;
    }
  };

  // arguments of a call: the first `count` string literals (for plural/trn!
  // after the first argument)
  const args = (j, name, count) => {
    const strs = [];
    j = skipSpace(j);
    if (count === 2) {
      // skip the first argument (the count) up to the top-level comma
      let depth = 0;
      while (j < n) {
        const c = src[j];
        if ("([{".includes(c)) depth++;
        else if (")]}".includes(c)) {
          if (depth === 0) return { strs, end: j, names: [], dynamic: false };
          depth--;
        } else if (c === "," && depth === 0) break;
        else if (c === '"' || (!rust && (c === "'" || c === "`"))) {
          j = literal(j).end;
          continue;
        }
        j++;
      }
      j = skipSpace(j + 1);
    }
    for (let a = 0; a < count; a++) {
      const lit = literal(j);
      if (!lit) break;
      strs.push(lit);
      j = skipSpace(lit.end);
      if (src[j] === ",") j = skipSpace(j + 1);
    }
    return { strs, end: j, ...varNames(j) };
  };

  // names of the placeholders passed: JS `{ a, b: x }`, Rust `a = x, b = y`.
  // `dynamic`: values passed as a variable or with `...` – then it's unknown which.
  function varNames(j) {
    const names = [];
    let dynamic = false;
    let depth = 0;
    let start = true; // at the start of an entry
    if (src[j] === ")") return { names, dynamic };
    if (!rust && src[j] !== "{") return { names, dynamic: true };
    const top = rust ? 0 : 1;
    while (j < n) {
      const c = src[j];
      if (c === '"' || (!rust && (c === "'" || c === "`"))) {
        j = literal(j).end;
        start = false;
        continue;
      }
      if ("([{".includes(c)) {
        depth++;
        j++;
        if (depth === top) start = true;
        continue;
      }
      if (")]}".includes(c)) {
        if (depth === 0) break;
        depth--;
        j++;
        continue;
      }
      if (c === "," && depth === top) {
        start = true;
        j++;
        continue;
      }
      if (start && depth === top && !/\s/.test(c)) {
        start = false;
        if (src.startsWith("...", j)) dynamic = true;
        const m = /^([A-Za-z_$][\w$]*)\s*(=(?!=)|:|,|\})?/.exec(src.slice(j, j + 60));
        if (m && (rust ? m[2] === "=" : m[2] !== undefined)) names.push(m[1]);
        else if (rust) dynamic = true;
      }
      j++;
    }
    return { names, dynamic };
  }

  // read code up to the closing character (for ${…}); returns the position after it
  function code(from, closer) {
    let depth = 0;
    let j = from;
    while (j < n) {
      const c = src[j];
      if (src.startsWith("//", j)) {
        j = src.indexOf("\n", j);
        if (j === -1) return n;
        continue;
      }
      if (src.startsWith("/*", j)) {
        j = src.indexOf("*/", j + 2) + 2;
        continue;
      }
      if (c === '"' || (!rust && (c === "'" || c === "`")) || (rust && /^b?r#*"|^b"/.test(src.slice(j, j + 4)) && !/[\w]/.test(src[j - 1] ?? ""))) {
        const lit = literal(j);
        if (lit) {
          j = lit.end;
          continue;
        }
      }
      if (!rust && c === "/" && src[j + 1] !== "/" && src[j + 1] !== "*") {
        // a regex literal (not a division) when an operator or the start comes before it
        let p = j - 1;
        while (p >= 0 && /\s/.test(src[p])) p--;
        const before = src.slice(Math.max(0, p - 5), p + 1);
        if (p < 0 || /[(,=:[!&|?{};+\-*%<>~^]$/.test(before) || /\b(return|typeof|case|of|in)$/.test(before)) {
          let k = j + 1;
          let cls = false;
          while (k < n && src[k] !== "\n") {
            if (src[k] === "\\") k++;
            else if (src[k] === "[") cls = true;
            else if (src[k] === "]") cls = false;
            else if (src[k] === "/" && !cls) break;
            k++;
          }
          j = k + 1;
          continue;
        }
      }
      if (rust && c === "'") {
        // a char ('a', '\n', '\u{1f}') or a lifetime ('a)
        const m = /^'(\\u\{[0-9a-fA-F_]+\}|\\.|[^\\'])'/u.exec(src.slice(j, j + 14));
        j += m ? m[0].length : 1;
        continue;
      }
      if (closer && depth === 0 && c === closer) return j + 1;
      if (c === "{") depth++;
      else if (c === "}") depth--;
      const m = /^[A-Za-z_$][\w$]*!?/.exec(src.slice(j, j + 12));
      if (m && !/[\w$.]/.test(src[j - 1] ?? "")) {
        const word = m[0];
        const after = j + word.length;
        const table = rust ? RS_CALLS : JS_CALLS;
        if (table[word] && src[skipSpace(after)] === "(") {
          const { strs, end, names, dynamic } = args(skipSpace(after) + 1, word, table[word]);
          const line = src.slice(0, j).split("\n").length;
          calls.push({ fn: word, strs, line, names, dynamic });
          j = Math.max(end, after);
          continue;
        }
        j = after;
        continue;
      }
      j++;
    }
    return n;
  }

  code(0, null);
  return calls;
}

/** Calls of the translation functions in one file, with the positions of their
 *  string literals (`strs[i].start`/`.end`) – for tools that rewrite keys. */
export function scanFile(file) {
  return scan(readFileSync(file, "utf8"), file.endsWith(".rs"));
}

/** The `{name}` placeholders of a text. */
export function placeholders(text) {
  return [...new Set([...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]))].sort();
}

/** All keys of the sources: Map key → { key, one?, files, plural, names, dynamic }. */
export function collectKeys(roots = DEFAULT_ROOTS) {
  const keys = new Map();
  const problems = [];
  for (const root of roots) {
    if (!existsSync(root)) continue;
    for (const file of walk(root)) {
      const rust = file.endsWith(".rs");
      const rel = relative(REPO, file).replaceAll("\\", "/");
      for (const c of scan(readFileSync(file, "utf8"), rust)) {
        const want = (rust ? RS_CALLS : JS_CALLS)[c.fn];
        if (c.strs.length < want || c.strs.some((s) => !s.plain)) {
          // t(variable) is allowed (the values are marked with tk); incomplete plural/trn! is not
          if (want === 2 || c.strs.some((s) => !s.plain)) problems.push(`${rel}:${c.line}: ${c.fn}(…) without a string literal`);
          continue;
        }
        const key = want === 2 ? c.strs[1].value : c.strs[0].value;
        const e = keys.get(key) ?? { key, files: new Set(), plural: false, names: new Set(), dynamic: false, id: false };
        if (c.fn === "tid" || c.fn === "tid!") e.id = true;
        for (const v of c.names ?? []) e.names.add(v);
        if (c.dynamic) e.dynamic = true;
        if (want === 2) {
          e.plural = true;
          e.one = c.strs[0].value;
        }
        e.files.add(`${rel}:${c.line}`);
        keys.set(key, e);
      }
    }
  }
  return { keys, problems };
}

/** Check a dictionary: missing and unknown keys, foreign placeholders. */
export function checkDict(dict, keys) {
  const missing = [];
  const unused = [];
  const badVars = [];
  for (const [key, e] of keys) {
    if (!(key in dict)) missing.push(key);
    else {
      const allowed = new Set([...placeholders(key), ...(e.one ? placeholders(e.one) : []), ...(e.plural ? ["n"] : []), ...(e.names ?? [])]);
      const entry = dict[key];
      const texts = typeof entry === "string" ? [entry] : Object.values(entry);
      for (const text of texts) {
        const foreign = placeholders(text).filter((p) => !allowed.has(p));
        if (foreign.length && !e.dynamic) badVars.push(`${key} → ${text} (unknown: ${foreign.join(", ")})`);
      }
      if (typeof entry !== "string" && typeof entry?.other !== "string") badVars.push(`${key}: plural without "other"`);
    }
  }
  for (const key of Object.keys(dict)) if (!keys.has(key)) unused.push(key);
  return { missing, unused, badVars };
}

/** The keys a language's dictionary must have: English only the identifiers
 *  (tid – their value is German), German everything except them, every other
 *  language all keys. */
export function keysFor(code, keys) {
  return new Map([...keys].filter(([, e]) => (code === "en" ? e.id : code === "de" ? !e.id : true)));
}

/** Load a dictionary – straight from ui/src/i18n/<code>.ts (a plain object
 *  literal), without a build. Missing file: empty. */
export async function loadDict(code) {
  const file = join(UI, "src", "i18n", `${code}.ts`);
  if (!existsSync(file)) return {};
  const js = readFileSync(file, "utf8")
    .replace(/^import type .*$/m, "")
    .replace(/const dict: Dict =/, "const dict =");
  return (await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`)).default;
}

const LANG_NAMES = { de: "German", en: "English", fr: "French", it: "Italian", es: "Spanish", nl: "Dutch", pl: "Polish" };

/** Write a dictionary file: entries in the order of their locations, grouped by
 *  file; entries without a location here (e.g. for the desktop shell) at the end. */
export function writeDict(code, dict, keys) {
  const lines = [];
  const q = (s) => JSON.stringify(s);
  const entry = (key, v) =>
    typeof v === "string"
      ? `  ${q(key)}: ${q(v)},`
      : `  ${q(key)}: { ${Object.entries(v).map(([c, s]) => `${c}: ${q(s)}`).join(", ")} },`;
  let lastFile = null;
  for (const [key, e] of keys) {
    if (!(key in dict)) continue;
    const file = [...e.files][0].replace(/:\d+$/, "");
    if (file !== lastFile) {
      lines.push(`${lastFile ? "\n" : ""}  // ${file}`);
      lastFile = file;
    }
    lines.push(entry(key, dict[key]));
  }
  const rest = Object.keys(dict).filter((k) => !keys.has(k));
  if (rest.length) {
    lines.push("\n  // not used in this repo (e.g. the desktop shell, klangwelt-desktop/app/src)");
    for (const k of rest) lines.push(entry(k, dict[k]));
  }
  const name = LANG_NAMES[code] ?? code;
  const what =
    code === "en"
      ? `// English dictionary (en): only identifiers marked with tid(…)/tid!(…) – their value is
// German and must not change (see ../i18n.ts). All other English texts are the keys in the code.`
      : `// ${name} dictionary (${code}) – keys are the English texts in the code (see ../i18n.ts).`;
  const src = `${what}
// Generated/extended with ui/scripts/i18n-keys.mjs --merge=${code}; editing by hand is fine.
import type { Dict } from "../i18n.js";

const dict: Dict = {
${lines.join("\n")}
};

export default dict;
`;
  writeFileSync(join(UI, "src", "i18n", `${code}.ts`), src);
}

async function main() {
  const opt = Object.fromEntries(process.argv.slice(2).map((a) => { const [k, ...v] = a.replace(/^--/, "").split("="); return [k, v.length ? v.join("=") : true]; }));
  const roots = [...DEFAULT_ROOTS, ...(opt.root ? opt.root.split(",") : [])];
  const { keys, problems } = collectKeys(roots);
  const asJson = (list) =>
    list.map((k) => {
      const e = keys.get(k);
      const more = [...e.names].filter((v) => !placeholders(k).includes(v)).sort();
      return { key: k, ...(e.plural ? { plural: true, one: e.one } : {}), vars: placeholders(k), ...(more.length ? { more } : {}), files: [...e.files] };
    });
  if (opt.json) {
    writeFileSync(opt.json, JSON.stringify(asJson([...keys.keys()]), null, 1));
    console.log(`${keys.size} keys → ${opt.json}`);
    return;
  }
  if (opt.missing) {
    const { missing } = checkDict(await loadDict(opt.missing), keysFor(opt.missing, keys));
    const text = JSON.stringify(asJson(missing), null, 1);
    if (opt.out) {
      writeFileSync(opt.out, text);
      console.log(`${missing.length} missing keys (${opt.missing}) → ${opt.out}`);
    } else console.log(text);
    return;
  }
  if (opt.merge) {
    // merge translations (JSON objects key → text or plural object)
    const dict = await loadDict(opt.merge);
    let added = 0;
    for (const f of (opt.from ?? "").split(",").filter(Boolean)) {
      for (const [k, v] of Object.entries(JSON.parse(readFileSync(f, "utf8")))) {
        if (!(k in dict)) added++;
        dict[k] = v;
      }
    }
    if (opt.prune) for (const k of Object.keys(dict)) if (!keys.has(k)) delete dict[k];
    writeDict(opt.merge, dict, keys);
    const { missing, badVars } = checkDict(dict, keysFor(opt.merge, keys));
    console.log(`${opt.merge}: ${added} new, ${Object.keys(dict).length} total, ${missing.length} missing, ${badVars.length} with foreign placeholders`);
    for (const b of badVars) console.log(`  ! ${b}`);
    return;
  }
  console.log(`${keys.size} keys (${[...keys.values()].filter((e) => e.plural).length} with plural forms)`);
  for (const p of problems) console.log(`  ! ${p}`);
  // languages from ui/src/i18n.ts (LANGUAGES), without a build
  const src = readFileSync(join(UI, "src", "i18n.ts"), "utf8");
  const codes = [...src.matchAll(/\{ code: "(\w+)", name: /g)].map((m) => m[1]);
  for (const code of codes) {
    const { missing, unused, badVars } = checkDict(await loadDict(code), keysFor(code, keys));
    console.log(`${code}: ${missing.length} missing, ${unused.length} unused, ${badVars.length} with foreign placeholders`);
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) await main();
