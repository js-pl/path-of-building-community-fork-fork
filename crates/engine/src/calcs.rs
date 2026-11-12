//! `Modules/Calcs.lua`: `calcs.buildOutput` of the sidebar's calculation
//! (`mode == "MAIN"`) and the Calcs tab's (`mode == "CALCS"`) with
//! `calcs.calcFullDPS`, from a loaded build.
//!
//! The Full DPS skills are calculated each in a calculation of its own
//! from the setup, where upstream reuses the first calculation's setup;
//! the Calcs tab's calculation reuses the sidebar's Full DPS results.
//!
//! Not ported yet: the conditions and mods in use that the
//! Configuration tab reads (`env.conditionsUsed`, ...; of them only
//! `env.multipliersUsed`). The calculators are in `calculator.rs`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::actor::OutVal;
use crate::breakdown::{Breakdown, Breakdowns, ModEntry};
use crate::build::Build;
use crate::calc::{PLAYER, m_min};
use crate::calc_cache::{Cache, Recalc, Recalculator, skill_uuid};
use crate::calc_env::{CalcData, CalcMode, init_env, init_env_with};
use crate::calc_override::Override;
use crate::calc_perform::BuffMap;
use crate::calc_perform::{Perform, PerformInput, Result, perform};
use crate::calc_setup::BuffMode;
use crate::calc_skills::ActiveSkill;
use crate::mods::{Cfg, Mod, ModType, ModValue, Mods, Names, StoreId, Sym, TagKind};
use crate::skills::SocketGroup;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

/// An actor's output
pub type Output = crate::mods::SymMap<OutVal>;

/// A row of the Full DPS table (`fullDPS.skills`)
#[derive(Clone, Debug, PartialEq)]
pub struct FullDpsSkill {
    pub name: String,
    pub dps: f64,
    pub count: f64,
    pub trigger: Option<String>,
    pub skill_part: Option<String>,
    /// The skill an ailment's best DPS comes from
    pub source: Option<String>,
}

/// `calcs.calcFullDPS`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FullDps {
    pub skills: Vec<FullDpsSkill>,
    pub combined_dps: f64,
    pub total_dot_dps: f64,
}

/// What `calcs.buildOutput(build, mode)` leaves: the player's output,
/// with the Full DPS results, and the main skill's minion's
#[derive(Clone, Debug)]
pub struct BuildOutput {
    pub player: Output,
    pub minion: Option<Output>,
    /// The Full DPS skills in `calcs.calcFullDPS`'s order
    pub full_dps: FullDps,
    /// `env.multipliersUsed` of the sidebar's calculation
    pub multipliers_used: HashSet<Sym>,
    /// The Calcs tab's breakdowns of the player and the minion
    pub breakdown: Option<Breakdowns>,
    pub minion_breakdown: Option<Breakdowns>,
}

fn num(out: &Output, k: &str) -> Option<f64> {
    out.get(&Sym::new(k)).and_then(OutVal::num)
}

fn arith(out: &Output, k: &str) -> Result<f64> {
    num(out, k).ok_or_else(|| format!("attempt to perform arithmetic on a nil value ({k})"))
}

/// `getActiveSkillCount(activeSkill)`: the skill's count and whether it
/// is enabled
fn active_skill_count(skill: &ActiveSkill, groups: &[SocketGroup]) -> (f64, bool) {
    let Some(group) = skill.group.and_then(|g| groups.get(g)) else {
        return (1.0, true);
    };
    if let Some(n) = group.group_count {
        return (n, true);
    }
    let effect = &skill.active_effect.granted_effect;
    for gem in &group.gem_list {
        let Some(data) = &gem.gem else { continue };
        let count = gem.count.unwrap_or(1.0);
        if data.vaal_gem {
            if Rc::ptr_eq(effect, &data.granted_effect) {
                return (count, gem.enable_global1);
            }
            if data
                .secondary_granted_effect
                .as_ref()
                .is_some_and(|s| Rc::ptr_eq(effect, s))
            {
                return (count, gem.enable_global2);
            }
        } else if (Rc::ptr_eq(effect, &data.granted_effect) && !data.granted_effect.support)
            || data
                .secondary_granted_effect
                .as_ref()
                .is_some_and(|s| Rc::ptr_eq(effect, s))
        {
            return (count, true);
        }
    }
    (1.0, true)
}

