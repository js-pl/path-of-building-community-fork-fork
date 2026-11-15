//! Find a Timeless Jewel's Search (`searchSingleSocket`, `formatResults`
//! and the Search button of `TreeTab:FindTimelessJewel`): the desired and
//! fallback lists read as weights, each seed of the jewel type scored on
//! the socket's notables from the jewel's look-up table, and the results
//! listed by total weight. Upstream keeps the radius' nodes and the
//! results in tables keyed by id and seed and lists them in `pairs` order
//! before sorting; [`LjMap`] keeps LuaJIT's, so ties come out the same.
//! The Abyss jewels' search is not ported yet.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::timeless_search::FindTimeless;
use super::{NOT_PORTED, Result, TypedSession, UPSTREAM_ERROR};
use engine::breakdown::{Arg, lua_format};
use engine::pat::{gsub, to_number};
use engine::timeless::ADDITIONS;
use engine::tree::NodeType;
use lua::ljtab::LjMap;

/// `data.timelessJewelSeedMin` and `Max`, by type from 1
const SEED_MIN: [f64; 11] = [
    100.0, 10000.0, 500.0, 2000.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0,
];
const SEED_MAX: [f64; 11] = [
    8000.0, 18000.0, 8000.0, 10000.0, 8000.0, 8000.0, 8000.0, 8000.0, 8000.0, 8000.0, 8000.0,
];

/// `reverseTotalModIDs`
const TOTAL_IDS: [&str; 9] = [
    "karui_notable_add_strength",
    "karui_attribute_strength",
    "karui_small_strength",
    "maraketh_notable_add_dexterity",
    "maraketh_attribute_dex",
    "maraketh_small_dex",
    "templar_notable_devotion",
    "templar_devotion_node",
    "templar_small_devotion",
];

/// A `desiredNodes` entry
#[derive(Clone, Debug)]
pub(super) struct Desired {
    pub id: String,
    pub weight: f64,
    pub weight2: f64,
    pub display: String,
    pub idx: usize,
}

/// A seed's matches for one desired node: the nodes it changes and their
/// names, and its weight
#[derive(Clone, Debug, Default)]
pub(super) struct Matched {
    pub names: Vec<String>,
    pub total: f64,
}

/// A `timelessData.searchResults` row
#[derive(Clone, Debug)]
pub(super) struct SeedResult {
    pub label: String,
    pub seed: f64,
    pub total: f64,
    /// The socket's id and label, searching All Sockets
    pub socket: Option<(f64, String)>,
    /// `result[legionId].targetNodeNames`, by desired node id
    pub matched: Vec<(String, Vec<String>)>,
    /// Militant Faith's `altVariant` and `altVariant2`, once its item was
    /// made
    pub alt: Option<(usize, usize)>,
}

/// `timelessData.sharedResults`
#[derive(Clone, Debug, Default)]
pub(super) struct Shared {
    pub jewel: usize,
    pub conqueror: usize,
    /// `devotionVariant1` and `2`, as indices of the list
    pub devotion: [usize; 2],
    /// The socket searched: id, label and keystone; `None` for All
    /// Sockets
    pub socket: Option<(f64, String, String)>,
    pub desired: Vec<Desired>,
}

/// A row's value: a number once the search wrote one
#[derive(Clone, Debug, PartialEq)]
enum Cell {
    S(String),
    N(f64),
}

impl Cell {
    fn number(&self) -> Option<f64> {
        match self {
            Cell::S(s) => to_number(s),
            Cell::N(n) => Some(*n),
        }
    }
}

/// A seed's weight, its matches by desired node and whether it is dropped
type Scored = (f64, Vec<(String, Matched)>, bool);

/// `searchSingleSocket`'s outcome
struct SocketSearch {
    /// Seeds left, with their matches by desired node id, in `pairs` order
    results: LjMap<Vec<(String, Matched)>>,
    weights: LjMap<f64>,
    desired: Vec<Desired>,
}

/// `formatSearchValue(input)`
fn format_value(v: f64) -> String {
    let mut s = format!(" {}", lua_format("%006.2f", &[Arg::Num(v)]));
    s = gsub(&s, " 0", "   ");
    s = gsub(&s, " 0", "   ");
    s = gsub(&s, ".0 ", "    ");
    s = gsub(&s, ".0 ", "    ");
    s = gsub(&s, "  %.", "0.");
    gsub(&s, "%.([0-9])0", ".%1  ")
}

fn unported(what: &str) -> String {
    format!("{NOT_PORTED}: {what} in a timeless jewel search")
}

