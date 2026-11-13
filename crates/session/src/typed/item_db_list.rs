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
/// `typeList` before the list is drawn (`LoadLeaguesAndTypes` adds the
/// items' types when it is)
const TYPES: [&str; 5] = [
    "Any type",
    "Armour",
    "Jewellery",
    "One Handed Melee",
    "Two Handed Melee",
];
/// `leagueList` before the list is drawn
const LEAGUES: [&str; 2] = ["Any league", "No league"];
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
        None | Some("slot" | "type" | "search" | "searchMode") => true,
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

impl TypedSession<'_> {
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
        // "Import from" selector; on Uniques, only the unique list shows
        let shown = !rare;
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
            Some("type") => drop_down(&TYPES, l.typ, shown),
            Some("league") => drop_down(&LEAGUES, l.league, shown),
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
        let l = &mut self.item_db_lists[rare as usize];
        match (input, sel, text) {
            (Some("sort"), Some(s), _) => {
                set_sel(&mut l.sort, s, sort_list().len());
            }
            (Some("slot"), Some(s), _) => {
                set_sel(&mut l.slot, s, SLOTS.len());
            }
            (Some("type"), Some(s), _) => {
                set_sel(&mut l.typ, s, TYPES.len());
            }
            (Some("league"), Some(s), _) => {
                set_sel(&mut l.league, s, LEAGUES.len());
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
    fn item_db_matches(&self, rare: bool, item: &Item) -> bool {
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
            _ => {}
        }
        if !rare {
            if l.league == 2 && item.league.is_some() {
                return false;
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
        let db = if rare {
            self.data.rare_db()?
        } else {
            self.data.unique_db()?
        };
        let mut list: Vec<(&Arc<Item>, f64)> = db
            .values()
            .filter(|i| self.item_db_matches(rare, i))
            .map(|i| (i, f64::NEG_INFINITY))
            .collect();
        // The rare list has no sort control: by name
        let stat = (!rare)
            .then(|| sort_list().into_iter().nth(self.db_list(rare).sort - 1))
            .flatten()
            .and_then(|(_, s)| s);
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
        Ok(list
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
}
