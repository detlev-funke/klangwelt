//! Compact note notation for the agent: `C4 1 · E4 .5 v100 · G4 .5` instead of
//! a long JSON list – less text to generate.
//!
//! * Notes are separated by `·`, `•`, `,`, `;`, `|` (bar line) or a line break.
//! * A note: pitch, length in beats, options. Pitch as a note name (German
//!   spelling like everywhere in the agent tools: `H` = B, `B` = B♭; `Bb`,
//!   `F#3`, `Fis3`, `Es4` work too) or as a MIDI number (`60` = middle C).
//!   Several pitches with `+` form a chord (`C4+E4+G4 2`), `r` is a rest.
//! * Length: `1`, `.5`, `1.5`, `1/3` (triplet). Omitted = the previous length
//!   (start: 1).
//! * Options: `v<1–127>` velocity – applies until the next `v` (start: 100);
//!   `@<beat>` start in beats from the clip start (otherwise right after the
//!   previous note).

use klang_core::Note;

const SEPARATORS: &[char] = &['·', '•', ',', ';', '|', '\n'];

fn number(s: &str) -> Option<f64> {
    let v = match s.split_once('/') {
        Some((a, b)) => a.parse::<f64>().ok()? / b.parse::<f64>().ok()?,
        None => s.parse::<f64>().ok()?,
    };
    v.is_finite().then_some(v)
}

fn pitch(s: &str) -> Result<u8, String> {
    crate::library::parse_note(s).ok_or_else(|| format!("Unknown pitch \"{s}\" – note names like C4, F#3, Bb2, H3 (German H = B, B = B♭) or MIDI numbers (60 = middle C)"))
}

/// Notes from the compact notation; start and length in beats from the clip start.
pub fn parse(text: &str) -> Result<Vec<Note>, String> {
    let mut notes = Vec::new();
    let (mut cursor, mut length, mut velocity) = (0.0f64, 1.0f64, 100u8);
    for (i, item) in text.split(SEPARATORS).map(str::trim).enumerate().filter(|(_, s)| !s.is_empty()) {
        let err = |e: String| format!("Note {} (\"{item}\"): {e}", i + 1);
        let mut parts = item.split_whitespace();
        let head = parts.next().unwrap_or_default();
        let pitches: Vec<u8> = if matches!(head, "r" | "R" | "-" | "_") {
            Vec::new()
        } else {
            head.split('+').map(pitch).collect::<Result<_, _>>().map_err(err)?
        };
        let mut start = None;
        let mut len_given = false;
        for p in parts {
            if let Some(v) = p.strip_prefix(['v', 'V']) {
                let v: u8 = v.parse().ok().filter(|v| (1..=127).contains(v)).ok_or_else(|| err(format!("velocity \"{p}\" must be v1 … v127")))?;
                velocity = v;
            } else if let Some(s) = p.strip_prefix('@') {
                let s = number(s).filter(|s| *s >= 0.0).ok_or_else(|| err(format!("start \"{p}\" must be @<beat ≥ 0>")))?;
                start = Some(s);
            } else if !len_given {
                length = number(p).filter(|l| *l > 0.0).ok_or_else(|| err(format!("\"{p}\" is not a length in beats (e.g. 1, .5, 1/3)")))?;
                len_given = true;
            } else {
                return Err(err(format!("don't know what \"{p}\" means – after the pitch come the length, v<velocity> and @<start>")));
            }
        }
        let at = start.unwrap_or(cursor);
        for &p in &pitches {
            notes.push(Note::new(p, at, length, velocity));
        }
        cursor = at + length;
    }
    if notes.is_empty() {
        return Err("No notes in the text – e.g. \"C4 1 · E4 .5 v100 · G4 .5\"".into());
    }
    Ok(notes)
}

/// Replaces compact `notes` (a string) in a command or clip by the JSON list –
/// everywhere in `v` where an object has `"notes": "<text>"`.
pub fn expand(v: &mut serde_json::Value) -> Result<(), String> {
    match v {
        serde_json::Value::Object(o) => {
            if let Some(serde_json::Value::String(text)) = o.get("notes") {
                let notes = parse(text)?;
                o.insert("notes".into(), serde_json::to_value(notes).map_err(|e| e.to_string())?);
            }
            o.values_mut().try_for_each(expand)
        }
        serde_json::Value::Array(a) => a.iter_mut().try_for_each(expand),
        _ => Ok(()),
    }
}

