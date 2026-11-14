//! The Items tab's Manage Item Sets popup (`ItemsTab:OpenItemSetManagePopup`,
//! `ItemSetListControl`): the sets' list with Copy, Delete, Rename and New,
//! the name popup (`RenameSet`), the delete confirmation, a double click
//! to activate a set, and a row's tooltip (`AddItemSetTooltip`). The shared
//! item set list beside it (`SharedItemSetListControl`) is empty: only
//! dragging fills it.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::{Deletion, ItemPopup};
use super::tooltip::Tooltip;
use super::{NOT_PORTED, Result, TypedSession, num};
use serde_json::{Value as J, json};

/// The popup's lists' selections (`selIndex`, `selValue`)
#[derive(Clone, Debug, Default)]
pub struct ManageItemSets {
    sel_index: Option<usize>,
    sel_value: Option<f64>,
}

/// The name popup of a set (`RenameSet(itemSet, addOnName)`)
#[derive(Clone, Debug)]
pub struct RenameSet {
    id: f64,
    /// A new set, added to the list on Save, dropped on Cancel
    add_on_name: bool,
    /// Whether the set had a title (the popup's title)
    titled: bool,
    buf: String,
    save_enabled: bool,
}

impl<'d> TypedSession<'d> {
    pub fn is_item_set_control(path: &str) -> bool {
        path == "items.setManage"
    }

    /// `control` of the Manage... button: the popup
    pub fn item_set_control(&mut self, req: &J) -> Result<J> {
        if req["click"].as_bool() != Some(true) {
            return Err(format!("{NOT_PORTED}: control items.setManage"));
        }
        if self.class_popups_open() > 0 {
            return Err(format!(
                "{NOT_PORTED}: item sets with a class change popup open"
            ));
        }
        self.editor
            .popups
            .insert(0, ItemPopup::ManageItemSets(Box::default()));
        self.refresh_item_popups();
        Ok(json!([]))
    }

    fn set_title(&self, id: f64) -> String {
        self.build
            .items
            .item_sets
            .get(&(id as i64))
            .and_then(|s| s.title.clone())
            .unwrap_or_else(|| "Default".into())
    }

