//! The UI's side of a build on a host: the engine API's commands (the
//! session's JSON requests, which the web app speaks) run on the same open
//! session as the agents' verbs, so a build has one owner. A command that
//! changes the build's XML is one undo step made by the user, saved
//! through [`Store::user_edit`] (coalesced into commits), with a change
//! event like an agent's step.

use crate::read::short;
use crate::text::{LINE_CAP, clean};
use crate::write::stats;
use crate::{Agent, BuildId, Event, R, Store, Who, cmd};
use serde_json::{Value as J, json};
use session::dispatch::READ_ONLY;

/// Commands that replace the session's build: a host opens a new build
/// for them instead (nothing replaces a build)
pub const OPENS_A_BUILD: &[&str] = &["loadXml", "loadCode", "newBuild"];

impl<'d, S: Store> Agent<'d, S> {
    /// Answers the UI's engine API request `{"cmd": ...}` on a build:
    /// the session's answer, `{"ok": ...}` or `{"error": "..."}`. `undo`
    /// undoes the build's last step, whoever made it.
    pub fn engine(&mut self, id: &BuildId, req: &J) -> J {
        match self.engine_answer(id, req) {
            Ok(v) => v,
            Err(e) => json!({ "error": e }),
        }
    }

    fn engine_answer(&mut self, id: &BuildId, req: &J) -> R<J> {
        let c = req["cmd"]
            .as_str()
            .ok_or("a request names its command: {\"cmd\": ...}")?;
        if c == "undo" {
            let r = self.undo(Who::User, id, id.name.clone(), None)?;
            return Ok(json!({ "ok": r }));
        }
        if OPENS_A_BUILD.contains(&c) {
            return Err(format!("{c} opens a new build on the host"));
        }
        self.with_open(id, |agent, o| {
            if READ_ONLY.contains(&c) {
                let text = o.session.request(&req.to_string())?;
                return serde_json::from_str(&text).map_err(|e| e.to_string());
            }
            let before = stats(&mut o.session)?;
            let alloc = if c == "clickNode" {
                allocated(&mut o.session)
            } else {
                0
            };
            let text = o.session.request(&req.to_string())?;
            let answer: J = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let label = if c == "clickNode" {
                let now = allocated(&mut o.session);
                let n = req["id"].as_u64().unwrap_or_default();
                match now.cmp(&alloc) {
                    std::cmp::Ordering::Greater => {
                        format!("allocate #{n} ({} points)", now - alloc)
                    }
                    std::cmp::Ordering::Less => format!("deallocate #{n} ({} points)", alloc - now),
                    _ => format!("node #{n}"),
                }
            } else {
                label_of(req)
            };
            agent.finish(
                Who::User,
                id,
                id.name.clone(),
                o,
                before,
                label,
                vec![],
                false,
            )?;
            Ok(answer)
        })
    }

    /// Restores a version from the history as a new version, for the UI:
    /// committed at once (`Source: restore`)
    pub fn restore_by_user(&mut self, id: &BuildId, version: &str) -> J {
        match self.restore(Who::User, id, id.name.clone(), None, version) {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "error": e }),
        }
    }

    /// A build's version and its undo list, newest first, each step with
    /// its label and who made it
    pub fn steps(&mut self, id: &BuildId) -> R<J> {
        let o = self.open(id)?;
        let steps: Vec<J> = o
            .undo
            .iter()
            .rev()
            .map(|u| json!({ "label": u.label, "source": u.source }))
            .collect();
        Ok(json!({ "build": id.name, "version": o.version, "steps": steps }))
    }

    /// A build's version, opening it
    pub fn version(&mut self, id: &BuildId) -> R<u64> {
        Ok(self.open(id)?.version)
    }

    /// Whether a build is open
    pub fn is_open(&self, id: &BuildId) -> bool {
        self.open.contains_key(id)
    }

    /// Closes a build's session (its file is moved, say); its undo list
    /// and changes go with it
    pub fn close(&mut self, id: &BuildId) {
        self.open.remove(id);
    }

    /// The changes made since the last call, oldest first: what a host
    /// sends its clients as change events
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}

/// How many points the tree has allocated
fn allocated(s: &mut session::Dispatcher) -> usize {
    cmd(s, json!({ "cmd": "treeState" }))
        .ok()
        .and_then(|t| t["alloc"].as_array().map(Vec::len))
        .unwrap_or_default()
}

/// An undo step's label for a UI command
fn label_of(req: &J) -> String {
    let c = req["cmd"].as_str().unwrap_or_default();
    let label = match c {
        "control" => {
            let path = req["path"].as_str().unwrap_or_default();
            // Numbers and switches only: free text stays out of labels,
            // which agents read
            let value = ["text", "sel", "state", "val"]
                .iter()
                .find_map(|k| req.get(*k))
                .and_then(|v| match v {
                    J::String(s) => s.trim().parse::<f64>().ok().map(|_| s.trim().to_string()),
                    J::Number(_) | J::Bool(_) => Some(v.to_string()),
                    _ => None,
                });
            match value {
                Some(v) => format!("{path}: {v}"),
                None => path.to_string(),
            }
        }
        "setGem" => format!(
            "gem {} in group {}",
            req["gem"].as_u64().unwrap_or_default(),
            req["group"].as_u64().unwrap_or_default()
        ),
        "removeGem" => "remove gem".into(),
        "setSocketGroup" => "socket group".into(),
        "removeSocketGroup" => "remove socket group".into(),
        "pasteSocketGroup" => "paste socket group".into(),
        "setMainSocketGroup" => "main skill".into(),
        "selectMastery" => "mastery".into(),
        "resetTree" => "reset tree".into(),
        "equipItem" => format!("equip {}", req["slot"].as_str().unwrap_or_default()),
        "addItem" | "saveDisplayItem" | "craftItem" => "add item".into(),
        "editItem" => "edit item".into(),
        "deleteItem" => "delete item".into(),
        "setNotes" => "notes".into(),
        "selectTreeSpec" => "tree set".into(),
        "selectSkillSet" => "skill set".into(),
        "selectItemSet" => "item set".into(),
        "selectConfigSet" => "config set".into(),
        other => other.to_string(),
    };
    clean(&label, LINE_CAP, false)
}

/// A change event as JSON, for the clients
pub fn event_json(e: &Event, build: &str) -> J {
    json!({
        "event": "change",
        "build": build,
        "version": e.version,
        "label": e.label,
        "source": e.source,
        "commit": e.commit.as_deref().map(short),
    })
}
