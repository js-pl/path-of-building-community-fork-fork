//! `calcs.triggers` (`Modules/CalcTriggers.lua`): the rate at which the
//! main skill is triggered, from its trigger's configuration
//! (`configTable`) and `defaultTriggerHandler`, with the simulation of
//! skill rotations (`calcMultiSpellRotationImpact`).
//!
//! Every configuration of `configTable` is ported, with the helmet's Focus
//! (`helmetFocusHandler`), Snipe, Manaforged Arrows and the uptime of
//! exerting warcries.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::calc::{PLAYER, m_max, m_min, round};
use crate::calc_cache::CachedCalc;
use crate::calc_perform::{Perform, Result, src};
use crate::mods::{ModType, ModValue};
use crate::sf;

/// A skill as the rotation simulation sees it
/// (`packageSkillDataForSimulation`)
#[derive(Clone, Debug)]
pub(crate) struct SimSkill {
    pub uuid: String,
    pub cd: Option<f64>,
    pub cd_override: Option<f64>,
    pub adds_cast_time: Option<f64>,
    pub icdr: f64,
    pub added_cooldown: f64,
}

/// `comparer(env, uuid, source, triggerRate)`: whether a skill's cached
/// calculation beats the source so far and its rate
type Comparer = fn(&CachedCalc, Option<usize>, Option<f64>) -> bool;

/// A trigger's configuration (`configTable[name](env)`)
#[derive(Default)]
pub(crate) struct TriggerConfig {
    pub actor: Option<usize>,
    pub trigger_name: Option<String>,
    /// The triggering skill, by index
    pub source: Option<usize>,
    pub trigger_chance: Option<f64>,
    pub triggered_skills: Vec<SimSkill>,
    /// `triggerSkillCond`: which skills can trigger the main skill
    pub trigger_skill_cond: Option<fn(&Perform, usize) -> bool>,
    /// `triggeredSkillCond`: which skills the trigger triggers
    pub triggered_skill_cond: Option<fn(&Perform, usize) -> bool>,
    /// The default compares the rates
    pub comparer: Option<Comparer>,
    pub trig_rate: Option<f64>,
    pub allow_triggered: bool,
    pub trigger_on_use: bool,
    /// `useCastRate`: the breakdown names the source's rate a cast rate
    pub use_cast_rate: bool,
    pub assuming_every_hit_kills: bool,
    pub ignore_source_rate: bool,
    pub source_weapon: bool,
    pub stages_are_overlaps: Option<f64>,
    pub overlaps: Option<f64>,
    /// `customHandler`: a handler of its own in place of the default one
    pub custom_handler: Option<fn(&mut Perform) -> Result<()>>,
    /// `minimumCostPerUse`: the cost stat a source must pay per use
    pub minimum_cost: Option<(&'static str, f64)>,
    /// `sourceName`: the source's name in the breakdown
    pub source_name: Option<String>,
    /// The skill whose active effect stands for the trigger's
    /// (`triggeredBy.grantedEffect`), for its cooldown
    pub trigger_effect: Option<usize>,
    /// Arcanist Brand as the trigger: the brand skill, its activation
    /// frequency's increase and more multipliers
    /// (`triggeredBy.mainSkill`, `activationFreqInc`, `activationFreqMore`)
    pub brand: Option<(usize, f64, f64)>,
}

/// A rotation simulation's main rate, simulated time and each skill's rate
/// by uuid
type Sim = (f64, f64, Vec<(String, f64)>);

/// `defaultComparer`: whether the skill's cached rate beats the source's
fn default_comparer(c: &CachedCalc, source: Option<usize>, rate: Option<f64>) -> bool {
    let speed = c.out("HitSpeed").or_else(|| c.out("Speed"));
    match source {
        None => speed.is_some(),
        Some(_) => speed.is_some_and(|s| s > rate.unwrap_or(0.0)),
    }
}

/// The cooldowns `breakdown.TriggerRateCap` shows
struct TriggerCaps<'n> {
    actor: usize,
    main: usize,
    /// `config.triggerName`
    name: &'n str,
    cd_override: Option<f64>,
    triggered_cd: Option<f64>,
    added: Option<f64>,
    trigger_cd: Option<f64>,
    adds_cast_time: Option<f64>,
    icdr: f64,
    triggered_adj: f64,
    trigger_adj: f64,
    action_tick: f64,
    cap: f64,
    /// Arcanist Brand, whose activation ignores the tick rate
    brand: bool,
}

/// `breakdown.SimData`: each simulated skill's rate, its uuid split into
/// name, slot and gem index
fn sim_breakdown(sim_time: f64, rates: &[(String, f64)]) -> crate::breakdown::Breakdown {
    use crate::breakdown::{Breakdown, Col, Row};
    let rows = rates
        .iter()
        .map(|(uuid, rate)| {
            let mut row: Row = vec![("rate", crate::calc::round(*rate, 2.0).into())];
            for (k, part) in ["skillName", "slotName", "gemIndex"]
                .into_iter()
                .zip(uuid.split('_').filter(|p| !p.is_empty()))
            {
                row.push((k, part.into()));
            }
            row
        })
        .collect();
    Breakdown {
        lines: vec![sf!(
            "Simulation duration: %.2f ^8(In game source skill usage duration in seconds)",
            sim_time
        )],
        rows: Some(rows),
        cols: Some(vec![
            Col::new("Rate", "rate"),
            Col::new("Skill Name", "skillName"),
            Col::new("Slot Name", "slotName"),
            Col::new("Gem Index", "gemIndex"),
        ]),
        ..Breakdown::default()
    }
}

/// The rotation of `calcMultiSpellRotationImpact`: the simulated time
/// and how often each skill, of these cooldowns, is triggered
pub(crate) fn simulate_rotation(cds: &[f64], source_rate: f64, tick: f64) -> (f64, Vec<f64>) {
    let increment = 1.0 / source_rate;
    let sim_time = increment * 1000.0;
    let n = cds.len();
    let mut next_trig = vec![0.0; n];
    let mut count = vec![0.0; n];
    let mut rotation_index = 0;
    let mut next_trigger = 0.0;
    while next_trigger < sim_time {
        let mut current = rotation_index;
        loop {
            if next_trig[current] <= next_trigger {
                count[current] += 1.0;
                next_trig[current] = ceil_b(floor_b(next_trigger, tick) + cds[current], tick);
                break;
            }
            current = (current + 1) % n;
            if current == rotation_index {
                break;
            }
        }
        rotation_index = (rotation_index + 1) % n;
        next_trigger += increment;
    }
    (sim_time, count)
}

/// `ceil_b(x, base)` and `floor_b(x, base)` of `Modules/Common.lua`
fn ceil_b(x: f64, base: f64) -> f64 {
    base * (x / base).ceil()
}

fn floor_b(x: f64, base: f64) -> f64 {
    base * (x / base).floor()
}

impl<'a> Perform<'a> {
    /// `cacheSkillUUID(skill, env)`
    pub(crate) fn skill_uuid(&self, i: usize) -> String {
        crate::calc_cache::skill_uuid(self.skill(i), &self.env.groups)
    }

    /// `isTriggered(skill)`
    pub(crate) fn is_triggered(&self, i: usize) -> bool {
        let skill = self.skill(i);
        self.sd_truthy(i, "triggeredByUnique")
            || self.sd_truthy(i, "triggered")
            || self.has_type(i, "InbuiltTrigger")
            || self.has_type(i, "Triggered")
            || skill
                .active_effect
                .src
                .and_then(|(g, k)| self.env.groups.get(g)?.gem_list.get(k))
                .is_some_and(|g| g.triggered)
    }

    /// `getUniqueItemTriggerName(skill)`
    fn unique_trigger_name(&self, i: usize) -> Option<String> {
        if let Some(ModValue::Str(t)) = self.sd(i, "triggerSource") {
            return Some(t.to_string());
        }
        let skill = self.skill(i);
        for e in &skill.support_list {
            if e.granted_effect.from_item && !e.granted_effect.support {
                return Some(e.granted_effect.name.clone());
            }
        }
        let src = skill
            .group
            .and_then(|g| self.env.groups.get(g))
            .and_then(|g| g.source.clone())?;
        crate::calc_offence::unique_trigger_name(&src)
    }

    /// `calcs.triggers(env, actor)`
    pub(crate) fn triggers(&mut self, actor: usize) -> Result<()> {
        let Some(main) = self.actor_main(actor) else {
            return Ok(());
        };
        if self.has_flag(main, "disable") || self.is_limited(main) {
            return Ok(());
        }
        let skill = self.skill(main);
        let ge = &skill.active_effect.granted_effect;
        let skill_name = ge.name.clone();
        let trigger_name = self
            .mods(main)
            .triggered_by
            .map(|t| skill.effect_list[t].granted_effect.name.clone());
        let unique = if self.is_triggered(main) {
            self.unique_trigger_name(main)
        } else {
            None
        };
        let mut keys = vec![ge.id.clone(), skill_name.to_lowercase()];
        if let Some(t) = &trigger_name {
            let t = t.to_lowercase();
            keys.push(t.clone());
            keys.push(crate::pat::gsub(&t, "^awakened ", ""));
        }
        if let Some(u) = &unique {
            keys.push(u.to_lowercase());
        }
        let mut config = None;
        for k in &keys {
            if crate::calc_offence::TRIGGER_CONFIGS.contains(&k.as_str()) {
                config = self.trigger_config(k)?;
                if config.is_some() {
                    break;
                }
            }
        }
        match config {
            Some(mut config) => {
                config.actor.get_or_insert(actor);
                if config.trigger_name.is_none() {
                    config.trigger_name = trigger_name.or(Some(skill_name)).or(unique);
                }
                if config.trigger_chance.is_none() {
                    config.trigger_chance = self
                        .skill(main)
                        .active_effect
                        .src
                        .and_then(|(g, k)| self.env.groups.get(g)?.gem_list.get(k))
                        .and_then(|g| g.trigger_chance);
                }
                match config.custom_handler {
                    Some(h) => h(self),
                    None => self.default_trigger_handler(config),
                }
            }
            None => {
                self.sd_clear(main, "triggered");
                Ok(())
            }
        }
    }

