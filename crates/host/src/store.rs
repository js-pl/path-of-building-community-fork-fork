//! The builds directory, with each build's history:
//!
//! ```text
//! <builds>/
//!   <name>/build.xml                 the user's builds, a git repository each
//!   agents/<agent dir>/<name>/...    the agents' builds, the same way
//! ```
//!
//! Every agent step is committed with `Recorder::agent_step` before the
//! store returns. Without git the files are still written, and there is
//! no history. Nothing here deletes a build or rewrites its history.

use agent::store::{BuildId, Entry, Owner, Result, Step, Store};
use history::{BUILD_FILE, BuildRepo, Git, Message, Recorder, Source};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The directory of the agents' builds in the builds directory; no user's
/// build has this name
pub const AGENTS_DIR: &str = "agents";
/// Where cleaned up agent directories go, which only the user empties; no
/// user's build has this name either
pub const TRASH_DIR: &str = "trash";

/// Whether a name is one of the builds directory's own, never a build's
pub fn reserved(name: &str) -> bool {
    name == AGENTS_DIR || name == TRASH_DIR
}

pub struct FsStore {
    root: PathBuf,
    git: Option<Git>,
    recorders: HashMap<BuildId, Recorder>,
    quiet: Option<std::time::Duration>,
}

impl FsStore {
    /// The builds in `root`; `git` keeps their history, `None` keeps none
    pub fn new(root: impl Into<PathBuf>, git: Option<Git>) -> FsStore {
        FsStore {
            root: root.into(),
            git,
            recorders: HashMap::new(),
            quiet: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn has_history(&self) -> bool {
        self.git.is_some()
    }

    fn owner_dir(&self, owner: &Owner) -> PathBuf {
        match owner {
            Owner::User => self.root.clone(),
            Owner::Agent(d) => self.root.join(AGENTS_DIR).join(d),
        }
    }

    /// A build's directory; its name is one directory, never a path
    pub fn dir(&self, id: &BuildId) -> Result<PathBuf> {
        let ok = !id.name.is_empty()
            && id.name != "."
            && id.name != ".."
            && !id.name.contains(['/', '\\'])
            && !(id.owner == Owner::User && reserved(&id.name));
        if let Owner::Agent(d) = &id.owner
            && (d.is_empty() || d == "." || d == ".." || d.contains(['/', '\\']))
        {
            return Err(format!("not an agent directory: {d:?}"));
        }
        if !ok {
            return Err(format!("not a build name: {:?}", id.name));
        }
        Ok(self.owner_dir(&id.owner).join(&id.name))
    }

    fn recorder(&mut self, id: &BuildId) -> Result<&mut Recorder> {
        if !self.recorders.contains_key(id) {
            let repo = match &self.git {
                Some(git) => {
                    Some(BuildRepo::open(git.clone(), self.dir(id)?).map_err(|e| e.to_string())?)
                }
                None => None,
            };
            let mut r = Recorder::new(repo);
            if let Some(q) = self.quiet {
                r = r.with_quiet(q);
            }
            self.recorders.insert(id.clone(), r);
        }
        Ok(self.recorders.get_mut(id).unwrap())
    }

    fn repo(&mut self, id: &BuildId) -> Result<Option<BuildRepo>> {
        Ok(self.recorder(id)?.repo().cloned())
    }

    fn save(&mut self, id: &BuildId, xml: &str, step: &Step) -> Result<Option<String>> {
        let source = if step.restore {
            Source::Restore
        } else {
            Source::Agent(step.agent.clone())
        };
        let mut message = Message::new(source, step.label.clone());
        message.trailers = step.trailers.clone();
        let file = self.dir(id)?.join(BUILD_FILE);
        let rec = self.recorder(id)?;
        if rec.is_enabled() {
            // A build that has no history yet is committed as it was found
            // first, so the agent's first step can be undone from it
            let new = rec
                .repo()
                .is_some_and(|r| r.head().ok().flatten().is_none());
            if new && let Ok(found) = std::fs::read_to_string(&file) {
                rec.user_edit(found, "as found before the first agent step");
            }
            // Writes build.xml and commits it
            return rec
                .agent_step(xml, &message)
                .map(|v| v.map(|v| v.as_str().to_string()))
                .map_err(|e| e.to_string());
        }
        self.write(id, xml)?;
        Ok(None)
    }

    /// Writes a build's file, whole, moved into place
    fn write(&self, id: &BuildId, xml: &str) -> Result<()> {
        let dir = self.dir(id)?;
        let file = dir.join(BUILD_FILE);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = dir.join(format!("{BUILD_FILE}.tmp"));
        std::fs::write(&tmp, xml).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &file).map_err(|e| format!("{}: {e}", file.display()))
    }
}

impl Store for FsStore {
    fn list(&self, owner: &Owner) -> Result<Vec<String>> {
        let dir = self.owner_dir(owner);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join(BUILD_FILE).is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !(*owner == Owner::User && reserved(n)))
            .collect();
        names.sort();
        Ok(names)
    }

