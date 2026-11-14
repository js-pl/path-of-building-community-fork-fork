//! The Spectre Library (`buildMode:OpenSpectreLibrary`, Manage
//! Spectres...): the build's spectres (`MinionListControl`, which takes
//! drops and removes rows), all spectres by name (`MinionSearchListControl`
//! with its search and search mode), the rows' tooltips
//! (`AddValueTooltip`), and Save, which sets the build's spectre list.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::tooltip::{Tooltip, cc};
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::breakdown::{Arg, lua_format};
use engine::pat::{gsub, is_match, lower, match_or_pattern};
use serde_json::{Value as J, json};

/// A list's selection: `selIndex` stays when `selValue` goes
#[derive(Clone, Debug, Default)]
struct Sel {
    index: Option<usize>,
    value: Option<String>,
}

impl Sel {
    /// `SelectIndex(index)`: the row's value, nil past the list
    fn select(&mut self, list: &[String], index: f64) -> bool {
        let row = (index >= 1.0 && index.fract() == 0.0)
            .then(|| list.get(index as usize - 1))
            .flatten();
        self.value = row.cloned();
        if row.is_some() {
            self.index = Some(index as usize);
        }
        row.is_some()
    }

    /// `sel` as `controls` gives it: the index while there is a value
    fn json(&self) -> J {
        match (&self.value, self.index) {
            (Some(_), Some(i)) => num(i as f64),
            _ => J::Null,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpectreLibrary {
    /// The build's spectres, as they will be saved
    dest: Vec<String>,
    dest_sel: Sel,
    /// All spectres, by name then id
    unfiltered: Vec<String>,
    /// The ones the search lets through
    source: Vec<String>,
    source_sel: Sel,
    search: String,
    /// Names, Skills or Both
    search_mode: usize,
}

impl<'d> TypedSession<'d> {
    fn minion_name(&self, id: &str) -> String {
        self.data
            .game
            .setup
            .minions
            .get(id)
            .and_then(|m| m.str("name"))
            .unwrap_or_default()
            .to_string()
    }

    /// Manage Spectres...: the build's spectres and all spectres
    pub(super) fn open_spectre_library(&mut self) {
        let minions = &self.data.game.setup.minions;
        let mut source: Vec<(&str, &String)> = minions
            .iter()
            .filter(|(_, m)| m.str("limit") == Some("ActiveSpectreLimit"))
            .map(|(id, m)| (m.str("name").unwrap_or_default(), id))
            .collect();
        source.sort();
        let source: Vec<String> = source.into_iter().map(|(_, id)| id.clone()).collect();
        let p = SpectreLibrary {
            dest: self.build.info.spectre_list.clone(),
            dest_sel: Sel::default(),
            unfiltered: source.clone(),
            source,
            source_sel: Sel::default(),
            search: String::new(),
            search_mode: 1,
        };
        self.editor
            .popups
            .insert(0, ItemPopup::SpectreLibrary(Box::new(p)));
        self.refresh_item_popups();
    }

    pub(super) fn describe_spectre_library(&self, p: &SpectreLibrary) -> J {
        let names = |l: &[String]| l.iter().map(|id| self.minion_name(id)).collect::<Vec<_>>();
        let add = p
            .source_sel
            .value
            .as_ref()
            .is_some_and(|v| !p.dest.contains(v));
        json!({
            "title": "Spectre Library",
            "lines": [],
            "buttons": [
                { "name": "cancel", "label": "Cancel", "enabled": true },
                { "name": "save", "label": "Save", "enabled": true },
            ],
            "controls": [
                {
                    "class": "ButtonControl", "enabled": p.dest_sel.value.is_some(),
                    "label": "Remove", "path": "popup.list.delete", "shown": true,
                },
                {
                    "class": "ButtonControl", "enabled": add, "label": "Add",
                    "path": "popup.source.add", "shown": true,
                },
                {
                    "class": "MinionListControl", "enabled": true,
                    "label": "^7Spectres in Build:", "list": names(&p.dest),
                    "path": "popup.list", "sel": p.dest_sel.json(), "shown": true,
                },
                {
                    "class": "EditControl", "enabled": true, "path": "popup.source.searchText",
                    "shown": true, "text": p.search,
                },
                {
                    "class": "DropDownControl", "enabled": true, "list": ["Names", "Skills", "Both"],
                    "path": "popup.source.searchModeDropDown", "sel": num(p.search_mode as f64),
                    "shown": true,
                },
                {
                    "class": "MinionSearchListControl", "enabled": true,
                    "label": "^7Available Spectres:", "list": names(&p.source),
                    "path": "popup.source", "sel": p.source_sel.json(), "shown": true,
                },
            ],
        })
    }

    /// `DoesEntryMatchFilters`: a malformed pattern matches nothing
    fn spectre_matches(&self, search: &str, id: &str, mode: usize) -> bool {
        let found = |s: &str| match_or_pattern(&lower(s), search).unwrap_or(false);
        let Some(m) = self.data.game.setup.minions.get(id) else {
            return false;
        };
        if (mode == 1 || mode == 3) && found(m.str("name").unwrap_or_default()) {
            return true;
        }
        if mode == 2 || mode == 3 {
            let skills = &self.data.game.skills.skills;
            return m
                .skill_list
                .iter()
                .filter_map(|s| skills.get(s))
                .any(|s| found(&s.name));
        }
        false
    }

    /// `ListFilterChanged(buf, filterMode)`
    fn spectre_filter(&self, p: &mut SpectreLibrary) {
        let search = gsub(&lower(&p.search), "[%-%.%+%[%]%$%^%%%?%*]", "%%%0");
        if is_match(&search, "%S") {
            p.source = p
                .unfiltered
                .iter()
                .filter(|id| self.spectre_matches(&search, id, p.search_mode))
                .cloned()
                .collect();
            p.source_sel.select(&p.source, 1.0);
        } else {
            p.source = p.unfiltered.clone();
        }
    }

    /// `AddSel` of the spectres list: its selection joins the build's
    fn spectre_add_sel(p: &mut SpectreLibrary) {
        if let Some(v) = &p.source_sel.value
            && !p.dest.contains(v)
        {
            p.dest.push(v.clone());
        }
    }

    pub(super) fn spectre_library_control(
        &self,
        p: &mut SpectreLibrary,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let click = req["click"].as_bool() == Some(true);
        let sel = req["sel"].as_f64();
        match name {
            "list.delete" if click => {
                // `OnSelDelete(selIndex, selValue)`, enabled or not
                let i = p
                    .dest_sel
                    .index
                    .filter(|i| *i <= p.dest.len())
                    .ok_or_else(|| format!("{NOT_PORTED}: Remove without a row"))?;
                p.dest.remove(i - 1);
                p.dest_sel = Sel::default();
            }
            "source.add" if click => Self::spectre_add_sel(p),
            "source.searchText" => {
                let text = req["text"].as_str().ok_or("no text")?;
                p.search = text.to_string();
                self.spectre_filter(p);
            }
            "source.searchModeDropDown" => {
                // `SetSel`: clamped, a change filters
                let sel = sel.ok_or("no sel")?.clamp(1.0, 3.0) as usize;
                if sel != p.search_mode {
                    p.search_mode = sel;
                    self.spectre_filter(p);
                }
            }
            "list" if super::list_order::move_to(req).is_some() => {
                // A row dragged within the list
                if p.dest_sel.select(&p.dest, sel.unwrap_or(0.0)) {
                    let at = p.dest_sel.index.unwrap_or(1);
                    let to = super::list_order::move_to(req).unwrap_or(0.0);
                    if super::list_order::move_row(&mut p.dest, at, to)?.is_some() {
                        p.dest_sel.value = None;
                    }
                }
            }
            "source" if req["dragTo"].is_string() => {
                // A spectre dropped on the build's, before row `at`
                if req["dragTo"].as_str() != Some("popup.list") {
                    return Err(format!("no drag target {}", req["dragTo"]));
                }
                if p.source_sel.select(&p.source, sel.unwrap_or(0.0)) {
                    let v = p.source_sel.value.clone().unwrap_or_default();
                    if !p.dest.contains(&v) {
                        let at = req["at"].as_f64().unwrap_or(p.dest.len() as f64 + 1.0);
                        if at < 1.0 || at.fract() != 0.0 || at as usize > p.dest.len() + 1 {
                            return Err(format!("{NOT_PORTED}: a drop at row {at}"));
                        }
                        p.dest.insert(at as usize - 1, v);
                    }
                    p.source_sel.value = None;
                }
            }
            "list" | "source" if req["dragTo"].is_string() => {
                return Err(format!("no drag target {}", req["dragTo"]));
            }
            "list" | "source" if sel.is_some() => {
                let sel = sel.unwrap_or(0.0);
                if name == "list" {
                    p.dest_sel.select(&p.dest, sel);
                } else if p.source_sel.select(&p.source, sel)
                    && click
                    && req["doubleClick"].as_bool() == Some(true)
                {
                    Self::spectre_add_sel(p);
                }
            }
            "list" if req["key"].as_str() == Some("DELETE") => {
                if let Some(i) = p.dest_sel.index.filter(|_| p.dest_sel.value.is_some()) {
                    p.dest.remove(i - 1);
                    p.dest_sel = Sel::default();
                }
            }
            "source" if req["key"].is_string() => {}
            _ => return Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
        Ok(())
    }

    /// Save: the build's spectre list, and a calculation
    pub(super) fn spectre_library_save(&mut self, p: &SpectreLibrary) -> Result<()> {
        self.build.info.spectre_list = p.dest.clone();
        self.ui.unsaved = true;
        self.rebuild()?;
        self.refresh_controls()
    }

    /// `controlTooltip` of a row: `AddValueTooltip`
    pub(super) fn spectre_row_tooltip(
        &self,
        p: &SpectreLibrary,
        name: &str,
        index: Option<usize>,
    ) -> Result<J> {
        let list = match name {
            "list" => &p.dest,
            "source" => &p.source,
            _ => return Err(format!("{NOT_PORTED}: the tooltip of popup.{name}")),
        };
        let mut tip = Tooltip::default();
        let id = index
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| list.get(i));
        if let Some(m) = id.and_then(|id| self.data.game.setup.minions.get(id)) {
            let n = |k: &str| m.num(k).unwrap_or(0.0);
            let f = |fmt: &str, v: f64| lua_format(fmt, &[Arg::Num(v)]);
            tip.add_line(18.0, &format!("^7{}", m.str("name").unwrap_or_default()));
            tip.add_line(14.0, &f("^7Life Multiplier: x%.2f", n("life")));
            if let Some(es) = m.num("energyShield") {
                tip.add_line(14.0, &f("^7Energy Shield: %d%% of base Life", es * 100.0));
            }
            if let Some(a) = m.num("armour") {
                tip.add_line(14.0, &f("^7Armour Multiplier: x%.2f", 1.0 + a));
            }
            if let Some(e) = m.num("evasion") {
                tip.add_line(14.0, &f("^7Evasion Multiplier: x%.2f", 1.0 + e));
            }
            tip.add_line(
                14.0,
                &lua_format(
                    "^7Resistances: %s%d^7/%s%d^7/%s%d^7/%s%d",
                    &[
                        Arg::Str(cc("FIRE")),
                        Arg::Num(n("fireResist")),
                        Arg::Str(cc("COLD")),
                        Arg::Num(n("coldResist")),
                        Arg::Str(cc("LIGHTNING")),
                        Arg::Num(n("lightningResist")),
                        Arg::Str(cc("CHAOS")),
                        Arg::Num(n("chaosResist")),
                    ],
                ),
            );
            tip.add_line(14.0, &f("^7Base Damage: x%.2f", n("damage")));
            tip.add_line(14.0, &f("^7Base Attack Speed: %.2f", 1.0 / n("attackTime")));
            let skills = &self.data.game.skills.skills;
            for s in m.skill_list.iter().filter_map(|s| skills.get(s)) {
                tip.add_line(14.0, &format!("^7Skill: {}", s.name));
            }
        }
        Ok(tip.json())
    }
}