/// The best of an ailment's DPS over the skills, and where it comes from
#[derive(Default)]
struct Best {
    dps: f64,
    source: String,
}

impl Best {
    fn offer(&mut self, out: &Output, k: &str, source: impl FnOnce() -> String) {
        if let Some(v) = num(out, k)
            && v > self.dps
        {
            self.dps = v;
            self.source = source();
        }
    }
}

/// `calcs.calcFullDPS(build, "CALCULATOR", {}, specEnv)` with the base
/// databases of a calculation in the buff mode `base`
pub fn calc_full_dps(build: &Build, data: &CalcData, base: BuffMode) -> Result<FullDps> {
    Ok(calc_full_dps_with(build, data, base, &Override::default(), Cache::new())?.0)
}

/// [`calc_full_dps`] with an override, on the calculator's cache, which
/// it gives back
pub fn calc_full_dps_with(
    build: &Build,
    data: &CalcData,
    base: BuffMode,
    ovr: &Override,
    cache: Cache,
) -> Result<(FullDps, Cache)> {
    let recalc = Recalculator::new(build, data, CalcMode::Calculator)
        .with_override(ovr)
        .with_cache(cache);
    let setup = |b: &mut Build| init_env_with(b, data, CalcMode::Calculator, base, ovr);
    let mut first = Some(setup(&mut build.clone())?);
    let list: Vec<(usize, f64)> = {
        let env = first.as_ref().unwrap();
        env.skills
            .active_skills
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.group
                    .and_then(|g| env.groups.get(g))
                    .is_some_and(|g| g.include_in_full_dps)
            })
            .filter_map(|(i, s)| match active_skill_count(s, &env.groups) {
                (n, true) => Some((i, n)),
                _ => None,
            })
            .collect()
    };
    let mut full = FullDps::default();
    let [
        mut bleed,
        mut corrupting,
        mut ignite,
        mut burning,
        mut caustic,
    ]: [Best; 5] = Default::default();
    let (mut poison, mut impale, mut decay, mut dot, mut culling) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, mut count) in list {
        let mut b = build.clone();
        let mut env = match first.take() {
            Some(e) => e,
            None => setup(&mut b)?,
        };
        env.skills.main_skill = i;
        let mut input = PerformInput::new(&b, data, false)?;
        input.recalc = Some(&recalc);
        input.skip_ehp = true;
        let mut p = perform(env, input)?;
        p.finish()?;
        let name = p.skill_name(i).to_string();
        let mods = p.mods(i);
        let (part, trigger) = (mods.skill_part_name.clone(), mods.info_trigger.clone());
        let mut info_message2 = mods.info_message2.clone();
        let mut minion_name = None;
        if let Some(a) = p.minion_actor() {
            let out = &p.c.actors.actors[a].output;
            if let Some(dps) = num(out, "TotalDPS")
                && dps > 0.0
            {
                let m = &p.minions[p.env_minion.unwrap_or_default()].minion;
                let mname = data
                    .setup
                    .minions
                    .get(&m.typ)
                    .and_then(|d| d.str("name"))
                    .unwrap_or_default();
                let mname = format!("{mname}: ");
                let part = part.as_deref().ok_or(
                    "attempt to concatenate a nil value (field 'skillPartName')".to_string(),
                )?;
                full.skills.push(FullDpsSkill {
                    name: name.clone(),
                    dps,
                    count,
                    trigger: trigger.clone(),
                    skill_part: Some(format!("{mname}{part}")),
                    source: None,
                });
                full.combined_dps += dps * count;
                minion_name = Some(mname);
            }
            bleed.offer(out, "BleedDPS", || name.clone());
            ignite.offer(out, "IgniteDPS", || name.clone());
            if num(out, "PoisonDPS").is_some_and(|v| v > 0.0) {
                poison += arith(out, "TotalPoisonDPS")? * count;
            }
            if let Some(v) = num(out, "ImpaleDPS").filter(|v| *v > 0.0) {
                impale += v * count;
            }
            if let Some(v) = num(out, "DecayDPS").filter(|v| *v > 0.0) {
                decay += v;
            }
            if let Some(v) = num(out, "TotalDot").filter(|v| *v > 0.0) {
                dot += v;
            }
            if let Some(v) = num(out, "CullMultiplier").filter(|v| *v > 1.0 && *v > culling) {
                culling = v;
            }
            let pdb = p.c.db(PLAYER);
            let once = |skill: &str, cond: &str| -> Result<bool> {
                Ok(name.contains(skill) && p.c.flag(pdb, None, &[cond])?)
            };
            if once("Absolution", "Condition:AbsolutionSkillDamageCountedOnce")?
                || once(
                    "Dominating Blow",
                    "Condition:DominatingBlowSkillDamageCountedOnce",
                )?
                || once("Holy Strike", "Condition:HolyStrikeSkillDamageCountedOnce")?
            {
                count = 1.0;
                info_message2 = Some("Skill Damage".into());
            }
        }
        let player = &p.c.actors.actors[PLAYER].output;
        if let Some(m) = &p.mirage {
            let n = m.count.unwrap_or(1.0) * count;
            let out = &m.output;
            let mname = || format!("{name} (Mirage)");
            if let Some(dps) = num(out, "TotalDPS")
                && dps > 0.0
            {
                full.skills.push(FullDpsSkill {
                    name: format!("{} (Mirage)", m.name),
                    dps,
                    count: n,
                    trigger: None,
                    skill_part: m.skill_part_name.clone(),
                    source: None,
                });
                full.combined_dps += dps * n;
            }
            bleed.offer(out, "BleedDPS", mname);
            ignite.offer(out, "IgniteDPS", mname);
            if num(out, "PoisonDPS").is_some_and(|v| v > 0.0) {
                poison += arith(out, "TotalPoisonDPS")? * n;
            }
            if let Some(v) = num(out, "ImpaleDPS").filter(|v| *v > 0.0) {
                impale += v * n;
            }
            if let Some(v) = num(out, "DecayDPS").filter(|v| *v > 0.0) {
                decay += v;
            }
            let stack = p.has_flag(i, "DotCanStack");
            if let Some(v) = num(out, "TotalDot").filter(|v| *v > 0.0)
                && (stack || num(player, "TotalDot") == Some(0.0))
            {
                dot += v * if stack { n } else { 1.0 };
            }
            if let Some(v) = num(out, "CullMultiplier").filter(|v| *v > 1.0 && *v > culling) {
                culling = v;
            }
            burning.offer(out, "BurningGroundDPS", mname);
            caustic.offer(out, "CausticGroundDPS", mname);
        }
        let out = player;
        if let Some(dps) = num(out, "TotalDPS")
            && dps > 0.0
        {
            full.skills.push(FullDpsSkill {
                name: name.clone(),
                dps,
                count,
                trigger,
                skill_part: if minion_name.is_some() {
                    info_message2.or(part)
                } else {
                    part
                },
                source: None,
            });
            full.combined_dps += dps * count;
        }
        bleed.offer(out, "BleedDPS", || name.clone());
        corrupting.offer(out, "CorruptingBloodDPS", || name.clone());
        ignite.offer(out, "IgniteDPS", || name.clone());
        burning.offer(out, "BurningGroundDPS", || name.clone());
        if num(out, "PoisonDPS").is_some_and(|v| v > 0.0) {
            poison += arith(out, "TotalPoisonDPS")? * count;
        }
        caustic.offer(out, "CausticGroundDPS", || name.clone());
        if let Some(v) = num(out, "ImpaleDPS").filter(|v| *v > 0.0) {
            impale += v * count;
        }
        if let Some(v) = num(out, "DecayDPS").filter(|v| *v > 0.0) {
            decay += v;
        }
        if let Some(v) = num(out, "TotalDot").filter(|v| *v > 0.0) {
            dot += v * if p.has_flag(i, "DotCanStack") {
                count
            } else {
                1.0
            };
        }
        if let Some(v) = num(out, "CullMultiplier").filter(|v| *v > 1.0 && *v > culling) {
            culling = v;
        }
    }

    // The ailments' DPS
    let row = |name: &str, dps: f64, source: Option<&str>| FullDpsSkill {
        name: name.into(),
        dps,
        count: 1.0,
        trigger: None,
        skill_part: None,
        source: source.map(String::from),
    };
    let mut total_dot = 0.0;
    for (name, best) in [
        ("Best Bleed DPS", &bleed),
        ("Corrupting Blood DPS", &corrupting),
        ("Best Ignite DPS", &ignite),
        ("Best Burning Ground DPS", &burning),
    ] {
        if best.dps > 0.0 {
            full.skills.push(row(name, best.dps, Some(&best.source)));
            total_dot += best.dps;
        }
    }
    let cap = data.setup.misc("DotDpsCap");
    if poison > 0.0 {
        poison = m_min(poison, cap);
        full.skills.push(row("Full Poison DPS", poison, None));
        total_dot += poison;
    }
    if caustic.dps > 0.0 {
        full.skills.push(row(
            "Best Caustic Ground DPS",
            caustic.dps,
            Some(&caustic.source),
        ));
        total_dot += caustic.dps;
    }
    if impale > 0.0 {
        full.skills.push(row("Full Impale DPS", impale, None));
        full.combined_dps += impale;
    }
    if decay > 0.0 {
        full.skills.push(row("Full Decay DPS", decay, None));
        total_dot += decay;
    }
    if dot > 0.0 {
        full.skills.push(row("Full DoT DPS", dot, None));
        total_dot += dot;
    }
    full.total_dot_dps = m_min(total_dot, cap);
    full.combined_dps += full.total_dot_dps;
    if culling > 0.0 {
        let dps = full.combined_dps * (culling - 1.0);
        full.skills.push(row("Full Culling DPS", dps, None));
        full.combined_dps += dps;
    }
    Ok((full, recalc.into_cache()))
}

