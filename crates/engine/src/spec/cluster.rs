//! Cluster jewel subgraphs: `PassiveSpecClass:BuildClusterJewelGraphs` and
//! `BuildSubgraph`, with the conversion of legacy (version 1) cluster node
//! hashes, as loading a build runs them.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.
//!
//! Upstream visits some of its tables with `pairs`; where the order decides
//! a result (which legacy node maps to which), [`LuaInts`] keeps the order
//! the translated program's tables have. Nodes imported from a character's
//! `jewel_data` (`addToAllocatedSubgraphNodes`) are not ported: builds
//! loaded from XML have none.

use super::{PassiveSpec, Result, SpecNode};
use crate::item::{Fields, Item};
use crate::item_data::{ClusterJewel, ItemData};
use crate::items_tab::ItemsTab;
use crate::mods::{Mod, ModType, ModValue, Sym};
use crate::tree::{self, NodeType};
use std::rc::Rc;

/// A Lua table with integer keys, in the order `pairs` visits the
/// translated program's tables: the array part (`1..n`), then the other
/// keys in the order first set
#[derive(Clone, Debug, Default)]
pub(super) struct LuaInts<V> {
    array: Vec<V>,
    hash: Vec<(i64, V)>,
}

impl<V: Clone> LuaInts<V> {
    pub(super) fn get(&self, k: i64) -> Option<&V> {
        if k >= 1 && (k as usize) <= self.array.len() {
            return Some(&self.array[k as usize - 1]);
        }
        self.hash.iter().find(|(h, _)| *h == k).map(|(_, v)| v)
    }

    pub(super) fn set(&mut self, k: i64, v: V) {
        if k >= 1 && (k as usize) <= self.array.len() {
            self.array[k as usize - 1] = v;
            return;
        }
        if k >= 1 && k as usize == self.array.len() + 1 {
            self.hash.retain(|(h, _)| *h != k);
            self.array.push(v);
            // Following keys move from the hash part to the array part
            loop {
                let next = self.array.len() as i64 + 1;
                match self.hash.iter().position(|(h, _)| *h == next) {
                    Some(p) => {
                        let (_, v) = self.hash.remove(p);
                        self.array.push(v);
                    }
                    None => break,
                }
            }
            return;
        }
        match self.hash.iter_mut().find(|(h, _)| *h == k) {
            Some(slot) => slot.1 = v,
            None => self.hash.push((k, v)),
        }
    }

    pub(super) fn pairs(&self) -> Vec<(i64, V)> {
        let mut out: Vec<(i64, V)> = self
            .array
            .iter()
            .enumerate()
            .map(|(i, v)| (i as i64 + 1, v.clone()))
            .collect();
        out.extend(self.hash.iter().cloned());
        out
    }
}

/// `legacyClusterNodeMap` and its reverse, while legacy hashes convert
#[derive(Clone, Debug, Default)]
pub(super) struct LegacyMap {
    forward: LuaInts<u32>,
    reverse: LuaInts<u32>,
}

/// A node of a subgraph before it is processed
struct Proto {
    kind: NodeType,
    id: u32,
    dn: Rc<str>,
    sd: Vec<Rc<str>>,
    orbit: i64,
    oidx: i64,
    expansion_jewel: Option<tree::ExpansionJewel>,
}

/// `TranslateClusterOrbitIndex(srcOidx, srcNodesPerOrbit,
/// destNodesPerOrbit)`; `None` where upstream's tables have no entry
fn translate_orbit_index(src: i64, src_n: i64, dest_n: i64) -> Option<i64> {
    let table: &[i64] = match (src_n, dest_n) {
        _ if src_n == dest_n => return Some(src),
        (12, 16) => &[0, 1, 3, 4, 5, 7, 8, 9, 11, 12, 13, 15],
        (16, 12) => &[0, 1, 1, 2, 3, 4, 4, 5, 6, 7, 7, 8, 9, 10, 10, 11],
        (6, 16) => &[0, 3, 5, 8, 11, 13],
        (16, 6) => &[0, 0, 0, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 5, 5, 5],
        _ => return Some((src as f64 * dest_n as f64 / src_n as f64).floor() as i64),
    };
    usize::try_from(src)
        .ok()
        .and_then(|i| table.get(i))
        .copied()
}

