//! Node power: `CalcsTab:PowerBuilder` (`Classes/CalcsTab.lua`), the
//! power of each passive node by the calculator's results with it
//! allocated or removed, for a power stat of `data.powerStatList`
//! (`Modules/Data.lua`) or the combined offence and defence; the
//! unallocated masteries by each of their effects, and the cluster
//! notables for the power report.
//!
//! Candidates with equal mods share one calculation (`node.modKey`), as
//! upstream's do.
//!
//! In sync with upstream `16de4b82d57f1c0de6eb40f37143c32d4da36a02`.

use crate::calc::m_max;
use crate::calc_override::{Override, OverrideNode, mastery_effect_node, tree_node};
use crate::calc_perform::Result;
use crate::calculator::{CalcOutput, MiscCalculator};
use crate::mods::{Mod, ModValue, Sym};
use crate::spec::PassiveSpec;
use crate::tree::{NodeType, PassiveTree, StatLine};
use lua::ljtab::LjMap;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// A stat of `data.powerStatList`
#[derive(Clone, Debug, PartialEq)]
pub struct PowerStat {
    pub stat: Option<String>,
    pub label: String,
    /// `transform`: lower is better
    pub negate: bool,
    pub combined_off_def: bool,
    pub ignore_for_nodes: bool,
    pub ignore_for_items: bool,
}

const STATS: &[(&str, &str)] = &[
    ("FullDPS", "Full DPS"),
    ("CombinedDPS", "Combined DPS"),
    ("TotalDPS", "Hit DPS"),
    ("WithImpaleDPS", "Impale + Hit DPS"),
    ("AverageDamage", "Average Hit"),
    ("Speed", "Attack/Cast Speed"),
    ("TotalDot", "DoT DPS"),
    ("BleedDPS", "Bleed DPS"),
    ("IgniteDPS", "Ignite DPS"),
    ("PoisonDPS", "Poison DPS"),
    ("Life", "Life"),
    ("LifeRegen", "Life regen"),
    ("LifeLeechRate", "Life leech"),
    ("Armour", "Armour"),
    ("Evasion", "Evasion"),
    ("EnergyShield", "Energy Shield"),
    ("EnergyShieldRecoveryCap", "Recoverable ES"),
    ("EnergyShieldRegen", "Energy Shield regen"),
    ("EnergyShieldLeechRate", "Energy Shield leech"),
    ("Mana", "Mana"),
    ("ManaRegen", "Mana regen"),
    ("ManaLeechRate", "Mana leech"),
    ("Ward", "Ward"),
    ("Str", "Strength"),
    ("Dex", "Dexterity"),
    ("Int", "Intelligence"),
    ("TotalAttr", "Total Attributes"),
    ("MeleeAvoidChance", "Melee avoid chance"),
    ("SpellAvoidChance", "Spell avoid chance"),
    ("ProjectileAvoidChance", "Projectile avoid chance"),
    ("TotalEHP", "Effective Hit Pool"),
    ("SecondMinimalMaximumHitTaken", "Eff. Maximum Hit Taken"),
    ("PhysicalTakenHit", "Taken Phys dmg"),
    ("LightningTakenHit", "Taken Lightning dmg"),
    ("ColdTakenHit", "Taken Cold dmg"),
    ("FireTakenHit", "Taken Fire dmg"),
    ("ChaosTakenHit", "Taken Chaos dmg"),
    ("CritChance", "Crit Chance"),
    ("CritMultiplier", "Crit Multiplier"),
    ("BleedChance", "Bleed Chance"),
    ("FreezeChance", "Freeze Chance"),
    ("IgniteChance", "Ignite Chance"),
    ("ShockChance", "Shock Chance"),
    ("EffectiveMovementSpeedMod", "Move speed"),
    ("LightRadiusMod", "Light Radius"),
    ("BlockChance", "Block Chance"),
    ("SpellBlockChance", "Spell Block Chance"),
    ("SpellSuppressionChance", "Spell Suppression Chance"),
];

/// Stats the minions' copies of the list leave out
const NOT_FOR_MINIONS: &[&str] = &[
    "AverageDamage",
    "TotalDot",
    "Str",
    "Dex",
    "Int",
    "Spirit",
    "EffectiveLootRarityMod",
    "LightRadiusMod",
];

