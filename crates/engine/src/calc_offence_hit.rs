//! The damage passes of `calcs.offence` (`Modules/CalcOffence.lua`): how
//! often the skill hits (speed, accuracy, block), then its hit damage
//! (exerts, crits, resistances, leech, gains on hit, DPS).
//!
//! In sync with upstream `16de4b82d57f1c0de6eb40f37143c32d4da36a02`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::OutVal;
use crate::calc::{PLAYER, floor, m_max, m_min, number, round};
use crate::calc_defence::{DMG_TYPES, armour_reduction_f, hit_chance, is_elemental};
use crate::calc_offence::{Off, OutMap, Pass, dmg_index, get_num, set_num};
use crate::calc_perform::{Perform, new_mod};
use crate::mods::{Cfg, Mod, ModType, ModValue, Sym};
use crate::breakdown::{Breakdown, Breakdowns, Cell, Chain, Row, color, f, multi_chain};
use crate::sf;

pub type Result<T> = std::result::Result<T, String>;

use ModType::{Base, Flag as FlagT, Inc, More};

/// How `combineStat` combines the hands' values
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Combine {
    Or,
    Average,
    HarmonicMean,
    Chance,
    ChanceAilment,
    Dps,
}

impl<'a> Perform<'a> {
    /// The current pass's output value
    pub(crate) fn po(&self, o: &Off, k: &str) -> Option<f64> {
        match o.passes[o.pass].weapon {
            Some(n) => get_num(&o.hands[n], k),
            None => self.c.out(o.actor, k),
        }
    }

    pub(crate) fn po_val(&self, o: &Off, k: &str) -> Option<OutVal> {
        match o.passes[o.pass].weapon {
            Some(n) => o.hands[n].get(&Sym::new(k)).cloned(),
            None => self.c.out_val(o.actor, k).cloned(),
        }
    }

    /// Sets the current pass's output value
    pub(crate) fn set_po(&mut self, o: &mut Off, k: &str, v: f64) {
        match o.passes[o.pass].weapon {
            Some(n) => set_num(&mut o.hands[n], k, v),
            None => self.c.set_out(o.actor, k, v),
        }
    }

    pub(crate) fn set_po_val(&mut self, o: &mut Off, k: &str, v: Option<OutVal>) {
        let sym = Sym::new(k);
        match (o.passes[o.pass].weapon, v) {
            (Some(n), Some(v)) => {
                o.hands[n].insert(sym, v);
            }
            (Some(n), None) => {
                o.hands[n].remove(&sym);
            }
            (None, Some(v)) => self.c.set_out_val(o.actor, k, v),
            (None, None) => self.c.clear_out(o.actor, k),
        }
    }

    /// The current pass's breakdown key (`MainHand.<k>` for the main
    /// hand's)
    pub(crate) fn pkey(o: &Off, k: &str) -> String {
        match o.passes[o.pass].weapon {
            Some(0) => format!("MainHand.{k}"),
            Some(_) => format!("OffHand.{k}"),
            None => k.to_string(),
        }
    }

    /// The global output (`globalOutput[k]`)
    pub(crate) fn go(&self, o: &Off, k: &str) -> Option<f64> {
        self.c.out(o.actor, k)
    }

    pub(crate) fn set_go(&mut self, o: &Off, k: &str, v: f64) {
        self.c.set_out(o.actor, k, v);
    }

    /// The cfg of the current pass; a weapon's sees the pass's output as
    /// its `skillStats`
    pub(crate) fn pcfg(&self, o: &Off) -> Cfg {
        match o.passes[o.pass].weapon {
            Some(n) => {
                let mut c = self.mods(o.skill).weapon_cfgs[n]
                    .clone()
                    .expect("a weapon pass's cfg");
                c.skill_stats = Some(
                    o.hands[n]
                        .iter()
                        .filter_map(|(k, v)| v.num().map(|v| (*k, v)))
                        .collect(),
                );
                c
            }
            None => self.cfg(o.skill),
        }
    }

    /// `source[k]` of the current pass
    pub(crate) fn src_val(&self, o: &Off, k: &str) -> Option<ModValue> {
        match o.passes[o.pass].weapon {
            Some(_) => o.passes[o.pass].source.get(k).cloned(),
            None => self.sd(o.skill, k).cloned(),
        }
    }

    pub(crate) fn src_num(&self, o: &Off, k: &str) -> Option<f64> {
        self.src_val(o, k).and_then(|v| v.as_number())
    }

