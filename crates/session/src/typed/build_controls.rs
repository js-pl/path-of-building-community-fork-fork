//! The build's own controls beyond the skill and class drop-downs: the
//! loadouts drop-down (`buildLoadouts`, with New Loadout's name popup), the
//! level scaling button (`levelScalingButton`), the view mode buttons
//! (`modeImport` ... `modeCompare`), and the descriptions of Back, Save,
//! Save As and Manage Spectres....
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::spec::PassiveSpec;
use serde_json::{Value as J, json};
use std::sync::Arc;

/// The view mode buttons: name, label and mode
const MODES: [(&str, &str, &str); 9] = [
    ("modeImport", "Import/Export Build", "IMPORT"),
    ("modeNotes", "Notes", "NOTES"),
    ("modeConfig", "Configuration", "CONFIG"),
    ("modeTree", "Tree", "TREE"),
    ("modeSkills", "Skills", "SKILLS"),
    ("modeItems", "Items", "ITEMS"),
    ("modeCalcs", "Calcs", "CALCS"),
    ("modeParty", "Party", "PARTY"),
    ("modeCompare", "Compare", "COMPARE"),
];

/// New Loadout's name popup (`Set Name`)
#[derive(Clone, Debug)]
pub struct NewLoadout {
    buf: String,
    save_enabled: bool,
}

impl<'d> TypedSession<'d> {
    pub fn is_build_control(path: &str) -> bool {
        let Some(name) = path.strip_prefix("build.") else {
            return false;
        };
        matches!(
            name,
            "buildLoadouts"
                | "levelScalingButton"
                | "back"
                | "save"
                | "saveAs"
                | "mainSkillMinionLibrary"
        ) || MODES.iter().any(|m| m.0 == name)
    }

    /// `controls` of one of the controls
    pub fn build_control_json(&self, path: &str) -> Option<J> {
        let name = path.strip_prefix("build.")?;
        let button = |label: &str, shown: bool, enabled: bool| json!({ "class": "ButtonControl", "shown": shown, "enabled": enabled, "label": label });
        if let Some(m) = MODES.iter().find(|m| m.0 == name) {
            return Some(button(m.1, true, true));
        }
        Some(match name {
            "buildLoadouts" => json!({
                "class": "DropDownControl", "shown": true, "enabled": true,
                "list": self.loadout_list(), "sel": num(self.loadout.sel as f64),
            }),
            "levelScalingButton" => button(
                self.controls.level_scaling_label.unwrap_or("Manual"),
                true,
                true,
            ),
            // The session has no file: Save is enabled, Save As not
            "back" => button("<< Back", true, true),
            "save" => button("Save", true, true),
            "saveAs" => button("Save As", true, false),
            "mainSkillMinionLibrary" => {
                // Anchored to the minion drop-down: shown with it
                let c = &self.controls.skill_select;
                let shown = c.minion.shown && c.library.unwrap_or(true);
                button("Manage Spectres...", shown, true)
            }
            _ => return None,
        })
    }

