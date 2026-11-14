//! Downloading a music library: reads a `.tar.gz` archive as a data stream and
//! writes the MusicXML files (`.mxl`, `.musicxml`, `.xml`) to a folder –
//! without caching the archive. Optionally stops after N files
//! (a sample from the start of a large archive).
//!
//! The download itself goes through the operating system's `curl` (Windows 10+,
//! macOS, Linux): this way the app needs no TLS stack of its own.

use flate2::read::MultiGzDecoder;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub struct ExtractStats {
    /// Scores in the target folder (newly written or already present).
    pub files: usize,
    /// Of those, already present (same size) – e.g. after an abort.
    pub existing: usize,
    pub bytes_written: u64,
    /// Stopped because the requested number was reached.
    pub limit_reached: bool,
    /// This many times the connection dropped and was resumed.
    pub resumed: u32,
}

/// Counts the bytes read (compressed) for the progress display.
pub struct Counting<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R> Counting<R> {
    pub fn new(inner: R, count: Arc<AtomicU64>) -> Self {
        Self { inner, count }
    }
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn is_score(p: &Path) -> bool {
    crate::score::is_score_file(p)
}

/// Relative target path from an archive path – normal components only (no
/// absolute path, no "..") – optionally without the leading folder `strip`.
fn safe_relative(p: &Path, strip: Option<&str>) -> Option<PathBuf> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => parts.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if let (Some(s), Some(first)) = (strip, parts.first()) {
        if parts.len() > 1 && first.to_str() == Some(s) {
            parts.remove(0);
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.iter().collect())
}

fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, klang_i18n::tr!("canceled"))
}

/// Abort within a data stream (`Read`): deliberately not `Interrupted` –
/// `read_exact`/`read_to_end` treat that as "just read again" and would then
/// hang. Whoever processes the stream turns it back into [`interrupted`].
fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, klang_i18n::tr!("canceled"))
}

/// Unpacks scores from a `.tar.gz` data stream into `dest`.
/// `on_file` is called after every score; `cancel` aborts (error
/// `Interrupted`, even if the abort came in the middle of the stream).
pub fn extract_scores(
    reader: impl Read,
    dest: &Path,
    strip: Option<&str>,
    limit: Option<usize>,
    cancel: &AtomicBool,
    on_file: impl FnMut(&ExtractStats),
) -> io::Result<ExtractStats> {
    extract_inner(reader, dest, strip, limit, cancel, on_file).map_err(|e| if cancel.load(Ordering::Relaxed) { interrupted() } else { e })
}

fn extract_inner(
    reader: impl Read,
    dest: &Path,
    strip: Option<&str>,
    limit: Option<usize>,
    cancel: &AtomicBool,
    mut on_file: impl FnMut(&ExtractStats),
) -> io::Result<ExtractStats> {
    std::fs::create_dir_all(dest)?;
    let mut archive = tar::Archive::new(MultiGzDecoder::new(reader));
    let mut stats = ExtractStats::default();
    for entry in archive.entries()? {
        if cancel.load(Ordering::Relaxed) {
            return Err(interrupted());
        }
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.into_owned();
        if !is_score(&path) {
            continue;
        }
        let Some(rel) = safe_relative(&path, strip) else { continue };
        let target = dest.join(rel);
        let size = entry.header().size()?;
        if target.metadata().map(|m| m.is_file() && m.len() == size).unwrap_or(false) {
            stats.existing += 1;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Write completely first, then rename: no half-written files after an abort.
            let tmp = target.with_extension("teil");
            let mut f = std::fs::File::create(&tmp)?;
            io::copy(&mut entry, &mut f)?;
            drop(f);
            std::fs::rename(&tmp, &target)?;
            stats.bytes_written += size;
        }
        stats.files += 1;
        on_file(&stats);
        if limit.is_some_and(|l| stats.files >= l) {
            stats.limit_reached = true;
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(interrupted());
    }
    Ok(stats)
}

/// Starts `curl` for `url`; the response arrives as a data stream on stdout.
pub fn curl(url: &str) -> io::Result<(Child, ChildStdout)> {
    curl_at(url, 0)
}

/// Like [`curl`], but from byte `from` (HTTP range). If the server does not answer
/// with exactly this range, curl aborts (instead of delivering from the start).
fn curl_at(url: &str, from: u64) -> io::Result<(Child, ChildStdout)> {
    let mut cmd = Command::new("curl");
    cmd.args([
        "--location",
        "--fail",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "30",
        // abort if less than 1 kB/s arrives for a minute
        "--speed-limit",
        "1000",
        "--speed-time",
        "60",
    ]);
    if from > 0 {
        cmd.arg("--continue-at").arg(from.to_string());
    }
    cmd.arg(url)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            io::Error::new(io::ErrorKind::NotFound, klang_i18n::tr!("curl was not found (included in Windows 10 and later; on macOS/Linux, please install it)"))
        } else {
            e
        }
    })?;
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other(klang_i18n::tr!("curl returned no output")))?;
    Ok((child, stdout))
}

