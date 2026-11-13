//! A passive tree allocation: upstream's `PassiveSpec` class
//! (`Classes/PassiveSpec.lua`): class and ascendancy, allocated nodes,
//! mastery selections, and the dependencies and paths
//! `BuildAllDependsAndPaths` derives from them.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.
//!
//! Jewels that let nodes be allocated without a connection (Intuitive Leap
//! and the like, Impossible Escape) and tattoos (`hashOverrides`) are
//! ported, and so are timeless jewels' conquered nodes
//! (`crate::timeless`) but the Abyss jewels'. Not ported yet: Split
//! Personality's path, tree conversion and URLs.
//!
//! Upstream keeps the nodes, allocated nodes and mastery selections in
//! tables keyed by node id and visits them with `pairs`; which paths win
//! ties, the order of dependency lists and of mods follow that order.
//! [`LjMap`] keeps LuaJIT's, so results compare exactly.

use crate::mod_parser::ModParser;
use crate::mods::{Mod, Sym};
use crate::tree::{self, NodeType, PassiveTree, StatLine};
use lua::ljtab::LjMap;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, String>;

/// A spec's `masteryEffects` attribute as upstream's table by mastery:
/// (mastery node, effect) in its `pairs` order.
pub fn mastery_effects(attr: &str) -> Vec<(u32, f64)> {
    let mut effects = LjMap::default();
    for c in crate::pat::gmatch(attr, "{(%d+),(%d+)}") {
        effects.insert(
            c[0].parse::<f64>().unwrap_or(0.0) as u32,
            c[1].parse().unwrap_or(0.0),
        );
    }
    effects.iter().map(|(k, &v)| (k, v)).collect()
}

/// `copyTable(t, true)`: a new table filled in `t`'s `pairs` order, whose
/// own order can differ (`env.allocNodes` is one of the spec's).
pub fn copy_table<V: Clone>(t: &LjMap<V>) -> LjMap<V> {
    let mut out = LjMap::default();
    for (k, v) in t.iter() {
        out.insert(k, v.clone());
    }
    out
}

/// A path to the nearest allocated node, nearest first; shared, as each
/// path is the path of the node it extends plus one node.
#[derive(Debug)]
pub struct PathStep {
    pub node: usize,
    pub rest: Option<Arc<PathStep>>,
}

/// A node's stat lines and mods when the spec changed them (a mastery's
/// selected effect, or all its effects).
#[derive(Clone, Debug, PartialEq)]
pub struct NodeStats {
    pub sd: Vec<Arc<str>>,
    pub mods: Vec<StatLine>,
    pub mod_list: Vec<Mod>,
}

/// An allocated jewel socket that changes paths or conquers nodes: its
/// index, jewel, the ids in its radius and in its keystones' radii
type RadiusJewel<'j> = (usize, &'j TreeJewel, LjMap<()>, Vec<LjMap<()>>);

/// What a socketed jewel changes in the tree's paths: Intuitive Leap and
/// the like, Impossible Escape
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeJewel {
    /// `jewelRadiusIndex`, from 1
    pub radius_index: Option<usize>,
    /// `jewelData.intuitiveLeapLike`
    pub intuitive_leap_like: bool,
    /// `jewelData.intuitiveLeapKeystoneOnly`
    pub keystone_only: bool,
    /// `jewelData.impossibleEscapeKeystone`
    pub impossible_escape: bool,
    /// `jewelData.impossibleEscapeKeystones`
    pub keystones: Vec<Arc<str>>,
    /// A timeless jewel's `jewelData.conqueredBy`
    pub conquest: Option<crate::timeless::Conquest>,
}

impl TreeJewel {
    /// The jewel's data, if it changes paths
    pub fn of(item: &crate::item::Item) -> Option<TreeJewel> {
        use crate::mods::ModValue;
        let jd = item.jewel_data.as_ref()?;
        let truthy = |k: &str| jd.get(k).is_some_and(ModValue::truthy);
        let keystones: Vec<Arc<str>> = match jd.get("impossibleEscapeKeystones") {
            Some(ModValue::Record(r)) => r
                .fields
                .iter()
                .map(|(k, _)| Arc::from(k.as_str()))
                .collect(),
            _ => Vec::new(),
        };
        let j = TreeJewel {
            radius_index: item.jewel_radius_index.map(|r| r as usize),
            intuitive_leap_like: truthy("intuitiveLeapLike"),
            keystone_only: truthy("intuitiveLeapKeystoneOnly"),
            impossible_escape: truthy("impossibleEscapeKeystone"),
            keystones,
            conquest: crate::timeless::Conquest::of(jd),
        };
        (j.intuitive_leap_like
            || j.impossible_escape
            || !j.keystones.is_empty()
            || j.conquest.is_some())
        .then_some(j)
    }
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
    pub path: Option<Option<Arc<PathStep>>>,
    pub distance_to_class_start: Option<f64>,
    /// Stats the spec set, instead of the tree's
    pub stats: Option<Arc<NodeStats>>,
    pub all_mastery_options: Option<bool>,
    /// Set when processing the spec's stats found an unknown line
    pub unknown: bool,
    /// A cluster jewel subgraph's node, which the tree does not have
    pub synth: Option<Arc<tree::Node>>,
    /// `intuitiveLeapLikesAffecting`: the sockets whose jewels let the
    /// node be allocated without a connection, by index
    pub intuitive: Vec<usize>,
    /// The tattoo or timeless passive that replaces the node
    /// (`ReplaceNode`): its name, keystone mod and tattoo fields
    pub overlay: Option<Arc<tree::Overlay>>,
    /// The timeless jewel that conquers the node (`conqueredBy`)
    pub conquered: Option<crate::timeless::Conquest>,
    visited: bool,
}

