//! Skill and gem data as typed data: upstream's granted effects
//! (`data.skills`, `Data/Skills/*.lua`) and skill gems (`data.gems`,
//! `Data/Gems.lua`), as `Modules/Data.lua` processes them. Read from
//! upstream's processed `data` table for now. What the calculations need
//! of a granted effect beyond its levels follows with them.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::item::Fields;
use crate::mods::convert::{Converter, array, get, number, text};
use crate::mods::{Mod, ModValue};
use lua::{Key, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

/// A level of a granted effect: its stat values, in the order of the
/// effect's stats, and its named fields
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillLevel {
    pub values: Vec<f64>,
    pub level_requirement: f64,
    pub stat_interpolation: Vec<f64>,
    /// Resource to amount (`Mana`, `Life`, `Soul`, ...)
    pub cost: Vec<(String, f64)>,
    /// `damageEffectiveness`, `critChance`, `cooldown`, ... by name
    pub fields: Vec<(String, f64)>,
}

impl SkillLevel {
    pub fn field(&self, name: &str) -> Option<f64> {
        self.fields.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }
}

/// A granted effect: an active skill or a support.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrantedEffect {
    pub id: String,
    pub name: String,
    pub base_type_name: Option<String>,
    /// 1 red, 2 green, 3 blue, other white
    pub color: Option<f64>,
    pub support: bool,
    pub plus_version_of: Option<String>,
    pub unsupported: bool,
    /// By level, from 1
    pub levels: BTreeMap<i64, SkillLevel>,
    /// `skillTypes`: the types the skill has
    pub skill_types: HashSet<i64>,
    pub minion_skill_types: Option<HashSet<i64>>,
    /// The truthy `baseFlags`
    pub base_flags: Vec<String>,
    /// For supports: the types they add, and the type expressions of the
    /// skills they support and do not
    pub add_skill_types: Vec<i64>,
    pub require_skill_types: Vec<i64>,
    pub exclude_skill_types: Vec<i64>,
    pub weapon_types: Option<Vec<String>>,
    pub add_flags: Option<Vec<String>>,
    pub is_trigger: bool,
    pub cannot_be_supported: bool,
    pub support_gems_only: bool,
    pub has_global_effect: bool,
    pub ignore_minion_types: bool,
    pub from_item: bool,
    /// The source of an effect an item or a node grants (`modSource`)
    pub mod_source: Option<String>,
    /// The stats of the levels' values, in order
    pub stats: Vec<String>,
    /// `constantStats`: stat and value, which may be nil
    pub constant_stats: Vec<(String, Option<f64>)>,
    /// `qualityStats`: stat and value per quality
    pub quality_stats: Vec<(String, f64)>,
    pub base_mods: Vec<Mod>,
    /// The effect's own stat map (`statMap`); the rest come from the
    /// shared one ([`SkillData::stat_map`])
    pub stat_map: HashMap<String, StatMap>,
    pub not_minion_stat: Vec<String>,
    /// `parts`: each part's fields (`name`, flags, `stages`, ...)
    pub parts: Option<Vec<Fields>>,
    pub skill_totem_id: Option<f64>,
    /// `minionList`: empty for spectres, which come from the build
    pub minion_list: Option<Vec<String>>,
    pub add_minion_list: Option<Vec<String>>,
    pub minion_has_item_set: bool,
    /// The keys of `minionUses`
    pub minion_uses: Option<Vec<String>>,
    pub base_effectiveness: Option<f64>,
    pub incremental_effectiveness: Option<f64>,
}

/// How a stat's value scales the mods it maps to: `value`, or `stat *
/// mult / div + base`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatScale {
    pub value: Option<f64>,
    pub mult: Option<f64>,
    pub div: Option<f64>,
    pub base: Option<f64>,
}

impl StatScale {
    pub fn apply(&self, stat: f64) -> f64 {
        self.value.unwrap_or_else(|| {
            stat * self.mult.unwrap_or(1.0) / self.div.unwrap_or(1.0) + self.base.unwrap_or(0.0)
        })
    }
}

/// A mod a stat maps to, or a group of mods with its own scale
#[derive(Clone, Debug, PartialEq)]
pub enum StatMapEntry {
    Mod(Mod),
    Group(Vec<Mod>, StatScale),
}

