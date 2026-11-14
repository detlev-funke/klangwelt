//! Humdrum-`**kern`-Leser (KernScores/CCARH: Bach-Choräle, Wohltemperiertes
//! Klavier, Klaviersonaten von Mozart und Beethoven …).
//!
//! `**kern` ist ein Textformat: Zeilen sind Zeitpunkte, Tabulator-getrennte
//! Spalten („Spines“) sind Stimmen bzw. Systeme. Gelesen wird, was das
//! Partiturmodell braucht: Noten, Akkorde, Haltebögen, Spaltenteilung (`*^`,
//! `*v`, `*x`, `*-`, `*+`), Vorzeichen (`*k[…]`), Tonart (`*G:` / `*e:`),
//! Taktart (`*M3/4`), Tempo (`*MM100`), Stimmnamen (`*I"Sopran`) und die
//! Kopfzeilen (`!!!OTL:` Titel, `!!!COM:` Komponist, `!!!SCT:` Werkverzeichnis).
//!
//! Jede Spalte hat ihre eigene Uhr: Eine Note beginnt, wo die letzte Note
//! derselben Spalte aufgehört hat, und `.` (Nullmarke) rückt nichts vor.
//! Wiederholungsanweisungen (`*>[A,A,B]`) bleiben unbeachtet – jeder
//! Abschnitt zählt einmal. Vorschlagsnoten und unsichtbare Noten fallen weg,
//! ebenso Alternativfassungen (`*S/ossia`).

use crate::score::{Part, Score, ScoreNote, Voice};
use crate::MusicError;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug)]
struct Column {
    /// Index der ursprünglichen `**kern`-Spalte, None = andere Darstellung (`**dynam` …).
    spine: Option<usize>,
    /// Uhr dieser Spalte in Viertelnoten.
    time: f64,
    /// Stimme innerhalb der Spalte (nach `*^` zählt die rechte Hälfte weiter).
    voice: u32,
    /// Alternativfassung (`*S/ossia`) – Noten zählen nicht.
    ignore: bool,
}

#[derive(Default)]
struct Spine {
    name: Option<String>,
    instrument: Option<String>,
    part_tag: Option<String>,
    /// System aus `*staff1` (1 = oben)
    staff: Option<u32>,
    notes: BTreeMap<u32, Vec<ScoreNote>>,
    open_ties: HashMap<(u32, u8), usize>,
}

/// Eine Note (oder Pause) aus einem Datentoken.
#[derive(Debug, PartialEq)]
struct Token {
    /// Dauer in Viertelnoten.
    duration: Option<f64>,
    /// None = Pause.
    midi: Option<u8>,
    tie_start: bool,
    tie_end: bool,
    /// Vorschlagsnote oder unsichtbar – zählt nicht.
    skip: bool,
}

fn parse_token(tok: &str) -> Token {
    let b = tok.as_bytes();
    let mut t = Token { duration: None, midi: None, tie_start: false, tie_end: false, skip: false };
    let mut i = 0;
    // Dauer: Kehrwert (4 = Viertel, 0 = Brevis, 00 = Longa), „3%2“ = rational, Punkte.
    let mut letters: Option<(u8, usize)> = None;
    let mut alter = 0i32;
    let mut rest = false;
    while i < b.len() {
        let c = b[i];
        match c {
            b'0'..=b'9' if t.duration.is_none() => {
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let digits = &tok[start..i];
                let mut recip: f64 = if digits.bytes().all(|d| d == b'0') {
                    0.5f64.powi(digits.len() as i32)
                } else {
                    digits.parse().unwrap_or(4.0)
                };
                if i < b.len() && b[i] == b'%' {
                    let s = i + 1;
                    let mut e = s;
                    while e < b.len() && b[e].is_ascii_digit() {
                        e += 1;
                    }
                    if let Ok(den) = tok[s..e].parse::<f64>() {
                        if den > 0.0 {
                            recip /= den;
                        }
                    }
                    i = e;
                }
                let mut dur = if recip > 0.0 { 4.0 / recip } else { 0.0 };
                let mut add = dur / 2.0;
                while i < b.len() && b[i] == b'.' {
                    dur += add;
                    add /= 2.0;
                    i += 1;
                }
                t.duration = Some(dur);
                continue;
            }
            b'a'..=b'g' | b'A'..=b'G' if letters.is_none() => {
                let mut n = 0;
                while i + n < b.len() && b[i + n] == c {
                    n += 1;
                }
                letters = Some((c, n));
                i += n;
                continue;
            }
            b'#' => alter += 1,
            b'-' => alter -= 1,
            b'r' => rest = true,
            b'[' => t.tie_start = true,
            b']' => t.tie_end = true,
            b'_' => {
                t.tie_start = true;
                t.tie_end = true;
            }
            b'q' | b'Q' | b'y' => t.skip = true,
            _ => {}
        }
        i += 1;
    }
    if !rest {
        if let Some((c, n)) = letters {
            let step: i32 = match c.to_ascii_lowercase() {
                b'c' => 0,
                b'd' => 2,
                b'e' => 4,
                b'f' => 5,
                b'g' => 7,
                b'a' => 9,
                _ => 11,
            };
            let octave = if c.is_ascii_lowercase() { 4 + n as i32 - 1 } else { 3 - (n as i32 - 1) };
            let midi = (octave + 1) * 12 + step + alter;
            if (0..=127).contains(&midi) {
                t.midi = Some(midi as u8);
            }
        }
    }
    t
}

