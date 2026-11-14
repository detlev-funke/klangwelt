//! Sources of the package and how they are fetched: pinned versions
//! (Zenodo record and file checksums, GitHub commits), a download cache, and
//! per source a stream of scores ([`Item`]).

use crate::hash;
use klang_music::fetch;
use klang_music::pdmx::{for_each_record, PdmxMeta};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// The build configuration (`sources.json`).
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Title of the package (manifest, Zenodo).
    pub title: String,
    /// License of the package itself (e.g. "CC BY 4.0").
    pub license: String,
    /// Statement about the scores' licenses (shown as is).
    #[serde(default)]
    pub score_licenses: String,
    /// Creators for the Zenodo record.
    #[serde(default)]
    pub creators: Vec<String>,
    /// Remove scores whose melodic content is identical to an earlier one.
    #[serde(default = "yes")]
    pub dedupe: bool,
    pub sources: Vec<Source>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// Short id (folder in the ZIP, `--only`).
    pub id: String,
    /// Collection name shown with every score (e.g. "OpenScore Lieder").
    pub collection: String,
    /// License of the scores, shown with every score (e.g. "CC0-1.0").
    pub license: String,
    /// Page of the source.
    pub url: String,
    /// Attribution line (Zenodo description, app).
    pub attribution: String,
    /// Raw genre for all scores of the source (PDMX brings its own).
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Kind {
    /// PDMX on Zenodo: `PDMX.csv` (licenses, metadata) and `mxl.tar.gz`.
    Pdmx {
        record: String,
        csv: Pinned,
        mxl: Pinned,
        /// Accepted values of the `license` column (e.g. "publicdomain", "cc-zero").
        licenses: Vec<String>,
        /// Only rows where this column is "True" (e.g. "subset:deduplicated"); none = all.
        #[serde(default)]
        subset: Option<String>,
        /// License per `license` value, shown with the scores.
        #[serde(default, rename = "licenseNames")]
        license_names: HashMap<String, String>,
    },
    /// Files of a GitHub repository at a fixed commit.
    Github { repo: String, commit: String, prefix: String, suffix: String },
    /// A local folder (tests, own collections); relative to the configuration file.
    Local { path: String },
}

/// A file with a known size and MD5 (from the Zenodo record).
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Pinned {
    pub name: String,
    pub size: u64,
    pub md5: String,
}

impl Config {
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))
    }
}

/// A score from a source.
pub struct Item {
    /// Entry name in the ZIP (`<source id>/<path>`).
    pub name: String,
    pub bytes: Vec<u8>,
    /// PDMX metadata (genre, composer, difficulty), or the source's genre.
    pub meta: Option<PdmxMeta>,
    /// License of this score (PDMX: per row).
    pub license: String,
}

/// What was fetched for a source (for manifest and provenance).
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Fetched {
    /// Files with size, SHA-256 and MD5.
    pub files: Vec<FetchedFile>,
    /// SHA-256 over the sorted list "path␉sha256" of all files (GitHub, local).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    /// PDMX: rows accepted by license and subset / rows in the CSV.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv_rows: Option<usize>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FetchedFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub md5: String,
}

/// Download plan: what isn't in the cache yet (name, bytes).
pub fn planned_downloads(cfg: &Config, cache: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for s in &cfg.sources {
        if let Kind::Pdmx { csv, mxl, .. } = &s.kind {
            for p in [csv, mxl] {
                let f = cache.join(&s.id).join(&p.name);
                if f.metadata().map(|m| m.len()).ok() != Some(p.size) {
                    out.push((format!("{}/{}", s.id, p.name), p.size));
                }
            }
        }
    }
    out
}

fn zenodo_url(record: &str, name: &str) -> String {
    format!("https://zenodo.org/api/records/{record}/files/{name}/content")
}

/// Downloads `url` to `target` (resuming a partial file) and checks size and MD5.
fn download_pinned(url: &str, target: &Path, pin: &Pinned, log: &dyn Fn(&str)) -> io::Result<FetchedFile> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if target.metadata().map(|m| m.len()).ok() != Some(pin.size) {
        let part = target.with_extension("part");
        log(&format!("downloading {url} ({} MB) …", pin.size / 1_000_000));
        let status = Command::new("curl")
            .args(["--location", "--fail", "--silent", "--show-error", "--retry", "5", "--retry-delay", "5", "--continue-at", "-", "--user-agent", "klang-libbuild"])
            .arg("--output")
            .arg(&part)
            .arg(url)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!("download failed: {url}")));
        }
        std::fs::rename(&part, target)?;
    }
    log(&format!("checking {} …", target.display()));
    let (sha256, md5, size) = hash::hashes(BufReader::new(File::open(target)?))?;
    if size != pin.size || !md5.eq_ignore_ascii_case(&pin.md5) {
        return Err(io::Error::other(format!(
            "{}: expected {} bytes, md5 {} – got {size} bytes, md5 {md5}. Delete the file to download it again.",
            target.display(),
            pin.size,
            pin.md5
        )));
    }
    Ok(FetchedFile { name: pin.name.clone(), size, sha256, md5 })
}

