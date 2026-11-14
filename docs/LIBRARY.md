# Score library for the browser (Zenodo package)

The browser version searches two libraries together:

1. **The score library package**, built offline from public-domain/CC0
   sources and hosted on Zenodo. The browser never downloads it as a whole: it
   reads the parts a search or an opened score needs, by HTTP range, and keeps
   just those in its cache.
2. **Your own scores**: MusicXML/MXL files dropped onto the library window or
   uploaded with "Add". They are indexed in the core (WebAssembly) and stored
   in the browser's storage (OPFS).

Text search, melody search (typed, from a clip, or sung) and browsing (the
tree) cover both and show the hits mixed. Every hit and every opened score
shows where it comes from: "My scores", or collection and license from the
package (e.g. "OpenScore Lieder · CC0-1.0").

## The package

`crates/klang-libbuild` builds it (see below). The byte-level description
of every file is in [LIBRARY-FORMAT.md](LIBRARY-FORMAT.md); the builder
copies it into the package's `README.md`. Files:

| File | Contents | How the browser reads it |
|---|---|---|
| `klangwelt-index.kwix` | Melody index, format KWIX04 (`klang_music::diskindex`): n-grams of interval/rhythm features of every voice, voice entries, feature sequences, facets, chord statistics. Between header and voice entries lies the **provenance** (JSON: sources, versions, checksums, counts, builder). | Header at start; per search the key directory once, then key blocks, occurrence lists, feature sequences and voice entries |
| `klangwelt-scores.zip` | The scores as they came (MXL, MusicXML), *stored* (no recompression). Every voice entry carries the offset and length of its score's ZIP entry (`DocInfo::zip`); the central directory at the end makes every file reachable for ordinary ZIP tools, too (ZIP64 for more than 65,535 entries). | One range per opened score |
| `klangwelt-text.bin` | Text index: the words of title, composer, part names, genre, instrument groups, key and mood → scores (`klang_music::webpack`). A query word matches every word that contains it, so it finds exactly what the desktop's text search finds. | Word list once (compressed), then the lists of the matching words |
| `klangwelt-facets.bin` | Facet table for the tree: per level the values that occur, per score their numbers; sorted like the tree lists scores. | Whole, when browsing for the first time |
| `klangwelt-chords.bin` | Chord progression statistics (all, Baroque, Bach) for the chord suggestions. | Whole, when the harmony window asks |
| `klangwelt-manifest.json` | Title, version, license, counts, every file with size, SHA-256 and MD5, the sources with attribution. | At start (small) |
| `README.md`, `zenodo.json` | Description and metadata for the Zenodo record (not read by the app). | – |

The KWIX04 index alone is a complete library for the desktop as well (it
carries facets and chord statistics itself); the extra files only exist so
that the browser never needs those large sections at once.

## Sources and licenses

`crates/klang-libbuild/sources.json` pins every source:

| Source | Version | Taken | License |
|---|---|---|---|
| OpenScore Lieder (GitHub) | commit `38c5db51…` | all `scores/**/*.mxl` (1,462) | CC0 1.0 |
| OpenScore String Quartets (GitHub) | commit `2614b0b2…` | all `scores/**/*.mxl` (196) | CC0 1.0 |
| PDMX (Zenodo) | record 15571083: `PDMX.csv` (md5 `30392ccf…`), `mxl.tar.gz` (md5 `49ffd75e…`) | rows with `license` = `publicdomain` or `cc-zero`, `license_conflict` = False, and in `subset:deduplicated` | per score: Public Domain Mark 1.0 / CC0 1.0; the dataset is CC BY 4.0 |

* **PDMX record:** 13763756 is the first version of the PDMX record and
  contains a single `PDMX.tar.gz`. `mxl.tar.gz` and `PDMX.csv` exist from
  version 14648209 on; 15571083 is the latest (same concept DOI
  10.5281/zenodo.13763755), and the desktop uses it as well.
* **Duplicates:** PDMX's own deduplicated subset (`subset:deduplicated`,
  "best unique arrangement", ≈ 102,600 of 254,077 scores) removes repeated
  uploads of the same piece. Leave out `"subset"` in `sources.json` to take
  every row without a license conflict (≈ 222,900). In addition the builder
  drops every score whose melodic content (the feature sequences of all its
  voices) equals one taken before. The curated sources come first, so a PDMX
  copy of an OpenScore song is the one that's dropped.