/// Kopfzeilen enthalten mitunter HTML: „W&auml;lder“, „K<sup>1</sup> 279“
/// (Köchel, 1. Auflage → „KV 279“).
fn html_text(s: &str) -> String {
    let mut out = s.trim().to_string();
    if let Some(rest) = out.strip_prefix("K<sup>") {
        if let Some((_, after)) = rest.split_once("</sup>") {
            out = format!("KV {}", after.trim());
        }
    }
    for (from, to) in [
        ("<sup>", ""), ("</sup>", ""), ("<i>", ""), ("</i>", ""),
        ("&auml;", "ä"), ("&ouml;", "ö"), ("&uuml;", "ü"), ("&Auml;", "Ä"), ("&Ouml;", "Ö"), ("&Uuml;", "Ü"), ("&szlig;", "ß"),
        ("&eacute;", "é"), ("&egrave;", "è"), ("&agrave;", "à"), ("&aacute;", "á"), ("&ccedil;", "ç"), ("&ntilde;", "ñ"),
        ("&quot;", "\""), ("&apos;", "'"), ("&lt;", "<"), ("&gt;", ">"), ("&amp;", "&"),
    ] {
        out = out.replace(from, to);
    }
    out
}

/// „ Stephen C. Foster, "Ah! May the red rose live alway!"“ → (Komponist, Titel).
fn comment_title(c: &str) -> (Option<String>, Option<String>) {
    let Some(open) = c.find('"') else { return (None, None) };
    let Some(len) = c[open + 1..].find('"') else { return (None, None) };
    let title = c[open + 1..open + 1 + len].trim().to_string();
    let who = c[..open].trim().trim_end_matches(',').trim().to_string();
    (Some(who).filter(|w| !w.is_empty() && w.len() < 60), Some(title).filter(|t| !t.is_empty()))
}

/// „Bach, Johann Sebastian“ → „Johann Sebastian Bach“.
fn composer_name(raw: &str) -> String {
    match raw.split_once(',') {
        Some((last, first)) if !first.trim().is_empty() && !last.trim().is_empty() => format!("{} {}", first.trim(), last.trim()),
        _ => raw.trim().to_string(),
    }
}

/// Vorzeichen aus `*k[f#c#]` bzw. `*k[b-e-]`.
fn key_fifths(tok: &str) -> Option<i8> {
    let inner = tok.strip_prefix("*k[")?.strip_suffix(']')?;
    let sharps = inner.matches('#').count() as i8;
    let flats = inner.matches('-').count() as i8;
    Some(sharps - flats)
}

/// Tonart `*G:` (Dur) oder `*f#:` (Moll); Kirchentonarten (`*G:dor`) zählen nicht.
fn key_mode(tok: &str) -> Option<&'static str> {
    let body = tok.strip_prefix('*')?.strip_suffix(':')?;
    let mut ch = body.chars();
    let first = ch.next()?;
    if !first.is_ascii_alphabetic() || !"abcdefgABCDEFG".contains(first) || !ch.all(|c| c == '#' || c == '-') {
        return None;
    }
    Some(if first.is_ascii_uppercase() { "major" } else { "minor" })
}

