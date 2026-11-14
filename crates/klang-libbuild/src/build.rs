//! The build: read the sources one after the other, parse the scores in
//! parallel, write them in a fixed order (the same sources give the same
//! package), then the package files and the manifest.

use crate::hash;
use crate::sources::{read_source, Config, Fetched, Item};
use klang_music::chordstats::{score_progression, Progression};
use klang_music::diskindex::{DiskIndex, IndexWriter};
use klang_music::facets::{score_facets, ScoreFacets};
use klang_music::index::DocInfo;
use klang_music::library::score_entries;
use klang_music::melody::{features, MelodyNote};
use klang_music::score::load_score_bytes;
use klang_music::scorezip::ZipWriter;
use klang_music::webpack::{self, write_chords, write_facet_table, write_text_index};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct Options {
    pub out: PathBuf,
    pub cache: PathBuf,
    /// Only these sources (ids); empty = all.
    pub only: Vec<String>,
    /// At most this many scores per source (trial runs).
    pub limit: Option<usize>,
    /// Date of the package (version, provenance) – fixed for a bit-identical rebuild.
    pub date: String,
    pub jobs: usize,
}

/// A parsed score, ready to be written.
struct Parsed {
    item_name: String,
    bytes: Vec<u8>,
    license: String,
    entries: Vec<(DocInfo, Vec<MelodyNote>)>,
    facets: ScoreFacets,
    progression: Option<Progression>,
    /// Hash of the melodic content (for removing duplicates).
    fingerprint: String,
}

/// Counts per source.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SourceStats {
    pub read: usize,
    pub scores: usize,
    pub voices: usize,
    pub duplicates: usize,
    /// Scores without a voice long enough to search (≥ 8 notes).
    pub too_short: usize,
    pub failed: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

fn parse(item: Item) -> Result<Parsed, String> {
    let score = load_score_bytes(&item.name, &item.bytes).map_err(|e| format!("{}: {e}", item.name))?;
    let facets = score_facets(&score, 0, item.meta.as_ref());
    let progression = score_progression(&score);
    let entries = score_entries(&score, &item.name);
    let mut lines: Vec<Vec<u8>> = entries.iter().filter(|(_, l)| l.len() >= 8).map(|(_, l)| features(l).iter().flat_map(|f| [f.interval as u8, f.rhythm as u8]).collect()).collect();
    lines.sort();
    let mut h = hash::Sha256::new();
    for l in &lines {
        h.update(&(l.len() as u32).to_le_bytes());
        h.update(l);
    }
    Ok(Parsed { item_name: item.name, bytes: item.bytes, license: item.license, entries, facets, progression, fingerprint: h.finish() })
}

/// Result of a build.
pub struct Built {
    pub stats: BTreeMap<String, SourceStats>,
    pub scores: usize,
    pub voices: usize,
    pub seconds: f64,
    pub manifest: Value,
}

fn log(msg: &str) {
    eprintln!("[klang-libbuild] {msg}");
}

