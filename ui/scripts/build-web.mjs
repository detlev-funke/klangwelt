// Browser-Version bauen: ui/dist-web/ – ein Ordner mit statischen Dateien,
// den jeder einfache HTTP-Server ausliefern kann (z. B. `npm run serve:web`).
// Nur Node-Bordmittel; WebAssembly mit cargo (Ziel wasm32-unknown-unknown).
//
//   npm run build:web   (baut vorher die Oberfläche mit tsc)
//
// Inhalt: die Oberfläche (wie für die Desktop-App), klang_web.wasm (Kern),
// klang_worklet.wasm (Mischer) und sw.js (Service Worker mit Dateiliste).
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const ui = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(ui, "..");
const dist = join(ui, "dist");
const out = join(ui, "dist-web");

if (!existsSync(join(dist, "main.js"))) throw new Error("ui/dist fehlt – erst `npm run build`");

console.log("WebAssembly bauen (klang-web, klang-worklet) …");
execFileSync("cargo", ["build", "--release", "--target", "wasm32-unknown-unknown", "-p", "klang-web", "-p", "klang-worklet"], { cwd: root, stdio: "inherit" });

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
cpSync(dist, out, { recursive: true, filter: (src) => !src.endsWith(".map") });
const wasmDir = join(root, "target", "wasm32-unknown-unknown", "release");
for (const f of ["klang_web.wasm", "klang_worklet.wasm"]) {
  // Beide Module dürfen nur importieren, was Worker und Worklet liefern
  // (Modul „klangwelt“, siehe web/worker.ts) – fehlt z. B. libopus eine
  // C-Funktion, landet sie sonst als Import „env.…“ in der Datei und der
  // Kern startet im Browser nicht.
  const foreign = WebAssembly.Module.imports(new WebAssembly.Module(readFileSync(join(wasmDir, f))))
    .filter((i) => i.module !== "klangwelt")
    .map((i) => `${i.module}.${i.name}`);
  if (foreign.length) throw new Error(`${f} importiert Unbekanntes: ${foreign.join(", ")}`);
  cpSync(join(wasmDir, f), join(out, f));
}

// Service Worker: Version = Prüfsumme aller Dateien, Liste = alles außer ihm selbst
const files = [];
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p);
    else files.push(relative(out, p).replaceAll("\\", "/"));
  }
};
walk(out);
const cached = files.filter((f) => f !== "web/sw.js").sort();
const hash = createHash("sha256");
for (const f of cached) hash.update(f).update(readFileSync(join(out, f)));
const version = hash.digest("hex").slice(0, 12);
const sw = readFileSync(join(out, "web", "sw.js"), "utf8")
  .replace("__KLANGWELT_VERSION__", version)
  .replace("__KLANGWELT_FILES__", JSON.stringify(["./", ...cached]));
writeFileSync(join(out, "sw.js"), sw);
rmSync(join(out, "web", "sw.js"));

const size = (f) => (statSync(join(out, f)).size / 1024 / 1024).toFixed(1);
console.log(`Browser-Version → ui/dist-web (Version ${version}; Kern ${size("klang_web.wasm")} MB, Mischer ${size("klang_worklet.wasm")} MB)`);
console.log("Ausliefern: npm run serve:web  (oder jeder andere statische Server; .wasm als application/wasm)");
