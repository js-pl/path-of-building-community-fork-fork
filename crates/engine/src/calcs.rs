//! The calculation system (`Modules/Calcs.lua`) on the typed engine:
//! `calcs.buildOutput`, `calcs.calcFullDPS` and `calcs.buildActiveSkill`,
//! with `calcs.perform` run to its end and the skill cache upstream keeps
//! in `GlobalCache.cachedData` (`cacheSkillUUID`, `cacheData` of
//! `Modules/Common.lua`).
//!
//! Not ported yet: the Calcs tab's output (`CALCS` mode), the node and
//! misc calculators (`getNodeCalculator`, `getMiscCalculator`).
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::actor::OutVal;
use crate::breakdown::{Bd, BdVal};
use crate::build::Build;
use crate::calc::PLAYER;
use crate::calc_env::{CalcData, Env};
use crate::calc_perform::{Perform, PerformInput, no_whitespace, perform};
use crate::calc_setup::BuffMode;
use crate::mods::{ModValue, Sym};
use crate::skills::SocketGroup;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

/// An actor's output
pub type Output = HashMap<Sym, OutVal>;

/// The calculation modes, which keep their own cache
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Main,
    Calcs,
    Calculator,
}

impl Mode {
    fn buff_mode(self) -> BuffMode {
        BuffMode::new(Some("EFFECTIVE"))
    }
}

/// A skill's cached calculation (`GlobalCache.cachedData[mode][uuid]`)
#[derive(Clone, Debug)]
pub struct Cached {
    /// `Env.player.output`
    pub output: Output,
    /// `Env.player.mainSkill.skillData`
    pub skill_data: BTreeMap<String, ModValue>,
}

impl Cached {
    pub fn num(&self, k: &str) -> Option<f64> {
        self.output.get(&Sym::new(k)).and_then(OutVal::num)
    }
}

/// What a whole calculation leaves: the player's and `env.minion`'s
/// outputs
#[derive(Clone, Debug, Default)]
pub struct Outputs {
    pub player: Output,
    pub minion: Option<Output>,
    /// The breakdowns of the Calcs tab's calculation
    pub player_breakdown: Option<crate::breakdown::Bd>,
    pub minion_breakdown: Option<crate::breakdown::Bd>,
}

/// `cacheSkillUUID(skill, env)`
pub fn skill_uuid(p: &Perform, i: usize) -> String {
    env_skill_uuid(&p.env, i)
}

/// `cacheSkillUUID(skill, env)` of an environment's skill
pub fn env_skill_uuid(env: &Env, i: usize) -> String {
    skill_uuid_in(&env.skills.active_skills[i], &env.groups)
}

/// `cacheSkillUUID(skill, env)` of a skill of the socket groups
pub fn skill_uuid_in(skill: &crate::calc_skills::ActiveSkill, groups: &[SocketGroup]) -> String {
    let name = no_whitespace(&skill.active_effect.granted_effect.name);
    let group = skill.group.and_then(|g| groups.get(g));
    let slot = group
        .and_then(|g| g.slot.as_deref())
        .map_or("NO_SLOT".to_string(), |s| no_whitespace(&s.to_uppercase()));
    let slot_index = match (group, skill.active_effect.src) {
        (Some(_), Some((_, k))) => k + 1,
        _ => 1,
    };
    let group_index = match (group, skill.group) {
        (Some(_), Some(g)) => g + 1,
        _ => 1,
    };
    format!("{name}_{slot}_{slot_index}_{group_index}")
}

/// The calculations of one build, with the skill cache they share
pub struct Calcs<'d> {
    pub data: &'d CalcData<'d>,
    cache: RefCell<HashMap<(Mode, String), Rc<Cached>>>,
    /// The override of the calculation running (`env.override`), which
    /// the skills it calculates again share
    pub override_: RefCell<crate::calc_env::Override>,
}

