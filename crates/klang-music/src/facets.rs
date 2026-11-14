//! Merkmale je Partitur zum Stöbern: Stimmung, Färbung, Energie, Tonart,
//! Takt, Tempo, Besetzung, Instrumentengruppen, Satzweise, Harmonik, Länge,
//! Komponist. Daraus baut [`tree_children`] einen Baum mit frei wählbarer
//! Reihenfolge der Ebenen; mehrwertige Merkmale (Instrumentengruppen) legen
//! eine Partitur in mehrere Zweige.

use crate::analysis::{analyze_score_with, ScoreAnalysis, VoiceRole};
use crate::pdmx::{composer_label, genre_label, PdmxMeta};
use crate::score::Score;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Merkmale einer Partitur (Werte als lesbare Wörter).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreFacets {
    /// Erste Stimme der Partitur im Melodie-Index (zum Öffnen).
    pub doc: u32,
    pub title: String,
    pub composer: String,
    /// Genres (deutsch); leer = ohne Angabe.
    #[serde(default)]
    pub genres: Vec<String>,
    /// Schwierigkeit laut MuseScore (leicht … sehr schwer), leer = ohne Angabe.
    #[serde(default)]
    pub difficulty: String,
    /// Stimmnamen (für die Textsuche, z. B. „Cello“).
    #[serde(default)]
    pub parts: Vec<String>,
    pub mood: String,
    pub color: String,
    pub energy: String,
    pub mode: String,
    pub key: String,
    pub meter_kind: String,
    pub meter: String,
    pub tempo: String,
    pub ensemble: String,
    pub families: Vec<String>,
    pub texture: String,
    pub harmony: String,
    pub length: String,
    pub bars: u32,
}

/// Eine Ebene des Baums.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dimension {
    pub key: &'static str,
    pub label: &'static str,
    /// Eine Partitur kann mehrere Werte haben.
    pub multi: bool,
    /// Feste Reihenfolge der Werte (sonst nach Anzahl).
    #[serde(skip)]
    order: &'static [&'static str],
}

pub const DIMENSIONS: [Dimension; 16] = [
    Dimension { key: "genre", label: "Genre", multi: true, order: &[] },
    Dimension { key: "komponist", label: "Komponist", multi: false, order: &[] },
    Dimension { key: "stimmung", label: "Stimmung", multi: false, order: &["fröhlich · energisch", "heiter · ruhig", "dramatisch · aufgewühlt", "melancholisch · ruhig"] },
    Dimension { key: "faerbung", label: "Färbung", multi: false, order: &["hell", "ausgewogen", "dunkel"] },
    Dimension { key: "energie", label: "Energie", multi: false, order: &["ruhig", "bewegt", "energisch"] },
    Dimension { key: "tongeschlecht", label: "Dur / Moll", multi: false, order: &["Dur", "Moll", "unbestimmt"] },
    Dimension { key: "tonart", label: "Tonart", multi: false, order: &[] },
    Dimension { key: "taktart", label: "Taktart", multi: false, order: &["gerade", "Dreier", "zusammengesetzt", "unregelmäßig", "ohne Angabe"] },
    Dimension { key: "takt", label: "Takt", multi: false, order: &[] },
    Dimension { key: "tempo", label: "Tempo", multi: false, order: &["langsam", "mäßig", "schnell", "sehr schnell", "ohne Angabe"] },
    Dimension { key: "besetzung", label: "Besetzung", multi: false, order: &["Solo", "Duo", "Trio", "Quartett", "Kammerensemble", "Großes Ensemble"] },
    Dimension { key: "instrumente", label: "Instrumentengruppe", multi: true, order: &["Tasteninstrumente", "Gesang", "Streicher", "Zupfinstrumente", "Holzbläser", "Blechbläser", "Schlagwerk", "Sonstige"] },
    Dimension { key: "satz", label: "Satzweise", multi: false, order: &["einstimmig", "Melodie mit Begleitung", "mehrstimmig", "akkordisch"] },
    Dimension { key: "harmonik", label: "Harmonik", multi: false, order: &["diatonisch", "leicht chromatisch", "chromatisch"] },
    Dimension { key: "laenge", label: "Länge", multi: false, order: &["kurz", "mittel", "lang", "sehr lang"] },
    Dimension { key: "schwierigkeit", label: "Schwierigkeit", multi: false, order: &["leicht", "mittel", "schwer", "sehr schwer", "ohne Angabe"] },
];

