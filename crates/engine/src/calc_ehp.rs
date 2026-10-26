//! `calcs.buildDefenceEstimations` (`Modules/CalcDefence.lua`) on the typed
//! calculation: the enemy's damage, damage taken, stun, the pools, the
//! number of hits to die (effective hit pool), recoup, the maximum hit
//! taken and degens.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::OutVal;
use crate::calc::{m_max, m_min, round};
use crate::calc_defence::{DMG_TYPES, armour_reduction, armour_reduction_f, is_elemental};
use crate::calc_perform::Perform;
use crate::config::Val;
use crate::mods::{Cfg, ModType};

pub type Result<T> = std::result::Result<T, String>;

use ModType::{Base, More};

/// An ally whose life is spent before the actor's (`allyLifePoolList`)
struct AllyPool {
    life: &'static str,
    mitigation: &'static str,
    redirect: Option<&'static str>,
    fallback: Option<&'static str>,
    melee_only: bool,
}

const ALLY_POOLS: [AllyPool; 9] = [
    AllyPool {
        life: "TotalMinionLife",
        mitigation: "MinionAllyDamageMitigation",
        redirect: Some("takenFromMinionBeforeYou"),
        fallback: Some("Multiplier:MinionLife"),
        melee_only: false,
    },
    AllyPool {
        life: "TotalRadianceSentinelLife",
        mitigation: "RadianceSentinelAllyDamageMitigation",
        redirect: Some("takenFromRadianceSentinelBeforeYou"),
        fallback: None,
        melee_only: false,
    },
    AllyPool {
        life: "TotalSpectreLife",
        mitigation: "SpectreAllyDamageMitigation",
        redirect: Some("takenFromSpectresBeforeYou"),
        fallback: None,
        melee_only: false,
    },
    AllyPool {
        life: "TotalTotemLife",
        mitigation: "TotemAllyDamageMitigation",
        redirect: Some("takenFromTotemsBeforeYou"),
        fallback: None,
        melee_only: false,
    },
    AllyPool {
        life: "TotalVaalRejuvenationTotemLife",
        mitigation: "VaalRejuvenationTotemAllyDamageMitigation",
        redirect: Some("takenFromVaalRejuvenationTotemsBeforeYou"),
        fallback: None,
        melee_only: false,
    },
    AllyPool {
        life: "TotalVoidSpawnLife",
        mitigation: "VoidSpawnAllyDamageMitigation",
        redirect: Some("takenFromVoidSpawnBeforeYou"),
        fallback: None,
        melee_only: false,
    },
    AllyPool {
        life: "TotalStoneGolemLife",
        mitigation: "StoneGolemAllyDamageMitigation",
        redirect: Some("takenFromStoneGolemBeforeYou"),
        fallback: None,
        melee_only: true,
    },
    AllyPool {
        life: "AlliedEnergyShield",
        mitigation: "SoulLinkMitigation",
        redirect: None,
        fallback: None,
        melee_only: false,
    },
    AllyPool {
        life: "FrostShieldLife",
        mitigation: "FrostShieldDamageMitigation",
        redirect: None,
        fallback: None,
        melee_only: false,
    },
];

/// An ally's pool during a hit: remaining life and the share it takes
#[derive(Clone, Debug)]
struct AllyLeft {
    remaining: f64,
    percent: f64,
}

