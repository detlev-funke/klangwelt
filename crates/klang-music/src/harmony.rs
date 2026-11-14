//! Harmonik für Quintenzirkel und Agent: Akkorde benennen (deutsche Tonnamen,
//! Stufen, Funktionstheorie), Fortsetzungen vorschlagen, Kadenzvorlagen,
//! Akkorde in Noten erkennen und vierstimmig setzen (Stimmführung wie im
//! Choralsatz: kurze Wege, keine Quint- und Oktavparallelen, Leitton und
//! Septime lösen sich auf).

use crate::theory::Key;
use klang_i18n::{tk, tr};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ Akkorde

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    Major,
    Minor,
    Dim,
    Aug,
    Dom7,
    Maj7,
    Min7,
    HalfDim7,
    Dim7,
    Sus4,
}

use Quality::*;

impl Quality {
    pub const ALL: [Quality; 10] = [Major, Minor, Dim, Aug, Dom7, Maj7, Min7, HalfDim7, Dim7, Sus4];

    /// Intervalle über dem Grundton in Halbtönen (Grundton, Terz, Quinte, Septime).
    pub fn intervals(self) -> &'static [u8] {
        match self {
            Major => &[0, 4, 7],
            Minor => &[0, 3, 7],
            Dim => &[0, 3, 6],
            Aug => &[0, 4, 8],
            Dom7 => &[0, 4, 7, 10],
            Maj7 => &[0, 4, 7, 11],
            Min7 => &[0, 3, 7, 10],
            HalfDim7 => &[0, 3, 6, 10],
            Dim7 => &[0, 3, 6, 9],
            Sus4 => &[0, 5, 7],
        }
    }

    pub fn is_seventh(self) -> bool {
        matches!(self, Dom7 | Maj7 | Min7 | HalfDim7 | Dim7)
    }

    /// Kleine Terz über dem Grundton – Name klein geschrieben, Stufe klein.
    pub fn minor_third(self) -> bool {
        matches!(self, Minor | Dim | Min7 | HalfDim7 | Dim7)
    }

    /// Der Dreiklang ohne Septime.
    pub fn triad(self) -> Quality {
        match self {
            Dom7 | Maj7 => Major,
            Min7 => Minor,
            HalfDim7 | Dim7 => Dim,
            q => q,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Major | Minor => "",
            Dim => "°",
            Aug => "+",
            Dom7 | Min7 => "7",
            Maj7 => "maj7",
            HalfDim7 => "ø7",
            Dim7 => "°7",
            Sus4 => "sus4",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chord {
    /// Grundton als Tonhöhenklasse (0 = C).
    pub root: u8,
    pub quality: Quality,
    /// Basston (Tonhöhenklasse), wenn nicht der Grundton – also eine Umkehrung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bass: Option<u8>,
}

impl Chord {
    pub fn new(root: u8, quality: Quality) -> Self {
        Self { root: root % 12, quality, bass: None }
    }

    /// Tonhöhenklassen der Akkordtöne (Grundton zuerst).
    pub fn pcs(&self) -> Vec<u8> {
        self.quality.intervals().iter().map(|i| (self.root + i) % 12).collect()
    }

    pub fn bass_pc(&self) -> u8 {
        self.bass.unwrap_or(self.root)
    }

    /// 0 = Grundstellung, 1 = Terz im Bass, 2 = Quinte, 3 = Septime.
    pub fn inversion(&self) -> usize {
        let b = (self.bass_pc() + 12 - self.root) % 12;
        self.quality.intervals().iter().position(|&i| i == b).unwrap_or(0)
    }

    /// Derselbe Akkord mit dem n-ten Akkordton im Bass (0 = Grundton).
    pub fn inverted(mut self, n: usize) -> Self {
        let iv = self.quality.intervals();
        self.bass = if n == 0 || n >= iv.len() { None } else { Some((self.root + iv[n]) % 12) };
        self
    }

    /// Bass auf einen Akkordton setzen; ein fremder Ton bleibt unbeachtet.
    fn normalized(mut self) -> Self {
        self.root %= 12;
        if let Some(b) = self.bass {
            if b % 12 == self.root || !self.pcs().contains(&(b % 12)) {
                self.bass = None;
            } else {
                self.bass = Some(b % 12);
            }
        }
        self
    }

    /// Gleicher Platz im Quintenzirkel (Grundton und Dreiklang gleich).
    pub fn same_harmony(&self, o: &Chord) -> bool {
        self.root == o.root && self.quality.triad() == o.quality.triad()
    }
}

// ------------------------------------------------------------------ Schreibweise

const LETTERS: [&str; 7] = ["C", "D", "E", "F", "G", "A", "H"];
const NATURAL: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const SHARP_NAMES: [&str; 12] = ["C", "Cis", "D", "Dis", "E", "F", "Fis", "G", "Gis", "A", "B", "H"];
const MAJOR_STEPS: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR_STEPS: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];
/// Harmonisches und natürliches Moll zusammen (wie in `theory`).
const MINOR_SET: [u8; 8] = [0, 2, 3, 5, 7, 8, 10, 11];

/// Deutscher Tonname einer Tonhöhenklasse auf einer Stammtonstufe (0 = C … 6 = H).
fn spell(pc: u8, letter: usize) -> String {
    let letter = letter % 7;
    let alt = (pc as i32 - NATURAL[letter] as i32 + 18).rem_euclid(12) - 6;
    let base = LETTERS[letter];
    match alt {
        0 => base.to_string(),
        1 => format!("{base}is"),
        2 => format!("{base}isis"),
        -1 => match letter {
            2 => "Es".into(),
            5 => "As".into(),
            6 => "B".into(),
            _ => format!("{base}es"),
        },
        -2 => match letter {
            2 => "Eses".into(),
            5 => "Ases".into(),
            6 => "Heses".into(),
            _ => format!("{base}eses"),
        },
        _ => SHARP_NAMES[pc as usize % 12].to_string(),
    }
}

/// Stammtonstufe des Grundtons einer Tonart (wie in `Key::name_de`).
fn tonic_letter(key: &Key) -> usize {
    const MAJ: [usize; 12] = [0, 1, 1, 2, 2, 3, 3, 4, 5, 5, 6, 6];
    const MIN: [usize; 12] = [0, 0, 1, 2, 2, 3, 3, 4, 4, 5, 6, 6];
    if key.minor { MIN[key.tonic as usize % 12] } else { MAJ[key.tonic as usize % 12] }
}

/// Halbtöne über dem Grundton → (Stufe 0…6, Alteration gegenüber der Tonleiter).
fn degree_of(offset: u8, minor: bool) -> (usize, i8) {
    const MAJ: [(usize, i8); 12] = [(0, 0), (1, -1), (1, 0), (2, -1), (2, 0), (3, 0), (3, 1), (4, 0), (5, -1), (5, 0), (6, -1), (6, 0)];
    const MIN: [(usize, i8); 12] = [(0, 0), (1, -1), (1, 0), (2, 0), (2, 1), (3, 0), (3, 1), (4, 0), (5, 0), (5, 1), (6, 0), (6, 1)];
    if minor { MIN[offset as usize % 12] } else { MAJ[offset as usize % 12] }
}

fn offset(pc: u8, key: &Key) -> u8 {
    (pc + 12 - key.tonic % 12) % 12
}

fn key_with(tonic: u8, minor: bool) -> Key {
    Key { tonic: tonic % 12, minor, confidence: 1.0 }
}

/// Akkordsymbol mit deutschen Tonnamen: Dur groß („G7“, „Es“), Moll und
/// vermindert klein („a“, „fis7“, „h°“), Umkehrung mit Basston („C/E“).
pub fn chord_name(ch: &Chord, key: &Key) -> String {
    let (deg, _) = degree_of(offset(ch.root, key), key.minor);
    let letter = tonic_letter(key) + deg;
    let mut root = spell(ch.root, letter);
    if ch.quality.minor_third() {
        root = root.to_lowercase();
    }
    let mut s = format!("{root}{}", ch.quality.suffix());
    if ch.bass.is_some() {
        let inv = ch.inversion();
        let steps = [0, 2, 4, 6][inv.min(3)];
        s.push('/');
        s.push_str(&spell(ch.bass_pc(), letter + steps));
    }
    s
}

fn in_key(pc: u8, key: &Key) -> bool {
    let o = offset(pc, key);
    if key.minor { MINOR_SET.contains(&o) } else { MAJOR_STEPS.contains(&o) }
}

pub fn is_diatonic(ch: &Chord, key: &Key) -> bool {
    ch.pcs().iter().all(|&p| in_key(p, key))
}

/// Leitereigener Dreiklang auf diesem Grundton (Moll: V und vii° mit Leitton).
fn diatonic_triad(off: u8, key: &Key) -> Option<Quality> {
    if key.minor {
        match off {
            0 => Some(Minor),
            2 => Some(Dim),
            3 => Some(Major),
            5 => Some(Minor),
            7 => Some(Major),
            8 => Some(Major),
            10 => Some(Major),
            11 => Some(Dim),
            _ => None,
        }
    } else {
        match off {
            0 | 5 | 7 => Some(Major),
            2 | 4 | 9 => Some(Minor),
            11 => Some(Dim),
            _ => None,
        }
    }
}

/// Zwischendominante: Dur- oder Dominantseptakkord, der eine Quinte über
/// einem leitereigenen (nicht verminderten) Akkord steht. Liefert dessen Abstand zur Tonika.
fn secondary_target(ch: &Chord, key: &Key) -> Option<u8> {
    if !matches!(ch.quality, Major | Dom7) {
        return None;
    }
    let off = offset(ch.root, key);
    if off == 7 {
        return None;
    }
    if key.minor && off == 0 && ch.quality == Major {
        return None; // Picardische Terz
    }
    let diatonic = is_diatonic(ch, key);
    if diatonic && !(key.minor && off == 10 && ch.quality == Dom7) {
        return None;
    }
    let target = (off + 5) % 12;
    match diatonic_triad(target, key) {
        Some(q) if q != Dim => Some(target),
        _ => None,
    }
}

const SUP: [(char, char); 6] = [('2', '²'), ('3', '³'), ('4', '⁴'), ('5', '⁵'), ('6', '⁶'), ('7', '⁷')];
const SUB: [(char, char); 6] = [('2', '₂'), ('3', '₃'), ('4', '₄'), ('5', '₅'), ('6', '₆'), ('7', '₇')];

fn sup(s: &str) -> String {
    s.chars().map(|c| SUP.iter().find(|p| p.0 == c).map(|p| p.1).unwrap_or(c)).collect()
}
fn sub(s: &str) -> String {
    s.chars().map(|c| SUB.iter().find(|p| p.0 == c).map(|p| p.1).unwrap_or(c)).collect()
}

/// Generalbass-Bezifferung der Umkehrung („⁶“, „⁶₄“, „⁶₅“ …).
fn figures(ch: &Chord) -> String {
    let inv = ch.inversion();
    if ch.quality.is_seventh() {
        match inv {
            0 => sup("7"),
            1 => format!("{}{}", sup("6"), sub("5")),
            2 => format!("{}{}", sup("4"), sub("3")),
            _ => sup("2"),
        }
    } else {
        match inv {
            0 => String::new(),
            1 => sup("6"),
            _ => format!("{}{}", sup("6"), sub("4")),
        }
    }
}

const NUMERALS: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];

/// Stufe als römische Zahl relativ zur Tonart (ohne Zwischendominanten).
fn plain_roman(ch: &Chord, key: &Key) -> String {
    let off = offset(ch.root, key);
    let (deg, alt) = degree_of(off, key.minor);
    let mut num = NUMERALS[deg].to_string();
    if ch.quality.minor_third() {
        num = num.to_lowercase();
    }
    let prefix = if key.minor && off == 11 {
        ""
    } else {
        match alt {
            -1 => "♭",
            1 => "♯",
            _ => "",
        }
    };
    let mark = match ch.quality {
        Dim | Dim7 => "°",
        HalfDim7 => "ø",
        Aug => "+",
        Maj7 => "maj",
        _ => "",
    };
    let tail = if ch.quality == Sus4 { "sus4" } else { "" };
    format!("{prefix}{num}{mark}{}{tail}", figures(ch))
}

