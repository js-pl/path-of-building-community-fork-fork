//! [`Session`]: upstream's build session behind the engine API's JSON
//! requests, as a reference for the typed engine: upstream on the oracle's
//! LuaJIT (`oracle/session.lua`, in a process of its own).

use std::cell::RefCell;

/// A running build session.
pub struct Session {
    process: RefCell<oracle::Process>,
}

impl Session {
    /// The session on the oracle (`oracle/session.lua` on upstream at
    /// `src`, on the LuaJIT `oracle/setup.sh` builds), with an empty build
    pub fn oracle(src: &std::path::Path) -> Result<Session, String> {
        Ok(Session {
            process: RefCell::new(oracle::Process::start(src)?),
        })
    }

    /// Answers one JSON request with a JSON response: `{"ok": ...}` or
    /// `{"error": "..."}`. `Err` only when the request could not be run.
    pub fn request(&self, json: &str) -> Result<String, String> {
        self.process.borrow_mut().request(json)
    }
}

mod oracle {
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    /// `oracle/session.lua` running in LuaJIT, one JSON line each way
    pub struct Process {
        child: Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    }

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    impl Process {
        pub fn start(src: &Path) -> Result<Process, String> {
            let root = root();
            let prefix = std::env::var_os("ORACLE_PREFIX")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join(".oracle"));
            let luajit = prefix.join("bin/luajit");
            if !luajit.exists() {
                return Err(format!(
                    "{} not found, run oracle/setup.sh first",
                    luajit.display()
                ));
            }
            // As oracle/run.py: upstream's runtime/lua first, then the rocks
            let rocks = prefix.join("rocks");
            let rocks = rocks.display();
            let mut child = Command::new(&luajit)
                .arg(root.join("oracle/session.lua"))
                .current_dir(src)
                .env(
                    "LUA_PATH",
                    format!(
                        "../runtime/lua/?.lua;../runtime/lua/?/init.lua;\
                         {rocks}/share/lua/5.1/?.lua;{rocks}/share/lua/5.1/?/init.lua;;"
                    ),
                )
                .env("LUA_CPATH", format!("{rocks}/lib/lua/5.1/?.so;;"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("{}: {e}", luajit.display()))?;
            let stdin = child.stdin.take().unwrap();
            let stdout = BufReader::new(child.stdout.take().unwrap());
            Ok(Process {
                child,
                stdin,
                stdout,
            })
        }

        pub fn request(&mut self, json: &str) -> Result<String, String> {
            let line = json.replace(['\n', '\r'], " ");
            writeln!(self.stdin, "{line}")
                .and_then(|_| self.stdin.flush())
                .map_err(|e| format!("oracle session: {e}"))?;
            let mut out = String::new();
            match self.stdout.read_line(&mut out) {
                Ok(n) if n > 0 => Ok(out.trim_end_matches('\n').to_string()),
                Ok(_) => Err("oracle session ended".into()),
                Err(e) => Err(format!("oracle session: {e}")),
            }
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
