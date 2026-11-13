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

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// Data files in memory, by path relative to upstream's `src/`. Files that
/// are asked for but missing are recorded, so a caller can fetch them and
/// retry the request. A clone shares the files, also across threads (the
/// timeless jewels' look-up tables are read through one).
#[derive(Clone, Default)]
pub struct Files {
    inner: Arc<Mutex<FileStore>>,
}

#[derive(Default)]
struct FileStore {
    files: HashMap<String, Arc<Vec<u8>>>,
    missing: BTreeSet<String>,
}

impl Files {
    pub fn new() -> Files {
        Files::default()
    }

    fn store(&self) -> std::sync::MutexGuard<'_, FileStore> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, path: impl Into<String>, bytes: Vec<u8>) {
        self.store().files.insert(path.into(), Arc::new(bytes));
    }

    pub fn get(&self, path: &str) -> Option<Arc<Vec<u8>>> {
        let path = path.trim_start_matches("./");
        let mut store = self.store();
        let found = store.files.get(path).cloned();
        if found.is_none() {
            store.missing.insert(path.to_string());
        }
        found
    }

    /// Drops a file, to free its memory once it is read
    pub fn remove(&self, path: &str) {
        self.store().files.remove(path);
    }

    /// Files asked for since the last call that were not there.
    pub fn take_missing(&self) -> Vec<String> {
        std::mem::take(&mut self.store().missing)
            .into_iter()
            .collect()
    }
}
