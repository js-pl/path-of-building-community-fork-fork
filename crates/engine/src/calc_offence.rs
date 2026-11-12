//! `calcs.offence` (`Modules/CalcOffence.lua`) on the typed calculation:
//! the active skill's area, projectiles, durations, costs, then its damage
//! per pass (main hand, off hand or the skill), speed, accuracy, crits,
//! leech, ailments and damage over time, and the combined DPS.
//!
//! Breakdowns are not built. Skill functions (`preDamageFunc`, ...) are
//! ported by hand per granted effect, see `calc_skill_funcs.rs`.
//!
//! In sync with upstream `16de4b82d57f1c0de6eb40f37143c32d4da36a02`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::OutVal;
use crate::calc::{floor, m_max, m_min, number, round};
use crate::calc_defence::{DMG_TYPES, is_elemental};
use crate::calc_perform::{Perform, converted_tags, new_mod};
use crate::item_tools::round_symmetric;
use crate::mods::{Cfg, Mod, ModType, ModValue, StoreId, Sym, Tag};
use crate::breakdown::{Breakdown, Chain, Col, Row, area, f, multi_chain};
use crate::breakdown::area as area_bd;
use crate::sf;
use std::collections::BTreeMap;

pub type Result<T> = std::result::Result<T, String>;

use ModType::{Base, Flag as FlagT, Inc, More};

/// One damage type's conversions to the later types, by type index
#[derive(Clone, Debug, Default)]
pub(crate) struct Conversion {
    pub conversion: [f64; 5],
    pub gain: [f64; 5],
    /// `conversionTable[type][other]`: converted and gained
    pub total: [f64; 5],
    /// The part not converted
    pub mult: f64,
}

pub(crate) type ConversionTable = [Conversion; 5];

pub(crate) fn dmg_index(t: &str) -> usize {
    DMG_TYPES
        .iter()
        .position(|x| *x == t)
        .expect("a damage type")
}

/// `calcAreaMod(incArea, moreArea)`: the client's integer rounding stages
fn calc_area_mod(inc: f64, more: f64) -> f64 {
    let base = round_symmetric(inc * 100.0, None);
    let fin = round_symmetric(m_max(0.0, more) * 100.0, None);
    let p = base * fin / 100.0;
    let p = if p >= 0.0 { p.floor() } else { p.ceil() };
    m_max(0.0, p) / 100.0
}

/// `calcRadius(baseRadius, areaMod, radiusMod)`
pub(crate) fn calc_radius(base: f64, area_mod: f64, radius_mod: f64) -> f64 {
    let pct = ((100.0 * area_mod.sqrt()).floor() * radius_mod).floor();
    (base * pct / 100.0).floor()
}

/// `calcMoltenStrikeTertiaryRadius`
fn molten_strike_tertiary_radius(base: f64, deadzone: f64, area_mod: f64, speed_mod: f64) -> f64 {
    let max_ignoring_speed =
        (base * base * area_mod - deadzone * deadzone * (area_mod - 1.0)).sqrt();
    ((max_ignoring_speed - deadzone) * speed_mod + deadzone).floor()
}

/// An output map: the actor's output or a pass's (`output.MainHand`)
pub(crate) type OutMap = std::collections::HashMap<Sym, OutVal>;

pub(crate) fn get_num(o: &OutMap, k: &str) -> Option<f64> {
    o.get(&Sym::new(k)).and_then(OutVal::num)
}

pub(crate) fn set_num(o: &mut OutMap, k: &str, v: f64) {
    o.insert(Sym::new(k), OutVal::Num(v));
}

/// What `calcs.offence` keeps across its sections
pub(crate) struct Off {
    pub actor: usize,
    /// The active skill
    pub skill: usize,
    /// `skillModList`
    pub list: StoreId,
    pub db: StoreId,
    pub edb: StoreId,
    /// `activeSkill.conversionTable`, `weapon1Cfg.conversionTable`, ...
    pub conv: [Option<ConversionTable>; 3],
    pub can_deal: [bool; 5],
    pub is_attack: bool,
    pub debuff_duration_mult: f64,
    /// The damage passes (`passList`)
    pub passes: Vec<Pass>,
    /// The pass being calculated
    pub pass: usize,
    /// `output.MainHand` and `output.OffHand`
    pub hands: [OutMap; 2],
    /// `quantityMultiplier`
    pub quantity_multiplier: f64,
    /// `monsterLife`
    pub monster_life: f64,
}

/// A damage pass: a weapon of an attack, or the skill
pub(crate) struct Pass {
    pub label: &'static str,
    /// The weapon (0 main hand, 1 off hand) whose cfg, output and copied
    /// data the pass uses; `None` for the skill's own (`skillData`,
    /// `skillCfg`, the actor's output)
    pub weapon: Option<usize>,
    pub source: crate::item::Fields,
}

impl<'a> Perform<'a> {
    pub(crate) fn sd_set(&mut self, i: usize, k: &str, v: impl Into<ModValue>) {
        self.mods_mut(i).skill_data.insert(k.to_string(), v.into());
    }

    pub(crate) fn sd_clear(&mut self, i: usize, k: &str) {
        self.mods_mut(i).skill_data.remove(k);
    }

    pub(crate) fn set_flag(&mut self, i: usize, f: &str, on: bool) {
        let flags = &mut self.mods_mut(i).skill_flags;
        if on {
            flags.insert(f.to_string());
        } else {
            flags.remove(f);
        }
    }

    /// `skillCfg.skillCond[var] = on`, which the weapon cfgs see through
    /// their own
    pub(crate) fn set_skill_cond(&mut self, i: usize, var: &str, on: bool) {
        let sym = Sym::new(var);
        let m = self.mods_mut(i);
        let set = |c: &mut Cfg| {
            let conds = c.skill_cond.get_or_insert_with(Default::default);
            if on {
                conds.insert(sym);
            } else {
                conds.remove(&sym);
            }
        };
        set(&mut m.skill_cfg);
        for c in m.weapon_cfgs.iter_mut().flatten() {
            set(c);
        }
    }

    /// `activeSkill.activeEffect.grantedEffectLevel`
    pub(crate) fn effect_level(&self, i: usize) -> Option<&crate::skill_data::SkillLevel> {
        let e = &self.skill(i).active_effect;
        e.granted_effect.level(e.level?)
    }

    pub(crate) fn weapon_num(&self, actor: usize, n: usize, k: &str) -> Option<f64> {
        self.weapon(actor, n)?.get(k)?.as_number()
    }

    /// Whether the slot holds a weapon (`itemList[slot].weaponData[1]`)
    fn slot_has_weapon(&self, actor: usize, slot: &str) -> bool {
        self.item(actor, slot)
            .and_then(|it| it.weapon_data.as_ref())
            .is_some_and(|w| w.contains_key(&1))
    }

    /// `calcs.offence(env, actor, activeSkill)`
    pub fn offence(&mut self, actor: usize, i: usize) -> Result<()> {
        let list = self.lists[i].list;
        let mut o = Off {
            actor,
            skill: i,
            list,
            db: self.c.db(actor),
            edb: self.c.db(crate::calc::ENEMY),
            conv: [None, None, None],
            can_deal: [true; 5],
            is_attack: false,
            debuff_duration_mult: 1.0,
            passes: Vec::new(),
            pass: 0,
            hands: Default::default(),
            quantity_multiplier: 1.0,
            monster_life: 100.0,
        };
        if self.sd_truthy(i, "showAverage") {
            self.set_flag(i, "showAverage", true);
        } else {
            self.set_flag(i, "notAverage", true);
        }
        if self.has_flag(i, "disable") {
            self.c.set_out(actor, "CombinedDPS", 0.0);
            return Ok(());
        }
        self.offence_setup(&mut o)?;
        self.offence_skill_types(&mut o)?;
        self.offence_durations(&mut o)?;
        self.offence_costs(&mut o)?;
        self.offence_pre_damage(&mut o)?;
        let result = self.offence_damage(&mut o);
        if o.is_attack {
            for (n, k) in ["MainHand", "OffHand"].into_iter().enumerate() {
                let t: BTreeMap<String, OutVal> = std::mem::take(&mut o.hands[n])
                    .into_iter()
                    .map(|(k, v)| (k.as_str().to_string(), v))
                    .collect();
                self.c
                    .set_out_val(actor, k, OutVal::Table(std::rc::Rc::new(t)));
            }
        }
        result
    }

    /// `breakdown[k] = breakdown.mod(list, cfg, ...)`
    pub(crate) fn bd_mod(
        &mut self,
        actor: usize,
        k: &str,
        list: StoreId,
        cfg: Option<&Cfg>,
        names: &[&str],
    ) -> Result<()> {
        if self.has_bd(actor) {
            let l = crate::breakdown::mod_(self.c.inc(list, cfg, names)?, self.c.more(list, cfg, names)?);
            self.bd(actor).unwrap().set_opt(k, l);
        }
        Ok(())
    }

    /// `runSkillFunc(name)`
    pub(crate) fn run_skill_func(&mut self, o: &mut Off, name: &str) -> Result<()> {
        let ge = self.skill(o.skill).active_effect.granted_effect.clone();
        if ge.funcs.iter().any(|f| f == name) {
            return self.skill_func(o, &ge.id, name);
        }
        Ok(())
    }

    /// `calcResistForType(damageType, cfg)`
    pub(crate) fn resist_for_type(&self, o: &Off, t: &str, cfg: Option<&Cfg>) -> Result<f64> {
        let edb = o.edb;
        let resist_name = format!("{t}Resist");
        let resist = self.c.override_num(edb, cfg, &[&resist_name])?;
        let max_resist = if self.c.flag(edb, None, &["DoNotChangeMaxResFromConfig"])? {
            self.misc_data("EnemyMaxResist")
        } else {
            m_min(
                m_max(
                    self.config_num(&format!("enemy{t}Resist"))
                        .unwrap_or(self.misc_data("EnemyMaxResist")),
                    self.misc_data("EnemyMaxResist"),
                ),
                self.misc_data("MaxResistCap"),
            )
        };
        let resist = match resist {
            Some(r) => r,
            None => {
                if self
                    .c
                    .flag(self.pdb(), None, &[&format!("Enemy{t}ResistEqualToYours")])?
                {
                    self.c
                        .out(crate::calc::PLAYER, &resist_name)
                        .unwrap_or(f64::NAN)
                } else {
                    let mut names = vec![resist_name.as_str()];
                    if is_elemental(t) {
                        names.push("ElementalResist");
                    }
                    self.c.base(edb, cfg, &names)? * m_max(self.c.calc_mod(edb, cfg, &names)?, 0.0)
                }
            }
        };
        Ok(m_max(
            m_min(resist, max_resist),
            self.misc_data("ResistFloor"),
        ))
    }

    /// Adds `mod` with another name, type or value and the same flags and
    /// tags
    fn copy_mod(name: &str, typ: ModType, value: f64, m: &Mod, flags: u32) -> Mod {
        new_mod(
            name,
            typ,
            value,
            m.source.as_deref(),
            flags,
            m.keyword_flags,
            m.tags.clone(),
        )
    }

