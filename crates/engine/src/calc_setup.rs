//! Setting up a calculation: upstream's `calcs.initEnv` and the helpers of
//! `Modules/CalcSetup.lua`, building the player's and the enemy's mod
//! databases from the build. Ported so far: the base mods of both, the
//! bandit and pantheon mods, the configuration's mods, and the passive
//! nodes' mods with the radius jewels.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::config::{Inputs, Val};
use crate::jewel::{RadiusData, RadiusFunc, RadiusNode, RadiusType};
use crate::mod_parser::ModParser;
use crate::mods::convert::{Converter, array, get, number, text};
use crate::mods::list::{self, Precision};
use crate::mods::{Mod, ModType, ModValue, Store, Sym, Tag};
use crate::spec::{OrderedMap, PassiveSpec};
use crate::tree::{NodeType, PassiveTree};
use lua::{Key, Value};
use std::collections::HashMap;

pub type Result<T> = std::result::Result<T, String>;

/// The game data the setup needs
#[derive(Clone, Debug, Default)]
pub struct SetupData {
    pub character_constants: HashMap<String, f64>,
    pub misc: HashMap<String, f64>,
    /// By monster level, from 1
    pub monster_accuracy: Vec<f64>,
    /// God to its souls' names and mod lines
    pub pantheons: HashMap<String, Vec<(String, Vec<String>)>>,
    pub mod_flags: HashMap<String, u32>,
    pub keyword_flags: HashMap<String, u32>,
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

impl SetupData {
    /// Reads the data from upstream's `data` table and its `ModFlag` and
    /// `KeywordFlag`.
    pub fn from_lua(data: &Value, mod_flags: &Value, keyword_flags: &Value) -> Result<SetupData> {
        let flags = |v: &Value| -> Result<HashMap<String, u32>> {
            Ok(numbers(v)?
                .into_iter()
                .map(|(k, n)| (k, n as u32))
                .collect())
        };
        let mut pantheons = HashMap::new();
        for (god, g) in pairs(&get(data, "pantheons")) {
            let mut souls = Vec::new();
            for (_, soul) in pairs(&get(&g, "souls")) {
                let lines = pairs(&get(&soul, "mods"))
                    .iter()
                    .map(|(_, m)| Ok(text(&get(m, "line"))?.to_string()))
                    .collect::<Result<_>>()?;
                souls.push((text(&get(&soul, "name"))?.to_string(), lines));
            }
            pantheons.insert(key(&god)?, souls);
        }
        Ok(SetupData {
            character_constants: numbers(&get(data, "characterConstants"))?,
            misc: numbers(&get(data, "misc"))?,
            monster_accuracy: array(&get(data, "monsterAccuracyTable"))
                .iter()
                .map(number)
                .collect::<Result<_>>()?,
            pantheons,
            mod_flags: flags(mod_flags)?,
            keyword_flags: flags(keyword_flags)?,
        })
    }

    /// `data.characterConstants[name]`
    fn constant(&self, name: &str) -> f64 {
        self.character_constants
            .get(name)
            .copied()
            .unwrap_or(f64::NAN)
    }

    /// `data.misc[name]`
    fn misc(&self, name: &str) -> f64 {
        self.misc.get(name).copied().unwrap_or(f64::NAN)
    }

    /// `ModFlag[name]`
    fn mod_flag(&self, name: &str) -> u32 {
        self.mod_flags.get(name).copied().unwrap_or(0)
    }

