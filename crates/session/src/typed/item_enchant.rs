//! The item editor's Enchant Item popup (`ItemsTab:EnchantDisplayItem`):
//! the skill (for helmets), the source and the enchantment, sorted by
//! the power of a stat as the calculator gives it with the enchanted item
//! in the item's slot.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession};
use engine::calc_override::Override;
use engine::calculator::MiscCalculator;
use engine::game_data::Enchantments;
use engine::item::Item;
use engine::item_enchant::{ENCHANTMENT_SOURCES, enchant};
use engine::power::{PowerStat, power_stats};
use serde_json::{Value as J, json};
use std::collections::{BTreeSet, HashMap};

/// `buildModSortList()`: "Default", then the power stats for nodes
pub(super) fn mod_sort_list() -> Vec<(String, Option<PowerStat>)> {
    let mut list = vec![("Default".to_string(), None)];
    for p in power_stats() {
        if p.stat.is_some() && !p.ignore_for_nodes {
            list.push((p.label.clone(), Some(p)));
        }
    }
    list
}

/// An entry of the enchantment list
#[derive(Clone, Debug)]
struct Entry {
    line: String,
    default_order: usize,
    sort_value: Option<f64>,
    sort_values: HashMap<String, f64>,
}

/// The popup's state; selections are 1-based as upstream's
#[derive(Clone, Debug)]
pub struct EnchantPopup {
    /// `self.enchantSlot`
    slot: usize,
    /// The key of the item's `enchantments`
    key: String,
    have_skills: bool,
    skills_used: BTreeSet<String>,
    skill_list: Vec<String>,
    skill_sel: usize,
    all_skills: bool,
    all_skills_enabled: bool,
    /// Indices into `ENCHANTMENT_SOURCES`
    source_list: Vec<usize>,
    source_sel: usize,
    sort_sel: usize,
    list: Vec<Entry>,
    sel: usize,
}

const NIL_INDEX: &str = "attempt to index a nil value";

/// `DropDownControl:SetSel(sel)`: the new selection, if it changed
fn new_sel(cur: usize, sel: f64, len: usize) -> Option<usize> {
    let s = sel.min(len as f64).max(1.0) as usize;
    (s != cur).then_some(s)
}

impl EnchantPopup {
    fn sources<'a>(&self, e: &'a Enchantments) -> Option<&'a [(String, Vec<String>)]> {
        if self.have_skills {
            let skill = self.skill_list.get(self.skill_sel.wrapping_sub(1))?;
            e.skills
                .iter()
                .find(|(s, _)| s == skill)
                .map(|(_, l)| l.as_slice())
        } else {
            Some(&e.sources)
        }
    }

    /// `buildSkillList(onlyUsedSkills)`
    fn build_skill_list(&mut self, e: &Enchantments, only_used: bool) {
        self.skill_list = e
            .skills
            .iter()
            .map(|(s, _)| s.clone())
            .filter(|s| !only_used || self.skills_used.is_empty() || self.skills_used.contains(s))
            .collect();
    }

    /// `buildEnchantmentSourceList()`
    fn build_source_list(&mut self, e: &Enchantments) -> Result<()> {
        let list = self.sources(e).ok_or(NIL_INDEX)?;
        self.source_list = ENCHANTMENT_SOURCES
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| list.iter().any(|(s, _)| s == name))
            .map(|(i, _)| i)
            .collect();
        Ok(())
    }

    /// `buildEnchantmentList()`, with `setDefaultSortOrder()`
    fn build_list(&mut self, e: &Enchantments) -> Result<()> {
        let list = self.sources(e).ok_or(NIL_INDEX)?;
        let source = self
            .source_list
            .get(self.source_sel.wrapping_sub(1))
            .ok_or(NIL_INDEX)?;
        let lines = Enchantments::lines(list, ENCHANTMENT_SOURCES[*source].0)
            .ok_or("bad argument #1 to 'ipairs' (table expected, got nil)")?;
        self.list = lines
            .iter()
            .enumerate()
            .map(|(i, l)| Entry {
                line: l.clone(),
                default_order: i + 1,
                sort_value: None,
                sort_values: HashMap::new(),
            })
            .collect();
        Ok(())
    }

    /// `controls.enchantment:SetSel(sel)`
    fn set_sel(&mut self, sel: f64) {
        if let Some(s) = new_sel(self.sel, sel, self.list.len()) {
            self.sel = s;
        }
    }
}

