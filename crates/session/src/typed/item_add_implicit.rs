//! The item editor's Add Implicit to Item popup
//! (`Modules/AddImplicitPopup.lua`): Searing Exarch and Eater of Worlds
//! implicits grouped by their mod group, a unique's vestigial implicits, or
//! a custom line; the lists sorted by the power of a stat with the mod on
//! the item in its slot.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_enchant::mod_sort_list;
use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession};
use engine::calc_override::Override;
use engine::calcs::lua_sort;
use engine::calculator::MiscCalculator;
use engine::item::{Item, ModLine};
use engine::item_data::Affix;
use engine::pat::{capture, gsub, is_match, lower};
use serde_json::{Value as J, json};
use std::collections::HashMap;
use std::sync::Arc;

const NIL_INDEX: &str = "attempt to index a nil value";
const VESTIGIAL_COLOR: &str = "^xCBA5F1";
/// `data.vestigialUniqueBaseTypes`
const VESTIGIAL_BASE_TYPES: [&str; 5] = ["Helmet", "Body Armour", "Gloves", "Boots", "Shield"];

/// A mod of a list (`listMod`), or a group of them
#[derive(Clone, Debug)]
struct ListMod {
    label: String,
    m: Arc<Affix>,
    /// `type`: "exarch", "eater" or "vestigial"
    typ: &'static str,
    /// The mod's id (`defaultOrder` of Exarch and Eater mods, `id` of
    /// vestigial ones)
    id: String,
    /// A vestigial mod's unique (`uniqueTitle`)
    unique_title: Option<String>,
    default_sort_order: usize,
    sort_value: Option<f64>,
    sort_values: HashMap<String, f64>,
}

#[derive(Clone, Debug)]
struct Group {
    label: String,
    m: Arc<Affix>,
    mod_list_index: usize,
    default_sort_order: usize,
    sort_value: Option<f64>,
}

/// The popup's state; selections are 1-based as upstream's
#[derive(Clone, Debug)]
pub struct AddImplicitPopup {
    /// Source ids and labels
    sources: Vec<(&'static str, &'static str)>,
    source_sel: usize,
    sort_sel: usize,
    mod_list: Vec<Vec<ListMod>>,
    groups: Vec<Group>,
    group_sel: usize,
    /// `controls.modSelect.list`: the index of its list in `mod_list`
    select_list: Option<usize>,
    mod_sel: usize,
    custom: String,
    /// Vestigial groups are their mods (one table): the group's fields
    /// follow the mod's
    vestigial: bool,
}

/// `SetSel(sel)` of a list: the new selection, if it changed
fn new_sel(cur: usize, sel: f64, len: usize) -> Option<usize> {
    let s = sel.min(len as f64).max(1.0) as usize;
    (s != cur).then_some(s)
}

/// The first `j` lines' stat order comparison of the group sort
fn lines_less(a: &Affix, b: &Affix) -> Option<bool> {
    for i in 0..a.lines.len().max(b.lines.len()) {
        if i >= a.lines.len() {
            return Some(true);
        } else if i >= b.lines.len() {
            return Some(false);
        } else if a.stat_order.get(i) != b.stat_order.get(i) {
            let (x, y) = (a.stat_order.get(i), b.stat_order.get(i));
            return Some(x.zip(y).is_some_and(|(x, y)| x < y));
        }
    }
    None
}

impl AddImplicitPopup {
    fn source(&self) -> &'static str {
        self.sources[self.source_sel - 1].0
    }

    /// `setDefaultSortOrder()`
    fn set_default_sort_order(&mut self) {
        for (i, g) in self.groups.iter_mut().enumerate() {
            g.default_sort_order = i + 1;
            g.sort_value = None;
        }
        for list in self.mod_list.iter_mut() {
            for (i, m) in list.iter_mut().enumerate() {
                m.default_sort_order = i + 1;
                m.sort_value = None;
                m.sort_values.clear();
            }
        }
        if self.vestigial {
            // The group is its list's only mod
            for g in self.groups.iter_mut() {
                g.default_sort_order = 1;
            }
        }
    }

    fn select_list(&self) -> &[ListMod] {
        self.select_list
            .and_then(|i| self.mod_list.get(i))
            .map_or(&[], |l| l.as_slice())
    }
}