    /// From `runSkillFunc("initialFunc")` to the momentum stacks
    fn offence_setup(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, db) = (o.skill, o.list, o.actor, o.db);
        self.run_skill_func(o, "initialFunc")?;
        let triggered = self.sd_truthy(i, "triggered");
        self.set_skill_cond(i, "SkillIsTriggered", triggered);
        if triggered {
            self.set_flag(i, "triggered", true);
        }
        let focused = self.sd_truthy(i, "chanceToTriggerOnFocus");
        self.set_skill_cond(i, "SkillIsFocused", focused);
        if focused {
            self.set_flag(i, "focused", true);
        }
        let cfg = self.cfg(i);
        // Update skill data
        for v in self.c.list(list, Some(&cfg), &["SkillData"])? {
            let ModValue::Record(r) = &v else { continue };
            let Some(ModValue::Str(k)) = r.get("key") else {
                continue;
            };
            let value = r.get("value").cloned();
            if matches!(r.get("merge"), Some(ModValue::Str(m)) if &**m == "MAX") {
                let cur = self.sd_num(i, k).unwrap_or(0.0);
                let v = number(value.as_ref().unwrap_or(&ModValue::Bool(false)))?;
                self.sd_set(i, k, m_max(v, cur));
            } else {
                match value {
                    Some(v) => self.sd_set(i, k, v),
                    None => self.sd_clear(i, k),
                }
            }
        }
        let cfg = self.cfg(i);
        let spell = self.mf("Spell");
        let attack = self.mf("Attack");
        // Add addition stat bonuses
        let str_bonus = self.str_dmg_bonus.unwrap_or(0.0);
        if self.c.flag(list, None, &["IronGrip"])? {
            let flags = attack | self.mf("Projectile");
            self.c.add(
                list,
                new_mod(
                    "PhysicalDamage",
                    Inc,
                    str_bonus,
                    Some("Strength"),
                    flags,
                    0,
                    vec![],
                ),
            );
        }
        if self.c.flag(list, None, &["IronWill"])? {
            self.c.add(
                list,
                new_mod("Damage", Inc, str_bonus, Some("Strength"), spell, 0, vec![]),
            );
        }
        for (flag, stat, source, flags) in [
            (
                "TransfigurationOfBody",
                "Life",
                "Transfiguration of Body",
                attack,
            ),
            (
                "TransfigurationOfMind",
                "Mana",
                "Transfiguration of Mind",
                0,
            ),
            (
                "TransfigurationOfSoul",
                "EnergyShield",
                "Transfiguration of Soul",
                spell,
            ),
        ] {
            if self.c.flag(list, None, &[flag])? {
                let v =
                    (self.c.inc(list, None, &[stat])? * self.misc_data("Transfiguration")).floor();
                self.c.add(
                    list,
                    new_mod("Damage", Inc, v, Some(source), flags, 0, vec![]),
                );
            }
        }
        if self.c.flag(db, None, &["Elusive"])?
            && self.c.flag(list, None, &["SupportedByNightblade"])?
        {
            let effect = self.c.out(actor, "ElusiveEffectMod").unwrap_or(f64::NAN) / 100.0;
            let multi = self
                .c
                .base(list, None, &["NightbladeElusiveCritMultiplier"])?;
            self.c.add(
                list,
                Mod::new(
                    "CritMultiplier",
                    Base,
                    (multi * effect).floor(),
                    Some("Nightblade"),
                ),
            );
        }
        // set other limits
        let v = self.c.base(list, Some(&cfg), &["ActiveTrapLimit"])?;
        self.c.set_out(actor, "ActiveTrapLimit", v);
        let v = self.c.base(list, Some(&cfg), &["ActiveMineLimit"])?;
        self.c.set_out(actor, "ActiveMineLimit", v);
        // set flask scaling
        for k in ["LifeFlaskRecovery", "LifeFlaskCharges"] {
            let v = self
                .env
                .setup
                .items
                .item_db
                .multipliers
                .get(&Sym::new(k))
                .copied();
            match v {
                Some(v) => self.c.set_out(actor, k, v),
                None => self.c.clear_out(actor, k),
            }
        }
        if self.c.cond(db, "AffectedByEnergyBlade") {
            let dmg_mod = self.c.calc_mod(list, Some(&cfg), &["EnergyBladeDamage"])?;
            let speed_mod = self
                .c
                .calc_mod(list, Some(&cfg), &["EnergyBladeAttackSpeed"])?;
            for (n, slot) in ["Weapon 1", "Weapon 2"].into_iter().enumerate() {
                if !self.slot_has_weapon(actor, slot) {
                    continue;
                }
                let Some(base) = self
                    .weapon_str(actor, n, "name")
                    .and_then(|name| self.input.data.items.base(&name).cloned())
                else {
                    continue;
                };
                let b = |k: &str| base.weapon.as_ref().and_then(|w| w.get(k).copied());
                let nan = f64::NAN;
                let mut set = vec![
                    ("CritChance".to_string(), b("CritChanceBase").unwrap_or(nan)),
                    (
                        "AttackRate".to_string(),
                        b("AttackRateBase").unwrap_or(nan) * speed_mod,
                    ),
                    ("Range".to_string(), b("Range").unwrap_or(nan)),
                ];
                for t in DMG_TYPES {
                    for mm in ["Min", "Max"] {
                        let added =
                            self.c
                                .base(list, Some(&cfg), &[&format!("EnergyBlade{mm}{t}")])?;
                        set.push((
                            format!("{t}{mm}"),
                            b(&format!("{t}{mm}")).unwrap_or(0.0) + (added * dmg_mod).floor(),
                        ));
                    }
                }
                if let Some(w) = self.c.actors.actors[actor].weapon_data[n].as_mut() {
                    for (k, v) in set {
                        w.insert(k, ModValue::Number(v));
                    }
                }
            }
        }
        // account for Battlemage
        if self.c.flag(list, None, &["Battlemage"])? && self.slot_has_weapon(actor, "Weapon 1") {
            let multiplier = self
                .c
                .max(list, Some(&cfg), &["MainHandWeaponDamageAppliesToSpells"])?
                .unwrap_or(100.0)
                / 100.0;
            for t in DMG_TYPES {
                for mm in ["Min", "Max"] {
                    let v = (self
                        .weapon_num(actor, 0, &format!("{t}{mm}"))
                        .unwrap_or(0.0)
                        * multiplier)
                        .floor();
                    self.c.add(
                        list,
                        new_mod(
                            &format!("{t}{mm}"),
                            Base,
                            v,
                            Some("Battlemage"),
                            spell,
                            0,
                            vec![],
                        ),
                    );
                }
            }
        }
        let w1_type = self.weapon_str(actor, 0, "type");
        let w2_type = self.weapon_str(actor, 1, "type");
        let info = |t: &Option<std::rc::Rc<str>>| {
            t.as_ref()
                .and_then(|t| self.data().weapon_type_info.get(&**t))
                .cloned()
        };
        let (w1_info, w2_info) = (info(&w1_type), info(&w2_type));
        // account for Spellblade
        let spellblade = self
            .c
            .max(list, Some(&cfg), &["OneHandWeaponDamageAppliesToSpells"])?;
        if let Some(multi) = spellblade
            && self.slot_has_weapon(actor, "Weapon 1")
            && let Some(w1) = &w1_info
            && w1.melee
            && w1.one_hand
        {
            let multiplier = multi / 100.0 * if w2_info.is_some() { 0.6 } else { 1.0 };
            let mut weapons = vec![(0, "Spellblade Main Hand")];
            if w2_info.is_some() {
                weapons.push((1, "Spellblade Off Hand"));
            }
            for (n, source) in weapons {
                for t in DMG_TYPES {
                    for mm in ["Min", "Max"] {
                        let v = (self
                            .weapon_num(actor, n, &format!("{t}{mm}"))
                            .unwrap_or(0.0)
                            * multiplier)
                            .floor();
                        self.c.add(
                            list,
                            new_mod(&format!("{t}{mm}"), Base, v, Some(source), spell, 0, vec![]),
                        );
                    }
                }
            }
        }
        if self.c.flag(list, None, &["MinionDamageAppliesToPlayer"])?
            || self
                .c
                .flag(list, Some(&cfg), &["MinionDamageAppliesToPlayer"])?
        {
            let multiplier = self
                .c
                .max(list, Some(&cfg), &["ImprovedMinionDamageAppliesToPlayer"])?
                .unwrap_or(100.0)
                / 100.0;
            for v in self.c.list(list, Some(&cfg), &["MinionModifier"])? {
                let ModValue::Record(r) = &v else { continue };
                let Some(ModValue::Mod(m)) = r.get("mod") else {
                    continue;
                };
                if m.name.as_str() == "Damage" && m.typ == Inc {
                    let tags = converted_tags(m, multiplier, true);
                    let value = number(&m.value)? * multiplier;
                    self.c.add(
                        list,
                        new_mod(
                            "Damage",
                            Inc,
                            value,
                            m.source.as_deref(),
                            m.flags,
                            m.keyword_flags,
                            tags,
                        ),
                    );
                }
            }
        }
        for (flag, improved, flag_bit) in [
            (
                "MinionAttackSpeedAppliesToPlayer",
                "ImprovedMinionAttackSpeedAppliesToPlayer",
                attack,
            ),
            (
                "MinionCastSpeedAppliesToPlayer",
                "ImprovedMinionCastSpeedAppliesToPlayer",
                self.mf("Cast"),
            ),
        ] {
            if self.c.flag(list, None, &[flag])? {
                let multiplier =
                    self.c.max(list, Some(&cfg), &[improved])?.unwrap_or(100.0) / 100.0;
                for v in self.c.list(list, Some(&cfg), &["MinionModifier"])? {
                    let ModValue::Record(r) = &v else { continue };
                    let Some(ModValue::Mod(m)) = r.get("mod") else {
                        continue;
                    };
                    if m.name.as_str() == "Speed"
                        && m.typ == Inc
                        && (m.flags == 0 || m.flags & flag_bit != 0)
                    {
                        let tags = converted_tags(m, multiplier, true);
                        let value = number(&m.value)? * multiplier;
                        self.c.add(
                            list,
                            new_mod(
                                "Speed",
                                Inc,
                                value,
                                m.source.as_deref(),
                                flag_bit,
                                m.keyword_flags,
                                tags,
                            ),
                        );
                    }
                }
            }
        }
        if self.c.flag(list, None, &["EvasionAppliesToSpellDamage"])? {
            for (_, m) in self
                .c
                .tabulate(list, Some(Inc), Some(&Cfg::default()), &["Evasion"])?
            {
                let v = number(&m.value)?;
                self.c
                    .add(list, Self::copy_mod("Damage", m.typ, v, &m, spell));
            }
        }
        if self.c.flag(list, None, &["SpellDamageAppliesToAttacks"])?
            || self
                .c
                .flag(list, Some(&cfg), &["SpellDamageAppliesToAttacks"])?
        {
            let multiplier = self
                .c
                .max(list, Some(&cfg), &["ImprovedSpellDamageAppliesToAttacks"])?
                .unwrap_or(100.0)
                / 100.0;
            let spell_cfg = Cfg {
                flags: Some(spell),
                ..Cfg::default()
            };
            for (_, m) in self
                .c
                .tabulate(list, Some(Inc), Some(&spell_cfg), &["Damage"])?
            {
                if m.flags & spell != 0 {
                    let tags = converted_tags(&m, multiplier, false);
                    let value = number(&m.value)? * multiplier;
                    self.c.add(
                        list,
                        new_mod(
                            "Damage",
                            Inc,
                            value,
                            m.source.as_deref(),
                            (m.flags & !spell) | attack,
                            m.keyword_flags,
                            tags,
                        ),
                    );
                    if m.source.as_deref() == Some("Strength") {
                        for flags in [self.mf("Melee"), attack | self.mf("Projectile")] {
                            self.c.replace_mod(
                                list,
                                new_mod(
                                    "PhysicalDamage",
                                    Inc,
                                    0.0,
                                    Some("Strength"),
                                    flags,
                                    0,
                                    vec![],
                                ),
                            );
                        }
                    }
                }
            }
        }
        let cast = self.mf("Cast");
        if self.c.flag(list, None, &["CastSpeedAppliesToAttacks"])? {
            let multiplier = self
                .c
                .max(list, Some(&cfg), &["ImprovedCastSpeedAppliesToAttacks"])?
                .unwrap_or(100.0)
                / 100.0;
            let cast_cfg = Cfg {
                flags: Some(cast),
                ..Cfg::default()
            };
            for (_, m) in self
                .c
                .tabulate(list, Some(Inc), Some(&cast_cfg), &["Speed"])?
            {
                if m.flags & cast != 0 {
                    let tags = converted_tags(&m, multiplier, false);
                    let value = number(&m.value)? * multiplier;
                    self.c.add(
                        list,
                        new_mod(
                            "Speed",
                            Inc,
                            value,
                            m.source.as_deref(),
                            (m.flags & !cast) | attack,
                            m.keyword_flags,
                            tags,
                        ),
                    );
                }
            }
        }
        let (bow, hit, claw, unarmed, melee) = (
            self.mf("Bow"),
            self.mf("Hit"),
            self.mf("Claw"),
            self.mf("Unarmed"),
            self.mf("Melee"),
        );
        if self
            .c
            .flag(list, None, &["ProjectileSpeedAppliesToBowDamage"])?
        {
            for (_, m) in
                self.c
                    .tabulate(list, Some(Inc), Some(&Cfg::default()), &["ProjectileSpeed"])?
            {
                let v = number(&m.value)?;
                self.c
                    .add(list, Self::copy_mod("Damage", m.typ, v, &m, bow | hit));
            }
        }
        if self.c.flag(list, None, &["ClawDamageAppliesToUnarmed"])? {
            let c = Cfg {
                flags: Some(claw | hit),
                keyword_flags: Some(self.kf("Hit")),
                ..Cfg::default()
            };
            for (_, m) in self.c.tabulate(list, Some(Inc), Some(&c), &["Damage"])? {
                if m.flags & claw != 0 {
                    let v = number(&m.value)?;
                    let flags = (m.flags & !claw) | unarmed | melee;
                    self.c
                        .add(list, Self::copy_mod("Damage", m.typ, v, &m, flags));
                }
            }
        }
        if self
            .c
            .flag(list, None, &["ClawAttackSpeedAppliesToUnarmed"])?
        {
            let c = Cfg {
                flags: Some(claw | attack | hit),
                ..Cfg::default()
            };
            for (_, m) in self.c.tabulate(list, Some(Inc), Some(&c), &["Speed"])? {
                if m.flags & claw != 0 && m.flags & attack != 0 {
                    let v = number(&m.value)?;
                    let flags = (m.flags & !claw) | unarmed;
                    self.c
                        .add(list, Self::copy_mod("Speed", m.typ, v, &m, flags));
                }
            }
        }
        let claw_hit = Cfg {
            flags: Some(claw | hit),
            ..Cfg::default()
        };
        if self
            .c
            .flag(list, None, &["ClawCritChanceAppliesToUnarmed"])?
        {
            for (_, m) in self
                .c
                .tabulate(list, Some(Inc), Some(&claw_hit), &["CritChance"])?
            {
                if m.flags & claw != 0 {
                    let v = number(&m.value)?;
                    let flags = (m.flags & !claw) | unarmed;
                    self.c
                        .add(list, Self::copy_mod("CritChance", m.typ, v, &m, flags));
                }
            }
        }
        if self
            .c
            .flag(list, None, &["ClawCritChanceAppliesToMinions"])?
            || self
                .c
                .flag(list, None, &["ClawCritMultiplierAppliesToMinions"])?
        {
            let mdb = self
                .minion_actor()
                .map(|a| self.c.db(a))
                .ok_or("attempt to index a nil value (field 'minion')")?;
            for (flag, typ, stat) in [
                ("ClawCritChanceAppliesToMinions", Inc, "CritChance"),
                ("ClawCritMultiplierAppliesToMinions", Base, "CritMultiplier"),
            ] {
                if !self.c.flag(list, None, &[flag])? {
                    continue;
                }
                for (_, m) in self.c.tabulate(list, Some(typ), Some(&claw_hit), &[stat])? {
                    if m.flags & claw != 0 {
                        let v = number(&m.value)?;
                        self.c
                            .add(mdb, Mod::new(stat, m.typ, v, m.source.as_deref()));
                    }
                }
            }
        }
        for (flag, stat, name) in [
            (
                "CritChanceIncreasedByUncappedLightningRes",
                "LightningResistTotal",
                "CritChance",
            ),
            (
                "CritChanceIncreasedByLightningRes",
                "LightningResist",
                "CritChance",
            ),
            (
                "CritChanceIncreasedByOvercappedLightningRes",
                "LightningResistOverCap",
                "CritChance",
            ),
            (
                "CritChanceIncreasedBySpellSuppressChance",
                "SpellSuppressionChance",
                "CritChance",
            ),
            (
                "FirePenIncreasedByUncappedFireRes",
                "FireResistOverCap",
                "FirePenetration",
            ),
        ] {
            if self.c.flag(list, None, &[flag])? {
                let typ = if name == "CritChance" { Inc } else { Base };
                if let Some((_, m)) = self
                    .c
                    .tabulate(db, Some(FlagT), None, &[flag])?
                    .into_iter()
                    .next()
                {
                    let v = self.c.out(actor, stat).unwrap_or(f64::NAN);
                    self.c
                        .add(list, Mod::new(name, typ, v, m.source.as_deref()));
                }
            }
        }
        for (flag, name, half) in [
            ("LightRadiusAppliesToAccuracy", "Accuracy", false),
            ("LightRadiusAppliesToAreaOfEffect", "AreaOfEffect", true),
            ("LightRadiusAppliesToDamage", "Damage", false),
        ] {
            if self.c.flag(list, None, &[flag])? {
                for (_, m) in
                    self.c
                        .tabulate(list, Some(Inc), Some(&Cfg::default()), &["LightRadius"])?
                {
                    let mut v = number(&m.value)?;
                    if half {
                        v = (v / 2.0).floor();
                    }
                    self.c.add(list, Self::copy_mod(name, Inc, v, &m, m.flags));
                }
            }
        }
        if self
            .c
            .flag(list, None, &["CastSpeedAppliesToTrapThrowingSpeed"])?
        {
            let c = Cfg {
                flags: Some(cast),
                ..Cfg::default()
            };
            for (_, m) in self.c.tabulate(list, Some(Inc), Some(&c), &["Speed"])? {
                if m.flags == 0 || m.flags & cast != 0 {
                    let v = number(&m.value)?;
                    // `band(flags, bnot(Cast), bnot(Attack))`
                    let flags = m.flags & !cast & !attack;
                    self.c
                        .add(list, Self::copy_mod("TrapThrowingSpeed", Inc, v, &m, flags));
                }
            }
        }
        if self.sd_truthy(i, "arrowSpeedAppliesToAreaOfEffect") {
            let c = Cfg {
                flags: Some(bow),
                ..Cfg::default()
            };
            for (_, m) in self
                .c
                .tabulate(list, Some(Inc), Some(&c), &["ProjectileSpeed"])?
            {
                let v = number(&m.value)?;
                self.c
                    .add(list, Self::copy_mod("AreaOfEffect", Inc, v, &m, m.flags));
            }
        }
        if self.c.flag(list, None, &["SequentialProjectiles"])?
            && !self.c.flag(list, None, &["OneShotProj"])?
            && !self.c.flag(list, None, &["NoAdditionalProjectiles"])?
            && !self.c.flag(list, None, &["SingleProjectile"])?
            && !self.c.flag(list, None, &["TriggeredBySnipe"])?
        {
            let v = self.c.base(list, Some(&cfg), &["ProjectileCount"])?;
            self.sd_set(i, "dpsMultiplier", v);
        }
        self.repeats(o)?;
        let cfg = self.cfg(i);
        if self.c.flag(list, None, &["WeaponPhysAppliesToSpells"])? {
            let mult = self
                .c
                .base(list, Some(&cfg), &["WeaponPhysAppliesToSpellsPercent"])?
                / 100.0;
            if self.weapon(actor, 0).is_some() {
                for mm in ["Min", "Max"] {
                    let name = format!("Physical{mm}");
                    let v = (self.weapon_num(actor, 0, &name).unwrap_or(0.0) * mult).floor();
                    self.c.add(
                        list,
                        new_mod(
                            &name,
                            Base,
                            v,
                            Some("Runegraft of the Spellbound"),
                            spell,
                            0,
                            vec![],
                        ),
                    );
                }
            }
        }
        if let Some(pct) = self.sd_num(i, "gainPercentBaseWandDamageToSpells") {
            let mult = pct / 100.0;
            let is_wand = |t: &Option<std::rc::Rc<str>>| t.as_deref() == Some("Wand");
            let weapons: Vec<usize> = if is_wand(&w1_type) && is_wand(&w2_type) {
                vec![0, 1]
            } else if is_wand(&w1_type) {
                vec![0]
            } else if is_wand(&w2_type) {
                vec![1]
            } else {
                vec![]
            };
            if !weapons.is_empty() {
                for t in DMG_TYPES {
                    for mm in ["Min", "Max"] {
                        let name = format!("{t}{mm}");
                        let v = if weapons.len() == 2 {
                            ((self.weapon_num(actor, 0, &name).unwrap_or(0.0)
                                + self.weapon_num(actor, 1, &name).unwrap_or(0.0))
                                / 2.0
                                * mult)
                                .floor()
                        } else {
                            (self.weapon_num(actor, weapons[0], &name).unwrap_or(0.0) * mult)
                                .floor()
                        };
                        self.c.add(
                            list,
                            new_mod(&name, Base, v, Some("Spellslinger"), spell, 0, vec![]),
                        );
                    }
                }
            }
        }
        if let Some(pct) = self.sd_num(i, "gainPercentBaseDaggerDamageToSpells") {
            let is_dagger = |n: usize, t: &Option<std::rc::Rc<str>>| {
                self.weapon(actor, n)
                    .and_then(|w| w.get("AddedUsingDagger"))
                    .is_some_and(ModValue::truthy)
                    || t.as_deref() == Some("Dagger")
            };
            let (d1, d2) = (is_dagger(0, &w1_type), is_dagger(1, &w2_type));
            let mult = pct / 100.0 * if d1 && d2 { 0.5 } else { 1.0 };
            for t in DMG_TYPES {
                for mm in ["Min", "Max"] {
                    let name = format!("{t}{mm}");
                    let base = if d1 {
                        self.weapon_num(actor, 0, &name).unwrap_or(0.0)
                    } else {
                        0.0
                    } + if d2 {
                        self.weapon_num(actor, 1, &name).unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    self.c.add(
                        list,
                        new_mod(
                            &name,
                            Base,
                            (base * mult).floor(),
                            Some("Blade Blast of Dagger Detonation"),
                            spell,
                            0,
                            vec![],
                        ),
                    );
                }
            }
        }
        if self.c.flag(list, None, &["HasSeals"])?
            && self.has_type(i, "CanRapidFire")
            && !self.c.flag(list, None, &["NoRepeatBonuses"])?
        {
            self.seals(o)?;
        }
        self.random_phys(o)?;
        // momentum stacks
        let cfg = self.cfg(i);
        if self.c.flag(list, None, &["SupportedByMomentum"])? {
            let max = self.c.base(list, Some(&cfg), &["MomentumStacksMax"])?;
            let extra = self.c.base(list, Some(&cfg), &["MomentumStacksExtra"])?;
            let name = "Multiplier:MomentumStacks";
            if max > 0.0 {
                if !self.c.has_mod(db, Base, None, &[name])? {
                    self.c.add(
                        db,
                        Mod::new(name, Base, m_min((max + extra) / 2.0, max), Some("Config"))
                            .tag(Tag::condition("Combat")),
                    );
                } else if self.c.base(db, None, &[name])? > max {
                    self.c.replace_mod(
                        db,
                        Mod::new(name, Base, max, Some("Config")).tag(Tag::condition("Combat")),
                    );
                }
            } else if self.c.has_mod(db, Base, None, &[name])? {
                self.c
                    .replace_mod(db, Mod::new(name, Base, 0.0, Some("Config")));
            }
        }
        o.is_attack = self.has_flag(i, "attack");
        self.run_skill_func(o, "preSkillTypeFunc")
    }