* **Not included:** Open Goldberg Variations and Open Well-Tempered Clavier
  (CC0) are available only as PDF/Braille without a login; their MusicXML
  is on musescore.com, which needs a login. They may be part of PDMX.
  Sources with a non-commercial license (KernScores, DCML) are never used.
* **The package** is licensed CC BY 4.0, with attribution to PDMX and
  OpenScore (in `README.md`, the manifest and `zenodo.json`).

## Building

```bash
cargo run --release -p klang-libbuild -- plan
cargo run --release -p klang-libbuild -- build --allow-download-mb 2200
cargo run --release -p klang-libbuild -- verify target/library/package
```

* `plan` lists what would be downloaded into the cache (`target/library/cache`).
* `build` refuses to download more than 200 MB unless `--allow-download-mb`
  allows it. Downloads are checked against the pinned MD5 (Zenodo) and
  resumed after an interruption. The result lands in `target/library/package`.
* Options: `--only openscore-lieder,openscore-quartets` (some sources),
  `--limit N` (at most N scores per source, for trial runs), `--date
  YYYY-MM-DD` (version of the package), `--jobs N`, `--sources FILE`,
  `--out DIR`, `--cache DIR`.
* **Reproducible:** sources are read in a fixed order, scores are parsed in
  parallel but written in that order, ZIP entries carry no times. The same
  sources and the same `--date` give byte-identical files (a test checks
  this). The provenance in the index header and the manifest record the
  source versions, the checksums of the downloads (SHA-256, MD5) and for
  GitHub sources a SHA-256 over all files.
* `verify` checks the sizes and checksums against the manifest, opens the
  index, and checks that every score in the index points to an entry of the
  ZIP's central directory with the right name.
* `describe [DIR]` writes `README.md` and `zenodo.json` of a built package
  again, from its manifest and the provenance in the index header (the
  build does this at its end). The README states that the package is a
  derived work of PDMX, lists per source what was taken, left out and
  changed, and contains [LIBRARY-FORMAT.md](LIBRARY-FORMAT.md).

### Trial run (OpenScore)

```bash
cargo run --release -p klang-libbuild -- build --only openscore-lieder,openscore-quartets --out target/library/openscore
```

Measured on 2026-11-14 (Ryzen 9 5900X, 8 parser threads):

| | |
|---|---|
| Scores | 1,656 (1,460 Lieder, 196 quartets; 2 exact duplicates dropped, 0 failed) |
| Voices | 8,163 |
| Time | 69 s with an empty cache (1,658 single files from GitHub), 5 s with a filled cache |
| Index | 59.4 MB · scores ZIP 59.4 MB · text index 36 KB · facet table 14 KB · chord statistics 0.8 MB |

Parsing and analysing runs at about 1,400 scores per second; the rest of
the time is downloading, merging the index and writing.

### Full build (measured)

`cargo run --release -p klang-libbuild -- build --allow-download-mb 2200`,
measured on 2026-11-14 (same machine, while other builds ran):

| | |
|---|---|
| PDMX rows accepted | 77,321 of 254,077 (license, no conflict, `subset:deduplicated`) |
| Scores | 76,552: PDMX 74,896 (1,739 duplicates and 686 without a voice of 8 notes left out, 0 failed), OpenScore Lieder 1,460, String Quartets 196 |
| Voices | 263,793 |
| Time | 9.5 min with an empty cache (the 2.1 GB download took 6 of them), 3 min from the cache |
| `klangwelt-index.kwix` | 671.9 MB (key table 157 MB, occurrence lists 310 MB, feature sequences 91 MB, voice entries 66 MB, facets 33 MB) |
| `klangwelt-scores.zip` | 1,137.6 MB (≈ 15 KB per score; ZIP64, 76,552 entries) |
| text index, facet table, chords | 1.9 MB, 1.0 MB, 3.8 MB |
| Upload to Zenodo | ≈ 1.82 GB in 7 files |
| Free disk while building | ≈ 5 GB (cache 2.1 GB, package 1.8 GB, temporary runs) |

