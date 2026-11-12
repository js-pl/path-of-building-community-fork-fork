//! `Modules/BuildDisplayStats.lua`: the sidebar's stats, their conditions,
//! warnings and suffixes, as plain Rust.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use engine::actor::OutVal;
use engine::calcs::Output;
use engine::mods::Sym;

/// An actor's output as the stats' functions read it (`o`), with
/// `data.misc.DotDpsCap`
pub struct Ctx<'a> {
    pub o: &'a Output,
    pub cap: f64,
}

impl Ctx<'_> {
    pub fn get(&self, k: &str) -> Option<&OutVal> {
        self.o.get(&Sym::new(k))
    }

    /// `o.k`, a number or nil
    pub fn n(&self, k: &str) -> Option<f64> {
        self.get(k).and_then(OutVal::num)
    }

    /// `o.k or 0`
    fn z(&self, k: &str) -> f64 {
        self.n(k).unwrap_or(0.0)
    }

    /// `o.k` as a condition
    pub fn t(&self, k: &str) -> bool {
        self.get(k).is_some_and(OutVal::truthy)
    }

    /// `v ~= o.k`
    fn ne(&self, v: f64, k: &str) -> bool {
        self.n(k) != Some(v)
    }

    /// `v > o.k`, false where Lua would fail on nil
    fn gt(&self, v: f64, k: &str) -> bool {
        self.n(k).is_some_and(|x| v > x)
    }

    fn lt(&self, v: f64, k: &str) -> bool {
        self.n(k).is_some_and(|x| v < x)
    }

    /// `o.a == o.b`
    fn same(&self, a: &str, b: &str) -> bool {
        self.n(a) == self.n(b)
    }

    /// Where a requirement comes from: `o.ReqStrItem.sourceItem.name`, ...
    fn req_source(&self, k: &str) -> String {
        let Some(OutVal::Table(t)) = self.get(k) else {
            return String::new();
        };
        let name = |key: &str| match t.get(key) {
            Some(OutVal::Item(n) | OutVal::Gem(n) | OutVal::Str(n)) => n.to_string(),
            _ => String::new(),
        };
        match t.get("source") {
            Some(OutVal::Str(s)) if &**s == "Item" => name("sourceItem"),
            Some(OutVal::Str(s)) if &**s == "Gem" => name("sourceGem"),
            _ => String::new(),
        }
    }
}

pub type Cond = fn(f64, &Ctx) -> bool;
pub type Warn = fn(f64, &Ctx) -> Option<String>;