impl<'d> TypedSession<'d> {
    /// `buildMods(sourceId)`
    fn build_implicit_mods(&self, p: &mut AddImplicitPopup, source: &str) -> Result<()> {
        let item = self.display_item.as_ref().ok_or("no item in the editor")?;
        p.mod_list.clear();
        p.groups.clear();
        p.vestigial = false;
        if source == "EXARCH" || source == "EATER" {
            let typ = if source == "EXARCH" {
                "exarch"
            } else {
                "eater"
            };
            let affixes = item
                .affixes
                .as_ref()
                .ok_or("bad argument #1 to 'pairs' (table expected, got nil)")?;
            let mut indexes: HashMap<String, usize> = HashMap::new();
            for m in &affixes.list {
                if item.spawn_weight(m) > 0.0 && m.typ.as_deref().is_some_and(|t| lower(t) == typ) {
                    let label = m.lines.join("/");
                    let g = m.group.as_deref().ok_or(NIL_INDEX)?;
                    let g = gsub(&gsub(g, "PinnaclePresence", ""), "UniquePresence", "");
                    let at = match indexes.get(&g) {
                        Some(&i) => i,
                        None => {
                            p.mod_list.push(Vec::new());
                            p.groups.push(Group {
                                label: label.clone(),
                                m: m.clone(),
                                mod_list_index: p.mod_list.len(),
                                default_sort_order: 0,
                                sort_value: None,
                            });
                            indexes.insert(g, p.groups.len());
                            p.groups.len()
                        }
                    };
                    p.mod_list[at - 1].push(ListMod {
                        label,
                        m: m.clone(),
                        typ,
                        id: m.id.clone(),
                        unique_title: None,
                        default_sort_order: 0,
                        sort_value: None,
                        sort_values: HashMap::new(),
                    });
                }
            }
            lua_sort(&mut p.groups, |a, b| {
                lines_less(&a.m, &b.m).unwrap_or_else(|| a.m.level > b.m.level)
            });
            let err = std::cell::Cell::new(None);
            for list in p.mod_list.iter_mut() {
                lua_sort(list, |a, b| {
                    let (ga, gb) = (
                        a.m.group.as_deref().unwrap_or(""),
                        b.m.group.as_deref().unwrap_or(""),
                    );
                    if ga != gb {
                        return if is_match(ga, "PinnaclePresence") {
                            false
                        } else if is_match(gb, "PinnaclePresence") {
                            true
                        } else {
                            !is_match(ga, "UniquePresence")
                        };
                    }
                    let (ma, mb) = (&a.m, &b.m);
                    if ma.lines.is_empty() && mb.lines.is_empty() {
                        return ma.level > mb.level;
                    }
                    if ma.lines.is_empty() {
                        return true;
                    } else if mb.lines.is_empty() {
                        return false;
                    } else if ma.stat_order.first() != mb.stat_order.first() {
                        return ma.stat_order.first() < mb.stat_order.first();
                    }
                    let num = |id: &str| capture(id, "%d+$").and_then(|n| n.parse::<f64>().ok());
                    match (num(&a.id), num(&b.id)) {
                        (Some(x), Some(y)) => x < y,
                        _ => {
                            err.set(Some("attempt to compare nil with number"));
                            false
                        }
                    }
                });
            }
            if let Some(e) = err.get() {
                return Err(e.into());
            }
            for g in p.groups.iter_mut() {
                let first = p.mod_list[g.mod_list_index - 1].first().ok_or(NIL_INDEX)?;
                g.label = gsub(
                    &gsub(&first.label, "%([%d%.]+%-[%d%.]+%)", "#"),
                    "[%d%.]+",
                    "#",
                );
            }
        } else if source == "VESTIGIAL" {
            p.vestigial = true;
            let mods = self
                .data
                .game
                .items
                .item_mods
                .get("Vestigial")
                .ok_or("no vestigial mods")?;
            let own = item.title.as_deref().map(lower);
            for (id, title) in &self.data.game.vestigial_mod_mappings {
                let t = lower(title);
                let Some(unique) = self.data.unique_by_title(&t)? else {
                    continue;
                };
                if unique.base.as_ref().map(|b| &b.typ) != item.base.as_ref().map(|b| &b.typ) {
                    continue;
                }
                if own.as_deref().ok_or(NIL_INDEX)? == t {
                    continue;
                }
                let m = mods.get(id).ok_or(NIL_INDEX)?;
                let label = format!("{VESTIGIAL_COLOR}{}", m.lines.join("/"));
                p.mod_list.push(vec![ListMod {
                    label,
                    m: Arc::new(m.clone()),
                    typ: "vestigial",
                    id: id.clone(),
                    unique_title: Some(title.clone()),
                    default_sort_order: 0,
                    sort_value: None,
                    sort_values: HashMap::new(),
                }]);
            }
            // Sorted by id (their groups compare as equal)
            lua_sort(&mut p.mod_list, |a, b| a[0].id < b[0].id);
            for (i, list) in p.mod_list.iter().enumerate() {
                let m = &list[0];
                p.groups.push(Group {
                    label: m.label.clone(),
                    m: m.m.clone(),
                    mod_list_index: i + 1,
                    default_sort_order: 0,
                    sort_value: None,
                });
            }
        }
        p.set_default_sort_order();
        Ok(())
    }

