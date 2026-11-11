//! The skills' own offence functions (`initialFunc`, `preDamageFunc`, ...
//! of `Data/Skills/*.lua`), ported by hand per granted effect id.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::calc::{m_max, m_min};
use crate::calc_offence::Off;
use crate::calc_perform::Perform;
use crate::mods::tag::TagKind;
use crate::mods::{Mod, ModType, ModValue, Tag};

pub type Result<T> = std::result::Result<T, String>;

impl<'a> Perform<'a> {
    /// Runs the function `name` of the granted effect `id`
    pub(crate) fn skill_func(&mut self, o: &mut Off, id: &str, name: &str) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let sd = |p: &Self, k: &str| p.sd_num(i, k).unwrap_or(f64::NAN);
        let part = self.mods(i).skill_part;
        let out_req = |p: &Self, k: &str| -> Result<f64> {
            p.c.out(actor, k).ok_or_else(|| {
                format!("attempt to perform arithmetic on a nil value (field '{k}')")
            })
        };
        // `repeatFrequency / (1 + inc Speed, BrandActivationFrequency) /
        // more BrandActivationFrequency`
        let brand_time = |p: &Self| -> Result<f64> {
            Ok(sd(p, "repeatFrequency")
                / (1.0 + p.c.inc(list, c, &["Speed", "BrandActivationFrequency"])? / 100.0)
                / p.c.more(list, c, &["BrandActivationFrequency"])?)
        };
        match (id, name) {
            (
                "ArmageddonBrand"
                | "ArmageddonBrandAltY"
                | "BrandSupport"
                | "PenanceBrandAltX"
                | "PenanceBrandAltY"
                | "StormBrand"
                | "StormBrandAltX",
                "preDamageFunc",
            ) => {
                let v = brand_time(self)?;
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("ChargedDash", "preDamageFunc") => {
                if part == Some(3.0) {
                    let v = self.c.inc(list, c, &["chargedDashFinalDamageModifier"])?;
                    let tag = Tag::other("Release Damage", [("skillPart", ModValue::Number(3.0))]);
                    let m = Mod::new("Damage", ModType::More, v, Some("Skill:ChargedDash"))
                        .flags(self.mf("Attack"), 0)
                        .tag(tag);
                    self.c.add(list, m);
                }
            }
            ("ExplosiveTrap" | "ExplosiveTrapAltX", "preDamageFunc") => {
                let enemy_radius = match self.c.override_num(list, c, &["EnemyRadius"])? {
                    Some(v) => v,
                    None => self.c.base(list, c, &["EnemyRadius"])?,
                };
                let full = self
                    .c
                    .out(actor, "AreaOfEffectRadiusSecondary")
                    .unwrap_or(f64::NAN);
                let width = sd(self, "radiusTertiaryBaseMargin") * 2.0 + 1.0;
                let occurrences =
                    match self
                        .c
                        .out_val(actor, "AreaOfEffectRadiusTertiaryOccurrences")
                    {
                        Some(crate::actor::OutVal::Table(t)) => t.clone(),
                        _ => return Err(
                            "attempt to index a nil value (AreaOfEffectRadiusTertiaryOccurrences)"
                                .into(),
                        ),
                    };
                let mut overlap = 0.0;
                let mut radii: Vec<(f64, f64)> = occurrences
                    .iter()
                    .filter_map(|(k, v)| Some((k.parse::<f64>().ok()?, v.num()?)))
                    .collect();
                radii.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (small, count) in radii {
                    let damaging = small + enemy_radius - 1.0;
                    let chance = m_min(damaging * damaging / (full * full), 1.0);
                    overlap += chance * count / width;
                }
                self.c.set_out(actor, "OverlapChance", overlap * 100.0);
                let small = self.c.base(list, c, &["SmallExplosions"])?;
                self.c.set_out(actor, "SmallExplosionsPerTrap", small);
                let mult = match part {
                    Some(2.0) => 1.0 + small * overlap,
                    Some(3.0) => 1.0 + small,
                    _ => 1.0,
                };
                if mult != 1.0 {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * mult);
                    let cur = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0);
                    self.c.set_out(actor, "SkillDPSMultiplier", cur * mult);
                }
            }
            ("Cyclone" | "VaalCyclone" | "CycloneAltX", "initialFunc") => {
                let w = |p: &Self, n: usize, k: &str| {
                    p.weapon(actor, n)
                        .and_then(|w| w.get(k))
                        .and_then(crate::mods::ModValue::as_number)
                };
                let wcfg = |p: &Self, n: usize| p.mods(i).weapon_cfgs[n].clone();
                let range_of = |p: &Self, n: usize| -> Result<f64> {
                    let cfg = wcfg(p, n);
                    Ok(p.c.base(list, cfg.as_ref(), &["MeleeWeaponRange"])?
                        + 10.0 * p.c.base(list, cfg.as_ref(), &["MeleeWeaponRangeMetre"])?
                        + w(p, n, "rangeBonus").unwrap_or(f64::NAN))
                };
                let range = if self.has_flag(i, "weapon1Attack") && w(self, 0, "range").is_some() {
                    let r1 = range_of(self, 0)?;
                    if self.has_flag(i, "weapon2Attack") && w(self, 1, "range").is_some() {
                        (r1 + range_of(self, 1)?) / 2.0
                    } else {
                        r1
                    }
                } else {
                    self.c.base(list, c, &["UnarmedRange"])?
                        + 10.0 * self.c.base(list, c, &["UnarmedRangeMetre"])?
                };
                self.c.add(
                    list,
                    Mod::new(
                        "Multiplier:AdditionalMeleeRange",
                        ModType::Base,
                        range,
                        Some(if id == "CycloneAltX" {
                            "Skill:CycloneAltX"
                        } else {
                            "Skill:Cyclone"
                        }),
                    ),
                );
            }
            ("IceSpear" | "IceSpearAltX", "preDamageFunc") => {
                if part == Some(3.0) || part == Some(4.0) {
                    let count = self.c.out(actor, "ProjectileCount").ok_or(
                        "attempt to perform arithmetic on a nil value (field 'ProjectileCount')",
                    )?;
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * count);
                }
            }
            ("StormRainAltY", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = sd(self, "hitFrequency")
                        / (1.0 + self.c.inc(list, c, &["StormRainBeamFrequency"])? / 100.0);
                    self.sd_set(i, "hitTimeOverride", v);
                    let m = self.c.base(list, c, &["StormRainAllowedStormArrows"])?;
                    self.sd_set(i, "dpsMultiplier", m);
                }
            }
            ("RighteousFire", "preDamageFunc") => {
                let o = |p: &Self, k: &str| out_req(p, k);
                if self.has_flag(i, "totem") && o(self, "TotemLife")? > 1.0 {
                    let v = o(self, "TotemLife")? * sd(self, "RFLifeMultiplier")
                        + o(self, "TotemEnergyShield")? * sd(self, "RFESMultiplier");
                    self.sd_set(i, "FireDot", v);
                } else if o(self, "LifeUnreserved")? > 1.0 {
                    let v = o(self, "Life")? * sd(self, "RFLifeMultiplier")
                        + o(self, "EnergyShield")? * sd(self, "RFESMultiplier");
                    self.sd_set(i, "FireDot", v);
                }
            }
            ("Spark" | "SparkAltY" | "SparkAltX", "preDamageFunc") => {
                if part == Some(2.0) {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    let v = cur * (1.0 + (out_req(self, "Duration")? / 0.66).floor());
                    self.sd_set(i, "dpsMultiplier", v);
                    self.c.set_out(actor, "SkillDPSMultiplier", v);
                }
            }
            ("StormBurst" | "StormBurstAltX", "preDamageFunc") => {
                let dm = out_req(self, "DurationMod");
                if part == Some(2.0) && id == "StormBurst" {
                    let duration = sd(self, "duration") * dm?;
                    let jump = sd(self, "repeatFrequency") * 10.0;
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(
                        i,
                        "dpsMultiplier",
                        cur * (1.0 + (duration * 10.0 / jump).floor()),
                    );
                } else if (part == Some(3.0) && id == "StormBurst")
                    || (part == Some(2.0) && id == "StormBurstAltX")
                {
                    let jumps = ((sd(self, "duration") * dm? + 0.001)
                        / sd(self, "repeatFrequency"))
                    .floor();
                    let v = m_max(sd(self, "remainingDurationDamage") * jumps - 100.0, 0.0);
                    self.c.add(
                        list,
                        Mod::new("Damage", ModType::More, v, Some("Skill:StormBurst")),
                    );
                }
            }
            ("DeathWish", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = out_req(self, "Life")? * sd(self, "selfFireExplosionLifeMultiplier");
                    self.sd_set(i, "FireBonusMin", v);
                    self.sd_set(i, "FireBonusMax", v);
                }
            }
            ("Bodyswap" | "BodyswapAltX", "preDamageFunc") => {
                if part == Some(1.0) {
                    let pool = if self.has_flag(i, "totem") {
                        "TotemLife"
                    } else {
                        "Life"
                    };
                    let v = out_req(self, pool)? * sd(self, "selfFireExplosionLifeMultiplier");
                    self.sd_set(i, "FireBonusMin", v);
                    self.sd_set(i, "FireBonusMax", v);
                }
            }
            ("LancingSteel" | "LancingSteelAltX", "preDamageFunc") => {
                if part == Some(2.0) {
                    let count = out_req(self, "ProjectileCount")?;
                    let reduced = (count - 1.0) / count;
                    let mult = (self.c.more(list, c, &["LancingSteelSubsequentDamage"])? - 1.0)
                        * 100.0
                        * reduced;
                    self.sd_set(i, "dpsMultiplier", count);
                    self.c.add(
                        list,
                        Mod::new("Damage", ModType::More, mult, Some(&format!("Skill:{id}"))),
                    );
                }
            }
            ("ShrapnelBallista" | "ShrapnelBallistaAltX", "preDamageFunc") => {
                if !self.c.flag(list, None, &["SequentialProjectiles"])? {
                    let count = out_req(self, "ProjectileCount")?;
                    let overlap = self
                        .sd_num(i, "ShrapnelBallistaProjectileOverlap")
                        .unwrap_or(if self.has_type(i, "Rain") { count } else { 1.0 });
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * m_min(overlap, count));
                }
                let split = self.c.out(actor, "SplitCount").unwrap_or(0.0);
                if split > 0.0 {
                    let m = Mod::new("DPS", ModType::More, split * 100.0, Some("Split Return"))
                        .tag(Tag::condition("ReturningProjectile"));
                    self.c.add(list, m);
                }
            }
            ("LightningTendrilsAltX", "preDamageFunc") => {
                let interval = sd(self, "pulseInterval");
                let part_tag = |n: f64| {
                    Tag::kind(TagKind::SkillPart {
                        list: None,
                        part: Some(n),
                        neg: false,
                    })
                };
                if part == Some(2.0) {
                    let m = Mod::new(
                        "DPS",
                        ModType::More,
                        -(1.0 / interval) * 100.0,
                        Some("Normal pulse"),
                    )
                    .tag(part_tag(2.0));
                    self.c.add(list, m);
                } else if part == Some(3.0) {
                    let m = Mod::new(
                        "DPS",
                        ModType::More,
                        -(interval - 1.0) / interval * 100.0,
                        Some("Stronger pulse"),
                    )
                    .tag(part_tag(3.0));
                    self.c.add(list, m);
                }
            }
            ("LightningTendrilsAltX", "postCritFunc") => {
                if part == Some(1.0) {
                    let interval = sd(self, "pulseInterval");
                    let pulse = sd(self, "pulseDamage") / 100.0;
                    let crit = out_req(self, "PreEffectiveCritChance")? / 100.0;
                    let eff = out_req(self, "CritChance")? / 100.0;
                    let multi = out_req(self, "CritMultiplier")?;
                    let average = 100.0
                        * (((interval - 1.0) * (1.0 + crit * (multi - 1.0))
                            + (1.0 + pulse) * multi)
                            / (interval * ((1.0 - eff) + multi * eff))
                            - 1.0);
                    let kw = self.kf("Hit") | self.kf("Ailment");
                    let m = Mod::new(
                        "Damage",
                        ModType::More,
                        average,
                        Some("Average Pulse Damage"),
                    )
                    .flags(0, kw)
                    .tag(Tag::kind(TagKind::SkillPart {
                        list: None,
                        part: Some(1.0),
                        neg: false,
                    }));
                    self.c.add(list, m);
                }
            }
            ("InfernalBlow" | "InfernalBlowAltX", "preDamageFunc") => {
                let effect = self.c.base(list, c, &["DebuffEffect"])?;
                if part == Some(2.0) || part == Some(3.0) {
                    let m = Mod::new(
                        "Damage",
                        ModType::More,
                        effect,
                        Some(&format!("Skill:{id}")),
                    )
                    .tag(Tag::multiplier("DebuffStack").base(-100.0 + effect));
                    self.c.add(list, m);
                }
                if part == Some(3.0) {
                    self.sd_set(i, "dpsMultiplier", 1.0 / 6.0);
                }
            }
            ("BloodSacramentUnique", "initialFunc") => {
                if out_req(self, "LifeReservedPercent")? < 100.0 {
                    let pct = self.sd_num(i, "LifeReservedPercent").unwrap_or(3.0);
                    let base = self.sd_num(i, "LifeReservedBase").unwrap_or(f64::INFINITY);
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:ChannelledLifeReservedPercentPerStage",
                            ModType::Base,
                            pct,
                            Some("Blood Sacrament"),
                        ),
                    );
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:ChannelledLifeReservedPerStage",
                            ModType::Base,
                            base,
                            Some("Blood Sacrament"),
                        ),
                    );
                }
            }
            ("AvengingFlame", "preDamageFunc") => {
                let life = match (self.sd(i, "triggerSourceUUID"), self.input.recalc) {
                    (Some(ModValue::Str(u)), Some(r)) => {
                        r.cached(u).and_then(|c| c.out("TotemLife")).unwrap_or(0.0)
                    }
                    _ => 0.0,
                };
                let add = life * sd(self, "lifeDealtAsFire") / 100.0;
                for k in ["FireMax", "FireMin"] {
                    let v = self.sd_num(i, k).unwrap_or(0.0) + add;
                    self.sd_set(i, k, v);
                }
            }
            ("RageVortex", "preSkillTypeFunc") => {
                if part == Some(2.0) {
                    let max_rage = self.c.base(list, c, &["MaximumRage"])?;
                    let pct = sd(self, "MaxRageVortexSacrificePercentage") / 100.0;
                    let over = self.c.base(list, c, &["Multiplier:RageSacrificedStacks"])?;
                    let max = (pct * max_rage).floor();
                    let stacks = m_min(if over > 0.0 { over } else { max }, max);
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:RageSacrificed",
                            ModType::Base,
                            stacks,
                            Some("Skill:RageVortex"),
                        ),
                    );
                }
            }
            ("TornadoShot", "preDamageFunc") => {
                if part == Some(2.0) && self.c.out(actor, "ReturnChance").unwrap_or(0.0) == 0.0 {
                    let split = self.c.out(actor, "SplitCount").unwrap_or(0.0);
                    let mut secondary = out_req(self, "ProjectileCount")? + split;
                    let f = |p: &Self, n: &str| p.c.flag(list, None, &[n]);
                    if f(self, "SequentialProjectiles")?
                        && !f(self, "OneShotProj")?
                        && !f(self, "NoAdditionalProjectiles")?
                        && !f(self, "SingleProjectile")?
                        && !f(self, "TriggeredBySnipe")?
                    {
                        secondary = 1.0 + split;
                    }
                    let chance = match self.sd_num(i, "tornadoShotSecondaryHitChance") {
                        Some(v) => v,
                        None => {
                            20.0 * self.c.base(list, c, &["tornadoShotSecondaryProjectiles"])?
                        }
                    };
                    let chance = m_min(chance / 100.0, 1.0);
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * (1.0 + chance * secondary));
                }
            }
            ("Bane" | "BaneAltX", "preSkillTypeFunc") => {
                let limit = self.c.out(actor, "EnemyCurseLimit");
                let group = self.skill(i).group;
                let mut count = 0.0;
                for j in self.actor_skills(actor) {
                    if self.skill(j).group == group
                        && self
                            .c
                            .get_condition(self.lists[j].list, "AppliedByBane", None)?
                    {
                        count += 1.0;
                        if Some(count) == limit {
                            break;
                        }
                    }
                }
                self.c.add(
                    list,
                    Mod::new(
                        "Multiplier:CurseApplied",
                        ModType::Base,
                        count,
                        Some("Base"),
                    ),
                );
            }
            ("DarkPact", "preDamageFunc") => {
                let life = if part == Some(1.0) {
                    let pool = if self.has_flag(i, "totem") {
                        "TotemLife"
                    } else {
                        "Life"
                    };
                    out_req(self, pool)?
                } else {
                    self.sd_num(i, "skeletonLife").unwrap_or(0.0)
                };
                let add = life * sd(self, "lifeDealtAsChaos") / 100.0;
                for k in ["ChaosMin", "ChaosMax"] {
                    let v = sd(self, k) + add;
                    self.sd_set(i, k, v);
                }
            }
            ("DarkPactAltX", "preDamageFunc") => {
                let pool = if self.has_flag(i, "totem") {
                    "TotemLife"
                } else {
                    "Life"
                };
                let add = out_req(self, pool)? * sd(self, "percentageLifeSacrificed") / 100.0
                    * (sd(self, "percentageSacrificedDealtAsChaos") / 100.0);
                let add = if part == Some(2.0) {
                    add.floor()
                } else {
                    let casts = 7.0
                        / (1.0
                            + m_min(100.0, self.sd_num(i, "additionalRuinChance").unwrap_or(0.0))
                                / 100.0);
                    (add / casts).floor()
                };
                for k in ["ChaosMin", "ChaosMax"] {
                    let v = sd(self, k) + add;
                    self.sd_set(i, k, v);
                }
            }
            ("FlamethrowerTrap" | "FlamethrowerTrapAltX", "preDamageFunc") => {
                let average = out_req(self, "Duration")? / out_req(self, "TrapCooldown")?;
                self.c.set_out(actor, "AverageActiveTraps", average);
                let alt = id == "FlamethrowerTrapAltX";
                let slow = !alt && matches!(part, Some(p) if p == 2.0 || p == 4.0);
                self.sd_set(i, "hitTimeOverride", if slow { 0.3 } else { 0.1 });
                if alt || matches!(part, Some(p) if p == 3.0 || p == 4.0) {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * average);
                }
            }
            ("BallLightningAltY" | "BallLightningAltX", "preDamageFunc") => {
                if id == "BallLightningAltY" || matches!(part, Some(p) if p == 2.0 || p == 3.0) {
                    let strikes = (sd(self, "duration") / sd(self, "strikeInterval")).floor();
                    if id == "BallLightningAltY" || strikes != 1.0 {
                        let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                        self.sd_set(i, "dpsMultiplier", cur * strikes);
                        let cur = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0);
                        self.c.set_out(actor, "SkillDPSMultiplier", cur * strikes);
                    }
                    self.c.set_out(actor, "NormalHitsPerCast", strikes);
                }
            }
            ("FrozenSweep" | "FrozenSweepAltX", "preDamageFunc") => {
                self.sd_set(i, "showAverage", false);
                self.set_flag(i, "showAverage", false);
                self.set_flag(i, "notAverage", true);
                let name = if id == "FrozenSweep" {
                    "Frozen Legion"
                } else {
                    "Frozen Legion of Rallying"
                };
                let slot_of = |p: &Self, k: usize| {
                    p.skill(k)
                        .group
                        .and_then(|g| p.env.groups.get(g))
                        .and_then(|g| g.slot.clone())
                };
                let main_slot = self.actor_main(actor).map(|m| slot_of(self, m));
                let own_slot = slot_of(self, i);
                let parent = self
                    .actor_skills(actor)
                    .into_iter()
                    .find(|&k| self.skill_name(k) == name && main_slot.as_ref() == Some(&own_slot))
                    .ok_or("attempt to index a nil value (local 'parentSkill')")?;
                let (pl, pc) = (self.lists[parent].list, self.cfg(parent));
                let pc = Some(&pc);
                if self.c.flag(pl, pc, &["DisableSkill"])?
                    && !self.c.flag(pl, pc, &["EnableSkill"])?
                {
                    return Ok(());
                }
                self.sd_copy_from(i, parent, "cooldown");
                let cooldown = match self.c.override_num(pl, pc, &["CooldownRecovery"])? {
                    Some(v) => v,
                    None => {
                        (self.sd_num(parent, "cooldown").ok_or(
                            "attempt to perform arithmetic on a nil value (field 'cooldown')",
                        )? + self.c.base(pl, pc, &["CooldownRecovery"])?)
                            / m_max(0.0, self.c.calc_mod(pl, pc, &["CooldownRecovery"])?)
                    }
                };
                let tick = self.misc_data("ServerTickRate");
                let cd = (cooldown * tick).ceil() / tick;
                self.c.set_out(actor, "Cooldown", cd);
                self.sd_set(i, "hitTimeOverride", cd);
                let max = self
                    .sd_num(parent, "storedUses")
                    .ok_or("attempt to perform arithmetic on a nil value (field 'storedUses')")?
                    + self.c.base(pl, pc, &["AdditionalCooldownUses"])?;
                match part {
                    Some(1.0) => self.sd_set(i, "averageBurstHits", 1.0),
                    Some(2.0) => self.sd_set(i, "averageBurstHits", max),
                    _ => self.sd_clear(i, "averageBurstHits"),
                }
            }
            ("KineticFusillade" | "KineticFusilladeAltX", "preDamageFunc") => {
                if part == Some(1.0) {
                    let count = out_req(self, "ProjectileCount")?;
                    self.sd_set(i, "dpsMultiplier", count);
                    let per = self.sd_num(i, "damagePerProjectile").unwrap_or(0.0);
                    if per != 0.0 && count > 1.0 {
                        let avg = per * (count - 1.0) / 2.0;
                        self.c.add(
                            list,
                            Mod::new("Damage", ModType::More, avg, Some(&format!("Skill:{id}"))),
                        );
                        self.c.set_out(actor, "KineticFusilladeAvgMoreMult", avg);
                    }
                }
            }
            ("ForbiddenRite" | "ForbiddenRiteAltX", "preDamageFunc") => {
                let names = [
                    "DamageTaken",
                    "ChaosDamageTaken",
                    "DamageTakenWhenHit",
                    "ChaosDamageTakenWhenHit",
                ];
                let flat = self.c.base(list, None, &names)?;
                let inc = self.c.inc(list, None, &names)?;
                let more = self.c.more(list, None, &names)?;
                let taken = m_max((1.0 + inc / 100.0) * more, 0.0);
                let chaos_flat = ((flat * taken + 0.5).floor() + 0.0001).floor();
                let totem = self.has_flag(i, "totem");
                let pick = |a: &str, b: &str| if totem { a.to_string() } else { b.to_string() };
                let es = out_req(self, &pick("TotemEnergyShield", "EnergyShield"))?;
                let res = out_req(self, &pick("TotemChaosResist", "ChaosResist"))?;
                let self_taken =
                    |v: f64| ((v + 0.5).floor() * (100.0 - res) / 100.0 * taken).floor();
                let taken_es = self_taken(es * sd(self, "SelfDamageTakenES"));
                let (add, total) = if id == "ForbiddenRite" {
                    let life = out_req(self, &pick("TotemLife", "Life"))?;
                    let add = life * sd(self, "lifeDealtAsChaos")
                        + es * sd(self, "energyShieldDealtAsChaos");
                    let taken_life = self_taken(life * sd(self, "SelfDamageTakenLife"));
                    (add, taken_life + taken_es + chaos_flat)
                } else {
                    (
                        es * sd(self, "energyShieldDealtAsChaos"),
                        taken_es + chaos_flat,
                    )
                };
                for k in ["ChaosMin", "ChaosMax"] {
                    let v = sd(self, k) + add;
                    self.sd_set(i, k, v);
                }
                if part == Some(2.0) {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    let v = cur * (out_req(self, "ProjectileCount")? + 1.0);
                    self.sd_set(i, "dpsMultiplier", v);
                }
                self.c.set_out(actor, "FRDamageTaken", total);
            }
            ("EnemyExplode", "preDamageFunc") => {
                self.c.set_out(actor, "ExplodeChance", 0.0);
                let rec = |v: &ModValue, k: &str| match v {
                    ModValue::Record(r) => r.get(k).cloned(),
                    _ => None,
                };
                let num = |v: &ModValue, k: &str| rec(v, k).and_then(|x| x.as_number());
                let typ = |v: &ModValue| match rec(v, "type") {
                    Some(ModValue::Str(t)) => t.to_string(),
                    _ => String::new(),
                };
                if part != Some(3.0) {
                    let source = self
                        .skill(i)
                        .active_effect
                        .src
                        .and_then(|(g, k)| self.env.groups.get(g)?.gem_list.get(k))
                        .and_then(|g| g.explode_source.clone())
                        .ok_or("attempt to index a nil value (field 'explodeSource')")?;
                    let source = match source {
                        crate::skills::ExplodeSource::Node(id) => format!("Tree:{id}"),
                        crate::skills::ExplodeSource::Item(id) => self
                            .input
                            .tab
                            .item(id)
                            .map(|it| it.mod_source.clone())
                            .ok_or("an explode source without its item")?,
                    };
                    for (v, m) in self
                        .c
                        .tabulate(list, Some(ModType::List), c, &["ExplodeMod"])?
                    {
                        if m.source.as_deref() != Some(source.as_str()) {
                            continue;
                        }
                        let amount = num(&v, "amount").unwrap_or(f64::NAN);
                        let t = typ(&v);
                        if t == "RandomElement" {
                            for e in ["Fire", "Cold", "Lightning"] {
                                self.sd_set(
                                    i,
                                    &format!("{e}EffectiveExplodePercentage"),
                                    amount / 3.0,
                                );
                            }
                        } else {
                            self.sd_set(i, &format!("{t}EffectiveExplodePercentage"), amount);
                        }
                        let chance = if part == Some(2.0) {
                            1.0
                        } else {
                            num(&v, "chance").unwrap_or(f64::NAN)
                        };
                        self.c.set_out(actor, "ExplodeChance", chance);
                    }
                } else {
                    // By type, then by amount, in the order first met
                    let mut types: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
                    for v in self.c.list(list, c, &["ExplodeMod"])? {
                        let t = typ(&v);
                        let amount = num(&v, "amount").unwrap_or(f64::NAN);
                        let chance = num(&v, "chance").unwrap_or(f64::NAN);
                        let k = match types.iter().position(|(x, _)| *x == t) {
                            Some(k) => k,
                            None => {
                                types.push((t, Vec::new()));
                                types.len() - 1
                            }
                        };
                        match types[k].1.iter_mut().find(|(a, _)| *a == amount) {
                            Some(e) => e.1 += chance,
                            None => types[k].1.push((amount, chance)),
                        }
                    }
                    for (t, amounts) in types {
                        let mut phys = 0.0;
                        for (amount, chance) in amounts {
                            let ax = amount * chance;
                            let add = |p: &mut Self, k: String, v: f64| {
                                let cur = p.sd_num(i, &k).unwrap_or(0.0);
                                p.sd_set(i, &k, cur + v);
                            };
                            if t == "RandomElement" {
                                for e in ["Fire", "Cold", "Lightning"] {
                                    add(self, format!("{e}EffectiveExplodePercentage"), ax / 3.0);
                                }
                            } else {
                                add(self, format!("{t}EffectiveExplodePercentage"), ax);
                            }
                            if t == "Physical" {
                                phys = 1.0 - ((1.0 - phys) * (1.0 - chance));
                            }
                            let ec = self.c.out(actor, "ExplodeChance").unwrap_or(0.0);
                            self.c.set_out(
                                actor,
                                "ExplodeChance",
                                1.0 - ((1.0 - ec) * (1.0 - chance)),
                            );
                        }
                        if t == "Physical" && phys != 0.0 {
                            self.c.add(
                                list,
                                Mod::new(
                                    "CalcArmourAsThoughDealing",
                                    ModType::More,
                                    100.0 / m_min(phys, 1.0) - 100.0,
                                    None,
                                ),
                            );
                        }
                    }
                }
                let ec = self.c.out(actor, "ExplodeChance").unwrap_or(0.0);
                self.c
                    .set_out(actor, "ExplodeChance", m_min(ec * 100.0, 100.0));
            }
            ("ExplosiveArrow", "explosiveArrowFunc") => {
                if part != Some(1.0) && part != Some(2.0) {
                    return Ok(());
                }
                let duration = self.skill_duration(list, c, i)?;
                let limit = self.c.base(list, c, &["ExplosiveArrowMaxFuseCount"])?;
                let totems = if self.has_flag(i, "totem") {
                    let pdb = self.pdb();
                    Some(match self.c.override_num(pdb, None, &["TotemsSummoned"])? {
                        Some(v) => v,
                        None => {
                            self.c
                                .base(list, c, &["ActiveTotemLimit", "ActiveBallistaLimit"])?
                        }
                    })
                } else {
                    None
                };
                let f = |p: &Self, n: &str| p.c.flag(list, None, &[n]);
                let mut barrage = None;
                if f(self, "SequentialProjectiles")?
                    && !f(self, "OneShotProj")?
                    && !f(self, "NoAdditionalProjectiles")?
                    && !f(self, "TriggeredBySnipe")?
                {
                    let b = self.c.base(list, c, &["ProjectileCount"])?;
                    barrage = Some(b);
                    let v = sd(self, "dpsMultiplier") / b;
                    self.sd_set(i, "dpsMultiplier", v);
                }
                let hit = self
                    .po(o, "HitChance")
                    .ok_or("attempt to perform arithmetic on a nil value (field 'HitChance')")?;
                let speed = self
                    .go(o, "Speed")
                    .ok_or("attempt to perform arithmetic on a nil value (field 'Speed')")?;
                let mut rate =
                    (hit / 100.0) * speed * sd(self, "dpsMultiplier") * barrage.unwrap_or(1.0);
                if let Some(t) = totems {
                    rate *= t;
                }
                if part == Some(2.0) {
                    let max = m_min((rate * duration).floor() + 1.0, limit);
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:ExplosiveArrowStage",
                            ModType::Base,
                            max,
                            Some("Base"),
                        ),
                    );
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:ExplosiveArrowStageAfterFirst",
                            ModType::Base,
                            max - 1.0,
                            Some("Base"),
                        ),
                    );
                    self.set_go(o, "MaxExplosiveArrowFuseCalculated", max);
                } else {
                    self.c.clear_out(actor, "MaxExplosiveArrowFuseCalculated");
                }
                let to_max = limit / rate;
                let stages = self.mods(i).active_stage_count.unwrap_or(0.0);
                let hit_time = if part == Some(2.0) || stages + 1.0 >= limit {
                    m_min(duration, to_max)
                } else {
                    duration
                };
                self.set_go(o, "HitTime", hit_time);
                self.set_go(o, "HitSpeed", 1.0 / hit_time);
            }
            ("MoltenStrike" | "MoltenStrikeAltX", "preDamageFunc") => {
                if part == Some(1.0) {
                    return Ok(());
                }
                let enemy = match self.c.override_num(list, c, &["EnemyRadius"])? {
                    Some(v) => v,
                    None => self.c.base(list, c, &["EnemyRadius"])?,
                };
                let ball = out_req(self, "AreaOfEffectRadius")?;
                let inner = out_req(self, "AreaOfEffectRadiusSecondary")?;
                let outer = out_req(self, "AreaOfEffectRadiusTertiary")?;
                let overlap = m_min(1.0, (enemy + ball - inner) / (outer - inner));
                self.c.set_out(actor, "OverlapChance", overlap * 100.0);
                let count = out_req(self, "ProjectileCount")?;
                let alt = id == "MoltenStrikeAltX";
                let mut mult = 1.0;
                if part == Some(3.0) || (alt && (part == Some(5.0) || part == Some(6.0))) {
                    mult = overlap * count;
                    if alt && part == Some(6.0) {
                        let fifth = 1.0 + sd(self, "FifthStrikeDamage") / 100.0;
                        let overall =
                            fifth * overlap * (count + sd(self, "FifthStrikeProjectiles"));
                        mult = 0.8 * mult + 0.2 * overall;
                    }
                }
                if mult != 1.0 {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * mult);
                    let cur = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0);
                    self.c.set_out(actor, "SkillDPSMultiplier", cur * mult);
                }
            }
            ("BallLightning", "preDamageFunc") => {
                let mut mult = 0.0;
                if part == Some(1.0) {
                    mult = 1.0;
                } else if part == Some(2.0) {
                    let per_strike = sd(self, "strikeInterval");
                    let max_strikes = (sd(self, "duration") / per_strike).floor();
                    let inc = 1.0 + self.c.inc(list, c, &["ProjectileSpeed"])? / 100.0;
                    let more = self.c.more(list, c, &["ProjectileSpeed"])?;
                    let per = sd(self, "projectileSpeed") * (inc * more) * per_strike;
                    let radius = out_req(self, "AreaOfEffectRadius")?;
                    let dist = match cfg.skill_dist {
                        Some(d) => d,
                        None if self.has_flag(i, "triggered") => (radius / 2.0).floor(),
                        None => radius,
                    };
                    let first = m_max(1.0, ((dist - radius) / per).ceil());
                    let last = (m_min(self.misc_data("ProjectileDistanceCap"), dist + radius)
                        / per)
                        .floor();
                    let n = m_max(0.0, m_min(max_strikes, last + 1.0 - first));
                    // `output.NormalHitsPerCast` only with breakdowns
                    mult = n;
                }
                if mult != 1.0 {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * mult);
                    let cur = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0);
                    self.c.set_out(actor, "SkillDPSMultiplier", cur * mult);
                }
            }
            ("KineticFusillade" | "KineticFusilladeAltX", "postCritFunc") => {
                let delay = sd(self, "delayPerProjectile");
                let count = if part == Some(1.0) {
                    out_req(self, "ProjectileCount")?
                } else {
                    1.0
                };
                let effective =
                    (sd(self, "duration") + delay * (count - 1.0)) * out_req(self, "DurationMod")?;
                let tick = self.misc_data("ServerTickTime");
                let max_aps = 1.0 / ((effective / tick).ceil() * tick);
                self.c
                    .set_out(actor, "KineticFusilladeMaxEffectiveAPS", max_aps);
                if part == Some(1.0)
                    && let Some(aps) = self.c.out(actor, "Speed")
                    && aps > max_aps
                {
                    let original = match self.sd_num(i, "dpsMultiplier") {
                        Some(v) => v,
                        None => out_req(self, "ProjectileCount")?,
                    };
                    self.sd_set(i, "dpsMultiplier", original * (max_aps / aps));
                }
            }
            ("SeismicTrap" | "SeismicTrapAltX", "preDamageFunc") => {
                let base = id == "SeismicTrap";
                let mut traps = 1.0;
                if base {
                    let names = ["TrapThrowingSpeed", "SeismicPulseFrequency"];
                    let inc = 1.0 + self.c.inc(list, c, &names)? / 100.0;
                    let more = self.c.more(list, c, &names)?;
                    let rate = inc * more / sd(self, "repeatInterval");
                    self.sd_set(i, "hitTimeOverride", 1.0 / rate);
                    self.c.set_out(actor, "WavePulseRate", rate);
                    let dinc = 1.0 + self.c.inc(list, c, &["Duration"])? / 100.0;
                    let dmore = self.c.more(list, c, &["Duration"])?;
                    let duration = sd(self, "duration") * dinc * dmore;
                    let pulses = (duration * rate).floor();
                    self.c.set_out(actor, "PulsesPerTrap", pulses);
                    traps = (pulses / rate) / out_req(self, "TrapCooldown")?;
                    self.c.set_out(actor, "AverageActiveTraps", traps);
                }
                let enemy = match self.c.override_num(list, c, &["EnemyRadius"])? {
                    Some(v) => v,
                    None => self.c.base(list, c, &["EnemyRadius"])?,
                };
                let wave = out_req(self, "AreaOfEffectRadiusSecondary")?;
                let full = out_req(self, "AreaOfEffectRadius")?;
                let damaging = wave + enemy - 1.0;
                let overlap = m_min(damaging * damaging / (full * full), 1.0);
                self.c.set_out(actor, "OverlapChance", overlap * 100.0);
                let waves = self.c.base(list, c, &["MaximumWaves"])?;
                let mult = match part {
                    Some(2.0) => waves * overlap,
                    Some(3.0) => waves,
                    Some(4.0) if base => traps,
                    Some(5.0) if base => traps * waves * overlap,
                    Some(6.0) if base => traps * waves,
                    _ => 1.0,
                };
                if mult != 1.0 {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * mult);
                    let cur = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0);
                    self.c.set_out(actor, "SkillDPSMultiplier", cur * mult);
                }
            }
            (
                "LightningSpireTrap" | "LightningSpireTrapAltX" | "LightningSpireTrapAltY",
                "preDamageFunc",
            ) => {
                // Its targeting area is not affected by area of effect
                match self.sd_num(i, "radius") {
                    Some(r) => self.c.set_out(actor, "AreaOfEffectRadius", r),
                    None => self.c.clear_out(actor, "AreaOfEffectRadius"),
                }
                let interval = sd(self, "repeatInterval");
                let rate = if id == "LightningSpireTrap" {
                    let inc = 1.0 + self.c.inc(list, c, &["TrapThrowingSpeed"])? / 100.0;
                    let more = self.c.more(list, c, &["TrapThrowingSpeed"])?;
                    let r = inc * more / interval;
                    self.c.set_out(actor, "WavePulseRate", r);
                    r
                } else {
                    1.0 / interval
                };
                self.sd_set(i, "hitTimeOverride", 1.0 / rate);
                let dinc = 1.0 + self.c.inc(list, c, &["Duration"])? / 100.0;
                let dmore = self.c.more(list, c, &["Duration"])?;
                let pulses = (sd(self, "duration") * dinc * dmore * rate).floor();
                self.c.set_out(actor, "PulsesPerTrap", pulses);
                let effective = pulses / rate;
                let traps = if id == "LightningSpireTrapAltX" {
                    let asm = 1.0 + self.c.inc(list, c, &["ActionSpeed"])? / 100.0;
                    let base = 1.0 / self.c.base(list, c, &["TrapThrowingTime"])?;
                    let throw = m_min(
                        base * self.c.calc_mod(list, c, &["TrapThrowingSpeed"])? * asm,
                        self.misc_data("ServerTickRate"),
                    );
                    effective / (1.0 / throw)
                } else {
                    effective / out_req(self, "TrapCooldown")?
                };
                self.c.set_out(actor, "AverageActiveTraps", traps);
                let enemy = match self.c.override_num(list, c, &["EnemyRadius"])? {
                    Some(v) => v,
                    None => self.c.base(list, c, &["EnemyRadius"])?,
                };
                let wave = out_req(self, "AreaOfEffectRadiusSecondary")?;
                let full = out_req(self, "AreaOfEffectRadius")?;
                let damaging = wave + enemy - 1.0;
                let overlap = m_min(damaging * damaging / (full * full), 1.0);
                self.c.set_out(actor, "OverlapChance", overlap * 100.0);
                let waves = self.c.base(list, c, &["MaximumWaves"])?;
                let mult = match part {
                    Some(2.0) => waves * overlap,
                    Some(3.0) => waves,
                    Some(4.0) => traps,
                    Some(5.0) => traps * waves * overlap,
                    Some(6.0) => traps * waves,
                    _ => 1.0,
                };
                if mult != 1.0 {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", cur * mult);
                    let cur = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0);
                    self.c.set_out(actor, "SkillDPSMultiplier", cur * mult);
                }
            }
            ("GAZombieCorpseGroundImpact" | "MinionInstability", "preDamageFunc") => {
                // `activeSkill.summonSkill.skillData.summonSpeed`
                let summon = self
                    .minions
                    .iter()
                    .find(|m| m.skills.contains(&i))
                    .map(|m| m.skill)
                    .ok_or("attempt to index a nil value (field 'summonSkill')")?;
                let speed = self.sd_num(summon, "summonSpeed").unwrap_or(1.0);
                self.sd_set(i, "hitTimeOverride", 1.0 / speed);
            }
            ("StormRainAltX", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = sd(self, "hitFrequency")
                        / (1.0 + self.c.inc(list, c, &["StormRainBeamFrequency"])? / 100.0);
                    self.sd_set(i, "hitTimeOverride", v);
                    let m = m_min(
                        self.sd_num(i, "activeArrowMultiplier").unwrap_or(1.0),
                        self.c.base(list, c, &["StormRainAllowedStormArrows"])?,
                    );
                    self.sd_set(i, "dpsMultiplier", m);
                }
            }
            ("BladeVortex", "preDamageFunc") => {
                let blades = self.c.base(list, c, &["Multiplier:BladeVortexBlade"])?;
                let v =
                    sd(self, "hitFrequency") / (1.0 + blades * sd(self, "hitFrequencyPerBlade"));
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("VaalBladeVortex", "preDamageFunc") => {
                let v = sd(self, "hitFrequency")
                    / (1.0
                        + sd(self, "VaalBladeVortexBlade")
                            * self.sd_num(i, "hitFrequencyPerBlade").unwrap_or(0.0));
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("BladefallAltZ", "preDamageFunc") => {
                let v = sd(self, "hitFrequency") / (1.0 + sd(self, "incVolleyFrequency") / 100.0);
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("CremationAltY", "preDamageFunc") => {
                let v = sd(self, "cremationFireRate")
                    / (1.0 + self.sd_num(i, "cremationFireRateIncrease").unwrap_or(0.0));
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("Cremation" | "CremationAltX", "preDamageFunc") => {
                if part == Some(1.0) {
                    let v = sd(self, "cremationFireRate")
                        / (1.0 + self.sd_num(i, "cremationFireRateIncrease").unwrap_or(0.0));
                    self.sd_set(i, "hitTimeOverride", v);
                }
            }
            ("ScourgeArrow", "preDamageFunc") => {
                // `cfg` is an undefined global there
                let v = m_max(
                    self.c.base(list, None, &["Multiplier:ScourgeArrowStage"])? - 0.5,
                    0.5,
                );
                self.sd_set(i, "hitTimeMultiplier", v);
            }
            ("ToxicRain" | "ToxicRainAltX" | "ToxicRainAltY", "preDamageFunc") => {
                let v = m_min(
                    self.sd_num(i, "podOverlapMultiplier").unwrap_or(1.0),
                    out_req(self, "ProjectileCount")?,
                );
                self.sd_set(i, "dpsMultiplier", v);
            }
            ("Tornado", "preDamageFunc") => {
                self.sd_copy(i, "damageInterval", "hitTimeOverride");
            }
            ("TornadoAltY", "preDamageFunc") => {
                self.sd_copy(i, "damageInterval", "hitTimeOverride");
                let stages =
                    self.c
                        .base(list, c, &["Multiplier:TornadoofElementalTurbulenceStage"])?;
                let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                self.sd_set(i, "dpsMultiplier", cur * stages);
            }
            ("DivineIreAltY", "preDamageFunc") => {
                let v = self
                    .c
                    .base(list, c, &["Multiplier:DivineIreofDisintegrationStage"])?;
                self.sd_set(i, "hitTimeMultiplier", v);
            }
            ("DivineIreAltX", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = self
                        .c
                        .base(list, c, &["Multiplier:DivineIreofHolyLightningStage"])?;
                    self.sd_set(i, "hitTimeMultiplier", v);
                }
            }
            ("EyeOfWinter" | "EyeOfWinterAltX" | "EyeOfWinterAltY", "preDamageFunc") => {
                let ramp = self.c.base(list, c, &["EyeOfWinterRamp"])?;
                let psm = out_req(self, "ProjectileSpeedMod")?;
                let m = Mod::new("Damage", ModType::More, ramp, Some(&format!("Skill:{id}"))).tag(
                    Tag::kind(TagKind::DistanceRamp(vec![(0.0, 0.0), (60.0 * psm, 1.0)])),
                );
                self.c.add(list, m);
            }
            ("FreezingPulse", "preDamageFunc") => {
                let psm = out_req(self, "ProjectileSpeedMod")?;
                let ramp = |pts: Vec<(f64, f64)>| Tag::kind(TagKind::DistanceRamp(pts));
                self.c.add(
                    list,
                    Mod::new("Damage", ModType::More, -50.0, Some("Skill:FreezingPulse"))
                        .tag(ramp(vec![(0.0, 0.0), (60.0 * psm, 1.0)])),
                );
                self.c.add(
                    list,
                    Mod::new(
                        "EnemyFreezeChance",
                        ModType::Base,
                        25.0,
                        Some("Skill:FreezingPulse"),
                    )
                    .tag(ramp(vec![(0.0, 1.0), (15.0 * psm, 0.0)])),
                );
            }
            ("FlameblastAltX" | "FlameblastAltY", "preDamageFunc") => {
                let stage = if id == "FlameblastAltX" {
                    "Multiplier:FlameblastofCelerityStage"
                } else {
                    "Multiplier:FlameblastofContractionStage"
                };
                let v = m_max(
                    self.c.base(list, c, &[stage])?
                        - self
                            .c
                            .base(list, c, &["Multiplier:FlameblastMinimumStage"])?,
                    1.0,
                );
                self.sd_set(i, "hitTimeMultiplier", v);
            }
            ("GalvanicField" | "GalvanicFieldAltX", "preDamageFunc") => {
                self.sd_copy(i, "repeatFrequency", "hitTimeOverride");
            }
            (
                "HeraldOfThunder" | "VoidSphere" | "VoidSphereAltX" | "HeraldOfTheBreach",
                "preDamageFunc",
            ) => {
                let stat = match id {
                    "HeraldOfThunder" => "HeraldStormFrequency",
                    "HeraldOfTheBreach" => "PulseFrequencyPerPressure",
                    _ => "VoidSphereFrequency",
                };
                let v = sd(self, "repeatFrequency") / (1.0 + self.c.inc(list, c, &[stat])? / 100.0);
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("Hydrosphere", "preDamageFunc") => {
                if matches!(part, Some(p) if p == 1.0 || p == 2.0 || p == 3.0) {
                    let v = sd(self, "repeatFrequency")
                        / (1.0 + self.c.inc(list, c, &["HydroSphereFrequency"])? / 100.0);
                    self.sd_set(i, "hitTimeOverride", v);
                }
            }
            ("StaticStrike", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = sd(self, "repeatFrequency")
                        / (1.0 + self.c.inc(list, c, &["StaticStrikeFrequency"])? / 100.0);
                    self.sd_set(i, "hitTimeOverride", v);
                }
            }
            ("OrbOfStorms", "preDamageFunc") => {
                let v = sd(self, "hitFrequency") / self.c.calc_mod(list, c, &["Speed"])?;
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("VortexAltX" | "LightningBolt", "preDamageFunc") => {
                match self.c.out(actor, "Cooldown") {
                    Some(cd) => self.sd_set(i, "hitTimeOverride", cd),
                    None => self.sd_clear(i, "hitTimeOverride"),
                }
            }
            ("Perforate" | "PerforateAltX" | "PerforateAltY", "preDamageFunc") => {
                let v = m_max(self.sd_num(i, "dpsMultiplier").unwrap_or(1.0), 1.0);
                self.sd_set(i, "dpsMultiplier", v);
            }
            ("AzmeriHydraBarrage", "preDamageFunc") => {
                let v = out_req(self, "ProjectileCount")?;
                self.sd_set(i, "dpsMultiplier", v);
            }
            ("Barrage" | "BarrageAltX" | "BlastRain" | "VaalLightningArrow", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = out_req(self, "ProjectileCount")?;
                    self.sd_set(i, "dpsMultiplier", v);
                }
            }
            ("BlazingSalvo", "preDamageFunc") => {
                if part == Some(2.0) {
                    let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                    let v = cur * out_req(self, "ProjectileCount")?;
                    self.sd_set(i, "dpsMultiplier", v);
                }
            }
            ("FrostBombAltY" | "VoltaxicBurst" | "Earthquake", "preDamageFunc") => {
                let duration =
                    (sd(self, "duration") * out_req(self, "DurationMod")? * 10.0).floor();
                let m = match id {
                    "FrostBombAltY" => Mod::new(
                        "Multiplier:100msFrostBombDuration",
                        ModType::Base,
                        duration,
                        Some("Skill:FrostBombAltY"),
                    ),
                    _ => {
                        let stat = if id == "Earthquake" {
                            "EarthquakeDurationIncDamage"
                        } else {
                            "VoltaxicDurationIncDamage"
                        };
                        let v = self.c.inc(list, c, &[stat])? * duration;
                        Mod::new("Damage", ModType::Inc, v, Some(&format!("Skill:{id}")))
                    }
                };
                self.c.add(list, m);
            }
            ("Incinerate" | "IncinerateAltX" | "IncinerateAltY", "preDamageFunc") => {
                if part == Some(2.0) {
                    let stage = match id {
                        "Incinerate" => "Multiplier:IncinerateStage",
                        "IncinerateAltX" => "Multiplier:IncinerateofExpanseStage",
                        _ => "Multiplier:IncinerateofVentingStage",
                    };
                    let v = m_max(
                        self.c.base(list, c, &[stage])?
                            - self
                                .c
                                .base(list, c, &["Multiplier:IncinerateMinimumStage"])?
                            - 0.4175,
                        0.5825,
                    );
                    self.sd_set(i, "hitTimeMultiplier", v);
                }
            }
            ("RighteousFireAltX", "preDamageFunc") => {
                if out_req(self, "LifeUnreserved")? > 1.0 {
                    let v = out_req(self, "Mana")? * sd(self, "RFManaMultiplier");
                    self.sd_set(i, "FireDot", v);
                }
            }
            ("WinterOrb", "preDamageFunc") => {
                let rate = 1.0 + self.c.inc(list, c, &["HitRate", "Speed"])? / 100.0;
                let mult = self.c.more(list, c, &["HitRate"])?;
                let v = sd(self, "repeatFrequency") / rate / mult;
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("MoltenShell", "preDamageFunc") => {
                let add = self.sd_num(i, "MoltenShellDamageMitigated").unwrap_or(0.0)
                    * sd(self, "moltenShellReflect")
                    / 100.0;
                self.sd_set(i, "FireMin", add);
                self.sd_set(i, "FireMax", add);
            }
            ("BladeBlast" | "BladeBlastAltX", "preDamageFunc") => {
                let cur = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
                self.sd_set(i, "dpsMultiplier", cur * sd(self, "dpsBaseMultiplier"));
                if part == Some(2.0) {
                    self.sd_set(i, "hitTimeOverride", 1.0);
                }
            }
            ("ChargedDashAltX", "preDamageFunc") => {
                if part == Some(3.0) {
                    let v = self.c.inc(list, c, &["chargedDashFinalDamageModifier"])?;
                    let tag = Tag::other("Release Damage", [("skillPart", ModValue::Number(3.0))]);
                    let m = Mod::new("Damage", ModType::More, v, Some("Skill:ChargedDashAltX"))
                        .flags(self.mf("Attack"), 0)
                        .tag(tag);
                    self.c.add(list, m);
                }
            }
            ("StormRain", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = sd(self, "hitFrequency")
                        / (1.0 + self.c.inc(list, c, &["StormRainBeamFrequency"])? / 100.0);
                    self.sd_set(i, "hitTimeOverride", v);
                    let m = self.sd_num(i, "beamOverlapMultiplier").unwrap_or(1.0);
                    self.sd_set(i, "dpsMultiplier", m);
                }
            }
            ("PoisonousConcoction" | "PoisonousConcoctionAltX", "preDamageFunc") => {
                let mult = self.c.base(list, c, &["ChaosPerLifeFlaskPercent"])?;
                let added = self.c.out(actor, "LifeFlaskRecovery").unwrap_or(0.0) * (mult / 100.0);
                for n in ["ChaosMin", "ChaosMax"] {
                    self.c.add(
                        list,
                        Mod::new(n, ModType::Base, added, Some("Life Flask charges consumed")),
                    );
                }
            }
            ("Manabond", "preDamageFunc") => {
                let missing = self
                    .sd_num(i, "ManabondMissingUnreservedManaPercentage")
                    .unwrap_or(100.0);
                let v = ((sd(self, "ManabondMissingManaGainPercent") / 100.0)
                    * (missing / 100.0)
                    * self.c.out(actor, "ManaUnreserved").unwrap_or(0.0))
                .floor();
                for n in ["LightningMin", "LightningMax"] {
                    self.c.add(
                        list,
                        Mod::new(
                            n,
                            ModType::Base,
                            v,
                            Some(
                                "Manabond gain % missing unreserved mana as base lightning damage",
                            ),
                        ),
                    );
                }
            }
            ("WaveOfConviction" | "WaveOfConvictionAltY", "preDamageFunc") => {
                if let Some(d) = self.sd_num(i, "duration") {
                    let tick = self.misc_data("ServerTickRate");
                    let duration =
                        ((d * tick).ceil() / tick * out_req(self, "DurationMod")? * 10.0).floor();
                    let v = self.c.inc(list, c, &["WaveOfConvictionDurationDotMulti"])? * duration
                        / 100.0;
                    let m = Mod::new(
                        "DotMultiplier",
                        ModType::Base,
                        v,
                        Some(&format!("Skill:{id}")),
                    )
                    .tag(Tag::multiplier("WoCDurationExpired"));
                    self.c.add(list, m);
                }
            }
            ("BladeFlurry" | "BladeFlurryAltX", "preDamageFunc") => {
                let stages = sd(self, "numStages");
                if part == Some(2.0) && stages > 0.0 {
                    let mut multi = 0.0;
                    let mut n = 1.0;
                    while n <= stages {
                        multi += 0.8 + (0.2 * n);
                        n += 1.0;
                    }
                    multi /= 0.8 + (0.2 * stages);
                    self.sd_set(i, "dpsMultiplier", multi / stages + 1.0);
                }
            }
            ("DivineIre", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = self.c.base(list, c, &["Multiplier:DivineIreStage"])?;
                    self.sd_set(i, "hitTimeMultiplier", v);
                }
            }
            ("Flameblast", "preDamageFunc") => {
                let v = m_max(
                    self.c.base(list, c, &["Multiplier:FlameblastStage"])?
                        - self
                            .c
                            .base(list, c, &["Multiplier:FlameblastMinimumStage"])?,
                    1.0,
                );
                self.sd_set(i, "hitTimeMultiplier", v);
            }
            ("HeraldOfAsh", "preDamageFunc") => {
                let v = self.sd_num(i, "hoaOverkill").unwrap_or(0.0)
                    * (1.0 + sd(self, "hoaMoreBurn") / 100.0)
                    * sd(self, "hoaOverkillPercent");
                self.sd_set(i, "FireDot", v);
            }
            ("PenanceBrand", "preDamageFunc") => {
                let v = sd(self, "repeatFrequency")
                    * self
                        .c
                        .base(list, c, &["Multiplier:PenanceBrandMaxEnergy"])?
                    / (1.0
                        + self
                            .c
                            .inc(list, c, &["Speed", "BrandActivationFrequency"])?
                            / 100.0)
                    / self.c.more(list, c, &["BrandActivationFrequency"])?;
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("VaalMoltenShell", "preDamageFunc") => {
                let add = self
                    .sd_num(i, "VaalMoltenShellDamageMitigated")
                    .unwrap_or(0.0)
                    * sd(self, "moltenShellReflect")
                    / 100.0;
                self.sd_set(i, "FireMin", add);
                self.sd_set(i, "FireMax", add);
            }
            ("VaalRighteousFire", "preDamageFunc") => {
                let out = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
                let pool = if self.has_flag(i, "totem") {
                    out(self, "TotemLife")
                } else {
                    out(self, "Life") + out(self, "EnergyShield")
                };
                let v = pool * sd(self, "percentSacrificed") * sd(self, "RFMultiplier");
                self.sd_set(i, "FireDot", v);
            }
            ("Vortex", "preDamageFunc") => match self.c.out(actor, "Cooldown") {
                Some(cd) => self.sd_set(i, "hitTimeOverride", cd),
                None => self.sd_clear(i, "hitTimeOverride"),
            },
            ("WintertideBrand", "preDamageFunc") => {
                self.sd_set(i, "countsAttachedBrandsInDamage", part == Some(1.0));
                let time = brand_time(self)?;
                self.sd_set(i, "hitTimeOverride", time);
                if part == Some(1.0) {
                    let max_stages =
                        self.c
                            .base(list, c, &["Multiplier:WintertideBrandMaxStages"])?;
                    let debuff = 1.0
                        / m_max(
                            self.misc_data("BuffExpirationSlowCap"),
                            self.c.calc_mod(o.edb, c, &["BuffExpireFaster"])?,
                        );
                    // `calcSkillDuration` with an empty env: not effective
                    let mut names = vec!["Duration", "PrimaryDuration"];
                    if self.sd_truthy(i, "mineDurationAppliesToSkill") {
                        names.push("MineDuration");
                    }
                    let dmod = m_max(self.c.calc_mod(list, c, &names)?, 0.0);
                    let base = self.sd_num(i, "duration").unwrap_or(0.0)
                        + self.c.base(list, c, &["Duration", "PrimaryDuration"])?;
                    let duration = base * dmod * debuff;
                    let stages = m_min((duration / time).floor(), max_stages);
                    let at_max = m_max(duration - stages * time, 0.0);
                    let per_stage =
                        self.c
                            .base(list, c, &["Multiplier:WintertideBrandDamagePerStage"])?;
                    let average =
                        (stages * (stages - 1.0) / 2.0 * time + stages * at_max) / duration;
                    let average_mult = average * per_stage;
                    let end_base = self.sd_num(i, "durationTertiary").unwrap_or(0.0)
                        + self.c.base(list, c, &["Duration", "TertiaryDuration"])?;
                    let end_mod = m_max(
                        self.c
                            .calc_mod(list, c, &["Duration", "TertiaryDuration"])?,
                        0.0,
                    );
                    let tick = self.misc_data("ServerTickRate");
                    let end = (end_base * end_mod * debuff * tick).ceil() / tick;
                    let uptime = m_min(end / duration, 1.0);
                    let attached = sd(self, "attachedBrandCount");
                    let end_mult = stages * per_stage;
                    let v = attached * (100.0 + average_mult) + uptime * (100.0 + end_mult) - 100.0;
                    self.c.add(
                        list,
                        Mod::new(
                            "Damage",
                            ModType::More,
                            v,
                            Some("Wintertide Brand Average Multiplier"),
                        ),
                    );
                }
            }
            _ => return Err(format!("not ported: {name} of {id}")),
        }
        Ok(())
    }

    /// `skillData[to] = skillData[from]`
    fn sd_copy(&mut self, i: usize, from: &str, to: &str) {
        match self.sd(i, from).cloned() {
            Some(v) => self.sd_set(i, to, v),
            None => self.sd_clear(i, to),
        }
    }

    /// `skillData[k] = other.skillData[k]`
    fn sd_copy_from(&mut self, i: usize, other: usize, k: &str) {
        match self.sd(other, k).cloned() {
            Some(v) => self.sd_set(i, k, v),
            None => self.sd_clear(i, k),
        }
    }
}
