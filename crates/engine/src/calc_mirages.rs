//! `calcs.mirages` (`Modules/CalcMirages.lua`): mirages that use the
//! player's skills, each a calculation of a copy of the skill
//! ([`crate::calcs::Calcs::mirage_perform`]).
//!
//! Not ported yet: Tawhoa's Chosen.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::calc::{PLAYER, m_max};
use crate::calc_perform::{Perform, Result, src};
use crate::calc_skill_mods::MirageInfo;
use crate::calcs::{MirageOut, skill_uuid};
use crate::mods::{Mod, ModType, Sym};

/// A mod with flags and keyword flags, as `NewMod(name, type, value,
/// source, flags, keywordFlags)` adds it
fn flagged(m: &Mod, name: &str) -> Mod {
    let mut d = Mod::new(name, m.typ, m.value.clone(), m.source.as_deref());
    d.flags = m.flags;
    d.keyword_flags = m.keyword_flags;
    d
}

impl<'a> Perform<'a> {
    fn used_by_mirage(&self, i: usize) -> bool {
        self.cfg(i)
            .skill_cond
            .as_ref()
            .is_some_and(|c| c.contains(&Sym::new("usedByMirage")))
    }

    /// A mirage's calculation of a skill's copy: `pre` adds its mods
    fn mirage_calc(&self, j: usize, pre: Vec<Mod>) -> Result<MirageOut> {
        let calcs = self
            .input
            .calcs
            .ok_or("not ported: mirages without the calculations' cache")?;
        let build = self
            .input
            .build
            .ok_or("not ported: mirages without the build")?;
        let uuid = skill_uuid(self, j);
        let uses = self.sd_num(self.main(), "storedUses");
        calcs.mirage_perform(
            build,
            &uuid,
            uses,
            &self.input.limited,
            Box::new(move |p: &mut Perform, i: usize| {
                let list = p.lists[i].list;
                for m in pre {
                    p.c.add(list, m);
                }
                Ok(())
            }),
        )
    }

    /// `calcs.mirages(env)`: whether the main skill's own offence is left
    /// out
    pub(crate) fn mirages(&mut self) -> Result<bool> {
        let pm = self.main();
        if self.used_by_mirage(pm) || self.has_flag(pm, "disable") {
            return Ok(false);
        }
        let name = self.skill_name(pm).to_string();
        if self.sd_truthy(pm, "triggeredByMirageArcher") {
            return self.mirage_like(
                "MirageArcherLessDamage",
                "MirageArcherLessAttackSpeed",
                "Mirage Archer",
                "Used by mirage",
                "mirageArcher",
            );
        }
        if name == "Reflection" {
            return self.saviour();
        }
        if name == "Tawhoa's Chosen" {
            return Err("not ported: mirages (Tawhoa's Chosen)".into());
        }
        if self.sd_truthy(pm, "triggeredBySacredWisps") {
            return self.mirage_like(
                "SacredWispsLessDamage",
                "",
                "Used by Sacred Wisps",
                "Used by Sacred Wisps",
                "wisp",
            );
        }
        if self.sd_truthy(pm, "triggeredByGeneralsCry") {
            self.generals_cry()?;
        }
        Ok(false)
    }

