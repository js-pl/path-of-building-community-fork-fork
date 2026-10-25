//! Skill and gem data as typed data: upstream's granted effects
//! (`data.skills`, `Data/Skills/*.lua`) and skill gems (`data.gems`,
//! `Data/Gems.lua`), as `Modules/Data.lua` processes them. Read from
//! upstream's processed `data` table for now. What the calculations need
//! of a granted effect beyond its levels follows with them.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::mods::convert::{array, get, number, text};
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
        })
    }
}

impl SkillData {
    /// Reads the skills and gems from upstream's processed `data` table and
    /// its `colorCodes`.
    pub fn from_lua(data: &Value, color_codes: &Value) -> Result<SkillData> {
        let mut d = SkillData::default();
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
        for (k, gem_id) in pairs(&get(data, "gemForBaseName")) {
            d.gem_for_base_name.insert(key(&k)?, string(&gem_id)?);
        }
        Ok(d)
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
