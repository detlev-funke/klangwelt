// Copies static files (HTML, CSS) to dist/. Node built-ins only.
import { cpSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
mkdirSync(join(root, "dist"), { recursive: true });
cpSync(join(root, "static"), join(root, "dist"), { recursive: true });
console.log("statische Dateien → dist/");
