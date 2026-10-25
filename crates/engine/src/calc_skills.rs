//! The skills of the calculation setup: the part of `calcs.initEnv`
//! (`Modules/CalcSetup.lua`) that adds the skills items and passive nodes
//! grant, and the explosions on kill, to the Skills tab's socket groups.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::calc_items::ItemSkill;
use crate::calc_setup::NodeMods;
use crate::mods::ModValue;
use crate::skill_data::SkillData;
use crate::skills::{
    ExplodeSource, GemInstance, SocketGroup, process_socket_group, validate_gem_level,
};
use std::collections::HashSet;

/// A skill an item or a passive node grants (`env.grantedSkills`)
#[derive(Clone, Debug, PartialEq)]
pub struct GrantedSkill {
    pub skill_id: Option<String>,
    pub level: Option<f64>,
    pub no_supports: bool,
    pub source: String,
    pub triggered: bool,
    pub trigger_chance: Option<f64>,
    /// The name of the skill an item grants (`nameSpec`)
    pub name_spec: Option<String>,
    pub slot_name: Option<String>,
    pub source_item: Option<f64>,
    pub source_node: Option<u32>,
}

/// `env.grantedSkills`: the nodes' skills, in allocation order, then the
/// items'
pub fn granted_skills(
    nodes: &[(u32, NodeMods)],
    items: &[ItemSkill],
    data: &SkillData,
) -> Vec<GrantedSkill> {
    let text = |v: &Option<ModValue>| match v {
        Some(ModValue::Str(s)) => Some(s.to_string()),
        _ => None,
    };
    let mut out = Vec::new();
    for (id, node) in nodes {
        for skill in &node.granted_skills {
            out.push(GrantedSkill {
                skill_id: text(&skill.skill_id),
                level: skill.level.as_ref().and_then(ModValue::as_number),
                no_supports: true,
                source: skill.source.clone(),
                triggered: false,
                trigger_chance: None,
                name_spec: None,
                slot_name: None,
                source_item: None,
                source_node: Some(*id),
            });
        }
    }
    for s in items {
        let skill_id = s.skill.skill_id.as_deref().map(String::from);
        out.push(GrantedSkill {
            name_spec: skill_id
                .as_ref()
                .and_then(|id| data.skills.get(id))
                .map(|e| e.name.clone()),
            skill_id,
            level: s.skill.level,
            no_supports: s.skill.no_supports,
            source: s.skill.source.clone(),
            triggered: s.skill.triggered,
            trigger_chance: s.skill.trigger_chance,
            slot_name: Some(s.slot_name.clone()),
            source_item: Some(s.item_id),
            source_node: None,
        });
    }
    out
}

/// The level of a granted skill as a gem's (`getNormalizedSkillLevel`)
fn normalized_level(skill: &GrantedSkill, data: &SkillData) -> Option<f64> {
    let mut gem = GemInstance {
        granted_effect: skill
            .skill_id
            .as_ref()
            .and_then(|id| data.skills.get(id))
            .cloned(),
        level: skill.level,
        ..GemInstance::default()
    };
    validate_gem_level(&mut gem);
    gem.level
}

/// Adds a socket group for each granted skill, or updates the one from
/// before, adds the explosion group, and removes the groups of sources
/// that are gone
pub fn add_granted_groups(
    groups: &mut Vec<SocketGroup>,
    granted: &[GrantedSkill],
    explode_sources: &[ExplodeSource],
    data: &SkillData,
) {
    let mut marked: HashSet<usize> = HashSet::new();
    for skill in granted {
        let found = groups.iter().position(|g| {
            g.source.as_deref() == Some(skill.source.as_str())
                && g.slot == skill.slot_name
                && g.gem_list.first().is_some_and(|gem| {
                    gem.skill_id == skill.skill_id
                        && (gem.level == skill.level || gem.level == normalized_level(skill, data))
                })
        });
        let i = match found {
            Some(i) => i,
            None => {
                groups.push(SocketGroup {
                    label: Some(String::new()),
                    enabled: true,
                    source: Some(skill.source.clone()),
                    slot: skill.slot_name.clone(),
                    main_active_skill: 1.0,
                    main_active_skill_calcs: 1.0,
                    ..SocketGroup::default()
                });
                groups.len() - 1
            }
        };
        marked.insert(i);
        let group = &mut groups[i];
        group.source_item = skill.source_item;
        group.source_node = skill.source_node;
        let mut gem = group
            .gem_list
            .first()
            .cloned()
            .unwrap_or_else(|| GemInstance {
                skill_id: skill.skill_id.clone(),
                name_spec: skill.name_spec.clone().unwrap_or_default(),
                quality: Some(0.0),
                enabled: true,
                ..GemInstance::default()
            });
        gem.from_item = skill.source_item.is_some();
        gem.gem_id = None;
        gem.level = skill.level;
        gem.enable_global1 = true;
        gem.no_supports = skill.no_supports;
        group.no_supports = skill.no_supports;
        gem.triggered = skill.triggered;
        gem.trigger_chance = skill.trigger_chance;
        group.gem_list = vec![gem];
        process_socket_group(group, data);
    }
    if !explode_sources.is_empty() {
        let i = match groups
            .iter()
            .position(|g| g.source.as_deref() == Some("Explode"))
        {
            Some(i) => i,
            None => {
                groups.push(SocketGroup {
                    label: Some("On Kill Monster Explosion".into()),
                    enabled: true,
                    source: Some("Explode".into()),
                    no_supports: true,
                    main_active_skill: 1.0,
                    main_active_skill_calcs: 1.0,
                    ..SocketGroup::default()
                });
                groups.len() - 1
            }
        };
        let group = &mut groups[i];
        group.explode_sources = explode_sources.to_vec();
        let old = std::mem::take(&mut group.gem_list);
        for source in explode_sources {
            let gem = old
                .iter()
                .find(|g| g.explode_source.as_ref() == Some(source))
                .cloned()
                .unwrap_or_else(|| GemInstance {
                    skill_id: Some("EnemyExplode".into()),
                    quality: Some(0.0),
                    enabled: true,
                    level: Some(1.0),
                    triggered: true,
                    explode_source: Some(source.clone()),
                    ..GemInstance::default()
                });
            group.gem_list.push(gem);
        }
        marked.insert(i);
        process_socket_group(group, data);
    }
    let mut i = 0;
    groups.retain(|g| {
        let keep = g.source.is_none() || marked.contains(&i);
        i += 1;
        keep
    });
}
