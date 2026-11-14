//! JSON-RPC 2.0 nach MCP-Spezifikation (nur Server-Seite: Werkzeuge und Prompts).

use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Unterstützte Protokollversionen, neueste zuerst.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Clone, Debug)]
pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// JSON-Schema der Argumente (`type: object`).
    pub input_schema: Value,
    /// Werkzeug verändert nichts.
    pub read_only: bool,
}

/// Vorgefertigte Anweisung, die Clients als Befehl anbieten
/// (Claude Code: `/mcp__<server>__<name>`).
#[derive(Clone, Debug)]
pub struct Prompt {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Text der Nutzernachricht, die der Prompt erzeugt.
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub name: String,
    pub title: String,
    pub version: String,
    /// Hinweise für das Modell, wie der Server zu benutzen ist.
    pub instructions: String,
}

pub trait Handler: Send + Sync + 'static {
    fn info(&self) -> ServerInfo;
    fn tools(&self) -> Vec<Tool>;
    /// `Ok(wert)` wird als Text und (bei Objekten) als `structuredContent`
    /// zurückgegeben; `Err(text)` als Werkzeugfehler (`isError`), den das Modell sieht.
    fn call(&self, name: &str, arguments: Value) -> Result<Value, String>;
    /// Prompts (Standard: keine).
    fn prompts(&self) -> Vec<Prompt> {
        Vec::new()
    }
}

fn prompt_json(p: &Prompt) -> Value {
    json!({ "name": p.name, "title": p.title, "description": p.description, "arguments": [] })
}

fn prompts_get(h: &dyn Handler, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "Parameter „name“ fehlt".to_string()))?;
    let p = h
        .prompts()
        .into_iter()
        .find(|p| p.name == name)
        .ok_or((-32602, format!("Unbekannter Prompt: {name}")))?;
    Ok(json!({
        "description": p.description,
        "messages": [{ "role": "user", "content": { "type": "text", "text": p.text } }]
    }))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_json(t: &Tool) -> Value {
    json!({
        "name": t.name,
        "title": t.title,
        "description": t.description,
        "inputSchema": t.input_schema,
        "annotations": {
            "title": t.title,
            "readOnlyHint": t.read_only,
            "destructiveHint": false,
            "openWorldHint": false
        }
    })
}

fn initialize(h: &dyn Handler, params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
    let version = PROTOCOL_VERSIONS.iter().find(|v| **v == requested).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
    let info = h.info();
    let mut capabilities = json!({ "tools": { "listChanged": false } });
    if !h.prompts().is_empty() {
        capabilities["prompts"] = json!({ "listChanged": false });
    }
    json!({
        "protocolVersion": version,
        "capabilities": capabilities,
        "serverInfo": { "name": info.name, "title": info.title, "version": info.version },
        "instructions": info.instructions
    })
}

fn tools_call(h: &dyn Handler, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "Parameter „name“ fehlt".to_string()))?;
    if !h.tools().iter().any(|t| t.name == name) {
        return Err((-32602, format!("Unbekanntes Werkzeug: {name}")));
    }
    let args = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(v) if v.is_object() => v.clone(),
        Some(_) => return Err((-32602, "„arguments“ muss ein Objekt sein".to_string())),
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| h.call(name, args)))
        .unwrap_or_else(|_| Err("Interner Fehler im Werkzeug".to_string()));
    Ok(match outcome {
        Ok(value) => {
            let text = match &value {
                Value::String(s) => s.clone(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            };
            let mut r = json!({ "content": [{ "type": "text", "text": text }], "isError": false });
            if value.is_object() {
                r["structuredContent"] = value;
            }
            r
        }
        Err(msg) => json!({ "content": [{ "type": "text", "text": msg }], "isError": true }),
    })
}