impl<'d> TypedSession<'d> {
    fn enchantments_of(&self, item: &Item) -> Option<&'d Enchantments> {
        let base = item.base.as_ref()?;
        let key = if base.flask.is_some() {
            "Flask"
        } else {
            base.typ.as_str()
        };
        self.data.game.enchantments(key)
    }

    /// The popup's `enchantments`
    fn popup_enchantments(&self, p: &EnchantPopup) -> Result<&'d Enchantments> {
        self.data
            .game
            .enchantments(&p.key)
            .ok_or_else(|| "no enchantments".to_string())
    }

    /// Whether the item has `enchantments`
    pub(super) fn has_enchantments(&self, item: &Item) -> bool {
        self.enchantments_of(item).is_some()
    }

    /// `ItemsTab:EnchantDisplayItem(enchantSlot)`
    pub(super) fn enchant_display_item(&mut self, slot: usize) -> Result<()> {
        let item = self.display_item.as_ref().ok_or("no item in the editor")?;
        let e = self.enchantments_of(item).ok_or(NIL_INDEX)?;
        let base = item.base.as_ref().ok_or("no base")?;
        let key = if base.flask.is_some() {
            "Flask".to_string()
        } else {
            base.typ.clone()
        };
        let have_skills = e.sources.is_empty();
        let mut skills_used = BTreeSet::new();
        if have_skills {
            let groups = self
                .build
                .skills
                .as_ref()
                .map_or(&[][..], |s| s.socket_group_list());
            for g in groups {
                for gem in &g.gem_list {
                    let Some(data) = &gem.gem else { continue };
                    let effects = std::iter::once(&data.granted_effect)
                        .chain(data.secondary_granted_effect.as_ref());
                    for effect in effects {
                        if !effect.support && e.skills.iter().any(|(s, _)| *s == effect.name) {
                            skills_used.insert(effect.name.clone());
                        }
                    }
                }
            }
        }
        let mut p = EnchantPopup {
            slot,
            key,
            have_skills,
            skills_used,
            skill_list: Vec::new(),
            skill_sel: 1,
            all_skills: false,
            all_skills_enabled: true,
            source_list: Vec::new(),
            source_sel: 1,
            sort_sel: 1,
            list: Vec::new(),
            sel: 1,
        };
        if have_skills {
            p.build_skill_list(e, true);
        }
        p.build_source_list(e)?;
        p.build_list(e)?;
        if have_skills && p.skills_used.is_empty() {
            p.all_skills = true;
            p.all_skills_enabled = false;
        }
        self.editor
            .popups
            .insert(0, super::item_popups::ItemPopup::Enchant(Box::new(p)));
        Ok(())
    }

    /// `enchantItem(idx, remove)`
    fn enchant_item(&self, p: &EnchantPopup, idx: usize, remove: bool) -> Result<Item> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        let line = p.list.get(idx.wrapping_sub(1)).map(|e| e.line.as_str());
        enchant(&mut item, p.slot, line, remove)?;
        self.build_and_parse_raw(&mut item)?;
        Ok(item)
    }

    /// `applySort(stat, selectFirst)`
    fn apply_sort(&self, p: &mut EnchantPopup, select_first: bool) -> Result<()> {
        let sort = mod_sort_list();
        let stat = sort
            .get(p.sort_sel.wrapping_sub(1))
            .and_then(|s| s.1.clone());
        let selected = (!select_first).then_some(p.sel);
        let selected_order =
            selected.and_then(|s| p.list.get(s.wrapping_sub(1)).map(|e| e.default_order));
        if let Some(stat) = stat {
            let name = stat.stat.clone().unwrap_or_default();
            let display = self.display_item.as_ref().ok_or("no item in the editor")?;
            let slot = display.primary_slot();
            let full_dps = name == "FullDPS";
            let data = self.data.calc_data();
            let calc = MiscCalculator::new(&self.build, &data)?;
            for i in 0..p.list.len() {
                let value = match p.list[i].sort_values.get(&name) {
                    Some(&v) => v,
                    None => {
                        let item = self.enchanted(p, &p.list[i].line)?;
                        let ovr = Override {
                            rep_item: Some((slot.clone(), Some(item))),
                            ..Override::default()
                        };
                        let v = stat.value(&calc.calculate(&ovr, full_dps)?);
                        p.list[i].sort_values.insert(name.clone(), v);
                        v
                    }
                };
                p.list[i].sort_value = Some(value);
            }
            p.list.sort_by(|a, b| {
                if a.sort_value != b.sort_value {
                    let (x, y) = (a.sort_value.unwrap_or(0.0), b.sort_value.unwrap_or(0.0));
                    if let Some(o) = y.partial_cmp(&x) {
                        return o;
                    }
                }
                a.default_order.cmp(&b.default_order)
            });
        } else {
            p.list.sort_by_key(|e| e.default_order);
        }
        match selected_order {
            Some(order) => {
                if let Some(i) = p.list.iter().position(|e| e.default_order == order) {
                    p.sel = i + 1;
                }
            }
            None => {
                if let Some(s) = new_sel(p.sel, 1.0, p.list.len()) {
                    p.sel = s;
                }
            }
        }
        Ok(())
    }

    /// The display item with `line` in the popup's slot, as `getSortValue`
    /// makes it
    fn enchanted(&self, p: &EnchantPopup, line: &str) -> Result<Item> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        enchant(&mut item, p.slot, Some(line), false)?;
        self.build_and_parse_raw(&mut item)?;
        Ok(item)
    }

    /// `control` of an input of the popup
    pub(super) fn enchant_popup_control(
        &mut self,
        p: &mut EnchantPopup,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let e = self.popup_enchantments(p)?;
        let sel = req["sel"].as_f64();
        let state = req["state"].as_bool();
        match name {
            "skill" if p.have_skills && sel.is_some() => {
                if let Some(s) = new_sel(p.skill_sel, sel.unwrap(), p.skill_list.len()) {
                    p.skill_sel = s;
                    self.skill_changed(p, e)?;
                }
            }
            "allSkills" if p.have_skills && state.is_some() => {
                p.all_skills = state.unwrap();
                p.build_skill_list(e, !p.all_skills);
                if let Some(s) = new_sel(p.skill_sel, 1.0, p.skill_list.len()) {
                    p.skill_sel = s;
                    self.skill_changed(p, e)?;
                }
                p.build_list(e)?;
                p.set_sel(1.0);
                self.apply_sort(p, true)?;
            }
            "enchantmentSource" if sel.is_some() => {
                if let Some(s) = new_sel(p.source_sel, sel.unwrap(), p.source_list.len()) {
                    p.source_sel = s;
                    p.build_list(e)?;
                    let s = p.sel.min(p.list.len());
                    p.set_sel(s as f64);
                    self.apply_sort(p, true)?;
                }
            }
            "sort" if sel.is_some() => {
                if let Some(s) = new_sel(p.sort_sel, sel.unwrap(), mod_sort_list().len()) {
                    p.sort_sel = s;
                    self.apply_sort(p, true)?;
                }
            }
            "enchantment" if sel.is_some() => p.set_sel(sel.unwrap()),
            _ => {
                return Err(format!(
                    "{NOT_PORTED}: control popup.{name} of Enchant Item"
                ));
            }
        }
        Ok(())
    }

    /// The skill's `selFunc`
    fn skill_changed(&self, p: &mut EnchantPopup, e: &Enchantments) -> Result<()> {
        p.build_source_list(e)?;
        p.build_list(e)?;
        p.set_sel(1.0);
        self.apply_sort(p, true)
    }

    /// The popup's buttons: the item enchanted or its enchantment removed,
    /// and the popup closed
    pub(super) fn enchant_popup_button(&mut self, p: &EnchantPopup, name: &str) -> Result<bool> {
        let item = match name {
            "save" => self.enchant_item(p, p.sel, false)?,
            "remove" => self.enchant_item(p, p.sel, true)?,
            "close" => return Ok(true),
            _ => return Err(format!("no popup button {name}")),
        };
        self.set_display_item_value(Some(item))?;
        Ok(true)
    }

    /// `describePopups` of the popup
    pub(super) fn describe_enchant_popup(&self, p: &EnchantPopup) -> J {
        let dd = |path: &str, label: &str, list: Vec<J>, sel: usize| {
            json!({
                "class": "DropDownControl", "enabled": true, "shown": true,
                "label": label, "list": list, "sel": sel, "path": format!("popup.{path}"),
            })
        };
        let mut controls = Vec::new();
        if p.have_skills {
            let skills = p.skill_list.iter().map(|s| json!(s)).collect();
            controls.push(dd("skill", "^7Skill:", skills, p.skill_sel));
            controls.push(json!({
                "class": "CheckBoxControl", "enabled": p.all_skills_enabled, "shown": true,
                "state": p.all_skills, "label": "All skills:",
                "tooltip": "Show all skills, not just those used by this build.",
                "path": "popup.allSkills",
            }));
        }
        let sources = p
            .source_list
            .iter()
            .map(|&i| json!(ENCHANTMENT_SOURCES[i].1))
            .collect();
        controls.push(dd("enchantmentSource", "^7Source:", sources, p.source_sel));
        let sort = mod_sort_list().into_iter().map(|(l, _)| json!(l)).collect();
        controls.push(dd("sort", "^7Sort by:", sort, p.sort_sel));
        let list = p.list.iter().map(|e| json!(e.line)).collect();
        controls.push(dd("enchantment", "^7Enchantment:", list, p.sel));
        json!({
            "title": "Enchant Item",
            "lines": [],
            "buttons": [
                { "name": "close", "label": "Cancel", "enabled": true },
                { "name": "remove", "label": "Remove", "enabled": true },
                { "name": "save", "label": "Enchant", "enabled": true },
            ],
            "controls": controls,
        })
    }
}

