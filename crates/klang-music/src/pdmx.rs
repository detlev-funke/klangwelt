//! PDMX-Metadaten (MuseScore): Genre, Komponist, Schwierigkeit je Partitur.
//!
//! `PDMX.csv` (≈ 225 MB) wird als Datenstrom gelesen; behalten werden nur die
//! Zeilen der Partituren, die im Bibliotheksordner liegen (Bezug über den
//! Dateinamen `Qm….mxl`). Gespeichert wird eine kleine Tabelle
//! `pdmx-metadaten.tsv` neben den Noten.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub const META_FILE: &str = "pdmx-metadaten.tsv";

/// Metadaten einer Partitur aus PDMX.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdmxMeta {
    /// Genres (roh, z. B. „classical“, „rbfunksoul“).
    pub genres: Vec<String>,
    /// Komponist (bzw. Interpret, wenn kein Komponist angegeben ist).
    pub composer: String,
    /// MuseScore-Schwierigkeit 0…3.
    pub complexity: Option<u8>,
    pub license_conflict: bool,
}

/// Liest CSV (RFC 4180: Anführungszeichen, verdoppelte Anführungszeichen,
/// Zeilenumbrüche in Feldern) Datensatz für Datensatz.
pub fn for_each_record(mut r: impl BufRead, cancel: &AtomicBool, mut f: impl FnMut(&[String]) -> bool) -> io::Result<()> {
    let mut row: Vec<String> = Vec::new();
    let mut field: Vec<u8> = Vec::new();
    let mut quoted = false;
    let mut after_quote = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "abgebrochen"));
        }
        let mut i = 0;
        while i < line.len() {
            let c = line[i];
            if quoted {
                if c == b'"' {
                    if line.get(i + 1) == Some(&b'"') {
                        field.push(b'"');
                        i += 1;
                    } else {
                        quoted = false;
                        after_quote = true;
                    }
                } else {
                    field.push(c);
                }
            } else {
                match c {
                    b'"' if field.is_empty() && !after_quote => quoted = true,
                    b',' => {
                        row.push(String::from_utf8_lossy(&field).into_owned());
                        field.clear();
                        after_quote = false;
                    }
                    b'\n' => {
                        row.push(String::from_utf8_lossy(&field).into_owned());
                        field.clear();
                        after_quote = false;
                        if !f(&row) {
                            return Ok(());
                        }
                        row.clear();
                    }
                    b'\r' => {}
                    _ => field.push(c),
                }
            }
            i += 1;
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(String::from_utf8_lossy(&field).into_owned());
        f(&row);
    }
    Ok(())
}

fn stem(path: &str) -> &str {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file.split('.').next().unwrap_or(file)
}

fn na(s: &str) -> &str {
    let t = s.trim();
    if t == "NA" || t == "nan" { "" } else { t }
}

/// Filtert `PDMX.csv` auf die Partituren `wanted` (Dateinamen ohne Endung).
/// Hört auf, sobald alle gefunden sind.
pub fn filter_metadata(r: impl BufRead, wanted: &HashSet<String>, cancel: &AtomicBool) -> io::Result<HashMap<String, PdmxMeta>> {
    let mut out = HashMap::new();
    let mut cols: Option<HashMap<String, usize>> = None;
    for_each_record(r, cancel, |rec| {
        let Some(c) = cols.as_ref() else {
            cols = Some(rec.iter().enumerate().map(|(i, n)| (n.trim().to_string(), i)).collect());
            return true;
        };
        let get = |name: &str| c.get(name).and_then(|&i| rec.get(i)).map(String::as_str).unwrap_or("");
        let key = stem(get("mxl"));
        if key.is_empty() || !wanted.contains(key) {
            return true;
        }
        let composer = [get("composer_name"), get("artist_name")].into_iter().map(na).find(|s| !s.is_empty()).unwrap_or("");
        out.insert(
            key.to_string(),
            PdmxMeta {
                genres: na(get("genres")).split('-').map(str::trim).filter(|g| !g.is_empty()).map(str::to_string).collect(),
                composer: composer.to_string(),
                complexity: na(get("complexity")).parse::<f32>().ok().map(|v| v.clamp(0.0, 9.0) as u8),
                license_conflict: get("license_conflict").trim() == "True",
            },
        );
        out.len() < wanted.len()
    })?;
    Ok(out)
}

/// Speichert die gefilterten Metadaten als Tabelle (Tab-getrennt).
pub fn save_meta(path: &Path, meta: &HashMap<String, PdmxMeta>) -> io::Result<()> {
    let mut w = io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(w, "datei\tgenres\tkomponist\tschwierigkeit\tlizenzkonflikt")?;
    let mut keys: Vec<&String> = meta.keys().collect();
    keys.sort();
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    for k in keys {
        let m = &meta[k];
        writeln!(
            w,
            "{k}\t{}\t{}\t{}\t{}",
            m.genres.join("-"),
            clean(&m.composer),
            m.complexity.map(|c| c.to_string()).unwrap_or_default(),
            if m.license_conflict { "1" } else { "0" }
        )?;
    }
    w.flush()
}

