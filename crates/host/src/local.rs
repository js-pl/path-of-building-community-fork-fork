//! The host's one platform layer: how the user's own processes reach the
//! host on this machine, and the few other things that differ per OS. The
//! rest of the host, `pob ctl` and `pob mcp` use only what is here, the
//! same on Linux, macOS and Windows.
//!
//! The local transport is the control socket: a Unix socket (mode 0600 in
//! a directory made 0700, [`unix`]) or, on Windows, a named pipe whose DACL
//! lets only the current user in ([`pipe`]). Both give the same API:
//! [`listen`] and [`Listener::accept`], [`serve`], [`answers`], [`admin`]
//! and [`Client`], over the lines of [`crate::control`].

#[cfg(windows)]
pub mod pipe;
#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
use pipe as imp;
#[cfg(unix)]
use unix as imp;

pub use imp::{Client, Listener, Stream, admin, answers, listen, serve};

use std::path::{Path, PathBuf};
use std::process::Command;

/// The base of the app's data directory: `%LOCALAPPDATA%` on Windows,
/// `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or
/// `~/.local/share` elsewhere
pub fn data_base() -> Option<PathBuf> {
    let var = |v: &str| {
        std::env::var_os(v)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
    }
}

/// The default control socket for a data directory: `control.sock` in it,
/// or on Windows the user's pipe,
/// `\\.\pipe\path-of-building-fork-fork-<user>-control`
pub fn default_socket(data_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = data_dir;
        PathBuf::from(pipe::default_name())
    }
    #[cfg(not(windows))]
    data_dir.join("control.sock")
}

/// A control socket of its own for `dir` (tests, a second host): a file
/// `name` in it, or on Windows a pipe named after both
pub fn endpoint_in(dir: &Path, name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let tag: String = dir
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        PathBuf::from(format!("{}pob-{tag}-{name}", pipe::PREFIX))
    }
    #[cfg(not(windows))]
    dir.join(name)
}

/// Where the files that belong next to the control socket go (a started
/// host's log): the socket's directory, or the data directory where the
/// socket is a pipe
pub fn beside(socket: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        crate::data_dir()
    } else {
        Some(socket.parent().unwrap_or(Path::new(".")).to_path_buf())
    }
}

/// Writes a file only the user may read: created 0600 on Unix; on Windows
/// it inherits the data directory's ACL, which is the user's
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Detaches a process to be started from this one, so it outlives it: a
/// process group of its own (and, on Windows, no console)
pub fn detach(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(
        cmd,
        0x0000_0200 | 0x0800_0000, // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
    );
    cmd
}

/// The command that opens a link in the user's browser, the link to be
/// added as its last argument
pub fn open_command() -> Command {
    if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/c", "start", ""]);
        c
    } else {
        Command::new("xdg-open")
    }
}
