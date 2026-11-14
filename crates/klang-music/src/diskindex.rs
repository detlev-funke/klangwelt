//! Melodie-Index auf der Platte (Format KWIX04) – für große Bibliotheken wie
//! den ganzen PDMX-Datensatz (≈ 254 000 Partituren, ≈ 200 Mio. Fundstellen).
//!
//! **Aufbau** ([`IndexWriter`]): Stimmen werden fortlaufend aufgenommen.
//! Stimmangaben und Merkmalsfolgen gehen sofort in Zwischendateien, die
//! Fundstellen (n-Gramm, Stimme, Position) sammeln sich in einem Block, der
//! sortiert als „Lauf“ auf die Platte geschrieben wird, sobald er voll ist.
//! Zum Schluss werden alle Läufe zusammengeführt (k-Wege-Mischen). Der
//! Speicherbedarf bleibt dabei begrenzt (ein Block ≈ 130 MB).
//!
//! **Datei** (alle Zahlen little-endian):
//! Kopf → Stimmangaben (JSON je Stimme) + Offsets → Merkmalsfolgen (2 Byte je
//! Intervall/Rhythmus) + Offsets → Merkmale je Partitur (JSON) → Fundstellen
//! (je n-Gramm: Stimme als Differenz und Position, beides als Varint) →
//! Schlüsseltabelle (n-Gramm, Offset, Länge; sortiert) → dünnes
//! Inhaltsverzeichnis (jeder 64. Schlüssel) → Akkordfolgen-Statistik
//! ([`ChordStats`], seit KWIX04; der letzte Kopfwert zeigt darauf, 0 = keine).
//!
//! **Lesen** ([`DiskIndex`]): Im Speicher liegen nur die Offsets, das dünne
//! Inhaltsverzeichnis, die Merkmale für den Baum und die Akkordstatistik.
//! Eine Suche liest die Listen ihrer n-Gramme und die Merkmalsfolgen der
//! besten Kandidaten. KWIX03-Dateien (ohne Akkordstatistik) bleiben lesbar.

use crate::chordstats::{composer_groups, ChordStats, Progression, GROUPS};
use crate::facets::ScoreFacets;
use crate::index::{gram_key, search_in, DocInfo, MelodyIndex, Posting, SearchData, SearchHit, LEVELS, MAX_POSTINGS_PER_GRAM};
use crate::melody::{features, Feature, MelodyNote};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAGIC: &[u8; 6] = b"KWIX04";
/// Vorgänger ohne Akkordstatistik – wird weiter gelesen.
const MAGIC_V3: &[u8; 6] = b"KWIX03";
/// Magic + 2 Füllbytes + 13 Zahlen.
const HEADER_LEN: usize = 8 + 13 * 8;
/// Fundstellen je Block, bevor er als Lauf geschrieben wird (16 Byte je Eintrag).
const CHUNK: usize = 8_000_000;
/// Jeder wievielte Schlüssel ins Inhaltsverzeichnis kommt.
const SPARSE_STEP: u64 = 64;
/// Bytes je Eintrag der Schlüsseltabelle: n-Gramm (8), Offset (8), Länge (4).
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

/// Liest genau `buf.len()` Bytes ab `off`, ohne die Dateiposition zu teilen.
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.read_exact_at(buf, off)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let (mut done, mut off) = (0usize, off);
        while done < buf.len() {
            let n = f.seek_read(&mut buf[done..], off)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, klang_i18n::tr!("Index file too short")));
            }
            done += n;
            off += n as u64;
        }
        Ok(())
    }
    // Andere Ziele (z. B. wasm32 im Browser): Position setzen und lesen.
    #[cfg(not(any(unix, windows)))]
    {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = f;
        f.seek(SeekFrom::Start(off))?;
        f.read_exact(buf)
    }
}

fn read_vec(f: &File, off: u64, len: u64) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; usize::try_from(len).map_err(io::Error::other)?];
    read_at(f, &mut buf, off)?;
    Ok(buf)
}

fn u64s(bytes: &[u8]) -> Vec<u64> {
    bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

// ---------------------------------------------------------------- Schreiben

/// Statistik eines fertigen Index.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriteStats {
    pub docs: u64,
    pub keys: u64,
    pub postings: u64,
    pub runs: usize,
}

