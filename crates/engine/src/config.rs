//! The configuration: upstream's options of the Configuration tab
//! (`Modules/ConfigOptions.lua`, with the map mods of `Data/ModMap.lua`)
//! and `ConfigTabClass:BuildModList`, which turns the options set into
//! mods for the player and the enemy.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::mod_parser::ModParser;
use crate::mods::convert::{Converter, array, get, number};
use crate::mods::{Mod, ModType, ModValue, Sym, Tag, record};
use crate::pat;
use lua::{Key, Value};
use std::collections::HashMap;
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

/// An option's value: a check box's, a number or a list or text entry
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Bool(bool),
    Number(f64),
    Str(Rc<str>),
}

impl Val {
    /// The value in arithmetic (Lua coerces numeric strings)
    pub fn num(&self) -> f64 {
        match self {
            Val::Number(n) => *n,
            Val::Str(s) => pat::to_number(s).unwrap_or(f64::NAN),
            Val::Bool(_) => f64::NAN,
        }
    }

    /// The value in a concatenation
    pub fn text(&self) -> String {
        match self {
            Val::Number(n) => pat::num(*n),
            Val::Str(s) => s.to_string(),
            Val::Bool(b) => b.to_string(),
        }
    }

    /// `val == s` for a string `s`
    pub fn is_str(&self, s: &str) -> bool {
        matches!(self, Val::Str(v) if &**v == s)
    }

    /// `val == n` for a number `n`
    pub fn is_num(&self, n: f64) -> bool {
        matches!(self, Val::Number(v) if *v == n)
    }

    pub fn value(&self) -> ModValue {
        match self {
            Val::Bool(b) => ModValue::Bool(*b),
            Val::Number(n) => ModValue::Number(*n),
            Val::Str(s) => ModValue::Str(s.clone()),
        }
    }

    pub fn truthy(&self) -> bool {
        !matches!(self, Val::Bool(false))
    }
}

/// The values of options by name: the input or the placeholders
pub type Inputs = HashMap<String, Val>;

/// How an option is set, which decides when it applies
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Kind {
    Check,
    Count,
    Integer,
    CountAllowZero,
    Float,
    List,
    Text,
}

/// What an option's apply function gets besides its value
pub struct Out<'a> {
    pub mods: Vec<Mod>,
    pub enemy_mods: Vec<Mod>,
    pub input: &'a mut Inputs,
    pub placeholder: &'a mut Inputs,
    pub data: &'a ConfigData,
    pub state: &'a mut ConfigState,
    pub character_level: f64,
}

type Apply = fn(&Val, &mut Out);

pub struct ConfigOption {
    pub var: &'static str,
    pub kind: Kind,
    apply: Apply,
}

impl ConfigOption {
    /// Applies the option set to `val`
    pub fn apply(&self, val: &Val, out: &mut Out) {
        (self.apply)(val, out)
    }
}

fn opt(var: &'static str, kind: Kind, apply: Apply) -> ConfigOption {
    ConfigOption { var, kind, apply }
}

/// `math.min` and `math.max` as upstream's runtime computes them
fn lua_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

fn lua_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// `round(val)` of `Modules/Common.lua`
fn round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// A mod from the configuration
fn cfg(name: &str, typ: ModType, value: impl Into<ModValue>) -> Mod {
    Mod::new(name, typ, value, Some("Config"))
}

/// A flag set by the configuration
fn flag(name: &str) -> Mod {
    cfg(name, ModType::Flag, true)
}

/// A boss skill preset (`data.bossSkills[name]`)
#[derive(Clone, Debug, Default)]
pub struct BossSkill {
    pub damage_type: Option<String>,
    /// Damage type to its base and roll range multipliers
    pub damage_multipliers: Vec<(String, [f64; 2])>,
    pub uber_damage_multiplier: Option<f64>,
    /// Penetration type to its value; `None` for an empty one (`""`)
    pub damage_penetrations: Option<Vec<(String, Option<f64>)>>,
    pub uber_damage_penetrations: Option<HashMap<String, Option<f64>>>,
    pub speed: Option<f64>,
    pub uber_speed: Option<f64>,
    pub crit_chance: Option<f64>,
    pub earlier_uber: bool,
    /// Stat to value, `None` for a flag
    pub additional_base: Option<BossStats>,
    pub additional_uber: Option<BossStats>,
}

/// A boss skill's extra enemy stats: stat to value, `None` for a flag
type BossStats = Vec<(String, Option<ModValue>)>;

/// The game data the options need
#[derive(Clone, Debug, Default)]
pub struct ConfigData {
    pub skill_types: HashMap<String, f64>,
    /// By monster level, from 1
    pub monster_damage: Vec<f64>,
    pub monster_armour: Vec<f64>,
    pub monster_evasion: Vec<f64>,
    pub monster_constants: HashMap<String, f64>,
    pub misc: HashMap<String, f64>,
    pub boss_stats: HashMap<String, f64>,
    pub boss_skills: HashMap<String, BossSkill>,
}

fn pairs(v: &Value) -> Vec<(Key, Value)> {
    v.as_table().map(|t| t.entries()).unwrap_or_default()
}

fn key(k: &Key) -> Result<String> {
    match k {
        Key::Str(s) => Ok(s.to_str_lossy().into_owned()),
        k => Err(format!("expected a string key, got {k:?}")),
    }
}

fn numbers(v: &Value) -> Result<HashMap<String, f64>> {
    pairs(v)
        .iter()
        .filter(|(_, x)| matches!(x, Value::Number(_)))
        .map(|(k, x)| Ok((key(k)?, number(x)?)))
        .collect()
}

fn opt_num(v: &Value) -> Result<Option<f64>> {
    if v.is_nil() {
        Ok(None)
    } else {
        number(v).map(Some)
    }
}

impl ConfigData {
    /// Reads the data from upstream's `data` table and its `SkillType`.
    pub fn from_lua(data: &Value, skill_types: &Value) -> Result<ConfigData> {
        let table = |name: &str| -> Result<Vec<f64>> {
            array(&get(data, name)).iter().map(number).collect()
        };
        let mut boss_skills = HashMap::new();
        for (k, b) in pairs(&get(data, "bossSkills")) {
            let mut skill = BossSkill {
                damage_type: get(&b, "DamageType")
                    .as_str()
                    .map(|s| s.to_str_lossy().into_owned()),
                uber_damage_multiplier: opt_num(&get(&b, "UberDamageMultiplier"))?,
                speed: opt_num(&get(&b, "speed"))?,
                uber_speed: opt_num(&get(&b, "UberSpeed"))?,
                crit_chance: opt_num(&get(&b, "critChance"))?,
                earlier_uber: get(&b, "earlierUber").truthy(),
                ..BossSkill::default()
            };
            for (t, m) in pairs(&get(&b, "DamageMultipliers")) {
                let m = array(&m);
                let at = |i: usize| m.get(i).map_or(Ok(f64::NAN), number);
                skill.damage_multipliers.push((key(&t)?, [at(0)?, at(1)?]));
            }
            // `SetPlaceholder(pen, true)` stores `tonumber(tostring(pen))`
            let pen_list = |v: &Value| -> Result<Vec<(String, Option<f64>)>> {
                pairs(v)
                    .iter()
                    .map(|(k, x)| Ok((key(k)?, pat::to_number(&lua::to_string(x).to_str_lossy()))))
                    .collect()
            };
            let pens = get(&b, "DamagePenetrations");
            if !pens.is_nil() {
                skill.damage_penetrations = Some(pen_list(&pens)?);
            }
            let uber_pens = get(&b, "UberDamagePenetrations");
            if !uber_pens.is_nil() {
                skill.uber_damage_penetrations = Some(pen_list(&uber_pens)?.into_iter().collect());
            }
            let stats = |v: &Value| -> Result<Option<BossStats>> {
                if v.is_nil() {
                    return Ok(None);
                }
                pairs(v)
                    .iter()
                    .map(|(k, x)| {
                        let flag = lua::to_string(x).to_str_lossy() == "flag";
                        let value = if flag {
                            None
                        } else {
                            Some(Converter::default().value(x)?)
                        };
                        Ok((key(k)?, value))
                    })
                    .collect::<Result<Vec<_>>>()
                    .map(Some)
            };
            let extra = get(&b, "additionalStats");
            skill.additional_base = stats(&get(&extra, "base"))?;
            skill.additional_uber = stats(&get(&extra, "uber"))?;
            boss_skills.insert(key(&k)?, skill);
        }
        Ok(ConfigData {
            skill_types: numbers(skill_types)?,
            monster_damage: table("monsterDamageTable")?,
            monster_armour: table("monsterArmourTable")?,
            monster_evasion: table("monsterEvasionTable")?,
            monster_constants: numbers(&get(data, "monsterConstants"))?,
            misc: numbers(&get(data, "misc"))?,
            boss_stats: numbers(&get(data, "bossStats"))?,
            boss_skills,
        })
    }

    fn by_level(table: &[f64], level: f64) -> f64 {
        if level.fract() != 0.0 || level < 1.0 {
            return f64::NAN;
        }
        table.get(level as usize - 1).copied().unwrap_or(f64::NAN)
    }

    fn misc(&self, name: &str) -> f64 {
        self.misc.get(name).copied().unwrap_or(f64::NAN)
    }
}

/// What the Configuration tab keeps between builds of its mods: its enemy
/// level and whether a boss preset locked the damage type
#[derive(Clone, Debug, Default)]
pub struct ConfigState {
    pub enemy_level: Option<f64>,
    pub damage_type_locked: bool,
}

/// A block of custom modifier lines
#[derive(Clone, Debug, PartialEq)]
pub struct CustomBlock {
    pub title: Option<String>,
    pub enabled: bool,
    pub text: String,
}

impl Out<'_> {
    /// `SetPlaceholder(value, true)` of an option's edit control: the
    /// placeholder becomes `tonumber(tostring(value))`
    fn set_placeholder(&mut self, var: &str, value: Option<f64>) {
        match value.and_then(|v| pat::to_number(&pat::num(v))) {
            Some(v) => self.placeholder.insert(var.to_string(), Val::Number(v)),
            None => self.placeholder.remove(var),
        };
    }

    /// `ConfigTab:UpdateLevel()`
    fn update_level(&mut self) {
        let max = self.data.misc("MaxEnemyLevel");
        let positive = |v: Option<&Val>| match v {
            Some(Val::Number(n)) if *n > 0.0 => Some(*n),
            _ => None,
        };
        self.state.enemy_level = Some(if let Some(l) = positive(self.input.get("enemyLevel")) {
            lua_min(max, l)
        } else if let Some(l) = positive(self.placeholder.get("enemyLevel")) {
            lua_min(max, l)
        } else {
            lua_min(max, self.character_level)
        });
    }

    /// `SkillType.<name>`
    fn skill_type(&self, name: &str) -> f64 {
        self.data.skill_types.get(name).copied().unwrap_or(f64::NAN)
    }

    /// The selected value of a list option: its input, else its default
    fn list_value(&self, var: &str, default: &str) -> String {
        match self.input.get(var) {
            Some(v) => v.text(),
            None => default.to_string(),
        }
    }
}

/// `ConfigTabClass:BuildModList()`: the mods of the options set, for the
/// player and the enemy, then those of the custom modifier lines.
#[allow(clippy::too_many_arguments)]
pub fn build_mod_list(
    data: &ConfigData,
    parser: &ModParser,
    input: &mut Inputs,
    placeholder: &mut Inputs,
    custom: &[CustomBlock],
    state: &mut ConfigState,
    character_level: f64,
) -> Result<(Vec<Mod>, Vec<Mod>)> {
    let mut o = Out {
        mods: vec![],
        enemy_mods: vec![],
        input,
        placeholder,
        data,
        state,
        character_level,
    };
    o.update_level();
    for option in OPTIONS.iter() {
        let value = |inputs: &Inputs| inputs.get(option.var).cloned();
        let val = match option.kind {
            Kind::Check | Kind::List | Kind::Text => value(o.input).filter(Val::truthy),
            Kind::Count | Kind::Integer | Kind::CountAllowZero | Kind::Float => {
                let set = |v: &Option<Val>| {
                    v.as_ref().is_some_and(|v| {
                        v.truthy() && (!v.is_num(0.0) || option.kind == Kind::CountAllowZero)
                    })
                };
                let (i, p) = (value(o.input), value(o.placeholder));
                if set(&i) {
                    i
                } else if set(&p) {
                    p
                } else {
                    None
                }
            }
        };
        if let Some(val) = val {
            option.apply(&val, &mut o);
        }
    }
    let (mut mods, enemy_mods) = (o.mods, o.enemy_mods);

    // Custom modifiers
    let conv = Converter::default();
    let mut parse_lines = |text: &str, source: &str| -> Result<()> {
        for line in pat::gmatch1(text, "([^\n]*)\n?") {
            let stripped = pat::gsub(&pat::gsub(&line, "%^%d", ""), "%^x%x%x%x%x%x%x", "");
            let stripped = pat::capture(&stripped, "^%s*(.-)%s*$").unwrap_or_default();
            let (list, extra) = parser.parse(stripped.as_bytes());
            if let (Value::Table(t), false) = (&list, extra.truthy()) {
                for m in t.array() {
                    let mut m = conv.mod_(&m)?;
                    m.source = Some(source.into());
                    if let ModValue::Record(r) = &mut m.value
                        && let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod"))
                    {
                        inner.source = Some(source.into());
                    }
                    mods.push(m);
                }
            }
        }
        Ok(())
    };
    let mut has_block_text = false;
    for block in custom {
        if block.enabled && !block.text.is_empty() {
            has_block_text = true;
            let source = format!("Custom:{}", block.title.as_deref().unwrap_or("Default"));
            parse_lines(&block.text, &source)?;
        }
    }
    // The fallback for tests and headless use
    if !has_block_text
        && let Some(Val::Str(text)) = input.get("customMods").cloned()
        && !text.is_empty()
    {
        parse_lines(&text, "Custom")?;
    }
    Ok((mods, enemy_mods))
}

/// `ModFlag.Attack`
const MOD_FLAG_ATTACK: u32 = 0x1;

