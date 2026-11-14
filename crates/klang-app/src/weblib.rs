//! The library of the browser version: a package on a server (Zenodo), read
//! by HTTP range, plus the person's own scores. Both are searched together –
//! the own scores are layered over the package ([`AnyIndex::layered`]), like
//! the desktop layers single added scores over a big index.
//!
//! * Package: [`attach`] opens it from its manifest; the files are
//!   [`RemoteFile`]s, whose missing ranges the web shell fetches
//!   ([`klang_music::source::remote`]).
//! * Own scores: MusicXML/MXL files in `…/Bibliothek/Eigene/` of the core's file
//!   tree (the browser keeps it in OPFS), indexed in the core; the index lies
//!   next to them (`eigene.kwix`, the same format as the desktop's index of
//!   added scores, KWIX02).
//!
//! The functions use only [`crate::vfs`], so they run on every platform; the
//! browser shell (`crates/klang-web`) calls them.

use crate::library::{emit_info, WebLibInfo};
use crate::state::{lock, AppState, CmdResult, Library};
use crate::vfs;
use klang_i18n::{tr, trn};
use klang_music::diskindex::{AnyIndex, DiskIndex};
use klang_music::facets::{score_facets, ScoreFacets};
use klang_music::index::MelodyIndex;
use klang_music::score::{is_score_file, load_score_bytes};
use klang_music::source::remote::{self, RemoteFile};
use klang_music::source::ReadAt;
use klang_music::webpack::WebParts;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Folder of the own scores and their index (below the library root).
fn own_dir(st: &AppState) -> PathBuf {
    st.host().library_root().join("Eigene")
}

fn own_index_path(st: &AppState) -> PathBuf {
    st.host().library_root().join("eigene.kwix")
}

/// Identity of the attached package (its index checksum or version): another
/// package forgets the ranges of the old one.
static ATTACHED: Mutex<Option<String>> = Mutex::new(None);

fn web_info(st: &AppState) -> WebLibInfo {
    lock(&st.library_status).ok().and_then(|s| s.web.clone()).unwrap_or_else(|| WebLibInfo { remote: "none".into(), ..Default::default() })
}

fn set_web_info(st: &AppState, f: impl FnOnce(&mut WebLibInfo)) {
    if let Ok(mut s) = lock(&st.library_status) {
        let w = s.web.get_or_insert_with(|| WebLibInfo { remote: "none".into(), ..Default::default() });
        f(w);
    }
}

/// Takes the library apart: the package (if attached) and the own index.
fn take_parts(st: &AppState) -> (Option<AnyIndex>, MelodyIndex) {
    let taken = lock(&st.library).ok().and_then(|mut l| l.take());
    let Some(lib) = taken else { return (None, MelodyIndex::new()) };
    match lib.index.split() {
        (base, extra) if base.is_remote() => (Some(base), extra.unwrap_or_default()),
        (AnyIndex::Mem(own), _) => (None, own),
        _ => (None, MelodyIndex::new()),
    }
}

/// Puts the library together again and tells the interface.
fn install(st: &AppState, remote: Option<AnyIndex>, own: MelodyIndex) {
    let own_scores = own.facets().len();
    let index = match remote {
        Some(r) => AnyIndex::layered(r, own),
        None => AnyIndex::Mem(own),
    };
    if let Ok(mut l) = lock(&st.library) {
        *l = Some(Library { id: "browser".into(), name: tr!("Library"), path: String::new(), index, packs: Vec::new(), has_meta: true });
    }
    set_web_info(st, |w| w.own_scores = own_scores);
    emit_info(st);
}

/// At start (after the files are restored): loads the own scores.
pub fn init(st: &AppState) {
    let own = load_own(st);
    if let Ok(mut s) = lock(&st.library_status) {
        s.message = None;
        s.loading = false;
    }
    let (remote, _) = take_parts(st);
    install(st, remote, own);
}

