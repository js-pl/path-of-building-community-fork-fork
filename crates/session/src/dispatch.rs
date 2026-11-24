//! The engine API's commands, each answered by the typed engine where it
//! is ported, else with a "not ported" error.

use crate::typed::{NOT_PORTED, TypedData, TypedSession, UPSTREAM_ERROR};
use serde_json::{Value as J, json};

/// Commands that leave the build as it is (the reference session's and
/// the typed engine's alike); a host needs no `saveXml` after them
pub const READ_ONLY: &[&str] = &[
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

/// A build session answering JSON requests, as `Session::request` does
pub struct Dispatcher<'d> {
    data: &'d TypedData,
    typed: Option<TypedSession<'d>>,
    /// Why the typed engine last did not answer a request
    pub typed_error: Option<String>,
    /// Whether the typed sessions still know the shared item lists: not
    /// once a reference session held a build ([`crate::Paired`])
    shared_known: bool,
    /// Whether the typed engine answered the last request
    pub typed_answered: bool,
    /// With `record_replays`, what the last requests did to the build that a
    /// reference session must repeat to stay in step ([`crate::Paired`])
    pub record_replays: bool,
    pub replays: Vec<Replay>,
}

/// A request the typed engine answered that changed the build, as a
/// reference session repeats it
#[derive(Clone, Debug, PartialEq)]
pub enum Replay {
    /// An edit, as the reference replays it
    Edit(String),
    /// A build loaded or made: it replaces what the edits before did
    Load(String),
}

fn ok(v: J) -> String {
    json!({ "ok": v }).to_string()
}

impl<'d> Dispatcher<'d> {
    /// A session on the typed engine alone: commands it does not have are
    /// "not ported" errors. No build is open until one is loaded.
    pub fn typed(data: &'d TypedData) -> Dispatcher<'d> {
        Dispatcher {
            data,
            typed: None,
            shared_known: true,
            typed_error: None,
            typed_answered: false,
            record_replays: false,
            replays: Vec::new(),
        }
    }

    /// Leaves the build to a reference session: the typed engine holds no
    /// build and does not know the shared item lists until given them
    pub fn drop_build(&mut self) {
        self.typed = None;
        self.shared_known = false;
    }

    /// The shared item lists, as a reference session has them
    /// (`sharedLists`), for the typed build just opened
    pub fn set_shared(&mut self, v: &J) -> bool {
        let Some(t) = self.typed.as_mut() else {
            return false;
        };
        match t.set_shared(v) {
            Ok(()) => {
                self.shared_known = true;
                true
            }
            Err(_) => {
                t.forget_shared();
                false
            }
        }
    }

    /// Whether the typed engine holds the current build
    pub fn typed_active(&self) -> bool {
        self.typed.is_some()
    }

    /// The typed engine's build, when it holds one
    pub fn typed_build(&self) -> Option<&engine::build::Build<'d>> {
        self.typed.as_ref().map(TypedSession::build)
    }

    /// Answers one JSON request, as `Session::request` does
    pub fn request(&mut self, json: &str) -> Result<String, String> {
        self.typed_error = None;
        self.typed_answered = false;
        if let Some(r) = self.typed_request(json) {
            self.typed_answered = true;
            return Ok(r);
        }
        let cmd = serde_json::from_str::<J>(json)
            .ok()
            .and_then(|r| r["cmd"].as_str().map(String::from));
        Ok(self.not_ported(cmd.as_deref().unwrap_or("?")))
    }

    /// The error of a command the typed engine did not answer
    fn not_ported(&self, cmd: &str) -> String {
        let msg = match &self.typed_error {
            Some(e) => format!("{cmd}: {e}"),
            None if self.typed.is_none() => {
                format!("{cmd}: no build is open")
            }
            None => format!("{NOT_PORTED}: {cmd}"),
        };
        json!({ "error": msg }).to_string()
    }

