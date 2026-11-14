//! Akkordfolgen-Statistik einer Bibliothek – Teil des Index (Format KWIX04).
//!
//! Beim Indizieren wird in jeder Partitur die Akkordfolge erkannt (alle
//! Stimmen zusammen, Raster halber Takt) und relativ zur Tonart des Stücks
//! festgehalten: Abstand des Grundtons zur Tonika, Klangart, Umkehrung. Gezählt
//! wird, wie oft auf einen Akkord ein anderer folgt – getrennt nach Dur- und
//! Mollstücken –, dazu je Übergang ein paar Beispielstellen (Partitur, Takt).
//! Modulationen innerhalb eines Stücks zählen relativ zur Haupttonart.

use crate::analysis::score_key;
use crate::harmony::{detect_chords, Chord, Quality, TimedPitch};
use crate::score::Score;
use crate::theory::Key;
use serde::Serialize;
use std::collections::HashMap;
use std::io;

pub const MAX_EXAMPLES: usize = 5;

/// Statistik-Gruppen im Index: alle Stücke, Barock, Bach.
pub const GROUPS: [(&str, &str); 3] = [("alle", "Alle Stücke"), ("barock", "Barock"), ("bach", "Bach")];

const BAROQUE: &[&str] = &[
    "bach", "händel", "handel", "haendel", "telemann", "vivaldi", "corelli", "buxtehude", "pachelbel", "scarlatti", "rameau",
    "couperin", "purcell", "frescobaldi", "albinoni", "marcello", "tartini", "lully", "zelenka", "graupner", "fux", "froberger",
    "kuhnau", "krebs", "walther", "böhm", "sweelinck", "schütz", "schutz", "praetorius", "monteverdi", "biber", "geminiani",
    "locatelli", "quantz", "hasse", "pergolesi", "boyce", "muffat", "fasch", "heinichen", "stölzel", "marais", "clérambault",
    "clerambault", "daquin", "leclair", "torelli", "stradella", "caldara", "legrenzi", "charpentier", "campra", "pasquini", "zipoli",
    "bononcini", "platti", "porpora", "keiser", "mattheson", "pepusch", "greene", "croft", "scheidt", "schein", "kerll", "pachelbels",
];

/// Gruppen eines Stücks nach Komponist: [alle, Barock, Bach].
pub fn composer_groups(composer: &str) -> [bool; 3] {
    let lower = composer.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let bach = words.contains(&"bach");
    let baroque = bach || words.iter().any(|w| BAROQUE.contains(w));
    [true, baroque, bach]
}
/// So viele Übergänge braucht ein Akkord in genau dieser Form (mit Septime,
/// Umkehrung), sonst zählt der Grundakkord.
const EXACT_MIN: u32 = 40;
const SECTION_MAGIC: &[u8; 4] = b"KWH1";

/// Akkord relativ zur Tonart (Abstand zur Tonika × Klangart × Umkehrung, < 480).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rel(u16);

fn quality_index(q: Quality) -> u16 {
    Quality::ALL.iter().position(|&x| x == q).unwrap_or(0) as u16
}

impl Rel {
    pub fn of(ch: &Chord, key: &Key) -> Rel {
        let off = (ch.root + 12 - key.tonic % 12) % 12;
        Rel(off as u16 * 40 + quality_index(ch.quality) * 4 + ch.inversion().min(3) as u16)
    }

    pub fn chord(self, key: &Key) -> Chord {
        let off = (self.0 / 40) as u8;
        let q = Quality::ALL[((self.0 % 40) / 4) as usize];
        Chord::new((key.tonic + off) % 12, q).inverted((self.0 % 4) as usize)
    }

    /// Grundakkord: gleicher Grundton und Dreiklang, ohne Septime und Umkehrung.
    pub fn loose(self) -> Rel {
        let q = Quality::ALL[((self.0 % 40) / 4) as usize].triad();
        Rel(self.0 / 40 * 40 + quality_index(q) * 4)
    }

