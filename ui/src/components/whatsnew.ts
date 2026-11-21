// "What's new": the changes of every version, from changelog/ in the repo
// (scripts/changelog.mjs builds them into dist/changelog/<lang>.js, one module per
// language). Opened from the gear menu. After an update a note with a button points
// to it once, and a dot on the gear stays until the window was opened; a first start
// shows nothing. Desktop and browser share this: entries for the other platform are
// left out.

import { backend } from "../bridge.js";
import { h, modal } from "../dom.js";
import { language, plural, t, tk } from "../i18n.js";
import { signal } from "../signals.js";
import { notify } from "../store.js";

export type ChangeType = "added" | "changed" | "fixed" | "removed";
export type ChangePlatform = "all" | "browser" | "desktop";
export interface Change {
  id: string;
  type: ChangeType;
  platform: ChangePlatform;
  text: string;
}
export interface Release {
  version: string;
  /** ISO 8601 with offset, e.g. 2026-11-21T14:30+01:00 */
  date: string;
  entries: Change[];
}
export interface Changelog {
  /** Version of this build (ui/package.json). */
  version: string;
  /** Git commit of this build, if known. */
  build: string | null;
  /** Newest first. */
  releases: Release[];
  /** Changes not released yet. */
  unreleased: Change[];
}

/** localStorage: date of the newest release this user was shown (or had at the first start). */
export const SEEN_KEY = "klangwelt.changelog.seen";
/** A release arrived that hasn't been looked at yet: dot on the gear and on the menu entry. */
export const whatsNewUnread = signal(false);

const LABELS: Record<ChangeType, string> = { added: tk("New"), changed: tk("Changed"), fixed: tk("Fixed"), removed: tk("Removed") };

let loading: Promise<Changelog | null> | null = null;
/** What was stored before this start (null: first start or no storage) – marks "new for you". */
let seenBefore: string | null = null;

/** The changelog in the UI's language (null if it can't be loaded). */
export function loadChangelog(): Promise<Changelog | null> {
  loading ??= import(`../changelog/${language()}.js`).then(
    (m: { default: Changelog }) => m.default,
    () => null,
  );
  return loading;
}

/** This app's platform as the changelog names it. */
export function currentPlatform(): "browser" | "desktop" {
  return backend.platform === "desktop" ? "desktop" : "browser";
}

/** Only what concerns `platform`; releases left without entries disappear. */
export function forPlatform(log: Changelog, platform: "browser" | "desktop"): Changelog {
  const keep = (c: Change) => c.platform === "all" || c.platform === platform;
  return {
    ...log,
    releases: log.releases.map((r) => ({ ...r, entries: r.entries.filter(keep) })).filter((r) => r.entries.length),
    unreleased: log.unreleased.filter(keep),
  };
}

/** Releases newer than `seen` (an ISO date; null: nothing counts as new). */
export function newSince(log: Changelog, seen: string | null): Release[] {
  if (!seen) return [];
  const since = Date.parse(seen);
  if (Number.isNaN(since)) return [];
  return log.releases.filter((r) => Date.parse(r.date) > since);
}

function readSeen(): string | null {
  try {
    return localStorage.getItem(SEEN_KEY);
  } catch {
    return null;
  }
}

function writeSeen(date: string): void {
  try {
    localStorage.setItem(SEEN_KEY, date);
  } catch {
    /* without storage there is no "new since" either */
  }
}

/** At start: the first start remembers the newest release quietly; after an update a note and the dot. */
export async function checkWhatsNew(): Promise<void> {
  const raw = await loadChangelog();
  if (!raw) return;
  const log = forPlatform(raw, currentPlatform());
  const newest = log.releases[0];
  if (!newest) return;
  seenBefore = readSeen();
  if (!seenBefore) {
    writeSeen(newest.date);
    return;
  }
  const fresh = newSince(log, seenBefore);
  if (!fresh.length) return;
  whatsNewUnread.value = true;
  const n = fresh.reduce((sum, r) => sum + r.entries.length, 0);
  notify(
    plural(n, "Klangwelt was updated: one change in version {version}", "Klangwelt was updated: {n} changes in version {version}", { version: newest.version }),
    "info",
    { label: t("Show what's new"), run: () => void openWhatsNew() },
  );
}

/** "21 November 2026, 14:30" in the UI's language and the viewer's time zone. */
function formatDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  try {
    return new Intl.DateTimeFormat(language(), { dateStyle: "long", timeStyle: "short" }).format(d);
  } catch {
    return d.toLocaleString();
  }
}

function changeList(entries: Change[]): HTMLElement {
  return h(
    "ul",
    { class: "whatsnew-entries" },
    ...entries.map((c) =>
      h("li", { class: `whatsnew-entry ${c.type}`, "data-change": c.id }, h("span", { class: "whatsnew-type" }, t(LABELS[c.type])), h("span", { class: "whatsnew-text" }, c.text)),
    ),
  );
}

function body(log: Changelog): Node[] {
  const fresh = new Set(newSince(log, seenBefore).map((r) => r.version));
  const sections: Node[] = [];
  if (log.unreleased.length)
    sections.push(
      h(
        "section",
        { class: "whatsnew-release unreleased" },
        h("h3", {}, h("span", { class: "whatsnew-date" }, t("Since version {version}", { version: log.releases[0]?.version ?? log.version }))),
        changeList(log.unreleased),
      ),
    );
  for (const r of log.releases)
    sections.push(
      h(
        "section",
        { class: "whatsnew-release", "data-version": r.version },
        h(
          "h3",
          {},
          h("span", { class: "whatsnew-date" }, formatDate(r.date)),
          h("span", { class: "whatsnew-ver" }, r.version),
          fresh.has(r.version) ? h("span", { class: "whatsnew-fresh" }, t("new for you")) : null,
        ),
        changeList(r.entries),
      ),
    );
  return [
    h("div", { class: "whatsnew-build" }, log.build ? t("Version {version} · build {build}", { version: log.version, build: log.build }) : t("Version {version}", { version: log.version })),
    h("div", { class: "whatsnew-list" }, ...sections),
  ];
}

/** The window with every version's changes; counts as seen. */
export async function openWhatsNew(): Promise<void> {
  const raw = await loadChangelog();
  const log = raw && forPlatform(raw, currentPlatform());
  if (log?.releases[0]) writeSeen(log.releases[0].date);
  whatsNewUnread.value = false;
  const content = log ? body(log) : [h("p", {}, t("The list of changes could not be loaded."))];
  await modal(t("What's new"), content, [{ label: t("Close"), value: "close", kind: "primary" }]);
}

/** Entry for the gear menu: "What's new" with this build's version (and the dot after an update). */
export function whatsNewMenuItem(close: () => void): HTMLElement {
  const version = h("small", {});
  void loadChangelog().then((log) => {
    if (log) version.textContent = t("Version {version}", { version: log.version });
  });
  return h(
    "button",
    {
      class: `set-option whatsnew-item${whatsNewUnread.peek() ? " unread" : ""}`,
      "data-whatsnew": "",
      onclick: () => {
        close();
        void openWhatsNew();
      },
    },
    h("span", { class: "whatsnew-dot", "aria-hidden": "true" }),
    h("span", {}, h("b", {}, t("What's new")), version),
  );
}
