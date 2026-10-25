//! `calcs.defence` and `calcs.resistances` (`Modules/CalcDefence.lua`) on
//! the typed calculation: resistances, block, the primary defences,
//! evasion, suppression, dodge, recovery, regeneration, recoup, damage
//! reduction and avoidance.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::OutVal;
use crate::calc::{ENEMY, PLAYER, floor, m_max, m_min, number, round};
use crate::calc_perform::{Perform, converted_tags, field_num, new_mod, src};
use crate::config::Val;
use crate::mods::{Cfg, ModType, ModValue, Tag};

pub type Result<T> = std::result::Result<T, String>;

use ModType::{Base, Inc, Max as MaxT, More};

pub(crate) const RESIST_TYPES: [&str; 4] = ["Fire", "Cold", "Lightning", "Chaos"];
pub(crate) const DMG_TYPES: [&str; 5] = ["Physical", "Lightning", "Cold", "Fire", "Chaos"];

pub(crate) fn is_elemental(t: &str) -> bool {
    matches!(t, "Fire" | "Cold" | "Lightning")
}

/// `math.modf(x)`: the integer part
pub(crate) fn modf(x: f64) -> f64 {
    if x.is_infinite() { x } else { x.trunc() }
}

/// `calcs.hitChance(evasion, accuracy)`
pub fn hit_chance(evasion: f64, accuracy: f64) -> f64 {
    if accuracy < 0.0 {
        return 5.0;
    }
    let raw = accuracy / (accuracy + (evasion / 5.0).powf(0.9)) * 125.0;
    m_max(m_min(round(raw, 0.0), 100.0), 5.0)
}

/// `calcs.armourReductionF(armour, raw)`
pub fn armour_reduction_f(armour: f64, raw: f64) -> f64 {
    if armour == 0.0 && raw == 0.0 {
        return 0.0;
    }
    armour / (armour + raw * 5.0) * 100.0
}

/// `calcs.armourReduction(armour, raw)`
pub fn armour_reduction(armour: f64, raw: f64) -> f64 {
    round(armour_reduction_f(armour, raw), 0.0)
}

impl<'a> Perform<'a> {
    /// Names with the elemental one for elemental types (`isElemental[x]
    /// and name or nil`)
    fn with_elemental(&self, name: String, elem: &str, ele_name: &str) -> Vec<String> {
        let mut v = vec![name];
        if is_elemental(elem) {
            v.push(ele_name.to_string());
        }
        v
    }

    /// A Lua `x and true or nil` boolean output: set when true, absent when
    /// nil
    pub(crate) fn set_bool_out(&mut self, actor: usize, k: &str, v: Option<bool>) {
        match v {
            Some(b) => self.c.set_out_val(actor, k, OutVal::Bool(b)),
            None => self.c.clear_out(actor, k),
        }
    }

    /// `calcs.actionSpeedMod(actor)`
    pub fn action_speed_mod(&self, actor: usize) -> Result<f64> {
        let db = self.c.db(actor);
        let min_speed = self
            .c
            .max(db, None, &["MinimumActionSpeed"])?
            .unwrap_or(0.0);
        let max_reduction = self.c.max(db, None, &["MaximumActionSpeedReduction"])?;
        let mut m = 1.0
            + (m_max(
                -self.misc_data("TemporalChainsEffectCap"),
                self.c.inc(db, None, &["TemporalChainsActionSpeed"])?,
            ) + self.c.inc(db, None, &["ActionSpeed"])?)
                / 100.0;
        m = m_max(min_speed / 100.0, m);
        if let Some(r) = max_reduction {
            m = m_min((100.0 - r) / 100.0, m);
        }
        Ok(m)
    }

    /// `calcs.resistances(actor)`
    pub fn resistances(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        self.c.set_out(actor, "PhysicalResist", 0.0);
        for from in RESIST_TYPES {
            let mut max_res: Option<f64> = None;
            for to in RESIST_TYPES {
                let rate = self
                    .c
                    .base(db, None, &[&format!("{from}MaxResConvertTo{to}")])?
                    / 100.0;
                if rate != 0.0 {
                    if max_res.is_none() {
                        let mut m = 0.0;
                        for (v, md) in
                            self.c
                                .tabulate(db, Some(Base), None, &[&format!("{from}ResistMax")])?
                        {
                            if md.source.as_deref() != Some("Base") {
                                m += number(&v)?;
                            }
                        }
                        max_res = Some(m);
                    }
                    let m = max_res.unwrap_or(0.0);
                    if m != 0.0 {
                        self.c.add(
                            db,
                            src(
                                &format!("{to}ResistMax"),
                                Base,
                                m * rate,
                                &format!("{from} To {to} Max Resistance Conversion"),
                            ),
                        );
                    }
                }
            }
        }
        for from in RESIST_TYPES {
            let mut res: Option<f64> = None;
            for to in RESIST_TYPES {
                let rate = self
                    .c
                    .base(db, None, &[&format!("{from}ResConvertTo{to}")])?
                    / 100.0;
                if rate != 0.0 {
                    if res.is_none() {
                        let mut r = 0.0;
                        for (v, md) in
                            self.c
                                .tabulate(db, Some(Base), None, &[&format!("{from}Resist")])?
                        {
                            if md.source.as_deref() != Some("Base") {
                                r += number(&v)?;
                            }
                        }
                        res = Some(r);
                    }
                    let r = res.unwrap_or(0.0);
                    if r != 0.0 {
                        self.c.add(
                            db,
                            src(
                                &format!("{to}Resist"),
                                Base,
                                r * rate,
                                &format!("{from} To {to} Resistance Conversion"),
                            ),
                        );
                    }
                    for typ in [Inc, More] {
                        for (v, md) in
                            self.c
                                .tabulate(db, Some(typ), None, &[&format!("{from}Resist")])?
                        {
                            let m = crate::mods::Mod::new(
                                &format!("{to}Resist"),
                                typ,
                                number(&v)? * rate,
                                md.source.as_deref(),
                            );
                            self.c.add(db, m);
                        }
                    }
                }
            }
        }
        let cap = self.misc_data("MaxResistCap");
        let resist_max = |p: &Self, prefix: &str, elem: &str| -> Result<f64> {
            let name = format!("{prefix}{elem}ResistMax");
            Ok(match p.c.override_num(db, None, &[&name])? {
                Some(v) => v,
                None => {
                    let names =
                        p.with_elemental(name, elem, &format!("{prefix}ElementalResistMax"));
                    let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                    m_min(cap, p.c.base(db, None, &names)?)
                }
            })
        };
        if self
            .c
            .flag(db, None, &["ElementalResistMaxIsHighestResistMax"])?
        {
            let mut highest = 0.0;
            let mut highest_type = "";
            for elem in RESIST_TYPES {
                let m = resist_max(self, "", elem)?;
                if m > highest && is_elemental(elem) {
                    highest = m;
                    highest_type = elem;
                }
            }
            for elem in RESIST_TYPES {
                if is_elemental(elem) {
                    self.c.add(
                        db,
                        src(
                            &format!("{elem}ResistMax"),
                            ModType::Override,
                            highest,
                            &format!("{highest_type} Melding of the Flesh"),
                        ),
                    );
                }
            }
        }
        let floor_ = self.misc_data("ResistFloor");
        for elem in RESIST_TYPES {
            let min = floor_;
            let max = resist_max(self, "", elem)?;
            let mut total = self.c.override_num(db, None, &[&format!("{elem}Resist")])?;
            let totem_max = resist_max(self, "Totem", elem)?;
            let mut totem_total =
                self.c
                    .override_num(db, None, &[&format!("Totem{elem}Resist")])?;
            let mut dot_total = None;
            if total.is_none() {
                let names = self.with_elemental(format!("{elem}Resist"), elem, "ElementalResist");
                let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                let base = self.c.base(db, None, &names)?;
                let inc = m_max(self.c.calc_mod(db, None, &names)?, 0.0);
                total = Some(base * inc);
                let dot_cfg = Cfg {
                    flags: Some(self.mf("Dot")),
                    keyword_flags: Some(0),
                    ..Cfg::default()
                };
                let dot_base = self.c.base(db, Some(&dot_cfg), &names)?;
                dot_total = Some(dot_base * inc);
            }
            if totem_total.is_none() {
                let names =
                    self.with_elemental(format!("Totem{elem}Resist"), elem, "TotemElementalResist");
                let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                let base = self.c.base(db, None, &names)?;
                totem_total = Some(base * m_max(self.c.calc_mod(db, None, &names)?, 0.0));
            }
            let total = modf(total.unwrap_or(0.0));
            let dot_total = dot_total.map_or(total, modf);
            let totem_total = modf(totem_total.unwrap_or(0.0));
            let min = modf(min);
            let max = modf(max);
            let totem_max = modf(totem_max);
            let fin = m_max(m_min(total, max), min);
            let dot_fin = m_max(m_min(dot_total, max), min);
            let totem_fin = m_max(m_min(totem_total, totem_max), min);
            let set = |p: &mut Self, k: String, v: f64| p.c.set_out(actor, &k, v);
            set(self, format!("{elem}Resist"), fin);
            set(self, format!("{elem}ResistTotal"), total);
            set(
                self,
                format!("{elem}ResistOverCap"),
                m_max(0.0, total - max),
            );
            set(self, format!("{elem}ResistOver75"), m_max(0.0, fin - 75.0));
            set(self, format!("Missing{elem}Resist"), m_max(0.0, max - fin));
            set(self, format!("{elem}ResistOverTime"), dot_fin);
            set(self, format!("Totem{elem}Resist"), totem_fin);
            set(self, format!("Totem{elem}ResistTotal"), totem_total);
            set(
                self,
                format!("Totem{elem}ResistOverCap"),
                m_max(0.0, totem_total - totem_max),
            );
            set(
                self,
                format!("MissingTotem{elem}Resist"),
                m_max(0.0, totem_max - totem_fin),
            );
        }
        Ok(())
    }

