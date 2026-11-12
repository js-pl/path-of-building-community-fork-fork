//! Calculations of other skills than the main one: `calcs.buildActiveSkill`
//! (`Modules/Calcs.lua`) and the results it leaves in upstream's
//! `GlobalCache` (`cacheData` of `Modules/Common.lua`), which triggers,
//! cached skill stages and mirages read.
//!
//! A [`Recalculator`] reruns the setup of a copy of the build, makes the
//! skill the main one and runs the whole of `calcs.perform` on it; each
//! calculation caches its main skill's results by the skill's id
//! (`cacheSkillUUID`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::actor::OutVal;
use crate::build::Build;
use crate::calc_env::{CalcData, CalcMode};
use crate::calc_override::Override;
use crate::calc_perform::{Perform, PerformInput, Result};
use crate::calc_skills::ActiveSkill;
use crate::mods::Sym;
use crate::skills::SocketGroup;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// What a calculation leaves for others (`GlobalCache.cachedData[mode][uuid]`)
#[derive(Clone, Debug)]
pub struct CachedCalc {
    /// `Env.player.output`
    pub output: crate::mods::SymMap<OutVal>,
    /// `ActiveSkill`: the main skill with its mods and skill data
    pub skill: ActiveSkill,
    /// `Env.minion.output`
    pub minion_output: Option<crate::mods::SymMap<OutVal>>,
    /// The calculation's player and enemy as they are at its end: the
    /// cached skill's actor, which a mirage's copy keeps
    pub player_store: crate::mods::Store,
    pub enemy_store: crate::mods::Store,
    pub player_actor: crate::actor::Actor,
    pub enemy_actor: crate::actor::Actor,
}

impl CachedCalc {
    pub fn out(&self, k: &str) -> Option<f64> {
        self.output.get(&Sym::new(k)).and_then(OutVal::num)
    }
}

/// Calculations of the build's other skills, and the cache of their results
pub trait Recalc {
    /// The cached results of a skill's calculation
    fn cached(&self, uuid: &str) -> Option<Rc<CachedCalc>>;
    /// Whether this is a calculator's calculation (`env.mode ==
    /// "CALCULATOR"`), which most lookups recalculate even when cached
    fn calculator(&self) -> bool;
    /// Caches a calculation's results (`cacheData`)
    fn store(&self, uuid: &str, calc: CachedCalc);
    /// `calcs.buildActiveSkill(env, mode, skill, uuid, limited)`: a
    /// calculation of the build with the skill as the main one, whose
    /// results it caches. `limited` are the skills whose own calculations
    /// it skips.
    fn calculate(&self, uuid: &str, limited: &[String]) -> Result<()>;
    /// A mirage's calculation of its copy of a skill (`calcs.copyActiveSkill`
    /// and `calcs.perform` of `calculateMirage`): the player's output
    fn mirage(
        &self,
        ext: crate::calc_mirage::External,
        limited: &[String],
    ) -> Result<crate::mods::SymMap<OutVal>>;
}

/// `cacheSkillUUID(skill, env)` of a skill of the setup
pub fn skill_uuid(skill: &ActiveSkill, groups: &[SocketGroup]) -> String {
    let name = crate::pat::gsub(&skill.active_effect.granted_effect.name, "%s+", "");
    let group = skill.group.and_then(|g| groups.get(g).map(|x| (g, x)));
    let slot = group
        .and_then(|(_, g)| g.slot.as_deref())
        .map(|s| crate::pat::gsub(&s.to_uppercase(), "%s+", ""))
        .unwrap_or_else(|| "NO_SLOT".into());
    let mut slot_index = 1;
    if let (Some((g, _)), Some((sg, k))) = (group, skill.active_effect.src)
        && sg == g
    {
        slot_index = k + 1;
    }
    let group_index = group.map_or(1, |(g, _)| g + 1);
    format!("{name}_{slot}_{slot_index}_{group_index}")
}

/// Cached calculations by skill id
pub type Cache = HashMap<String, Rc<CachedCalc>>;

/// Recalculates a build with other main skills
pub struct Recalculator<'a, 't> {
    /// The build after its main calculation's setup
    pub build: &'a Build<'t>,
    pub data: &'a CalcData<'a>,
    /// The mode of the calculations, each with a cache of its own
    pub mode: CalcMode,
    /// The override of the calculations (`env.override`)
    pub ovr: Override,
    cache: RefCell<Cache>,
}

