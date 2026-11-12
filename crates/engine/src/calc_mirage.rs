//! `calcs.mirages` (`Modules/CalcMirages.lua`): Mirage Archer, The
//! Saviour's mirage warriors and Sacred Wisps use a copy of a skill
//! (`calcs.copyActiveSkill`), calculated in a calculation of its own.
//!
//! The copy's mod list stands on the database of the calculation the skill
//! comes from, as upstream's copy keeps the skill's actor: that database and
//! its enemy's are copied into the mirage's calculation, with stand-in
//! actors, and the copy is its main skill, outside the player's skills.
//!
//! Not ported yet: Tawhoa's Chosen, mirages of skills with minions.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::actor::{Actor, OutVal};
use crate::calc::{ENEMY, PLAYER};
use crate::calc_cache::CachedCalc;
use crate::calc_perform::{Perform, Result, SkillLists, SkillMarks};
use crate::calc_skills::ActiveSkill;
use crate::mods::{Cfg, Mod, ModType, Store, Sym};
use std::collections::HashMap;

/// A copy of a skill for a mirage, with the databases it stands on
pub struct External {
    pub skill: ActiveSkill,
    /// The skill's actor's database and its enemy's, and the actors
    pub player_store: Store,
    pub enemy_store: Store,
    pub player_actor: Actor,
    pub enemy_actor: Actor,
    /// The mirage's own mods on the copy's `skillModList`
    pub list_mods: Vec<Mod>,
    /// What the mirage adds once the copy's mod list is built
    pub kind: MirageKind,
    /// `env.player.mainSkill.skillData.storedUses` (`mirageUses`)
    pub mirage_uses: Option<f64>,
}

/// The mods each mirage adds to its copy (`preCalcFunc`)
#[derive(Clone, Debug)]
pub enum MirageKind {
    /// Mirage Archer: less damage and attack speed from the copy's mods
    Archer,
    /// The Saviour: from the main skill, more damage and the warriors
    Saviour { more_damage: f64, warriors: f64 },
    /// Sacred Wisps: the less damage queried with the main skill's cfg,
    /// and the cast chance
    Wisps {
        main_cfg: Box<Cfg>,
        cast_chance: f64,
    },
}

/// A mirage's results (`activeSkill.mirage`)
#[derive(Clone, Debug)]
pub struct Mirage {
    /// The copied skill's name and part
    pub name: String,
    pub skill_part_name: Option<String>,
    pub output: HashMap<Sym, OutVal>,
    pub count: Option<f64>,
}

impl MirageKind {
    /// The mirage's mods; `sum` queries the copy's `skillModList` with a cfg
    pub(crate) fn mods(&self, sum: &dyn Fn(Option<&Cfg>, &str) -> Result<f64>) -> Result<Vec<Mod>> {
        let flag = |source: &str| Mod::new("HasNoCost", ModType::Flag, true, Some(source));
        Ok(match self {
            MirageKind::Archer => {
                let damage = sum(None, "MirageArcherLessDamage")?;
                let speed = sum(None, "MirageArcherLessAttackSpeed")?;
                vec![
                    Mod::new("Damage", ModType::More, damage, Some("Mirage Archer")),
                    Mod::new("Speed", ModType::More, speed, Some("Mirage Archer")),
                    flag("Used by mirage"),
                ]
            }
            MirageKind::Saviour {
                more_damage,
                warriors,
            } => vec![
                Mod::new("Damage", ModType::More, *more_damage, Some("The Saviour")),
                Mod::new(
                    "QuantityMultiplier",
                    ModType::Base,
                    *warriors,
                    Some("The Saviour Mirage Warriors"),
                ),
                flag("Used by mirage"),
            ],
            MirageKind::Wisps {
                main_cfg,
                cast_chance,
            } => {
                let less = sum(Some(main_cfg), "SacredWispsLessDamage")?;
                vec![
                    Mod::new("Damage", ModType::More, less, Some("Used by Sacred Wisps")),
                    Mod::new(
                        "Speed",
                        ModType::More,
                        cast_chance - 100.0,
                        Some("Sacred Wisps cast chance"),
                    ),
                    flag("Used by Sacred Wisps"),
                ]
            }
        })
    }
}

