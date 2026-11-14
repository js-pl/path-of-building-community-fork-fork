//! The Manage... popups of the item, skill and config sets
//! (`ItemsTab:OpenItemSetManagePopup`, `SkillsTab:OpenSkillSetManagePopup`,
//! `ConfigTab:OpenConfigSetManagePopup`, with `ItemSetListControl`,
//! `SkillSetListControl` and `ConfigSetListControl`): the sets' list with
//! Copy, Delete, Rename and New, the name popup (`RenameSet`), the delete
//! confirmation, a double click to activate a set, and an item set row's
//! tooltip (`AddItemSetTooltip`). The shared item set list beside the item
//! sets (`SharedItemSetListControl`) is empty: only dragging fills it. The
//! skill and config set popups pass their list unnamed: it is `popup.1`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::{Deletion, ItemPopup};
use super::tooltip::Tooltip;
use super::{NOT_PORTED, Result, TypedSession, num};
use serde_json::{Value as J, json};
use std::sync::Arc;

/// Which tab's sets
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SetKind {
    Item,
    Skill,
    Config,
}

impl SetKind {
    /// The list's name in the popup
    fn list(self) -> &'static str {
        match self {
            SetKind::Item => "setList",
            _ => "1",
        }
    }

    fn class(self) -> &'static str {
        match self {
            SetKind::Item => "ItemSetListControl",
            SetKind::Skill => "SkillSetListControl",
            SetKind::Config => "ConfigSetListControl",
        }
    }

    fn title(self) -> &'static str {
        match self {
            SetKind::Item => "Manage Item Sets",
            SetKind::Skill => "Manage Skill Sets",
            SetKind::Config => "Manage Config Sets",
        }
    }
}

/// A Manage... popup: its list's selection (`selIndex`, `selValue`)
#[derive(Clone, Debug)]
pub struct ManageSets {
    pub kind: SetKind,
    sel_index: Option<usize>,
    sel_value: Option<f64>,
    /// The shared item sets' list's selection
    pub shared_sel: super::shared_lists::ListSel,
}

impl ManageSets {
    /// The list's name in the popup
    pub fn kind_list(&self) -> &'static str {
        self.kind.list()
    }
}

/// The name popup of a set (`RenameSet(set, addOnName)`)
#[derive(Clone, Debug)]
pub struct RenameSet {
    kind: SetKind,
    id: f64,
    /// A new set, added to the list on Save, dropped on Cancel
    add_on_name: bool,
    /// Whether the set had a title (the popup's title)
    titled: bool,
    buf: String,
    save_enabled: bool,
}

impl<'d> TypedSession<'d> {
    /// The Manage... buttons of the Items, Skills and Configuration tabs
    pub fn is_set_manage_control(path: &str) -> bool {
        matches!(
            path,
            "items.setManage" | "skills.setManage" | "configTab.setManage"
        )
    }

