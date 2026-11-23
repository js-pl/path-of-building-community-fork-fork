//! `pob agents install`: adds `pob mcp` to the harnesses found on this
//! machine, each through its own mechanism, and says what it did. Run
//! twice, it changes nothing the second time.
//!
//! - **Claude Code**: `claude mcp add --scope user pob -- <pob> mcp ...`
//! - **Codex**: `codex mcp add pob -- <pob> mcp ...`, or, without the
//!   `codex` command, a `[mcp_servers.pob]` table in `~/.codex/config.toml`

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The server's name in each harness
pub const NAME: &str = "pob";

/// What happened for one harness
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub harness: &'static str,
    pub outcome: Outcome,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not on this machine
    NotFound,
    /// There already, the same
    Already,
    /// Added (or replaced an older entry)
    Added(String),
    /// Failed, with why
    Failed(String),
}

/// Where to look: `PATH` and the home directory, so tests can pick theirs
pub struct Env<'a> {
    pub path: &'a OsStr,
    pub home: Option<&'a Path>,
}

/// A command on `PATH`
fn find(cmd: &str, path: &OsStr) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|e| format!("{cmd}.{e}"))
            .collect()
    } else {
        vec![cmd.to_string()]
    };
    std::env::split_paths(path)
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

/// The command line `pob mcp` is run with
pub fn command_line(exe: &Path, args: &[String]) -> Vec<String> {
    let mut v = vec![exe.display().to_string(), "mcp".to_string()];
    v.extend(args.iter().cloned());
    v
}

