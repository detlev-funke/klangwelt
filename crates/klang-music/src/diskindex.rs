//! Melody index on disk (format KWIX04) – for large libraries such as
//! the whole PDMX dataset (≈ 254,000 scores, ≈ 200 million occurrences).
//!
//! **Building** ([`IndexWriter`]): voices are added continuously.
//! Voice entries and feature sequences go straight into temporary files, while the
//! occurrences (n-gram, voice, position) collect in a block that is written to disk
//! sorted as a "run" as soon as it is full.
//! At the end all runs are merged (k-way merge). Memory
//! use stays bounded (one block ≈ 130 MB).
//!
//! **File** (all numbers little-endian):
//! header → voice entries (JSON per voice) + offsets → feature sequences (2 bytes per
//! interval/rhythm) + offsets → features per score (JSON) → occurrences
//! (per n-gram: voice as a difference and position, both as varints) →
//! key table (n-gram, offset, length; sorted) → sparse
//! table of contents (every 64th key) → chord progression statistics
//! ([`ChordStats`], since KWIX04; the last header value points to it, 0 = none).
//!
//! **Reading** ([`DiskIndex`]): Only the offsets, the sparse
//! table of contents, the features for the tree and the chord statistics are held in memory.
//! A search reads the lists of its n-grams and the feature sequences of the
//! best candidates. KWIX03 files (without chord statistics) remain readable.

use crate::chordstats::{composer_groups, ChordStats, Progression, GROUPS};
use crate::facets::{tree_children, tree_children_rows, ScoreFacets, TreeLevel};
use crate::index::{gram_key, search_in, DocInfo, MelodyIndex, Posting, SearchData, SearchHit, LEVELS, MAX_POSTINGS_PER_GRAM};
use crate::melody::{features, Feature, MelodyNote};
use crate::source::{read_vec, ReadAt};
use crate::webpack::WebParts;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

pub const MAGIC: &[u8; 6] = b"KWIX04";
/// Predecessor without chord statistics – still readable.
const MAGIC_V3: &[u8; 6] = b"KWIX03";
/// Magic + 2 padding bytes + 13 numbers.
const HEADER_LEN: usize = 8 + 13 * 8;
/// Occurrences per block before it is written as a run (16 bytes per entry).
const CHUNK: usize = 8_000_000;
/// Every how-many-th key goes into the table of contents.
const SPARSE_STEP: u64 = 64;
/// Bytes per key table entry: n-gram (8), offset (8), length (4).
const KEY_ENTRY: usize = 20;

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(buf: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *buf.get(*i)?;
        *i += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b < 0x80 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// Reads exactly `buf.len()` bytes from `off` (file, memory or remote file).
fn read_at(f: &dyn ReadAt, buf: &mut [u8], off: u64) -> io::Result<()> {
    f.read_at(buf, off)
}

fn u64s(bytes: &[u8]) -> Vec<u64> {
    bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

// ---------------------------------------------------------------- Writing

/// Statistics of a finished index.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriteStats {
    pub docs: u64,
    pub keys: u64,
    pub postings: u64,
    pub runs: usize,
}

pub struct IndexWriter {
    tmp: PathBuf,
    /// Temporary files (closed on completion – Windows doesn't delete open files).
    docs: Option<BufWriter<File>>,
    docs_idx: Vec<u64>,
    seqs: Option<BufWriter<File>>,
    seqs_idx: Vec<u64>,
    facets: Vec<ScoreFacets>,
    /// Chord statistics per group (see [`GROUPS`]).
    chords: Vec<ChordStats>,
    chunk: Vec<(u64, u32, u32)>,
    runs: Vec<PathBuf>,
    chunk_limit: usize,
    /// Provenance (JSON) between header and voice entries, see [`DiskIndex::provenance`].
    provenance: Vec<u8>,
}

impl IndexWriter {
    /// `tmp`: folder for temporary files (created, and deleted at the end).
    pub fn new(tmp: &Path) -> io::Result<Self> {
        Self::with_chunk(tmp, CHUNK)
    }

    fn with_chunk(tmp: &Path, chunk_limit: usize) -> io::Result<Self> {
        let _ = std::fs::remove_dir_all(tmp);
        std::fs::create_dir_all(tmp)?;
        Ok(Self {
            tmp: tmp.to_path_buf(),
            docs: Some(BufWriter::new(File::create(tmp.join("stimmen.json"))?)),
            docs_idx: vec![0],
            seqs: Some(BufWriter::new(File::create(tmp.join("folgen.bin"))?)),
            seqs_idx: vec![0],
            facets: Vec::new(),
            chords: vec![ChordStats::default(); GROUPS.len()],
            chunk: Vec::new(),
            runs: Vec::new(),
            chunk_limit,
            provenance: Vec::new(),
        })
    }

    /// Provenance of the index (a JSON object: sources, versions, checksums,
    /// builder). It is written between the header and the voice entries; every
    /// KWIX04 reader skips it, because all sections are found by their offsets.
    pub fn set_provenance(&mut self, json: &str) {
        self.provenance = json.as_bytes().to_vec();
    }

    /// Chord progression of a score for the statistics (doc = its first voice);
    /// `composer` additionally assigns it to the Baroque/Bach groups.
    pub fn add_progression(&mut self, doc: u32, p: &Progression, composer: &str) {
        for (g, member) in composer_groups(composer).into_iter().enumerate() {
            if member {
                self.chords[g].add(doc, p);
            }
        }
    }

    pub fn doc_count(&self) -> u32 {
        (self.docs_idx.len() - 1) as u32
    }

    /// Adds a melody line (like [`MelodyIndex::add_melody`]: lines that are too short don't count).
    pub fn add_melody(&mut self, mut info: DocInfo, notes: &[MelodyNote]) -> io::Result<Option<u32>> {
        if notes.len() < 8 {
            return Ok(None);
        }
        let f = features(notes);
        let doc = self.doc_count();
        info.note_count = notes.len() as u32;
        let closed = || io::Error::other(klang_i18n::tr!("Index is already finalized"));
        let json = serde_json::to_vec(&info).map_err(io::Error::other)?;
        self.docs.as_mut().ok_or_else(closed)?.write_all(&json)?;
        self.docs_idx.push(self.docs_idx.last().unwrap() + json.len() as u64);
        let raw: Vec<u8> = f.iter().flat_map(|x| [x.interval as u8, x.rhythm as u8]).collect();
        self.seqs.as_mut().ok_or_else(closed)?.write_all(&raw)?;
        self.seqs_idx.push(self.seqs_idx.last().unwrap() + raw.len() as u64);
        for level in LEVELS {
            if f.len() < level.n {
                continue;
            }
            for pos in 0..=f.len() - level.n {
                self.chunk.push((gram_key(level.id, &f[pos..pos + level.n]), doc, pos as u32));
            }
        }
        // Only flush after a whole voice: a voice never spans two runs.
        if self.chunk.len() >= self.chunk_limit {
            self.flush_chunk()?;
        }
        Ok(Some(doc))
    }

    pub fn add_facets(&mut self, f: ScoreFacets) {
        self.facets.push(f);
    }

    fn flush_chunk(&mut self) -> io::Result<()> {
        if self.chunk.is_empty() {
            return Ok(());
        }
        self.chunk.sort_unstable();
        let path = self.tmp.join(format!("lauf-{:04}.bin", self.runs.len()));
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        let mut i = 0;
        while i < self.chunk.len() {
            let key = self.chunk[i].0;
            let mut j = i;
            while j < self.chunk.len() && self.chunk[j].0 == key {
                j += 1;
            }
            w.write_all(&key.to_le_bytes())?;
            w.write_all(&((j - i) as u32).to_le_bytes())?;
            for &(_, d, p) in &self.chunk[i..j] {
                w.write_all(&d.to_le_bytes())?;
                w.write_all(&p.to_le_bytes())?;
            }
            i = j;
        }
        w.flush()?;
        self.runs.push(path);
        self.chunk.clear();
        Ok(())
    }

    /// Merges everything into the finished index file `out`.
    pub fn finish(mut self, out: &Path, cancel: &AtomicBool) -> io::Result<WriteStats> {
        self.flush_chunk()?;
        self.chunk = Vec::new();
        for w in [self.docs.take(), self.seqs.take()].into_iter().flatten() {
            w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        }
        let part = out.with_extension("kwix.teil");
        let result = self.write_final(&part, cancel);
        let _ = std::fs::remove_dir_all(&self.tmp);
        match result {
            Ok(stats) => {
                std::fs::rename(&part, out)?;
                Ok(stats)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(e)
            }
        }
    }

    fn write_final(&mut self, part: &Path, cancel: &AtomicBool) -> io::Result<WriteStats> {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(part)?);
        w.write_all(&[0u8; HEADER_LEN])?;
        w.write_all(&self.provenance)?;
        let mut pos = (HEADER_LEN + self.provenance.len()) as u64;
        let copy = |w: &mut BufWriter<File>, path: &Path| -> io::Result<u64> { io::copy(&mut BufReader::new(File::open(path)?), w) };

        let docs_off = pos;
        pos += copy(&mut w, &self.tmp.join("stimmen.json"))?;
        let docs_idx_off = pos;
        for o in &self.docs_idx {
            w.write_all(&o.to_le_bytes())?;
        }
        pos += 8 * self.docs_idx.len() as u64;
        let seqs_off = pos;
        pos += copy(&mut w, &self.tmp.join("folgen.bin"))?;
        let seqs_idx_off = pos;
        for o in &self.seqs_idx {
            w.write_all(&o.to_le_bytes())?;
        }
        pos += 8 * self.seqs_idx.len() as u64;
        let facets_off = pos;
        let facets = serde_json::to_vec(&self.facets).map_err(io::Error::other)?;
        w.write_all(&facets)?;
        pos += facets.len() as u64;

        // Merge runs: for each n-gram, all occurrences in voice order.
        let post_off = pos;
        let keys_path = self.tmp.join("schluessel.bin");
        let mut keys = BufWriter::with_capacity(1 << 20, File::create(&keys_path)?);
        let mut sparse: Vec<u64> = Vec::new();
        let mut readers = self.runs.iter().map(|p| RunReader::open(p)).collect::<io::Result<Vec<_>>>()?;
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
        for (i, r) in readers.iter().enumerate() {
            if let Some(k) = r.key() {
                heap.push(Reverse((k, i)));
            }
        }
        let (mut n_keys, mut n_post, mut rel) = (0u64, 0u64, 0u64);
        let mut list: Vec<(u32, u32)> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut from: Vec<usize> = Vec::new();
        while let Some(&Reverse((key, _))) = heap.peek() {
            if n_keys % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, klang_i18n::tr!("canceled")));
            }
            from.clear();
            while let Some(&Reverse((k, i))) = heap.peek() {
                if k != key {
                    break;
                }
                heap.pop();
                from.push(i);
            }
            from.sort_unstable(); // Runs are ordered by time → voices ascending
            list.clear();
            for &i in &from {
                readers[i].read_group(&mut list)?;
                if let Some(k) = readers[i].key() {
                    heap.push(Reverse((k, i)));
                }
            }
            // As in the in-memory index: the search skips n-grams that are too frequent, so don't store them.
            if list.len() > MAX_POSTINGS_PER_GRAM {
                continue;
            }
            buf.clear();
            let mut prev = 0u32;
            for &(d, p) in &list {
                put_varint(&mut buf, (d - prev) as u64);
                put_varint(&mut buf, p as u64);
                prev = d;
            }
            w.write_all(&buf)?;
            if n_keys % SPARSE_STEP == 0 {
                sparse.push(key);
            }
            keys.write_all(&key.to_le_bytes())?;
            keys.write_all(&rel.to_le_bytes())?;
            keys.write_all(&(buf.len() as u32).to_le_bytes())?;
            rel += buf.len() as u64;
            n_keys += 1;
            n_post += list.len() as u64;
        }
        drop(readers);
        pos += rel;
        keys.flush()?;
        drop(keys);
        let keys_off = pos;
        pos += copy(&mut w, &keys_path)?;
        let sparse_off = pos;
        for k in &sparse {
            w.write_all(&k.to_le_bytes())?;
        }
        pos += 8 * sparse.len() as u64;
        let chords_off = pos;
        let chords = ChordStats::encode_groups(&self.chords);
        w.write_all(&chords)?;
        pos += chords.len() as u64;
        let n_docs = (self.docs_idx.len() - 1) as u64;

        let mut f = w.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(0))?;
        let mut head = Vec::with_capacity(HEADER_LEN);
        head.extend_from_slice(MAGIC);
        head.extend_from_slice(&[0, 0]);
        for v in [docs_off, docs_idx_off, seqs_off, seqs_idx_off, facets_off, post_off, keys_off, sparse_off, pos, n_docs, n_keys, n_post, chords_off] {
            head.extend_from_slice(&v.to_le_bytes());
        }
        f.write_all(&head)?;
        f.sync_all()?;
        Ok(WriteStats { docs: n_docs, keys: n_keys, postings: n_post, runs: self.runs.len() })
    }
}

