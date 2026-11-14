//! The library package for the web: the melody index (KWIX04) plus small files
//! of its own, all read by HTTP range from a static server (Zenodo).
//!
//! | file                    | what                                     | read                         |
//! |-------------------------|------------------------------------------|------------------------------|
//! | `klangwelt-index.kwix`  | melody index KWIX04 with provenance      | ranges, per search           |
//! | `klangwelt-scores.zip`  | the scores (MXL …), stored               | one range per opened score   |
//! | `klangwelt-text.bin`    | text index: terms → scores               | terms once, lists per search |
//! | `klangwelt-facets.bin`  | facet table for the tree                 | whole, when browsing         |
//! | `klangwelt-chords.bin`  | chord statistics (groups)                | whole, for chord suggestions |
//! | `klangwelt-manifest.json` | sizes, checksums, sources, counts      | whole, at start              |
//!
//! The KWIX04 index alone is a complete library for the desktop, too (it
//! carries facets and chord statistics itself); the extra files exist so the
//! browser never needs those big sections at once.
//!
//! All numbers little-endian, `varint` = LEB128. Compressed parts are raw
//! deflate streams.

use crate::chordstats::ChordStats;
use crate::facets::{FacetRows, ScoreFacets, DIMENSIONS};
use crate::source::{read_vec, ReadAt};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::sync::OnceLock;

pub const INDEX_FILE: &str = "klangwelt-index.kwix";
pub const SCORES_FILE: &str = "klangwelt-scores.zip";
pub const TEXT_FILE: &str = "klangwelt-text.bin";
pub const FACETS_FILE: &str = "klangwelt-facets.bin";
pub const CHORDS_FILE: &str = "klangwelt-chords.bin";
pub const MANIFEST_FILE: &str = "klangwelt-manifest.json";

const TEXT_MAGIC: &[u8; 8] = b"KWTX01\0\0";
const FACETS_MAGIC: &[u8; 8] = b"KWFT01\0\0";
const CHORDS_MAGIC: &[u8; 8] = b"KWCS01\0\0";

/// A word that matches more terms than this (or more list bytes than
/// [`BROAD_BYTES`]) narrows nothing down; it's left out when other words exist.
const BROAD_TERMS: usize = 20_000;
const BROAD_BYTES: u64 = 8 << 20;

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(buf: &[u8], i: &mut usize) -> io::Result<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *buf.get(*i).ok_or_else(corrupt)?;
        *i += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b < 0x80 {
            return Ok(v);
        }
        shift += 7;
        if shift > 63 {
            return Err(corrupt());
        }
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn get_str(buf: &[u8], i: &mut usize) -> io::Result<String> {
    let n = get_varint(buf, i)? as usize;
    let s = buf.get(*i..*i + n).ok_or_else(corrupt)?;
    *i += n;
    Ok(String::from_utf8_lossy(s).into_owned())
}

fn corrupt() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, klang_i18n::tr!("Index file incomplete or corrupted"))
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data).expect("in memory");
    e.finish().expect("in memory")
}