    fn config_val(&self, k: &str) -> Option<&Val> {
        self.input.config.get(k)
    }

    /// `env.configInput.EHPUnluckyWorstOf`
    pub(crate) fn unlucky_worst_of(&self) -> Option<f64> {
        match self.config_val("EHPUnluckyWorstOf") {
            Some(Val::Number(n)) => Some(*n),
            _ => None,
        }
    }

    /// Lucky or unlucky rolls of a chance (block, suppression)
    fn lucky_chance(&self, db: usize, chance: f64, lucky: &str, unlucky: &str) -> Result<f64> {
        let mut rolls = 0.0;
        if self.env.mode.effective {
            if self.c.flag(db, None, &[lucky])? {
                rolls += 1.0;
            }
            if self.c.flag(db, None, &[unlucky])? {
                rolls -= 1.0;
            }
            if self.c.flag(db, None, &["ExtremeLuck"])? {
                rolls *= 2.0;
            }
        }
        if let Some(w) = self.unlucky_worst_of()
            && w != 1.0
        {
            rolls = -w / 2.0;
        }
        if rolls == 0.0 {
            return Ok(chance);
        }
        let p = chance / 100.0;
        Ok(if self.c.flag(db, None, &["Unexciting"])? {
            (3.0 * p.powf(2.0) - 2.0 * p.powf(3.0)) * 100.0
        } else if rolls > 0.0 {
            (1.0 - (1.0 - p).powf(rolls + 1.0)) * 100.0
        } else {
            p.powf(rolls.abs()) * chance
        })
    }