    /// `KeywordFlag[name]`
    fn keyword_flag(&self, name: &str) -> u32 {
        self.keyword_flags.get(name).copied().unwrap_or(0)
    }
}

/// How buffs and conditions count (`misc_buffMode`)
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BuffMode {
    pub buffs: bool,
    pub combat: bool,
    pub effective: bool,
}

impl BuffMode {
    /// The buff mode of a calculation: the main one is effective, the
    /// Calcs tab's is its input's
    pub fn new(mode: Option<&str>) -> BuffMode {
        let (buffs, combat, effective) = match mode {
            Some("EFFECTIVE") => (true, true, true),
            Some("COMBAT") => (true, true, false),
            Some("BUFFED") => (true, false, false),
            _ => (false, false, false),
        };
        BuffMode {
            buffs,
            combat,
            effective,
        }
    }
}

/// `calcs.initModDB(env, modDB)`: the base mods both actors have
pub fn init_mod_db(d: &SetupData, db: &mut Store, mode: BuffMode) {
    db.add(Mod::new(
        "FireResistMax",
        ModType::Base,
        d.constant("base_maximum_all_resistances_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "ColdResistMax",
        ModType::Base,
        d.constant("base_maximum_all_resistances_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "LightningResistMax",
        ModType::Base,
        d.constant("base_maximum_all_resistances_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "ChaosResistMax",
        ModType::Base,
        d.constant("base_maximum_all_resistances_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "TotemFireResistMax",
        ModType::Base,
        d.constant("base_maximum_all_resistances_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "TotemColdResistMax",
        ModType::Base,
        d.constant("base_maximum_all_resistances_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "TotemLightningResistMax",
        ModType::Base,
        d.constant("base_maximum_all_resistances_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "TotemChaosResistMax",
        ModType::Base,
        d.constant("base_maximum_all_resistances_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "BlockChanceMax",
        ModType::Base,
        d.constant("maximum_block_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "SpellBlockChanceMax",
        ModType::Base,
        d.constant("base_maximum_spell_block_%"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "SpellDodgeChanceMax",
        ModType::Base,
        75.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "ChargeDuration",
        ModType::Base,
        10.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "PowerChargesMax",
        ModType::Base,
        d.constant("max_power_charges"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "FrenzyChargesMax",
        ModType::Base,
        d.constant("max_frenzy_charges"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "EnduranceChargesMax",
        ModType::Base,
        d.constant("max_endurance_charges"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "SiphoningChargesMax",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "ChallengerChargesMax",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "BlitzChargesMax",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "InspirationChargesMax",
        ModType::Base,
        d.constant("maximum_righteous_charges"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "CrabBarriersMax",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "BrutalChargesMax",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "BrineChargesMax",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(
        Mod::new(
            "PhysicalDamageGainAsCold",
            ModType::Base,
            d.constant("physical_damage_%_to_add_as_cold_per_brine_charge"),
            Some("Base"),
        )
        .tag(Tag::multiplier("BrineCharge")),
    );
    db.add(
        Mod::new(
            "PhysicalDamageGainAsLightning",
            ModType::Base,
            d.constant("physical_damage_%_to_add_as_lightning_per_brine_charge"),
            Some("Base"),
        )
        .tag(Tag::multiplier("BrineCharge")),
    );
    db.add(Mod::new(
        "AbsorptionChargesMax",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "AfflictionChargesMax",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "BloodChargesMax",
        ModType::Base,
        d.constant("maximum_blood_scythe_charges"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "MaxLifeLeechRate",
        ModType::Base,
        d.constant("maximum_life_leech_rate_%_per_minute") / 60.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "MaxManaLeechRate",
        ModType::Base,
        d.constant("maximum_mana_leech_rate_%_per_minute") / 60.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "ImpaleStacksMax",
        ModType::Base,
        d.constant("impaled_debuff_number_of_reflected_hits"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "SoulEaterMax",
        ModType::Base,
        d.constant("soul_eater_maximum_stacks"),
        Some("Base"),
    ));
    db.add(Mod::new("BleedStacksMax", ModType::Base, 1.0, Some("Base")));
    db.add(Mod::new(
        "MaxEnergyShieldLeechRate",
        ModType::Base,
        10.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "MaxLifeLeechInstance",
        ModType::Base,
        d.constant("maximum_life_leech_amount_per_leech_%_max_life"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "MaxManaLeechInstance",
        ModType::Base,
        d.constant("maximum_mana_leech_amount_per_leech_%_max_mana"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "MaxEnergyShieldLeechInstance",
        ModType::Base,
        d.constant("maximum_energy_shield_leech_amount_per_leech_%_max_energy_shield"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "TrapThrowingTime",
        ModType::Base,
        0.6,
        Some("Base"),
    ));
    db.add(Mod::new("MineLayingTime", ModType::Base, 0.3, Some("Base")));
    db.add(Mod::new("WarcryCastTime", ModType::Base, 0.8, Some("Base")));
    db.add(Mod::new(
        "TotemPlacementTime",
        ModType::Base,
        0.6,
        Some("Base"),
    ));
    db.add(Mod::new(
        "BallistaPlacementTime",
        ModType::Base,
        0.5,
        Some("Base"),
    ));
    db.add(Mod::new(
        "ActiveTotemLimit",
        ModType::Base,
        d.constant("base_number_of_totems_allowed"),
        Some("Base"),
    ));
    db.add(Mod::new("ShockStacksMax", ModType::Base, 1.0, Some("Base")));
    db.add(Mod::new(
        "ScorchStacksMax",
        ModType::Base,
        1.0,
        Some("Base"),
    ));
    db.add(
        Mod::new("MovementSpeed", ModType::Inc, -30.0, Some("Base")).tag(Tag::condition("Maimed")),
    );
    db.add(
        Mod::new("DamageTaken", ModType::Inc, 10.0, Some("Base"))
            .flags(d.mod_flag("Attack"), 0)
            .tag(Tag::condition("Intimidated")),
    );
    db.add(
        Mod::new("DamageTaken", ModType::Inc, 10.0, Some("Base"))
            .flags(d.mod_flag("Attack"), 0)
            .tag(Tag::condition("Intimidated").negated())
            .tag(Tag::condition("Party:Intimidated")),
    );
    db.add(
        Mod::new("DamageTaken", ModType::Inc, 10.0, Some("Base"))
            .flags(d.mod_flag("Spell"), 0)
            .tag(Tag::condition("Unnerved")),
    );
    db.add(
        Mod::new("DamageTaken", ModType::Inc, 10.0, Some("Base"))
            .flags(d.mod_flag("Spell"), 0)
            .tag(Tag::condition("Unnerved").negated())
            .tag(Tag::condition("Party:Unnerved")),
    );
    db.add(
        Mod::new("Damage", ModType::More, -10.0, Some("Base"))
            .tag(Tag::condition("Debilitated"))
            .tag(Tag::other(
                "GlobalEffect",
                [
                    ("effectName", "Debilitated".into()),
                    ("effectType", "Debuff".into()),
                ],
            )),
    );
    db.add(
        Mod::new("MovementSpeed", ModType::More, -20.0, Some("Base"))
            .tag(Tag::condition("Debilitated"))
            .tag(Tag::other(
                "GlobalEffect",
                [
                    ("effectName", "Debilitated".into()),
                    ("effectType", "Debuff".into()),
                ],
            )),
    );
    db.add(
        Mod::new("Damage", ModType::More, -10.0, Some("Base"))
            .tag(Tag::condition("MalignantMadness"))
            .tag(Tag::other(
                "GlobalEffect",
                [
                    ("effectName", "Malignant Madness".into()),
                    ("effectType", "Debuff".into()),
                ],
            )),
    );
    db.add(
        Mod::new("ActionSpeed", ModType::More, -10.0, Some("Base"))
            .tag(Tag::condition("MalignantMadness"))
            .tag(Tag::other(
                "GlobalEffect",
                [
                    ("effectName", "Malignant Madness".into()),
                    ("effectType", "Debuff".into()),
                ],
            )),
    );
    db.add(
        Mod::new("Condition:Burning", ModType::Flag, true, Some("Base"))
            .tag(Tag::other("IgnoreCond", []))
            .tag(Tag::condition("Ignited")),
    );
    db.add(
        Mod::new("Condition:Poisoned", ModType::Flag, true, Some("Base"))
            .tag(Tag::other("IgnoreCond", []))
            .tag(Tag::multiplier_threshold("PoisonStack", 1.0)),
    );
    db.add(Mod::new("Blind", ModType::Flag, true, Some("Base")).tag(Tag::condition("Blinded")));
    db.add(Mod::new("Chill", ModType::Flag, true, Some("Base")).tag(Tag::condition("Chilled")));
    db.add(Mod::new("Freeze", ModType::Flag, true, Some("Base")).tag(Tag::condition("Frozen")));
    db.add(Mod::new("Fortify", ModType::Flag, true, Some("Base")).tag(Tag::condition("Fortify")));
    db.add(
        Mod::new("Fortified", ModType::Flag, true, Some("Base")).tag(Tag::condition("Fortified")),
    );
    db.add(
        Mod::new("Excommunicated", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("Excommunicated")),
    );
    db.add(
        Mod::new("Fanaticism", ModType::Flag, true, Some("Base")).tag(Tag::condition("Fanaticism")),
    );
    db.add(
        Mod::new("Onslaught", ModType::Flag, true, Some("Base")).tag(Tag::condition("Onslaught")),
    );
    db.add(
        Mod::new("UnholyMight", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("UnholyMight")),
    );
    db.add(
        Mod::new("ChaoticMight", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("ChaoticMight")),
    );
    db.add(Mod::new("Tailwind", ModType::Flag, true, Some("Base")).tag(Tag::condition("Tailwind")));
    db.add(
        Mod::new("Adrenaline", ModType::Flag, true, Some("Base")).tag(Tag::condition("Adrenaline")),
    );
    db.add(
        Mod::new("AccelerationShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("AccelerationShrine")),
    );
    db.add(
        Mod::new("BrutalShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("BrutalShrine")),
    );
    db.add(
        Mod::new("DiamondShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("DiamondShrine")),
    );
    db.add(
        Mod::new("DivineShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("DivineShrine")),
    );
    db.add(
        Mod::new("EchoingShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("EchoingShrine")),
    );
    db.add(
        Mod::new("GloomShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("GloomShrine")),
    );
    db.add(
        Mod::new("GreaterFreezingShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("GreaterFreezingShrine")),
    );
    db.add(
        Mod::new("GreaterShockingShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("GreaterShockingShrine")),
    );
    db.add(
        Mod::new("GreaterSkeletalShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("GreaterSkeletalShrine")),
    );
    db.add(
        Mod::new("ImpenetrableShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("ImpenetrableShrine")),
    );
    db.add(
        Mod::new("MassiveShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("MassiveShrine")),
    );
    db.add(
        Mod::new("ReplenishingShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("ReplenishingShrine")),
    );
    db.add(
        Mod::new("ResistanceShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("ResistanceShrine")),
    );
    db.add(
        Mod::new("ResonatingShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("ResonatingShrine")),
    );
    db.add(
        Mod::new(
            "LesserAccelerationShrine",
            ModType::Flag,
            true,
            Some("Base"),
        )
        .tag(Tag::condition("LesserAccelerationShrine"))
        .tag(Tag::condition("AccelerationShrine").negated()),
    );
    db.add(
        Mod::new("LesserBrutalShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("LesserBrutalShrine"))
            .tag(Tag::condition("BrutalShrine").negated()),
    );
    db.add(
        Mod::new(
            "LesserImpenetrableShrine",
            ModType::Flag,
            true,
            Some("Base"),
        )
        .tag(Tag::condition("LesserImpenetrableShrine"))
        .tag(Tag::condition("ImpenetrableShrine").negated()),
    );
    db.add(
        Mod::new("LesserMassiveShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("LesserMassiveShrine"))
            .tag(Tag::condition("MassiveShrine").negated()),
    );
    db.add(
        Mod::new(
            "LesserReplenishingShrine",
            ModType::Flag,
            true,
            Some("Base"),
        )
        .tag(Tag::condition("LesserReplenishingShrine"))
        .tag(Tag::condition("ReplenishingShrine").negated()),
    );
    db.add(
        Mod::new("LesserResistanceShrine", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("LesserResistanceShrine"))
            .tag(Tag::condition("ResistanceShrine").negated()),
    );
    db.add(
        Mod::new("AlchemistsGenius", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("AlchemistsGenius")),
    );
    db.add(
        Mod::new("LuckyHits", ModType::Flag, true, Some("Base")).tag(Tag::condition("LuckyHits")),
    );
    db.add(
        Mod::new("Convergence", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("Convergence")),
    );
    db.add(
        Mod::new(
            "PhysicalDamageReduction",
            ModType::Base,
            -15.0,
            Some("Base"),
        )
        .tag(Tag::condition("Crushed")),
    );
    db.add(Mod::new(
        "CritChanceCap",
        ModType::Base,
        100.0,
        Some("Base"),
    ));

    db.conditions.insert(Sym::new("Buffed"), mode.buffs);
    db.conditions.insert(Sym::new("Combat"), mode.combat);
    db.conditions.insert(Sym::new("Effective"), mode.effective);
}

/// What the base of a calculation is built from
pub struct BaseInput<'a> {
    /// The class's `base_str`, `base_dex` and `base_int`
    pub class_stats: [f64; 3],
    pub character_level: f64,
    pub enemy_level: f64,
    pub config_input: &'a Inputs,
    pub config_mods: &'a [Mod],
    pub config_enemy_mods: &'a [Mod],
    pub mode: BuffMode,
}

/// The player's and the enemy's mod databases before the tree, items and
/// skills: `initEnv` up to the cached copies (`specCopy`)
pub fn base_dbs(d: &SetupData, parser: &ModParser, b: &BaseInput) -> Result<(Store, Store)> {
    let mut db = Store::db();
    for (stat, base) in ["Str", "Dex", "Int"].iter().zip(b.class_stats) {
        db.add(Mod::new(stat, ModType::Base, base, Some("Base")));
    }
    db.multipliers
        .insert(Sym::new("Level"), 1f64.max(100f64.min(b.character_level)));
    init_mod_db(d, &mut db, b.mode);
    let penalty = match b.config_input.get("resistancePenalty") {
        Some(v) if !matches!(v, Val::Bool(false)) => v.value(),
        _ => ModValue::Number(-60.0),
    };
    db.add(
        Mod::new(
            "Life",
            ModType::Base,
            d.constant("life_per_level"),
            Some("Base"),
        )
        .tag(Tag::multiplier("Level").base(38.0)),
    );
    db.add(
        Mod::new(
            "Mana",
            ModType::Base,
            d.constant("mana_per_level"),
            Some("Base"),
        )
        .tag(Tag::multiplier("Level").base(34.0)),
    );
    db.add(
        Mod::new(
            "ManaRegen",
            ModType::Base,
            d.misc("ManaRegenBase"),
            Some("Base"),
        )
        .tag(Tag::per_stat("Mana").divisor(1.0)),
    );
    db.add(Mod::new("Devotion", ModType::Base, 0.0, Some("Base")));
    db.add(Mod::new(
        "Evasion",
        ModType::Base,
        d.constant("base_evasion_rating"),
        Some("Base"),
    ));
    db.add(
        Mod::new(
            "Accuracy",
            ModType::Base,
            d.constant("accuracy_rating_per_level"),
            Some("Base"),
        )
        .tag(Tag::multiplier("Level").base(-d.constant("accuracy_rating_per_level"))),
    );
    db.add(Mod::new(
        "CritMultiplier",
        ModType::Base,
        d.constant("base_critical_strike_multiplier") - 100.0,
        Some("Base"),
    ));
    db.add(
        Mod::new(
            "DotMultiplier",
            ModType::Base,
            d.constant("critical_ailment_dot_multiplier_+"),
            Some("Base"),
        )
        .tag(Tag::condition("CriticalStrike")),
    );
    for stat in ["FireResist", "ColdResist", "LightningResist", "ChaosResist"] {
        db.add(Mod::new(stat, ModType::Base, penalty.clone(), Some("Base")));
    }
    db.add(Mod::new(
        "TotemFireResist",
        ModType::Base,
        40.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "TotemColdResist",
        ModType::Base,
        40.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "TotemLightningResist",
        ModType::Base,
        40.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "TotemChaosResist",
        ModType::Base,
        20.0,
        Some("Base"),
    ));
    db.add(
        Mod::new(
            "CritChance",
            ModType::Inc,
            d.constant("critical_strike_chance_+%_per_power_charge"),
            Some("Base"),
        )
        .tag(Tag::multiplier("PowerCharge")),
    );
    db.add(
        Mod::new(
            "Speed",
            ModType::Inc,
            d.constant("base_attack_speed_+%_per_frenzy_charge"),
            Some("Base"),
        )
        .flags(d.mod_flag("Attack"), 0)
        .tag(Tag::multiplier("FrenzyCharge")),
    );
    db.add(
        Mod::new(
            "Speed",
            ModType::Inc,
            d.constant("base_cast_speed_+%_per_frenzy_charge"),
            Some("Base"),
        )
        .flags(d.mod_flag("Cast"), 0)
        .tag(Tag::multiplier("FrenzyCharge")),
    );
    db.add(
        Mod::new(
            "Damage",
            ModType::More,
            d.constant("object_inherent_damage_+%_final_per_frenzy_charge"),
            Some("Base"),
        )
        .tag(Tag::multiplier("FrenzyCharge")),
    );
    db.add(
        Mod::new(
            "PhysicalDamageReduction",
            ModType::Base,
            d.constant("physical_damage_reduction_%_per_endurance_charge"),
            Some("Base"),
        )
        .tag(Tag::multiplier("EnduranceCharge")),
    );
    db.add(
        Mod::new(
            "ElementalDamageReduction",
            ModType::Base,
            d.constant("elemental_damage_reduction_%_per_endurance_charge"),
            Some("Base"),
        )
        .tag(Tag::multiplier("EnduranceCharge")),
    );
    db.add(Mod::new(
        "MaximumRage",
        ModType::Base,
        d.constant("maximum_rage"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "Multiplier:GaleForce",
        ModType::Base,
        0.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "MaximumGaleForce",
        ModType::Base,
        10.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "MaximumFortification",
        ModType::Base,
        d.constant("base_max_fortification"),
        Some("Base"),
    ));
    db.add(Mod::new("MaximumValour", ModType::Base, 50.0, Some("Base")));
    db.add(Mod::new(
        "Multiplier:IntensityLimit",
        ModType::Base,
        3.0,
        Some("Base"),
    ));
    db.add(
        Mod::new(
            "Damage",
            ModType::Inc,
            d.constant("damage_+%_per_10_rampage_stacks"),
            Some("Base"),
        )
        .tag(
            Tag::multiplier("Rampage")
                .divisor(20.0)
                .limit(d.constant("max_rampage_stacks") / 20.0),
        ),
    );
    db.add(
        Mod::new(
            "MovementSpeed",
            ModType::Inc,
            d.constant("movement_velocity_+%_per_10_rampage_stacks"),
            Some("Base"),
        )
        .tag(
            Tag::multiplier("Rampage")
                .divisor(20.0)
                .limit(d.constant("max_rampage_stacks") / 20.0),
        ),
    );
    db.add(Mod::new(
        "ActiveTrapLimit",
        ModType::Base,
        d.constant("base_number_of_traps_allowed"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "ActiveMineLimit",
        ModType::Base,
        d.constant("base_number_of_remote_mines_allowed"),
        Some("Base"),
    ));
    db.add(Mod::new("MineThrowCount", ModType::Base, 1.0, Some("Base")));
    db.add(Mod::new("TrapThrowCount", ModType::Base, 1.0, Some("Base")));
    db.add(Mod::new(
        "ActiveBrandLimit",
        ModType::Base,
        3.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "EnemyCurseLimit",
        ModType::Base,
        1.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "SocketedCursesHexLimitValue",
        ModType::Base,
        1.0,
        Some("Base"),
    ));
    db.add(Mod::new(
        "ProjectileCount",
        ModType::Base,
        1.0,
        Some("Base"),
    ));
    db.add(
        Mod::new(
            "Speed",
            ModType::More,
            d.constant("dual_wield_inherent_attack_speed_+%_final"),
            Some("Base"),
        )
        .flags(d.mod_flag("Attack"), 0)
        .tag(Tag::condition("DualWielding"))
        .tag(Tag::condition("DoubledInherentDualWieldingSpeed").negated()),
    );
    db.add(
        Mod::new(
            "Speed",
            ModType::More,
            2.0 * d.constant("dual_wield_inherent_attack_speed_+%_final"),
            Some("Base"),
        )
        .flags(d.mod_flag("Attack"), 0)
        .tag(Tag::condition("DualWielding"))
        .tag(Tag::condition("DoubledInherentDualWieldingSpeed")),
    );
    db.add(
        Mod::new(
            "BlockChance",
            ModType::Base,
            d.constant("inherent_block_while_dual_wielding_%"),
            Some("Base"),
        )
        .tag(Tag::condition("DualWielding"))
        .tag(Tag::condition("NoInherentBlock").negated())
        .tag(Tag::condition("DoubledInherentDualWieldingBlock").negated()),
    );
    db.add(
        Mod::new(
            "BlockChance",
            ModType::Base,
            2.0 * d.constant("inherent_block_while_dual_wielding_%"),
            Some("Base"),
        )
        .tag(Tag::condition("DualWielding"))
        .tag(Tag::condition("NoInherentBlock").negated())
        .tag(Tag::condition("DoubledInherentDualWieldingBlock")),
    );
    db.add(
        Mod::new("Damage", ModType::More, 200.0, Some("Base"))
            .flags(0, d.keyword_flag("Bleed"))
            .tag(Tag::actor_condition("enemy", "Moving"))
            .tag(Tag::condition("NoExtraBleedDamageToMovingEnemy").negated()),
    );
    db.add(
        Mod::new("Condition:BloodStance", ModType::Flag, true, Some("Base"))
            .tag(Tag::condition("SandStance").negated()),
    );
    db.add(
        Mod::new(
            "Condition:PrideMinEffect",
            ModType::Flag,
            true,
            Some("Base"),
        )
        .tag(Tag::condition("PrideMaxEffect").negated()),
    );
    db.add(Mod::new(
        "PerBrutalTripleDamageChance",
        ModType::Base,
        d.constant("chance_to_deal_triple_damage_%_per_brutal_charge"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "PerAfflictionAilmentDamage",
        ModType::Base,
        d.constant("ailment_damage_+%_final_per_affliction_charge"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "PerAfflictionNonDamageEffect",
        ModType::Base,
        d.constant("non_damaging_ailment_effect_+%_final_per_affliction_charge"),
        Some("Base"),
    ));
    db.add(Mod::new(
        "PerAbsorptionElementalEnergyShieldRecoup",
        ModType::Base,
        d.constant(
            "elemental_damage_taken_goes_to_energy_shield_over_4_seconds_%_per_absorption_charge",
        ),
        Some("Base"),
    ));
    db.add(Mod::new("TinctureLimit", ModType::Base, 1.0, Some("Base")));
    db.add(
        Mod::new("ManaDegenPercentTincture", ModType::Base, 1.0, Some("Base"))
            .tag(Tag::multiplier("EffectiveManaBurnStacks")),
    );
    db.add(
        Mod::new("LifeDegenPercentTincture", ModType::Base, 1.0, Some("Base"))
            .tag(Tag::multiplier("WeepingWoundsStacks")),
    );
    db.add(Mod::new(
        "PresenceRadius",
        ModType::Base,
        d.constant("base_presence_radius"),
        Some("Base"),
    ));

    // Bandit
    let bandit = b.config_input.get("bandit");
    if bandit.is_some_and(|v| v.is_str("Alira")) {
        db.add(Mod::new(
            "ElementalResist",
            ModType::Base,
            15.0,
            Some("Bandit"),
        ));
    } else if bandit.is_some_and(|v| v.is_str("Kraityn")) {
        db.add(Mod::new("MovementSpeed", ModType::Inc, 8.0, Some("Bandit")));
    } else if bandit.is_some_and(|v| v.is_str("Oak")) {
        db.add(Mod::new("Life", ModType::Base, 40.0, Some("Bandit")));
    } else {
        db.add(Mod::new("ExtraPoints", ModType::Base, 1.0, Some("Bandit")));
    }

    // Pantheon
    for var in ["pantheonMajorGod", "pantheonMinorGod"] {
        let god = b.config_input.get(var);
        if god.is_some_and(|g| g.is_str("None")) {
            continue;
        }
        let god = god.map(|g| g.text()).unwrap_or_default();
        let souls = d
            .pantheons
            .get(&god)
            .ok_or_else(|| format!("attempt to index a nil value (pantheon {god:?})"))?;
        apply_soul_mods(&mut db, parser, souls)?;
    }

    // The enemy
    let mut enemy = Store::db();
    init_mod_db(d, &mut enemy, b.mode);
    let accuracy = if b.enemy_level.fract() == 0.0 && b.enemy_level >= 1.0 {
        d.monster_accuracy.get(b.enemy_level as usize - 1).copied()
    } else {
        None
    };
    let accuracy = accuracy.ok_or("no monster accuracy for the enemy level")?;
    enemy.add(Mod::new("Accuracy", ModType::Base, accuracy, Some("Base")));
    enemy.add(
        Mod::new(
            "Condition:AgainstDamageOverTime",
            ModType::Flag,
            true,
            Some("Base"),
        )
        .flags(d.mod_flag("Dot"), 0)
        .tag(Tag::actor_condition("player", "Combat")),
    );

    // The configuration's mods; the party tab adds none for the enemy here
    for m in b.config_mods {
        db.add(m.clone());
    }
    for m in b.config_enemy_mods {
        enemy.add(m.clone());
    }
    Ok((db, enemy))
}

/// `pantheon.applySoulMod(db, parser, god)`: the mods of all of a god's
/// souls, sourced from the god's first soul
fn apply_soul_mods(
    db: &mut Store,
    parser: &ModParser,
    souls: &[(String, Vec<String>)],
) -> Result<()> {
    let god_name = souls.first().map(|(n, _)| n.as_str()).unwrap_or("");
    let conv = Converter::default();
    for (_, lines) in souls {
        for line in lines {
            let (list, extra) = parser.parse(line.as_bytes());
            if let (Value::Table(t), false) = (&list, extra.truthy()) {
                for m in t.array() {
                    let mut m = conv.mod_(&m)?;
                    m.source = Some(format!("Pantheon:{god_name}").into());
                    db.add(m);
                }
            }
        }
    }
    Ok(())
}

/// A radius jewel in the setup (`env.radiusJewelList`)
#[derive(Clone, Debug)]
pub struct RadiusJewel {
    /// The socket's node
    pub node_id: u32,
    pub item_id: f64,
    /// The nodes in the radius, by id, with their type
    pub nodes: HashMap<u32, NodeType>,
    pub func: RadiusFunc,
    pub typ: RadiusType,
    pub data: RadiusData,
}

/// A node's mods as the setup builds them (`calcs.buildModListForNode`)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeMods {
    pub mods: Vec<Mod>,
    /// `CanExplode`: the node is an explode source
    pub explode: bool,
    /// The skills the node grants (`node.grantedSkills`)
    pub granted_skills: Vec<NodeSkill>,
}

/// A skill a passive node grants
#[derive(Clone, Debug, PartialEq)]
pub struct NodeSkill {
    pub skill_id: Option<ModValue>,
    pub level: Option<ModValue>,
    pub source: String,
}

/// `calcLib.mod(modList, nil, name)`
fn calc_mod(mods: &[Mod], name: &str) -> Result<f64> {
    let inc = list::sum(mods, ModType::Inc, name)?;
    let more = list::more(mods, name)?;
    Ok((1.0 + inc / 100.0) * more)
}

/// `calcs.buildModListForNode(env, node)`: the node's mods with the radius
/// jewels applied and its passive effect scaling
pub fn build_mod_list_for_node(
    spec: &PassiveSpec,
    i: usize,
    alloc: &OrderedMap<usize>,
    rads: &mut [RadiusJewel],
    precision: &dyn Precision,
) -> Result<NodeMods> {
    let node = spec.tree_node(i);
    let id = spec.nodes[i].id;
    let mut mods: Vec<Mod> = if node.kind == NodeType::Keystone {
        node.keystone_mod.iter().cloned().collect()
    } else {
        spec.mod_list(i).to_vec()
    };
    let own = spec.mod_list(i).to_vec();
    let view = RadiusNode {
        id,
        typ: node.kind,
        mod_list: &own,
        is_tattoo: false,
        conquered: false,
    };
    // First pass: the jewels that change nodes
    for rad in rads.iter_mut() {
        if rad.typ == RadiusType::Other
            && rad.nodes.get(&id).is_some_and(|t| *t != NodeType::Mastery)
        {
            rad.func.run(Some(&view), &mut mods, &mut rad.data)?;
        }
    }
    if list::flag(&mods, "PassiveSkillHasNoEffect")?
        || (alloc.contains(id) && list::flag(&mods, "AllocatedPassiveSkillHasNoEffect")?)
    {
        mods.clear();
    }
    // The passive effect scaling
    let scale = calc_mod(&mods, "PassiveSkillEffect")?;
    if scale != 1.0 {
        mods = mods
            .iter()
            .map(|m| list::scale_mod(m, scale, precision))
            .collect();
    }
    // Second pass: the jewels that read nodes
    let allocated = alloc.contains(id);
    for rad in rads.iter_mut() {
        let applies = match rad.typ {
            RadiusType::Threshold => true,
            RadiusType::Self_ => allocated,
            RadiusType::SelfUnalloc => !allocated,
            RadiusType::Other => false,
        };
        if applies && rad.nodes.get(&id).is_some_and(|t| *t != NodeType::Mastery) {
            rad.func.run(Some(&view), &mut mods, &mut rad.data)?;
        }
    }
    if list::flag(&mods, "PassiveSkillHasOtherEffect")? {
        let others = list::list(&mods, "NodeModifier")?;
        for (n, v) in others.into_iter().enumerate() {
            if n == 0 {
                mods.clear();
            }
            match v {
                ModValue::Record(r) => match r.get("mod") {
                    Some(ModValue::Mod(m)) => mods.push((**m).clone()),
                    m => return Err(format!("NodeModifier {m:?}")),
                },
                v => return Err(format!("NodeModifier {v:?}")),
            }
        }
    }
    let mut granted_skills = Vec::new();
    for v in list::list(&mods, "ExtraSkill")? {
        let ModValue::Record(r) = v else {
            return Err(format!("ExtraSkill {v:?}"));
        };
        if matches!(r.get("name"), Some(ModValue::Str(n)) if &**n == "Unknown") {
            continue;
        }
        granted_skills.push(NodeSkill {
            skill_id: r.get("skillId").cloned(),
            level: r.get("level").cloned(),
            source: format!("Tree:{id}"),
        });
    }
    let explode = list::flag(&mods, "CanExplode")?;
    Ok(NodeMods {
        mods,
        explode,
        granted_skills,
    })
}

/// The mods of a node list, and each node's own by node id
pub type NodeListMods = (Vec<Mod>, Vec<(u32, NodeMods)>);

/// `calcs.buildModListForNodeList(env, nodeList, finishJewels)`: the mods
/// of the nodes, in order, and each node's own; with `finish`, the extra
/// nodes in the radius of jewels run too, and the jewels finish.
pub fn build_mod_list_for_node_list(
    spec: &PassiveSpec,
    nodes: &[usize],
    alloc: &OrderedMap<usize>,
    rads: &mut [RadiusJewel],
    extra: &[usize],
    finish: bool,
    precision: &dyn Precision,
) -> Result<NodeListMods> {
    for rad in rads.iter_mut() {
        rad.data = RadiusData {
            mod_source: format!("Tree:{}", rad.node_id),
            ..RadiusData::default()
        };
    }
    let mut out = Vec::new();
    let mut per_node = Vec::new();
    for &i in nodes {
        let node = build_mod_list_for_node(spec, i, alloc, rads, precision)?;
        out.extend(node.mods.iter().cloned());
        per_node.push((spec.nodes[i].id, node));
    }
    if finish {
        for &i in extra {
            build_mod_list_for_node(spec, i, alloc, rads, precision)?;
        }
        for rad in rads.iter_mut() {
            rad.func.run(None, &mut out, &mut rad.data)?;
        }
    }
    Ok((out, per_node))
}

/// `modLib.mergeKeystones(env, modList)`: adds the mods of the keystones
/// the mods grant (`Keystone`), once each, sourced from what grants them
/// unless that is the tree. Upstream sets that source on the tree's own
/// keystone mods; here the added copies have it.
pub fn merge_keystones(
    mods: &mut Vec<Mod>,
    tree: &PassiveTree,
    added: &mut Vec<String>,
) -> Result<()> {
    let grants: Vec<(String, Option<std::rc::Rc<str>>)> =
        list::tabulate(mods, Some(ModType::List), "Keystone")?
            .into_iter()
            .filter_map(|(v, m)| match v {
                ModValue::Str(v) => Some((v.to_string(), m.source.clone())),
                _ => None,
            })
            .collect();
    for (name, source) in grants {
        if added.contains(&name) {
            continue;
        }
        let Some(&id) = tree.keystone_map.get(name.as_str()) else {
            continue;
        };
        added.push(name);
        let from_other = source
            .as_ref()
            .is_some_and(|s| !s.to_ascii_lowercase().contains("tree"));
        for m in &tree.node(id).ok_or("keystone not in the tree")?.mod_list {
            let mut m = m.clone();
            if from_other {
                set_source(&mut m, source.as_deref().unwrap_or(""));
            }
            mods.push(m);
        }
    }
    Ok(())
}

/// `modLib.setSource(mod, source)`
fn set_source(m: &mut Mod, source: &str) {
    m.source = Some(source.into());
    if let ModValue::Record(r) = &mut m.value
        && let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod"))
    {
        inner.source = Some(source.into());
    }
}

/// The allocation counts' multipliers of `initEnv`
pub fn allocation_mods(spec: &PassiveSpec, db: &mut Store) {
    let counts = [
        ("Multiplier:AllocatedNotable", spec.allocated_notable_count),
        (
            "Multiplier:AllocatedKeystone",
            spec.allocated_keystone_count,
        ),
        ("Multiplier:AllocatedMastery", spec.allocated_mastery_count),
        (
            "Multiplier:AllocatedMasteryType",
            spec.allocated_mastery_type_count,
        ),
    ];
    for (name, n) in counts {
        if n > 0 {
            db.add(Mod::new(name, ModType::Base, n as f64, None));
        }
    }
    if let Some((_, n)) = spec
        .allocated_mastery_types
        .iter()
        .find(|(name, _)| &**name == "Life Mastery")
        && *n > 0
    {
        db.add(Mod::new(
            "Multiplier:AllocatedLifeMastery",
            ModType::Base,
            *n as f64,
            None,
        ));
    }
}
