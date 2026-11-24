//! A build's history: every version of a build, kept in a git repository
//! of its own and written with the git the user has installed (git for
//! Windows under Windows). See `docs/history.md`.
//!
//! ```text
//! <root>/                       History: the builds' directory
//!   Righteous Fire/             BuildRepo: one ordinary git repository per build
//!     .git/
//!     .gitattributes            `* -text`: no tool changes a build's line endings
//!     build.xml                 the build, as `saveXml` writes it
//! ```
//!
//! The history is optional. Without git ([`Git::find`] gives `None`, or
//! `POB_GIT=off`), [`History::open`] gives no repository and a
//! [`Recorder`] made from that records nothing, so callers write the same
//! code either way.
//!
//! The history only grows: nothing here resets, rebases or force-pushes,
//! and restoring a version is a new commit on top of the others.

mod git;
mod recorder;

pub use git::{GIT_ENV, Git, GitOptions};
pub use recorder::{QUIET, Recorder};

use std::fmt;
use std::path::{Path, PathBuf};

/// The file a build is kept in, in its repository
pub const BUILD_FILE: &str = "build.xml";
/// The branch the history is on
pub const BRANCH: &str = "main";

const ATTRIBUTES: &str = "* -text\n";
const FALLBACK_NAME: &str = "Path of Building";
const FALLBACK_EMAIL: &str = "pob@localhost";

/// What went wrong
#[derive(Debug)]
pub enum Error {
    /// No git program could be run
    NoGit(String),
    /// A git command failed
    Git {
        command: String,
        status: String,
        stderr: String,
    },
    /// Reading or writing a file, or starting git, failed
    Io {
        what: String,
        source: std::io::Error,
    },
    /// An argument we refuse: a version that isn't one, a remote that
    /// reads as an option
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoGit(e) => write!(f, "git not found: {e}"),
            Error::Git {
                command,
                status,
                stderr,
            } => write!(f, "{command}: {status}: {stderr}"),
            Error::Io { what, source } => write!(f, "{what}: {source}"),
            Error::Invalid(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn io(what: impl fmt::Display, path: &Path) -> impl FnOnce(std::io::Error) -> Error {
    let what = format!("{what} {}", path.display());
    move |source| Error::Io { what, source }
}

/// A version of a build: the full hash of its commit
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version(String);

impl Version {
    /// The full hash
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 10 characters, as shown to people
    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(10)]
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who made a change: the `Source` trailer of its commit
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The user, in the UI
    User,
    /// An agent, by the name of its connection
    Agent(String),
    /// A restore of an earlier version
    Restore,
    /// A step undone before, applied again
    Redo,
    /// A source this version doesn't know, as written
    Other(String),
}

impl Source {
    fn parse(s: &str) -> Source {
        match s.split_once(' ') {
            _ if s == "user" => Source::User,
            _ if s == "restore" => Source::Restore,
            _ if s == "redo" => Source::Redo,
            Some(("agent", name)) => Source::Agent(name.to_string()),
            _ if s == "agent" => Source::Agent(String::new()),
            _ => Source::Other(s.to_string()),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::User => f.write_str("user"),
            Source::Agent(n) if n.is_empty() => f.write_str("agent"),
            Source::Agent(n) => write!(f, "agent {n}"),
            Source::Restore => f.write_str("restore"),
            Source::Redo => f.write_str("redo"),
            Source::Other(s) => f.write_str(s),
        }
    }
}

/// A commit's message: the undo step's label, then trailers for its
/// source and what it changed (`Full DPS: +3.1%`)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub label: String,
    pub source: Source,
    /// Lines under the label, e.g. the steps of coalesced edits
    pub body: Vec<String>,
    /// Trailers after `Source`, in order
    pub trailers: Vec<(String, String)>,
}

impl Message {
    pub fn new(source: Source, label: impl Into<String>) -> Message {
        Message {
            label: label.into(),
            source,
            body: Vec::new(),
            trailers: Vec::new(),
        }
    }

    /// The user's change, labelled
    pub fn user(label: impl Into<String>) -> Message {
        Message::new(Source::User, label)
    }

    /// An agent's step, labelled; `agent` names its connection
    pub fn agent(agent: impl Into<String>, label: impl Into<String>) -> Message {
        Message::new(Source::Agent(agent.into()), label)
    }

    /// Adds a trailer: `.trailer("Full DPS", "+3.1%")`
    pub fn trailer(mut self, key: impl Into<String>, value: impl Into<String>) -> Message {
        self.trailers.push((key.into(), value.into()));
        self
    }

