//! The agent API: build-level verbs an agent calls, run on the build's
//! session within the caller's [`Grant`]. See `docs/agent.md`, "The agent
//! API".
//!
//! - **Reading** verbs answer from the session's views, compact and with
//!   names; text the build's author wrote comes only from `notes`, `items`
//!   with `text` and `export`, under an `untrusted` key ([`text`]).
//! - **Changing** verbs map onto the session's commands, so they run the
//!   same code as the UI. Each call is one labelled undo step, committed
//!   through the [`Store`] before it answers, with the stats it changed.
//! - **Grants**: a connection reads and edits the user's builds its grant
//!   names and the builds in its own agent directory (`agent/<name>`),
//!   nothing else; no verb widens a grant, takes a file path, deletes a
//!   build or rewrites history.
//!
//! The crate does no I/O: builds come from a [`Store`], and requests and
//! answers are JSON values, so any transport (the control socket, MCP)
//! can carry them.

mod assume;
pub mod engine;
mod eval;
mod evaluate;
mod generate;
mod goal;
pub mod grant;
mod read;
pub mod schema;
mod search;
pub mod store;
pub mod text;
mod write;

pub use grant::{AGENT_PREFIX, Grant, agent_dir_name};
pub use store::{BuildId, Entry, MemStore, Owner, Step, Store};

use serde_json::{Value as J, json};
use session::Dispatcher;
use session::typed::TypedData;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// A request's cap, in bytes
pub const MAX_REQUEST: usize = 8 << 20;
/// Undo steps kept in memory per build; older ones are in the history
pub const UNDO_DEPTH: usize = 100;
/// Builds kept open at once by default; the least recently used one no
/// client shows is closed when another opens
pub const MAX_OPEN: usize = 8;

/// The verbs, for help and for transports that list them
pub const VERBS: &[(&str, &str)] = &[
    ("builds", "the builds this connection may read and edit"),
    ("summary", "the sidebar's stats, grouped, and the warnings"),
    (
        "tree",
        "class, ascendancy, points, keystones, notables, masteries",
    ),
    ("skills", "socket groups and their gems"),
    (
        "items",
        "equipped items by slot and base, with the engine's lines; `text` adds the author's text, untrusted",
    ),
    ("config", "the configuration options set"),
    (
        "notes",
        "the build's author text (notes, labels, titles), untrusted",
    ),
    ("export", "the build's XML, untrusted"),
    (
        "search",
        "nodes, gems, uniques, bases or config options by name or text",
    ),
    (
        "evaluate_item",
        "an item (pasted text or an id from `items`) in a slot, evaluated exactly on a copy of the build, and the mod weights of the slot; changes nothing",
    ),
    (
        "eval",
        "variants of the build, each a list of changes as `apply` takes them, evaluated exactly on copies on all cores: one row per variant with the stats, their changes and the warnings each adds; changes nothing",
    ),
    (
        "assumptions",
        "the configuration the numbers rest on and which numbers are exact; with `stat`, the options that change it and by how much, each evaluated exactly",
    ),
    (
        "best",
        "one generator's moves (tree, tree_remove, masteries, supports, gem_levels, items_remove, config), each evaluated exactly and ranked by an objective under constraints; says how much it searched; changes nothing",
    ),
    (
        "improve",
        "greedy improvement by an objective under constraints: steps of the generators' best moves, each evaluated exactly, proposed as one `apply`; changes nothing",
    ),
    ("history", "the build's last versions"),
    ("changes_since", "the changes after a version"),
    ("allocate", "allocates nodes with their paths"),
    ("deallocate", "deallocates nodes and what depends on them"),
    ("set_mastery", "a mastery's effect"),
    ("set_ascendancy", "the ascendancy"),
    ("set_level", "the character level"),
    (
        "set_gem",
        "a gem's name, level, quality or state; appends one without `gem`",
    ),
    ("remove_gem", "removes a gem"),
    ("add_group", "a socket group with gems"),
    ("set_group", "a group's state, Full DPS inclusion or slot"),
    ("set_main_skill", "the main socket group and skill"),
    ("add_item", "an item from its text, as copied in game"),
    ("equip", "an item, or nothing, in a slot"),
    ("set_config", "a configuration option"),
    ("select_set", "the active tree, skill, item or config set"),
    ("apply", "several changes as one step"),
    ("undo", "undoes the build's last step"),
    (
        "redo",
        "applies again the step the last undo took back; any new change clears what can be redone",
    ),
    ("restore", "a version from the history, as a new version"),
    ("copy", "copies a build into the agent directory"),
    (
        "import",
        "a build from a build code or XML into the agent directory",
    ),
];

