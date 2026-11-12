//! The engine API's commands, each answered by the typed engine where it
//! is ported, else by the translated session.
//!
//! The translation follows lazily: requests the typed engine answered that
//! change the build are queued and replayed on the translated session
//! before it answers anything. A command only the translation has that
//! changes the build leaves the typed build behind; until the next typed
//! load, the translation answers everything.

use crate::Session;
use crate::typed::{TypedData, TypedSession};
use serde_json::{Value as J, json};

/// Which implementation answers the commands both have
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer {
    Translated,
    Typed,
}

/// Commands of the translation that leave the build as it is
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
    translated: Session,
    data: Option<&'d TypedData>,
    prefer: Prefer,
    typed: Option<TypedSession<'d>>,
    /// Requests the typed engine answered that the translation has not seen
    pending: Vec<String>,
    /// Why the typed engine last gave a request to the translation
    pub typed_error: Option<String>,
}

fn ok(v: J) -> String {
    json!({ "ok": v }).to_string()
}

impl<'d> Dispatcher<'d> {
    pub fn new(translated: Session, data: Option<&'d TypedData>, prefer: Prefer) -> Dispatcher<'d> {
        Dispatcher {
            translated,
            data,
            prefer,
            typed: None,
            pending: Vec::new(),
            typed_error: None,
        }
    }

    pub fn translated(&self) -> &Session {
        &self.translated
    }

    /// Whether the typed engine holds the current build
    pub fn typed_active(&self) -> bool {
        self.typed.is_some()
    }

    /// Answers a request on the translation, whatever is preferred
    pub fn translated_request(&mut self, json: &str) -> Result<String, String> {
        self.sync()?
            .map_or_else(|| self.translated.request(json), Ok)
    }

    /// Answers one JSON request, as [`Session::request`] does
    pub fn request(&mut self, json: &str) -> Result<String, String> {
        if let Some(r) = self.typed_request(json) {
            return Ok(r);
        }
        self.sync()?.map_or_else(|| self.fallback(json), Ok)
    }

    /// Replays the queued requests on the translation; an error response
    /// stops it, the rest stays queued
    fn sync(&mut self) -> Result<Option<String>, String> {
        while let Some(req) = self.pending.first() {
            let r = self.translated.request(req)?;
            if r.starts_with("{\"error\"") {
                return Ok(Some(r));
            }
            self.pending.remove(0);
        }
        Ok(None)
    }

    fn fallback(&mut self, json: &str) -> Result<String, String> {
        let cmd = serde_json::from_str::<J>(json)
            .ok()
            .and_then(|r| r["cmd"].as_str().map(String::from));
        if !cmd.is_some_and(|c| READ_ONLY.contains(&c.as_str())) {
            self.typed = None;
        }
        self.translated.request(json)
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
            "state" => self.typed.as_ref()?.state().ok().map(ok),
            "outputs" => Some(ok(self.typed.as_ref()?.outputs())),
            "calcs" => self.typed.as_mut()?.calcs().ok().map(ok),
            "treeState" => self.typed.as_ref()?.tree_state().ok().map(ok),
            "nodePath" => self.typed.as_ref()?.node_path(id(&req)?).ok().map(ok),
            "masteryOptions" => self.typed.as_ref()?.mastery_options(id(&req)?).ok().map(ok),
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
                self.typed.as_mut()?.breakdown(&cell).ok().map(ok)
            }
            "nodePower" => {
                let index = req["index"].as_f64().unwrap_or(1.0);
                self.typed.as_ref()?.node_power(index).ok().map(ok)
            }
            _ => None,
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
            Ok(v) => {
                self.pending.push(json.to_string());
                Some(ok(v))
            }
            Err(e) => {
                self.typed = None;
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
        match TypedSession::load(data, xml, name) {
            Ok(s) => {
                self.typed = Some(s);
                // A load replaces whatever the queued requests did
                self.pending = vec![json.to_string()];
                Some(ok(json!([])))
            }
            Err(e) => {
                self.typed = None;
                self.typed_error = Some(e);
                None
            }
        }
    }
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
