//! Partiturmodell und MusicXML-Leser (score-partwise, .musicxml/.xml/.mxl).
//! Dazu [`load_score`] für alle Formate der Bibliothek: Humdrum `**kern`
//! (.krn, [`crate::kern`]) und DCML-Notentabellen (.notes.tsv, [`crate::dcml`]).
//!
//! Gelesen wird nur, was für Melodie- und Stimmenanalyse gebraucht wird:
//! Stimmen, Tonhöhen, Dauern, Akkorde, Haltebögen, backup/forward, Tonart,
//! Taktart, Tempo. Zeiten werden in Viertelnoten ab Stückbeginn gerechnet.

use crate::MusicError;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Score {
    pub title: Option<String>,
    pub composer: Option<String>,
    pub key_fifths: Option<i8>,
    pub key_mode: Option<String>,
    pub time_signature: Option<(u32, u32)>,
    pub tempo_bpm: Option<f64>,
    pub parts: Vec<Part>,
    /// Akkordanalyse von Fachleuten (DCML), sortiert nach Einsatz – leer, wenn
    /// die Quelle keine mitbringt. Grundlage für die Akkordstatistik statt der
    /// automatischen Erkennung.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harmony: Vec<LabeledChord>,
}

/// Ein Akkord aus einer Analyse von Fachleuten.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabeledChord {
    /// Einsatz und Dauer in Viertelnoten.
    pub onset: f64,
    pub duration: f64,
    pub chord: crate::harmony::Chord,
    /// Bezeichnung wie in der Quelle (z. B. „V65/V“).
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub id: String,
    pub name: String,
    pub voices: Vec<Voice>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Voice {
    pub staff: u32,
    pub voice: u32,
    /// Sortiert nach Einsatz, bei Akkorden höchste Note zuerst.
    pub notes: Vec<ScoreNote>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreNote {
    /// Einsatz in Viertelnoten.
    pub onset: f64,
    /// Dauer in Viertelnoten (Haltebögen bereits zusammengefasst).
    pub duration: f64,
    pub midi: u8,
}

impl Score {
    pub fn duration_quarters(&self) -> f64 {
        self.parts
            .iter()
            .flat_map(|p| &p.voices)
            .flat_map(|v| &v.notes)
            .map(|n| n.onset + n.duration)
            .fold(0.0, f64::max)
    }

    pub fn note_count(&self) -> usize {
        self.parts.iter().flat_map(|p| &p.voices).map(|v| v.notes.len()).sum()
    }
}

/// Kann [`load_score`] diese Datei lesen? (.mxl, .musicxml, .xml, .krn, .notes.tsv)
pub fn is_score_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    let lower = name.to_ascii_lowercase();
    [".mxl", ".musicxml", ".xml", ".krn", ".notes.tsv"].iter().any(|e| lower.ends_with(e))
}

/// Lädt .musicxml, .xml, komprimiertes .mxl, Humdrum .krn oder eine
/// DCML-Notentabelle (.notes.tsv, mit Harmonien und Metadaten daneben).
pub fn load_score(path: &Path) -> Result<Score, MusicError> {
    let mut score = load_any(path)?;
    if score.composer.as_deref().is_none_or(|c| c.trim().is_empty()) {
        score.composer = composer_hint(path);
    }
    Ok(score)
}

/// Name der Datei mit dem Komponisten einer Sammlung, deren Quelle ihn nicht
/// in jeder Partitur nennt (z. B. die DCML-Bach-Choräle). Liegt im Ordner
/// der Partitur oder bis zu zwei Ebenen darüber.
pub const COMPOSER_FILE: &str = "komponist.txt";