    /// Repeats and their damage and area
    fn repeats(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        let mut check = ![
            "Instant",
            "Channel",
            "Triggered",
            "Retaliation",
            "NonRepeatable",
        ]
        .iter()
        .any(|t| self.has_type(i, t));
        if check {
            check = !self.c.flag(list, None, &["CannotRepeat"])?
                && (self.has_type(i, "Attack") || self.has_type(i, "Spell"));
        }
        let repeats = 1.0
            + if check {
                self.c.base(list, Some(&cfg), &["RepeatCount"])?
            } else {
                0.0
            };
        self.c.set_out(actor, "Repeats", repeats);
        if repeats <= 1.0 {
            return Ok(());
        }
        self.c.set_out(actor, "RepeatCount", repeats);
        let mode = match self.input.config.get("repeatMode") {
            Some(crate::config::Val::Str(s)) => Some(s.to_string()),
            _ => None,
        };
        if mode.as_deref() == Some("NONE") {
            return Ok(());
        }
        let average = mode.as_deref() == Some("AVERAGE");
        for (_, m) in self
            .c
            .tabulate(list, Some(Inc), Some(&cfg), &["RepeatFinalAreaOfEffect"])?
        {
            let mut v = number(&m.value)?;
            if average {
                v /= repeats;
            }
            self.c
                .add(list, Self::copy_mod("AreaOfEffect", Inc, v, &m, m.flags));
        }
        for (_, m) in self.c.tabulate(
            list,
            Some(Inc),
            Some(&cfg),
            &["RepeatPerRepeatAreaOfEffect"],
        )? {
            let mut v = number(&m.value)? * (repeats - 1.0);
            if average {
                v /= 2.0;
            }
            self.c
                .add(list, Self::copy_mod("AreaOfEffect", Inc, v, &m, m.flags));
        }
        for (_, m) in self.c.tabulate(
            list,
            Some(Base),
            Some(&cfg),
            &["RepeatFinalDoubleDamageChance"],
        )? {
            let mut v = number(&m.value)?;
            if average {
                v /= repeats;
            }
            self.c.add(
                list,
                Self::copy_mod("DoubleDamageChance", Base, v, &m, m.flags),
            );
        }
        let mut final_more_total = 1.0;
        let mut more_total = 0.0;
        for (_, m) in self
            .c
            .tabulate(list, Some(More), Some(&cfg), &["RepeatFinalDamage"])?
        {
            let mut v = number(&m.value)?;
            final_more_total *= 1.0 + v / 100.0;
            more_total += v;
            if average && !self.c.flag(list, None, &["OnlyFinalRepeat"])? {
                v /= repeats;
            }
            self.c
                .add(list, Self::copy_mod("Damage", More, v, &m, m.flags));
        }
        for (_, m) in self
            .c
            .tabulate(list, Some(More), Some(&cfg), &["RepeatPerRepeatDamage"])?
        {
            let mv = number(&m.value)?;
            let mut v = mv * (repeats - 1.0);
            if average {
                if final_more_total != 1.0 {
                    v = ((100.0 + mv * (repeats - 2.0) / 2.0) * (repeats - 1.0)
                        + (100.0 + mv * (repeats - 1.0)) * final_more_total)
                        / (repeats + more_total / 100.0)
                        - 100.0;
                } else {
                    v /= 2.0;
                }
            }
            self.c
                .add(list, Self::copy_mod("Damage", More, v, &m, m.flags));
        }
        let mut last_mod: Option<Mod> = None;
        let final_total = more_total;
        for (n, word) in [(2.0, "One"), (3.0, "Two"), (4.0, "Three")] {
            if n > repeats {
                break;
            }
            let name = format!("Repeat{word}Damage");
            if average {
                for (_, m) in self.c.tabulate(list, Some(More), Some(&cfg), &[&name])? {
                    more_total += number(&m.value)?;
                    last_mod = Some(m);
                }
            } else if n == repeats {
                for (_, m) in self.c.tabulate(list, Some(More), Some(&cfg), &[&name])? {
                    let v = number(&m.value)?;
                    self.c
                        .add(list, Self::copy_mod("Damage", More, v, &m, m.flags));
                }
            }
        }
        if average && let Some(m) = last_mod {
            let v = (more_total / repeats + 100.0) / (1.0 + final_total / repeats / 100.0) - 100.0;
            self.c
                .add(list, Self::copy_mod("Damage", More, v, &m, m.flags));
        }
        if self.c.flag(list, None, &["FinalRepeatSumsDamage"])? {
            for (_, m) in
                self.c
                    .tabulate(list, Some(FlagT), Some(&cfg), &["FinalRepeatSumsDamage"])?
            {
                let v = (100.0 * repeats + final_total) / (1.0 + final_total / 100.0) - 100.0;
                self.c
                    .add(list, Self::copy_mod("Damage", More, v, &m, m.flags));
            }
        }
        if self.has_flag(i, "trap") || self.has_flag(i, "mine") {
            self.c.add(
                list,
                Mod::new("DPS", More, (repeats - 1.0) * 100.0, Some("Repeat Count")),
            );
        }
        Ok(())
    }

    /// Seals (`HasSeals`)
    fn seals(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        let cooldown = self.c.base(list, Some(&cfg), &["SealGainFrequency"])?
            / self.c.calc_mod(list, Some(&cfg), &["SealGainFrequency"])?;
        let max = self.c.base(list, Some(&cfg), &["SealCount"])?;
        self.c.set_out(actor, "SealCooldown", cooldown);
        self.c.set_out(actor, "SealMax", max);
        self.c.set_out(actor, "AverageBurstHits", max);
        self.c.set_out(actor, "TimeMaxSeals", cooldown * max);
        if !self.sd(i, "hitTimeOverride").is_some_and(ModValue::truthy) {
            self.seal_repeats(o, cooldown, max)?;
        }
        if self.has_bd(actor) {
            let c = Some(&cfg);
            let base = self.c.base(list, c, &["SealGainFrequency"])?;
            let inc = 1.0 + self.c.inc(list, c, &["SealGainFrequency"])? / 100.0;
            let action = self.c.out(actor, "ActionSpeedMod").unwrap_or(f64::NAN);
            let mut l = Vec::new();
            multi_chain(
                &mut l,
                Chain {
                    label: Some("Gain frequency:"),
                    base: Some(f("%.2fs ^8(base gain frequency)", base)),
                    factors: vec![
                        f("%.2f ^8(increased/reduced gain frequency)", inc),
                        f("%.2f ^8(action speed modifier)", action),
                    ],
                    total: Some(sf!("= %.2fs ^8per Seal", cooldown)),
                    ..Chain::default()
                },
            );
            self.bd(actor).unwrap().set_lines("SealGainTime", l);
        }
        Ok(())
    }

