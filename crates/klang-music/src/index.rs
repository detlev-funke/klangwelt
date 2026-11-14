//! Melodie-Index: n-Gramme über Intervall- und Rhythmusmerkmale.
//!
//! Suche in zwei Stufen:
//! 1. **Abstimmung**: Jedes n-Gramm der Anfrage stimmt für (Stück, Versatz).
//!    Drei Genauigkeitsstufen (exakt, nur Intervalle, nur Kontur) machen die
//!    Suche robust gegen unsauberes Summen.
//! 2. **Feinbewertung**: Die besten Kandidaten werden per Alignment
//!    (semi-global, mit Lücken) gegen die Anfrage bewertet.

use crate::facets::ScoreFacets;
use crate::melody::{features, skyline, Feature, MelodyNote};
use crate::score::Score;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocInfo {
    pub source: String,
    pub title: String,
    pub composer: String,
    pub part: String,
    pub voice: String,
    pub note_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Posting {
    pub(crate) doc: u32,
    pub(crate) pos: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct Level {
    pub(crate) id: u8,
    pub(crate) n: usize,
    weight: f32,
}

pub(crate) const LEVELS: [Level; 3] = [
    Level { id: 1, n: 4, weight: 3.0 }, // Intervall + Rhythmus
    Level { id: 2, n: 4, weight: 2.0 }, // nur Intervall
    Level { id: 3, n: 6, weight: 1.0 }, // nur Kontur
];

/// n-Gramme, die in sehr vielen Stücken vorkommen (Tonleitern …), tragen kaum Information.
pub(crate) const MAX_POSTINGS_PER_GRAM: usize = 20_000;
const MAX_CANDIDATES: usize = 300;
pub const MIN_QUERY_NOTES: usize = 5;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub doc: u32,
    pub info: DocInfo,
    /// 0…1, 1 = perfekte Übereinstimmung von Intervallen und Rhythmus.
    pub score: f32,
    /// Notenindex in der gefundenen Stimme, an dem der Treffer beginnt.
    pub note_index: usize,
}

#[derive(Default)]
pub struct MelodyIndex {
    docs: Vec<DocInfo>,
    seqs: Vec<Vec<Feature>>,
    postings: HashMap<u64, Vec<Posting>>,
    /// n-Gramme mit mehr als `MAX_POSTINGS_PER_GRAM` Fundstellen: die Suche
    /// übergeht sie ohnehin, also werden sie gar nicht erst gespeichert
    /// (spart bei großen Bibliotheken viel Speicher). Nur beim Aufbau nötig.
    stop: std::collections::HashSet<u64>,
    /// Merkmale je Partitur zum Stöbern (leer bei Indexdateien vor Format 2).
    facets: Vec<ScoreFacets>,
}

pub(crate) fn gram_key(level: u8, grams: &[Feature]) -> u64 {
    // FNV-1a, deterministisch über Programmläufe hinweg.
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    };
    eat(level);
    for f in grams {
        match level {
            1 => {
                eat(f.interval as u8);
                eat(f.rhythm as u8);
            }
            2 => eat(f.interval as u8),
            _ => eat(f.contour() as u8),
        }
    }
    h
}

fn similarity(a: Feature, b: Feature) -> f32 {
    let di = (a.interval as i16 - b.interval as i16).abs();
    let interval = match di {
        0 => 2.0,
        1 => 0.6,
        _ if a.contour() == b.contour() => 0.2,
        _ => -1.0,
    };
    let rhythm = match (a.rhythm as i16 - b.rhythm as i16).abs() {
        0 => 1.0,
        1 => 0.3,
        _ => -0.4,
    };
    interval + rhythm
}

const MAX_SIM: f32 = 3.0;
const GAP: f32 = 1.5;