fn composer_hint(path: &Path) -> Option<String> {
    path.ancestors()
        .skip(1)
        .take(3)
        .find_map(|d| std::fs::read_to_string(d.join(COMPOSER_FILE)).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn load_any(path: &Path) -> Result<Score, MusicError> {
    let lower = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    if lower.ends_with(".krn") {
        return crate::kern::parse_kern(&decode_text(&std::fs::read(path)?));
    }
    if lower.ends_with(".notes.tsv") {
        return crate::dcml::load_dcml(path);
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = if ext == "mxl" {
        read_mxl(path)?
    } else {
        decode_text(&std::fs::read(path)?)
    };
    parse_musicxml(&text)
}

/// Loads a score from its bytes; `name` (file name or path) tells the format:
/// .mxl, .musicxml, .xml or .krn. For scores that don't lie in a file of their
/// own – in a package's ZIP or in the browser's storage.
pub fn load_score_bytes(name: &str, bytes: &[u8]) -> Result<Score, MusicError> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".krn") {
        return crate::kern::parse_kern(&decode_text(bytes));
    }
    if lower.ends_with(".mxl") {
        return parse_musicxml(&read_mxl_from(std::io::Cursor::new(bytes))?);
    }
    if lower.ends_with(".musicxml") || lower.ends_with(".xml") {
        return parse_musicxml(&decode_text(bytes));
    }
    Err(MusicError::Unsupported(name.to_string()))
}

fn read_mxl(path: &Path) -> Result<String, MusicError> {
    read_mxl_from(std::io::BufReader::new(std::fs::File::open(path)?))
}

fn read_mxl_from<R: std::io::Read + std::io::Seek>(reader: R) -> Result<String, MusicError> {
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| MusicError::Zip(e.to_string()))?;

    let mut root: Option<String> = None;
    if let Ok(mut c) = zip.by_name("META-INF/container.xml") {
        let mut s = String::new();
        if c.read_to_string(&mut s).is_ok() {
            if let Some(i) = s.find("full-path=\"") {
                let rest = &s[i + 11..];
                if let Some(end) = rest.find('"') {
                    root = Some(rest[..end].to_string());
                }
            }
        }
    }
    let name = match root {
        Some(r) => r,
        None => (0..zip.len())
            .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| !n.starts_with("META-INF") && (n.ends_with(".xml") || n.ends_with(".musicxml")))
            .ok_or_else(|| MusicError::Zip(klang_i18n::tr!("no MusicXML file in the archive")))?,
    };
    let mut f = zip.by_name(&name).map_err(|e| MusicError::Zip(e.to_string()))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    Ok(decode_text(&bytes))
}