impl SpecNode {
    /// A node of a cluster jewel subgraph
    pub(crate) fn synthetic(node: tree::Node, linked: Vec<usize>) -> SpecNode {
        SpecNode {
            id: node.id,
            tree: usize::MAX,
            linked,
            alloc: false,
            depends: Vec::new(),
            connected_to_start: false,
            path_dist: 0.0,
            path: None,
            distance_to_class_start: None,
            stats: None,
            all_mastery_options: None,
            unknown: false,
            synth: Some(Arc::new(node)),
            intuitive: Vec::new(),
            overlay: None,
            conquered: None,
            visited: false,
        }
    }
}

/// A cluster jewel subgraph (`subGraphs[id]`)
#[derive(Clone, Debug, Default)]
pub struct SubGraph {
    /// Its node ids
    pub nodes: Vec<u32>,
    /// The node pairs it builds connectors for (`nodeId1`, `nodeId2`), in
    /// upstream's order
    pub connectors: Vec<(u32, u32)>,
}

#[derive(Clone)]
pub struct PassiveSpec<'t> {
    pub tree: &'t PassiveTree,
    pub(crate) parser: &'t ModParser,
    pub nodes: Vec<SpecNode>,
    pub index: HashMap<u32, usize>,
    /// `self.nodes` as upstream's table: node indices in `pairs` order
    pub order: LjMap<usize>,
    /// Allocated nodes, by id
    pub alloc_nodes: LjMap<usize>,
    /// Selected mastery effect by mastery node id
    pub mastery_selections: LjMap<f64>,
    /// Allocated node ids the spec does not have (in cluster subgraphs)
    pub alloc_subgraph_nodes: Vec<u32>,
    /// `allocExtendedNodes`: the subgraph nodes `BuildClusterJewelGraphs`
    /// allocated again, which `ImportFromNodeList` (loads, undo) allocates
    /// whatever its list says; upstream never empties it
    pub alloc_extended_nodes: Vec<u32>,
    /// `subGraphs`: each cluster jewel subgraph, by its id
    pub sub_graphs: LjMap<SubGraph>,
    pub cur_class_id: Option<i64>,
    pub cur_ascend_class_id: Option<i64>,
    pub cur_secondary_ascend_class_id: Option<i64>,
    /// `curSecondaryAscendClass`, by id: unlike the id, selecting nil
    /// leaves it
    pub cur_secondary_ascend_class: Option<i64>,
    pub allocated_mastery_count: u32,
    pub allocated_notable_count: u32,
    pub allocated_keystone_count: u32,
    /// Allocated masteries by mastery name, in the order first counted
    pub allocated_mastery_types: Vec<(Arc<str>, u32)>,
    pub allocated_mastery_type_count: u32,
    /// Jewel socket node id to the id of the item in it
    pub jewels: BTreeMap<u32, f64>,
    /// The socketed jewels that change paths, by socket
    pub tree_jewels: BTreeMap<u32, TreeJewel>,
    /// `hashOverrides`: the tattoos, by node id
    pub hash_overrides: BTreeMap<u32, Arc<tree::Overlay>>,
    /// `allocatedTattooTypes`: allocated tattoos by type
    pub allocated_tattoo_types: Vec<(Arc<str>, u32)>,
    /// The timeless jewels' data, which conquered nodes need
    pub timeless: Option<Arc<crate::timeless::Timeless>>,
    pub title: Option<String>,
    /// `clusterHashFormatVersion`: 1 for builds saved with legacy cluster
    /// node ids
    pub cluster_hash_format_version: f64,
    /// `legacyClusterNodeMap` (in insertion order) and its reverse, while
    /// the subgraphs are built
    pub(crate) legacy_map: Option<Vec<(u32, u32)>>,
    pub(crate) legacy_reverse: Option<HashMap<u32, u32>>,
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
            order: LjMap::default(),
            alloc_nodes: LjMap::default(),
            mastery_selections: LjMap::default(),
            alloc_subgraph_nodes: Vec::new(),
            alloc_extended_nodes: Vec::new(),
            sub_graphs: LjMap::default(),
            cur_class_id: None,
            cur_ascend_class_id: None,
            cur_secondary_ascend_class_id: None,
            cur_secondary_ascend_class: None,
            allocated_mastery_count: 0,
            allocated_notable_count: 0,
            allocated_keystone_count: 0,
            allocated_mastery_types: Vec::new(),
            allocated_mastery_type_count: 0,
            jewels: BTreeMap::new(),
            tree_jewels: BTreeMap::new(),
            hash_overrides: BTreeMap::new(),
            allocated_tattoo_types: Vec::new(),
            timeless: None,
            title: None,
            cluster_hash_format_version: 2.0,
            legacy_map: None,
            legacy_reverse: None,
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
                spec.order.insert(n.id, spec.nodes.len());
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
                    synth: None,
                    intuitive: Vec::new(),
                    overlay: None,
                    conquered: None,
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

    pub fn tree_node(&self, i: usize) -> &tree::Node {
        match &self.nodes[i].synth {
            Some(n) => n,
            None => &self.tree.nodes[self.nodes[i].tree],
        }
    }

    pub fn kind(&self, i: usize) -> NodeType {
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

    pub fn sd(&self, i: usize) -> &[Arc<str>] {
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

    /// What `ItemsTab:DeleteItem` does to a socket whose jewel goes: the
    /// nodes depending on it lose their allocation, and `allocNodes` the
    /// keys of their places in `depends` (upstream's `ipairs` index)
    pub fn drop_jewel_socket(&mut self, node: u32) {
        let Some(&i) = self.index.get(&node) else {
            return;
        };
        let depends = self.nodes[i].depends.clone();
        for (k, d) in depends.iter().enumerate() {
            self.nodes[*d].alloc = false;
            self.alloc_nodes.remove(k as u32 + 1);
        }
        self.dealloc(i);
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
        self.select_secondary(Some(ascend_class_id))
    }

    /// `SelectSecondaryAscendClass(ascendClassId)`, which may be nil
    fn select_secondary(&mut self, ascend_class_id: Option<i64>) -> Result<()> {
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
        self.cur_secondary_ascend_class_id = ascend_class_id;
        match ascend_class_id {
            Some(0) => self.cur_secondary_ascend_class = None,
            Some(id) if find(id).is_some() => self.cur_secondary_ascend_class = Some(id),
            _ => {}
        }
        if let Some(id) = ascend_class_id.filter(|id| *id != 0)
            && let Some(a) = find(id)
            && let Some(i) = self.start_index(a.start_node_id)
        {
            self.alloc(i);
        }
        self.build_all_depends_and_paths()
    }

    /// `ResetNodes()`: deallocates every node but the start nodes
    pub fn reset_nodes(&mut self) {
        for p in 0..self.order.places() {
            if let Some((_, &i)) = self.order.at(p)
                && !self.is_start(i)
            {
                self.dealloc(i);
            }
        }
        self.mastery_selections.clear();
    }

    /// `PassiveSpecClass:Load(xml)`: the spec of a `<Spec>` element, its
    /// jewel sockets, tattoos (`<Overrides>`) and allocations. Specs in the
    /// old URL format only are not ported.
    pub fn load(&mut self, xml: &crate::xml::Element) -> Result<()> {
        self.load_with(xml, None)
    }

    /// [`PassiveSpec::load`] with the build's items loaded before, whose
    /// jewels change the tree's paths
    pub fn load_with(
        &mut self,
        xml: &crate::xml::Element,
        tab: Option<&crate::items_tab::ItemsTab>,
    ) -> Result<()> {
        self.title = xml.attr("title").map(String::from);
        self.cluster_hash_format_version = xml
            .attr("clusterHashFormatVersion")
            .and_then(crate::pat::to_number)
            .unwrap_or(if xml.attr("nodes").is_some() {
                1.0
            } else {
                2.0
            });
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
        if let Some(tab) = tab {
            self.set_tree_jewels(tab);
        }
        let Some(nodes) = xml.attr("nodes") else {
            if url {
                return Err("specs in the URL format are not ported".into());
            }
            return Ok(());
        };
        self.hash_overrides.clear();
        for o in xml.elements().filter(|n| n.elem == "Overrides") {
            for child in o.elements() {
                let id = child
                    .attr("nodeId")
                    .and_then(crate::pat::to_number)
                    .ok_or("'Override' element missing 'nodeId' attribute")?;
                let dn = child.attr("dn").unwrap_or("");
                // A tattoo replaced by another is found by its images
                let tattoo = self.tree.tattoos.get(dn).or_else(|| {
                    self.tree.tattoos.values().find(|t| {
                        t.active_effect_image.as_deref() == child.attr("activeEffectImage")
                            && t.icon.as_deref() == child.attr("icon")
                    })
                });
                if let Some(t) = tattoo {
                    self.hash_overrides.insert(id as u32, t.clone());
                }
            }
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
        let effects = mastery_effects(xml.attr("masteryEffects").unwrap_or(""));
        self.import_from_node_list(
            class_id as i64,
            ascend_class_id as i64,
            match xml.attr("secondaryAscendClassId") {
                None => Some(0),
                Some(v) => crate::pat::to_number(v).map(|v| v as i64),
            },
            &hash_list,
            &effects,
        )
    }

    /// `ImportFromNodeList(nil, classId, ascendClassId,
    /// secondaryAscendClassId, hashList, {}, masteryEffects)`, as loading a
    /// build does. `mastery_effects` are (mastery node, effect) in the
    /// `pairs` order of upstream's table.
    pub fn import_from_node_list(
        &mut self,
        class_id: i64,
        ascend_class_id: i64,
        secondary_ascend_class_id: Option<i64>,
        hash_list: &[u32],
        mastery_effects: &[(u32, f64)],
    ) -> Result<()> {
        self.reset_nodes();
        self.select_class(class_id)?;
        self.select_ascend_class(ascend_class_id)?;
        self.select_secondary(secondary_ascend_class_id)?;
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
        for k in 0..self.alloc_extended_nodes.len() {
            if let Some(&i) = self.index.get(&self.alloc_extended_nodes[k]) {
                self.alloc(i);
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
                    if !self.alloc_extended_nodes.contains(&id) {
                        self.alloc_extended_nodes.push(id);
                    }
                }
            }
        }
        self.build_all_depends_and_paths()
    }

    /// `ProcessStats` on a node with the given lines, keeping its unknown
    /// flag as upstream does
    fn set_stats(&mut self, i: usize, sd: Vec<Arc<str>>, starts: &[usize]) -> Result<()> {
        let source = format!("Tree:{}", self.nodes[i].id);
        let mut mods = Vec::new();
        let mut mod_list = Vec::new();
        let mut lines: Vec<Arc<str>> = Vec::new();
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
        self.nodes[i].stats = Some(Arc::new(NodeStats {
            sd: lines,
            mods,
            mod_list,
        }));
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

    /// `AllocNode(node)`: the node and the nodes on its path (the node
    /// alone where a jewel lets it go unconnected), then the cluster jewel
    /// subgraphs or the paths rebuilt.
    pub fn alloc_node(
        &mut self,
        i: usize,
        tab: &crate::items_tab::ItemsTab,
        data: &crate::item_data::ItemData,
    ) -> Result<()> {
        let Some(path) = self.path(i) else {
            return Ok(());
        };
        self.set_tree_jewels(tab);
        let mut rebuild = false;
        let path = if self.nodes[i].intuitive.is_empty() {
            path
        } else {
            vec![i]
        };
        for p in path {
            rebuild |= !self.nodes[p].alloc && self.tree_node(p).expansion_jewel.is_some();
            self.alloc(p);
        }
        if self.tree_node(i).is_multiple_choice_option
            && let Some(&parent) = self.nodes[i].linked.first()
        {
            for k in 0..self.nodes[parent].linked.len() {
                let o = self.nodes[parent].linked[k];
                if o != i && self.nodes[o].alloc && self.tree_node(o).is_multiple_choice_option {
                    self.dealloc(o);
                }
            }
        }
        if rebuild {
            self.rebuild_cluster_jewel_graphs(tab, data)
        } else {
            self.build_all_depends_and_paths()
        }
    }

    /// `DeallocNode(node)`: the node and the nodes that depend on it
    pub fn dealloc_node(
        &mut self,
        i: usize,
        tab: &crate::items_tab::ItemsTab,
        data: &crate::item_data::ItemData,
    ) -> Result<()> {
        self.set_tree_jewels(tab);
        let mut rebuild = false;
        for d in self.nodes[i].depends.clone() {
            rebuild |= self.nodes[d].alloc && self.tree_node(d).expansion_jewel.is_some();
            self.dealloc_single(d)?;
        }
        if rebuild {
            self.rebuild_cluster_jewel_graphs(tab, data)
        } else {
            self.build_all_depends_and_paths()
        }
    }

    /// `curAscendClassBaseName`: the current ascendancy's id
    pub fn ascend_class_base_name(&self) -> Option<Arc<str>> {
        self.ascend_class(self.cur_ascend_class_id?)
            .ok()?
            .id
            .clone()
    }

    /// The current class's ascendancies (`curClass.classes`)
    pub fn ascend_classes(&self) -> Vec<(i64, &'t tree::Ascendancy)> {
        self.cur_class_id
            .and_then(|c| self.class(c).ok())
            .map(|c| c.ascendancies.iter().map(|(i, a)| (*i, a)).collect())
            .unwrap_or_default()
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

    /// `IsClassConnected(classId)`: whether an allocated node next to the
    /// class's start node leads back to the current class's start
    pub fn is_class_connected(&mut self, class_id: i64) -> Result<bool> {
        let start = self.class(class_id)?.start_node_id;
        let Some(start) = self.start_index(start) else {
            return Ok(false);
        };
        for li in 0..self.nodes[start].linked.len() {
            let other = self.nodes[start].linked[li];
            if !self.nodes[other].alloc {
                continue;
            }
            self.nodes[other].visited = true;
            let mut visited = Vec::new();
            let found = self.find_start(other, &mut visited, true);
            for v in visited {
                self.nodes[v].visited = false;
            }
            self.nodes[other].visited = false;
            if found {
                return Ok(true);
            }
        }
        Ok(false)
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

    /// Reads the socketed jewels that change paths from the items
    pub fn set_tree_jewels(&mut self, tab: &crate::items_tab::ItemsTab) {
        self.tree_jewels = self
            .jewels
            .iter()
            .filter(|(_, id)| **id > 0.0)
            .filter_map(|(&s, &id)| Some((s, TreeJewel::of(tab.item(id)?)?)))
            .collect();
    }

    /// The ids of the nodes in a radius of a tree node, in upstream's
    /// `pairs` order over `nodesInRadius[radiusIndex]`
    fn radius_ids(&self, id: u32, radius: Option<usize>) -> LjMap<()> {
        let mut out = LjMap::default();
        let list = radius
            .and_then(|r| r.checked_sub(1))
            .and_then(|r| self.tree.node(id)?.nodes_in_radius.as_ref()?.get(r));
        for &n in list.into_iter().flatten() {
            out.insert(n, ());
        }
        out
    }

    /// `NodesInIntuitiveLeapLikeRadius(node)`: the allocated nodes a jewel
    /// in the socket lets go unconnected
    fn nodes_in_intuitive_radius(&self, socket: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let Some(j) = self.tree_jewels.get(&self.nodes[socket].id) else {
            return out;
        };
        let mut add = |ids: LjMap<()>| {
            for (id, _) in ids.iter() {
                if let Some(&k) = self.index.get(&id)
                    && self.nodes[k].alloc
                {
                    out.push(k);
                }
            }
        };
        if j.intuitive_leap_like {
            add(self.radius_ids(self.nodes[socket].id, j.radius_index));
        }
        if j.impossible_escape {
            for name in &j.keystones {
                if let Some(&k) = self.tree.keystone_map.get(name) {
                    add(self.radius_ids(k, j.radius_index));
                }
            }
        }
        out
    }

    /// `BuildAllDependsAndPaths()`
    pub fn build_all_depends_and_paths(&mut self) -> Result<()> {
        // The allocated jewels that change paths, and the nodes in their
        // radius
        let mut radius_jewels: Vec<RadiusJewel> = Vec::new();
        for (socket, j) in &self.tree_jewels {
            if j.radius_index.is_none() || !self.alloc_nodes.contains(*socket) {
                continue;
            }
            let Some(&si) = self.index.get(socket) else {
                continue;
            };
            let keystones = j
                .keystones
                .iter()
                .filter_map(|n| self.tree.keystone_map.get(n))
                .map(|&k| self.radius_ids(k, j.radius_index))
                .collect();
            radius_jewels.push((si, j, self.radius_ids(*socket, j.radius_index), keystones));
        }
        // The Abyss jewels' changes, read before the nodes are reset (Zorath
        // follows the allocated path from its socket to the class start)
        let mut abyss: HashMap<u32, crate::timeless::Conquest> = HashMap::new();
        for (socket, j) in &self.tree_jewels {
            let Some(c) = &j.conquest else {
                continue;
            };
            let Some(ty) = c.jewel_type().filter(|&t| t >= 7) else {
                continue;
            };
            let Some(&si) = self.index.get(socket) else {
                continue;
            };
            if !self.alloc_nodes.contains(*socket)
                || self
                    .tree_node(si)
                    .expansion_jewel
                    .as_ref()
                    .is_some_and(|e| e.parent.is_some())
            {
                continue;
            }
            let path = if ty == 11 {
                self.shortest_path_to_class_start(si)
            } else {
                None
            };
            let timeless = self
                .timeless
                .clone()
                .ok_or("not ported: no timeless jewel data")?;
            for (node, m) in timeless.read_abyss(c.seed, *socket, ty, path.as_deref())? {
                abyss.insert(
                    node,
                    crate::timeless::Conquest {
                        modification: Some(m),
                        ..c.clone()
                    },
                );
            }
        }
        let mut intuitive: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut conquered: Vec<(usize, crate::timeless::Conquest)> = Vec::new();
        for i in 0..self.nodes.len() {
            let kind = self.kind(i);
            let id = self.nodes[i].id;
            if radius_jewels.is_empty()
                || matches!(kind, NodeType::ClassStart | NodeType::Socket)
                || self.tree_node(i).ascendancy_name.is_some()
            {
                if let Some(c) = abyss.remove(&id) {
                    conquered.push((i, c));
                }
                continue;
            }
            let mut by = Vec::new();
            let mut conquest = abyss.remove(&id);
            for (si, j, radius, keystones) in &radius_jewels {
                if radius.contains(id)
                    && j.intuitive_leap_like
                    && !(j.keystone_only && kind != NodeType::Keystone)
                {
                    by.push(*si);
                }
                if radius.contains(id)
                    && let Some(c) = &j.conquest
                    && c.jewel_type().is_none_or(|t| t < 7)
                {
                    conquest = Some(c.clone());
                }
                for k in keystones {
                    if k.contains(id) {
                        by.push(*si);
                    }
                }
            }
            if !by.is_empty() {
                intuitive.push((i, by));
            }
            if let Some(c) = conquest {
                conquered.push((i, c));
            }
        }
        drop(radius_jewels);
        // Reset the nodes to the tree's
        for i in 0..self.nodes.len() {
            self.nodes[i].depends.clear();
            self.nodes[i].intuitive.clear();
            self.nodes[i].stats = None;
            self.nodes[i].overlay = None;
            self.nodes[i].conquered = None;
            if self.nodes[i].alloc {
                self.nodes[i].depends.push(i);
            }
        }
        for (i, by) in intuitive {
            self.nodes[i].intuitive = by;
        }
        for (i, c) in conquered {
            self.nodes[i].conquered = Some(c);
        }
        // `ReplaceNode(node, self.tree.nodes[id])`: where the data lists its
        // nodes as an array, that is the node at the place of the id.
        // Upstream skips unallocated masteries with all their options, which
        // keep what the first pass replaced (their lines are the options')
        for &(id, by) in &self.tree.keyed_as {
            let (Some(&i), Some(n)) = (self.index.get(&id), self.tree.node(by)) else {
                continue;
            };
            self.nodes[i].stats = Some(Arc::new(NodeStats {
                sd: n.sd.clone(),
                mods: n.mods.clone(),
                mod_list: n.mod_list.clone(),
            }));
            self.nodes[i].overlay = Some(Arc::new(tree::Overlay::of_node(n)));
        }
        // Tattoos replace their nodes
        for (id, o) in &self.hash_overrides {
            if let Some(&i) = self.index.get(id) {
                self.nodes[i].stats = Some(Arc::new(NodeStats {
                    sd: o.sd.clone(),
                    mods: o.mods.clone(),
                    mod_list: o.mod_list.clone(),
                }));
                if o.override_type.as_deref() == Some("AlternateMastery") {
                    self.nodes[i].all_mastery_options = Some(false);
                }
                self.nodes[i].overlay = Some(o.clone());
            }
        }
        // Timeless jewels replace their nodes or add to them
        for i in 0..self.nodes.len() {
            if let Some(c) = self.nodes[i].conquered.clone()
                && self.kind(i) != NodeType::Socket
            {
                self.conquer(i, &c)?;
            }
        }
        // Selected mastery effects, and counts
        self.allocated_tattoo_types.clear();
        self.allocated_mastery_count = 0;
        self.allocated_notable_count = 0;
        self.allocated_keystone_count = 0;
        self.allocated_mastery_types.clear();
        self.allocated_mastery_type_count = 0;
        for p in 0..self.order.places() {
            let Some((id, &i)) = self.order.at(p) else {
                continue;
            };
            let kind = self.kind(i);
            if kind == NodeType::Mastery
                && let Some(&sel) = self.mastery_selections.get(id)
            {
                let effect = self.tree.mastery_effects.get(&sel.to_bits());
                if let (Some(effect), true) = (effect, self.alloc_nodes.contains(id)) {
                    let sd = match &self.nodes[i].overlay {
                        Some(o) => o.sd.clone(),
                        None => effect.sd.clone(),
                    };
                    self.set_stats(i, sd, &[])?;
                    self.nodes[i].all_mastery_options = Some(false);
                    self.allocated_mastery_count += 1;
                    let name: Arc<str> = self.dn(i).into();
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
                if self.override_type(i).map(|t| &**t) != Some("AlternateMastery") {
                    self.add_mastery_effect_options(i)?;
                }
            } else if kind == NodeType::Notable && self.nodes[i].alloc {
                self.allocated_notable_count += 1;
            } else if kind == NodeType::Keystone && self.nodes[i].alloc {
                self.allocated_keystone_count += 1;
            }
            if self.is_tattoo(i)
                && self.nodes[i].alloc
                && let Some(t) = self.override_type(i).cloned()
            {
                match self
                    .allocated_tattoo_types
                    .iter_mut()
                    .find(|(n, _)| *n == t)
                {
                    Some((_, c)) => *c += 1,
                    None => self.allocated_tattoo_types.push((t, 1)),
                }
            }
        }

        // Dependencies: which allocated nodes reach a start only through
        // which others
        let mut visited: Vec<usize> = Vec::new();
        // Nodes allocated through jewels, which may depend on others:
        // `potentialDeps` by node id, `intuitiveLeaps` by node id
        let mut potential: LjMap<Vec<usize>> = LjMap::default();
        let mut leaps: LjMap<Vec<usize>> = LjMap::default();
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
                    let dep_ids: Vec<u32> = visited
                        .iter()
                        .filter(|&&n| self.nodes[n].intuitive.is_empty())
                        .map(|&n| self.nodes[n].id)
                        .collect();
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
                            if self.nodes[n].intuitive.is_empty() {
                                self.nodes[i].depends.push(n);
                            } else {
                                let nid = self.nodes[n].id;
                                if potential.get(nid).is_none() {
                                    potential.insert(nid, Vec::new());
                                }
                                let mut v = potential.get(nid).cloned().unwrap_or_default();
                                v.push(i);
                                potential.insert(nid, v);
                            }
                            let more = self.nodes_in_intuitive_radius(n);
                            let iid = self.nodes[i].id;
                            let mut v = leaps.get(iid).cloned().unwrap_or_default();
                            v.extend(more);
                            leaps.insert(iid, v);
                        }
                        self.nodes[n].visited = false;
                    }
                    visited.clear();
                }
            }
            self.nodes[i].visited = false;
            if !any_start {
                // An orphan: it and what depends on it are deallocated,
                // but for nodes a jewel may hold
                let deps = self.nodes[i].depends.clone();
                for d in deps {
                    let did = self.nodes[d].id;
                    let holder = self.jewels.keys().copied().find(|s| {
                        self.alloc_nodes.contains(*s)
                            && self.tree_jewels.get(s).is_some_and(|j| {
                                (j.intuitive_leap_like
                                    && j.radius_index.is_some()
                                    && self.radius_ids(*s, j.radius_index).contains(did))
                                    || j.keystones.iter().any(|n| {
                                        self.tree.keystone_map.get(n).is_some_and(|&k| {
                                            self.radius_ids(k, j.radius_index).contains(did)
                                        })
                                    })
                            })
                    });
                    match holder {
                        Some(s) => {
                            let mut v = leaps.get(s).cloned().unwrap_or_default();
                            v.push(d);
                            leaps.insert(s, v);
                        }
                        None => self.dealloc_single(d)?,
                    }
                }
            }
        }
        // Nodes held by jewels depend on the nodes that connect them and
        // their jewels' sockets to the start
        for (id, deps) in potential.iter() {
            let Some(&pi) = self.index.get(&id) else {
                continue;
            };
            let mut seen = Vec::new();
            for &n in deps {
                let all = self.nodes[pi]
                    .intuitive
                    .iter()
                    .all(|s| self.nodes[n].depends.contains(s));
                if all && !seen.contains(&n) {
                    self.nodes[n].depends.push(pi);
                    seen.push(n);
                }
            }
        }
        for (id, deps) in leaps.iter() {
            let Some(&ni) = self.index.get(&id) else {
                continue;
            };
            let mut seen = Vec::new();
            for &d in deps {
                if self.nodes[d].connected_to_start {
                    continue;
                }
                let all = self.nodes[d]
                    .intuitive
                    .iter()
                    .all(|s| self.nodes[ni].depends.contains(s));
                if all && !seen.contains(&d) {
                    self.nodes[ni].depends.push(d);
                    seen.push(d);
                }
            }
        }

        // Paths from every allocated node
        for i in 0..self.nodes.len() {
            self.nodes[i].path_dist = if self.nodes[i].alloc && self.nodes[i].intuitive.is_empty() {
                0.0
            } else {
                1000.0
            };
            self.nodes[i].path = None;
            if self.is_socket(i) {
                self.nodes[i].distance_to_class_start = Some(0.0);
            }
        }
        let mut roots = Vec::new();
        for place in 0..self.alloc_nodes.places() {
            if let Some((_, &i)) = self.alloc_nodes.at(place)
                && (self.nodes[i].intuitive.is_empty() || self.nodes[i].connected_to_start)
            {
                roots.push(i);
            }
        }
        self.build_paths(&roots);
        for &i in &roots {
            if self.is_socket(i) {
                self.set_node_distance_to_class_start(i)?;
            }
        }
        Ok(())
    }

    /// `node.isJewelSocket or node.expansionJewel`
    fn is_socket(&self, i: usize) -> bool {
        let tn = self.tree_node(i);
        tn.kind == NodeType::Socket || tn.expansion_jewel.is_some()
    }

    /// The paths from the allocated nodes: a multi-source 0-1 breadth-first
    /// search where allocated nodes cost nothing and the others one point
    fn build_paths(&mut self, roots: &[usize]) {
        let mut queue = std::collections::VecDeque::new();
        for &r in roots {
            self.nodes[r].path_dist = 0.0;
            self.nodes[r].path = Some(None);
            queue.push_back(r);
        }
        while let Some(node) = queue.pop_front() {
            let cur = self.nodes[node].path_dist;
            let node_asc = self.tree_node(node).ascendancy_name;
            let node_mastery = self.kind(node) == NodeType::Mastery;
            for li in 0..self.nodes[node].linked.len() {
                let other = self.nodes[node].linked[li];
                let free = self.nodes[other].alloc;
                let dist = cur + if free { 0.0 } else { 1.0 };
                let other_asc = self.tree_node(other).ascendancy_name;
                if dist < self.nodes[other].path_dist
                    && !node_mastery
                    && !self.is_start(other)
                    && (node_asc == other_asc || (cur == 0.0 && other_asc.is_none()))
                {
                    if free {
                        queue.push_front(other);
                    } else {
                        queue.push_back(other);
                    }
                    self.nodes[other].path_dist = dist;
                    let rest = self.nodes[node].path.clone().flatten();
                    self.nodes[other].path = Some(Some(Arc::new(PathStep { node: other, rest })));
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

    /// `GetShortestPathToClassStart(rootId)`: the ids of the nodes on the
    /// shortest allocated path from the node to the class start
    pub fn shortest_path_to_class_start(&self, root: usize) -> Option<Vec<u32>> {
        if !self.nodes[root].alloc {
            return None;
        }
        let target = self.class(self.cur_class_id?).ok()?.start_node_id;
        let mut parent: HashMap<usize, usize> = HashMap::new();
        let mut queue = vec![root];
        let mut o = 0;
        while o < queue.len() {
            let node = queue[o];
            o += 1;
            for &other in &self.nodes[node].linked {
                if Some(self.nodes[other].id) == target {
                    let mut path = vec![self.nodes[root].id, self.nodes[other].id];
                    let mut cur = Some(node);
                    while let Some(c) = cur {
                        path.push(self.nodes[c].id);
                        cur = parent.get(&c).copied();
                    }
                    path.sort_unstable();
                    path.dedup();
                    return Some(path);
                }
                if self.nodes[other].alloc
                    && self.kind(node) != NodeType::Mastery
                    && !self.is_start(other)
                    && !parent.contains_key(&other)
                    && other != root
                {
                    parent.insert(other, node);
                    queue.push(other);
                }
            }
        }
        None
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
        Sym::new(self.dn(i))
    }

    /// The node's name, a tattoo's where one replaces it
    pub fn dn(&self, i: usize) -> &str {
        match &self.nodes[i].overlay {
            Some(o) => &o.dn,
            None => &self.tree_node(i).dn,
        }
    }

    /// `node.keystoneMod`
    pub fn keystone_mod(&self, i: usize) -> Option<&Mod> {
        match &self.nodes[i].overlay {
            Some(o) => o.keystone_mod.as_ref(),
            None => self.tree_node(i).keystone_mod.as_ref(),
        }
    }

    /// `node.isTattoo`
    pub fn is_tattoo(&self, i: usize) -> bool {
        self.nodes[i].overlay.as_ref().is_some_and(|o| o.is_tattoo)
    }

    /// `node.overrideType`
    pub fn override_type(&self, i: usize) -> Option<&Arc<str>> {
        self.nodes[i].overlay.as_ref()?.override_type.as_ref()
    }
}
