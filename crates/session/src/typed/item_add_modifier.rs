//! The item editor's Add Modifier to Item popup
//! (`ItemsTab:AddCustomModifierToDisplayItem`): crafting bench, essence,
//! veiled, beastcraft, Necropolis, fossil, drop-restricted, prefix and
//! suffix mods, or a custom line, sorted by the power of a stat with the
//! mod on the item in its slot.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_enchant::mod_sort_list;
use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession};
use engine::calc_override::Override;
use engine::calcs::lua_sort;
use engine::calculator::MiscCalculator;
use engine::item::{Item, ModLine};
use engine::item_data::{Affix, Essence};
use engine::pat::{capture, gsub, is_match, lower};
use serde_json::{Value as J, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const NIL_INDEX: &str = "attempt to index a nil value";

/// `defaultOrder`: a number (the bench's index) or a mod id
#[derive(Clone, Debug, PartialEq, PartialOrd)]
enum Order {
    None,
    Num(f64),
    Id(String),
}

#[derive(Clone, Debug)]
struct ListMod {
    label: String,
    m: Arc<Affix>,
    /// `type`: "crafted" (the bench) or "custom"
    crafted: bool,
    affix_type: Option<String>,
    default_order: Order,
    essence: Option<Essence>,
    default_sort_order: usize,
    sort_value: Option<f64>,
    sort_values: HashMap<String, f64>,
}

/// The popup's state; selections are 1-based as upstream's
#[derive(Clone, Debug)]
pub struct AddModifierPopup {
    sources: Vec<(&'static str, &'static str)>,
    source_sel: usize,
    sort_sel: usize,
    list: Vec<ListMod>,
    sel: usize,
    custom: String,
}

/// `SetSel(sel)` of a list: the new selection, if it changed
fn new_sel(cur: usize, sel: f64, len: usize) -> Option<usize> {
    let s = sel.min(len as f64).max(1.0) as usize;
    (s != cur).then_some(s)
}

/// `a.affixType == "Prefix" and b.affixType == "Suffix"`, else by order
fn by_prefix_suffix(a: &ListMod, b: &ListMod) -> bool {
    if a.affix_type != b.affix_type {
        a.affix_type.as_deref() == Some("Prefix") && b.affix_type.as_deref() == Some("Suffix")
    } else {
        a.default_order < b.default_order
    }
}

impl AddModifierPopup {
    fn source(&self) -> &'static str {
        self.sources[self.source_sel - 1].0
    }
}