impl FullDpsSkill {
    /// The row as upstream's table
    pub fn out_val(&self) -> OutVal {
        let mut t = BTreeMap::new();
        let s = |v: &str| OutVal::Str(v.into());
        t.insert("name".into(), s(&self.name));
        t.insert("dps".into(), OutVal::Num(self.dps));
        t.insert("count".into(), OutVal::Num(self.count));
        for (k, v) in [
            ("trigger", &self.trigger),
            ("skillPart", &self.skill_part),
            ("source", &self.source),
        ] {
            if let Some(v) = v {
                t.insert(k.into(), s(v));
            }
        }
        OutVal::Table(Rc::new(t))
    }
}

/// `table.sort(t, less)`: Lua 5.1's `auxsort`, which LuaJIT keeps, so that
/// elements comparing equal end up in upstream's order
pub fn lua_sort<T>(t: &mut [T], less: impl Fn(&T, &T) -> bool) {
    if t.len() < 2 {
        return;
    }
    let mut stack = vec![(0, t.len() - 1)];
    while let Some((mut l, mut u)) = stack.pop() {
        while l < u {
            // Sort a[l], a[(l+u)/2] and a[u]
            if less(&t[u], &t[l]) {
                t.swap(l, u);
            }
            if u - l == 1 {
                break;
            }
            let mut i = (l + u) / 2;
            if less(&t[i], &t[l]) {
                t.swap(i, l);
            } else if less(&t[u], &t[i]) {
                t.swap(i, u);
            }
            if u - l == 2 {
                break;
            }
            // The pivot to a[u-1]; a[l] <= P == a[u-1] <= a[u]
            t.swap(i, u - 1);
            let p = u - 1;
            i = l;
            let mut j = u - 1;
            loop {
                i += 1;
                while less(&t[i], &t[p]) {
                    i += 1;
                }
                j -= 1;
                while less(&t[p], &t[j]) {
                    j -= 1;
                }
                if j < i {
                    break;
                }
                t.swap(i, j);
            }
            t.swap(u - 1, i);
            // Sort the smaller part next, the larger one later
            if i - l < u - i {
                stack.push((i + 1, u));
                u = i - 1;
            } else {
                stack.push((l, i - 1));
                l = i + 1;
            }
        }
    }
}