fn inflate(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

/// Magic + deflated payload.
fn packed(magic: &[u8; 8], payload: &[u8]) -> Vec<u8> {
    let mut out = magic.to_vec();
    out.extend_from_slice(&deflate(payload));
    out
}

fn unpacked(magic: &[u8; 8], file: &[u8]) -> io::Result<Vec<u8>> {
    if file.len() < 8 || &file[..8] != magic {
        return Err(corrupt());
    }
    inflate(&file[8..])
}

// ---------------------------------------------------------------- text index

/// Words of a score for the text search – the same text as the desktop's
/// search over the facets (title, composer, parts, genres, instrument groups,
/// key, mood), lower case, split at white space. A query word is contained in
/// that text exactly when it is contained in one of these words.
pub fn score_terms(f: &ScoreFacets) -> BTreeSet<String> {
    let hay = format!("{} {} {} {} {} {} {}", f.title, f.composer, f.parts.join(" "), f.genres.join(" "), f.families.join(" "), f.key, f.mood).to_lowercase();
    hay.split_whitespace().map(str::to_string).collect()
}

/// Writes `klangwelt-text.bin`: header, the sorted terms (front-coded,
/// deflated, read at once) and per term the scores (`doc` of their first
/// voice, ascending, as differences).
///
/// Header: magic (8) · terms offset, terms length, lists offset, lists length,
/// number of terms, number of scores (u64 each).
pub fn write_text_index(facets: &[ScoreFacets]) -> Vec<u8> {
    let mut terms: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for f in facets {
        for t in score_terms(f) {
            terms.entry(t).or_default().push(f.doc);
        }
    }
    let mut dict = Vec::new();
    let mut lists = Vec::new();
    put_varint(&mut dict, terms.len() as u64);
    let mut prev = String::new();
    for (term, docs) in &mut terms {
        docs.sort_unstable();
        docs.dedup();
        let start = lists.len();
        let mut last = 0u32;
        for &d in docs.iter() {
            put_varint(&mut lists, (d - last) as u64);
            last = d;
        }
        let shared = prev.bytes().zip(term.bytes()).take_while(|(a, b)| a == b).count();
        // only cut at character boundaries
        let shared = (0..=shared).rev().find(|&k| term.is_char_boundary(k)).unwrap_or(0);
        put_varint(&mut dict, shared as u64);
        put_str(&mut dict, &term[shared..]);
        put_varint(&mut dict, (lists.len() - start) as u64);
        prev = term.clone();
    }
    let dict = deflate(&dict);
    let head = 8 + 6 * 8;
    let mut out = Vec::with_capacity(head + dict.len() + lists.len());
    out.extend_from_slice(TEXT_MAGIC);
    for v in [head as u64, dict.len() as u64, (head + dict.len()) as u64, lists.len() as u64, terms.len() as u64, facets.len() as u64] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&dict);
    out.extend_from_slice(&lists);
    out
}

/// The terms of a text index (in memory) and where their lists lie.
pub struct TextDict {
    terms: Vec<String>,
    /// Start of each term's list (relative), plus the end.
    starts: Vec<u64>,
    lists_off: u64,
}

impl TextDict {
    /// Reads the header and the terms (two rounds for a remote file).
    pub fn read(src: &dyn ReadAt) -> io::Result<Self> {
        let head = read_vec(src, 0, 56)?;
        if &head[..8] != TEXT_MAGIC {
            return Err(corrupt());
        }
        let v: Vec<u64> = head[8..].chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
        let (dict_off, dict_len, lists_off) = (v[0], v[1], v[2]);
        let raw = inflate(&read_vec(src, dict_off, dict_len)?)?;
        let mut i = 0;
        let n = get_varint(&raw, &mut i)? as usize;
        let mut terms = Vec::with_capacity(n);
        let mut starts = Vec::with_capacity(n + 1);
        let mut at = 0u64;
        let mut prev = String::new();
        for _ in 0..n {
            let shared = get_varint(&raw, &mut i)? as usize;
            let rest = get_str(&raw, &mut i)?;
            let len = get_varint(&raw, &mut i)?;
            let term = format!("{}{}", prev.get(..shared).ok_or_else(corrupt)?, rest);
            starts.push(at);
            at += len;
            prev = term.clone();
            terms.push(term);
        }
        starts.push(at);
        Ok(Self { terms, starts, lists_off })
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Scores (doc of the first voice) whose text contains every word of
    /// `query`, ascending, at most `limit`.
    pub fn search(&self, src: &dyn ReadAt, query: &str, limit: usize) -> io::Result<Vec<u32>> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        // per word: the matching terms and how many list bytes they have
        let mut per_word: Vec<(Vec<usize>, u64)> = words
            .iter()
            .map(|w| {
                let ids: Vec<usize> = (0..self.terms.len()).filter(|&t| self.terms[t].contains(w.as_str())).collect();
                let bytes = ids.iter().map(|&t| self.starts[t + 1] - self.starts[t]).sum();
                (ids, bytes)
            })
            .collect();
        if per_word.iter().any(|(ids, _)| ids.is_empty()) {
            return Ok(Vec::new());
        }
        let broad = |(ids, bytes): &(Vec<usize>, u64)| ids.len() > BROAD_TERMS || *bytes > BROAD_BYTES;
        if per_word.iter().any(|w| !broad(w)) {
            per_word.retain(|w| !broad(w));
        }
        per_word.sort_by_key(|(_, bytes)| *bytes);
        // read the lists of all words in one round: every read is tried (a
        // remote file notes each missing range), errors count only afterwards
        let lists: Vec<Vec<io::Result<Vec<u8>>>> = per_word
            .iter()
            .map(|(ids, _)| ids.iter().map(|&t| read_vec(src, self.lists_off + self.starts[t], self.starts[t + 1] - self.starts[t])).collect())
            .collect();
        let mut result: Option<BTreeSet<u32>> = None;
        for raws in lists {
            let mut docs = BTreeSet::new();
            for raw in raws {
                let raw = raw?;
                let (mut i, mut d) = (0usize, 0u64);
                while i < raw.len() {
                    d += get_varint(&raw, &mut i)?;
                    docs.insert(d as u32);
                }
            }
            result = Some(match result {
                None => docs,
                Some(r) => r.intersection(&docs).copied().collect(),
            });
        }
        Ok(result.unwrap_or_default().into_iter().take(limit).collect())
    }
}

