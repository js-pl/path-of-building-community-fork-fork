//! The tree tab's Manage Passive Trees popup (`TreeTab:OpenSpecManagePopup`,
//! `PassiveSpecListControl`, opened by the Items tab's `specButton`): the
//! specs' list with Copy, Delete, Rename and New, the name popup
//! (`RenameSpec`), the delete confirmation and a double click to activate a
//! spec. The list is passed unnamed: it is `popup.1`; the Import Tree,
//! Export Tree and Done buttons have no name either.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::{Deletion, ItemPopup};
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::spec::PassiveSpec;
use engine::tree::NodeType;
use serde_json::{Value as J, json};
use std::sync::Arc;

/// The popup's list selection (`selIndex`; `selValue` is the spec there)
#[derive(Clone, Debug, Default)]
pub struct ManageSpecs {
    sel: Option<usize>,
}

/// The name popup of a spec (`RenameSpec(spec, title, addOnName)`)
#[derive(Clone, Debug)]
pub struct RenameSpec {
    title: &'static str,
    /// The spec renamed, or `None` for the new one waiting in
    /// `pending_spec` (added to the list on Save)
    index: Option<usize>,
    buf: String,
    save_enabled: bool,
}

/// `treeVersions[v].display`
fn version_display(v: &str) -> String {
    let parts: Vec<&str> = v.split('_').collect();
    let base = parts.iter().take(2).copied().collect::<Vec<_>>().join(".");
    if parts.len() > 2 {
        format!("{base} ({})", parts[2..].join(" "))
    } else {
        base
    }
}

/// `PassiveSpec:CountAllocNodes()`'s points used
fn points_used(spec: &PassiveSpec) -> usize {
    spec.alloc_nodes
        .iter()
        .filter(|(_, i)| {
            let n = spec.tree_node(**i);
            !matches!(n.kind, NodeType::ClassStart | NodeType::AscendClassStart)
                && n.ascendancy_name.is_none()
        })
        .count()
}

/// `curClassName`, or `curAscendClassName` but "None"
fn class_label(spec: &PassiveSpec) -> String {
    let class = spec
        .cur_class_id
        .and_then(|id| spec.tree.classes.iter().find(|(i, _)| *i == id))
        .map(|(_, c)| c);
    let ascend = class.and_then(|c| {
        let id = spec.cur_ascend_class_id.unwrap_or(0);
        c.ascendancies
            .iter()
            .find(|(i, _)| *i == id && id != 0)
            .map(|(_, a)| a.name.to_string())
    });
    ascend
        .or_else(|| class.map(|c| c.name.to_string()))
        .unwrap_or_default()
}

impl<'d> TypedSession<'d> {
    pub fn is_spec_manage_control(path: &str) -> bool {
        path == "items.specButton"
    }

    /// `control` of the Items tab's Manage... button of the trees
    pub fn spec_manage_control(&mut self, req: &J) -> Result<J> {
        if req["click"].as_bool() != Some(true) {
            return Err(format!("{NOT_PORTED}: control items.specButton"));
        }
        if self.class_popups_open() > 0 {
            return Err(format!(
                "{NOT_PORTED}: managing trees with a class change popup open"
            ));
        }
        self.editor
            .popups
            .insert(0, ItemPopup::ManageSpecs(Box::default()));
        self.refresh_item_popups();
        Ok(json!([]))
    }