pub fn load_meta(path: &Path) -> io::Result<HashMap<String, PdmxMeta>> {
    let text = std::fs::read_to_string(path)?;
    let mut out = HashMap::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 5 {
            continue;
        }
        out.insert(
            f[0].to_string(),
            PdmxMeta {
                genres: f[1].split('-').filter(|g| !g.is_empty()).map(str::to_string).collect(),
                composer: f[2].to_string(),
                complexity: f[3].parse().ok(),
                license_conflict: f[4] == "1",
            },
        );
    }
    Ok(out)
}

/// Deutsche Bezeichnung eines MuseScore-Genres.
pub fn genre_label(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "classical" => "Klassik",
        "folk" => "Volksmusik & Folk",
        "soundtrack" => "Film- & Spielemusik",
        "pop" => "Pop",
        "rock" => "Rock",
        "rbfunksoul" => "R&B, Funk & Soul",
        "jazz" => "Jazz",
        "worldmusic" => "Weltmusik",
        "electronic" => "Elektronische Musik",
        "disco" => "Disco",
        "religiousmusic" => "Geistliche Musik",
        "country" => "Country",
        "experimental" => "Experimentell",
        "newage" => "New Age",
        "hiphop" => "Hip-Hop",
        "metal" => "Metal",
        "blues" => "Blues",
        "latin" => "Latin",
        "reggae" => "Reggae",
        "alternative" => "Alternative",
        "holiday" | "holidaymusic" => "Festliche Musik",
        "childrensmusic" | "children" => "Kindermusik",
        "musical" | "musicaltheatre" => "Musical",
        "" => "ohne Angabe",
        other => {
            let mut c = other.chars();
            return c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
        }
    }
    .to_string()
}

/// Komponistenname vereinheitlichen: Lebensdaten weg, Leerraum, Traditionals.
pub fn composer_label(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // „(1685–1750)“, „(c.1540-1623)“ usw. am Ende entfernen
    if let Some(open) = s.rfind('(') {
        if s.ends_with(')') && s[open..].chars().any(|c| c.is_ascii_digit()) {
            s.truncate(open);
        }
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_matches(|c: char| c == ',' || c == ';' || c == '-').trim().to_string();
    let low = s.to_lowercase();
    let low = low.trim_end_matches('.');
    if ["trad", "traditional", "tradicional", "traditionnel", "traditionell", "anon", "anonymous", "anonym", "unattributed", "unknown", "folk", "volkslied", "volksweise"]
        .contains(&low)
    {
        return "Traditionell / anonym".into();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "path,mxl,license_conflict,genres,title,artist_name,composer_name,complexity\n\
./data/1/a.json,./mxl/1/QmA.mxl,False,classical,Erstes,NA,Johann Sebastian Bach (1685-1750),1\n\
./data/1/b.json,./mxl/1/QmB.mxl,True,classical-folk,\"Zweites, mit Komma\",Trad.,NA,0\n\
./data/2/c.json,./mxl/2/QmC.mxl,False,NA,\"Drittes \"\"zitiert\"\"\nzweite Zeile\",NA,NA,NA\n\
./data/2/d.json,./mxl/2/QmD.mxl,False,pop,Fremd,X,Y,2\n";

    #[test]
    fn filters_rows_handles_quotes_and_newlines() {
        let wanted: HashSet<String> = ["QmA", "QmB", "QmC"].iter().map(|s| s.to_string()).collect();
        let never = AtomicBool::new(false);
        let m = filter_metadata(CSV.as_bytes(), &wanted, &never).unwrap();
        assert_eq!(m.len(), 3, "QmD gehört nicht dazu");
        assert_eq!(m["QmA"].genres, vec!["classical"]);
        assert_eq!(m["QmA"].composer, "Johann Sebastian Bach (1685-1750)");
        assert_eq!(m["QmA"].complexity, Some(1));
        assert_eq!(m["QmB"].genres, vec!["classical", "folk"]);
        assert_eq!(m["QmB"].composer, "Trad.", "ohne Komponist: Interpret");
        assert!(m["QmB"].license_conflict);
        assert!(m["QmC"].genres.is_empty() && m["QmC"].composer.is_empty() && m["QmC"].complexity.is_none());

        let dir = std::env::temp_dir().join(format!("klang-pdmx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(META_FILE);
        save_meta(&path, &m).unwrap();
        assert_eq!(load_meta(&path).unwrap(), m);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn labels() {
        assert_eq!(genre_label("classical"), "Klassik");
        assert_eq!(genre_label("rbfunksoul"), "R&B, Funk & Soul");
        assert_eq!(genre_label("polka"), "Polka");
        assert_eq!(composer_label("  William Byrd (c.1540-1623) "), "William Byrd");
        assert_eq!(composer_label("Trad."), "Traditionell / anonym");
        assert_eq!(composer_label("anon."), "Traditionell / anonym");
        assert_eq!(composer_label("Ludwig  van Beethoven"), "Ludwig van Beethoven");
    }
}