fn instrument_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "piano" | "klav" | "cemba" | "clavi" | "forte" => "Klavier",
        "organ" => "Orgel",
        "soprn" | "sopran" => "Sopran",
        "alto" | "calto" => "Alt",
        "tenor" | "ctenor" => "Tenor",
        "bass" | "basso" => "Bass",
        "vox" => "Singstimme",
        "violn" => "Violine",
        "viola" => "Viola",
        "cello" => "Violoncello",
        "flt" => "Flöte",
        _ => return None,
    })
}

pub fn parse_kern(text: &str) -> Result<Score, MusicError> {
    let mut score = Score::default();
    let mut refs: Vec<(String, String)> = Vec::new();
    let mut cols: Vec<Column> = Vec::new();
    let mut spines: Vec<Spine> = Vec::new();
    let mut started = false;
    let mut first_comment: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if let Some(rec) = line.strip_prefix("!!!") {
            if let Some((k, v)) = rec.split_once(':') {
                refs.push((k.trim().to_string(), v.trim().to_string()));
            }
            continue;
        }
        if let Some(c) = line.strip_prefix("!!") {
            if !started && first_comment.is_none() && c.contains('"') {
                first_comment = Some(c.trim().to_string());
            }
            continue;
        }
        let toks: Vec<&str> = line.split('\t').collect();
        if !started {
            if toks[0].starts_with("**") {
                for t in &toks {
                    let spine = (*t == "**kern").then(|| {
                        spines.push(Spine::default());
                        spines.len() - 1
                    });
                    cols.push(Column { spine, time: 0.0, voice: 1, ignore: false });
                }
                started = true;
            }
            continue;
        }
        if toks.len() != cols.len() {
            // Unstimmige Zeile (oder Ende einer Datei mit mehreren Abschnitten): überspringen.
            continue;
        }
        let first = toks[0];
        if first.starts_with('!') || first.starts_with('=') {
            continue;
        }
        if first.starts_with('*') {
            cols = interpret(&toks, &cols, &mut spines, &mut score);
            if cols.is_empty() {
                // alle Spalten beendet (`*-`): ein weiterer Abschnitt dürfte mit `**` neu beginnen
                started = false;
            }
            continue;
        }
        // Datenzeile
        for (col, tok) in cols.iter_mut().zip(&toks) {
            let Some(si) = col.spine else { continue };
            if *tok == "." {
                continue;
            }
            let parts: Vec<Token> = tok.split(' ').filter(|s| !s.is_empty()).map(parse_token).collect();
            let step = parts.iter().filter(|p| !p.skip).filter_map(|p| p.duration).filter(|d| *d > 0.0).fold(f64::INFINITY, f64::min);
            let onset = col.time;
            if step.is_finite() {
                col.time += step;
            }
            if col.ignore {
                continue;
            }
            let spine = &mut spines[si];
            for p in parts {
                let (Some(midi), Some(dur)) = (p.midi, p.duration) else { continue };
                if p.skip || dur <= 0.0 {
                    continue;
                }
                let key = (col.voice, midi);
                let list = spine.notes.entry(col.voice).or_default();
                match spine.open_ties.get(&key).copied() {
                    Some(idx) if p.tie_end && idx < list.len() => {
                        let n = &mut list[idx];
                        n.duration = (onset + dur - n.onset).max(n.duration);
                        if !p.tie_start {
                            spine.open_ties.remove(&key);
                        }
                    }
                    _ => {
                        list.push(ScoreNote { onset, duration: dur, midi });
                        if p.tie_start {
                            spine.open_ties.insert(key, list.len() - 1);
                        }
                    }
                }
            }
        }
    }
    if spines.is_empty() {
        return Err(MusicError::Unsupported("keine **kern-Spalte".into()));
    }

    let get = |prefix: &str| refs.iter().find(|(k, v)| k.starts_with(prefix) && !v.is_empty()).map(|(_, v)| html_text(v));
    let mut title = get("OTL").or_else(|| get("OPR"));
    // Satzbezeichnung ohne Metronomangabe: „(Allegro) ([quarter]=126)“ → „(Allegro)“
    let movement = get("OMD").map(|m| {
        let mut m = m;
        while let Some(i) = m.find("([") {
            let end = m[i..].find(')').map(|e| i + e + 1).unwrap_or(m.len());
            m.replace_range(i..end, "");
        }
        m.trim().to_string()
    });
    if let (Some(t), Some(mv)) = (title.as_mut(), movement.filter(|m| !m.is_empty())) {
        if !t.contains(mv.trim_end_matches('.')) {
            *t = format!("{t} – {}", mv.trim_end_matches('.'));
        }
    }
    if let (Some(t), Some(cat)) = (title.as_mut(), get("SCT")) {
        *t = format!("{t} ({cat})");
    }
    // Ohne Kopfzeilen: erster Kommentar „Komponist, "Titel"“ (Foster, Gershwin)
    let (c_composer, c_title) = first_comment.as_deref().map(comment_title).unwrap_or((None, None));
    score.title = title.or(c_title);
    score.composer = get("COM").or_else(|| get("COA")).map(|c| composer_name(&c)).or(c_composer);

    // Spalten zu Teilen: gleiche `*part`-Angabe, sonst gleiches Instrument
    // nebeneinander (Klavier: linke und rechte Hand), sonst jede für sich.
    // In **kern steht die tiefste Stimme links – Teile von oben nach unten.
    let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    for (i, s) in spines.iter().enumerate() {
        let key = s.part_tag.clone().or_else(|| s.instrument.clone().filter(|i| instrument_label(i) == Some("Klavier")));
        match groups.last_mut() {
            Some((k, members)) if key.is_some() && *k == key => members.push(i),
            _ => groups.push((key, vec![i])),
        }
    }
    // Zwei Spalten ohne Namen und Instrument auf zwei Systemen: ein Tasteninstrument
    // (Haydn-Sonaten, Inventionen …). Mehrere Fugenstimmen bleiben getrennt.
    let bare = |i: usize| spines[i].name.is_none() && spines[i].instrument.is_none() && spines[i].part_tag.is_none();
    if groups.len() == 2 && spines.len() == 2 && bare(0) && bare(1) {
        if let (Some(a), Some(b)) = (spines[0].staff, spines[1].staff) {
            if a != b {
                groups = vec![(None, vec![0, 1])];
                spines[1].instrument = Some("piano".into());
            }
        }
    }
    let mut unnamed = 0;
    for (gi, (_, members)) in groups.iter().enumerate().rev() {
        let name = members
            .iter()
            .rev()
            .find_map(|&i| spines[i].name.clone())
            .or_else(|| members.iter().find_map(|&i| spines[i].instrument.as_deref().and_then(instrument_label).map(str::to_string)))
            .unwrap_or_else(|| {
                unnamed += 1;
                format!("Stimme {unnamed}")
            });
        let mut voices = Vec::new();
        for (staff, &si) in members.iter().rev().enumerate() {
            for (voice, notes) in std::mem::take(&mut spines[si].notes) {
                let mut notes = notes;
                notes.sort_by(|a, b| a.onset.total_cmp(&b.onset).then(b.midi.cmp(&a.midi)));
                if !notes.is_empty() {
                    voices.push(Voice { staff: staff as u32 + 1, voice, notes });
                }
            }
        }
        if !voices.is_empty() {
            score.parts.push(Part { id: format!("K{gi}"), name, voices });
        }
    }
    if score.parts.is_empty() {
        return Err(MusicError::Unsupported("keine Noten in der **kern-Datei".into()));
    }
    Ok(score)
}

