//! Engine API: a build session over upstream's translated program.
//!
//! A [`Session`] runs upstream headless (see `upstream::host`) and takes JSON
//! requests, `{"cmd": "...", ...}`, answered by `lua/session.lua`, which drives
//! upstream's build mode the way its UI does. The engine does no I/O: data
//! files come from a [`DataSource`], either upstream's source directory
//! ([`upstream::imported::Imported`]) or files handed over in memory
//! ([`Files`]), as the browser does.

pub mod bundle;

use lua::rt::{self, first, func};
use lua::{LResult, LuaError, Value};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
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

/// Data files in memory, by path relative to upstream's `src/`. Files that
/// are asked for but missing are recorded, so a caller can fetch them and
/// retry the request.
#[derive(Default)]
pub struct Files {
    files: RefCell<HashMap<String, Rc<Vec<u8>>>>,
    missing: RefCell<BTreeSet<String>>,
}

impl Files {
    pub fn new() -> Files {
        Files::default()
    }

    pub fn insert(&self, path: impl Into<String>, bytes: Vec<u8>) {
        self.files.borrow_mut().insert(path.into(), Rc::new(bytes));
    }

    fn get(&self, path: &str) -> Option<Rc<Vec<u8>>> {
        let path = path.trim_start_matches("./");
        let found = self.files.borrow().get(path).cloned();
        if found.is_none() {
            self.missing.borrow_mut().insert(path.to_string());
        }
        found
    }

    /// Files asked for since the last call that were not there.
    pub fn take_missing(&self) -> Vec<String> {
        std::mem::take(&mut *self.missing.borrow_mut())
            .into_iter()
            .collect()
    }
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
        upstream::imported::evaluate(env, &rel, &src, args)
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
    host: Rc<Host>,
    handle: Value,
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
        let api = first(bridge::chunk(&host.env, &[]).map_err(|e| describe(&e))?);
        let handle = rt::index(&api, &Value::str("handle")).map_err(|e| describe(&e))?;
        let session = Session { host, handle };
        session.request(r#"{"cmd":"newBuild"}"#)?;
        Ok(session)
    }

    /// Answers one JSON request with a JSON response: `{"ok": ...}` or
    /// `{"error": "..."}`. `Err` only when the request could not be run.
    pub fn request(&self, json: &str) -> Result<String, String> {
        let r = rt::call(&self.handle, &[Value::Str(json.as_bytes().into())])
            .map_err(|e| describe(&e))?;
        Ok(String::from_utf8_lossy(lua::to_string(&first(r)).as_bytes()).into_owned())
    }

    pub fn host(&self) -> &Rc<Host> {
        &self.host
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
