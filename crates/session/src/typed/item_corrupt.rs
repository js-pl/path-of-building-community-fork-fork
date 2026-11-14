//! The item editor's Corrupt Item popup (`ItemsTab:CorruptDisplayItem`):
//! the corrupted, Scourge or Glimpse of Chaos implicits from the source's
//! lists, sorted by name or by the power of a stat, and for a unique the
//! corrupted roll ranges of its explicit lines.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_enchant::mod_sort_list;
use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession};
use engine::calc_override::Override;
use engine::calculator::MiscCalculator;
use engine::item::{Item, ModLine};
use engine::item_data::Affix;
use engine::item_tools::{Range, apply_range};
use engine::pat::gsub;
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const MAX_IMPLICITS: usize = 5;
const SCOURGE_UP: &str = "ScourgeUpside";
const SCOURGE_DOWN: &str = "ScourgeDownside";
const NIL_INDEX: &str = "attempt to index a nil value";

/// An entry of an implicit list
#[derive(Clone, Debug)]
struct Entry {
    m: Arc<Affix>,
    default_order: usize,
    sort_value: Option<f64>,
    sort_values: HashMap<String, f64>,
}

/// An implicit dropdown: "None" (`None`) and mods
#[derive(Clone, Debug)]
struct Drop {
    list: Vec<Option<Arc<Affix>>>,
    sel: usize,
    shown: bool,
}

impl Drop {
    fn selected(&self) -> Option<&Arc<Affix>> {
        self.list
            .get(self.sel.wrapping_sub(1))
            .and_then(|m| m.as_ref())
    }

    /// `SelByValue(mod, "mod")`
    fn sel_by_value(&mut self, m: Option<&Arc<Affix>>) {
        if let Some(i) = self
            .list
            .iter()
            .position(|e| e.as_ref().map(|a| &a.id) == m.map(|a| &a.id))
        {
            self.sel = i + 1;
        }
    }
}

/// A unique's explicit line with a corrupted roll range
#[derive(Clone, Debug)]
struct Roll {
    /// The line's index in the explicit lines (1-based)
    index: usize,
    val: f64,
}

/// The popup's state; selections are 1-based as upstream's
#[derive(Clone, Debug)]
pub struct CorruptPopup {
    source_list: Vec<&'static str>,
    source_sel: usize,
    sources_shown: bool,
    sort_sel: usize,
    current: String,
    max_implicits: f64,
    unique: bool,
    cannot_be_changed: bool,
    lists: BTreeMap<String, Vec<Entry>>,
    implicits: Vec<Drop>,
    rolls: Vec<Roll>,
    rolls_shown: bool,
    corrupted_ranges: HashMap<usize, f64>,
}

/// `mod[1]:lower():gsub(...)`: the first line as the list sorts it
fn sort_name(m: &Affix) -> String {
    let first = m.lines.first().map(String::as_str).unwrap_or("");
    let s = gsub(&engine::pat::lower(first), "%(.-%)", "$");
    let s = gsub(&s, "[%+%-%%]", "");
    gsub(&s, "%d+", "$")
}

/// The lines a mod adds, with its tags and the Scourge mark
fn mod_lines(m: &Affix, scourge: bool) -> Vec<ModLine> {
    m.lines
        .iter()
        .map(|l| {
            let l = if scourge {
                format!("{{scourge}}{l}")
            } else {
                l.clone()
            };
            let line = if m.mod_tags.is_empty() {
                l
            } else {
                format!("{{tags:{}}}{l}", m.mod_tags.join(","))
            };
            ModLine {
                line,
                ..ModLine::default()
            }
        })
        .collect()
}

/// `round(val, dec)`
fn round(v: f64, dec: i32) -> f64 {
    let f = 10f64.powi(dec);
    (v * f + 0.5).floor() / f
}

impl CorruptPopup {
    fn is_scourge(&self) -> bool {
        self.current == SCOURGE_UP
    }

