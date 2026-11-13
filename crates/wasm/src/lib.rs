//! The engine API for the browser, compiled to `wasm32-unknown-unknown`.
//!
//! A plain C ABI instead of generated bindings: the worker (`web/src/engine`)
//! copies bytes in with `alloc`, calls a function, and reads the result
//! (UTF-8 JSON) from `result_ptr`/`result_len`. One engine instance per
//! module instance; the engine does no I/O, data files are handed in.
//!
//! Commands run on the typed engine, whose data comes in its binary form
//! (`session::bundle`). With the `translation` feature (the default) the
//! commands it does not have yet run on upstream's translated program,
//! which reads upstream's Lua data files from the bundle; without it they
//! are "not ported" errors.

use session::typed::TypedData;
use session::{Dispatcher, Files};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static FILES: Rc<Files> = Rc::new(Files::new());
    static DATA: RefCell<Option<&'static TypedData>> = const { RefCell::new(None) };
    static SESSION: RefCell<Option<Dispatcher<'static>>> = const { RefCell::new(None) };
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn set_result(s: String) {
    RESULT.with(|r| *r.borrow_mut() = s.into_bytes());
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn error(msg: &str) -> String {
    format!("{{\"error\":{}}}", json_string(msg))
}

/// # Safety
/// `ptr..ptr+len` must be memory returned by [`alloc`].
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` must come from [`alloc`] with the same `len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len.max(1)) });
}

#[unsafe(no_mangle)]
pub extern "C" fn result_ptr() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn result_len() -> usize {
    RESULT.with(|r| r.borrow().len())
}

/// 1 when commands the typed engine does not have fall back to the
/// translation, 0 when they are "not ported" errors
#[unsafe(no_mangle)]
pub extern "C" fn has_translation() -> u32 {
    cfg!(feature = "translation") as u32
}

/// Adds one data file (path relative to upstream's `src/`).
///
/// # Safety
/// Both ranges must be memory returned by [`alloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn add_file(
    path: *const u8,
    path_len: usize,
    data: *const u8,
    data_len: usize,
) {
    let path = String::from_utf8_lossy(unsafe { bytes(path, path_len) }).into_owned();
    let data = unsafe { bytes(data, data_len) }.to_vec();
    FILES.with(|f| f.insert(path, data));
}

/// Adds the files of a data bundle; the result is `{"ok": count}`.
///
/// # Safety
/// The range must be memory returned by [`alloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn add_bundle(ptr: *const u8, len: usize) {
    let r = FILES.with(|f| session::bundle::unpack(unsafe { bytes(ptr, len) }, f));
    set_result(match r {
        Ok(n) => format!("{{\"ok\":{n}}}"),
        Err(e) => error(&e),
    });
}

/// Files asked for but missing since the last call, as a JSON array.
fn missing_json() -> String {
    let list: Vec<String> = FILES
        .with(|f| f.take_missing())
        .iter()
        .map(|p| json_string(p))
        .collect();
    format!("[{}]", list.join(","))
}

/// The typed data, read once
fn typed_data() -> Result<&'static TypedData, String> {
    if let Some(d) = DATA.with(|d| *d.borrow()) {
        return Ok(d);
    }
    let d: &'static TypedData =
        Box::leak(Box::new(TypedData::from_files(FILES.with(|f| f.clone()))?));
    DATA.with(|cell| *cell.borrow_mut() = Some(d));
    Ok(d)
}

/// The session, with a new build open on the typed engine
fn dispatcher() -> Result<Dispatcher<'static>, String> {
    let mut d = session()?;
    let r = d.request(r#"{"cmd":"newBuild"}"#)?;
    if r.starts_with("{\"error\"") {
        return Err(r);
    }
    Ok(d)
}

#[cfg(feature = "translation")]
fn session() -> Result<Dispatcher<'static>, String> {
    let data = typed_data()?;
    let translated = session::Session::new(FILES.with(|f| f.clone()))?;
    Ok(Dispatcher::new(
        translated,
        Some(data),
        session::Prefer::Typed,
    ))
}

#[cfg(not(feature = "translation"))]
fn session() -> Result<Dispatcher<'static>, String> {
    Ok(Dispatcher::typed(typed_data()?))
}

/// Starts the engine. On a missing data file the result is
/// `{"error": ..., "missing": [...]}`; add the files and start again.
#[unsafe(no_mangle)]
pub extern "C" fn start() {
    FILES.with(|f| f.take_missing());
    let r = match dispatcher() {
        Ok(s) => {
            SESSION.with(|cell| *cell.borrow_mut() = Some(s));
            "{\"ok\":{}}".to_string()
        }
        Err(e) => {
            let missing = missing_json();
            format!("{{\"error\":{},\"missing\":{missing}}}", json_string(&e))
        }
    };
    set_result(r);
}

/// Runs one JSON request (see `crates/session/lua/session.lua`). On an
/// error, files that were missing are listed in `"missing"`.
///
/// # Safety
/// The range must be memory returned by [`alloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn request(ptr: *const u8, len: usize) {
    let req = String::from_utf8_lossy(unsafe { bytes(ptr, len) }).into_owned();
    FILES.with(|f| f.take_missing());
    let r = SESSION.with(|cell| match &mut *cell.borrow_mut() {
        Some(s) => s.request(&req).unwrap_or_else(|e| error(&e)),
        None => error("engine not started"),
    });
    let r = if r.starts_with("{\"error\"") {
        let missing = missing_json();
        if missing != "[]" {
            format!("{},\"missing\":{missing}}}", &r[..r.len() - 1])
        } else {
            r
        }
    } else {
        r
    };
    set_result(r);
}