/// `CalcsTab:BuildOutput`: the sidebar's calculation, then the Calcs
/// tab's, which shares the first's Full DPS results
pub struct Outputs {
    pub main: BuildOutput,
    pub calcs: Result<BuildOutput>,
}

/// `CalcsTab:BuildOutput`
pub fn build_outputs(build: &mut Build, data: &CalcData) -> Result<Outputs> {
    let main = build_output(build, data, CalcMode::Main, None)?;
    let calcs = build_output(build, data, CalcMode::Calcs, Some(&main));
    Ok(Outputs { main, calcs })
}

/// `calcs.buildOutput(build, mode)`. The setup adds the granted skills'
/// socket groups to the build, as upstream's does. `main` is the sidebar's
/// calculation, whose Full DPS results the Calcs tab's reuses. The sidebar
/// sorts its Full DPS skills by their DPS in place
/// (`buildMode:AddDisplayStatList`); the Calcs tab's keep their order.
pub fn build_output(
    build: &mut Build,
    data: &CalcData,
    mode: CalcMode,
    main: Option<&BuildOutput>,
) -> Result<BuildOutput> {
    let env = init_env(build, data, mode)?;
    // `calcFullDPS` sets up from this calculation's base databases
    let base = env.mode;
    let build = &*build;
    let recalc = Recalculator::new(build, data, mode);
    let breakdown = matches!(mode, CalcMode::Main | CalcMode::Calcs);
    let mut input = PerformInput::new(build, data, breakdown)?;
    input.recalc = Some(&recalc);
    let mut p = perform(env, input)?;
    p.finish()?;
    let full_dps = match main {
        Some(m) if base == BuffMode::new(Some("EFFECTIVE")) => m.full_dps.clone(),
        _ => calc_full_dps(build, data, base)?,
    };
    let mut skills = full_dps.skills.clone();
    if mode == CalcMode::Main {
        lua_sort(&mut skills, |a, b| a.dps * a.count > b.dps * b.count);
    }
    let skills = skills.iter().map(FullDpsSkill::out_val).collect();
    p.c.set_out_val(PLAYER, "SkillDPS", OutVal::List(Rc::new(skills)));
    p.c.set_out(PLAYER, "FullDPS", full_dps.combined_dps);
    p.c.set_out(PLAYER, "FullDotDPS", full_dps.total_dot_dps);
    let mut multipliers_used = HashSet::new();
    match mode {
        CalcMode::Main => {
            main_extras(&mut p, &recalc)?;
            multipliers_used = used_multipliers(&p);
        }
        CalcMode::Calcs => {
            let used = main.map_or(&multipliers_used, |m| &m.multipliers_used);
            calcs_lists(&mut p, used)?;
        }
        CalcMode::Calculator => {}
    }
    let minion = p
        .minion_actor()
        .map(|a| p.c.actors.actors[a].output.clone());
    let minion_breakdown = p
        .minion_actor()
        .and_then(|a| p.c.actors.actors[a].breakdown.take())
        .map(|b| *b);
    let player = &mut p.c.actors.actors[PLAYER];
    Ok(BuildOutput {
        player: std::mem::take(&mut player.output),
        minion,
        full_dps,
        multipliers_used,
        breakdown: player.breakdown.take().map(|b| *b),
        minion_breakdown,
    })
}

