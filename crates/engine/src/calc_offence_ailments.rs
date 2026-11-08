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
    fn ailment_source_damage(
        &mut self,
        o: &Off,
        cfg: &Cfg,
        t: &str,
        flags: u32,
        key: Option<&str>,
    ) -> Result<(f64, f64)> {
        use crate::sf;
        let conv = o.conv[0].as_ref().expect("conversion tables");
        let mut tbl = match key {
            Some(k) => self.pbd(o).and_then(|b| b.table_mut(k)).map(std::mem::take),
            None => None,
        };
        let (min, max) = self.calc_damage(o, cfg, conv, t, flags, tbl.as_mut(), None)?;
        let m = conv[crate::calc_offence::dmg_index(t)].mult;
        if let (Some(mut b), Some(k)) = (tbl, key) {
            if m != 1.0 {
                b.push("Source damage:");
                b.push(sf!("%d to %d ^8(total damage)", min, max));
                b.push(sf!(
                    "x %g ^8(%g%% converted to other damage types)",
                    m,
                    (1.0 - m) * 100.0
                ));
                b.push(sf!("= %d to %d", min * m, max * m));
            }
            if let Some(bd) = self.pbd(o) {
                bd.set(k, b);
            }
        }
        Ok((min * m, max * m))
    }

    /// `breakdown[key] = { damageTypes = { } }`
    fn ailment_source_bd(&mut self, o: &Off, key: &str) {
        if let Some(bd) = self.pbd(o) {
            let mut t = crate::breakdown::Bd::new();
            t.set("damageTypes", crate::breakdown::Bd::new());
            bd.set(key, t);
        }
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
        hide: bool,
    ) -> Result<f64> {
        use crate::sf;
        let on_hit = self.po(o, &format!("{t}ChanceOnHit")).unwrap_or(f64::NAN);
        let on_crit = self.po(o, &format!("{t}ChanceOnCrit")).unwrap_or(f64::NAN);
        let from_hit = on_hit * (1.0 - crit_chance / 100.0);
        let from_crit = on_crit * crit_chance / 100.0;
        let chance = from_hit + from_crit;
        self.set_po(o, &format!("{t}Chance"), chance);
        let base_hit = hit * from_hit / (from_hit + from_crit);
        let base_crit = crit * from_crit / (from_hit + from_crit);
        let base = base_hit + base_crit;
        if hide || self.pbd(o).is_none() {
            return Ok(base);
        }
        let cfg = self.pcfg(o);
        let mult = self
            .c
            .more(o.list, Some(&cfg), &[&format!("{t}AsThoughDealing")])?;
        let label = o.passes[o.pass].label;
        let attack = o.is_attack;
        let pass_crit = self.po(o, "CritChance").unwrap_or(f64::NAN);
        let bd = self.pbd(o).unwrap();
        if chance != 0.0 {
            let b = bd.table(&format!("{t}Chance"));
            if !b.lines.is_empty() {
                b.push("");
            }
            if attack {
                b.push(format!("{label}:"));
            }
            b.push(sf!("Chance on Non-crit: %d%%", on_hit));
            b.push(sf!("Chance on Crit: %d%%", on_crit));
            if on_hit != on_crit {
                b.push("Combined chance:");
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
            let b = bd.table(&format!("{t}DPS"));
            if !b.lines.is_empty() {
                b.push("");
            }
            if attack {
                b.push(format!("{label}:"));
            }
            if hit == crit || pass_crit == 0.0 {
                b.push("Total damage:");
                b.push(sf!("%.1f ^8(source damage)", hit));
                if mult > 1.0 {
                    b.push(sf!(
                        "x %.2f ^8(inflicting as though dealing more damage)",
                        mult
                    ));
                    b.push(sf!("= %.1f", base * mult));
                }
            } else {
                if base_hit > 0.0 {
                    b.push("Damage from Non-crits:");
                    b.push(sf!("%.1f ^8(source damage from non-crits)", hit));
                    b.push(sf!(
                        "x %.3f ^8(portion of instances created by non-crits)",
                        from_hit / (from_hit + from_crit)
                    ));
                    if mult == 1.0 || base_crit != 0.0 {
                        b.push(sf!("= %.1f", base_hit));
                    }
                }
                if base_crit > 0.0 {
                    b.push("Damage from Crits:");
                    b.push(sf!("%.1f ^8(source damage from crits)", crit));
                    b.push(sf!(
                        "x %.3f ^8(portion of instances created by crits)",
                        from_crit / (from_hit + from_crit)
                    ));
                    if mult == 1.0 || base_hit != 0.0 {
                        b.push(sf!("= %.1f", base_crit));
                    }
                }
                if base_hit > 0.0 && base_crit > 0.0 {
                    b.push("Total damage:");
                    b.push(sf!("%.1f + %.1f", base_hit, base_crit));
                    if mult == 1.0 {
                        b.push(sf!("= %.1f", base));
                    }
                }
                if mult > 1.0 {
                    b.push(sf!(
                        "x %.2f ^8(inflicting as though dealing more damage)",
                        mult
                    ));
                    b.push(sf!("= %.1f", base * mult));
                }
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
        Ok(self.dot_eff_parts(o, cfg, t, elemental)?.3)
    }

    /// The resistance, increased and more damage taken and the multiplier of
    /// [`Self::dot_eff_mult`]
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
            resist,
            inc,
            more,
            (1.0 - resist / 100.0) * (1.0 + inc / 100.0) * more,
        ))
    }

    /// The `sourceRes` of an ailment's effective DPS modifier
    pub(crate) fn dot_source_res(&self, t: &str) -> Result<String> {
        let pdb = self.pdb();
        Ok(
            if self
                .c
                .flag(pdb, None, &[&format!("Enemy{t}ResistEqualToYours")])?
            {
                format!("Your {t} Resistance")
            } else {
                t.to_string()
            },
        )
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
        let breakdown = self.input.breakdown && self.bd(o.actor).is_some();
        if breakdown {
            use crate::breakdown::Bd;
            use crate::sf;
            self.ailment_source_bd(o, "BleedPhysical");
            let hit_chance = pv(self, o, "HitChance");
            let time = self
                .po(o, "HitTime")
                .or_else(|| self.po(o, "Time"))
                .unwrap_or(f64::NAN);
            let is_totem = self.has_flag(i, "totem");
            let mut b = Bd::of(["Ratio of bleed stacks to maximum active bleeds:"]);
            if let Some(v) = override_potential {
                b.push(sf!("= %g ^8(stack potential override)", v));
            } else {
                b.push(sf!("%.2f ^8(chance to hit)", hit_chance / 100.0));
                b.push(sf!("* %.2f ^8(chance to bleed)", chance));
                if speed > 0.0 {
                    b.push(sf!(
                        "* (%.2f / %.2f) ^8(Bleed duration / Attack Time)",
                        duration,
                        time
                    ));
                }
                if dps_mult != 1.0 {
                    b.push(sf!("* %g ^8(DPS multiplier for this skill)", dps_mult));
                }
                if is_totem {
                    b.push(sf!("* %d ^8(active number of Totems)", totems));
                }
                b.push(sf!("/ %d ^8(max number of stacks)", max_stacks));
                b.push(sf!("= %.2f", potential));
            }
            let mut r = Bd::of([
                format!(
                    "{CUSTOM}This is the average roll of a Bleed affecting the enemy if you are constantly attacking"
                ),
                sf!(
                    &format!(
                        "{CUSTOM}If attacking constantly, your average strongest bleed currently achieves ^7%.2f%%{CUSTOM} of its max damage"
                    ),
                    roll
                ),
                String::new(),
                "Average Bleed Roll:".to_string(),
            ]);
            if stacks >= max_stacks {
                r.push(sf!("%.2f - (%.2f - 1)/2", stacks, max_stacks));
                r.push(sf!("/ (%.2f + 1)", stacks));
                r.push(sf!("= %.2f%%", roll));
            } else {
                r.push(sf!("50%% (averaging <= %d bleeds)", max_stacks));
            }
            let g = self.bd(o.actor).unwrap();
            g.set("BleedStackPotential", b);
            g.set("BleedRollAverage", r);
        }
        let (mut avg_crit, mut max_crit, mut avg_hit, mut min_hit, mut max_hit) =
            (0.0, 0.0, 0.0, 0.0, 0.0);
        for sub in 1..=2 {
            set_cond(&mut dot, "CriticalStrike", sub != 1);
            let key = (sub == 1 && breakdown).then_some("BleedPhysical");
            let (min, max) = self.ailment_source_damage(o, &dot, "Physical", 0, key)?;
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
        if breakdown {
            use crate::breakdown::Bd;
            let note = Bd::of([format!(
                "{CUSTOM}NOTE: Calculation uses new Weighted Avg formula for critical hits"
            )]);
            self.pbd(o).unwrap().set("BleedDPS", note);
            let (pmin, pmax) = (
                pv(self, o, "BleedPhysicalMin"),
                pv(self, o, "BleedPhysicalMax"),
            );
            let (dm, cdm) = (
                pv(self, o, "BleedDotMulti"),
                pv(self, o, "CritBleedDotMulti"),
            );
            let src = "phys";
            let lines = derivation(
                "Bleed",
                src,
                "bleed",
                (pmin, pmax, roll),
                (dm, cdm),
                (avg_hit, avg_crit),
                avg_hit == avg_crit || crit == 0.0,
                "%.0f",
            );
            self.bd(o.actor).unwrap().set("BleedDPS", Bd::of(lines));
        }
        let base_percent = self
            .sd_num(i, "bleedBasePercent")
            .unwrap_or_else(|| self.misc_data("BleedPercentBase"));
        let ailment_crit = 100.0 * (1.0 - (1.0 - crit / 100.0).powf(m_max(potential, 1.0)));
        let ruthless = pv(self, o, "RuthlessBlowAilmentEffect");
        let fist = pv(self, o, "FistOfWarDamageEffect");
        let warcry = self.go(o, "AilmentWarcryEffect").unwrap_or(f64::NAN);
        let effects = ruthless * fist * warcry;
        let base_min = self.ailment_damage(o, "Bleed", ailment_crit, min_hit, 0.0, true)?
            * base_percent
            / 100.0;
        let base_max =
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
                if breakdown && eff != 1.0 {
                    let b = crate::breakdown::eff_mult(
                        "Physical", resist, 0.0, inc, eff, more, None, true, None, None,
                    );
                    self.bd(o.actor).unwrap().set("BleedEffMult", b);
                }
            }
            let effect = self.c.calc_mod(list, Some(&dot), &["AilmentEffect"])?;
            let active = m_min(stacks, max_stacks);
            let base = base_dps * effect * rate_mod * active * eff;
            let cap = self.misc_data("DotDpsCap");
            self.set_po(o, "BaseBleedDPS", base);
            let capped = m_min(base, cap);
            self.set_po(o, "BleedDPS", capped);
            self.set_go(o, "BleedStacks", stacks);
            self.set_go(o, "BleedDamage", base * duration);
            if self.input.breakdown {
                self.blend_crit_dot_multi(o, "Bleed", ailment_crit);
            }
            if breakdown {
                use crate::sf;
                let k = |v: f64| v * effect * rate_mod * active * eff;
                let (min_u, max_u) = (k(base_min), k(base_max));
                let (min_c, max_c) = (m_min(min_u, cap), m_min(max_u, cap));
                let b = self.pbd(o).unwrap().table("BleedDPS");
                b.push(sf!(
                    "x %.2f ^8(bleed deals %d%% per second)",
                    base_percent / 100.0,
                    base_percent
                ));
                if effect != 1.0 {
                    b.push(sf!("x %.2f ^8(ailment effect modifier)", effect));
                }
                if ruthless != 1.0 {
                    b.push(sf!("x %.2f ^8(ruthless blow effect modifier)", ruthless));
                }
                if fist != 1.0 {
                    b.push(sf!("x %.2f ^8(fist of war effect modifier)", fist));
                }
                if warcry > 1.0 {
                    b.push(sf!(
                        "x %.2f ^8(combined ailment warcry effect modifier)",
                        warcry
                    ));
                }
                b.push(sf!("= %.1f", base_dps));
                b.push("");
                b.push("Total Bleed DPS:");
                dps_lines(
                    b,
                    "Bleed",
                    (base_dps, base, capped),
                    effect,
                    (rate_mod, "damage rate"),
                    Some((active, "bleed")),
                    eff,
                    (min_u, max_u, min_c, max_c),
                );
                if duration != duration_base {
                    let d = duration_bd(
                        duration_base,
                        duration_mod,
                        rate_mod,
                        "damage rate",
                        o.debuff_duration_mult,
                        duration,
                    );
                    self.bd(o.actor).unwrap().set("BleedDuration", d);
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
        let breakdown = self.input.breakdown && self.bd(o.actor).is_some();
        if breakdown {
            for t in ["Physical", "Lightning", "Cold", "Fire", "Chaos"] {
                self.ailment_source_bd(o, &format!("Poison{t}"));
            }
        }
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
        let dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
        let stack_mult = self.sd_num(i, "stackMultiplier").unwrap_or(1.0);
        let mut stacks = pv(self, o, "HitChance") / 100.0
            * chance
            * additional
            * dps_mult
            * stack_mult
            * o.quantity_multiplier;
        let mut uncapped_stacks = None;
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
                uncapped_stacks = Some(m_max(stacks, hits * additional));
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
        if breakdown {
            use crate::breakdown::{Bd, Chain, multi_chain};
            use crate::{factor, sf};
            let mut b = Bd::new();
            multi_chain(
                &mut b,
                Chain {
                    base: Some(factor!("%.2fs ^8(poison duration)", duration)),
                    factors: vec![
                        factor!("%.2f ^8(poison chance)", chance),
                        factor!("%.2f ^8(hit chance)", pv(self, o, "HitChance") / 100.0),
                        factor!("%.2f ^8(avg. # of poisons inflicted)", additional),
                        factor!("%.2f ^8(hits per second)", speed),
                        factor!("%g ^8(dps multiplier for this skill)", dps_mult),
                        factor!("%g ^8(stack multiplier for this skill)", stack_mult),
                        factor!(
                            "%g ^8(quantity multiplier for this skill)",
                            o.quantity_multiplier
                        ),
                    ],
                    total: Some(sf!("= %.2f", stacks)),
                    ..Chain::default()
                },
            );
            let n = crate::pat::num;
            if self.c.flag(list, None, &["Condition:NonPoisonedOnly"])? {
                b.push("Assuming 'non-Poisoned' Enemy");
                if stacks < additional {
                    b.push("^8(time between hits is longer than poison duration)");
                } else {
                    b.push(format!(
                        "^8(affected by poison stack limit of: {})",
                        n(additional)
                    ));
                }
            } else if let Some(l) = limit
                && stacks >= l
            {
                b.push(format!("^8(affected by poison stack limit of: {})", n(l)));
                if let Some(u) = uncapped_stacks {
                    b.push(format!("^8(uncapped poison stacks: {})", sf!("%.2f", u)));
                }
            }
            self.bd(o.actor).unwrap().set("PoisonStacks", b);
        }
        let (mut hit_dmg, mut crit_dmg, mut max_crit, mut min_hit, mut max_hit) =
            (0.0, 0.0, 0.0, 0.0, 0.0);
        let chaos_flag = 0x10;
        for sub in 1..=2 {
            set_cond(&mut dot, "CriticalStrike", sub != 1);
            let (mut total_min, mut total_max) = (0.0, 0.0);
            let (min, max) = self.ailment_source_damage(
                o,
                &dot,
                "Chaos",
                0,
                (sub == 1 && breakdown).then_some("PoisonChaos"),
            )?;
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
                    let key = format!("Poison{t}");
                    let key = (sub == 1 && breakdown).then_some(key.as_str());
                    let (min, max) = self.ailment_source_damage(o, &dot, t, chaos_flag, key)?;
                    self.set_po(o, &format!("Poison{t}Min"), min);
                    self.set_po(o, &format!("Poison{t}Max"), max);
                    total_min += min * non_chaos;
                    total_max += max * non_chaos;
                }
            }
            if o.can_deal[0] {
                let (min, max) = self.ailment_source_damage(
                    o,
                    &dot,
                    "Physical",
                    chaos_flag,
                    (sub == 1 && breakdown).then_some("PoisonPhysical"),
                )?;
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
        if breakdown {
            let crits_only = matches!(
                self.input.config.get("ailmentMode"),
                Some(crate::config::Val::Str(s)) if &**s == "CRIT"
            );
            let line = format!(
                "Ailment mode: {} ^8(can be changed in the Configuration tab)",
                if crits_only {
                    "Crits Only"
                } else {
                    "Average Damage"
                }
            );
            self.bd(o.actor)
                .unwrap()
                .set("PoisonDPS", crate::breakdown::Bd::of([line]));
        }
        let percent = self.misc_data("PoisonPercentBase");
        let ruthless = pv(self, o, "RuthlessBlowAilmentEffect");
        let effects = ruthless
            * pv(self, o, "FistOfWarDamageEffect")
            * self.go(o, "AilmentWarcryEffect").unwrap_or(f64::NAN);
        let crit = pv(self, o, "CritChance");
        let base_min = self.ailment_damage(o, "Poison", crit, min_hit, 0.0, true)? * percent;
        let base_max =
            self.ailment_damage(o, "Poison", 100.0, max_hit, max_crit, true)? * percent * effects;
        let base =
            self.ailment_damage(o, "Poison", crit, hit_dmg, crit_dmg, false)? * percent * effects;
        if base > 0.0 {
            self.set_flag(i, "poison", true);
            self.set_flag(i, "duration", true);
            let mut eff = 1.0;
            if self.env.mode.effective {
                let (resist, inc, more, e) = self.dot_eff_parts(o, &dot, "Chaos", false)?;
                eff = e;
                self.set_go(o, "PoisonEffMult", eff);
                if breakdown && eff != 1.0 {
                    let src = self.dot_source_res("Chaos")?;
                    let b = crate::breakdown::eff_mult(
                        "Chaos",
                        resist,
                        0.0,
                        inc,
                        eff,
                        more,
                        Some(&src),
                        true,
                        None,
                        None,
                    );
                    self.bd(o.actor).unwrap().set("PoisonEffMult", b);
                }
            }
            if self.c.flag(list, None, &["Condition:NonPoisonedOnly"])? {
                stacks = m_min(additional, stacks);
            }
            self.set_go(o, "PoisonStacks", stacks);
            let effect = self.c.calc_mod(list, Some(&dot), &["AilmentEffect"])?;
            let cap = self.misc_data("DotDpsCap");
            let single_u = m_min(base * effect * rate_mod * eff, cap);
            let single = m_min(single_u, cap);
            self.set_po(o, "PoisonDPS", single);
            self.set_po(o, "PoisonDamage", single * duration);
            if self.input.breakdown {
                self.blend_crit_dot_multi(
                    o,
                    "Poison",
                    self.go(o, "CritChance").unwrap_or(f64::NAN),
                );
            }
            let ground = m_max(
                self.c
                    .max(list, None, &["PoisonDpsAsCausticGround"])?
                    .unwrap_or(0.0),
                self.c
                    .max(edb, None, &["PoisonDpsAsCausticGround"])?
                    .unwrap_or(0.0),
            );
            if ground > 0.0 {
                let v = m_min(base * effect * rate_mod * eff * ground / 100.0, cap);
                self.set_po(o, "CausticGroundDPS", v);
                if breakdown {
                    use crate::sf;
                    let b = crate::breakdown::Bd::of([
                        sf!(
                            "%.1f ^8(single poison damage per second)",
                            base * effect * rate_mod
                        ),
                        sf!("* %.1f%% ^8(percent as Caustic ground)", ground),
                        sf!("* %.3f ^8(effect mult)", eff),
                        sf!("= %.1f ^8per second", v),
                    ]);
                    self.pbd(o).unwrap().set("CausticGroundDPS", b);
                }
                self.c
                    .set_out_val(o.actor, "CausticGroundFromPoison", OutVal::Bool(true));
            }
            let show_average = self.sd_truthy(i, "showAverage");
            let total_u = single * stacks;
            let total_c = m_min(total_u, cap);
            if show_average {
                self.set_po(o, "TotalPoisonAverageDamage", single * duration);
                self.set_po(o, "TotalPoisonDPS", single);
            } else {
                self.set_po(o, "TotalPoisonDPS", total_c);
            }
            if breakdown {
                use crate::breakdown::Bd;
                use crate::sf;
                let k = |v: f64| v * effect * rate_mod * eff;
                let (min_u, max_u) = (k(base_min), k(base_max));
                let (min_c, max_c) = (m_min(min_u, cap), m_min(max_u, cap));
                let attack = o.is_attack;
                let label = o.passes[o.pass].label;
                let pbd = self.pbd(o).unwrap();
                let b = pbd.table("PoisonDPS");
                if ruthless != 1.0 {
                    b.push(sf!("x %.2f ^8(ruthless blow effect modifier)", ruthless));
                }
                b.push("x 0.30 ^8(poison deals 30% per second)");
                b.push(sf!("= %.1f", base));
                b.push("");
                b.push("Poison DPS:");
                dps_lines(
                    b,
                    "Poison",
                    (base, single_u, single),
                    effect,
                    (rate_mod, "damage rate"),
                    None,
                    eff,
                    (min_u, max_u, min_c, max_c),
                );
                if !show_average && stacks != 1.0 {
                    let mut t = Bd::of([
                        sf!("%.1f ^8single poison damage per second", single),
                        sf!("x %.2f ^8(poison stacks)", stacks),
                    ]);
                    if total_c != total_u {
                        t.push(sf!("= %.1f ^8(Uncapped raw Poison DPS)", total_u));
                        t.push(sf!(
                            &format!(
                                "^8(Raw Poison DPS is {NEGATIVE}overcapped ^8by^7 %.0f ^8:^7 %.1f%%^8)"
                            ),
                            total_u - total_c,
                            (total_u - total_c) / total_c * 100.0
                        ));
                        t.push(sf!("= %d ^8(Capped Poison DPS)", total_c));
                    } else {
                        t.push(sf!("= %.1f ^8per second", total_c));
                    }
                    pbd.set("TotalPoisonDPS", t);
                }
                let mut d = Bd::new();
                if attack {
                    d.push(format!("{label}:"));
                }
                d.push(sf!("%.1f ^8(damage per second)", single));
                d.push(sf!("x %.2fs ^8(poison duration)", duration));
                d.push(sf!("= %.1f ^8damage per poison stack", single * duration));
                pbd.set("PoisonDamage", d);
                if duration != 2.0 {
                    let d = duration_bd(
                        duration_base,
                        duration_mod,
                        rate_mod,
                        "damage rate",
                        o.debuff_duration_mult,
                        duration,
                    );
                    self.bd(o.actor).unwrap().set("PoisonDuration", d);
                }
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
        let breakdown = self.input.breakdown && self.bd(o.actor).is_some();
        if breakdown {
            for t in ["Physical", "Lightning", "Cold", "Fire", "Chaos"] {
                self.ailment_source_bd(o, &format!("Ignite{t}"));
            }
        }
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
        let override_raw = self
            .c
            .override_num(list, None, &["IgniteStackPotentialOverride"])?;
        let override_potential = override_raw.map(|v| v / max_stacks);
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
        let cooldown = self.po(o, "Cooldown");
        let time = self
            .po(o, "HitTime")
            .or_else(|| self.po(o, "Time"))
            .unwrap_or(f64::NAN);
        if !on_death {
            if let Some(cd) = cooldown {
                stacks = stacks * duration / m_max(cd, time);
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
        if breakdown {
            use crate::breakdown::Bd;
            use crate::sf;
            let mut b = Bd::of([
                format!("{CUSTOM}NOTE: Calculation uses a Weighted Avg formula"),
                String::new(),
            ]);
            if let (Some(raw), Some(p)) = (override_raw, override_potential) {
                if max_stacks != 1.0 {
                    b.push(sf!(
                        "= %d / %d ^8(stack potential override / max ignite stacks)",
                        raw,
                        max_stacks
                    ));
                    b.push(sf!("= %g ^8(stack potential)", p));
                } else {
                    b.push(sf!("= %g ^8(stack potential override)", p));
                }
            } else {
                b.push(sf!(
                    "%.2f ^8(chance to hit)",
                    pv(self, o, "HitChance") / 100.0
                ));
                b.push(sf!("* %.2f ^8(chance to ignite)", chance));
                if on_death {
                    b.push("* 1 ^8Cast on Death override");
                } else if let Some(cd) = cooldown {
                    b.push(sf!(
                        "* (%.2f / max(%.2f, %.2f) ^8(Ignite Duration / max(Cooldown, Cast Time))",
                        duration,
                        cd,
                        time
                    ));
                } else if speed > 0.0 {
                    let gt = self
                        .go(o, "HitTime")
                        .or_else(|| self.po(o, "Time"))
                        .unwrap_or(f64::NAN);
                    b.push(sf!(
                        "* (%.2f / %.2f) ^8(Ignite Duration / Attack Time)",
                        duration,
                        gt
                    ));
                }
                if dps_mult != 1.0 {
                    b.push(sf!("* %g ^8(DPS multiplier for this skill)", dps_mult));
                }
                b.push(sf!("/ %d ^8(max number of stacks)", max_stacks));
                b.push(sf!("= %.2f", potential));
            }
            let mut r = Bd::of([
                format!(
                    "{CUSTOM}This is the average roll of an ignite affecting the enemy if you are constantly attacking"
                ),
                sf!(
                    &format!(
                        "{CUSTOM}If attacking constantly, your average strongest Ignite currently achieves ^7%.2f%%{CUSTOM} of its max damage"
                    ),
                    roll
                ),
                String::new(),
                "Average Ignite Roll:".to_string(),
            ]);
            if stacks >= max_stacks {
                r.push(sf!("%.2f - (%.2f - 1)/2", stacks, max_stacks));
                r.push(sf!("/ (%.2f + 1)", stacks));
                r.push(sf!("= %.2f%%", roll));
            } else {
                r.push(sf!("50%% (averaging <= %d ignites)", max_stacks));
            }
            let g = self.bd(o.actor).unwrap();
            g.set("IgniteStackPotential", b);
            g.set("IgniteRollAverage", r);
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
                    let key = format!("Ignite{t}");
                    let key = (sub == 1 && breakdown).then_some(key.as_str());
                    let (min, max) = self.ailment_source_damage(o, &dot, t, flags, key)?;
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
        if breakdown {
            let (tmin, tmax) = (pv(self, o, "IgniteTotalMin"), pv(self, o, "IgniteTotalMax"));
            let (dm, cdm) = (
                pv(self, o, "IgniteDotMulti"),
                pv(self, o, "CritIgniteDotMulti"),
            );
            let lines = derivation(
                "Ignite",
                "combined",
                "ignite",
                (tmin, tmax, roll),
                (dm, cdm),
                (hit_dmg, crit_dmg),
                hit_dmg == crit_dmg || crit == 0.0,
                "%.2f",
            );
            self.bd(o.actor)
                .unwrap()
                .set("IgniteDPS", crate::breakdown::Bd::of(lines));
        }
        let ailment_crit = 100.0 * (1.0 - (1.0 - crit / 100.0).powf(m_max(1.0, stacks)));
        let percent = self.misc_data("IgnitePercentBase");
        let ruthless = pv(self, o, "RuthlessBlowAilmentEffect");
        let effects = ruthless
            * pv(self, o, "FistOfWarDamageEffect")
            * self.go(o, "AilmentWarcryEffect").unwrap_or(f64::NAN);
        let base_min =
            self.ailment_damage(o, "Ignite", ailment_crit, min_hit, 0.0, true)? * percent;
        let base_max =
            self.ailment_damage(o, "Ignite", 100.0, max_hit, max_crit, true)? * percent * effects;
        let base = self.ailment_damage(o, "Ignite", ailment_crit, hit_dmg, crit_dmg, false)?
            * percent
            * effects;
        if base > 0.0 {
            self.set_flag(i, "ignite", true);
            let mut eff = 1.0;
            if self.env.mode.effective {
                let t = if self.c.flag(list, c, &["IgniteToChaos"])? {
                    "Chaos"
                } else {
                    "Fire"
                };
                let (resist, inc, more, e) = self.dot_eff_parts(o, &dot, t, t == "Fire")?;
                eff = e;
                self.set_go(o, "IgniteEffMult", eff);
                if breakdown && eff != 1.0 {
                    let src = self.dot_source_res(t)?;
                    let b = crate::breakdown::eff_mult(
                        t,
                        resist,
                        0.0,
                        inc,
                        eff,
                        more,
                        Some(&src),
                        true,
                        None,
                        None,
                    );
                    self.bd(o.actor).unwrap().set("IgniteEffMult", b);
                }
            }
            let effect = self.c.calc_mod(list, Some(&dot), &["AilmentEffect"])?;
            let active = m_min(stacks, max_stacks);
            let cap = self.misc_data("DotDpsCap");
            let dps_u = base * effect * rate_mod * active * eff;
            let dps = m_min(dps_u, cap);
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
                let v = m_min(base * effect * rate_mod * fire * ground / 100.0, cap);
                self.set_go(o, "BurningGroundDPS", v);
                if breakdown {
                    use crate::sf;
                    let b = crate::breakdown::Bd::of([
                        sf!(
                            "%.1f ^8(ignite damage per second)",
                            base * effect * rate_mod
                        ),
                        sf!("* %.1f%% ^8(percent as burning ground)", ground),
                        sf!("* %.3f ^8(effective DPS modifier)", fire),
                        sf!("= %.1f ^8per second", v),
                    ]);
                    self.bd(o.actor).unwrap().set("BurningGroundDPS", b);
                }
                self.c
                    .set_out_val(o.actor, "BurningGroundFromIgnite", OutVal::Bool(true));
            }
            self.set_go(o, "IgniteDamage", dps * duration);
            if can_stack {
                self.set_po(o, "IgniteDamage", dps * duration);
                self.set_po(o, "IgniteStacksMax", max_stacks);
                self.set_po(o, "TotalIgniteDPS", dps);
            }
            if breakdown {
                use crate::sf;
                let k = |v: f64| v * effect * rate_mod * active * eff;
                let (min_u, max_u) = (k(base_min), k(base_max));
                let (min_c, max_c) = (m_min(min_u, cap), m_min(max_u, cap));
                let b = self.pbd(o).unwrap().table("IgniteDPS");
                if ruthless != 1.0 {
                    b.push(sf!("x %.2f ^8(ruthless blow effect modifier)", ruthless));
                }
                b.push("x 0.9 ^8(ignite deals 90% per second)");
                b.push(sf!("= %.1f", base));
                b.push("");
                b.push("Average Ignite DPS:");
                dps_lines(
                    b,
                    "Ignite",
                    (base, dps_u, dps),
                    effect,
                    (rate_mod, "burn rate"),
                    Some((active, "ignite")),
                    eff,
                    (min_u, max_u, min_c, max_c),
                );
            }
            if self.input.breakdown {
                self.blend_crit_dot_multi(o, "Ignite", ailment_crit);
            }
            if breakdown {
                use crate::sf;
                if can_stack {
                    let mut d = crate::breakdown::Bd::new();
                    if o.is_attack {
                        d.push(format!("{}:", o.passes[o.pass].label));
                    }
                    d.push(sf!("%.1f ^8(damage per second)", dps));
                    d.push(sf!("x %.2fs ^8(ignite duration)", duration));
                    d.push(sf!("= %.1f ^8damage per ignite stack", dps * duration));
                    self.pbd(o).unwrap().set("IgniteDamage", d);
                }
                if duration != self.misc_data("IgniteDurationBase") {
                    let d = duration_bd(
                        duration_base,
                        duration_mod,
                        rate_mod,
                        "burn rate",
                        o.debuff_duration_mult,
                        duration,
                    );
                    self.bd(o.actor).unwrap().set("IgniteDuration", d);
                }
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
        let breakdown = self.input.breakdown && self.bd(o.actor).is_some();
        let boss_cfg = match self.input.config.get("enemyIsBoss") {
            Some(crate::config::Val::Str(s)) => s.to_string(),
            _ => "Pinnacle".to_string(),
        };
        let mode_line = format!(
            "Ailment mode: {} ^8(can be changed in the Configuration tab)",
            if ailment_mode == "CRIT" {
                "Crits Only"
            } else {
                "Average Damage"
            }
        );
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
            let v = m_min(max, (default * effect).floor());
            self.set_po(o, "ChillSourceEffect", v);
            if breakdown {
                use crate::breakdown::{Bd, Chain, multi_chain};
                use crate::{factor, sf};
                let mut b = Bd::new();
                multi_chain(
                    &mut b,
                    Chain {
                        label: Some(sf!("Effect of Chill: ^8(capped at %d%%)", max)),
                        // upstream's base is a string, which `multiChain` skips
                        factors: vec![
                            factor!(
                                "%.2f ^8(increased/reduced effect of chill)",
                                1.0 + inc / 100.0
                            ),
                            factor!("%.2f ^8(more/less effect of chill)", more),
                        ],
                        total: Some(sf!("= %.0f%%", v)),
                        ..Chain::default()
                    },
                );
                self.pbd(o).unwrap().set("DotChill", b);
            }
        }
        let crit = pv(self, o, "CritChance");
        if pv(self, o, "FreezeChanceOnHit") + pv(self, o, "FreezeChanceOnCrit") > 0.0 {
            if breakdown {
                let b = crate::breakdown::Bd::of([mode_line.clone()]);
                self.bd(o.actor).unwrap().set("FreezeDurationMod", b);
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
                if let Some(bd) = self.pbd(o) {
                    use crate::sf;
                    let b = bd.table("FreezeDPS");
                    b.push(sf!("For freeze to apply for the minimum of 0.3 seconds, target must have no more than %.0f Ailment Threshold.", base * 20.0 * v));
                    b.push("^8(Ailment Threshold is about equal to Life except on bosses where it is about half of their life)");
                }
            }
        }
        let never_crit = self.c.flag(db, None, &["AilmentsAreNeverFromCrit"])?;
        for a in ["Chill", "Shock", "Scorch", "Brittle", "Sap"] {
            if pv(self, o, &format!("{a}ChanceOnHit")) + pv(self, o, &format!("{a}ChanceOnCrit"))
                <= 0.0
            {
                continue;
            }
            if breakdown {
                let b = crate::breakdown::Bd::of([mode_line.clone()]);
                self.bd(o.actor).unwrap().set(&format!("{a}EffectMod"), b);
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
                if breakdown {
                    self.ailment_effect_table(o, a, damage, effect, &boss_cfg)?;
                    if duration != base {
                        use crate::sf;
                        let mut d = crate::breakdown::Bd::new();
                        if o.is_attack {
                            d.push(format!("{}:", o.passes[o.pass].label));
                        }
                        d.push(sf!("%.2fs ^8(base duration)", base));
                        if inc != 0.0 {
                            d.push(sf!(
                                "x %.2f ^8(increased/reduced duration)",
                                1.0 + inc / 100.0
                            ));
                        }
                        if more != 1.0 {
                            d.push(sf!("x %.2f ^8(more/less duration)", more));
                        }
                        if o.debuff_duration_mult != 1.0 {
                            d.push(sf!(
                                "/ %.2f ^8(debuff expires slower/faster)",
                                1.0 / o.debuff_duration_mult
                            ));
                        }
                        d.push(sf!("= %.2fs", duration));
                        self.pbd(o).unwrap().set(&format!("{a}Duration"), d);
                    }
                }
            }
        }
        Ok(())
    }

    /// The table of a non-damaging ailment's effect by enemy ailment
    /// threshold (`breakdown[ailment.."DPS"]`)
    fn ailment_effect_table(
        &mut self,
        o: &Off,
        a: &str,
        damage: f64,
        effect_mod: f64,
        boss_cfg: &str,
    ) -> Result<()> {
        use crate::breakdown::{BdVal, row};
        use crate::sf;
        let edb = o.edb;
        let lvl = self.env.enemy_level;
        let is_boss = boss_cfg != "None";
        let tbl = |n: &str| crate::calc_minion::table_at(self, n, lvl);
        let base_threshold = tbl("monsterAilmentThresholdTable").unwrap_or(f64::NAN);
        let (mut life_mult, mut ailment_mult) = (1.0, 1.0);
        if lvl >= 66.0 {
            life_mult = if is_boss {
                tbl("mapLevelBossLifeMult")
            } else {
                tbl("mapLevelLifeMult")
            }
            .unwrap_or(f64::NAN);
            if is_boss {
                ailment_mult = tbl("mapLevelBossAilmentMult").unwrap_or(f64::NAN);
            }
        }
        let type_mult = if is_boss { 7.68 } else { 1.0 };
        let threshold = base_threshold
            * type_mult
            * life_mult
            * self.c.more(edb, None, &["Life"])?
            * ailment_mult
            * self.c.more(edb, None, &["AilmentThreshold"])?;
        let (mut eff_list, k, ramping): (Vec<f64>, f64, bool) = match a {
            "Chill" => (
                vec![10.0, 20.0],
                50.0,
                self.go(o, "HasBonechill").is_some_and(|v| v != 0.0)
                    || matches!(
                        self.c.out_val(o.actor, "HasBonechill"),
                        Some(OutVal::Bool(true))
                    ),
            ),
            "Shock" => (vec![10.0, 20.0, 40.0], 50.0, true),
            "Scorch" => (vec![5.0, 10.0, 20.0], 50.0, true),
            "Brittle" => (vec![2.0, 4.0], 10.0, true),
            _ => (vec![5.0, 10.0], 100.0 / 3.0, false),
        };
        let ad = self.data().non_damaging_ailment[a].clone();
        let maximum = self.go(o, &format!("Maximum{a}")).unwrap_or(ad.max);
        let current = m_max(
            m_min(self.go(o, &format!("Current{a}")).unwrap_or(0.0), maximum),
            0.0,
        );
        let desired = m_max(
            m_min(
                self.c.base(edb, None, &[&format!("Desired{a}Val")])?,
                maximum,
            ),
            0.0,
        );
        if ad.min != 0.0 {
            eff_list.push(ad.min);
        }
        if threshold > 0.0 {
            eff_list.push(k * (damage / threshold).powf(0.4) * effect_mod);
        }
        if !eff_list.contains(&maximum) {
            eff_list.push(maximum);
        }
        if current > 0.0 && !eff_list.contains(&current) {
            eff_list.push(current);
        }
        if desired > 0.0 && !eff_list.contains(&desired) && current == 0.0 {
            eff_list.push(desired);
        }
        eff_list.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
        let n = crate::pat::num;
        let label = format!(
            "Resulting ailment effect{}",
            if current > 0.0 && ramping {
                format!(" ^8(with a ^7{}% ^8{a} on the enemy)^7", n(current))
            } else {
                String::new()
            }
        );
        let footer = format!(
            "^8(ailment threshold is about equal to life, except on bosses that have specific ailment thresholds)\n(the above table shows that when the enemy has X ailment threshold, you ^8{} for Y)",
            a.to_lowercase()
        );
        let mut rows = Vec::new();
        let p = 10f64.powf(ad.precision);
        for value in eff_list {
            let thresh = damage * (k * effect_mod / value).powf(2.5);
            let dec_check = value / value.floor();
            let value = (value * p).floor() / p;
            let fmt = format!("%.{}f%%", n(ad.precision));
            let mut thresh_s = sf!("%d", thresh);
            if (thresh + 0.5).floor() == (threshold + 0.5).floor() {
                thresh_s.push_str(&sf!(" ^8(%s)", boss_cfg));
            }
            let mut labels = Vec::new();
            if dec_check == 1.0 && value != 0.0 {
                if value == current {
                    labels.push("current");
                }
                if value == desired {
                    labels.push("desired");
                }
                if value == maximum {
                    labels.push("maximum");
                }
                if value == ad.min {
                    labels.push("minimum");
                }
            }
            let mut e = sf!(&fmt, value);
            if !labels.is_empty() {
                e.push_str(&format!(" ^8({})", labels.join(", ")));
            }
            rows.push(row([
                ("effect", BdVal::from(e)),
                ("thresh", thresh_s.into()),
            ]));
        }
        let Some(bd) = self.pbd(o) else {
            return Ok(());
        };
        let t = bd.table(&format!("{a}DPS"));
        let rt = crate::breakdown::row_table(
            &label,
            &[
                ("Ailment Threshold", "thresh"),
                (&format!("{a} Effect"), "effect"),
            ],
        );
        for (k, v) in rt.fields {
            t.fields.insert(k, v);
        }
        t.set("footer", footer);
        for r in rows {
            t.add_row(r);
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
            if let Some(bd) = self.pbd(o) {
                let mut b = crate::breakdown::Bd::new();
                b.set("radius", v);
                bd.set("KnockbackDistance", b);
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
        let mut max;
        let mut dur;
        if inc_crit != 0.0 && crit != 0.0 {
            if crit == 100.0 {
                min *= 1.0 + (inc + inc_crit) / 100.0;
                max = min;
                dur = min;
            } else {
                dur = min * (1.0 + (inc + inc_crit * crit / 100.0) / 100.0);
                max = min * (1.0 + (inc + inc_crit) / 100.0);
                min *= 1.0 + inc / 100.0;
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
        if self.pbd(o).is_some() && dur != base {
            use crate::sf;
            let speed = self.action_speed_mod(crate::calc::ENEMY)?;
            let mut b = crate::breakdown::Bd::of([sf!("%.2fs ^8(base duration)", base)]);
            if inc != 0.0 || (inc_crit != 0.0 && crit != 0.0) {
                b.push(sf!(
                    "x %.2f ^8(increased/reduced stun duration)",
                    1.0 + (inc + inc_crit * crit / 100.0) / 100.0
                ));
            }
            if more != 1.0 {
                b.push(sf!("x %.2f ^8(more/less stun duration)", more));
            }
            if double != 0.0 {
                b.push(sf!(
                    "x %.2f ^8(chance to double stun duration)",
                    1.0 + double / 100.0
                ));
            }
            if recov != 0.0 {
                b.push(sf!(
                    "/ %.2f ^8(increased/reduced enemy stun recovery)",
                    1.0 + recov / 100.0
                ));
            }
            b.push(sf!("= %.2fs", dur));
            if min != max {
                b.push(sf!("(minimum: %.2fs, maximum: %.2fs)", min, max));
            }
            if speed != 1.0 {
                b.push(sf!("/ %.2f ^8(enemy action speed)", speed));
                b.push(sf!(
                    "= %.2fs (note that for effects that care about duration this is ignored)",
                    dur / speed
                ));
                if min != max {
                    b.push(sf!(
                        "(minimum: %.2fs, maximum: %.2fs)",
                        min / speed,
                        max / speed
                    ));
                }
            }
            self.pbd(o).unwrap().set("EnemyStunDuration", b);
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
            if let Some(bd) = self.pbd(o) {
                use crate::breakdown::Bd;
                use crate::sf;
                let m = (1.0 + inc) * more;
                bd.set(
                    "ImpaleStoredDamage",
                    Bd::of([
                        "10% ^8(base value)".to_string(),
                        sf!("x %.2f ^8(increased effectiveness)", m),
                        sf!("= %.1f%%", stored * 100.0),
                    ]),
                );
                let mut b = Bd::of([
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
                ]);
                if taken != 1.0 {
                    b.push(sf!("x %.2f ^8(impale enemy damage taken)", taken));
                }
                b.push(sf!("= %.3f ^8(impale damage multiplier)", modifier));
                bd.set("ImpaleModifier", b);
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

impl<'a> Perform<'a> {
    /// The Calcs tab's DoT multiplier of an ailment, weighted between hits
    /// and crits (`breakdown.critDot`)
    fn blend_crit_dot_multi(&mut self, o: &mut Off, ailment: &str, ailment_crit: f64) {
        let m = self.po(o, &format!("{ailment}DotMulti"));
        let Some(cm) = self.po(o, &format!("Crit{ailment}DotMulti")) else {
            return;
        };
        if Some(cm) == m {
            return;
        }
        let m = m.unwrap_or(f64::NAN);
        let n = |p: &Self, k: &str| p.po(o, k).unwrap_or(f64::NAN);
        let crit = self.go(o, "CritChance").unwrap_or(f64::NAN);
        let from_hit = n(self, &format!("{ailment}ChanceOnHit")) / 100.0 * (1.0 - crit / 100.0);
        let from_crit = n(self, &format!("{ailment}ChanceOnCrit")) / 100.0 * ailment_crit / 100.0;
        let total_hit = from_hit / (from_hit + from_crit);
        let total_crit = from_crit / (from_hit + from_crit);
        if let Some(bd) = self.pbd(o) {
            let b = crate::breakdown::crit_dot(m, cm, total_hit, total_crit);
            bd.set(&format!("{ailment}DotMulti"), b);
        }
        self.set_po(
            o,
            &format!("{ailment}DotMulti"),
            m * total_hit + cm * total_crit,
        );
    }
}

const CUSTOM: &str = "^x5CF0BB";
const NEGATIVE: &str = "^xDD0022";

/// The "Dmg Derivation" lines of a weighted average ailment
#[allow(clippy::too_many_arguments)]
fn derivation(
    name: &str,
    src: &str,
    roll_name: &str,
    (min, max, roll): (f64, f64, f64),
    (dm, cdm): (f64, f64),
    (hit, crit): (f64, f64),
    single: bool,
    non_crit_fmt: &str,
) -> Vec<String> {
    use crate::sf;
    let (a, b) = if src == "phys" {
        ("min source phys", "max source phys")
    } else {
        ("min combined sources", "max combined sources")
    };
    let part = |label: &str, suffix: &str, m: f64, total: String| {
        vec![
            label.to_string(),
            sf!("%.0f + (%.0f - %.0f) * %.2f%%", min, max, min, roll),
            format!("^8{a} + ({b} - {a}) * average {roll_name} roll"),
            sf!(&format!("* %.2f ^8({name} DoT Multi{suffix})"), m),
            total,
        ]
    };
    if single {
        part("Dmg Derivation:", "", dm, sf!("= %.2f", hit))
    } else {
        let mut l = part(
            "Non-Crit Dmg Derivation:",
            " for Non-Crit",
            dm,
            sf!(&format!("= {non_crit_fmt}"), hit),
        );
        l.push(String::new());
        l.extend(part(
            "Crit Dmg Derivation:",
            " for Crit",
            cdm,
            sf!("= %.2f", crit),
        ));
        l
    }
}

/// The DPS and DPS range lines of a damaging ailment
#[allow(clippy::too_many_arguments)]
fn dps_lines(
    b: &mut crate::breakdown::Bd,
    name: &str,
    (base, uncapped, capped): (f64, f64, f64),
    effect: f64,
    (rate, rate_label): (f64, &str),
    stacks: Option<(f64, &str)>,
    eff: f64,
    (min_u, max_u, min_c, max_c): (f64, f64, f64, f64),
) {
    use crate::sf;
    if base != uncapped {
        b.push(sf!("%.1f ^8(base damage per second)", base));
    }
    if effect != 1.0 {
        b.push(sf!("x %.2f ^8(ailment effect modifier)", effect));
    }
    if rate != 1.0 {
        b.push(sf!(&format!("x %.2f ^8({rate_label} modifier)"), rate));
    }
    if let Some((n, s)) = stacks
        && n != 1.0
    {
        b.push(sf!(&format!("x %.2f ^8(avg {s} stacks)"), n));
    }
    if eff != 1.0 {
        b.push(sf!(
            "x %.3f ^8(effective DPS modifier from enemy debuffs)",
            eff
        ));
    }
    if capped != uncapped {
        b.push(sf!(
            &format!("= %.1f ^8(Uncapped raw {name} DPS)"),
            uncapped
        ));
        b.push(sf!(
            &format!("^8(Raw {name} DPS is {NEGATIVE}overcapped ^8by^7 %.0f ^8:^7 %.1f%%^8)"),
            uncapped - capped,
            (uncapped - capped) / capped * 100.0
        ));
        b.push(sf!(&format!("= %d ^8(Capped {name} DPS)"), capped));
    } else {
        b.push(sf!("= %.1f ^8per second", capped));
    }
    b.push(sf!(
        &format!("%.2f%% of Maximum {name} DPS"),
        capped / max_c * 100.0
    ));
    b.push("");
    b.push("DPS Range:");
    if max_c == max_u && min_c == min_u {
        b.push(sf!(
            &format!("%.0f to %.0f ^8({name} DPS Range)"),
            min_u,
            max_u
        ));
    } else {
        b.push(sf!(
            &format!("%.0f to %.0f ^8(Uncapped {name} DPS Range)"),
            min_u,
            max_u
        ));
    }
    for (label, u, c) in [("Min", min_u, min_c), ("Max", max_u, max_c)] {
        if c != u {
            b.push(sf!(
                &format!(
                    "^8(Raw {label} {name} DPS is {NEGATIVE}overcapped ^8by^7 %.0f ^8:^7 %.1f%%^8)"
                ),
                u - c,
                (u - c) / c * 100.0
            ));
        }
    }
    if max_c != max_u || min_c != min_u {
        b.push(sf!(
            &format!("%.0f to %.0f ^8(Capped {name} DPS Range)"),
            min_c,
            max_c
        ));
    }
}

/// The duration breakdown of a damaging ailment
fn duration_bd(
    base: f64,
    duration_mod: f64,
    rate: f64,
    rate_label: &str,
    debuff_mult: f64,
    total: f64,
) -> crate::breakdown::Bd {
    use crate::sf;
    let mut d = crate::breakdown::Bd::of([sf!("%.2fs ^8(base duration)", base)]);
    if duration_mod != 1.0 {
        d.push(sf!("x %.2f ^8(duration modifier)", duration_mod));
    }
    if rate != 1.0 {
        d.push(sf!(&format!("/ %.2f ^8({rate_label} modifier)"), rate));
    }
    if debuff_mult != 1.0 {
        d.push(sf!(
            "/ %.2f ^8(debuff expires slower/faster)",
            1.0 / debuff_mult
        ));
    }
    d.push(sf!("= %.2fs", total));
    d
}