    /// `calcs.defence(env, actor)`
    pub fn defence(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let enemy = self.c.actors.actors[actor].enemy.ok_or("no enemy")?;
        let edb = self.c.db(enemy);
        let o = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        let misc = |p: &Self, k: &str| p.misc_data(k);

        let asm = self.action_speed_mod(actor)?;
        self.c.set_out(actor, "ActionSpeedMod", asm);
        self.resistances(actor)?;

        // Block
        let block_max = m_min(
            self.c.base(db, None, &["BlockChanceMax"])?,
            misc(self, "BlockChanceCap"),
        );
        self.c.set_out(actor, "BlockChanceMax", block_max);
        if self
            .c
            .flag(db, None, &["MaximumBlockAttackChanceIsEqualToParent"])?
        {
            let parent = self.c.actors.actors[actor].parent.ok_or("no parent")?;
            let v = self.c.out(parent, "BlockChanceMax").unwrap_or(f64::NAN);
            self.c.set_out(actor, "BlockChanceMax", v);
        } else if self
            .c
            .flag(db, None, &["MaximumBlockAttackChanceIsEqualToPartyMember"])?
        {
            return Err("not ported: block chance from party members".into());
        }
        let block_max = o(self, "BlockChanceMax");
        self.c.set_out(actor, "BlockChanceOverCap", 0.0);
        self.c.set_out(actor, "SpellBlockChanceOverCap", 0.0);
        let mut base_block = 0.0;
        for slot in ["Weapon 2", "Weapon 3"] {
            if let Some(ad) = self.item(actor, slot).and_then(|i| i.armour_data.as_ref()) {
                base_block +=
                    field_num(ad, "BlockChance").ok_or("an armour without block chance")?;
            }
        }
        self.c.set_out(actor, "ShieldBlockChance", base_block);
        let necro = self
            .keystones_added
            .iter()
            .any(|k| k == "Necromantic Aegis");
        if !necro && let Some(v) = self.c.override_num(db, None, &["ReplaceShieldBlock"])? {
            base_block = v;
        }
        if self
            .c
            .flag(db, None, &["BlockAttackChanceIsEqualToParent"])?
        {
            let parent = self.c.actors.actors[actor].parent.ok_or("no parent")?;
            let v = self.c.out(parent, "BlockChance").unwrap_or(f64::NAN);
            self.c.set_out(actor, "BlockChance", m_min(v, block_max));
        } else if self
            .c
            .flag(db, None, &["BlockAttackChanceIsEqualToPartyMember"])?
        {
            return Err("not ported: block chance from party members".into());
        } else if self.c.flag(db, None, &["MaxBlockIfNotBlockedRecently"])? {
            self.c.set_out(actor, "BlockChance", block_max);
        } else {
            let inc = self.c.inc(db, None, &["BlockChance"])?;
            let more = self.c.more(db, None, &["BlockChance"])?;
            let total = round(
                (base_block + self.c.base(db, None, &["BlockChance"])?)
                    * (1.0 + inc / 100.0)
                    * more,
                0.0,
            );
            self.c
                .set_out(actor, "BlockChance", m_min(total, block_max));
            self.c
                .set_out(actor, "BlockChanceOverCap", m_max(0.0, total - block_max));
        }
        let v = m_min(
            o(self, "BlockChance")
                + self.c.base(db, None, &["ProjectileBlockChance"])?
                    * self.c.calc_mod(db, None, &["BlockChance"])?,
            block_max,
        );
        self.c.set_out(actor, "ProjectileBlockChance", v);
        if self
            .c
            .flag(db, None, &["SpellBlockChanceMaxIsBlockChanceMax"])?
        {
            self.c.set_out(actor, "SpellBlockChanceMax", block_max);
        } else {
            let v = m_min(
                self.c.base(db, None, &["SpellBlockChanceMax"])?,
                misc(self, "BlockChanceCap"),
            );
            self.c.set_out(actor, "SpellBlockChanceMax", v);
        }
        let spell_max = o(self, "SpellBlockChanceMax");
        if self
            .c
            .flag(db, None, &["MaxSpellBlockIfNotBlockedRecently"])?
        {
            self.c.set_out(actor, "SpellBlockChance", spell_max);
            self.c
                .set_out(actor, "SpellProjectileBlockChance", spell_max);
        } else if self.c.flag(db, None, &["SpellBlockChanceIsBlockChance"])? {
            let (b, pb, oc) = (
                o(self, "BlockChance"),
                o(self, "ProjectileBlockChance"),
                o(self, "BlockChanceOverCap"),
            );
            self.c.set_out(actor, "SpellBlockChance", b);
            self.c.set_out(actor, "SpellProjectileBlockChance", pb);
            self.c.set_out(actor, "SpellBlockChanceOverCap", oc);
        } else {
            let inc = self.c.inc(db, None, &["BlockChance"])?;
            let more = self.c.more(db, None, &["BlockChance"])?;
            let total = round(
                self.c.base(db, None, &["SpellBlockChance"])? * (1.0 + inc / 100.0) * more,
                0.0,
            );
            self.c
                .set_out(actor, "SpellBlockChance", m_min(total, spell_max));
            self.c.set_out(
                actor,
                "SpellBlockChanceOverCap",
                m_max(0.0, total - spell_max),
            );
            let v = m_max(
                m_min(
                    o(self, "SpellBlockChance")
                        + self.c.base(db, None, &["ProjectileSpellBlockChance"])?
                            * self.c.calc_mod(db, None, &["SpellBlockChance"])?,
                    spell_max,
                ),
                0.0,
            );
            self.c.set_out(actor, "SpellProjectileBlockChance", v);
        }
        if self.c.flag(db, None, &["CannotBlockAttacks"])? {
            self.c.set_out(actor, "BlockChance", 0.0);
            self.c.set_out(actor, "ProjectileBlockChance", 0.0);
        }
        if self.c.flag(db, None, &["CannotBlockSpells"])? {
            self.c.set_out(actor, "SpellBlockChance", 0.0);
            self.c.set_out(actor, "SpellProjectileBlockChance", 0.0);
        }
        for bt in [
            "BlockChance",
            "ProjectileBlockChance",
            "SpellBlockChance",
            "SpellProjectileBlockChance",
        ] {
            let v = if self.env.mode.effective {
                m_max(
                    o(self, bt) - self.c.base(edb, None, &["reduceEnemyBlock"])?,
                    0.0,
                )
            } else {
                o(self, bt)
            };
            let v = self.lucky_chance(db, v, &format!("{bt}IsLucky"), &format!("{bt}IsUnlucky"))?;
            self.c.set_out(actor, &format!("Effective{bt}"), v);
        }
        let avg = (o(self, "EffectiveBlockChance")
            + o(self, "EffectiveProjectileBlockChance")
            + o(self, "EffectiveSpellBlockChance")
            + o(self, "EffectiveSpellProjectileBlockChance"))
            / 4.0;
        self.c.set_out(actor, "EffectiveAverageBlockChance", avg);
        let be = 100.0 - self.c.base(db, None, &["BlockEffect"])?;
        self.c.set_out(actor, "BlockEffect", be);
        if be != 0.0 {
            self.c
                .set_out_val(actor, "ShowBlockEffect", OutVal::Bool(true));
            self.c.set_out(actor, "DamageTakenOnBlock", 100.0 - be);
        }

        if self
            .c
            .flag(db, None, &["ArmourAppliesToEnergyShieldRecharge"])?
        {
            let mult = self
                .c
                .max(db, None, &["ImprovedArmourAppliesToEnergyShieldRecharge"])?
                .unwrap_or(100.0)
                / 100.0;
            for (_, m) in self.c.tabulate(
                db,
                Some(Inc),
                None,
                &["Armour", "ArmourAndEvasion", "Defences"],
            )? {
                let tags = converted_tags(&m, mult, false);
                let v = (number(&m.value)? * mult).floor();
                self.c.add(
                    db,
                    new_mod(
                        "EnergyShieldRecharge",
                        Inc,
                        v,
                        m.source.as_deref(),
                        m.flags,
                        m.keyword_flags,
                        tags,
                    ),
                );
            }
        }
        for (flag, name, out) in [
            (
                "ArmourIncreasedByUncappedFireRes",
                "Armour",
                "FireResistTotal",
            ),
            (
                "ArmourIncreasedByOvercappedFireRes",
                "Armour",
                "FireResistOverCap",
            ),
            (
                "EvasionRatingIncreasedByUncappedColdRes",
                "Evasion",
                "ColdResistTotal",
            ),
            (
                "EvasionRatingIncreasedByOvercappedColdRes",
                "Evasion",
                "ColdResistOverCap",
            ),
            (
                "EnergyShieldIncreasedByChanceToBlockSpellDamage",
                "EnergyShield",
                "SpellBlockChance",
            ),
            (
                "EnergyShieldIncreasedByChaosResistance",
                "EnergyShield",
                "ChaosResist",
            ),
        ] {
            if self.c.flag(db, None, &[flag])?
                && let Some((_, m)) = self
                    .c
                    .tabulate(db, Some(ModType::Flag), None, &[flag])?
                    .into_iter()
                    .next()
            {
                let v = o(self, out);
                self.c
                    .add(db, crate::mods::Mod::new(name, Inc, v, m.source.as_deref()));
            }
        }

        self.primary_defences(actor)?;

        let suppression = self.c.base(db, None, &["SpellSuppressionChance"])?;
        let total_suppression = self
            .c
            .override_num(db, None, &["SpellSuppressionChance"])?
            .unwrap_or(suppression);
        if self
            .c
            .flag(db, None, &["ConvertSpellSuppressionToSpellDodge"])?
        {
            self.c.add(
                db,
                src("SpellDodgeChance", Base, suppression / 2.0, "Acrobatics"),
            );
        }
        let cap = misc(self, "SuppressionChanceCap");
        self.c.set_out(
            actor,
            "SpellSuppressionChance",
            m_min(total_suppression, cap),
        );
        let v = m_max(
            misc(self, "SuppressionEffect") + self.c.base(db, None, &["SpellSuppressionEffect"])?,
            0.0,
        );
        self.c.set_out(actor, "SpellSuppressionEffect", v);
        let eff = if self.c.flag(edb, None, &["CannotBeSuppressed"])? {
            0.0
        } else {
            o(self, "SpellSuppressionChance")
        };
        let eff = self.lucky_chance(
            db,
            eff,
            "SpellSuppressionChanceIsLucky",
            "SpellSuppressionChanceIsUnlucky",
        )?;
        self.c
            .set_out(actor, "EffectiveSpellSuppressionChance", eff);
        self.c.set_out(
            actor,
            "SpellSuppressionChanceOverCap",
            m_max(0.0, total_suppression - cap),
        );

        // Dodge
        let attack_dodge = self.c.base(db, None, &["AttackDodgeChance"])?;
        let spell_dodge = self.c.base(db, None, &["SpellDodgeChance"])?;
        let attack_max = misc(self, "DodgeChanceCap");
        let spell_dodge_max = match self.c.override_num(db, None, &["SpellDodgeChanceMax"])? {
            Some(v) => v,
            None => self.c.base(db, None, &["SpellDodgeChanceMax"])?,
        };
        let reduce = self.c.base(edb, None, &["reduceEnemyDodge"])?;
        let cannot = self.c.flag(edb, None, &["CannotBeDodged"])?;
        self.c
            .set_out(actor, "AttackDodgeChance", m_min(attack_dodge, attack_max));
        self.c.set_out(
            actor,
            "EffectiveAttackDodgeChance",
            if cannot {
                0.0
            } else {
                m_min(m_max(attack_dodge - reduce, 0.0), attack_max)
            },
        );
        self.c.set_out(
            actor,
            "SpellDodgeChance",
            m_min(spell_dodge, spell_dodge_max),
        );
        self.c.set_out(
            actor,
            "EffectiveSpellDodgeChance",
            if cannot {
                0.0
            } else {
                m_min(m_max(spell_dodge - reduce, 0.0), spell_dodge_max)
            },
        );
        if self.env.mode.effective && self.c.flag(db, None, &["DodgeChanceIsUnlucky"])? {
            for k in ["EffectiveAttackDodgeChance", "EffectiveSpellDodgeChance"] {
                let v = o(self, k);
                self.c.set_out(actor, k, v / 100.0 * v);
            }
        }
        self.c.set_out(
            actor,
            "AttackDodgeChanceOverCap",
            m_max(0.0, attack_dodge - attack_max),
        );
        self.c.set_out(
            actor,
            "SpellDodgeChanceOverCap",
            m_max(0.0, spell_dodge - spell_dodge_max),
        );

        // Recovery modifiers
        let mut life_rate = 1.0;
        if !self.c.flag(db, None, &["CannotRecoverLifeOutsideLeech"])? {
            life_rate = self.c.calc_mod(db, None, &["LifeRecoveryRate"])?;
        }
        self.c.set_out(actor, "LifeRecoveryRateMod", life_rate);
        let v = self.c.calc_mod(db, None, &["ManaRecoveryRate"])?;
        self.c.set_out(actor, "ManaRecoveryRateMod", v);
        let v = self.c.calc_mod(db, None, &["EnergyShieldRecoveryRate"])?;
        self.c.set_out(actor, "EnergyShieldRecoveryRateMod", v);

        // Leech caps
        let life = o(self, "Life");
        let v = life * self.c.val(db, "MaxLifeLeechInstance", None)? / 100.0;
        self.c.set_out(actor, "MaxLifeLeechInstance", v);
        let v = self.c.val(db, "MaxLifeLeechRate", None)?;
        self.c.set_out(actor, "MaxLifeLeechRatePercent", v);
        if self
            .c
            .flag(db, None, &["MaximumLifeLeechIsEqualToParent"])?
        {
            let parent = self.c.actors.actors[actor].parent.ok_or("no parent")?;
            let v = self
                .c
                .out(parent, "MaxLifeLeechRatePercent")
                .unwrap_or(f64::NAN);
            self.c.set_out(actor, "MaxLifeLeechRatePercent", v);
        } else if self
            .c
            .flag(db, None, &["MaximumLifeLeechIsEqualToPartyMember"])?
        {
            return Err("not ported: leech from party members".into());
        }
        let v = life * o(self, "MaxLifeLeechRatePercent") / 100.0;
        self.c.set_out(actor, "MaxLifeLeechRate", v);
        let es = o(self, "EnergyShield");
        let v = es * self.c.val(db, "MaxEnergyShieldLeechInstance", None)? / 100.0;
        self.c.set_out(actor, "MaxEnergyShieldLeechInstance", v);
        let v = es * self.c.val(db, "MaxEnergyShieldLeechRate", None)? / 100.0;
        self.c.set_out(actor, "MaxEnergyShieldLeechRate", v);
        let mana = o(self, "Mana");
        let v = mana * self.c.val(db, "MaxManaLeechInstance", None)? / 100.0;
        self.c.set_out(actor, "MaxManaLeechInstance", v);
        let v = mana * self.c.val(db, "MaxManaLeechRate", None)? / 100.0;
        self.c.set_out(actor, "MaxManaLeechRate", v);

        self.regeneration(actor)?;

        // Energy Shield recharge
        let to_life = if self
            .c
            .flag(db, None, &["EnergyShieldRechargeAppliesToLife"])?
        {
            Some(!self.c.flag(db, None, &["CannotRecoverLifeOutsideLeech"])?)
        } else {
            None
        };
        self.set_bool_out(actor, "EnergyShieldRechargeAppliesToLife", to_life);
        let to_es = !(self.c.flag(db, None, &["NoEnergyShieldRecharge"])?
            || self.c.flag(db, None, &["CannotGainEnergyShield"])?
            || to_life == Some(true));
        self.c.set_out_val(
            actor,
            "EnergyShieldRechargeAppliesToEnergyShield",
            OutVal::Bool(to_es),
        );
        if to_life == Some(true) || to_es {
            let inc = self.c.inc(db, None, &["EnergyShieldRecharge"])?;
            let more = self.c.more(db, None, &["EnergyShieldRecharge"])?;
            let base = self
                .c
                .override_num(db, None, &["EnergyShieldRecharge"])?
                .unwrap_or(misc(self, "EnergyShieldRechargeBase"));
            if to_life == Some(true) {
                let recharge = o(self, "Life") * base * (1.0 + inc / 100.0) * more;
                let v = round(recharge * o(self, "LifeRecoveryRateMod"), 0.0);
                self.c.set_out(actor, "LifeRecharge", v);
            } else {
                let recharge = o(self, "EnergyShield") * base * (1.0 + inc / 100.0) * more;
                let v = round(recharge * o(self, "EnergyShieldRecoveryRateMod"), 0.0);
                self.c.set_out(actor, "EnergyShieldRecharge", v);
            }
            let v = misc(self, "EnergyShieldRechargeDelay")
                / (1.0 + self.c.inc(db, None, &["EnergyShieldRechargeFaster"])? / 100.0);
            self.c.set_out(actor, "EnergyShieldRechargeDelay", v);
        } else {
            self.c.set_out(actor, "EnergyShieldRecharge", 0.0);
        }

        self.recoup(actor)?;

        let v = misc(self, "WardRechargeDelay")
            / (1.0 + self.c.inc(db, None, &["WardRechargeFaster"])? / 100.0);
        self.c.set_out(actor, "WardRechargeDelay", v);

        // Damage reduction
        let dr_max = self
            .c
            .override_num(db, None, &["DamageReductionMax"])?
            .unwrap_or(misc(self, "DamageReductionCap"));
        self.c.set_out(actor, "DamageReductionMax", dr_max);
        self.c.add(
            db,
            crate::mods::Mod::new("ArmourAppliesToPhysicalDamageTaken", Base, 100.0, None),
        );
        for t in DMG_TYPES {
            let over = self
                .c
                .override_num(db, None, &[&format!("{t}DamageReduction")])?;
            let names =
                self.with_elemental(format!("{t}DamageReduction"), t, "ElementalDamageReduction");
            let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            let base = match over {
                Some(v) => v,
                None => m_min(m_max(0.0, self.c.base(db, None, &names)?), dr_max),
            };
            self.c
                .set_out(actor, &format!("Base{t}DamageReduction"), base);
            let when_hit = match over {
                Some(v) => v,
                None => m_min(
                    m_max(
                        0.0,
                        base + self
                            .c
                            .base(db, None, &[&format!("{t}DamageReductionWhenHit")])?,
                    ),
                    dr_max,
                ),
            };
            self.c
                .set_out(actor, &format!("Base{t}DamageReductionWhenHit"), when_hit);
        }

        // Movement speed, avoidance
        let ms = match self.c.override_num(db, None, &["MovementSpeed"])? {
            Some(v) => v,
            None => {
                if self
                    .c
                    .flag(db, None, &["MovementSpeedEqualHighestLinkedPlayers"])?
                {
                    return Err("not ported: movement speed of party members".into());
                }
                self.c.calc_mod(db, None, &["MovementSpeed"])?
            }
        };
        let ms = if self.c.flag(db, None, &["MovementSpeedCannotBeBelowBase"])? {
            m_max(ms, 1.0)
        } else {
            ms
        };
        self.c.set_out(actor, "MovementSpeedMod", ms);
        let v = ms * o(self, "ActionSpeedMod");
        self.c.set_out(actor, "EffectiveMovementSpeedMod", v);
        if self.c.flag(edb, None, &["Blind"])? {
            let v = self
                .c
                .calc_mod(edb, None, &["BlindEffect", "BuffEffectOnSelf"])?
                * 100.0;
            self.c.set_out(actor, "BlindEffectMod", v);
        }
        self.c.set_out(actor, "LifeOnBlock", 0.0);
        self.c.set_out(actor, "LifeOnSuppress", 0.0);
        if !self.c.flag(db, None, &["CannotRecoverLifeOutsideLeech"])? {
            let v = self.c.base(db, None, &["LifeOnBlock"])?;
            self.c.set_out(actor, "LifeOnBlock", v);
            let v = self.c.base(db, None, &["LifeOnSuppress"])?;
            self.c.set_out(actor, "LifeOnSuppress", v);
        }
        for k in [
            "ManaOnBlock",
            "EnergyShieldOnBlock",
            "EnergyShieldOnSpellBlock",
            "EnergyShieldOnSuppress",
        ] {
            let v = self.c.base(db, None, &[k])?;
            self.c.set_out(actor, k, v);
        }
        let avoid_cap = misc(self, "AvoidChanceCap");
        let mut specific = false;
        for t in DMG_TYPES {
            let k = format!("Avoid{t}DamageChance");
            let v = m_min(self.c.base(db, None, &[&k])?, avoid_cap);
            self.c.set_out(actor, &k, v);
            if v > 0.0 {
                specific = true;
            }
        }
        self.c
            .set_out_val(actor, "specificTypeAvoidance", OutVal::Bool(specific));
        for k in ["AvoidProjectilesChance", "AvoidAllDamageFromHitsChance"] {
            let v = m_min(self.c.base(db, None, &[k])?, avoid_cap);
            self.c.set_out(actor, k, v);
        }
        let v = if self.c.flag(db, None, &["BlindImmune"])? {
            100.0
        } else {
            m_min(self.c.base(db, None, &["AvoidBlind"])?, 100.0)
        };
        self.c.set_out(actor, "BlindAvoidChance", v);
        let v = if self.c.flag(db, None, &["ImpaleImmune"])? {
            100.0
        } else {
            m_min(self.c.base(db, None, &["AvoidImpale"])?, 100.0)
        };
        self.c.set_out(actor, "ImpaleAvoidChance", v);
        for (k, f) in [
            ("CorruptedBloodImmunity", "CorruptedBloodImmune"),
            ("MaimImmunity", "MaimImmune"),
            ("HinderImmunity", "HinderImmune"),
            ("KnockbackImmunity", "KnockbackImmune"),
        ] {
            let v = self.c.flag(db, None, &[f])?.then_some(true);
            self.set_bool_out(actor, k, v);
        }
        if self
            .c
            .flag(db, None, &["SpellSuppressionAppliesToAilmentAvoidance"])?
        {
            let pct = self.c.base(
                db,
                None,
                &["SpellSuppressionAppliesToAilmentAvoidancePercent"],
            )? / 100.0;
            self.c.add(
                db,
                src(
                    "AvoidElementalAilments",
                    Base,
                    (pct * suppression).floor(),
                    "Ancestral Vision",
                ),
            );
        }
        if self.c.flag(
            db,
            None,
            &["SpellSuppressionAppliesToChanceToDefendWithArmour"],
        )? {
            let pct = self
                .c
                .max(
                    db,
                    None,
                    &["SpellSuppressionAppliesToChanceToDefendWithArmourPercent"],
                )?
                .unwrap_or(0.0);
            let armour = self
                .c
                .max(
                    db,
                    None,
                    &["SpellSuppressionAppliesToChanceToDefendWithArmourPercentArmour"],
                )?
                .unwrap_or(0.0);
            self.c.add(
                db,
                src(
                    "ArmourDefense",
                    MaxT,
                    armour - 100.0,
                    "Chance to Defend from Spell Suppression: Max Calc",
                )
                .tag(Tag::condition("ArmourMax")),
            );
            self.c.add(
                db,
                src(
                    "ArmourDefense",
                    MaxT,
                    m_min(pct * suppression / 100.0, 1.0) * (armour - 100.0),
                    "Chance to Defend from Spell Suppression: Average Calc",
                )
                .tag(Tag::condition("ArmourAvg")),
            );
            self.c.add(
                db,
                src(
                    "ArmourDefense",
                    MaxT,
                    m_min((pct * suppression / 100.0).floor(), 1.0) * (armour - 100.0),
                    "Chance to Defend from Spell Suppression: Min Calc",
                )
                .tag(Tag::condition("ArmourMax").negated())
                .tag(Tag::condition("ArmourAvg").negated()),
            );
        }
        let ad = self.c.max(db, None, &["ArmourDefense"])?.unwrap_or(0.0) / 100.0;
        self.c.set_out(actor, "ArmourDefense", ad);
        if ad > 0.0 {
            self.c
                .set_out(actor, "RawArmourDefense", (1.0 + ad) * 100.0);
        } else {
            self.c.clear_out(actor, "RawArmourDefense");
        }
        if self
            .c
            .flag(db, None, &["ShockAvoidAppliesToElementalAilments"])?
        {
            let base = self.c.base(db, None, &["AvoidShock"])?;
            if base != 0.0 {
                self.c.add(
                    db,
                    src(
                        "AvoidShockAppliesToElementalAilments",
                        Base,
                        base,
                        "Stormshroud",
                    ),
                );
            }
        }
        let data = self.data();
        let non_elemental = data.non_elemental_ailment_types.clone();
        let elemental = data.elemental_ailment_types.clone();
        let all = data.ailment_types.clone();
        for a in &non_elemental {
            let v = if self.c.flag(db, None, &[&format!("{a}Immune")])? {
                100.0
            } else {
                m_min(
                    self.c
                        .base(db, None, &[&format!("Avoid{a}"), "AvoidAilments"])?,
                    100.0,
                )
                .floor()
            };
            self.c.set_out(actor, &format!("{a}AvoidChance"), v);
        }
        for a in &elemental {
            let shock_all = self
                .c
                .flag(db, None, &["ShockAvoidAppliesToElementalAilments"])?
                && a != "Shock";
            let v = if self
                .c
                .flag(db, None, &[&format!("{a}Immune"), "ElementalAilmentImmune"])?
            {
                100.0
            } else {
                m_min(
                    self.c.base(
                        db,
                        None,
                        &[
                            &format!("Avoid{a}"),
                            "AvoidAilments",
                            "AvoidElementalAilments",
                        ],
                    )? + if shock_all {
                        self.c.base(db, None, &["AvoidShock"])?
                    } else {
                        0.0
                    },
                    100.0,
                )
                .floor()
            };
            self.c.set_out(actor, &format!("{a}AvoidChance"), v);
        }
        let curse_avoid = if self.c.flag(db, None, &["CurseImmune"])? {
            100.0
        } else {
            m_min(self.c.base(db, None, &["AvoidCurse"])?, 100.0)
        };
        self.c.set_out(actor, "CurseAvoidChance", curse_avoid);
        let v = if self.c.flag(db, None, &["SilenceImmune"])? {
            100.0
        } else {
            curse_avoid
        };
        self.c.set_out(actor, "SilenceAvoidChance", v);
        let v = m_min(self.c.base(db, None, &["ReduceCritExtraDamage"])?, 100.0);
        self.c.set_out(actor, "CritExtraDamageReduction", v);
        let v = self.c.calc_mod(db, None, &["LightRadius"])?;
        self.c.set_out(actor, "LightRadiusMod", v);
        let v = m_max(self.c.inc(db, None, &["LightRadius"])?, 0.0);
        self.c.set_out(actor, "LightRadiusInc", v);
        let v = m_max(
            self.c.more(db, None, &["CurseEffectOnSelf"])?
                * (100.0 + self.c.inc(db, None, &["CurseEffectOnSelf"])?),
            0.0,
        );
        self.c.set_out(actor, "CurseEffectOnSelf", v);
        for k in ["ExposureEffectOnSelf", "WitherEffectOnSelf"] {
            let v = self.c.more(db, None, &[k])? * (100.0 + self.c.inc(db, None, &[k])?);
            self.c.set_out(actor, k, v);
        }
        let rate = self.c.base(db, None, &["SelfDebuffExpirationRate"])?;
        self.c.set_out(actor, "DebuffExpirationRate", rate);
        let modifier = 10000.0 / (100.0 + rate);
        self.c.set_out(actor, "DebuffExpirationModifier", modifier);
        self.c.set_out_val(
            actor,
            "showDebuffExpirationModifier",
            OutVal::Bool(modifier != 100.0),
        );
        let v = self.c.more(db, None, &["SelfBlindDuration"])?
            * (100.0 + self.c.inc(db, None, &["SelfBlindDuration"])?)
            * modifier
            / 100.0;
        self.c.set_out(actor, "SelfBlindDuration", v);
        if self
            .c
            .flag(db, None, &["IgniteDurationAppliesToElementalAilments"])?
        {
            let inc = self.c.inc(db, None, &["SelfIgniteDuration"])?;
            let more = self.c.more(db, None, &["SelfIgniteDuration"])?;
            if inc != 0.0 {
                self.c.add(
                    db,
                    src(
                        "SelfIgniteDurationToElementalAilments",
                        Inc,
                        inc,
                        "Firesong",
                    ),
                );
            }
            if more != 1.0 {
                self.c.add(
                    db,
                    src(
                        "SelfIgniteDurationToElementalAilments",
                        More,
                        more,
                        "Firesong",
                    ),
                );
            }
        }
        for a in &non_elemental {
            let d = format!("Self{a}Duration");
            let more = self.c.more(db, None, &[&d, "SelfAilmentDuration"])?;
            let inc = (100.0 + self.c.inc(db, None, &[&d, "SelfAilmentDuration"])?) * 100.0;
            let v = inc * more
                / (100.0
                    + rate
                    + self
                        .c
                        .base(db, None, &[&format!("Self{a}DebuffExpirationRate")])?);
            self.c.set_out(actor, &d, v);
        }
        for a in &elemental {
            let ignite_all =
                self.c
                    .flag(db, None, &["IgniteDurationAppliesToElementalAilments"])?
                    && a != "Ignite";
            let d = format!("Self{a}Duration");
            let names = [
                d.as_str(),
                "SelfAilmentDuration",
                "SelfElementalAilmentDuration",
            ];
            let more = self.c.more(db, None, &names)?
                * if ignite_all {
                    self.c.more(db, None, &["SelfIgniteDuration"])?
                } else {
                    1.0
                };
            let inc = (100.0
                + self.c.inc(db, None, &names)?
                + if ignite_all {
                    self.c.inc(db, None, &["SelfIgniteDuration"])?
                } else {
                    0.0
                })
                * 100.0;
            let v = more * inc
                / (100.0
                    + rate
                    + self
                        .c
                        .base(db, None, &[&format!("Self{a}DebuffExpirationRate")])?);
            self.c.set_out(actor, &d, v);
        }
        for a in &all {
            let self_mod = self.c.calc_mod(db, None, &[&format!("Self{a}Effect")])?;
            let enemy_mod = if self.c.flag(db, None, &[&format!("Condition:{a}edSelf")])? {
                self.c.calc_mod(db, None, &[&format!("Enemy{a}Effect")])?
            } else {
                self.c.calc_mod(edb, None, &[&format!("Enemy{a}Effect")])?
            };
            self.c.set_out(
                actor,
                &format!("Self{a}Effect"),
                self_mod * enemy_mod * 100.0,
            );
        }
        let _ = ENEMY;
        Ok(())
    }