/// Data stream of a URL via `curl` that continues at the same place after a
/// connection drop (HTTP range). For large archives that are unpacked while
/// they arrive: the reader behind it does not notice the drop.
/// The running curl process lives in `slot` (an abort can end it there);
/// if `cancel` is set, there is no new attempt. On drop, curl is ended
/// (e.g. when a sample has read enough).
pub struct ResumingDownload<'a> {
    url: String,
    pos: u64,
    /// State at the last connect (did the connection deliver anything?)
    start_pos: u64,
    stdout: Option<ChildStdout>,
    slot: &'a Mutex<Option<Child>>,
    cancel: &'a AtomicBool,
    /// Consecutive connections that delivered not a single byte
    fruitless: u32,
    connections: u32,
    done: bool,
    on_resume: Box<dyn FnMut(u64, u32, &str) + 'a>,
    /// Wait time before a new attempt (tests: short)
    pause: std::time::Duration,
}

/// This many connections in a row without a new byte, then give up.
const MAX_FRUITLESS: u32 = 5;
/// At most this many connections in total.
const MAX_CONNECTIONS: u32 = 60;

impl<'a> ResumingDownload<'a> {
    /// Connects immediately (errors like "curl is missing" come right away). `on_resume`
    /// reports a new attempt: (byte position, connection number, reason).
    pub fn new(url: &str, slot: &'a Mutex<Option<Child>>, cancel: &'a AtomicBool, on_resume: impl FnMut(u64, u32, &str) + 'a) -> io::Result<Self> {
        let mut d = Self {
            url: url.to_string(),
            pos: 0,
            start_pos: 0,
            stdout: None,
            slot,
            cancel,
            fruitless: 0,
            connections: 0,
            done: false,
            on_resume: Box::new(on_resume),
            pause: std::time::Duration::from_secs(2),
        };
        d.connect()?;
        Ok(d)
    }

    /// Bytes read so far.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// How often the connection dropped and was resumed.
    pub fn resumed(&self) -> u32 {
        self.connections.saturating_sub(1)
    }

    fn connect(&mut self) -> io::Result<()> {
        let (child, stdout) = curl_at(&self.url, self.pos)?;
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        self.stdout = Some(stdout);
        self.start_pos = self.pos;
        self.connections += 1;
        Ok(())
    }

    /// Ends curl (`kill`: not yet finished on its own) – returns
    /// (success, exit code, error text).
    fn finish(&mut self, kill: bool) -> (bool, Option<i32>, String) {
        self.stdout = None;
        let child = self.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(mut c) = child else { return (false, None, klang_i18n::tr!("canceled")) };
        if kill {
            let _ = c.kill();
        }
        let status = c.wait().ok();
        let mut err = String::new();
        if let Some(mut e) = c.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        (status.is_some_and(|s| s.success()), status.and_then(|s| s.code()), err.trim().to_string())
    }

    /// Is a new attempt worthwhile? Not for "range not possible" (33) and
    /// for HTTP errors 4xx except 408/429 (e.g. 404: the file does not exist).
    fn retryable(code: Option<i32>, text: &str) -> bool {
        match code {
            Some(33) => false,
            Some(22) => {
                let status = text.rsplit("error: ").next().and_then(|s| s.trim().get(..3)).and_then(|s| s.parse::<u16>().ok());
                !matches!(status, Some(400..=499)) || matches!(status, Some(408 | 429))
            }
            _ => true,
        }
    }