/// `data.powerStatList`, with the minions' stats after the player's
pub fn power_stats() -> Vec<PowerStat> {
    let stat = |s: &str, label: &str| PowerStat {
        stat: Some(s.into()),
        label: label.into(),
        negate: s.ends_with("TakenHit"),
        combined_off_def: false,
        ignore_for_nodes: false,
        ignore_for_items: false,
    };
    let mut list = vec![
        PowerStat {
            stat: None,
            label: "Offence/Defence".into(),
            negate: false,
            combined_off_def: true,
            ignore_for_nodes: false,
            ignore_for_items: true,
        },
        PowerStat {
            stat: None,
            label: "Name".into(),
            negate: false,
            combined_off_def: false,
            ignore_for_nodes: true,
            ignore_for_items: false,
        },
    ];
    list.extend(STATS.iter().map(|(s, l)| stat(s, l)));
    let minions: Vec<PowerStat> = list
        .iter()
        .filter_map(|p| {
            let s = p.stat.as_deref()?;
            if s.contains("DPS") || NOT_FOR_MINIONS.contains(&s) {
                return None;
            }
            Some(PowerStat {
                stat: Some(format!("Minion{s}")),
                label: format!("Minion {}", p.label),
                ..p.clone()
            })
        })
        .collect();
    list.extend(minions);
    list
}

impl PowerStat {
    /// `data.powerStatList.GetFromOutput(output, stat)`
    pub fn value(&self, out: &CalcOutput) -> f64 {
        let Some(stat) = self.stat.as_deref() else {
            return 0.0;
        };
        let or0 = |v: Option<f64>| v.unwrap_or(0.0);
        let v = if stat == "FullDPS" {
            match out.num(stat) {
                Some(v) => v,
                None => or0(out.num("CombinedDPS")) + or0(out.minion_num("CombinedDPS")),
            }
        } else if let Some(m) = stat.strip_prefix("Minion").filter(|m| !m.is_empty()) {
            or0(out.minion_num(m))
        } else if stat == "AverageDamage" || stat == "TotalDot" || stat.contains("DPS") {
            or0(out.num(stat)) + or0(out.minion_num(stat))
        } else {
            or0(out.num(stat))
        };
        if self.negate { -v } else { v }
    }

    /// `CalcsTab:CalculatePowerStat(stat, original, modified)`
    pub fn power(&self, original: &CalcOutput, modified: &CalcOutput) -> f64 {
        self.value(original) - self.value(modified)
    }
}

fn need(out: &CalcOutput, k: &str) -> Result<f64> {
    out.num(k)
        .ok_or_else(|| format!("attempt to perform arithmetic on a nil value (field '{k}')"))
}

/// `CalcsTab:CalculateCombinedOffDefStat(original, modified)`: the offence
/// and the defence
pub fn combined_off_def(original: &CalcOutput, modified: &CalcOutput) -> Result<(f64, f64)> {
    let (o, m) = (original, modified);
    let es = |out: &CalcOutput| match out.num("EnergyShieldRecoveryCap") {
        Some(v) => Ok(v),
        None => need(out, "EnergyShield"),
    };
    let defence = (need(o, "LifeUnreserved")? - need(m, "LifeUnreserved")?)
        / m_max(3000.0, need(m, "Life")?)
        + (need(o, "Armour")? - need(m, "Armour")?) / m_max(10000.0, need(m, "Armour")?)
        + (es(o)? - es(m)?) / m_max(3000.0, es(m)?)
        + (need(o, "Evasion")? - need(m, "Evasion")?) / m_max(10000.0, need(m, "Evasion")?)
        + (need(o, "LifeRegenRecovery")? - need(m, "LifeRegenRecovery")?) / 500.0
        + (need(o, "EnergyShieldRegenRecovery")? - need(m, "EnergyShieldRegenRecovery")?) / 1000.0;
    let minion = |out: &CalcOutput| out.minion_num("CombinedDPS").unwrap_or(0.0);
    let modified_dps = need(m, "CombinedDPS")? + minion(m);
    let incr = need(o, "CombinedDPS")? + minion(o) - modified_dps;
    Ok((incr / modified_dps, defence))
}

/// A node's power (`node.power`), or a mastery effect's
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Power {
    pub single_stat: Option<f64>,
    pub path_power: Option<f64>,
    pub offence: Option<f64>,
    pub defence: Option<f64>,
}

/// A node of the spec's power
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodePower {
    pub power: Power,
    /// The points to allocate the node
    pub distance: Option<f64>,
    /// A mastery's power by effect id, in the order calculated
    pub mastery_effects: Option<Vec<(f64, Power)>>,
}