    /// The text git gets. Labels and values are single lines without
    /// control characters, keys without `:`, so each reads back as given.
    fn text(&self) -> String {
        let mut s = one_line(&self.label);
        if s.is_empty() {
            s.push_str("Edit");
        }
        s.push_str("\n\n");
        let body: Vec<String> = self
            .body
            .iter()
            .map(|l| one_line(l))
            .filter(|l| !l.is_empty())
            .collect();
        for line in &body {
            s.push_str(line);
            s.push('\n');
        }
        if !body.is_empty() {
            s.push('\n');
        }
        s.push_str(&format!("Source: {}\n", one_line(&self.source.to_string())));
        for (k, v) in &self.trailers {
            let k: String = one_line(k)
                .chars()
                .map(|c| if c == ':' || c == ' ' { '-' } else { c })
                .collect();
            if !k.is_empty() {
                s.push_str(&format!("{k}: {}\n", one_line(v)));
            }
        }
        s
    }
}

/// `s` on one line, without control characters
fn one_line(s: &str) -> String {
    s.split(|c: char| c.is_control())
        .filter(|p| !p.trim().is_empty())
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A version in the history, as [`BuildRepo::log`] lists it
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub version: Version,
    /// When it was committed, in seconds since the Unix epoch
    pub time: i64,
    pub label: String,
    pub source: Source,
    /// The trailers but `Source`, in order
    pub trailers: Vec<(String, String)>,
}

impl Entry {
    /// The value of the trailer `key`, e.g. `Full DPS`
    pub fn trailer(&self, key: &str) -> Option<&str> {
        self.trailers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// The builds' directory: one git repository per build in it
#[derive(Clone, Debug)]
pub struct History {
    root: PathBuf,
    git: Option<Git>,
}

impl History {
    /// The history under `root`, with the git [`Git::find`] finds, if any
    pub fn new(root: impl Into<PathBuf>) -> History {
        History::with_git(root, Git::find())
    }

    /// The history under `root` with `git`; `None` keeps no history
    pub fn with_git(root: impl Into<PathBuf>, git: Option<Git>) -> History {
        History {
            root: root.into(),
            git,
        }
    }

    /// Whether there is a git to keep the history with
    pub fn is_enabled(&self) -> bool {
        self.git.is_some()
    }

    pub fn git(&self) -> Option<&Git> {
        self.git.as_ref()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The repository of the build `name`, made if there is none yet;
    /// `None` without git. The name becomes a directory name that every
    /// platform takes ([`dir_name`]).
    pub fn open(&self, name: &str) -> Result<Option<BuildRepo>> {
        match &self.git {
            None => Ok(None),
            Some(git) => BuildRepo::open(git.clone(), self.root.join(dir_name(name))).map(Some),
        }
    }

    /// The builds that have a repository here, by directory name, sorted
    pub fn builds(&self) -> Result<Vec<String>> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io("reading", &self.root)(e)),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io("reading", &self.root))?;
            if entry.path().join(".git").exists() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }
}

/// `name` as a directory name that Windows, macOS and Linux all take:
/// characters Windows forbids become `_`, trailing dots and spaces go, and
/// device names (`CON`, `COM1`) get a leading `_`
pub fn dir_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = s.trim_end_matches(['.', ' ']).trim_start();
    s = trimmed.to_string();
    if s.is_empty() || s == "." || s == ".." {
        return "build".into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if device {
        s.insert(0, '_');
    }
    s
}

/// One build's repository: `build.xml` and its history on `main`
#[derive(Clone, Debug)]
pub struct BuildRepo {
    git: Git,
    dir: PathBuf,
}

impl BuildRepo {
    /// The repository in `dir`, made (with its directory) if there is none
    pub fn open(git: Git, dir: impl Into<PathBuf>) -> Result<BuildRepo> {
        let dir = dir.into();
        let repo = BuildRepo { git, dir };
        if !repo.dir.join(".git").exists() {
            repo.init()?;
        }
        Ok(repo)
    }

    fn init(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(io("creating", &self.dir))?;
        self.git.run(&self.dir, &["init", "--quiet"], None)?;
        // `init -b` needs git 2.28; this works on any
        let head = format!("refs/heads/{BRANCH}");
        self.git
            .run(&self.dir, &["symbolic-ref", "HEAD", &head], None)?;
        // Commits need an identity: the user's if git has one, else ours
        for (key, fallback) in [("user.name", FALLBACK_NAME), ("user.email", FALLBACK_EMAIL)] {
            if self.git.run(&self.dir, &["config", key], None).is_err() {
                self.git
                    .run(&self.dir, &["config", "--local", key, fallback], None)?;
            }
        }
        write(&self.dir.join(".gitattributes"), ATTRIBUTES.as_bytes())?;
        // A crash between writing and renaming leaves `build.xml.tmp`
        let exclude = self.dir.join(".git").join("info").join("exclude");
        if let Some(parent) = exclude.parent() {
            std::fs::create_dir_all(parent).map_err(io("creating", parent))?;
        }
        write(&exclude, b"*.tmp\n")?;
        self.git
            .run(&self.dir, &["add", "--", ".gitattributes"], None)?;
        Ok(())
    }

    /// The repository's directory, to open in any git tool
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The build's file in the working tree
    pub fn file(&self) -> PathBuf {
        self.dir.join(BUILD_FILE)
    }