    /// The seals' DPS multiplier and hit time on the player's main skill
    fn seal_repeats(&mut self, o: &mut Off, cooldown: f64, max: f64) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        // upstream sets these on the player's main skill
        let main = self.main();
        let cast_time = self
            .skill(i)
            .active_effect
            .granted_effect
            .cast_time
            .unwrap_or(f64::NAN);
        let action = self.c.out(actor, "ActionSpeedMod").unwrap_or(f64::NAN);
        let speed = 1.0 / cast_time * 1.1 * self.c.calc_mod(list, Some(&cfg), &["Speed"])? * action;
        let penalty = self.c.calc_mod(list, Some(&cfg), &["SealRepeatPenalty"])?;
        if self.c.flag(list, None, &["UseMaxUnleash"])? {
            for (_, m) in
                self.c
                    .tabulate(list, Some(Inc), Some(&Cfg::default()), &["MaxSealCrit"])?
            {
                let v = number(&m.value)?;
                self.c
                    .add(list, Self::copy_mod("CritChance", Inc, v, &m, m.flags));
            }
            for (_, m) in
                self.c
                    .tabulate(list, Some(More), Some(&Cfg::default()), &["MaxSealDamage"])?
            {
                let v = number(&m.value)? * max;
                self.c
                    .add(list, Self::copy_mod("Damage", More, v, &m, m.flags));
            }
            self.sd_set(main, "dpsMultiplier", 1.0 + max * penalty);
            self.sd_set(main, "hitTimeOverride", m_max(cooldown * max, speed));
        } else {
            self.sd_set(
                main,
                "dpsMultiplier",
                1.0 + 1.0 / cooldown / speed * penalty,
            );
        }
        Ok(())
    }

    /// Random physical conversions and gains, by `physMode`
    fn random_phys(&mut self, o: &mut Off) -> Result<()> {
        let (i, list) = (o.skill, o.list);
        let phys_mode = match self.input.config.get("physMode") {
            Some(crate::config::Val::Str(s)) => s.to_string(),
            _ => "AVERAGE".to_string(),
        };
        let mut processed: Vec<Mod> = Vec::new();
        let cfgs: Vec<Cfg> = {
            let m = self.mods(i);
            std::iter::once(m.skill_cfg.clone())
                .chain(m.weapon_cfgs.iter().flatten().cloned())
                .collect()
        };
        for cfg in cfgs {
            if self.c.base(
                list,
                Some(&cfg),
                &[
                    "PhysicalDamageGainAsRandom",
                    "PhysicalDamageConvertToRandom",
                    "PhysicalDamageGainAsColdOrLightning",
                ],
            )? <= 0.0
            {
                continue;
            }
            self.set_flag(i, "randomPhys", true);
            let before = processed.clone();
            for (stat, prefix, three) in [
                ("PhysicalDamageGainAsRandom", "PhysicalDamageGainAs", true),
                (
                    "PhysicalDamageConvertToRandom",
                    "PhysicalDamageConvertTo",
                    true,
                ),
                (
                    "PhysicalDamageGainAsColdOrLightning",
                    "PhysicalDamageGainAs",
                    false,
                ),
            ] {
                for (_, m) in self.c.tabulate(list, Some(Base), Some(&cfg), &[stat])? {
                    if before.contains(&m) {
                        continue;
                    }
                    processed.push(m.clone());
                    let value = number(&m.value)?;
                    let targets: Vec<(&str, f64)> = if three {
                        match phys_mode.as_str() {
                            "AVERAGE" => {
                                let v = value / 3.0;
                                vec![("Fire", v), ("Cold", v), ("Lightning", v)]
                            }
                            "FIRE" => vec![("Fire", value)],
                            "COLD" => vec![("Cold", value)],
                            "LIGHTNING" => vec![("Lightning", value)],
                            _ => vec![],
                        }
                    } else {
                        match phys_mode.as_str() {
                            "AVERAGE" | "FIRE" => {
                                let v = value / 2.0;
                                vec![("Cold", v), ("Lightning", v)]
                            }
                            "COLD" => vec![("Cold", value)],
                            "LIGHTNING" => vec![("Lightning", value)],
                            _ => vec![],
                        }
                    };
                    for (t, v) in targets {
                        self.c.add(
                            list,
                            Self::copy_mod(&format!("{prefix}{t}"), Base, v, &m, m.flags),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// `calcAreaOfEffect`
    pub(crate) fn area_of_effect(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let bd = self.has_bd(actor);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let names = ["AreaOfEffect", "AreaOfEffectPrimary"];
        let inc = 1.0 + self.c.inc(list, c, &names)? / 100.0;
        let more = self.c.more(list, c, &names)?;
        let area_mod = calc_area_mod(inc, more);
        self.c.set_out(actor, "AreaOfEffectMod", area_mod);
        let radius_mod = self.c.calc_mod(list, c, &["AreaOfEffectRadius"])?;
        if self.sd_truthy(i, "radiusIsWeaponRange") {
            let mut range: f64 = 0.0;
            if self.has_flag(i, "weapon1Attack") {
                range = m_max(range, self.weapon_range[0].unwrap_or(f64::NAN));
            }
            if self.has_flag(i, "weapon2Attack") {
                range = m_max(range, self.weapon_range[1].unwrap_or(f64::NAN));
            }
            self.sd_set(i, "radius", range + 2.0);
        }
        if let Some(radius) = self.sd_num(i, "radius") {
            self.area_radii(o, radius, area_mod, radius_mod, [inc, more])?;
        }
        if bd {
            let mut l = Vec::new();
            if area_mod != 1.0 {
                let inc = 1.0 + self.c.inc(list, c, &["AreaOfEffect"])? / 100.0;
                let more = self.c.more(list, c, &["AreaOfEffect"])?;
                multi_chain(
                    &mut l,
                    Chain {
                        factors: vec![
                            f("%.2f ^8(increased/reduced)", inc),
                            f("%.2f ^8(more/less)", more),
                        ],
                        total: Some(sf!("= %.2f", area_mod)),
                        ..Chain::default()
                    },
                );
            }
            self.bd(actor).unwrap().set_lines("AreaOfEffectMod", l);
        }
        Ok(())
    }

    /// The radii of `calcAreaOfEffect`, for a skill with a radius
    fn area_radii(
        &mut self,
        o: &mut Off,
        radius: f64,
        area_mod: f64,
        radius_mod: f64,
        [inc, more]: [f64; 2],
    ) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let bd = self.has_bd(actor);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let label = |p: &Self, k: &str| p.sd_str(i, k);
        self.set_flag(i, "area", true);
        let extra = self.sd_num(i, "radiusExtra").unwrap_or(0.0);
        let base = radius + extra + self.c.base(list, c, &["AreaOfEffect"])?;
        let r = calc_radius(base, area_mod, radius_mod);
        self.c.set_out(actor, "AreaOfEffectRadius", r);
        self.c.set_out(actor, "AreaOfEffectRadiusMetres", r / 10.0);
        if bd {
            let b = area(
                base,
                area_mod,
                r,
                radius_breakpoints(base, inc, more, radius_mod),
                label(self, "radiusLabel").as_deref(),
                Some(radius_mod),
            );
            self.bd(actor).unwrap().set("AreaOfEffectRadius", b);
        }
        if let Some(secondary) = self.sd_num(i, "radiusSecondary") {
            let names = ["AreaOfEffect", "AreaOfEffectSecondary"];
            let inc = 1.0 + self.c.inc(list, c, &names)? / 100.0;
            let more = self.c.more(list, c, &names)?;
            let m = calc_area_mod(inc, more);
            self.c.set_out(actor, "AreaOfEffectModSecondary", m);
            let base = secondary + extra;
            let r = calc_radius(base, m, radius_mod);
            self.c.set_out(actor, "AreaOfEffectRadiusSecondary", r);
            self.c
                .set_out(actor, "AreaOfEffectRadiusSecondaryMetres", r / 10.0);
            if bd {
                let bp = (!self.sd_truthy(i, "projectileSpeedAppliesToMSAreaOfEffect"))
                    .then(|| radius_breakpoints(base, inc, more, radius_mod))
                    .flatten();
                let b = area(
                    base,
                    m,
                    r,
                    bp,
                    label(self, "radiusSecondaryLabel").as_deref(),
                    Some(radius_mod),
                );
                self.bd(actor).unwrap().set("AreaOfEffectRadiusSecondary", b);
            }
        }
        if let Some(tertiary) = self.sd_num(i, "radiusTertiary") {
            let names = ["AreaOfEffect", "AreaOfEffectTertiary"];
            let inc = 1.0 + self.c.inc(list, c, &names)? / 100.0;
            let more = self.c.more(list, c, &names)?;
            let m = calc_area_mod(inc, more);
            self.c.set_out(actor, "AreaOfEffectModTertiary", m);
            let base = tertiary + extra;
            let tlabel = label(self, "radiusTertiaryLabel");
            let r = if self.sd_truthy(i, "projectileSpeedAppliesToMSAreaOfEffect") {
                let s_inc = 1.0 + self.c.inc(list, c, &["ProjectileSpeed"])? / 100.0;
                let s_more = self.c.more(list, c, &["ProjectileSpeed"])?;
                let speed = round(round(s_inc * s_more, 10.0), 2.0);
                self.c.set_out(actor, "SpeedModTertiary", speed);
                let deadzone = self.sd_num(i, "radiusSecondary").unwrap_or(f64::NAN);
                if bd {
                    let b = molten_strike_breakdown(
                        deadzone,
                        base,
                        tlabel.as_deref(),
                        [inc, more, s_inc, s_more],
                    );
                    self.bd(actor).unwrap().set("AreaOfEffectRadiusTertiary", b);
                }
                molten_strike_tertiary_radius(base, deadzone, m, speed)
            } else if let Some(margin_pct) = self.sd_num(i, "radiusTertiaryBaseMargin") {
                let margin = margin_pct / 100.0;
                let width = margin_pct * 2.0 + 1.0;
                // The possible base radii, in the order Lua's float loop
                // finds them
                let mut occurrences: Vec<(f64, f64)> = Vec::new();
                let mut dev = 1.0 - margin;
                let limit = 1.0 + margin + 0.01;
                while dev <= limit {
                    let r = (base * dev).floor();
                    match occurrences.iter_mut().find(|(k, _)| *k == r) {
                        Some((_, n)) => *n += 1.0,
                        None => occurrences.push((r, 1.0)),
                    }
                    dev += 0.01;
                }
                let mut sum = 0.0;
                let mut radii: BTreeMap<i64, f64> = BTreeMap::new();
                for (b, n) in &occurrences {
                    let r = calc_radius(*b, m, radius_mod);
                    sum += r * n;
                    *radii.entry(r as i64).or_default() += n;
                }
                self.c.set_out_val(
                    actor,
                    "AreaOfEffectRadiusTertiaryOccurrences",
                    OutVal::Table(std::rc::Rc::new(
                        radii
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), OutVal::Num(v)))
                            .collect(),
                    )),
                );
                let avg = sum / width;
                if bd {
                    let mut l = Vec::new();
                    l.extend(tlabel.clone());
                    l.push(sf!(
                        "R ^8(base radius)^7 x %.2f ^8(square root of area of effect modifier)",
                        (100.0 * m.sqrt()).floor() / 100.0
                    ));
                    if radius_mod != 1.0 {
                        l.push(sf!("x %.2f ^8(final radius modifier)", radius_mod));
                    }
                    occurrences.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let mut bps = [f64::INFINITY; 4];
                    for (b, n) in &occurrences {
                        l.push(sf!(
                            "%.1f%% ^8chance of^7 %.1fm ^8base radius resulting in^7 %.1fm ^8final radius",
                            n / width * 100.0,
                            b / 10.0,
                            calc_radius(*b, m, radius_mod) / 10.0
                        ));
                        let x = radius_breakpoints(*b, inc, more, radius_mod)
                            .ok_or("attempt to compare nil with number")?;
                        for (a, x) in bps.iter_mut().zip(x) {
                            *a = m_min(*a, x);
                        }
                    }
                    l.push(sf!(
                        "^8Next closest 0.1m breakpoint: %d%% increased AoE / a %d%% more AoE multiplier",
                        bps[0],
                        bps[1]
                    ));
                    l.push(sf!(
                        "^8Previous closest 0.1m breakpoint: %d%% reduced AoE / a %d%% less AoE multiplier",
                        bps[2],
                        bps[3]
                    ));
                    l.push(sf!("On average, the radius is %.2fm", avg / 10.0));
                    self.bd(actor)
                        .unwrap()
                        .set_lines("AreaOfEffectRadiusTertiary", l);
                }
                avg
            } else {
                let r = calc_radius(base, m, radius_mod);
                if bd {
                    let b = area(
                        base,
                        m,
                        r,
                        radius_breakpoints(base, inc, more, radius_mod),
                        tlabel.as_deref(),
                        Some(radius_mod),
                    );
                    self.bd(actor).unwrap().set("AreaOfEffectRadiusTertiary", b);
                }
                r
            };
            self.c.set_out(actor, "AreaOfEffectRadiusTertiary", r);
            self.c
                .set_out(actor, "AreaOfEffectRadiusTertiaryMetres", r / 10.0);
        }
        Ok(())
    }
}

/// The throwing time of traps and mines
fn throwing_time(speed: f64, time_mod: f64) -> Vec<String> {
    let mut l = Vec::new();
    multi_chain(
        &mut l,
        Chain {
            label: Some("Throwing time:"),
            base: Some(f("%.2f ^8(base throwing time)", 1.0 / (speed * time_mod))),
            factors: vec![f("%.2f ^8(total modifier)", time_mod)],
            total: Some(sf!("= %.2f ^8seconds per throw", 1.0 / speed)),
            ..Chain::default()
        },
    );
    l
}

/// `calcRadiusBreakpoints(baseRadius, incArea, moreArea, radiusMod)`:
/// increased, more, reduced and less, as `breakdown.area` takes them
pub(crate) fn radius_breakpoints(base: f64, inc: f64, more: f64, radius_mod: f64) -> Option<[f64; 4]> {
    let radius = calc_radius(base, calc_area_mod(inc, more), radius_mod);
    if radius <= 0.0 {
        return None;
    }
    let find = |f: &dyn Fn(f64) -> f64, up: bool| {
        let mut n = 0.0;
        loop {
            n += 1.0;
            let r = f(n);
            if (up && r > radius) || (!up && r < radius) {
                return n;
            }
        }
    };
    let r = |i: f64, m: f64| calc_radius(base, calc_area_mod(i, m), radius_mod);
    Some([
        find(&|n| r(inc + n / 100.0, more), true),
        find(&|n| r(inc, more * (1.0 + n / 100.0)), true),
        find(&|n| r(inc - n / 100.0, more), false),
        find(&|n| r(inc, more * (1.0 - n / 100.0)), false),
    ])
}

/// `setMoltenStrikeTertiaryRadiusBreakdown`
fn molten_strike_breakdown(
    deadzone: f64,
    base: f64,
    label: Option<&str>,
    [inc_area, more_area, inc_spd, more_spd]: [f64; 4],
) -> Breakdown {
    let calc = |a: [f64; 4]| {
        let area_mod = round(round((inc_area + a[0] / 100.0) * more_area * (1.0 + a[1] / 100.0), 10.0), 2.0);
        let speed_mod = round(round((inc_spd + a[2] / 100.0) * more_spd * (1.0 + a[3] / 100.0), 10.0), 2.0);
        (molten_strike_tertiary_radius(base, deadzone, area_mod, speed_mod), area_mod, speed_mod)
    };
    let (dist, area_mod, speed_mod) = calc([0.0; 4]);
    let mut l: Vec<String> = label.into_iter().map(String::from).collect();
    l.push(" = (sqrt(R*R*a - r*r*(a-1)) - r) * s + r".into());
    l.push(sf!(" = %d", dist));
    if dist > 0.0 {
        let find = |sign: f64, k: usize| {
            let mut a = [0.0; 4];
            loop {
                a[k] += sign;
                let d = calc(a).0;
                if d != dist || d == 0.0 {
                    return a[k] * sign;
                }
            }
        };
        for (fmt, sign, k) in [
            ("^8Next AoE breakpoint: %d%% increased or %d%% more", 1.0, 0),
            ("^8Next Proj. Speed breakpoint: %d%% increased or %d%% more", 1.0, 2),
            ("^8Previous AoE breakpoint: %d%% increased or %d%% more", -1.0, 0),
            ("^8Previous Proj. Speed breakpoint: %d%% increased or %d%% more", -1.0, 2),
        ] {
            l.push(sf!(fmt, find(sign, k), find(sign, k + 1)));
        }
    }
    let row = |n: &str, v: String, d: &str| -> Row {
        vec![("name", n.into()), ("value", v.into()), ("description", d.into())]
    };
    Breakdown {
        lines: l,
        label: Some("Inputs".into()),
        rows: Some(vec![
            row("r", sf!("%d", deadzone), "fixed deadzone radius"),
            row("R", sf!("%d", base), "base outer radius"),
            row("a", sf!("%.2f", area_mod), "net AoE multiplier (scales area)"),
            row("s", sf!("%.2f", speed_mod), "net projectile speed multiplier (scales range)"),
        ]),
        cols: Some(vec![
            Col::new("Variable", "name"),
            Col::new("Value", "value"),
            Col::new("Description", "description"),
        ]),
        radius: Some(dist),
        ..Breakdown::default()
    }
}

impl<'a> Perform<'a> {
    /// `calcSkillCooldown(skillModList, skillCfg, skillData)`: the
    /// cooldown, whether it is rounded to server ticks and the added base
    pub(crate) fn skill_cooldown(
        &self,
        list: StoreId,
        cfg: &Cfg,
        i: usize,
    ) -> Result<(f64, bool, f64)> {
        let c = Some(cfg);
        let over = self.c.override_num(list, c, &["CooldownRecovery"])?;
        let added = self.c.base(list, c, &["CooldownRecovery"])?;
        let cooldown = match over {
            Some(v) => v,
            None => {
                (self.sd_num(i, "cooldown").unwrap_or(0.0) + added)
                    / m_max(0.0, self.c.calc_mod(list, c, &["CooldownRecovery"])?)
            }
        };
        if self.sd_num(i, "storedUses").is_some_and(|u| u > 1.0)
            || self.sd_num(i, "VaalStoredUses").is_some_and(|u| u > 1.0)
            || self.c.base(list, c, &["AdditionalCooldownUses"])? > 0.0
        {
            return Ok((cooldown, false, added));
        }
        let tick = self.misc_data("ServerTickRate");
        Ok(((cooldown * tick).ceil() / tick, true, added))
    }

    /// `calcs.calcTotemLife(env, activeSkill)`: life and its modifier
    pub(crate) fn totem_life(&self, i: usize) -> Result<(f64, f64)> {
        let list = self.lists[i].list;
        let cfg = self.cfg(i);
        let life_mod = self.c.calc_mod(list, Some(&cfg), &["TotemLife"])?;
        let level = self.sd_num(i, "totemLevel").unwrap_or(f64::NAN);
        let base = self
            .data()
            .table("monsterAllyLifeTable", &format!("{}", level as i64))
            .ok_or_else(|| format!("no ally life for totem level {level}"))?;
        let id = self.mods(i).skill_totem_id.unwrap_or(f64::NAN);
        let mult = self
            .data()
            .table("totemLifeMult", &format!("{}", id as i64))
            .ok_or_else(|| format!("no totem life multiplier for {id}"))?;
        Ok((round((base * mult).floor() * life_mod, 0.0), life_mod))
    }

    /// The skill type stats: minions, chains, projectiles, range, area,
    /// auras, reservations, curses, warcries, traps, mines, totems, brands
    fn offence_skill_types(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb) = (o.skill, o.list, o.actor, o.edb);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let set = |p: &mut Self, k: &str, v: f64| p.c.set_out(actor, k, v);
        if let Some(m) = self.mods(i).minion.clone() {
            let limit = self
                .data()
                .minions
                .get(&m.typ)
                .and_then(|d| d.str("limit"))
                .map(str::to_string);
            if let Some(limit) = limit {
                let pdb = self.pdb();
                let v = match self.c.override_num(pdb, None, &[&limit])? {
                    Some(v) => v,
                    None => {
                        self.c.val(list, &limit, c)?
                            * self.c.more(list, c, &["ActiveMinionLimit"])?
                    }
                };
                set(self, "ActiveMinionLimit", v.floor());
            }
            let mut n = self.c.val(list, "MinionPerCastCount", c)?.floor();
            if n == 0.0 {
                n = 1.0;
            }
            set(self, "SummonedMinionsPerCast", n);
        }
        if self.has_flag(i, "chaining") {
            if self.c.flag(list, c, &["CannotChain"])?
                || self.c.flag(list, c, &["NoAdditionalChains"])?
            {
                self.c
                    .set_out_val(actor, "ChainMaxString", OutVal::Str("Cannot chain".into()));
            } else {
                let mut names = vec!["ChainCountMax"];
                if !self.has_flag(i, "projectile") {
                    names.push("BeamChainCountMax");
                }
                let mut max = self.c.base(list, c, &names)?;
                if self
                    .c
                    .flag(list, c, &["AdditionalProjectilesAddChainsInstead"])?
                {
                    let proj = if self.c.flag(list, c, &["SingleProjectile"])? {
                        0.0
                    } else {
                        ((self.c.base(list, c, &["ProjectileCount"])? - 1.0)
                            * self.c.more(list, c, &["ProjectileCount"])?)
                        .floor()
                    };
                    max += proj;
                }
                max *= self.c.more(list, c, &["ChainCountMax"])?;
                set(self, "ChainMax", max);
                set(self, "ChainMaxString", max);
                let chain = m_min(max, self.c.base(list, c, &["ChainCount"])?);
                set(self, "Chain", chain);
                set(self, "ChainRemaining", m_max(0.0, max - chain));
            }
        }
        if self.has_flag(i, "projectile") {
            let ap = self.mf("Attack") | self.mf("Projectile");
            if self.c.flag(list, None, &["PointBlank"])? {
                self.c.add(
                    list,
                    new_mod(
                        "Damage",
                        More,
                        30.0,
                        Some("Point Blank"),
                        ap,
                        0,
                        vec![Tag {
                            kind: crate::mods::tag::TagKind::DistanceRamp(vec![
                                (10.0, 1.0),
                                (35.0, 0.0),
                                (120.0, -1.0),
                            ]),
                            global_limit: None,
                        }],
                    ),
                );
            }
            if self.c.flag(list, None, &["FarShot"])? {
                self.c.add(
                    list,
                    new_mod(
                        "Damage",
                        More,
                        100.0,
                        Some("Far Shot"),
                        ap,
                        0,
                        vec![Tag {
                            kind: crate::mods::tag::TagKind::DistanceRamp(vec![
                                (10.0, -0.2),
                                (25.0, 0.0),
                                (70.0, 0.6),
                            ]),
                            global_limit: None,
                        }],
                    ),
                );
            }
            if self.c.flag(list, c, &["NoAdditionalProjectiles"])?
                || self.c.flag(list, c, &["SingleProjectile"])?
            {
                set(self, "ProjectileCount", 1.0);
            } else {
                let min = self.c.base(list, c, &["ProjectileCountMinimum"])?;
                let max = self
                    .c
                    .override_num(list, c, &["ProjectileCountMaximum"])?
                    .unwrap_or(f64::INFINITY);
                let proj = (self.c.base(list, c, &["ProjectileCount"])?
                    * self.c.more(list, c, &["ProjectileCount"])?)
                .floor();
                set(self, "ProjectileCount", m_max(m_min(proj, max), min));
            }
            if self
                .c
                .flag(list, c, &["AdditionalProjectilesAddBouncesInstead"])?
            {
                let base = if self.c.flag(list, c, &["SingleProjectile"])? {
                    0.0
                } else {
                    self.c.base(list, c, &["ProjectileCount"])?
                        + self.c.base(list, c, &["BounceCount"])?
                        - 1.0
                };
                let more = self.c.more(list, c, &["ProjectileCount"])?;
                set(self, "BounceCount", (base * more).floor());
            }
            if self.c.flag(list, c, &["CannotSplit"])? || self.has_type(i, "ProjectileNumber") {
                if self.input.breakdown {
                    let n = self.c.base(list, c, &["SplitCount"])?
                        + self.c.base(edb, c, &["SelfSplitCount"])?;
                    if n > 0.0 {
                        self.c.set_out_val(
                            actor,
                            "SplitCountString",
                            OutVal::Str("Cannot split".into()),
                        );
                    }
                }
            } else {
                let mut n = self.c.base(list, c, &["SplitCount"])?
                    + self.c.base(edb, c, &["SelfSplitCount"])?;
                if self
                    .c
                    .flag(list, c, &["AdditionalProjectilesAddSplitsInstead"])?
                {
                    n += if self.c.flag(list, c, &["SingleProjectile"])? {
                        0.0
                    } else {
                        ((self.c.base(list, c, &["ProjectileCount"])? - 1.0)
                            * self.c.more(list, c, &["ProjectileCount"])?)
                        .floor()
                    };
                }
                if self
                    .c
                    .flag(list, c, &["AdditionalChainsAddSplitsInstead"])?
                {
                    n += self.c.base(list, c, &["ChainCountMax"])?;
                }
                set(self, "SplitCount", n);
                set(self, "SplitCountString", n);
            }
            if self.c.flag(list, c, &["CannotFork"])? {
                self.c
                    .set_out_val(actor, "ForkCountString", OutVal::Str("Cannot fork".into()));
            } else if self.c.flag(list, c, &["ForkOnce"])? {
                self.set_flag(i, "forking", true);
                let cap = if self.c.flag(list, c, &["ForkTwice"])? {
                    2.0
                } else {
                    1.0
                };
                let max = m_min(self.c.base(list, c, &["ForkCountMax"])?, cap);
                set(self, "ForkCountMax", max);
                let forked = m_min(max, self.c.base(list, c, &["ForkedCount"])?);
                set(self, "ForkedCount", forked);
                set(self, "ForkCountString", max);
                set(self, "ForkRemaining", m_max(0.0, max - forked));
            } else {
                self.c
                    .set_out_val(actor, "ForkCountString", OutVal::Str("0".into()));
            }
            if self.c.flag(list, c, &["CannotPierce"])? {
                set(self, "PierceCount", 0.0);
                self.c.set_out_val(
                    actor,
                    "PierceCountString",
                    OutVal::Str("Cannot pierce".into()),
                );
            } else {
                let n = if self.c.flag(list, c, &["PierceAllTargets"])?
                    || self.c.flag(edb, None, &["AlwaysPierceSelf"])?
                {
                    self.c.set_out_val(
                        actor,
                        "PierceCountString",
                        OutVal::Str("All targets".into()),
                    );
                    100.0
                } else {
                    let n = self.c.base(list, c, &["PierceCount"])?;
                    set(self, "PierceCountString", n);
                    n
                };
                set(self, "PierceCount", n);
                if n > 0.0 {
                    self.set_flag(i, "piercing", true);
                }
                let pierced = m_min(n, self.c.base(list, c, &["PiercedCount"])?);
                set(self, "PiercedCount", pierced);
            }
            let v = self.c.calc_mod(list, c, &["ProjectileSpeed"])?;
            set(self, "ProjectileSpeedMod", v);
            self.bd_mod(actor, "ProjectileSpeedMod", list, c, &["ProjectileSpeed"])?;
        }
        if self.has_flag(i, "melee") {
            for n in 0..2 {
                if !self.has_flag(
                    i,
                    if n == 0 {
                        "weapon1Attack"
                    } else {
                        "weapon2Attack"
                    },
                ) {
                    continue;
                }
                let range = match self.weapon_num(actor, n, "range") {
                    Some(r) => {
                        let wc = self.mods(i).weapon_cfgs[n].clone();
                        r + self.c.base(list, wc.as_ref(), &["MeleeWeaponRange"])?
                            + 10.0 * self.c.base(list, wc.as_ref(), &["MeleeWeaponRangeMetre"])?
                    }
                    None => {
                        6.0 + self.c.base(list, c, &["UnarmedRange"])?
                            + 10.0 * self.c.base(list, c, &["UnarmedRangeMetre"])?
                    }
                };
                self.weapon_range[n] = Some(range);
            }
            if self.has_type(i, "MeleeSingleTarget") {
                let mut range: f64 = 100.0;
                for n in 0..2 {
                    if self.has_flag(
                        i,
                        if n == 0 {
                            "weapon1Attack"
                        } else {
                            "weapon2Attack"
                        },
                    ) {
                        range = m_min(range, self.weapon_range[n].unwrap_or(f64::NAN));
                    }
                }
                set(self, "WeaponRange", range + 2.0);
                set(self, "WeaponRangeMetre", (range + 2.0) / 10.0);
                if let Some(b) = self.bd(actor) {
                    b.set(
                        "WeaponRange",
                        Breakdown {
                            radius: Some(range + 2.0),
                            ..Breakdown::default()
                        },
                    );
                }
                let v = 1.0 + self.c.base(list, c, &["AdditionalStrikeTarget"])?;
                set(self, "StrikeTargets", v);
            }
        }
        if self.has_flag(i, "area")
            || self.sd(i, "radius").is_some_and(ModValue::truthy)
            || (self.has_flag(i, "mine") && self.has_type(i, "Aura"))
        {
            self.area_of_effect(o)?;
        }
        if self.has_type(i, "Aura") {
            let mut names = vec!["AuraEffect"];
            if !(self.sd_truthy(i, "auraCannotAffectSelf")
                || self.has_type(i, "AuraAffectsEnemies"))
            {
                names.push("SkillAuraEffectOnSelf");
            }
            let v = self.c.calc_mod(list, c, &names)?;
            set(self, "AuraEffectMod", v);
            self.bd_mod(actor, "AuraEffectMod", list, c, &names)?;
        }
        if self.has_type(i, "HasReservation") && !self.has_type(i, "ReservationBecomesCost") {
            for pool in ["Life", "Mana"] {
                let k = format!("{pool}ReservedMod");
                set(self, &k, 0.0);
                let support = self.c.calc_mod(list, c, &["SupportManaMultiplier"])?;
                let reserved =
                    self.c
                        .calc_mod(list, c, &[&format!("{pool}Reserved"), "Reserved"])?;
                if support > 0.0 && reserved > 0.0 {
                    let eff = self.c.calc_mod(
                        list,
                        c,
                        &[
                            &format!("{pool}ReservationEfficiency"),
                            "ReservationEfficiency",
                        ],
                    )?;
                    set(self, &k, reserved * floor(support, 4.0) / m_max(0.0, eff));
                }
                if self.has_bd(actor) {
                    let names = [
                        &format!("{pool}Reserved") as &str,
                        "Reserved",
                        "SupportManaMultiplier",
                    ];
                    let inc = self.c.inc(list, c, &names)?;
                    let more = self.c.more(list, c, &names)?;
                    if inc != 0.0 && more != 1.0 {
                        let eff = self.c.calc_mod(
                            list,
                            c,
                            &[
                                &format!("{pool}ReservationEfficiency"),
                                "ReservationEfficiency",
                            ],
                        )?;
                        let total = self.c.out(actor, &k).unwrap_or(f64::NAN);
                        let l = vec![
                            sf!("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
                            sf!("x %.2f ^8(more/less)", more),
                            sf!("/ %.2f ^8(reservation efficiency)", eff),
                            sf!("= %.2f", total),
                        ];
                        self.bd(actor).unwrap().set_lines(k, l);
                    }
                }
            }
        }
        if self.has_type(i, "Hex") || self.has_type(i, "Mark") {
            let v = self.c.calc_mod(list, c, &["CurseEffect"])?;
            set(self, "CurseEffectMod", v);
            self.bd_mod(actor, "CurseEffectMod", list, c, &["CurseEffect"])?;
        }
        if self.has_type(i, "Warcry") {
            let full = self.skill_duration(list, c, i)?;
            let over = self.c.override_num(list, c, &["CooldownRecovery"])?;
            let cooldown = match over {
                Some(v) => v,
                None => {
                    (self.sd_num(i, "cooldown").unwrap_or(f64::NAN)
                        + self.c.base(list, c, &["CooldownRecovery"])?)
                        / self.c.calc_mod(list, c, &["CooldownRecovery"])?
                }
            };
            let max_hit = self.c.flag(self.pdb(), None, &["Condition:WarcryMaxHit"])?;
            let uptime = if max_hit {
                1.0
            } else {
                m_min(full / cooldown, 1.0)
            };
            let effect = self.c.calc_mod(list, c, &["WarcryEffect", "BuffEffect"])?;
            set(self, "WarcryEffectMod", effect * uptime);
            if let Some(b) = self.bd(actor) {
                let mut l = vec![sf!("%.2f ^8(effect modifiers)", effect)];
                if max_hit || uptime != 1.0 {
                    l.push(if max_hit {
                        "* 100% uptime (WarcryMaxHit Override)".into()
                    } else {
                        sf!("* %.2f%% ^8(uptime)", uptime * 100.0)
                    });
                    l.push(sf!("= %.1f%%", effect * uptime * 100.0));
                }
                b.set_lines("WarcryEffectMod", l);
            }
        }
        if self.has_type(i, "Link") {
            let v = self.c.calc_mod(list, c, &["LinkEffect", "BuffEffect"])?;
            set(self, "LinkEffectMod", v);
            self.bd_mod(actor, "LinkEffectMod", list, c, &["LinkEffect", "BuffEffect"])?;
        }
        if self.has_type(i, "Buff") && self.has_type(i, "Herald") {
            let v = self
                .c
                .calc_mod(list, c, &["BuffEffect", "BuffEffectOnSelf"])?;
            set(self, "HeraldBuffEffectMod", v);
            let names = ["BuffEffect", "BuffEffectOnSelf"];
            self.bd_mod(actor, "HeraldBuffEffectMod", self.pdb(), c, &names)?;
        }
        let has_cooldown = self.sd(i, "trapCooldown").is_some_and(ModValue::truthy)
            || self.sd(i, "cooldown").is_some_and(ModValue::truthy);
        if (self.has_flag(i, "trap") || self.has_flag(i, "mine")) && !has_cooldown {
            self.set_flag(i, "notAverage", true);
            self.set_flag(i, "showAverage", false);
            self.sd_set(i, "showAverage", false);
        }
        let action = self.c.out(actor, "ActionSpeedMod").unwrap_or(f64::NAN);
        let tick = self.misc_data("ServerTickRate");
        let bd = self.has_bd(actor);
        if self.has_flag(i, "trap") {
            let mut base_speed = 1.0 / self.c.base(list, c, &["TrapThrowingTime"])?;
            let time_mod = self.c.calc_mod(list, c, &["SkillTrapThrowingTime"])?;
            if time_mod > 0.0 {
                base_speed *= 1.0 / time_mod;
            }
            let mut speed = base_speed * self.c.calc_mod(list, c, &["TrapThrowingSpeed"])? * action;
            let mut count = self.c.val(list, "TrapThrowCount", c)?;
            if has_cooldown {
                count = 1.0;
            }
            let count = self
                .c
                .override_num(self.pdb(), None, &["TrapThrowCount"])?
                .unwrap_or(count);
            set(self, "TrapThrowCount", count);
            speed = m_min(speed, tick);
            set(self, "TrapThrowingSpeed", speed);
            set(self, "TrapThrowingTime", 1.0 / speed);
            self.sd_set(i, "timeOverride", 1.0 / speed / count);
            if bd {
                let inc = 1.0 + self.c.inc(list, c, &["TrapThrowingSpeed"])? / 100.0;
                let more = self.c.more(list, c, &["TrapThrowingSpeed"])?;
                let mut l = Vec::new();
                multi_chain(
                    &mut l,
                    Chain {
                        label: Some("Throwing rate:"),
                        base: Some(f("%.2f ^8(base throwing rate)", base_speed)),
                        factors: vec![
                            f("%.2f ^8(increased/reduced throwing speed)", inc),
                            f("%.2f ^8(more/less throwing speed)", more),
                            f("%.2f ^8(action speed modifier)", action),
                        ],
                        total: Some(sf!("= %.2f ^8per second", speed)),
                        ..Chain::default()
                    },
                );
                let b = self.bd(actor).unwrap();
                b.set_lines("TrapThrowingSpeed", l);
                if time_mod > 0.0 {
                    b.set_lines("TrapThrowingTime", throwing_time(speed, time_mod));
                }
            }
            let base_cooldown = self
                .sd_num(i, "trapCooldown")
                .or_else(|| self.sd_num(i, "cooldown"));
            if base_cooldown.is_some() || self.c.base(list, c, &["CooldownRecovery"])? != 0.0 {
                let cd = match base_cooldown {
                    Some(b) => {
                        let cd = b / self.c.calc_mod(list, c, &["CooldownRecovery"])?;
                        (cd * tick).ceil() / tick
                    }
                    None => self.skill_cooldown(list, &cfg, i)?.0,
                };
                set(self, "TrapCooldown", cd);
                if bd {
                    let inc = 1.0 + self.c.inc(list, c, &["CooldownRecovery"])? / 100.0;
                    let l = vec![
                        sf!("%.2fs ^8(base)", base_cooldown.unwrap_or(4.0)),
                        sf!("/ %.2f ^8(increased/reduced cooldown recovery)", inc),
                        "rounded up to nearest server tick".into(),
                        sf!("= %.3fs", cd),
                    ];
                    self.bd(actor).unwrap().set_lines("TrapCooldown", l);
                }
            }
            let inc = 1.0 + self.c.inc(list, c, &["TrapTriggerAreaOfEffect"])? / 100.0;
            let more = self.c.more(list, c, &["TrapTriggerAreaOfEffect"])?;
            let area = calc_area_mod(inc, more);
            let rb = self.misc_data("TrapTriggerRadiusBase");
            let r = calc_radius(rb, area, 1.0);
            set(self, "TrapTriggerRadius", r);
            set(self, "TrapTriggerRadiusMetre", r / 10.0);
            if let Some(b) = self.bd(actor) {
                let bp = radius_breakpoints(rb, inc, more, 1.0);
                b.set("TrapTriggerRadius", area_bd(rb, area, r, bp, None, None));
            }
        } else if self.sd(i, "cooldown").is_some_and(ModValue::truthy)
            || self.c.base(list, c, &["CooldownRecovery"])? != 0.0
        {
            let (cd, rounded, added) = self.skill_cooldown(list, &cfg, i)?;
            set(self, "Cooldown", cd);
            if bd {
                let inc = 1.0 + self.c.inc(list, c, &["CooldownRecovery"])? / 100.0;
                let mut l = vec![
                    sf!("%.2fs ^8(base)", self.sd_num(i, "cooldown").unwrap_or(added)),
                    sf!("/ %.2f ^8(increased/reduced cooldown recovery)", inc),
                ];
                if rounded {
                    l.push("rounded up to nearest server tick".into());
                }
                l.push(sf!("= %.3fs", cd));
                self.bd(actor).unwrap().set_lines("Cooldown", l);
            }
        }
        if let Some(base) = self.sd_num(i, "storedUses") {
            let add = self
                .c
                .base(list, c, &["AdditionalCooldownUses", "AdditionalUses"])?;
            set(self, "StoredUses", base + add);
            if let Some(b) = self.bd(actor) {
                let s = |n: f64| if n == 1.0 { "" } else { "s" };
                let mut l = vec![sf!("%d ^8(skill use%s)", base, s(base))];
                if add != 0.0 {
                    l.push(sf!("+ %d ^8(additional use%s)", add, s(add)));
                    l.push(sf!("= %d ^8(total use%s)", base + add, s(base + add)));
                }
                b.set_lines("StoredUses", l);
            }
        }
        if self.has_flag(i, "mine") {
            let mut base_speed = 1.0 / self.c.base(list, c, &["MineLayingTime"])?;
            let time_mod = self.c.calc_mod(list, c, &["SkillMineThrowingTime"])?;
            if time_mod > 0.0 {
                base_speed *= 1.0 / time_mod;
            }
            let mut speed = base_speed * self.c.calc_mod(list, c, &["MineLayingSpeed"])? * action;
            let mut count = self.c.val(list, "MineThrowCount", c)?;
            if has_cooldown {
                count = 1.0;
            }
            let count = self
                .c
                .override_num(self.pdb(), None, &["MineThrowCount"])?
                .unwrap_or(count);
            set(self, "MineThrowCount", count);
            if count >= 1.0 {
                speed /= 1.0 + (count - 1.0) * 0.1;
            }
            speed = m_min(speed, tick);
            set(self, "MineLayingSpeed", speed);
            let time = 1.0 / speed;
            set(self, "MineLayingTime", time);
            let over = if self.has_flag(i, "trap") {
                time / count / self.c.out(actor, "TrapThrowCount").unwrap_or(f64::NAN)
            } else {
                time / count
            };
            self.sd_set(i, "timeOverride", over);
            if bd {
                let inc = 1.0 + self.c.inc(list, c, &["MineLayingSpeed"])? / 100.0;
                let more = self.c.more(list, c, &["MineLayingSpeed"])?;
                let mut l = Vec::new();
                multi_chain(
                    &mut l,
                    Chain {
                        label: Some("Throwing rate:"),
                        base: Some(f("%.2f ^8(base throwing rate)", base_speed)),
                        factors: vec![
                            f("%.2f ^8(increased/reduced throwing speed)", inc),
                            f("%.2f ^8(more/less throwing speed)", more),
                            f("%.2f ^8(action speed modifier)", action),
                            f(
                                "%.2f ^8(additional mine thrown)",
                                1.0 / (1.0 + (count - 1.0) * 0.1),
                            ),
                        ],
                        total: Some(sf!("= %.2f ^8per second", speed)),
                        ..Chain::default()
                    },
                );
                let b = self.bd(actor).unwrap();
                b.set_lines("MineLayingTime", l);
                if time_mod > 0.0 {
                    b.set_lines("MineThrowingTime", throwing_time(speed, time_mod));
                }
            }
            let inc = 1.0 + self.c.inc(list, c, &["MineDetonationAreaOfEffect"])? / 100.0;
            let more = self.c.more(list, c, &["MineDetonationAreaOfEffect"])?;
            let area = calc_area_mod(inc, more);
            let rb = self.misc_data("MineDetonationRadiusBase");
            let r = calc_radius(rb, area, 1.0);
            set(self, "MineDetonationRadius", r);
            set(self, "MineDetonationRadiusMetre", r / 10.0);
            if let Some(b) = self.bd(actor) {
                let bp = radius_breakpoints(rb, inc, more, 1.0);
                b.set("MineDetonationRadius", area_bd(rb, area, r, bp, None, None));
            }
            if self.has_type(i, "Aura") {
                let rb = self.misc_data("MineAuraRadiusBase");
                let area = self.c.out(actor, "AreaOfEffectMod").unwrap_or(f64::NAN);
                let r = calc_radius(rb, area, 1.0);
                set(self, "MineAuraRadius", r);
                set(self, "MineAuraRadiusMetre", r / 10.0);
                if bd {
                    let inc = 1.0 + self.c.inc(list, c, &["AreaOfEffect"])? / 100.0;
                    let more = self.c.more(list, c, &["AreaOfEffect"])?;
                    let bp = radius_breakpoints(rb, inc, more, 1.0);
                    let b = area_bd(rb, area, r, bp, None, None);
                    self.bd(actor).unwrap().set("MineAuraRadius", b);
                }
            }
        }
        if self.has_flag(i, "totem") {
            let base_speed = if self.has_flag(i, "ballista") {
                1.0 / self.c.base(list, c, &["BallistaPlacementTime"])?
            } else {
                1.0 / self.c.base(list, c, &["TotemPlacementTime"])?
            };
            let speed = base_speed * self.c.calc_mod(list, c, &["TotemPlacementSpeed"])? * action;
            set(self, "TotemPlacementSpeed", speed);
            set(self, "TotemPlacementTime", 1.0 / speed);
            if bd {
                let inc = 1.0 + self.c.inc(list, c, &["TotemPlacementSpeed"])? / 100.0;
                let more = self.c.more(list, c, &["TotemPlacementSpeed"])?;
                let mut l = Vec::new();
                multi_chain(
                    &mut l,
                    Chain {
                        label: Some("Placement speed:"),
                        base: Some(f("%.2f ^8(base placement speed)", base_speed)),
                        factors: vec![
                            f("%.2f ^8(increased/reduced placement speed)", inc),
                            f("%.2f ^8(more/less placement speed)", more),
                            f("%.2f ^8(action speed modifier)", action),
                        ],
                        total: Some(sf!("= %.2f ^8per second", speed)),
                        ..Chain::default()
                    },
                );
                self.bd(actor).unwrap().set_lines("TotemPlacementTime", l);
            }
            let limit = self
                .c
                .base(list, c, &["ActiveTotemLimit", "ActiveBallistaLimit"])?;
            set(self, "ActiveTotemLimit", limit);
            let summoned = self
                .c
                .override_num(self.pdb(), None, &["TotemsSummoned"])?
                .unwrap_or(limit);
            set(self, "TotemsSummoned", summoned);
            if bd {
                let note = if self.input.config.contains_key("TotemsSummoned") {
                    " ^8(overridden from the Configuration tab)"
                } else {
                    " ^8(can be overridden in the Configuration tab)"
                };
                let l = vec![format!("Totems Summoned: {}{note}", crate::pat::num(summoned))];
                self.bd(actor).unwrap().set_lines("ActiveTotemLimit", l);
            }
            let (life, life_mod) = self.totem_life(i)?;
            set(self, "TotemLife", life);
            set(self, "TotemLifeMod", life_mod);
            for k in ["TotemEnergyShield", "TotemBlockChance", "TotemArmour"] {
                let v = self.c.base(list, c, &[k])?;
                set(self, k, v);
            }
            if bd {
                self.bd_mod(actor, "TotemLifeMod", list, c, &["TotemLife"])?;
                let num = crate::pat::num;
                let level = self.sd_num(i, "totemLevel").unwrap_or(f64::NAN);
                let base = self
                    .data()
                    .table("monsterAllyLifeTable", &format!("{}", level as i64))
                    .unwrap_or(f64::NAN);
                let id = self.mods(i).skill_totem_id.unwrap_or(f64::NAN);
                let mult = self
                    .data()
                    .table("totemLifeMult", &format!("{}", id as i64))
                    .unwrap_or(f64::NAN);
                let l = vec![
                    format!("Totem level: {}", num(level)),
                    format!(
                        "{} ^8(base life for a level {} monster)",
                        num(base),
                        num(level)
                    ),
                    format!("x {} ^8(life multiplier for this totem type)", num(mult)),
                    format!("x {} ^8(totem life modifier)", num(life_mod)),
                    format!("= {}", num(life)),
                ];
                self.bd(actor).unwrap().set_lines("TotemLife", l);
                for k in ["TotemEnergyShield", "TotemBlockChance", "TotemArmour"] {
                    self.bd_mod(actor, k, list, c, &[k])?;
                }
            }
        }
        if self.has_type(i, "Brand") {
            let r = self.misc_data("BrandAttachmentRangeBase")
                * self.c.calc_mod(list, c, &["BrandAttachmentRange"])?;
            set(self, "BrandAttachmentRange", r);
            set(self, "BrandAttachmentRangeMetre", r / 10.0);
            if let Some(b) = self.bd(actor) {
                b.set(
                    "BrandAttachmentRange",
                    Breakdown {
                        radius: Some(r),
                        ..Breakdown::default()
                    },
                );
            }
            let v = self.c.base(list, c, &["ActiveBrandLimit"])?;
            set(self, "ActiveBrandLimit", v);
            match self.sd_num(i, "attachedBrandCount") {
                Some(n) => set(self, "AttachedBrandCount", n),
                None => self.c.clear_out(actor, "AttachedBrandCount"),
            }
        }
        if self.has_flag(i, "warcry") {
            let base_speed = 1.0 / self.c.base(list, c, &["WarcryCastTime"])?;
            let mut t = base_speed
                * self.c.calc_mod(list, c, &["WarcrySpeed"])?
                * self.action_speed_mod(actor)?;
            t = m_min(t, tick);
            t = 1.0 / t;
            if self.c.flag(list, c, &["InstantWarcry"])?
                || self.sd_truthy(i, "triggeredByAutoexertion")
            {
                t = 0.0;
            }
            set(self, "WarcryCastTime", t);
        }
        if self.has_flag(i, "corpse") {
            let level = self.c.base(list, c, &["CorpseLevel"])?;
            set(self, "CorpseLevel", level);
            let life = self
                .data()
                .table("monsterLifeTable", &format!("{}", level as i64))
                .ok_or_else(|| format!("no monster life for level {level}"))?;
            let variety = match self.sd(i, "corpseMonsterVariety") {
                Some(ModValue::Str(v)) => self.data().table("monsterVarietyLifeMult", v),
                _ => None,
            };
            let map = self.data().table(
                "mapLevelLifeMult",
                &format!("{}", self.env.enemy_level as i64),
            );
            let base = life * variety.unwrap_or(1.0) * map.unwrap_or(1.0);
            set(self, "BaseCorpseLife", base);
            let inc = 1.0 + self.c.inc(list, c, &["CorpseLife"])? / 100.0;
            set(self, "CorpseLifeInc", inc);
            set(self, "CorpseLife", base * inc);
            if bd {
                let name = self.sd_str(i, "corpseMonsterVariety");
                let enemy_level = self.env.enemy_level;
                let l = vec![
                    sf!("%d ^8(base life of a level %d monster)", life, level),
                    sf!(
                        "x %.2f ^8(%s variety multiplier)",
                        variety.unwrap_or(1.0),
                        name.as_deref().unwrap_or("nil")
                    ),
                    sf!(
                        "x %.2f ^8(map level %d monster life multiplier from config)",
                        map.unwrap_or(1.0),
                        enemy_level
                    ),
                    sf!(" = %d ^8(base corpse life)", base),
                    String::new(),
                    sf!("x %.2f ^8(corpse maximum life increases)", inc),
                    sf!(" = %d", base * inc),
                ];
                self.bd(actor).unwrap().set_lines("CorpseLife", l);
            }
        }
        Ok(())
    }

    /// Skill durations and uptimes
    fn offence_durations(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb) = (o.skill, o.list, o.actor, o.edb);
        let bd = self.has_bd(actor);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let tick = self.misc_data("ServerTickRate");
        let mut debuff_mult = 1.0;
        if self.env.mode.effective {
            debuff_mult = 1.0
                / m_max(
                    self.misc_data("BuffExpirationSlowCap"),
                    self.c.calc_mod(edb, c, &["BuffExpireFaster"])?,
                );
        }
        o.debuff_duration_mult = debuff_mult;
        let mine = self.sd_truthy(i, "mineDurationAppliesToSkill");
        let with_mine = |mut v: Vec<&'static str>| {
            if mine {
                v.push("MineDuration");
            }
            v
        };
        let mod_lines = |p: &Self, names: &[&str], label: &str| -> Result<Option<Vec<String>>> {
            let l = crate::breakdown::mod_(p.c.inc(list, c, names)?, p.c.more(list, c, names)?);
            Ok(l.map(|mut l| {
                if !label.is_empty() {
                    l.insert(0, label.to_string());
                }
                l
            }))
        };
        let duration_lines = |base: f64, m: f64, show_mod: bool, debuff: bool, total: f64| {
            let mut l = vec![sf!("%.2fs ^8(base)", base)];
            if show_mod {
                l.push(sf!("x %.4f ^8(duration modifier)", m));
            }
            if debuff && debuff_mult != 1.0 {
                l.push(sf!("/ %.3f ^8(debuff expires slower/faster)", 1.0 / debuff_mult));
            }
            l.push("rounded up to nearest server tick".into());
            l.push(sf!("= %.3fs", total));
            l
        };
        let names = with_mine(vec!["Duration", "PrimaryDuration"]);
        let dmod = m_max(self.c.calc_mod(list, c, &names)?, 0.0);
        self.c.set_out(actor, "DurationMod", dmod);
        if bd {
            let label = if self.sd(i, "durationSecondary").is_some_and(ModValue::truthy) {
                "Primary duration:"
            } else {
                ""
            };
            let l = mod_lines(self, &names, label)?;
            self.bd(actor).unwrap().set_opt("DurationMod", l);
        }
        let base = self.sd_num(i, "duration").unwrap_or(0.0)
            + self.c.base(list, c, &["Duration", "PrimaryDuration"])?;
        let permanent = match &self.mods(i).minion {
            Some(m) => self
                .c
                .flag(list, c, &[&format!("{}PermanentDuration", m.typ)])?,
            None => false,
        };
        if base > 0.0 && !permanent {
            let mut d = base * dmod;
            let debuff = self.sd_truthy(i, "debuff");
            if debuff {
                d *= debuff_mult;
            }
            let d = (d * tick).ceil() / tick;
            self.c.set_out(actor, "Duration", d);
            if bd && d != base {
                let l = duration_lines(base, dmod, dmod != 1.0, debuff, d);
                self.bd(actor).unwrap().set_lines("Duration", l);
            }
        }
        for (key, name, out, debuff, label) in [
            (
                "durationSecondary",
                "SecondaryDuration",
                "DurationSecondary",
                "debuffSecondary",
                "Secondary",
            ),
            (
                "durationTertiary",
                "TertiaryDuration",
                "DurationTertiary",
                "debuffTertiary",
                "Tertiary",
            ),
        ] {
            let base =
                self.sd_num(i, key).unwrap_or(0.0) + self.c.base(list, c, &["Duration", name])?;
            if base > 0.0 {
                let names = with_mine(vec!["Duration", name]);
                let m = m_max(self.c.calc_mod(list, c, &names)?, 0.0);
                let mut d = base * m;
                let debuff = self.sd_truthy(i, debuff);
                if debuff {
                    d *= debuff_mult;
                }
                let d = (d * tick).ceil() / tick;
                self.c.set_out(actor, out, d);
                if bd && d != base {
                    let ml = mod_lines(self, &names, &format!("{label} duration:"))?;
                    let l = duration_lines(base, m, dmod != 1.0, debuff, d);
                    let b = self.bd(actor).unwrap();
                    b.set_opt(format!("{label}DurationMod"), ml);
                    b.set_lines(out, l);
                }
            }
        }
        for (key, out) in [
            ("auraDuration", "AuraDuration"),
            ("reserveDuration", "ReserveDuration"),
        ] {
            let base = self.sd_num(i, key).unwrap_or(0.0);
            if base > 0.0 {
                let m = m_max(self.c.calc_mod(list, c, &["Duration"])?, 0.0);
                let d = (base * m * tick).ceil() / tick;
                self.c.set_out(actor, out, d);
                if bd && d != base {
                    let l = duration_lines(base, m, true, false, d);
                    self.bd(actor).unwrap().set_lines(out, l);
                }
            }
        }
        let base = self.sd_num(i, "soulPreventionDuration").unwrap_or(0.0);
        if base > 0.0 {
            let mut names = vec!["SoulGainPreventionDuration"];
            if self.sd_truthy(i, "skillEffectAppliesToSoulGainPrevention") {
                names.push("Duration");
            }
            if mine {
                names.push("MineDuration");
            }
            let m = m_max(self.c.calc_mod(list, c, &names)?, 0.0);
            let d = m_max((base * m * tick).ceil(), 1.0) / tick;
            self.c.set_out(actor, "SoulGainPreventionDuration", d);
            if bd && d != base {
                let l = duration_lines(base, m, true, false, d);
                self.bd(actor)
                    .unwrap()
                    .set_lines("SoulGainPreventionDuration", l);
            }
        }
        let m = m_max(self.c.calc_mod(list, c, &["TotemDuration"])?, 0.0);
        self.c.set_out(actor, "TotemDurationMod", m);
        let base = self.c.base(list, c, &["TotemDuration"])?;
        let d = (base * m * tick).ceil() / tick;
        self.c.set_out(actor, "TotemDuration", d);
        if bd {
            let ml = mod_lines(self, &["TotemDuration"], "")?;
            let l = duration_lines(base, m, m != 1.0, false, d);
            let b = self.bd(actor).unwrap();
            b.set_opt("TotemDurationMod", ml);
            b.set_lines("TotemDuration", l);
        }
        let m = m_max(self.c.calc_mod(list, c, &["ImpaleDuration"])?, 0.0);
        self.c.set_out(actor, "ImpaleDurationMod", m);
        let base = self.data().character_constants["impaled_debuff_base_duration_ms"] / 1000.0;
        let d = (base * m * tick * debuff_mult).ceil() / tick;
        self.c.set_out(actor, "ImpaleDuration", d);
        if bd {
            let ml = mod_lines(self, &["ImpaleDuration"], "")?;
            let mut l = vec![sf!("%.2fs ^8(base)", base)];
            if debuff_mult != 1.0 {
                l.push(sf!("/ %.3f ^8(debuff expires slower/faster)", 1.0 / debuff_mult));
            }
            if m != 1.0 {
                l.push(sf!("x %.4f ^8(duration modifier)", m));
            }
            l.push("rounded up to nearest server tick".into());
            l.push(sf!("= %.3fs", d));
            let b = self.bd(actor).unwrap();
            b.set_opt("ImpaleDurationMod", ml);
            b.set_lines("ImpaleDuration", l);
        }
        // Skill uptime
        if !self.has_type(i, "Vaal") {
            let cooldown = self.c.out(actor, "Cooldown").unwrap_or(0.0);
            for t in [
                "Duration",
                "DurationSecondary",
                "DurationTertiary",
                "AuraDuration",
                "reserveDuration",
            ] {
                let d = self.c.out(actor, t).unwrap_or(0.0);
                if d != 0.0 && cooldown != 0.0 {
                    let no_recovery = self.c.flag(list, c, &["NoCooldownRecoveryInDuration"])?;
                    let uptime = m_min(
                        if no_recovery {
                            d / (cooldown + d)
                        } else {
                            d / cooldown
                        },
                        1.0,
                    ) * 100.0;
                    let k = format!("{t}Uptime");
                    self.c.set_out(actor, &k, uptime);
                    if let Some(b) = self.bd(actor) {
                        let first = if no_recovery {
                            sf!("%.2fs / (%.2fs + %.2fs)", d, cooldown, d)
                        } else {
                            sf!("%.2fs / %.2fs", d, cooldown)
                        };
                        b.set_lines(k, vec![first, sf!("= %d%%", uptime)]);
                    }
                }
            }
        }
        Ok(())
    }
}