impl<'d> Calcs<'d> {
    pub fn new(data: &'d CalcData<'d>) -> Calcs<'d> {
        Calcs {
            data,
            cache: RefCell::new(HashMap::new()),
            override_: RefCell::new(crate::calc_env::Override::default()),
        }
    }

    /// `wipeGlobalCache()`
    pub fn wipe_cache(&self) {
        self.cache.borrow_mut().clear();
    }

    pub fn cached(&self, mode: Mode, uuid: &str) -> Option<Rc<Cached>> {
        self.cache.borrow().get(&(mode, uuid.to_string())).cloned()
    }

    /// `calcs.initEnv(build, mode)`
    pub fn init_env(&self, build: &Build, mode: Mode) -> Result<Env> {
        crate::calc_env::init_env_copy(
            build,
            self.data,
            mode.buff_mode(),
            mode == Mode::Calcs,
            mode == Mode::Main,
            None,
            &self.override_.borrow(),
        )
    }

    /// `calcs.perform(env, skipEHP)` to its end: the setup, the player's
    /// defence, estimations, triggers and offence, then the minion's;
    /// caches the main skill's calculation. `f` reads the calculation
    /// before it is dropped.
    #[allow(clippy::too_many_arguments)]
    pub fn perform<T>(
        &self,
        build: &Build,
        env: Env,
        mode: Mode,
        skip_ehp: bool,
        limited: &[String],
        f: impl FnOnce(&Perform) -> T,
    ) -> Result<(T, Env)> {
        self.perform_with(build, env, mode, skip_ehp, limited, None, f)
    }

    /// [`Calcs::perform`] with a mirage's copy of a skill as the last
    /// skill and main skill
    #[allow(clippy::too_many_arguments)]
    pub fn perform_with<'b, T>(
        &'b self,
        build: &'b Build,
        env: Env,
        mode: Mode,
        skip_ehp: bool,
        limited: &[String],
        extra_main: Option<crate::calc_perform::ExtraMain<'b>>,
        f: impl FnOnce(&Perform) -> T,
    ) -> Result<(T, Env)> {
        let mut input = PerformInput::new(build, self.data, mode == Mode::Calcs)?;
        input.skip_ehp = skip_ehp;
        input.calcs = Some(self);
        input.build = Some(build);
        input.mode = mode;
        input.limited = limited.to_vec();
        input.extra_main = extra_main;
        let mut p = perform(env, input)?;
        p.finish()?;
        let main = p.main();
        let (uuid, skill_data) = match &p.replaced_main {
            Some((u, d)) => (u.clone(), d.clone()),
            None => (skill_uuid(&p, main), p.mods(main).skill_data.clone()),
        };
        let cached = Cached {
            output: p.c.actors.actors[PLAYER].output.clone(),
            skill_data,
        };
        self.cache
            .borrow_mut()
            .insert((mode, uuid), Rc::new(cached));
        let r = f(&p);
        let mut env = p.env;
        // The minions' skills are made again by the next calculation
        env.skills.active_skills.truncate(p.n_player);
        Ok((r, env))
    }

    /// `calcs.buildActiveSkill(env, mode, skill, targetUUID,
    /// limitedProcessingFlags)`: `limited` are the skills whose triggers
    /// and cached stages are not calculated again
    pub fn build_active_skill(
        &self,
        build: &Build,
        mode: Mode,
        uuid: &str,
        limited: &[String],
    ) -> Result<()> {
        let mut env = self.init_env(build, mode)?;
        let found = (0..env.skills.active_skills.len()).find(|&i| env_skill_uuid(&env, i) == uuid);
        let Some(i) = found else {
            return Ok(());
        };
        env.skills.main_skill = i;
        self.perform(build, env, mode, true, limited, |_| ())?;
        Ok(())
    }

    /// `calcs.calcFullDPS(build, mode)`: the Full DPS results
    /// (`SkillDPS`, `FullDPS`, `FullDotDPS`)
    pub fn calc_full_dps(&self, build: &Build, mode: Mode) -> Result<FullDps> {
        let mut env = self.init_env(build, mode)?;
        let mut full = FullDps::default();
        let mut sources = Sources::default();
        let dot_cap = self.data.setup.misc("DotDpsCap");
        let n = env.skills.active_skills.len();
        for i in 0..n {
            let skill = &env.skills.active_skills[i];
            let Some(group) = skill.group.and_then(|g| env.groups.get(g)) else {
                continue;
            };
            if !group.include_in_full_dps {
                continue;
            }
            let (count, enabled) = active_skill_count(&env, i);
            if !enabled {
                continue;
            }
            env.skills.main_skill = i;
            let (step, next) =
                self.perform(build, env, mode, true, &[], |p| full_dps_step(p, i))?;
            env = next;
            full.add(step?, count, &mut sources);
        }
        full.finish(&sources, dot_cap);
        Ok(full)
    }

    /// `calcs.getMiscCalculator(build)`'s base pass: the player's output
    /// in `CALCULATOR` mode, with Full DPS if a skill is in it, and
    /// whether one is
    pub fn misc_calculator_base(&self, build: &Build) -> Result<(Output, bool)> {
        *self.override_.borrow_mut() = crate::calc_env::Override::default();
        let env = self.init_env(build, Mode::Calculator)?;
        let (mut out, _) = self.perform(build, env, Mode::Calculator, false, &[], |p| {
            p.c.actors.actors[PLAYER].output.clone()
        })?;
        let full = self.calc_full_dps(build, Mode::Calculator)?;
        let used = !full.skills.is_empty();
        if used {
            full.write(&mut out);
        }
        Ok((out, used))
    }

    /// A call of `calcs.getMiscCalculator(build)`'s function:
    /// `calcFunc(override, useFullDPS)`. `used_full_dps` is the base
    /// pass's; `tree_view` is `build.viewMode == "TREE"`
    pub fn misc_calculate(
        &self,
        build: &Build,
        ov: crate::calc_env::Override,
        use_full_dps: Option<bool>,
        used_full_dps: bool,
        tree_view: bool,
    ) -> Result<Output> {
        *self.override_.borrow_mut() = ov;
        let r = (|| {
            let env = self.init_env(build, Mode::Calculator)?;
            let (mut out, _) = self.perform(build, env, Mode::Calculator, false, &[], |p| {
                p.c.actors.actors[PLAYER].output.clone()
            })?;
            if (use_full_dps != Some(false) || tree_view) && used_full_dps {
                let full = self.calc_full_dps(build, Mode::Calculator)?;
                full.write(&mut out);
            }
            Ok(out)
        })();
        *self.override_.borrow_mut() = crate::calc_env::Override::default();
        r
    }

    /// `calcs.buildOutput(build, "MAIN")`
    pub fn build_output_main(&self, build: &mut Build) -> Result<Outputs> {
        Ok(self.build_output_main_with(build)?.0)
    }

    /// `calcs.buildOutput(build, "MAIN")`, with its `multipliersUsed`
    pub fn build_output_main_with(
        &self,
        build: &mut Build,
    ) -> Result<(Outputs, std::collections::HashSet<String>)> {
        let env = self.init_env(build, Mode::Main)?;
        crate::calc_env::write_back(build, &env);
        let build = &*build;
        let (out, _) = self.perform(build, env, Mode::Main, false, &[], |p| {
            let skills: Vec<(String, String, bool)> = (0..p.n_player)
                .map(|i| (skill_uuid(p, i), p.skill_name(i).to_string(), i == p.main()))
                .collect();
            let pdb = p.c.db(PLAYER);
            let es_protects = p.c.flag(pdb, None, &["EnergyShieldProtectsMana"])?;
            let extra_points = p.c.base(pdb, None, &["ExtraPoints"])?;
            let tree = crate::calc_perform::source_cfg("Tree");
            let spec = |names: &[&str]| p.c.inc(pdb, Some(&tree), names);
            let specs = [
                ("Spec:LifeInc", spec(&["Life"])?),
                ("Spec:ManaInc", spec(&["Mana"])?),
                ("Spec:ArmourInc", spec(&["Armour", "ArmourAndEvasion"])?),
                ("Spec:EvasionInc", spec(&["Evasion", "ArmourAndEvasion"])?),
                ("Spec:EnergyShieldInc", spec(&["EnergyShield"])?),
            ];
            let outputs = Outputs {
                player: p.c.actors.actors[PLAYER].output.clone(),
                minion: p
                    .minion_actor()
                    .map(|a| p.c.actors.actors[a].output.clone()),
                ..Outputs::default()
            };
            let used = multipliers_used(p);
            Ok::<_, String>((outputs, skills, es_protects, extra_points, specs, used))
        })?;
        let (mut outputs, skills, es_protects, extra_points, specs, used) = out?;
        let full = self.calc_full_dps(build, Mode::Calculator)?;
        let player = &mut outputs.player;
        full.write(player);
        let num = |o: &Output, k: &str| o.get(&Sym::new(k)).and_then(OutVal::num);
        for (uuid, name, is_main) in &skills {
            if self.cached(Mode::Main, uuid).is_none() {
                self.build_active_skill(build, Mode::Main, uuid, &[])?;
            }
            let Some(cached) = self.cached(Mode::Main, uuid) else {
                continue;
            };
            // `modDB:Flag` is nil without the flag
            if es_protects {
                player.insert(Sym::new("EnergyShieldProtectsMana"), OutVal::Bool(true));
            } else {
                player.remove(&Sym::new("EnergyShieldProtectsMana"));
            }
            for (pool, cost) in [
                ("LifeUnreserved", "LifeCost"),
                ("ManaUnreserved", "ManaCost"),
                ("Rage", "RageCost"),
                ("EnergyShield", "ESCost"),
            ] {
                let Some(cached_cost) = cached.num(cost) else {
                    continue;
                };
                let total = if es_protects && cost == "ManaCost" {
                    num(player, "EnergyShield").unwrap_or(0.0)
                } else {
                    0.0
                } + num(player, pool).unwrap_or(0.0);
                if total < cached_cost {
                    let raw = pool.trim_end_matches("Unreserved");
                    let reservation = cached
                        .skill_data
                        .get(&format!("{raw}ReservedPercent"))
                        .and_then(ModValue::as_number);
                    let short = match reservation {
                        None => true,
                        Some(r) => {
                            total + (num(player, raw).unwrap_or(0.0) * r / 100.0).ceil()
                                < cached_cost
                        }
                    };
                    if short {
                        if *is_main {
                            player.insert(Sym::new(&format!("{cost}Warning")), OutVal::Bool(true));
                        }
                        push_list(player, &format!("{cost}WarningList"), name);
                    }
                }
            }
            for (pool, cost) in [
                ("LifeUnreservedPercent", "LifePercentCost"),
                ("ManaUnreservedPercent", "ManaPercentCost"),
            ] {
                if let Some(cached_cost) = cached.num(cost)
                    && num(player, pool).unwrap_or(0.0) < cached_cost
                {
                    push_list(player, &format!("{cost}PercentCostWarningList"), name);
                }
            }
        }
        player.insert(Sym::new("ExtraPoints"), OutVal::Num(extra_points));
        for (k, v) in specs {
            player.insert(Sym::new(k), OutVal::Num(v));
        }
        Ok((outputs, used))
    }
}

