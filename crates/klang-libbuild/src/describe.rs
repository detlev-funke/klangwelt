//! The package's description: `README.md` (in the package and as the text of
//! the Zenodo record) and `zenodo.json` (metadata for the upload form).
//!
//! Both are made from the package itself – the manifest and the provenance in
//! the index header – so `klang-libbuild describe` can write them again for a
//! finished package without building it again.

use crate::sources::Config;
use klang_music::diskindex::read_provenance;
use klang_music::webpack;
use serde_json::{json, Value};
use std::fs::File;
use std::io;
use std::path::Path;

/// The file formats, copied into every README (headings one level down).
const FORMAT: &str = include_str!("../../../docs/LIBRARY-FORMAT.md");
const REPO: &str = "https://github.com/detlev-funke/klangwelt";

/// Writes `README.md` and `zenodo.json` into the package folder `dir`.
pub fn write(dir: &Path, cfg: &Config) -> io::Result<()> {
    let manifest: Value = serde_json::from_slice(&std::fs::read(dir.join(webpack::MANIFEST_FILE))?).map_err(io::Error::other)?;
    let prov = read_provenance(&File::open(dir.join(webpack::INDEX_FILE))?)?.ok_or_else(|| io::Error::other("the index carries no provenance"))?;
    let prov: Value = serde_json::from_str(&prov).map_err(io::Error::other)?;
    std::fs::write(dir.join("README.md"), readme(&manifest, &prov))?;
    let meta = serde_json::to_string_pretty(&zenodo(cfg, &manifest, &prov)).map_err(io::Error::other)?;
    std::fs::write(dir.join("zenodo.json"), format!("{meta}\n"))?;
    Ok(())
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn count(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

/// 76552 → "76,552".
fn num(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.1} kB", b / 1e3)
    } else {
        format!("{bytes} bytes")
    }
}