/// An open build: its session and what the agent API keeps beside it
struct Open<'d> {
    session: Dispatcher<'d>,
    /// The XML last saved
    xml: String,
    /// Bumped by each change
    version: u64,
    /// The steps that can be undone, oldest first
    undo: Vec<Undo>,
    /// The steps undone that can be applied again, the last undone last;
    /// each holds the XML with the step. Any new change clears it.
    redo: Vec<Undo>,
    /// Each change since the build was opened
    changes: Vec<Change>,
    /// When it was last used, by [`Agent`]'s clock
    used: u64,
}

/// What is kept of a build closed while idle: its version and changes go
/// on when it opens again; its session and undo list are gone
struct Parked {
    version: u64,
    changes: Vec<Change>,
}

/// A step that can be undone: the XML before it (or, to redo, with it),
/// its label and who made it
struct Undo {
    xml: String,
    label: String,
    source: String,
}

/// A change to a build, as `changes_since` gives it
#[derive(Clone, Debug)]
struct Change {
    version: u64,
    label: String,
    source: String,
    commit: Option<String>,
}

/// The agent API on a store of builds. Not `Send`: the host runs it on
/// one thread, and connections pass it their requests.
pub struct Agent<'d, S: Store> {
    data: &'d TypedData,
    store: S,
    open: BTreeMap<BuildId, Open<'d>>,
    /// Builds closed while idle
    parked: BTreeMap<BuildId, Parked>,
    /// Ticks on each use of an open build
    clock: u64,
    /// How many builds stay open at once
    max_open: usize,
    /// The builds a client shows, which stay open
    shown: BTreeSet<BuildId>,
    names: Names,
    /// The changes not yet taken by [`Agent::take_events`]
    events: Vec<Event>,
}

/// The names the engine knows, which answers quote; asked for once. Its
/// own value, so evaluations on other threads carry a copy.
#[derive(Clone, Default)]
pub(crate) struct Names {
    /// The gem names
    gems: Option<HashSet<String>>,
    /// The unique items, `Title, Base`
    uniques: Option<HashSet<String>>,
}

/// A change to a build, for the change events a host sends every client:
/// the build's new version, the step's label and who made it (`user`,
/// `agent <connection>`); the same as `changes_since` gives
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub build: BuildId,
    pub version: u64,
    pub label: String,
    pub source: String,
    pub commit: Option<String>,
}

/// Who makes a step: an agent's connection, or the user in the UI
#[derive(Clone, Copy)]
pub(crate) enum Who<'a> {
    Agent(&'a Grant),
    User,
}

impl Who<'_> {
    pub(crate) fn source(&self) -> String {
        match self {
            Who::Agent(g) => format!("agent {}", g.connection),
            Who::User => "user".into(),
        }
    }
}

type R<T> = Result<T, String>;

/// A session command's answer
fn cmd(s: &mut Dispatcher, req: J) -> R<J> {
    let text = s.request(&req.to_string())?;
    let mut v: J = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(e) = v.get("error") {
        let e = e
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| e.to_string());
        return Err(format!("{}: {e}", req["cmd"].as_str().unwrap_or("?")));
    }
    Ok(v["ok"].take())
}