/// The mods a skill stat maps to (`statMap[stat]`)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatMap {
    pub entries: Vec<StatMapEntry>,
    pub scale: StatScale,
    /// The skill flag a non-zero value sets
    pub skill_flag: Option<String>,
    /// Why upstream's map is unusable: a few pass a tag where the mod's
    /// flags go, which fails when queried
    pub error: Option<String>,
}

fn scale_from_lua(v: &Value) -> Result<StatScale> {
    Ok(StatScale {
        value: opt_number(&get(v, "value"))?,
        mult: opt_number(&get(v, "mult"))?,
        div: opt_number(&get(v, "div"))?,
        base: opt_number(&get(v, "base"))?,
    })
}

fn stat_map_from_lua(v: &Value) -> Result<StatMap> {
    let conv = Converter {
        templates: true,
        ..Converter::default()
    };
    let mut entries = Vec::new();
    let mut error = None;
    for e in array(v) {
        let entry = if get(&e, "name").is_nil() {
            array(&e)
                .iter()
                .map(|m| conv.mod_(m))
                .collect::<Result<_>>()
                .and_then(|mods| Ok(StatMapEntry::Group(mods, scale_from_lua(&e)?)))
        } else {
            conv.mod_(&e).map(StatMapEntry::Mod)
        };
        match entry {
            Ok(x) => entries.push(x),
            Err(e) => error = Some(e),
        }
    }
    Ok(StatMap {
        entries,
        scale: scale_from_lua(v)?,
        skill_flag: opt_string(&get(v, "skillFlag"))?,
        error,
    })
}

fn strings(v: &Value) -> Result<Vec<String>> {
    array(v).iter().map(string).collect()
}

fn opt_strings(v: &Value) -> Result<Option<Vec<String>>> {
    if v.is_nil() {
        Ok(None)
    } else {
        strings(v).map(Some)
    }
}

impl GrantedEffect {
    pub fn level(&self, level: f64) -> Option<&SkillLevel> {
        (level.fract() == 0.0)
            .then(|| self.levels.get(&(level as i64)))
            .flatten()
    }

    /// `#levels`: the levels from 1 without a gap
    pub fn level_count(&self) -> i64 {
        let mut n = 0;
        while self.levels.contains_key(&(n + 1)) {
            n += 1;
        }
        n
    }
}

/// A skill gem.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gem {
    pub id: String,
    pub name: String,
    pub base_type_name: Option<String>,
    pub game_id: String,
    pub variant_id: String,
    pub granted_effect: Rc<GrantedEffect>,
    pub secondary_granted_effect: Option<Rc<GrantedEffect>>,
    pub natural_max_level: f64,
    pub req_str: f64,
    pub req_dex: f64,
    pub req_int: f64,
    pub tags: HashSet<String>,
    pub tag_string: Option<String>,
    pub vaal_gem: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SkillData {
    pub skills: HashMap<String, Rc<GrantedEffect>>,
    /// In data order
    gems: Vec<Rc<Gem>>,
    gem_index: HashMap<String, usize>,
    /// `data.gemForSkill`, by granted effect id
    gem_for_skill: HashMap<String, String>,
    /// `data.gemsByGameId`: game id to its variants (variant id, gem id),
    /// in data order
    gems_by_game_id: HashMap<String, Vec<(String, String)>>,
    /// `data.gemForBaseName`: lower case base name to gem id
    gem_for_base_name: HashMap<String, String>,
    /// `colorCodes.STRENGTH`, `.DEXTERITY`, `.INTELLIGENCE` and `.NORMAL`
    pub color_codes: [String; 4],
    /// `SkillType`: the skill types by name
    pub skill_type: HashMap<String, i64>,
    /// `data.gemVaalGemIdForBaseGemId`
    gem_vaal_gem_id_for_base_gem_id: HashMap<String, String>,
    /// `data.skillStatMap`: the stat maps shared by the skills
    pub skill_stat_map: HashMap<String, StatMap>,
}

fn rt_index(v: &Value, i: i64) -> Value {
    v.as_table()
        .map(|t| t.get(&Value::Number(i as f64)))
        .unwrap_or_default()
}