fn load_own(st: &AppState) -> MelodyIndex {
    match vfs::read(&own_index_path(st)).and_then(|b| MelodyIndex::from_bytes(&b)) {
        Ok(m) => m,
        // no index (yet) or a damaged one: rebuild from the files
        Err(_) => rebuild_own(st, &[]).0,
    }
}

fn save_own(st: &AppState, own: &MelodyIndex) -> CmdResult<()> {
    let bytes = own.to_bytes().map_err(|e| e.to_string())?;
    vfs::create_dir_all(&st.host().library_root()).map_err(|e| e.to_string())?;
    vfs::write(&own_index_path(st), &bytes).map_err(|e| e.to_string())
}

/// Own scores, file by file.
fn own_files(st: &AppState) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = vfs::list(&own_dir(st)).unwrap_or_default().into_iter().filter(|e| !e.is_dir && is_score_file(&e.path)).map(|e| e.path).collect();
    files.sort();
    files
}

/// Adds one score to the own index; `facets` (if known) saves the analysis.
fn index_score(own: &mut MelodyIndex, path: &Path, bytes: &[u8], facets: Option<ScoreFacets>) -> Result<bool, String> {
    let name = path.display().to_string();
    let score = load_score_bytes(&name, bytes).map_err(|e| format!("{}: {e}", file_name(path)))?;
    let first = own.doc_count() as u32;
    if own.add_score(&score, &name) == 0 {
        return Ok(false);
    }
    let mut f = facets.unwrap_or_else(|| score_facets(&score, first, None));
    f.doc = first;
    own.add_facets(f);
    Ok(true)
}

/// Builds the own index from the files, reusing known facets (source → facets,
/// no new analysis). Returns the index and the files that couldn't be read.
fn rebuild_own(st: &AppState, known: &[(String, ScoreFacets)]) -> (MelodyIndex, Vec<String>) {
    let mut own = MelodyIndex::new();
    let mut failed = Vec::new();
    for path in own_files(st) {
        let source = path.display().to_string();
        let reuse = known.iter().find(|(s, _)| *s == source).map(|(_, f)| f.clone());
        if let Err(e) = vfs::read(&path).map_err(|e| e.to_string()).and_then(|b| index_score(&mut own, &path, &b, reuse)) {
            failed.push(e);
        }
    }
    own.compact();
    (own, failed)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A name for the file in the own folder: only harmless characters.
fn clean_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "partitur.musicxml".into() } else { s }
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddResult {
    /// Scores added
    pub added: usize,
    /// One line per file that wasn't added (why)
    pub skipped: Vec<String>,
    /// Summary for a notification
    pub message: String,
}

/// Adds MusicXML/MXL files (paths in the core's file tree, e.g. dropped files
/// in `/Klangwelt/Import`) to the own scores: they move into the own folder
/// and are indexed. Files that are already there (same content) are skipped.
pub fn add(st: &AppState, paths: &[String]) -> CmdResult<AddResult> {
    let dir = own_dir(st);
    vfs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (remote, mut own) = take_parts(st);
    let mut result = AddResult::default();
    for p in paths {
        let src = PathBuf::from(p);
        let name = clean_name(&file_name(&src));
        if !is_score_file(Path::new(&name)) {
            result.skipped.push(tr!("{path}: not a score (MusicXML, MXL, **kern)", path = name));
            continue;
        }
        let bytes = match vfs::read(&src) {
            Ok(b) => b,
            Err(e) => {
                result.skipped.push(format!("{name}: {e}"));
                continue;
            }
        };
        // same name: the same file again, or another one (→ new name)
        let mut target = dir.join(&name);
        let mut n = 2;
        let mut same = false;
        while vfs::exists(&target) {
            if vfs::read(&target).map(|b| b == bytes).unwrap_or(false) {
                same = true;
                break;
            }
            let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
            target = dir.join(format!("{stem} ({n}).{ext}"));
            n += 1;
        }
        if same {
            result.skipped.push(tr!("\"{name}\" is already in \"{library}\".", name = name, library = tr!("My scores")));
            let _ = remove_import(st, &src);
            continue;
        }
        match index_score(&mut own, &target, &bytes, None) {
            Ok(true) => {
                vfs::write(&target, &bytes).map_err(|e| e.to_string())?;
                let _ = remove_import(st, &src);
                result.added += 1;
            }
            Ok(false) => result.skipped.push(tr!("{file}: no voice is long enough to search (at least 8 notes).", file = name)),
            Err(e) => result.skipped.push(e),
        }
    }
    own.compact();
    let saved = save_own(st, &own);
    install(st, remote, own);
    saved?;
    result.message = match (result.added, result.skipped.len()) {
        (0, 0) => String::new(),
        (n, 0) => trn!(n, "Added one score to \"My scores\".", "Added {n} scores to \"My scores\"."),
        (n, _) => {
            let first = trn!(n, "Added one score to \"My scores\".", "Added {n} scores to \"My scores\".");
            format!("{first} {}", result.skipped.join(" · "))
        }
    };
    Ok(result)
}

