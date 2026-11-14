//! The Items tab's list of the build's items (`ItemListControl`,
//! `items.itemList`): its rows and selection, a row's click (Ctrl to
//! equip, a double click to edit), the loadout filter, and the Sort, Del
//! Unused, Del All and Delete buttons with their confirmations; the
//! loadouts are `buildMode:SyncLoadouts`'s.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::{ConfirmAction, ItemPopup};
use super::tooltip::Tooltip;
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::calcs::lua_sort;
use serde_json::{Value as J, json};
use std::collections::HashSet;

/// The list's state as upstream keeps it on the control
#[derive(Clone, Debug, Default)]
pub struct ItemList {
    /// `selIndex`, `selValue`
    sel_index: Option<usize>,
    sel_value: Option<f64>,
    /// `controls.loadoutFilter`'s list and selection (1-based)
    filter_list: Vec<String>,
    filter_sel: usize,
    /// `loadoutListKey`, `lastOutputRevision`
    loadout_key: Option<String>,
    last_rev: Option<f64>,
    /// `self.list` when filtered; `None` is `itemOrderList` itself
    list: Option<Vec<f64>>,
}

/// The paths of the list and its inputs
const INPUTS: [&str; 5] = ["loadoutFilter", "sort", "deleteUnused", "deleteAll", "delete"];

fn parse(path: &str) -> Option<Option<&str>> {
    let rest = path.strip_prefix("items.itemList")?;
    if rest.is_empty() {
        return Some(None);
    }
    let input = rest.strip_prefix('.')?;
    INPUTS.contains(&input).then_some(Some(input))
}

/// `treeVersions[v].display`: "3_25_ruthless_alternate" is "3.25
/// (ruthless alternate)"
fn version_display(v: &str) -> String {
    let parts: Vec<&str> = v.split('_').collect();
    let base = parts.iter().take(2).copied().collect::<Vec<_>>().join(".");
    if parts.len() > 2 {
        format!("{base} ({})", parts[2..].join(" "))
    } else {
        base
    }
}

/// `title:match("%{([%w,]+)%}")`, and the title without it, trimmed
fn link_of(title: &str) -> Option<(String, String)> {
    let id = engine::pat::capture(title, "%{([%w,]+)%}")?;
    let name = title.replacen(&format!("{{{id}}}"), "", usize::MAX);
    let name = name.trim_matches(|c: char| c.is_ascii_whitespace());
    let name = if name.is_empty() { "Default" } else { name };
    Some((id, name.to_string()))
}

/// The loadouts `buildMode:SyncLoadouts` lists, and the special links of
/// item sets and specs by link id (`setId` of each)
struct Loadouts {
    list: Vec<String>,
    item_links: Vec<(String, f64)>,
    tree_links: Vec<(String, usize)>,
}

impl<'d> TypedSession<'d> {
    pub fn is_item_list_control(path: &str) -> bool {
        parse(path).is_some()
    }

    /// The title of spec `i` (1-based) and its tree version
    fn spec_entry(&self, i: usize) -> Option<(String, String, &engine::spec::PassiveSpec<'d>)> {
        let title = self.specs.1.get(i.wrapping_sub(1))?.clone();
        let spec = if i == self.specs.0 {
            &*self.build.spec
        } else {
            &**self.other_specs.get(i - 1)?.as_ref()?
        };
        Some((title, spec.tree.version.clone(), spec))
    }

    fn spec_count(&self) -> usize {
        self.specs.1.len()
    }

