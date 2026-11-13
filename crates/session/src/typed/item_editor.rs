//! The Items tab's item edits and the editor's display item (`addItem`,
//! `editItem`, `setDisplayItem`, `saveDisplayItem` of `lua/session.lua`):
//! `ItemsTab:AddItem`, `AddDisplayItem`, `CreateDisplayItemFromRaw`,
//! `CopyAnointsAndEldritchImplicits`, `SetDisplayItem`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession};
use engine::item::{INFLUENCES, Item};
use serde_json::{Value as J, json};
use std::sync::Arc;

/// `forbiddenJewelCounterpart`
const FORBIDDEN: [(&str, &str); 2] = [
    ("Forbidden Flame", "Forbidden Flesh"),
    ("Forbidden Flesh", "Forbidden Flame"),
];

/// `isAnointable(item)`
fn anointable(item: &Item) -> bool {
    item.base.as_ref().is_some_and(|b| {
        !b.cannot_be_anointed
            && b.sub_type.as_deref() != Some("Talisman")
            && (item.can_be_anointed || b.typ == "Amulet")
    })
}

impl<'d> TypedSession<'d> {
    /// `new("Item", raw)`, `None` when its base is not recognised
    fn new_item(&self, raw: &str) -> Result<Option<Item>> {
        let item = Item::new(&self.data.item_context(), raw, None, false)?;
        Ok(item.base.is_some().then_some(item))
    }

    /// `BuildAndParseRaw()`
    fn build_and_parse_raw(&self, item: &mut Item) -> Result<()> {
        let raw = item.build_raw();
        item.parse_raw(&self.data.item_context(), &raw, None, false)
    }

    /// `CopyAnointsAndEldritchImplicits(newItem, copyEldritchImplicits,
    /// false)`
    fn copy_anoints_and_eldritch_implicits(&self, new: &mut Item, eldritch: bool) -> Result<()> {
        let tab = &self.build.items;
        let Some(base) = new.base.clone() else {
            return Ok(());
        };
        let typ = if base.weapon.is_some() {
            "Weapon 1"
        } else {
            base.typ.as_str()
        };
        let Some(sel) = tab.active_item_set().slots.get(typ).map(|s| s.sel_item_id) else {
            return Ok(());
        };
        let Some(current) = tab.item(sel).filter(|_| sel != 0.0) else {
            return Ok(());
        };
        if anointable(new) && new.enchant_mod_lines.is_empty() && sel > 0.0 {
            // Not an amulet with more than one anoint, as Stranglegasp
            if current.enchant_mod_lines.len() == 1 {
                new.enchant_mod_lines = current.enchant_mod_lines.clone();
            }
        }
        if new.influences[..6].iter().any(|i| *i) {
            return Ok(());
        }
        let modifiable = !(new.corrupted || new.mirrored);
        let (cleansing, tangle) = (INFLUENCES.len() - 2, INFLUENCES.len() - 1);
        if eldritch
            && ["Helmet", "Body Armour", "Gloves", "Boots"].contains(&base.typ.as_str())
            && ["NORMAL", "MAGIC", "RARE"].contains(&new.rarity.as_str())
            && new.implicit_mod_lines.is_empty()
            && modifiable
            && (current.influences[cleansing] || current.influences[tangle])
        {
            new.implicit_mod_lines = current.implicit_mod_lines.clone();
            new.influences[tangle] = current.influences[tangle];
            new.influences[cleansing] = current.influences[cleansing];
        }
        // Harvest and heist enchantments
        if (base.weapon.is_some() || base.typ == "Body Armour")
            && new.enchant_mod_lines.is_empty()
            && sel > 0.0
            && modifiable
        {
            new.enchant_mod_lines = current.enchant_mod_lines.clone();
        }
        self.build_and_parse_raw(new)
    }

    /// `CreateDisplayItemFromRaw(raw, true)`: leaves the display item as
    /// it is when the base is not recognised
    fn create_display_item_from_raw(&mut self, raw: &str) -> Result<()> {
        let Some(mut item) = self.new_item(raw)? else {
            return Ok(());
        };
        // `main.migrateEldritchImplicits`, true by default
        self.copy_anoints_and_eldritch_implicits(&mut item, true)?;
        item.normalise_quality();
        item.build_mod_list(&self.data.item_context())?;
        self.set_display(Some(item))
    }

    /// `SetDisplayItem(item)`, the item's side of it
    fn set_display(&mut self, item: Option<Item>) -> Result<()> {
        if let Some(item) = &item
            && item.crafted
            && item.cluster_jewel_size_index.is_some()
        {
            // `UpdateClusterJewelControls` crafts it again (`Item:Craft`)
            return Err(format!("{NOT_PORTED}: crafting a cluster jewel"));
        }
        self.display_item = item.map(|mut item| {
            if item.catalyst.is_none_or(|c| c == 0) {
                item.catalyst_quality = None;
            }
            item
        });
        Ok(())
    }