fn sources(prov: &Value) -> &[Value] {
    prov["sources"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// "1,460 scores with 7,182 voices. Left out: …"
fn counts_line(c: &Value) -> String {
    format!(
        "- **Included:** {} scores with {} searchable voices. Left out: {} with the same melodic content as a score included before, {} without a voice of at least 8 notes, {} that could not be read.\n",
        num(count(c, "scores")),
        num(count(c, "voices")),
        num(count(c, "duplicates")),
        num(count(c, "tooShort")),
        num(count(c, "failed"))
    )
}

fn file_line(f: &Value) -> String {
    format!("`{}` ({} bytes, MD5 `{}`, SHA-256 `{}`)", text(f, "name"), num(count(f, "size")), text(f, "md5"), text(f, "sha256"))
}

/// What a source contributed and how, for the section "Derived work".
fn source_section(src: &Value) -> String {
    let c = &src["counts"];
    let fetched = &src["fetched"];
    let id = text(src, "id");
    let mut s = format!("### {}\n\n", text(src, "collection"));
    match text(src, "kind") {
        "pdmx" => {
            s += &format!(
                "{} <{}> (Zenodo record {}). PDMX is licensed under CC BY 4.0. This package is an adaptation of PDMX in the sense of that license: it takes a selection of PDMX's scores and adds index data computed from them. The changes are listed here.\n\n",
                text(src, "attribution"),
                text(src, "url"),
                text(src, "record")
            );
            let files: Vec<String> = fetched["files"].as_array().map(|v| v.iter().map(file_line).collect()).unwrap_or_default();
            s += &format!("- **Files used** from the record: {}.\n", files.join(" and "));
            let licenses: Vec<String> = src["licenses"].as_array().map(|v| v.iter().filter_map(Value::as_str).map(|l| format!("`{l}`")).collect()).unwrap_or_default();
            let subset = text(src, "subset");
            s += &format!(
                "- **Selection:** the rows of `PDMX.csv` whose `license` is {}, whose `license_conflict` is not `True`{}: {} of {} rows.\n",
                licenses.join(" or "),
                if subset.is_empty() { String::new() } else { format!(" and that belong to PDMX's own deduplicated subset (`{subset}` is `True`)") },
                num(count(fetched, "acceptedRows")),
                num(count(fetched, "csvRows"))
            );
            s += &counts_line(c);
            s += &format!(
                "- **Unchanged:** every MXL file is stored exactly as it is in `mxl.tar.gz`, under `{id}/<path in mxl.tar.gz without the leading mxl/>` in `{}`. Its file name is the one in the `mxl` column of `PDMX.csv`, which links every score back to its PDMX row.\n",
                webpack::SCORES_FILE
            );
            let names = &src["licenseNames"];
            let shown: Vec<String> = src["licenses"].as_array().map(|v| v.iter().filter_map(Value::as_str).map(|l| format!("\"{}\"", names[l].as_str().unwrap_or(l))).collect()).unwrap_or_default();
            s += &format!(
                "- **Taken from `PDMX.csv`** per score: `genres`, `composer_name` (or `artist_name` when that is empty), `complexity` (shown as difficulty) and `license` (shown as {}).\n",
                shown.join(" or ")
            );
            s += "- **Not included:** PDMX's other files (MusicRender JSON, metadata, PDF and MIDI), the other columns of `PDMX.csv`, and every score with another license or a license conflict.\n";
        }
        "github" => {
            s += &format!("{} Repository <https://github.com/{}> at commit `{}`, license {}.\n\n", text(src, "attribution"), text(src, "repo"), text(src, "commit"), text(src, "license"));
            s += &format!(
                "- **Files used:** the {} files `{}…{}` at that commit (SHA-256 over the sorted list of path and SHA-256 of every file: `{}`).\n",
                num(count(c, "read")),
                text(src, "prefix"),
                text(src, "suffix"),
                text(fetched, "contentSha256")
            );
            s += &counts_line(c);
            s += &format!("- **Unchanged:** every file is stored exactly as it is in the repository, under `{id}/<path in the repository>` in `{}`.\n", webpack::SCORES_FILE);
        }
        _ => {
            s += &format!("{} <{}>, license {}.\n\n", text(src, "attribution"), text(src, "url"), text(src, "license"));
            s += &format!(
                "- **Files used:** {} files from the folder `{}` (SHA-256 over the sorted list of path and SHA-256 of every file: `{}`).\n",
                num(count(c, "read")),
                text(src, "path"),
                text(fetched, "contentSha256")
            );
            s += &counts_line(c);
            s += &format!("- **Unchanged:** every file is stored exactly as it is, under `{id}/<path in the folder>` in `{}`.\n", webpack::SCORES_FILE);
        }
    }
    s + "\n"
}

/// The format description with its headings one level down (it becomes a
/// section of the README) and links that work outside the repository.
fn format_section() -> String {
    let mut out = String::new();
    let mut fence = false;
    for (i, line) in FORMAT.lines().enumerate() {
        if i == 0 && line.starts_with("# ") {
            out += "## File formats\n";
            continue;
        }
        if line.starts_with("```") {
            fence = !fence;
        }
        if !fence && line.starts_with('#') {
            out.push('#');
        }
        out += line;
        out.push('\n');
    }
    out.replace("](LIBRARY.md)", &format!("]({REPO}/blob/main/docs/LIBRARY.md)"))
}

/// `README.md` of the package.
pub fn readme(manifest: &Value, prov: &Value) -> String {
    let srcs = sources(prov);
    let pdmx = srcs.iter().find(|s| text(s, "kind") == "pdmx");
    let others: Vec<String> = srcs
        .iter()
        .filter(|s| text(s, "kind") != "pdmx")
        .map(|s| format!("{} from {} ({})", num(count(&s["counts"], "scores")), text(s, "collection"), text(s, "license")))
        .collect();

    let mut s = format!("# {}\n\n", text(manifest, "title"));
    s += &format!(
        "Score library for [Klangwelt]({REPO}): {} scores with {} searchable voices, prepared for melody search (\"hum a tune\"), text search and browsing by genre, composer, mood and more. Klangwelt's browser version reads these files directly from this record with HTTP range requests; it never downloads them whole. Version {}.\n\n",
        num(count(manifest, "scores")),
        num(count(manifest, "voices")),
        text(manifest, "version")
    );
    match pdmx {
        Some(p) => {
            s += &format!(
                "**This dataset is a derived work of [PDMX]({}),** the Public Domain MusicXML dataset that is already published on Zenodo. It contains {} of PDMX's scores, unchanged{}. What this package adds are index files computed from those scores; it contains no new scores. For the complete PDMX dataset (all scores, with PDF, MIDI and metadata), use the PDMX record itself.\n\n",
                text(p, "url"),
                num(count(&p["counts"], "scores")),
                if others.is_empty() { String::new() } else { format!(", and {}", others.join(" and ")) }
            );
        }
        None => s += "This dataset is a compilation: every score is a file of one of the sources below, included unchanged. What this package adds are index files computed from those scores.\n\n",
    }

    s += "## Derived work\n\n";
    s += "Every score in this package is a file of one of the sources below, included byte for byte: no score was changed. Klangwelt computed everything else from those files: the melody index, the text index, the facets (mood, key, meter, tempo, ensemble, texture, harmony and length come from a heuristic analysis of each score) and the chord statistics.";
    if prov["dedupe"].as_bool().unwrap_or(false) {
        let order: Vec<&str> = srcs.iter().map(|s| text(s, "collection")).collect();
        s += &format!(" Scores whose melodic content is identical to a score read before are left out; the sources were read in this order: {}.", order.join(", "));
    }
    s += "\n\n";
    // PDMX first: the package is derived from it
    let mut shown: Vec<&Value> = srcs.iter().collect();
    shown.sort_by_key(|s| text(s, "kind") != "pdmx");
    for src in shown {
        s += &source_section(src);
    }

    s += "## Files\n\n| File | Size | Contents |\n|---|---|---|\n";
    let what = [
        ("index", "Melody index (KWIX04): voice entries, melody features, n-gram occurrence lists, facets, chord statistics. The header carries the provenance (sources, versions, checksums)."),
        ("scores", "The scores themselves (MXL), stored uncompressed in a ZIP so that one score can be read with one HTTP range request."),
        ("text", "Text index: words of title, composer, parts, genre, instruments, key and mood → scores."),
        ("facets", "Facet table for browsing (genre, composer, mood, key, …)."),
        ("chords", "Chord progression statistics (all scores, Baroque, Bach)."),
    ];
    for (key, desc) in what {
        let f = &manifest["files"][key];
        s += &format!("| `{}` | {} | {desc} |\n", text(f, "name"), size(count(f, "size")));
    }
    s += &format!("| `{}` | – | Sizes and checksums (SHA-256, MD5) of the files above, sources and counts. |\n\n", webpack::MANIFEST_FILE);

    let license = text(manifest, "license");
    s += "## License\n\n";
    s += &format!("This package (the index files and the compilation) is licensed under {license}");
    if license == "CC BY 4.0" {
        s += " (<https://creativecommons.org/licenses/by/4.0/>)";
    }
    s += ".";
    if pdmx.is_some() {
        s += " As an adaptation of PDMX it keeps PDMX's license and credits PDMX. The section \"Derived work\" lists what was changed.";
    }
    s += &format!(" {}\n\n", text(manifest, "scoreLicenses"));

    s += "## How to cite\n\n";
    if let Some(p) = pdmx {
        s += &format!("If you use this package, please cite PDMX:\n\n> {} <{}>\n\n", text(p, "attribution"), text(p, "url"));
        s += "and this package by the DOI of its Zenodo record.";
    } else {
        s += "Please cite this package by the DOI of its Zenodo record.";
    }
    let credits: Vec<String> = srcs.iter().filter(|s| text(s, "kind") != "pdmx").map(|s| format!("{} <{}>", text(s, "attribution"), text(s, "url"))).collect();
    if !credits.is_empty() {
        s += &format!(" Credit for the other sources is welcome:\n\n{}", credits.iter().map(|c| format!("- {c}\n")).collect::<String>());
    } else {
        s += "\n";
    }
    s += "\n";

    s += &format_section();
    s += "\n## Build\n\n";
    s += &format!(
        "Built with {} from the [Klangwelt repository]({REPO}) (`docs/LIBRARY.md` there). Building again from the same sources with `--date {}` gives byte-identical files. `klang-libbuild verify` checks a package against its manifest.\n",
        text(manifest, "builder"),
        text(manifest, "version")
    );
    s
}

/// Metadata for the Zenodo upload form.
pub fn zenodo(cfg: &Config, manifest: &Value, prov: &Value) -> Value {
    let srcs = sources(prov);
    let related: Vec<Value> = srcs.iter().map(|s| json!({ "identifier": text(s, "url"), "relation": "isDerivedFrom", "resource_type": "dataset" })).collect();
    let parts: Vec<String> = srcs
        .iter()
        .map(|s| format!("{} scores from <a href=\"{}\">{}</a> ({})", num(count(&s["counts"], "scores")), text(s, "url"), text(s, "collection"), text(s, "license")))
        .collect();
    let derived = match srcs.iter().find(|s| text(s, "kind") == "pdmx") {
        Some(p) => format!("<p>This dataset is a derived work of PDMX, which is published on Zenodo: {} <a href=\"{url}\">{url}</a> (CC BY 4.0).</p>", text(p, "attribution"), url = text(p, "url")),
        None => String::new(),
    };
    json!({
        "metadata": {
            "title": format!("{} ({})", text(manifest, "title"), text(manifest, "version")),
            "upload_type": "dataset",
            "description": format!(
                "<p>Score library for Klangwelt: {} scores with {} searchable voices, prepared for melody search, text search and browsing, and readable by HTTP range requests.</p>{derived}<p>It contains {}, all unchanged, and adds index files computed from them: a melody index (KWIX04), a text index, a facet table and chord statistics. README.md describes the selection, the changes and the file formats in detail.</p><p>{}</p>",
                num(count(manifest, "scores")),
                num(count(manifest, "voices")),
                parts.join(", "),
                text(manifest, "scoreLicenses")
            ),
            "creators": cfg.creators.iter().map(|c| json!({ "name": c })).collect::<Vec<_>>(),
            "license": "cc-by-4.0",
            "version": manifest["version"],
            "keywords": ["music", "scores", "MusicXML", "melody search", "PDMX", "OpenScore"],
            "related_identifiers": related,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_sizes_read_well() {
        assert_eq!(num(0), "0");
        assert_eq!(num(999), "999");
        assert_eq!(num(76_552), "76,552");
        assert_eq!(num(1_894_335_797), "1,894,335,797");
        assert_eq!(size(1_137_643_231), "1.14 GB");
        assert_eq!(size(670_710_749), "670.7 MB");
        assert_eq!(size(987_841), "987.8 kB");
    }

    #[test]
    fn the_format_section_sits_one_level_down() {
        let f = format_section();
        assert!(f.starts_with("## File formats\n"));
        assert!(f.contains("\n### `klangwelt-index.kwix`"));
        assert!(f.contains("\n#### Header (112 bytes)"));
        assert!(!f.contains("](LIBRARY.md)"));
        // inside code blocks nothing changes
        assert!(f.contains("\"KWH2\", u8 G"));
    }
}
