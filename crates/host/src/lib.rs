//! `pob host`: owns the builds, on disk with their history, and answers
//! the agent API for the user's own processes on the control socket. See
//! `docs/agent.md`, "What runs where". It serves the agent API as MCP
//! ([`mcp`]), and serves the web app with its WebSocket ([`web`], [`ui`])
//! unless `--headless`.

pub mod connections;
pub mod control;
pub mod http;
pub mod local;
pub mod mcp;
pub mod store;
pub mod tokens;
pub mod ui;
pub mod web;
pub mod ws;

pub use store::FsStore;

use std::path::PathBuf;

/// The variable that names another builds directory
pub const BUILDS_ENV: &str = "POB_BUILDS";
/// The variable that names another control socket
pub const SOCKET_ENV: &str = "POB_SOCKET";

/// The app's data directory: [`local::data_base`] with
/// `path-of-building-fork-fork/`
pub fn data_dir() -> Option<PathBuf> {
    local::data_base().map(|b| b.join("path-of-building-fork-fork"))
}

/// The builds directory: `POB_BUILDS`, else `builds/` in [`data_dir`]
pub fn default_builds() -> Option<PathBuf> {
    std::env::var_os(BUILDS_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| data_dir().map(|d| d.join("builds")))
}

/// The control socket: `POB_SOCKET`, else [`local::default_socket`]
/// (`control.sock` in [`data_dir`], a named pipe on Windows)
pub fn default_socket() -> Option<PathBuf> {
    std::env::var_os(SOCKET_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| data_dir().map(|d| local::default_socket(&d)))
}