/// Eine Interpretationszeile: Angaben übernehmen und Spalten umbauen.
fn interpret(toks: &[&str], cols: &[Column], spines: &mut Vec<Spine>, score: &mut Score) -> Vec<Column> {
    // Alternativfassungen: je Spalte die erste *S/-Marke einer Zeile gilt, die anderen nicht.
    let mut first_label: HashMap<usize, String> = HashMap::new();
    let mut out: Vec<Column> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let tok = toks[i];
        let mut col = cols[i].clone();
        match tok {
            "*^" => {
                let next = cols.iter().filter(|c| c.spine == col.spine).map(|c| c.voice).max().unwrap_or(col.voice) + 1;
                out.push(col.clone());
                out.push(Column { voice: next, ..col });
                i += 1;
                continue;
            }
            "*v" => {
                let mut j = i;
                while j + 1 < toks.len() && toks[j + 1] == "*v" && cols[j + 1].spine == col.spine {
                    j += 1;
                }
                for c in &cols[i..=j] {
                    col.time = col.time.max(c.time);
                    col.voice = col.voice.min(c.voice);
                    col.ignore &= c.ignore;
                }
                out.push(col);
                i = j + 1;
                continue;
            }
            "*-" => {
                i += 1;
                continue;
            }
            "*+" => {
                out.push(col);
                out.push(Column { spine: None, time: 0.0, voice: 1, ignore: false });
                i += 1;
                continue;
            }
            "*x" if i + 1 < toks.len() && toks[i + 1] == "*x" => {
                out.push(cols[i + 1].clone());
                out.push(col);
                i += 2;
                continue;
            }
            _ => {}
        }
        if tok == "**kern" && col.spine.is_none() {
            spines.push(Spine::default());
            col.spine = Some(spines.len() - 1);
        }
        if let Some(si) = col.spine {
            let spine = &mut spines[si];
            if let Some(label) = tok.strip_prefix("*S/") {
                if label == "fin" {
                    col.ignore = false;
                } else {
                    let first = first_label.entry(si).or_insert_with(|| label.to_string());
                    col.ignore = first != label || label == "ossia";
                }
            } else if tok == "*S-" {
                col.ignore = false;
            } else if let Some(name) = tok.strip_prefix("*I\"") {
                if !name.trim().is_empty() {
                    spine.name = Some(name.trim().to_string());
                }
            } else if let Some(code) = tok.strip_prefix("*I") {
                if !code.starts_with(['C', 'G', 'T', '\'']) && !code.is_empty() {
                    spine.instrument = Some(code.to_string());
                }
            } else if let Some(n) = tok.strip_prefix("*staff").and_then(|n| n.parse().ok()) {
                spine.staff = Some(n);
            } else if let Some(p) = tok.strip_prefix("*part") {
                spine.part_tag = Some(p.to_string());
            } else if let Some(f) = key_fifths(tok) {
                score.key_fifths.get_or_insert(f);
            } else if let Some(m) = key_mode(tok) {
                if score.key_mode.is_none() {
                    score.key_mode = Some(m.to_string());
                }
            } else if let Some(mm) = tok.strip_prefix("*MM") {
                if score.tempo_bpm.is_none() {
                    score.tempo_bpm = mm.trim_start_matches('[').split(|c: char| !c.is_ascii_digit() && c != '.').next().and_then(|v| v.parse().ok());
                }
            } else if let Some(ts) = tok.strip_prefix("*M") {
                if score.time_signature.is_none() {
                    if let Some((n, d)) = ts.split_once('/') {
                        if let (Ok(n), Ok(d)) = (n.parse::<u32>(), d.parse::<u32>()) {
                            score.time_signature = Some((n, d));
                        }
                    }
                }
            }
        }
        out.push(col);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let t = parse_token("4cc#");
        assert_eq!((t.duration, t.midi), (Some(1.0), Some(73)));
        let t = parse_token("8.GG-");
        assert_eq!((t.duration, t.midi), (Some(0.75), Some(42)));
        assert_eq!(parse_token("2r").midi, None);
        assert_eq!(parse_token("0C").duration, Some(8.0));
        assert_eq!(parse_token("3%2e").duration, Some(4.0 * 2.0 / 3.0));
        assert!(parse_token("[4c").tie_start && parse_token("4c]").tie_end);
        assert!(parse_token("8qd").skip);
        assert_eq!(html_text("Nun ruhen alle W&auml;lder"), "Nun ruhen alle Wälder");
        assert_eq!(html_text("K<sup>1</sup> 279"), "KV 279");
        assert_eq!(comment_title(" Stephen C. Foster, \"Ah! May the red rose live alway!\""), (Some("Stephen C. Foster".into()), Some("Ah! May the red rose live alway!".into())));
        assert_eq!(parse_token("(16CLL").midi, Some(48));
        assert_eq!(parse_token("4.e-;").midi, Some(63));
    }

    const CHORAL: &str = "!!!COM: Bach, Johann Sebastian\n\
!!!OTL@@DE: Aus meines Herzens Grunde\n\
!!!SCT: BWV 269\n\
**kern\t**kern\t**dynam\n\
*I\"Bass\t*I\"Soprano\t*\n\
*k[f#]\t*k[f#]\t*\n\
*G:\t*G:\t*\n\
*M3/4\t*M3/4\t*\n\
*MM100\t*MM100\t*\n\
4GG\t4g\tp\n\
=1\t=1\t=1\n\
4G\t[2g\t.\n\
4E\t.\t.\n\
*\t*^\t*\n\
4F#\t4g]\t4dd\t.\n\
*\t*v\t*v\t*\n\
2G;\t2g;\t.\n\
*-\t*-\t*-\n";

    #[test]
    fn reads_a_chorale_with_split_and_tie() {
        let s = parse_kern(CHORAL).unwrap();
        assert_eq!(s.title.as_deref(), Some("Aus meines Herzens Grunde (BWV 269)"));
        assert_eq!(s.composer.as_deref(), Some("Johann Sebastian Bach"));
        assert_eq!((s.key_fifths, s.key_mode.as_deref(), s.time_signature, s.tempo_bpm), (Some(1), Some("major"), Some((3, 4)), Some(100.0)));
        assert_eq!(s.parts.len(), 2);
        assert_eq!(s.parts[0].name, "Soprano", "oberste Stimme zuerst");
        assert_eq!(s.parts[1].name, "Bass");
        let sop = &s.parts[0].voices;
        assert_eq!(sop.len(), 2, "geteilte Spalte: zweite Stimme");
        let v1: Vec<(f64, f64, u8)> = sop[0].notes.iter().map(|n| (n.onset, n.duration, n.midi)).collect();
        assert_eq!(v1, vec![(0.0, 1.0, 67), (1.0, 3.0, 67), (4.0, 2.0, 67)], "Haltebogen über die Teilung");
        assert_eq!(sop[1].notes.iter().map(|n| (n.onset, n.midi)).collect::<Vec<_>>(), vec![(3.0, 74)]);
        let bass: Vec<(f64, u8)> = s.parts[1].voices[0].notes.iter().map(|n| (n.onset, n.midi)).collect();
        assert_eq!(bass, vec![(0.0, 43), (1.0, 55), (2.0, 52), (3.0, 54), (4.0, 55)]);
    }

    #[test]
    fn piano_hands_form_one_part_and_ossia_is_skipped() {
        let k = "!!!COM: Beethoven, Ludwig van\n!!!OTL: Piano Sonata no. 1 in F minor\n!!!OMD: Allegro\n\
**kern\t**kern\n*staff2\t*staff1\n*Ipiano\t*Ipiano\n*k[b-e-a-d-]\t*k[b-e-a-d-]\n*f:\t*f:\n\
4F\t4c\n\
*\t*^\n\
*\t*S/sic\t*S/ossia\n\
4G\t4d\t4ee\n\
*\t*S/fin\t*S/fin\n\
*\t*v\t*v\n\
4A-\t4e-\n*-\t*-\n";
        let s = parse_kern(k).unwrap();
        assert_eq!(s.title.as_deref(), Some("Piano Sonata no. 1 in F minor – Allegro"));
        assert_eq!((s.key_fifths, s.key_mode.as_deref()), (Some(-4), Some("minor")));
        assert_eq!(s.parts.len(), 1);
        assert_eq!(s.parts[0].name, "Klavier");
        let staffs: Vec<u32> = s.parts[0].voices.iter().map(|v| v.staff).collect();
        assert_eq!(staffs, vec![1, 2], "rechte Hand oben");
        let rh: Vec<u8> = s.parts[0].voices[0].notes.iter().map(|n| n.midi).collect();
        assert_eq!(rh, vec![60, 62, 63], "Ossia (ee) fällt weg");
        assert_eq!(s.note_count(), 6);
    }
}