/// `ItemsTab:getAnoint(item)`: the names of the nodes the item allocates
pub(super) fn anoints(item: &Item) -> Vec<String> {
    let mut out = Vec::new();
    for list in [
        &item.enchant_mod_lines,
        &item.scourge_mod_lines,
        &item.implicit_mod_lines,
        &item.explicit_mod_lines,
        &item.crucible_mod_lines,
    ] {
        for m in list {
            if let Some((start, _)) = engine::pat::find(&m.line, "Allocates ([a-zA-Z ]+)") {
                out.push(m.line[start + "Allocates ".len()..].to_string());
            }
        }
    }
    out
}

impl<'d> TypedSession<'d> {
    /// `ItemsTab:AnointDisplayItem(enchantSlot)`; the session has no
    /// input for the notable list, so the popup saves without a node
    pub(super) fn anoint_display_item(&mut self, slot: usize) -> Result<()> {
        if self.display_item.is_none() {
            return Err(format!("{NOT_PORTED}: anointing without an item"));
        }
        self.editor
            .popups
            .insert(0, super::item_popups::ItemPopup::Anoint { slot });
        Ok(())
    }

    /// `anointItem(nil)`: the display item without the slot's enchantment
    pub(super) fn anoint_item(&self, slot: usize) -> Result<Item> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        if item.enchant_mod_lines.len() >= slot {
            item.enchant_mod_lines.remove(slot - 1);
        }
        self.build_and_parse_raw(&mut item)?;
        Ok(item)
    }

    /// `describePopups` of Anoint Item
    pub(super) fn describe_anoint_popup(&self, slot: usize) -> J {
        let current = self.display_item.as_ref().map(anoints).unwrap_or_default();
        let save = match current.get(slot.wrapping_sub(1)) {
            Some(name) => format!("Remove {name}"),
            None => "No Anoint".to_string(),
        };
        json!({
            "title": "Anoint Item",
            "lines": [],
            "buttons": [
                { "name": "close", "label": "Cancel", "enabled": true },
                { "name": "save", "label": save, "enabled": true },
            ],
            "controls": [],
        })
    }
}
