//! Minions in `calcs.perform`: `calcs.createMinionSkills` of
//! `Modules/CalcActiveSkill.lua`, with `initMinionModDB` and
//! `addMinionModifiers` of `Modules/CalcPerform.lua`. Each active skill
//! with a minion gets a minion actor with its own database and skills;
//! the main skill's minion is `env.minion`.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::Actor;
use crate::calc::{PLAYER, m_max, m_min};
use crate::calc_perform::{
    BuffMap, Curse, Perform, Result, merge_buff, merge_extra, nospace, scaled, src, tag_num,
    tag_str,
};
use crate::calc_skill_mods::{MinionSkill, ModsInput, SkillDb, build_skill_mod_list};
use crate::calc_skills::{ActiveSkill, SkillEffect, SupportContext, create_active_skill};
use crate::mods::{Cfg, Mod, ModType, ModValue, Store, StoreId, Tag};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use ModType::{Base, Flag as FlagT, More};

/// A skill's minion as the calculation has it (`activeSkill.minion`)
#[derive(Clone, Debug)]
pub struct MinionActor {
    /// The skill that summons it, by index in the player's skills
    pub skill: usize,
    /// The minion's actor
    pub actor: usize,
    /// `minion.activeSkillList`
    pub skills: Vec<ActiveSkill>,
    /// Each skill's mod list, whose parent is the minion's database
    /// (`skillModList`, also `baseSkillModList`)
    pub lists: Vec<StoreId>,
    /// `minion.mainSkill`, by index in its skills
    pub main_skill: usize,
}

/// An actor's main skill (`actor.mainSkill`): the player's or a minion's
#[derive(Clone, Debug)]
pub struct MainSkill {
    /// `skillModList`
    pub list: StoreId,
    pub cfg: Cfg,
    pub flags: BTreeSet<String>,
    pub types: HashSet<i64>,
    pub data: BTreeMap<String, ModValue>,
}

/// `round(val)` of `Modules/Common.lua`
fn round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// `table[level]` of a numeric data table by level
fn table_at(p: &Perform, table: &str, level: f64) -> Option<f64> {
    if level.fract() != 0.0 {
        return None;
    }
    p.data()
        .tables
        .get(table)?
        .get(&(level as i64).to_string())
        .copied()
}

impl<'a> Perform<'a> {
    /// The minion of a player's skill
    pub fn minion_of(&self, skill: usize) -> Option<&MinionActor> {
        self.minions.iter().find(|m| m.skill == skill)
    }

    /// `env.minion`: the main skill's minion
    pub fn env_minion(&self) -> Option<&MinionActor> {
        self.minion.map(|i| &self.minions[i])
    }

    /// The actor of `env.minion`
    pub fn minion_actor(&self) -> Option<usize> {
        self.env_minion().map(|m| m.actor)
    }

    /// `actor.mainSkill`: the player's main skill, a minion's chosen skill
    pub fn actor_main_skill(&self, actor: usize) -> Option<MainSkill> {
        let (list, skill) = if actor == PLAYER {
            let main = self.main();
            (self.lists[main].list, &self.env.skills.active_skills[main])
        } else {
            let m = self.minions.iter().find(|m| m.actor == actor)?;
            (m.lists[m.main_skill], &m.skills[m.main_skill])
        };
        let mods = skill.mods.as_ref()?;
        Some(MainSkill {
            list,
            cfg: mods.skill_cfg.clone(),
            flags: mods.skill_flags.clone(),
            types: skill.skill_types.clone(),
            data: mods.skill_data.clone(),
        })
    }

    /// Whether `env.minion` is hostile
    pub(crate) fn minion_hostile(&self) -> bool {
        self.env_minion()
            .and_then(|m| self.mods(m.skill).minion.as_ref())
            .is_some_and(|x| x.hostile)
    }

    /// Adds the `MinionModifier` mods of a buff's list to a minion
    pub(crate) fn add_minion_modifiers_of(
        &mut self,
        mods: &[Mod],
        cfg: Option<&Cfg>,
        m: usize,
    ) -> Result<()> {
        let mut store = Store::list();
        for x in mods {
            store.add(x.clone());
        }
        let list = self.c.push_store(store, None);
        self.add_minion_modifiers(list, cfg, m)
    }