/// Function (Riemann) and role in words of a diatonic or
/// borrowed triad; None = no common function. The role is a
/// key, translated in [`describe`].
fn function_of(off: u8, triad: Quality, key: &Key) -> Option<(&'static str, &'static str)> {
    if key.minor {
        Some(match (off, triad) {
            (0, Minor) => ("t", tk!("Tonic")),
            (0, Major) => ("T", tk!("Picardy third (major tonic)")),
            (2, Dim) => ("s⁶", tk!("Subdominant with added sixth")),
            (3, Major) => ("tP", tk!("Tonic relative")),
            (5, Minor) => ("s", tk!("Subdominant")),
            (5, Major) => ("S", tk!("Major subdominant (Dorian)")),
            (7, Minor) => ("d", tk!("Minor dominant (no leading tone)")),
            (7, Major) => ("D", tk!("Dominant")),
            (8, Major) => ("tG", tk!("Tonic counter-relative")),
            (10, Major) => ("dP", tk!("Dominant relative")),
            (11, Dim) => ("Đ", tk!("Incomplete dominant")),
            (1, Major) => ("sN", tk!("Neapolitan")),
            _ => return None,
        })
    } else {
        Some(match (off, triad) {
            (0, Major) => ("T", tk!("Tonic")),
            (2, Minor) => ("Sp", tk!("Subdominant relative")),
            (4, Minor) => ("Dp", tk!("Dominant relative")),
            (5, Major) => ("S", tk!("Subdominant")),
            (7, Major) => ("D", tk!("Dominant")),
            (9, Minor) => ("Tp", tk!("Tonic relative")),
            (11, Dim) => ("Đ", tk!("Incomplete dominant")),
            (5, Minor) => ("s", tk!("Minor subdominant (borrowed from minor)")),
            (0, Minor) => ("t", tk!("Minor tonic (borrowed from minor)")),
            (7, Minor) => ("d", tk!("Minor dominant (borrowed from minor)")),
            (3, Major) => ("tP", tk!("borrowed from minor")),
            (8, Major) => ("sP", tk!("borrowed from minor")),
            (10, Major) => ("dP", tk!("borrowed from minor")),
            (1, Major) => ("sN", tk!("Neapolitan")),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChordInfo {
    pub chord: Chord,
    /// Akkordsymbol, z. B. „G7“, „fis°“, „C/E“.
    pub name: String,
    /// Stufe, z. B. „V⁷“, „ii⁶“, „V⁷/vi“, „N⁶“.
    pub roman: String,
    /// Funktion nach Riemann, z. B. „D⁷“, „Sp“, „(D⁷) Tp“, „DD“.
    pub function: String,
    /// Role in words, e.g. "Dominant", "Secondary dominant of a" (translated).
    pub role: String,
    pub diatonic: bool,
}

/// Akkord in einer Tonart beschreiben: Name, Stufe, Funktion, Rolle.
pub fn describe(ch: &Chord, key: &Key) -> ChordInfo {
    let ch = ch.normalized();
    let off = offset(ch.root, key);
    let name = chord_name(&ch, key);
    let diatonic = is_diatonic(&ch, key);
    let triad = ch.quality.triad();
    let seventh = if ch.quality.is_seventh() { "⁷" } else { "" };
    let bass_mark = match ch.inversion() {
        0 => String::new(),
        n => sub(&["", "3", "5", "7"][n.min(3)].to_string()),
    };

    let tonic_triad = if key.minor { Minor } else { Major };
    let (roman, function, role) = if let Some(target) = secondary_target(&ch, key) {
        let tq = diatonic_triad(target, key).unwrap_or(Major);
        let target_chord = Chord::new((key.tonic + target) % 12, tq);
        let mut target_roman = plain_roman(&target_chord, key);
        if key.minor && target == 7 {
            target_roman = "V".into();
        }
        let v = format!("V{}", figures(&ch));
        let tf = function_of(target, tq, key).map(|f| f.0).unwrap_or("?");
        let func = if target == 7 { format!("DD{seventh}") } else { format!("(D{seventh}) {tf}") };
        (format!("{v}/{target_roman}"), func, tr!("Secondary dominant of {chord}", chord = crate::theory::display_name(&chord_name(&target_chord, key))))
    } else if off == 1 && triad == Major {
        let fig = if ch.inversion() == 1 { sup("6") } else { String::new() };
        (format!("N{fig}"), "sN".to_string(), tr!("Neapolitan"))
    } else if off == 0 && ch.quality == tonic_triad && ch.inversion() == 2 {
        (plain_roman(&ch, key), format!("D{}{}", sup("6"), sub("4")), tr!("Cadential six-four before the dominant"))
    } else if let Some((f, r)) = function_of(off, triad, key) {
        let special = matches!(f, "s⁶" | "Đ" | "sN");
        let func = match (f, ch.quality) {
            ("s⁶", HalfDim7) => format!("s{}{}", sup("6"), sub("5")),
            ("Đ", Dim7) => format!("Đ{}", sup("9")),
            ("Đ", HalfDim7) => format!("Đ{}", sup("9")),
            ("Đ", _) => format!("Đ{}", sup("7")),
            _ if special => f.to_string(),
            _ => format!("{f}{seventh}{bass_mark}"),
        };
        (plain_roman(&ch, key), func, klang_i18n::translate(r))
    } else {
        (plain_roman(&ch, key), "–".to_string(), if diatonic { tr!("diatonic") } else { tr!("non-diatonic") })
    };
    ChordInfo { chord: ch, name, roman, function, role, diatonic }
}

// ------------------------------------------------------------------ Einlesen

fn normalize_digits(s: &str) -> String {
    s.chars()
        .map(|c| {
            SUP.iter().chain(SUB.iter()).find(|p| p.1 == c).map(|p| p.0).unwrap_or(c)
        })
        .collect()
}

/// Stufe einlesen: „I“, „ii6“, „V7“, „vii°7“, „iiø65“, „♭VI“, „V7/V“, „N6“, „I64“.
pub fn parse_roman(input: &str, key: &Key) -> Option<Chord> {
    let s = normalize_digits(input.trim());
    if let Some((head, target)) = s.split_once('/') {
        let t = parse_roman(target, key)?;
        // Ziel als eigene Tonart (Dur oder Moll nach dem Zielakkord).
        let tkey = key_with(t.root, t.quality.minor_third());
        let mut ch = parse_roman(head, &tkey)?;
        // „V/…“ im Moll-Ziel bleibt Dur (Dominante mit Leitton).
        if tkey.minor && head.trim_start().starts_with('V') && ch.quality == Minor {
            ch.quality = Major;
        }
        return Some(ch.normalized());
    }
    let mut rest = s.as_str();
    if rest.eq_ignore_ascii_case("N") || rest.eq_ignore_ascii_case("N6") {
        return Some(Chord::new((key.tonic + 1) % 12, Major).inverted(1));
    }
    if rest.eq_ignore_ascii_case("K64") {
        let q = if key.minor { Minor } else { Major };
        return Some(Chord::new(key.tonic, q).inverted(2));
    }
    let mut alt: i32 = 0;
    for (p, a) in [("♭", -1), ("b", -1), ("♯", 1), ("#", 1)] {
        if let Some(r) = rest.strip_prefix(p) {
            alt = a;
            rest = r;
            break;
        }
    }
    let upper_rest = rest.to_ascii_uppercase();
    let numeral = ["VII", "VI", "V", "IV", "III", "II", "I"].into_iter().find(|n| upper_rest.starts_with(n))?;
    let raw = &rest[..numeral.len()];
    let upper = raw.chars().all(|c| c.is_ascii_uppercase());
    let lower = raw.chars().all(|c| c.is_ascii_lowercase());
    if !upper && !lower {
        return None;
    }
    let deg = NUMERALS.iter().position(|n| *n == numeral)?;
    let mut tail = &rest[numeral.len()..];

    let mut quality = if upper { Major } else { Minor };
    let mut dim = false;
    let mut half = false;
    let mut maj = false;
    for (p, f) in [("°", 0), ("o", 0), ("ø", 1), ("+", 2), ("maj", 3)] {
        if let Some(r) = tail.strip_prefix(p) {
            match f {
                0 => dim = true,
                1 => half = true,
                2 => quality = Aug,
                _ => maj = true,
            }
            tail = r;
            break;
        }
    }
    let fig = tail.trim();
    let (seventh, inv) = match fig {
        "" => (false, 0),
        "6" => (false, 1),
        "64" => (false, 2),
        "7" => (true, 0),
        "65" => (true, 1),
        "43" => (true, 2),
        "42" | "2" => (true, 3),
        _ => return None,
    };
    if dim {
        quality = if seventh { Dim7 } else { Dim };
    } else if half {
        quality = HalfDim7;
    } else if seventh {
        quality = match (quality, maj) {
            (_, true) => Maj7,
            (Major, _) => Dom7,
            (Minor, _) => Min7,
            (q, _) => q,
        };
    }
    if quality == HalfDim7 && !seventh && inv > 0 {
        return None;
    }
    let steps = if key.minor { MINOR_STEPS } else { MAJOR_STEPS };
    let mut off = steps[deg] as i32;
    // In Moll: „V“ hat dieselbe Stufe; „vii°“ steht auf dem Leitton.
    if key.minor && deg == 6 && quality.minor_third() {
        off = 11;
    }
    let root = (key.tonic as i32 + off + alt).rem_euclid(12) as u8;
    let ch = Chord::new(root, quality);
    let inv = if quality == HalfDim7 && fig.is_empty() { 0 } else { inv };
    Some(ch.inverted(inv))
}

/// Tonname am Anfang einlesen (deutsch: H = h, B = b, Es, Fis …; auch C#, Bb).
/// Liefert (Tonhöhenklasse, Kleinbuchstabe?, verbrauchte Zeichen).
fn parse_note_name(s: &str) -> Option<(u8, bool, usize)> {
    let mut chars = s.char_indices();
    let (_, c) = chars.next()?;
    let lower = c.is_ascii_lowercase();
    let base: i32 = match c.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 10,
        'H' => 11,
        _ => return None,
    };
    let letter = c.to_ascii_uppercase();
    let mut pc = base;
    let mut i = c.len_utf8();
    let rest = &s[i..];
    // deutsche Endungen
    let mut r = rest;
    loop {
        if let Some(x) = r.strip_prefix("is") {
            pc += 1;
            r = x;
        } else if let Some(x) = r.strip_prefix("es") {
            pc -= 1;
            r = x;
        } else if (letter == 'E' || letter == 'A') && r.starts_with('s') && !r.starts_with("sus") {
            pc -= 1;
            r = &r[1..];
        } else if let Some(x) = r.strip_prefix('#').or_else(|| r.strip_prefix('♯')) {
            pc += 1;
            r = x;
        } else if let Some(x) = r.strip_prefix('♭') {
            if letter != 'B' {
                pc -= 1;
            }
            r = x;
        } else if r.starts_with('b') && !r.starts_with("b5") {
            // „Bb“ = B (deutsch), „Eb“ = Es
            if letter != 'B' {
                pc -= 1;
            }
            r = &r[1..];
        } else {
            break;
        }
    }
    i += rest.len() - r.len();
    Some((pc.rem_euclid(12) as u8, lower, i))
}

/// Akkordsymbol einlesen: „C“, „a“ (a-Moll), „Am“, „G7“, „fis°“, „Hø7“,
/// „Cmaj7“, „Dsus4“, „C/E“, „Es-Dur“, „cis-Moll“.
pub fn parse_symbol(input: &str) -> Option<Chord> {
    let s = input.trim();
    let (main, bass) = match s.split_once('/') {
        Some((m, b)) => (m.trim(), Some(b.trim())),
        None => (s, None),
    };
    let (root, lower, used) = parse_note_name(main)?;
    let mut t = main[used..].trim_start_matches(['-', ' ']).to_lowercase();
    let mut quality = if lower { Minor } else { Major };
    if let Some(x) = t.strip_prefix("moll") {
        quality = Minor;
        t = x.to_string();
    } else if let Some(x) = t.strip_prefix("dur") {
        quality = Major;
        t = x.to_string();
    }
    let t = t.trim();
    quality = match t {
        "" => quality,
        "m" | "min" | "mi" => Minor,
        "7" => match quality {
            Minor => Min7,
            _ => Dom7,
        },
        "m7" | "min7" | "-7" => Min7,
        "maj7" | "j7" | "δ" | "δ7" | "Δ" | "Δ7" | "maj" => Maj7,
        "°" | "dim" | "o" => Dim,
        "°7" | "dim7" | "o7" => Dim7,
        "ø" | "ø7" | "m7b5" | "m7♭5" => HalfDim7,
        "+" | "aug" => Aug,
        "sus4" | "sus" => Sus4,
        _ => return None,
    };
    let mut ch = Chord::new(root, quality);
    if let Some(b) = bass {
        let (bpc, _, used) = parse_note_name(b)?;
        if used != b.len() || !ch.pcs().contains(&bpc) {
            return None;
        }
        if bpc != root {
            ch.bass = Some(bpc);
        }
    }
    Some(ch)
}

/// Akkord als Stufe („V7“, „ii6“, „V/V“) oder als Symbol („G7“, „a“) einlesen.
pub fn parse_chord(input: &str, key: &Key) -> Option<Chord> {
    let s = input.trim();
    let after_acc = s.trim_start_matches(['♭', '♯', '#']).trim_start_matches('b');
    let looks_roman = after_acc.starts_with(['I', 'i', 'V', 'v']) || s.eq_ignore_ascii_case("N") || s.eq_ignore_ascii_case("N6") || s.eq_ignore_ascii_case("K64");
    if looks_roman {
        if let Some(c) = parse_roman(s, key) {
            return Some(c);
        }
    }
    parse_symbol(s)
}

/// Tonart einlesen: „C-Dur“, „a-Moll“, „Es“, „fis“, „Am“, „F#m“.
pub fn parse_key(input: &str) -> Option<Key> {
    let s = input.trim();
    let lower = s.to_lowercase();
    let (s, force) = if let Some(x) = lower.strip_suffix(" major") {
        (&s[..x.len()], Some(false))
    } else if let Some(x) = lower.strip_suffix(" minor") {
        (&s[..x.len()], Some(true))
    } else {
        (s, None)
    };
    let ch = parse_symbol(s)?;
    let minor = force.unwrap_or(ch.quality.minor_third());
    if !matches!(ch.quality, Major | Minor) || ch.bass.is_some() {
        return None;
    }
    Some(key_with(ch.root, minor))
}

// ------------------------------------------------------------------ Quintenzirkel

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CircleCell {
    /// 0 = Dur (außen), 1 = Moll (Mitte), 2 = vermindert (innen).
    pub ring: u8,
    /// Platz im Uhrzeigersinn ab oben (0 = C/a/h°, 1 = G/e/fis° …).
    pub pos: u8,
    #[serde(flatten)]
    pub info: ChordInfo,
}

/// Die 36 Felder des Quintenzirkels, benannt und gedeutet in der Tonart.
pub fn circle(key: &Key) -> Vec<CircleCell> {
    let mut out = Vec::with_capacity(36);
    for ring in 0..3u8 {
        for pos in 0..12u8 {
            let major = (pos as u32 * 7 % 12) as u8;
            let ch = match ring {
                0 => Chord::new(major, Major),
                1 => Chord::new((major + 9) % 12, Minor),
                _ => Chord::new((major + 11) % 12, Dim),
            };
            out.push(CircleCell { ring, pos, info: describe(&ch, key) });
        }
    }
    out
}

/// Platz eines Akkords im Zirkel (Ring, Position) – Septakkorde beim Dreiklang.
pub fn circle_pos(ch: &Chord) -> Option<(u8, u8)> {
    let pos_of = |major: u8| ((major as u32 * 7) % 12) as u8;
    match ch.quality.triad() {
        Major => Some((0, pos_of(ch.root))),
        Minor => Some((1, pos_of((ch.root + 3) % 12))),
        Dim => Some((2, pos_of((ch.root + 1) % 12))),
        _ => None,
    }
}

// ------------------------------------------------------------------ Fortsetzungen

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    /// Schließt ab (Ganzschluss, Plagalschluss, Trugschluss).
    Schluss,
    /// Baut Spannung zur Dominante auf.
    Spannung,
    /// Führt weiter, ohne zu schließen.
    Weiter,
    /// Chromatische Farbe: entlehnte Akkorde, Zwischendominanten, Neapolitaner.
    Farbe,
    /// Öffnet eine Nachbartonart.
    Ausweichen,
    /// Keine Regel, aber in der Bibliothek häufig.
    Haeufig,
}

use Category::*;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    #[serde(flatten)]
    pub info: ChordInfo,
    pub score: f32,
    pub category: Category,
    /// Reason (translated).
    pub reason: String,
    /// For secondary dominants: the key one can move to this way (translated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_key: Option<String>,
    /// Stimmung des Übergangs vom letzten Akkord hierher.
    pub mood: Mood,
}

// ------------------------------------------------------------------ Stimmung

/// Stimmung eines Übergangs – für die Farben im Quintenzirkel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mood {
    /// Spannung löst sich: Dominante → Ziel, Plagalschluss.
    Loesend,
    /// Spannung baut sich auf: zur Dominante, Doppel-/Zwischendominante, Quartsextvorhalt.
    Spannend,
    /// Unerwartet: Trugschluss, chromatische Terzverwandtschaft.
    Ueberraschend,
    /// Hell: zu einem Durakkord der Tonart.
    Hell,
    /// Dunkel: zu Moll/vermindert oder aus Moll entlehnt.
    Dunkel,
}

