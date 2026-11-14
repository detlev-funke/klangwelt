//! Model Context Protocol (MCP) – schlank und ohne Async-Laufzeit.
//!
//! * [`protocol`] – JSON-RPC-Verarbeitung: `initialize`, `ping`, `tools/list`, `tools/call`
//! * [`http`]     – „Streamable HTTP“-Transport auf `127.0.0.1` (nur JSON-Antworten, kein SSE)
//! * [`client`]   – Minimal-HTTP-Client für die stdio-Brücke
//! * [`bridge`]   – stdio-Brücke zur laufenden App (`klangwelt --mcp`)
//!
//! Die Anwendung implementiert [`Handler`]; alles Weitere erledigt diese Crate.

pub mod bridge;
pub mod client;
pub mod http;
pub mod protocol;

pub use http::{caller_gone, serve, Presence, ServerHandle};
pub use protocol::{handle, Handler, Prompt, ServerInfo, Tool, PROTOCOL_VERSIONS};

/// Standard-Port der Klangwelt-App.
pub const DEFAULT_PORT: u16 = 7419;
