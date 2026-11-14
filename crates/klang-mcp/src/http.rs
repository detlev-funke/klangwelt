//! MCP "Streamable HTTP" transport, minimal and blocking.
//!
//! * `POST /mcp` - JSON-RPC message (or batch) → JSON response or `202 Accepted`
//! * `GET /mcp`  - event stream (`text/event-stream`) that stays open until the
//!   client closes it. The server only sends keep-alives (SSE comments);
//!   it has no messages of its own for the client.
//! * `DELETE /mcp` - end the session
//!
//! Security: binds to loopback only, checks the `Origin` header against
//! DNS rebinding, optionally a bearer token.
//!
//! Connected clients ([`Presence`]): a client counts as connected as long as it
//! keeps a connection open - its event stream (Claude Code opens it
//! right after `initialize`) or a request that is still running (e.g. a
//! waiting long poll). If it closes them, the server notices immediately. Clients
//! that never open a stream (scripts) count for a while after their last
//! request; `DELETE` ends a session immediately.

use crate::protocol::{handle, Handler};
use serde_json::Value;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const MAX_BODY: usize = 16 * 1024 * 1024;
const MAX_HEADERS: usize = 100;
/// The server remembers at most this many sessions (the ones idle the longest drop out).
const MAX_SESSIONS: usize = 64;
/// Keep-alives in the event stream so that no client or intermediary closes it
/// for lack of traffic (Node/undici aborts after 300 s without data).
const KEEPALIVE: Duration = Duration::from_secs(20);
/// For this long a client still counts as connected after closing its stream
/// - bridges a brief reconnect without the display flickering.
pub const STREAM_GRACE: Duration = Duration::from_secs(3);

/// Who is connected: open event streams, running requests, last request.
#[derive(Default)]
pub struct Presence {
    inner: Mutex<PresenceInner>,
}

#[derive(Default)]
struct PresenceInner {
    sessions: HashMap<String, Client>,
    /// Requests without a session (e.g. a script that calls tools directly) - together one client
    anonymous: Client,
}

#[derive(Default)]
struct Client {
    /// Last request (POST)
    last: Option<Instant>,
    /// Open event streams (GET)
    streams: usize,
    /// Requests that are currently running (e.g. a waiting long poll)
    requests: usize,
    /// When the last stream was closed (None = none was ever open)
    stream_closed: Option<Instant>,
}

impl Client {
    /// Holds a connection open or only just did. Whoever never had a
    /// stream (or asks again since closing it) counts for `grace`
    /// after its last request.
    fn connected(&self, grace: Duration) -> bool {
        if self.streams > 0 || self.requests > 0 {
            return true;
        }
        if self.stream_closed.is_some_and(|t| t.elapsed() < STREAM_GRACE) {
            return true;
        }
        self.last.is_some_and(|l| l.elapsed() < grace && self.stream_closed.is_none_or(|c| l > c))
    }

    fn busy(&self) -> bool {
        self.streams > 0 || self.requests > 0
    }
}

/// Counts an open connection for as long as it lives.
struct Open<'a> {
    presence: &'a Presence,
    session: Option<String>,
    stream: bool,
}

impl Drop for Open<'_> {
    fn drop(&mut self) {
        let mut g = self.presence.lock();
        let c = match &self.session {
            Some(id) => match g.sessions.get_mut(id) {
                Some(c) => c,
                None => return, // ended via DELETE
            },
            None => &mut g.anonymous,
        };
        if self.stream {
            c.streams = c.streams.saturating_sub(1);
            if c.streams == 0 {
                c.stream_closed = Some(Instant::now());
            }
        } else {
            c.requests = c.requests.saturating_sub(1);
        }
    }
}

