//! Minimal HTTP/1.1 client (only `http://`, one request per connection).

use crate::http::{read_head, read_head_and_body};
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }
}

/// Splits `http://host:port/path` into (host:port, path).
pub fn split_url(url: &str) -> io::Result<(String, String)> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "only http:// is supported"))?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = if host.contains(':') && !host.ends_with(']') { host.to_string() } else { format!("{host}:80") };
    Ok((host, path.to_string()))
}

pub fn post(url: &str, body: &[u8], headers: &[(&str, &str)]) -> io::Result<Response> {
    request("POST", url, body, headers)
}

/// `DELETE` – e.g. to end an MCP session.
pub fn delete(url: &str, headers: &[(&str, &str)]) -> io::Result<Response> {
    request("DELETE", url, &[], headers)
}

/// Connects to `url`; returns the connection along with host:port and path.
fn connect(url: &str) -> io::Result<(TcpStream, String, String)> {
    let (host, path) = split_url(url)?;
    let addr = host
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "address cannot be resolved"))?;
    Ok((TcpStream::connect_timeout(&addr, Duration::from_secs(3))?, host, path))
}

/// Opens the event stream (`GET` with `Accept: text/event-stream`) and keeps it
/// open until the server closes it or `keep()` says no – for as long as that,
/// the server sees the client as connected. Whatever arrives in the stream is discarded.
pub fn hold_stream(url: &str, headers: &[(&str, &str)], keep: &dyn Fn() -> bool) -> io::Result<()> {
    let (mut stream, host, path) = connect(url)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\nCache-Control: no-store\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes())?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let (line, _) = read_head(&mut reader)?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no response"))?;
    let status: u16 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if status != 200 {
        return Err(io::Error::other(format!("Ereignisstrom abgelehnt: HTTP {status}")));
    }
    reader.get_ref().set_read_timeout(Some(Duration::from_millis(500)))?;
    let mut buf = [0u8; 512];
    while keep() {
        match reader.read(&mut buf) {
            Ok(0) => break, // server closed
            Ok(_) => {}
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn request(method: &str, url: &str, body: &[u8], headers: &[(&str, &str)]) -> io::Result<Response> {
    let (mut stream, host, path) = connect(url)?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let (line, headers, body) = read_head_and_body(&mut reader)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no response"))?;
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(Response { status, headers, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        assert_eq!(split_url("http://127.0.0.1:7419/mcp").unwrap(), ("127.0.0.1:7419".into(), "/mcp".into()));
        assert_eq!(split_url("http://localhost").unwrap(), ("localhost:80".into(), "/".into()));
        assert!(split_url("https://x").is_err());
    }
}