pub struct IndexWriter {
    tmp: PathBuf,
    /// Zwischendateien (beim Abschluss geschlossen – Windows löscht keine offenen Dateien).
    docs: Option<BufWriter<File>>,
    docs_idx: Vec<u64>,
    seqs: Option<BufWriter<File>>,
    seqs_idx: Vec<u64>,
    facets: Vec<ScoreFacets>,
    /// Akkordstatistik je Gruppe (siehe [`GROUPS`]).
    chords: Vec<ChordStats>,
    chunk: Vec<(u64, u32, u32)>,
    runs: Vec<PathBuf>,
    chunk_limit: usize,
}

impl IndexWriter {
    /// `tmp`: Ordner für Zwischendateien (wird angelegt und am Ende gelöscht).
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
        })
    }

    /// Akkordfolge einer Partitur für die Statistik (doc = ihre erste Stimme);
    /// `composer` ordnet sie zusätzlich den Gruppen Barock/Bach zu.
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

    /// Nimmt eine Melodielinie auf (wie [`MelodyIndex::add_melody`]: zu kurze Linien zählen nicht).
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
        // Erst nach einer ganzen Stimme leeren: eine Stimme liegt nie in zwei Läufen.
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

    /// Führt alles zur fertigen Indexdatei `out` zusammen.
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
        let mut pos = HEADER_LEN as u64;
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

        // Läufe zusammenführen: je n-Gramm alle Fundstellen in Stimmenreihenfolge.
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
            from.sort_unstable(); // Läufe sind zeitlich geordnet → Stimmen aufsteigend
            list.clear();
            for &i in &from {
                readers[i].read_group(&mut list)?;
                if let Some(k) = readers[i].key() {
                    heap.push(Reverse((k, i)));
                }
            }
            // Wie im Speicher-Index: zu häufige n-Gramme übergeht die Suche, also nicht speichern.
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

/// Liest einen Lauf gruppenweise: (n-Gramm, Anzahl, Anzahl × (Stimme, Position)).
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

// ---------------------------------------------------------------- Lesen

pub struct DiskIndex {
    file: File,
    docs_off: u64,
    docs_idx: Vec<u64>,
    seqs_off: u64,
    seqs_idx: Vec<u64>,
    post_off: u64,
    keys_off: u64,
    n_keys: u64,
    sparse: Vec<u64>,
    facets: Vec<ScoreFacets>,
    chords: Vec<ChordStats>,
}