impl<'d> TypedSession<'d> {
    /// `buildMods(sourceId)`: the source's mods for the display item
    fn build_modifier_list(&self, source: &str) -> Result<Vec<ListMod>> {
        let item = self.display_item.as_ref().ok_or("no item in the editor")?;
        let base = item.base.as_ref().ok_or(NIL_INDEX)?;
        let typ = base.typ.as_str();
        let craft = &self.data.game.craft;
        let items = &self.data.game.items;
        let affixes = item.affixes.as_ref();
        let entry = |label: String, m: &Arc<Affix>, crafted: bool, order: Order| ListMod {
            label,
            m: m.clone(),
            crafted,
            affix_type: m.typ.clone(),
            default_order: order,
            essence: None,
            default_sort_order: 0,
            sort_value: None,
            sort_values: HashMap::new(),
        };
        let typed_label = |m: &Affix| -> Result<String> {
            let t = m
                .typ
                .as_deref()
                .ok_or("attempt to concatenate a nil value (field 'type')")?;
            Ok(format!("{} ({t})", m.lines.join("/")))
        };
        let mut list: Vec<ListMod> = Vec::new();
        match source {
            "MASTER" => {
                let mut exclude: HashSet<String> = HashSet::new();
                for slots in [&item.prefixes, &item.suffixes] {
                    let limit = slots.limit.unwrap_or(item.affix_limit / 2.0);
                    let mut i = 1;
                    while i as f64 <= limit {
                        if let Some(s) = slots.list.get(i - 1)
                            && s.mod_id != "None"
                        {
                            let m = affixes.and_then(|a| a.get(&s.mod_id)).ok_or(NIL_INDEX)?;
                            if let Some(g) = &m.group {
                                exclude.insert(g.clone());
                            }
                        }
                        i += 1;
                    }
                }
                for (i, (m, types)) in craft.master_mods.iter().enumerate() {
                    if types.iter().any(|t| t == typ)
                        && !m.group.as_ref().is_some_and(|g| exclude.contains(g))
                    {
                        let t = m
                            .typ
                            .as_deref()
                            .ok_or("attempt to concatenate a nil value")?;
                        let label = format!("{} ^8({t})", m.lines.join("/"));
                        list.push(entry(label, m, true, Order::Num((i + 1) as f64)));
                    }
                }
                lua_sort(&mut list, by_prefix_suffix);
            }
            "ESSENCE" => {
                for e in &craft.essences {
                    let Some((_, id)) = e.mods.iter().find(|(t, _)| t == typ) else {
                        continue;
                    };
                    let Some(m) = affixes.and_then(|a| a.list.iter().find(|m| &m.id == id)) else {
                        continue;
                    };
                    let t = m
                        .typ
                        .as_deref()
                        .ok_or("attempt to concatenate a nil value")?;
                    let label = format!("{}   ^8[{}] ({t})", e.name, m.lines.join("/"));
                    let mut l = entry(label, m, false, Order::None);
                    l.essence = Some(e.clone());
                    list.push(l);
                }
                lua_sort(&mut list, |a, b| {
                    let (a, b) = (a.essence.as_ref().unwrap(), b.essence.as_ref().unwrap());
                    if a.typ != b.typ {
                        a.typ > b.typ
                    } else {
                        a.tier > b.tier
                    }
                });
            }
            "PREFIX" | "SUFFIX" => {
                let want = lower(source);
                for m in affixes.map_or(&[][..], |a| a.list.as_slice()) {
                    if m.typ.as_deref().map(lower).as_deref() == Some(want.as_str())
                        && item.spawn_weight(m) > 0.0
                    {
                        let a = m
                            .affix
                            .as_deref()
                            .ok_or("attempt to concatenate a nil value")?;
                        let label = format!("{a}   ^8[{}]", m.lines.join("/"));
                        list.push(entry(label, m, false, Order::None));
                    }
                }
                lua_sort(&mut list, |a, b| {
                    let (ma, mb) = (&a.m, &b.m);
                    for i in 0..ma.lines.len().max(mb.lines.len()) {
                        if i >= ma.lines.len() {
                            return true;
                        } else if i >= mb.lines.len() {
                            return false;
                        } else if ma.stat_order.get(i) != mb.stat_order.get(i) {
                            return ma.stat_order.get(i) < mb.stat_order.get(i);
                        }
                    }
                    ma.level > mb.level
                });
            }
            "VEILED" | "NECROPOLIS" | "BEASTCRAFT" => {
                let mods = match source {
                    "VEILED" => &craft.veiled_mods,
                    "NECROPOLIS" => &craft.necropolis_mods,
                    _ => &craft.beast_craft,
                };
                for m in mods {
                    let weight = match source {
                        "VEILED" => item.spawn_weight(m),
                        // `GetNecropolisModSpawnWeight(mod)`
                        "NECROPOLIS" => m
                            .weight_key
                            .iter()
                            .position(|k| base.tags.contains(&gsub(k, "necropolis_", "")))
                            .map_or(0.0, |i| m.weight_val.get(i).copied().unwrap_or(f64::NAN)),
                        _ => 1.0,
                    };
                    if weight > 0.0 {
                        list.push(entry(typed_label(m)?, m, false, Order::Id(m.id.clone())));
                    }
                }
                lua_sort(&mut list, by_prefix_suffix);
            }
            "INCURSION" | "DELVE" => {
                let (cats, db) = if source == "INCURSION" {
                    (&craft.incursion_drop_only, "Explicit")
                } else {
                    (&craft.delve_drop_only, "Delve")
                };
                let db = items.item_mods.get(db).ok_or("no mods")?;
                let sub = base.sub_type.as_ref().map(|s| format!("{typ}: {s}"));
                for (id, categories) in cats {
                    let Some(m) = db.list.iter().find(|m| &m.id == id) else {
                        continue;
                    };
                    if categories
                        .iter()
                        .any(|c| c == typ || sub.as_ref() == Some(c))
                    {
                        list.push(entry(typed_label(m)?, m, false, Order::Id(id.clone())));
                    }
                }
                lua_sort(&mut list, by_prefix_suffix);
            }
            "FOSSIL" => {
                for m in affixes.map_or(&[][..], |a| a.list.as_slice()) {
                    let delve = matches!(
                        m.affix.as_deref(),
                        Some("Subterranean" | "of the Underground")
                    );
                    if delve && item.spawn_weight(m) > 0.0 {
                        list.push(entry(typed_label(m)?, m, false, Order::Id(m.id.clone())));
                    }
                }
                lua_sort(&mut list, by_prefix_suffix);
            }
            _ => {}
        }
        for (i, l) in list.iter_mut().enumerate() {
            l.default_sort_order = i + 1;
        }
        Ok(list)
    }

