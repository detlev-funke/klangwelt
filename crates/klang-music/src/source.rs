//! Byte sources for the library files: a file on disk, bytes in memory, or a
//! file on a web server that is read in ranges (the browser version, [`remote`]).
//!
//! The index reader ([`crate::diskindex::DiskIndex`]) and the web package
//! readers ([`crate::webpack`]) only ever ask for "these bytes at that offset",
//! so the same code serves all three.

use std::fs::File;
use std::io;

/// Random access to the bytes of one file.
pub trait ReadAt: Send + Sync {
    /// Reads exactly `buf.len()` bytes starting at `off`.
    fn read_at(&self, buf: &mut [u8], off: u64) -> io::Result<()>;
    /// Length of the file in bytes.
    fn size(&self) -> io::Result<u64>;
}

impl ReadAt for File {
    fn read_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.read_exact_at(buf, off)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let (mut done, mut off) = (0usize, off);
            while done < buf.len() {
                let n = self.seek_read(&mut buf[done..], off)?;
                if n == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, klang_i18n::tr!("Index file too short")));
                }
                done += n;
                off += n as u64;
            }
            Ok(())
        }
        // Other targets (e.g. wasm32): set the position and read.
        #[cfg(not(any(unix, windows)))]
        {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = self;
            f.seek(SeekFrom::Start(off))?;
            f.read_exact(buf)
        }
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
}

impl ReadAt for Vec<u8> {
    fn read_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        let start = usize::try_from(off).map_err(io::Error::other)?;
        let end = start.checked_add(buf.len()).filter(|&e| e <= self.len()).ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, klang_i18n::tr!("Index file too short")))?;
        buf.copy_from_slice(&self[start..end]);
        Ok(())
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

/// `len` bytes from `off`.
pub fn read_vec(src: &dyn ReadAt, off: u64, len: u64) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; usize::try_from(len).map_err(io::Error::other)?];
    src.read_at(&mut buf, off)?;
    Ok(buf)
}

/// Files on a web server, read in ranges (browser version).
///
/// The core runs synchronously, the network does not. So a read that isn't
/// in memory yet fails with [`io::ErrorKind::WouldBlock`] and is noted as
/// *missing*. The caller (the web shell) runs a command, collects what was
/// missing ([`take_missing`]), fetches those ranges – from the browser cache or
/// the server, several per request – hands them over with [`feed`] and runs
/// the command again. A search needs a handful of such rounds (key table →
/// occurrence lists → feature sequences → entries); each round asks for
/// everything it needs at once.
pub mod remote {
    use super::ReadAt;
    use serde::Serialize;
    use std::collections::{BTreeMap, VecDeque};
    use std::io;
    use std::sync::{Arc, Mutex, MutexGuard};

    /// A range of a file the last command needed but didn't have (`end` exclusive).
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
    pub struct Need {
        pub file: String,
        pub start: u64,
        pub end: u64,
    }

    /// Ranges of one file that have arrived (start → bytes), oldest first in `order`.
    #[derive(Default)]
    struct Store {
        chunks: BTreeMap<u64, Vec<u8>>,
        order: VecDeque<u64>,
        bytes: usize,
    }

    /// Upper bound for what all remote files keep in memory together; the
    /// browser cache keeps more (and survives a reload).
    const MEMORY_LIMIT: usize = 48 << 20;

    static FILES: Mutex<BTreeMap<String, Arc<Mutex<Store>>>> = Mutex::new(BTreeMap::new());

