// Mini-Webserver für die Oberfläche im Browser – nur statische Dateien.
//
//   npm run serve       → ui/dist mit der Attrappe (ohne Audio, für UI-Arbeit)
//   npm run serve:web   → ui/dist-web, die Browser-Version mit echtem Kern
//                          (vorher `npm run build:web`)
//
// Port: --port=… oder PORT (Standard 1420). Kein COOP/COEP nötig.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const web = process.argv.includes("--web");
const dist = join(dirname(fileURLToPath(import.meta.url)), "..", web ? "dist-web" : "dist");
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".map": "application/json",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
  ".json": "application/json",
};
const portArg = process.argv.find((a) => a.startsWith("--port="));
const port = Number(portArg?.slice(7) ?? process.env.PORT ?? 1420);

createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const rel = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, "") || "index.html";
  const file = join(dist, rel);
  if (!file.startsWith(dist)) {
    res.writeHead(403).end();
    return;
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream", "cache-control": "no-store" });
    res.end(body);
  } catch {
    res.writeHead(404).end("nicht gefunden");
  }
}).listen(port, "127.0.0.1", () => console.log(web ? `Klangwelt im Browser: http://localhost:${port}/` : `Klangwelt-UI (Attrappe): http://localhost:${port}/?attrappe`));