impl<'a, 't> Recalculator<'a, 't> {
    pub fn new(build: &'a Build<'t>, data: &'a CalcData<'a>, mode: CalcMode) -> Self {
        Recalculator {
            build,
            data,
            mode,
            ovr: Override::default(),
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// The calculations with an override
    pub fn with_override(mut self, ovr: &Override) -> Self {
        self.ovr = ovr.clone();
        self
    }

    /// The calculations with the results of earlier ones cached, as
    /// upstream's `GlobalCache` keeps them between a calculator's
    /// calculations
    pub fn with_cache(self, cache: Cache) -> Self {
        *self.cache.borrow_mut() = cache;
        self
    }

    pub fn into_cache(self) -> Cache {
        self.cache.into_inner()
    }

    fn init_env(&self, build: &mut Build) -> Result<crate::calc_env::Env> {
        crate::calc_env::init_env_with(
            build,
            self.data,
            self.mode,
            self.mode.buff_mode(build),
            &self.ovr,
        )
    }
}

impl Recalc for Recalculator<'_, '_> {
    fn cached(&self, uuid: &str) -> Option<Rc<CachedCalc>> {
        self.cache.borrow().get(uuid).cloned()
    }

    fn calculator(&self) -> bool {
        self.mode == CalcMode::Calculator
    }

    fn store(&self, uuid: &str, calc: CachedCalc) {
        self.cache
            .borrow_mut()
            .insert(uuid.to_string(), Rc::new(calc));
    }

    fn calculate(&self, uuid: &str, limited: &[String]) -> Result<()> {
        let mut build = self.build.clone();
        let mut env = self.init_env(&mut build)?;
        let Some(main) = env
            .skills
            .active_skills
            .iter()
            .position(|s| skill_uuid(s, &env.groups) == uuid)
        else {
            // `[calcs.buildActiveSkill] Failed to process skill`
            return Ok(());
        };
        env.skills.main_skill = main;
        // `fullEnv.buildBreakdown = false`
        let mut input = PerformInput::new(&build, self.data, false)?;
        input.recalc = Some(self);
        input.limited = limited.to_vec();
        input.skip_ehp = true;
        let mut p = crate::calc_perform::perform(env, input)?;
        p.finish()
    }

    fn mirage(
        &self,
        mut ext: crate::calc_mirage::External,
        limited: &[String],
    ) -> Result<crate::mods::SymMap<OutVal>> {
        use crate::calc_skill_mods::{ModsInput, Owner};
        let mut build = self.build.clone();
        let mut env = self.init_env(&mut build)?;
        let data = self.data;
        let weapons = [
            ext.player_actor.weapon_data[0].clone().unwrap_or_default(),
            ext.player_actor.weapon_data[1].clone().unwrap_or_default(),
        ];
        let mut copy = crate::calc_skills::copy_active_skill(
            &ext.skill,
            &env.groups,
            &env.skills.from_item,
            &crate::calc_skills::ActorWeapons::new(&weapons),
            data.skills,
        );
        let mut db = crate::calc_skills::PlayerDb::new(
            ext.player_store.clone(),
            &ext.enemy_store,
            &ext.player_actor.item_list,
            &weapons,
            &build.items,
            data.items,
            data.skills,
        );
        let env_db = db.stores.push(env.player_db.clone());
        let config = build.config.active().input.clone();
        let placeholders = build.config.active().placeholder.clone();
        let input = ModsInput {
            setup: data.setup,
            skill_data: data.skills,
            tab: &build.items,
            item_data: data.items,
            item_list: &ext.player_actor.item_list,
            weapon_data: &weapons,
            mode: env.mode,
            calcs: env.calc_mode == CalcMode::Calcs,
            config: &config,
            placeholders: &placeholders,
            spectre_list: &build.info.spectre_list,
            character_level: build.info.character_level,
            enemy_level: env.enemy_level,
            iron_mass: env.setup.items.iron_mass_mods.is_some(),
            enemy_db: &ext.enemy_store,
        };
        let owner = Owner {
            env_db: Some(env_db),
            ..Owner::PLAYER
        };
        crate::calc_skill_mods::build_copied_skill_mod_list(
            &mut copy,
            &mut env.groups,
            &mut db,
            &mut Default::default(),
            &input,
            &owner,
        )?;
        // The mirage's mods, queried on the copy's mod list
        let mods = copy.mods.as_ref().ok_or("a copy without its mod list")?;
        let mut list = crate::mods::Store::list();
        list.parent = Some(db.id);
        for m in &mods.mods {
            list.add(m.clone());
        }
        let list = db.stores.push(list);
        db.actors.store_actor.insert(list, 0);
        let skill_cfg = mods.skill_cfg.clone();
        let sum = |cfg: Option<&crate::mods::Cfg>, name: &str| -> Result<f64> {
            db.query().sum(
                list,
                crate::mods::ModType::Base,
                Some(cfg.unwrap_or(&skill_cfg)),
                &[Sym::new(name)],
            )
        };
        ext.list_mods = ext.kind.mods(&sum)?;
        // `usedByMirage`, no parts or minions shown, used by another thing
        let other = data.skills.skill_type("OtherThingUsesSkill").unwrap_or(-1);
        copy.skill_types.insert(other);
        let mods = copy.mods.as_mut().ok_or("a copy without its mod list")?;
        let cond = Sym::new("usedByMirage");
        for c in std::iter::once(&mut mods.skill_cfg).chain(mods.weapon_cfgs.iter_mut().flatten()) {
            c.skill_cond
                .get_or_insert_with(Default::default)
                .insert(cond);
        }
        mods.skill_flags.remove("multiPart");
        mods.skill_flags.remove("haveMinion");
        let mut limited = limited.to_vec();
        limited.push(skill_uuid(&copy, &env.groups));
        ext.skill = copy;
        let mut input = PerformInput::new(&build, self.data, false)?;
        input.recalc = Some(self);
        input.limited = limited;
        input.external = Some(ext);
        let mut p = crate::calc_perform::perform(env, input)?;
        p.finish()?;
        Ok(p.c.actors.actors[crate::calc::PLAYER].output.clone())
    }
}

impl<'a> Perform<'a> {
    /// The rest of `calcs.perform(env, skipEHP)` after the setup: the
    /// defence, the triggers, mirages and offence of the player and the
    /// minion, then the main skill's results cached
    pub fn finish(&mut self) -> Result<()> {
        let skip = self.input.skip_ehp;
        self.defence(crate::calc::PLAYER)?;
        if !skip {
            self.defence_estimations(crate::calc::PLAYER)?;
        }
        self.player_offence()?;
        if let Some(a) = self.minion_actor() {
            self.defence(a)?;
            if !skip {
                self.defence_estimations(a)?;
            }
            self.triggers(a)?;
            let main = self.actor_main(a).ok_or("a minion without a main skill")?;
            self.offence(a, main)?;
        }
        self.cache_main();
        Ok(())
    }

    /// `cacheData(cacheSkillUUID(env.player.mainSkill, env), env)`
    pub fn cache_main(&self) {
        let Some(recalc) = self.input.recalc else {
            return;
        };
        let main = self.main();
        recalc.store(
            &self.skill_uuid(main),
            CachedCalc {
                output: self.c.actors.actors[crate::calc::PLAYER].output.clone(),
                skill: self.skill(main).clone(),
                minion_output: self
                    .minion_actor()
                    .map(|a| self.c.actors.actors[a].output.clone()),
                player_store: self.c.store(self.pdb()).clone(),
                enemy_store: self.c.store(self.edb()).clone(),
                player_actor: self.c.actors.actors[crate::calc::PLAYER].clone(),
                enemy_actor: self.c.actors.actors[crate::calc::ENEMY].clone(),
            },
        );
    }

    /// The cached calculation of skill `i`, calculating it unless it is
    /// cached (`GlobalCache.cachedData[env.mode][uuid]` after
    /// `calcs.buildActiveSkill`); `limit_self` adds the skill to the
    /// limited ones, as `getCachedOutputValue` does. With `refresh`, a
    /// calculator recalculates it anyway (`or env.mode == "CALCULATOR"`).
    pub(crate) fn cached_calc(
        &self,
        i: usize,
        limit_self: bool,
        refresh: bool,
    ) -> Result<Option<Rc<CachedCalc>>> {
        let recalc = self
            .input
            .recalc
            .ok_or("not ported: calculations of other skills without a recalculator")?;
        let uuid = self.skill_uuid(i);
        if recalc.cached(&uuid).is_none() || (refresh && recalc.calculator()) {
            let mut limited = self.input.limited.clone();
            if limit_self {
                limited.push(uuid.clone());
            }
            recalc.calculate(&uuid, &limited)?;
        }
        Ok(recalc.cached(&uuid))
    }

    /// Whether skill `i` is one whose calculations are limited
    /// (`env.limitedSkills[cacheSkillUUID(skill, env)]`)
    pub(crate) fn is_limited(&self, i: usize) -> bool {
        !self.input.limited.is_empty() && self.input.limited.contains(&self.skill_uuid(i))
    }
}