/// Appends a name to an output list
fn push_list(o: &mut Output, k: &str, name: &str) {
    let sym = Sym::new(k);
    let mut list = match o.get(&sym) {
        Some(OutVal::List(l)) => (**l).clone(),
        _ => Vec::new(),
    };
    list.push(OutVal::Str(name.into()));
    o.insert(sym, OutVal::List(Rc::new(list)));
}

/// `getActiveSkillCount(activeSkill)`
fn active_skill_count(env: &Env, i: usize) -> (f64, bool) {
    let skill = &env.skills.active_skills[i];
    let Some(group) = skill.group.and_then(|g| env.groups.get(g)) else {
        return (1.0, true);
    };
    if let Some(n) = group.group_count {
        return (n, true);
    }
    let ge = &skill.active_effect.granted_effect;
    for gem in &group.gem_list {
        let Some(data) = &gem.gem else { continue };
        let count = gem.count.unwrap_or(1.0);
        if data.vaal_gem {
            if Rc::ptr_eq(ge, &data.granted_effect) || ge.id == data.granted_effect.id {
                return (count, gem.enable_global1);
            }
            if data
                .secondary_granted_effect
                .as_ref()
                .is_some_and(|s| s.id == ge.id)
            {
                return (count, gem.enable_global2);
            }
        } else if (data.granted_effect.id == ge.id && !data.granted_effect.support)
            || data
                .secondary_granted_effect
                .as_ref()
                .is_some_and(|s| s.id == ge.id)
        {
            return (count, true);
        }
    }
    (1.0, true)
}

/// What one skill's calculation adds to Full DPS
#[derive(Clone, Debug, Default)]
struct Step {
    name: String,
    skill_part: Option<String>,
    info_trigger: Option<String>,
    dot_can_stack: bool,
    player: Output,
    minion: Option<(String, Output)>,
    /// Absolution, Dominating Blow and Holy Strike count their skill
    /// damage once
    counted_once: bool,
    info_message2: Option<String>,
    mirage: Option<crate::calc_skill_mods::MirageInfo>,
}

fn full_dps_step(p: &Perform, i: usize) -> Result<Step> {
    let mods = p.mods(i);
    let name = p.skill_name(i).to_string();
    let pdb = p.c.db(PLAYER);
    let minion = match p.minion_actor() {
        Some(a) => {
            let m = p.env_minion().ok_or("no minion")?;
            let typ = &p.mods(m.skill).minion.as_ref().ok_or("no minion")?.typ;
            let md = p.data().minions.get(typ).ok_or("no minion data")?;
            Some((
                md.str("name").unwrap_or("").to_string(),
                p.c.actors.actors[a].output.clone(),
            ))
        }
        None => None,
    };
    let once = |k: &str| p.c.flag(pdb, None, &[k]);
    let counted_once = (name.contains("Absolution")
        && once("Condition:AbsolutionSkillDamageCountedOnce")?)
        || (name.contains("Dominating Blow")
            && once("Condition:DominatingBlowSkillDamageCountedOnce")?)
        || (name.contains("Holy Strike") && once("Condition:HolyStrikeSkillDamageCountedOnce")?);
    Ok(Step {
        name,
        skill_part: mods.skill_part_name.clone(),
        info_trigger: mods.info_trigger.clone(),
        dot_can_stack: mods.skill_flags.contains("DotCanStack"),
        player: p.c.actors.actors[PLAYER].output.clone(),
        minion,
        counted_once,
        info_message2: mods.info_message2.clone(),
        mirage: mods.mirage.clone(),
    })
}