/// Rows of PDMX.csv that are taken (key: file stem `Qm…`).
pub struct PdmxRows {
    pub rows: HashMap<String, (PdmxMeta, String)>,
    pub total: usize,
}

/// Reads PDMX.csv: keeps rows with an accepted license, without license
/// conflict and (if set) in the subset.
pub fn read_pdmx_csv(path: &Path, licenses: &[String], subset: Option<&str>, names: &HashMap<String, String>) -> io::Result<PdmxRows> {
    let mut cols: Option<HashMap<String, usize>> = None;
    let mut rows = HashMap::new();
    let mut total = 0usize;
    let na = |s: &str| {
        let t = s.trim();
        if t == "NA" || t == "nan" { String::new() } else { t.to_string() }
    };
    for_each_record(BufReader::new(File::open(path)?), &AtomicBool::new(false), |rec| {
        let Some(c) = cols.as_ref() else {
            cols = Some(rec.iter().enumerate().map(|(i, n)| (n.trim().to_string(), i)).collect());
            return true;
        };
        total += 1;
        let get = |name: &str| c.get(name).and_then(|&i| rec.get(i)).map(String::as_str).unwrap_or("");
        let license = get("license").trim();
        if !licenses.iter().any(|l| l == license) || get("license_conflict").trim() == "True" {
            return true;
        }
        if let Some(col) = subset {
            if get(col).trim() != "True" {
                return true;
            }
        }
        let mxl = get("mxl");
        let file = mxl.rsplit(['/', '\\']).next().unwrap_or(mxl);
        let Some(stem) = file.strip_suffix(".mxl") else { return true };
        let composer = [get("composer_name"), get("artist_name")].into_iter().map(na).find(|s| !s.is_empty()).unwrap_or_default();
        let meta = PdmxMeta {
            genres: na(get("genres")).split('-').map(str::trim).filter(|g| !g.is_empty()).map(str::to_string).collect(),
            composer,
            complexity: na(get("complexity")).parse::<f32>().ok().map(|v| v.clamp(0.0, 9.0) as u8),
            license_conflict: false,
        };
        let shown = names.get(license).cloned().unwrap_or_else(|| license.to_string());
        rows.insert(stem.to_string(), (meta, shown));
        true
    })?;
    if cols.is_none() {
        return Err(io::Error::other(format!("{}: empty", path.display())));
    }
    Ok(PdmxRows { rows, total })
}