impl<'a> Perform<'a> {
    /// The mirage's copy taken in, before the minions: the stand-in actors
    /// and their databases, then the copy as the main skill
    pub(crate) fn add_external(&mut self, ext: External) -> Result<()> {
        if ext.skill.mods.as_ref().is_some_and(|m| m.minion.is_some()) {
            return Err("not ported: a mirage of a skill with a minion".into());
        }
        let op = self.c.push_store(ext.player_store, None);
        let oe = self.c.push_store(ext.enemy_store, None);
        let (pa, ea) = (self.c.actors.actors.len(), self.c.actors.actors.len() + 1);
        self.c.actors.store_actor.insert(op, pa);
        self.c.actors.store_actor.insert(oe, ea);
        self.c.actors.actors.push(Actor {
            store: op,
            player: Some(pa),
            enemy: Some(ea),
            parent: None,
            ..ext.player_actor
        });
        self.c.actors.actors.push(Actor {
            store: oe,
            player: Some(pa),
            enemy: Some(pa),
            parent: None,
            ..ext.enemy_actor
        });
        self.applied_enemy_modifiers.push(Vec::new());
        self.applied_enemy_modifiers.push(Vec::new());
        let mods = ext
            .skill
            .mods
            .as_ref()
            .ok_or("a mirage's copy without its mod list")?
            .mods
            .clone();
        let mut base = Store::list();
        base.parent = Some(op);
        for m in mods {
            base.add(m);
        }
        let base = self.c.push_store(base, Some(pa));
        let mut list = Store::list();
        list.parent = Some(base);
        for m in ext.list_mods {
            list.add(m);
        }
        let list = self.c.push_store(list, Some(pa));
        let index = self.n_player + self.minion_skills.len();
        self.minion_skills.push(ext.skill);
        self.lists.push(SkillLists { base, list });
        self.marks.push(SkillMarks::default());
        self.env.skills.main_skill = index;
        if let Some(u) = ext.mirage_uses {
            self.sd_set(index, "mirageUses", u);
        } else {
            self.sd_clear(index, "mirageUses");
        }
        Ok(())
    }

    /// A copy of skill `i` of this calculation for a mirage
    fn external_from_here(&self, i: usize, kind: MirageKind) -> External {
        External {
            skill: self.skill(i).clone(),
            player_store: self.c.store(self.pdb()).clone(),
            enemy_store: self.c.store(self.edb()).clone(),
            player_actor: self.c.actors.actors[PLAYER].clone(),
            enemy_actor: self.c.actors.actors[ENEMY].clone(),
            list_mods: Vec::new(),
            kind,
            mirage_uses: self.sd_num(self.main(), "storedUses"),
        }
    }

    /// A copy of a cached calculation's main skill for a mirage
    fn external_from_cache(&self, c: &CachedCalc, kind: MirageKind) -> External {
        External {
            skill: c.skill.clone(),
            player_store: c.player_store.clone(),
            enemy_store: c.enemy_store.clone(),
            player_actor: c.player_actor.clone(),
            enemy_actor: c.enemy_actor.clone(),
            list_mods: Vec::new(),
            kind,
            mirage_uses: self.sd_num(self.main(), "storedUses"),
        }
    }