fn pairs(v: &Value) -> Vec<(Key, Value)> {
    v.as_table().map(|t| t.entries()).unwrap_or_default()
}

fn string(v: &Value) -> Result<String> {
    text(v).map(|s| s.to_string())
}

fn opt_string(v: &Value) -> Result<Option<String>> {
    if v.is_nil() {
        Ok(None)
    } else {
        string(v).map(Some)
    }
}

fn opt_number(v: &Value) -> Result<Option<f64>> {
    if v.is_nil() {
        Ok(None)
    } else {
        number(v).map(Some)
    }
}

fn key(k: &Key) -> Result<String> {
    match k {
        Key::Str(s) => Ok(s.to_str_lossy().into_owned()),
        k => Err(format!("expected a string key, got {k:?}")),
    }
}

/// The keys of a set of skill types (`{ [SkillType.Attack] = true }`)
fn int_set(v: &Value) -> Result<HashSet<i64>> {
    pairs(v)
        .into_iter()
        .filter(|(_, x)| x.truthy())
        .map(|(k, _)| match k {
            Key::Int(i) => Ok(i),
            k => Err(format!("skill type {k:?}")),
        })
        .collect()
}

fn int_array(v: &Value) -> Result<Vec<i64>> {
    array(v).iter().map(|x| Ok(number(x)? as i64)).collect()
}

/// The keys of the truthy entries of a set
fn truthy_keys(v: &Value) -> Result<Vec<String>> {
    let mut keys: Vec<String> = pairs(v)
        .iter()
        .filter(|(_, x)| x.truthy())
        .map(|(k, _)| key(k))
        .collect::<Result<_>>()?;
    keys.sort();
    Ok(keys)
}

fn level_from_lua(v: &Value) -> Result<SkillLevel> {
    let mut level = SkillLevel::default();
    for (k, x) in pairs(v) {
        match k {
            Key::Int(_) => {}
            Key::Str(name) => {
                let name = name.to_str_lossy().into_owned();
                match name.as_str() {
                    "levelRequirement" => level.level_requirement = number(&x)?,
                    "statInterpolation" => {
                        level.stat_interpolation =
                            array(&x).iter().map(number).collect::<Result<_>>()?
                    }
                    "cost" => {
                        level.cost = pairs(&x)
                            .iter()
                            .map(|(k, v)| Ok((key(k)?, number(v)?)))
                            .collect::<Result<_>>()?
                    }
                    _ => level
                        .fields
                        .push((name, number(&x).map_err(|e| format!("level field: {e}"))?)),
                }
            }
            k => return Err(format!("level key {k:?}")),
        }
    }
    level.values = array(v).iter().map(number).collect::<Result<_>>()?;
    Ok(level)
}