    /// Waits briefly (in small steps so that an abort takes effect immediately).
    fn wait_before_retry(&self) -> io::Result<()> {
        let factor = 1u32 << self.fruitless.min(4);
        let end = std::time::Instant::now() + (self.pause * factor).min(std::time::Duration::from_secs(30));
        while std::time::Instant::now() < end {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            std::thread::sleep(std::time::Duration::from_millis(50).min(self.pause));
        }
        Ok(())
    }
}

impl Read for ResumingDownload<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.done || buf.is_empty() {
                return Ok(0);
            }
            if self.cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            if self.stdout.is_none() {
                self.connect()?;
            }
            let (code, reason) = match self.stdout.as_mut().map(|s| s.read(buf)) {
                Some(Ok(0)) | None => {
                    let (ok, code, text) = self.finish(false);
                    if ok {
                        self.done = true;
                        return Ok(0);
                    }
                    (code, if text.is_empty() { klang_i18n::tr!("Connection closed") } else { text })
                }
                Some(Ok(n)) => {
                    self.pos += n as u64;
                    return Ok(n);
                }
                Some(Err(e)) if e.kind() == io::ErrorKind::Interrupted => continue,
                Some(Err(e)) => {
                    let (_, code, text) = self.finish(true);
                    (code, if text.is_empty() { e.to_string() } else { text })
                }
            };
            if self.cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            self.fruitless = if self.pos > self.start_pos { 0 } else { self.fruitless + 1 };
            let reason = reason.strip_prefix("curl: ").unwrap_or(&reason).to_string();
            if !Self::retryable(code, &reason) || self.fruitless >= MAX_FRUITLESS || self.connections >= MAX_CONNECTIONS {
                let msg = if self.connections > 1 {
                    klang_i18n::tr!("{reason} (after {n} connections)", reason = reason, n = self.connections)
                } else {
                    reason
                };
                return Err(io::Error::other(msg));
            }
            // report first, then wait briefly – so the message is shown during the pause
            (self.on_resume)(self.pos, self.connections + 1, &reason);
            self.wait_before_retry()?;
        }
    }
}

impl Drop for ResumingDownload<'_> {
    fn drop(&mut self) {
        if self.stdout.is_some() {
            let _ = self.finish(true);
        }
    }
}

// ---------------------------------------------------------------- Single files (GitHub)
//
// Collections such as OpenScore, KernScores or the DCML corpora live as
// Git repositories on GitHub. Their archives contain a lot the app does not
// need (PDF, MuseScore raw files – over 90 % for OpenScore). Hence: first
// fetch the file list (one request to the GitHub API), then download only the
// matching files one by one from raw.githubusercontent.com – with `curl`, several
// at the same time.

/// A file to download.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteFile {
    pub url: String,
    /// Target path relative to the library folder (ASCII only, see [`ascii_path`]).
    pub rel: PathBuf,
    /// Expected size in bytes (from the file list).
    pub size: u64,
}

/// Files (path, size) of a GitHub repository from the response to
/// `GET /repos/{repo}/git/trees/{branch}?recursive=1`.
pub fn parse_github_tree(json: &[u8]) -> io::Result<Vec<(String, u64)>> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| io::Error::other(klang_i18n::tr!("File list unreadable: {e}", e = e)))?;
    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        let text = if msg.contains("rate limit") {
            klang_i18n::tr!("GitHub: {msg} – GitHub allows about 60 requests per hour; please try again later", msg = msg)
        } else {
            format!("GitHub: {msg}")
        };
        return Err(io::Error::other(text));
    }
    let tree = v.get("tree").and_then(|t| t.as_array()).ok_or_else(|| io::Error::other(klang_i18n::tr!("File list has no entries")))?;
    Ok(tree
        .iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("blob"))
        .filter_map(|e| Some((e.get("path")?.as_str()?.to_string(), e.get("size").and_then(|s| s.as_u64()).unwrap_or(0))))
        .collect())
}

/// Files of a repository from a bundled list:
/// `# owner/name@branch`, below it lines `path<TAB>size` (0 = unknown).
pub fn manifest_files(text: &str, repo: &str) -> Option<Vec<(String, u64)>> {
    let mut found = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("# ") {
            if found {
                break;
            }
            found = head.split('@').next() == Some(repo);
            continue;
        }
        if found {
            let (path, size) = line.split_once('\t').unwrap_or((line, "0"));
            if !path.is_empty() {
                out.push((path.to_string(), size.trim().parse().unwrap_or(0)));
            }
        }
    }
    found.then_some(out)
}