// ---------------------------------------------------------------- facet table

/// The facets of all scores, compact: per level a list of the values that
/// occur, per score their numbers. Rows are sorted like the tree lists scores
/// (by title, then doc) – the table doesn't carry titles.
pub struct FacetTable {
    rows: usize,
    docs: Vec<u32>,
    /// Per level (in [`DIMENSIONS`] order): the values and per row the value numbers.
    dims: Vec<Column>,
}

struct Column {
    key: String,
    multi: bool,
    values: Vec<String>,
    /// Start of each row's numbers in `ids` (plus the end).
    starts: Vec<u32>,
    ids: Vec<u32>,
}

/// The raw value of a score for a level, as stored in the table – the field
/// itself, so that [`values`] can be applied to it later. Multi-valued levels
/// keep their list.
fn raw_values<'a>(f: &'a ScoreFacets, dim: &str) -> Vec<&'a str> {
    match dim {
        "genre" => f.genres.iter().map(String::as_str).collect(),
        "instrumente" => f.families.iter().map(String::as_str).collect(),
        "schwierigkeit" => vec![&f.difficulty],
        "stimmung" => vec![&f.mood],
        "faerbung" => vec![&f.color],
        "energie" => vec![&f.energy],
        "tongeschlecht" => vec![&f.mode],
        "tonart" => vec![&f.key],
        "taktart" => vec![&f.meter_kind],
        "takt" => vec![&f.meter],
        "tempo" => vec![&f.tempo],
        "besetzung" => vec![&f.ensemble],
        "satz" => vec![&f.texture],
        "harmonik" => vec![&f.harmony],
        "laenge" => vec![&f.length],
        "komponist" => vec![&f.composer],
        _ => Vec::new(),
    }
}

/// Writes `klangwelt-facets.bin` (magic + deflated table).
pub fn write_facet_table(facets: &[ScoreFacets]) -> Vec<u8> {
    let mut order: Vec<&ScoreFacets> = facets.iter().collect();
    let key = |f: &ScoreFacets| if f.title.is_empty() { "\u{10FFFF}".to_string() } else { f.title.to_lowercase() };
    order.sort_by(|a, b| key(a).cmp(&key(b)).then(a.doc.cmp(&b.doc)));
    let mut out = Vec::new();
    put_varint(&mut out, order.len() as u64);
    for f in &order {
        put_varint(&mut out, f.doc as u64);
    }
    put_varint(&mut out, DIMENSIONS.len() as u64);
    for d in DIMENSIONS {
        let mut ids: BTreeMap<&str, u32> = BTreeMap::new();
        let mut list: Vec<&str> = Vec::new();
        let rows: Vec<Vec<u32>> = order
            .iter()
            .map(|f| {
                raw_values(f, d.key)
                    .into_iter()
                    .map(|v| {
                        *ids.entry(v).or_insert_with(|| {
                            list.push(v);
                            (list.len() - 1) as u32
                        })
                    })
                    .collect()
            })
            .collect();
        put_str(&mut out, d.key);
        out.push(u8::from(d.multi));
        put_varint(&mut out, list.len() as u64);
        for v in &list {
            put_str(&mut out, v);
        }
        for r in rows {
            if d.multi {
                put_varint(&mut out, r.len() as u64);
            }
            for id in r {
                put_varint(&mut out, id as u64);
            }
        }
    }
    packed(FACETS_MAGIC, &out)
}

