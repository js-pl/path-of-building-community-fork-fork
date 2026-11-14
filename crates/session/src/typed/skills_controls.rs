//! The Skills tab's own controls above and below the socket group list: the
//! skill set drop-down (`setSelect`, whose list the tab's draw sets) and the
//! gem options (`sortGemsByDPS`, `sortGemsByDPSFieldControl`,
//! `defaultLevel` with its tooltip, `defaultQuality`,
//! `showSupportGemTypes`, `showLegacyGems`), which the gem list reads and
//! the build saves.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession, num};
use engine::skills::DefaultGemLevel;
use engine::xml::Element;
use serde_json::{Value as J, json};

/// `defaultGemLevelList`: label, level and description
const DEFAULT_LEVELS: [(&str, DefaultGemLevel, &str); 5] = [
    (
        "Normal Maximum",
        DefaultGemLevel::NormalMaximum,
        "All gems default to their highest valid non-corrupted gem level.",
    ),
    (
        "Corrupted Maximum",
        DefaultGemLevel::CorruptedMaximum,
        "Normal gems default to their highest valid corrupted gem level.\nAwakened gems default to their highest valid non-corrupted gem level.",
    ),
    (
        "Awakened Maximum",
        DefaultGemLevel::AwakenedMaximum,
        "All gems default to their highest valid corrupted gem level.",
    ),
    (
        "Match Character Level",
        DefaultGemLevel::CharacterLevel,
        "All gems default to their highest valid non-corrupted gem level that your character meets the level requirement for.\nThis hides gems with a minimum level requirement above your character level, preventing them from showing up in the dropdown list.",
    ),
    (
        "Level 1",
        DefaultGemLevel::LevelOne,
        "All gems default to level 1.",
    ),
];

/// `showSupportGemTypeList`
const SHOW_SUPPORTS: [(&str, &str); 3] = [
    ("All", "ALL"),
    ("Non-Exceptional", "NORMAL"),
    ("Exceptional", "EXCEPTIONAL"),
];

/// `sortGemTypeList`
const SORT_FIELDS: [(&str, &str); 9] = [
    ("Full DPS", "FullDPS"),
    ("Combined DPS", "CombinedDPS"),
    ("Hit DPS", "TotalDPS"),
    ("Average Hit", "AverageDamage"),
    ("DoT DPS", "TotalDot"),
    ("Bleed DPS", "BleedDPS"),
    ("Ignite DPS", "IgniteDPS"),
    ("Poison DPS", "TotalPoisonDPS"),
    ("Effective Hit Pool", "TotalEHP"),
];

const PATHS: [&str; 8] = [
    "skills.setSelect",
    "skills.setManage",
    "skills.sortGemsByDPS",
    "skills.sortGemsByDPSFieldControl",
    "skills.defaultLevel",
    "skills.defaultQuality",
    "skills.showSupportGemTypes",
    "skills.showLegacyGems",
];

/// The Skills tab's gem options and their controls' selections
#[derive(Clone, Debug)]
pub struct GemOptions {
    /// `showSupportGemTypes`: `ALL`, `NORMAL` or `EXCEPTIONAL`, its
    /// drop-down's row
    pub show: &'static str,
    show_sel: usize,
    pub show_legacy: bool,
    pub sort_by_dps: bool,
    /// `sortGemsByDPSField`, and its drop-down's row: a tab without a
    /// load sorts by Combined DPS with Full DPS selected
    pub field: &'static str,
    field_sel: usize,
    /// `defaultQuality`'s text
    quality_text: String,
}

impl GemOptions {
    /// The tab's own defaults, and what `SkillsTab:Load` sets from a
    /// build's `<Skills>`: a value no row has keeps the row selected
    pub fn load(xml: Option<&Element>, quality: Option<f64>) -> GemOptions {
        let mut o = GemOptions {
            show: "ALL",
            show_sel: 1,
            show_legacy: false,
            sort_by_dps: true,
            field: "CombinedDPS",
            field_sel: 1,
            quality_text: String::new(),
        };
        let Some(xml) = xml else {
            return o;
        };
        let attr = |k: &str| xml.attr(k);
        if let Some(v) = attr("sortGemsByDPS") {
            o.sort_by_dps = v == "true";
        }
        if let Some(v) = attr("showLegacyGems") {
            o.show_legacy = v == "true";
        }
        let show = attr("showSupportGemTypes").unwrap_or("ALL");
        if let Some(i) = SHOW_SUPPORTS.iter().position(|r| r.1 == show) {
            o.show_sel = i + 1;
        }
        o.show = SHOW_SUPPORTS[o.show_sel - 1].1;
        let field = attr("sortGemsByDPSField").unwrap_or("CombinedDPS");
        if let Some(i) = SORT_FIELDS.iter().position(|r| r.1 == field) {
            o.field_sel = i + 1;
        }
        o.field = SORT_FIELDS[o.field_sel - 1].1;
        if let Some(q) = quality {
            o.quality_text = engine::pat::num(q);
        }
        o
    }
}

/// `SetSel`'s row: clamped to the list
fn clamp_sel(sel: f64, n: usize) -> usize {
    sel.min(n as f64).max(1.0) as usize
}

impl<'d> TypedSession<'d> {
    pub fn is_skills_control(path: &str) -> bool {
        PATHS.contains(&path)
    }

