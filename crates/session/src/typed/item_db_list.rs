//! The Items tab's unique and rare item databases as lists
//! (`Classes/ItemDBControl.lua`, `items.uniqueDB` and `items.rareDB` of the
//! `controls` and `control` commands): the items its filters let through
//! (`DoesItemMatchFilters`: slot, type, league, requirements, source and
//! search), each item's power by the sort's stat in the slots it fits
//! (`ListBuilder` on `engine::compare::item_power`), and the sort.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedSession, num};
use engine::calc_override::Override;
use engine::calculator::MiscCalculator;
use engine::compare;
use engine::item::Item;
use engine::parallel::Threads;
use engine::pat::{gsub, is_match, lower, match_or_pattern};
use engine::power::{PowerStat, power_stats};
use serde_json::{Map, Value as J, json};
use std::sync::Arc;

const SLOTS: [&str; 14] = [
    "Any slot",
    "Weapon 1",
    "Weapon 2",
    "Helmet",
    "Body Armour",
    "Gloves",
    "Boots",
    "Amulet",
    "Ring",
    "Belt",
    "Jewel",
    "Flask",
    "Graft 1",
    "Graft 2",
];
/// `typeList` before `LoadLeaguesAndTypes` adds the items' types
const TYPES: [&str; 5] = [
    "Any type",
    "Armour",
    "Jewellery",
    "One Handed Melee",
    "Two Handed Melee",
];
/// `leagueList` before `LoadLeaguesAndTypes` adds the items' leagues
const LEAGUES: [&str; 2] = ["Any league", "No league"];
const SELECT_DB: [&str; 2] = ["Uniques", "Rare Templates"];
const REQUIREMENTS: [&str; 4] = [
    "Any requirements",
    "Current level",
    "Current attributes",
    "Current useable",
];
const OBTAINABLE: [&str; 8] = [
    "Obtainable",
    "Any source",
    "Unobtainable",
    "Vendor Recipe",
    "Upgraded",
    "Boss Item",
    "Corruption",
    "Core Drop Pool",
];
const SEARCH_MODES: [&str; 3] = ["Anywhere", "Names", "Modifiers"];

/// A database list's inputs; selections are 1-based as upstream's
#[derive(Clone, Debug)]
pub struct DbList {
    sort: usize,
    slot: usize,
    typ: usize,
    league: usize,
    requirement: usize,
    obtainable: usize,
    search: String,
    search_mode: usize,
}

impl Default for DbList {
    fn default() -> DbList {
        DbList {
            sort: 1,
            slot: 1,
            typ: 1,
            league: 1,
            requirement: 1,
            obtainable: 1,
            search: String::new(),
            search_mode: 1,
        }
    }
}

/// `BuildSortOrder`'s list: label and power stat, `None` for Name
fn sort_list() -> Vec<(String, Option<PowerStat>)> {
    power_stats()
        .into_iter()
        .filter(|p| !p.ignore_for_items)
        .map(|p| {
            let label = format!("Sort by {}", p.label);
            (label, p.stat.is_some().then_some(p))
        })
        .collect()
}

/// Which database a path names, and the input after it
fn parse(path: &str) -> Option<(bool, Option<&str>)> {
    let rest = path.strip_prefix("items.")?;
    if rest == "selectDB" {
        return Some((false, Some("selectDB")));
    }
    let (db, input) = match rest.split_once('.') {
        Some((db, input)) => (db, Some(input)),
        None => (rest, None),
    };
    let rare = match db {
        "uniqueDB" => false,
        "rareDB" => true,
        _ => return None,
    };
    // The rare list has no sort, league, requirement or source inputs
    let ok = match input {
        None | Some("slot" | "type" | "search" | "searchMode" | "selectDB") => true,
        Some("sort" | "league" | "requirement" | "obtainable") => !rare,
        Some(_) => false,
    };
    ok.then_some((rare, input))
}

fn drop_down(list: &[&str], sel: usize, shown: bool) -> J {
    json!({ "class": "DropDownControl", "enabled": true, "list": list, "sel": sel, "shown": shown })
}

/// `DropDownControl:SetSel`: clamped; only a change calls the handler
fn set_sel(cur: &mut usize, sel: f64, len: usize) -> bool {
    let s = (sel as usize).clamp(1, len.max(1));
    let changed = s != *cur;
    *cur = s;
    changed
}

