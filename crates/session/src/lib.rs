//! Engine API: build sessions answering JSON requests, `{"cmd": "...", ...}`.
//!
//! A [`Dispatcher`] answers each command with the typed engine
//! ([`typed`]) where it is ported. With the `translation` feature (the
//! default) the other commands go to a [`Session`] running upstream's
//! translated program headless (see `upstream::host`), answered by
//! `lua/session.lua`, which drives upstream's build mode the way its UI
//! does; without it they are "not ported" errors, and neither the
//! translation nor the Lua runtime is linked. The engine does no I/O: data
//! files come from upstream's source directory or are handed over in
//! memory ([`Files`]), as the browser does.

pub mod bundle;
pub mod dispatch;
#[cfg(feature = "translation")]
mod translated;
pub mod typed;

pub use dispatch::{Dispatcher, Prefer};
#[cfg(feature = "translation")]
pub use translated::{Recording, Session};

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

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

    pub fn get(&self, path: &str) -> Option<Rc<Vec<u8>>> {
        let path = path.trim_start_matches("./");
        let found = self.files.borrow().get(path).cloned();
        if found.is_none() {
            self.missing.borrow_mut().insert(path.to_string());
        }
        found
    }

    /// Drops a file, to free its memory once it is read
    pub fn remove(&self, path: &str) {
        self.files.borrow_mut().remove(path);
    }

    /// Files asked for since the last call that were not there.
    pub fn take_missing(&self) -> Vec<String> {
        std::mem::take(&mut *self.missing.borrow_mut())
            .into_iter()
            .collect()
    }
}
