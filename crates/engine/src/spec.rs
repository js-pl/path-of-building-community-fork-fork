//! A passive tree allocation: upstream's `PassiveSpec` class
//! (`Classes/PassiveSpec.lua`): class and ascendancy, allocated nodes,
//! mastery selections, and the dependencies and paths
//! `BuildAllDependsAndPaths` derives from them.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.
//!
//! Not ported yet: jewels that change the tree (Intuitive Leap and the like,
//! timeless jewels, Impossible Escape, Split Personality), tattoos, cluster
//! jewel subgraphs, tree conversion and URLs.
//!
//! Upstream keeps the allocated nodes and mastery selections in tables keyed
//! by node id and visits them with `pairs`; which paths win ties and the
//! order of dependency lists follow that order. [`OrderedMap`] keeps it as
//! the translated program's tables do (by when a key was first set), so
//! results compare with it exactly.

use crate::mod_parser::ModParser;
use crate::mods::{Mod, Sym};
use crate::tree::{self, NodeType, PassiveTree, StatLine};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

/// A map in the order its keys were first set: a removed key keeps its
/// place, and setting it again puts it back there.
#[derive(Clone, Debug, Default)]
pub struct OrderedMap<V> {
    slots: Vec<Option<(u32, V)>>,
    index: HashMap<u32, usize>,
}

impl<V> OrderedMap<V> {
    pub fn get(&self, k: u32) -> Option<&V> {
        let &i = self.index.get(&k)?;
        self.slots[i].as_ref().map(|(_, v)| v)
    }

    pub fn contains(&self, k: u32) -> bool {
        self.get(k).is_some()
    }

    pub fn insert(&mut self, k: u32, v: V) {
        match self.index.get(&k) {
            Some(&i) => self.slots[i] = Some((k, v)),
            None => {
                self.index.insert(k, self.slots.len());
                self.slots.push(Some((k, v)));
            }
        }
    }

    pub fn remove(&mut self, k: u32) {
        if let Some(&i) = self.index.get(&k) {
            self.slots[i] = None;
        }
    }

    /// Removes every entry (`wipeTable`); keys keep their places.
    pub fn clear(&mut self) {
        for s in &mut self.slots {
            *s = None;
        }
    }

    /// The key at place `i`, if set: visiting places in order while removing
    /// entries is how upstream's `pairs` loops behave.
    pub fn at(&self, i: usize) -> Option<(u32, &V)> {
        self.slots.get(i)?.as_ref().map(|(k, v)| (*k, v))
    }

    pub fn places(&self) -> usize {
        self.slots.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &V)> {
        self.slots.iter().flatten().map(|(k, v)| (*k, v))
    }

    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A path to the nearest allocated node, nearest first; shared, as each
/// path is the path of the node it extends plus one node.
#[derive(Debug)]
pub struct PathStep {
    pub node: usize,
    pub rest: Option<Rc<PathStep>>,
}

/// A node's stat lines and mods when the spec changed them (a mastery's
/// selected effect, or all its effects).
#[derive(Clone, Debug, PartialEq)]
pub struct NodeStats {
    pub sd: Vec<Rc<str>>,
    pub mods: Vec<StatLine>,
    pub mod_list: Vec<Mod>,
}

#[derive(Clone, Debug)]
pub struct SpecNode {
    pub id: u32,
    /// Index of the node in the tree
    pub tree: usize,
    /// Linked nodes of the spec, by index
    pub linked: Vec<usize>,
    pub alloc: bool,
    /// Nodes connected to the tree only through this one, itself first
    pub depends: Vec<usize>,
    pub connected_to_start: bool,
    pub path_dist: f64,
    /// `node.path`: `None` when nil, `Some(None)` for a root's empty path
    pub path: Option<Option<Rc<PathStep>>>,
    pub distance_to_class_start: Option<f64>,
    /// Stats the spec set, instead of the tree's
    pub stats: Option<NodeStats>,
    pub all_mastery_options: Option<bool>,
    /// Set when processing the spec's stats found an unknown line
    pub unknown: bool,
    visited: bool,
}

pub struct PassiveSpec<'t> {
    pub tree: &'t PassiveTree,
    parser: &'t ModParser,
    pub nodes: Vec<SpecNode>,
    pub index: HashMap<u32, usize>,
    /// Allocated nodes, by id
    pub alloc_nodes: OrderedMap<usize>,
    /// Selected mastery effect by mastery node id
    pub mastery_selections: OrderedMap<f64>,
    /// Allocated node ids the spec does not have (in cluster subgraphs)
    pub alloc_subgraph_nodes: Vec<u32>,
    pub cur_class_id: Option<i64>,
    pub cur_ascend_class_id: Option<i64>,
    pub cur_secondary_ascend_class_id: Option<i64>,
    pub allocated_mastery_count: u32,
    pub allocated_notable_count: u32,
    pub allocated_keystone_count: u32,
    /// Allocated masteries by mastery name, in the order first counted
    pub allocated_mastery_types: Vec<(Rc<str>, u32)>,
    pub allocated_mastery_type_count: u32,
    /// Jewel socket node id to the id of the item in it
    pub jewels: BTreeMap<u32, f64>,
    pub title: Option<String>,
}