    /// `control` of one of the controls
    pub fn build_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let name = path.strip_prefix("build.").unwrap_or("");
        let click = req["click"].as_bool() == Some(true);
        if let (Some(m), true) = (MODES.iter().find(|m| m.0 == name), click) {
            // `self.viewMode = ...`; Import/Export also fetches the last
            // realm's characters, which the session has none of
            self.view_mode = m.2.to_string();
            return Ok(json!([]));
        }
        match (name, req["sel"].as_f64(), click) {
            ("buildLoadouts", Some(sel), _) => self.select_loadout(sel)?,
            ("levelScalingButton", _, true) => {
                let info = &mut self.build.info;
                info.character_level_auto_mode = !info.character_level_auto_mode;
                self.controls.level_scaling_label = Some(if info.character_level_auto_mode {
                    "Auto"
                } else {
                    "Manual"
                });
                self.build
                    .build_config_mods(&self.data.game.config, &self.data.parser)?;
                self.config_mods_built();
                self.ui.unsaved = true;
                self.rebuild()?;
            }
            _ => return Err(format!("{NOT_PORTED}: control {path}")),
        }
        Ok(json!([]))
    }

    pub fn is_build_tooltip(path: &str) -> bool {
        matches!(
            path,
            "build.characterLevel" | "build.mainSocketGroup" | "build.mainSkillMinion"
        )
    }

    /// `controlTooltip` of the level box (the experience multiplier by area
    /// level), the main socket group (`AddSocketGroupTooltip` of the
    /// entry's group) or the minion drop-down (an item set entry's
    /// `AddItemSetTooltip`, and the tip to drag items onto it)
    pub fn build_control_tooltip(&mut self, path: &str, index: Option<f64>) -> Result<J> {
        let mut tip = super::tooltip::Tooltip::default();
        match path {
            "build.characterLevel" => {
                tip.add_line(16.0, "Experience multiplier:");
                let player = self.build.info.character_level;
                let safe_zone = 3.0 + (player / 16.0).floor();
                for (k, exp_level) in self.data.game.monster_experience_levels.iter().enumerate() {
                    let level = k + 1;
                    let diff = (player - exp_level).abs() - safe_zone;
                    let mut mult = if diff <= 0.0 {
                        1.0
                    } else {
                        ((player + 5.0) / (player + 5.0 + diff.powf(2.5))).powf(1.5)
                    };
                    if player >= 95.0 {
                        let penalty = [0.935, 0.885, 0.813, 0.7175, 0.6]
                            .get((player - 95.0) as usize)
                            .copied()
                            .unwrap_or(0.0);
                        mult *= (1.0 / (1.0 + 0.1 * (player - 94.0))) * penalty;
                    }
                    if mult > 0.01 {
                        let mut line = level.to_string();
                        if level >= 68 {
                            line += &format!(" (Tier {})", level - 67);
                        }
                        line += &format!(": {}%", engine::pat::format_f(mult * 100.0, 1));
                        tip.add_line(14.0, &line);
                    }
                }
            }
            "build.mainSocketGroup" => {
                let n = self.groups().len();
                return match index {
                    Some(i) if i >= 1.0 && i.fract() == 0.0 && i as usize <= n => {
                        self.socket_group_tooltip(i)
                    }
                    _ => Ok(tip.json()),
                };
            }
            _ => {
                // `value.itemSetId` of a missing entry fails upstream
                let c = &self.controls.skill_select;
                let entry = index
                    .filter(|i| *i >= 1.0 && i.fract() == 0.0)
                    .and_then(|i| c.minion_entry(i as usize - 1));
                match entry {
                    None => {
                        return Err(format!(
                            "{}attempt to index a nil value",
                            super::UPSTREAM_ERROR
                        ));
                    }
                    Some(super::sidebar::MinionEntry::ItemSet(id)) => {
                        self.add_item_set_tooltip(&mut tip, id);
                        tip.add_separator(14.0);
                        tip.add_line(
                            14.0,
                            &format!(
                                "{}Tip: You can drag items from the Items tab onto this dropdown to equip them onto the minion.",
                                super::tooltip::cc("TIP")
                            ),
                        );
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(tip.json())
    }

    /// New Loadout: `Set Name`, its edit holding "New Loadout"
    pub(super) fn open_new_loadout(&mut self) {
        self.editor.popups.insert(
            0,
            ItemPopup::NewLoadout(Box::new(NewLoadout {
                buf: "New Loadout".into(),
                save_enabled: false,
            })),
        );
        self.refresh_item_popups();
    }

    pub(super) fn describe_new_loadout(&self, p: &NewLoadout) -> J {
        json!({
            "title": "Set Name",
            "lines": [],
            "buttons": [
                { "name": "cancel", "label": "Cancel", "enabled": true },
                { "name": "save", "label": "Save", "enabled": p.save_enabled },
            ],
            "controls": [{
                "class": "EditControl", "enabled": true, "path": "popup.edit",
                "shown": true, "text": p.buf,
            }],
        })
    }

    /// `control` of the name popup's edit: `save.enabled = buf:match("%S")`,
    /// a string or nil, never false
    pub(super) fn new_loadout_control(
        &self,
        p: &mut NewLoadout,
        name: &str,
        req: &J,
    ) -> Result<()> {
        match (name, req["text"].as_str()) {
            ("edit", Some(t)) => {
                p.buf = t.to_string();
                p.save_enabled = true;
                Ok(())
            }
            _ => Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
    }

    /// `#t + 1` of a table whose keys are these ids: the next id after
    /// 1..n, when they are exactly that
    fn next_set_id(ids: impl Iterator<Item = f64>) -> Result<f64> {
        let mut ids: Vec<f64> = ids.collect();
        ids.sort_by(f64::total_cmp);
        let contiguous = ids.iter().enumerate().all(|(i, id)| *id == (i + 1) as f64);
        if !contiguous {
            return Err(format!(
                "{NOT_PORTED}: a new loadout with sets that are not numbered 1 to n"
            ));
        }
        Ok(ids.len() as f64 + 1.0)
    }

    /// The name popup's Save: a spec of the latest tree, and an item, skill
    /// and config set, all named after the loadout
    pub(super) fn new_loadout_save(&mut self, p: &NewLoadout) -> Result<()> {
        let name = p.buf.clone();
        let item_id = Self::next_set_id(self.build.items.item_sets.keys().map(|k| *k as f64))?;
        let skill_id = Self::next_set_id(
            self.build
                .skills
                .as_ref()
                .ok_or("no skills tab")?
                .skill_sets
                .values()
                .map(|s| s.id),
        )?;
        let config_id = Self::next_set_id(self.build.config.config_sets.values().map(|s| s.id))?;
        let latest = self.data.game.latest_tree_version().to_string();
        let mut spec = PassiveSpec::new(self.data.tree(&latest)?, &self.data.parser)?;
        spec.select_class(0)?;
        spec.title = Some(name.clone());
        if self.other_specs.is_empty() {
            // The list held the active spec alone
            self.other_specs.push(None);
        }
        self.other_specs.push(Some(Arc::new(spec)));
        self.specs.1.push(name.clone());
        let mut set = self.build.items.new_item_set(item_id);
        set.title = Some(name.clone());
        self.build.items.item_sets.insert(item_id as i64, set);
        self.build.items.item_set_order_list.push(item_id);
        let skills = self.build.skills.as_mut().ok_or("no skills tab")?;
        skills.new_skill_set(skill_id);
        if let Some(s) = skills.skill_sets.values_mut().find(|s| s.id == skill_id) {
            s.title = Some(name.clone());
        }
        skills.skill_set_order_list.push(skill_id);
        let defaults = &self.data.game.option_defaults;
        let config = Arc::make_mut(&mut self.build.config);
        config.new_set(config_id, Some(&name), defaults);
        config.config_set_order_list.push(config_id);
        self.ui.unsaved = true;
        self.sync_loadouts_and_rebuild()?;
        // The next frame's `RefreshSkillSelectControls`: the minion
        // drop-down lists the new item set
        self.refresh_controls()
    }
}