    /// `buildMode:SyncLoadouts()`'s list
    fn loadouts(&self) -> Loadouts {
        let latest = self.data.game.latest_tree_version();
        let tab = &self.build.items;
        let mut tree_list: Vec<String> = Vec::new();
        let mut tree_links: Vec<(String, String, usize)> = Vec::new();
        for i in 1..=self.spec_count() {
            let Some((title, version, _)) = self.spec_entry(i) else {
                continue;
            };
            match link_of(&title) {
                Some((ids, name)) => {
                    for id in ids.split(',').filter(|s| !s.is_empty()) {
                        tree_links.push((id.to_string(), name.clone(), i));
                    }
                }
                None => {
                    let prefix = if version != latest {
                        format!("[{}] ", version_display(&version))
                    } else {
                        String::new()
                    };
                    tree_list.push(prefix + &title);
                }
            }
        }
        // `identifyLinks`
        fn identify<'a>(
            sets: impl Iterator<Item = (f64, Option<&'a str>)>,
        ) -> (HashSet<String>, Vec<(String, f64)>) {
            let (mut titles, mut links) = (HashSet::new(), Vec::new());
            for (id, title) in sets {
                let title = title.unwrap_or("Default");
                match link_of(title) {
                    Some((ids, _)) => {
                        for l in ids.split(',').filter(|s| !s.is_empty()) {
                            links.retain(|(x, _): &(String, f64)| x != l);
                            links.push((l.to_string(), id));
                        }
                    }
                    None => {
                        titles.insert(title.to_string());
                    }
                }
            }
            (titles, links)
        }
        let item_sets = tab.item_set_order_list.iter().map(|id| {
            let t = tab.item_sets.get(&(*id as i64)).and_then(|s| s.title.as_deref());
            (*id, t)
        });
        let (item_titles, item_links) = identify(item_sets);
        let (skill_titles, skill_links, one_skill) = match &self.build.skills {
            Some(s) => {
                let sets = s.skill_set_order_list.iter().map(|id| {
                    let t = s.skill_sets.get(&(*id as i64)).and_then(|s| s.title.as_deref());
                    (*id, t)
                });
                let (t, l) = identify(sets);
                (t, l, s.skill_set_order_list.len() == 1)
            }
            None => (HashSet::new(), Vec::new(), false),
        };
        let config = &self.build.config;
        let sets = config.config_set_order_list.iter().map(|id| {
            let t = config.config_sets.get(&(*id as i64)).and_then(|s| s.title.as_deref());
            (*id, t)
        });
        let (config_titles, config_links) = identify(sets);
        let one_item = tab.item_set_order_list.len() == 1;
        let one_config = config.config_set_order_list.len() == 1;
        let has = |links: &[(String, f64)], id: &str| links.iter().any(|(x, _)| x == id);
        let mut list = Vec::new();
        for tree in &tree_list {
            if (one_item || item_titles.contains(tree))
                && (one_skill || skill_titles.contains(tree))
                && (one_config || config_titles.contains(tree))
            {
                list.push(tree.clone());
            }
        }
        // `treeListSpecialLinks[linkId]`: the last spec with the id
        let mut last_tree: Vec<(String, usize)> = Vec::new();
        for (id, name, i) in &tree_links {
            if (one_item || has(&item_links, id))
                && (one_skill || has(&skill_links, id))
                && (one_config || has(&config_links, id))
            {
                list.push(format!("{name} {{{id}}}"));
            }
            last_tree.retain(|(x, _)| x != id);
            last_tree.push((id.clone(), *i));
        }
        Loadouts {
            list,
            item_links,
            tree_links: last_tree,
        }
    }

    /// `ItemListClass:UpdateLoadoutList()`: whether the list changed
    fn update_loadout_list(&self, s: &mut ItemList) -> bool {
        let mut list: Vec<String> = ["Any Loadout", "Current Loadout", "Unused Items"]
            .map(String::from)
            .to_vec();
        for v in self.loadouts().list {
            if !list.contains(&v) {
                list.push(v);
            }
        }
        let tab = &self.build.items;
        for id in &tab.item_set_order_list {
            if let Some(set) = tab.item_sets.get(&(*id as i64)) {
                let title = set.title.clone().unwrap_or_else(|| "Default".into());
                if !list.contains(&title) {
                    list.push(title);
                }
            }
        }
        let key = list.join("\0");
        if s.loadout_key.as_deref() == Some(key.as_str()) {
            return false;
        }
        s.loadout_key = Some(key);
        let sel = s.filter_sel.max(1);
        let value = s
            .filter_list
            .get(sel - 1)
            .cloned()
            .unwrap_or_else(|| "Any Loadout".into());
        s.filter_sel = list.iter().position(|v| *v == value).map_or(1, |i| i + 1);
        s.filter_list = list;
        true
    }