impl Presence {
    fn lock(&self) -> MutexGuard<'_, PresenceInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Entry of the client; an unknown session is created - for example a
    /// client that keeps working with its old session after an app restart.
    fn client<'a>(g: &'a mut PresenceInner, session: Option<&str>) -> &'a mut Client {
        let Some(id) = session else { return &mut g.anonymous };
        if g.sessions.len() >= MAX_SESSIONS && !g.sessions.contains_key(id) {
            let quiet = g.sessions.iter().filter(|(_, c)| !c.busy()).min_by_key(|(_, c)| c.last).map(|(k, _)| k.clone());
            if let Some(k) = quiet {
                g.sessions.remove(&k);
            }
        }
        g.sessions.entry(id.to_string()).or_default()
    }

    /// A request arrived (with or without a session).
    pub fn seen(&self, session: Option<&str>) {
        let mut g = self.lock();
        Self::client(&mut g, session).last = Some(Instant::now());
    }

    /// Request that counts as an open connection until it ends.
    fn request(&self, session: Option<&str>) -> Open<'_> {
        let mut g = self.lock();
        let c = Self::client(&mut g, session);
        c.last = Some(Instant::now());
        c.requests += 1;
        Open { presence: self, session: session.map(str::to_string), stream: false }
    }

    /// Event stream opened; counts until the guard drops.
    fn stream(&self, session: Option<&str>) -> Open<'_> {
        let mut g = self.lock();
        Self::client(&mut g, session).streams += 1;
        Open { presence: self, session: session.map(str::to_string), stream: true }
    }

    /// Session ended (`DELETE`).
    pub fn closed(&self, session: &str) {
        self.lock().sessions.remove(session);
    }

    /// Connected clients: with an open connection (stream, running request) or
    /// - for those that never opened a stream - with a request within `grace`.
    pub fn connected(&self, grace: Duration) -> usize {
        let g = self.lock();
        g.sessions.values().filter(|c| c.connected(grace)).count() + usize::from(g.anonymous.connected(grace))
    }

    /// Open connections (streams and running requests) of all clients.
    pub fn open_connections(&self) -> usize {
        let g = self.lock();
        g.sessions.values().chain(std::iter::once(&g.anonymous)).map(|c| c.streams + c.requests).sum()
    }
}

pub struct ServerHandle {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    presence: Arc<Presence>,
}