pub fn dimension(key: &str) -> Option<&'static Dimension> {
    DIMENSIONS.iter().find(|d| d.key == key)
}

/// Werte einer Partitur für eine Ebene.
pub fn values<'a>(f: &'a ScoreFacets, dim: &str) -> Vec<&'a str> {
    let one = |s: &'a str| vec![s];
    match dim {
        "genre" if f.genres.is_empty() => vec!["ohne Angabe"],
        "genre" => f.genres.iter().map(String::as_str).collect(),
        "schwierigkeit" => one(if f.difficulty.is_empty() { "ohne Angabe" } else { &f.difficulty }),
        "stimmung" => one(&f.mood),
        "faerbung" => one(&f.color),
        "energie" => one(&f.energy),
        "tongeschlecht" => one(&f.mode),
        "tonart" => one(if f.key.is_empty() { "unbestimmt" } else { &f.key }),
        "taktart" => one(&f.meter_kind),
        "takt" => one(if f.meter.is_empty() { "ohne Angabe" } else { &f.meter }),
        "tempo" => one(&f.tempo),
        "besetzung" => one(&f.ensemble),
        "instrumente" => f.families.iter().map(String::as_str).collect(),
        "satz" => one(&f.texture),
        "harmonik" => one(&f.harmony),
        "laenge" => one(&f.length),
        "komponist" => one(if f.composer.trim().is_empty() { "unbekannt" } else { f.composer.trim() }),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- Merkmale bestimmen

/// Instrumentengruppe aus dem Stimmnamen (deutsch, englisch, italienisch).
pub fn family(part_name: &str, has_voices: bool) -> &'static str {
    let n = part_name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    let word = |w: &str| n.split(|c: char| !c.is_alphanumeric()).any(|t| t == w);
    if has(&["drum", "schlagzeug", "percussion", "perkussion", "timpani", "pauke", "xylophon", "marimba", "vibraphon", "glockenspiel", "cymbal", "becken", "snare", "conga", "bongo", "triangle"]) {
        return "Schlagwerk";
    }
    if has(&["english horn", "englisch horn", "cor anglais", "flute", "flöte", "floete", "piccolo", "recorder", "oboe", "clarinet", "klarinette", "bassoon", "fagott", "sax"]) {
        return "Holzbläser";
    }
    if has(&["piano", "klavier", "keyboard", "organ", "orgel", "harpsichord", "cembalo", "celesta", "accordion", "akkordeon", "synth", "e-piano", "rhodes"]) {
        return "Tasteninstrumente";
    }
    if has(&["guitar", "gitarre", "harp", "harfe", "lute", "laute", "ukulele", "mandolin", "banjo", "bass guitar", "e-bass", "electric bass", "zither"]) {
        return "Zupfinstrumente";
    }
    if has(&["violin", "violine", "geige", "viola", "bratsche", "cello", "violoncello", "contrabass", "kontrabass", "double bass", "string", "streicher", "fiddle"]) {
        return "Streicher";
    }
    if has(&["trumpet", "trompete", "horn", "trombone", "posaune", "tuba", "euphonium", "cornet", "kornett", "flugelhorn", "flügelhorn", "brass"]) {
        return "Blechbläser";
    }
    if has(&["voice", "vocal", "voix", "soprano", "sopran", "alto", "tenor", "baritone", "bariton", "mezzo", "choir", "chor", "singstimme", "gesang", "melody", "lead", "canto", "voce", "stimme"])
        || word("alt")
        || word("s")
        || word("a")
        || word("t")
    {
        return "Gesang";
    }
    if word("bass") || word("b") {
        return if has_voices { "Gesang" } else { "Zupfinstrumente" };
    }
    "Sonstige"
}

fn ensemble(parts: usize) -> &'static str {
    match parts {
        0 | 1 => "Solo",
        2 => "Duo",
        3 => "Trio",
        4 => "Quartett",
        5..=9 => "Kammerensemble",
        _ => "Großes Ensemble",
    }
}

fn meter_kind(ts: Option<(u32, u32)>) -> &'static str {
    match ts {
        None => "ohne Angabe",
        Some((n, d)) => match (n, d) {
            (6 | 9 | 12, 8 | 16) => "zusammengesetzt",
            (3, _) => "Dreier",
            (2 | 4, _) => "gerade",
            (6 | 9 | 12, _) => "zusammengesetzt",
            _ => "unregelmäßig",
        },
    }
}