    /// `GetEquippedSlotForItem(item)`: the slot's index, and the other
    /// item set holding it there
    fn equipped_slot_for_item(&self, id: f64) -> Option<(usize, Option<f64>)> {
        let tab = &self.build.items;
        for (i, slot) in tab.ordered_slots.iter().enumerate() {
            if self.slot_inactive(i) {
                continue;
            }
            if slot.sel_item_id == id {
                return Some((i, None));
            }
            for set_id in &tab.item_set_order_list {
                if *set_id == tab.active_item_set_id {
                    continue;
                }
                let held = tab
                    .item_sets
                    .get(&(*set_id as i64))
                    .and_then(|s| s.slots.get(&slot.slot_name))
                    .is_some_and(|e| e.sel_item_id == id);
                if held {
                    return Some((i, Some(*set_id)));
                }
            }
        }
        None
    }

    fn item_set_title(&self, id: f64) -> String {
        self.build
            .items
            .item_sets
            .get(&(id as i64))
            .and_then(|s| s.title.clone())
            .unwrap_or_else(|| "Default".into())
    }

    /// `ItemListClass:FindSocketedJewel(jewelId, excludeActiveSpec)`
    fn find_socketed_jewel(&self, id: f64, exclude_active: bool) -> Option<String> {
        let item = self.build.items.item(id)?;
        if item.typ.as_deref() != Some("Jewel") {
            return None;
        }
        let mut equip = None;
        let mut match_active = false;
        for i in (1..=self.spec_count()).rev() {
            let Some((title, _, spec)) = self.spec_entry(i) else {
                continue;
            };
            for (node, item_id) in &spec.jewels {
                if *item_id == id && spec.alloc_nodes.contains(*node) {
                    if exclude_active && (i == self.specs.0 || match_active) {
                        equip = None;
                        match_active = true;
                    } else {
                        equip = Some(title.clone());
                    }
                }
            }
        }
        equip
    }

    /// `ItemListClass:FindEquippedAbyssJewel(jewelId, excludeActiveSet)`
    fn find_equipped_abyss_jewel(&self, id: f64, exclude_active: bool) -> Option<String> {
        let tab = &self.build.items;
        let item = tab.item(id)?;
        let abyss = item
            .base
            .as_ref()
            .and_then(|b| b.sub_type.as_deref())
            .is_some_and(|t| t == "Abyss");
        if !abyss {
            return None;
        }
        let mut equip = None;
        let mut match_active = false;
        for (set_id, set) in &tab.item_sets {
            for e in set.slots.values() {
                if e.sel_item_id == id {
                    if exclude_active && (*set_id as f64 == tab.active_item_set_id || match_active)
                    {
                        equip = None;
                        match_active = true;
                    } else {
                        equip = Some(set.title.clone().unwrap_or_else(|| "Default".into()));
                    }
                }
            }
        }
        equip
    }

    fn item_unused(&self, id: f64) -> bool {
        self.equipped_slot_for_item(id).is_none()
            && self.find_equipped_abyss_jewel(id, false).is_none()
            && self.find_socketed_jewel(id, false).is_none()
    }