impl FacetTable {
    pub fn parse(file: &[u8]) -> io::Result<Self> {
        let raw = unpacked(FACETS_MAGIC, file)?;
        let mut i = 0;
        let rows = get_varint(&raw, &mut i)? as usize;
        let docs = (0..rows).map(|_| get_varint(&raw, &mut i).map(|d| d as u32)).collect::<io::Result<Vec<_>>>()?;
        let n_dims = get_varint(&raw, &mut i)? as usize;
        let mut dims = Vec::with_capacity(n_dims);
        for _ in 0..n_dims {
            let key = get_str(&raw, &mut i)?;
            let multi = *raw.get(i).ok_or_else(corrupt)? != 0;
            i += 1;
            let n_values = get_varint(&raw, &mut i)? as usize;
            let values = (0..n_values).map(|_| get_str(&raw, &mut i)).collect::<io::Result<Vec<_>>>()?;
            let mut starts = Vec::with_capacity(rows + 1);
            let mut ids = Vec::with_capacity(rows);
            for _ in 0..rows {
                starts.push(ids.len() as u32);
                let n = if multi { get_varint(&raw, &mut i)? as usize } else { 1 };
                for _ in 0..n {
                    let id = get_varint(&raw, &mut i)? as u32;
                    if id as usize >= values.len() {
                        return Err(corrupt());
                    }
                    ids.push(id);
                }
            }
            starts.push(ids.len() as u32);
            dims.push(Column { key, multi, values, starts, ids });
        }
        Ok(Self { rows, docs, dims })
    }

    pub fn len(&self) -> usize {
        self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    fn column(&self, dim: &str) -> Option<&Column> {
        self.dims.iter().find(|c| c.key == dim)
    }

    /// Stored (raw) values of row `i` for a level.
    fn raw(&self, i: usize, dim: &str) -> Vec<&str> {
        let Some(c) = self.column(dim) else { return Vec::new() };
        c.ids[c.starts[i] as usize..c.starts[i + 1] as usize].iter().map(|&id| c.values[id as usize].as_str()).collect()
    }

    fn one(&self, i: usize, dim: &str) -> &str {
        self.column(dim).and_then(|c| c.ids.get(c.starts[i] as usize).filter(|_| !c.multi).map(|&id| c.values[id as usize].as_str())).unwrap_or("")
    }
}

impl FacetRows for FacetTable {
    fn row_count(&self) -> usize {
        self.rows
    }

    fn row_values(&self, i: usize, dim: &str) -> Vec<&str> {
        // the same substitutions as `facets::values`, on the stored fields
        let raw = self.raw(i, dim);
        match dim {
            "genre" if raw.is_empty() => vec!["ohne Angabe"],
            "genre" | "instrumente" => raw,
            "schwierigkeit" | "takt" => vec![raw.first().copied().filter(|v| !v.is_empty()).unwrap_or("ohne Angabe")],
            "tonart" => vec![raw.first().copied().filter(|v| !v.is_empty()).unwrap_or("unbestimmt")],
            "komponist" => vec![raw.first().map(|v| v.trim()).filter(|v| !v.is_empty()).unwrap_or("unbekannt")],
            _ => raw,
        }
    }

    fn row_doc(&self, i: usize) -> u32 {
        self.docs[i]
    }

    fn row_title(&self, _i: usize) -> &str {
        ""
    }

    fn row_composer(&self, i: usize) -> &str {
        self.one(i, "komponist")
    }

    fn row_field(&self, i: usize, dim: &str) -> &str {
        self.one(i, dim)
    }