impl DiskIndex {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut head = [0u8; HEADER_LEN];
        read_at(&file, &mut head, 0)?;
        let v4 = &head[..6] == MAGIC;
        if !v4 && &head[..6] != MAGIC_V3 {
            return Err(invalid(&klang_i18n::tr!("not a Klangwelt index file (format 3/4)")));
        }
        let v = u64s(&head[8..]);
        let [docs_off, docs_idx_off, seqs_off, seqs_idx_off, facets_off, post_off, keys_off, sparse_off, end, n_docs, n_keys, _n_post, chords_off] =
            v[..13].try_into().map_err(|_| invalid(&klang_i18n::tr!("Header corrupted")))?;
        let chords_off = if v4 { chords_off } else { 0 };
        let sparse_end = if chords_off > 0 { chords_off } else { end };
        if file.metadata()?.len() < end || !(docs_off <= docs_idx_off && docs_idx_off <= seqs_off && seqs_off <= seqs_idx_off
            && seqs_idx_off <= facets_off && facets_off <= post_off && post_off <= keys_off && keys_off <= sparse_off && sparse_off <= sparse_end
            && sparse_end <= end)
        {
            return Err(invalid(&klang_i18n::tr!("Index file incomplete or corrupted")));
        }
        let docs_idx = u64s(&read_vec(&file, docs_idx_off, 8 * (n_docs + 1))?);
        let seqs_idx = u64s(&read_vec(&file, seqs_idx_off, 8 * (n_docs + 1))?);
        let facets: Vec<ScoreFacets> = serde_json::from_slice(&read_vec(&file, facets_off, post_off - facets_off)?).map_err(io::Error::other)?;
        let sparse = u64s(&read_vec(&file, sparse_off, sparse_end - sparse_off)?);
        if keys_off + n_keys * KEY_ENTRY as u64 != sparse_off {
            return Err(invalid(&klang_i18n::tr!("Key table corrupted")));
        }
        let chords = if chords_off > 0 { ChordStats::decode_groups(&read_vec(&file, chords_off, end - chords_off)?)? } else { Vec::new() };
        Ok(Self { file, docs_off, docs_idx, seqs_off, seqs_idx, post_off, keys_off, n_keys, sparse, facets, chords })
    }

    /// Akkordfolgen-Statistik aller Stücke (None bei Indexdateien vor KWIX04).
    pub fn chords(&self) -> Option<&ChordStats> {
        self.chords.first()
    }

    /// Statistik einer Gruppe (0 = alle, 1 = Barock, 2 = Bach), falls vorhanden.
    pub fn chords_group(&self, g: usize) -> Option<&ChordStats> {
        self.chords.get(g)
    }

    pub fn doc_count(&self) -> usize {
        self.docs_idx.len() - 1
    }

    pub fn gram_count(&self) -> usize {
        self.n_keys as usize
    }

    pub fn facets(&self) -> &[ScoreFacets] {
        &self.facets
    }

    pub fn doc(&self, id: u32) -> Option<DocInfo> {
        let i = id as usize;
        let (a, b) = (*self.docs_idx.get(i)?, *self.docs_idx.get(i + 1)?);
        let raw = read_vec(&self.file, self.docs_off + a, b - a).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// Schlüssel → (Offset, Länge) der Fundstellen.
    fn lookup(&self, key: u64) -> Option<(u64, u32)> {
        let block = self.sparse.partition_point(|&k| k <= key).checked_sub(1)?;
        let start = block as u64 * SPARSE_STEP;
        let end = (start + SPARSE_STEP).min(self.n_keys);
        let raw = read_vec(&self.file, self.keys_off + start * KEY_ENTRY as u64, (end - start) * KEY_ENTRY as u64).ok()?;
        let entries: Vec<(u64, u64, u32)> = raw
            .chunks_exact(KEY_ENTRY)
            .map(|c| (u64::from_le_bytes(c[..8].try_into().unwrap()), u64::from_le_bytes(c[8..16].try_into().unwrap()), u32::from_le_bytes(c[16..].try_into().unwrap())))
            .collect();
        let i = entries.binary_search_by_key(&key, |e| e.0).ok()?;
        Some((entries[i].1, entries[i].2))
    }

    pub fn search(&self, query: &[MelodyNote], limit: usize) -> Vec<SearchHit> {
        search_in(self, query, limit)
    }

    /// Textsuche über Titel, Komponist, Stimmnamen, Genre, Instrumentengruppen,
    /// Tonart und Stimmung (je Partitur; alle Wörter müssen vorkommen).
    pub fn search_text(&self, query: &str, limit: usize) -> Vec<(u32, DocInfo)> {
        facet_text_search(self.facets.iter(), query).take(limit).filter_map(|doc| self.doc(doc).map(|d| (doc, d))).collect()
    }
}

impl SearchData for DiskIndex {
    fn postings(&self, key: u64) -> Option<Vec<Posting>> {
        let (rel, len) = self.lookup(key)?;
        let raw = read_vec(&self.file, self.post_off + rel, len as u64).ok()?;
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
        let i = doc as usize;
        let (Some(&a), Some(&b)) = (self.seqs_idx.get(i), self.seqs_idx.get(i + 1)) else { return Vec::new() };
        read_vec(&self.file, self.seqs_off + a, b - a)
            .map(|raw| raw.chunks_exact(2).map(|c| Feature { interval: c[0] as i8, rhythm: c[1] as i8 }).collect())
            .unwrap_or_default()
    }

    fn info(&self, doc: u32) -> DocInfo {
        self.doc(doc).unwrap_or_default()
    }
}

// ---------------------------------------------------------------- beide Formate

/// Bibliotheksindex: neu auf der Platte (Format 3/4) oder älter im Speicher
/// (1/2) – ggf. mit einzeln hinzugefügten Partituren darüber ([`Layered`]).
pub enum AnyIndex {
    Mem(MelodyIndex),
    Disk(DiskIndex),
    Layered(Box<Layered>),
}