impl<'d> TypedSession<'d> {
    /// `searchSingleSocket(socketId)`
    fn search_socket(&self, p: &FindTimeless, socket: u32) -> Result<Option<SocketSearch>> {
        let spec = &self.build.spec;
        let tree = spec.tree;
        let jewel = p.jewel();
        let abyss = jewel >= 7;
        let Some(socket_node) = tree.node(socket).filter(|n| n.kind == NodeType::Socket) else {
            return Ok(None);
        };
        if socket_node.nodes_in_radius.is_none() {
            return Ok(None);
        }
        // Reclaimed Malevolence changes the socket's path to the class
        // start and an ascendancy notable
        let zorath = if jewel == 11 {
            let path = spec
                .index
                .get(&socket)
                .and_then(|&i| spec.shortest_path_to_class_start(i));
            if path.is_none() {
                return Ok(None);
            }
            path
        } else {
            None
        };
        let ascendancy = spec.ascend_class_name();
        let t = self
            .data
            .timeless()
            .ok_or_else(|| format!("{NOT_PORTED}: timeless jewels without their data"))?;
        let td = &self.tree_tab.timeless;
        let total_mod = super::timeless_search::total_mod(jewel);
        // The desired list, then the fallback rows it does not have
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        let tbl = p.tables();
        for r in &tbl[0] {
            rows.push(r.iter().map(|c| Cell::S(c.clone())).collect());
        }
        for r in &tbl[1] {
            if !tbl[0].iter().any(|d| d.first() == r.first()) {
                rows.push(r.iter().map(|c| Cell::S(c.clone())).collect());
            }
        }
        let combined = rows.len();
        let mut desired: Vec<Desired> = Vec::new();
        let mut minimum: Vec<(String, f64)> = Vec::new();
        let mut desired_idx = 0;
        let allocated_in_radius = self.tree_tab.allocated_in_radius.get();
        for mut row in rows {
            if row.len() <= 1 {
                continue;
            }
            let Cell::S(first) = row[0].clone() else {
                continue;
            };
            let mut id = first.clone();
            let mut display = None;
            let mut single = false;
            if let Some(total) = total_mod
                && first == format!("total_{}", total.to_lowercase())
            {
                id = "totalStat".into();
                row[0] = Cell::S(id.clone());
                display = Some(total.to_string());
            }
            if display.is_none()
                && let Some(l) = t.nodes.iter().find(|l| *l.node.id == *first)
            {
                single = jewel > 1 && jewel < 7;
                display = Some(
                    l.node
                        .sd
                        .iter()
                        .map(|s| &**s)
                        .collect::<Vec<_>>()
                        .join(" + "),
                );
            }
            if display.is_none()
                && let Some(l) = t.additions.iter().find(|l| *l.node.id == *first)
            {
                single = jewel < 7;
                display = Some(
                    l.node
                        .sd
                        .iter()
                        .map(|s| &**s)
                        .collect::<Vec<_>>()
                        .join(" + "),
                );
            }
            let Some(display) = display else { continue };
            // `ipairs(desiredNode)`, each value read as it is reached
            let mut i = 0;
            while i < row.len() {
                let val = row[i].clone();
                if single && i == 1 {
                    let n = row[1]
                        .number()
                        .or_else(|| row.get(2).and_then(Cell::number))
                        .unwrap_or(1.0);
                    row[1] = Cell::N(n);
                }
                if val == Cell::S("required".into()) {
                    row[i] = if single && i == 1 {
                        row[1].clone()
                    } else {
                        Cell::N(0.0)
                    };
                    match row.get(3) {
                        None => {
                            row.resize(3, Cell::N(0.0));
                            row.push(Cell::N(0.001));
                        }
                        Some(Cell::N(n)) if *n < 0.001 => row[3] = Cell::N(0.001),
                        Some(Cell::N(_)) => {}
                        // Upstream compares the text with a number
                        Some(Cell::S(_)) => {
                            return Err(format!(
                                "{UPSTREAM_ERROR}attempt to compare string with number"
                            ));
                        }
                    }
                }
                i += 1;
            }
            if let Some(c) = row.get(3) {
                let w = c
                    .number()
                    .ok_or_else(|| format!("{UPSTREAM_ERROR}attempt to compare nil with number"))?;
                if w > 0.0 {
                    minimum.push((id.clone(), w));
                }
            }
            if jewel == 4
                && p.protected_count() > 0
                && (p.protected_count() as usize) < allocated_in_radius
                && combined < allocated_in_radius
            {
                minimum.push((id.clone(), 1.0));
            }
            let weight = row.get(1).and_then(Cell::number).unwrap_or(0.001);
            let weight2 = row.get(2).and_then(Cell::number).unwrap_or(0.001);
            match desired.iter_mut().find(|d| d.id == id) {
                Some(d) => {
                    d.weight = weight;
                    d.weight2 = weight2;
                    d.display = display;
                }
                None => {
                    desired_idx += 1;
                    desired.push(Desired {
                        id,
                        weight,
                        weight2,
                        display,
                        idx: desired_idx,
                    });
                }
            }
        }
        let roots: Vec<u32> = tree
            .classes
            .iter()
            .filter_map(|(_, c)| c.start_node_id)
            .collect();
        let radius = spec.radius_ids(socket, Some(3));
        let granted = &self.sidebar()?.granted_passives;
        let filter = p.socket_filter_state();
        let distance = td.socket_filter_distance.unwrap_or(0.0);
        let mut targets: LjMap<()> = LjMap::default();
        let (mut attribute_smalls, mut other_smalls) = (0.0, 0.0);
        let has_total = desired.iter().any(|d| d.id == "totalStat");
        for (id, _) in radius.iter() {
            if abyss {
                break;
            }
            let Some(n) = tree.node(id) else { continue };
            if roots.contains(&id) || n.kind == NodeType::Socket || n.kind == NodeType::Keystone {
                continue;
            }
            if filter {
                let allocated = granted.contains(&id) || spec.alloc_nodes.contains(id);
                let near = distance > 0.0 && {
                    let d = match spec.index.get(&id) {
                        Some(&i) => spec.nodes[i].path_dist,
                        None => return Err(unported("a node the spec does not have")),
                    };
                    d <= distance
                };
                if !allocated && !near {
                    continue;
                }
            }
            if n.kind == NodeType::Notable || jewel == 1 {
                targets.insert(id, ());
            } else if has_total {
                if ["Strength", "Intelligence", "Dexterity"].contains(&&*n.dn) {
                    attribute_smalls += 1.0;
                } else {
                    other_smalls += 1.0;
                }
            }
        }
        let protected = |id: u32| -> bool { tree.node(id).is_some_and(|n| p.is_protected(&n.dn)) };
        let mult = if jewel == 5 { 20.0 } else { 1.0 };
        let mut seeds = Vec::new();
        let mut seed = SEED_MIN[jewel - 1] * mult;
        while seed <= SEED_MAX[jewel - 1] * mult {
            seeds.push(seed);
            seed += mult;
        }
        let targets: Vec<(u32, bool)> = targets.iter().map(|(id, _)| (id, protected(id))).collect();
        // Militant Faith adds a Devotion total the first time a seed keeps
        // a protected notable: where, by seed and target
        let base = desired.clone();
        let mut with_total = desired.clone();
        let adds_total = jewel == 4 && !base.iter().any(|d| d.id == "totalStat");
        if adds_total {
            with_total.push(Desired {
                id: "totalStat".into(),
                weight: 0.1,
                weight2: 0.0,
                display: "Devotion".into(),
                idx: desired_idx + 1,
            });
        }
        let added_at: std::cell::Cell<Option<(f64, usize)>> = std::cell::Cell::new(None);
        // The desired nodes at a target of a seed
        let desired_at = |seed: f64, pos: usize| -> &Vec<Desired> {
            match added_at.get() {
                Some((s, p)) if seed > s || (seed == s && pos >= p) => &with_total,
                _ => &base,
            }
        };
        let add = |list: &mut Vec<(String, Matched)>, id: &str, node: u32, w: f64| {
            let i = match list.iter().position(|(k, _)| k == id) {
                Some(i) => i,
                None => {
                    list.push((id.to_string(), Matched::default()));
                    list.len() - 1
                }
            };
            let m = &mut list[i].1;
            m.total += w;
            m.names
                .push(tree.node(node).map_or(String::new(), |n| n.dn.to_string()));
        };
        // Each seed on its own: its weight, its matches and whether it is
        // dropped
        let score = |seed: f64| -> Result<Scored> {
            let mut weight = 0.0;
            let mut list: Vec<(String, Matched)> = Vec::new();
            let mut dropped = false;
            if abyss {
                // The affected nodes, in the order upstream's table gets
                // them
                let read = t.read_abyss_in(
                    seed,
                    socket,
                    jewel,
                    zorath.as_deref(),
                    ascendancy.as_deref(),
                )?;
                let mut affected: LjMap<usize> = LjMap::default();
                for (i, (n, _)) in read.iter().enumerate() {
                    affected.insert(*n, i);
                }
                for (target, &i) in affected.iter() {
                    let tree_node = tree.node(target);
                    let mut score = tree_node.is_some_and(|n| {
                        !roots.contains(&target)
                            && n.kind != NodeType::Socket
                            && n.kind != NodeType::Keystone
                    });
                    if score
                        && tree_node
                            .is_some_and(|n| n.ascendancy_name.is_some() && p.is_protected(&n.dn))
                    {
                        dropped = true;
                        break;
                    }
                    if score && filter && zorath.is_none() {
                        let allocated =
                            granted.contains(&target) || spec.alloc_nodes.contains(target);
                        let d = match spec.index.get(&target) {
                            Some(&k) => spec.nodes[k].path_dist,
                            None => return Err(unported("a node the spec does not have")),
                        };
                        score = allocated || (distance > 0.0 && d <= distance);
                    }
                    if !score {
                        continue;
                    }
                    for c in read[i].1.iter() {
                        // `resolveAbyssJewelComponent`
                        let changed = match c.kind {
                            1 => {
                                c.id.checked_sub(ADDITIONS)
                                    .and_then(|k| t.nodes.get(k as usize))
                            }
                            2 => t.additions.get(c.id as usize),
                            _ => None,
                        };
                        let Some(changed) = changed else { continue };
                        let Some(d) = base.iter().find(|d| *d.id == *changed.node.id) else {
                            continue;
                        };
                        // `getAbyssJewelComponentRoll(component, node, statIndex)`
                        let roll = |k: usize| -> Option<Option<f64>> {
                            let (_, stat) = changed.stats.iter().find(|(_, s)| s.index == k)?;
                            let r = c.rolls.get(stat.index.wrapping_sub(1)).copied();
                            Some(r.map(|r| if r < 0.0 && stat.min >= 0.0 { -r } else { r }))
                        };
                        let mut w = d.weight * roll(1).flatten().unwrap_or(1.0);
                        if let Some(r2) = roll(2) {
                            w += d.weight2 * r2.unwrap_or(1.0);
                        }
                        add(&mut list, &d.id, target, w);
                        weight += w;
                    }
                }
            }
            for (pos, &(target, is_protected)) in targets.iter().enumerate() {
                if dropped {
                    break;
                }
                let lut = t.read_lut(seed, target, jewel)?;
                if lut.is_empty() {
                    continue;
                }
                if jewel == 4 && is_protected && lut[0] >= ADDITIONS {
                    dropped = true;
                    break;
                }
                if adds_total && is_protected && added_at.get().is_none() {
                    added_at.set(Some((seed, pos)));
                }
                let desired = desired_at(seed, pos);
                let has_total = desired.iter().any(|d| d.id == "totalStat");
                let cur = if lut[0] >= ADDITIONS && !is_protected {
                    t.nodes.get((lut[0] - ADDITIONS) as usize)
                } else {
                    t.additions.get(lut[0] as usize)
                };
                let mut cur_id = cur.map(|c| c.node.id.to_string());
                if has_total && cur_id.as_deref().is_some_and(|c| TOTAL_IDS.contains(&c)) {
                    cur_id = Some("totalStat".into());
                }
                let find = |id: &Option<String>| -> Option<&Desired> {
                    let id = id.as_deref()?;
                    desired.iter().find(|d| d.id == id)
                };
                if jewel == 1 {
                    let header = lut.len();
                    if header == 2 || header == 3 {
                        if let (Some(d), Some(c)) = (find(&cur_id), cur) {
                            let roll = |k: usize| -> Result<Option<f64>> {
                                let Some((_, s)) = c.sorted_stat(k) else {
                                    return Ok(None);
                                };
                                lut.get(s.index)
                                    .map(|v| Some(*v as f64))
                                    .ok_or_else(|| unported("a roll the table does not have"))
                            };
                            let mut w = d.weight * roll(0)?.ok_or_else(|| unported("a stat"))?;
                            if let Some(r) = roll(1)? {
                                w += d.weight2 * r;
                            }
                            add(&mut list, &d.id, target, w);
                            weight += w;
                        }
                    } else if header == 6 || header == 8 {
                        for k in 0..header / 2 {
                            let id = t
                                .additions
                                .get(lut[k] as usize)
                                .map(|a| a.node.id.to_string());
                            if let Some(d) = find(&id) {
                                let w = d.weight * lut[k + header / 2] as f64;
                                add(&mut list, &d.id, target, w);
                                weight += w;
                            }
                        }
                    }
                } else if let Some(d) = find(&cur_id) {
                    add(&mut list, &d.id, target, d.weight);
                    weight += d.weight;
                }
            }
            if !dropped {
                let desired = desired_at(seed, targets.len());
                if let Some(d) = desired.iter().find(|d| d.id == "totalStat") {
                    let i = match list.iter().position(|(k, _)| k == "totalStat") {
                        Some(i) => i,
                        None => {
                            list.push(("totalStat".into(), Matched::default()));
                            list.len() - 1
                        }
                    };
                    let m = &mut list[i].1;
                    let added = if jewel == 4 {
                        d.weight * (5.0 * other_smalls + 10.0 * attribute_smalls) + m.total * 4.0
                    } else {
                        d.weight * (4.0 * other_smalls + 2.0 * attribute_smalls) + m.total * 19.0
                    };
                    m.total += added;
                    weight += added;
                }
                dropped = minimum.iter().any(|(id, w)| {
                    list.iter()
                        .find(|(k, _)| k == id)
                        .map_or(0.0, |(_, m)| m.total)
                        < *w
                });
            }
            Ok((weight, list, dropped))
        };
        let mut results: LjMap<Vec<(String, Matched)>> = LjMap::default();
        let mut weights: LjMap<f64> = LjMap::default();
        for seed in seeds {
            let (weight, list, dropped) = score(seed)?;
            let key = seed as u32;
            weights.insert(key, weight);
            // `resultNodes[curSeed] = { }`, then nil for a seed dropped
            results.insert(key, list);
            if dropped {
                results.remove(key);
            }
        }
        let desired = if added_at.get().is_some() {
            with_total
        } else {
            base
        };
        Ok(Some(SocketSearch {
            results,
            weights,
            desired,
        }))
    }