/// The options with an apply function, in upstream's order
pub static OPTIONS: std::sync::LazyLock<Vec<ConfigOption>> = std::sync::LazyLock::new(|| {
    vec![
        opt("detonateDeadCorpseLife", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "SkillData",
                ModType::List,
                record([("key", "corpseLife".into()), ("value", val.value())]),
            ));
        }),
        opt("conditionStationary", Kind::Count, condition_stationary),
        opt("conditionMoving", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:Moving"));
        }),
        opt("conditionFullLife", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:FullLife"));
        }),
        opt("conditionLowLife", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:LowLife"));
        }),
        opt("conditionFullMana", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:FullMana"));
        }),
        opt("conditionLowMana", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:LowMana"));
        }),
        opt("conditionFullEnergyShield", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:FullEnergyShield"));
        }),
        opt("conditionLowEnergyShield", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:LowEnergyShield"));
        }),
        opt("conditionHaveEnergyShield", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:HaveEnergyShield"));
        }),
        opt("conditionUnbrokenWard", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:UnbrokenWard"));
        }),
        opt("minionsConditionFullLife", Kind::Check, |_, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([("mod", flag("Condition:FullLife").into())]),
            ));
        }),
        opt("minionsConditionLowLife", Kind::Check, |_, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([("mod", flag("Condition:LowLife").into())]),
            ));
        }),
        opt("minionsConditionFullEnergyShield", Kind::Check, |_, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([("mod", flag("Condition:FullEnergyShield").into())]),
            ));
        }),
        opt("minionsConditionCreatedRecently", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:MinionsCreatedRecently"));
        }),
        opt("lifeRegenMode", Kind::List, |val, o| {
            if val.is_str("AVERAGE") {
                o.mods.push(flag("Condition:LifeRegenBurstAvg"));
            } else if val.is_str("FULL") {
                o.mods.push(flag("Condition:LifeRegenBurstFull"));
            }
        }),
        opt("resourceGainMode", Kind::List, |val, o| {
            if val.is_str("AVERAGE") {
                o.mods.push(flag("Condition:AverageResourceGain"));
            } else if val.is_str("MAX") {
                o.mods.push(flag("Condition:MaxResourceGain"));
            }
        }),
        opt("armourCalculationMode", Kind::List, |val, o| {
            if val.is_str("MAX") {
                o.mods.push(flag("Condition:ArmourMax"));
            } else if val.is_str("AVERAGE") {
                o.mods.push(flag("Condition:ArmourAvg"));
            }
        }),
        opt("warcryMode", Kind::List, |val, o| {
            if val.is_str("MAX") {
                o.mods.push(flag("Condition:WarcryMaxHit"));
            }
        }),
        opt("pactMode", Kind::List, |val, o| {
            if val.is_str("MAX") {
                o.mods.push(flag("Condition:PactMaxHit"));
            }
        }),
        opt("EVBypass", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:EVBypass"));
        }),
        opt("arcaneCloakUsedRecentlyCheck", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:ArcaneCloakUsedRecently"));
        }),
        opt("aspectOfTheAvianAviansMight", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:AviansMightActive"));
        }),
        opt("aspectOfTheAvianAviansFlight", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:AviansFlightActive"));
        }),
        opt("aspectOfTheCatCatsStealth", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:CatsStealthActive"));
        }),
        opt("aspectOfTheCatCatsAgility", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:CatsAgilityActive"));
        }),
        opt("overrideCrabBarriers", Kind::CountAllowZero, |val, o| {
            o.mods.push(
                cfg("CrabBarriers", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt(
            "aspectOfTheSpiderWebStacks",
            Kind::CountAllowZero,
            |val, o| {
                o.mods.push(
                    cfg(
                        "ExtraSkillMod",
                        ModType::List,
                        record([(
                            "mod",
                            Mod::new(
                                "Multiplier:SpiderWebApplyStack",
                                ModType::Base,
                                val.value(),
                                None,
                            )
                            .into(),
                        )]),
                    )
                    .tag(Tag::skill_name("Aspect of the Spider")),
                );
                if val.num() > 0.0 {
                    o.mods.push(flag("Condition:AspectOfTheSpiderActive"));
                }
            },
        ),
        opt("bannerPlanted", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:BannerPlanted"));
        }),
        opt("bannerValour", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:ValourStacks", ModType::Base, val.value())
                    .tag(Tag::other("IgnoreCond", []))
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("barkskinStacks", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:BarkskinStacks",
                ModType::Base,
                lua_min(val.num(), 10.0),
            ));
            o.mods.push(cfg(
                "Multiplier:MissingBarkskinStacks",
                ModType::Base,
                lua_max(-val.num(), -10.0),
            ));
        }),
        opt("Unbound", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:Unbound"));
        }),
        opt("bladestormInBloodstorm", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:BladestormInBloodstorm")
                    .tag(Tag::skill_name("Bladestorm").include_transfigured()),
            );
        }),
        opt("bladestormInSandstorm", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:BladestormInSandstorm")
                    .tag(Tag::skill_name("Bladestorm").include_transfigured()),
            );
        }),
        opt("bloodsoakedBannerStages", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:BloodsoakedBannerStageAfterFirst",
                    ModType::Base,
                    lua_min(val.num() - 1.0, 9.0),
                )
                .tag(Tag::condition("Effective")),
            );
        }),
        opt("bloodSacramentReservationEHP", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:BloodSacramentReservationEHP"));
        }),
        opt("ActiveBrands", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:ConfigActiveBrands",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("BrandsAttachedToEnemy", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:ConfigBrandsAttachedToEnemy",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("targetBrandedEnemy", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:TargetingBrandedEnemy"));
        }),
        opt("BrandsInLastQuarter", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:BrandLastQuarter"));
            o.mods.push(flag("Condition:BrandLastHalf"));
        }),
        opt("BrandsInLastHalf", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:BrandLastHalf"));
        }),
        opt("carrionGolemNearbyMinion", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:NearbyNonGolemMinion",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("carrionGolemEqualsChaosGolem", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:CarrionEqualChaosGolem"));
        }),
        opt("chaosGolemEqualsStoneGolem", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:ChaosEqualStoneGolem"));
        }),
        opt("cinderflameStacks", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:CinderflameStacks",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("stoneGolemEqualsCarrionGolem", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:StoneEqualCarrionGolem"));
        }),
        opt("closeCombatCombatRush", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:CombatRushActive"));
        }),
        opt("ColdSnapBypassCD", Kind::Check, |_, o| {
            o.mods.push(
                cfg("CooldownRecovery", ModType::Override, 0.0)
                    .tag(Tag::skill_name("Cold Snap").include_transfigured()),
            );
        }),
        opt("ConcPathBypassCD", Kind::Check, |_, o| {
            o.mods.push(
                cfg("CooldownRecovery", ModType::Override, 0.0)
                    .tag(Tag::skill_name("Consecrated Path of Endurance")),
            );
        }),
        opt("conditionCorruptingCryStages", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:CorruptingCryStageAfterFirst",
                    ModType::Base,
                    lua_min(val.num() - 1.0, 9.0),
                )
                .tag(Tag::condition("Effective")),
            );
        }),
        opt("overrideCruelty", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Cruelty", ModType::Override, lua_min(val.num(), 40.0))
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("channellingCycloneCheck", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:ChannellingCyclone"));
        }),
        opt("darkPactSkeletonLife", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([("key", "skeletonLife".into()), ("value", val.value())]),
                )
                .tag(Tag::skill_name("Dark Bargain")),
            );
        }),
        opt("divineSentinelPhysAsFire", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:DivineSentinelPhysAsFire"));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([("mod", flag("Condition:DivineSentinelPhysAsFire").into())]),
                None,
            ));
        }),
        opt("divineSentinelPhysAsLightning", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:DivineSentinelPhysAsLightning"));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:DivineSentinelPhysAsLightning").into(),
                )]),
                None,
            ));
        }),
        opt("divineSentinelRegenLife", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:DivineSentinelRegenLife"));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([("mod", flag("Condition:DivineSentinelRegenLife").into())]),
                None,
            ));
        }),
        opt("divineSentinelRegenMana", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:DivineSentinelRegenMana"));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([("mod", flag("Condition:DivineSentinelRegenMana").into())]),
                None,
            ));
        }),
        opt("divineSentinelChaosResistance", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:DivineSentinelChaosResistance"));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:DivineSentinelChaosResistance").into(),
                )]),
                None,
            ));
        }),
        opt("divineSentinelSelfCurseEffect", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:DivineSentinelSelfCurseEffect"));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:DivineSentinelSelfCurseEffect").into(),
                )]),
                None,
            ));
        }),
        opt("curseOverlaps", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:CurseOverlaps", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt("elementalArmyExposureType", Kind::List, |val, o| {
            if val.is_str("Fire") {
                o.mods.push(cfg("FireExposureChance", ModType::Base, 100.0));
            } else if val.is_str("Cold") {
                o.mods.push(cfg("ColdExposureChance", ModType::Base, 100.0));
            } else if val.is_str("Lightning") {
                o.mods
                    .push(cfg("LightningExposureChance", ModType::Base, 100.0));
            }
        }),
        opt("embraceMadnessActive", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:AffectedByGloriousMadness"));
        }),
        opt("touchedDebuffsCount", Kind::CountAllowZero, |val, o| {
            let val = &Val::Number(lua_min(val.num(), 10.0));
            o.mods.push(
                Mod::new(
                    "DamageTaken",
                    ModType::Inc,
                    val.num() * 6.0,
                    Some(&format!("{} Eroding Touch Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AffectedByGloriousMadness")),
            );
            o.mods.push(
                Mod::new(
                    "ActionSpeed",
                    ModType::Inc,
                    -val.num() * 6.0,
                    Some(&format!("{} Paralysing Touch Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AffectedByGloriousMadness")),
            );
            o.mods.push(
                Mod::new(
                    "FlaskChargesGained",
                    ModType::Inc,
                    -val.num() * 9.0,
                    Some(&format!("{} Diluting Touch Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AffectedByGloriousMadness")),
            );
            o.mods.push(
                Mod::new(
                    "FlaskEffect",
                    ModType::Inc,
                    -val.num() * 9.0,
                    Some(&format!("{} Diluting Touch Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AffectedByGloriousMadness")),
            );
            o.mods.push(
                Mod::new(
                    "LifeRecoveryRate",
                    ModType::Inc,
                    -val.num() * 9.0,
                    Some(&format!("{} Wasting Touch Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AffectedByGloriousMadness")),
            );
            o.mods.push(
                Mod::new(
                    "EnergyShieldRecoveryRate",
                    ModType::Inc,
                    -val.num() * 9.0,
                    Some(&format!("{} Wasting Touch Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AffectedByGloriousMadness")),
            );
        }),
        opt("feedingFrenzyFeedingFrenzyActive", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:FeedingFrenzyActive"));
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    Mod::new("Damage", ModType::More, 10.0, Some("Feeding Frenzy")).into(),
                )]),
            ));
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    Mod::new("MovementSpeed", ModType::Inc, 10.0, Some("Feeding Frenzy")).into(),
                )]),
            ));
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    Mod::new("Speed", ModType::Inc, 10.0, Some("Feeding Frenzy")).into(),
                )]),
            ));
        }),
        opt("flameWallAddedDamage", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:FlameWallAddedDamage"));
        }),
        opt("FlickerStrikeBypassCD", Kind::Check, |_, o| {
            o.mods.push(
                cfg("CooldownRecovery", ModType::Override, 0.0)
                    .tag(Tag::skill_name("Flicker Strike").include_transfigured()),
            );
        }),
        opt("freshMeatBuffs", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:FreshMeatActive"));
        }),
        opt("frostShieldStages", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:FrostShieldStage",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("Disgorged", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:Disgorged"));
        }),
        opt("greaterHarbingerOfTimeSlipstream", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:GreaterHarbingerOfTime"));
        }),
        opt("harbingerOfTimeSlipstream", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:HarbingerOfTime"));
        }),
        opt("multiplierHexDoom", Kind::Count, |val, o| {
            o.mods
                .push(cfg("Multiplier:HexDoomStack", ModType::Base, val.value()));
        }),
        opt("heraldOfAgonyVirulenceStack", Kind::Count, |val, o| {
            o.mods
                .push(cfg("Multiplier:VirulenceStack", ModType::Base, val.value()));
        }),
        opt("hoaOverkill", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([("key", "hoaOverkill".into()), ("value", val.value())]),
                )
                .tag(Tag::skill_name("Herald of Ash")),
            );
        }),
        opt("heraldOfTheHivePressure", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:OtherworldlyPressure",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("inventionMineTrapPlacedDuration", Kind::Count, |val, o| {
            o.mods
                .push(cfg("Multiplier:PlacedDuration", ModType::Base, val.value()));
        }),
        opt("iceNovaCastOnFrostbolt", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:CastOnFrostbolt").tag(Tag::skill_name("Ice Nova of Frostbolts")),
            );
        }),
        opt("infusedChannellingInfusion", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:InfusionActive"));
        }),
        opt("innervateInnervation", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:InnervationActive"));
        }),
        opt("intensifyIntensity", Kind::Count, |val, o| {
            o.mods
                .push(cfg("Multiplier:Intensity", ModType::Base, val.value()));
        }),
        opt("OverloadedIntensity", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:OverloadedIntensity",
                ModType::Base,
                lua_min(val.num(), 3.0),
            ));
        }),
        opt("multiplierLinkedTargets", Kind::Count, |val, o| {
            o.mods
                .push(cfg("Multiplier:LinkedTargets", ModType::Base, val.value()));
        }),
        opt("linkedToMinion", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:LinkedToMinion"));
        }),
        opt("linkedSourceRate", Kind::Float, |val, o| {
            o.mods
                .push(cfg("IntuitiveLinkSourceRate", ModType::Base, val.value()));
        }),
        opt(
            "manabondMissingUnreservedManaPercentage",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([
                            ("key", "ManabondMissingUnreservedManaPercentage".into()),
                            ("value", lua_max(lua_min(val.num(), 100.0), 0.0).into()),
                        ]),
                    )
                    .tag(Tag::skill_name("Manabond")),
                );
            },
        ),
        opt("minionPactLife", Kind::Count, |val, o| {
            o.mods
                .push(cfg("Multiplier:MinionLife", ModType::Base, val.value()));
        }),
        opt("conditionEnemyMalignantMadness", Kind::Check, |_, o| {
            o.enemy_mods.push(flag("Condition:MalignantMadness"));
        }),
        opt("meatShieldEnemyNearYou", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:MeatShieldEnemyNearYou"));
        }),
        opt("enemyHitMistyReflection", Kind::Check, |_, o| {
            o.enemy_mods.push(flag("Condition:MistyReflection"));
        }),
        opt("MomentumStacks", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:MomentumStacks", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("MomentumSwiftnessStacks", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:MomentumStacksRemoved",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("plagueBearerState", Kind::List, |val, o| {
            if val.is_str("INC") {
                o.mods.push(flag("Condition:PlagueBearerIncubating"));
            } else if val.is_str("INF") {
                o.mods.push(flag("Condition:PlagueBearerInfecting"));
            }
        }),
        opt("perforateSpikeOverlap", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:PerforateSpikeOverlap",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::skill_name("Perforate").include_transfigured()),
            );
        }),
        opt("physicalAegisDepleted", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:PhysicalAegisDepleted"));
        }),
        opt("deathmarkDeathmarkActive", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:EnemyHasDeathmark"));
        }),
        opt("prideEffect", Kind::List, |val, o| {
            if val.is_str("MAX") {
                o.mods.push(flag("Condition:PrideMaxEffect"));
            }
        }),
        opt("sacrificedRageCount", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:RageSacrificedStacks",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("raiseSpectreEnableBuffs", Kind::Check, |_, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([("key", "enable".into()), ("value", true.into())]),
                )
                .tag(Tag::skill_type(o.skill_type("Buff")))
                .tag(
                    Tag::skill_name("Raise Spectre")
                        .include_transfigured()
                        .summon_skill(),
                ),
            );
        }),
        opt("raiseSpectreEnableCurses", Kind::Check, |_, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([("key", "enable".into()), ("value", true.into())]),
                )
                .tag(Tag::skill_type(o.skill_type("Hex")))
                .tag(
                    Tag::skill_name("Raise Spectre")
                        .include_transfigured()
                        .summon_skill(),
                ),
            );
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([("key", "enable".into()), ("value", true.into())]),
                )
                .tag(Tag::skill_type(o.skill_type("Mark")))
                .tag(
                    Tag::skill_name("Raise Spectre")
                        .include_transfigured()
                        .summon_skill(),
                ),
            );
        }),
        opt("conditionSummonedSpectreInPast8Sec", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SummonedSpectreInPast8Sec").tag(Tag::condition("Combat")));
        }),
        opt(
            "raiseSpectreBladeVortexBladeCount",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([("key", "dpsMultiplier".into()), ("value", val.value())]),
                    )
                    .tag(Tag::skill_id("DemonModularBladeVortexSpectre")),
                );
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([("key", "dpsMultiplier".into()), ("value", val.value())]),
                    )
                    .tag(Tag::skill_id("GhostPirateBladeVortexSpectre")),
                );
            },
        ),
        opt(
            "raiseSpectreKaomFireBeamTotemStage",
            Kind::Count,
            |val, o| {
                o.mods.push(cfg(
                    "Multiplier:KaomFireBeamTotemStage",
                    ModType::Base,
                    val.value(),
                ));
            },
        ),
        opt(
            "raiseSpectreEnableSummonedUrsaRallyingCry",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([("key", "enable".into()), ("value", true.into())]),
                    )
                    .tag(Tag::skill_id("DropBearSummonedRallyingCry")),
                );
            },
        ),
        opt(
            "raiseSpectreEnableSlashingHorrorEnrage",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([("key", "enable".into()), ("value", false.into())]),
                    )
                    .tag(Tag::skill_id("AzmeriDualStrikeDemonFireEnrage")),
                );
            },
        ),
        opt(
            "raiseSpectreEnableSanguimancerDemonLowLife",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([("key", "enable".into()), ("value", false.into())]),
                    )
                    .tag(Tag::skill_id("ABTTAzmeriShepherdSpellDamage")),
                );
            },
        ),
        opt("raiseSpidersSpiderCount", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:RaisedSpiderConfig",
                ModType::Base,
                val.value(),
            ));
            o.mods.push(
                cfg("Multiplier:RaisedSpider", ModType::Base, 1.0)
                    .tag(Tag::multiplier("RaisedSpiderConfig").limit_stat("ActiveSpiderLimit")),
            );
        }),
        opt("conditionSummonedZombieInPast8Sec", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SummonedZombieInPast8Sec").tag(Tag::condition("Combat")));
        }),
        opt("animateWeaponLingeringBlade", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:AnimatingLingeringBlades"));
        }),
        opt(
            "ShrapnelBallistaProjectileOverlap",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([
                            ("key", "ShrapnelBallistaProjectileOverlap".into()),
                            ("value", val.value()),
                        ]),
                    )
                    .tag(Tag::skill_name("Shrapnel Ballista").include_transfigured()),
                );
            },
        ),
        opt("sigilOfPowerStages", Kind::CountAllowZero, |val, o| {
            o.mods.push(cfg(
                "Multiplier:SigilOfPowerStage",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("siphoningTrapAffectedEnemies", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:EnemyAffectedBySiphoningTrap",
                ModType::Base,
                val.value(),
            ));
            o.mods.push(flag("Condition:SiphoningTrapSiphoning"));
        }),
        opt("configSnipeStages", Kind::Count, |val, o| {
            o.mods
                .push(cfg("Multiplier:SnipeStage", ModType::Base, val.value()));
        }),
        opt("configSpectralTigerCount", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:SpectralTigerConfig",
                ModType::Base,
                val.value(),
            ));
            o.mods.push(
                cfg("Multiplier:SpectralTigerCount", ModType::Base, 1.0)
                    .tag(Tag::multiplier("SpectralTigerConfig").limit_stat("ActiveTigerLimit")),
            );
        }),
        opt("configSpectralWolfCount", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:SpectralWolfConfig",
                ModType::Base,
                val.value(),
            ));
            o.mods.push(
                cfg("Multiplier:SpectralWolfCount", ModType::Base, 1.0)
                    .tag(Tag::multiplier("SpectralWolfConfig").limit_stat("ActiveWolfLimit")),
            );
        }),
        opt("bloodSandStance", Kind::List, |val, o| {
            if val.is_str("SAND") {
                o.mods.push(flag("Condition:SandStance"));
            }
        }),
        opt("changedStance", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:ChangedStanceRecently"));
        }),
        opt("shardsConsumed", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:SteelShardConsumed",
                ModType::Base,
                lua_min(val.num(), 12.0),
            ));
        }),
        opt("steelWards", Kind::Count, |val, o| {
            o.mods
                .push(cfg("Multiplier:SteelWardCount", ModType::Base, val.value()));
        }),
        opt("stormRainBeamOverlap", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([
                        ("key", "beamOverlapMultiplier".into()),
                        ("value", val.value()),
                    ]),
                )
                .tag(Tag::skill_name("Storm Rain")),
            );
        }),
        opt("stormRainActiveArrows", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([
                        ("key", "activeArrowMultiplier".into()),
                        ("value", val.value()),
                    ]),
                )
                .tag(Tag::skill_name("Storm Rain of the Conduit")),
            );
        }),
        opt(
            "summonElementalRelicEnableAngerAura",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([("key", "enable".into()), ("value", true.into())]),
                    )
                    .tag(Tag::skill_id("Anger"))
                    .tag(Tag::skill_name("Summon Elemental Relic").summon_skill()),
                );
            },
        ),
        opt(
            "summonElementalRelicEnableHatredAura",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([("key", "enable".into()), ("value", true.into())]),
                    )
                    .tag(Tag::skill_id("Hatred"))
                    .tag(Tag::skill_name("Summon Elemental Relic").summon_skill()),
                );
            },
        ),
        opt(
            "summonElementalRelicEnableWrathAura",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    cfg(
                        "SkillData",
                        ModType::List,
                        record([("key", "enable".into()), ("value", true.into())]),
                    )
                    .tag(Tag::skill_id("Wrath"))
                    .tag(Tag::skill_name("Summon Elemental Relic").summon_skill()),
                );
            },
        ),
        opt("summonHolyRelicEnableHolyRelicBoon", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:HolyRelicBoonActive"));
        }),
        opt("summonLightningGolemEnableWrath", Kind::Check, |_, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([("key", "enable".into()), ("value", true.into())]),
                )
                .tag(Tag::skill_id("LightningGolemWrath")),
            );
        }),
        opt("summonReaperConsumeRecently", Kind::Check, |_, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([("key", "enable".into()), ("value", true.into())]),
                )
                .tag(Tag::skill_id("ReaperConsumeMinionForBuff")),
            );
        }),
        opt("weepingBlackStacks", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:WeepingBlackStacks",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("nearbyBleedingEnemies", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:NearbyBleedingEnemies",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("tornadoShotSecondaryHitChance", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([
                        ("key", "tornadoShotSecondaryHitChance".into()),
                        ("value", val.value()),
                    ]),
                )
                .tag(Tag::skill_name("Tornado Shot")),
            );
        }),
        opt("toxicRainPodOverlap", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([
                        ("key", "podOverlapMultiplier".into()),
                        ("value", val.value()),
                    ]),
                )
                .tag(Tag::skill_name("Toxic Rain").include_transfigured()),
            );
        }),
        opt("traumaStacks", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:TraumaStacks", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("configResonanceCount", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:ResonanceCount",
                ModType::Base,
                lua_max(lua_min(val.num(), 50.0), 0.0),
            ));
        }),
        opt("configUnholyResonanceCount", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:UnholyResonanceCount",
                ModType::Base,
                lua_max(lua_min(val.num(), 50.0), 0.0),
            ));
        }),
        opt("conditionInsane", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:Insane"));
        }),
        opt("VigilantStrikeBypassCD", Kind::Check, |_, o| {
            o.mods.push(
                cfg("CooldownRecovery", ModType::Override, 0.0)
                    .tag(Tag::skill_name("Vigilant Strike")),
            );
        }),
        opt("voltaxicBurstSpellsQueued", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:VoltaxicWaitingStages",
                ModType::Base,
                val.value(),
            ));
        }),
        opt("vortexCastOnFrostbolt", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:CastOnFrostbolt").tag(Tag::skill_name("Vortex of Projection")),
            );
        }),
        opt("multiplierWarcryPower", Kind::Count, |val, o| {
            o.mods
                .push(cfg("WarcryPower", ModType::Override, val.value()));
        }),
        opt("waveOfConvictionExposureType", Kind::List, |val, o| {
            if val.is_str("Fire") {
                o.mods
                    .push(flag("Condition:WaveOfConvictionFireExposureActive"));
            } else if val.is_str("Cold") {
                o.mods
                    .push(flag("Condition:WaveOfConvictionColdExposureActive"));
            } else if val.is_str("Lightning") {
                o.mods
                    .push(flag("Condition:WaveOfConvictionLightningExposureActive"));
            }
        }),
        opt("multiplierWoCExpiredDuration", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:WoCDurationExpired",
                    ModType::Base,
                    lua_min(val.num(), 100.0),
                )
                .tag(Tag::condition("Effective")),
            );
        }),
        opt("absolutionSkillDamageCountedOnce", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:AbsolutionSkillDamageCountedOnce").tag(Tag::condition("Combat")),
            );
        }),
        opt(
            "dominatingBlowSkillDamageCountedOnce",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:DominatingBlowSkillDamageCountedOnce")
                        .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("holyStrikeSkillDamageCountedOnce", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:HolyStrikeSkillDamageCountedOnce").tag(Tag::condition("Combat")),
            );
        }),
        opt("MoltenShellDamageMitigated", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([
                        ("key", "MoltenShellDamageMitigated".into()),
                        ("value", val.value()),
                    ]),
                )
                .tag(Tag::skill_name("Molten Shell")),
            );
        }),
        opt("VaalMoltenShellDamageMitigated", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "SkillData",
                    ModType::List,
                    record([
                        ("key", "VaalMoltenShellDamageMitigated".into()),
                        ("value", val.value()),
                    ]),
                )
                .tag(Tag::skill_name("Molten Shell")),
            );
        }),
        opt("enemySizePreset", Kind::List, enemy_size_preset),
        opt("enemyRadius", Kind::Integer, |val, o| {
            o.mods.push(cfg(
                "EnemyRadius",
                ModType::Override,
                lua_max(val.num(), 1.0),
            ));
        }),
        opt("TotalMinionLife", Kind::Integer, |val, o| {
            o.mods
                .push(cfg("TotalMinionLife", ModType::Override, val.value()));
        }),
        opt("TotalSpectreLife", Kind::Integer, |val, o| {
            o.mods
                .push(cfg("TotalSpectreLife", ModType::Override, val.value()));
        }),
        opt("TotalTotemLife", Kind::Integer, |val, o| {
            o.mods
                .push(cfg("TotalTotemLife", ModType::Override, val.value()));
        }),
        opt("TotalRadianceSentinelLife", Kind::Integer, |val, o| {
            o.mods.push(cfg(
                "TotalRadianceSentinelLife",
                ModType::Override,
                val.value(),
            ));
        }),
        opt("TotalVoidSpawnLife", Kind::Integer, |val, o| {
            o.mods
                .push(cfg("TotalVoidSpawnLife", ModType::Override, val.value()));
        }),
        opt("TotalStoneGolemLife", Kind::Integer, |val, o| {
            o.mods
                .push(cfg("TotalStoneGolemLife", ModType::Override, val.value()));
        }),
        opt("TotalVaalRejuvenationTotemLife", Kind::Integer, |val, o| {
            o.mods.push(cfg(
                "TotalVaalRejuvenationTotemLife",
                ModType::Override,
                val.value(),
            ));
        }),
        opt("multiplierSextant", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "Multiplier:Sextant",
                ModType::Base,
                lua_min(val.num(), 5.0),
            ));
        }),
        opt("MapPrefix1", Kind::List, map_affix),
        opt("MapPrefix2", Kind::List, map_affix),
        opt("MapPrefix3", Kind::List, map_affix),
        opt("MapPrefix4", Kind::List, map_affix),
        opt("MapSuffix1", Kind::List, map_affix),
        opt("MapSuffix2", Kind::List, map_affix),
        opt("MapSuffix3", Kind::List, map_affix),
        opt("MapSuffix4", Kind::List, map_affix),
        opt("PvpScaling", Kind::Check, |_, o| {
            o.mods.push(flag("HasPvpScaling"));
        }),
        opt("playerCursedWithAssassinsMark", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "AssassinsMark".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("playerCursedWithConductivity", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "Conductivity".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("playerCursedWithDespair", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "Despair".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt(
            "playerCursedWithElementalWeakness",
            Kind::Count,
            |val, o| {
                o.mods.push(Mod::new(
                    "ExtraCurse",
                    ModType::List,
                    record([
                        ("skillId", "ElementalWeakness".into()),
                        ("level", val.value()),
                        ("applyToPlayer", true.into()),
                    ]),
                    None,
                ));
            },
        ),
        opt("playerCursedWithEnfeeble", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "Enfeeble".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("playerCursedWithFlammability", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "Flammability".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("playerCursedWithFrostbite", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "Frostbite".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("playerCursedWithPoachersMark", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "PoachersMark".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt(
            "playerCursedWithProjectileWeakness",
            Kind::Count,
            |val, o| {
                o.mods.push(Mod::new(
                    "ExtraCurse",
                    ModType::List,
                    record([
                        ("skillId", "ProjectileWeakness".into()),
                        ("level", val.value()),
                        ("applyToPlayer", true.into()),
                    ]),
                    None,
                ));
            },
        ),
        opt("playerCursedWithPunishment", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "Punishment".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("playerCursedWithTemporalChains", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "TemporalChains".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("playerCursedWithVulnerability", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "Vulnerability".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("playerCursedWithWarlordsMark", Kind::Count, |val, o| {
            o.mods.push(Mod::new(
                "ExtraCurse",
                ModType::List,
                record([
                    ("skillId", "WarlordsMark".into()),
                    ("level", val.value()),
                    ("applyToPlayer", true.into()),
                ]),
                None,
            ));
        }),
        opt("usePowerCharges", Kind::Check, |_, o| {
            o.mods
                .push(flag("UsePowerCharges").tag(Tag::condition("Combat")));
        }),
        opt("overridePowerCharges", Kind::Count, |val, o| {
            o.mods.push(
                cfg("PowerCharges", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("useFrenzyCharges", Kind::Check, |_, o| {
            o.mods
                .push(flag("UseFrenzyCharges").tag(Tag::condition("Combat")));
        }),
        opt("overrideFrenzyCharges", Kind::Count, |val, o| {
            o.mods.push(
                cfg("FrenzyCharges", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("useEnduranceCharges", Kind::Check, |_, o| {
            o.mods
                .push(flag("UseEnduranceCharges").tag(Tag::condition("Combat")));
        }),
        opt("overrideEnduranceCharges", Kind::Count, |val, o| {
            o.mods.push(
                cfg("EnduranceCharges", ModType::Override, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("useSiphoningCharges", Kind::Check, |_, o| {
            o.mods
                .push(flag("UseSiphoningCharges").tag(Tag::condition("Combat")));
        }),
        opt("overrideSiphoningCharges", Kind::Count, |val, o| {
            o.mods.push(
                cfg("SiphoningCharges", ModType::Override, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("useChallengerCharges", Kind::Check, |_, o| {
            o.mods
                .push(flag("UseChallengerCharges").tag(Tag::condition("Combat")));
        }),
        opt("overrideChallengerCharges", Kind::Count, |val, o| {
            o.mods.push(
                cfg("ChallengerCharges", ModType::Override, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("useBlitzCharges", Kind::Check, |_, o| {
            o.mods
                .push(flag("UseBlitzCharges").tag(Tag::condition("Combat")));
        }),
        opt("overrideBlitzCharges", Kind::Count, |val, o| {
            o.mods.push(
                cfg("BlitzCharges", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierGaleForce", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:GaleForce", ModType::Base, val.value())
                    .tag(Tag::other("IgnoreCond", []))
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("CanGainGaleForce")),
            );
        }),
        opt(
            "overrideInspirationCharges",
            Kind::CountAllowZero,
            |val, o| {
                o.mods.push(
                    cfg("InspirationCharges", ModType::Override, val.value())
                        .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("useGhostShrouds", Kind::Check, |_, o| {
            o.mods
                .push(flag("UseGhostShrouds").tag(Tag::condition("Combat")));
        }),
        opt("overrideGhostShrouds", Kind::Count, |val, o| {
            o.mods.push(
                cfg("GhostShrouds", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("waitForMaxSeals", Kind::Check, |_, o| {
            o.mods
                .push(flag("UseMaxUnleash").tag(Tag::condition("Combat")));
        }),
        opt("repeatMode", Kind::List, |val, o| {
            if val.is_str("AVERAGE") {
                o.mods.push(flag("Condition:averageRepeat"));
                o.mods.push(Mod::new(
                    "MinionModifier",
                    ModType::List,
                    record([("mod", flag("Condition:averageRepeat").into())]),
                    None,
                ));
            } else if val.is_str("FINAL") || val.is_str("FINAL_DPS") {
                o.mods.push(flag("Condition:alwaysFinalRepeat"));
                o.mods.push(Mod::new(
                    "MinionModifier",
                    ModType::List,
                    record([("mod", flag("Condition:alwaysFinalRepeat").into())]),
                    None,
                ));
            }
        }),
        opt("overrideBloodCharges", Kind::CountAllowZero, |val, o| {
            o.mods.push(
                cfg("BloodCharges", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("overrideSpiritCharges", Kind::CountAllowZero, |val, o| {
            o.mods.push(
                cfg("SpiritCharges", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("overrideSpiritInfusion", Kind::CountAllowZero, |val, o| {
            o.mods.push(
                cfg("SpiritInfusion", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("minionsUsePowerCharges", Kind::Check, |_, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("UsePowerCharges").tag(Tag::condition("Combat")).into(),
                )]),
            ));
        }),
        opt("minionsUseFrenzyCharges", Kind::Check, |_, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("UseFrenzyCharges")
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
            ));
        }),
        opt("minionsUseEnduranceCharges", Kind::Check, |_, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("UseEnduranceCharges")
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
            ));
        }),
        opt("minionsOverridePowerCharges", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    cfg("PowerCharges", ModType::Override, val.value())
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
            ));
        }),
        opt("minionsOverrideFrenzyCharges", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    cfg("FrenzyCharges", ModType::Override, val.value())
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
            ));
        }),
        opt("minionsOverrideEnduranceCharges", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    cfg("EnduranceCharges", ModType::Override, val.value())
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
            ));
        }),
        opt("multiplierRampage", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:Rampage", ModType::Base, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierSoulEater", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:SoulEaterStack", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierMinionSoulEater", Kind::Count, |val, o| {
            o.mods.push(cfg(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    cfg("Multiplier:SoulEaterStack", ModType::Base, val.value()).into(),
                )]),
            ));
        }),
        opt("conditionFocused", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Focused").tag(Tag::condition("Combat")));
        }),
        opt("buffLifetap", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Lifetap").tag(Tag::condition("Combat")));
            o.mods.push(Mod::new(
                "FlaskLifeRecovery",
                ModType::Inc,
                20.0,
                Some("Lifetap"),
            ));
        }),
        opt("buffOnslaught", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Onslaught").tag(Tag::condition("Combat")));
        }),
        opt("buffArcaneSurge", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ArcaneSurge").tag(Tag::condition("Combat")));
        }),
        opt("minionBuffOnslaught", Kind::Check, |_, o| {
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:Onslaught")
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
                None,
            ));
        }),
        opt("buffUnholyMight", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UnholyMight").tag(Tag::condition("Combat")));
            o.mods.push(
                Mod::new(
                    "Condition:CanWither",
                    ModType::Flag,
                    true,
                    Some("Unholy Might"),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("minionbuffUnholyMight", Kind::Check, |_, o| {
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:UnholyMight")
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
                None,
            ));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    Mod::new(
                        "Condition:CanWither",
                        ModType::Flag,
                        true,
                        Some("Unholy Might"),
                    )
                    .tag(Tag::condition("Combat"))
                    .into(),
                )]),
                None,
            ));
        }),
        opt("buffChaoticMight", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ChaoticMight").tag(Tag::condition("Combat")));
        }),
        opt("buffSacrificialZeal", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SacrificialZeal").tag(Tag::condition("Combat")));
        }),
        opt("minionbuffChaoticMight", Kind::Check, |_, o| {
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:ChaoticMight")
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
                None,
            ));
        }),
        opt("buffPhasing", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Phasing").tag(Tag::condition("Combat")));
        }),
        opt("buffFortification", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Fortified").tag(Tag::condition("Combat")));
        }),
        opt("overrideFortification", Kind::Count, |val, o| {
            o.mods.push(
                cfg("FortificationStacks", ModType::Override, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("buffTailwind", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Tailwind").tag(Tag::condition("Combat")));
        }),
        opt("buffAdrenaline", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Adrenaline").tag(Tag::condition("Combat")));
        }),
        opt("conditionChangedStanceLastSecond", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:StanceChangeLastSecond").tag(Tag::condition("Combat")));
        }),
        opt("buffAlchemistsGenius", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:AlchemistsGenius")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("CanHaveAlchemistGenius")),
            );
        }),
        opt(
            "buffVaalArcLuckyHits",
            Kind::Check,
            buff_vaal_arc_lucky_hits,
        ),
        opt("buffElusive", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:Elusive")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("CanBeElusive")),
            );
            o.mods.push(
                flag("Elusive")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("CanBeElusive")),
            );
        }),
        opt("overrideBuffElusive", Kind::Count, |val, o| {
            o.mods.push(
                cfg("ElusiveEffect", ModType::Override, val.value())
                    .tag(Tag::other("GlobalEffect", [("effectType", "Buff".into())])),
            );
        }),
        opt("buffDivinity", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Divinity").tag(Tag::condition("Combat")));
        }),
        opt("multiplierDefiance", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:Defiance",
                    ModType::Base,
                    lua_min(val.num(), 10.0),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierRage", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:RageStack", ModType::Base, val.value())
                    .tag(Tag::other("IgnoreCond", []))
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("CanGainRage")),
            );
        }),
        opt("buffWildSavagery", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:WildSavagery").tag(Tag::condition("Combat")));
        }),
        opt("conditionLeeching", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Leeching").tag(Tag::condition("Combat")));
        }),
        opt("conditionLeechingLife", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LeechingLife").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:Leeching").tag(Tag::condition("Combat")));
        }),
        opt("conditionLeechingEnergyShield", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LeechingEnergyShield").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:Leeching").tag(Tag::condition("Combat")));
        }),
        opt("conditionLeechingMana", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LeechingMana").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:Leeching").tag(Tag::condition("Combat")));
        }),
        opt(
            "minionsConditionLeechingEnergyShield",
            Kind::Check,
            |_, o| {
                o.mods.push(cfg(
                    "MinionModifier",
                    ModType::List,
                    record([("mod", flag("Condition:LeechingEnergyShield").into())]),
                ));
                o.mods.push(cfg(
                    "MinionModifier",
                    ModType::List,
                    record([("mod", flag("Condition:Leeching").into())]),
                ));
            },
        ),
        opt("conditionUsingFlask", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsingFlask").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedAmethystFlaskRecently", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:UsedAmethystFlaskRecently")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("HaveAmethystFlask")),
            );
        }),
        opt("conditionUsedRubyFlaskRecently", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:UsedRubyFlaskRecently")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("HaveRubyFlask")),
            );
        }),
        opt("conditionUsedSapphireFlaskRecently", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:UsedSapphireFlaskRecently")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("HaveSapphireFlask")),
            );
        }),
        opt("conditionUsedTopazFlaskRecently", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:UsedTopazFlaskRecently")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("HaveTopazFlask")),
            );
        }),
        opt("conditionUsingTincture", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsingTincture").tag(Tag::condition("Combat")));
        }),
        opt("conditionHaveTotem", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HaveTotem").tag(Tag::condition("Combat")));
        }),
        opt("conditionSummonedTotemRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SummonedTotemRecently").tag(Tag::condition("Combat")));
        }),
        opt("TotemsSummoned", Kind::Count, |val, o| {
            o.mods.push(
                cfg("TotemsSummoned", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
            o.mods.push(
                cfg("Condition:HaveTotem", ModType::Flag, val.num() >= 1.0)
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionSummonedGolemInPast8Sec", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SummonedGolemInPast8Sec").tag(Tag::condition("Combat")));
        }),
        opt("conditionSummonedGolemInPast10Sec", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SummonedGolemInPast10Sec").tag(Tag::condition("Combat")));
        }),
        opt("multiplierNearbyAlly", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:NearbyAlly", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierNearbyCorpse", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:NearbyCorpse", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierSummonedMinion", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:SummonedMinion", ModType::Override, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierNonVaalSummonedMinion", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:NonVaalSummonedMinion",
                    ModType::Override,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierPermanentMinion", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:PermanentMinion", ModType::Override, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionOnConsecratedGround", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:OnConsecratedGround").tag(Tag::condition("Combat")));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:OnConsecratedGround")
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
                None,
            ));
        }),
        opt("conditionOnProfaneGround", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:OnProfaneGround").tag(Tag::condition("Combat")));
        }),
        opt("minionConditionOnProfaneGround", Kind::Check, |_, o| {
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:OnProfaneGround")
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
                None,
            ));
        }),
        opt("conditionOnBrineGround", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:OnBrineGround").tag(Tag::condition("Combat")));
            o.mods.push(
                cfg("PhysicalDamageGainAsCold", ModType::Base, 10.0)
                    .tag(Tag::condition("OnBrineGround")),
            );
            o.mods.push(
                cfg("PhysicalDamageGainAsLightning", ModType::Base, 10.0)
                    .tag(Tag::condition("OnBrineGround")),
            );
        }),
        opt("minionConditionOnBrineGround", Kind::Check, |_, o| {
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    flag("Condition:OnBrineGround")
                        .tag(Tag::condition("Combat"))
                        .into(),
                )]),
                None,
            ));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    cfg("PhysicalDamageGainAsCold", ModType::Base, 10.0)
                        .tag(Tag::condition("OnBrineGround"))
                        .into(),
                )]),
                None,
            ));
            o.mods.push(Mod::new(
                "MinionModifier",
                ModType::List,
                record([(
                    "mod",
                    cfg("PhysicalDamageGainAsLightning", ModType::Base, 10.0)
                        .tag(Tag::condition("OnBrineGround"))
                        .into(),
                )]),
                None,
            ));
        }),
        opt("conditionOnCausticGround", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:OnCausticGround").tag(Tag::condition("Combat")));
        }),
        opt("conditionOnFungalGround", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:OnFungalGround").tag(Tag::condition("Combat")));
        }),
        opt("conditionOnBurningGround", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:OnBurningGround").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:Burning").tag(Tag::condition("Combat")));
        }),
        opt("conditionOnChilledGround", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:OnChilledGround").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:Chilled").tag(Tag::condition("Combat")));
        }),
        opt("conditionOnShockedGround", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:OnShockedGround").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:Shocked").tag(Tag::condition("Combat")));
        }),
        opt("conditionBlinded", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Blinded").tag(Tag::condition("Combat")));
        }),
        opt("conditionBurning", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Burning").tag(Tag::condition("Combat")));
        }),
        opt("conditionIgnited", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Ignited").tag(Tag::condition("Combat")));
        }),
        opt("conditionScorched", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Scorched").tag(Tag::condition("Combat")));
        }),
        opt("conditionChilled", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Chilled").tag(Tag::condition("Combat")));
        }),
        opt("conditionChilledEffect", Kind::Count, |val, o| {
            o.mods.push(
                Mod::new("ChillVal", ModType::Override, val.value(), Some("Chill"))
                    .tag(Tag::condition("Chilled")),
            );
        }),
        opt("conditionFrozen", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Frozen").tag(Tag::condition("Combat")));
        }),
        opt("conditionBrittle", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Brittle").tag(Tag::condition("Combat")));
        }),
        opt("conditionShocked", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Shocked").tag(Tag::condition("Combat")));
        }),
        opt("conditionPlayerShockEffect", Kind::Count, |val, o| {
            o.mods.push(
                Mod::new("ShockVal", ModType::Override, val.value(), Some("Shock"))
                    .tag(Tag::condition("Shocked")),
            );
        }),
        opt("conditionSapped", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Sapped").tag(Tag::condition("Combat")));
        }),
        opt("conditionBleeding", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Bleeding").tag(Tag::condition("Combat")));
        }),
        opt("conditionPoisoned", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Poisoned").tag(Tag::condition("Combat")));
        }),
        opt("conditionCanBeCurseImmune", Kind::Check, |_, o| {
            o.mods.push(
                cfg("AvoidCurse", ModType::Base, 100.0)
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::other(
                        "GlobalEffect",
                        [("effectType", "Global".into()), ("unscalable", true.into())],
                    )),
            );
        }),
        opt("multiplierPoisonOnSelf", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:PoisonStack", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt(
            "multiplierWitheredStackCountSelf",
            Kind::CountAllowZero,
            |val, o| {
                o.mods.push(
                    cfg("Multiplier:WitheredStack", ModType::Base, val.value())
                        .tag(Tag::condition("Effective")),
                );
            },
        ),
        opt("multiplierNearbyEnemies", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:NearbyEnemies", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.mods.push(
                cfg(
                    "Condition:OnlyOneNearbyEnemy",
                    ModType::Flag,
                    val.is_num(1.0),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt(
            "multiplierNearbyRareOrUniqueEnemies",
            Kind::CountAllowZero,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:NearbyRareOrUniqueEnemies",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Combat")),
                );
                o.mods.push(
                    cfg("Multiplier:NearbyEnemies", ModType::Base, val.value())
                        .tag(Tag::condition("Combat")),
                );
                o.mods.push(
                    cfg(
                        "Condition:AtMostOneNearbyRareOrUniqueEnemy",
                        ModType::Flag,
                        val.num() <= 1.0,
                    )
                    .tag(Tag::condition("Combat")),
                );
                o.enemy_mods.push(
                    cfg(
                        "Condition:NearbyRareOrUniqueEnemy",
                        ModType::Flag,
                        val.num() >= 1.0,
                    )
                    .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionHitRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HitRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionHitSpellRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HitSpellRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:HitRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionCritRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:CritRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:SkillCritRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionSkillCritRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SkillCritRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "conditionCritWithHeraldSkillRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:CritWithHeraldSkillRecently").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("multiplierCritRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:CritRecently", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.mods.push(
                cfg(
                    "Condition:SkillCritRecently",
                    ModType::Flag,
                    1.0 <= val.num(),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("LostNonVaalBuffRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LostNonVaalBuffRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionNonCritRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:NonCritRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionChannelling", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:Channelling").tag(Tag::condition("Combat")));
        }),
        opt("multiplierChannelling", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:ChannellingTime", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.mods
                .push(flag("Condition:Channelling").tag(Tag::condition("Combat")));
        }),
        opt("conditionHitRecentlyWithWeapon", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HitRecentlyWithWeapon").tag(Tag::condition("Combat")));
        }),
        opt("conditionKilledRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:KilledRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierKilledRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:EnemyKilledRecently", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.mods
                .push(flag("Condition:KilledRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionKilledLast3Seconds", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:KilledLast3Seconds").tag(Tag::condition("Combat")));
        }),
        opt(
            "conditionKilledPoisonedLast2Seconds",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:KilledPoisonedLast2Seconds").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt(
            "conditionKilledTauntedEnemyRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:KilledTauntedEnemyRecently").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt(
            "conditionTotemsNotSummonedInPastTwoSeconds",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:NoSummonedTotemsInPastTwoSeconds")
                        .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionTotemsKilledRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:TotemsKilledRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionTotemsHitRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:TotemsHitRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionTotemsHitSpellRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:TotemsHitSpellRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:TotemsHitRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedBrandRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedBrandRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierTotemsKilledRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:EnemyKilledByTotemsRecently",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
            o.mods
                .push(flag("Condition:TotemsKilledRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionMinionsKilledRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:MinionsKilledRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionMinionsDiedRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:MinionsDiedRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierMinionsKilledRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:EnemyKilledByMinionsRecently",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
            o.mods
                .push(flag("Condition:MinionsKilledRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionKilledAffectedByDoT", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:KilledAffectedByDotRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "multiplierShockedEnemyKilledRecently",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:ShockedEnemyKilledRecently",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt(
            "multiplierShockedNonShockedEnemyRecently",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:ShockedNonShockedEnemyRecently",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionFrozenEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:FrozenEnemyRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionChilledEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ChilledEnemyRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionShatteredEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ShatteredEnemyRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionIgnitedEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:IgnitedEnemyRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierIgniteAppliedRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:IgniteAppliedRecently",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionShockedEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ShockedEnemyRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionStunnedEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:StunnedEnemyRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionStunnedRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:StunnedRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionPoisonedEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:PoisonedEnemyRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierPoisonAppliedRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:PoisonAppliedRecently",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierLifeSpentRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:LifeSpentRecently", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierManaSpentRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:ManaSpentRecently", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionWardBrokenPast2Seconds", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:WardBrokenPast2Seconds").tag(Tag::condition("Combat")));
        }),
        opt("conditionBeenHitRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BeenHitRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierBeenHitRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:BeenHitRecently", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.mods.push(
                cfg("Condition:BeenHitRecently", ModType::Flag, 1.0 <= val.num())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionBeenHitByAttackRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BeenHitByAttackRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionBeenCritRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BeenCritRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "conditionConsumed12SteelShardsRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:Consumed12SteelShardsRecently").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionGainedPowerChargeRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:GainedPowerChargeRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "conditionGainedFrenzyChargeRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:GainedFrenzyChargeRecently").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionBeenSavageHitRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BeenSavageHitRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:BeenHitRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionHitByFireDamageRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HitByFireDamageRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:BeenHitRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionHitByColdDamageRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HitByColdDamageRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:BeenHitRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "conditionHitByLightningDamageRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:HitByLightningDamageRecently").tag(Tag::condition("Combat")),
                );
                o.mods
                    .push(flag("Condition:BeenHitRecently").tag(Tag::condition("Combat")));
            },
        ),
        opt("conditionHitBySpellDamageRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HitBySpellDamageRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:BeenHitRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "conditionTakenFireDamageFromEnemyHitRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:TakenFireDamageFromEnemyHitRecently")
                        .tag(Tag::condition("Combat")),
                );
                o.mods
                    .push(flag("Condition:BeenHitRecently").tag(Tag::condition("Combat")));
            },
        ),
        opt("conditionBlockedRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BlockedRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionBlockedAttackRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BlockedAttackRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:BlockedRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionBlockedSpellRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BlockedSpellRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:BlockedRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "conditionEnergyShieldRechargeRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:EnergyShieldRechargeRecently").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt(
            "conditionEnergyShieldRechargePastTwoSec",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:EnergyShieldRechargePastTwoSec").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt(
            "conditionStoppedTakingDamageOverTimeRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:StoppedTakingDamageOverTimeRecently")
                        .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionConvergence", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:Convergence")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("CanGainConvergence")),
            );
        }),
        opt("buffPendulum", Kind::List, |val, o| {
            if val.is_str("AREA") {
                o.mods.push(
                    flag("Condition:PendulumOfDestructionAreaOfEffect")
                        .tag(Tag::condition("Combat")),
                );
            } else if val.is_str("DAMAGE") {
                o.mods.push(
                    flag("Condition:PendulumOfDestructionElementalDamage")
                        .tag(Tag::condition("Combat")),
                );
            }
        }),
        opt("buffConflux", Kind::List, |val, o| {
            if val.is_str("CHILLING") || val.is_str("ALL") {
                o.mods
                    .push(flag("Condition:ChillingConflux").tag(Tag::condition("Combat")));
            }
            if val.is_str("SHOCKING") || val.is_str("ALL") {
                o.mods
                    .push(flag("Condition:ShockingConflux").tag(Tag::condition("Combat")));
            }
            if val.is_str("IGNITING") || val.is_str("ALL") {
                o.mods
                    .push(flag("Condition:IgnitingConflux").tag(Tag::condition("Combat")));
            }
        }),
        opt("highestDamageType", Kind::List, |val, o| {
            if !val.is_str("NONE") {
                o.mods.push(flag(&format!(
                    "Condition:{}IsHighestDamageType",
                    val.text()
                )));
                o.mods.push(flag("IsHighestDamageTypeOVERRIDE"));
            }
        }),
        opt("buffHeartstopper", Kind::List, |val, o| {
            if val.is_str("HIT") {
                o.mods
                    .push(flag("Condition:HeartstopperHIT").tag(Tag::condition("Combat")));
            } else if val.is_str("DOT") {
                o.mods
                    .push(flag("Condition:HeartstopperDOT").tag(Tag::condition("Combat")));
            } else if val.is_str("AVERAGE") {
                o.mods
                    .push(flag("Condition:HeartstopperAVERAGE").tag(Tag::condition("Combat")));
            }
        }),
        opt("buffBastionOfHope", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BastionOfHopeActive").tag(Tag::condition("Combat")));
        }),
        opt("buffNgamahuFlamesAdvance", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:NgamahuFlamesAdvance").tag(Tag::condition("Combat")));
        }),
        opt("buffHerEmbrace", Kind::Check, |_, o| {
            o.mods.push(
                flag("HerEmbrace")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("CanGainHerEmbrace")),
            );
        }),
        opt("conditionChampionIntimidate", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:ChampionIntimidate").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedSkillRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierSkillUsedRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:SkillUsedRecently", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionAttackedRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:AttackedRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionCastSpellRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:CastSpellRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "multiplierNonInstantSpellCastRecently",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:NonInstantSpellCastRecently",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt(
            "multiplierAppliedAilmentsRecently",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:AppliedAilmentsRecently",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionLinkedRecently", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:LinkedRecently"));
        }),
        opt(
            "conditionStunnedWhileCastingRecently",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:StunnedWhileCastingRecently").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionCastLast1Seconds", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:CastLast1Seconds").tag(Tag::condition("Combat")));
        }),
        opt("multiplierCastLast8Seconds", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:CastLast8Seconds", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionSuppressedRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SuppressedRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierHitsSuppressedRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:HitsSuppressedRecently",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
            o.mods
                .push(flag("Condition:SuppressedRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedFireSkillRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedFireSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedColdSkillRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedColdSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedMinionSkillRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedMinionSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedTravelSkillRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedTravelSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedMovementSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedDashRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:CastDashRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedTravelSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedMovementSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedMovementSkillRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedMovementSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedVaalSkillRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedVaalSkillRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "multiplierUsedVaalSkillInPast8Seconds",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:VaalSkillsUsedInPast8Seconds",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionSoulGainPrevention", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SoulGainPrevention").tag(Tag::condition("Combat")));
        }),
        opt("conditionSacrificeMinion", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SacrificeMinionOnAttack").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedWarcryRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedWarcryRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedWarcryInPast8Seconds").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionUsedWarcryInPast8Seconds", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:UsedWarcryInPast8Seconds").tag(Tag::condition("Combat")));
        }),
        opt(
            "multiplierAffectedByWarcryBuffDuration",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:AffectedByWarcryBuffDuration",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("DetonatedMinesRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:DetonatedMinesRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierMineDetonatedRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:MineDetonatedRecently",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("minesPerThrow", Kind::Count, |val, o| {
            o.mods.push(
                cfg("MineThrowCount", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("TriggeredTrapsRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:TriggeredTrapsRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierTrapTriggeredRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:TrapTriggeredRecently",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionThrownTrapOrMineRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:TrapOrMineThrownRecently").tag(Tag::condition("Combat")));
        }),
        opt("trapsPerThrow", Kind::Count, |val, o| {
            o.mods.push(
                cfg("TrapThrowCount", ModType::Override, val.value()).tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionCursedEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:CursedEnemyRecently").tag(Tag::condition("Effective")));
        }),
        opt("conditionCastMarkRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:CastMarkRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionSpawnedCorpseRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SpawnedCorpseRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionConsumedCorpseRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ConsumedCorpseRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionConsumedCorpseInPast2Sec", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ConsumedCorpseInPast2Sec").tag(Tag::condition("Combat")));
        }),
        opt("multiplierCorpseConsumedRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:CorpseConsumedRecently",
                    ModType::Base,
                    val.value(),
                )
                .tag(Tag::condition("Combat")),
            );
            o.mods
                .push(flag("Condition:ConsumedCorpseRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionRavenousCorpseConsumed", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:RavenousCorpseConsumed").tag(Tag::condition("Combat")));
        }),
        opt("multiplierWarcryUsedRecently", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:WarcryUsedRecently",
                    ModType::Base,
                    lua_min(val.num(), 100.0),
                )
                .tag(Tag::condition("Combat")),
            );
            o.mods
                .push(flag("Condition:UsedWarcryRecently").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedWarcryInPast8Seconds").tag(Tag::condition("Combat")));
            o.mods
                .push(flag("Condition:UsedSkillRecently").tag(Tag::condition("Combat")));
        }),
        opt("conditionTauntedEnemyRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:TauntedEnemyRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "conditionLostEnduranceChargeInPast8Sec",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:LostEnduranceChargeInPast8Sec").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt(
            "multiplierEnduranceChargesLostRecently",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:EnduranceChargesLostRecently",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Combat")),
                );
                o.mods.push(
                    flag("Condition:LostEnduranceChargeInPast8Sec").tag(Tag::condition("Combat")),
                );
            },
        ),
        opt(
            "conditionBlockedHitFromUniqueEnemyInPast10Sec",
            Kind::Check,
            |_, o| {
                o.mods.push(
                    flag("Condition:BlockedHitFromUniqueEnemyInPast10Sec")
                        .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionKilledUniqueEnemy", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:KilledUniqueEnemy").tag(Tag::condition("Combat")));
        }),
        opt("BlockedPast10Sec", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:BlockedPast10Sec", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionImpaledRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ImpaledRecently").tag(Tag::condition("Combat")));
        }),
        opt(
            "multiplierImpalesOnEnemy",
            Kind::CountAllowZero,
            |val, o| {
                o.enemy_mods.push(
                    cfg("Multiplier:ImpaleStacks", ModType::Base, val.value())
                        .tag(Tag::condition("Combat")),
                );
            },
        ),
        opt("conditionCausedBleedingRecently", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:CausedBleedingRecently").tag(Tag::condition("Combat")));
        }),
        opt("multiplierBleedsOnEnemy", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:BleedStacks", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.enemy_mods
                .push(flag("Condition:Bleeding").tag(Tag::condition("Effective")));
        }),
        opt("multiplierFragileRegrowth", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "Multiplier:FragileRegrowthCount",
                    ModType::Base,
                    lua_min(val.num(), 10.0),
                )
                .tag(Tag::condition("Combat")),
            );
        }),
        opt("conditionHaveArborix", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HaveIronReflexes").tag(Tag::condition("Combat")));
            o.mods.push(cfg("Keystone", ModType::List, "Iron Reflexes"));
        }),
        opt("conditionHaveAugyre", Kind::List, |val, o| {
            if val.is_str("EleOverload") {
                o.mods
                    .push(flag("Condition:HaveElementalOverload").tag(Tag::condition("Combat")));
                o.mods
                    .push(cfg("Keystone", ModType::List, "Elemental Overload"));
            } else if val.is_str("ResTechnique") {
                o.mods
                    .push(flag("Condition:HaveResoluteTechnique").tag(Tag::condition("Combat")));
                o.mods
                    .push(cfg("Keystone", ModType::List, "Resolute Technique"));
            }
        }),
        opt("conditionHaveVulconus", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:HaveAvatarOfFire").tag(Tag::condition("Combat")));
            o.mods
                .push(cfg("Keystone", ModType::List, "Avatar of Fire"));
        }),
        opt("conditionHaveManaStorm", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SacrificeManaForLightning").tag(Tag::condition("Combat")));
        }),
        opt("GamblesprintMovementSpeed", Kind::List, |val, o| {
            o.mods.push(
                cfg("MovementSpeed", ModType::Inc, val.value())
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("HaveGamblesprint")),
            );
        }),
        opt("EverlastingSacrifice", Kind::Check, |_, o| {
            o.mods.push(
                cfg("ElementalResistMax", ModType::Base, 5.0)
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("EverlastingSacrifice")),
            );
            o.mods.push(
                cfg("ChaosResistMax", ModType::Base, 5.0)
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("EverlastingSacrifice")),
            );
        }),
        opt("buffFanaticism", Kind::Check, |_, o| {
            o.mods.push(
                flag("Condition:Fanaticism")
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("CanGainFanaticism")),
            );
        }),
        opt("conditionHitsAlwaysStun", Kind::Check, |_, o| {
            o.mods.push(
                cfg("CullPercent", ModType::Max, 10.0)
                    .tag(Tag::condition("Combat"))
                    .tag(Tag::condition("maceMasteryStunCullSpecced")),
            );
        }),
        opt("multiplierPvpTvalueOverride", Kind::Count, |val, o| {
            o.mods.push(
                cfg("MultiplierPvpTvalueOverride", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
        }),
        opt("multiplierPvpDamage", Kind::Count, |val, o| {
            o.mods
                .push(cfg("PvpDamageMultiplier", ModType::More, val.num() - 100.0));
        }),
        opt("buffAccelerationShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:AccelerationShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffBrutalShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:BrutalShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffDiamondShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:DiamondShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffDivineShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:DivineShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffEchoingShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:EchoingShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffGloomShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:GloomShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffGreaterFreezingShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:GreaterFreezingShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffGreaterShockingShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:GreaterShockingShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffGreaterSkeletalShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:GreaterSkeletalShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffImpenetrableShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ImpenetrableShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffMassiveShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:MassiveShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffReplenishingShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ReplenishingShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffResistanceShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ResistanceShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffResonatingShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:ResonatingShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffLesserAccelerationShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LesserAccelerationShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffLesserBrutalShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LesserBrutalShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffLesserImpenetrableShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LesserImpenetrableShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffLesserMassiveShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LesserMassiveShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffLesserReplenishingShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LesserReplenishingShrine").tag(Tag::condition("Combat")));
        }),
        opt("buffLesserResistanceShrine", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:LesserResistanceShrine").tag(Tag::condition("Combat")));
        }),
        opt("skillForkCount", Kind::Count, |val, o| {
            o.mods.push(
                cfg("ForkedCount", ModType::Base, val.value()).tag(Tag::condition("Effective")),
            );
        }),
        opt("skillChainCount", Kind::Count, |val, o| {
            o.mods.push(
                cfg("ChainCount", ModType::Base, val.value()).tag(Tag::condition("Effective")),
            );
        }),
        opt("skillPierceCount", Kind::Count, |val, o| {
            o.mods.push(
                cfg("PiercedCount", ModType::Base, val.value()).tag(Tag::condition("Effective")),
            );
        }),
        opt("conditionAtCloseRange", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:AtCloseRange").tag(Tag::condition("Effective")));
        }),
        opt(
            "enemyMultiplierEnemyPresenceSeconds",
            Kind::Count,
            |val, o| {
                o.enemy_mods.push(
                    cfg(
                        "Multiplier:EnemyPresenceSeconds",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Effective")),
                );
            },
        ),
        opt("conditionEnemyMoving", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Moving").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyFullLife", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:FullLife").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyLowLife", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:LowLife").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyCursed", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Cursed").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyStunned", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Stunned").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyBleeding", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Bleeding").tag(Tag::condition("Effective")));
        }),
        opt("overrideBleedStackPotential", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "BleedStackPotentialOverride",
                    ModType::Override,
                    val.value(),
                )
                .tag(Tag::condition("Effective")),
            );
        }),
        opt("conditionSingleBleed", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:SingleBleed").tag(Tag::condition("Effective")));
        }),
        opt("multiplierRuptureStacks", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:RuptureStack", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt("conditionEnemyPoisoned", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Poisoned").tag(Tag::condition("Effective")));
        }),
        opt("multiplierPoisonOnEnemy", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:PoisonStack", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt("conditionNonPoisonedOnly", Kind::Check, |_, o| {
            o.mods
                .push(flag("Condition:NonPoisonedOnly").tag(Tag::condition("Effective")));
        }),
        opt("multiplierCurseExpiredOnEnemy", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:CurseExpired", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt(
            "multiplierCurseDurationExpiredOnEnemy",
            Kind::Count,
            |val, o| {
                o.enemy_mods.push(
                    cfg(
                        "Multiplier:CurseDurationExpired",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Effective")),
                );
            },
        ),
        opt("multiplierWitheredStackCount", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:WitheredStack", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt("multiplierCorrosionStackCount", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:CorrosionStack", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
            o.enemy_mods.push(
                Mod::new("Armour", ModType::Base, -5000.0, Some("Corrosion"))
                    .tag(Tag::multiplier("CorrosionStack"))
                    .tag(Tag::actor_condition("enemy", "CanCorrode")),
            );
            o.enemy_mods.push(
                Mod::new("Evasion", ModType::Base, -1000.0, Some("Corrosion"))
                    .tag(Tag::multiplier("CorrosionStack"))
                    .tag(Tag::actor_condition("enemy", "CanCorrode")),
            );
        }),
        opt("multiplierEnsnaredStackCount", Kind::Count, |val, o| {
            o.mods.push(
                cfg("Multiplier:EnsnareStackCount", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
            o.enemy_mods.push(
                flag("Condition:Moving")
                    .tag(Tag::multiplier_threshold("EnsnareStackCount", 1.0).actor("enemy")),
            );
        }),
        opt("conditionEnemyMaimed", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Maimed").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyHindered", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Hindered").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyExcommunicated", Kind::Check, |_, o| {
            o.enemy_mods.push(
                flag("Condition:Excommunicated")
                    .tag(Tag::actor_condition("enemy", "CanExcommunicate")),
            );
        }),
        opt("conditionEnemyBlinded", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Blinded").tag(Tag::condition("Effective")));
        }),
        opt("overrideBuffBlinded", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("BlindEffect", ModType::Override, val.value())
                    .tag(Tag::other("GlobalEffect", [("effectType", "Buff".into())])),
            );
        }),
        opt("conditionEnemyTaunted", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Taunted").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyDebilitated", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Debilitated").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyPacified", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Pacified").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyBurning", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Burning").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyIgnited", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Ignited").tag(Tag::condition("Effective")));
        }),
        opt("overrideIgniteStackPotential", Kind::Count, |val, o| {
            o.mods.push(
                cfg(
                    "IgniteStackPotentialOverride",
                    ModType::Override,
                    val.value(),
                )
                .tag(Tag::condition("Effective")),
            );
        }),
        opt("conditionEnemyScorched", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Scorched").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:ScorchedConfig").tag(Tag::condition("Effective")));
        }),
        opt("conditionScorchedEffect", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("ScorchVal", ModType::Base, val.value()).tag(Tag::condition("ScorchedConfig")),
            );
            o.enemy_mods.push(
                Mod::new(
                    "DesiredScorchVal",
                    ModType::Base,
                    val.value(),
                    Some("Scorch"),
                )
                .tag(Tag::condition("ScorchedConfig").negated()),
            );
        }),
        opt("ScorchStacks", Kind::Integer, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:ScorchStacks", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt("conditionEnemyOnScorchedGround", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Scorched").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:OnScorchedGround").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyChilled", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Chilled").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:ChilledConfig").tag(Tag::condition("Effective")));
        }),
        opt("multiplierChilledByYouSeconds", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:ChilledByYouSeconds", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.enemy_mods
                .push(flag("Condition:ChilledByYou").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyChilledEffect", Kind::Count, |val, o| {
            o.enemy_mods.push(
                Mod::new("ChillVal", ModType::Base, val.value(), Some("Chill"))
                    .tag(Tag::condition("ChilledConfig")),
            );
            o.enemy_mods.push(
                Mod::new("DesiredChillVal", ModType::Base, val.value(), Some("Chill"))
                    .tag(Tag::condition("ChilledConfig").negated()),
            );
        }),
        opt("conditionEnemyChilledByYourHits", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Chilled").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:ChilledByYourHits").tag(Tag::condition("Effective")));
        }),
        opt("HoarfrostStacks", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("HoarfrostFreezeDuration", ModType::Inc, val.num() * 20.0)
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt("multiplierBarnacleStacks", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg(
                    "Multiplier:BarnacleStack",
                    ModType::Base,
                    lua_min(val.num(), 10.0),
                )
                .tag(Tag::condition("Effective")),
            );
            o.enemy_mods.push(
                cfg("SelfFreezeDuration", ModType::Inc, 5.0)
                    .tag(Tag::multiplier("BarnacleStack"))
                    .tag(Tag::condition("Effective")),
            );
            o.enemy_mods.push(
                cfg("SelfChillDuration", ModType::Inc, 5.0)
                    .tag(Tag::multiplier("BarnacleStack"))
                    .tag(Tag::condition("Effective")),
            );
            o.enemy_mods.push(
                cfg("PhysicalDamageConvertToCold", ModType::Base, 5.0)
                    .tag(Tag::multiplier("BarnacleStack"))
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt("conditionEnemyFrozen", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Frozen").tag(Tag::condition("Effective")));
        }),
        opt("multiplierFrozenByYouSeconds", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:FrozenByYouSeconds", ModType::Base, val.value())
                    .tag(Tag::condition("Combat")),
            );
            o.enemy_mods
                .push(flag("Condition:FrozenByYou").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyBrittle", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Brittle").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:BrittleConfig").tag(Tag::condition("Effective")));
        }),
        opt("conditionBrittleEffect", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("BrittleVal", ModType::Base, val.value()).tag(Tag::condition("BrittleConfig")),
            );
            o.enemy_mods.push(
                Mod::new(
                    "DesiredBrittleVal",
                    ModType::Base,
                    val.value(),
                    Some("Brittle"),
                )
                .tag(Tag::condition("BrittleConfig").negated()),
            );
        }),
        opt("conditionEnemyOnBrittleGround", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Brittle").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:OnBrittleGround").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyShocked", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Shocked").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:ShockedConfig").tag(Tag::condition("Effective")));
        }),
        opt("conditionShockEffect", Kind::Count, |val, o| {
            o.enemy_mods.push(
                Mod::new("ShockVal", ModType::Base, val.value(), Some("Shock"))
                    .tag(Tag::condition("ShockedConfig")),
            );
            o.enemy_mods.push(
                Mod::new("DesiredShockVal", ModType::Base, val.value(), Some("Shock"))
                    .tag(Tag::condition("ShockedConfig").negated()),
            );
        }),
        opt("ShockStacks", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:ShockStacks", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt("conditionEnemyOnShockedGround", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Shocked").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:OnShockedGround").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemySapped", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Sapped").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:SappedConfig").tag(Tag::condition("Effective")));
        }),
        opt("conditionSapEffect", Kind::Count, |val, o| {
            o.enemy_mods.push(
                Mod::new("SapVal", ModType::Base, val.value(), Some("Sap"))
                    .tag(Tag::condition("SappedConfig")),
            );
            o.enemy_mods.push(
                Mod::new("DesiredSapVal", ModType::Base, val.value(), Some("Sap"))
                    .tag(Tag::condition("SappedConfig").negated()),
            );
        }),
        opt("conditionEnemyOnSappedGround", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Sapped").tag(Tag::condition("Effective")));
            o.enemy_mods
                .push(flag("Condition:OnSappedGround").tag(Tag::condition("Effective")));
        }),
        opt(
            "multiplierFreezeShockIgniteOnEnemy",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:FreezeShockIgniteOnEnemy",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Effective")),
                );
            },
        ),
        opt("conditionEnemyFireExposure", Kind::Check, |_, o| {
            o.enemy_mods.push(
                cfg("FireExposure", ModType::Base, -10.0)
                    .tag(Tag::condition("Effective"))
                    .tag(Tag::actor_condition("enemy", "CanApplyFireExposure")),
            );
        }),
        opt("conditionEnemyColdExposure", Kind::Check, |_, o| {
            o.enemy_mods.push(
                cfg("ColdExposure", ModType::Base, -10.0)
                    .tag(Tag::condition("Effective"))
                    .tag(Tag::actor_condition("enemy", "CanApplyColdExposure")),
            );
        }),
        opt("conditionEnemyLightningExposure", Kind::Check, |_, o| {
            o.enemy_mods.push(
                cfg("LightningExposure", ModType::Base, -10.0)
                    .tag(Tag::condition("Effective"))
                    .tag(Tag::actor_condition("enemy", "CanApplyLightningExposure")),
            );
        }),
        opt("conditionEnemyIntimidated", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Intimidated").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyCrushed", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Crushed").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyHallowingFlame", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:HallowingFlame").tag(Tag::condition("Effective")));
        }),
        opt("multiplierEnemyHallowingFlame", Kind::Count, |val, o| {
            o.enemy_mods.push(
                cfg("Multiplier:HallowingFlame", ModType::Base, val.value())
                    .tag(Tag::condition("Effective")),
            );
        }),
        opt(
            "multiplierHallowingFlameStacksRemovedByAlly",
            Kind::CountAllowZero,
            |val, o| {
                o.mods.push(cfg(
                    "Multiplier:HallowingFlameStacksRemovedByAlly",
                    ModType::Base,
                    val.value(),
                ));
            },
        ),
        opt(
            "conditionHallowingFlameMagnitude",
            Kind::CountAllowZero,
            |val, o| {
                o.mods.push(cfg(
                    "HallowingFlameMagnitude",
                    ModType::Override,
                    val.value(),
                ));
            },
        ),
        opt("conditionNearLinkedTarget", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:NearLinkedTarget").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyUnnerved", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:Unnerved").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyCoveredInAsh", Kind::Check, |_, o| {
            o.mods.push(Mod::new(
                "CoveredInAshEffect",
                ModType::Base,
                20.0,
                Some("Covered in Ash"),
            ));
        }),
        opt("conditionEnemyCoveredInFrost", Kind::Check, |_, o| {
            o.mods.push(Mod::new(
                "CoveredInFrostEffect",
                ModType::Base,
                20.0,
                Some("Covered in Frost"),
            ));
        }),
        opt("conditionEnemyOnConsecratedGround", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:OnConsecratedGround").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyHaveEnergyShield", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:HaveEnergyShield").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyOnProfaneGround", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:OnProfaneGround").tag(Tag::condition("Effective")));
            o.enemy_mods.push(
                cfg("CurseEffectOnSelf", ModType::Inc, 10.0).tag(Tag::condition("OnProfaneGround")),
            );
            o.enemy_mods.push(
                cfg("SelfCritChance", ModType::Inc, 100.0).tag(Tag::condition("OnProfaneGround")),
            );
        }),
        opt(
            "multiplierEnemyAffectedByGraspingVines",
            Kind::Count,
            |val, o| {
                o.mods.push(
                    cfg(
                        "Multiplier:GraspingVinesAffectingEnemy",
                        ModType::Base,
                        val.value(),
                    )
                    .tag(Tag::condition("Effective")),
                );
            },
        ),
        opt("conditionEnemyOnFungalGround", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:OnFungalGround").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyOnBrineGround", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:OnBrineGround").tag(Tag::condition("Effective")));
            o.enemy_mods.push(
                cfg("SelfChillEffect", ModType::Inc, 30.0).tag(Tag::condition("OnBrineGround")),
            );
            o.enemy_mods.push(
                cfg("SelfBrittleEffect", ModType::Inc, 30.0).tag(Tag::condition("OnBrineGround")),
            );
            o.enemy_mods.push(
                cfg("SelfShockEffect", ModType::Inc, 30.0).tag(Tag::condition("OnBrineGround")),
            );
            o.enemy_mods.push(
                cfg("SelfSapEffect", ModType::Inc, 30.0).tag(Tag::condition("OnBrineGround")),
            );
            o.enemy_mods
                .push(cfg("Armour", ModType::Inc, -25.0).tag(Tag::condition("OnBrineGround")));
            o.enemy_mods
                .push(cfg("Evasion", ModType::Inc, -25.0).tag(Tag::condition("OnBrineGround")));
        }),
        opt("conditionEnemyInChillingArea", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:InChillingArea").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyInFrostGlobe", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:EnemyInFrostGlobe").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyLifeHigherThanPlayer", Kind::Check, |_, o| {
            o.enemy_mods.push(
                flag("Condition:HigherLifePercentThanPlayer").tag(Tag::condition("Effective")),
            );
        }),
        opt("enemyConditionHitByFireDamage", Kind::Check, |_, o| {
            o.enemy_mods.push(flag("Condition:HitByFireDamage"));
        }),
        opt("enemyConditionHitByColdDamage", Kind::Check, |_, o| {
            o.enemy_mods.push(flag("Condition:HitByColdDamage"));
        }),
        opt("enemyConditionHitByLightningDamage", Kind::Check, |_, o| {
            o.enemy_mods.push(flag("Condition:HitByLightningDamage"));
        }),
        opt("enemyInRFOrScorchingRay", Kind::Check, |_, o| {
            o.mods.push(flag("Condition:InRFOrScorchingRay"));
        }),
        opt("conditionBetweenYouAndLinkedTarget", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:BetweenYouAndLinkedTarget").tag(Tag::condition("Effective")));
        }),
        opt("conditionEnemyFireResZero", Kind::Check, |_, o| {
            o.enemy_mods.push(
                cfg("FireResist", ModType::Override, 0.0)
                    .tag(Tag::condition("Effective"))
                    .tag(Tag::actor_condition("enemy", "HaveTrickstersSmile")),
            );
        }),
        opt("conditionEnemyColdResZero", Kind::Check, |_, o| {
            o.enemy_mods.push(
                cfg("ColdResist", ModType::Override, 0.0)
                    .tag(Tag::condition("Effective"))
                    .tag(Tag::actor_condition("enemy", "HaveTrickstersSmile")),
            );
        }),
        opt("conditionEnemyLightningResZero", Kind::Check, |_, o| {
            o.enemy_mods.push(
                cfg("LightningResist", ModType::Override, 0.0)
                    .tag(Tag::condition("Effective"))
                    .tag(Tag::actor_condition("enemy", "HaveTrickstersSmile")),
            );
        }),
        opt("maniaDebuffsCount", Kind::CountAllowZero, |val, o| {
            let val = &Val::Number(lua_min(val.num(), 15.0));
            o.enemy_mods.push(
                Mod::new(
                    "DamageTaken",
                    ModType::Inc,
                    val.num() * 4.0,
                    Some(&format!("{} Mania Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AfflictedByMania")),
            );
            o.enemy_mods.push(
                Mod::new(
                    "ActionSpeed",
                    ModType::Inc,
                    -val.num() * 2.0,
                    Some(&format!("{} Mania Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AfflictedByMania")),
            );
            o.enemy_mods.push(
                Mod::new(
                    "LifeRecoveryRate",
                    ModType::Inc,
                    -val.num() * 10.0,
                    Some(&format!("{} Mania Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AfflictedByMania")),
            );
            o.enemy_mods.push(
                Mod::new(
                    "EnergyShieldRecoveryRate",
                    ModType::Inc,
                    -val.num() * 10.0,
                    Some(&format!("{} Mania Stacks", val.text())),
                )
                .tag(Tag::other(
                    "GlobalEffect",
                    [("effectType", "Debuff".into())],
                ))
                .tag(Tag::condition("AfflictedByMania")),
            );
        }),
        opt("conditionEnemyRareOrUnique", Kind::Check, |_, o| {
            o.enemy_mods
                .push(flag("Condition:RareOrUnique").tag(Tag::condition("Effective")));
        }),
        opt("enemyIsBoss", Kind::List, enemy_is_boss),
        opt("deliriousPercentage", Kind::List, |val, o| {
            if val.is_str("20Percent") {
                o.enemy_mods.push(Mod::new(
                    "DamageTaken",
                    ModType::More,
                    -16.0,
                    Some("20% Delirious"),
                ));
                o.enemy_mods
                    .push(Mod::new("Damage", ModType::Inc, 6.0, Some("20% Delirious")));
            }
            if val.is_str("40Percent") {
                o.enemy_mods.push(Mod::new(
                    "DamageTaken",
                    ModType::More,
                    -32.0,
                    Some("40% Delirious"),
                ));
                o.enemy_mods.push(Mod::new(
                    "Damage",
                    ModType::Inc,
                    12.0,
                    Some("40% Delirious"),
                ));
            }
            if val.is_str("60Percent") {
                o.enemy_mods.push(Mod::new(
                    "DamageTaken",
                    ModType::More,
                    -48.0,
                    Some("60% Delirious"),
                ));
                o.enemy_mods.push(Mod::new(
                    "Damage",
                    ModType::Inc,
                    18.0,
                    Some("60% Delirious"),
                ));
            }
            if val.is_str("80Percent") {
                o.enemy_mods.push(Mod::new(
                    "DamageTaken",
                    ModType::More,
                    -64.0,
                    Some("80% Delirious"),
                ));
                o.enemy_mods.push(Mod::new(
                    "Damage",
                    ModType::Inc,
                    24.0,
                    Some("80% Delirious"),
                ));
            }
            if val.is_str("100Percent") {
                o.enemy_mods.push(Mod::new(
                    "DamageTaken",
                    ModType::More,
                    -80.0,
                    Some("100% Delirious"),
                ));
                o.enemy_mods.push(Mod::new(
                    "Damage",
                    ModType::Inc,
                    30.0,
                    Some("100% Delirious"),
                ));
            }
        }),
        opt("enemyPhysicalReduction", Kind::Integer, |val, o| {
            o.enemy_mods.push(Mod::new(
                "PhysicalDamageReduction",
                ModType::Base,
                val.value(),
                Some("EnemyConfig"),
            ));
        }),
        opt("enemyLightningResist", Kind::CountAllowZero, |val, o| {
            o.enemy_mods.push(Mod::new(
                "LightningResist",
                ModType::Base,
                val.value(),
                Some("EnemyConfig"),
            ));
        }),
        opt("enemyColdResist", Kind::CountAllowZero, |val, o| {
            o.enemy_mods.push(Mod::new(
                "ColdResist",
                ModType::Base,
                val.value(),
                Some("EnemyConfig"),
            ));
        }),
        opt("enemyFireResist", Kind::CountAllowZero, |val, o| {
            o.enemy_mods.push(Mod::new(
                "FireResist",
                ModType::Base,
                val.value(),
                Some("EnemyConfig"),
            ));
        }),
        opt("enemyChaosResist", Kind::CountAllowZero, |val, o| {
            o.enemy_mods.push(Mod::new(
                "ChaosResist",
                ModType::Base,
                val.value(),
                Some("EnemyConfig"),
            ));
        }),
        opt("enemyMaxResist", Kind::Check, |_, o| {
            o.enemy_mods.push(Mod::new(
                "DoNotChangeMaxResFromConfig",
                ModType::Flag,
                true,
                Some("EnemyConfig"),
            ));
        }),
        opt("enemyBlockChance", Kind::CountAllowZero, |val, o| {
            o.enemy_mods
                .push(cfg("BlockChance", ModType::Base, val.value()));
        }),
        opt("enemyEvasion", Kind::CountAllowZero, |val, o| {
            o.enemy_mods
                .push(cfg("Evasion", ModType::Base, val.value()));
        }),
        opt("enemyArmour", Kind::CountAllowZero, |val, o| {
            o.enemy_mods.push(cfg("Armour", ModType::Base, val.value()));
        }),
        opt("presetBossSkills", Kind::List, preset_boss_skills),
        opt("enemyMultiplierPvpDamage", Kind::Count, |val, o| {
            o.enemy_mods
                .push(cfg("MultiplierPvpDamage", ModType::Base, val.value()));
        }),
    ]
});