/// A resource cost being computed (`costs[resource]`)
#[derive(Clone, Debug)]
struct Cost {
    resource: &'static str,
    typ: &'static str,
    upfront: bool,
    percent: bool,
    unaffected: bool,
    text: &'static str,
    base_cost: f64,
    base_cost_raw: Option<f64>,
    total_cost: f64,
    base_cost_no_mult: f64,
    final_base_cost: f64,
}

fn costs() -> Vec<Cost> {
    let c = |resource, typ, upfront, percent, text| Cost {
        resource,
        typ,
        upfront,
        percent,
        unaffected: resource == "Soul",
        text,
        base_cost: 0.0,
        base_cost_raw: if resource == "Mana" { Some(0.0) } else { None },
        total_cost: 0.0,
        base_cost_no_mult: 0.0,
        final_base_cost: 0.0,
    };
    vec![
        c("Mana", "Mana", true, false, "mana"),
        c("Life", "Life", true, false, "life"),
        c("ES", "ES", true, false, "ES"),
        c("Soul", "Soul", true, false, "soul"),
        c("Rage", "Rage", true, false, "rage"),
        c("ManaPercent", "Mana", true, true, "mana"),
        c("LifePercent", "Life", true, true, "life"),
        c("ManaPerMinute", "Mana", false, false, "mana/s"),
        c("LifePerMinute", "Life", false, false, "life/s"),
        c("ManaPercentPerMinute", "Mana", false, true, "mana/s"),
        c("LifePercentPerMinute", "Life", false, true, "life/s"),
        c("ESPerMinute", "ES", false, false, "ES/s"),
        c("ESPercentPerMinute", "ES", false, true, "ES/s"),
    ]
}