    /// `calcs.mirages(env)`: whether the main skill's own offence is left out
    pub(crate) fn mirages(&mut self) -> Result<bool> {
        let main = self.main();
        let used_by_mirage = self
            .cfg(main)
            .skill_cond
            .as_ref()
            .is_some_and(|c| c.contains(&Sym::new("usedByMirage")));
        if used_by_mirage || self.has_flag(main, "disable") {
            return Ok(false);
        }
        let name = self.skill_name(main).to_string();
        let (list, cfg) = (self.lists[main].list, self.cfg(main));
        let recalc = || {
            self.input
                .recalc
                .ok_or_else(|| "not ported: mirages without a recalculator".to_string())
        };
        if self.sd_truthy(main, "triggeredByMirageArcher") {
            let count = self.c.base(list, Some(&cfg), &["MirageArcherMaxCount"])?;
            let ext = self.external_from_here(main, MirageKind::Archer);
            let output = recalc()?.mirage(ext, &self.input.limited)?;
            self.mirage = Some(Mirage {
                name: name.clone(),
                skill_part_name: self.mods(main).skill_part_name.clone(),
                output,
                count: Some(count),
            });
            self.set_flag(main, "mirageArcher", true);
            return Ok(false);
        }
        if name == "Reflection" {
            let mut warriors = self
                .c
                .base(list, Some(&cfg), &["SaviourMirageWarriorMaxCount"])?;
            // The best attack with a one handed sword that can crit
            let sword = self.mf("Sword") | self.mf("Weapon1H");
            let mut best: Option<(f64, std::rc::Rc<CachedCalc>)> = None;
            for k in 0..self.n_player {
                let flags = self.cfg(k).flags.unwrap_or(0);
                let mirage = self
                    .cfg(k)
                    .skill_cond
                    .as_ref()
                    .is_some_and(|c| c.contains(&Sym::new("usedByMirage")));
                if k == main
                    || !self.has_type(k, "Attack")
                    || self.has_type(k, "Totem")
                    || self.has_type(k, "SummonsTotem")
                    || flags & sword != sword
                    || mirage
                {
                    continue;
                }
                let Some(c) = self.cached_calc(k, false, false)? else {
                    continue;
                };
                if c.out("CritChance").is_some_and(|x| x > 0.0) {
                    let dps = c.out("TotalDPS").unwrap_or(f64::NAN);
                    if best.as_ref().is_none_or(|(b, _)| dps > *b) {
                        best = Some((dps, c));
                    }
                }
            }
            let Some((_, cached)) = best else {
                self.mods_mut(main).disable_reason = Some("No Saviour active skill found".into());
                self.set_flag(main, "disable", true);
                return Ok(true);
            };
            let more_damage = self
                .c
                .base(list, Some(&cfg), &["SaviourMirageWarriorLessDamage"])?;
            let same = match (self.item(PLAYER, "Weapon 1"), self.item(PLAYER, "Weapon 2")) {
                (Some(a), Some(b)) => a.name == b.name,
                _ => false,
            };
            if same {
                warriors /= 2.0;
            }
            let ext = self.external_from_cache(
                &cached,
                MirageKind::Saviour {
                    more_damage,
                    warriors,
                },
            );
            let output = recalc()?.mirage(ext, &self.input.limited)?;
            // The player's output is the warriors'
            self.c.actors.actors[PLAYER].output = output;
            return Ok(true);
        }
        if name == "Tawhoa's Chosen" {
            return Err("not ported: the mirage of Tawhoa's Chosen".into());
        }
        if self.sd_truthy(main, "triggeredBySacredWisps") {
            let mut chance = None;
            let mut count = None;
            for k in 0..self.n_player {
                if self.skill_name(k) == "Summon Sacred Wisps" {
                    let l = self.lists[k].list;
                    chance = Some(self.c.base(l, Some(&cfg), &["SacredWispsChance"])?);
                    count = Some(self.c.base(l, Some(&cfg), &["SacredWispsMaxCount"])?);
                    break;
                }
            }
            let cast_chance = chance
                .ok_or("attempt to perform arithmetic on a nil value (local 'wispsCastChance')")?;
            let ext = self.external_from_here(
                main,
                MirageKind::Wisps {
                    main_cfg: Box::new(cfg),
                    cast_chance,
                },
            );
            let output = recalc()?.mirage(ext, &self.input.limited)?;
            self.mirage = Some(Mirage {
                name: name.clone(),
                skill_part_name: self.mods(main).skill_part_name.clone(),
                output,
                count,
            });
            self.set_flag(main, "wisp", true);
            return Ok(false);
        }
        Ok(false)
    }
}
