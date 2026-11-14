//! The engine API's commands, each answered by the typed engine where it
//! is ported, else by the translated session (with the `translation`
//! feature) or with a "not ported" error.
//!
//! The translation follows lazily: requests the typed engine answered that
//! change the build are queued and replayed on the translated session
//! before it answers anything. A command only the translation has that
//! changes the build leaves the typed build behind; until the next typed
//! load, the translation answers everything.

#[cfg(feature = "translation")]
use crate::Session;
use crate::typed::{NOT_PORTED, TypedData, TypedSession, UPSTREAM_ERROR};
use serde_json::{Value as J, json};

/// Which implementation answers the commands both have
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer {
    Translated,
    Typed,
}

/// Commands of the translation that leave the build as it is
#[cfg(feature = "translation")]
const READ_ONLY: &[&str] = &[
    "state",
    "outputs",
    "controls",
    "tree",
    "treeState",
    "nodePath",
    "nodeTooltip",
    "masteryOptions",
    "treeSpecs",
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

/// A build session answering JSON requests, as [`Session::request`]
pub struct Dispatcher<'d> {
    /// The translation, if this session falls back to it
    #[cfg(feature = "translation")]
    translated: Option<Session>,
    data: Option<&'d TypedData>,
    prefer: Prefer,
    typed: Option<TypedSession<'d>>,
    /// Requests the typed engine answered that the translation has not seen
    #[cfg(feature = "translation")]
    pending: Vec<String>,
    /// Why the typed engine last gave a request to the translation
    pub typed_error: Option<String>,
    /// Each request the typed engine preferred gave to the translation
    /// while it held the build or loading one: the command and why
    pub fallbacks: Vec<(String, String)>,
}

fn ok(v: J) -> String {
    json!({ "ok": v }).to_string()
}

