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
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::actor::OutVal;
use crate::build::Build;
use crate::calc_env::CalcData;
use crate::calc_perform::{Perform, PerformInput, Result};
use crate::calc_setup::BuffMode;
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
    pub output: HashMap<Sym, OutVal>,
    /// `ActiveSkill`: the main skill with its mods and skill data
    pub skill: ActiveSkill,
    /// `Env.minion.output`
    pub minion_output: Option<HashMap<Sym, OutVal>>,
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
    /// Caches a calculation's results (`cacheData`)
    fn store(&self, uuid: &str, calc: CachedCalc);
    /// `calcs.buildActiveSkill(env, mode, skill, uuid, limited)`: a
    /// calculation of the build with the skill as the main one, whose
    /// results it caches. `limited` are the skills whose own calculations
    /// it skips.
    fn calculate(&self, uuid: &str, limited: &[String]) -> Result<()>;
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

/// Recalculates a build with other main skills
pub struct Recalculator<'a, 't> {
    /// The build after its main calculation's setup
    pub build: &'a Build<'t>,
    pub data: &'a CalcData<'a>,
    pub mode: BuffMode,
    cache: RefCell<HashMap<String, Rc<CachedCalc>>>,
}

impl<'a, 't> Recalculator<'a, 't> {
    pub fn new(build: &'a Build<'t>, data: &'a CalcData<'a>, mode: BuffMode) -> Self {
        Recalculator {
            build,
            data,
            mode,
            cache: RefCell::new(HashMap::new()),
        }
    }
}

impl Recalc for Recalculator<'_, '_> {
    fn cached(&self, uuid: &str) -> Option<Rc<CachedCalc>> {
        self.cache.borrow().get(uuid).cloned()
    }

    fn store(&self, uuid: &str, calc: CachedCalc) {
        self.cache
            .borrow_mut()
            .insert(uuid.to_string(), Rc::new(calc));
    }

    fn calculate(&self, uuid: &str, limited: &[String]) -> Result<()> {
        let mut build = self.build.clone();
        let mut env = crate::calc_env::init_env(&mut build, self.data, self.mode)?;
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
        let mut input = PerformInput::new(&build, self.data, false)?;
        input.recalc = Some(self);
        input.limited = limited.to_vec();
        input.skip_ehp = true;
        let mut p = crate::calc_perform::perform(env, input)?;
        p.finish()
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
            },
        );
    }

    /// The cached calculation of skill `i`, calculating it unless it is
    /// cached (`GlobalCache.cachedData[env.mode][uuid]` after
    /// `calcs.buildActiveSkill`); `limit_self` adds the skill to the
    /// limited ones, as `getCachedOutputValue` does
    pub(crate) fn cached_calc(&self, i: usize, limit_self: bool) -> Result<Option<Rc<CachedCalc>>> {
        let recalc = self
            .input
            .recalc
            .ok_or("not ported: calculations of other skills without a recalculator")?;
        let uuid = self.skill_uuid(i);
        if recalc.cached(&uuid).is_none() {
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