/// Dropped files lie in the import folder only for the way into the library.
fn remove_import(st: &AppState, src: &Path) -> std::io::Result<()> {
    let import = st.host().library_root().parent().map(|p| p.join("Import"));
    if import.is_some_and(|d| src.starts_with(d)) {
        vfs::remove_file(src)?;
    }
    Ok(())
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OwnScore {
    pub file: String,
    pub path: String,
    pub doc: u32,
    pub title: String,
    pub composer: String,
    pub bytes: u64,
}

/// The own scores (for the management view).
pub fn list(st: &AppState) -> CmdResult<Vec<OwnScore>> {
    let lib = lock(&st.library)?;
    let Some(lib) = lib.as_ref() else { return Ok(Vec::new()) };
    let offset = lib.index.extra_offset().filter(|_| lib.index.is_remote()).unwrap_or(0);
    let facets = lib.index.facets();
    let sizes: Vec<(String, u64)> = vfs::list(&own_dir(st)).unwrap_or_default().into_iter().map(|e| (e.path.display().to_string(), e.len)).collect();
    let mut out: Vec<OwnScore> = facets
        .iter()
        .filter(|f| f.doc >= offset)
        .filter_map(|f| {
            let info = lib.index.doc(f.doc)?;
            Some(OwnScore {
                file: file_name(Path::new(&info.source)),
                bytes: sizes.iter().find(|(p, _)| *p == info.source).map(|s| s.1).unwrap_or(0),
                path: info.source,
                doc: f.doc,
                title: if f.title.is_empty() { tr!("(untitled)") } else { f.title.clone() },
                composer: f.composer.clone(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(out)
}

/// Removes an own score (file and index entry).
pub fn remove(st: &AppState, path: &str) -> CmdResult<Vec<OwnScore>> {
    let p = PathBuf::from(path);
    if !p.starts_with(own_dir(st)) {
        return Err(tr!("{path} doesn't exist", path = path));
    }
    let (remote, old) = take_parts(st);
    // keep the analysis of the other scores
    let known: Vec<(String, ScoreFacets)> = old.facets().iter().filter_map(|f| old.doc(f.doc).map(|d| (d.source.clone(), f.clone()))).collect();
    let removed = vfs::remove_file(&p).map_err(|e| e.to_string());
    let (own, _failed) = rebuild_own(st, &known);
    let saved = save_own(st, &own);
    install(st, remote, own);
    removed?;
    saved?;
    list(st)
}

// ---------------------------------------------------------------- the package

#[derive(Deserialize)]
struct ManifestFile {
    name: String,
    size: u64,
    #[serde(default)]
    sha256: String,
}

/// Opens the package described by its manifest (`klangwelt-manifest.json`)
/// and layers the own scores over it. Needs a few rounds of data (header,
/// provenance) – see [`klang_music::source::remote`].
pub fn attach(st: &AppState, manifest: &Value) -> CmdResult<WebLibInfo> {
    let files = manifest.get("files").ok_or_else(|| tr!("Index file incomplete or corrupted"))?;
    let file = |key: &str| files.get(key).and_then(|v| serde_json::from_value::<ManifestFile>(v.clone()).ok());
    let index = file("index").ok_or_else(|| tr!("Index file incomplete or corrupted"))?;
    let id = format!("{}:{}:{}", index.name, index.size, index.sha256);
    {
        let mut attached = ATTACHED.lock().unwrap_or_else(|e| e.into_inner());
        if attached.as_deref() != Some(id.as_str()) {
            remote::reset();
            *attached = Some(id);
        }
    }
    let open = |f: Option<ManifestFile>| -> Option<Box<dyn ReadAt>> { f.map(|f| Box::new(RemoteFile::new(&f.name, f.size)) as Box<dyn ReadAt>) };
    let count = |k: &str| manifest.get(k).and_then(Value::as_u64).unwrap_or(0);
    let chord_scores = manifest.get("chordScores").and_then(|v| serde_json::from_value::<[u32; 2]>(v.clone()).ok());
    let web = WebParts::new(open(file("text")), open(file("facets")), open(file("chords")), count("scores") as usize, chord_scores);
    let index = DiskIndex::open_remote(Box::new(RemoteFile::new(&index.name, index.size)), web, open(file("scores"))).map_err(|e| e.to_string())?;
    let (_, own) = take_parts(st);
    let text = |k: &str| manifest.get(k).and_then(Value::as_str).map(str::to_string);
    let sources: Vec<String> = manifest
        .get("sources")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|s| s.get("attribution").or(s.get("name")).and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default();
    let scores = index.score_count();
    install(st, Some(AnyIndex::Disk(index)), own);
    set_web_info(st, |w| {
        w.remote = "ready".into();
        w.remote_title = text("title");
        w.remote_version = text("version");
        w.remote_scores = scores;
        w.remote_url = text("url");
        w.remote_license = text("license");
        w.remote_sources = sources;
        w.error = None;
    });
    emit_info(st);
    Ok(web_info(st))
}

/// The shell tells how the package stands when it can't attach it: "none"
/// (not configured), "loading", "offline", "error" (with a reason).
pub fn set_remote_state(st: &AppState, state: &str, error: Option<String>) -> CmdResult<WebLibInfo> {
    if state != "ready" {
        set_web_info(st, |w| {
            w.remote = state.to_string();
            w.error = error;
        });
    }
    emit_info(st);
    Ok(web_info(st))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host whose library lies in a temporary folder.
    struct TempHost(PathBuf);

    impl crate::Host for TempHost {
        fn emit_json(&self, _event: &str, _json: String) {}
        fn projects_dir(&self) -> PathBuf {
            self.0.join("Projekte")
        }
        fn library_root(&self) -> PathBuf {
            self.0.join("Bibliothek")
        }
    }

    /// Minimal MusicXML: one part, quarter notes (MIDI numbers).
    fn xml(title: &str, composer: &str, notes: &[u8]) -> String {
        let names = ["C", "C", "D", "D", "E", "F", "F", "G", "G", "A", "A", "B"];
        let mut s = format!("<?xml version=\"1.0\"?><score-partwise version=\"4.0\"><work><work-title>{title}</work-title></work><identification><creator type=\"composer\">{composer}</creator></identification><part-list><score-part id=\"P1\"><part-name>Flöte</part-name></score-part></part-list><part id=\"P1\"><measure number=\"1\"><attributes><divisions>1</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>");
        for &m in notes {
            let alter = if [1, 3, 6, 8, 10].contains(&(m % 12)) { "<alter>1</alter>" } else { "" };
            s += &format!("<note><pitch><step>{}</step>{alter}<octave>{}</octave></pitch><duration>1</duration><voice>1</voice></note>", names[(m % 12) as usize], m / 12 - 1);
        }
        s + "</measure></part></score-partwise>"
    }

    #[test]
    fn own_scores_are_added_found_opened_and_removed() {
        let root = std::env::temp_dir().join(format!("klang-weblib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let import = root.join("Import");
        std::fs::create_dir_all(&import).unwrap();
        let st = AppState::new();
        st.set_host(Box::new(TempHost(root.clone())));
        init(&st);
        assert_eq!(web_info(&st).own_scores, 0);

        let tune = [60u8, 62, 64, 65, 67, 67, 69, 69, 67, 65, 64, 62, 60];
        let other = [72u8, 71, 69, 67, 69, 71, 72, 74, 76, 74, 72];
        std::fs::write(import.join("lied.musicxml"), xml("Mein Lied", "Ich", &tune)).unwrap();
        std::fs::write(import.join("zweites.xml"), xml("Zweites Stück", "Du", &other)).unwrap();
        std::fs::write(import.join("kurz.musicxml"), xml("Kurz", "", &tune[..4])).unwrap();
        std::fs::write(import.join("notiz.txt"), "keine Noten").unwrap();
        let paths: Vec<String> = ["lied.musicxml", "zweites.xml", "kurz.musicxml", "notiz.txt"].iter().map(|n| import.join(n).display().to_string()).collect();
        let r = add(&st, &paths).unwrap();
        assert_eq!(r.added, 2, "{r:?}");
        assert_eq!(r.skipped.len(), 2, "{r:?}");
        assert!(!import.join("lied.musicxml").exists(), "moved out of the import folder");
        assert!(root.join("Bibliothek/Eigene/lied.musicxml").is_file());
        assert!(root.join("Bibliothek/eigene.kwix").is_file());
        assert_eq!(web_info(&st).own_scores, 2);

        // the same file once more: skipped
        std::fs::write(import.join("lied.musicxml"), xml("Mein Lied", "Ich", &tune)).unwrap();
        let again = add(&st, &[import.join("lied.musicxml").display().to_string()]).unwrap();
        assert_eq!(again.added, 0);

        // text and melody search, open
        let hits = crate::library::search(&st, Some("lied".into()), None, None, None, None).unwrap();
        let v = serde_json::to_value(&hits).unwrap();
        assert_eq!(v[0]["title"], "Mein Lied");
        // "own" is marked in the browser only (here: a library without a package)
        assert!(v[0].get("origin").is_none());
        let mel = crate::library::search(&st, None, Some("72 71 69 67 69 71 72".into()), None, None, None).unwrap();
        let mel = serde_json::to_value(&mel).unwrap();
        assert_eq!(mel[0]["title"], "Zweites Stück");
        let doc = v[0]["doc"].as_u64().unwrap() as u32;
        let view = serde_json::to_value(crate::library::open(&st, doc, None, None, None).unwrap()).unwrap();
        assert_eq!(view["title"], "Mein Lied");

        // a new start finds the own scores again (index from the file)
        let st2 = AppState::new();
        st2.set_host(Box::new(TempHost(root.clone())));
        init(&st2);
        assert_eq!(list(&st2).unwrap().len(), 2);

        // remove: file and entry go, the other stays searchable
        let own = list(&st).unwrap();
        let lied = own.iter().find(|o| o.title == "Mein Lied").unwrap();
        let rest = remove(&st, &lied.path).unwrap();
        assert_eq!(rest.len(), 1);
        assert!(!root.join("Bibliothek/Eigene/lied.musicxml").exists());
        let mel = serde_json::to_value(crate::library::search(&st, None, Some("72 71 69 67 69 71 72".into()), None, None, None).unwrap()).unwrap();
        assert_eq!(mel[0]["title"], "Zweites Stück");
        let _ = std::fs::remove_dir_all(root);
    }
}