/// Reads a run group by group: (n-gram, count, count × (voice, position)).
struct RunReader {
    r: BufReader<File>,
    next: Option<(u64, u32)>,
}

impl RunReader {
    fn open(path: &Path) -> io::Result<Self> {
        let mut me = Self { r: BufReader::with_capacity(1 << 18, File::open(path)?), next: None };
        me.advance()?;
        Ok(me)
    }

    fn advance(&mut self) -> io::Result<()> {
        let mut head = [0u8; 12];
        match self.r.read_exact(&mut head) {
            Ok(()) => {
                self.next = Some((u64::from_le_bytes(head[..8].try_into().unwrap()), u32::from_le_bytes(head[8..].try_into().unwrap())));
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                self.next = None;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn key(&self) -> Option<u64> {
        self.next.map(|n| n.0)
    }

    fn read_group(&mut self, into: &mut Vec<(u32, u32)>) -> io::Result<()> {
        let Some((_, count)) = self.next else { return Ok(()) };
        let mut raw = vec![0u8; count as usize * 8];
        self.r.read_exact(&mut raw)?;
        into.extend(raw.chunks_exact(8).map(|c| (u32::from_le_bytes(c[..4].try_into().unwrap()), u32::from_le_bytes(c[4..].try_into().unwrap()))));
        self.advance()
    }
}

// ---------------------------------------------------------------- Reading

/// Offsets of the voice entries or feature sequences: in memory, or – for a
/// large remote index in the browser – read whole on first use instead of at
/// opening. Read whole, a table costs one range once (the browser keeps it);
/// read entry by entry, every search would need a round of requests more.
enum Offsets {
    All(Vec<u64>),
    Later { at: u64, len: u64, loaded: OnceLock<Vec<u64>> },
}

/// Up to this size, the offset tables of a remote index are loaded at
/// opening; larger ones with the first search.
const SMALL_TABLE: u64 = 256 << 10;

impl Offsets {
    fn later(at: u64, len: u64) -> Self {
        Offsets::Later { at, len, loaded: OnceLock::new() }
    }

    /// The whole table (`None` while a remote table is still missing).
    fn load(&self, src: &dyn ReadAt) -> Option<&[u64]> {
        match self {
            Offsets::All(v) => Some(v),
            Offsets::Later { at, len, loaded } => {
                if let Some(v) = loaded.get() {
                    return Some(v);
                }
                let raw = read_vec(src, *at, *len).ok()?;
                let _ = loaded.set(u64s(&raw));
                loaded.get().map(Vec::as_slice)
            }
        }
    }

    /// (start, end) of entry `i` (of `n`) relative to its section.
    fn span(&self, src: &dyn ReadAt, i: usize, n: usize) -> Option<(u64, u64)> {
        if i >= n {
            return None;
        }
        let v = self.load(src)?;
        Some((*v.get(i)?, *v.get(i + 1)?))
    }

    fn all(&self) -> io::Result<&[u64]> {
        match self {
            Offsets::All(v) => Ok(v),
            Offsets::Later { loaded, .. } => loaded.get().map(Vec::as_slice).ok_or_else(|| io::Error::other("remote index: offset table not loaded")),
        }
    }
}

/// The 13 numbers of the header.
struct Header {
    docs_off: u64,
    docs_idx_off: u64,
    seqs_off: u64,
    seqs_idx_off: u64,
    facets_off: u64,
    post_off: u64,
    keys_off: u64,
    sparse_off: u64,
    sparse_end: u64,
    end: u64,
    n_docs: u64,
    n_keys: u64,
    chords_off: u64,
}

fn read_header(file: &dyn ReadAt) -> io::Result<Header> {
    let mut head = [0u8; HEADER_LEN];
    read_at(file, &mut head, 0)?;
    let v4 = &head[..6] == MAGIC;
    if !v4 && &head[..6] != MAGIC_V3 {
        return Err(invalid(&klang_i18n::tr!("not a Klangwelt index file (format 3/4)")));
    }
    let v = u64s(&head[8..]);
    let [docs_off, docs_idx_off, seqs_off, seqs_idx_off, facets_off, post_off, keys_off, sparse_off, end, n_docs, n_keys, _n_post, chords_off] =
        v[..13].try_into().map_err(|_| invalid(&klang_i18n::tr!("Header corrupted")))?;
    let chords_off = if v4 { chords_off } else { 0 };
    let sparse_end = if chords_off > 0 { chords_off } else { end };
    if file.size()? < end
        || !(HEADER_LEN as u64 <= docs_off
            && docs_off <= docs_idx_off
            && docs_idx_off <= seqs_off
            && seqs_off <= seqs_idx_off
            && seqs_idx_off <= facets_off
            && facets_off <= post_off
            && post_off <= keys_off
            && keys_off <= sparse_off
            && sparse_off <= sparse_end
            && sparse_end <= end)
    {
        return Err(invalid(&klang_i18n::tr!("Index file incomplete or corrupted")));
    }
    if keys_off + n_keys * KEY_ENTRY as u64 != sparse_off || docs_idx_off + 8 * (n_docs + 1) > seqs_off {
        return Err(invalid(&klang_i18n::tr!("Key table corrupted")));
    }
    Ok(Header { docs_off, docs_idx_off, seqs_off, seqs_idx_off, facets_off, post_off, keys_off, sparse_off, sparse_end, end, n_docs, n_keys, chords_off })
}

/// Provenance between header and voice entries (if the index has one).
fn provenance_range(h: &Header) -> Option<(u64, u64)> {
    let len = h.docs_off - HEADER_LEN as u64;
    (len > 0 && len <= 4 << 20).then_some((HEADER_LEN as u64, len))
}

fn parse_provenance(raw: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(raw).ok()?.trim();
    s.starts_with('{').then(|| s.to_string())
}

/// The provenance of an index file, reading only the header and the JSON
/// (without opening the index).
pub fn read_provenance(file: &dyn ReadAt) -> io::Result<Option<String>> {
    let h = read_header(file)?;
    Ok(match provenance_range(&h) {
        Some((off, len)) => parse_provenance(&read_vec(file, off, len)?),
        None => None,
    })
}

pub struct DiskIndex {
    file: Box<dyn ReadAt>,
    n_docs: usize,
    docs_off: u64,
    docs_idx: Offsets,
    seqs_off: u64,
    seqs_idx: Offsets,
    post_off: u64,
    keys_off: u64,
    n_keys: u64,
    sparse_off: u64,
    sparse_end: u64,
    /// Sparse table of contents – loaded on opening, for a remote
    /// index on the first melody search.
    sparse: OnceLock<Vec<u64>>,
    facets: Vec<ScoreFacets>,
    chords: Vec<ChordStats>,
    provenance: Option<String>,
    /// Package for the web (remote index): text index, tree, chord statistics.
    web: Option<Box<WebParts>>,
    /// The ZIP with the scores, if there is one ([`crate::scorezip`]).
    scores: Option<Box<dyn ReadAt>>,
}

impl DiskIndex {
    /// Opens an index file. Lies the ZIP of a package next to it
    /// ([`crate::webpack::SCORES_FILE`]), scores are read from there.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut me = Self::open_source(Box::new(File::open(path)?))?;
        let zip = path.with_file_name(crate::webpack::SCORES_FILE);
        if zip.is_file() {
            if let Ok(f) = File::open(&zip) {
                me.scores = Some(Box::new(f));
            }
        }
        Ok(me)
    }

    /// Opens an index from any byte source; offsets, key directory, facets and
    /// chord statistics are loaded into memory.
    pub fn open_source(file: Box<dyn ReadAt>) -> io::Result<Self> {
        let h = read_header(&*file)?;
        let docs_idx = u64s(&read_vec(&*file, h.docs_idx_off, 8 * (h.n_docs + 1))?);
        let seqs_idx = u64s(&read_vec(&*file, h.seqs_idx_off, 8 * (h.n_docs + 1))?);
        let facets: Vec<ScoreFacets> = serde_json::from_slice(&read_vec(&*file, h.facets_off, h.post_off - h.facets_off)?).map_err(io::Error::other)?;
        let sparse = u64s(&read_vec(&*file, h.sparse_off, h.sparse_end - h.sparse_off)?);
        let chords = if h.chords_off > 0 { ChordStats::decode_groups(&read_vec(&*file, h.chords_off, h.end - h.chords_off)?)? } else { Vec::new() };
        let provenance = match provenance_range(&h) {
            Some((at, len)) => parse_provenance(&read_vec(&*file, at, len)?),
            None => None,
        };
        Ok(Self {
            n_docs: h.n_docs as usize,
            docs_off: h.docs_off,
            docs_idx: Offsets::All(docs_idx),
            seqs_off: h.seqs_off,
            seqs_idx: Offsets::All(seqs_idx),
            post_off: h.post_off,
            keys_off: h.keys_off,
            n_keys: h.n_keys,
            sparse_off: h.sparse_off,
            sparse_end: h.sparse_end,
            sparse: OnceLock::from(sparse),
            facets,
            chords,
            provenance,
            web: None,
            scores: None,
            file,
        })
    }

    /// Opens a remote index (browser): now only the header, the provenance
    /// and – for small indexes – the offset tables; everything else when a
    /// search needs it. Text search, tree and chord statistics come from the
    /// package's own files (`web`), scores from its ZIP. While data is still
    /// missing this fails with `WouldBlock` ([`crate::source::remote`]).
    pub fn open_remote(file: Box<dyn ReadAt>, web: WebParts, scores: Option<Box<dyn ReadAt>>) -> io::Result<Self> {
        Self::open_remote_with(file, web, scores, SMALL_TABLE)
    }

    fn open_remote_with(file: Box<dyn ReadAt>, web: WebParts, scores: Option<Box<dyn ReadAt>>, small_table: u64) -> io::Result<Self> {
        let h = read_header(&*file)?;
        let table = 8 * (h.n_docs + 1);
        // all reads of this round first, then the errors (one round of requests)
        let prov = provenance_range(&h).map(|(at, len)| read_vec(&*file, at, len));
        let (docs_idx, seqs_idx) = if table <= small_table {
            let d = read_vec(&*file, h.docs_idx_off, table);
            let s = read_vec(&*file, h.seqs_idx_off, table);
            (Offsets::All(u64s(&d?)), Offsets::All(u64s(&s?)))
        } else {
            (Offsets::later(h.docs_idx_off, table), Offsets::later(h.seqs_idx_off, table))
        };
        let provenance = match prov {
            Some(raw) => parse_provenance(&raw?),
            None => None,
        };
        Ok(Self {
            n_docs: h.n_docs as usize,
            docs_off: h.docs_off,
            docs_idx,
            seqs_off: h.seqs_off,
            seqs_idx,
            post_off: h.post_off,
            keys_off: h.keys_off,
            n_keys: h.n_keys,
            sparse_off: h.sparse_off,
            sparse_end: h.sparse_end,
            sparse: OnceLock::new(),
            facets: Vec::new(),
            chords: Vec::new(),
            provenance,
            web: Some(Box::new(web)),
            scores,
            file,
        })
    }

    /// Provenance written by the package builder (JSON), if any.
    pub fn provenance(&self) -> Option<&str> {
        self.provenance.as_deref()
    }

    /// Is this a remote index (browser) with the package's own files?
    pub fn is_remote(&self) -> bool {
        self.web.is_some()
    }

    /// Chord progression statistics of all pieces (None for index files before KWIX04).
    pub fn chords(&self) -> Option<&ChordStats> {
        self.chords_group(0)
    }

    /// Statistics of a group (0 = all, 1 = Baroque, 2 = Bach), if present.
    /// A remote index loads it on first use.
    pub fn chords_group(&self, g: usize) -> Option<&ChordStats> {
        match &self.web {
            Some(w) => w.chords()?.get(g),
            None => self.chords.get(g),
        }
    }

    /// Scores with a chord progression [major, minor] – without loading anything.
    pub fn chord_counts(&self) -> Option<[u32; 2]> {
        match &self.web {
            Some(w) => w.chord_scores(),
            None => self.chords.first().map(|c| c.scores),
        }
    }

    pub fn doc_count(&self) -> usize {
        self.n_docs
    }

    pub fn gram_count(&self) -> usize {
        self.n_keys as usize
    }

    /// Facets of all scores in memory (empty for a remote index – its tree
    /// comes from the package's facet table, see [`DiskIndex::tree`]).
    pub fn facets(&self) -> &[ScoreFacets] {
        &self.facets
    }

    /// Number of scores (with a searchable voice).
    pub fn score_count(&self) -> usize {
        match &self.web {
            Some(w) => w.score_count(),
            None => self.facets.len(),
        }
    }

    /// Scores in the tree (0 = index without facets).
    pub fn tree_score_count(&self) -> usize {
        match &self.web {
            Some(w) if w.has_tree() => w.score_count(),
            Some(_) => 0,
            None => self.facets.len(),
        }
    }

    pub fn doc(&self, id: u32) -> Option<DocInfo> {
        let (a, b) = self.docs_idx.span(&*self.file, id as usize, self.n_docs)?;
        let raw = read_vec(&*self.file, self.docs_off + a, b - a).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// The key directory (a remote index reads it on first use).
    fn sparse(&self) -> Option<&[u64]> {
        if let Some(s) = self.sparse.get() {
            return Some(s);
        }
        let raw = read_vec(&*self.file, self.sparse_off, self.sparse_end - self.sparse_off).ok()?;
        let _ = self.sparse.set(u64s(&raw));
        self.sparse.get().map(Vec::as_slice)
    }

    /// Key → (offset, length) of the occurrences.
    fn lookup(&self, key: u64) -> Option<(u64, u32)> {
        let sparse = self.sparse()?;
        let block = sparse.partition_point(|&k| k <= key).checked_sub(1)?;
        let start = block as u64 * SPARSE_STEP;
        let end = (start + SPARSE_STEP).min(self.n_keys);
        let raw = read_vec(&*self.file, self.keys_off + start * KEY_ENTRY as u64, (end - start) * KEY_ENTRY as u64).ok()?;
        let entries: Vec<(u64, u64, u32)> = raw
            .chunks_exact(KEY_ENTRY)
            .map(|c| (u64::from_le_bytes(c[..8].try_into().unwrap()), u64::from_le_bytes(c[8..16].try_into().unwrap()), u32::from_le_bytes(c[16..].try_into().unwrap())))
            .collect();
        let i = entries.binary_search_by_key(&key, |e| e.0).ok()?;
        Some((entries[i].1, entries[i].2))
    }

    pub fn search(&self, query: &[MelodyNote], limit: usize) -> Vec<SearchHit> {
        if self.web.is_some() {
            // a remote index: what every search needs, in the first round together
            let _ = self.sparse();
            let _ = self.seqs_idx.load(&*self.file);
            let _ = self.docs_idx.load(&*self.file);
        }
        search_in(self, query, limit)
    }

    /// Text search over title, composer, voice names, genre, instrument groups,
    /// key and mood (per score; all words must occur).
    pub fn search_text(&self, query: &str, limit: usize) -> Vec<(u32, DocInfo)> {
        if self.web.is_some() {
            // the hits' voice entries need it: fetched in the round of the terms
            let _ = self.docs_idx.load(&*self.file);
        }
        let docs: Vec<u32> = match &self.web {
            Some(w) => w.search_text(query, limit),
            None => facet_text_search(self.facets.iter(), query).take(limit).collect(),
        };
        docs.into_iter().filter_map(|doc| self.doc(doc).map(|d| (doc, d))).collect()
    }

    /// Children of a tree node (see [`crate::facets::tree_children`]); a remote
    /// index uses the package's facet table and reads the titles of the listed scores.
    pub fn tree(&self, levels: &[&str], path: &[String], offset: usize, limit: usize) -> TreeLevel {
        match &self.web {
            Some(w) => {
                let Some(table) = w.table() else { return TreeLevel::default() };
                let mut level = tree_children_rows(table, levels, path, offset, limit);
                self.fill_titles(&mut level);
                level
            }
            None => tree_children(&self.facets, levels, path, offset, limit),
        }
    }

    /// Titles of listed scores that the facet table doesn't carry (from the voice entries).
    fn fill_titles(&self, level: &mut TreeLevel) {
        let infos: Vec<Option<DocInfo>> = level.scores.iter().map(|s| if s.title.is_empty() { self.doc(s.doc) } else { None }).collect();
        for (s, info) in level.scores.iter_mut().zip(infos) {
            if let Some(i) = info {
                s.title = i.title.trim().to_string();
            }
        }
    }

    /// The score of a voice entry from the package's ZIP: `None` if it doesn't
    /// lie in a ZIP (then `info.source` is a path).
    pub fn score_bytes(&self, info: &DocInfo) -> Option<io::Result<(String, Vec<u8>)>> {
        let at = info.zip?;
        let zip = self.scores.as_deref()?;
        Some(crate::scorezip::read_entry(zip, at))
    }
}

impl SearchData for DiskIndex {
    fn postings(&self, key: u64) -> Option<Vec<Posting>> {
        let (rel, len) = self.lookup(key)?;
        let raw = read_vec(&*self.file, self.post_off + rel, len as u64).ok()?;
        let mut out = Vec::new();
        let (mut i, mut doc) = (0usize, 0u64);
        while i < raw.len() {
            doc += get_varint(&raw, &mut i)?;
            let pos = get_varint(&raw, &mut i)?;
            out.push(Posting { doc: doc as u32, pos: pos as u32 });
        }
        Some(out)
    }

    fn seq(&self, doc: u32) -> Vec<Feature> {
        let Some((a, b)) = self.seqs_idx.span(&*self.file, doc as usize, self.n_docs) else { return Vec::new() };
        read_vec(&*self.file, self.seqs_off + a, b - a)
            .map(|raw| raw.chunks_exact(2).map(|c| Feature { interval: c[0] as i8, rhythm: c[1] as i8 }).collect())
            .unwrap_or_default()
    }

    fn info(&self, doc: u32) -> DocInfo {
        self.doc(doc).unwrap_or_default()
    }

    fn pending(&self) -> bool {
        crate::source::remote::pending()
    }
}

// ---------------------------------------------------------------- both formats

/// Library index: new on disk (format 3/4) or older in memory
/// (1/2) – possibly with individually added scores on top ([`Layered`]).
pub enum AnyIndex {
    Mem(MelodyIndex),
    Disk(DiskIndex),
    Layered(Box<Layered>),
}

/// An index plus extra: individually added scores live in a
/// small side file ([`extra_path`]); the large index stays unchanged.
/// The extra's voices follow those of the base index (numbers starting at
/// `offset`), the features of both are kept together in `facets`.
pub struct Layered {
    base: AnyIndex,
    extra: MelodyIndex,
    offset: u32,
    base_scores: usize,
    /// The base index had its own features (otherwise: old text search over voices).
    base_has_facets: bool,
    facets: Vec<ScoreFacets>,
}

/// Side file with individually added scores for an index:
/// `klangwelt-bibliothek.kwix` → `klangwelt-bibliothek.zusatz.kwix`.
pub fn extra_path(index: &Path) -> PathBuf {
    let stem = index.file_stem().and_then(|s| s.to_str()).unwrap_or("klangwelt-bibliothek");
    index.with_file_name(format!("{stem}.zusatz.kwix"))
}

fn is_extra_file(path: &Path) -> bool {
    path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.ends_with(".zusatz"))
}

/// Text search over the features per score (all words must occur).
fn facet_text_search<'a>(facets: impl Iterator<Item = &'a ScoreFacets> + 'a, query: &str) -> impl Iterator<Item = u32> + 'a {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    facets
        .filter(move |f| {
            let hay = format!(
                "{} {} {} {} {} {} {}",
                f.title,
                f.composer,
                f.parts.join(" "),
                f.genres.join(" "),
                f.families.join(" "),
                f.key,
                f.mood
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|f| f.doc)
}

impl AnyIndex {
    /// Loads an index; if an extra file ([`extra_path`]) lies next to it, it is added.
    pub fn load(path: &Path) -> io::Result<Self> {
        let base = Self::load_plain(path)?;
        let extra = extra_path(path);
        if is_extra_file(path) || !extra.is_file() {
            return Ok(base);
        }
        // A damaged extra must not block the large library.
        match MelodyIndex::load(&extra) {
            Ok(e) => Ok(Self::layered(base, e)),
            Err(_) => Ok(base),
        }
    }

    /// Loads only the file itself, without the extra.
    pub fn load_plain(path: &Path) -> io::Result<Self> {
        let mut magic = [0u8; 6];
        File::open(path)?.read_exact(&mut magic)?;
        if &magic == MAGIC || &magic == MAGIC_V3 {
            Ok(Self::Disk(DiskIndex::open(path)?))
        } else {
            Ok(Self::Mem(MelodyIndex::load(path)?))
        }
    }

    /// Layers `extra` over `base` (voice numbers of the extra are shifted).
    pub fn layered(mut base: AnyIndex, extra: MelodyIndex) -> Self {
        let offset = base.doc_count() as u32;
        let base_scores = base.score_count();
        let mut facets = base.take_facets();
        let base_has_facets = !facets.is_empty();
        facets.extend(extra.facets().iter().cloned().map(|mut f| {
            f.doc += offset;
            f
        }));
        Self::Layered(Box::new(Layered { base, extra, offset, base_scores, base_has_facets, facets }))
    }

    fn take_facets(&mut self) -> Vec<ScoreFacets> {
        match self {
            Self::Mem(m) => m.take_facets(),
            Self::Disk(d) => std::mem::take(&mut d.facets),
            Self::Layered(l) => std::mem::take(&mut l.facets),
        }
    }

    /// Source files of the individually added scores (without duplicates).
    pub fn extra_sources(&self) -> Vec<String> {
        let Self::Layered(l) = self else { return Vec::new() };
        let mut out: Vec<String> = l.extra.parts().0.iter().map(|d| d.source.clone()).collect();
        out.dedup();
        out
    }

    /// Number of individually added scores.
    pub fn extra_scores(&self) -> usize {
        match self {
            Self::Layered(l) => l.extra.facets().len(),
            _ => 0,
        }
    }

    /// Chord progression statistics, if the index has them (from KWIX04).
    pub fn chords(&self) -> Option<&ChordStats> {
        match self {
            Self::Mem(_) => None,
            Self::Disk(d) => d.chords(),
            Self::Layered(l) => l.base.chords(),
        }
    }

    /// Statistics of a group (0 = all, 1 = Baroque, 2 = Bach).
    pub fn chords_group(&self, g: usize) -> Option<&ChordStats> {
        match self {
            Self::Mem(_) => None,
            Self::Disk(d) => d.chords_group(g),
            Self::Layered(l) => l.base.chords_group(g),
        }
    }

    pub fn doc_count(&self) -> usize {
        match self {
            Self::Mem(m) => m.doc_count(),
            Self::Disk(d) => d.doc_count(),
            Self::Layered(l) => l.offset as usize + l.extra.doc_count(),
        }
    }

    pub fn score_count(&self) -> usize {
        match self {
            Self::Mem(m) => m.score_count(),
            Self::Disk(d) => d.score_count(),
            Self::Layered(l) => l.base_scores + l.extra.facets().len(),
        }
    }

    /// Scores in the tree (0 = no facets, reindex) – without loading anything.
    pub fn tree_score_count(&self) -> usize {
        match self {
            Self::Mem(m) => m.facets().len(),
            Self::Disk(d) => d.tree_score_count(),
            Self::Layered(l) if l.base.is_remote() => l.base.tree_score_count() + l.extra.facets().len(),
            Self::Layered(l) => l.facets.len(),
        }
    }

    /// Scores with a chord progression [major, minor] – without loading anything.
    pub fn chord_counts(&self) -> Option<[u32; 2]> {
        match self {
            Self::Mem(_) => None,
            Self::Disk(d) => d.chord_counts(),
            Self::Layered(l) => l.base.chord_counts(),
        }
    }

    /// A remote index (browser), possibly with own scores layered over it.
    pub fn is_remote(&self) -> bool {
        match self {
            Self::Mem(_) => false,
            Self::Disk(d) => d.is_remote(),
            Self::Layered(l) => l.base.is_remote(),
        }
    }

    /// Provenance of the (base) index, see [`DiskIndex::provenance`].
    pub fn provenance(&self) -> Option<&str> {
        match self {
            Self::Mem(_) => None,
            Self::Disk(d) => d.provenance(),
            Self::Layered(l) => l.base.provenance(),
        }
    }

    /// Voice numbers from here on belong to the layered scores (`None`: nothing layered).
    pub fn extra_offset(&self) -> Option<u32> {
        match self {
            Self::Layered(l) => Some(l.offset),
            _ => None,
        }
    }

    /// The score of a voice entry from a package's ZIP (`None`: it lies in a file of its own).
    pub fn score_bytes(&self, info: &DocInfo) -> Option<io::Result<(String, Vec<u8>)>> {
        match self {
            Self::Mem(_) => None,
            Self::Disk(d) => d.score_bytes(info),
            Self::Layered(l) => l.base.score_bytes(info),
        }
    }

    /// Children of a tree node over all scores (see [`tree_children`]). Own
    /// scores layered over a remote index are listed before its scores.
    pub fn tree(&self, levels: &[&str], path: &[String], offset: usize, limit: usize) -> TreeLevel {
        match self {
            Self::Mem(m) => tree_children(m.facets(), levels, path, offset, limit),
            Self::Disk(d) => d.tree(levels, path, offset, limit),
            Self::Layered(l) => match &l.base {
                Self::Disk(d) if d.is_remote() => {
                    let Some(table) = d.web.as_ref().and_then(|w| w.table()) else { return TreeLevel::default() };
                    let rows = crate::facets::ChainRows { first: &l.facets[..], then: table };
                    let mut level = tree_children_rows(&rows, levels, path, offset, limit);
                    d.fill_titles(&mut level);
                    level
                }
                _ => tree_children(&l.facets, levels, path, offset, limit),
            },
        }
    }

    /// Takes the layered scores off again: the base index and the extra one
    /// (to layer a changed extra index over the same base).
    pub fn split(self) -> (AnyIndex, Option<MelodyIndex>) {
        match self {
            Self::Layered(l) => {
                let Layered { mut base, extra, offset, facets, base_has_facets, .. } = *l;
                if base_has_facets {
                    base.restore_facets(facets.into_iter().filter(|f| f.doc < offset).collect());
                }
                (base, Some(extra))
            }
            other => (other, None),
        }
    }

    fn restore_facets(&mut self, facets: Vec<ScoreFacets>) {
        match self {
            Self::Mem(m) => m.set_facets(facets),
            Self::Disk(d) => d.facets = facets,
            Self::Layered(l) => l.facets = facets,
        }
    }

    pub fn gram_count(&self) -> usize {
        match self {
            Self::Mem(m) => m.gram_count(),
            Self::Disk(d) => d.gram_count(),
            Self::Layered(l) => l.base.gram_count() + l.extra.gram_count(),
        }
    }

    pub fn facets(&self) -> &[ScoreFacets] {
        match self {
            Self::Mem(m) => m.facets(),
            Self::Disk(d) => d.facets(),
            Self::Layered(l) => &l.facets,
        }
    }

    pub fn doc(&self, id: u32) -> Option<DocInfo> {
        match self {
            Self::Mem(m) => m.doc(id).cloned(),
            Self::Disk(d) => d.doc(id),
            Self::Layered(l) => match id.checked_sub(l.offset) {
                None => l.base.doc(id),
                Some(i) => l.extra.doc(i).cloned(),
            },
        }
    }

    pub fn search(&self, query: &[MelodyNote], limit: usize) -> Vec<SearchHit> {
        match self {
            Self::Mem(m) => m.search(query, limit),
            Self::Disk(d) => d.search(query, limit),
            Self::Layered(l) => {
                let mut hits = l.base.search(query, limit);
                hits.extend(l.extra.search(query, limit).into_iter().map(|mut h| {
                    h.doc += l.offset;
                    h
                }));
                hits.sort_by(|a, b| b.score.total_cmp(&a.score));
                hits.truncate(limit);
                hits
            }
        }
    }

    pub fn search_text(&self, query: &str, limit: usize) -> Vec<(u32, DocInfo)> {
        match self {
            Self::Mem(m) => m.search_text(query, limit),
            Self::Disk(d) => d.search_text(query, limit),
            Self::Layered(l) if l.base.is_remote() => {
                // own scores first (few), then the remote index
                let mut out: Vec<(u32, DocInfo)> = facet_text_search(l.facets.iter(), query).take(limit).filter_map(|doc| self.doc(doc).map(|d| (doc, d))).collect();
                let rest = limit.saturating_sub(out.len());
                if rest > 0 {
                    out.extend(l.base.search_text(query, rest));
                }
                out
            }
            Self::Layered(l) => {
                // Without its own features the base index searches by itself (via its voices).
                let mut out = if l.base_has_facets { Vec::new() } else { l.base.search_text(query, limit) };
                let rest = limit.saturating_sub(out.len());
                out.extend(facet_text_search(l.facets.iter(), query).take(rest).filter_map(|doc| self.doc(doc).map(|d| (doc, d))));
                out
            }
        }
    }
}

// ---------------------------------------------------------------- Merging
//
// Several indexes become one KWIX04 file without re-reading a single score:
// voice entries and feature sequences are copied one after another
// (numbers shifted), the occurrences are merged via the sorted key tables,
// features and chord statistics are renumbered. An n-gram that
// only the first source knows is copied unchanged – for a large
// library plus a small package that is almost everything.

/// Result of [`merge`].
#[derive(Clone, Copy, Debug, Default)]
pub struct MergeStats {
    pub docs: u64,
    pub scores: u64,
    pub keys: u64,
    pub postings: u64,
    /// Voices or scores that an earlier source already contained (skipped).
    pub skipped_docs: u64,
    pub skipped_scores: u64,
}

/// Key under which a score enters a library only once:
/// PDMX files are named after their IPFS identifier ("Qm…", globally unique) –
/// so the sample and the selection coincide in the full PDMX, no matter
/// which folder they are in. All others by their path.
pub fn source_key(source: &str) -> String {
    let norm = source.replace('/', "\\");
    let name = norm.rsplit('\\').next().unwrap_or(&norm);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    if stem.len() == 46 && stem.starts_with("Qm") && stem.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return format!("pdmx:{stem}");
    }
    norm.strip_prefix(r"\\?\").unwrap_or(&norm).to_lowercase()
}

/// Part of an index (a layered index consists of two).
enum Part<'a> {
    Disk(&'a DiskIndex),
    Mem(&'a MelodyIndex),
}

fn collect_parts<'a>(any: &'a AnyIndex, offset: u32, out: &mut Vec<(Part<'a>, u32)>) {
    match any {
        AnyIndex::Disk(d) => out.push((Part::Disk(d), offset)),
        AnyIndex::Mem(m) => out.push((Part::Mem(m), offset)),
        AnyIndex::Layered(l) => {
            collect_parts(&l.base, offset, out);
            out.push((Part::Mem(&l.extra), offset + l.offset));
        }
    }
}

/// Reads a range of the file sequentially in large blocks.
struct BlockReader<'a> {
    file: &'a dyn ReadAt,
    /// File position of the first byte in `buf`.
    start: u64,
    buf: Vec<u8>,
    at: usize,
    end: u64,
}

impl<'a> BlockReader<'a> {
    const BLOCK: u64 = 4 << 20;

    fn new(file: &'a dyn ReadAt, pos: u64, end: u64) -> Self {
        Self { file, start: pos, buf: Vec::new(), at: 0, end }
    }

    /// `len` bytes from file position `pos` (usually right after the last read).
    fn take(&mut self, pos: u64, len: usize, out: &mut Vec<u8>) -> io::Result<()> {
        out.clear();
        if pos != self.start + self.at as u64 {
            self.start = pos;
            self.buf.clear();
            self.at = 0;
        }
        while out.len() < len {
            if self.at == self.buf.len() {
                self.start += self.buf.len() as u64;
                let n = Self::BLOCK.max((len - out.len()) as u64).min(self.end.saturating_sub(self.start));
                if n == 0 {
                    return Err(invalid(&klang_i18n::tr!("Index file too short")));
                }
                self.buf.resize(n as usize, 0);
                read_at(self.file, &mut self.buf, self.start)?;
                self.at = 0;
            }
            let k = (len - out.len()).min(self.buf.len() - self.at);
            out.extend_from_slice(&self.buf[self.at..self.at + k]);
            self.at += k;
        }
        Ok(())
    }
}

impl Part<'_> {
    fn doc_count(&self) -> usize {
        match self {
            Part::Disk(d) => d.doc_count(),
            Part::Mem(m) => m.doc_count(),
        }
    }

    fn key_count(&self) -> u64 {
        match self {
            Part::Disk(d) => d.n_keys,
            Part::Mem(m) => m.gram_count() as u64,
        }
    }

    /// `f(i, json)` for each voice entry in order.
    fn each_doc(&self, mut f: impl FnMut(usize, &[u8]) -> io::Result<()>) -> io::Result<()> {
        match self {
            Part::Disk(d) => each_slice(&*d.file, d.docs_off, d.docs_idx.all()?, f),
            Part::Mem(m) => {
                for (i, doc) in m.parts().0.iter().enumerate() {
                    f(i, &serde_json::to_vec(doc).map_err(io::Error::other)?)?;
                }
                Ok(())
            }
        }
    }

    /// `f(i, folge)` for each feature sequence (2 bytes per feature) in order.
    fn each_seq(&self, mut f: impl FnMut(usize, &[u8]) -> io::Result<()>) -> io::Result<()> {
        match self {
            Part::Disk(d) => each_slice(&*d.file, d.seqs_off, d.seqs_idx.all()?, f),
            Part::Mem(m) => {
                let mut raw = Vec::new();
                for (i, s) in m.parts().1.iter().enumerate() {
                    raw.clear();
                    raw.extend(s.iter().flat_map(|x| [x.interval as u8, x.rhythm as u8]));
                    f(i, &raw)?;
                }
                Ok(())
            }
        }
    }

    fn keys(&self) -> KeyCursor<'_> {
        let src = match *self {
            Part::Disk(d) => {
                let table = BlockReader::new(&*d.file, d.keys_off, d.keys_off + d.n_keys * KEY_ENTRY as u64);
                let posts = BlockReader::new(&*d.file, d.post_off, d.keys_off);
                KeySrc::Disk { d, next: 0, table, posts }
            }
            Part::Mem(m) => {
                let mut keys: Vec<u64> = m.parts().2.keys().copied().collect();
                keys.sort_unstable();
                KeySrc::Mem { m, keys, next: 0 }
            }
        };
        KeyCursor { src, key: None, raw: Vec::new(), entry: Vec::new() }
    }
}

/// Entries `idx[i]..idx[i+1]` from `base` in order, read in blocks.
fn each_slice(file: &dyn ReadAt, base: u64, idx: &[u64], mut f: impl FnMut(usize, &[u8]) -> io::Result<()>) -> io::Result<()> {
    let n = idx.len().saturating_sub(1);
    let mut i = 0;
    let mut buf = Vec::new();
    while i < n {
        // as many entries as fit in a block (at least one)
        let mut j = i + 1;
        while j < n && idx[j + 1] - idx[i] <= BlockReader::BLOCK {
            j += 1;
        }
        buf.resize((idx[j] - idx[i]) as usize, 0);
        read_at(file, &mut buf, base + idx[i])?;
        for k in i..j {
            f(k, &buf[(idx[k] - idx[i]) as usize..(idx[k + 1] - idx[i]) as usize])?;
        }
        i = j;
    }
    Ok(())
}

/// Walks through the occurrences of a part in key order.
struct KeyCursor<'a> {
    src: KeySrc<'a>,
    key: Option<u64>,
    /// Occurrences of the current key: varints, voice as a difference (from 0) and position.
    raw: Vec<u8>,
    entry: Vec<u8>,
}

enum KeySrc<'a> {
    Disk { d: &'a DiskIndex, next: u64, table: BlockReader<'a>, posts: BlockReader<'a> },
    Mem { m: &'a MelodyIndex, keys: Vec<u64>, next: usize },
}

impl KeyCursor<'_> {
    fn advance(&mut self) -> io::Result<()> {
        self.key = None;
        match &mut self.src {
            KeySrc::Disk { d, next, table, posts } => {
                if *next >= d.n_keys {
                    return Ok(());
                }
                table.take(d.keys_off + *next * KEY_ENTRY as u64, KEY_ENTRY, &mut self.entry)?;
                let e = &self.entry;
                let key = u64::from_le_bytes(e[..8].try_into().unwrap());
                let rel = u64::from_le_bytes(e[8..16].try_into().unwrap());
                let len = u32::from_le_bytes(e[16..].try_into().unwrap());
                posts.take(d.post_off + rel, len as usize, &mut self.raw)?;
                *next += 1;
                self.key = Some(key);
            }
            KeySrc::Mem { m, keys, next } => {
                let Some(&key) = keys.get(*next) else { return Ok(()) };
                *next += 1;
                self.raw.clear();
                let mut prev = 0u32;
                for p in &m.parts().2[&key] {
                    put_varint(&mut self.raw, (p.doc - prev) as u64);
                    put_varint(&mut self.raw, p.pos as u64);
                    prev = p.doc;
                }
                self.key = Some(key);
            }
        }
        Ok(())
    }
}

/// New numbers of the voices of a part.
struct Renumber {
    /// first voice of the part in the count of all sources
    old_start: u32,
    count: u32,
    /// None: all stay, new number = old + `shift`; otherwise per voice (u32::MAX = dropped)
    map: Option<Vec<u32>>,
    shift: u32,
}

impl Renumber {
    fn get(&self, i: u32) -> Option<u32> {
        match &self.map {
            None => Some(i + self.shift),
            Some(m) => m.get(i as usize).copied().filter(|&d| d != u32::MAX),
        }
    }
}

fn renumber_global(parts: &[Renumber], old: u32) -> Option<u32> {
    let p = parts.partition_point(|r| r.old_start + r.count <= old);
    let r = parts.get(p)?;
    r.get(old.checked_sub(r.old_start)?)
}

/// Merges `sources` (in this order) into a KWIX04 file `out`.
/// With `dedupe` each score goes in only once ([`source_key`]): whatever an
/// earlier source already has is skipped in later ones. `progress(0…1)`
/// is called regularly; `cancel` aborts (error `Interrupted`, `out` stays
/// untouched). The chord counts of skipped scores remain included.
pub fn merge(sources: &[&AnyIndex], out: &Path, dedupe: bool, progress: &dyn Fn(f32), cancel: &AtomicBool) -> io::Result<MergeStats> {
    let part = out.with_extension("kwix.teil");
    let result = write_merged(sources, &part, dedupe, progress, cancel);
    match result {
        Ok(stats) => {
            std::fs::rename(&part, out)?;
            Ok(stats)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn write_merged(sources: &[&AnyIndex], out: &Path, dedupe: bool, progress: &dyn Fn(f32), cancel: &AtomicBool) -> io::Result<MergeStats> {
    #[derive(serde::Deserialize)]
    struct Src {
        #[serde(default)]
        source: String,
    }
    let stop = || -> io::Result<()> {
        if cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(io::ErrorKind::Interrupted, klang_i18n::tr!("canceled")))
        } else {
            Ok(())
        }
    };
    // Parts of all sources with their start number in the overall count
    let mut parts: Vec<(Part, u32)> = Vec::new();
    let mut input_start = Vec::with_capacity(sources.len());
    let mut total = 0u32;
    for s in sources {
        input_start.push(total);
        collect_parts(s, total, &mut parts);
        total += s.doc_count() as u32;
    }

    // New numbers (skip duplicates)
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut renum: Vec<Renumber> = Vec::with_capacity(parts.len());
    let mut kept = 0u32;
    let mut skipped_docs = 0u64;
    for (i, (p, start)) in parts.iter().enumerate() {
        let n = p.doc_count() as u32;
        let last = i + 1 == parts.len();
        let mut map: Option<Vec<u32>> = None;
        if dedupe && (!seen.is_empty() || !last) {
            let mut keys: Vec<String> = Vec::with_capacity(n as usize);
            p.each_doc(|_, json| {
                let s: Src = serde_json::from_slice(json).map_err(io::Error::other)?;
                keys.push(source_key(&s.source));
                Ok(())
            })?;
            stop()?;
            if keys.iter().any(|k| seen.contains(k)) {
                let mut next = kept;
                map = Some(
                    keys.iter()
                        .map(|k| {
                            if seen.contains(k) {
                                skipped_docs += 1;
                                u32::MAX
                            } else {
                                next += 1;
                                next - 1
                            }
                        })
                        .collect(),
                );
            }
            if !last {
                seen.extend(keys);
            }
        }
        let added = map.as_ref().map(|m| m.iter().filter(|&&d| d != u32::MAX).count() as u32).unwrap_or(n);
        renum.push(Renumber { old_start: *start, count: n, map, shift: kept });
        kept += added;
    }
    drop(seen);

    let mut w = BufWriter::with_capacity(1 << 20, File::create(out)?);
    w.write_all(&[0u8; HEADER_LEN])?;
    let mut pos = HEADER_LEN as u64;

    // Voice entries and feature sequences
    let copy_section = |w: &mut BufWriter<File>, pos: &mut u64, seqs: bool| -> io::Result<Vec<u64>> {
        let mut idx = Vec::with_capacity(kept as usize + 1);
        idx.push(0u64);
        for ((p, _), r) in parts.iter().zip(&renum) {
            let mut f = |i: usize, bytes: &[u8]| -> io::Result<()> {
                if r.get(i as u32).is_some() {
                    w.write_all(bytes)?;
                    idx.push(idx.last().unwrap() + bytes.len() as u64);
                }
                Ok(())
            };
            if seqs {
                p.each_seq(&mut f)?;
            } else {
                p.each_doc(&mut f)?;
            }
            stop()?;
        }
        *pos += idx.last().unwrap();
        Ok(idx)
    };
    let docs_off = pos;
    let docs_idx = copy_section(&mut w, &mut pos, false)?;
    let docs_idx_off = pos;
    for o in &docs_idx {
        w.write_all(&o.to_le_bytes())?;
    }
    pos += 8 * docs_idx.len() as u64;
    progress(0.08);
    let seqs_off = pos;
    let seqs_idx = copy_section(&mut w, &mut pos, true)?;
    let seqs_idx_off = pos;
    for o in &seqs_idx {
        w.write_all(&o.to_le_bytes())?;
    }
    pos += 8 * seqs_idx.len() as u64;
    progress(0.15);

    // Features per score (old indexes without features: from the voice entries)
    let mut facets: Vec<ScoreFacets> = Vec::new();
    let mut all_scores = 0usize;
    for (s, &start) in sources.iter().zip(&input_start) {
        let own: Vec<ScoreFacets> = if s.facets().is_empty() && s.doc_count() > 0 { facets_from_docs(s) } else { s.facets().to_vec() };
        all_scores += own.len();
        facets.extend(own.into_iter().filter_map(|mut f| {
            f.doc = renumber_global(&renum, start + f.doc)?;
            Some(f)
        }));
    }
    let skipped_scores = (all_scores - facets.len()) as u64;
    let facets_off = pos;
    let facets_json = serde_json::to_vec(&facets).map_err(io::Error::other)?;
    w.write_all(&facets_json)?;
    pos += facets_json.len() as u64;

    // Occurrences: merge all parts in key order
    let post_off = pos;
    let keys_path = out.with_extension("schluessel");
    let mut keys_w = BufWriter::with_capacity(1 << 20, File::create(&keys_path)?);
    let result = (|| -> io::Result<(u64, u64, u64, Vec<u64>)> {
        let total_keys: u64 = parts.iter().map(|(p, _)| p.key_count()).sum::<u64>().max(1);
        let mut cursors: Vec<KeyCursor> = parts.iter().map(|(p, _)| p.keys()).collect();
        for c in &mut cursors {
            c.advance()?;
        }
        let (mut n_keys, mut n_post, mut rel, mut done) = (0u64, 0u64, 0u64, 0u64);
        let mut sparse = Vec::new();
        let mut list: Vec<(u32, u32)> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut from: Vec<usize> = Vec::new();
        while let Some(key) = cursors.iter().filter_map(|c| c.key).min() {
            if done % 8192 == 0 {
                stop()?;
                progress(0.15 + 0.85 * done as f32 / total_keys as f32);
            }
            from.clear();
            from.extend((0..cursors.len()).filter(|&i| cursors[i].key == Some(key)));
            done += from.len() as u64;
            let only = from[0];
            let (bytes, count): (&[u8], u64) = if from.len() == 1 && renum[only].map.is_none() && renum[only].shift == 0 {
                // only in one part and unchanged there: copy the bytes
                let raw = &cursors[only].raw;
                (raw, raw.iter().filter(|&&b| b < 0x80).count() as u64 / 2)
            } else {
                list.clear();
                for &i in &from {
                    let raw = &cursors[i].raw;
                    let (mut at, mut doc) = (0usize, 0u64);
                    while at < raw.len() {
                        doc += get_varint(raw, &mut at).ok_or_else(|| invalid(&klang_i18n::tr!("Occurrence list corrupted")))?;
                        let p = get_varint(raw, &mut at).ok_or_else(|| invalid(&klang_i18n::tr!("Occurrence list corrupted")))?;
                        if let Some(d) = renum[i].get(doc as u32) {
                            list.push((d, p as u32));
                        }
                    }
                }
                buf.clear();
                if list.is_empty() || list.len() > MAX_POSTINGS_PER_GRAM {
                    (&buf[..], 0)
                } else {
                    let mut prev = 0u32;
                    for &(d, p) in &list {
                        put_varint(&mut buf, (d - prev) as u64);
                        put_varint(&mut buf, p as u64);
                        prev = d;
                    }
                    (&buf[..], list.len() as u64)
                }
            };
            if count > 0 {
                w.write_all(bytes)?;
                if n_keys % SPARSE_STEP == 0 {
                    sparse.push(key);
                }
                keys_w.write_all(&key.to_le_bytes())?;
                keys_w.write_all(&rel.to_le_bytes())?;
                keys_w.write_all(&(bytes.len() as u32).to_le_bytes())?;
                rel += bytes.len() as u64;
                n_keys += 1;
                n_post += count;
            }
            for &i in &from {
                cursors[i].advance()?;
            }
        }
        keys_w.flush()?;
        Ok((n_keys, n_post, rel, sparse))
    })();
    drop(keys_w);
    let (n_keys, n_post, rel, sparse) = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&keys_path);
            return Err(e);
        }
    };
    pos += rel;
    let keys_off = pos;
    let copied = io::copy(&mut BufReader::new(File::open(&keys_path)?), &mut w);
    let _ = std::fs::remove_file(&keys_path);
    pos += copied?;
    let sparse_off = pos;
    for k in &sparse {
        w.write_all(&k.to_le_bytes())?;
    }
    pos += 8 * sparse.len() as u64;

    // Chord statistics per group
    let mut chords = vec![ChordStats::default(); GROUPS.len()];
    for (s, &start) in sources.iter().zip(&input_start) {
        for (g, merged) in chords.iter_mut().enumerate() {
            if let Some(c) = s.chords_group(g) {
                merged.absorb(c, |doc| renumber_global(&renum, start + doc));
            }
        }
    }
    for c in &mut chords {
        c.finalize();
    }
    let chords_off = pos;
    let chords_bytes = ChordStats::encode_groups(&chords);
    w.write_all(&chords_bytes)?;
    pos += chords_bytes.len() as u64;

    let n_docs = kept as u64;
    let mut f = w.into_inner().map_err(|e| e.into_error())?;
    f.seek(SeekFrom::Start(0))?;
    let mut head = Vec::with_capacity(HEADER_LEN);
    head.extend_from_slice(MAGIC);
    head.extend_from_slice(&[0, 0]);
    for v in [docs_off, docs_idx_off, seqs_off, seqs_idx_off, facets_off, post_off, keys_off, sparse_off, pos, n_docs, n_keys, n_post, chords_off] {
        head.extend_from_slice(&v.to_le_bytes());
    }
    f.write_all(&head)?;
    f.sync_all()?;
    progress(1.0);
    Ok(MergeStats { docs: n_docs, scores: facets.len() as u64, keys: n_keys, postings: n_post, skipped_docs, skipped_scores })
}