/// The sources of the best ailments
#[derive(Default)]
struct Sources {
    bleed: String,
    corrupting_blood: String,
    ignite: String,
    burning_ground: String,
    caustic_ground: String,
}

/// `fullDPS` of `calcs.calcFullDPS`
#[derive(Clone, Debug, Default)]
pub struct FullDps {
    pub combined_dps: f64,
    pub total_dot_dps: f64,
    pub skills: Vec<OutVal>,
    total_poison_dps: f64,
    caustic_ground_dps: f64,
    impale_dps: f64,
    ignite_dps: f64,
    burning_ground_dps: f64,
    bleed_dps: f64,
    corrupting_blood_dps: f64,
    decay_dps: f64,
    dot_dps: f64,
    culling_multi: f64,
    culling_dps: Option<f64>,
}

fn entry(fields: &[(&str, Option<OutVal>)]) -> OutVal {
    OutVal::Table(Rc::new(
        fields
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.clone()?)))
            .collect(),
    ))
}

impl FullDps {
    fn add(&mut self, mut step: Step, mut count: f64, src: &mut Sources) {
        let num = |o: &Output, k: &str| o.get(&Sym::new(k)).and_then(OutVal::num);
        let s = |x: &str| Some(OutVal::Str(x.into()));
        let n = |x: f64| Some(OutVal::Num(x));
        let mut minion_name: Option<String> = None;
        if let Some((mname, mo)) = &step.minion {
            if let Some(dps) = num(mo, "TotalDPS").filter(|d| *d > 0.0) {
                let prefix = format!("{mname}: ");
                let part = step.skill_part.clone().unwrap_or_default();
                self.skills.push(entry(&[
                    ("name", s(&step.name)),
                    ("dps", n(dps)),
                    ("count", n(count)),
                    ("trigger", step.info_trigger.as_deref().and_then(s)),
                    ("skillPart", s(&format!("{prefix}{part}"))),
                ]));
                self.combined_dps += dps * count;
                minion_name = Some(prefix);
            }
            if let Some(v) = num(mo, "BleedDPS").filter(|v| *v > self.bleed_dps) {
                self.bleed_dps = v;
                src.bleed = step.name.clone();
            }
            if let Some(v) = num(mo, "IgniteDPS").filter(|v| *v > self.ignite_dps) {
                self.ignite_dps = v;
                src.ignite = step.name.clone();
            }
            if num(mo, "PoisonDPS").is_some_and(|v| v > 0.0) {
                self.total_poison_dps += num(mo, "TotalPoisonDPS").unwrap_or(f64::NAN) * count;
            }
            if let Some(v) = num(mo, "ImpaleDPS").filter(|v| *v > 0.0) {
                self.impale_dps += v * count;
            }
            if let Some(v) = num(mo, "DecayDPS").filter(|v| *v > 0.0) {
                self.decay_dps += v;
            }
            if let Some(v) = num(mo, "TotalDot").filter(|v| *v > 0.0) {
                self.dot_dps += v;
            }
            if let Some(v) =
                num(mo, "CullMultiplier").filter(|v| *v > 1.0 && *v > self.culling_multi)
            {
                self.culling_multi = v;
            }
            if step.counted_once {
                count = 1.0;
                step.info_message2 = Some("Skill Damage".into());
            }
        }
        if let Some(m) = &step.mirage {
            let mo = &m.output;
            let mcount = m.count.unwrap_or(1.0) * count;
            if let Some(dps) = num(mo, "TotalDPS").filter(|d| *d > 0.0) {
                self.skills.push(entry(&[
                    ("name", s(&format!("{} (Mirage)", m.name))),
                    ("dps", n(dps)),
                    ("count", n(mcount)),
                    ("skillPart", m.skill_part_name.as_deref().and_then(s)),
                ]));
                self.combined_dps += dps * mcount;
            }
            let mname = format!("{} (Mirage)", step.name);
            if let Some(v) = num(mo, "BleedDPS").filter(|v| *v > self.bleed_dps) {
                self.bleed_dps = v;
                src.bleed = mname.clone();
            }
            if let Some(v) = num(mo, "IgniteDPS").filter(|v| *v > self.ignite_dps) {
                self.ignite_dps = v;
                src.ignite = mname.clone();
            }
            if num(mo, "PoisonDPS").is_some_and(|v| v > 0.0) {
                self.total_poison_dps += num(mo, "TotalPoisonDPS").unwrap_or(f64::NAN) * mcount;
            }
            if let Some(v) = num(mo, "ImpaleDPS").filter(|v| *v > 0.0) {
                self.impale_dps += v * mcount;
            }
            if let Some(v) = num(mo, "DecayDPS").filter(|v| *v > 0.0) {
                self.decay_dps += v;
            }
            let player_dot = num(&step.player, "TotalDot");
            if let Some(v) = num(mo, "TotalDot").filter(|v| *v > 0.0)
                && (step.dot_can_stack || player_dot == Some(0.0))
            {
                self.dot_dps += v * if step.dot_can_stack { mcount } else { 1.0 };
            }
            if let Some(v) =
                num(mo, "CullMultiplier").filter(|v| *v > 1.0 && *v > self.culling_multi)
            {
                self.culling_multi = v;
            }
            if let Some(v) = num(mo, "BurningGroundDPS").filter(|v| *v > self.burning_ground_dps) {
                self.burning_ground_dps = v;
                src.burning_ground = mname.clone();
            }
            if let Some(v) = num(mo, "CausticGroundDPS").filter(|v| *v > self.caustic_ground_dps) {
                self.caustic_ground_dps = v;
                src.caustic_ground = mname;
            }
        }
        let po = &step.player;
        if let Some(dps) = num(po, "TotalDPS").filter(|d| *d > 0.0) {
            let part = if minion_name.is_some() {
                step.info_message2.clone().or(step.skill_part.clone())
            } else {
                step.skill_part.clone()
            };
            self.skills.push(entry(&[
                ("name", s(&step.name)),
                ("dps", n(dps)),
                ("count", n(count)),
                ("trigger", step.info_trigger.as_deref().and_then(s)),
                ("skillPart", part.as_deref().and_then(s)),
            ]));
            self.combined_dps += dps * count;
        }
        if let Some(v) = num(po, "BleedDPS").filter(|v| *v > self.bleed_dps) {
            self.bleed_dps = v;
            src.bleed = step.name.clone();
        }
        if let Some(v) = num(po, "CorruptingBloodDPS").filter(|v| *v > self.corrupting_blood_dps) {
            self.corrupting_blood_dps = v;
            src.corrupting_blood = step.name.clone();
        }
        if let Some(v) = num(po, "IgniteDPS").filter(|v| *v > self.ignite_dps) {
            self.ignite_dps = v;
            src.ignite = step.name.clone();
        }
        if let Some(v) = num(po, "BurningGroundDPS").filter(|v| *v > self.burning_ground_dps) {
            self.burning_ground_dps = v;
            src.burning_ground = step.name.clone();
        }
        if num(po, "PoisonDPS").is_some_and(|v| v > 0.0) {
            self.total_poison_dps += num(po, "TotalPoisonDPS").unwrap_or(f64::NAN) * count;
        }
        if let Some(v) = num(po, "CausticGroundDPS").filter(|v| *v > self.caustic_ground_dps) {
            self.caustic_ground_dps = v;
            src.caustic_ground = step.name.clone();
        }
        if let Some(v) = num(po, "ImpaleDPS").filter(|v| *v > 0.0) {
            self.impale_dps += v * count;
        }
        if let Some(v) = num(po, "DecayDPS").filter(|v| *v > 0.0) {
            self.decay_dps += v;
        }
        if let Some(v) = num(po, "TotalDot").filter(|v| *v > 0.0) {
            self.dot_dps += v * if step.dot_can_stack { count } else { 1.0 };
        }
        if let Some(v) = num(po, "CullMultiplier").filter(|v| *v > 1.0 && *v > self.culling_multi) {
            self.culling_multi = v;
        }
    }