    fn from_code(c: u16) -> Option<Rel> {
        (c < 480 && ((c % 40) / 4) < Quality::ALL.len() as u16).then_some(Rel(c))
    }
}

/// Eine Fundstelle: Stimme (doc) der Partitur, Takt, Bereich in Vierteln.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub doc: u32,
    pub bar: u32,
    pub start: f32,
    pub end: f32,
}

#[derive(Clone, Debug, Default)]
struct Entry {
    count: u32,
    /// (Rang, Beispiel) – behalten werden die mit kleinstem Rang, je Partitur eines.
    examples: Vec<(u64, Example)>,
}

impl Entry {
    fn offer(&mut self, rank: u64, ex: Example) {
        if let Some(slot) = self.examples.iter_mut().find(|(_, e)| e.doc == ex.doc) {
            if rank < slot.0 {
                *slot = (rank, ex);
            }
            return;
        }
        if self.examples.len() < MAX_EXAMPLES {
            self.examples.push((rank, ex));
            return;
        }
        if let Some((i, worst)) = self.examples.iter().enumerate().max_by_key(|(_, (r, _))| *r).map(|(i, (r, _))| (i, *r)) {
            if rank < worst {
                self.examples[i] = (rank, ex);
            }
        }
    }

    fn sorted_examples(&self) -> Vec<Example> {
        let mut v = self.examples.clone();
        v.sort_by_key(|(r, _)| *r);
        v.into_iter().map(|(_, e)| e).collect()
    }
}