/// What only the sidebar's calculation sets: the cost warnings, extra
/// points and the tree's increases
fn main_extras(p: &mut Perform, recalc: &Recalculator) -> Result<()> {
    cost_warnings(p, recalc)?;
    let pdb = p.c.db(PLAYER);
    let extra = p.c.sum(pdb, ModType::Base, None, &["ExtraPoints"])?;
    p.c.set_out(PLAYER, "ExtraPoints", extra);
    let tree = Cfg {
        source: Some("Tree".into()),
        ..Cfg::default()
    };
    for (k, names) in [
        ("Spec:LifeInc", &["Life"][..]),
        ("Spec:ManaInc", &["Mana"]),
        ("Spec:ArmourInc", &["Armour", "ArmourAndEvasion"]),
        ("Spec:EvasionInc", &["Evasion", "ArmourAndEvasion"]),
        ("Spec:EnergyShieldInc", &["EnergyShield"]),
    ] {
        let v = p.c.sum(pdb, ModType::Inc, Some(&tree), names)?;
        p.c.set_out(PLAYER, k, v);
    }
    Ok(())
}

/// The multipliers the player's mods and skills use without an actor
/// (`env.multipliersUsed`): the player's database and the base mod lists
/// of the player's skills, without the `Base` descriptors
fn used_multipliers(p: &Perform) -> HashSet<Sym> {
    let mut used = HashSet::new();
    let mut add = |m: &Mod| {
        if m.source.as_deref() == Some("Base") {
            return;
        }
        for t in &m.tags {
            let vars = match &t.kind {
                TagKind::Other { kind, .. } if kind.as_str() == "IgnoreCond" => break,
                TagKind::Multiplier(x) if x.actor.is_none() => &x.vars,
                TagKind::MultiplierThreshold(x) if x.actor.is_none() => &x.vars,
                _ => continue,
            };
            used.extend(vars.iter().flat_map(Names::iter));
        }
    };
    let stores =
        std::iter::once(p.c.db(PLAYER)).chain(p.lists[..p.n_player].iter().map(|l| l.base));
    for id in stores {
        match &p.c.store(id).mods {
            Mods::Db(by_name) => by_name.values().flatten().for_each(&mut add),
            Mods::List(list) => list.iter().for_each(&mut add),
        }
    }
    used
}

