//! The damage passes of `calcs.offence` (`Modules/CalcOffence.lua`): how
//! often the skill hits (speed, accuracy, block), then its hit damage
//! (exerts, crits, resistances, leech, gains on hit, DPS).
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::OutVal;
use crate::calc::{PLAYER, floor, m_max, m_min, number, round};
use crate::calc_defence::{DMG_TYPES, armour_reduction_f, hit_chance, is_elemental};
use crate::calc_offence::{Off, OutMap, Pass, dmg_index, get_num, set_num};
use crate::calc_perform::{Perform, new_mod};
use crate::mods::{Cfg, Mod, ModType, ModValue, Sym};

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

    /// The current pass's breakdown (`breakdown.MainHand`, ... or the
    /// actor's)
    pub(crate) fn pbd(&mut self, o: &Off) -> Option<&mut crate::breakdown::Bd> {
        let weapon = o.passes[o.pass].weapon;
        let bd = self.bd(o.actor)?;
        match weapon {
            Some(0) => bd.table_mut("MainHand"),
            Some(_) => bd.table_mut("OffHand"),
            None => Some(bd),
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
        let mut bd_lines: Vec<String> = Vec::new();
        let (mh, oh) = (&o.hands[0], &o.hands[1]);
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
                        use crate::sf;
                        let c = chance.expect("a chance stat");
                        let main = num(mh, c).unwrap_or(f64::NAN)
                            * num(mh, "HitChance").unwrap_or(f64::NAN);
                        let off = num(oh, c).unwrap_or(f64::NAN)
                            * num(oh, "HitChance").unwrap_or(f64::NAN);
                        if mode == Combine::Chance {
                            let (mc, oc, main_label, off_label) =
                                if self.sd_truthy(o.skill, "doubleHitsWhenDualWielding") {
                                    (
                                        main / 10000.0,
                                        off / 10000.0,
                                        "x %.3f ^8(chance of main hand)",
                                        "x %.3f ^8(chance of  off hand)",
                                    )
                                } else {
                                    (
                                        main / (main + off),
                                        off / (main + off),
                                        "x %.3f ^8(portion of instances created by main hand)",
                                        "x %.3f ^8(portion of instances created by off hand)",
                                    )
                                };
                            let v = a * mc + b * oc;
                            bd_lines.extend([
                                "Contribution from Main Hand:".to_string(),
                                sf!("%.1f", a),
                                sf!(main_label, mc),
                                sf!("= %.1f", a * mc),
                                "Contribution from Off Hand:".to_string(),
                                sf!("%.1f", b),
                                sf!(off_label, oc),
                                sf!("= %.1f", b * oc),
                                "Total:".to_string(),
                                sf!("%.1f + %.1f", a * mc, b * oc),
                                sf!("= %.1f", v),
                            ]);
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
                            bd_lines.push(String::new());
                            bd_lines.push(sf!(
                                "%.2f%% of ailment stacks use maximum damage",
                                stacks * 100.0
                            ));
                            bd_lines.push(sf!(
                                "Max Damage comes from %s",
                                if a >= b { "Main Hand" } else { "Off Hand" }
                            ));
                            bd_lines.push(sf!("= %.1f", max * stacks));
                            if stacks < 1.0 {
                                bd_lines.push(sf!(
                                    "%.2f%% of ailment stacks use non-maximum damage",
                                    (1.0 - stacks) * 100.0
                                ));
                                bd_lines.push(sf!("= %.1f", min * (1.0 - stacks)));
                            }
                            bd_lines.push(String::new());
                            bd_lines.push("Total:".into());
                            if stacks < 1.0 {
                                bd_lines.push(sf!(
                                    "%.1f + %.1f",
                                    max * stacks,
                                    min * (1.0 - stacks)
                                ));
                            }
                            bd_lines.push(sf!("= %.1f", v));
                            Some(OutVal::Num(v))
                        }
                    }
                    (a, _) => {
                        if mode == Combine::ChanceAilment {
                            bd_lines.push(crate::sf!(
                                "All ailment stacks comes from %s",
                                if a.is_some() { "Main Hand" } else { "Off Hand" }
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
        if !bd_lines.is_empty()
            && let Some(bd) = self.bd(o.actor)
        {
            let t = bd.table(stat);
            for l in bd_lines {
                t.push(l);
            }
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
                if let Some(bd) = self.bd(actor) {
                    bd.set(
                        if n == 0 { "MainHand" } else { "OffHand" },
                        crate::breakdown::Bd::new(),
                    );
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
        let mut trauma_bd = crate::breakdown::Bd::new();
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
            if let Some(bd) = self.pbd(o) {
                use crate::breakdown::{Bd, Chain, multi_chain};
                let mut b = Bd::new();
                multi_chain(
                    &mut b,
                    Chain {
                        base: Some(crate::factor!("%g ^8(base)", base)),
                        factors: vec![
                            crate::factor!("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
                            crate::factor!("%.2f ^8(more/less)", more),
                        ],
                        total: Some(crate::sf!("= %g", accuracy)),
                        ..Default::default()
                    },
                );
                if accuracy != accuracy_vs {
                    b.push("");
                    multi_chain(
                        &mut b,
                        Chain {
                            label: Some("Effective Accuracy vs Enemy".into()),
                            base: Some(crate::factor!("%g ^8(base)", base_vs)),
                            factors: vec![
                                crate::factor!("%.2f ^8(increased/reduced)", 1.0 + inc_vs / 100.0),
                                crate::factor!("%.2f ^8(more/less)", more_vs),
                            ],
                            total: Some(crate::sf!("= %g", accuracy_vs)),
                            ..Default::default()
                        },
                    );
                }
                bd.set("Accuracy", b);
            }
            if off_hand_acc && label == "Main Hand" {
                stored_accuracy = Some((accuracy, accuracy_vs));
            } else if off_hand_acc
                && label == "Off Hand"
                && let Some((a, v)) = stored_accuracy
            {
                accuracy = a;
                accuracy_vs = v;
                if let Some(bd) = self.pbd(o) {
                    bd.set(
                        "Accuracy",
                        crate::breakdown::Bd::of([format!(
                            "Using Main Hand Accuracy due to Mastery: {}",
                            crate::pat::num(a)
                        )]),
                    );
                }
            }
            self.set_po(o, "Accuracy", accuracy);
            // The weapon's `skillStats` is the pass's output as it grows
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            let acc_hit = if !o.is_attack
                || self.c.flag(list, c, &["CannotBeEvaded"])?
                || self.sd_truthy(i, "cannotBeEvaded")
                || (effective && self.c.flag(edb, None, &["CannotEvade"])?)
            {
                100.0
            } else {
                let evasion = m_max(round(self.c.val(edb, "Evasion", None)?, 0.0), 0.0);
                let h =
                    hit_chance(evasion, accuracy_vs) * self.c.calc_mod(list, c, &["HitChance"])?;
                let lvl = self.env.enemy_level;
                let over = self.input.config.contains_key("enemyLevel");
                if let Some(bd) = self.pbd(o) {
                    let n = crate::pat::num;
                    bd.set(
                        "AccuracyHitChance",
                        crate::breakdown::Bd::of([
                            format!(
                                "Enemy level: {}{}",
                                n(lvl),
                                if over {
                                    " ^8(overridden from the Configuration tab"
                                } else {
                                    " ^8(can be overridden in the Configuration tab)"
                                }
                            ),
                            format!("Enemy evasion: {}", n(evasion)),
                            format!("Approximate hit chance: {}%", n(h)),
                        ]),
                    );
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
            let hc = acc_hit * (1.0 - block / 100.0);
            self.set_po(o, "HitChance", hc);
            if block > 0.0
                && let Some(bd) = self.pbd(o)
            {
                let n = crate::pat::num;
                let mut b = crate::breakdown::Bd::new();
                if acc_hit < 100.0 {
                    b.push(format!("Accuracy Hit Chance: {}%", n(acc_hit)));
                }
                b.push(format!("Enemy Block Chance: {}%", n(block)));
                b.push(format!("Approximate hit chance: {}%", n(hc)));
                bd.set("HitChance", b);
            }
            if block > 0.0 && !o.is_attack {
                self.c
                    .set_out_val(actor, "enemyHasSpellBlock", OutVal::Bool(true));
            }
            let cfg = self.pcfg(o);
            let c = Some(&cfg);
            let cond = format!("{}AccRatingHigherThanMaxLife", label.replace(' ', ""));
            let life = self.c.out(PLAYER, "Life").unwrap_or(f64::NAN);
            self.c.set_cond(list, &cond, accuracy > life);
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
                    let trauma_shown = trauma;
                    let mut invalid = false;
                    if trauma < 0.0 || inc_per * trauma > inc_cap {
                        trauma = per_attack * (hit / 100.0) * cap / repeats * duration;
                        invalid = true;
                    }
                    if self.pbd(o).is_some() {
                        use crate::breakdown::{Bd, Chain, multi_chain};
                        use crate::{factor, sf};
                        let mut b = Bd::new();
                        if inc_per == 0.0 {
                            multi_chain(
                                &mut b,
                                Chain {
                                    label: Some("Trauma".into()),
                                    base: Some(factor!(
                                        "%.2f ^8(attack speed)",
                                        before_inc * (1.0 + inc / 100.0)
                                    )),
                                    factors: vec![
                                        factor!("%.2f ^8(trauma per attack)", per_attack),
                                        factor!("%.2f ^8(chance to hit)", hit / 100.0),
                                        factor!("%.2f ^8(duration)", duration),
                                    ],
                                    no_total: true,
                                    ..Default::default()
                                },
                            );
                            if repeats != 1.0 {
                                b.push(sf!("/ %.2f ^8(repeats)", repeats));
                            }
                        } else {
                            multi_chain(
                                &mut b,
                                Chain {
                                    label: Some(
                                        "Attack Speed before increased Attack Speed".into(),
                                    ),
                                    base: Some(factor!("%.2f ^8(base)", 1.0 / base_time)),
                                    factors: vec![
                                        factor!("%.2f ^8(more/less)", more),
                                        factor!("%.2f ^8(action speed modifier)", action),
                                    ],
                                    total: Some(sf!("= %.2f ^8attacks per second", before_inc)),
                                    ..Default::default()
                                },
                            );
                            b.push("");
                            multi_chain(
                                &mut b,
                                Chain {
                                    label: Some(
                                        "Trauma per second before increased Attack Speed".into(),
                                    ),
                                    base: Some(factor!("%.2f ^8(base)", before_inc)),
                                    factors: vec![
                                        factor!("%.2f ^8(trauma per attack)", per_attack),
                                        factor!("%.2f ^8(chance to hit)", hit / 100.0),
                                    ],
                                    no_total: true,
                                    ..Default::default()
                                },
                            );
                            if repeats != 1.0 {
                                b.push(sf!("/ %.2f ^8(repeats)", repeats));
                            }
                            b.push(sf!("= %.2f ^8trauma per second", rate_before));
                            b.push("");
                            b.push("Trauma");
                            b.push(sf!("%.2f ^8(base)", rate_before));
                            b.push(sf!("x %.2f ^8(increased/reduced)", 1.0 + inc / 100.0));
                            b.push(sf!(
                                "/ %.4f ^8(1 / duration - trauma per second * increased attack speed per trauma / 100)",
                                1.0 / duration - rate_before * inc_per / 100.0
                            ));
                        }
                        let neg = if invalid { "^xDD0022" } else { "" };
                        b.push(sf!(&format!("= {neg}%d ^8trauma"), trauma_shown));
                        if invalid {
                            b.push("");
                            b.push("Attack Speed exceeds cap; Recalculating");
                            multi_chain(
                                &mut b,
                                Chain {
                                    base: Some(factor!("%.2f ^8(base)", cap)),
                                    factors: vec![
                                        factor!("%.2f ^8(trauma per attack)", per_attack),
                                        factor!("%.2f ^8(chance to hit)", hit / 100.0),
                                        factor!("%.2f ^8(duration)", duration),
                                    ],
                                    no_total: true,
                                    ..Default::default()
                                },
                            );
                            if repeats != 1.0 {
                                b.push(sf!("/ %.2f ^8(repeats)", repeats));
                            }
                            b.push(sf!("= %d ^8trauma", trauma));
                        }
                        trauma_bd = b;
                    }
                    if self.has_flag(i, "bothWeaponAttack") {
                        trauma /= 2.0;
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
                if self.pbd(o).is_some() {
                    use crate::breakdown::{Bd, Chain, multi_chain};
                    use crate::{factor, sf};
                    let cast_rate = self.po(o, "CastRate").unwrap_or(f64::NAN);
                    let asm = if self.has_flag(i, "totem") {
                        self.po(o, "TotemActionSpeed").unwrap_or(f64::NAN)
                    } else if self.has_flag(i, "selfCast") {
                        action
                    } else {
                        1.0
                    };
                    let cd = self.po(o, "Cooldown");
                    let attack_time = self.c.calc_mod(list, sc, &["SkillAttackTime"])?;
                    let mut b = Bd::new();
                    multi_chain(
                        &mut b,
                        Chain {
                            base: Some(factor!("%.2f ^8(base)", 1.0 / base_time)),
                            factors: vec![
                                factor!("%.2f ^8(increased/reduced)", 1.0 + inc / 100.0),
                                factor!("%.2f ^8(more/less)", more),
                                factor!("%.2f ^8(action speed modifier)", asm),
                            ],
                            total: Some(sf!("= %.2f ^8casts per second", cast_rate)),
                            ..Default::default()
                        },
                    );
                    if let Some(cd) = cd
                        && 1.0 / cd < cast_rate
                    {
                        b.push("\n");
                        b.push(sf!("1 / %.2f ^8(skill cooldown)", cd));
                        if repeats > 1.0 {
                            b.push(sf!("x %d ^8(repeat count)", repeats));
                        }
                        b.push(sf!("= %.2f ^8(casts per second)", repeats / cd));
                        b.push("\n");
                        b.push(sf!("= %.2f ^8(lower of cast rates)", speed));
                    }
                    let bd = self.pbd(o).unwrap();
                    bd.set("Speed", b);
                    if attack_time > 0.0 {
                        let mut t = Bd::new();
                        multi_chain(
                            &mut t,
                            Chain {
                                base: Some(factor!("%.2f ^8(base)", 1.0 / (speed * attack_time))),
                                factors: vec![factor!("%.2f ^8(total modifier)", attack_time)],
                                total: Some(sf!("= %.2f ^8seconds per attack", time)),
                                ..Default::default()
                            },
                        );
                        bd.set("Time", t);
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
        if let Some(bd) = self.bd(actor) {
            bd.set("SustainableTrauma", trauma_bd);
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
            if self.c.flag(list, None, &["UseOffhandAttackSpeed"])?
                && !self.has_flag(i, "forceMainHand")
            {
                for k in ["Speed", "Time"] {
                    match get_num(&o.hands[1], k) {
                        Some(v) => self.c.set_out(actor, k, v),
                        None => self.c.clear_out(actor, k),
                    }
                }
                let sp = self.c.out(actor, "Speed").unwrap_or(f64::NAN);
                if let Some(bd) = self.bd(actor) {
                    bd.set(
                        "Speed",
                        crate::breakdown::Bd::of([
                            "Use Offhand Weapon Attack Speed:".to_string(),
                            crate::sf!("= %.2f", sp),
                        ]),
                    );
                }
            } else if self.has_flag(i, "bothWeaponAttack") {
                let sp = self.c.out(actor, "Speed").unwrap_or(f64::NAN);
                let (m, oh) = (
                    get_num(&o.hands[0], "Speed").unwrap_or(f64::NAN),
                    get_num(&o.hands[1], "Speed").unwrap_or(f64::NAN),
                );
                if let Some(bd) = self.bd(actor) {
                    bd.set(
                        "Speed",
                        crate::breakdown::Bd::of([
                            "Both weapons:".to_string(),
                            crate::sf!("2 / (1 / %.2f + 1 / %.2f)", m, oh),
                            crate::sf!("= %.2f", sp),
                        ]),
                    );
                }
            }
            // the global output
            o.pass = usize::MAX;
            self.hit_time(o, true)?;
        }
        if self.bd(actor).is_some() {
            use crate::sf;
            let on_death = self.sd_truthy(i, "triggeredOnDeath");
            let out = |p: &Self, k: &str| p.c.out(actor, k);
            let ht = out(self, "HitTime").unwrap_or(f64::NAN);
            let hs = out(self, "HitSpeed").unwrap_or(f64::NAN);
            let cd = out(self, "Cooldown");
            let time = out(self, "Time");
            if self.sd(i, "hitTimeOverride").is_some_and(ModValue::truthy) && !on_death {
                self.bd(actor).unwrap().set(
                    "HitSpeed",
                    crate::breakdown::Bd::of([
                        sf!("1 / %.2f ^8(hit time override)", ht),
                        sf!("= %.2f", hs),
                    ]),
                );
            } else if let (Some(m), Some(t)) = (self.sd_num(i, "hitTimeMultiplier"), time)
                && !on_death
            {
                let mut h = crate::breakdown::Bd::new();
                if m.floor() != m {
                    h.push(sf!(
                        "^x5CF0BBNOTE: First stage has a %.2fx channel time multiplier",
                        m - m.floor()
                    ));
                }
                if o.is_attack {
                    h.push(sf!("%.2f ^8(attack time)", t));
                } else {
                    h.push(sf!("%.2f ^8(cast time)", t));
                }
                h.push(sf!("x %.2f ^8(channel time multiplier)", m));
                h.push(sf!("= %.2f", ht));
                let triggered = self.sd_truthy(i, "triggered");
                let mut sp = crate::breakdown::Bd::new();
                match cd {
                    Some(cd) if triggered => {
                        sp.push(sf!("1 / min(%.2f, %.2f) ^8min(hit time, cooldown)", ht, cd))
                    }
                    Some(cd) => sp.push(sf!("1 / (%.2f + %.2f) ^8(hit time + cooldown)", ht, cd)),
                    None => sp.push(sf!("1 / %.2f ^8(hit time)", ht)),
                }
                sp.push(sf!("= %.2f", hs));
                let bd = self.bd(actor).unwrap();
                bd.set("HitTime", h);
                bd.set("HitSpeed", sp);
            }
        }
        // Grab quantity multiplier
        let q = m_max(self.c.base(list, sc, &["QuantityMultiplier"])?, 1.0);
        if q > 1.0 {
            self.c.set_out(actor, "QuantityMultiplier", q);
        }
        o.quantity_multiplier = q;
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
                    self.set_go(o, "IntimidatingHitEffect", hit);
                    self.set_go(o, "IntimidatingMaxHitEffect", 1.0 + avg);
                    mul(self, o, "TheoreticalOffensiveWarcryEffect", hit);
                    mul(self, o, "TheoreticalMaxOffensiveWarcryEffect", 1.0 + avg);
                }
                "Rallying Cry" => {
                    let avg = m_min(self.c.base(pdb, c, &["Multiplier:NearbyAlly"])?, 5.0)
                        * (self
                            .c
                            .base(pdb, None, &["RallyingExertMoreDamagePerAlly"])?
                            / 100.0);
                    self.set_go(o, "RallyingAvgDmg", avg);
                    let hit = 1.0 + avg * up / 100.0;
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
        if self.bd(actor).is_some() {
            use crate::sf;
            let mut b = crate::breakdown::Bd::of(["Maximum of:"]);
            for (_, _, _, r, _, _) in EXERTING_CRIES {
                if let Some(v) = self.go(o, r) {
                    let name = r.strip_suffix("UpTimeRatio").unwrap_or(r);
                    b.push(sf!("%d%% ^8(%s Cry Uptime)", v, name));
                }
            }
            b.push(sf!("= %d%%", ratio));
            self.bd(actor).unwrap().set("ExertedAttackUptimeRatio", b);
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
                    self.set_go(
                        o,
                        "SoulGainPreventionDuration",
                        m_max((d * m * tick).ceil(), 1.0) / tick,
                    );
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
                    && let Some(bd) = self.pbd(o)
                {
                    bd.set(
                        "CritChance",
                        crate::breakdown::Bd::of([
                            "100%".to_string(),
                            crate::sf!("/ %d ^8(number of repeats)", repeats),
                            crate::sf!("= %.2f%% average critical strike chance", v),
                        ]),
                    );
                }
            }
        } else {
            let mut crit_override = self.c.override_num(list, c, &["CritChance"])?;
            if self.c.flag(list, c, &["MainHandCritIsEqualToParent"])? {
                return Err("not ported: minions".into());
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
            if from_main {
                base_crit = self.weapon_num(actor, 0, "CritChance").unwrap_or(f64::NAN);
            } else if self
                .c
                .flag(list, c, &["AttackCritIsEqualToParentMainHand"])?
            {
                return Err("not ported: minions".into());
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
                    more = self.c.more(list, c, &["CritChance"])?;
                }
                let mut crit = (base_crit + base) * (1.0 + inc / 100.0) * more;
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
                let pre_lucky = crit;
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
                let bifurcate = self.c.flag(list, c, &["BifurcateCrit"])?;
                if effective && bifurcate {
                    crit = (1.0 - (1.0 - crit / 100.0).powf(2.0)) * 100.0;
                }
                let pre_skill_use = crit;
                let mut pre_hit_check = crit;
                let every3 = self.c.flag(list, sc, &["Every3UseCrit"])?;
                let every5 = self.c.flag(list, sc, &["Every5UseCrit"])?;
                let acc_hit = self.po(o, "AccuracyHitChance").unwrap_or(f64::NAN);
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
                if crit != base_crit && self.pbd(o).is_some() {
                    use crate::sf;
                    let unexciting = self.c.flag(list, sc, &["Unexciting"])?;
                    let from = if from_main { " from main weapon" } else { "" };
                    let mut b = crate::breakdown::Bd::new();
                    if base != 0.0 {
                        b.push(sf!("(%g + %g) ^8(base%s)", base_crit, base, from));
                    } else {
                        b.push(sf!("%g ^8(base%s)", base_crit + base, from));
                    }
                    if inc != 0.0 {
                        b.push(format!(
                            "{} ^8(increased/reduced)",
                            sf!("x %.2f", 1.0 + inc / 100.0)
                        ));
                    }
                    if more != 1.0 {
                        b.push(format!("{} ^8(more/less)", sf!("x %.2f", more)));
                    }
                    b.push(sf!("= %.2f%% ^8(crit chance)", pre_effective));
                    if pre_cap > 100.0 {
                        let over = pre_cap - 100.0;
                        b.push(sf!(
                            "Crit is overcapped by %.2f%% (%d%% increased Critical Strike Chance)",
                            over,
                            over / more / (base_crit + base) * 100.0
                        ));
                    }
                    if effective {
                        if rolls != 0.0 {
                            if unexciting {
                                b.push("Crit Chance is Unexciting:");
                                b.push(sf!(
                                    "(3 x %.4f^ 2) - (2 x %.4f^ 3)",
                                    pre_lucky / 100.0,
                                    pre_lucky / 100.0
                                ));
                            } else {
                                b.push("Crit Chance is Lucky:");
                                b.push(sf!("1 - (1 - %.4f)^ %d", pre_lucky / 100.0, rolls + 1.0));
                            }
                            b.push(sf!("= %.2f%%", pre_bifurcate));
                        }
                        if bifurcate {
                            b.push("Critical Strike Bifurcates:");
                            b.push(sf!(
                                "1 - (1 - %.4f) x (1 - %.4f)",
                                pre_bifurcate / 100.0,
                                pre_bifurcate / 100.0
                            ));
                            b.push(sf!("= %.2f%%", pre_skill_use));
                        }
                        if every3 {
                            b.push(sf!(
                                "+ %.2f%% ^8(crit every 3rd use)",
                                (2.0 * pre_skill_use + 100.0) / 3.0 - pre_skill_use
                            ));
                        }
                        if every5 {
                            b.push(sf!(
                                "+ %.2f%% ^8(crit every 5th use)",
                                (4.0 * pre_skill_use + 100.0) / 5.0 - pre_skill_use
                            ));
                        }
                        if every3 || every5 {
                            b.push(sf!("= %.2f%%", pre_hit_check));
                        }
                    }
                    if effective && acc_hit < 100.0 {
                        b.push("Crit confirmation roll:");
                        b.push(sf!("%.2f%%", pre_hit_check));
                        b.push(sf!("x %.2f ^8(chance to hit)", acc_hit / 100.0));
                        b.push(sf!("= %.2f%%", crit));
                    }
                    self.pbd(o).unwrap().set("CritChance", b);
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
                    let self_base = self.c.base(edb, None, &["SelfCritMultiplier"])?;
                    extra += self_base / 100.0;
                    extra = round(extra * enemy_inc, 2.0);
                    if enemy_inc != 1.0 && self.pbd(o).is_some() {
                        use crate::sf;
                        let own = self.c.base(list, c, &["CritMultiplier"])?;
                        self.pbd(o).unwrap().set(
                            "CritMultiplier",
                            crate::breakdown::Bd::of([
                                sf!(
                                    "%d%% ^8(additional extra damage)",
                                    (self_base + own) / 100.0
                                ),
                                sf!(
                                    "x %.2f ^8(increased/reduced extra crit damage taken by enemy)",
                                    enemy_inc
                                ),
                                sf!("= %d%% ^8(extra crit damage)", extra * 100.0),
                            ]),
                        );
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
                    if let Some(bd) = self.pbd(o) {
                        use crate::sf;
                        let mut b = crate::breakdown::Bd::of([
                            sf!("%.2f%% ^8(pre-bifurcate crit chance)", pre),
                            sf!("x %.2f%%", pre),
                        ]);
                        if use_chance < 1.0 {
                            b.push(sf!(
                                "x %.2f%% ^8(uses that can bifurcate)",
                                use_chance * 100.0
                            ));
                        }
                        b.push(sf!(
                            "= %.2f%% ^8(chance both crit rolls succeed)",
                            multi_chance
                        ));
                        b.push(sf!("/ %.2f%% ^8(effective crit chance)", effective_crit));
                        b.push(sf!("= %.2f ^8(crit Bifurcates effect)", 1.0 + conditional));
                        bd.set("CritBifurcates", b);
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
                && let Some(bd) = self.pbd(o)
            {
                use crate::sf;
                bd.set(
                    "CritEffect",
                    crate::breakdown::Bd::of([
                        sf!("(1 - %.4f) ^8(portion of damage from non-crits)", pct),
                        sf!(
                            "+ [ (%.4f x %g) ^8(portion of damage from crits)",
                            pct,
                            multi
                        ),
                        sf!("= %.3f", effect),
                    ]),
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn calc_damage(
        &self,
        o: &Off,
        cfg: &Cfg,
        conv: &crate::calc_offence::ConversionTable,
        t: &str,
        type_flags: u32,
        mut bd: Option<&mut crate::breakdown::Bd>,
        conv_dst: Option<&str>,
    ) -> Result<(f64, f64)> {
        use crate::breakdown::BdVal;
        let n = crate::pat::num;
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
                    self.calc_damage(o, cfg, conv, other, flags, bd.as_deref_mut(), Some(t))?;
                add_min += min * conv_mult;
                add_max += max * conv_mult;
            }
        }
        if add_min != 0.0 && add_max != 0.0 {
            add_min = round(add_min, 0.0);
            add_max = round(add_max, 0.0);
        }
        let base_min = self.po(o, &format!("{t}MinBase")).unwrap_or(f64::NAN);
        let base_max = self.po(o, &format!("{t}MaxBase")).unwrap_or(f64::NAN);
        let has_add = add_min != 0.0 || add_max != 0.0;
        let conv_src = || -> BdVal {
            if has_add {
                format!("{} to {}", n(add_min), n(add_max)).into()
            } else {
                false.into()
            }
        };
        let dst = |check: bool| -> Option<(BdVal, BdVal)> {
            let d = conv_dst?;
            let di = dmg_index(d);
            let (c, g) = (conv[ti].conversion[di], conv[ti].gain[di]);
            let f = |v: f64, s: String| -> BdVal {
                if !check || v > 0.0 {
                    s.into()
                } else {
                    false.into()
                }
            };
            Some((
                f(c, crate::sf!("%d%% to %s", c * 100.0, d)),
                f(g, crate::sf!("%d%% gained as %s", g * 100.0, d)),
            ))
        };
        if base_min == 0.0 && base_max == 0.0 {
            if let Some(b) = bd.as_deref_mut()
                && has_add
            {
                let mut r = crate::breakdown::Bd::new();
                r.set("source", t);
                r.set("convSrc", conv_src());
                r.set("total", format!("{} to {}", n(add_min), n(add_max)));
                if let Some((c, g)) = dst(false) {
                    r.set("convDst", c);
                    r.set("gainDst", g);
                }
                b.table("damageTypes").push(r);
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
        if let Some(b) = bd {
            let opt = |cnd: bool, v: f64| -> BdVal {
                if cnd {
                    format!("x {}", n(v)).into()
                } else {
                    false.into()
                }
            };
            let mut r = crate::breakdown::Bd::new();
            r.set("source", t);
            r.set("base", format!("{} to {}", n(base_min), n(base_max)));
            r.set("inc", opt(inc != 1.0, inc));
            r.set("more", opt(more != 1.0, more));
            r.set("convSrc", conv_src());
            r.set(
                "total",
                format!(
                    "{} to {}",
                    n(round(base_min * inc * more, 0.0) + add_min),
                    n(round(base_max * inc * more, 0.0) + add_max)
                ),
            );
            if let Some((cv, g)) = dst(true) {
                r.set("convDst", cv);
                r.set("gainDst", g);
            }
            b.table("damageTypes").push(r);
        }
        Ok((
            round(
                ((base_min * inc * more) * generic_min + add_min) * more_min * inc_min,
                0.0,
            ),
            round(
                ((base_max * inc * more) * generic_max + add_max) * more_max * inc_max,
                0.0,
            ),
        ))
    }

    /// The second passes: damage
    fn hit_passes(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor, edb, db) = (o.skill, o.list, o.actor, o.edb, o.db);
        let effective = self.env.mode.effective;
        let calcs_mode = self.input.breakdown;
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
                if self.pbd(o).is_some() {
                    use crate::sf;
                    let (smin, smax) = (
                        self.src_num(o, &min_n).unwrap_or(0.0),
                        self.src_num(o, &max_n).unwrap_or(0.0),
                    );
                    let from = if self.src_val(o, "type").is_some() {
                        "weapon"
                    } else {
                        "skill"
                    };
                    let mut b = crate::breakdown::Bd::new();
                    b.set("damageTypes", crate::breakdown::Bd::new());
                    if base_min != 0.0 && base_max != 0.0 {
                        b.push("Base damage:");
                        let mut plus = "";
                        if smin != 0.0 || smax != 0.0 {
                            b.push(sf!("%d to %d ^8(base damage from %s)", smin, smax, from));
                            if base_mult != 1.0 {
                                b.push(sf!("x %.2f ^8(base damage multiplier)", base_mult));
                            }
                            plus = "+ ";
                        }
                        if added_min != 0.0 || added_max != 0.0 {
                            b.push(sf!(
                                "%s%d to %d ^8(added damage)",
                                plus,
                                added_min,
                                added_max
                            ));
                            if effectiveness != 1.0 {
                                b.push(sf!("x %.2f ^8(damage effectiveness)", effectiveness));
                            }
                            if added_mult != 1.0 {
                                b.push(sf!("x %.2f ^8(added damage multiplier)", added_mult));
                            }
                        }
                        b.push(sf!("= %.1f to %.1f", base_min, base_max));
                    }
                    self.pbd(o).unwrap().set(t, b);
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
        let mut total_crit_avg = 0.0;
        let (mut total_crit_min, mut total_crit_max) = (0.0, 0.0);
        let ghost_reaver = self.c.flag(list, None, &["GhostReaver"])?;
        let mut ghost_reaver_leech = 0.0;
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
                if self.has_flag(i, "hit") && o.can_deal[ti] {
                    let mut tbl = if pass == 2 {
                        self.pbd(o).and_then(|b| b.table_mut(t)).map(std::mem::take)
                    } else {
                        None
                    };
                    let (mn, mx) = self.calc_damage(o, &cfg, &conv, t, 0, tbl.as_mut(), None)?;
                    hit_min = mn;
                    hit_max = mx;
                    let conv_mult = conv[ti].mult;
                    if let Some(mut b) = tbl {
                        use crate::sf;
                        let po = |k: &str| self.po(o, k).unwrap_or(f64::NAN);
                        let g = |k: &str| self.go(o, k).unwrap_or(f64::NAN);
                        let max_hit = self.c.flag(list, None, &["Condition:WarcryMaxHit"])?;
                        b.push("Hit damage:");
                        b.push(sf!("%d to %d ^8(total damage)", hit_min, hit_max));
                        if conv_mult != 1.0 {
                            b.push(sf!(
                                "x %g ^8(%g%% converted to other damage types)",
                                conv_mult,
                                (1.0 - conv_mult) * 100.0
                            ));
                        }
                        let (dd, td) = (po("DoubleDamageEffect"), po("TripleDamageEffect"));
                        if dd != 0.0 {
                            if td != 0.0 {
                                b.push(sf!(
                                    "x %.2f ^8(1 + %.2f + %.2f multiplier from %.1f%% chance to deal double damage and %d%% chance to deal triple damage)",
                                    1.0 + dd + td,
                                    dd,
                                    td,
                                    po("DoubleDamageChance"),
                                    po("TripleDamageChance")
                                ));
                            } else {
                                b.push(sf!(
                                    "x %.2f ^8(multiplier from %d%% chance to deal double damage)",
                                    1.0 + dd,
                                    po("DoubleDamageChance")
                                ));
                            }
                        } else if td != 0.0 {
                            b.push(sf!(
                                "x %.2f ^8(multiplier from %d%% chance to deal triple damage)",
                                1.0 + td,
                                po("TripleDamageChance")
                            ));
                        }
                        let rb = po("RuthlessBlowHitEffect");
                        if rb != 1.0 {
                            b.push(sf!("x %.2f ^8(ruthless blow effect modifier)", rb));
                        }
                        let fw = po("FistOfWarDamageEffect");
                        if fw != 1.0 {
                            b.push(sf!("x %.2f ^8(fist of war effect modifier)", fw));
                        }
                        let ow = g("OffensiveWarcryEffect");
                        if ow != 1.0 && !max_hit {
                            b.push(sf!(
                                "x %.2f ^8(aggregated warcry exerted effect modifier)",
                                ow
                            ));
                        }
                        let mw = g("MaxOffensiveWarcryEffect");
                        if mw != 1.0 && max_hit {
                            b.push(sf!(
                                "x %.2f ^8(aggregated max warcry exerted effect modifier)",
                                mw
                            ));
                        }
                        if let Some(bd) = self.pbd(o) {
                            bd.set(t, b);
                        }
                    }
                    let warcry = if self.c.flag(list, None, &["Condition:WarcryMaxHit"])? {
                        self.go(o, "MaxOffensiveWarcryEffect")
                    } else {
                        self.go(o, "OffensiveWarcryEffect")
                    }
                    .unwrap_or(f64::NAN);
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
                        let eff_mult = self.hit_eff_mult(o, &cfg, t, pass, hit_avg, crit)?;
                        hit_min *= eff_mult;
                        hit_max *= eff_mult;
                        hit_avg *= eff_mult;
                        if self.input.breakdown {
                            self.set_po(o, &format!("{t}EffMult"), eff_mult);
                        }
                    }
                    if pass == 2
                        && let Some(b) = self.pbd(o).and_then(|b| b.table_mut(t))
                    {
                        b.push(crate::sf!("= %d to %d", hit_min, hit_max));
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
                } else if let Some(bd) = self.pbd(o) {
                    bd.set(
                        t,
                        crate::breakdown::Bd::of([format!("You can't deal {t} damage")]),
                    );
                }
                if pass == 1 {
                    self.set_po(o, &format!("{t}CritAverage"), hit_avg);
                    total_crit_avg += hit_avg;
                    total_crit_min += hit_min;
                    total_crit_max += hit_max;
                } else {
                    if self.input.breakdown {
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
                let portion = avg / total_hit_avg * 100.0;
                if let Some(b) = self.pbd(o).and_then(|b| b.table_mut(t)) {
                    b.push(crate::sf!("Portion of total damage: %d%%", portion));
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
        if self.bd(actor).is_some() {
            use crate::breakdown::Bd;
            use crate::sf;
            let effect = pv(self, o, "CritEffect");
            let hc = pv(self, o, "HitChance");
            let crit_lucky = self.c.flag(list, sc, &["CritLucky"])?;
            if effect != 1.0 {
                let dr = self.po(o, "DamageRolls").unwrap_or(0.0);
                let cr = self.po(o, "CritDamageRolls").unwrap_or(0.0);
                let rolls = dr.abs() + 2.0;
                let rc = cr.abs() + 2.0;
                let mut b = Bd::new();
                if dr > 0.0 {
                    b.push(sf!(
                        "(1/%d) x %d + (%d/%d) x %d = %.1f ^8(average from non-crits)",
                        rolls,
                        total_hit_min,
                        rolls - 1.0,
                        rolls,
                        total_hit_max,
                        total_hit_avg
                    ));
                }
                if dr < 0.0 {
                    b.push(sf!(
                        "(%d/%d) x %d + (1/%d) x %d = %.1f ^8(average from non-crits)",
                        rolls - 1.0,
                        rolls,
                        total_hit_min,
                        rolls,
                        total_hit_max,
                        total_hit_avg
                    ));
                }
                if dr > 0.0 || cr > 0.0 || crit_lucky {
                    b.push(sf!(
                        "(1/%d) x %d + (%d/%d) x %d = %.1f ^8(average from crits)",
                        rc,
                        total_crit_min,
                        rc - 1.0,
                        rc,
                        total_crit_max,
                        total_crit_avg
                    ));
                    b.push("");
                }
                if dr < 0.0 && !crit_lucky {
                    b.push(sf!(
                        "(%d/%d) x %d + (1/%d) x %d = %.1f ^8(average from crits)",
                        rolls - 1.0,
                        rolls,
                        total_crit_min,
                        rolls,
                        total_crit_max,
                        total_crit_avg
                    ));
                    b.push("");
                }
                b.push(sf!(
                    "%.1f x (1 - %.4f) ^8(damage from non-crits)",
                    total_hit_avg,
                    crit / 100.0
                ));
                b.push(sf!(
                    "+ %.1f x %.4f ^8(damage from crits)",
                    total_crit_avg,
                    crit / 100.0
                ));
                b.push(sf!("= %.1f", avg_hit));
                if let Some(bd) = self.pbd(o) {
                    bd.set("AverageHit", b);
                }
            }
            if hc < 100.0 {
                let label = o.passes[o.pass].label;
                let b = Bd::of([
                    sf!("%s:", label),
                    sf!("%.1f ^8(average hit)", avg_hit),
                    sf!("x %.2f ^8(chance to hit)", hc / 100.0),
                    sf!("= %.1f", avg_damage),
                ]);
                if let Some(bd) = self.pbd(o) {
                    bd.set("AverageDamage", b);
                }
            }
            let burst = self.go(o, "AverageBurstDamage").unwrap_or(f64::NAN);
            if burst > 0.0 {
                let mut b = Bd::of([sf!("%.1f ^8(average hit)", avg_hit)]);
                if hc < 100.0 {
                    b.push(sf!("x %.2f ^8(chance to hit)", hc / 100.0));
                }
                if repeat_penalty < 1.0 {
                    b.push(sf!("x %.2f ^8(number of repeats)", burst_hits - 1.0));
                    b.push(sf!("x %.2f ^8(repeat penalty)", repeat_penalty));
                    b.push(sf!("= %.1f ^8(repeat damage total)", burst - avg_damage));
                    b.push("");
                    b.push(sf!("+ %.1f ^8(first hit)", avg_hit));
                    if hc < 100.0 {
                        b.push(sf!("x %.2f ^8(chance to hit)", hc / 100.0));
                    }
                } else {
                    b.push(sf!("x %.2f ^8(number of hits)", burst_hits));
                }
                b.push(sf!("= %.1f ^8(total burst damage)", burst));
                self.bd(actor).unwrap().set("AverageBurstDamage", b);
            }
        }
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
    ) -> Result<f64> {
        let (i, list, edb) = (o.skill, o.list, o.edb);
        let c = Some(cfg);
        let mut resist;
        let mut pen = 0.0;
        let mut source_res = t.to_string();
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
                source_res = used.to_string();
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
        let pdb = self.pdb();
        if self.c.flag(
            pdb,
            None,
            &[&format!("Enemy{source_res}ResistEqualToYours")],
        )? {
            source_res = format!("Your {source_res} Resistance");
        }
        let mut eff = (1.0 + taken_inc / 100.0) * taken_more;
        let use_chance = use_resist(self, t)?;
        let cannot = is_elemental(t) && self.c.flag(list, c, &["CannotElePenIgnore"])?;
        let mut effective_resist = resist;
        if cannot {
            let r = if invert > 0.0 {
                resist - 2.0 * invert * resist
            } else {
                resist
            };
            effective_resist = r;
            eff *= 1.0 - r / 100.0;
        } else if use_chance > 0.0 {
            let r = if invert > 0.0 {
                ((resist - pen) * (1.0 - invert) + (-resist - pen) * invert) * use_chance
            } else {
                (resist - pen) * use_chance
            };
            effective_resist = r;
            eff *= 1.0 - r / 100.0;
        }
        if pass == 2 && (eff != 1.0 || source_res != t || invert > 0.0) && self.pbd(o).is_some() {
            let b = crate::breakdown::eff_mult(
                t,
                resist,
                if cannot { 0.0 } else { pen },
                taken_inc,
                eff,
                taken_more,
                Some(&source_res),
                use_chance > 0.0,
                Some(invert),
                Some(effective_resist),
            );
            let bd = self.pbd(o).unwrap();
            if let Some(tb) = bd.table_mut(t) {
                tb.push(crate::sf!("x %.3f ^8(effective DPS modifier)", eff));
            }
            bd.set(&format!("{t}EffMult"), b);
        }
        Ok(eff)
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
            if self.has_flag(i, "bothWeaponAttack") && self.bd(actor).is_some() {
                use crate::sf;
                let (m, oh) = (
                    get_num(&o.hands[0], "AverageDamage").unwrap_or(f64::NAN),
                    get_num(&o.hands[1], "AverageDamage").unwrap_or(f64::NAN),
                );
                let avg = self.c.out(actor, "AverageDamage").unwrap_or(f64::NAN);
                let line = if self.sd_truthy(i, "doubleHitsWhenDualWielding") {
                    sf!(
                        "%.1f + %.1f ^8(skill hits with both weapons at once)",
                        m,
                        oh
                    )
                } else {
                    sf!("(%.1f + %.1f) / 2 ^8(skill alternates weapons)", m, oh)
                };
                self.bd(actor).unwrap().set(
                    "AverageDamage",
                    crate::breakdown::Bd::of([
                        "Both weapons:".to_string(),
                        line,
                        sf!("= %.1f", avg),
                    ]),
                );
            }
        }
        if self.bd(actor).is_some() {
            use crate::sf;
            let g = |p: &Self, k: &str| p.c.out(actor, k);
            let avg = g(self, "AverageDamage").unwrap_or(f64::NAN);
            let rate_line = |kind: &str| match g(self, "HitSpeed") {
                Some(h) => sf!("x %.2f ^8(hit rate)", h),
                None => sf!(
                    &format!("x %.2f ^8({kind} rate)"),
                    g(self, "Speed").unwrap_or(f64::NAN)
                ),
            };
            let mut b = if o.is_attack {
                crate::breakdown::Bd::of([sf!("%.1f ^8(average damage)", avg), rate_line("attack")])
            } else if self.sd_truthy(i, "triggered") {
                crate::breakdown::Bd::of([
                    sf!("%.1f ^8(average damage)", avg),
                    rate_line("trigger"),
                ])
            } else {
                crate::breakdown::Bd::of([sf!("%.1f ^8(average hit)", avg), rate_line("cast")])
            };
            let dps_mult = self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
            if dps_mult != 1.0 {
                b.push(sf!("x %g ^8(DPS multiplier for this skill)", dps_mult));
            }
            if o.quantity_multiplier > 1.0 {
                b.push(sf!(
                    "x %g ^8(quantity multiplier for this skill)",
                    o.quantity_multiplier
                ));
            }
            b.push(sf!("= %.1f", g(self, "TotalDPS").unwrap_or(f64::NAN)));
            self.bd(actor).unwrap().set("TotalDPS", b);
        }
        if self.input.mode == crate::calcs::Mode::Calcs {
            let g = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
            let text = if self.sd_truthy(i, "showAverage") {
                format!(
                    "{} average damage",
                    format_num_sep(&format!("{:.1}", g(self, "AverageDamage")))
                )
            } else {
                format!(
                    "{} DPS",
                    format_num_sep(&format!("{:.1}", g(self, "TotalDPS")))
                )
            };
            self.c
                .set_out_val(actor, "DisplayDamage", OutVal::Str(text.into()));
        }
        if self.mods(i).minion.is_some() {
            let g = |p: &Self, k: &str| p.c.out(actor, k);
            let speed = g(self, "HitSpeed")
                .or_else(|| g(self, "Speed"))
                .unwrap_or(f64::NAN);
            let v = g(self, "SummonedMinionsPerCast").unwrap_or(f64::NAN)
                * speed
                * self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
            self.sd_set(i, "summonSpeed", v);
        }
        let g = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
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
        if self.bd(actor).is_some() {
            let speed = g(self, "HitSpeed");
            let speed = if speed.is_nan() {
                g(self, "Speed")
            } else {
                speed
            };
            let hit_rate = g(self, "HitChance") / 100.0
                * speed
                * self.sd_num(i, "dpsMultiplier").unwrap_or(f64::NAN);
            for (flag, pool, key) in [
                ("leechLife", "Life", "LifeLeech"),
                ("leechES", "EnergyShield", "EnergyShieldLeech"),
                ("leechMana", "Mana", "ManaLeech"),
            ] {
                if self.has_flag(i, flag) {
                    let b = self.leech_bd(actor, pool, hit_rate)?;
                    self.bd(actor).unwrap().set(key, b);
                }
            }
        }
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
        self.offence_ailments(o)
    }
}

impl<'a> Perform<'a> {
    /// `breakdown.leech(instant, instantRate, instances, pool, rate, max,
    /// dur, instantLeechProportion, hitRate)` of the actor's pool
    fn leech_bd(&self, actor: usize, pool: &str, hit_rate: f64) -> Result<crate::breakdown::Bd> {
        use crate::sf;
        let g = |k: &str| self.c.out(actor, k).unwrap_or(f64::NAN);
        let instant = g(&format!("{pool}LeechInstant"));
        let instant_rate = g(&format!("{pool}LeechInstantRate"));
        let instances = g(&format!("{pool}LeechInstances"));
        let size = g(pool);
        let max = g(&format!("Max{pool}LeechRate"));
        let dur = g(&format!("{pool}LeechDuration"));
        let prop = g(&format!("{pool}LeechInstantProportion"));
        let base = self.misc_data("LeechRateBase");
        let db = self.c.db(actor);
        // the module's `skillCfg` is an undefined global
        let rate_mod = self.c.calc_mod(db, None, &[&format!("{pool}LeechRate")])?;
        let show_average = self
            .actor_main_index(actor)
            .is_some_and(|m| self.sd_truthy(m, "showAverage"));
        let mut out = crate::breakdown::Bd::new();
        let instant_line = |out: &mut crate::breakdown::Bd| {
            if prop != 1.0 {
                out.push(sf!(
                    "Instant Leech: %.1f ^8(%d%% x %.1f)",
                    instant,
                    prop * 100.0,
                    dur * size * base / (1.0 - prop)
                ));
            } else {
                out.push(sf!("Instant Leech: %.1f", instant));
            }
        };
        let base_lines = |out: &mut crate::breakdown::Bd| {
            out.push(sf!("%d ^8(size of leech destination pool)", size));
            out.push(sf!(
                "x %.2f ^8(base leech rate is %d%% per second)",
                base,
                100.0 * base
            ));
            if rate_mod != 1.0 {
                out.push(sf!("x %.2f ^8(leech rate modifier)", rate_mod));
            }
        };
        if show_average {
            if instant > 0.0 {
                instant_line(&mut out);
            }
            if instances > 0.0 {
                out.push("Total leeched per instance:");
                base_lines(&mut out);
                out.push(sf!("x %.2fs ^8(instance duration)", dur));
                out.push(sf!("= %.1f", size * base * rate_mod * dur));
            }
        } else {
            if instant_rate > 0.0 {
                instant_line(&mut out);
                out.push(sf!(
                    "Instant Leech per second: %.1f ^8(%.1f x %.2f)",
                    instant_rate,
                    instant,
                    hit_rate
                ));
            }
            if instances > 0.0 {
                out.push("Rate per instance:");
                base_lines(&mut out);
                out.push(sf!("= %.1f ^8per second", size * base * rate_mod));
                out.push("Maximum leech rate against one target:");
                out.push(sf!("%.1f", size * base * rate_mod));
                out.push(sf!("x %.2f ^8(average instances)", instances));
                let total = size * base * rate_mod * instances;
                out.push(sf!("= %.1f ^8per second", total));
                if total <= max {
                    out.push(sf!("Time to reach max: %.1fs", dur));
                }
                out.push(sf!("Leech rate cap: %.1f", max));
                if total > max {
                    out.push(sf!("Time to reach cap: %.1fs", dur / total * max));
                }
            }
        }
        Ok(out)
    }
}

/// `formatNumSep(str)` of a number formatted with `%.1f`, with the default
/// separators (`,` for thousands, `.` for decimals)
pub(crate) fn format_num_sep(s: &str) -> String {
    let (minus, rest) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s),
    };
    let (int, frac) = match rest.find('.') {
        Some(k) => (&rest[..k], &rest[k..]),
        None => (rest, ""),
    };
    if !int.chars().all(|c| c.is_ascii_digit()) {
        return s.to_string();
    }
    let mut out = String::new();
    for (k, c) in int.chars().enumerate() {
        if k > 0 && (int.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{minus}{out}{frac}")
}
