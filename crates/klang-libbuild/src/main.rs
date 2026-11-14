//! klang-libbuild – builds the score library package for Klangwelt's browser
//! version (and, optionally, the desktop) from pinned sources.
//!
//! ```text
//! klang-libbuild build  [--sources FILE] [--out DIR] [--cache DIR] [--only ID,…]
//!                       [--limit N] [--date YYYY-MM-DD] [--jobs N] [--allow-download-mb N]
//! klang-libbuild verify   DIR
//! klang-libbuild describe [DIR] [--sources FILE]
//! klang-libbuild plan   [--sources FILE] [--cache DIR]
//! ```
//!
//! See `docs/LIBRARY.md`.

mod build;
mod describe;
mod hash;
mod sources;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
klang-libbuild – builds the Klangwelt score library package (see docs/LIBRARY.md)

usage:
  klang-libbuild build  [options]   fetch the sources and build the package
  klang-libbuild plan   [options]   show what would be downloaded
  klang-libbuild verify DIR         check a built package
  klang-libbuild describe [DIR]     write README.md and zenodo.json of a built package again

options:
  --sources FILE          build configuration (default: crates/klang-libbuild/sources.json)
  --out DIR               package folder (default: target/library/package)
  --cache DIR             download cache (default: target/library/cache)
  --only ID,ID            only these sources
  --limit N               at most N scores per source (trial run)
  --date YYYY-MM-DD       version date of the package (default: today, UTC);
                          the same date and sources give byte-identical files
  --jobs N                parser threads (default: number of CPU threads)
  --allow-download-mb N   download at most N MB without asking (default: 200)
";

struct Args {
    cmd: String,
    rest: Vec<String>,
    sources: PathBuf,
    out: PathBuf,
    cache: PathBuf,
    only: Vec<String>,
    limit: Option<usize>,
    date: Option<String>,
    jobs: usize,
    allow_mb: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().ok_or_else(|| USAGE.to_string())?;
    let mut a = Args {
        cmd,
        rest: Vec::new(),
        sources: PathBuf::from("crates/klang-libbuild/sources.json"),
        out: PathBuf::from("target/library/package"),
        cache: PathBuf::from("target/library/cache"),
        only: Vec::new(),
        limit: None,
        date: None,
        jobs: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        allow_mb: 200,
    };
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| inline.clone().or_else(|| it.next()).ok_or_else(|| format!("{name} needs a value"));
        match key.as_str() {
            "--sources" => a.sources = value("--sources")?.into(),
            "--out" => a.out = value("--out")?.into(),
            "--cache" => a.cache = value("--cache")?.into(),
            "--only" => a.only = value("--only")?.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
            "--limit" => a.limit = Some(value("--limit")?.parse().map_err(|_| "--limit: a number")?),
            "--date" => a.date = Some(value("--date")?),
            "--jobs" => a.jobs = value("--jobs")?.parse::<usize>().map_err(|_| "--jobs: a number")?.max(1),
            "--allow-download-mb" => a.allow_mb = value("--allow-download-mb")?.parse().map_err(|_| "--allow-download-mb: a number")?,
            "-h" | "--help" => return Err(USAGE.to_string()),
            _ if key.starts_with("--") => return Err(format!("unknown option {key}\n\n{USAGE}")),
            _ => a.rest.push(arg),
        }
    }
    Ok(a)
}

/// Today (UTC) as YYYY-MM-DD.
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn run() -> Result<(), String> {
    let a = parse_args()?;
    let cfg_dir = |p: &Path| p.parent().map(Path::to_path_buf).unwrap_or_default();
    match a.cmd.as_str() {
        "build" | "plan" => {
            let mut cfg = sources::Config::load(&a.sources).map_err(|e| e.to_string())?;
            if !a.only.is_empty() {
                for id in &a.only {
                    if !cfg.sources.iter().any(|s| &s.id == id) {
                        return Err(format!("unknown source {id} (in {}: {})", a.sources.display(), cfg.sources.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", ")));
                    }
                }
                cfg.sources.retain(|s| a.only.contains(&s.id));
            }
            let downloads = sources::planned_downloads(&cfg, &a.cache);
            let total_mb: u64 = downloads.iter().map(|d| d.1).sum::<u64>() / 1_000_000;
            for (name, size) in &downloads {
                eprintln!("to download: {name} ({} MB)", size / 1_000_000);
            }
            if a.cmd == "plan" {
                eprintln!("{} MB to download into {}", total_mb, a.cache.display());
                return Ok(());
            }
            if total_mb > a.allow_mb {
                return Err(format!("{total_mb} MB would be downloaded – allow it with --allow-download-mb {total_mb} (or more)"));
            }
            let opts = build::Options { out: a.out.clone(), cache: a.cache.clone(), only: Vec::new(), limit: a.limit, date: a.date.clone().unwrap_or_else(today), jobs: a.jobs };
            let built = build::build(&cfg, &cfg_dir(&a.sources), &opts).map_err(|e| e.to_string())?;
            eprintln!();
            for (id, st) in &built.stats {
                eprintln!("{id}: {} scores, {} voices, {} duplicates, {} too short, {} failed", st.scores, st.voices, st.duplicates, st.too_short, st.failed);
                for e in st.errors.iter().take(5) {
                    eprintln!("    {e}");
                }
            }
            let size = |k: &str| built.manifest["files"][k]["size"].as_u64().unwrap_or(0) as f64 / 1e6;
            eprintln!(
                "package in {}: {} scores, {} voices in {:.0} s ({:.0} scores/s) – index {:.1} MB, scores {:.1} MB, text {:.1} MB, facets {:.2} MB, chords {:.2} MB",
                a.out.display(),
                built.scores,
                built.voices,
                built.seconds,
                built.scores as f64 / built.seconds.max(0.001),
                size("index"),
                size("scores"),
                size("text"),
                size("facets"),
                size("chords")
            );
            Ok(())
        }
        "verify" => {
            let dir = a.rest.first().map(PathBuf::from).unwrap_or(a.out);
            let report = build::verify(&dir).map_err(|e| e.to_string())?;
            print!("{report}");
            Ok(())
        }
        "describe" => {
            let dir = a.rest.first().map(PathBuf::from).unwrap_or(a.out);
            let cfg = sources::Config::load(&a.sources).map_err(|e| e.to_string())?;
            describe::write(&dir, &cfg).map_err(|e| e.to_string())?;
            eprintln!("wrote {} and {}", dir.join("README.md").display(), dir.join("zenodo.json").display());
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