    /// `AddImplicitPopup.AddImplicitToDisplayItem(itemsTab, displayItem)`
    pub(super) fn add_implicit_to_display_item(&mut self) -> Result<()> {
        let item = self.display_item.as_ref().ok_or(NIL_INDEX)?;
        let mut sources = Vec::new();
        let title = item.title.as_deref().map(lower);
        let typ = item.base.as_ref().map(|b| b.typ.as_str()).unwrap_or("");
        let db_unique = match &title {
            Some(t) => self.data.unique_by_title(t)?,
            None => None,
        };
        if item.rarity == "UNIQUE"
            && let Some(u) = &db_unique
            && VESTIGIAL_BASE_TYPES.contains(&typ)
            && (u.source.is_none() || title.as_deref() == Some("stasis prison"))
        {
            sources.push(("VESTIGIAL", "Vestigial"));
        }
        if item.rarity != "UNIQUE"
            && item.rarity != "RELIC"
            && ["Helmet", "Body Armour", "Gloves", "Boots"].contains(&typ)
        {
            if item.influences[6] {
                sources.push(("EXARCH", "Searing Exarch"));
            }
            if item.influences[7] {
                sources.push(("EATER", "Eater of Worlds"));
            }
        }
        sources.push(("CUSTOM", "Custom"));
        let mut p = AddImplicitPopup {
            sources,
            source_sel: 1,
            sort_sel: 1,
            mod_list: Vec::new(),
            groups: Vec::new(),
            group_sel: 1,
            select_list: None,
            mod_sel: 1,
            custom: String::new(),
            vestigial: false,
        };
        let first = p.source();
        self.build_implicit_mods(&mut p, first)?;
        if first != "CUSTOM" {
            let g = p.groups.first().ok_or(NIL_INDEX)?;
            p.select_list = Some(g.mod_list_index - 1);
        }
        self.editor
            .popups
            .insert(0, ItemPopup::AddImplicit(Box::new(p)));
        Ok(())
    }

    /// `applyCandidateMod(item, listMod)`
    fn apply_candidate_mod(item: &mut Item, m: &ListMod) {
        let line = |l: &str| {
            let mut ml = ModLine {
                line: l.to_string(),
                mod_tags: m.m.mod_tags.clone(),
                ..ModLine::default()
            };
            match m.typ {
                "exarch" => ml.flags.exarch = true,
                "eater" => ml.flags.eater = true,
                _ => ml.flags.vestigial = true,
            }
            ml
        };
        if m.typ == "exarch" || m.typ == "eater" {
            let index = item.implicit_mod_lines.iter().position(|ml| {
                if m.typ == "exarch" {
                    ml.flags.exarch
                } else {
                    ml.flags.eater
                }
            });
            if let Some(index) = index {
                for (i, l) in m.m.lines.iter().enumerate() {
                    let at = index + i;
                    if at < item.implicit_mod_lines.len() {
                        item.implicit_mod_lines[at] = line(l);
                    } else {
                        item.implicit_mod_lines.push(line(l));
                    }
                }
                return;
            }
        } else {
            item.implicit_mod_lines.clear();
        }
        for l in &m.m.lines {
            item.implicit_mod_lines.push(line(l));
        }
    }