    /// `formatResults(resultNodes, seedWeights, desiredNodes, socketInfo)`
    fn format_results(&self, s: &SocketSearch, socket: Option<(f64, String)>) -> Vec<SeedResult> {
        let jewel = self.tree_tab.timeless.jewel_type.unwrap_or(1.0) as usize;
        let min = self.tree_tab.timeless.total_minimum_weight;
        let mut out = Vec::new();
        for (seed, list) in s.results.iter() {
            let weight = s.weights.get(seed).copied().unwrap_or(0.0);
            if weight <= 0.0 || min.is_some_and(|m| weight < m) {
                continue;
            }
            let prefix = socket
                .as_ref()
                .map_or(String::new(), |(_, l)| format!("{l} | "));
            let mut label = format!("{prefix}{seed}:");
            let pad = match jewel {
                1 | 3 | 7.. if seed < 1000 => "  ",
                4 if seed < 10000 => "  ",
                2 | 5 | 6 if seed < 10000 => "    ",
                2 | 5 | 6 if seed < 100000 => "  ",
                _ => "",
            };
            label = format!("{pad}{label}");
            let mut cells: Vec<Option<String>> = Vec::new();
            let mut matched = Vec::new();
            for d in &s.desired {
                let set = |cells: &mut Vec<Option<String>>, v: String| {
                    if cells.len() < d.idx {
                        cells.resize(d.idx, None);
                    }
                    cells[d.idx - 1] = Some(v);
                };
                if let Some((_, m)) = list.iter().find(|(k, _)| *k == d.id) {
                    if d.idx == 8 {
                        set(&mut cells, " ...".into());
                    } else if d.idx < 8 {
                        set(&mut cells, format_value(m.total));
                    }
                    matched.push((d.id.clone(), m.names.clone()));
                } else if d.idx < 8 {
                    set(&mut cells, "     0     ".into());
                }
            }
            // `t_concat` stops at the first hole
            for c in cells.into_iter().map_while(|c| c) {
                label.push_str(&c);
            }
            out.push(SeedResult {
                label,
                seed: seed as f64,
                total: weight,
                socket: socket.clone(),
                matched,
                alt: None,
            });
        }
        out
    }

