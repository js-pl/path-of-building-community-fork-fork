//! The items of a build and what is equipped: upstream's Items tab
//! (`Classes/ItemsTab.lua`, `Classes/ItemSlotControl.lua`) without its UI.
//! Loads the items and item sets, and settles which item each slot holds,
//! as the tab's slot lists do.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::item::{Item, ItemContext};
use crate::spec::PassiveSpec;
use crate::xml::{Element, Node};
use std::collections::{BTreeMap, HashMap};

pub type Result<T> = std::result::Result<T, String>;

/// The slots of the tab without jewel sockets, in order (`baseSlots`)
pub const BASE_SLOTS: [&str; 18] = [
    "Weapon 1",
    "Weapon 2",
    "Helmet",
    "Body Armour",
    "Gloves",
    "Boots",
    "Amulet",
    "Ring 1",
    "Ring 2",
    "Ring 3",
    "Belt",
    "Graft 1",
    "Graft 2",
    "Flask 1",
    "Flask 2",
    "Flask 3",
    "Flask 4",
    "Flask 5",
];

/// A slot an item can go in.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub slot_name: String,
    /// 1 or 2 for weapon slots and their abyssal sockets
    pub weapon_set: Option<u8>,
    /// The slot an abyssal socket belongs to
    pub parent_slot: Option<String>,
    /// The number at the end of the name, else the first in it (`Ring 2`
    /// is 2, `Weapon 1 Swap` 1)
    pub slot_num: Option<f64>,
    /// The passive node of a jewel socket
    pub node_id: Option<u32>,
    pub sel_item_id: f64,
    /// For flasks and tinctures, whether they are in use
    pub active: bool,
}

