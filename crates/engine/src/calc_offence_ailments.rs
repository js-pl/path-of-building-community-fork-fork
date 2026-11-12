//! The ailment passes of `calcs.offence` (`Modules/CalcOffence.lua`):
//! chances to inflict ailments, bleed, poison and ignite damage,
//! non-damaging ailments, knockback, stun and impale.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::OutVal;
use crate::breakdown::{Breakdown, Chain, color, f, multi_chain};
use crate::calc::{m_max, m_min, number, round};
use crate::calc_defence::{DMG_TYPES, armour_reduction_f};
use crate::calc_offence::Off;
use crate::calc_perform::{Perform, new_mod};
use crate::mods::{Cfg, Mod, ModType, Sym, Tag};
use crate::sf;

pub type Result<T> = std::result::Result<T, String>;

use ModType::{Base, Flag as FlagT, Override};

impl<'a> Perform<'a> {
    /// `calcAilmentSourceDamage(activeSkill, output, cfg, breakdown,
    /// damageType, typeFlags)`: the skill's own conversion table
    /// With `bd`, the breakdown it adds to (`breakdown.BleedPhysical`, ...)
    fn ailment_source_damage(
        &mut self,
        o: &Off,
        cfg: &Cfg,
        t: &str,
        flags: u32,
        bd: Option<&str>,
    ) -> Result<(f64, f64)> {
        let conv = o.conv[0].as_ref().expect("conversion tables");
        let m = conv[crate::calc_offence::dmg_index(t)].mult;
        let (min, max) = match bd.filter(|_| self.has_bd(o.actor)) {
            Some(k) => {
                let mut rows = Vec::new();
                let r = self.calc_damage_bd(o, cfg, conv, t, flags, &mut rows)?;
                let key = Self::pkey(o, k);
                let b = self.bd(o.actor).unwrap().existing(&key)?;
                b.damage_types.get_or_insert_with(Vec::new).extend(rows);
                if m != 1.0 {
                    b.lines.extend([
                        "Source damage:".into(),
                        sf!("%d to %d ^8(total damage)", r.0, r.1),
                        sf!(
                            "x %g ^8(%g%% converted to other damage types)",
                            m,
                            (1.0 - m) * 100.0
                        ),
                        sf!("= %d to %d", r.0 * m, r.1 * m),
                    ]);
                }
                r
            }
            None => self.calc_damage(o, cfg, conv, t, flags)?,
        };
        Ok((min * m, max * m))
    }

    /// Starts a damaging ailment's source breakdown (`{ damageTypes = { } }`)
    fn source_bd(&mut self, o: &Off, k: &str) {
        let key = Self::pkey(o, k);
        if let Some(b) = self.bd(o.actor) {
            b.set(
                key,
                Breakdown {
                    damage_types: Some(Vec::new()),
                    ..Breakdown::default()
                },
            );
        }
    }

    /// `calcAilmentDamage(type, sourceCritChance, sourceHitDmg,
    /// sourceCritDmg)`: sets `<type>Chance`
    /// `hide`: `hideFromBreakdown`
    fn ailment_damage(
        &mut self,
        o: &mut Off,
        t: &str,
        crit_chance: f64,
        hit: f64,
        crit: f64,
        hide: bool,
    ) -> Result<f64> {
        let on_hit = self.po(o, &format!("{t}ChanceOnHit")).unwrap_or(f64::NAN);
        let on_crit = self.po(o, &format!("{t}ChanceOnCrit")).unwrap_or(f64::NAN);
        let from_hit = on_hit * (1.0 - crit_chance / 100.0);
        let from_crit = on_crit * crit_chance / 100.0;
        let chance = from_hit + from_crit;
        self.set_po(o, &format!("{t}Chance"), chance);
        let base_hit = hit * from_hit / (from_hit + from_crit);
        let base_crit = crit * from_crit / (from_hit + from_crit);
        let base = base_hit + base_crit;
        if hide || !self.has_bd(o.actor) || (chance == 0.0 && base <= 0.0) {
            return Ok(base);
        }
        let cfg = self.pcfg(o);
        let source_mult = self
            .c
            .more(o.list, Some(&cfg), &[&format!("{t}AsThoughDealing")])?;
        let label = o.is_attack.then(|| format!("{}:", o.passes[o.pass].label));
        let out_crit = self.po(o, "CritChance").unwrap_or(f64::NAN);
        let start = |b: &mut Breakdown| {
            if !b.lines.is_empty() {
                b.push(String::new());
            }
            b.lines.extend(label.clone());
        };
        let b = self.bd(o.actor).unwrap();
        if chance != 0.0 {
            let b = b.entry(&Self::pkey(o, &format!("{t}Chance")));
            start(b);
            b.push(sf!("Chance on Non-crit: %d%%", on_hit));
            b.push(sf!("Chance on Crit: %d%%", on_crit));
            if on_hit != on_crit {
                b.push("Combined chance:".into());
                b.push(sf!(
                    "%d x (1 - %.4f) ^8(chance from non-crits)",
                    on_hit,
                    crit_chance / 100.0
                ));
                b.push(sf!(
                    "+ %d x %.4f ^8(chance from crits)",
                    on_crit,
                    crit_chance / 100.0
                ));
                b.push(sf!(
                    "= %.2f",
                    on_hit * (1.0 - crit_chance / 100.0) + on_crit * crit_chance / 100.0
                ));
            }
        }
        if base > 0.0 {
            let b = b.entry(&Self::pkey(o, &format!("{t}DPS")));
            start(b);
            let as_though = |b: &mut Breakdown| {
                if source_mult > 1.0 {
                    b.push(sf!(
                        "x %.2f ^8(inflicting as though dealing more damage)",
                        source_mult
                    ));
                    b.push(sf!("= %.1f", base * source_mult));
                }
            };
            if hit == crit || out_crit == 0.0 {
                b.push("Total damage:".into());
                b.push(sf!("%.1f ^8(source damage)", hit));
                as_though(b);
            } else {
                let portion = |x: f64| x / (from_hit + from_crit);
                if base_hit > 0.0 {
                    b.push("Damage from Non-crits:".into());
                    b.push(sf!("%.1f ^8(source damage from non-crits)", hit));
                    b.push(sf!(
                        "x %.3f ^8(portion of instances created by non-crits)",
                        portion(from_hit)
                    ));
                    if source_mult == 1.0 || base_crit != 0.0 {
                        b.push(sf!("= %.1f", base_hit));
                    }
                }
                if base_crit > 0.0 {
                    b.push("Damage from Crits:".into());
                    b.push(sf!("%.1f ^8(source damage from crits)", crit));
                    b.push(sf!(
                        "x %.3f ^8(portion of instances created by crits)",
                        portion(from_crit)
                    ));
                    if source_mult == 1.0 || base_hit != 0.0 {
                        b.push(sf!("= %.1f", base_crit));
                    }
                }
                if base_hit > 0.0 && base_crit > 0.0 {
                    b.push("Total damage:".into());
                    b.push(sf!("%.1f + %.1f", base_hit, base_crit));
                    if source_mult == 1.0 {
                        b.push(sf!("= %.1f", base));
                    }
                }
                as_though(b);
            }
        }
        Ok(base)
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
        Ok(self.dot_eff_parts(o, cfg, t, elemental)?.0)
    }