pub fn build(cfg: &Config, cfg_dir: &Path, opts: &Options) -> io::Result<Built> {
    let started = Instant::now();
    std::fs::create_dir_all(&opts.out)?;
    let tmp = opts.out.join(".build-tmp");
    let index_path = opts.out.join(webpack::INDEX_FILE);
    let mut index = IndexWriter::new(&tmp)?;
    let zip_part = opts.out.join(format!("{}.part", webpack::SCORES_FILE));
    let mut zip = ZipWriter::new(BufWriter::with_capacity(1 << 20, File::create(&zip_part)?));
    let mut all_facets: Vec<ScoreFacets> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut stats: BTreeMap<String, SourceStats> = BTreeMap::new();
    let mut fetched: BTreeMap<String, Fetched> = BTreeMap::new();
    let (mut scores, mut voices) = (0usize, 0usize);
    let sources: Vec<_> = cfg.sources.iter().filter(|s| opts.only.is_empty() || opts.only.contains(&s.id)).collect();
    if sources.is_empty() {
        return Err(io::Error::other("no source selected"));
    }

    for src in &sources {
        log(&format!("source {} ({})", src.id, src.collection));
        let st = stats.entry(src.id.clone()).or_default();
        let t0 = Instant::now();
        let mut last_report = Instant::now();
        let mut write_error: Option<io::Error> = None;
        // reader → parsers → writer (here), in the order of the source
        let result: io::Result<Fetched> = std::thread::scope(|s| {
            let (item_tx, item_rx) = sync_channel::<(u64, Item)>(opts.jobs * 4);
            let item_rx: Arc<Mutex<Receiver<(u64, Item)>>> = Arc::new(Mutex::new(item_rx));
            let (res_tx, res_rx) = sync_channel::<(u64, Result<Parsed, String>)>(opts.jobs * 4);
            let reader = s.spawn(move || {
                let mut seq = 0u64;
                let r = read_source(src, cfg_dir, &opts.cache, opts.limit, &log, &mut |item| {
                    let ok = item_tx.send((seq, item)).is_ok();
                    seq += 1;
                    Ok(ok)
                });
                drop(item_tx);
                r
            });
            for _ in 0..opts.jobs {
                let rx = item_rx.clone();
                let tx = res_tx.clone();
                s.spawn(move || loop {
                    let next = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    let Ok((seq, item)) = next else { return };
                    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse(item))).unwrap_or_else(|_| Err("parser panicked".into()));
                    if tx.send((seq, parsed)).is_err() {
                        return;
                    }
                });
            }
            drop(res_tx);
            let mut waiting: BTreeMap<u64, Result<Parsed, String>> = BTreeMap::new();
            let mut next = 0u64;
            for (seq, r) in res_rx {
                waiting.insert(seq, r);
                while let Some(r) = waiting.remove(&next) {
                    next += 1;
                    st.read += 1;
                    if write_error.is_some() {
                        continue;
                    }
                    match r {
                        Err(e) => {
                            st.failed += 1;
                            if st.errors.len() < 20 {
                                st.errors.push(e);
                            }
                        }
                        Ok(p) => {
                            if let Err(e) = write_score(p, src.collection.as_str(), cfg.dedupe, &mut seen, &mut zip, &mut index, &mut all_facets, st) {
                                write_error = Some(e);
                            }
                        }
                    }
                    if last_report.elapsed().as_secs() >= 5 {
                        last_report = Instant::now();
                        let rate = st.read as f64 / t0.elapsed().as_secs_f64().max(0.001);
                        log(&format!("{}: {} read, {} scores, {} duplicates, {} failed · {rate:.0}/s", src.id, st.read, st.scores, st.duplicates, st.failed));
                    }
                }
            }
            reader.join().unwrap_or_else(|_| Err(io::Error::other("reader panicked")))
        });
        if let Some(e) = write_error {
            return Err(e);
        }
        fetched.insert(src.id.clone(), result?);
        let st = &stats[&src.id];
        scores += st.scores;
        voices += st.voices;
        log(&format!(
            "{}: {} scores ({} voices) from {} files – {} duplicates, {} too short, {} failed – {:.0} s",
            src.id,
            st.scores,
            st.voices,
            st.read,
            st.duplicates,
            st.too_short,
            st.failed,
            t0.elapsed().as_secs_f64()
        ));
    }

    // provenance in the index header: sources, versions, checksums
    let sources_json: Vec<Value> = sources
        .iter()
        .map(|s| {
            let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
            v["fetched"] = serde_json::to_value(&fetched[&s.id]).unwrap_or(Value::Null);
            v["counts"] = serde_json::to_value(&stats[&s.id]).unwrap_or(Value::Null);
            if let Some(c) = v.get_mut("counts").and_then(Value::as_object_mut) {
                c.remove("errors");
            }
            v
        })
        .collect();
    let provenance = json!({
        "format": "KWIX04",
        "package": cfg.title,
        "version": opts.date,
        "builder": format!("klang-libbuild {}", env!("CARGO_PKG_VERSION")),
        "license": cfg.license,
        "scores": scores,
        "voices": voices,
        "dedupe": cfg.dedupe,
        "sources": sources_json,
    });
    index.set_provenance(&serde_json::to_string(&provenance).map_err(io::Error::other)?);

    log("writing the scores' ZIP …");
    let w = zip.finish()?;
    w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(&zip_part, opts.out.join(webpack::SCORES_FILE))?;
    log("merging the index …");
    index.finish(&index_path, &AtomicBool::new(false))?;
    log("writing text index, facet table, chord statistics …");
    let disk = DiskIndex::open(&index_path)?;
    let groups: Vec<_> = (0..3).filter_map(|g| disk.chords_group(g).cloned()).collect();
    let chord_scores = disk.chords().map(|c| c.scores);
    std::fs::write(opts.out.join(webpack::TEXT_FILE), write_text_index(&all_facets))?;
    std::fs::write(opts.out.join(webpack::FACETS_FILE), write_facet_table(&all_facets))?;
    std::fs::write(opts.out.join(webpack::CHORDS_FILE), write_chords(&groups))?;
    drop(disk);

    let mut files = serde_json::Map::new();
    for (key, name) in [("index", webpack::INDEX_FILE), ("scores", webpack::SCORES_FILE), ("text", webpack::TEXT_FILE), ("facets", webpack::FACETS_FILE), ("chords", webpack::CHORDS_FILE)] {
        let (sha256, md5, size) = hash::hashes(BufReader::new(File::open(opts.out.join(name))?))?;
        files.insert(key.into(), json!({ "name": name, "size": size, "sha256": sha256, "md5": md5 }));
    }
    let manifest = json!({
        "format": "klangwelt-library",
        "formatVersion": 1,
        "title": cfg.title,
        "version": opts.date,
        "license": cfg.license,
        "scoreLicenses": cfg.score_licenses,
        "scores": scores,
        "voices": voices,
        "chordScores": chord_scores,
        "files": files,
        "sources": sources.iter().map(|s| json!({
            "id": s.id,
            "collection": s.collection,
            "license": s.license,
            "url": s.url,
            "attribution": s.attribution,
            "scores": stats[&s.id].scores,
        })).collect::<Vec<_>>(),
        "builder": format!("klang-libbuild {}", env!("CARGO_PKG_VERSION")),
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(io::Error::other)?;
    std::fs::write(opts.out.join(webpack::MANIFEST_FILE), format!("{text}\n"))?;
    crate::describe::write(&opts.out, cfg)?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(Built { stats, scores, voices, seconds: started.elapsed().as_secs_f64(), manifest })
}

#[allow(clippy::too_many_arguments)]
fn write_score(
    p: Parsed,
    collection: &str,
    dedupe: bool,
    seen: &mut HashSet<String>,
    zip: &mut ZipWriter<BufWriter<File>>,
    index: &mut IndexWriter,
    all_facets: &mut Vec<ScoreFacets>,
    st: &mut crate::build::SourceStats,
) -> io::Result<()> {
    if !p.entries.iter().any(|(_, l)| l.len() >= 8) {
        st.too_short += 1;
        return Ok(());
    }
    if dedupe && !seen.insert(p.fingerprint) {
        st.duplicates += 1;
        return Ok(());
    }
    let at = zip.add(&p.item_name, &p.bytes)?;
    let mut first = None;
    for (mut info, line) in p.entries {
        info.license = p.license.clone();
        info.collection = collection.to_string();
        info.zip = Some(at);
        if let Some(doc) = index.add_melody(info, &line)? {
            st.voices += 1;
            first.get_or_insert(doc);
        }
    }
    let doc = first.expect("checked above");
    let mut facets = p.facets;
    facets.doc = doc;
    if let Some(pr) = p.progression {
        index.add_progression(doc, &pr, &facets.composer);
    }
    index.add_facets(facets.clone());
    all_facets.push(facets);
    st.scores += 1;
    Ok(())
}

/// Checks a package: manifest sizes and checksums, index readable, every ZIP
/// entry reachable through the central directory and through the index.
pub fn verify(dir: &Path) -> io::Result<String> {
    let manifest: Value = serde_json::from_slice(&std::fs::read(dir.join(webpack::MANIFEST_FILE))?).map_err(io::Error::other)?;
    let mut report = String::new();
    for (key, f) in manifest["files"].as_object().ok_or_else(|| io::Error::other("manifest without files"))? {
        let name = f["name"].as_str().unwrap_or("");
        let (sha256, _, size) = hash::hashes(BufReader::new(File::open(dir.join(name))?))?;
        if Some(size) != f["size"].as_u64() || Some(sha256.as_str()) != f["sha256"].as_str() {
            return Err(io::Error::other(format!("{name}: size or checksum differs from the manifest")));
        }
        report += &format!("{key}: {name} ok ({size} bytes)\n");
    }
    let index = DiskIndex::open(&dir.join(webpack::INDEX_FILE))?;
    let zip = File::open(dir.join(webpack::SCORES_FILE))?;
    let entries = klang_music::scorezip::read_directory(&zip)?;
    let offsets: HashSet<u64> = entries.iter().map(|e| e.offset).collect();
    let mut checked = 0;
    for f in index.facets() {
        let info = index.doc(f.doc).ok_or_else(|| io::Error::other(format!("voice {} unreadable", f.doc)))?;
        let at = info.zip.ok_or_else(|| io::Error::other(format!("{}: no ZIP position", info.source)))?;
        if !offsets.contains(&at[0]) {
            return Err(io::Error::other(format!("{}: not in the ZIP directory", info.source)));
        }
        let (name, data) = klang_music::scorezip::read_entry(&zip, at)?;
        if name != info.source {
            return Err(io::Error::other(format!("{}: ZIP entry is {name}", info.source)));
        }
        if checked < 50 {
            load_score_bytes(&name, &data).map_err(|e| io::Error::other(format!("{name}: {e}")))?;
        }
        checked += 1;
    }
    if checked != entries.len() || Some(checked as u64) != manifest["scores"].as_u64() {
        return Err(io::Error::other(format!("{checked} scores in the index, {} in the ZIP, {} in the manifest", entries.len(), manifest["scores"])));
    }
    report += &format!("{checked} scores: index ↔ ZIP consistent; provenance: {}\n", index.provenance().map(|p| p.len()).unwrap_or(0));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{Kind, Source};

    fn xml(title: &str, composer: &str, notes: &[u8]) -> String {
        let names = ["C", "C", "D", "D", "E", "F", "F", "G", "G", "A", "A", "B"];
        let mut s = format!("<?xml version=\"1.0\"?><score-partwise version=\"4.0\"><work><work-title>{title}</work-title></work><identification><creator type=\"composer\">{composer}</creator></identification><part-list><score-part id=\"P1\"><part-name>Violine</part-name></score-part></part-list><part id=\"P1\"><measure number=\"1\"><attributes><divisions>1</divisions><time><beats>3</beats><beat-type>4</beat-type></time></attributes>");
        for &m in notes {
            let alter = if [1, 3, 6, 8, 10].contains(&(m % 12)) { "<alter>1</alter>" } else { "" };
            s += &format!("<note><pitch><step>{}</step>{alter}<octave>{}</octave></pitch><duration>1</duration><voice>1</voice></note>", names[(m % 12) as usize], m / 12 - 1);
        }
        s + "</measure></part></score-partwise>"
    }

    #[test]
    fn builds_a_package_from_a_local_folder_and_it_verifies() {
        let root = std::env::temp_dir().join(format!("klang-libbuild-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("noten");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        let a = [67u8, 64, 64, 65, 62, 62, 60, 62, 64, 65, 67, 67, 67];
        let b = [60u8, 62, 64, 60, 60, 62, 64, 60, 64, 65, 67, 64, 65, 67];
        std::fs::write(src.join("haenschen.musicxml"), xml("Hänschen klein", "Volkslied", &a)).unwrap();
        std::fs::write(src.join("sub").join("jakob.musicxml"), xml("Bruder Jakob", "Volkslied", &b)).unwrap();
        // the same notes under another name: a duplicate
        std::fs::write(src.join("sub").join("kopie.musicxml"), xml("Hänschen (Kopie)", "", &a)).unwrap();
        std::fs::write(src.join("kaputt.musicxml"), "<score-partwise><part").unwrap();
        let cfg = Config {
            title: "Test library".into(),
            license: "CC BY 4.0".into(),
            score_licenses: "Scores: CC0.".into(),
            creators: vec!["Test".into()],
            dedupe: true,
            sources: vec![Source {
                id: "probe".into(),
                collection: "Probe".into(),
                license: "CC0-1.0".into(),
                url: "https://example.org".into(),
                attribution: "Test scores".into(),
                genre: Some("folk".into()),
                kind: Kind::Local { path: "noten".into() },
            }],
        };
        let opts = Options { out: root.join("paket"), cache: root.join("cache"), only: Vec::new(), limit: None, date: "2026-11-14".into(), jobs: 3 };
        let built = build(&cfg, &root, &opts).unwrap();
        let st = &built.stats["probe"];
        assert_eq!((st.read, st.scores, st.duplicates, st.failed), (4, 2, 1, 1), "{st:?}");
        let report = verify(&opts.out).unwrap();
        assert!(report.contains("2 scores"), "{report}");
        let index = DiskIndex::open(&opts.out.join(webpack::INDEX_FILE)).unwrap();
        let prov: Value = serde_json::from_str(index.provenance().unwrap()).unwrap();
        assert_eq!(prov["sources"][0]["id"], "probe");
        assert!(prov["sources"][0]["fetched"]["contentSha256"].as_str().unwrap().len() == 64);
        let hit = &index.search_text("jakob", 5)[0].1;
        assert_eq!((hit.collection.as_str(), hit.license.as_str()), ("Probe", "CC0-1.0"));
        assert_eq!(index.facets()[0].genres, vec!["Volksmusik & Folk".to_string()]);
        // README and zenodo.json come from the package itself and can be written again
        let readme = std::fs::read_to_string(opts.out.join("README.md")).unwrap();
        assert!(readme.contains("2 scores with 2 searchable voices"), "{readme}");
        assert!(readme.contains("Left out: 1 with the same melodic content"), "{readme}");
        assert!(readme.contains("#### Header (112 bytes)"), "{readme}");
        crate::describe::write(&opts.out, &cfg).unwrap();
        assert_eq!(std::fs::read_to_string(opts.out.join("README.md")).unwrap(), readme);
        let meta: Value = serde_json::from_slice(&std::fs::read(opts.out.join("zenodo.json")).unwrap()).unwrap();
        assert_eq!(meta["metadata"]["related_identifiers"][0]["relation"], "isDerivedFrom");
        // the same sources give the same files
        let first: Vec<Vec<u8>> = [webpack::INDEX_FILE, webpack::SCORES_FILE, webpack::TEXT_FILE, webpack::FACETS_FILE].iter().map(|f| std::fs::read(opts.out.join(f)).unwrap()).collect();
        build(&cfg, &root, &Options { jobs: 1, ..opts }).unwrap();
        let out = root.join("paket");
        for (f, bytes) in [webpack::INDEX_FILE, webpack::SCORES_FILE, webpack::TEXT_FILE, webpack::FACETS_FILE].iter().zip(first) {
            assert!(std::fs::read(out.join(f)).unwrap() == bytes, "{f} differs between builds");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