/// `conditionStationary`: older builds set it as a check box
fn condition_stationary(val: &Val, o: &mut Out) {
    let val = match val {
        Val::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        v => v.num(),
    };
    let seconds = lua_max(0.0, val);
    o.mods
        .push(cfg("Multiplier:StationarySeconds", ModType::Base, seconds));
    if seconds > 0.0 {
        o.mods.push(flag("Condition:Stationary"));
    }
}

/// `enemySizePreset`; the enemy radius control's placeholder changes too,
/// but only for display
fn enemy_size_preset(val: &Val, o: &mut Out) {
    let radius = match val {
        v if v.is_str("Small") => 2.0,
        v if v.is_str("Medium") => 3.0,
        v if v.is_str("Large") => 5.0,
        v if v.is_str("Huge") => 11.0,
        _ => return,
    };
    o.mods.push(cfg("EnemyRadius", ModType::Base, radius));
}

fn buff_vaal_arc_lucky_hits(_: &Val, o: &mut Out) {
    o.mods.push(
        flag("LuckyHits")
            .tag(Tag::condition_any(&["Combat", "CanBeLucky"]))
            .tag(Tag::skill_name("Arc").include_transfigured()),
    );
}

const ENEMY_RESISTS: [&str; 4] = [
    "enemyLightningResist",
    "enemyColdResist",
    "enemyFireResist",
    "enemyChaosResist",
];
const ENEMY_DAMAGES: [&str; 5] = [
    "enemyPhysicalDamage",
    "enemyLightningDamage",
    "enemyColdDamage",
    "enemyFireDamage",
    "enemyChaosDamage",
];
const ENEMY_PENS: [&str; 4] = [
    "enemyPhysicalOverwhelm",
    "enemyLightningPen",
    "enemyColdPen",
    "enemyFirePen",
];

