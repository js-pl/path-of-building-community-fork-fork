//! Radius jewel functions: what upstream's radius jewels do to the passive
//! nodes in their radius (`jewelOtherFuncs`, `jewelSelfFuncs`,
//! `jewelSelfUnallocFuncs` and `jewelThresholdFuncs` of
//! `Modules/ModParser.lua`). The mod parser gives a jewel's function as a
//! [`Func`] value (a factory and its arguments, or the entry that defines
//! it); [`RadiusFunc::new`] makes it a typed function the setup runs on each
//! node in the radius, then once without a node to finish.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::mods::list::{merge_mod, sum};
use crate::mods::tag::{Names, TagKind};
use crate::mods::{Func, Mod, ModType, ModValue, Sym, Tag, record};
use crate::pat;
use crate::tree::NodeType;
use std::collections::HashMap;

pub type Result<T> = std::result::Result<T, String>;

/// `ModFlag` and `KeywordFlag` bits the functions use
mod flags {
    pub const ATTACK: u32 = 0x1;
    pub const SPELL: u32 = 0x2;
    pub const MELEE: u32 = 0x100;
    pub const AXE: u32 = 0x10000;
    pub const BOW: u32 = 0x20000;
    pub const CLAW: u32 = 0x40000;
    pub const DAGGER: u32 = 0x80000;
    pub const MACE: u32 = 0x100000;
    pub const STAFF: u32 = 0x200000;
    pub const SWORD: u32 = 0x400000;
    pub const WAND: u32 = 0x800000;
    pub const UNARMED: u32 = 0x1000000;
    pub const WEAPON_MELEE: u32 = 0x4000000;
    pub const WEAPON_1H: u32 = 0x8000000;
    pub const WEAPON_2H: u32 = 0x10000000;
    pub const KEYWORD_TRAP: u32 = 0x1000;
    pub const KEYWORD_MINE: u32 = 0x2000;
}

/// A node a function runs on
pub struct RadiusNode<'a> {
    pub id: u32,
    pub typ: NodeType,
    /// The node's own mods (`node.modList`)
    pub mod_list: &'a [Mod],
    pub is_tattoo: bool,
    /// Changed by a timeless jewel (`conqueredBy`)
    pub conquered: bool,
}

/// A function's working data (`rad.data`): tallies by name, the mods it
/// collects, the data of the functions it runs, and its source
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RadiusData {
    pub mod_source: String,
    pub values: HashMap<String, f64>,
    pub mod_list: Option<Vec<Mod>>,
    pub func_data: Vec<RadiusData>,
}

impl RadiusData {
    fn add(&mut self, key: &str, v: f64) {
        *self.values.entry(key.to_string()).or_insert(0.0) += v;
    }

    fn get(&self, key: &str) -> Option<f64> {
        self.values.get(key).copied()
    }
}

/// The nodes a function runs on
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RadiusType {
    /// Before the passive effect scaling, on every node in the radius
    Other,
    /// On the allocated nodes
    Self_,
    /// On the unallocated nodes
    SelfUnalloc,
    /// On every node
    Threshold,
}

impl RadiusType {
    pub fn new(name: &str) -> Option<RadiusType> {
        Some(match name {
            "Other" => RadiusType::Other,
            "Self" => RadiusType::Self_,
            "SelfUnalloc" => RadiusType::SelfUnalloc,
            "Threshold" => RadiusType::Threshold,
            _ => return None,
        })
    }
}

/// Which nodes a node filter keeps
#[derive(Clone, Debug, PartialEq)]
pub enum NodeFilter {
    NotKeystoneOrClassStart,
    NotKeystoneSocketOrClassStart,
    Notable,
    Normal,
    Tattoo,
}

impl NodeFilter {
    fn keeps(&self, n: &RadiusNode) -> bool {
        use NodeType::*;
        match self {
            NodeFilter::NotKeystoneOrClassStart => !matches!(n.typ, Keystone | ClassStart),
            NodeFilter::NotKeystoneSocketOrClassStart => {
                !matches!(n.typ, Keystone | Socket | ClassStart)
            }
            NodeFilter::Notable => n.typ == Notable,
            NodeFilter::Normal => n.typ == Normal,
            NodeFilter::Tattoo => n.is_tattoo,
        }
    }
}