    thread_local! {
        /// Per thread: the browser core has one, tests run side by side.
        static MISSING: std::cell::RefCell<Vec<Need>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A file on the server, known by name and length (from the package manifest).
    pub struct RemoteFile {
        name: String,
        len: u64,
        store: Arc<Mutex<Store>>,
    }

    impl RemoteFile {
        /// Registers the file; ranges for it can then be [`feed`]-ed.
        pub fn new(name: &str, len: u64) -> Self {
            let store = guard(&FILES).entry(name.to_string()).or_default().clone();
            Self { name: name.to_string(), len, store }
        }

        pub fn name(&self) -> &str {
            &self.name
        }
    }

    impl ReadAt for RemoteFile {
        fn read_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
            let end = off.checked_add(buf.len() as u64).filter(|&e| e <= self.len);
            let Some(end) = end else {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, klang_i18n::tr!("Index file too short")));
            };
            if buf.is_empty() {
                return Ok(());
            }
            {
                let store = guard(&self.store);
                if let Some((&start, data)) = store.chunks.range(..=off).next_back() {
                    if start + data.len() as u64 >= end {
                        let at = (off - start) as usize;
                        buf.copy_from_slice(&data[at..at + buf.len()]);
                        return Ok(());
                    }
                }
            }
            MISSING.with(|m| m.borrow_mut().push(Need { file: self.name.clone(), start: off, end }));
            Err(io::Error::new(io::ErrorKind::WouldBlock, "not loaded yet"))
        }

        fn size(&self) -> io::Result<u64> {
            Ok(self.len)
        }
    }

    /// Start of a command: forget what earlier commands missed.
    pub fn begin() {
        MISSING.with(|m| m.borrow_mut().clear());
    }

    /// Did the current command miss data?
    pub fn pending() -> bool {
        MISSING.with(|m| !m.borrow().is_empty())
    }

    /// What the current command missed (sorted, without duplicates).
    pub fn take_missing() -> Vec<Need> {
        let mut v = MISSING.with(|m| std::mem::take(&mut *m.borrow_mut()));
        v.sort();
        v.dedup();
        v
    }

    /// Bytes of `file` from `start` arrived. `false`: no such file is registered.
    pub fn feed(file: &str, start: u64, data: Vec<u8>) -> bool {
        let Some(store) = guard(&FILES).get(file).cloned() else { return false };
        {
            let mut s = guard(&store);
            if let Some(old) = s.chunks.insert(start, data) {
                s.bytes -= old.len();
            } else {
                s.order.push_back(start);
            }
            let added = s.chunks.get(&start).map(Vec::len).unwrap_or(0);
            s.bytes += added;
        }
        trim();
        true
    }

    /// Bytes held in memory by all remote files.
    pub fn memory() -> usize {
        guard(&FILES).values().map(|s| guard(s).bytes).sum()
    }

    /// Keeps the memory below [`MEMORY_LIMIT`]: the oldest ranges go first.
    fn trim() {
        let files: Vec<Arc<Mutex<Store>>> = guard(&FILES).values().cloned().collect();
        let mut total: usize = files.iter().map(|s| guard(s).bytes).sum();
        while total > MEMORY_LIMIT {
            // the file with the most bytes gives up its oldest range
            let Some(big) = files.iter().max_by_key(|s| guard(s).bytes) else { break };
            let mut s = guard(big);
            let Some(start) = s.order.pop_front() else { break };
            if let Some(d) = s.chunks.remove(&start) {
                s.bytes -= d.len();
                total -= d.len();
            }
        }
    }

    /// Forgets all files and ranges (another package is attached).
    pub fn reset() {
        guard(&FILES).clear();
        begin();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn missing_ranges_are_noted_and_fed_ranges_serve_reads() {
            let f = RemoteFile::new("test-remote-a.bin", 100);
            begin();
            let mut buf = [0u8; 4];
            assert_eq!(f.read_at(&mut buf, 10).unwrap_err().kind(), io::ErrorKind::WouldBlock);
            assert!(f.read_at(&mut buf, 10).is_err());
            assert!(pending());
            let need = take_missing();
            assert_eq!(need, vec![Need { file: "test-remote-a.bin".into(), start: 10, end: 14 }]);
            assert!(!pending());
            // a bigger range covers smaller reads inside it
            assert!(feed("test-remote-a.bin", 8, (8u8..20).collect()));
            f.read_at(&mut buf, 10).unwrap();
            assert_eq!(buf, [10, 11, 12, 13]);
            assert!(f.read_at(&mut buf, 18).is_err(), "only partly there");
            assert!(f.read_at(&mut buf, 98).is_err());
            assert_eq!(take_missing().len(), 1, "past the end is an error, not a missing range");
            assert!(!feed("unknown-file", 0, vec![1]));
        }
    }
}