    fn row_cmp(&self, a: usize, b: usize) -> Ordering {
        a.cmp(&b)
    }
}

// ---------------------------------------------------------------- chord statistics

/// Writes `klangwelt-chords.bin` (magic + deflated [`ChordStats::encode_groups`]).
pub fn write_chords(groups: &[ChordStats]) -> Vec<u8> {
    packed(CHORDS_MAGIC, &ChordStats::encode_groups(groups))
}

pub fn parse_chords(file: &[u8]) -> io::Result<Vec<ChordStats>> {
    ChordStats::decode_groups(&unpacked(CHORDS_MAGIC, file)?)
}

// ---------------------------------------------------------------- the package's parts

/// The package files next to a remote index, loaded on first use.
pub struct WebParts {
    text: Option<Box<dyn ReadAt>>,
    facets: Option<Box<dyn ReadAt>>,
    chords: Option<Box<dyn ReadAt>>,
    /// Number of scores and [major, minor] scores with chords (from the manifest).
    scores: usize,
    chord_scores: Option<[u32; 2]>,
    dict: OnceLock<TextDict>,
    table: OnceLock<FacetTable>,
    chord_stats: OnceLock<Vec<ChordStats>>,
}

impl WebParts {
    pub fn new(text: Option<Box<dyn ReadAt>>, facets: Option<Box<dyn ReadAt>>, chords: Option<Box<dyn ReadAt>>, scores: usize, chord_scores: Option<[u32; 2]>) -> Self {
        Self { text, facets, chords, scores, chord_scores, dict: OnceLock::new(), table: OnceLock::new(), chord_stats: OnceLock::new() }
    }

    pub fn score_count(&self) -> usize {
        self.scores
    }

    pub fn chord_scores(&self) -> Option<[u32; 2]> {
        self.chord_scores.filter(|_| self.chords.is_some())
    }

    pub fn has_tree(&self) -> bool {
        self.facets.is_some()
    }

    fn whole(src: &dyn ReadAt) -> io::Result<Vec<u8>> {
        read_vec(src, 0, src.size()?)
    }

    /// The facet table (read whole on first use; `None` while it's missing).
    pub fn table(&self) -> Option<&FacetTable> {
        if let Some(t) = self.table.get() {
            return Some(t);
        }
        let t = FacetTable::parse(&Self::whole(self.facets.as_deref()?).ok()?).ok()?;
        let _ = self.table.set(t);
        self.table.get()
    }

    /// The chord statistics per group (read whole on first use).
    pub fn chords(&self) -> Option<&Vec<ChordStats>> {
        if let Some(c) = self.chord_stats.get() {
            return Some(c);
        }
        let c = parse_chords(&Self::whole(self.chords.as_deref()?).ok()?).ok()?;
        let _ = self.chord_stats.set(c);
        self.chord_stats.get()
    }

    fn dict(&self) -> Option<&TextDict> {
        if let Some(d) = self.dict.get() {
            return Some(d);
        }
        let d = TextDict::read(self.text.as_deref()?).ok()?;
        let _ = self.dict.set(d);
        self.dict.get()
    }