/// The most power of the nodes on the tree (`powerMax`)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerMax {
    pub single_stat: f64,
    pub offence: f64,
    pub offence_per_point: f64,
    pub defence: f64,
    pub defence_per_point: f64,
}

/// What node power calculates
#[derive(Clone, Debug, Default)]
pub struct PowerOptions {
    /// The power stat; `None` for the combined offence and defence
    pub stat: Option<PowerStat>,
    /// `nodePowerMaxDepth`: no nodes further than this many points
    pub max_depth: Option<f64>,
    /// The cluster notables to calculate, by tree node id: [`cluster_nodes`]
    /// for the power report, none for the tree's heat map
    pub cluster_nodes: Vec<u32>,
}

/// The power of every node
#[derive(Clone, Debug, Default)]
pub struct NodePowers {
    /// Each node of the spec's power, by id, in the spec's order
    pub nodes: Vec<(u32, NodePower)>,
    /// The cluster notables' power, by tree node id
    pub cluster: Vec<(u32, Power)>,
    pub max: PowerMax,
}

/// The tree's cluster notables and keystones (`clusterNodeMap`), in the
/// order of their names' first nodes
pub fn cluster_nodes(tree: &PassiveTree) -> Vec<u32> {
    let mut seen = std::collections::HashSet::new();
    tree.nodes
        .iter()
        .filter(|n| tree.cluster_node_map.contains_key(&n.dn) && seen.insert(n.dn.clone()))
        .map(|n| tree.cluster_node_map[&n.dn])
        .collect()
}

/// A mod as `modLib.formatMod` formats it for `node.modKey`: everything but
/// its source
fn mod_key_part(m: &Mod, out: &mut String) {
    let mut m = m.clone();
    m.source = None;
    if let ModValue::Record(r) = &mut m.value
        && let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod"))
    {
        inner.source = None;
    }
    out.push('[');
    out.push_str(&format!("{m:?}"));
    out.push(']');
}

/// `node.modKey`: the mods of the lines the parser fully understood
fn mod_key(lines: &[StatLine]) -> String {
    let mut k = String::new();
    for l in lines.iter().filter(|l| l.extra.is_none()) {
        for m in l.list.iter().flatten() {
            mod_key_part(m, &mut k);
        }
    }
    k
}

fn spec_mod_key(spec: &PassiveSpec, i: usize) -> String {
    match &spec.nodes[i].stats {
        Some(s) => mod_key(&s.mods),
        None => mod_key(&spec.tree_node(i).mods),
    }
}

/// The calculator's results by candidate, calculated once per `modKey`
struct Candidates<'c, 'a, 't> {
    calc: &'c MiscCalculator<'a, 't>,
    full_dps: bool,
    cache: HashMap<String, Rc<CalcOutput>>,
}

impl Candidates<'_, '_, '_> {
    fn get(&mut self, key: String, ovr: impl FnOnce() -> Override) -> Result<Rc<CalcOutput>> {
        if let Some(out) = self.cache.get(&key) {
            return Ok(out.clone());
        }
        let out = Rc::new(self.calc.calculate(&ovr(), self.full_dps)?);
        self.cache.insert(key, out.clone());
        Ok(out)
    }

    fn uncached(&self, ovr: &Override) -> Result<CalcOutput> {
        self.calc.calculate(ovr, self.full_dps)
    }
}

/// How the power is measured
#[derive(Clone, Copy)]
enum Mode<'s> {
    Stat(&'s PowerStat),
    Combined,
    None,
}

