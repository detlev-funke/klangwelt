# Score library package: file formats

This is the byte-level description of the files in a Klangwelt score library
package (see [LIBRARY.md](LIBRARY.md)). `klang-libbuild` writes them and copies
this text into the package's `README.md`. The reference implementation is in
`crates/klang-music/src`: `diskindex.rs` (index), `webpack.rs` (text index,
facet table, chord file), `scorezip.rs` (scores ZIP), `chordstats.rs`
(chord statistics) and `melody.rs` (melody features).

## Conventions

* All integers are little-endian. `u8`/`u16`/`u32`/`u64` are unsigned, `i8`
  is signed (two's complement), `f32` is IEEE 754 single precision.
* `varint`: unsigned LEB128. Each byte carries 7 bits, the lowest first, and
  the high bit is set when another byte follows.
* `string`: a `varint` byte length, then that many bytes of UTF-8.
* `deflate`: a raw deflate stream (RFC 1951) without a zlib or gzip header.
* JSON is UTF-8 without a byte order mark.
* **Voice number:** every searchable voice of every score gets a number,
  counting from 0 in build order. The voices of one score have consecutive
  numbers. A **score number** is the voice number of the score's first voice.
  The text index, the facet table and the chord statistics refer to scores by
  that number.
* Offsets are absolute byte positions in the file unless marked *relative*.

## Melody features

Melody search compares voices by their transitions from note to note, not by
their notes:

1. A **voice** is one MusicXML `<voice>` on one staff of one part. Its notes
   are sorted by onset, and chords are reduced to their highest note (one note
   per onset). Voices with fewer than 8 notes are not indexed. A score without
   such a voice is left out.
2. A voice of *n* notes gives *n* − 1 **transitions**. Transition *i* is two
   signed bytes:
   * `interval = clamp(midi[i+1] − midi[i], −24, 24)` in semitones;
   * `rhythm = clamp(round(2 · log2(ioi[i+1] / ioi[i])), −4, 4)`, where
     `ioi[k]` is the time from note *k* to note *k* + 1 (for the last note:
     its duration), at least 10⁻⁶.

   The pair is the same in every key and at every tempo: a melody hummed
   higher or slower gives the same transitions.
3. The **contour** of a transition is a coarser interval: −2 for intervals of
   −3 or less, −1 for −2 to −1, 0 for a repeated note, 1 for 1 to 2, and 2 for
   3 or more.

### n-gram keys

Every window of *n* consecutive transitions of a voice is an **occurrence**:
the voice number and the position of the window's first transition. There
are three levels:

| Level | *n* | Bytes per transition | Weight in the search |
|---|---|---|---|
| 1 | 4 | `interval`, `rhythm` | 3 |
| 2 | 4 | `interval` | 2 |
| 3 | 6 | contour | 1 |

The **key** of a window is the 64-bit FNV-1a hash (offset basis
`0xcbf29ce484222325`, prime `0x100000001b3`). It is computed over the level
number as one byte, followed by the bytes of the *n* transitions as listed
above. A signed value enters as its two's-complement byte.

**Search:** the query (at least 5 notes) is turned into keys of all three
levels. Every occurrence of a query key votes, with the level's weight, for
its voice at the offset where the query would start. The 300 best (voice,
offset) candidates are then compared with the query transition by
transition: a semi-global alignment with gaps, where similar intervals and
rhythms score partly. The result is a score from 0 to 1 per hit.

## `klangwelt-index.kwix` – melody index (KWIX04)

### Header (112 bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | 6 bytes | magic `KWIX04` |
| 6 | 2 bytes | zero |
| 8 | u64 | `docs_off`: voice entries |
| 16 | u64 | `docs_idx_off`: voice entry offsets |
| 24 | u64 | `seqs_off`: feature sequences |
| 32 | u64 | `seqs_idx_off`: feature sequence offsets |
| 40 | u64 | `facets_off`: score facets |
| 48 | u64 | `post_off`: occurrence lists |
| 56 | u64 | `keys_off`: key table |
| 64 | u64 | `sparse_off`: sparse table of contents |
| 72 | u64 | `end`: file length |
| 80 | u64 | `n_docs`: number of voices |
| 88 | u64 | `n_keys`: number of keys |
| 96 | u64 | `n_post`: number of occurrences |
| 104 | u64 | `chords_off`: chord statistics, 0 = none |

### Sections, in file order

| Section | From | To | Contents |
|---|---|---|---|
| Provenance | 112 | `docs_off` | One JSON object (see below). Absent when `docs_off` = 112. Readers that don't know it skip it, because every section is found through the header. |
| Voice entries | `docs_off` | `docs_idx_off` | One JSON object per voice, concatenated without separators. |
| Voice entry offsets | `docs_idx_off` | `seqs_off` | (`n_docs` + 1) × u64, *relative* to `docs_off`: voice *i* is the bytes `o[i] … o[i+1]`. `o[0]` = 0. |
| Feature sequences | `seqs_off` | `seqs_idx_off` | Per voice its transitions, 2 bytes each: `interval` (i8), `rhythm` (i8). |
| Feature sequence offsets | `seqs_idx_off` | `facets_off` | (`n_docs` + 1) × u64, *relative* to `seqs_off`, as above. |
| Score facets | `facets_off` | `post_off` | A JSON array with one object per score, in score order. |
| Occurrence lists | `post_off` | `keys_off` | Per key the list of its occurrences, sorted by voice, then position. Each occurrence is `varint` (voice − previous voice, with the previous voice starting at 0 for every list) followed by `varint` position. Keys with more than 20,000 occurrences are not stored: they are too common to tell melodies apart. |
| Key table | `keys_off` | `sparse_off` | `n_keys` entries of 20 bytes, sorted by key: key (u64), list offset (u64, *relative* to `post_off`), list length in bytes (u32). |
| Sparse table of contents | `sparse_off` | `chords_off` (`end` if 0) | Every 64th key of the key table (entries 0, 64, 128, …) as u64. |
| Chord statistics | `chords_off` | `end` | See [Chord statistics](#chord-statistics). |

**Looking up a key:** binary-search the sparse table for the last entry *j*
that is ≤ the key. Then read key table entries 64·*j* … 64·*j* + 63 (at most
1,280 bytes, from `keys_off + 20 · 64 · j`), find the key in them, and read
its list.

### Voice entry (JSON)

| Field | Type | Meaning |
|---|---|---|
| `source` | string | The score's file in `klangwelt-scores.zip`: `<source id>/<path in the source>`. |
| `title`, `composer` | string | From the score (`work-title`/`movement-title`, `creator`). |
| `part` | string | Part name. |
| `voice` | string | Staff and voice in the part, as the label `System <staff> · Stimme <voice>`. |
| `noteCount` | number | Notes in the voice. |
| `license` | string | License of the score as shown, e.g. `CC0-1.0`, `Public Domain Mark 1.0`. Omitted when empty. |
| `collection` | string | Source collection, e.g. `PDMX`, `OpenScore Lieder`. Omitted when empty. |
| `zip` | [number, number] | Offset of the score's local header in `klangwelt-scores.zip`, and length of local header plus data. |

### Score facets (JSON)

One object per score: `doc` (score number), `title`, `composer`, `genres`
(list), `difficulty`, `parts` (list of part names), `mood`, `color`,
`energy`, `mode`, `key`, `meterKind`, `meter`, `tempo`, `ensemble`,
`families` (list of instrument families), `texture`, `harmony`, `length` and
`bars` (number). An empty string means unknown. The values are stable
identifiers, partly German words (e.g. the genre `Volksmusik & Folk`), that
Klangwelt translates for display. Genres, composer and difficulty come from
the source's metadata where it has any. Everything else is computed by
Klangwelt's heuristic analysis of the score.

### Chord statistics

```text
"KWH2", u8 G (groups), then G × (u32 L, L bytes: one KWH1 block)

KWH1 block:
  "KWH1", u32 major scores, u32 minor scores, u32 T (transitions),
  T × ( u8 mode (0 major, 1 minor), u16 from, u16 to, u32 count,
        u8 E, E × (u32 voice, u32 bar, f32 start, f32 end) )
```

* **Groups:** 0 all scores, 1 Baroque, 2 Bach (assigned by composer name).
* Chords are detected on a half-bar grid over all voices of a score,
  relative to the piece's main key. A **chord code** is
  `40 · r + 4 · q + inversion`. Here *r* = (root − tonic) mod 12 in
  semitones, the inversion is 0–3, and *q* is the quality: 0 major, 1 minor,
  2 diminished, 3 augmented, 4 dominant seventh, 5 major seventh, 6 minor
  seventh, 7 half-diminished seventh, 8 diminished seventh, 9 suspended fourth.
* A transition counts how often chord `from` is directly followed by chord
  `to` in pieces of that mode. Transitions are sorted by (mode, from, to).
* Up to 5 examples per transition: the score (its first voice's number), the
  bar, and start and end in quarter notes.

### Provenance (JSON)

`format` (`KWIX04`), `package`, `version` (build date), `builder`, `license`,
`scores`, `voices`, `dedupe`, and `sources`. Each source carries:

* its configuration from `sources.json`: `id`, `collection`, `license`,
  `url`, `attribution`, `kind` and the pin for that kind:
  * `pdmx`: `record`, `csv`/`mxl` with `name`, `size`, `md5`, plus
    `licenses`, `licenseNames`, `subset`;
  * `github`: `repo`, `commit`, `prefix`, `suffix`;
  * `local`: `path`;
* `fetched`: what was read:
  * `files`: a list of `name`, `size`, `sha256`, `md5`;
  * `contentSha256`: SHA-256 over the sorted lines `path<TAB>sha256<LF>` of
    all files read;
  * `acceptedRows`/`csvRows` for PDMX;
* `counts`: `read`, `scores`, `voices`, `duplicates`, `tooShort`, `failed`.

## `klangwelt-scores.zip` – the scores

* A ZIP file (PKWARE APPNOTE). Every entry is **stored** (method 0): MXL is
  compressed already, and stored entries can be served as they are.
* Local headers carry CRC-32 and both sizes; there are no data descriptors.
  Flags are `0x0800` (UTF-8 names), the time is 1980-01-01 00:00, and there
  are no extra fields. Only an entry of 4 GiB or more would get a ZIP64
  extra field.
* Entry names are `<source id>/<path in the source>`, the same as `source`
  in the voice entries. Entries are in score order.
* `zip: [offset, length]` in a voice entry covers the whole local entry. One
  HTTP request with `Range: bytes=offset-(offset+length−1)` returns it, and the
  file data start at `offset + 30 + name length + extra length`.
* The central directory at the end makes the ZIP readable for ordinary
  tools. With 65,535 entries or more (or above 4 GiB), a ZIP64 end of central
  directory record and its locator come before the classic end record, which
  then holds `0xFFFF`/`0xFFFFFFFF`.

## `klangwelt-text.bin` – text index (KWTX01)

```text
magic "KWTX01\0\0" (8 bytes)
u64 terms offset, u64 terms length, u64 lists offset, u64 lists length,
u64 number of terms, u64 number of scores
terms block (deflate):
  varint T, then T × (varint shared, string suffix, varint list length)
lists:
  per term, in the same order and without gaps: score numbers, ascending,
  as varint differences (the first from 0)
```

* **Terms** are the lower-cased words (split at white space) of a score's
  title, composer, part names, genres, instrument families, key and mood. They
  are sorted bytewise.
* A term is the previous term's first `shared` bytes followed by `suffix`.
  `shared` always ends on a UTF-8 character boundary.
* The first list starts at the lists offset, and each further list starts
  where the previous one ends.
* **Search:** a query word matches every term that contains it as a
  substring. A score matches when every query word matches one of its terms.
  A reader loads the terms block once and then reads only the lists of the
  matching terms.

## `klangwelt-facets.bin` – facet table (KWFT01)

```text
magic "KWFT01\0\0", then deflate of:
  varint R (rows, one per score)
  R × varint score number
  varint D (levels)
  D × ( string key, u8 multi (0 or 1), varint V, V × string value,
        R × ( multi: varint count, count × varint value number
              single: varint value number ) )
```

* Rows are sorted the way the tree lists scores: by lower-cased title, with
  empty titles last, then by score number.
* Value numbers index the level's value list. The values are the raw
  facet-field strings from the score facets.
* The levels and the score facet fields they come from, in file order:
  `genre` (`genres`, multi), `komponist` (`composer`), `stimmung` (`mood`),
  `faerbung` (`color`), `energie` (`energy`), `tongeschlecht` (`mode`),
  `tonart` (`key`), `taktart` (`meterKind`), `takt` (`meter`), `tempo`,
  `besetzung` (`ensemble`), `instrumente` (`families`, multi), `satz`
  (`texture`), `harmonik` (`harmony`), `laenge` (`length`), `schwierigkeit`
  (`difficulty`).

## `klangwelt-chords.bin` – chord statistics (KWCS01)

The magic `KWCS01\0\0`, then the deflated `KWH2` block. It is the same data
as the index's chord statistics section, so a browser needn't find that
section.

## `klangwelt-manifest.json`

| Field | Meaning |
|---|---|
| `format`, `formatVersion` | `klangwelt-library`, 1 |
| `title`, `version`, `license`, `scoreLicenses` | Package title, build date, license of the package, statement on the scores' licenses |
| `scores`, `voices` | Counts |
| `chordScores` | [major, minor]: scores with a detected chord progression |
| `files` | `index`, `scores`, `text`, `facets`, `chords`, each with `name`, `size`, `sha256`, `md5` |
| `sources` | Per source: `id`, `collection`, `license`, `url`, `attribution`, `scores` |
| `builder` | `klang-libbuild` and its version |

## Reading over HTTP

Klangwelt reads the package with HTTP range requests (several ranges per
request where the server allows it) and keeps only the ranges it used:

* **Start:** the manifest, the index header and the provenance. The two offset
  tables are read whole at the start when they are small (≤ 256 KB),
  otherwise whole with the first search.
* **Melody search:** the first search reads the sparse table of contents
  (and the large offset tables) in one round. Every search then reads the
  key table slices and the occurrence lists of the query's keys, followed by
  the feature sequences of the best candidates and the voice entries of the
  hits: 4 rounds of requests.
* **Text search:** the terms block once, then the lists of the matching terms.
* **Browsing:** the facet table once.
* **Opening a score:** one range of `klangwelt-scores.zip`.
