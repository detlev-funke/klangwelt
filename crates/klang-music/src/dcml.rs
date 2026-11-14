//! DCML corpora (Digital and Cognitive Musicology Lab, EPFL Lausanne):
//! piano sonatas by Mozart and Beethoven, Beethoven's string quartets,
//! Bach's suites and solo works – with a chord analysis by experts for
//! every bar.
//!
//! What is read are the tables that the lab itself generates from its MuseScore
//! files (`ms3`), instead of the much larger `.mscx` files:
//!
//! * `notes/<piece>.notes.tsv` – every note with onset and duration in quarters,
//!   staff, voice, MIDI pitch and tie,
//! * `harmonies/<piece>.harmonies.tsv` – the analysis (scale degree, key,
//!   chord tones in fifths relative to the local tonic),
//! * `metadata.tsv` – title, composer, key signature, time signature, instrumentation.
//!
//! The harmonies end up in [`Score::harmony`] and replace the automatic chord
//! detection when indexing.

use crate::harmony::{parse_roman, Chord, Quality};
use crate::score::{LabeledChord, Part, Score, ScoreNote, Voice};
use crate::theory::Key;
use crate::MusicError;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Table with a header row: column name → value per row.
struct Table {
    cols: HashMap<String, usize>,
    rows: Vec<Vec<String>>,
}

impl Table {
    fn parse(text: &str) -> Table {
        let mut lines = text.lines();
        let cols = lines
            .next()
            .unwrap_or("")
            .trim_start_matches('\u{feff}')
            .split('\t')
            .enumerate()
            .map(|(i, n)| (n.trim().to_string(), i))
            .collect();
        let rows = lines.filter(|l| !l.trim().is_empty()).map(|l| l.trim_end_matches('\r').split('\t').map(str::to_string).collect()).collect();
        Table { cols, rows }
    }

    fn get<'a>(&self, row: &'a [String], col: &str) -> &'a str {
        self.cols.get(col).and_then(|&i| row.get(i)).map(|s| s.trim()).unwrap_or("")
    }
}

/// "17/2", "8.5" or "3" → number.
fn number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    match s.split_once('/') {
        Some((a, b)) => {
            let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            (b != 0.0).then(|| a / b)
        }
        None => s.parse().ok(),
    }
}

/// First value from "1: 4/4, 33: 3/4".
fn first_value(s: &str) -> &str {
    let first = s.split(',').next().unwrap_or("");
    first.split_once(':').map(|(_, v)| v).unwrap_or(first).trim()
}

/// Pitch class from a note name like "F#", "Bb", "eb", "a".
fn pitch_class(name: &str) -> Option<u8> {
    let mut ch = name.chars();
    let base: i32 = match ch.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let alt: i32 = ch.map(|c| match c {
        '#' => 1,
        'b' => -1,
        _ => 0,
    })
    .sum();
    Some((base + alt).rem_euclid(12) as u8)
}

fn quality(chord_type: &str) -> Option<Quality> {
    Some(match chord_type {
        "M" => Quality::Major,
        "m" => Quality::Minor,
        "o" => Quality::Dim,
        "+" | "+7" | "+M7" => Quality::Aug,
        "Mm7" => Quality::Dom7,
        "mm7" => Quality::Min7,
        "MM7" => Quality::Maj7,
        "mM7" => Quality::Minor,
        "o7" => Quality::Dim7,
        "%7" => Quality::HalfDim7,
        // augmented sixth chords: sound like a dominant seventh chord on the bass note
        "It" | "Ger" | "Fr" => Quality::Dom7,
        _ => return None,
    })
}

/// Chord from a row of the harmony table (None: only a phrase/key marker).
fn labeled(t: &Table, row: &[String], global: &Key) -> Option<LabeledChord> {
    let onset = number(t.get(row, "quarterbeats"))?;
    let duration = number(t.get(row, "duration_qb")).unwrap_or(0.0);
    let chord_type = t.get(row, "chord_type");
    let q = quality(chord_type)?;
    // Local key as a scale degree of the main key ("V", "vi", "bVI" …)
    let local = t.get(row, "localkey");
    let local_tonic = if local.is_empty() || local == "I" || local == "i" {
        global.tonic
    } else {
        parse_roman(local, global)?.root
    };
    let pc = |field: &str| -> Option<u8> {
        let f: i32 = t.get(row, field).parse().ok()?;
        Some((local_tonic as i32 + 7 * f).rem_euclid(12) as u8)
    };
    let bass = pc("bass_note");
    let root = if matches!(chord_type, "It" | "Ger" | "Fr") { bass? } else { pc("root")? };
    let mut chord = Chord::new(root, q);
    if let Some(b) = bass {
        if b != root && chord.pcs().contains(&b) {
            chord.bass = Some(b);
        }
    }
    Some(LabeledChord { onset, duration, chord, label: t.get(row, "label").to_string() })
}

