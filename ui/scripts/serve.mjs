// Mini web server for the UI in the browser – static files only.
//
//   npm run serve       → ui/dist with the mock (no audio, for UI work)
//   npm run serve:web   → ui/dist-web, the browser version with the real core
//                          (run `npm run build:web` first)
//
// Port: --port=… or PORT (default 1420). No COOP/COEP needed.
//
// Score library: `--library=DIR` serves a package folder (built with
// klang-libbuild, see docs/LIBRARY.md) under /library/ and answers
// /library.json so that the app reads it from there instead of Zenodo.
// Range requests (one range: 206; several: multipart/byteranges) and CORS
// work like on Zenodo; `--no-multirange` answers several ranges with the
// whole file, like GitHub Pages (to test the fallback). `--no-library` answers
// /library.json with no package, so the app reads nothing from Zenodo (tests).
// `--dist=DIR` serves another folder instead of ui/dist(-web), e.g. a copy a test
// changes to stand for a new version.
import { createServer } from "node:http";
import { open, readFile, stat } from "node:fs/promises";
import { extname, join, normalize, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const web = process.argv.includes("--web");
const distArg = process.argv.find((a) => a.startsWith("--dist="))?.slice(7);
const dist = distArg ? resolve(distArg) : join(dirname(fileURLToPath(import.meta.url)), "..", web ? "dist-web" : "dist");
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".map": "application/json",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
  ".png": "image/png",
};
const arg = (name) => process.argv.find((a) => a.startsWith(`--${name}=`))?.slice(name.length + 3);
const port = Number(arg("port") ?? process.env.PORT ?? 1420);
const library = arg("library") ? resolve(arg("library")) : null;
const multiRange = !process.argv.includes("--no-multirange");
const noLibrary = process.argv.includes("--no-library");

/** Parses a Range header against a file length: list of [start, end] (inclusive), null = ignore, "bad" = 416. */
export function parseRange(header, size) {
  const m = /^bytes=(.+)$/.exec(header ?? "");
  if (!m) return null;
  const out = [];
  for (const part of m[1].split(",")) {
    const r = /^\s*(\d*)-(\d*)\s*$/.exec(part);
    if (!r || (r[1] === "" && r[2] === "")) return null;
    let start;
    let end;
    if (r[1] === "") {
      start = Math.max(0, size - Number(r[2]));
      end = size - 1;
    } else {
      start = Number(r[1]);
      end = r[2] === "" ? size - 1 : Math.min(Number(r[2]), size - 1);
    }
    if (start > end || start >= size) continue;
    out.push([start, end]);
  }
  return out.length ? out : "bad";
}

const cors = {
  "access-control-allow-origin": "*",
  "access-control-allow-headers": "range",
  "access-control-expose-headers": "content-range, content-length, accept-ranges",
};

/** A file with Range support. */
async function sendFile(req, res, file, type, extra = {}) {
  const info = await stat(file);
  if (!info.isFile()) throw new Error("no file");
  const size = info.size;
  const base = { "content-type": type, "accept-ranges": "bytes", "cache-control": "no-store", ...extra };
  const ranges = req.headers.range ? parseRange(req.headers.range, size) : null;
  if (ranges === "bad") {
    res.writeHead(416, { ...base, "content-range": `bytes */${size}` }).end();
    return;
  }
  const fh = await open(file, "r");
  try {
    const read = async (start, end) => {
      const buf = Buffer.alloc(end - start + 1);
      await fh.read(buf, 0, buf.length, start);
      return buf;
    };
    if (!ranges || (ranges.length > 1 && !multiRange)) {
      res.writeHead(200, { ...base, "content-length": size });
      res.end(req.method === "HEAD" ? undefined : await readFile(file));
      return;
    }
    if (ranges.length === 1) {
      const [start, end] = ranges[0];
      res.writeHead(206, { ...base, "content-range": `bytes ${start}-${end}/${size}`, "content-length": end - start + 1 });
      res.end(req.method === "HEAD" ? undefined : await read(start, end));
      return;
    }
    const boundary = `klangwelt${Date.now().toString(16)}`;
    const parts = [];
    for (const [start, end] of ranges) {
      parts.push(Buffer.from(`\r\n--${boundary}\r\nContent-Type: ${type}\r\nContent-Range: bytes ${start}-${end}/${size}\r\n\r\n`));
      parts.push(await read(start, end));
    }
    parts.push(Buffer.from(`\r\n--${boundary}--\r\n`));
    const body = Buffer.concat(parts);
    res.writeHead(206, { ...base, "content-type": `multipart/byteranges; boundary=${boundary}`, "content-length": body.length });
    res.end(req.method === "HEAD" ? undefined : body);
  } finally {
    await fh.close();
  }
}

createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  if (req.method === "OPTIONS") {
    res.writeHead(204, { ...cors, "access-control-allow-methods": "GET, HEAD, OPTIONS" }).end();
    return;
  }
  // the score library package (docs/LIBRARY.md)
  if ((library || noLibrary) && url.pathname === "/library.json") {
    res.writeHead(200, { "content-type": types[".json"], "cache-control": "no-store" }).end(JSON.stringify(library ? { baseUrl: "library/" } : {}));
    return;
  }
  let root = dist;
  let path = url.pathname;
  if (library && path.startsWith("/library/")) {
    root = library;
    path = path.slice("/library".length);
  }
  const rel = normalize(decodeURIComponent(path)).replace(/^([/\\])+/, "") || "index.html";
  const file = join(root, rel);
  if (!file.startsWith(root)) {
    res.writeHead(403).end();
    return;
  }
  try {
    await sendFile(req, res, file, types[extname(file)] ?? "application/octet-stream", root === library ? cors : {});
  } catch {
    if (!res.headersSent) res.writeHead(404).end("nicht gefunden");
  }
}).listen(port, "127.0.0.1", () =>
  console.log(
    web ? `Klangwelt im Browser: http://localhost:${port}/${library ? ` (library: ${library})` : ""}` : `Klangwelt-UI (Attrappe): http://localhost:${port}/?attrappe`,
  ),
);