    /// `buildCorruptLists(modType)`
    fn build_corrupt_lists(&mut self, mod_type: &str) -> Result<()> {
        let mut selected: HashMap<String, (usize, Arc<Affix>)> = HashMap::new();
        for (i, c) in self.implicits.iter().enumerate() {
            if let Some(m) = c.selected() {
                let group = m.group.clone().ok_or("table index is nil")?;
                selected.insert(group, (i, m.clone()));
            }
        }
        let entries = self
            .lists
            .get(mod_type)
            .ok_or("bad argument #1 to 'ipairs'")?;
        for (i, c) in self.implicits.iter_mut().enumerate() {
            c.list = vec![None];
            for e in entries {
                let free = match e.m.group.as_ref().and_then(|g| selected.get(g)) {
                    Some((idx, _)) => *idx == i,
                    None => true,
                };
                if free {
                    c.list.push(Some(e.m.clone()));
                }
            }
        }
        for (i, m) in selected.values() {
            self.implicits[*i].sel_by_value(Some(m));
        }
        Ok(())
    }

    /// `buildScourgeList(control, other, modType)`
    fn build_scourge_list(&mut self, c: usize, other: usize, mod_type: &str) -> Result<()> {
        let own = {
            let d = self.implicits.get(c).ok_or(NIL_INDEX)?;
            if d.sel > 1 {
                Some(
                    d.list
                        .get(d.sel - 1)
                        .ok_or(NIL_INDEX)?
                        .clone()
                        .ok_or(NIL_INDEX)?,
                )
            } else {
                None
            }
        };
        let other_mod = match self.implicits.get(other) {
            Some(d) if d.sel > 1 => Some(
                d.list
                    .get(d.sel - 1)
                    .ok_or(NIL_INDEX)?
                    .clone()
                    .ok_or(NIL_INDEX)?,
            ),
            _ => None,
        };
        let entries = self
            .lists
            .get(mod_type)
            .ok_or("bad argument #1 to 'ipairs'")?;
        let mut list = vec![None];
        for e in entries {
            if other_mod.as_ref().is_none_or(|o| e.m.group != o.group) {
                list.push(Some(e.m.clone()));
            }
        }
        let d = &mut self.implicits[c];
        d.list = list;
        // `SelByValue(false)` of no selection finds nothing
        if let Some(m) = own {
            d.sel_by_value(Some(&m));
        }
        Ok(())
    }

    /// `setImplicitControlsShown(implicitNum, canChangeImplicits)`
    fn set_implicits_shown(&mut self, n: f64, can: bool) {
        for (i, c) in self.implicits.iter_mut().enumerate() {
            c.shown = can && (i + 1) as f64 <= n;
        }
    }
}

impl<'d> TypedSession<'d> {
    /// `buildImplicitList(modType)`
    fn build_implicit_list(&self, p: &mut CorruptPopup, mod_type: &str) -> Result<()> {
        if p.lists.contains_key(mod_type) {
            return Ok(());
        }
        let item = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut list: Vec<Entry> = Vec::new();
        let push = |list: &mut Vec<Entry>, m: &Arc<Affix>| {
            list.push(Entry {
                m: m.clone(),
                default_order: 0,
                sort_value: None,
                sort_values: HashMap::new(),
            })
        };
        if mod_type == "Glimpse of Chaos" {
            let exclusive = self
                .data
                .game
                .items
                .item_mods
                .get("ItemExclusive")
                .ok_or("no ItemExclusive mods")?;
            for m in &exclusive.list {
                if m.id.starts_with("UniqueSpecialCorruption") {
                    push(&mut list, m);
                }
            }
        } else {
            let affixes = item
                .affixes
                .as_ref()
                .ok_or("bad argument #1 to 'pairs' (table expected, got nil)")?;
            for m in &affixes.list {
                if m.typ.as_deref() == Some(mod_type) && item.spawn_weight(m) > 0.0 {
                    push(&mut list, m);
                }
            }
        }
        let err = std::cell::Cell::new(None);
        engine::calcs::lua_sort(&mut list, |a, b| {
            let (an, bn) = (sort_name(&a.m), sort_name(&b.m));
            if an != bn {
                an < bn
            } else {
                match (a.m.level, b.m.level) {
                    (Some(x), Some(y)) => x < y,
                    _ => {
                        err.set(Some("attempt to compare nil with number"));
                        false
                    }
                }
            }
        });
        if let Some(e) = err.get() {
            return Err(e.into());
        }
        for (i, e) in list.iter_mut().enumerate() {
            e.default_order = i + 1;
        }
        p.lists.insert(mod_type.to_string(), list);
        Ok(())
    }

