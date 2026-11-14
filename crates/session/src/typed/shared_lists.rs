//! The shared item list and shared item sets (`main.sharedItemList`,
//! `main.sharedItemSetList`, `SharedItemListControl`,
//! `SharedItemSetListControl`): lists that outlive the build, filled by
//! dragging items and item sets onto them (`control` with `dragTo`), and
//! dragged back onto the build's lists. A typed session hands them to the
//! next build it loads; once the translation took over they are unknown
//! (`None`), and the typed session leaves them to it.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::{Deletion, ItemPopup};
use super::tooltip::Tooltip;
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::item::Item;
use serde_json::{Value as J, json};
use std::sync::Arc;

/// A shared item set: its title and an item for each slot
#[derive(Clone, Debug)]
pub struct SharedSet {
    pub title: Option<String>,
    pub slots: Vec<(String, Arc<Item>)>,
}

#[derive(Clone, Debug, Default)]
pub struct SharedLists {
    pub items: Vec<Arc<Item>>,
    pub sets: Vec<SharedSet>,
}

/// A list's `selIndex`, and whether `selValue` is set
#[derive(Clone, Copy, Debug, Default)]
pub struct ListSel {
    pub index: Option<usize>,
    pub value: bool,
}

impl ListSel {
    /// `SelectIndex(sel)` on a list of `len` rows: whether it selected
    pub fn select(&mut self, sel: f64, len: usize) -> bool {
        let ok = sel >= 1.0 && sel.fract() == 0.0 && (sel as usize) <= len;
        self.value = ok;
        if ok {
            self.index = Some(sel as usize);
        }
        ok
    }

    /// The selected row, when `selValue` is set
    pub fn row(&self) -> Option<usize> {
        self.value.then_some(self.index).flatten()
    }
}

/// The name popup of a shared item set (`SharedItemSetList:RenameSet`)
#[derive(Clone, Debug)]
pub struct RenameShared {
    index: usize,
    titled: bool,
    buf: String,
    save_enabled: bool,
}

/// The drag targets upstream gives each list (`dragTargetList`) that the
/// session can reach
fn drag_target_ok(src: &str, dst: &str) -> bool {
    matches!(
        (src, dst),
        ("items.itemList", "items.sharedItemList")
            | (
                "items.uniqueDB" | "items.rareDB",
                "items.itemList" | "items.sharedItemList"
            )
            | ("items.sharedItemList", "items.itemList")
            | ("popup.setList", "popup.sharedList")
            | ("popup.sharedList", "popup.setList")
    )
}

impl<'d> TypedSession<'d> {
    /// The shared lists, which a session that fell back no longer knows
    fn shared(&self) -> Result<&SharedLists> {
        self.shared
            .as_ref()
            .ok_or_else(|| format!("{NOT_PORTED}: the shared lists after a fallback"))
    }

    fn shared_mut(&mut self) -> Result<&mut SharedLists> {
        self.shared
            .as_mut()
            .ok_or_else(|| format!("{NOT_PORTED}: the shared lists after a fallback"))
    }

    /// The shared lists of the session a load replaces
    pub fn keep_shared(&mut self, prev: &TypedSession) {
        self.shared = prev.shared.clone();
    }

    /// The translation held the build: its shared lists are unknown
    pub fn forget_shared(&mut self) {
        self.shared = None;
    }

    pub fn is_shared_list_control(path: &str) -> bool {
        matches!(path, "items.sharedItemList" | "items.sharedItemList.delete")
    }

    /// `controls` of the shared item list and its Delete
    pub fn shared_list_control_json(&self, path: &str) -> Result<J> {
        let shared = self.shared()?;
        let sel = self.shared_sel;
        Ok(if path == "items.sharedItemList" {
            let rows: Vec<J> = shared
                .items
                .iter()
                .map(|i| {
                    json!(format!(
                        "{}{}",
                        super::display_stats::rarity_color(&i.rarity),
                        i.name
                    ))
                })
                .collect();
            let mut o = json!({
                "class": "SharedItemListControl", "enabled": true, "shown": true,
                "label": "^7Shared items:", "list": rows,
            });
            if let Some(i) = sel.row() {
                o["sel"] = num(i as f64);
            }
            o
        } else {
            json!({
                "class": "ButtonControl", "enabled": sel.value, "shown": true, "label": "Delete",
            })
        })
    }

