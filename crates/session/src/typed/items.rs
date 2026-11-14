//! The Items tab's views and simple edits (`items`, `equipItem`,
//! `deleteItem`, `selectItemSet` of `lua/session.lua`) on the typed
//! items tab (`ItemsTab:UpdateSockets`, `ItemSlotControl:SetSelItemId`,
//! `ItemsTab:DeleteItem`, `SetActiveItemSet`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedSession, num};
use engine::item::Item;
use serde_json::{Map, Value as J, json};
use std::sync::Arc;

pub(super) fn describe(item: &Item) -> J {
    let mut o = Map::new();
    if let Some(id) = item.id {
        o.insert("id".into(), num(id));
    }
    o.insert("name".into(), json!(item.name));
    for (k, v) in [
        ("title", &item.title),
        ("baseName", &item.base_name),
        ("type", &item.typ),
    ] {
        if let Some(v) = v {
            o.insert(k.into(), json!(v));
        }
    }
    o.insert("rarity".into(), json!(item.rarity));
    o.insert("raw".into(), json!(item.build_raw()));
    J::Object(o)
}

impl<'d> TypedSession<'d> {
    /// Whether the slot at this index is shown and active (`IsShown()`
    /// and not `inactive`); `ring3` is `AdditionalRingSlot`
    /// `slot.inactive`: an unallocated socket, or an abyssal socket its
    /// item does not have
    pub(super) fn slot_inactive(&self, i: usize) -> bool {
        let tab = &self.build.items;
        let s = &tab.ordered_slots[i];
        match (&s.node_id, &s.parent_slot) {
            (Some(n), _) => !self.build.spec.alloc_nodes.contains(*n),
            (None, Some(p)) => {
                let sel = tab
                    .ordered_slots
                    .iter()
                    .find(|s| s.slot_name == *p)
                    .map_or(0.0, |s| s.sel_item_id);
                let count = if sel > 0.0 {
                    tab.item(sel).map_or(0.0, |i| i.abyssal_socket_count)
                } else {
                    0.0
                };
                s.slot_num.unwrap_or(0.0) > count
            }
            _ => false,
        }
    }

    pub(super) fn slot_shown(&self, i: usize, ring3: bool) -> bool {
        let tab = &self.build.items;
        let spec = &self.build.spec;
        let s = &tab.ordered_slots[i];
        let abyssal_count = |parent: &str| {
            let sel = tab
                .ordered_slots
                .iter()
                .find(|s| s.slot_name == parent)
                .map_or(0.0, |s| s.sel_item_id);
            if sel > 0.0 {
                tab.item(sel).map_or(0.0, |i| i.abyssal_socket_count)
            } else {
                0.0
            }
        };
        let inactive = match (&s.node_id, &s.parent_slot) {
            (Some(n), _) => !spec.alloc_nodes.contains(*n),
            (None, Some(p)) => s.slot_num.unwrap_or(0.0) > abyssal_count(p),
            _ => false,
        };
        let second = tab.active_item_set().use_second_weapon_set;
        let name = s.slot_name.as_str();
        !inactive
            && match s.weapon_set {
                Some(1) => !second,
                Some(2) => second,
                _ if name == "Graft 1" || name == "Graft 2" => spec.tree.version.contains("3_27"),
                _ if name == "Ring 3" => ring3,
                _ => true,
            }
    }

    /// Whether an item goes in a slot (`IsItemValidForSlot`)
    pub(super) fn valid_for_slot(&self, item: &Item, slot: &str) -> bool {
        let info = &self.data.game.setup.weapon_type_info;
        let one_hand = |t: &str| info.get(t).map(|w| w.one_hand);
        self.build
            .items
            .valid_for_slot(item, slot, self.build.spec.tree, &one_hand)
    }

    /// `PopulateSlots()`
    pub(super) fn populate_slots(&mut self) {
        let info = &self.data.game.setup.weapon_type_info;
        let one_hand = |t: &str| info.get(t).map(|w| w.one_hand);
        let b = &mut self.build;
        let tree = b.spec.tree;
        b.items
            .populate_slots(Arc::make_mut(&mut b.spec), tree, &one_hand);
    }

