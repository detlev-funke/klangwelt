// "What's new" (ui/src/components/whatsnew.ts): the gear menu opens the changes of every
// version from changelog/, in the UI's language and without desktop-only entries. A
// first start shows nothing; after an update a note with a button and a dot on the gear
// point to the window until it was opened.
import { after } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT, sleep, startWeb, makeTest } from "./harness.mjs";

const { appData } = await import(pathToFileURL(join(ROOT, "ui", "scripts", "changelog.mjs")).href);
const VERSION = JSON.parse(readFileSync(join(ROOT, "ui", "package.json"), "utf8")).version;
const SEEN = "klangwelt.changelog.seen";

/** What the browser version should show in German: no internal and no desktop-only entries. */
function expected() {
  const d = appData("de");
  const keep = (e) => e.platform !== "desktop";
  return {
    releases: d.releases.map((r) => ({ ...r, entries: r.entries.filter(keep) })).filter((r) => r.entries.length),
    unreleased: d.unreleased.filter(keep),
  };
}

let app;
const test = makeTest(() => app);
after(async () => {
  await app?.close();
});

/** The open "What's new" window as data. */
const dialog = () =>
  app.eval(() => {
    const box = document.querySelector(".modal .whatsnew-list")?.closest(".modal");
    if (!box) return null;
    const r = box.getBoundingClientRect();
    return {
      title: box.querySelector(".modal-title")?.textContent,
      build: box.querySelector(".whatsnew-build")?.textContent,
      inside: r.left >= 0 && r.top >= 0 && r.right <= innerWidth && r.bottom <= innerHeight,
      scrolls: getComputedStyle(box.querySelector(".whatsnew-list")).overflowY,
      sections: [...box.querySelectorAll(".whatsnew-release")].map((s) => ({
        unreleased: s.classList.contains("unreleased"),
        version: s.dataset.version ?? null,
        heading: s.querySelector(".whatsnew-date")?.textContent,
        fresh: !!s.querySelector(".whatsnew-fresh"),
        entries: [...s.querySelectorAll(".whatsnew-entry")].map((e) => ({
          id: e.dataset.change,
          label: e.querySelector(".whatsnew-type").textContent,
          title: e.querySelector(".whatsnew-title").textContent,
          titleBold: Number(getComputedStyle(e.querySelector(".whatsnew-title")).fontWeight) >= 600,
          line: e.querySelector(".whatsnew-line").textContent,
          text: e.querySelector(".whatsnew-text").textContent,
          image: e.querySelector("img.whatsnew-image")?.getAttribute("src") ?? null,
        })),
      })),
    };
  });

const state = () =>
  app.eval((key) => ({
    seen: localStorage.getItem(key),
    gearDot: document.querySelector('[data-help="einstellungen"]').classList.contains("unread"),
    note: document.querySelector("kw-toast .toast.has-action")?.textContent ?? null,
  }), SEEN);

test("first start: no note and no dot, the newest release is remembered quietly", async () => {
  app = await startWeb();
  const newest = expected().releases[0];
  await app.waitFor((key) => !!localStorage.getItem(key), { what: "remembered release" }, SEEN);
  await sleep(500);
  const s = await state();
  assert.equal(s.seen, newest.date);
  assert.equal(s.gearDot, false);
  assert.equal(s.note, null);
});