/// A number as the notation writes it, and the value `parse` reads back: exact
/// decimals for binary values (`.5`, `.125`), fractions for triplets and
/// sextuplets (`1/3`, `5/6`), otherwise rounded to three places.
fn number_text(x: f64) -> (String, f64) {
    let trim = |s: String| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    for den in [1.0, 2.0, 4.0, 8.0, 16.0, 32.0] {
        if (x * den - (x * den).round()).abs() < 1e-6 {
            let v = (x * den).round() / den;
            return (trim(format!("{v:.5}")), v);
        }
    }
    for den in [3u32, 6, 12, 24] {
        let k = x * den as f64;
        if (k - k.round()).abs() < 1e-6 {
            let (mut k, mut d) = (k.round() as u64, den as u64);
            let g = gcd(k, d);
            (k, d) = (k / g, d / g);
            return (format!("{k}/{d}"), k as f64 / d as f64);
        }
    }
    let v = (x * 1000.0).round() / 1000.0;
    (trim(format!("{v:.3}")), v)
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

/// Notes in the compact notation – the counterpart of [`parse`]: (start,
/// length, MIDI) with start and length in beats. Notes with the same start and
/// length become a chord; a length is written on the first note and where it
/// changes, a start (`@`) only where the note doesn't follow the previous one;
/// velocity is left out.
pub fn format(notes: &[(f64, f64, u8)]) -> String {
    let mut sorted: Vec<(f64, f64, u8)> = notes.iter().copied().filter(|n| n.1 > 0.0).collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.2.cmp(&a.2)));
    let mut items: Vec<String> = Vec::new();
    // what `parse` will have read so far: position after the previous note, current length (none yet)
    let (mut cursor, mut length) = (0.0f64, f64::NAN);
    let mut i = 0;
    while i < sorted.len() {
        let (start, len, _) = sorted[i];
        let mut j = i + 1;
        while j < sorted.len() && (sorted[j].0 - start).abs() < 1e-9 && (sorted[j].1 - len).abs() < 1e-9 {
            j += 1;
        }
        let mut item = sorted[i..j].iter().map(|n| crate::sound::note_name(n.2)).collect::<Vec<_>>().join("+");
        let (len_text, len_read) = number_text(len);
        if length.is_nan() || (len_read - length).abs() > 1e-9 {
            item += &format!(" {len_text}");
            length = len_read;
        }
        let mut at = cursor;
        if (start - cursor).abs() > 1e-6 {
            let (start_text, start_read) = number_text(start);
            item += &format!(" @{start_text}");
            at = start_read;
        }
        cursor = at + length;
        items.push(item);
        i = j;
    }
    items.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_writes_what_parse_reads_back() {
        // chord, rest, triplets, a note under a held one, an off-grid start
        let notes = vec![
            (0.0, 2.0, 60), (0.0, 2.0, 64), (0.0, 2.0, 67),
            (3.0, 1.0 / 3.0, 62), (3.0 + 1.0 / 3.0, 1.0 / 3.0, 63), (3.0 + 2.0 / 3.0, 1.0 / 3.0, 65),
            (4.0, 0.5, 70), (4.0, 4.0, 48), (4.5, 0.5, 71), (9.1234, 0.25, 72),
        ];
        let text = format(&notes);
        assert_eq!(text, "G4+E4+C4 2 · D4 1/3 @3 · Eb4 · F4 · Bb4 0.5 · C3 4 @4 · H4 0.5 @4.5 · C5 0.25 @9.123");
        let back: Vec<(f64, f64, u8)> = parse(&text).unwrap().iter().map(|n| (n.start_beats, n.length_beats, n.pitch)).collect();
        let mut want = notes.clone();
        want.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.2.cmp(&a.2)));
        assert_eq!(back.len(), want.len());
        for (b, w) in back.iter().zip(&want) {
            assert!((b.0 - w.0).abs() < 1e-3 && (b.1 - w.1).abs() < 1e-9 && b.2 == w.2, "{b:?} vs {w:?}");
        }
        assert_eq!(format(&[]), "");
    }

    fn short(n: &[Note]) -> Vec<(u8, f64, f64, u8)> {
        n.iter().map(|n| (n.pitch, n.start_beats, n.length_beats, n.velocity)).collect()
    }

    #[test]
    fn example_from_the_report() {
        let n = parse("C4 1 · E4 .5 v100 · G4 .5").unwrap();
        assert_eq!(short(&n), vec![(60, 0.0, 1.0, 100), (64, 1.0, 0.5, 100), (67, 1.5, 0.5, 100)]);
    }

    #[test]
    fn chords_rests_starts_carry_over() {
        let n = parse("C3+E3+G3 2 v70 | r 1 | A2 | H2 1/2 v110 @6 ; 62").unwrap();
        assert_eq!(
            short(&n),
            vec![(48, 0.0, 2.0, 70), (52, 0.0, 2.0, 70), (55, 0.0, 2.0, 70), (45, 3.0, 1.0, 70), (47, 6.0, 0.5, 110), (62, 6.5, 0.5, 110)]
        );
        // German spelling: B = B♭, H = B; Bb and Fis work too
        assert_eq!(short(&parse("B3 1, H3, Bb3, Fis4").unwrap()).iter().map(|n| n.0).collect::<Vec<_>>(), vec![58, 59, 58, 66]);
    }

    #[test]
    fn errors_name_the_note() {
        assert!(parse("C4 1 · X9 1").unwrap_err().contains("Note 2"));
        assert!(parse("C4 0").unwrap_err().contains("length"));
        assert!(parse("C4 1 v200").unwrap_err().contains("velocity"));
        assert!(parse("C4 1 2").unwrap_err().contains("don't know"));
        assert!(parse(" · ").is_err());
    }

    #[test]
    fn expand_replaces_strings_in_commands() {
        let mut v = serde_json::json!({ "type": "setClipNotes", "trackId": 1, "clipId": 2, "notes": "C4 1 · D4" });
        expand(&mut v).unwrap();
        assert_eq!(v["notes"][1]["pitch"], 62);
        assert_eq!(v["notes"][1]["startBeats"], 1.0);
        let mut nested = serde_json::json!({ "track": { "clips": [{ "source": { "type": "notes", "lengthBeats": 4, "notes": "A2 4" } }] } });
        expand(&mut nested).unwrap();
        assert_eq!(nested["track"]["clips"][0]["source"]["notes"][0]["pitch"], 45);
    }
}