/// The charges, buffs and curses the Calcs tab lists (`BuffList`,
/// `CombatList`, `CurseList`), for the player and the main skill's minion.
/// `used` are the sidebar calculation's multipliers in use.
fn calcs_lists(p: &mut Perform, used: &HashSet<Sym>) -> Result<()> {
    let pdb = p.c.db(PLAYER);
    let out = |p: &Perform, a: usize, k: &str| {
        p.c.out(a, k)
            .ok_or_else(|| format!("attempt to compare nil with number ({k})"))
    };
    let mut combat = Vec::new();
    for (k, label) in CHARGES {
        let shown = match *k {
            "InspirationCharges" => used.contains(&Sym::new("InspirationCharge")),
            "BloodCharges" => used.contains(&Sym::new("BloodCharge")),
            "SpiritCharges" => used.contains(&Sym::new("SpiritCharge")),
            "SpiritInfusions" => used.contains(&Sym::new("SpiritInfusion")),
            _ => out(p, PLAYER, k)? > 0.0,
        };
        if shown {
            combat.push(format!("{} {label}", lua_int(out(p, PLAYER, k)?)?));
        }
    }
    let main_base = p.lists[p.main()].base;
    let main_cfg = p.cfg(p.main());
    if p.c.flag(main_base, Some(&main_cfg), &["Cruelty"])? {
        combat.push("Cruelty".to_string());
    }
    for (flag, label) in COMBAT_FLAGS {
        if p.c.flag(pdb, None, &[flag])? {
            combat.push(label.to_string());
        }
    }
    let mut buffs: Vec<&str> = p
        .merged_buffs
        .0
        .iter()
        .map(|(k, _, _)| k.as_str())
        .collect();
    if p.c.flag(pdb, None, &["Elusive"])? {
        combat.push("Elusive".to_string());
    }
    buffs.sort_unstable();
    let buff_list = buffs.join(", ");
    let skill_buffs = evaluated(p, pdb, &p.merged_buffs.0, &buffs)?;
    let mut curses = Vec::new();
    let mut names: Vec<&(String, Vec<Mod>, bool)> = p.debuffs.iter().collect();
    names.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let edb = p.c.db(crate::calc::ENEMY);
    let mut skill_debuffs = Vec::new();
    for (_, list, _) in &names {
        mod_entries(p, edb, list, &mut skill_debuffs)?;
    }
    for (_, list) in &p.curse_slots {
        mod_entries(
            p,
            edb,
            list.as_deref().unwrap_or_default(),
            &mut skill_debuffs,
        )?;
    }
    for (name, list, _) in names {
        let stack = Sym::new(&format!("Multiplier:{name}Stack"));
        let count = list
            .iter()
            .filter(|m| m.name == stack && m.typ == ModType::Base)
            .try_fold(0.0, |n, m| m.value.as_number().map(|v| n + v))
            .ok_or("a debuff's stack count is not a number")?;
        curses.push(if count > 0.0 {
            let s = if count > 1.0 { "s" } else { "" };
            format!("{name} ({} stack{s})", crate::pat::num(count))
        } else {
            name.clone()
        });
    }
    curses.extend(p.curse_slots.iter().map(|(n, _)| n.clone()));
    let curse_list = curses.join(", ");
    let set = |p: &mut Perform, a: usize, k: &str, v: &str| {
        p.c.set_out_val(a, k, OutVal::Str(v.into()));
    };
    let mod_list = |l: Vec<ModEntry>| Breakdown {
        mod_list: Some(l),
        ..Breakdown::default()
    };
    if let Some(b) = p.bd(PLAYER) {
        b.set("SkillBuffs", mod_list(skill_buffs));
        b.set("SkillDebuffs", mod_list(skill_debuffs.clone()));
    }
    set(p, PLAYER, "BuffList", &buff_list);
    set(p, PLAYER, "CombatList", &combat.join(", "));
    set(p, PLAYER, "CurseList", &curse_list);
    if let Some(a) = p.minion_actor() {
        let mdb = p.c.db(a);
        let mut combat = Vec::new();
        for (k, label) in CHARGES {
            if matches!(*k, "PowerCharges" | "FrenzyCharges" | "EnduranceCharges") {
                let n = out(p, a, k)?;
                if n > 0.0 {
                    combat.push(format!("{} {label}", lua_int(n)?));
                }
            }
        }
        for (flag, label) in MINION_COMBAT_FLAGS {
            if p.c.flag(mdb, None, &[flag])? {
                combat.push(label.to_string());
            }
        }
        let mut buffs: Vec<&str> = p
            .merged_buffs
            .1
            .iter()
            .map(|(k, _, _)| k.as_str())
            .collect();
        buffs.sort_unstable();
        let buff_list = buffs.join(", ");
        let skill_buffs = evaluated(p, mdb, &p.merged_buffs.1, &buffs)?;
        if let Some(b) = p.bd(a) {
            b.set("SkillBuffs", mod_list(skill_buffs));
            b.set("SkillDebuffs", mod_list(skill_debuffs));
        }
        set(p, a, "BuffList", &buff_list);
        set(p, a, "CombatList", &combat.join(", "));
        set(p, a, "CurseList", &curse_list);
    }
    Ok(())
}