    /// The minions' actors and skills: for each skill with a minion, a
    /// new database and `calcs.createMinionSkills(env, activeSkill)`
    pub(crate) fn create_minions(&mut self) -> Result<()> {
        for i in 0..self.env.skills.active_skills.len() {
            let Some(minion) = self.mods(i).minion.clone() else {
                continue;
            };
            let md = self
                .data()
                .minions
                .get(&minion.typ)
                .ok_or_else(|| format!("no minion {}", minion.typ))?;
            let monster_tags = md.monster_tags.iter().map(|t| t.as_str().into()).collect();
            let (parent, enemy) = if minion.hostile {
                (crate::calc::ENEMY, PLAYER)
            } else {
                (PLAYER, crate::calc::ENEMY)
            };
            let actor = self.c.actors.actors.len();
            let store = self.c.push_store(Store::db(), Some(actor));
            self.c.actors.actors.push(Actor {
                store,
                weapon_data: [
                    Some(minion.weapon_data[0].clone()),
                    Some(minion.weapon_data[1].clone()),
                ],
                monster_tags: Some(monster_tags),
                parent: Some(parent),
                enemy: Some(enemy),
                ..Actor::default()
            });
            self.applied_enemy_modifiers.push(Vec::new());
            self.minions.push(MinionActor {
                skill: i,
                actor,
                skills: Vec::new(),
                lists: Vec::new(),
                main_skill: 0,
            });
            self.create_minion_skills(self.minions.len() - 1)?;
            let m = &self.minions[self.minions.len() - 1];
            let name = m.skills[m.main_skill]
                .active_effect
                .granted_effect
                .name
                .clone();
            self.mods_mut(i).skill_part_name = Some(name);
        }
        Ok(())
    }

