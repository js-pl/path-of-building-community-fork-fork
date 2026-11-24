//! The web app's clients on the host's one engine thread. The host's
//! session of a build is its only copy: the UI's engine API requests run
//! on it beside the agents' verbs ([`agent::Agent::engine`]), and every
//! change, the UI's or an agent's, goes to every client as a change event,
//! in the order the changes were made.
//!
//! A client sends JSON text messages, each with an `id` the answer
//! carries:
//!
//! ```text
//! → {"id": 1, "req": {"cmd": "state"}}        the engine API, on the client's build
//! ← {"id": 1, "ok": {...}, "version": 3}
//! → {"id": 2, "host": "builds", "args": {}}   the host's own requests
//! ← {"id": 2, "ok": {...}}
//! ← {"event": "change", "build": "RF", "version": 4, "label": "...", "source": "agent claude", "commit": "1a2b3c4"}
//! ```
//!
//! On connecting, a client is sent `{"event": "hello", ...}` with the
//! build it shows. `loadXml`, `loadCode` and `newBuild` open a new build
//! (nothing replaces a build), and the client is sent `{"event": "open"}`.

use crate::FsStore;
use crate::control::Call;
use crate::store::AGENTS_DIR;
use crate::ws::Out;
use agent::engine::{OPENS_A_BUILD, event_json};
use agent::{Agent, BuildId, Owner, Store};
use serde_json::{Value as J, json};
use session::Dispatcher;
use session::typed::TypedData;
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::Duration;

/// What the engine thread is asked
pub enum Msg {
    /// An agent API call, from the control socket or MCP
    Agent(Call),
    /// A web app client connected; its frames go to `out`
    Open { client: u64, out: mpsc::Sender<Out> },
    /// A message from a web app client
    Ui { client: u64, text: String },
    /// A web app client is gone
    Close { client: u64 },
    /// The host stops
    Stop,
}

/// Passes the agent API's calls on to the engine thread
pub fn forward(calls: mpsc::Receiver<Call>, to: mpsc::Sender<Msg>) {
    for c in calls {
        if to.send(Msg::Agent(c)).is_err() {
            break;
        }
    }
}

struct Client {
    out: mpsc::Sender<Out>,
    build: Option<BuildId>,
}

/// The engine thread's state: the agent API on the builds, and the web
/// app's clients
pub struct Host<'d> {
    pub agent: Agent<'d, FsStore>,
    data: &'d TypedData,
    clients: BTreeMap<u64, Client>,
    /// The build a client opens first, by name
    pub default_build: Option<String>,
}

type R<T> = Result<T, String>;

/// A build's name for the UI: a user's build by its name, an agent's as
/// `agents/<dir>/<name>`
pub fn ui_name(id: &BuildId) -> String {
    match &id.owner {
        Owner::User => id.name.clone(),
        Owner::Agent(d) => format!("{AGENTS_DIR}/{d}/{}", id.name),
    }
}

/// A build from its UI name
pub fn parse_name(name: &str) -> R<BuildId> {
    let id = match name.strip_prefix(&format!("{AGENTS_DIR}/")) {
        Some(rest) => {
            let (d, n) = rest
                .split_once('/')
                .ok_or_else(|| format!("not a build: {name:?}"))?;
            BuildId::agent(d, n)
        }
        None => BuildId::user(name),
    };
    Ok(id)
}

