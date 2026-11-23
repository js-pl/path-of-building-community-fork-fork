//! [`Paired`]: the typed engine with a reference [`Session`] (upstream on
//! the oracle's LuaJIT) kept in step, for the tests that compare the two.
//! Each build-changing request the typed engine answers is queued and
//! replayed on the reference before the reference answers anything; a
//! request the typed engine does not answer goes to the reference, which
//! then holds the build until the next load.

use crate::dispatch::Replay;
use crate::typed::TypedData;
use crate::{Dispatcher, Session};
use serde_json::{Value as J, json};

/// Commands of the reference session that leave the build as it is
pub(crate) const READ_ONLY: &[&str] = &[
    "state",
    "outputs",
    "controls",
    "tree",
    "treeState",
    "nodePath",
    "nodeTooltip",
    "masteryOptions",
    "treeSpecs",
    "sharedLists",
    "powerStats",
    "nodePower",
    "skills",
    "copySocketGroup",
    "gemList",
    "socketGroupTooltip",
    "items",
    "itemTooltip",
    "previewItem",
    "itemDb",
    "displayItem",
    "controlTooltip",
    "config",
    "calcs",
    "breakdown",
    "party",
    "notes",
    "saveXml",
    "saveCode",
];

/// Whether a request may change what outlives the build: a popup (opened
/// by a click, used or closed) or the shared item lists
pub(crate) fn outlives_build(json: &str) -> bool {
    let Ok(r) = serde_json::from_str::<J>(json) else {
        return true;
    };
    let path = r["path"].as_str().unwrap_or("");
    match r["cmd"].as_str() {
        Some("closePopup" | "popupButton") => true,
        Some("control") => {
            path.starts_with("popup.")
                || path.starts_with("items.sharedItemList")
                || r["click"] == json!(true)
                || r["dragTo"].is_string()
        }
        _ => false,
    }
}

/// A typed [`Dispatcher`] and a reference [`Session`] in step
pub struct Paired<'d> {
    /// The typed engine
    typed: Dispatcher<'d>,
    reference: Session,
    /// Requests the typed engine answered that the reference has not seen
    pending: Vec<String>,
    /// Whether the reference holds the build (the typed engine gave it up)
    lost: bool,
    /// Whether the typed engine must be given the shared item lists at the
    /// next load
    shared_unknown: bool,
    /// Why the typed engine last gave a request to the reference
    pub typed_error: Option<String>,
    /// Each request the typed engine gave to the reference while it held
    /// the build or loading one: the command and why
    pub fallbacks: Vec<(String, String)>,
}

impl<'d> Paired<'d> {
    /// The typed engine on `data` with `reference`, whose empty build it
    /// does not hold yet
    pub fn new(reference: Session, data: &'d TypedData) -> Paired<'d> {
        let mut typed = Dispatcher::typed(data);
        typed.record_replays = true;
        Paired {
            typed,
            reference,
            pending: Vec::new(),
            lost: true,
            shared_unknown: false,
            typed_error: None,
            fallbacks: Vec::new(),
        }
    }

    /// The reference session
    pub fn reference(&self) -> &Session {
        &self.reference
    }

    /// Whether the typed engine holds the current build
    pub fn typed_active(&self) -> bool {
        !self.lost && self.typed.typed_active()
    }

    /// Answers one JSON request: the typed engine's answer, or the
    /// reference's where the typed engine has none
    pub fn request(&mut self, json: &str) -> Result<String, String> {
        let req = serde_json::from_str::<J>(json).unwrap_or_default();
        let cmd = req["cmd"].as_str().unwrap_or_default().to_string();
        let load = matches!(cmd.as_str(), "loadXml" | "loadCode" | "newBuild");
        let held = self.typed_active();
        if held || load {
            self.typed_error = None;
        }
        if held || load {
            let typed = &mut self.typed;
            let r = typed.request(json)?;
            let answered = typed.typed_answered;
            let error = typed.typed_error.clone();
            let replays = std::mem::take(&mut typed.replays);
            for replay in replays {
                self.queue(replay)?;
            }
            if answered {
                if load {
                    self.lost = false;
                }
                return Ok(r);
            }
            self.typed_error = error;
            let why = self
                .typed_error
                .clone()
                .unwrap_or_else(|| format!("{cmd}: not typed"));
            self.fallbacks.push((cmd.clone(), why));
        }
        if let Some(e) = self.sync()? {
            return Ok(e);
        }
        // A gem slot's list leaves its calculations' selections on the gems
        let gem_slot = cmd == "gemList" && !req["group"].is_null();
        if gem_slot || !READ_ONLY.contains(&cmd.as_str()) {
            self.typed.drop_build();
            self.lost = true;
            self.shared_unknown = true;
        }
        self.reference.request(json)
    }

    /// Queues what the typed engine did for the reference. A load replaces
    /// what the queued requests did to the build; the popups and the
    /// shared lists outlive it, so requests that may have changed them
    /// reach the reference first.
    fn queue(&mut self, replay: Replay) -> Result<(), String> {
        match replay {
            Replay::Edit(r) => self.pending.push(r),
            Replay::Load(r) => {
                if self.shared_unknown {
                    // The reference held the builds since: its lists
                    self.sync()?;
                    let lists = self.reference.request(r#"{"cmd":"sharedLists"}"#)?;
                    let v: J = serde_json::from_str(&lists).unwrap_or_default();
                    self.typed.set_shared(&v["ok"]);
                    self.shared_unknown = false;
                }
                if self.pending.iter().any(|r| outlives_build(r)) {
                    self.sync()?;
                }
                self.pending = vec![r];
            }
        }
        Ok(())
    }

    /// Answers a request on the reference. A save the typed engine
    /// answered just before a save asked of the reference is the same save:
    /// it is not replayed, so both have saved (and re-parsed their items)
    /// as often. So is a gem slot's list asked again just after, whose
    /// calculations leave selections on the gems.
    pub fn reference_request(&mut self, json: &str) -> Result<String, String> {
        let is_save = |r: &str| {
            serde_json::from_str::<J>(r)
                .ok()
                .is_some_and(|r| matches!(r["cmd"].as_str(), Some("saveXml" | "saveCode")))
        };
        let is_gem_list = |r: &str| {
            serde_json::from_str::<J>(r)
                .ok()
                .is_some_and(|r| r["cmd"] == "gemList" && !r["group"].is_null())
        };
        if (is_save(json) && self.pending.last().is_some_and(|p| is_save(p)))
            || (is_gem_list(json) && self.pending.last().is_some_and(|p| p == json))
        {
            self.pending.pop();
        }
        self.sync()?
            .map_or_else(|| self.reference.request(json), Ok)
    }

    /// Replays the queued requests on the reference; an error response
    /// stops it, the rest stays queued
    fn sync(&mut self) -> Result<Option<String>, String> {
        while let Some(req) = self.pending.first() {
            let r = self.reference.request(req)?;
            if r.starts_with("{\"error\"") {
                return Ok(Some(r));
            }
            self.pending.remove(0);
        }
        Ok(None)
    }
}