#[derive(Clone, Copy)]
pub enum Suffix {
    None,
    Text(&'static str),
    Func(Warn),
}

/// One row of the sidebar (`DisplayStat`); without `stat` it is a gap
#[derive(Clone, Copy)]
pub struct Stat {
    pub stat: Option<&'static str>,
    pub child: Option<&'static str>,
    pub label: &'static str,
    pub fmt: &'static str,
    pub color: Option<&'static str>,
    pub flag: Option<&'static str>,
    pub not_flag: Option<&'static str>,
    pub cond: Option<Cond>,
    pub warn: Option<Warn>,
    pub warn_color: bool,
    pub suffix: Suffix,
    pub suffix_cond: Option<Cond>,
    pub over_cap: Option<&'static str>,
    pub over_cap_cond: Option<Cond>,
    pub pc: bool,
    pub modifier: bool,
    pub hide: bool,
    pub chaos_inoc: bool,
}

const GAP: Stat = Stat {
    stat: None,
    child: None,
    label: "",
    fmt: "",
    color: None,
    flag: None,
    not_flag: None,
    cond: None,
    warn: None,
    warn_color: false,
    suffix: Suffix::None,
    suffix_cond: None,
    over_cap: None,
    over_cap_cond: None,
    pc: false,
    modifier: false,
    hide: false,
    chaos_inoc: false,
};

/// The format of the Full DPS rows
pub const SKILL_DPS: Stat = s("SkillDPS", "", ".1f");

const fn s(stat: &'static str, label: &'static str, fmt: &'static str) -> Stat {
    Stat {
        stat: Some(stat),
        label,
        fmt,
        ..GAP
    }
}

impl Stat {
    const fn color(self, c: &'static str) -> Stat {
        Stat {
            color: Some(c),
            ..self
        }
    }
    const fn flag(self, f: &'static str) -> Stat {
        Stat {
            flag: Some(f),
            ..self
        }
    }
    const fn not_flag(self, f: &'static str) -> Stat {
        Stat {
            not_flag: Some(f),
            ..self
        }
    }
    const fn cond(self, f: Cond) -> Stat {
        Stat {
            cond: Some(f),
            ..self
        }
    }
    const fn warn(self, f: Warn) -> Stat {
        Stat {
            warn: Some(f),
            ..self
        }
    }
    const fn child(self, c: &'static str) -> Stat {
        Stat {
            child: Some(c),
            ..self
        }
    }
    const fn suffix(self, x: Suffix) -> Stat {
        Stat { suffix: x, ..self }
    }
    const fn suffix_cond(self, f: Cond) -> Stat {
        Stat {
            suffix_cond: Some(f),
            ..self
        }
    }
    const fn over_cap(self, k: &'static str) -> Stat {
        Stat {
            over_cap: Some(k),
            ..self
        }
    }
}

// colorCodes of `Data/Global.lua`
pub const NORMAL: &str = "^xC8C8C8";
pub const MAGIC: &str = "^x8888FF";
pub const RARE: &str = "^xFFFF77";
pub const UNIQUE: &str = "^xAF6025";
pub const RELIC: &str = "^x60C060";
pub const CURRENCY: &str = "^xAA9E82";
pub const CUSTOM: &str = "^x5CF0BB";
pub const WARNING: &str = "^xFF9922";
pub const NEGATIVE: &str = "^xDD0022";
const FIRE: &str = "^xB97123";
const COLD: &str = "^x3F6DB3";
const LIGHTNING: &str = "^xADAA47";
const CHAOS: &str = "^xD02090";
const SOURCE: &str = "^x88FFFF";
const POSITIVE: &str = "^x33FF77";
const MARAUDER: &str = "^xE05030";
const RANGER: &str = "^x70FF70";
const WITCH: &str = "^x7070FF";
const STRENGTH: &str = MARAUDER;
const DEXTERITY: &str = RANGER;
const INTELLIGENCE: &str = WITCH;
const LIFE: &str = MARAUDER;
const MANA: &str = WITCH;
const ES: &str = SOURCE;
const WARD: &str = RARE;
const EVASION: &str = POSITIVE;
const RAGE: &str = WARNING;
const PHYS: &str = NORMAL;

/// The colour codes by name, for tests against upstream's
pub const COLOR_CODES: &[(&str, &str)] = &[
    ("NORMAL", NORMAL),
    ("MAGIC", MAGIC),
    ("RARE", RARE),
    ("UNIQUE", UNIQUE),
    ("RELIC", RELIC),
    ("CURRENCY", CURRENCY),
    ("CUSTOM", CUSTOM),
    ("WARNING", WARNING),
    ("NEGATIVE", NEGATIVE),
    ("FIRE", FIRE),
    ("COLD", COLD),
    ("LIGHTNING", LIGHTNING),
    ("CHAOS", CHAOS),
    ("STRENGTH", STRENGTH),
    ("DEXTERITY", DEXTERITY),
    ("INTELLIGENCE", INTELLIGENCE),
    ("LIFE", LIFE),
    ("MANA", MANA),
    ("ES", ES),
    ("WARD", WARD),
    ("EVASION", EVASION),
    ("RAGE", RAGE),
    ("PHYS", PHYS),
];

/// The colour of an item rarity (`colorCodes[rarity]`)
pub fn rarity_color(rarity: &str) -> &'static str {
    match rarity {
        "MAGIC" => MAGIC,
        "RARE" => RARE,
        "UNIQUE" => UNIQUE,
        "RELIC" => RELIC,
        _ => NORMAL,
    }
}

macro_rules! cap_warn {
    ($msg:literal) => {
        |v: f64, c: &Ctx| (v >= c.cap).then(|| $msg.to_string())
    };
}

fn max_hit_suffix(v: f64, c: &Ctx) -> Option<String> {
    if v == f64::INFINITY {
        Some("Immune".into())
    } else if c.t("GuardSkillActive") {
        Some("Guard".into())
    } else {
        None
    }
}

fn no_dot_but(c: &Ctx, v: f64, skip: &str) -> bool {
    c.ne(v, "TotalDPS")
        && [
            "TotalDot",
            "PoisonDPS",
            "IgniteDPS",
            "ImpaleDPS",
            "BleedDPS",
        ]
        .iter()
        .filter(|k| **k != skip)
        .all(|k| c.z(k) == 0.0)
}

fn max_hit_split(_: f64, c: &Ctx) -> bool {
    !c.same("LightningMaximumHitTaken", "ColdMaximumHitTaken")
        || !c.same("LightningMaximumHitTaken", "FireMaximumHitTaken")
}