/// Fetches a source into the cache and hands its scores to `each` (in a
/// fixed order). `limit` stops after that many scores (trial runs).
pub fn read_source(src: &Source, cfg_dir: &Path, cache: &Path, limit: Option<usize>, log: &dyn Fn(&str), each: &mut dyn FnMut(Item) -> io::Result<bool>) -> io::Result<Fetched> {
    let genre_meta = || src.genre.as_ref().map(|g| PdmxMeta { genres: vec![g.clone()], ..Default::default() });
    let mut count = 0usize;
    let under_limit = |count: usize| limit.is_none_or(|l| count < l);
    match &src.kind {
        Kind::Pdmx { record, csv, mxl, licenses, subset, license_names } => {
            let dir = cache.join(&src.id);
            let csv_file = download_pinned(&zenodo_url(record, &csv.name), &dir.join(&csv.name), csv, log)?;
            let mxl_file = download_pinned(&zenodo_url(record, &mxl.name), &dir.join(&mxl.name), mxl, log)?;
            log("reading PDMX.csv …");
            let rows = read_pdmx_csv(&dir.join(&csv.name), licenses, subset.as_deref(), license_names)?;
            log(&format!("PDMX.csv: {} of {} rows accepted (license, no conflict{})", rows.rows.len(), rows.total, subset.as_ref().map(|s| format!(", {s}")).unwrap_or_default()));
            let file = File::open(dir.join(&mxl.name))?;
            let mut archive = tar::Archive::new(flate2::read::MultiGzDecoder::new(BufReader::with_capacity(1 << 20, file)));
            for entry in archive.entries()? {
                if !under_limit(count) {
                    break;
                }
                let mut entry = entry?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry.path()?.to_string_lossy().replace('\\', "/");
                let Some(stem) = path.rsplit('/').next().and_then(|f| f.strip_suffix(".mxl")) else { continue };
                let Some((meta, license)) = rows.rows.get(stem) else { continue };
                let mut bytes = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut bytes)?;
                let rel = path.strip_prefix("mxl/").unwrap_or(&path);
                count += 1;
                if !each(Item { name: format!("{}/{rel}", src.id), bytes, meta: Some(meta.clone()), license: license.clone() })? {
                    break;
                }
            }
            Ok(Fetched { files: vec![csv_file, mxl_file], accepted_rows: Some(rows.rows.len()), csv_rows: Some(rows.total), ..Default::default() })
        }
        Kind::Github { repo, commit, prefix, suffix } => {
            let dir = cache.join(&src.id).join(commit);
            let list_file = dir.join(".tree.json");
            // the file list at this commit (one API request; cached)
            let tree = match std::fs::read(&list_file) {
                Ok(b) => fetch::parse_github_tree(&b)?,
                Err(_) => {
                    log(&format!("file list of {repo} at {commit} …"));
                    let get = |url: &str| -> io::Result<Vec<u8>> {
                        let out = Command::new("curl").args(["--location", "--fail", "--silent", "--show-error", "--user-agent", "klang-libbuild"]).arg(url).output()?;
                        if !out.status.success() {
                            return Err(io::Error::other(format!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim())));
                        }
                        Ok(out.stdout)
                    };
                    // the commit's tree (the trees API doesn't take a commit for big repositories)
                    let commit_json: serde_json::Value = serde_json::from_slice(&get(&format!("https://api.github.com/repos/{repo}/git/commits/{commit}"))?).map_err(io::Error::other)?;
                    let tree_sha = commit_json["tree"]["sha"].as_str().ok_or_else(|| io::Error::other(format!("{repo}@{commit}: no tree")))?.to_string();
                    let body = get(&format!("https://api.github.com/repos/{repo}/git/trees/{tree_sha}?recursive=1"))?;
                    let v: serde_json::Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
                    if v["truncated"].as_bool() == Some(true) {
                        return Err(io::Error::other(format!("{repo}: file list truncated by GitHub")));
                    }
                    let tree = fetch::parse_github_tree(&body)?;
                    std::fs::create_dir_all(&dir)?;
                    std::fs::write(&list_file, &body)?;
                    tree
                }
            };
            let mut files: Vec<(String, u64)> = tree.into_iter().filter(|(p, _)| p.starts_with(prefix.as_str()) && p.ends_with(suffix.as_str())).collect();
            files.sort();
            if let Some(l) = limit {
                files.truncate(l);
            }
            let remote: Vec<fetch::RemoteFile> = files.iter().map(|(p, size)| fetch::RemoteFile { url: fetch::github_raw_url(repo, commit, p), rel: fetch::ascii_path(p), size: *size }).collect();
            log(&format!("{repo}: {} files …", remote.len()));
            let stats = fetch::fetch_files(&remote, &dir, &AtomicBool::new(false), &Mutex::new(None), |_| {})?;
            if stats.failed > 0 {
                return Err(io::Error::other(format!("{repo}: {} files not downloaded (e.g. {})", stats.failed, stats.failed_paths.join(", "))));
            }
            let mut digest = String::new();
            for ((path, _), f) in files.iter().zip(&remote) {
                let bytes = std::fs::read(dir.join(&f.rel))?;
                digest.push_str(&format!("{path}\t{}\n", hash::sha256(&bytes)));
                if under_limit(count) {
                    count += 1;
                    if !each(Item { name: format!("{}/{path}", src.id), bytes, meta: genre_meta(), license: src.license.clone() })? {
                        break;
                    }
                }
            }
            Ok(Fetched { content_sha256: Some(hash::sha256(digest.as_bytes())), ..Default::default() })
        }
        Kind::Local { path } => {
            let root = cfg_dir.join(path);
            let mut files = klang_music::library::collect_files(&root)?;
            files.sort();
            let mut digest = String::new();
            for f in files {
                let rel = f.strip_prefix(&root).unwrap_or(&f).to_string_lossy().replace('\\', "/");
                let bytes = std::fs::read(&f)?;
                digest.push_str(&format!("{rel}\t{}\n", hash::sha256(&bytes)));
                if under_limit(count) {
                    count += 1;
                    if !each(Item { name: format!("{}/{rel}", src.id), bytes, meta: genre_meta(), license: src.license.clone() })? {
                        break;
                    }
                }
            }
            Ok(Fetched { content_sha256: Some(hash::sha256(digest.as_bytes())), ..Default::default() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn without_nulls(v: Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(m.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, without_nulls(v))).collect()),
            Value::Array(a) => Value::Array(a.into_iter().map(without_nulls).collect()),
            v => v,
        }
    }

    /// Every key of the shipped configuration is read (a misspelt or
    /// unrenamed field would silently fall back to its default).
    #[test]
    fn the_shipped_configuration_is_read_completely() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("sources.json");
        let cfg = Config::load(&path).unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(without_nulls(serde_json::to_value(&cfg).unwrap()), without_nulls(file));
        let pdmx = cfg.sources.iter().find(|s| s.id == "pdmx").unwrap();
        let Kind::Pdmx { license_names, .. } = &pdmx.kind else { panic!("source pdmx is not of kind pdmx") };
        assert_eq!(license_names.get("cc-zero").map(String::as_str), Some("CC0-1.0"));
    }
}
