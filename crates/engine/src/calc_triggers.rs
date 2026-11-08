//! `calcs.triggers` (`Modules/CalcTriggers.lua`): the rate at which the
//! main skill is triggered, from the skill that triggers it, its
//! cooldowns and the other skills it shares the trigger with. The source
//! skill's rate comes from its own cached calculation
//! (`calcs.buildActiveSkill`, [`crate::calcs`]).
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::breakdown::Bd;
use crate::calc::{PLAYER, m_max, m_min};
use crate::calc_perform::{Perform, Result, src};
use crate::calcs::{Cached, Mode, skill_uuid};
use crate::mods::{Mod, ModType, ModValue, Sym};
use crate::sf;
use std::rc::Rc;

/// `packageSkillDataForSimulation(skill, env)`
#[derive(Clone, Debug)]
struct Sim {
    uuid: String,
    cd: Option<f64>,
    cd_override: Option<f64>,
    adds_cast_time: Option<f64>,
    icdr: f64,
    added_cooldown: f64,
}

/// How the source skills compare (`config.comparer`)
#[derive(Clone, Copy)]
enum Comparer {
    Default,
    /// Kitava's Thirst: the cost the source needs
    Cost(&'static str, f64),
    /// Battlemage's Cry and Combust: the source needs an uptime
    Uptime(&'static str),
    /// Avenging Flame: the totem with the most life
    TotemLife,
}

type Cond = Box<dyn Fn(&Perform, usize) -> Result<bool>>;

#[derive(Clone, Copy, PartialEq)]
enum Custom {
    Cwc,
    Focus,
}

/// A trigger's configuration (`configTable[name](env)`)
#[derive(Default)]
struct Config {
    actor: Option<usize>,
    source: Option<usize>,
    trig_rate: Option<f64>,
    uuid: Option<String>,
    triggered_skills: Option<Vec<Sim>>,
    trigger_name: Option<String>,
    trigger_skill_cond: Option<Cond>,
    triggered_skill_cond: Option<Cond>,
    comparer: Option<Comparer>,
    trigger_on_use: bool,
    // Read by the breakdowns only
    use_cast_rate: bool,
    assuming_every_hit_kills: bool,
    source_name: Option<String>,
    allow_triggered: bool,
    trigger_chance: Option<f64>,
    source_weapon: bool,
    ignore_source_rate: bool,
    stages_are_overlaps: Option<f64>,
    overlaps: Option<f64>,
    custom_trigger_name: Option<String>,
    custom_handler: Option<Custom>,
}

fn cond(f: impl Fn(&Perform, usize) -> Result<bool> + 'static) -> Option<Cond> {
    Some(Box::new(f))
}

/// `uniqueTriggerName` of a socket group's source: `.*:.*:(.*),.*`
fn unique_name_of_source(source: &str) -> Option<String> {
    crate::pat::capture(source, ".*:.*:(.*),.*")
}

impl<'a> Perform<'a> {
    fn ty(&self, i: usize, t: &str) -> bool {
        self.has_type(i, t)
    }

    fn cfg_flag(&self, i: usize, f: &str) -> bool {
        self.cfg(i).flags.unwrap_or(0) & self.mf(f) != 0
    }

    fn group_slot(&self, i: usize) -> Option<String> {
        self.skill(i)
            .group
            .and_then(|g| self.env.groups.get(g))
            .and_then(|g| g.slot.clone())
    }

    fn src_gem(&self, i: usize) -> Option<&crate::skills::GemInstance> {
        let (g, k) = self.skill(i).active_effect.src?;
        self.env.groups.get(g)?.gem_list.get(k)
    }

    /// `isTriggered(skill)`
    pub(crate) fn is_triggered(&self, i: usize) -> bool {
        self.sd_truthy(i, "triggeredByUnique")
            || self.sd_truthy(i, "triggered")
            || self.ty(i, "InbuiltTrigger")
            || self.ty(i, "Triggered")
            || self.skill(i).active_effect.granted_effect.triggered
            || self.src_gem(i).is_some_and(|g| g.triggered)
    }

    /// `slotMatch(env, skill)`
    fn slot_match(&self, i: usize) -> bool {
        let pm = self.main();
        let from_item = |k: usize| {
            self.skill(k).active_effect.granted_effect.from_item
                || self
                    .env
                    .skills
                    .from_item
                    .contains(&self.skill(k).active_effect.granted_effect.id)
        };
        let main_from_item = from_item(pm);
        let fi = main_from_item
            || from_item(i)
            || self.src_gem(pm).is_some_and(|g| g.from_item)
            || self.src_gem(i).is_some_and(|g| g.from_item);
        let match1 =
            fi && self.skill(i).group.is_some() && self.group_slot(i) == self.group_slot(pm);
        let match2 = !main_from_item && self.skill(i).group == self.skill(pm).group;
        match1 || match2
    }

    /// `processAddedCastTime(skill)`: the cast time added to the cooldown
    fn added_cast_time(&mut self, i: usize, bd: Option<usize>) -> Result<Option<f64>> {
        let list = self.lists[i].list;
        let cfg = self.cfg(i);
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
        let csi = crate::calc::round((1.0 + inc / 100.0) * more, 2.0);
        self.set_flag(i, "addsCastTime", true);
        let adds = base / csi;
        if let Some(a) = bd {
            let name = self.skill_name(i).to_string();
            if let Some(b) = self.bd(a) {
                b.set(
                    "AddedCastTime",
                    Bd::of([
                        sf!("%.2f ^8(base cast time of %s)", base, name.as_str()),
                        sf!("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
                        sf!("%.2f ^8(more/less)", more),
                        sf!("= %.2f ^8cast time", adds),
                    ]),
                );
            }
        }
        Ok(Some(adds))
    }

    fn sim(&mut self, i: usize) -> Result<Sim> {
        let list = self.lists[i].list;
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let adds = self.added_cast_time(i, None)?;
        Ok(Sim {
            uuid: skill_uuid(self, i),
            cd: self.sd_num(i, "cooldown"),
            cd_override: self.c.override_num(list, c, &["CooldownRecovery"])?,
            adds_cast_time: adds,
            icdr: self.c.calc_mod(list, c, &["CooldownRecovery"])?,
            added_cooldown: self.c.base(list, c, &["CooldownRecovery"])?,
        })
    }

    /// A skill's cached calculation, made if there is none or in the
    /// calculator mode (`calcs.buildActiveSkill`)
    fn cached_calc(&self, i: usize, limit_self: bool) -> Result<Option<Rc<Cached>>> {
        let calcs = self
            .input
            .calcs
            .ok_or("not ported: triggers without the calculations' cache")?;
        let build = self
            .input
            .build
            .ok_or("not ported: triggers without the build")?;
        let mode = self.input.mode;
        let uuid = skill_uuid(self, i);
        if calcs.cached(mode, &uuid).is_none() || mode == Mode::Calculator {
            let mut limited = self.input.limited.clone();
            if limit_self {
                limited.push(uuid.clone());
            }
            calcs.build_active_skill(build, mode, &uuid, &limited)?;
        }
        Ok(calcs.cached(mode, &uuid))
    }

    fn cached_now(&self, uuid: &str) -> Option<Rc<Cached>> {
        self.input.calcs?.cached(self.input.mode, uuid)
    }

    fn compare(
        &self,
        c: Comparer,
        cached: &Cached,
        source: Option<usize>,
        rate: Option<f64>,
    ) -> bool {
        let speed = cached.num("HitSpeed").or(cached.num("Speed"));
        let default = match speed {
            Some(s) => source.is_none() || s > rate.unwrap_or(0.0),
            None => false,
        };
        match c {
            Comparer::Default => default,
            Comparer::Cost(k, required) => default && cached.num(k).unwrap_or(0.0) >= required,
            Comparer::Uptime(k) => default && cached.output.contains_key(&Sym::new(k)),
            Comparer::TotemLife => match cached.num("TotemLife") {
                Some(l) => source.is_none() || l > rate.unwrap_or(0.0),
                None => false,
            },
        }
    }

    /// `findTriggerSkill(env, skill, source, triggerRate, comparer)`
    fn find_trigger_skill(
        &self,
        i: usize,
        source: Option<usize>,
        rate: Option<f64>,
        comparer: Comparer,
    ) -> Result<(Option<usize>, Option<f64>, Option<String>)> {
        let cached = self.cached_calc(i, false)?;
        if let Some(c) = &cached
            && self.compare(comparer, c, source, rate)
            && !self.has_flag(i, "disable")
            && !self
                .cfg(i)
                .skill_cond
                .as_ref()
                .is_some_and(|c| c.contains(&Sym::new("usedByMirage")))
            && !self.ty(i, "OtherThingUsesSkill")
        {
            let speed = match comparer {
                Comparer::TotemLife => c.num("HitSpeed").or(c.num("Speed")),
                _ => c.num("HitSpeed").or(c.num("Speed")),
            };
            let v = match comparer {
                // The comparer's value is passed on as the rate
                Comparer::TotemLife => speed,
                _ => speed,
            };
            return Ok((Some(i), v, Some(skill_uuid(self, i))));
        }
        Ok((source, rate, source.map(|s| skill_uuid(self, s))))
    }

    /// `calcMultiSpellRotationImpact(env, skillRotation, sourceRate,
    /// triggerCD, chance, actor)`
    fn rotation_rate(
        &self,
        rotation: &mut [Sim],
        source_rate: f64,
        trigger_cd: f64,
        chance: f64,
        actor: usize,
    ) -> (f64, SimRates) {
        let tick = self.misc_data("ServerTickTime");
        let increment = 1.0 / source_rate;
        let sim_time = increment * 1000.0;
        let n = rotation.len();
        let mut cds = Vec::with_capacity(n);
        for s in rotation.iter() {
            let cd = m_max(
                s.cd_override
                    .unwrap_or((s.cd.unwrap_or(0.0) + s.added_cooldown) / s.icdr),
                (trigger_cd + s.adds_cast_time.unwrap_or(0.0)) / s.icdr,
            );
            cds.push(cd);
        }
        let mut next_trig = vec![0.0; n];
        let mut count = vec![0.0; n];
        let mut index = 0;
        let mut next = 0.0;
        while next < sim_time {
            let mut cur = index;
            loop {
                if next_trig[cur] <= next {
                    count[cur] += 1.0;
                    next_trig[cur] =
                        tick * ((tick * (next / tick).floor() + cds[cur]) / tick).ceil();
                    break;
                }
                cur = (cur + 1) % n;
                if cur == index {
                    break;
                }
            }
            index = (index + 1) % n;
            next += increment;
        }
        let main_uuid = self.actor_main_index(actor).map(|m| skill_uuid(self, m));
        let mut main_rate = 0.0;
        let mut rates = Vec::with_capacity(n);
        for (k, s) in rotation.iter().enumerate() {
            let rate = 1.0 / (sim_time / count[k] + (increment / chance * 100.0) - increment);
            rates.push((s.uuid.clone(), rate));
            if main_uuid.as_deref() == Some(s.uuid.as_str()) {
                main_rate = rate;
            }
        }
        (main_rate, SimRates { sim_time, rates })
    }

    /// `addTriggerIncMoreMods(activeSkill, sourceSkill)`
    fn add_trigger_inc_more(&mut self, i: usize, source: usize) -> Result<()> {
        let list = self.lists[i].list;
        let scfg = self.cfg(source);
        for typ in [ModType::Inc, ModType::More] {
            for (_, m) in self
                .c
                .tabulate(list, Some(typ), Some(&scfg), &["TriggeredDamage"])?
            {
                let mut d = m.clone();
                d.name = Sym::new("Damage");
                d.source_slot = None;
                d.unreached_tags.clear();
                self.c.add(list, d);
            }
        }
        Ok(())
    }

    /// The granted effect and level of `triggeredBy`
    fn triggered_by(
        &self,
        i: usize,
    ) -> Option<(Rc<crate::skill_data::GrantedEffect>, Option<f64>)> {
        let t = self.mods(i).triggered_by?;
        let e = &self.skill(i).effect_list[t];
        let ge = self
            .mods(i)
            .triggered_by_effect
            .clone()
            .unwrap_or_else(|| e.granted_effect.clone());
        Some((ge, e.level))
    }

    fn triggered_by_cooldown(&self, i: usize) -> Option<f64> {
        let (ge, level) = self.triggered_by(i)?;
        ge.level(level?)?.field("cooldown")
    }

    /// `getUniqueItemTriggerName(skill)`
    fn unique_trigger_name(&self, i: usize) -> Option<String> {
        if let Some(ModValue::Str(s)) = self.sd(i, "triggerSource") {
            return Some(s.to_string());
        }
        for e in &self.skill(i).support_list {
            let ge = &e.granted_effect;
            if (ge.from_item || self.env.skills.from_item.contains(&ge.id)) && !ge.support {
                return Some(ge.name.clone());
            }
        }
        let source = self
            .skill(i)
            .group
            .and_then(|g| self.env.groups.get(g))
            .and_then(|g| g.source.clone())?;
        unique_name_of_source(&source)
    }

    /// `calcs.triggers(env, actor)`
    pub(crate) fn triggers(&mut self, actor: usize) -> Result<()> {
        let Some(m) = self.actor_main_index(actor) else {
            return Ok(());
        };
        if self.has_flag(m, "disable") {
            return Ok(());
        }
        let uuid = skill_uuid(self, m);
        if self.input.limited.contains(&uuid) {
            return Ok(());
        }
        let ge = self.skill(m).active_effect.granted_effect.clone();
        let skill_name = ge.name.clone();
        let trigger_name = self.triggered_by(m).map(|(g, _)| g.name.clone());
        let unique = if self.is_triggered(m) {
            self.unique_trigger_name(m)
        } else {
            None
        };
        let lower = |s: &str| crate::pat::lower(s);
        let mut keys: Vec<String> = vec![ge.id.clone(), lower(&skill_name)];
        if let Some(t) = &trigger_name {
            keys.push(lower(t));
            keys.push(crate::pat::gsub(&lower(t), "^awakened ", ""));
        }
        if let Some(u) = &unique {
            keys.push(lower(u));
        }
        let mut config = None;
        for k in &keys {
            if let Some(c) = self.trigger_config(k, actor)? {
                config = Some(c);
                break;
            }
        }
        let Some(mut config) = config else {
            self.sd_clear(m, "triggered");
            return Ok(());
        };
        let cactor = config.actor.unwrap_or(actor);
        if config.trigger_name.is_none() {
            config.trigger_name = trigger_name.or(Some(skill_name)).or(unique);
        }
        if config.trigger_chance.is_none() {
            config.trigger_chance = self.src_gem(m).and_then(|g| g.trigger_chance);
        }
        match config.custom_handler {
            Some(Custom::Cwc) => self.cwc_handler(),
            Some(Custom::Focus) => self.focus_handler(),
            None => self.default_trigger_handler(config, cactor),
        }
    }

    /// `configTable[key](env)`; `None` when the key has no configuration
    /// or its function returns nil
    fn trigger_config(&mut self, key: &str, _actor: usize) -> Result<Option<Config>> {
        let pm = self.main();
        let pdb = self.pdb();
        let main_name = self.skill_name(pm).to_string();
        let attack_or = |other: &'static str| {
            cond(move |p: &Perform, k| Ok(p.ty(k, other) || p.ty(k, "Attack")))
        };
        let c = match key {
            "law of the wilds" => Config {
                trigger_skill_cond: cond(|p, k| {
                    Ok(!p.ty(k, "SummonsTotem")
                        && (p.ty(k, "Melee") || p.ty(k, "Attack"))
                        && p.cfg_flag(k, "Claw"))
                }),
                ..Config::default()
            },
            "the rippling thoughts" | "the surging thoughts" => {
                if main_name != "Storm Cascade" {
                    return Ok(None);
                }
                Config {
                    trigger_on_use: key == "the surging thoughts",
                    trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Melee") || p.ty(k, "Attack"))),
                    ..Config::default()
                }
            }
            "the hidden blade" => {
                self.set_flag(pm, "globalTrigger", true);
                self.sd_set(pm, "triggerRateCapOverride", 2.0);
                if self.c.flag(pdb, None, &["Condition:Phasing"])? {
                    if let Some(bd) = self.bd(PLAYER) {
                        bd.set(
                            "TriggerRateCap",
                            Bd::of([
                                sf!("%.2f ^8(Unseen Strike from The Hidden Blade has no cooldown but is still triggered)", 2.0),
                                sf!("= %.2f", 2.0),
                            ]),
                        );
                    }
                    Config {
                        source: Some(pm),
                        ..Config::default()
                    }
                } else {
                    self.mods_mut(pm).skill_flags.insert("disable".into());
                    self.mods_mut(pm).disable_reason =
                        Some("This skill is requires you to be phasing".into());
                    return Ok(None);
                }
            }
            "replica eternity shroud" | "shroud of the lightless" | "tempest shield" => {
                self.set_flag(pm, "globalTrigger", true);
                Config {
                    source: Some(pm),
                    ..Config::default()
                }
            }
            "limbsplit" | "the cauteriser" => Config {
                trigger_name: Some("Gore Shockwave".into()),
                trigger_skill_cond: attack_or("Melee"),
                ..Config::default()
            },
            "duskblight" => Config {
                trigger_name: Some("Stalking Pustule".into()),
                trigger_skill_cond: attack_or("Damage"),
                ..Config::default()
            },
            "lioneye's paws" | "replica lioneye's paws" => {
                self.sd_set(pm, "cooldown", 1.0);
                Config {
                    trigger_on_use: true,
                    trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack") && p.cfg_flag(k, "Bow"))),
                    ..Config::default()
                }
            }
            "moonbender's wing" => {
                self.sd_set(pm, "cooldown", 1.0);
                Config {
                    trigger_name: Some("Lightning Warp".into()),
                    trigger_skill_cond: attack_or("Melee"),
                    ..Config::default()
                }
            }
            "ngamahu's flame" => Config {
                trigger_name: Some("Molten Burst".into()),
                trigger_skill_cond: attack_or("Melee"),
                ..Config::default()
            },
            "cameria's avarice" => Config {
                trigger_name: Some("Icicle Burst".into()),
                trigger_skill_cond: attack_or("Damage"),
                ..Config::default()
            },
            "starcaller" => Config {
                trigger_name: Some("Starfall".into()),
                trigger_skill_cond: attack_or("Melee"),
                ..Config::default()
            },
            "uul-netol's embrace" => Config {
                trigger_name: Some("Bone Nova".into()),
                trigger_skill_cond: attack_or("Damage"),
                ..Config::default()
            },
            "rigwald's crest" | "jorrhast's blacksteel" | "ashcaller" => {
                self.sd_set(pm, "sourceRateIsFinal", true);
                Config {
                    assuming_every_hit_kills: true,
                    trigger_skill_cond: attack_or("Damage"),
                    ..Config::default()
                }
            }
            "arakaali's fang" | "sporeguard" | "mark of the elder" | "mark of the shaper" => {
                Config {
                    assuming_every_hit_kills: true,
                    trigger_skill_cond: attack_or("Damage"),
                    ..Config::default()
                }
            }
            "poet's pen" | "asenath's chant" => {
                let flag = if key == "poet's pen" { "Wand" } else { "Bow" };
                Config {
                    trigger_on_use: true,
                    trigger_skill_cond: cond(move |p, k| {
                        Ok((p.ty(k, "Damage") || p.ty(k, "Attack")) && p.cfg_flag(k, flag))
                    }),
                    triggered_skill_cond: cond(|p, k| {
                        Ok(p.sd_truthy(k, "triggeredByUnique")
                            && p.group_slot(p.main()) == p.group_slot(k)
                            && p.ty(k, "Spell"))
                    }),
                    ..Config::default()
                }
            }
            "maloney's mechanism" => {
                let slot = self
                    .cfg(pm)
                    .slot_name
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default();
                let source = self
                    .item(PLAYER, &slot)
                    .map(|i| i.mod_source.clone())
                    .ok_or("attempt to index a nil value (itemList[slotName])")?;
                let name = unique_name_of_source(&source)
                    .ok_or("attempt to index a nil value (uniqueTriggerName)")?;
                let replica = crate::pat::find(&name, "Replica.").is_some();
                Config {
                    trigger_on_use: true,
                    trigger_name: Some(name),
                    use_cast_rate: replica,
                    trigger_skill_cond: cond(move |p, k| {
                        let attack = p.ty(k, "Attack") && p.cfg_flag(k, "Bow") && !replica;
                        let spell = p.ty(k, "Spell") && replica;
                        Ok(attack || spell)
                    }),
                    triggered_skill_cond: cond(|p, k| {
                        Ok(p.sd_truthy(k, "triggeredByUnique")
                            && p.group_slot(p.main()) == p.group_slot(k)
                            && p.ty(k, "RangedAttack"))
                    }),
                    ..Config::default()
                }
            }
            "vixen's entrapment" => Config {
                use_cast_rate: true,
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Hex"))),
                ..Config::default()
            },
            "flames of judgement" | "storm of judgement" => {
                self.sd_set(pm, "sourceRateIsFinal", true);
                Config {
                    trigger_name: Some(main_name.clone()),
                    trigger_skill_cond: cond(|p, k| Ok(p.skill_name(k) == "Queen's Demand")),
                    triggered_skill_cond: cond(|p, k| {
                        Ok(p.sd_truthy(k, "triggeredByUnique")
                            && p.group_slot(p.main()) == p.group_slot(k))
                    }),
                    ..Config::default()
                }
            }
            "trigger craft" => {
                if !self.sd_truthy(pm, "triggeredByCraft") {
                    return Ok(None);
                }
                let (mut rate, mut source, mut uuid) = (None, None, None);
                let mut use_cast_rate = false;
                let mut triggered = Vec::new();
                for k in 0..self.n_player {
                    let ge = self.skill(k).active_effect.granted_effect.clone();
                    if (self.ty(k, "Damage") || self.ty(k, "Attack") || self.ty(k, "Spell"))
                        && !self.has_flag(k, "aura")
                        && k != pm
                        && !self.sd_truthy(k, "triggeredByCraft")
                        && !ge.from_item
                        && !self.is_triggered(k)
                    {
                        (source, rate, uuid) =
                            self.find_trigger_skill(k, source, rate, Comparer::Default)?;
                        let f = |x: &str| self.has_flag(k, x);
                        if (f("totem") || f("golem") || f("banner") || f("ballista"))
                            && let Some(ct) = ge.cast_time
                        {
                            let lvl_cd = self
                                .skill(k)
                                .active_effect
                                .level
                                .and_then(|l| ge.level(l))
                                .and_then(|l| l.field("cooldown"))
                                .unwrap_or(0.0);
                            rate = Some(1.0 / (ct + lvl_cd));
                            use_cast_rate = true;
                        }
                    }
                    if self.sd_truthy(k, "triggeredByCraft")
                        && self.group_slot(pm) == self.group_slot(k)
                    {
                        let s = self.sim(k)?;
                        triggered.push(s);
                    }
                }
                Config {
                    trig_rate: rate,
                    source,
                    uuid,
                    use_cast_rate,
                    triggered_skills: Some(triggered),
                    ..Config::default()
                }
            }
            "kitava's thirst" | "foulborn kitava's thirst" => {
                let foulborn = key.starts_with("foulborn");
                let (req, chance, name, cost) = if foulborn {
                    (
                        "FoulbornKitavaRequiredLifeCost",
                        "FoulbornKitavaTriggerChance",
                        "Foulborn Kitava's Thirst",
                        "LifeCost",
                    )
                } else {
                    (
                        "KitavaRequiredManaCost",
                        "KitavaTriggerChance",
                        "Kitava's Thirst",
                        "ManaCost",
                    )
                };
                let required = self.c.base(pdb, None, &[req])?;
                Config {
                    trigger_chance: Some(self.c.base(pdb, None, &[chance])?),
                    trigger_name: Some(name.into()),
                    comparer: Some(Comparer::Cost(cost, required)),
                    trigger_skill_cond: cond(|_, _| Ok(true)),
                    ..Config::default()
                }
            }
            "mjolner" | "wing of the wyvern" => {
                let mjolner = key == "mjolner";
                Config {
                    trigger_skill_cond: cond(move |p, k| {
                        let flags = if mjolner {
                            p.cfg(k).flags.unwrap_or(0) & (p.mf("Mace") | p.mf("Weapon1H")) != 0
                        } else {
                            p.cfg_flag(k, "Bow")
                        };
                        Ok((p.ty(k, "Damage") || p.ty(k, "Attack")) && flags && !p.slot_match(k))
                    }),
                    triggered_skill_cond: cond(move |p, k| {
                        let by = if mjolner {
                            "triggeredByMjolner"
                        } else {
                            "triggeredByUnique"
                        };
                        Ok(p.sd_truthy(k, by) && p.slot_match(k))
                    }),
                    ..Config::default()
                }
            }
            "cospri's malice" => Config {
                trigger_skill_cond: cond(|p, k| {
                    Ok(p.ty(k, "Melee")
                        && p.cfg(k).flags.unwrap_or(0) & (p.mf("Sword") | p.mf("Weapon1H")) != 0)
                }),
                triggered_skill_cond: cond(|p, k| {
                    Ok(p.sd_truthy(k, "triggeredByCospris")
                        && p.group_slot(p.main()) == p.group_slot(k))
                }),
                ..Config::default()
            },
            "seven teachings" => Config {
                trigger_skill_cond: cond(|p, k| {
                    let want = p.mf("Unarmed") | p.mf("Melee");
                    Ok(p.ty(k, "Melee") && p.cfg(k).flags.unwrap_or(0) & want == want)
                }),
                triggered_skill_cond: cond(|p, k| {
                    Ok(p.sd_truthy(k, "triggeredBySevenTeachings") && p.slot_match(k))
                }),
                ..Config::default()
            },
            "squirming terror" | "cast on melee kill" => {
                if self.c.flag(pdb, None, &["Condition:KilledRecently"])? {
                    let terror = key == "squirming terror";
                    Config {
                        assuming_every_hit_kills: true,
                        trigger_skill_cond: cond(move |p, k| {
                            Ok(
                                p.ty(k, "Attack")
                                    && p.ty(k, "Melee")
                                    && (terror || p.slot_match(k)),
                            )
                        }),
                        triggered_skill_cond: cond(move |p, k| {
                            let by = if terror {
                                "triggeredBySquirmingTerror"
                            } else {
                                "triggeredByMeleeKill"
                            };
                            Ok(p.sd_truthy(k, by) && p.slot_match(k))
                        }),
                        ..Config::default()
                    }
                } else {
                    let msg = if key == "squirming terror" {
                        "Squirming Terror requires recent kills"
                    } else {
                        "Cast on Melee Kill requires recent kills"
                    };
                    let mm = self.mods_mut(pm);
                    mm.info_message2 = Some("DPS reported assuming Self-Cast".into());
                    mm.info_message = Some(msg.into());
                    return Ok(None);
                }
            }
            "kinetic flux" | "cast on critical strike" => {
                let by = if key == "kinetic flux" {
                    "triggeredByKineticFlux"
                } else {
                    "triggeredByCoc"
                };
                Config {
                    trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack") && p.slot_match(k))),
                    triggered_skill_cond: cond(move |p, k| {
                        Ok(p.sd_truthy(k, by) && p.slot_match(k))
                    }),
                    ..Config::default()
                }
            }
            "nova" => {
                let Some(ma) = self.minion_actor() else {
                    return Ok(None);
                };
                let mm = self.actor_main_index(ma).ok_or("no minion main skill")?;
                Config {
                    trigger_name: Some("Summon Holy Relic".into()),
                    actor: Some(ma),
                    triggered_skills: Some(vec![Sim {
                        uuid: skill_uuid(self, mm),
                        cd: self.sd_num(mm, "cooldown"),
                        cd_override: None,
                        adds_cast_time: None,
                        icdr: 1.0,
                        added_cooldown: 0.0,
                    }]),
                    trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack"))),
                    ..Config::default()
                }
            }
            "cast when damage taken" => {
                let Some(t) = self.sd_num(pm, "triggeredByDamageTaken") else {
                    return Ok(None);
                };
                let list = self.lists[pm].list;
                let m = self.c.calc_mod(list, None, &["CWDTThreshold"])?;
                self.c.set_out(PLAYER, "CWDTThreshold", t * m);
                self.set_flag(pm, "globalTrigger", true);
                Config {
                    source: Some(pm),
                    ..Config::default()
                }
            }
            "cast when stunned" | "cast on ward break" => {
                self.set_flag(pm, "globalTrigger", true);
                let k = if key == "cast when stunned" {
                    "chanceToTriggerOnStun"
                } else {
                    "chanceToTriggerOnWardBreak"
                };
                Config {
                    trigger_chance: self.sd_num(pm, k),
                    source: Some(pm),
                    ..Config::default()
                }
            }
            "spellslinger" => {
                if main_name == "Spellslinger" {
                    self.set_flag(pm, "skipEffectiveRate", true);
                }
                self.sd_set(pm, "sourceRateIsFinal", true);
                Config {
                    trigger_name: Some("Spellslinger".into()),
                    trigger_on_use: true,
                    trigger_skill_cond: cond(|p, k| {
                        Ok(p.ty(k, "Attack")
                            && p.ty(k, "Projectile")
                            && p.cfg_flag(k, "Wand")
                            && !p.sd_truthy(k, "triggeredBySpellSlinger"))
                    }),
                    ..Config::default()
                }
            }
            "call to arms" | "automation" | "autoexertion" => {
                let active = match key {
                    "call to arms" => "Call to Arms",
                    "automation" => "Automation",
                    _ => "Autoexertion",
                };
                if main_name == active {
                    self.set_flag(pm, "globalTrigger", true);
                    self.set_flag(pm, "skipEffectiveRate", true);
                } else if let Some(k) = (0..self.n_player).find(|&k| self.skill_name(k) == active) {
                    let ge = self.skill(k).active_effect.granted_effect.clone();
                    if self.mods(pm).triggered_by.is_none() {
                        return Err("attempt to index a nil value (triggeredBy)".into());
                    }
                    self.mods_mut(pm).triggered_by_effect = Some(ge);
                }
                if key != "automation" {
                    self.sd_set(pm, "sourceRateIsFinal", true);
                    self.sd_set(pm, "ignoresTickRate", true);
                }
                Config {
                    trigger_on_use: true,
                    use_cast_rate: true,
                    source: Some(pm),
                    ..Config::default()
                }
            }
            "seize the flesh" => Config {
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Warcry") && p.slot_match(k))),
                ..Config::default()
            },
            "fissure" => Config {
                trigger_on_use: true,
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Slam") && p.slot_match(k))),
                ..Config::default()
            },
            "mark on hit" => Config {
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack"))),
                ..Config::default()
            },
            "hextouch" => {
                self.sd_set(pm, "sourceRateIsFinal", true);
                Config {
                    trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack") && p.slot_match(k))),
                    ..Config::default()
                }
            }
            "oskarm" => {
                self.sd_set(pm, "sourceRateIsFinal", true);
                Config {
                    trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack"))),
                    ..Config::default()
                }
            }
            "shattershard" => {
                self.set_flag(pm, "globalTrigger", true);
                let cached = self
                    .cached_calc(pm, true)?
                    .ok_or("attempt to index a nil value (cachedData)")?;
                let d = cached.num("Duration").unwrap_or(f64::NAN);
                self.sd_set(pm, "triggerRateCapOverride", 1.0 / d);
                if let Some(bd) = self.bd(PLAYER) {
                    bd.set(
                        "SkillTriggerRate",
                        Bd::of([
                            "Shattershard uses duration as pseudo cooldown".to_string(),
                            sf!("1 / %.2f ^8(Shattershard duration)", d),
                            sf!("= %.2f ^8per second", 1.0 / d),
                        ]),
                    );
                }
                Config {
                    source: Some(pm),
                    ..Config::default()
                }
            }
            "battlemage's cry" => {
                if main_name == "Battlemage's Cry" {
                    return Ok(None);
                }
                Config {
                    trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Melee"))),
                    comparer: Some(Comparer::Uptime("BattlemageUpTimeRatio")),
                    triggered_skill_cond: cond(|p, k| {
                        Ok(p.sd_truthy(k, "triggeredByBattleMageCry") && p.slot_match(k))
                    }),
                    ..Config::default()
                }
            }
            "arcanist brand" => {
                if main_name == "Arcanist Brand" {
                    return Ok(None);
                }
                self.sd_set(pm, "sourceRateIsFinal", true);
                if self.mods(pm).triggered_by.is_none() {
                    return Err("attempt to index a nil value (triggeredBy)".into());
                }
                if let Some(k) =
                    (0..self.n_player).find(|&k| self.skill_name(k) == "Arcanist Brand")
                {
                    self.mods_mut(pm).triggered_by_main = Some(k);
                }
                let b = self
                    .mods(pm)
                    .triggered_by_main
                    .ok_or("attempt to index a nil value (triggeredBy.mainSkill)")?;
                let (bl, bc) = (self.lists[b].list, self.cfg(b));
                let inc = (100.0
                    + self
                        .c
                        .inc(bl, Some(&bc), &["Speed", "BrandActivationFrequency"])?)
                    / 100.0;
                let more = self.c.more(bl, Some(&bc), &["BrandActivationFrequency"])?;
                let attached = self.sd_num(b, "attachedBrandCount").unwrap_or(f64::NAN);
                let mm = self.mods_mut(pm);
                mm.triggered_by_brand = Some((inc, more, attached));
                mm.triggered_by_ignores_tick_rate = true;
                Config {
                    trig_rate: Some(
                        self.sd_num(b, "repeatFrequency").unwrap_or(f64::NAN)
                            * inc
                            * more
                            * attached,
                    ),
                    source: Some(b),
                    triggered_skill_cond: cond(|p, k| {
                        Ok(p.sd_truthy(k, "triggeredByBrand") && p.slot_match(k))
                    }),
                    ..Config::default()
                }
            }
            "cast on death" => {
                self.set_flag(pm, "globalTrigger", true);
                self.sd_set(pm, "triggered", true);
                self.mods_mut(pm).info_message = Some(format!("{main_name} Triggered on Death"));
                return Ok(None);
            }
            "combust" => Config {
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Melee"))),
                comparer: Some(Comparer::Uptime("InfernalUpTimeRatio")),
                ..Config::default()
            },
            "prismatic burst" => Config {
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack") && p.slot_match(k))),
                ..Config::default()
            },
            "voidstorm" => Config {
                trigger_on_use: true,
                trigger_skill_cond: cond(|p, k| {
                    Ok(p.ty(k, "Attack") && p.ty(k, "Rain") && p.slot_match(k))
                }),
                ..Config::default()
            },
            "shockwave" | "void shockwave" | "falling crystal" => Config {
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Melee") && p.slot_match(k))),
                ..Config::default()
            },
            "call the pyre" => {
                if self.c.flag(self.edb(), None, &["Condition:Ignited"])? {
                    Config {
                        trigger_chance: Some(50.0),
                        trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Melee") && p.slot_match(k))),
                        ..Config::default()
                    }
                } else {
                    self.mods_mut(pm).info_message =
                        Some("Call the Pyre requires Ignited enemies".into());
                    return Ok(None);
                }
            }
            "manaforged arrows" => Config {
                trigger_on_use: true,
                trigger_name: Some("Manaforged Arrows".into()),
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack") && p.cfg_flag(k, "Bow"))),
                ..Config::default()
            },
            "doom blast" => {
                if self.config_str("doomBlastSource").as_deref() == Some("replacement") {
                    self.c
                        .add(pdb, src("UsesCurseOverlaps", ModType::Flag, true, "Config"));
                }
                self.sd_set(pm, "ignoresTickRate", true);
                let has = !self
                    .c
                    .tabulate(
                        pdb,
                        Some(ModType::Base),
                        None,
                        &["Multiplier:CurseOverlaps"],
                    )?
                    .is_empty();
                let overlaps = if has {
                    Some(m_max(
                        self.c.base(pdb, None, &["Multiplier:CurseOverlaps"])?,
                        1.0,
                    ))
                } else {
                    None
                };
                Config {
                    use_cast_rate: true,
                    overlaps,
                    custom_trigger_name: Some("Doom Blast triggering Hex: ".into()),
                    trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Hex") && p.slot_match(k))),
                    ..Config::default()
                }
            }
            "cast while channelling" => Config {
                custom_handler: Some(Custom::Cwc),
                ..Config::default()
            },
            "focus" => Config {
                custom_handler: Some(Custom::Focus),
                ..Config::default()
            },
            "snipe" => return self.snipe_config(),
            "avenging flame" => Config {
                trigger_skill_cond: cond(|p, k| Ok(p.has_flag(k, "totem") && p.slot_match(k))),
                comparer: Some(Comparer::TotemLife),
                ignore_source_rate: true,
                ..Config::default()
            },
            "intuitive link" => {
                if main_name == "Intuitive Link" {
                    return Ok(None);
                }
                if self.mods(pm).triggered_by.is_none() {
                    return Err("attempt to index a nil value (triggeredBy)".into());
                }
                if let Some(k) =
                    (0..self.n_player).find(|&k| self.skill_name(k) == "Intuitive Link")
                {
                    self.mods_mut(pm).triggered_by_main = Some(k);
                }
                let b = self.mods(pm).triggered_by_main;
                Config {
                    triggered_skill_cond: cond(move |p, k| {
                        Ok(p.ty(k, "Spell") && p.slot_match(k) && Some(k) != b)
                    }),
                    trig_rate: Some(self.c.base(pdb, None, &["IntuitiveLinkSourceRate"])?),
                    source: b,
                    source_name: Some("Custom source".into()),
                    use_cast_rate: true,
                    ..Config::default()
                }
            }
            "svalinn cast on block" | "festering resentment cast on block" => {
                self.set_flag(pm, "globalTrigger", true);
                Config {
                    source: Some(pm),
                    triggered_skill_cond: cond(|p, k| {
                        let Some((ge, _)) = p.triggered_by(k) else {
                            return Ok(false);
                        };
                        Ok(p.slot_match(k) && p.can_support_skill(&ge, k))
                    }),
                    ..Config::default()
                }
            }
            "hex on trap" => Config {
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Trapped"))),
                triggered_skill_cond: cond(|p, k| {
                    Ok(p.sd_truthy(k, "triggeredByTrapTrigger") && p.slot_match(k))
                }),
                ..Config::default()
            },
            "supporttriggerfirespellonhit" => Config {
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Melee"))),
                triggered_skill_cond: cond(|p, k| {
                    Ok(p.sd_truthy(k, "triggeredBySettlersEnchantTrigger") && p.slot_match(k))
                }),
                ..Config::default()
            },
            "ghostly artillery" => Config {
                source_weapon: true,
                trigger_on_use: true,
                trigger_skill_cond: cond(|p, k| Ok(p.ty(k, "Attack"))),
                ..Config::default()
            },
            "replica gifts from above" => Config {
                trigger_skill_cond: cond(|p, k| Ok(!p.ty(k, "SummonsTotem") && p.ty(k, "Attack"))),
                ..Config::default()
            },
            "bursting toad" => {
                let interval = (0..self.n_player)
                    .filter_map(|k| self.sd_num(k, "hextoadTriggerInterval"))
                    .fold(f64::INFINITY, m_min);
                if interval < f64::INFINITY {
                    self.set_flag(pm, "globalTrigger", true);
                    self.sd_set(pm, "triggerRateCapOverride", 1.0 / interval);
                    if let Some(bd) = self.bd(PLAYER) {
                        bd.set(
                            "TriggerRateCap",
                            Bd::of([
                                sf!("1 / %.2f ^8(Hextoad trigger interval)", interval),
                                sf!("= %.2f", 1.0 / interval),
                            ]),
                        );
                    }
                    Config {
                        source: Some(pm),
                        ..Config::default()
                    }
                } else {
                    return Ok(None);
                }
            }
            "TriggeredMoltenStrike" => Config {
                trigger_skill_cond: attack_or("Melee"),
                ..Config::default()
            },
            "FieryImpactHeistMaceImplicit" => Config {
                source_weapon: true,
                trigger_skill_cond: attack_or("Melee"),
                ..Config::default()
            },
            _ => return Ok(None),
        };
        Ok(Some(c))
    }

    fn config_str(&self, k: &str) -> Option<String> {
        match self.input.config.get(k) {
            Some(crate::config::Val::Str(s)) => Some(s.to_string()),
            _ => None,
        }
    }

    /// `calcLib.canGrantedEffectSupportActiveSkill(grantedEffect, skill)`
    fn can_support_skill(&self, ge: &crate::skill_data::GrantedEffect, k: usize) -> bool {
        let summon = self
            .minion_of_skill(k)
            .map(|m| self.skill(self.minions[m].skill).clone());
        let ctx = crate::calc_skills::SupportContext {
            from_item: self.env.skills.from_item.clone(),
            groups: &self.env.groups,
            all_one_hand: self
                .c
                .actors
                .actors
                .iter()
                .take(1)
                .flat_map(|a| a.weapon_data.iter().flatten())
                .any(|d| d.get("countsAsAll1H").is_some_and(ModValue::truthy)),
            data: self.input.data.skills,
            summon: summon.as_ref(),
        };
        ctx.can_support(ge, self.skill(k))
    }

    /// The `snipe` configuration
    fn snipe_config(&mut self) -> Result<Option<Config>> {
        let pm = self.main();
        let pdb = self.pdb();
        let (ml, mc) = (self.lists[pm].list, self.cfg(pm));
        let mut stages = m_min(
            self.c.base(pdb, None, &["Multiplier:SnipeStage"])?,
            self.c.base(pdb, None, &["Multiplier:SnipeStagesMax"])?,
        );
        let hit_multi = self.c.base(ml, Some(&mc), &["snipeHitMulti"])?;
        let ailment_multi = self.c.base(ml, Some(&mc), &["snipeAilmentMulti"])?;
        let main_slot = self.group_slot(pm);
        let triggered: Vec<usize> = (0..self.n_player)
            .filter(|&k| {
                self.sd_truthy(k, "triggeredBySnipe")
                    && self.skill(k).group.is_some()
                    && self.group_slot(k) == main_slot
            })
            .collect();
        let hit = self.mf("Hit");
        let ailment = self.mf("Ailment");
        let stage_mods = |p: &mut Self, stages: f64, ailment_first: bool| {
            p.c.add(
                ml,
                src("Multiplier:SnipeStages", ModType::Base, stages, "Snipe"),
            );
            let h = Mod::new("Damage", ModType::More, hit_multi, Some("Snipe"))
                .flags(hit, 0)
                .tag(crate::mods::Tag::multiplier("SnipeStages"));
            let a = Mod::new("Damage", ModType::More, ailment_multi, Some("Snipe"))
                .flags(ailment, 0)
                .tag(crate::mods::Tag::multiplier("SnipeStages"));
            if ailment_first {
                p.c.add(ml, a);
                p.c.add(ml, h);
            } else {
                p.c.add(ml, h);
                p.c.add(ml, a);
            }
        };
        if self.skill_name(pm) == "Snipe" {
            if self.input.limited.contains(&skill_uuid(self, pm)) {
                stages = 0.0;
            } else {
                self.sd_set(pm, "hitTimeMultiplier", m_max(1.0, stages) - 0.5);
            }
            if triggered.is_empty() {
                stage_mods(self, stages, false);
            } else {
                for e in ["Lightning", "Cold", "Fire", "Chaos", "Physical"] {
                    let mut t = crate::mods::Tag::skill_name("Snipe");
                    if let crate::mods::tag::TagKind::SkillName {
                        include_transfigured,
                        ..
                    } = &mut t.kind
                    {
                        *include_transfigured = true;
                    }
                    self.c.add(
                        ml,
                        Mod::new(&format!("DealNo{e}"), ModType::Flag, true, None).tag(t),
                    );
                }
            }
            return Ok(None);
        }
        let index = triggered
            .iter()
            .position(|&k| k == pm)
            .map(|x| x as f64 + 1.0);
        if let Some(index) = index
            && index <= stages
        {
            stage_mods(self, stages, true);
            let (mut source, mut rate) = (None, None);
            for k in 0..self.n_player {
                if self.skill_name(k) == "Snipe"
                    && self.skill(k).group.is_some()
                    && self.group_slot(k) == main_slot
                {
                    self.sd_set(k, "hitTimeMultiplier", stages - 0.5);
                    let cached = self
                        .cached_calc(k, false)?
                        .ok_or("attempt to index a nil value (cachedData)")?;
                    let speed = cached.num("HitSpeed");
                    let ok = !self.has_flag(k, "disable")
                        && !self
                            .cfg(k)
                            .skill_cond
                            .as_ref()
                            .is_some_and(|c| c.contains(&Sym::new("usedByMirage")))
                        && !self.ty(k, "OtherThingUsesSkill")
                        && match speed {
                            Some(s) => source.is_none() || s > rate.unwrap_or(0.0),
                            None => false,
                        };
                    if ok {
                        rate = speed;
                        let t = cached.num("HitTime").unwrap_or(f64::NAN);
                        self.c.set_out(PLAYER, "ChannelTimeToTrigger", t);
                        source = Some(k);
                    }
                }
            }
            return Ok(Some(Config {
                trig_rate: rate,
                source,
                ..Config::default()
            }));
        }
        self.sd_clear(pm, "triggered");
        let mm = self.mods_mut(pm);
        mm.info_message2 = Some("DPS reported assuming Self-Cast".into());
        mm.info_message = Some("Not enough Snipe stages to trigger this skill".into());
        mm.info_trigger = Some(String::new());
        Ok(None)
    }

    /// `helmetFocusHandler(env)`
    fn focus_handler(&mut self) -> Result<()> {
        let pm = self.main();
        if self.has_flag(pm, "minion")
            || self.has_flag(pm, "disable")
            || self.mods(pm).triggered_by.is_none()
        {
            return Ok(());
        }
        self.sd_set(pm, "triggered", true);
        let trigger_cd = self.triggered_by_cooldown(pm);
        let triggered_cd = self.sd_num(pm, "cooldown");
        let (ml, mc) = (self.lists[pm].list, self.cfg(pm));
        let icdr_focus = self.c.calc_mod(ml, Some(&mc), &["FocusCooldownRecovery"])?;
        let icdr = self.c.calc_mod(ml, Some(&mc), &["CooldownRecovery"])?;
        let focus = self
            .input
            .data
            .skills
            .skills
            .get("Focus")
            .cloned()
            .ok_or("no skill Focus")?;
        let duration = focus
            .constant_stats
            .first()
            .and_then(|s| s.1)
            .unwrap_or(f64::NAN)
            / 1000.0;
        let cd = focus
            .level(1.0)
            .and_then(|l| l.field("cooldown"))
            .unwrap_or(f64::NAN)
            / icdr_focus;
        let total = duration + cd;
        let action = m_max(
            triggered_cd.unwrap_or(0.0),
            trigger_cd.unwrap_or(0.0) / icdr,
        );
        let tick = self.misc_data("ServerTickRate");
        let adjusted = (action * tick).ceil() / tick;
        let rate = if adjusted != 0.0 {
            1.0 / adjusted
        } else {
            f64::INFINITY
        };
        self.c.set_out(PLAYER, "TriggerRateCap", rate);
        self.c.set_out(PLAYER, "SkillTriggerRate", 1.0 / total);
        if self.bd(PLAYER).is_some() {
            let tick_line = if self.sd_truthy(self.main(), "ignoresTickRate") {
                String::new()
            } else {
                sf!("%.3f ^8(adjusted for server tick rate)", adjusted)
            };
            let tcd = trigger_cd.unwrap_or(f64::NAN);
            let cap = match triggered_cd {
                Some(t) => Bd::of([
                    sf!("%.2f ^8(base cooldown of triggered skill)", t),
                    sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr),
                    sf!("= %.4f ^8(final cooldown of triggered skill)", t / icdr),
                    String::new(),
                    sf!("%.2f ^8(base cooldown of trigger)", tcd),
                    sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr),
                    sf!("= %.4f ^8(final cooldown of trigger)", tcd / icdr),
                    String::new(),
                    sf!(
                        "%.3f ^8(biggest of trigger cooldown and triggered skill cooldown)",
                        action
                    ),
                    String::new(),
                    tick_line,
                    String::new(),
                    "Trigger rate:".to_string(),
                    sf!("1 / %.3f", adjusted),
                    sf!("= %.2f ^8per second", rate),
                ]),
                None => Bd::of([
                    "Triggered skill has no base cooldown".to_string(),
                    String::new(),
                    sf!("%.2f ^8(base cooldown of trigger)", tcd),
                    sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr),
                    sf!("= %.4f ^8(final cooldown of trigger)", tcd / icdr),
                    String::new(),
                    tick_line,
                    String::new(),
                    "Trigger rate:".to_string(),
                    sf!("1 / %.3f", adjusted),
                    sf!("= %.3f ^8per second", rate),
                ]),
            };
            let base_cd = focus
                .level(1.0)
                .and_then(|l| l.field("cooldown"))
                .unwrap_or(f64::NAN);
            let str_ = Bd::of([
                sf!("%.2f ^8(focus base cooldown)", base_cd),
                sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr_focus),
                sf!(
                    "+ %.2f ^8(skills are only triggered on activation thus we add focus duration)",
                    duration
                ),
                sf!(
                    "= %.2f ^8(effective skill cooldown for trigger purposes)",
                    total
                ),
                String::new(),
                sf!(
                    "%.3f casts per second ^8(Assuming player uses focus exactly when its cooldown of %.2fs expires)",
                    1.0 / total,
                    total
                ),
            ]);
            let bd = self.bd(PLAYER).unwrap();
            bd.set("TriggerRateCap", cap);
            bd.set("SkillTriggerRate", str_);
        }
        self.add_trigger_inc_more(pm, pm)?;
        let mm = self.mods_mut(pm);
        mm.info_message = Some("Assuming perfect focus Re-Use".into());
        mm.info_trigger = Some("Focus".into());
        self.sd_set(pm, "triggerRate", 1.0 / total);
        self.set_flag(pm, "globalTrigger", true);
        Ok(())
    }

    /// `CWCHandler(env)`
    fn cwc_handler(&mut self) -> Result<()> {
        let pm = self.main();
        if self.has_flag(pm, "minion") || self.has_flag(pm, "disable") {
            return Ok(());
        }
        let mut triggered = Vec::new();
        let mut source = None;
        let gem = self
            .mods(pm)
            .triggered_by
            .and_then(|t| self.skill(pm).effect_list[t].gem.clone());
        for k in 0..self.n_player {
            let sm = self.slot_match(k);
            if source.is_none() {
                let can = gem
                    .as_ref()
                    .is_some_and(|g| self.can_support_skill(&g.granted_effect, k));
                if self.sd(k, "triggerTime").is_some()
                    && can
                    && k != pm
                    && sm
                    && !self.is_triggered(k)
                {
                    source = Some(k);
                }
            }
            if self.sd_truthy(k, "triggeredWhileChannelling") && sm {
                let s = self.sim(k)?;
                triggered.push(s);
            }
        }
        let Some(source) = source.filter(|_| !triggered.is_empty()) else {
            self.sd_clear(pm, "triggered");
            let mm = self.mods_mut(pm);
            mm.info_message2 = Some("DPS reported assuming Self-Cast".into());
            mm.info_message = Some("No Cast While Channeling Triggering Skill Found".into());
            mm.info_trigger = Some(String::new());
            return Ok(());
        };
        let adds = self.added_cast_time(pm, Some(PLAYER))?;
        match adds {
            Some(v) => self.c.set_out(PLAYER, "addsCastTime", v),
            None => self.c.clear_out(PLAYER, "addsCastTime"),
        }
        let (ml, mc) = (self.lists[pm].list, self.cfg(pm));
        let icdr = self.c.calc_mod(ml, Some(&mc), &["CooldownRecovery"])?;
        let interval = self.sd_num(source, "triggerTime").unwrap_or(f64::NAN);
        let rate_of_trigger = 1.0 / interval;
        let triggered_cd = self.sd_num(pm, "cooldown");
        let over = self.c.override_num(ml, Some(&mc), &["CooldownRecovery"])?;
        if over.is_some() {
            self.set_flag(pm, "hasOverride", true);
        }
        let total = over.unwrap_or(m_max(triggered_cd.unwrap_or(0.0), adds.unwrap_or(0.0)) / icdr);
        let tick = self.misc_data("ServerTickRate");
        let adjusted = (total * tick).ceil() / tick;
        let eff = (adjusted * rate_of_trigger).ceil() / rate_of_trigger;
        self.c
            .set_out(PLAYER, "TriggerRateCap", m_min(1.0 / eff, rate_of_trigger));
        let (mut rate, sim) =
            self.rotation_rate(&mut triggered, rate_of_trigger, 0.0, 100.0, PLAYER);
        let bots = self.c.flag(self.pdb(), None, &["HaveTriggerBots"])? && self.ty(pm, "Spell");
        if bots {
            rate *= 2.0;
        }
        if self.bd(PLAYER).is_some() {
            let name = self.skill_name(pm).to_string();
            let sname = self.skill_name(source).to_string();
            let head = sf!(
                "Cast While Channeling triggers %s every %.2fs while channeling %s ",
                name.as_str(),
                interval,
                sname.as_str()
            );
            let mut cap = Vec::new();
            if triggered_cd.is_some() || over.is_some() {
                let t = triggered_cd.unwrap_or(f64::NAN);
                cap.push(head);
                cap.push(String::new());
                cap.push(sf!("%.2f ^8(base cooldown of triggered skill)", t));
                cap.push(sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr));
                cap.push(sf!(
                    "= %.4f ^8(final cooldown of triggered skill)",
                    t / icdr
                ));
                cap.push(String::new());
                if let Some(o) = over {
                    cap[3] = sf!("%.2f ^8(hard override of cooldown of %s)", o, name.as_str());
                    cap.remove(4);
                    cap.remove(4);
                }
            } else {
                cap.push(head);
                cap.push(String::new());
                cap.push(format!("{name} has no base cooldown or cooldown override"));
                cap.push(String::new());
            }
            let extra = |affected: f64| -> Option<f64> {
                if over.is_some() {
                    return None;
                }
                let next = eff - interval;
                let over_bp = total - next;
                let reduced = total * icdr - total;
                if over_bp < affected {
                    let div = affected / (affected - over_bp - reduced);
                    let inc = ((div - 1.0) * 100.0).ceil();
                    Some(inc - (icdr - 1.0) * 100.0)
                } else {
                    None
                }
            };
            if let Some(a) = adds {
                cap.push("Cast While Channeling had no base cooldown".to_string());
                cap.push(sf!(
                    "+ %.2f ^8(%s adds cast time as cooldown to trigger)",
                    a,
                    name.as_str()
                ));
                cap.push(sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr));
                cap.push(sf!(
                    "= %.4f ^8(final cooldown of triggered skill)",
                    a / icdr
                ));
                cap.push(String::new());
                cap.push(sf!("%.3f ^8(adjusted for server tick rate)", adjusted));
                cap.push(String::new());
                let csi = extra(a);
                if let Some(e) = extra(total * icdr) {
                    cap.push(sf!("^8(extra ICDR of %d%% would reach next breakpoint)", e));
                }
                if let Some(e) = csi {
                    cap.push(sf!(
                        "^8(extra Cast Rate Increase of %d%% would reach next breakpoint)",
                        e
                    ));
                    cap.push(String::new());
                }
            } else if let Some(e) = extra(total * icdr) {
                cap.push(sf!("^8(extra ICDR of %d%% would reach next breakpoint)", e));
                cap.push(String::new());
            }
            let trc = m_min(1.0 / eff, rate_of_trigger);
            cap.push("Trigger rate:".to_string());
            cap.push(sf!(
                "1 / %.3f ^8(trigger rate adjusted for triggering interval)",
                1.0 / trc
            ));
            cap.push(sf!("= %.2f ^8 %s casts per second", trc, name.as_str()));
            let bd = self.bd(PLAYER).unwrap();
            bd.set("TriggerRateCap", Bd::of(cap));
            if triggered.len() > 1 {
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
                        "x 2 ^8(Trigger bots effectively cause the skill to trigger twice)"
                            .to_string(),
                    );
                }
                bd.set("SkillTriggerRate", Bd::of(l));
                bd.set("SimData", sim_data(&sim));
            }
        }
        self.c.set_out(PLAYER, "SkillTriggerRate", rate);
        self.add_trigger_inc_more(pm, pm)?;
        self.c.set_out(PLAYER, "ChannelTimeToTrigger", interval);
        self.sd_set(pm, "triggered", true);
        self.set_flag(pm, "globalTrigger", true);
        self.sd_set(pm, "triggerRate", rate);
        let su = skill_uuid(self, source);
        self.sd_set(pm, "triggerSourceUUID", su.as_str());
        let name = self.skill_name(source).to_string();
        self.mods_mut(pm).info_message = Some(format!("Cast While Channeling's Trigger: {name}"));
        // `env.player.infoTrigger` is set, not the skill's
        Ok(())
    }

    /// `defaultTriggerHandler(env, config)`
    fn default_trigger_handler(&mut self, config: Config, actor: usize) -> Result<()> {
        let pm = self.main();
        let m = self.actor_main_index(actor).ok_or("no main skill")?;
        let pdb = self.pdb();
        let comparer = config.comparer.unwrap_or(Comparer::Default);
        let mut source = config.source;
        let mut trig_rate = config.trig_rate;
        let mut uuid = config.uuid.clone();
        let mut triggered_skills = config.triggered_skills.clone().unwrap_or_default();
        let source_weapon_flag = if config.source_weapon {
            self.group_slot(m).map(|s| {
                if s.starts_with("Weapon 2") {
                    "weapon2Attack"
                } else {
                    "weapon1Attack"
                }
            })
        } else {
            None
        };
        let global = |p: &Self| p.has_flag(m, "globalTrigger");
        if config.triggered_skill_cond.is_some() || config.trigger_skill_cond.is_some() {
            for k in 0..self.n_player {
                if let Some(c) = &config.trigger_skill_cond
                    && c(self, k)?
                    && source_weapon_flag.is_none_or(|f| self.has_flag(k, f))
                    && (!self.is_triggered(k) || global(self) || config.allow_triggered)
                    && k != m
                {
                    (source, trig_rate, uuid) =
                        self.find_trigger_skill(k, source, trig_rate, comparer)?;
                }
                if let Some(c) = &config.triggered_skill_cond
                    && c(self, k)?
                {
                    let s = self.sim(k)?;
                    triggered_skills.push(s);
                }
            }
        }
        if !(!triggered_skills.is_empty() || config.triggered_skill_cond.is_none()) {
            return Ok(());
        }
        if source.is_none() && !(global(self) && config.triggered_skill_cond.is_some()) {
            self.sd_clear(m, "triggered");
            let name = config.trigger_name.clone().unwrap_or_default();
            let mm = self.mods_mut(m);
            mm.info_message2 = Some("DPS reported assuming Self-Cast".into());
            mm.info_message = Some(format!("No {name} Triggering Skill Found"));
            mm.info_trigger = Some(String::new());
            return Ok(());
        }
        self.sd_set(m, "triggered", true);
        let mut cached = uuid.as_deref().and_then(|u| self.cached_now(u));
        let bd_on = self.bd(actor).is_some();
        // `breakdown.EffectiveSourceRate`, set on the actor's breakdown below
        let mut esr: Vec<String> = Vec::new();
        if bd_on {
            if self.sd_truthy(m, "triggeredByBrand") {
                let b = self.mods(m).triggered_by_main;
                let (inc, more, attached) =
                    self.mods(m)
                        .triggered_by_brand
                        .unwrap_or((f64::NAN, f64::NAN, f64::NAN));
                let freq = b
                    .and_then(|b| self.sd_num(b, "repeatFrequency"))
                    .unwrap_or(f64::NAN);
                let bname = b
                    .map(|b| self.skill_name(b).to_string())
                    .unwrap_or_default();
                let tname = config.trigger_name.clone().unwrap_or_default();
                esr = vec![
                    sf!(
                        "%.2f ^8(base activation cooldown of %s)",
                        freq,
                        tname.as_str()
                    ),
                    sf!("* %.2f ^8(more activation frequency)", more),
                    sf!("* %.2f ^8(increased activation frequency)", inc),
                    sf!("* %d ^8(attached Brands)", attached),
                    sf!(
                        "= %.2f ^8(activation rate of %s)",
                        trig_rate.unwrap_or(f64::NAN),
                        bname.as_str()
                    ),
                ];
            } else if let (Some(r), Some(s)) = (trig_rate, source) {
                if config.assuming_every_hit_kills {
                    esr.push("Assuming every attack kills".into());
                }
                let name = config
                    .source_name
                    .clone()
                    .unwrap_or_else(|| self.skill_name(s).to_string());
                esr.push(sf!(
                    "%.2f ^8(%s %s)",
                    r,
                    name.as_str(),
                    if config.use_cast_rate {
                        "cast rate"
                    } else {
                        "attack rate"
                    }
                ));
            }
        }
        let wtype = |p: &Self, n: usize| p.weapon_str(PLAYER, n, "type").is_some();
        // Dual wield triggers
        if let (Some(s), Some(r)) = (source, trig_rate) {
            let source_weapon_trigger =
                config.source_weapon && self.has_flag(s, "bothWeaponAttack");
            let item_support_trigger = self.triggered_by(m).is_some_and(|(ge, _)| {
                ge.support && (ge.from_item || self.env.skills.from_item.contains(&ge.id))
            });
            if wtype(self, 0)
                && wtype(self, 1)
                && !self.sd_truthy(s, "doubleHitsWhenDualWielding")
                && (self.ty(s, "Melee") || self.ty(s, "Attack"))
                && (source_weapon_trigger || item_support_trigger)
            {
                trig_rate = Some(r / 2.0);
                if bd_on {
                    esr.insert(1.min(esr.len()), "/ 2 ^8(due to dual wielding)".into());
                }
            }
        }
        let stored = self.sd_num(m, "storedUses");
        if !self.sd_truthy(m, "ignoresTickRate") {
            let v = stored.is_some_and(|u| u > 1.0);
            match self.sd(m, "ignoresTickRate") {
                Some(_) => self.sd_set(m, "ignoresTickRate", v),
                None if stored.is_some() => self.sd_set(m, "ignoresTickRate", v),
                None => {}
            }
        }
        // Unleash
        if let (Some(s), Some(c)) = (source, &cached)
            && self.c.flag(self.lists[s].list, None, &["HasSeals"])?
            && self.ty(s, "CanRapidFire")
        {
            let mult = c
                .skill_data
                .get("dpsMultiplier")
                .and_then(ModValue::as_number)
                .unwrap_or(1.0);
            trig_rate = trig_rate.map(|r| r * mult);
            if bd_on {
                esr.push(sf!("x %.2f ^8(multiplier from Unleash)", mult));
            }
            self.set_flag(m, "HasSeals", true);
            self.sd_set(m, "ignoresTickRate", true);
        }
        // Skills that hit several times per use
        if let (Some(s), Some(c)) = (source, &cached)
            && self
                .mods(s)
                .skill_part_name
                .as_deref()
                .is_some_and(|n| crate::pat::find(n, "(.*)All(.*)Projectiles(.*)").is_some())
            && self.has_flag(s, "projectile")
        {
            let mult = c.num("ProjectileCount").unwrap_or(1.0);
            trig_rate = trig_rate.map(|r| r * mult);
            if bd_on {
                esr.push(sf!("x %.2f ^8(%d projectiles hit)", mult, mult));
            }
        }
        // Kitava's Thirst
        if self.sd_truthy(m, "triggeredByManaSpent") {
            let s = source.ok_or("attempt to index a nil value (source)")?;
            let repeats = 1.0 + self.c.base(self.lists[s].list, None, &["RepeatCount"])?;
            trig_rate = trig_rate.map(|r| r / repeats);
            if bd_on && repeats > 1.0 {
                esr.push(sf!(
                    "/%d ^8(repeated attacks/casts do not count as they don't use mana)",
                    repeats
                ));
            }
        }
        let tick_time = self.misc_data("ServerTickTime");
        let ceil_b = |x: f64| tick_time * (x / tick_time).ceil();
        // Battlemage's Cry and Infernal Cry uptime
        for (cond, exerts, duration, cast, cooldown) in [
            (
                self.sd_truthy(m, "triggeredByBattleMageCry"),
                "BattleCryExertsCount",
                "BattleMageCryDuration",
                "BattleMageCryCastTime",
                "BattleMageCryCooldown",
            ),
            (
                self.skill_name(m) == "Combust",
                "InfernalExertsCount",
                "InfernalCryDuration",
                "InfernalCryCastTime",
                "InfernalCryCooldown",
            ),
        ] {
            if cond
                && let (Some(c), Some(s)) = (&cached, source)
                && self.ty(s, "Melee")
            {
                let n = |k: &str| c.num(k).unwrap_or(f64::NAN);
                let r = trig_rate.unwrap_or(f64::NAN);
                let hits = m_max(m_min(r * ceil_b(n(duration)), n(exerts)), 0.0);
                if bd_on {
                    esr.push(sf!(
                        "^8(min(%.2f * %.2f, %d) = %.2f average number of exerted attacks capped by exerted count)",
                        r,
                        ceil_b(n(duration)),
                        n(exerts),
                        hits
                    ));
                    esr.push(sf!(
                        "= %.2f / (%.2f + %.2f) ^8(the calculated number of exerted attacks happens every cooldown + duration)",
                        hits,
                        n(cast),
                        ceil_b(n(cooldown))
                    ));
                }
                trig_rate = Some(hits / (n(cast) + ceil_b(n(cooldown))));
            }
        }
        // Manaforged Arrows
        if self.sd_truthy(m, "triggeredByManaforged") && trig_rate.is_some_and(|r| r > 0.0) {
            let tc = self
                .cached_calc(m, true)?
                .ok_or("attempt to index a nil value (cachedData)")?;
            let triggered_cost = tc.num("ManaCostRaw").unwrap_or(0.0);
            if triggered_cost > 0.0 {
                let threshold = triggered_cost
                    * self
                        .sd_num(m, "ManaForgedArrowsPercentThreshold")
                        .unwrap_or(f64::NAN);
                let source_cost = cached
                    .as_ref()
                    .ok_or("attempt to index a nil value (cachedData)")?
                    .num("ManaCostRaw")
                    .unwrap_or(0.0);
                if bd_on {
                    if source_cost > 0.0 {
                        let pct = self
                            .sd_num(m, "ManaForgedArrowsPercentThreshold")
                            .unwrap_or(f64::NAN);
                        let mut l = vec![
                            sf!("%.4f ^8(Mana cost of trigger source)", source_cost),
                            String::new(),
                            sf!("%.4f ^8(Mana Cost of triggered)", triggered_cost),
                            sf!("%.2f ^8(Manaforged threshold multiplier)", pct),
                            sf!("= %.4f ^8(Manaforged trigger threshold)", threshold),
                            String::new(),
                            sf!("%.4f ^8(Manaforged trigger threshold)", threshold),
                            sf!("/ %.4f ^8(Mana cost of trigger source)", source_cost),
                            sf!("= %.2f ^8(Skill usages required)", threshold / source_cost),
                            String::new(),
                        ];
                        l.extend(esr.first().cloned());
                        l.push(sf!(
                            "/ ceil(%.2f) ^8(%d Skill usages required)",
                            threshold / source_cost,
                            (threshold / source_cost).ceil()
                        ));
                        esr = l;
                    } else {
                        esr.push("Source skill has no mana cost".into());
                    }
                }
                trig_rate = if source_cost > 0.0 {
                    trig_rate.map(|r| r / (threshold / source_cost).ceil())
                } else {
                    Some(0.0)
                };
            }
        }
        let (ml, mc) = (self.lists[m].list, self.cfg(m));
        let c = Some(&mc);
        let icdr = self.c.calc_mod(ml, c, &["CooldownRecovery"])?;
        let added = self.c.base(ml, c, &["CooldownRecovery"])?;
        let added = (added != 0.0).then_some(added);
        let over = self.c.override_num(ml, c, &["CooldownRecovery"])?;
        let mut trigger_cd = if self.mods(m).triggered_by.is_some() {
            self.triggered_by_cooldown(pm)
        } else {
            None
        };
        if trigger_cd.is_none() {
            let s = source.ok_or("attempt to index a nil value (source)")?;
            if self.mods(s).triggered_by.is_some() {
                trigger_cd = self.triggered_by_cooldown(s);
            }
        }
        let triggered_cd = self.sd_num(m, "cooldown");
        if self.sd_truthy(m, "triggeredByBrand") {
            let b = self
                .mods(m)
                .triggered_by_main
                .ok_or("attempt to index a nil value (triggeredBy.mainSkill)")?;
            let (inc, more, _) =
                self.mods(m)
                    .triggered_by_brand
                    .unwrap_or((f64::NAN, f64::NAN, f64::NAN));
            let v = self.sd_num(b, "repeatFrequency").unwrap_or(f64::NAN) / more / inc;
            trigger_cd = Some(v * icdr);
        }
        let adds = self.added_cast_time(pm, Some(actor))?;
        match adds {
            Some(v) => self.c.set_out(actor, "addsCastTime", v),
            None => self.c.clear_out(actor, "addsCastTime"),
        }
        let tick = self.misc_data("ServerTickRate");
        let triggered_adj = (triggered_cd.unwrap_or(0.0) + added.unwrap_or(0.0)) / icdr;
        let trigger_adj = (trigger_cd.unwrap_or(0.0) + adds.unwrap_or(0.0)) / icdr;
        let triggered_rounded = if self.sd_truthy(m, "ignoresTickRate") {
            triggered_adj
        } else {
            (triggered_adj * tick).ceil() / tick
        };
        let trigger_rounded =
            if self.mods(m).triggered_by.is_some() && self.mods(m).triggered_by_ignores_tick_rate {
                trigger_adj
            } else {
                (trigger_adj * tick).ceil() / tick
            };
        let action_rounded = match over {
            Some(o) => (o * tick).ceil() / tick,
            None => m_max(trigger_rounded, triggered_rounded),
        };
        let mut cap = if source == Some(m) {
            self.sd_num(m, "triggerRateCapOverride")
                .unwrap_or(f64::INFINITY)
        } else {
            f64::INFINITY
        };
        if action_rounded != 0.0 {
            cap = 1.0 / action_rounded;
        }
        self.c.set_out(actor, "TriggerRateCap", cap);
        let doom = config.trigger_name.as_deref() == Some("Doom Blast");
        let doom_source = self.config_str("doomBlastSource");
        if doom && doom_source.as_deref() == Some("expiration") {
            let c = cached
                .as_ref()
                .ok_or("attempt to index a nil value (cachedData)")?;
            let d = c.num("Duration").unwrap_or(f64::NAN);
            let rate = 1.0 / d;
            if bd_on {
                let l = sf!("1 / %.2f ^8(source curse duration)", d);
                match esr.first_mut() {
                    Some(f) => *f = l,
                    None => esr.push(l),
                }
            }
            if rate > trig_rate.unwrap_or(f64::NAN) {
                self.c
                    .add(pdb, src("UsesCurseOverlaps", ModType::Flag, true, "Config"));
                if bd_on {
                    let t = trig_rate.unwrap_or(f64::NAN);
                    let sname = source
                        .map(|s| self.skill_name(s).to_string())
                        .unwrap_or_default();
                    let at = 1.min(esr.len());
                    esr.insert(
                        at,
                        sf!(
                            "max(%.2f, %.2f) ^8(If a curse expires instantly curse expiration is equivalent to curse replacement)",
                            rate,
                            t
                        ),
                    );
                    esr.insert(at, sf!("%.2f ^8(%s cast rate)", t, sname.as_str()));
                }
            } else {
                trig_rate = Some(rate);
            }
        } else if doom && doom_source.as_deref() == Some("hexblast") {
            let (mut hex, mut rate) = (None, None);
            let mut found = false;
            for k in 0..self.n_player {
                if self.skill_name(k) == "Hexblast" && !self.is_triggered(k) && k != m {
                    (hex, rate, uuid) = self.find_trigger_skill(k, hex, rate, Comparer::Default)?;
                    found = true;
                }
            }
            if found {
                // upstream's search reassigns the source's uuid
                cached = uuid.as_deref().and_then(|u| self.cached_now(u));
            }
            if hex.is_some() {
                let (t, r) = (trig_rate.unwrap_or(f64::NAN), rate.unwrap_or(f64::NAN));
                if bd_on {
                    let l = sf!(
                        "1 / (%.2f + %.2f) ^8(sum of triggered curse and hexblast cast time)",
                        1.0 / t,
                        1.0 / r
                    );
                    match esr.first_mut() {
                        Some(f) => *f = l,
                        None => esr.push(l),
                    }
                }
                trig_rate = Some(1.0 / (1.0 / t + 1.0 / r));
            }
        }
        let triggered_name = if self.minion_actor() != Some(actor) {
            self.skill_name(m).to_string()
        } else {
            "Triggered skill".to_string()
        };
        if bd_on && self.bd(actor).unwrap().get("TriggerRateCap").is_none() {
            let tname = config.trigger_name.clone().unwrap_or_default();
            let action_cd = over.unwrap_or(m_max(
                trigger_cd.unwrap_or(0.0) + adds.unwrap_or(0.0),
                triggered_cd.unwrap_or(0.0) + added.unwrap_or(0.0),
            ));
            let action_adj = over.unwrap_or(m_max(trigger_adj, triggered_adj));
            let mut l: Vec<String> = Vec::new();
            if let Some(o) = over {
                l.push(sf!(
                    "%.2f ^8(hard override of cooldown of %s)",
                    o,
                    triggered_name.as_str()
                ));
            } else if triggered_adj == 0.0 {
                l.push(format!(
                    "{triggered_name} has no base cooldown or cooldown override"
                ));
            } else {
                match triggered_cd {
                    Some(t) => l.push(sf!("%.2f ^8(base cooldown of triggered skill)", t)),
                    None => l.push(format!(
                        "{triggered_name} has no base cooldown or cooldown override"
                    )),
                }
                if let Some(a) = added {
                    l.push(sf!("+ %.2f ^8(flat added cooldown)", a));
                }
                l.push(sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr));
                l.push(sf!(
                    "= %.4f ^8(final cooldown of triggered skill)",
                    triggered_adj
                ));
            }
            l.push(String::new());
            if self.minion_actor() != Some(actor) {
                if trigger_adj == 0.0 {
                    l.push(sf!(
                        "Trigger rate based on %s cooldown",
                        triggered_name.as_str()
                    ));
                } else {
                    match trigger_cd {
                        Some(t) => l.push(sf!("%.2f ^8(base cooldown of %s)", t, tname.as_str())),
                        None => l.push(format!("{tname} has no base cooldown")),
                    }
                    if let Some(a) = adds {
                        l.push(sf!(
                            "+ %.2f ^8(this skill adds cast time to cooldown when triggered)",
                            a
                        ));
                    }
                    l.push(sf!("/ %.2f ^8(increased/reduced cooldown recovery)", icdr));
                    l.push(sf!("= %.4f ^8(final cooldown of trigger)", trigger_adj));
                }
            }
            l.push(String::new());
            if triggered_adj != 0.0 && trigger_adj != 0.0 {
                l.push(sf!(
                    "%.3f ^8(biggest of trigger cooldown and triggered skill cooldown)",
                    action_adj
                ));
            }
            let show_rounding = (!self.sd_truthy(m, "ignoresTickRate") && triggered_adj != 0.0)
                || (self.mods(m).triggered_by.is_some()
                    && !self.mods(m).triggered_by_ignores_tick_rate
                    && trigger_adj != 0.0);
            if show_rounding {
                l.push(sf!(
                    "%.3f ^8(adjusted for server tick rate)",
                    action_rounded
                ));
            }
            let extra = |affected: f64| -> Option<f64> {
                if over.is_some() {
                    return None;
                }
                let next = action_rounded - tick_time;
                let over_bp = action_adj - next;
                let reduced = action_cd - action_adj;
                if over_bp < affected {
                    let div = affected / (affected - over_bp - reduced);
                    Some(((div - 1.0) * 100.0).ceil() - (icdr - 1.0) * 100.0)
                } else {
                    None
                }
            };
            if let Some(e) = extra(action_cd) {
                l.push(sf!("^8(extra ICDR of %d%% would reach next breakpoint)", e));
            }
            if let Some(e) = adds.and_then(extra) {
                l.push(sf!("^8(extra ICS  of %d%% would reach next breakpoint)", e));
            }
            l.push(String::new());
            if triggered_cd.is_none() && trigger_cd.is_none() && over.is_none() {
                l.push("Assuming cast on every kill/attack/hit".into());
            } else {
                l.push("Trigger rate:".into());
                l.push(sf!("1 / %.3f", action_rounded));
                l.push(sf!("= %.2f ^8per second", cap));
            }
            self.bd(actor).unwrap().set("TriggerRateCap", Bd::of(l));
        }
        let effective = if self.skill_name(pm) == "Doom Blast"
            && doom_source.as_deref() == Some("vixen")
        {
            let vixen_gloves = self.item(PLAYER, "Gloves").is_some_and(|i| {
                i.title
                    .as_deref()
                    .is_some_and(|t| t.contains("Vixen's Entrapment"))
            });
            if !vixen_gloves {
                self.c.set_out_val(
                    PLAYER,
                    "VixenModeNoVixenGlovesWarn",
                    crate::actor::OutVal::Bool(true),
                );
            }
            self.c
                .add(pdb, src("UsesCurseOverlaps", ModType::Flag, true, "Config"));
            let vixens_cd = self
                .input
                .data
                .skills
                .skills
                .get("SupportUniqueCastCurseOnCurse")
                .and_then(|g| g.level(1.0).and_then(|l| l.field("cooldown")))
                .map(|cd| cd / icdr);
            let mut rot = vec![Sim {
                uuid: skill_uuid(self, pm),
                cd: None,
                cd_override: None,
                adds_cast_time: None,
                icdr,
                added_cooldown: 0.0,
            }];
            let r = trig_rate.unwrap_or(f64::NAN);
            let e = self
                .rotation_rate(&mut rot, r, vixens_cd.unwrap_or(0.0), 100.0, actor)
                .0;
            self.c.set_out_val(
                PLAYER,
                "VixensTooMuchCastSpeedWarn",
                crate::actor::OutVal::Bool(vixens_cd.unwrap_or(f64::NAN) > 1.0 / r),
            );
            if bd_on {
                let v = vixens_cd.unwrap_or(f64::NAN);
                esr.push(sf!(
                    "%.2f / %.2f = %.2f ^8(Vixen's trigger cooldown)",
                    v * icdr,
                    icdr,
                    v
                ));
                esr.push(sf!(
                    "%.2f ^8(Simulated trigger rate of a curse socketed in Vixen's given ^7%.2f ^8CD and ^7%.2f ^8source rate)",
                    e,
                    v,
                    r
                ));
            }
            e
        } else if let Some(r) = trig_rate.filter(|_| !global(self) && !config.ignore_source_rate) {
            r
        } else {
            self.set_flag(m, "globalTrigger", true);
            cap
        };
        self.c.set_out(actor, "EffectiveSourceRate", effective);
        if bd_on {
            if !self.sd_truthy(m, "sourceRateIsFinal") {
                esr.push(sf!("= %.2f ^8(Effective source rate)", effective));
            }
            self.bd(actor)
                .unwrap()
                .set("EffectiveSourceRate", Bd::of(esr));
        }
        let skill_name = match source {
            Some(s) => self.skill_name(s).to_string(),
            None => match self.triggered_by(m) {
                Some((ge, _)) => ge.name.clone(),
                None => self.skill_name(m).to_string(),
            },
        };
        let mut tcb: Vec<String> = Vec::new();
        let mut str_bd: Option<Bd> = None;
        let mut sim_bd: Option<Bd> = None;
        let rate = if effective != 0.0 && !self.has_flag(pm, "skipEffectiveRate") {
            let mut chance = 100.0;
            let mut config_chance = config.trigger_chance;
            if let (Some(s), Some(c)) = (source, &cached)
                && (self.ty(s, "Melee") || self.ty(s, "Attack"))
                && !config.trigger_on_use
            {
                let sname = self.skill_name(s).to_string();
                let dual = wtype(self, 0)
                    && wtype(self, 1)
                    && self.sd_truthy(s, "doubleHitsWhenDualWielding");
                let hand = |k: &str, h: &str| -> f64 {
                    match c.output.get(&Sym::new(h)) {
                        Some(crate::actor::OutVal::Table(t)) => {
                            t.get(k).and_then(|v| v.num()).unwrap_or(f64::NAN)
                        }
                        _ => f64::NAN,
                    }
                };
                let both = |k: &str| {
                    let (mh, oh) = (hand(k, "MainHand"), hand(k, "OffHand"));
                    mh * oh / 100.0 + mh * (100.0 - oh) / 100.0 + (100.0 - mh) * oh / 100.0
                };
                let hit = c.num("HitChance").unwrap_or(0.0);
                if hit != 100.0 {
                    if dual {
                        let e = both("HitChance");
                        chance *= e / 100.0;
                        tcb.push(sf!(
                            "x %.2f%% ^8(%s effective hit chance for skills that hit with both weapons)",
                            e,
                            sname.as_str()
                        ));
                    } else {
                        chance *= hit / 100.0;
                        tcb.push(sf!("x %.2f%% ^8(%s hit chance)", hit, sname.as_str()));
                    }
                }
                if self.sd_truthy(m, "triggerOnCrit") {
                    let on_crit = self.sd_num(m, "chanceToTriggerOnCrit").or(c
                        .skill_data
                        .get("chanceToTriggerOnCrit")
                        .and_then(ModValue::as_number));
                    config_chance = config_chance
                        .or(self.sd_num(m, "chanceToTriggerOnCrit"))
                        .or(on_crit);
                    let crit = c.num("CritChance").unwrap_or(0.0);
                    if crit != 100.0 {
                        if dual {
                            let e = both("CritChance");
                            chance *= e / 100.0;
                            tcb.push(sf!(
                                "x %.2f%% ^8(%s effective crit chance for skills that hit with both weapons)",
                                e,
                                sname.as_str()
                            ));
                        } else {
                            chance *= crit / 100.0;
                            tcb.push(sf!("x %.2f%% ^8(%s crit chance)", crit, sname.as_str()));
                        }
                    }
                }
            }
            if let Some(tc) = config_chance.filter(|c| *c != 100.0) {
                chance = chance * tc / 100.0;
                tcb.push(sf!("x %.2f%% ^8(chance to trigger)", tc));
            }
            let single = if self.sd_truthy(m, "ignoresTickRate") {
                if config.triggered_skill_cond.is_none() {
                    true
                } else {
                    if triggered_skills.len() == 1 {
                        // Upstream compares two different tables
                        self.sim(m)?;
                    }
                    false
                }
            } else {
                false
            };
            let stage_count_v = self.src_gem(pm).and_then(|g| g.skill_stage_count);
            let stage_count = || stage_count_v;
            if single {
                let overlaps = match config.stages_are_overlaps {
                    Some(p) if self.mods(pm).skill_part == Some(p) => stage_count(),
                    _ => None,
                }
                .or(config.overlaps);
                let r = m_min(cap, effective * overlaps.unwrap_or(1.0));
                str_bd = Some(Bd::of([match overlaps {
                    Some(n) => sf!(
                        "min(%.2f, %.2f *  %d) ^8(%d overlaps)",
                        cap,
                        effective,
                        n,
                        n
                    ),
                    None => sf!("min(%.2f, %.2f)", cap, effective),
                }]));
                r
            } else if global(self) && config.triggered_skill_cond.is_none() {
                effective
            } else {
                let mut rot = if config.triggered_skill_cond.is_some() {
                    triggered_skills.clone()
                } else {
                    vec![self.sim(m)?]
                };
                let cd = if self.sd_truthy(m, "triggeredByBrand") {
                    0.0
                } else {
                    trigger_cd.or(triggered_cd).unwrap_or(0.0)
                };
                let (mut r, sim) = self.rotation_rate(&mut rot, effective, cd, chance, actor);
                let adb = self.c.db(actor);
                let bots = self.c.flag(adb, None, &["HaveTriggerBots"])? && self.ty(m, "Spell");
                if bots {
                    r *= 2.0;
                }
                let hits = match config.stages_are_overlaps {
                    Some(p) if self.mods(pm).skill_part == Some(p) => stage_count(),
                    _ => None,
                }
                .unwrap_or(1.0);
                let r = hits * r;
                if bd_on {
                    let what = if self.sd_truthy(m, "triggeredByBrand") {
                        let sname = source
                            .map(|s| self.skill_name(s).to_string())
                            .unwrap_or_default();
                        format!("{sname} activations per second")
                    } else if trig_rate.is_none() {
                        format!("{skill_name} triggers per second")
                    } else {
                        "Effective source rate".to_string()
                    };
                    let mut l = vec![
                        sf!("%.2f ^8(%s)", effective, what.as_str()),
                        sf!(
                            "/ %.2f ^8(Estimated impact of skill rotation, cooldown alignment and trigger chance)",
                            m_max(effective / r, 1.0)
                        ),
                        sf!("= %.2f ^8per second", r),
                    ];
                    if bots {
                        l.insert(
                            2,
                            "x 2 ^8(Trigger bots effectively cause the skill to trigger twice)"
                                .to_string(),
                        );
                    }
                    if hits > 1.0 {
                        l.insert(2, sf!("x %.2f ^8(hits per triggered skill cast)", hits));
                    }
                    if chance != 100.0 {
                        l.insert(0, String::new());
                        l.insert(0, sf!("= %.2f%% ^8(Effective chance to trigger)", chance));
                        for line in &tcb {
                            l.insert(0, line.clone());
                        }
                        l.insert(0, "100% ^8(Base chance)".to_string());
                    }
                    str_bd = Some(Bd::of(l));
                    let mut t = sim_data(&sim);
                    t.push(sf!(
                        "Simulation duration: %.2f ^8(In game source skill usage duration in seconds)",
                        sim.sim_time
                    ));
                    sim_bd = Some(t);
                }
                r
            }
        } else {
            str_bd = Some(Bd::of([
                "The trigger needs to be triggered for any skill to be triggered.",
            ]));
            0.0
        };
        if let Some(bd) = self.bd(actor) {
            if let Some(b) = str_bd {
                bd.set("SkillTriggerRate", b);
            }
            if let Some(b) = sim_bd {
                bd.set("SimData", b);
            }
        }
        self.c.set_out(actor, "SkillTriggerRate", rate);
        self.sd_set(m, "triggerRate", rate);
        self.c.set_out(actor, "Speed", rate);
        self.add_trigger_inc_more(m, source.unwrap_or(m))?;
        let trigger_name = config.trigger_name.clone().unwrap_or_default();
        if let Some(s) = source.filter(|&s| s != m) {
            let su = skill_uuid(self, s);
            self.sd_set(m, "triggerSourceUUID", su.as_str());
            let sname = self.skill_name(s).to_string();
            let triggered_name = if self.minion_actor() != Some(actor) {
                self.skill_name(m).to_string()
            } else {
                "Triggered skill".to_string()
            };
            let head = config.custom_trigger_name.clone().unwrap_or_else(|| {
                let n = if trigger_name != sname {
                    trigger_name.clone()
                } else {
                    triggered_name
                };
                let tail = if self.minion_actor() == Some(actor) {
                    "'s attack Trigger: "
                } else {
                    "'s Trigger: "
                };
                format!("{n}{tail}")
            });
            self.mods_mut(m).info_message = Some(format!("{head}{sname}"));
        } else {
            let msg = match self.triggered_by(m) {
                Some((ge, _)) => ge.name.clone(),
                None => format!("{trigger_name} Trigger"),
            };
            self.mods_mut(m).info_message = Some(msg);
        }
        if self.sd_truthy(m, "triggeredByBrand") {
            let n = self.mods(m).triggered_by_brand.map_or(f64::NAN, |b| b.2);
            let s = if n == 1.0 { "" } else { "s" };
            let mm = self.mods_mut(m);
            let base = mm.info_message.clone().unwrap_or_default();
            mm.info_message = Some(format!("{base}:{} attached Brand{s}", n as i64));
        }
        self.mods_mut(m).info_trigger = config.trigger_name;
        Ok(())
    }
}

/// The rates of `calcMultiSpellRotationImpact`'s simulation
pub(crate) struct SimRates {
    sim_time: f64,
    rates: Vec<(String, f64)>,
}

/// `breakdown.SimData`: the simulated rate of each skill
fn sim_data(sim: &SimRates) -> Bd {
    let mut t = crate::breakdown::row_table(
        "",
        &[
            ("Rate", "rate"),
            ("Skill Name", "skillName"),
            ("Slot Name", "slotName"),
            ("Gem Index", "gemIndex"),
        ],
    );
    t.fields.remove("label");
    for (name, rate) in &sim.rates {
        let mut row = Bd::new();
        row.set("rate", crate::calc::round(*rate, 2.0));
        let parts: Vec<&str> = name.split('_').filter(|p| !p.is_empty()).collect();
        for (k, key) in ["skillName", "slotName", "gemIndex"].iter().enumerate() {
            if let Some(p) = parts.get(k) {
                row.set(key, *p);
            }
        }
        t.add_row(row);
    }
    t
}
