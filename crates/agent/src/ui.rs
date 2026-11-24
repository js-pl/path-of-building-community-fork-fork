//! What the user is looking at in the UI, as a host reports it: the build
//! a client shows, its tab and what is selected or hovered there. The
//! `ui_context` verb answers it within the caller's grant, and a request
//! that names no build goes to the build the UI shows when the grant has
//! it (docs/agent.md, "Live sync").

use crate::read::node_label;
use crate::text::{LINE_CAP, NAME_CAP, clean, untrusted};
use crate::{Agent, BuildId, Grant, Owner, R, Store, check_fields, cmd};
use serde_json::{Map, Value as J, json};

/// The UI's tabs a context may name
pub const TABS: &[&str] = &[
    "Import/Export",
    "Tree",
    "Skills",
    "Items",
    "Calcs",
    "Config",
    "Notes",
    "Party",
    "History",
    "Agents",
];

/// What a UI client shows, as the host passes it on
#[derive(Clone, Debug, PartialEq)]
pub struct UiView {
    /// The build shown
    pub build: BuildId,
    /// Its tab, one of [`TABS`]
    pub tab: Option<String>,
    /// What is selected or hovered: `{"node": id}`, `{"item": id}`,
    /// `{"slot": name}`, `{"group": n, "gem"?: n}`, checked by
    /// [`selection`]
    pub selection: Option<J>,
    /// Seconds since the client last reported
    pub age: u64,
    /// How many UI clients are connected
    pub clients: usize,
}

/// A selection from a client's report, with only the fields and types
/// a selection has; `None` for anything else
pub fn selection(v: &J) -> Option<J> {
    let o = v.as_object()?;
    let pick = |k: &str| o.get(k).filter(|v| !v.is_null());
    let sel = if let Some(n) = pick("node") {
        json!({ "node": n.as_u64()? })
    } else if let Some(i) = pick("item") {
        json!({ "item": i.as_u64()? })
    } else if let Some(s) = pick("slot") {
        let s = s.as_str()?;
        if s.is_empty() || s.chars().count() > 64 {
            return None;
        }
        json!({ "slot": s })
    } else {
        let g = pick("group")?.as_u64().filter(|g| *g >= 1)?;
        match pick("gem") {
            Some(m) => json!({ "group": g, "gem": m.as_u64().filter(|m| *m >= 1)? }),
            None => json!({ "group": g }),
        }
    };
    Some(sel)
}

impl Grant {
    /// Whether the grant covers a build: a user's build it names, or one in
    /// its agent directory
    pub fn covers(&self, id: &BuildId) -> bool {
        match &id.owner {
            Owner::User => self.builds.contains(&id.name),
            Owner::Agent(d) => *d == self.dir,
        }
    }
}

impl<'d, S: Store> Agent<'d, S> {
    /// What the UI shows now, or `None` when no client shows a build; a
    /// host sets it before each request
    pub fn set_ui(&mut self, view: Option<UiView>) {
        self.ui = view;
    }

    /// The build a request naming none goes to: the one the UI shows when
    /// the grant covers it, else the grant's first
    pub(crate) fn default_build(&self, grant: &Grant) -> Option<String> {
        self.ui
            .as_ref()
            .filter(|v| grant.covers(&v.build))
            .map(|v| grant.api_name(&v.build))
    }

    pub(crate) fn ui_context(&mut self, grant: &Grant, req: &J) -> R<J> {
        check_fields(req, &[])?;
        let Some(view) = self.ui.clone() else {
            return Ok(json!({
                "ui": false,
                "note": "no UI shows a build now",
            }));
        };
        let mut out = Map::new();
        out.insert("ui".into(), json!(true));
        out.insert("clients".into(), json!(view.clients));
        out.insert("reported".into(), json!(format!("{} s ago", view.age)));
        if !grant.covers(&view.build) {
            // Not even its name: a build outside the grant isn't told of
            out.insert("build".into(), J::Null);
            out.insert(
                "note".into(),
                json!("the UI shows a build this connection wasn't granted"),
            );
            return Ok(J::Object(out));
        }
        let name = grant.api_name(&view.build);
        out.insert("build".into(), json!(name));
        out.insert("tab".into(), json!(view.tab));
        out.insert("version".into(), json!(self.version(&view.build)?));
        let mut text = Map::new();
        if let Some(sel) = &view.selection {
            let s = self.describe_selection(&view.build, sel, &mut text)?;
            out.insert("selection".into(), s);
        } else {
            out.insert("selection".into(), J::Null);
        }
        if !text.is_empty() {
            let (k, v) = untrusted(text);
            out.insert(k, v);
        }
        out.insert(
            "names".into(),
            json!("as the other verbs name things: nodes `Name #id`, items by slot and base, groups by their index in `skills`"),
        );
        Ok(J::Object(out))
    }