    /// `items`: the items, the slots with what fits them, the item sets
    pub fn items(&mut self) -> Result<J> {
        let tab = &self.build.items;
        let spec = &self.build.spec;
        // `UpdateSockets`: the labels of the allocated sockets
        let mut active: Vec<u32> = tab
            .ordered_slots
            .iter()
            .filter_map(|s| s.node_id)
            .filter(|n| spec.alloc_nodes.contains(*n))
            .collect();
        active.sort_unstable();
        for (i, n) in active.iter().enumerate() {
            self.socket_labels.insert(*n, format!("Socket #{}", i + 1));
        }
        let sb = self.sidebar()?;
        let tab = &self.build.items;
        let items: Vec<J> = tab
            .item_order_list
            .iter()
            .filter_map(|id| tab.item(*id))
            .map(describe)
            .collect();
        let ring3 = sb.additional_ring_slot;
        let slots: Vec<J> = tab
            .ordered_slots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let name = s.slot_name.as_str();
                let shown = self.slot_shown(i, ring3);
                let label = match s.node_id {
                    Some(n) => self
                        .socket_labels
                        .get(&n)
                        .cloned()
                        .unwrap_or_else(|| "Socket".into()),
                    None => s.label(),
                };
                let mut o = Map::new();
                o.insert("name".into(), json!(name));
                o.insert("label".into(), json!(label));
                o.insert("shown".into(), J::Bool(shown));
                if s.sel_item_id != 0.0 {
                    o.insert("item".into(), num(s.sel_item_id));
                }
                if let Some(n) = s.node_id {
                    o.insert("nodeId".into(), json!(n));
                }
                let candidates: Vec<J> = self
                    .item_keys
                    .iter()
                    .filter_map(|(id, _)| tab.item(id as f64))
                    .filter(|i| self.valid_for_slot(i, name))
                    .filter_map(|i| i.id.map(num))
                    .collect();
                o.insert("candidates".into(), J::Array(candidates));
                J::Object(o)
            })
            .collect();
        let sets: Vec<J> = tab
            .item_set_order_list
            .iter()
            .map(|id| {
                let title = tab
                    .item_sets
                    .get(&(*id as i64))
                    .and_then(|s| s.title.clone())
                    .unwrap_or_else(|| "Default".into());
                json!({ "id": num(*id), "title": title })
            })
            .collect();
        Ok(json!({
            "items": items,
            "slots": slots,
            "itemSets": sets,
            "activeItemSet": num(tab.active_item_set_id),
        }))
    }

    /// `ItemSlotControl:SetSelItemId(id)`
    pub(super) fn set_sel_item(&mut self, slot: usize, id: f64) -> Result<()> {
        let b = &mut self.build;
        let s = &b.items.ordered_slots[slot];
        match s.node_id {
            Some(node) => {
                let changed = id != s.sel_item_id;
                let spec = Arc::make_mut(&mut b.spec);
                spec.jewels.insert(node, id);
                if changed {
                    spec.rebuild_cluster_jewel_graphs(&b.items, &self.data.game.items)?;
                }
            }
            None => {
                let name = s.slot_name.clone();
                let set_id = b.items.active_item_set_id as i64;
                if let Some(e) = b
                    .items
                    .item_sets
                    .get_mut(&set_id)
                    .and_then(|s| s.slots.get_mut(&name))
                {
                    e.sel_item_id = id;
                }
            }
        }
        b.items.ordered_slots[slot].sel_item_id = id;
        Ok(())
    }

    /// `equipItem`: puts an item (or nothing) in a slot
    pub fn equip_item(&mut self, slot: &str, id: f64) -> Result<J> {
        let i = self
            .build
            .items
            .ordered_slots
            .iter()
            .position(|s| s.slot_name == slot)
            .ok_or_else(|| format!("no slot {slot}"))?;
        self.set_sel_item(i, id)?;
        self.populate_slots();
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `deleteItem`: `ItemsTab:DeleteItem(item)`
    pub fn delete_item(&mut self, id: f64) -> Result<J> {
        if self.build.items.item(id).is_none() || id <= 0.0 {
            return Err(format!("no item {}", engine::pat::num(id)));
        }
        // `buildFlag` only when something held it
        let mut flag = false;
        for i in 0..self.build.items.ordered_slots.len() {
            if self.build.items.ordered_slots[i].sel_item_id == id {
                self.set_sel_item(i, 0.0)?;
                flag = true;
            }
        }
        let tab = &mut self.build.items;
        for set in tab.item_sets.values_mut() {
            for e in set.slots.values_mut() {
                if e.sel_item_id == id {
                    e.sel_item_id = 0.0;
                    flag = true;
                }
            }
        }
        if let Some(p) = tab.item_order_list.iter().position(|x| *x == id) {
            tab.item_order_list.remove(p);
        }
        let data = &self.data.game.items;
        let b = &mut self.build;
        let specs = std::iter::once(&mut b.spec).chain(self.other_specs.iter_mut().flatten());
        for spec in specs {
            if !spec.jewels.values().any(|j| *j == id) {
                continue;
            }
            let spec = Arc::make_mut(spec);
            for node in spec
                .jewels
                .iter()
                .filter(|(_, j)| **j == id)
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
            {
                spec.jewels.insert(node, 0.0);
                spec.drop_jewel_socket(node);
            }
            spec.rebuild_cluster_jewel_graphs(&b.items, data)?;
        }
        let tab = &mut self.build.items;
        tab.items.remove(&(id as i64));
        self.item_keys.remove(id as u32);
        self.populate_slots();
        if flag {
            self.rebuild()?;
        }
        Ok(json!([]))
    }

    /// `selectItemSet`: `ItemsTab:SetActiveItemSet(id)`
    pub fn select_item_set(&mut self, id: f64) -> Result<J> {
        let tab = &mut self.build.items;
        let id = if tab.item_sets.contains_key(&(id as i64)) && id.fract() == 0.0 {
            id
        } else {
            *tab.item_set_order_list.first().ok_or("no item set")?
        };
        tab.active_item_set_id = id;
        self.populate_slots();
        self.rebuild()?;
        Ok(json!([]))
    }
}