    /// Mirage Archer and Sacred Wisps: the main skill's copy, whose output
    /// Full DPS adds
    fn mirage_like(
        &mut self,
        less_damage: &str,
        less_speed: &str,
        damage_source: &str,
        cost_source: &str,
        flag: &str,
    ) -> Result<bool> {
        let pm = self.main();
        let (ml, mc) = (self.lists[pm].list, self.cfg(pm));
        let wisps = flag == "wisp";
        let (count, speed) = if wisps {
            let mut count = f64::NAN;
            let mut chance = f64::NAN;
            if let Some(k) =
                (0..self.n_player).find(|&k| self.skill_name(k) == "Summon Sacred Wisps")
            {
                let kl = self.lists[k].list;
                chance = self.c.base(kl, Some(&mc), &["SacredWispsChance"])?;
                count = self.c.base(kl, Some(&mc), &["SacredWispsMaxCount"])?;
            }
            (count, Some(chance - 100.0))
        } else {
            (f64::NAN, None)
        };
        let name = self.skill_name(pm).to_string();
        let less_damage = less_damage.to_string();
        let less_speed = less_speed.to_string();
        let damage_source = damage_source.to_string();
        let cost_source = cost_source.to_string();
        let outer_cfg = mc.clone();
        let calcs = self
            .input
            .calcs
            .ok_or("not ported: mirages without the calculations' cache")?;
        let build = self
            .input
            .build
            .ok_or("not ported: mirages without the build")?;
        let uuid = skill_uuid(self, pm);
        let uses = self.sd_num(pm, "storedUses");
        let counted = std::rc::Rc::new(std::cell::Cell::new(count));
        let counted_in = counted.clone();
        let out = calcs.mirage_perform(
            build,
            &uuid,
            uses,
            &self.input.limited,
            Box::new(move |p: &mut Perform, i: usize| {
                let list = p.lists[i].list;
                let cfg = p.cfg(i);
                let damage = if wisps {
                    p.c.base(list, Some(&outer_cfg), &[&less_damage])?
                } else {
                    p.c.base(list, Some(&cfg), &[&less_damage])?
                };
                p.c.add(
                    list,
                    Mod::new("Damage", ModType::More, damage, Some(&damage_source)),
                );
                match speed {
                    Some(s) => p.c.add(
                        list,
                        Mod::new("Speed", ModType::More, s, Some("Sacred Wisps cast chance")),
                    ),
                    None => {
                        let s = p.c.base(list, Some(&cfg), &[&less_speed])?;
                        let n =
                            p.c.base(list, Some(&outer_cfg), &["MirageArcherMaxCount"])?;
                        counted_in.set(n);
                        p.c.add(
                            list,
                            Mod::new("Speed", ModType::More, s, Some(&damage_source)),
                        );
                    }
                }
                p.c.add(list, src("HasNoCost", ModType::Flag, true, &cost_source));
                Ok(())
            }),
        )?;
        let count = counted.get();
        let label = if wisps {
            "Sacred Wisps"
        } else {
            "Mirage Archers"
        };
        let mm = self.mods_mut(pm);
        if mm.info_message.is_none() {
            mm.info_message = Some(format!("{} {label} using {name}", crate::pat::num(count)));
        }
        mm.mirage = Some(MirageInfo {
            name,
            count: Some(count),
            skill_part_name: out.skill_part_name.clone(),
            output: out.outputs.player,
        });
        self.set_flag(pm, flag, true);
        let _ = ml;
        Ok(false)
    }

    /// The Saviour's Reflection: the best one-handed sword attack's copy
    /// takes over the main skill and its output
    fn saviour(&mut self) -> Result<bool> {
        let pm = self.main();
        let (ml, mc) = (self.lists[pm].list, self.cfg(pm));
        let mut warriors = self
            .c
            .base(ml, Some(&mc), &["SaviourMirageWarriorMaxCount"])?;
        let calcs = self
            .input
            .calcs
            .ok_or("not ported: mirages without the calculations' cache")?;
        let build = self
            .input
            .build
            .ok_or("not ported: mirages without the build")?;
        let mode = self.input.mode;
        let mut best: Option<(usize, f64)> = None;
        let sword = self.mf("Sword") | self.mf("Weapon1H");
        for k in 0..self.n_player {
            if self.used_by_mirage(k) {
                continue;
            }
            if k != pm
                && self.has_type(k, "Attack")
                && !self.has_type(k, "Totem")
                && !self.has_type(k, "SummonsTotem")
                && self.cfg(k).flags.unwrap_or(0) & sword == sword
            {
                let uuid = skill_uuid(self, k);
                if calcs.cached(mode, &uuid).is_none() {
                    calcs.build_active_skill(build, mode, &uuid, &self.input.limited)?;
                }
                if let Some(c) = calcs.cached(mode, &uuid)
                    && c.num("CritChance").is_some_and(|v| v > 0.0)
                {
                    let dps = c.num("TotalDPS").unwrap_or(f64::NAN);
                    match best {
                        None => best = Some((k, dps)),
                        Some((_, b)) if dps > b => best = Some((k, dps)),
                        _ => {}
                    }
                }
            }
        }
        let Some((j, _)) = best else {
            let mm = self.mods_mut(pm);
            mm.disable_reason = Some("No Saviour active skill found".into());
            mm.skill_flags.insert("disable".into());
            return Ok(true);
        };
        let damage = self
            .c
            .base(ml, Some(&mc), &["SaviourMirageWarriorLessDamage"])?;
        let same = match (self.item(PLAYER, "Weapon 1"), self.item(PLAYER, "Weapon 2")) {
            (Some(a), Some(b)) => a.name == b.name,
            _ => false,
        };
        if same {
            warriors /= 2.0;
        }
        let pre = vec![
            Mod::new("Damage", ModType::More, damage, Some("The Saviour")),
            Mod::new(
                "QuantityMultiplier",
                ModType::Base,
                warriors,
                Some("The Saviour Mirage Warriors"),
            ),
            src("HasNoCost", ModType::Flag, true, "Used by mirage"),
        ];
        let out = self.mirage_calc(j, pre)?;
        // `env.player.mainSkill = newSkill`, `env.player.output =
        // newEnv.player.output`
        self.c.actors.actors[PLAYER].output = out.outputs.player;
        self.replaced_main = Some((skill_uuid(self, j), out.skill_data));
        Ok(true)
    }