impl<'a> Perform<'a> {
    /// The skill's costs
    fn offence_costs(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        if !self.c.flag(list, c, &["HasNoCost"])? {
            let mut mult = floor(self.c.more(list, c, &["SupportManaMultiplier"])?, 4.0);
            let additional_life = self.c.base(list, c, &["ManaCostAsLifeCost"])? / 100.0;
            let additional_es = self.c.base(list, c, &["ManaCostAsEnergyShieldCost"])? / 100.0;
            let hybrid_life = self.c.base(list, c, &["HybridManaAndLifeCost_Life"])? / 100.0;
            let mut costs = costs();
            let idx = |costs: &[Cost], r: &str| costs.iter().position(|x| x.resource == r).unwrap();
            let level_cost: Vec<(String, f64)> = self
                .effect_level(i)
                .map(|l| l.cost.clone())
                .unwrap_or_default();
            for k in 0..costs.len() {
                let resource = costs[k].resource;
                let skill_cost =
                    match self
                        .c
                        .override_num(list, c, &[&format!("Base{resource}CostOverride")])?
                    {
                        Some(v) => Some(v),
                        None => level_cost
                            .iter()
                            .find(|(r, _)| r == resource)
                            .map(|(_, v)| *v),
                    };
                let mut base_cost = round(
                    match skill_cost {
                        Some(v) => {
                            v / self
                                .data()
                                .cost_divisors
                                .get(resource)
                                .copied()
                                .unwrap_or(f64::NAN)
                        }
                        None => 0.0,
                    },
                    2.0,
                );
                let base_no_mult = self.c.base(list, c, &[&format!("{resource}CostNoMult")])?;
                let mut correction = 0.0;
                if costs[k].upfront {
                    base_cost += self.c.base(list, c, &[&format!("{resource}CostBase")])?;
                    costs[k].total_cost =
                        self.c
                            .base(list, c, &[&format!("{resource}Cost"), "Cost"])?;
                    if resource == "Mana"
                        && self.has_type(i, "ReservationBecomesCost")
                        && !costs[k].percent
                        && !self.c.flag(list, c, &["CostESInsteadOfManaOrLife"])?
                        && !self.c.flag(list, c, &["CostLifeInsteadOfMana"])?
                    {
                        let text = costs[k].text;
                        let level = |key: &str| self.effect_level(i).and_then(|l| l.field(key));
                        let flat_key = format!("{text}ReservationFlat");
                        let flat = self
                            .sd_num(i, &flat_key)
                            .or_else(|| level(&flat_key))
                            .unwrap_or(0.0);
                        base_cost += flat;
                        let pct_key = format!("{text}ReservationPercent");
                        let pct = self
                            .sd_num(i, &pct_key)
                            .or_else(|| level(&pct_key))
                            .unwrap_or(0.0);
                        let pool = self.c.out(actor, resource).unwrap_or(0.0);
                        base_cost += (pool * pct / 100.0).floor();
                        correction = round(pool * pct / 100.0 * mult, 0.0)
                            - (pool * pct / 100.0 * mult).floor();
                    }
                }
                let v = &mut costs[k];
                v.base_cost += base_cost;
                v.base_cost_no_mult += base_no_mult;
                v.final_base_cost =
                    ((v.base_cost * mult).floor() + v.base_cost_no_mult) + correction;
                v.base_cost_raw = v
                    .base_cost_raw
                    .map(|_| v.base_cost * mult + v.base_cost_no_mult + correction);
                match v.typ {
                    "Life" => {
                        let m = idx(&costs, &resource.replace("Life", "Mana"));
                        let mana_cost = format!("{}Cost", costs[m].resource);
                        if self.c.flag(list, c, &["CostLifeInsteadOfMana"])? {
                            if costs[k].upfront {
                                let total = self.c.base(list, c, &[&mana_cost])?;
                                costs[k].total_cost += m_max(0.0, total);
                                costs[m].total_cost -= total;
                            }
                            let mana = costs[m].clone();
                            let v = &mut costs[k];
                            v.base_cost += mana.base_cost;
                            v.base_cost_no_mult += mana.base_cost_no_mult;
                            v.final_base_cost += mana.final_base_cost;
                            let mana = &mut costs[m];
                            mana.base_cost = 0.0;
                            mana.base_cost_raw = Some(0.0);
                            mana.final_base_cost = 0.0;
                            mana.base_cost_no_mult = 0.0;
                        } else if (additional_life > 0.0 || hybrid_life > 0.0)
                            && !self.c.flag(list, c, &["CostESInsteadOfManaOrLife"])?
                        {
                            let mana = costs[m].clone();
                            let added = if costs[k].upfront && hybrid_life > 0.0 {
                                let total = self.c.base(list, c, &[&mana_cost])?;
                                round(m_max(0.0, total) * hybrid_life, 0.0)
                            } else {
                                0.0
                            };
                            let v = &mut costs[k];
                            v.base_cost = mana.base_cost;
                            v.final_base_cost +=
                                round(mana.final_base_cost * (hybrid_life + additional_life), 0.0);
                            v.total_cost += added;
                        }
                    }
                    "ES" => {
                        let m = idx(&costs, &resource.replace("ES", "Mana"));
                        let l = idx(&costs, &resource.replace("ES", "Life"));
                        if self.c.flag(list, c, &["CostESInsteadOfManaOrLife"])? {
                            if costs[k].upfront {
                                let mana = self.c.base(
                                    list,
                                    c,
                                    &[&format!("{}Cost", costs[m].resource)],
                                )?;
                                let life = self.c.base(
                                    list,
                                    c,
                                    &[&format!("{}Cost", costs[l].resource)],
                                )?;
                                costs[k].total_cost += m_max(0.0, mana) + m_max(0.0, life);
                                costs[m].total_cost -= mana;
                                costs[l].total_cost -= life;
                            }
                            let (mana, life) = (costs[m].clone(), costs[l].clone());
                            let v = &mut costs[k];
                            v.base_cost += mana.base_cost + life.base_cost;
                            v.base_cost_no_mult += mana.base_cost_no_mult + life.base_cost_no_mult;
                            v.final_base_cost += mana.final_base_cost + life.final_base_cost;
                            for x in [m, l] {
                                let x = &mut costs[x];
                                x.base_cost = 0.0;
                                x.base_cost_raw = Some(0.0);
                                x.final_base_cost = 0.0;
                                x.base_cost_no_mult = 0.0;
                            }
                        } else if additional_es > 0.0 {
                            let mana = costs[m].clone();
                            let v = &mut costs[k];
                            v.base_cost = mana.base_cost;
                            v.final_base_cost += round(mana.final_base_cost * additional_es, 0.0);
                        }
                    }
                    "Rage" if self.c.flag(list, c, &["CostRageInsteadOfSouls"])? => {
                        let s = idx(&costs, "Soul");
                        let soul = costs[s].clone();
                        let v = &mut costs[k];
                        v.base_cost = soul.base_cost;
                        v.base_cost_no_mult += soul.base_cost_no_mult;
                        v.final_base_cost = soul.base_cost;
                        mult = 1.0;
                        let soul = &mut costs[s];
                        soul.base_cost = 0.0;
                        soul.base_cost_no_mult = 0.0;
                        soul.final_base_cost = 0.0;
                    }
                    _ => {}
                }
            }
            for v in &costs {
                let resource = if v.upfront {
                    v.resource.to_string()
                } else {
                    v.resource.replace("Minute", "Second")
                };
                let has_cost = v.base_cost > 0.0
                    || v.total_cost > 0.0
                    || v.base_cost_no_mult > 0.0
                    || v.final_base_cost > 0.0;
                self.c
                    .set_out_val(actor, &format!("{resource}HasCost"), OutVal::Bool(has_cost));
                let name = format!("{resource}Cost");
                let raw_name = format!("{name}Raw");
                let eff = self.c.calc_mod(
                    list,
                    c,
                    &[&format!("{}CostEfficiency", v.typ), "CostEfficiency"],
                )?;
                let type_cost = format!("{}Cost", v.typ);
                let (cost, raw, more_type, inc);
                let mut more_cost = 1.0;
                if !v.unaffected {
                    more_type = self.c.more(list, c, &[&type_cost])?;
                    more_cost = self.c.more(list, c, &["Cost"])?;
                    inc = self.c.inc(list, c, &[&type_cost, "Cost"])?;
                    let mut r = v.base_cost_raw.map(|r| {
                        m_max(
                            0.0,
                            m_max(0.0, (1.0 + inc / 100.0) * r * more_type * more_cost / eff)
                                + v.total_cost,
                        )
                    });
                    let mut x = v.final_base_cost;
                    let scale = |x: f64, by: f64, up: bool| {
                        m_max(
                            0.0,
                            if up {
                                (by * x).ceil()
                            } else {
                                (by * x).floor()
                            },
                        )
                    };
                    x = scale(x, 1.0 + inc / 100.0, inc < 0.0);
                    x = scale(x, more_type, more_type < 1.0);
                    x = scale(x, more_cost, more_cost < 1.0);
                    x = m_max(0.0, x / eff);
                    if v.upfront && !v.percent {
                        x = round(x, 0.0);
                    }
                    x = m_max(0.0, x + v.total_cost);
                    if v.typ == "Mana" && hybrid_life > 0.0 {
                        x = m_max(0.0, ((1.0 - hybrid_life) * x).floor());
                        r = r.map(|r| m_max(0.0, (1.0 - hybrid_life) * r));
                    }
                    cost = x;
                    raw = r;
                } else {
                    more_type = self.c.more(list, c, &[&type_cost])?;
                    inc = self.c.inc(list, c, &[&type_cost])?;
                    let mut x = (v.base_cost + v.base_cost_no_mult).floor();
                    x = m_max(0.0, (1.0 + inc / 100.0) * x);
                    x = m_max(0.0, more_type * x);
                    x = m_max(0.0, x / eff);
                    if v.upfront && !v.percent {
                        x = round(x, 0.0);
                    }
                    x = m_max(0.0, x + v.total_cost);
                    cost = x;
                    raw = v.base_cost_raw.map(|r| {
                        m_max(
                            0.0,
                            m_max(
                                0.0,
                                (1.0 + inc / 100.0) * (r + v.base_cost_no_mult) * more_type / eff,
                            ) + v.total_cost,
                        )
                    });
                }
                self.c.set_out(actor, &name, cost);
                match raw {
                    Some(r) => self.c.set_out(actor, &raw_name, r),
                    None => self.c.clear_out(actor, &raw_name),
                }
                if has_cost && self.has_bd(actor) {
                    let pct = if v.percent { "%%" } else { "" };
                    let text = v.text;
                    let mut l = vec![sf!(&format!("%.2f{pct} ^8(base {text} cost)"), v.base_cost)];
                    if mult != 1.0 {
                        l.push(sf!("x %.4f ^8(cost multiplier)", mult));
                    }
                    if v.base_cost_no_mult != 0.0 {
                        l.push(sf!(
                            &format!("+ %d ^8(additional {text} cost)"),
                            v.base_cost_no_mult
                        ));
                    }
                    let life_instead = self.c.flag(list, c, &["CostLifeInsteadOfMana"])?;
                    if v.typ == "Life" && hybrid_life + additional_life != 0.0 && !life_instead {
                        l.push(sf!(
                            "x %.2f ^8(mana cost conversion)",
                            hybrid_life + additional_life
                        ));
                    }
                    if v.typ == "ES"
                        && additional_es != 0.0
                        && !life_instead
                        && !self.c.flag(list, c, &["CostESInsteadOfManaOrLife"])?
                    {
                        l.push(sf!("x %.2f ^8(mana cost conversion)", additional_es));
                    }
                    if inc != 0.0 {
                        l.push(sf!(
                            &format!("x %.2f ^8(increased/reduced {text} cost)"),
                            1.0 + inc / 100.0
                        ));
                    }
                    if more_cost != 1.0 {
                        l.push(sf!("x %.2f ^8(more/less cost)", more_cost));
                    }
                    if more_type != 1.0 {
                        l.push(sf!(&format!("x %.2f ^8(more/less {text} cost)"), more_type));
                    }
                    if eff != 1.0 {
                        l.push(sf!(&format!("/ %.2f ^8({text} cost efficiency)"), eff));
                    }
                    if v.total_cost != 0.0 {
                        l.push(sf!(&format!("%+d ^8(total {text} cost)"), v.total_cost));
                    }
                    if v.typ == "Mana" && hybrid_life > 0.0 {
                        l.push(sf!(
                            "x %.2f ^8(%d%% paid for with life)",
                            1.0 - hybrid_life,
                            hybrid_life * 100.0
                        ));
                    }
                    let d = if v.upfront { "d" } else { ".2f" };
                    l.push(sf!(&format!("= %{d}{pct}"), cost));
                    self.bd(actor).unwrap().set_lines(name, l);
                }
            }
        }
        let es = if self.c.flag(o.db, None, &["EnergyShieldProtectsMana"])? {
            self.c.out(actor, "EnergyShield").unwrap_or(0.0)
        } else {
            0.0
        };
        let pool = self.c.out(actor, "ManaUnreserved").unwrap_or(0.0) + es;
        self.c.set_out(actor, "ManaCostPayablePool", pool);
        // account for Sacrificial Zeal
        if self.c.flag(list, None, &["Condition:SacrificialZeal"])?
            && matches!(
                self.c.out_val(actor, "ManaHasCost"),
                Some(OutVal::Bool(true))
            )
        {
            let v = (self.c.out(actor, "ManaCost").unwrap_or(f64::NAN) * 0.25).floor();
            let spell = self.mf("Spell");
            for n in ["PhysicalMin", "PhysicalMax"] {
                self.c.add(
                    list,
                    new_mod(n, Base, v, Some("Sacrificial Zeal"), spell, 0, vec![]),
                );
            }
        }
        Ok(())
    }