    fn finish(&mut self, src: &Sources, dot_cap: f64) {
        let s = |x: &str| Some(OutVal::Str(x.into()));
        let n = |x: f64| Some(OutVal::Num(x));
        let one = n(1.0);
        self.total_dot_dps = 0.0;
        let add_dot = |me: &mut Self, name: &str, dps: f64, source: Option<&str>| {
            me.skills.push(entry(&[
                ("name", s(name)),
                ("dps", n(dps)),
                ("count", one.clone()),
                ("source", source.and_then(s)),
            ]));
            me.total_dot_dps += dps;
        };
        if self.bleed_dps > 0.0 {
            add_dot(self, "Best Bleed DPS", self.bleed_dps, Some(&src.bleed));
        }
        if self.corrupting_blood_dps > 0.0 {
            add_dot(
                self,
                "Corrupting Blood DPS",
                self.corrupting_blood_dps,
                Some(&src.corrupting_blood),
            );
        }
        if self.ignite_dps > 0.0 {
            add_dot(self, "Best Ignite DPS", self.ignite_dps, Some(&src.ignite));
        }
        if self.burning_ground_dps > 0.0 {
            add_dot(
                self,
                "Best Burning Ground DPS",
                self.burning_ground_dps,
                Some(&src.burning_ground),
            );
        }
        if self.total_poison_dps > 0.0 {
            self.total_poison_dps = self.total_poison_dps.min(dot_cap);
            add_dot(self, "Full Poison DPS", self.total_poison_dps, None);
        }
        if self.caustic_ground_dps > 0.0 {
            add_dot(
                self,
                "Best Caustic Ground DPS",
                self.caustic_ground_dps,
                Some(&src.caustic_ground),
            );
        }
        if self.impale_dps > 0.0 {
            self.skills.push(entry(&[
                ("name", s("Full Impale DPS")),
                ("dps", n(self.impale_dps)),
                ("count", one.clone()),
            ]));
            self.combined_dps += self.impale_dps;
        }
        if self.decay_dps > 0.0 {
            add_dot(self, "Full Decay DPS", self.decay_dps, None);
        }
        if self.dot_dps > 0.0 {
            add_dot(self, "Full DoT DPS", self.dot_dps, None);
        }
        self.total_dot_dps = self.total_dot_dps.min(dot_cap);
        self.combined_dps += self.total_dot_dps;
        if self.culling_multi > 0.0 {
            let c = self.combined_dps * (self.culling_multi - 1.0);
            self.culling_dps = Some(c);
            self.skills.push(entry(&[
                ("name", s("Full Culling DPS")),
                ("dps", n(c)),
                ("count", one),
            ]));
            self.combined_dps += c;
        }
    }

    /// `output.SkillDPS`, `FullDPS` and `FullDotDPS`
    pub fn write(&self, o: &mut Output) {
        o.insert(
            Sym::new("SkillDPS"),
            OutVal::List(Rc::new(self.skills.clone())),
        );
        o.insert(Sym::new("FullDPS"), OutVal::Num(self.combined_dps));
        o.insert(Sym::new("FullDotDPS"), OutVal::Num(self.total_dot_dps));
    }
}

