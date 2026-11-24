//! The generators: the single moves `best` ranks and `improve` combines,
//! each a list of changes as `apply` takes them. The searches of step 9
//! combine the same moves.
//!
//! Generators that make few moves make them all; the tree's, which can
//! reach hundreds of paths, first rank them by node power (`engine::power`,
//! the misc calculator on all cores) and keep the best `candidates`. Every
//! move kept is then evaluated exactly; the answer says which were.

use crate::{Names, R, cmd};
use engine::calculator::MiscCalculator;
use engine::power::{PowerOptions, node_power};
use engine::tree::NodeType;
use serde_json::{Value as J, json};
use session::Dispatcher;
use session::typed::TypedData;

/// The generators, with what each makes
pub(crate) const GENERATORS: &[(&str, &str)] = &[
    (
        "tree",
        "allocates each node within `depth` points with its path; ranked by node power, the best `candidates` kept",
    ),
    (
        "tree_remove",
        "deallocates each allocated node with what depends on it; ranked by node power (least lost first), the best `candidates` kept",
    ),
    (
        "masteries",
        "each other effect of each allocated mastery, and each effect of a mastery next to an allocated node",
    ),
    (
        "supports",
        "each support gem the engine knows in place of each support of a group (`group`, default the main one; `gem` for one of its gems)",
    ),
    (
        "gem_levels",
        "each gem one level higher, and at 20 quality, one at a time",
    ),
    (
        "items_remove",
        "empties each slot of the active set that holds an item, one at a time: what each item is worth, the weakest (least lost) first",
    ),
    ("config", "each other bandit and pantheon choice"),
];

/// Not generated yet, with where they come
pub(crate) const NOT_YET: &str = "anoints, uniques, crafts and rares have no generator yet; evaluate_item evaluates a pasted item and its slot's mod weights";

/// A move: changes as `apply` takes them
#[derive(Clone, Debug)]
pub(crate) struct Move {
    pub changes: Vec<J>,
    /// The ranking's estimate of the move, where there is one
    pub estimate: Option<f64>,
}

/// What a generator made
#[derive(Clone, Debug, Default)]
pub(crate) struct Generated {
    pub moves: Vec<Move>,
    /// The moves it could make, before any ranking kept the best
    pub possible: usize,
    /// How the moves kept were chosen
    pub how: String,
}

impl Generated {
    /// Whether every move the generator could make was kept
    pub fn exhaustive(&self) -> bool {
        self.moves.len() == self.possible
    }
}

/// What a generator takes
#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub depth: f64,
    pub candidates: usize,
    pub group: Option<u64>,
    pub gem: Option<u64>,
    /// The power stat the tree is ranked by, and whether lower is wanted
    pub rank: engine::mod_weights::StatWeight,
}