test("gear menu → What's new: every release in German with bold titles and screenshots, browser entries only, with the version", async () => {
  // the service worker serves the screenshots below (and keeps them)
  await app.waitFor(() => !!navigator.serviceWorker.controller, { timeout: 15000, what: "service worker active" });
  await app.clickOn('[data-help="einstellungen"]');
  await app.waitFor(() => /Version \S+/.test(document.querySelector(".set-menu.open [data-whatsnew]")?.textContent ?? ""), { what: "menu entry with version" });
  const item = await app.eval(() => {
    const b = document.querySelector(".set-menu.open [data-whatsnew]");
    return { text: b.textContent, unread: b.classList.contains("unread"), last: b === b.parentElement.lastElementChild };
  });
  assert.equal(item.text, `NeuigkeitenVersion ${VERSION}`);
  assert.equal(item.unread, false);
  assert.ok(item.last, "the last entry of the gear menu");

  await app.clickOn(".set-menu.open [data-whatsnew]");
  await app.waitFor(() => !!document.querySelector(".modal .whatsnew-list"), { what: "What's new window" });
  const d = await dialog();
  const want = expected();
  assert.equal(d.title, "Neuigkeiten");
  assert.match(d.build, new RegExp(`^Version ${VERSION.replace(/[.+]/g, "\\$&")}( · Build [0-9a-f]{7,})?$`));
  assert.ok(d.inside, "the window fits on the screen");
  assert.equal(d.scrolls, "auto");
  const releases = d.sections.filter((s) => !s.unreleased);
  assert.deepEqual(releases.map((s) => s.version), want.releases.map((r) => r.version));
  for (const [i, r] of want.releases.entries()) assert.deepEqual(releases[i].entries.map((e) => [e.id, e.text]), r.entries.map((e) => [e.id, e.text]), r.version);
  // every entry: "**Title:** text", a screenshot where the entry has one
  const shown = d.sections.flatMap((s) => s.entries);
  const wanted = new Map([...want.releases.flatMap((r) => r.entries), ...want.unreleased].map((e) => [e.id, e]));
  for (const e of shown) {
    const w = wanted.get(e.id);
    assert.equal(e.title, `${w.title}:`, e.id);
    assert.ok(e.titleBold, `${e.id}: title in bold`);
    assert.equal(e.line, `${w.title}: ${w.text}`, e.id);
    assert.equal(e.image, w.image ?? null, `${e.id}: screenshot`);
  }
  assert.ok(releases.every((s) => !s.fresh), "nothing is new for a first start");
  // dates in German, type labels translated
  assert.match(releases.at(-1).heading, /^14\. November 2026/);
  assert.equal(releases.at(-1).entries[0].label, "Neu");
  const labels = new Set(d.sections.flatMap((s) => s.entries.map((e) => e.label)));
  for (const l of labels) assert.ok(["Neu", "Geändert", "Behoben", "Entfernt"].includes(l), l);
  // not released yet: on top, "since" the newest release
  const open = d.sections.filter((s) => s.unreleased);
  assert.equal(open.length, want.unreleased.length ? 1 : 0);
  if (want.unreleased.length) {
    assert.equal(d.sections[0].unreleased, true);
    assert.equal(open[0].heading, `Seit Version ${want.releases[0].version}`);
    assert.deepEqual(open[0].entries.map((e) => e.id), want.unreleased.map((e) => e.id));
    assert.ok(open[0].entries.some((e) => e.id === "installable-web-app"), "a browser-only entry is shown");
  }

  // the screenshots load when scrolled into view and fit the window's width
  const pictures = await app.eval(async () => {
    const out = [];
    for (const img of document.querySelectorAll(".modal img.whatsnew-image")) {
      img.scrollIntoView({ block: "center" });
      const t0 = performance.now();
      while (!(img.complete && img.naturalWidth) && performance.now() - t0 < 5000) await new Promise((r) => setTimeout(r, 50));
      const list = img.closest(".whatsnew-list").getBoundingClientRect();
      const r = img.getBoundingClientRect();
      out.push({ src: img.getAttribute("src"), loaded: img.naturalWidth > 0, lazy: img.loading, alt: img.alt, fits: r.width > 0 && r.width <= list.width + 0.5 });
    }
    return out;
  });
  const withImage = shown.filter((e) => e.image);
  assert.equal(pictures.length, withImage.length);
  for (const p of pictures) {
    assert.ok(p.loaded, `${p.src} loads`);
    assert.equal(p.lazy, "lazy");
    assert.ok(p.fits, `${p.src} fits the window`);
    assert.equal(p.alt, wanted.get(withImage.find((e) => e.image === p.src).id).title, "alt text: the title");
  }
  // the service worker's offline list leaves them out; each one is kept once it was shown
  const sw = await app.eval(async (srcs) => {
    const key = (await caches.keys()).find((k) => k.startsWith("klangwelt-"));
    const cache = await caches.open(key);
    const kept = await Promise.all(srcs.map(async (s) => !!(await cache.match(new URL(s, location.href).href))));
    const script = await (await fetch("sw.js", { cache: "no-store" })).text();
    return { kept, listed: script.match(/"changelog\/images\/[^"]+"/g) ?? [] };
  }, pictures.map((p) => p.src));
  assert.deepEqual(sw.listed, [], "no screenshot in the list every visitor downloads");
  assert.ok(sw.kept.every(Boolean), "shown screenshots are kept for offline use");

  await app.clickOn(".modal .modal-actions .btn", { text: "Schließen" });
  await app.waitFor(() => !document.querySelector(".modal"), { what: "window closed" });
});

test("after an update: a note with a button and the dot on the gear, until the window was opened", async () => {
  const want = expected();
  assert.ok(want.releases.length >= 2, "two releases to step back");
  const [newest, before] = want.releases;
  await app.eval((key, date) => localStorage.setItem(key, date), SEEN, before.date);
  await app.reload();
  await app.waitFor(() => !!document.querySelector("kw-toast .toast.has-action"), { what: "update note" });
  let s = await state();
  const n = newest.entries.length;
  assert.equal(s.note, `${n === 1 ? `Klangwelt wurde aktualisiert: eine Änderung in Version ${newest.version}` : `Klangwelt wurde aktualisiert: ${n} Änderungen in Version ${newest.version}`}Neuigkeiten ansehen`);
  assert.equal(s.gearDot, true);
  assert.equal(s.seen, before.date, "not seen until opened");

  // the menu entry carries the dot too
  await app.clickOn('[data-help="einstellungen"]');
  await app.waitFor(() => !!document.querySelector(".set-menu.open [data-whatsnew]"), { what: "gear menu" });
  assert.equal(await app.eval(() => document.querySelector(".set-menu.open [data-whatsnew]").classList.contains("unread")), true);
  await app.clickOn('[data-help="einstellungen"]');

  await app.clickOn("kw-toast .toast-action");
  await app.waitFor(() => !!document.querySelector(".modal .whatsnew-list"), { what: "What's new window" });
  const d = await dialog();
  assert.deepEqual(d.sections.filter((x) => x.fresh).map((x) => x.version), [newest.version], "only the newer release is marked");
  s = await state();
  assert.equal(s.gearDot, false);
  assert.equal(s.seen, newest.date);
  await app.key("Escape");
  await app.waitFor(() => !document.querySelector(".modal"), { what: "window closed" });

  await app.reload();
  await sleep(1500);
  s = await state();
  assert.equal(s.note, null, "no note once seen");
  assert.equal(s.gearDot, false);
});

test("platform filter and 'new since' as functions", async () => {
  const r = await app.eval(async () => {
    const m = await import("./components/whatsnew.js");
    const e = (id, platform) => ({ id, type: "added", platform, text: id });
    const log = {
      version: "x",
      build: null,
      releases: [
        { version: "b", date: "2026-11-21T14:30+01:00", entries: [e("desk", "desktop")] },
        { version: "a", date: "2026-11-20T10:00+01:00", entries: [e("all", "all"), e("web", "browser")] },
      ],
      unreleased: [e("u-desk", "desktop"), e("u-web", "browser")],
    };
    const browser = m.forPlatform(log, "browser");
    const desktop = m.forPlatform(log, "desktop");
    return {
      platform: m.currentPlatform(),
      browser: browser.releases.map((x) => [x.version, x.entries.map((y) => y.id)]),
      browserOpen: browser.unreleased.map((y) => y.id),
      desktop: desktop.releases.map((x) => [x.version, x.entries.map((y) => y.id)]),
      since: m.newSince(log, "2026-11-20T10:00+01:00").map((x) => x.version),
      sinceNothing: m.newSince(log, null).length,
    };
  });
  assert.equal(r.platform, "browser");
  assert.deepEqual(r.browser, [["a", ["all", "web"]]], "a release with only desktop entries disappears");
  assert.deepEqual(r.browserOpen, ["u-web"]);
  assert.deepEqual(r.desktop, [["b", ["desk"]], ["a", ["all"]]]);
  assert.deepEqual(r.since, ["b"]);
  assert.equal(r.sinceNothing, 0);
});
