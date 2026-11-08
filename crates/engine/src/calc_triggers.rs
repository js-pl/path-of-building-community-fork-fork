//! `calcs.triggers` (`Modules/CalcTriggers.lua`): the rate at which the
//! main skill is triggered, from its trigger's configuration
//! (`configTable`) and `defaultTriggerHandler`, with the simulation of
//! skill rotations (`calcMultiSpellRotationImpact`).
//!
//! Not ported yet: breakdowns, Doom Blast's sources, Arcanist Brand's
//! activation, Manaforged Arrows, and every configuration but those in
//! [`Perform::trigger_config`].
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::calc::{PLAYER, m_max, m_min, round};
use crate::calc_perform::{Perform, Result, nospace};
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
    pub trig_rate: Option<f64>,
    pub allow_triggered: bool,
    pub trigger_on_use: bool,
    pub ignore_source_rate: bool,
    pub source_weapon: bool,
    pub stages_are_overlaps: Option<f64>,
    pub overlaps: Option<f64>,
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
        let skill = self.skill(i);
        let name = nospace(&crate::pat::gsub(
            &skill.active_effect.granted_effect.name,
            "%s+",
            "",
        ));
        let group = skill
            .group
            .and_then(|g| self.env.groups.get(g).map(|x| (g, x)));
        let slot = group
            .and_then(|(_, g)| g.slot.as_deref())
            .map(|s| crate::pat::gsub(&s.to_uppercase(), "%s+", ""))
            .unwrap_or_else(|| "NO_SLOT".into());
        let mut slot_index = 1;
        if let (Some((g, _)), Some((sg, k))) = (group, skill.active_effect.src)
            && sg == g
        {
            slot_index = k + 1;
        }
        let group_index = group.map_or(1, |(g, _)| g + 1);
        format!("{name}_{slot}_{slot_index}_{group_index}")
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
        if self.has_flag(main, "disable") {
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
                self.default_trigger_handler(config)
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
            k => return Err(format!("not ported: the trigger {k}")),
        }))
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
        let source = config.source;
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
                    return Err("not ported: calculations of other skills".into());
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
        let stored = self.sd_num(main, "storedUses");
        if !self.sd_truthy(main, "ignoresTickRate") && stored.is_some_and(|s| s > 1.0) {
            self.sd_set(main, "ignoresTickRate", true);
        }
        if self.sd_truthy(main, "triggeredByManaSpent") {
            let s = source.ok_or("attempt to index a nil value (local 'source')")?;
            let repeats = 1.0 + self.c.base(self.lists[s].list, None, &["RepeatCount"])?;
            trig_rate = trig_rate.map(|r| r / repeats);
        }
        if self.sd_truthy(main, "triggeredByManaforged") && trig_rate.is_some_and(|r| r > 0.0) {
            return Err("not ported: Manaforged Arrows".into());
        }
        if self.sd_truthy(main, "triggeredByBrand") {
            return Err("not ported: Arcanist Brand".into());
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
        let trigger_tick = (trigger_adj * tick_rate).ceil() / tick_rate;
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
        if config.trigger_name.as_deref() == Some("Doom Blast")
            || self.skill_name(player_main) == "Doom Blast"
        {
            return Err("not ported: Doom Blast's sources".into());
        }
        let esr = if let Some(r) = trig_rate
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
                && !config.trigger_on_use
            {
                return Err("not ported: calculations of other skills".into());
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
                let cd = trigger_cd.or(triggered_cd).unwrap_or(0.0);
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
        // `addTriggerIncMoreMods(actor.mainSkill, source or actor.mainSkill)`
        let from = source.unwrap_or(main);
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
        if let Some(s) = source
            && s != main
        {
            let uuid = self.skill_uuid(s);
            self.sd_set(main, "triggerSourceUUID", uuid.as_str());
        }
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