    /// The conversion table of the current pass (`cfg.conversionTable or
    /// activeSkill.conversionTable`)
    pub(crate) fn pconv<'o>(&self, o: &'o Off) -> &'o crate::calc_offence::ConversionTable {
        let n = o.passes[o.pass].weapon.map_or(0, |n| n + 1);
        o.conv[n]
            .as_ref()
            .or(o.conv[0].as_ref())
            .expect("conversion tables")
    }

    /// `cfg.skillCond[var] = on` on the current pass's cfg
    pub(crate) fn set_pass_cond(&mut self, o: &Off, var: &str, on: bool) {
        match o.passes[o.pass].weapon {
            Some(n) => {
                let sym = Sym::new(var);
                if let Some(c) = self.mods_mut(o.skill).weapon_cfgs[n].as_mut() {
                    let conds = c.skill_cond.get_or_insert_with(Default::default);
                    if on {
                        conds.insert(sym);
                    } else {
                        conds.remove(&sym);
                    }
                }
            }
            None => self.set_skill_cond(o.skill, var, on),
        }
    }

    /// `combineStat(stat, mode, ...)`
    pub(crate) fn combine_stat(
        &mut self,
        o: &Off,
        stat: &str,
        mode: Combine,
        chance: Option<&str>,
    ) {
        let sym = Sym::new(stat);
        let (mh, oh) = (&o.hands[0], &o.hands[1]);
        let mut lines: Vec<String> = Vec::new();
        let num = |m: &OutMap, k: &str| get_num(m, k);
        let or = || {
            mh.get(&sym)
                .filter(|v| v.truthy())
                .or_else(|| oh.get(&sym))
                .cloned()
        };
        let value: Option<OutVal> = if mode == Combine::Or
            || !self.has_flag(o.skill, "bothWeaponAttack")
        {
            or()
        } else {
            match mode {
                Combine::Dps => {
                    let mut v = num(mh, stat).unwrap_or(0.0) + num(oh, stat).unwrap_or(0.0);
                    if mode == Combine::Dps
                        && !self.sd_truthy(o.skill, "doubleHitsWhenDualWielding")
                    {
                        v /= 2.0;
                    }
                    Some(OutVal::Num(v))
                }
                Combine::Average => Some(OutVal::Num(
                    (num(mh, stat).unwrap_or(0.0) + num(oh, stat).unwrap_or(0.0)) / 2.0,
                )),
                Combine::HarmonicMean => {
                    let (a, b) = (num(mh, stat), num(oh, stat));
                    if a == Some(0.0) || b == Some(0.0) {
                        Some(OutVal::Num(0.0))
                    } else {
                        let (a, b) = (a.unwrap_or(f64::NAN), b.unwrap_or(f64::NAN));
                        Some(OutVal::Num(2.0 / ((1.0 / a) + (1.0 / b))))
                    }
                }
                Combine::Chance | Combine::ChanceAilment => match (num(mh, stat), num(oh, stat)) {
                    (Some(a), Some(b)) => {
                        let c = chance.expect("a chance stat");
                        let main = num(mh, c).unwrap_or(f64::NAN)
                            * num(mh, "HitChance").unwrap_or(f64::NAN);
                        let off = num(oh, c).unwrap_or(f64::NAN)
                            * num(oh, "HitChance").unwrap_or(f64::NAN);
                        if mode == Combine::Chance {
                            let double = self.sd_truthy(o.skill, "doubleHitsWhenDualWielding");
                            let (mp, op, what) = if double {
                                (main / 10000.0, off / 10000.0, ["chance of main hand", "chance of  off hand"])
                            } else {
                                (
                                    main / (main + off),
                                    off / (main + off),
                                    [
                                        "portion of instances created by main hand",
                                        "portion of instances created by off hand",
                                    ],
                                )
                            };
                            let v = a * mp + b * op;
                            for (hand, x, p, w) in [("Main", a, mp, what[0]), ("Off", b, op, what[1])] {
                                lines.push(format!("Contribution from {hand} Hand:"));
                                lines.push(sf!("%.1f", x));
                                lines.push(sf!(&format!("x %.3f ^8({w})"), p));
                                lines.push(sf!("= %.1f", x * p));
                            }
                            lines.push("Total:".into());
                            lines.push(sf!("%.1f + %.1f", a * mp, b * op));
                            lines.push(sf!("= %.1f", v));
                            Some(OutVal::Num(v))
                        } else {
                            let max = m_max(a, b);
                            let min = m_min(a, b);
                            let stack = format!("{}Stacks", stat.replacen("DPS", "", 1));
                            let stacks = m_min(
                                1.0,
                                self.go(o, &stack).unwrap_or(1.0)
                                    / self.go(o, &format!("{stack}Max")).unwrap_or(1.0),
                            );
                            let v = max * stacks + min * (1.0 - stacks);
                            lines.push(String::new());
                            lines.push(sf!("%.2f%% of ailment stacks use maximum damage", stacks * 100.0));
                            lines.push(sf!(
                                "Max Damage comes from %s",
                                if a >= b { "Main Hand" } else { "Off Hand" }
                            ));
                            lines.push(sf!("= %.1f", max * stacks));
                            if stacks < 1.0 {
                                lines.push(sf!(
                                    "%.2f%% of ailment stacks use non-maximum damage",
                                    (1.0 - stacks) * 100.0
                                ));
                                lines.push(sf!("= %.1f", min * (1.0 - stacks)));
                            }
                            lines.push(String::new());
                            lines.push("Total:".into());
                            if stacks < 1.0 {
                                lines.push(sf!("%.1f + %.1f", max * stacks, min * (1.0 - stacks)));
                            }
                            lines.push(sf!("= %.1f", v));
                            Some(OutVal::Num(v))
                        }
                    }
                    _ => {
                        if mode == Combine::ChanceAilment {
                            let main = mh.get(&sym).is_some_and(OutVal::truthy);
                            lines.push(sf!(
                                "All ailment stacks comes from %s",
                                if main { "Main Hand" } else { "Off Hand" }
                            ));
                        }
                        or()
                    }
                },
                Combine::Or => unreachable!(),
            }
        };
        match value {
            Some(v) => self.c.set_out_val(o.actor, stat, v),
            None => self.c.clear_out(o.actor, stat),
        }
        if !lines.is_empty()
            && let Some(b) = self.bd(o.actor)
        {
            b.entry(stat).lines.extend(lines);
        }
    }

    /// Sets up the passes and calculates them
    pub(crate) fn offence_damage(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        if o.is_attack {
            if self
                .keystones_added
                .iter()
                .any(|k| k == "Precise Technique")
            {
                self.c
                    .set_out_val(actor, "PreciseTechnique", OutVal::Bool(true));
            } else {
                self.c.clear_out(actor, "PreciseTechnique");
            }
            let crit_override = self
                .c
                .override_num(list, Some(&cfg), &["WeaponBaseCritChance"])?;
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
                if let Some(b) = self.bd(actor) {
                    let p = if n == 0 { "MainHand." } else { "OffHand." };
                    b.map.retain(|k, _| !k.starts_with(p));
                }
                let mut source = self.weapon(actor, n).cloned().unwrap_or_default();
                let typed = matches!(source.get("type"), Some(ModValue::Str(t)) if &**t != "None");
                if let Some(c) = crit_override
                    && typed
                {
                    source.insert("CritChance".into(), ModValue::Number(c));
                }
                if n == 1 {
                    if let Some(c) = self.sd(i, "CritChance").cloned() {
                        source.insert("CritChance".into(), c);
                    }
                    for t in ["Physical", "Fire", "Cold"] {
                        let (min, max) = (
                            self.sd(i, &format!("setOffHand{t}Min")).cloned(),
                            self.sd(i, &format!("setOffHand{t}Max")).cloned(),
                        );
                        if let (Some(min), Some(max)) = (min, max) {
                            source.insert(format!("{t}Min"), min);
                            source.insert(format!("{t}Max"), max);
                        }
                    }
                    if let Some(t) = self.sd_num(i, "attackTime") {
                        source.insert("AttackRate".into(), ModValue::Number(1000.0 / t));
                    }
                }
                o.passes.push(Pass {
                    label: if n == 0 { "Main Hand" } else { "Off Hand" },
                    weapon: Some(n),
                    source,
                });
            }
        } else {
            o.passes.push(Pass {
                label: "Skill",
                weapon: None,
                source: Default::default(),
            });
        }
        self.speed_passes(o)?;
        self.hit_passes(o)?;
        self.offence_after_hits(o)
    }

    /// The first passes: accuracy, block, speed
    fn speed_passes(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb, db) = (o.skill, o.list, o.actor, o.edb, o.db);
        let skill_cfg = self.cfg(i);
        let sc = Some(&skill_cfg);
        let effective = self.env.mode.effective;
        let tick = self.misc_data("ServerTickRate");
        let off_hand_acc =
            self.c
                .flag(list, None, &["Condition:OffHandAccuracyIsMainHandAccuracy"])?;
        let mut stored_accuracy: Option<(f64, f64)> = None;
        let bd = self.has_bd(actor);
        let mut trauma_bd: Vec<String> = Vec::new();
        for p in 0..o.passes.len() {
            o.pass = p;
            let label = o.passes[p].label;
            if let Some(b) = self.sd_num(i, "averageBurstHits") {
                self.set_po(o, "AverageBurstHits", b);
            } else if let Some(r) = self.po(o, "Repeats")
                && r > 1.0
            {
                self.set_po(o, "AverageBurstHits", r);
            }
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            // Calculate hit chance
            let base = self.c.base(list, c, &["Accuracy"])?;
            let base_vs = self.c.base(list, c, &["Accuracy", "AccuracyVsEnemy"])?;
            let inc = self.c.inc(list, c, &["Accuracy"])?;
            let inc_vs = self.c.inc(list, c, &["Accuracy", "AccuracyVsEnemy"])?;
            // upstream passes "MORE" as the cfg
            let bug = Cfg::default();
            let more = self.c.more(list, Some(&bug), &["Accuracy"])?;
            let more_vs = self
                .c
                .more(list, Some(&bug), &["Accuracy", "AccuracyVsEnemy"])?;
            let mut accuracy = m_max(0.0, (base * (1.0 + inc / 100.0) * more).floor());
            let mut accuracy_vs = m_max(0.0, (base_vs * (1.0 + inc_vs / 100.0) * more_vs).floor());
            if let Some(b) = self.bd(actor) {
                let mut l = Vec::new();
                let chain = |l: &mut Vec<String>, label, base, inc: f64, more, total| {
                    multi_chain(
                        l,
                        Chain {
                            label,
                            base: Some(f("%g ^8(base)", base)),
                            factors: vec![
                                f("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
                                f("%.2f ^8(more/less)", more),
                            ],
                            total: Some(sf!("= %g", total)),
                            ..Chain::default()
                        },
                    );
                };
                chain(&mut l, None, base, inc, more, accuracy);
                if accuracy != accuracy_vs {
                    l.push(String::new());
                    chain(
                        &mut l,
                        Some("Effective Accuracy vs Enemy"),
                        base_vs,
                        inc_vs,
                        more_vs,
                        accuracy_vs,
                    );
                }
                b.set_lines(Self::pkey(o, "Accuracy"), l);
            }
            if off_hand_acc && label == "Main Hand" {
                stored_accuracy = Some((accuracy, accuracy_vs));
            } else if off_hand_acc
                && label == "Off Hand"
                && let Some((a, v)) = stored_accuracy
            {
                accuracy = a;
                accuracy_vs = v;
                if let Some(b) = self.bd(actor) {
                    let l = vec![format!(
                        "Using Main Hand Accuracy due to Mastery: {}",
                        crate::pat::num(a)
                    )];
                    b.set_lines(Self::pkey(o, "Accuracy"), l);
                }
            }
            self.set_po(o, "Accuracy", accuracy);
            let acc_hit = if !o.is_attack
                || self.c.flag(list, c, &["CannotBeEvaded"])?
                || self.sd_truthy(i, "cannotBeEvaded")
                || (effective && self.c.flag(edb, None, &["CannotEvade"])?)
            {
                100.0
            } else {
                let evasion = m_max(round(self.c.val(edb, "Evasion", None)?, 0.0), 0.0);
                let h = hit_chance(evasion, accuracy_vs) * self.c.calc_mod(list, c, &["HitChance"])?;
                if bd {
                    let num = crate::pat::num;
                    let note = if self.input.config.contains_key("enemyLevel") {
                        " ^8(overridden from the Configuration tab"
                    } else {
                        " ^8(can be overridden in the Configuration tab)"
                    };
                    let l = vec![
                        format!("Enemy level: {}{note}", num(self.env.enemy_level)),
                        format!("Enemy evasion: {}", num(evasion)),
                        format!("Approximate hit chance: {}%", num(h)),
                    ];
                    let k = Self::pkey(o, "AccuracyHitChance");
                    self.bd(actor).unwrap().set_lines(k, l);
                }
                h
            };
            self.set_po(o, "AccuracyHitChance", acc_hit);
            let mut block = m_max(
                m_min(self.c.base(edb, c, &["BlockChance"])?, 100.0)
                    - self.c.base(list, c, &["reduceEnemyBlock"])?,
                0.0,
            );
            if self.c.flag(edb, None, &["CannotBlockAttacks"])? && o.is_attack {
                block = 0.0;
            }
            self.set_po(o, "enemyBlockChance", block);
            self.set_po(o, "HitChance", acc_hit * (1.0 - block / 100.0));
            if block > 0.0
                && let Some(b) = self.bd(actor)
            {
                let num = crate::pat::num;
                let mut l = Vec::new();
                if acc_hit < 100.0 {
                    l.push(format!("Accuracy Hit Chance: {}%", num(acc_hit)));
                }
                l.push(format!("Enemy Block Chance: {}%", num(block)));
                l.push(format!(
                    "Approximate hit chance: {}%",
                    num(acc_hit * (1.0 - block / 100.0))
                ));
                b.set_lines(Self::pkey(o, "HitChance"), l);
            }
            if block > 0.0 && !o.is_attack {
                self.c
                    .set_out_val(actor, "enemyHasSpellBlock", OutVal::Bool(true));
            }
            let cond = format!("{}AccRatingHigherThanMaxLife", label.replace(' ', ""));
            let life = self.c.out(PLAYER, "Life").unwrap_or(f64::NAN);
            self.c.set_cond(list, &cond, accuracy > life);
            // The pass's output so far, as the queries' `skillStats`
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            // Calculate attack/cast speed
            let ge = self.skill(i).active_effect.granted_effect.clone();
            let triggered = self.sd_truthy(i, "triggered");
            if ge.cast_time == Some(0.0) && !self.sd_truthy(i, "castTimeOverride") && !triggered {
                self.set_po(o, "Time", 0.0);
                self.set_po(o, "Speed", 0.0);
            } else if let Some(t) = self.sd_num(i, "timeOverride") {
                self.set_po(o, "Time", t);
                self.set_po(o, "Speed", 1.0 / t);
            } else if self.sd_truthy(i, "fixedCastTime") {
                let t = ge.cast_time.unwrap_or(f64::NAN);
                self.set_po(o, "Time", t);
                self.set_po(o, "Speed", 1.0 / t);
            } else if let Some(tt) = self.sd_num(i, "triggerTime")
                && triggered
            {
                let linked = self.c.base(list, c, &["ActiveSkillsLinkedToTrigger"])?;
                let cdr = 1.0 + self.c.inc(list, c, &["CooldownRecovery"])? / 100.0;
                let t = if linked > 0.0 {
                    tt / cdr * linked
                } else {
                    tt / cdr
                };
                self.set_po(o, "Time", t);
                self.set_po(o, "TriggerTime", t);
                self.set_po(o, "Speed", 1.0 / t);
            } else if let Some(rate) = self.sd_num(i, "triggerRate")
                && triggered
            {
                self.set_po(o, "Time", 1.0 / rate);
                self.set_po(o, "TriggerTime", 1.0 / rate);
                self.set_po(o, "Speed", rate);
                self.sd_set(i, "showAverage", false);
            } else {
                let base_time = if o.is_attack {
                    if let Some(m) = self.sd_num(i, "attackSpeedMultiplier")
                        && let Some(r) = self.src_num(o, "AttackRate")
                    {
                        o.passes[p]
                            .source
                            .insert("AttackRate".into(), ModValue::Number(r * (1.0 + m / 100.0)));
                    }
                    let rate = self.src_num(o, "AttackRate").unwrap_or(1.0);
                    if self.sd_truthy(i, "castTimeOverridesAttackTime") {
                        ge.cast_time.unwrap_or(f64::NAN)
                            / (1.0 + self.src_num(o, "AttackSpeedInc").unwrap_or(0.0) / 100.0)
                    } else {
                        let attack_time = self.c.calc_mod(list, sc, &["SkillAttackTime"])?;
                        let t = 1.0 / rate + self.c.base(list, c, &["Speed"])?;
                        if attack_time > 0.0 {
                            t * attack_time
                        } else {
                            t
                        }
                    }
                } else {
                    self.sd_num(i, "castTimeOverride")
                        .or(ge.cast_time)
                        .unwrap_or(1.0)
                };
                let more = self.c.more(list, c, &["Speed"])?;
                let repeats = self.go(o, "Repeats").unwrap_or(1.0);
                self.set_po(o, "Repeats", repeats);
                let action = self.go(o, "ActionSpeedMod").unwrap_or(f64::NAN);
                if self.c.flag(list, None, &["HasTrauma"])? {
                    let cap = tick * repeats;
                    let duration = self.c.base(list, c, &["TraumaDuration"])?
                        * self.c.calc_mod(list, sc, &["Duration"])?;
                    let per_attack =
                        1.0 + m_min(self.c.base(list, c, &["ExtraTrauma"])?, 100.0) / 100.0;
                    let inc_per = self.c.inc(list, sc, &["SpeedPerTrauma"])?;
                    let config_trauma = self.c.base(list, sc, &["Multiplier:TraumaStacks"])?;
                    let inc = self.c.inc(list, c, &["Speed"])? - inc_per * config_trauma;
                    let before_inc = 1.0 / base_time * action * more;
                    let inc_cap = (cap - before_inc * (1.0 + inc / 100.0)) / before_inc * 100.0;
                    let hit = self.po(o, "HitChance").unwrap_or(f64::NAN);
                    let rate_before = per_attack * (hit / 100.0) * before_inc / repeats;
                    let mut trauma = rate_before * (1.0 + inc / 100.0)
                        / (1.0 / duration - rate_before * inc_per / 100.0);
                    let shown = trauma;
                    let invalid = trauma < 0.0 || inc_per * trauma > inc_cap;
                    if invalid {
                        trauma = per_attack * (hit / 100.0) * cap / repeats * duration;
                    }
                    if self.has_flag(i, "bothWeaponAttack") {
                        trauma /= 2.0;
                    }
                    if bd {
                        trauma_bd = trauma_lines(TraumaBd {
                            inc_per,
                            inc,
                            base_time,
                            more,
                            action,
                            before_inc,
                            per_attack,
                            hit,
                            duration,
                            repeats,
                            rate_before,
                            shown,
                            invalid,
                            cap,
                            trauma,
                        });
                    }
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:SustainableTraumaStacks",
                            Base,
                            trauma,
                            Some("Maximum Sustainable Trauma Stacks"),
                        ),
                    );
                }
                if self.c.base(list, sc, &["Multiplier:TraumaStacks"])? == 0.0 {
                    let v = self
                        .c
                        .base(list, sc, &["Multiplier:SustainableTraumaStacks"])?;
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:TraumaStacks",
                            Base,
                            v,
                            Some("Maximum Sustainable Trauma Stacks"),
                        ),
                    );
                }
                let inc = self.c.inc(list, c, &["Speed"])?;
                let mut speed = if self.has_flag(i, "warcry") {
                    1.0 / self.go(o, "WarcryCastTime").unwrap_or(f64::NAN)
                } else {
                    1.0 / (base_time / round((1.0 + inc / 100.0) * more, 2.0)
                        + self.c.base(list, c, &["TotalAttackTime"])?
                        + self.c.base(list, c, &["TotalCastTime"])?)
                };
                self.set_po(o, "CastRate", speed);
                if self.has_flag(i, "selfCast") {
                    speed *= action;
                    self.set_po(o, "CastRate", speed);
                }
                if self.has_flag(i, "totem") {
                    let totem_speed = 1.0 + self.c.inc(db, None, &["TotemActionSpeed"])? / 100.0;
                    self.set_po(o, "TotemActionSpeed", totem_speed);
                    speed *= totem_speed;
                    self.set_po(o, "CastRate", speed);
                    if self.sd_truthy(i, "totemFireOnce") {
                        let t = 1.0 / speed + self.go(o, "TotemPlacementTime").unwrap_or(f64::NAN);
                        self.set_po(o, "HitTime", t);
                        self.set_po(o, "HitSpeed", 1.0 / t);
                    }
                }
                if let Some(cd) = self.go(o, "Cooldown") {
                    self.set_po(o, "Cooldown", cd);
                    speed = m_min(speed, 1.0 / cd * repeats);
                }
                if self.po(o, "Cooldown").is_some() && self.has_flag(i, "selfCast") {
                    self.set_flag(i, "notAverage", true);
                    self.set_flag(i, "showAverage", false);
                    self.sd_set(i, "showAverage", false);
                }
                if !self.has_type(i, "Channel") {
                    speed = m_min(speed, tick * repeats);
                }
                self.set_po(o, "Speed", speed);
                let time = if speed == 0.0 { 0.0 } else { 1.0 / speed };
                self.set_po(o, "Time", time);
                if bd {
                    let act = if self.has_flag(i, "totem") {
                        self.po(o, "TotemActionSpeed")
                    } else {
                        None
                    }
                    .or_else(|| self.has_flag(i, "selfCast").then_some(action))
                    .unwrap_or(1.0);
                    let cast_rate = self.po(o, "CastRate").unwrap_or(f64::NAN);
                    let mut l = Vec::new();
                    multi_chain(
                        &mut l,
                        Chain {
                            base: Some(f("%.2f ^8(base)", 1.0 / base_time)),
                            factors: vec![
                                f("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
                                f("%.2f ^8(more/less)", more),
                                f("%.2f ^8(action speed modifier)", act),
                            ],
                            total: Some(sf!("= %.2f ^8casts per second", cast_rate)),
                            ..Chain::default()
                        },
                    );
                    if let Some(cd) = self.po(o, "Cooldown")
                        && 1.0 / cd < cast_rate
                    {
                        l.push("\n".into());
                        l.push(sf!("1 / %.2f ^8(skill cooldown)", cd));
                        if repeats > 1.0 {
                            l.push(sf!("x %d ^8(repeat count)", repeats));
                        }
                        l.push(sf!("= %.2f ^8(casts per second)", repeats / cd));
                        l.push("\n".into());
                        l.push(sf!("= %.2f ^8(lower of cast rates)", speed));
                    }
                    let b = self.bd(actor).unwrap();
                    b.set_lines(Self::pkey(o, "Speed"), l);
                    let attack_time = self.c.calc_mod(list, sc, &["SkillAttackTime"])?;
                    if attack_time > 0.0 {
                        let mut l = Vec::new();
                        multi_chain(
                            &mut l,
                            Chain {
                                base: Some(f("%.2f ^8(base)", 1.0 / (speed * attack_time))),
                                factors: vec![f("%.2f ^8(total modifier)", attack_time)],
                                total: Some(sf!("= %.2f ^8seconds per attack", time)),
                                ..Chain::default()
                            },
                        );
                        self.bd(actor).unwrap().set_lines(Self::pkey(o, "Time"), l);
                    }
                }
            }
            self.hit_time(o, false)?;
        }
        // Other Misc DPS multipliers (like custom source)
        let mut dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0)
            * (1.0 + self.c.inc(list, sc, &["DPS"])? / 100.0)
            * self.c.more(list, sc, &["DPS"])?;
        if self.has_type(i, "Brand") && !self.sd_truthy(i, "countsAttachedBrandsInDamage") {
            let n = self.c.out(actor, "AttachedBrandCount").unwrap_or(f64::NAN);
            dps_mult *= n;
            let m = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0) * n;
            self.c.set_out(actor, "SkillDPSMultiplier", m);
        }
        let repeat_final = matches!(self.input.config.get("repeatMode"), Some(crate::config::Val::Str(s)) if &**s == "FINAL");
        if repeat_final || self.c.flag(list, None, &["OnlyFinalRepeat"])? {
            dps_mult /= self.c.out(actor, "Repeats").unwrap_or(1.0);
        }
        self.sd_set(i, "dpsMultiplier", dps_mult);
        if self.c.flag(list, None, &["TriggeredBySnipe"])? {
            self.set_flag(i, "channelRelease", true);
        }
        if let Some(b) = self.bd(actor) {
            b.set_lines("SustainableTrauma", trauma_bd);
        }
        // `Flag(...) and Sum(...)`: nil without the flag
        if self.c.flag(list, None, &["HasTrauma"])? {
            let v = self
                .c
                .base(list, sc, &["Multiplier:SustainableTraumaStacks"])?;
            self.c.set_out(actor, "SustainableTrauma", v);
        } else {
            self.c.clear_out(actor, "SustainableTrauma");
        }
        // Mantra of Flames buff count
        let buffs = self.c.mult(db, "BuffOnSelf").unwrap_or(0.0)
            + self.c.base(list, None, &["Multiplier:TraumaStacks"])?;
        self.c.set_mult(db, "BuffOnSelf", buffs);
        let buffs = buffs
            + self
                .c
                .base(list, None, &["Multiplier:VoltaxicWaitingStages"])?;
        self.c.set_mult(db, "BuffOnSelf", buffs);
        if o.is_attack {
            self.combine_stat(o, "AccuracyHitChance", Combine::Average, None);
            self.combine_stat(o, "HitChance", Combine::Average, None);
            self.combine_stat(o, "Speed", Combine::HarmonicMean, None);
            self.combine_stat(o, "HitSpeed", Combine::Or, None);
            self.combine_stat(o, "HitTime", Combine::Or, None);
            let speed = self.c.out(actor, "Speed").unwrap_or(f64::NAN);
            let time = if speed == 0.0 { 0.0 } else { 1.0 / speed };
            self.c.set_out(actor, "Time", time);
            if time > 1.0 {
                self.c.add(
                    db,
                    Mod::new("Condition:OneSecondAttackTime", FlagT, true, None),
                );
            }
            let both = self.has_flag(i, "bothWeaponAttack");
            if self.c.flag(list, None, &["UseOffhandAttackSpeed"])?
                && !self.has_flag(i, "forceMainHand")
            {
                for k in ["Speed", "Time"] {
                    match get_num(&o.hands[1], k) {
                        Some(v) => self.c.set_out(actor, k, v),
                        None => self.c.clear_out(actor, k),
                    }
                }
                if let Some(b) = self.bd(actor) {
                    let speed = get_num(&o.hands[1], "Speed").unwrap_or(f64::NAN);
                    let l = vec!["Use Offhand Weapon Attack Speed:".into(), sf!("= %.2f", speed)];
                    b.set_lines("Speed", l);
                }
            } else if both && let Some(b) = self.bd(actor) {
                let l = vec![
                    "Both weapons:".into(),
                    sf!(
                        "2 / (1 / %.2f + 1 / %.2f)",
                        get_num(&o.hands[0], "Speed").unwrap_or(f64::NAN),
                        get_num(&o.hands[1], "Speed").unwrap_or(f64::NAN)
                    ),
                    sf!("= %.2f", speed),
                ];
                b.set_lines("Speed", l);
            }
            if bd {
                let w1 = self.has_flag(i, "weapon1Attack");
                let hit = self.c.out(actor, "HitChance").unwrap_or(f64::NAN);
                let b = self.bd(actor).unwrap();
                if both {
                    let l = vec![
                        "Both weapons:".into(),
                        sf!(
                            "%.2f%% ^8(main hand)",
                            get_num(&o.hands[0], "HitChance").unwrap_or(f64::NAN)
                        ),
                        sf!(
                            "%.2f%% ^8(off hand)",
                            get_num(&o.hands[1], "HitChance").unwrap_or(f64::NAN)
                        ),
                        sf!("= %.2f%% ^8(average)", hit),
                    ];
                    b.set_lines("HitChance", l);
                } else {
                    let h = if w1 { "MainHand" } else { "OffHand" };
                    let hand = |b: &Breakdowns, k: &str| b.get(&format!("{h}.{k}")).cloned();
                    match hand(b, "HitChance").or_else(|| hand(b, "AccuracyHitChance")) {
                        Some(x) => b.set("HitChance", x),
                        None => {
                            b.map.remove("HitChance");
                        }
                    }
                    if b.get("Speed").is_none()
                        && let Some(x) = hand(b, "Speed")
                    {
                        b.set("Speed", x);
                    }
                }
            }
            // the global output
            o.pass = usize::MAX;
            self.hit_time(o, true)?;
        }
        if bd {
            self.hit_speed_breakdown(o)?;
        }
        // Grab quantity multiplier
        let q = m_max(self.c.base(list, sc, &["QuantityMultiplier"])?, 1.0);
        if q > 1.0 {
            self.c.set_out(actor, "QuantityMultiplier", q);
        }
        o.quantity_multiplier = q;
        Ok(())
    }

    /// The breakdowns of `HitTime` and `HitSpeed`
    fn hit_speed_breakdown(&mut self, o: &Off) -> Result<()> {
        let (i, actor) = (o.skill, o.actor);
        let out = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        let on_death = self.sd_truthy(i, "triggeredOnDeath");
        let hit_speed = out(self, "HitSpeed");
        let hit_time = out(self, "HitTime");
        let cooldown = self.c.out(actor, "Cooldown");
        if self.sd_num(i, "hitTimeOverride").is_some() && !on_death {
            let l = vec![
                sf!("1 / %.2f ^8(hit time override)", hit_time),
                sf!("= %.2f", hit_speed),
            ];
            self.bd(actor).unwrap().set_lines("HitSpeed", l);
        } else if let Some(m) = self.sd_num(i, "hitTimeMultiplier")
            && let Some(time) = self.c.out(actor, "Time")
            && !on_death
        {
            let mut l = Vec::new();
            if m.floor() != m {
                l.push(sf!(
                    &format!("{}NOTE: First stage has a %.2fx channel time multiplier", color::CUSTOM),
                    m - m.floor()
                ));
            }
            l.push(if o.is_attack {
                sf!("%.2f ^8(attack time)", time)
            } else {
                sf!("%.2f ^8(cast time)", time)
            });
            l.push(sf!("x %.2f ^8(channel time multiplier)", m));
            l.push(sf!("= %.2f", hit_time));
            let first = match cooldown {
                Some(cd) if self.sd_truthy(i, "triggered") => sf!(
                    "1 / min(%.2f, %.2f) ^8min(hit time, cooldown)",
                    hit_time,
                    cd
                ),
                Some(cd) => sf!("1 / (%.2f + %.2f) ^8(hit time + cooldown)", hit_time, cd),
                None => sf!("1 / %.2f ^8(hit time)", hit_time),
            };
            let b = self.bd(actor).unwrap();
            b.set_lines("HitTime", l);
            b.set_lines("HitSpeed", vec![first, sf!("= %.2f", hit_speed)]);
        }
        Ok(())
    }

    /// `HitTime` and `HitSpeed` from `hitTimeOverride` or
    /// `hitTimeMultiplier`, of the pass or (`global`) of the actor
    fn hit_time(&mut self, o: &mut Off, global: bool) -> Result<()> {
        let i = o.skill;
        let get = |p: &Self, o: &Off, k: &str| {
            if global {
                p.c.out(o.actor, k)
            } else {
                p.po(o, k)
            }
        };
        let set = |p: &mut Self, o: &mut Off, k: &str, v: f64| {
            if global {
                p.c.set_out(o.actor, k, v)
            } else {
                p.set_po(o, k, v)
            }
        };
        let on_death = self.sd_truthy(i, "triggeredOnDeath");
        if let Some(t) = self.sd_num(i, "hitTimeOverride")
            && !on_death
        {
            set(self, o, "HitTime", t);
            set(self, o, "HitSpeed", 1.0 / t);
            if !global
                && self.has_type(i, "Brand")
                && !self.c.flag(o.list, None, &["UnlimitedBrandDuration"])?
            {
                let d = self.po(o, "Duration").unwrap_or(f64::NAN);
                set(self, o, "BrandTicks", (d * (1.0 / t)).floor());
            }
        } else if let Some(m) = self.sd_num(i, "hitTimeMultiplier")
            && let Some(time) = get(self, o, "Time")
            && !on_death
        {
            let t = time * m;
            set(self, o, "HitTime", t);
            let speed = match get(self, o, "Cooldown") {
                Some(cd) if self.sd_truthy(i, "triggered") => 1.0 / m_max(t, cd),
                Some(cd) => 1.0 / (t + cd),
                None if global => m_min(1.0 / t, self.misc_data("ServerTickRate")),
                None => 1.0 / t,
            };
            set(self, o, "HitSpeed", speed);
        }
        Ok(())
    }
}