/// What an item set has in a slot
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotSelection {
    pub sel_item_id: f64,
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemSet {
    pub id: f64,
    pub title: Option<String>,
    pub use_second_weapon_set: bool,
    pub slots: HashMap<String, SlotSelection>,
}

#[derive(Clone, Debug, Default)]
pub struct ItemsTab {
    pub items: BTreeMap<i64, Item>,
    /// Item ids in the order the build lists them
    pub item_order_list: Vec<f64>,
    pub item_sets: BTreeMap<i64, ItemSet>,
    pub item_set_order_list: Vec<f64>,
    pub active_item_set_id: f64,
    /// All slots in the tab's order: base slots with their swap and
    /// abyssal slots, then jewel sockets by node id
    pub ordered_slots: Vec<Slot>,
}

/// `tonumber(slotName:match("%d+$") or slotName:match("%d+"))`
fn slot_num(name: &str) -> Option<f64> {
    crate::pat::capture(name, "%d+$")
        .or_else(|| crate::pat::capture(name, "%d+"))
        .and_then(|n| crate::pat::to_number(&n))
}

fn num(v: Option<&str>) -> Option<f64> {
    v.and_then(crate::pat::to_number)
}

/// The slots of a tab: `socket_ids` are the jewel sockets of the latest
/// tree
pub fn ordered_slots(socket_ids: &[u32]) -> Vec<Slot> {
    let slot = |name: String, weapon_set: Option<u8>, parent: Option<String>| Slot {
        slot_num: slot_num(&name),
        slot_name: name,
        weapon_set,
        parent_slot: parent,
        node_id: None,
        sel_item_id: 0.0,
        active: false,
    };
    let mut out = Vec::new();
    for name in BASE_SLOTS {
        let weapon = name.contains("Weapon");
        out.push(slot(name.into(), weapon.then_some(1), None));
        if weapon {
            let swap = format!("{name} Swap");
            out.push(slot(swap.clone(), Some(2), None));
            for i in 1..=6 {
                out.push(slot(
                    format!("{name} Swap Abyssal Socket {i}"),
                    Some(2),
                    Some(swap.clone()),
                ));
            }
        }
        if matches!(
            name,
            "Weapon 1" | "Weapon 2" | "Helmet" | "Gloves" | "Body Armour" | "Boots" | "Belt"
        ) {
            for i in 1..=6 {
                out.push(slot(
                    format!("{name} Abyssal Socket {i}"),
                    weapon.then_some(1),
                    Some(name.into()),
                ));
            }
        }
    }
    let mut ids = socket_ids.to_vec();
    ids.sort();
    for id in ids {
        out.push(Slot {
            node_id: Some(id),
            ..slot(format!("Jewel {id}"), None, None)
        });
    }
    out
}

/// What checking a slot needs of the tree: a jewel socket's kind
pub trait Sockets {
    /// Whether a node is a jewel socket of the tree and the size of its
    /// cluster socket (`expansionJewel.size`), if it is one, and whether it
    /// is a charm socket
    fn socket(&self, node_id: u32) -> Option<(Option<f64>, bool)>;
}

impl ItemsTab {
    /// `ItemsTabClass:Load(xml)`: the items and item sets of `<Items>`
    pub fn load(xml: &Element, ctx: &ItemContext, socket_ids: &[u32]) -> Result<ItemsTab> {
        let mut tab = ItemsTab {
            ordered_slots: ordered_slots(socket_ids),
            ..ItemsTab::default()
        };
        // Slots before item sets: they go in the first set if there are none
        let mut legacy: Vec<(String, SlotSelection)> = Vec::new();
        for node in xml.elements() {
            match node.elem.as_str() {
                "Item" => {
                    let mut item = Item::default();
                    item.parse_raw(ctx, "", None, false)?;
                    let id = num(node.attr("id"));
                    item.id = id;
                    item.variant = num(node.attr("variant"));
                    for (i, k) in [
                        "variantAlt",
                        "variantAlt2",
                        "variantAlt3",
                        "variantAlt4",
                        "variantAlt5",
                    ]
                    .iter()
                    .enumerate()
                    {
                        if let Some(v) = node.attr(k) {
                            item.has_alt_variant[i] = true;
                            item.variant_alt[i] = crate::pat::to_number(v);
                        }
                    }
                    for child in &node.children {
                        match child {
                            Node::Text(t) => item.parse_raw(ctx, t, None, false)?,
                            Node::Element(e) if e.elem == "ModRange" => {
                                // The range of a line, counted across the lists
                                let mut id = num(e.attr("id")).unwrap_or(0.0);
                                let range = num(e.attr("range")).unwrap_or(1.0);
                                for list in [
                                    &mut item.buff_mod_lines,
                                    &mut item.enchant_mod_lines,
                                    &mut item.scourge_mod_lines,
                                    &mut item.implicit_mod_lines,
                                    &mut item.explicit_mod_lines,
                                    &mut item.crucible_mod_lines,
                                ] {
                                    if id <= list.len() as f64 {
                                        if id >= 1.0 && id.fract() == 0.0 {
                                            list[id as usize - 1].range = Some(range);
                                        }
                                        break;
                                    }
                                    id -= list.len() as f64;
                                }
                            }
                            Node::Element(_) => {}
                        }
                    }
                    if item.base.is_some() {
                        item.build_mod_list(ctx)?;
                        let id = id.ok_or("item without an id")?;
                        tab.items.insert(id as i64, item);
                        tab.item_order_list.push(id);
                    }
                }
                "Slot" => {
                    let name = node.attr("name").unwrap_or("");
                    if tab
                        .ordered_slots
                        .iter()
                        .any(|s| s.slot_name == name && s.node_id.is_none())
                    {
                        legacy.push((
                            name.to_string(),
                            SlotSelection {
                                sel_item_id: num(node.attr("itemId")).unwrap_or(f64::NAN),
                                // Only flask slots have the check box
                                active: name.contains("Flask")
                                    && node.attr("active") == Some("true"),
                            },
                        ));
                    }
                }
                "ItemSet" => {
                    let id = num(node.attr("id")).unwrap_or(f64::NAN);
                    let mut set = tab.new_item_set(id);
                    set.title = node.attr("title").map(String::from);
                    set.use_second_weapon_set = node.attr("useSecondWeaponSet") == Some("true");
                    for child in node.elements() {
                        if child.elem == "Slot" {
                            let name = child.attr("name").unwrap_or("");
                            if let Some(s) = set.slots.get_mut(name) {
                                s.sel_item_id = num(child.attr("itemId")).unwrap_or(f64::NAN);
                                s.active = child.attr("active") == Some("true");
                            }
                        }
                    }
                    tab.item_sets.insert(id as i64, set);
                    tab.item_set_order_list.push(id);
                }
                _ => {}
            }
        }
        if tab.item_set_order_list.is_empty() {
            let mut set = tab.new_item_set(1.0);
            set.use_second_weapon_set = xml.attr("useSecondWeaponSet") == Some("true");
            for (name, sel) in legacy {
                set.slots.insert(name, sel);
            }
            tab.item_sets.insert(1, set);
            tab.item_set_order_list.push(1.0);
        }
        let active = num(xml.attr("activeItemSet")).unwrap_or(1.0);
        let active = if tab.item_sets.contains_key(&(active as i64)) && active.fract() == 0.0 {
            active
        } else {
            tab.item_set_order_list[0]
        };
        tab.active_item_set_id = active;
        Ok(tab)
    }

    /// `NewItemSet(id)`: every slot but the jewel sockets empty
    fn new_item_set(&self, id: f64) -> ItemSet {
        ItemSet {
            id,
            title: None,
            use_second_weapon_set: false,
            slots: self
                .ordered_slots
                .iter()
                .filter(|s| s.node_id.is_none())
                .map(|s| (s.slot_name.clone(), SlotSelection::default()))
                .collect(),
        }
    }

    pub fn active_item_set(&self) -> &ItemSet {
        &self.item_sets[&(self.active_item_set_id as i64)]
    }

    pub fn item(&self, id: f64) -> Option<&Item> {
        (id.fract() == 0.0)
            .then(|| self.items.get(&(id as i64)))
            .flatten()
    }

    /// `IsItemValidForSlot(item, slotName)`
    fn valid_for_slot(
        &self,
        item: &Item,
        slot_name: &str,
        sockets: &dyn Sockets,
        one_hand: &dyn Fn(&str) -> Option<bool>,
    ) -> bool {
        let (slot_type, slot_id) = match crate::pat::captures(slot_name, "^([%a ]+) (%d+)$") {
            Some(c) => (c[0].clone(), Some(c[1].clone())),
            None => (slot_name.to_string(), None),
        };
        let typ = item.typ.as_deref().unwrap_or("");
        let sub_type = item.base.as_ref().and_then(|b| b.sub_type.as_deref());
        if slot_type == "Jewel" {
            let node = slot_id
                .and_then(|i| i.parse::<u32>().ok())
                .and_then(|id| sockets.socket(id));
            let Some((expansion, charm)) = node else {
                return false;
            };
            if typ != "Jewel" {
                return false;
            }
            let cluster = item.cluster_jewel_size_index;
            if charm || sub_type == Some("Charm") {
                return charm && sub_type == Some("Charm");
            } else if cluster.is_some() && expansion.is_none() {
                return false;
            } else if expansion.is_none_or(|s| s == 2.0) {
                return true;
            }
            return cluster.is_none_or(|c| c <= expansion.unwrap_or(f64::NAN));
        }
        if typ == slot_type {
            return true;
        }
        if typ == "Tincture" && slot_type == "Flask" {
            return true;
        }
        if typ == "Jewel" && sub_type == Some("Abyss") && slot_name.contains("Abyssal Socket") {
            return true;
        }
        match slot_name {
            "Weapon 1" | "Weapon 1 Swap" | "Weapon" => {
                item.base.as_ref().is_some_and(|b| b.weapon.is_some())
            }
            "Weapon 2" | "Weapon 2 Swap" => {
                let set = self.active_item_set();
                let first = if slot_name == "Weapon 2" {
                    "Weapon 1"
                } else {
                    "Weapon 1 Swap"
                };
                let sel = set.slots.get(first).map_or(0.0, |s| s.sel_item_id);
                let weapon1 = self
                    .item(sel)
                    .and_then(|i| i.base.as_ref())
                    .map_or("None".to_string(), |b| b.typ.clone());
                if weapon1 == "None" {
                    typ == "Shield" || one_hand(typ) == Some(true)
                } else if weapon1 == "Bow" {
                    typ == "Quiver"
                } else if one_hand(&weapon1) == Some(true) {
                    typ == "Shield"
                        || (one_hand(typ) == Some(true)
                            && ((weapon1 == "Wand" && typ == "Wand")
                                || (weapon1 != "Wand" && typ != "Wand")))
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// `PopulateSlots()` after the tree is loaded: each slot takes the
    /// active set's item, or the spec's jewel, if it fits there
    pub fn populate_slots(
        &mut self,
        spec: &mut PassiveSpec,
        sockets: &dyn Sockets,
        one_hand: &dyn Fn(&str) -> Option<bool>,
    ) {
        let set_id = self.active_item_set_id as i64;
        for i in 0..self.ordered_slots.len() {
            let name = self.ordered_slots[i].slot_name.clone();
            let mut sel = match self.ordered_slots[i].node_id {
                Some(node) => spec.jewels.get(&node).copied().unwrap_or(0.0),
                None => {
                    let s = self.item_sets[&set_id]
                        .slots
                        .get(&name)
                        .cloned()
                        .unwrap_or_default();
                    self.ordered_slots[i].active = s.active;
                    s.sel_item_id
                }
            };
            let valid = self
                .item(sel)
                .is_some_and(|item| self.valid_for_slot(item, &name, sockets, one_hand));
            if !valid {
                sel = 0.0;
                match self.ordered_slots[i].node_id {
                    Some(node) => {
                        spec.jewels.insert(node, 0.0);
                    }
                    None => {
                        if let Some(s) = self
                            .item_sets
                            .get_mut(&set_id)
                            .and_then(|s| s.slots.get_mut(&name))
                        {
                            s.sel_item_id = 0.0;
                        }
                    }
                }
            }
            self.ordered_slots[i].sel_item_id = sel;
        }
    }
}

impl Sockets for crate::tree::PassiveTree {
    fn socket(&self, node_id: u32) -> Option<(Option<f64>, bool)> {
        self.node(node_id)
            .map(|n| (n.expansion_jewel.as_ref().map(|e| e.size), n.charm_socket))
    }
}