    /// Energy shield, armour, evasion and ward, and the evade chances
    fn primary_defences(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let enemy = self.c.actors.actors[actor].enemy.ok_or("no enemy")?;
        let edb = self.c.db(enemy);
        let o = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        let iron_reflexes = self.c.flag(db, None, &["IronReflexes"])?;
        let (mut ward, mut es, mut armour, mut evasion) = (0.0, 0.0, 0.0, 0.0);
        let (mut gear_ward, mut gear_es, mut gear_armour, mut gear_evasion) = (0.0, 0.0, 0.0, 0.0);
        let es_to_ward = self.c.flag(db, None, &["EnergyShieldToWard"])?;
        for slot in [
            "Helmet",
            "Gloves",
            "Boots",
            "Body Armour",
            "Weapon 2",
            "Weapon 3",
        ] {
            let Some(ad) = self.item(actor, slot).and_then(|i| i.armour_data.as_ref()) else {
                continue;
            };
            let slot_cfg = Cfg {
                slot_name: Some(crate::mods::Sym::new(slot)),
                ..Cfg::default()
            };
            let sc = Some(&slot_cfg);
            let no = |p: &Self, d: &str| p.c.flag(db, None, &[&format!("GainNo{d}From{slot}")]);
            let es_base = if !no(self, "EnergyShield")? {
                field_num(ad, "EnergyShield").unwrap_or(0.0)
            } else {
                0.0
            };
            let mut armour_base = if !no(self, "Armour")? {
                field_num(ad, "Armour").unwrap_or(0.0)
            } else {
                0.0
            };
            let mut evasion_base =
                if !(no(self, "Evasion")? || (no(self, "Armour")? && iron_reflexes)) {
                    field_num(ad, "Evasion").unwrap_or(0.0)
                } else {
                    0.0
                };
            let mut ward_base = if !no(self, "Ward")? {
                field_num(ad, "Ward").unwrap_or(0.0)
            } else {
                0.0
            };
            if slot == "Body Armour"
                && self
                    .c
                    .flag(db, None, &["ConvertBodyArmourArmourEvasionToWard"])?
            {
                let conv = m_min(
                    self.c
                        .base(db, None, &["BodyArmourArmourEvasionToWardPercent"])?
                        / 100.0,
                    1.0,
                );
                let ca = armour_base * conv;
                let ce = evasion_base * conv;
                armour_base -= ca;
                evasion_base -= ce;
                ward_base += ce + ca;
            }
            let es_armour = format!("{slot}ESAndArmour");
            if ward_base > 0.0 {
                if es_to_ward {
                    let inc = self.c.inc(db, sc, &["Ward", "Defences", "EnergyShield"])?;
                    let more = self.c.more(db, sc, &["Ward", "Defences"])?;
                    ward += ward_base * (1.0 + inc / 100.0) * more;
                } else {
                    ward += ward_base * self.c.calc_mod(db, sc, &["Ward", "Defences"])?;
                }
                gear_ward += ward_base;
            }
            if es_base > 0.0 {
                if es_to_ward {
                    es += es_base * self.c.more(db, sc, &["EnergyShield", "Defences"])?;
                    gear_es += es_base;
                } else if !self.c.flag(db, None, &["ConvertArmourESToLife"])? {
                    es += es_base
                        * self
                            .c
                            .calc_mod(db, sc, &["EnergyShield", "Defences", &es_armour])?;
                    gear_es += es_base;
                }
            }
            if armour_base > 0.0 {
                armour += armour_base
                    * self.c.calc_mod(
                        db,
                        sc,
                        &["Armour", "ArmourAndEvasion", "Defences", &es_armour],
                    )?;
                gear_armour += armour_base;
            }
            if evasion_base > 0.0 {
                gear_evasion += evasion_base;
                if iron_reflexes {
                    armour += evasion_base
                        * self.c.calc_mod(
                            db,
                            sc,
                            &["Armour", "Evasion", "ArmourAndEvasion", "Defences"],
                        )?;
                } else {
                    evasion += evasion_base
                        * self
                            .c
                            .calc_mod(db, sc, &["Evasion", "ArmourAndEvasion", "Defences"])?;
                }
            }
        }
        let ward_base = self.c.base(db, None, &["Ward"])?;
        if ward_base > 0.0 {
            if es_to_ward {
                let inc = self
                    .c
                    .inc(db, None, &["Ward", "Defences", "EnergyShield"])?;
                let more = self.c.more(db, None, &["Ward", "Defences"])?;
                ward += ward_base * (1.0 + inc / 100.0) * more;
            } else {
                ward += ward_base * self.c.calc_mod(db, None, &["Ward", "Defences"])?;
            }
        }
        let es_base = self.c.base(db, None, &["EnergyShield"])?;
        if es_base > 0.0 {
            if es_to_ward {
                es += es_base * self.c.more(db, None, &["EnergyShield", "Defences"])?;
            } else {
                es += es_base * self.c.calc_mod(db, None, &["EnergyShield", "Defences"])?;
            }
        }
        let armour_base = self.c.base(db, None, &["Armour", "ArmourAndEvasion"])?;
        if armour_base > 0.0 {
            armour += armour_base
                * self
                    .c
                    .calc_mod(db, None, &["Armour", "ArmourAndEvasion", "Defences"])?;
        }
        let evasion_base = self.c.base(db, None, &["Evasion", "ArmourAndEvasion"])?;
        if evasion_base > 0.0 {
            if iron_reflexes {
                armour += evasion_base
                    * self.c.calc_mod(
                        db,
                        None,
                        &["Armour", "Evasion", "ArmourAndEvasion", "Defences"],
                    )?;
            } else {
                evasion += evasion_base
                    * self
                        .c
                        .calc_mod(db, None, &["Evasion", "ArmourAndEvasion", "Defences"])?;
            }
        }
        let conv = self.c.base(db, None, &["ManaConvertToArmour"])?;
        if conv > 0.0 {
            let base = 2.0 * self.c.base(db, None, &["Mana"])? * conv / 100.0;
            armour += base
                * self.c.calc_mod(
                    db,
                    None,
                    &["Mana", "Armour", "ArmourAndEvasion", "Defences"],
                )?;
        }
        let conv = self.c.base(db, None, &["ManaGainAsEnergyShield"])?;
        if conv > 0.0 {
            let base = self.c.base(db, None, &["Mana"])? * conv / 100.0;
            es += base
                * self
                    .c
                    .calc_mod(db, None, &["Mana", "EnergyShield", "Defences"])?;
        }
        let ci = self.c.flag(db, None, &["ChaosInoculation"])?;
        let conv = self.c.base(db, None, &["LifeGainAsArmour"])?;
        if conv > 0.0 {
            let base = self.c.base(db, None, &["Life"])? * conv / 100.0;
            armour += if ci {
                1.0
            } else {
                base * self.c.calc_mod(
                    db,
                    None,
                    &["Life", "Armour", "ArmourAndEvasion", "Defences"],
                )?
            };
        }
        let conv = self.c.base(
            db,
            None,
            &["LifeConvertToEnergyShield", "LifeGainAsEnergyShield"],
        )?;
        if conv > 0.0 {
            let base = self.c.base(db, None, &["Life"])? * conv / 100.0;
            es += if ci {
                1.0
            } else {
                base * self
                    .c
                    .calc_mod(db, None, &["Life", "EnergyShield", "Defences"])?
            };
        }
        let conv = self.c.base(db, None, &["EvasionGainAsArmour"])?;
        if conv > 0.0 {
            let base = (self.c.base(db, None, &["Evasion", "ArmourAndEvasion"])? + gear_evasion)
                * conv
                / 100.0;
            armour += base
                * self.c.calc_mod(
                    db,
                    None,
                    &["Evasion", "Armour", "ArmourAndEvasion", "Defences"],
                )?;
        }
        let es_out = self
            .c
            .override_num(db, None, &["EnergyShield"])?
            .unwrap_or_else(|| m_max(round(es, 0.0), 0.0));
        self.c.set_out(actor, "EnergyShield", es_out);
        self.c
            .set_out(actor, "Armour", m_max(round(armour, 0.0), 0.0));
        self.c
            .set_out(actor, "Evasion", m_max(round(evasion, 0.0), 0.0));
        let v = m_max(
            round(evasion * self.c.calc_mod(db, None, &["MeleeEvasion"])?, 0.0),
            0.0,
        );
        self.c.set_out(actor, "MeleeEvasion", v);
        let v = m_max(
            round(
                evasion * self.c.calc_mod(db, None, &["ProjectileEvasion"])?,
                0.0,
            ),
            0.0,
        );
        self.c.set_out(actor, "ProjectileEvasion", v);
        let v = m_min(o(self, "Armour"), o(self, "Evasion"));
        self.c.set_out(actor, "LowestOfArmourAndEvasion", v);
        self.c.set_out(actor, "Ward", m_max(ward.floor(), 0.0));
        self.c.set_out(actor, "Gear:Ward", gear_ward);
        self.c.set_out(actor, "Gear:EnergyShield", gear_es);
        self.c.set_out(actor, "Gear:Armour", gear_armour);
        self.c.set_out(actor, "Gear:Evasion", gear_evasion);
        let armour_cap = self.c.flag(db, None, &["ArmourESRecoveryCap"])?;
        let evasion_cap = self.c.flag(db, None, &["EvasionESRecoveryCap"])?;
        let (a, e, s) = (
            o(self, "Armour"),
            o(self, "Evasion"),
            o(self, "EnergyShield"),
        );
        let low_es = self.config_val("conditionLowEnergyShield").cloned();
        let capping = if (armour_cap && a < s) || (evasion_cap && e < s) {
            Some(true)
        } else {
            match &low_es {
                Some(Val::Bool(b)) => Some(*b),
                Some(_) => Some(true),
                None => None,
            }
        };
        self.set_bool_out(actor, "CappingES", capping);
        let low = low_es
            .as_ref()
            .is_some_and(|v| !matches!(v, Val::Bool(false)));
        let cap = if capping == Some(true) {
            let c = if armour_cap && evasion_cap {
                m_min(a, e)
            } else if armour_cap {
                a
            } else if evasion_cap {
                e
            } else {
                s
            };
            if low {
                m_min(s * self.misc_data("LowPoolThreshold"), c)
            } else {
                c
            }
        } else {
            s
        };
        self.c.set_out(actor, "EnergyShieldRecoveryCap", cap);

        if self.c.flag(db, None, &["CannotEvade"])?
            || self.c.flag(edb, None, &["CannotBeEvaded"])?
        {
            for k in ["EvadeChance", "MeleeEvadeChance", "ProjectileEvadeChance"] {
                self.c.set_out(actor, k, 0.0);
            }
        } else if self.c.flag(db, None, &["AlwaysEvade"])? {
            for k in ["EvadeChance", "MeleeEvadeChance", "ProjectileEvadeChance"] {
                self.c.set_out(actor, k, 100.0);
            }
        } else {
            let acc = round(self.c.val(edb, "Accuracy", None)?, 0.0);
            let evade_chance = self.c.base(db, None, &["EvadeChance"])?;
            let hit = self.c.calc_mod(edb, None, &["HitChance"])?;
            let mut stat = o(self, "Evasion");
            let mut melee = o(self, "MeleeEvasion");
            let mut proj = o(self, "ProjectileEvasion");
            if self.c.flag(db, None, &["EvadeChanceBasedOnWard"])? {
                let m = self
                    .c
                    .override_num(db, None, &["EvadeChanceBasedOnWardPercent"])?
                    .ok_or("attempt to perform arithmetic on a nil value")?
                    / 100.0;
                stat = o(self, "Ward") * m;
                melee = stat;
                proj = stat;
            }
            let cap = self.misc_data("EvadeChanceCap");
            self.c.set_out(
                actor,
                "EvadeChance",
                100.0 - (hit_chance(stat, acc) - evade_chance) * hit,
            );
            let v = m_max(
                0.0,
                m_min(
                    cap,
                    (100.0 - (hit_chance(melee, acc) - evade_chance) * hit)
                        * self
                            .c
                            .calc_mod(db, None, &["EvadeChance", "MeleeEvadeChance"])?,
                ),
            );
            self.c.set_out(actor, "MeleeEvadeChance", v);
            let v = m_max(
                0.0,
                m_min(
                    cap,
                    (100.0 - (hit_chance(proj, acc) - evade_chance) * hit)
                        * self
                            .c
                            .calc_mod(db, None, &["EvadeChance", "ProjectileEvadeChance"])?,
                ),
            );
            self.c.set_out(actor, "ProjectileEvadeChance", v);
            let (m, p) = (
                o(self, "MeleeEvadeChance"),
                o(self, "ProjectileEvadeChance"),
            );
            if m != p {
                self.c.set_out_val(actor, "splitEvade", OutVal::Bool(true));
            } else {
                self.c.set_out(actor, "EvadeChance", m);
                self.c
                    .set_out_val(actor, "noSplitEvade", OutVal::Bool(true));
            }
        }
        Ok(())
    }