/// Damage by type, in the order the types were set (Lua's tables)
type Damage = Vec<(&'static str, f64)>;

fn dmg_get(d: &Damage, t: &str) -> Option<f64> {
    d.iter().find(|(k, _)| *k == t).map(|(_, v)| *v)
}

/// The pools `reducePoolsByDamage` reduces
#[derive(Clone, Debug, Default)]
struct Pools {
    allies: Option<Vec<Option<AllyLeft>>>,
    aegis: Option<Vec<(String, f64)>>,
    guard: Option<Vec<(String, f64)>>,
    ward: Option<f64>,
    ward_active_chance: Option<f64>,
    energy_shield: Option<f64>,
    mana: Option<f64>,
    life: Option<f64>,
    recoupable: Vec<(&'static str, f64)>,
    life_loss_lost_over_time: Option<f64>,
    life_below_half_loss_lost_over_time: Option<f64>,
    overkill: f64,
    hit_pool_remaining: f64,
}

fn kv_get(v: &[(String, f64)], k: &str) -> f64 {
    v.iter().find(|(x, _)| x == k).map_or(0.0, |(_, n)| *n)
}

fn kv_set(v: &mut Vec<(String, f64)>, k: &str, n: f64) {
    match v.iter_mut().find(|(x, _)| x == k) {
        Some((_, x)) => *x = n,
        None => v.push((k.to_string(), n)),
    }
}

/// `numberOfHitsToDie`'s input (`DamageIn`)
#[derive(Clone, Debug, Default)]
struct DamageIn {
    dmg: [f64; 5],
    cycles: f64,
    iterations: f64,
    track_recoupable: bool,
    track_life_loss: bool,
    limit_speedup: bool,
    gain_when_hit: bool,
    life_when_hit: Option<f64>,
    mana_when_hit: Option<f64>,
    es_when_hit: Option<f64>,
    missing_life: Option<f64>,
    missing_mana: Option<f64>,
    cycles_ran: bool,
}

fn calc_life_hit_pool(life: f64, max_life: f64, prevented: f64, below_half: f64) -> f64 {
    let half = max_life * 0.5;
    let above = m_max(life - half, 0.0);
    above / (1.0 - prevented / 100.0)
        + m_min(life, half) / (1.0 - below_half / 100.0) / (1.0 - prevented / 100.0)
}

impl<'a> Perform<'a> {
    fn o(&self, actor: usize, k: &str) -> f64 {
        self.c.out(actor, k).unwrap_or(f64::NAN)
    }

    /// `output[k] or default`
    fn o_or(&self, actor: usize, k: &str, d: f64) -> f64 {
        self.c.out(actor, k).unwrap_or(d)
    }

    fn input_num(&self, k: &str) -> Option<f64> {
        self.input
            .config
            .get(k)
            .map(Val::num)
            .filter(|n| !n.is_nan())
    }

    fn placeholder_num(&self, k: &str) -> Option<f64> {
        self.input
            .placeholder
            .get(k)
            .map(Val::num)
            .filter(|n| !n.is_nan())
    }

    fn input_truthy(&self, k: &str) -> bool {
        self.input
            .config
            .get(k)
            .is_some_and(|v| !matches!(v, Val::Bool(false)))
    }

    /// `buildAllyLifePools(output)`
    fn ally_life_pools(&self, actor: usize) -> Vec<Option<AllyLeft>> {
        ALLY_POOLS
            .iter()
            .map(|a| {
                let life = self.c.out(actor, a.life)?;
                let mitigation = self.c.out(actor, a.mitigation)?;
                (life > 0.0 && mitigation > 0.0).then(|| AllyLeft {
                    remaining: life,
                    percent: m_min(mitigation, 100.0) / 100.0,
                })
            })
            .collect()
    }

    /// `calcs.takenHitFromDamage(rawDamage, damageType, actor)`
    fn taken_hit_from_damage(
        &self,
        actor: usize,
        raw: f64,
        typ: &str,
        shift: &[(&'static str, f64)],
    ) -> Result<(f64, Damage)> {
        let db = self.c.db(actor);
        let o = |k: &str| self.o(actor, k);
        let mut sum = 0.0;
        let mut damages: Damage = Vec::new();
        let _ = typ;
        for &(conv_type, percent) in shift {
            let taken_flat = o(&format!("{conv_type}takenFlat"));
            if percent > 0.0 || taken_flat != 0.0 {
                let converted = raw * percent / 100.0;
                let vaal = m_min(
                    -self
                        .c
                        .sum(db, More, None, &["VaalArcticArmourMitigation"])?
                        / 100.0,
                    1.0,
                );
                let mult = {
                    let resist_mult = o(&format!("{conv_type}ResistTakenHitMulti"));
                    let applied = o(&format!("{conv_type}EffectiveAppliedArmour"));
                    let armour_dr = armour_reduction_f(applied, converted * resist_mult);
                    let flat_dr = if self.c.flag(
                        db,
                        None,
                        &[&format!("SelfIgnoreBase{conv_type}DamageReduction")],
                    )? {
                        0.0
                    } else {
                        self.c
                            .out(actor, &format!("Base{conv_type}DamageReductionWhenHit"))
                            .unwrap_or_else(|| o(&format!("Base{conv_type}DamageReduction")))
                    };
                    let total_dr = m_min(o("DamageReductionMax"), armour_dr + flat_dr);
                    let overwhelm = if self.c.flag(
                        db,
                        None,
                        &[&format!("SelfIgnore{conv_type}DamageReduction")],
                    )? {
                        0.0
                    } else {
                        o(&format!("{conv_type}EnemyOverwhelm"))
                    };
                    let dr_multi = 1.0
                        - m_max(m_min(o("DamageReductionMax"), total_dr - overwhelm), 0.0) / 100.0;
                    resist_mult * dr_multi
                };
                let reduced = round(
                    m_max(converted * mult + taken_flat, 0.0)
                        * o(&format!("{conv_type}AfterReductionTakenHitMulti")),
                    0.0,
                ) * (1.0 - vaal);
                sum += reduced;
                if reduced > 0.0 || percent > 0.0 {
                    damages.push((conv_type, reduced));
                }
            }
        }
        Ok((sum, damages))
    }

    /// `calcs.reducePoolsByDamage(poolTable, damageTable, actor)`
    fn reduce_pools(&self, actor: usize, pools: Option<Pools>, damage: &Damage) -> Result<Pools> {
        let db = self.c.db(actor);
        let o = |k: &str| self.o(actor, k);
        let p = pools.unwrap_or_default();
        let mut table: Damage = Vec::new();
        let mut damage_total = 0.0;
        for &(t, d) in damage {
            if d > 0.0 {
                let c = d.ceil();
                table.push((t, c));
                damage_total += c;
            }
        }
        let mut allies = p
            .allies
            .clone()
            .unwrap_or_else(|| self.ally_life_pools(actor));
        let mut recoupable = p.recoupable.clone();
        let mut aegis = match p.aegis.clone() {
            Some(a) => a,
            None => {
                let mut a = vec![
                    ("shared".to_string(), self.o_or(actor, "sharedAegis", 0.0)),
                    (
                        "sharedElemental".to_string(),
                        self.o_or(actor, "sharedElementalAegis", 0.0),
                    ),
                ];
                for &(t, _) in &table {
                    kv_set(&mut a, t, self.o_or(actor, &format!("{t}Aegis"), 0.0));
                }
                a
            }
        };
        let mut guard = match p.guard.clone() {
            Some(g) => g,
            None => {
                let mut g = vec![(
                    "shared".to_string(),
                    self.o_or(actor, "sharedGuardAbsorb", 0.0),
                )];
                for &(t, _) in &table {
                    kv_set(&mut g, t, self.o_or(actor, &format!("{t}GuardAbsorb"), 0.0));
                }
                g
            }
        };
        let mut ward = p.ward.unwrap_or_else(|| self.o_or(actor, "Ward", 0.0));
        let ward_active = p
            .ward_active_chance
            .unwrap_or(if ward > 0.0 { 1.0 } else { 0.0 });
        let ward_avoid_break = if self.c.flag(db, None, &["Condition:WardNotBreak"])? {
            1.0
        } else {
            m_min(
                self.c.base(db, None, &["WardAvoidBreakChance"])? / 100.0,
                1.0,
            )
        };
        let ward_bypass_below = self.c.base(db, None, &["WardBypassBelowPercent"])? / 100.0;
        let mut es = p
            .energy_shield
            .unwrap_or_else(|| o("EnergyShieldRecoveryCap"));
        let mut mana = p
            .mana
            .unwrap_or_else(|| self.o_or(actor, "ManaUnreserved", 0.0));
        let mut life = p
            .life
            .unwrap_or_else(|| self.o_or(actor, "LifeRecoverable", 0.0));
        let below_half = self.c.base(db, None, &["LifeLossBelowHalfPrevented"])?;
        let mut lost_over_time = p.life_loss_lost_over_time.unwrap_or(0.0);
        let mut below_half_lost = p.life_below_half_loss_lost_over_time.unwrap_or(0.0);
        let mut overkill = 0.0;
        if ward < 0.0 {
            ward = 0.0;
        }
        let ward_before = ward;
        if es < 0.0 {
            es = 0.0;
        }
        if mana < 0.0 {
            mana = 0.0;
        }
        if life < 0.0 {
            life = 0.0;
        }
        let es_protects_mana = self.c.flag(db, None, &["EnergyShieldProtectsMana"])?;
        let initial_pool = calc_life_hit_pool(life, o("Life"), o("preventedLifeLoss"), below_half);
        let mut mom_left: Vec<(&str, f64)> = Vec::new();
        let mut es_left: Vec<(&str, f64)> = Vec::new();
        for &(t, _) in &table {
            let mom = m_min(
                o("sharedMindOverMatter") + o(&format!("{t}MindOverMatter")),
                100.0,
            ) / 100.0;
            let max_mom = initial_pool / (1.0 - mom) - initial_pool;
            let mom_pool = if mom < 1.0 {
                m_min(max_mom, mana)
            } else {
                mana
            };
            let life_mom = initial_pool + mom_pool;
            let bypass = o(&format!("{t}EnergyShieldBypass")) / 100.0;
            let es_pool = if bypass > 0.0 {
                m_min(life_mom / bypass - life_mom, es)
            } else {
                es
            };
            if es_protects_mana && bypass < 1.0 {
                let e = m_min(
                    m_min(max_mom * (1.0 - bypass), es),
                    life_mom / (1.0 - (1.0 - bypass) * mom) - life_mom,
                )
                .floor();
                es_left.push((t, e));
                mom_left.push((t, m_min(max_mom - e, mom_pool).floor()));
            } else {
                es_left.push((t, es_pool.floor()));
                mom_left.push((t, mom_pool.floor()));
            }
        }
        let get = |v: &Vec<(&str, f64)>, t: &str| {
            v.iter()
                .find(|(k, _)| *k == t)
                .map_or(f64::NAN, |(_, x)| *x)
        };
        let ward_bypass_mod = self.c.base(db, None, &["WardBypass"])?;
        for t in DMG_TYPES {
            let Some(mut rem) = dmg_get(&table, t) else {
                continue;
            };
            for (a, ally) in ALLY_POOLS.iter().zip(allies.iter_mut()) {
                let _ = a;
                if let Some(v) = ally
                    && v.remaining > 0.0
                {
                    let temp = m_min(rem * v.percent, v.remaining);
                    v.remaining = (v.remaining - temp).floor();
                    rem -= temp;
                }
            }
            match recoupable.iter_mut().find(|(k, _)| *k == t) {
                Some((_, x)) => *x += rem,
                None => recoupable.push((t, rem)),
            }
            let a = kv_get(&aegis, t);
            if a > 0.0 {
                let temp = m_min(rem, a);
                kv_set(&mut aegis, t, a - temp);
                rem -= temp;
            }
            let se = kv_get(&aegis, "sharedElemental");
            if is_elemental(t) && se > 0.0 {
                let temp = m_min(rem, se);
                kv_set(&mut aegis, "sharedElemental", se - temp);
                rem -= temp;
            }
            let sh = kv_get(&aegis, "shared");
            if sh > 0.0 {
                let temp = m_min(rem, sh);
                kv_set(&mut aegis, "shared", sh - temp);
                rem -= temp;
            }
            let g = kv_get(&guard, t);
            if g > 0.0 {
                let temp = m_min(rem * o(&format!("{t}GuardAbsorbRate")) / 100.0, g);
                kv_set(&mut guard, t, (g - temp).floor());
                rem -= temp;
            }
            let gs = kv_get(&guard, "shared");
            if gs > 0.0 {
                let temp = m_min(rem * o("sharedGuardAbsorbRate") / 100.0, gs);
                kv_set(&mut guard, "shared", (gs - temp).floor());
                rem -= temp;
            }
            if ward > 0.0
                && (ward_bypass_below == 0.0 || damage_total >= ward_before * ward_bypass_below)
            {
                let mut temp = m_min(rem * (1.0 - ward_bypass_mod / 100.0), ward);
                ward -= temp;
                temp *= ward_active;
                rem -= temp;
            }
            let mom = m_min(
                o("sharedMindOverMatter") + o(&format!("{t}MindOverMatter")),
                100.0,
            ) / 100.0;
            let bypass = o(&format!("{t}EnergyShieldBypass")) / 100.0;
            let es_pool = get(&es_left, t);
            if es > 0.0 && !es_protects_mana && bypass < 1.0 {
                let temp = m_min(rem * (1.0 - bypass), es_pool);
                for (_, x) in es_left.iter_mut() {
                    *x = m_max(*x - temp, 0.0);
                }
                es -= temp;
                rem -= temp;
            }
            if mom > 0.0 {
                let mut mom_damage = (rem * mom).ceil();
                if es_protects_mana && es > 0.0 && bypass < 1.0 {
                    let temp = m_min(mom_damage * (1.0 - bypass), es_pool).ceil();
                    for (_, x) in es_left.iter_mut() {
                        *x = m_max(*x - temp, 0.0).floor();
                    }
                    es -= temp;
                    rem -= temp;
                    mom_damage -= temp;
                }
                let temp = m_min(mom_damage, get(&mom_left, t)).ceil();
                for (_, x) in mom_left.iter_mut() {
                    *x = m_max(*x - temp, 0.0).floor();
                }
                mana -= temp;
                rem -= temp;
            }
            let prevented_total = o("preventedLifeLossTotal");
            if prevented_total > 0.0 {
                let half = o("Life") * 0.5;
                let over_half = m_max(life - half, 0.0);
                let prevent = o("preventedLifeLoss") / 100.0;
                let above_low = over_half / (1.0 - prevent);
                let below_pct = below_half / 100.0;
                let can_take = above_low
                    + m_max(m_min(life, half), 0.0)
                        / (1.0 - below_pct)
                        / (1.0 - o("preventedLifeLoss") / 100.0);
                if can_take < rem {
                    overkill += rem - can_take;
                    rem = can_take;
                }
                if o("preventedLifeLossBelowHalf") != 0.0 {
                    let split = m_min(rem, above_low);
                    let lost = split * (1.0 - prevent);
                    let prevented = split * prevent;
                    rem -= split;
                    lost_over_time += prevented;
                    life -= lost;
                    if life <= half {
                        let unspecific = rem * prevent;
                        lost_over_time += unspecific;
                        rem -= unspecific;
                        let specific = rem * below_pct;
                        below_half_lost += specific;
                        rem -= specific;
                    }
                } else {
                    let temp = rem * o("preventedLifeLoss") / 100.0;
                    lost_over_time += temp;
                    rem -= temp;
                }
            }
            if life > 0.0 {
                let temp = m_min(rem, life);
                life -= temp;
                rem -= temp;
            }
            overkill += rem;
        }
        let mut mom_remaining = f64::INFINITY;
        let mut es_remaining = f64::INFINITY;
        for &(t, _) in &table {
            mom_remaining = m_min(mom_remaining, get(&mom_left, t));
            es_remaining = m_min(es_remaining, get(&es_left, t));
        }
        let hit_pool = (if life >= 1.0 {
            calc_life_hit_pool(life, o("Life"), o("preventedLifeLoss"), below_half)
        } else {
            0.0
        }) + if mom_remaining != f64::INFINITY {
            mom_remaining
        } else {
            0.0
        } + if es_remaining != f64::INFINITY {
            es_remaining
        } else {
            0.0
        };
        Ok(Pools {
            allies: Some(allies),
            aegis: Some(aegis),
            guard: Some(guard),
            ward: Some(
                (if ward < ward_before {
                    if ward_avoid_break > 0.0 {
                        ward_before
                    } else {
                        0.0
                    }
                } else {
                    ward
                })
                .floor(),
            ),
            ward_active_chance: Some(if ward < ward_before {
                ward_active * ward_avoid_break
            } else {
                ward_active
            }),
            energy_shield: Some(es.floor()),
            mana: Some(mana.floor()),
            life: Some(life.floor()),
            recoupable,
            life_loss_lost_over_time: Some(lost_over_time.ceil()),
            life_below_half_loss_lost_over_time: Some(below_half_lost.ceil()),
            overkill: overkill.ceil(),
            hit_pool_remaining: hit_pool.floor(),
        })
    }

    /// `numberOfHitsToDie(DamageIn)`
    fn number_of_hits_to_die(&mut self, actor: usize, d: &mut DamageIn) -> Result<f64> {
        let db = self.c.db(actor);
        let mut num_hits = 0.0;
        for x in d.dmg {
            num_hits += x;
        }
        if num_hits == 0.0 {
            return Ok(f64::INFINITY);
        }
        let ward_avoid_break = if self.c.flag(db, None, &["Condition:WardNotBreak"])? {
            1.0
        } else {
            m_min(
                self.c.base(db, None, &["WardAvoidBreakChance"])? / 100.0,
                1.0,
            )
        };
        let out_ward = self.o(actor, "Ward");
        if ward_avoid_break == 1.0 && out_ward > 0.0 && num_hits < out_ward {
            return Ok(f64::INFINITY);
        }
        num_hits = 0.0;
        let mut ward = self.o_or(actor, "Ward", 0.0);
        if ward_avoid_break < 1.0 && d.cycles > 1.0 {
            ward = 0.0;
        }
        let mut aegis = vec![
            ("shared".to_string(), self.o_or(actor, "sharedAegis", 0.0)),
            (
                "sharedElemental".to_string(),
                self.o_or(actor, "sharedElementalAegis", 0.0),
            ),
        ];
        let mut guard = vec![(
            "shared".to_string(),
            self.o_or(actor, "sharedGuardAbsorb", 0.0),
        )];
        for t in DMG_TYPES {
            aegis.push((t.to_string(), self.o_or(actor, &format!("{t}Aegis"), 0.0)));
            guard.push((
                t.to_string(),
                self.o_or(actor, &format!("{t}GuardAbsorb"), 0.0),
            ));
        }
        let mut pools = Pools {
            allies: Some(self.ally_life_pools(actor)),
            aegis: Some(aegis),
            guard: Some(guard),
            ward: Some(ward),
            ward_active_chance: None,
            energy_shield: Some(self.o(actor, "EnergyShieldRecoveryCap")),
            mana: Some(self.o_or(actor, "ManaUnreserved", 0.0)),
            life: Some(self.o_or(actor, "LifeRecoverable", 0.0)),
            recoupable: Vec::new(),
            life_loss_lost_over_time: Some(self.o_or(actor, "LifeLossLostOverTime", 0.0)),
            life_below_half_loss_lost_over_time: Some(self.o_or(
                actor,
                "LifeBelowHalfLossLostOverTime",
                0.0,
            )),
            overkill: 0.0,
            hit_pool_remaining: 0.0,
        };
        if d.cycles != 1.0 {
            d.track_recoupable = false;
            d.track_life_loss = false;
        }
        let mut vaal_hits_left = self.o(actor, "VaalArcticArmourLife");
        if d.cycles > 1.0 {
            vaal_hits_left = 0.0;
        }
        let mut iteration_mult = 1.0;
        let mut damage_total = 0.0;
        let max_damage = self.misc_data("ehpCalcMaxDamage");
        let max_iterations = self.misc_data("ehpCalcMaxIterationsToCalc");
        let vaal_mitigation = self.o(actor, "VaalArcticArmourMitigation");
        let life_recoverable = self.o_or(actor, "LifeRecoverable", 0.0);
        let mana_unreserved = self.o_or(actor, "ManaUnreserved", 0.0);
        let es_cap = self.o(actor, "EnergyShieldRecoveryCap");
        let life_unreserved = self.o_or(actor, "LifeUnreserved", 0.0);
        while pools.life.unwrap_or(0.0) > 0.0 && d.iterations < max_iterations {
            d.iterations += 1.0;
            damage_total = 0.0;
            let vaal_mult = if vaal_hits_left > 0.0 {
                1.0 - vaal_mitigation * m_min(vaal_hits_left / iteration_mult, 1.0)
            } else {
                1.0
            };
            vaal_hits_left -= iteration_mult;
            let mut damage: Damage = Vec::new();
            for (k, t) in DMG_TYPES.iter().enumerate() {
                let x = d.dmg[k];
                if x > 0.0 {
                    damage.push((t, x * iteration_mult * vaal_mult));
                }
                damage_total += x;
            }
            if (d.gain_when_hit || d.missing_life.is_some() || d.missing_mana.is_some())
                && (iteration_mult > 1.0 || d.cycles > 1.0)
            {
                let gain = iteration_mult * d.cycles;
                if let Some(m) = d.missing_life {
                    let l = pools.life.unwrap_or(0.0);
                    pools.life = Some(m_min(
                        l + m * (life_unreserved - l) * (gain - 1.0) / 100.0,
                        gain * life_recoverable,
                    ));
                }
                if let Some(m) = d.missing_mana {
                    let mn = pools.mana.unwrap_or(0.0);
                    pools.mana = Some(m_min(
                        mn + m * (mana_unreserved - mn) * (gain - 1.0) / 100.0,
                        gain * mana_unreserved,
                    ));
                }
                if d.gain_when_hit {
                    let nan = f64::NAN;
                    pools.life = Some(m_min(
                        pools.life.unwrap_or(0.0) + d.life_when_hit.unwrap_or(nan) * (gain - 1.0),
                        gain * life_recoverable,
                    ));
                    pools.mana = Some(m_min(
                        pools.mana.unwrap_or(0.0) + d.mana_when_hit.unwrap_or(nan) * (gain - 1.0),
                        gain * mana_unreserved,
                    ));
                    pools.energy_shield = Some(m_min(
                        pools.energy_shield.unwrap_or(0.0)
                            + d.es_when_hit.unwrap_or(nan) * (gain - 1.0),
                        gain * es_cap,
                    ));
                }
            }
            if let Some(m) = d.missing_life
                && pools.life.unwrap_or(0.0) > 0.0
            {
                let l = pools.life.unwrap_or(0.0);
                pools.life = Some(m_min(
                    l + m * (life_unreserved - l) / 100.0,
                    life_recoverable,
                ));
            }
            if let Some(m) = d.missing_mana
                && pools.mana.unwrap_or(0.0) > 0.0
            {
                let mn = pools.mana.unwrap_or(0.0);
                pools.mana = Some(m_min(
                    mn + m * (mana_unreserved - mn) / 100.0,
                    mana_unreserved,
                ));
            }
            pools = self.reduce_pools(actor, Some(pools), &damage)?;
            if pools.life.unwrap_or(0.0) > 0.0 && damage_total >= max_damage {
                return Ok(f64::INFINITY);
            }
            if d.gain_when_hit && pools.life.unwrap_or(0.0) > 0.0 {
                let nan = f64::NAN;
                pools.life = Some(m_min(
                    pools.life.unwrap_or(0.0) + d.life_when_hit.unwrap_or(nan),
                    life_recoverable,
                ));
                pools.mana = Some(m_min(
                    pools.mana.unwrap_or(0.0) + d.mana_when_hit.unwrap_or(nan),
                    mana_unreserved,
                ));
                pools.energy_shield = Some(m_min(
                    pools.energy_shield.unwrap_or(0.0) + d.es_when_hit.unwrap_or(nan),
                    es_cap,
                ));
            }
            iteration_mult = 1.0;
            let speed_up = if d.limit_speedup {
                4.0
            } else {
                self.misc_data("ehpCalcSpeedUp")
            };
            let ward_avoid_active =
                ward_avoid_break < 1.0 && pools.ward_active_chance.unwrap_or(0.0) > 0.01;
            if !d.cycles_ran
                && !ward_avoid_active
                && pools.life.unwrap_or(0.0) > 0.0
                && d.iterations < max_iterations
            {
                let mut inner = DamageIn {
                    dmg: d.dmg.map(|x| x * speed_up),
                    limit_speedup: d.limit_speedup,
                    cycles: d.cycles * speed_up,
                    iterations: d.iterations,
                    ..DamageIn::default()
                };
                if d.gain_when_hit {
                    inner.gain_when_hit = true;
                    inner.life_when_hit = d.life_when_hit;
                    inner.mana_when_hit = d.mana_when_hit;
                    inner.es_when_hit = d.es_when_hit;
                }
                iteration_mult = m_max(
                    (self.number_of_hits_to_die(actor, &mut inner)? - 1.0) * speed_up - 1.0,
                    1.0,
                );
                if iteration_mult == f64::INFINITY {
                    return Ok(f64::INFINITY);
                }
                d.iterations = inner.iterations;
                d.cycles_ran = true;
            }
            num_hits += iteration_mult;
        }
        if d.track_recoupable {
            for (t, r) in &pools.recoupable {
                let k = format!("{t}RecoupableDamageTaken");
                let v = self.o(actor, &k) + r;
                self.c.set_out(actor, &k, v);
            }
        }
        if d.track_life_loss {
            let v = self.o(actor, "LifeLossLostOverTime")
                + pools.life_loss_lost_over_time.unwrap_or(0.0);
            self.c.set_out(actor, "LifeLossLostOverTime", v);
            let v = self.o(actor, "LifeBelowHalfLossLostOverTime")
                + pools.life_below_half_loss_lost_over_time.unwrap_or(0.0);
            self.c.set_out(actor, "LifeBelowHalfLossLostOverTime", v);
        }
        if pools.life == Some(0.0) && d.cycles == 1.0 {
            num_hits -= pools.overkill / damage_total;
        }
        damage_total = 0.0;
        for x in d.dmg {
            damage_total += x * num_hits;
        }
        if pools.life.unwrap_or(0.0) >= 0.0 && damage_total >= max_damage {
            return Ok(f64::INFINITY);
        }
        if num_hits.is_nan() {
            return Ok(0.0);
        }
        Ok(m_max(num_hits, 0.0))
    }

    /// `calcs.buildDefenceEstimations(env, actor)`
    pub fn defence_estimations(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let enemy = self.c.actors.actors[actor].enemy.ok_or("no enemy")?;
        let edb = self.c.db(enemy);
        let category = match self.input.config.get("enemyDamageType") {
            Some(v) => v.text(),
            None => "Average".to_string(),
        };
        let category = category.as_str();
        let o = |p: &Self, k: &str| p.o(actor, k);
        let set = |p: &mut Self, k: &str, v: f64| p.c.set_out(actor, k, v);
        let worst_of = self.unlucky_worst_of();

        if category != "DamageOverTime" {
            let worst = worst_of.unwrap_or(1.0);
            let attack_dodge = 1.0 - o(self, "EffectiveAttackDodgeChance") / 100.0;
            let avoid_all = 1.0 - o(self, "AvoidAllDamageFromHitsChance") / 100.0;
            let specific = matches!(
                self.c.out_val(actor, "specificTypeAvoidance"),
                Some(OutVal::Bool(true))
            );
            let proj = 1.0
                - (if specific {
                    0.0
                } else {
                    o(self, "AvoidProjectilesChance")
                }) / 100.0;
            let spell_dodge = 1.0 - o(self, "EffectiveSpellDodgeChance") / 100.0;
            let v = 100.0
                - (1.0 - o(self, "MeleeEvadeChance") / 100.0) * attack_dodge * avoid_all * 100.0;
            set(self, "MeleeNotHitChance", v);
            let v = 100.0
                - (1.0 - o(self, "ProjectileEvadeChance") / 100.0)
                    * attack_dodge
                    * avoid_all
                    * proj
                    * 100.0;
            set(self, "ProjectileNotHitChance", v);
            set(
                self,
                "SpellNotHitChance",
                100.0 - spell_dodge * avoid_all * 100.0,
            );
            set(
                self,
                "SpellProjectileNotHitChance",
                100.0 - spell_dodge * avoid_all * proj * 100.0,
            );
            set(self, "UntypedNotHitChance", 100.0 - avoid_all * 100.0);
            let v = (o(self, "MeleeNotHitChance")
                + o(self, "ProjectileNotHitChance")
                + o(self, "SpellNotHitChance")
                + o(self, "SpellProjectileNotHitChance"))
                / 4.0;
            set(self, "AverageNotHitChance", v);
            let v = (o(self, "MeleeEvadeChance") + o(self, "ProjectileEvadeChance")) / 4.0;
            set(self, "AverageEvadeChance", v);
            let mut nh = o(self, &format!("{category}NotHitChance"));
            let mut ev = self.o_or(actor, &format!("{category}EvadeChance"), 0.0);
            if worst > 1.0 {
                nh = nh / 100.0 * nh;
                ev = ev / 100.0 * ev;
                if worst == 4.0 {
                    nh = nh / 100.0 * nh;
                    ev = ev / 100.0 * ev;
                }
            }
            set(self, "ConfiguredNotHitChance", nh);
            set(self, "ConfiguredEvadeChance", ev);
        }

        // The enemy's damage
        set(self, "totalEnemyDamage", 0.0);
        set(self, "totalEnemyDamageIn", 0.0);
        let enemy_mult = |p: &Self, names: &[String]| -> Result<f64> {
            let n: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            p.c.calc_mod(edb, None, &n)
        };
        let names_for = |t: &str| -> Vec<String> {
            let mut v = vec!["Damage".to_string(), format!("{t}Damage")];
            if is_elemental(t) {
                v.push("ElementalDamage".into());
            }
            v
        };
        if category == "DamageOverTime" {
            for t in DMG_TYPES {
                set(self, &format!("{t}EnemyPen"), 0.0);
                let m = enemy_mult(self, &names_for(t))?;
                set(self, &format!("{t}EnemyDamageMult"), m);
                set(self, &format!("{t}EnemyOverwhelm"), 0.0);
                set(self, &format!("{t}EnemyDamage"), 0.0);
            }
        } else {
            let crit_chance = if self.c.flag(edb, None, &["NeverCrit"])? {
                0.0
            } else if self.c.flag(edb, None, &["AlwaysCrit"])? {
                100.0
            } else {
                let base = match self.c.override_num(db, None, &["enemyCritChance"])? {
                    Some(v) => v,
                    None => self
                        .input
                        .config
                        .get("enemyCritChance")
                        .or_else(|| self.input.placeholder.get("enemyCritChance"))
                        .map_or(0.0, Val::num),
                };
                m_max(
                    m_min(
                        base * (1.0
                            + self.c.inc(db, None, &["EnemyCritChance"])? / 100.0
                            + self.c.inc(edb, None, &["CritChance"])? / 100.0)
                            * (1.0 - o(self, "ConfiguredEvadeChance") / 100.0),
                        100.0,
                    ),
                    0.0,
                )
            };
            set(self, "EnemyCritChance", crit_chance);
            let crit_damage = m_max(
                self.input
                    .config
                    .get("enemyCritDamage")
                    .or_else(|| self.input.placeholder.get("enemyCritDamage"))
                    .map_or(0.0, Val::num)
                    + self.c.base(edb, None, &["CritMultiplier"])?,
                0.0,
            );
            let crit_effect = 1.0
                + crit_chance / 100.0
                    * (crit_damage / 100.0)
                    * (1.0 - o(self, "CritExtraDamageReduction") / 100.0);
            set(self, "EnemyCritEffect", crit_effect);
            let match_all = self.kf("MatchAll");
            let enemy_cfg = Cfg {
                keyword_flags: Some(!match_all),
                ..Cfg::default()
            };
            let ec = Some(&enemy_cfg);
            // Converted damage: to type, from type, amount
            let mut conversion: Vec<(&str, Vec<(&str, f64)>)> = Vec::new();
            for t in DMG_TYPES {
                let mult = enemy_mult(self, &names_for(t))?;
                let mut damage = self.input_num(&format!("enemy{t}Damage"));
                let pen = self.input_num(&format!("enemy{t}Pen"));
                let overwhelm = self.input_num(&format!("enemy{t}Overwhelm"));
                if damage.is_none() {
                    damage = Some(
                        self.placeholder_num(&format!("enemy{t}Damage"))
                            .unwrap_or(0.0),
                    );
                }
                let pen = pen.unwrap_or_else(|| {
                    self.placeholder_num(&format!("enemy{t}Pen")).unwrap_or(0.0)
                });
                let mut overwhelm = overwhelm.unwrap_or_else(|| {
                    self.placeholder_num(&format!("enemy{t}enemyOverwhelm"))
                        .unwrap_or(0.0)
                });
                let mut damage = damage.unwrap_or(0.0);
                damage += (self.c.base(edb, ec, &[&format!("{t}Min")])?
                    + self.c.base(edb, ec, &[&format!("{t}Max")])?)
                    / 2.0;
                let mut conversion_total = 0.0;
                if t == "Physical" {
                    let mut skill = [0.0; 5];
                    let mut conv = [0.0; 5];
                    let (mut total, mut total_skill) = (0.0, 0.0);
                    for (k, to) in DMG_TYPES.iter().enumerate() {
                        skill[k] =
                            self.c
                                .base(edb, ec, &[&format!("{t}DamageSkillConvertTo{to}")])?;
                        conv[k] = self
                            .c
                            .base(edb, ec, &[&format!("{t}DamageConvertTo{to}")])?;
                        total_skill += skill[k];
                        total += conv[k];
                    }
                    if total_skill > 100.0 {
                        let m = 100.0 / total_skill;
                        total_skill *= m;
                        total = 0.0;
                        for k in 0..5 {
                            skill[k] *= m;
                            conv[k] = 0.0;
                        }
                    } else if total + total_skill > 100.0 {
                        let m = (100.0 - total_skill) / total;
                        total *= m;
                        for c in conv.iter_mut() {
                            *c *= m;
                        }
                    }
                    conversion_total = total + total_skill;
                    for (k, to) in DMG_TYPES.iter().enumerate() {
                        let gain =
                            self.c.base(edb, ec, &[&format!("{t}DamageGainAs{to}")])? / 100.0;
                        let mut cp = conv[k] / 100.0;
                        let sp = skill[k] / 100.0;
                        if sp > 0.0 && *to != "Chaos" {
                            let lvl = self.env.enemy_level;
                            let table = &self.data().monster_phys_conversion_multi;
                            let m = (lvl.fract() == 0.0 && lvl >= 1.0)
                                .then(|| table.get(lvl as usize - 1).copied())
                                .flatten()
                                .ok_or("attempt to perform arithmetic on a nil value")?;
                            cp += sp * (1.0 + m / 100.0);
                        }
                        if gain > 0.0 || cp > 0.0 {
                            let v = damage * gain + damage * cp;
                            match conversion.iter_mut().find(|(k, _)| k == to) {
                                Some((_, l)) => match l.iter_mut().find(|(f, _)| *f == t) {
                                    Some((_, x)) => *x = v,
                                    None => l.push((t, v)),
                                },
                                None => conversion.push((to, vec![(t, v)])),
                            }
                        }
                    }
                }
                overwhelm += self.c.base(edb, None, &["PhysicalOverwhelm"])?
                    + self.c.base(db, None, &["EnemyPhysicalOverwhelm"])?;
                set(self, &format!("{t}EnemyPen"), pen);
                set(self, &format!("{t}EnemyDamageMult"), mult);
                set(self, &format!("{t}EnemyOverwhelm"), overwhelm);
                let v = o(self, "totalEnemyDamageIn") + damage;
                set(self, "totalEnemyDamageIn", v);
                let mut ed = damage * (1.0 - conversion_total / 100.0) * mult * crit_effect;
                if let Some((_, froms)) = conversion.iter().find(|(k, _)| *k == t) {
                    for &(from, amount) in froms {
                        let mut names = vec![
                            "Damage".to_string(),
                            format!("{t}Damage"),
                            format!("{from}Damage"),
                        ];
                        if is_elemental(t) {
                            names.push("ElementalDamage".into());
                        }
                        if is_elemental(from) {
                            names.push("ElementalDamage".into());
                        }
                        let m = enemy_mult(self, &names)?;
                        ed += amount * m * crit_effect;
                    }
                }
                set(self, &format!("{t}EnemyDamage"), ed);
                let v = o(self, "totalEnemyDamage") + ed;
                set(self, "totalEnemyDamage", v);
            }
        }

        // Damage taken as
        let mut shift: Vec<(&'static str, Vec<(&'static str, f64)>)> = Vec::new();
        let mut dot_shift: Vec<(&'static str, Vec<(&'static str, f64)>)> = Vec::new();
        for t in DMG_TYPES {
            let mut st = Vec::new();
            let mut dst = Vec::new();
            let (mut dest_total, mut dot_dest_total) = (0.0, 0.0);
            for dest in DMG_TYPES {
                if dest == t {
                    continue;
                }
                let mut names = vec![format!("{t}DamageTakenAs{dest}")];
                if is_elemental(t) {
                    names.push(format!("ElementalDamageTakenAs{dest}"));
                }
                let n: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                let dv = self.c.base(db, None, &n)?;
                dst.push((dest, dv));
                dot_dest_total += dv;
                let mut names = vec![format!("{t}DamageFromHitsTakenAs{dest}")];
                if is_elemental(t) {
                    names.push(format!("ElementalDamageFromHitsTakenAs{dest}"));
                }
                let n: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                let sv = dv + self.c.base(db, None, &n)?;
                st.push((dest, sv));
                dest_total += sv;
            }
            dst.push((t, m_max(100.0 - dot_dest_total, 0.0)));
            st.push((t, m_max(100.0 - dest_total, 0.0)));
            let own = st.last().unwrap().1;
            dot_shift.push((t, dst));
            shift.push((t, st));
            let v = o(self, &format!("{t}EnemyDamage")) * own / 100.0;
            set(self, &format!("{t}TakenDamage"), v);
        }
        let shift_of = |t: &str| -> Vec<(&'static str, f64)> {
            shift
                .iter()
                .find(|(k, _)| *k == t)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        for t in DMG_TYPES {
            for c in DMG_TYPES {
                if t != c {
                    let pct = shift_of(t)
                        .iter()
                        .find(|(k, _)| *k == c)
                        .map_or(0.0, |(_, v)| *v);
                    let d = o(self, &format!("{t}EnemyDamage")) * pct / 100.0;
                    let v = o(self, &format!("{c}TakenDamage")) + d;
                    set(self, &format!("{c}TakenDamage"), v);
                }
            }
        }
        set(self, "totalTakenDamage", 0.0);
        for t in DMG_TYPES {
            let v = o(self, "totalTakenDamage") + o(self, &format!("{t}TakenDamage"));
            set(self, "totalTakenDamage", v);
        }

        // Damage taken multipliers
        self.c
            .set_out_val(actor, "AnyTakenReflect", OutVal::Bool(false));
        for t in DMG_TYPES {
            let mut base_inc =
                self.c
                    .inc(db, None, &["DamageTaken", &format!("{t}DamageTaken")])?;
            let mut base_more =
                self.c
                    .more(db, None, &["DamageTaken", &format!("{t}DamageTaken")])?;
            if is_elemental(t) {
                base_inc += self.c.inc(db, None, &["ElementalDamageTaken"])?;
                base_more *= self.c.more(db, None, &["ElementalDamageTaken"])?;
            }
            let names = [
                "DamageTakenWhenHit".to_string(),
                format!("{t}DamageTakenWhenHit"),
            ];
            let n: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            let mut inc = base_inc + self.c.inc(db, None, &n)?;
            let mut more = base_more * self.c.more(db, None, &n)?;
            if is_elemental(t) {
                inc += self.c.inc(db, None, &["ElementalDamageTakenWhenHit"])?;
                more *= self.c.more(db, None, &["ElementalDamageTakenWhenHit"])?;
            }
            set(
                self,
                &format!("{t}TakenHitMult"),
                m_max((1.0 + inc / 100.0) * more, 0.0),
            );
            for h in ["Attack", "Spell"] {
                let i = inc + self.c.inc(db, None, &[&format!("{h}DamageTaken")])?;
                let m = more * self.c.more(db, None, &[&format!("{h}DamageTaken")])?;
                let v = m_max((1.0 + i / 100.0) * m, 0.0);
                set(self, &format!("{h}TakenHitMult"), v);
                set(self, &format!("{t}{h}TakenHitMult"), v);
            }
            let names = [
                "ReflectedDamageTaken".to_string(),
                format!("{t}ReflectedDamageTaken"),
            ];
            let n: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            inc += self.c.inc(db, None, &n)?;
            more *= self.c.more(db, None, &n)?;
            if is_elemental(t) {
                inc += self.c.inc(db, None, &["ElementalReflectedDamageTaken"])?;
                more *= self.c.more(db, None, &["ElementalReflectedDamageTaken"])?;
            }
            set(
                self,
                &format!("{t}TakenReflect"),
                m_max((1.0 + inc / 100.0) * more, 0.0),
            );
            // Damage over time
            let names = [
                "DamageTakenOverTime".to_string(),
                format!("{t}DamageTakenOverTime"),
            ];
            let n: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            let mut inc = base_inc + self.c.inc(db, None, &n)?;
            let mut more = base_more * self.c.more(db, None, &n)?;
            if is_elemental(t) {
                inc += self.c.inc(db, None, &["ElementalDamageTakenOverTime"])?;
                more *= self.c.more(db, None, &["ElementalDamageTakenOverTime"])?;
            }
            let resist = if self
                .c
                .flag(db, None, &[&format!("SelfIgnore{t}Resistance")])?
            {
                0.0
            } else {
                self.c
                    .out(actor, &format!("{t}ResistOverTime"))
                    .unwrap_or_else(|| o(self, &format!("{t}Resist")))
            };
            let reduction =
                if self
                    .c
                    .flag(db, None, &[&format!("SelfIgnoreBase{t}DamageReduction")])?
                {
                    0.0
                } else {
                    o(self, &format!("Base{t}DamageReduction"))
                };
            let v = m_max(
                (1.0 - resist / 100.0) * (1.0 - reduction / 100.0) * (1.0 + inc / 100.0) * more,
                0.0,
            );
            set(self, &format!("{t}TakenDotMult"), v);
        }

        // Incoming hit damage
        set(self, "totalTakenHit", 0.0);
        let impale_flags = if matches!(category, "Melee" | "Projectile" | "Average") {
            self.mf("Attack")
        } else {
            0
        };
        let impale_cfg = Cfg {
            flags: Some(impale_flags),
            keyword_flags: Some(0),
            ..Cfg::default()
        };
        let impale_chance = self.c.base(edb, Some(&impale_cfg), &["ImpaleChance"])?
            * if category == "Average" { 0.5 } else { 1.0 }
            * (1.0 - self.o_or(actor, "ImpaleAvoidChance", 0.0));
        let dr_max = o(self, "DamageReductionMax");
        for t in DMG_TYPES {
            let resist = if self
                .c
                .flag(db, None, &[&format!("SelfIgnore{t}Resistance")])?
            {
                0.0
            } else {
                self.c
                    .out(actor, &format!("{t}ResistWhenHit"))
                    .unwrap_or_else(|| o(self, &format!("{t}Resist")))
            };
            let reduction =
                if self
                    .c
                    .flag(db, None, &[&format!("SelfIgnoreBase{t}DamageReduction")])?
                {
                    0.0
                } else {
                    self.c
                        .out(actor, &format!("Base{t}DamageReductionWhenHit"))
                        .unwrap_or_else(|| o(self, &format!("Base{t}DamageReduction")))
                };
            let pen = if self.c.flag(
                db,
                None,
                &[
                    &format!("SelfIgnore{t}Resistance"),
                    &format!("EnemyCannotPen{t}Resistance"),
                ],
            )? {
                0.0
            } else {
                o(self, &format!("{t}EnemyPen"))
            };
            let overwhelm = if self
                .c
                .flag(db, None, &[&format!("SelfIgnore{t}DamageReduction")])?
            {
                0.0
            } else {
                o(self, &format!("{t}EnemyOverwhelm"))
            };
            let damage = o(self, &format!("{t}TakenDamage"));
            let mut impale = if impale_chance > 0.0 && t == "Physical" {
                damage * self.misc_data("ImpaleStoredDamageBase")
            } else {
                0.0
            };
            let mut armour_reduct = 0.0;
            let mut impale_armour_reduct = 0.0;
            let pct_armour = m_min(
                if !self
                    .c
                    .flag(db, None, &[&format!("ArmourDoesNotApplyTo{t}DamageTaken")])?
                {
                    self.c
                        .base(db, None, &[&format!("ArmourAppliesTo{t}DamageTaken")])?
                } else {
                    0.0
                },
                100.0,
            );
            let mut applied =
                (o(self, "Armour") * pct_armour / 100.0) * (1.0 + o(self, "ArmourDefense"));
            let ward_based =
                t == "Physical" && self.c.flag(db, None, &["PhysicalReductionBasedOnWard"])?;
            if ward_based {
                let m = self
                    .c
                    .override_num(db, None, &["PhysicalReductionBasedOnWardPercent"])?
                    .ok_or("attempt to perform arithmetic on a nil value")?
                    / 100.0;
                applied = o(self, "Ward") * m;
            }
            let res_mult = 1.0 - (resist - pen) / 100.0;
            let mut taken_flat = self.c.base(
                db,
                None,
                &[
                    "DamageTaken",
                    &format!("{t}DamageTaken"),
                    "DamageTakenWhenHit",
                    &format!("{t}DamageTakenWhenHit"),
                ],
            )?;
            match category {
                "Melee" | "Projectile" => {
                    taken_flat += self.c.base(
                        db,
                        None,
                        &[
                            "DamageTakenFromAttacks",
                            &format!("{t}DamageTakenFromAttacks"),
                            &format!("{t}DamageTakenFrom{category}Attacks"),
                        ],
                    )?;
                }
                "Spell" | "SpellProjectile" => {
                    taken_flat += self.c.base(
                        db,
                        None,
                        &[
                            "DamageTakenFromSpells",
                            &format!("{t}DamageTakenFromSpells"),
                            &format!("{t}DamageTakenFromSpellProjectiles"),
                        ],
                    )?;
                }
                "Average" => {
                    taken_flat += self.c.base(
                        db,
                        None,
                        &[
                            "DamageTakenFromAttacks",
                            &format!("{t}DamageTakenFromAttacks"),
                        ],
                    )? / 2.0
                        + self.c.base(
                            db,
                            None,
                            &[&format!("{t}DamageTakenFromProjectileAttacks")],
                        )? / 4.0
                        + self.c.base(
                            db,
                            None,
                            &[
                                "DamageTakenFromSpells",
                                &format!("{t}DamageTakenFromSpells"),
                            ],
                        )? / 2.0
                        + self.c.base(
                            db,
                            None,
                            &[
                                "DamageTakenFromSpellProjectiles",
                                &format!("{t}DamageTakenFromSpellProjectiles"),
                            ],
                        )? / 4.0;
                }
                _ => {}
            }
            set(self, &format!("{t}takenFlat"), taken_flat);
            if pct_armour > 0.0 || ward_based {
                armour_reduct = m_min(dr_max, armour_reduction(applied, damage * res_mult));
                if impale > 0.0 {
                    impale_armour_reduct =
                        m_min(dr_max, armour_reduction(applied, impale * res_mult));
                }
            }
            let total_reduct = m_min(dr_max, armour_reduct + reduction);
            let reduct_mult = 1.0 - m_max(m_min(dr_max, total_reduct - overwhelm), 0.0) / 100.0;
            set(
                self,
                &format!("{t}DamageReduction"),
                100.0 - reduct_mult * 100.0,
            );
            if impale > 0.0 {
                impale = impale
                    * res_mult
                    * (1.0
                        - m_max(
                            m_min(
                                dr_max,
                                m_min(dr_max, impale_armour_reduct + reduction) - overwhelm,
                            ),
                            0.0,
                        ) / 100.0);
                impale =
                    impale * impale_chance / 100.0 * 5.0 * o(self, &format!("{t}TakenReflect"));
            }
            let mut taken_mult = o(self, &format!("{t}TakenHitMult"));
            let mut suppress_mult = 1.0;
            let suppress_full = o(self, "EffectiveSpellSuppressionChance") == 100.0;
            match category {
                "Melee" | "Projectile" => taken_mult = o(self, &format!("{t}AttackTakenHitMult")),
                "Spell" | "SpellProjectile" => {
                    taken_mult = o(self, &format!("{t}SpellTakenHitMult"));
                    if suppress_full {
                        suppress_mult = 1.0 - o(self, "SpellSuppressionEffect") / 100.0;
                    }
                }
                "Average" => {
                    taken_mult = (o(self, &format!("{t}SpellTakenHitMult"))
                        + o(self, &format!("{t}AttackTakenHitMult")))
                        / 2.0;
                    if suppress_full {
                        suppress_mult = 1.0 - o(self, "SpellSuppressionEffect") / 100.0 / 2.0;
                    }
                }
                _ => {}
            }
            set(self, &format!("{t}EffectiveAppliedArmour"), applied);
            set(self, &format!("{t}ResistTakenHitMulti"), res_mult);
            let after = taken_mult * suppress_mult;
            set(self, &format!("{t}AfterReductionTakenHitMulti"), after);
            let base_mult = res_mult * reduct_mult;
            set(self, &format!("{t}BaseTakenHitMult"), base_mult * after);
            let taken_hit =
                m_max(damage * base_mult + taken_flat, 0.0) * taken_mult * suppress_mult + impale;
            set(self, &format!("{t}TakenHit"), taken_hit);
            set(
                self,
                &format!("{t}TakenHitMult"),
                if damage > 0.0 {
                    taken_hit / damage
                } else {
                    0.0
                },
            );
            let v = o(self, "totalTakenHit") + taken_hit;
            set(self, "totalTakenHit", v);
        }

        self.stun(actor, category)?;
        self.pools(actor, category)?;

        if category != "DamageOverTime" {
            self.hits_to_die(actor, category)?;
        }
        self.recoup_estimates(actor, category)?;
        self.pvp(actor)?;
        self.maximum_hit(actor, &shift)?;
        for t in DMG_TYPES {
            let v = o(self, &format!("{t}TotalPool")) / o(self, &format!("{t}TakenDotMult"));
            set(self, &format!("{t}DotEHP"), v);
        }
        self.degens(actor, category, &dot_shift)?;
        Ok(())
    }

    fn stun(&mut self, actor: usize, category: &str) -> Result<()> {
        let db = self.c.db(actor);
        let o = |p: &Self, k: &str| p.o(actor, k);
        let mut base =
            if self
                .c
                .flag(db, None, &["StunThresholdBasedOnEnergyShieldInsteadOfLife"])?
            {
                o(self, "EnergyShield")
                    * self
                        .c
                        .base(db, None, &["StunThresholdEnergyShieldPercent"])?
                    / 100.0
            } else if self
                .c
                .flag(db, None, &["StunThresholdBasedOnManaInsteadOfLife"])?
            {
                o(self, "Mana") * self.c.base(db, None, &["StunThresholdManaPercent"])? / 100.0
            } else if self.c.flag(db, None, &["ChaosInoculation"])? {
                self.c.base(db, None, &["Life"])?
            } else {
                o(self, "Life")
            };
        if self.c.flag(db, None, &["AddESToStunThreshold"])? {
            base += o(self, "EnergyShield")
                * self.c.base(db, None, &["ESToStunThresholdPercent"])?
                / 100.0;
        }
        let threshold = base * (1.0 + self.c.inc(db, None, &["StunThreshold"])? / 100.0);
        self.c.set_out(actor, "StunThreshold", threshold);
        let mut not_avoid = if self.c.flag(db, None, &["StunImmune"])? {
            0.0
        } else {
            100.0 - m_min(self.c.base(db, None, &["AvoidStun"])?, 100.0)
        };
        let pdb = self.pdb();
        if o(self, "EnergyShield") > o(self, "totalTakenHit")
            && !self.c.flag(pdb, None, &["EnergyShieldProtectsMana"])?
        {
            not_avoid *= 0.5;
        }
        self.c.set_out(actor, "StunAvoidChance", 100.0 - not_avoid);
        if 100.0 - not_avoid >= 100.0 {
            self.c.set_out(actor, "StunDuration", 0.0);
            self.c.set_out(actor, "BlockDuration", 0.0);
        } else {
            let dur = 1.0 + self.c.inc(db, None, &["StunDuration"])? / 100.0;
            let base_dur = self.misc_data("StunBaseDuration");
            let rec = 1.0 + self.c.inc(db, None, &["StunRecovery"])? / 100.0;
            let block_rec = 1.0 + self.c.inc(db, None, &["StunRecovery", "BlockRecovery"])? / 100.0;
            let tick = self.misc_data("ServerTickRate");
            self.c.set_out(
                actor,
                "StunDuration",
                (base_dur * dur / rec * tick).ceil() / tick,
            );
            self.c.set_out(
                actor,
                "BlockDuration",
                (base_dur * dur / block_rec * tick).ceil() / tick,
            );
        }
        let v = m_min(self.c.base(db, None, &["AvoidInterruptStun"])?, 100.0);
        self.c.set_out(actor, "InterruptStunAvoidChance", v);
        let mut eff = o(self, "totalTakenHit") + o(self, "PhysicalTakenHit") * 0.25;
        let not_melee = self.misc_data("StunNotMeleeDamageMult");
        if category != "Average" {
            eff = eff * (1.0 + not_melee * 3.0) / 4.0;
        } else if category != "Melee" {
            eff *= not_melee;
        }
        let base_chance = m_min(self.misc_data("StunBaseMult") * eff / threshold, 100.0);
        let v = (if base_chance > self.misc_data("MinStunChanceNeeded") {
            base_chance
        } else {
            0.0
        }) * not_avoid
            / 100.0;
        self.c.set_out(actor, "SelfStunChance", v);
        Ok(())
    }

    /// Life recoverable, loss prevention, bypass, mind over matter, guard,
    /// aegis, allies' pools and the total pools
    fn pools(&mut self, actor: usize, category: &str) -> Result<()> {
        let db = self.c.db(actor);
        let o = |p: &Self, k: &str| p.o(actor, k);
        let set = |p: &mut Self, k: &str, v: f64| p.c.set_out(actor, k, v);
        let low_life = self.input_truthy("conditionLowLife");
        let mut recoverable = o(self, "LifeUnreserved");
        if low_life {
            let pct = self
                .c
                .out(actor, "LowLifePercentage")
                .unwrap_or_else(|| self.misc_data("LowPoolThreshold"));
            recoverable = m_min(o(self, "Life") * pct / 100.0, o(self, "LifeUnreserved"));
            if recoverable < o(self, "LifeUnreserved") {
                self.c.set_out_val(actor, "CappingLife", OutVal::Bool(true));
            }
        }
        if self.c.flag(db, None, &["DamageInsteadReservesLife"])? {
            recoverable = (o(self, "LifeCancellableReservation") / 100.0) * o(self, "Life");
        }
        let recoverable = m_max(recoverable, 1.0);
        set(self, "LifeRecoverable", recoverable);
        let prevented = m_min(self.c.base(db, None, &["LifeLossPrevented"])?, 100.0);
        set(self, "preventedLifeLoss", prevented);
        let below_half = self.c.base(db, None, &["LifeLossBelowHalfPrevented"])?;
        let ph = (1.0 - prevented / 100.0) * below_half;
        set(self, "preventedLifeLossBelowHalf", ph);
        if !low_life {
            let portion = m_min(o(self, "Life") * 0.5 / recoverable, 1.0);
            set(self, "preventedLifeLossTotal", prevented + ph * portion);
        } else {
            set(self, "preventedLifeLossTotal", prevented + ph);
        }
        let hit_pool = calc_life_hit_pool(recoverable, o(self, "Life"), prevented, below_half);
        set(self, "LifeHitPool", hit_pool);

        // Energy shield bypass
        let mut any_bypass = false;
        let mut minimum = 100.0;
        for t in DMG_TYPES {
            let k = format!("{t}EnergyShieldBypass");
            let mut v;
            if self.c.flag(db, None, &["UnblockedDamageDoesBypassES"])? {
                v = 100.0;
                any_bypass = true;
            } else {
                v = match self.c.override_num(db, None, &[&k])? {
                    Some(x) => x,
                    None => self.c.base(db, None, &[&k])?,
                };
                if v != 0.0 {
                    any_bypass = true;
                }
                if t == "Chaos" {
                    if !self.c.flag(db, None, &["ChaosNotBypassEnergyShield"])? {
                        v += 100.0;
                    } else {
                        any_bypass = true;
                    }
                }
            }
            let v = m_max(m_min(v, 100.0), 0.0);
            set(self, &k, v);
            minimum = m_min(minimum, v);
        }
        self.c
            .set_out_val(actor, "AnyBypass", OutVal::Bool(any_bypass));
        set(self, "MinimumBypass", minimum);

        let mut any_specific = false;
        self.c
            .set_out_val(actor, "OnlySharedMindOverMatter", OutVal::Bool(false));
        self.c
            .set_out_val(actor, "AnySpecificMindOverMatter", OutVal::Bool(false));
        let shared_mom = m_min(
            self.c.base(db, None, &["DamageTakenFromManaBeforeLife"])?,
            100.0,
        );
        set(self, "sharedMindOverMatter", shared_mom);
        let es_protects = self.c.flag(db, None, &["EnergyShieldProtectsMana"])?;
        let mom_pool = |p: &Self, life_pool: f64, mom: f64, bypass: f64| -> f64 {
            let mana = m_max(p.o_or(actor, "ManaUnreserved", 0.0), 0.0);
            let max_mom = if mom < 1.0 {
                life_pool / (1.0 - mom) - life_pool
            } else {
                f64::INFINITY
            };
            let max_mana = m_min(mana, max_mom).floor();
            let max_es = if es_protects && bypass < 1.0 {
                m_min(
                    m_min(o(p, "EnergyShieldRecoveryCap"), max_mom * (1.0 - bypass)),
                    (life_pool + max_mana) / (1.0 - (1.0 - bypass) * mom) - (life_pool + max_mana),
                )
                .floor()
            } else {
                0.0
            };
            let used = m_min(max_mom - max_es, max_mana).floor();
            life_pool + used + max_es
        };
        if shared_mom > 0.0 {
            let mom = shared_mom / 100.0;
            let bypass = minimum / 100.0;
            let v = mom_pool(self, recoverable, mom, bypass);
            set(self, "sharedManaEffectiveLife", v);
            let v = mom_pool(self, hit_pool, mom, bypass);
            set(self, "sharedMoMHitPool", v);
        } else {
            set(self, "sharedManaEffectiveLife", recoverable);
            set(self, "sharedMoMHitPool", hit_pool);
        }
        for t in DMG_TYPES {
            let v = m_min(
                self.c
                    .base(db, None, &[&format!("{t}DamageTakenFromManaBeforeLife")])?,
                100.0 - shared_mom,
            );
            set(self, &format!("{t}MindOverMatter"), v);
            let bypass_t = o(self, &format!("{t}EnergyShieldBypass"));
            if v > 0.0 || (bypass_t > minimum && shared_mom > 0.0) {
                any_specific = true;
                self.c
                    .set_out_val(actor, "AnySpecificMindOverMatter", OutVal::Bool(true));
                self.c
                    .set_out_val(actor, "OnlySharedMindOverMatter", OutVal::Bool(false));
                let mom = (v + shared_mom) / 100.0;
                let bypass = bypass_t / 100.0;
                let x = mom_pool(self, recoverable, mom, bypass);
                set(self, &format!("{t}ManaEffectiveLife"), x);
                let x = mom_pool(self, hit_pool, mom, bypass);
                set(self, &format!("{t}MoMHitPool"), x);
            } else {
                let x = o(self, "sharedManaEffectiveLife");
                set(self, &format!("{t}ManaEffectiveLife"), x);
                let x = o(self, "sharedMoMHitPool");
                set(self, &format!("{t}MoMHitPool"), x);
            }
        }

        // Guard
        self.c.set_out_val(actor, "AnyGuard", OutVal::Bool(false));
        let rate = m_min(self.c.base(db, None, &["GuardAbsorbRate"])?, 100.0);
        set(self, "sharedGuardAbsorbRate", rate);
        if rate > 0.0 {
            self.c
                .set_out_val(actor, "OnlySharedGuard", OutVal::Bool(true));
            let v = self.c.val(db, "GuardAbsorbLimit", None)?;
            set(self, "sharedGuardAbsorb", v);
        }
        for t in DMG_TYPES {
            let r = m_min(
                self.c.base(db, None, &[&format!("{t}GuardAbsorbRate")])?,
                100.0,
            );
            set(self, &format!("{t}GuardAbsorbRate"), r);
            if r > 0.0 {
                any_specific = true;
                self.c.set_out_val(actor, "AnyGuard", OutVal::Bool(true));
                self.c
                    .set_out_val(actor, "OnlySharedGuard", OutVal::Bool(false));
                let v = self.c.val(db, &format!("{t}GuardAbsorbLimit"), None)?;
                set(self, &format!("{t}GuardAbsorb"), v);
            }
        }

        // Aegis
        let mut any_aegis = false;
        let shared = self.c.max(db, None, &["AegisValue"])?.unwrap_or(0.0);
        let shared_ele = self
            .c
            .max(db, None, &["ElementalAegisValue"])?
            .unwrap_or(0.0);
        set(self, "sharedAegis", shared);
        set(self, "sharedElementalAegis", shared_ele);
        if shared > 0.0 {
            any_aegis = true;
        }
        if shared_ele > 0.0 {
            any_specific = true;
            any_aegis = true;
        }
        for t in DMG_TYPES {
            let v = self
                .c
                .max(db, None, &[&format!("{t}AegisValue")])?
                .unwrap_or(0.0);
            if v > 0.0 {
                any_specific = true;
                any_aegis = true;
                set(self, &format!("{t}Aegis"), v);
            } else {
                set(self, &format!("{t}Aegis"), 0.0);
            }
            if is_elemental(t) {
                let v = o(self, &format!("{t}Aegis")) + shared_ele;
                set(self, &format!("{t}AegisDisplay"), v);
            }
        }
        self.c
            .set_out_val(actor, "AnyAegis", OutVal::Bool(any_aegis));
        self.c.set_out_val(
            actor,
            "ehpSectionAnySpecificTypes",
            OutVal::Bool(any_specific),
        );

        // Frost shield and allies
        let v = self.c.base(db, None, &["FrostGlobeHealth"])?;
        set(self, "FrostShieldLife", v);
        let v = self.c.base(db, None, &["FrostGlobeDamageMitigation"])?;
        set(self, "FrostShieldDamageMitigation", v);
        for a in &ALLY_POOLS {
            let Some(redirect) = a.redirect else { continue };
            let mut m = self.c.base(db, None, &[redirect])?;
            if a.melee_only {
                m = match category {
                    "Melee" => m,
                    "Average" => m / 4.0,
                    _ => 0.0,
                };
            }
            set(self, a.mitigation, m);
            if m != 0.0 {
                let mut life = self.c.base(db, None, &[a.life])?;
                if life == 0.0
                    && let Some(f) = a.fallback
                {
                    life = self.c.base(db, None, &[f])?;
                }
                let v = self.c.override_num(db, None, &[a.life])?.unwrap_or(life);
                set(self, a.life, v);
            }
        }
        for a in &ALLY_POOLS {
            if self
                .c
                .flag(db, None, &[&format!("MinionLifeShares{}", a.life)])?
            {
                return Err("not ported: shared minion life pools".into());
            }
        }
        if self.o_or(actor, "TotemAllyDamageMitigation", 0.0) > 0.0
            && self.o_or(actor, "TotalTotemLife", 0.0) == 0.0
            && self.o_or(actor, "TotalVaalRejuvenationTotemLife", 0.0) > 0.0
        {
            let v = o(self, "VaalRejuvenationTotemAllyDamageMitigation")
                + o(self, "TotemAllyDamageMitigation");
            set(self, "VaalRejuvenationTotemAllyDamageMitigation", v);
            set(self, "TotemAllyDamageMitigation", 0.0);
            self.c.clear_out(actor, "TotalTotemLife");
        }
        let soul_link = self.c.base(db, None, &["TakenFromParentESBeforeYou"])?;
        set(self, "SoulLinkMitigation", soul_link);
        if soul_link != 0.0 {
            let parent = self.c.actors.actors[actor].parent.ok_or("no parent")?;
            let v = self.c.out(parent, "EnergyShieldRecoveryCap").unwrap_or(0.0);
            set(self, "AlliedEnergyShield", v);
        } else {
            let v = self
                .c
                .base(db, None, &["TakenFromPartyMemberESBeforeYou"])?;
            set(self, "SoulLinkMitigation", v);
            if v != 0.0 {
                return Err("not ported: energy shield of party members".into());
            }
        }

        let v = self.c.base(db, None, &["VaalArcticArmourMaxHits"])?;
        set(self, "VaalArcticArmourLife", v);
        let v = m_min(
            -self
                .c
                .sum(db, More, None, &["VaalArcticArmourMitigation"])?
                / 100.0,
            1.0,
        );
        set(self, "VaalArcticArmourMitigation", v);

        // Total pools
        let es_cap = o(self, "EnergyShieldRecoveryCap");
        for t in DMG_TYPES {
            let mut total = o(self, &format!("{t}ManaEffectiveLife"));
            let mut total_hit = o(self, &format!("{t}MoMHitPool"));
            let bypass = o(self, &format!("{t}EnergyShieldBypass"));
            if bypass < 100.0 && !es_protects {
                if bypass > 0.0 {
                    let protected = es_cap / (1.0 - bypass / 100.0) * (bypass / 100.0);
                    total =
                        m_max(total - protected, 0.0) + m_min(total, protected) / (bypass / 100.0);
                    total_hit = m_max(total_hit - protected, 0.0)
                        + m_min(total_hit, protected) / (bypass / 100.0);
                } else {
                    total += es_cap;
                    total_hit += es_cap;
                }
            }
            set(self, &format!("{t}TotalPool"), total);
            set(self, &format!("{t}TotalHitPool"), total_hit);
        }
        let _ = Base;
        Ok(())
    }

    /// The number of hits to die, the chance to be hit and the EHP
    fn hits_to_die(&mut self, actor: usize, category: &str) -> Result<()> {
        let db = self.c.db(actor);
        let enemy = self.c.actors.actors[actor].enemy.ok_or("no enemy")?;
        let edb = self.c.db(enemy);
        let o = |p: &Self, k: &str| p.o(actor, k);
        let mut d = DamageIn {
            cycles: 1.0,
            ..DamageIn::default()
        };
        for (k, t) in DMG_TYPES.iter().enumerate() {
            d.dmg[k] = o(self, &format!("{t}TakenHit"));
        }
        d.limit_speedup = o(self, "preventedLifeLossTotal") > 0.0;
        let n = self.number_of_hits_to_die(actor, &mut d)?;
        self.c.set_out(actor, "NumberOfDamagingHits", n);

        let mut d = DamageIn {
            cycles: 1.0,
            ..DamageIn::default()
        };
        let mut block = o(self, "EffectiveBlockChance") / 100.0;
        if category != "Melee" && category != "Untyped" {
            block = o(self, &format!("Effective{category}BlockChance")) / 100.0;
        }
        if self.c.flag(edb, None, &["CannotBeBlocked"])? {
            block = 0.0;
        }
        let block_effect = 1.0 - block * o(self, "BlockEffect") / 100.0;
        let mut suppress = 0.0;
        let mut suppress_effect = 1.0;
        let mut extra_avoid = 0.0;
        let mut avg_avoid = 0.0;
        let gain_on_block = !self.input_truthy("DisableEHPGainOnBlock") && n > 1.0;
        if gain_on_block {
            d.life_when_hit = Some(o(self, "LifeOnBlock") * block);
            d.mana_when_hit = Some(o(self, "ManaOnBlock") * block);
            let mut es = o(self, "EnergyShieldOnBlock") * block;
            if category == "Spell" || category == "SpellProjectile" {
                es += o(self, "EnergyShieldOnSpellBlock") * block;
            } else if category == "Average" {
                es += o(self, "EnergyShieldOnSpellBlock") / 2.0 * block;
            }
            d.es_when_hit = Some(es);
        }
        if matches!(category, "Spell" | "SpellProjectile" | "Average") {
            suppress = o(self, "EffectiveSpellSuppressionChance") / 100.0;
        }
        if suppress < 1.0 {
            if category == "Average" {
                suppress /= 2.0;
            }
            d.es_when_hit =
                Some(d.es_when_hit.unwrap_or(0.0) + o(self, "EnergyShieldOnSuppress") * suppress);
            d.life_when_hit =
                Some(d.life_when_hit.unwrap_or(0.0) + o(self, "LifeOnSuppress") * suppress);
            suppress_effect = 1.0 - suppress * o(self, "SpellSuppressionEffect") / 100.0;
        } else {
            let f = if category == "Average" { 0.5 } else { 1.0 };
            d.es_when_hit =
                Some(d.es_when_hit.unwrap_or(0.0) + o(self, "EnergyShieldOnSuppress") * f);
            d.life_when_hit = Some(d.life_when_hit.unwrap_or(0.0) + o(self, "LifeOnSuppress") * f);
        }
        if category == "Projectile" || category == "SpellProjectile" {
            extra_avoid += o(self, "AvoidProjectilesChance");
        } else if category == "Average" {
            extra_avoid += o(self, "AvoidProjectilesChance") / 2.0;
        }
        if gain_on_block {
            let ml = self.c.base(db, None, &["MissingLifeBeforeEnemyHit"])?;
            let mm = self.c.base(db, None, &["MissingManaBeforeEnemyHit"])?;
            d.missing_life = Some(ml);
            d.missing_mana = Some(mm);
            if d.life_when_hit.unwrap_or(0.0) != 0.0
                || d.mana_when_hit.unwrap_or(0.0) != 0.0
                || d.es_when_hit != Some(0.0)
                || ml != 0.0
                || mm != 0.0
            {
                d.gain_when_hit = true;
            }
        } else {
            d.life_when_hit = Some(0.0);
            d.mana_when_hit = Some(0.0);
            d.es_when_hit = Some(0.0);
        }
        let specific = matches!(
            self.c.out_val(actor, "specificTypeAvoidance"),
            Some(OutVal::Bool(true))
        );
        let worst = self.unlucky_worst_of().unwrap_or(1.0);
        for (k, t) in DMG_TYPES.iter().enumerate() {
            let mut avoid = 0.0;
            if specific {
                avoid = m_min(
                    o(self, &format!("Avoid{t}DamageChance")) + extra_avoid,
                    self.misc_data("AvoidChanceCap"),
                );
                if worst > 1.0 {
                    avoid = avoid / 100.0 * avoid;
                    if worst == 4.0 {
                        avoid = avoid / 100.0 * avoid;
                    }
                }
                avg_avoid += avoid;
            }
            d.dmg[k] = o(self, &format!("{t}TakenHit"))
                * (block_effect * suppress_effect * (1.0 - avoid / 100.0));
        }
        if o(self, "anyRecoup") > 0.0 {
            d.track_recoupable = true;
            for t in DMG_TYPES {
                self.c
                    .set_out(actor, &format!("{t}RecoupableDamageTaken"), 0.0);
            }
        }
        if o(self, "preventedLifeLossTotal") > 0.0 {
            d.track_life_loss = true;
            self.c.set_out(actor, "LifeLossLostOverTime", 0.0);
            self.c.set_out(actor, "LifeBelowHalfLossLostOverTime", 0.0);
        }
        d.limit_speedup = d.track_recoupable || d.track_life_loss || d.gain_when_hit;
        avg_avoid /= 5.0;
        let chance = 100.0 * (block_effect * suppress_effect * (1.0 - avg_avoid / 100.0));
        self.c.set_out(actor, "ConfiguredDamageChance", chance);
        let m = if chance != 100.0 || d.track_recoupable || d.track_life_loss || d.gain_when_hit {
            self.number_of_hits_to_die(actor, &mut d)?
        } else {
            n
        };
        self.c.set_out(actor, "NumberOfMitigatedDamagingHits", m);
        let total = m / (1.0 - o(self, "ConfiguredNotHitChance") / 100.0);
        self.c.set_out(actor, "TotalNumberOfHits", total);
        let v = total * o(self, "totalEnemyDamageIn");
        self.c.set_out(actor, "TotalEHP", v);
        let speed = self
            .input
            .config
            .get("enemySpeed")
            .or_else(|| self.input.placeholder.get("enemySpeed"))
            .map_or(700.0, Val::num);
        let mut skill_time = speed / (1.0 + self.c.inc(edb, None, &["Speed"])? / 100.0);
        let enemy_speed = self.action_speed_mod(enemy)?;
        skill_time = skill_time / 1000.0 / enemy_speed;
        self.c.set_out(actor, "enemySkillTime", skill_time);
        self.c.set_out(actor, "EHPSurvivalTime", total * skill_time);
        Ok(())
    }

    fn recoup_estimates(&mut self, actor: usize, category: &str) -> Result<()> {
        let db = self.c.db(actor);
        let o = |p: &Self, k: &str| p.o(actor, k);
        let oo = |p: &Self, k: &str| p.o_or(actor, k, 0.0);
        if o(self, "anyRecoup") > 0.0 && category != "DamageOverTime" {
            let mut total = 0.0;
            let mut total_ele = 0.0;
            let mitigated = o(self, "NumberOfMitigatedDamagingHits")
                * (o(self, "PhysicalTakenDamage") - o(self, "PhysicalTakenHit"));
            let mut extra: Vec<(&str, f64, f64)> = Vec::new();
            for t in DMG_TYPES {
                let r = o(self, &format!("{t}RecoupableDamageTaken"));
                total += r;
                if is_elemental(t) {
                    total_ele += r;
                }
            }
            let types = ["Life", "Mana", "EnergyShield"];
            let survival = o(self, "EHPSurvivalTime");
            for (i, r) in types.iter().enumerate() {
                let time = if self.c.flag(db, None, &[&format!("3Second{r}Recoup")])?
                    || self.c.flag(db, None, &["3SecondRecoup"])?
                {
                    3.0
                } else {
                    4.0
                };
                let mut rec = oo(self, &format!("{r}Recoup")) / 100.0 * total;
                if oo(self, &format!("Elemental{r}Recoup")) > 0.0 && total_ele > 0.0 {
                    rec += o(self, &format!("Elemental{r}Recoup")) / 100.0 * total_ele;
                }
                for t in DMG_TYPES {
                    let x = oo(self, &format!("{t}{r}Recoup"));
                    let taken = o(self, &format!("{t}RecoupableDamageTaken"));
                    if x > 0.0 && taken > 0.0 {
                        rec += x / 100.0 * taken;
                    }
                }
                self.c
                    .set_out(actor, &format!("Total{r}RecoupRecovery"), rec);
                let mut pseudo = oo(self, &format!("PhysicalDamageMitigated{r}PseudoRecoup"))
                    / 100.0
                    * mitigated;
                let pdur = self
                    .c
                    .out(
                        actor,
                        &format!("PhysicalDamageMitigated{r}PseudoRecoupDuration"),
                    )
                    .unwrap_or(4.0);
                if pseudo != 0.0 {
                    for to in &types[i + 1..] {
                        if self
                            .c
                            .flag(db, None, &[&format!("{r}RegenerationRecovers{to}")])?
                            && !self
                                .c
                                .flag(db, None, &[&format!("UnaffectedBy{to}Regen")])?
                            && !self.c.flag(db, None, &[&format!("No{to}Regen")])?
                            && !self.c.flag(db, None, &[&format!("CannotGain{to}")])?
                        {
                            let v = pseudo * o(self, &format!("{to}RecoveryRateMod"))
                                / o(self, &format!("{r}RecoveryRateMod"));
                            extra.retain(|(k, _, _)| k != to);
                            extra.push((to, v, pdur));
                        }
                    }
                }
                if self.c.flag(db, None, &[&format!("UnaffectedBy{r}Regen")])? {
                    pseudo = 0.0;
                }
                self.c
                    .set_out(actor, &format!("Total{r}PseudoRecoup"), pseudo);
                let ex = extra.iter().find(|(k, _, _)| k == r).copied();
                let max = rec / time + pseudo / pdur + ex.map_or(0.0, |(_, a, d)| a / d);
                self.c.set_out(actor, &format!("{r}RecoupRecoveryMax"), max);
                let avg = rec / (survival + time)
                    + pseudo / (survival + pdur)
                    + ex.map_or(0.0, |(_, a, d)| a / (survival + d));
                self.c.set_out(actor, &format!("{r}RecoupRecoveryAvg"), avg);
            }
        }
        let prevented_total = o(self, "preventedLifeLossTotal");
        if prevented_total > 0.0
            && self.c.out(actor, "LifeLossLostOverTime").is_some()
            && self.c.out(actor, "LifeBelowHalfLossLostOverTime").is_some()
        {
            let lost = self.c.base(db, None, &["LifeLossBelowHalfLost"])? / 100.0;
            let x =
                o(self, "LifeLossLostOverTime") + o(self, "LifeBelowHalfLossLostOverTime") * lost;
            self.c.set_out(actor, "LifeLossLostMax", x / 4.0);
            self.c.set_out(
                actor,
                "LifeLossLostAvg",
                x / (o(self, "EHPSurvivalTime") + 4.0),
            );
        }
        if oo(self, "LifeRecoupRecoveryAvg") > 0.0 || prevented_total > 0.0 {
            let v = oo(self, "LifeRecoupRecoveryMax") - oo(self, "LifeLossLostMax");
            self.c
                .set_out(actor, "netLifeRecoupAndLossLostOverTimeMax", v);
            let v = oo(self, "LifeRecoupRecoveryAvg") - oo(self, "LifeLossLostAvg");
            self.c
                .set_out(actor, "netLifeRecoupAndLossLostOverTimeAvg", v);
            if oo(self, "LifeRecoupRecoveryAvg") > 0.0 && prevented_total > 0.0 {
                self.c
                    .set_out_val(actor, "showNetRecoup", OutVal::Bool(true));
            }
        }
        Ok(())
    }

    fn pvp(&mut self, actor: usize) -> Result<()> {
        if !self.input_truthy("PvpScaling") {
            return Ok(());
        }
        let o = |p: &Self, k: &str| p.o(actor, k);
        let t = o(self, "enemySkillTime");
        let mult = self.input_num("enemyMultiplierPvpDamage").unwrap_or(100.0) / 100.0;
        let (ne1, ne2, e1, e2) = (
            self.misc_data("PvpNonElemental1"),
            self.misc_data("PvpNonElemental2"),
            self.misc_data("PvpElemental1"),
            self.misc_data("PvpElemental2"),
        );
        let total = o(self, "totalTakenHit");
        let pne = (o(self, "PhysicalTakenHit") + o(self, "ChaosTakenHit")) / total;
        let pe = 1.0 - pne;
        let portion_ne = (total / t / ne2).powf(ne1) * t * ne2 * pne;
        let portion_e = (total / t / e2).powf(e1) * t * e2 * pe;
        self.c
            .set_out(actor, "PvPTotalTakenHit", (portion_ne + portion_e) * mult);
        Ok(())
    }

    fn maximum_hit(
        &mut self,
        actor: usize,
        shift: &[(&'static str, Vec<(&'static str, f64)>)],
    ) -> Result<()> {
        let db = self.c.db(actor);
        let o = |p: &Self, k: &str| p.o(actor, k);
        let ward_bypass = self.c.base(db, None, &["WardBypass"])?;
        for t in DMG_TYPES {
            let k = format!("{t}TotalHitPool");
            if ward_bypass > 0.0 {
                let protected =
                    o(self, "Ward") / (1.0 - ward_bypass / 100.0) * (ward_bypass / 100.0);
                let src = o(self, &k);
                let v = m_max(src - protected, 0.0) + m_min(src, protected) / (ward_bypass / 100.0);
                self.c.set_out(actor, &k, v);
            } else {
                let v = o(self, &k) + o(self, "Ward");
                self.c.set_out(actor, &k, v);
            }
            let v = o(self, &k)
                + m_max(
                    m_max(o(self, &format!("{t}Aegis")), o(self, "sharedAegis")),
                    if is_elemental(t) {
                        o(self, &format!("{t}AegisDisplay"))
                    } else {
                        0.0
                    },
                );
            self.c.set_out(actor, &k, v);
            // `output.sharedGuardAbsorbRate or 0 + ...`: the shared rate alone
            let rate = self.o_or(actor, "sharedGuardAbsorbRate", 0.0);
            if rate > 0.0 {
                let absorb = self
                    .c
                    .out(actor, "sharedGuardAbsorb")
                    .unwrap_or_else(|| self.o_or(actor, &format!("{t}GuardAbsorb"), 0.0));
                if rate >= 100.0 {
                    let v = o(self, &k) + absorb;
                    self.c.set_out(actor, &k, v);
                } else {
                    let protected = absorb / (rate / 100.0) * (1.0 - rate / 100.0);
                    let p = o(self, &k);
                    let v = m_max(p - protected, 0.0) + m_min(p, protected) / (1.0 - rate / 100.0);
                    self.c.set_out(actor, &k, v);
                }
            }
            for a in ALLY_POOLS.iter().rev() {
                let (Some(life), Some(m)) =
                    (self.c.out(actor, a.life), self.c.out(actor, a.mitigation))
                else {
                    continue;
                };
                if life > 0.0 && m > 0.0 {
                    let m = m_min(m, 100.0);
                    let p = o(self, &k);
                    let v = if m == 100.0 {
                        p + life
                    } else {
                        let protected = life / (m / 100.0) * (1.0 - m / 100.0);
                        m_max(p - protected, 0.0) + m_min(p, protected) / (1.0 - m / 100.0)
                    };
                    self.c.set_out(actor, &k, v);
                }
            }
        }
        let dr_max = o(self, "DamageReductionMax");
        for (t, st) in shift {
            let mut part_min = f64::INFINITY;
            let mut smoothing = false;
            for c in DMG_TYPES {
                let convert = st.iter().find(|(k, _)| *k == c).map_or(0.0, |(_, v)| *v);
                let taken_flat = o(self, &format!("{c}takenFlat"));
                if convert > 0.0 || taken_flat != 0.0 {
                    let applied = o(self, &format!("{c}EffectiveAppliedArmour"));
                    let conv_multi = convert / 100.0;
                    let pool = o(self, &format!("{c}TotalHitPool"));
                    let taken_multi = o(self, &format!("{c}AfterReductionTakenHitMulti"))
                        * (1.0 - o(self, "VaalArcticArmourMitigation"));
                    let hit_taken;
                    if conv_multi <= 0.0 {
                        let without = m_max(taken_flat, 0.0) * taken_multi;
                        hit_taken = if without >= pool { 0.0 } else { f64::INFINITY };
                    } else if applied == 0.0 && convert == 100.0 {
                        let resist_mult = o(self, &format!("{c}ResistTakenHitMulti"));
                        let dr_multi =
                            resist_mult * (1.0 - o(self, &format!("{c}DamageReduction")) / 100.0);
                        hit_taken =
                            m_max(pool / conv_multi / dr_multi - taken_flat, 0.0) / taken_multi;
                    } else {
                        let resist_mult = o(self, &format!("{c}ResistTakenHitMulti"));
                        let reduction = if self.c.flag(
                            db,
                            None,
                            &[&format!("SelfIgnoreBase{c}DamageReduction")],
                        )? {
                            0.0
                        } else {
                            self.c
                                .out(actor, &format!("Base{c}DamageReductionWhenHit"))
                                .unwrap_or_else(|| o(self, &format!("Base{c}DamageReduction")))
                        };
                        let flat_dr = reduction / 100.0;
                        let overwhelm =
                            if self
                                .c
                                .flag(db, None, &[&format!("SelfIgnore{c}DamageReduction")])?
                            {
                                0.0
                            } else {
                                o(self, &format!("{c}EnemyOverwhelm"))
                            };
                        let rxc = resist_mult * conv_multi;
                        let a = 5.0 * (1.0 - flat_dr + overwhelm / 100.0) * taken_multi * rxc * rxc;
                        let b = ((overwhelm / 100.0 - flat_dr) * applied * taken_multi
                            - 5.0 * (pool - taken_flat * taken_multi))
                            * rxc;
                        let cc = -applied * (pool - taken_flat * taken_multi);
                        let raw = if a != 0.0 {
                            (m_max(b * b - 4.0 * a * cc, 0.0).sqrt() - b) / (2.0 * a)
                        } else {
                            f64::INFINITY
                        };
                        let no_dr = pool / conv_multi / resist_mult / taken_multi
                            * (1.0 - taken_flat * taken_multi / pool);
                        let max_dr = no_dr / (1.0 - (dr_max - overwhelm) / 100.0);
                        hit_taken = m_max(m_min(raw, max_dr), no_dr).floor();
                        smoothing = smoothing || convert != 100.0;
                    }
                    part_min = m_min(part_min, hit_taken);
                }
            }
            let enemy_mult = o(self, &format!("{t}EnemyDamageMult"));
            let final_max = if part_min == f64::INFINITY {
                f64::INFINITY
            } else if smoothing {
                let mut incoming = part_min;
                let mut previous: Option<f64> = None;
                let passes = self.misc_data("maxHitSmoothingPasses") as i64;
                for n in 1..=passes {
                    let (_, damages) = self.taken_hit_from_damage(actor, incoming, t, st)?;
                    let pools = self.reduce_pools(actor, None, &damages)?;
                    let overkill = pools.overkill - pools.hit_pool_remaining;
                    let mut ratio: f64 = 0.0;
                    for (pt, _) in &damages {
                        let pp = self.o_or(actor, &format!("{pt}TotalHitPool"), 0.0);
                        if pp > 0.0 {
                            ratio = m_max(ratio, (overkill + pp) / pp);
                        }
                    }
                    if ratio <= 0.0 {
                        ratio = 1.0;
                    }
                    let mut step = 1.0;
                    if n > 1
                        && let Some(prev) = previous
                        && prev != 0.0
                        && !prev.is_nan()
                    {
                        step = m_min(((overkill - prev) / prev).abs(), 2.0);
                    }
                    let adjust = if step > 1.0 {
                        -overkill / step
                    } else if n > 1 {
                        -overkill * step
                    } else {
                        0.0
                    };
                    previous = Some(overkill);
                    incoming = (incoming + adjust) / ratio.sqrt();
                    if overkill < 1.0 && overkill > -1.0 {
                        break;
                    }
                }
                round(incoming / enemy_mult, 0.0)
            } else {
                round(part_min / enemy_mult, 0.0)
            };
            self.c
                .set_out(actor, &format!("{t}MaximumHitTaken"), final_max);
        }
        let mut minimum = f64::INFINITY;
        let mut second = f64::INFINITY;
        for t in DMG_TYPES {
            let v = o(self, &format!("{t}MaximumHitTaken"));
            if v < minimum {
                second = minimum;
                minimum = v;
            } else if v < second {
                second = v;
            }
        }
        self.c
            .set_out(actor, "SecondMinimalMaximumHitTaken", second);
        Ok(())
    }

    fn degens(
        &mut self,
        actor: usize,
        category: &str,
        dot_shift: &[(&'static str, Vec<(&'static str, f64)>)],
    ) -> Result<()> {
        let db = self.c.db(actor);
        let enemy = self.c.actors.actors[actor].enemy.ok_or("no enemy")?;
        let edb = self.c.db(enemy);
        let o = |p: &Self, k: &str| p.o(actor, k);
        let mut total_build = 0.0;
        for (t, ds) in dot_shift {
            let base = self.c.base(db, None, &[&format!("{t}Degen")])?;
            if base > 0.0 {
                for &(c, pct) in ds {
                    if pct > 0.0 {
                        let total = base * (pct / 100.0) * o(self, &format!("{c}TakenDotMult"));
                        let k = format!("{c}BuildDegen");
                        let v = self.o_or(actor, &k, 0.0) + total;
                        self.c.set_out(actor, &k, v);
                        total_build += total;
                    }
                }
            }
        }
        let es_protects = self.c.flag(db, None, &["EnergyShieldProtectsMana"])?;
        let split = |p: &Self, t: &str, degen: f64| -> (f64, f64, f64) {
            let from_mana = o(p, &format!("{t}MindOverMatter")) + o(p, "sharedMindOverMatter");
            let bypass = o(p, &format!("{t}EnergyShieldBypass"));
            if o(p, "EnergyShieldRegenRecovery") > 0.0 {
                let (life, es) = if es_protects {
                    (
                        degen * (1.0 - from_mana / 100.0),
                        degen * (1.0 - bypass / 100.0) * (from_mana / 100.0),
                    )
                } else {
                    (
                        degen * (bypass / 100.0) * (1.0 - from_mana / 100.0),
                        degen * (1.0 - bypass / 100.0),
                    )
                };
                (life, degen * (bypass / 100.0) * (from_mana / 100.0), es)
            } else {
                (
                    degen * (1.0 - from_mana / 100.0),
                    degen * (from_mana / 100.0),
                    0.0,
                )
            }
        };
        if total_build == 0.0 {
            self.c.clear_out(actor, "TotalBuildDegen");
        } else {
            self.c.set_out(actor, "TotalBuildDegen", total_build);
            let (mut tl, mut tm, mut te) = (0.0, 0.0, 0.0);
            for t in DMG_TYPES {
                if let Some(d) = self.c.out(actor, &format!("{t}BuildDegen")) {
                    let (l, m, e) = split(self, t, d);
                    tl += l;
                    tm += m;
                    te += e;
                }
            }
            let nl = o(self, "LifeRegenRecovery") - tl;
            let nm = o(self, "ManaRegenRecovery") - tm;
            let ne = o(self, "EnergyShieldRegenRecovery") - te;
            self.c.set_out(actor, "NetLifeRegen", nl);
            self.c.set_out(actor, "NetManaRegen", nm);
            self.c.set_out(actor, "NetEnergyShieldRegen", ne);
            self.c.set_out(actor, "TotalNetRegen", nl + nm + ne);
        }
        let crit_ailment = 1.0
            + self.o_or(actor, "EnemyCritChance", 0.0) / 100.0
                * 0.5
                * (1.0 - o(self, "CritExtraDamageReduction") / 100.0);
        let threshold = 10.1;
        let bleed = 0.0;
        let mut ignite = 0.0;
        if o(self, "SelfIgniteEffect") != 0.0
            && o(self, "IgniteAvoidChance") < 100.0
            && o(self, "SelfIgniteDuration") != 0.0
            && category != "DamageOverTime"
        {
            ignite = self
                .c
                .base(edb, None, &["IgniteChance", "ElementalAilmentChance"])?;
            let crit_chance =
                if !self
                    .c
                    .flag(db, None, &["CritsOnYouDontAlwaysApplyElementalAilments"])?
                {
                    if o(self, "EnemyCritChance") > threshold || ignite > 0.0 {
                        o(self, "EnemyCritChance")
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
            ignite = (crit_chance + (1.0 - crit_chance / 100.0) * ignite)
                * (1.0 - o(self, "IgniteAvoidChance") / 100.0);
        }
        let mut poison = 0.0;
        if o(self, "SelfPoisonEffect") != 0.0
            && o(self, "PoisonAvoidChance") < 100.0
            && o(self, "SelfPoisonDuration") != 0.0
            && category != "DamageOverTime"
        {
            poison = self.c.base(edb, None, &["PoisonChance"])?
                * (1.0 - o(self, "PoisonAvoidChance") / 100.0);
        }
        if category == "DamageOverTime" || ignite + poison + bleed > 0.0 {
            let build = self.c.out(actor, "TotalBuildDegen").unwrap_or(0.0);
            let mut total = build;
            let dot_of = |t: &str| -> Vec<(&'static str, f64)> {
                dot_shift
                    .iter()
                    .find(|(k, _)| *k == t)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            if category == "DamageOverTime" {
                for t in DMG_TYPES {
                    let base = self
                        .input_num(&format!("enemy{t}Damage"))
                        .or_else(|| {
                            Some(
                                self.placeholder_num(&format!("enemy{t}Damage"))
                                    .unwrap_or(0.0),
                            )
                        })
                        .unwrap_or(0.0);
                    if base > 0.0 {
                        for (c, pct) in dot_of(t) {
                            if pct > 0.0 {
                                let x = base * (pct / 100.0) * o(self, &format!("{c}TakenDotMult"));
                                let k = format!("{c}EnemyDegen");
                                let v = self.o_or(actor, &k, 0.0) + x;
                                self.c.set_out(actor, &k, v);
                                total += x;
                            }
                        }
                    }
                }
            } else {
                // Bleed, ignite, poison in the order the table was built
                let mut list: Vec<(&str, &str, Vec<&str>)> = Vec::new();
                if ignite > 0.0 {
                    let sources = if self.c.flag(edb, None, &["AllDamageIgnites"])? {
                        DMG_TYPES.to_vec()
                    } else {
                        vec!["Fire"]
                    };
                    list.push(("Ignite", "Fire", sources));
                }
                if poison > 0.0 {
                    list.push(("Poison", "Chaos", vec!["Physical", "Chaos"]));
                }
                for (source, dt, sources) in list {
                    let mut base = 0.0;
                    for s in sources {
                        base += o(self, &format!("{s}TakenDamage"));
                    }
                    base = base
                        * self.misc_data(&format!("{source}PercentBase"))
                        * (crit_ailment / o(self, "EnemyCritEffect"))
                        * o(self, &format!("Self{source}Effect"))
                        / 100.0;
                    if base > 0.0 {
                        for (c, pct) in dot_of(dt) {
                            if pct > 0.0 {
                                let x = base * (pct / 100.0) * o(self, &format!("{c}TakenDotMult"));
                                let k = format!("{c}EnemyDegen");
                                let v = self.o_or(actor, &k, 0.0) + x;
                                self.c.set_out(actor, &k, v);
                                total += x;
                            }
                        }
                    }
                }
            }
            if total == build {
                self.c.clear_out(actor, "TotalDegen");
            } else {
                self.c.set_out(actor, "TotalDegen", total);
                let (mut tl, mut tm, mut te) = (0.0, 0.0, 0.0);
                for t in DMG_TYPES {
                    let d = self.o_or(actor, &format!("{t}BuildDegen"), 0.0)
                        + self.o_or(actor, &format!("{t}EnemyDegen"), 0.0);
                    if d != 0.0 {
                        let (l, m, e) = split(self, t, d);
                        tl += l;
                        tm += m;
                        te += e;
                    }
                }
                let nl = o(self, "LifeRegenRecovery")
                    + self.o_or(actor, "LifeRecoupRecoveryAvg", 0.0)
                    - tl
                    - self.o_or(actor, "LifeLossLostAvg", 0.0);
                let nm = o(self, "ManaRegenRecovery")
                    + self.o_or(actor, "ManaRecoupRecoveryAvg", 0.0)
                    - tm;
                let ne = o(self, "EnergyShieldRegenRecovery")
                    + self.o_or(actor, "EnergyShieldRecoupRecoveryAvg", 0.0)
                    - te;
                self.c.set_out(actor, "ComprehensiveNetLifeRegen", nl);
                self.c.set_out(actor, "ComprehensiveNetManaRegen", nm);
                self.c
                    .set_out(actor, "ComprehensiveNetEnergyShieldRegen", ne);
                self.c
                    .set_out(actor, "ComprehensiveTotalNetRegen", nl + nm + ne);
            }
        }
        Ok(())
    }
}