/// `LoadLeaguesAndTypes`: the leagues and item types of a database's
/// items, sorted, after the lists' first entries
fn leagues_and_types(db: &super::item_db::ItemDb) -> (Vec<String>, Vec<String>) {
    let mut leagues = std::collections::BTreeSet::new();
    let mut types = std::collections::BTreeSet::new();
    for item in db.values() {
        if let Some(l) = &item.league {
            leagues.extend(engine::pat::gmatch1(l, " ?([%w ]+),?"));
        }
        if let Some(t) = &item.typ {
            types.insert(t.clone());
        }
    }
    let leagues = LEAGUES
        .iter()
        .map(|s| s.to_string())
        .chain(leagues)
        .collect();
    let types = TYPES.iter().map(|s| s.to_string()).chain(types).collect();
    (leagues, types)
}

impl TypedSession<'_> {
    /// A database's league and item type lists
    fn db_lists(&self, rare: bool) -> Result<(Vec<String>, Vec<String>)> {
        let db = if rare {
            self.data.rare_db()?
        } else {
            self.data.unique_db()?
        };
        Ok(leagues_and_types(db))
    }

    /// Whether a control path is an item database list or one of its inputs
    pub fn is_item_db_control(path: &str) -> bool {
        parse(path).is_some()
    }

    fn db_list(&self, rare: bool) -> &DbList {
        &self.item_db_lists[rare as usize]
    }

    /// `controls` of an item database list or its inputs
    pub(super) fn item_db_control_json(&self, path: &str) -> Result<J> {
        let (rare, input) = parse(path).ok_or("no control")?;
        let l = self.db_list(rare);
        // The session lays the Items tab out with no height, which shows the
        // "Import from" selector: the list it selects shows
        let shown = self.select_db == if rare { 2 } else { 1 };
        let (leagues, types) = self.db_lists(rare)?;
        let leagues: Vec<&str> = leagues.iter().map(String::as_str).collect();
        let types: Vec<&str> = types.iter().map(String::as_str).collect();
        Ok(match input {
            None => {
                let rows = self.item_db_rows(rare)?;
                json!({ "class": "ItemDBControl", "enabled": true, "list": rows, "shown": shown })
            }
            Some("sort") => {
                let labels: Vec<String> = sort_list().into_iter().map(|(l, _)| l).collect();
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                drop_down(&labels, l.sort, shown)
            }
            Some("slot") => drop_down(&SLOTS, l.slot, shown),
            Some("type") => drop_down(&types, l.typ, shown),
            Some("league") => drop_down(&leagues, l.league, shown),
            Some("selectDB") => drop_down(&SELECT_DB, self.select_db, true),
            Some("requirement") => drop_down(&REQUIREMENTS, l.requirement, shown),
            Some("obtainable") => drop_down(&OBTAINABLE, l.obtainable, shown),
            Some("searchMode") => drop_down(&SEARCH_MODES, l.search_mode, shown),
            Some("search") => {
                json!({ "class": "EditControl", "enabled": true, "shown": shown, "text": l.search })
            }
            Some(_) => return Err("no control".into()),
        })
    }

    /// `control` of an item database list's input
    pub fn item_db_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().ok_or("no control path")?;
        let (rare, input) = parse(path).ok_or("no control")?;
        let sel = req.get("sel").and_then(J::as_f64);
        let text = req.get("text").and_then(J::as_str);
        if let (Some("selectDB"), Some(s)) = (input, sel) {
            set_sel(&mut self.select_db, s, SELECT_DB.len());
            return Ok(json!([]));
        }
        if let (None, Some(s)) = (input, sel) {
            if self.class_popups_open() > 0 {
                return Err(format!(
                    "{}: the item database with a class change popup open",
                    super::NOT_PORTED
                ));
            }
            self.item_db_select(rare, s, req)?;
            return Ok(json!([]));
        }
        let (leagues, types) = self.db_lists(rare)?;
        let l = &mut self.item_db_lists[rare as usize];
        match (input, sel, text) {
            (Some("sort"), Some(s), _) => {
                set_sel(&mut l.sort, s, sort_list().len());
            }
            (Some("slot"), Some(s), _) => {
                set_sel(&mut l.slot, s, SLOTS.len());
            }
            (Some("type"), Some(s), _) => {
                set_sel(&mut l.typ, s, types.len());
            }
            (Some("league"), Some(s), _) => {
                set_sel(&mut l.league, s, leagues.len());
            }
            (Some("requirement"), Some(s), _) => {
                set_sel(&mut l.requirement, s, REQUIREMENTS.len());
            }
            (Some("obtainable"), Some(s), _) => {
                set_sel(&mut l.obtainable, s, OBTAINABLE.len());
            }
            (Some("searchMode"), Some(s), _) => {
                set_sel(&mut l.search_mode, s, SEARCH_MODES.len());
            }
            // `EditControl:SetText` with the filter `%c`
            (Some("search"), _, Some(t)) => l.search = gsub(t, "%c", ""),
            _ => return Err(format!("{}: control {path}", super::NOT_PORTED)),
        }
        Ok(json!([]))
    }

    /// `DoesItemMatchFilters(item)`
    fn item_db_matches(
        &self,
        rare: bool,
        item: &Item,
        leagues: &[String],
        types: &[String],
    ) -> bool {
        let l = self.db_list(rare);
        if l.slot > 1 {
            let primary = item.primary_slot();
            let want = SLOTS[l.slot - 1];
            if primary != want && gsub(&primary, " %d", "") != want {
                return false;
            }
        }
        let typ = item.typ.as_deref().unwrap_or("");
        match l.typ {
            2 if !item.base.as_ref().is_some_and(|b| b.armour.is_some()) => return false,
            3 if !matches!(typ, "Amulet" | "Ring" | "Belt") => return false,
            4 | 5 => {
                let info = self.data.game.setup.weapon_type_info.get(typ);
                let ok = info.is_some_and(|w| w.melee && (w.one_hand == (l.typ == 4)));
                if !ok {
                    return false;
                }
            }
            n if n > 5 && types.get(n - 1).is_some_and(|t| t != typ) => return false,
            _ => {}
        }
        if !rare {
            if l.league == 2 && item.league.is_some() {
                return false;
            }
            if l.league > 2 {
                let want = leagues.get(l.league - 1).map_or("", String::as_str);
                if !item.league.as_deref().is_some_and(|il| is_match(il, want)) {
                    return false;
                }
            }
            if l.obtainable != 2 {
                let source = item.source.as_deref().unwrap_or("");
                let obtainable = !(source == "No longer obtainable"
                    || item.league.as_deref() == Some("Race Events"));
                let fails = match l.obtainable {
                    1 => !obtainable,
                    3 => obtainable,
                    4 => source != "Vendor Recipe",
                    5 => !is_match(source, "Upgraded from"),
                    6 => !is_match(source, "Drops from unique"),
                    7 => !is_match(source, "Vaal Orb"),
                    8 => item.source.is_some(),
                    _ => false,
                };
                if fails {
                    return false;
                }
            }
            if l.requirement > 1 {
                let r = &item.requirements;
                let level = self.build.info.character_level;
                if (l.requirement == 2 || l.requirement == 4) && r.level.is_some_and(|v| v > level)
                {
                    return false;
                }
                let out = |k: &str| {
                    self.main
                        .player
                        .get(&engine::mods::Sym::new(k))
                        .and_then(engine::actor::OutVal::num)
                        .unwrap_or(0.0)
                };
                if l.requirement > 2
                    && (r.str_mod > out("Str") || r.dex_mod > out("Dex") || r.int_mod > out("Int"))
                {
                    return false;
                }
            }
        }
        let mut search = gsub(&lower(&l.search), "[%-%.%+%[%]%$%^%%%?%*]", "%%%0");
        if !is_match(&search, "%S") {
            return true;
        }
        // A malformed pattern matches nothing (`PCall`)
        let found = |s: &str, p: &str| match_or_pattern(s, p).unwrap_or(false);
        let mut ok = false;
        if l.search_mode == 1 || l.search_mode == 2 {
            let mut name = lower(&item.name);
            if item
                .explicit_mod_lines
                .iter()
                .any(|m| m.new_mod_id.is_some())
            {
                name = format!("foulborn {name}");
            }
            if found(&name, &search) {
                ok = true;
            }
        }
        if l.search_mode == 1 || l.search_mode == 3 {
            let lines = item
                .enchant_mod_lines
                .iter()
                .chain(&item.implicit_mod_lines);
            if lines.into_iter().any(|m| found(&lower(&m.line), &search)) {
                ok = true;
            }
            let mods = &self.data.game.items.item_mods;
            for m in &item.explicit_mod_lines {
                let mut line = lower(&m.line);
                // The other side of a Foulborn transformation
                if let Some(id) = &m.new_mod_id
                    && let Some(new) = ["ItemExclusive", "Foulborn"]
                        .iter()
                        .find_map(|g| mods.get(*g)?.get(id))
                {
                    line.push_str(&lower(&new.lines.join(" ")));
                }
                if found(&line, &search) {
                    ok = true;
                    break;
                }
            }
            if !ok {
                search = search.replace(' ', "");
                ok = item
                    .base_mod_list
                    .iter()
                    .any(|m| found(&lower(m.name.as_str()), &search));
            }
        }
        ok
    }

    /// The slots an item's power is measured in: those it fits that are
    /// shown and in use (`IsItemValidForSlot`, `slot.inactive`,
    /// `slot.weaponSet`, `slot.shown()`)
    fn item_db_slots(&self) -> Vec<String> {
        let tab = &self.build.items;
        let spec = &self.build.spec;
        let second = tab.active_item_set().use_second_weapon_set;
        let ring3 = self
            .main
            .sidebar
            .as_ref()
            .is_some_and(|s| s.additional_ring_slot);
        let abyssal_count = |parent: &str| {
            tab.ordered_slots
                .iter()
                .find(|s| s.slot_name == parent)
                .and_then(|s| tab.item(s.sel_item_id))
                .map_or(0.0, |i| i.abyssal_socket_count)
        };
        tab.ordered_slots
            .iter()
            .filter(|s| {
                let name = s.slot_name.as_str();
                let inactive = match (&s.node_id, &s.parent_slot) {
                    (Some(n), _) => !spec.alloc_nodes.contains(*n),
                    (None, Some(p)) => s.slot_num.unwrap_or(0.0) > abyssal_count(p),
                    _ => false,
                };
                !inactive
                    && match s.weapon_set {
                        Some(1) => !second,
                        Some(_) => second,
                        None if name == "Graft 1" || name == "Graft 2" => {
                            spec.tree.version.contains("3_27")
                        }
                        None if name == "Ring 3" => ring3,
                        None => true,
                    }
            })
            .map(|s| s.slot_name.clone())
            .collect()
    }

    /// `ListBuilder()`: the rows of the list, each item's name and, sorted
    /// by a stat, its power
    fn item_db_rows(&self, rare: bool) -> Result<Vec<J>> {
        let by_stat = self.item_db_sort_stat(rare).is_some();
        Ok(self
            .item_db_list(rare)?
            .iter()
            .map(|(item, power)| {
                let mut o = Map::new();
                o.insert("name".into(), json!(item.name));
                if by_stat {
                    o.insert("power".into(), num(*power));
                }
                J::Object(o)
            })
            .collect())
    }

    /// The sort's stat (the rare list has no sort control)
    fn item_db_sort_stat(&self, rare: bool) -> Option<PowerStat> {
        (!rare)
            .then(|| sort_list().into_iter().nth(self.db_list(rare).sort - 1))
            .flatten()
            .and_then(|(_, s)| s)
    }

    /// The list's items in its order, with their power by the sort's stat
    fn item_db_list(&self, rare: bool) -> Result<Vec<(Arc<Item>, f64)>> {
        let db = if rare {
            self.data.rare_db()?
        } else {
            self.data.unique_db()?
        };
        let (leagues, types) = leagues_and_types(db);
        let mut list: Vec<(&Arc<Item>, f64)> = db
            .values()
            .filter(|i| self.item_db_matches(rare, i, &leagues, &types))
            .map(|i| (i, f64::NEG_INFINITY))
            .collect();
        let stat = self.item_db_sort_stat(rare);
        if let Some(stat) = &stat {
            let info = &self.data.game.setup.weapon_type_info;
            let one_hand = |t: &str| info.get(t).map(|w| w.one_hand);
            let tab = &self.build.items;
            let tree = self.build.spec.tree;
            let slots = self.item_db_slots();
            let overrides: Vec<Vec<Override>> = list
                .iter()
                .map(|(item, _)| {
                    let base = item.base.as_ref();
                    let fits: Vec<&String> = slots
                        .iter()
                        .filter(|s| tab.valid_for_slot(item, s, tree, &one_hand))
                        .collect();
                    if fits.is_empty() {
                        return vec![];
                    }
                    // Every slot a flask or tincture fits toggles the same
                    if base.is_some_and(|b| b.flask.is_some() || b.tincture.is_some()) {
                        return vec![Override {
                            toggle_item: Some((***item).clone()),
                            ..Override::default()
                        }];
                    }
                    fits.into_iter()
                        .map(|s| Override {
                            rep_item: Some((s.clone(), Some((***item).clone()))),
                            ..Override::default()
                        })
                        .collect()
                })
                .collect();
            let data = self.data.calc_data();
            let calc = MiscCalculator::new(&self.build, &data)?;
            let full_dps = compare::uses_full_dps(stat) || self.view_mode == "TREE";
            let powers =
                compare::item_power(&calc, overrides, stat, full_dps, Threads::available());
            for ((_, p), power) in list.iter_mut().zip(powers) {
                *p = power?;
            }
        }
        // `sortOrder`: the stat, descending, then the name without "The "
        let by_stat = stat.is_some();
        let name = |i: &Item| gsub(&i.name, "^The ", "");
        engine::calcs::lua_sort(&mut list, |a, b| {
            if by_stat && a.1 != b.1 {
                return b.1 < a.1;
            }
            name(a.0) < name(b.0)
        });
        Ok(list.into_iter().map(|(i, p)| (i.clone(), p)).collect())
    }

    /// A row's item (1-based)
    fn item_db_row(&self, rare: bool, index: Option<f64>) -> Result<Option<Arc<Item>>> {
        let Some(i) = index.filter(|i| *i >= 1.0 && i.fract() == 0.0) else {
            return Ok(None);
        };
        Ok(self
            .item_db_list(rare)?
            .get(i as usize - 1)
            .map(|(item, _)| item.clone()))
    }

    /// `SelectIndex(sel)`, then `OnSelClick` for a click: Ctrl adds the
    /// item and equips it, a double click opens it in the editor
    fn item_db_select(&mut self, rare: bool, sel: f64, req: &J) -> Result<()> {
        let Some(item) = self.item_db_row(rare, Some(sel))? else {
            return Ok(());
        };
        if req["click"].as_bool() != Some(true) {
            return Ok(());
        }
        let ctrl = req["ctrl"].as_bool().unwrap_or(false);
        let shift = req["shift"].as_bool().unwrap_or(false);
        if ctrl {
            let mut new = Item::new(&self.data.item_context(), &item.raw, None, false)?;
            if new.base.is_none() {
                return Err(format!("{}: an item without a base", super::NOT_PORTED));
            }
            new.normalise_quality();
            let id = self.add_item(new, true)?;
            let new = self.build.items.item(id).ok_or("no item")?.clone();
            let mut name = new.primary_slot();
            let tab = &self.build.items;
            if let Some(i) = tab.ordered_slots.iter().position(|s| s.slot_name == name) {
                if tab.ordered_slots[i].weapon_set == Some(1)
                    && tab.active_item_set().use_second_weapon_set
                {
                    name += " Swap";
                }
                if shift {
                    let alt = name.replace('1', "2");
                    if self.valid_for_slot(&new, &alt) {
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
                self.set_sel_item(i, id)?;
            }
            let other = super::item_editor::FORBIDDEN
                .iter()
                .find(|(t, _)| new.title.as_deref() == Some(*t))
                .map(|(_, o)| *o);
            if let Some(other) = other {
                self.add_forbidden_counterpart(&new, other)?;
            }
            self.populate_slots();
            self.rebuild()?;
        } else if req["doubleClick"].as_bool() == Some(true) {
            self.display_item_from_raw(&item.raw, true)?;
        }
        Ok(())
    }

    /// `controlTooltip` of a row: `AddValueTooltip`
    pub fn item_db_tooltip(&mut self, path: &str, index: Option<f64>) -> Result<J> {
        let (rare, input) = parse(path).ok_or("no control")?;
        if input.is_some() {
            return Ok(super::tooltip::Tooltip::default().json());
        }
        self.update_sockets();
        let mut tip = super::tooltip::Tooltip::default();
        let popups = self.ui.popups.as_array().map_or(0, Vec::len);
        if popups == 0
            && let Some(item) = self.item_db_row(rare, index)?
        {
            self.add_item_tooltip_mode(&mut tip, &item, None, true)?;
        }
        Ok(tip.json())
    }
}