    /// Regeneration and degeneration of each resource
    fn regeneration(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let resources = ["Mana", "Life", "Energy Shield", "Rage"];
        for (i, name) in resources.iter().enumerate() {
            let r = name.replace(' ', "");
            let pool = self.c.out(actor, &r).unwrap_or(0.0);
            let mut inc = self.c.inc(db, None, &[&format!("{r}Regen")])?;
            let more = self.c.more(db, None, &[&format!("{r}Regen")])?;
            let mut regen_rate = 0.0;
            let rate_mod = self
                .c
                .out(actor, &format!("{r}RecoveryRateMod"))
                .unwrap_or(1.0);
            if self.c.flag(db, None, &[&format!("No{r}Regen")])?
                || self.c.flag(db, None, &[&format!("CannotGain{r}")])?
            {
                self.c.set_out(actor, &format!("{r}Regen"), 0.0);
            } else if r == "Life" && self.c.flag(db, None, &["ZealotsOath"])? {
                self.c.set_out(actor, "LifeRegen", 0.0);
                let base = self.c.base(db, None, &["LifeRegen"])?;
                if base > 0.0 {
                    self.c
                        .add(db, src("EnergyShieldRegen", Base, base, "Zealot's Oath"));
                }
                let pct = self.c.base(db, None, &["LifeRegenPercent"])?;
                if pct > 0.0 {
                    self.c.add(
                        db,
                        src("EnergyShieldRegenPercent", Base, pct, "Zealot's Oath"),
                    );
                }
            } else {
                if inc != 0.0 {
                    for to in &resources[i + 1..] {
                        let t = to.replace(' ', "");
                        if self.c.flag(db, None, &[&format!("{r}RegenTo{t}Regen")])? {
                            self.c.add(
                                db,
                                src(
                                    &format!("{t}Regen"),
                                    Inc,
                                    inc,
                                    &format!("{name} instead applies to {to}"),
                                ),
                            );
                            inc = 0.0;
                        }
                    }
                }
                if r == "Life" && self.c.base(db, None, &["LifeRegenAppliesToEnergyShield"])? > 0.0
                {
                    let conv = m_min(
                        self.c.base(db, None, &["LifeRegenAppliesToEnergyShield"])?,
                        100.0,
                    ) / 100.0;
                    let base = self.c.base(db, None, &["LifeRegen"])?;
                    let pct = self.c.base(db, None, &["LifeRegenPercent"])?;
                    self.c.add(
                        db,
                        src(
                            "EnergyShieldRegen",
                            Base,
                            floor(base * conv, 2.0),
                            "Life Regen to ES Regen",
                        ),
                    );
                    self.c.add(
                        db,
                        src(
                            "EnergyShieldRegenPercent",
                            Base,
                            floor(pct * conv, 2.0),
                            "Life Regen to ES Regen",
                        ),
                    );
                }
                let base = self.c.base(db, None, &[&format!("{r}Regen")])?
                    + pool * self.c.base(db, None, &[&format!("{r}RegenPercent")])? / 100.0;
                let regen = base * (1.0 + inc / 100.0) * more;
                if regen != 0.0 {
                    for to in &resources[i + 1..] {
                        let t = to.replace(' ', "");
                        if self
                            .c
                            .flag(db, None, &[&format!("{r}RegenerationRecovers{t}")])?
                        {
                            self.c.add(
                                db,
                                src(
                                    &format!("{t}Recovery"),
                                    Base,
                                    regen,
                                    &format!("{name} Regeneration Recovers {to}"),
                                ),
                            );
                        }
                    }
                }
                regen_rate = round(regen * rate_mod, 1.0);
                self.c.set_out(actor, &format!("{r}Regen"), regen_rate);
            }
            self.c.set_out(actor, &format!("{r}RegenInc"), inc);
            let mut base_degen = self.c.base(db, None, &[&format!("{r}Degen")])?
                + pool * self.c.base(db, None, &[&format!("{r}DegenPercent")])? / 100.0;
            let tincture = self
                .c
                .base(db, None, &[&format!("{r}DegenPercentTincture")])?;
            base_degen += m_max(pool * tincture / 100.0, tincture);
            let degen_rate = if base_degen > 0.0 {
                base_degen * self.c.calc_mod(db, None, &[&format!("{r}Degen")])?
            } else {
                0.0
            };
            self.c.set_out(actor, &format!("{r}Degen"), degen_rate);
            let recovery = self.c.base(db, None, &[&format!("{r}Recovery")])? * rate_mod;
            self.c.set_out(actor, &format!("{r}Recovery"), recovery);
            let unaffected = self.c.flag(db, None, &[&format!("UnaffectedBy{r}Regen")])?;
            let rr = if unaffected { 0.0 } else { regen_rate } - degen_rate + recovery;
            self.c.set_out(actor, &format!("{r}RegenRecovery"), rr);
            if rr > 0.0 {
                self.c.add(
                    db,
                    src(
                        &format!("Condition:CanGain{r}"),
                        ModType::Flag,
                        true,
                        &format!("{name}Regen"),
                    ),
                );
            }
            let pct = if pool > 0.0 {
                round(rr / pool * 100.0, 1.0)
            } else {
                0.0
            };
            self.c.set_out(actor, &format!("{r}RegenPercent"), pct);
        }
        Ok(())
    }