/// `enemyIsBoss`: the enemy's boss mods and the placeholders of its stats
fn enemy_is_boss(val: &Val, o: &mut Out) {
    // Defaults, so the placeholders reset
    o.set_placeholder("enemySpeed", Some(700.0));
    o.set_placeholder("enemyCritChance", Some(5.0));
    let crit = o
        .data
        .monster_constants
        .get("base_critical_strike_multiplier")
        .copied()
        .unwrap_or(f64::NAN);
    o.set_placeholder("enemyCritDamage", Some(crit - 100.0));
    let boss_mods = |o: &mut Out, pinnacle: bool| {
        o.enemy_mods
            .push(flag("Condition:RareOrUnique").tag(Tag::condition("Effective")));
        if pinnacle {
            o.enemy_mods
                .push(flag("Condition:PinnacleBoss").tag(Tag::condition("Effective")));
        }
    };
    let boss = |o: &mut Out, threshold: f64| {
        o.enemy_mods.push(Mod::new(
            "AilmentThreshold",
            ModType::More,
            threshold,
            Some("Boss"),
        ));
        o.mods
            .push(Mod::new("WarcryPower", ModType::Base, 20.0, Some("Boss")));
        o.mods.push(Mod::new(
            "Multiplier:EnemyPower",
            ModType::Base,
            20.0,
            Some("Boss"),
        ));
    };
    let resists = |o: &mut Out, ele: Option<f64>, chaos: Option<f64>| {
        for (i, var) in ENEMY_RESISTS.iter().enumerate() {
            o.set_placeholder(var, if i < 3 { ele } else { chaos });
        }
    };
    let level = |o: &mut Out, default: f64, at_least: bool| -> f64 {
        o.set_placeholder("enemyLevel", if at_least { Some(default) } else { None });
        o.update_level();
        match o.state.enemy_level {
            Some(l) if at_least => lua_max(l, default),
            Some(l) => l,
            None => default,
        }
    };
    let damages = |o: &mut Out, damage: f64, chaos_div: f64| {
        for var in &ENEMY_DAMAGES[..4] {
            o.set_placeholder(var, Some(damage));
        }
        o.set_placeholder("enemyChaosDamage", Some(round(damage / chaos_div)));
    };
    let pens = |o: &mut Out, pen: Option<f64>, overwhelm: bool| {
        if overwhelm {
            o.set_placeholder("enemyPhysicalOverwhelm", None);
        }
        for var in &ENEMY_PENS[1..] {
            o.set_placeholder(var, pen);
        }
    };
    let damage_at = |o: &Out, level: f64| ConfigData::by_level(&o.data.monster_damage, level);
    let armour_at = |o: &Out, level: f64| ConfigData::by_level(&o.data.monster_armour, level);
    let evasion_at = |o: &Out, level: f64| ConfigData::by_level(&o.data.monster_evasion, level);
    let stat = |o: &Out, name: &str| o.data.boss_stats.get(name).copied().unwrap_or(f64::NAN);
    if val.is_str("None") {
        resists(o, None, None);
        let lvl = level(o, 83.0, false);
        let damage = round(damage_at(o, lvl) * 1.5);
        o.set_placeholder("enemyPhysicalDamage", Some(damage));
        for var in &ENEMY_DAMAGES[1..] {
            o.set_placeholder(var, None);
        }
        pens(o, None, true);
        let (a, e) = (armour_at(o, lvl), evasion_at(o, lvl));
        o.set_placeholder("enemyArmour", Some(a));
        o.set_placeholder("enemyEvasion", Some(e));
    } else if val.is_str("Boss") {
        boss_mods(o, false);
        boss(o, 488.0);
        resists(o, Some(40.0), Some(25.0));
        let lvl = level(o, 83.0, false);
        let damage = round(damage_at(o, lvl) * 1.5 * o.data.misc("stdBossDPSMult"));
        damages(o, damage, 2.5);
        pens(o, None, true);
        let (a, e) = (armour_at(o, lvl), evasion_at(o, lvl));
        o.set_placeholder("enemyArmour", Some(a));
        o.set_placeholder("enemyEvasion", Some(e));
    } else if val.is_str("Pinnacle") || val.is_str("Uber") {
        let uber = val.is_str("Uber");
        boss_mods(o, true);
        if uber {
            o.enemy_mods
                .push(Mod::new("DamageTaken", ModType::More, -70.0, Some("Boss")));
        }
        boss(o, 404.0);
        resists(o, Some(50.0), Some(30.0));
        let lvl = level(o, if uber { 85.0 } else { 84.0 }, true);
        let (mult, pen, chaos_div, armour, evasion) = if uber {
            (
                "uberBossDPSMult",
                "uberBossPen",
                4.0,
                "UberArmourMean",
                "UberEvasionMean",
            )
        } else {
            (
                "pinnacleBossDPSMult",
                "pinnacleBossPen",
                2.5,
                "PinnacleArmourMean",
                "PinnacleEvasionMean",
            )
        };
        let damage = round(damage_at(o, lvl) * 1.5 * o.data.misc(mult));
        damages(o, damage, chaos_div);
        let p = o.data.misc(pen);
        pens(o, Some(p), false);
        let a = round(armour_at(o, lvl) * (stat(o, armour) / 100.0));
        let e = round(evasion_at(o, lvl) * (stat(o, evasion) / 100.0));
        o.set_placeholder("enemyArmour", Some(a));
        o.set_placeholder("enemyEvasion", Some(e));
    }
}

