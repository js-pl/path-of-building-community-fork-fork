//! The translated session: upstream's program run headless, answering
//! requests with `lua/session.lua`.

use crate::Files;
use lua::rt::{self, first, func};
use lua::{LResult, LuaError, Value};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use upstream::host::{DataSource, Host};

#[allow(
    dead_code,
    unused_mut,
    unused_variables,
    unused_assignments,
    unreachable_code,
    unused_imports,
    non_snake_case,
    clippy::all
)]
mod bridge {
    include!(concat!(env!("OUT_DIR"), "/session_lua.rs"));
}

impl DataSource for Files {
    fn load(&self, env: &Rc<lua::env::Env>, name: &str, args: &[Value]) -> LResult<Vec<Value>> {
        let rel = format!("{name}.lua");
        if let Some(chunk) = upstream::up::chunk(&format!("src/{rel}")) {
            return chunk(env, args);
        }
        let src = self.get(&rel).ok_or_else(|| {
            LuaError::msg(format!(
                "LoadModule() error loading '{rel}': missing data file"
            ))
        })?;
        upstream::imported::load_data(env, rel, Rc::new((*src).clone()), args)
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.get(path).map(|b| (*b).clone())
    }
}

/// A data source that records which files were read, for building bundles.
pub struct Recording<D> {
    pub inner: D,
    pub read: RefCell<BTreeSet<String>>,
}

impl<D: DataSource> DataSource for Recording<D> {
    fn load(&self, env: &Rc<lua::env::Env>, name: &str, args: &[Value]) -> LResult<Vec<Value>> {
        let rel = format!("{name}.lua");
        if upstream::up::chunk(&format!("src/{rel}")).is_none() {
            self.read.borrow_mut().insert(rel);
        }
        self.inner.load(env, name, args)
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let r = self.inner.read_file(path);
        if r.is_some() {
            self.read
                .borrow_mut()
                .insert(path.trim_start_matches("./").to_string());
        }
        r
    }
}

/// A running build session.
pub struct Session {
    inner: Inner,
}

enum Inner {
    Translated {
        host: Rc<Host>,
        handle: Value,
    },
    /// `oracle/session.lua`: the same session on upstream on LuaJIT, in a
    /// process of its own
    #[cfg(not(target_arch = "wasm32"))]
    Oracle(RefCell<oracle::Process>),
}

fn describe(e: &LuaError) -> String {
    let mut s = lua::to_string(e.value()).to_string();
    for frame in e.traceback() {
        s.push_str("\n  ");
        s.push_str(frame);
    }
    s
}

impl Session {
    /// Starts upstream and opens an empty build.
    pub fn new(data: Rc<dyn DataSource>) -> Result<Session, String> {
        let host = Host::new(data);
        install_zlib(&host);
        host.start().map_err(|e| describe(&e))?;
        engine::install(&host.env).map_err(|e| describe(&e))?;
        let api = first(bridge::chunk(&host.env, &[]).map_err(|e| describe(&e))?);
        let handle = rt::index(&api, &Value::str("handle")).map_err(|e| describe(&e))?;
        let session = Session {
            inner: Inner::Translated { host, handle },
        };
        session.request(r#"{"cmd":"newBuild"}"#)?;
        Ok(session)
    }

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
            Inner::Translated { handle, .. } => {
                let r = rt::call(handle, &[Value::Str(json.as_bytes().into())])
                    .map_err(|e| describe(&e))?;
                Ok(String::from_utf8_lossy(lua::to_string(&first(r)).as_bytes()).into_owned())
            }
            #[cfg(not(target_arch = "wasm32"))]
            Inner::Oracle(p) => p.borrow_mut().request(json),
        }
    }

    /// The translated program's host; `None` on the oracle
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

fn bytes_arg(a: &[Value]) -> Vec<u8> {
    a.first()
        .map(|v| lua::to_string(v).as_bytes().to_vec())
        .unwrap_or_default()
}

/// `Inflate` and `Deflate`, which build codes use: zlib streams, as
/// SimpleGraphic's (headless upstream has no implementation).
fn install_zlib(host: &Host) {
    host.env.set(
        "Deflate",
        func("Deflate", |a| {
            let data = bytes_arg(a);
            let out = miniz_oxide::deflate::compress_to_vec_zlib(&data, 9);
            Ok(vec![Value::Str(out.as_slice().into())])
        }),
    );
    host.env.set(
        "Inflate",
        func("Inflate", |a| {
            let data = bytes_arg(a);
            match miniz_oxide::inflate::decompress_to_vec_zlib(&data) {
                Ok(out) => Ok(vec![Value::Str(out.as_slice().into())]),
                Err(_) => Ok(vec![Value::Nil]),
            }
        }),
    );
}