impl GrantedEffect {
    fn from_lua(id: &str, v: &Value) -> Result<GrantedEffect> {
        let mut levels = BTreeMap::new();
        for (k, l) in pairs(&get(v, "levels")) {
            let Key::Int(n) = k else {
                return Err(format!("levels key {k:?}"));
            };
            levels.insert(
                n,
                level_from_lua(&l).map_err(|e| format!("level {n}: {e}"))?,
            );
        }
        Ok(GrantedEffect {
            id: id.to_string(),
            name: string(&get(v, "name"))?,
            base_type_name: opt_string(&get(v, "baseTypeName"))?,
            color: opt_number(&get(v, "color"))?,
            support: get(v, "support").truthy(),
            plus_version_of: opt_string(&get(v, "plusVersionOf"))?,
            unsupported: get(v, "unsupported").truthy(),
            levels,
            skill_types: int_set(&get(v, "skillTypes"))?,
            minion_skill_types: match get(v, "minionSkillTypes") {
                Value::Nil => None,
                t => Some(int_set(&t)?),
            },
            base_flags: truthy_keys(&get(v, "baseFlags"))?,
            add_skill_types: pairs(&get(v, "addSkillTypes"))
                .iter()
                .map(|(_, x)| Ok(number(x)? as i64))
                .collect::<Result<_>>()?,
            require_skill_types: int_array(&get(v, "requireSkillTypes"))?,
            exclude_skill_types: int_array(&get(v, "excludeSkillTypes"))?,
            weapon_types: match get(v, "weaponTypes") {
                Value::Nil => None,
                t => Some(truthy_keys(&t)?),
            },
            add_flags: match get(v, "addFlags") {
                Value::Nil => None,
                t => Some(truthy_keys(&t)?),
            },
            is_trigger: get(v, "isTrigger").truthy(),
            cannot_be_supported: get(v, "cannotBeSupported").truthy(),
            support_gems_only: get(v, "supportGemsOnly").truthy(),
            has_global_effect: get(v, "hasGlobalEffect").truthy(),
            ignore_minion_types: get(v, "ignoreMinionTypes").truthy(),
            from_item: get(v, "fromItem").truthy(),
            mod_source: opt_string(&get(v, "modSource"))?,
            stats: strings(&get(v, "stats"))?,
            constant_stats: array(&get(v, "constantStats"))
                .iter()
                .map(|x| Ok((string(&rt_index(x, 1))?, opt_number(&rt_index(x, 2))?)))
                .collect::<Result<_>>()?,
            quality_stats: array(&get(v, "qualityStats"))
                .iter()
                .map(|x| Ok((string(&rt_index(x, 1))?, number(&rt_index(x, 2))?)))
                .collect::<Result<_>>()?,
            base_mods: {
                let conv = Converter {
                    templates: true,
                    ..Converter::default()
                };
                array(&get(v, "baseMods"))
                    .iter()
                    .map(|m| conv.mod_(m))
                    .collect::<Result<_>>()
                    .map_err(|e| format!("baseMods: {e}"))?
            },
            stat_map: pairs(&get(v, "statMap"))
                .into_iter()
                .filter_map(|(k, m)| match k {
                    Key::Str(k) if &*k.to_str_lossy() != "_grantedEffect" => {
                        Some((k.to_str_lossy().into_owned(), m))
                    }
                    _ => None,
                })
                .map(|(k, m)| {
                    Ok((
                        k.clone(),
                        stat_map_from_lua(&m).map_err(|e| format!("statMap {k}: {e}"))?,
                    ))
                })
                .collect::<Result<_>>()?,
            not_minion_stat: match get(v, "notMinionStat") {
                Value::Nil => vec![],
                x => strings(&x)?,
            },
            parts: match get(v, "parts") {
                Value::Nil => None,
                x => {
                    let conv = Converter::default();
                    Some(
                        array(&x)
                            .iter()
                            .map(|p| {
                                pairs(p)
                                    .iter()
                                    .map(|(k, f)| Ok((key(k)?, conv.value(f)?)))
                                    .collect::<Result<Fields>>()
                            })
                            .collect::<Result<_>>()?,
                    )
                }
            },
            skill_totem_id: opt_number(&get(v, "skillTotemId"))?,
            minion_list: opt_strings(&get(v, "minionList"))?,
            add_minion_list: opt_strings(&get(v, "addMinionList"))?,
            minion_has_item_set: get(v, "minionHasItemSet").truthy(),
            minion_uses: match get(v, "minionUses") {
                Value::Nil => None,
                x => Some(truthy_keys(&x)?),
            },
            base_effectiveness: opt_number(&get(v, "baseEffectiveness"))?,
            incremental_effectiveness: opt_number(&get(v, "incrementalEffectiveness"))?,
        })
    }
}

