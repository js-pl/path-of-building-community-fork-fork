//! What the build mode's sidebar reads of the sidebar's calculation
//! (`calcsTab.mainEnv`) besides the outputs: the main skills' flags and
//! messages, the socket groups' active skills for the skill selection,
//! the item warnings and the aspects in use (`buildMode:RefreshStatList`,
//! `RefreshSkillSelectControls`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::calc::PLAYER;
use crate::calc_items::ItemWarnings;
use crate::calc_perform::{Perform, Result};
use crate::mods::ModValue;
use std::collections::BTreeSet;
use std::sync::Arc;

/// A main skill as the sidebar shows it
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillInfo {
    pub flags: BTreeSet<String>,
    pub info_message: Option<String>,
    pub info_message2: Option<String>,
    pub disable_reason: Option<String>,
}

impl SkillInfo {
    fn of(m: &crate::calc_skill_mods::SkillMods) -> SkillInfo {
        SkillInfo {
            flags: m.skill_flags.clone(),
            info_message: m.info_message.clone(),
            info_message2: m.info_message2.clone(),
            disable_reason: m.disable_reason.clone(),
        }
    }

    pub fn flag(&self, f: &str) -> bool {
        self.flags.contains(f)
    }
}

/// An active skill of a socket group (`displaySkillList`)
#[derive(Clone, Debug, PartialEq)]
pub struct DisplaySkill {
    pub granted_effect: Arc<crate::skill_data::GrantedEffect>,
    /// The gem it comes from (`activeEffect.srcInstance`): group and place
    pub src: Option<(usize, usize)>,
    pub flags: BTreeSet<String>,
    pub stages_max: Option<f64>,
    pub stages_min: Option<f64>,
    /// `activeSkill.minionList`
    pub minion_list: Vec<String>,
    /// The names of its minion's skills (`minion.activeSkillList`)
    pub minion_skills: Option<Vec<String>>,
}

/// What the sidebar reads of the sidebar's calculation
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sidebar {
    pub main_skill: SkillInfo,
    /// `env.minion.mainSkill`
    pub minion_skill: Option<SkillInfo>,
    /// Each socket group's active skills
    pub groups: Vec<Vec<DisplaySkill>>,
    /// `env.mainSocketGroup`
    pub main_socket_group: usize,
    pub item_warnings: ItemWarnings,
    /// The player's `Condition:AspectOfTheSpiderActive`, then Cat's
    /// Agility or Stealth, then Avian's Flight or Might
    pub aspects: [bool; 3],
    /// The same of `env.minion`
    pub minion_aspects: [bool; 3],
    /// `env.grantedPassives`
    pub granted_passives: BTreeSet<u32>,
    /// Whether each socket group's slot is in use (`group.slotEnabled`)
    pub slot_enabled: Vec<bool>,
    /// The player's `AdditionalRingSlot` flag, which shows the third ring
    pub additional_ring_slot: bool,
    /// Whether Saviour's mirage replaced the main skill, whose gem level
    /// and quality outputs are not ported
    pub main_copy: bool,
}

fn num(v: Option<&ModValue>) -> Option<f64> {
    v.and_then(ModValue::as_number)
}

/// The sidebar's view of a finished calculation
pub(crate) fn collect(p: &Perform) -> Result<Sidebar> {
    let main_skill = match &p.main_copy {
        Some(m) => SkillInfo::of(m),
        None => SkillInfo::of(p.mods(p.main())),
    };
    let minion_skill = p
        .env_minion
        .map(|m| SkillInfo::of(p.mods(p.minions[m].main_skill)));
    let groups = p
        .env
        .skills
        .group_skills
        .iter()
        .map(|list| {
            list.iter()
                .map(|&i| {
                    let skill = p.skill(i);
                    let mods = skill.mods.as_ref();
                    let minion = p.minions.iter().find(|m| m.skill == i);
                    DisplaySkill {
                        granted_effect: skill.active_effect.granted_effect.clone(),
                        src: skill.active_effect.src,
                        flags: mods
                            .map_or_else(|| skill.skill_flags.clone(), |m| m.skill_flags.clone()),
                        stages_max: num(mods.and_then(|m| m.skill_data.get("stagesMax"))),
                        stages_min: num(mods.and_then(|m| m.skill_data.get("stagesMin"))),
                        minion_list: mods.map(|m| m.minion_list.clone()).unwrap_or_default(),
                        minion_skills: minion.map(|m| {
                            m.skills
                                .iter()
                                .map(|&k| p.skill_name(k).to_string())
                                .collect()
                        }),
                    }
                })
                .collect()
        })
        .collect();
    let aspects = |actor: usize| -> Result<[bool; 3]> {
        let db = p.c.db(actor);
        let flag = |names: &[&str]| -> Result<bool> {
            for n in names {
                let name = format!("Condition:{n}");
                if p.c.flag(db, None, &[name.as_str()])? {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        Ok([
            flag(&["AspectOfTheSpiderActive"])?,
            flag(&["CatsAgilityActive", "CatsStealthActive"])?,
            flag(&["AviansFlightActive", "AviansMightActive"])?,
        ])
    };
    Ok(Sidebar {
        main_skill,
        minion_skill,
        groups,
        main_socket_group: p.env.skills.main_socket_group,
        item_warnings: p.env.setup.items.warnings.clone(),
        aspects: aspects(PLAYER)?,
        minion_aspects: match p.minion_actor() {
            Some(a) => aspects(a)?,
            None => [false; 3],
        },
        granted_passives: p.env.setup.tree.granted_passives.clone(),
        slot_enabled: p.env.skills.slot_enabled.clone(),
        additional_ring_slot: p.c.flag(p.c.db(PLAYER), None, &["AdditionalRingSlot"])?,
        main_copy: p.main_copy.is_some(),
    })
}
