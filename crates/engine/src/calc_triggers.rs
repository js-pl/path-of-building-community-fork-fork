//! `calcs.triggers` (`Modules/CalcTriggers.lua`): the rate at which the
//! main skill is triggered, from its trigger's configuration
//! (`configTable`) and `defaultTriggerHandler`, with the simulation of
//! skill rotations (`calcMultiSpellRotationImpact`).
//!
//! Not ported yet: breakdowns, Arcanist Brand's
//! activation, Manaforged Arrows, Cast while Channelling's handler, and every
//! configuration but those in [`Perform::trigger_config`].
//!
//! In sync with upstream `16de4b82d57f1c0de6eb40f37143c32d4da36a02`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::calc::{PLAYER, m_max, m_min, round};
use crate::calc_cache::CachedCalc;
use crate::calc_perform::{Perform, Result, src};
use crate::mods::{ModType, ModValue};

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
    pub ignore_source_rate: bool,
    pub source_weapon: bool,
    pub stages_are_overlaps: Option<f64>,
    pub overlaps: Option<f64>,
    /// `customHandler`: a handler of its own in place of the default one
    pub custom_handler: Option<fn(&mut Perform) -> Result<()>>,
    /// Arcanist Brand as the trigger: the brand skill, its activation
    /// frequency's increase and more multipliers
    /// (`triggeredBy.mainSkill`, `activationFreqInc`, `activationFreqMore`)
    pub brand: Option<(usize, f64, f64)>,
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
                    return Ok(None);
                }
                TriggerConfig {
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
            "uul-netol's embrace" => TriggerConfig {
                trigger_name: Some("Bone Nova".into()),
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Damage") || p.has_type(s, "Attack")),
                ..TriggerConfig::default()
            },
            "duskblight" => TriggerConfig {
                trigger_name: Some("Stalking Pustule".into()),
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Damage") || p.has_type(s, "Attack")),
                ..TriggerConfig::default()
            },
            "jorrhast's blacksteel" => {
                self.sd_set(main, "sourceRateIsFinal", true);
                TriggerConfig {
                    trigger_skill_cond: Some(|p, s| {
                        p.has_type(s, "Damage") || p.has_type(s, "Attack")
                    }),
                    ..TriggerConfig::default()
                }
            }
            "cospri's malice" => TriggerConfig {
                trigger_skill_cond: Some(|p, s| {
                    let sword = p.mf("Sword") | p.mf("Weapon1H");
                    p.has_type(s, "Melee") && p.cfg(s).flags.unwrap_or(0) & sword > 0
                }),
                triggered_skill_cond: Some(|p, s| {
                    let slot = |k: usize| {
                        p.skill(k)
                            .group
                            .and_then(|g| p.env.groups.get(g))
                            .map(|g| g.slot.clone())
                    };
                    p.sd_truthy(s, "triggeredByCospris") && slot(p.main()) == slot(s)
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
                if self.skill_name(main) == name {
                    set_global(self);
                    self.set_flag(main, "skipEffectiveRate", true);
                } else if (0..self.n_player).any(|i| self.skill_name(i) == name) {
                    // The trigger's cooldown comes from the active part
                    return Err(format!("not ported: the cooldown of {name}'s active part"));
                }
                if key != "automation" {
                    self.sd_set(main, "sourceRateIsFinal", true);
                    self.sd_set(main, "ignoresTickRate", true);
                }
                TriggerConfig {
                    trigger_on_use: true,
                    source: Some(main),
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
                    trigger_skill_cond: Some(|p, s| p.has_type(s, "Hex") && p.slot_match(s)),
                    ..TriggerConfig::default()
                }
            }
            "FieryImpactHeistMaceImplicit" => TriggerConfig {
                source_weapon: true,
                trigger_skill_cond: Some(|p, s| p.has_type(s, "Melee") || p.has_type(s, "Attack")),
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
    ) -> Result<(Option<usize>, Option<f64>)> {
        if let Some(c) = self.cached_calc(skill, false, true)? {
            let speed = c.out("HitSpeed").or_else(|| c.out("Speed"));
            let better = match comparer {
                Some(f) => f(&c, source, rate),
                None => match source {
                    None => speed.is_some(),
                    Some(_) => speed.is_some_and(|s| s > rate.unwrap_or(0.0)),
                },
            };
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

    /// `calcMultiSpellRotationImpact(env, skillRotation, sourceRate,
    /// triggerCD, chance, actor)`: the main skill's rate
    fn rotation_impact(
        &self,
        mut rotation: Vec<SimSkill>,
        source_rate: f64,
        trigger_cd: f64,
        chance: f64,
        actor: usize,
    ) -> Result<f64> {
        let increment = 1.0 / source_rate;
        let sim_time = increment * 1000.0;
        let tick = self.misc_data("ServerTickTime");
        let n = rotation.len();
        let mut cds = Vec::with_capacity(n);
        for s in &rotation {
            let own = s
                .cd_override
                .unwrap_or((s.cd.unwrap_or(0.0) + s.added_cooldown) / s.icdr);
            cds.push(m_max(
                own,
                (trigger_cd + s.adds_cast_time.unwrap_or(0.0)) / s.icdr,
            ));
        }
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
        let main_uuid = self
            .actor_main(actor)
            .map(|m| self.skill_uuid(m))
            .unwrap_or_default();
        let mut main_rate = 0.0;
        for (k, s) in rotation.drain(..).enumerate() {
            let rate = 1.0 / (sim_time / count[k] + (increment / chance * 100.0) - increment);
            if s.uuid == main_uuid {
                main_rate = rate;
            }
        }
        Ok(main_rate)
    }

    /// `defaultTriggerHandler(env, config)` without its breakdowns
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
                    (source, trig_rate) =
                        self.find_trigger_skill(i, source, trig_rate, config.comparer)?;
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
            let mods = self.mods_mut(main);
            mods.info_message2 = Some("DPS reported assuming Self-Cast".into());
            mods.info_trigger = Some(String::new());
            return Ok(());
        }
        self.sd_set(main, "triggered", true);

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
            }
            let part = self.mods(s).skill_part_name.clone().unwrap_or_default();
            if crate::pat::find(&part, "(.*)All(.*)Projectiles(.*)").is_some()
                && self.has_flag(s, "projectile")
            {
                let mult = c.out("ProjectileCount").unwrap_or(1.0);
                trig_rate = trig_rate.map(|r| r * mult);
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
        }
        let melee_source = source.is_some_and(|s| self.has_type(s, "Melee"));
        if (self.sd_truthy(main, "triggeredByBattleMageCry") || self.skill_name(main) == "Combust")
            && cached.is_some()
            && melee_source
        {
            return Err("not ported: the uptime of exerting warcries as a trigger".into());
        }
        if self.sd_truthy(main, "triggeredByManaforged") && trig_rate.is_some_and(|r| r > 0.0) {
            return Err("not ported: Manaforged Arrows".into());
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
        let effect_cd = |p: &Self, i: usize| -> Result<Option<f64>> {
            let Some(t) = p.mods(i).triggered_by else {
                return Ok(None);
            };
            let e = &p.skill(i).effect_list[t];
            let level = e
                .level
                .and_then(|l| e.granted_effect.level(l))
                .ok_or("attempt to index a nil value (field '?')")?;
            Ok(level.field("cooldown"))
        };
        let mut trigger_cd = if self.mods(main).triggered_by.is_some() {
            effect_cd(self, player_main)?
        } else {
            None
        };
        if trigger_cd.is_none() {
            let s = source.ok_or("attempt to index a nil value (local 'source')")?;
            trigger_cd = effect_cd(self, s)?;
        }
        let triggered_cd = self.sd_num(main, "cooldown");
        // Brand activation is not affected by cooldown recovery
        if by_brand && let Some((brand, inc, more)) = config.brand {
            let freq = self.sd_num(brand, "repeatFrequency").unwrap_or(f64::NAN);
            trigger_cd = Some(freq / more / inc * icdr);
        }
        let adds_cast_time = self.added_cast_time(player_main)?.map(|(t, _)| t);
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
            let rate = trig_rate.ok_or("attempt to compare nil with number")?;
            if expiration > rate {
                self.c.add(
                    pdb,
                    crate::calc_perform::src("UsesCurseOverlaps", ModType::Flag, true, "Config"),
                );
            } else {
                trig_rate = Some(expiration);
            }
        } else if doom_trigger && doom_source == Some("hexblast") {
            let (mut hexblast, mut rate) = (None, None);
            for i in 0..self.n_player {
                if self.skill_name(i) == "Hexblast" && !self.is_triggered(i) && i != main {
                    (hexblast, rate) = self.find_trigger_skill(i, hexblast, rate, None)?;
                }
            }
            if hexblast.is_some() {
                let (t, r) = (
                    trig_rate.ok_or("attempt to perform arithmetic on a nil value")?,
                    rate.ok_or("attempt to perform arithmetic on a nil value")?,
                );
                trig_rate = Some(1.0 / (1.0 / t + 1.0 / r));
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
                if hit != 100.0 {
                    chance = if dual {
                        chance * both("HitChance") / 100.0
                    } else {
                        chance * hit / 100.0
                    };
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
                        chance = if dual {
                            chance * both("CritChance") / 100.0
                        } else {
                            chance * crit / 100.0
                        };
                    }
                }
            }
            if let Some(tc) = config.trigger_chance
                && tc != 100.0
            {
                chance = chance * tc / 100.0;
            }
            if self.sd_truthy(main, "ignoresTickRate") && config.triggered_skill_cond.is_none() {
                let overlaps = if config.stages_are_overlaps.is_some()
                    && self.mods(player_main).skill_part == config.stages_are_overlaps
                {
                    self.stage_count(player_main)
                } else {
                    config.overlaps
                };
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
                let mut r = self.rotation_impact(rotation, esr, cd, chance, actor)?;
                let db = self.c.db(actor);
                if self.c.flag(db, None, &["HaveTriggerBots"])? && self.has_type(main, "Spell") {
                    r *= 2.0;
                }
                let hits = if config.stages_are_overlaps.is_some()
                    && self.mods(player_main).skill_part == config.stages_are_overlaps
                {
                    self.stage_count(player_main).unwrap_or(f64::NAN)
                } else {
                    1.0
                };
                hits * r
            }
        } else {
            0.0
        };
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
        self.mods_mut(main).info_trigger = config.trigger_name;
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

    /// `CWCHandler(env)` without its breakdowns: Cast while Channelling
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
        self.c.set_out(
            PLAYER,
            "TriggerRateCap",
            m_min(1.0 / effective, rate_of_trigger),
        );
        let mut rate = self.rotation_impact(triggered, rate_of_trigger, 0.0, 100.0, PLAYER)?;
        let pdb = self.pdb();
        if self.c.flag(pdb, None, &["HaveTriggerBots"])? && self.has_type(main, "Spell") {
            rate *= 2.0;
        }
        self.c.set_out(PLAYER, "SkillTriggerRate", rate);
        self.add_trigger_inc_more(main, main)?;
        self.c.set_out(PLAYER, "ChannelTimeToTrigger", interval);
        self.sd_set(main, "triggered", true);
        self.set_flag(main, "globalTrigger", true);
        self.sd_set(main, "triggerRate", rate);
        let uuid = self.skill_uuid(source);
        self.sd_set(main, "triggerSourceUUID", uuid.as_str());
        Ok(())
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