impl<'d> Host<'d> {
    pub fn new(data: &'d TypedData, store: FsStore) -> Host<'d> {
        Host {
            agent: Agent::new(data, store),
            data,
            clients: BTreeMap::new(),
            default_build: None,
        }
    }

    /// Answers until every sender is gone, committing the user's edits
    /// when they have waited long enough
    pub fn run(&mut self, msgs: mpsc::Receiver<Msg>) {
        loop {
            match msgs.recv_timeout(Duration::from_millis(500)) {
                Ok(Msg::Stop) => break,
                Ok(m) => self.handle(m),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            for (id, r) in self.agent.store_mut().tick() {
                if let Err(e) = r {
                    eprintln!("pob host: {}: the history: {e}", ui_name(&id));
                }
            }
        }
        self.agent.store_mut().close();
    }

    pub fn handle(&mut self, m: Msg) {
        match m {
            Msg::Agent(c) => {
                let answer = self.agent.call_str(&c.grant, &c.line);
                // The answer first, then the events, as for the UI's own
                let _ = c.reply.send(answer);
            }
            Msg::Open { client, out } => {
                self.clients.insert(client, Client { out, build: None });
                let hello = match self.initial_build() {
                    Ok(id) => {
                        let version = self.agent.version(&id).unwrap_or_default();
                        let v = json!({
                            "event": "hello",
                            "build": ui_name(&id),
                            "version": version,
                            "history": self.agent.store().has_history(),
                        });
                        if let Some(c) = self.clients.get_mut(&client) {
                            c.build = Some(id);
                        }
                        v
                    }
                    Err(e) => json!({ "event": "hello", "error": e }),
                };
                self.send(client, &hello);
            }
            Msg::Ui { client, text } => {
                let answer = self.ui(client, &text);
                self.send(client, &answer);
            }
            Msg::Close { client } => {
                self.clients.remove(&client);
            }
            Msg::Stop => {}
        }
        self.broadcast();
    }

    fn send(&self, client: u64, v: &J) {
        if let Some(c) = self.clients.get(&client) {
            let _ = c.out.send(Out::Text(v.to_string()));
        }
    }

    /// Sends the changes made to every client, in order
    fn broadcast(&mut self) {
        for e in self.agent.take_events() {
            let v = event_json(&e, &ui_name(&e.build)).to_string();
            for c in self.clients.values() {
                let _ = c.out.send(Out::Text(v.clone()));
            }
        }
    }

    /// The build a client shows first: the default build, else the user's
    /// build saved last, else a new one
    fn initial_build(&mut self) -> R<BuildId> {
        if let Some(name) = &self.default_build {
            return Ok(BuildId::user(name.clone()));
        }
        let store = self.agent.store();
        let mut newest: Option<(std::time::SystemTime, String)> = None;
        for name in store.list(&Owner::User)? {
            let id = BuildId::user(name.clone());
            let t = std::fs::metadata(store.dir(&id)?.join(history::BUILD_FILE))
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            if newest.as_ref().is_none_or(|(n, _)| t > *n) {
                newest = Some((t, name));
            }
        }
        let id = match newest {
            Some((_, name)) => BuildId::user(name),
            None => self.new_build(&json!({ "cmd": "newBuild" }))?,
        };
        self.default_build = Some(id.name.clone());
        Ok(id)
    }

    /// Makes a new user build from a `loadXml`, `loadCode` or `newBuild`
    /// request, under a name no build has
    fn new_build(&mut self, req: &J) -> R<BuildId> {
        let mut s = Dispatcher::typed(self.data);
        let a: J =
            serde_json::from_str(&s.request(&req.to_string())?).map_err(|e| e.to_string())?;
        if let Some(e) = a.get("error") {
            return Err(e.as_str().unwrap_or_default().to_string());
        }
        let save: J =
            serde_json::from_str(&s.request(r#"{"cmd":"saveXml"}"#)?).map_err(|e| e.to_string())?;
        let xml = save["ok"]["xml"]
            .as_str()
            .ok_or("saveXml gave no XML")?
            .to_string();
        let state: J =
            serde_json::from_str(&s.request(r#"{"cmd":"state"}"#)?).map_err(|e| e.to_string())?;
        let name = req["name"]
            .as_str()
            .filter(|n| !n.trim().is_empty())
            .or(state["ok"]["name"].as_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or("New build");
        let base = history::dir_name(name);
        let store = self.agent.store_mut();
        let taken = |n: &str| {
            n == AGENTS_DIR
                || n == crate::store::TRASH_DIR
                || store.dir(&BuildId::user(n)).is_ok_and(|d| d.exists())
        };
        let mut pick = base.clone();
        let mut k = 2;
        while taken(&pick) {
            pick = format!("{base} ({k})");
            k += 1;
        }
        let id = BuildId::user(pick);
        let label = match req["cmd"].as_str() {
            Some("newBuild") => "new build",
            _ => "import",
        };
        store.add(&id, &xml, label)?;
        Ok(id)
    }

    /// Answers a client's message
    fn ui(&mut self, client: u64, text: &str) -> J {
        let msg: J = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => return json!({ "error": format!("not JSON: {e}") }),
        };
        let mut answer = match self.ui_answer(client, &msg) {
            Ok(v) => v,
            Err(e) => json!({ "error": e }),
        };
        answer["id"] = msg["id"].clone();
        answer
    }

    fn ui_answer(&mut self, client: u64, msg: &J) -> R<J> {
        let build = self
            .clients
            .get(&client)
            .and_then(|c| c.build.clone())
            .ok_or("no build is open")?;
        if let Some(req) = msg.get("req") {
            let c = req["cmd"].as_str().unwrap_or_default();
            if OPENS_A_BUILD.contains(&c) {
                let id = self.new_build(req)?;
                self.open(client, id.clone())?;
                return Ok(json!({ "ok": { "build": ui_name(&id) } }));
            }
            let mut a = self.agent.engine(&build, req);
            a["version"] = json!(self.agent.version(&build).unwrap_or_default());
            return Ok(a);
        }
        let what = msg["host"]
            .as_str()
            .ok_or("a message is {\"req\": ...} or {\"host\": ...}")?;
        let req = &msg["args"];
        let ok = match what {
            "builds" => {
                let mut list = Vec::new();
                for name in self.agent.store().list(&Owner::User)? {
                    list.push(json!(name));
                }
                json!({ "builds": list, "current": ui_name(&build) })
            }
            "steps" => self.agent.steps(&build)?,
            "history" => {
                let n = req["n"].as_u64().unwrap_or(50).min(500) as usize;
                let store = self.agent.store();
                let versions: Vec<J> = store
                    .log(&build, n)?
                    .into_iter()
                    .map(|e| {
                        json!({
                            "version": e.version,
                            "time": e.time,
                            "label": e.label,
                            "source": e.source,
                            "trailers": e.trailers.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                json!({
                    "build": ui_name(&build),
                    "enabled": store.has_history(),
                    "pending": store.is_pending(&build),
                    "versions": versions,
                })
            }
            "restore" => {
                let v = req["version"].as_str().ok_or("restore names a version")?;
                let a = self.agent.restore_by_user(&build, v);
                if let Some(e) = a.get("error") {
                    return Err(e.as_str().unwrap_or_default().to_string());
                }
                a["ok"].clone()
            }
            "open" => {
                let name = req["build"].as_str().ok_or("open names a build")?;
                let id = parse_name(name)?;
                if self.agent.store().read(&id)?.is_none() {
                    return Err(format!("no build {name}"));
                }
                self.open(client, id.clone())?;
                json!({ "build": ui_name(&id), "version": self.agent.version(&id)? })
            }
            other => return Err(format!("no host request {other:?}")),
        };
        Ok(json!({ "ok": ok }))
    }

    /// Shows a build in a client
    fn open(&mut self, client: u64, id: BuildId) -> R<()> {
        let version = self.agent.version(&id)?;
        if let Some(c) = self.clients.get_mut(&client) {
            c.build = Some(id.clone());
        }
        self.send(
            client,
            &json!({ "event": "open", "build": ui_name(&id), "version": version }),
        );
        Ok(())
    }
}