impl ServerHandle {
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// Connected clients.
    pub fn presence(&self) -> &Presence {
        &self.presence
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts the server in its own threads. With port 0 the system picks a free port.
pub fn serve(addr: impl ToSocketAddrs, handler: Arc<dyn Handler>, token: Option<String>) -> io::Result<ServerHandle> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "MCP-Server darf nur an Loopback gebunden werden"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let token = Arc::new(token);
    let presence = Arc::new(Presence::default());
    let presence2 = presence.clone();
    std::thread::Builder::new().name("mcp-accept".into()).spawn(move || {
        for conn in listener.incoming() {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            let Ok(stream) = conn else { continue };
            let (h, t, p, s) = (handler.clone(), token.clone(), presence2.clone(), stop2.clone());
            let _ = std::thread::Builder::new().name("mcp-conn".into()).spawn(move || {
                let _ = connection(stream, h.as_ref(), t.as_deref(), &p, &s);
            });
        }
    })?;
    Ok(ServerHandle { addr: local, stop, presence })
}

pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Reads header lines and body (Content-Length or chunked).
pub(crate) fn read_head_and_body<R: BufRead>(r: &mut R) -> io::Result<Option<(String, Vec<(String, String)>, Vec<u8>)>> {
    let Some((first, headers)) = read_head(r)? else { return Ok(None) };
    let body = read_body(r, &headers)?;
    Ok(Some((first, headers, body)))
}

/// Reads the start line and header lines (names lowercased); None at the end of the connection.
pub(crate) fn read_head<R: BufRead>(r: &mut R) -> io::Result<Option<(String, Vec<(String, String)>)>> {
    let mut first = String::new();
    loop {
        first.clear();
        if r.read_line(&mut first)? == 0 {
            return Ok(None);
        }
        if !first.trim().is_empty() {
            break;
        }
    }
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Err(bad("Verbindung mitten im Header beendet"));
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADERS {
            return Err(bad("zu viele Header"));
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    Ok(Some((first.trim_end().to_string(), headers)))
}

/// Reads the body after the header lines (Content-Length or chunked).
fn read_body<R: BufRead>(r: &mut R, headers: &[(String, String)]) -> io::Result<Vec<u8>> {
    let get = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let mut body = Vec::new();
    if get("transfer-encoding").map(|v| v.to_ascii_lowercase().contains("chunked")).unwrap_or(false) {
        loop {
            let mut size_line = String::new();
            r.read_line(&mut size_line)?;
            let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
                .map_err(|_| bad("invalid chunk size"))?;
            if size == 0 {
                let mut end = String::new();
                r.read_line(&mut end)?;
                break;
            }
            if body.len() + size > MAX_BODY {
                return Err(bad("body too large"));
            }
            let start = body.len();
            body.resize(start + size, 0);
            r.read_exact(&mut body[start..])?;
            let mut crlf = [0u8; 2];
            r.read_exact(&mut crlf)?;
        }
    } else if let Some(len) = get("content-length") {
        let len: usize = len.parse().map_err(|_| bad("invalid Content-Length"))?;
        if len > MAX_BODY {
            return Err(bad("body too large"));
        }
        body.resize(len, 0);
        r.read_exact(&mut body)?;
    }
    Ok(body)
}

fn read_request<R: BufRead>(r: &mut R) -> io::Result<Option<Request>> {
    let Some((line, headers, body)) = read_head_and_body(r)? else { return Ok(None) };
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/").to_string();
    Ok(Some(Request { method, path, headers, body }))
}

fn write_response(w: &mut impl Write, status: u16, extra: &[(&str, String)], body: &[u8], close: bool) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n", body.len());
    if !body.is_empty() {
        head.push_str("Content-Type: application/json; charset=utf-8\r\n");
    }
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(if close { "Connection: close\r\n\r\n" } else { "Connection: keep-alive\r\n\r\n" });
    w.write_all(head.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

/// Allows only pages from localhost (protection against DNS rebinding from the browser).
fn origin_allowed(origin: &str) -> bool {
    let rest = origin.split("://").nth(1).unwrap_or(origin);
    let host = if rest.starts_with('[') {
        rest.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        rest.split([':', '/']).next().unwrap_or("").to_string()
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") || host.ends_with(".localhost")
}

fn session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    let a = h.finish();
    let mut h2 = RandomState::new().build_hasher();
    h2.write_u64(a);
    format!("{:016x}{:016x}", a, h2.finish())
}

thread_local! {
    /// Connection of the request this thread is currently handling.
    static CALLER: std::cell::RefCell<Option<TcpStream>> = const { std::cell::RefCell::new(None) };
}

/// Has the client closed the connection of the running request (e.g. because
/// its timeout expired)? For long waits such as long polls: then
/// it is not worth waiting any longer, and results would never arrive.
pub fn caller_gone() -> bool {
    CALLER.with(|c| c.borrow().as_ref().is_some_and(peer_gone))
}

/// Has the other side closed the connection? (Peeks without reading.)
fn peer_gone(s: &TcpStream) -> bool {
    if s.set_nonblocking(true).is_err() {
        return false;
    }
    let mut buf = [0u8; 1];
    let gone = match s.peek(&mut buf) {
        Ok(0) => true,                                             // closed cleanly
        Ok(_) => false,                                            // more data (next request)
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => false, // still open, nothing there
        Err(_) => true,                                            // reset
    };
    let _ = s.set_nonblocking(false);
    gone
}

fn wants_events(req: &Request) -> bool {
    req.header("accept").is_some_and(|a| a.contains("text/event-stream"))
}

/// `GET /mcp`: keep the event stream open until the client closes it (or the
/// server stops). It carries only keep-alives; as long as it is open, the
/// client counts as connected.
fn event_stream(w: &mut TcpStream, watch: &TcpStream, presence: &Presence, session: Option<&str>, stop: &AtomicBool) -> io::Result<()> {
    w.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")?;
    let _open = presence.stream(session);
    let chunk = |w: &mut TcpStream, s: &str| -> io::Result<()> {
        write!(w, "{:x}\r\n{s}\r\n", s.len())?;
        w.flush()
    };
    chunk(w, ": verbunden\n\n")?;
    let mut last = Instant::now();
    while !stop.load(Ordering::Relaxed) && !peer_gone(watch) {
        std::thread::sleep(Duration::from_millis(250));
        if last.elapsed() >= KEEPALIVE {
            chunk(w, ": ping\n\n")?;
            last = Instant::now();
        }
    }
    let _ = w.write_all(b"0\r\n\r\n");
    Ok(())
}

fn connection(stream: TcpStream, handler: &dyn Handler, token: Option<&str>, presence: &Presence, stop: &AtomicBool) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(300)))?;
    let mut writer = stream.try_clone()?;
    let watch = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    while let Some(req) = read_request(&mut reader)? {
        if req.method == "GET" && wants_events(&req) && reject(&req, token).is_none() {
            let session = req.header("mcp-session-id").filter(|s| !s.is_empty());
            return event_stream(&mut writer, &watch, presence, session, stop);
        }
        let close = req.header("connection").map(|v| v.eq_ignore_ascii_case("close")).unwrap_or(false);
        CALLER.with(|c| *c.borrow_mut() = watch.try_clone().ok());
        let (status, headers, body) = respond(&req, handler, token, presence);
        CALLER.with(|c| *c.borrow_mut() = None);
        write_response(&mut writer, status, &headers, &body, close)?;
        if close {
            break;
        }
    }
    Ok(())
}