/// `IntimidatingHitEffect` and `RallyingHitEffect`
fn hit_effect_lines(avg: f64, up: f64, hit: f64) -> Vec<String> {
    vec![
        sf!("1 + (%.2f ^8(average exerted damage)", avg),
        sf!("x %.2f) ^8(uptime %%)", up / 100.0),
        sf!("= %.2f", hit),
    ]
}

/// The values of the sustainable trauma's breakdown
struct TraumaBd {
    inc_per: f64,
    inc: f64,
    base_time: f64,
    more: f64,
    action: f64,
    before_inc: f64,
    per_attack: f64,
    hit: f64,
    duration: f64,
    repeats: f64,
    rate_before: f64,
    shown: f64,
    invalid: bool,
    cap: f64,
    trauma: f64,
}

fn trauma_lines(t: TraumaBd) -> Vec<String> {
    let mut l = Vec::new();
    let repeats = |l: &mut Vec<String>| {
        if t.repeats != 1.0 {
            l.push(sf!("/ %.2f ^8(repeats)", t.repeats));
        }
    };
    if t.inc_per == 0.0 {
        multi_chain(
            &mut l,
            Chain {
                label: Some("Trauma"),
                base: Some(f(
                    "%.2f ^8(attack speed)",
                    t.before_inc * (1.0 + t.inc / 100.0),
                )),
                factors: vec![
                    f("%.2f ^8(trauma per attack)", t.per_attack),
                    f("%.2f ^8(chance to hit)", t.hit / 100.0),
                    f("%.2f ^8(duration)", t.duration),
                ],
                no_total: true,
                ..Chain::default()
            },
        );
        repeats(&mut l);
    } else {
        multi_chain(
            &mut l,
            Chain {
                label: Some("Attack Speed before increased Attack Speed"),
                base: Some(f("%.2f ^8(base)", 1.0 / t.base_time)),
                factors: vec![
                    f("%.2f ^8(more/less)", t.more),
                    f("%.2f ^8(action speed modifier)", t.action),
                ],
                total: Some(sf!("= %.2f ^8attacks per second", t.before_inc)),
                ..Chain::default()
            },
        );
        l.push(String::new());
        multi_chain(
            &mut l,
            Chain {
                label: Some("Trauma per second before increased Attack Speed"),
                base: Some(f("%.2f ^8(base)", t.before_inc)),
                factors: vec![
                    f("%.2f ^8(trauma per attack)", t.per_attack),
                    f("%.2f ^8(chance to hit)", t.hit / 100.0),
                ],
                no_total: true,
                ..Chain::default()
            },
        );
        repeats(&mut l);
        l.push(sf!("= %.2f ^8trauma per second", t.rate_before));
        l.push(String::new());
        l.push("Trauma".into());
        l.push(sf!("%.2f ^8(base)", t.rate_before));
        l.push(sf!("x %.2f ^8(increased/reduced)", 1.0 + t.inc / 100.0));
        l.push(sf!(
            "/ %.4f ^8(1 / duration - trauma per second * increased attack speed per trauma / 100)",
            1.0 / t.duration - t.rate_before * t.inc_per / 100.0
        ));
    }
    let neg = if t.invalid { color::NEGATIVE } else { "" };
    l.push(sf!(&format!("= {neg}%d ^8trauma"), t.shown));
    if t.invalid {
        l.push(String::new());
        l.push("Attack Speed exceeds cap; Recalculating".into());
        multi_chain(
            &mut l,
            Chain {
                base: Some(f("%.2f ^8(base)", t.cap)),
                factors: vec![
                    f("%.2f ^8(trauma per attack)", t.per_attack),
                    f("%.2f ^8(chance to hit)", t.hit / 100.0),
                    f("%.2f ^8(duration)", t.duration),
                ],
                no_total: true,
                ..Chain::default()
            },
        );
        repeats(&mut l);
        l.push(sf!("= %d ^8trauma", t.trauma));
    }
    l
}

/// The warcries that exert attacks: name, output prefix, exert count stat,
/// uptime output, the skill type they need
type Cry = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

const EXERTING_CRIES: [Cry; 6] = [
    (
        "Ancestral Cry",
        "AncestralCry",
        "Ancestral",
        "AncestralUpTimeRatio",
        "NumAncestralExerts",
        Some("MeleeSingleTarget"),
    ),
    (
        "Infernal Cry",
        "InfernalCry",
        "Infernal",
        "InfernalUpTimeRatio",
        "NumInfernalExerts",
        None,
    ),
    (
        "Intimidating Cry",
        "IntimidatingCry",
        "Intimidating",
        "IntimidatingUpTimeRatio",
        "NumIntimidatingExerts",
        Some("Melee"),
    ),
    (
        "Rallying Cry",
        "RallyingCry",
        "Rallying",
        "RallyingUpTimeRatio",
        "NumRallyingExerts",
        Some("Melee"),
    ),
    (
        "Seismic Cry",
        "SeismicCry",
        "Seismic",
        "SeismicUpTimeRatio",
        "NumSeismicExerts",
        Some("Slam"),
    ),
    (
        "Battlemage's Cry",
        "BattleMageCry",
        "BattleCry",
        "BattlemageUpTimeRatio",
        "NumBattlemageExerts",
        None,
    ),
];

impl<'a> Perform<'a> {
    /// `calcWarcryCastTime` for active skill `j`
    pub(crate) fn warcry_cast_time(&self, j: usize, actor: usize) -> Result<f64> {
        let list = self.lists[j].list;
        let cfg = self.cfg(j);
        let c = Some(&cfg);
        let base = 1.0 / self.c.base(list, c, &["WarcryCastTime"])?;
        let t =
            base * self.c.calc_mod(list, c, &["WarcrySpeed"])? * self.action_speed_mod(actor)?;
        let t = 1.0 / m_min(t, self.misc_data("ServerTickRate"));
        if self.c.flag(list, c, &["InstantWarcry"])? || self.sd_truthy(j, "triggeredByAutoexertion")
        {
            return Ok(0.0);
        }
        Ok(t)
    }