    /// `ItemListClass:UpdateList()`
    fn update_list(&self, s: &mut ItemList) {
        self.update_loadout_list(s);
        let sel = s.filter_sel.max(1);
        let value = s
            .filter_list
            .get(sel - 1)
            .cloned()
            .unwrap_or_else(|| "Any Loadout".into());
        let tab = &self.build.items;
        if sel == 1 || value == "Any Loadout" {
            s.list = None;
        } else {
            let (mut set, mut spec): (Option<f64>, Option<usize>) = (None, None);
            if sel == 2 || value == "Current Loadout" {
                set = Some(tab.active_item_set_id);
                spec = Some(self.specs.0);
            } else if sel != 3 && value != "Unused Items" {
                let title = engine::pat::gsub(&value, "^%[[^%]]+%]%s*", "");
                set = tab
                    .item_set_order_list
                    .iter()
                    .find(|id| self.item_set_title(**id) == title)
                    .copied();
                spec = (1..=self.spec_count())
                    .find(|i| self.spec_entry(*i).is_some_and(|(t, _, _)| t == title));
                let link = engine::pat::capture(&value, "%{(%w+)%}");
                let l = self.loadouts();
                let item_link = link
                    .as_ref()
                    .and_then(|k| l.item_links.iter().find(|(x, _)| x == k).map(|(_, id)| *id));
                let tree_link = link
                    .as_ref()
                    .and_then(|k| l.tree_links.iter().find(|(x, _)| x == k).map(|(_, i)| *i));
                if set.is_none() {
                    set = if tab.item_set_order_list.len() == 1 {
                        tab.item_set_order_list.first().copied()
                    } else {
                        item_link.filter(|id| tab.item_sets.contains_key(&(*id as i64)))
                    };
                }
                if spec.is_none() {
                    spec = if self.spec_count() == 1 {
                        Some(1)
                    } else {
                        tree_link
                    };
                }
            }
            let unused = sel == 3 || value == "Unused Items";
            let mut list = Vec::new();
            for id in &tab.item_order_list {
                if tab.item(*id).is_none() {
                    continue;
                }
                let keep = if unused {
                    self.item_unused(*id)
                } else {
                    let in_set = set
                        .and_then(|s| tab.item_sets.get(&(s as i64)))
                        .is_some_and(|s| s.slots.values().any(|e| e.sel_item_id == *id));
                    in_set
                        || spec.and_then(|i| self.spec_entry(i)).is_some_and(|(_, _, sp)| {
                            sp.jewels
                                .iter()
                                .any(|(n, j)| *j == *id && sp.alloc_nodes.contains(*n))
                        })
                };
                if keep {
                    list.push(*id);
                }
            }
            s.list = Some(list);
        }
        let list = self.list_of(s);
        s.sel_index = s
            .sel_value
            .and_then(|v| list.iter().position(|x| *x == v))
            .map(|i| i + 1);
        s.sel_value = s.sel_index.and_then(|i| list.get(i - 1).copied());
    }

    fn list_of(&self, s: &ItemList) -> Vec<f64> {
        s.list
            .clone()
            .unwrap_or_else(|| self.build.items.item_order_list.clone())
    }

    /// What drawing the list does first (`ItemsTab:Draw`'s
    /// `UpdateSockets`, then `ItemListClass:Draw`'s updates)
    fn refresh_item_list(&self) {
        let mut s = self.editor.list.borrow_mut();
        let changed = self.update_loadout_list(&mut s);
        let rev = self.ui.output_revision;
        if changed || s.last_rev != Some(rev) {
            s.last_rev = Some(rev);
            self.update_list(&mut s);
        }
    }

    /// `GetRowValue(1, index, itemId)`
    fn item_row(&self, id: f64) -> Result<String> {
        let item = self
            .build
            .items
            .item(id)
            .ok_or_else(|| format!("{NOT_PORTED}: a deleted item in the item list"))?;
        let used = match self
            .find_equipped_abyss_jewel(id, true)
            .or_else(|| self.find_socketed_jewel(id, true))
        {
            Some(t) => format!("  ^9(Used in '{t}')"),
            None => match self.equipped_slot_for_item(id) {
                None => "  ^9(Unused)".into(),
                Some((_, Some(set))) => format!("  ^9(Used in '{}')", self.item_set_title(set)),
                Some((_, None)) => String::new(),
            },
        };
        Ok(format!(
            "{}{}{used}",
            super::display_stats::rarity_color(&item.rarity),
            item.name
        ))
    }