    /// `AddItem(item, noAutoEquip)`: the item's id
    fn add_item(&mut self, mut item: Item, no_auto_equip: bool) -> Result<f64> {
        let new = item.id.is_none();
        if new {
            let mut id = 1.0;
            while self.build.items.item(id).is_some() {
                id += 1.0;
            }
            item.id = Some(id);
        }
        let id = item.id.unwrap();
        item.build_mod_list(&self.data.item_context())?;
        let cluster = item.cluster_jewel_size_index.is_some();
        let replacing = self.build.items.items.insert(id as i64, Arc::new(item));
        if new {
            self.build.items.item_order_list.push(id);
            self.item_keys.insert(id as u32, ());
            if !no_auto_equip {
                let ring3 = self.sidebar()?.additional_ring_slot;
                let item = self.build.items.item(id).unwrap();
                let slot = (0..self.build.items.ordered_slots.len()).find(|&i| {
                    let s = &self.build.items.ordered_slots[i];
                    s.node_id.is_none()
                        && s.sel_item_id == 0.0
                        && self.slot_shown(i, ring3)
                        && self.valid_for_slot(item, &s.slot_name)
                });
                if let Some(i) = slot {
                    self.set_sel_item(i, id)?;
                }
            }
        }
        if let Some(old) = replacing {
            let rebuild = old.cluster_jewel_size_index.is_some()
                || cluster
                || old.base_name.as_deref() == Some("Timeless Jewel");
            if rebuild && self.build.spec.jewels.values().any(|j| *j == id) {
                let b = &mut self.build;
                Arc::make_mut(&mut b.spec)
                    .rebuild_cluster_jewel_graphs(&b.items, &self.data.game.items)?;
            }
        }
        Ok(id)
    }

    /// `AddDisplayItem()`
    fn add_display_item(&mut self) -> Result<()> {
        let item = self.display_item.take().ok_or("no item in the editor")?;
        if let Some((_, other)) = FORBIDDEN
            .iter()
            .find(|(t, _)| item.title.as_deref() == Some(t))
        {
            // `UpdateForbiddenJewelCounterpart` and
            // `AddForbiddenJewelCounterpart` read the unique database
            let variant = |i: &Item| {
                let v = i.variant.filter(|v| *v >= 1.0)? as usize;
                i.variant_list.as_ref()?.get(v - 1).cloned()
            };
            let name = variant(&item);
            let old = item.id.and_then(|id| self.build.items.item(id));
            let changed = old.is_some_and(|o| o.title == item.title && variant(o) != name);
            let paired = self
                .build
                .items
                .items
                .values()
                .any(|i| i.title.as_deref() == Some(other) && variant(i) == name);
            if name.is_some() && (changed || !paired) {
                return Err(format!("{NOT_PORTED}: the Forbidden jewels' counterparts"));
            }
        }
        self.add_item(item, false)?;
        self.items_changed()
    }

    /// `PopulateSlots()` and `buildFlag`
    fn items_changed(&mut self) -> Result<()> {
        self.populate_slots();
        self.rebuild()
    }

    /// `addItem`: the item from its text, equipped in the slot or where
    /// upstream would auto-equip it
    pub fn add_item_text(&mut self, text: &str, slot: Option<&str>) -> Result<J> {
        self.create_display_item_from_raw(text)?;
        let id = match slot {
            Some(slot) => {
                let item = self.display_item.take().ok_or("unrecognised item")?;
                let i = self
                    .build
                    .items
                    .ordered_slots
                    .iter()
                    .position(|s| s.slot_name == slot)
                    .ok_or_else(|| format!("{NOT_PORTED}: no slot {slot}"))?;
                let id = self.add_item(item, true)?;
                self.set_sel_item(i, id)?;
                self.items_changed()?;
                id
            }
            None => {
                let item = self.display_item.as_ref().ok_or("unrecognised item")?;
                let id = item.id;
                self.add_display_item()?;
                match id {
                    Some(id) => id,
                    // The id `AddItem` gave it: the last one listed
                    None => *self.build.items.item_order_list.last().unwrap(),
                }
            }
        };
        Ok(json!({ "id": super::num(id) }))
    }

    /// `editItem`: replaces an item's text, keeping its id and slots
    pub fn edit_item(&mut self, id: f64, text: &str) -> Result<J> {
        if self.build.items.item(id).is_none() {
            return Err(format!("no item {}", engine::pat::num(id)));
        }
        let mut item = self.new_item(text)?.ok_or("unrecognised item")?;
        item.id = Some(id);
        self.add_item(item, true)?;
        self.items_changed()?;
        Ok(json!([]))
    }

    /// `setDisplayItem`: a copy of one of the build's items, an item from
    /// its text, or none
    pub fn set_display_item(&mut self, id: Option<f64>, text: Option<&str>) -> Result<J> {
        match (id, text) {
            (Some(id), _) => {
                let item = self
                    .build
                    .items
                    .item(id)
                    .ok_or_else(|| format!("no item {}", engine::pat::num(id)))?;
                let raw = item.build_raw();
                let mut copy = self.new_item(&raw)?.ok_or("unrecognised item")?;
                copy.id = item.id;
                self.set_display(Some(copy))?;
            }
            (None, Some(text)) => {
                self.create_display_item_from_raw(text)?;
                if self.display_item.is_none() {
                    return Err("unrecognised item".into());
                }
            }
            (None, None) => self.set_display(None)?,
        }
        Ok(json!([]))
    }

    /// `saveDisplayItem`: adds the display item, or replaces the item it
    /// was opened from
    pub fn save_display_item(&mut self) -> Result<J> {
        let id = self
            .display_item
            .as_ref()
            .ok_or("no item in the editor")?
            .id;
        self.add_display_item()?;
        Ok(match id {
            Some(id) => json!({ "id": super::num(id) }),
            None => json!([]),
        })
    }
}