    /// The Search button
    pub(super) fn timeless_search(&mut self, p: &mut FindTimeless) -> Result<()> {
        let td = &self.tree_tab.timeless;
        let socket = td.socket.unwrap_or(-1.0);
        let devotion = td.devotion.map(|d| d as usize);
        let shared = |desired: Vec<Desired>, socket: Option<(f64, String, String)>| Shared {
            jewel: p.jewel(),
            conqueror: p.conqueror(),
            devotion,
            socket,
            desired,
        };
        if socket == -1.0 {
            let mut results = Vec::new();
            let mut desired = Vec::new();
            for (id, label, _) in p.socket_list().into_iter().skip(1) {
                // Upstream wipes the results before it fails
                let s = self.search_socket(p, id as u32).map_err(|e| {
                    if e.starts_with(UPSTREAM_ERROR) {
                        unported("a failing search of all sockets")
                    } else {
                        e
                    }
                })?;
                if let Some(s) = s {
                    results.extend(self.format_results(&s, Some((id, label))));
                    desired = s.desired;
                }
            }
            engine::calcs::lua_sort(&mut results, |a, b| {
                if a.total == b.total {
                    let l =
                        |r: &SeedResult| r.socket.as_ref().map_or(String::new(), |s| s.1.clone());
                    l(a) < l(b)
                } else {
                    a.total > b.total
                }
            });
            self.tree_tab.timeless.results = results;
            self.tree_tab.timeless.shared = Some(shared(desired, None));
        } else {
            let entry = p.socket_list().into_iter().find(|(id, _, _)| *id == socket);
            let Some(s) = self.search_socket(p, socket as u32)? else {
                return Ok(());
            };
            let mut results = self.format_results(&s, None);
            engine::calcs::lua_sort(&mut results, |a, b| a.total > b.total);
            self.tree_tab.timeless.results = results;
            self.tree_tab.timeless.shared = Some(shared(s.desired, entry));
        }
        p.searched();
        let enabled = !self.tree_tab.timeless.results.is_empty();
        p.set_trade_enabled(enabled);
        Ok(())
    }
}