    /// `calcs.createMinionSkills(env, activeSkill)`
    fn create_minion_skills(&mut self, m: usize) -> Result<()> {
        let skill = self.minions[m].skill;
        let actor = self.minions[m].actor;
        let minion = self
            .mods(skill)
            .minion
            .clone()
            .ok_or("a skill without a minion")?;
        let data = self.input.data;
        let md = data
            .setup
            .minions
            .get(&minion.typ)
            .ok_or_else(|| format!("no minion {}", minion.typ))?;
        let mut ids: Vec<String> = md
            .skill_list
            .iter()
            .filter(|id| data.skills.skills.contains_key(*id))
            .cloned()
            .collect();
        let list = self.lists[skill].list;
        let cfg = self.cfg(skill);
        for v in self.c.list(list, Some(&cfg), &["ExtraMinionSkill"])? {
            let ModValue::Record(r) = &v else { continue };
            let applies = match r.get("minionList") {
                None => true,
                Some(ModValue::List(l)) => l
                    .iter()
                    .any(|x| matches!(x, ModValue::Str(s) if **s == *minion.typ)),
                Some(ModValue::Record(l)) => l
                    .fields
                    .iter()
                    .any(|(_, x)| matches!(x, ModValue::Str(s) if **s == *minion.typ)),
                Some(_) => false,
            };
            if applies {
                match r.get("skillId") {
                    Some(ModValue::Str(id)) => ids.push(id.to_string()),
                    _ => return Err("an extra minion skill without a skill id".into()),
                }
            }
        }
        if ids.is_empty() {
            ids.push("Melee".into());
        }
        let summon = self.env.skills.active_skills[skill].clone();
        let all_one_hand = minion
            .weapon_data
            .iter()
            .any(|d| d.get("countsAsAll1H").is_some_and(ModValue::truthy));
        let damage_effectiveness = 1.0
            + self
                .sd_num(skill, "minionDamageEffectiveness")
                .unwrap_or(0.0)
                / 100.0;
        let spectre = summon
            .mods
            .as_ref()
            .is_some_and(|s| s.skill_flags.contains("spectre"));
        let mut skills = Vec::new();
        let mut lists = Vec::new();
        for id in ids {
            let ge = data
                .skills
                .skills
                .get(&id)
                .cloned()
                .ok_or_else(|| format!("no skill {id}"))?;
            let mut level = 1.0;
            if ge.level_count() > 1 {
                for l in 1..=ge.level_count() {
                    let req = ge.level(l as f64).map_or(f64::NAN, |x| x.level_requirement);
                    if req > minion.level {
                        break;
                    }
                    level = l as f64;
                }
            }
            let active = SkillEffect::new(ge, Some(level), 0.0);
            let ctx = SupportContext {
                from_item: self.env.skills.from_item.clone(),
                groups: &self.env.groups,
                all_one_hand,
                data: data.skills,
                summon: Some(&summon),
            };
            let mut ms = create_active_skill(active, summon.support_list.clone(), None, &ctx);
            let setup = &self.env.setup;
            let item_list = self.c.actors.actors[actor].item_list.clone();
            let no_enemy = Store::db();
            let input = ModsInput {
                setup: data.setup,
                skill_data: data.skills,
                tab: self.input.tab,
                item_data: data.items,
                item_list: &item_list,
                weapon_data: &minion.weapon_data,
                mode: self.env.mode,
                config: self.input.config,
                placeholders: self.input.placeholder,
                spectre_list: self.input.spectre_list,
                character_level: f64::NAN,
                enemy_level: self.env.enemy_level,
                iron_mass: setup.items.iron_mass_mods.is_some(),
                enemy_db: &no_enemy,
            };
            let pdb = self.c.db(PLAYER);
            let id = self.c.db(actor);
            let mut db = SkillDb {
                stores: &mut self.c.stores,
                actors: &mut self.c.actors,
                actor,
                id,
                player_db: pdb,
            };
            let ctx = MinionSkill {
                summon: &summon,
                data: md,
                level: minion.level,
            };
            build_skill_mod_list(
                &mut ms,
                &mut self.env.groups,
                &mut db,
                &mut HashMap::new(),
                &input,
                Some(&ctx),
            )?;
            let mods = ms
                .mods
                .as_mut()
                .ok_or("a minion skill without its mod list")?;
            for f in ["minion", "minionSkill", "haveMinion"] {
                mods.skill_flags.insert(f.into());
            }
            if spectre {
                mods.skill_flags.insert("spectre".into());
            } else {
                mods.skill_flags.remove("spectre");
            }
            mods.skill_data.insert(
                "damageEffectiveness".into(),
                ModValue::Number(damage_effectiveness),
            );
            let mut store = Store::list();
            store.parent = Some(id);
            for m in &mods.mods {
                store.add(m.clone());
            }
            lists.push(self.c.push_store(store, Some(actor)));
            skills.push(ms);
        }
        // The chosen skill, which the gem keeps in the main mode
        let n = skills.len() as f64;
        let src = summon.active_effect.src;
        let gem = src.and_then(|(g, i)| self.env.groups.get_mut(g)?.gem_list.get_mut(i));
        let chosen = gem
            .as_ref()
            .and_then(|g| g.skill_minion_skill)
            .unwrap_or(1.0);
        let index = m_max(m_min(chosen, n), 1.0);
        // The main mode keeps the choice; the Calcs tab's is not ported
        if let Some(g) = gem {
            g.skill_minion_skill = Some(index);
        }
        let mm = &mut self.minions[m];
        mm.skills = skills;
        mm.lists = lists;
        mm.main_skill = index as usize - 1;
        Ok(())
    }