fn tempo_word(bpm: Option<f64>) -> &'static str {
    match bpm {
        None => "ohne Angabe",
        Some(b) if b < 76.0 => "langsam",
        Some(b) if b < 108.0 => "mäßig",
        Some(b) if b < 140.0 => "schnell",
        Some(_) => "sehr schnell",
    }
}

fn three(v: f32, low: &'static str, mid: &'static str, high: &'static str) -> &'static str {
    if v > 0.33 {
        high
    } else if v < -0.33 {
        low
    } else {
        mid
    }
}

fn texture(a: &ScoreAnalysis) -> &'static str {
    let melodic = a.voices.iter().filter(|v| matches!(v.role, VoiceRole::MainTheme | VoiceRole::CounterMelody | VoiceRole::InnerVoice)).count();
    if a.voices.len() <= 1 {
        if a.voices.first().map(|v| v.chordal > 0.5).unwrap_or(false) { "akkordisch" } else { "einstimmig" }
    } else if a.voices.iter().any(|v| v.role == VoiceRole::CounterMelody) || melodic >= 3 {
        "mehrstimmig"
    } else if a.voices.iter().all(|v| v.chordal > 0.5) {
        "akkordisch"
    } else {
        "Melodie mit Begleitung"
    }
}

/// Bestimmt die Merkmale einer Partitur (`doc` = erste Stimme im Index);
/// `meta` ergänzt Genre, Komponist und Schwierigkeit aus PDMX.
pub fn score_facets(score: &Score, doc: u32, meta: Option<&PdmxMeta>) -> ScoreFacets {
    let a = analyze_score_with(score, false);
    facets_from(score, &a, doc, meta)
}

pub fn facets_from(score: &Score, a: &ScoreAnalysis, doc: u32, meta: Option<&PdmxMeta>) -> ScoreFacets {
    let has_voices = score.parts.iter().any(|p| {
        let n = p.name.to_lowercase();
        ["voice", "soprano", "sopran", "alto", "tenor", "choir", "chor", "vocal", "gesang"].iter().any(|w| n.contains(w))
    });
    let mut families: Vec<String> = score.parts.iter().map(|p| family(&p.name, has_voices).to_string()).collect();
    families.sort();
    families.dedup();
    if families.is_empty() {
        families.push("Sonstige".into());
    }
    let beats_per_bar = score.time_signature.map(|(n, d)| n as f64 * 4.0 / d.max(1) as f64).unwrap_or(4.0).max(0.5);
    let bars = (a.duration_quarters / beats_per_bar).round().max(0.0) as u32;
    let from_meta = meta.map(|m| m.composer.trim()).filter(|c| !c.is_empty());
    let composer = composer_label(from_meta.unwrap_or(score.composer.as_deref().unwrap_or("")));
    let mut genres: Vec<String> = meta.map(|m| m.genres.iter().map(|g| genre_label(g)).collect()).unwrap_or_default();
    genres.sort();
    genres.dedup();
    let mut parts: Vec<String> = score.parts.iter().map(|p| p.name.trim().to_string()).filter(|n| !n.is_empty()).collect();
    parts.dedup();
    ScoreFacets {
        doc,
        title: score.title.clone().unwrap_or_default().trim().to_string(),
        composer,
        genres,
        parts,
        difficulty: match meta.and_then(|m| m.complexity) {
            Some(0) => "leicht",
            Some(1) => "mittel",
            Some(2) => "schwer",
            Some(_) => "sehr schwer",
            None => "",
        }
        .to_string(),
        mood: a.mood.label.to_string(),
        color: three(a.mood.valence, "dunkel", "ausgewogen", "hell").to_string(),
        energy: three(a.mood.arousal, "ruhig", "bewegt", "energisch").to_string(),
        mode: match a.key {
            Some(k) if k.minor => "Moll",
            Some(_) => "Dur",
            None => "unbestimmt",
        }
        .to_string(),
        key: a.key_name.clone().unwrap_or_default(),
        meter_kind: meter_kind(score.time_signature).to_string(),
        meter: score.time_signature.map(|(n, d)| format!("{n}/{d}")).unwrap_or_default(),
        tempo: tempo_word(score.tempo_bpm).to_string(),
        ensemble: ensemble(score.parts.len()).to_string(),
        families,
        texture: texture(a).to_string(),
        harmony: if a.chromaticism < 0.05 {
            "diatonisch"
        } else if a.chromaticism < 0.15 {
            "leicht chromatisch"
        } else {
            "chromatisch"
        }
        .to_string(),
        length: match bars {
            0..=15 => "kurz",
            16..=63 => "mittel",
            64..=199 => "lang",
            _ => "sehr lang",
        }
        .to_string(),
        bars,
    }
}