/// Reproduzierbarer Streuwert – verteilt die Beispiele über die ganze Bibliothek.
fn rank(doc: u32, bar: u32) -> u64 {
    let mut z = ((doc as u64) << 32 | bar as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Ein erkannter Akkord in einer Partitur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgItem {
    pub start: f64,
    pub end: f64,
    pub bar: u32,
    pub rel: Rel,
}

fn is_percussion(part_name: &str) -> bool {
    let n = part_name.to_lowercase();
    ["drum", "schlagzeug", "perc", "batterie", "drumset", "kit", "cymbal", "becken", "snare", "hi-hat"].iter().any(|w| n.contains(w))
}

/// Erkannte Akkordfolge einer Partitur.
#[derive(Clone, Debug)]
pub struct Progression {
    pub minor: bool,
    pub items: Vec<ProgItem>,
    /// Mit Titel und Komponist – solche Stücke werden als Beispiel bevorzugt.
    pub notable: bool,
}

/// Akkordfolge einer Partitur relativ zu ihrer Tonart; None ohne Tonart oder Akkorde.
/// Bringt die Partitur eine Analyse von Fachleuten mit ([`Score::harmony`]),
/// zählt diese statt der automatischen Erkennung.
pub fn score_progression(score: &Score) -> Option<Progression> {
    let key = score_key(score)?;
    let (num, den) = score.time_signature.unwrap_or((4, 4));
    let bar = (num.max(1) as f64 * 4.0 / den.max(1) as f64).clamp(0.5, 16.0);
    if !score.harmony.is_empty() {
        let mut items: Vec<ProgItem> = Vec::new();
        for h in &score.harmony {
            let rel = Rel::of(&h.chord, &key);
            let end = h.onset + h.duration.max(0.0);
            match items.last_mut() {
                // gleicher Akkord weiter (z. B. nach einer Phrasenmarke): verlängern
                Some(last) if last.rel == rel => last.end = last.end.max(end),
                _ => items.push(ProgItem { start: h.onset, end, bar: (h.onset / bar).floor().max(0.0) as u32 + 1, rel }),
            }
        }
        if items.len() >= 2 {
            return Some(Progression { minor: key.minor, items, notable: true });
        }
    }
    let notes: Vec<TimedPitch> = score
        .parts
        .iter()
        .filter(|p| !is_percussion(&p.name))
        .flat_map(|p| &p.voices)
        .flat_map(|v| &v.notes)
        .map(|n| TimedPitch { pitch: n.midi, start: n.onset, length: n.duration })
        .collect();
    let window = if num % 2 == 0 { bar / 2.0 } else { bar }.clamp(1.0, 4.0);
    let chords = detect_chords(&notes, window, Some(&key));
    if chords.len() < 2 {
        return None;
    }
    let items = chords
        .iter()
        .map(|d| ProgItem { start: d.start, end: d.start + d.length, bar: (d.start / bar).floor().max(0.0) as u32 + 1, rel: Rel::of(&d.chord, &key) })
        .collect();
    // Platzhalter aus Notensatzprogrammen zählen nicht als Angabe
    const PLACEHOLDER: [&str; 9] = ["title", "composer", "untitled", "score", "me", "unknown", "?", "-", "titel"];
    let named = |s: &Option<String>| s.as_deref().map(str::trim).is_some_and(|t| !t.is_empty() && !PLACEHOLDER.contains(&t.to_lowercase().as_str()));
    Some(Progression { minor: key.minor, items, notable: named(&score.title) && named(&score.composer) })
}

/// Wie oft folgt `next` auf `prev` – Anteil an allen Fortsetzungen von `prev`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Follow {
    pub share: f32,
    pub count: u32,
    pub total: u32,
    /// true: genau diese Formen (Septime, Umkehrung); false: Grundakkorde.
    pub exact: bool,
    pub examples: Vec<Example>,
}

type TransKey = (bool, Rel, Rel);

#[derive(Clone, Debug, Default)]
pub struct ChordStats {
    /// Partituren mit erkannter Akkordfolge: [Dur, Moll].
    pub scores: [u32; 2],
    trans: HashMap<TransKey, Entry>,
    totals: HashMap<(bool, Rel), u32>,
    loose: HashMap<TransKey, Entry>,
    loose_totals: HashMap<(bool, Rel), u32>,
}

impl ChordStats {
    /// Akkordfolge einer Partitur aufnehmen (doc = ihre erste Stimme im Index).
    pub fn add(&mut self, doc: u32, p: &Progression) {
        let items = &p.items;
        if items.len() < 2 {
            return;
        }
        let minor = p.minor;
        self.scores[minor as usize] += 1;
        for w in items.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b.start - a.end > 1e-6 || a.rel == b.rel {
                continue; // Lücke (Pause, einstimmig) oder kein Wechsel
            }
            let e = self.trans.entry((minor, a.rel, b.rel)).or_default();
            e.count += 1;
            // Stücke mit Titel und Komponist zuerst (oberstes Bit frei)
            let r = (rank(doc, a.bar) >> 1) | if p.notable { 0 } else { 1 << 63 };
            e.offer(r, Example { doc, bar: a.bar, start: a.start as f32, end: b.end as f32 });
        }
    }

    /// Summen und Grundakkord-Tabellen ableiten (nach dem Aufnehmen bzw. Laden).
    pub fn finalize(&mut self) {
        self.totals.clear();
        self.loose.clear();
        self.loose_totals.clear();
        for (&(m, a, b), e) in &self.trans {
            *self.totals.entry((m, a)).or_default() += e.count;
            *self.loose_totals.entry((m, a.loose())).or_default() += e.count;
            if a.loose() == b.loose() {
                continue; // nur Septime/Umkehrung gewechselt: kein neuer Grundakkord
            }
            let l = self.loose.entry((m, a.loose(), b.loose())).or_default();
            l.count += e.count;
            for &(r, ex) in &e.examples {
                l.offer(r, ex);
            }
        }
        // Grundakkord-Summen ohne Wechsel innerhalb desselben Grundakkords
        for (&(m, a, b), e) in &self.trans {
            if a.loose() == b.loose() {
                if let Some(t) = self.loose_totals.get_mut(&(m, a.loose())) {
                    *t -= e.count;
                }
            }
        }
    }

    /// Statistik eines anderen Index dazunehmen (beim Verschmelzen): Zählungen
    /// addieren, Beispiele so auswählen, als kämen sie aus beiden zusammen
    /// (Rang = Platz ÷ Häufigkeit – große Sammlungen behalten ihren Anteil).
    /// `doc` bildet die Stimmnummern ab (None = Stimme entfällt). Gedacht für
    /// eine leere Statistik, die nacheinander alle Quellen aufnimmt; danach
    /// [`finalize`](Self::finalize) aufrufen.
    pub fn absorb(&mut self, other: &ChordStats, doc: impl Fn(u32) -> Option<u32>) {
        self.scores[0] += other.scores[0];
        self.scores[1] += other.scores[1];
        for (k, e) in &other.trans {
            let mine = self.trans.entry(*k).or_default();
            mine.count += e.count;
            for (pos, ex) in e.sorted_examples().into_iter().enumerate() {
                let Some(d) = doc(ex.doc) else { continue };
                let rank = ((pos + 1) as f64 / e.count.max(1) as f64 * (1u64 << 52) as f64) as u64;
                mine.offer(rank, Example { doc: d, ..ex });
            }
        }
    }

    pub fn transitions(&self) -> usize {
        self.trans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trans.is_empty()
    }

    /// Häufigkeit von `next` nach `prev` (genaue Form, bei zu wenig Daten Grundakkorde).
    pub fn follow(&self, minor: bool, prev: Rel, next: Rel) -> Option<Follow> {
        let total = self.totals.get(&(minor, prev)).copied().unwrap_or(0);
        if total >= EXACT_MIN {
            let e = self.trans.get(&(minor, prev, next));
            let count = e.map(|e| e.count).unwrap_or(0);
            return Some(Follow { share: count as f32 / total as f32, count, total, exact: true, examples: e.map(Entry::sorted_examples).unwrap_or_default() });
        }
        let total = self.loose_totals.get(&(minor, prev.loose())).copied().unwrap_or(0);
        if total == 0 || prev.loose() == next.loose() {
            return None;
        }
        let e = self.loose.get(&(minor, prev.loose(), next.loose()));
        let count = e.map(|e| e.count).unwrap_or(0);
        Some(Follow { share: count as f32 / total as f32, count, total, exact: false, examples: e.map(Entry::sorted_examples).unwrap_or_default() })
    }

    /// Nur der Anteil von `next` an den Fortsetzungen nach `prev` (wie `follow`, ohne Beispiele).
    pub fn share(&self, minor: bool, prev: Rel, next: Rel) -> Option<f32> {
        let total = self.totals.get(&(minor, prev)).copied().unwrap_or(0);
        if total >= EXACT_MIN {
            let count = self.trans.get(&(minor, prev, next)).map(|e| e.count).unwrap_or(0);
            return Some(count as f32 / total as f32);
        }
        let total = self.loose_totals.get(&(minor, prev.loose())).copied().unwrap_or(0);
        if total == 0 || prev.loose() == next.loose() {
            return None;
        }
        let count = self.loose.get(&(minor, prev.loose(), next.loose())).map(|e| e.count).unwrap_or(0);
        Some(count as f32 / total as f32)
    }

    /// Häufigste Fortsetzungen nach `prev`, beste zuerst.
    pub fn top_after(&self, minor: bool, prev: Rel, limit: usize) -> Vec<(Rel, Follow)> {
        let exact_total = self.totals.get(&(minor, prev)).copied().unwrap_or(0);
        let (map, from, total, exact) = if exact_total >= EXACT_MIN {
            (&self.trans, prev, exact_total, true)
        } else {
            (&self.loose, prev.loose(), self.loose_totals.get(&(minor, prev.loose())).copied().unwrap_or(0), false)
        };
        if total == 0 {
            return Vec::new();
        }
        let mut v: Vec<(Rel, &Entry)> = map.iter().filter(|((m, a, _), _)| *m == minor && *a == from).map(|((_, _, b), e)| (*b, e)).collect();
        v.sort_by(|x, y| y.1.count.cmp(&x.1.count).then(x.0.cmp(&y.0)));
        v.truncate(limit);
        v.into_iter()
            .map(|(b, e)| (b, Follow { share: e.count as f32 / total as f32, count: e.count, total, exact, examples: e.sorted_examples() }))
            .collect()
    }

    // ------------------------------------------------------------ Datei

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.trans.len() * 40);
        out.extend_from_slice(SECTION_MAGIC);
        out.extend_from_slice(&self.scores[0].to_le_bytes());
        out.extend_from_slice(&self.scores[1].to_le_bytes());
        out.extend_from_slice(&(self.trans.len() as u32).to_le_bytes());
        let mut keys: Vec<&TransKey> = self.trans.keys().collect();
        keys.sort();
        for k in keys {
            let e = &self.trans[k];
            out.push(k.0 as u8);
            out.extend_from_slice(&k.1 .0.to_le_bytes());
            out.extend_from_slice(&k.2 .0.to_le_bytes());
            out.extend_from_slice(&e.count.to_le_bytes());
            let ex = e.sorted_examples();
            out.push(ex.len() as u8);
            for x in ex {
                out.extend_from_slice(&x.doc.to_le_bytes());
                out.extend_from_slice(&x.bar.to_le_bytes());
                out.extend_from_slice(&x.start.to_le_bytes());
                out.extend_from_slice(&x.end.to_le_bytes());
            }
        }
        out
    }

    /// Mehrere Gruppen hintereinander (Abschnitt „KWH2“).
    pub fn encode_groups(groups: &[ChordStats]) -> Vec<u8> {
        let mut out = b"KWH2".to_vec();
        out.push(groups.len() as u8);
        for g in groups {
            let e = g.encode();
            out.extend_from_slice(&(e.len() as u32).to_le_bytes());
            out.extend_from_slice(&e);
        }
        out
    }

    /// Gruppen lesen; ein älterer Abschnitt „KWH1“ ergibt nur die Gruppe „alle“.
    pub fn decode_groups(buf: &[u8]) -> io::Result<Vec<ChordStats>> {
        if buf.starts_with(SECTION_MAGIC) {
            return Ok(vec![Self::decode(buf)?]);
        }
        let mut c = Cursor { buf, i: 0 };
        if c.take(4)? != b"KWH2" {
            return Err(bad());
        }
        let n = c.u8()? as usize;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let len = c.u32()? as usize;
            out.push(Self::decode(c.take(len)?)?);
        }
        Ok(out)
    }

    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut c = Cursor { buf, i: 0 };
        if c.take(4)? != SECTION_MAGIC {
            return Err(bad());
        }
        let mut me = ChordStats { scores: [c.u32()?, c.u32()?], ..Default::default() };
        let n = c.u32()? as usize;
        me.trans.reserve(n.min(1 << 22));
        for _ in 0..n {
            let minor = c.u8()? != 0;
            let a = Rel::from_code(c.u16()?).ok_or_else(bad)?;
            let b = Rel::from_code(c.u16()?).ok_or_else(bad)?;
            let count = c.u32()?;
            let n_ex = c.u8()? as usize;
            let mut examples = Vec::with_capacity(n_ex);
            for r in 0..n_ex {
                let (doc, bar) = (c.u32()?, c.u32()?);
                let (start, end) = (f32::from_bits(c.u32()?), f32::from_bits(c.u32()?));
                // Reihenfolge der Datei bleibt erhalten (Rang = Position)
                examples.push((r as u64, Example { doc, bar, start, end }));
            }
            me.trans.insert((minor, a, b), Entry { count, examples });
        }
        me.finalize();
        Ok(me)
    }
}