/// Ein Index plus Zusatz: einzeln hinzugefügte Partituren liegen in einer
/// kleinen Nebendatei ([`extra_path`]); der große Index bleibt unverändert.
/// Die Stimmen des Zusatzes folgen auf die des Grundindex (Nummern ab
/// `offset`), die Merkmale beider liegen zusammen in `facets`.
pub struct Layered {
    base: AnyIndex,
    extra: MelodyIndex,
    offset: u32,
    base_scores: usize,
    /// Der Grundindex hatte eigene Merkmale (sonst: alte Textsuche über Stimmen).
    base_has_facets: bool,
    facets: Vec<ScoreFacets>,
}

/// Nebendatei mit einzeln hinzugefügten Partituren zu einem Index:
/// `klangwelt-bibliothek.kwix` → `klangwelt-bibliothek.zusatz.kwix`.
pub fn extra_path(index: &Path) -> PathBuf {
    let stem = index.file_stem().and_then(|s| s.to_str()).unwrap_or("klangwelt-bibliothek");
    index.with_file_name(format!("{stem}.zusatz.kwix"))
}

fn is_extra_file(path: &Path) -> bool {
    path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.ends_with(".zusatz"))
}

/// Textsuche über die Merkmale je Partitur (alle Wörter müssen vorkommen).
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
    /// Lädt einen Index; liegt daneben eine Zusatzdatei ([`extra_path`]), kommt sie dazu.
    pub fn load(path: &Path) -> io::Result<Self> {
        let base = Self::load_plain(path)?;
        let extra = extra_path(path);
        if is_extra_file(path) || !extra.is_file() {
            return Ok(base);
        }
        // Ein beschädigter Zusatz soll die große Bibliothek nicht blockieren.
        match MelodyIndex::load(&extra) {
            Ok(e) => Ok(Self::layered(base, e)),
            Err(_) => Ok(base),
        }
    }

    /// Lädt nur die Datei selbst, ohne Zusatz.
    pub fn load_plain(path: &Path) -> io::Result<Self> {
        let mut magic = [0u8; 6];
        File::open(path)?.read_exact(&mut magic)?;
        if &magic == MAGIC || &magic == MAGIC_V3 {
            Ok(Self::Disk(DiskIndex::open(path)?))
        } else {
            Ok(Self::Mem(MelodyIndex::load(path)?))
        }
    }

    /// Legt `extra` über `base` (Stimmnummern des Zusatzes werden verschoben).
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

    /// Quelldateien der einzeln hinzugefügten Partituren (ohne Doppelte).
    pub fn extra_sources(&self) -> Vec<String> {
        let Self::Layered(l) = self else { return Vec::new() };
        let mut out: Vec<String> = l.extra.parts().0.iter().map(|d| d.source.clone()).collect();
        out.dedup();
        out
    }

    /// Anzahl einzeln hinzugefügter Partituren.
    pub fn extra_scores(&self) -> usize {
        match self {
            Self::Layered(l) => l.extra.facets().len(),
            _ => 0,
        }
    }

    /// Akkordfolgen-Statistik, falls der Index eine hat (ab KWIX04).
    pub fn chords(&self) -> Option<&ChordStats> {
        match self {
            Self::Mem(_) => None,
            Self::Disk(d) => d.chords(),
            Self::Layered(l) => l.base.chords(),
        }
    }

    /// Statistik einer Gruppe (0 = alle, 1 = Barock, 2 = Bach).
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
            Self::Disk(d) => d.facets().len(),
            Self::Layered(l) => l.base_scores + l.extra.facets().len(),
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
            Self::Layered(l) => {
                // Ohne eigene Merkmale sucht der Grundindex selbst (über seine Stimmen).
                let mut out = if l.base_has_facets { Vec::new() } else { l.base.search_text(query, limit) };
                let rest = limit.saturating_sub(out.len());
                out.extend(facet_text_search(l.facets.iter(), query).take(rest).filter_map(|doc| self.doc(doc).map(|d| (doc, d))));
                out
            }
        }
    }
}