    /// The effective multiplier with the resistance and the damage taken
    /// increase and more
    fn dot_eff_parts(
        &self,
        o: &Off,
        cfg: &Cfg,
        t: &str,
        elemental: bool,
    ) -> Result<(f64, f64, f64, f64)> {
        let resist = self.resist_for_type(o, t, Some(cfg))?;
        let (a, b) = (format!("{t}DamageTaken"), format!("{t}DamageTakenOverTime"));
        let mut names = vec!["DamageTaken", "DamageTakenOverTime", a.as_str(), b.as_str()];
        if elemental {
            names.push("ElementalDamageTaken");
        }
        let inc = self.c.inc(o.edb, Some(cfg), &names)?;
        let more = self.c.more(o.edb, Some(cfg), &names)?;
        Ok((
            (1.0 - resist / 100.0) * (1.0 + inc / 100.0) * more,
            resist,
            inc,
            more,
        ))
    }

    /// `breakdown.effMult(t, resist, 0, takenInc, effMult, takenMore,
    /// sourceRes, true)` of a damage over time, `Your <t> Resistance` for
    /// an enemy with the player's
    fn dot_eff_breakdown(
        &self,
        t: &str,
        (eff, resist, inc, more): (f64, f64, f64, f64),
    ) -> Result<Vec<String>> {
        self.dot_type_eff_breakdown(t, eff, resist, inc, more)
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
        self.source_bd(o, "BleedPhysical");
        let bd = self.has_bd(o.actor);
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
        if bd {
            let mut l = vec!["Ratio of bleed stacks to maximum active bleeds:".to_string()];
            if let Some(p) = override_potential {
                l.push(sf!("= %g ^8(stack potential override)", p));
            } else {
                l.push(sf!(
                    "%.2f ^8(chance to hit)",
                    pv(self, o, "HitChance") / 100.0
                ));
                l.push(sf!("* %.2f ^8(chance to bleed)", chance));
                if speed > 0.0 {
                    let t = self.po(o, "HitTime").unwrap_or(pv(self, o, "Time"));
                    l.push(sf!(
                        "* (%.2f / %.2f) ^8(Bleed duration / Attack Time)",
                        duration,
                        t
                    ));
                }
                if dps_mult != 1.0 {
                    l.push(sf!("* %g ^8(DPS multiplier for this skill)", dps_mult));
                }
                if self.has_flag(i, "totem") {
                    l.push(sf!("* %d ^8(active number of Totems)", totems));
                }
                l.push(sf!("/ %d ^8(max number of stacks)", max_stacks));
                l.push(sf!("= %.2f", potential));
            }
            let custom = color::CUSTOM;
            let mut r = vec![
                format!(
                    "{custom}This is the average roll of a Bleed affecting the enemy if you are constantly attacking"
                ),
                sf!(
                    &format!(
                        "{custom}If attacking constantly, your average strongest bleed currently achieves ^7%.2f%%{custom} of its max damage"
                    ),
                    roll
                ),
                String::new(),
                "Average Bleed Roll:".into(),
            ];
            if stacks >= max_stacks {
                r.push(sf!("%.2f - (%.2f - 1)/2", stacks, max_stacks));
                r.push(sf!("/ (%.2f + 1)", stacks));
                r.push(sf!("= %.2f%%", roll));
            } else {
                r.push(sf!("50%% (averaging <= %d bleeds)", max_stacks));
            }
            let b = self.bd(o.actor).unwrap();
            b.set_lines("BleedStackPotential", l);
            b.set_lines("BleedRollAverage", r);
        }
        let (mut avg_crit, mut max_crit, mut avg_hit, mut min_hit, mut max_hit) =
            (0.0, 0.0, 0.0, 0.0, 0.0);
        for sub in 1..=2 {
            set_cond(&mut dot, "CriticalStrike", sub != 1);
            let k = (sub == 1).then_some("BleedPhysical");
            let (min, max) = self.ailment_source_damage(o, &dot, "Physical", 0, k)?;
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
        if bd {
            let (pmin, pmax) = (
                pv(self, o, "BleedPhysicalMin"),
                pv(self, o, "BleedPhysicalMax"),
            );
            let dm = pv(self, o, "BleedDotMulti");
            let derive = |l: &mut Vec<String>| {
                l.push(sf!("%.0f + (%.0f - %.0f) * %.2f%%", pmin, pmax, pmin, roll));
                l.push(
                    "^8min source phys + (max source phys - min source phys) * average bleed roll"
                        .into(),
                );
            };
            let mut l = Vec::new();
            if avg_hit == avg_crit || crit == 0.0 {
                l.push("Dmg Derivation:".into());
                derive(&mut l);
                l.push(sf!("* %.2f ^8(Bleed DoT Multi)", dm));
                l.push(sf!("= %.2f", avg_hit));
            } else {
                l.push("Non-Crit Dmg Derivation:".into());
                derive(&mut l);
                l.push(sf!("* %.2f ^8(Bleed DoT Multi for Non-Crit)", dm));
                l.push(sf!("= %.0f", avg_hit));
                l.push(String::new());
                l.push("Crit Dmg Derivation:".into());
                derive(&mut l);
                l.push(sf!(
                    "* %.2f ^8(Bleed DoT Multi for Crit)",
                    pv(self, o, "CritBleedDotMulti")
                ));
                l.push(sf!("= %.2f", avg_crit));
            }
            let note = format!(
                "{}NOTE: Calculation uses new Weighted Avg formula for critical hits",
                color::CUSTOM
            );
            let b = self.bd(o.actor).unwrap();
            b.set_lines(Self::pkey(o, "BleedDPS"), vec![note]);
            b.set_lines("BleedDPS", l);
        }
        let min_val = self.ailment_damage(o, "Bleed", ailment_crit, min_hit, 0.0, true)?
            * base_percent
            / 100.0;
        let max_val =
            self.ailment_damage(o, "Bleed", 100.0, max_hit, max_crit, true)? * base_percent / 100.0
                * effects;
        let avg = self.ailment_damage(o, "Bleed", ailment_crit, avg_hit, avg_crit, false)?;
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
                if bd && eff != 1.0 {
                    let l = crate::breakdown::eff_mult(crate::breakdown::EffMult {
                        damage_type: "Physical",
                        resist,
                        pen: 0.0,
                        taken: inc,
                        mult: eff,
                        taken_more: more,
                        source_res: None,
                        use_res: true,
                        invert_chance: None,
                        effective_resist: None,
                    });
                    self.bd(o.actor).unwrap().set_lines("BleedEffMult", l);
                }
            }
            let effect = self.c.calc_mod(list, Some(&dot), &["AilmentEffect"])?;
            let active = m_min(stacks, max_stacks);
            let base = base_dps * effect * rate_mod * active * eff;
            let cap = self.misc_data("DotDpsCap");
            let dps = m_min(base, cap);
            self.set_po(o, "BaseBleedDPS", base);
            self.set_po(o, "BleedDPS", dps);
            self.set_go(o, "BleedStacks", stacks);
            self.set_go(o, "BleedDamage", base * duration);
            self.weigh_dot_multi(o, "Bleed", ailment_crit);
            if bd {
                let mut l = vec![sf!(
                    "x %.2f ^8(bleed deals %d%% per second)",
                    base_percent / 100.0,
                    base_percent
                )];
                let eff_lines = EffLines {
                    effect,
                    rate: rate_mod,
                    rate_label: "damage rate modifier",
                    stacks: active,
                    stacks_label: "avg bleed stacks",
                    eff,
                };
                self.ailment_effect_lines(o, &mut l, effect, base_dps);
                l.push("Total Bleed DPS:".into());
                dps_lines(
                    &mut l,
                    "Bleed",
                    base_dps,
                    eff_lines,
                    [base, dps],
                    [min_val, max_val].map(|v| v * effect * rate_mod * active * eff),
                    cap,
                );
                self.bd(o.actor)
                    .unwrap()
                    .entry(&Self::pkey(o, "BleedDPS"))
                    .lines
                    .extend(l);
                if duration != duration_base {
                    let l = ailment_duration_lines(
                        duration_base,
                        duration_mod,
                        rate_mod,
                        o.debuff_duration_mult,
                        duration,
                    );
                    self.bd(o.actor).unwrap().set_lines("BleedDuration", l);
                }
            }
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
        for t in DMG_TYPES {
            self.source_bd(o, &format!("Poison{t}"));
        }
        let bd = self.has_bd(o.actor);
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
        let mut uncapped = None;
        if speed > 0.0 {
            stacks = stacks * duration * speed;
            if let Some(l) = limit
                && l > 0.0
                && stacks > l
            {
                let hits = (l / additional).ceil();
                uncapped = Some(m_max(stacks, hits * additional));
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
        if bd {
            let mut l = Vec::new();
            let dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
            multi_chain(
                &mut l,
                Chain {
                    base: Some(f("%.2fs ^8(poison duration)", duration)),
                    factors: vec![
                        f("%.2f ^8(poison chance)", chance),
                        f("%.2f ^8(hit chance)", pv(self, o, "HitChance") / 100.0),
                        f("%.2f ^8(avg. # of poisons inflicted)", additional),
                        f("%.2f ^8(hits per second)", speed),
                        f("%g ^8(dps multiplier for this skill)", dps_mult),
                        f(
                            "%g ^8(stack multiplier for this skill)",
                            self.sd_num(i, "stackMultiplier").unwrap_or(1.0),
                        ),
                        f(
                            "%g ^8(quantity multiplier for this skill)",
                            o.quantity_multiplier,
                        ),
                    ],
                    total: Some(sf!("= %.2f", stacks)),
                    ..Chain::default()
                },
            );
            let num = crate::pat::num;
            if self.c.flag(list, None, &["Condition:NonPoisonedOnly"])? {
                l.push("Assuming 'non-Poisoned' Enemy".into());
                l.push(if stacks < additional {
                    "^8(time between hits is longer than poison duration)".into()
                } else {
                    format!("^8(affected by poison stack limit of: {})", num(additional))
                });
            } else if let Some(lim) = limit
                && stacks >= lim
            {
                l.push(format!(
                    "^8(affected by poison stack limit of: {})",
                    num(lim)
                ));
                if let Some(u) = uncapped {
                    l.push(format!("^8(uncapped poison stacks: {})", sf!("%.2f", u)));
                }
            }
            self.bd(o.actor).unwrap().set_lines("PoisonStacks", l);
        }
        let (mut hit_dmg, mut crit_dmg, mut max_crit, mut min_hit, mut max_hit) =
            (0.0, 0.0, 0.0, 0.0, 0.0);
        let chaos_flag = 0x10;
        for sub in 1..=2 {
            set_cond(&mut dot, "CriticalStrike", sub != 1);
            let (mut total_min, mut total_max) = (0.0, 0.0);
            let k = (sub == 1).then_some("PoisonChaos");
            let (min, max) = self.ailment_source_damage(o, &dot, "Chaos", 0, k)?;
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
                    let k = format!("Poison{t}");
                    let k = (sub == 1).then_some(k.as_str());
                    let (min, max) = self.ailment_source_damage(o, &dot, t, chaos_flag, k)?;
                    self.set_po(o, &format!("Poison{t}Min"), min);
                    self.set_po(o, &format!("Poison{t}Max"), max);
                    total_min += min * non_chaos;
                    total_max += max * non_chaos;
                }
            }
            if o.can_deal[0] {
                let k = (sub == 1).then_some("PoisonPhysical");
                let (min, max) = self.ailment_source_damage(o, &dot, "Physical", chaos_flag, k)?;
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
        let crit_mode = matches!(self.input.config.get("ailmentMode"), Some(crate::config::Val::Str(s)) if &**s == "CRIT");
        if let Some(b) = self.bd(o.actor) {
            let mode = if crit_mode {
                "Crits Only"
            } else {
                "Average Damage"
            };
            b.set_lines(
                "PoisonDPS",
                vec![sf!(
                    "Ailment mode: %s ^8(can be changed in the Configuration tab)",
                    mode
                )],
            );
        }
        let min_val = self.ailment_damage(o, "Poison", crit, min_hit, 0.0, true)? * percent;
        let max_val =
            self.ailment_damage(o, "Poison", 100.0, max_hit, max_crit, true)? * percent * effects;
        let base =
            self.ailment_damage(o, "Poison", crit, hit_dmg, crit_dmg, false)? * percent * effects;
        if base > 0.0 {
            self.set_flag(i, "poison", true);
            self.set_flag(i, "duration", true);
            let mut eff = 1.0;
            if self.env.mode.effective {
                let parts = self.dot_eff_parts(o, &dot, "Chaos", false)?;
                eff = parts.0;
                self.set_go(o, "PoisonEffMult", eff);
                if bd && eff != 1.0 {
                    let l = self.dot_eff_breakdown("Chaos", parts)?;
                    self.bd(o.actor).unwrap().set_lines("PoisonEffMult", l);
                }
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
                let g = m_min(base * effect * rate_mod * eff * ground / 100.0, cap);
                self.set_po(o, "CausticGroundDPS", g);
                self.c
                    .set_out_val(o.actor, "CausticGroundFromPoison", OutVal::Bool(true));
                if let Some(b) = self.bd(o.actor) {
                    b.set_lines(
                        Self::pkey(o, "CausticGroundDPS"),
                        vec![
                            sf!(
                                "%.1f ^8(single poison damage per second)",
                                base * effect * rate_mod
                            ),
                            sf!("* %.1f%% ^8(percent as Caustic ground)", ground),
                            sf!("* %.3f ^8(effect mult)", eff),
                            sf!("= %.1f ^8per second", g),
                        ],
                    );
                }
            }
            let show_average = self.sd_truthy(i, "showAverage");
            let total_uncapped = single * stacks;
            if show_average {
                self.set_po(o, "TotalPoisonAverageDamage", single * duration);
                self.set_po(o, "TotalPoisonDPS", single);
            } else {
                self.set_po(o, "TotalPoisonDPS", m_min(total_uncapped, cap));
            }
            let crit = self.go(o, "CritChance").unwrap_or(f64::NAN);
            self.weigh_dot_multi(o, "Poison", crit);
            if bd {
                let mut l = Vec::new();
                let ruthless = pv(self, o, "RuthlessBlowAilmentEffect");
                if ruthless != 1.0 {
                    l.push(sf!("x %.2f ^8(ruthless blow effect modifier)", ruthless));
                }
                l.push("x 0.30 ^8(poison deals 30% per second)".into());
                l.push(sf!("= %.1f", base));
                l.push(String::new());
                l.push("Poison DPS:".into());
                let fl = EffLines {
                    effect,
                    rate: rate_mod,
                    rate_label: "damage rate modifier",
                    stacks: 1.0,
                    stacks_label: "",
                    eff,
                };
                dps_lines(
                    &mut l,
                    "Poison",
                    base,
                    fl,
                    [single, single],
                    [min_val, max_val].map(|v| v * effect * rate_mod * eff),
                    cap,
                );
                let key = Self::pkey(o, "PoisonDPS");
                let b = self.bd(o.actor).unwrap();
                b.entry(&key).lines.extend(l);
                if !show_average && stacks != 1.0 {
                    let tc = m_min(total_uncapped, cap);
                    let mut l = vec![
                        sf!("%.1f ^8single poison damage per second", single),
                        sf!("x %.2f ^8(poison stacks)", stacks),
                    ];
                    if tc != total_uncapped {
                        l.push(sf!("= %.1f ^8(Uncapped raw Poison DPS)", total_uncapped));
                        l.push(sf!(
                            &format!(
                                "^8(Raw Poison DPS is {}overcapped ^8by^7 %.0f ^8:^7 %.1f%%^8)",
                                color::NEGATIVE
                            ),
                            total_uncapped - tc,
                            (total_uncapped - tc) / tc * 100.0
                        ));
                        l.push(sf!("= %d ^8(Capped Poison DPS)", tc));
                    } else {
                        l.push(sf!("= %.1f ^8per second", tc));
                    }
                    b.set_lines(Self::pkey(o, "TotalPoisonDPS"), l);
                }
                if duration != 2.0 {
                    let l = ailment_duration_lines(
                        duration_base,
                        duration_mod,
                        rate_mod,
                        o.debuff_duration_mult,
                        duration,
                    );
                    b.set_lines("PoisonDuration", l);
                }
                let mut l: Vec<String> = Vec::new();
                if o.is_attack {
                    l.push(format!("{}:", o.passes[o.pass].label));
                }
                l.push(sf!("%.1f ^8(damage per second)", single));
                l.push(sf!("x %.2fs ^8(poison duration)", duration));
                l.push(sf!("= %.1f ^8damage per poison stack", single * duration));
                b.set_lines(Self::pkey(o, "PoisonDamage"), l);
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
        for t in DMG_TYPES {
            self.source_bd(o, &format!("Ignite{t}"));
        }
        let bd = self.has_bd(o.actor);
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
        let dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
        let mut stacks = pv(self, o, "HitChance") / 100.0 * chance * dps_mult;
        let on_death = self.sd_truthy(i, "triggeredOnDeath");
        let speed = self
            .go(o, "HitSpeed")
            .or_else(|| self.go(o, "Speed"))
            .unwrap_or(f64::NAN);
        if !on_death {
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
        if bd {
            let custom = color::CUSTOM;
            let mut l = vec![
                format!("{custom}NOTE: Calculation uses a Weighted Avg formula"),
                String::new(),
            ];
            if let Some(p) = override_potential {
                if max_stacks != 1.0 {
                    l.push(sf!(
                        "= %d / %d ^8(stack potential override / max ignite stacks)",
                        p * max_stacks,
                        max_stacks
                    ));
                    l.push(sf!("= %g ^8(stack potential)", p));
                } else {
                    l.push(sf!("= %g ^8(stack potential override)", p));
                }
            } else {
                l.push(sf!(
                    "%.2f ^8(chance to hit)",
                    pv(self, o, "HitChance") / 100.0
                ));
                l.push(sf!("* %.2f ^8(chance to ignite)", chance));
                if on_death {
                    l.push("* 1 ^8Cast on Death override".into());
                } else if let Some(cd) = self.po(o, "Cooldown") {
                    let t = self.po(o, "HitTime").unwrap_or(pv(self, o, "Time"));
                    l.push(sf!(
                        "* (%.2f / max(%.2f, %.2f) ^8(Ignite Duration / max(Cooldown, Cast Time))",
                        duration,
                        cd,
                        t
                    ));
                } else if speed > 0.0 {
                    let t = self.go(o, "HitTime").unwrap_or(pv(self, o, "Time"));
                    l.push(sf!(
                        "* (%.2f / %.2f) ^8(Ignite Duration / Attack Time)",
                        duration,
                        t
                    ));
                }
                if dps_mult != 1.0 {
                    l.push(sf!("* %g ^8(DPS multiplier for this skill)", dps_mult));
                }
                l.push(sf!("/ %d ^8(max number of stacks)", max_stacks));
                l.push(sf!("= %.2f", potential));
            }
            let mut r = vec![
                format!(
                    "{custom}This is the average roll of an ignite affecting the enemy if you are constantly attacking"
                ),
                sf!(
                    &format!(
                        "{custom}If attacking constantly, your average strongest Ignite currently achieves ^7%.2f%%{custom} of its max damage"
                    ),
                    roll
                ),
                String::new(),
                "Average Ignite Roll:".into(),
            ];
            if stacks >= max_stacks {
                r.push(sf!("%.2f - (%.2f - 1)/2", stacks, max_stacks));
                r.push(sf!("/ (%.2f + 1)", stacks));
                r.push(sf!("= %.2f%%", roll));
            } else {
                r.push(sf!("50%% (averaging <= %d ignites)", max_stacks));
            }
            let b = self.bd(o.actor).unwrap();
            b.set_lines("IgniteStackPotential", l);
            b.set_lines("IgniteRollAverage", r);
        }
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
                    let k = format!("Ignite{t}");
                    let k = (sub == 1).then_some(k.as_str());
                    let (min, max) = self.ailment_source_damage(o, &dot, t, flags, k)?;
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
        if bd {
            let (tmin, tmax) = (pv(self, o, "IgniteTotalMin"), pv(self, o, "IgniteTotalMax"));
            let derive = |l: &mut Vec<String>| {
                l.push(sf!("%.0f + (%.0f - %.0f) * %.2f%%", tmin, tmax, tmin, roll));
                l.push("^8min combined sources + (max combined sources - min combined sources) * average ignite roll".into());
            };
            let dm = pv(self, o, "IgniteDotMulti");
            let mut l = Vec::new();
            if hit_dmg == crit_dmg || crit == 0.0 {
                l.push("Dmg Derivation:".into());
                derive(&mut l);
                l.push(sf!("* %.2f ^8(Ignite DoT Multi)", dm));
                l.push(sf!("= %.2f", hit_dmg));
            } else {
                l.push("Non-Crit Dmg Derivation:".into());
                derive(&mut l);
                l.push(sf!("* %.2f ^8(Ignite DoT Multi for Non-Crit)", dm));
                l.push(sf!("= %.2f", hit_dmg));
                l.push(String::new());
                l.push("Crit Dmg Derivation:".into());
                derive(&mut l);
                l.push(sf!(
                    "* %.2f ^8(Ignite DoT Multi for Crit)",
                    pv(self, o, "CritIgniteDotMulti")
                ));
                l.push(sf!("= %.2f", crit_dmg));
            }
            self.bd(o.actor).unwrap().set_lines("IgniteDPS", l);
        }
        let min_val = self.ailment_damage(o, "Ignite", ailment_crit, min_hit, 0.0, true)? * percent;
        let max_val =
            self.ailment_damage(o, "Ignite", 100.0, max_hit, max_crit, true)? * percent * effects;
        let base = self.ailment_damage(o, "Ignite", ailment_crit, hit_dmg, crit_dmg, false)?
            * percent
            * effects;
        if base > 0.0 {
            self.set_flag(i, "ignite", true);
            let mut eff = 1.0;
            if self.env.mode.effective {
                let (t, ele) = if self.c.flag(list, c, &["IgniteToChaos"])? {
                    ("Chaos", false)
                } else {
                    ("Fire", true)
                };
                let parts = self.dot_eff_parts(o, &dot, t, ele)?;
                eff = parts.0;
                self.set_go(o, "IgniteEffMult", eff);
                if bd && eff != 1.0 {
                    let l = self.dot_eff_breakdown(t, parts)?;
                    self.bd(o.actor).unwrap().set_lines("IgniteEffMult", l);
                }
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
                let g = m_min(base * effect * rate_mod * fire * ground / 100.0, cap);
                self.set_go(o, "BurningGroundDPS", g);
                self.c
                    .set_out_val(o.actor, "BurningGroundFromIgnite", OutVal::Bool(true));
                if let Some(b) = self.bd(o.actor) {
                    b.set_lines(
                        "BurningGroundDPS",
                        vec![
                            sf!(
                                "%.1f ^8(ignite damage per second)",
                                base * effect * rate_mod
                            ),
                            sf!("* %.1f%% ^8(percent as burning ground)", ground),
                            sf!("* %.3f ^8(effective DPS modifier)", fire),
                            sf!("= %.1f ^8per second", g),
                        ],
                    );
                }
            }
            self.set_go(o, "IgniteDamage", dps * duration);
            if can_stack {
                self.set_po(o, "IgniteDamage", dps * duration);
                self.set_po(o, "IgniteStacksMax", max_stacks);
                self.set_po(o, "TotalIgniteDPS", dps);
            }
            if bd {
                let mut l = Vec::new();
                let ruthless = pv(self, o, "RuthlessBlowAilmentEffect");
                if ruthless != 1.0 {
                    l.push(sf!("x %.2f ^8(ruthless blow effect modifier)", ruthless));
                }
                l.push("x 0.9 ^8(ignite deals 90% per second)".into());
                l.push(sf!("= %.1f", base));
                l.push(String::new());
                l.push("Average Ignite DPS:".into());
                let fl = EffLines {
                    effect,
                    rate: rate_mod,
                    rate_label: "burn rate modifier",
                    stacks: active,
                    stacks_label: "avg ignite stacks",
                    eff,
                };
                let uncapped = base * effect * rate_mod * active * eff;
                dps_lines(
                    &mut l,
                    "Ignite",
                    base,
                    fl,
                    [uncapped, dps],
                    [min_val, max_val].map(|v| v * effect * rate_mod * active * eff),
                    cap,
                );
                let b = self.bd(o.actor).unwrap();
                b.entry(&Self::pkey(o, "IgniteDPS")).lines.extend(l);
                if can_stack {
                    let mut l: Vec<String> = Vec::new();
                    if o.is_attack {
                        l.push(format!("{}:", o.passes[o.pass].label));
                    }
                    l.push(sf!("%.1f ^8(damage per second)", dps));
                    l.push(sf!("x %.2fs ^8(ignite duration)", duration));
                    l.push(sf!("= %.1f ^8damage per ignite stack", dps * duration));
                    b.set_lines(Self::pkey(o, "IgniteDamage"), l);
                }
                if duration != self.misc_data("IgniteDurationBase") {
                    let l = ailment_duration_lines_by(
                        duration_base,
                        duration_mod,
                        (rate_mod, "burn rate modifier"),
                        o.debuff_duration_mult,
                        duration,
                    );
                    self.bd(o.actor).unwrap().set_lines("IgniteDuration", l);
                }
            }
            self.weigh_dot_multi(o, "Ignite", ailment_crit);
        }
        Ok(())
    }

    /// The Calcs tab's ailment DoT multiplier: the hits' and crits'
    /// multipliers weighted by their chances to inflict the ailment
    /// The modifier lines after a damaging ailment's base damage, to its
    /// `= <base dps>` and the blank line before its total
    fn ailment_effect_lines(&self, o: &Off, l: &mut Vec<String>, effect: f64, base_dps: f64) {
        let pv = |k: &str| self.po(o, k).unwrap_or(f64::NAN);
        if effect != 1.0 {
            l.push(sf!("x %.2f ^8(ailment effect modifier)", effect));
        }
        let ruthless = pv("RuthlessBlowAilmentEffect");
        if ruthless != 1.0 {
            l.push(sf!("x %.2f ^8(ruthless blow effect modifier)", ruthless));
        }
        let fist = pv("FistOfWarDamageEffect");
        if fist != 1.0 {
            l.push(sf!("x %.2f ^8(fist of war effect modifier)", fist));
        }
        let warcry = self.go(o, "AilmentWarcryEffect").unwrap_or(f64::NAN);
        if warcry > 1.0 {
            l.push(sf!(
                "x %.2f ^8(combined ailment warcry effect modifier)",
                warcry
            ));
        }
        l.push(sf!("= %.1f", base_dps));
        l.push(String::new());
    }

    fn weigh_dot_multi(&mut self, o: &mut Off, ailment: &str, crit_chance: f64) {
        if !self.has_bd(o.actor) {
            return;
        }
        let k = format!("{ailment}DotMulti");
        let (Some(hit), Some(crit)) = (self.po(o, &k), self.po(o, &format!("Crit{k}"))) else {
            return;
        };
        if crit == hit {
            return;
        }
        let pv = |k: String| self.po(o, &k).unwrap_or(f64::NAN);
        let global_crit = self.go(o, "CritChance").unwrap_or(f64::NAN);
        let from_hit = pv(format!("{ailment}ChanceOnHit")) / 100.0 * (1.0 - global_crit / 100.0);
        let from_crit = pv(format!("{ailment}ChanceOnCrit")) / 100.0 * crit_chance / 100.0;
        let total_hit = from_hit / (from_hit + from_crit);
        let total_crit = from_crit / (from_hit + from_crit);
        let l = crate::breakdown::crit_dot(hit, crit, total_hit, total_crit);
        self.bd(o.actor).unwrap().set_lines(Self::pkey(o, &k), l);
        self.set_po(o, &k, hit * total_hit + crit * total_crit);
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
            let source = m_min(max, (default * effect).floor());
            self.set_po(o, "ChillSourceEffect", source);
            if let Some(b) = self.bd(o.actor) {
                let mut l = Vec::new();
                let label = sf!("Effect of Chill: ^8(capped at %d%%)", max);
                l.push(label);
                multi_chain(
                    &mut l,
                    Chain {
                        factors: vec![
                            f(
                                "%.2f ^8(increased/reduced effect of chill)",
                                1.0 + inc / 100.0,
                            ),
                            f("%.2f ^8(more/less effect of chill)", more),
                        ],
                        total: Some(sf!("= %.0f%%", source)),
                        ..Chain::default()
                    },
                );
                b.set_lines(Self::pkey(o, "DotChill"), l);
            }
        }
        let bd = self.has_bd(o.actor);
        let mode_line = sf!(
            "Ailment mode: %s ^8(can be changed in the Configuration tab)",
            if ailment_mode == "CRIT" {
                "Crits Only"
            } else {
                "Average Damage"
            }
        );
        let crit = pv(self, o, "CritChance");
        if pv(self, o, "FreezeChanceOnHit") + pv(self, o, "FreezeChanceOnCrit") > 0.0 {
            if let Some(b) = self.bd(o.actor) {
                b.set_lines("FreezeDurationMod", vec![mode_line.clone()]);
            }
            let (h, cr) = self.average_source_damage(o, &cfg, "Freeze")?;
            let base = self.ailment_damage(o, "Freeze", crit, h, cr, false)?
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
                if let Some(b) = self.bd(o.actor) {
                    let b = b.entry(&Self::pkey(o, "FreezeDPS"));
                    b.push(sf!("For freeze to apply for the minimum of 0.3 seconds, target must have no more than %.0f Ailment Threshold.", base * 20.0 * v));
                    b.push("^8(Ailment Threshold is about equal to Life except on bosses where it is about half of their life)".into());
                }
            }
        }
        let threshold = if bd {
            self.enemy_ailment_threshold(o)?
        } else {
            0.0
        };
        let never_crit = self.c.flag(db, None, &["AilmentsAreNeverFromCrit"])?;
        for a in ["Chill", "Shock", "Scorch", "Brittle", "Sap"] {
            if pv(self, o, &format!("{a}ChanceOnHit")) + pv(self, o, &format!("{a}ChanceOnCrit"))
                <= 0.0
            {
                continue;
            }
            if let Some(b) = self.bd(o.actor) {
                b.set_lines(format!("{a}EffectMod"), vec![mode_line.clone()]);
            }
            self.set_pass_cond(o, "CriticalStrike", ailment_mode == "CRIT" && !never_crit);
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            let (h, cr) = self.average_source_damage(o, &cfg, a)?;
            let damage = self.ailment_damage(o, a, crit, h, cr, false)?
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
                let duration = base * (1.0 + inc / 100.0) * more * o.debuff_duration_mult;
                self.set_po(o, &format!("{a}Duration"), duration);
                let effect = self.c.calc_mod(list, c, &[&format!("Enemy{a}Effect")])?
                    * self.c.calc_mod(edb, None, &[&format!("Self{a}Effect")])?;
                self.set_po(o, &format!("{a}EffectMod"), effect);
                if bd {
                    self.ailment_effect_table(o, a, damage, effect, threshold)?;
                    if duration != base {
                        let mut l: Vec<String> = Vec::new();
                        if o.is_attack {
                            l.push(format!("{}:", o.passes[o.pass].label));
                        }
                        l.push(sf!("%.2fs ^8(base duration)", base));
                        if inc != 0.0 {
                            l.push(sf!(
                                "x %.2f ^8(increased/reduced duration)",
                                1.0 + inc / 100.0
                            ));
                        }
                        if more != 1.0 {
                            l.push(sf!("x %.2f ^8(more/less duration)", more));
                        }
                        if o.debuff_duration_mult != 1.0 {
                            l.push(sf!(
                                "/ %.2f ^8(debuff expires slower/faster)",
                                1.0 / o.debuff_duration_mult
                            ));
                        }
                        l.push(sf!("= %.2fs", duration));
                        self.bd(o.actor)
                            .unwrap()
                            .set_lines(Self::pkey(o, &format!("{a}Duration")), l);
                    }
                }
            }
        }
        Ok(())
    }

    /// The enemy's ailment threshold (`enemyThreshold`)
    fn enemy_ailment_threshold(&self, o: &Off) -> Result<f64> {
        let boss = self.config_str("enemyIsBoss") != Some("None");
        let level = self.env.enemy_level;
        let key = format!("{}", level as i64);
        let t = |n: &str| self.data().table(n, &key);
        let base = t("monsterAilmentThresholdTable").unwrap_or(f64::NAN);
        let (mut life, mut ailment) = (1.0, 1.0);
        if level >= 66.0 {
            life = if boss {
                t("mapLevelBossLifeMult")
            } else {
                t("mapLevelLifeMult")
            }
            .unwrap_or(f64::NAN);
            if boss {
                ailment = t("mapLevelBossAilmentMult").unwrap_or(f64::NAN);
            }
        }
        let typ = if boss { 7.68 } else { 1.0 };
        Ok(base
            * typ
            * life
            * self.c.more(o.edb, None, &["Life"])?
            * ailment
            * self.c.more(o.edb, None, &["AilmentThreshold"])?)
    }

    /// The table of a non-damaging ailment's effect by ailment threshold
    fn ailment_effect_table(
        &mut self,
        o: &Off,
        a: &str,
        damage: f64,
        effect: f64,
        threshold: f64,
    ) -> Result<()> {
        let data = &self.data().non_damaging_ailment[a];
        let (min, max_default, precision) = (data.min, data.max, data.precision);
        let bonechill = self.po_val(o, "HasBonechill").is_some_and(|v| v.truthy());
        let (mut list, ramping, scale): (Vec<f64>, bool, f64) = match a {
            "Chill" => (vec![10.0, 20.0], bonechill, 50.0),
            "Shock" => (vec![10.0, 20.0, 40.0], true, 50.0),
            "Scorch" => (vec![5.0, 10.0, 20.0], true, 50.0),
            "Brittle" => (vec![2.0, 4.0], true, 10.0),
            _ => (vec![5.0, 10.0], false, 100.0 / 3.0),
        };
        let maximum = self.go(o, &format!("Maximum{a}")).unwrap_or(max_default);
        let current = m_max(
            m_min(self.go(o, &format!("Current{a}")).unwrap_or(0.0), maximum),
            0.0,
        );
        let desired = m_max(
            m_min(
                self.c.base(o.edb, None, &[&format!("Desired{a}Val")])?,
                maximum,
            ),
            0.0,
        );
        if min != 0.0 {
            list.push(min);
        }
        if threshold > 0.0 {
            list.push(scale * (damage / threshold).powf(0.4) * effect);
        }
        if !list.contains(&maximum) {
            list.push(maximum);
        }
        if current > 0.0 && !list.contains(&current) {
            list.push(current);
        }
        if desired > 0.0 && !list.contains(&desired) && current == 0.0 {
            list.push(desired);
        }
        list.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let boss = self.config_str("enemyIsBoss").unwrap_or("nil").to_string();
        let p = 10f64.powf(precision);
        let fmt = format!("%.{}f%%", crate::pat::num(precision));
        let mut rows = Vec::new();
        for v in list {
            let thresh = damage * (scale * effect / v).powf(2.5);
            let dec = v / v.floor();
            let value = (v * p).floor() / p;
            let mut t = sf!("%d", thresh);
            if (thresh + 0.5).floor() == (threshold + 0.5).floor() {
                t += &sf!(" ^8(%s)", boss.as_str());
            }
            let mut labels = Vec::new();
            if dec == 1.0 && value != 0.0 {
                for (x, n) in [
                    (current, "current"),
                    (desired, "desired"),
                    (maximum, "maximum"),
                    (min, "minimum"),
                ] {
                    if value == x {
                        labels.push(n);
                    }
                }
            }
            let mut e = sf!(&fmt, value);
            if !labels.is_empty() {
                e += &format!(" ^8({})", labels.join(", "));
            }
            rows.push(vec![("effect", e.into()), ("thresh", t.into())]);
        }
        let label = if current > 0.0 && ramping {
            format!(
                "Resulting ailment effect{}",
                sf!(" ^8(with a ^7%s%% ^8%s on the enemy)^7", current, a)
            )
        } else {
            "Resulting ailment effect".to_string()
        };
        let b = self
            .bd(o.actor)
            .unwrap()
            .entry(&Self::pkey(o, &format!("{a}DPS")));
        b.label = Some(label);
        b.footer = Some(sf!(
            "^8(ailment threshold is about equal to life, except on bosses that have specific ailment thresholds)\n(the above table shows that when the enemy has X ailment threshold, you ^8%s for Y)",
            a.to_lowercase().as_str()
        ));
        b.rows = Some(rows);
        b.cols = Some(vec![
            crate::breakdown::Col::new("Ailment Threshold", "thresh"),
            crate::breakdown::Col::new(format!("{a} Effect"), "effect"),
        ]);
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
            if let Some(b) = self.bd(o.actor) {
                b.set(
                    Self::pkey(o, "KnockbackDistance"),
                    Breakdown {
                        radius: Some(v),
                        ..Breakdown::default()
                    },
                );
            }
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
        let mut min = base * more / (1.0 + recov / 100.0);
        let mut max = min;
        let mut dur;
        if inc_crit != 0.0 && crit != 0.0 {
            if crit == 100.0 {
                min *= 1.0 + (inc + inc_crit) / 100.0;
                max = min;
                dur = min;
            } else {
                dur = min * (1.0 + (inc + inc_crit * crit / 100.0) / 100.0);
                min *= 1.0 + inc / 100.0;
                max *= 1.0 + (inc + inc_crit) / 100.0;
            }
        } else {
            min *= 1.0 + inc / 100.0;
            max = min;
            dur = min;
        }
        if double != 0.0 {
            if double == 100.0 {
                min *= 2.0;
            }
            max *= 2.0;
            dur *= 1.0 + double / 100.0;
        }
        self.set_po(o, "EnemyStunDuration", dur);
        if dur != base && self.has_bd(o.actor) {
            let mut l = vec![sf!("%.2fs ^8(base duration)", base)];
            if inc != 0.0 || (inc_crit != 0.0 && crit != 0.0) {
                l.push(sf!(
                    "x %.2f ^8(increased/reduced stun duration)",
                    1.0 + (inc + inc_crit * crit / 100.0) / 100.0
                ));
            }
            if more != 1.0 {
                l.push(sf!("x %.2f ^8(more/less stun duration)", more));
            }
            if double != 0.0 {
                l.push(sf!(
                    "x %.2f ^8(chance to double stun duration)",
                    1.0 + double / 100.0
                ));
            }
            if recov != 0.0 {
                l.push(sf!(
                    "/ %.2f ^8(increased/reduced enemy stun recovery)",
                    1.0 + recov / 100.0
                ));
            }
            l.push(sf!("= %.2fs", dur));
            if min != max {
                l.push(sf!("(minimum: %.2fs, maximum: %.2fs)", min, max));
            }
            let enemy_speed = self.action_speed_mod(crate::calc::ENEMY)?;
            if enemy_speed != 1.0 {
                l.push(sf!("/ %.2f ^8(enemy action speed)", enemy_speed));
                l.push(sf!(
                    "= %.2fs (note that for effects that care about duration this is ignored)",
                    dur / enemy_speed
                ));
                if min != max {
                    l.push(sf!(
                        "(minimum: %.2fs, maximum: %.2fs)",
                        min / enemy_speed,
                        max / enemy_speed
                    ));
                }
            }
            self.bd(o.actor)
                .unwrap()
                .set_lines(Self::pkey(o, "EnemyStunDuration"), l);
        }
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
            if let Some(b) = self.bd(o.actor) {
                b.set_lines(
                    Self::pkey(o, "ImpaleStoredDamage"),
                    vec![
                        "10% ^8(base value)".into(),
                        sf!("x %.2f ^8(increased effectiveness)", (1.0 + inc) * more),
                        sf!("= %.1f%%", stored * 100.0),
                    ],
                );
                let mut l = vec![
                    sf!(
                        "%d ^8(number of stacks, can be overridden in the Configuration tab)",
                        stacks
                    ),
                    sf!("x %.3f ^8(stored damage)", stored),
                    sf!("x %.2f ^8(impale chance)", chance),
                    sf!(
                        "x %.2f ^8(impale enemy physical damage reduction)",
                        1.0 - resist / 100.0
                    ),
                ];
                if taken != 1.0 {
                    l.push(sf!("x %.2f ^8(impale enemy damage taken)", taken));
                }
                l.push(sf!("= %.3f ^8(impale damage multiplier)", modifier));
                b.set_lines(Self::pkey(o, "ImpaleModifier"), l);
            }
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

/// A damaging ailment's duration breakdown
fn ailment_duration_lines(base: f64, m: f64, rate: f64, debuff: f64, total: f64) -> Vec<String> {
    ailment_duration_lines_by(base, m, (rate, "damage rate modifier"), debuff, total)
}

fn ailment_duration_lines_by(
    base: f64,
    m: f64,
    (rate, rate_label): (f64, &str),
    debuff: f64,
    total: f64,
) -> Vec<String> {
    let mut l = vec![sf!("%.2fs ^8(base duration)", base)];
    if m != 1.0 {
        l.push(sf!("x %.2f ^8(duration modifier)", m));
    }
    if rate != 1.0 {
        l.push(sf!(&format!("/ %.2f ^8({rate_label})"), rate));
    }
    if debuff != 1.0 {
        l.push(sf!("/ %.2f ^8(debuff expires slower/faster)", 1.0 / debuff));
    }
    l.push(sf!("= %.2fs", total));
    l
}

/// The factors of a damaging ailment's total DPS
struct EffLines {
    effect: f64,
    rate: f64,
    rate_label: &'static str,
    stacks: f64,
    stacks_label: &'static str,
    eff: f64,
}

/// A damaging ailment's total DPS, its share of the maximum and its range,
/// capped at `DotDpsCap`
fn dps_lines(
    l: &mut Vec<String>,
    name: &str,
    base_dps: f64,
    f: EffLines,
    [uncapped, dps]: [f64; 2],
    [min, max]: [f64; 2],
    cap: f64,
) {
    let neg = color::NEGATIVE;
    let over = |what: &str, u: f64, c: f64| {
        sf!(
            &format!("^8(Raw {what} is {neg}overcapped ^8by^7 %.0f ^8:^7 %.1f%%^8)"),
            u - c,
            (u - c) / c * 100.0
        )
    };
    if base_dps != uncapped {
        l.push(sf!("%.1f ^8(base damage per second)", base_dps));
    }
    if f.effect != 1.0 {
        l.push(sf!("x %.2f ^8(ailment effect modifier)", f.effect));
    }
    if f.rate != 1.0 {
        l.push(sf!(&format!("x %.2f ^8({})", f.rate_label), f.rate));
    }
    if f.stacks != 1.0 {
        l.push(sf!(&format!("x %.2f ^8({})", f.stacks_label), f.stacks));
    }
    if f.eff != 1.0 {
        l.push(sf!(
            "x %.3f ^8(effective DPS modifier from enemy debuffs)",
            f.eff
        ));
    }
    if dps != uncapped {
        l.push(sf!(
            &format!("= %.1f ^8(Uncapped raw {name} DPS)"),
            uncapped
        ));
        l.push(over(&format!("{name} DPS"), uncapped, dps));
        l.push(sf!(&format!("= %d ^8(Capped {name} DPS)"), dps));
    } else {
        l.push(sf!("= %.1f ^8per second", dps));
    }
    let (min_c, max_c) = (m_min(min, cap), m_min(max, cap));
    l.push(sf!(
        &format!("%.2f%% of Maximum {name} DPS"),
        dps / max_c * 100.0
    ));
    l.push(String::new());
    l.push("DPS Range:".into());
    if max_c == max && min_c == min {
        l.push(sf!(&format!("%.0f to %.0f ^8({name} DPS Range)"), min, max));
    } else {
        l.push(sf!(
            &format!("%.0f to %.0f ^8(Uncapped {name} DPS Range)"),
            min,
            max
        ));
    }
    if min_c != min {
        l.push(over(&format!("Min {name} DPS"), min, min_c));
    }
    if max_c != max {
        l.push(over(&format!("Max {name} DPS"), max, max_c));
    }
    if max_c != max || min_c != min {
        l.push(sf!(
            &format!("%.0f to %.0f ^8(Capped {name} DPS Range)"),
            min_c,
            max_c
        ));
    }
}
