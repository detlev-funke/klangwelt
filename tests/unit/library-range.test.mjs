// Score library over HTTP: the development server (ui/scripts/serve.mjs
// --library) answers ranges like Zenodo – one range 206, several as
// multipart/byteranges, CORS – and the browser's parser (ui/src/web/remote.ts)
// reads such answers, also Zenodo's (LF line ends, no multipart content type).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { fingerprint, groups, parseMultipart } from "../../ui/dist/web/remote.js";

const SERVE = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "ui", "scripts", "serve.mjs");
const dir = mkdtempSync(join(tmpdir(), "klangwelt-range-"));
const data = Buffer.from(Array.from({ length: 1000 }, (_, i) => i % 251));
writeFileSync(join(dir, "klangwelt-index.kwix"), data);
const servers = [];

const freePort = () =>
  new Promise((res) => {
    const s = createServer();
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => res(port));
    });
  });

async function serve(...args) {
  const port = await freePort();
  const p = spawn(process.execPath, [SERVE, "--web", `--port=${port}`, `--library=${dir}`, ...args], { stdio: ["ignore", "pipe", "inherit"] });
  servers.push(p);
  await new Promise((res, rej) => {
    p.stdout.on("data", (d) => String(d).includes("http://") && res());
    p.on("exit", (c) => rej(new Error(`server exited (${c})`)));
  });
  return `http://127.0.0.1:${port}`;
}

let base;
before(async () => {
  base = await serve();
});
after(() => {
  for (const s of servers) s.kill();
  rmSync(dir, { recursive: true, force: true });
});

const get = (path, headers = {}) => fetch(`${base}${path}`, { headers });

test("one range: 206 with Content-Range, CORS; suffix ranges; 416 outside the file", async () => {
  const r = await get("/library/klangwelt-index.kwix", { Range: "bytes=10-19", Origin: "https://example.org" });
  assert.equal(r.status, 206);
  assert.equal(r.headers.get("content-range"), "bytes 10-19/1000");
  assert.equal(r.headers.get("access-control-allow-origin"), "*");
  assert.deepEqual(new Uint8Array(await r.arrayBuffer()), new Uint8Array(data.subarray(10, 20)));
  const tail = await get("/library/klangwelt-index.kwix", { Range: "bytes=-4" });
  assert.deepEqual(new Uint8Array(await tail.arrayBuffer()), new Uint8Array(data.subarray(996)));
  const open = await get("/library/klangwelt-index.kwix", { Range: "bytes=990-" });
  assert.equal((await open.arrayBuffer()).byteLength, 10);
  const bad = await get("/library/klangwelt-index.kwix", { Range: "bytes=5000-6000" });
  assert.equal(bad.status, 416);
  const whole = await get("/library/klangwelt-index.kwix");
  assert.equal(whole.status, 200);
  assert.equal((await whole.arrayBuffer()).byteLength, 1000);
  const pre = await fetch(`${base}/library/klangwelt-index.kwix`, { method: "OPTIONS" });
  assert.equal(pre.status, 204);
  assert.match(pre.headers.get("access-control-allow-headers") ?? "", /range/);
  assert.deepEqual(await (await get("/library.json")).json(), { baseUrl: "library/" });
});

test("several ranges: multipart/byteranges, parsed by the browser code", async () => {
  const r = await get("/library/klangwelt-index.kwix", { Range: "bytes=0-3,100-109,990-999" });
  assert.equal(r.status, 206);
  assert.match(r.headers.get("content-type"), /^multipart\/byteranges; boundary=/);
  const parts = parseMultipart(new Uint8Array(await r.arrayBuffer()));
  assert.deepEqual(parts.map(([start, b]) => [start, b.length]), [[0, 4], [100, 10], [990, 10]]);
  assert.deepEqual(parts[1][1], new Uint8Array(data.subarray(100, 110)));
});

test("a server without multi-range sends the whole file (the browser then asks range by range)", async () => {
  const plain = await serve("--no-multirange");
  const r = await fetch(`${plain}/library/klangwelt-index.kwix`, { headers: { Range: "bytes=0-3,100-103" } });
  assert.equal(r.status, 200);
  const one = await fetch(`${plain}/library/klangwelt-index.kwix`, { headers: { Range: "bytes=0-3" } });
  assert.equal(one.status, 206);
});

test("Zenodo's multi-range answer: LF line ends, boundary only in the body", () => {
  const enc = new TextEncoder();
  const body = enc.encode(
    "\n--EOSMULTIPARTBOUNDARY\nContent-Type: application/octet-stream\nContent-Range: bytes 0-3/225399738\n\npath\n--EOSMULTIPARTBOUNDARY\nContent-Type: application/octet-stream\nContent-Range: bytes 100-103/225399738\n\ns_of\n--EOSMULTIPARTBOUNDARY--\n",
  );
  const parts = parseMultipart(body);
  assert.deepEqual(parts.map(([s, b]) => [s, new TextDecoder().decode(b)]), [[0, "path"], [100, "s_of"]]);
  // binary data that looks like a boundary doesn't confuse it (lengths come from Content-Range)
  const tricky = enc.encode("--B\r\nContent-Range: bytes 5-12/99\r\n\r\n\n--B\r\nxy\r\n--B--\r\n");
  assert.deepEqual(parseMultipart(tricky).map(([s, b]) => [s, b.length]), [[5, 8]]);
  assert.equal(parseMultipart(enc.encode("no multipart")), null);
});

test("ranges go out in groups of at most 250 per request, with a Range header of at most 6000 characters", () => {
  const near = Array.from({ length: 600 }, (_, i) => [i * 10_000, i * 10_000 + 100]);
  const g = groups(near);
  assert.deepEqual(g.map((x) => x.length), [250, 250, 100]);
  assert.deepEqual(g.flat(), near, "all spans, in order");
  // long offsets (a big file): the header limit cuts earlier
  const far = Array.from({ length: 600 }, (_, i) => [1_000_000_000 + i * 10_000, 1_000_000_000 + i * 10_000 + 100]);
  for (const x of groups(far)) assert.ok(x.map(([a, b]) => `${a}-${b - 1}`).join(",").length <= 6000);
  assert.deepEqual(groups([]), []);
});

test("a package is recognised by its files' checksums (another package at the same address empties the cache)", () => {
  const a = { version: "1", files: { index: { name: "i.kwix", size: 10, sha256: "aa" }, text: { name: "t.bin", size: 5, sha256: "bb" } } };
  const same = { version: "1", files: { text: { name: "t.bin", size: 5, sha256: "bb" }, index: { name: "i.kwix", size: 10, sha256: "aa" } } };
  const rebuilt = { version: "1", files: { index: { name: "i.kwix", size: 10, sha256: "cc" }, text: { name: "t.bin", size: 5, sha256: "bb" } } };
  assert.equal(fingerprint(a), fingerprint(same));
  assert.notEqual(fingerprint(a), fingerprint(rebuilt));
  assert.equal(fingerprint(null), fingerprint({}));
});
