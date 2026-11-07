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

/// A `DistanceRamp` tag
fn ramp(points: &[(f64, f64)]) -> Tag {
    Tag {
        kind: TagKind::DistanceRamp(points.to_vec()),
        global_limit: None,
    }
}

/// A `SkillPart` tag
fn skill_part(part: f64) -> Tag {
    Tag {
        kind: TagKind::SkillPart {
            list: None,
            part: Some(part),
            neg: false,
        },
        global_limit: None,
    }
}

impl<'a> Perform<'a> {
    /// Runs the function `name` of the granted effect `id`
    pub(crate) fn skill_func(&mut self, o: &mut Off, id: &str, name: &str) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let sd = |p: &Self, k: &str| p.sd_num(i, k).unwrap_or(f64::NAN);
        let part = self.mods(i).skill_part;
        let out = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        let dps_mult = |p: &Self| p.sd_num(i, "dpsMultiplier").unwrap_or(1.0);
        // `repeatFrequency / (1 + inc Speed, BrandActivationFrequency) /
        // more BrandActivationFrequency`
        let brand_time = |p: &Self| -> Result<f64> {
            Ok(sd(p, "repeatFrequency")
                / (1.0 + p.c.inc(list, c, &["Speed", "BrandActivationFrequency"])? / 100.0)
                / p.c.more(list, c, &["BrandActivationFrequency"])?)
        };
        match (id, name) {
            ("BladeFlurryAltX", "preDamageFunc") => {
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
            ("StormBrand", "preDamageFunc") => {
                let v = brand_time(self)?;
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
            ("CycloneAltX", "initialFunc") => {
                let w = |p: &Self, n: usize, k: &str| p.weapon_num(actor, n, k);
                let range_sum = |p: &Self, n: usize| -> Result<f64> {
                    let wc = p.mods(i).weapon_cfgs[n].clone();
                    Ok(p.c.base(list, wc.as_ref(), &["MeleeWeaponRange"])?
                        + 10.0 * p.c.base(list, wc.as_ref(), &["MeleeWeaponRangeMetre"])?
                        + w(p, n, "rangeBonus").unwrap_or(f64::NAN))
                };
                let range = if self.has_flag(i, "weapon1Attack") && w(self, 0, "range").is_some() {
                    let r1 = range_sum(self, 0)?;
                    if self.has_flag(i, "weapon2Attack") && w(self, 1, "range").is_some() {
                        (r1 + range_sum(self, 1)?) / 2.0
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
                        Some("Skill:CycloneAltX"),
                    ),
                );
            }
            (
                "ArmageddonBrand" | "ArmageddonBrandAltY" | "StormBrandAltX" | "BrandSupport",
                "preDamageFunc",
            ) => {
                let v = brand_time(self)?;
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("GalvanicField" | "GalvanicFieldAltX", "preDamageFunc") => {
                let v = sd(self, "repeatFrequency");
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("BallLightningAltY", "preDamageFunc") => {
                let strikes = (sd(self, "duration") / sd(self, "strikeInterval")).floor();
                let m = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0) * strikes;
                self.sd_set(i, "dpsMultiplier", m);
                self.c.set_out(actor, "NormalHitsPerCast", strikes);
                let o2 = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0) * strikes;
                self.c.set_out(actor, "SkillDPSMultiplier", o2);
            }
            ("Bodyswap" | "BodyswapAltX", "preDamageFunc") => {
                if part == Some(1.0) {
                    let pool = if self.has_flag(i, "totem") {
                        "TotemLife"
                    } else {
                        "Life"
                    };
                    let v = self.c.out(actor, pool).unwrap_or(f64::NAN)
                        * sd(self, "selfFireExplosionLifeMultiplier");
                    self.sd_set(i, "FireBonusMin", v);
                    self.sd_set(i, "FireBonusMax", v);
                }
            }
            ("ChargedDash" | "ChargedDashAltX", "preDamageFunc") => {
                if part == Some(3.0) {
                    let v = self.c.inc(list, c, &["chargedDashFinalDamageModifier"])?;
                    let mut t = crate::mods::Tag::other("Release Damage", []);
                    if let crate::mods::tag::TagKind::Other { fields, .. } = &mut t.kind {
                        fields
                            .fields
                            .push((crate::mods::Sym::new("skillPart"), 3.0.into()));
                    }
                    let m = Mod::new("Damage", ModType::More, v, Some("Skill:ChargedDash"))
                        .flags(self.mf("Attack"), 0)
                        .tag(t);
                    self.c.add(list, m);
                }
            }
            ("DarkPactAltX", "preDamageFunc") => {
                let pool = if self.has_flag(i, "totem") {
                    "TotemLife"
                } else {
                    "Life"
                };
                let life = self.c.out(actor, pool).unwrap_or(f64::NAN);
                let add = life * sd(self, "percentageLifeSacrificed") / 100.0
                    * (sd(self, "percentageSacrificedDealtAsChaos") / 100.0);
                let add = if part == Some(2.0) {
                    add.floor()
                } else {
                    let ruin = 7.0
                        / (1.0
                            + m_min(100.0, self.sd_num(i, "additionalRuinChance").unwrap_or(0.0))
                                / 100.0);
                    (add / ruin).floor()
                };
                for k in ["ChaosMin", "ChaosMax"] {
                    let v = sd(self, k) + add;
                    self.sd_set(i, k, v);
                }
            }
            ("ForbiddenRite" | "ForbiddenRiteAltX", "preDamageFunc") => {
                // The transfigured gem takes energy shield only
                let alt = id == "ForbiddenRiteAltX";
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
                let chaos_flat = crate::calc::floor(crate::calc::round(flat * taken, 0.0), 0.0);
                let totem = self.has_flag(i, "totem");
                let out = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
                let (life, es, res) = if totem {
                    (
                        out(self, "TotemLife"),
                        out(self, "TotemEnergyShield"),
                        out(self, "TotemChaosResist"),
                    )
                } else {
                    (
                        out(self, "Life"),
                        out(self, "EnergyShield"),
                        out(self, "ChaosResist"),
                    )
                };
                let add = if alt {
                    es * sd(self, "energyShieldDealtAsChaos")
                } else {
                    life * sd(self, "lifeDealtAsChaos") + es * sd(self, "energyShieldDealtAsChaos")
                };
                let r = |v: f64| crate::calc::round(v, 0.0);
                let fl = |v: f64| crate::calc::floor(v, 0.0);
                let taken_life = if alt {
                    0.0
                } else {
                    fl(r(life * sd(self, "SelfDamageTakenLife")) * (100.0 - res) / 100.0 * taken)
                };
                let taken_es =
                    fl(r(es * sd(self, "SelfDamageTakenES")) * (100.0 - res) / 100.0 * taken);
                for k in ["ChaosMin", "ChaosMax"] {
                    let v = sd(self, k) + add;
                    self.sd_set(i, k, v);
                }
                if part == Some(2.0) {
                    let m = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0)
                        * (out(self, "ProjectileCount") + 1.0);
                    self.sd_set(i, "dpsMultiplier", m);
                }
                let total = if alt {
                    taken_es + chaos_flat
                } else {
                    taken_life + taken_es + chaos_flat
                };
                self.c.set_out(actor, "FRDamageTaken", total);
            }
            ("MoltenShell", "preDamageFunc") => {
                let add = self.sd_num(i, "MoltenShellDamageMitigated").unwrap_or(0.0)
                    * sd(self, "moltenShellReflect")
                    / 100.0;
                self.sd_set(i, "FireMin", add);
                self.sd_set(i, "FireMax", add);
            }
            ("RighteousFireAltX", "preDamageFunc") => {
                if self.c.out(actor, "LifeUnreserved").unwrap_or(f64::NAN) > 1.0 {
                    let v = self.c.out(actor, "Mana").unwrap_or(f64::NAN)
                        * sd(self, "RFManaMultiplier");
                    self.sd_set(i, "FireDot", v);
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
                let mut occ: Vec<(f64, f64)> = match self
                    .c
                    .out_val(actor, "AreaOfEffectRadiusTertiaryOccurrences")
                {
                    Some(crate::actor::OutVal::Table(t)) => t
                        .iter()
                        .filter_map(|(k, v)| Some((k.parse::<f64>().ok()?, v.num()?)))
                        .collect(),
                    _ => return Err("attempt to index a nil value (TertiaryOccurrences)".into()),
                };
                occ.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut overlap = 0.0;
                for (small, count) in occ {
                    let r = small + enemy_radius - 1.0;
                    let hit = m_min(r * r / (full * full), 1.0);
                    overlap += hit * count / width;
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
                    let m = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0) * mult;
                    self.sd_set(i, "dpsMultiplier", m);
                    let o2 = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0) * mult;
                    self.c.set_out(actor, "SkillDPSMultiplier", o2);
                }
            }
            ("EnemyExplode", "preDamageFunc") => self.enemy_explode(o)?,
            ("Barrage" | "BarrageAltX" | "BlastRain" | "VaalLightningArrow", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = out(self, "ProjectileCount");
                    self.sd_set(i, "dpsMultiplier", v);
                }
            }
            ("AzmeriHydraBarrage", "preDamageFunc") => {
                let v = out(self, "ProjectileCount");
                self.sd_set(i, "dpsMultiplier", v);
            }
            ("BladeBlast" | "BladeBlastAltX", "preDamageFunc") => {
                let v = dps_mult(self) * sd(self, "dpsBaseMultiplier");
                self.sd_set(i, "dpsMultiplier", v);
                if part == Some(2.0) {
                    self.sd_set(i, "hitTimeOverride", 1.0);
                }
            }
            ("BladeFlurry", "preDamageFunc") => {
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
            ("BladeVortex", "preDamageFunc") => {
                let v = sd(self, "hitFrequency")
                    / (1.0
                        + self.c.base(list, c, &["Multiplier:BladeVortexBlade"])?
                            * sd(self, "hitFrequencyPerBlade"));
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
            ("Cremation" | "CremationAltX" | "CremationAltY", "preDamageFunc") => {
                if id == "CremationAltY" || part == Some(1.0) {
                    let v = sd(self, "cremationFireRate")
                        / (1.0 + self.sd_num(i, "cremationFireRateIncrease").unwrap_or(0.0));
                    self.sd_set(i, "hitTimeOverride", v);
                }
            }
            ("Cyclone" | "VaalCyclone", "initialFunc") => {
                let w = |p: &Self, n: usize, k: &str| p.weapon_num(actor, n, k);
                let range_sum = |p: &Self, n: usize| -> Result<f64> {
                    let wc = p.mods(i).weapon_cfgs[n].clone();
                    Ok(p.c.base(list, wc.as_ref(), &["MeleeWeaponRange"])?
                        + 10.0 * p.c.base(list, wc.as_ref(), &["MeleeWeaponRangeMetre"])?
                        + w(p, n, "rangeBonus").unwrap_or(f64::NAN))
                };
                let range = if self.has_flag(i, "weapon1Attack") && w(self, 0, "range").is_some() {
                    let r1 = range_sum(self, 0)?;
                    if self.has_flag(i, "weapon2Attack") && w(self, 1, "range").is_some() {
                        (r1 + range_sum(self, 1)?) / 2.0
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
                        Some("Skill:Cyclone"),
                    ),
                );
            }
            ("FlamethrowerTrap" | "FlamethrowerTrapAltX", "preDamageFunc") => {
                let traps = out(self, "Duration") / out(self, "TrapCooldown");
                self.c.set_out(actor, "AverageActiveTraps", traps);
                let alt = id == "FlamethrowerTrapAltX";
                let hit = if !alt && (part == Some(2.0) || part == Some(4.0)) {
                    0.3
                } else {
                    0.1
                };
                self.sd_set(i, "hitTimeOverride", hit);
                if alt || part == Some(3.0) || part == Some(4.0) {
                    let v = dps_mult(self) * traps;
                    self.sd_set(i, "dpsMultiplier", v);
                }
            }
            ("LancingSteel" | "LancingSteelAltX", "preDamageFunc") => {
                if part == Some(2.0) {
                    let n = out(self, "ProjectileCount");
                    let reduced = (n - 1.0) / n;
                    let mult = (self.c.more(list, c, &["LancingSteelSubsequentDamage"])? - 1.0)
                        * 100.0
                        * reduced;
                    self.sd_set(i, "dpsMultiplier", n);
                    let source = format!("Skill:{id}");
                    self.c
                        .add(list, Mod::new("Damage", ModType::More, mult, Some(&source)));
                }
            }
            ("StormRain" | "StormRainAltX" | "StormRainAltY", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = sd(self, "hitFrequency")
                        / (1.0 + self.c.inc(list, c, &["StormRainBeamFrequency"])? / 100.0);
                    self.sd_set(i, "hitTimeOverride", v);
                    let m = match id {
                        "StormRain" => self.sd_num(i, "beamOverlapMultiplier").unwrap_or(1.0),
                        "StormRainAltX" => m_min(
                            self.sd_num(i, "activeArrowMultiplier").unwrap_or(1.0),
                            self.c.base(list, c, &["StormRainAllowedStormArrows"])?,
                        ),
                        _ => self.c.base(list, c, &["StormRainAllowedStormArrows"])?,
                    };
                    self.sd_set(i, "dpsMultiplier", m);
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
            ("ShrapnelBallista" | "ShrapnelBallistaAltX", "preDamageFunc") => {
                if !self.c.flag(list, None, &["SequentialProjectiles"])? {
                    let n = out(self, "ProjectileCount");
                    let overlap = self
                        .sd_num(i, "ShrapnelBallistaProjectileOverlap")
                        .unwrap_or(if self.has_type(i, "Rain") { n } else { 1.0 });
                    let v = dps_mult(self) * m_min(overlap, n);
                    self.sd_set(i, "dpsMultiplier", v);
                }
                let split = self.c.out(actor, "SplitCount").unwrap_or(0.0);
                if split > 0.0 {
                    self.c.add(
                        list,
                        Mod::new("DPS", ModType::More, split * 100.0, Some("Split Return"))
                            .tag(Tag::condition("ReturningProjectile")),
                    );
                }
            }
            ("ToxicRain" | "ToxicRainAltX" | "ToxicRainAltY", "preDamageFunc") => {
                let v = m_min(
                    self.sd_num(i, "podOverlapMultiplier").unwrap_or(1.0),
                    out(self, "ProjectileCount"),
                );
                self.sd_set(i, "dpsMultiplier", v);
            }
            ("TornadoShot", "preDamageFunc") => {
                if part == Some(2.0) && self.c.out(actor, "ReturnChance").unwrap_or(0.0) == 0.0 {
                    let split = self.c.out(actor, "SplitCount").unwrap_or(0.0);
                    let f = |p: &Self, k: &str| p.c.flag(list, None, &[k]);
                    let secondary = if f(self, "SequentialProjectiles")?
                        && !f(self, "OneShotProj")?
                        && !f(self, "NoAdditionalProjectiles")?
                        && !f(self, "SingleProjectile")?
                        && !f(self, "TriggeredBySnipe")?
                    {
                        1.0 + split
                    } else {
                        out(self, "ProjectileCount") + split
                    };
                    let chance = m_min(
                        match self.sd_num(i, "tornadoShotSecondaryHitChance") {
                            Some(v) => v,
                            None => {
                                20.0 * self.c.base(list, c, &["tornadoShotSecondaryProjectiles"])?
                            }
                        } / 100.0,
                        1.0,
                    );
                    let v = dps_mult(self) * (1.0 + chance * secondary);
                    self.sd_set(i, "dpsMultiplier", v);
                }
            }
            ("PoisonousConcoction" | "PoisonousConcoctionAltX", "preDamageFunc") => {
                let multiplier = self.c.base(list, c, &["ChaosPerLifeFlaskPercent"])?;
                let add =
                    self.c.out(actor, "LifeFlaskRecovery").unwrap_or(0.0) * (multiplier / 100.0);
                for k in ["ChaosMin", "ChaosMax"] {
                    self.c.add(
                        list,
                        Mod::new(k, ModType::Base, add, Some("Life Flask charges consumed")),
                    );
                }
            }
            ("Tornado" | "TornadoAltY", "preDamageFunc") => {
                let v = sd(self, "damageInterval");
                self.sd_set(i, "hitTimeOverride", v);
                if id == "TornadoAltY" {
                    let v = dps_mult(self)
                        * self.c.base(
                            list,
                            c,
                            &["Multiplier:TornadoofElementalTurbulenceStage"],
                        )?;
                    self.sd_set(i, "dpsMultiplier", v);
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
            ("BlazingSalvo", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = dps_mult(self) * out(self, "ProjectileCount");
                    self.sd_set(i, "dpsMultiplier", v);
                }
            }
            ("IceSpear" | "IceSpearAltX", "preDamageFunc") => {
                if part == Some(3.0) || part == Some(4.0) {
                    let v = dps_mult(self) * out(self, "ProjectileCount");
                    self.sd_set(i, "dpsMultiplier", v);
                }
            }
            ("DarkPact", "preDamageFunc") => {
                let life = if part == Some(1.0) {
                    let pool = if self.has_flag(i, "totem") {
                        "TotemLife"
                    } else {
                        "Life"
                    };
                    out(self, pool)
                } else {
                    self.sd_num(i, "skeletonLife").unwrap_or(0.0)
                };
                let add = life * sd(self, "lifeDealtAsChaos") / 100.0;
                for k in ["ChaosMin", "ChaosMax"] {
                    let v = sd(self, k) + add;
                    self.sd_set(i, k, v);
                }
            }
            ("DivineIreAltX", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = self
                        .c
                        .base(list, c, &["Multiplier:DivineIreofHolyLightningStage"])?;
                    self.sd_set(i, "hitTimeMultiplier", v);
                }
            }
            ("DivineIreAltY", "preDamageFunc") => {
                let v = self
                    .c
                    .base(list, c, &["Multiplier:DivineIreofDisintegrationStage"])?;
                self.sd_set(i, "hitTimeMultiplier", v);
            }
            ("EyeOfWinter" | "EyeOfWinterAltX" | "EyeOfWinterAltY", "preDamageFunc") => {
                let v = self.c.base(list, c, &["EyeOfWinterRamp"])?;
                let speed = out(self, "ProjectileSpeedMod");
                let source = format!("Skill:{id}");
                self.c.add(
                    list,
                    Mod::new("Damage", ModType::More, v, Some(&source))
                        .tag(ramp(&[(0.0, 0.0), (60.0 * speed, 1.0)])),
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
            ("FreezingPulse", "preDamageFunc") => {
                let speed = out(self, "ProjectileSpeedMod");
                self.c.add(
                    list,
                    Mod::new("Damage", ModType::More, -50.0, Some("Skill:FreezingPulse"))
                        .tag(ramp(&[(0.0, 0.0), (60.0 * speed, 1.0)])),
                );
                self.c.add(
                    list,
                    Mod::new(
                        "EnemyFreezeChance",
                        ModType::Base,
                        25.0,
                        Some("Skill:FreezingPulse"),
                    )
                    .tag(ramp(&[(0.0, 1.0), (15.0 * speed, 0.0)])),
                );
            }
            ("FrostBombAltY", "preDamageFunc") => {
                let d = (sd(self, "duration") * out(self, "DurationMod") * 10.0).floor();
                self.c.add(
                    list,
                    Mod::new(
                        "Multiplier:100msFrostBombDuration",
                        ModType::Base,
                        d,
                        Some("Skill:FrostBombAltY"),
                    ),
                );
            }
            ("Hydrosphere", "preDamageFunc") => {
                if matches!(part, Some(1.0) | Some(2.0) | Some(3.0)) {
                    let v = sd(self, "repeatFrequency")
                        / (1.0 + self.c.inc(list, c, &["HydroSphereFrequency"])? / 100.0);
                    self.sd_set(i, "hitTimeOverride", v);
                }
            }
            ("HeraldOfThunder", "preDamageFunc") => {
                let v = sd(self, "repeatFrequency")
                    / (1.0 + self.c.inc(list, c, &["HeraldStormFrequency"])? / 100.0);
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("HeraldOfTheBreach", "preDamageFunc") => {
                let v = sd(self, "repeatFrequency")
                    / (1.0 + self.c.inc(list, c, &["PulseFrequencyPerPressure"])? / 100.0);
                self.sd_set(i, "hitTimeOverride", v);
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
            ("LightningTendrilsAltX", "preDamageFunc") => {
                let interval = sd(self, "pulseInterval");
                let (v, src, p) = match part {
                    Some(2.0) => (-(1.0 / interval) * 100.0, "Normal pulse", 2.0),
                    Some(3.0) => (-(interval - 1.0) / interval * 100.0, "Stronger pulse", 3.0),
                    _ => return Ok(()),
                };
                self.c.add(
                    list,
                    Mod::new("DPS", ModType::More, v, Some(src)).tag(skill_part(p)),
                );
            }
            ("LightningTendrilsAltX", "postCritFunc") => {
                if part == Some(1.0) {
                    let interval = sd(self, "pulseInterval");
                    let pulse = sd(self, "pulseDamage") / 100.0;
                    let crit = out(self, "PreEffectiveCritChance") / 100.0;
                    let eff = out(self, "CritChance") / 100.0;
                    let multi = out(self, "CritMultiplier");
                    let more = 100.0
                        * (((interval - 1.0) * (1.0 + crit * (multi - 1.0))
                            + (1.0 + pulse) * multi)
                            / (interval * ((1.0 - eff) + multi * eff))
                            - 1.0);
                    let kw = self.kf("Hit") | self.kf("Ailment");
                    self.c.add(
                        list,
                        Mod::new("Damage", ModType::More, more, Some("Average Pulse Damage"))
                            .flags(0, kw)
                            .tag(skill_part(1.0)),
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
                for k in ["LightningMin", "LightningMax"] {
                    self.c.add(
                        list,
                        Mod::new(
                            k,
                            ModType::Base,
                            v,
                            Some(
                                "Manabond gain % missing unreserved mana as base lightning damage",
                            ),
                        ),
                    );
                }
            }
            ("OrbOfStorms", "preDamageFunc") => {
                let v = sd(self, "hitFrequency") / self.c.calc_mod(list, c, &["Speed"])?;
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("PenanceBrandAltX" | "PenanceBrandAltY", "preDamageFunc") => {
                let v = brand_time(self)?;
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("RighteousFire", "preDamageFunc") => {
                let rl = sd(self, "RFLifeMultiplier");
                let re = sd(self, "RFESMultiplier");
                if self.has_flag(i, "totem") && out(self, "TotemLife") > 1.0 {
                    let v = out(self, "TotemLife") * rl + out(self, "TotemEnergyShield") * re;
                    self.sd_set(i, "FireDot", v);
                } else if out(self, "LifeUnreserved") > 1.0 {
                    let v = out(self, "Life") * rl + out(self, "EnergyShield") * re;
                    self.sd_set(i, "FireDot", v);
                }
            }
            ("Spark" | "SparkAltX" | "SparkAltY", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = dps_mult(self) * (1.0 + (out(self, "Duration") / 0.66).floor());
                    self.sd_set(i, "dpsMultiplier", v);
                    self.c.set_out(actor, "SkillDPSMultiplier", v);
                }
            }
            ("StormBurst" | "StormBurstAltX", "preDamageFunc") => {
                let remaining = |p: &Self| {
                    ((sd(p, "duration") * out(p, "DurationMod") + 0.001) / sd(p, "repeatFrequency"))
                        .floor()
                };
                let bonus_part = if id == "StormBurst" { 3.0 } else { 2.0 };
                if id == "StormBurst" && part == Some(2.0) {
                    let duration = sd(self, "duration") * out(self, "DurationMod");
                    let jump = sd(self, "repeatFrequency") * 10.0;
                    let v = dps_mult(self) * (1.0 + (duration * 10.0 / jump).floor());
                    self.sd_set(i, "dpsMultiplier", v);
                } else if part == Some(bonus_part) {
                    let v = m_max(
                        sd(self, "remainingDurationDamage") * remaining(self) - 100.0,
                        0.0,
                    );
                    self.c.add(
                        list,
                        Mod::new("Damage", ModType::More, v, Some("Skill:StormBurst")),
                    );
                }
            }
            ("VoidSphere" | "VoidSphereAltX", "preDamageFunc") => {
                let v = sd(self, "repeatFrequency")
                    / (1.0 + self.c.inc(list, c, &["VoidSphereFrequency"])? / 100.0);
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("VoltaxicBurst" | "Earthquake", "preDamageFunc") => {
                let d = (sd(self, "duration") * out(self, "DurationMod") * 10.0).floor();
                let (stat, source) = if id == "Earthquake" {
                    ("EarthquakeDurationIncDamage", "Skill:Earthquake")
                } else {
                    ("VoltaxicDurationIncDamage", "Skill:VoltaxicBurst")
                };
                let v = self.c.inc(list, c, &[stat])? * d;
                self.c
                    .add(list, Mod::new("Damage", ModType::Inc, v, Some(source)));
            }
            ("VortexAltX" | "LightningBolt", "preDamageFunc") => {
                match self.c.out(actor, "Cooldown") {
                    Some(cd) => self.sd_set(i, "hitTimeOverride", cd),
                    None => self.sd_clear(i, "hitTimeOverride"),
                }
            }
            ("WaveOfConviction" | "WaveOfConvictionAltY", "preDamageFunc") => {
                if let Some(d) = self.sd_num(i, "duration") {
                    let tick = self.misc_data("ServerTickRate");
                    let d = ((d * tick).ceil() / tick * out(self, "DurationMod") * 10.0).floor();
                    let v = self.c.inc(list, c, &["WaveOfConvictionDurationDotMulti"])? * d / 100.0;
                    let source = format!("Skill:{id}");
                    self.c.add(
                        list,
                        Mod::new("DotMultiplier", ModType::Base, v, Some(&source))
                            .tag(Tag::multiplier("WoCDurationExpired")),
                    );
                }
            }
            ("WinterOrb", "preDamageFunc") => {
                let rate = 1.0 + self.c.inc(list, c, &["HitRate", "Speed"])? / 100.0;
                let more = self.c.more(list, c, &["HitRate"])?;
                let v = sd(self, "repeatFrequency") / rate / more;
                self.sd_set(i, "hitTimeOverride", v);
            }
            ("InfernalBlow" | "InfernalBlowAltX", "preDamageFunc") => {
                let effect = self.c.base(list, c, &["DebuffEffect"])?;
                if part == Some(2.0) || part == Some(3.0) {
                    let source = format!("Skill:{id}");
                    self.c.add(
                        list,
                        Mod::new("Damage", ModType::More, effect, Some(&source))
                            .tag(Tag::multiplier("DebuffStack").base(-100.0 + effect)),
                    );
                }
                if part == Some(3.0) {
                    self.sd_set(i, "dpsMultiplier", 1.0 / 6.0);
                }
            }
            ("Perforate" | "PerforateAltX" | "PerforateAltY", "preDamageFunc") => {
                let v = m_max(dps_mult(self), 1.0);
                self.sd_set(i, "dpsMultiplier", v);
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
            ("StaticStrike", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = sd(self, "repeatFrequency")
                        / (1.0 + self.c.inc(list, c, &["StaticStrikeFrequency"])? / 100.0);
                    self.sd_set(i, "hitTimeOverride", v);
                }
            }
            ("GAZombieCorpseGroundImpact" | "MinionInstability", "preDamageFunc") => {
                let summon = self
                    .minion_of_skill(i)
                    .map(|m| self.minions[m].skill)
                    .ok_or("attempt to index a nil value (summonSkill)")?;
                let speed = self.sd_num(summon, "summonSpeed").unwrap_or(1.0);
                self.sd_set(i, "hitTimeOverride", 1.0 / speed);
            }
            ("BloodSacramentUnique", "initialFunc") => {
                if out(self, "LifeReservedPercent") >= 100.0 {
                    return Ok(());
                }
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
            ("SeismicTrap" | "SeismicTrapAltX", "preDamageFunc") => {
                let mut active_traps = f64::NAN;
                if id == "SeismicTrap" {
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
                    active_traps = pulses / rate / out(self, "TrapCooldown");
                    self.c.set_out(actor, "AverageActiveTraps", active_traps);
                }
                let enemy_radius = match self.c.override_num(list, c, &["EnemyRadius"])? {
                    Some(v) => v,
                    None => self.c.base(list, c, &["EnemyRadius"])?,
                };
                let r = out(self, "AreaOfEffectRadiusSecondary") + enemy_radius - 1.0;
                let full = out(self, "AreaOfEffectRadius");
                let overlap = m_min(r * r / (full * full), 1.0);
                self.c.set_out(actor, "OverlapChance", overlap * 100.0);
                let waves = self.c.base(list, c, &["MaximumWaves"])?;
                let mult = match part {
                    Some(2.0) => waves * overlap,
                    Some(3.0) => waves,
                    Some(4.0) if id == "SeismicTrap" => active_traps,
                    Some(5.0) if id == "SeismicTrap" => active_traps * waves * overlap,
                    Some(6.0) if id == "SeismicTrap" => active_traps * waves,
                    _ => 1.0,
                };
                self.scale_dps(i, actor, mult);
            }
            ("BallLightning", "preDamageFunc") => {
                let mult = match part {
                    Some(1.0) => 1.0,
                    Some(2.0) => {
                        let per = sd(self, "strikeInterval");
                        let max_strikes = (sd(self, "duration") / per).floor();
                        let inc = 1.0 + self.c.inc(list, c, &["ProjectileSpeed"])? / 100.0;
                        let more = self.c.more(list, c, &["ProjectileSpeed"])?;
                        let per_strike = sd(self, "projectileSpeed") * (inc * more) * per;
                        let radius = out(self, "AreaOfEffectRadius");
                        let dist = match cfg.skill_dist {
                            Some(d) => d,
                            None if self.has_flag(i, "triggered") => (radius / 2.0).floor(),
                            None => radius,
                        };
                        let first = m_max(1.0, ((dist - radius) / per_strike).ceil());
                        let last = (m_min(self.misc_data("ProjectileDistanceCap"), dist + radius)
                            / per_strike)
                            .floor();
                        m_max(0.0, m_min(max_strikes, last + 1.0 - first))
                    }
                    _ => 0.0,
                };
                self.scale_dps(i, actor, mult);
            }
            ("BallLightningAltX", "preDamageFunc") => {
                if part == Some(2.0) || part == Some(3.0) {
                    let strikes = (sd(self, "duration") / sd(self, "strikeInterval")).floor();
                    self.scale_dps(i, actor, strikes);
                    self.c.set_out(actor, "NormalHitsPerCast", strikes);
                }
            }
            ("KineticFusillade" | "KineticFusilladeAltX", "preDamageFunc") => {
                if part == Some(1.0) {
                    let n = out(self, "ProjectileCount");
                    self.sd_set(i, "dpsMultiplier", n);
                    let per = self.sd_num(i, "damagePerProjectile").unwrap_or(0.0);
                    if per != 0.0 && n > 1.0 {
                        let avg = per * (n - 1.0) / 2.0;
                        let source = format!("Skill:{id}");
                        self.c
                            .add(list, Mod::new("Damage", ModType::More, avg, Some(&source)));
                        self.c.set_out(actor, "KineticFusilladeAvgMoreMult", avg);
                    }
                }
            }
            ("KineticFusillade" | "KineticFusilladeAltX", "postCritFunc") => {
                let n = if part == Some(1.0) {
                    out(self, "ProjectileCount")
                } else {
                    1.0
                };
                let delay = (sd(self, "duration") + sd(self, "delayPerProjectile") * (n - 1.0))
                    * out(self, "DurationMod");
                let tick = self.misc_data("ServerTickTime");
                let max_aps = 1.0 / ((delay / tick).ceil() * tick);
                self.c
                    .set_out(actor, "KineticFusilladeMaxEffectiveAPS", max_aps);
                if part == Some(1.0)
                    && let Some(aps) = self.c.out(actor, "Speed")
                    && aps > max_aps
                {
                    let orig = self
                        .sd_num(i, "dpsMultiplier")
                        .unwrap_or(out(self, "ProjectileCount"));
                    self.sd_set(i, "dpsMultiplier", orig * (max_aps / aps));
                }
            }
            (
                "LightningSpireTrap" | "LightningSpireTrapAltX" | "LightningSpireTrapAltY",
                "preDamageFunc",
            ) => {
                let radius = sd(self, "radius");
                self.c.set_out(actor, "AreaOfEffectRadius", radius);
                let rate = if id == "LightningSpireTrap" {
                    let inc = 1.0 + self.c.inc(list, c, &["TrapThrowingSpeed"])? / 100.0;
                    let more = self.c.more(list, c, &["TrapThrowingSpeed"])?;
                    inc * more / sd(self, "repeatInterval")
                } else {
                    1.0 / sd(self, "repeatInterval")
                };
                self.sd_set(i, "hitTimeOverride", 1.0 / rate);
                if id == "LightningSpireTrap" {
                    self.c.set_out(actor, "WavePulseRate", rate);
                }
                let dinc = 1.0 + self.c.inc(list, c, &["Duration"])? / 100.0;
                let dmore = self.c.more(list, c, &["Duration"])?;
                let duration = sd(self, "duration") * dinc * dmore;
                let pulses = (duration * rate).floor();
                self.c.set_out(actor, "PulsesPerTrap", pulses);
                let effective = pulses / rate;
                let active_traps = if id == "LightningSpireTrapAltX" {
                    let action = 1.0 + self.c.inc(list, c, &["ActionSpeed"])? / 100.0;
                    let base_speed = 1.0 / self.c.base(list, c, &["TrapThrowingTime"])?;
                    let throw = m_min(
                        base_speed * self.c.calc_mod(list, c, &["TrapThrowingSpeed"])? * action,
                        self.misc_data("ServerTickRate"),
                    );
                    effective / (1.0 / throw)
                } else {
                    effective / out(self, "TrapCooldown")
                };
                self.c.set_out(actor, "AverageActiveTraps", active_traps);
                let enemy_radius = match self.c.override_num(list, c, &["EnemyRadius"])? {
                    Some(v) => v,
                    None => self.c.base(list, c, &["EnemyRadius"])?,
                };
                let r = out(self, "AreaOfEffectRadiusSecondary") + enemy_radius - 1.0;
                let overlap = m_min(r * r / (radius * radius), 1.0);
                self.c.set_out(actor, "OverlapChance", overlap * 100.0);
                let waves = self.c.base(list, c, &["MaximumWaves"])?;
                let mult = match part {
                    Some(2.0) => waves * overlap,
                    Some(3.0) => waves,
                    Some(4.0) => active_traps,
                    Some(5.0) => active_traps * waves * overlap,
                    Some(6.0) => active_traps * waves,
                    _ => 1.0,
                };
                self.scale_dps(i, actor, mult);
            }
            ("FrozenSweep" | "FrozenSweepAltX", "preDamageFunc") => {
                self.sd_set(i, "showAverage", false);
                self.set_flag(i, "showAverage", false);
                self.set_flag(i, "notAverage", true);
                let legion = if id == "FrozenSweep" {
                    "Frozen Legion"
                } else {
                    "Frozen Legion of Rallying"
                };
                let main = self.actor_main_index(actor).ok_or("no main skill")?;
                let slot = |p: &Self, k: usize| {
                    p.skill(k)
                        .group
                        .and_then(|g| p.env.groups.get(g))
                        .and_then(|g| g.slot.clone())
                };
                let parent = self
                    .actor_skills(actor)
                    .into_iter()
                    .find(|&k| self.skill_name(k) == legion && slot(self, main) == slot(self, i))
                    .ok_or("attempt to index a nil value (parentSkill)")?;
                let (pl, pc) = (self.lists[parent].list, self.cfg(parent));
                let pc = Some(&pc);
                if self.c.flag(pl, pc, &["DisableSkill"])?
                    && !self.c.flag(pl, pc, &["EnableSkill"])?
                {
                    return Ok(());
                }
                let pcd = self.sd(parent, "cooldown").cloned();
                match &pcd {
                    Some(v) => self.sd_set(i, "cooldown", v.clone()),
                    None => self.sd_clear(i, "cooldown"),
                }
                let cooldown = match self.c.override_num(pl, pc, &["CooldownRecovery"])? {
                    Some(v) => v,
                    None => {
                        (self.sd_num(parent, "cooldown").unwrap_or(f64::NAN)
                            + self.c.base(pl, pc, &["CooldownRecovery"])?)
                            / m_max(0.0, self.c.calc_mod(pl, pc, &["CooldownRecovery"])?)
                    }
                };
                let tick = self.misc_data("ServerTickRate");
                let cd = (cooldown * tick).ceil() / tick;
                self.c.set_out(actor, "Cooldown", cd);
                self.sd_set(i, "hitTimeOverride", cd);
                let max = self.sd_num(parent, "storedUses").unwrap_or(f64::NAN)
                    + self.c.base(pl, pc, &["AdditionalCooldownUses"])?;
                match part {
                    Some(1.0) => self.sd_set(i, "averageBurstHits", 1.0),
                    Some(2.0) => self.sd_set(i, "averageBurstHits", max),
                    _ => self.sd_clear(i, "averageBurstHits"),
                }
            }
            ("MoltenStrike" | "MoltenStrikeAltX", "preDamageFunc") => {
                if part == Some(1.0) {
                    return Ok(());
                }
                let enemy_radius = match self.c.override_num(list, c, &["EnemyRadius"])? {
                    Some(v) => v,
                    None => self.c.base(list, c, &["EnemyRadius"])?,
                };
                let inner = out(self, "AreaOfEffectRadiusSecondary");
                let hit = enemy_radius + out(self, "AreaOfEffectRadius") - inner;
                let landing = out(self, "AreaOfEffectRadiusTertiary") - inner;
                let overlap = m_min(1.0, hit / landing);
                self.c.set_out(actor, "OverlapChance", overlap * 100.0);
                let n = out(self, "ProjectileCount");
                let mut mult = 1.0;
                let alt = id == "MoltenStrikeAltX";
                if part == Some(3.0) || (alt && (part == Some(5.0) || part == Some(6.0))) {
                    mult = overlap * n;
                    if alt && part == Some(6.0) {
                        let fifth = 1.0 + sd(self, "FifthStrikeDamage") / 100.0;
                        let fifth_all = fifth * overlap * (n + sd(self, "FifthStrikeProjectiles"));
                        mult = 0.8 * mult + 0.2 * fifth_all;
                    }
                }
                self.scale_dps(i, actor, mult);
            }
            ("ExplosiveArrow", "explosiveArrowFunc") => {
                if part != Some(1.0) && part != Some(2.0) {
                    return Ok(());
                }
                let duration = self.skill_duration(list, c, i)?;
                let fuse_limit = self.c.base(list, c, &["ExplosiveArrowMaxFuseCount"])?;
                let totem = self.has_flag(i, "totem");
                let totems = if totem {
                    match self.c.override_num(self.pdb(), None, &["TotemsSummoned"])? {
                        Some(v) => v,
                        None => {
                            self.c
                                .base(list, c, &["ActiveTotemLimit", "ActiveBallistaLimit"])?
                        }
                    }
                } else {
                    f64::NAN
                };
                let f = |p: &Self, k: &str| p.c.flag(list, None, &[k]);
                let mut barrage = None;
                if f(self, "SequentialProjectiles")?
                    && !f(self, "OneShotProj")?
                    && !f(self, "NoAdditionalProjectiles")?
                    && !f(self, "TriggeredBySnipe")?
                {
                    let b = self.c.base(list, c, &["ProjectileCount"])?;
                    let v = sd(self, "dpsMultiplier") / b;
                    self.sd_set(i, "dpsMultiplier", v);
                    barrage = Some(b);
                }
                let mut rate = (self.po(o, "HitChance").unwrap_or(f64::NAN) / 100.0)
                    * self.go(o, "Speed").unwrap_or(f64::NAN)
                    * sd(self, "dpsMultiplier")
                    * barrage.unwrap_or(1.0);
                if totem {
                    rate *= totems;
                }
                if part == Some(2.0) {
                    let maximum = m_min((rate * duration).floor() + 1.0, fuse_limit);
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:ExplosiveArrowStage",
                            ModType::Base,
                            maximum,
                            Some("Base"),
                        ),
                    );
                    self.c.add(
                        list,
                        Mod::new(
                            "Multiplier:ExplosiveArrowStageAfterFirst",
                            ModType::Base,
                            maximum - 1.0,
                            Some("Base"),
                        ),
                    );
                    self.set_go(o, "MaxExplosiveArrowFuseCalculated", maximum);
                } else {
                    self.c.clear_out(actor, "MaxExplosiveArrowFuseCalculated");
                }
                let to_max = fuse_limit / rate;
                let stages = self.mods(i).active_stage_count.unwrap_or(0.0);
                let hit_time =
                    if part == Some(2.0) || (part == Some(1.0) && stages + 1.0 >= fuse_limit) {
                        m_min(duration, to_max)
                    } else {
                        duration
                    };
                self.set_go(o, "HitTime", hit_time);
                self.set_go(o, "HitSpeed", 1.0 / hit_time);
            }
            ("AvengingFlame", "preDamageFunc") => {
                let totem_life = match (self.sd(i, "triggerSourceUUID"), self.input.calcs) {
                    (Some(ModValue::Str(u)), Some(calcs)) => calcs
                        .cached(crate::calcs::Mode::Main, u)
                        .or_else(|| calcs.cached(crate::calcs::Mode::Calcs, u))
                        .and_then(|c| c.num("TotemLife"))
                        .unwrap_or(0.0),
                    _ => 0.0,
                };
                let add = totem_life * sd(self, "lifeDealtAsFire") / 100.0;
                for k in ["FireMax", "FireMin"] {
                    let v = self.sd_num(i, k).unwrap_or(0.0) + add;
                    self.sd_set(i, k, v);
                }
            }
            ("DeathWish", "preDamageFunc") => {
                if part == Some(2.0) {
                    let v = out(self, "Life") * sd(self, "selfFireExplosionLifeMultiplier");
                    self.sd_set(i, "FireBonusMin", v);
                    self.sd_set(i, "FireBonusMax", v);
                }
            }
            _ => return Err(format!("not ported: {name} of {id}")),
        }
        Ok(())
    }

    /// `preDamageFunc` of `EnemyExplode`
    fn enemy_explode(&mut self, o: &mut Off) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let part = self.mods(i).skill_part;
        let mut chance_out = 0.0;
        let rec = |v: &ModValue, k: &str| match v {
            ModValue::Record(r) => r.get(k).cloned(),
            _ => None,
        };
        let rnum =
            |v: &ModValue, k: &str| rec(v, k).and_then(|x| x.as_number()).unwrap_or(f64::NAN);
        let rstr = |v: &ModValue, k: &str| match rec(v, k) {
            Some(ModValue::Str(s)) => s.to_string(),
            _ => String::new(),
        };
        if part != Some(3.0) {
            let source = {
                let s = self.skill(i);
                let gem = s
                    .active_effect
                    .src
                    .and_then(|(g, k)| self.env.groups.get(g)?.gem_list.get(k))
                    .ok_or("attempt to index a nil value (srcInstance)")?;
                match gem.explode_source.as_ref().ok_or("no explodeSource")? {
                    crate::skills::ExplodeSource::Item(id) => {
                        let item = self.input.tab.item(*id).ok_or("no explode source item")?;
                        item.mod_source.clone()
                    }
                    crate::skills::ExplodeSource::Node(n) => {
                        format!("Tree:{n}")
                    }
                }
            };
            for (v, m) in self
                .c
                .tabulate(list, Some(ModType::List), c, &["ExplodeMod"])?
            {
                if m.source.as_deref() != Some(source.as_str()) {
                    continue;
                }
                let typ = rstr(&v, "type");
                let amount = rnum(&v, "amount");
                if typ == "RandomElement" {
                    for e in ["Fire", "Cold", "Lightning"] {
                        self.sd_set(i, &format!("{e}EffectiveExplodePercentage"), amount / 3.0);
                    }
                } else {
                    self.sd_set(i, &format!("{typ}EffectiveExplodePercentage"), amount);
                }
                chance_out = if part == Some(2.0) {
                    1.0
                } else {
                    rnum(&v, "chance")
                };
            }
        } else {
            // By type, then amount, in the order first met
            let mut types: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
            for v in self.c.list(list, c, &["ExplodeMod"])? {
                let typ = rstr(&v, "type");
                let (amount, chance) = (rnum(&v, "amount"), rnum(&v, "chance"));
                let k = match types.iter().position(|t| t.0 == typ) {
                    Some(k) => k,
                    None => {
                        types.push((typ, Vec::new()));
                        types.len() - 1
                    }
                };
                match types[k].1.iter_mut().find(|a| a.0 == amount) {
                    Some(a) => a.1 += chance,
                    None => types[k].1.push((amount, chance)),
                }
            }
            for (typ, amounts) in types {
                let mut phys = 0.0;
                for (amount, chance) in amounts {
                    let ac = amount * chance;
                    let add = |p: &mut Self, k: &str, v: f64| {
                        let cur = p.sd_num(i, k).unwrap_or(0.0);
                        p.sd_set(i, k, cur + v);
                    };
                    if typ == "RandomElement" {
                        for e in ["Fire", "Cold", "Lightning"] {
                            add(self, &format!("{e}EffectiveExplodePercentage"), ac / 3.0);
                        }
                    } else {
                        add(self, &format!("{typ}EffectiveExplodePercentage"), ac);
                    }
                    if typ == "Physical" {
                        phys = 1.0 - ((1.0 - phys) * (1.0 - chance));
                    }
                    chance_out = 1.0 - ((1.0 - chance_out) * (1.0 - chance));
                }
                if typ == "Physical" && phys != 0.0 {
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
        self.c
            .set_out(actor, "ExplodeChance", m_min(chance_out * 100.0, 100.0));
        Ok(())
    }

    /// `skillData.dpsMultiplier` and `output.SkillDPSMultiplier` times a
    /// multiplier other than 1
    fn scale_dps(&mut self, i: usize, actor: usize, mult: f64) {
        if mult != 1.0 {
            let v = self.sd_num(i, "dpsMultiplier").unwrap_or(1.0) * mult;
            self.sd_set(i, "dpsMultiplier", v);
            let o = self.c.out(actor, "SkillDPSMultiplier").unwrap_or(1.0) * mult;
            self.c.set_out(actor, "SkillDPSMultiplier", o);
        }
    }
}