/// `table.sort(t, less)`: Lua 5.1's `auxsort`, which LuaJIT keeps, so
/// elements that compare equal end up in upstream's order
pub fn lua_sort<T: Clone>(v: &mut [T], less: impl Fn(&T, &T) -> bool) -> Result<()> {
    let n = v.len();
    // 1-based, as upstream's
    let mut stack = vec![(1usize, n)];
    let lt = |v: &[T], a: usize, b: usize| less(&v[a - 1], &v[b - 1]);
    while let Some((mut l, mut u)) = stack.pop() {
        while l < u {
            if lt(v, u, l) {
                v.swap(l - 1, u - 1);
            }
            if u - l == 1 {
                break;
            }
            let mut i = (l + u) / 2;
            if lt(v, i, l) {
                v.swap(i - 1, l - 1);
            } else if lt(v, u, i) {
                v.swap(i - 1, u - 1);
            }
            if u - l == 2 {
                break;
            }
            let p = v[i - 1].clone();
            v.swap(i - 1, u - 2);
            i = l;
            let mut j = u - 1;
            loop {
                loop {
                    i += 1;
                    if i > n {
                        return Err("invalid order function for sorting".into());
                    }
                    if !less(&v[i - 1], &p) {
                        break;
                    }
                    if i > u {
                        return Err("invalid order function for sorting".into());
                    }
                }
                loop {
                    if j <= 1 {
                        return Err("invalid order function for sorting".into());
                    }
                    j -= 1;
                    if !less(&p, &v[j - 1]) {
                        break;
                    }
                    if j < l {
                        return Err("invalid order function for sorting".into());
                    }
                }
                if j < i {
                    break;
                }
                v.swap(i - 1, j - 1);
            }
            v.swap(u - 2, i - 1);
            if i - l < u - i {
                stack.push((i + 1, u));
                u = i - 1;
            } else {
                stack.push((l, i - 1));
                l = i + 1;
            }
        }
    }
    Ok(())
}

/// What the sidebar does to the main output (`buildMode:RefreshStatList`):
/// sorts `SkillDPS` by total damage
pub fn refresh_stat_list(out: &mut Outputs) -> Result<()> {
    let k = Sym::new("SkillDPS");
    if let Some(OutVal::List(l)) = out.player.get(&k) {
        let mut l = (**l).clone();
        let total = |e: &OutVal| -> f64 {
            let OutVal::Table(t) = e else { return f64::NAN };
            let n = |k: &str| t.get(k).and_then(OutVal::num).unwrap_or(f64::NAN);
            n("dps") * n("count")
        };
        lua_sort(&mut l, |a, b| total(a) > total(b))?;
        out.player.insert(k, OutVal::List(Rc::new(l)));
    }
    Ok(())
}

/// What a mirage's calculation leaves (`newEnv`, `newSkill`)
#[derive(Clone, Debug, Default)]
pub struct MirageOut {
    pub outputs: Outputs,
    /// The copy's skill part and its name, and its skill data
    pub skill_part: Option<f64>,
    pub skill_part_name: Option<String>,
    pub skill_data: BTreeMap<String, ModValue>,
}

impl<'d> Calcs<'d> {
    /// `calcs.copyActiveSkill(env, "CALCULATOR", skill)` and the copy's
    /// calculation (`calculateMirage`): the copy is used by the mirage,
    /// `pre` is upstream's `preCalcFunc` on it
    pub fn mirage_perform<'b>(
        &'b self,
        build: &'b Build,
        uuid: &str,
        mirage_uses: Option<f64>,
        limited: &[String],
        pre: crate::calc_perform::ExtraMain<'b>,
    ) -> Result<MirageOut> {
        let mut env = crate::calc_env::init_env_copy(
            build,
            self.data,
            Mode::Calculator.buff_mode(),
            false,
            false,
            Some(uuid),
            &self.override_.borrow(),
        )?;
        let mut copy = env
            .skills
            .active_skills
            .pop()
            .ok_or("no copy of the skill")?;
        let other = self
            .data
            .skills
            .skill_type("OtherThingUsesSkill")
            .ok_or("no skill type OtherThingUsesSkill")?;
        copy.skill_types.insert(other);
        let mods = copy.mods.as_mut().ok_or("a skill without its mod list")?;
        let cond = Sym::new("usedByMirage");
        mods.skill_cfg
            .skill_cond
            .get_or_insert_with(Default::default)
            .insert(cond);
        mods.skill_cfg.skill_types = Some(copy.skill_types.clone());
        for c in mods.weapon_cfgs.iter_mut().flatten() {
            c.skill_cond
                .get_or_insert_with(Default::default)
                .insert(cond);
            c.skill_types = Some(copy.skill_types.clone());
        }
        mods.skill_flags.remove("multiPart");
        mods.skill_flags.remove("haveMinion");
        match mirage_uses {
            Some(u) => mods
                .skill_data
                .insert("mirageUses".into(), ModValue::Number(u)),
            None => mods.skill_data.remove("mirageUses"),
        };
        env.skills.active_skills.push(copy);
        env.skills.main_skill = env.skills.active_skills.len() - 1;
        let mut limited = limited.to_vec();
        limited.push(uuid.to_string());
        let (out, _) = self.perform_with(
            build,
            env,
            Mode::Calculator,
            false,
            &limited,
            Some(pre),
            |p| {
                let main = p.main();
                MirageOut {
                    outputs: Outputs {
                        player: p.c.actors.actors[PLAYER].output.clone(),
                        minion: p
                            .minion_actor()
                            .map(|a| p.c.actors.actors[a].output.clone()),
                        ..Outputs::default()
                    },
                    skill_part: p.mods(main).skill_part,
                    skill_part_name: p.mods(main).skill_part_name.clone(),
                    skill_data: p.mods(main).skill_data.clone(),
                }
            },
        )?;
        Ok(out)
    }
}

/// The `multipliersUsed` of the main calculation: the multipliers the
/// player's mods and skills' mods are tagged with (`buildOutput`, `MAIN`)
fn multipliers_used(p: &Perform) -> std::collections::HashSet<String> {
    use crate::mods::store::Mods;
    use crate::mods::tag::{Names, TagKind};
    let mut used = std::collections::HashSet::new();
    let mut scan = |m: &crate::mods::Mod| {
        if m.source.as_deref() == Some("Base") {
            return;
        }
        for t in &m.tags {
            let (vars, actor) = match &t.kind {
                TagKind::Other { kind, .. } if kind.as_str() == "IgnoreCond" => break,
                TagKind::Multiplier(x) => (&x.vars, &x.actor),
                TagKind::MultiplierThreshold(x) => (&x.vars, &x.actor),
                _ => continue,
            };
            if actor.is_none() {
                match vars {
                    Some(Names::One(v)) => {
                        used.insert(v.as_str().to_string());
                    }
                    Some(Names::List(l)) => {
                        for v in l {
                            used.insert(v.as_str().to_string());
                        }
                    }
                    None => {}
                }
            }
        }
    };
    let mods_of = |id| -> Vec<crate::mods::Mod> {
        match &p.c.store(id).mods {
            Mods::Db(m) => m.values().flatten().cloned().collect(),
            Mods::List(l) => l.clone(),
        }
    };
    for m in mods_of(p.c.db(PLAYER)) {
        scan(&m);
    }
    for i in 0..p.n_player {
        for m in mods_of(p.lists[i].base) {
            scan(&m);
        }
    }
    used
}

