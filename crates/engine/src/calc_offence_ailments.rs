//! The ailment passes of `calcs.offence` (`Modules/CalcOffence.lua`):
//! chances to inflict ailments, bleed, poison and ignite damage,
//! non-damaging ailments, knockback, stun and impale.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::OutVal;
use crate::calc::{m_max, m_min, number, round};
use crate::calc_defence::{DMG_TYPES, armour_reduction_f};
use crate::calc_offence::Off;
use crate::calc_perform::{Perform, new_mod};
use crate::mods::{Cfg, Mod, ModType, Sym, Tag};

pub type Result<T> = std::result::Result<T, String>;

use ModType::{Base, Flag as FlagT, Override};

impl<'a> Perform<'a> {
    /// `calcAilmentSourceDamage(activeSkill, output, cfg, breakdown,
    /// damageType, typeFlags)`: the skill's own conversion table
    fn ailment_source_damage(&self, o: &Off, cfg: &Cfg, t: &str, flags: u32) -> Result<(f64, f64)> {
        let conv = o.conv[0].as_ref().expect("conversion tables");
        let (min, max) = self.calc_damage(o, cfg, conv, t, flags)?;
        let m = conv[crate::calc_offence::dmg_index(t)].mult;
        Ok((min * m, max * m))
    }

    /// `calcAilmentDamage(type, sourceCritChance, sourceHitDmg,
    /// sourceCritDmg)`: sets `<type>Chance`
    fn ailment_damage(
        &mut self,
        o: &mut Off,
        t: &str,
        crit_chance: f64,
        hit: f64,
        crit: f64,
    ) -> f64 {
        let on_hit = self.po(o, &format!("{t}ChanceOnHit")).unwrap_or(f64::NAN);
        let on_crit = self.po(o, &format!("{t}ChanceOnCrit")).unwrap_or(f64::NAN);
        let from_hit = on_hit * (1.0 - crit_chance / 100.0);
        let from_crit = on_crit * crit_chance / 100.0;
        self.set_po(o, &format!("{t}Chance"), from_hit + from_crit);
        hit * from_hit / (from_hit + from_crit) + crit * from_crit / (from_hit + from_crit)
    }

    /// `calcAverageSourceDamage(ailment)`: hit and crit damage of the types
    /// that can inflict it
    fn average_source_damage(&self, o: &Off, cfg: &Cfg, ailment: &str) -> Result<(f64, f64)> {
        let assoc = self.data().non_damaging_ailment[ailment]
            .associated_type
            .clone();
        let (mut hit, mut crit) = (0.0, 0.0);
        for (ti, t) in DMG_TYPES.iter().enumerate() {
            if !o.can_deal[ti] {
                continue;
            }
            let ok = if **t == *assoc {
                !self
                    .c
                    .flag(o.list, Some(cfg), &[&format!("{t}Cannot{ailment}")])?
            } else {
                self.c
                    .flag(o.list, Some(cfg), &[&format!("{t}Can{ailment}")])?
            };
            if ok {
                hit += self.po(o, &format!("{t}HitAverage")).unwrap_or(f64::NAN);
                crit += self.po(o, &format!("{t}CritAverage")).unwrap_or(f64::NAN);
            }
        }
        Ok((hit, crit))
    }

    /// The cfg of a damaging ailment (`activeSkill.bleedCfg`, ...)
    fn ailment_cfg(&self, o: &Off, cfg: &Cfg, kw: &[&str]) -> Cfg {
        let skill_cfg = self.cfg(o.skill);
        let flags = cfg.flags.unwrap_or(0);
        let mut f = self.mf("Dot") | self.mf("Ailment") | (flags & self.mf("WeaponMask"));
        if flags & self.mf("Melee") != 0 {
            f |= self.mf("MeleeHit");
        }
        f |= flags & self.mf("Weapon1H");
        let mut k = cfg.keyword_flags.unwrap_or(0) & !self.kf("Hit");
        for n in kw {
            k |= self.kf(n);
        }
        let mut conds: std::collections::HashSet<Sym> =
            skill_cfg.skill_cond.clone().unwrap_or_default();
        conds.extend(cfg.skill_cond.iter().flatten().copied());
        conds.insert(Sym::new("CriticalStrike"));
        Cfg {
            skill_name: skill_cfg.skill_name.clone(),
            skill_part: skill_cfg.skill_part,
            skill_types: skill_cfg.skill_types.clone(),
            summon_skill_name: skill_cfg.summon_skill_name.clone(),
            slot_name: skill_cfg.slot_name,
            flags: Some(f),
            keyword_flags: Some(k),
            skill_cond: Some(conds),
            skill_dist: skill_cfg.skill_dist,
            ..Cfg::default()
        }
    }

    /// `1 + (Override(DotMultiplier) or Sum(DotMultiplier, <type>DotMultiplier)) / 100`
    fn dot_multi(&self, o: &Off, cfg: &Cfg, t: &str) -> Result<f64> {
        let c = Some(cfg);
        Ok(1.0
            + match self.c.override_num(o.list, c, &["DotMultiplier"])? {
                Some(v) => v,
                None => {
                    self.c.base(o.list, c, &["DotMultiplier"])?
                        + self.c.base(o.list, c, &[&format!("{t}DotMultiplier")])?
                }
            } / 100.0)
    }

    /// The dot multiplier of crit ailments (sub pass 2)
    fn crit_dot_multi(&self, o: &Off, cfg: &mut Cfg, t: &str) -> Result<f64> {
        if self.c.flag(o.db, None, &["AilmentsAreNeverFromCrit"])? {
            set_cond(cfg, "CriticalStrike", false);
            let v = self.dot_multi(o, cfg, t)?;
            set_cond(cfg, "CriticalStrike", true);
            Ok(v)
        } else {
            self.dot_multi(o, cfg, t)
        }
    }

    /// The enemy's resistance and damage taken against a damage over time
    fn dot_eff_mult(&self, o: &Off, cfg: &Cfg, t: &str, elemental: bool) -> Result<f64> {
        let resist = self.resist_for_type(o, t, Some(cfg))?;
        let (a, b) = (format!("{t}DamageTaken"), format!("{t}DamageTakenOverTime"));
        let mut names = vec!["DamageTaken", "DamageTakenOverTime", a.as_str(), b.as_str()];
        if elemental {
            names.push("ElementalDamageTaken");
        }
        let inc = self.c.inc(o.edb, Some(cfg), &names)?;
        let more = self.c.more(o.edb, Some(cfg), &names)?;
        Ok((1.0 - resist / 100.0) * (1.0 + inc / 100.0) * more)
    }