impl Mood {
    /// Keyword (a key for `klang_i18n::translate`).
    pub fn label(self) -> &'static str {
        match self {
            Mood::Loesend => tk!("resolving"),
            Mood::Spannend => tk!("tense"),
            Mood::Ueberraschend => tk!("surprising"),
            Mood::Hell => tk!("bright"),
            Mood::Dunkel => tk!("dark"),
        }
    }
}

fn dominant_function(info: &ChordInfo) -> bool {
    let f = info.function.as_str();
    f.starts_with('D') || f.starts_with('Đ') || f.starts_with("(D") || f.starts_with("DD")
}

/// Stimmung des Übergangs `prev` → `next` (ohne `prev`: nach dem Klang von `next`).
pub fn transition_mood(prev: Option<&Chord>, next: &Chord, key: &Key) -> Mood {
    let ni = describe(next, key);
    let next_dark = next.quality.minor_third() || !ni.diatonic && secondary_target(next, key).is_none();
    let Some(prev) = prev else {
        return if next_dark { Mood::Dunkel } else { Mood::Hell };
    };
    let pi = describe(prev, key);
    let (pr, nr) = (prev.root, next.root);
    let prev_dom = dominant_function(&pi) || matches!(prev.quality, Dom7 | Dim7);
    let prev_dom_major = prev_dom && matches!(prev.quality, Major | Dom7);
    // Quartsextvorhalt ist Spannung, auch wenn der Bass schon zur Tonika greift
    let cadential64 = ni.function.starts_with("D⁶");
    if cadential64 {
        return Mood::Spannend;
    }
    // Trugschluss: Dominante → eine Stufe darüber (vi bzw. ♭VI)
    if prev_dom_major && ((pr + 2) % 12 == nr || (pr + 1) % 12 == nr) {
        return Mood::Ueberraschend;
    }
    // chromatische Terzverwandtschaft (z. B. C → As, C → E)
    let diff = (nr + 12 - pr) % 12;
    if matches!(diff, 3 | 4 | 8 | 9) && prev.quality.triad() == next.quality.triad() && !ni.diatonic && secondary_target(next, key).is_none() && !ni.function.starts_with("sN") {
        return Mood::Ueberraschend;
    }
    // Auflösung: Quintfall aus der Dominante, Leitton aufwärts, Plagalschluss
    let falls_fifth = (pr + 12 - nr) % 12 == 7;
    let leading = matches!(prev.quality, Dim | Dim7 | HalfDim7) && (pr + 1) % 12 == nr;
    let plagal = matches!(pi.function.as_str(), "S" | "s" | "S⁷" | "s⁷") && nr == key.tonic % 12 && matches!(next.quality, Major | Minor);
    if (prev_dom && (falls_fifth || leading)) || plagal {
        return Mood::Loesend;
    }
    if dominant_function(&ni) || matches!(next.quality, Dom7 | Dim7) {
        return Mood::Spannend;
    }
    if next_dark {
        Mood::Dunkel
    } else {
        Mood::Hell
    }
}

// ------------------------------------------------------------------ Brüche

/// Harmonic break at the step `prev` → `next`: a chord that the key doesn't
/// explain (neither diatonic nor a secondary dominant, nor borrowed from
/// the parallel key, nor a Neapolitan), or a
/// tritone leap of the root without a dominant reason. Returns the
/// reason (a key for `klang_i18n::translate`).
pub fn harmonic_break(prev: Option<&Chord>, next: &Chord, key: &Key) -> Option<&'static str> {
    let ni = describe(next, key);
    let parallel = Key { tonic: key.tonic, minor: !key.minor, confidence: key.confidence };
    // eine Zwischendominante erklärt nur, wenn ihr Ziel ansteuerbar ist (kein verminderter Akkord)
    let tonicizes = secondary_target(next, key).is_some_and(|t| diatonic_triad(t, key) != Some(Dim));
    let explained = ni.diatonic || tonicizes || ni.function.starts_with("sN") || is_diatonic(next, &parallel);
    if !explained {
        return Some(tk!("out of key"));
    }
    let prev = prev?;
    let pi = describe(prev, key);
    let tritone = (next.root + 12 - prev.root) % 12 == 6;
    // erklärt: Neapolitaner → Dominante, Leittonakkord, Dominanten untereinander
    let dominantish = |i: &ChordInfo, c: &Chord| dominant_function(i) || matches!(c.quality, Dom7 | Dim | Dim7 | HalfDim7);
    if tritone && !pi.function.starts_with("sN") && !dominantish(&pi, prev) && !dominantish(&ni, next) {
        return Some(tk!("Tritone leap"));
    }
    None
}

// ------------------------------------------------------------------ Wege zum Schluss

/// Ein mehrschrittiger Weg vom aktuellen Akkord zu einem Schluss.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CadencePath {
    /// ganzschluss, plagalschluss, halbschluss, trugschluss, phrygisch
    pub goal: &'static str,
    /// Name of the kind of cadence (translated).
    pub label: String,
    pub steps: Vec<ChordInfo>,
    /// Stimmung je Übergang (in steps[i] hinein, der erste vom aktuellen Akkord).
    pub moods: Vec<Mood>,
    pub score: f32,
    /// true: längere Variante desselben Schlusses (steht direkt nach dem kürzeren besten Weg).
    pub longer: bool,
}

/// Abschlag je zusätzlichem Akkord: kurze Wege gewinnen, wenn sie gleich gut klingen.
const STEP_FACTOR: f32 = 0.93;

const GOALS: [(&str, &str); 5] = [
    ("ganzschluss", tk!("Perfect cadence")),
    ("plagalschluss", tk!("Plagal cadence")),
    ("halbschluss", tk!("Half cadence")),
    ("trugschluss", tk!("Deceptive cadence")),
    ("phrygisch", tk!("Phrygian half cadence")),
];

/// Welcher Schluss endet mit `prev` → `next`?
fn goal_of(prev: &Chord, next: &Chord, key: &Key) -> Option<usize> {
    let off = |c: &Chord| (c.root + 12 - key.tonic % 12) % 12;
    let (po, no) = (off(prev), off(next));
    let tonic = no == 0 && matches!(next.quality, Major | Minor) && next.inversion() == 0;
    let pi = describe(prev, key);
    let prev_d = (po == 7 && matches!(prev.quality, Major | Dom7)) || (po == 11 && matches!(prev.quality, Dim | Dim7 | HalfDim7));
    if tonic && prev_d && prev.inversion() == 0 || tonic && po == 11 && prev_d {
        return Some(0);
    }
    if tonic && matches!(pi.function.as_str(), "S" | "s") {
        return Some(1);
    }
    let dom_target = no == 7 && next.quality == Major && next.inversion() == 0;
    if dom_target && key.minor && po == 5 && prev.quality == Minor && prev.inversion() == 1 {
        return Some(4);
    }
    if dom_target && !matches!(po, 0 | 7) {
        return Some(2);
    }
    let deceptive_target = if key.minor { no == 8 && next.quality == Major } else { no == 9 && next.quality == Minor };
    if prev_d && po == 7 && deceptive_target {
        return Some(3);
    }
    None
}