    /// Commits `xml` as the build's new version. `None` when it is the
    /// version already committed: nothing changed, nothing is committed.
    pub fn commit(&self, xml: &str, message: &Message) -> Result<Option<Version>> {
        write(&self.file(), xml.as_bytes())?;
        self.git.run(
            &self.dir,
            &["add", "--", BUILD_FILE, ".gitattributes"],
            None,
        )?;
        if self.git.test(&self.dir, &["diff", "--cached", "--quiet"])? {
            return Ok(None);
        }
        self.git.run(
            &self.dir,
            &[
                "commit",
                "--quiet",
                "--no-verify",
                "--cleanup=verbatim",
                "--file=-",
            ],
            Some(message.text().as_bytes()),
        )?;
        self.head()
    }

    /// The latest version, `None` before the first commit
    pub fn head(&self) -> Result<Option<Version>> {
        let out = self.git.run(
            &self.dir,
            &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
            None,
        );
        match out {
            Ok(out) => Ok(Some(Version(text(out)?.trim().to_string()))),
            Err(Error::Git { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The version `rev` names: a hash or its prefix, `HEAD~2`, a tag
    pub fn resolve(&self, rev: &str) -> Result<Version> {
        if rev.starts_with('-') || rev.trim().is_empty() {
            return Err(Error::Invalid(format!("not a version: {rev:?}")));
        }
        let spec = format!("{rev}^{{commit}}");
        let out = self
            .git
            .run(
                &self.dir,
                &["rev-parse", "--verify", "--quiet", &spec],
                None,
            )
            .map_err(|_| Error::Invalid(format!("no version {rev:?} in {}", self.dir.display())))?;
        Ok(Version(text(out)?.trim().to_string()))
    }

    /// The build's XML at `version`
    pub fn read(&self, version: &Version) -> Result<String> {
        let spec = format!("{}:{BUILD_FILE}", version.as_str());
        text(
            self.git
                .run(&self.dir, &["cat-file", "blob", &spec], None)?,
        )
    }

    /// The last `n` versions, newest first
    pub fn log(&self, n: usize) -> Result<Vec<Entry>> {
        if n == 0 || self.head()?.is_none() {
            return Ok(Vec::new());
        }
        let count = format!("--max-count={n}");
        let out = text(self.git.run(
            &self.dir,
            &[
                "log",
                &count,
                "--first-parent",
                "--format=%H%x1f%ct%x1f%s%x1f%(trailers:only,unfold)%x1e",
                BRANCH,
                "--",
            ],
            None,
        )?)?;
        Ok(out
            .split('\u{1e}')
            .filter_map(|record| {
                let mut fields = record.trim_start_matches('\n').split('\u{1f}');
                let version = Version(fields.next()?.trim().to_string());
                if version.0.is_empty() {
                    return None;
                }
                let time = fields.next()?.trim().parse().unwrap_or(0);
                let label = fields.next()?.to_string();
                let mut source = Source::Other(String::new());
                let mut trailers = Vec::new();
                for line in fields.next().unwrap_or("").lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        let (k, v) = (k.trim(), v.trim());
                        if k.eq_ignore_ascii_case("Source") {
                            source = Source::parse(v);
                        } else {
                            trailers.push((k.to_string(), v.to_string()));
                        }
                    }
                }
                Some(Entry {
                    version,
                    time,
                    label,
                    source,
                    trailers,
                })
            })
            .collect())
    }

    /// Makes `version` the build's latest version again, as a new commit,
    /// so the restore can be undone too. `None` when it already is.
    pub fn restore(&self, version: &Version) -> Result<Option<Version>> {
        let xml = self.read(version)?;
        let label = self.label(version)?;
        let message = Message::new(
            Source::Restore,
            format!("Restore {}: {label}", version.short()),
        )
        .trailer("Restores", version.as_str());
        self.commit(&xml, &message)
    }

    fn label(&self, version: &Version) -> Result<String> {
        let out = self.git.run(
            &self.dir,
            &["log", "-1", "--format=%s", version.as_str(), "--"],
            None,
        )?;
        Ok(text(out)?.trim().to_string())
    }

    /// Pushes the history to `remote` (a URL, a path, or a remote's name)
    /// as a backup. Never forced: a remote with other history refuses it.
    pub fn push(&self, remote: &str) -> Result<()> {
        if remote.starts_with('-') || remote.trim().is_empty() {
            return Err(Error::Invalid(format!("not a remote: {remote:?}")));
        }
        let refspec = format!("refs/heads/{BRANCH}:refs/heads/{BRANCH}");
        self.git.run(
            &self.dir,
            &["push", "--quiet", "--no-verify", remote, &refspec],
            None,
        )?;
        Ok(())
    }
}

/// Writes `bytes` to `path` through a file beside it, so a crash leaves
/// the old file or the new one, never half of one
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(io("writing", &tmp))?;
    std::fs::rename(&tmp, path).map_err(io("replacing", path))
}

fn text(out: Vec<u8>) -> Result<String> {
    String::from_utf8(out).map_err(|_| Error::Invalid("git gave text that isn't UTF-8".into()))
}