/// `displayStats`
pub fn display_stats() -> Vec<Stat> {
    vec![
        s("ActiveMinionLimit", "Active Minion Limit", "d"),
        s("AverageHit", "Average Hit", ".1f"),
        s("PvpAverageHit", "PvP Average Hit", ".1f").flag("isPvP"),
        s("AverageDamage", "Average Damage", ".1f").flag("attack"),
        s("AverageDamage", "Average Damage", ".1f")
            .flag("monsterExplode")
            .cond(|_, c| c.n("HitChance") != Some(100.0)),
        s("AverageBurstDamage", "Average Burst Damage", ".1f")
            .cond(|v, c| c.n("AverageBurstHits").is_some_and(|h| h > 1.0) && v > 0.0),
        s("PvpAverageDamage", "PvP Average Damage", ".1f").flag("attackPvP"),
        s("Speed", "Attack Rate", ".2f")
            .flag("attack")
            .cond(|v, c| v > 0.0 && c.z("TriggerTime") == 0.0),
        s(
            "KineticFusilladeMaxEffectiveAPS",
            "Max Eff. KF Attack Rate",
            ".2f",
        )
        .flag("attack")
        .cond(|v, _| v > 0.0),
        s("Speed", "Cast Rate", ".2f")
            .flag("spell")
            .cond(|v, c| v > 0.0 && c.z("TriggerTime") == 0.0),
        s("Speed", "Effective Trigger Rate", ".2f")
            .not_flag("skipEffectiveRate")
            .cond(|_, c| c.z("TriggerTime") != 0.0),
        s("WarcryCastTime", "Cast Time", ".2fs").flag("warcry"),
        s("HitSpeed", "Hit Rate", ".2f").cond(|_, c| c.get("TriggerTime").is_none()),
        s("HitTime", "Channel Time", ".2fs")
            .flag("channelRelease")
            .cond(|_, c| c.get("TriggerTime").is_none()),
        s("ChannelTimeToTrigger", "Channel Time", ".2fs"),
        s("TrapThrowingTime", "Trap Throwing Time", ".2fs"),
        s("TrapCooldown", "Trap Cooldown", ".3fs"),
        s("MineLayingTime", "Mine Throwing Time", ".2fs"),
        s("TrapThrowCount", "Avg. Traps per Throw", ".2f"),
        s("MineThrowCount", "Avg. Mines per Throw", ".2f"),
        s("TotemPlacementTime", "Totem Placement Time", ".2fs")
            .cond(|_, c| c.get("TriggerTime").is_none()),
        s("PreEffectiveCritChance", "Crit Chance", ".2f%%").flag("hit"),
        s("CritChance", "Effective Crit Chance", ".2f%%")
            .flag("hit")
            .cond(|v, c| c.ne(v, "PreEffectiveCritChance")),
        Stat {
            pc: true,
            ..s("CritMultiplier", "Crit Multiplier", "d%%").cond(|_, c| c.z("CritChance") > 0.0)
        },
        s("HitChance", "Hit Chance", ".0f%%").flag("attack"),
        s("HitChance", "Hit Chance", ".0f%%").cond(|_, c| c.t("enemyHasSpellBlock")),
        s("TotalDPS", "Hit DPS", ".1f").flag("notAverage"),
        s("PvpTotalDPS", "PvP Hit DPS", ".1f").flag("notAveragePvP"),
        s("TotalDPS", "Hit DPS", ".1f")
            .flag("showAverage")
            .cond(|_, c| c.z("TriggerTime") != 0.0),
        s("TotalDot", "DoT DPS", ".1f"),
        s("WithDotDPS", "Total DPS inc. DoT", ".1f")
            .flag("notAverage")
            .cond(|v, c| no_dot_but(c, v, "TotalDot")),
        s("BleedDPS", "Bleed DPS", ".1f").warn(cap_warn!("Bleed DPS exceeds in game limit")),
        s("CorruptingBloodDPS", "Corrupting Blood DPS", ".1f")
            .warn(cap_warn!("Corrupting Blood DPS exceeds in game limit")),
        s("BleedDamage", "Total Damage per Bleed", ".1f").flag("showAverage"),
        s("WithBleedDPS", "Total DPS inc. Bleed", ".1f")
            .flag("notAverage")
            .cond(|v, c| no_dot_but(c, v, "BleedDPS")),
        s("IgniteDPS", "Ignite DPS", ".1f").warn(cap_warn!("Ignite DPS exceeds in game limit")),
        s("IgniteDamage", "Total Damage per Ignite", ".1f").flag("showAverage"),
        s("BurningGroundDPS", "Burning Ground DPS", ".1f")
            .warn(cap_warn!("Burning Ground DPS exceeds in game limit")),
        s("MirageBurningGroundDPS", "Mirage Burning Ground DPS", ".1f")
            .cond(|v, c| c.ne(v, "BurningGroundDPS"))
            .warn(cap_warn!("Mirage Burning Ground DPS exceeds in game limit")),
        s("WithIgniteDPS", "Total DPS inc. Ignite", ".1f")
            .flag("notAverage")
            .cond(|v, c| no_dot_but(c, v, "IgniteDPS")),
        s("WithIgniteAverageDamage", "Average Dmg. inc. Ignite", ".1f"),
        s("PoisonDPS", "Single Poison DPS", ".1f")
            .warn(cap_warn!("Poison DPS exceeds in game limit")),
        s("CausticGroundDPS", "Caustic Ground DPS", ".1f")
            .warn(cap_warn!("Caustic Ground DPS exceeds in game limit")),
        s("MirageCausticGroundDPS", "Mirage Caustic Ground DPS", ".1f")
            .cond(|v, c| c.ne(v, "CausticGroundDPS"))
            .warn(cap_warn!("Mirage Caustic Ground DPS exceeds in game limit")),
        s("PoisonDamage", "Total Damage per Poison", ".1f"),
        s("WithPoisonDPS", "Total DPS inc. Poison", ".1f")
            .flag("notAverage")
            .cond(|v, c| no_dot_but(c, v, "PoisonDPS")),
        s("DecayDPS", "Decay DPS", ".1f"),
        s("TotalDotDPS", "Total DoT DPS", ".1f")
            .cond(|v, c| {
                c.t("showTotalDotDPS")
                    || (c.ne(v, "TotalDot")
                        && c.ne(v, "TotalPoisonDPS")
                        && c.ne(v, "CausticGroundDPS")
                        && Some(v) != c.n("TotalIgniteDPS").or(c.n("IgniteDPS"))
                        && c.ne(v, "BurningGroundDPS")
                        && c.ne(v, "BleedDPS")
                        && c.ne(v, "CorruptingBloodDPS")
                        && c.ne(v, "MirageCausticGroundDPS")
                        && c.ne(v, "MirageBurningGroundDPS"))
            })
            .warn(cap_warn!("DoT DPS exceeds in game limit")),
        s("ImpaleDPS", "Impale Damage", ".1f").flag("showAverage"),
        s("WithImpaleDPS", "Damage inc. Impale", ".1f")
            .flag("showAverage")
            .cond(|v, c| no_dot_but(c, v, "ImpaleDPS")),
        s("ImpaleDPS", "Impale DPS", ".1f").flag("notAverage"),
        s("WithImpaleDPS", "Total DPS inc. Impale", ".1f")
            .flag("notAverage")
            .cond(|v, c| no_dot_but(c, v, "ImpaleDPS")),
        s("MirageDPS", "Total Mirage DPS", ".1f")
            .flag("mirageArcher")
            .cond(|v, _| v > 0.0),
        s("MirageDPS", "Total Wisp DPS", ".1f")
            .flag("wisp")
            .cond(|v, _| v > 0.0),
        s("CullingDPS", "Culling DPS", ".1f").cond(|_, c| c.z("CullingDPS") > 0.0),
        s("ReservationDPS", "Reservation DPS", ".1f").cond(|_, c| c.z("ReservationDPS") > 0.0),
        s("CombinedDPS", "Combined DPS", ".1f")
            .flag("notAverage")
            .cond(|v, c| {
                v != c.z("TotalDPS") + c.z("TotalDot")
                    && c.ne(v, "WithImpaleDPS")
                    && (c.t("showTotalDotDPS")
                        || (c.ne(v, "WithPoisonDPS")
                            && c.ne(v, "WithIgniteDPS")
                            && c.ne(v, "WithBleedDPS")))
            }),
        s("CombinedAvg", "Combined Total Damage", ".1f")
            .flag("showAverage")
            .cond(|v, c| {
                (c.ne(v, "AverageDamage") && c.z("TotalDot") == 0.0)
                    && (c.ne(v, "WithPoisonDPS")
                        || c.ne(v, "WithIgniteDPS")
                        || c.ne(v, "WithBleedDPS"))
            }),
        s("ExplodeChance", "Total Explode Chance", ".0f%%"),
        s("CombinedAvgToMonsterLife", "Enemy Life Equivalent", ".1f%%"),
        s("Cooldown", "Skill Cooldown", ".3fs").not_flag("skipEffectiveRate"),
        s("SealCooldown", "Seal Gain Frequency", ".2fs"),
        s("SealMax", "Max Number of Seals", "d"),
        s("TimeMaxSeals", "Time to Gain Max Seals", ".2fs"),
        s("AreaOfEffectRadiusMetres", "AoE Radius", ".1fm"),
        s("BrandAttachmentRangeMetre", "Attachment Range", ".1fm").flag("brand"),
        s("BrandTicks", "Activations per Brand", "d").flag("brand"),
        s("ManaCost", "Mana Cost", "d")
            .color(MANA)
            .cond(|_, c| c.t("ManaHasCost")),
        s("ManaPercentCost", "Mana Cost", "d%%")
            .color(MANA)
            .cond(|_, c| c.t("ManaPercentHasCost")),
        s("ManaPerSecondCost", "Mana Cost per second", ".2f")
            .color(MANA)
            .cond(|_, c| c.t("ManaPerSecondHasCost")),
        s("ManaPercentPerSecondCost", "Mana Cost per second", ".2f%%")
            .color(MANA)
            .cond(|_, c| c.t("ManaPercentPerSecondHasCost")),
        s("LifeCost", "Life Cost", "d")
            .color(LIFE)
            .cond(|_, c| c.t("LifeHasCost")),
        s("LifePercentCost", "Life Cost", "d%%")
            .color(LIFE)
            .cond(|_, c| c.t("LifePercentHasCost")),
        s("LifePerSecondCost", "Life Cost per second", ".2f")
            .color(LIFE)
            .cond(|_, c| c.t("LifePerSecondHasCost")),
        s("LifePercentPerSecondCost", "Life Cost per second", ".2f%%")
            .color(LIFE)
            .cond(|_, c| c.t("LifePercentPerSecondHasCost")),
        s("ESCost", "Energy Shield Cost", "d")
            .color(ES)
            .cond(|_, c| c.t("ESHasCost")),
        s("ESPerSecondCost", "ES Cost per second", ".2f")
            .color(ES)
            .cond(|_, c| c.t("ESPerSecondHasCost")),
        s("ESPercentPerSecondCost", "ES Cost per second", ".2f%%")
            .color(ES)
            .cond(|_, c| c.t("ESPercentPerSecondHasCost")),
        s("RageCost", "Rage Cost", "d")
            .color(RAGE)
            .cond(|_, c| c.t("RageHasCost")),
        s("RagePerSecondCost", "Rage Cost per second", ".2f")
            .color(RAGE)
            .cond(|_, c| c.t("RagePerSecondHasCost")),
        s("SoulCost", "Soul Cost", "d")
            .color(RAGE)
            .cond(|_, c| c.t("SoulHasCost")),
        GAP,
        s("Str", "Strength", "d").color(STRENGTH),
        s("ReqStr", "Strength Required", "d")
            .color(STRENGTH)
            .cond(|v, c| c.gt(v, "Str"))
            .warn(|_, c| {
                Some(format!(
                    "You do not meet the Strength requirement of {}",
                    c.req_source("ReqStrItem")
                ))
            }),
        s("Dex", "Dexterity", "d").color(DEXTERITY),
        s("ReqDex", "Dexterity Required", "d")
            .color(DEXTERITY)
            .cond(|v, c| c.gt(v, "Dex"))
            .warn(|_, c| {
                Some(format!(
                    "You do not meet the Dexterity requirement of {}",
                    c.req_source("ReqDexItem")
                ))
            }),
        s("Int", "Intelligence", "d").color(INTELLIGENCE),
        s("ReqInt", "Intelligence Required", "d")
            .color(INTELLIGENCE)
            .cond(|v, c| c.gt(v, "Int"))
            .warn(|_, c| {
                Some(format!(
                    "You do not meet the Intelligence requirement of {}",
                    c.req_source("ReqIntItem")
                ))
            }),
        s("Omni", "Omniscience", "d").color(RARE),
        s("ReqOmni", "Omniscience Required", "d")
            .color(RARE)
            .cond(|v, c| v > c.z("Omni"))
            .warn(|_, c| {
                Some(format!(
                    "You do not meet the Omniscience requirement of {}",
                    c.req_source("ReqOmniItem")
                ))
            }),
        GAP,
        s("Devotion", "Devotion", "d").color(RARE),
        GAP,
        s("TotalEHP", "Effective Hit Pool", ".0f")
            .suffix(Suffix::Text("Guard"))
            .suffix_cond(|_, c| c.t("GuardSkillActive")),
        s("PvPTotalTakenHit", "PvP Hit Taken", ".1f").flag("isPvP"),
        s("PhysicalMaximumHitTaken", "Phys Max Hit", ".0f")
            .color(PHYS)
            .suffix(Suffix::Func(max_hit_suffix)),
        s("LightningMaximumHitTaken", "Elemental Max Hit", ".0f")
            .color(LIGHTNING)
            .suffix(Suffix::Func(max_hit_suffix))
            .cond(|v, c| !max_hit_split(v, c)),
        s("FireMaximumHitTaken", "Fire Max Hit", ".0f")
            .color(FIRE)
            .suffix(Suffix::Func(max_hit_suffix))
            .cond(max_hit_split),
        s("ColdMaximumHitTaken", "Cold Max Hit", ".0f")
            .color(COLD)
            .suffix(Suffix::Func(max_hit_suffix))
            .cond(max_hit_split),
        s("LightningMaximumHitTaken", "Lightning Max Hit", ".0f")
            .color(LIGHTNING)
            .suffix(Suffix::Func(max_hit_suffix))
            .cond(max_hit_split),
        s("ChaosMaximumHitTaken", "Chaos Max Hit", ".0f")
            .color(CHAOS)
            .suffix(Suffix::Func(max_hit_suffix)),
        GAP,
        Stat {
            warn_color: true,
            ..s("MainHand", "MH Accuracy", "d")
                .child("Accuracy")
                .cond(|_, c| c.t("PreciseTechnique"))
                .warn(|v, c| {
                    c.lt(v, "Life")
                        .then(|| "You do not have enough Accuracy for Precise Technique".into())
                })
        },
        Stat {
            warn_color: true,
            ..s("OffHand", "OH Accuracy", "d")
                .child("Accuracy")
                .cond(|_, c| c.t("PreciseTechnique"))
                .warn(|v, c| {
                    c.lt(v, "Life")
                        .then(|| "You do not have enough Accuracy for Precise Technique".into())
                })
        },
        s("Life", "Total Life", "d").color(LIFE),
        s("Spec:LifeInc", "%Inc Life from Tree", "d%%")
            .color(LIFE)
            .cond(|v, c| v > 0.0 && c.n("Life").is_some_and(|l| l > 1.0)),
        s("LifeUnreserved", "Unreserved Life", "d")
            .color(LIFE)
            .cond(|v, c| c.lt(v, "Life"))
            .warn(|v, _| (v <= 0.0).then(|| "Your unreserved Life is below 1".into())),
        s("LifeRecoverable", "Life Recoverable", "d")
            .color(LIFE)
            .cond(|v, c| c.lt(v, "LifeUnreserved")),
        s("LifeUnreservedPercent", "Unreserved Life", "d%%")
            .color(LIFE)
            .cond(|v, _| v < 100.0),
        s("LifeRegenRecovery", "Life Regen", ".1f")
            .color(LIFE)
            .cond(|_, c| c.z("LifeRecovery") <= 0.0 && c.n("LifeRegenRecovery") != Some(0.0)),
        s("LifeRegenRecovery", "Life Recovery", ".1f")
            .color(LIFE)
            .cond(|_, c| c.z("LifeRecovery") > 0.0 && c.n("LifeRegenRecovery") != Some(0.0)),
        s("LifeRecharge", "Life Recharge", ".1f")
            .color(LIFE)
            .cond(|v, _| v > 0.0),
        s("LifeLeechGainRate", "Life Leech/On Hit Rate", ".1f").color(LIFE),
        s("LifeLeechGainPerHit", "Life Leech/Gain per Hit", ".1f").color(LIFE),
        GAP,
        s("Mana", "Total Mana", "d").color(MANA),
        s("Spec:ManaInc", "%Inc Mana from Tree", "d%%").color(MANA),
        s("ManaUnreserved", "Unreserved Mana", "d")
            .color(MANA)
            .cond(|v, c| c.lt(v, "Mana"))
            .warn(|v, _| (v < 0.0).then(|| "Your unreserved Mana is negative".into())),
        s("ManaUnreservedPercent", "Unreserved Mana", "d%%")
            .color(MANA)
            .cond(|v, _| v < 100.0),
        s("ManaRegenRecovery", "Mana Regen", ".1f")
            .color(MANA)
            .cond(|_, c| c.z("ManaRecovery") <= 0.0 && c.n("ManaRegenRecovery") != Some(0.0)),
        s("ManaRegenRecovery", "Mana Recovery", ".1f")
            .color(MANA)
            .cond(|_, c| c.z("ManaRecovery") > 0.0 && c.n("ManaRegenRecovery") != Some(0.0)),
        s("ManaLeechGainRate", "Mana Leech/On Hit Rate", ".1f").color(MANA),
        s("ManaLeechGainPerHit", "Mana Leech/Gain per Hit", ".1f").color(MANA),
        GAP,
        s("EnergyShield", "Energy Shield", "d").color(ES),
        s("EnergyShieldRecoveryCap", "Recoverable ES", "d")
            .color(ES)
            .cond(|_, c| c.t("CappingES")),
        s("Spec:EnergyShieldInc", "%Inc ES from Tree", "d%%").color(ES),
        s("EnergyShieldRegenRecovery", "ES Regen", ".1f")
            .color(ES)
            .cond(|_, c| {
                c.z("EnergyShieldRecovery") <= 0.0 && c.n("EnergyShieldRegenRecovery") != Some(0.0)
            }),
        s("EnergyShieldRegenRecovery", "ES Recovery", ".1f")
            .color(ES)
            .cond(|_, c| {
                c.z("EnergyShieldRecovery") > 0.0 && c.n("EnergyShieldRegenRecovery") != Some(0.0)
            }),
        s("EnergyShieldLeechGainRate", "ES Leech/On Hit Rate", ".1f").color(ES),
        s(
            "EnergyShieldLeechGainPerHit",
            "ES Leech/Gain per Hit",
            ".1f",
        )
        .color(ES),
        GAP,
        s("Ward", "Ward", "d").color(WARD),
        GAP,
        s("Rage", "Rage", "d").color(RAGE),
        s("RageRegenRecovery", "Rage Regen", ".1f").color(RAGE),
        GAP,
        s("TotalBuildDegen", "Total Degen", ".1f"),
        s("TotalNetRegen", "Total Net Recovery", "+.1f"),
        s("NetLifeRegen", "Net Life Recovery", "+.1f").color(LIFE),
        s("NetManaRegen", "Net Mana Recovery", "+.1f").color(MANA),
        s("NetEnergyShieldRegen", "Net ES Recovery", "+.1f").color(ES),
        GAP,
        s("Evasion", "Evasion rating", "d").color(EVASION),
        s("Spec:EvasionInc", "%Inc Evasion from Tree", "d%%").color(EVASION),
        s("MeleeEvadeChance", "Evade Chance", "d%%")
            .color(EVASION)
            .cond(|v, c| v > 0.0 && c.same("MeleeEvadeChance", "ProjectileEvadeChance")),
        s("MeleeEvadeChance", "Melee Evade Chance", "d%%")
            .color(EVASION)
            .cond(|v, c| v > 0.0 && !c.same("MeleeEvadeChance", "ProjectileEvadeChance")),
        s("ProjectileEvadeChance", "Projectile Evade Chance", "d%%")
            .color(EVASION)
            .cond(|v, c| v > 0.0 && !c.same("MeleeEvadeChance", "ProjectileEvadeChance")),
        GAP,
        s("Armour", "Armour", "d"),
        s("Spec:ArmourInc", "%Inc Armour from Tree", "d%%"),
        s("PhysicalDamageReduction", "Phys. Damage Reduction", "d%%").cond(|_, _| true),
        GAP,
        s("EffectiveBlockChance", "Block Chance", ".3f%%").over_cap("BlockChanceOverCap"),
        s("EffectiveSpellBlockChance", "Spell Block Chance", ".3f%%")
            .over_cap("SpellBlockChanceOverCap"),
        s("AttackDodgeChance", "Attack Dodge Chance", "d%%").over_cap("AttackDodgeChanceOverCap"),
        s("SpellDodgeChance", "Spell Dodge Chance", "d%%").over_cap("SpellDodgeChanceOverCap"),
        s(
            "EffectiveSpellSuppressionChance",
            "Spell Suppression Chance",
            ".2f%%",
        )
        .over_cap("SpellSuppressionChanceOverCap"),
        GAP,
        s("FireResist", "Fire Resistance", "d%%")
            .color(FIRE)
            .cond(|_, _| true)
            .over_cap("FireResistOverCap"),
        Stat {
            hide: true,
            ..s("FireResistOverCap", "Fire Res. Over Max", "d%%")
        },
        s("ColdResist", "Cold Resistance", "d%%")
            .color(COLD)
            .cond(|_, _| true)
            .over_cap("ColdResistOverCap"),
        Stat {
            hide: true,
            ..s("ColdResistOverCap", "Cold Res. Over Max", "d%%")
        },
        s("LightningResist", "Lightning Resistance", "d%%")
            .color(LIGHTNING)
            .cond(|_, _| true)
            .over_cap("LightningResistOverCap"),
        Stat {
            hide: true,
            ..s("LightningResistOverCap", "Lightning Res. Over Max", "d%%")
        },
        Stat {
            over_cap_cond: Some(|_, c| !c.t("ChaosInoculation")),
            ..s("ChaosResist", "Chaos Resistance", "d%%")
                .color(CHAOS)
                .over_cap("ChaosResistOverCap")
                .suffix(Suffix::Text("Immune"))
                .suffix_cond(|_, c| c.t("ChaosInoculation"))
        },
        Stat {
            hide: true,
            ..s("ChaosResistOverCap", "Chaos Res. Over Max", "d%%")
        },
        GAP,
        Stat {
            modifier: true,
            ..s(
                "EffectiveMovementSpeedMod",
                "Movement Speed Modifier",
                "+d%%",
            )
            .cond(|_, _| true)
        },
        GAP,
        s("FullDPS", "Full DPS", ".1f").color(CURRENCY),
        s("FullDotDPS", "Full Dot DPS", ".1f")
            .color(CURRENCY)
            .cond(|v, c| v >= c.cap)
            .warn(|_, _| Some("Full Dot DPS exceeds in game limit".into())),
        GAP,
        s("SkillDPS", "Skill DPS", "").cond(|_, _| true),
    ]
}

