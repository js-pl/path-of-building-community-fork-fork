//! The installed git: found once, run with settings that keep the user's
//! configuration from changing a build's bytes or the history's shape.

use crate::Error;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The environment variable that picks the git program: a path to it, or
/// `off` to keep no history
pub const GIT_ENV: &str = "POB_GIT";

/// Variables a parent git process (a hook, `git rebase --exec`) can leave
/// behind; each would point our commands at another repository
const INHERITED: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// An installed git program, e.g. git for Windows' `git.exe`
#[derive(Clone, Debug)]
pub struct Git {
    program: PathBuf,
    version: String,
}

impl Git {
    /// The git to use: `POB_GIT` if set (a path, or `off` for none), else
    /// `git` on the `PATH`, else, on Windows, git for Windows where its
    /// installer puts it. `None` when there is none: the history is
    /// optional and the caller keeps working without it.
    pub fn find() -> Option<Git> {
        match std::env::var_os(GIT_ENV) {
            Some(v) if is_off(&v) => None,
            Some(v) => Git::at(v).ok(),
            None => candidates().into_iter().find_map(|p| Git::at(p).ok()),
        }
    }

    /// The git program at `program` (a path, or a name looked up on the
    /// `PATH`), if it runs
    pub fn at(program: impl Into<PathBuf>) -> Result<Git, Error> {
        let program = program.into();
        let out = command(&program)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Error::NoGit(format!("{}: {e}", program.display())))?;
        if !out.status.success() {
            return Err(Error::NoGit(format!(
                "{} --version: {}",
                program.display(),
                out.status
            )));
        }
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(Git { program, version })
    }

    /// The program run, as given or found
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// What `git --version` says, e.g. `git version 2.47.1.windows.1`
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Runs git in `dir` with `args`, `input` on its stdin; its stdout on
    /// success
    pub(crate) fn run<S: AsRef<OsStr>>(
        &self,
        dir: &Path,
        args: &[S],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>, Error> {
        let out = self.output(dir, args, input)?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(failed(args, &out))
        }
    }

    /// Runs git in `dir` with `args` for its exit code: `Ok(true)` on 0,
    /// `Ok(false)` on 1, an error otherwise (`git diff --quiet`)
    pub(crate) fn test<S: AsRef<OsStr>>(&self, dir: &Path, args: &[S]) -> Result<bool, Error> {
        let out = self.output(dir, args, None)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failed(args, &out)),
        }
    }

    fn output<S: AsRef<OsStr>>(
        &self,
        dir: &Path,
        args: &[S],
        input: Option<&[u8]>,
    ) -> Result<Output, Error> {
        let mut cmd = command(&self.program);
        cmd.arg("-C").arg(dir);
        // The user's settings must not change a build's bytes (line
        // endings), sign or run hooks on our commits, or start daemons
        for setting in [
            "core.autocrlf=false",
            "core.safecrlf=false",
            "core.fsmonitor=false",
            "core.quotepath=false",
            "commit.gpgsign=false",
            "log.showSignature=false",
            "i18n.logOutputEncoding=UTF-8",
        ] {
            cmd.arg("-c").arg(setting);
        }
        cmd.arg("-c")
            .arg(format!("core.hooksPath={}", no_hooks(dir).display()));
        cmd.args(args);
        for v in INHERITED {
            cmd.env_remove(v);
        }
        // Never wait for a password on a terminal nobody sees
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| Error::Io {
            what: format!("running {}", self.program.display()),
            source: e,
        })?;
        if let Some(input) = input {
            use std::io::Write;
            let mut stdin = child.stdin.take().expect("stdin is piped");
            stdin.write_all(input).map_err(|e| Error::Io {
                what: "writing to git".into(),
                source: e,
            })?;
        }
        child.wait_with_output().map_err(|e| Error::Io {
            what: format!("running {}", self.program.display()),
            source: e,
        })
    }
}

fn is_off(v: &OsStr) -> bool {
    matches!(
        v.to_string_lossy().trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "none" | ""
    )
}

/// Where to look for git, in order
fn candidates() -> Vec<PathBuf> {
    let mut list = vec![PathBuf::from("git")];
    if cfg!(windows) {
        // git for Windows' installer, for all users or for one, when its
        // `cmd` directory isn't on the PATH
        for (var, sub) in [
            ("ProgramFiles", r"Git\cmd\git.exe"),
            ("ProgramW6432", r"Git\cmd\git.exe"),
            ("ProgramFiles(x86)", r"Git\cmd\git.exe"),
            ("LOCALAPPDATA", r"Programs\Git\cmd\git.exe"),
        ] {
            if let Some(base) = std::env::var_os(var) {
                list.push(PathBuf::from(base).join(sub));
            }
        }
    }
    list
}

/// A hooks directory that never exists, so no hook runs on our commands
fn no_hooks(dir: &Path) -> PathBuf {
    dir.join(".git").join("pob-no-hooks")
}

fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        // No console window flashing up for each git call from a GUI host
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn failed<S: AsRef<OsStr>>(args: &[S], out: &Output) -> Error {
    let args = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    Error::Git {
        command: format!("git {args}"),
        status: out.status.to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    }
}
