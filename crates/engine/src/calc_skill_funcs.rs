//! The skills' own offence functions (`initialFunc`, `preDamageFunc`, ...
//! of `Data/Skills/*.lua`), ported by hand per granted effect id.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::calc::{m_max, m_min};
use crate::calc_offence::Off;
use crate::calc_perform::Perform;
use crate::mods::{Mod, ModType};

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
            _ => return Err(format!("not ported: {name} of {id}")),
        }
        Ok(())
    }
}