    /// Text search: scores (doc of the first voice) containing every word.
    pub fn search_text(&self, query: &str, limit: usize) -> Vec<u32> {
        let (Some(dict), Some(src)) = (self.dict(), self.text.as_deref()) else { return Vec::new() };
        dict.search(src, query, limit).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facets::{tree_children, tree_children_rows};

    fn f(doc: u32, title: &str, composer: &str, genres: &[&str], families: &[&str], key: &str) -> ScoreFacets {
        ScoreFacets {
            doc,
            title: title.into(),
            composer: composer.into(),
            genres: genres.iter().map(|s| s.to_string()).collect(),
            families: families.iter().map(|s| s.to_string()).collect(),
            parts: vec!["Violine".into(), "Violoncello".into()],
            key: key.into(),
            mood: "heiter · ruhig".into(),
            meter: if doc % 2 == 0 { "3/4".into() } else { String::new() },
            ensemble: "Duo".into(),
            tempo: "mäßig".into(),
            ..Default::default()
        }
    }

    fn sample() -> Vec<ScoreFacets> {
        vec![
            f(0, "Ständchen", "Schubert, Franz", &["Klassik"], &["Gesang", "Tasteninstrumente"], "d-Moll"),
            f(3, "Ave Maria", "Schubert, Franz", &["Klassik", "Geistliche Musik"], &["Gesang"], ""),
            f(5, "", "", &[], &["Streicher"], "C-Dur"),
            f(9, "Quartett Nr. 4", "  ", &["Klassik"], &["Streicher"], "c-Moll"),
        ]
    }

    #[test]
    fn text_index_finds_what_the_facet_search_finds() {
        let facets = sample();
        let bytes = write_text_index(&facets);
        let dict = TextDict::read(&bytes).unwrap();
        let find = |q: &str| dict.search(&bytes, q, 10).unwrap();
        assert_eq!(find("schubert"), vec![0, 3]);
        assert_eq!(find("bert  maria"), vec![3], "parts of words, all words");
        assert_eq!(find("CELLO"), vec![0, 3, 5, 9]);
        assert_eq!(find("moll"), vec![0, 9]);
        assert_eq!(find("nr."), vec![9]);
        assert!(find("mozart").is_empty());
        assert_eq!(dict.search(&bytes, "klassik", 2).unwrap(), vec![0, 3], "limit");
        // the same as the desktop's search over the facets
        for q in ["schubert", "ave", "streicher", "geistliche musik", "c-dur", "4"] {
            let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
            let want: Vec<u32> = facets.iter().filter(|x| words.iter().all(|w| score_terms(x).iter().any(|t| t.contains(w.as_str())))).map(|x| x.doc).collect();
            assert_eq!(find(q), want, "{q}");
        }
    }

    #[test]
    fn facet_table_gives_the_same_tree_as_the_facets() {
        let facets = sample();
        let table = FacetTable::parse(&write_facet_table(&facets)).unwrap();
        assert_eq!(table.len(), 4);
        for levels in [vec!["genre", "komponist"], vec!["instrumente", "tonart"], vec!["takt", "besetzung"], vec!["komponist"]] {
            let top = tree_children(&facets, &levels, &[], 0, 50);
            let got = tree_children_rows(&table, &levels, &[], 0, 50);
            assert_eq!(got.nodes, top.nodes, "{levels:?}");
            for n in &top.nodes {
                let path = vec![n.value.clone()];
                let a = tree_children(&facets, &levels, &path, 0, 50);
                let b = tree_children_rows(&table, &levels, &path, 0, 50);
                assert_eq!((a.matching, a.total), (b.matching, b.total));
                let full: Vec<String> = levels.iter().map(|_| String::new()).collect();
                let _ = full;
            }
        }
        // listed scores: same order (title), same docs and facts; titles come later
        let a = tree_children(&facets, &["besetzung"], &["Duo".to_string()], 0, 50);
        let b = tree_children_rows(&table, &["besetzung"], &["Duo".to_string()], 0, 50);
        assert_eq!(a.scores.iter().map(|s| (s.doc, s.facts.clone(), s.composer.clone())).collect::<Vec<_>>(), b.scores.iter().map(|s| (s.doc, s.facts.clone(), s.composer.clone())).collect::<Vec<_>>());
        assert!(b.scores.iter().all(|s| s.title.is_empty()));
    }

    #[test]
    fn chord_file_round_trip() {
        let groups = vec![ChordStats::default(); 3];
        assert_eq!(parse_chords(&write_chords(&groups)).unwrap().len(), 3);
        assert!(parse_chords(b"KWXX01\0\0abc").is_err());
    }

    /// A word that matches many terms ("bach" in "offenbach", "bachmann", …)
    /// fetches all their lists in one round, not one list per round.
    #[test]
    fn remote_text_search_fetches_all_lists_in_one_round() {
        use crate::source::remote::{self, RemoteFile};
        let facets: Vec<ScoreFacets> = (0..300).map(|i| f(i * 2, &format!("Stück {i}"), &format!("Bach{i}"), &[], &[], "")).collect();
        let bytes = write_text_index(&facets);
        let name = format!("text-rounds-{}", std::process::id());
        let file = RemoteFile::new(&name, bytes.len() as u64);
        let mut rounds = 0;
        let (dict, hits) = loop {
            remote::begin();
            let dict = TextDict::read(&file).ok();
            let hits = dict.as_ref().map(|d| d.search(&file, "bach", 1000));
            let need = remote::take_missing();
            if need.is_empty() {
                break (dict.unwrap(), hits.unwrap().unwrap());
            }
            rounds += 1;
            assert!(rounds < 10, "no progress");
            for n in need {
                assert!(remote::feed(&n.file, n.start, bytes[n.start as usize..n.end as usize].to_vec()));
            }
        };
        assert!(dict.len() > 600, "bachN, N, stück, parts, mood");
        assert_eq!(hits.len(), 300);
        assert_eq!(rounds, 3, "header, terms, then all 300 lists at once");
    }
}