    /// `ItemsTab:AddCustomModifierToDisplayItem()`
    pub(super) fn add_custom_modifier_to_display_item(&mut self) -> Result<()> {
        let item = self.display_item.as_ref().ok_or(NIL_INDEX)?;
        let base = item.base.as_ref().ok_or(NIL_INDEX)?;
        let typ = base.typ.as_str();
        let mut sources: Vec<(&'static str, &'static str)> = Vec::new();
        if typ != "Tincture" && typ != "Graft" {
            if typ != "Jewel" {
                sources.push(("MASTER", "Crafting Bench"));
            }
            if typ != "Jewel" && typ != "Flask" {
                sources.push(("ESSENCE", "Essence"));
                sources.push(("VEILED", "Veiled"));
                sources.push(("BEASTCRAFT", "Beastcraft"));
            }
            if ["Helmet", "Body Armour", "Gloves", "Boots"].contains(&typ) {
                sources.push(("NECROPOLIS", "Necropolis"));
            }
            let cluster = self.data.game.items.cluster_jewels.contains_key(&base.name);
            if !cluster && typ != "Flask" {
                sources.push(("FOSSIL", "Fossil"));
                sources.push(("DELVE", "Delve Drop-restricted"));
                sources.push(("INCURSION", "Incursion Drop-restricted"));
            }
            if !item.crafted {
                sources.push(("PREFIX", "Prefix"));
                sources.push(("SUFFIX", "Suffix"));
            }
        }
        if let Some(title) = &item.rare_like_unique
            && let Some((_, ids)) = self
                .data
                .game
                .craft
                .custom_modifier_sources
                .iter()
                .find(|(t, _)| t == title)
        {
            sources.retain(|(id, _)| ids.iter().any(|x| x == id));
        }
        let mut kept = Vec::new();
        for s in sources {
            if !self.build_modifier_list(s.0)?.is_empty() {
                kept.push(s);
            }
        }
        kept.push(("CUSTOM", "Custom"));
        let list = self.build_modifier_list(kept[0].0)?;
        let p = AddModifierPopup {
            sources: kept,
            source_sel: 1,
            sort_sel: 1,
            list,
            sel: 1,
            custom: String::new(),
        };
        self.editor
            .popups
            .insert(0, ItemPopup::AddModifier(Box::new(p)));
        Ok(())
    }

    /// `checkLineForAllocates(line, spec.nodes)`
    fn line_for_allocates(&self, line: &str) -> String {
        if is_match(line, "Allocates")
            && let Some(id) = capture(line, "%d+").and_then(|n| n.parse::<u32>().ok())
            && let Some(&i) = self.build.spec.tree.node_index.get(&id)
        {
            return format!("Allocates {}", self.build.spec.tree.nodes[i].dn);
        }
        line.to_string()
    }