impl<'d, S: Store> Agent<'d, S> {
    pub fn new(data: &'d TypedData, store: S) -> Agent<'d, S> {
        Agent {
            data,
            store,
            open: BTreeMap::new(),
            parked: BTreeMap::new(),
            clock: 0,
            max_open: MAX_OPEN,
            shown: BTreeSet::new(),
            names: Names::default(),
            events: Vec::new(),
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    /// Answers a request as text, `{"ok": ...}` or `{"error": "..."}`
    pub fn call_str(&mut self, grant: &Grant, request: &str) -> String {
        if request.len() > MAX_REQUEST {
            return json!({ "error": format!("a request is at most {MAX_REQUEST} bytes") })
                .to_string();
        }
        match serde_json::from_str::<J>(request) {
            Ok(req) => self.call(grant, &req).to_string(),
            Err(e) => json!({ "error": format!("not JSON: {e}") }).to_string(),
        }
    }

    /// Answers a request, `{"verb": "...", ...}`: `{"ok": ...}` or
    /// `{"error": "..."}`
    pub fn call(&mut self, grant: &Grant, request: &J) -> J {
        match self.answer(grant, request) {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "error": e }),
        }
    }

    fn answer(&mut self, grant: &Grant, req: &J) -> R<J> {
        let verb = req["verb"]
            .as_str()
            .ok_or("a request names its verb: {\"verb\": \"summary\", ...}")?;
        if !VERBS.iter().any(|(v, _)| *v == verb) {
            return Err(format!("no verb {verb:?}; the verbs: {}", verb_names()));
        }
        let build = req.get("build").and_then(J::as_str);
        match verb {
            "builds" => self.builds(grant),
            "evaluate_item" => {
                let id = grant.resolve(build)?;
                let name = grant.api_name(&id);
                self.evaluate_item(&id, name, req)
            }
            "eval" | "assumptions" | "best" | "improve" => {
                let id = grant.resolve(build)?;
                let name = grant.api_name(&id);
                match verb {
                    "eval" => self.eval(&id, name, req),
                    "assumptions" => self.assumptions(&id, name, req),
                    "best" => self.best(&id, name, req),
                    _ => self.improve(&id, name, req),
                }
            }
            "copy" => self.copy(grant, req),
            "import" => self.import(grant, req),
            "summary" | "tree" | "skills" | "items" | "config" | "notes" | "export" | "search"
            | "history" | "changes_since" => {
                let id = grant.resolve(build)?;
                self.read(grant, &id, verb, req)
            }
            _ => {
                let id = grant.resolve(build)?;
                self.write(grant, &id, verb, req)
            }
        }
    }

    /// The build's open session, opened from the store when it is not.
    /// Opening one may close the least recently used other build no
    /// client shows ([`Agent::set_max_open`]).
    fn open(&mut self, id: &BuildId) -> R<&mut Open<'d>> {
        self.clock += 1;
        if !self.open.contains_key(id) {
            let xml = self
                .store
                .read(id)?
                .ok_or_else(|| format!("no build {}", id.name))?;
            let mut session = Dispatcher::typed(self.data);
            cmd(
                &mut session,
                json!({ "cmd": "loadXml", "xml": xml, "name": id.name }),
            )?;
            // As the session writes it, so a change that changes nothing
            // is seen as such
            let xml = cmd(&mut session, json!({ "cmd": "saveXml" }))?["xml"]
                .as_str()
                .map(String::from)
                .unwrap_or(xml);
            let parked = self.parked.remove(id);
            self.open.insert(
                id.clone(),
                Open {
                    session,
                    xml,
                    version: parked.as_ref().map_or(0, |p| p.version),
                    undo: Vec::new(),
                    redo: Vec::new(),
                    changes: parked.map(|p| p.changes).unwrap_or_default(),
                    used: self.clock,
                },
            );
            self.close_idle(Some(id));
        }
        let o = self.open.get_mut(id).unwrap();
        o.used = self.clock;
        Ok(o)
    }