/// `devotionVariants[i].id`, by index from 1
const DEVOTION_IDS: [usize; 17] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 17, 15, 16];

/// `GetJewelItem`'s texts of the non-Abyss jewels: name, the conquerors'
/// variants and their lines (`{}` for the seed), and the conquering line
/// A jewel's name, its conquerors (variant name and line) and its
/// conquering line
type JewelText = (
    &'static str,
    [(&'static str, &'static str); 3],
    &'static str,
);

const JEWELS: [JewelText; 6] = [
    (
        "Glorious Vanity",
        [
            (
                "Doryani (Corrupted Soul)",
                "Bathed in the blood of {} sacrificed in the name of Doryani",
            ),
            (
                "Xibaqua (Divine Flesh)",
                "Bathed in the blood of {} sacrificed in the name of Xibaqua",
            ),
            (
                "Ahuana (Immortal Ambition)",
                "Bathed in the blood of {} sacrificed in the name of Ahuana",
            ),
        ],
        "Passives in radius are Conquered by the Vaal",
    ),
    (
        "Lethal Pride",
        [
            (
                "Kaom (Strength of Blood)",
                "Commanded leadership over {} warriors under Kaom",
            ),
            (
                "Rakiata (Tempered by War)",
                "Commanded leadership over {} warriors under Rakiata",
            ),
            (
                "Akoya (Chainbreaker)",
                "Commanded leadership over {} warriors under Akoya",
            ),
        ],
        "Passives in radius are Conquered by the Karui",
    ),
    (
        "Brutal Restraint",
        [
            (
                "Asenath (Dance with Death)",
                "Denoted service of {} dekhara in the akhara of Asenath",
            ),
            (
                "Nasima (Second Sight)",
                "Denoted service of {} dekhara in the akhara of Nasima",
            ),
            (
                "Balbala (The Traitor)",
                "Denoted service of {} dekhara in the akhara of Balbala",
            ),
        ],
        "Passives in radius are Conquered by the Maraketh",
    ),
    (
        "Militant Faith",
        [
            (
                "Avarius (Power of Purpose)",
                "Carved to glorify {} new faithful converted by High Templar Avarius",
            ),
            (
                "Dominus (Inner Conviction)",
                "Carved to glorify {} new faithful converted by High Templar Dominus",
            ),
            (
                "Maxarius (Transcendence)",
                "Carved to glorify {} new faithful converted by High Templar Maxarius",
            ),
        ],
        "Passives in radius are Conquered by the Templars",
    ),
    (
        "Elegant Hubris",
        [
            (
                "Cadiro (Supreme Decadence)",
                "Commissioned {} coins to commemorate Cadiro",
            ),
            (
                "Victario (Supreme Grandstanding)",
                "Commissioned {} coins to commemorate Victario",
            ),
            (
                "Caspiro (Supreme Ostentation)",
                "Commissioned {} coins to commemorate Caspiro",
            ),
        ],
        "Passives in radius are Conquered by the Eternal Empire",
    ),
    (
        "Heroic Tragedy",
        [
            (
                "Vorana (Black Scythe Training)",
                "Remembrancing {} songworthy deeds by the line of Vorana",
            ),
            (
                "Uhtred (Celestial Mathematics)",
                "Remembrancing {} songworthy deeds by the line of Uhtred",
            ),
            (
                "Medved (The Unbreaking Circle)",
                "Remembrancing {} songworthy deeds by the line of Medved",
            ),
        ],
        "Passives in radius are Conquered by the Kalguur",
    ),
];

/// `GetJewelItem`'s Abyss jewels: name, base and the seed's line (`{}`
/// for the seed)
const ABYSS_JEWELS: [(&str, &str, &str); 5] = [
    (
        "Festering Vengeance",
        "Murderous Eye Jewel",
        "Subjugating {} souls in the thrall of Tecrod",
    ),
    (
        "Extinguishing Grasp",
        "Searching Eye Jewel",
        "Subjugating {} souls in the thrall of Ulaman",
    ),
    (
        "Baleful Dominion",
        "Hypnotic Eye Jewel",
        "Subjugating {} souls in the thrall of Kurgal",
    ),
    (
        "Destructive Aspiration",
        "Ghastly Eye Jewel",
        "Subjugating {} souls in the thrall of Amanamu",
    ),
    (
        "Reclaimed Malevolence",
        "Assembled Eye Jewel",
        "Binding {} souls to phylacteries to sustain Zorath",
    ),
];