    /// A selection with the engine's names; the author's text goes to
    /// `text`, which the answer marks untrusted
    fn describe_selection(&mut self, id: &BuildId, sel: &J, text: &mut Map<String, J>) -> R<J> {
        let data = self.data;
        self.with_open(id, |agent, o| {
            let s = &mut o.session;
            if let Some(n) = sel["node"].as_u64() {
                let tree = cmd(s, json!({ "cmd": "tree" }))?;
                let Some(node) = tree["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|x| x["id"].as_u64() == Some(n))
                else {
                    return Ok(
                        json!({ "node": format!("#{n}"), "note": "no such node in this tree" }),
                    );
                };
                let state = cmd(s, json!({ "cmd": "treeState" }))?;
                let allocated = state["alloc"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x.as_u64() == Some(n)));
                return Ok(json!({
                    "node": node_label(node),
                    "kind": node["kind"],
                    "allocated": allocated,
                }));
            }
            if sel.get("item").is_some() || sel.get("slot").is_some() {
                let items = cmd(s, json!({ "cmd": "items" }))?;
                let uniques = agent.unique_names(s)?.clone();
                let described = crate::read::describe_items(data, &items, &uniques, false);
                let item_id = match sel["slot"].as_str() {
                    Some(slot) => items["slots"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|x| x["name"].as_str() == Some(slot))
                        .and_then(|x| x["item"].as_u64()),
                    None => sel["item"].as_u64(),
                };
                let Some(item_id) = item_id.filter(|i| *i != 0) else {
                    return Ok(json!({ "slot": sel["slot"], "item": J::Null }));
                };
                let equipped = described["equipped"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|e| e["id"].as_u64() == Some(item_id));
                let mut o = Map::new();
                match equipped {
                    Some(e) => {
                        for k in [
                            "item",
                            "slot",
                            "id",
                            "base",
                            "rarity",
                            "unique",
                            "lines",
                            "unparsed_lines",
                        ] {
                            if let Some(v) = e.get(k) {
                                o.insert(k.into(), v.clone());
                            }
                        }
                        o.insert("equipped".into(), json!(true));
                    }
                    None => {
                        let Some(u) = described["unequipped"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|e| e["id"].as_u64() == Some(item_id))
                        else {
                            return Ok(json!({ "item": J::Null, "note": "no such item" }));
                        };
                        o.extend(u.as_object().cloned().unwrap_or_default());
                        o.insert("equipped".into(), json!(false));
                    }
                }
                if let Some(raw) = items["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|x| x["id"].as_u64() == Some(item_id))
                {
                    text.insert(
                        "item_name".into(),
                        json!(clean(
                            raw["name"].as_str().unwrap_or_default(),
                            NAME_CAP,
                            false
                        )),
                    );
                }
                return Ok(J::Object(o));
            }
            if let Some(g) = sel["group"].as_u64() {
                let skills = agent.skills_of(s)?;
                let raw = cmd(s, json!({ "cmd": "skills" }))?;
                let Some(group) = skills["groups"].get(g as usize - 1) else {
                    return Ok(json!({ "group": g, "note": "no such group" }));
                };
                let mut o = group.as_object().cloned().unwrap_or_default();
                o.insert(
                    "main".into(),
                    json!(skills["main_group"].as_u64() == Some(g)),
                );
                if let Some(m) = sel["gem"].as_u64() {
                    o.insert("gem".into(), json!(m));
                    o.insert("selected_gem".into(), group["gems"][m as usize - 1].clone());
                }
                let label = raw["groups"][g as usize - 1]["label"]
                    .as_str()
                    .unwrap_or_default();
                if !label.is_empty() {
                    text.insert("group_label".into(), json!(clean(label, LINE_CAP, false)));
                }
                return Ok(J::Object(o));
            }
            Ok(J::Null)
        })
    }
}