    /// The exerting warcries' uptimes and their damage
    fn exerts(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb) = (o.skill, o.list, o.actor, o.edb);
        let pdb = self.pdb();
        let cfg = self.pcfg(o);
        let c = Some(&cfg);
        let skill_cfg = self.cfg(i);
        let effective = self.env.mode.effective;
        let exerted_dd = self.c.base(pdb, c, &["ExertDoubleDamageChance"])?;
        let exerting_count = self
            .c
            .base(pdb, None, &["Multiplier:ExertingWarcryCount"])?;
        if [
            "NeverExertable",
            "Triggered",
            "Channel",
            "OtherThingUsesSkill",
            "Retaliation",
            "SummonsTotem",
        ]
        .iter()
        .any(|t| self.has_type(i, t))
        {
            return Ok(());
        }
        let max_hit = self.c.flag(list, None, &["Condition:WarcryMaxHit"])?;
        for j in self.actor_skills(actor) {
            let name = self.skill_name(j).to_string();
            let Some(&(_, key, short, ratio, count_stat, needs)) =
                EXERTING_CRIES.iter().find(|c| c.0 == name)
            else {
                continue;
            };
            if self.go(o, &format!("{key}Calculated")).is_some()
                || self.c.out_val(actor, &format!("{key}Calculated")).is_some()
                || self.has_flag(j, "disable")
            {
                continue;
            }
            // Ancestral, Intimidating, Rallying and Seismic need the type to
            // be calculated at all; Infernal and Battlemage's only for the
            // uptime
            let type_ok = needs.is_none_or(|t| self.has_type(i, t));
            if !type_ok {
                continue;
            }
            let jl = self.lists[j].list;
            let jc = self.cfg(j);
            let duration = self.skill_duration(jl, Some(&jc), j)?;
            let cooldown = self.skill_cooldown(jl, &jc, j)?.0;
            let cast = self.warcry_cast_time(j, actor)?;
            let prefix = if key == "BattleMageCry" {
                "BattleMageCry"
            } else {
                key
            };
            self.set_go(o, &format!("{prefix}Duration"), duration);
            self.set_go(o, &format!("{prefix}Cooldown"), cooldown);
            self.set_go(o, &format!("{prefix}CastTime"), cast);
            let uptime_applies = match name.as_str() {
                "Infernal Cry" | "Battlemage's Cry" => self.has_type(i, "Melee"),
                _ => true,
            };
            if matches!(
                name.as_str(),
                "Intimidating Cry" | "Rallying Cry" | "Seismic Cry"
            ) {
                self.c.set_out_val(
                    actor,
                    "CreateWarcryOffensiveCalcSection",
                    OutVal::Bool(true),
                );
            }
            if uptime_applies {
                let count = self.c.base(pdb, None, &[count_stat])?;
                let count_key = if name == "Battlemage's Cry" {
                    "BattleCryExertsCount".to_string()
                } else {
                    format!("{short}ExertsCount")
                };
                self.set_go(o, &count_key, count);
                let speed = self.go(o, "Speed").unwrap_or(f64::NAN);
                let base_ratio = m_min((count / speed) / (cooldown + cast), 1.0) * 100.0;
                let stored = self.sd_num(j, "storedUses").unwrap_or(f64::NAN)
                    + self.c.base(jl, Some(&jc), &["AdditionalCooldownUses"])?;
                let up = m_min(100.0, base_ratio * stored);
                self.set_go(o, ratio, up);
                let g = self.go(o, "GlobalWarcryUptimeRatio").unwrap_or(0.0);
                self.set_go(o, "GlobalWarcryUptimeRatio", g + up);
                if let Some(b) = self.bd(actor) {
                    let mut l = vec![
                        sf!("(%d ^8(number of exerts)", count),
                        sf!("/ %.2f) ^8(attacks per second)", speed),
                    ];
                    if cast > 0.0 {
                        l.push(sf!("/ (%.2f ^8(warcry cooldown)", cooldown));
                        l.push(sf!("+ %.2f) ^8(warcry casttime)", cast));
                    } else {
                        l.push(sf!("/ %.2f ^8(average warcry cooldown)", cooldown));
                    }
                    l.push(sf!("* %d ^8(stored uses)", stored));
                    l.push(sf!("= %d%%", up));
                    b.set_lines(ratio, l);
                }
            }
            let up = self.go(o, ratio).unwrap_or(f64::NAN);
            let mul = |p: &mut Self, o: &Off, k: &str, by: f64| {
                let v = p.go(o, k).unwrap_or(f64::NAN) * by;
                p.set_go(o, k, v);
            };
            match name.as_str() {
                "Intimidating Cry" => {
                    let mut dd = self.c.base(list, c, &["DoubleDamageChance"])?;
                    if effective {
                        dd += self.c.base(edb, c, &["SelfDoubleDamageChance"])?;
                    }
                    let dd = m_min(dd + exerted_dd, 100.0);
                    let avg = 2.0 * (1.0 - dd / 100.0);
                    self.set_go(o, "IntimidatingAvgDmg", avg);
                    let hit = 1.0 + avg * up / 100.0;
                    if let Some(b) = self.bd(actor) {
                        b.set_lines(
                            "IntimidatingAvgDmg",
                            vec![
                                "Average Intimidating Cry Damage:".into(),
                                sf!(
                                    "%.2f%% ^8(base double damage increase to hit 100%%)",
                                    (1.0 - dd / 100.0) * 100.0
                                ),
                                sf!("x %d ^8(double damage multiplier)", 2.0),
                                sf!("= %.2f", avg),
                            ],
                        );
                        b.set_lines("IntimidatingHitEffect", hit_effect_lines(avg, up, hit));
                    }
                    self.set_go(o, "IntimidatingHitEffect", hit);
                    self.set_go(o, "IntimidatingMaxHitEffect", 1.0 + avg);
                    mul(self, o, "TheoreticalOffensiveWarcryEffect", hit);
                    mul(self, o, "TheoreticalMaxOffensiveWarcryEffect", 1.0 + avg);
                }
                "Rallying Cry" => {
                    let allies = m_min(self.c.base(pdb, c, &["Multiplier:NearbyAlly"])?, 5.0);
                    let per = self
                        .c
                        .base(pdb, None, &["RallyingExertMoreDamagePerAlly"])?
                        / 100.0;
                    let avg = allies * per;
                    self.set_go(o, "RallyingAvgDmg", avg);
                    let hit = 1.0 + avg * up / 100.0;
                    if let Some(b) = self.bd(actor) {
                        b.set_lines(
                            "RallyingAvgDmg",
                            vec![
                                "Average Rallying Cry Damage:".into(),
                                sf!("%.2f ^8(average damage multiplier per ally)", per),
                                sf!("x %d ^8(number of nearby allies (max=5))", allies),
                                sf!("= %.2f", avg),
                            ],
                        );
                        b.set_lines("RallyingHitEffect", hit_effect_lines(avg, up, hit));
                    }
                    self.set_go(o, "RallyingHitEffect", hit);
                    self.set_go(o, "RallyingMaxHitEffect", 1.0 + avg);
                    mul(self, o, "OffensiveWarcryEffect", hit);
                    mul(self, o, "MaxOffensiveWarcryEffect", 1.0 + avg);
                    mul(self, o, "TheoreticalOffensiveWarcryEffect", hit);
                    mul(self, o, "TheoreticalMaxOffensiveWarcryEffect", 1.0 + avg);
                }
                "Seismic Cry" => {
                    let aoe = self.c.base(pdb, None, &["SeismicMoreAoE"])?;
                    let m = if max_hit {
                        Mod::new("AreaOfEffect", More, aoe, Some("Max Seismic Exert AoE"))
                    } else {
                        Mod::new(
                            "AreaOfEffect",
                            More,
                            (aoe / 100.0 * up).floor(),
                            Some("Avg Seismic Exert AoE"),
                        )
                    };
                    self.c.add(list, m);
                    self.area_of_effect(o)?;
                }
                _ => {}
            }
            self.c
                .set_out_val(actor, &format!("{key}Calculated"), OutVal::Bool(true));
        }
        if max_hit {
            let v = self.go(o, "MaxOffensiveWarcryEffect").unwrap_or(f64::NAN);
            self.set_go(o, "AilmentWarcryEffect", v);
            self.sd_set(i, "showAverage", true);
            self.set_flag(i, "showAverage", true);
            self.set_flag(i, "notAverage", false);
        } else {
            let v = self.go(o, "OffensiveWarcryEffect").unwrap_or(f64::NAN);
            self.set_go(o, "AilmentWarcryEffect", v);
        }
        let mut ratio = self.go(o, "ExertedAttackUptimeRatio").unwrap_or(0.0);
        for (_, _, _, r, _, _) in EXERTING_CRIES {
            ratio = m_max(ratio, self.go(o, r).unwrap_or(0.0));
        }
        self.set_go(o, "ExertedAttackUptimeRatio", ratio);
        if self.has_bd(actor) {
            let mut l = vec!["Maximum of:".to_string()];
            for (_, _, _, r, _, _) in EXERTING_CRIES {
                if let Some(v) = self.go(o, r) {
                    let name = &r[..r.find("Up").unwrap_or(r.len())];
                    l.push(sf!("%d%% ^8(%s Cry Uptime)", v, name));
                }
            }
            l.push(sf!("= %d%%", ratio));
            self.bd(actor)
                .unwrap()
                .set_lines("ExertedAttackUptimeRatio", l);
        }
        if ratio > 0.0
            && self
                .c
                .out_val(actor, "ExertedAttackUptimeRatioCalculated")
                .is_none()
        {
            let inc = self.c.inc(list, c, &["ExertIncrease"])?;
            let more = self.c.sum(list, More, c, &["ExertIncrease"])?;
            let more_attack = self.c.sum(list, More, c, &["ExertAttackIncrease"])?;
            let over = self
                .c
                .sum(list, More, c, &["OverexertionExertAverageIncrease"])?;
            let echoes = self.c.sum(list, More, c, &["EchoesExertAverageIncrease"])?;
            let attack = self.mf("Attack");
            let global = self.go(o, "GlobalWarcryUptimeRatio").unwrap_or(f64::NAN);
            let mods: Vec<Mod> = if max_hit {
                vec![
                    Mod::new("Damage", Inc, inc, Some("Exerted Attacks")),
                    Mod::new("Damage", More, more, Some("Exerted Attacks")),
                    new_mod(
                        "Damage",
                        More,
                        more_attack,
                        Some("Exerted Attack Damage"),
                        attack,
                        0,
                        vec![],
                    ),
                    Mod::new(
                        "Damage",
                        More,
                        over * exerting_count,
                        Some("Max Autoexertion Support"),
                    ),
                    Mod::new(
                        "Damage",
                        More,
                        echoes * exerting_count,
                        Some("Max Echoes of Creation"),
                    ),
                ]
            } else {
                vec![
                    Mod::new(
                        "Damage",
                        Inc,
                        inc * ratio / 100.0,
                        Some("Uptime Scaled Exerted Attacks"),
                    ),
                    Mod::new(
                        "Damage",
                        More,
                        more * ratio / 100.0,
                        Some("Uptime Scaled Exerted Attacks"),
                    ),
                    new_mod(
                        "Damage",
                        More,
                        more_attack * ratio / 100.0,
                        Some("Uptime Scaled Exerted Attack Damage"),
                        attack,
                        0,
                        vec![],
                    ),
                    Mod::new(
                        "Damage",
                        More,
                        over * global / 100.0,
                        Some("Uptime Scaled Autoexertion Support"),
                    ),
                    Mod::new(
                        "Damage",
                        More,
                        echoes * global / 100.0,
                        Some("Uptime Scaled Echoes of Creation"),
                    ),
                ]
            };
            for m in mods {
                self.c.add(list, m);
            }
            let sc = Some(&skill_cfg);
            let avg = self.c.calc_mod(list, sc, &["ExertIncrease"])?
                * self.c.calc_mod(
                    list,
                    sc,
                    &[
                        "ExertAttackIncrease",
                        "OverexertionExertAverageIncrease",
                        "EchoesExertAverageIncrease",
                    ],
                )?;
            self.set_go(o, "ExertedAttackAvgDmg", avg);
            self.set_go(o, "ExertedAttackHitEffect", avg * ratio / 100.0);
            if let Some(b) = self.bd(actor) {
                b.set_lines(
                    "ExertedAttackHitEffect",
                    vec![
                        sf!("(%.2f ^8(average exerted damage)", avg),
                        sf!("x %.2f) ^8(uptime %%)", ratio / 100.0),
                        sf!("= %.2f", avg * ratio / 100.0),
                    ],
                );
            }
            self.set_go(o, "ExertedAttackMaxHitEffect", avg);
            self.c.set_out_val(
                actor,
                "ExertedAttackUptimeRatioCalculated",
                OutVal::Bool(true),
            );
        }
        Ok(())
    }

    /// The pacts' uptimes and their damage
    fn pacts(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let skill_cfg = self.cfg(i);
        let sc = Some(&skill_cfg);
        if !self.has_type(i, "Spell") || self.has_type(i, "Brand") {
            return Ok(());
        }
        for j in self.actor_skills(actor) {
            let pact_name = self.skill_name(j).to_string();
            let Some(key) = pact_name.strip_prefix("Pact of ") else {
                continue;
            };
            let key = if key == "K'Tash" { "Ktash" } else { key };
            let vaal = self.has_type(i, "Vaal");
            let (proj, chaining) = (self.has_flag(i, "projectile"), self.has_flag(i, "chaining"));
            let applies = match key {
                "Beidat" => {
                    !vaal
                        && !self.has_type(i, "Channel")
                        && (proj || self.has_type(i, "Cascadable") || chaining && !proj)
                }
                "Ghorr" => !vaal && self.has_type(i, "DamageOverTime"),
                "Ktash" => {
                    vaal && (self.has_type(i, "Damage") || self.has_type(i, "DamageOverTime"))
                }
                "Lycia" => !vaal && self.has_type(i, "Channel") && self.has_flag(i, "selfCast"),
                _ => false,
            };
            let calculated = format!("PactOf{key}Calculated");
            if !applies
                || self.c.out_val(actor, &calculated).is_some()
                || self.has_flag(j, "disable")
            {
                continue;
            }
            let jl = self.lists[j].list;
            let jc = self.cfg(j);
            let jcc = Some(&jc);
            let cooldown = self.skill_cooldown(jl, &jc, j)?.0;
            let cast_time = self
                .skill(j)
                .active_effect
                .granted_effect
                .cast_time
                .unwrap_or(f64::NAN);
            let rate = 1.0 / cast_time
                * self.c.calc_mod(jl, jcc, &["Speed"])?
                * self.action_speed_mod(actor)?;
            let cast = 1.0 / m_min(rate, self.misc_data("ServerTickRate"));
            let count = self.c.base(jl, jcc, &[&format!("{key}EmpoweredSpells")])?;
            let stored = self.sd_num(j, "storedUses").unwrap_or(f64::NAN)
                + self.c.base(jl, jcc, &["AdditionalCooldownUses"])?;
            let speed = self.go(o, "Speed").unwrap_or(0.0);
            let mut uptime = if speed == 0.0 {
                100.0
            } else {
                m_min((count / speed) / (cooldown + cast), 1.0) * 100.0
            };
            uptime = m_min(100.0, uptime * stored);
            let effect = if self.c.flag(list, None, &["Condition:PactMaxHit"])? {
                100.0
            } else {
                uptime
            };
            let em = effect / 100.0;
            self.c
                .set_out_val(actor, "CreatePactOffensiveCalcSection", OutVal::Bool(true));
            self.set_go(o, &format!("PactOf{key}Cooldown"), cooldown);
            self.set_go(o, &format!("PactOf{key}CastTime"), cast);
            self.set_go(o, &format!("{key}EmpoweredCount"), count);
            self.set_go(o, &format!("{key}UpTimeRatio"), uptime);
            if let Some(b) = self.bd(actor) {
                b.set_lines(
                    format!("{key}UpTimeRatio"),
                    vec![
                        sf!("(%.2f ^8(number of Empowered)", count),
                        sf!("/ %.2f) ^8(casts per second)", speed),
                        sf!("/ (%.2f ^8(pact cooldown)", cooldown),
                        sf!("+ %.2f) ^8(pact cast time)", cast),
                        sf!("* %.2f ^8(stored uses)", stored),
                        sf!("= %.2f%%", uptime),
                    ],
                );
            }
            if let Some(ModValue::Record(r)) = self
                .c
                .list(jl, jcc, &[&format!("{key}PactDamage")])?
                .first()
                && let Some(ModValue::Mod(m)) = r.get("mod")
            {
                let v = number(&m.value)? * em;
                let src = format!("Uptime Scaled {pact_name}");
                self.c.add(
                    list,
                    new_mod(
                        m.name.as_str(),
                        m.typ,
                        v,
                        Some(&src),
                        m.flags,
                        m.keyword_flags,
                        m.tags.clone(),
                    ),
                );
            }
            if key == "Beidat" {
                if proj
                    && !self.c.flag(list, sc, &["NoAdditionalProjectiles"])?
                    && !self.c.flag(list, sc, &["SingleProjectile"])?
                {
                    let v = self.c.base(jl, jcc, &["BeidatAdditionalProjectiles"])? * em;
                    self.set_go(o, "BeidatAdditionalProjectiles", v);
                    let p = self.go(o, "ProjectileCount").unwrap_or(f64::NAN) + v;
                    self.set_go(o, "ProjectileCount", p);
                }
                if chaining
                    && !proj
                    && !self.c.flag(list, sc, &["CannotChain"])?
                    && !self.c.flag(list, sc, &["NoAdditionalChains"])?
                {
                    let v = self.c.base(jl, jcc, &["BeidatAdditionalBeamChains"])? * em;
                    self.set_go(o, "BeidatAdditionalBeamChains", v);
                    let max = self.go(o, "ChainMax").unwrap_or(f64::NAN) + v;
                    self.set_go(o, "ChainMax", max);
                    self.set_go(o, "ChainMaxString", max);
                    let chain = self.go(o, "Chain").unwrap_or(f64::NAN);
                    self.set_go(o, "ChainRemaining", m_max(0.0, max - chain));
                }
                if self.has_type(i, "Cascadable") {
                    let v = self.c.base(jl, jcc, &["BeidatAdditionalCascades"])? * em;
                    self.set_go(o, "BeidatAdditionalCascades", v);
                }
            } else if key == "Ktash" {
                let v = self.c.base(jl, jcc, &["KtashPactSoulRefundChance"])? * em;
                self.set_go(o, "KtashSoulRefundChance", v);
                if let Some(d) = self.go(o, "SoulGainPreventionDuration") {
                    let m =
                        1.0 + self.c.base(jl, jcc, &["KtashPactSoulGainPrevention"])? * em / 100.0;
                    let tick = self.misc_data("ServerTickRate");
                    let nd = m_max((d * m * tick).ceil(), 1.0) / tick;
                    self.set_go(o, "SoulGainPreventionDuration", nd);
                    if let Some(b) = self.bd(actor) {
                        b.set_lines(
                            "SoulGainPreventionDuration",
                            vec![
                                sf!("%.3fs ^8(before Pact of K'Tash)", d),
                                sf!("x %.4f ^8(average Pact modifier)", m),
                                "rounded up to nearest server tick".into(),
                                sf!("= %.3fs", nd),
                            ],
                        );
                    }
                }
            }
            self.c.set_out_val(actor, &calculated, OutVal::Bool(true));
        }
        Ok(())
    }

    /// Ruthless and Fist of War
    fn ruthless(&mut self, o: &mut Off) -> Result<()> {
        let (i, list) = (o.skill, o.list);
        let cfg = self.pcfg(o);
        let c = Some(&cfg);
        let mode = match self.input.config.get("ruthlessSupportMode") {
            Some(crate::config::Val::Str(s)) => s.to_string(),
            _ => "AVERAGE".to_string(),
        };
        let max_count = self.c.base(list, c, &["RuthlessBlowMaxCount"])?;
        self.set_po(o, "RuthlessBlowMaxCount", max_count);
        let mirage = self
            .cfg(i)
            .skill_cond
            .as_ref()
            .is_some_and(|s| s.contains(&Sym::new("usedByMirage")));
        let chance = if max_count > 0.0
            && (!mirage || self.sd_num(i, "mirageUses").unwrap_or(0.0) > max_count)
        {
            match mode.as_str() {
                "AVERAGE" => Some(round(100.0 / max_count, 0.0)),
                "MAX" => {
                    let d = self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
                    self.sd_set(i, "dpsMultiplier", d / max_count);
                    Some(100.0)
                }
                _ => None,
            }
        } else {
            Some(0.0)
        };
        if let Some(ch) = chance {
            self.set_po(o, "RuthlessBlowChance", ch);
        }
        let ch = self.po(o, "RuthlessBlowChance").unwrap_or(f64::NAN);
        let hit_mult = 1.0 + self.c.base(list, c, &["RuthlessBlowHitMultiplier"])? / 100.0;
        let ail_mult = 1.0 + self.c.base(list, c, &["RuthlessBlowAilmentMultiplier"])? / 100.0;
        self.set_po(o, "RuthlessBlowHitMultiplier", hit_mult);
        self.set_po(o, "RuthlessBlowAilmentMultiplier", ail_mult);
        self.set_po(
            o,
            "RuthlessBlowHitEffect",
            1.0 - ch / 100.0 + ch / 100.0 * hit_mult,
        );
        self.set_po(
            o,
            "RuthlessBlowAilmentEffect",
            1.0 - ch / 100.0 + ch / 100.0 * ail_mult,
        );
        let cd = self.c.base(list, c, &["FistOfWarCooldown"])?;
        self.set_go(o, "FistOfWarCooldown", cd);
        if cd != 0.0
            && self.has_type(i, "Slam")
            && !self.has_type(i, "Vaal")
            && !self.has_type(i, "OtherThingUsesSkill")
        {
            let mult = self.c.base(list, None, &["FistOfWarDamageMultiplier"])? / 100.0;
            self.set_go(o, "FistOfWarDamageMultiplier", mult);
            let speed = self.go(o, "Speed").unwrap_or(f64::NAN);
            let up = m_min((1.0 / speed) / cd, 1.0) * 100.0;
            self.set_go(o, "FistOfWarUptimeRatio", up);
            self.set_go(o, "AvgFistOfWarDamage", mult);
            let avg = 1.0 + mult * (up / 100.0);
            if let Some(b) = self.bd(o.actor) {
                b.set_lines(
                    "FistOfWarUptimeRatio",
                    vec![
                        sf!("min( (1 / %.2f) ^8(second per attack)", speed),
                        sf!("/ %.2f, 1) ^8(fist of war cooldown)", cd),
                        sf!("= %d%%", up),
                    ],
                );
                b.set_lines(
                    "AvgFistOfWarDamageEffect",
                    vec![
                        sf!("1 + (%.2f ^8(fist of war damage multiplier)", mult),
                        sf!("x %.2f) ^8(fist of war uptime ratio)", up / 100.0),
                        sf!("= %.2f", avg),
                    ],
                );
            }
            self.set_go(o, "AvgFistOfWarDamageEffect", avg);
            self.set_go(o, "MaxFistOfWarDamageEffect", 1.0 + mult);
            let aoe = self.c.base(list, None, &["FistOfWarIncAoE"])?;
            if self.c.flag(list, None, &["Condition:WarcryMaxHit"])? {
                self.set_po(o, "FistOfWarDamageEffect", 1.0 + mult);
                self.c.add(
                    list,
                    Mod::new(
                        "AreaOfEffect",
                        Inc,
                        aoe,
                        Some("Max Fist of War Boosted AoE"),
                    ),
                );
            } else {
                self.set_po(o, "FistOfWarDamageEffect", avg);
                self.c.add(
                    list,
                    Mod::new(
                        "AreaOfEffect",
                        Inc,
                        (aoe / 100.0 * up).floor(),
                        Some("Avg Fist Of War Boosted AoE"),
                    ),
                );
            }
            self.area_of_effect(o)?;
            for (k, by) in [
                ("TheoreticalOffensiveWarcryEffect", avg),
                ("TheoreticalMaxOffensiveWarcryEffect", 1.0 + mult),
            ] {
                let v = self.go(o, k).unwrap_or(f64::NAN) * by;
                self.set_go(o, k, v);
            }
        } else {
            self.set_po(o, "FistOfWarDamageEffect", 1.0);
        }
        Ok(())
    }

    /// Crit chance, multiplier and effect
    fn crits(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb, db) = (o.skill, o.list, o.actor, o.edb, o.db);
        let cfg = self.pcfg(o);
        let c = Some(&cfg);
        let skill_cfg = self.cfg(i);
        let sc = Some(&skill_cfg);
        let effective = self.env.mode.effective;
        let zero = |p: &mut Self, o: &mut Off| {
            for k in [
                "PreEffectiveCritChance",
                "CritChance",
                "CritMultiplier",
                "BonusCritDotMultiplier",
            ] {
                p.set_po(o, k, 0.0);
            }
            p.set_po(o, "CritEffect", 1.0);
        };
        if self.c.flag(list, c, &["NeverCrit"])? {
            zero(self, o);
        } else if self.c.flag(
            list,
            c,
            &["SpellSkillsCannotDealCriticalStrikesExceptOnFinalRepeat"],
        )? {
            let repeats = self.po(o, "Repeats").unwrap_or(1.0);
            if repeats == 1.0 {
                zero(self, o);
            } else if self.c.flag(
                list,
                c,
                &["SpellSkillsAlwaysDealCriticalStrikesOnFinalRepeat"],
            )? {
                let mode = match self.input.config.get("repeatMode") {
                    Some(crate::config::Val::Str(s)) => Some(s.to_string()),
                    _ => None,
                };
                let v = match mode.as_deref() {
                    Some("None") => 0.0,
                    Some("AVERAGE") => 100.0 / repeats,
                    _ => 100.0,
                };
                self.set_po(o, "PreEffectiveCritChance", v);
                self.set_po(o, "CritChance", v);
                if mode.as_deref() == Some("AVERAGE")
                    && let Some(b) = self.bd(actor)
                {
                    b.set_lines(
                        Self::pkey(o, "CritChance"),
                        vec![
                            "100%".into(),
                            sf!("/ %d ^8(number of repeats)", repeats),
                            sf!("= %.2f%% average critical strike chance", v),
                        ],
                    );
                }
            }
        } else {
            let mut crit_override = self.c.override_num(list, c, &["CritChance"])?;
            if self.c.flag(list, c, &["MainHandCritIsEqualToParent"])? {
                let parent = self.c.actors.actors[actor]
                    .parent
                    .ok_or("an actor without a parent")?;
                let from_output = match self.c.out_val(parent, "MainHand") {
                    Some(OutVal::Table(t)) => t.get("CritChance").and_then(OutVal::num),
                    _ => None,
                };
                crit_override = from_output.or_else(|| self.weapon_num(parent, 0, "CritChance"));
            } else if self
                .c
                .flag(list, c, &["MainHandCritIsEqualToPartyMember"])?
            {
                return Err("not ported: party".into());
            }
            let mut base_crit = crit_override
                .or_else(|| self.src_num(o, "CritChance"))
                .unwrap_or(0.0);
            let from_main = self.c.flag(list, c, &["BaseCritFromMainHand"])?;
            let from_parent = self
                .c
                .flag(list, c, &["AttackCritIsEqualToParentMainHand"])?;
            if from_main {
                base_crit = self.weapon_num(actor, 0, "CritChance").unwrap_or(f64::NAN);
            } else if from_parent {
                let parent = self.c.actors.actors[actor].parent;
                if let Some(c) = parent.and_then(|p| self.weapon_num(p, 0, "CritChance")) {
                    base_crit = c;
                }
            }
            if crit_override == Some(100.0) {
                self.set_po(o, "PreEffectiveCritChance", 100.0);
                self.set_po(o, "PreBifurcateCritChance", 100.0);
                self.set_po(o, "CritChance", 100.0);
            } else {
                let (mut base, mut inc, mut more) = (0.0, 0.0, 1.0);
                if crit_override.take().is_none() {
                    base = self.c.base(list, c, &["CritChance"])?
                        + if effective {
                            self.c.base(edb, None, &["SelfCritChance"])?
                        } else {
                            0.0
                        };
                    inc = self.c.inc(list, c, &["CritChance"])?
                        + if effective {
                            self.c.inc(edb, None, &["SelfCritChance"])?
                        } else {
                            0.0
                        };
                    more = self.c.more(list, c, &["CritChance"])?
                        * if effective {
                            self.c.more(edb, None, &["SelfCritChance"])?
                        } else {
                            1.0
                        };
                }
                let mut crit =
                    round((base_crit + base) * (1.0 + inc / 100.0) * more * 100.0, 0.0) / 100.0;
                let pre_cap = crit;
                let cap = match self.c.override_num(list, None, &["CritChanceCap"])? {
                    Some(v) => v,
                    None => self.c.base(list, c, &["CritChanceCap"])?,
                };
                crit = m_min(crit, cap);
                if base_crit + base > 0.0 {
                    crit = m_max(crit, 0.0);
                }
                self.set_po(o, "PreEffectiveCritChance", crit);
                let pre_effective = crit;
                let mut rolls = 0.0;
                if effective && self.c.flag(list, c, &["CritChanceLucky"])? {
                    rolls += 1.0;
                }
                if self.c.flag(list, sc, &["ExtremeLuck"])? {
                    rolls *= 2.0;
                }
                if rolls != 0.0 {
                    crit = if self.c.flag(db, None, &["Unexciting"])? {
                        (3.0 * (crit / 100.0).powf(2.0) - 2.0 * (crit / 100.0).powf(3.0)) * 100.0
                    } else {
                        (1.0 - (1.0 - crit / 100.0).powf(rolls + 1.0)) * 100.0
                    };
                }
                self.set_po(o, "PreBifurcateCritChance", crit);
                let pre_bifurcate = crit;
                if effective && self.c.flag(list, c, &["BifurcateCrit"])? {
                    crit = (1.0 - (1.0 - crit / 100.0).powf(2.0)) * 100.0;
                }
                let pre_skill_use = crit;
                let mut pre_hit_check = crit;
                if effective {
                    if self.c.flag(list, sc, &["Every3UseCrit"])? {
                        crit = (2.0 * crit + 100.0) / 3.0;
                    }
                    if self.c.flag(list, sc, &["Every5UseCrit"])? {
                        crit = (4.0 * crit + 100.0) / 5.0;
                    }
                    pre_hit_check = crit;
                    crit = crit * self.po(o, "AccuracyHitChance").unwrap_or(f64::NAN) / 100.0;
                }
                self.set_po(o, "CritChance", crit);
                if crit != base_crit && self.has_bd(actor) {
                    let from = if from_main {
                        " from main weapon"
                    } else if from_parent {
                        " from parent main weapon"
                    } else {
                        ""
                    };
                    let mut l = vec![if base != 0.0 {
                        sf!("(%g + %g) ^8(base%s)", base_crit, base, from)
                    } else {
                        sf!("%g ^8(base%s)", base_crit + base, from)
                    }];
                    if inc != 0.0 {
                        l.push(sf!("x %.2f", 1.0 + inc / 100.0) + " ^8(increased/reduced)");
                    }
                    if more != 1.0 {
                        l.push(sf!("x %.2f", more) + " ^8(more/less)");
                    }
                    l.push(sf!("= %.2f%% ^8(crit chance)", pre_effective));
                    if pre_cap > 100.0 {
                        let over = pre_cap - 100.0;
                        l.push(sf!(
                            "Crit is overcapped by %.2f%% (%d%% increased Critical Strike Chance)",
                            over,
                            over / more / (base_crit + base) * 100.0
                        ));
                    }
                    let pre = l.clone();
                    let acc = self.po(o, "AccuracyHitChance").unwrap_or(f64::NAN);
                    if effective {
                        if rolls != 0.0 {
                            if self.c.flag(list, sc, &["Unexciting"])? {
                                l.push("Crit Chance is Unexciting:".into());
                                l.push(sf!(
                                    "(3 x %.4f^ 2) - (2 x %.4f^ 3)",
                                    pre_effective / 100.0,
                                    pre_effective / 100.0
                                ));
                            } else {
                                l.push("Crit Chance is Lucky:".into());
                                l.push(sf!(
                                    "1 - (1 - %.4f)^ %d",
                                    pre_effective / 100.0,
                                    rolls + 1.0
                                ));
                            }
                            l.push(sf!("= %.2f%%", pre_bifurcate));
                        }
                        if self.c.flag(list, c, &["BifurcateCrit"])? {
                            l.push("Critical Strike Bifurcates:".into());
                            l.push(sf!(
                                "1 - (1 - %.4f) x (1 - %.4f)",
                                pre_bifurcate / 100.0,
                                pre_bifurcate / 100.0
                            ));
                            l.push(sf!("= %.2f%%", pre_skill_use));
                        }
                        let e3 = self.c.flag(list, sc, &["Every3UseCrit"])?;
                        let e5 = self.c.flag(list, sc, &["Every5UseCrit"])?;
                        if e3 {
                            l.push(sf!(
                                "+ %.2f%% ^8(crit every 3rd use)",
                                (2.0 * pre_skill_use + 100.0) / 3.0 - pre_skill_use
                            ));
                        }
                        if e5 {
                            l.push(sf!(
                                "+ %.2f%% ^8(crit every 5th use)",
                                (4.0 * pre_skill_use + 100.0) / 5.0 - pre_skill_use
                            ));
                        }
                        if e3 || e5 {
                            l.push(sf!("= %.2f%%", pre_hit_check));
                        }
                    }
                    if effective && acc < 100.0 {
                        l.push("Crit confirmation roll:".into());
                        l.push(sf!("%.2f%%", pre_hit_check));
                        l.push(sf!("x %.2f ^8(chance to hit)", acc / 100.0));
                        l.push(sf!("= %.2f%%", crit));
                    }
                    let b = self.bd(actor).unwrap();
                    b.set_lines(Self::pkey(o, "PreEffectiveCritChance"), pre);
                    b.set_lines(Self::pkey(o, "CritChance"), l);
                }
            }
        }
        if self.po_val(o, "CritEffect").is_none() {
            if self.c.flag(list, c, &["NoCritMultiplier"])? {
                self.set_po(o, "CritMultiplier", 1.0);
            } else {
                let mut extra = self.c.base(list, c, &["CritMultiplier"])? / 100.0;
                if let Some(m) = self.c.override_num(list, sc, &["CritMultiplier"])? {
                    extra = (m - 100.0) / 100.0;
                }
                if effective {
                    let enemy_inc = 1.0 + self.c.inc(edb, None, &["SelfCritMultiplier"])? / 100.0;
                    let enemy_base = self.c.base(edb, None, &["SelfCritMultiplier"])?;
                    extra += enemy_base / 100.0;
                    extra = round(extra * enemy_inc, 2.0);
                    if enemy_inc != 1.0 && self.has_bd(actor) {
                        let own = self.c.base(list, c, &["CritMultiplier"])?;
                        let l = vec![
                            sf!("%d%% ^8(additional extra damage)", (enemy_base + own) / 100.0),
                            sf!(
                                "x %.2f ^8(increased/reduced extra crit damage taken by enemy)",
                                enemy_inc
                            ),
                            sf!("= %d%% ^8(extra crit damage)", extra * 100.0),
                        ];
                        self.bd(actor)
                            .unwrap()
                            .set_lines(Self::pkey(o, "CritMultiplier"), l);
                    }
                }
                let crit_override = self.c.override_num(list, c, &["CritChance"])?;
                if effective
                    && self.c.flag(list, c, &["BifurcateCrit"])?
                    && let Some(pre) = self.po(o, "PreBifurcateCritChance")
                    && crit_override != Some(100.0)
                {
                    let mut multi_chance = pre.powf(2.0) / 100.0;
                    let effective_crit = self.po(o, "CritChance").unwrap_or(f64::NAN);
                    let mut use_chance = 1.0;
                    if self.c.flag(list, sc, &["Every3UseCrit"])? {
                        use_chance = use_chance * 2.0 / 3.0;
                    }
                    if self.c.flag(list, sc, &["Every5UseCrit"])? {
                        use_chance = use_chance * 4.0 / 5.0;
                    }
                    multi_chance *= use_chance;
                    let conditional = if effective_crit > 0.0 {
                        multi_chance / effective_crit
                    } else {
                        0.0
                    };
                    self.set_po(o, "CritBifurcates", 1.0 + conditional);
                    if let Some(b) = self.bd(actor) {
                        let mut l = vec![
                            sf!("%.2f%% ^8(pre-bifurcate crit chance)", pre),
                            sf!("x %.2f%%", pre),
                        ];
                        if use_chance < 1.0 {
                            l.push(sf!("x %.2f%% ^8(uses that can bifurcate)", use_chance * 100.0));
                        }
                        l.push(sf!("= %.2f%% ^8(chance both crit rolls succeed)", multi_chance));
                        l.push(sf!("/ %.2f%% ^8(effective crit chance)", effective_crit));
                        l.push(sf!("= %.2f ^8(crit Bifurcates effect)", 1.0 + conditional));
                        b.set_lines(Self::pkey(o, "CritBifurcates"), l);
                    }
                    extra += conditional * extra;
                    let hit = self.mf("Hit");
                    self.c.add(
                        list,
                        new_mod(
                            "CritMultiplier",
                            More,
                            floor(conditional * 100.0, 2.0),
                            Some("Bifurcated Crit Damage Bonus"),
                            hit,
                            0,
                            vec![],
                        ),
                    );
                }
                self.set_po(o, "CritMultiplier", 1.0 + m_max(0.0, extra));
            }
            let pct = self.po(o, "CritChance").unwrap_or(f64::NAN) / 100.0;
            let multi = self.po(o, "CritMultiplier").unwrap_or(f64::NAN);
            let effect = 1.0 - pct + pct * multi;
            self.set_po(o, "CritEffect", effect);
            if effect != 1.0
                && let Some(b) = self.bd(actor)
            {
                b.set_lines(
                    Self::pkey(o, "CritEffect"),
                    vec![
                        sf!("(1 - %.4f) ^8(portion of damage from non-crits)", pct),
                        sf!(
                            "+ [ (%.4f x %g) ^8(portion of damage from crits)",
                            pct,
                            multi
                        ),
                        sf!("= %.3f", effect),
                    ],
                );
            }
            let v = (self.c.base(list, c, &["CritMultiplier"])? - 50.0)
                * self.c.base(list, c, &["CritMultiplierAppliesToDegen"])?
                / 10000.0;
            self.set_po(o, "BonusCritDotMultiplier", v);
        }
        if self.po(o, "CritChance") != Some(0.0) {
            self.c.set_cond(list, "CritInPast8Sec", true);
        }
        Ok(())
    }
}

