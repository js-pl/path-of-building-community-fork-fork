//! `Modules/Calcs.lua`: `calcs.buildOutput` of the main calculation
//! (`mode == "MAIN"`) with `calcs.calcFullDPS`, from a loaded build.
//!
//! The Full DPS skills are calculated each in a calculation of its own
//! from the setup, where upstream reuses the first calculation's setup.
//!
//! Not ported yet: the Calcs tab's calculation (`mode == "CALCS"`), the
//! conditions, multipliers and mods in use that the Configuration tab
//! reads (`env.conditionsUsed`, ...), the node and misc calculators.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::actor::OutVal;
use crate::build::Build;
use crate::calc::{PLAYER, m_min};
use crate::calc_cache::{Recalc, Recalculator, skill_uuid};
use crate::calc_env::{CalcData, init_env};
use crate::calc_perform::{Perform, PerformInput, Result, perform};
use crate::calc_setup::BuffMode;
use crate::calc_skills::ActiveSkill;
use crate::mods::{Cfg, ModType, Sym};
use crate::skills::SocketGroup;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// An actor's output
pub type Output = HashMap<Sym, OutVal>;

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

/// What `calcs.buildOutput(build, "MAIN")` leaves: the player's output,
/// with the Full DPS results, and the main skill's minion's
#[derive(Clone, Debug)]
pub struct BuildOutput {
    pub player: Output,
    pub minion: Option<Output>,
    pub full_dps: FullDps,
}

const MODE: BuffMode = BuffMode {
    buffs: true,
    combat: true,
    effective: true,
};

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

/// `calcs.calcFullDPS(build, "CALCULATOR")`
pub fn calc_full_dps(build: &Build, data: &CalcData) -> Result<FullDps> {
    let recalc = Recalculator::calculator(build, data, MODE);
    let mut first = Some(init_env(&mut build.clone(), data, MODE)?);
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
            None => init_env(&mut b, data, MODE)?,
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
    Ok(full)
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

/// `calcs.buildOutput(build, "MAIN")`. The setup adds the granted skills'
/// socket groups to the build, as upstream's does. The Full DPS skills are
/// sorted by their DPS, as the sidebar sorts upstream's in place
/// (`buildMode:AddDisplayStatList`).
pub fn build_output(build: &mut Build, data: &CalcData) -> Result<BuildOutput> {
    let env = init_env(build, data, MODE)?;
    let build = &*build;
    let recalc = Recalculator::new(build, data, MODE);
    let mut input = PerformInput::new(build, data, false)?;
    input.recalc = Some(&recalc);
    let mut p = perform(env, input)?;
    p.finish()?;
    let mut full_dps = calc_full_dps(build, data)?;
    lua_sort(&mut full_dps.skills, |a, b| {
        a.dps * a.count > b.dps * b.count
    });
    let skills = full_dps.skills.iter().map(FullDpsSkill::out_val).collect();
    p.c.set_out_val(PLAYER, "SkillDPS", OutVal::List(Rc::new(skills)));
    p.c.set_out(PLAYER, "FullDPS", full_dps.combined_dps);
    p.c.set_out(PLAYER, "FullDotDPS", full_dps.total_dot_dps);
    cost_warnings(&mut p, &recalc)?;
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
    let minion = p
        .minion_actor()
        .map(|a| p.c.actors.actors[a].output.clone());
    Ok(BuildOutput {
        player: std::mem::take(&mut p.c.actors.actors[PLAYER].output),
        minion,
        full_dps,
    })
}

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