    /// General's Cry: the main skill's mirages, on the skill itself
    fn generals_cry(&mut self) -> Result<()> {
        let pm = self.main();
        let uuid = skill_uuid(self, pm);
        if self.input.limited.contains(&uuid) {
            return Ok(());
        }
        let st = |p: &Self, n: &str| p.input.data.skills.skill_type(n).unwrap_or(-1);
        let (triggered, other) = (st(self, "Triggered"), st(self, "OtherThingUsesSkill"));
        {
            let s = &mut self.env.skills.active_skills[pm];
            s.skill_types.insert(triggered);
            s.skill_types.insert(other);
            let types = s.skill_types.clone();
            let mods = s.mods.as_mut().ok_or("a skill without its mod list")?;
            mods.skill_cfg
                .skill_cond
                .get_or_insert_with(Default::default)
                .insert(Sym::new("usedByMirage"));
            mods.skill_cfg.skill_types = Some(types.clone());
            for c in mods.weapon_cfgs.iter_mut().flatten() {
                c.skill_types = Some(types.clone());
            }
        }
        let calcs = self
            .input
            .calcs
            .ok_or("not ported: mirages without the calculations' cache")?;
        let build = self
            .input
            .build
            .ok_or("not ported: mirages without the build")?;
        let mode = self.input.mode;
        if calcs.cached(mode, &uuid).is_none() || mode == crate::calcs::Mode::Calculator {
            let mut limited = self.input.limited.clone();
            limited.push(uuid.clone());
            calcs.build_active_skill(build, mode, &uuid, &limited)?;
        }
        let cache = calcs
            .cached(mode, &uuid)
            .ok_or("attempt to index a nil value (cachedData)")?;
        let slot = |p: &Self, k: usize| {
            p.skill(k)
                .group
                .and_then(|g| p.env.groups.get(g))
                .map(|g| g.slot.clone())
        };
        let mut cooldown = 1.0;
        let mut gc = None;
        for k in 0..self.n_player {
            if self.skill_name(k) == "General's Cry" {
                let ks = slot(self, k).ok_or("attempt to index a nil value (socketGroup)")?;
                if ks == slot(self, pm).ok_or("attempt to index a nil value (socketGroup)")? {
                    let (kl, kc) = (self.lists[k].list, self.cfg(k));
                    cooldown = self.skill_cooldown(kl, &kc, k)?.0;
                    gc = Some(k);
                    break;
                }
            }
        }
        let gc = gc.ok_or("attempt to index a nil value (generalsCryActiveSkill)")?;
        let (gl, gcfg) = (self.lists[gc].list, self.cfg(gc));
        let (ml, mc) = (self.lists[pm].list, self.cfg(pm));
        let mut warriors = 0.0;
        for (_, m) in self.c.tabulate(
            gl,
            Some(ModType::Base),
            Some(&gcfg),
            &["GeneralsCryDoubleMaxCount"],
        )? {
            warriors += crate::calc::number(&m.value)?;
            self.c.add(ml, flagged(&m, "QuantityMultiplier"));
        }
        let mut spawn = 0.3 + 0.2 * warriors;
        if self.has_type(pm, "Channel") {
            spawn += 1.0;
        } else {
            self.sd_set(pm, "hitTimeOverride", 1.0);
        }
        let spawn = crate::calc::round(spawn, 2.0);
        cooldown = m_max(
            cooldown,
            cache
                .num("HitTime")
                .or(cache.num("Time"))
                .unwrap_or(f64::NAN),
        );
        self.c.add(
            ml,
            src(
                "DPS",
                ModType::More,
                (1.0 / cooldown - 1.0) * 100.0,
                "General's Cry Cooldown",
            ),
        );
        self.c
            .add(ml, src("HasNoCost", ModType::Flag, true, "Used by mirage"));
        let mc = {
            let mut c = mc;
            c.skill_cond
                .get_or_insert_with(Default::default)
                .insert(Sym::new("usedByMirage"));
            c
        };
        for (typ, name, to) in [
            (ModType::Inc, "ExertIncrease", "Damage"),
            (ModType::More, "ExertIncrease", "Damage"),
            (ModType::More, "ExertAttackIncrease", "Damage"),
            (ModType::More, "OverexertionExertAverageIncrease", "Damage"),
            (
                ModType::Base,
                "ExertDoubleDamageChance",
                "DoubleDamageChance",
            ),
        ] {
            for (_, m) in self.c.tabulate(ml, Some(typ), Some(&mc), &[name])? {
                self.c.add(ml, flagged(&m, to));
            }
        }
        let skill = self.skill_name(pm).to_string();
        let w = crate::pat::num(warriors);
        let mm = self.mods_mut(pm);
        mm.info_message = Some(format!("{w} GC Mirages using {skill}"));
        mm.info_message2 = Some(format!(
            "{}s for {w} Mirages to finish Attacking",
            crate::pat::num(spawn)
        ));
        Ok(())
    }
}
