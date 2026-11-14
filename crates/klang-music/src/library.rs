//! Bibliothek aufbauen: MusicXML/MXL-Dateien eines Ordners parallel lesen und
//! ihre Stimmen in einen Index auf der Platte schreiben ([`IndexWriter`]).

use crate::chordstats::{score_progression, Progression};
use crate::diskindex::{source_key, IndexWriter};
use crate::facets::{score_facets, ScoreFacets};
use crate::index::DocInfo;
use crate::pdmx::{load_meta, PdmxMeta, META_FILE};
use crate::melody::{skyline, MelodyNote};
use crate::score::{is_score_file, load_score, Score, Voice};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

#[derive(Clone, Debug, Default)]
pub struct BuildStats {
    pub files: usize,
    pub scores: usize,
    pub voices: usize,
    pub failed: usize,
    /// Die ersten Fehlermeldungen (Datei: Grund).
    pub errors: Vec<String>,
    /// Verschiedene n-Gramme und Fundstellen im fertigen Index.
    pub grams: usize,
    pub postings: usize,
}

/// Alle Partituren unterhalb von `dir` (MusicXML, **kern, DCML-Notentabellen).
pub fn collect_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if is_score_file(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Bezeichnung einer Stimme, wie sie im Index steht.
pub fn voice_label(v: &Voice) -> String {
    format!("System {} · Stimme {}", v.staff, v.voice)
}

/// Stimmen einer Partitur als Index-Einträge (Oberstimme je Stimme).
pub fn score_entries(score: &Score, source: &str) -> Vec<(DocInfo, Vec<MelodyNote>)> {
    score
        .parts
        .iter()
        .flat_map(|p| p.voices.iter().map(move |v| (p, v)))
        .map(|(p, v)| {
            let info = DocInfo {
                source: source.to_string(),
                title: score.title.clone().unwrap_or_default(),
                composer: score.composer.clone().unwrap_or_default(),
                part: p.name.clone(),
                voice: voice_label(v),
                ..Default::default()
            };
            (info, skyline(&v.notes))
        })
        .collect()
}

/// Baut den Index für `dir` und schreibt ihn nach `out` (Format 4, auf der
/// Platte). Dateien werden parallel gelesen; `progress(fertig, gesamt)` kommt
/// regelmäßig – bei `fertig == gesamt` wird der Index zusammengeführt.
/// `cancel` bricht ab (Fehler `Interrupted`).
pub fn build_index_to(dir: &Path, out: &Path, progress: impl Fn(usize, usize), cancel: &AtomicBool) -> std::io::Result<BuildStats> {
    let files = collect_files(dir)?;
    // PDMX-Metadaten (Genre, Komponist, Schwierigkeit), falls heruntergeladen.
    let meta = load_meta(&dir.join(META_FILE)).unwrap_or_default();
    build_files_to(&files, &meta, out, progress, cancel)
}

/// Dateien und Ordner (rekursiv) zu einer Liste von Partituren, ohne Doppelte;
/// dazu die Genre/Komponist-Angaben der beteiligten Ordner. Nicht Gefundenes
/// steht in der dritten Liste.
pub fn gather(paths: &[PathBuf]) -> (Vec<PathBuf>, HashMap<String, PdmxMeta>, Vec<String>) {
    let (mut files, mut missing) = (Vec::new(), Vec::new());
    let mut meta_dirs: Vec<PathBuf> = Vec::new();
    for p in paths {
        if p.is_dir() {
            match collect_files(p) {
                Ok(list) => {
                    for f in &list {
                        if let Some(d) = f.parent() {
                            meta_dirs.push(d.to_path_buf());
                        }
                    }
                    meta_dirs.push(p.clone());
                    files.extend(list);
                }
                Err(e) => missing.push(format!("{}: {e}", p.display())),
            }
        } else if p.is_file() && is_score_file(p) {
            if let Some(d) = p.parent() {
                meta_dirs.push(d.to_path_buf());
            }
            files.push(p.clone());
        } else if p.is_file() {
            missing.push(klang_i18n::tr!("{path}: not a score (MusicXML, MXL, **kern)", path = p.display()));
        } else {
            missing.push(klang_i18n::tr!("{path}: not found", path = p.display()));
        }
    }
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(source_key(&f.display().to_string())));
    meta_dirs.sort();
    meta_dirs.dedup();
    let mut meta = HashMap::new();
    for d in meta_dirs {
        if let Ok(m) = load_meta(&d.join(META_FILE)) {
            meta.extend(m);
        }
    }
    (files, meta, missing)
}