/// The Calcs tab's lists of the buffs, combat buffs and curses
/// (`buildOutput`, `CALCS`)
/// The lists of the buffs `sorted` names, in that order
fn buff_list<'x>(
    names: &[String],
    lists: &'x [Vec<crate::mods::Mod>],
    sorted: &[String],
) -> Vec<&'x Vec<crate::mods::Mod>> {
    sorted
        .iter()
        .filter_map(|n| names.iter().position(|x| x == n).map(|i| &lists[i]))
        .collect()
}

fn calcs_lists(
    p: &Perform,
    main_multipliers: &std::collections::HashSet<String>,
) -> Result<CalcsLists> {
    let pdb = p.c.db(PLAYER);
    let out = |a: usize, k: &str| p.c.out(a, k).unwrap_or(f64::NAN);
    let mut combat: Vec<String> = Vec::new();
    for (k, label) in [
        ("PowerCharges", "Power Charges"),
        ("AbsorptionCharges", "Absorption Charges"),
        ("FrenzyCharges", "Frenzy Charges"),
        ("AfflictionCharges", "Affliction Charges"),
        ("EnduranceCharges", "Endurance Charges"),
        ("BrutalCharges", "Brutal Charges"),
        ("BrineCharges", "Brine Charges"),
        ("SiphoningCharges", "Siphoning Charges"),
        ("ChallengerCharges", "Challenger Charges"),
        ("BlitzCharges", "Blitz Charges"),
    ] {
        if out(PLAYER, k) > 0.0 {
            combat.push(format!("{} {label}", out(PLAYER, k) as i64));
        }
    }
    if main_multipliers.contains("InspirationCharge") {
        combat.push(format!(
            "{} Inspiration Charges",
            out(PLAYER, "InspirationCharges") as i64
        ));
    }
    if out(PLAYER, "GhostShrouds") > 0.0 {
        combat.push(format!(
            "{} Ghost Shrouds",
            out(PLAYER, "GhostShrouds") as i64
        ));
    }
    if out(PLAYER, "CrabBarriers") > 0.0 {
        combat.push(format!(
            "{} Crab Barriers",
            out(PLAYER, "CrabBarriers") as i64
        ));
    }
    for (m, k, label) in [
        ("BloodCharge", "BloodCharges", "Blood Charges"),
        ("SpiritCharge", "SpiritCharges", "Spirit Charges"),
        ("SpiritInfusion", "SpiritInfusions", "Spirit Infusions"),
    ] {
        if main_multipliers.contains(m) {
            combat.push(format!("{} {label}", out(PLAYER, k) as i64));
        }
    }
    let main = p.main();
    if p.c.flag(p.lists[main].base, None, &["Cruelty"])? {
        combat.push("Cruelty".into());
    }
    for (flag, label) in [
        ("Fortify", "Fortify"),
        ("Onslaught", "Onslaught"),
        ("UnholyMight", "Unholy Might"),
        ("ChaoticMight", "Chaotic Might"),
        ("Tailwind", "Tailwind"),
        ("Adrenaline", "Adrenaline"),
        ("AlchemistsGenius", "Alchemist's Genius"),
        ("HerEmbrace", "Her Embrace"),
        ("AccelerationShrine", "Acceleration Shrine"),
        ("BrutalShrine", "Brutal Shrine"),
        ("DiamondShrine", "Diamond Shrine"),
        ("DivineShrine", "Divine Shrine"),
        ("EchoingShrine", "Echoing Shrine"),
        ("GloomShrine", "Gloom Shrine"),
        ("GreaterFreezingShrine", "Greater Freezing Shrine"),
        ("GreaterShockingShrine", "Greater Shocking Shrine"),
        ("GreaterSkeletalShrine", "Greater Skeletal Shrine"),
        ("ImpenetrableShrine", "Impenetrable Shrine"),
        ("MassiveShrine", "Massive Shrine"),
        ("ReplenishingShrine", "Replenishing Shrine"),
        ("ResistanceShrine", "Resistance Shrine"),
        ("ResonatingShrine", "Resonating Shrine"),
        ("LesserAccelerationShrine", "Lesser Acceleration Shrine"),
        ("LesserBrutalShrine", "Lesser Brutal Shrine"),
        ("LesserImpenetrableShrine", "Lesser Impenetrable Shrine"),
        ("LesserMassiveShrine", "Lesser Massive Shrine"),
        ("LesserReplenishingShrine", "Lesser Replenishing Shrine"),
        ("LesserResistanceShrine", "Lesser Resistance Shrine"),
    ] {
        if p.c.flag(pdb, None, &[flag])? {
            combat.push(label.into());
        }
    }
    let mut buffs: Vec<String> = p.buff_names.clone();
    if p.c.flag(pdb, None, &["Elusive"])? {
        combat.push("Elusive".into());
    }
    buffs.sort();
    let edb = p.c.db(crate::calc::ENEMY);
    // `breakdown.SkillBuffs` and `SkillDebuffs`: the buffs' and debuffs'
    // mods that apply
    let mod_rows = |db, lists: Vec<&Vec<crate::mods::Mod>>| -> Result<Bd> {
        let mut rows = Bd::new();
        for l in lists {
            for m in l {
                let v = p.c.query().eval_mod(db, m, None, None)?;
                let v = match v {
                    Some(ModValue::Number(n)) if n != 0.0 => BdVal::Num(n),
                    Some(ModValue::Number(_)) | None => continue,
                    Some(ModValue::Bool(b)) => BdVal::Bool(b),
                    Some(ModValue::Str(s)) => BdVal::Str(s),
                    Some(v) => BdVal::Value(v),
                };
                let mut r = Bd::new();
                r.set("mod", BdVal::Mod(Rc::new(m.clone())));
                r.set("value", v);
                rows.push(r);
            }
        }
        let mut t = Bd::new();
        t.set("modList", rows);
        Ok(t)
    };
    let skill_buffs = mod_rows(pdb, buff_list(&p.buff_names, &p.buff_lists, &buffs))?;
    let mut curses: Vec<String> = p.debuffs.iter().map(|d| d.0.clone()).collect();
    curses.sort();
    let mut debuff_lists: Vec<&Vec<crate::mods::Mod>> = Vec::new();
    for c in &curses {
        if let Some(d) = p.debuffs.iter().find(|d| d.0 == *c) {
            debuff_lists.push(&d.1);
        }
    }
    for l in p.curse_slot_mods.iter().flatten() {
        debuff_lists.push(l);
    }
    let skill_debuffs = mod_rows(edb, debuff_lists)?;
    for c in curses.iter_mut() {
        let mods = &p.debuffs.iter().find(|d| d.0 == *c).ok_or("no debuff")?.1;
        let stacks = crate::mods::list::sum(
            mods,
            crate::mods::ModType::Base,
            &format!("Multiplier:{c}Stack"),
        )?;
        if stacks > 0.0 {
            let plural = if stacks > 1.0 { "s" } else { "" };
            *c = format!("{c} ({} stack{plural})", crate::pat::num(stacks));
        }
    }
    curses.extend(p.curse_slots.iter().cloned());
    let curse_list = curses.join(", ");
    let mut bds: Vec<(String, Bd)> = vec![
        ("SkillBuffs".into(), skill_buffs),
        ("SkillDebuffs".into(), skill_debuffs.clone()),
    ];
    let mut out_lists = vec![
        ("BuffList".to_string(), OutVal::Str(buffs.join(", ").into())),
        (
            "CombatList".to_string(),
            OutVal::Str(combat.join(", ").into()),
        ),
        (
            "CurseList".to_string(),
            OutVal::Str(curse_list.as_str().into()),
        ),
    ];
    if let Some(a) = p.minion_actor() {
        let mdb = p.c.db(a);
        let mut combat: Vec<String> = Vec::new();
        for (k, label) in [
            ("PowerCharges", "Power Charges"),
            ("FrenzyCharges", "Frenzy Charges"),
            ("EnduranceCharges", "Endurance Charges"),
        ] {
            if out(a, k) > 0.0 {
                combat.push(format!("{} {label}", out(a, k) as i64));
            }
        }
        for (flag, label) in [
            ("Fortify", "Fortify"),
            ("Onslaught", "Onslaught"),
            ("UnholyMight", "Unholy Might"),
            ("ChaoticMight", "Chaotic Might"),
            ("Tailwind", "Tailwind"),
            ("DiamondShrine", "Diamond Shrine"),
            ("MassiveShrine", "Massive Shrine"),
        ] {
            if p.c.flag(mdb, None, &[flag])? {
                combat.push(label.into());
            }
        }
        let mut buffs = p.minion_buff_names.clone();
        buffs.sort();
        let rows = mod_rows(
            mdb,
            buff_list(&p.minion_buff_names, &p.minion_buff_lists, &buffs),
        )?;
        bds.push(("Minion.SkillBuffs".into(), rows));
        bds.push(("Minion.SkillDebuffs".into(), skill_debuffs.clone()));
        out_lists.push((
            "Minion.BuffList".into(),
            OutVal::Str(buffs.join(", ").into()),
        ));
        out_lists.push((
            "Minion.CombatList".into(),
            OutVal::Str(combat.join(", ").into()),
        ));
        out_lists.push(("Minion.CurseList".into(), OutVal::Str(curse_list.into())));
    }
    Ok((out_lists, bds))
}