// ---------------------------------------------------------------- Baum

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    pub value: String,
    pub count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeScore {
    pub doc: u32,
    pub title: String,
    pub composer: String,
    /// Kurzbeschreibung: Tonart · Takt · Tempo · Besetzung
    pub facts: String,
}

/// Inhalt eines Knotens: weitere Zweige oder (auf der untersten Ebene) Partituren.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeLevel {
    /// Ebene der Zweige (None bei Partituren).
    pub dimension: Option<&'static str>,
    pub nodes: Vec<TreeNode>,
    pub scores: Vec<TreeScore>,
    /// Gesamtzahl der Zweige bzw. Partituren (für „weitere …“).
    pub total: usize,
    /// Partituren unter diesem Knoten.
    pub matching: usize,
}

fn matches(f: &ScoreFacets, levels: &[&str], path: &[String]) -> bool {
    path.iter().enumerate().all(|(i, want)| levels.get(i).map(|d| values(f, d).contains(&want.as_str())).unwrap_or(true))
}

/// Kinder des Knotens `path` (Werte der ersten `path.len()` Ebenen).
/// Ist `path` so lang wie `levels`, kommen die Partituren (alphabetisch).
pub fn tree_children(all: &[ScoreFacets], levels: &[&str], path: &[String], offset: usize, limit: usize) -> TreeLevel {
    let hits: Vec<&ScoreFacets> = all.iter().filter(|f| matches(f, levels, path)).collect();
    let matching = hits.len();
    if let Some(&dim) = levels.get(path.len()) {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for f in &hits {
            for v in values(f, dim) {
                *counts.entry(v).or_default() += 1;
            }
        }
        let order = dimension(dim).map(|d| d.order).unwrap_or(&[]);
        // Feste Reihenfolge, sonst nach Anzahl; Restgruppen („ohne Angabe“ …) immer ans Ende.
        let rest = |v: &str| matches!(v, "ohne Angabe" | "unbekannt" | "unbestimmt" | "Sonstige");
        let rank = |v: &str| order.iter().position(|o| *o == v).unwrap_or(order.len() + usize::from(rest(v)));
        let mut nodes: Vec<TreeNode> = counts.into_iter().map(|(v, c)| TreeNode { value: v.to_string(), count: c }).collect();
        nodes.sort_by(|a, b| {
            rank(&a.value)
                .cmp(&rank(&b.value))
                .then(b.count.cmp(&a.count))
                .then_with(|| a.value.to_lowercase().cmp(&b.value.to_lowercase()))
        });
        let total = nodes.len();
        let nodes = nodes.into_iter().skip(offset).take(limit).collect();
        return TreeLevel { dimension: dimension(dim).map(|d| d.key), nodes, scores: Vec::new(), total, matching };
    }
    let mut scores: Vec<&ScoreFacets> = hits;
    scores.sort_by(|a, b| {
        let key = |f: &ScoreFacets| if f.title.is_empty() { "\u{10FFFF}".to_string() } else { f.title.to_lowercase() };
        key(a).cmp(&key(b)).then(a.doc.cmp(&b.doc))
    });
    let total = scores.len();
    let scores = scores
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|f| TreeScore {
            doc: f.doc,
            title: f.title.clone(),
            composer: f.composer.clone(),
            facts: [f.key.as_str(), f.meter.as_str(), f.tempo.as_str(), f.ensemble.as_str()]
                .iter()
                .filter(|s| !s.is_empty() && **s != "ohne Angabe")
                .copied()
                .collect::<Vec<_>>()
                .join(" · "),
        })
        .collect();
    TreeLevel { dimension: None, nodes: Vec::new(), scores, total, matching }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::{parse_musicxml, tests::make_xml};

    fn f(doc: u32, mood: &str, families: &[&str], composer: &str) -> ScoreFacets {
        ScoreFacets {
            doc,
            title: format!("Stück {doc}"),
            composer: composer.into(),
            mood: mood.into(),
            families: families.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn instrument_families_from_part_names() {
        assert_eq!(family("Piano", false), "Tasteninstrumente");
        assert_eq!(family("Violine I", false), "Streicher");
        assert_eq!(family("Flûte / Flute", false), "Holzbläser");
        assert_eq!(family("English Horn", false), "Holzbläser");
        assert_eq!(family("Horn in F", false), "Blechbläser");
        assert_eq!(family("Sopran", false), "Gesang");
        assert_eq!(family("Bass", true), "Gesang");
        assert_eq!(family("Bass", false), "Zupfinstrumente");
        assert_eq!(family("Acoustic Guitar", false), "Zupfinstrumente");
        assert_eq!(family("Drumset", false), "Schlagwerk");
        assert_eq!(family("Part 1", false), "Sonstige");
    }

    #[test]
    fn tree_groups_counts_orders_and_multi_membership() {
        let all = vec![
            f(1, "heiter · ruhig", &["Tasteninstrumente"], "Bach"),
            f(2, "fröhlich · energisch", &["Gesang", "Tasteninstrumente"], "Mozart"),
            f(3, "fröhlich · energisch", &["Streicher"], "Bach"),
            f(4, "melancholisch · ruhig", &["Gesang"], ""),
        ];
        let levels = ["stimmung", "instrumente"];
        // Ebene 1: feste Reihenfolge der Stimmungen
        let top = tree_children(&all, &levels, &[], 0, 50);
        assert_eq!(top.dimension, Some("stimmung"));
        assert_eq!(top.matching, 4);
        assert_eq!(
            top.nodes.iter().map(|n| (n.value.as_str(), n.count)).collect::<Vec<_>>(),
            vec![("fröhlich · energisch", 2), ("heiter · ruhig", 1), ("melancholisch · ruhig", 1)]
        );
        // Ebene 2: Partitur 2 steht unter Gesang und Tasten
        let sub = tree_children(&all, &levels, &["fröhlich · energisch".into()], 0, 50);
        let names: Vec<&str> = sub.nodes.iter().map(|n| n.value.as_str()).collect();
        assert_eq!(names, vec!["Tasteninstrumente", "Gesang", "Streicher"]);
        // Blätter: Partituren unter dem Pfad
        let leaves = tree_children(&all, &levels, &["fröhlich · energisch".into(), "Gesang".into()], 0, 50);
        assert_eq!(leaves.dimension, None);
        assert_eq!(leaves.scores.iter().map(|s| s.doc).collect::<Vec<_>>(), vec![2]);
        // Komponist: leer → „unbekannt“; Seiten
        let comp = tree_children(&all, &["komponist"], &[], 0, 1);
        assert_eq!((comp.total, comp.nodes[0].value.as_str(), comp.nodes[0].count), (3, "Bach", 2));
        let page2 = tree_children(&all, &["komponist"], &[], 1, 5);
        assert!(page2.nodes.iter().any(|n| n.value == "unbekannt"));
        // Restgruppe zuletzt, auch wenn sie am größten ist
        let mut many = all.clone();
        for d in 10..20 {
            many.push(f(d, "heiter · ruhig", &["Tasteninstrumente"], ""));
        }
        let comp = tree_children(&many, &["komponist"], &[], 0, 10);
        assert_eq!(comp.nodes.last().unwrap().value, "unbekannt");
        assert_eq!(comp.nodes[0].value, "Bach");
    }

    #[test]
    fn facets_of_a_real_score() {
        let tune: &[(u8, u32)] = &[(60, 2), (62, 2), (64, 2), (65, 2), (67, 4), (67, 4), (69, 2), (67, 2), (65, 2), (64, 2), (62, 4), (60, 8)];
        let s = parse_musicxml(&make_xml("Lied", &[("Sopran", tune), ("Klavier", tune)])).unwrap();
        let meta = PdmxMeta { genres: vec!["folk".into(), "classical".into()], composer: "Trad.".into(), complexity: Some(0), license_conflict: false };
        let f = score_facets(&s, 7, Some(&meta));
        assert_eq!(f.doc, 7);
        assert_eq!(f.title, "Lied");
        assert_eq!(f.genres, vec!["Klassik".to_string(), "Volksmusik & Folk".to_string()]);
        assert_eq!(f.composer, "Traditionell / anonym");
        assert_eq!(f.difficulty, "leicht");
        let plain = score_facets(&s, 7, None);
        assert_eq!(values(&plain, "genre"), vec!["ohne Angabe"]);
        assert_eq!(f.ensemble, "Duo");
        assert_eq!(f.families, vec!["Gesang".to_string(), "Tasteninstrumente".to_string()]);
        assert_eq!(f.mode, "Dur");
        assert!(!f.length.is_empty() && f.bars > 0);
        for d in DIMENSIONS {
            assert!(!values(&f, d.key).is_empty(), "Ebene {} ohne Wert", d.key);
        }
    }
}