    /// `buildConversionTable(cfg)`
    pub(crate) fn conversion_table(
        &self,
        list: StoreId,
        cfg: Option<&Cfg>,
    ) -> Result<ConversionTable> {
        let mut table: ConversionTable = Default::default();
        for (ti, t) in DMG_TYPES.iter().enumerate().take(4) {
            let mut global = [0.0; 5];
            let mut skill = [0.0; 5];
            let mut add = [0.0; 5];
            let (mut global_total, mut skill_total) = (0.0, 0.0);
            for oi in ti + 1..5 {
                let other = DMG_TYPES[oi];
                let mut names = vec![format!("{t}DamageConvertTo{other}")];
                if is_elemental(t) {
                    names.push(format!("ElementalDamageConvertTo{other}"));
                }
                if *t != "Chaos" {
                    names.push(format!("NonChaosDamageConvertTo{other}"));
                }
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                global[oi] = m_max(self.c.base(list, cfg, &names)?, 0.0);
                global_total += global[oi];
                skill[oi] = m_max(
                    self.c
                        .base(list, cfg, &[&format!("Skill{t}DamageConvertTo{other}")])?,
                    0.0,
                );
                skill_total += skill[oi];
                let mut names = vec![format!("{t}DamageGainAs{other}")];
                if is_elemental(t) {
                    names.push(format!("ElementalDamageGainAs{other}"));
                }
                if *t != "Chaos" {
                    names.push(format!("NonChaosDamageGainAs{other}"));
                }
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                add[oi] = m_max(self.c.base(list, cfg, &names)?, 0.0);
            }
            if skill_total > 100.0 {
                let factor = 100.0 / skill_total;
                for oi in ti + 1..5 {
                    skill[oi] *= factor;
                    global[oi] = 0.0;
                }
            } else if global_total + skill_total > 100.0 {
                let factor = (100.0 - skill_total) / global_total;
                for g in global.iter_mut().skip(ti + 1) {
                    *g *= factor;
                }
            }
            let e = &mut table[ti];
            let mut converted = 0.0;
            for oi in ti + 1..5 {
                let conversion = round(global[oi] + skill[oi], 0.0);
                e.conversion[oi] = conversion / 100.0;
                e.gain[oi] = add[oi] / 100.0;
                e.total[oi] = (conversion + add[oi]) / 100.0;
                converted += conversion;
            }
            e.mult = 1.0 - m_min(converted / 100.0, 1.0);
        }
        table[4].mult = 1.0;
        Ok(table)
    }