/// A command line as a shell would take it, for the line to add by hand
pub fn shell_words(words: &[String]) -> String {
    words
        .iter()
        .map(|w| {
            if !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c))
            {
                w.clone()
            } else {
                format!("'{}'", w.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn run(cmd: &Path, args: &[&str], env: &Env) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .env("PATH", env.path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", cmd.display()))?;
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().lines().last().unwrap_or("failed").to_string())
    }
}

/// Adds the server through a harness's `<cli> mcp get|remove|add`:
/// unchanged when `get` shows the same command line, replaced when it
/// shows another
fn through_cli(cli: &Path, scope: &[&str], line: &[String], env: &Env) -> Outcome {
    if let Ok(shown) = run(cli, &["mcp", "get", NAME], env) {
        if line.iter().all(|w| shown.contains(w.as_str())) {
            return Outcome::Already;
        }
        let mut rm = vec!["mcp", "remove"];
        rm.extend_from_slice(scope);
        rm.push(NAME);
        if let Err(e) = run(cli, &rm, env) {
            return Outcome::Failed(format!("removing the older entry: {e}"));
        }
    }
    let mut add = vec!["mcp", "add"];
    add.extend_from_slice(scope);
    add.extend([NAME, "--"]);
    add.extend(line.iter().map(String::as_str));
    match run(cli, &add, env) {
        Ok(_) => Outcome::Added(format!(
            "{} {}",
            cli.display(),
            shell_words(&add.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        )),
        Err(e) => Outcome::Failed(e),
    }
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Codex without its command: the table in its config file
fn codex_config(home: &Path, line: &[String]) -> Outcome {
    let dir = home.join(".codex");
    if !dir.is_dir() {
        return Outcome::NotFound;
    }
    let file = dir.join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let header = format!("[mcp_servers.{NAME}]");
    if text.lines().any(|l| l.trim() == header) {
        return if text.contains(&toml_string(&line[0])) {
            Outcome::Already
        } else {
            Outcome::Failed(format!(
                "{} has another {header}; change it by hand",
                file.display()
            ))
        };
    }
    let args = line[1..]
        .iter()
        .map(|a| toml_string(a))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = text.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!(
        "{header}\ncommand = {}\nargs = [{args}]\n",
        toml_string(&line[0])
    ));
    match std::fs::write(&file, out) {
        Ok(()) => Outcome::Added(format!("{header} in {}", file.display())),
        Err(e) => Outcome::Failed(format!("{}: {e}", file.display())),
    }
}

/// Adds `pob mcp` (as `line`) to every harness found
pub fn install(line: &[String], env: &Env) -> Vec<Report> {
    let claude = match find("claude", env.path) {
        Some(cli) => through_cli(&cli, &["--scope", "user"], line, env),
        None => Outcome::NotFound,
    };
    let codex = match (find("codex", env.path), env.home) {
        (Some(cli), _) => through_cli(&cli, &[], line, env),
        (None, Some(home)) => codex_config(home, line),
        (None, None) => Outcome::NotFound,
    };
    vec![
        Report {
            harness: "Claude Code",
            outcome: claude,
        },
        Report {
            harness: "Codex",
            outcome: codex,
        },
    ]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fake harness CLI keeping its servers in a file: `mcp get|add|remove`
    fn fake(dir: &Path, name: &str) {
        let state = dir.join(format!("{name}.servers"));
        let script = format!(
            r#"#!/bin/sh
state='{state}'
[ "$1" = mcp ] || exit 2
shift
case "$1" in
  get) grep "^$2 " "$state" 2>/dev/null || {{ echo "no server $2" >&2; exit 1; }} ;;
  remove) shift; while [ "$1" != pob ]; do shift; done
    grep -v "^pob " "$state" > "$state.tmp"; mv "$state.tmp" "$state" ;;
  add) shift; while [ "$1" != pob ]; do shift; done; shift; shift
    echo "pob $*" >> "$state" ;;
esac
"#,
            state = state.display()
        );
        let p = dir.join(name);
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pob-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn through_the_harnesses_commands() {
        let dir = temp("cli");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        fake(&bin, "claude");
        fake(&bin, "codex");
        let path = std::env::join_paths([bin.as_path(), Path::new("/bin"), Path::new("/usr/bin")])
            .unwrap();
        let env = Env {
            path: &path,
            home: Some(&dir),
        };
        let line = command_line(Path::new("/opt/pob/pob"), &["--build".into(), "RF".into()]);
        let r = install(&line, &env);
        assert!(
            r.iter().all(|r| matches!(r.outcome, Outcome::Added(_))),
            "{r:?}"
        );
        let state = std::fs::read_to_string(bin.join("claude.servers")).unwrap();
        assert_eq!(state, "pob /opt/pob/pob mcp --build RF\n");
        // Again: nothing changes
        let r = install(&line, &env);
        assert!(r.iter().all(|r| r.outcome == Outcome::Already), "{r:?}");
        assert_eq!(
            std::fs::read_to_string(bin.join("claude.servers")).unwrap(),
            state
        );
        // pob moved: the entry is replaced, not doubled
        let moved = command_line(Path::new("/usr/local/bin/pob"), &[]);
        let r = install(&moved, &env);
        assert!(matches!(r[0].outcome, Outcome::Added(_)), "{r:?}");
        assert_eq!(
            std::fs::read_to_string(bin.join("codex.servers")).unwrap(),
            "pob /usr/local/bin/pob mcp\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_config_file() {
        let dir = temp("toml");
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        std::fs::write(dir.join(".codex/config.toml"), "model = \"o3\"").unwrap();
        let path = std::ffi::OsString::from("/nonexistent");
        let env = Env {
            path: &path,
            home: Some(&dir),
        };
        let line = command_line(Path::new("/opt/pob/pob"), &[]);
        let r = install(&line, &env);
        assert_eq!(r[0].outcome, Outcome::NotFound);
        assert!(matches!(r[1].outcome, Outcome::Added(_)), "{r:?}");
        let text = std::fs::read_to_string(dir.join(".codex/config.toml")).unwrap();
        assert_eq!(
            text,
            "model = \"o3\"\n\n[mcp_servers.pob]\ncommand = \"/opt/pob/pob\"\nargs = [\"mcp\"]\n"
        );
        assert_eq!(install(&line, &env)[1].outcome, Outcome::Already);
        assert_eq!(
            std::fs::read_to_string(dir.join(".codex/config.toml")).unwrap(),
            text
        );
        let other = command_line(Path::new("/elsewhere/pob"), &[]);
        assert!(matches!(
            install(&other, &env)[1].outcome,
            Outcome::Failed(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words() {
        assert_eq!(
            shell_words(&[
                "/a b/pob".into(),
                "mcp".into(),
                "--build".into(),
                "it's".into()
            ]),
            "'/a b/pob' mcp --build 'it'\\''s'"
        );
    }
}