/// The mods of a list with a value other than 0 on the database
/// (`db:EvalMod(mod)`), for the breakdowns
fn mod_entries(p: &Perform, db: StoreId, list: &[Mod], out: &mut Vec<ModEntry>) -> Result<()> {
    if !p.has_bd(PLAYER) {
        return Ok(());
    }
    for m in list {
        if let Some(value) = p.c.query().eval_mod(db, m, None, None)?
            && value != ModValue::Number(0.0)
        {
            out.push(ModEntry {
                m: m.clone(),
                value,
            });
        }
    }
    Ok(())
}

/// The mods of the named buffs, in the names' order, with their values
fn evaluated(p: &Perform, db: StoreId, buffs: &BuffMap, names: &[&str]) -> Result<Vec<ModEntry>> {
    let mut out = Vec::new();
    for name in names {
        if let Some((_, list, _)) = buffs.iter().find(|(k, _, _)| k == name) {
            mod_entries(p, db, list, &mut out)?;
        }
    }
    Ok(out)
}

/// `s_format("%d", n)`
fn lua_int(n: f64) -> Result<i64> {
    if n.is_finite() {
        Ok(n as i64)
    } else {
        Err(format!(
            "bad argument to 'format' (number has no integer representation: {n})"
        ))
    }
}

/// The charges the Calcs tab lists, in its order
const CHARGES: &[(&str, &str)] = &[
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
    ("InspirationCharges", "Inspiration Charges"),
    ("GhostShrouds", "Ghost Shrouds"),
    ("CrabBarriers", "Crab Barriers"),
    ("BloodCharges", "Blood Charges"),
    ("SpiritCharges", "Spirit Charges"),
    ("SpiritInfusions", "Spirit Infusions"),
];

