//! The minions of a calculation: `calcs.createMinionSkills`
//! (`Modules/CalcActiveSkill.lua`), and `initMinionModDB` and
//! `addMinionModifiers` of `Modules/CalcPerform.lua`.
//!
//! Each skill with a minion has an actor of its own, after the player and
//! the enemy, whose database its skills' mod lists stand on. The minions'
//! skills follow the player's in [`Perform::lists`].
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::actor::Actor;
use crate::calc::{ENEMY, PLAYER, round};
use crate::calc_perform::{Perform, Result, SkillLists, SkillMarks, new_mod, src};
use crate::calc_skill_mods::{Minion, ModsInput, Owner};
use crate::calc_skills::{PlayerDb, SkillEffect};
use crate::mods::{Mod, ModType, ModValue, Store, Tag};

use ModType::{Base, Flag as FlagT, Inc, More};

/// A skill's minion as an actor (`activeSkill.minion`)
#[derive(Clone, Debug)]
pub struct MinionActor {
    /// The player's skill that summons it
    pub skill: usize,
    /// Its place in the calculation's actors
    pub actor: usize,
    pub minion: Minion,
    /// `minion.activeSkillList`, by index in [`Perform::lists`]
    pub skills: Vec<usize>,
    /// `minion.mainSkill`
    pub main_skill: usize,
}

fn truthy(v: Option<&ModValue>) -> bool {
    v.is_some_and(ModValue::truthy)
}

impl<'a> Perform<'a> {
    /// The minion's data (`minion.minionData`)
    pub(crate) fn minion_data(&self, m: usize) -> Result<&crate::calc_setup::MinionData> {
        let typ = &self.minions[m].minion.typ;
        self.data()
            .minions
            .get(typ)
            .ok_or_else(|| format!("no minion {typ}"))
    }

    /// A numeric table of the data by level (`data.monsterLifeTable`, ...)
    pub(crate) fn level_table(&self, table: &str, level: f64) -> Option<f64> {
        self.data()
            .tables
            .get(table)?
            .get(&(level as i64).to_string())
            .copied()
    }

    /// Gives every skill with a minion its actor and the minion its
    /// skills (`calcs.createMinionSkills`), then sets `env.minion`
    pub(crate) fn create_minions(&mut self) -> Result<()> {
        for i in 0..self.n_player {
            let Some(minion) = self.mods(i).minion.clone() else {
                continue;
            };
            let data = self
                .data()
                .minions
                .get(&minion.typ)
                .cloned()
                .ok_or_else(|| format!("no minion {}", minion.typ))?;
            let store = self.c.push_store(Store::db(), None);
            let actor = self.c.actors.actors.len();
            self.c.actors.store_actor.insert(store, actor);
            let (parent, enemy) = if minion.hostile {
                (ENEMY, PLAYER)
            } else {
                (PLAYER, ENEMY)
            };
            self.c.actors.actors.push(Actor {
                store,
                weapon_data: [
                    Some(minion.weapon_data[0].clone()),
                    Some(minion.weapon_data[1].clone()),
                ],
                monster_tags: Some(
                    data.monster_tags
                        .iter()
                        .map(|t| t.as_str().into())
                        .collect(),
                ),
                parent: Some(parent),
                enemy: Some(enemy),
                ..Actor::default()
            });
            self.applied_enemy_modifiers.push(Vec::new());
            let m = self.minions.len();
            self.minions.push(MinionActor {
                skill: i,
                actor,
                minion,
                skills: Vec::new(),
                main_skill: 0,
            });
            self.create_minion_skills(m)?;
        }
        let main = self.main();
        self.env_minion = self.minions.iter().position(|m| m.skill == main);
        Ok(())
    }