/// Mehrschrittige Wege zu einem Schluss, bis `max_steps` Akkorde: je
/// Schlussart der beste und – wenn es einen gibt – der beste längere Weg.
/// Die Schritte folgen den Fortsetzungsregeln; `weight` gewichtet einen
/// Schritt (z. B. mit der Häufigkeit in der Bibliothek).
pub fn cadence_paths(key: &Key, history: &[Chord], max_steps: usize, weight: &dyn Fn(&Chord, &Chord, f32) -> f32) -> Vec<CadencePath> {
    let tonic = Chord::new(key.tonic, if key.minor { Minor } else { Major });
    let start: Vec<Chord> = if history.is_empty() { vec![tonic] } else { history.to_vec() };
    // beste Wege je Schlussart und Länge: (Punkte, Akkorde)
    let mut best: Vec<Vec<Option<(f32, Vec<Chord>)>>> = vec![vec![None; max_steps]; GOALS.len()];
    let origin = *start.last().unwrap();
    let mut stack: Vec<(Vec<Chord>, f32)> = vec![(Vec::new(), 0.0)];
    // Fortsetzungen hängen nur von den letzten zwei Akkorden ab: zwischenspeichern
    let mut cache: std::collections::HashMap<(Chord, Option<Chord>), Vec<Suggestion>> = std::collections::HashMap::new();
    while let Some((path, log_sum)) = stack.pop() {
        if path.len() >= max_steps {
            continue;
        }
        let mut hist = start.clone();
        hist.extend_from_slice(&path);
        let prev = *hist.last().unwrap();
        let before = hist.len().checked_sub(2).map(|i| hist[i]);
        let cands = cache
            .entry((prev, before))
            .or_insert_with(|| {
                let tail: Vec<Chord> = before.into_iter().chain(std::iter::once(prev)).collect();
                suggest(key, &tail, 12)
            })
            .clone();
        for s in cands.iter().filter(|s| s.category != Category::Ausweichen && s.score >= 0.3).take(8) {
            let next = s.info.chord;
            let at_tonic = next.root == key.tonic % 12 && matches!(next.quality, Major | Minor) && next.inversion() == 0;
            // keine Kreise: kein Akkord doppelt im Weg; zurück zum Ausgangsakkord
            // nur als Ziel eines Schlusses (vi–IV–V–vi), nicht unterwegs
            let back_home = next == origin && !at_tonic;
            if path.contains(&next) || (back_home && path.is_empty()) {
                continue;
            }
            let w = weight(&prev, &next, s.score).max(1e-3);
            let ls = log_sum + w.ln();
            let mut np = path.clone();
            np.push(next);
            let n = np.len() as f32;
            let score = (ls / n).exp() * STEP_FACTOR.powi(np.len() as i32 - 1);
            if let Some(g) = goal_of(&prev, &next, key) {
                let slot = &mut best[g][np.len() - 1];
                if slot.as_ref().is_none_or(|(b, _)| score > *b) {
                    *slot = Some((score, np.clone()));
                }
            }
            // an der Tonika (oder wieder am Ausgangsakkord) angekommen: dieser Weg ist zu Ende
            if !at_tonic && !back_home {
                stack.push((np, ls));
            }
        }
    }
    let last = *start.last().unwrap();
    let build = |g: usize, score: f32, chords: &[Chord], longer: bool| {
        let mut prev = last;
        let moods = chords
            .iter()
            .map(|c| {
                let m = transition_mood(Some(&prev), c, key);
                prev = *c;
                m
            })
            .collect();
        CadencePath {
            goal: GOALS[g].0,
            label: klang_i18n::translate(GOALS[g].1),
            steps: chords.iter().map(|c| describe(c, key)).collect(),
            moods,
            score: (score * 100.0).round() / 100.0,
            longer,
        }
    };
    // je Schlussart: der beste Weg, dahinter der beste längere (wenn er nicht viel schlechter ist)
    let mut groups: Vec<(f32, Vec<CadencePath>)> = Vec::new();
    for (g, by_len) in best.iter().enumerate() {
        let pick = |from: usize| by_len.iter().enumerate().skip(from).filter_map(|(i, b)| b.as_ref().map(|b| (i, b))).max_by(|a, b| a.1 .0.total_cmp(&b.1 .0));
        let Some((len, (score, chords))) = pick(0) else { continue };
        let mut paths = vec![build(g, *score, chords, false)];
        if let Some((_, (s2, c2))) = pick(len + 1) {
            if *s2 >= 0.75 * score {
                paths.push(build(g, *s2, c2, true));
            }
        }
        groups.push((*score, paths));
    }
    groups.sort_by(|a, b| b.0.total_cmp(&a.0));
    groups.into_iter().flat_map(|(_, p)| p).collect()
}

type Rule = (&'static str, f32, Category, &'static str);

/// Übliche Fortsetzungen in Dur: Stufe → (Ziel, Gewicht, Art, Begründung).
/// The reasons are keys; they are translated in [`suggest`].
const MAJOR_RULES: &[(&str, &[Rule])] = &[
    ("I", &[
        ("IV", 0.85, Weiter, tk!("T → S: opens the cadence")),
        ("V", 0.80, Spannung, tk!("T → D: builds tension, half cadence possible")),
        ("vi", 0.75, Weiter, tk!("T → Tp: falling third, soft")),
        ("ii", 0.60, Weiter, tk!("T → Sp: prepares the dominant")),
        ("iii", 0.40, Weiter, tk!("T → Dp")),
        ("V7/IV", 0.45, Ausweichen, tk!("Tonic with minor seventh pulls toward the subdominant")),
        ("V7/V", 0.42, Ausweichen, tk!("Double dominant: opens up the dominant key")),
        ("V7/vi", 0.38, Ausweichen, tk!("Secondary dominant of the tonic relative")),
        ("iv", 0.35, Farbe, tk!("Minor subdominant: dark color")),
        ("♭VI", 0.30, Farbe, tk!("borrowed from minor: softer, cinematic sound")),
        ("♭VII", 0.30, Farbe, tk!("borrowed from minor: Mixolydian color")),
    ]),
    ("I6", &[
        ("IV", 0.70, Weiter, tk!("Bass keeps rising to the subdominant")),
        ("ii6", 0.65, Spannung, tk!("Sp with the third in the bass prepares the dominant")),
        ("V", 0.55, Spannung, tk!("T → D")),
        ("vi", 0.30, Weiter, tk!("Falling third")),
    ]),
    ("I64", &[
        ("V7", 0.95, Spannung, tk!("Cadential six-four resolves to the dominant")),
        ("V", 0.90, Spannung, tk!("Cadential six-four resolves to the dominant")),
    ]),
    ("ii", &[
        ("V", 0.90, Spannung, tk!("Sp → D: falling fifth, the standard way to the dominant")),
        ("V7", 0.88, Spannung, tk!("Sp → D⁷: falling fifth")),
        ("I64", 0.75, Spannung, tk!("Sp → cadential six-four → dominant")),
        ("vii°6", 0.45, Spannung, tk!("to the incomplete dominant")),
        ("V7/V", 0.45, Farbe, tk!("Sp turns chromatically into the double dominant")),
        ("IV", 0.30, Weiter, tk!("back to the subdominant")),
    ]),
    ("iii", &[
        ("vi", 0.85, Weiter, tk!("Dp → Tp: falling fifth")),
        ("IV", 0.60, Weiter, tk!("Dp → S")),
        ("ii", 0.35, Weiter, tk!("Dp → Sp")),
        ("V7/vi", 0.30, Farbe, tk!("Dp becomes the secondary dominant of Tp")),
    ]),
    ("IV", &[
        ("V", 0.90, Spannung, tk!("S → D: the core of every cadence")),
        ("V7", 0.85, Spannung, tk!("S → D⁷")),
        ("I64", 0.75, Spannung, tk!("Cadential six-four before the dominant")),
        ("I", 0.70, Schluss, tk!("Plagal cadence (church cadence, \"Amen\")")),
        ("V7/V", 0.50, Spannung, tk!("Double dominant strengthens the pull toward the dominant")),
        ("iv", 0.45, Farbe, tk!("S → s: the major subdominant turns minor – typical just before the end")),
        ("ii", 0.45, Weiter, tk!("S → Sp")),
        ("vii°6", 0.40, Spannung, tk!("to the incomplete dominant")),
        ("vi", 0.30, Weiter, tk!("S → Tp")),
    ]),
    ("V", &[
        ("I", 0.95, Schluss, tk!("Authentic cadence (perfect cadence)")),
        ("vi", 0.80, Schluss, tk!("Deceptive cadence: D → Tp instead of T")),
        ("V7", 0.60, Spannung, tk!("Adding the seventh: strengthens the pull toward the tonic")),
        ("I6", 0.40, Weiter, tk!("to the tonic with the third in the bass: no ending, it continues")),
        ("♭VI", 0.40, Farbe, tk!("Deceptive cadence to ♭VI (borrowed from minor)")),
        ("IV6", 0.25, Weiter, tk!("weakened way back")),
    ]),
    ("V7", &[
        ("I", 0.95, Schluss, tk!("Authentic cadence (perfect cadence)")),
        ("vi", 0.82, Schluss, tk!("Deceptive cadence: D⁷ → Tp")),
        ("I6", 0.40, Weiter, tk!("to the tonic with the third in the bass: it continues")),
        ("♭VI", 0.40, Farbe, tk!("Deceptive cadence to ♭VI (borrowed from minor)")),
    ]),
    ("vi", &[
        ("ii", 0.80, Weiter, tk!("Tp → Sp: falling fifth")),
        ("IV", 0.80, Weiter, tk!("Tp → S")),
        ("V", 0.50, Spannung, tk!("Tp → D")),
        ("iii", 0.35, Weiter, tk!("Tp → Dp")),
        ("V7/ii", 0.40, Farbe, tk!("Secondary dominant of Sp")),
        ("I6", 0.30, Weiter, tk!("to the tonic with the third in the bass")),
    ]),
    ("vii°", &[
        ("I", 0.90, Schluss, tk!("Leading tone resolves: incomplete dominant → T")),
        ("I6", 0.60, Weiter, tk!("incomplete dominant → T with the third in the bass")),
    ]),
    ("vii°6", &[
        ("I", 0.90, Schluss, tk!("Leading tone resolves")),
        ("I6", 0.65, Weiter, tk!("Voice exchange: T with the third in the bass")),
    ]),
    ("V/V", &[
        ("V", 0.90, Spannung, tk!("DD → D")),
        ("V7", 0.85, Spannung, tk!("DD → D⁷")),
        ("I64", 0.70, Spannung, tk!("DD → cadential six-four")),
    ]),
    ("V7/V", &[
        ("V", 0.90, Spannung, tk!("DD⁷ → D")),
        ("V7", 0.80, Spannung, tk!("DD⁷ → D⁷")),
        ("I64", 0.70, Spannung, tk!("DD⁷ → cadential six-four")),
    ]),
    ("V7/ii", &[("ii", 0.95, Weiter, tk!("Secondary dominant resolves to Sp")), ("V7/V", 0.40, Farbe, tk!("Chain of secondary dominants (falling fifths)"))]),
    ("V7/iii", &[("iii", 0.95, Weiter, tk!("Secondary dominant resolves to Dp")), ("V7/vi", 0.40, Farbe, tk!("Chain of secondary dominants"))]),
    ("V7/IV", &[("IV", 0.95, Weiter, tk!("Secondary dominant resolves to S")), ("ii", 0.35, Weiter, tk!("Deceptive resolution of the secondary dominant"))]),
    ("V7/vi", &[("vi", 0.95, Weiter, tk!("Secondary dominant resolves to Tp")), ("IV", 0.40, Farbe, tk!("Deceptive resolution of the secondary dominant")), ("V7/ii", 0.40, Farbe, tk!("Chain of secondary dominants"))]),
    ("iv", &[
        ("I", 0.75, Schluss, tk!("Minor plagal cadence")),
        ("V", 0.70, Spannung, tk!("s → D")),
        ("V7", 0.65, Spannung, tk!("s → D⁷")),
        ("I64", 0.55, Spannung, tk!("s → cadential six-four")),
    ]),
    ("♭VI", &[
        ("♭VII", 0.65, Weiter, tk!("♭VI → ♭VII → I: hymn-like ending")),
        ("V", 0.60, Spannung, tk!("♭VI → D")),
        ("iv", 0.45, Farbe, tk!("stays in the minor color")),
        ("IV", 0.40, Weiter, tk!("back to major")),
    ]),
    ("♭VII", &[
        ("I", 0.75, Schluss, tk!("♭VII → I: Mixolydian ending")),
        ("IV", 0.50, Weiter, tk!("♭VII → IV")),
    ]),
    ("N6", &[
        ("V", 0.85, Spannung, tk!("Neapolitan → dominant")),
        ("V7", 0.85, Spannung, tk!("Neapolitan → D⁷")),
        ("I64", 0.80, Spannung, tk!("Neapolitan → cadential six-four")),
    ]),
];