/// Reads the file list of a GitHub repository (via `curl`).
pub fn github_tree(repo: &str, branch: &str) -> io::Result<Vec<(String, u64)>> {
    let url = format!("https://api.github.com/repos/{repo}/git/trees/{branch}?recursive=1");
    let (mut child, mut stdout) = curl(&url)?;
    let mut body = Vec::new();
    stdout.read_to_end(&mut body)?;
    let status = child.wait()?;
    if !status.success() && body.is_empty() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        return Err(io::Error::other(klang_i18n::tr!("Couldn't load the file list of {repo}: {err}", repo = repo, err = err.trim())));
    }
    parse_github_tree(&body)
}

/// URL of a file in a GitHub repository (path percent-encoded).
pub fn github_raw_url(repo: &str, branch: &str, path: &str) -> String {
    let mut enc = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("https://raw.githubusercontent.com/{repo}/{branch}/{enc}")
}

/// Path of ASCII characters only (umlauts transliterated, accents dropped, rest "_") –
/// so `curl` copes with the file names on any operating system.
pub fn ascii_path(path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..") {
        let mut s = String::new();
        for c in part.chars() {
            match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' | ',' | '(' | ')' => s.push(c),
                'ä' => s.push_str("ae"),
                'ö' => s.push_str("oe"),
                'ü' => s.push_str("ue"),
                'Ä' => s.push_str("Ae"),
                'Ö' => s.push_str("Oe"),
                'Ü' => s.push_str("Ue"),
                'ß' => s.push_str("ss"),
                'à' | 'á' | 'â' | 'ã' | 'å' => s.push('a'),
                'è' | 'é' | 'ê' | 'ë' | 'ě' => s.push('e'),
                'ì' | 'í' | 'î' | 'ï' => s.push('i'),
                'ò' | 'ó' | 'ô' | 'õ' | 'ø' => s.push('o'),
                'ù' | 'ú' | 'û' | 'ů' => s.push('u'),
                'ç' | 'č' | 'ć' => s.push('c'),
                'ñ' | 'ň' | 'ń' => s.push('n'),
                'š' | 'ś' => s.push('s'),
                'ž' | 'ź' | 'ż' => s.push('z'),
                'ř' => s.push('r'),
                'ý' => s.push('y'),
                'É' => s.push('E'),
                'Č' => s.push('C'),
                'Š' => s.push('S'),
                'Ž' => s.push('Z'),
                _ => s.push('_'),
            }
        }
        out.push(s);
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct FetchStats {
    /// Files in the target folder (new or already present).
    pub files: usize,
    pub existing: usize,
    /// Not downloaded (errors), with the first paths.
    pub failed: usize,
    pub failed_paths: Vec<String>,
    pub bytes: u64,
}

fn complete(target: &Path, size: u64) -> bool {
    target.metadata().map(|m| m.is_file() && (size == 0 || m.len() == size)).unwrap_or(false)
}

/// Entry for a `curl` configuration file (quotation marks, slashes).
fn curl_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "/").replace('"', "\\\""))
}