    /// `initMinionModDB(env, activeSkill, output)`
    pub(crate) fn init_minion_mod_db(&mut self, m: usize) -> Result<()> {
        let skill = self.minions[m].skill;
        let actor = self.minions[m].actor;
        let minion = self
            .mods(skill)
            .minion
            .clone()
            .ok_or("a skill without a minion")?;
        let data = self.input.data.setup;
        let md = data
            .minions
            .get(&minion.typ)
            .ok_or_else(|| format!("no minion {}", minion.typ))?;
        let pdb = self.c.db(PLAYER);
        let db = self.c.db(actor);
        self.c.actors.actors[actor].output.clear();
        self.c.set_mult(db, "Level", minion.level);
        crate::calc_setup::init_mod_db(data, self.c.store_mut(db), self.env.mode);
        let level = minion.level;
        let enemy_level = self.env.enemy_level;
        let life_mult = || table_at(self, "mapLevelLifeMult", enemy_level).unwrap_or(1.0);
        let num = |k: &str| md.num(k).unwrap_or(f64::NAN);
        let mut base_life =
            table_at(self, minion.life_table, level).unwrap_or(f64::NAN) * num("life");
        if minion.hostile {
            base_life *= life_mult();
        }
        let mut mods = vec![src("Life", Base, base_life.floor(), "Base")];
        if let Some(es) = md.num("energyShield") {
            let table = if minion.hostile {
                minion.life_table
            } else {
                "monsterAllyLifeTable"
            };
            let mut base_es = table_at(self, table, level).unwrap_or(f64::NAN) * num("life") * es;
            if minion.hostile {
                base_es *= life_mult();
            }
            mods.push(src("EnergyShield", Base, base_es.floor(), "Base"));
        }
        let or1 = |k: &str| md.num(k).unwrap_or(1.0);
        mods.push(src(
            "Armour",
            Base,
            round(table_at(self, "monsterArmourTable", level).unwrap_or(f64::NAN) * or1("armour")),
            "Base",
        ));
        mods.push(src(
            "Evasion",
            Base,
            round(
                table_at(self, "monsterEvasionTable", level).unwrap_or(f64::NAN) * or1("evasion"),
            ),
            "Base",
        ));
        if self.c.flag(pdb, None, &["MinionAccuracyEqualsAccuracy"])? {
            let per_dex = match self.c.override_num(pdb, None, &["DexAccBonusOverride"])? {
                Some(v) => v,
                None => self.misc_data("AccuracyPerDexBase"),
            };
            let acc = self.c.val(pdb, "Accuracy", None)? + self.c.val(pdb, "Dex", None)? * per_dex;
            mods.push(src("Accuracy", Base, acc, "Player"));
        } else {
            mods.push(src(
                "CannotBeEvaded",
                FlagT,
                1.0,
                "Minion Attacks always hit",
            ));
        }
        let mc = |k: &str| data.monster_constants.get(k).copied().unwrap_or(f64::NAN);
        mods.push(src(
            "CritMultiplier",
            Base,
            mc("base_critical_strike_multiplier") - 100.0,
            "Base",
        ));
        mods.push(
            src(
                "DotMultiplier",
                Base,
                mc("critical_ailment_dot_multiplier_+"),
                "Base",
            )
            .tag(Tag::condition("CriticalStrike")),
        );
        for (name, k) in [
            ("FireResist", "fireResist"),
            ("ColdResist", "coldResist"),
            ("LightningResist", "lightningResist"),
            ("ChaosResist", "chaosResist"),
        ] {
            mods.push(src(name, Base, num(k), "Base"));
        }
        let mf = |n: &str| data.mod_flag(n);
        let kf = |n: &str| data.keyword_flag(n);
        mods.push(
            Mod::new(
                "CritChance",
                ModType::Inc,
                mc("critical_strike_chance_+%_per_power_charge"),
                Some("Base"),
            )
            .tag(Tag::multiplier("PowerCharge")),
        );
        mods.push(
            Mod::new(
                "Speed",
                ModType::Inc,
                mc("base_attack_speed_+%_per_frenzy_charge"),
                Some("Base"),
            )
            .flags(mf("Attack"), 0)
            .tag(Tag::multiplier("FrenzyCharge")),
        );
        mods.push(
            Mod::new(
                "Speed",
                ModType::Inc,
                mc("base_cast_speed_+%_per_frenzy_charge"),
                Some("Base"),
            )
            .flags(mf("Cast"), 0)
            .tag(Tag::multiplier("FrenzyCharge")),
        );
        mods.push(
            src(
                "Damage",
                More,
                mc("object_inherent_damage_+%_final_per_frenzy_charge"),
                "Base",
            )
            .tag(Tag::multiplier("FrenzyCharge")),
        );
        mods.push(
            src(
                "PhysicalDamageReduction",
                Base,
                mc("physical_damage_reduction_%_per_endurance_charge"),
                "Base",
            )
            .tag(Tag::multiplier("EnduranceCharge")),
        );
        mods.push(
            src(
                "ElementalDamageReduction",
                Base,
                mc("elemental_damage_reduction_%_per_endurance_charge_if_player_minion"),
                "Base",
            )
            .tag(Tag::multiplier("EnduranceCharge")),
        );
        mods.push(src("ProjectileCount", Base, 1.0, "Base"));
        mods.push(src(
            "MaximumFortification",
            Base,
            mc("base_max_fortification"),
            "Base",
        ));
        mods.push(
            src("Damage", More, 200.0, "Base")
                .flags(0, kf("Bleed"))
                .tag(Tag::actor_condition("enemy", "Moving")),
        );
        mods.extend(md.mod_list.iter().cloned());
        mods.extend(self.mods(skill).extra_skill_mod_list.iter().cloned());
        self.c.add_list(db, &mods);
        let items = &self.env.setup.items;
        if let Some(aegis) = &items.aegis_mods {
            if let Some(&id) = items.item_list.get("Weapon 2") {
                self.c.actors.actors[actor]
                    .item_list
                    .insert("Weapon 3".into(), id);
            }
            let aegis = aegis.clone();
            self.c.add_list(db, &aegis);
        }
        if let Some(iron) = &items.iron_mass_mods
            && minion.typ == "RaisedSkeleton"
        {
            let iron = iron.clone();
            self.c.add_list(db, &iron);
        }
        let slot_mods = |p: &Perform, slot: &str| -> Result<Vec<Mod>> {
            let item = p
                .item(PLAYER, slot)
                .ok_or_else(|| format!("no item in {slot}"))?;
            Ok(item
                .slot_mod_list
                .as_ref()
                .and_then(|l| l.first())
                .cloned()
                .unwrap_or_default())
        };
        if self.sd_truthy(skill, "minionUseBowAndQuiver") {
            if self.weapon_str(PLAYER, 0, "type").as_deref() == Some("Bow") {
                let l = slot_mods(self, "Weapon 1")?;
                self.c.add_list(db, &l);
            }
            if let Some(q) = self.item(PLAYER, "Weapon 2")
                && q.typ.as_deref() == Some("Quiver")
            {
                let l = q.mod_list.clone().unwrap_or_default();
                let scale = m_max(self.c.base(pdb, None, &["WidowHailMultiplier"])?, 1.0);
                self.c.scale_add_list(db, &l, scale);
            }
            if self.c.flag(pdb, None, &["BlinkAndMirrorUseGloves"])?
                && let Some(g) = self.item(PLAYER, "Gloves")
            {
                let l = g.mod_list.clone().unwrap_or_default();
                self.c.add_list(db, &l);
            }
        }
        if self.sd_truthy(skill, "minionUseMainHandWeapon") {
            let l = slot_mods(self, "Weapon 1")?;
            self.c.add_list(db, &l);
        }
        if minion.item_set.is_some() || minion.uses.is_some() {
            let uses = minion.uses.clone().unwrap_or_default();
            let set = minion
                .item_set
                .and_then(|id| self.input.tab.item_sets.get(&(id as i64)));
            for slot in &self.input.tab.ordered_slots {
                if slot.node_id.is_some() || !uses.contains(&slot.slot_name) {
                    continue;
                }
                let mut name = slot.slot_name.clone();
                let item = match set {
                    Some(set) => {
                        if slot.weapon_set == Some(1) && set.use_second_weapon_set {
                            name = format!("{name} Swap");
                        }
                        let sel = set
                            .slots
                            .get(&name)
                            .ok_or_else(|| format!("an item set without {name}"))?;
                        self.input.tab.item(sel.sel_item_id)
                    }
                    None => self.item(PLAYER, &name),
                };
                if let Some(item) = item {
                    let id = item.id.ok_or("an item without an id")?;
                    self.c.actors.actors[actor].item_list.insert(name, id);
                    let l = match &item.mod_list {
                        Some(l) => l.clone(),
                        None => slot
                            .slot_num
                            .and_then(|n| item.slot_mod_list.as_ref()?.get(n as usize - 1))
                            .cloned()
                            .unwrap_or_default(),
                    };
                    self.c.add_list(db, &l);
                }
            }
        }
        let str_added = self.c.base(pdb, None, &["StrengthAddedToMinions"])?;
        if str_added > 0.0 {
            let s = round(self.c.val(pdb, "Str", None)? * str_added / 100.0);
            self.c.add(db, src("Str", Base, s, "Player"));
        }
        Ok(())
    }

