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

mod evaluate;
pub mod grant;
mod read;
pub mod store;
pub mod text;
mod write;

pub use grant::{AGENT_PREFIX, Grant, agent_dir_name};
pub use store::{BuildId, Entry, MemStore, Owner, Step, Store};

use serde_json::{Value as J, json};
use session::Dispatcher;
use session::typed::TypedData;
use std::collections::{BTreeMap, HashSet};

/// A request's cap, in bytes
pub const MAX_REQUEST: usize = 8 << 20;
/// Undo steps kept in memory per build; older ones are in the history
pub const UNDO_DEPTH: usize = 100;

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
    /// The steps that can be undone: the XML before each, and its label
    undo: Vec<(String, String)>,
    /// Each change since the build was opened
    changes: Vec<Change>,
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
    /// The gem names, once asked for
    gems: Option<HashSet<String>>,
    /// The unique items, `Title, Base`, once asked for
    uniques: Option<HashSet<String>>,
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
            gems: None,
            uniques: None,
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

    /// The build's open session, opened from the store when it is not
    fn open(&mut self, id: &BuildId) -> R<&mut Open<'d>> {
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
            self.open.insert(
                id.clone(),
                Open {
                    session,
                    xml,
                    version: 0,
                    undo: Vec::new(),
                    changes: Vec::new(),
                },
            );
        }
        Ok(self.open.get_mut(id).unwrap())
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
                "version": self.open.get(&id).map(|o| o.version),
            }));
        }
        for name in self.store.list(&Owner::Agent(grant.dir.clone()))? {
            let id = BuildId::agent(grant.dir.clone(), name);
            out.push(json!({
                "build": grant.api_name(&id),
                "owner": "agent",
                "version": self.open.get(&id).map(|o| o.version),
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
    fn unique_names(&mut self, s: &mut Dispatcher) -> R<&HashSet<String>> {
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
