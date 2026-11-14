// Entry point of the UI: first the language – load the dictionary and hand it
// to the core – then the app. So every module may call `t()` while loading,
// also in tables, and every answer of the core is already in that language
// (panels ask the core for templates and specs while the layout is built).

import { coreDictionary, initI18n, language } from "./i18n.js";

await initI18n();
const { backend } = await import("./bridge.js");
await backend.setLanguage(language(), coreDictionary()).catch((e) => console.warn("set_language:", e));
await import("./app.js");