/// Militant Faith's devotion variants after its conquerors
const DEVOTION_VARIANTS: [(&str, &str); 16] = [
    ("Totem Damage", "4% increased Totem Damage per 10 Devotion"),
    ("Brand Damage", "4% increased Brand Damage per 10 Devotion"),
    (
        "Channelling Damage",
        "Channelling Skills deal 4% increased Damage per 10 Devotion",
    ),
    ("Area Damage", "4% increased Area Damage per 10 Devotion"),
    (
        "Elemental Damage",
        "4% increased Elemental Damage per 10 Devotion",
    ),
    (
        "Elemental Resistances",
        "+2% to all Elemental Resistances per 10 Devotion",
    ),
    (
        "Effect of non-Damaging Ailments",
        "3% increased Effect of non-Damaging Ailments on Enemies per 10 Devotion",
    ),
    (
        "Elemental Ailment Duration",
        "4% reduced Elemental Ailment Duration on you per 10 Devotion",
    ),
    (
        "Duration of Curses",
        "4% reduced Duration of Curses on you per 10 Devotion",
    ),
    (
        "Minion Attack and Cast Speed",
        "1% increased Minion Attack and Cast Speed per 10 Devotion",
    ),
    (
        "Minions Accuracy Rating",
        "Minions have +60 to Accuracy Rating per 10 Devotion",
    ),
    (
        "Mana Regen",
        "Regenerate 0.6 Mana per Second per 10 Devotion",
    ),
    (
        "Mana Cost (legacy)",
        "1% reduced Mana Cost of Skills per 10 Devotion",
    ),
    (
        "Non-Curse Aura Effect",
        "1% increased effect of Non-Curse Auras per 10 Devotion",
    ),
    (
        "Defences from Shield",
        "3% increased Defences from Equipped Shield per 10 Devotion",
    ),
    (
        "Mana Cost Efficiency",
        "3% increased Mana Cost Efficiency per 10 Devotion",
    ),
];

impl<'d> TypedSession<'d> {
    /// `GetJewelItem(data)` of a results row: its text, and Militant
    /// Faith's alternate variants
    fn timeless_jewel_text(&self, row: usize) -> Result<(String, Option<(usize, usize)>)> {
        let td = &self.tree_tab.timeless;
        let r = td.results.get(row - 1).ok_or("no row")?;
        let sh = td
            .shared
            .as_ref()
            .ok_or_else(|| format!("{UPSTREAM_ERROR}attempt to index a nil value"))?;
        let socket_info = r
            .socket
            .as_ref()
            .map(|s| s.1.clone())
            .or_else(|| sh.socket.as_ref().map(|s| s.2.clone()))
            .unwrap_or_else(|| "Unknown".into());
        let seed = lua::value::format_number(r.seed);
        let label = format!(
            "[{seed}; {}; {socket_info}]\n",
            lua::value::format_number(r.total)
        );
        let variant = if sh.conqueror == 1 {
            "1".to_string()
        } else {
            format!("{}\n", sh.conqueror - 1)
        };
        if let Some((name, base, line)) = ABYSS_JEWELS.get(sh.jewel.wrapping_sub(7)) {
            let line = line.replace("{}", &seed);
            return Ok((
                format!(
                    "{name} {label}{base}\nLeague: Allflame\nLimited to: 1 Historic\nImplicits: 0\n{line}\nPassives affected are Conquered by the Abyssal\nHistoric\n"
                ),
                None,
            ));
        }
        let Some((name, conquerors, conquered)) = JEWELS.get(sh.jewel.wrapping_sub(1)) else {
            return Err(unported("a timeless jewel's item"));
        };
        let mut text =
            format!("{name} {label}Timeless Jewel\nLeague: Legion\nLimited to: 1 Historic\n");
        let mut alt = None;
        if sh.jewel == 4 {
            let id = |i: usize| DEVOTION_IDS.get(sh.devotion[i].wrapping_sub(1)).copied();
            let pick = |i: usize, stored: Option<usize>| match id(i) {
                Some(v) if v != 1 => Some(v),
                _ => stored,
            };
            let (a1, a2) = match (pick(0, r.alt.map(|a| a.0)), pick(1, r.alt.map(|a| a.1))) {
                (Some(a1), Some(a2)) => (a1, a2),
                // A random devotion variant
                _ => return Err(unported("Militant Faith with any devotion variant")),
            };
            let a1 = if a1 == a2 { a1 % 16 + 2 } else { a1 };
            alt = Some((a1, a2));
            text.push_str("Has Alt Variant: true\nHas Alt Variant Two: true\n");
            for (v, _) in conquerors {
                text.push_str(&format!("Variant: {v}\n"));
            }
            for (v, _) in &DEVOTION_VARIANTS {
                text.push_str(&format!("Variant: {v}\n"));
            }
            text.push_str(&format!(
                "Selected Variant: {variant}\nSelected Alt Variant: {}\nSelected Alt Variant Two: {}\n",
                a1 + 2,
                a2 + 2
            ));
        } else {
            for (v, _) in conquerors {
                text.push_str(&format!("Variant: {v}\n"));
            }
            text.push_str(&format!("Selected Variant: {variant}\n"));
        }
        text.push_str("Radius: Large\nImplicits: 0\n");
        for (i, (_, line)) in conquerors.iter().enumerate() {
            text.push_str(&format!(
                "{{variant:{}}}{}\n",
                i + 1,
                line.replace("{}", &seed)
            ));
        }
        if sh.jewel == 4 {
            for (i, (_, line)) in DEVOTION_VARIANTS.iter().enumerate() {
                text.push_str(&format!("{{variant:{}}}{line}\n", i + 4));
            }
        }
        text.push_str(&format!("{conquered}\nHistoric\n"));
        Ok((text, alt))
    }

    /// The socket a results row's jewel goes in: `data.socketId or
    /// sharedList.socket.id`
    fn timeless_row_socket(&self, row: usize) -> Result<f64> {
        let td = &self.tree_tab.timeless;
        let r = td.results.get(row - 1).ok_or("no row")?;
        r.socket
            .as_ref()
            .map(|s| s.0)
            .or_else(|| td.shared.as_ref()?.socket.as_ref().map(|s| s.0))
            .ok_or_else(|| format!("{UPSTREAM_ERROR}attempt to index a nil value"))
    }