/// Verarbeitet eine einzelne JSON-RPC-Nachricht. `None` bei Benachrichtigungen
/// und Antworten des Clients (darauf wird nicht geantwortet).
pub fn handle(h: &dyn Handler, msg: &Value) -> Option<Value> {
    if !msg.is_object() || msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error(msg.get("id").cloned().unwrap_or(Value::Null), -32600, "Ungültige JSON-RPC-Nachricht"));
    }
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // Antwort des Clients auf eine Server-Anfrage – wir stellen keine.
        return None;
    };
    let id = id?; // Benachrichtigung (z. B. notifications/initialized)
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        "initialize" => Ok(initialize(h, &params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": h.tools().iter().map(tool_json).collect::<Vec<_>>() })),
        "tools/call" => tools_call(h, &params),
        "prompts/list" => Ok(json!({ "prompts": h.prompts().iter().map(prompt_json).collect::<Vec<_>>() })),
        "prompts/get" => prompts_get(h, &params),
        _ => Err((-32601, format!("Methode nicht unterstützt: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, m)) => error(id, code, &m),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub struct Echo;

    impl Handler for Echo {
        fn info(&self) -> ServerInfo {
            ServerInfo { name: "echo".into(), title: "Echo".into(), version: "1".into(), instructions: "Test".into() }
        }
        fn tools(&self) -> Vec<Tool> {
            vec![Tool {
                name: "echo",
                title: "Echo",
                description: "Gibt die Argumente zurück",
                input_schema: json!({ "type": "object" }),
                read_only: true,
            }]
        }
        fn call(&self, name: &str, args: Value) -> Result<Value, String> {
            match args.get("fail").and_then(Value::as_bool) {
                Some(true) => Err(format!("{name} absichtlich fehlgeschlagen")),
                _ if args.get("panic").is_some() => panic!("boom"),
                _ => Ok(json!({ "got": args })),
            }
        }
        fn prompts(&self) -> Vec<Prompt> {
            vec![Prompt { name: "hallo", title: "Hallo", description: "Grüßt", text: "Sag hallo".into() }]
        }
    }

    #[test]
    fn prompts_list_and_get() {
        let r = handle(&Echo, &req(20, "initialize", json!({}))).unwrap();
        assert!(r["result"]["capabilities"]["prompts"].is_object());
        let r = handle(&Echo, &req(21, "prompts/list", json!({}))).unwrap();
        assert_eq!(r["result"]["prompts"][0]["name"], "hallo");
        let r = handle(&Echo, &req(22, "prompts/get", json!({ "name": "hallo" }))).unwrap();
        assert_eq!(r["result"]["messages"][0]["role"], "user");
        assert_eq!(r["result"]["messages"][0]["content"]["text"], "Sag hallo");
        let r = handle(&Echo, &req(23, "prompts/get", json!({ "name": "fehlt" }))).unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }

    fn req(id: i64, method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    #[test]
    fn initialize_negotiates_version() {
        let r = handle(&Echo, &req(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {} }))).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "echo");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        let r = handle(&Echo, &req(2, "initialize", json!({ "protocolVersion": "1999-01-01" }))).unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_answer() {
        let n = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle(&Echo, &n).is_none());
    }

    #[test]
    fn tools_list_and_call() {
        let r = handle(&Echo, &req(3, "tools/list", json!({}))).unwrap();
        assert_eq!(r["result"]["tools"][0]["name"], "echo");
        assert_eq!(r["result"]["tools"][0]["annotations"]["readOnlyHint"], true);

        let r = handle(&Echo, &req(4, "tools/call", json!({ "name": "echo", "arguments": { "x": 1 } }))).unwrap();
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["structuredContent"]["got"]["x"], 1);

        let r = handle(&Echo, &req(5, "tools/call", json!({ "name": "echo", "arguments": { "fail": true } }))).unwrap();
        assert_eq!(r["result"]["isError"], true);

        let r = handle(&Echo, &req(6, "tools/call", json!({ "name": "echo", "arguments": { "panic": 1 } }))).unwrap();
        assert_eq!(r["result"]["isError"], true);

        let r = handle(&Echo, &req(7, "tools/call", json!({ "name": "fehlt" }))).unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn unknown_method_and_garbage() {
        let r = handle(&Echo, &req(8, "resources/read", json!({}))).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(&Echo, &json!([1, 2])).unwrap();
        assert_eq!(r["error"]["code"], -32600);
    }
}