/// Features for indexes without their own (format 1): one entry per source file.
fn facets_from_docs(index: &AnyIndex) -> Vec<ScoreFacets> {
    let mut out: Vec<ScoreFacets> = Vec::new();
    let mut last = String::new();
    for i in 0..index.doc_count() as u32 {
        let Some(d) = index.doc(i) else { continue };
        if d.source == last {
            if let Some(f) = out.last_mut() {
                f.parts.push(d.part);
            }
            continue;
        }
        last = d.source.clone();
        out.push(ScoreFacets { doc: i, title: d.title, composer: d.composer, parts: vec![d.part], ..Default::default() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pseudo-random but reproducible melodies.
    fn melodies(n: usize) -> Vec<Vec<MelodyNote>> {
        let mut x: u32 = 12345;
        let mut rnd = move |m: u32| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x % m
        };
        (0..n)
            .map(|_| {
                let len = 12 + rnd(40) as usize;
                let mut t = 0.0;
                let mut p = 60i32;
                (0..len)
                    .map(|_| {
                        p = (p + rnd(9) as i32 - 4).clamp(40, 90);
                        let d = [0.5, 1.0, 1.0, 2.0][rnd(4) as usize];
                        let note = MelodyNote { onset: t, duration: d, midi: p as u8 };
                        t += d;
                        note
                    })
                    .collect()
            })
            .collect()
    }

    fn info(i: usize) -> DocInfo {
        DocInfo { source: format!("s{}.mxl", i / 2), title: format!("Stück {}", i / 2), part: format!("Stimme {i}"), ..Default::default() }
    }

    #[test]
    fn disk_index_matches_memory_index() {
        let tunes = melodies(300);
        let dir = std::env::temp_dir().join(format!("klang-disk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Small blocks force many runs → merging gets tested too.
        let mut w = IndexWriter::with_chunk(&dir.join("tmp"), 2_000).unwrap();
        let mut mem = MelodyIndex::new();
        for (i, t) in tunes.iter().enumerate() {
            let a = w.add_melody(info(i), t).unwrap();
            let b = mem.add_melody(info(i), t);
            assert_eq!(a, b);
            if i % 2 == 0 {
                let f = ScoreFacets { doc: a.unwrap(), title: format!("Stück {}", i / 2), genres: vec!["Klassik".into()], ..Default::default() };
                w.add_facets(f.clone());
                mem.add_facets(f);
            }
        }
        mem.compact();
        let out = dir.join("t.kwix");
        let stats = w.finish(&out, &AtomicBool::new(false)).unwrap();
        assert!(stats.runs > 3, "mehrere Läufe: {}", stats.runs);
        assert!(!dir.join("tmp").exists(), "Zwischendateien aufgeräumt");

        let any = AnyIndex::load(&out).unwrap();
        let AnyIndex::Disk(disk) = &any else { panic!("Format 3 erwartet") };
        assert_eq!(disk.doc_count(), mem.doc_count());
        assert_eq!(disk.gram_count(), mem.gram_count());
        assert_eq!(disk.facets(), mem.facets());
        assert_eq!(disk.doc(7), mem.doc(7).cloned());
        assert_eq!(disk.seq(5), mem.seq(5));
        // Occurrences identical, also for unknown keys
        for t in tunes.iter().take(40) {
            let f = features(t);
            for pos in 0..f.len().saturating_sub(4) {
                let k = gram_key(1, &f[pos..pos + 4]);
                assert_eq!(SearchData::postings(disk, k), SearchData::postings(&mem, k));
            }
        }
        assert_eq!(SearchData::postings(disk, 0xdead_beef), None);
        // Search returns the same hits
        for i in [3usize, 77, 150, 299] {
            let q: Vec<MelodyNote> = tunes[i][2..10].to_vec();
            let a = any.search(&q, 5);
            let b = mem.search(&q, 5);
            assert_eq!(a.iter().map(|h| (h.doc, h.note_index)).collect::<Vec<_>>(), b.iter().map(|h| (h.doc, h.note_index)).collect::<Vec<_>>());
            assert_eq!(a[0].doc as usize, i, "eigene Melodie zuerst");
        }
        assert_eq!(any.search_text("stück 149", 10).len(), 1);
        assert_eq!(any.search_text("stück 12", 100).len(), 12, "Wörter einzeln: 12, 112 und 120–129");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Chord progression I – V7 – I (with `vi` additionally V7 – vi).
    fn progression(vi: bool) -> Progression {
        use crate::chordstats::{ProgItem, Rel};
        use crate::harmony::parse_roman;
        use crate::theory::Key;
        let k = Key { tonic: 0, minor: false, confidence: 1.0 };
        let names: &[&str] = if vi { &["I", "V7", "I", "V7", "vi"] } else { &["I", "V7", "I"] };
        let items = names
            .iter()
            .enumerate()
            .map(|(i, s)| ProgItem { start: i as f64, end: i as f64 + 1.0, bar: i as u32 + 1, rel: Rel::of(&parse_roman(s, &k).unwrap(), &k) })
            .collect();
        Progression { minor: false, items, notable: true }
    }

    /// Index from the melodies `range` (every two voices one score) as a file.
    fn build(dir: &Path, name: &str, tunes: &[Vec<MelodyNote>], range: std::ops::Range<usize>, source: impl Fn(usize) -> String) -> PathBuf {
        let mut w = IndexWriter::with_chunk(&dir.join(format!("tmp-{name}")), 3_000).unwrap();
        for i in range {
            let doc = w.add_melody(DocInfo { source: source(i), ..info(i) }, &tunes[i]).unwrap().unwrap();
            if i % 2 == 0 {
                w.add_facets(ScoreFacets { doc, title: format!("Stück {}", i / 2), genres: vec!["Klassik".into()], ..Default::default() });
                w.add_progression(doc, &progression(i % 4 == 0), "Johann Sebastian Bach");
            }
        }
        let out = dir.join(format!("{name}.kwix"));
        w.finish(&out, &AtomicBool::new(false)).unwrap();
        out
    }

    #[test]
    fn merging_equals_indexing_everything_at_once() {
        let tunes = melodies(300);
        let dir = std::env::temp_dir().join(format!("klang-merge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = |i: usize| format!("s{}.mxl", i / 2);
        let a = AnyIndex::load(&build(&dir, "a", &tunes, 0..180, src)).unwrap();
        let b = AnyIndex::load(&build(&dir, "b", &tunes, 180..300, src)).unwrap();
        let whole = AnyIndex::load(&build(&dir, "ganz", &tunes, 0..300, src)).unwrap();
        let out = dir.join("verschmolzen.kwix");
        let seen = std::sync::Mutex::new(Vec::new());
        let stats = merge(&[&a, &b], &out, true, &|p| seen.lock().unwrap().push(p), &AtomicBool::new(false)).unwrap();
        assert_eq!((stats.docs, stats.scores, stats.skipped_docs), (300, 150, 0));
        assert_eq!(seen.lock().unwrap().last(), Some(&1.0));
        assert!(!out.with_extension("kwix.teil").exists() && !out.with_extension("schluessel").exists(), "Zwischendateien weg");

        let m = AnyIndex::load(&out).unwrap();
        let (AnyIndex::Disk(md), AnyIndex::Disk(wd)) = (&m, &whole) else { panic!("Format 4 erwartet") };
        assert_eq!(md.doc_count(), wd.doc_count());
        assert_eq!(md.gram_count(), wd.gram_count());
        assert_eq!(md.facets(), wd.facets());
        for d in [0u32, 179, 180, 299] {
            assert_eq!(md.doc(d), wd.doc(d));
            assert_eq!(md.seq(d), wd.seq(d));
        }
        for t in tunes.iter().step_by(7) {
            let f = features(t);
            for pos in 0..f.len().saturating_sub(4) {
                let k = gram_key(1, &f[pos..pos + 4]);
                assert_eq!(SearchData::postings(md, k), SearchData::postings(wd, k));
            }
        }
        for i in [3usize, 150, 200, 299] {
            let q: Vec<MelodyNote> = tunes[i][2..10].to_vec();
            let hits = |x: &AnyIndex| x.search(&q, 5).iter().map(|h| (h.doc, h.note_index)).collect::<Vec<_>>();
            assert_eq!(hits(&m), hits(&whole));
            assert_eq!(m.search(&q, 1)[0].doc as usize, i);
        }
        // Chord statistics add up; examples point to the right pieces
        for g in 0..GROUPS.len() {
            assert_eq!(m.chords_group(g).unwrap().scores, whole.chords_group(g).unwrap().scores);
        }
        let k = crate::theory::Key { tonic: 0, minor: false, confidence: 1.0 };
        let r = |s: &str| crate::chordstats::Rel::of(&crate::harmony::parse_roman(s, &k).unwrap(), &k);
        let (fm, fw) = (m.chords().unwrap().follow(false, r("V7"), r("vi")).unwrap(), whole.chords().unwrap().follow(false, r("V7"), r("vi")).unwrap());
        assert_eq!((fm.count, fm.total), (fw.count, fw.total));
        for e in &fm.examples {
            assert!(m.facets().iter().any(|f| f.doc == e.doc), "Beispiel {} ist eine Partitur", e.doc);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn merging_skips_scores_that_are_already_there() {
        let tunes = melodies(120);
        let dir = std::env::temp_dir().join(format!("klang-merge-doppelt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // PDMX files are named the same everywhere, no matter which folder
        let cid = |i: usize| format!("Qm{:0>44}", i / 2);
        let big = AnyIndex::load(&build(&dir, "pdmx", &tunes, 0..80, |i| format!(r"C:\Bib\PDMX\{}.mxl", cid(i)))).unwrap();
        // Sample: 20 voices already in the large one, 20 new
        let small = AnyIndex::load(&build(&dir, "kostprobe", &tunes, 60..100, |i| format!(r"C:\Bib\PDMX-Kostprobe\{}.mxl", cid(i)))).unwrap();
        // Extra (in-memory format) on top of the sample: one piece new, one duplicate (same path, different spelling)
        let mut extra = MelodyIndex::new();
        for i in [100usize, 101, 0, 1] {
            let source = if i < 100 { format!(r"c:/bib/pdmx/{}.MXL", cid(i)) } else { r"C:\Eigene\Souvenir.mxl".to_string() };
            let doc = extra.add_melody(DocInfo { source, ..info(i) }, &tunes[i]).unwrap();
            if i % 2 == 0 {
                extra.add_facets(ScoreFacets { doc, title: format!("Stück {}", i / 2), ..Default::default() });
            }
        }
        let layered = AnyIndex::layered(small, extra);
        let out = dir.join("standard.kwix");
        let stats = merge(&[&big, &layered], &out, true, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!((stats.skipped_docs, stats.skipped_scores), (22, 11));
        assert_eq!((stats.docs, stats.scores), (102, 51));
        let m = AnyIndex::load(&out).unwrap();
        assert_eq!(m.doc_count(), 102);
        assert_eq!(m.score_count(), 51);
        // each score exactly once, features point to their voices
        let mut titles: Vec<String> = m.facets().iter().map(|f| f.title.clone()).collect();
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), 51);
        for f in m.facets() {
            assert_eq!(m.doc(f.doc).unwrap().title, f.title);
        }
        // new pieces from both parts of the second source are found
        for i in [90usize, 100] {
            let hit = &m.search(&tunes[i][1..9].to_vec(), 1)[0];
            assert_eq!(hit.info.part, format!("Stimme {i}"));
        }
        // without the comparison everything stays in
        let all = dir.join("alles.kwix");
        assert_eq!(merge(&[&big, &layered], &all, false, &|_| {}, &AtomicBool::new(false)).unwrap().docs, 124);
        // Aborting leaves nothing behind
        let gone = dir.join("abgebrochen.kwix");
        let err = merge(&[&big, &layered], &gone, true, &|_| {}, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(!gone.exists() && !gone.with_extension("kwix.teil").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn source_keys() {
        assert_eq!(source_key(r"C:\A\PDMX\0\Qma113RUUvApyeCqFzReRGQgzyoTbvx5sgm91MGkokDcMW.mxl"), "pdmx:Qma113RUUvApyeCqFzReRGQgzyoTbvx5sgm91MGkokDcMW");
        assert_eq!(source_key("D:/x/Qma113RUUvApyeCqFzReRGQgzyoTbvx5sgm91MGkokDcMW.mxl"), "pdmx:Qma113RUUvApyeCqFzReRGQgzyoTbvx5sgm91MGkokDcMW");
        assert_eq!(source_key(r"C:\Eigene\Souvenir.MXL"), source_key("c:/eigene/souvenir.mxl"));
        assert_eq!(source_key(r"\\?\C:\Eigene\a.mxl"), source_key(r"C:\Eigene\a.mxl"));
        assert_ne!(source_key(r"C:\a\Satz 1.mxl"), source_key(r"C:\b\Satz 1.mxl"));
    }

    #[test]
    fn varints_roundtrip() {
        let mut buf = Vec::new();
        for v in [0u64, 1, 127, 128, 300, 65_535, u32::MAX as u64, u64::MAX] {
            put_varint(&mut buf, v);
        }
        let mut i = 0;
        for v in [0u64, 1, 127, 128, 300, 65_535, u32::MAX as u64, u64::MAX] {
            assert_eq!(get_varint(&buf, &mut i), Some(v));
        }
        assert_eq!(i, buf.len());
    }

    /// Runs `f` like the web shell does: as long as data is missing, hand over
    /// the missing ranges (from `files`) and run again. Returns the result and
    /// the number of rounds with requests.
    fn in_rounds<T>(files: &[(&str, &[u8])], mut f: impl FnMut() -> T) -> (T, usize) {
        use crate::source::remote;
        let mut rounds = 0;
        loop {
            remote::begin();
            let r = f();
            let need = remote::take_missing();
            if need.is_empty() {
                return (r, rounds);
            }
            rounds += 1;
            assert!(rounds < 12, "no progress: {need:?}");
            for n in need {
                let bytes = files.iter().find(|(name, _)| *name == n.file).expect("known file").1;
                assert!(remote::feed(&n.file, n.start, bytes[n.start as usize..n.end as usize].to_vec()));
            }
        }
    }

    #[test]
    fn remote_index_reads_in_rounds_and_matches_the_file() {
        use crate::source::remote::RemoteFile;
        use crate::webpack::{write_facet_table, write_text_index};
        let tunes = melodies(300);
        let dir = std::env::temp_dir().join(format!("klang-disk-remote-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = IndexWriter::new(&dir.join("tmp")).unwrap();
        w.set_provenance(r#"{"format":"KWIX04","sources":["test"]}"#);
        let mut facets = Vec::new();
        for (i, t) in tunes.iter().enumerate() {
            let doc = w.add_melody(info(i), t).unwrap().unwrap();
            if i % 2 == 0 {
                let f = ScoreFacets { doc, title: format!("Stück {}", i / 2), composer: if i % 4 == 0 { "Bach".into() } else { "Händel".into() }, genres: vec!["Klassik".into()], ..Default::default() };
                w.add_facets(f.clone());
                facets.push(f);
            }
        }
        let out = dir.join("t.kwix");
        w.finish(&out, &AtomicBool::new(false)).unwrap();
        let local = DiskIndex::open(&out).unwrap();
        assert_eq!(local.provenance(), Some(r#"{"format":"KWIX04","sources":["test"]}"#));
        let (index, text, table) = (std::fs::read(&out).unwrap(), write_text_index(&facets), write_facet_table(&facets));
        let tag = std::process::id();
        for small in [SMALL_TABLE, 0] {
            let names = [format!("idx-{tag}-{small}"), format!("txt-{tag}-{small}"), format!("tab-{tag}-{small}")];
            let files: Vec<(&str, &[u8])> = vec![(&names[0], &index), (&names[1], &text), (&names[2], &table)];
            let open = || {
                let web = WebParts::new(
                    Some(Box::new(RemoteFile::new(&names[1], text.len() as u64))),
                    Some(Box::new(RemoteFile::new(&names[2], table.len() as u64))),
                    None,
                    facets.len(),
                    None,
                );
                DiskIndex::open_remote_with(Box::new(RemoteFile::new(&names[0], index.len() as u64)), web, None, small)
            };
            let (remote, rounds) = in_rounds(&files, open);
            let remote = remote.unwrap();
            assert_eq!(rounds, 2, "header, then provenance (and small tables)");
            assert_eq!(remote.provenance(), local.provenance());
            assert_eq!((remote.doc_count(), remote.score_count()), (local.doc_count(), local.score_count()));
            // melody search: the same hits in a few rounds
            for i in [3usize, 77, 150] {
                let q: Vec<MelodyNote> = tunes[i][2..12].to_vec();
                let (hits, rounds) = in_rounds(&files, || remote.search(&q, 5));
                let want = local.search(&q, 5);
                assert_eq!(hits.iter().map(|h| (h.doc, h.note_index, h.info.clone())).collect::<Vec<_>>(), want.iter().map(|h| (h.doc, h.note_index, h.info.clone())).collect::<Vec<_>>());
                assert!(rounds <= 5, "rounds: {rounds}");
            }
            // text search through the text index, tree through the facet table
            let (hits, _) = in_rounds(&files, || remote.search_text("bach", 50));
            assert_eq!(hits, local.search_text("bach", 50));
            assert!(!hits.is_empty());
            let (top, _) = in_rounds(&files, || remote.tree(&["komponist"], &[], 0, 10));
            assert_eq!(top.nodes, local.tree(&["komponist"], &[], 0, 10).nodes);
            let path = vec!["Bach".to_string()];
            let (leaves, _) = in_rounds(&files, || remote.tree(&["komponist"], &path, 0, 5));
            let want = local.tree(&["komponist"], &path, 0, 5);
            assert_eq!(leaves.scores.iter().map(|s| (s.doc, s.title.clone())).collect::<Vec<_>>(), want.scores.iter().map(|s| (s.doc, s.title.clone())).collect::<Vec<_>>());
            // an own index layered over the remote one: own scores first in text search and tree
            let mut own = MelodyIndex::new();
            let doc = own.add_melody(DocInfo { source: "/Klangwelt/Bibliothek/Eigene/mein.musicxml".into(), title: "Mein Bach".into(), ..Default::default() }, &tunes[0]).unwrap();
            own.add_facets(ScoreFacets { doc, title: "Mein Bach".into(), composer: "Bach".into(), ..Default::default() });
            let layered = AnyIndex::layered(AnyIndex::Disk(remote), own);
            let (hits, _) = in_rounds(&files, || layered.search_text("bach", 3));
            assert_eq!(hits[0].1.title, "Mein Bach");
            assert_eq!(hits.len(), 3);
            let (leaves, _) = in_rounds(&files, || layered.tree(&["komponist"], &path, 0, 3));
            assert_eq!(leaves.scores[0].title, "Mein Bach");
            assert_eq!(leaves.matching, want.matching + 1);
            assert_eq!(layered.tree_score_count(), facets.len() + 1);
            let (hits, _) = in_rounds(&files, || layered.search(&tunes[0][1..11], 3));
            assert!(hits.iter().any(|h| h.info.title == "Mein Bach"));
            let (base, extra) = layered.split();
            assert!(base.is_remote() && extra.is_some());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_truncated_files() {
        let dir = std::env::temp_dir().join(format!("klang-disk-kaputt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = IndexWriter::new(&dir.join("tmp")).unwrap();
        for (i, t) in melodies(20).iter().enumerate() {
            w.add_melody(info(i), t).unwrap();
        }
        let out = dir.join("t.kwix");
        w.finish(&out, &AtomicBool::new(false)).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        std::fs::write(&out, &bytes[..bytes.len() / 2]).unwrap();
        assert!(DiskIndex::open(&out).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The browser's network, simulated: fetches what a command misses the way
    /// the web worker does (per file, ranges merged across gaps under 2 KB, at
    /// most `per_request` per request, a range read once is cached) and counts it.
    struct Net {
        dir: PathBuf,
        seen: std::collections::HashSet<(String, u64, u64)>,
        per_request: usize,
    }

    impl Net {
        /// Runs `f` until nothing is missing: (result, rounds, requests, bytes).
        fn run<T>(&mut self, mut f: impl FnMut() -> T) -> (T, usize, usize, u64) {
            use crate::source::remote;
            let (mut rounds, mut requests, mut bytes) = (0, 0, 0);
            loop {
                remote::begin();
                let r = f();
                let need = remote::take_missing();
                if need.is_empty() {
                    return (r, rounds, requests, bytes);
                }
                let mut by_file: std::collections::BTreeMap<String, Vec<(u64, u64)>> = Default::default();
                for n in &need {
                    if self.seen.insert((n.file.clone(), n.start, n.end)) {
                        by_file.entry(n.file.clone()).or_default().push((n.start, n.end));
                    }
                    let file = File::open(self.dir.join(&n.file)).unwrap();
                    assert!(remote::feed(&n.file, n.start, read_vec(&file, n.start, n.end - n.start).unwrap()));
                }
                if !by_file.is_empty() {
                    rounds += 1;
                }
                for (_, mut list) in by_file {
                    list.sort();
                    let mut spans: Vec<(u64, u64)> = Vec::new();
                    for (a, b) in list {
                        match spans.last_mut() {
                            Some(last) if a <= last.1 + 2048 => last.1 = last.1.max(b),
                            _ => spans.push((a, b)),
                        }
                    }
                    requests += spans.len().div_ceil(self.per_request);
                    bytes += spans.iter().map(|(a, b)| b - a).sum::<u64>();
                }
                assert!(rounds < 30, "no progress");
            }
        }
    }

    /// Notes back from a voice's transitions (pitch 60, first spacing 1).
    fn notes_of(seq: &[u8]) -> Vec<MelodyNote> {
        let (mut midi, mut onset, mut ioi) = (60i32, 0.0f64, 1.0f64);
        let mut out = vec![MelodyNote { onset, duration: ioi, midi: 60 }];
        for f in seq.chunks_exact(2) {
            midi = (midi + f[0] as i8 as i32).clamp(0, 127);
            onset += ioi;
            ioi *= 2f64.powf(f[1] as i8 as f64 / 2.0);
            out.push(MelodyNote { onset, duration: ioi, midi: midi as u8 });
        }
        out
    }

    /// What the browser version asks of a built package: rounds, HTTP requests
    /// and bytes per action, with 80 and with 250 ranges per request. Run with
    /// `KLANG_PACKAGE=target/library/package cargo test --release -p klang-music requests_against_a_package -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn requests_against_a_package() {
        use crate::source::remote::{self, RemoteFile};
        use crate::webpack::{CHORDS_FILE, FACETS_FILE, INDEX_FILE, MANIFEST_FILE, SCORES_FILE, TEXT_FILE};
        let Ok(dir) = std::env::var("KLANG_PACKAGE") else { return };
        let dir = PathBuf::from(dir);
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_FILE)).unwrap()).unwrap();
        let len = |n: &str| std::fs::metadata(dir.join(n)).unwrap().len();
        let index_file = File::open(dir.join(INDEX_FILE)).unwrap();
        let h = read_header(&index_file).unwrap();
        let mb = |a: u64, b: u64| (b - a) as f64 / 1e6;
        eprintln!(
            "voices {}, keys {}, occurrences {} · voice entries {:.1} MB, offsets 2 × {:.1} MB, sequences {:.1} MB, facets {:.1} MB, lists {:.1} MB, key table {:.1} MB, sparse table {:.2} MB",
            h.n_docs, h.n_keys, _n_post(&index_file), mb(h.docs_off, h.docs_idx_off), mb(h.docs_idx_off, h.seqs_off), mb(h.seqs_off, h.seqs_idx_off),
            mb(h.facets_off, h.post_off), mb(h.post_off, h.keys_off), mb(h.keys_off, h.sparse_off), mb(h.sparse_off, h.sparse_end)
        );
        // queries: known tunes and windows of real voices
        let tune = |m: &[u8]| m.iter().enumerate().map(|(i, &midi)| MelodyNote { onset: i as f64, duration: 1.0, midi }).collect::<Vec<_>>();
        let mut queries: Vec<(String, Vec<MelodyNote>)> = vec![
            ("Ode to Joy".into(), tune(&[64, 64, 65, 67, 67, 65, 64, 62, 60, 60, 62, 64])),
            ("Twinkle".into(), tune(&[60, 60, 67, 67, 69, 69, 67, 65, 65, 64, 64, 62, 62, 60])),
            ("Für Elise".into(), tune(&[76, 75, 76, 75, 76, 71, 74, 72, 69])),
        ];
        for d in [10u64, 5_000, 40_000, 120_000, 250_000].into_iter().filter(|&d| d < h.n_docs) {
            let o = u64s(&read_vec(&index_file, h.seqs_idx_off + 8 * d, 16).unwrap());
            let seq = read_vec(&index_file, h.seqs_off + o[0], o[1] - o[0]).unwrap();
            if seq.len() >= 2 * 14 {
                queries.push((format!("voice {d}"), notes_of(&seq[6..6 + 2 * 11])));
            }
        }
        for per_request in [80, 250] {
            eprintln!("\n--- {per_request} ranges per request");
            remote::reset();
            let mut net = Net { dir: dir.clone(), seen: Default::default(), per_request };
            let report = |what: &str, rounds: usize, requests: usize, bytes: u64| eprintln!("{what:<34} rounds {rounds:>2}  requests {requests:>3}  {:>8.1} kB", bytes as f64 / 1e3);
            let open = || {
                let file = |n: &str| -> Option<Box<dyn ReadAt>> { Some(Box::new(RemoteFile::new(n, len(n)))) };
                let chord_scores = manifest["chordScores"].as_array().map(|a| [a[0].as_u64().unwrap_or(0) as u32, a[1].as_u64().unwrap_or(0) as u32]);
                let web = WebParts::new(file(TEXT_FILE), file(FACETS_FILE), file(CHORDS_FILE), manifest["scores"].as_u64().unwrap() as usize, chord_scores);
                DiskIndex::open_remote(Box::new(RemoteFile::new(INDEX_FILE, len(INDEX_FILE))), web, file(SCORES_FILE))
            };
            let (index, r, q, b) = net.run(open);
            let index = index.unwrap();
            report("open", r, q, b);
            let mut all = (0, 0, 0u64);
            for (name, query) in &queries {
                let (hits, r, q, b) = net.run(|| index.search(query, 10));
                all = (all.0 + r, all.1 + q, all.2 + b);
                let top = hits.first().map(|h| format!("{} ({:.2}) {}", h.doc, h.score, h.info.title)).unwrap_or_default();
                report(&format!("melody: {name}"), r, q, b);
                eprintln!("{:<34} → {top}", "");
                if let Some(hit) = hits.first() {
                    let (bytes, r, q, b) = net.run(|| index.score_bytes(&hit.info).map(|r| r.map(|(_, b)| b.len())));
                    report(&format!("  open the score ({} kB)", bytes.and_then(Result::ok).unwrap_or(0) / 1000), r, q, b);
                }
            }
            eprintln!("melody searches on average: {:.1} rounds, {:.1} requests, {:.0} kB", all.0 as f64 / queries.len() as f64, all.1 as f64 / queries.len() as f64, all.2 as f64 / 1e3 / queries.len() as f64);
            for words in ["bach", "mozart sonate", "ave maria", "lied"] {
                let (hits, r, q, b) = net.run(|| index.search_text(words, 50).len());
                report(&format!("text: {words} ({hits} hits)"), r, q, b);
            }
            let (top, r, q, b) = net.run(|| index.tree(&["genre", "komponist"], &[], 0, 50));
            report(&format!("browse: genres ({} values)", top.nodes.len()), r, q, b);
            if let Some(first) = top.nodes.first() {
                let path = vec![first.value.clone()];
                let (level, r, q, b) = net.run(|| index.tree(&["genre", "komponist"], &path, 0, 50));
                report(&format!("browse: {} ({} composers)", first.value, level.nodes.len()), r, q, b);
            }
            let (_, r, q, b) = net.run(|| index.chords().map(|c| c.scores));
            report("chord statistics", r, q, b);
        }
    }

    fn _n_post(file: &File) -> u64 {
        u64s(&read_vec(file, 96, 8).unwrap())[0]
    }
}