/// Downloads `files` to `dest` – files that are already complete (same size)
/// are skipped. For each batch a `curl` starts with several parallel
/// connections; meanwhile the process lives in `slot` (an abort can end it
/// there), `cancel` is checked between batches, `progress`
/// reports the state.
pub fn fetch_files(
    files: &[RemoteFile],
    dest: &Path,
    cancel: &AtomicBool,
    slot: &Mutex<Option<Child>>,
    mut progress: impl FnMut(&FetchStats),
) -> io::Result<FetchStats> {
    std::fs::create_dir_all(dest)?;
    let mut stats = FetchStats::default();
    let mut todo: Vec<&RemoteFile> = Vec::new();
    for f in files {
        if complete(&dest.join(&f.rel), f.size) {
            stats.existing += 1;
            stats.files += 1;
            stats.bytes += f.size;
        } else {
            todo.push(f);
        }
    }
    progress(&stats);
    let config = dest.join(".klangwelt-download.txt");
    let mut parallel = true;
    for batch in todo.chunks(48) {
        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&config);
            return Err(interrupted());
        }
        let mut cfg = String::new();
        for f in batch {
            let target = dest.join(&f.rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            cfg.push_str(&format!("url = {}\noutput = {}\n", curl_quote(&f.url), curl_quote(&target.display().to_string())));
        }
        std::fs::write(&config, cfg)?;
        let mut tries = 0;
        loop {
            tries += 1;
            let mut cmd = Command::new("curl");
            cmd.args(["--location", "--fail", "--silent", "--show-error", "--connect-timeout", "30", "--retry", "2"]);
            if parallel {
                cmd.args(["--parallel", "--parallel-max", "8"]);
            }
            cmd.arg("--config").arg(&config).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            let child = cmd.spawn().map_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    io::Error::new(io::ErrorKind::NotFound, klang_i18n::tr!("curl was not found (included in Windows 10 and later; on macOS/Linux, please install it)"))
                } else {
                    e
                }
            })?;
            *slot.lock().map_err(|_| io::Error::other(klang_i18n::tr!("Lock")))? = Some(child);
            // Wait without holding the lock (an abort ends the process via `slot`).
            let (status, mut child) = loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let mut g = slot.lock().map_err(|_| io::Error::other(klang_i18n::tr!("Lock")))?;
                let Some(c) = g.as_mut() else { return Err(interrupted()) };
                if let Some(st) = c.try_wait()? {
                    break (st, g.take().expect("Prozess"));
                }
            };
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            // Old curl (before 7.66) does not know --parallel: download one after another.
            if parallel && !status.success() && (err.contains("parallel") || status.code() == Some(2)) && tries == 1 {
                parallel = false;
                continue;
            }
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&config);
            return Err(interrupted());
        }
        for f in batch {
            let target = dest.join(&f.rel);
            if complete(&target, f.size) {
                stats.files += 1;
                stats.bytes += f.size;
            } else {
                // Do not leave error pages or half files behind
                let _ = std::fs::remove_file(&target);
                stats.failed += 1;
                if stats.failed_paths.len() < 5 {
                    stats.failed_paths.push(f.rel.display().to_string());
                }
            }
        }
        progress(&stats);
    }
    let _ = std::fs::remove_file(&config);
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use std::io::Write;

    /// HTTP server for tests: serves `data` (from a range if requested), but
    /// cuts off every response after `cut` bytes. `honor_range = false`: always
    /// serves from the start. Returns URL and number of requests.
    fn flaky_server(data: Vec<u8>, cut: usize, honor_range: bool) -> (String, Arc<AtomicU64>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicU64::new(0));
        let counter = hits.clone();
        let data = Arc::new(data);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                counter.fetch_add(1, Ordering::SeqCst);
                let data = data.clone();
                std::thread::spawn(move || {
                    let mut req = Vec::new();
                    let mut b = [0u8; 1];
                    while !req.ends_with(b"\r\n\r\n") {
                        if s.read(&mut b).unwrap_or(0) == 0 {
                            return;
                        }
                        req.push(b[0]);
                    }
                    let head = String::from_utf8_lossy(&req).to_lowercase();
                    let from = head
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.split('-').next())
                        .and_then(|n| n.trim().parse::<usize>().ok())
                        .filter(|_| honor_range)
                        .unwrap_or(0)
                        .min(data.len());
                    let total = data.len();
                    let body = &data[from..];
                    let head = if from > 0 {
                        format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{}/{total}\r\nConnection: close\r\n\r\n", body.len(), total - 1)
                    } else {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n")
                    };
                    let _ = s.write_all(head.as_bytes());
                    let _ = s.write_all(&body[..body.len().min(cut)]);
                    let _ = s.flush();
                    // Close the connection although bytes are still missing
                });
            }
        });
        (format!("http://127.0.0.1:{port}/archiv.tar.gz"), hits)
    }

    fn quick(d: &mut ResumingDownload) {
        d.pause = std::time::Duration::from_millis(5);
    }

    #[test]
    fn download_resumes_after_dropped_connections() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let (url, hits) = flaky_server(data.clone(), 70_000, true);
        let (slot, cancel) = (Mutex::new(None), AtomicBool::new(false));
        let resumes = std::cell::RefCell::new(Vec::new());
        let mut d = ResumingDownload::new(&url, &slot, &cancel, |pos, n, why| resumes.borrow_mut().push((pos, n, why.to_string()))).unwrap();
        quick(&mut d);
        let mut got = Vec::new();
        d.read_to_end(&mut got).unwrap();
        assert!(got == data, "Daten lückenlos und in der richtigen Reihenfolge ({} von {} Bytes)", got.len(), data.len());
        assert_eq!(d.resumed(), 4);
        drop(d);
        assert_eq!(hits.load(Ordering::SeqCst), 5);
        let r = resumes.borrow();
        assert_eq!(r.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(), vec![(70_000, 2), (140_000, 3), (210_000, 4), (280_000, 5)]);
        assert!(r[0].2.contains("(18)"), "Grund von curl: {}", r[0].2);
        assert!(slot.lock().unwrap().is_none(), "curl beendet");
    }

    #[test]
    fn archive_extracts_completely_across_dropped_connections() {
        let files: Vec<(String, Vec<u8>)> = (0..40).map(|i| (format!("mxl/{i}/stueck{i}.mxl"), vec![b'a' + (i % 26) as u8; 3000 + i * 97])).collect();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        let gz = archive(&refs);
        let (url, _) = flaky_server(gz.clone(), gz.len() / 5 + 1, true);
        let dir = std::env::temp_dir().join(format!("klang-fortsetzen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (slot, cancel) = (Mutex::new(None), AtomicBool::new(false));
        let mut d = ResumingDownload::new(&url, &slot, &cancel, |_, _, _| {}).unwrap();
        quick(&mut d);
        let stats = extract_scores(&mut d, &dir, Some("mxl"), None, &cancel, |_| {}).unwrap();
        assert_eq!(stats.files, 40);
        assert!(d.resumed() >= 4);
        for (name, data) in &files {
            let rel = name.strip_prefix("mxl/").unwrap();
            assert_eq!(&std::fs::read(dir.join(rel)).unwrap(), data, "{rel}");
        }
        drop(d);
        // Abort in the middle of the stream: unpacking reports "aborted" (Interrupted)
        let _ = std::fs::remove_dir_all(&dir);
        let (url, _) = flaky_server(gz.clone(), gz.len() / 5 + 1, true);
        let mut d = ResumingDownload::new(&url, &slot, &cancel, |_, _, _| cancel.store(true, Ordering::SeqCst)).unwrap();
        quick(&mut d);
        let err = extract_scores(&mut d, &dir, Some("mxl"), None, &cancel, |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        drop(d);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_gives_up_when_server_ignores_ranges_or_on_cancel() {
        // Server always serves from the start: no resuming (otherwise the stream would be broken)
        let data = vec![7u8; 50_000];
        let (url, hits) = flaky_server(data.clone(), 20_000, false);
        let (slot, cancel) = (Mutex::new(None), AtomicBool::new(false));
        let mut d = ResumingDownload::new(&url, &slot, &cancel, |_, _, _| {}).unwrap();
        quick(&mut d);
        let mut got = Vec::new();
        let err = d.read_to_end(&mut got).unwrap_err();
        assert!(err.to_string().contains("(33)"), "{err}");
        assert_eq!(got.len(), 20_000, "nichts doppelt");
        drop(d);
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        // Abort: no new attempt
        let (url, hits) = flaky_server(data, 20_000, true);
        let mut d = ResumingDownload::new(&url, &slot, &cancel, |_, _, _| {}).unwrap();
        quick(&mut d);
        let mut buf = vec![0u8; 20_000];
        d.read_exact(&mut buf).unwrap();
        cancel.store(true, Ordering::SeqCst);
        let err = d.read_to_end(&mut Vec::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted, "nicht Interrupted – read_to_end hinge sonst fest");
        drop(d);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn github_listing_urls_and_ascii_paths() {
        let json = br#"{"sha":"x","tree":[{"path":"kern","type":"tree"},{"path":"kern/chor001.krn","type":"blob","size":3106},{"path":"scores/Holm\u00e8s,_Augusta/Song/lc1.mxl","type":"blob","size":9}],"truncated":false}"#;
        let t = parse_github_tree(json).unwrap();
        assert_eq!(t, vec![("kern/chor001.krn".to_string(), 3106), ("scores/Holmès,_Augusta/Song/lc1.mxl".to_string(), 9)]);
        assert!(parse_github_tree(br#"{"message":"API rate limit exceeded for 1.2.3.4."}"#).unwrap_err().to_string().contains("60 requests"));
        assert_eq!(
            github_raw_url("OpenScore/Lieder", "main", "scores/Holmès,_A/x y.mxl"),
            "https://raw.githubusercontent.com/OpenScore/Lieder/main/scores/Holm%C3%A8s%2C_A/x%20y.mxl"
        );
        assert_eq!(ascii_path("scores/Holmès,_Augusta/Dvořák/../Grüße.mxl"), PathBuf::from("scores").join("Holmes,_Augusta").join("Dvorak").join("Gruesse.mxl"));
        assert_eq!(curl_quote(r#"C:\a "b".mxl"#), r#""C:/a \"b\".mxl""#);
        let m = "# Kopf\n# a/x@main\nkern/1.krn\t10\nkern/2.krn\t0\n# a/y@master\nkern/3.krn\t5\n";
        assert_eq!(manifest_files(m, "a/x"), Some(vec![("kern/1.krn".into(), 10), ("kern/2.krn".into(), 0)]));
        assert_eq!(manifest_files(m, "a/y"), Some(vec![("kern/3.krn".into(), 5)]));
        assert_eq!(manifest_files(m, "a/z"), None);
    }

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let gz = GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("klang-fetch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn extracts_scores_strips_prefix_and_respects_limit() {
        let data = archive(&[
            ("mxl/8/31/a.mxl", b"AAAA"),
            ("mxl/8/31/liesmich.txt", b"nein"),
            ("mxl/1/b.musicxml", b"BB"),
            ("mxl/1/c.xml", b"CCC"),
        ]);
        let dir = temp("limit");
        let never = AtomicBool::new(false);
        let mut calls = 0;
        let s = extract_scores(&data[..], &dir, Some("mxl"), Some(2), &never, |_| calls += 1).unwrap();
        assert_eq!((s.files, s.limit_reached, calls), (2, true, 2));
        assert_eq!(std::fs::read(dir.join("8/31/a.mxl")).unwrap(), b"AAAA");
        assert!(dir.join("1/b.musicxml").is_file());
        assert!(!dir.join("8/31/liesmich.txt").exists(), "nur Partituren");
        assert!(!dir.join("1/c.xml").exists(), "nach 2 Dateien Schluss");

        // Second run without a limit: existing files are recognized, the rest is added.
        let s = extract_scores(&data[..], &dir, Some("mxl"), None, &never, |_| {}).unwrap();
        assert_eq!((s.files, s.existing, s.limit_reached), (3, 2, false));
        assert!(dir.join("1/c.xml").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_paths_outside_the_target() {
        assert_eq!(safe_relative(Path::new("../boese.mxl"), None), None);
        assert_eq!(safe_relative(Path::new("/etc/boese.mxl"), None), None);
        assert_eq!(safe_relative(Path::new("mxl/x/../y.mxl"), Some("mxl")), None);
        assert_eq!(safe_relative(Path::new("./mxl/a/b.mxl"), Some("mxl")), Some(PathBuf::from("a/b.mxl")));
        assert_eq!(safe_relative(Path::new("mxl"), Some("mxl")), Some(PathBuf::from("mxl")));
    }

    #[test]
    fn cancel_stops_extraction() {
        let data = archive(&[("a.mxl", b"1"), ("b.mxl", b"2"), ("c.mxl", b"3")]);
        let dir = temp("cancel");
        let cancel = AtomicBool::new(false);
        let r = extract_scores(&data[..], &dir, None, None, &cancel, |s| {
            if s.files == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(!dir.join("c.mxl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn counting_reader_counts() {
        let n = Arc::new(AtomicU64::new(0));
        let mut r = Counting::new(&b"hallo welt"[..], n.clone());
        let mut s = String::new();
        r.read_to_string(&mut s).unwrap();
        assert_eq!(n.load(Ordering::Relaxed), 10);
    }
}
