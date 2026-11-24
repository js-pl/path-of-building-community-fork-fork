//! Where builds are kept: the host's builds directory with its history
//! (`crates/host`), or memory ([`MemStore`], for tests and the browser).
//! The verbs do no I/O themselves; they go through a [`Store`].

use std::collections::BTreeMap;

/// Whose a build is: the user's own, or in an agent's directory
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    User,
    /// An agent directory, by its name
    Agent(String),
}

/// A build in a store: its owner and its directory name
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BuildId {
    pub owner: Owner,
    pub name: String,
}

impl BuildId {
    pub fn user(name: impl Into<String>) -> BuildId {
        BuildId {
            owner: Owner::User,
            name: name.into(),
        }
    }

    pub fn agent(dir: impl Into<String>, name: impl Into<String>) -> BuildId {
        BuildId {
            owner: Owner::Agent(dir.into()),
            name: name.into(),
        }
    }
}

/// One step's commit: the undo step's label, who made it and what it
/// changed (`Full DPS: +3.1%`)
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub label: String,
    /// The connection that made it
    pub agent: String,
    /// A restore of an earlier version
    pub restore: bool,
    /// A step undone before, applied again
    pub redo: bool,
    pub trailers: Vec<(String, String)>,
}

/// A version in a build's history
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub version: String,
    /// Unix seconds
    pub time: i64,
    pub label: String,
    pub source: String,
    pub trailers: Vec<(String, String)>,
}

pub type Result<T> = std::result::Result<T, String>;

/// Keeps builds and their history. No method deletes a build or rewrites
/// its history.
pub trait Store {
    /// The builds an owner has, by name, sorted
    fn list(&self, owner: &Owner) -> Result<Vec<String>>;
    /// A build's current XML; `None` when there is no such build
    fn read(&self, id: &BuildId) -> Result<Option<String>>;
    /// Makes a new build; an error if one by that name exists
    fn create(&mut self, id: &BuildId, xml: &str, step: &Step) -> Result<Option<String>>;
    /// Saves a build's new version and commits it before returning; the
    /// version, or `None` without a history
    fn commit(&mut self, id: &BuildId, xml: &str, step: &Step) -> Result<Option<String>>;
    /// Saves the user's edit of a build; its commit may wait, coalesced
    /// with the user's next edits
    fn user_edit(&mut self, id: &BuildId, xml: &str, label: &str) -> Result<()>;
    /// The last `n` versions, newest first; empty without a history
    fn log(&self, id: &BuildId, n: usize) -> Result<Vec<Entry>>;
    /// The build's XML at a version
    fn read_version(&self, id: &BuildId, version: &str) -> Result<String>;
}

/// A store in memory, with a history of its own
#[derive(Default, Debug)]
pub struct MemStore {
    pub builds: BTreeMap<BuildId, Vec<(Entry, String)>>,
}

impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }

    /// Adds a user's build, as if it were saved before
    pub fn insert(&mut self, id: BuildId, xml: impl Into<String>) {
        let entry = Entry {
            version: "v1".into(),
            time: 0,
            label: "saved".into(),
            source: "user".into(),
            trailers: Vec::new(),
        };
        self.builds.insert(id, vec![(entry, xml.into())]);
    }

    fn push(&mut self, id: &BuildId, xml: &str, step: &Step) -> Option<String> {
        let source = if step.restore {
            "restore".to_string()
        } else if step.redo {
            "redo".to_string()
        } else {
            format!("agent {}", step.agent)
        };
        self.push_as(id, xml, step, source)
    }

    fn push_as(&mut self, id: &BuildId, xml: &str, step: &Step, source: String) -> Option<String> {
        let list = self.builds.entry(id.clone()).or_default();
        if list.last().is_some_and(|(_, x)| x == xml) {
            return None;
        }
        let version = format!("v{}", list.len() + 1);
        let entry = Entry {
            version: version.clone(),
            time: 0,
            label: step.label.clone(),
            source,
            trailers: step.trailers.clone(),
        };
        list.push((entry, xml.to_string()));
        Some(version)
    }
}

impl Store for MemStore {
    fn list(&self, owner: &Owner) -> Result<Vec<String>> {
        Ok(self
            .builds
            .keys()
            .filter(|id| &id.owner == owner)
            .map(|id| id.name.clone())
            .collect())
    }

    fn read(&self, id: &BuildId) -> Result<Option<String>> {
        Ok(self
            .builds
            .get(id)
            .and_then(|l| l.last())
            .map(|(_, x)| x.clone()))
    }

    fn create(&mut self, id: &BuildId, xml: &str, step: &Step) -> Result<Option<String>> {
        if self.builds.contains_key(id) {
            return Err(format!("a build {} exists already", id.name));
        }
        Ok(self.push(id, xml, step))
    }

    fn commit(&mut self, id: &BuildId, xml: &str, step: &Step) -> Result<Option<String>> {
        if !self.builds.contains_key(id) {
            return Err(format!("no build {}", id.name));
        }
        Ok(self.push(id, xml, step))
    }

    fn user_edit(&mut self, id: &BuildId, xml: &str, label: &str) -> Result<()> {
        if !self.builds.contains_key(id) {
            return Err(format!("no build {}", id.name));
        }
        let step = Step {
            label: label.into(),
            agent: String::new(),
            restore: false,
            redo: false,
            trailers: Vec::new(),
        };
        // Committed at once: memory has nothing to coalesce
        self.push_as(id, xml, &step, "user".into());
        Ok(())
    }

    fn log(&self, id: &BuildId, n: usize) -> Result<Vec<Entry>> {
        Ok(self
            .builds
            .get(id)
            .map(|l| l.iter().rev().take(n).map(|(e, _)| e.clone()).collect())
            .unwrap_or_default())
    }

    fn read_version(&self, id: &BuildId, version: &str) -> Result<String> {
        self.builds
            .get(id)
            .and_then(|l| l.iter().find(|(e, _)| e.version == version))
            .map(|(_, x)| x.clone())
            .ok_or_else(|| format!("no version {version}"))
    }
}