impl SkillData {
    /// Reads the skills and gems from upstream's processed `data` table, its
    /// `colorCodes` and `SkillType`.
    pub fn from_lua(data: &Value, color_codes: &Value, skill_type: &Value) -> Result<SkillData> {
        let mut d = SkillData::default();
        for (k, v) in pairs(skill_type) {
            d.skill_type.insert(key(&k)?, number(&v)? as i64);
        }
        for (k, v) in pairs(&get(data, "skillStatMap")) {
            let name = key(&k)?;
            let map = stat_map_from_lua(&v).map_err(|e| format!("skillStatMap {name}: {e}"))?;
            d.skill_stat_map.insert(name, map);
        }
        for (i, c) in ["STRENGTH", "DEXTERITY", "INTELLIGENCE", "NORMAL"]
            .iter()
            .enumerate()
        {
            d.color_codes[i] = string(&get(color_codes, c))?;
        }
        for (k, v) in pairs(&get(data, "skills")) {
            let id = key(&k)?;
            let effect =
                GrantedEffect::from_lua(&id, &v).map_err(|e| format!("skill {id}: {e}"))?;
            d.skills.insert(id, Rc::new(effect));
        }
        let effect = |v: &Value| -> Result<Option<Rc<GrantedEffect>>> {
            if v.is_nil() {
                return Ok(None);
            }
            let id = string(&get(v, "id"))?;
            d.skills
                .get(&id)
                .cloned()
                .map(Some)
                .ok_or_else(|| format!("no skill {id}"))
        };
        let mut gems = Vec::new();
        for (k, g) in pairs(&get(data, "gems")) {
            let id = key(&k)?;
            let num = |f: &str| -> Result<f64> { number(&get(&g, f)) };
            let gem = Gem {
                id: id.clone(),
                name: string(&get(&g, "name"))?,
                base_type_name: opt_string(&get(&g, "baseTypeName"))?,
                game_id: string(&get(&g, "gameId"))?,
                variant_id: string(&get(&g, "variantId"))?,
                granted_effect: effect(&get(&g, "grantedEffect"))?
                    .ok_or_else(|| format!("gem {id} without a granted effect"))?,
                secondary_granted_effect: effect(&get(&g, "secondaryGrantedEffect"))?,
                natural_max_level: num("naturalMaxLevel")?,
                req_str: num("reqStr")?,
                req_dex: num("reqDex")?,
                req_int: num("reqInt")?,
                tags: pairs(&get(&g, "tags"))
                    .iter()
                    .filter(|(_, x)| x.truthy())
                    .map(|(k, _)| key(k))
                    .collect::<Result<_>>()?,
                tag_string: opt_string(&get(&g, "tagString"))?,
                vaal_gem: get(&g, "vaalGem").truthy(),
            };
            gems.push(gem);
        }
        for gem in gems {
            d.gem_index.insert(gem.id.clone(), d.gems.len());
            d.gems.push(Rc::new(gem));
        }
        // `gemForSkill` is keyed by the granted effect tables
        for (k, gem_id) in pairs(&get(data, "gemForSkill")) {
            let effect = k.to_value();
            let skill_id = string(&get(&effect, "id"))?;
            d.gem_for_skill.insert(skill_id, string(&gem_id)?);
        }
        for (k, variants) in pairs(&get(data, "gemsByGameId")) {
            let mut by_variant = Vec::new();
            for (variant, gem) in pairs(&variants) {
                by_variant.push((key(&variant)?, string(&get(&gem, "id"))?));
            }
            d.gems_by_game_id.insert(key(&k)?, by_variant);
        }
        for (k, gem_id) in pairs(&get(data, "gemVaalGemIdForBaseGemId")) {
            d.gem_vaal_gem_id_for_base_gem_id
                .insert(key(&k)?, string(&gem_id)?);
        }
        for (k, gem_id) in pairs(&get(data, "gemForBaseName")) {
            d.gem_for_base_name.insert(key(&k)?, string(&gem_id)?);
        }
        Ok(d)
    }