    /// `addMinionModifiers(modList, skillCfg, minion)`: the mods a skill or
    /// a buff grants the minion
    pub(crate) fn add_minion_modifiers(
        &mut self,
        list: StoreId,
        cfg: Option<&Cfg>,
        m: usize,
    ) -> Result<()> {
        let typ = self
            .mods(self.minions[m].skill)
            .minion
            .as_ref()
            .map(|x| x.typ.clone());
        let db = self.c.db(self.minions[m].actor);
        for v in self.c.list(list, cfg, &["MinionModifier"])? {
            let ModValue::Record(r) = &v else { continue };
            let applies = match r.get("type") {
                None => true,
                Some(ModValue::Str(t)) => typ.as_deref() == Some(&**t),
                Some(_) => false,
            };
            if applies && let Some(ModValue::Mod(inner)) = r.get("mod") {
                self.c.add(db, (**inner).clone());
            }
        }
        Ok(())
    }

    /// The buffs, auras, curses and debuffs of a skill's minion's skills
    pub(crate) fn minion_skill_buffs(
        &mut self,
        i: usize,
        buffs: &mut BuffMap,
        minion_buffs: &mut BuffMap,
        debuffs: &mut BuffMap,
        minion_curses: &mut Vec<Curse>,
    ) -> Result<()> {
        let Some(m) = self.minions.iter().position(|x| x.skill == i) else {
            return Ok(());
        };
        let pdb = self.pdb();
        let edb = self.edb();
        let mode = self.env.mode;
        let precision = self.c.precision;
        let casting = self.minions[m].actor;
        let cdb = self.c.db(casting);
        let env_minion = self.minion_actor();
        let mdb = env_minion.map(|a| self.c.db(a));
        let main = self.main();
        let main_list = self.lists[main].list;
        let spectre = self.has_flag(i, "spectre");
        let minion_name = self
            .mods(i)
            .minion
            .as_ref()
            .and_then(|x| self.data().minions.get(&x.typ))
            .and_then(|d| d.str("name"))
            .unwrap_or("")
            .to_string();
        let spectre_source = |l: &mut Vec<Mod>, skill: Option<&str>| {
            if spectre {
                let source = match skill {
                    Some(s) => format!("Spectre:{s} - {minion_name}"),
                    None => format!("Spectre:{minion_name}"),
                };
                for x in l.iter_mut() {
                    x.source = Some(source.as_str().into());
                }
            }
        };
        let st = |n: &str| self.input.data.skills.skill_type(n).unwrap_or(-1);
        for j in 0..self.minions[m].skills.len() {
            let list = self.minions[m].lists[j];
            let ms = &self.minions[m].skills[j];
            let mods = ms
                .mods
                .as_ref()
                .ok_or("a minion skill without its mod list")?;
            let cfg = mods.skill_cfg.clone();
            let buff_list = mods.buff_list.clone();
            let sd = mods.skill_data.clone();
            let is_mark = ms.skill_types.contains(&st("Mark"));
            let is_aura = ms.skill_types.contains(&st("Aura"));
            let enable = sd.get("enable").is_some_and(ModValue::truthy);
            let sd_truthy = |k: &str| sd.get(k).is_some_and(ModValue::truthy);
            for buff in &buff_list {
                let bname = buff.name.as_str();
                let affected = format!("AffectedBy{}", nospace(bname));
                match buff.typ.as_str() {
                    "Buff" => {
                        if !(mode.buffs && enable) {
                            continue;
                        }
                        let bcfg = buff.active_skill_buff.then(|| cfg.clone());
                        let store = if buff.active_skill_buff { list } else { cdb };
                        if buff.apply_allies {
                            self.c.set_cond(pdb, &affected, true);
                            let inc = self.c.inc(
                                store,
                                bcfg.as_ref(),
                                &["BuffEffect", "BuffEffectOnPlayer"],
                            )? + self.c.inc(pdb, None, &["BuffEffectOnSelf"])?;
                            let more = self.c.more(
                                store,
                                bcfg.as_ref(),
                                &["BuffEffect", "BuffEffectOnPlayer"],
                            )? * self.c.more(pdb, None, &["BuffEffectOnSelf"])?;
                            let l = scaled(&buff.mod_list, (1.0 + inc / 100.0) * more, precision);
                            merge_buff(&l, buffs, bname)?;
                            merge_buff(&buff.mod_list, buffs, bname)?;
                            if sd_truthy("thisIsNotABuff")
                                && let Some(b) = buffs.iter_mut().find(|b| b.0 == bname)
                            {
                                b.2 = true;
                            }
                        }
                        let env_check =
                            env_minion.is_some_and(|a| a == casting || buff.apply_allies);
                        if buff.apply_minions || env_check {
                            let target = if env_check { mdb.unwrap_or(cdb) } else { cdb };
                            self.c.set_cond(target, &affected, true);
                            let names: &[&str] = if env_minion == Some(casting) {
                                &["BuffEffect", "BuffEffectOnSelf"]
                            } else {
                                &["BuffEffect"]
                            };
                            let inc = self.c.inc(store, bcfg.as_ref(), names)?;
                            let more = self.c.more(store, bcfg.as_ref(), names)?;
                            let l = scaled(&buff.mod_list, (1.0 + inc / 100.0) * more, precision);
                            merge_buff(&l, minion_buffs, bname)?;
                            merge_buff(&buff.mod_list, minion_buffs, bname)?;
                            if sd_truthy("thisIsNotABuff") {
                                match buffs.iter_mut().find(|b| b.0 == bname) {
                                    Some(b) => b.2 = true,
                                    None => {
                                        return Err(format!(
                                            "attempt to index a nil value (buffs[{bname}])"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    "Aura" => {
                        if !(mode.buffs && enable) {
                            continue;
                        }
                        let extra =
                            merge_extra(self.c.list(cdb, Some(&cfg), &["ExtraAuraEffect"])?)?;
                        if self.c.flag(cdb, None, &["SelfAurasCannotAffectAllies"])?
                            || self.c.flag(cdb, None, &["SelfAurasOnlyAffectYou"])?
                            || self
                                .c
                                .flag(cdb, None, &["SelfAuraSkillsCannotAffectAllies"])?
                            || self.c.flag(
                                list,
                                Some(&cfg),
                                &["SelfAurasAffectYouAndLinkedTarget"],
                            )?
                        {
                            continue;
                        }
                        if !self.c.flag(pdb, None, &["AlliesAurasCannotAffectSelf"])?
                            && !self.c.cond(pdb, &affected)
                        {
                            let inc = self.c.inc(
                                list,
                                Some(&cfg),
                                &[
                                    "AuraEffect",
                                    "BuffEffect",
                                    "BuffEffectOnPlayer",
                                    "AuraBuffEffect",
                                ],
                            )? + self.c.inc(
                                pdb,
                                Some(&cfg),
                                &["BuffEffectOnSelf", "AuraEffectOnSelf"],
                            )?;
                            let more = self.c.more(
                                list,
                                Some(&cfg),
                                &["AuraEffect", "BuffEffect", "AuraBuffEffect"],
                            )? * self.c.more(
                                pdb,
                                Some(&cfg),
                                &["BuffEffectOnSelf", "AuraEffectOnSelf"],
                            )?;
                            let mult = (1.0 + inc / 100.0) * more;
                            self.c.set_cond(pdb, "AffectedByAura", true);
                            if bname.starts_with("Vaal") {
                                let rest: String = bname.chars().skip(5).collect();
                                self.c.set_cond(
                                    pdb,
                                    &format!("AffectedBy{}", nospace(&rest)),
                                    true,
                                );
                            }
                            self.c.set_cond(pdb, &affected, true);
                            let mut l = scaled(&buff.mod_list, mult, precision);
                            l.extend(scaled(&extra, mult, precision));
                            spectre_source(&mut l, Some(bname));
                            merge_buff(&l, buffs, bname)?;
                        }
                        if let (Some(a), Some(mdb)) = (env_minion, mdb)
                            && !self.c.cond(mdb, &affected)
                            && (a != casting || !self.sd_truthy(i, "auraCannotAffectSelf"))
                        {
                            let inc =
                                self.c
                                    .inc(list, Some(&cfg), &["AuraEffect", "BuffEffect"])?
                                    + self.c.inc(
                                        mdb,
                                        Some(&cfg),
                                        &["BuffEffectOnSelf", "AuraEffectOnSelf"],
                                    )?;
                            let more =
                                self.c
                                    .more(list, Some(&cfg), &["AuraEffect", "BuffEffect"])?
                                    * self.c.more(
                                        mdb,
                                        Some(&cfg),
                                        &["BuffEffectOnSelf", "AuraEffectOnSelf"],
                                    )?;
                            let mult = (1.0 + inc / 100.0) * more;
                            self.c.set_cond(mdb, &affected, true);
                            self.c.set_cond(mdb, "AffectedByAura", true);
                            let mut l = scaled(&buff.mod_list, mult, precision);
                            l.extend(scaled(&extra, mult, precision));
                            spectre_source(&mut l, Some(bname));
                            merge_buff(&l, minion_buffs, bname)?;
                        }
                        if self.has_flag(main, "totem") && !self.c.cond(main_list, &affected) {
                            self.c.set_cond(main_list, &affected, true);
                            self.c.set_cond(main_list, "AffectedByAura", true);
                            let names = ["AuraEffect", "BuffEffect", "AuraBuffEffect"];
                            let inc = self.c.inc(list, Some(&cfg), &names)?;
                            let more = self.c.more(list, Some(&cfg), &names)?;
                            let scale = m_max((1.0 + inc / 100.0) * more, 0.0);
                            let mut l = self.totem_aura_mods([&extra, &buff.mod_list], scale)?;
                            spectre_source(&mut l, None);
                            merge_buff(&l, buffs, &format!("Totem {bname}"))?;
                        }
                    }
                    "Curse" => {
                        if !(mode.effective
                            && enable
                            && (!self.c.flag(edb, None, &["Hexproof"])? || is_mark))
                        {
                            continue;
                        }
                        let cp = |k: &str| self.data().curse_priority.get(k).copied();
                        let priority = cp(bname).unwrap_or(0.0)
                            + cp("SocketPriorityBase").unwrap_or(f64::NAN)
                            + if is_aura {
                                cp("CurseFromAura").unwrap_or(f64::NAN)
                            } else {
                                0.0
                            };
                        let inc = self.c.inc(list, Some(&cfg), &["CurseEffect"])?
                            + self.c.inc(edb, None, &["CurseEffectOnSelf"])?;
                        let more = self.c.more(list, Some(&cfg), &["CurseEffect"])?
                            * self.c.more(edb, None, &["CurseEffectOnSelf"])?;
                        minion_curses.push(Curse {
                            name: bname.to_string(),
                            priority,
                            mod_list: Some(scaled(
                                &buff.mod_list,
                                (1.0 + inc / 100.0) * more,
                                precision,
                            )),
                            ..Curse::default()
                        });
                    }
                    "Debuff" | "AuraDebuff" => {
                        let stack_var = tag_str(buff, "effectStackVar");
                        let stack_count = match &stack_var {
                            Some(v) => {
                                let mut n =
                                    self.c
                                        .base(list, Some(&cfg), &[&format!("Multiplier:{v}")])?;
                                if let Some(l) = tag_num(buff, "effectStackLimit") {
                                    n = m_min(n, l);
                                } else if let Some(lv) = tag_str(buff, "effectStackLimitVar") {
                                    n = m_min(
                                        n,
                                        self.c.base(
                                            list,
                                            Some(&cfg),
                                            &[&format!("Multiplier:{lv}")],
                                        )?,
                                    );
                                }
                                n
                            }
                            None => sd
                                .get("stackCount")
                                .and_then(ModValue::as_number)
                                .unwrap_or(1.0),
                        };
                        if !(mode.effective && stack_count > 0.0) {
                            continue;
                        }
                        let mut mult = 1.0;
                        if buff.typ == "AuraDebuff" {
                            mult = 0.0;
                            if !self.c.flag(list, None, &["SelfAurasOnlyAffectYou"])?
                                || self.c.flag(
                                    list,
                                    Some(&cfg),
                                    &["SelfAurasAffectYouAndLinkedTarget"],
                                )?
                            {
                                let names = ["AuraEffect", "BuffEffect", "DebuffEffect"];
                                let inc = self.c.inc(list, Some(&cfg), &names)?;
                                let more = self.c.more(list, Some(&cfg), &names)?;
                                mult = (1.0 + inc / 100.0) * more;
                                if self.c.cond(edb, &affected) {
                                    mult = 0.0;
                                }
                            }
                        }
                        self.c.set_cond(edb, &affected, true);
                        if env_minion == Some(casting)
                            && let Some(mdb) = mdb
                        {
                            self.c.set_cond(mdb, &affected, true);
                        }
                        if buff.typ == "Debuff" {
                            let inc = self.c.inc(list, Some(&cfg), &["DebuffEffect"])?;
                            let more = self.c.more(list, Some(&cfg), &["DebuffEffect"])?;
                            mult = (1.0 + inc / 100.0) * more;
                        }
                        let mut l = scaled(&buff.mod_list, mult * stack_count, precision);
                        let count = sd.get("stackCount").and_then(ModValue::as_number);
                        if count.is_some() || stack_var.is_some() {
                            let count =
                                count.ok_or("not ported: a minion debuff stack without a count")?;
                            l.push(src(&format!("Multiplier:{bname}Stack"), Base, count, bname));
                        }
                        merge_buff(&l, debuffs, bname)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