// ---------------------------------------------------------------- Verschmelzen
//
// Mehrere Indizes werden zu einer KWIX04-Datei, ohne eine einzige Partitur neu
// zu lesen: Stimmangaben und Merkmalsfolgen werden hintereinander kopiert
// (Nummern verschoben), die Fundstellen über die sortierten Schlüsseltabellen
// zusammengeführt, Merkmale und Akkordstatistik umnummeriert. Ein n-Gramm, das
// nur die erste Quelle kennt, wird unverändert kopiert – bei einer großen
// Bibliothek plus einem kleinen Paket also fast alles.

/// Ergebnis von [`merge`].
#[derive(Clone, Copy, Debug, Default)]
pub struct MergeStats {
    pub docs: u64,
    pub scores: u64,
    pub keys: u64,
    pub postings: u64,
    /// Stimmen bzw. Partituren, die eine frühere Quelle schon enthielt (übersprungen).
    pub skipped_docs: u64,
    pub skipped_scores: u64,
}

/// Schlüssel, unter dem eine Partitur nur einmal in eine Bibliothek kommt:
/// PDMX-Dateien heißen nach ihrer IPFS-Kennung („Qm…“, weltweit eindeutig) –
/// so fallen Kostprobe und Auswahl in der vollen PDMX zusammen, egal in
/// welchem Ordner sie liegen. Alle anderen nach ihrem Pfad.
pub fn source_key(source: &str) -> String {
    let norm = source.replace('/', "\\");
    let name = norm.rsplit('\\').next().unwrap_or(&norm);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    if stem.len() == 46 && stem.starts_with("Qm") && stem.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return format!("pdmx:{stem}");
    }
    norm.strip_prefix(r"\\?\").unwrap_or(&norm).to_lowercase()
}

/// Teil eines Index (ein geschichteter Index besteht aus zweien).
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

/// Liest einen Bereich der Datei der Reihe nach in großen Blöcken.
struct BlockReader<'a> {
    file: &'a File,
    /// Dateiposition des ersten Bytes in `buf`.
    start: u64,
    buf: Vec<u8>,
    at: usize,
    end: u64,
}

impl<'a> BlockReader<'a> {
    const BLOCK: u64 = 4 << 20;

    fn new(file: &'a File, pos: u64, end: u64) -> Self {
        Self { file, start: pos, buf: Vec::new(), at: 0, end }
    }

    /// `len` Bytes ab der Dateiposition `pos` (meist direkt hinter dem letzten Lesen).
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

    /// `f(i, json)` für jede Stimmangabe der Reihe nach.
    fn each_doc(&self, mut f: impl FnMut(usize, &[u8]) -> io::Result<()>) -> io::Result<()> {
        match self {
            Part::Disk(d) => each_slice(&d.file, d.docs_off, &d.docs_idx, f),
            Part::Mem(m) => {
                for (i, doc) in m.parts().0.iter().enumerate() {
                    f(i, &serde_json::to_vec(doc).map_err(io::Error::other)?)?;
                }
                Ok(())
            }
        }
    }