/// A mod a node filter's function adds, sourced from the jewel
#[derive(Clone, Debug, PartialEq)]
pub struct NodeMod {
    name: String,
    typ: ModType,
    value: f64,
    flags: u32,
    keyword_flags: u32,
}

/// What notables are transformed to grant
#[derive(Clone, Debug, PartialEq)]
pub enum Transform {
    /// `(%d+)% increased mana cost of skills and (%d+)% increased spell damage`
    ManaCostAndSpellDamage(f64, f64),
    /// `minions take (%d+)% increased damage`
    MinionDamageTaken(f64),
    /// `minions have (%d+)% reduced movement speed`
    MinionMovementSpeed(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RadiusFunc {
    /// `getSimpleConv(srcList, dst, type, remove, factor, srcType)`
    SimpleConv {
        src: Vec<Sym>,
        dst: Sym,
        typ: ModType,
        remove: bool,
        factor: Option<f64>,
        src_type: Option<ModType>,
    },
    /// `getPerStat(dst, modType, flags, stat, factor)`
    PerStat {
        dst: Sym,
        typ: ModType,
        flags: u32,
        stat: Sym,
        factor: f64,
    },
    /// `getThreshold(attrib, ...)`: the mod, with 40 of the attributes
    Threshold {
        attribs: Vec<Sym>,
        attrib_list: bool,
        base: Box<Mod>,
    },
    /// Several thresholds of one line, each with its own data
    Multi(Vec<RadiusFunc>),
    /// A function of `jewelOtherFuncs`, which leaves timeless nodes alone
    Other(Box<RadiusFunc>),
    /// Mods for each node a filter keeps
    NodeMods {
        filter: NodeFilter,
        mods: Vec<NodeMod>,
    },
    /// Notables in the radius are transformed to grant other mods
    NotableTransform(Transform),
    /// "Melee and Melee Weapon Type modifiers in Radius are Transformed to
    /// Bow Modifiers"
    MeleeToBow,
    /// "Dexterity and Intelligence from passives in Radius count towards
    /// Strength Melee Damage bonus"
    DexIntToMeleeBonus,
    /// "Grants all bonuses of Unallocated Small (Notable) Passive Skills in
    /// Radius"
    GrantUnallocated(NodeType),
    /// The default function: tallies the attributes in the radius
    Tally,
}

fn arg(f: &Func, i: usize) -> Option<&ModValue> {
    f.args.get(i).and_then(Option::as_ref)
}

fn arg_str(f: &Func, i: usize) -> Result<String> {
    match arg(f, i) {
        Some(ModValue::Str(s)) => Ok(s.to_string()),
        v => Err(format!("{}: argument {} is {v:?}", f.name, i + 1)),
    }
}

fn arg_num(f: &Func, i: usize) -> Result<f64> {
    match arg(f, i) {
        Some(ModValue::Number(n)) => Ok(*n),
        v => Err(format!("{}: argument {} is {v:?}", f.name, i + 1)),
    }
}

fn strings(v: Option<&ModValue>) -> Result<Vec<Sym>> {
    match v {
        Some(ModValue::List(l)) => l
            .iter()
            .map(|x| match x {
                ModValue::Str(s) => Ok(Sym::new(s)),
                x => Err(format!("expected a string, got {x:?}")),
            })
            .collect(),
        Some(ModValue::Str(s)) => Ok(vec![Sym::new(s)]),
        v => Err(format!("expected a list, got {v:?}")),
    }
}

/// `tonumber` of a capture
fn cap_num(caps: &[String], i: usize) -> Result<f64> {
    caps.get(i)
        .and_then(|c| pat::to_number(c))
        .ok_or_else(|| format!("capture {} is not a number", i + 1))
}

fn first_to_upper(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn simple_conv(
    src: &[&str],
    dst: &str,
    typ: ModType,
    remove: bool,
    factor: Option<f64>,
    src_type: Option<ModType>,
) -> RadiusFunc {
    RadiusFunc::SimpleConv {
        src: src.iter().map(|s| Sym::new(s)).collect(),
        dst: Sym::new(dst),
        typ,
        remove,
        factor,
        src_type,
    }
}

fn node_mods(filter: NodeFilter, mods: &[(&str, ModType, f64, u32, u32)]) -> RadiusFunc {
    RadiusFunc::NodeMods {
        filter,
        mods: mods
            .iter()
            .map(|(name, typ, value, flags, keyword_flags)| NodeMod {
                name: name.to_string(),
                typ: *typ,
                value: *value,
                flags: *flags,
                keyword_flags: *keyword_flags,
            })
            .collect(),
    }
}

/// `getThreshold(attrib, name, modType, value, ...)` for the lines with
/// captures, built as the mod parser's `mod` would
fn threshold(attrib: &str, m: Mod) -> RadiusFunc {
    RadiusFunc::Threshold {
        attribs: vec![Sym::new(attrib)],
        attrib_list: false,
        base: Box::new(Mod {
            source: Some("".into()),
            ..m
        }),
    }
}

impl RadiusFunc {
    /// The typed function of a jewel function value
    pub fn new(v: &ModValue) -> Result<RadiusFunc> {
        let ModValue::Func(f) = v else {
            return Err(format!("jewel function {v:?}"));
        };
        let typ = |i: usize| -> Result<ModType> { Ok(ModType::new(&arg_str(f, i)?)) };
        Ok(match &*f.name {
            "getSimpleConv" => RadiusFunc::SimpleConv {
                src: strings(arg(f, 0))?,
                dst: Sym::new(&arg_str(f, 1)?),
                typ: typ(2)?,
                remove: arg(f, 3).is_some_and(ModValue::truthy),
                factor: arg(f, 4).and_then(ModValue::as_number),
                src_type: match arg(f, 5) {
                    Some(ModValue::Str(s)) => Some(ModType::new(s)),
                    _ => None,
                },
            },
            "getPerStat" => RadiusFunc::PerStat {
                dst: Sym::new(&arg_str(f, 0)?),
                typ: typ(1)?,
                flags: arg_num(f, 2)? as u32,
                stat: Sym::new(&arg_str(f, 3)?),
                factor: arg_num(f, 4)?,
            },
            "getThreshold" => {
                let attrib_list = matches!(arg(f, 0), Some(ModValue::List(_)));
                let base = match arg(f, 1) {
                    Some(ModValue::Mod(m)) => m.clone(),
                    v => return Err(format!("getThreshold mod {v:?}")),
                };
                RadiusFunc::Threshold {
                    attribs: strings(arg(f, 0))?,
                    attrib_list,
                    base,
                }
            }
            "jewelOtherWrapper" => RadiusFunc::Other(Box::new(RadiusFunc::new(
                arg(f, 0).ok_or("jewelOtherWrapper without a function")?,
            )?)),
            "jewelThresholdMulti" => match arg(f, 0) {
                Some(ModValue::List(l)) => {
                    RadiusFunc::Multi(l.iter().map(RadiusFunc::new).collect::<Result<_>>()?)
                }
                v => return Err(format!("jewelThresholdMulti {v:?}")),
            },
            "jewelFunc" => {
                let pattern = arg_str(f, 0)?;
                let caps: Vec<String> = (1..f.args.len())
                    .filter_map(|i| match arg(f, i) {
                        Some(ModValue::Str(s)) => Some(s.to_string()),
                        Some(ModValue::Number(n)) => Some(pat::num(*n)),
                        _ => None,
                    })
                    .collect();
                RadiusFunc::with_captures(&pattern, &caps)?
            }
            name => RadiusFunc::by_entry(name)?,
        })
    }

    /// The functions of the entries with captures, by their lower case
    /// pattern
    fn with_captures(pattern: &str, caps: &[String]) -> Result<RadiusFunc> {
        use ModType::*;
        let n = |i: usize| cap_num(caps, i);
        let other = |f: RadiusFunc| RadiusFunc::Other(Box::new(f));
        let pass_all = NodeFilter::NotKeystoneSocketOrClassStart;
        let res =
            |src: &str, dst: &str, typ: ModType, src_type: Option<ModType>| -> Result<RadiusFunc> {
                Ok(other(simple_conv(
                    &[src, "ElementalResist"],
                    dst,
                    typ,
                    false,
                    Some(n(0)? / 100.0),
                    src_type,
                )))
            };
        Ok(match pattern {
            "increases and reductions to energy shield in radius are transformed to apply to armour at (%d+)%% of their value" => {
                other(simple_conv(
                    &["EnergyShield"],
                    "Armour",
                    Inc,
                    true,
                    Some(n(0)? / 100.0),
                    None,
                ))
            }
            "increases and reductions to life in radius are transformed to apply to mana at (%d+)%% of their value" => {
                other(simple_conv(
                    &["Life"],
                    "Mana",
                    Inc,
                    true,
                    Some(n(0)? / 100.0),
                    None,
                ))
            }
            "passives granting lightning resistance or all elemental resistances in radius also grant chance to block spells? ?d?a?m?a?g?e? at (%d+)%% of its value" => {
                res("LightningResist", "SpellBlockChance", Base, None)?
            }
            "passives granting lightning resistance or all elemental resistances in radius also grant increased maximum energy shield at (%d+)%% of its value" => {
                res("LightningResist", "EnergyShield", Inc, Some(Base))?
            }
            "passives granting lightning resistance or all elemental resistances in radius also grant lightning damage converted to chaos damage at (%d+)%% of its value" => {
                res(
                    "LightningResist",
                    "LightningDamageConvertToChaos",
                    Base,
                    None,
                )?
            }
            "passives granting cold resistance or all elemental resistances in radius also grant chance to dodge attacks? ?h?i?t?s? at (%d+)%% of its value" => {
                res("ColdResist", "AttackDodgeChance", Base, None)?
            }
            "passives granting cold resistance or all elemental resistances in radius also grant chance to suppress spell damage at (%d+)%% of its value" => {
                res("ColdResist", "SpellSuppressionChance", Base, None)?
            }
            "passives granting cold resistance or all elemental resistances in radius also grant increased maximum mana at (%d+)%% of its value" => {
                res("ColdResist", "Mana", Inc, Some(Base))?
            }
            "passives granting cold resistance or all elemental resistances in radius also grant cold damage converted to chaos damage at (%d+)%% of its value" => {
                res("ColdResist", "ColdDamageConvertToChaos", Base, None)?
            }
            "passives granting fire resistance or all elemental resistances in radius also grant chance to block attack damage at (%d+)%% of its value"
            | "passives granting fire resistance or all elemental resistances in radius also grant chance to block at (%d+)%% of its value" => {
                res("FireResist", "BlockChance", Base, None)?
            }
            "passives granting fire resistance or all elemental resistances in radius also grant increased maximum life at (%d+)%% of its value" => {
                res("FireResist", "Life", Inc, Some(Base))?
            }
            "passives granting fire resistance or all elemental resistances in radius also grant fire damage converted to chaos damage at (%d+)%% of its value" => {
                res("FireResist", "FireDamageConvertToChaos", Base, None)?
            }
            "(%d+)%% increased effect of non%-keystone passive skills in radius" => {
                other(node_mods(
                    NodeFilter::NotKeystoneOrClassStart,
                    &[("PassiveSkillEffect", Inc, n(0)?, 0, 0)],
                ))
            }
            "(%d+)%% increased effect of tattoos in radius" => other(node_mods(
                NodeFilter::Tattoo,
                &[("PassiveSkillEffect", Inc, n(0)?, 0, 0)],
            )),
            "passive skills in radius also grant: traps and mines deal (%d+) to (%d+) added physical damage" =>
            {
                let kw = flags::KEYWORD_TRAP | flags::KEYWORD_MINE;
                other(node_mods(
                    NodeFilter::NotKeystoneOrClassStart,
                    &[
                        ("PhysicalMin", Base, n(0)?, 0, kw),
                        ("PhysicalMax", Base, n(1)?, 0, kw),
                    ],
                ))
            }
            "passive skills in radius also grant: (%d+)%% increased unarmed attack speed with melee skills" => {
                other(node_mods(
                    NodeFilter::NotKeystoneOrClassStart,
                    &[(
                        "Speed",
                        Inc,
                        n(0)?,
                        flags::UNARMED | flags::ATTACK | flags::MELEE,
                        0,
                    )],
                ))
            }
            "passive skills in radius also grant (%d+)%% increased global critical strike chance" => {
                other(node_mods(pass_all, &[("CritChance", Inc, n(0)?, 0, 0)]))
            }
            "passive skills in radius also grant %+(%d+) to maximum life" => {
                other(node_mods(pass_all, &[("Life", Base, n(0)?, 0, 0)]))
            }
            "passive skills in radius also grant %+(%d+) to maximum mana" => {
                other(node_mods(pass_all, &[("Mana", Base, n(0)?, 0, 0)]))
            }
            "passive skills in radius also grant (%d+)%% increased energy shield" => {
                other(node_mods(pass_all, &[("EnergyShield", Inc, n(0)?, 0, 0)]))
            }
            "passive skills in radius also grant (%d+)%% increased armour" => {
                other(node_mods(pass_all, &[("Armour", Inc, n(0)?, 0, 0)]))
            }
            "passive skills in radius also grant (%d+)%% increased evasion rating" => {
                other(node_mods(pass_all, &[("Evasion", Inc, n(0)?, 0, 0)]))
            }
            "passive skills in radius also grant %+(%d+) to all attributes" => {
                let v = n(0)?;
                other(node_mods(
                    pass_all,
                    &[
                        ("Str", Base, v, 0, 0),
                        ("Dex", Base, v, 0, 0),
                        ("Int", Base, v, 0, 0),
                        ("All", Base, v, 0, 0),
                    ],
                ))
            }
            "passive skills in radius also grant %+(%d+)%% to chaos resistance" => {
                other(node_mods(pass_all, &[("ChaosResist", Base, n(0)?, 0, 0)]))
            }
            "passive skills in radius also grant (%d+)%% increased (%w+) damage" => {
                let name = format!(
                    "{}Damage",
                    first_to_upper(caps.get(1).map_or("", String::as_str))
                );
                other(RadiusFunc::NodeMods {
                    filter: pass_all,
                    mods: vec![NodeMod {
                        name,
                        typ: Inc,
                        value: n(0)?,
                        flags: 0,
                        keyword_flags: 0,
                    }],
                })
            }
            "notable passive skills in radius are transformed to instead grant: (%d+)%% increased mana cost of skills and (%d+)%% increased spell damage" => {
                other(RadiusFunc::NotableTransform(
                    Transform::ManaCostAndSpellDamage(n(0)?, n(1)?),
                ))
            }
            "notable passive skills in radius are transformed to instead grant: minions take (%d+)%% increased damage" => {
                other(RadiusFunc::NotableTransform(Transform::MinionDamageTaken(
                    n(0)?,
                )))
            }
            "notable passive skills in radius are transformed to instead grant: minions have (%d+)%% reduced movement speed" => {
                other(RadiusFunc::NotableTransform(
                    Transform::MinionMovementSpeed(n(0)?),
                ))
            }
            "with at least 40 dexterity in radius, dual strike has (%d+)%% increased attack speed while wielding a claw" => {
                threshold(
                    "Dex",
                    Mod::new("Speed", Inc, n(0)?, None)
                        .tag(Tag::skill_name("Dual Strike").include_transfigured())
                        .tag(Tag::condition("UsingClaw")),
                )
            }
            "with at least 40 dexterity in radius, dual strike has %+(%d+)%% to critical strike multiplier while wielding a dagger" => {
                threshold(
                    "Dex",
                    Mod::new("CritMultiplier", Base, n(0)?, None)
                        .tag(Tag::skill_name("Dual Strike").include_transfigured())
                        .tag(Tag::condition("UsingDagger")),
                )
            }
            "with at least 40 dexterity in radius, dual strike has (%d+)%% increased accuracy rating while wielding a sword" => {
                threshold(
                    "Dex",
                    Mod::new("Accuracy", Inc, n(0)?, None)
                        .tag(Tag::skill_name("Dual Strike").include_transfigured())
                        .tag(Tag::condition("UsingSword")),
                )
            }
            "with at least 40 intelligence in radius, fireball has %+(%d+)%% chance to inflict scorch" => {
                threshold(
                    "Int",
                    Mod::new("EnemyScorchChance", Base, n(0)?, None)
                        .tag(Tag::skill_name("Fireball")),
                )
            }
            "with at least 40 intelligence in radius, (%d+)%% of damage taken recouped as mana if you've warcried recently" => {
                threshold(
                    "Int",
                    Mod::new("ManaRecoup", Base, n(0)?, None)
                        .tag(Tag::condition("UsedWarcryRecently")),
                )
            }
            "with at least 40 intelligence in radius, fireball projectiles gain radius as they travel farther, up to %+(%d+) radius" => {
                threshold(
                    "Int",
                    Mod::new("AreaOfEffect", Base, n(0)?, None).tag(distance_ramp()),
                )
            }
            "with at least 40 intelligence in radius, projectiles gain radius as they travel farther, up to a maximum of %+([%d%.]+) metres? to radius" => {
                threshold(
                    "Int",
                    Mod::new("AreaOfEffect", Base, n(0)? * 10.0, None).tag(distance_ramp()),
                )
            }
            p => return Err(format!("jewel function with captures not ported: {p:?}")),
        })
    }

    /// The functions defined in the tables, by their entry
    fn by_entry(name: &str) -> Result<RadiusFunc> {
        Ok(match name {
            "jewelOtherFuncs[\"Melee and Melee Weapon Type modifiers in Radius are Transformed to Bow Modifiers\"]" => {
                RadiusFunc::MeleeToBow
            }
            "jewelOtherFuncs[\"Notable Passive Skills in Radius grant nothing\"]" => node_mods(
                NodeFilter::Notable,
                &[("PassiveSkillHasNoEffect", ModType::Flag, f64::NAN, 0, 0)],
            ),
            "jewelOtherFuncs[\"Allocated Small Passive Skills in Radius grant nothing\"]" => {
                node_mods(
                    NodeFilter::Normal,
                    &[(
                        "AllocatedPassiveSkillHasNoEffect",
                        ModType::Flag,
                        f64::NAN,
                        0,
                        0,
                    )],
                )
            }
            "jewelOtherFuncs[\"Allocated Notable Passive Skills in Radius grant nothing\"]" => {
                node_mods(
                    NodeFilter::Notable,
                    &[(
                        "AllocatedPassiveSkillHasNoEffect",
                        ModType::Flag,
                        f64::NAN,
                        0,
                        0,
                    )],
                )
            }
            "jewelSelfFuncs[\"Dexterity and Intelligence from passives in Radius count towards Strength Melee Damage bonus\"]" => {
                RadiusFunc::DexIntToMeleeBonus
            }
            "jewelSelfUnallocFuncs[\"Grants all bonuses of Unallocated Small Passive Skills in Radius\"]" => {
                RadiusFunc::GrantUnallocated(NodeType::Normal)
            }
            "jewelSelfUnallocFuncs[\"Grants all bonuses of Unallocated Notable Passive Skills in Radius\"]" => {
                RadiusFunc::GrantUnallocated(NodeType::Notable)
            }
            n => return Err(format!("jewel function not ported: {n}")),
        })
    }

    /// Runs the function on a node, or without one to finish
    pub fn run(
        &self,
        node: Option<&RadiusNode>,
        out: &mut Vec<Mod>,
        data: &mut RadiusData,
    ) -> Result<()> {
        let source = data.mod_source.clone();
        match self {
            RadiusFunc::SimpleConv {
                src,
                dst,
                typ,
                remove,
                factor,
                src_type,
            } => {
                let Some(node) = node else { return Ok(()) };
                let attributes = ["Dex", "Int", "Str"];
                for s in src {
                    for m in node.mod_list {
                        let type_ok = match src_type {
                            Some(t) => m.typ == *t,
                            None => m.typ == *typ,
                        };
                        // Tattoos' attributes are not converted
                        if m.name == *s
                            && type_ok
                            && !(node.is_tattoo && attributes.contains(&s.as_str()))
                        {
                            let value = m.value.as_number().ok_or("conversion of a non-number")?;
                            // `MergeNewMod(name, type, value, mod.source,
                            // mod.flags, mod.keywordFlags, unpack(mod))`
                            let derived = |name: Sym, v: f64| Mod {
                                name,
                                typ: *typ,
                                value: ModValue::Number(v),
                                source_slot: None,
                                unreached_tags: vec![],
                                ..m.clone()
                            };
                            if *remove {
                                merge_mod(out, derived(*s, -value))?;
                            }
                            let v = match factor {
                                Some(f) => (value * f).floor(),
                                None => value,
                            };
                            merge_mod(out, derived(*dst, v))?;
                        }
                    }
                }
            }
            RadiusFunc::PerStat {
                dst,
                typ,
                flags,
                stat,
                factor,
            } => match node {
                Some(_) => {
                    let v = sum(out, ModType::Base, stat.as_str())?;
                    data.add(stat.as_str(), v);
                }
                None => {
                    // A missing tally is nil, which is not 0
                    if data.get(stat.as_str()) != Some(0.0) {
                        let v = (data.get(stat.as_str()).unwrap_or(0.0) * factor).floor();
                        out.push(Mod::new(dst.as_str(), *typ, v, Some(&source)).flags(*flags, 0));
                    }
                }
            },
            RadiusFunc::Threshold { attribs, base, .. } => match node {
                Some(_) => {
                    for a in attribs {
                        let v = sum(out, ModType::Base, a.as_str())?;
                        data.add(a.as_str(), v);
                        data.add("total", v);
                    }
                }
                None => {
                    if data.get("total").unwrap_or(0.0) >= 40.0 {
                        // Upstream sets the source of the value's mod on the
                        // closure's own table after copying, so a copy has
                        // it from the calculations before; here it always
                        // has it
                        let mut m = (**base).clone();
                        m.source = Some(source.as_str().into());
                        if let ModValue::Record(r) = &mut m.value
                            && let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod"))
                        {
                            inner.source = Some(source.as_str().into());
                        }
                        out.push(m);
                    }
                }
            },
            RadiusFunc::Multi(funcs) => {
                for (i, f) in funcs.iter().enumerate() {
                    if data.func_data.len() <= i {
                        data.func_data.resize(i + 1, RadiusData::default());
                    }
                    data.func_data[i].mod_source = source.clone();
                    let mut d = std::mem::take(&mut data.func_data[i]);
                    let r = f.run(node, out, &mut d);
                    data.func_data[i] = d;
                    r?;
                }
            }
            RadiusFunc::Other(inner) => {
                if node.is_some_and(|n| n.conquered) {
                    return Ok(());
                }
                inner.run(node, out, data)?;
            }
            RadiusFunc::NodeMods { filter, mods } => {
                if let Some(n) = node
                    && filter.keeps(n)
                {
                    for m in mods {
                        let value = if m.typ == ModType::Flag {
                            ModValue::Bool(true)
                        } else {
                            ModValue::Number(m.value)
                        };
                        out.push(
                            Mod::new(&m.name, m.typ, value, Some(&source))
                                .flags(m.flags, m.keyword_flags),
                        );
                    }
                }
            }
            RadiusFunc::NotableTransform(t) => {
                if node.is_some_and(|n| n.typ == NodeType::Notable) {
                    let src = Some(source.as_str());
                    out.push(Mod::new(
                        "PassiveSkillHasOtherEffect",
                        ModType::Flag,
                        true,
                        src,
                    ));
                    // The minion mod itself has no source, as upstream
                    // builds it with `mod(name, type, value)`
                    let minion = |m: Mod| {
                        Mod::new(
                            "MinionModifier",
                            ModType::List,
                            record([("mod", m.into())]),
                            None,
                        )
                    };
                    let mods = match t {
                        Transform::ManaCostAndSpellDamage(cost, damage) => vec![
                            Mod::new("ManaCost", ModType::Inc, *cost, src),
                            Mod::new("Damage", ModType::Inc, *damage, src).flags(flags::SPELL, 0),
                        ],
                        Transform::MinionDamageTaken(v) => {
                            vec![minion(Mod::new("DamageTaken", ModType::Inc, *v, src))]
                        }
                        Transform::MinionMovementSpeed(v) => {
                            vec![minion(Mod::new("MovementSpeed", ModType::Inc, -*v, src))]
                        }
                    };
                    for m in mods {
                        out.push(Mod::new(
                            "NodeModifier",
                            ModType::List,
                            record([("mod", m.into())]),
                            src,
                        ));
                    }
                }
            }
            RadiusFunc::MeleeToBow => {
                let Some(node) = node else { return Ok(()) };
                use flags::*;
                let mask1 = AXE | CLAW | DAGGER | MACE | STAFF | SWORD | MELEE;
                let mask2 = WEAPON_1H | WEAPON_MELEE;
                let mask3 = WEAPON_2H | WEAPON_MELEE;
                let using = [
                    "UsingAxe",
                    "UsingClaw",
                    "UsingDagger",
                    "UsingMace",
                    "UsingStaff",
                    "UsingSword",
                    "UsingMeleeWeapon",
                ];
                let _ = WAND;
                for m in node.mod_list {
                    let value = m.value.as_number();
                    let with = |m: &Mod, v: f64, flags: u32, tags: Vec<Tag>| Mod {
                        value: ModValue::Number(v),
                        flags,
                        tags,
                        source_slot: None,
                        unreached_tags: vec![],
                        ..m.clone()
                    };
                    if m.flags & mask1 != 0 || m.flags & mask2 == mask2 || m.flags & mask3 == mask3
                    {
                        let v = value.ok_or("transform of a non-number")?;
                        merge_mod(out, with(m, -v, m.flags, m.tags.clone()))?;
                        let flags = (m.flags & !(mask1 | mask2 | mask3)) | BOW;
                        merge_mod(out, with(m, v, flags, m.tags.clone()))?;
                    } else if !m.tags.is_empty() {
                        let is_using = |t: &Tag| matches!(&t.kind, TagKind::Condition { vars: Names::One(v), .. } if using.contains(&v.as_str()));
                        if m.tags.iter().any(is_using) {
                            let mut tags = m.tags.clone();
                            if let Some(t) = tags.iter_mut().find(|t| is_using(t))
                                && let TagKind::Condition { vars, .. } = &mut t.kind
                            {
                                *vars = Names::One(Sym::new("UsingBow"));
                            }
                            let v = value.ok_or("transform of a non-number")?;
                            merge_mod(out, with(m, -v, m.flags, m.tags.clone()))?;
                            merge_mod(out, with(m, v, m.flags, tags))?;
                        }
                    }
                }
            }
            RadiusFunc::DexIntToMeleeBonus => match node {
                Some(n) => {
                    let dex = sum(n.mod_list, ModType::Base, "Dex")?;
                    let int = sum(n.mod_list, ModType::Base, "Int")?;
                    data.add("Dex", dex);
                    data.add("Int", int);
                }
                None => {
                    if data.get("Dex").is_some() || data.get("Int").is_some() {
                        let v = data.get("Dex").unwrap_or(0.0) + data.get("Int").unwrap_or(0.0);
                        out.push(Mod::new(
                            "DexIntToMeleeBonus",
                            ModType::Base,
                            v,
                            Some(&source),
                        ));
                    }
                }
            },
            RadiusFunc::GrantUnallocated(typ) => match node {
                Some(n) => {
                    if n.typ == *typ {
                        // Not the connections, which the jewel does not allocate
                        let list = data.mod_list.get_or_insert_with(Vec::new);
                        list.extend(
                            out.iter()
                                .filter(|m| !m.name.as_str().starts_with("Condition:ConnectedTo"))
                                .cloned(),
                        );
                    }
                }
                None => {
                    if let Some(list) = &data.mod_list {
                        out.extend(list.iter().cloned());
                    }
                }
            },
            RadiusFunc::Tally => {
                if let Some(_n) = node {
                    for stat in ["Str", "Dex", "Int"] {
                        let v = sum(out, ModType::Base, stat)?;
                        data.add(stat, v);
                    }
                }
            }
        }
        Ok(())
    }
}

/// `{ type = "DistanceRamp", ramp = {{0,0},{50,1}} }`
fn distance_ramp() -> Tag {
    Tag {
        kind: TagKind::DistanceRamp(vec![(0.0, 0.0), (50.0, 1.0)]),
        global_limit: None,
    }
}
