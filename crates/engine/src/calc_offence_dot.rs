//! The end of `calcs.offence` (`Modules/CalcOffence.lua`): the hands'
//! ailment stats combined, decay, burning ground, the skill's damage over
//! time, costs per second and the combined DPS.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::OutVal;
use crate::calc::{m_max, m_min, round};
use crate::calc_defence::{DMG_TYPES, is_elemental};
use crate::calc_offence::Off;
use crate::calc_offence_hit::Combine;
use crate::calc_perform::Perform;
use crate::mods::{Cfg, ModValue};

pub type Result<T> = std::result::Result<T, String>;

impl<'a> Perform<'a> {
    /// From combining the secondary effect stats to the end
    pub(crate) fn offence_secondary(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb) = (o.skill, o.list, o.actor, o.edb);
        let show_average = self.sd_truthy(i, "showAverage");
        if o.is_attack {
            use Combine::*;
            let mut stats: Vec<(&str, Combine, Option<&str>)> = vec![
                ("BleedChance", Average, None),
                ("BleedDPS", ChanceAilment, Some("BleedChance")),
                ("PoisonChance", Average, None),
                ("PoisonDPS", Chance, Some("PoisonChance")),
                ("CausticGroundDPS", Chance, Some("PoisonChance")),
                ("TotalPoisonDPS", Dps, None),
                ("PoisonDamage", Chance, Some("PoisonChance")),
            ];
            if show_average {
                stats.push(("TotalPoisonAverageDamage", Dps, None));
            } else {
                stats.push(("TotalPoisonStacks", Dps, None));
            }
            stats.push(("IgniteChance", Average, None));
            stats.push(("IgniteDPS", ChanceAilment, Some("IgniteChance")));
            if self.has_flag(i, "igniteCanStack") {
                stats.push(("IgniteDamage", Chance, Some("IgniteChance")));
                if show_average {
                    stats.push(("TotalIgniteAverageDamage", Dps, None));
                }
                stats.push(("IgniteStacksMax", Dps, None));
                stats.push(("TotalIgniteDPS", Dps, None));
            }
            for k in [
                "ChillEffectMod",
                "ChillDuration",
                "ShockChance",
                "ShockDuration",
                "ShockEffectMod",
                "FreezeChance",
                "FreezeDurationMod",
                "ScorchChance",
                "ScorchEffectMod",
                "ScorchDuration",
                "BrittleChance",
                "BrittleEffectMod",
                "BrittleDuration",
                "SapChance",
                "SapEffectMod",
                "SapDuration",
                "ImpaleChance",
                "ImpaleStoredDamage",
            ] {
                stats.push((k, Average, None));
            }
            stats.push(("ImpaleModifier", Chance, Some("ImpaleChance")));
            for (k, mode, chance) in stats {
                self.combine_stat(o, k, mode, chance);
            }
        }
        let skill_cfg = self.cfg(i);
        let effective = self.env.mode.effective;
        let g = |p: &Self, k: &str| p.c.out(actor, k);
        if self.has_flag(i, "hit")
            && let Some(decay) = self.sd_num(i, "decay")
            && o.can_deal[4]
        {
            self.set_flag(i, "decay", true);
            let dot = Cfg {
                skill_name: skill_cfg.skill_name.clone(),
                skill_part: skill_cfg.skill_part,
                skill_types: skill_cfg.skill_types.clone(),
                summon_skill_name: skill_cfg.summon_skill_name.clone(),
                slot_name: skill_cfg.slot_name,
                flags: Some(self.mf("Dot")),
                keyword_flags: Some(
                    (skill_cfg.keyword_flags.unwrap_or(0) & !self.kf("Hit")) | self.kf("ChaosDot"),
                ),
                ..Cfg::default()
            };
            let mut eff = 1.0;
            if effective {
                let resist = self.resist_for_type(o, "Chaos", Some(&dot))?;
                let names = [
                    "DamageTaken",
                    "DamageTakenOverTime",
                    "ChaosDamageTaken",
                    "ChaosDamageTakenOverTime",
                ];
                let inc = self.c.inc(edb, None, &names)?;
                let more = self.c.more(edb, None, &names)?;
                eff = (1.0 - resist / 100.0) * (1.0 + inc / 100.0) * more;
                self.c.set_out(actor, "DecayEffMult", eff);
            }
            let inc = self.c.inc(list, Some(&dot), &["Damage", "ChaosDamage"])?;
            let more = self.c.more(list, Some(&dot), &["Damage", "ChaosDamage"])?;
            let mult = match self.c.override_num(list, Some(&dot), &["DotMultiplier"])? {
                Some(v) => v,
                None => {
                    self.c.base(list, Some(&dot), &["DotMultiplier"])?
                        + self.c.base(list, Some(&dot), &["ChaosDotMultiplier"])?
                }
            };
            self.c.set_out(
                actor,
                "DecayDPS",
                decay * (1.0 + inc / 100.0) * more * (1.0 + mult / 100.0) * eff,
            );
            self.c
                .set_out(actor, "DecayDuration", 8.0 * o.debuff_duration_mult);
        }
        let db = o.db;
        let ground = self.c.base(db, None, &["DropsBurningGround"])?;
        if ground > 0.0 && o.can_deal[3] {
            let taken = Cfg {
                flags: Some(self.mf("Dot")),
                keyword_flags: Some(0),
                ..Cfg::default()
            };
            let typ = Cfg {
                keyword_flags: Some(self.kf("FireDot")),
                ..taken.clone()
            };
            let mut eff = 1.0;
            if effective {
                let resist = self.resist_for_type(o, "Fire", Some(&typ))?;
                let names = [
                    "DamageTaken",
                    "DamageTakenOverTime",
                    "FireDamageTaken",
                    "FireDamageTakenOverTime",
                    "ElementalDamageTaken",
                ];
                let inc = self.c.inc(edb, Some(&taken), &names)?;
                let more = self.c.more(edb, Some(&taken), &names)?;
                eff = (1.0 - resist / 100.0) * (1.0 + inc / 100.0) * more;
            }
            let names = ["Damage", "FireDamage", "ElementalDamage"];
            let inc = self.c.inc(db, Some(&typ), &names)?;
            let more = self.c.more(db, Some(&typ), &names)?;
            let mult = match self.c.override_num(db, Some(&typ), &["DotMultiplier"])? {
                Some(v) => v,
                None => {
                    self.c.base(db, Some(&typ), &["DotMultiplier"])?
                        + self.c.base(db, Some(&typ), &["FireDotMultiplier"])?
                }
            };
            let total = ground * (1.0 + inc / 100.0) * more * (1.0 + mult / 100.0) * eff;
            if g(self, "BurningGroundDPS").is_none_or(|v| v < total) {
                self.c.set_out(actor, "BurningGroundDPS", total);
                self.c
                    .set_out_val(actor, "BurningGroundFromIgnite", OutVal::Bool(false));
            }
        }
        self.skill_dot(o)?;
        self.cost_per_second(o)?;
        self.self_hit(o)?;
        self.combined_dps(o)
    }

    /// The skill's own damage over time
    fn skill_dot(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb) = (o.skill, o.list, o.actor, o.edb);
        let skill_cfg = self.cfg(i);
        let effective = self.env.mode.effective;
        let mut flags = self.mf("Dot") | skill_cfg.flags.unwrap_or(0);
        for (f, key) in [
            ("Area", "dotIsArea"),
            ("Projectile", "dotIsProjectile"),
            ("Spell", "dotIsSpell"),
            ("Attack", "dotIsAttack"),
            ("Hit", "dotIsHit"),
        ] {
            let bit = self.mf(f);
            if flags | bit == flags && !self.sd_truthy(i, key) {
                flags &= !bit;
            }
        }
        let dot = Cfg {
            skill_name: skill_cfg.skill_name.clone(),
            skill_part: skill_cfg.skill_part,
            skill_types: skill_cfg.skill_types.clone(),
            summon_skill_name: skill_cfg.summon_skill_name.clone(),
            slot_name: skill_cfg.slot_name,
            flags: Some(flags),
            keyword_flags: Some(skill_cfg.keyword_flags.unwrap_or(0) & !self.kf("Hit")),
            ..Cfg::default()
        };
        let mut taken = dot.clone();
        if self.sd_truthy(i, "dotIsSpell") {
            taken.flags = Some(flags & !self.mf("Spell"));
        }
        self.c.set_out(actor, "TotalDotInstance", 0.0);
        self.run_skill_func(o, "preDotFunc")?;
        let cap = self.misc_data("DotDpsCap");
        let g = |p: &Self, k: &str| p.c.out(actor, k);
        for (ti, t) in DMG_TYPES.iter().enumerate() {
            let t = *t;
            let typ = Cfg {
                keyword_flags: Some(dot.keyword_flags.unwrap_or(0) | self.kf(&format!("{t}Dot"))),
                ..dot.clone()
            };
            let base = if o.can_deal[ti] {
                self.sd_num(i, &format!("{t}Dot")).unwrap_or(0.0)
            } else {
                0.0
            };
            let key = format!("{t}Dot");
            if !(base > 0.0 || g(self, &key).unwrap_or(0.0) > 0.0) {
                continue;
            }
            self.set_flag(i, "dot", true);
            let mut eff = 1.0;
            if effective {
                let (a, b) = (format!("{t}DamageTaken"), format!("{t}DamageTakenOverTime"));
                let names = ["DamageTaken", "DamageTakenOverTime", a.as_str(), b.as_str()];
                let inc = self.c.inc(edb, Some(&taken), &names)?
                    + if is_elemental(t) {
                        self.c.inc(edb, Some(&taken), &["ElementalDamageTaken"])?
                    } else {
                        0.0
                    };
                let more = self.c.more(edb, Some(&taken), &names)?
                    * if is_elemental(t) {
                        self.c.more(edb, Some(&taken), &["ElementalDamageTaken"])?
                    } else {
                        1.0
                    };
                let resist = if t == "Physical" {
                    m_max(
                        0.0,
                        m_min(
                            self.c.base(edb, None, &["PhysicalDamageReduction"])?,
                            self.misc_data("EnemyPhysicalDamageReductionCap"),
                        ),
                    )
                } else {
                    self.resist_for_type(o, t, Some(&typ))?
                };
                eff = (1.0 - resist / 100.0) * (1.0 + inc / 100.0) * more;
                self.c.set_out(actor, &format!("{t}DotEffMult"), eff);
            }
            let td = format!("{t}Damage");
            let mut names = vec!["Damage", td.as_str()];
            if is_elemental(t) {
                names.push("ElementalDamage");
            }
            let mut inc = self.c.inc(list, Some(&typ), &names)?;
            if self.c.flag(list, None, &["dotIsHeraldOfAsh"])? {
                inc = m_max(inc - self.c.inc(list, Some(&skill_cfg), &names)?, 0.0);
            }
            let more = self.c.more(list, Some(&typ), &names)?;
            let mult = match self.c.override_num(list, Some(&typ), &["DotMultiplier"])? {
                Some(v) => v,
                None => {
                    self.c.base(list, Some(&typ), &["DotMultiplier"])?
                        + self
                            .c
                            .base(list, Some(&typ), &[&format!("{t}DotMultiplier")])?
                }
            };
            let aura = if self.has_type(i, "Aura")
                && !self.has_type(i, "RemoteMined")
                && !self.has_type(i, "Banner")
            {
                self.c.calc_mod(list, Some(&typ), &["AuraEffect"])?
            } else {
                1.0
            };
            let total = base * (1.0 + inc / 100.0) * more * (1.0 + mult / 100.0) * aura * eff;
            let instance = g(self, "TotalDotInstance").unwrap_or(f64::NAN);
            match g(self, &key) {
                Some(v) if v != 0.0 => {
                    self.c
                        .set_out(actor, "TotalDotInstance", m_min(instance + total + v, cap));
                }
                _ => {
                    self.c.set_out(actor, &key, total);
                    self.c
                        .set_out(actor, "TotalDotInstance", m_min(instance + total, cap));
                }
            }
        }
        let instance = g(self, "TotalDotInstance").unwrap_or(f64::NAN);
        if self.c.flag(list, None, &["DotCanStack"])? {
            self.set_flag(i, "DotCanStack", true);
            let kw = dot.keyword_flags.unwrap_or(0);
            let speed = if kw & self.kf("Mine") != 0 {
                g(self, "MineLayingSpeed")
            } else if kw & self.kf("Trap") != 0 {
                g(self, "TrapThrowingSpeed")
            } else {
                g(self, "Speed")
            }
            .unwrap_or(f64::NAN);
            let total = m_min(
                instance
                    * speed
                    * g(self, "Duration").unwrap_or(f64::NAN)
                    * self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN)
                    * o.quantity_multiplier,
                cap,
            );
            self.c.set_out(actor, "TotalDot", total);
            self.c.set_out(actor, "TotalDotCalcSection", total);
        } else if let Some((flag, out, from)) = [
            (
                "dotIsBurningGround",
                "BurningGroundDPS",
                Some("BurningGroundFromIgnite"),
            ),
            (
                "dotIsCausticGround",
                "CausticGroundDPS",
                Some("CausticGroundFromPoison"),
            ),
            ("dotIsCorruptingBlood", "CorruptingBloodDPS", None),
        ]
        .into_iter()
        .map(|x| -> Result<_> { Ok((self.c.flag(list, None, &[x.0])?, x)) })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .find(|(on, _)| *on)
        .map(|(_, x)| x)
        {
            let _ = flag;
            self.c.set_out(actor, "TotalDot", 0.0);
            self.c.set_out(actor, "TotalDotCalcSection", instance);
            let cur = g(self, out);
            if cur.is_none_or(|v| v < instance) {
                self.c
                    .set_out(actor, out, m_max(cur.unwrap_or(0.0), instance));
                if let Some(f) = from {
                    self.c.set_out_val(actor, f, OutVal::Bool(false));
                }
            }
        } else {
            if self.c.flag(list, None, &["DotCanStackAsTotems"])? && self.has_flag(i, "totem") {
                self.set_flag(i, "DotCanStack", true);
            }
            let brands = if self.has_type(i, "Brand")
                && !self.sd_truthy(i, "countsAttachedBrandsInDamage")
            {
                g(self, "AttachedBrandCount")
            } else {
                Some(1.0)
            };
            let total = match brands {
                Some(n) if n > 1.0 => m_min(instance * n, cap),
                _ => instance,
            };
            self.c.set_out(actor, "TotalDot", total);
            self.c.set_out(actor, "TotalDotCalcSection", total);
        }
        Ok(())
    }

    /// Costs per second of skills that have none (link skills)
    fn cost_per_second(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let eb = self
            .c
            .flag(self.pdb(), None, &["EnergyShieldProtectsMana"])?;
        let g = |p: &Self, k: &str| p.c.out(actor, k);
        let has = |p: &Self, k: &str| p.c.out_val(actor, k).is_some_and(|v| v.truthy());
        for resource in [
            "Mana",
            "Life",
            "ES",
            "Soul",
            "Rage",
            "ManaPercent",
            "LifePercent",
        ] {
            let speed = g(self, "Speed").unwrap_or(f64::NAN);
            let cooldown = g(self, "Cooldown");
            if !(has(self, &format!("{resource}HasCost"))
                && g(self, &format!("{resource}Cost")).unwrap_or(f64::NAN) > 0.0
                && !(has(self, &format!("{resource}PerSecondHasCost")) && !eb)
                && (speed > 0.0 || cooldown.is_some()))
            {
                continue;
            }
            let used = if eb && resource == "Mana" {
                "ES"
            } else {
                resource
            };
            let repeats = g(self, "Repeats").unwrap_or(1.0);
            let use_speed = if self.has_flag(i, "trap") || self.has_flag(i, "mine") {
                let pre = g(self, "TrapThrowingSpeed").or_else(|| g(self, "MineLayingSpeed"));
                match g(self, "TrapCooldown").or(cooldown) {
                    Some(cd) if cd > 0.0 => 1.0 / cd,
                    _ => pre.unwrap_or(f64::NAN),
                }
            } else if self.has_flag(i, "totem") {
                let place = g(self, "TotemPlacementSpeed").unwrap_or(f64::NAN);
                (match cooldown {
                    Some(cd) if cd > 0.0 => {
                        if place > 0.0 {
                            place
                        } else {
                            1.0 / cd
                        }
                    }
                    _ => place,
                }) / repeats
            } else if self.c.flag(list, None, &["HasSeals"])?
                && self.c.flag(list, None, &["UseMaxUnleash"])?
                && let Some(h) = self.sd_num(self.main(), "hitTimeOverride")
            {
                h / repeats
            } else {
                (match cooldown {
                    Some(cd) if cd > 0.0 => {
                        if speed > 0.0 {
                            speed
                        } else {
                            1.0 / cd
                        }
                    }
                    _ => speed,
                }) / repeats
            };
            self.c.set_out_val(
                actor,
                &format!("{used}PerSecondHasCost"),
                OutVal::Bool(true),
            );
            let v = g(self, &format!("{used}PerSecondCost")).unwrap_or(0.0)
                + g(self, &format!("{resource}Cost")).unwrap_or(f64::NAN) * use_speed;
            self.c.set_out(actor, &format!("{used}PerSecondCost"), v);
        }
        Ok(())
    }

    /// Damage the skill deals to the player (`SelfHitDamage`)
    fn self_hit(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let name = self.skill_name(i).to_string();
        let records = |p: &Self, n: &str| -> Result<Vec<crate::mods::Record>> {
            Ok(p.c
                .list(list, None, &[n])?
                .into_iter()
                .filter_map(|v| match v {
                    ModValue::Record(r) => Some(r),
                    _ => None,
                })
                .collect())
        };
        let num = |r: &crate::mods::Record, k: &str| -> Result<f64> {
            r.get(k)
                .and_then(ModValue::as_number)
                .ok_or_else(|| format!("attempt to perform arithmetic on a nil value ({k})"))
        };
        let typ = |r: &crate::mods::Record| -> Option<String> {
            match r.get("damageType") {
                Some(ModValue::Str(t)) => Some(title_case(t)),
                _ => None,
            }
        };
        let out = |p: &Self, k: &str| p.c.out(actor, k);
        // Each source's damage type and base damage, in a fixed order
        let mut sources: Vec<(String, f64)> = Vec::new();
        // Heartbound Loop, Storm Secret, Enmity's Embrace: the rings' sum
        let summed = |p: &Self, n: &str| -> Result<Option<(String, f64)>> {
            let mut t = None;
            let mut v: Option<f64> = None;
            for r in records(p, n)? {
                v = Some(v.unwrap_or(0.0) + num(&r, "baseDamage")?);
                t = typ(&r);
            }
            Ok(t.zip(v))
        };
        if name == "Summon Skeletons"
            && let Some((t, v)) = summed(self, "HeartboundLoopSelfDamage")?
        {
            let taken = self.dmg_taken_conversion(o, &t, v)?;
            let per_cast = out(self, "SummonedMinionsPerCast").unwrap_or(f64::NAN);
            sources.push((t, taken * per_cast));
        }
        if name == "Herald of Thunder"
            && let Some((t, v)) = summed(self, "StormSecretSelfDamage")?
        {
            let taken = self.dmg_taken_conversion(o, &t, v)?;
            sources.push((t, taken));
        }
        if let Some(r) = records(self, "EyeOfInnocenceSelfDamage")?
            .into_iter()
            .next()
            && self.has_flag(i, "ignite")
            && let Some(t) = typ(&r)
        {
            let v = num(&r, "baseDamage")?;
            let taken = self.dmg_taken_conversion(o, &t, v)?;
            sources.push((t, taken));
        }
        if let Some(r) = records(self, "EchoesOfCreationSelfDamage")?
            .into_iter()
            .next()
            && let Some(t) = typ(&r)
        {
            let mult = num(&r, "dmgMult")?;
            let mut count = out(self, "GlobalWarcryUptimeRatio")
                .ok_or("attempt to perform arithmetic on a nil value (GlobalWarcryUptimeRatio)")?
                / 100.0;
            if self.c.flag(list, None, &["Condition:WarcryMaxHit"])? {
                count = self
                    .c
                    .base(self.pdb(), None, &["Multiplier:ExertingWarcryCount"])?;
            }
            let base = out(self, "Life").unwrap_or(0.0) * mult / 100.0 * count;
            let taken = self.dmg_taken_conversion(o, &t, base)?;
            sources.push((t, taken));
        }
        let has_cost = matches!(
            self.c.out_val(actor, "ManaHasCost"),
            Some(OutVal::Bool(true))
        );
        if has_cost
            && let Some(r) = records(self, "ScoldsBridleSelfDamage")?.into_iter().next()
            && let Some(t) = typ(&r)
        {
            let mult = num(&r, "dmgMult")?;
            let base = out(self, "ManaCost").unwrap_or(0.0) * mult / 100.0;
            let taken = self.dmg_taken_conversion(o, &t, base)?;
            sources.push((t, taken));
        }
        if self.c.flag(self.lists[i].base, None, &["HasTrauma"])? {
            let stacks = m_max(self.c.base(list, None, &["Multiplier:TraumaStacks"])?, 1.0);
            let per = self.c.base(list, None, &["TraumaSelfDamageTakenLife"])?;
            let taken = self.dmg_taken_conversion(o, "Physical", per * stacks)?;
            sources.push(("Physical".into(), taken));
        }
        if let Some((t, v)) = summed(self, "EnmitysEmbraceSelfDamage")? {
            let taken = self.dmg_taken_conversion(o, &t, v)?;
            sources.push((t, taken));
        }
        for (_, taken) in sources {
            let v = out(self, "SelfHitDamage").unwrap_or(0.0) + taken;
            self.c.set_out(actor, "SelfHitDamage", v);
        }
        Ok(())
    }

    /// `calcs.applyDmgTakenConversion(activeSkill, output, breakdown,
    /// sourceType, baseDmg)`: the damage the actor takes from its own hit
    fn dmg_taken_conversion(&self, o: &Off, source: &str, base: f64) -> Result<f64> {
        let (list, actor) = (o.list, o.actor);
        let out = |k: &str| self.c.out(actor, k);
        let elemental = |t: &str| matches!(t, "Lightning" | "Cold" | "Fire");
        let types = ["Physical", "Lightning", "Cold", "Fire", "Chaos"];
        let mut shift = Vec::new();
        let mut total_as = 0.0;
        for t in types {
            let mut names = vec![
                format!("{source}DamageTakenAs{t}"),
                format!("{source}DamageFromHitsTakenAs{t}"),
            ];
            if elemental(source) {
                names.push(format!("ElementalDamageTakenAs{t}"));
                names.push(format!("ElementalDamageFromHitsTakenAs{t}"));
            }
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let v = self.c.base(list, None, &names)?;
            total_as += v;
            shift.push(v);
        }
        let mut total = 0.0;
        for (k, t) in types.iter().enumerate() {
            let taken_as = if *t == source {
                m_max(1.0 - total_as / 100.0, 0.0)
            } else {
                shift[k] / 100.0
            };
            if taken_as == 0.0 {
                continue;
            }
            let damage = base * taken_as;
            let names = [
                "DamageTaken".to_string(),
                format!("{t}DamageTaken"),
                "DamageTakenWhenHit".to_string(),
                format!("{t}DamageTakenWhenHit"),
            ];
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let mut inc = self.c.inc(list, None, &names)?;
            let mut more = self.c.more(list, None, &names)?;
            if elemental(t) {
                inc += self.c.inc(
                    list,
                    None,
                    &["ElementalDamageTaken", "ElementalDamageTakenWhenHit"],
                )?;
                more *= self.c.more(
                    list,
                    None,
                    &["ElementalDamageTaken", "ElementalDamageTakenWhenHit"],
                )?;
            }
            let taken_mods = m_max((1.0 + inc / 100.0) * more, 0.0);
            let reduction =
                if self
                    .c
                    .flag(list, None, &[&format!("SelfIgnoreBase{t}DamageReduction")])?
                {
                    0.0
                } else {
                    out(&format!("Base{t}DamageReductionWhenHit"))
                        .or_else(|| out(&format!("Base{t}DamageReduction")))
                        .ok_or("attempt to perform arithmetic on a nil value (reduction)")?
                };
            let resist = if self
                .c
                .flag(list, None, &[&format!("SelfIgnore{t}Resistance")])?
            {
                0.0
            } else {
                out(&format!("{t}ResistWhenHit"))
                    .or_else(|| out(&format!("{t}Resist")))
                    .ok_or("attempt to perform arithmetic on a nil value (resist)")?
            };
            let res_mult = 1.0 - resist / 100.0;
            let applies = if self.c.flag(
                list,
                None,
                &[&format!("ArmourDoesNotApplyTo{t}DamageTaken")],
            )? {
                0.0
            } else {
                self.c
                    .base(list, None, &[&format!("ArmourAppliesTo{t}DamageTaken")])?
            };
            let applies = m_min(applies, 100.0);
            let ward =
                *t == "Physical" && self.c.flag(list, None, &["PhysicalReductionBasedOnWard"])?;
            let reduct_max = out("DamageReductionMax").unwrap_or(f64::NAN);
            let mut armour_reduct = 0.0;
            if applies > 0.0 || ward {
                let mut eff = (out("Armour").unwrap_or(f64::NAN) * applies / 100.0)
                    * (1.0 + out("ArmourDefense").unwrap_or(f64::NAN));
                if ward {
                    let m = self
                        .c
                        .override_num(list, None, &["PhysicalReductionBasedOnWardPercent"])?
                        .ok_or("attempt to perform arithmetic on a nil value (ward percent)")?
                        / 100.0;
                    eff = out("Ward").unwrap_or(f64::NAN) * m;
                }
                let eff_damage = damage * res_mult;
                armour_reduct = if eff != 0.0 && damage * res_mult != 0.0 {
                    round(eff / (eff + eff_damage * 5.0) * 100.0, 0.0)
                } else {
                    0.0
                };
                armour_reduct = m_min(reduct_max, armour_reduct);
            }
            let reduct_mult = (1.0
                - m_max(m_min(reduct_max, armour_reduct + reduction), 0.0) / 100.0)
                * taken_mods;
            total += damage * (res_mult * reduct_mult);
        }
        Ok(total)
    }

    /// The combined DPS estimate, with damage over time
    fn combined_dps(&mut self, o: &mut Off) -> Result<()> {
        let (i, actor) = (o.skill, o.actor);
        let g = |p: &Self, k: &str| p.c.out(actor, k);
        let n = |p: &Self, k: &str| g(p, k).unwrap_or(f64::NAN);
        let show_average = self.sd_truthy(i, "showAverage");
        let base = n(
            self,
            if show_average {
                "AverageDamage"
            } else {
                "TotalDPS"
            },
        );
        let mut combined = base;
        let mut avg = base;
        if self.has_flag(i, "dot") {
            self.c.set_out(
                actor,
                "WithDotDPS",
                base + g(self, "TotalDot").unwrap_or(0.0),
            );
        }
        let cap = self.misc_data("DotDpsCap");
        if o.quantity_multiplier > 1.0
            && let Some(p) = g(self, "TotalPoisonDPS")
        {
            self.c.set_out(
                actor,
                "TotalPoisonDPS",
                m_min(p * o.quantity_multiplier, cap),
            );
        }
        if show_average {
            avg += g(self, "TotalPoisonAverageDamage").unwrap_or(0.0);
            self.c.set_out(
                actor,
                "WithPoisonDPS",
                base + g(self, "TotalPoisonAverageDamage").unwrap_or(0.0),
            );
        } else {
            self.c.set_out(
                actor,
                "WithPoisonDPS",
                base + g(self, "TotalPoisonDPS").unwrap_or(0.0),
            );
        }
        if self.has_flag(i, "ignite") {
            if self.has_flag(i, "igniteCanStack") {
                if show_average {
                    avg = combined + n(self, "IgniteDamage");
                } else {
                    self.c
                        .set_out(actor, "WithIgniteDPS", base + n(self, "TotalIgniteDPS"));
                }
            } else if show_average {
                self.c
                    .set_out(actor, "WithIgniteDPS", base + n(self, "IgniteDamage"));
                avg += n(self, "IgniteDamage");
            } else {
                self.c
                    .set_out(actor, "WithIgniteDPS", base + n(self, "IgniteDPS"));
            }
        } else {
            self.c.set_out(actor, "WithIgniteDPS", base);
        }
        if self.has_flag(i, "monsterExplode") {
            self.c.set_out(
                actor,
                "CombinedAvgToMonsterLife",
                avg / o.monster_life * 100.0,
            );
        }
        if self.has_flag(i, "bleed") {
            if show_average {
                self.c
                    .set_out(actor, "WithBleedDPS", base + n(self, "BleedDamage"));
                avg += n(self, "BleedDamage");
            } else {
                self.c
                    .set_out(actor, "WithBleedDPS", base + n(self, "BleedDPS"));
            }
        } else {
            self.c.set_out(actor, "WithBleedDPS", base);
        }
        let dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
        if self.has_flag(i, "impale") {
            let hand = |_: &Self, h: usize, k: &str| crate::calc_offence::get_num(&o.hands[h], k);
            let mut impale = if self.has_flag(i, "attack")
                && self.sd_truthy(i, "doubleHitsWhenDualWielding")
                && self.has_flag(i, "bothWeaponAttack")
            {
                (0..2)
                    .map(|h| {
                        hand(self, h, "impaleStoredHitAvg").unwrap_or(f64::NAN)
                            * (hand(self, h, "ImpaleModifier").unwrap_or(1.0) - 1.0)
                            * hand(self, h, "HitChance").unwrap_or(f64::NAN)
                            / 100.0
                            * dps_mult
                    })
                    .sum()
            } else {
                n(self, "impaleStoredHitAvg")
                    * (g(self, "ImpaleModifier").unwrap_or(1.0) - 1.0)
                    * n(self, "HitChance")
                    / 100.0
                    * dps_mult
            };
            if n(self, "ImpaleDuration") <= 0.0 {
                impale = 0.0;
            }
            if show_average {
                self.c
                    .set_out(actor, "WithImpaleDPS", n(self, "AverageDamage") + impale);
                avg += impale;
            } else {
                self.set_flag(i, "notAverage", true);
                impale *= g(self, "HitSpeed")
                    .or_else(|| g(self, "Speed"))
                    .unwrap_or(f64::NAN);
                self.c
                    .set_out(actor, "WithImpaleDPS", n(self, "TotalDPS") + impale);
            }
            if o.quantity_multiplier > 1.0 {
                impale *= o.quantity_multiplier;
            }
            self.c.set_out(actor, "ImpaleDPS", impale);
            combined += impale;
        }
        let total_dot = g(self, "TotalDot").unwrap_or(0.0)
            + g(self, "TotalPoisonDPS").unwrap_or(0.0)
            + m_max(
                g(self, "CausticGroundDPS").unwrap_or(0.0),
                g(self, "MirageCausticGroundDPS").unwrap_or(0.0),
            )
            + g(self, "TotalIgniteDPS")
                .or_else(|| g(self, "IgniteDPS"))
                .unwrap_or(0.0)
            + m_max(
                g(self, "BurningGroundDPS").unwrap_or(0.0),
                g(self, "MirageBurningGroundDPS").unwrap_or(0.0),
            )
            + g(self, "BleedDPS").unwrap_or(0.0)
            + g(self, "CorruptingBloodDPS").unwrap_or(0.0)
            + g(self, "DecayDPS").unwrap_or(0.0);
        let capped = m_min(total_dot, cap);
        self.c.set_out(actor, "TotalDotDPS", capped);
        if capped != total_dot {
            self.c
                .set_out_val(actor, "showTotalDotDPS", OutVal::Bool(true));
        }
        if !show_average {
            combined += capped;
        }
        let best_cull = m_max(1.0, n(self, "CullMultiplier"));
        self.c
            .set_out(actor, "CullingDPS", combined * (best_cull - 1.0));
        let reservation = n(self, "ReservationDpsMultiplier");
        self.c
            .set_out(actor, "ReservationDPS", combined * (reservation - 1.0));
        self.c
            .set_out(actor, "CombinedDPS", combined * best_cull * reservation);
        self.c.set_out(actor, "CombinedAvg", avg);
        Ok(())
    }
}

/// `string.gsub(" " .. s, "%W%l", string.upper):sub(2)`: each word's
/// first lowercase letter in upper case
fn title_case(s: &str) -> String {
    let mut out = String::new();
    let mut prev_word = false;
    for c in s.chars() {
        if !prev_word && c.is_ascii_lowercase() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
        prev_word = c.is_ascii_alphanumeric();
    }
    out
}
