//! Items dropped on an item slot (`ItemSlotClass:CanReceiveDrag` and
//! `ReceiveDrag`, the slots as `slots.<name>`) or on the sidebar's minion
//! drop-down (`build.mainSkillMinion`'s `ReceiveDrag`:
//! `ItemsTab:EquipItemInSet` into the item set it shows), from the item
//! list, the item databases or the shared item list.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::sidebar::MinionEntry;
use super::{Result, TypedSession};
use engine::item::Item;

const MISSING: &str = "attempt to index a nil value";

impl<'d> TypedSession<'d> {
    /// `IsItemValidForSlot(item, slotName, itemSet)`
    fn valid_for_slot_in_set(&self, item: &Item, slot: &str, set: f64) -> bool {
        let info = &self.data.game.setup.weapon_type_info;
        let one_hand = |t: &str| info.get(t).map(|w| w.one_hand);
        let tab = &self.build.items;
        tab.item_sets
            .get(&(set as i64))
            .is_some_and(|s| tab.valid_for_slot_in(item, slot, s, self.build.spec.tree, &one_hand))
    }

    /// An item the build holds (`value.id and itemsTab.items[value.id]`)
    fn held_item_id(&self, item: &Item) -> Option<f64> {
        item.id.filter(|id| self.build.items.item(*id).is_some())
    }

    /// The slot's `CanReceiveDrag` and `ReceiveDrag`: whether it took the
    /// item. Another list's item is added first, its quality normalised.
    pub(super) fn drop_on_slot(&mut self, name: &str, item: &Item) -> Result<bool> {
        let i = self
            .build
            .items
            .ordered_slots
            .iter()
            .position(|s| s.slot_name == name)
            .ok_or_else(|| format!("no control slots.{name}"))?;
        if !self.valid_for_slot(item, name) {
            return Ok(false);
        }
        match self.held_item_id(item) {
            Some(id) => self.set_sel_item(i, id)?,
            None => {
                let mut new = self.item_from_raw(&item.raw)?;
                new.normalise_quality();
                let id = self.add_item(new, true)?;
                self.set_sel_item(i, id)?;
                self.add_forbidden_jewel_counterpart(id)?;
            }
        }
        self.populate_slots();
        self.rebuild()?;
        Ok(true)
    }

    /// The minion drop-down's `CanReceiveDrag` (an item set selected, and
    /// a slot the main skill's minion uses) and `ReceiveDrag`
    pub(super) fn drop_on_minion(&mut self, item: &Item) -> Result<bool> {
        let c = &self.controls.skill_select;
        let entry = (c.minion.sel as usize)
            .checked_sub(1)
            .and_then(|i| c.minion_entry(i));
        let Some(MinionEntry::ItemSet(set)) = entry else {
            return Ok(false);
        };
        let slot = item.primary_slot();
        if !c.minion_uses.as_ref().is_some_and(|u| u.contains(&slot)) {
            return Ok(false);
        }
        self.equip_item_in_set(item, set)?;
        Ok(true)
    }

    /// `ItemsTab:EquipItemInSet(item, itemSetId)`, without Shift
    fn equip_item_in_set(&mut self, item: &Item, set_id: f64) -> Result<()> {
        let tab = &self.build.items;
        let set = tab.item_sets.get(&(set_id as i64)).ok_or(MISSING)?;
        let mut slot = item.primary_slot();
        let s = tab
            .ordered_slots
            .iter()
            .find(|s| s.slot_name == slot)
            .ok_or(MISSING)?;
        if s.weapon_set == Some(1) && set.use_second_weapon_set {
            // Redirect to second weapon set
            slot += " Swap";
        }
        let id = match self.held_item_id(item) {
            Some(id) => id,
            None => {
                let new = self.item_from_raw(&item.raw)?;
                self.add_item(new, true)?
            }
        };
        let alt = engine::pat::gsub(&slot, "1", "2");
        if set_id == self.build.items.active_item_set_id {
            let i = self
                .build
                .items
                .ordered_slots
                .iter()
                .position(|s| s.slot_name == slot)
                .ok_or(MISSING)?;
            self.set_sel_item(i, id)?;
        } else {
            let set = self
                .build
                .items
                .item_sets
                .get_mut(&(set_id as i64))
                .ok_or(MISSING)?;
            set.slots.get_mut(&slot).ok_or(MISSING)?.sel_item_id = id;
            let alt_id = set.slots.get(&alt).ok_or(MISSING)?.sel_item_id;
            if alt_id != 0.0 {
                let alt_item = self.build.items.item(alt_id).cloned().ok_or(MISSING)?;
                if !self.valid_for_slot_in_set(&alt_item, &alt, set_id)
                    && let Some(e) = self
                        .build
                        .items
                        .item_sets
                        .get_mut(&(set_id as i64))
                        .and_then(|s| s.slots.get_mut(&alt))
                {
                    e.sel_item_id = 0.0;
                }
            }
        }
        self.populate_slots();
        self.rebuild()
    }
}