    /// From `runSkillFunc("preDamageFunc")` to the conversion tables
    fn offence_pre_damage(&mut self, o: &mut Off) -> Result<()> {
        self.run_skill_func(o, "preDamageFunc")?;
        let (i, list) = (o.skill, o.list);
        // Handle corpse and enemy explosions
        let monster_life = match self.sd_num(i, "corpseLife") {
            Some(l) => l,
            None => self
                .data()
                .table(
                    "monsterLifeTable",
                    &format!("{}", self.env.enemy_level as i64),
                )
                .unwrap_or(100.0),
        };
        if self.sd_truthy(i, "explodeCorpse") {
            let t = match self.sd(i, "corpseExplosionDamageType") {
                Some(ModValue::Str(s)) => s.to_string(),
                _ => "Fire".to_string(),
            };
            let mult = self
                .sd_num(i, "corpseExplosionLifeMultiplier")
                .or_else(|| self.sd_num(i, "selfFireExplosionLifeMultiplier"))
                .unwrap_or(f64::NAN);
            self.sd_set(i, &format!("{t}BonusMin"), monster_life * mult);
            self.sd_set(i, &format!("{t}BonusMax"), monster_life * mult);
        }
        o.monster_life = monster_life;
        if self.has_flag(i, "monsterExplode") {
            for t in DMG_TYPES {
                let pct = self.sd_num(i, &format!("{t}EffectiveExplodePercentage"));
                let base = pct.unwrap_or(0.0) * monster_life / 100.0;
                self.sd_set(i, &format!("{t}Min"), base);
                self.sd_set(i, &format!("{t}Max"), base);
            }
        }
        let cfg = self.cfg(i);
        for (k, t) in DMG_TYPES.iter().enumerate() {
            o.can_deal[k] =
                !self
                    .c
                    .flag(list, Some(&cfg), &[&format!("DealNo{t}"), "DealNoDamage"])?;
        }
        o.conv[0] = Some(self.conversion_table(list, Some(&cfg))?);
        for n in 0..2 {
            if let Some(wc) = self.mods(i).weapon_cfgs[n].clone() {
                o.conv[n + 1] = Some(self.conversion_table(list, Some(&wc))?);
            }
        }
        Ok(())
    }
}

/// The keys of `configTable` in `CalcTriggers.lua`: skill ids and lower
/// case names of skills, triggers and uniques
pub(crate) const TRIGGER_CONFIGS: [&str; 82] = [
    "law of the wilds",
    "the rippling thoughts",
    "the surging thoughts",
    "the hidden blade",
    "replica eternity shroud",
    "shroud of the lightless",
    "limbsplit",
    "the cauteriser",
    "duskblight",
    "lioneye's paws",
    "replica lioneye's paws",
    "moonbender's wing",
    "ngamahu's flame",
    "cameria's avarice",
    "starcaller",
    "uul-netol's embrace",
    "rigwald's crest",
    "jorrhast's blacksteel",
    "ashcaller",
    "arakaali's fang",
    "sporeguard",
    "mark of the elder",
    "mark of the shaper",
    "poet's pen",
    "maloney's mechanism",
    "asenath's chant",
    "vixen's entrapment",
    "flames of judgement",
    "storm of judgement",
    "trigger craft",
    "kitava's thirst",
    "foulborn kitava's thirst",
    "mjolner",
    "wing of the wyvern",
    "cospri's malice",
    "seven teachings",
    "squirming terror",
    "kinetic flux",
    "cast on critical strike",
    "cast on melee kill",
    "nova",
    "cast when damage taken",
    "cast when stunned",
    "cast on ward break",
    "spellslinger",
    "call to arms",
    "automation",
    "autoexertion",
    "seize the flesh",
    "fissure",
    "mark on hit",
    "hextouch",
    "oskarm",
    "tempest shield",
    "shattershard",
    "battlemage's cry",
    "arcanist brand",
    "cast on death",
    "combust",
    "prismatic burst",
    "voidstorm",
    "shockwave",
    "void shockwave",
    "falling crystal",
    "call the pyre",
    "manaforged arrows",
    "doom blast",
    "cast while channelling",
    "focus",
    "snipe",
    "avenging flame",
    "intuitive link",
    "svalinn cast on block",
    "festering resentment cast on block",
    "hex on trap",
    "supporttriggerfirespellonhit",
    "ghostly artillery",
    "replica gifts from above",
    "BloodShrineUniqueTriggeredExplodingToad",
    "bursting toad",
    "TriggeredMoltenStrike",
    "FieryImpactHeistMaceImplicit",
];

impl<'a> Perform<'a> {
    /// `calcs.triggers(env, env.player)`, `calcs.mirages(env)`, then
    /// `calcs.offence(env, env.player, env.player.mainSkill)`
    pub fn player_offence(&mut self) -> Result<()> {
        let main = self.main();
        self.triggers(crate::calc::PLAYER)?;
        // Mirages
        let generals_cry = self.sd_truthy(main, "triggeredByGeneralsCry")
            && !self.has_flag(main, "disable")
            && !self
                .cfg(main)
                .skill_cond
                .as_ref()
                .is_some_and(|c| c.contains(&Sym::new("usedByMirage")));
        if generals_cry && !self.is_limited(main) {
            self.generals_cry(main)?;
        }
        if !generals_cry && self.mirages()? {
            return Ok(());
        }
        self.offence(crate::calc::PLAYER, main)?;
        // The main skill's gem level and quality
        let e = &self.skill(main).active_effect;
        if e.src.is_some() {
            let (level, list, cfg) = (e.level, self.lists[main].list, self.cfg(main));
            let c = Some(&cfg);
            let actor = crate::calc::PLAYER;
            let sums = |p: &Self, names: [&str; 4]| -> Result<[f64; 4]> {
                let mut out = [0.0; 4];
                for (o, n) in out.iter_mut().zip(names) {
                    if !n.is_empty() {
                        *o = p.c.base(list, c, &[n])?;
                    }
                }
                Ok(out)
            };
            if level.is_some() {
                let [base, item, support, _] =
                    sums(self, ["GemLevel", "GemItemLevel", "GemSupportLevel", ""])?;
                let v = base + support + item;
                self.c.set_out_val(actor, "GemHasLevel", OutVal::Bool(true));
                self.c.set_out(actor, "GemLevel", v);
                if let Some(b) = self.bd(actor) {
                    let mut l = vec![sf!("%d ^8(level from gem)", base)];
                    if support > 0.0 {
                        l.push(sf!("+ %d ^8(level from support)", support));
                    }
                    if item > 0.0 {
                        l.push(sf!("+ %d ^8(level from items)", item));
                    }
                    l.push(sf!("= %d", v));
                    b.set_lines("GemLevel", l);
                }
            }
            let [base, item, support, socket] = sums(
                self,
                [
                    "GemQuality",
                    "GemItemQuality",
                    "GemSupportQuality",
                    "GemSocketQuality",
                ],
            )?;
            let v = base + support + item + socket;
            self.c
                .set_out_val(actor, "GemHasQuality", OutVal::Bool(true));
            self.c.set_out(actor, "GemQuality", v);
            if let Some(b) = self.bd(actor) {
                let mut l = vec![sf!("%d ^8(quality from gem)", base)];
                if support > 0.0 {
                    l.push(sf!("+ %d ^8(quality from support)", support));
                }
                if item > 0.0 {
                    l.push(sf!("+ %d ^8(quality from items)", item));
                }
                if socket > 0.0 {
                    l.push(sf!("+ %d ^8(quality from socket)", socket));
                }
                l.push(sf!("= %d", v));
                b.set_lines("GemQuality", l);
            }
        }
        Ok(())
    }
}

impl<'a> Perform<'a> {
    /// General's Cry in `calcs.mirages`: the main skill used by its mirage
    /// warriors, scaled by their count and the cry's cooldown
    fn generals_cry(&mut self, main: usize) -> Result<()> {
        for t in ["Triggered", "OtherThingUsesSkill"] {
            let t = self.st(t);
            match main.checked_sub(self.n_player) {
                None => self.env.skills.active_skills[main].skill_types.insert(t),
                Some(k) => self.minion_skills[k].skill_types.insert(t),
            };
        }
        self.set_skill_cond(main, "usedByMirage", true);
        let cached = self
            .cached_calc(main, true, true)?
            .ok_or("attempt to index a nil value (the cached calculation of the main skill)")?;
        let slot_of = |p: &Self, k: usize| {
            p.skill(k)
                .group
                .and_then(|g| p.env.groups.get(g))
                .and_then(|g| g.slot.clone())
        };
        let main_slot = slot_of(self, main);
        let cry = (0..self.n_player)
            .find(|&k| self.skill_name(k) == "General's Cry" && slot_of(self, k) == main_slot)
            .ok_or("attempt to index a nil value (local 'generalsCryActiveSkill')")?;
        let (cl, cc) = (self.lists[cry].list, self.cfg(cry));
        let (mut cooldown, _, _) = self.skill_cooldown(cl, &cc, cry)?;
        let list = self.lists[main].list;
        let cfg = self.cfg(main);
        let copy = |m: &Mod, name: &str| {
            let mut d = Mod::new(name, m.typ, m.value.clone(), m.source.as_deref());
            d.flags = m.flags;
            d.keyword_flags = m.keyword_flags;
            d
        };
        let mut warriors = 0.0;
        for (_, m) in self
            .c
            .tabulate(cl, Some(Base), Some(&cc), &["GeneralsCryDoubleMaxCount"])?
        {
            self.c.add(list, copy(&m, "QuantityMultiplier"));
            warriors += number(&m.value)?;
        }
        // The mirages' spawn time only shows in upstream's info message
        let _ = warriors;
        if !self.has_type(main, "Channel") {
            self.sd_set(main, "hitTimeOverride", 1.0);
        }
        let time = cached
            .out("HitTime")
            .or_else(|| cached.out("Time"))
            .unwrap_or(f64::NAN);
        cooldown = m_max(cooldown, time);
        self.c.add(
            list,
            Mod::new(
                "DPS",
                More,
                (1.0 / cooldown - 1.0) * 100.0,
                Some("General's Cry Cooldown"),
            ),
        );
        self.c.add(
            list,
            Mod::new("HasNoCost", ModType::Flag, true, Some("Used by mirage")),
        );
        for (typ, name, to) in [
            (Inc, "ExertIncrease", "Damage"),
            (More, "ExertIncrease", "Damage"),
            (More, "ExertAttackIncrease", "Damage"),
            (More, "OverexertionExertAverageIncrease", "Damage"),
            (Base, "ExertDoubleDamageChance", "DoubleDamageChance"),
        ] {
            for (_, m) in self.c.tabulate(list, Some(typ), Some(&cfg), &[name])? {
                self.c.add(list, copy(&m, to));
            }
        }
        Ok(())
    }
}

/// `source:find(".*:.*:(.*),.*")`
pub(crate) fn unique_trigger_name(src: &str) -> Option<String> {
    let comma = src.rfind(',')?;
    let head = &src[..comma];
    let colon = head.rfind(':')?;
    let before = &head[..colon];
    before.contains(':').then(|| head[colon + 1..].to_string())
}
