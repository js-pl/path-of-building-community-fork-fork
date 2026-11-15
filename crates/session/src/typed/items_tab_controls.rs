//! The Items tab's own controls around the slots and the lists: the item
//! set and passive tree drop-downs (`setSelect`, `specSelect`, whose lists
//! the tab's draw sets), the weapon set buttons (`weaponSwap1`,
//! `weaponSwap2`), Craft item... and Create custom..., and the Manage...
//! buttons' descriptions.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession, num};
use serde_json::{Value as J, json};

const PATHS: [&str; 8] = [
    "items.setSelect",
    "items.specSelect",
    "items.weaponSwap1",
    "items.weaponSwap2",
    "items.craftDisplayItem",
    "items.newDisplayItem",
    "items.setManage",
    "items.specButton",
];

impl<'d> TypedSession<'d> {
    pub fn is_items_tab_control(path: &str) -> bool {
        PATHS.contains(&path)
    }

    /// The item sets' titles, and the active one's row
    fn set_select_list(&self) -> (Vec<String>, usize) {
        let tab = &self.build.items;
        let mut sel = 1;
        let list = tab
            .item_set_order_list
            .iter()
            .enumerate()
            .map(|(i, id)| {
                if *id == tab.active_item_set_id {
                    sel = i + 1;
                }
                tab.item_sets
                    .get(&(*id as i64))
                    .and_then(|s| s.title.clone())
                    .unwrap_or_else(|| "Default".into())
            })
            .collect();
        (list, sel)
    }

    /// `treeTab:GetSpecList()`: the specs' titles, an older tree's with
    /// its version
    fn items_spec_list(&self) -> Vec<String> {
        let latest = self.data.game.latest_tree_version();
        let n = self.specs.1.len();
        (1..=n)
            .map(|i| {
                let version = if i == self.specs.0 || self.other_specs.is_empty() {
                    self.build.spec.tree.version.to_string()
                } else {
                    self.other_specs
                        .get(i - 1)
                        .and_then(|s| s.as_ref())
                        .map_or_else(String::new, |s| s.tree.version.to_string())
                };
                let title = self.specs.1[i - 1].clone();
                if version != latest {
                    let display = self
                        .data
                        .game
                        .tree_version(&version)
                        .map_or(version.clone(), |v| v.display.clone());
                    format!("[{display}] {title}")
                } else {
                    title
                }
            })
            .collect()
    }

    /// `controls` of one of the controls
    pub fn items_tab_control_json(&self, path: &str) -> Option<J> {
        let button = |label: &str, shown: bool| json!({ "class": "ButtonControl", "shown": shown, "enabled": true, "label": label });
        let no_item = self.display_item.is_none();
        Some(match path {
            "items.setSelect" => {
                let (list, sel) = self.set_select_list();
                json!({
                    "class": "DropDownControl", "shown": true,
                    "enabled": list.len() > 1, "list": list, "sel": num(sel as f64),
                })
            }
            "items.specSelect" => {
                let list = self.items_spec_list();
                json!({
                    "class": "DropDownControl", "shown": true,
                    "enabled": list.len() > 1, "list": list, "sel": num(self.specs.0 as f64),
                })
            }
            "items.weaponSwap1" => button("I", true),
            "items.weaponSwap2" => button("II", true),
            "items.craftDisplayItem" => button("Craft item...", no_item),
            "items.newDisplayItem" => button("Create custom...", no_item),
            "items.setManage" | "items.specButton" => button("Manage...", true),
            _ => return None,
        })
    }

    /// `control` of one of the controls
    pub fn items_tab_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let click = req["click"].as_bool() == Some(true);
        let sel = req["sel"].as_f64();
        match (path, sel, click) {
            ("items.setSelect", Some(sel), _) => {
                // `SetSel`: clamped, and only a change calls `selFunc`
                let (list, cur) = self.set_select_list();
                let sel = sel.min(list.len() as f64).max(1.0) as usize;
                if sel != cur {
                    let id = self.build.items.item_set_order_list[sel - 1];
                    self.select_item_set(id)?;
                }
            }
            ("items.specSelect", Some(sel), _) => {
                let n = self.items_spec_list().len();
                let sel = sel.min(n as f64).max(1.0) as usize;
                if sel != self.specs.0 {
                    self.ui.unsaved = true;
                    self.select_tree_spec(sel as f64)?;
                }
            }
            ("items.weaponSwap1" | "items.weaponSwap2", _, true) => {
                self.swap_weapon_set(path == "items.weaponSwap2")?;
            }
            ("items.craftDisplayItem", _, true) => {
                self.craft_item()?;
            }
            ("items.newDisplayItem", _, true) => {
                self.edit_display_item_text()?;
                self.refresh_item_popups();
            }
            _ => return Err(format!("{NOT_PORTED}: control {path}")),
        }
        Ok(json!([]))
    }

    /// `weaponSwap1` or `weaponSwap2`: the other weapon set, and a main
    /// socket group in a slot of the set left moves to the first group in
    /// the set used
    fn swap_weapon_set(&mut self, second: bool) -> Result<()> {
        let tab = &self.build.items;
        if tab.active_item_set().use_second_weapon_set == second {
            return Ok(());
        }
        let set_id = tab.active_item_set_id as i64;
        let set = self
            .build
            .items
            .item_sets
            .get_mut(&set_id)
            .ok_or("attempt to index a nil value")?;
        set.use_second_weapon_set = second;
        set.second_unset = false;
        let weapon_set = |s: &Self, slot: &str| {
            s.build
                .items
                .ordered_slots
                .iter()
                .find(|x| x.slot_name == slot)
                .and_then(|x| x.weapon_set)
        };
        let (from, to) = if second { (1, 2) } else { (2, 1) };
        let groups = self.groups();
        let main = self.build.info.main_socket_group;
        let in_set = |s: &Self, g: &engine::skills::SocketGroup, set: u8| {
            g.slot
                .as_deref()
                .is_some_and(|slot| weapon_set(s, slot) == Some(set))
        };
        let main_group = (main as usize).checked_sub(1).and_then(|i| groups.get(i));
        if main_group.is_some_and(|g| in_set(self, g, from))
            && let Some(i) = groups.iter().position(|g| in_set(self, g, to))
        {
            self.build.info.main_socket_group = (i + 1) as f64;
        }
        self.ui.unsaved = true;
        self.rebuild()
    }
}