/// `presetBossSkills`: a boss skill's damage, penetration, speed and mods
fn preset_boss_skills(val: &Val, o: &mut Out) {
    if val.is_str("None") {
        if o.state.damage_type_locked {
            o.input
                .insert("enemyDamageType".into(), Val::Str("Average".into()));
        }
        o.state.damage_type_locked = false;
        return;
    }
    let name = val.text();
    let Some(boss) = o.data.boss_skills.get(&name).cloned() else {
        return;
    };
    let is_boss = o.list_value("enemyIsBoss", "Pinnacle");
    let uber = is_boss == "Uber" || (boss.earlier_uber && is_boss == "Pinnacle");
    for var in ENEMY_DAMAGES {
        o.set_placeholder(var, None);
    }
    let range = match o.input.get("enemyDamageRollRange") {
        Some(v) if v.truthy() => v.num(),
        // The roll range control shows its placeholder, 70 by default
        _ => o
            .placeholder
            .get("enemyDamageRollRange")
            .map_or(f64::NAN, Val::num),
    };
    let roll = lua_min(lua_max(range, 0.0), 100.0);
    let level = o.state.enemy_level.unwrap_or(f64::NAN);
    let base = ConfigData::by_level(&o.data.monster_damage, level);
    for (typ, [m, r]) in &boss.damage_multipliers {
        let mut damage = base * (m + roll * r);
        if uber && let Some(u) = boss.uber_damage_multiplier {
            damage *= u;
        }
        o.set_placeholder(&format!("enemy{typ}Damage"), Some(round(damage)));
    }
    for var in ENEMY_PENS {
        o.set_placeholder(var, None);
    }
    if let Some(pens) = &boss.damage_penetrations {
        for (typ, pen) in pens {
            let uber_pen = boss
                .uber_damage_penetrations
                .as_ref()
                .and_then(|u| u.get(typ));
            let pen = match uber_pen {
                Some(p) if uber => *p,
                _ => *pen,
            };
            o.set_placeholder(&format!("enemy{typ}"), pen);
        }
    }
    if let Some(t) = &boss.damage_type {
        o.input
            .insert("enemyDamageType".into(), Val::Str(t.as_str().into()));
    }
    o.state.damage_type_locked = true;
    match (uber, boss.uber_speed, boss.speed) {
        (true, Some(s), _) | (_, _, Some(s)) => o.set_placeholder("enemySpeed", Some(s)),
        _ => {}
    }
    if let Some(c) = boss.crit_chance {
        o.set_placeholder("enemyCritChance", Some(c));
    }
    o.mods.push(flag("BossSkillActive"));
    // Boss specific mods
    if name == "Atziri Flameblast" && uber {
        o.enemy_mods.push(Mod::new(
            "Damage",
            ModType::Inc,
            60.0,
            Some("Alluring Abyss Map Mod"),
        ));
    }
    let stats = if uber {
        &boss.additional_uber
    } else {
        &boss.additional_base
    };
    for (stat, value) in stats.iter().flatten() {
        o.enemy_mods.push(match value {
            None => Mod::new(stat, ModType::Flag, true, Some("BossSkillAdditionalData")),
            Some(v) => Mod::new(
                stat,
                ModType::Base,
                v.clone(),
                Some("BossSkillAdditionalData"),
            ),
        });
    }
}

