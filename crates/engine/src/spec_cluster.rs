//! Cluster jewel subgraphs of a passive spec:
//! `PassiveSpecClass:BuildClusterJewelGraphs` and `BuildSubgraph` of
//! `Classes/PassiveSpec.lua`, with the conversion of legacy cluster node
//! ids (`clusterHashFormatVersion` 1).
//!
//! A subgraph's nodes are synthetic: the spec owns them
//! ([`SpecNode::synth`](crate::spec::SpecNode)), appended in the order
//! upstream adds them to `spec.nodes`, each nested subgraph after its
//! socket.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::item::Item;
use crate::item_data::{ClusterJewel, ItemData};
use crate::items_tab::ItemsTab;
use crate::mods::{Mod, ModType, ModValue};
use crate::spec::{PassiveSpec, SpecNode};
use crate::tree::{ExpansionJewel, Node, NodeType};
use std::collections::HashMap;
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, String>;

/// A Lua table with integer keys as the translated runtime orders it: the
/// array part `1..n` first, then the other keys in insertion order
#[derive(Default)]
struct IntTable<T> {
    array: Vec<T>,
    hash: Vec<(i64, T)>,
}

impl<T: Clone> IntTable<T> {
    fn get(&self, k: i64) -> Option<&T> {
        if k >= 1 && (k as usize) <= self.array.len() {
            return self.array.get(k as usize - 1);
        }
        self.hash.iter().find(|(x, _)| *x == k).map(|(_, v)| v)
    }

    fn set(&mut self, k: i64, v: T) {
        if k >= 1 && (k as usize) <= self.array.len() {
            self.array[k as usize - 1] = v;
            return;
        }
        if k >= 1 && k as usize == self.array.len() + 1 {
            self.hash.retain(|(x, _)| *x != k);
            self.array.push(v);
            loop {
                let next = self.array.len() as i64 + 1;
                match self.hash.iter().position(|(x, _)| *x == next) {
                    Some(p) => {
                        let (_, v) = self.hash.remove(p);
                        self.array.push(v);
                    }
                    None => break,
                }
            }
            return;
        }
        match self.hash.iter_mut().find(|(x, _)| *x == k) {
            Some(e) => e.1 = v,
            None => self.hash.push((k, v)),
        }
    }

    fn pairs(&self) -> Vec<(i64, T)> {
        self.array
            .iter()
            .enumerate()
            .map(|(i, v)| (i as i64 + 1, v.clone()))
            .chain(self.hash.iter().cloned())
            .collect()
    }
}

/// A subgraph node before it joins the spec: its data and links by id
struct Pending {
    node: Node,
    linked: Vec<u32>,
}

/// What building the subgraphs collects
#[derive(Default)]
struct Builder {
    pending: Vec<Pending>,
    by_id: HashMap<u32, usize>,
    /// Links added to the spec's own nodes (the large sockets)
    spec_links: Vec<(usize, u32)>,
}

/// The parent socket of a subgraph
#[derive(Clone, Copy)]
enum Parent {
    Spec(usize),
    Pending(u32),
}

/// A node of a subgraph while it is built
struct Synth {
    kind: NodeType,
    id: u32,
    dn: Arc<str>,
    sd: Vec<Arc<str>>,
    expansion: Option<ExpansionJewel>,
    orbit: i64,
    oidx: i64,
    linked: Vec<u32>,
}

fn field<'a>(item: &'a Item, k: &str) -> Option<&'a ModValue> {
    item.jewel_data.as_ref()?.get(k)
}

fn field_num(item: &Item, k: &str) -> Option<f64> {
    field(item, k).and_then(ModValue::as_number)
}

