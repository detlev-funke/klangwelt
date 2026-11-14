//! stdio bridge to the running Klangwelt app – for MCP clients that start their
//! server as a process (e.g. Claude Desktop). Reads JSON-RPC messages line by
//! line, forwards them to the app over HTTP and writes the replies back line
//! by line. Called by `klangwelt --mcp`.

use crate::client::post;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Starts the app if it is not reachable, and waits until it answers.
pub type StartApp<'a> = &'a mut dyn FnMut() -> Result<(), String>;

/// Keeps the session's event stream open as long as the bridge runs – so the
/// app sees the client as connected until it stops. If the stream breaks (for
/// example because the app restarts), it is reopened.
fn hold_while_running(url: &str, auth: Option<&str>, session: &Mutex<Option<String>>, running: &AtomicBool) {
    let current = || session.lock().map(|s| s.clone()).unwrap_or(None);
    while running.load(Ordering::SeqCst) {
        if let Some(s) = current() {
            let mut headers = vec![("Mcp-Session-Id", s.as_str())];
            if let Some(t) = auth {
                headers.push(("Authorization", t));
            }
            let keep = || running.load(Ordering::SeqCst) && current().as_deref() == Some(s.as_str());
            let _ = crate::client::hold_stream(url, &headers, &keep);
        }
        // session still unknown or stream ended: wait briefly
        for _ in 0..10 {
            if !running.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Runs until `input` ends. `start_app` is tried at most once – the first
/// time the app is not reachable.
pub fn run(url: &str, token: Option<&str>, input: impl BufRead, mut out: impl Write, mut start_app: Option<StartApp>) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut session: Option<String> = None;
    let mut tried_start = false;
    let shared = Arc::new(Mutex::new(None::<String>));
    let running = Arc::new(AtomicBool::new(true));
    let holder = {
        let (url, auth, shared, running) = (url.to_string(), auth.clone(), shared.clone(), running.clone());
        std::thread::Builder::new()
            .name("mcp-strom".into())
            .spawn(move || hold_while_running(&url, auth.as_deref(), &shared, &running))
            .ok()
    };

    for line in input.lines() {
        let Ok(line) = line else { break };
        // Some shells (Windows PowerShell) prepend a BOM.
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        let id = match serde_json::from_str::<Value>(line) {
            Ok(v) => v.get("id").cloned(),
            Err(e) => {
                let msg = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Invalid JSON: {e}") } });
                let _ = writeln!(out, "{msg}");
                let _ = out.flush();
                continue;
            }
        };

        let send = |session: &Option<String>| {
            let mut headers: Vec<(&str, &str)> = Vec::new();
            if let Some(s) = session {
                headers.push(("Mcp-Session-Id", s));
            }
            if let Some(t) = &auth {
                headers.push(("Authorization", t));
            }
            post(url, line.as_bytes(), &headers)
        };
        let mut result = send(&session);
        if result.is_err() && !tried_start {
            if let Some(start) = start_app.as_deref_mut() {
                tried_start = true;
                result = match start() {
                    Ok(()) => send(&session),
                    Err(e) => Err(std::io::Error::other(e)),
                };
            }
        }

        let reply = match result {
            // Notifications without an id get no reply even on failure (JSON-RPC).
            Ok(r) => {
                if let Some(s) = r.header("mcp-session-id") {
                    session = Some(s.to_string());
                    if let Ok(mut g) = shared.lock() {
                        *g = session.clone();
                    }
                }
                match r.status {
                    202 => None,
                    200 => Some(String::from_utf8_lossy(&r.body).into_owned()),
                    code => Some(error(id.unwrap_or(Value::Null), &format!("Klangwelt antwortet mit HTTP {code}"))),
                }
            }
            Err(e) => id.map(|id| error(id, &format!("Klangwelt cannot be reached ({url}): {e}. Is the app running?"))),
        };
        if let Some(r) = reply {
            let _ = writeln!(out, "{}", r.trim());
            let _ = out.flush();
        }
    }
    // Client has stopped: close the stream and end the session – the app then
    // shows "No agent connected".
    running.store(false, Ordering::SeqCst);
    if let Some(h) = holder {
        let _ = h.join();
    }
    if let Some(s) = &session {
        let mut headers = vec![("Mcp-Session-Id", s.as_str())];
        if let Some(t) = &auth {
            headers.push(("Authorization", t));
        }
        let _ = crate::client::delete(url, &headers);
    }
}

fn error(id: Value, msg: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": msg } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{serve, Handler, ServerInfo, Tool};
    use std::io::Cursor;
    use std::sync::Arc;

    struct Echo;
    impl Handler for Echo {
        fn info(&self) -> ServerInfo {
            ServerInfo { name: "test".into(), title: "Test".into(), version: "0".into(), instructions: String::new() }
        }
        fn tools(&self) -> Vec<Tool> {
            Vec::new()
        }
        fn call(&self, _name: &str, _args: Value) -> Result<Value, String> {
            Ok(json!({}))
        }
    }

    fn lines(out: Vec<u8>) -> Vec<Value> {
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn forwards_requests_and_skips_notifications() {
        let server = serve("127.0.0.1:0", Arc::new(Echo), None).expect("Server");
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\nkein json\n";
        let mut out = Vec::new();
        run(&server.url(), None, Cursor::new(input), &mut out, None);
        server.stop();
        let got = lines(out);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0]["id"], 1);
        assert!(got[0].get("result").is_some(), "{:?}", got[0]);
        assert_eq!(got[1]["error"]["code"], -32700);
    }

    #[test]
    fn starts_the_app_once_when_unreachable() {
        // free port without a server
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let mut started = 0;
        let mut server = None;
        let mut start = || {
            started += 1;
            server = Some(serve(("127.0.0.1", port), Arc::new(Echo), None).map_err(|e| e.to_string())?);
            Ok(())
        };
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        run(&url, None, Cursor::new(input), &mut out, Some(&mut start));
        assert_eq!(started, 1);
        let got = lines(out);
        assert!(got.iter().all(|v| v.get("result").is_some()), "{got:?}");
        if let Some(s) = server {
            s.stop();
        }
    }

    /// Input that delivers lines from a channel and waits until the sender drops (like stdin).
    struct ChannelInput {
        rx: std::sync::mpsc::Receiver<String>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl std::io::Read for ChannelInput {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(line) => (self.buf, self.pos) = (line.into_bytes(), 0),
                    Err(_) => return Ok(0),
                }
            }
            let n = (self.buf.len() - self.pos).min(out.len());
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn holds_a_stream_while_the_client_runs() {
        let server = serve("127.0.0.1:0", Arc::new(Echo), None).expect("Server");
        let url = server.url();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let bridge = std::thread::spawn(move || {
            let input = std::io::BufReader::new(ChannelInput { rx, buf: Vec::new(), pos: 0 });
            run(&url, None, input, std::io::sink(), None);
        });
        tx.send("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n".into()).unwrap();
        let wait = |f: &dyn Fn() -> bool| {
            let start = std::time::Instant::now();
            while !f() && start.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(20));
            }
            f()
        };
        assert!(wait(&|| server.presence().open_connections() == 1), "Brücke hält einen Strom offen");
        assert_eq!(server.presence().connected(Duration::ZERO), 1);
        drop(tx); // client stops
        bridge.join().unwrap();
        assert!(wait(&|| server.presence().open_connections() == 0), "Strom geschlossen");
        assert_eq!(server.presence().connected(Duration::from_secs(600)), 0, "Sitzung beendet");
        server.stop();
    }

    #[test]
    fn reports_unreachable_app_without_start() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut out = Vec::new();
        run(&format!("http://127.0.0.1:{port}/mcp"), None, Cursor::new("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n"), &mut out, None);
        let got = lines(out);
        assert_eq!(got[0]["id"], 7);
        assert!(got[0]["error"]["message"].as_str().unwrap().contains("cannot be reached"));
    }
}