    /// `f(i, folge)` für jede Merkmalsfolge (2 Byte je Merkmal) der Reihe nach.
    fn each_seq(&self, mut f: impl FnMut(usize, &[u8]) -> io::Result<()>) -> io::Result<()> {
        match self {
            Part::Disk(d) => each_slice(&d.file, d.seqs_off, &d.seqs_idx, f),
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
                let table = BlockReader::new(&d.file, d.keys_off, d.keys_off + d.n_keys * KEY_ENTRY as u64);
                let posts = BlockReader::new(&d.file, d.post_off, d.keys_off);
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

/// Einträge `idx[i]..idx[i+1]` ab `base` der Reihe nach, in Blöcken gelesen.
fn each_slice(file: &File, base: u64, idx: &[u64], mut f: impl FnMut(usize, &[u8]) -> io::Result<()>) -> io::Result<()> {
    let n = idx.len().saturating_sub(1);
    let mut i = 0;
    let mut buf = Vec::new();
    while i < n {
        // so viele Einträge wie in einen Block passen (mindestens einer)
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

/// Läuft in Schlüsselreihenfolge durch die Fundstellen eines Teils.
struct KeyCursor<'a> {
    src: KeySrc<'a>,
    key: Option<u64>,
    /// Fundstellen des aktuellen Schlüssels: Varints, Stimme als Differenz (ab 0) und Position.
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

/// Neue Nummern der Stimmen eines Teils.
struct Renumber {
    /// erste Stimme des Teils in der Zählung aller Quellen
    old_start: u32,
    count: u32,
    /// None: alle bleiben, neue Nummer = alt + `shift`; sonst je Stimme (u32::MAX = entfällt)
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

/// Verschmilzt `sources` (in dieser Reihenfolge) zu einer KWIX04-Datei `out`.
/// Mit `dedupe` kommt jede Partitur nur einmal hinein ([`source_key`]): Was eine
/// frühere Quelle schon hat, wird bei späteren übersprungen. `progress(0…1)`
/// kommt regelmäßig; `cancel` bricht ab (Fehler `Interrupted`, `out` bleibt
/// unberührt). Die Akkordzählungen übersprungener Partituren bleiben enthalten.
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
    // Teile aller Quellen mit ihrer Startnummer in der Gesamtzählung
    let mut parts: Vec<(Part, u32)> = Vec::new();
    let mut input_start = Vec::with_capacity(sources.len());
    let mut total = 0u32;
    for s in sources {
        input_start.push(total);
        collect_parts(s, total, &mut parts);
        total += s.doc_count() as u32;
    }

    // Neue Nummern (Doppelte überspringen)
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

    // Stimmangaben und Merkmalsfolgen
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

    // Merkmale je Partitur (alte Indizes ohne Merkmale: aus den Stimmangaben)
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

    // Fundstellen: alle Teile in Schlüsselreihenfolge zusammenführen
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
                // nur in einem Teil und dort unverändert: Bytes übernehmen
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

    // Akkordstatistik je Gruppe
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

/// Merkmale für Indizes ohne eigene (Format 1): je Quelldatei ein Eintrag.
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

    /// Pseudozufällige, aber reproduzierbare Melodien.
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
        // Kleine Blöcke erzwingen viele Läufe → Zusammenführen wird mitgetestet.
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
        // Fundstellen identisch, auch für unbekannte Schlüssel
        for t in tunes.iter().take(40) {
            let f = features(t);
            for pos in 0..f.len().saturating_sub(4) {
                let k = gram_key(1, &f[pos..pos + 4]);
                assert_eq!(SearchData::postings(disk, k), SearchData::postings(&mem, k));
            }
        }
        assert_eq!(SearchData::postings(disk, 0xdead_beef), None);
        // Suche liefert dieselben Treffer
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

    /// Akkordfolge I – V7 – I (bei `vi` zusätzlich V7 – vi).
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

    /// Index aus den Melodien `range` (je zwei Stimmen eine Partitur) als Datei.
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
        // Akkordstatistik addiert sich; Beispiele zeigen auf die richtigen Stücke
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
        // PDMX-Dateien heißen überall gleich, egal in welchem Ordner
        let cid = |i: usize| format!("Qm{:0>44}", i / 2);
        let big = AnyIndex::load(&build(&dir, "pdmx", &tunes, 0..80, |i| format!(r"C:\Bib\PDMX\{}.mxl", cid(i)))).unwrap();
        // Kostprobe: 20 Stimmen schon in der großen, 20 neu
        let small = AnyIndex::load(&build(&dir, "kostprobe", &tunes, 60..100, |i| format!(r"C:\Bib\PDMX-Kostprobe\{}.mxl", cid(i)))).unwrap();
        // Zusatz (Speicherformat) über der Kostprobe: ein Stück neu, eins doppelt (gleicher Pfad, andere Schreibweise)
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
        // jede Partitur genau einmal, Merkmale zeigen auf ihre Stimmen
        let mut titles: Vec<String> = m.facets().iter().map(|f| f.title.clone()).collect();
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), 51);
        for f in m.facets() {
            assert_eq!(m.doc(f.doc).unwrap().title, f.title);
        }
        // neue Stücke aus beiden Teilen der zweiten Quelle werden gefunden
        for i in [90usize, 100] {
            let hit = &m.search(&tunes[i][1..9].to_vec(), 1)[0];
            assert_eq!(hit.info.part, format!("Stimme {i}"));
        }
        // ohne Abgleich bleibt alles drin
        let all = dir.join("alles.kwix");
        assert_eq!(merge(&[&big, &layered], &all, false, &|_| {}, &AtomicBool::new(false)).unwrap().docs, 124);
        // Abbrechen lässt nichts zurück
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
}