/// How a map mod is set
#[derive(Copy, Clone, Debug, PartialEq)]
enum MapKind {
    Check,
    List,
    Count,
}

/// A map mod's apply function: its tier (0 to 2), roll range (0 to 100)
/// and effect multiplier
type MapApply = fn(usize, f64, f64, &mut Out);

/// `mapAffixDropDownFunction`: the map mod named by the option's value
fn map_affix(val: &Val, o: &mut Out) {
    if val.is_str("NONE") {
        return;
    }
    let name = val.text();
    let Some((_, kind, apply)) = map_mods().into_iter().find(|(n, _, _)| *n == name) else {
        return;
    };
    let effect = 1.0 + o.input.get("multiplierMapModEffect").map_or(0.0, Val::num) / 100.0;
    // `4 - selIndex` of the tier list (red, yellow, white); upstream indexes
    // the values with it from 1
    let sel = match o.input.get("multiplierMapModTier") {
        Some(v) if v.is_str("MED") => 2,
        Some(v) if v.is_str("LOW") => 3,
        _ => 1,
    };
    let tier = 3 - sel;
    match kind {
        MapKind::Check => apply(tier, f64::NAN, effect, o),
        MapKind::List => apply(tier, f64::NAN, effect, o),
        MapKind::Count => apply(tier, 100.0, effect, o),
    }
}