    /// The skill sets' titles, and the active one's row
    fn skill_set_select_list(&self) -> (Vec<String>, usize) {
        let Some(tab) = &self.build.skills else {
            return (vec![], 1);
        };
        let mut sel = 1;
        let list = tab
            .skill_set_order_list
            .iter()
            .enumerate()
            .map(|(i, id)| {
                if *id == tab.active_skill_set_id {
                    sel = i + 1;
                }
                tab.skill_sets
                    .get(&(*id as i64))
                    .and_then(|s| s.title.clone())
                    .unwrap_or_else(|| "Default".into())
            })
            .collect();
        (list, sel)
    }

    fn default_level_sel(&self) -> usize {
        let level = self
            .build
            .skills
            .as_ref()
            .map_or(DefaultGemLevel::NormalMaximum, |s| s.default_gem_level);
        DEFAULT_LEVELS
            .iter()
            .position(|r| r.1 == level)
            .map_or(1, |i| i + 1)
    }

    /// `controls` of one of the controls
    pub fn skills_control_json(&self, path: &str) -> Option<J> {
        let drop = |list: Vec<String>, sel: usize, enabled: bool| {
            json!({
                "class": "DropDownControl", "shown": true, "enabled": enabled,
                "list": list, "sel": num(sel as f64),
            })
        };
        let check = |label: &str, state: bool| {
            json!({
                "class": "CheckBoxControl", "shown": true, "enabled": true,
                "label": label, "state": state,
            })
        };
        let labels = |rows: &[&str]| rows.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let o = &self.gem_options;
        Some(match path {
            "skills.setSelect" => {
                let (list, sel) = self.skill_set_select_list();
                let enabled = list.len() > 1;
                drop(list, sel, enabled)
            }
            "skills.setManage" => json!({
                "class": "ButtonControl", "shown": true, "enabled": true, "label": "Manage...",
            }),
            "skills.sortGemsByDPS" => check("Sort gems by DPS:", o.sort_by_dps),
            "skills.showLegacyGems" => check("^7Show legacy gems:", o.show_legacy),
            "skills.sortGemsByDPSFieldControl" => {
                drop(labels(&SORT_FIELDS.map(|r| r.0)), o.field_sel, true)
            }
            "skills.showSupportGemTypes" => {
                drop(labels(&SHOW_SUPPORTS.map(|r| r.0)), o.show_sel, true)
            }
            "skills.defaultLevel" => drop(
                labels(&DEFAULT_LEVELS.map(|r| r.0)),
                self.default_level_sel(),
                true,
            ),
            "skills.defaultQuality" => json!({
                "class": "EditControl", "shown": true, "enabled": true, "text": o.quality_text,
            }),
            _ => return None,
        })
    }

    /// `control` of one of the controls
    pub fn skills_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let sel = req["sel"].as_f64();
        let state = req["state"].as_bool();
        let text = req["text"].as_str();
        match (path, sel, state, text) {
            ("skills.setSelect", Some(sel), _, _) => {
                // The selection follows the active set at every draw; a
                // change selects the row's set and adds an undo state
                let (list, cur) = self.skill_set_select_list();
                let sel = clamp_sel(sel, list.len());
                if !list.is_empty() && sel != cur {
                    let id = self
                        .build
                        .skills
                        .as_ref()
                        .ok_or("no skills tab")?
                        .skill_set_order_list[sel - 1];
                    self.select_skill_set(Some(id))?;
                    self.ui.unsaved = true;
                }
            }
            ("skills.sortGemsByDPS", _, Some(state), _) => self.gem_options.sort_by_dps = state,
            ("skills.showLegacyGems", _, Some(state), _) => self.gem_options.show_legacy = state,
            ("skills.sortGemsByDPSFieldControl", Some(sel), _, _) => {
                let sel = clamp_sel(sel, SORT_FIELDS.len());
                if sel != self.gem_options.field_sel {
                    self.gem_options.field_sel = sel;
                    self.gem_options.field = SORT_FIELDS[sel - 1].1;
                }
            }
            ("skills.showSupportGemTypes", Some(sel), _, _) => {
                let sel = clamp_sel(sel, SHOW_SUPPORTS.len());
                if sel != self.gem_options.show_sel {
                    self.gem_options.show_sel = sel;
                    self.gem_options.show = SHOW_SUPPORTS[sel - 1].1;
                }
            }
            ("skills.defaultLevel", Some(sel), _, _) => {
                let sel = clamp_sel(sel, DEFAULT_LEVELS.len());
                if sel != self.default_level_sel() {
                    let tab = self.build.skills.as_mut().ok_or("no skills tab")?;
                    tab.default_gem_level = DEFAULT_LEVELS[sel - 1].1;
                }
            }
            ("skills.defaultQuality", _, _, Some(text)) => {
                // `m_min(tonumber(buf) or 0, 23)`
                let tab = self.build.skills.as_mut().ok_or("no skills tab")?;
                tab.default_gem_quality = engine::pat::to_number(text).unwrap_or(0.0).min(23.0);
                self.gem_options.quality_text = text.to_string();
                self.gem_quality_nil = false;
            }
            _ => return Err(format!("{NOT_PORTED}: control {path}")),
        }
        Ok(json!([]))
    }

    /// `controlTooltip` of the default gem level drop-down: the entry's
    /// description (indexing a missing entry fails upstream)
    pub fn skills_control_tooltip(&self, index: Option<f64>) -> Result<J> {
        let row = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0)
            .and_then(|i| DEFAULT_LEVELS.get(i as usize - 1));
        let Some(row) = row else {
            return Err(format!(
                "{}attempt to index a nil value",
                super::UPSTREAM_ERROR
            ));
        };
        let mut tip = super::tooltip::Tooltip::default();
        tip.add_line(16.0, &format!("^7{}", row.2));
        Ok(tip.json())
    }
}
