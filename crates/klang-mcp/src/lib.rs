//! Model Context Protocol (MCP) – lean and without an async runtime.
//!
//! * [`protocol`] – JSON-RPC handling: `initialize`, `ping`, `tools/list`, `tools/call`
//! * [`http`]     – "Streamable HTTP" transport on `127.0.0.1` (JSON responses only, no SSE)
//! * [`client`]   – minimal HTTP client for the stdio bridge
//! * [`bridge`]   – stdio bridge to the running app (`klangwelt --mcp`)
//!
//! The application implements [`Handler`]; this crate takes care of everything else.

pub mod bridge;
pub mod client;
pub mod http;
pub mod protocol;

pub use http::{caller_gone, serve, Presence, ServerHandle};
pub use protocol::{handle, Handler, Prompt, ServerInfo, Tool, PROTOCOL_VERSIONS};

/// Default port of the Klangwelt app.
pub const DEFAULT_PORT: u16 = 7419;