/// UTF-8 oder UTF-16 (mit BOM) → String.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |le: bool| {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, ..] => utf16(true),
        [0xFE, 0xFF, ..] => utf16(false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[derive(Default)]
struct NoteBuild {
    rest: bool,
    chord: bool,
    grace: bool,
    step: Option<u8>,
    alter: f64,
    octave: Option<i32>,
    duration: f64,
    voice: u32,
    staff: u32,
    tie_start: bool,
    tie_stop: bool,
}

#[derive(Default)]
struct PartBuild {
    id: String,
    voices: BTreeMap<(u32, u32), Vec<ScoreNote>>,
    open_ties: HashMap<(u32, u32, u8), usize>,
    divisions: f64,
    time: f64,
    last_onset: f64,
}

fn attr(e: &BytesStart, name: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn step_semitone(c: u8) -> Option<u8> {
    Some(match c.to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    })
}

pub fn parse_musicxml(xml: &str) -> Result<Score, MusicError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut score = Score::default();
    let mut part_names: HashMap<String, String> = HashMap::new();
    let mut part_order: Vec<String> = Vec::new();
    let mut path: Vec<Vec<u8>> = Vec::new();
    let mut current_score_part: Option<String> = None;
    let mut creator_type: Option<String> = None;
    let mut part: Option<PartBuild> = None;
    let mut note: Option<NoteBuild> = None;
    let mut seen_root = false;

    loop {
        let ev = reader.read_event().map_err(|e| MusicError::Xml(e.to_string()))?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(ev, Event::Empty(_));
                let name = e.local_name().as_ref().to_vec();
                if !seen_root {
                    seen_root = true;
                    if name.as_slice() == b"score-timewise" {
                        return Err(MusicError::Unsupported("score-timewise".into()));
                    }
                }
                match name.as_slice() {
                    b"score-part" => current_score_part = attr(e, b"id"),
                    b"creator" => creator_type = attr(e, b"type"),
                    b"part" => {
                        let id = attr(e, b"id").unwrap_or_default();
                        part = Some(PartBuild { id, divisions: 1.0, ..Default::default() });
                    }
                    b"note" => note = Some(NoteBuild { voice: 1, staff: 1, ..Default::default() }),
                    b"rest" => {
                        if let Some(n) = note.as_mut() {
                            n.rest = true
                        }
                    }
                    b"chord" => {
                        if let Some(n) = note.as_mut() {
                            n.chord = true
                        }
                    }
                    b"grace" => {
                        if let Some(n) = note.as_mut() {
                            n.grace = true
                        }
                    }
                    b"tie" => {
                        if let Some(n) = note.as_mut() {
                            match attr(e, b"type").as_deref() {
                                Some("start") => n.tie_start = true,
                                Some("stop") => n.tie_stop = true,
                                _ => {}
                            }
                        }
                    }
                    b"sound" => {
                        if score.tempo_bpm.is_none() {
                            score.tempo_bpm = attr(e, b"tempo").and_then(|t| t.parse().ok());
                        }
                    }
                    _ => {}
                }
                if is_empty {
                    end_element(&name, &mut part, &mut note, &mut score);
                } else {
                    path.push(name);
                }
            }
            Event::End(e) => {
                let name = e.local_name().as_ref().to_vec();
                path.pop();
                end_element(&name, &mut part, &mut note, &mut score);
                if name.as_slice() == b"score-part" {
                    current_score_part = None;
                }
            }
            Event::Text(t) => {
                let text = match t.unescape() {
                    Ok(s) => s.into_owned(),
                    Err(_) => String::from_utf8_lossy(&t).into_owned(),
                };
                let text = text.trim();
                let Some(el) = path.last() else { continue };
                let parent = path.len().checked_sub(2).map(|i| path[i].as_slice());
                match el.as_slice() {
                    b"part-name" => {
                        if let Some(id) = &current_score_part {
                            part_names.insert(id.clone(), text.to_string());
                            part_order.push(id.clone());
                        }
                    }
                    b"work-title" | b"movement-title" if score.title.is_none() => {
                        score.title = Some(text.to_string())
                    }
                    b"creator" if creator_type.as_deref() == Some("composer") && score.composer.is_none() => {
                        score.composer = Some(text.to_string())
                    }
                    b"divisions" => {
                        if let (Some(p), Ok(d)) = (part.as_mut(), text.parse::<f64>()) {
                            if d > 0.0 {
                                p.divisions = d;
                            }
                        }
                    }
                    b"fifths" if score.key_fifths.is_none() => score.key_fifths = text.parse().ok(),
                    b"mode" if score.key_mode.is_none() => score.key_mode = Some(text.to_string()),
                    b"beats" if score.time_signature.is_none() => {
                        if let Ok(b) = text.parse::<u32>() {
                            score.time_signature = Some((b, 4));
                        }
                    }
                    b"beat-type" => {
                        if let (Some((b, _)), Ok(u)) = (score.time_signature, text.parse::<u32>()) {
                            if part.as_ref().map(|p| p.time == 0.0).unwrap_or(true) {
                                score.time_signature = Some((b, u));
                            }
                        }
                    }
                    b"per-minute" if score.tempo_bpm.is_none() => score.tempo_bpm = text.parse().ok(),
                    b"step" => {
                        if let Some(n) = note.as_mut() {
                            n.step = text.bytes().next().and_then(step_semitone);
                        }
                    }
                    b"alter" => {
                        if let Some(n) = note.as_mut() {
                            n.alter = text.parse().unwrap_or(0.0);
                        }
                    }
                    b"octave" => {
                        if let Some(n) = note.as_mut() {
                            n.octave = text.parse().ok();
                        }
                    }
                    b"voice" => {
                        if let Some(n) = note.as_mut() {
                            n.voice = text.parse().unwrap_or(1);
                        }
                    }
                    b"staff" => {
                        if let Some(n) = note.as_mut() {
                            n.staff = text.parse().unwrap_or(1);
                        }
                    }
                    b"duration" => {
                        let d: f64 = text.parse().unwrap_or(0.0);
                        match parent {
                            Some(b"note") => {
                                if let Some(n) = note.as_mut() {
                                    n.duration = d;
                                }
                            }
                            Some(b"backup") => {
                                if let Some(p) = part.as_mut() {
                                    p.time = (p.time - d / p.divisions).max(0.0);
                                }
                            }
                            Some(b"forward") => {
                                if let Some(p) = part.as_mut() {
                                    p.time += d / p.divisions;
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // Teilnamen aus der part-list übernehmen, Reihenfolge wie dort.
    for p in &mut score.parts {
        if let Some(n) = part_names.get(&p.id) {
            p.name = n.clone();
        }
    }
    score.parts.sort_by_key(|p| part_order.iter().position(|id| *id == p.id).unwrap_or(usize::MAX));
    Ok(score)
}

fn end_element(name: &[u8], part: &mut Option<PartBuild>, note: &mut Option<NoteBuild>, score: &mut Score) {
    match name {
        b"note" => {
            let (Some(n), Some(p)) = (note.take(), part.as_mut()) else { return };
            if n.grace {
                return;
            }
            let dur = n.duration / p.divisions;
            let onset = if n.chord { p.last_onset } else { p.time };
            if !n.rest {
                if let (Some(step), Some(oct)) = (n.step, n.octave) {
                    let midi = ((oct + 1) * 12 + step as i32 + n.alter.round() as i32).clamp(0, 127) as u8;
                    let key = (n.staff, n.voice, midi);
                    let list = p.voices.entry((n.staff, n.voice)).or_default();
                    match p.open_ties.get(&key).copied() {
                        Some(idx) if n.tie_stop && idx < list.len() => {
                            let first = &mut list[idx];
                            first.duration = (onset + dur - first.onset).max(first.duration);
                            if !n.tie_start {
                                p.open_ties.remove(&key);
                            }
                        }
                        _ => {
                            list.push(ScoreNote { onset, duration: dur, midi });
                            if n.tie_start {
                                p.open_ties.insert(key, list.len() - 1);
                            }
                        }
                    }
                }
            }
            if !n.chord {
                p.last_onset = p.time;
                p.time += dur;
            }
        }
        b"part" => {
            if let Some(p) = part.take() {
                let voices = p
                    .voices
                    .into_iter()
                    .map(|((staff, voice), mut notes)| {
                        notes.sort_by(|a, b| a.onset.total_cmp(&b.onset).then(b.midi.cmp(&a.midi)));
                        Voice { staff, voice, notes }
                    })
                    .filter(|v| !v.notes.is_empty())
                    .collect();
                score.parts.push(Part { id: p.id, name: String::new(), voices });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Baut eine einfache MusicXML-Datei: je Teil eine Stimme aus (midi, Achtel).
    /// midi = 0 steht für eine Pause. divisions = 2 (Achtel = 1).
    pub fn make_xml(title: &str, parts: &[(&str, &[(u8, u32)])]) -> String {
        let names = ["C", "C", "D", "D", "E", "F", "F", "G", "G", "A", "A", "B"];
        let alters = [0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 0];
        let mut s = format!(
            "<?xml version=\"1.0\"?><score-partwise version=\"4.0\"><work><work-title>{title}</work-title></work><part-list>"
        );
        for (i, (name, _)) in parts.iter().enumerate() {
            s += &format!("<score-part id=\"P{i}\"><part-name>{name}</part-name></score-part>");
        }
        s += "</part-list>";
        for (i, (_, notes)) in parts.iter().enumerate() {
            s += &format!("<part id=\"P{i}\"><measure number=\"1\"><attributes><divisions>2</divisions><key><fifths>0</fifths><mode>major</mode></key><time><beats>4</beats><beat-type>4</beat-type></time></attributes>");
            for &(m, d) in notes.iter() {
                if m == 0 {
                    s += &format!("<note><rest/><duration>{d}</duration></note>");
                } else {
                    let pc = (m % 12) as usize;
                    let oct = m as i32 / 12 - 1;
                    let alter = if alters[pc] != 0 { "<alter>1</alter>".to_string() } else { String::new() };
                    s += &format!(
                        "<note><pitch><step>{}</step>{alter}<octave>{oct}</octave></pitch><duration>{d}</duration><voice>1</voice></note>",
                        names[pc]
                    );
                }
            }
            s += "</measure></part>";
        }
        s += "</score-partwise>";
        s
    }

    #[test]
    fn parses_chords_ties_backup_and_alter() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE score-partwise PUBLIC "-//Recordare//DTD MusicXML 4.0 Partwise//EN" "http://www.musicxml.org/dtds/partwise.dtd">
<score-partwise version="4.0">
  <work><work-title>Probe &amp; Test</work-title></work>
  <identification><creator type="composer">Niemand</creator></identification>
  <part-list><score-part id="P1"><part-name>Klavier</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>4</divisions><key><fifths>-1</fifths><mode>minor</mode></key>
        <time><beats>3</beats><beat-type>4</beat-type></time><staves>2</staves></attributes>
      <direction><sound tempo="96"/></direction>
      <note><pitch><step>D</step><octave>5</octave></pitch><duration>4</duration><tie type="start"/><voice>1</voice><staff>1</staff></note>
      <note><pitch><step>D</step><octave>5</octave></pitch><duration>2</duration><tie type="stop"/><voice>1</voice><staff>1</staff></note>
      <note><pitch><step>B</step><alter>-1</alter><octave>4</octave></pitch><duration>2</duration><voice>1</voice><staff>1</staff></note>
      <note><chord/><pitch><step>F</step><octave>4</octave></pitch><duration>2</duration><voice>1</voice><staff>1</staff></note>
      <note><grace/><pitch><step>C</step><octave>5</octave></pitch><voice>1</voice><staff>1</staff></note>
      <note><rest/><duration>4</duration><voice>1</voice><staff>1</staff></note>
      <backup><duration>12</duration></backup>
      <note><pitch><step>D</step><octave>3</octave></pitch><duration>12</duration><voice>5</voice><staff>2</staff></note>
    </measure>
  </part>
</score-partwise>"#;
        let s = parse_musicxml(xml).unwrap();
        assert_eq!(s.title.as_deref(), Some("Probe & Test"));
        assert_eq!(s.composer.as_deref(), Some("Niemand"));
        assert_eq!(s.key_fifths, Some(-1));
        assert_eq!(s.key_mode.as_deref(), Some("minor"));
        assert_eq!(s.time_signature, Some((3, 4)));
        assert_eq!(s.tempo_bpm, Some(96.0));
        assert_eq!(s.parts.len(), 1);
        assert_eq!(s.parts[0].name, "Klavier");
        let v = &s.parts[0].voices;
        assert_eq!(v.len(), 2);
        let upper = &v[0].notes;
        assert_eq!(upper.len(), 3, "{upper:?}");
        assert_eq!(upper[0], ScoreNote { onset: 0.0, duration: 1.5, midi: 74 });
        assert_eq!(upper[1], ScoreNote { onset: 1.5, duration: 0.5, midi: 70 });
        assert_eq!(upper[2], ScoreNote { onset: 1.5, duration: 0.5, midi: 65 });
        assert_eq!(v[1].notes, vec![ScoreNote { onset: 0.0, duration: 3.0, midi: 50 }]);
        assert!((s.duration_quarters() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn helper_roundtrip() {
        let xml = make_xml("Mini", &[("Flöte", &[(72, 2), (0, 2), (61, 4)])]);
        let s = parse_musicxml(&xml).unwrap();
        let n = &s.parts[0].voices[0].notes;
        assert_eq!(n.len(), 2);
        assert_eq!(n[1], ScoreNote { onset: 2.0, duration: 2.0, midi: 61 });
    }

    #[test]
    fn rejects_timewise() {
        let r = parse_musicxml("<score-timewise version=\"4.0\"></score-timewise>");
        assert!(matches!(r, Err(MusicError::Unsupported(_))));
    }
}