    /// `controls` of the list or one of its inputs
    pub fn item_list_control_json(&self, path: &str) -> Result<J> {
        let input = parse(path).ok_or("no control")?;
        self.refresh_item_list();
        let s = self.editor.list.borrow().clone();
        let list = self.list_of(&s);
        Ok(match input {
            None => {
                let rows = list
                    .iter()
                    .map(|id| self.item_row(*id).map(J::from))
                    .collect::<Result<Vec<J>>>()?;
                let mut o = json!({
                    "class": "ItemListControl", "shown": true, "enabled": true,
                    "list": rows,
                });
                if s.sel_value.is_some()
                    && let Some(i) = s.sel_index
                {
                    o["sel"] = num(i as f64);
                }
                o
            }
            Some("loadoutFilter") => json!({
                "class": "DropDownControl", "shown": true, "enabled": true,
                "list": s.filter_list, "sel": s.filter_sel.max(1),
            }),
            Some(name) => {
                let (label, enabled) = match name {
                    "sort" => ("Sort", true),
                    "deleteUnused" => ("Del Unused", !list.is_empty()),
                    "deleteAll" => ("Del All", !list.is_empty()),
                    _ => ("Delete", s.sel_value.is_some()),
                };
                json!({
                    "class": "ButtonControl", "shown": true, "enabled": enabled, "label": label,
                })
            }
        })
    }