    /// The ailment passes
    pub(crate) fn offence_ailments(&mut self, o: &mut Off) -> Result<()> {
        let (i, list) = (o.skill, o.list);
        let skill_cfg = self.cfg(i);
        let sc = Some(&skill_cfg);
        let ailments = self.data().ailment_types.clone();
        for a in &ailments {
            self.set_flag(i, &a.to_lowercase(), false);
        }
        let v = self.c.flag(list, sc, &["IgniteCanStack"])?;
        self.set_flag(i, "igniteCanStack", v);
        let v = self.c.flag(list, sc, &["IgniteToChaos"])?;
        self.set_flag(i, "igniteToChaos", v);
        self.set_flag(i, "impale", false);
        for p in 0..o.passes.len() {
            o.pass = p;
            self.ailment_pass(o)?;
        }
        self.offence_secondary(o)
    }

    fn ailment_pass(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, edb, db) = (o.skill, o.list, o.edb, o.db);
        let effective = self.env.mode.effective;
        let label = o.passes[o.pass].label;
        let ailment_flag = self.mf("Ailment");
        // Perfect Agony
        {
            let cfg = self.pcfg(o);
            let hand = match label {
                "Off Hand" => Some(Tag::condition("OffHandAttack")),
                "Main Hand" => Some(Tag::condition("MainHandAttack")),
                _ => None,
            };
            if self.c.base(list, None, &["CritMultiplierAppliesToDegen"])? > 0.0 {
                for (_, m) in self
                    .c
                    .tabulate(list, Some(Base), Some(&cfg), &["CritMultiplier"])?
                {
                    if m.source.as_deref() != Some("Base") {
                        let mut tags = vec![Tag::condition("CriticalStrike")];
                        tags.extend(hand.clone());
                        tags.extend(m.tags.iter().cloned());
                        self.c.add(
                            list,
                            new_mod(
                                "DotMultiplier",
                                Base,
                                (number(&m.value)? / 2.0).floor(),
                                m.source.as_deref(),
                                ailment_flag,
                                0,
                                tags,
                            ),
                        );
                    }
                }
            }
            if self
                .c
                .flag(list, None, &["DotMultiplierIsCritMultiplier"])?
            {
                let over = self
                    .c
                    .override_num(list, Some(&cfg), &["CritMultiplier"])?
                    .map(|v| v - 100.0);
                let v = match over {
                    Some(v) => v,
                    None => self.c.base(list, Some(&cfg), &["CritMultiplier"])?,
                } + self.c.base(edb, Some(&cfg), &["SelfCritMultiplier"])?;
                let mut tags = vec![Tag::condition("CriticalStrike")];
                tags.extend(hand);
                self.c.add(
                    list,
                    new_mod(
                        "DotMultiplier",
                        Override,
                        v,
                        Some("Perfect Agony"),
                        ailment_flag,
                        0,
                        tags,
                    ),
                );
            }
        }
        let attack = self.has_flag(i, "attack");
        let hit = self.has_flag(i, "hit");
        // Chances on crit
        self.set_pass_cond(o, "CriticalStrike", true);
        let cfg = self.pcfg(o);
        let c = Some(&cfg);
        let v = if !attack || self.c.flag(list, c, &["CannotBleed"])? {
            0.0
        } else {
            m_min(
                100.0,
                self.c.base(list, c, &["BleedChance"])?
                    + self.c.base(edb, None, &["SelfBleedChance"])?,
            )
        };
        self.set_po(o, "BleedChanceOnCrit", v);
        let v = if !hit || self.c.flag(list, c, &["CannotPoison"])? {
            0.0
        } else {
            m_min(
                100.0,
                self.c.base(list, c, &["PoisonChance"])?
                    + self.c.base(edb, None, &["SelfPoisonChance"])?,
            )
        };
        self.set_po(o, "PoisonChanceOnCrit", v);
        let v = if !hit {
            0.0
        } else if effective {
            m_min(100.0, self.c.base(list, c, &["ImpaleChance"])?)
        } else {
            0.0
        };
        self.set_po(o, "ImpaleChanceOnCrit", v);
        let v = if !hit || self.c.flag(list, c, &["CannotKnockback"])? {
            0.0
        } else {
            self.c.base(list, c, &["EnemyKnockbackChance"])?
        };
        self.set_po(o, "KnockbackChanceOnCrit", v);
        // Chances on hit
        self.set_pass_cond(o, "CriticalStrike", false);
        let cfg = self.pcfg(o);
        let c = Some(&cfg);
        let v = if !attack || self.c.flag(list, c, &["CannotBleed"])? {
            0.0
        } else {
            m_min(
                100.0,
                self.c.base(list, c, &["BleedChance"])?
                    + self.c.base(edb, None, &["SelfBleedChance"])?,
            )
        };
        self.set_po(o, "BleedChanceOnHit", v);
        if !hit || self.c.flag(list, c, &["CannotPoison"])? {
            self.set_po(o, "PoisonChanceOnHit", 0.0);
            self.set_po(o, "ChaosPoisonChance", 0.0);
        } else {
            let v = m_min(
                100.0,
                self.c.base(list, c, &["PoisonChance"])?
                    + self.c.base(edb, None, &["SelfPoisonChance"])?,
            );
            self.set_po(o, "PoisonChanceOnHit", v);
            let v = m_min(100.0, self.c.base(list, c, &["ChaosPoisonChance"])?);
            self.set_po(o, "ChaosPoisonChance", v);
        }
        let pdb = self.pdb();
        let can_scorch = self.c.flag(pdb, None, &["IgniteCanScorch"])?;
        for a in self.data().elemental_ailment_types.clone() {
            let mut chance = self.c.base(list, c, &[&format!("Enemy{a}Chance")])?
                + self.c.base(edb, None, &[&format!("Self{a}Chance")])?;
            let immune = self.c.flag(
                edb,
                None,
                &[&format!("{a}Immune"), "ElementalAilmentImmune"],
            )?;
            let avoid = 1.0
                - m_min(
                    if immune {
                        100.0
                    } else {
                        self.c.base(
                            edb,
                            None,
                            &[
                                &format!("Avoid{a}"),
                                "AvoidAilments",
                                "AvoidElementalAilments",
                            ],
                        )?
                    },
                    100.0,
                ) / 100.0;
            if a == "Chill" {
                chance = 100.0;
            }
            chance *= avoid;
            if a == "Ignite" && can_scorch {
                self.set_po(o, "ScorchChance", m_min(100.0, chance));
                self.c.add(
                    list,
                    Mod::new("EnemyScorchChance", Base, chance, Some("Ignite Chance")),
                );
            }
            if hit && !(self.c.flag(list, c, &[&format!("Cannot{a}")])? || avoid == 0.0) {
                let on_hit = m_min(100.0, chance);
                self.set_po(o, &format!("{a}ChanceOnHit"), on_hit);
                let alt = self
                    .data()
                    .non_damaging_ailment
                    .get(&a)
                    .is_some_and(|d| d.alt);
                if self.c.flag(list, c, &[&format!("CritsDontAlways{a}")])?
                    || (alt && !self.c.flag(list, c, &["CritAlwaysAltAilments"])?)
                {
                    self.set_po(o, &format!("{a}ChanceOnCrit"), on_hit);
                } else {
                    self.set_po(o, &format!("{a}ChanceOnCrit"), 100.0);
                }
            } else {
                self.set_po(o, &format!("{a}ChanceOnHit"), 0.0);
                self.set_po(o, &format!("{a}ChanceOnCrit"), 0.0);
            }
            if a == "Scorch" && can_scorch && avoid != 0.0 {
                self.set_po(o, "ScorchChanceOnCrit", 100.0);
            }
            let on_hit = self.po(o, &format!("{a}ChanceOnHit")).unwrap_or(f64::NAN);
            let on_crit = if self.c.flag(list, c, &["NeverCrit"])? {
                0.0
            } else {
                self.po(o, &format!("{a}ChanceOnCrit")).unwrap_or(f64::NAN)
            };
            if on_hit + on_crit > 0.0 {
                self.set_flag(i, &format!("inflict{a}"), true);
            }
        }
        let v = if !hit || self.c.flag(list, c, &["CannotKnockback"])? {
            0.0
        } else {
            self.c.base(list, c, &["EnemyKnockbackChance"])?
        };
        self.set_po(o, "KnockbackChanceOnHit", v);
        let v = if effective {
            m_min(100.0, self.c.base(list, c, &["ImpaleChance"])?)
        } else {
            0.0
        };
        self.set_po(o, "ImpaleChance", v);
        for t in ["Fire", "Cold", "Lightning"] {
            if self.c.base(list, c, &[&format!("{t}ExposureChance")])? > 0.0 {
                self.set_flag(i, &format!("apply{t}Exposure"), true);
            }
        }
        let ailments = self.data().ailment_types.clone();
        if effective {
            for a in &ailments {
                let mult = if self.c.flag(edb, None, &[&format!("{a}Immune")])? {
                    0.0
                } else {
                    1.0 - self.c.base(edb, None, &[&format!("Avoid{a}")])? / 100.0
                };
                for k in [format!("{a}ChanceOnHit"), format!("{a}ChanceOnCrit")] {
                    let v = self.po(o, &k).unwrap_or(f64::NAN) * mult;
                    self.set_po(o, &k, v);
                }
                if a == "Poison" {
                    let v = self.po(o, "ChaosPoisonChance").unwrap_or(f64::NAN) * mult;
                    self.set_po(o, "ChaosPoisonChance", v);
                }
            }
        }
        let ailment_mode = match self.input.config.get("ailmentMode") {
            Some(crate::config::Val::Str(s)) => s.to_string(),
            _ => "AVERAGE".to_string(),
        };
        let only_crit = self.c.flag(db, None, &["AilmentsOnlyFromCrit"])?;
        if ailment_mode == "CRIT" || only_crit {
            for a in &ailments {
                self.set_po(o, &format!("{a}ChanceOnHit"), 0.0);
            }
        }
        let never_crit = self.c.flag(db, None, &["AilmentsAreNeverFromCrit"])?;
        if never_crit {
            for a in &ailments {
                let v = if only_crit {
                    0.0
                } else {
                    self.po(o, &format!("{a}ChanceOnHit")).unwrap_or(f64::NAN)
                };
                self.set_po(o, &format!("{a}ChanceOnCrit"), v);
            }
        }
        if o.can_deal[0] {
            self.bleed(o, &cfg)?;
        }
        if o.can_deal[4] {
            self.poison(o, &cfg)?;
        }
        if o.can_deal[3] {
            self.ignite(o, &cfg)?;
        }
        self.non_damaging(o, &ailment_mode)?;
        self.knockback_stun_impale(o)
    }

    fn bleed(&mut self, o: &mut Off, cfg: &Cfg) -> Result<()> {
        let (i, list, edb, db) = (o.skill, o.list, o.edb, o.db);
        let pv = |p: &Self, o: &Off, k: &str| p.po(o, k).unwrap_or(f64::NAN);
        if pv(self, o, "BleedChanceOnHit") + pv(self, o, "BleedChanceOnCrit") <= 0.0 {
            return Ok(());
        }
        let c = Some(cfg);
        let mut dot = self.ailment_cfg(o, cfg, &["Bleed", "Ailment", "PhysicalDot"]);
        let config_stacks = self.c.base(edb, None, &["Multiplier:BleedStacks"])?;
        let max_stacks = match self.c.override_num(list, c, &["BleedStacksMax"])? {
            Some(v) => v,
            None => self.c.base(list, c, &["BleedStacksMax"])?,
        };
        let override_potential = self
            .c
            .override_num(list, None, &["BleedStackPotentialOverride"])?
            .map(|v| v / max_stacks);
        self.set_go(o, "BleedStacksMax", max_stacks);
        let duration_base = match self
            .c
            .override_num(list, Some(&dot), &["BleedDurationBase"])?
        {
            Some(v) => v,
            None => {
                if self.sd_truthy(i, "bleedDurationIsSkillDuration")
                    && let Some(d) = self.sd_num(i, "duration")
                {
                    d
                } else {
                    self.misc_data("BleedDurationBase")
                }
            }
        };
        let mut names = vec![
            "EnemyBleedDuration",
            "EnemyAilmentDuration",
            "DamagingAilmentDuration",
        ];
        if self.sd_truthy(i, "bleedIsSkillEffect") {
            names.push("Duration");
        }
        let duration_mod = m_max(
            self.c.calc_mod(list, Some(&dot), &names)?
                * self
                    .c
                    .calc_mod(edb, None, &["SelfBleedDuration", "SelfAilmentDuration"])?
                / self.c.calc_mod(edb, Some(&dot), &["BleedExpireRate"])?,
            0.0,
        );
        let rate_mod = self.c.calc_mod(list, c, &["BleedFaster"])?
            + self.c.inc(edb, None, &["SelfBleedFaster"])? / 100.0;
        let duration = duration_base * duration_mod / rate_mod * o.debuff_duration_mult;
        self.set_go(o, "BleedDuration", duration);
        let crit = pv(self, o, "CritChance");
        let chance = pv(self, o, "BleedChanceOnHit") / 100.0 * (1.0 - crit / 100.0)
            + pv(self, o, "BleedChanceOnCrit") / 100.0 * crit / 100.0;
        let dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
        let mut stacks = pv(self, o, "HitChance") / 100.0 * chance * dps_mult;
        let speed = self
            .go(o, "HitSpeed")
            .or_else(|| self.go(o, "Speed"))
            .unwrap_or(f64::NAN);
        if speed > 0.0 {
            stacks = stacks * duration * speed;
        }
        let skill_cfg = self.cfg(i);
        let totems = match self.c.override_num(self.pdb(), None, &["TotemsSummoned"])? {
            Some(v) => v,
            None => self.c.base(
                list,
                Some(&skill_cfg),
                &["ActiveTotemLimit", "ActiveBallistaLimit"],
            )?,
        };
        if self.has_flag(i, "totem") {
            stacks *= totems;
        }
        if config_stacks > 0.0 {
            stacks = config_stacks;
        }
        if stacks < 1.0 && override_potential.unwrap_or(0.0) <= 1.0 {
            self.c.add(
                list,
                Mod::new("Condition:SingleBleed", FlagT, true, Some("bleed")),
            );
        }
        let potential = override_potential.unwrap_or(stacks / max_stacks);
        self.set_go(o, "BleedStackPotential", potential);
        let roll = if potential > 1.0 {
            (stacks - (max_stacks - 1.0) / 2.0) / (stacks + 1.0) * 100.0
        } else {
            50.0
        };
        self.set_go(o, "BleedRollAverage", roll);
        let (mut avg_crit, mut max_crit, mut avg_hit, mut min_hit, mut max_hit) =
            (0.0, 0.0, 0.0, 0.0, 0.0);
        for sub in 1..=2 {
            set_cond(&mut dot, "CriticalStrike", sub != 1);
            let (min, max) = self.ailment_source_damage(o, &dot, "Physical", 0)?;
            self.set_po(o, "BleedPhysicalMin", min);
            self.set_po(o, "BleedPhysicalMax", max);
            if sub == 2 {
                let m = self.crit_dot_multi(o, &mut dot, "Physical")?;
                self.set_po(o, "CritBleedDotMulti", m);
                let min_crit = min * m;
                max_crit = max * m;
                avg_crit = min_crit + (max_crit - min_crit) * roll / 100.0;
            } else {
                let m = self.dot_multi(o, &dot, "Physical")?;
                self.set_po(o, "BleedDotMulti", m);
                min_hit = min * m;
                max_hit = max * m;
                avg_hit = min_hit + (max_hit - min_hit) * roll / 100.0;
            }
        }
        let base_percent = self
            .sd_num(i, "bleedBasePercent")
            .unwrap_or_else(|| self.misc_data("BleedPercentBase"));
        let ailment_crit = 100.0 * (1.0 - (1.0 - crit / 100.0).powf(m_max(potential, 1.0)));
        let effects = pv(self, o, "RuthlessBlowAilmentEffect")
            * pv(self, o, "FistOfWarDamageEffect")
            * self.go(o, "AilmentWarcryEffect").unwrap_or(f64::NAN);
        self.ailment_damage(o, "Bleed", ailment_crit, min_hit, 0.0);
        self.ailment_damage(o, "Bleed", 100.0, max_hit, max_crit);
        let avg = self.ailment_damage(o, "Bleed", ailment_crit, avg_hit, avg_crit);
        let base_dps = avg * base_percent / 100.0 * effects;
        if base_dps > 0.0 {
            self.set_flag(i, "bleed", true);
            self.set_flag(i, "duration", true);
            let mut eff = 1.0;
            if self.env.mode.effective {
                let resist = m_min(
                    m_max(0.0, self.c.base(edb, None, &["PhysicalDamageReduction"])?),
                    self.misc_data("EnemyPhysicalDamageReductionCap"),
                );
                let names = [
                    "DamageTaken",
                    "DamageTakenOverTime",
                    "PhysicalDamageTaken",
                    "PhysicalDamageTakenOverTime",
                ];
                let inc = self.c.inc(edb, Some(&dot), &names)?;
                let more = self.c.more(edb, Some(&dot), &names)?;
                eff = (1.0 - resist / 100.0) * (1.0 + inc / 100.0) * more;
                self.set_go(o, "BleedEffMult", eff);
            }
            let effect = self.c.calc_mod(list, Some(&dot), &["AilmentEffect"])?;
            let active = m_min(stacks, max_stacks);
            let base = base_dps * effect * rate_mod * active * eff;
            self.set_po(o, "BaseBleedDPS", base);
            self.set_po(o, "BleedDPS", m_min(base, self.misc_data("DotDpsCap")));
            self.set_go(o, "BleedStacks", stacks);
            self.set_go(o, "BleedDamage", base * duration);
        }
        let _ = db;
        Ok(())
    }

    fn poison(&mut self, o: &mut Off, cfg: &Cfg) -> Result<()> {
        let (i, list, edb) = (o.skill, o.list, o.edb);
        let pv = |p: &Self, o: &Off, k: &str| p.po(o, k).unwrap_or(f64::NAN);
        if pv(self, o, "PoisonChanceOnHit")
            + pv(self, o, "PoisonChanceOnCrit")
            + pv(self, o, "ChaosPoisonChance")
            <= 0.0
        {
            return Ok(());
        }
        let c = Some(cfg);
        let mut dot = self.ailment_cfg(o, cfg, &["Poison", "Ailment", "ChaosDot"]);
        let rate_mod = self.c.calc_mod(list, c, &["PoisonFaster"])?
            + self.c.inc(edb, None, &["SelfPoisonFaster"])? / 100.0;
        let duration_base = match self
            .c
            .override_num(list, Some(&dot), &["PoisonDurationBase"])?
        {
            Some(v) => v,
            None => {
                if self.sd_truthy(i, "poisonDurationIsSkillDuration")
                    && let Some(d) = self.sd_num(i, "duration")
                {
                    d
                } else {
                    self.misc_data("PoisonDurationBase")
                }
            }
        };
        let mut names = vec![
            "EnemyPoisonDuration",
            "EnemyAilmentDuration",
            "DamagingAilmentDuration",
        ];
        if self.sd_truthy(i, "poisonIsSkillEffect") {
            names.push("Duration");
        }
        let duration_mod = m_max(
            self.c.calc_mod(list, Some(&dot), &names)?
                * self
                    .c
                    .calc_mod(edb, None, &["SelfPoisonDuration", "SelfAilmentDuration"])?,
            0.0,
        );
        let duration = duration_base * duration_mod / rate_mod * o.debuff_duration_mult;
        self.set_go(o, "PoisonDuration", duration);
        let chaos_chance = if self.po(o, "ChaosHitAverage").unwrap_or(0.0) > 0.0 {
            pv(self, o, "ChaosPoisonChance")
        } else {
            0.0
        };
        let crit = pv(self, o, "CritChance");
        let on_hit = m_min(100.0, pv(self, o, "PoisonChanceOnHit") + chaos_chance);
        let on_crit = m_min(100.0, pv(self, o, "PoisonChanceOnCrit") + chaos_chance);
        let chance = on_hit / 100.0 * (1.0 - crit / 100.0) + on_crit / 100.0 * crit / 100.0;
        let mut additional = 1.0;
        if !self.c.flag(list, None, &["CannotMultiplePoison"])? {
            additional =
                1.0 + m_min(
                    self.c.base(list, c, &["AdditionalPoisonChance"])? / 100.0,
                    1.0,
                ) + self.c.base(list, c, &["AdditionalPoisonStacks"])?;
        }
        let limit = self.c.min(list, c, &["PoisonStackLimit"])?;
        let mut stacks = pv(self, o, "HitChance") / 100.0
            * chance
            * additional
            * self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN)
            * self.sd_num(i, "stackMultiplier").unwrap_or(1.0)
            * o.quantity_multiplier;
        let speed = self
            .go(o, "HitSpeed")
            .or_else(|| self.go(o, "Speed"))
            .unwrap_or(f64::NAN);
        if speed > 0.0 {
            stacks = stacks * duration * speed;
            if let Some(l) = limit
                && l > 0.0
                && stacks > l
            {
                let hits = (l / additional).ceil();
                stacks = m_min(stacks, hits * additional);
            }
        }
        if stacks < additional && self.config_num("multiplierPoisonOnEnemy").unwrap_or(0.0) == 0.0 {
            self.c.add(
                list,
                Mod::new(
                    "Condition:NonPoisonedOnly",
                    FlagT,
                    true,
                    Some("Calculation"),
                ),
            );
        }
        let (mut hit_dmg, mut crit_dmg, mut max_crit, mut min_hit, mut max_hit) =
            (0.0, 0.0, 0.0, 0.0, 0.0);
        let chaos_flag = 0x10;
        for sub in 1..=2 {
            set_cond(&mut dot, "CriticalStrike", sub != 1);
            let (mut total_min, mut total_max) = (0.0, 0.0);
            let (min, max) = self.ailment_source_damage(o, &dot, "Chaos", 0)?;
            self.set_po(o, "PoisonChaosMin", min);
            self.set_po(o, "PoisonChaosMax", max);
            total_min += min;
            total_max += max;
            let mut non_chaos = 1.0;
            if pv(self, o, "ChaosPoisonChance") > 0.0 && pv(self, o, "PoisonChaosMax") > 0.0 {
                let k = if sub == 2 {
                    "PoisonChanceOnCrit"
                } else {
                    "PoisonChanceOnHit"
                };
                let cc = m_min(100.0, pv(self, o, k) + pv(self, o, "ChaosPoisonChance"));
                non_chaos = pv(self, o, k) / cc;
                self.set_po(o, k, cc);
            }
            for (ti, t) in [(1, "Lightning"), (2, "Cold"), (3, "Fire")] {
                if o.can_deal[ti] && self.c.flag(list, c, &[&format!("{t}CanPoison")])? {
                    let (min, max) = self.ailment_source_damage(o, &dot, t, chaos_flag)?;
                    self.set_po(o, &format!("Poison{t}Min"), min);
                    self.set_po(o, &format!("Poison{t}Max"), max);
                    total_min += min * non_chaos;
                    total_max += max * non_chaos;
                }
            }
            if o.can_deal[0] {
                let (min, max) = self.ailment_source_damage(o, &dot, "Physical", chaos_flag)?;
                self.set_po(o, "PoisonPhysicalMin", min);
                self.set_po(o, "PoisonPhysicalMax", max);
                total_min += min * non_chaos;
                total_max += max * non_chaos;
            }
            if sub == 2 {
                let m = self.crit_dot_multi(o, &mut dot, "Chaos")?;
                self.set_po(o, "CritPoisonDotMulti", m);
                crit_dmg = (total_min + total_max) / 2.0 * m;
                max_crit = total_max * m;
            } else {
                let m = self.dot_multi(o, &dot, "Chaos")?;
                self.set_po(o, "PoisonDotMulti", m);
                hit_dmg = (total_min + total_max) / 2.0 * m;
                min_hit = total_min * m;
                max_hit = total_max * m;
            }
        }
        let percent = self.misc_data("PoisonPercentBase");
        let effects = pv(self, o, "RuthlessBlowAilmentEffect")
            * pv(self, o, "FistOfWarDamageEffect")
            * self.go(o, "AilmentWarcryEffect").unwrap_or(f64::NAN);
        let crit = pv(self, o, "CritChance");
        self.ailment_damage(o, "Poison", crit, min_hit, 0.0);
        self.ailment_damage(o, "Poison", 100.0, max_hit, max_crit);
        let base = self.ailment_damage(o, "Poison", crit, hit_dmg, crit_dmg) * percent * effects;
        if base > 0.0 {
            self.set_flag(i, "poison", true);
            self.set_flag(i, "duration", true);
            let mut eff = 1.0;
            if self.env.mode.effective {
                eff = self.dot_eff_mult(o, &dot, "Chaos", false)?;
                self.set_go(o, "PoisonEffMult", eff);
            }
            if self.c.flag(list, None, &["Condition:NonPoisonedOnly"])? {
                stacks = m_min(additional, stacks);
            }
            self.set_go(o, "PoisonStacks", stacks);
            let effect = self.c.calc_mod(list, Some(&dot), &["AilmentEffect"])?;
            let cap = self.misc_data("DotDpsCap");
            let single = m_min(m_min(base * effect * rate_mod * eff, cap), cap);
            self.set_po(o, "PoisonDPS", single);
            self.set_po(o, "PoisonDamage", single * duration);
            let ground = m_max(
                self.c
                    .max(list, None, &["PoisonDpsAsCausticGround"])?
                    .unwrap_or(0.0),
                self.c
                    .max(edb, None, &["PoisonDpsAsCausticGround"])?
                    .unwrap_or(0.0),
            );
            if ground > 0.0 {
                self.set_po(
                    o,
                    "CausticGroundDPS",
                    m_min(base * effect * rate_mod * eff * ground / 100.0, cap),
                );
                self.c
                    .set_out_val(o.actor, "CausticGroundFromPoison", OutVal::Bool(true));
            }
            if self.sd_truthy(i, "showAverage") {
                self.set_po(o, "TotalPoisonAverageDamage", single * duration);
                self.set_po(o, "TotalPoisonDPS", single);
            } else {
                self.set_po(o, "TotalPoisonDPS", m_min(single * stacks, cap));
            }
        }
        Ok(())
    }

    fn ignite(&mut self, o: &mut Off, cfg: &Cfg) -> Result<()> {
        let (i, list, edb) = (o.skill, o.list, o.edb);
        let pv = |p: &Self, o: &Off, k: &str| p.po(o, k).unwrap_or(f64::NAN);
        if pv(self, o, "IgniteChanceOnHit") + pv(self, o, "IgniteChanceOnCrit") <= 0.0 {
            return Ok(());
        }
        let c = Some(cfg);
        let mut dot = self.ailment_cfg(o, cfg, &["Ignite", "Ailment", "FireDot"]);
        let crit = pv(self, o, "CritChance");
        let per_hit = pv(self, o, "IgniteChanceOnHit") * (1.0 - crit / 100.0)
            + pv(self, o, "IgniteChanceOnCrit") * crit / 100.0;
        self.set_go(o, "IgniteChancePerHit", per_hit);
        let can_stack = self.has_flag(i, "igniteCanStack");
        let mut max_stacks = 1.0;
        if can_stack {
            max_stacks = match self.c.override_num(list, c, &["IgniteStacks"])? {
                Some(v) => v,
                None => max_stacks + self.c.base(list, c, &["IgniteStacks"])?,
            };
        }
        let override_potential = self
            .c
            .override_num(list, None, &["IgniteStackPotentialOverride"])?
            .map(|v| v / max_stacks);
        self.set_go(o, "IgniteStacksMax", max_stacks);
        let rate_mod = (self.c.calc_mod(list, c, &["IgniteBurnFaster"])?
            + self.c.inc(edb, None, &["SelfIgniteBurnFaster"])? / 100.0)
            / self.c.calc_mod(list, c, &["IgniteBurnSlower"])?;
        let duration_base = self
            .c
            .override_num(list, Some(&dot), &["IgniteDurationBase"])?
            .unwrap_or_else(|| self.misc_data("IgniteDurationBase"));
        let duration_mod = m_max(
            self.c.calc_mod(
                list,
                Some(&dot),
                &[
                    "EnemyIgniteDuration",
                    "EnemyAilmentDuration",
                    "EnemyElementalAilmentDuration",
                    "DamagingAilmentDuration",
                ],
            )? * self.c.calc_mod(
                edb,
                None,
                &[
                    "SelfIgniteDuration",
                    "SelfAilmentDuration",
                    "SelfElementalAilmentDuration",
                ],
            )?,
            0.0,
        );
        let duration = duration_base * duration_mod / rate_mod * o.debuff_duration_mult;
        self.set_go(o, "IgniteDuration", duration);
        let chance = pv(self, o, "IgniteChanceOnHit") / 100.0 * (1.0 - crit / 100.0)
            + pv(self, o, "IgniteChanceOnCrit") / 100.0 * crit / 100.0;
        let mut stacks = pv(self, o, "HitChance") / 100.0
            * chance
            * self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
        if !self.sd_truthy(i, "triggeredOnDeath") {
            let speed = self
                .go(o, "HitSpeed")
                .or_else(|| self.go(o, "Speed"))
                .unwrap_or(f64::NAN);
            if let Some(cd) = self.po(o, "Cooldown") {
                let t = self
                    .po(o, "HitTime")
                    .or_else(|| self.po(o, "Time"))
                    .unwrap_or(f64::NAN);
                stacks = stacks * duration / m_max(cd, t);
            } else if speed > 0.0 {
                stacks = stacks * duration * speed;
            }
        }
        let potential = override_potential.unwrap_or(stacks / max_stacks);
        self.set_go(o, "IgniteStackPotential", potential);
        let roll = if potential > 1.0 {
            (stacks - (max_stacks - 1.0) / 2.0) / (stacks + 1.0) * 100.0
        } else {
            50.0
        };
        self.set_go(o, "IgniteRollAverage", roll);
        let (mut hit_dmg, mut crit_dmg, mut max_crit, mut min_hit, mut max_hit) =
            (0.0, 0.0, 0.0, 0.0, 0.0);
        let fire_flag = 0x08;
        for sub in 1..=2 {
            set_cond(&mut dot, "CriticalStrike", sub != 1);
            let (mut total_min, mut total_max) = (0.0, 0.0);
            for (ti, t) in DMG_TYPES.iter().enumerate() {
                let t = *t;
                let ok = if t == "Fire" {
                    o.can_deal[ti] && !self.c.flag(list, c, &["FireCannotIgnite"])?
                } else {
                    o.can_deal[ti] && self.c.flag(list, c, &[&format!("{t}CanIgnite")])?
                };
                if ok {
                    let flags = if t == "Fire" { 0 } else { fire_flag };
                    let (min, max) = self.ailment_source_damage(o, &dot, t, flags)?;
                    self.set_po(o, &format!("Ignite{t}Min"), min);
                    self.set_po(o, &format!("Ignite{t}Max"), max);
                    total_min += min;
                    total_max += max;
                }
            }
            if sub == 2 {
                let m = self.crit_dot_multi(o, &mut dot, "Fire")?;
                self.set_po(o, "CritIgniteDotMulti", m);
                crit_dmg = (total_min + (total_max - total_min) * roll / 100.0) * m;
                max_crit = total_max * m;
            } else {
                let m = self.dot_multi(o, &dot, "Fire")?;
                self.set_po(o, "IgniteDotMulti", m);
                hit_dmg = (total_min + (total_max - total_min) * roll / 100.0) * m;
                min_hit = total_min * m;
                max_hit = total_max * m;
            }
            self.set_po(o, "IgniteTotalMin", total_min);
            self.set_po(o, "IgniteTotalMax", total_max);
        }
        let ailment_crit = 100.0 * (1.0 - (1.0 - crit / 100.0).powf(m_max(1.0, stacks)));
        let percent = self.misc_data("IgnitePercentBase");
        let effects = pv(self, o, "RuthlessBlowAilmentEffect")
            * pv(self, o, "FistOfWarDamageEffect")
            * self.go(o, "AilmentWarcryEffect").unwrap_or(f64::NAN);
        self.ailment_damage(o, "Ignite", ailment_crit, min_hit, 0.0);
        self.ailment_damage(o, "Ignite", 100.0, max_hit, max_crit);
        let base =
            self.ailment_damage(o, "Ignite", ailment_crit, hit_dmg, crit_dmg) * percent * effects;
        if base > 0.0 {
            self.set_flag(i, "ignite", true);
            let mut eff = 1.0;
            if self.env.mode.effective {
                eff = if self.c.flag(list, c, &["IgniteToChaos"])? {
                    self.dot_eff_mult(o, &dot, "Chaos", false)?
                } else {
                    self.dot_eff_mult(o, &dot, "Fire", true)?
                };
                self.set_go(o, "IgniteEffMult", eff);
            }
            let effect = self.c.calc_mod(list, Some(&dot), &["AilmentEffect"])?;
            let active = m_min(stacks, max_stacks);
            let cap = self.misc_data("DotDpsCap");
            let dps = m_min(base * effect * rate_mod * active * eff, cap);
            self.set_po(o, "IgniteDPS", dps);
            let ground = m_max(
                self.c
                    .max(list, None, &["IgniteDpsAsBurningGround"])?
                    .unwrap_or(0.0),
                self.c
                    .max(edb, None, &["IgniteDpsAsBurningGround"])?
                    .unwrap_or(0.0),
            );
            if ground > 0.0 {
                let fire = self.dot_eff_mult(o, &dot, "Fire", true)?;
                self.set_go(
                    o,
                    "BurningGroundDPS",
                    m_min(base * effect * rate_mod * fire * ground / 100.0, cap),
                );
                self.c
                    .set_out_val(o.actor, "BurningGroundFromIgnite", OutVal::Bool(true));
            }
            self.set_go(o, "IgniteDamage", dps * duration);
            if can_stack {
                self.set_po(o, "IgniteDamage", dps * duration);
                self.set_po(o, "IgniteStacksMax", max_stacks);
                self.set_po(o, "TotalIgniteDPS", dps);
            }
        }
        Ok(())
    }

    /// Chill, freeze, shock, scorch, brittle and sap
    fn non_damaging(&mut self, o: &mut Off, ailment_mode: &str) -> Result<()> {
        let (i, list, edb, db) = (o.skill, o.list, o.edb, o.db);
        let pv = |p: &Self, o: &Off, k: &str| p.po(o, k).unwrap_or(f64::NAN);
        let cfg = self.pcfg(o);
        let c = Some(&cfg);
        if self.has_type(i, "ChillingArea") || self.has_type(i, "NonHitChill") {
            self.set_flag(i, "chill", true);
            let inc = self.c.inc(list, c, &["EnemyChillEffect"])?;
            let more = self.c.more(list, c, &["EnemyChillEffect"])?;
            let effect =
                (1.0 + inc / 100.0) * more * self.c.calc_mod(edb, None, &["SelfChillEffect"])?;
            self.set_po(o, "ChillEffectMod", effect);
            let dur_names = [
                "EnemyChillDuration",
                "EnemyAilmentDuration",
                "EnemyElementalAilmentDuration",
            ];
            let self_names = [
                "SelfChillDuration",
                "SelfAilmentDuration",
                "SelfElementalAilmentDuration",
            ];
            let inc_d = self.c.inc(list, c, &dur_names)? + self.c.inc(edb, None, &self_names)?;
            let more_d = self.c.more(list, c, &dur_names)? * self.c.more(edb, None, &self_names)?;
            self.set_po(o, "ChillDurationMod", (1.0 + inc_d / 100.0) * more_d);
            let chill = &self.data().non_damaging_ailment["Chill"];
            let (max, default) = (chill.max, chill.default.unwrap_or(f64::NAN));
            let max = self
                .c
                .override_num(list, None, &["ChillMax"])?
                .unwrap_or(max);
            self.set_po(
                o,
                "ChillSourceEffect",
                m_min(max, (default * effect).floor()),
            );
        }
        let crit = pv(self, o, "CritChance");
        if pv(self, o, "FreezeChanceOnHit") + pv(self, o, "FreezeChanceOnCrit") > 0.0 {
            let (h, cr) = self.average_source_damage(o, &cfg, "Freeze")?;
            let base = self.ailment_damage(o, "Freeze", crit, h, cr)
                * self.c.more(list, c, &["FreezeAsThoughDealing"])?;
            if base > 0.0 {
                self.set_flag(i, "freeze", true);
                let v = 1.0
                    + self.c.inc(
                        list,
                        c,
                        &[
                            "EnemyFreezeDuration",
                            "EnemyAilmentDuration",
                            "EnemyElementalAilmentDuration",
                        ],
                    )? / 100.0
                    + self.c.inc(
                        edb,
                        None,
                        &[
                            "SelfFreezeDuration",
                            "SelfElementalAilmentDuration",
                            "SelfAilmentDuration",
                            "HoarfrostFreezeDuration",
                        ],
                    )? / 100.0;
                self.set_po(o, "FreezeDurationMod", v);
            }
        }
        let never_crit = self.c.flag(db, None, &["AilmentsAreNeverFromCrit"])?;
        for a in ["Chill", "Shock", "Scorch", "Brittle", "Sap"] {
            if pv(self, o, &format!("{a}ChanceOnHit")) + pv(self, o, &format!("{a}ChanceOnCrit"))
                <= 0.0
            {
                continue;
            }
            self.set_pass_cond(o, "CriticalStrike", ailment_mode == "CRIT" && !never_crit);
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            let (h, cr) = self.average_source_damage(o, &cfg, a)?;
            let damage = self.ailment_damage(o, a, crit, h, cr)
                * self.c.more(list, c, &[&format!("{a}AsThoughDealing")])?;
            if damage > 0.0 {
                self.set_flag(i, &a.to_lowercase(), true);
                let ed = format!("Enemy{a}Duration");
                let sd = format!("Self{a}Duration");
                let inc = self.c.inc(
                    list,
                    c,
                    &[&ed, "EnemyElementalAilmentDuration", "EnemyAilmentDuration"],
                )? + self.c.inc(
                    edb,
                    None,
                    &[&sd, "SelfElementalAilmentDuration", "SelfAilmentDuration"],
                )?;
                let more = self.c.more(
                    list,
                    c,
                    &[&ed, "EnemyElementalAilmentDuration", "EnemyAilmentDuration"],
                )? * self.c.more(
                    edb,
                    None,
                    &[&sd, "SelfElementalAilmentDuration", "SelfAilmentDuration"],
                )?;
                let base = self.data().non_damaging_ailment[a]
                    .duration
                    .unwrap_or(f64::NAN);
                self.set_po(
                    o,
                    &format!("{a}Duration"),
                    base * (1.0 + inc / 100.0) * more * o.debuff_duration_mult,
                );
                let effect = self.c.calc_mod(list, c, &[&format!("Enemy{a}Effect")])?
                    * self.c.calc_mod(edb, None, &[&format!("Self{a}Effect")])?;
                self.set_po(o, &format!("{a}EffectMod"), effect);
            }
        }
        Ok(())
    }

    fn knockback_stun_impale(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, edb) = (o.skill, o.list, o.edb);
        let pv = |p: &Self, o: &Off, k: &str| p.po(o, k).unwrap_or(f64::NAN);
        let cfg = self.pcfg(o);
        let c = Some(&cfg);
        let crit = pv(self, o, "CritChance");
        let kb = m_min(
            100.0,
            pv(self, o, "KnockbackChanceOnHit") * (1.0 - crit / 100.0)
                + pv(self, o, "KnockbackChanceOnCrit") * crit / 100.0
                + self.c.base(edb, None, &["SelfKnockbackChance"])?,
        );
        self.set_po(o, "KnockbackChance", kb);
        if kb > 0.0 {
            let v = round(
                4.0 * self.c.calc_mod(list, c, &["EnemyKnockbackDistance"])?,
                0.0,
            );
            self.set_po(o, "KnockbackDistance", v);
        }
        let red = -self.c.inc(list, c, &["EnemyStunThreshold"])?;
        let v = if red > 75.0 {
            1.0 - (75.0 + (red - 75.0) * 25.0 / (red - 50.0)) / 100.0
        } else {
            1.0 - red / 100.0
        };
        self.set_po(o, "EnemyStunThresholdMod", v);
        let base = self.sd_num(i, "baseStunDuration").unwrap_or(0.35);
        let inc = self.c.inc(list, c, &["EnemyStunDuration"])?;
        let inc_crit = self.c.inc(list, c, &["EnemyStunDurationOnCrit"])?;
        let more = self.c.more(list, c, &["EnemyStunDuration"])?;
        let double = m_min(
            self.c.base(list, c, &["DoubleEnemyStunDurationChance"])?
                + self.c.base(edb, c, &["SelfDoubleStunDurationChance"])?,
            100.0,
        );
        let recov = self.c.inc(edb, None, &["StunRecovery"])?;
        let min = base * more / (1.0 + recov / 100.0);
        let mut dur;
        if inc_crit != 0.0 && crit != 0.0 {
            if crit == 100.0 {
                dur = min * (1.0 + (inc + inc_crit) / 100.0);
            } else {
                dur = min * (1.0 + (inc + inc_crit * crit / 100.0) / 100.0);
            }
        } else {
            dur = min * (1.0 + inc / 100.0);
        }
        if double != 0.0 {
            dur *= 1.0 + double / 100.0;
        }
        self.set_po(o, "EnemyStunDuration", dur);
        // Impale
        if o.can_deal[0] && pv(self, o, "ImpaleChance") + pv(self, o, "ImpaleChanceOnCrit") > 0.0 {
            self.set_flag(i, "impale", true);
            let cc = crit / 100.0;
            let chance = m_min(pv(self, o, "ImpaleChance") / 100.0, 1.0) * (1.0 - cc)
                + m_min(pv(self, o, "ImpaleChanceOnCrit") / 100.0, 1.0) * cc;
            let max_stacks = self.c.base(list, c, &["ImpaleStacksMax"])?
                * (1.0 + self.c.base(list, c, &["ImpaleAdditionalDurationChance"])? / 100.0);
            let config_stacks = self.c.base(edb, c, &["Multiplier:ImpaleStacks"])?;
            let stacks = m_min(max_stacks, config_stacks);
            let on_bleed = self.c.inc(list, c, &["ImpaleEffectOnBleed"])?
                * m_min(self.c.base(list, c, &["BleedChance"])? / 100.0, 1.0);
            let inc = (self.c.inc(list, c, &["ImpaleEffect"])? + on_bleed) / 100.0;
            let more = round(self.c.more(list, c, &["ImpaleEffect"])?, 2.0);
            let stored = self.misc_data("ImpaleStoredDamageBase") * (1.0 + inc) * more;
            let hit_mod = stored * stacks;
            let armour = m_max(self.c.val(edb, "Armour", None)?, 0.0);
            let reduction = armour_reduction_f(armour, hit_mod * pv(self, o, "impaleStoredHitAvg"));
            let mut resist = m_min(
                m_max(
                    0.0,
                    self.c.base(edb, None, &["PhysicalDamageReduction"])?
                        + self
                            .c
                            .base(list, c, &["EnemyImpalePhysicalDamageReduction"])?
                        + reduction,
                ),
                self.misc_data("EnemyPhysicalDamageReductionCap"),
            );
            if self
                .c
                .flag(list, c, &["IgnoreEnemyImpalePhysicalDamageReduction"])?
            {
                resist = 0.0;
            }
            let taken_cfg = Cfg {
                flags: Some(self.mf("Hit")),
                ..Cfg::default()
            };
            let names = ["DamageTaken", "PhysicalDamageTaken", "ReflectedDamageTaken"];
            let taken = (1.0 + self.c.inc(edb, Some(&taken_cfg), &names)? / 100.0)
                * self.c.more(edb, Some(&taken_cfg), &names)?;
            let modifier = hit_mod * (1.0 - resist / 100.0) * chance * taken;
            self.set_go(o, "ImpaleStacksMax", max_stacks);
            self.set_go(o, "ImpaleStacks", stacks);
            self.set_po(o, "ImpaleStoredDamage", stored * 100.0);
            self.set_po(o, "ImpaleModifier", 1.0 + modifier);
        }
        Ok(())
    }
}

/// `cfg.skillCond[var] = on`
pub(crate) fn set_cond(cfg: &mut Cfg, var: &str, on: bool) {
    let conds = cfg.skill_cond.get_or_insert_with(Default::default);
    if on {
        conds.insert(Sym::new(var));
    } else {
        conds.remove(&Sym::new(var));
    }
}