/// Instrument name from the metadata in German; None for placeholders
/// ("unbenannt2", "MusicXML Default 1").
fn instrument_de(raw: &str) -> Option<String> {
    let l = raw.to_lowercase().replace("(2)", "");
    let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
    let has = |w: &str| l.contains(w);
    Some(if has("klavier") || has("piano") || has("keyboard") {
        "Klavier".into()
    } else if has("harpsichord") || has("cembalo") {
        "Cembalo".into()
    } else if has("organ") || has("orgel") {
        "Orgel".into()
    } else if has("cello") {
        "Violoncello".into()
    } else if has("viola") {
        "Viola".into()
    } else if has("violin") || has("violn") {
        match l.rsplit(' ').next() {
            Some("ii" | "2") => "Violine 2".into(),
            Some("i" | "1") => "Violine 1".into(),
            _ => "Violine".into(),
        }
    } else if has("flute") || has("flöte") {
        "Flöte".into()
    } else if l.is_empty() || has("unbenannt") || has("default") || has("untitled") || has("midi") {
        return None;
    } else {
        raw.trim().to_string()
    })
}

fn is_keyboard(name: &str) -> bool {
    matches!(name, "Klavier" | "Cembalo" | "Orgel")
}

/// Reads `…/notes/<piece>.notes.tsv` together with the harmonies and metadata next to it.
pub fn load_dcml(notes_path: &Path) -> Result<Score, MusicError> {
    let file = notes_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let piece = &file[..file.len().saturating_sub(".notes.tsv".len())];
    let notes = Table::parse(&std::fs::read_to_string(notes_path)?);
    let base = notes_path.parent().and_then(Path::parent);

    let mut score = Score::default();
    // Metadata
    let mut instruments: BTreeMap<u32, String> = BTreeMap::new();
    let mut annotated_key = String::new();
    if let Some(meta) = base.and_then(|b| std::fs::read_to_string(b.join("metadata.tsv")).ok()) {
        let meta = Table::parse(&meta);
        // File names are simplified when downloading ("001 Aus meines …" → "001_Aus_meines_…")
        let same = |p: &str| p == piece || crate::fetch::ascii_path(p).to_string_lossy() == piece;
        if let Some(row) = meta.rows.iter().find(|r| same(meta.get(r, "piece"))) {
            let work = meta.get(row, "workTitle");
            let mv_no = meta.get(row, "movementNumber");
            let mv = meta.get(row, "movementTitle");
            let catalog = meta.get(row, "workNumber");
            let mut title = match (work.is_empty(), mv.is_empty()) {
                (false, false) if !mv_no.is_empty() => format!("{work} – {mv_no}. {mv}"),
                (false, false) => format!("{work} – {mv}"),
                (false, true) => work.to_string(),
                (true, false) => mv.to_string(),
                (true, true) => meta.get(row, "piece").to_string(),
            };
            if !catalog.is_empty() && !title.contains(catalog) {
                title = format!("{title} ({catalog})");
            }
            score.title = Some(title);
            let composer = meta.get(row, "composer");
            if !composer.is_empty() {
                score.composer = Some(composer.to_string());
            }
            score.key_fifths = first_value(meta.get(row, "KeySig")).parse().ok();
            if let Some((n, d)) = first_value(meta.get(row, "TimeSig")).split_once('/') {
                if let (Ok(n), Ok(d)) = (n.trim().parse(), d.trim().parse()) {
                    score.time_signature = Some((n, d));
                }
            }
            annotated_key = meta.get(row, "annotated_key").to_string();
            for (name, &i) in &meta.cols {
                if let Some(n) = name.strip_prefix("staff_").and_then(|s| s.strip_suffix("_instrument")).and_then(|s| s.parse::<u32>().ok()) {
                    if let Some(v) = row.get(i).and_then(|s| instrument_de(s)) {
                        instruments.insert(n, v);
                    }
                }
            }
        }
    }
    if score.title.is_none() {
        score.title = Some(piece.replace('_', " "));
    }

    // Notes (ties: 1 = start, 0 = middle, −1 = end)
    let mut voices: BTreeMap<(u32, u32), Vec<ScoreNote>> = BTreeMap::new();
    let mut open: HashMap<(u32, u32, u8), usize> = HashMap::new();
    let mut rows: Vec<(f64, f64, u32, u32, u8, i32)> = Vec::new();
    for r in &notes.rows {
        if !notes.get(r, "gracenote").is_empty() {
            continue;
        }
        let (Some(onset), Some(dur)) = (number(notes.get(r, "quarterbeats")), number(notes.get(r, "duration_qb"))) else { continue };
        let Ok(midi) = notes.get(r, "midi").parse::<u8>() else { continue };
        let staff = notes.get(r, "staff").parse().unwrap_or(1);
        let voice = notes.get(r, "voice").parse().unwrap_or(1);
        let tied = notes.get(r, "tied").parse::<i32>().unwrap_or(i32::MIN);
        rows.push((onset, dur, staff, voice, midi, tied));
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (onset, dur, staff, voice, midi, tied) in rows {
        let key = (staff, voice, midi);
        let list = voices.entry((staff, voice)).or_default();
        match (tied, open.get(&key).copied()) {
            (0 | -1, Some(idx)) if idx < list.len() => {
                let n = &mut list[idx];
                n.duration = (onset + dur - n.onset).max(n.duration);
                if tied == -1 {
                    open.remove(&key);
                }
            }
            _ => {
                list.push(ScoreNote { onset, duration: dur, midi });
                if tied == 1 {
                    open.insert(key, list.len() - 1);
                }
            }
        }
    }

    // Parts: staves of a keyboard instrument belong together; identical
    // instruments next to each other are numbered (Violin 1, Violin 2).
    let staffs: Vec<u32> = {
        let mut s: Vec<u32> = voices.keys().map(|(s, _)| *s).collect();
        s.dedup();
        s
    };
    // Staves without a usable name: four = string quartet, otherwise piano
    // (if another staff is piano or there are at most two).
    let keyboard_named = instruments.values().any(|n| is_keyboard(n));
    let n_staffs = staffs.len();
    let name_of = |s: u32| {
        instruments.get(&s).cloned().unwrap_or_else(|| match (n_staffs, s) {
            (4, 1) => "Violine 1".into(),
            (4, 2) => "Violine 2".into(),
            (4, 3) => "Viola".into(),
            (4, 4) => "Violoncello".into(),
            _ if keyboard_named || n_staffs <= 2 => "Klavier".into(),
            _ => format!("System {s}"),
        })
    };
    let mut groups: Vec<(String, Vec<u32>)> = Vec::new();
    for s in staffs {
        let name = name_of(s);
        match groups.last_mut() {
            Some((n, members)) if *n == name && is_keyboard(&name) => members.push(s),
            _ => groups.push((name, vec![s])),
        }
    }
    let mut count: HashMap<String, usize> = HashMap::new();
    for (n, _) in &groups {
        *count.entry(n.clone()).or_default() += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (gi, (name, members)) in groups.into_iter().enumerate() {
        let label = if count[&name] > 1 {
            let k = seen.entry(name.clone()).or_default();
            *k += 1;
            format!("{name} {k}")
        } else {
            name
        };
        let vs: Vec<Voice> = members
            .iter()
            .enumerate()
            .flat_map(|(i, &s)| {
                let staff_no = i as u32 + 1;
                voices
                    .iter()
                    .filter(move |((st, _), _)| *st == s)
                    .map(move |((_, v), notes)| {
                        let mut notes = notes.clone();
                        notes.sort_by(|a, b| a.onset.total_cmp(&b.onset).then(b.midi.cmp(&a.midi)));
                        Voice { staff: staff_no, voice: *v, notes }
                    })
            })
            .filter(|v| !v.notes.is_empty())
            .collect();
        if !vs.is_empty() {
            score.parts.push(Part { id: format!("S{gi}"), name: label, voices: vs });
        }
    }
    if score.parts.is_empty() {
        return Err(MusicError::Unsupported(klang_i18n::tr!("{piece}: no notes", piece = piece)));
    }

    // Harmonies
    if let Some(text) = base.and_then(|b| std::fs::read_to_string(b.join("harmonies").join(format!("{piece}.harmonies.tsv"))).ok()) {
        let h = Table::parse(&text);
        let global_name = h.rows.iter().map(|r| h.get(r, "globalkey")).find(|g| !g.is_empty()).map(str::to_string).unwrap_or(annotated_key.clone());
        if let Some(tonic) = pitch_class(&global_name) {
            let minor = global_name.chars().next().is_some_and(|c| c.is_lowercase());
            let global = Key { tonic, minor, confidence: 1.0 };
            score.key_mode = Some(if minor { "minor" } else { "major" }.into());
            score.harmony = h.rows.iter().filter_map(|r| labeled(&h, r, &global)).collect();
            score.harmony.sort_by(|a, b| a.onset.total_cmp(&b.onset));
        }
    } else if let Some(c) = annotated_key.chars().next() {
        score.key_mode = Some(if c.is_lowercase() { "minor" } else { "major" }.into());
    }
    Ok(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_notes_ties_metadata_and_expert_chords() {
        let root = std::env::temp_dir().join(format!("klang-dcml-{}", std::process::id()));
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::create_dir_all(root.join("harmonies")).unwrap();
        std::fs::write(
            root.join("metadata.tsv"),
            "piece\tTimeSig\tKeySig\tannotated_key\tcomposer\tworkTitle\tmovementNumber\tmovementTitle\tworkNumber\tstaff_1_instrument\tstaff_2_instrument\n\
             K279-1\t1: 4/4\t1: 0\tC\tWolfgang Amadeus Mozart\tPiano Sonata no. 1 in C major\t1\tAllegro\tK279\tPiano\tPiano\n",
        )
        .unwrap();
        std::fs::write(
            root.join("notes/K279-1.notes.tsv"),
            "mc\tquarterbeats\tduration_qb\tstaff\tvoice\tgracenote\ttied\tmidi\n\
             1\t0\t1.0\t1\t1\t\t\t64\n\
             1\t0\t4.0\t2\t1\t\t\t48\n\
             1\t1\t1.0\t1\t1\t\t1\t67\n\
             1\t3/2\t0.25\t1\t1\tacciaccatura\t\t69\n\
             1\t2\t2.0\t1\t1\t\t-1\t67\n\
             2\t4\t1.0\t1\t1\t\t\t65\n",
        )
        .unwrap();
        std::fs::write(
            root.join("harmonies/K279-1.harmonies.tsv"),
            "quarterbeats\tduration_qb\tlabel\tglobalkey\tlocalkey\tchord_type\tlocalkey_is_minor\troot\tbass_note\n\
             0\t2.0\tC.I{\tC\tI\tM\t0\t0\t0\n\
             2\t1.0\tV65/V\tC\tI\tMm7\t0\t2\t6\n\
             3\t1.0\t}\tC\tI\t\t0\t\t\n\
             4\t2.0\tV.vi\tC\tV\tm\t0\t3\t3\n",
        )
        .unwrap();
        let s = crate::score::load_score(&root.join("notes/K279-1.notes.tsv")).unwrap();
        assert_eq!(s.title.as_deref(), Some("Piano Sonata no. 1 in C major – 1. Allegro (K279)"));
        assert_eq!(s.composer.as_deref(), Some("Wolfgang Amadeus Mozart"));
        assert_eq!((s.key_fifths, s.key_mode.as_deref(), s.time_signature), (Some(0), Some("major"), Some((4, 4))));
        assert_eq!(s.parts.len(), 1, "beide Klaviersysteme in einem Teil");
        assert_eq!(s.parts[0].name, "Klavier");
        let rh: Vec<(f64, f64, u8)> = s.parts[0].voices[0].notes.iter().map(|n| (n.onset, n.duration, n.midi)).collect();
        assert_eq!(rh, vec![(0.0, 1.0, 64), (1.0, 3.0, 67), (4.0, 1.0, 65)], "Haltebogen verbunden, Vorschlag weg");
        assert_eq!(s.harmony.len(), 3, "Phrasenmarke ohne Akkord fällt weg");
        assert_eq!(s.harmony[0].chord, Chord::new(0, Quality::Major));
        // V65/V in C: D major seventh chord with F# in the bass
        assert_eq!(s.harmony[1].chord, Chord::new(2, Quality::Dom7).inverted(1));
        assert_eq!(s.harmony[1].label, "V65/V");
        // vi in G major (local key V): E minor
        assert_eq!(s.harmony[2].chord, Chord::new(4, Quality::Minor));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn helpers() {
        assert_eq!(number("17/2"), Some(8.5));
        assert_eq!(number(""), None);
        assert_eq!(first_value("1: 4/4, 33: 3/4"), "4/4");
        assert_eq!(pitch_class("F#"), Some(6));
        assert_eq!(pitch_class("eb"), Some(3));
        assert_eq!(pitch_class("Bb"), Some(10));
    }
}