    /// `controlTooltip` of a row: `AddValueTooltip`
    pub fn item_list_tooltip(&mut self, index: Option<f64>) -> Result<J> {
        self.update_sockets();
        self.refresh_item_list();
        let mut tip = Tooltip::default();
        let list = self.list_of(&self.editor.list.borrow());
        let id = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0)
            .and_then(|i| list.get(i as usize - 1).copied());
        // No tooltip while a popup is open
        let popups = self.ui.popups.as_array().map_or(0, Vec::len);
        if let Some(id) = id
            && popups == 0
        {
            let item = self.build.items.item(id).ok_or("no item")?.clone();
            self.add_item_tooltip(&mut tip, &item, None)?;
        }
        Ok(tip.json())
    }

    /// `control` of the list or its inputs
    pub fn item_list_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let input = parse(path).ok_or("no control")?;
        if self.class_popups_open() > 0 {
            return Err(format!(
                "{NOT_PORTED}: the item list with a class change popup open"
            ));
        }
        self.update_sockets();
        self.refresh_item_list();
        let click = req["click"].as_bool().unwrap_or(false) || req["click"] == json!(1);
        match input {
            None => {
                if let Some(sel) = req["sel"].as_f64() {
                    self.item_list_select(sel, click, req)?;
                } else if req["key"].as_str() == Some("DELETE") {
                    self.item_list_delete_selected()?;
                } else {
                    return Err(format!("{NOT_PORTED}: control {path}"));
                }
            }
            Some("loadoutFilter") => {
                let sel = req["sel"]
                    .as_f64()
                    .ok_or_else(|| format!("{NOT_PORTED}: control {path}"))?;
                let mut s = self.editor.list.borrow_mut();
                let len = s.filter_list.len();
                let new = (sel.min(len as f64).max(1.0)) as usize;
                if new != s.filter_sel.max(1) {
                    s.filter_sel = new;
                    self.update_list(&mut s);
                }
            }
            Some(name) if req["click"].as_bool() == Some(true) => match name {
                "sort" => self.sort_item_list()?,
                "deleteUnused" => self.delete_unused_items()?,
                "deleteAll" => {
                    if !self.list_of(&self.editor.list.borrow()).is_empty() {
                        self.open_confirm(
                            "Delete All",
                            "Are you sure you want to delete all items in this build?",
                            ConfirmAction::DeleteAll,
                        );
                    }
                }
                _ => self.item_list_delete_selected()?,
            },
            Some(_) => return Err(format!("{NOT_PORTED}: control {path}")),
        }
        Ok(json!([]))
    }

    /// `SelectIndex(sel)`, then `OnSelClick` for a click
    fn item_list_select(&mut self, sel: f64, click: bool, req: &J) -> Result<()> {
        let id = {
            let mut s = self.editor.list.borrow_mut();
            let list = self.list_of(&s);
            let value = (sel >= 1.0 && sel.fract() == 0.0)
                .then(|| list.get(sel as usize - 1).copied())
                .flatten();
            s.sel_value = value;
            if value.is_none() {
                return Ok(());
            }
            s.sel_index = Some(sel as usize);
            value.unwrap_or(0.0)
        };
        if !click {
            return Ok(());
        }
        let ctrl = req["ctrl"].as_bool().unwrap_or(false);
        let shift = req["shift"].as_bool().unwrap_or(false);
        let double = req["doubleClick"].as_bool().unwrap_or(false);
        if ctrl {
            let item = self.build.items.item(id).ok_or("no item")?.clone();
            let mut name = item.primary_slot();
            let tab = &self.build.items;
            let Some(i) = tab.ordered_slots.iter().position(|s| s.slot_name == name) else {
                return Ok(());
            };
            if tab.ordered_slots[i].weapon_set == Some(1)
                && tab.active_item_set().use_second_weapon_set
            {
                name += " Swap";
            }
            if shift {
                let alt = name.replace('1', "2");
                if self.valid_for_slot(&item, &alt) {
                    name = alt;
                }
            }
            let i = self
                .build
                .items
                .ordered_slots
                .iter()
                .position(|s| s.slot_name == name)
                .ok_or("attempt to index a nil value")?;
            let cur = self.build.items.ordered_slots[i].sel_item_id;
            self.set_sel_item(i, if cur == id { 0.0 } else { id })?;
            self.populate_slots();
            self.rebuild()?;
        } else if double {
            self.set_display_item(Some(id), None)?;
        }
        Ok(())
    }

    /// `OnSelDelete(selIndex, selValue)`: at once, or after a confirmation
    /// for an equipped item
    fn item_list_delete_selected(&mut self) -> Result<()> {
        let Some(id) = self.editor.list.borrow().sel_value else {
            return Ok(());
        };
        let item = self.build.items.item(id).ok_or("no item")?;
        let name = item.name.clone();
        let msg = if let Some((slot, set)) = self.equipped_slot_for_item(id) {
            let label = self.slot_label(&self.build.items.ordered_slots[slot]);
            let in_set = set.map_or(String::new(), |s| {
                format!(" in set '{}'", self.item_set_title(s))
            });
            Some(format!("{name} is currently equipped in {label}{in_set}."))
        } else if let Some(set) = self.find_equipped_abyss_jewel(id, true) {
            Some(format!(
                "{name} is currently equipped in an Abyssal Socket in set '{set}'."
            ))
        } else {
            self.find_socketed_jewel(id, true).map(|tree| {
                format!("{name} is currently equipped in passive tree '{tree}'.")
            })
        };
        match msg {
            Some(m) => self.open_confirm(
                "Delete Item",
                &format!("{m}\nAre you sure you want to delete it?"),
                ConfirmAction::DeleteItem(id),
            ),
            None => self.list_delete_item(id)?,
        }
        Ok(())
    }

    /// `DeleteItem(item)`, then the selection cleared and `UpdateList()`
    fn list_delete_item(&mut self, id: f64) -> Result<()> {
        self.delete_item(id)?;
        let mut s = self.editor.list.borrow_mut();
        s.sel_index = None;
        s.sel_value = None;
        self.update_list(&mut s);
        Ok(())
    }

    fn open_confirm(&mut self, title: &str, msg: &str, action: ConfirmAction) {
        self.editor.popups.insert(
            0,
            ItemPopup::Confirm {
                title: title.to_string(),
                lines: msg.split('\n').map(String::from).collect(),
                action,
            },
        );
        self.refresh_item_popups();
    }

    /// The confirmation's Delete
    pub(super) fn confirm_action(&mut self, action: &ConfirmAction) -> Result<()> {
        match action {
            ConfirmAction::DeleteItem(id) => self.list_delete_item(*id),
            ConfirmAction::DeleteAll => self.delete_all_items(),
        }
    }

    /// `ItemsTab:SortItemList()`, then `UpdateList()`
    fn sort_item_list(&mut self) -> Result<()> {
        let tab = &self.build.items;
        let order = |name: &str| {
            tab.ordered_slots
                .iter()
                .position(|s| s.slot_name == name)
                .map(|i| i + 1)
        };
        let set_pos = |set: f64| tab.item_set_order_list.iter().position(|x| *x == set);
        struct Key {
            id: f64,
            prim: String,
            name: String,
            equip: Option<(usize, Option<f64>)>,
        }
        let mut keys: Vec<Key> = tab
            .item_order_list
            .iter()
            .map(|id| {
                let item = tab.item(*id).ok_or("attempt to index a nil value")?;
                Ok(Key {
                    id: *id,
                    prim: item.primary_slot(),
                    name: item.name.clone(),
                    equip: self.equipped_slot_for_item(*id),
                })
            })
            .collect::<Result<_>>()?;
        lua_sort(&mut keys, |a, b| {
            if a.prim != b.prim {
                return match (order(&a.prim), order(&b.prim)) {
                    (None, _) => false,
                    (_, None) => true,
                    (Some(x), Some(y)) => x < y,
                };
            }
            match (a.equip, b.equip) {
                (Some((sa, ea)), Some((sb, eb))) => {
                    if sa != sb {
                        return sa < sb;
                    }
                    match (ea, eb) {
                        (Some(_), None) => return false,
                        (None, Some(_)) => return true,
                        (Some(x), Some(y)) => return set_pos(x) < set_pos(y),
                        (None, None) => {}
                    }
                }
                (Some(_), None) => return true,
                (None, Some(_)) => return false,
                (None, None) => {}
            }
            a.name < b.name
        });
        self.build.items.item_order_list = keys.into_iter().map(|k| k.id).collect();
        let mut s = self.editor.list.borrow_mut();
        self.update_list(&mut s);
        Ok(())
    }

    /// Del Unused
    fn delete_unused_items(&mut self) -> Result<()> {
        let unused: Vec<f64> = self
            .build
            .items
            .item_order_list
            .iter()
            .copied()
            .filter(|id| self.item_unused(*id))
            .collect();
        for id in unused.iter().rev() {
            self.delete_item(*id)?;
        }
        self.populate_slots();
        self.rebuild()?;
        let mut s = self.editor.list.borrow_mut();
        self.update_list(&mut s);
        Ok(())
    }

    /// Del All's Delete
    fn delete_all_items(&mut self) -> Result<()> {
        for i in 0..self.build.items.ordered_slots.len() {
            self.set_sel_item(i, 0.0)?;
        }
        let specs = std::iter::once(&mut self.build.spec).chain(self.other_specs.iter_mut().flatten());
        for spec in specs {
            if spec.jewels.values().any(|j| *j != 0.0) {
                let spec = std::sync::Arc::make_mut(spec);
                for j in spec.jewels.values_mut() {
                    *j = 0.0;
                }
            }
        }
        let tab = &mut self.build.items;
        tab.item_order_list.clear();
        tab.items.clear();
        self.item_keys = Default::default();
        self.populate_slots();
        self.rebuild()?;
        let mut s = self.editor.list.borrow_mut();
        s.sel_index = None;
        s.sel_value = None;
        self.update_list(&mut s);
        Ok(())
    }
}