    /// The configuration of the trigger `key` (`configTable[key](env)`),
    /// or `None` when it does not apply
    fn trigger_config(&mut self, key: &str) -> Result<Option<TriggerConfig>> {
        let main = self.main();
        let set_global = |p: &mut Self| p.set_flag(main, "globalTrigger", true);
        fn melee_or_attack(p: &Perform, s: usize) -> bool {
            p.has_type(s, "Melee") || p.has_type(s, "Attack")
        }
        fn damage_or_attack(p: &Perform, s: usize) -> bool {
            p.has_type(s, "Damage") || p.has_type(s, "Attack")
        }
        let named = |name: &str, cond: fn(&Perform, usize) -> bool| TriggerConfig {
            trigger_name: Some(name.into()),
            trigger_skill_cond: Some(cond),
            ..TriggerConfig::default()
        };
        let kills = |cond: fn(&Perform, usize) -> bool| TriggerConfig {
            assuming_every_hit_kills: true,
            trigger_skill_cond: Some(cond),
            ..TriggerConfig::default()
        };
        Ok(Some(match key {
            "nova" => {
                let Some(a) = self.minion_actor() else {
                    return Ok(None);
                };
                let Some(m) = self.actor_main(a) else {
                    return Ok(None);
                };
                TriggerConfig {
                    trigger_name: Some("Summon Holy Relic".into()),
                    actor: Some(a),
                    triggered_skills: vec![SimSkill {
                        uuid: self.skill_uuid(m),
                        cd: self.sd_num(m, "cooldown"),
                        cd_override: None,
                        adds_cast_time: None,
                        icdr: 1.0,
                        added_cooldown: 0.0,
                    }],
                    trigger_skill_cond: Some(|p, s| p.has_type(s, "Attack")),
                    ..TriggerConfig::default()
                }
            }
            "cast on critical strike" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Attack") && p.slot_match(s)),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredByCoc") && p.slot_match(s)
                }),
                ..TriggerConfig::default()
            },
            "cast on melee kill" => {
                let pdb = self.pdb();
                if !self.c.flag(pdb, None, &["Condition:KilledRecently"])? {
                    let mods = self.mods_mut(main);
                    mods.info_message2 = Some("DPS reported assuming Self-Cast".into());
                    mods.info_message = Some("Cast on Melee Kill requires recent kills".into());
                    return Ok(None);
                }
                TriggerConfig {
                    assuming_every_hit_kills: true,
                    trigger_skill_cond: Some(|p, s| {
                        p.has_type(s, "Attack") && p.has_type(s, "Melee") && p.slot_match(s)
                    }),
                    triggered_skill_cond: Some(|p, s| {
                        p.sd_truthy(s, "triggeredByMeleeKill") && p.slot_match(s)
                    }),
                    ..TriggerConfig::default()
                }
            }
            "arcanist brand" => {
                if self.skill_name(main) == "Arcanist Brand" {
                    return Ok(None);
                }
                self.sd_set(main, "sourceRateIsFinal", true);
                let brand = (0..self.n_player)
                    .find(|&k| self.skill_name(k) == "Arcanist Brand")
                    .ok_or("attempt to index a nil value (field 'mainSkill')")?;
                let (bl, bc) = (self.lists[brand].list, self.cfg(brand));
                let inc = (100.0
                    + self
                        .c
                        .inc(bl, Some(&bc), &["Speed", "BrandActivationFrequency"])?)
                    / 100.0;
                let more = self.c.more(bl, Some(&bc), &["BrandActivationFrequency"])?;
                let attached = self.sd_num(brand, "attachedBrandCount").unwrap_or(f64::NAN);
                let freq = self.sd_num(brand, "repeatFrequency").unwrap_or(f64::NAN);
                TriggerConfig {
                    trig_rate: Some(freq * inc * more * attached),
                    source: Some(brand),
                    triggered_skill_cond: Some(|p, s| {
                        p.sd_truthy(s, "triggeredByBrand") && p.slot_match(s)
                    }),
                    brand: Some((brand, inc, more)),
                    ..TriggerConfig::default()
                }
            }
            "cast when stunned" => {
                set_global(self);
                TriggerConfig {
                    trigger_chance: self.sd_num(main, "chanceToTriggerOnStun"),
                    source: Some(main),
                    ..TriggerConfig::default()
                }
            }
            "cast on ward break" => {
                set_global(self);
                TriggerConfig {
                    trigger_chance: self.sd_num(main, "chanceToTriggerOnWardBreak"),
                    source: Some(main),
                    ..TriggerConfig::default()
                }
            }
            "replica eternity shroud" | "shroud of the lightless" => {
                set_global(self);
                TriggerConfig {
                    source: Some(main),
                    ..TriggerConfig::default()
                }
            }
            "uul-netol's embrace" => named("Bone Nova", damage_or_attack),
            "duskblight" => named("Stalking Pustule", damage_or_attack),
            "jorrhast's blacksteel" | "rigwald's crest" | "ashcaller" => {
                self.sd_set(main, "sourceRateIsFinal", true);
                kills(damage_or_attack)
            }
            "cospri's malice" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| {
                    let sword = p.mf("Sword") | p.mf("Weapon1H");
                    p.has_type(s, "Melee") && p.cfg(s).flags.unwrap_or(0) & sword > 0
                }),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredByCospris")
                        && p.group_slot(p.main()) == p.group_slot(s)
                }),
                ..TriggerConfig::default()
            },
            "cast when damage taken" => {
                let Some(base) = self.sd_num(main, "triggeredByDamageTaken") else {
                    return Ok(None);
                };
                let list = self.lists[main].list;
                let threshold = self.c.calc_mod(list, None, &["CWDTThreshold"])?;
                self.c.set_out(PLAYER, "CWDTThreshold", base * threshold);
                if base * threshold != base
                    && let Some(b) = self.bd(PLAYER)
                {
                    b.set_lines(
                        "CWDTThreshold",
                        vec![
                            sf!("%.2f ^8(base threshold)", base),
                            sf!("x %.2f ^8(threshold modifier)", threshold),
                            sf!("= %.2f", base * threshold),
                        ],
                    );
                }
                set_global(self);
                TriggerConfig {
                    source: Some(main),
                    ..TriggerConfig::default()
                }
            }
            "oskarm" => {
                self.sd_set(main, "sourceRateIsFinal", true);
                TriggerConfig {
                    trigger_skill_cond: Some(|p, s| p.has_type(s, "Attack")),
                    ..TriggerConfig::default()
                }
            }
            "cast on death" => {
                set_global(self);
                self.sd_set(main, "triggered", true);
                return Ok(None);
            }
            "prismatic burst" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Attack") && p.slot_match(s)),
                ..TriggerConfig::default()
            },
            "falling crystal" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Melee") && p.slot_match(s)),
                ..TriggerConfig::default()
            },
            "call the pyre" => {
                let edb = self.edb();
                if !self.c.flag(edb, None, &["Condition:Ignited"])? {
                    return Ok(None);
                }
                TriggerConfig {
                    trigger_chance: Some(50.0),
                    trigger_skill_cond: Some(|p, s| p.has_type(s, "Melee") && p.slot_match(s)),
                    ..TriggerConfig::default()
                }
            }
            "avenging flame" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_flag(s, "totem") && p.slot_match(s)),
                comparer: Some(|c, source, life| {
                    let totem = c.out("TotemLife");
                    match source {
                        None => totem.is_some(),
                        Some(_) => totem.is_some_and(|t| t > life.unwrap_or(0.0)),
                    }
                }),
                ignore_source_rate: true,
                ..TriggerConfig::default()
            },
            "BloodShrineUniqueTriggeredExplodingToad" => {
                let chance = self
                    .skill(main)
                    .active_effect
                    .src
                    .and_then(|(g, k)| self.env.groups.get(g)?.gem_list.get(k))
                    .and_then(|g| g.trigger_chance)
                    .ok_or(
                        "attempt to perform arithmetic on field 'triggerChance' (a nil value)",
                    )?;
                let pdb = self.pdb();
                let chance = chance
                    + self
                        .c
                        .base(pdb, None, &["BloodShrineExplodingToadTriggerChance"])?;
                TriggerConfig {
                    trigger_chance: Some(m_min(chance, 100.0)),
                    trigger_skill_cond: Some(|p, s| {
                        p.has_type(s, "Damage") || p.has_type(s, "Attack")
                    }),
                    ..TriggerConfig::default()
                }
            }
            "bursting toad" => {
                let interval = (0..self.n_player)
                    .filter_map(|k| self.sd_num(k, "hextoadTriggerInterval"))
                    .fold(f64::INFINITY, m_min);
                if interval == f64::INFINITY {
                    return Ok(None);
                }
                set_global(self);
                self.sd_set(main, "triggerRateCapOverride", 1.0 / interval);
                if let Some(b) = self.bd(PLAYER) {
                    b.set_lines(
                        "TriggerRateCap",
                        vec![
                            sf!("1 / %.2f ^8(Hextoad trigger interval)", interval),
                            sf!("= %.2f", 1.0 / interval),
                        ],
                    );
                }
                TriggerConfig {
                    source: Some(main),
                    ..TriggerConfig::default()
                }
            }
            "cast while channelling" => TriggerConfig {
                custom_handler: Some(|p| p.cwc_handler()),
                ..TriggerConfig::default()
            },
            "tempest shield" => {
                set_global(self);
                TriggerConfig {
                    source: Some(main),
                    ..TriggerConfig::default()
                }
            }
            "autoexertion" | "automation" | "call to arms" => {
                let name = match key {
                    "autoexertion" => "Autoexertion",
                    "automation" => "Automation",
                    _ => "Call to Arms",
                };
                let mut trigger_effect = None;
                if self.skill_name(main) == name {
                    set_global(self);
                    self.set_flag(main, "skipEffectiveRate", true);
                } else {
                    // The trigger's cooldown comes from the active part
                    trigger_effect = (0..self.n_player).find(|&i| self.skill_name(i) == name);
                }
                if key != "automation" {
                    self.sd_set(main, "sourceRateIsFinal", true);
                    self.sd_set(main, "ignoresTickRate", true);
                }
                TriggerConfig {
                    trigger_on_use: true,
                    use_cast_rate: true,
                    source: Some(main),
                    trigger_effect,
                    ..TriggerConfig::default()
                }
            }
            "doom blast" => {
                let pdb = self.pdb();
                if self
                    .input
                    .config
                    .get("doomBlastSource")
                    .is_some_and(|v| v.is_str("replacement"))
                {
                    self.c
                        .add(pdb, src("UsesCurseOverlaps", ModType::Flag, true, "Config"));
                }
                self.sd_set(main, "ignoresTickRate", true);
                let overlaps = "Multiplier:CurseOverlaps";
                let overlaps = if self
                    .c
                    .tabulate(pdb, Some(ModType::Base), None, &[overlaps])?
                    .is_empty()
                {
                    None
                } else {
                    Some(m_max(self.c.base(pdb, None, &[overlaps])?, 1.0))
                };
                TriggerConfig {
                    overlaps,
                    use_cast_rate: true,
                    trigger_skill_cond: Some(|p, s| p.has_type(s, "Hex") && p.slot_match(s)),
                    ..TriggerConfig::default()
                }
            }
            "FieryImpactHeistMaceImplicit" => TriggerConfig {
                source_weapon: true,
                trigger_skill_cond: Some(melee_or_attack),
                ..TriggerConfig::default()
            },
            "law of the wilds" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| {
                    !p.has_type(s, "SummonsTotem")
                        && (p.has_type(s, "Melee") || p.has_type(s, "Attack"))
                        && p.cfg(s).flags.unwrap_or(0) & p.mf("Claw") > 0
                }),
                ..TriggerConfig::default()
            },
            "the rippling thoughts" | "the surging thoughts" => {
                if self.skill_name(main) != "Storm Cascade" {
                    return Ok(None);
                }
                TriggerConfig {
                    trigger_on_use: key == "the surging thoughts",
                    trigger_skill_cond: Some(melee_or_attack),
                    ..TriggerConfig::default()
                }
            }
            "the hidden blade" => {
                set_global(self);
                self.sd_set(main, "triggerRateCapOverride", 2.0);
                let pdb = self.pdb();
                if self.c.flag(pdb, None, &["Condition:Phasing"])? {
                    if let Some(b) = self.bd(PLAYER) {
                        b.set_lines(
                            "TriggerRateCap",
                            vec![
                                sf!(
                                    "%.2f ^8(Unseen Strike from The Hidden Blade has no cooldown but is still triggered)",
                                    2.0
                                ),
                                sf!("= %.2f", 2.0),
                            ],
                        );
                    }
                    TriggerConfig {
                        source: Some(main),
                        ..TriggerConfig::default()
                    }
                } else {
                    self.set_flag(main, "disable", true);
                    self.mods_mut(main).disable_reason =
                        Some("This skill is requires you to be phasing".into());
                    return Ok(None);
                }
            }
            "limbsplit" | "the cauteriser" => named("Gore Shockwave", melee_or_attack),
            "lioneye's paws" | "replica lioneye's paws" => {
                self.sd_set(main, "cooldown", 1.0);
                TriggerConfig {
                    trigger_on_use: true,
                    trigger_skill_cond: Some(|p, s| {
                        p.has_type(s, "Attack") && p.cfg(s).flags.unwrap_or(0) & p.mf("Bow") > 0
                    }),
                    ..TriggerConfig::default()
                }
            }
            "moonbender's wing" => {
                self.sd_set(main, "cooldown", 1.0);
                named("Lightning Warp", melee_or_attack)
            }
            "ngamahu's flame" => named("Molten Burst", melee_or_attack),
            "cameria's avarice" => named("Icicle Burst", damage_or_attack),
            "starcaller" => named("Starfall", melee_or_attack),
            "arakaali's fang" | "sporeguard" | "mark of the elder" | "mark of the shaper" => {
                kills(damage_or_attack)
            }
            "poet's pen" => TriggerConfig {
                trigger_on_use: true,
                trigger_skill_cond: Some(|p, s| {
                    (p.has_type(s, "Damage") || p.has_type(s, "Attack"))
                        && p.cfg(s).flags.unwrap_or(0) & p.mf("Wand") > 0
                }),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredByUnique")
                        && p.group_slot(p.main()) == p.group_slot(s)
                        && p.has_type(s, "Spell")
                }),
                ..TriggerConfig::default()
            },
            "maloney's mechanism" => {
                let slot = self.cfg(main).slot_name;
                let item = slot
                    .and_then(|s| self.item(PLAYER, s.as_str()))
                    .ok_or("attempt to index a nil value (field '?')")?;
                let name = crate::calc_offence::unique_trigger_name(&item.mod_source)
                    .ok_or("attempt to index a nil value (local 'uniqueTriggerName')")?;
                let replica = crate::pat::find(&name, "Replica.").is_some();
                TriggerConfig {
                    trigger_on_use: true,
                    trigger_name: Some(name),
                    use_cast_rate: replica,
                    trigger_skill_cond: Some(if replica {
                        |p, s| p.has_type(s, "Spell")
                    } else {
                        |p, s| {
                            p.has_type(s, "Attack") && p.cfg(s).flags.unwrap_or(0) & p.mf("Bow") > 0
                        }
                    }),
                    triggered_skill_cond: Some(|p, s| {
                        p.sd_truthy(s, "triggeredByUnique")
                            && p.group_slot(p.main()) == p.group_slot(s)
                            && p.has_type(s, "RangedAttack")
                    }),
                    ..TriggerConfig::default()
                }
            }
            "asenath's chant" => TriggerConfig {
                trigger_on_use: true,
                trigger_skill_cond: Some(|p, s| {
                    (p.has_type(s, "Damage") || p.has_type(s, "Attack"))
                        && p.cfg(s).flags.unwrap_or(0) & p.mf("Bow") > 0
                }),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredByUnique")
                        && p.group_slot(p.main()) == p.group_slot(s)
                        && p.has_type(s, "Spell")
                }),
                ..TriggerConfig::default()
            },
            "vixen's entrapment" => TriggerConfig {
                use_cast_rate: true,
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Hex")),
                ..TriggerConfig::default()
            },
            "flames of judgement" | "storm of judgement" => {
                self.sd_set(main, "sourceRateIsFinal", true);
                TriggerConfig {
                    trigger_name: Some(self.skill_name(main).to_string()),
                    trigger_skill_cond: Some(|p, s| p.skill_name(s) == "Queen's Demand"),
                    triggered_skill_cond: Some(|p, s| {
                        p.sd_truthy(s, "triggeredByUnique")
                            && p.group_slot(p.main()) == p.group_slot(s)
                    }),
                    ..TriggerConfig::default()
                }
            }
            "trigger craft" => {
                if !self.sd_truthy(main, "triggeredByCraft") {
                    return Ok(None);
                }
                let (mut source, mut rate, mut use_cast_rate) = (None, None, false);
                let mut triggered = Vec::new();
                for i in 0..self.n_player {
                    if (self.has_type(i, "Damage")
                        || self.has_type(i, "Attack")
                        || self.has_type(i, "Spell"))
                        && !self.has_flag(i, "aura")
                        && i != main
                        && !self.sd_truthy(i, "triggeredByCraft")
                        && !self.skill(i).active_effect.granted_effect.from_item
                        && !self.is_triggered(i)
                    {
                        (source, rate) = self.find_trigger_skill(i, source, rate, None, None)?;
                        let ae = &self.skill(i).active_effect;
                        if ["totem", "golem", "banner", "ballista"]
                            .iter()
                            .any(|f| self.has_flag(i, f))
                            && let Some(t) = ae.granted_effect.cast_time
                        {
                            let cd = ae
                                .level
                                .and_then(|l| ae.granted_effect.level(l))
                                .and_then(|l| l.field("cooldown"))
                                .unwrap_or(0.0);
                            rate = Some(1.0 / (t + cd));
                            use_cast_rate = true;
                        }
                    }
                    if self.sd_truthy(i, "triggeredByCraft")
                        && self.group_slot(main) == self.group_slot(i)
                    {
                        triggered.push(self.package_skill(i)?);
                    }
                }
                TriggerConfig {
                    trig_rate: rate,
                    source,
                    use_cast_rate,
                    triggered_skills: triggered,
                    ..TriggerConfig::default()
                }
            }
            "kitava's thirst" | "foulborn kitava's thirst" => {
                let pdb = self.pdb();
                let (chance, name, stat, min) = if key == "kitava's thirst" {
                    (
                        "KitavaTriggerChance",
                        "Kitava's Thirst",
                        "ManaCost",
                        "KitavaRequiredManaCost",
                    )
                } else {
                    (
                        "FoulbornKitavaTriggerChance",
                        "Foulborn Kitava's Thirst",
                        "LifeCost",
                        "FoulbornKitavaRequiredLifeCost",
                    )
                };
                TriggerConfig {
                    trigger_chance: Some(self.c.base(pdb, None, &[chance])?),
                    trigger_name: Some(name.into()),
                    minimum_cost: Some((stat, self.c.base(pdb, None, &[min])?)),
                    trigger_skill_cond: Some(|_, _| true),
                    ..TriggerConfig::default()
                }
            }
            "mjolner" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| {
                    (p.has_type(s, "Damage") || p.has_type(s, "Attack"))
                        && p.cfg(s).flags.unwrap_or(0) & (p.mf("Mace") | p.mf("Weapon1H")) > 0
                        && !p.slot_match(s)
                }),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredByMjolner") && p.slot_match(s)
                }),
                ..TriggerConfig::default()
            },
            "wing of the wyvern" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| {
                    (p.has_type(s, "Damage") || p.has_type(s, "Attack"))
                        && p.cfg(s).flags.unwrap_or(0) & p.mf("Bow") > 0
                        && !p.slot_match(s)
                }),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredByUnique") && p.slot_match(s)
                }),
                ..TriggerConfig::default()
            },
            "seven teachings" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| {
                    let both = p.mf("Unarmed") | p.mf("Melee");
                    p.has_type(s, "Melee") && p.cfg(s).flags.unwrap_or(0) & both == both
                }),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredBySevenTeachings") && p.slot_match(s)
                }),
                ..TriggerConfig::default()
            },
            "squirming terror" => {
                let pdb = self.pdb();
                if !self.c.flag(pdb, None, &["Condition:KilledRecently"])? {
                    let mods = self.mods_mut(main);
                    mods.info_message2 = Some("DPS reported assuming Self-Cast".into());
                    mods.info_message = Some("Squirming Terror requires recent kills".into());
                    return Ok(None);
                }
                TriggerConfig {
                    assuming_every_hit_kills: true,
                    trigger_skill_cond: Some(|p, s| {
                        p.has_type(s, "Attack") && p.has_type(s, "Melee")
                    }),
                    triggered_skill_cond: Some(|p, s| {
                        p.sd_truthy(s, "triggeredBySquirmingTerror") && p.slot_match(s)
                    }),
                    ..TriggerConfig::default()
                }
            }
            "kinetic flux" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Attack") && p.slot_match(s)),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredByKineticFlux") && p.slot_match(s)
                }),
                ..TriggerConfig::default()
            },
            "spellslinger" => {
                if self.skill_name(main) == "Spellslinger" {
                    self.set_flag(main, "skipEffectiveRate", true);
                }
                self.sd_set(main, "sourceRateIsFinal", true);
                TriggerConfig {
                    trigger_name: Some("Spellslinger".into()),
                    trigger_on_use: true,
                    trigger_skill_cond: Some(|p, s| {
                        p.has_type(s, "Attack")
                            && p.has_type(s, "Projectile")
                            && p.cfg(s).flags.unwrap_or(0) & p.mf("Wand") > 0
                            && !p.sd_truthy(s, "triggeredBySpellSlinger")
                    }),
                    ..TriggerConfig::default()
                }
            }
            "seize the flesh" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Warcry") && p.slot_match(s)),
                ..TriggerConfig::default()
            },
            "fissure" => TriggerConfig {
                trigger_on_use: true,
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Slam") && p.slot_match(s)),
                ..TriggerConfig::default()
            },
            "mark on hit" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Attack")),
                ..TriggerConfig::default()
            },
            "hextouch" => {
                self.sd_set(main, "sourceRateIsFinal", true);
                TriggerConfig {
                    trigger_skill_cond: Some(|p, s| p.has_type(s, "Attack") && p.slot_match(s)),
                    ..TriggerConfig::default()
                }
            }
            "shattershard" => {
                set_global(self);
                let c = self
                    .cached_calc(main, true, true)?
                    .ok_or("attempt to index a nil value (field '?')")?;
                let duration = c.out("Duration").unwrap_or(f64::NAN);
                self.sd_set(main, "triggerRateCapOverride", 1.0 / duration);
                if let Some(b) = self.bd(PLAYER) {
                    b.set_lines(
                        "SkillTriggerRate",
                        vec![
                            "Shattershard uses duration as pseudo cooldown".into(),
                            sf!("1 / %.2f ^8(Shattershard duration)", duration),
                            sf!("= %.2f ^8per second", 1.0 / duration),
                        ],
                    );
                }
                TriggerConfig {
                    source: Some(main),
                    ..TriggerConfig::default()
                }
            }
            "battlemage's cry" | "combust" => {
                if key == "battlemage's cry" && self.skill_name(main) == "Battlemage's Cry" {
                    return Ok(None);
                }
                TriggerConfig {
                    trigger_skill_cond: Some(|p, s| p.has_type(s, "Melee")),
                    comparer: Some(if key == "combust" {
                        |c, source, rate| {
                            default_comparer(c, source, rate)
                                && c.out("InfernalUpTimeRatio").is_some()
                        }
                    } else {
                        |c, source, rate| {
                            default_comparer(c, source, rate)
                                && c.out("BattlemageUpTimeRatio").is_some()
                        }
                    }),
                    triggered_skill_cond: (key == "battlemage's cry").then_some(
                        |p: &Perform, s| {
                            p.sd_truthy(s, "triggeredByBattleMageCry") && p.slot_match(s)
                        },
                    ),
                    ..TriggerConfig::default()
                }
            }
            "voidstorm" => TriggerConfig {
                trigger_on_use: true,
                trigger_skill_cond: Some(|p, s| {
                    p.has_type(s, "Attack") && p.has_type(s, "Rain") && p.slot_match(s)
                }),
                ..TriggerConfig::default()
            },
            "shockwave" | "void shockwave" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Melee") && p.slot_match(s)),
                ..TriggerConfig::default()
            },
            "manaforged arrows" => TriggerConfig {
                trigger_on_use: true,
                trigger_name: Some("Manaforged Arrows".into()),
                trigger_skill_cond: Some(|p, s| {
                    p.has_type(s, "Attack") && p.cfg(s).flags.unwrap_or(0) & p.mf("Bow") > 0
                }),
                ..TriggerConfig::default()
            },
            "focus" => TriggerConfig {
                custom_handler: Some(|p| p.helmet_focus_handler()),
                ..TriggerConfig::default()
            },
            "snipe" => return self.snipe_config(),
            "intuitive link" => {
                if self.skill_name(main) == "Intuitive Link" {
                    return Ok(None);
                }
                let link =
                    |p: &Self| (0..p.n_player).find(|&k| p.skill_name(k) == "Intuitive Link");
                let pdb = self.pdb();
                let rate = self.c.base(pdb, None, &["IntuitiveLinkSourceRate"])?;
                TriggerConfig {
                    triggered_skill_cond: Some(|p, s| {
                        p.has_type(s, "Spell")
                            && p.slot_match(s)
                            && Some(s)
                                != (0..p.n_player).find(|&k| p.skill_name(k) == "Intuitive Link")
                    }),
                    trig_rate: Some(rate),
                    source: link(self),
                    source_name: Some("Custom source".into()),
                    use_cast_rate: true,
                    ..TriggerConfig::default()
                }
            }
            "svalinn cast on block" | "festering resentment cast on block" => {
                set_global(self);
                TriggerConfig {
                    source: Some(main),
                    triggered_skill_cond: Some(|p, s| p.slot_match(s) && p.trigger_supports(s)),
                    ..TriggerConfig::default()
                }
            }
            "hex on trap" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Trapped")),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredByTrapTrigger") && p.slot_match(s)
                }),
                ..TriggerConfig::default()
            },
            "supporttriggerfirespellonhit" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Melee")),
                triggered_skill_cond: Some(|p, s| {
                    p.sd_truthy(s, "triggeredBySettlersEnchantTrigger") && p.slot_match(s)
                }),
                ..TriggerConfig::default()
            },
            "ghostly artillery" => TriggerConfig {
                source_weapon: true,
                trigger_on_use: true,
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Attack")),
                ..TriggerConfig::default()
            },
            "replica gifts from above" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| {
                    !p.has_type(s, "SummonsTotem") && p.has_type(s, "Attack")
                }),
                ..TriggerConfig::default()
            },
            "TriggeredMoltenStrike" => TriggerConfig {
                trigger_skill_cond: Some(melee_or_attack),
                ..TriggerConfig::default()
            },
            k => return Err(format!("not ported: the trigger {k}")),
        }))
    }

    /// `env.build.configTab.input["doomBlastSource"]`
    fn doom_blast_source(&self) -> Option<&'a str> {
        match self.input.config.get("doomBlastSource") {
            Some(crate::config::Val::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// `findTriggerSkill(env, skill, source, triggerRate, comparer)`: the
    /// skill if its cached calculation beats the source, by default its rate
    fn find_trigger_skill(
        &self,
        skill: usize,
        source: Option<usize>,
        rate: Option<f64>,
        comparer: Option<Comparer>,
        minimum_cost: Option<(&str, f64)>,
    ) -> Result<(Option<usize>, Option<f64>)> {
        if let Some(c) = self.cached_calc(skill, false, true)? {
            let speed = c.out("HitSpeed").or_else(|| c.out("Speed"));
            let better = comparer.unwrap_or(default_comparer)(&c, source, rate)
                && minimum_cost.is_none_or(|(stat, min)| c.out(stat).unwrap_or(0.0) >= min);
            let used_by_mirage = self
                .cfg(skill)
                .skill_cond
                .as_ref()
                .is_some_and(|c| c.contains(&crate::mods::Sym::new("usedByMirage")));
            if better
                && !self.has_flag(skill, "disable")
                && !used_by_mirage
                && !self.has_type(skill, "OtherThingUsesSkill")
            {
                return Ok((Some(skill), speed));
            }
        }
        Ok((source, rate))
    }

    /// `slotMatch(env, skill)`: the skill is in the main skill's socket
    /// group, or in its slot when either comes from an item
    pub(crate) fn slot_match(&self, i: usize) -> bool {
        let main = self.main();
        let gem_from_item = |k: usize| {
            self.skill(k)
                .active_effect
                .src
                .and_then(|(g, x)| self.env.groups.get(g)?.gem_list.get(x))
                .is_some_and(|g| g.from_item)
        };
        let ge_from_item = |k: usize| self.skill(k).active_effect.granted_effect.from_item;
        let from_item =
            ge_from_item(main) || ge_from_item(i) || gem_from_item(main) || gem_from_item(i);
        let group = |k: usize| self.skill(k).group;
        let slot = |k: usize| {
            group(k)
                .and_then(|g| self.env.groups.get(g))
                .and_then(|g| g.slot.clone())
        };
        let match1 = from_item && group(i).is_some() && slot(i) == slot(main);
        let match2 = !ge_from_item(main) && group(i) == group(main);
        match1 || match2
    }

    /// `packageSkillDataForSimulation(skill, env)`
    fn package_skill(&self, i: usize) -> Result<SimSkill> {
        let (list, cfg) = (self.lists[i].list, self.cfg(i));
        let c = Some(&cfg);
        Ok(SimSkill {
            uuid: self.skill_uuid(i),
            cd: self.sd_num(i, "cooldown"),
            cd_override: self.c.override_num(list, c, &["CooldownRecovery"])?,
            adds_cast_time: self.added_cast_time(i)?.map(|(t, _)| t),
            icdr: self.c.calc_mod(list, c, &["CooldownRecovery"])?,
            added_cooldown: self.c.base(list, c, &["CooldownRecovery"])?,
        })
    }

    /// `processAddedCastTime(skill)`: the cast time a triggered spell adds
    /// to its cooldown, with its cast speed
    fn added_cast_time(&self, i: usize) -> Result<Option<(f64, f64)>> {
        let (list, cfg) = (self.lists[i].list, self.cfg(i));
        let c = Some(&cfg);
        if !self
            .c
            .flag(list, c, &["SpellCastTimeAddedToCooldownIfTriggered"])?
        {
            return Ok(None);
        }
        let base = self
            .sd_num(i, "castTimeOverride")
            .or(self.skill(i).active_effect.granted_effect.cast_time)
            .unwrap_or(1.0);
        let inc = self.c.inc(list, c, &["Speed"])?;
        let more = self.c.more(list, c, &["Speed"])?;
        let csi = round((1.0 + inc / 100.0) * more, 2.0);
        Ok(Some((base / csi, csi)))
    }

    /// `breakdown.AddedCastTime` of `processAddedCastTime(skill,
    /// breakdown)`
    fn added_cast_time_lines(&self, i: usize) -> Result<Option<Vec<String>>> {
        let Some((t, _)) = self.added_cast_time(i)? else {
            return Ok(None);
        };
        let (list, cfg) = (self.lists[i].list, self.cfg(i));
        let c = Some(&cfg);
        let base = self
            .sd_num(i, "castTimeOverride")
            .or(self.skill(i).active_effect.granted_effect.cast_time)
            .unwrap_or(1.0);
        Ok(Some(vec![
            sf!("%.2f ^8(base cast time of %s)", base, self.skill_name(i)),
            sf!(
                "%.2f ^8(increased/reduced)",
                1.0 + self.c.inc(list, c, &["Speed"])? / 100.0
            ),
            sf!("%.2f ^8(more/less)", self.c.more(list, c, &["Speed"])?),
            sf!("= %.2f ^8cast time", t),
        ]))
    }

    /// `breakdown.TriggerRateCap` of the default trigger handler
    fn trigger_rate_cap_lines(&self, t: TriggerCaps) -> Vec<String> {
        let triggered_name = if t.actor == PLAYER {
            self.skill_name(t.main)
        } else {
            "Triggered skill"
        };
        let mut l = Vec::new();
        if let Some(o) = t.cd_override {
            l.push(sf!(
                "%.2f ^8(hard override of cooldown of %s)",
                o,
                triggered_name
            ));
        } else if t.triggered_adj == 0.0 {
            l.push(format!(
                "{triggered_name} has no base cooldown or cooldown override"
            ));
        } else {
            match t.triggered_cd {
                Some(cd) => l.push(sf!("%.2f ^8(base cooldown of triggered skill)", cd)),
                None => l.push(format!(
                    "{triggered_name} has no base cooldown or cooldown override"
                )),
            }
            if let Some(a) = t.added {
                l.push(sf!("+ %.2f ^8(flat added cooldown)", a));
            }
            l.push(sf!(
                "/ %.2f ^8(increased/reduced cooldown recovery)",
                t.icdr
            ));
            l.push(sf!(
                "= %.4f ^8(final cooldown of triggered skill)",
                t.triggered_adj
            ));
        }
        l.push(String::new());
        if t.actor == PLAYER {
            if t.trigger_adj == 0.0 {
                l.push(sf!("Trigger rate based on %s cooldown", triggered_name));
            } else {
                match t.trigger_cd {
                    Some(cd) => l.push(sf!("%.2f ^8(base cooldown of %s)", cd, t.name)),
                    None => l.push(format!("{} has no base cooldown", t.name)),
                }
                if let Some(a) = t.adds_cast_time {
                    l.push(sf!(
                        "+ %.2f ^8(this skill adds cast time to cooldown when triggered)",
                        a
                    ));
                }
                l.push(sf!(
                    "/ %.2f ^8(increased/reduced cooldown recovery)",
                    t.icdr
                ));
                l.push(sf!("= %.4f ^8(final cooldown of trigger)", t.trigger_adj));
            }
        }
        l.push(String::new());
        let action_adj = t
            .cd_override
            .unwrap_or(m_max(t.trigger_adj, t.triggered_adj));
        if t.triggered_adj != 0.0 && t.trigger_adj != 0.0 {
            l.push(sf!(
                "%.3f ^8(biggest of trigger cooldown and triggered skill cooldown)",
                action_adj
            ));
        }
        let rounding = (!self.sd_truthy(t.main, "ignoresTickRate") && t.triggered_adj != 0.0)
            || (self.mods(t.main).triggered_by.is_some() && !t.brand && t.trigger_adj != 0.0);
        if rounding {
            l.push(sf!("%.3f ^8(adjusted for server tick rate)", t.action_tick));
        }
        let action_cd = t.cd_override.unwrap_or(m_max(
            t.trigger_cd.unwrap_or(0.0) + t.adds_cast_time.unwrap_or(0.0),
            t.triggered_cd.unwrap_or(0.0) + t.added.unwrap_or(0.0),
        ));
        let tick_time = self.misc_data("ServerTickTime");
        let extra = |affected: f64| -> Option<f64> {
            if t.cd_override.is_some() {
                return None;
            }
            let next = t.action_tick - tick_time;
            let over = action_adj - next;
            let reduced = action_cd - action_adj;
            (over < affected).then(|| {
                let div = affected / (affected - over - reduced);
                ((div - 1.0) * 100.0).ceil() - (t.icdr - 1.0) * 100.0
            })
        };
        if let Some(x) = extra(action_cd) {
            l.push(sf!("^8(extra ICDR of %d%% would reach next breakpoint)", x));
        }
        if let Some(x) = t.adds_cast_time.and_then(extra) {
            l.push(sf!("^8(extra ICS  of %d%% would reach next breakpoint)", x));
        }
        l.push(String::new());
        if t.triggered_cd.is_none() && t.trigger_cd.is_none() && t.cd_override.is_none() {
            l.push("Assuming cast on every kill/attack/hit".into());
        } else {
            l.push("Trigger rate:".into());
            l.push(sf!("1 / %.3f", t.action_tick));
            l.push(sf!("= %.2f ^8per second", t.cap));
        }
        l
    }

    /// `calcMultiSpellRotationImpact(env, skillRotation, sourceRate,
    /// triggerCD, chance, actor)`: the main skill's rate
    fn rotation_impact(
        &self,
        rotation: Vec<SimSkill>,
        source_rate: f64,
        trigger_cd: f64,
        chance: f64,
        actor: usize,
    ) -> Result<f64> {
        Ok(self
            .rotation_sim(rotation, source_rate, trigger_cd, chance, actor)?
            .0)
    }

    /// `calcMultiSpellRotationImpact` with its table: the simulated time
    /// and each skill's rate by uuid
    fn rotation_sim(
        &self,
        mut rotation: Vec<SimSkill>,
        source_rate: f64,
        trigger_cd: f64,
        chance: f64,
        actor: usize,
    ) -> Result<Sim> {
        let tick = self.misc_data("ServerTickTime");
        let cds: Vec<f64> = rotation
            .iter()
            .map(|s| {
                let own = s
                    .cd_override
                    .unwrap_or((s.cd.unwrap_or(0.0) + s.added_cooldown) / s.icdr);
                m_max(own, (trigger_cd + s.adds_cast_time.unwrap_or(0.0)) / s.icdr)
            })
            .collect();
        let increment = 1.0 / source_rate;
        let (sim_time, count) = simulate_rotation(&cds, source_rate, tick);
        let n = rotation.len();
        let main_uuid = self
            .actor_main(actor)
            .map(|m| self.skill_uuid(m))
            .unwrap_or_default();
        let mut main_rate = 0.0;
        let mut rates = Vec::with_capacity(n);
        for (k, s) in rotation.drain(..).enumerate() {
            let rate = 1.0 / (sim_time / count[k] + (increment / chance * 100.0) - increment);
            if s.uuid == main_uuid {
                main_rate = rate;
            }
            rates.push((s.uuid, rate));
        }
        Ok((main_rate, sim_time, rates))
    }

    /// `defaultTriggerHandler(env, config)`
    fn default_trigger_handler(&mut self, config: TriggerConfig) -> Result<()> {
        let actor = config.actor.unwrap_or(PLAYER);
        let main = self
            .actor_main(actor)
            .ok_or("an actor without a main skill")?;
        let player_main = self.main();
        let mut config = config;
        let mut source = config.source;
        // `uuid`: the source's, once a skill was looked at as one
        let mut source_uuid: Option<String> = None;
        let mut triggered = config.triggered_skills.clone();
        let mut trig_rate = config.trig_rate;
        let weapon_flag = if config.source_weapon {
            let slot = self
                .skill(main)
                .group
                .and_then(|g| self.env.groups.get(g))
                .and_then(|g| g.slot.clone());
            slot.map(|s| {
                if s.starts_with("Weapon 2") {
                    "weapon2Attack"
                } else {
                    "weapon1Attack"
                }
            })
        } else {
            None
        };
        if config.triggered_skill_cond.is_some() || config.trigger_skill_cond.is_some() {
            for i in 0..self.n_player {
                if let Some(cond) = config.trigger_skill_cond
                    && cond(self, i)
                    && weapon_flag.is_none_or(|f| self.has_flag(i, f))
                    && (!self.is_triggered(i)
                        || self.has_flag(main, "globalTrigger")
                        || config.allow_triggered)
                    && i != main
                {
                    (source, trig_rate) = self.find_trigger_skill(
                        i,
                        source,
                        trig_rate,
                        config.comparer,
                        config.minimum_cost,
                    )?;
                    source_uuid = source.map(|s| self.skill_uuid(s));
                }
                if let Some(cond) = config.triggered_skill_cond
                    && cond(self, i)
                {
                    triggered.push(self.package_skill(i)?);
                }
            }
        }
        if triggered.is_empty() && config.triggered_skill_cond.is_some() {
            return Ok(());
        }
        let global = |p: &Self| p.has_flag(main, "globalTrigger");
        if source.is_none() && !(global(self) && config.triggered_skill_cond.is_some()) {
            self.sd_clear(main, "triggered");
            let name = config.trigger_name.clone().unwrap_or_default();
            let mods = self.mods_mut(main);
            mods.info_message2 = Some("DPS reported assuming Self-Cast".into());
            mods.info_message = Some(format!("No {name} Triggering Skill Found"));
            mods.info_trigger = Some(String::new());
            return Ok(());
        }
        self.sd_set(main, "triggered", true);
        let bd = self.has_bd(actor);
        // `breakdown.EffectiveSourceRate`
        let mut esr_l: Vec<String> = Vec::new();
        if bd {
            if self.sd_truthy(main, "triggeredByBrand") {
                let (brand, inc, more) = config
                    .brand
                    .ok_or("attempt to index a nil value (field 'mainSkill')")?;
                esr_l = vec![
                    sf!(
                        "%.2f ^8(base activation cooldown of %s)",
                        self.sd_num(brand, "repeatFrequency").unwrap_or(f64::NAN),
                        config.trigger_name.as_deref().unwrap_or("nil")
                    ),
                    sf!("* %.2f ^8(more activation frequency)", more),
                    sf!("* %.2f ^8(increased activation frequency)", inc),
                    sf!(
                        "* %d ^8(attached Brands)",
                        self.sd_num(brand, "attachedBrandCount").unwrap_or(f64::NAN)
                    ),
                    sf!(
                        "= %.2f ^8(activation rate of %s)",
                        trig_rate.unwrap_or(f64::NAN),
                        self.skill_name(brand)
                    ),
                ];
            } else if let (Some(r), Some(s)) = (trig_rate, source) {
                if config.assuming_every_hit_kills {
                    esr_l.push("Assuming every attack kills".into());
                }
                let what = if config.use_cast_rate {
                    "cast rate"
                } else {
                    "attack rate"
                };
                let name = config.source_name.as_deref().unwrap_or(self.skill_name(s));
                esr_l.push(sf!("%.2f ^8(%s %s)", r, name, what));
            }
        }
        let esr_insert = |l: &mut Vec<String>, at: usize, x: String| l.insert(at.min(l.len()), x);

        // Dual wield triggers
        let item_support_trigger = self.mods(main).triggered_by.is_some_and(|t| {
            let g = &self.skill(main).effect_list[t].granted_effect;
            g.support && g.from_item
        });
        if let (Some(rate), Some(s)) = (trig_rate, source) {
            let source_weapon_trigger =
                config.source_weapon && self.has_flag(s, "bothWeaponAttack");
            if self.weapon_str(PLAYER, 0, "type").is_some()
                && self.weapon_str(PLAYER, 1, "type").is_some()
                && !self.sd_truthy(s, "doubleHitsWhenDualWielding")
                && (self.has_type(s, "Melee") || self.has_type(s, "Attack"))
                && (source_weapon_trigger || item_support_trigger)
            {
                trig_rate = Some(rate / 2.0);
                if bd {
                    esr_insert(&mut esr_l, 1, "/ 2 ^8(due to dual wielding)".into());
                }
            }
        }
        let cached = match (&source_uuid, self.input.recalc) {
            (Some(u), Some(r)) => r.cached(u),
            _ => None,
        };
        if let (Some(s), Some(c)) = (source, &cached) {
            if self.c.flag(self.lists[s].list, None, &["HasSeals"])?
                && self.has_type(s, "CanRapidFire")
            {
                let mult = c
                    .skill
                    .mods
                    .as_ref()
                    .and_then(|m| m.skill_data.get("dpsMultiplier"))
                    .and_then(ModValue::as_number)
                    .unwrap_or(1.0);
                trig_rate = trig_rate.map(|r| r * mult);
                self.set_flag(main, "HasSeals", true);
                self.sd_set(main, "ignoresTickRate", true);
                if bd {
                    esr_l.push(sf!("x %.2f ^8(multiplier from Unleash)", mult));
                }
            }
            let part = self.mods(s).skill_part_name.clone().unwrap_or_default();
            if crate::pat::find(&part, "(.*)All(.*)Projectiles(.*)").is_some()
                && self.has_flag(s, "projectile")
            {
                let mult = c.out("ProjectileCount").unwrap_or(1.0);
                trig_rate = trig_rate.map(|r| r * mult);
                if bd {
                    esr_l.push(sf!("x %.2f ^8(%d projectiles hit)", mult, mult));
                }
            }
        }
        let stored = self.sd_num(main, "storedUses");
        if !self.sd_truthy(main, "ignoresTickRate") && stored.is_some_and(|s| s > 1.0) {
            self.sd_set(main, "ignoresTickRate", true);
        }
        if self.sd_truthy(main, "triggeredByManaSpent") {
            let s = source.ok_or("attempt to index a nil value (local 'source')")?;
            let repeats = 1.0 + self.c.base(self.lists[s].list, None, &["RepeatCount"])?;
            trig_rate = trig_rate.map(|r| r / repeats);
            if bd && repeats > 1.0 {
                esr_l.push(sf!(
                    "/%d ^8(repeated attacks/casts do not count as they don't use mana)",
                    repeats
                ));
            }
        }
        let melee_source = source.is_some_and(|s| self.has_type(s, "Melee"));
        // The uptime of the exerting warcry: Battlemage's Cry, Infernal Cry
        let tick_time = self.misc_data("ServerTickTime");
        for (on, prefix, cry) in [
            (
                self.sd_truthy(main, "triggeredByBattleMageCry"),
                "BattleCry",
                "BattleMageCry",
            ),
            (
                self.skill_name(main) == "Combust",
                "Infernal",
                "InfernalCry",
            ),
        ] {
            let Some(c) = cached.as_ref().filter(|_| on && melee_source) else {
                continue;
            };
            let num = |k: String| {
                c.out(&k)
                    .ok_or_else(|| format!("attempt to perform arithmetic on a nil value ({k})"))
            };
            let exerts = num(format!("{prefix}ExertsCount"))?;
            let duration = ceil_b(num(format!("{cry}Duration"))?, tick_time);
            let cast = num(format!("{cry}CastTime"))?;
            let cd = ceil_b(num(format!("{cry}Cooldown"))?, tick_time);
            let rate = trig_rate.ok_or("attempt to perform arithmetic on a nil value")?;
            let hits = m_max(m_min(rate * duration, exerts), 0.0);
            if bd {
                esr_l.push(sf!(
                    "^8(min(%.2f * %.2f, %d) = %.2f average number of exerted attacks capped by exerted count)",
                    rate,
                    duration,
                    exerts,
                    hits
                ));
                esr_l.push(sf!(
                    "= %.2f / (%.2f + %.2f) ^8(the calculated number of exerted attacks happens every cooldown + duration)",
                    hits,
                    cast,
                    cd
                ));
            }
            trig_rate = Some(hits / (cast + cd));
        }
        if self.sd_truthy(main, "triggeredByManaforged") && trig_rate.is_some_and(|r| r > 0.0) {
            let own = self
                .cached_calc(main, true, false)?
                .ok_or("attempt to index a nil value (field '?')")?;
            let cost = own.out("ManaCostRaw").unwrap_or(0.0);
            if cost > 0.0 {
                let pct = self
                    .sd_num(main, "ManaForgedArrowsPercentThreshold")
                    .ok_or("attempt to perform arithmetic on a nil value")?;
                let threshold = cost * pct;
                let source_cost = cached
                    .as_ref()
                    .ok_or("attempt to index a nil value (field '?')")?
                    .out("ManaCostRaw")
                    .unwrap_or(0.0);
                let rate = trig_rate.unwrap_or(0.0);
                if source_cost > 0.0 {
                    let uses = threshold / source_cost;
                    if bd {
                        let first = esr_l.first().cloned();
                        let mut l = vec![
                            sf!("%.4f ^8(Mana cost of trigger source)", source_cost),
                            String::new(),
                            sf!("%.4f ^8(Mana Cost of triggered)", cost),
                            sf!("%.2f ^8(Manaforged threshold multiplier)", pct),
                            sf!("= %.4f ^8(Manaforged trigger threshold)", threshold),
                            String::new(),
                            sf!("%.4f ^8(Manaforged trigger threshold)", threshold),
                            sf!("/ %.4f ^8(Mana cost of trigger source)", source_cost),
                            sf!("= %.2f ^8(Skill usages required)", uses),
                            String::new(),
                        ];
                        // A nil first line ends the list there
                        if let Some(f) = first {
                            l.push(f);
                            l.push(sf!(
                                "/ ceil(%.2f) ^8(%d Skill usages required)",
                                uses,
                                uses.ceil()
                            ));
                        }
                        esr_l = l;
                    }
                    trig_rate = Some(rate / uses.ceil());
                } else {
                    if bd {
                        esr_l.push("Source skill has no mana cost".into());
                    }
                    trig_rate = Some(0.0);
                }
            }
        }
        let by_brand = self.sd_truthy(main, "triggeredByBrand");
        if by_brand && config.brand.is_none() {
            return Err("attempt to index a nil value (field 'mainSkill')".into());
        }

        // Cooldowns
        let (list, cfg) = (self.lists[main].list, self.cfg(main));
        let c = Some(&cfg);
        let icdr = self.c.calc_mod(list, c, &["CooldownRecovery"])?;
        let added = self.c.base(list, c, &["CooldownRecovery"])?;
        let added = (added != 0.0).then_some(added);
        let cd_override = self.c.override_num(list, c, &["CooldownRecovery"])?;
        let effect_cd = |p: &Self, i: usize, by: Option<usize>| -> Result<Option<f64>> {
            let Some(t) = p.mods(i).triggered_by else {
                return Ok(None);
            };
            let e = &p.skill(i).effect_list[t];
            let ge = match by {
                Some(k) => &p.skill(k).active_effect.granted_effect,
                None => &e.granted_effect,
            };
            let level = e
                .level
                .and_then(|l| ge.level(l))
                .ok_or("attempt to index a nil value (field '?')")?;
            Ok(level.field("cooldown"))
        };
        let mut trigger_cd = if self.mods(main).triggered_by.is_some() {
            effect_cd(self, player_main, config.trigger_effect)?
        } else {
            None
        };
        if trigger_cd.is_none() {
            let s = source.ok_or("attempt to index a nil value (local 'source')")?;
            let by = config.trigger_effect.filter(|_| s == player_main);
            trigger_cd = effect_cd(self, s, by)?;
        }
        let triggered_cd = self.sd_num(main, "cooldown");
        // Brand activation is not affected by cooldown recovery
        if by_brand && let Some((brand, inc, more)) = config.brand {
            let freq = self.sd_num(brand, "repeatFrequency").unwrap_or(f64::NAN);
            trigger_cd = Some(freq / more / inc * icdr);
        }
        let adds_cast_time = self.added_cast_time(player_main)?.map(|(t, _)| t);
        if bd && let Some(l) = self.added_cast_time_lines(player_main)? {
            self.bd(actor).unwrap().set_lines("AddedCastTime", l);
        }
        match adds_cast_time {
            Some(t) => {
                self.set_flag(player_main, "addsCastTime", true);
                self.c.set_out(actor, "addsCastTime", t);
            }
            None => self.c.clear_out(actor, "addsCastTime"),
        }
        let tick_rate = self.misc_data("ServerTickRate");
        let triggered_adj = (triggered_cd.unwrap_or(0.0) + added.unwrap_or(0.0)) / icdr;
        let trigger_adj = (trigger_cd.unwrap_or(0.0) + adds_cast_time.unwrap_or(0.0)) / icdr;
        let triggered_tick = if self.sd_truthy(main, "ignoresTickRate") {
            triggered_adj
        } else {
            (triggered_adj * tick_rate).ceil() / tick_rate
        };
        // `triggeredBy.ignoresTickRate`, which only Arcanist Brand sets
        let trigger_tick = if config.brand.is_some() && self.mods(main).triggered_by.is_some() {
            trigger_adj
        } else {
            (trigger_adj * tick_rate).ceil() / tick_rate
        };
        let action_tick = match cd_override {
            Some(o) => (o * tick_rate).ceil() / tick_rate,
            None => m_max(trigger_tick, triggered_tick),
        };
        let mut cap = if source == Some(main) {
            self.sd_num(main, "triggerRateCapOverride")
                .unwrap_or(f64::INFINITY)
        } else {
            f64::INFINITY
        };
        if action_tick != 0.0 {
            cap = 1.0 / action_tick;
        }
        self.c.set_out(actor, "TriggerRateCap", cap);
        if bd && self.bd(actor).unwrap().get("TriggerRateCap").is_none() {
            let l = self.trigger_rate_cap_lines(TriggerCaps {
                actor,
                main,
                name: config.trigger_name.as_deref().unwrap_or("nil"),
                cd_override,
                triggered_cd,
                added,
                trigger_cd,
                adds_cast_time,
                icdr,
                triggered_adj,
                trigger_adj,
                action_tick,
                cap,
                brand: config.brand.is_some(),
            });
            self.bd(actor).unwrap().set_lines("TriggerRateCap", l);
        }
        let doom_source = self.doom_blast_source();
        let doom_trigger = config.trigger_name.as_deref() == Some("Doom Blast");
        let pdb = self.pdb();
        if doom_trigger && doom_source == Some("expiration") {
            // The source curse's duration, from its own calculation
            let duration = cached
                .as_ref()
                .ok_or("attempt to index a nil value (field '?')")?
                .out("Duration")
                .unwrap_or(f64::NAN);
            let expiration = 1.0 / duration;
            if bd && !esr_l.is_empty() {
                esr_l[0] = sf!("1 / %.2f ^8(source curse duration)", duration);
            }
            let rate = trig_rate.ok_or("attempt to compare nil with number")?;
            if expiration > rate {
                self.c.add(
                    pdb,
                    crate::calc_perform::src("UsesCurseOverlaps", ModType::Flag, true, "Config"),
                );
                if bd {
                    esr_insert(
                        &mut esr_l,
                        1,
                        sf!(
                            "max(%.2f, %.2f) ^8(If a curse expires instantly curse expiration is equivalent to curse replacement)",
                            expiration,
                            rate
                        ),
                    );
                    let name = source.map_or("nil", |s| self.skill_name(s));
                    esr_insert(&mut esr_l, 1, sf!("%.2f ^8(%s cast rate)", rate, name));
                }
            } else {
                trig_rate = Some(expiration);
            }
        } else if doom_trigger && doom_source == Some("hexblast") {
            let (mut hexblast, mut rate) = (None, None);
            for i in 0..self.n_player {
                if self.skill_name(i) == "Hexblast" && !self.is_triggered(i) && i != main {
                    (hexblast, rate) = self.find_trigger_skill(i, hexblast, rate, None, None)?;
                }
            }
            if hexblast.is_some() {
                let (t, r) = (
                    trig_rate.ok_or("attempt to perform arithmetic on a nil value")?,
                    rate.ok_or("attempt to perform arithmetic on a nil value")?,
                );
                trig_rate = Some(1.0 / (1.0 / t + 1.0 / r));
                if bd {
                    let l = sf!(
                        "1 / (%.2f + %.2f) ^8(sum of triggered curse and hexblast cast time)",
                        1.0 / t,
                        1.0 / r
                    );
                    if esr_l.is_empty() {
                        esr_l.push(l);
                    } else {
                        esr_l[0] = l;
                    }
                }
            }
        }
        let esr = if self.skill_name(player_main) == "Doom Blast" && doom_source == Some("vixen") {
            let vixen = self.item(PLAYER, "Gloves").is_some_and(|g| {
                g.title
                    .as_deref()
                    .is_some_and(|t| crate::pat::find(t, "Vixen's Entrapment").is_some())
            });
            if !vixen {
                self.c.set_out_val(
                    actor,
                    "VixenModeNoVixenGlovesWarn",
                    crate::actor::OutVal::Bool(true),
                );
            }
            self.c.add(
                pdb,
                crate::calc_perform::src("UsesCurseOverlaps", ModType::Flag, true, "Config"),
            );
            let vixens_cd = self
                .input
                .data
                .skills
                .skills
                .get("SupportUniqueCastCurseOnCurse")
                .and_then(|g| g.level(1.0)?.field("cooldown"))
                .ok_or("attempt to perform arithmetic on a nil value")?
                / icdr;
            let rate = trig_rate.ok_or("attempt to perform arithmetic on a nil value")?;
            let rotation = vec![SimSkill {
                uuid: self.skill_uuid(player_main),
                cd: None,
                cd_override: None,
                adds_cast_time: None,
                icdr,
                added_cooldown: 0.0,
            }];
            let esr = self.rotation_impact(rotation, rate, vixens_cd, 100.0, PLAYER)?;
            self.c.set_out_val(
                actor,
                "VixensTooMuchCastSpeedWarn",
                crate::actor::OutVal::Bool(vixens_cd > 1.0 / rate),
            );
            if bd {
                esr_l.push(sf!(
                    "%.2f / %.2f = %.2f ^8(Vixen's trigger cooldown)",
                    vixens_cd * icdr,
                    icdr,
                    vixens_cd
                ));
                esr_l.push(sf!(
                    "%.2f ^8(Simulated trigger rate of a curse socketed in Vixen's given ^7%.2f ^8CD and ^7%.2f ^8source rate)",
                    esr,
                    vixens_cd,
                    rate
                ));
            }
            esr
        } else if let Some(r) = trig_rate
            && !global(self)
            && !config.ignore_source_rate
        {
            r
        } else {
            self.set_flag(main, "globalTrigger", true);
            cap
        };
        self.c.set_out(actor, "EffectiveSourceRate", esr);
        if bd {
            if !self.sd_truthy(main, "sourceRateIsFinal") {
                esr_l.push(sf!("= %.2f ^8(Effective source rate)", esr));
            }
            self.bd(actor)
                .unwrap()
                .set_lines("EffectiveSourceRate", esr_l);
        }
        let mut chance_l: Vec<String> = Vec::new();
        let mut rate_l: Option<Vec<String>> = None;

        let rate = if esr != 0.0 && !self.has_flag(player_main, "skipEffectiveRate") {
            let mut chance = 100.0;
            if let Some(s) = source
                && (self.has_type(s, "Melee") || self.has_type(s, "Attack"))
                && let Some(c) = &cached
                && !config.trigger_on_use
            {
                let dual = self.weapon_str(PLAYER, 0, "type").is_some()
                    && self.weapon_str(PLAYER, 1, "type").is_some()
                    && self.sd_truthy(s, "doubleHitsWhenDualWielding");
                let hand = |k: &str, hand: &str| -> f64 {
                    match c.output.get(&crate::mods::Sym::new(hand)) {
                        Some(crate::actor::OutVal::Table(t)) => t
                            .get(k)
                            .and_then(crate::actor::OutVal::num)
                            .unwrap_or(f64::NAN),
                        _ => f64::NAN,
                    }
                };
                let both = |k: &str| {
                    let (m, o) = (hand(k, "MainHand"), hand(k, "OffHand"));
                    m * o / 100.0 + m * (100.0 - o) / 100.0 + (100.0 - m) * o / 100.0
                };
                let hit = c.out("HitChance").unwrap_or(0.0);
                let name = self.skill_name(s).to_string();
                if hit != 100.0 {
                    if dual {
                        let h = both("HitChance");
                        chance = chance * h / 100.0;
                        chance_l.push(sf!("x %.2f%% ^8(%s effective hit chance for skills that hit with both weapons)", h, name.as_str()));
                    } else {
                        chance = chance * hit / 100.0;
                        chance_l.push(sf!("x %.2f%% ^8(%s hit chance)", hit, name.as_str()));
                    }
                }
                if self.sd_truthy(main, "triggerOnCrit") {
                    let on_crit = self.sd_num(main, "chanceToTriggerOnCrit").or_else(|| {
                        c.skill
                            .mods
                            .as_ref()
                            .and_then(|m| m.skill_data.get("chanceToTriggerOnCrit"))
                            .and_then(ModValue::as_number)
                    });
                    if config.trigger_chance.is_none() {
                        config.trigger_chance = on_crit;
                    }
                    let crit = c.out("CritChance").unwrap_or(0.0);
                    if crit != 100.0 {
                        if dual {
                            let h = both("CritChance");
                            chance = chance * h / 100.0;
                            chance_l.push(sf!("x %.2f%% ^8(%s effective crit chance for skills that hit with both weapons)", h, name.as_str()));
                        } else {
                            chance = chance * crit / 100.0;
                            chance_l.push(sf!("x %.2f%% ^8(%s crit chance)", crit, name.as_str()));
                        }
                    }
                }
            }
            if let Some(tc) = config.trigger_chance
                && tc != 100.0
            {
                chance = chance * tc / 100.0;
                chance_l.push(sf!("x %.2f%% ^8(chance to trigger)", tc));
            }
            if self.sd_truthy(main, "ignoresTickRate") && config.triggered_skill_cond.is_none() {
                let overlaps = if config.stages_are_overlaps.is_some()
                    && self.mods(player_main).skill_part == config.stages_are_overlaps
                {
                    self.stage_count(player_main)
                } else {
                    config.overlaps
                };
                rate_l = Some(vec![match overlaps {
                    Some(n) => sf!("min(%.2f, %.2f *  %d) ^8(%d overlaps)", cap, esr, n, n),
                    None => sf!("min(%.2f, %.2f)", cap, esr),
                }]);
                m_min(cap, esr * overlaps.unwrap_or(1.0))
            } else if global(self) && config.triggered_skill_cond.is_none() {
                esr
            } else {
                let rotation = if config.triggered_skill_cond.is_some() {
                    triggered.clone()
                } else {
                    vec![self.package_skill(main)?]
                };
                let cd = if by_brand {
                    0.0
                } else {
                    trigger_cd.or(triggered_cd).unwrap_or(0.0)
                };
                let (mut r, sim_time, rates) =
                    self.rotation_sim(rotation, esr, cd, chance, actor)?;
                let db = self.c.db(actor);
                let bots =
                    self.c.flag(db, None, &["HaveTriggerBots"])? && self.has_type(main, "Spell");
                if bots {
                    r *= 2.0;
                }
                let hits = if config.stages_are_overlaps.is_some()
                    && self.mods(player_main).skill_part == config.stages_are_overlaps
                {
                    self.stage_count(player_main).unwrap_or(f64::NAN)
                } else {
                    1.0
                };
                let r = hits * r;
                if bd {
                    let what = if by_brand {
                        let name = source.map_or("nil", |s| self.skill_name(s));
                        sf!("%s activations per second", name)
                    } else if trig_rate.is_none() {
                        let name = match source {
                            Some(s) => self.skill_name(s).to_string(),
                            None => match self.mods(main).triggered_by {
                                Some(t) => {
                                    self.skill(main).effect_list[t].granted_effect.name.clone()
                                }
                                None => self.skill_name(main).to_string(),
                            },
                        };
                        sf!("%s triggers per second", name.as_str())
                    } else {
                        "Effective source rate".into()
                    };
                    let mut l = vec![
                        sf!("%.2f ^8(%s)", esr, what.as_str()),
                        sf!(
                            "/ %.2f ^8(Estimated impact of skill rotation, cooldown alignment and trigger chance)",
                            m_max(esr / r, 1.0)
                        ),
                        sf!("= %.2f ^8per second", r),
                    ];
                    if bots {
                        l.insert(
                            2,
                            "x 2 ^8(Trigger bots effectively cause the skill to trigger twice)"
                                .into(),
                        );
                    }
                    if hits > 1.0 {
                        l.insert(2, sf!("x %.2f ^8(hits per triggered skill cast)", hits));
                    }
                    if chance != 100.0 {
                        l.insert(0, String::new());
                        l.insert(0, sf!("= %.2f%% ^8(Effective chance to trigger)", chance));
                        for x in std::mem::take(&mut chance_l) {
                            l.insert(0, x);
                        }
                        l.insert(0, "100% ^8(Base chance)".into());
                    }
                    rate_l = Some(l);
                    self.bd(actor)
                        .unwrap()
                        .set("SimData", sim_breakdown(sim_time, &rates));
                }
                r
            }
        } else {
            rate_l = Some(vec![
                "The trigger needs to be triggered for any skill to be triggered.".into(),
            ]);
            0.0
        };
        if bd && let Some(l) = rate_l {
            self.bd(actor).unwrap().set_lines("SkillTriggerRate", l);
        }
        self.c.set_out(actor, "SkillTriggerRate", rate);
        self.sd_set(main, "triggerRate", rate);
        self.c.set_out(actor, "Speed", rate);
        self.add_trigger_inc_more(main, source.unwrap_or(main))?;
        if let Some(s) = source
            && s != main
        {
            let uuid = self.skill_uuid(s);
            self.sd_set(main, "triggerSourceUUID", uuid.as_str());
        }
        let name = config.trigger_name.clone().unwrap_or_default();
        let mut info = match source.filter(|&s| s != main) {
            Some(s) => {
                let source_name = self.skill_name(s);
                let by = if name != source_name {
                    name.clone()
                } else {
                    self.skill_name(main).to_string()
                };
                let what = if actor == PLAYER {
                    "'s Trigger: "
                } else {
                    "'s attack Trigger: "
                };
                format!("{by}{what}{source_name}")
            }
            None => match self.mods(main).triggered_by {
                Some(t) => self.skill(main).effect_list[t].granted_effect.name.clone(),
                None => format!("{name} Trigger"),
            },
        };
        if self.sd_truthy(main, "triggeredByBrand") {
            let n = config
                .brand
                .and_then(|(b, _, _)| self.sd_num(b, "attachedBrandCount"))
                .unwrap_or(f64::NAN);
            let s = if n == 1.0 { "" } else { "s" };
            info = format!("{info}:{} attached Brand{s}", n as i64);
        }
        let mods = self.mods_mut(main);
        mods.info_message = Some(info);
        mods.info_trigger = config.trigger_name;
        Ok(())
    }

    /// `addTriggerIncMoreMods(activeSkill, sourceSkill)`
    fn add_trigger_inc_more(&mut self, i: usize, from: usize) -> Result<()> {
        let list = self.lists[i].list;
        let from_cfg = self.cfg(from);
        for typ in [ModType::Inc, ModType::More] {
            for (_, m) in self
                .c
                .tabulate(list, Some(typ), Some(&from_cfg), &["TriggeredDamage"])?
            {
                let mut d = m.clone();
                d.name = crate::mods::Sym::new("Damage");
                self.c.add(list, d);
            }
        }
        Ok(())
    }

    /// `CWCHandler(env)`: Cast while Channelling
    /// triggers the main skill while its source channels
    fn cwc_handler(&mut self) -> Result<()> {
        let main = self.main();
        if self.has_flag(main, "minion") || self.has_flag(main, "disable") {
            return Ok(());
        }
        let support = self
            .mods(main)
            .triggered_by
            .map(|t| self.skill(main).effect_list[t].gem.clone());
        let weapons =
            crate::calc_skills::ActorWeapons::new((0..2).filter_map(|n| self.weapon(PLAYER, n)));
        let mut source = None;
        let mut triggered = Vec::new();
        for i in 0..self.n_player {
            let slot_match = self.slot_match(i);
            if source.is_none() {
                let gem = support
                    .as_ref()
                    .ok_or("attempt to index a nil value (field 'triggeredBy')")?;
                let can = gem.as_ref().is_some_and(|g| {
                    crate::calc_skills::can_support(
                        &crate::calc_skills::SkillEffect::new(g.granted_effect.clone(), None, 0.0),
                        self.skill(i),
                        &self.env.skills.from_item,
                        &self.env.groups,
                        &weapons,
                        self.input.data.skills,
                    )
                });
                if self.sd(i, "triggerTime").is_some_and(ModValue::truthy)
                    && can
                    && i != main
                    && slot_match
                    && !self.is_triggered(i)
                {
                    source = Some(i);
                }
            }
            if self.sd_truthy(i, "triggeredWhileChannelling") && slot_match {
                triggered.push(self.package_skill(i)?);
            }
        }
        let Some(source) = source.filter(|_| !triggered.is_empty()) else {
            self.sd_clear(main, "triggered");
            let mods = self.mods_mut(main);
            mods.info_message2 = Some("DPS reported assuming Self-Cast".into());
            mods.info_message = Some("No Cast While Channeling Triggering Skill Found".into());
            mods.info_trigger = Some(String::new());
            return Ok(());
        };
        let adds_cast_time = self.added_cast_time(main)?.map(|(t, _)| t);
        match adds_cast_time {
            Some(t) => {
                self.set_flag(main, "addsCastTime", true);
                self.c.set_out(PLAYER, "addsCastTime", t);
            }
            None => self.c.clear_out(PLAYER, "addsCastTime"),
        }
        let bd = self.has_bd(PLAYER);
        if bd && let Some(l) = self.added_cast_time_lines(main)? {
            self.bd(PLAYER).unwrap().set_lines("AddedCastTime", l);
        }
        let n_triggered = triggered.len();
        let (list, cfg) = (self.lists[main].list, self.cfg(main));
        let icdr = self.c.calc_mod(list, Some(&cfg), &["CooldownRecovery"])?;
        let interval = self.sd_num(source, "triggerTime").unwrap_or(f64::NAN);
        let rate_of_trigger = 1.0 / interval;
        let triggered_cd = self.sd_num(main, "cooldown");
        let cd_override = self
            .c
            .override_num(list, Some(&cfg), &["CooldownRecovery"])?;
        if cd_override.is_some() {
            self.set_flag(main, "hasOverride", true);
        }
        let total = cd_override.unwrap_or_else(|| {
            m_max(triggered_cd.unwrap_or(0.0), adds_cast_time.unwrap_or(0.0)) / icdr
        });
        let tick = self.misc_data("ServerTickRate");
        let adjusted = (total * tick).ceil() / tick;
        let effective = (adjusted * rate_of_trigger).ceil() / rate_of_trigger;
        let cap = m_min(1.0 / effective, rate_of_trigger);
        self.c.set_out(PLAYER, "TriggerRateCap", cap);
        let (mut rate, sim_time, rates) =
            self.rotation_sim(triggered, rate_of_trigger, 0.0, 100.0, PLAYER)?;
        let pdb = self.pdb();
        let bots = self.c.flag(pdb, None, &["HaveTriggerBots"])? && self.has_type(main, "Spell");
        if bots {
            rate *= 2.0;
        }
        self.c.set_out(PLAYER, "SkillTriggerRate", rate);
        if bd {
            let name = self.skill_name(main).to_string();
            let first = sf!(
                "Cast While Channeling triggers %s every %.2fs while channeling %s ",
                name.as_str(),
                interval,
                self.skill_name(source)
            );
            let mut l = if triggered_cd.is_some() || cd_override.is_some() {
                let cd =
                    triggered_cd.ok_or("bad argument #2 to 'format' (number expected, got nil)")?;
                let mut l = vec![
                    first,
                    String::new(),
                    sf!("%.2f ^8(base cooldown of triggered skill)", cd),
                    sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr),
                    sf!("= %.4f ^8(final cooldown of triggered skill)", cd / icdr),
                    String::new(),
                ];
                if let Some(o) = cd_override {
                    l[3] = sf!("%.2f ^8(hard override of cooldown of %s)", o, name.as_str());
                    l.remove(4);
                    l.remove(4);
                }
                l
            } else {
                vec![
                    first,
                    String::new(),
                    format!("{name} has no base cooldown or cooldown override"),
                    String::new(),
                ]
            };
            let extra = |affected: f64| -> Option<f64> {
                if cd_override.is_some() {
                    return None;
                }
                let next = effective - interval;
                let over = total - next;
                let reduced = total * icdr - total;
                (over < affected).then(|| {
                    let div = affected / (affected - over - reduced);
                    ((div - 1.0) * 100.0).ceil() - (icdr - 1.0) * 100.0
                })
            };
            if let Some(a) = adds_cast_time {
                l.push("Cast While Channeling had no base cooldown".into());
                l.push(sf!(
                    "+ %.2f ^8(%s adds cast time as cooldown to trigger)",
                    a,
                    name.as_str()
                ));
                l.push(sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr));
                l.push(sf!(
                    "= %.4f ^8(final cooldown of triggered skill)",
                    a / icdr
                ));
                l.push(String::new());
                l.push(sf!("%.3f ^8(adjusted for server tick rate)", adjusted));
                l.push(String::new());
                let csi = extra(a);
                if let Some(x) = extra(total * icdr) {
                    l.push(sf!("^8(extra ICDR of %d%% would reach next breakpoint)", x));
                }
                if let Some(x) = csi {
                    l.push(sf!(
                        "^8(extra Cast Rate Increase of %d%% would reach next breakpoint)",
                        x
                    ));
                    l.push(String::new());
                }
            } else if let Some(x) = extra(total * icdr) {
                l.push(sf!("^8(extra ICDR of %d%% would reach next breakpoint)", x));
                l.push(String::new());
            }
            l.push("Trigger rate:".into());
            l.push(sf!(
                "1 / %.3f ^8(trigger rate adjusted for triggering interval)",
                1.0 / cap
            ));
            l.push(sf!("= %.2f ^8 %s casts per second", cap, name.as_str()));
            let b = self.bd(PLAYER).unwrap();
            b.set_lines("TriggerRateCap", l);
            if n_triggered > 1 {
                let mut l = vec![
                    sf!(
                        "%.2f ^8(%s triggers per second)",
                        rate_of_trigger,
                        "Cast While Channeling"
                    ),
                    sf!(
                        "/ %.2f ^8(Estimated impact of linked spells)",
                        rate_of_trigger / rate
                    ),
                    sf!("= %.2f ^8%s casts per second", rate, name.as_str()),
                ];
                if bots {
                    l.insert(
                        2,
                        "x 2 ^8(Trigger bots effectively cause the skill to trigger twice)".into(),
                    );
                }
                b.set_lines("SkillTriggerRate", l);
                b.set("SimData", sim_breakdown(sim_time, &rates));
            }
        }
        self.add_trigger_inc_more(main, main)?;
        self.c.set_out(PLAYER, "ChannelTimeToTrigger", interval);
        self.sd_set(main, "triggered", true);
        self.set_flag(main, "globalTrigger", true);
        self.sd_set(main, "triggerRate", rate);
        let uuid = self.skill_uuid(source);
        self.sd_set(main, "triggerSourceUUID", uuid.as_str());
        let info = format!(
            "Cast While Channeling's Trigger: {}",
            self.skill_name(source)
        );
        self.mods_mut(main).info_message = Some(info);
        Ok(())
    }

    /// `skill.socketGroup.slot`
    pub(crate) fn group_slot(&self, i: usize) -> Option<String> {
        self.skill(i)
            .group
            .and_then(|g| self.env.groups.get(g))
            .and_then(|g| g.slot.clone())
    }

    /// `skill.triggeredBy and
    /// calcLib.canGrantedEffectSupportActiveSkill(skill.triggeredBy.grantedEffect, skill)`
    fn trigger_supports(&self, i: usize) -> bool {
        let Some(t) = self.mods(i).triggered_by else {
            return false;
        };
        let skill = self.skill(i);
        let weapons =
            crate::calc_skills::ActorWeapons::new((0..2).filter_map(|n| self.weapon(PLAYER, n)));
        let effect = crate::calc_skills::SkillEffect::new(
            skill.effect_list[t].granted_effect.clone(),
            None,
            0.0,
        );
        crate::calc_skills::can_support(
            &effect,
            skill,
            &self.env.skills.from_item,
            &self.env.groups,
            &weapons,
            self.input.data.skills,
        )
    }

    /// `helmetFocusHandler(env)`: skills triggered when the player
    /// focuses, assuming focus is used again as soon as it can be
    fn helmet_focus_handler(&mut self) -> Result<()> {
        let main = self.main();
        let Some(t) = self.mods(main).triggered_by else {
            return Ok(());
        };
        if self.has_flag(main, "minion") || self.has_flag(main, "disable") {
            return Ok(());
        }
        self.sd_set(main, "triggered", true);
        let e = &self.skill(main).effect_list[t];
        let trigger_cd = e
            .level
            .and_then(|l| e.granted_effect.level(l))
            .ok_or("attempt to index a nil value (field '?')")?
            .field("cooldown");
        let triggered_cd = self.sd_num(main, "cooldown");
        let (list, cfg) = (self.lists[main].list, self.cfg(main));
        let icdr_focus = self
            .c
            .calc_mod(list, Some(&cfg), &["FocusCooldownRecovery"])?;
        let icdr = self.c.calc_mod(list, Some(&cfg), &["CooldownRecovery"])?;
        let focus = self
            .input
            .data
            .skills
            .skills
            .get("Focus")
            .ok_or("attempt to index a nil value (local 'skillFocus')")?;
        let duration = focus
            .constant_stats
            .first()
            .and_then(|(_, v)| *v)
            .ok_or("attempt to perform arithmetic on a nil value")?
            / 1000.0;
        let focus_base_cd = focus
            .level(1.0)
            .and_then(|l| l.field("cooldown"))
            .ok_or("attempt to perform arithmetic on a nil value")?;
        let total = duration + focus_base_cd / icdr_focus;
        let action_cd = m_max(
            triggered_cd.unwrap_or(0.0),
            trigger_cd.unwrap_or(0.0) / icdr,
        );
        let tick = self.misc_data("ServerTickRate");
        let adjusted = (action_cd * tick).ceil() / tick;
        let rate = if adjusted != 0.0 {
            1.0 / adjusted
        } else {
            f64::INFINITY
        };
        self.c.set_out(PLAYER, "TriggerRateCap", rate);
        self.c.set_out(PLAYER, "SkillTriggerRate", 1.0 / total);
        if self.has_bd(PLAYER) {
            let tcd = trigger_cd.ok_or("bad argument #2 to 'format' (number expected, got nil)")?;
            let rounding = if self.sd_truthy(main, "ignoresTickRate") {
                String::new()
            } else {
                sf!("%.3f ^8(adjusted for server tick rate)", adjusted)
            };
            let trigger = [
                sf!("%.2f ^8(base cooldown of trigger)", tcd),
                sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr),
                sf!("= %.4f ^8(final cooldown of trigger)", tcd / icdr),
                String::new(),
            ];
            let mut l = Vec::new();
            match triggered_cd {
                Some(cd) => {
                    l.extend([
                        sf!("%.2f ^8(base cooldown of triggered skill)", cd),
                        sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr),
                        sf!("= %.4f ^8(final cooldown of triggered skill)", cd / icdr),
                        String::new(),
                    ]);
                    l.extend(trigger);
                    l.extend([
                        sf!(
                            "%.3f ^8(biggest of trigger cooldown and triggered skill cooldown)",
                            action_cd
                        ),
                        String::new(),
                        rounding,
                        String::new(),
                        "Trigger rate:".into(),
                        sf!("1 / %.3f", adjusted),
                        sf!("= %.2f ^8per second", rate),
                    ]);
                }
                None => {
                    l.extend(["Triggered skill has no base cooldown".into(), String::new()]);
                    l.extend(trigger);
                    l.extend([
                        rounding,
                        String::new(),
                        "Trigger rate:".into(),
                        sf!("1 / %.3f", adjusted),
                        sf!("= %.3f ^8per second", rate),
                    ]);
                }
            }
            let b = self.bd(PLAYER).unwrap();
            b.set_lines("TriggerRateCap", l);
            b.set_lines(
                "SkillTriggerRate",
                vec![
                    sf!("%.2f ^8(focus base cooldown)", focus_base_cd),
                    sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr_focus),
                    sf!(
                        "+ %.2f ^8(skills are only triggered on activation thus we add focus duration)",
                        duration
                    ),
                    sf!("= %.2f ^8(effective skill cooldown for trigger purposes)", total),
                    String::new(),
                    sf!(
                        "%.3f casts per second ^8(Assuming player uses focus exactly when its cooldown of %.2fs expires)",
                        1.0 / total,
                        total
                    ),
                ],
            );
        }
        self.add_trigger_inc_more(main, main)?;
        let mods = self.mods_mut(main);
        mods.info_message = Some("Assuming perfect focus Re-Use".into());
        mods.info_trigger = Some("Focus".into());
        self.sd_set(main, "triggerRate", 1.0 / total);
        self.set_flag(main, "globalTrigger", true);
        Ok(())
    }

    /// `configTable["snipe"](env)`: Snipe by itself, as the trigger of the
    /// skills linked to it, or such a skill triggered by its stages
    fn snipe_config(&mut self) -> Result<Option<TriggerConfig>> {
        let main = self.main();
        let pdb = self.pdb();
        let mut stages = m_min(
            self.c.base(pdb, None, &["Multiplier:SnipeStage"])?,
            self.c.base(pdb, None, &["Multiplier:SnipeStagesMax"])?,
        );
        let (list, cfg) = (self.lists[main].list, self.cfg(main));
        let hit_multi = self.c.base(list, Some(&cfg), &["snipeHitMulti"])?;
        let ailment_multi = self.c.base(list, Some(&cfg), &["snipeAilmentMulti"])?;
        let main_slot = self.group_slot(main);
        let in_slot =
            |p: &Self, i: usize| p.skill(i).group.is_some() && p.group_slot(i) == main_slot;
        let triggered: Vec<usize> = (0..self.n_player)
            .filter(|&i| self.sd_truthy(i, "triggeredBySnipe") && in_slot(self, i))
            .collect();
        let (hit, ailment) = (self.mf("Hit"), self.mf("Ailment"));
        let stage_mods = |p: &mut Self, stages: f64, hit_first: bool| {
            let base = src("Multiplier:SnipeStages", ModType::Base, stages, "Snipe");
            let more = |v: f64, f: u32| {
                src("Damage", ModType::More, v, "Snipe")
                    .flags(f, 0)
                    .tag(crate::mods::tag::Tag::multiplier("SnipeStages"))
            };
            p.c.add(list, base);
            let (a, b) = (more(hit_multi, hit), more(ailment_multi, ailment));
            let (a, b) = if hit_first { (a, b) } else { (b, a) };
            p.c.add(list, a);
            p.c.add(list, b);
        };
        if self.skill_name(main) == "Snipe" {
            if self.is_limited(main) {
                stages = 0.0;
            } else {
                self.sd_set(main, "hitTimeMultiplier", m_max(1.0, stages) - 0.5);
            }
            if triggered.is_empty() {
                stage_mods(self, stages, true);
            } else {
                for n in ["Lightning", "Cold", "Fire", "Chaos", "Physical"] {
                    let mut tag = crate::mods::tag::Tag::skill_name("Snipe");
                    if let crate::mods::tag::TagKind::SkillName {
                        include_transfigured,
                        ..
                    } = &mut tag.kind
                    {
                        *include_transfigured = true;
                    }
                    let mut m =
                        crate::mods::Mod::new(&format!("DealNo{n}"), ModType::Flag, true, None);
                    m.tags.push(tag);
                    self.c.add(list, m);
                }
            }
            return Ok(None);
        }
        let index = triggered
            .iter()
            .position(|&i| i == main)
            .map(|k| k as f64 + 1.0);
        if !index.is_some_and(|k| k <= stages) {
            self.sd_clear(main, "triggered");
            let mods = self.mods_mut(main);
            mods.info_message2 = Some("DPS reported assuming Self-Cast".into());
            mods.info_message = Some("Not enough Snipe stages to trigger this skill".into());
            mods.info_trigger = Some(String::new());
            return Ok(None);
        }
        stage_mods(self, stages, false);
        let (mut source, mut rate) = (None, None);
        for i in 0..self.n_player {
            if self.skill_name(i) != "Snipe" || !in_slot(self, i) {
                continue;
            }
            self.sd_set(i, "hitTimeMultiplier", stages - 0.5);
            let c = self
                .cached_calc(i, false, true)?
                .ok_or("attempt to index a nil value (field '?')")?;
            let speed = c.out("HitSpeed");
            let used_by_mirage = self
                .cfg(i)
                .skill_cond
                .as_ref()
                .is_some_and(|c| c.contains(&crate::mods::Sym::new("usedByMirage")));
            if !self.has_flag(i, "disable")
                && !used_by_mirage
                && !self.has_type(i, "OtherThingUsesSkill")
                && match source {
                    None => speed.is_some(),
                    Some(_) => speed.is_some_and(|s| s > rate.unwrap_or(0.0)),
                }
            {
                rate = speed;
                match c.out("HitTime") {
                    Some(t) => self.c.set_out(PLAYER, "ChannelTimeToTrigger", t),
                    None => self.c.clear_out(PLAYER, "ChannelTimeToTrigger"),
                }
                source = Some(i);
            }
        }
        Ok(Some(TriggerConfig {
            trig_rate: rate,
            source,
            ..TriggerConfig::default()
        }))
    }

    /// `activeEffect.srcInstance.skillStageCount`
    fn stage_count(&self, i: usize) -> Option<f64> {
        self.skill(i)
            .active_effect
            .src
            .and_then(|(g, k)| self.env.groups.get(g)?.gem_list.get(k))
            .and_then(|g| g.skill_stage_count)
    }
}