    /// `control` of the shared item list: a row selected, clicked (twice to
    /// edit), deleted, or dragged onto the item list
    pub fn shared_list_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        if self.class_popups_open() > 0 {
            return Err(format!(
                "{NOT_PORTED}: the shared items with a class change popup open"
            ));
        }
        let len = self.shared()?.items.len();
        if path == "items.sharedItemList.delete" {
            if req["click"].as_bool() != Some(true) {
                return Err(format!("{NOT_PORTED}: control {path}"));
            }
            // `OnSelDelete(nil, nil)` indexes the missing item
            let i = self
                .shared_sel
                .row()
                .ok_or("attempt to index a nil value")?;
            self.delete_shared_item(i)?;
            return Ok(json!([]));
        }
        if let Some(sel) = req["sel"].as_f64() {
            if !self.shared_sel.select(sel, len) {
                return Ok(json!([]));
            }
            let i = sel as usize;
            if req["click"].as_bool() == Some(true) && req["doubleClick"].as_bool() == Some(true) {
                // `CreateDisplayItemFromRaw(item.raw, true)`
                let raw = self.shared()?.items[i - 1].raw.clone();
                self.display_item_from_raw(&raw, true)?;
            }
        } else if req["key"].as_str() == Some("DELETE") {
            if let Some(i) = self.shared_sel.row() {
                self.delete_shared_item(i)?;
            }
        } else {
            return Err(format!("{NOT_PORTED}: control {path}"));
        }
        Ok(json!([]))
    }

    /// `OnSelDelete`: its confirmation
    fn delete_shared_item(&mut self, index: usize) -> Result<()> {
        let name = self.shared()?.items[index - 1].name.clone();
        self.editor.popups.insert(
            0,
            ItemPopup::Confirm {
                title: "Delete Item".into(),
                lines: vec![format!(
                    "Are you sure you want to remove '{name}' from the shared item list?"
                )],
                action: Deletion::SharedItem { index },
            },
        );
        self.refresh_item_popups();
        Ok(())
    }

    /// The confirmations' Delete
    pub(super) fn confirm_delete_shared(&mut self, index: usize, set: bool) -> Result<()> {
        let shared = self.shared_mut()?;
        let list_len = if set {
            shared.sets.len()
        } else {
            shared.items.len()
        };
        if index >= 1 && index <= list_len {
            if set {
                shared.sets.remove(index - 1);
            } else {
                shared.items.remove(index - 1);
            }
        }
        if set {
            if let Some(m) = self.manage_sets_popup_mut() {
                m.shared_sel = ListSel::default();
            }
        } else {
            self.shared_sel = ListSel::default();
        }
        Ok(())
    }

    /// `controlTooltip` of a shared item: `AddItemTooltip`
    pub fn shared_list_tooltip(&mut self, index: Option<f64>) -> Result<J> {
        self.update_sockets();
        let mut tip = Tooltip::default();
        let item = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0)
            .and_then(|i| self.shared().ok()?.items.get(i as usize - 1).cloned());
        let popups = self.ui.popups.as_array().map_or(0, Vec::len);
        if let Some(item) = item
            && popups == 0
        {
            self.add_item_tooltip(&mut tip, &item, None)?;
        }
        Ok(tip.json())
    }

    /// A new item from a text, as `new("Item"):Item(raw)`
    fn item_from_raw(&self, raw: &str) -> Result<Item> {
        let item = Item::new(&self.data.item_context(), raw, None, false)?;
        if item.base.is_none() {
            return Err(format!("{NOT_PORTED}: a dragged item without a base"));
        }
        Ok(item)
    }

    /// `control` with `dragTo`: a row of the item list, an item database or
    /// the shared item list dropped onto another list, before row `at`
    pub fn items_drag(&mut self, req: &J) -> Result<J> {
        let src = req["path"].as_str().unwrap_or("");
        let dst = req["dragTo"].as_str().unwrap_or("");
        if !drag_target_ok(src, dst) {
            return Err(format!("no drag target {dst}"));
        }
        if self.class_popups_open() > 0 {
            return Err(format!(
                "{NOT_PORTED}: dragging with a class change popup open"
            ));
        }
        let sel = req["sel"]
            .as_f64()
            .ok_or_else(|| format!("{NOT_PORTED}: a drag without a row"))?;
        // `SelectIndex(sel)` and `GetDragValue`: the dragged item
        let item: Arc<Item> = match src {
            "items.itemList" => {
                let Some(id) = self.item_list_select_value(sel) else {
                    return Ok(json!([]));
                };
                Arc::new(self.build.items.item(id).ok_or("no item")?.clone())
            }
            "items.sharedItemList" => {
                let len = self.shared()?.items.len();
                if !self.shared_sel.select(sel, len) {
                    return Ok(json!([]));
                }
                self.shared()?.items[sel as usize - 1].clone()
            }
            _ => {
                let rare = src == "items.rareDB";
                match self.item_db_row_item(rare, sel)? {
                    Some(i) => i,
                    None => return Ok(json!([])),
                }
            }
        };
        match dst {
            "items.sharedItemList" => {
                // `SharedItemListClass:ReceiveDrag`: a copy, of a database
                // item with its quality set
                let mut new = self.item_from_raw(&item.build_raw())?;
                if item.id.is_none() {
                    new.normalise_quality();
                }
                let shared = self.shared_mut()?;
                let at = req["at"]
                    .as_f64()
                    .unwrap_or(shared.items.len() as f64 + 1.0);
                if at < 1.0 || at.fract() != 0.0 || at as usize > shared.items.len() + 1 {
                    return Err(format!("{NOT_PORTED}: dropping outside the list"));
                }
                shared.items.insert(at as usize - 1, Arc::new(new));
            }
            _ => {
                // `ItemListClass:ReceiveDrag`: the item added at the drop
                let mut new = self.item_from_raw(&item.raw)?;
                new.normalise_quality();
                let len = self.build.items.item_order_list.len();
                let at = req["at"].as_f64().unwrap_or(len as f64 + 1.0);
                if at < 1.0 || at.fract() != 0.0 || at as usize > len + 1 {
                    return Err(format!("{NOT_PORTED}: dropping outside the list"));
                }
                let id = self.add_item(new, true)?;
                let order = &mut self.build.items.item_order_list;
                if let Some(p) = order.iter().position(|x| *x == id) {
                    order.remove(p);
                    order.insert(at as usize - 1, id);
                }
                let added = self.build.items.item(id).ok_or("no item")?.clone();
                let other = super::item_editor::FORBIDDEN
                    .iter()
                    .find(|(t, _)| added.title.as_deref() == Some(*t))
                    .map(|(_, o)| *o);
                if let Some(other) = other {
                    self.add_forbidden_counterpart(&added, other)?;
                }
                self.populate_slots();
                self.item_list_changed();
            }
        }
        // The source's `selValue = nil`
        match src {
            "items.itemList" => self.item_list_drop_sent(),
            "items.sharedItemList" => self.shared_sel.value = false,
            _ => {}
        }
        Ok(json!([]))
    }

    fn manage_sets_popup_mut(&mut self) -> Option<&mut super::item_sets::ManageSets> {
        self.editor.popups.iter_mut().find_map(|p| match p {
            ItemPopup::ManageSets(m) if m.kind == super::item_sets::SetKind::Item => Some(&mut **m),
            _ => None,
        })
    }

    /// The shared item sets' rows in Manage Item Sets
    pub(super) fn shared_set_rows(&self) -> Vec<J> {
        self.shared
            .as_ref()
            .map(|s| {
                s.sets
                    .iter()
                    .map(|s| json!(s.title.clone().unwrap_or_else(|| "Default".into())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `control` of the shared item sets' list and its buttons, and drags
    /// between it and the build's sets
    pub(super) fn shared_sets_control(
        &mut self,
        m: &mut super::item_sets::ManageSets,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let click = req["click"].as_bool() == Some(true);
        let len = self.shared()?.sets.len();
        let missing = || "attempt to index a nil value".to_string();
        match name {
            "sharedList" => {
                if let Some(dst) = req["dragTo"].as_str() {
                    if dst != "popup.setList" {
                        return Err(format!("no drag target {dst}"));
                    }
                    let sel = req["sel"].as_f64().unwrap_or(0.0);
                    if !m.shared_sel.select(sel, len) {
                        return Ok(());
                    }
                    self.drop_shared_set(m, sel as usize, req["at"].as_f64())?;
                    m.shared_sel.value = false;
                } else if let Some(sel) = req["sel"].as_f64() {
                    // No `OnSelClick`
                    m.shared_sel.select(sel, len);
                } else if let Some(key) = req["key"].as_str() {
                    match (key, m.shared_sel.row()) {
                        ("DELETE", Some(i)) => self.delete_shared_set(i)?,
                        ("F2", Some(i)) => self.open_rename_shared(i)?,
                        _ => {}
                    }
                } else {
                    return Err(format!("{NOT_PORTED}: control popup.{name}"));
                }
            }
            "sharedList.rename" if click => {
                let i = m.shared_sel.row().ok_or_else(missing)?;
                self.open_rename_shared(i)?;
            }
            "sharedList.delete" if click => {
                let i = m.shared_sel.row().ok_or_else(missing)?;
                self.delete_shared_set(i)?;
            }
            _ => return Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
        Ok(())
    }

    /// A build set dragged onto the shared sets
    /// (`SharedItemSetListClass:ReceiveDrag`): copies of its items, appended
    /// (it reads `dragIndex`, which nothing sets)
    pub(super) fn drop_item_set_on_shared(&mut self, set_id: f64) -> Result<()> {
        let tab = &self.build.items;
        let set = tab
            .item_sets
            .get(&(set_id as i64))
            .ok_or("attempt to index a nil value")?;
        let active = set_id == tab.active_item_set_id;
        let mut slots = Vec::new();
        for s in tab.ordered_slots.iter().filter(|s| s.node_id.is_none()) {
            let sel = if active {
                s.sel_item_id
            } else {
                set.slots.get(&s.slot_name).map_or(0.0, |e| e.sel_item_id)
            };
            if sel != 0.0 {
                let item = tab.item(sel).ok_or("attempt to index a nil value")?;
                // The set has an id: no `NormaliseQuality`
                let new = self.item_from_raw(&item.build_raw())?;
                slots.push((s.slot_name.clone(), Arc::new(new)));
            }
        }
        let title = set.title.clone();
        self.shared_mut()?.sets.push(SharedSet { title, slots });
        Ok(())
    }

    /// A shared set dragged onto the build's sets
    /// (`ItemSetListClass:ReceiveDrag`): a new set with copies of its items
    fn drop_shared_set(
        &mut self,
        m: &mut super::item_sets::ManageSets,
        index: usize,
        at: Option<f64>,
    ) -> Result<()> {
        let shared = self.shared()?.sets[index - 1].clone();
        if shared.slots.len() > 1 {
            // `pairs(value.slots)` gives the items their ids in the order
            // of the slot names' hashes
            return Err(format!(
                "{NOT_PORTED}: a shared item set of several items dragged into the build"
            ));
        }
        let len = self.build.items.item_set_order_list.len();
        let at = at.unwrap_or(len as f64 + 1.0);
        if at < 1.0 || at.fract() != 0.0 || at as usize > len + 1 {
            return Err(format!("{NOT_PORTED}: dropping outside the list"));
        }
        let mut id = 1.0;
        while self.build.items.item_sets.contains_key(&(id as i64)) {
            id += 1.0;
        }
        let mut set = self.build.items.new_item_set(id);
        set.title = shared.title.clone();
        self.build.items.item_sets.insert(id as i64, set);
        for (slot, item) in &shared.slots {
            let mut new = self.item_from_raw(&item.raw)?;
            new.normalise_quality();
            let item_id = self.add_item(new, true)?;
            if let Some(e) = self
                .build
                .items
                .item_sets
                .get_mut(&(id as i64))
                .and_then(|s| s.slots.get_mut(slot))
            {
                e.sel_item_id = item_id;
            }
        }
        self.build
            .items
            .item_set_order_list
            .insert(at as usize - 1, id);
        let _ = m;
        self.ui.unsaved = true;
        Ok(())
    }

    fn delete_shared_set(&mut self, index: usize) -> Result<()> {
        let title = self.shared()?.sets[index - 1]
            .title
            .clone()
            .unwrap_or_else(|| "Default".into());
        self.editor.popups.insert(
            0,
            ItemPopup::Confirm {
                title: "Delete Item Set".into(),
                lines: vec![format!(
                    "Are you sure you want to delete '{title}' from the shared item set list?"
                )],
                action: Deletion::SharedSet { index },
            },
        );
        Ok(())
    }

    fn open_rename_shared(&mut self, index: usize) -> Result<()> {
        let title = self.shared()?.sets[index - 1].title.clone();
        self.editor.popups.insert(
            0,
            ItemPopup::RenameShared(Box::new(RenameShared {
                index,
                titled: title.is_some(),
                buf: title.unwrap_or_default(),
                save_enabled: false,
            })),
        );
        Ok(())
    }

    pub(super) fn describe_rename_shared(&self, p: &RenameShared) -> J {
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

    pub(super) fn rename_shared_control(
        &mut self,
        p: &mut RenameShared,
        name: &str,
        req: &J,
    ) -> Result<()> {
        match (name, req["text"].as_str()) {
            ("edit", Some(t)) => {
                p.buf = t.to_string();
                p.save_enabled = true;
                Ok(())
            }
            _ => Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
    }

    pub(super) fn rename_shared_save(&mut self, p: &RenameShared) -> Result<()> {
        if let Some(s) = self.shared_mut()?.sets.get_mut(p.index - 1) {
            s.title = Some(p.buf.clone());
        }
        self.ui.unsaved = true;
        Ok(())
    }

    /// `controlTooltip` of a shared set: its items by slot
    pub(super) fn shared_set_tooltip(&mut self, index: Option<f64>) -> Result<J> {
        self.update_sockets();
        let mut tip = Tooltip::default();
        let set = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0)
            .and_then(|i| self.shared().ok()?.sets.get(i as usize - 1).cloned());
        if let Some(set) = set {
            for s in self
                .build
                .items
                .ordered_slots
                .iter()
                .filter(|s| s.node_id.is_none())
            {
                if let Some((_, item)) = set.slots.iter().find(|(n, _)| *n == s.slot_name) {
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