/// The map mods with an apply function (`data.mapMods.AffixData`)
fn map_mods() -> Vec<(&'static str, MapKind, MapApply)> {
    vec![
        ("Armoured", MapKind::List, |tier, _, effect, o| {
            let values = [20.0, 30.0, 40.0];
            o.enemy_mods.push(Mod::new(
                "PhysicalDamageReduction",
                ModType::Base,
                values[tier] * effect,
                Some("Map mod Armoured"),
            ));
        }),
        ("Hexproof", MapKind::Check, |_, _, _, o| {
            o.enemy_mods.push(Mod::new(
                "Hexproof",
                ModType::Flag,
                true,
                Some("Map mod Hexproof"),
            ));
        }),
        ("Hexwarded", MapKind::List, |tier, _, effect, o| {
            let values = [25.0, 40.0, 60.0];
            o.enemy_mods.push(Mod::new(
                "CurseEffectOnSelf",
                ModType::More,
                -values[tier] * effect,
                Some("Map mod Hexwarded"),
            ));
        }),
        ("Resistant", MapKind::List, |tier, _, effect, o| {
            let values = [[20.0, 15.0], [30.0, 20.0], [40.0, 25.0]];
            o.enemy_mods.push(Mod::new(
                "ElementalResist",
                ModType::Base,
                values[tier][0] * effect,
                Some("Map mod Resistant"),
            ));
            o.enemy_mods.push(Mod::new(
                "ChaosResist",
                ModType::Base,
                values[tier][1] * effect,
                Some("Map mod Resistant"),
            ));
        }),
        ("Unwavering", MapKind::Count, |tier, roll, effect, o| {
            let values = [[[15.0, 19.0]], [[20.0, 24.0]], [[25.0, 30.0]]];
            o.enemy_mods.push(Mod::new(
                "AvoidStun",
                ModType::Base,
                100.0,
                Some("Map mod Unwavering"),
            ));
            o.enemy_mods.push(Mod::new(
                "Life",
                ModType::More,
                (values[tier][0][0] + (values[tier][0][1] - values[tier][0][0]) * roll / 100.0)
                    * effect,
                Some("Map mod Unwavering"),
            ));
        }),
        ("Fecund", MapKind::Count, |tier, roll, effect, o| {
            let values = [[20.0, 29.0], [30.0, 39.0], [40.0, 49.0]];
            o.enemy_mods.push(Mod::new(
                "Life",
                ModType::More,
                (values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect,
                Some("Map mod Fecund"),
            ));
        }),
        ("Unstoppable", MapKind::Check, |_, _, _, o| {
            o.enemy_mods.push(Mod::new(
                "MinimumActionSpeed",
                ModType::Max,
                100.0,
                Some("Map mod Unstoppable"),
            ));
        }),
        ("Impervious", MapKind::List, |tier, _, effect, o| {
            let values = [20.0, 35.0, 50.0];
            o.enemy_mods.push(Mod::new(
                "AvoidPoison",
                ModType::Base,
                values[tier] * effect,
                Some("Map mod Impervious"),
            ));
            o.enemy_mods.push(Mod::new(
                "AvoidImpale",
                ModType::Base,
                values[tier] * effect,
                Some("Map mod Impervious"),
            ));
            o.enemy_mods.push(Mod::new(
                "AvoidBleed",
                ModType::Base,
                values[tier] * effect,
                Some("Map mod Impervious"),
            ));
        }),
        ("Oppressive", MapKind::List, |tier, _, effect, o| {
            let values = [30.0, 45.0, 60.0];
            o.enemy_mods.push(Mod::new(
                "SpellSuppressionChance",
                ModType::Base,
                values[tier] * effect,
                Some("Map mod Oppressive"),
            ));
        }),
        ("Buffered", MapKind::Count, |tier, roll, effect, o| {
            let values = [[20.0, 29.0], [30.0, 39.0], [40.0, 49.0]];
            o.enemy_mods.push(Mod::new(
                "LifeGainAsEnergyShield",
                ModType::Base,
                (values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect,
                Some("Map mod Buffered"),
            ));
        }),
        ("Titan's", MapKind::List, |tier, _, effect, o| {
            let values = [[25.0, 45.0], [30.0, 55.0], [35.0, 70.0]];
            o.enemy_mods.push(
                Mod::new(
                    "Life",
                    ModType::More,
                    values[tier][0] * effect,
                    Some("Map mod Titan's"),
                )
                .tag(Tag::condition("RareOrUnique")),
            );
            o.enemy_mods.push(
                Mod::new(
                    "AreaOfEffect",
                    ModType::Inc,
                    values[tier][1] * effect,
                    Some("Map mod Titan's"),
                )
                .tag(Tag::condition("RareOrUnique")),
            );
        }),
        ("Savage", MapKind::Count, |tier, roll, effect, o| {
            let values = [[14.0, 17.0], [18.0, 21.0], [22.0, 25.0]];
            o.enemy_mods.push(Mod::new(
                "Damage",
                ModType::Inc,
                (values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect,
                Some("Map mod Savage"),
            ));
        }),
        ("Burning", MapKind::Count, |tier, roll, effect, o| {
            let values = [[50.0, 69.0], [70.0, 89.0], [90.0, 110.0]];
            o.enemy_mods.push(Mod::new(
                "PhysicalDamageGainAsFire",
                ModType::Base,
                (values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect,
                Some("Map mod Burning"),
            ));
        }),
        ("Freezing", MapKind::Count, |tier, roll, effect, o| {
            let values = [[50.0, 69.0], [70.0, 89.0], [90.0, 110.0]];
            o.enemy_mods.push(Mod::new(
                "PhysicalDamageGainAsCold",
                ModType::Base,
                (values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect,
                Some("Map mod Freezing"),
            ));
        }),
        ("Shocking", MapKind::Count, |tier, roll, effect, o| {
            let values = [[50.0, 69.0], [70.0, 89.0], [90.0, 110.0]];
            o.enemy_mods.push(Mod::new(
                "PhysicalDamageGainAsLightning",
                ModType::Base,
                (values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect,
                Some("Map mod Shocking"),
            ));
        }),
        ("Profane", MapKind::Count, |tier, roll, effect, o| {
            let values = [[0.0, 0.0], [21.0, 25.0], [31.0, 35.0]];
            o.enemy_mods.push(Mod::new(
                "PhysicalDamageGainAsChaos",
                ModType::Base,
                (values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect,
                Some("Map mod Profane"),
            ));
            o.mods.push(Mod::new(
                "Condition:CanBeWithered",
                ModType::Flag,
                true,
                Some("Map mod Profane"),
            ));
        }),
        ("Fleet", MapKind::Count, |tier, roll, effect, o| {
            let values = [
                [[15.0, 20.0], [20.0, 25.0], [20.0, 25.0]],
                [[20.0, 25.0], [25.0, 35.0], [25.0, 35.0]],
                [[25.0, 30.0], [35.0, 45.0], [35.0, 45.0]],
            ];
            o.enemy_mods.push(Mod::new(
                "Speed",
                ModType::Inc,
                (values[tier][1][0] + (values[tier][1][1] - values[tier][1][0]) * roll / 100.0)
                    * effect,
                Some("Map mod Fleet"),
            ));
        }),
        ("Conflagrating", MapKind::Check, |_, _, _, o| {
            o.enemy_mods.push(Mod::new(
                "IgniteChance",
                ModType::Base,
                100.0,
                Some("Map mod Conflagrating"),
            ));
            o.enemy_mods.push(Mod::new(
                "AllDamageIgnites",
                ModType::Flag,
                true,
                Some("Map mod Conflagrating"),
            ));
        }),
        ("Impaling", MapKind::List, |tier, _, effect, o| {
            let values = [25.0, 40.0, 60.0];
            let mut m = Mod::new(
                "ImpaleChance",
                ModType::Base,
                values[tier] * effect,
                Some("Map mod Impaling"),
            );
            m.flags = MOD_FLAG_ATTACK;
            o.enemy_mods.push(m);
        }),
        ("Empowered", MapKind::List, |tier, _, effect, o| {
            let values = [0.0, 15.0, 20.0];
            o.enemy_mods.push(Mod::new(
                "ElementalAilmentChance",
                ModType::Base,
                values[tier] * effect,
                Some("Map mod Empowered"),
            ));
        }),
        ("Overlord's", MapKind::List, |tier, _, effect, o| {
            let values = [[15.0, 20.0], [20.0, 25.0], [25.0, 30.0]];
            o.enemy_mods.push(
                Mod::new(
                    "Damage",
                    ModType::Inc,
                    values[tier][0] * effect,
                    Some("Map mod Overlord's"),
                )
                .tag(Tag::condition("RareOrUnique")),
            );
            o.enemy_mods.push(
                Mod::new(
                    "Speed",
                    ModType::Inc,
                    values[tier][1] * effect,
                    Some("Map mod Overlord's"),
                )
                .tag(Tag::condition("RareOrUnique")),
            );
        }),
        ("of Congealment", MapKind::Check, |_, _, _, o| {
            o.enemy_mods.push(Mod::new(
                "CannotLeechLifeFromSelf",
                ModType::Flag,
                true,
                Some("Map mod of Congealment"),
            ));
            o.enemy_mods.push(Mod::new(
                "CannotLeechManaFromSelf",
                ModType::Flag,
                true,
                Some("Map mod of Congealment"),
            ));
            o.enemy_mods.push(Mod::new(
                "CannotLeechEnergyShieldFromSelf",
                ModType::Flag,
                true,
                Some("Map mod of Congealment"),
            ));
        }),
        ("of Drought", MapKind::List, |tier, _, effect, o| {
            let values = [30.0, 40.0, 50.0];
            o.mods.push(Mod::new(
                "FlaskChargesGained",
                ModType::Inc,
                -values[tier] * effect,
                Some("Map mod of Drought"),
            ));
        }),
        ("of Exposure", MapKind::Count, |tier, roll, effect, o| {
            let values = [[0.0, 0.0], [5.0, 8.0], [9.0, 12.0]];
            if values[tier][1] != 0.0 {
                let roll =
                    (values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect;
                for stat in [
                    "FireResistMax",
                    "ColdResistMax",
                    "LightningResistMax",
                    "ChaosResistMax",
                ] {
                    o.mods.push(Mod::new(
                        stat,
                        ModType::Base,
                        -roll,
                        Some("Map mod of Exposure"),
                    ));
                }
            }
        }),
        ("of Impotence", MapKind::List, |tier, _, effect, o| {
            let values = [15.0, 20.0, 25.0];
            o.mods.push(Mod::new(
                "AreaOfEffect",
                ModType::More,
                -values[tier] * effect,
                Some("Map mod of Impotence"),
            ));
        }),
        ("of Insulation", MapKind::List, |tier, _, effect, o| {
            let values = [30.0, 50.0, 70.0];
            o.enemy_mods.push(Mod::new(
                "AvoidElementalAilments",
                ModType::Base,
                values[tier] * effect,
                Some("Map mod of Insulation"),
            ));
        }),
        ("of Miring", MapKind::List, |tier, _, effect, o| {
            let values = [[10.0, 30.0], [15.0, 40.0], [20.0, 50.0]];
            o.mods.push(Mod::new(
                "SpellSuppressionEffect",
                ModType::Base,
                -values[tier][0] * effect,
                Some("Map mod of Miring"),
            ));
            o.enemy_mods.push(Mod::new(
                "Accuracy",
                ModType::Inc,
                values[tier][1] * effect,
                Some("Map mod of Miring"),
            ));
        }),
        ("of Rust", MapKind::List, |tier, _, effect, o| {
            let values = [[20.0, 20.0], [30.0, 25.0], [40.0, 30.0]];
            o.mods.push(Mod::new(
                "BlockChance",
                ModType::Inc,
                -values[tier][0] * effect,
                Some("Map mod of Rust"),
            ));
            o.mods.push(Mod::new(
                "Armour",
                ModType::More,
                -values[tier][1] * effect,
                Some("Map mod of Rust"),
            ));
        }),
        ("of Smothering", MapKind::List, |tier, _, effect, o| {
            let values = [20.0, 40.0, 60.0];
            o.mods.push(Mod::new(
                "LifeRecoveryRate",
                ModType::More,
                -values[tier] * effect,
                Some("Map mod of Smothering"),
            ));
            o.mods.push(Mod::new(
                "EnergyShieldRecoveryRate",
                ModType::More,
                -values[tier] * effect,
                Some("Map mod of Smothering"),
            ));
        }),
        ("of Stasis", MapKind::Check, |_, _, _, o| {
            o.mods.push(Mod::new(
                "NoLifeRegen",
                ModType::Flag,
                true,
                Some("Map mod of Stasis"),
            ));
            o.mods.push(Mod::new(
                "NoEnergyShieldRegen",
                ModType::Flag,
                true,
                Some("Map mod of Stasis"),
            ));
            o.mods.push(Mod::new(
                "NoManaRegen",
                ModType::Flag,
                true,
                Some("Map mod of Stasis"),
            ));
        }),
        ("of Toughness", MapKind::Count, |tier, roll, effect, o| {
            let values = [[25.0, 30.0], [31.0, 35.0], [36.0, 40.0]];
            o.enemy_mods.push(Mod::new(
                "SelfCritMultiplier",
                ModType::Inc,
                -(values[tier][0] + (values[tier][1] - values[tier][0]) * roll / 100.0) * effect,
                Some("Map mod of Toughness"),
            ));
        }),
        ("of Fatigue", MapKind::List, |tier, _, effect, o| {
            let values = [20.0, 30.0, 40.0];
            o.mods.push(Mod::new(
                "CooldownRecovery",
                ModType::More,
                -values[tier] * effect,
                Some("Map mod of Fatigue"),
            ));
        }),
        ("of Doubt", MapKind::List, |tier, _, effect, o| {
            let values = [25.0, 40.0, 60.0];
            let aura = o.skill_type("Aura");
            let curse = o.skill_type("AppliesCurse");
            o.mods.push(
                Mod::new(
                    "AuraEffect",
                    ModType::Inc,
                    -values[tier] * effect,
                    Some("Map mod of Doubt"),
                )
                .tag(Tag::skill_type(aura))
                .tag(Tag::skill_type(curse).negated()),
            );
        }),
        ("of Imprecision", MapKind::List, |tier, _, effect, o| {
            let values = [15.0, 20.0, 25.0];
            o.mods.push(Mod::new(
                "Accuracy",
                ModType::More,
                -values[tier] * effect,
                Some("Map mod of Imprecision"),
            ));
        }),
        ("of Venom", MapKind::Check, |_, _, _, o| {
            o.enemy_mods.push(Mod::new(
                "PoisonChance",
                ModType::Base,
                100.0,
                Some("Map mod of Venom"),
            ));
        }),
        ("of Deadliness", MapKind::Count, |tier, roll, effect, o| {
            let values = [
                [[160.0, 200.0], [30.0, 35.0]],
                [[260.0, 300.0], [36.0, 40.0]],
                [[360.0, 400.0], [41.0, 45.0]],
            ];
            o.enemy_mods.push(Mod::new(
                "CritChance",
                ModType::Inc,
                (values[tier][0][0] + (values[tier][0][1] - values[tier][0][0]) * roll / 100.0)
                    * effect,
                Some("Map mod of Deadliness"),
            ));
            o.enemy_mods.push(Mod::new(
                "CritMultiplier",
                ModType::Base,
                (values[tier][1][0] + (values[tier][1][1] - values[tier][1][0]) * roll / 100.0)
                    * effect,
                Some("Map mod of Deadliness"),
            ));
        }),
    ]
}