/// The player's flags the Calcs tab lists after Cruelty, in its order
const COMBAT_FLAGS: &[(&str, &str)] = &[
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
    ("BloodShrineOfRats", "Blood Shrine of Rats"),
    ("BloodShrineOfLocusts", "Blood Shrine of Locusts"),
    ("BloodShrineOfToads", "Blood Shrine of Toads"),
    ("BloodShrineOfCrows", "Blood Shrine of Crows"),
    ("BloodShrineOfBats", "Blood Shrine of Bats"),
];

/// The minion's flags the Calcs tab lists, in its order
const MINION_COMBAT_FLAGS: &[(&str, &str)] = &[
    ("Fortify", "Fortify"),
    ("Onslaught", "Onslaught"),
    ("UnholyMight", "Unholy Might"),
    ("ChaoticMight", "Chaotic Might"),
    ("Tailwind", "Tailwind"),
    ("DiamondShrine", "Diamond Shrine"),
    ("MassiveShrine", "Massive Shrine"),
];

/// The skills whose costs the pools cannot pay, each calculated as the
/// main skill unless cached
fn cost_warnings(p: &mut Perform, recalc: &Recalculator) -> Result<()> {
    let pdb = p.c.db(PLAYER);
    let main_name = p.skill_name(p.main()).to_string();
    let mut lists: BTreeMap<String, Vec<OutVal>> = BTreeMap::new();
    for i in 0..p.n_player {
        let uuid = skill_uuid(p.skill(i), &p.env.groups);
        if recalc.cached(&uuid).is_none() {
            recalc.calculate(&uuid, &[])?;
        }
        let Some(cached) = recalc.cached(&uuid) else {
            continue;
        };
        let name = p.skill_name(i).to_string();
        let protects = p.c.flag(pdb, None, &["EnergyShieldProtectsMana"])?;
        // `Flag` is nil, not false, without the flag
        if protects {
            p.c.set_out_val(PLAYER, "EnergyShieldProtectsMana", OutVal::Bool(true));
        } else {
            p.c.clear_out(PLAYER, "EnergyShieldProtectsMana");
        }
        let out = &p.c.actors.actors[PLAYER].output;
        let reserved = |k: &str| {
            cached
                .skill
                .mods
                .as_ref()
                .and_then(|m| m.skill_data.get(&format!("{k}ReservedPercent")))
                .and_then(crate::mods::ModValue::as_number)
        };
        let mut warn = Vec::new();
        for (pool, cost) in [
            ("LifeUnreserved", "LifeCost"),
            ("ManaUnreserved", "ManaCost"),
            ("Rage", "RageCost"),
            ("EnergyShield", "ESCost"),
        ] {
            let Some(c) = cached.out(cost) else { continue };
            let es = if protects && cost == "ManaCost" {
                num(out, "EnergyShield").unwrap_or(0.0)
            } else {
                0.0
            };
            let total = es + num(out, pool).unwrap_or(0.0);
            if total < c {
                let raw = pool.strip_suffix("Unreserved").unwrap_or(pool);
                let short = reserved(raw)
                    .is_none_or(|r| total + (num(out, raw).unwrap_or(0.0) * r / 100.0).ceil() < c);
                if short {
                    if main_name == name {
                        warn.push(format!("{cost}Warning"));
                    }
                    lists
                        .entry(format!("{cost}WarningList"))
                        .or_default()
                        .push(OutVal::Str(name.as_str().into()));
                }
            }
        }
        for (pool, cost) in [
            ("LifeUnreservedPercent", "LifePercentCost"),
            ("ManaUnreservedPercent", "ManaPercentCost"),
        ] {
            if let Some(c) = cached.out(cost)
                && num(out, pool).unwrap_or(0.0) < c
            {
                lists
                    .entry(format!("{cost}PercentCostWarningList"))
                    .or_default()
                    .push(OutVal::Str(name.as_str().into()));
            }
        }
        for k in warn {
            p.c.set_out_val(PLAYER, &k, OutVal::Bool(true));
        }
    }
    for (k, v) in lists {
        p.c.set_out_val(PLAYER, &k, OutVal::List(Rc::new(v)));
    }
    Ok(())
}