    /// `ItemsTab:CorruptDisplayItem()`
    pub(super) fn corrupt_display_item(&mut self) -> Result<()> {
        let item = self.display_item.as_ref().ok_or("no item in the editor")?;
        let base = item.base.as_ref().ok_or(NIL_INDEX)?;
        let mut max = item.corrupt_implicit_count.unwrap_or(2.0);
        for ml in &item.implicit_mod_lines {
            for m in &ml.mod_list {
                if m.name.as_str() == "CorruptImplicitCount" {
                    max -= 1.0;
                }
            }
        }
        let mut source_list = vec!["Corrupted", "Scourge"];
        if base.typ == "Helmet" {
            // `isUnique` is no field of an item, so Glimpse of Chaos goes last
            source_list.push("Glimpse of Chaos");
        }
        let unique = item.rarity == "UNIQUE" || item.rarity == "RELIC";
        let cannot_be_changed = item.implicits_cannot_be_changed;
        let mut p = CorruptPopup {
            source_list: source_list.clone(),
            source_sel: 1,
            sources_shown: true,
            sort_sel: 1,
            current: source_list[0].to_string(),
            max_implicits: max,
            unique,
            cannot_be_changed,
            lists: BTreeMap::new(),
            implicits: (1..=MAX_IMPLICITS)
                .map(|i| Drop {
                    list: Vec::new(),
                    sel: 1,
                    shown: i as f64 <= max && !cannot_be_changed,
                })
                .collect(),
            rolls: Vec::new(),
            rolls_shown: false,
            corrupted_ranges: HashMap::new(),
        };
        let current = p.current.clone();
        self.build_implicit_list(&mut p, &current)?;
        if unique {
            let copy = Item::new(&self.data.item_context(), &item.build_raw(), None, false)?;
            for (i, ml) in copy.explicit_mod_lines.iter().enumerate() {
                let range = Range::One(ml.range.unwrap_or(0.5));
                let at = |r: f64| {
                    apply_range(
                        &self.data.game.items,
                        &self.data.parser,
                        &ml.line,
                        &range,
                        ml.value_scalar,
                        Some(r),
                    )
                };
                if at(1.0)? != at(2.0)? && copy.check_mod_line_variant(ml) {
                    let cr = ml.corrupted_range.unwrap_or(1.0);
                    p.corrupted_ranges.insert(i + 1, cr);
                    p.rolls.push(Roll {
                        index: i + 1,
                        val: (cr - 0.78) / 0.44,
                    });
                }
            }
        }
        // Each shown implicit's `selFunc()`
        let mut i = 1;
        while i as f64 <= max && i <= MAX_IMPLICITS {
            p.build_corrupt_lists(&current)?;
            i += 1;
        }
        self.editor
            .popups
            .insert(0, ItemPopup::Corrupt(Box::new(p)));
        Ok(())
    }