/// Übliche Fortsetzungen in Moll.
const MINOR_RULES: &[(&str, &[Rule])] = &[
    ("i", &[
        ("iv", 0.85, Weiter, tk!("t → s: opens the cadence")),
        ("V", 0.80, Spannung, tk!("t → D: builds tension")),
        ("VI", 0.70, Weiter, tk!("t → tG: falling third")),
        ("iiø7", 0.55, Spannung, tk!("t → s⁶₅: prepares the dominant")),
        ("III", 0.55, Weiter, tk!("t → tP: turn to the relative key")),
        ("VII", 0.45, Weiter, tk!("t → dP: descending bass")),
        ("v6", 0.40, Weiter, tk!("Lament bass: the bass descends chromatically/diatonically")),
        ("V7/iv", 0.45, Ausweichen, tk!("Major tonic with a seventh pulls toward the subdominant")),
        ("V7/III", 0.40, Ausweichen, tk!("opens up the relative key")),
        ("N6", 0.40, Farbe, tk!("Neapolitan: sharp color before the dominant")),
        ("iv6", 0.38, Spannung, tk!("s with the third in the bass – leads to the Phrygian half cadence")),
        ("V/V", 0.35, Spannung, tk!("Double dominant")),
    ]),
    ("i64", &[
        ("V7", 0.95, Spannung, tk!("Cadential six-four resolves to the dominant")),
        ("V", 0.90, Spannung, tk!("Cadential six-four resolves to the dominant")),
    ]),
    ("ii°", &[
        ("V", 0.90, Spannung, tk!("s⁶ → D")),
        ("V7", 0.85, Spannung, tk!("s⁶ → D⁷")),
        ("i64", 0.75, Spannung, tk!("s⁶ → cadential six-four")),
    ]),
    ("iiø7", &[
        ("V", 0.90, Spannung, tk!("s⁶₅ → D: falling fifth")),
        ("V7", 0.88, Spannung, tk!("s⁶₅ → D⁷")),
        ("i64", 0.75, Spannung, tk!("s⁶₅ → cadential six-four")),
    ]),
    ("III", &[
        ("VI", 0.80, Weiter, tk!("tP → tG: falling fifth")),
        ("iv", 0.60, Weiter, tk!("tP → s")),
        ("VII", 0.40, Weiter, tk!("tP → dP")),
        ("V7/VI", 0.35, Farbe, tk!("Secondary dominant of tG")),
        ("i", 0.30, Weiter, tk!("back to the tonic")),
    ]),
    ("iv", &[
        ("V", 0.90, Spannung, tk!("s → D: the core of the cadence")),
        ("V7", 0.85, Spannung, tk!("s → D⁷")),
        ("i64", 0.75, Spannung, tk!("Cadential six-four before the dominant")),
        ("i", 0.65, Schluss, tk!("Plagal cadence in minor")),
        ("N6", 0.45, Farbe, tk!("s → sN: the Neapolitan sharpens it")),
        ("V/V", 0.45, Spannung, tk!("Double dominant")),
        ("iv6", 0.42, Spannung, tk!("Third in the bass: afterwards the bass falls a semitone to the dominant (Phrygian)")),
        ("VII", 0.40, Weiter, tk!("s → dP")),
        ("ii°", 0.35, Spannung, tk!("s → s⁶")),
    ]),
    ("iv6", &[
        ("V", 0.95, Spannung, tk!("Phrygian half cadence: the bass falls a semitone to the dominant")),
    ]),
    ("v", &[
        ("VI", 0.55, Weiter, tk!("Minor dominant → tG")),
        ("iv6", 0.50, Weiter, tk!("Lament bass continues downward")),
        ("iv", 0.45, Weiter, tk!("Minor dominant → s")),
    ]),
    ("v6", &[
        ("iv6", 0.80, Weiter, tk!("Lament bass: continues stepwise downward")),
        ("VI", 0.40, Weiter, tk!("to tG")),
    ]),
    ("V", &[
        ("i", 0.95, Schluss, tk!("Authentic cadence (perfect cadence)")),
        ("VI", 0.85, Schluss, tk!("Deceptive cadence: D → tG")),
        ("V7", 0.60, Spannung, tk!("Adding the seventh: strengthens the pull toward the tonic")),
        ("I", 0.45, Farbe, tk!("Picardy third: ending in major")),
    ]),
    ("V7", &[
        ("i", 0.95, Schluss, tk!("Authentic cadence (perfect cadence)")),
        ("VI", 0.85, Schluss, tk!("Deceptive cadence: D⁷ → tG")),
        ("I", 0.45, Farbe, tk!("Picardy third: ending in major")),
    ]),
    ("VI", &[
        ("ii°6", 0.70, Spannung, tk!("tG → s⁶: falling fifth toward the dominant")),
        ("iv", 0.70, Weiter, tk!("tG → s: falling third")),
        ("V", 0.60, Spannung, tk!("tG → D: the bass falls a semitone")),
        ("N6", 0.55, Farbe, tk!("tG → Neapolitan")),
        ("III", 0.40, Weiter, tk!("tG → tP")),
    ]),
    ("VII", &[
        ("III", 0.85, Weiter, tk!("dP → tP: falling fifth (dominant of the relative key)")),
        ("i", 0.35, Weiter, tk!("back to the tonic")),
        ("VI", 0.30, Weiter, tk!("Bass descends")),
    ]),
    ("vii°7", &[
        ("i", 0.95, Schluss, tk!("Diminished seventh chord resolves")),
        ("i64", 0.40, Spannung, tk!("delayed by the cadential six-four")),
    ]),
    ("N6", &[
        ("V", 0.85, Spannung, tk!("Neapolitan → dominant")),
        ("V7", 0.85, Spannung, tk!("Neapolitan → D⁷")),
        ("i64", 0.80, Spannung, tk!("Neapolitan → cadential six-four")),
        ("vii°7", 0.50, Spannung, tk!("Neapolitan → diminished seventh chord")),
    ]),
    ("V/V", &[
        ("V", 0.90, Spannung, tk!("DD → D")),
        ("V7", 0.85, Spannung, tk!("DD → D⁷")),
        ("i64", 0.70, Spannung, tk!("DD → cadential six-four")),
    ]),
    ("V7/V", &[
        ("V", 0.90, Spannung, tk!("DD⁷ → D")),
        ("i64", 0.70, Spannung, tk!("DD⁷ → cadential six-four")),
    ]),
    ("V7/iv", &[("iv", 0.95, Weiter, tk!("Secondary dominant resolves to s"))]),
    ("V7/III", &[("III", 0.95, Weiter, tk!("Secondary dominant resolves to tP"))]),
    ("V7/VI", &[("VI", 0.95, Weiter, tk!("Secondary dominant resolves to tG"))]),
    ("I", &[("iv", 0.50, Weiter, tk!("Major tonic becomes the secondary dominant of s"))]),
];

/// Wie gut passt `prev` auf die Stufe der Regel? 0 = gar nicht.
fn match_rank(rule: &Chord, prev: &Chord) -> u8 {
    if rule.root != prev.root || rule.quality.triad() != prev.quality.triad() {
        return 0;
    }
    let same_bass = rule.bass_pc() == prev.bass_pc();
    // Quartsextakkord nur bei Quinte im Bass
    if rule.inversion() == 2 && !rule.quality.is_seventh() && !same_bass {
        return 0;
    }
    match (rule.quality == prev.quality, same_bass) {
        (true, true) => 4,
        (true, false) => 3,
        (false, true) => 2,
        (false, false) => 1,
    }
}

fn common_tones(a: &Chord, b: &Chord) -> usize {
    let pa = a.pcs();
    b.pcs().iter().filter(|p| pa.contains(p)).count()
}

fn to_key_of(ch: &Chord, key: &Key) -> Option<String> {
    let target = secondary_target(ch, key)?;
    let tq = diatonic_triad(target, key)?;
    Some(key_with((key.tonic + target) % 12, tq.minor_third()).name())
}

/// Fortsetzungen nach dem letzten Akkord von `history` (leer = Anfang),
/// beste zuerst. Regeln der Funktionsharmonik; für fremde Akkorde
/// Grundtonschritte (Quintfall zuerst) und gemeinsame Töne.
pub fn suggest(key: &Key, history: &[Chord], limit: usize) -> Vec<Suggestion> {
    let rules = if key.minor { MINOR_RULES } else { MAJOR_RULES };
    let tonic = if key.minor { "i" } else { "I" };
    let make = |id: &str, w: f32, cat: Category, reason: &str| -> Option<Suggestion> {
        let ch = parse_roman(id, key)?;
        let mood = transition_mood(history.last(), &ch, key);
        Some(Suggestion { info: describe(&ch, key), score: w, category: cat, reason: klang_i18n::translate(reason), to_key: None, mood })
    };
    let mut out: Vec<Suggestion> = Vec::new();
    let Some(prev) = history.last() else {
        let start: [(&str, f32, Category, &str); 4] = [
            (tonic, 1.0, Weiter, tk!("Start with the tonic")),
            (if key.minor { "iv" } else { "IV" }, 0.4, Weiter, tk!("Start with the subdominant")),
            (if key.minor { "VI" } else { "vi" }, 0.35, Weiter, tk!("Start with the relative chord")),
            ("V", 0.3, Spannung, tk!("Upbeat on the dominant")),
        ];
        return start.iter().filter_map(|&(id, w, c, r)| make(id, w, c, r)).collect();
    };
    let before = history.len().checked_sub(2).map(|i| history[i]);

    let best_rule = rules
        .iter()
        .filter_map(|(id, list)| parse_roman(id, key).map(|c| (match_rank(&c, prev), list)))
        .filter(|(r, _)| *r > 0)
        .max_by_key(|(r, _)| *r)
        .map(|(_, list)| *list);

    if let Some(list) = best_rule {
        for &(id, w, cat, reason) in list {
            if let Some(s) = make(id, w, cat, reason) {
                out.push(s);
            }
        }
    } else {
        // Fremder Akkord: Grundtonschritte und Rückkehr in die Tonart.
        let catalog: &[&str] = if key.minor {
            &["i", "ii°", "III", "iv", "V", "V7", "VI", "VII", "vii°7"]
        } else {
            &["I", "ii", "iii", "IV", "V", "V7", "vi", "vii°"]
        };
        for id in catalog {
            let Some(ch) = parse_roman(id, key) else { continue };
            let step = (ch.root + 12 - prev.root) % 12;
            let (w, why) = match step {
                5 => (0.70, tk!("Falling fifth: the strongest root movement")),
                2 => (0.50, tk!("Rising second")),
                8 | 9 => (0.45, tk!("Falling third")),
                7 => (0.35, tk!("Rising fifth")),
                3 | 4 => (0.30, tk!("Rising third")),
                10 | 11 => (0.30, tk!("Falling second")),
                _ => (0.15, tk!("Change")),
            };
            let resolves = matches!(prev.quality, Major | Dom7) && step == 5;
            let (w, why, cat) = if resolves {
                (0.85, tk!("Resolution: a fifth down"), Schluss)
            } else {
                (w, why, Weiter)
            };
            if let Some(s) = make(id, w + 0.04 * common_tones(prev, &ch) as f32, cat, why) {
                out.push(s);
            }
        }
        if let Some(s) = make(tonic, 0.5, Schluss, tk!("back to the tonic")) {
            out.push(s);
        }
    }

    for s in &mut out {
        let ch = s.info.chord;
        s.score += 0.02 * common_tones(prev, &ch) as f32;
        if before.is_some_and(|b| b.same_harmony(&ch) && b.bass_pc() == ch.bass_pc()) && s.category != Schluss {
            s.score -= 0.10; // Hin und her vermeiden
        }
        s.to_key = to_key_of(&ch, key);
    }
    out.retain(|s| s.info.chord != *prev);
    // gleiche Akkorde nur einmal (beste Begründung behalten)
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut seen: Vec<Chord> = Vec::new();
    out.retain(|s| {
        if seen.contains(&s.info.chord) {
            false
        } else {
            seen.push(s.info.chord);
            true
        }
    });
    out.truncate(limit);
    for s in &mut out {
        s.score = (s.score.clamp(0.0, 1.0) * 100.0).round() / 100.0;
    }
    out
}