fn bad() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Harmonik-Abschnitt beschädigt")
}

struct Cursor<'a> {
    buf: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let s = self.buf.get(self.i..self.i + n).ok_or_else(bad)?;
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harmony::parse_roman;
    use crate::score::load_score;
    use crate::score::tests::make_xml;

    fn key_c() -> Key {
        Key { tonic: 0, minor: false, confidence: 1.0 }
    }

    fn item(i: usize, s: &str) -> ProgItem {
        let k = key_c();
        ProgItem { start: i as f64 * 2.0, end: i as f64 * 2.0 + 2.0, bar: i as u32 / 2 + 1, rel: Rel::of(&parse_roman(s, &k).unwrap(), &k) }
    }

    #[test]
    fn rel_roundtrip_and_loose() {
        let k = Key { tonic: 3, minor: false, confidence: 1.0 };
        for s in ["I", "V7", "ii6", "I64", "vii°7", "V43"] {
            let c = parse_roman(s, &k).unwrap();
            assert_eq!(Rel::of(&c, &k).chord(&k), c, "{s}");
        }
        let k = key_c();
        let v7 = Rel::of(&parse_roman("V65", &k).unwrap(), &k);
        assert_eq!(v7.loose(), Rel::of(&parse_roman("V", &k).unwrap(), &k));
    }

    #[test]
    fn counts_shares_examples_and_survives_encoding() {
        let mut st = ChordStats::default();
        // 50 Stücke: V7 → I, 30 davon zusätzlich V7 → vi
        for doc in 0..50u32 {
            let mut prog = vec![item(0, "I"), item(1, "IV"), item(2, "V7"), item(3, "I")];
            if doc < 30 {
                prog.extend([item(4, "V7"), item(5, "vi")]);
            }
            st.add(doc * 3, &Progression { minor: false, items: prog, notable: doc % 2 == 0 });
        }
        st.finalize();
        let k = key_c();
        let r = |s: &str| Rel::of(&parse_roman(s, &k).unwrap(), &k);
        let f = st.follow(false, r("V7"), r("I")).unwrap();
        assert!(f.exact);
        assert_eq!((f.count, f.total), (50, 80));
        assert_eq!(f.examples.len(), MAX_EXAMPLES);
        let docs: std::collections::HashSet<u32> = f.examples.iter().map(|e| e.doc).collect();
        assert_eq!(docs.len(), MAX_EXAMPLES, "Beispiele aus verschiedenen Stücken");
        assert_eq!(st.top_after(false, r("V7"), 3)[0].0, r("I"));
        // seltene Form (V) → Grundakkord-Statistik
        let f = st.follow(false, r("V"), r("vi")).unwrap();
        assert!(!f.exact);
        assert_eq!((f.count, f.total), (30, 80));
        assert_eq!(st.scores, [50, 0]);

        let back = ChordStats::decode(&st.encode()).unwrap();
        assert_eq!(back.follow(false, r("V7"), r("I")).map(|f| (f.count, f.total, f.examples.len())), Some((50, 80, MAX_EXAMPLES)));
        assert_eq!(back.scores, st.scores);
        assert!(ChordStats::decode(&st.encode()[..20]).is_err());
        // Gruppen: neues und altes Format
        let groups = ChordStats::decode_groups(&ChordStats::encode_groups(&[st.clone(), ChordStats::default(), st.clone()])).unwrap();
        assert_eq!(groups.len(), 3);
        assert!(groups[1].is_empty() && groups[2].follow(false, r("V7"), r("I")).is_some());
        assert_eq!(ChordStats::decode_groups(&st.encode()).unwrap().len(), 1);
    }

    #[test]
    fn absorbing_two_statistics_counts_like_one() {
        let k = key_c();
        let r = |s: &str| Rel::of(&parse_roman(s, &k).unwrap(), &k);
        let prog = |vi: bool| {
            let mut p = vec![item(0, "I"), item(1, "V7"), item(2, "I")];
            if vi {
                p.extend([item(3, "V7"), item(4, "vi")]);
            }
            Progression { minor: false, items: p, notable: true }
        };
        let (mut a, mut b, mut all) = (ChordStats::default(), ChordStats::default(), ChordStats::default());
        for doc in 0..40u32 {
            a.add(doc, &prog(doc < 10));
            all.add(doc, &prog(doc < 10));
        }
        for doc in 0..8u32 {
            b.add(doc, &prog(true));
            all.add(1000 + doc, &prog(true));
        }
        // wie aus der Datei gelesen
        let (a, b) = (ChordStats::decode(&a.encode()).unwrap(), ChordStats::decode(&b.encode()).unwrap());
        let mut merged = ChordStats::default();
        merged.absorb(&a, Some);
        merged.absorb(&b, |d| if d == 3 { None } else { Some(1000 + d) });
        merged.finalize();
        all.finalize();
        assert_eq!(merged.scores, all.scores);
        for (p, n) in [("V7", "I"), ("V7", "vi"), ("I", "V7")] {
            let (m, x) = (merged.follow(false, r(p), r(n)).unwrap(), all.follow(false, r(p), r(n)).unwrap());
            assert_eq!((m.count, m.total), (x.count, x.total), "{p} → {n}");
        }
        let ex = merged.follow(false, r("V7"), r("vi")).unwrap().examples;
        assert_eq!(ex.len(), MAX_EXAMPLES);
        assert!(ex.iter().all(|e| e.doc < 40 || (e.doc >= 1000 && e.doc != 1003)), "umnummeriert, entfallene fehlen: {ex:?}");
        assert!(ex.iter().any(|e| e.doc >= 1000), "die kleine Quelle kommt auch vor (8 von 18 Stücken)");
    }

    #[test]
    fn composer_groups_find_bach_and_baroque() {
        assert_eq!(composer_groups("Johann Sebastian Bach"), [true, true, true]);
        assert_eq!(composer_groups("J.S.Bach"), [true, true, true]);
        assert_eq!(composer_groups("Jacques Offenbach"), [true, false, false]);
        assert_eq!(composer_groups("Georg Friedrich Händel"), [true, true, false]);
        assert_eq!(composer_groups("Antonio Vivaldi (1678-1741)"), [true, true, false]);
        assert_eq!(composer_groups("Wolfgang Amadeus Mozart"), [true, false, false]);
        assert_eq!(composer_groups(""), [true, false, false]);
    }

    #[test]
    fn progression_of_a_chorale() {
        // vierstimmig in Halben: C – F – G7 – C
        let s: &[(u8, u32)] = &[(72, 4), (72, 4), (74, 4), (72, 4)];
        let a: &[(u8, u32)] = &[(67, 4), (69, 4), (71, 4), (67, 4)];
        let t: &[(u8, u32)] = &[(64, 4), (65, 4), (65, 4), (64, 4)];
        let b: &[(u8, u32)] = &[(48, 4), (41, 4), (43, 4), (48, 4)];
        let dir = std::env::temp_dir().join(format!("klang-choral-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("choral.musicxml");
        std::fs::write(&path, make_xml("Choral", &[("Sopran", s), ("Alt", a), ("Tenor", t), ("Bass", b)])).unwrap();
        let score = load_score(&path).unwrap();
        let p = score_progression(&score).unwrap();
        assert!(!p.minor);
        assert!(!p.notable, "ohne Komponist");
        let k = key_c();
        let names: Vec<String> = p.items.iter().map(|i| crate::harmony::chord_name(&i.rel.chord(&k), &k)).collect();
        assert_eq!(names, ["C", "F", "G7", "C"]);
        assert_eq!(p.items[2].bar, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