    fn read(&self, id: &BuildId) -> Result<Option<String>> {
        let file = self.dir(id)?.join(BUILD_FILE);
        match std::fs::read_to_string(&file) {
            Ok(x) => Ok(Some(x)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", file.display())),
        }
    }

    fn create(&mut self, id: &BuildId, xml: &str, step: &Step) -> Result<Option<String>> {
        if self.dir(id)?.join(BUILD_FILE).exists() {
            return Err(format!("a build {} exists already", id.name));
        }
        self.save(id, xml, step)
    }

    fn commit(&mut self, id: &BuildId, xml: &str, step: &Step) -> Result<Option<String>> {
        if !self.dir(id)?.join(BUILD_FILE).exists() {
            return Err(format!("no build {}", id.name));
        }
        self.save(id, xml, step)
    }

    fn user_edit(&mut self, id: &BuildId, xml: &str, label: &str) -> Result<()> {
        let file = self.dir(id)?.join(BUILD_FILE);
        if !file.exists() {
            return Err(format!("no build {}", id.name));
        }
        let rec = self.recorder(id)?;
        if rec.is_enabled()
            && !rec.is_pending()
            && rec
                .repo()
                .is_some_and(|r| r.head().ok().flatten().is_none())
            && let Ok(found) = std::fs::read_to_string(&file)
        {
            // As it was found, so the user's first edit can be undone from
            // the history too
            rec.user_edit(found, "as found before the first edit");
            rec.flush().map_err(|e| e.to_string())?;
        }
        // The file is saved at once; its commit waits for the user's next
        // edits (`Recorder`)
        self.write(id, xml)?;
        self.recorder(id)?.user_edit(xml, label);
        Ok(())
    }

    fn log(&self, id: &BuildId, n: usize) -> Result<Vec<Entry>> {
        let Some(git) = &self.git else {
            return Ok(Vec::new());
        };
        let dir = self.dir(id)?;
        if !dir.join(".git").exists() {
            return Ok(Vec::new());
        }
        let repo = BuildRepo::open(git.clone(), dir).map_err(|e| e.to_string())?;
        Ok(repo
            .log(n)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|e| Entry {
                version: e.version.as_str().to_string(),
                time: e.time,
                label: e.label,
                source: e.source.to_string(),
                trailers: e.trailers,
            })
            .collect())
    }

    fn read_version(&self, id: &BuildId, version: &str) -> Result<String> {
        let git = self.git.as_ref().ok_or("there is no history: git is off")?;
        let repo = BuildRepo::open(git.clone(), self.dir(id)?).map_err(|e| e.to_string())?;
        let v = repo.resolve(version).map_err(|e| e.to_string())?;
        repo.read(&v).map_err(|e| e.to_string())
    }
}

impl FsStore {
    /// Commits the user's edits that waited long enough; call it often.
    /// The versions committed, by build.
    pub fn tick(&mut self) -> Vec<(BuildId, Result<String>)> {
        let mut out = Vec::new();
        for (id, r) in &mut self.recorders {
            match r.tick() {
                Ok(Some(v)) => out.push((id.clone(), Ok(v.as_str().to_string()))),
                Ok(None) => {}
                Err(e) => out.push((id.clone(), Err(e.to_string()))),
            }
        }
        out
    }

    /// Commits a build's waiting edits now and forgets its recorder (its
    /// directory is moved, say)
    pub fn release(&mut self, id: &BuildId) -> Result<()> {
        match self.recorders.remove(id) {
            Some(r) => r.close().map(|_| ()).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    /// Waits `quiet` before committing the user's edits of builds opened
    /// from now on (tests)
    pub fn set_quiet(&mut self, quiet: std::time::Duration) {
        self.quiet = Some(quiet);
    }

    /// Adds a user's build, committed at once; an error if one by that
    /// name exists
    pub fn add(&mut self, id: &BuildId, xml: &str, label: &str) -> Result<()> {
        if self.dir(id)?.join(BUILD_FILE).exists() {
            return Err(format!("a build {} exists already", id.name));
        }
        self.write(id, xml)?;
        let rec = self.recorder(id)?;
        rec.user_edit(xml, label);
        rec.flush().map(|_| ()).map_err(|e| e.to_string())
    }

    /// Commits whatever the recorders hold; the host stops
    pub fn close(&mut self) {
        for (_, r) in self.recorders.drain() {
            let _ = r.close();
        }
    }

    /// The repository of a build, for tests and tools
    pub fn build_repo(&mut self, id: &BuildId) -> Result<Option<BuildRepo>> {
        self.repo(id)
    }
}
