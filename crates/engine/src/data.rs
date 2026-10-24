//! Port of upstream `src/Modules/Data.lua` (v2.67.2): processing of the raw
//! data tables the importer reads. Only the parts ported so far are here.

use crate::common::sanitise_text;
use crate::rt::*;
use lua::{TableRef, Value};

/// Game data, as upstream's global `data` table, plus the globals from
/// `Data/Global.lua`.
pub struct Data {
    pub globals: TableRef,
    pub data: TableRef,
}

impl Data {
    /// Processes raw data as `Data.lua` does. Takes ownership of the tables,
    /// which are modified in place.
    pub fn new(globals: TableRef, data: TableRef) -> Data {
        let d = Data { globals, data };
        d.process_skills();
        d.process_gems();
        d
    }

    pub fn get(&self, key: &str) -> Value {
        self.data.get_str(key)
    }

    pub fn global(&self, name: &str) -> Value {
        self.globals.get_str(name)
    }

    /// `SkillType.<name>`
    pub fn skill_type(&self, name: &str) -> Value {
        field(&self.global("SkillType"), name)
    }

    /// `processMod` from Data.lua.
    fn process_mod(&self, granted_effect: &Value, m: &Value, stat_name: &Value) {
        set_index(m, s("source"), field(granted_effect, "modSource"));
        let value = field(m, "value");
        if let Value::Table(_) = value
            && field(&value, "mod").truthy()
        {
            set_index(
                &field(&value, "mod"),
                s("source"),
                concat(&s("Skill:"), &field(granted_effect, "id")),
            );
        }
        for (_, tag) in ipairs(m) {
            if field(&tag, "type").raw_eq(&s("GlobalEffect")) {
                set_index(granted_effect, s("hasGlobalEffect"), Value::Bool(true));
                break;
            }
        }
        let mut not_minion_stat = false;
        let skill_types = field(granted_effect, "skillTypes");
        if field(granted_effect, "notMinionStat").truthy()
            && stat_name.truthy()
            && (field(granted_effect, "support").truthy()
                || skill_types.truthy() && index(&skill_types, &self.skill_type("Buff")).truthy())
        {
            for (_, name) in ipairs(&field(granted_effect, "notMinionStat")) {
                if name.raw_eq(stat_name) {
                    not_minion_stat = true;
                }
            }
        }
        if not_minion_stat {
            t_insert(
                m,
                tbl(
                    vec![
                        (s("type"), s("ActorCondition")),
                        (s("actor"), s("parent")),
                        (s("neg"), Value::Bool(true)),
                    ],
                    vec![],
                ),
            );
        }
    }

    fn process_skills(&self) {
        let skills = self.get("skills");
        for (skill_id, granted_effect) in pairs(&skills) {
            set_index(
                &granted_effect,
                s("name"),
                sanitise_text(&field(&granted_effect, "name")),
            );
            set_index(&granted_effect, s("id"), skill_id.clone());
            set_index(
                &granted_effect,
                s("modSource"),
                concat(&s("Skill:"), &skill_id),
            );
            for key in ["baseMods", "qualityMods", "levelMods"] {
                let list = field(&granted_effect, key);
                if list.is_nil() {
                    continue;
                }
                for (_, m) in pairs(&list) {
                    if field(&m, "name").truthy() {
                        self.process_mod(&granted_effect, &m, &nil());
                    } else {
                        for (_, m) in ipairs(&m) {
                            self.process_mod(&granted_effect, &m, &nil());
                        }
                    }
                }
            }
            // The stat map metatable that copies data.skillStatMap entries on
            // first use is ported with the calcs; the skill's own entries are
            // processed here
            if field(&granted_effect, "statMap").is_nil() {
                set_index(&granted_effect, s("statMap"), tbl(vec![], vec![]));
            }
            let stat_map = field(&granted_effect, "statMap");
            for (name, map) in pairs(&stat_map) {
                for (_, mod_or_group) in ipairs(&map) {
                    if field(&mod_or_group, "name").truthy() {
                        self.process_mod(&granted_effect, &mod_or_group, &name);
                    } else {
                        for (_, m) in ipairs(&mod_or_group) {
                            self.process_mod(&granted_effect, &m, &name);
                        }
                    }
                }
            }
        }
    }

