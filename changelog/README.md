# Changelog entries

Every change a user can notice gets one small JSON file in `changelog/unreleased/`,
committed together with the change itself. From these files come
[CHANGELOG.md](../CHANGELOG.md) (English), the release notes on GitHub and the app's
**What's new** window (gear menu), which shows each entry in the UI's language.

One file per change means that sessions working in parallel never edit the same
file: nobody has to merge or split hunks in a shared list.

## Adding an entry

The file name is the entry's id – lowercase letters, digits and hyphens, named after
the topic: `changelog/unreleased/tuned-drums.json`.

```json
{
  "type": "added",
  "platform": "browser",
  "text": {
    "en": "The browser version can be installed as an app, on phones too, and then starts offline.",
    "de": "Die Browser-Version lässt sich als App installieren, auch auf dem Handy, und startet dann ohne Netz.",
    "fr": "…", "it": "…", "es": "…", "nl": "…", "pl": "…"
  },
  "dev": "Web app manifest and icons (scripts/icons.mjs renders them with a headless Edge)."
}
```

* **`type`**: `added`, `changed`, `fixed` or `removed` – shown in the app.
  `internal` is for developers only (workflows, docs, refactoring): it appears only in
  CHANGELOG.md and needs only the English text.
* **`platform`** (optional): `browser` or `desktop` when only one of them is
  affected; left out, it concerns both. The app leaves out the other platform's
  entries, CHANGELOG.md marks them.
* **`text`**: one line per language – `en`, `de`, `fr`, `it`, `es`, `nl`, `pl`. Write
  for the people using Klangwelt: what they can do now, or what works differently.
  Use the UI's own words for menus and windows (look them up in `ui/src/i18n/*.ts`)
  and the same form of address as the UI (German "du", French "vous"). File and
  function names belong in `dev`.
* **`dev`** (optional): one technical line in English, shown under the entry in
  CHANGELOG.md.

Keep it to the change as a whole: a follow-up fix to a feature that isn't released
yet edits that feature's entry instead of adding a second one. Texts can be corrected
at any time – after a release in `changelog/releases/*.json`, followed by
`npm run changelog -- markdown`.

Check (in `ui/`): `npm run changelog`. The unit tests (`npm run test:unit`) check the
same – every language present, known types, CHANGELOG.md up to date.

## Versions

A version is named after the date and time of its release: `2026.11.21+1430` is
21 November 2026, 14:30. That is valid SemVer (the date as major.minor.patch, the time
as build metadata), so Cargo, npm and Tauri accept it, and several releases a day are
fine. The browser version and the desktop app share one version number: it lives in
`ui/package.json`, and the desktop app reads it from there (`app/tauri.conf.json` in
klangwelt-desktop points to that file; Windows shows the exe's file version as
2026.11.21.1430).

## Making a release

In `ui/`:

```bash
npm run changelog -- release
```

This takes the **committed** entries in `changelog/unreleased/`, writes them to
`changelog/releases/<date>-<time>.json`, rewrites CHANGELOG.md, sets the version in
`ui/package.json` (and the lock file), commits exactly these files and tags the commit
`v2026.11.21+1430` – all locally. Entries other sessions haven't committed yet stay
for the next release, and other staged changes stay staged. It refuses while one of
the files it writes has uncommitted changes. An agent session adds its attribution
with `--trailer="Co-Authored-By: …"`.

The desktop app follows in klangwelt-desktop:

```bash
npm run release:tag
```

It moves the `klangwelt/` submodule to the release commit if it isn't there yet,
commits that, and sets the same tag in klangwelt-desktop.

**Publishing** happens when the tags are pushed (`git push --follow-tags`, klangwelt
first, then klangwelt-desktop). Then `.github/workflows/release.yml` creates the GitHub
release of klangwelt with the notes from the release file, and the one in
klangwelt-desktop builds the exe on Windows and attaches it. The browser version is
deployed on every push to main anyway; a release gives that state a name.

If a push comes before a release, nothing is missing: the app shows unreleased
entries at the top under "Since version …".

## In the app

`npm run build` writes `ui/dist/changelog/<lang>.js` (one ES module per language, no
internal entries) next to the UI; `ui/src/components/whatsnew.ts` shows it. After an
update – a release newer than the one stored in `localStorage`
(`klangwelt.changelog.seen`) – a note with a button and a dot on the gear point to
the window until it is opened. A first start shows nothing.