impl<'t> PassiveSpec<'t> {
    /// A spec on `tree` with class 0 selected, as upstream's constructor
    /// leaves it.
    pub fn new(tree: &'t PassiveTree, parser: &'t ModParser) -> Result<PassiveSpec<'t>> {
        let mut spec = PassiveSpec {
            tree,
            parser,
            nodes: Vec::new(),
            index: HashMap::new(),
            alloc_nodes: OrderedMap::default(),
            mastery_selections: OrderedMap::default(),
            alloc_subgraph_nodes: Vec::new(),
            cur_class_id: None,
            cur_ascend_class_id: None,
            cur_secondary_ascend_class_id: None,
            allocated_mastery_count: 0,
            allocated_notable_count: 0,
            allocated_keystone_count: 0,
            allocated_mastery_types: Vec::new(),
            allocated_mastery_type_count: 0,
            jewels: BTreeMap::new(),
            title: None,
        };
        // A copy of the tree without proxies, groupless nodes and nested
        // cluster sockets
        for (ti, n) in tree.nodes.iter().enumerate() {
            let group_proxy = n.group.is_some_and(|g| tree.groups[&g].is_proxy);
            let nested = n
                .expansion_jewel
                .as_ref()
                .is_some_and(|e| e.parent.is_some());
            if n.group.is_some() && !n.is_proxy && !group_proxy && !nested {
                spec.index.insert(n.id, spec.nodes.len());
                spec.nodes.push(SpecNode {
                    id: n.id,
                    tree: ti,
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
            }
        }
        for i in 0..spec.nodes.len() {
            let tn = &tree.nodes[spec.nodes[i].tree];
            let linked: Vec<usize> = tn
                .linked_id
                .iter()
                .filter_map(|id| spec.index.get(id).copied())
                .collect();
            spec.nodes[i].linked = linked;
        }
        spec.select_class(0)?;
        Ok(spec)
    }

    pub fn tree_node(&self, i: usize) -> &'t tree::Node {
        &self.tree.nodes[self.nodes[i].tree]
    }

    fn kind(&self, i: usize) -> NodeType {
        self.tree_node(i).kind
    }

    fn is_start(&self, i: usize) -> bool {
        matches!(
            self.kind(i),
            NodeType::ClassStart | NodeType::AscendClassStart
        )
    }

    /// The node's mods: the spec's, or the tree's
    pub fn mod_list(&self, i: usize) -> &[Mod] {
        match &self.nodes[i].stats {
            Some(s) => &s.mod_list,
            None => &self.tree_node(i).mod_list,
        }
    }

    pub fn sd(&self, i: usize) -> &[Rc<str>] {
        match &self.nodes[i].stats {
            Some(s) => &s.sd,
            None => &self.tree_node(i).sd,
        }
    }