/// `minionDisplayStats`
pub fn minion_display_stats() -> Vec<Stat> {
    vec![
        s("AverageDamage", "Average Damage", ".1f"),
        s("Speed", "Attack/Cast Rate", ".2f").cond(|v, c| v > 0.0 && c.z("TriggerTime") == 0.0),
        s("HitSpeed", "Hit Rate", ".2f"),
        s("Speed", "Effective Trigger Rate", ".2f")
            .not_flag("skipEffectiveRate")
            .cond(|_, c| c.z("TriggerTime") != 0.0),
        s("TotalDPS", "Hit DPS", ".1f"),
        s("TotalDot", "DoT DPS", ".1f"),
        s("WithDotDPS", "Total DPS inc. DoT", ".1f").cond(|v, c| no_dot_but(c, v, "TotalDot")),
        s("BleedDPS", "Bleed DPS", ".1f").warn(cap_warn!("Minion Bleed DPS exceeds in game limit")),
        s("WithBleedDPS", "Total DPS inc. Bleed", ".1f").cond(|v, c| no_dot_but(c, v, "BleedDPS")),
        s("IgniteDPS", "Ignite DPS", ".1f")
            .warn(cap_warn!("Minion Ignite DPS exceeds in game limit")),
        s("WithIgniteDPS", "Total DPS inc. Ignite", ".1f")
            .cond(|v, c| no_dot_but(c, v, "IgniteDPS")),
        s("PoisonDPS", "Poison DPS", ".1f")
            .warn(cap_warn!("Minion Poison dps exceeds in game limit")),
        s("PoisonDamage", "Total Damage per Poison", ".1f"),
        s("WithPoisonDPS", "Total DPS inc. Poison", ".1f")
            .cond(|v, c| no_dot_but(c, v, "PoisonDPS")),
        s("DecayDPS", "Decay DPS", ".1f"),
        s("TotalDotDPS", "Total DoT DPS", ".1f")
            .cond(|v, c| {
                c.ne(v, "TotalDot")
                    && c.ne(v, "ImpaleDPS")
                    && c.ne(v, "TotalPoisonDPS")
                    && Some(v) != c.n("TotalIgniteDPS").or(c.n("IgniteDPS"))
                    && c.ne(v, "BleedDPS")
            })
            .warn(cap_warn!("Minion DoT DPS exceeds in game limit")),
        s("ImpaleDPS", "Impale DPS", ".1f").flag("impale"),
        s("WithImpaleDPS", "Total DPS inc. Impale", ".1f")
            .flag("impale")
            .cond(|v, c| no_dot_but(c, v, "ImpaleDPS")),
        s("CullingDPS", "Culling DPS", ".1f").cond(|_, c| c.z("CullingDPS") > 0.0),
        s("ReservationDPS", "Reservation DPS", ".1f").cond(|_, c| c.z("ReservationDPS") > 0.0),
        s("CombinedDPS", "Combined DPS", ".1f").cond(|v, c| {
            v != c.z("TotalDPS") + c.z("TotalDot")
                && c.ne(v, "WithImpaleDPS")
                && c.ne(v, "WithPoisonDPS")
                && c.ne(v, "WithIgniteDPS")
                && c.ne(v, "WithBleedDPS")
        }),
        s("Cooldown", "Skill Cooldown", ".3fs").not_flag("skipEffectiveRate"),
        s("Life", "Total Life", ".1f").color(LIFE),
        s("LifeRegenRecovery", "Life Recovery", ".1f").color(LIFE),
        s("LifeLeechGainRate", "Life Leech/On Hit Rate", ".1f").color(LIFE),
        s("EnergyShield", "Energy Shield", "d").color(ES),
        s("EnergyShieldRegenRecovery", "ES Recovery", ".1f").color(ES),
        s("EnergyShieldLeechGainRate", "ES Leech/On Hit Rate", ".1f").color(ES),
    ]
}
