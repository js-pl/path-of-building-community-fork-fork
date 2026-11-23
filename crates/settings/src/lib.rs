//! The app's settings: `settings.json` in the user's config directory, ours
//! rather than upstream's `Settings.xml`, so both apps can be installed side
//! by side. See `docs/settings.md`.
//!
//! ```json
//! {
//!   "history": {
//!     "enabled": true,
//!     "git": "",
//!     "gitConfig": ""
//!   }
//! }
//! ```
//!
//! A missing file reads as the defaults. Keys this version doesn't know are
//! kept as they are when the file is saved again, so an older and a newer
//! version can share one file.

use history::{Git, GitOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::path::{Path, PathBuf};

/// The environment variable that picks another settings file
pub const SETTINGS_ENV: &str = "POB_SETTINGS";
/// The directory the settings file is in, in the user's config directory
pub const APP_DIR: &str = "path-of-building-fork-fork";
/// The settings file's name
pub const FILE: &str = "settings.json";

/// The settings `pob settings` and the Options dialog change: key, what it is
pub const KEYS: &[(&str, &str)] = &[
    (
        "history.enabled",
        "keep each build's versions in a git repository (true or false)",
    ),
    (
        "history.git",
        "the git program; empty to look for git on the PATH (and git for Windows)",
    ),
    (
        "history.gitConfig",
        "a gitconfig used instead of your global ~/.gitconfig; empty for yours",
    ),
];

#[derive(Debug)]
pub enum Error {
    Io {
        what: String,
        source: std::io::Error,
    },
    /// The file isn't valid JSON or has a value of the wrong type; it is
    /// left as it is
    Parse { path: PathBuf, message: String },
    /// No such setting
    UnknownKey(String),
    /// A value the setting doesn't take
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { what, source } => write!(f, "{what}: {source}"),
            Error::Parse { path, message } => write!(f, "{}: {message}", path.display()),
            Error::UnknownKey(k) => {
                let keys: Vec<&str> = KEYS.iter().map(|(k, _)| *k).collect();
                write!(f, "no setting {k:?}; the settings are {}", keys.join(", "))
            }
            Error::Invalid(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The settings file's contents
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub history: HistorySettings,
    /// Sections this version doesn't know, kept
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// The build history's settings (`crates/history`)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HistorySettings {
    /// Whether to keep a history at all
    pub enabled: bool,
    /// The git program; empty to search for one
    pub git: String,
    /// A gitconfig used instead of the user's global one; empty for theirs
    pub git_config: String,
    /// Keys this version doesn't know, kept
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Default for HistorySettings {
    fn default() -> HistorySettings {
        HistorySettings {
            enabled: true,
            git: String::new(),
            git_config: String::new(),
            other: Map::new(),
        }
    }
}

impl HistorySettings {
    /// What `crates/history` finds git with
    pub fn git_options(&self) -> GitOptions {
        let path = |s: &str| {
            let s = s.trim();
            (!s.is_empty()).then(|| PathBuf::from(s))
        };
        GitOptions {
            enabled: self.enabled,
            program: path(&self.git),
            config: path(&self.git_config),
        }
    }

    /// The git these settings give (`POB_GIT` still overrides them):
    /// `Ok(None)` when the history is off, an error saying why when it is
    /// on and git can't be used
    pub fn git(&self) -> std::result::Result<Option<Git>, history::Error> {
        Git::from_options(&self.git_options())
    }
}

/// Where the settings file is: `POB_SETTINGS` if set, else `settings.json`
/// in [`APP_DIR`] in the user's config directory (`%APPDATA%` on Windows,
/// `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` or
/// `~/.config` elsewhere). `None` when there is no such directory.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(SETTINGS_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    config_dir().map(|d| d.join(APP_DIR).join(FILE))
}

fn config_dir() -> Option<PathBuf> {
    let var = |v: &str| {
        std::env::var_os(v)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    }
}

impl Settings {
    /// The settings in `path`; the defaults if there is no file
    pub fn load(path: &Path) -> Result<Settings> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
            Err(e) => {
                return Err(Error::Io {
                    what: format!("reading {}", path.display()),
                    source: e,
                });
            }
        };
        if text.trim().is_empty() {
            return Ok(Settings::default());
        }
        serde_json::from_str(&text).map_err(|e| Error::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    /// Writes the settings to `path`, with its directory, through a file
    /// beside it so a crash never leaves half a file
    pub fn save(&self, path: &Path) -> Result<()> {
        let io = |what: &str, p: &Path| {
            let what = format!("{what} {}", p.display());
            move |source| Error::Io { what, source }
        };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(io("creating", dir))?;
        }
        let mut text = serde_json::to_string_pretty(self).expect("settings serialize");
        text.push('\n');
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, text).map_err(io("writing", &tmp))?;
        std::fs::rename(&tmp, path).map_err(io("replacing", path))
    }

    /// The value of the setting `key` (one of [`KEYS`]), as text
    pub fn get(&self, key: &str) -> Result<String> {
        let h = &self.history;
        Ok(match key {
            "history.enabled" => h.enabled.to_string(),
            "history.git" => h.git.clone(),
            "history.gitConfig" => h.git_config.clone(),
            _ => return Err(Error::UnknownKey(key.to_string())),
        })
    }

    /// Sets the setting `key` (one of [`KEYS`]) from text: `true`/`false`
    /// (or `on`/`off`, `yes`/`no`, `1`/`0`) for a switch, a path or empty
    /// for a path
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let h = &mut self.history;
        match key {
            "history.enabled" => h.enabled = switch(key, value)?,
            "history.git" => h.git = value.trim().to_string(),
            "history.gitConfig" => h.git_config = value.trim().to_string(),
            _ => return Err(Error::UnknownKey(key.to_string())),
        }
        Ok(())
    }
}

fn switch(key: &str, value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(Error::Invalid(format!(
            "{key} is true or false, not {value:?}"
        ))),
    }
}