    /// `calcs.createMinionSkills(env, activeSkill)`
    fn create_minion_skills(&mut self, m: usize) -> Result<()> {
        let (i, actor) = (self.minions[m].skill, self.minions[m].actor);
        let minion = self.minions[m].minion.clone();
        let data = self.minion_data(m)?.clone();
        let skill_data = self.input.data.skills;
        let mut ids: Vec<String> = data
            .skill_list
            .iter()
            .filter(|id| skill_data.skills.contains_key(*id))
            .cloned()
            .collect();
        let cfg = self.cfg(i);
        for v in self
            .c
            .list(self.lists[i].list, Some(&cfg), &["ExtraMinionSkill"])?
        {
            let ModValue::Record(r) = &v else { continue };
            let applies = match r.get("minionList") {
                Some(ModValue::List(l)) => l
                    .iter()
                    .any(|t| matches!(t, ModValue::Str(s) if **s == *minion.typ)),
                _ => true,
            };
            if applies && let Some(ModValue::Str(id)) = r.get("skillId") {
                ids.push(id.to_string());
            }
        }
        if ids.is_empty() {
            ids.push("Melee".into());
        }
        // The databases the skills' mod lists are built with: the
        // minion's is still empty
        let pdb = self.pdb();
        let edb = self.edb();
        let mut db = PlayerDb::new(
            self.c.store(pdb).clone(),
            self.c.store(edb),
            &self.env.setup.items.item_list,
            &self.env.skills.weapon_data,
            self.input.tab,
            self.input.data.items,
            skill_data,
        );
        let owner_store = db.stores.push(Store::db());
        let owner_actor = db.actors.actors.len();
        db.actors.store_actor.insert(owner_store, owner_actor);
        let mut owner = self.c.actors.actors[actor].clone();
        owner.store = owner_store;
        db.actors.actors.push(owner);
        let all_one_hand = minion
            .weapon_data
            .iter()
            .any(|w| truthy(w.get("countsAsAll1H")));
        let summon_name = self.skill(i).active_effect.granted_effect.name.clone();
        let spectre = self.has_flag(i, "spectre");
        let effectiveness = self.sd_num(i, "minionDamageEffectiveness").unwrap_or(0.0);
        let mut enemy_multipliers = std::collections::HashMap::new();
        let mut built = Vec::new();
        {
            let input = ModsInput {
                setup: self.input.data.setup,
                skill_data,
                tab: self.input.tab,
                item_data: self.input.data.items,
                item_list: &self.env.setup.items.item_list,
                weapon_data: &self.env.skills.weapon_data,
                mode: self.env.mode,
                config: self.input.config,
                placeholders: self.input.placeholder,
                spectre_list: self.input.spectre_list,
                character_level: self.input.character_level,
                enemy_level: self.env.enemy_level,
                iron_mass: self.env.setup.items.iron_mass_mods.is_some(),
                enemy_db: self.c.store(edb),
            };
            let owner = Owner {
                actor: owner_actor,
                summon: Some(&summon_name),
                minion: Some((&data, minion.level)),
                env_db: None,
            };
            for id in &ids {
                let ge = skill_data
                    .skills
                    .get(id)
                    .cloned()
                    .ok_or_else(|| format!("no skill {id}"))?;
                let mut level = 1.0;
                if ge.levels.len() > 1 {
                    let mut k = 1;
                    while let Some(l) = ge.levels.get(&k) {
                        if l.level_requirement > minion.level {
                            break;
                        }
                        level = k as f64;
                        k += 1;
                    }
                }
                let effect = SkillEffect::new(ge, Some(level), 0.0);
                let mut skill = crate::calc_skills::create_minion_skill(
                    effect,
                    self.skill(i),
                    &self.env.skills.from_item,
                    all_one_hand,
                    skill_data,
                );
                crate::calc_skill_mods::build_minion_skill_mod_list(
                    &mut skill,
                    &mut db,
                    &mut enemy_multipliers,
                    &input,
                    &owner,
                )?;
                let mods = skill.mods.as_mut().ok_or("a minion skill without mods")?;
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
                    ModValue::Number(1.0 + effectiveness / 100.0),
                );
                built.push(skill);
            }
        }
        for (k, v) in enemy_multipliers {
            let cur = self
                .c
                .store(edb)
                .multipliers
                .get(&k)
                .copied()
                .unwrap_or(0.0);
            self.c.store_mut(edb).multipliers.insert(k, cur.max(v));
        }
        let store = self.c.actors.actors[actor].store;
        for skill in built {
            let mut list = Store::list();
            list.parent = Some(store);
            for m in &skill
                .mods
                .as_ref()
                .ok_or("a minion skill without mods")?
                .mods
            {
                list.add(m.clone());
            }
            let list = self.c.push_store(list, Some(actor));
            let index = self.n_player + self.minion_skills.len();
            self.minion_skills.push(skill);
            self.lists.push(SkillLists { base: list, list });
            self.marks.push(SkillMarks::default());
            self.minions[m].skills.push(index);
        }
        // The selected skill
        let n = self.minions[m].skills.len() as f64;
        let breakdown = self.input.breakdown;
        let gem = self.skill(i).active_effect.src;
        let chosen = gem
            .and_then(|(g, k)| self.env.groups.get(g)?.gem_list.get(k))
            .and_then(|g| {
                if breakdown {
                    g.skill_minion_skill_calcs
                } else {
                    g.skill_minion_skill
                }
            });
        let index = chosen.unwrap_or(1.0).min(n).max(1.0);
        if let Some((g, k)) = gem
            && let Some(g) = self
                .env
                .groups
                .get_mut(g)
                .and_then(|g| g.gem_list.get_mut(k))
        {
            if breakdown {
                g.skill_minion_skill_calcs = Some(index);
            } else {
                g.skill_minion_skill = Some(index);
            }
        }
        let main = self.minions[m].skills[index as usize - 1];
        self.minions[m].main_skill = main;
        let name = self.skill(main).active_effect.granted_effect.name.clone();
        self.mods_mut(i).skill_part_name = Some(name);
        Ok(())
    }

    /// `initMinionModDB(env, activeSkill, output)`: the minion's base mods,
    /// its data's and skill's, and the items it uses
    pub(crate) fn init_minion_mod_db(&mut self, m: usize) -> Result<()> {
        let pdb = self.pdb();
        let (i, actor) = (self.minions[m].skill, self.minions[m].actor);
        let minion = self.minions[m].minion.clone();
        let data = self.minion_data(m)?.clone();
        let db = self.c.db(actor);
        self.c.actors.actors[actor].output.clear();
        self.c.set_mult(db, "Level", minion.level);
        let mut store = std::mem::replace(self.c.store_mut(db), Store::db());
        crate::calc_setup::init_mod_db(self.data(), &mut store, self.env.mode);
        *self.c.store_mut(db) = store;
        let level = minion.level;
        let map_mult = self
            .level_table("mapLevelLifeMult", self.env.enemy_level)
            .unwrap_or(1.0);
        let num = |k: &str| data.num(k);
        let mut life = self
            .level_table(minion.life_table, level)
            .unwrap_or(f64::NAN)
            * num("life").unwrap_or(f64::NAN);
        if minion.hostile {
            life *= map_mult;
        }
        self.c.add(db, src("Life", Base, life.floor(), "Base"));
        if let Some(es) = num("energyShield") {
            let table = if minion.hostile {
                minion.life_table
            } else {
                "monsterAllyLifeTable"
            };
            let mut es = self.level_table(table, level).unwrap_or(f64::NAN)
                * num("life").unwrap_or(f64::NAN)
                * es;
            if minion.hostile {
                es *= map_mult;
            }
            self.c
                .add(db, src("EnergyShield", Base, es.floor(), "Base"));
        }
        let armour = round(
            self.level_table("monsterArmourTable", level)
                .unwrap_or(f64::NAN)
                * num("armour").unwrap_or(1.0),
            0.0,
        );
        self.c.add(db, src("Armour", Base, armour, "Base"));
        let evasion = round(
            self.level_table("monsterEvasionTable", level)
                .unwrap_or(f64::NAN)
                * num("evasion").unwrap_or(1.0),
            0.0,
        );
        self.c.add(db, src("Evasion", Base, evasion, "Base"));
        if self.c.flag(pdb, None, &["MinionAccuracyEqualsAccuracy"])? {
            let per_dex = match self.c.override_num(pdb, None, &["DexAccBonusOverride"])? {
                Some(v) => v,
                None => self.misc_data("AccuracyPerDexBase"),
            };
            let v = self.c.val(pdb, "Accuracy", None)? + self.c.val(pdb, "Dex", None)? * per_dex;
            self.c.add(db, src("Accuracy", Base, v, "Player"));
        } else {
            self.c.add(
                db,
                src("CannotBeEvaded", FlagT, 1.0, "Minion Attacks always hit"),
            );
        }
        let mc = |k: &str| {
            self.data()
                .monster_constants
                .get(k)
                .copied()
                .unwrap_or(f64::NAN)
        };
        let cond = |var: &str| Tag::condition(var);
        let mult = |var: &str| Tag::multiplier(var);
        let (attack, cast, bleed) = (self.mf("Attack"), self.mf("Cast"), self.kf("Bleed"));
        let mods = vec![
            src(
                "CritMultiplier",
                Base,
                mc("base_critical_strike_multiplier") - 100.0,
                "Base",
            ),
            new_mod(
                "DotMultiplier",
                Base,
                mc("critical_ailment_dot_multiplier_+"),
                Some("Base"),
                0,
                0,
                vec![cond("CriticalStrike")],
            ),
            src(
                "FireResist",
                Base,
                num("fireResist").unwrap_or(f64::NAN),
                "Base",
            ),
            src(
                "ColdResist",
                Base,
                num("coldResist").unwrap_or(f64::NAN),
                "Base",
            ),
            src(
                "LightningResist",
                Base,
                num("lightningResist").unwrap_or(f64::NAN),
                "Base",
            ),
            src(
                "ChaosResist",
                Base,
                num("chaosResist").unwrap_or(f64::NAN),
                "Base",
            ),
            new_mod(
                "CritChance",
                Inc,
                mc("critical_strike_chance_+%_per_power_charge"),
                Some("Base"),
                0,
                0,
                vec![mult("PowerCharge")],
            ),
            new_mod(
                "Speed",
                Inc,
                mc("base_attack_speed_+%_per_frenzy_charge"),
                Some("Base"),
                attack,
                0,
                vec![mult("FrenzyCharge")],
            ),
            new_mod(
                "Speed",
                Inc,
                mc("base_cast_speed_+%_per_frenzy_charge"),
                Some("Base"),
                cast,
                0,
                vec![mult("FrenzyCharge")],
            ),
            new_mod(
                "Damage",
                More,
                mc("object_inherent_damage_+%_final_per_frenzy_charge"),
                Some("Base"),
                0,
                0,
                vec![mult("FrenzyCharge")],
            ),
            new_mod(
                "PhysicalDamageReduction",
                Base,
                mc("physical_damage_reduction_%_per_endurance_charge"),
                Some("Base"),
                0,
                0,
                vec![mult("EnduranceCharge")],
            ),
            new_mod(
                "ElementalDamageReduction",
                Base,
                mc("elemental_damage_reduction_%_per_endurance_charge_if_player_minion"),
                Some("Base"),
                0,
                0,
                vec![mult("EnduranceCharge")],
            ),
            src("ProjectileCount", Base, 1.0, "Base"),
            src(
                "MaximumFortification",
                Base,
                mc("base_max_fortification"),
                "Base",
            ),
            new_mod(
                "Damage",
                More,
                200.0,
                Some("Base"),
                0,
                bleed,
                vec![Tag::actor_condition("enemy", "Moving")],
            ),
        ];
        self.c.add_list(db, &mods);
        self.c.add_list(db, &data.mod_list);
        let extra = self.mods(i).extra_skill_mod_list.clone();
        self.c.add_list(db, &extra);
        if let Some(aegis) = self.env.setup.items.aegis_mods.clone() {
            if let Some(&id) = self.env.setup.items.item_list.get("Weapon 2") {
                self.c.actors.actors[actor]
                    .item_list
                    .insert("Weapon 3".into(), id);
            }
            self.c.add_list(db, &aegis);
        }
        if minion.typ == "RaisedSkeleton"
            && let Some(mass) = self.env.setup.items.iron_mass_mods.clone()
        {
            self.c.add_list(db, &mass);
        }
        let player_item = |p: &Self, slot: &str| p.item(PLAYER, slot);
        let slot_mods = |item: &crate::item::Item, n: usize| -> Vec<Mod> {
            item.slot_mod_list
                .as_ref()
                .and_then(|l| l.get(n))
                .cloned()
                .unwrap_or_default()
        };
        if self.sd_truthy(i, "minionUseBowAndQuiver") {
            if self.weapon_str(PLAYER, 0, "type").as_deref() == Some("Bow")
                && let Some(w) = player_item(self, "Weapon 1")
            {
                self.c.add_list(db, &slot_mods(w, 0));
            }
            if let Some(q) = player_item(self, "Weapon 2")
                && q.typ.as_deref() == Some("Quiver")
            {
                let scale = self.c.base(pdb, None, &["WidowHailMultiplier"])?.max(1.0);
                let mods = q.mod_list.clone().unwrap_or_default();
                self.c.scale_add_list(db, &mods, scale);
            }
            if self.c.flag(pdb, None, &["BlinkAndMirrorUseGloves"])?
                && let Some(g) = player_item(self, "Gloves")
            {
                let mods = g.mod_list.clone().unwrap_or_default();
                self.c.add_list(db, &mods);
            }
        }
        if self.sd_truthy(i, "minionUseMainHandWeapon") {
            let w = player_item(self, "Weapon 1").ok_or("no main hand weapon")?;
            self.c.add_list(db, &slot_mods(w, 0));
        }
        if minion.item_set.is_some() || minion.uses.is_some() {
            let uses = minion.uses.clone().unwrap_or_default();
            let tab = self.input.tab;
            let set = minion
                .item_set
                .and_then(|id| tab.item_sets.get(&(id as i64)));
            for slot in &tab.ordered_slots {
                if !uses.contains(&slot.slot_name) {
                    continue;
                }
                let item_id = match set {
                    Some(set) => {
                        let name = if slot.weapon_set == Some(1) && set.use_second_weapon_set {
                            format!("{} Swap", slot.slot_name)
                        } else {
                            slot.slot_name.clone()
                        };
                        set.slots.get(&name).map(|s| s.sel_item_id)
                    }
                    None => self.c.actors.actors[PLAYER]
                        .item_list
                        .get(&slot.slot_name)
                        .copied(),
                };
                let Some(id) = item_id else { continue };
                let Some(item) = tab.item(id) else { continue };
                self.c.actors.actors[actor]
                    .item_list
                    .insert(slot.slot_name.clone(), id);
                let mods = match &item.mod_list {
                    Some(l) => l.clone(),
                    None => slot_mods(item, slot.slot_num.unwrap_or(1.0) as usize - 1),
                };
                self.c.add_list(db, &mods);
            }
        }
        let str_added = self.c.base(pdb, None, &["StrengthAddedToMinions"])?;
        if str_added > 0.0 {
            let v = round(self.c.val(pdb, "Str", None)? * str_added / 100.0, 0.0);
            self.c.add(db, src("Str", Base, v, "Player"));
        }
        Ok(())
    }

    /// `calcs.defence`, `calcs.buildDefenceEstimations`, `calcs.triggers`
    /// and `calcs.offence` of `env.minion`, after the player's
    pub fn minion_calcs(&mut self) -> Result<()> {
        let Some(actor) = self.minion_actor() else {
            return Ok(());
        };
        self.defence(actor)?;
        self.defence_estimations(actor)?;
        self.triggers(actor)?;
        let main = self
            .actor_main(actor)
            .ok_or("a minion without a main skill")?;
        self.offence(actor, main)
    }

    /// `addMinionModifiers(modList, skillCfg, minion)`
    pub(crate) fn add_minion_modifiers(
        &mut self,
        list: crate::mods::StoreId,
        cfg: &crate::mods::Cfg,
        m: usize,
    ) -> Result<()> {
        let db = self.c.db(self.minions[m].actor);
        let typ = self.minions[m].minion.typ.clone();
        for v in self.c.list(list, Some(cfg), &["MinionModifier"])? {
            let ModValue::Record(r) = &v else { continue };
            let applies = match r.get("type") {
                Some(ModValue::Str(t)) => **t == *typ,
                _ => true,
            };
            if applies && let Some(ModValue::Mod(m)) = r.get("mod") {
                self.c.add(db, (**m).clone());
            }
        }
        Ok(())
    }

    /// The minion's keystones' mods (`env.spec.tree.keystoneMap[name]`)
    pub(crate) fn minion_keystones(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        for v in self.c.list(db, None, &["Keystone"])? {
            let ModValue::Str(name) = &v else { continue };
            if let Some(mods) = self.input.tree.keystone_mods(name) {
                let mods = mods.to_vec();
                self.c.add_list(db, &mods);
            }
        }
        Ok(())
    }
}
