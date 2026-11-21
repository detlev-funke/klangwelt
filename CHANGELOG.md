# Changelog

All notable changes to Klangwelt. The browser version and the desktop app share
one version number: the date and time of the release, so `2026.11.21+1430` is
21 November 2026, 14:30. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This file is generated from the JSON files in [changelog/](changelog/README.md),
which also says how to add an entry. The app shows the same entries in its own
language under the gear menu → What's new.

## 2026.11.16+0845 - 2026-11-16 08:45

### Added

- Usable on phones and tablets: a compact window view, two fingers pan and zoom, a long press opens the context menu, a double tap adds a note.
- The toolbar fits every window width: what has no room moves into the “⋯” menu, and the gear always stays.
- The recording analysis detects dynamics, accents and sforzati, staccato and legato, and carries them into the notes.
  - Technical: Velocity from the note's level relative to the recording; accents as a higher velocity and a brighter attack.
- Flags in the language menu.
  - Technical: Inline SVG (ui/src/flags.ts), because emoji flags show up as letters on Windows.

### Changed

- Export is in the File menu (Ctrl+E); Save is a disk with a dot for unsaved changes, and the project name has more room.

### Fixed

- The bar above the track headers no longer cuts off “+ Instrument” in any language.

## 2026.11.15+2036 - 2026-11-15 20:36

### Added

- Live analysis while recording: the note you sing or play, the level, the key and the tempo as they come in.

### Internal

- README: the browser version has the score library too.

## 2026.11.14+2315 - 2026-11-14 23:15

### Added

- Klangwelt in the browser and as a desktop app, both with the same core: tracks and notes, a sound workshop, rhythm and song, harmony, fugue, recording with melody detection, and export.
  - Technical: Rust core with its own DSP, TypeScript UI without runtime libraries, Tauri 2 shell; the browser runs the core as WebAssembly.
- Seven languages: English, German, French, Italian, Spanish, Dutch and Polish. Klangwelt follows the system language, and the gear menu switches it.
  - Technical: Keys are the English texts, dictionaries in ui/src/i18n/; the core's messages follow through set_language.
- The score library in the browser too: more than 76,000 free scores, searchable by title, composer or melody.
  - Technical: KWIX04 index read by HTTP range from Zenodo; package builder for own scores.
- Note entry: a double-click adds a note, and notes can be typed on the keyboard, with Shift for chords.
- Note clips play live; when playback can't keep up, they are prepared in the background.
  - Technical: Pre-rendering in the background under load.
- Tools for AI agents: an agent can reserve tasks from the chat, show a preview, stop, and work in batches.
  - Technical: MCP tools: reserve, peek, stop, revision, placeholders, batch, compact notes.

### Changed

- The metronome is off by default and stays silent at the start of a recording; switched on, it can be heard.

### Internal

- The bundled libopus is marked as vendored code.
- Source code comments, agent texts and test titles are in English.
- README: screenshots of the overview, piano roll, bass sound fields and circle of fifths.