    /// `control` of a Manage... button: the popup
    pub fn set_manage_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        if req["click"].as_bool() != Some(true) {
            return Err(format!("{NOT_PORTED}: control {path}"));
        }
        if self.class_popups_open() > 0 {
            return Err(format!(
                "{NOT_PORTED}: managing sets with a class change popup open"
            ));
        }
        let kind = match path {
            "items.setManage" => SetKind::Item,
            "skills.setManage" => SetKind::Skill,
            _ => SetKind::Config,
        };
        if kind == SetKind::Skill && self.build.skills.is_none() {
            return Err(format!("{NOT_PORTED}: a build without a skills tab"));
        }
        self.editor.popups.insert(
            0,
            ItemPopup::ManageSets(Box::new(ManageSets {
                kind,
                sel_index: None,
                sel_value: None,
                shared_sel: Default::default(),
            })),
        );
        self.refresh_item_popups();
        Ok(json!([]))
    }

    /// The sets' ids in the list's order
    fn set_order(&self, kind: SetKind) -> Vec<f64> {
        match kind {
            SetKind::Item => self.build.items.item_set_order_list.clone(),
            SetKind::Skill => self
                .build
                .skills
                .as_ref()
                .map_or(vec![], |s| s.skill_set_order_list.clone()),
            SetKind::Config => self.build.config.config_set_order_list.clone(),
        }
    }

    fn set_order_mut(&mut self, kind: SetKind) -> Option<&mut Vec<f64>> {
        match kind {
            SetKind::Item => Some(&mut self.build.items.item_set_order_list),
            SetKind::Skill => self
                .build
                .skills
                .as_mut()
                .map(|s| &mut s.skill_set_order_list),
            SetKind::Config => {
                Some(&mut Arc::make_mut(&mut self.build.config).config_set_order_list)
            }
        }
    }

    fn active_set(&self, kind: SetKind) -> f64 {
        match kind {
            SetKind::Item => self.build.items.active_item_set_id,
            SetKind::Skill => self
                .build
                .skills
                .as_ref()
                .map_or(0.0, |s| s.active_skill_set_id),
            SetKind::Config => self.build.config.active_config_set_id,
        }
    }

    /// A set's title: `None` for a missing set, `Some(None)` for no title
    fn set_title_opt(&self, kind: SetKind, id: f64) -> Option<Option<String>> {
        let k = id as i64;
        match kind {
            SetKind::Item => self.build.items.item_sets.get(&k).map(|s| s.title.clone()),
            SetKind::Skill => self
                .build
                .skills
                .as_ref()
                .and_then(|s| s.skill_sets.get(&k))
                .map(|s| s.title.clone()),
            SetKind::Config => self
                .build
                .config
                .config_sets
                .get(&k)
                .map(|s| s.title.clone()),
        }
    }

    fn set_title(&self, kind: SetKind, id: f64) -> String {
        self.set_title_opt(kind, id)
            .flatten()
            .unwrap_or_else(|| "Default".into())
    }

    fn set_exists(&self, kind: SetKind, id: f64) -> bool {
        self.set_title_opt(kind, id).is_some()
    }

    fn write_set_title(&mut self, kind: SetKind, id: f64, title: String) {
        let k = id as i64;
        match kind {
            SetKind::Item => {
                if let Some(s) = self.build.items.item_sets.get_mut(&k) {
                    s.title = Some(title);
                }
            }
            SetKind::Skill => {
                if let Some(s) = self
                    .build
                    .skills
                    .as_mut()
                    .and_then(|s| s.skill_sets.get_mut(&k))
                {
                    s.title = Some(title);
                }
            }
            SetKind::Config => {
                if let Some(s) = Arc::make_mut(&mut self.build.config)
                    .config_sets
                    .get_mut(&k)
                {
                    s.title = Some(title);
                }
            }
        }
    }

    fn remove_set(&mut self, kind: SetKind, id: f64) {
        let k = id as i64;
        match kind {
            SetKind::Item => {
                self.build.items.item_sets.remove(&k);
            }
            SetKind::Skill => {
                if let Some(s) = self.build.skills.as_mut() {
                    s.skill_sets.remove(&k);
                }
            }
            SetKind::Config => {
                Arc::make_mut(&mut self.build.config).config_sets.remove(&k);
            }
        }
    }

    /// A free set id, from 1
    fn free_set_id(&self, kind: SetKind) -> f64 {
        let mut id = 1.0;
        while self.set_exists(kind, id) {
            id += 1.0;
        }
        id
    }

    /// `SetActive...Set(id)`
    fn activate_set(&mut self, kind: SetKind, id: f64) -> Result<()> {
        match kind {
            SetKind::Item => self.select_item_set(id)?,
            SetKind::Skill => self.select_skill_set(Some(id))?,
            SetKind::Config => self.select_config_set(id)?,
        };
        Ok(())
    }

    /// `describePopups` of a Manage... popup
    pub(super) fn describe_manage_sets(&self, p: &ManageSets) -> J {
        let kind = p.kind;
        let order = self.set_order(kind);
        let active = self.active_set(kind);
        let rows: Vec<J> = order
            .iter()
            .map(|id| {
                let current = if *id == active { "  ^9(Current)" } else { "" };
                json!(format!("{}{current}", self.set_title(kind, *id)))
            })
            .collect();
        let selected = p.sel_value.is_some();
        let list_name = kind.list();
        let button = |name: &str, label: &str, enabled: bool| {
            json!({
                "class": "ButtonControl", "enabled": enabled, "label": label,
                "path": format!("popup.{list_name}.{name}"), "shown": true,
            })
        };
        let mut list = json!({
            "class": kind.class(), "enabled": true, "list": rows,
            "path": format!("popup.{list_name}"), "shown": true,
        });
        if selected && let Some(i) = p.sel_index {
            list["sel"] = num(i as f64);
        }
        let mut controls = vec![
            button("new", "New", true),
            button("rename", "Rename", selected),
            button("copy", "Copy", selected),
            button("delete", "Delete", selected && order.len() > 1),
        ];
        // The Done button of the skill and config set popups has no name
        let mut buttons = vec![];
        if kind == SetKind::Item {
            let shared_selected = p.shared_sel.value;
            let shared = |name: &str, label: &str| {
                json!({
                    "class": "ButtonControl", "enabled": shared_selected, "label": label,
                    "path": format!("popup.sharedList.{name}"), "shown": true,
                })
            };
            controls.push(shared("rename", "Rename"));
            controls.push(shared("delete", "Delete"));
            buttons.push(json!({ "name": "close", "label": "Done", "enabled": true }));
        }
        controls.push(list);
        if kind == SetKind::Item {
            let mut shared = json!({
                "class": "SharedItemSetListControl", "enabled": true,
                "list": self.shared_set_rows(), "path": "popup.sharedList", "shown": true,
            });
            if let Some(i) = p.shared_sel.row() {
                shared["sel"] = num(i as f64);
            }
            controls.push(shared);
        }
        json!({
            "title": kind.title(),
            "lines": [],
            "buttons": buttons,
            "controls": controls,
        })
    }

    /// `describePopups` of the name popup
    pub(super) fn describe_rename_set(&self, p: &RenameSet) -> J {
        json!({
            "title": if p.titled { "Rename" } else { "Set Name" },
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

    /// `RenameSet(set, addOnName)`
    fn open_rename_set(&mut self, kind: SetKind, id: f64, add_on_name: bool) {
        let title = self.set_title_opt(kind, id).flatten();
        self.editor.popups.insert(
            0,
            ItemPopup::RenameSet(Box::new(RenameSet {
                kind,
                id,
                add_on_name,
                titled: title.is_some(),
                buf: title.unwrap_or_default(),
                save_enabled: false,
            })),
        );
    }

    /// `New...Set()`
    fn new_set(&mut self, kind: SetKind) -> Result<f64> {
        let id = self.free_set_id(kind);
        match kind {
            SetKind::Item => {
                let set = self.build.items.new_item_set(id);
                self.build.items.item_sets.insert(id as i64, set);
            }
            SetKind::Skill => self
                .build
                .skills
                .as_mut()
                .ok_or("no skills tab")?
                .new_skill_set(id),
            SetKind::Config => {
                let defaults = &self.data.game.option_defaults;
                Arc::make_mut(&mut self.build.config).new_set(id, None, defaults);
            }
        }
        Ok(id)
    }

    /// Copy: the set under a free id (`copyTable`, deep)
    fn copy_set(&mut self, kind: SetKind, src: f64) -> Result<f64> {
        let id = self.free_set_id(kind);
        let k = src as i64;
        let missing = "attempt to index a nil value";
        match kind {
            SetKind::Item => {
                let mut set = self.build.items.item_sets.get(&k).cloned().ok_or(missing)?;
                set.id = id;
                self.build.items.item_sets.insert(id as i64, set);
            }
            SetKind::Skill => {
                let tab = self.build.skills.as_mut().ok_or(missing)?;
                let mut set = tab.skill_sets.get(&k).cloned().ok_or(missing)?;
                set.id = id;
                tab.skill_sets.insert(id as i64, set);
            }
            SetKind::Config => {
                let tab = Arc::make_mut(&mut self.build.config);
                let mut set = tab.config_sets.get(&k).cloned().ok_or(missing)?;
                set.id = id;
                tab.config_sets.insert(id as i64, set);
            }
        }
        Ok(id)
    }

    /// `control` of the popup's list or its buttons
    pub(super) fn manage_sets_control(
        &mut self,
        p: &mut ManageSets,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let kind = p.kind;
        let click = req["click"].as_bool() == Some(true);
        let list = kind.list();
        let button = name
            .strip_prefix(list)
            .and_then(|r| r.strip_prefix('.'))
            .filter(|_| click);
        let missing = || "attempt to index a nil value".to_string();
        if kind == SetKind::Item && name.split('.').next() == Some("sharedList") {
            return self.shared_sets_control(p, name, req);
        }
        match (name == list, button) {
            (true, _) if req["dragTo"].is_string() => {
                // A set dragged onto the shared sets
                if req["dragTo"].as_str() != Some("popup.sharedList") || kind != SetKind::Item {
                    return Err(format!("no drag target {}", req["dragTo"]));
                }
                let sel = req["sel"].as_f64().unwrap_or(0.0);
                let order = self.set_order(kind);
                let value = (sel >= 1.0 && sel.fract() == 0.0)
                    .then(|| order.get(sel as usize - 1).copied())
                    .flatten();
                p.sel_value = value;
                let Some(id) = value else {
                    return Ok(());
                };
                p.sel_index = Some(sel as usize);
                self.drop_item_set_on_shared(id)?;
                p.sel_value = None;
            }
            (true, _) => {
                if let Some(sel) = req["sel"].as_f64() {
                    let order = self.set_order(kind);
                    let value = (sel >= 1.0 && sel.fract() == 0.0)
                        .then(|| order.get(sel as usize - 1).copied())
                        .flatten();
                    p.sel_value = value;
                    let Some(id) = value else {
                        return Ok(());
                    };
                    p.sel_index = Some(sel as usize);
                    // `OnSelClick`: a double click activates the set
                    if click
                        && req["doubleClick"].as_bool() == Some(true)
                        && id != self.active_set(kind)
                    {
                        self.activate_set(kind, id)?;
                    }
                } else if let Some(key) = req["key"].as_str() {
                    match (key, p.sel_value) {
                        ("DELETE", Some(id)) => self.delete_set(p, id),
                        ("F2", Some(id)) => self.open_rename_set(kind, id, false),
                        _ => {}
                    }
                } else {
                    return Err(format!("{NOT_PORTED}: control popup.{name}"));
                }
            }
            (false, Some("new")) => {
                let id = self.new_set(kind)?;
                self.open_rename_set(kind, id, true);
            }
            (false, Some("copy")) => {
                // Nothing selected indexes a missing set
                let src = p.sel_value.ok_or_else(missing)?;
                let id = self.copy_set(kind, src)?;
                self.open_rename_set(kind, id, true);
            }
            (false, Some("rename")) => {
                let id = p.sel_value.ok_or_else(missing)?;
                if !self.set_exists(kind, id) {
                    return Err(missing());
                }
                self.open_rename_set(kind, id, false);
            }
            (false, Some("delete")) => {
                // `OnSelDelete(nil, nil)` indexes the missing set
                let id = p.sel_value.ok_or_else(missing)?;
                self.delete_set(p, id);
            }
            _ => return Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
        Ok(())
    }

    /// `OnSelDelete`: a confirmation, while there is another set
    fn delete_set(&mut self, p: &ManageSets, id: f64) {
        let kind = p.kind;
        if self.set_order(kind).len() < 2 {
            return;
        }
        let index = p.sel_index.unwrap_or(1);
        let question = format!(
            "Are you sure you want to delete '{}'?",
            self.set_title(kind, id)
        );
        let (title, lines) = match kind {
            SetKind::Item => (
                "Delete Item Set",
                vec![
                    question,
                    "This will not delete any items used by the set.".into(),
                ],
            ),
            // The skill sets' says item set too
            SetKind::Skill => ("Delete Item Set", vec![question]),
            SetKind::Config => ("Delete Config Set", vec![question]),
        };
        self.editor.popups.insert(
            0,
            ItemPopup::Confirm {
                title: title.into(),
                lines,
                action: Deletion::Set { kind, index, id },
            },
        );
    }

    /// The popup's list as the delete and the name popup change it
    fn manage_popup_mut(&mut self, kind: SetKind) -> Option<&mut ManageSets> {
        self.editor.popups.iter_mut().find_map(|p| match p {
            ItemPopup::ManageSets(m) if m.kind == kind => Some(&mut **m),
            _ => None,
        })
    }

    /// The confirmation's Delete
    pub(super) fn confirm_delete_set(
        &mut self,
        kind: SetKind,
        index: usize,
        id: f64,
    ) -> Result<()> {
        let order = self.set_order_mut(kind).ok_or("no sets")?;
        if index >= 1 && index <= order.len() {
            order.remove(index - 1);
        }
        self.remove_set(kind, id);
        if let Some(m) = self.manage_popup_mut(kind) {
            m.sel_index = None;
            m.sel_value = None;
        }
        if id == self.active_set(kind) {
            let next = self
                .set_order(kind)
                .get(index.saturating_sub(1).max(1) - 1)
                .copied()
                .ok_or("no set")?;
            self.activate_set(kind, next)?;
        }
        Ok(())
    }

    /// `control` of the name popup's edit
    pub(super) fn rename_set_control(
        &mut self,
        p: &mut RenameSet,
        name: &str,
        req: &J,
    ) -> Result<()> {
        match (name, req["text"].as_str()) {
            ("edit", Some(t)) => {
                // `SetText(text, true)` neither filters nor limits
                p.buf = t.to_string();
                // `save.enabled = buf:match("%S")`: a string or nil, and nil
                // is no `false`, so it reads as enabled from then on
                p.save_enabled = true;
                Ok(())
            }
            _ => Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
    }

    /// The name popup's Save
    pub(super) fn rename_set_save(&mut self, p: &RenameSet) -> Result<()> {
        self.write_set_title(p.kind, p.id, p.buf.clone());
        self.ui.unsaved = true;
        if p.add_on_name {
            let order = self.set_order_mut(p.kind).ok_or("no sets")?;
            order.push(p.id);
            let n = order.len();
            if let Some(m) = self.manage_popup_mut(p.kind) {
                m.sel_index = Some(n);
                m.sel_value = Some(p.id);
            }
        }
        Ok(())
    }

    /// The name popup's Cancel
    pub(super) fn rename_set_cancel(&mut self, p: &RenameSet) {
        if p.add_on_name {
            self.remove_set(p.kind, p.id);
        }
    }

    /// `controlTooltip` of a row: `AddItemSetTooltip` for an item set; the
    /// other lists have none
    pub(super) fn set_list_tooltip(&mut self, kind: SetKind, index: Option<f64>) -> Result<J> {
        let mut tip = Tooltip::default();
        if kind != SetKind::Item {
            return Ok(tip.json());
        }
        self.update_sockets();
        let tab = &self.build.items;
        let id = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0)
            .and_then(|i| tab.item_set_order_list.get(i as usize - 1).copied());
        if let Some(set) = id.and_then(|id| tab.item_sets.get(&(id as i64))) {
            for s in tab.ordered_slots.iter().filter(|s| s.node_id.is_none()) {
                let sel = set.slots.get(&s.slot_name).map_or(0.0, |e| e.sel_item_id);
                if let Some(item) = tab.item(sel) {
                    tip.add_line(
                        16.0,
                        &format!(
                            "^7{}: {}{}",
                            self.slot_label(s),
                            super::display_stats::rarity_color(&item.rarity),
                            item.name
                        ),
                    );
                }
            }
        }
        Ok(tip.json())
    }
}