fn strings(v: Option<&ModValue>) -> Vec<String> {
    match v {
        Some(ModValue::List(l)) => l
            .iter()
            .filter_map(|x| match x {
                ModValue::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// `PassiveSpecClass:TranslateClusterOrbitIndex`
fn translate_orbit_index(src: i64, src_n: i64, dest_n: i64) -> i64 {
    let table: &[i64] = match (src_n, dest_n) {
        (a, b) if a == b => return src,
        (12, 16) => &[0, 1, 3, 4, 5, 7, 8, 9, 11, 12, 13, 15],
        (16, 12) => &[0, 1, 1, 2, 3, 4, 4, 5, 6, 7, 7, 8, 9, 10, 10, 11],
        (6, 16) => &[0, 3, 5, 8, 11, 13],
        (16, 6) => &[0, 0, 0, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 5, 5, 5],
        _ => return (src as f64 * dest_n as f64 / src_n as f64).floor() as i64,
    };
    table.get(src as usize).copied().unwrap_or(0)
}

impl<'t> PassiveSpec<'t> {
    /// `GetJewel(itemId)`
    fn jewel<'i>(&self, tab: &'i ItemsTab, id: Option<f64>) -> Option<&'i Item> {
        let id = id.filter(|i| *i != 0.0)?;
        tab.item(id).filter(|i| i.jewel_data.is_some())
    }

    /// `GetSocketedJewel(nodeId)`
    fn socketed_jewel<'i>(&self, tab: &'i ItemsTab, node: u32) -> Option<&'i Item> {
        let mut id = self.jewels.get(&node).copied();
        if id.is_none_or(|i| i == 0.0)
            && let Some(rev) = &self.legacy_reverse
            && let Some(legacy) = rev.get(&node)
        {
            id = self.jewels.get(legacy).copied();
        }
        self.jewel(tab, id)
    }

    /// `RegisterLegacyClusterNodeMap(legacyNodeId, currentNodeId)`
    fn register_legacy(&mut self, legacy: u32, current: u32) {
        let Some(map) = &mut self.legacy_map else {
            return;
        };
        match map.iter_mut().find(|(k, _)| *k == legacy) {
            Some(e) => e.1 = current,
            None => map.push((legacy, current)),
        }
        if let Some(rev) = &mut self.legacy_reverse {
            rev.insert(current, legacy);
        }
    }

    /// `FindClusterSocket(group, index)`
    fn find_cluster_socket(&self, group: i64, index: f64) -> Option<&'t Node> {
        let tree = self.tree;
        tree.groups.get(&group)?.nodes.iter().find_map(|id| {
            let n = tree.node(*id)?;
            n.expansion_jewel
                .as_ref()
                .is_some_and(|e| e.index == index)
                .then_some(n)
        })
    }

    /// `BuildLegacyProxyGroup(proxyGroup, expansionJewelSize, clusterSizeIndex)`
    fn legacy_proxy_group(&self, proxy_group: i64, size: f64, size_index: f64) -> i64 {
        let mut group = proxy_group;
        let mut group_size = size;
        let mut guard = 0;
        while size_index < group_size && guard < 4 {
            let Some(socket) = self
                .find_cluster_socket(group, 1.0)
                .or_else(|| self.find_cluster_socket(group, 0.0))
            else {
                break;
            };
            let Some(ej) = &socket.expansion_jewel else {
                break;
            };
            let proxy = ej
                .proxy
                .as_deref()
                .and_then(crate::pat::to_number)
                .and_then(|p| self.tree.node(p as u32));
            let Some(g) = proxy.and_then(|p| p.group) else {
                break;
            };
            group = g;
            group_size = ej.size;
            guard += 1;
        }
        group
    }

    /// `BuildClusterJewelGraphs()` after loading: the subgraphs of the
    /// cluster jewels socketed in large sockets, their allocated nodes,
    /// then the dependencies and paths
    pub fn build_cluster_jewel_graphs(&mut self, tab: &ItemsTab, data: &ItemData) -> Result<()> {
        self.set_tree_jewels(tab);
        let legacy = self.cluster_hash_format_version < 2.0;
        self.legacy_map = legacy.then(Vec::new);
        self.legacy_reverse = legacy.then(HashMap::new);
        let mut b = Builder::default();
        for &socket in &self.tree.sockets {
            let Some(node) = self.tree.node(socket) else {
                continue;
            };
            let large = node.expansion_jewel.as_ref().is_some_and(|e| e.size == 2.0);
            let Some(jewel) = self.socketed_jewel(tab, socket) else {
                continue;
            };
            if large
                && self.alloc_nodes.contains(socket)
                && field(jewel, "clusterJewelValid").is_some_and(ModValue::truthy)
                && let Some(&parent) = self.index.get(&socket)
            {
                self.build_subgraph(&mut b, tab, data, jewel, Parent::Spec(parent), None)?;
            }
        }
        // The synthetic nodes join the spec
        let base = self.nodes.len();
        for (k, p) in b.pending.iter().enumerate() {
            self.index.insert(p.node.id, base + k);
            self.order.insert(p.node.id, base + k);
        }
        for p in std::mem::take(&mut b.pending) {
            let linked = p
                .linked
                .iter()
                .filter_map(|id| self.index.get(id).copied())
                .collect();
            self.nodes.push(SpecNode::synthetic(p.node, linked));
        }
        for (i, id) in b.spec_links {
            if let Some(&j) = self.index.get(&id) {
                self.nodes[i].linked.push(j);
            }
        }
        if legacy {
            self.apply_legacy_remap();
        }
        let result = self.post_load();
        self.cluster_hash_format_version = 2.0;
        self.legacy_map = None;
        self.legacy_reverse = None;
        result
    }

    /// `BuildClusterJewelGraphs()` after an edit: the subgraphs removed,
    /// keeping their allocated nodes to allocate again, and built anew
    pub fn rebuild_cluster_jewel_graphs(&mut self, tab: &ItemsTab, data: &ItemData) -> Result<()> {
        for (_, ids) in self.sub_graphs.iter() {
            for &id in ids {
                if self.index.remove(&id).is_some() {
                    self.order.remove(id);
                    if self.alloc_nodes.contains(id) {
                        self.alloc_nodes.remove(id);
                        self.alloc_subgraph_nodes.push(id);
                    }
                }
            }
        }
        let base = self
            .nodes
            .iter()
            .position(|n| n.synth.is_some())
            .unwrap_or(self.nodes.len());
        self.nodes.truncate(base);
        for n in &mut self.nodes {
            n.linked.retain(|&j| j < base);
        }
        self.sub_graphs.clear();
        self.build_cluster_jewel_graphs(tab, data)
    }

    /// `ApplyLegacyClusterNodeRemap()`
    fn apply_legacy_remap(&mut self) {
        let Some(map) = self.legacy_map.clone() else {
            return;
        };
        let mapped = |s: &Self, id: u32| -> u32 {
            match map.iter().find(|(k, _)| *k == id) {
                Some((_, c)) if s.index.contains_key(c) => *c,
                _ => id,
            }
        };
        let mut converted = Vec::new();
        for &id in &self.alloc_subgraph_nodes {
            let id = mapped(self, id);
            if !converted.contains(&id) {
                converted.push(id);
            }
        }
        self.alloc_subgraph_nodes = converted;
        for (legacy, current) in &map {
            if legacy != current
                && self.alloc_nodes.contains(*legacy)
                && self.index.contains_key(current)
            {
                if let Some(&i) = self.index.get(legacy) {
                    self.nodes[i].alloc = false;
                }
                self.alloc_nodes.remove(*legacy);
                if !self.alloc_subgraph_nodes.contains(current) {
                    self.alloc_subgraph_nodes.push(*current);
                }
            }
        }
        self.jewels = std::mem::take(&mut self.jewels)
            .into_iter()
            .map(|(k, v)| (mapped(self, k), v))
            .collect();
    }

    /// `BuildSubgraph(jewel, parentSocket, id, upSize)`
    fn build_subgraph(
        &mut self,
        b: &mut Builder,
        tab: &ItemsTab,
        data: &ItemData,
        jewel: &Item,
        parent: Parent,
        id: Option<u32>,
    ) -> Result<()> {
        let tree = self.tree;
        let parent_node: Node = match parent {
            Parent::Spec(i) => self.tree_node(i).clone(),
            Parent::Pending(id) => b.pending[b.by_id[&id]].node.clone(),
        };
        let ej = parent_node
            .expansion_jewel
            .clone()
            .ok_or("a cluster socket without its expansion jewel")?;
        let base = jewel.base_name.clone().unwrap_or_default();
        let cj: &ClusterJewel = data
            .cluster_jewels
            .get(&base)
            .ok_or_else(|| format!("no cluster jewel data for {base}"))?;
        let mut id = id.unwrap_or(0x10000);
        if ej.size == 2.0 {
            id += (ej.index as u32) << 6;
        } else if ej.size == 1.0 {
            id += (ej.index as u32) << 9;
        }
        let size_index = cj.size_index as i64;
        let node_id = id + ((size_index as u32) << 4);
        self.sub_graphs.insert(node_id, Vec::new());
        let proxy_node = ej
            .proxy
            .as_deref()
            .and_then(crate::pat::to_number)
            .and_then(|p| tree.node(p as u32))
            .ok_or("Proxy node not found")?;
        let proxy_group = proxy_node.group.ok_or("a proxy without a group")?;

        let mut nodes: Vec<Synth> = Vec::new();
        let link = |nodes: &mut Vec<Synth>, a: usize, c: usize| {
            let (ia, ic) = (nodes[a].id, nodes[c].id);
            nodes[a].linked.push(ic);
            nodes[c].linked.push(ia);
        };
        if let Some(name) = field(jewel, "clusterJewelKeystone").and_then(|v| match v {
            ModValue::Str(s) => Some(s.to_string()),
            _ => None,
        }) {
            let kn = tree
                .cluster_node_map
                .get(name.as_str())
                .and_then(|&id| tree.node(id))
                .ok_or_else(|| format!("Keystone node not found: {name}"))?;
            nodes.push(Synth {
                kind: NodeType::Keystone,
                id: node_id,
                dn: kn.dn.clone(),
                sd: kn.sd.clone(),
                expansion: None,
                orbit: 0,
                oidx: 1,
                linked: vec![],
            });
            return self.add_subgraph(b, tab, data, jewel, parent, nodes, 0, (id, node_id));
        }
        let legacy_group = self
            .legacy_map
            .is_some()
            .then(|| self.legacy_proxy_group(proxy_group, ej.size, cj.size_index));
        let node_orbit = size_index + 1;

        // The notables, in their sort order
        let mut notables: Vec<&Node> = Vec::new();
        for name in strings(field(jewel, "clusterJewelNotables")) {
            let Some(n) = tree
                .cluster_node_map
                .get(name.as_str())
                .and_then(|&id| tree.node(id))
            else {
                // Old trees miss some notables: no subgraph
                self.sub_graphs.remove(node_id);
                return Ok(());
            };
            notables.push(n);
        }
        let order = |n: &Node| {
            data.cluster_notable_sort_order
                .get(&*n.dn)
                .copied()
                .unwrap_or(f64::NAN)
        };
        notables.sort_by(|a, c| order(a).total_cmp(&order(c)));
        let skill = field(jewel, "clusterJewelSkill")
            .and_then(|v| match v {
                ModValue::Str(s) => cj.skill_data.get(&**s),
                _ => None,
            })
            .cloned()
            .unwrap_or(crate::item_data::ClusterSkill {
                name: "Nothingness".into(),
                stats: vec![],
                mastery: false,
            });
        let socket_count = field_num(jewel, "clusterJewelSocketCountOverride")
            .or(field_num(jewel, "clusterJewelSocketCount"))
            .unwrap_or(0.0) as i64;
        let notable_count = notables.len() as i64;
        let node_count = match field_num(jewel, "clusterJewelNodeCount") {
            Some(n) => n as i64,
            None => {
                socket_count
                    + notable_count
                    + field_num(jewel, "clusterJewelNothingnessCount").unwrap_or(0.0) as i64
            }
        };
        let small_count = node_count - socket_count - notable_count;
        if skill.mastery {
            nodes.push(Synth {
                kind: NodeType::Mastery,
                id: node_id + 12,
                dn: "Nothingness".into(),
                sd: vec![],
                expansion: None,
                orbit: 0,
                oidx: 0,
                linked: vec![],
            });
        }
        let mut indicies: IntTable<usize> = IntTable::default();
        // First pass: sockets
        let mut make_jewel =
            |s: &mut Self, nodes: &mut Vec<Synth>, index: i64, jewel_index: f64| -> Result<()> {
                let socket = s
                    .find_cluster_socket(proxy_group, jewel_index)
                    .ok_or("Socket not found (ran out of sockets nani?)")?;
                nodes.push(Synth {
                    kind: NodeType::Socket,
                    id: socket.id,
                    dn: socket.dn.clone(),
                    sd: vec![],
                    expansion: socket.expansion_jewel.clone(),
                    orbit: node_orbit,
                    oidx: index,
                    linked: vec![],
                });
                indicies.set(index, nodes.len() - 1);
                if let Some(g) = legacy_group
                    && let Some(legacy) = s.find_cluster_socket(g, jewel_index)
                {
                    s.register_legacy(legacy.id, socket.id);
                }
                Ok(())
            };
        if cj.size == "Large" && socket_count == 1 {
            make_jewel(self, &mut nodes, 6, 1.0)?;
        } else {
            if socket_count as usize > cj.socket_indicies.len() {
                return Err("Too many sockets!".into());
            }
            let get_jewels = [0.0, 2.0, 1.0];
            for (&index, &jewel_index) in cj
                .socket_indicies
                .iter()
                .zip(&get_jewels)
                .take(socket_count as usize)
            {
                make_jewel(self, &mut nodes, index, jewel_index)?;
            }
        }
        // Second pass: notables
        let mut notable_indices: Vec<i64> = Vec::new();
        for &ni in &cj.notable_indicies {
            if notable_indices.len() as i64 == notable_count {
                break;
            }
            let mut ni = ni;
            if cj.size == "Medium" {
                if socket_count == 0 && notable_count == 2 {
                    ni = match ni {
                        6 => 4,
                        10 => 8,
                        x => x,
                    };
                } else if node_count == 4 {
                    ni = match ni {
                        10 => 9,
                        2 => 3,
                        x => x,
                    };
                }
            }
            if indicies.get(ni).is_none() {
                notable_indices.push(ni);
            }
        }
        notable_indices.sort();
        for (k, base) in notables.iter().enumerate() {
            let Some(&ni) = notable_indices.get(k) else {
                break;
            };
            nodes.push(Synth {
                kind: NodeType::Notable,
                id: node_id + ni as u32,
                dn: base.dn.clone(),
                sd: base.sd.clone(),
                expansion: None,
                orbit: node_orbit,
                oidx: ni,
                linked: vec![],
            });
            indicies.set(ni, nodes.len() - 1);
        }
        // Third pass: small nodes
        let mut small_indices: Vec<i64> = Vec::new();
        for &ni in &cj.small_indicies {
            if small_indices.len() as i64 == small_count {
                break;
            }
            let mut ni = ni;
            if cj.size == "Medium" {
                if node_count == 5 && ni == 4 {
                    ni = 3;
                } else if node_count == 4 {
                    ni = match ni {
                        8 => 9,
                        4 => 3,
                        x => x,
                    };
                }
            }
            if indicies.get(ni).is_none() {
                small_indices.push(ni);
            }
        }
        let added = strings(field(jewel, "clusterJewelAddedMods"));
        for k in 0..small_count.max(0) as usize {
            let Some(&ni) = small_indices.get(k) else {
                break;
            };
            let mut sd: Vec<Arc<str>> = skill.stats.iter().map(|s| s.as_str().into()).collect();
            sd.extend(added.iter().map(|s| s.as_str().into()));
            nodes.push(Synth {
                kind: NodeType::Normal,
                id: node_id + ni as u32,
                dn: skill.name.as_str().into(),
                sd,
                expansion: None,
                orbit: node_orbit,
                oidx: ni,
                linked: vec![],
            });
            indicies.set(ni, nodes.len() - 1);
        }
        let entrance = *indicies.get(0).ok_or("No entrance to subgraph")?;

        // Orbit indices in the tree's orbit space
        let total = cj.total_indicies;
        let skills_per_orbit = tree
            .skills_per_orbit
            .get(size_index as usize + 1)
            .copied()
            .unwrap_or(f64::NAN) as i64;
        let start = data
            .cluster_orbit_offsets
            .get(&proxy_node.id)
            .and_then(|o| o.get(&size_index))
            .copied()
            .ok_or("attempt to index a nil value (orbitOffsets)")?;
        for (_, k) in indicies.pairs() {
            let rel = (nodes[k].oidx + start).rem_euclid(total);
            nodes[k].oidx = translate_orbit_index(rel, total, skills_per_orbit);
        }
        // `BuildLegacyClusterOrbitMappings`
        if self.legacy_map.is_some() {
            let legacy_n = tree
                .skills_per_orbit
                .get(proxy_node.orbit.unwrap_or(0) as usize)
                .copied()
                .unwrap_or(f64::NAN) as i64;
            let proxy_rel =
                translate_orbit_index(proxy_node.orbit_index.unwrap_or(0), legacy_n, total);
            let mut legacy_ids: IntTable<u32> = IntTable::default();
            let mut current_ids: IntTable<u32> = IntTable::default();
            for (ni, k) in indicies.pairs() {
                let rel = (ni + proxy_rel).rem_euclid(total);
                legacy_ids.set(translate_orbit_index(rel, total, legacy_n), nodes[k].id);
                let cur = translate_orbit_index(nodes[k].oidx, skills_per_orbit, total);
                current_ids.set(translate_orbit_index(cur, total, legacy_n), nodes[k].id);
            }
            for (oidx, legacy) in legacy_ids.pairs() {
                if let Some(&current) = current_ids.get(oidx)
                    && legacy != current
                {
                    self.register_legacy(legacy, current);
                }
            }
        }
        // Connectors: around the orbit, closed on non-small clusters
        let (mut first, mut last) = (None, None);
        for i in 0..total {
            if let Some(&k) = indicies.get(i) {
                if first.is_none() {
                    first = Some(k);
                }
                if let Some(l) = last {
                    link(&mut nodes, k, l);
                }
                last = Some(k);
            }
        }
        if let (Some(f), Some(l)) = (first, last)
            && f != l
            && cj.size != "Small"
        {
            link(&mut nodes, f, l);
        }
        self.add_subgraph(b, tab, data, jewel, parent, nodes, entrance, (id, node_id))
    }

    /// The end of `BuildSubgraph`: the nodes processed and linked to the
    /// parent socket, added to the spec's nodes, with nested subgraphs
    #[allow(clippy::too_many_arguments)]
    fn add_subgraph(
        &mut self,
        b: &mut Builder,
        tab: &ItemsTab,
        data: &ItemData,
        jewel: &Item,
        parent: Parent,
        mut nodes: Vec<Synth>,
        entrance: usize,
        (id, key): (u32, u32),
    ) -> Result<()> {
        let inc_effect = field_num(jewel, "clusterJewelIncEffect");
        // `linkNodes(subGraph.entranceNode, parentSocket)`
        let entrance_id = nodes[entrance].id;
        if let Some(ids) = self
            .sub_graphs
            .get(key)
            .map(|_| nodes.iter().map(|n| n.id).collect())
        {
            self.sub_graphs.insert(key, ids);
        }
        let parent_id = match parent {
            Parent::Spec(i) => {
                b.spec_links.push((i, entrance_id));
                self.nodes[i].id
            }
            Parent::Pending(pid) => {
                let k = b.by_id[&pid];
                b.pending[k].linked.push(entrance_id);
                pid
            }
        };
        nodes[entrance].linked.push(parent_id);
        for s in nodes {
            let node = self.synthetic_node(&s, inc_effect)?;
            let is_socket = s.kind == NodeType::Socket;
            b.by_id.insert(s.id, b.pending.len());
            b.pending.push(Pending {
                node,
                linked: s.linked,
            });
            if is_socket
                && (self.alloc_nodes.contains(s.id) || self.alloc_subgraph_nodes.contains(&s.id))
                && let Some(j) = self.socketed_jewel(tab, s.id)
                && field(j, "clusterJewelValid").is_some_and(ModValue::truthy)
            {
                self.build_subgraph(b, tab, data, j, Parent::Pending(s.id), Some(id))?;
            }
        }
        Ok(())
    }

    /// A subgraph node with its stats processed (`tree:ProcessNode(node)`)
    fn synthetic_node(&self, s: &Synth, inc_effect: Option<f64>) -> Result<Node> {
        let mut sd = s.sd.clone();
        let source = format!("Tree:{}", s.id);
        let (mods, mut mod_list, unknown, extra) =
            crate::tree::process_stats(self.parser, &mut sd, Some(&source))?;
        if s.kind == NodeType::Normal
            && let Some(e) = inc_effect
        {
            mod_list.push(Mod::new("PassiveSkillEffect", ModType::Inc, e, None));
        }
        let keystone_mod = (s.kind == NodeType::Keystone).then(|| {
            Mod::new(
                "Keystone",
                ModType::List,
                ModValue::Str(s.dn.clone()),
                Some(&format!("Tree{}", s.id)),
            )
        });
        Ok(Node {
            id: s.id,
            dn: s.dn.clone(),
            sd,
            kind: s.kind,
            ascendancy_name: None,
            class_start_index: None,
            group: None,
            orbit: Some(s.orbit),
            orbit_index: Some(s.oidx),
            angle: None,
            x: None,
            y: None,
            linked_id: vec![],
            passive_points_granted: 0.0,
            is_proxy: false,
            is_blighted: false,
            is_mastery: s.kind == NodeType::Mastery,
            is_multiple_choice: false,
            is_multiple_choice_option: false,
            is_ascendancy_start: false,
            charm_socket: false,
            expansion_jewel: s.expansion.clone(),
            mastery_effects: vec![],
            mods,
            mod_list,
            keystone_mod,
            unknown,
            extra,
            nodes_in_radius: None,
        })
    }
}