fn text_list(jd: &Fields, key: &str) -> Vec<Rc<str>> {
    match jd.get(key) {
        Some(ModValue::List(l)) => l
            .iter()
            .filter_map(|v| match v {
                ModValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn num(jd: &Fields, key: &str) -> Option<f64> {
    jd.get(key).and_then(ModValue::as_number)
}

impl<'t> PassiveSpec<'t> {
    /// `GetSocketedJewel(nodeId)`: the jewel item in a socket
    fn socketed_jewel<'i>(&self, items: &'i ItemsTab, node_id: u32) -> Option<&'i Item> {
        let mut item_id = self.jewels.get(&node_id).copied();
        if item_id.is_none_or(|i| i == 0.0)
            && let Some(legacy) = &self.legacy_cluster_map
            && let Some(&old) = legacy.reverse.get(node_id as i64)
        {
            item_id = self.jewels.get(&old).copied();
        }
        let item_id = item_id.filter(|&i| i != 0.0)?;
        items
            .items
            .get(&(item_id as i64))
            .filter(|i| i.jewel_data.is_some())
    }

    /// The cluster jewel data of a socketed jewel with a valid cluster
    fn valid_cluster<'i>(
        &self,
        items: &'i ItemsTab,
        data: &'i ItemData,
        node_id: u32,
    ) -> Option<(&'i ClusterJewel, &'i Fields)> {
        let item = self.socketed_jewel(items, node_id)?;
        let jd = item.jewel_data.as_ref()?;
        if !jd.get("clusterJewelValid").is_some_and(ModValue::truthy) {
            return None;
        }
        Some((data.cluster_jewels.get(item.base_name.as_ref()?)?, jd))
    }

    /// `RegisterLegacyClusterNodeMap(legacyNodeId, currentNodeId)`
    fn register_legacy(&mut self, legacy: u32, current: u32) {
        if let Some(map) = &mut self.legacy_cluster_map {
            map.forward.set(legacy as i64, current);
            map.reverse.set(current as i64, legacy);
        }
    }

    /// `GetMappedClusterNodeId(nodeId)`
    fn mapped_cluster_node_id(&self, id: u32) -> u32 {
        match &self.legacy_cluster_map {
            Some(map) => match map.forward.get(id as i64) {
                Some(&m) if self.index.contains_key(&m) => m,
                _ => id,
            },
            None => id,
        }
    }

    /// `FindClusterSocket(group, index)`
    fn find_cluster_socket(&self, group: i64, index: f64) -> Result<Option<&'t tree::Node>> {
        let tree = self.tree;
        let Some(g) = tree.groups.get(&group) else {
            return Ok(None);
        };
        for id in &g.nodes {
            let node = tree
                .node(*id)
                .ok_or_else(|| format!("group {group} lists no node {id}"))?;
            if node
                .expansion_jewel
                .as_ref()
                .is_some_and(|e| e.index == index)
            {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    /// `BuildLegacyProxyGroup(proxyGroup, expansionJewelSize,
    /// clusterSizeIndex)`
    fn legacy_proxy_group(&self, proxy_group: i64, size: f64, size_index: f64) -> Result<i64> {
        let mut group = proxy_group;
        let mut group_size = size;
        let mut guard = 0;
        while size_index < group_size && guard < 4 {
            let socket = match self.find_cluster_socket(group, 1.0)? {
                Some(s) => Some(s),
                None => self.find_cluster_socket(group, 0.0)?,
            };
            let Some(socket) = socket else { break };
            let ej = socket.expansion_jewel.as_ref().unwrap();
            let proxy = ej
                .proxy
                .as_deref()
                .and_then(|p| p.parse::<u32>().ok())
                .and_then(|p| self.tree.node(p));
            let Some(g) = proxy.and_then(|p| p.group) else {
                break;
            };
            group = g;
            group_size = ej.size;
            guard += 1;
        }
        Ok(group)
    }

    /// `BeginLegacyClusterHashConversion`, the subgraphs of every large
    /// socket with a valid cluster jewel, `ApplyLegacyClusterNodeRemap`,
    /// the allocation of subgraph nodes and `BuildAllDependsAndPaths`
    pub(super) fn build_cluster_jewel_graphs(
        &mut self,
        items: &ItemsTab,
        data: &ItemData,
    ) -> Result<()> {
        let legacy = self.cluster_hash_format_version < 2.0;
        self.legacy_cluster_map = legacy.then(LegacyMap::default);
        let tree = self.tree;
        for &socket_id in &tree.sockets {
            let Some(node) = tree.node(socket_id) else {
                continue;
            };
            if !node.expansion_jewel.as_ref().is_some_and(|e| e.size == 2.0) {
                continue;
            }
            let Some((cj, jd)) = self.valid_cluster(items, data, socket_id) else {
                continue;
            };
            let parent = *self
                .index
                .get(&socket_id)
                .ok_or_else(|| format!("no spec node for socket {socket_id}"))?;
            self.build_subgraph(items, data, cj, jd, parent, None)?;
        }
        if legacy {
            self.apply_legacy_cluster_node_remap();
        }
        // (Re-)allocate subgraph nodes
        for id in std::mem::take(&mut self.alloc_subgraph_nodes) {
            if let Some(&i) = self.index.get(&id) {
                self.nodes[i].alloc = true;
                if !self.alloc_nodes.contains(id) {
                    self.alloc_nodes.insert(id, i);
                }
            }
        }
        self.build_all_depends_and_paths()?;
        self.cluster_hash_format_version = 2.0;
        self.legacy_cluster_map = None;
        Ok(())
    }

    /// `ApplyLegacyClusterNodeRemap()`
    fn apply_legacy_cluster_node_remap(&mut self) {
        let Some(map) = self.legacy_cluster_map.clone() else {
            return;
        };
        let mut seen: Vec<u32> = Vec::new();
        let mut converted = Vec::new();
        for id in std::mem::take(&mut self.alloc_subgraph_nodes) {
            let id = self.mapped_cluster_node_id(id);
            if !seen.contains(&id) {
                seen.push(id);
                converted.push(id);
            }
        }
        self.alloc_subgraph_nodes = converted;
        // Legacy socket ids can be the tree's, allocated directly
        for (legacy, current) in map.forward.pairs() {
            let legacy = legacy as u32;
            if legacy == current || !self.index.contains_key(&current) {
                continue;
            }
            if let Some(&i) = self.alloc_nodes.get(legacy) {
                self.nodes[i].alloc = false;
                self.alloc_nodes.remove(legacy);
                if !seen.contains(&current) {
                    seen.push(current);
                    self.alloc_subgraph_nodes.push(current);
                }
            }
        }
        let jewels = std::mem::take(&mut self.jewels);
        for (node, item) in jewels {
            let node = self.mapped_cluster_node_id(node);
            self.jewels.insert(node, item);
        }
    }

    /// Adds a processed subgraph node to the spec
    fn add_subgraph_node(&mut self, node: tree::Node) -> usize {
        let i = self.nodes.len();
        self.index.insert(node.id, i);
        self.nodes.push(SpecNode {
            id: node.id,
            tree: usize::MAX,
            synthetic: Some(Rc::new(node)),
            linked: Vec::new(),
            alloc: false,
            depends: Vec::new(),
            connected_to_start: false,
            path_dist: 0.0,
            path: None,
            distance_to_class_start: None,
            stats: None,
            all_mastery_options: None,
            unknown: false,
            visited: false,
        });
        i
    }

    /// `tree:ProcessNode(node)` for a subgraph node of a group at `(x, y)`
    fn process_subgraph_node(
        &self,
        p: Proto,
        x: f64,
        y: f64,
        inc_effect: Option<f64>,
    ) -> Result<tree::Node> {
        let tree = self.tree;
        let angle = tree
            .orbit_angles
            .get(p.orbit as usize)
            .and_then(|a| usize::try_from(p.oidx).ok().and_then(|i| a.get(i)))
            .copied()
            .ok_or_else(|| {
                format!(
                    "node {}: no angle for orbit {} index {}",
                    p.id, p.orbit, p.oidx
                )
            })?;
        let r = tree.orbit_radii[p.orbit as usize];
        let mut sd = p.sd;
        let source = format!("Tree:{}", p.id);
        let (mods, mut mod_list, unknown, extra) =
            tree::process_stats(self.parser, &mut sd, Some(&source))?;
        let keystone_mod = (p.kind == NodeType::Keystone).then(|| Mod {
            name: Sym::new("Keystone"),
            typ: ModType::List,
            value: ModValue::Str(p.dn.clone()),
            flags: 0,
            keyword_flags: 0,
            source: Some(format!("Tree{}", p.id).into()),
            source_slot: None,
            tags: vec![],
            unreached_tags: vec![],
        });
        if let Some(inc) = inc_effect
            && p.kind == NodeType::Normal
        {
            mod_list.push(Mod {
                name: Sym::new("PassiveSkillEffect"),
                typ: ModType::Inc,
                value: ModValue::Number(inc),
                flags: 0,
                keyword_flags: 0,
                source: None,
                source_slot: None,
                tags: vec![],
                unreached_tags: vec![],
            });
        }
        Ok(tree::Node {
            id: p.id,
            dn: p.dn,
            sd,
            kind: p.kind,
            ascendancy_name: None,
            class_start_index: None,
            group: None,
            orbit: Some(p.orbit),
            orbit_index: Some(p.oidx),
            angle: Some(angle),
            x: Some(x + angle.sin() * r),
            y: Some(y - angle.cos() * r),
            linked_id: Vec::new(),
            passive_points_granted: 0.0,
            is_proxy: false,
            is_blighted: false,
            is_mastery: false,
            is_multiple_choice: false,
            is_multiple_choice_option: false,
            is_ascendancy_start: false,
            charm_socket: false,
            expansion_jewel: p.expansion_jewel,
            mastery_effects: Vec::new(),
            mods,
            mod_list,
            keystone_mod,
            unknown,
            extra,
            nodes_in_radius: None,
        })
    }

    fn link(&mut self, a: usize, b: usize) {
        self.nodes[a].linked.push(b);
        self.nodes[b].linked.push(a);
    }

    /// `BuildSubgraph(jewel, parentSocket, id, upSize, importedNodes,
    /// importedGroups)`
    fn build_subgraph(
        &mut self,
        items: &ItemsTab,
        data: &ItemData,
        cj: &ClusterJewel,
        jd: &Fields,
        parent: usize,
        id: Option<u32>,
    ) -> Result<()> {
        let tree = self.tree;
        let expansion = self
            .tree_node(parent)
            .expansion_jewel
            .clone()
            .ok_or("a subgraph's parent is not a cluster socket")?;
        // Ids: node index in bits 0-3, group size in 4-5, large index in
        // 6-8, medium index in 9-10, and bit 16 set
        let mut id = id.unwrap_or(0x10000);
        if expansion.size == 2.0 {
            id += (expansion.index as u32) << 6;
        } else if expansion.size == 1.0 {
            id += (expansion.index as u32) << 9;
        }
        let node_id = id + ((cj.size_index as u32) << 4);

        let proxy_node = expansion
            .proxy
            .as_deref()
            .and_then(|p| p.parse::<u32>().ok())
            .and_then(|p| tree.node(p))
            .ok_or("Proxy node not found")?;
        let proxy_group = proxy_node.group.ok_or("proxy node without a group")?;
        let (gx, gy) = {
            let g = &tree.groups[&proxy_group];
            (g.x, g.y)
        };

        if let Some(ModValue::Str(keystone)) = jd.get("clusterJewelKeystone") {
            let base = tree
                .cluster_node_map
                .get(keystone)
                .and_then(|&k| tree.node(k))
                .ok_or_else(|| format!("Keystone node not found: {keystone}"))?;
            let p = Proto {
                kind: NodeType::Keystone,
                id: node_id,
                dn: base.dn.clone(),
                sd: base.sd.clone(),
                orbit: 0,
                oidx: 1,
                expansion_jewel: None,
            };
            let node = self.process_subgraph_node(p, gx, gy, None)?;
            let i = self.add_subgraph_node(node);
            self.link(i, parent);
            return Ok(());
        }

        let legacy_group = match self.legacy_cluster_map {
            Some(_) => Some(self.legacy_proxy_group(proxy_group, expansion.size, cj.size_index)?),
            None => None,
        };
        let node_orbit = cj.size_index as i64 + 1;

        // Notables, in the sort order
        let mut notables: Vec<&tree::Node> = Vec::new();
        for name in text_list(jd, "clusterJewelNotables") {
            let Some(base) = tree.cluster_node_map.get(&name).and_then(|&k| tree.node(k)) else {
                // Old trees lack some notables: no subgraph
                return Ok(());
            };
            if !data.cluster_notable_sort_order.contains_key(&*base.dn) {
                return Err(format!("Cluster notable has no sort order: {name}"));
            }
            notables.push(base);
        }
        let order = |n: &tree::Node| data.cluster_notable_sort_order[&*n.dn];
        notables.sort_by(|a, b| order(a).total_cmp(&order(b)));

        let skill = match jd.get("clusterJewelSkill") {
            Some(ModValue::Str(s)) => cj.skills.get(&**s),
            _ => None,
        };
        let (skill_name, skill_stats, mastery): (Rc<str>, Vec<Rc<str>>, bool) = match skill {
            Some(s) => (s.name.clone(), s.stats.clone(), s.has_mastery_icon),
            None => ("Nothingness".into(), Vec::new(), false),
        };
        let socket_count = num(jd, "clusterJewelSocketCountOverride")
            .or_else(|| num(jd, "clusterJewelSocketCount"))
            .unwrap_or(0.0) as i64;
        let notable_count = notables.len() as i64;
        let node_count = num(jd, "clusterJewelNodeCount")
            .map(|n| n as i64)
            .unwrap_or_else(|| {
                socket_count
                    + notable_count
                    + num(jd, "clusterJewelNothingnessCount").unwrap_or(0.0) as i64
            });
        let small_count = node_count - socket_count - notable_count;

        let mut protos: Vec<Proto> = Vec::new();
        if mastery {
            protos.push(Proto {
                kind: NodeType::Mastery,
                id: node_id + 12,
                dn: "Nothingness".into(),
                sd: Vec::new(),
                orbit: 0,
                oidx: 0,
                expansion_jewel: None,
            });
        }
        let mut indicies: LuaInts<usize> = LuaInts::default();

        // First pass: sockets
        let mut make_jewel = |spec: &mut Self,
                              protos: &mut Vec<Proto>,
                              node_index: i64,
                              jewel_index: f64|
         -> Result<()> {
            let socket = spec
                .find_cluster_socket(proxy_group, jewel_index)?
                .ok_or("Socket not found (ran out of sockets nani?)")?;
            indicies.set(node_index, protos.len());
            protos.push(Proto {
                kind: NodeType::Socket,
                id: socket.id,
                dn: socket.dn.clone(),
                sd: Vec::new(),
                orbit: node_orbit,
                oidx: node_index,
                expansion_jewel: socket.expansion_jewel.clone(),
            });
            if let Some(g) = legacy_group
                && let Some(old) = spec.find_cluster_socket(g, jewel_index)?
            {
                spec.register_legacy(old.id, socket.id);
            }
            Ok(())
        };
        if cj.size == "Large" && socket_count == 1 {
            // Large clusters always have the single jewel at index 6
            make_jewel(self, &mut protos, 6, 1.0)?;
        } else {
            if socket_count > cj.socket_indicies.len() as i64 {
                return Err("Too many sockets!".into());
            }
            let get_jewels = [0.0, 2.0, 1.0];
            for (k, &index) in cj
                .socket_indicies
                .iter()
                .take(socket_count as usize)
                .enumerate()
            {
                let jewel = *get_jewels
                    .get(k)
                    .ok_or("Socket not found (ran out of sockets nani?)")?;
                make_jewel(self, &mut protos, index, jewel)?;
            }
        }

        // Second pass: notables
        let mut notable_indices: Vec<i64> = Vec::new();
        for &index in &cj.notable_indicies {
            if notable_indices.len() as i64 == notable_count {
                break;
            }
            let mut index = index;
            if cj.size == "Medium" {
                if socket_count == 0 && notable_count == 2 {
                    // Two notables in a Medium cluster
                    index = match index {
                        6 => 4,
                        10 => 8,
                        x => x,
                    };
                } else if node_count == 4 {
                    // Notables in a 4-node Medium cluster
                    index = match index {
                        10 => 9,
                        2 => 3,
                        x => x,
                    };
                }
            }
            if indicies.get(index).is_none() {
                notable_indices.push(index);
            }
        }
        notable_indices.sort();
        for (k, base) in notables.iter().enumerate() {
            // Jewels with more notables than allowed lose the rest
            let Some(&index) = notable_indices.get(k) else {
                break;
            };
            indicies.set(index, protos.len());
            protos.push(Proto {
                kind: NodeType::Notable,
                id: node_id + index as u32,
                dn: base.dn.clone(),
                sd: base.sd.clone(),
                orbit: node_orbit,
                oidx: index,
                expansion_jewel: None,
            });
        }

        // Third pass: small nodes
        let mut small_indices: Vec<i64> = Vec::new();
        for &index in &cj.small_indicies {
            if small_indices.len() as i64 == small_count {
                break;
            }
            let mut index = index;
            if cj.size == "Medium" {
                if node_count == 5 && index == 4 {
                    index = 3;
                } else if node_count == 4 {
                    index = match index {
                        8 => 9,
                        4 => 3,
                        x => x,
                    };
                }
            }
            if indicies.get(index).is_none() {
                small_indices.push(index);
            }
        }
        let added = text_list(jd, "clusterJewelAddedMods");
        for k in 0..small_count.max(0) as usize {
            let Some(&index) = small_indices.get(k) else {
                break;
            };
            let mut sd = skill_stats.clone();
            sd.extend(added.iter().cloned());
            indicies.set(index, protos.len());
            protos.push(Proto {
                kind: NodeType::Normal,
                id: node_id + index as u32,
                dn: skill_name.clone(),
                sd,
                orbit: node_orbit,
                oidx: index,
                expansion_jewel: None,
            });
        }
        let entrance = *indicies.get(0).ok_or("No entrance to subgraph")?;

        // From the cluster's template index space into the tree's orbit
        // index space
        let skills_per_orbit = tree.skills_per_orbit[cj.size_index as usize + 1] as i64;
        let start = data
            .cluster_orbit_offsets
            .get(&proxy_node.id)
            .and_then(|o| o.get(&(cj.size_index as i64)))
            .copied()
            .ok_or_else(|| format!("no orbit offset for proxy {}", proxy_node.id))?;
        let total = cj.total_indicies;
        for (_, p) in indicies.pairs() {
            let rel = (protos[p].oidx + start).rem_euclid(total);
            protos[p].oidx = translate_orbit_index(rel, total, skills_per_orbit)
                .ok_or("a cluster node off its orbit")?;
        }
        // `BuildLegacyClusterOrbitMappings`
        if self.legacy_cluster_map.is_some() {
            let legacy_spo = tree.skills_per_orbit[proxy_node.orbit.unwrap_or(0) as usize] as i64;
            let proxy_rel =
                translate_orbit_index(proxy_node.orbit_index.unwrap_or(0), legacy_spo, total);
            let mut legacy_by_oidx: LuaInts<u32> = LuaInts::default();
            let mut current_by_oidx: LuaInts<u32> = LuaInts::default();
            for (index, p) in indicies.pairs() {
                if let Some(proxy_rel) = proxy_rel {
                    let rel = (index + proxy_rel).rem_euclid(total);
                    if let Some(o) = translate_orbit_index(rel, total, legacy_spo) {
                        legacy_by_oidx.set(o, protos[p].id);
                    }
                }
                let cur = translate_orbit_index(protos[p].oidx, skills_per_orbit, total)
                    .and_then(|r| translate_orbit_index(r, total, legacy_spo));
                if let Some(o) = cur {
                    current_by_oidx.set(o, protos[p].id);
                }
            }
            for (oidx, legacy) in legacy_by_oidx.pairs() {
                if let Some(&current) = current_by_oidx.get(oidx)
                    && legacy != current
                {
                    self.register_legacy(legacy, current);
                }
            }
        }

        // Process and add the nodes, then link them
        let inc_effect = num(jd, "clusterJewelIncEffect");
        let mut spec_index = Vec::with_capacity(protos.len());
        for p in protos {
            let node = self.process_subgraph_node(p, gx, gy, inc_effect)?;
            spec_index.push(self.add_subgraph_node(node));
        }
        let (mut first, mut last) = (None, None);
        for i in 0..total {
            if let Some(&p) = indicies.get(i) {
                let this = spec_index[p];
                if first.is_none() {
                    first = Some(this);
                }
                if let Some(l) = last {
                    self.link(this, l);
                }
                last = Some(this);
            }
        }
        if let (Some(f), Some(l)) = (first, last)
            && f != l
            && cj.size != "Small"
        {
            // Close the loop on non-small clusters
            self.link(f, l);
        }
        self.link(spec_index[entrance], parent);

        // Smaller jewels in the sockets
        for &i in &spec_index {
            if self.tree_node(i).kind != NodeType::Socket {
                continue;
            }
            let sid = self.nodes[i].id;
            if let Some((cj, jd)) = self.valid_cluster(items, data, sid) {
                self.build_subgraph(items, data, cj, jd, i, Some(id))?;
            }
        }
        Ok(())
    }
}
