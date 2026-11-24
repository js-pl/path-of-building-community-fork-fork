//! `pob host`: owns the builds, on disk with their history, and answers
//! the agent API for the user's own processes on the control socket. See
//! `docs/agent.md`, "What runs where". It serves the agent API as MCP
//! ([`mcp`]); the web app's WebSocket (step 6) comes later, so
//! `--headless` is what there is.

pub mod connections;
pub mod control;
pub mod http;
pub mod mcp;
pub mod store;
pub mod tokens;

pub use store::FsStore;

use std::path::PathBuf;

/// The variable that names another builds directory
pub const BUILDS_ENV: &str = "POB_BUILDS";
/// The variable that names another control socket
pub const SOCKET_ENV: &str = "POB_SOCKET";

/// The app's data directory: `%LOCALAPPDATA%` on Windows,
/// `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or
/// `~/.local/share` elsewhere, each with `path-of-building-fork-fork/`
pub fn data_dir() -> Option<PathBuf> {
    let var = |v: &str| {
        std::env::var_os(v)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
    };
    base.map(|b| b.join("path-of-building-fork-fork"))
}

/// The builds directory: `POB_BUILDS`, else `builds/` in [`data_dir`]
pub fn default_builds() -> Option<PathBuf> {
    std::env::var_os(BUILDS_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| data_dir().map(|d| d.join("builds")))
}

/// The control socket: `POB_SOCKET`, else `control.sock` in [`data_dir`]
/// (on Windows a named pipe, `\\.\pipe\path-of-building-fork-fork-<user>-control`)
pub fn default_socket() -> Option<PathBuf> {
    let var = std::env::var_os(SOCKET_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    #[cfg(windows)]
    return var.or_else(|| Some(PathBuf::from(control::pipe::default_name())));
    #[cfg(not(windows))]
    var.or_else(|| data_dir().map(|d| d.join("control.sock")))
}
