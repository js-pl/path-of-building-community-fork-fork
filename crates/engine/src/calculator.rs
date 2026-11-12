//! Calculators: `calcs.getMiscCalculator` (`Modules/Calcs.lua`), the
//! calculation of a build with an override (allocated or deallocated
//! nodes, another item in a slot, a flask toggled, ...), which node power
//! and the item, gem and config comparisons compare with the build's own.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::actor::OutVal;
use crate::build::Build;
use crate::calc::PLAYER;
use crate::calc_cache::{Cache, Recalculator};
use crate::calc_env::{CalcData, CalcMode, init_env_with};
use crate::calc_override::Override;
use crate::calc_perform::{PerformInput, Result, perform};
use crate::calcs::{FullDps, FullDpsSkill, Output, calc_full_dps_with};
use crate::mods::Sym;
use std::rc::Rc;

/// A calculator's results: `env.player.output`, and the main skill's
/// minion's (`output.Minion`)
#[derive(Clone, Debug)]
pub struct CalcOutput {
    pub player: Output,
    pub minion: Option<Output>,
}

impl CalcOutput {
    pub fn num(&self, k: &str) -> Option<f64> {
        self.player.get(&Sym::new(k)).and_then(OutVal::num)
    }

    pub fn minion_num(&self, k: &str) -> Option<f64> {
        self.minion
            .as_ref()?
            .get(&Sym::new(k))
            .and_then(OutVal::num)
    }

    /// The Full DPS results as upstream sets them on the output
    fn set_full_dps(&mut self, full: &FullDps) {
        let skills = full.skills.iter().map(FullDpsSkill::out_val).collect();
        let mut set = |k: &str, v| {
            self.player.insert(Sym::new(k), v);
        };
        set("SkillDPS", OutVal::List(Rc::new(skills)));
        set("FullDPS", OutVal::Num(full.combined_dps));
        set("FullDotDPS", OutVal::Num(full.total_dot_dps));
    }
}

/// `calcs.initEnv(build, "CALCULATOR", override)` and `calcs.perform(env)`
/// on the calculator's cache, which it gives back
fn calculate(
    build: &Build,
    data: &CalcData,
    ovr: &Override,
    cache: Cache,
) -> Result<(CalcOutput, Cache)> {
    let mut b = build.clone();
    let mode = CalcMode::Calculator;
    let base = mode.buff_mode(&b);
    let env = init_env_with(&mut b, data, mode, base, ovr)?;
    let recalc = Recalculator::new(&b, data, mode)
        .with_override(ovr)
        .with_cache(cache);
    let mut input = PerformInput::new(&b, data, false)?;
    input.recalc = Some(&recalc);
    let mut p = perform(env, input)?;
    p.finish()?;
    let minion = p
        .minion_actor()
        .map(|a| p.c.actors.actors[a].output.clone());
    let out = CalcOutput {
        player: std::mem::take(&mut p.c.actors.actors[PLAYER].output),
        minion,
    };
    drop(p);
    Ok((out, recalc.into_cache()))
}

/// `calcs.getMiscCalculator(build)`.
///
/// Upstream's calculators share the results of other skills' calculations
/// (`GlobalCache`), and some lookups (Saviour's mirages, ...) take a
/// cached result without calculating it again: each calculation here
/// starts from the cache of the build's own calculation, so calculations
/// are independent of each other.
pub struct MiscCalculator<'a, 't> {
    build: Build<'t>,
    data: &'a CalcData<'a>,
    base: CalcOutput,
    cache: Cache,
    used_full_dps: bool,
}

impl<'a, 't> MiscCalculator<'a, 't> {
    /// The calculator of a build after its sidebar's and Calcs tab's
    /// calculations, with the build's own calculation
    pub fn new(build: &Build<'t>, data: &'a CalcData<'a>) -> Result<Self> {
        let none = Override::default();
        let (mut base, cache) = calculate(build, data, &none, Cache::new())?;
        let mode = CalcMode::Calculator.buff_mode(build);
        let (full, cache) = calc_full_dps_with(build, data, mode, &none, cache)?;
        let used_full_dps = !full.skills.is_empty();
        if used_full_dps {
            base.set_full_dps(&full);
        }
        Ok(MiscCalculator {
            build: build.clone(),
            data,
            base,
            cache,
            used_full_dps,
        })
    }

    /// The build's own calculation (`calcBase`)
    pub fn base(&self) -> &CalcOutput {
        &self.base
    }

    pub fn build(&self) -> &Build<'t> {
        &self.build
    }

    /// `calcFunc(override, useFullDPS)`: with `full_dps` (`useFullDPS ~=
    /// false or build.viewMode == "TREE"`), the Full DPS is calculated again
    /// when the build has Full DPS skills
    pub fn calculate(&self, ovr: &Override, full_dps: bool) -> Result<CalcOutput> {
        let (mut out, cache) = calculate(&self.build, self.data, ovr, self.cache.clone())?;
        if full_dps && self.used_full_dps {
            let mode = CalcMode::Calculator.buff_mode(&self.build);
            let (full, _) = calc_full_dps_with(&self.build, self.data, mode, ovr, cache)?;
            out.set_full_dps(&full);
        }
        Ok(out)
    }
}