    /// Recoup and pseudo recoup
    fn recoup(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let o = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        self.c.set_out(actor, "anyRecoup", 0.0);
        let types = ["Life", "Mana", "EnergyShield"];
        let es_instead = self
            .c
            .flag(db, None, &["EnergyShieldRecoupInsteadOfLife"])?;
        for t in types {
            let base = self.c.base(db, None, &[&format!("{t}Recoup")])?;
            if t == "Life" && es_instead {
                self.c.set_out(actor, "LifeRecoup", 0.0);
                let life = self.c.base(db, None, &["LifeRecoup"])?;
                self.c.add(
                    db,
                    src("EnergyShieldRecoup", Base, life, "Life Recoup Conversion"),
                );
            } else {
                let v = base * o(self, &format!("{t}RecoveryRateMod"));
                self.c.set_out(actor, &format!("{t}Recoup"), v);
                let any = o(self, "anyRecoup") + v;
                self.c.set_out(actor, "anyRecoup", any);
            }
            let flag = format!("Add{t}RecoupToEnergyShieldRecoup");
            if self.c.flag(db, None, &[&flag])? {
                let source = self
                    .c
                    .tabulate(db, Some(ModType::Flag), None, &[&flag])?
                    .first()
                    .and_then(|(_, m)| m.source.clone());
                self.c.replace_mod(
                    db,
                    crate::mods::Mod::new("EnergyShieldRecoup", Base, base, source.as_deref()),
                );
            }
        }
        if self.c.flag(db, None, &["UsePowerCharges"])?
            && self
                .c
                .flag(db, None, &["PowerChargesConvertToAbsorptionCharges"])?
        {
            let v = self
                .c
                .base(db, None, &["PerAbsorptionElementalEnergyShieldRecoup"])?;
            for e in ["Cold", "Fire", "Lightning"] {
                self.c.add(
                    db,
                    src(
                        &format!("{e}EnergyShieldRecoup"),
                        Base,
                        v,
                        "Absorption Charges",
                    )
                    .tag(Tag::multiplier("AbsorptionCharge")),
                );
            }
        }
        for t in types {
            for d in DMG_TYPES {
                let recoup = self.c.base(db, None, &[&format!("{d}{t}Recoup")])?;
                if t == "Life" && es_instead {
                    self.c.set_out(actor, &format!("{d}LifeRecoup"), 0.0);
                    self.c.add(
                        db,
                        src(
                            &format!("{d}EnergyShieldRecoup"),
                            Base,
                            recoup,
                            "Life Recoup Conversion",
                        ),
                    );
                } else {
                    let v = recoup * o(self, &format!("{t}RecoveryRateMod"));
                    self.c.set_out(actor, &format!("{d}{t}Recoup"), v);
                    let any = o(self, "anyRecoup") + v;
                    self.c.set_out(actor, "anyRecoup", any);
                }
                let flag = format!("Add{t}RecoupToEnergyShieldRecoup");
                if self.c.flag(db, None, &[&flag])? {
                    let source = self
                        .c
                        .tabulate(db, Some(ModType::Flag), None, &[&flag])?
                        .first()
                        .and_then(|(_, m)| m.source.clone());
                    self.c.replace_mod(
                        db,
                        crate::mods::Mod::new(
                            &format!("{d}EnergyShieldRecoup"),
                            Base,
                            recoup,
                            source.as_deref(),
                        ),
                    );
                }
            }
        }
        for r in types {
            if !self.c.flag(db, None, &[&format!("No{r}Regen")])?
                && !self.c.flag(db, None, &[&format!("CannotGain{r}")])?
            {
                let pseudo = self.c.base(
                    db,
                    None,
                    &[&format!("PhysicalDamageMitigated{r}PseudoRecoup")],
                )?;
                if pseudo > 0.0 {
                    let dk = format!("PhysicalDamageMitigated{r}PseudoRecoupDuration");
                    let mut dur = self.c.base(db, None, &[&dk])?;
                    if dur == 0.0 {
                        dur = 4.0;
                    }
                    self.c.set_out(actor, &dk, dur);
                    let inc = self.c.inc(db, None, &[&format!("{r}Regen")])?;
                    let more = self.c.more(db, None, &[&format!("{r}Regen")])?;
                    let v = pseudo
                        * (1.0 + inc / 100.0)
                        * more
                        * o(self, &format!("{r}RecoveryRateMod"));
                    self.c
                        .set_out(actor, &format!("PhysicalDamageMitigated{r}PseudoRecoup"), v);
                    let any = o(self, "anyRecoup") + v;
                    self.c.set_out(actor, "anyRecoup", any);
                }
            }
        }
        let _ = (PLAYER, ModValue::Bool(true));
        Ok(())
    }
}
