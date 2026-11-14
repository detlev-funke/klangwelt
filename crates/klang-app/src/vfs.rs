//! Files of the core (projects, settings, sounds, exports).
//!
//! * Desktop: the file system - thin wrappers around `std::fs`.
//! * Browser: an in-memory file tree with the same paths as on the
//!   desktop (`/Klangwelt/Projekte/…`). The web core loads it at startup from
//!   the browser's storage (OPFS, [`put`]) and afterwards saves changes
//!   back there ([`take_changes`]).
//!
//! The core uses only these functions for its files; this way
//! both platforms behave the same.

use std::io;
use std::path::{Path, PathBuf};

/// Entry of a folder.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub len: u64,
    /// Unix time in milliseconds (0 = unknown).
    pub modified_ms: u64,
}

// ---------------------------------------------------------------- Desktop

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::Entry;
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    #[inline]
    pub fn read(path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    #[inline]
    pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
        std::fs::write(path, data)
    }

    #[inline]
    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    #[inline]
    pub fn remove_file(path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    #[inline]
    pub fn exists(path: &Path) -> bool {
        path.exists()
    }

    #[inline]
    pub fn is_dir(path: &Path) -> bool {
        path.is_dir()
    }

    /// Rename; Windows sometimes releases a just-closed file only a moment
    /// later (virus scanner) - then retry several times.
    pub fn rename_retry(from: &Path, to: &Path) -> io::Result<()> {
        let mut tries = 0;
        loop {
            match std::fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(e) if tries >= 20 => return Err(e),
                Err(_) => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    pub fn list(dir: &Path) -> io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir)?.flatten() {
            let meta = e.metadata().ok();
            out.push(Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
                is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                len: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                modified_ms: meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- Browser

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::Entry;
    use std::collections::{BTreeMap, BTreeSet};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard};

    struct File {
        data: Arc<Vec<u8>>,
        modified_ms: u64,
    }

    #[derive(Default)]
    struct Tree {
        files: BTreeMap<PathBuf, File>,
        dirs: BTreeSet<PathBuf>,
        /// Written or deleted since the last [`take_changes`].
        changed: BTreeSet<PathBuf>,
    }

    static TREE: Mutex<Option<Tree>> = Mutex::new(None);

    fn tree() -> MutexGuard<'static, Option<Tree>> {
        let mut g = TREE.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(Tree::default());
        }
        g
    }

    fn with<T>(f: impl FnOnce(&mut Tree) -> T) -> T {
        let mut g = tree();
        f(g.as_mut().expect("angelegt"))
    }

    fn not_found(path: &Path) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, klang_i18n::tr!("{path} doesn't exist", path = path.display()))
    }

    fn add_parents(t: &mut Tree, path: &Path) {
        let mut p = path.parent();
        while let Some(d) = p {
            if d.as_os_str().is_empty() || !t.dirs.insert(d.to_path_buf()) {
                break;
            }
            p = d.parent();
        }
    }

    pub fn read(path: &Path) -> io::Result<Vec<u8>> {
        with(|t| t.files.get(path).map(|f| f.data.as_ref().clone()).ok_or_else(|| not_found(path)))
    }

    pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
        let now = crate::clock::now_ms();
        with(|t| {
            add_parents(t, path);
            t.files.insert(path.to_path_buf(), File { data: Arc::new(data.to_vec()), modified_ms: now });
            t.changed.insert(path.to_path_buf());
        });
        Ok(())
    }

    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        with(|t| {
            t.dirs.insert(path.to_path_buf());
            add_parents(t, path);
        });
        Ok(())
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        with(|t| {
            t.files.remove(path).ok_or_else(|| not_found(path))?;
            t.changed.insert(path.to_path_buf());
            Ok(())
        })
    }

    pub fn exists(path: &Path) -> bool {
        with(|t| t.files.contains_key(path) || t.dirs.contains(path))
    }

    pub fn is_dir(path: &Path) -> bool {
        with(|t| t.dirs.contains(path))
    }

    pub fn rename_retry(from: &Path, to: &Path) -> io::Result<()> {
        with(|t| {
            let f = t.files.remove(from).ok_or_else(|| not_found(from))?;
            add_parents(t, to);
            t.files.insert(to.to_path_buf(), f);
            t.changed.insert(from.to_path_buf());
            t.changed.insert(to.to_path_buf());
            Ok(())
        })
    }

    pub fn list(dir: &Path) -> io::Result<Vec<Entry>> {
        with(|t| {
            if !t.dirs.contains(dir) {
                return Err(not_found(dir));
            }
            let mut out: Vec<Entry> = t
                .dirs
                .iter()
                .filter(|d| d.parent() == Some(dir))
                .map(|d| Entry { name: d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), path: d.clone(), is_dir: true, len: 0, modified_ms: 0 })
                .collect();
            out.extend(t.files.iter().filter(|(p, _)| p.parent() == Some(dir)).map(|(p, f)| Entry {
                name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                path: p.clone(),
                is_dir: false,
                len: f.data.len() as u64,
                modified_ms: f.modified_ms,
            }));
            Ok(out)
        })
    }

    /// Takes over a file from the browser's storage (at startup) without marking it
    /// as changed.
    pub fn put(path: &Path, data: Vec<u8>, modified_ms: u64) {
        with(|t| {
            add_parents(t, path);
            t.files.insert(path.to_path_buf(), File { data: Arc::new(data), modified_ms });
        });
    }

    /// Changed paths since the last call: with content (written) or
    /// without (deleted). The web core saves them to OPFS.
    pub fn take_changes() -> Vec<(PathBuf, Option<Arc<Vec<u8>>>)> {
        with(|t| {
            let changed = std::mem::take(&mut t.changed);
            changed.into_iter().map(|p| { let data = t.files.get(&p).map(|f| f.data.clone()); (p, data) }).collect()
        })
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::{put, take_changes};

/// Reads a whole file.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    imp::read(path)
}

/// Writes a whole file (replaces it).
pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    imp::write(path, data)
}

pub fn create_dir_all(path: &Path) -> io::Result<()> {
    imp::create_dir_all(path)
}

pub fn remove_file(path: &Path) -> io::Result<()> {
    imp::remove_file(path)
}

pub fn exists(path: &Path) -> bool {
    imp::exists(path)
}

pub fn is_dir(path: &Path) -> bool {
    imp::is_dir(path)
}

/// Renames (desktop: with retry in case Windows still holds the file).
pub fn rename_retry(from: &Path, to: &Path) -> io::Result<()> {
    imp::rename_retry(from, to)
}

/// Entries of a folder (order unspecified).
pub fn list(dir: &Path) -> io::Result<Vec<Entry>> {
    imp::list(dir)
}

/// Writes via a side file and then renames it - a crash
/// midway never destroys the old file.
pub fn write_atomic(path: &Path, data: &[u8], tmp_ext: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        create_dir_all(dir)?;
    }
    let tmp: PathBuf = path.with_extension(tmp_ext);
    write(&tmp, data)?;
    rename_retry(&tmp, path)
}