/// `damageStatsForTypes[typeFlags]`
fn damage_stats_for_types(flags: u32) -> Vec<&'static str> {
    let mut names = vec!["Damage"];
    for (t, f) in [
        ("Physical", 0x01),
        ("Lightning", 0x02),
        ("Cold", 0x04),
        ("Fire", 0x08),
        ("Elemental", 0x0E),
        ("Chaos", 0x10),
    ] {
        if flags & f != 0 {
            names.push(match t {
                "Physical" => "PhysicalDamage",
                "Lightning" => "LightningDamage",
                "Cold" => "ColdDamage",
                "Fire" => "FireDamage",
                "Elemental" => "ElementalDamage",
                _ => "ChaosDamage",
            });
        }
    }
    names
}

fn type_flag(t: &str) -> u32 {
    match t {
        "Physical" => 0x01,
        "Lightning" => 0x02,
        "Cold" => 0x04,
        "Fire" => 0x08,
        _ => 0x10,
    }
}

impl<'a> Perform<'a> {
    /// `calcDamage(activeSkill, output, cfg, breakdown, damageType,
    /// typeFlags)` with the current pass's output and the given cfg and
    /// conversion table
    pub(crate) fn calc_damage(
        &self,
        o: &Off,
        cfg: &Cfg,
        conv: &crate::calc_offence::ConversionTable,
        t: &str,
        type_flags: u32,
    ) -> Result<(f64, f64)> {
        let (min, max) = self.calc_damage_raw(o, cfg, conv, t, type_flags, None, None)?;
        Ok((round(min, 0.0), round(max, 0.0)))
    }

    /// `calcDamage` with a breakdown: the rows of its `damageTypes` table
    pub(crate) fn calc_damage_bd(
        &self,
        o: &Off,
        cfg: &Cfg,
        conv: &crate::calc_offence::ConversionTable,
        t: &str,
        type_flags: u32,
        rows: &mut Vec<Row>,
    ) -> Result<(f64, f64)> {
        let (min, max) = self.calc_damage_raw(o, cfg, conv, t, type_flags, Some(rows), None)?;
        Ok((round(min, 0.0), round(max, 0.0)))
    }

    /// `calcDamage` with `convDst`: converted paths stay fractional
    #[allow(clippy::too_many_arguments)]
    fn calc_damage_raw(
        &self,
        o: &Off,
        cfg: &Cfg,
        conv: &crate::calc_offence::ConversionTable,
        t: &str,
        type_flags: u32,
        mut rows: Option<&mut Vec<Row>>,
        conv_dst: Option<usize>,
    ) -> Result<(f64, f64)> {
        let list = o.list;
        let flags = type_flags | type_flag(t);
        let ti = dmg_index(t);
        let (mut add_min, mut add_max) = (0.0, 0.0);
        for (oi, other) in DMG_TYPES.iter().enumerate() {
            if oi == ti {
                break;
            }
            let conv_mult = conv[oi].total[ti];
            if conv_mult > 0.0 {
                let (min, max) =
                    self.calc_damage_raw(o, cfg, conv, other, flags, rows.as_deref_mut(), Some(ti))?;
                add_min += min * conv_mult;
                add_max += max * conv_mult;
            }
        }
        let base_min = self.po(o, &format!("{t}MinBase")).unwrap_or(f64::NAN);
        let base_max = self.po(o, &format!("{t}MaxBase")).unwrap_or(f64::NAN);
        let num = crate::pat::num;
        let range = |a: f64, b: f64| format!("{} to {}", num(a), num(b));
        let dst = |d: usize, of: &[f64; 5], fmt: &str| sf!(fmt, of[d] * 100.0, DMG_TYPES[d]);
        let added = (add_min != 0.0 || add_max != 0.0)
            .then(|| range(round(add_min, 0.0), round(add_max, 0.0)));
        if base_min == 0.0 && base_max == 0.0 {
            if let Some(rows) = rows
                && let Some(a) = added
            {
                let mut row: Row = vec![
                    ("source", t.into()),
                    ("convSrc", a.clone().into()),
                    ("total", a.into()),
                ];
                if let Some(d) = conv_dst {
                    row.push(("convDst", dst(d, &conv[ti].conversion, "%d%% to %s").into()));
                    row.push(("gainDst", dst(d, &conv[ti].gain, "%d%% gained as %s").into()));
                }
                rows.push(row);
            }
            return Ok((add_min, add_max));
        }
        let c = Some(cfg);
        let names = damage_stats_for_types(flags);
        let inc = 1.0 + self.c.inc(list, c, &names)? / 100.0;
        let more = self.c.more(list, c, &names)?;
        let generic_min = self.c.more(list, c, &["MinDamage"])?;
        let generic_max = self.c.more(list, c, &["MaxDamage"])?;
        let min_name = format!("Min{t}Damage");
        let max_name = format!("Max{t}Damage");
        let more_min = self.c.more(list, c, &[&min_name])?;
        let more_max = self.c.more(list, c, &[&max_name])?;
        let inc_min = 1.0 + self.c.inc(list, c, &[&min_name])? / 100.0;
        let inc_max = 1.0 + self.c.inc(list, c, &[&max_name])? / 100.0;
        if let Some(rows) = rows {
            let x = |v: f64| Cell::from((v != 1.0).then(|| format!("x {}", num(v))));
            let mut row: Row = vec![
                ("source", t.into()),
                ("base", range(base_min, base_max).into()),
                ("inc", x(inc)),
                ("more", x(more)),
                ("convSrc", added.into()),
                (
                    "total",
                    range(
                        round(base_min * inc * more + add_min, 0.0),
                        round(base_max * inc * more + add_max, 0.0),
                    )
                    .into(),
                ),
            ];
            if let Some(d) = conv_dst {
                let c = conv[ti].conversion[d] > 0.0;
                let g = conv[ti].gain[d] > 0.0;
                row.push((
                    "convDst",
                    Cell::from(c.then(|| dst(d, &conv[ti].conversion, "%d%% to %s"))),
                ));
                row.push((
                    "gainDst",
                    Cell::from(g.then(|| dst(d, &conv[ti].gain, "%d%% gained as %s"))),
                ));
            }
            rows.push(row);
        }
        Ok((
            ((base_min * inc * more) * generic_min + add_min) * more_min * inc_min,
            ((base_max * inc * more) * generic_max + add_max) * more_max * inc_max,
        ))
    }