Without `"subset"` about 2.9 times as many PDMX rows qualify (≈ 223,000),
so expect roughly 2–3 times the sizes and the build time. Zenodo allows
50 GB and 100 files per record.

## Uploading to Zenodo

1. Build and verify (above).
2. On zenodo.org: **New upload**. Upload these files from
   `target/library/package`, keeping their names:
   `klangwelt-index.kwix`, `klangwelt-scores.zip`, `klangwelt-text.bin`,
   `klangwelt-facets.bin`, `klangwelt-chords.bin`, `klangwelt-manifest.json`
   and `README.md`.
3. Fill in the form from `zenodo.json`: resource type *Dataset*, title,
   description, creators, license **CC BY 4.0**, version, keywords, and as
   related works *is derived from* PDMX (`10.5281/zenodo.15571083`) and the
   two OpenScore repositories.
4. Publish. Note the **record ID** (the number in
   `https://zenodo.org/records/<id>`).
5. Enter it in `ui/static/library.json`:

   ```json
   { "zenodoRecord": "<id>", "baseUrl": "" }
   ```

   then `npm run build:web` and publish the app (gh-pages) as usual.

**Published:** version 2026-11-14 is record `24231432`,
[10.5281/zenodo.24231432](https://doi.org/10.5281/zenodo.24231432) (all
versions: [10.5281/zenodo.24231431](https://doi.org/10.5281/zenodo.24231431)).

A new version of the package is a new version of the Zenodo record, which
gets a new record ID. Update `library.json`. Files of one record never
change, so cached ranges stay valid. On a server whose files change in
place (a mirror via `baseUrl`, the development server), the worker
compares the new manifest's file checksums with the stored copy and
empties the range cache when they differ, before it reads anything.

## How the app finds and reads the package

* `library.json` next to the app names the package: `zenodoRecord`
  (files at `https://zenodo.org/api/records/<id>/files/<name>/content`, which
  answers ranges with `Access-Control-Allow-Origin: *`) or `baseUrl` (any
  static server with range support). Empty: no package, only own scores.
  `?library=<url>` in the address overrides it (development).
* At start the worker fetches the manifest. Offline, it uses the copy from
  last time. Then it attaches the package
  (`klang_app::weblib::attach`), which reads the index header.
* **Rounds:** the core runs synchronously, the network doesn't. A command
  that misses bytes ends with status 2 and the list of missing ranges
  (`klang_music::source::remote`). The worker (`ui/src/web/remote.ts`)
  fetches them, feeds them to the core (`kw_lib_feed`) and runs the command
  again. A melody search needs 4 such rounds: key blocks → occurrence lists
  → feature sequences of the candidates → voice entries of the hits. The
  first search fetches, in one round before those, what every search needs:
  the key directory and – for a large index, whose tables aren't loaded at
  opening – the two offset tables, whole. The search stops early as long as
  votes or candidates are incomplete (`SearchData::pending`), so it never
  fetches data for wrong candidates.
* **Requests:** the ranges of one round go out together: close ranges are
  merged, up to 250 ranges per request (Range header ≤ 6,000 characters) as
  `multipart/byteranges`. Zenodo answers 400 ranges in one request; it
  sends them with a wrong `Content-Type` and LF line ends, so the parser
  takes the boundary from the body and each part's length from its
  `Content-Range`. A server that answers several ranges with the whole file
  (GitHub Pages, status 200) is noticed. The reply is not read, and from
  then on one range is requested at a time. Any other answer is an error.
* **Cache:** every range the core asked for is stored in the Cache Storage
  `klangwelt.library` (key: URL plus range), *only* what was actually read,
  never the whole index. The worker serves later reads from there, so
  searches and scores used before work offline. The library window
  ("Score library" panel, click on the library's name) shows how much is
  stored and empties it. In memory the core keeps at most 48 MB of ranges.
* **Request limit:** Zenodo allows guests 60 requests per minute and 2,000
  per hour (documented; `X-RateLimit-Limit` reports 133 per minute), counted
  per client: each visitor's browser has its own budget, unless several
  share one address (a school network). Both file URLs
  (`/api/records/…/content` and `/records/…?download=1`) count against the
  same budget, and only the API URL sends CORS headers. Measured against
  the full package (`requests_against_a_package`, below):

  | Action | Requests | Data |
  |---|---|---|
  | Opening the package (manifest, header, provenance) | 3 | 3 KB |
  | First melody search (plus key directory and offset tables) | 5 | ≈ 5.6 MB |
  | Every further melody search | 4 | 0.3–0.6 MB |
  | Text search | 2 (4 for a word in very many terms, like "bach") | 2–300 KB |
  | Opening a score | 1 | its size (typ. 5–80 KB) |
  | Browsing, chord suggestions (first time) | 1 each | 1.0 MB, 3.8 MB |

  Ranges read once come from the cache and cost nothing. The worker keeps
  below the limit with a token bucket: 20 requests at once, then 30 per
  minute on average (1,800 per hour). It reads `X-RateLimit-Remaining` and
  `-Reset` (exposed through CORS) and waits for the server's next window
  when 3 or fewer requests are left. On a 429 answer it waits for that
  window too and tries again (up to 3 times). A wait of 2 s or more shows
  a short notice in the library window. Searches run on Enter, not while
  typing.

## Own scores

* Drag MusicXML/MXL files onto the open library window, or use "Add".
  They move from `/Klangwelt/Import` to `/Klangwelt/Bibliothek/Eigene/` in
  the core's file tree, which the worker mirrors in OPFS.
* The core indexes them (`klang_app::weblib::add`) into an in-memory index
  (KWIX02, the same format as the desktop's index of individually added
  scores), saved as `/Klangwelt/Bibliothek/eigene.kwix`, and layers it over
  the package (`AnyIndex::layered`). Voice numbers of own scores follow those
  of the package.
* The same file twice is skipped. A score without a voice of at least 8
  notes is reported and not added. The "Score library" panel lists the own
  scores and removes them; removing re-indexes the rest without analysing
  them again.

## Desktop

Nothing changes on the desktop. As an **option**, the package can serve as a
desktop library without any conversion. `DiskIndex::open` notices a
`klangwelt-scores.zip` next to the index and reads the scores from there. A
desktop "package" would then be: download `klangwelt-index.kwix` and
`klangwelt-scores.zip` (about the same size as the PDMX download, but
already indexed), put them into a library folder and register the index.
The download itself (curl, as for the other packs) and the entry in the
desktop's pack catalog are what's still missing, both in the desktop shell
(`app/src/library.rs`).

## Tests

* `cargo test -p klang-libbuild`: building from a local folder (duplicates,
  broken files, provenance, `verify`, byte-identical rebuild, README and
  `zenodo.json` written again), SHA-256/MD5, every key of `sources.json` read.
* `cargo test -p klang-music`: ZIP writer/reader (also against the `zip`
  crate, ZIP64), text index and facet table against the desktop's search and
  tree, the remote index read round by round against the file (≤ 5 rounds
  per melody search, all lists of a broad text search in one round).
* Against a built package (ignored by default): rounds, requests and bytes
  per action, simulated like the worker fetches them:

  ```bash
  KLANG_PACKAGE=$PWD/target/library/package cargo test --release -p klang-music requests_against_a_package -- --ignored --nocapture
  ```
* `cargo test -p klang-app weblib`: own scores added, found, opened, kept
  across a restart, removed.
* `tests/unit/library-range.test.mjs`: the development server's ranges, the
  multipart parser (including Zenodo's format), and how ranges are grouped
  into requests.
* `tests/web/library.test.mjs`: builds a sample package from
  `tests/web/fixtures/library` (six traditional tunes) and serves it with
  `serve.mjs --library`. Checks: text and melody search find a piece in the
  package and one of the own scores, a score opens from the ZIP, the tree
  covers both, only a fraction is cached, and after a restart without a
  server the same searches and scores still work.

The other web tests start the server with `--no-library`, so they never read
the Zenodo record named in `ui/static/library.json`.

For local trying out:

```bash
cargo run --release -p klang-libbuild -- build --sources tests/web/fixtures/library/sources.json --out target/library/sample
cd ui && npm run build:web && node scripts/serve.mjs --web --library=../target/library/sample
```