    /// The typed engine's answer, where it has one
    fn typed_request(&mut self, json: &str) -> Option<String> {
        let data = self.data;
        let req: J = serde_json::from_str(json).ok()?;
        let name = req["name"].as_str();
        // main:OnFrame gives input to the front popup alone
        if req["cmd"] == "control"
            && let Some(path) = req["path"].as_str()
            && !path.starts_with("popup.")
            && self.typed.as_ref()?.popup_open()
        {
            let e = format!("control {path} is behind a popup");
            return Some(json!({ "error": e }).to_string());
        }
        match req["cmd"].as_str()? {
            "loadXml" => {
                let xml = req["xml"].as_str()?;
                self.load(data, xml, name, json)
            }
            "loadCode" => {
                let xml = decode_build_code(req["code"].as_str()?)?;
                self.load(data, &xml, name, json)
            }
            "state" => view(&mut self.typed_error, self.typed.as_ref()?.state()),
            "outputs" => Some(ok(self.typed.as_ref()?.outputs())),
            "calcs" => view(&mut self.typed_error, self.typed.as_mut()?.calcs()),
            "tree" => view(&mut self.typed_error, self.typed.as_ref()?.tree()),
            "treeState" => view(&mut self.typed_error, self.typed.as_ref()?.tree_state()),
            "nodePath" => view(
                &mut self.typed_error,
                self.typed.as_ref()?.node_path(id(&req)?),
            ),
            "masteryOptions" => view(
                &mut self.typed_error,
                self.typed.as_ref()?.mastery_options(id(&req)?),
            ),
            "treeSpecs" => Some(ok(self.typed.as_ref()?.tree_specs())),
            "sharedLists" => view(&mut self.typed_error, self.typed.as_ref()?.shared_lists()),
            "powerStats" => {
                self.typed.as_ref()?;
                Some(ok(TypedSession::power_stats()))
            }
            "clickNode" => {
                let id = id(&req)?;
                self.edit(json, |s| s.click_node(id))
            }
            "selectMastery" => {
                let (id, effect) = (id(&req)?, req["effect"].as_f64()?);
                self.edit(json, |s| s.select_mastery(id, effect))
            }
            "resetTree" => self.edit(json, |s| s.reset_tree()),
            "skills" => view(&mut self.typed_error, self.typed.as_ref()?.skills()),
            "gemList" => match (req["group"].as_f64(), req["slot"].as_f64()) {
                // Its calculations leave selections on the gems
                (Some(g), Some(k)) => self.edit(json, |s| s.gem_select_list(g, k)),
                _ => Some(ok(self.typed.as_ref()?.gem_list())),
            },
            "setMainSocketGroup" => {
                let i = req["index"].as_f64()?;
                self.edit(json, |s| s.set_main_socket_group(i))
            }
            "setSocketGroup" => {
                let i = req["index"].as_f64()?;
                let e = crate::typed::GroupEdit {
                    label: req["label"].as_str().map(String::from),
                    enabled: req["enabled"].as_bool(),
                    include_in_full_dps: req["includeInFullDPS"].as_bool(),
                    slot: req["slot"].as_str().map(String::from),
                };
                self.edit(json, |s| s.set_socket_group(i, e))
            }
            "addSocketGroup" => {
                let label = req["label"].as_str().map(String::from);
                self.edit(json, |s| s.add_socket_group(label))
            }
            "removeSocketGroup" => {
                let i = req["index"].as_f64()?;
                self.edit(json, |s| s.remove_socket_group(i))
            }
            "setGem" => {
                let (g, k) = (req["group"].as_f64()?, req["gem"].as_f64()?);
                let e = crate::typed::GemEdit {
                    name: req["name"].as_str().map(String::from),
                    level: req["level"].as_f64(),
                    quality: req["quality"].as_f64(),
                    quality_id: req["qualityId"].as_str().map(String::from),
                    enabled: req["enabled"].as_bool(),
                    count: req["count"].as_f64(),
                };
                self.edit(json, |s| s.set_gem(g, k, e))
            }
            "removeGem" => {
                let (g, k) = (req["group"].as_f64()?, req["gem"].as_f64()?);
                self.edit(json, |s| s.remove_gem(g, k))
            }
            "selectSkillSet" => {
                let id = req["id"].as_f64();
                self.edit(json, |s| s.select_skill_set(id))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("popup."))
                    && self.typed.as_ref()?.has_item_popup() =>
            {
                let req = req.clone();
                self.edit(json, |s| s.item_popup_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_item_editor_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.item_editor_control(&req))
            }
            "control"
                if req["dragTo"].is_string()
                    && req["path"]
                        .as_str()
                        .is_some_and(|p| p.starts_with("items.")) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.items_drag(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_shared_list_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.shared_list_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_spec_manage_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.spec_manage_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_set_manage_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.set_manage_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_build_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.build_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_skills_sockets_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.skills_sockets_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_gem_slot_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.gem_slot_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_group_detail_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.group_detail_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_group_list_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.group_list_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_skills_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.skills_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_items_tab_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.items_tab_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_item_list_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.item_list_control(&req))
            }
            "control"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_item_db_control) =>
            {
                let req = req.clone();
                self.edit(json, |s| s.item_db_control(&req))
            }
            "control" => {
                let req = req.clone();
                self.edit(json, |s| s.control(&req))
            }
            "controls" => {
                let paths: Vec<&str> = req["paths"]
                    .as_array()?
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Option<_>>()?;
                self.typed.as_ref()?.controls(&paths).map(ok)
            }
            "breakdown" => {
                let cell: Vec<usize> = req["cell"]
                    .as_array()?
                    .iter()
                    .map(|v| v.as_u64().map(|n| n as usize))
                    .collect::<Option<_>>()?;
                view(&mut self.typed_error, self.typed.as_mut()?.breakdown(&cell))
            }
            "items" => view(&mut self.typed_error, self.typed.as_mut()?.items()),
            "copySocketGroup" => {
                let i = req["index"].as_f64()?;
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.copy_socket_group(i),
                )
            }
            "pasteSocketGroup" => {
                let text = req["text"].as_str()?.to_string();
                self.edit(json, |s| s.paste_socket_group(&text))
            }
            "equipItem" => {
                let slot = req["slot"].as_str()?.to_string();
                let id = req["id"].as_f64().unwrap_or(0.0);
                self.edit(json, |s| s.equip_item(&slot, id))
            }
            "deleteItem" => {
                let id = req["id"].as_f64()?;
                self.edit(json, |s| s.delete_item(id))
            }
            "addItem" => {
                let text = req["text"].as_str()?.to_string();
                let slot = req["slot"].as_str().map(String::from);
                self.edit(json, |s| s.add_item_text(&text, slot.as_deref()))
            }
            "editItem" => {
                let (id, text) = (req["id"].as_f64()?, req["text"].as_str()?.to_string());
                self.edit(json, |s| s.edit_item(id, &text))
            }
            "setDisplayItem" => {
                let (id, text) = (req["id"].as_f64(), req["text"].as_str().map(String::from));
                self.edit(json, |s| s.set_display_item(id, text.as_deref()))
            }
            "saveDisplayItem" => self.edit(json, |s| s.save_display_item()),
            "itemDb" => {
                let rare = req["kind"].as_str() == Some("rare");
                view(&mut self.typed_error, self.typed.as_ref()?.item_db(rare))
            }
            "selectItemSet" => {
                let id = req["id"].as_f64()?;
                self.edit(json, |s| s.select_item_set(id))
            }
            "selectTreeSpec" => {
                let i = req["index"].as_f64()?;
                self.edit(json, |s| s.select_tree_spec(i))
            }
            "selectConfigSet" => {
                let id = req["id"].as_f64()?;
                self.edit(json, |s| s.select_config_set(id))
            }
            "saveXml" => self.edit(json, |s| Ok(json!({ "xml": s.save_xml()? }))),
            "saveCode" => self.edit(json, |s| Ok(json!({ "code": s.save_code()? }))),
            "config" => view(&mut self.typed_error, self.typed.as_ref()?.config()),
            "undo" => self.edit(json, |s| s.undo()),
            "newBuild" => self.open(TypedSession::new_build(data), json),
            "popupButton" if self.typed.as_ref()?.has_item_popup() => {
                let name = req["name"].as_str()?.to_string();
                self.edit(json, |s| s.item_popup_button(&name))
            }
            "closePopup" if self.typed.as_ref()?.has_item_popup() => {
                self.edit(json, |s| Ok(s.close_item_popup()))
            }
            "craftItem" => self.edit(json, |s| s.craft_item()),
            "displayItem" => view(&mut self.typed_error, self.typed.as_mut()?.display_item()),
            "controlTooltip"
                if req["path"]
                    .as_str()
                    .is_some_and(|p| p == "items.uniqueDB" || p == "items.rareDB") =>
            {
                let path = req["path"].as_str()?.to_string();
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.item_db_tooltip(&path, index),
                )
            }
            "controlTooltip"
                if req["path"]
                    .as_str()
                    .is_some_and(|p| p == "items.sharedItemList") =>
            {
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.shared_list_tooltip(index),
                )
            }
            "controlTooltip"
                if req["path"]
                    .as_str()
                    .is_some_and(TypedSession::is_build_tooltip) =>
            {
                let path = req["path"].as_str()?.to_string();
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.build_control_tooltip(&path, index),
                )
            }
            "controlTooltip"
                if req["path"].as_str().is_some_and(|p| {
                    crate::typed::gem_slots_count_path(p)
                        && self.typed.as_ref().is_some_and(|t| t.gem_slot_exists(p))
                }) =>
            {
                Some(ok(self.typed.as_ref()?.gem_slot_count_tooltip()))
            }
            "controlTooltip"
                if req["path"].as_str().is_some_and(|p| {
                    crate::typed::gem_slots_enable_path(p)
                        && self.typed.as_ref().is_some_and(|t| t.gem_slot_exists(p))
                }) =>
            {
                let path = req["path"].as_str()?.to_string();
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.gem_slot_enable_tooltip(&path),
                )
            }
            "controlTooltip"
                if req["path"].as_str().is_some_and(|p| {
                    crate::typed::gem_slots_quality_path(p)
                        && self.typed.as_ref().is_some_and(|t| t.gem_slot_exists(p))
                }) =>
            {
                let path = req["path"].as_str()?.to_string();
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.gem_slot_quality_tooltip(&path),
                )
            }
            "controlTooltip" if req["path"].as_str() == Some("skills.groupSlot") => {
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.group_slot_tooltip(index),
                )
            }
            "controlTooltip" if req["path"].as_str() == Some("skills.groupList") => {
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.group_list_tooltip(index),
                )
            }
            "controlTooltip" if req["path"].as_str() == Some("skills.defaultLevel") => {
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.skills_control_tooltip(index),
                )
            }
            "controlTooltip" if req["path"].as_str() == Some("items.setSelect") => {
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.item_set_select_tooltip(index),
                )
            }
            "controlTooltip" if req["path"].as_str().is_some_and(|p| p == "items.itemList") => {
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.item_list_tooltip(index),
                )
            }
            "controlTooltip"
                if req["path"].as_str().is_some_and(|p| {
                    p.starts_with("configTab.") && p.matches('.').count() == 2
                }) =>
            {
                let name = req["path"].as_str()?["configTab.".len()..].to_string();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.custom_block_tooltip(&name),
                )
            }
            "controlTooltip"
                if req["path"].as_str().is_some_and(|p| p.starts_with("tree."))
                    && (req["path"] != "tree.specSelect" || req["index"].is_null()) =>
            {
                let name = req["path"].as_str()?["tree.".len()..].to_string();
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.tree_tooltip(&name),
                )
            }
            "controlTooltip"
                if req["path"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("party.")) =>
            {
                let name = req["path"].as_str()?["party.".len()..].to_string();
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.party_tooltip(&name),
                )
            }
            "controlTooltip" if self.typed.as_ref()?.has_item_popup() => {
                let path = req["path"].as_str()?.to_string();
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.item_popup_tooltip(&path, index),
                )
            }
            "popupButton" => {
                // The typed session's popups are the class change popups,
                // whose only button with a name is Continue's
                let name = req["name"].as_str().unwrap_or("nil").to_string();
                if !self.typed.as_ref()?.has_class_popup() || name != "confirm" {
                    let e = format!("no popup button {name}");
                    return Some(json!({ "error": e }).to_string());
                }
                self.edit(json, |s| s.popup_button())
            }
            "closePopup" => self.edit(json, |s| Ok(s.close_popup())),
            "nodePower" => {
                let index = req["index"].as_f64().unwrap_or(1.0);
                let cluster: Option<Vec<u32>> = req["cluster"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u32))
                        .collect()
                });
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.node_power(index, cluster),
                )
            }
            // Notes, party and tooltips
            "notes" => Some(ok(self.typed.as_ref()?.notes())),
            "party" => {
                let r = self.typed.as_ref()?.party();
                self.view(r)
            }
            "setNotes" => {
                let text = req["text"].as_str()?.to_string();
                self.edit(json, |s| s.set_notes(&text))
            }
            "socketGroupTooltip" => {
                let i = req["index"].as_f64()?;
                let r = self.typed.as_ref()?.socket_group_tooltip(i);
                self.view(r)
            }
            "nodeTooltip" => {
                let r = self.typed.as_mut()?.node_tooltip(id(&req)?);
                self.view(r)
            }
            "itemTooltip" => {
                let (i, slot) = (req["id"].as_f64()?, req["slot"].as_str());
                let r = self.typed.as_mut()?.item_tooltip(i, slot);
                self.view(r)
            }
            "previewItem" => {
                let text = req["text"].as_str()?;
                let r = self.typed.as_mut()?.preview_item(text);
                self.view(r)
            }
            _ => None,
        }
    }

    /// A view of the typed build; why it cannot give one is kept
    fn view(&mut self, r: crate::typed::Result<J>) -> Option<String> {
        match r {
            Ok(v) => Some(ok(v)),
            Err(e) if e.starts_with(UPSTREAM_ERROR) => Some(upstream_error(&e)),
            Err(e) => {
                self.typed_error = Some(e);
                None
            }
        }
    }

    /// A request that changes the typed build, recorded for a reference
    /// session with `record_replays`. A typed edit that fails leaves the
    /// build as far as the edit got.
    fn edit(
        &mut self,
        json: &str,
        f: impl FnOnce(&mut TypedSession<'d>) -> crate::typed::Result<J>,
    ) -> Option<String> {
        // What a reference replays, as the build was before
        let replay = serde_json::from_str::<J>(json)
            .ok()
            .and_then(|req| self.typed.as_ref()?.reference_replay(&req));
        let r = f(self.typed.as_mut()?);
        match r {
            // Upstream fails the same way, before changing the build
            Err(e) if e.starts_with(UPSTREAM_ERROR) => Some(upstream_error(&e)),
            Ok(v) => {
                if self.record_replays
                    && let Some(r) = replay
                {
                    self.replays.push(Replay::Edit(r.to_string()));
                }
                Some(ok(v))
            }
            Err(e) => {
                self.typed_error = Some(e);
                None
            }
        }
    }

    fn load(
        &mut self,
        data: &'d TypedData,
        xml: &str,
        name: Option<&str>,
        json: &str,
    ) -> Option<String> {
        self.open(TypedSession::load(data, xml, name), json)
    }

    /// A build loaded or made by a request
    fn open(
        &mut self,
        session: crate::typed::Result<TypedSession<'d>>,
        json: &str,
    ) -> Option<String> {
        match session {
            Ok(mut s) => {
                if let Some(prev) = &self.typed {
                    if prev.has_item_popup() {
                        // Its buttons act on the items tab it came from
                        self.typed = None;
                        self.shared_known = false;
                        self.typed_error = Some(format!(
                            "{}: a load with an item popup open",
                            crate::typed::NOT_PORTED
                        ));
                        return None;
                    }
                    s.keep_popups(prev);
                    s.keep_shared(prev);
                } else if !self.shared_known {
                    // A reference session held the builds since: their
                    // lists, until it gives them (`set_shared`)
                    s.forget_shared();
                }
                self.typed = Some(s);
                if self.record_replays {
                    self.replays.push(Replay::Load(json.to_string()));
                }
                Some(ok(json!([])))
            }
            Err(e) => {
                // The open build stays, so a load can be retried once
                // missing data files are added
                self.typed_error = Some(e);
                None
            }
        }
    }
}

/// A view's answer; why the typed engine has none
fn view(error: &mut Option<String>, r: crate::typed::Result<J>) -> Option<String> {
    match r {
        Err(e) if e.starts_with(UPSTREAM_ERROR) => Some(upstream_error(&e)),
        r => r.map_err(|e| *error = Some(e)).ok().map(ok),
    }
}

/// The error response upstream gives, of an [`UPSTREAM_ERROR`]
fn upstream_error(e: &str) -> String {
    json!({ "error": &e[UPSTREAM_ERROR.len()..] }).to_string()
}

/// A request's node id
fn id(req: &J) -> Option<u32> {
    req["id"].as_f64().map(|n| n as u32)
}

/// A build code's XML: URL-safe base64 of a zlib stream, as the import tab
/// reads it
pub fn decode_build_code(code: &str) -> Option<String> {
    let mut bits = 0u32;
    let mut n = 0;
    let mut out = Vec::new();
    for c in code.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        bits = (bits << 6) | v as u32;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
        }
    }
    let xml = miniz_oxide::inflate::decompress_to_vec_zlib(&out).ok()?;
    let xml = String::from_utf8(xml).ok()?;
    (!xml.is_empty()).then_some(xml)
}