/// `CalcsTab:PowerBuilder()` on a calculator of the build
pub fn node_power(calc: &MiscCalculator, opts: &PowerOptions) -> Result<NodePowers> {
    let spec = &calc.build().spec;
    check_tree_jewels(calc)?;
    let base = calc.base();
    let granted = calc.granted_passives();
    let mode = match &opts.stat {
        Some(s) if s.stat.is_some() && !s.ignore_for_nodes => Mode::Stat(s),
        Some(s) if s.ignore_for_nodes => Mode::None,
        _ => Mode::Combined,
    };
    let full_dps = opts
        .stat
        .as_ref()
        .is_some_and(|s| s.stat.as_deref() == Some("FullDPS"));
    let mut cands = Candidates {
        calc,
        full_dps,
        cache: HashMap::new(),
    };
    let mut max = PowerMax::default();
    let mut powers: Vec<NodePower> = vec![NodePower::default(); spec.nodes.len()];
    // `distanceMap`: the nodes by distance, each a table by node id
    let mut by_distance: BTreeMap<u64, LjMap<usize>> = BTreeMap::new();
    let mut masteries = Vec::new();
    let keys: Vec<String> = (0..spec.nodes.len())
        .map(|i| spec_mod_key(spec, i))
        .collect();
    // `pairs(spec.nodes)`
    for (id, &i) in spec.order.iter() {
        let n = &spec.nodes[i];
        let tn = spec.tree_node(i);
        if tn.kind == NodeType::Mastery {
            powers[i].mastery_effects = Some(vec![]);
        }
        if !keys[i].is_empty() && !granted.contains(&id) {
            if tn.kind == NodeType::Mastery && n.all_mastery_options == Some(true) {
                if opts.max_depth.is_none_or(|d| d >= n.path_dist) {
                    masteries.push(i);
                }
            } else {
                by_distance
                    .entry(n.path_dist.to_bits())
                    .or_default()
                    .insert(id, i);
                powers[i].distance = Some(n.path_dist);
            }
        }
    }
    let mut dists: Vec<(f64, Vec<usize>)> = by_distance
        .into_iter()
        .map(|(d, v)| (f64::from_bits(d), v.iter().map(|(_, &i)| i).collect()))
        .collect();
    dists.sort_by(|a, b| a.0.total_cmp(&b.0));

    // What a node's allocation adds
    let add_power = |cands: &mut Candidates,
                     max: &mut PowerMax,
                     power: &mut Power,
                     distance: f64,
                     on_path: bool,
                     out: &CalcOutput,
                     path: &dyn Fn() -> Override|
     -> Result<()> {
        match mode {
            Mode::Stat(stat) => {
                let single = stat.power(out, base);
                power.single_stat = Some(single);
                if on_path {
                    max.single_stat = m_max(max.single_stat, single);
                    power.path_power = Some(single);
                    if distance > 1.0 {
                        power.path_power = Some(stat.power(&cands.uncached(&path())?, base));
                    }
                }
            }
            Mode::Combined => {
                let (offence, defence) = combined_off_def(out, base)?;
                power.offence = Some(offence);
                power.defence = Some(defence);
                power.single_stat = Some(offence);
                if on_path {
                    max.offence = m_max(max.offence, offence);
                    max.defence = m_max(max.defence, defence);
                    max.offence_per_point = m_max(max.offence_per_point, offence / distance);
                    max.defence_per_point = m_max(max.defence_per_point, defence / distance);
                }
            }
            Mode::None => {}
        }
        Ok(())
    };
    let on_path =
        |i: usize| spec.nodes[i].path.is_some() && spec.tree_node(i).ascendancy_name.is_none();
    let path_nodes = |i: usize| {
        let mut v = Vec::new();
        let mut step = spec.nodes[i].path.clone().flatten();
        while let Some(s) = step {
            v.push(s.node);
            step = s.rest.clone();
        }
        v
    };

    for (distance, nodes) in &dists {
        if opts.max_depth.is_some_and(|d| d < *distance) {
            break;
        }
        for &i in nodes {
            let n = &spec.nodes[i];
            let key = keys[i].clone();
            if !n.alloc {
                let out = cands.get(key, || Override::default().add_node(i))?;
                let path = || Override {
                    add_nodes: path_nodes(i).into_iter().map(OverrideNode::Spec).collect(),
                    ..Override::default()
                };
                let mut power = std::mem::take(&mut powers[i].power);
                add_power(
                    &mut cands,
                    &mut max,
                    &mut power,
                    *distance,
                    on_path(i),
                    &out,
                    &path,
                )?;
                powers[i].power = power;
            } else {
                let out = cands.get(key + "_remove", || Override::default().remove_node(i))?;
                if let Mode::Stat(stat) = mode {
                    let single = stat.power(&out, base);
                    let p = &mut powers[i].power;
                    p.single_stat = Some(single);
                    if spec.tree_node(i).ascendancy_name.is_none() {
                        p.path_power = Some(single);
                        if n.depends.len() > 1 {
                            let ovr = Override {
                                remove_nodes: n.depends.clone(),
                                ..Override::default()
                            };
                            p.path_power = Some(stat.power(&cands.uncached(&ovr)?, base));
                        }
                    }
                }
            }
            if spec.tree_node(i).kind == NodeType::Mastery
                && let Some(&effect) = spec.mastery_selections.get(n.id)
            {
                let p = powers[i].power.clone();
                powers[i]
                    .mastery_effects
                    .get_or_insert_with(Vec::new)
                    .push((effect, p));
            }
        }
    }

    for &i in &masteries {
        let n = &spec.nodes[i];
        for &effect in &spec.tree_node(i).mastery_effects {
            let assignable = spec
                .mastery_selections
                .iter()
                .find(|(_, e)| **e == effect)
                .is_none_or(|(id, _)| id == n.id);
            if !assignable || !spec.tree.mastery_effects.contains_key(&effect.to_bits()) {
                continue;
            }
            let node = Rc::new(mastery_effect_node(spec, i, effect)?);
            let key = match &node.stats {
                Some(s) => mod_key(&s.mods),
                None => String::new(),
            };
            if key.is_empty() {
                continue;
            }
            let own = || OverrideNode::Own(node.clone());
            let out = cands.get(key, || Override {
                add_nodes: vec![own()],
                ..Override::default()
            })?;
            let path = || Override {
                add_nodes: std::iter::once(own())
                    .chain(
                        path_nodes(i)
                            .into_iter()
                            .filter(|&p| p != i)
                            .map(OverrideNode::Spec),
                    )
                    .collect(),
                ..Override::default()
            };
            let mut ep = Power::default();
            add_power(
                &mut cands,
                &mut max,
                &mut ep,
                n.path_dist,
                on_path(i),
                &out,
                &path,
            )?;
            let p = &mut powers[i].power;
            let at_least = |v: Option<f64>, e: Option<f64>| -> Result<Option<f64>> {
                let e = e.ok_or("attempt to compare nil with number")?;
                Ok(Some(m_max(v.unwrap_or(0.0), e)))
            };
            match mode {
                Mode::Stat(_) => {
                    ep.path_power = ep.path_power.or(ep.single_stat);
                    p.single_stat = at_least(p.single_stat, ep.single_stat)?;
                    p.path_power = at_least(p.path_power, ep.path_power)?;
                }
                Mode::Combined => {
                    p.offence = at_least(p.offence, ep.offence)?;
                    p.defence = at_least(p.defence, ep.defence)?;
                    p.single_stat = at_least(p.single_stat, ep.single_stat)?;
                }
                Mode::None => {}
            }
            let effects = powers[i].mastery_effects.get_or_insert_with(Vec::new);
            match effects.iter_mut().find(|(e, _)| *e == effect) {
                Some((_, p)) => *p = ep,
                None => effects.push((effect, ep)),
            }
        }
    }

    let mut cluster = Vec::new();
    for &id in &opts.cluster_nodes {
        let Some(tn) = spec.tree.node(id) else {
            continue;
        };
        let key = mod_key(&tn.mods);
        let mut p = Power::default();
        if !key.is_empty() && !granted.contains(&id) {
            let out = cands.get(key, || Override {
                add_nodes: tree_node(spec, id)
                    .map(|n| OverrideNode::Own(Rc::new(n)))
                    .into_iter()
                    .collect(),
                ..Override::default()
            })?;
            if let Mode::Stat(stat) = mode {
                p.single_stat = Some(stat.power(&out, base));
            }
        }
        cluster.push((id, p));
    }

    Ok(NodePowers {
        nodes: spec
            .nodes
            .iter()
            .zip(powers)
            .map(|(n, p)| (n.id, p))
            .collect(),
        cluster,
        max,
    })
}

/// Jewels that change the tree's paths (Intuitive Leap and the like) are
/// not ported in the spec
fn check_tree_jewels(calc: &MiscCalculator) -> Result<()> {
    let b = calc.build();
    for (&socket, &id) in &b.spec.jewels {
        if !b.spec.alloc_nodes.contains(socket) {
            continue;
        }
        let Some(item) = b.items.item(id) else {
            continue;
        };
        let changes = item.jewel_data.as_ref().is_some_and(|j| {
            j.get("intuitiveLeapLike")
                .is_some_and(|v| !matches!(v, ModValue::Bool(false)))
                || matches!(
                    j.get("impossibleEscapeKeystones"),
                    Some(ModValue::Record(r)) if !r.fields.is_empty()
                )
        });
        if changes {
            return Err("not ported: node power with Intuitive Leap and the like".into());
        }
    }
    Ok(())
}