/// Semi-globales Alignment: Anfrage vollständig, Ziel ausschnittsweise.
/// Liefert (Punkte, Startposition im Ziel).
fn align(q: &[Feature], t: &[Feature]) -> (f32, usize) {
    let (m, n) = (q.len(), t.len());
    let mut prev: Vec<(f32, usize)> = (0..=n).map(|j| (0.0, j)).collect();
    let mut cur = vec![(0.0f32, 0usize); n + 1];
    for i in 1..=m {
        cur[0] = (-(GAP * i as f32), 0);
        for j in 1..=n {
            let diag = (prev[j - 1].0 + similarity(q[i - 1], t[j - 1]), prev[j - 1].1);
            let up = (prev[j].0 - GAP, prev[j].1);
            let left = (cur[j - 1].0 - GAP, cur[j - 1].1);
            cur[j] = [diag, up, left].into_iter().fold((f32::MIN, 0), |a, b| if b.0 > a.0 { b } else { a });
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev.into_iter().fold((f32::MIN, 0), |a, b| if b.0 > a.0 { b } else { a })
}

/// Datenquelle der Melodiesuche – Index im Speicher oder auf der Platte.
pub(crate) trait SearchData {
    /// Fundstellen eines n-Gramms (`None`, wenn unbekannt).
    fn postings(&self, key: u64) -> Option<Vec<Posting>>;
    fn seq(&self, doc: u32) -> Vec<Feature>;
    fn info(&self, doc: u32) -> DocInfo;
}

/// Melodiesuche: Abstimmung über n-Gramme, dann Alignment der besten Kandidaten.
pub(crate) fn search_in(data: &impl SearchData, query: &[MelodyNote], limit: usize) -> Vec<SearchHit> {
    let q = features(query);
    if q.len() + 1 < MIN_QUERY_NOTES {
        return Vec::new();
    }

    let mut votes: HashMap<(u32, i64), f32> = HashMap::new();
    for level in LEVELS {
        if q.len() < level.n {
            continue;
        }
        for qpos in 0..=q.len() - level.n {
            let Some(list) = data.postings(gram_key(level.id, &q[qpos..qpos + level.n])) else {
                continue;
            };
            if list.len() > MAX_POSTINGS_PER_GRAM {
                continue;
            }
            for p in &list {
                *votes.entry((p.doc, p.pos as i64 - qpos as i64)).or_default() += level.weight;
            }
        }
    }

    let mut cands: Vec<((u32, i64), f32)> = votes.into_iter().collect();
    cands.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    cands.truncate(MAX_CANDIDATES);

    let mut seqs: HashMap<u32, Vec<Feature>> = HashMap::new();
    let mut best: HashMap<u32, (f32, usize)> = HashMap::new();
    let ql = q.len() as i64;
    for ((doc, offset), _) in cands {
        let seq = seqs.entry(doc).or_insert_with(|| data.seq(doc));
        let from = (offset - ql / 2).max(0) as usize;
        let to = ((offset + ql * 3 / 2 + 2).max(0) as usize).min(seq.len());
        if from >= to {
            continue;
        }
        let (pts, start) = align(&q, &seq[from..to]);
        let score = (pts / (q.len() as f32 * MAX_SIM)).clamp(0.0, 1.0);
        let e = best.entry(doc).or_insert((0.0, 0));
        if score > e.0 {
            *e = (score, from + start);
        }
    }

    let mut ranked: Vec<(u32, f32, usize)> = best.into_iter().map(|(doc, (score, idx))| (doc, score, idx)).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(limit);
    ranked
        .into_iter()
        .map(|(doc, score, note_index)| SearchHit { doc, info: data.info(doc), score, note_index })
        .collect()
}

impl SearchData for MelodyIndex {
    fn postings(&self, key: u64) -> Option<Vec<Posting>> {
        self.postings.get(&key).cloned()
    }
    fn seq(&self, doc: u32) -> Vec<Feature> {
        self.seqs.get(doc as usize).cloned().unwrap_or_default()
    }
    fn info(&self, doc: u32) -> DocInfo {
        self.docs.get(doc as usize).cloned().unwrap_or_default()
    }
}

impl MelodyIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }

    pub fn gram_count(&self) -> usize {
        self.postings.len()
    }

    pub fn doc(&self, id: u32) -> Option<&DocInfo> {
        self.docs.get(id as usize)
    }

    /// Anzahl verschiedener Partituren (Quelldateien).
    pub fn score_count(&self) -> usize {
        let mut s: Vec<&str> = self.docs.iter().map(|d| d.source.as_str()).collect();
        s.sort_unstable();
        s.dedup();
        s.len()
    }

    /// Textsuche in Titel, Komponist und Stimme (alle Wörter müssen vorkommen, ohne Groß-/Kleinschreibung).
    pub fn search_text(&self, query: &str, limit: usize) -> Vec<(u32, DocInfo)> {
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        self.docs
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                let hay = format!("{} {} {} {}", d.title, d.composer, d.part, d.source).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .take(limit)
            .map(|(i, d)| (i as u32, d.clone()))
            .collect()
    }

    /// Nimmt eine Melodielinie auf. Zu kurze Linien werden ignoriert.
    pub fn add_melody(&mut self, mut info: DocInfo, notes: &[MelodyNote]) -> Option<u32> {
        if notes.len() < 8 {
            return None;
        }
        let f = features(notes);
        let doc = self.docs.len() as u32;
        info.note_count = notes.len() as u32;
        for level in LEVELS {
            if f.len() < level.n {
                continue;
            }
            for pos in 0..=f.len() - level.n {
                let key = gram_key(level.id, &f[pos..pos + level.n]);
                if self.stop.contains(&key) {
                    continue;
                }
                let list = self.postings.entry(key).or_default();
                list.push(Posting { doc, pos: pos as u32 });
                if list.len() > MAX_POSTINGS_PER_GRAM {
                    self.postings.remove(&key);
                    self.stop.insert(key);
                }
            }
        }
        self.docs.push(info);
        self.seqs.push(f);
        Some(doc)
    }

    /// Merkmale einer Partitur aufnehmen (`facets.doc` = ihre erste Stimme).
    pub fn add_facets(&mut self, facets: ScoreFacets) {
        self.facets.push(facets);
    }

    pub fn facets(&self) -> &[ScoreFacets] {
        &self.facets
    }

    pub(crate) fn take_facets(&mut self) -> Vec<ScoreFacets> {
        std::mem::take(&mut self.facets)
    }

    /// Stimmangaben, Merkmalsfolgen und Fundstellen (zum Verschmelzen).
    pub(crate) fn parts(&self) -> (&[DocInfo], &[Vec<Feature>], &HashMap<u64, Vec<Posting>>) {
        (&self.docs, &self.seqs, &self.postings)
    }

    /// Ist diese Quelldatei schon im Index?
    pub fn has_source(&self, source: &str) -> bool {
        self.docs.iter().any(|d| d.source == source)
    }

    /// Nach dem Aufbau: Reserveplatz der Listen freigeben (spart rund ein Viertel).
    pub fn compact(&mut self) {
        for list in self.postings.values_mut() {
            list.shrink_to_fit();
        }
        self.postings.shrink_to_fit();
        self.seqs.shrink_to_fit();
        self.docs.shrink_to_fit();
        self.stop = Default::default();
    }

    /// Anzahl gespeicherter Fundstellen (für Statistik und Tests).
    pub fn posting_count(&self) -> usize {
        self.postings.values().map(Vec::len).sum()
    }

    /// Nimmt alle Stimmen einer Partitur auf (jeweils deren Oberstimme).
    pub fn add_score(&mut self, score: &Score, source: &str) -> usize {
        let mut added = 0;
        for part in &score.parts {
            for v in &part.voices {
                let info = DocInfo {
                    source: source.to_string(),
                    title: score.title.clone().unwrap_or_default(),
                    composer: score.composer.clone().unwrap_or_default(),
                    part: part.name.clone(),
                    voice: format!("System {} · Stimme {}", v.staff, v.voice),
                    note_count: 0,
                };
                if self.add_melody(info, &skyline(&v.notes)).is_some() {
                    added += 1;
                }
            }
        }
        added
    }

    pub fn search(&self, query: &[MelodyNote], limit: usize) -> Vec<SearchHit> {
        search_in(self, query, limit)
    }

    // ---------- Speichern / Laden (kompaktes Binärformat) ----------

    const MAGIC_V1: &'static [u8; 6] = b"KWIX01";
    /// Format 2: wie 1, danach die Merkmale je Partitur (JSON).
    const MAGIC: &'static [u8; 6] = b"KWIX02";

    pub fn save(&self, path: &Path) -> io::Result<()> {
        // Erst in eine Nebendatei schreiben, dann ersetzen: kein halber Index bei Abbruch.
        let tmp = path.with_extension("kwix.teil");
        self.write_to(&tmp)?;
        std::fs::rename(&tmp, path)
    }

    fn write_to(&self, path: &Path) -> io::Result<()> {
        let mut w = io::BufWriter::new(std::fs::File::create(path)?);
        w.write_all(Self::MAGIC)?;
        let docs = serde_json::to_vec(&self.docs).map_err(io::Error::other)?;
        w.write_all(&(docs.len() as u64).to_le_bytes())?;
        w.write_all(&docs)?;
        w.write_all(&(self.seqs.len() as u32).to_le_bytes())?;
        for s in &self.seqs {
            w.write_all(&(s.len() as u32).to_le_bytes())?;
            for f in s {
                w.write_all(&[f.interval as u8, f.rhythm as u8])?;
            }
        }
        let mut keys: Vec<&u64> = self.postings.keys().collect();
        keys.sort();
        w.write_all(&(keys.len() as u64).to_le_bytes())?;
        for k in keys {
            let list = &self.postings[k];
            w.write_all(&k.to_le_bytes())?;
            w.write_all(&(list.len() as u32).to_le_bytes())?;
            for p in list {
                w.write_all(&p.doc.to_le_bytes())?;
                w.write_all(&p.pos.to_le_bytes())?;
            }
        }
        let facets = serde_json::to_vec(&self.facets).map_err(io::Error::other)?;
        w.write_all(&(facets.len() as u64).to_le_bytes())?;
        w.write_all(&facets)?;
        w.flush()
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let mut r = io::BufReader::new(std::fs::File::open(path)?);
        let mut magic = [0u8; 6];
        r.read_exact(&mut magic)?;
        let v2 = &magic == Self::MAGIC;
        if !v2 && &magic != Self::MAGIC_V1 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "keine Klangwelt-Indexdatei"));
        }
        let mut b8 = [0u8; 8];
        let mut b4 = [0u8; 4];
        let mut u64r = |r: &mut io::BufReader<std::fs::File>| -> io::Result<u64> {
            r.read_exact(&mut b8)?;
            Ok(u64::from_le_bytes(b8))
        };
        let docs_len = u64r(&mut r)? as usize;
        let mut docs_buf = vec![0u8; docs_len];
        r.read_exact(&mut docs_buf)?;
        let docs: Vec<DocInfo> = serde_json::from_slice(&docs_buf).map_err(io::Error::other)?;

        let mut u32r = |r: &mut io::BufReader<std::fs::File>| -> io::Result<u32> {
            r.read_exact(&mut b4)?;
            Ok(u32::from_le_bytes(b4))
        };
        let nseq = u32r(&mut r)? as usize;
        let mut seqs = Vec::with_capacity(nseq);
        for _ in 0..nseq {
            let len = u32r(&mut r)? as usize;
            let mut raw = vec![0u8; len * 2];
            r.read_exact(&mut raw)?;
            seqs.push(raw.chunks_exact(2).map(|c| Feature { interval: c[0] as i8, rhythm: c[1] as i8 }).collect());
        }
        let nkeys = u64r(&mut r)? as usize;
        let mut postings = HashMap::with_capacity(nkeys);
        for _ in 0..nkeys {
            let key = u64r(&mut r)?;
            let len = u32r(&mut r)? as usize;
            let mut list = Vec::with_capacity(len);
            for _ in 0..len {
                let doc = u32r(&mut r)?;
                let pos = u32r(&mut r)?;
                list.push(Posting { doc, pos });
            }
            postings.insert(key, list);
        }
        if seqs.len() != docs.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Index ist inkonsistent"));
        }
        let facets = if v2 {
            let len = u64r(&mut r)? as usize;
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf)?;
            serde_json::from_slice(&buf).map_err(io::Error::other)?
        } else {
            Vec::new()
        };
        Ok(Self { docs, seqs, postings, stop: Default::default(), facets })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::parse_musicxml;
    use crate::score::tests::make_xml;

    /// Kleine Bibliothek aus frei erfundenen Melodien (Achtel = 1).
    fn library() -> MelodyIndex {
        let tunes: [(&str, &[(u8, u32)]); 4] = [
            ("Aufstieg", &[(60, 2), (62, 2), (64, 2), (65, 2), (67, 4), (67, 4), (69, 2), (69, 2), (69, 2), (69, 2), (67, 8), (65, 2), (65, 2), (65, 2), (65, 2), (64, 4), (64, 4)]),
            ("Bogen", &[(67, 1), (72, 1), (71, 2), (69, 2), (67, 2), (64, 4), (65, 2), (67, 2), (69, 4), (67, 2), (65, 2), (64, 2), (62, 2), (60, 8)]),
            ("Sprünge", &[(60, 2), (67, 2), (64, 2), (72, 2), (69, 4), (65, 4), (62, 2), (70, 2), (67, 4), (60, 4), (64, 2), (71, 2), (67, 8)]),
            ("Wiegen", &[(64, 3), (65, 1), (64, 2), (62, 2), (60, 3), (62, 1), (64, 4), (64, 3), (65, 1), (67, 2), (65, 2), (64, 8)]),
        ];
        let mut idx = MelodyIndex::new();
        for (title, notes) in tunes {
            let s = parse_musicxml(&make_xml(title, &[("Melodie", notes)])).unwrap();
            assert_eq!(idx.add_score(&s, title), 1);
        }
        idx
    }

    fn hum(notes: &[(f64, u8)], tempo_scale: f64, transpose: i16) -> Vec<MelodyNote> {
        let mut t = 0.0;
        notes
            .iter()
            .map(|&(d, m)| {
                let n = MelodyNote { onset: t, duration: d * tempo_scale, midi: (m as i16 + transpose) as u8 };
                t += d * tempo_scale;
                n
            })
            .collect()
    }

    #[test]
    fn finds_transposed_and_faster_excerpt() {
        let idx = library();
        // Ausschnitt aus „Bogen“, eine Quarte höher und 1,7× so schnell gesummt.
        let q = hum(&[(1.0, 69), (1.0, 67), (2.0, 64), (1.0, 65), (1.0, 67), (2.0, 69), (1.0, 67)], 0.3, 5);
        let hits = idx.search(&q, 3);
        assert_eq!(hits[0].info.title, "Bogen", "{hits:?}");
        assert!(hits[0].score > 0.9, "{}", hits[0].score);
    }

    #[test]
    fn tolerates_a_wrong_note() {
        let idx = library();
        // „Wiegen“ mit einem falschen Ton (63 statt 62) und ungenauem Rhythmus.
        let q = hum(
            &[
                (1.5, 64), (0.5, 65), (1.0, 64), (1.0, 63), (1.6, 60), (0.4, 62),
                (2.0, 64), (1.5, 64), (0.5, 65), (1.0, 67), (1.0, 65), (4.0, 64),
            ],
            1.0,
            -2,
        );
        let hits = idx.search(&q, 3);
        assert!(!hits.is_empty(), "keine Treffer");
        assert_eq!(hits[0].info.title, "Wiegen", "{hits:?}");
        assert!(hits[0].score > 0.6 && hits[0].score < 1.0, "{}", hits[0].score);
    }

    #[test]
    fn overly_common_grams_are_not_stored_and_search_still_works() {
        let mut idx = library();
        // Eine Stimme aus lauter gleichen Vierteln: ihre n-Gramme kämen über 20 000-mal vor.
        let drone: Vec<MelodyNote> = (0..20_010).map(|i| MelodyNote { onset: i as f64, duration: 1.0, midi: 60 }).collect();
        let before = idx.posting_count();
        idx.add_melody(DocInfo { title: "Bordun".into(), ..Default::default() }, &drone);
        idx.compact();
        // Alle drei Ebenen dieser Stimme kippen in die Stoppliste – fast nichts kommt hinzu.
        assert!(idx.posting_count() < before + 3 * MAX_POSTINGS_PER_GRAM, "{} → {}", before, idx.posting_count());
        let q = hum(&[(1.0, 69), (1.0, 67), (2.0, 64), (1.0, 65), (1.0, 67), (2.0, 69), (1.0, 67)], 0.5, 0);
        assert_eq!(idx.search(&q, 1)[0].info.title, "Bogen");
    }

    #[test]
    fn save_and_load_roundtrip() {
        let mut idx = library();
        idx.add_facets(ScoreFacets { doc: 2, title: "Sprünge".into(), genres: vec!["Klassik".into()], ..Default::default() });
        let dir = std::env::temp_dir().join(format!("klang-index-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.kwix");
        idx.save(&path).unwrap();
        let back = MelodyIndex::load(&path).unwrap();
        assert_eq!(back.doc_count(), idx.doc_count());
        assert_eq!(back.gram_count(), idx.gram_count());
        assert_eq!(back.facets(), idx.facets());
        let q = hum(&[(1.0, 60), (1.0, 67), (1.0, 64), (1.0, 72), (2.0, 69), (2.0, 65)], 1.0, 0);
        assert_eq!(back.search(&q, 1)[0].info.title, "Sprünge");
        let _ = std::fs::remove_dir_all(dir);
    }
}