    /// `setupGem` from Data.lua.
    fn setup_gem(&self, gem: &Value, gem_id: &Value) {
        let skills = self.get("skills");
        set_index(gem, s("id"), gem_id.clone());
        let granted_effect = index(&skills, &field(gem, "grantedEffectId"));
        set_index(gem, s("grantedEffect"), granted_effect.clone());
        set_index(
            &self.get("gemForSkill"),
            granted_effect.clone(),
            gem_id.clone(),
        );
        let by_game_id = self.get("gemsByGameId");
        let game_id = field(gem, "gameId");
        if index(&by_game_id, &game_id).is_nil() {
            set_index(&by_game_id, game_id.clone(), tbl(vec![], vec![]));
        }
        set_index(
            &index(&by_game_id, &game_id),
            field(gem, "variantId"),
            gem.clone(),
        );
        let mut base_name = field(gem, "name");
        if field(&granted_effect, "support").truthy()
            && !field(gem, "grantedEffectId").raw_eq(&s("SupportBarrage"))
        {
            base_name = concat(&base_name, &s(" Support"));
        }
        let for_base_name = self.get("gemForBaseName");
        set_index(&for_base_name, lower(&base_name), gem_id.clone());
        let base_type_name = field(gem, "baseTypeName");
        if base_type_name.truthy() && !base_type_name.raw_eq(&base_name) {
            set_index(&for_base_name, lower(&base_type_name), gem_id.clone());
        }
        let secondary = {
            let id = field(gem, "secondaryGrantedEffectId");
            if id.truthy() { index(&skills, &id) } else { id }
        };
        set_index(gem, s("secondaryGrantedEffect"), secondary.clone());
        set_index(
            gem,
            s("grantedEffectList"),
            tbl(vec![], vec![granted_effect.clone(), secondary]),
        );
        let natural = field(gem, "naturalMaxLevel");
        let natural = if natural.truthy() {
            natural
        } else {
            let levels = field(&granted_effect, "levels");
            let count = len(&levels).as_number().unwrap_or(0.0);
            if count > 20.0 {
                n(count - 20.0)
            } else if index(&index(&levels, &n(3.0)), &n(1.0)).truthy() {
                n(3.0)
            } else {
                n(1.0)
            }
        };
        set_index(gem, s("naturalMaxLevel"), natural);
    }

    fn process_gems(&self) {
        let skills = self.get("skills");
        for key in [
            "gemForSkill",
            "gemForBaseName",
            "gemsByGameId",
            "gemGrantedEffectIdForVaalGemId",
            "gemVaalGemIdForBaseGemId",
        ] {
            self.data.set_str(key, tbl(vec![], vec![]));
        }
        let gems = self.get("gems");
        let to_add = TableRef::new();
        for (gem_id, gem) in pairs(&gems) {
            set_index(&gem, s("name"), sanitise_text(&field(&gem, "name")));
            self.setup_gem(&gem, &gem_id);
            let vaal = field(&gem, "vaalGem").truthy();
            let secondary_id = field(&gem, "secondaryGrantedEffectId");
            if vaal {
                set_index(
                    &self.get("gemGrantedEffectIdForVaalGemId"),
                    secondary_id.clone(),
                    gem_id.clone(),
                );
                for (other_id, other) in pairs(&gems) {
                    if field(&other, "grantedEffectId").raw_eq(&secondary_id) {
                        set_index(
                            &self.get("gemVaalGemIdForBaseGemId"),
                            gem_id.clone(),
                            other_id,
                        );
                        break;
                    }
                }
            }
            for alt in ["AltX", "AltY"] {
                if !vaal {
                    continue;
                }
                let alt_skill_id = concat(&secondary_id, &s(alt));
                let alt_skill = index(&skills, &alt_skill_id);
                if alt_skill.is_nil() {
                    continue;
                }
                let alt_gem_id = concat(&gem_id, &s(alt));
                set_index(
                    &self.get("gemGrantedEffectIdForVaalGemId"),
                    alt_skill_id.clone(),
                    alt_gem_id.clone(),
                );
                let base = index(&self.get("gemVaalGemIdForBaseGemId"), &gem_id);
                set_index(
                    &self.get("gemVaalGemIdForBaseGemId"),
                    alt_gem_id.clone(),
                    concat(&base, &s(alt)),
                );
                let new_gem = tbl(vec![], vec![]);
                set_index(&new_gem, s("tags"), tbl(vec![], vec![]));
                set_index(
                    &new_gem,
                    s("name"),
                    concat(&s("Vaal "), &field(&alt_skill, "baseTypeName")),
                );
                for key in [
                    "gameId",
                    "grantedEffectId",
                    "vaalGem",
                    "tagString",
                    "reqStr",
                    "reqDex",
                    "reqInt",
                    "naturalMaxLevel",
                ] {
                    set_index(&new_gem, s(key), field(&gem, key));
                }
                set_index(
                    &new_gem,
                    s("variantId"),
                    concat(&field(&gem, "variantId"), &s(alt)),
                );
                set_index(&new_gem, s("secondaryGrantedEffectId"), alt_skill_id);
                set_index(&new_gem, s("tags"), copy_table(&field(&gem, "tags")));
                self.setup_gem(&new_gem, &alt_gem_id);
                to_add.set(alt_gem_id, new_gem);
            }
        }
        for (id, gem) in to_add.entries() {
            set_index(&gems, id.to_value(), gem);
        }
    }
}