    /// The specs in the list's order (`treeTab.specList`)
    fn spec_list(&self) -> Vec<&PassiveSpec<'d>> {
        if self.other_specs.is_empty() {
            return vec![&*self.build.spec];
        }
        self.other_specs
            .iter()
            .map(|s| s.as_deref().unwrap_or(&*self.build.spec))
            .collect()
    }

    /// `describePopups` of Manage Passive Trees
    pub(super) fn describe_manage_specs(&self, p: &ManageSpecs) -> J {
        let latest = self.data.game.latest_tree_version();
        let specs = self.spec_list();
        let rows: Vec<J> = specs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let version = if &*s.tree.version != latest {
                    format!("[{}] ", version_display(&s.tree.version))
                } else {
                    String::new()
                };
                let current = if i + 1 == self.specs.0 {
                    "  ^9(Current)"
                } else {
                    ""
                };
                json!(format!(
                    "{version}{} ({}, {} points){current}",
                    s.title.as_deref().unwrap_or("Default"),
                    class_label(s),
                    points_used(s)
                ))
            })
            .collect();
        let selected = p.sel.is_some();
        let button = |name: &str, label: &str, enabled: bool| {
            json!({
                "class": "ButtonControl", "enabled": enabled, "label": label,
                "path": format!("popup.1.{name}"), "shown": true,
            })
        };
        let mut list = json!({
            "class": "PassiveSpecListControl", "enabled": true, "list": rows,
            "path": "popup.1", "shown": true,
        });
        if let Some(i) = p.sel {
            list["sel"] = num(i as f64);
        }
        json!({
            "title": "Manage Passive Trees",
            "lines": [],
            "buttons": [],
            "controls": [
                button("new", "New", true),
                button("rename", "Rename", selected),
                button("copy", "Copy", selected),
                button("delete", "Delete", selected && specs.len() > 1),
                list,
            ],
        })
    }

    /// `describePopups` of the name popup
    pub(super) fn describe_rename_spec(&self, p: &RenameSpec) -> J {
        json!({
            "title": p.title,
            "lines": [],
            "buttons": [
                { "name": "cancel", "label": "Cancel", "enabled": true },
                { "name": "save", "label": "Save", "enabled": p.save_enabled },
            ],
            "controls": [{
                "class": "EditControl", "enabled": true, "path": "popup.edit",
                "shown": true, "text": p.buf,
            }],
        })
    }

    /// `RenameSpec(spec, title, addOnName)`
    fn open_rename_spec(&mut self, title: &'static str, index: Option<usize>) {
        let spec_title = match index {
            Some(i) => self.spec_list().get(i - 1).and_then(|s| s.title.clone()),
            None => self.pending_spec.as_ref().and_then(|s| s.title.clone()),
        };
        self.editor.popups.insert(
            0,
            ItemPopup::RenameSpec(Box::new(RenameSpec {
                title,
                index,
                buf: spec_title.unwrap_or_default(),
                save_enabled: false,
            })),
        );
    }

    /// `control` of the popup's list or its buttons
    pub(super) fn manage_specs_control(
        &mut self,
        p: &mut ManageSpecs,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let click = req["click"].as_bool() == Some(true);
        let count = self.spec_list().len();
        let missing = || "attempt to index a nil value".to_string();
        match (name, click) {
            ("1", _) => {
                if let Some(sel) = req["sel"].as_f64() {
                    // `SelectIndex`: a missing row leaves `selIndex`
                    if !(sel >= 1.0 && sel.fract() == 0.0 && (sel as usize) <= count) {
                        return Ok(());
                    }
                    let i = sel as usize;
                    p.sel = Some(i);
                    // `OnSelClick`: a double click activates the spec
                    if click && req["doubleClick"].as_bool() == Some(true) && i != self.specs.0 {
                        self.select_tree_spec(i as f64)?;
                    }
                } else if let Some(key) = req["key"].as_str() {
                    match (key, p.sel) {
                        ("DELETE", Some(i)) => self.delete_spec(i),
                        ("F2", Some(i)) => self.open_rename_spec("Rename Tree", Some(i)),
                        _ => {}
                    }
                } else {
                    return Err(format!("{NOT_PORTED}: control popup.{name}"));
                }
            }
            ("1.new", true) => {
                // A spec of the latest tree with the build's class
                let latest = self.data.game.latest_tree_version().to_string();
                let tree = self.data.tree(&latest)?;
                let mut spec = PassiveSpec::new(tree, &self.data.parser)?;
                let cur = &self.build.spec;
                let (class, ascend, secondary) = (
                    cur.cur_class_id.ok_or(missing())?,
                    cur.cur_ascend_class_id.ok_or(missing())?,
                    cur.cur_secondary_ascend_class_id,
                );
                spec.select_class(class)?;
                spec.select_ascend_class(ascend)?;
                spec.select_secondary(secondary)?;
                self.pending_spec = Some(Arc::new(spec));
                self.open_rename_spec("New Tree", None);
            }
            ("1.copy", true) => {
                // A new spec restored from the selected one's undo state:
                // `CreateUndoState` reads no bloodline, so the copy has none
                let i = p.sel.ok_or_else(missing)?;
                let src = self.spec_list()[i - 1];
                if !src.hash_overrides.is_empty() {
                    return Err(format!("{NOT_PORTED}: copying a tree with tattoos"));
                }
                let (Some(class), Some(ascend)) = (src.cur_class_id, src.cur_ascend_class_id)
                else {
                    return Err(format!("{NOT_PORTED}: copying a tree without a class"));
                };
                let hash_list: Vec<u32> = src.alloc_nodes.iter().map(|(k, _)| k).collect();
                let masteries: Vec<(u32, f64)> = src
                    .mastery_selections
                    .iter()
                    .map(|(k, v)| (k, *v))
                    .collect();
                let (title, jewels) = (src.title.clone(), src.jewels.clone());
                let tree = self.data.tree(&src.tree.version)?;
                let mut spec = PassiveSpec::new(tree, &self.data.parser)?;
                spec.title = title;
                spec.jewels = jewels;
                spec.import_from_node_list(class, ascend, None, &hash_list, &masteries)?;
                spec.build_cluster_jewel_graphs(&self.build.items, &self.data.game.items)?;
                self.pending_spec = Some(Arc::new(spec));
                self.open_rename_spec("Copy Tree", None);
            }
            ("1.rename", true) => {
                let i = p.sel.ok_or_else(missing)?;
                self.open_rename_spec("Rename Tree", Some(i));
            }
            ("1.delete", true) => {
                // `OnSelDelete(nil, nil)` with nothing selected
                let i = p.sel.ok_or_else(missing)?;
                self.delete_spec(i);
            }
            _ => return Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
        Ok(())
    }

    /// `OnSelDelete`: a confirmation, while there is another spec
    fn delete_spec(&mut self, index: usize) {
        let specs = self.spec_list();
        if specs.len() < 2 {
            return;
        }
        let title = specs[index - 1]
            .title
            .clone()
            .unwrap_or_else(|| "Default".into());
        self.editor.popups.insert(
            0,
            ItemPopup::Confirm {
                title: "Delete Tree".into(),
                lines: vec![format!("Are you sure you want to delete '{title}'?")],
                action: Deletion::Spec { index },
            },
        );
    }

    /// The confirmation's Delete: the spec out of the list, and the one
    /// before it active if it was
    pub(super) fn confirm_delete_spec(&mut self, index: usize) -> Result<()> {
        let n = self.spec_list().len();
        if index < 1 || index > n || self.other_specs.is_empty() {
            return Err(format!("{NOT_PORTED}: deleting a missing tree"));
        }
        let active = index == self.specs.0;
        self.other_specs.remove(index - 1);
        self.specs.1.remove(index - 1);
        if self.specs.0 > index {
            self.specs.0 -= 1;
        }
        // The undo states of the specs after it move up
        let mut undo: std::collections::HashMap<usize, _> = Default::default();
        for (k, v) in self.undo.drain() {
            match k.cmp(&index) {
                std::cmp::Ordering::Less => {
                    undo.insert(k, v);
                }
                std::cmp::Ordering::Greater => {
                    undo.insert(k - 1, v);
                }
                std::cmp::Ordering::Equal => {}
            }
        }
        self.undo = undo;
        if active {
            // `SetActiveSpec(m_max(1, index - 1))` of the list without
            // it: the deleted spec's jewels go nowhere
            let to = index.saturating_sub(1).max(1);
            let next = self.other_specs[to - 1].take().ok_or("no spec")?;
            self.build.spec = next;
            self.specs.0 = to;
            if !self.build.items.item_order_list.is_empty() {
                self.populate_slots();
            }
            self.sync_loadouts()?;
            self.rebuild()?;
        }
        if let Some(m) = self.editor.popups.iter_mut().find_map(|p| match p {
            ItemPopup::ManageSpecs(m) => Some(m),
            _ => None,
        }) {
            m.sel = None;
        }
        self.ui.unsaved = true;
        Ok(())
    }

    /// `control` of the name popup's edit
    pub(super) fn rename_spec_control(
        &mut self,
        p: &mut RenameSpec,
        name: &str,
        req: &J,
    ) -> Result<()> {
        match (name, req["text"].as_str()) {
            ("edit", Some(t)) => {
                p.buf = t.to_string();
                // `buf:match("%S")` or nil: never `false`
                p.save_enabled = true;
                Ok(())
            }
            _ => Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
    }

    /// The name popup's Save
    pub(super) fn rename_spec_save(&mut self, p: &RenameSpec) -> Result<()> {
        self.ui.unsaved = true;
        match p.index {
            Some(i) => {
                if i == self.specs.0 || self.other_specs.is_empty() {
                    Arc::make_mut(&mut self.build.spec).title = Some(p.buf.clone());
                } else if let Some(Some(s)) = self.other_specs.get_mut(i - 1) {
                    Arc::make_mut(s).title = Some(p.buf.clone());
                }
                if let Some(t) = self.specs.1.get_mut(i - 1) {
                    *t = p.buf.clone();
                }
            }
            None => {
                let mut spec = self.pending_spec.take().ok_or("no tree")?;
                Arc::make_mut(&mut spec).title = Some(p.buf.clone());
                if self.other_specs.is_empty() {
                    // The list held the active spec alone
                    self.other_specs.push(None);
                }
                self.other_specs.push(Some(spec));
                self.specs.1.push(p.buf.clone());
                let n = self.other_specs.len();
                if let Some(m) = self.editor.popups.iter_mut().find_map(|p| match p {
                    ItemPopup::ManageSpecs(m) => Some(m),
                    _ => None,
                }) {
                    m.sel = Some(n);
                }
            }
        }
        Ok(())
    }

    /// The name popup's Cancel: a new spec is never added
    pub(super) fn rename_spec_cancel(&mut self, p: &RenameSpec) {
        if p.index.is_none() {
            self.pending_spec = None;
        }
    }
}
