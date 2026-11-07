//! The skills' own offence functions (`initialFunc`, `preDamageFunc`, ...
//! of `Data/Skills/*.lua`), ported by hand per granted effect id.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::calc::{m_max, m_min};
use crate::calc_offence::Off;
use crate::calc_perform::Perform;
use crate::mods::{Mod, ModType, ModValue};

pub type Result<T> = std::result::Result<T, String>;

impl<'a> Perform<'a> {
    /// Runs the function `name` of the granted effect `id`
    pub(crate) fn skill_func(&mut self, o: &mut Off, id: &str, name: &str) -> Result<()> {
        let (i, list, actor) = (o.skill, o.list, o.actor);
        let cfg = self.cfg(i);
        let c = Some(&cfg);
        let sd = |p: &Self, k: &str| p.sd_num(i, k).unwrap_or(f64::NAN);
        let part = self.mods(i).skill_part;
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
}