    fn alloc(&mut self, i: usize) {
        self.nodes[i].alloc = true;
        self.alloc_nodes.insert(self.nodes[i].id, i);
    }

    fn dealloc(&mut self, i: usize) {
        self.nodes[i].alloc = false;
        self.alloc_nodes.remove(self.nodes[i].id);
    }

    fn class(&self, id: i64) -> Result<&'t tree::Class> {
        self.tree
            .classes
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, c)| c)
            .ok_or_else(|| format!("no class {id}"))
    }

    fn start_index(&self, id: Option<u32>) -> Option<usize> {
        id.and_then(|id| self.index.get(&id).copied())
    }

    /// `SelectClass(classId)`
    pub fn select_class(&mut self, class_id: i64) -> Result<()> {
        if let Some(old) = self.cur_class_id {
            let start = self.class(old)?.start_node_id;
            if let Some(i) = self.start_index(start) {
                self.dealloc(i);
            }
        }
        self.reset_ascend_class()?;
        self.cur_class_id = Some(class_id);
        let start = self.class(class_id)?.start_node_id;
        let i = self
            .start_index(start)
            .ok_or_else(|| format!("class {class_id} has no start node"))?;
        self.alloc(i);
        self.select_ascend_class(0)
    }

    /// The current ascendancy, or the class's "None"
    fn ascend_class(&self, id: i64) -> Result<&'t tree::Ascendancy> {
        let class = self.class(self.cur_class_id.ok_or("no class")?)?;
        let find = |id| {
            class
                .ascendancies
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, a)| a)
        };
        find(id)
            .or_else(|| find(0))
            .ok_or_else(|| "no ascendancy 0".into())
    }

    fn reset_ascend_class(&mut self) -> Result<()> {
        if let Some(cur) = self.cur_ascend_class_id {
            let start = self.ascend_class(cur)?.start_node_id;
            if let Some(i) = self.start_index(start) {
                self.dealloc(i);
            }
        }
        Ok(())
    }

    /// `SelectAscendClass(ascendClassId)`
    pub fn select_ascend_class(&mut self, ascend_class_id: i64) -> Result<()> {
        self.reset_ascend_class()?;
        self.cur_ascend_class_id = Some(ascend_class_id);
        let start = self.ascend_class(ascend_class_id)?.start_node_id;
        if let Some(i) = self.start_index(start) {
            self.alloc(i);
        }
        self.build_all_depends_and_paths()
    }

    /// `SelectSecondaryAscendClass(ascendClassId)`
    pub fn select_secondary_ascend_class(&mut self, ascend_class_id: i64) -> Result<()> {
        let Some(alternates) = &self.tree.alternate_ascendancies else {
            return Ok(());
        };
        let find = |id: i64| alternates.iter().find(|(i, _)| *i == id).map(|(_, a)| a);
        if let Some(cur) = self.cur_secondary_ascend_class_id
            && let Some(a) = find(cur)
            && let Some(i) = self.start_index(a.start_node_id)
        {
            self.dealloc(i);
        }
        self.cur_secondary_ascend_class_id = Some(ascend_class_id);
        if ascend_class_id != 0
            && let Some(a) = find(ascend_class_id)
            && let Some(i) = self.start_index(a.start_node_id)
        {
            self.alloc(i);
        }
        self.build_all_depends_and_paths()
    }

    /// `ResetNodes()`: deallocates every node but the start nodes
    fn reset_nodes(&mut self) {
        for i in 0..self.nodes.len() {
            if !self.is_start(i) {
                self.dealloc(i);
            }
        }
        self.mastery_selections.clear();
    }

    /// `PassiveSpecClass:Load(xml)`: the spec of a `<Spec>` element, its
    /// jewel sockets and allocations. Tattoos (`<Overrides>`) and specs in
    /// the old URL format only are not ported.
    pub fn load(&mut self, xml: &crate::xml::Element) -> Result<()> {
        self.title = xml.attr("title").map(String::from);
        let mut url = false;
        for node in xml.elements() {
            if node.elem == "URL" {
                url = true;
            }
            if node.elem == "Sockets" {
                for child in node.elements().filter(|c| c.elem == "Socket") {
                    let num = |k: &str| -> Result<f64> {
                        child
                            .attr(k)
                            .and_then(crate::pat::to_number)
                            .ok_or_else(|| format!("'Socket' element missing '{k}' attribute"))
                    };
                    // Badly saved files have empty sockets with item 0
                    let item = num("itemId")?;
                    if item > 0.0 {
                        self.jewels.insert(num("nodeId")? as u32, item);
                    }
                }
            }
        }
        let Some(nodes) = xml.attr("nodes") else {
            if url {
                return Err("specs in the URL format are not ported".into());
            }
            return Ok(());
        };
        if xml
            .elements()
            .any(|n| n.elem == "Overrides" && n.elements().next().is_some())
        {
            return Err("tattoos are not ported".into());
        }
        let num = |k: &str| xml.attr(k).and_then(crate::pat::to_number);
        let class_id = num("classId").ok_or("'Spec' element missing 'classId' attribute")?;
        let ascend_class_id =
            num("ascendClassId").ok_or("'Spec' element missing 'ascendClassId' attribute")?;
        let hash_list: Vec<u32> = crate::pat::gmatch1(nodes, "%d+")
            .iter()
            .filter_map(|h| crate::pat::to_number(h))
            .map(|h| h as u32)
            .collect();
        // A table by mastery: a mastery listed again keeps its place
        let mut effects: Vec<(u32, f64)> = Vec::new();
        for c in crate::pat::gmatch(xml.attr("masteryEffects").unwrap_or(""), "{(%d+),(%d+)}") {
            let (m, e) = (
                c[0].parse::<f64>().unwrap_or(0.0) as u32,
                c[1].parse().unwrap_or(0.0),
            );
            match effects.iter_mut().find(|(k, _)| *k == m) {
                Some(slot) => slot.1 = e,
                None => effects.push((m, e)),
            }
        }
        self.import_from_node_list(
            class_id as i64,
            ascend_class_id as i64,
            num("secondaryAscendClassId").unwrap_or(0.0) as i64,
            &hash_list,
            &effects,
        )
    }

    /// `ImportFromNodeList(nil, classId, ascendClassId,
    /// secondaryAscendClassId, hashList, {}, masteryEffects)`, as loading a
    /// build does. `mastery_effects` are (mastery node, effect) in the order
    /// the build lists them.
    pub fn import_from_node_list(
        &mut self,
        class_id: i64,
        ascend_class_id: i64,
        secondary_ascend_class_id: i64,
        hash_list: &[u32],
        mastery_effects: &[(u32, f64)],
    ) -> Result<()> {
        self.reset_nodes();
        self.select_class(class_id)?;
        self.select_ascend_class(ascend_class_id)?;
        self.select_secondary_ascend_class(secondary_ascend_class_id)?;
        self.mastery_selections.clear();
        for (mastery, effect) in mastery_effects {
            // Ignore GGG's codes from profile imports
            if *effect < 65536.0 {
                self.mastery_selections.insert(*mastery, *effect);
            }
        }
        for id in hash_list {
            match self.index.get(id).copied() {
                Some(i) => {
                    if self.kind(i) != NodeType::Mastery || self.mastery_selections.contains(*id) {
                        self.alloc(i);
                    }
                }
                None => self.alloc_subgraph_nodes.push(*id),
            }
        }
        self.build_all_depends_and_paths()
    }

    /// `PostLoad()`, which rebuilds the cluster jewel subgraphs (not ported:
    /// there are none here): allocated ids the spec now has are allocated,
    /// then dependencies and paths are rebuilt.
    pub fn post_load(&mut self) -> Result<()> {
        for id in std::mem::take(&mut self.alloc_subgraph_nodes) {
            if let Some(&i) = self.index.get(&id) {
                self.nodes[i].alloc = true;
                if !self.alloc_nodes.contains(id) {
                    self.alloc_nodes.insert(id, i);
                }
            }
        }
        self.build_all_depends_and_paths()
    }

    /// `ProcessStats` on a node with the given lines, keeping its unknown
    /// flag as upstream does
    fn set_stats(&mut self, i: usize, sd: Vec<Rc<str>>, starts: &[usize]) -> Result<()> {
        let source = format!("Tree:{}", self.nodes[i].id);
        let mut mods = Vec::new();
        let mut mod_list = Vec::new();
        let mut lines: Vec<Rc<str>> = Vec::new();
        let mut unknown = false;
        // Lines are added and processed effect by effect
        let mut bounds: Vec<usize> = starts.to_vec();
        bounds.push(sd.len());
        if starts.is_empty() {
            let mut sd = sd;
            let (m, u, _) =
                tree::process_stats_from(self.parser, &mut sd, &mut mods, 0, Some(&source))?;
            mod_list = m;
            unknown |= u;
            lines = sd;
        } else {
            for w in bounds.windows(2) {
                let start = lines.len();
                lines.extend_from_slice(&sd[w[0]..w[1]]);
                let (m, u, _) = tree::process_stats_from(
                    self.parser,
                    &mut lines,
                    &mut mods,
                    start,
                    Some(&source),
                )?;
                if start == 0 {
                    mod_list = m;
                } else {
                    mod_list.extend(m);
                }
                unknown |= u;
            }
        }
        self.nodes[i].unknown |= unknown;
        self.nodes[i].stats = Some(NodeStats {
            sd: lines,
            mods,
            mod_list,
        });
        Ok(())
    }

    /// `AddMasteryEffectOptionsToNode(node)`: every effect's lines
    fn add_mastery_effect_options(&mut self, i: usize) -> Result<()> {
        let tn = self.tree_node(i);
        let mut sd = Vec::new();
        let mut starts = Vec::new();
        for eid in &tn.mastery_effects {
            let effect = &self.tree.mastery_effects[&eid.to_bits()];
            starts.push(sd.len());
            sd.extend(effect.sd.iter().cloned());
        }
        self.set_stats(i, sd, &starts)?;
        self.nodes[i].all_mastery_options = Some(true);
        Ok(())
    }

    /// `DeallocSingleNode(node)`
    fn dealloc_single(&mut self, i: usize) -> Result<()> {
        self.dealloc(i);
        if self.kind(i) == NodeType::Mastery {
            self.add_mastery_effect_options(i)?;
            self.mastery_selections.remove(self.nodes[i].id);
        }
        Ok(())
    }

    /// `FindStartFromNode(node, visited, noAscend)`: whether an allocated
    /// path leads from the node to a start node
    fn find_start(&mut self, i: usize, visited: &mut Vec<usize>, no_ascend: bool) -> bool {
        self.nodes[i].visited = true;
        visited.push(i);
        for li in 0..self.nodes[i].linked.len() {
            let other = self.nodes[i].linked[li];
            let start = visited.len();
            let reaches = self.nodes[other].alloc
                && (self.is_start(other)
                    || (!self.nodes[other].visited
                        && self.kind(i) != NodeType::Mastery
                        && self.find_start(other, visited, no_ascend)));
            if reaches {
                if self.tree_node(i).ascendancy_name.is_some()
                    && self.tree_node(other).ascendancy_name.is_none()
                {
                    // Pathing out of Ascendant: unvisit the outside nodes
                    for &v in &visited[start..] {
                        self.nodes[v].visited = false;
                    }
                    visited.truncate(start);
                } else if !no_ascend || self.kind(other) != NodeType::AscendClassStart {
                    return true;
                }
            }
        }
        false
    }

    /// `BuildAllDependsAndPaths()`
    pub fn build_all_depends_and_paths(&mut self) -> Result<()> {
        // Reset the nodes to the tree's
        for i in 0..self.nodes.len() {
            self.nodes[i].depends.clear();
            self.nodes[i].stats = None;
            if self.nodes[i].alloc {
                self.nodes[i].depends.push(i);
            }
        }
        // Selected mastery effects, and counts
        self.allocated_mastery_count = 0;
        self.allocated_notable_count = 0;
        self.allocated_keystone_count = 0;
        self.allocated_mastery_types.clear();
        self.allocated_mastery_type_count = 0;
        for i in 0..self.nodes.len() {
            let id = self.nodes[i].id;
            let kind = self.kind(i);
            if kind == NodeType::Mastery
                && let Some(&sel) = self.mastery_selections.get(id)
            {
                let effect = self.tree.mastery_effects.get(&sel.to_bits());
                if let (Some(effect), true) = (effect, self.alloc_nodes.contains(id)) {
                    self.set_stats(i, effect.sd.clone(), &[])?;
                    self.nodes[i].all_mastery_options = Some(false);
                    self.allocated_mastery_count += 1;
                    let name = self.tree_node(i).dn.clone();
                    match self
                        .allocated_mastery_types
                        .iter_mut()
                        .find(|(n, _)| *n == name)
                    {
                        None => {
                            self.allocated_mastery_types.push((name, 1));
                            self.allocated_mastery_type_count += 1;
                        }
                        Some((_, count)) => {
                            if *count == 0 {
                                self.allocated_mastery_type_count += 1;
                            }
                            *count += 1;
                        }
                    }
                } else {
                    self.dealloc(i);
                    self.mastery_selections.remove(id);
                }
            } else if kind == NodeType::Mastery {
                self.add_mastery_effect_options(i)?;
            } else if kind == NodeType::Notable && self.nodes[i].alloc {
                self.allocated_notable_count += 1;
            } else if kind == NodeType::Keystone && self.nodes[i].alloc {
                self.allocated_keystone_count += 1;
            }
        }

        // Dependencies: which allocated nodes reach a start only through
        // which others
        let mut visited: Vec<usize> = Vec::new();
        let mut place = 0;
        while place < self.alloc_nodes.places() {
            let Some((_, &i)) = self.alloc_nodes.at(place) else {
                place += 1;
                continue;
            };
            place += 1;
            self.nodes[i].visited = true;
            self.nodes[i].connected_to_start = false;
            let mut any_start = self.is_start(i);
            for li in 0..self.nodes[i].linked.len() {
                let other = self.nodes[i].linked[li];
                if !self.nodes[other].alloc || self.nodes[i].depends.contains(&other) {
                    continue;
                }
                if self.is_start(other) {
                    any_start = true;
                    self.nodes[i].connected_to_start = true;
                } else if self.find_start(other, &mut visited, false) {
                    any_start = true;
                    self.nodes[i].connected_to_start = true;
                    for &v in &visited {
                        self.nodes[v].visited = false;
                    }
                    visited.clear();
                } else {
                    // Everything visited depends on this node, but masteries
                    // with another allocated neighbour
                    let dep_ids: Vec<u32> = visited.iter().map(|&n| self.nodes[n].id).collect();
                    for &n in &visited {
                        if self.kind(n) == NodeType::Mastery {
                            let alloc_links = self.nodes[n]
                                .linked
                                .iter()
                                .filter(|&&l| self.nodes[l].alloc)
                                .count();
                            let other_path = alloc_links > 1
                                && self.nodes[n].linked.iter().any(|&l| {
                                    self.nodes[l].alloc && !dep_ids.contains(&self.nodes[l].id)
                                });
                            if !other_path {
                                self.nodes[i].depends.push(n);
                            }
                        } else {
                            self.nodes[i].depends.push(n);
                        }
                        self.nodes[n].visited = false;
                    }
                    visited.clear();
                }
            }
            self.nodes[i].visited = false;
            if !any_start {
                // An orphan: it and what depends on it are deallocated
                let deps = self.nodes[i].depends.clone();
                for d in deps {
                    self.dealloc_single(d)?;
                }
            }
        }

        // Paths from every allocated node
        for i in 0..self.nodes.len() {
            self.nodes[i].path_dist = if self.nodes[i].alloc { 0.0 } else { 1000.0 };
            self.nodes[i].path = None;
            if self.is_socket(i) {
                self.nodes[i].distance_to_class_start = Some(0.0);
            }
        }
        let mut place = 0;
        while place < self.alloc_nodes.places() {
            if let Some((_, &i)) = self.alloc_nodes.at(place) {
                self.build_path_from_node(i);
                if self.is_socket(i) {
                    self.set_node_distance_to_class_start(i)?;
                }
            }
            place += 1;
        }
        Ok(())
    }

    /// `node.isJewelSocket or node.expansionJewel`
    fn is_socket(&self, i: usize) -> bool {
        let tn = self.tree_node(i);
        tn.kind == NodeType::Socket || tn.expansion_jewel.is_some()
    }

    /// `BuildPathFromNode(root)`: a breadth-first search recording in every
    /// node closer to `root` than to any other allocated node its distance
    /// and path
    fn build_path_from_node(&mut self, root: usize) {
        self.nodes[root].path_dist = 0.0;
        self.nodes[root].path = Some(None);
        let mut queue = vec![root];
        let mut o = 0;
        while o < queue.len() {
            let node = queue[o];
            o += 1;
            let cur = self.nodes[node].path_dist;
            let node_asc = self.tree_node(node).ascendancy_name;
            let node_mastery = self.kind(node) == NodeType::Mastery;
            for li in 0..self.nodes[node].linked.len() {
                let other = self.nodes[node].linked[li];
                let other_asc = self.tree_node(other).ascendancy_name;
                if !node_mastery
                    && !self.is_start(other)
                    && self.nodes[other].path_dist > cur
                    && (node_asc == other_asc || (cur == 0.0 && other_asc.is_none()))
                {
                    let mut dist = cur;
                    if !self.nodes[other].alloc {
                        dist += 1.0;
                    }
                    self.nodes[other].path_dist = dist;
                    let rest = self.nodes[node].path.clone().flatten();
                    self.nodes[other].path = Some(Some(Rc::new(PathStep { node: other, rest })));
                    queue.push(other);
                }
            }
        }
    }

    /// A node's path (`node.path`), nearest node first; empty for a root,
    /// `None` when no allocated node reaches it
    pub fn path(&self, i: usize) -> Option<Vec<usize>> {
        let mut cur = self.nodes[i].path.clone()?;
        let mut out = Vec::new();
        while let Some(step) = cur {
            out.push(step.node);
            cur = step.rest.clone();
        }
        Some(out)
    }

    /// `SetNodeDistanceToClassStart(root)`
    fn set_node_distance_to_class_start(&mut self, root: usize) -> Result<()> {
        self.nodes[root].distance_to_class_start = Some(0.0);
        if !self.nodes[root].alloc || !self.nodes[root].connected_to_start {
            return Ok(());
        }
        let target = self
            .class(self.cur_class_id.ok_or("no class")?)?
            .start_node_id;
        let mut dist: HashMap<usize, f64> = HashMap::new();
        dist.insert(root, 0.0);
        let mut queue = vec![root];
        let mut o = 0;
        while o < queue.len() {
            let node = queue[o];
            o += 1;
            let cur = dist[&node] + 1.0;
            for li in 0..self.nodes[node].linked.len() {
                let other = self.nodes[node].linked[li];
                if Some(self.nodes[other].id) == target {
                    self.nodes[root].distance_to_class_start = Some(cur - 1.0);
                    return Ok(());
                }
                if self.nodes[other].alloc
                    && self.kind(node) != NodeType::Mastery
                    && !self.is_start(other)
                    && !dist.contains_key(&other)
                {
                    dist.insert(other, cur);
                    queue.push(other);
                }
            }
        }
        Ok(())
    }

    /// The name of the node's mastery for counts
    pub fn mastery_name(&self, i: usize) -> Sym {
        Sym::new(&self.tree_node(i).dn)
    }
}