    /// The display item with the mod's lines added
    fn with_modifier(&self, m: &ListMod, sorting: bool) -> Result<Item> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        for l in &m.m.lines {
            let line = if sorting {
                self.line_for_allocates(l)
            } else {
                l.clone()
            };
            let mut ml = ModLine {
                line,
                mod_tags: m.m.mod_tags.clone(),
                ..ModLine::default()
            };
            if m.crafted {
                ml.flags.crafted = true;
            } else {
                ml.flags.custom = true;
            }
            item.explicit_mod_lines.push(ml);
        }
        self.build_and_parse_raw(&mut item)?;
        Ok(item)
    }

    /// `applySort(stat, true)`
    fn modifier_apply_sort(&self, p: &mut AddModifierPopup) -> Result<()> {
        if p.source() == "CUSTOM" {
            return Ok(());
        }
        let sort = mod_sort_list();
        let stat = sort
            .get(p.sort_sel.wrapping_sub(1))
            .and_then(|s| s.1.clone());
        if let Some(stat) = &stat {
            let name = stat.stat.clone().unwrap_or_default();
            let display = self.display_item.as_ref().ok_or("no item in the editor")?;
            let slot = display.primary_slot();
            let data = self.data.calc_data();
            let calc = MiscCalculator::new(&self.build, &data)?;
            let full_dps = name == "FullDPS";
            for i in 0..p.list.len() {
                let v = match p.list[i].sort_values.get(&name) {
                    Some(&v) => v,
                    None => {
                        let item = self.with_modifier(&p.list[i], true)?;
                        let ovr = Override {
                            rep_item: Some((slot.clone(), Some(item))),
                            ..Override::default()
                        };
                        let v = stat.value(&calc.calculate(&ovr, full_dps)?);
                        p.list[i].sort_values.insert(name.clone(), v);
                        v
                    }
                };
                p.list[i].sort_value = Some(v);
            }
            lua_sort(&mut p.list, |a, b| {
                if a.sort_value != b.sort_value {
                    return a.sort_value.unwrap_or(0.0) > b.sort_value.unwrap_or(0.0);
                }
                a.default_sort_order < b.default_sort_order
            });
        } else {
            lua_sort(&mut p.list, |a, b| {
                a.default_sort_order < b.default_sort_order
            });
        }
        // `SetSel(1, true)`
        p.sel = 1;
        Ok(())
    }

    /// `control` of an input of the popup
    pub(super) fn add_modifier_popup_control(
        &mut self,
        p: &mut AddModifierPopup,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let sel = req["sel"].as_f64();
        match name {
            "source" if sel.is_some() => {
                let Some(s) = new_sel(p.source_sel, sel.unwrap(), p.sources.len()) else {
                    return Ok(());
                };
                p.source_sel = s;
                p.list = self.build_modifier_list(p.source())?;
                if let Some(m) = new_sel(p.sel, 1.0, p.list.len()) {
                    p.sel = m;
                }
                self.modifier_apply_sort(p)?;
            }
            "sort" if sel.is_some() => {
                if let Some(s) = new_sel(p.sort_sel, sel.unwrap(), mod_sort_list().len()) {
                    p.sort_sel = s;
                    self.modifier_apply_sort(p)?;
                }
            }
            "modSelect" if sel.is_some() => {
                if let Some(m) = new_sel(p.sel, sel.unwrap(), p.list.len()) {
                    p.sel = m;
                }
            }
            "custom" if req["text"].is_string() => {
                p.custom = req["text"].as_str().unwrap_or("").to_string();
            }
            _ => {
                return Err(format!(
                    "{NOT_PORTED}: control popup.{name} of Add Modifier"
                ));
            }
        }
        Ok(())
    }

    /// Add: `SetDisplayItem(addModifier())`
    pub(super) fn add_modifier_popup_save(&mut self, p: &AddModifierPopup) -> Result<()> {
        let item = if p.source() == "CUSTOM" {
            let display = self.display_item.as_ref().ok_or("no item in the editor")?;
            let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
            item.id = display.id;
            if is_match(&p.custom, "%S") {
                let mut ml = ModLine {
                    line: p.custom.clone(),
                    ..ModLine::default()
                };
                ml.flags.custom = true;
                item.explicit_mod_lines.push(ml);
            }
            self.build_and_parse_raw(&mut item)?;
            item
        } else {
            match p.list.get(p.sel.wrapping_sub(1)) {
                Some(m) => self.with_modifier(m, false)?,
                None => {
                    let display = self.display_item.as_ref().ok_or("no item in the editor")?;
                    let mut item =
                        Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
                    item.id = display.id;
                    self.build_and_parse_raw(&mut item)?;
                    item
                }
            }
        };
        self.set_display_item_value(Some(item))
    }

    /// `describePopups` of the popup
    pub(super) fn describe_add_modifier_popup(&self, p: &AddModifierPopup) -> J {
        let custom = p.source() == "CUSTOM";
        let dd = |path: &str, label: &str, list: Vec<J>, sel: usize| {
            json!({
                "class": "DropDownControl", "enabled": true, "shown": true,
                "label": label, "list": list, "sel": sel, "path": format!("popup.{path}"),
            })
        };
        let sources: Vec<J> = p.sources.iter().map(|s| json!(s.1)).collect();
        let mut source = dd("source", "^7Source:", sources, p.source_sel);
        source["enabled"] = json!(p.sources.len() > 1);
        let mut controls = vec![source];
        if custom {
            controls.push(json!({
                "class": "EditControl", "enabled": true, "shown": true,
                "text": p.custom, "path": "popup.custom",
            }));
        } else {
            let sort: Vec<J> = mod_sort_list().into_iter().map(|(l, _)| json!(l)).collect();
            controls.push(dd("sort", "^7Sort by:", sort, p.sort_sel));
            let list: Vec<J> = p.list.iter().map(|m| json!(m.label)).collect();
            controls.push(dd("modSelect", "^7Modifier:", list, p.sel));
        }
        json!({
            "title": "Add Modifier to Item",
            "lines": [],
            "buttons": [
                { "name": "close", "label": "Cancel", "enabled": true },
                { "name": "save", "label": "Add", "enabled": true },
            ],
            "controls": controls,
        })
    }
}