    /// `AddValueTooltip` of a results row
    pub(super) fn timeless_row_tooltip(&mut self, row: usize) -> Result<serde_json::Value> {
        let socket_id = self.timeless_row_socket(row)?;
        if socket_id != -1.0 && self.socket_slot(socket_id as u32).is_none() {
            return Err(unported("a row's socket without a slot"));
        }
        let (text, alt) = self.timeless_jewel_text(row)?;
        self.tree_tab.timeless.results[row - 1].alt =
            alt.or(self.tree_tab.timeless.results[row - 1].alt);
        let td = &self.tree_tab.timeless;
        let r = &td.results[row - 1];
        let sh = td.shared.as_ref().ok_or("no search")?;
        let mut tip = super::tooltip::Tooltip::default();
        if r.label.contains("B2B2B2") {
            let jewel = super::timeless_search::JEWEL_TYPES[sh.jewel - 1].0;
            tip.add_line(
                16.0,
                &format!(
                    "^7{jewel} {} was successfully added to your build.",
                    lua::value::format_number(r.seed)
                ),
            );
        } else {
            tip.add_line(16.0, "^7Double click to add this jewel to your build.");
            if td.socket_allocate {
                let allocated =
                    socket_id != -1.0 && self.build.spec.alloc_nodes.contains(socket_id as u32);
                if allocated {
                    let label = r
                        .socket
                        .as_ref()
                        .map(|s| s.1.clone())
                        .or_else(|| sh.socket.as_ref().map(|s| s.1.clone()))
                        .unwrap_or_else(|| lua::value::format_number(socket_id));
                    tip.add_line(16.0, &format!("^7It will be socketed into {label}."));
                } else {
                    tip.add_line(
                        16.0,
                        &format!(
                            "{}That jewel socket is not allocated, so the jewel will only be added to your items.",
                            super::tooltip::cc("WARNING")
                        ),
                    );
                }
            }
        }
        let mut desired = sh.desired.clone();
        desired.sort_by_key(|d| d.idx);
        let lists: Vec<String> = desired
            .iter()
            .filter_map(|d| {
                let (_, names) = r.matched.iter().find(|(k, _)| *k == d.id)?;
                Some(if names.is_empty() {
                    format!("^7        {}:\n^8                None", d.display)
                } else {
                    format!(
                        "^7        {}:\n^8                {}",
                        d.display,
                        names.join("\n                ")
                    )
                })
            })
            .collect();
        if !lists.is_empty() {
            tip.add_line(16.0, "^7Node List:");
            for l in &lists {
                tip.add_line(16.0, l);
            }
        }
        if r.total > 0.0 {
            tip.add_line(
                16.0,
                &format!(
                    "^7Combined Node Weight: {}",
                    lua::value::format_number(r.total)
                ),
            );
        }
        let item = engine::item::Item::new(&self.data.item_context(), &text, None, false)?;
        let slot = (socket_id != -1.0)
            .then(|| self.socket_slot(socket_id as u32))
            .flatten();
        self.add_item_stat_differences(&mut tip, &item, slot)?;
        Ok(tip.json())
    }

    /// `OnSelClick` of a results row, double clicked: the jewel added to
    /// the build, and socketed when Socket Jewel is on
    pub(super) fn timeless_add_jewel(&mut self, row: usize) -> Result<()> {
        let (text, alt) = self.timeless_jewel_text(row)?;
        if alt.is_some() {
            self.tree_tab.timeless.results[row - 1].alt = alt;
        }
        let item = engine::item::Item::new(&self.data.item_context(), &text, None, false)?;
        let socket_id = if self.tree_tab.timeless.socket_allocate {
            Some(self.timeless_row_socket(row)?)
        } else {
            None
        };
        let id = self.add_item(item, true)?;
        let mut build = false;
        if let Some(s) = socket_id.filter(|s| *s != -1.0) {
            let tab = &self.build.items;
            let slot = tab
                .ordered_slots
                .iter()
                .position(|x| x.node_id == Some(s as u32));
            if let Some(i) = slot
                && self.build.spec.alloc_nodes.contains(s as u32)
                && self.valid_for_slot(
                    self.build.items.item(id).ok_or("no item")?,
                    &self.build.items.ordered_slots[i].slot_name.clone(),
                )
            {
                self.set_sel_item(i, id)?;
                build = true;
            }
        }
        self.populate_slots();
        // `itemsTab:AddUndoState()`
        self.ui.unsaved = true;
        let r = &mut self.tree_tab.timeless.results[row - 1];
        r.label = format!("^xB2B2B2{}", r.label);
        if build {
            self.rebuild()?;
        }
        Ok(())
    }
}

/// `totalModIDs`: the passives each total stands for
fn total_ids(total: &str) -> &'static [&'static str] {
    match total {
        "total_strength" => &TOTAL_IDS[0..3],
        "total_dexterity" => &TOTAL_IDS[3..6],
        "total_devotion" => &TOTAL_IDS[6..9],
        _ => &[],
    }
}

impl<'d> TypedSession<'d> {
    /// `modLib.parseMod(line)`: none when the line is not understood
    /// (`AddList(nil)` adds nothing)
    fn timeless_parse(&self, line: &str) -> Vec<engine::mods::Mod> {
        self.data
            .parser
            .parse(line.as_bytes())
            .0
            .unwrap_or_default()
    }