    /// Closes the least recently used builds no client shows while more
    /// than the cap are open, keeping `keep`. Their waiting commits are
    /// made ([`Store::closed`]); their versions and changes go on when
    /// they open again, their undo lists don't.
    fn close_idle(&mut self, keep: Option<&BuildId>) {
        while self.open.len() > self.max_open {
            let Some(idle) = self
                .open
                .iter()
                .filter(|(k, _)| Some(*k) != keep && !self.shown.contains(*k))
                .min_by_key(|(_, o)| o.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            let o = self.open.remove(&idle).unwrap();
            self.parked.insert(
                idle.clone(),
                Parked {
                    version: o.version,
                    changes: o.changes,
                },
            );
            self.store.closed(&idle);
        }
    }

    /// How many builds stay open at once, at least 1; others close when
    /// idle
    pub fn set_max_open(&mut self, n: usize) {
        self.max_open = n.max(1);
        self.close_idle(None);
    }

    /// The builds the UI's clients show: they stay open. Builds no longer
    /// shown close if more than the cap are open.
    pub fn set_shown(&mut self, shown: impl IntoIterator<Item = BuildId>) {
        let shown: BTreeSet<BuildId> = shown.into_iter().collect();
        if shown != self.shown {
            self.shown = shown;
            self.close_idle(None);
        }
    }

    /// The builds open now
    pub fn open_builds(&self) -> Vec<BuildId> {
        self.open.keys().cloned().collect()
    }

    /// A build's version if it was opened, without opening it
    fn known_version(&self, id: &BuildId) -> Option<u64> {
        self.open
            .get(id)
            .map(|o| o.version)
            .or_else(|| self.parked.get(id).map(|p| p.version))
    }

    fn builds(&mut self, grant: &Grant) -> R<J> {
        let mut out = Vec::new();
        for name in &grant.builds {
            let id = BuildId::user(name.clone());
            let exists = self.open.contains_key(&id) || self.store.read(&id)?.is_some();
            out.push(json!({
                "build": name,
                "owner": "user",
                "exists": exists,
                "version": self.known_version(&id),
            }));
        }
        for name in self.store.list(&Owner::Agent(grant.dir.clone()))? {
            let id = BuildId::agent(grant.dir.clone(), name);
            out.push(json!({
                "build": grant.api_name(&id),
                "owner": "agent",
                "version": self.known_version(&id),
            }));
        }
        Ok(json!({
            "builds": out,
            "default": grant.builds.first(),
            "agent_directory": grant.dir,
        }))
    }

    /// The gem names the engine knows
    fn gem_names(&mut self, s: &mut Dispatcher) -> R<&HashSet<String>> {
        self.names.gems(s)
    }

    /// The unique items the engine knows, `Title, Base`
    fn unique_names(&mut self, s: &mut Dispatcher) -> R<&HashSet<String>> {
        self.names.uniques(s)
    }
}

impl Names {
    /// The gem names the engine knows
    pub(crate) fn gems(&mut self, s: &mut Dispatcher) -> R<&HashSet<String>> {
        if self.gems.is_none() {
            let list = cmd(s, json!({ "cmd": "gemList" }))?;
            let names = list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| g["name"].as_str().map(String::from))
                .collect();
            self.gems = Some(names);
        }
        Ok(self.gems.as_ref().unwrap())
    }

    /// The unique items the engine knows, `Title, Base`
    pub(crate) fn uniques(&mut self, s: &mut Dispatcher) -> R<&HashSet<String>> {
        if self.uniques.is_none() {
            let list = cmd(s, json!({ "cmd": "itemDb", "kind": "unique" }))?;
            let names = list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| g["name"].as_str().map(String::from))
                .collect();
            self.uniques = Some(names);
        }
        Ok(self.uniques.as_ref().unwrap())
    }
}

fn verb_names() -> String {
    VERBS.iter().map(|(v, _)| *v).collect::<Vec<_>>().join(", ")
}

/// A request's optional unsigned field
fn opt_u64(req: &J, key: &str) -> R<Option<u64>> {
    match req.get(key) {
        None | Some(J::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} is a whole number")),
    }
}

fn opt_str<'a>(req: &'a J, key: &str) -> R<Option<&'a str>> {
    match req.get(key) {
        None | Some(J::Null) => Ok(None),
        Some(J::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("{key} is a string")),
    }
}

fn need_str<'a>(req: &'a J, key: &str) -> R<&'a str> {
    opt_str(req, key)?.ok_or_else(|| format!("{key} is missing"))
}

fn opt_bool(req: &J, key: &str) -> R<Option<bool>> {
    match req.get(key) {
        None | Some(J::Null) => Ok(None),
        Some(J::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("{key} is true or false")),
    }
}

/// Fails on fields a verb doesn't take, so a typo isn't silently ignored
fn check_fields(req: &J, allowed: &[&str]) -> R<()> {
    if let Some(o) = req.as_object() {
        for k in o.keys() {
            if k != "verb" && !allowed.contains(&k.as_str()) {
                return Err(format!(
                    "{} takes no field {k:?}; it takes: {}",
                    req["verb"].as_str().unwrap_or("?"),
                    allowed.join(", ")
                ));
            }
        }
    }
    Ok(())
}