    /// The second passes: damage
    fn hit_passes(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb, db) = (o.skill, o.list, o.actor, o.edb, o.db);
        let effective = self.env.mode.effective;
        let calcs_mode = self.calcs_mode();
        for p in 0..o.passes.len() {
            o.pass = p;
            for k in [
                "OffensiveWarcryEffect",
                "MaxOffensiveWarcryEffect",
                "TheoreticalOffensiveWarcryEffect",
                "TheoreticalMaxOffensiveWarcryEffect",
                "RallyingHitEffect",
                "AilmentWarcryEffect",
            ] {
                self.set_go(o, k, 1.0);
            }
            self.set_go(o, "GlobalWarcryUptimeRatio", 0.0);
            if self.env.mode.buffs {
                self.exerts(o)?;
                self.pacts(o)?;
            }
            self.set_po(o, "RuthlessBlowHitEffect", 1.0);
            self.set_po(o, "RuthlessBlowAilmentEffect", 1.0);
            self.set_po(o, "FistOfWarDamageEffect", 1.0);
            if self.env.mode.combat {
                self.ruthless(o)?;
            }
            if self.skill_name(i) == "Explosive Arrow" {
                let id = self.skill(i).active_effect.granted_effect.id.clone();
                self.skill_func(o, &id, "explosiveArrowFunc")?;
            }
            self.crits(o)?;
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            self.set_po(o, "ScaledDamageEffect", 1.0);
            let crit = self.po(o, "CritChance").unwrap_or(f64::NAN);
            // Triple and double damage
            let triple_on_crit = m_min(self.c.base(list, c, &["TripleDamageChanceOnCrit"])?, 100.0);
            self.set_po(o, "TripleDamageChanceOnCrit", triple_on_crit);
            // `Sum(...) or 0 + (...)` adds nothing to the sum
            let triple = m_min(self.c.base(list, c, &["TripleDamageChance"])?, 100.0);
            self.set_po(o, "TripleDamageChance", triple);
            self.set_po(o, "TripleDamageEffect", 2.0 * triple / 100.0);
            let dd_on_crit = m_min(self.c.base(list, c, &["DoubleDamageChanceOnCrit"])?, 100.0);
            self.set_po(o, "DoubleDamageChanceOnCrit", dd_on_crit);
            let mut dd = m_min(
                self.c.base(list, c, &["DoubleDamageChance"])?
                    + if effective {
                        self.c.base(edb, c, &["SelfDoubleDamageChance"])?
                    } else {
                        0.0
                    }
                    + dd_on_crit * crit / 100.0,
                100.0,
            );
            if let Some(up) = self.go(o, "IntimidatingUpTimeRatio") {
                dd = if self.c.flag(list, None, &["Condition:WarcryMaxHit"])? {
                    100.0
                } else {
                    m_min(dd + up, 100.0)
                };
            }
            if triple > 0.0 {
                dd = m_max(dd - triple * dd / 100.0, 0.0);
            }
            self.set_po(o, "DoubleDamageChance", dd);
            self.set_po(o, "DoubleDamageEffect", dd / 100.0);
            let scaled = 1.0 + dd / 100.0 + 2.0 * triple / 100.0;
            self.set_po(o, "ScaledDamageEffect", scaled);
            let hit_chance = self.po(o, "HitChance").unwrap_or(f64::NAN);
            let speed = self
                .go(o, "HitSpeed")
                .or_else(|| self.go(o, "Speed"))
                .unwrap_or(f64::NAN);
            let hit_rate =
                hit_chance / 100.0 * speed * self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
            // Calculate culling DPS
            let mut critical_cull = self
                .c
                .max(list, c, &["CriticalCullPercent"])?
                .unwrap_or(0.0);
            if critical_cull > 0.0 {
                critical_cull = m_min(
                    critical_cull,
                    critical_cull * (1.0 - (1.0 - crit / 100.0).powf(hit_rate)),
                );
            }
            let regular_cull = self.c.max(list, c, &["CullPercent"])?.unwrap_or(0.0);
            let cull = m_max(critical_cull, regular_cull);
            self.set_go(o, "CullPercent", cull);
            self.set_go(o, "CullMultiplier", 100.0 / (100.0 - cull));
            let reservation = self.c.base(edb, None, &["LifeReservationPercent"])?;
            self.set_go(o, "ReservationDpsMultiplier", 100.0 / (100.0 - reservation));
            self.run_skill_func(o, "postCritFunc")?;
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            if self.c.flag(list, c, &["AllAddedDamageAsLightning"])?
                || self.c.flag(list, c, &["AllAddedDamageAsCold"])?
            {
                return Err("not ported: added damage redirection".into());
            }
            // Calculate base hit damage
            let level_field = |p: &Self, k: &str| p.effect_level(i).and_then(|l| l.field(k));
            for t in DMG_TYPES {
                let base_mult = level_field(self, "baseMultiplier")
                    .or_else(|| self.sd_num(i, "baseMultiplier"))
                    .unwrap_or(1.0);
                let effectiveness = level_field(self, "damageEffectiveness")
                    .or_else(|| self.sd_num(i, "damageEffectiveness"))
                    .unwrap_or(1.0);
                let (min_n, max_n) = (format!("{t}Min"), format!("{t}Max"));
                let added_min = self.c.base(list, c, &[&min_n])?
                    + self.c.base(edb, c, &[&format!("Self{min_n}")])?;
                let added_max = self.c.base(list, c, &[&max_n])?
                    + self.c.base(edb, c, &[&format!("Self{max_n}")])?;
                let added_mult =
                    self.c
                        .calc_mod(list, c, &[&format!("Added{t}Damage"), "AddedDamage"])?;
                let s = |p: &Self, k: &str| p.src_num(o, k).unwrap_or(0.0);
                let base_min = (s(self, &min_n) + s(self, &format!("{t}BonusMin"))) * base_mult
                    + added_min * effectiveness * added_mult;
                let base_max = (s(self, &max_n) + s(self, &format!("{t}BonusMax"))) * base_mult
                    + added_max * effectiveness * added_mult;
                self.set_po(o, &format!("{min_n}Base"), base_min);
                self.set_po(o, &format!("{max_n}Base"), base_max);
                if self.has_bd(actor) {
                    let mut l = Vec::new();
                    if base_min != 0.0 && base_max != 0.0 {
                        l.push("Base damage:".to_string());
                        let mut plus = "";
                        let (a, b) = (self.src_num(o, &min_n), self.src_num(o, &max_n));
                        if a.unwrap_or(0.0) != 0.0 || b.unwrap_or(0.0) != 0.0 {
                            let from = if self.src_val(o, "type").is_some() {
                                "weapon"
                            } else {
                                "skill"
                            };
                            let arg = |v: Option<f64>| v.map_or(crate::breakdown::Arg::Str("nil"), crate::breakdown::Arg::Num);
                            l.push(crate::breakdown::lua_format(
                                "%d to %d ^8(base damage from %s)",
                                &[arg(a), arg(b), from.into()],
                            ));
                            if base_mult != 1.0 {
                                l.push(sf!("x %.2f ^8(base damage multiplier)", base_mult));
                            }
                            plus = "+ ";
                        }
                        if added_min != 0.0 || added_max != 0.0 {
                            l.push(sf!("%s%d to %d ^8(added damage)", plus, added_min, added_max));
                            if effectiveness != 1.0 {
                                l.push(sf!("x %.2f ^8(damage effectiveness)", effectiveness));
                            }
                            if added_mult != 1.0 {
                                l.push(sf!("x %.2f ^8(added damage multiplier)", added_mult));
                            }
                        }
                        l.push(sf!("= %.1f to %.1f", base_min, base_max));
                    }
                    let b = Breakdown {
                        lines: l,
                        damage_types: Some(Vec::new()),
                        ..Breakdown::default()
                    };
                    self.bd(actor).unwrap().set(Self::pkey(o, t), b);
                }
            }
            self.hit_damage(o, hit_rate)?;
            let _ = (calcs_mode, actor, db);
        }
        Ok(())
    }

    /// Hit damage by type for crits and non-crits, leech, gains and DPS
    fn hit_damage(&mut self, o: &mut Off, hit_rate: f64) -> Result<()> {
        let (i, list, actor, edb, db) = (o.skill, o.list, o.actor, o.edb, o.db);
        let skill_cfg = self.cfg(i);
        let sc = Some(&skill_cfg);
        let effective = self.env.mode.effective;
        let pdb = self.pdb();
        let (mut total_hit_min, mut total_hit_max, mut total_hit_avg) = (0.0, 0.0, 0.0);
        let (mut total_crit_min, mut total_crit_max, mut total_crit_avg) = (0.0, 0.0, 0.0);
        let ghost_reaver = self.c.flag(list, None, &["GhostReaver"])?;
        let mut ghost_reaver_leech = 0.0;
        let bd = self.has_bd(actor);
        for k in [
            "LifeLeech",
            "LifeLeechInstant",
            "EnergyShieldLeech",
            "EnergyShieldLeechInstant",
            "ManaLeech",
            "ManaLeechInstant",
            "impaleStoredHitAvg",
        ] {
            self.set_po(o, k, 0.0);
        }
        let conv = self.pconv(o).clone();
        for pass in 1..=2 {
            self.set_pass_cond(o, "CriticalStrike", pass == 1);
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            let (mut life_total, mut es_total, mut mana_total) = (0.0, 0.0, 0.0);
            let no_life = self.c.flag(list, c, &["CannotLeechLife"])?
                || self.c.flag(edb, None, &["CannotLeechLifeFromSelf"])?
                || self.c.flag(list, c, &["CannotGainLife"])?;
            let no_es = self.c.flag(list, c, &["CannotLeechEnergyShield"])?
                || self
                    .c
                    .flag(edb, None, &["CannotLeechEnergyShieldFromSelf"])?
                || self.c.flag(list, c, &["CannotGainEnergyShield"])?;
            let no_mana = self.c.flag(list, c, &["CannotLeechMana"])?
                || self.c.flag(edb, None, &["CannotLeechManaFromSelf"])?
                || self.c.flag(list, c, &["CannotGainMana"])?;
            let crit = self.po(o, "CritChance").unwrap_or(f64::NAN);
            for (ti, t) in DMG_TYPES.iter().enumerate() {
                let t = *t;
                let (mut hit_min, mut hit_max, mut hit_avg) = (0.0, 0.0, 0.0);
                let bd_key = (pass == 2 && bd).then(|| Self::pkey(o, t));
                if self.has_flag(i, "hit") && o.can_deal[ti] {
                    let (mn, mx) = match &bd_key {
                        Some(k) => {
                            let mut rows = Vec::new();
                            let r = self.calc_damage_bd(o, &cfg, &conv, t, 0, &mut rows)?;
                            let b = self.bd(actor).unwrap().existing(k)?;
                            b.damage_types.get_or_insert_with(Vec::new).extend(rows);
                            r
                        }
                        None => self.calc_damage(o, &cfg, &conv, t, 0)?,
                    };
                    hit_min = mn;
                    hit_max = mx;
                    let conv_mult = conv[ti].mult;
                    let max_hit = self.c.flag(list, None, &["Condition:WarcryMaxHit"])?;
                    let warcry = if max_hit {
                        self.go(o, "MaxOffensiveWarcryEffect")
                    } else {
                        self.go(o, "OffensiveWarcryEffect")
                    }
                    .unwrap_or(f64::NAN);
                    if let Some(k) = &bd_key {
                        let l = self.hit_damage_lines(o, mn, mx, conv_mult, max_hit);
                        self.bd(actor).unwrap().existing(k)?.lines.extend(l);
                    }
                    let all = conv_mult
                        * self.po(o, "ScaledDamageEffect").unwrap_or(f64::NAN)
                        * self.po(o, "RuthlessBlowHitEffect").unwrap_or(f64::NAN)
                        * self.po(o, "FistOfWarDamageEffect").unwrap_or(f64::NAN)
                        * warcry;
                    self.set_po(o, "allMult", all);
                    let mut all_mult = all;
                    if pass == 1 {
                        all_mult *= self.po(o, "CritMultiplier").unwrap_or(f64::NAN);
                    }
                    hit_min *= all_mult;
                    hit_max *= all_mult;
                    let lucky = self.c.flag(list, sc, &["LuckyHits"])?
                        || (pass == 2
                            && t == "Lightning"
                            && self.c.flag(list, sc, &["LightningNoCritLucky"])?)
                        || (pass == 1 && self.c.flag(list, sc, &["CritLucky"])?)
                        || (t == "Lightning" && self.c.flag(db, None, &["LightningLuckHits"])?)
                        || (t == "Chaos" && self.c.flag(db, None, &["ChaosLuckyHits"])?)
                        || (t == "Fire" && self.c.flag(db, None, &["FireLuckyHits"])?)
                        || (t == "Cold" && self.c.flag(db, None, &["ColdLuckyHits"])?)
                        || (is_elemental(t) && self.c.flag(list, sc, &["ElementalLuckHits"])?);
                    let mut lucky_chance = if lucky {
                        1.0
                    } else {
                        m_min(self.c.base(list, sc, &["LuckyHitsChance"])?, 100.0) / 100.0
                    };
                    if self.c.flag(list, sc, &["UnluckyHits"])? {
                        lucky_chance -= 1.0;
                    }
                    let mut rolls = lucky_chance;
                    if self.c.flag(list, sc, &["ExtremeLuck"])? {
                        rolls *= 2.0;
                    }
                    if self.c.flag(list, sc, &["Unexciting"])? {
                        rolls = 0.0;
                    }
                    self.set_po(o, "DamageRolls", rolls);
                    if pass == 1 {
                        self.set_po(o, "CritDamageRolls", rolls);
                    }
                    let rolls = rolls.abs() + 2.0;
                    let not_lucky = hit_min / rolls + hit_max / rolls;
                    let avg_lucky = hit_min / rolls + (rolls - 1.0) * hit_max / rolls;
                    let avg_unlucky = (rolls - 1.0) * hit_min / rolls + hit_max / rolls;
                    hit_avg = if lucky_chance >= 0.0 {
                        not_lucky * (1.0 - lucky_chance) + avg_lucky * lucky_chance
                    } else {
                        not_lucky * (1.0 - lucky_chance.abs()) + avg_unlucky * lucky_chance.abs()
                    };
                    if (hit_min != 0.0 || hit_max != 0.0) && effective {
                        let (eff_mult, eff_bd) =
                            self.hit_eff_mult(o, &cfg, t, pass, hit_avg, crit, bd_key.is_some())?;
                        hit_min *= eff_mult;
                        hit_max *= eff_mult;
                        hit_avg *= eff_mult;
                        if self.calcs_mode() {
                            self.set_po(o, &format!("{t}EffMult"), eff_mult);
                        }
                        if let (Some(k), Some(l)) = (&bd_key, eff_bd) {
                            let b = self.bd(actor).unwrap();
                            b.existing(k)?
                                .push(sf!("x %.3f ^8(effective DPS modifier)", eff_mult));
                            b.set_lines(Self::pkey(o, &format!("{t}EffMult")), l);
                        }
                    }
                    if let Some(k) = &bd_key {
                        let l = sf!("= %d to %d", hit_min, hit_max);
                        self.bd(actor).unwrap().existing(k)?.push(l);
                    }
                    // Leech for this damage type
                    let (mut life, mut es, mut mana) = (0.0, 0.0, 0.0);
                    if self.has_flag(i, "mine")
                        || self.has_flag(i, "trap")
                        || self.has_flag(i, "totem")
                    {
                        life = self.c.base(list, c, &["DamageLifeLeechToPlayer"])?;
                    } else {
                        let self_life = self.c.base(edb, c, &["SelfDamageLifeLeech"])? / 100.0;
                        if self.c.flag(list, None, &["LifeLeechBasedOnChaosDamage"])? {
                            if t == "Chaos" {
                                life = self.c.base(
                                    list,
                                    c,
                                    &[
                                        "DamageLeech",
                                        "DamageLifeLeech",
                                        "PhysicalDamageLifeLeech",
                                        "LightningDamageLifeLeech",
                                        "ColdDamageLifeLeech",
                                        "FireDamageLifeLeech",
                                        "ChaosDamageLifeLeech",
                                        "ElementalDamageLifeLeech",
                                    ],
                                )? + self_life;
                            }
                        } else {
                            let tn = format!("{t}DamageLifeLeech");
                            let mut names = vec!["DamageLeech", "DamageLifeLeech", tn.as_str()];
                            if is_elemental(t) {
                                names.push("ElementalDamageLifeLeech");
                            }
                            life = self.c.base(list, c, &names)? + self_life;
                        }
                        let tn = format!("{t}DamageEnergyShieldLeech");
                        let mut names = vec!["DamageEnergyShieldLeech", tn.as_str()];
                        if is_elemental(t) {
                            names.push("ElementalDamageEnergyShieldLeech");
                        }
                        es = self.c.base(list, c, &names)?
                            + self.c.base(edb, c, &["SelfDamageEnergyShieldLeech"])? / 100.0;
                        let tn = format!("{t}DamageManaLeech");
                        let mut names = vec!["DamageLeech", "DamageManaLeech", tn.as_str()];
                        if is_elemental(t) {
                            names.push("ElementalDamageManaLeech");
                        }
                        mana = self.c.base(list, c, &names)?
                            + self.c.base(edb, c, &["SelfDamageManaLeech"])? / 100.0;
                    }
                    if life > 0.0 && !no_life {
                        life_total += hit_avg * life / 100.0;
                    }
                    if mana > 0.0 && !no_mana {
                        mana_total += hit_avg * mana / 100.0;
                    }
                    if es > 0.0 && !no_es {
                        es_total += hit_avg * es / 100.0;
                    }
                } else if let Some(b) = self.bd(actor) {
                    let l = vec![format!("You can't deal {t} damage")];
                    b.set_lines(Self::pkey(o, t), l);
                }
                if pass == 1 {
                    self.set_po(o, &format!("{t}CritAverage"), hit_avg);
                    total_crit_avg += hit_avg;
                    total_crit_min += hit_min;
                    total_crit_max += hit_max;
                } else {
                    if self.calcs_mode() {
                        self.set_po(o, &format!("{t}Min"), hit_min);
                        self.set_po(o, &format!("{t}Max"), hit_max);
                    }
                    self.set_po(o, &format!("{t}HitAverage"), hit_avg);
                    total_hit_avg += hit_avg;
                    total_hit_min += hit_min;
                    total_hit_max += hit_max;
                }
            }
            if let Some(v) = self.sd_num(i, "lifeLeechPerUse")
                && !no_life
            {
                life_total += v;
            }
            if let Some(v) = self.sd_num(i, "manaLeechPerUse") {
                mana_total += v;
            }
            let g = |p: &Self, k: &str| p.go(o, k).unwrap_or(f64::NAN);
            life_total = if ghost_reaver {
                m_min(life_total, g(self, "MaxEnergyShieldLeechInstance"))
            } else {
                m_min(life_total, g(self, "MaxLifeLeechInstance"))
            };
            es_total = m_min(es_total, g(self, "MaxEnergyShieldLeechInstance"));
            mana_total = m_min(mana_total, g(self, "MaxManaLeechInstance"));
            if ghost_reaver && no_es {
                life_total = 0.0;
            }
            let portion = if pass == 1 {
                crit / 100.0
            } else {
                1.0 - crit / 100.0
            };
            let add = |p: &mut Self, o: &mut Off, k: &str, v: f64| {
                let x = p.po(o, k).unwrap_or(f64::NAN) + v;
                p.set_po(o, k, x);
            };
            if ghost_reaver {
                ghost_reaver_leech += life_total * portion;
            } else {
                add(self, o, "LifeLeech", life_total * portion);
            }
            add(self, o, "EnergyShieldLeech", es_total * portion);
            add(self, o, "ManaLeech", mana_total * portion);
        }
        self.set_po(o, "TotalMin", total_hit_min);
        self.set_po(o, "TotalMax", total_hit_max);
        let cfg = self.pcfg(o);
        let c = Some(&cfg);
        let pv = |p: &Self, o: &Off, k: &str| p.po(o, k).unwrap_or(f64::NAN);
        if self.c.flag(list, sc, &["ElementalEquilibrium"])?
            && !self.config_truthy("EEIgnoreHitDamage")
            && (pv(self, o, "FireHitAverage")
                + pv(self, o, "ColdHitAverage")
                + pv(self, o, "LightningHitAverage")
                > 0.0)
        {
            for t in ["Fire", "Cold", "Lightning"] {
                let v = pv(self, o, &format!("{t}HitAverage")) > 0.0;
                self.c.set_cond(edb, &format!("HitBy{t}Damage"), v);
            }
        }
        let mut highest = "Physical";
        for t in DMG_TYPES {
            let avg = pv(self, o, &format!("{t}HitAverage"));
            if avg > 0.0 {
                if let Some(b) = self.bd(actor) {
                    let l = sf!("Portion of total damage: %d%%", avg / total_hit_avg * 100.0);
                    b.entry(&Self::pkey(o, t)).push(l);
                }
                self.c.add(
                    list,
                    Mod::new(
                        &format!("Condition:{t}HasDamage"),
                        FlagT,
                        true,
                        Some("Config"),
                    ),
                );
                if avg > pv(self, o, &format!("{highest}HitAverage")) {
                    highest = t;
                }
            }
        }
        if !self.c.flag(list, None, &["IsHighestDamageTypeOVERRIDE"])? {
            self.c.add(
                list,
                Mod::new(
                    &format!("Condition:{highest}IsHighestDamageType"),
                    FlagT,
                    true,
                    Some("Config"),
                ),
            );
        }
        // Leech
        let leech_rate_base = self.misc_data("LeechRateBase");
        let instances = |amount: f64, total: f64| {
            if total == 0.0 {
                (0.0, 0.0)
            } else {
                let d = amount / total / leech_rate_base;
                (d, d * hit_rate)
            }
        };
        for (pool, instant) in [
            ("Life", "InstantLifeLeech"),
            ("EnergyShield", "InstantEnergyShieldLeech"),
            ("Mana", "InstantManaLeech"),
        ] {
            let prop = m_max(m_min(self.c.base(list, c, &[instant])?, 100.0), 0.0) / 100.0;
            self.set_po(o, &format!("{pool}LeechInstantProportion"), prop);
            let leech = pv(self, o, &format!("{pool}Leech"));
            if prop > 0.0 {
                self.set_po(o, &format!("{pool}LeechInstant"), leech * prop);
                self.set_po(o, &format!("{pool}Leech"), leech * (1.0 - prop));
            }
            if pool == "Life" && ghost_reaver && ghost_reaver_leech > 0.0 {
                let es = pv(self, o, "EnergyShieldLeech") + ghost_reaver_leech;
                self.set_po(o, "EnergyShieldLeech", es);
                self.set_po(o, "LifeLeech", 0.0);
                self.set_po(o, "LifeLeechInstant", 0.0);
            }
        }
        for pool in ["Life", "EnergyShield", "Mana"] {
            let leech = pv(self, o, &format!("{pool}Leech"));
            let (d, n) = instances(leech, self.go(o, pool).unwrap_or(f64::NAN));
            self.set_po(o, &format!("{pool}LeechDuration"), d);
            self.set_po(o, &format!("{pool}LeechInstances"), n);
            let inst = pv(self, o, &format!("{pool}LeechInstant"));
            self.set_po(o, &format!("{pool}LeechInstantRate"), inst * hit_rate);
        }
        // Gain on hit and kill
        let minion_like =
            self.has_flag(i, "mine") || self.has_flag(i, "trap") || self.has_flag(i, "totem");
        let no_gain_life = self.c.flag(list, c, &["CannotGainLife"])?;
        let no_gain_es = self.c.flag(list, c, &["CannotGainEnergyShield"])?;
        let no_gain_mana = self.c.flag(list, c, &["CannotGainMana"])?;
        let no_life_recovery = self.c.flag(list, c, &["CannotRecoverLifeOutsideLeech"])?;
        let (life_hit, es_hit, mana_hit, life_kill, es_kill, mana_kill) = if minion_like {
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        } else {
            let on_hit = |p: &Self, k: &str| -> Result<f64> {
                Ok(p.c.base(list, c, &[k])? + p.c.base(edb, c, &[&format!("Self{k}")])?)
            };
            (
                if !no_gain_life && !no_life_recovery {
                    on_hit(self, "LifeOnHit")?
                } else {
                    0.0
                },
                if !no_gain_es {
                    on_hit(self, "EnergyShieldOnHit")?
                } else {
                    0.0
                },
                if !no_gain_mana {
                    on_hit(self, "ManaOnHit")?
                } else {
                    0.0
                },
                if !no_gain_life && !no_life_recovery {
                    self.c.base(list, c, &["LifeOnKill"])?.floor()
                } else {
                    0.0
                },
                if !no_gain_es {
                    self.c.base(list, c, &["EnergyShieldOnKill"])?.floor()
                } else {
                    0.0
                },
                if !no_gain_mana {
                    self.c.base(list, c, &["ManaOnKill"])?.floor()
                } else {
                    0.0
                },
            )
        };
        for (k, v) in [
            ("LifeOnHit", life_hit),
            ("EnergyShieldOnHit", es_hit),
            ("ManaOnHit", mana_hit),
        ] {
            self.set_po(o, k, v);
        }
        for (k, v) in [
            ("LifeOnHit", life_hit),
            ("EnergyShieldOnHit", es_hit),
            ("ManaOnHit", mana_hit),
        ] {
            self.set_po(o, &format!("{k}Rate"), v * hit_rate);
        }
        for (k, v) in [
            ("LifeOnKill", life_kill),
            ("EnergyShieldOnKill", es_kill),
            ("ManaOnKill", mana_kill),
        ] {
            self.set_po(o, k, v);
        }
        for (k, n) in [
            ("EnemyLifeRegen", "LifeRegen"),
            ("EnemyManaRegen", "ManaRegen"),
            ("EnemyEnergyShieldRegen", "EnergyShieldRegen"),
        ] {
            let v = self.c.inc(edb, c, &[n])?;
            self.set_po(o, k, v);
        }
        // Average damage and DPS
        let crit = pv(self, o, "CritChance");
        let avg_hit = total_hit_avg * (1.0 - crit / 100.0) + total_crit_avg * crit / 100.0;
        self.set_po(o, "AverageHit", avg_hit);
        if self.has_flag(i, "monsterExplode") {
            self.set_po(
                o,
                "AverageHitToMonsterLifePercentage",
                avg_hit / o.monster_life * 100.0,
            );
            if self.sd_truthy(i, "hitChanceIsExplodeChance") {
                let v = self.po_val(o, "ExplodeChance");
                self.set_po_val(o, "HitChance", v);
            }
        }
        let avg_damage = avg_hit * pv(self, o, "HitChance") / 100.0;
        self.set_po(o, "AverageDamage", avg_damage);
        let burst_hits = self.po(o, "AverageBurstHits").unwrap_or(1.0);
        self.set_go(o, "AverageBurstHits", burst_hits);
        let repeat_penalty = if self.c.flag(list, None, &["HasSeals"])?
            && self.has_type(i, "CanRapidFire")
            && !self.c.flag(list, None, &["NoRepeatBonuses"])?
        {
            self.c.calc_mod(list, sc, &["SealRepeatPenalty"])?
        } else {
            1.0
        };
        self.set_go(
            o,
            "AverageBurstDamage",
            avg_damage + avg_damage * (burst_hits - 1.0) * repeat_penalty,
        );
        self.c
            .set_out_val(actor, "ShowBurst", OutVal::Bool(burst_hits > 1.0));
        let speed = self
            .go(o, "HitSpeed")
            .or_else(|| self.go(o, "Speed"))
            .unwrap_or(f64::NAN);
        let dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
        self.set_po(
            o,
            "TotalDPS",
            avg_damage * speed * dps_mult * o.quantity_multiplier,
        );
        if bd {
            let hit_chance = pv(self, o, "HitChance");
            if pv(self, o, "CritEffect") != 1.0 {
                let damage_rolls = self.po(o, "DamageRolls").unwrap_or(0.0);
                let crit_rolls = self.po(o, "CritDamageRolls").unwrap_or(0.0);
                let rolls = damage_rolls.abs() + 2.0;
                let rolls_crit = crit_rolls.abs() + 2.0;
                let crit_lucky = self.c.flag(list, sc, &["CritLucky"])?;
                let mut l = Vec::new();
                if damage_rolls > 0.0 {
                    l.push(sf!(
                        "(1/%d) x %d + (%d/%d) x %d = %.1f ^8(average from non-crits)",
                        rolls,
                        total_hit_min,
                        rolls - 1.0,
                        rolls,
                        total_hit_max,
                        total_hit_avg
                    ));
                }
                if damage_rolls < 0.0 {
                    l.push(sf!(
                        "(%d/%d) x %d + (1/%d) x %d = %.1f ^8(average from non-crits)",
                        rolls - 1.0,
                        rolls,
                        total_hit_min,
                        rolls,
                        total_hit_max,
                        total_hit_avg
                    ));
                }
                if damage_rolls > 0.0 || crit_rolls > 0.0 || crit_lucky {
                    l.push(sf!(
                        "(1/%d) x %d + (%d/%d) x %d = %.1f ^8(average from crits)",
                        rolls_crit,
                        total_crit_min,
                        rolls_crit - 1.0,
                        rolls_crit,
                        total_crit_max,
                        total_crit_avg
                    ));
                    l.push(String::new());
                }
                if damage_rolls < 0.0 && !crit_lucky {
                    l.push(sf!(
                        "(%d/%d) x %d + (1/%d) x %d = %.1f ^8(average from crits)",
                        rolls - 1.0,
                        rolls,
                        total_crit_min,
                        rolls,
                        total_crit_max,
                        total_crit_avg
                    ));
                    l.push(String::new());
                }
                l.push(sf!(
                    "%.1f x (1 - %.4f) ^8(damage from non-crits)",
                    total_hit_avg,
                    crit / 100.0
                ));
                l.push(sf!(
                    "+ %.1f x %.4f ^8(damage from crits)",
                    total_crit_avg,
                    crit / 100.0
                ));
                l.push(sf!("= %.1f", avg_hit));
                self.bd(actor)
                    .unwrap()
                    .set_lines(Self::pkey(o, "AverageHit"), l);
            }
            if hit_chance < 100.0 {
                let l = vec![
                    sf!("%s:", o.passes[o.pass].label),
                    sf!("%.1f ^8(average hit)", avg_hit),
                    sf!("x %.2f ^8(chance to hit)", hit_chance / 100.0),
                    sf!("= %.1f", avg_damage),
                ];
                self.bd(actor)
                    .unwrap()
                    .set_lines(Self::pkey(o, "AverageDamage"), l);
            }
            let burst = self.go(o, "AverageBurstDamage").unwrap_or(f64::NAN);
            if burst > 0.0 {
                let mut l = vec![sf!("%.1f ^8(average hit)", avg_hit)];
                if hit_chance < 100.0 {
                    l.push(sf!("x %.2f ^8(chance to hit)", hit_chance / 100.0));
                }
                if repeat_penalty < 1.0 {
                    l.push(sf!("x %.2f ^8(number of repeats)", burst_hits - 1.0));
                    l.push(sf!("x %.2f ^8(repeat penalty)", repeat_penalty));
                    l.push(sf!("= %.1f ^8(repeat damage total)", burst - avg_damage));
                    l.push(String::new());
                    l.push(sf!("+ %.1f ^8(first hit)", avg_hit));
                    if hit_chance < 100.0 {
                        l.push(sf!("x %.2f ^8(chance to hit)", hit_chance / 100.0));
                    }
                } else {
                    l.push(sf!("x %.2f ^8(number of hits)", burst_hits));
                }
                l.push(sf!("= %.1f ^8(total burst damage)", burst));
                self.bd(actor)
                    .unwrap()
                    .set_lines("AverageBurstDamage", l);
            }
        }
        // PvP
        for f in [
            "isPvP",
            "notAttackPvP",
            "attackPvP",
            "weapon1AttackPvP",
            "weapon2AttackPvP",
            "notAveragePvP",
        ] {
            self.set_flag(i, f, false);
        }
        if self.config_truthy("PvpScaling") {
            let _ = pdb;
            return Err("not ported: PvP scaling".into());
        }
        Ok(())
    }

    /// The enemy's resistances and damage taken against a hit
    /// (`effMult`); stores the impale's physical damage
    fn hit_eff_mult(
        &mut self,
        o: &mut Off,
        cfg: &Cfg,
        t: &str,
        pass: u32,
        hit_avg: f64,
        crit: f64,
        bd: bool,
    ) -> Result<(f64, Option<Vec<String>>)> {
        let (i, list, edb) = (o.skill, o.list, o.edb);
        let mut source_res = t;
        let c = Some(cfg);
        let mut resist;
        let mut pen = 0.0;
        let tn = format!("{t}DamageTaken");
        let mut taken_inc = self.c.inc(edb, c, &["DamageTaken", &tn])?;
        let taken_more = self.c.more(edb, c, &["DamageTaken", &tn])?;
        let use_resist = |p: &Self, t: &str| -> Result<f64> {
            let ig = format!("Ignore{t}Resistance");
            let mut names = vec![ig.as_str()];
            if is_elemental(t) {
                names.push("IgnoreElementalResistances");
            }
            if p.c.flag(list, c, &names)?
                || p.c
                    .flag(edb, None, &[&format!("SelfIgnore{t}Resistance")])?
            {
                return Ok(0.0);
            }
            let ch = format!("ChanceToIgnore{t}Resistance");
            let mut names = vec![ch.as_str()];
            if is_elemental(t) {
                names.push("ChanceToIgnoreElementalResistances");
            }
            let chance = m_min(m_max(p.c.base(list, c, &names)?, 0.0), 100.0);
            Ok(1.0 - chance / 100.0)
        };
        if t == "Physical" {
            let stored = self.po(o, "impaleStoredHitAvg").unwrap_or(f64::NAN)
                + hit_avg
                    * if pass == 1 {
                        crit / 100.0
                    } else {
                        1.0 - crit / 100.0
                    };
            self.set_po(o, "impaleStoredHitAvg", stored);
            let armour = m_max(self.c.val(edb, "Armour", None)?, 0.0);
            let reduction = armour_reduction_f(
                armour,
                hit_avg * self.c.more(list, c, &["CalcArmourAsThoughDealing"])?,
            );
            let mut ignore = m_min(
                self.c
                    .base(list, c, &["ChanceToIgnoreEnemyPhysicalDamageReduction"])?,
                100.0,
            );
            if ignore > 0.0 && ignore < 100.0 {
                match self
                    .input
                    .config
                    .get("ChanceToIgnoreEnemyPhysicalDamageReductionMode")
                {
                    Some(crate::config::Val::Str(s)) if &**s == "MAX" => ignore = 100.0,
                    Some(crate::config::Val::Str(s)) if &**s == "MIN" => ignore = 0.0,
                    _ => {}
                }
            }
            if self
                .c
                .flag(list, c, &["IgnoreEnemyPhysicalDamageReduction"])?
                || ignore >= 100.0
            {
                resist = 0.0;
            } else {
                resist = m_min(
                    m_max(
                        0.0,
                        self.c.base(edb, None, &["PhysicalDamageReduction"])?
                            + self.c.base(list, c, &["EnemyPhysicalDamageReduction"])?
                            + reduction,
                    ),
                    self.misc_data("EnemyPhysicalDamageReductionCap"),
                );
                if resist > 0.0 {
                    resist *= 1.0
                        - (self.c.base(
                            list,
                            None,
                            &["PartialIgnoreEnemyPhysicalDamageReduction"],
                        )? / 100.0
                            + ignore / 100.0);
                }
            }
        } else {
            resist = self.resist_for_type(o, t, None)?;
            let lowest_chaos = self.c.flag(list, c, &["ChaosDamageUsesLowestResistance"])?
                || self
                    .c
                    .flag(list, c, &["ChaosDamageUsesHighestResistance"])?;
            if (lowest_chaos && t == "Chaos")
                || (self
                    .c
                    .flag(list, c, &["ElementalDamageUsesLowestResistance"])?
                    && is_elemental(t))
            {
                let mut used = t;
                if is_elemental(t) {
                    taken_inc += self.c.inc(edb, c, &["ElementalDamageTaken"])?;
                }
                let highest = self
                    .c
                    .flag(list, c, &["ChaosDamageUsesHighestResistance"])?;
                for e in DMG_TYPES {
                    if is_elemental(e) && use_resist(self, e)? > 0.0 && t != e {
                        let r = self.resist_for_type(o, e, None)?;
                        if (highest && resist < r) || (!highest && resist > r) {
                            resist = r;
                            used = e;
                        }
                    }
                }
                source_res = used;
                if is_elemental(used) {
                    pen = self.c.base(
                        list,
                        c,
                        &[&format!("{used}Penetration"), "ElementalPenetration"],
                    )?;
                } else if used == "Chaos" {
                    pen = self.c.base(list, c, &["ChaosPenetration"])?;
                }
            } else if is_elemental(t) {
                pen = self.c.base(
                    list,
                    c,
                    &[&format!("{t}Penetration"), "ElementalPenetration"],
                )?;
                taken_inc += self.c.inc(edb, c, &["ElementalDamageTaken"])?;
            } else if t == "Chaos" {
                pen = self.c.base(list, c, &["ChaosPenetration"])?;
            }
        }
        let chance = ModType::Other(Sym::new("CHANCE"));
        let invert_ele = m_max(
            m_min(
                self.c.sum(list, chance, c, &["HitsInvertEleResChance"])?,
                1.0,
            ),
            0.0,
        );
        let invert_chaos = m_max(
            m_min(
                self.c.sum(list, chance, c, &["HitsInvertChaosResChance"])?,
                1.0,
            ),
            0.0,
        );
        let invert = if is_elemental(t) {
            invert_ele
        } else if t == "Chaos" {
            invert_chaos
        } else {
            0.0
        };
        if self.has_flag(i, "projectile") {
            taken_inc += self.c.inc(edb, None, &["ProjectileDamageTaken"])?;
        }
        if self.has_flag(i, "projectile") && self.has_flag(i, "attack") {
            taken_inc += self.c.inc(edb, None, &["ProjectileAttackDamageTaken"])?;
        }
        if self.has_flag(i, "trap") || self.has_flag(i, "mine") {
            taken_inc += self.c.inc(edb, None, &["TrapMineDamageTaken"])?;
        }
        let mut eff = (1.0 + taken_inc / 100.0) * taken_more;
        let use_chance = use_resist(self, t)?;
        let cannot = is_elemental(t) && self.c.flag(list, c, &["CannotElePenIgnore"])?;
        let mut effective_resist = resist;
        if cannot {
            effective_resist = if invert > 0.0 {
                resist - 2.0 * invert * resist
            } else {
                resist
            };
            eff *= 1.0 - effective_resist / 100.0;
        } else if use_chance > 0.0 {
            effective_resist = if invert > 0.0 {
                ((resist - pen) * (1.0 - invert) + (-resist - pen) * invert) * use_chance
            } else {
                (resist - pen) * use_chance
            };
            eff *= 1.0 - effective_resist / 100.0;
        }
        if !(bd && pass == 2) {
            return Ok((eff, None));
        }
        let source = if self.c.flag(
            self.pdb(),
            None,
            &[&format!("Enemy{source_res}ResistEqualToYours")],
        )? {
            format!("Your {source_res} Resistance")
        } else {
            source_res.to_string()
        };
        if eff == 1.0 && source == t && invert <= 0.0 {
            return Ok((eff, None));
        }
        let l = crate::breakdown::eff_mult(crate::breakdown::EffMult {
            damage_type: t,
            resist,
            pen: if cannot { 0.0 } else { pen },
            taken: taken_inc,
            mult: eff,
            taken_more,
            source_res: Some(&source),
            use_res: use_chance > 0.0,
            invert_chance: Some(invert),
            effective_resist: Some(effective_resist),
        });
        Ok((eff, Some(l)))
    }

    /// `breakdown.leech(...)` of a pool
    fn leech_breakdown(&self, o: &Off, pool: &str, hit_rate: f64) -> Result<Vec<String>> {
        let actor = o.actor;
        let g = |k: &str| self.c.out(actor, &format!("{pool}{k}")).unwrap_or(f64::NAN);
        let (instant, instant_rate, instances) = (g("LeechInstant"), g("LeechInstantRate"), g("LeechInstances"));
        let size = self.c.out(actor, pool).unwrap_or(f64::NAN);
        let max = self.c.out(actor, &format!("Max{pool}LeechRate")).unwrap_or(f64::NAN);
        let (dur, proportion) = (g("LeechDuration"), g("LeechInstantProportion"));
        let base = self.misc_data("LeechRateBase");
        let rate_mod = self.c.calc_mod(o.db, None, &[&format!("{pool}LeechRate")])?;
        let mut l = Vec::new();
        let instant_line = |l: &mut Vec<String>| {
            l.push(if proportion != 1.0 {
                sf!(
                    "Instant Leech: %.1f ^8(%d%% x %.1f)",
                    instant,
                    proportion * 100.0,
                    dur * size * base / (1.0 - proportion)
                )
            } else {
                sf!("Instant Leech: %.1f", instant)
            });
        };
        let pool_lines = |l: &mut Vec<String>| {
            l.push(sf!("%d ^8(size of leech destination pool)", size));
            l.push(sf!(
                "x %.2f ^8(base leech rate is %d%% per second)",
                base,
                100.0 * base
            ));
            if rate_mod != 1.0 {
                l.push(sf!("x %.2f ^8(leech rate modifier)", rate_mod));
            }
        };
        if self.sd_truthy(o.skill, "showAverage") {
            if instant > 0.0 {
                instant_line(&mut l);
            }
            if instances > 0.0 {
                l.push("Total leeched per instance:".into());
                pool_lines(&mut l);
                l.push(sf!("x %.2fs ^8(instance duration)", dur));
                l.push(sf!("= %.1f", size * base * rate_mod * dur));
            }
        } else {
            if instant_rate > 0.0 {
                instant_line(&mut l);
                l.push(sf!(
                    "Instant Leech per second: %.1f ^8(%.1f x %.2f)",
                    instant_rate,
                    instant,
                    hit_rate
                ));
            }
            if instances > 0.0 {
                l.push("Rate per instance:".into());
                pool_lines(&mut l);
                let per = size * base * rate_mod;
                l.push(sf!("= %.1f ^8per second", per));
                l.push("Maximum leech rate against one target:".into());
                l.push(sf!("%.1f", per));
                l.push(sf!("x %.2f ^8(average instances)", instances));
                let total = per * instances;
                l.push(sf!("= %.1f ^8per second", total));
                if total <= max {
                    l.push(sf!("Time to reach max: %.1fs", dur));
                }
                l.push(sf!("Leech rate cap: %.1f", max));
                if total > max {
                    l.push(sf!("Time to reach cap: %.1fs", dur / total * max));
                }
            }
        }
        Ok(l)
    }

    /// The crit chance and average damage breakdowns of an attack after
    /// its passes
    fn combine_crit_breakdowns(&mut self, o: &Off) {
        let (i, actor) = (o.skill, o.actor);
        let both = self.has_flag(i, "bothWeaponAttack");
        let w1 = self.has_flag(i, "weapon1Attack");
        let double = self.sd_truthy(i, "doubleHitsWhenDualWielding");
        let out = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        let (crit, pre) = (out(self, "CritChance"), out(self, "PreEffectiveCritChance"));
        let avg = out(self, "AverageDamage");
        let source = if w1 {
            "MainHand.CritChance"
        } else {
            "OffHand.CritChance"
        };
        let hands = [
            ("Main Hand", "MainHand", &o.hands[0]),
            ("Off Hand", "OffHand", &o.hands[1]),
        ];
        let b = self.bd(actor).unwrap();
        for (k, total) in [("PreEffectiveCritChance", pre), ("CritChance", crit)] {
            if both {
                let mut l = vec!["Both weapons:".to_string()];
                for (label, prefix, h) in hands {
                    l.push(format!("{label}:"));
                    match b.get(&format!("{prefix}.{k}")) {
                        Some(x) => l.extend(x.lines.iter().cloned()),
                        None => l.push(sf!("%.2f%%", get_num(h, k).unwrap_or(f64::NAN))),
                    }
                }
                l.push(sf!("= %.2f%% ^8(average)", total));
                b.set(
                    k,
                    Breakdown {
                        lines: l,
                        breakdown_source: Some(source.into()),
                        ..Breakdown::default()
                    },
                );
            } else {
                let hand = format!("{}.{k}", if w1 { "MainHand" } else { "OffHand" });
                match b.map.get_mut(&hand) {
                    Some(x) => {
                        x.breakdown_source = Some(source.into());
                        let x = x.clone();
                        b.set(k, x);
                    }
                    None => {
                        b.map.remove(k);
                    }
                }
            }
        }
        if both {
            let (m, f) = (
                get_num(&o.hands[0], "AverageDamage").unwrap_or(f64::NAN),
                get_num(&o.hands[1], "AverageDamage").unwrap_or(f64::NAN),
            );
            let l = vec![
                "Both weapons:".to_string(),
                if double {
                    sf!("%.1f + %.1f ^8(skill hits with both weapons at once)", m, f)
                } else {
                    sf!("(%.1f + %.1f) / 2 ^8(skill alternates weapons)", m, f)
                },
                sf!("= %.1f", avg),
            ];
            b.set_lines("AverageDamage", l);
        }
    }

    /// The lines of a damage type's breakdown after its `damageTypes` table
    fn hit_damage_lines(&self, o: &Off, min: f64, max: f64, conv_mult: f64, max_hit: bool) -> Vec<String> {
        let po = |k: &str| self.po(o, k).unwrap_or(f64::NAN);
        let go = |k: &str| self.go(o, k).unwrap_or(f64::NAN);
        let mut l = vec![
            "Hit damage:".to_string(),
            sf!("%d to %d ^8(total damage)", min, max),
        ];
        if conv_mult != 1.0 {
            l.push(sf!(
                "x %g ^8(%g%% converted to other damage types)",
                conv_mult,
                (1.0 - conv_mult) * 100.0
            ));
        }
        let (dd, td) = (po("DoubleDamageEffect"), po("TripleDamageEffect"));
        if dd != 0.0 {
            if td != 0.0 {
                l.push(sf!(
                    "x %.2f ^8(1 + %.2f + %.2f multiplier from %.1f%% chance to deal double damage and %d%% chance to deal triple damage)",
                    1.0 + dd + td,
                    dd,
                    td,
                    po("DoubleDamageChance"),
                    po("TripleDamageChance")
                ));
            } else {
                l.push(sf!(
                    "x %.2f ^8(multiplier from %d%% chance to deal double damage)",
                    1.0 + dd,
                    po("DoubleDamageChance")
                ));
            }
        } else if td != 0.0 {
            l.push(sf!(
                "x %.2f ^8(multiplier from %d%% chance to deal triple damage)",
                1.0 + td,
                po("TripleDamageChance")
            ));
        }
        let ruthless = po("RuthlessBlowHitEffect");
        if ruthless != 1.0 {
            l.push(sf!("x %.2f ^8(ruthless blow effect modifier)", ruthless));
        }
        let fist = po("FistOfWarDamageEffect");
        if fist != 1.0 {
            l.push(sf!("x %.2f ^8(fist of war effect modifier)", fist));
        }
        let (warcry, max_warcry) = (go("OffensiveWarcryEffect"), go("MaxOffensiveWarcryEffect"));
        if warcry != 1.0 && !max_hit {
            l.push(sf!(
                "x %.2f ^8(aggregated warcry exerted effect modifier)",
                warcry
            ));
        }
        if max_warcry != 1.0 && max_hit {
            l.push(sf!(
                "x %.2f ^8(aggregated max warcry exerted effect modifier)",
                max_warcry
            ));
        }
        l
    }
}