/// Wie [`build_index_to`], für eine Liste von Partituren; `meta` liefert
/// Genre/Komponist je Dateiname (ohne Endung).
pub fn build_files_to(files: &[PathBuf], meta: &HashMap<String, PdmxMeta>, out: &Path, progress: impl Fn(usize, usize), cancel: &AtomicBool) -> std::io::Result<BuildStats> {
    let total = files.len();
    let mut stats = BuildStats { files: total, ..Default::default() };
    let tmp = out.with_extension("kwix-tmp");
    let mut index = IndexWriter::new(&tmp)?;
    progress(0, total);

    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    type Extracted = Result<(Vec<(DocInfo, Vec<MelodyNote>)>, ScoreFacets, Option<Progression>), String>;
    let (tx, rx) = mpsc::sync_channel::<Extracted>(threads * 4);
    // Schreibfehler beenden auch die lesenden Threads.
    let abort = AtomicBool::new(false);
    let abort = &abort;
    let mut write_error: Option<std::io::Error> = None;
    std::thread::scope(|s| {
        for chunk in files.chunks(total.div_ceil(threads).max(1)) {
            let tx = tx.clone();
            s.spawn(move || {
                for path in chunk {
                    if cancel.load(Ordering::Relaxed) || abort.load(Ordering::Relaxed) {
                        return;
                    }
                    let res = load_score(path)
                        .map(|score| {
                            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                            let facets = score_facets(&score, 0, meta.get(stem));
                            let progression = score_progression(&score);
                            (score_entries(&score, &path.display().to_string()), facets, progression)
                        })
                        .map_err(|e| format!("{}: {e}", path.display()));
                    if tx.send(res).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        for (i, res) in rx.iter().enumerate() {
            match res {
                Ok((list, mut facets, progression)) => {
                    if write_error.is_some() {
                        continue;
                    }
                    stats.scores += 1;
                    let mut first = None;
                    for (info, line) in list {
                        match index.add_melody(info, &line) {
                            Ok(Some(doc)) => {
                                stats.voices += 1;
                                first.get_or_insert(doc);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                abort.store(true, Ordering::Relaxed);
                                write_error = Some(e);
                                break;
                            }
                        }
                    }
                    // Nur Partituren mit durchsuchbarer Stimme kommen in den Baum (zum Öffnen nötig).
                    if let Some(doc) = first {
                        facets.doc = doc;
                        if let Some(p) = progression {
                            index.add_progression(doc, &p, &facets.composer);
                        }
                        index.add_facets(facets);
                    }
                }
                Err(e) => {
                    stats.failed += 1;
                    if stats.errors.len() < 20 {
                        stats.errors.push(e);
                    }
                }
            }
            if (i + 1) % 25 == 0 || i + 1 == total {
                progress(i + 1, total);
            }
        }
    });
    if let Some(e) = write_error {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, klang_i18n::tr!("canceled")));
    }
    let written = index.finish(out, cancel)?;
    stats.grams = written.keys as usize;
    stats.postings = written.postings as usize;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diskindex::AnyIndex;
    use crate::score::tests::make_xml;

    #[test]
    fn builds_from_a_folder_with_progress_and_errors() {
        let dir = std::env::temp_dir().join(format!("klang-lib-test-{}", std::process::id()));
        let sub = dir.join("unterordner");
        std::fs::create_dir_all(&sub).unwrap();
        let tune: &[(u8, u32)] = &[(60, 2), (62, 2), (64, 2), (65, 2), (67, 4), (65, 2), (64, 2), (62, 4), (60, 8)];
        std::fs::write(dir.join("a.musicxml"), make_xml("Erstes", &[("Flöte", tune)])).unwrap();
        std::fs::write(sub.join("b.xml"), make_xml("Zweites", &[("Geige", tune), ("Cello", tune)])).unwrap();
        std::fs::write(dir.join("kaputt.musicxml"), "<score-partwise><part").unwrap();
        std::fs::write(dir.join("notiz.txt"), "keine Noten").unwrap();

        let calls = std::sync::Mutex::new(Vec::new());
        let out = dir.join("klangwelt-bibliothek.kwix");
        let stats = build_index_to(&dir, &out, |d, t| calls.lock().unwrap().push((d, t)), &AtomicBool::new(false)).unwrap();
        assert_eq!(stats.files, 3);
        assert_eq!(stats.scores, 2);
        assert_eq!(stats.voices, 3);
        assert_eq!(stats.failed, 1);
        assert!(stats.grams > 0 && stats.postings > 0);
        let index = AnyIndex::load(&out).unwrap();
        assert!(matches!(index, AnyIndex::Disk(_)));
        assert!(!out.with_extension("kwix-tmp").exists(), "Zwischendateien weg");
        assert_eq!(index.doc_count(), 3);
        assert_eq!(index.score_count(), 2);
        assert_eq!(calls.lock().unwrap().last(), Some(&(3, 3)));
        assert_eq!(index.search_text("zweit", 10).len(), 1, "eine Partitur");
        assert_eq!(index.search_text("cello", 10).len(), 1);
        let q: Vec<MelodyNote> = [60u8, 62, 64, 65, 67, 65, 64].iter().enumerate().map(|(i, &m)| MelodyNote { onset: i as f64, duration: 1.0, midi: m }).collect();
        assert!(!index.search(&q, 5).is_empty());
        // Merkmale je Partitur für den Baum, „doc“ zeigt auf eine Stimme der Partitur
        assert_eq!(index.facets().len(), 2);
        let zweites = index.facets().iter().find(|f| f.title == "Zweites").unwrap();
        assert_eq!(zweites.ensemble, "Duo");
        assert_eq!(zweites.families, vec!["Streicher".to_string()]);
        assert!(index.doc(zweites.doc).unwrap().title == "Zweites");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn index_carries_chord_statistics() {
        use crate::chordstats::Rel;
        use crate::harmony::parse_roman;
        use crate::theory::Key;
        let dir = std::env::temp_dir().join(format!("klang-lib-akk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Choral C – F – G7 – C (und einmal mit Trugschluss G7 – a); Stimmen lang genug für den Index
        let rep = |v: &[(u8, u32)]| -> Vec<(u8, u32)> { v.iter().cycle().take(v.len() * 3).copied().collect() };
        let s = rep(&[(72, 4), (72, 4), (74, 4), (72, 4)]);
        let a = rep(&[(67, 4), (69, 4), (71, 4), (67, 4)]);
        let t = rep(&[(64, 4), (65, 4), (65, 4), (64, 4)]);
        let b = rep(&[(48, 4), (41, 4), (43, 4), (48, 4)]);
        let sd = rep(&[(72, 4), (72, 4), (74, 4), (72, 4)]);
        let ad = rep(&[(67, 4), (69, 4), (71, 4), (69, 4)]);
        let td = rep(&[(64, 4), (65, 4), (65, 4), (64, 4)]);
        let bd = rep(&[(48, 4), (41, 4), (43, 4), (45, 4)]);
        for (name, parts) in [("Choral eins", [&s, &a, &t, &b]), ("Choral zwei", [&s, &a, &t, &b]), ("Trug", [&sd, &ad, &td, &bd])] {
            let xml = make_xml(name, &[("Sopran", parts[0]), ("Alt", parts[1]), ("Tenor", parts[2]), ("Bass", parts[3])]);
            std::fs::write(dir.join(format!("{name}.musicxml")), xml).unwrap();
        }
        let out = dir.join("klangwelt-bibliothek.kwix");
        build_index_to(&dir, &out, |_, _| {}, &AtomicBool::new(false)).unwrap();
        let index = AnyIndex::load(&out).unwrap();
        let stats = index.chords().expect("Akkordstatistik im Index");
        assert_eq!(stats.scores, [3, 0]);
        let k = Key { tonic: 0, minor: false, confidence: 1.0 };
        let r = |x: &str| Rel::of(&parse_roman(x, &k).unwrap(), &k);
        let to_i = stats.follow(false, r("V7"), r("I")).unwrap();
        let to_vi = stats.follow(false, r("V7"), r("vi")).unwrap();
        assert!(to_i.count > to_vi.count && to_vi.count > 0, "{to_i:?} {to_vi:?}");
        assert!((to_i.share + to_vi.share - 1.0).abs() < 1e-6);
        let ex = &to_vi.examples[0];
        assert_eq!(index.doc(ex.doc).unwrap().title, "Trug");
        assert!(ex.end > ex.start && ex.bar >= 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_and_folders_join_a_library_by_merging() {
        use crate::diskindex::merge;
        let root = std::env::temp_dir().join(format!("klang-lib-dazu-{}", std::process::id()));
        let (dir, own) = (root.join("pdmx"), root.join("eigene"));
        std::fs::create_dir_all(own.join("sätze")).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let tune: &[(u8, u32)] = &[(60, 2), (62, 2), (64, 2), (65, 2), (67, 4), (65, 2), (64, 2), (62, 4), (60, 8)];
        let other: &[(u8, u32)] = &[(69, 4), (64, 2), (69, 2), (72, 4), (71, 2), (69, 2), (68, 4), (69, 8), (76, 4)];
        std::fs::write(dir.join("a.musicxml"), make_xml("Grundstück", &[("Flöte", tune)])).unwrap();
        std::fs::write(own.join("souvenir.musicxml"), make_xml("Souvenir", &[("Violine", other), ("Cello", tune)])).unwrap();
        std::fs::write(own.join("sätze").join("satz.musicxml"), make_xml("Satz", &[("Bratsche", other)])).unwrap();
        std::fs::write(own.join("notiz.txt"), "keine Noten").unwrap();
        let lib = root.join("bibliothek.kwix");
        build_index_to(&dir, &lib, |_, _| {}, &AtomicBool::new(false)).unwrap();

        // Ordner und darin liegende Datei doppelt angegeben, dazu Fehlendes
        let (files, _meta, missing) = gather(&[own.clone(), own.join("souvenir.musicxml"), own.join("notiz.txt"), own.join("fehlt.mxl")]);
        assert_eq!(files.len(), 2, "{files:?}");
        assert_eq!(missing.len(), 2, "{missing:?}");
        let new = root.join("neu.kwix");
        let stats = build_files_to(&files, &HashMap::new(), &new, |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!((stats.scores, stats.voices), (2, 3));

        let out = root.join("bibliothek.neu.kwix");
        let (a, b) = (AnyIndex::load(&lib).unwrap(), AnyIndex::load(&new).unwrap());
        let m = merge(&[&a, &b], &out, true, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!((m.scores, m.docs, m.skipped_scores), (3, 4, 0));
        let index = AnyIndex::load(&out).unwrap();
        assert!(matches!(index, AnyIndex::Disk(_)), "eine einzige Datei");
        assert_eq!(index.score_count(), 3);
        assert_eq!(index.search_text("souvenir", 10)[0].1.title, "Souvenir");
        let q: Vec<MelodyNote> = [69u8, 64, 69, 72, 71, 69, 68].iter().enumerate().map(|(i, &m)| MelodyNote { onset: i as f64, duration: 1.0, midi: m }).collect();
        let best = &index.search(&q, 5)[0];
        assert!(best.info.title == "Souvenir" || best.info.title == "Satz", "{best:?}");

        // Nochmal dazu: alles schon enthalten
        let again = AnyIndex::load(&new).unwrap();
        let twice = merge(&[&index, &again], &root.join("zweimal.kwix"), true, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!((twice.scores, twice.skipped_scores), (3, 2));
        let _ = std::fs::remove_dir_all(root);
    }
}
