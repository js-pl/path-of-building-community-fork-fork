//! Item, gem and anoint power: the sorts of the Items tab's item database
//! (`ItemDBControl:ListBuilder`), the Skills tab's gem dropdown
//! (`GemSelectControl:DPSBuilder`) and the anoint popup
//! (`NotableDBControl:ListBuilder`). Each runs the build's calculator
//! (`CalcsTab:GetMiscCalculator`) over the candidates and sorts by a power
//! stat of `data.powerStatList`, as node power (`power.rs`) does for the
//! tree. Natively the candidates' calculations run on a thread pool, in the
//! browser one after another; results do not depend on the thread count.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::build::Build;
use crate::calc_override::Override;
use crate::calc_perform::Result;
use crate::calculator::{CalcOutput, MiscCalculator};
use crate::parallel::{self, Threads};
use crate::power::PowerStat;

/// Item power (`ItemDBControl:ListBuilder`): each item's `measuredPower`
/// is the greatest of its power in each slot it fits, by the stat
/// (`data.powerStatList.GetFromOutput`). `items` gives the overrides for
/// each item's slots (`{ repSlotName, repItem }`, or `{ toggleFlask }` /
/// `{ toggleTincture }`), in the order the tree's slots list them. An item
/// that fits no slot keeps `-inf`, as upstream's `-math.huge` does.
pub fn item_power(
    calc: &MiscCalculator,
    items: Vec<Vec<Override>>,
    stat: &PowerStat,
    threads: Threads,
) -> Vec<Result<f64>> {
    let full_dps = is_full_dps(stat);
    parallel::map(items, threads, |slots| {
        let mut best = f64::NEG_INFINITY;
        for ovr in slots {
            let out = calc.calculate(&ovr, full_dps)?;
            best = crate::calc::m_max(best, stat.value(&out));
        }
        Ok(best)
    })
}

/// Anoint power (`NotableDBControl:ListBuilder`): each notable's
/// `measuredPower` is its power when anointed over the base (the item with
/// no anoint), by the stat (`CalculatePowerStat(sortDetail, output,
/// calcBase)`). `base` is `{ repSlotName, repItem = anointItem(nil) }` and
/// each of `nodes` is `{ repSlotName, repItem = anointItem(node) }`. The
/// anoint popup passes no `useFullDPS`, so each calculation reruns Full DPS.
pub fn anoint_power(
    calc: &MiscCalculator,
    base: Override,
    nodes: Vec<Override>,
    stat: &PowerStat,
    threads: Threads,
) -> Result<Vec<Result<f64>>> {
    let base = stat.value(&calc.calculate(&base, true)?);
    Ok(parallel::map(nodes, threads, |ovr| {
        let out = calc.calculate(&ovr, true)?;
        Ok(stat.value(&out) - base)
    }))
}

/// A gem swapped into the display socket group, for [`gem_power`]: the
/// build with the candidate gem in place (`CalcOutputWithThisGem`), which
/// a calculation of the build gives in place of the calculator's.
pub struct GemCandidate<'t> {
    pub build: Build<'t>,
}

/// Gem power (`GemSelectControl:DPSBuilder`): each candidate gem's DPS, by
/// the Skills tab's `sortGemsByDPSField` (`FullDPS`, `CombinedDPS`,
/// `TotalDPS`, ...), when swapped into the display socket group. A gem swap
/// changes the socket group, not something a `CalcOverride` expresses, so
/// each candidate is a build of its own ([`MiscCalculator::calculate_on`]).
pub fn gem_power(
    calc: &MiscCalculator,
    cands: Vec<GemCandidate>,
    field: &str,
    threads: Threads,
) -> Vec<Result<f64>> {
    let full_dps = field == "FullDPS";
    parallel::map(cands, threads, |cand| {
        let out = calc.calculate_on(&cand.build, &Override::default(), full_dps)?;
        Ok(gem_dps(&out, field))
    })
}

/// The base DPS a gem's is compared with (`DPSBuilder`'s `baseDPS`): the
/// calculator's own output by the field
pub fn base_gem_dps(calc: &MiscCalculator, field: &str) -> f64 {
    gem_dps(calc.base(), field)
}

/// Whether the stat needs Full DPS recalculated (`useFullDPS = stat ==
/// "FullDPS"` of the item and gem sorts)
fn is_full_dps(stat: &PowerStat) -> bool {
    stat.stat.as_deref() == Some("FullDPS")
}

/// `(dpsField == "FullDPS" and output[dpsField]) or (output.Minion and
/// output.Minion.CombinedDPS) or output[dpsField] or 0`
fn gem_dps(out: &CalcOutput, field: &str) -> f64 {
    if field == "FullDPS"
        && let Some(v) = out.num(field)
    {
        return v;
    }
    if out.minion.is_some()
        && let Some(v) = out.minion_num("CombinedDPS")
    {
        return v;
    }
    out.num(field).unwrap_or(0.0)
}
