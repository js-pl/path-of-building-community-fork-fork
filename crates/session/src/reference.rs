//! [`Session`]: upstream's build session behind the engine API's JSON
//! requests, as a reference for the typed engine: upstream on the oracle's
//! LuaJIT (`oracle/session.lua`, in a process of its own) or, with the
//! `translation` feature, upstream's translated program in this process.

#[cfg(feature = "translation")]
use lua::Value;
#[cfg(feature = "translation")]
use lua::rt::{self, first};
use std::cell::RefCell;
#[cfg(feature = "translation")]
use std::rc::Rc;
#[cfg(feature = "translation")]
use upstream::host::Host;

/// A running build session.
pub struct Session {
    pub(crate) inner: Inner,
}

pub(crate) enum Inner {
    #[cfg(feature = "translation")]
    Translated { host: Rc<Host>, handle: Value },
    /// `oracle/session.lua`: the same session on upstream on LuaJIT, in a
    /// process of its own
    #[cfg(not(target_arch = "wasm32"))]
    Oracle(RefCell<oracle::Process>),
}

impl Session {
    /// The same session on the oracle (`oracle/session.lua` on upstream at
    /// `src`, on the LuaJIT `oracle/setup.sh` builds), with an empty build
    #[cfg(not(target_arch = "wasm32"))]
    pub fn oracle(src: &std::path::Path) -> Result<Session, String> {
        Ok(Session {
            inner: Inner::Oracle(RefCell::new(oracle::Process::start(src)?)),
        })
    }

    /// Answers one JSON request with a JSON response: `{"ok": ...}` or
    /// `{"error": "..."}`. `Err` only when the request could not be run.
    pub fn request(&self, json: &str) -> Result<String, String> {
        match &self.inner {
            #[cfg(feature = "translation")]
            Inner::Translated { handle, .. } => {
                let r = rt::call(handle, &[Value::Str(json.as_bytes().into())])
                    .map_err(|e| crate::translated::describe(&e))?;
                Ok(String::from_utf8_lossy(lua::to_string(&first(r)).as_bytes()).into_owned())
            }
            #[cfg(not(target_arch = "wasm32"))]
            Inner::Oracle(p) => p.borrow_mut().request(json),
        }
    }

    /// The translated program's host; `None` on the oracle
    #[cfg(feature = "translation")]
    pub fn host(&self) -> Option<&Rc<Host>> {
        match &self.inner {
            Inner::Translated { host, .. } => Some(host),
            #[cfg(not(target_arch = "wasm32"))]
            Inner::Oracle(_) => None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