    /// `getSortValue`: the stat with the entry's mod as the item's implicits
    fn corrupt_sort_value(
        &self,
        p: &CorruptPopup,
        m: &Affix,
        mod_type: &str,
        stat: &engine::power::PowerStat,
        calc: &MiscCalculator,
        slot: &str,
    ) -> Result<f64> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        item.corrupted = true;
        let lines = mod_lines(m, p.is_scourge());
        if mod_type == SCOURGE_UP || mod_type == SCOURGE_DOWN {
            item.scourge_mod_lines = lines;
        } else {
            item.implicit_mod_lines = lines;
        }
        self.build_and_parse_raw(&mut item)?;
        let ovr = Override {
            rep_item: Some((slot.to_string(), Some(item))),
            ..Override::default()
        };
        let full_dps = stat.stat.as_deref() == Some("FullDPS");
        Ok(stat.value(&calc.calculate(&ovr, full_dps)?))
    }

    /// `applySort(stat)`
    fn corrupt_apply_sort(&self, p: &mut CorruptPopup) -> Result<()> {
        let sort = mod_sort_list();
        let stat = sort
            .get(p.sort_sel.wrapping_sub(1))
            .and_then(|s| s.1.clone());
        let types: Vec<&str> = if p.is_scourge() {
            vec![SCOURGE_UP, SCOURGE_DOWN]
        } else {
            vec![]
        };
        let current = p.current.clone();
        let types: Vec<String> = if types.is_empty() {
            vec![current.clone()]
        } else {
            types.into_iter().map(String::from).collect()
        };
        let data = self.data.calc_data();
        let calc = match &stat {
            Some(_) => Some(MiscCalculator::new(&self.build, &data)?),
            None => None,
        };
        let slot = self
            .display_item
            .as_ref()
            .ok_or("no item in the editor")?
            .primary_slot();
        for t in &types {
            let Some(mut list) = p.lists.remove(t) else {
                continue;
            };
            let r = (|| -> Result<()> {
                match (&stat, &calc) {
                    (Some(stat), Some(calc)) => {
                        let name = stat.stat.clone().unwrap_or_default();
                        for e in list.iter_mut() {
                            let v = match e.sort_values.get(&name) {
                                Some(&v) => v,
                                None => {
                                    let v =
                                        self.corrupt_sort_value(p, &e.m, t, stat, calc, &slot)?;
                                    e.sort_values.insert(name.clone(), v);
                                    v
                                }
                            };
                            e.sort_value = Some(v);
                        }
                        engine::calcs::lua_sort(&mut list, |a, b| {
                            if a.sort_value != b.sort_value {
                                a.sort_value.unwrap_or(0.0) > b.sort_value.unwrap_or(0.0)
                            } else {
                                a.default_order < b.default_order
                            }
                        });
                    }
                    _ => list.sort_by_key(|e| e.default_order),
                }
                Ok(())
            })();
            p.lists.insert(t.clone(), list);
            r?;
        }
        if p.is_scourge() {
            p.build_scourge_list(0, 1, SCOURGE_UP)?;
            p.build_scourge_list(1, 0, SCOURGE_UP)?;
            p.build_scourge_list(2, 3, SCOURGE_DOWN)?;
            p.build_scourge_list(3, 2, SCOURGE_DOWN)?;
        } else {
            p.build_corrupt_lists(&current)?;
        }
        Ok(())
    }

    /// `corruptItem(addingImplicits)`
    fn corrupt_item(&self, p: &CorruptPopup, adding: bool) -> Result<Item> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        item.corrupted = true;
        let special = if p.is_scourge() {
            None
        } else {
            display
                .implicit_mod_lines
                .iter()
                .rev()
                .find(|ml| {
                    ml.mod_list
                        .iter()
                        .any(|m| m.name.as_str() == "CorruptImplicitCount")
                })
                .cloned()
        };
        if adding {
            let mut new = Vec::new();
            for c in &p.implicits {
                if c.sel > 1 {
                    let m = c
                        .list
                        .get(c.sel - 1)
                        .ok_or(NIL_INDEX)?
                        .as_ref()
                        .ok_or(NIL_INDEX)?;
                    new.extend(mod_lines(m, p.is_scourge()));
                }
            }
            if !new.is_empty() {
                if p.is_scourge() {
                    item.scourge_mod_lines.clear();
                } else {
                    item.implicit_mod_lines.clear();
                }
                if let Some(s) = special {
                    item.implicit_mod_lines.push(s);
                }
                if p.is_scourge() {
                    item.scourge_mod_lines.extend(new);
                } else {
                    item.implicit_mod_lines.extend(new);
                }
            }
        } else {
            for (i, ml) in item.explicit_mod_lines.iter_mut().enumerate() {
                if let Some(&cr) = p.corrupted_ranges.get(&(i + 1)) {
                    ml.corrupted_range = (cr != 1.0).then_some(cr);
                }
            }
        }
        self.build_and_parse_raw(&mut item)?;
        Ok(item)
    }

    /// `control` of an input of the popup
    pub(super) fn corrupt_popup_control(
        &mut self,
        p: &mut CorruptPopup,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let sel = req["sel"].as_f64();
        let new_sel = |cur: usize, sel: f64, len: usize| {
            let s = sel.min(len as f64).max(1.0) as usize;
            (s != cur).then_some(s)
        };
        if let Some(i) = name
            .strip_prefix("implicit")
            .and_then(|i| i.parse::<usize>().ok())
        {
            let sel = sel.ok_or_else(|| format!("{NOT_PORTED}: control popup.{name}"))?;
            let c = p.implicits.get(i.wrapping_sub(1)).ok_or(NIL_INDEX)?;
            if let Some(s) = new_sel(c.sel, sel, c.list.len()) {
                p.implicits[i - 1].sel = s;
                // `selFunc`
                if p.is_scourge() {
                    let other = if i % 2 == 0 { i - 1 } else { i + 1 };
                    let t = if i <= 2 { SCOURGE_UP } else { SCOURGE_DOWN };
                    if other > MAX_IMPLICITS {
                        return Err(NIL_INDEX.into());
                    }
                    p.build_scourge_list(other - 1, i - 1, t)?;
                } else {
                    let current = p.current.clone();
                    p.build_corrupt_lists(&current)?;
                }
            }
            return Ok(());
        }
        if let Some(i) = name
            .strip_prefix("rollRangeSlider")
            .and_then(|i| i.parse::<usize>().ok())
        {
            let val = req["val"]
                .as_f64()
                .ok_or_else(|| format!("{NOT_PORTED}: control popup.{name}"))?;
            let r = p.rolls.iter_mut().find(|r| r.index == i).ok_or(NIL_INDEX)?;
            let v = val.clamp(0.0, 1.0);
            if v != r.val {
                r.val = v;
                p.corrupted_ranges.insert(i, 0.78 + round(0.44 * v, 2));
            }
            return Ok(());
        }
        match name {
            "source" if sel.is_some() => {
                let Some(s) = new_sel(p.source_sel, sel.unwrap(), p.source_list.len()) else {
                    return Ok(());
                };
                p.source_sel = s;
                // `SetSel(1, true)`: no callbacks
                for c in p.implicits.iter_mut() {
                    c.sel = 1;
                }
                let value = p.source_list[s - 1];
                if value == "Scourge" {
                    p.current = SCOURGE_UP.to_string();
                    self.build_implicit_list(p, SCOURGE_UP)?;
                    self.build_implicit_list(p, SCOURGE_DOWN)?;
                    let n = if p.unique { 4.0 } else { 3.0 };
                    p.set_implicits_shown(n, true);
                    p.build_scourge_list(2, 3, SCOURGE_DOWN)?;
                    p.build_scourge_list(3, 2, SCOURGE_DOWN)?;
                } else {
                    self.build_implicit_list(p, value)?;
                    p.current = value.to_string();
                    let (max, can) = (p.max_implicits, !p.cannot_be_changed);
                    p.set_implicits_shown(max, can);
                }
                self.corrupt_apply_sort(p)?;
            }
            "sort" if sel.is_some() => {
                if let Some(s) = new_sel(p.sort_sel, sel.unwrap(), mod_sort_list().len()) {
                    p.sort_sel = s;
                    self.corrupt_apply_sort(p)?;
                }
            }
            _ => {
                return Err(format!(
                    "{NOT_PORTED}: control popup.{name} of Corrupt Item"
                ));
            }
        }
        Ok(())
    }

    /// The popup's buttons; true when it closes
    pub(super) fn corrupt_popup_button(
        &mut self,
        p: &mut CorruptPopup,
        name: &str,
    ) -> Result<bool> {
        match name {
            "save" => {
                let adding = p.implicits[0].shown;
                let item = self.corrupt_item(p, adding)?;
                self.set_display_item_value(Some(item))?;
                Ok(true)
            }
            "close" => Ok(true),
            "implicits" => {
                let n = if p.is_scourge() { 4.0 } else { p.max_implicits };
                let can = p.current != "Corrupted" || !p.cannot_be_changed;
                p.set_implicits_shown(n, can);
                p.rolls_shown = false;
                p.sources_shown = true;
                Ok(false)
            }
            "rolls" => {
                p.set_implicits_shown(0.0, false);
                p.rolls_shown = true;
                p.sources_shown = false;
                Ok(false)
            }
            _ => Err(format!("no popup button {name}")),
        }
    }

    /// `describePopups` of the popup
    pub(super) fn describe_corrupt_popup(&self, p: &CorruptPopup) -> J {
        let label = |m: &Option<Arc<Affix>>| match m {
            Some(m) => json!(m.lines.join("/")),
            None => json!("None"),
        };
        let mut controls = Vec::new();
        if p.sources_shown {
            let sort: Vec<J> = mod_sort_list().into_iter().map(|(l, _)| json!(l)).collect();
            controls.push(json!({
                "class": "DropDownControl", "enabled": true, "shown": true,
                "label": "^7Sort by:", "list": sort, "sel": p.sort_sel, "path": "popup.sort",
            }));
            controls.push(json!({
                "class": "DropDownControl", "enabled": p.source_list.len() > 1, "shown": true,
                "label": "^7Source:", "list": p.source_list, "sel": p.source_sel,
                "path": "popup.source",
            }));
        }
        for (i, c) in p.implicits.iter().enumerate() {
            if c.shown {
                let list: Vec<J> = c.list.iter().map(label).collect();
                controls.push(json!({
                    "class": "DropDownControl", "enabled": true, "shown": true,
                    "label": format!("^7Implicit #{}:", i + 1), "list": list, "sel": c.sel,
                    "path": format!("popup.implicit{}", i + 1),
                }));
            }
        }
        if p.rolls_shown {
            for r in &p.rolls {
                controls.push(json!({
                    "class": "SliderControl", "enabled": true, "shown": true, "val": super::num(r.val),
                    "path": format!("popup.rollRangeSlider{}", r.index),
                }));
            }
        }
        let mut buttons = vec![json!({ "name": "close", "label": "Cancel", "enabled": true })];
        if p.unique {
            buttons.push(json!({ "name": "implicits", "label": "Implicits", "enabled": true }));
            buttons.push(json!({ "name": "rolls", "label": "Roll Ranges", "enabled": true }));
        }
        buttons.push(json!({ "name": "save", "label": "Corrupt", "enabled": true }));
        json!({
            "title": "Corrupt Item",
            "lines": [],
            "buttons": buttons,
            "controls": controls,
        })
    }
}

impl<'d> TypedSession<'d> {
    /// The mod of an implicit drop-down's entry
    pub(super) fn corrupt_entry(
        &self,
        p: &CorruptPopup,
        name: &str,
        idx: Option<usize>,
    ) -> Option<Arc<Affix>> {
        let i: usize = name.strip_prefix("implicit")?.parse().ok()?;
        let c = p.implicits.get(i.wrapping_sub(1))?;
        c.list.get(idx?.wrapping_sub(1))?.clone()
    }

    /// The item Corrupt makes
    pub(super) fn corrupt_save_item(&self, p: &CorruptPopup) -> Result<Item> {
        self.corrupt_item(p, p.implicits[0].shown)
    }
}
