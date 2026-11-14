//! The Skills tab's gem dropdown (`Classes/GemSelectControl.lua`) of a
//! socket group's gem slot: the gems it lists (`PopulateGemList`,
//! `FilterSupport`), which supports apply to the group's active skills
//! (`UpdateSortCache`), each candidate's DPS in the slot (`DPSBuilder` on
//! `engine::compare::gem_power`) and the list's order (`BuildList("")`,
//! `SortGemList`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedSession, num};
use engine::calc_env::{CalcMode, init_env};
use engine::calc_skills::effect_supports;
use engine::calculator::MiscCalculator;
use engine::compare::{self, GemCandidate};
use engine::parallel::Threads;
use engine::skill_data::Gem;
use engine::skills::{DefaultGemLevel, GemInstance, process_gem_level};
use serde_json::{Map, Value as J, json};
use std::collections::HashSet;
use std::sync::Arc;

impl TypedSession<'_> {
    /// `gemList` of a group's gem slot (both 1-based; the slot after the
    /// last adds a gem): the gems of its dropdown in their order, each
    /// with its DPS in the slot and whether it supports the group's skills
    pub fn gem_select_list(&self, group: f64, slot: f64) -> Result<J> {
        let opts = &self.gem_options;
        let skills_tab = self.build.skills.as_ref().ok_or("no skills tab")?;
        let groups = skills_tab.socket_group_list();
        let g = (group as usize)
            .checked_sub(1)
            .filter(|&g| g < groups.len())
            .ok_or_else(|| format!("no socket group {group}"))?;
        let k = (slot as usize)
            .checked_sub(1)
            .filter(|&k| k <= groups[g].gem_list.len())
            .ok_or_else(|| format!("no gem slot {slot}"))?;
        let data = &self.data.game.skills;
        let char_level = self.build.info.character_level;
        let match_level = skills_tab.default_gem_level == DefaultGemLevel::CharacterLevel;

        // `PopulateGemList`
        let gems: Vec<&Arc<Gem>> = data
            .gems()
            .iter()
            .filter(|gem| {
                let e = &gem.granted_effect;
                if e.hide_from_gem_list {
                    return false;
                }
                let req = e.level(1.0).map_or(1.0, |l| l.level_requirement);
                if char_level < req && match_level {
                    return false;
                }
                let legacy_awakened = e.legacy && e.plus_version_of.is_some();
                let exceptional = gem
                    .tag_string
                    .as_deref()
                    .is_some_and(|t| t.contains("Exceptional"));
                let (all, normal, exc) = (
                    opts.show == "ALL",
                    opts.show == "NORMAL",
                    opts.show == "EXCEPTIONAL",
                );
                if (exc || all) && (legacy_awakened || exceptional) {
                    opts.show_legacy || !e.legacy
                } else {
                    normal || all
                }
            })
            .collect();

        // `UpdateSortCache`: the supports that apply to the group's active
        // skills (`displaySkillList` of the sidebar's calculation), else to
        // the skills of items in the group's slot
        let calc_data = self.data.calc_data();
        let mut b = self.build.clone();
        let env = init_env(&mut b, &calc_data, CalcMode::Main)?;
        let supports_any = |skills: &[usize]| -> HashSet<String> {
            gems.iter()
                .filter(|gem| gem.granted_effect.support)
                .filter(|gem| {
                    skills.iter().any(|&i| {
                        effect_supports(&gem.granted_effect, &env.skills, i, &env.groups, data)
                    })
                })
                .map(|gem| gem.id.clone())
                .collect()
        };
        let group_skills = |i: usize| env.skills.group_skills.get(i).cloned().unwrap_or_default();
        let mut can_support = HashSet::new();
        let own = group_skills(g);
        if !own.is_empty() {
            can_support = supports_any(&own);
        } else if let Some(slot) = &groups[g].slot {
            for (i, other) in env.groups.iter().enumerate() {
                let skills = group_skills(i);
                if other.source.is_some()
                    && other.slot.as_deref() == Some(slot)
                    && !skills.is_empty()
                {
                    can_support.extend(supports_any(&skills));
                }
            }
        }

        // `DPSBuilder`: each pending gem in the slot
        let calc = MiscCalculator::new(&self.build, &calc_data)?;
        let base = compare::base_gem_dps(&calc, opts.field);
        let full_dps = opts.field == "FullDPS" || self.view_mode == "TREE";
        let pending: Vec<&Arc<Gem>> = gems
            .iter()
            .copied()
            .filter(|gem| {
                can_support.contains(&gem.id)
                    || (gem.granted_effect.has_global_effect && !gem.granted_effect.support)
            })
            .collect();
        let default_level = skills_tab.default_gem_level;
        let quality = skills_tab.default_gem_quality;
        let cands: Vec<GemCandidate> = pending
            .iter()
            .map(|gem| {
                let mut b = self.build.clone();
                let level = process_gem_level(gem, false, default_level, char_level);
                let list = &mut b
                    .skills
                    .as_mut()
                    .and_then(|s| s.socket_group_list_mut())
                    .expect("socket groups")[g]
                    .gem_list;
                match list.get_mut(k) {
                    // The gem in the slot keeps all but its data and level
                    Some(old) => {
                        old.gem = Some((*gem).clone());
                        old.level = Some(level);
                    }
                    None => list.push(GemInstance {
                        level: Some(level),
                        quality: Some(quality),
                        count: Some(1.0),
                        enabled: true,
                        enable_global1: true,
                        enable_global2: true,
                        gem_id: Some(gem.id.clone()),
                        name_spec: gem.name.clone(),
                        skill_id: Some(gem.granted_effect.id.clone()),
                        gem: Some((*gem).clone()),
                        ..GemInstance::default()
                    }),
                }
                GemCandidate { build: b }
            })
            .collect();
        let powers = compare::gem_power(&calc, cands, opts.field, full_dps, Threads::available());
        let mut dps: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
        for (gem, p) in pending.iter().zip(powers) {
            dps.insert(gem.id.as_str(), p?);
        }

        // `BuildList("")` with `FilterSupport`, sorted by `SortGemList`
        let mut list: Vec<&Arc<Gem>> = gems
            .into_iter()
            .filter(|gem| {
                let e = &gem.granted_effect;
                if e.legacy && !opts.show_legacy {
                    return false;
                }
                let legacy_awakened = e.legacy && e.plus_version_of.is_some();
                let exceptional = gem
                    .tag_string
                    .as_deref()
                    .is_some_and(|t| t.contains("Exceptional"));
                !e.support
                    || opts.show == "ALL"
                    || (opts.show == "NORMAL" && !(legacy_awakened || exceptional))
                    || (opts.show == "EXCEPTIONAL" && (legacy_awakened || exceptional))
            })
            .collect();
        let dps_of = |gem: &Gem| dps.get(gem.id.as_str()).copied().unwrap_or(base);
        engine::calcs::lua_sort(&mut list, |a, b| {
            let (sa, sb) = (can_support.contains(&a.id), can_support.contains(&b.id));
            if sa == sb {
                if opts.sort_by_dps && dps_of(a) != dps_of(b) {
                    dps_of(a) > dps_of(b)
                } else {
                    a.name < b.name
                }
            } else {
                sa
            }
        });
        let rows: Vec<J> = list
            .iter()
            .map(|gem| {
                let mut o = Map::new();
                o.insert("id".into(), json!(gem.id));
                o.insert("name".into(), json!(gem.name));
                o.insert("dps".into(), num(dps_of(gem)));
                o.insert("canSupport".into(), J::Bool(can_support.contains(&gem.id)));
                J::Object(o)
            })
            .collect();
        Ok(json!({ "baseDps": num(base), "gems": rows }))
    }
}
