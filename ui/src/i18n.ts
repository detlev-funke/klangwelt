// Internationalisation of the UI (and, through `set_language`, of the core).
//
// Keys are the English texts in the code: `t("Save")`. Every other language has
// a dictionary in `i18n/<code>.ts` (German included); at start only the chosen
// one is loaded, plus English as a fallback. Order: chosen language → English →
// the key itself.
//
// `i18n/en.ts` holds only identifiers: values marked with `tid("…")` that must
// keep their historical (German) value because they are persisted or sent to
// the core; they are translated for display in every language but German.
//
// New language: an entry in LANGUAGES, a flag in `flags.ts` and
// `i18n/<code>.ts` with the keys of `i18n/de.ts` plus the identifiers of
// `i18n/en.ts` (tests/unit/i18n.test.mjs shows what's missing).
//
// Placeholders: `t("Track {name}", { name })`. Plural: `plural(n, "one note",
// "{n} notes")` – the key is the plural, `{n}` is always set; in the dictionary
// the entry may then be an object with the forms of `Intl.PluralRules`
// (`{ one, few, many, other }`).
//
// `tk("…")` marks texts in tables as keys (returns them unchanged); they are
// translated when shown, with `t(value)`. `scripts/i18n-keys.mjs` builds the
// key list from all `t`/`plural`/`tk`/`tid` calls (and `tr!` etc. in the core).

export interface Language {
  /** BCP 47 code (language only), e.g. "en". */
  code: string;
  /** Name in the language itself – that's how it appears in the menu. */
  name: string;
}

export const LANGUAGES: readonly Language[] = [
  { code: "en", name: "English" },
  { code: "de", name: "Deutsch" },
  { code: "fr", name: "Français" },
  { code: "it", name: "Italiano" },
  { code: "es", name: "Español" },
  { code: "nl", name: "Nederlands" },
  { code: "pl", name: "Polski" },
];

/** Language of the keys (the texts in the code). */
export const SOURCE = "en";
/** Fallback when no preferred language is supported or an entry is missing. */
export const FALLBACK = "en";
/** Language of the identifiers marked with `tid` (their values are German). */
export const ID_LANGUAGE = "de";
/** localStorage key of the remembered choice. */
export const STORAGE_KEY = "klangwelt.language";

export type PluralEntry = { zero?: string; one?: string; two?: string; few?: string; many?: string; other: string };
export type Entry = string | PluralEntry;
export type Dict = Record<string, Entry>;
export type Vars = Record<string, string | number>;

let current = SOURCE;
let dict: Dict = {};
let fallback: Dict = {};
let rules: Intl.PluralRules | null = null;

/** Map a language tag to a supported language ("de-AT" → "de"), otherwise null. */
export function supported(tag: string | null | undefined): string | null {
  if (!tag) return null;
  const base = tag.trim().toLowerCase().split(/[-_]/)[0] ?? "";
  return LANGUAGES.some((l) => l.code === base) ? base : null;
}

/** First supported language of the list (like `navigator.languages`), otherwise English. */
export function detectLanguage(preferred: readonly string[]): string {
  for (const tag of preferred) {
    const code = supported(tag);
    if (code) return code;
  }
  return FALLBACK;
}

/** Remembered choice (null: none, or no storage). */
export function storedLanguage(): string | null {
  try {
    return supported(localStorage.getItem(STORAGE_KEY));
  } catch {
    return null;
  }
}

/** Remember the choice. It takes effect with the next load (see `switchLanguage`). */
export function storeLanguage(code: string): void {
  try {
    localStorage.setItem(STORAGE_KEY, code);
  } catch {
    /* no storage: the choice can't outlive a reload */
  }
}

/** Switch language: remember it and reload the UI (the browser asks first
 *  if unsaved changes would be lost). */
export function switchLanguage(code: string): void {
  if (!supported(code)) return;
  storeLanguage(code);
  location.reload();
}

/** Current language (code). */
export function language(): string {
  return current;
}

/** Fill `{name}` placeholders; unknown ones stay as they are. */
export function fill(text: string, vars?: Vars): string {
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (m, name: string) => (name in vars ? String(vars[name]) : m));
}

function pick(entry: Entry, n: number): string {
  if (typeof entry === "string") return entry;
  rules ??= new Intl.PluralRules(current);
  const cat = rules.select(n) as keyof PluralEntry;
  return entry[cat] ?? entry.other;
}

function lookup(key: string): Entry | undefined {
  return dict[key] ?? fallback[key];
}

/** Translate a text and fill placeholders. */
export function t(key: string, vars?: Vars): string {
  const entry = lookup(key);
  if (entry === undefined) return fill(key, vars);
  return fill(pick(entry, typeof vars?.n === "number" ? vars.n : 1), vars);
}

/** Text with a count: the key is the plural `other`; `{n}` is set. */
export function plural(n: number, one: string, other: string, vars?: Vars): string {
  const all = { n, ...vars };
  const entry = lookup(other);
  if (entry === undefined) return fill(n === 1 ? one : other, all);
  return fill(pick(entry, n), all);
}

/** Marks a text as a key; returns it unchanged. */
export function tk<T extends string>(key: T): T {
  return key;
}

/** Marks an identifier that is also shown (its value must not change, e.g. it is
 *  persisted or sent to the core); every dictionary, English included, translates
 *  it for display with `t(id)`. Returns it unchanged. */
export function tid<T extends string>(id: T): T {
  return id;
}

/** Dictionary for the core: flat (text → text), chosen language over English;
 *  plural forms as `key` (other) and `key\u001fone` etc. */
export function coreDictionary(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const source of [fallback, dict]) {
    for (const [key, entry] of Object.entries(source)) {
      if (typeof entry === "string") {
        out[key] = entry;
        continue;
      }
      out[key] = entry.other;
      for (const [cat, text] of Object.entries(entry)) if (cat !== "other" && text) out[`${key}\u001f${cat}`] = text;
    }
  }
  return out;
}

/** Load the dictionary of a language (`i18n/<code>.js`). */
async function loadDict(code: string): Promise<Dict> {
  const mod = (await import(`./i18n/${code}.js`)) as { default: Dict };
  return mod.default;
}

export interface InitOptions {
  /** Preferred languages (default: `navigator.languages`). */
  preferred?: readonly string[];
  /** Remembered choice (default: localStorage); `null` = none. */
  stored?: string | null;
  /** Load a dictionary (default: dynamic `import()`). */
  load?: (code: string) => Promise<Dict>;
}

/** Determine the language and load the dictionaries – before anything else (see main.ts). */
export async function initI18n(opts: InitOptions = {}): Promise<string> {
  const nav = typeof navigator === "undefined" ? [] : navigator.languages?.length ? navigator.languages : [navigator.language];
  const stored = opts.stored !== undefined ? supported(opts.stored) : storedLanguage();
  const code = stored ?? detectLanguage(opts.preferred ?? nav);
  const load = opts.load ?? loadDict;
  let main: Dict = {};
  let fb: Dict = {};
  try {
    main = await load(code);
    // English fills gaps – but not for German: its identifiers are German already
    if (code !== FALLBACK && code !== ID_LANGUAGE) fb = await load(FALLBACK);
  } catch (e) {
    console.warn(`No dictionary for "${code}":`, e);
  }
  setLanguage(code, main, fb);
  return code;
}

/** Set language and dictionaries directly (tests, initI18n). */
export function setLanguage(code: string, main: Dict, fb: Dict = {}): void {
  current = code;
  dict = main;
  fallback = fb;
  rules = null;
  if (typeof document !== "undefined") document.documentElement.lang = code;
}