/// The moves of a generator on the build the session holds
pub(crate) fn generate(
    kind: &str,
    data: &TypedData,
    names: &mut Names,
    s: &mut Dispatcher,
    o: &Options,
) -> R<Generated> {
    match kind {
        "tree" | "tree_remove" => tree(kind == "tree", data, s, o),
        "masteries" => masteries(s),
        "supports" => supports(names, s, o),
        "gem_levels" => gem_levels(s),
        "items_remove" => items_remove(s),
        "config" => config(s),
        k => Err(format!(
            "no generator {:?}; the generators: {}",
            crate::text::clean(k, 60, false),
            GENERATORS
                .iter()
                .map(|(g, _)| *g)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn tree(add: bool, data: &TypedData, s: &mut Dispatcher, o: &Options) -> R<Generated> {
    let build = s.typed_build().ok_or("no build is open")?;
    let calc_data = data.calc_data();
    let calc = MiscCalculator::new(build, &calc_data)?;
    let powers = node_power(
        &calc,
        &PowerOptions {
            stat: Some(o.rank.stat.clone()),
            // Allocated nodes are at distance 0
            max_depth: Some(if add { o.depth } else { 0.0 }),
            cluster_nodes: Vec::new(),
        },
    )?;
    let spec = &build.spec;
    let sign = if o.rank.weight < 0.0 { -1.0 } else { 1.0 };
    let mut ranked: Vec<(f64, u32)> = Vec::new();
    for (id, p) in &powers.nodes {
        let Some(&i) = spec.index.get(id) else {
            continue;
        };
        let (n, tn) = (&spec.nodes[i], spec.tree_node(i));
        if tn.ascendancy_name.is_some()
            || matches!(
                tn.kind,
                NodeType::ClassStart | NodeType::AscendClassStart | NodeType::Mastery
            )
            || n.alloc != !add
        {
            continue;
        }
        if add && p.distance.is_none_or(|d| d > o.depth) {
            continue;
        }
        let Some(power) = p.power.path_power.or(p.power.single_stat) else {
            continue;
        };
        ranked.push((sign * power, *id));
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let possible = ranked.len();
    let verb = if add { "allocate" } else { "deallocate" };
    let moves = ranked
        .into_iter()
        .take(o.candidates)
        .map(|(p, id)| Move {
            changes: vec![json!({ "verb": verb, "nodes": [format!("#{id}")] })],
            estimate: Some(sign * p),
        })
        .collect::<Vec<_>>();
    let what = if add {
        format!("{possible} nodes within {} points", o.depth)
    } else {
        format!("{possible} allocated nodes (starts and ascendancy nodes left out)")
    };
    let how = format!(
        "{what}, ranked by node power of {} (the misc calculator, an estimate); the best {} evaluated exactly",
        o.rank.stat.label,
        moves.len()
    );
    Ok(Generated {
        moves,
        possible,
        how,
    })
}

fn masteries(s: &mut Dispatcher) -> R<Generated> {
    let build = s.typed_build().ok_or("no build is open")?;
    let spec = &build.spec;
    let mut moves = Vec::new();
    for (i, n) in spec.nodes.iter().enumerate() {
        let tn = spec.tree_node(i);
        if tn.kind != NodeType::Mastery || !(n.alloc || n.path_dist == 1.0) {
            continue;
        }
        let current = spec.mastery_selections.get(n.id).copied();
        for &effect in &tn.mastery_effects {
            if Some(effect) == current || !spec.tree.mastery_effects.contains_key(&effect.to_bits())
            {
                continue;
            }
            // One mastery's effect is not another's
            let taken = spec
                .mastery_selections
                .iter()
                .any(|(id, e)| *e == effect && id != n.id);
            if taken {
                continue;
            }
            moves.push(Move {
                changes: vec![json!({
                    "verb": "set_mastery",
                    "node": format!("#{}", n.id),
                    "effect": effect as u64,
                })],
                estimate: None,
            });
        }
    }
    let possible = moves.len();
    Ok(Generated {
        moves,
        possible,
        how: "every effect of each allocated mastery and of each mastery next to an allocated node, all evaluated exactly".into(),
    })
}

fn supports(names: &mut Names, s: &mut Dispatcher, o: &Options) -> R<Generated> {
    let skills = cmd(s, json!({ "cmd": "skills" }))?;
    let g = o
        .group
        .or_else(|| skills["mainSocketGroup"].as_u64())
        .unwrap_or(1);
    let gems = skills["groups"]
        .get(g.saturating_sub(1) as usize)
        .and_then(|x| x["gems"].as_array())
        .ok_or_else(|| format!("no group {g}; `skills` lists them"))?
        .clone();
    let known = names.gems(s)?.clone();
    let list = cmd(s, json!({ "cmd": "gemList" }))?;
    let in_group: Vec<&str> = gems.iter().filter_map(|x| x["name"].as_str()).collect();
    let supports: Vec<String> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["support"] == true)
        .filter_map(|x| x["name"].as_str())
        .filter(|n| known.contains(*n) && !in_group.contains(n))
        .map(String::from)
        .collect();
    let mut moves = Vec::new();
    for (k, gem) in gems.iter().enumerate() {
        let k = k as u64 + 1;
        if gem["support"] != true || o.gem.is_some_and(|w| w != k) {
            continue;
        }
        for name in &supports {
            moves.push(Move {
                changes: vec![json!({ "verb": "set_gem", "group": g, "gem": k, "name": name })],
                estimate: None,
            });
        }
    }
    if moves.is_empty() {
        return Err(match o.gem {
            Some(k) => format!("gem {k} of group {g} is no support gem"),
            None => format!("group {g} has no support gems"),
        });
    }
    let possible = moves.len();
    moves.truncate(o.candidates);
    let how = if moves.len() == possible {
        format!(
            "each of {} supports in place of each support of group {g}, all evaluated exactly",
            supports.len()
        )
    } else {
        format!(
            "each of {} supports in place of each support of group {g}: {possible} moves, the first {} (by slot, then name) evaluated; raise `candidates` or name a `gem` for the rest",
            supports.len(),
            moves.len()
        )
    };
    Ok(Generated {
        moves,
        possible,
        how,
    })
}

fn gem_levels(s: &mut Dispatcher) -> R<Generated> {
    let skills = cmd(s, json!({ "cmd": "skills" }))?;
    let mut moves = Vec::new();
    for (g, group) in skills["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if group["enabled"] == false {
            continue;
        }
        for (k, gem) in group["gems"].as_array().into_iter().flatten().enumerate() {
            if gem["enabled"] == false {
                continue;
            }
            let set = |field: &str, v: u64| Move {
                changes: vec![json!({ "verb": "set_gem", "group": g + 1, "gem": k + 1, field: v })],
                estimate: None,
            };
            if let Some(l) = gem["level"].as_u64()
                && l < 21
            {
                moves.push(set("level", l + 1));
            }
            if let Some(q) = gem["quality"].as_u64()
                && q < 20
            {
                moves.push(set("quality", 20));
            }
        }
    }
    let possible = moves.len();
    Ok(Generated {
        moves,
        possible,
        how: "each enabled gem one level higher (up to 21) and at 20 quality, one at a time, all evaluated exactly".into(),
    })
}

fn items_remove(s: &mut Dispatcher) -> R<Generated> {
    let items = cmd(s, json!({ "cmd": "items" }))?;
    let moves: Vec<Move> = items["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["shown"] == true && x["item"].is_number())
        .filter_map(|x| x["name"].as_str())
        .map(|slot| Move {
            changes: vec![json!({ "verb": "equip", "slot": slot, "item": null })],
            estimate: None,
        })
        .collect();
    if moves.is_empty() {
        return Err("no item is equipped".into());
    }
    let possible = moves.len();
    Ok(Generated {
        moves,
        possible,
        how: "each equipped slot of the active item set emptied, one at a time, all evaluated exactly: a row's change is what that item adds, so the first rows are the weakest items, the upgrades to look at first; evaluate_item gives a slot's mod weights and checks a replacement".into(),
    })
}

fn config(s: &mut Dispatcher) -> R<Generated> {
    let config = cmd(s, json!({ "cmd": "config" }))?;
    let mut moves = Vec::new();
    for v in config["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|sec| sec["vars"].as_array().into_iter().flatten())
    {
        let var = v["var"].as_str().unwrap_or_default();
        if !["bandit", "pantheonMajorGod", "pantheonMinorGod"].contains(&var) {
            continue;
        }
        let sel = v["sel"].as_u64().unwrap_or(1) as usize;
        for (i, c) in v["list"].as_array().into_iter().flatten().enumerate() {
            if i + 1 == sel {
                continue;
            }
            moves.push(Move {
                changes: vec![json!({ "verb": "set_config", "option": var, "value": crate::text::plain(c.as_str().unwrap_or_default()) })],
                estimate: None,
            });
        }
    }
    let possible = moves.len();
    Ok(Generated {
        moves,
        possible,
        how: "each other bandit, major god and minor god choice, all evaluated exactly".into(),
    })
}