fn rpc_error(code: i64, msg: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": msg } })).unwrap_or_default()
}

type Full = (u16, Vec<(&'static str, String)>, Vec<u8>);

/// Checks path, origin and token; `Some` = rejection.
fn reject(req: &Request, token: Option<&str>) -> Option<Full> {
    if req.path != "/mcp" && req.path != "/" {
        return Some((404, vec![], vec![]));
    }
    if let Some(origin) = req.header("origin") {
        if !origin_allowed(origin) {
            return Some((403, vec![], rpc_error(-32000, "Origin not allowed")));
        }
    }
    if let Some(t) = token {
        let ok = req.header("authorization").map(|v| v.strip_prefix("Bearer ").unwrap_or("") == t).unwrap_or(false);
        if !ok {
            return Some((401, vec![("WWW-Authenticate", "Bearer".into())], rpc_error(-32001, "Token missing or wrong")));
        }
    }
    None
}

fn respond(req: &Request, handler: &dyn Handler, token: Option<&str>, presence: &Presence) -> Full {
    if let Some(r) = reject(req, token) {
        return r;
    }
    let session = req.header("mcp-session-id").filter(|s| !s.is_empty());
    match req.method.as_str() {
        "POST" => {}
        "DELETE" => {
            if let Some(s) = session {
                presence.closed(s);
            }
            return (200, vec![], vec![]);
        }
        // event stream only with `Accept: text/event-stream` (see `connection`)
        "GET" => return (405, vec![("Allow", "GET, POST, DELETE".into())], vec![]),
        "OPTIONS" => return (204, vec![("Allow", "GET, POST, DELETE".into())], vec![]),
        _ => return (405, vec![("Allow", "GET, POST, DELETE".into())], vec![]),
    }

    let body = req.body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&req.body);
    let msg: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return (400, vec![], rpc_error(-32700, "JSON could not be parsed")),
    };
    let mut headers = Vec::new();
    let is_init = |m: &Value| m.get("method").and_then(Value::as_str) == Some("initialize");
    let init = match &msg {
        Value::Array(list) => list.iter().any(is_init),
        single => is_init(single),
    };
    // The request counts as an open connection until it is answered (long poll)
    let _open = if init {
        let id = session_id();
        presence.seen(Some(&id));
        headers.push(("Mcp-Session-Id", id));
        None
    } else {
        Some(presence.request(session))
    };
    let reply = match &msg {
        Value::Array(list) => {
            let out: Vec<Value> = list.iter().filter_map(|m| handle(handler, m)).collect();
            (!out.is_empty()).then(|| Value::Array(out))
        }
        single => handle(handler, single),
    };
    match reply {
        Some(v) => (200, headers, serde_json::to_vec(&v).unwrap_or_default()),
        None => (202, headers, vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::post;
    use crate::protocol::tests::Echo;
    use serde_json::json;

    fn server(token: Option<&str>) -> ServerHandle {
        serve("127.0.0.1:0", Arc::new(Echo), token.map(str::to_string)).unwrap()
    }

    #[test]
    fn full_roundtrip_over_tcp() {
        let s = server(None);
        let url = s.url();
        let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } });
        let r = post(&url, init.to_string().as_bytes(), &[]).unwrap();
        assert_eq!(r.status, 200);
        assert!(r.header("mcp-session-id").map(|s| s.len() == 32).unwrap_or(false));
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "echo");

        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(post(&url, note.to_string().as_bytes(), &[]).unwrap().status, 202);

        let call = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "echo", "arguments": { "ä": "ü" } } });
        let r = post(&url, call.to_string().as_bytes(), &[]).unwrap();
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["result"]["structuredContent"]["got"]["ä"], "ü");

        let batch = json!([note, { "jsonrpc": "2.0", "id": 3, "method": "ping" }]);
        let r = post(&url, batch.to_string().as_bytes(), &[]).unwrap();
        let v: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);

        assert_eq!(post(&url, b"{kaputt", &[]).unwrap().status, 400);
        let bom = [b"\xEF\xBB\xBF".as_slice(), br#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#].concat();
        assert_eq!(post(&url, &bom, &[]).unwrap().status, 200);
    }

    fn wait_until(mut f: impl FnMut() -> bool, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    fn init_session(url: &str) -> String {
        let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} });
        post(url, init.to_string().as_bytes(), &[]).unwrap().header("mcp-session-id").unwrap().to_string()
    }

    /// Opens the event stream raw over TCP; as long as the connection lives, the client is connected.
    fn open_stream(url: &str, session: &str) -> TcpStream {
        use std::io::Read;
        let addr = url.trim_start_matches("http://").trim_end_matches("/mcp").to_string();
        let mut c = TcpStream::connect(&addr).unwrap();
        write!(c, "GET /mcp HTTP/1.1\r\nHost: {addr}\r\nAccept: text/event-stream\r\nMcp-Session-Id: {session}\r\n\r\n").unwrap();
        let mut buf = [0u8; 512];
        let n = c.read(&mut buf).unwrap();
        let head = String::from_utf8_lossy(&buf[..n]);
        assert!(head.starts_with("HTTP/1.1 200") && head.contains("text/event-stream"), "{head}");
        c
    }

    #[test]
    fn presence_follows_open_connections() {
        let s = server(None);
        let url = s.url();
        let p = s.presence();
        let long = Duration::from_secs(600);
        assert_eq!(p.connected(long), 0, "anfangs niemand");
        // client without a stream (e.g. a script): counts for a while after its last request
        let sid = init_session(&url);
        assert_eq!(p.connected(long), 1);
        assert_eq!(p.connected(Duration::ZERO), 0, "hält keine Verbindung offen");
        // DELETE ends the session immediately
        assert_eq!(crate::client::delete(&url, &[("Mcp-Session-Id", &sid)]).unwrap().status, 200);
        assert_eq!(p.connected(long), 0);

        // client with an event stream (like Claude Code): connected as long as the stream is open
        let sid = init_session(&url);
        let stream = open_stream(&url, &sid);
        assert!(wait_until(|| p.open_connections() == 1, Duration::from_secs(2)));
        assert_eq!(p.connected(Duration::ZERO), 1, "offener Strom zählt ohne Zeitfenster");
        let ping = json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }).to_string();
        post(&url, ping.as_bytes(), &[("Mcp-Session-Id", &sid)]).unwrap();
        assert_eq!(p.connected(long), 1, "weiterhin ein Client");
        // client closes the stream: the server notices immediately, after the short
        // grace period it counts as disconnected - even though its last request is fresh
        drop(stream);
        assert!(wait_until(|| p.open_connections() == 0, Duration::from_secs(2)), "Schließen bemerkt");
        assert!(wait_until(|| p.connected(long) == 0, STREAM_GRACE + Duration::from_secs(2)), "getrennt");

        // requests without a session together count as one client
        post(&url, ping.as_bytes(), &[]).unwrap();
        post(&url, ping.as_bytes(), &[]).unwrap();
        assert_eq!(p.connected(long), 1);
    }

    #[test]
    fn get_without_event_stream_is_refused() {
        let s = server(Some("geheim"));
        let addr = s.url().trim_start_matches("http://").trim_end_matches("/mcp").to_string();
        let get = |extra: &str| {
            use std::io::Read;
            let mut c = TcpStream::connect(&addr).unwrap();
            write!(c, "GET /mcp HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{extra}\r\n").unwrap();
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            out.lines().next().unwrap_or("").to_string()
        };
        assert!(get("Authorization: Bearer geheim\r\n").contains("405"), "ohne Accept: text/event-stream");
        assert!(get("Accept: text/event-stream\r\n").contains("401"), "ohne Token");
        assert_eq!(s.presence().open_connections(), 0);
    }

    #[test]
    fn rejects_foreign_origin_and_missing_token() {
        let s = server(Some("geheim"));
        let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();
        assert_eq!(post(&s.url(), ping.as_bytes(), &[]).unwrap().status, 401);
        let auth = [("Authorization", "Bearer geheim")];
        assert_eq!(post(&s.url(), ping.as_bytes(), &auth).unwrap().status, 200);
        let evil = [("Authorization", "Bearer geheim"), ("Origin", "http://evil.example")];
        assert_eq!(post(&s.url(), ping.as_bytes(), &evil).unwrap().status, 403);
        let local = [("Authorization", "Bearer geheim"), ("Origin", "http://localhost:5173")];
        assert_eq!(post(&s.url(), ping.as_bytes(), &local).unwrap().status, 200);
    }

    /// Tool that waits until the caller hangs up, and reports that.
    struct Waiter(Arc<std::sync::atomic::AtomicBool>);
    impl Handler for Waiter {
        fn info(&self) -> crate::ServerInfo {
            crate::ServerInfo { name: "w".into(), title: "W".into(), version: "1".into(), instructions: String::new() }
        }
        fn tools(&self) -> Vec<crate::Tool> {
            vec![crate::Tool { name: "wait", title: "Wait", description: "", input_schema: json!({ "type": "object" }), read_only: true }]
        }
        fn call(&self, _: &str, _: Value) -> Result<Value, String> {
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if caller_gone() {
                    self.0.store(true, Ordering::SeqCst);
                    return Ok(json!("weg"));
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            Ok(json!("Zeit abgelaufen"))
        }
    }

    #[test]
    fn notices_when_the_caller_hangs_up() {
        use std::io::Write;
        let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = serve("127.0.0.1:0", Arc::new(Waiter(seen.clone())), None).unwrap();
        let addr = s.url().trim_start_matches("http://").trim_end_matches("/mcp").to_string();
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "wait" } }).to_string();
        let mut c = TcpStream::connect(&addr).unwrap();
        write!(c, "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!seen.load(Ordering::SeqCst), "Verbindung ist noch offen");
        // the waiting request keeps the connection open - the client counts as connected
        assert_eq!(s.presence().open_connections(), 1);
        assert_eq!(s.presence().connected(Duration::ZERO), 1);
        drop(c); // client gives up (as on a timeout)
        let start = std::time::Instant::now();
        while !seen.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(seen.load(Ordering::SeqCst), "Abbruch wurde nicht bemerkt");
        assert!(wait_until(|| s.presence().open_connections() == 0, Duration::from_secs(2)));
        assert_eq!(s.presence().connected(Duration::ZERO), 0);
    }

    #[test]
    fn origin_rules() {
        assert!(origin_allowed("http://localhost:3000"));
        assert!(origin_allowed("http://127.0.0.1"));
        assert!(origin_allowed("http://[::1]:8080"));
        assert!(origin_allowed("http://tauri.localhost"));
        assert!(!origin_allowed("https://localhost.evil.com"));
        assert!(!origin_allowed("http://192.168.1.4"));
    }

    #[test]
    fn chunked_body_is_read() {
        let raw = b"POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let req = read_request(&mut &raw[..]).unwrap().unwrap();
        assert_eq!(req.body, b"Wikipedia");
        assert_eq!(req.method, "POST");
    }
}