impl<'a> Perform<'a> {
    /// After the hit passes: the combined hit stats and leech rates, then
    /// ailments, damage over time and the combined DPS
    fn offence_after_hits(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let skill_cfg = self.cfg(i);
        let sc = Some(&skill_cfg);
        if o.is_attack {
            for k in ["PreEffectiveCritChance", "CritChance", "CritMultiplier"] {
                self.combine_stat(o, k, Combine::Average, None);
            }
            for k in [
                "AverageDamage",
                "PvpAverageDamage",
                "TotalDPS",
                "PvpTotalDPS",
                "LifeLeechDuration",
                "LifeLeechInstances",
                "LifeLeechInstant",
                "LifeLeechInstantRate",
                "LifeLeechInstantProportion",
                "EnergyShieldLeechDuration",
                "EnergyShieldLeechInstances",
                "EnergyShieldLeechInstant",
                "EnergyShieldLeechInstantRate",
                "EnergyShieldLeechInstantProportion",
                "ManaLeechDuration",
                "ManaLeechInstances",
                "ManaLeechInstant",
                "ManaLeechInstantRate",
                "ManaLeechInstantProportion",
                "LifeOnHit",
                "LifeOnHitRate",
                "LifeOnKill",
                "EnergyShieldOnHit",
                "EnergyShieldOnHitRate",
                "EnergyShieldOnKill",
                "ManaOnHit",
                "ManaOnHitRate",
                "ManaOnKill",
                "impaleStoredHitAvg",
            ] {
                self.combine_stat(o, k, Combine::Dps, None);
            }
            if self.has_bd(actor) {
                self.combine_crit_breakdowns(o);
            }
        }
        if self.calcs_mode() {
            let (k, unit) = if self.sd_truthy(i, "showAverage") {
                ("AverageDamage", "average damage")
            } else {
                ("TotalDPS", "DPS")
            };
            let v = self
                .c
                .out(actor, k)
                .ok_or_else(|| format!("no {k} to display"))?;
            let shown = crate::pat::format_num_sep(&crate::pat::format_f(v, 1));
            self.c.set_out_val(
                actor,
                "DisplayDamage",
                OutVal::Str(format!("{shown} {unit}").into()),
            );
        }
        let g = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        if self.has_bd(actor) {
            let mut l = vec![];
            let what = if o.is_attack {
                "attack"
            } else if self.sd_truthy(i, "triggered") {
                "trigger"
            } else {
                "cast"
            };
            let avg = if o.is_attack || what == "trigger" {
                "%.1f ^8(average damage)"
            } else {
                "%.1f ^8(average hit)"
            };
            l.push(sf!(avg, g(self, "AverageDamage")));
            l.push(match self.c.out(actor, "HitSpeed") {
                Some(h) => sf!("x %.2f ^8(hit rate)", h),
                None => sf!(&format!("x %.2f ^8({what} rate)"), g(self, "Speed")),
            });
            let dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
            if dps_mult != 1.0 {
                l.push(sf!("x %g ^8(DPS multiplier for this skill)", dps_mult));
            }
            if o.quantity_multiplier > 1.0 {
                l.push(sf!(
                    "x %g ^8(quantity multiplier for this skill)",
                    o.quantity_multiplier
                ));
            }
            l.push(sf!("= %.1f", g(self, "TotalDPS")));
            self.bd(actor).unwrap().set_lines("TotalDPS", l);
        }
        if self.mods(i).minion.is_some() {
            let speed = self.c.out(actor, "HitSpeed").unwrap_or(g(self, "Speed"));
            let v = g(self, "SummonedMinionsPerCast")
                * speed
                * self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
            self.sd_set(i, "summonSpeed", v);
        }
        let base = self.misc_data("LeechRateBase");
        for pool in ["Life", "EnergyShield", "Mana"] {
            let rate =
                g(self, pool) * base * self.c.calc_mod(list, sc, &[&format!("{pool}LeechRate")])?;
            self.c
                .set_out(actor, &format!("{pool}LeechInstanceRate"), rate);
            self.c.set_out(
                actor,
                &format!("{pool}LeechRate"),
                g(self, &format!("{pool}LeechInstances")) * rate,
            );
            self.c.set_out(actor, &format!("{pool}LeechPerHit"), rate);
        }
        if self.c.flag(list, None, &["ImmortalAmbition"])? {
            let v = g(self, "EnergyShieldLeechRate") + g(self, "LifeLeechRate");
            self.c.set_out(actor, "EnergyShieldLeechRate", v);
            let v = g(self, "EnergyShieldLeechPerHit") + g(self, "LifeLeechPerHit");
            self.c.set_out(actor, "EnergyShieldLeechPerHit", v);
            self.c.set_out(actor, "LifeLeechRate", 0.0);
            self.c.set_out(actor, "LifeLeechPerHit", 0.0);
        }
        if self
            .c
            .flag(list, None, &["UnaffectedByNonInstantLifeLeech"])?
        {
            self.c.set_out(actor, "LifeLeechRate", 0.0);
            self.c.set_out(actor, "LifeLeechPerHit", 0.0);
            self.c.set_out(actor, "LifeLeechInstances", 0.0);
        }
        for pool in ["Life", "EnergyShield", "Mana"] {
            let max = g(self, &format!("Max{pool}LeechRate"));
            let recovery = g(self, &format!("{pool}RecoveryRateMod"));
            let rate = g(self, &format!("{pool}LeechInstantRate"))
                + m_min(g(self, &format!("{pool}LeechRate")), max) * recovery;
            self.c.set_out(actor, &format!("{pool}LeechRate"), rate);
            let per_hit = g(self, &format!("{pool}LeechInstant"))
                + m_min(g(self, &format!("{pool}LeechPerHit")), max)
                    * g(self, &format!("{pool}LeechDuration"))
                    * recovery;
            self.c
                .set_out(actor, &format!("{pool}LeechPerHit"), per_hit);
        }
        self.set_flag(i, "leechLife", g(self, "LifeLeechRate") > 0.0);
        self.set_flag(i, "leechES", g(self, "EnergyShieldLeechRate") > 0.0);
        self.set_flag(i, "leechMana", g(self, "ManaLeechRate") > 0.0);
        let show_average = self.sd_truthy(i, "showAverage");
        for pool in ["Life", "EnergyShield", "Mana"] {
            if show_average {
                let v = g(self, &format!("{pool}LeechPerHit")) + g(self, &format!("{pool}OnHit"));
                self.c.set_out(actor, &format!("{pool}LeechGainPerHit"), v);
            } else {
                let v = g(self, &format!("{pool}LeechRate")) + g(self, &format!("{pool}OnHitRate"));
                self.c.set_out(actor, &format!("{pool}LeechGainRate"), v);
            }
        }
        if self.has_bd(actor) {
            let speed = self
                .c
                .out(actor, "HitSpeed")
                .unwrap_or(g(self, "Speed"));
            let hit_rate = g(self, "HitChance") / 100.0
                * speed
                * self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
            for (pool, flag) in [
                ("Life", "leechLife"),
                ("EnergyShield", "leechES"),
                ("Mana", "leechMana"),
            ] {
                if self.has_flag(i, flag) {
                    let l = self.leech_breakdown(o, pool, hit_rate)?;
                    self.bd(actor).unwrap().set_lines(format!("{pool}Leech"), l);
                }
            }
        }
        self.offence_ailments(o)
    }
}