impl<'d> Dispatcher<'d> {
    #[cfg(feature = "translation")]
    pub fn new(translated: Session, data: Option<&'d TypedData>, prefer: Prefer) -> Dispatcher<'d> {
        Dispatcher {
            translated: Some(translated),
            data,
            prefer,
            typed: None,
            pending: Vec::new(),
            typed_error: None,
            fallbacks: Vec::new(),
        }
    }

    /// A session on the typed engine alone: commands it does not have are
    /// "not ported" errors. No build is open until one is loaded.
    pub fn typed(data: &'d TypedData) -> Dispatcher<'d> {
        Dispatcher {
            #[cfg(feature = "translation")]
            translated: None,
            data: Some(data),
            prefer: Prefer::Typed,
            typed: None,
            #[cfg(feature = "translation")]
            pending: Vec::new(),
            typed_error: None,
            fallbacks: Vec::new(),
        }
    }

    /// The translated session; panics on a session without one
    #[cfg(feature = "translation")]
    pub fn translated(&self) -> &Session {
        self.translated
            .as_ref()
            .expect("a dispatcher on the typed engine alone")
    }

    /// Whether the typed engine holds the current build
    pub fn typed_active(&self) -> bool {
        self.typed.is_some()
    }

    /// Whether commands the typed engine does not have go to the
    /// translation
    pub fn has_translation(&self) -> bool {
        #[cfg(feature = "translation")]
        return self.translated.is_some();
        #[cfg(not(feature = "translation"))]
        false
    }

    /// Answers a request on the translation, whatever is preferred. A save
    /// the typed engine answered just before a save asked of the
    /// translation is the same save: it is not replayed, so both have
    /// saved (and re-parsed their items) as often.
    #[cfg(feature = "translation")]
    pub fn translated_request(&mut self, json: &str) -> Result<String, String> {
        let is_save = |r: &str| {
            serde_json::from_str::<J>(r)
                .ok()
                .is_some_and(|r| matches!(r["cmd"].as_str(), Some("saveXml" | "saveCode")))
        };
        if is_save(json) && self.pending.last().is_some_and(|p| is_save(p)) {
            self.pending.pop();
        }
        self.sync()?
            .map_or_else(|| self.translated().request(json), Ok)
    }

    /// Answers one JSON request, as [`Session::request`] does
    pub fn request(&mut self, json: &str) -> Result<String, String> {
        let cmd = serde_json::from_str::<J>(json)
            .ok()
            .and_then(|r| r["cmd"].as_str().map(String::from))
            .unwrap_or_default();
        let load = cmd == "loadXml" || cmd == "loadCode" || cmd == "newBuild";
        let held = self.typed.is_some();
        if !self.has_translation() || held || load {
            self.typed_error = None;
        }
        if let Some(r) = self.typed_request(json) {
            return Ok(r);
        }
        if (held || load) && self.prefer == Prefer::Typed && self.has_translation() {
            let why = self
                .typed_error
                .clone()
                .unwrap_or_else(|| format!("{cmd}: not typed"));
            self.fallbacks.push((cmd, why));
        }
        self.sync()?.map_or_else(|| self.fallback(json), Ok)
    }

    /// Replays the queued requests on the translation; an error response
    /// stops it, the rest stays queued
    #[cfg(feature = "translation")]
    fn sync(&mut self) -> Result<Option<String>, String> {
        let Some(translated) = &self.translated else {
            return Ok(None);
        };
        while let Some(req) = self.pending.first() {
            let r = translated.request(req)?;
            if r.starts_with("{\"error\"") {
                return Ok(Some(r));
            }
            self.pending.remove(0);
        }
        Ok(None)
    }

    #[cfg(not(feature = "translation"))]
    fn sync(&mut self) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn fallback(&mut self, json: &str) -> Result<String, String> {
        let cmd = serde_json::from_str::<J>(json)
            .ok()
            .and_then(|r| r["cmd"].as_str().map(String::from));
        #[cfg(feature = "translation")]
        if let Some(translated) = &self.translated {
            if !cmd.as_deref().is_some_and(|c| READ_ONLY.contains(&c)) {
                self.typed = None;
            }
            return translated.request(json);
        }
        Ok(self.not_ported(cmd.as_deref().unwrap_or("?")))
    }

    /// The error of a command the typed engine did not answer, on a
    /// session without the translation
    fn not_ported(&self, cmd: &str) -> String {
        let msg = match &self.typed_error {
            Some(e) => format!("{cmd}: {e}"),
            None if self.typed.is_none() && self.data.is_some() => {
                format!("{cmd}: no build is open")
            }
            None => format!("{NOT_PORTED}: {cmd}"),
        };
        json!({ "error": msg }).to_string()
    }

    /// The typed engine's answer, where it has one
    fn typed_request(&mut self, json: &str) -> Option<String> {
        if self.prefer != Prefer::Typed {
            return None;
        }
        let data = self.data?;
        let req: J = serde_json::from_str(json).ok()?;
        let name = req["name"].as_str();
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
                (Some(g), Some(k)) => view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.gem_select_list(g, k),
                ),
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
            "controlTooltip" if req["path"].as_str().is_some_and(|p| p == "items.itemList") => {
                let index = req["index"].as_f64();
                view(
                    &mut self.typed_error,
                    self.typed.as_mut()?.item_list_tooltip(index),
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
                view(
                    &mut self.typed_error,
                    self.typed.as_ref()?.node_power(index),
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

    /// A view of the typed build; one it cannot give comes from the
    /// translation, with the reason kept
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

    /// A request that changes the typed build: queued for the translation.
    /// A typed edit that fails leaves the build to the translation.
    fn edit(
        &mut self,
        json: &str,
        f: impl FnOnce(&mut TypedSession<'d>) -> crate::typed::Result<J>,
    ) -> Option<String> {
        let r = f(self.typed.as_mut()?);
        match r {
            // The translation fails the same way, before changing the build
            Err(e) if e.starts_with(UPSTREAM_ERROR) => Some(upstream_error(&e)),
            Ok(v) => {
                #[cfg(feature = "translation")]
                if self.translated.is_some() {
                    self.pending.push(json.to_string());
                }
                let _ = json;
                Some(ok(v))
            }
            Err(e) => {
                // Without the translation the build stays, as far as the
                // edit got
                if self.has_translation() {
                    self.typed = None;
                }
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
                        self.typed_error = Some(format!(
                            "{}: a load with an item popup open",
                            crate::typed::NOT_PORTED
                        ));
                        return None;
                    }
                    s.keep_popups(prev);
                }
                self.typed = Some(s);
                // A load replaces whatever the queued requests did
                #[cfg(feature = "translation")]
                if self.translated.is_some() {
                    self.pending = vec![json.to_string()];
                }
                let _ = json;
                Some(ok(json!([])))
            }
            Err(e) => {
                // Without the translation the open build stays, so a load
                // can be retried once missing data files are added
                if self.has_translation() {
                    self.typed = None;
                }
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

/// The error response the translation gives, of an [`UPSTREAM_ERROR`]
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