    /// `grantedEffect.statMap[stat]`: the effect's own map, else the shared
    /// one with the effect's source on its mods (`processMod` of
    /// `Modules/Data.lua`, which reaches only the mods outside groups)
    pub fn stat_map(
        &self,
        effect: &GrantedEffect,
        stat: &str,
    ) -> Option<std::borrow::Cow<'_, StatMap>> {
        if let Some(m) = effect.stat_map.get(stat) {
            return Some(std::borrow::Cow::Owned(m.clone()));
        }
        let shared = self.skill_stat_map.get(stat)?;
        let source = effect
            .mod_source
            .clone()
            .unwrap_or_else(|| format!("Skill:{}", effect.id));
        let not_minion = effect.not_minion_stat.iter().any(|s| s == stat)
            && (effect.support
                || self
                    .skill_type("Buff")
                    .is_some_and(|b| effect.skill_types.contains(&b)));
        if not_minion
            && shared
                .entries
                .iter()
                .any(|e| matches!(e, StatMapEntry::Group(..)))
        {
            // Upstream adds the tag to the group as if it were a mod
            return None;
        }
        let mut map = shared.clone();
        for e in &mut map.entries {
            if let StatMapEntry::Mod(m) = e {
                m.source = Some(source.as_str().into());
                if let ModValue::Record(r) = &mut m.value
                    && let Some(ModValue::Mod(inner)) = r.get_mut(crate::mods::Sym::new("mod"))
                {
                    inner.source = Some(format!("Skill:{}", effect.id).into());
                }
                if not_minion {
                    m.tags.push(crate::mods::Tag {
                        kind: crate::mods::tag::TagKind::ActorCondition {
                            actor: Some(crate::mods::Sym::new("parent")),
                            vars: None,
                            neg: true,
                        },
                        global_limit: None,
                    });
                }
            }
        }
        Some(std::borrow::Cow::Owned(map))
    }

    /// `calcLib.getGameIdFromGemName(name, dropVaal)`
    pub fn game_id_from_gem_name(&self, name: &str, drop_vaal: bool) -> Option<&str> {
        let gem = self.gem_for_base_name(&crate::pat::lower(name))?;
        if drop_vaal && gem.vaal_gem {
            let base = self.gem_vaal_gem_id_for_base_gem_id.get(&gem.id)?;
            return self.gem(base).map(|g| g.game_id.as_str());
        }
        Some(&gem.game_id)
    }

    /// `calcLib.isGemIdSame(gemName, typeName, dropVaal)`
    pub fn is_gem_id_same(&self, gem_name: &str, type_name: &str, drop_vaal: bool) -> bool {
        match (
            self.game_id_from_gem_name(gem_name, drop_vaal),
            self.game_id_from_gem_name(type_name, drop_vaal),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// `calcLib.gemIsType(gem, type, includeTransfigured)`
    pub fn gem_is_type(&self, gem: &Gem, typ: &str, include_transfigured: bool) -> bool {
        let tag = |t: &str| gem.tags.contains(t);
        let name = crate::pat::lower(&gem.name);
        let active = matches!(typ, "active skill" | "grants_active_skill" | "skill");
        typ == "all"
            || (typ == "elemental" && (tag("fire") || tag("cold") || tag("lightning")))
            || (typ == "aoe" && tag("area"))
            || (typ == "trap or mine" && (tag("trap") || tag("mine")))
            || (active && tag("grants_active_skill") && !tag("support"))
            || (typ == "non-vaal" && !tag("vaal"))
            || (typ == "non-exceptional" && !tag("exceptional"))
            || typ == name
            || typ == crate::pat::gsub(&name, "^vaal ", "")
            || (include_transfigured && self.is_gem_id_same(&gem.name, typ, true))
            || (!active && tag(typ))
    }

    /// The number of a skill type (`SkillType[name]`)
    pub fn skill_type(&self, name: &str) -> Option<i64> {
        self.skill_type.get(name).copied()
    }

    pub fn gem(&self, id: &str) -> Option<&Rc<Gem>> {
        self.gem_index.get(id).map(|&i| &self.gems[i])
    }

    /// The gems, in data order
    pub fn gems(&self) -> &[Rc<Gem>] {
        &self.gems
    }

    /// The gem that grants a skill (`data.gemForSkill[grantedEffect]`)
    pub fn gem_for_skill(&self, skill_id: &str) -> Option<&Rc<Gem>> {
        self.gem_for_skill.get(skill_id).and_then(|id| self.gem(id))
    }

    /// The gem of a game id and variant (`data.gemsByGameId[gameId][variantId]`)
    pub fn gem_by_game_id(&self, game_id: &str, variant_id: &str) -> Option<&Rc<Gem>> {
        self.gems_by_game_id
            .get(game_id)?
            .iter()
            .find(|(v, _)| v == variant_id)
            .and_then(|(_, id)| self.gem(id))
    }

    /// The variants of a game id, in data order
    pub fn variants_of(&self, game_id: &str) -> Option<impl Iterator<Item = &Rc<Gem>>> {
        self.gems_by_game_id
            .get(game_id)
            .map(|v| v.iter().filter_map(|(_, id)| self.gem(id)))
    }

    /// The gem of a lower case base name (`data.gemForBaseName[name]`)
    pub fn gem_for_base_name(&self, name: &str) -> Option<&Rc<Gem>> {
        self.gem_for_base_name.get(name).and_then(|id| self.gem(id))
    }
}