    /// Generate (`setupFallbackWeights`): each passive of the jewel type
    /// weighed by what it adds to the fallback mode's stat, written to the
    /// fallback list
    pub(super) fn timeless_generate(&mut self, p: &mut FindTimeless) -> Result<()> {
        let t = self
            .data
            .timeless()
            .ok_or_else(|| format!("{NOT_PORTED}: timeless jewels without their data"))?;
        let jewel = p.jewel();
        let variable = jewel == 1 || jewel >= 7;
        // Each passive's mod lists: one, or with several stats one per
        // stat (`calcMultiple`), each with its divisor
        let mut nodes: Vec<(String, Vec<WeightedLine>)> = Vec::new();
        let describer = &self.data.game.stat_descriptions;
        // `buildStatModLists(legionPassive)`: each stat at 100, the others
        // at 0, described and parsed
        let stat_lists =
            |l: &engine::timeless::LegionNode| -> Result<Vec<(Vec<engine::mods::Mod>, f64)>> {
                let mut out = Vec::new();
                for key in &l.sorted_stats {
                    let values: Vec<(String, f64)> = l
                        .stats
                        .iter()
                        .map(|(k, _)| (k.to_string(), if k == key { 100.0 } else { 0.0 }))
                        .collect();
                    let lines = describer
                        .describe_stats(&values, "stat_descriptions")
                        .map_err(|why| format!("{NOT_PORTED}: {why}"))?;
                    let line = lines
                        .first()
                        .ok_or_else(|| unported("a stat without a description"))?;
                    out.push((self.timeless_parse(line), 100.0));
                }
                Ok(out)
            };
        for (id, total) in p.mod_ids() {
            let matches = |lid: &str| lid == id || total_ids(&id).contains(&lid);
            let missing = || unported("a passive without stats");
            if let Some(l) = t.nodes.iter().find(|l| matches(&l.node.id)) {
                if variable && l.sorted_stats.len() > 1 {
                    nodes.push((id, stat_lists(l)?));
                    continue;
                }
                let (mods, divisor) = if variable {
                    let original = l.node.sd.first().ok_or_else(missing)?;
                    let (key, stat) = l.sorted_stat(0).ok_or_else(missing)?;
                    let line = engine::timeless::replace_stat(original, key, stat, 100.0);
                    let mods = if line == **original && l.node.sd.len() > 1 {
                        l.node.mod_list.clone()
                    } else {
                        self.timeless_parse(&line)
                    };
                    (mods, if line != **original { 100.0 } else { 1.0 })
                } else {
                    let mods = l.node.mod_list.clone();
                    let divisor = if total { first_value(&mods)? } else { 1.0 };
                    (mods, divisor)
                };
                nodes.push((id, vec![(mods, divisor)]));
                continue;
            }
            if let Some(a) = t.additions.iter().find(|a| matches(&a.node.id)) {
                if variable && a.sorted_stats.len() > 1 {
                    nodes.push((id, stat_lists(a)?));
                    continue;
                }
                let original = a.node.sd.first().ok_or_else(missing)?.to_string();
                let mut line = original.clone();
                if variable {
                    for (key, stat) in &a.stats {
                        line = engine::timeless::replace_stat(&line, key, stat, 100.0);
                    }
                }
                let mods = self.timeless_parse(&line);
                let mut divisor = if line != original { 100.0 } else { 1.0 };
                if !variable && total {
                    divisor = first_value(&mods)?;
                }
                nodes.push((id, vec![(mods, divisor)]));
            }
        }
        // `generateFallbackWeights(nodes, powerStat)`
        let stats: Vec<engine::power::PowerStat> = engine::power::power_stats()
            .into_iter()
            .filter(|s| !s.ignore_for_items && s.label != "Name")
            .collect();
        let mode = p.fallback_mode();
        let stat = (mode >= 1.0 && mode.fract() == 0.0)
            .then(|| stats.get(mode as usize - 1))
            .flatten()
            .ok_or_else(|| format!("{UPSTREAM_ERROR}attempt to index a nil value"))?;
        let data = self.data.calc_data();
        let calc = engine::calculator::MiscCalculator::new(&self.build, &data)?;
        let base = stat.value(calc.base());
        let mut text = String::new();
        for (id, lines) in nodes {
            // `weight1`, `weight2`, ... of each line
            let mut weights = Vec::new();
            for (mods, divisor) in lines {
                let node = engine::calc_override::mod_list_node(&self.build.spec, mods)
                    .ok_or("a tree without nodes")?;
                let out = calc.calculate(
                    &engine::calc_override::Override {
                        add_nodes: vec![engine::calc_override::OverrideNode::Own(
                            std::sync::Arc::new(node),
                        )],
                        ..Default::default()
                    },
                    true,
                )?;
                weights.push(if base == 0.0 {
                    0.0
                } else {
                    (stat.value(&out) - base) / base.abs() / divisor
                });
            }
            let w1 = weights.first().copied().unwrap_or(0.0);
            let w2 = weights.get(1).copied();
            if w1 != 0.0 || w2.is_some_and(|w| w != 0.0) {
                if !text.is_empty() {
                    text.push('\n');
                }
                let round = |v: f64| (v * 1000.0 + 0.5).floor() / 1000.0;
                text.push_str(&format!(
                    "{id}, {}, {}, 0",
                    lua::value::format_number(round(w1 * 100.0)),
                    lua::value::format_number(round(w2.unwrap_or(0.0) * 100.0))
                ));
            }
        }
        self.timeless_update_list(p, &text, true);
        Ok(())
    }
}

/// A passive's mod list and its divisor, weighed on its own
type WeightedLine = (Vec<engine::mods::Mod>, f64);

/// `modList[1].value`: a total's divisor
fn first_value(mods: &[engine::mods::Mod]) -> Result<f64> {
    match mods.first().map(|m| &m.value) {
        Some(engine::mods::ModValue::Number(n)) => Ok(*n),
        _ => Err(unported("a total whose first mod is no number")),
    }
}