    /// The display item with the mod as `getSortValue` and `addModifier`
    /// make it
    fn with_implicit(&self, m: &ListMod) -> Result<Item> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        Self::apply_candidate_mod(&mut item, m);
        self.build_and_parse_raw(&mut item)?;
        Ok(item)
    }

    /// `applySort(stat, true)`
    fn implicit_apply_sort(&self, p: &mut AddImplicitPopup) -> Result<()> {
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
            for li in 0..p.mod_list.len() {
                for mi in 0..p.mod_list[li].len() {
                    let v = match p.mod_list[li][mi].sort_values.get(&name) {
                        Some(&v) => v,
                        None => {
                            let item = self.with_implicit(&p.mod_list[li][mi])?;
                            let ovr = Override {
                                rep_item: Some((slot.clone(), Some(item))),
                                ..Override::default()
                            };
                            let v = stat.value(&calc.calculate(&ovr, full_dps)?);
                            p.mod_list[li][mi].sort_values.insert(name.clone(), v);
                            v
                        }
                    };
                    p.mod_list[li][mi].sort_value = Some(v);
                }
                lua_sort(&mut p.mod_list[li], |a, b| {
                    if a.sort_value != b.sort_value {
                        return a.sort_value.unwrap_or(0.0) > b.sort_value.unwrap_or(0.0);
                    }
                    a.default_sort_order < b.default_sort_order
                });
            }
            for g in p.groups.iter_mut() {
                let mut best: Option<f64> = None;
                for m in p
                    .mod_list
                    .get(g.mod_list_index - 1)
                    .map_or(&[][..], |l| l.as_slice())
                {
                    let v = m.sort_value.unwrap_or(0.0);
                    if best.is_none_or(|b| v > b) {
                        best = Some(v);
                    }
                }
                g.sort_value = Some(best.unwrap_or(0.0));
            }
            lua_sort(&mut p.groups, |a, b| {
                if a.sort_value != b.sort_value {
                    return a.sort_value.unwrap_or(0.0) > b.sort_value.unwrap_or(0.0);
                }
                a.default_sort_order < b.default_sort_order
            });
        } else {
            for list in p.mod_list.iter_mut() {
                lua_sort(list, |a, b| a.default_sort_order < b.default_sort_order);
            }
            lua_sort(&mut p.groups, |a, b| {
                a.default_sort_order < b.default_sort_order
            });
        }
        // `SetSel(1, true)` of both
        p.group_sel = 1;
        let g = p.groups.first().ok_or(NIL_INDEX)?;
        p.select_list = Some(g.mod_list_index - 1);
        p.mod_sel = 1;
        Ok(())
    }

    /// `control` of an input of the popup
    pub(super) fn add_implicit_popup_control(
        &mut self,
        p: &mut AddImplicitPopup,
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
                let source = p.source();
                if source != "CUSTOM" {
                    self.build_implicit_mods(p, source)?;
                    // `modGroupSelect:SetSel(1)`, with its `selFunc`
                    if let Some(g) = new_sel(p.group_sel, 1.0, p.groups.len()) {
                        p.group_sel = g;
                        let i = p.groups.get(g - 1).ok_or(NIL_INDEX)?.mod_list_index;
                        p.select_list = Some(i - 1);
                        p.mod_sel = 1;
                    }
                    let g = p.groups.first().ok_or(NIL_INDEX)?;
                    p.select_list = Some(g.mod_list_index - 1);
                    if let Some(m) = new_sel(p.mod_sel, 1.0, p.select_list().len()) {
                        p.mod_sel = m;
                    }
                    self.implicit_apply_sort(p)?;
                }
            }
            "sort" if sel.is_some() => {
                if let Some(s) = new_sel(p.sort_sel, sel.unwrap(), mod_sort_list().len()) {
                    p.sort_sel = s;
                    self.implicit_apply_sort(p)?;
                }
            }
            "modGroupSelect" if sel.is_some() => {
                if let Some(g) = new_sel(p.group_sel, sel.unwrap(), p.groups.len()) {
                    p.group_sel = g;
                    let i = p.groups[g - 1].mod_list_index;
                    p.select_list = Some(i - 1);
                    if let Some(m) = new_sel(p.mod_sel, 1.0, p.select_list().len()) {
                        p.mod_sel = m;
                    }
                }
            }
            "modSelect" if sel.is_some() => {
                if let Some(m) = new_sel(p.mod_sel, sel.unwrap(), p.select_list().len()) {
                    p.mod_sel = m;
                }
            }
            "custom" if req["text"].is_string() => {
                p.custom = req["text"].as_str().unwrap_or("").to_string();
            }
            _ => {
                return Err(format!(
                    "{NOT_PORTED}: control popup.{name} of Add Implicit"
                ));
            }
        }
        Ok(())
    }

    /// Add: `SetDisplayItem(addModifier())`
    pub(super) fn add_implicit_popup_save(&mut self, p: &AddImplicitPopup) -> Result<()> {
        let item = self.add_implicit_save_item(p)?;
        self.set_display_item_value(Some(item))
    }

    /// The item the save button makes
    pub(super) fn add_implicit_save_item(&self, p: &AddImplicitPopup) -> Result<Item> {
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
                item.implicit_mod_lines.push(ml);
            }
            self.build_and_parse_raw(&mut item)?;
            item
        } else {
            let g = p.groups.get(p.group_sel.wrapping_sub(1)).ok_or(NIL_INDEX)?;
            let m = p
                .mod_list
                .get(g.mod_list_index - 1)
                .and_then(|l| l.get(p.mod_sel.wrapping_sub(1)))
                .ok_or(NIL_INDEX)?;
            self.with_implicit(m)?
        };
        Ok(item)
    }

    /// `describePopups` of the popup
    pub(super) fn describe_add_implicit_popup(&self, p: &AddImplicitPopup) -> J {
        let custom = p.source() == "CUSTOM";
        let mod_select_shown = !custom && p.source() != "VESTIGIAL";
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
        if !custom {
            let sort: Vec<J> = mod_sort_list().into_iter().map(|(l, _)| json!(l)).collect();
            controls.push(dd("sort", "^7Sort by:", sort, p.sort_sel));
            let groups: Vec<J> = p.groups.iter().map(|g| json!(g.label)).collect();
            let label = if mod_select_shown {
                "^7Type:"
            } else {
                "^7Modifier:"
            };
            controls.push(dd("modGroupSelect", label, groups, p.group_sel));
        } else {
            controls.push(json!({
                "class": "EditControl", "enabled": true, "shown": true,
                "text": p.custom, "path": "popup.custom",
            }));
        }
        if mod_select_shown {
            let list: Vec<J> = p.select_list().iter().map(|m| json!(m.label)).collect();
            controls.push(dd("modSelect", "^7Modifier:", list, p.mod_sel));
        }
        json!({
            "title": "Add Implicit to Item",
            "lines": [],
            "buttons": [
                { "name": "close", "label": "Cancel", "enabled": true },
                { "name": "save", "label": "Add", "enabled": true },
            ],
            "controls": controls,
        })
    }
}

impl<'d> TypedSession<'d> {
    /// A group entry's mod, unique title and whether it replaces the
    /// implicits (a vestigial group is its mod)
    pub(super) fn implicit_group_entry(
        &self,
        p: &AddImplicitPopup,
        idx: Option<usize>,
    ) -> Option<(Arc<Affix>, Option<String>, bool)> {
        let g = p.groups.get(idx?.wrapping_sub(1))?;
        if p.vestigial {
            let m = p.mod_list.get(g.mod_list_index - 1)?.first()?;
            Some((m.m.clone(), m.unique_title.clone(), true))
        } else {
            Some((g.m.clone(), None, false))
        }
    }

    /// A mod entry's mod and unique title
    pub(super) fn implicit_mod_entry(
        &self,
        p: &AddImplicitPopup,
        idx: Option<usize>,
    ) -> Option<(Arc<Affix>, Option<String>, bool)> {
        let m = p.select_list().get(idx?.wrapping_sub(1))?;
        Some((m.m.clone(), m.unique_title.clone(), false))
    }
}