// ------------------------------------------------------------------ Kadenzen

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CadenceTemplate {
    pub id: &'static str,
    /// Name and description (translated).
    pub name: String,
    pub description: String,
    pub steps: Vec<ChordInfo>,
    /// Stimmung je Übergang (steps[i] → steps[i+1]).
    pub moods: Vec<Mood>,
}

type Template = (&'static str, &'static str, &'static str, &'static [&'static str]);

const MAJOR_CADENCES: &[Template] = &[
    ("authentisch", tk!("Authentic cadence"), tk!("Perfect cadence T – S – D⁷ – T: the classic ending."), &["I", "IV", "V7", "I"]),
    ("subdominantparallele", tk!("With subdominant relative"), tk!("Sp with the third in the bass instead of S – this is how Bach ends many chorales."), &["I", "ii6", "V7", "I"]),
    ("quartsext", tk!("With cadential six-four"), tk!("The dominant is delayed by the cadential six-four chord."), &["I", "IV", "I64", "V7", "I"]),
    ("plagal", tk!("Plagal cadence"), tk!("S → T, the \"Amen\" cadence."), &["I", "IV", "I"]),
    ("halbschluss", tk!("Half cadence"), tk!("Ends open on the dominant – the phrase calls for an answer."), &["I", "vi", "IV", "V"]),
    ("trugschluss", tk!("Deceptive cadence"), tk!("The dominant resolves not to the tonic but to Tp."), &["I", "IV", "V7", "vi"]),
    ("doppeldominante", tk!("With double dominant"), tk!("The dominant of the dominant sharpens the pull."), &["I", "IV", "V7/V", "V7", "I"]),
    ("neapolitaner", tk!("Neapolitan"), tk!("The Neapolitan sixth chord colors the subdominant."), &["I", "N6", "V7", "I"]),
    ("moll-plagal", tk!("Minor plagal cadence"), tk!("The minor subdominant before the ending – soft and dark."), &["I", "IV", "iv", "I"]),
    ("quintfall", tk!("Falling-fifths sequence"), tk!("All roots fall in fifths – a Baroque sequence."), &["I", "IV", "vii°", "iii", "vi", "ii", "V7", "I"]),
];

const MINOR_CADENCES: &[Template] = &[
    ("authentisch", tk!("Authentic cadence"), tk!("Perfect cadence t – s – D⁷ – t."), &["i", "iv", "V7", "i"]),
    ("sixte-ajoutee", tk!("With s⁶₅"), tk!("Subdominant with added sixth before the dominant."), &["i", "iiø65", "V7", "i"]),
    ("quartsext", tk!("With cadential six-four"), tk!("The dominant is delayed by the cadential six-four chord."), &["i", "iv", "i64", "V7", "i"]),
    ("plagal", tk!("Plagal cadence"), tk!("s → t."), &["i", "iv", "i"]),
    ("halbschluss", tk!("Half cadence"), tk!("Ends open on the dominant."), &["i", "iv", "V"]),
    ("phrygisch", tk!("Phrygian half cadence"), tk!("s with the third in the bass → D: the bass falls a semitone – a Baroque movement ending."), &["i", "iv6", "V"]),
    ("trugschluss", tk!("Deceptive cadence"), tk!("The dominant resolves to tG instead of the tonic."), &["i", "iv", "V7", "VI"]),
    ("neapolitaner", tk!("Neapolitan"), tk!("The Neapolitan sixth chord before the dominant."), &["i", "N6", "V7", "i"]),
    ("picardie", tk!("Picardy third"), tk!("Ending in major – typical of Baroque minor-key pieces."), &["i", "iv", "V7", "I"]),
    ("lamento", tk!("Lament bass"), tk!("The bass falls stepwise from the tonic to the dominant."), &["i", "v6", "iv6", "V"]),
    ("quintfall", tk!("Falling-fifths sequence"), tk!("All roots fall in fifths – a Baroque sequence."), &["i", "iv", "VII", "III", "VI", "ii°", "V7", "i"]),
];

pub fn cadences(key: &Key) -> Vec<CadenceTemplate> {
    let list = if key.minor { MINOR_CADENCES } else { MAJOR_CADENCES };
    list.iter()
        .map(|&(id, name, description, steps)| {
            let chords: Vec<Chord> = steps.iter().filter_map(|s| parse_roman(s, key)).collect();
            CadenceTemplate {
                id,
                name: klang_i18n::translate(name),
                description: klang_i18n::translate(description),
                moods: chords.windows(2).map(|w| transition_mood(Some(&w[0]), &w[1], key)).collect(),
                steps: chords.iter().map(|c| describe(c, key)).collect(),
            }
        })
        .collect()
}

// ------------------------------------------------------------------ Erkennen

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedPitch {
    pub pitch: u8,
    pub start: f64,
    pub length: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedChord {
    pub start: f64,
    pub length: f64,
    pub chord: Chord,
}

/// Akkord zu einer Tonverteilung (Gewicht je Tonhöhenklasse) und Basston.
pub fn identify(weights: &[f64; 12], bass: Option<u8>, key: Option<&Key>) -> Option<Chord> {
    let total: f64 = weights.iter().sum();
    let max = weights.iter().cloned().fold(0.0, f64::max);
    if total <= 0.0 {
        return None;
    }
    let distinct = weights.iter().filter(|&&w| w >= 0.08 * max).count();
    if distinct < 2 {
        return None;
    }
    let mut best: Option<(f64, Chord)> = None;
    // Nur klingende Töne kommen als Grundton in Frage (ohne Grundton verliert ein Akkord ohnehin).
    for root in (0..12u8).filter(|&r| weights[r as usize] >= 0.05 * max) {
        for q in Quality::ALL {
            let iv = q.intervals();
            let mut covered = 0.0;
            let mut missing = 0;
            let mut diatonic = true;
            for (n, &i) in iv.iter().enumerate() {
                let pc = (root + i) % 12;
                let w = weights[pc as usize];
                // Die Septime erklärt nur halb: Durchgangstöne sollen keinen Septakkord machen.
                covered += if n == 3 { 0.5 * w } else { w };
                if w < 0.05 * max && !(q.is_seventh() && i == 7) {
                    missing += 1;
                }
                if let Some(k) = key {
                    diatonic &= in_key(pc, k);
                }
            }
            let mut score = covered / total - 0.6 * (total - covered) / total - 0.18 * missing as f64;
            score -= match q {
                Dom7 | Dim7 => 0.05,
                HalfDim7 => 0.08,
                Min7 => 0.10,
                Maj7 => 0.12,
                Aug | Sus4 => 0.08,
                _ => 0.0,
            };
            if bass == Some(root) {
                score += 0.1;
            }
            if q.is_seventh() && bass == Some((root + iv[3]) % 12) && q != Dom7 {
                score -= 0.06; // Septime im Bass ist außer beim D⁷ selten
            }
            if key.is_some() && diatonic {
                score += 0.03;
            }
            if best.map(|(s, _)| score > s + 1e-9).unwrap_or(true) {
                best = Some((score, Chord::new(root, q)));
            }
        }
    }
    let (score, mut ch) = best?;
    if score < 0.35 {
        return None;
    }
    if let Some(b) = bass {
        if b != ch.root && ch.pcs().contains(&b) {
            ch.bass = Some(b);
        }
    }
    Some(ch)
}

/// Akkordfolge in Noten erkennen: Fenster von `window` Schlägen, gleiche
/// Nachbarn zusammengefasst. Einstimmige Stellen bleiben leer.
pub fn detect_chords(notes: &[TimedPitch], window: f64, key: Option<&Key>) -> Vec<DetectedChord> {
    let window = window.max(0.25);
    let notes: Vec<&TimedPitch> = notes.iter().filter(|n| n.length > 1e-9 && n.start.is_finite() && n.length.is_finite()).collect();
    let Some(start) = notes.iter().map(|n| n.start).reduce(f64::min) else { return Vec::new() };
    let end = notes.iter().map(|n| n.start + n.length).fold(f64::MIN, f64::max);
    let t0 = (start / window).floor() * window;
    let n_win = ((end - t0) / window - 1e-9).ceil().max(0.0) as usize;
    if n_win == 0 || n_win > 400_000 {
        return Vec::new();
    }
    // Jede Note verteilt ihre Dauer auf die Fenster, die sie berührt.
    let mut weights = vec![[0.0f64; 12]; n_win];
    let mut bass: Vec<Option<u8>> = vec![None; n_win];
    for n in &notes {
        let (a, b) = (n.start, n.start + n.length);
        let i0 = (((a - t0) / window).floor().max(0.0) as usize).min(n_win);
        let i1 = (((b - t0) / window).ceil().max(0.0) as usize).min(n_win);
        for i in i0..i1 {
            let wa = t0 + i as f64 * window;
            let ov = b.min(wa + window) - a.max(wa);
            if ov <= 1e-9 {
                continue;
            }
            weights[i][(n.pitch % 12) as usize] += ov;
            // Basston: tiefster Ton, der hier hörbar Gewicht hat (bei Arpeggien jeder ganze Ton)
            let weighty = ov >= 0.25 * window || ov >= 0.9 * n.length;
            if weighty && bass[i].map(|p| n.pitch < p).unwrap_or(true) {
                bass[i] = Some(n.pitch);
            }
        }
    }
    let mut out: Vec<DetectedChord> = Vec::new();
    for i in 0..n_win {
        let a = t0 + i as f64 * window;
        if let Some(ch) = identify(&weights[i], bass[i].map(|p| p % 12), key) {
            match out.last_mut() {
                Some(last) if last.chord == ch && (last.start + last.length - a).abs() < 1e-6 => last.length += window,
                _ => out.push(DetectedChord { start: a, length: window, chord: ch }),
            }
        }
    }
    out
}

// ------------------------------------------------------------------ Stimmführung

const BASS: (u8, u8) = (38, 57);
const TENOR: (u8, u8) = (48, 69);
const ALTO: (u8, u8) = (53, 74);
const SOPRANO: (u8, u8) = (58, 81);

fn in_range(pcs: &[u8], (lo, hi): (u8, u8)) -> Vec<u8> {
    (lo..=hi).filter(|m| pcs.contains(&(m % 12))).collect()
}

fn voice_cost(v: &[u8; 4], ch: &Chord, prev: Option<(&[u8], &Chord)>, key: Option<&Key>) -> Option<f32> {
    let iv = ch.quality.intervals();
    let pcs = ch.pcs();
    let count = |pc: u8| v.iter().filter(|m| *m % 12 == pc).count();
    // nötige Töne: Grundton, Terz, (Septime); Quinte darf fehlen außer bei °, ø, +, sus
    let fifth_optional = matches!(ch.quality, Major | Minor | Dom7 | Maj7 | Min7);
    for (i, &step) in iv.iter().enumerate() {
        let pc = (ch.root + step) % 12;
        if count(pc) == 0 && !(i == 2 && fifth_optional) {
            return None;
        }
    }
    let mut cost = 0.0f32;
    if count((ch.root + iv[2]) % 12) == 0 {
        cost += if ch.quality.is_seventh() { 1.0 } else { 3.0 };
    }
    // Verdopplungen
    if !ch.quality.is_seventh() && matches!(ch.quality, Major | Minor) {
        let third = (ch.root + iv[1]) % 12;
        if count(third) > 1 {
            cost += 4.0;
        }
        if count(ch.root) > 1 {
            cost -= 1.0;
        }
    }
    if ch.quality.is_seventh() && count((ch.root + iv[3]) % 12) > 1 {
        cost += 10.0;
    }
    if let Some(k) = key {
        let lt = (k.tonic + 11) % 12;
        if pcs.contains(&lt) && count(lt) > 1 {
            cost += 10.0;
        }
    }
    match prev {
        None => {
            cost += (v[3] as f32 - 72.0).abs() * 0.5 + (v[2] as f32 - 66.0).abs() * 0.3 + (v[1] as f32 - 60.0).abs() * 0.3 + (v[0] as f32 - 48.0).abs() * 0.2;
            // enge Lage in den Oberstimmen bevorzugen
            cost += (v[3] - v[1]) as f32 * 0.15;
        }
        Some((p, pch)) => {
            for i in 1..4 {
                let d = (v[i] as i32 - p[i] as i32).unsigned_abs() as f32;
                cost += d;
                if d > 7.0 {
                    cost += 3.0;
                }
            }
            cost += (v[0] as i32 - p[0] as i32).unsigned_abs() as f32 * 0.3;
            cost += (v[3] as f32 - 72.0).abs() * 0.1;
            // Quint- und Oktavparallelen
            for i in 0..4 {
                for j in i + 1..4 {
                    let pi = (p[j] as i32 - p[i] as i32).rem_euclid(12);
                    let ci = (v[j] as i32 - v[i] as i32).rem_euclid(12);
                    let mi = v[i] as i32 - p[i] as i32;
                    let mj = v[j] as i32 - p[j] as i32;
                    if pi == ci && (pi == 7 || pi == 0) && mi != 0 && mi.signum() == mj.signum() {
                        cost += 30.0;
                    }
                }
            }
            // Stimmen überholen einander nicht
            for i in 0..3 {
                if v[i] > p[i + 1] || v[i + 1] < p[i] {
                    cost += 2.0;
                }
            }
            // Leittöne steigen einen Halbton: Terz einer (Zwischen-)Dominante
            // vor ihrem Ziel, Leitton der Tonart nach D oder vii° (auch im Trugschluss).
            let mut tendency: Vec<u8> = Vec::new();
            if pch.quality.triad() == Major && (ch.root + 7) % 12 == pch.root {
                tendency.push((pch.root + 4) % 12);
            }
            if matches!(pch.quality, Dim | Dim7 | HalfDim7) && (pch.root + 1) % 12 == ch.root {
                tendency.push(pch.root);
            }
            if let Some(k) = key {
                let lt = (k.tonic + 11) % 12;
                let dominant_like = pch.root == (k.tonic + 7) % 12 && pch.quality.triad() == Major || pch.root == lt;
                if dominant_like && pcs.contains(&k.tonic) {
                    tendency.push(lt);
                }
            }
            for i in 1..4 {
                if tendency.contains(&(p[i] % 12)) && v[i] != p[i] + 1 {
                    cost += if i == 3 { 6.0 } else { 3.0 };
                }
            }
            if (v[0] as i32 - p[0] as i32).abs() > 7 {
                cost += 2.0;
            }
            // Septime löst sich abwärts auf (oder bleibt liegen)
            if pch.quality.is_seventh() {
                let sev = (pch.root + pch.quality.intervals()[3]) % 12;
                for i in 0..4 {
                    if p[i] % 12 == sev && v[i] != p[i] && !(v[i] < p[i] && p[i] - v[i] <= 2) {
                        cost += 6.0;
                    }
                }
            }
        }
    }
    Some(cost)
}

/// Einen Akkord vierstimmig setzen (Bass, Tenor, Alt, Sopran – aufsteigend),
/// möglichst glatt an den vorigen anschließend.
pub fn voice_chord(ch: &Chord, prev: Option<(&[u8], &Chord)>, key: Option<&Key>) -> Vec<u8> {
    let ch = ch.normalized();
    let pcs = ch.pcs();
    let mut basses = in_range(&[ch.bass_pc()], BASS);
    let target = prev.map(|(p, _)| p[0] as i32).unwrap_or(47);
    basses.sort_by_key(|&b| (b as i32 - target).abs());
    basses.truncate(2);
    let (tenor, alto, sop) = (in_range(&pcs, TENOR), in_range(&pcs, ALTO), in_range(&pcs, SOPRANO));
    let mut best: Option<(f32, [u8; 4])> = None;
    for &b in &basses {
        for &t in tenor.iter().filter(|&&t| t > b && t - b <= 24) {
            for &a in alto.iter().filter(|&&a| a > t && a - t <= 12) {
                for &s in sop.iter().filter(|&&s| s > a && s - a <= 12) {
                    let v = [b, t, a, s];
                    if let Some(c) = voice_cost(&v, &ch, prev, key) {
                        if best.map(|(bc, _)| c < bc).unwrap_or(true) {
                            best = Some((c, v));
                        }
                    }
                }
            }
        }
    }
    match best {
        Some((_, v)) => v.to_vec(),
        None => {
            // Notlösung: Bass und enge Lage darüber
            let b = basses.first().copied().unwrap_or(48);
            let mut v = vec![b];
            let mut m = 60u8;
            for pc in pcs.iter().take(3) {
                while m % 12 != *pc {
                    m += 1;
                }
                v.push(m);
            }
            v.sort_unstable();
            v
        }
    }
}

/// Akkordfolge vierstimmig setzen; `start` = Stimmen des Akkords davor.
pub fn voice_progression(chords: &[Chord], key: Option<&Key>, start: Option<(&[u8], &Chord)>) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(chords.len());
    let mut prev: Option<(Vec<u8>, Chord)> = start.filter(|(v, _)| v.len() == 4).map(|(v, c)| (v.to_vec(), *c));
    for ch in chords {
        let v = voice_chord(ch, prev.as_ref().map(|(v, c)| (v.as_slice(), c)), key);
        prev = if v.len() == 4 { Some((v.clone(), *ch)) } else { None };
        out.push(v);
    }
    out
}

// ------------------------------------------------------------------ Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> Key {
        key_with(0, false)
    }
    fn a() -> Key {
        key_with(9, true)
    }

    #[test]
    fn breaks_are_unexplained_chords_and_tritone_jumps() {
        let k = c();
        let ch = |s: &str| parse_chord(s, &k).unwrap();
        let brk = |a: &str, b: &str| harmonic_break(Some(&ch(a)), &ch(b), &k);
        // erklärt: leitereigen, Zwischendominante, entlehnt (As aus c-Moll), Neapolitaner, Dominante
        for (x, y) in [("C", "a"), ("C", "D7"), ("C", "As"), ("C", "f"), ("F", "Des"), ("Des", "G7"), ("C", "G7"), ("h°", "F")] {
            assert_eq!(brk(x, y), None, "{x} → {y}");
        }
        // tonartfremd: Fis-Dur in C ist keine Zwischendominante und nicht entlehnt
        assert_eq!(brk("C", "Fis"), Some("out of key"));
        assert_eq!(harmonic_break(None, &ch("Fis"), &k), Some("out of key"));
        assert_eq!(harmonic_break(None, &ch("Cis"), &k), None, "Cis = Des = Neapolitaner");
        // Tritonus-Sprung ohne Dominante: F → h (beide leitereigen, h aber als Moll-Akkord fremd) …
        assert_eq!(brk("F", "H"), None, "H = V/iii ist erklärt");
        let ka = a();
        let cha = |s: &str| parse_chord(s, &ka).unwrap();
        // a-Moll: d → As? As fremd; F → H-Dur: Zwischendominante zu e – erklärt
        assert_eq!(harmonic_break(Some(&cha("F")), &cha("H"), &ka), None);
        // C-Dur: a → Es (aus c-Moll entlehnt) ist ein Tritonus ohne Dominante → Bruch
        assert_eq!(brk("a", "Es"), Some("Tritone leap"));
    }

    #[test]
    fn names_use_german_spelling() {
        let es = key_with(3, false);
        assert_eq!(chord_name(&Chord::new(10, Dom7), &es), "B7");
        assert_eq!(chord_name(&Chord::new(8, Major), &es), "As");
        assert_eq!(chord_name(&Chord::new(11, Dim), &c()), "h°");
        assert_eq!(chord_name(&Chord::new(8, Dim7), &a()), "gis°7");
        assert_eq!(chord_name(&Chord::new(0, Major).inverted(1), &c()), "C/E");
        assert_eq!(chord_name(&Chord::new(6, Minor), &key_with(2, false)), "fis");
        assert_eq!(chord_name(&Chord::new(8, Major), &c()), "As");
        assert_eq!(chord_name(&Chord::new(10, Major), &a()), "B");
    }

    #[test]
    fn roman_and_function() {
        let k = c();
        let d = |s: &str| describe(&parse_chord(s, &k).unwrap(), &k);
        assert_eq!((d("G7").roman, d("G7").function), ("V⁷".to_string(), "D⁷".to_string()));
        assert_eq!(d("d").function, "Sp");
        assert_eq!(d("D7").roman, "V⁷/V");
        assert_eq!(d("D7").function, "DD⁷");
        assert_eq!(d("E7").roman, "V⁷/vi");
        assert_eq!(d("E7").function, "(D⁷) Tp");
        assert_eq!(d("C/G").function, "D⁶₄");
        assert_eq!(d("Des/F").roman, "N⁶");
        assert_eq!(d("f").function, "s");
        let m = a();
        let dm = |s: &str| describe(&parse_chord(s, &m).unwrap(), &m);
        assert_eq!(dm("E").function, "D");
        assert_eq!(dm("G7").roman, "V⁷/III");
        assert_eq!(dm("gis°7").roman, "vii°⁷");
        assert_eq!(dm("F").function, "tG");
        assert_eq!(dm("A").function, "T");
    }

    #[test]
    fn parses_roman_numerals() {
        let k = c();
        assert_eq!(parse_roman("V7", &k), Some(Chord::new(7, Dom7)));
        assert_eq!(parse_roman("ii6", &k), Some(Chord::new(2, Minor).inverted(1)));
        assert_eq!(parse_roman("I64", &k), Some(Chord::new(0, Major).inverted(2)));
        assert_eq!(parse_roman("V7/V", &k), Some(Chord::new(2, Dom7)));
        assert_eq!(parse_roman("V7/vi", &k), Some(Chord::new(4, Dom7)));
        assert_eq!(parse_roman("♭VI", &k), Some(Chord::new(8, Major)));
        assert_eq!(parse_roman("vii°", &k), Some(Chord::new(11, Dim)));
        assert_eq!(parse_roman("N6", &k), Some(Chord::new(1, Major).inverted(1)));
        let m = a();
        assert_eq!(parse_roman("vii°7", &m), Some(Chord::new(8, Dim7)));
        assert_eq!(parse_roman("VII", &m), Some(Chord::new(7, Major)));
        assert_eq!(parse_roman("iiø65", &m), Some(Chord::new(11, HalfDim7).inverted(1)));
        assert_eq!(parse_roman("iv6", &m), Some(Chord::new(2, Minor).inverted(1)));
        assert_eq!(parse_roman("V/V", &m), Some(Chord::new(11, Major)));
        assert_eq!(parse_roman("V7/iv", &m), Some(Chord::new(9, Dom7)));
    }

    #[test]
    fn parses_symbols_and_keys() {
        assert_eq!(parse_symbol("Es"), Some(Chord::new(3, Major)));
        assert_eq!(parse_symbol("fis"), Some(Chord::new(6, Minor)));
        assert_eq!(parse_symbol("Am"), Some(Chord::new(9, Minor)));
        assert_eq!(parse_symbol("B"), Some(Chord::new(10, Major)));
        assert_eq!(parse_symbol("Bb"), Some(Chord::new(10, Major)));
        assert_eq!(parse_symbol("H7"), Some(Chord::new(11, Dom7)));
        assert_eq!(parse_symbol("F#m7"), Some(Chord::new(6, Min7)));
        assert_eq!(parse_symbol("As"), Some(Chord::new(8, Major)));
        assert_eq!(parse_symbol("Dsus4"), Some(Chord::new(2, Sus4)));
        assert_eq!(parse_symbol("C/E"), Some(Chord::new(0, Major).inverted(1)));
        assert_eq!(parse_symbol("C/D"), None);
        assert_eq!(parse_key("Es-Dur").map(|k| (k.tonic, k.minor)), Some((3, false)));
        assert_eq!(parse_key("fis-Moll").map(|k| (k.tonic, k.minor)), Some((6, true)));
        assert_eq!(parse_key("a").map(|k| (k.tonic, k.minor)), Some((9, true)));
        assert_eq!(parse_key("C major").map(|k| (k.tonic, k.minor)), Some((0, false)));
    }

    #[test]
    fn suggestions_follow_functional_harmony() {
        let k = c();
        let g7 = parse_roman("V7", &k).unwrap();
        let s = suggest(&k, &[Chord::new(0, Major), Chord::new(5, Major), g7], 8);
        assert_eq!(s[0].info.name, "C");
        assert_eq!(s[0].category, Category::Schluss);
        assert!(s.iter().any(|x| x.info.name == "a" && x.reason.contains("Deceptive cadence")));
        // Zwischendominante löst sich auf
        let e7 = parse_roman("V7/vi", &k).unwrap();
        let s = suggest(&k, &[Chord::new(0, Major), e7], 5);
        assert_eq!(s[0].info.name, "a");
        // Zwischendominante nennt die Ausweichtonart
        let s = suggest(&k, &[Chord::new(0, Major)], 20);
        let d7 = s.iter().find(|x| x.info.name == "D7").unwrap();
        assert_eq!(d7.to_key.as_deref(), Some("G major"));
        // Anfang
        assert_eq!(suggest(&a(), &[], 3)[0].info.name, "a");
        // fremder Akkord: Quintfall bevorzugt
        let s = suggest(&k, &[Chord::new(6, Major)], 5);
        assert_eq!(s[0].info.name, "h°");
    }

    #[test]
    fn cadence_templates_parse_completely() {
        for key in [c(), a(), key_with(3, false), key_with(6, true)] {
            let list = if key.minor { MINOR_CADENCES } else { MAJOR_CADENCES };
            for (t, (_, _, _, steps)) in cadences(&key).iter().zip(list) {
                assert_eq!(t.steps.len(), steps.len(), "{} in {}", t.id, key.name_de());
            }
        }
        let rules = [MAJOR_RULES, MINOR_RULES];
        for (minor, set) in [false, true].into_iter().zip(rules) {
            let key = key_with(0, minor);
            for (from, list) in set {
                assert!(parse_roman(from, &key).is_some(), "{from}");
                for (to, ..) in *list {
                    assert!(parse_roman(to, &key).is_some(), "{from} → {to}");
                }
            }
        }
    }

    #[test]
    fn detects_chords_from_notes() {
        let n = |pitch, start, length| TimedPitch { pitch, start, length };
        // C – a/C – F – G7 je zwei Schläge
        let notes = vec![
            n(48, 0.0, 2.0), n(64, 0.0, 2.0), n(67, 0.0, 2.0), n(72, 0.0, 2.0),
            n(48, 2.0, 2.0), n(64, 2.0, 2.0), n(69, 2.0, 2.0), n(72, 2.0, 2.0),
            n(41, 4.0, 2.0), n(65, 4.0, 2.0), n(69, 4.0, 2.0), n(72, 4.0, 2.0),
            n(43, 6.0, 2.0), n(65, 6.0, 2.0), n(71, 6.0, 2.0), n(74, 6.0, 2.0),
        ];
        let k = c();
        let got: Vec<String> = detect_chords(&notes, 2.0, Some(&k)).iter().map(|d| chord_name(&d.chord, &k)).collect();
        assert_eq!(got, ["C", "a/C", "F", "G7"]);
        // Arpeggio im Takt wird ein Akkord
        let arp: Vec<TimedPitch> = [45, 52, 57, 60, 64, 60].iter().enumerate().map(|(i, &p)| n(p, i as f64 * 0.5, 0.5)).collect();
        let got = detect_chords(&arp, 4.0, Some(&a()));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].chord, Chord::new(9, Minor));
        // einstimmig: kein Akkord
        assert!(detect_chords(&[n(60, 0.0, 1.0)], 2.0, None).is_empty());
    }

    #[test]
    fn voice_leading_is_smooth_and_avoids_parallels() {
        let k = c();
        let prog: Vec<Chord> = ["I", "IV", "V7", "I", "ii6", "I64", "V7", "vi"].iter().map(|s| parse_roman(s, &k).unwrap()).collect();
        let v = voice_progression(&prog, Some(&k), None);
        for (ch, voices) in prog.iter().zip(&v) {
            assert_eq!(voices.len(), 4);
            assert!(voices.windows(2).all(|w| w[0] < w[1]), "{voices:?}");
            assert_eq!(voices[0] % 12, ch.bass_pc(), "Bass von {ch:?}");
            for m in voices {
                assert!(ch.pcs().contains(&(m % 12)));
            }
        }
        for w in v.windows(2) {
            let (p, q) = (&w[0], &w[1]);
            for i in 1..4 {
                assert!((q[i] as i32 - p[i] as i32).abs() <= 7, "Sprung {p:?} → {q:?}");
            }
            for i in 0..4 {
                for j in i + 1..4 {
                    let a = (p[j] as i32 - p[i] as i32).rem_euclid(12);
                    let b = (q[j] as i32 - q[i] as i32).rem_euclid(12);
                    let moved = q[i] != p[i] && (q[i] as i32 - p[i] as i32).signum() == (q[j] as i32 - p[j] as i32).signum();
                    assert!(!(moved && a == b && (a == 7 || a == 0)), "Parallele {p:?} → {q:?}");
                }
            }
        }
        // Leitton steigt in V7 → I im Sopran, falls er oben liegt
        let v = voice_progression(&[Chord::new(7, Dom7), Chord::new(0, Major)], Some(&k), None);
        if v[0][3] % 12 == 11 {
            assert_eq!(v[1][3], v[0][3] + 1);
        }
    }

    #[test]
    fn transitions_have_moods() {
        let k = c();
        let m = |a: &str, b: &str| transition_mood(Some(&parse_chord(a, &k).unwrap()), &parse_chord(b, &k).unwrap(), &k);
        assert_eq!(m("G7", "C"), Mood::Loesend);
        assert_eq!(m("G", "a"), Mood::Ueberraschend, "Trugschluss");
        assert_eq!(m("C", "G"), Mood::Spannend);
        assert_eq!(m("d", "G7"), Mood::Spannend);
        assert_eq!(m("C", "a"), Mood::Dunkel);
        assert_eq!(m("a", "F"), Mood::Hell);
        assert_eq!(m("F", "C"), Mood::Loesend, "Plagalschluss");
        assert_eq!(m("C", "As"), Mood::Ueberraschend, "chromatische Terz");
        assert_eq!(m("F", "f"), Mood::Dunkel, "Moll-Subdominante");
        assert_eq!(m("F", "C/G"), Mood::Spannend, "Quartsextvorhalt");
        assert_eq!(m("E7", "a"), Mood::Loesend, "Zwischendominante löst sich");
        assert_eq!(m("h°", "C"), Mood::Loesend);
        let s = suggest(&k, &[Chord::new(7, Dom7)], 5);
        assert_eq!(s[0].mood, Mood::Loesend);
        assert!(cadences(&k).iter().all(|t| t.moods.len() + 1 == t.steps.len()));
    }

    #[test]
    fn finds_multi_step_paths_to_cadences() {
        let k = c();
        let paths = cadence_paths(&k, &[Chord::new(0, Major)], 4, &|_, _, w| w);
        let goals: Vec<&str> = paths.iter().map(|p| p.goal).collect();
        for g in ["ganzschluss", "plagalschluss", "halbschluss", "trugschluss"] {
            assert!(goals.contains(&g), "{g} fehlt: {goals:?}");
        }
        for p in &paths {
            assert_eq!(p.steps.len(), p.moods.len());
            assert!(p.steps.len() <= 4);
            let names: Vec<&str> = p.steps.iter().map(|s| s.name.as_str()).collect();
            match p.goal {
                "ganzschluss" | "plagalschluss" => assert_eq!(*names.last().unwrap(), "C", "{names:?}"),
                "halbschluss" => assert_eq!(*names.last().unwrap(), "G", "{names:?}"),
                "trugschluss" => assert_eq!(*names.last().unwrap(), "a", "{names:?}"),
                _ => {}
            }
        }
        // längere Varianten stehen direkt hinter dem besten Weg derselben Schlussart
        for w in paths.windows(2) {
            if w[1].longer {
                assert_eq!(w[0].goal, w[1].goal);
                assert!(w[1].steps.len() > w[0].steps.len());
            }
        }
        assert!(paths.iter().any(|p| p.longer && p.steps.len() >= 3), "keine längere Variante");
        let ganz = paths.iter().find(|p| p.goal == "ganzschluss").unwrap();
        assert!(!ganz.longer);
        assert!(ganz.steps.len() >= 2, "von der Tonika aus braucht der Ganzschluss Schritte: {:?}", ganz.steps.iter().map(|s| &s.name).collect::<Vec<_>>());
        assert_eq!(*ganz.moods.last().unwrap(), Mood::Loesend);
        // von G7 aus genügt ein Schritt
        let from_d = cadence_paths(&k, &[Chord::new(0, Major), Chord::new(7, Dom7)], 4, &|_, _, w| w);
        let g = from_d.iter().find(|p| p.goal == "ganzschluss").unwrap();
        assert_eq!(g.steps.len(), 1);
        // Moll: phrygischer Halbschluss über iv⁶
        let am = a();
        let p = cadence_paths(&am, &[Chord::new(9, Minor)], 4, &|_, _, w| w);
        let phr = p.iter().find(|x| x.goal == "phrygisch").expect("phrygisch");
        assert_eq!(phr.steps.last().unwrap().name, "E");
        // Gewichtung wirkt: bevorzugt man ii, kommt sie im Ganzschluss vor
        let favor = cadence_paths(&k, &[Chord::new(0, Major)], 4, &|_, n, w| if n.root == 2 && n.quality.triad() == Minor { w * 5.0 } else { w });
        let gz = favor.iter().find(|x| x.goal == "ganzschluss").unwrap();
        assert!(gz.steps.iter().any(|s| s.chord.root == 2), "{:?}", gz.steps.iter().map(|s| &s.name).collect::<Vec<_>>());
        // unterwegs nie zurück zum Ausgangsakkord (a → G → a → F → C), als Ziel
        // aber erlaubt: Trugschluss vi – IV – V – vi
        let from_a = cadence_paths(&k, &[Chord::new(0, Major), Chord::new(9, Minor)], 4, &|_, _, w| w);
        for p in &from_a {
            let inner = &p.steps[..p.steps.len() - 1];
            assert!(inner.iter().all(|s| s.chord != Chord::new(9, Minor)), "{}: {:?}", p.label, p.steps.iter().map(|s| &s.name).collect::<Vec<_>>());
        }
        let trug = from_a.iter().find(|p| p.goal == "trugschluss").expect("Trugschluss von a aus");
        assert_eq!(trug.steps.last().unwrap().name, "a");
        assert!(trug.steps.len() >= 2);
    }

    #[test]
    fn circle_has_36_named_cells() {
        let cells = circle(&c());
        assert_eq!(cells.len(), 36);
        assert_eq!(cells[0].info.name, "C");
        assert_eq!(cells[1].info.name, "G");
        assert_eq!(cells[12].info.name, "a");
        assert_eq!(cells[24].info.name, "h°");
        assert_eq!(circle_pos(&Chord::new(7, Dom7)), Some((0, 1)));
        assert_eq!(circle_pos(&Chord::new(4, Minor)), Some((1, 1)));
        assert_eq!(circle_pos(&Chord::new(8, Dim7)), Some((2, 3)));
    }
}