/// The Calcs tab's lists (`BuffList`, ...) and the breakdowns of the buffs
/// and debuffs, `Minion.` prefixed for the minion's
type CalcsLists = (Vec<(String, OutVal)>, Vec<(String, Bd)>);

impl<'d> Calcs<'d> {
    /// `calcsTab:BuildOutput()`: `calcs.buildOutput(build, "MAIN")`, then
    /// `"CALCS"`
    pub fn build_outputs(&self, build: &mut Build) -> Result<(Outputs, Outputs)> {
        let (main, used) = self.build_output_main_with(build)?;
        let calcs = self.build_output_calcs(build, &used)?;
        Ok((main, calcs))
    }

    /// `calcs.buildOutput(build, "CALCS")`
    pub fn build_output_calcs(
        &self,
        build: &mut Build,
        main_multipliers: &std::collections::HashSet<String>,
    ) -> Result<Outputs> {
        let env = self.init_env(build, Mode::Calcs)?;
        crate::calc_env::write_back(build, &env);
        let build = &*build;
        let (out, _) = self.perform(build, env, Mode::Calcs, false, &[], |p| {
            let lists = calcs_lists(p, main_multipliers)?;
            Ok::<_, String>((
                Outputs {
                    player: p.c.actors.actors[PLAYER].output.clone(),
                    minion: p
                        .minion_actor()
                        .map(|a| p.c.actors.actors[a].output.clone()),
                    player_breakdown: p.c.actors.actors[PLAYER].breakdown.clone(),
                    minion_breakdown: p
                        .minion_actor()
                        .and_then(|a| p.c.actors.actors[a].breakdown.clone()),
                },
                lists,
            ))
        })?;
        let (mut outputs, (lists, bds)) = out?;
        for (k, b) in bds {
            let bd = match k.strip_prefix("Minion.") {
                Some(k) => outputs
                    .minion_breakdown
                    .as_mut()
                    .map(|bd| (bd, k.to_string())),
                None => outputs.player_breakdown.as_mut().map(|bd| (bd, k.clone())),
            };
            if let Some((bd, k)) = bd {
                bd.set(&k, b);
            }
        }
        let full = self.calc_full_dps(build, Mode::Calculator)?;
        full.write(&mut outputs.player);
        for (k, v) in lists {
            match k.strip_prefix("Minion.") {
                Some(k) => {
                    if let Some(m) = outputs.minion.as_mut() {
                        m.insert(Sym::new(k), v);
                    }
                }
                None => {
                    outputs.player.insert(Sym::new(&k), v);
                }
            }
        }
        Ok(outputs)
    }
}