    /// `describePopups` of Manage Item Sets
    pub(super) fn describe_manage_item_sets(&self, p: &ManageItemSets) -> J {
        let tab = &self.build.items;
        let rows: Vec<J> = tab
            .item_set_order_list
            .iter()
            .map(|id| {
                let current = if *id == tab.active_item_set_id {
                    "  ^9(Current)"
                } else {
                    ""
                };
                json!(format!("{}{current}", self.set_title(*id)))
            })
            .collect();
        let selected = p.sel_value.is_some();
        let button = |path: &str, label: &str, enabled: bool| json!({ "class": "ButtonControl", "enabled": enabled, "label": label, "path": path, "shown": true });
        let mut list = json!({
            "class": "ItemSetListControl", "enabled": true, "list": rows,
            "path": "popup.setList", "shown": true,
        });
        if selected && let Some(i) = p.sel_index {
            list["sel"] = num(i as f64);
        }
        let shared = json!({
            "class": "SharedItemSetListControl", "enabled": true, "list": [],
            "path": "popup.sharedList", "shown": true,
        });
        json!({
            "title": "Manage Item Sets",
            "lines": [],
            "buttons": [{ "name": "close", "label": "Done", "enabled": true }],
            "controls": [
                button("popup.setList.new", "New", true),
                button("popup.setList.rename", "Rename", selected),
                button("popup.setList.copy", "Copy", selected),
                button("popup.setList.delete", "Delete", selected && tab.item_set_order_list.len() > 1),
                button("popup.sharedList.rename", "Rename", false),
                button("popup.sharedList.delete", "Delete", false),
                list,
                shared,
            ],
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

    /// `RenameSet(itemSet, addOnName)`
    fn open_rename_set(&mut self, id: f64, add_on_name: bool) {
        let title = self
            .build
            .items
            .item_sets
            .get(&(id as i64))
            .and_then(|s| s.title.clone());
        self.editor.popups.insert(
            0,
            ItemPopup::RenameSet(Box::new(RenameSet {
                id,
                add_on_name,
                titled: title.is_some(),
                buf: title.unwrap_or_default(),
                save_enabled: false,
            })),
        );
    }

    /// A free set id, from 1
    fn free_set_id(&self) -> f64 {
        let mut id = 1.0;
        while self.build.items.item_sets.contains_key(&(id as i64)) {
            id += 1.0;
        }
        id
    }

    /// `control` of the popup's list or its buttons
    pub(super) fn manage_item_sets_control(
        &mut self,
        p: &mut ManageItemSets,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let click = req["click"].as_bool() == Some(true);
        match name {
            "setList" => {
                if let Some(sel) = req["sel"].as_f64() {
                    let list = &self.build.items.item_set_order_list;
                    let value = (sel >= 1.0 && sel.fract() == 0.0)
                        .then(|| list.get(sel as usize - 1).copied())
                        .flatten();
                    p.sel_value = value;
                    let Some(id) = value else {
                        return Ok(());
                    };
                    p.sel_index = Some(sel as usize);
                    // `OnSelClick`: a double click activates the set
                    if click
                        && req["doubleClick"].as_bool() == Some(true)
                        && id != self.build.items.active_item_set_id
                    {
                        self.select_item_set(id)?;
                    }
                } else if let Some(key) = req["key"].as_str() {
                    match (key, p.sel_value) {
                        ("DELETE", Some(id)) => self.delete_item_set(p, id),
                        ("F2", Some(id)) => self.open_rename_set(id, false),
                        _ => {}
                    }
                } else {
                    return Err(format!("{NOT_PORTED}: control popup.{name}"));
                }
            }
            "setList.new" if click => {
                // `NewItemSet()`: every slot but the sockets empty
                let id = self.free_set_id();
                let set = self.build.items.new_item_set(id);
                self.build.items.item_sets.insert(id as i64, set);
                self.open_rename_set(id, true);
            }
            "setList.copy" if click => {
                // `copyTable` of the set under a free id; nothing selected
                // indexes a missing set
                let src = p.sel_value.ok_or("attempt to index a nil value")?;
                let mut set = self
                    .build
                    .items
                    .item_sets
                    .get(&(src as i64))
                    .cloned()
                    .ok_or("attempt to index a nil value")?;
                let id = self.free_set_id();
                set.id = id;
                self.build.items.item_sets.insert(id as i64, set);
                self.open_rename_set(id, true);
            }
            "setList.rename" if click => {
                let id = p.sel_value.ok_or("attempt to index a nil value")?;
                self.open_rename_set(id, false);
            }
            "setList.delete" if click => {
                // `OnSelDelete(nil, nil)` indexes the missing set
                let id = p.sel_value.ok_or("attempt to index a nil value")?;
                self.delete_item_set(p, id);
            }
            _ => return Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
        Ok(())
    }

    /// `OnSelDelete`: a confirmation, while there is another set
    fn delete_item_set(&mut self, p: &ManageItemSets, id: f64) {
        if self.build.items.item_set_order_list.len() < 2 {
            return;
        }
        let index = p.sel_index.unwrap_or(1);
        let title = self.set_title(id);
        self.editor.popups.insert(
            0,
            ItemPopup::Confirm {
                title: "Delete Item Set".into(),
                lines: vec![
                    format!("Are you sure you want to delete '{title}'?"),
                    "This will not delete any items used by the set.".into(),
                ],
                action: Deletion::ItemSet { index, id },
            },
        );
    }

    /// The confirmation's Delete
    pub(super) fn confirm_delete_item_set(&mut self, index: usize, id: f64) -> Result<()> {
        let tab = &mut self.build.items;
        if index >= 1 && index <= tab.item_set_order_list.len() {
            tab.item_set_order_list.remove(index - 1);
        }
        tab.item_sets.remove(&(id as i64));
        // The selection of the list the popup shows
        if let Some(ItemPopup::ManageItemSets(m)) = self
            .editor
            .popups
            .iter_mut()
            .find(|p| matches!(p, ItemPopup::ManageItemSets(_)))
        {
            m.sel_index = None;
            m.sel_value = None;
        }
        if id == self.build.items.active_item_set_id {
            let list = &self.build.items.item_set_order_list;
            let next = list
                .get(index.saturating_sub(1).max(1) - 1)
                .copied()
                .ok_or("no item set")?;
            self.select_item_set(next)?;
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
        if let Some(set) = self.build.items.item_sets.get_mut(&(p.id as i64)) {
            set.title = Some(p.buf.clone());
        }
        self.ui.unsaved = true;
        if p.add_on_name {
            self.build.items.item_set_order_list.push(p.id);
            let n = self.build.items.item_set_order_list.len();
            if let Some(ItemPopup::ManageItemSets(m)) = self
                .editor
                .popups
                .iter_mut()
                .find(|p| matches!(p, ItemPopup::ManageItemSets(_)))
            {
                m.sel_index = Some(n);
                m.sel_value = Some(p.id);
            }
        }
        Ok(())
    }

    /// The name popup's Cancel
    pub(super) fn rename_set_cancel(&mut self, p: &RenameSet) {
        if p.add_on_name {
            self.build.items.item_sets.remove(&(p.id as i64));
        }
    }

    /// `controlTooltip` of a row: `AddItemSetTooltip`
    pub(super) fn item_set_tooltip(&mut self, index: Option<f64>) -> Result<J> {
        self.update_sockets();
        let mut tip = Tooltip::default();
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
