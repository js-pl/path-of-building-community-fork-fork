//! The tree's geometry (`tree` of `lua/session.lua`): the spec's nodes with
//! their positions and art, cluster jewel subgraphs included, the
//! connectors between them and the classes, in tree coordinates.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedSession, num};
use engine::tree::NodeType;
use serde_json::{Map, Value as J, json};
use std::collections::HashSet;

/// `classArt` of `PassiveTree.lua`, by class index
const CLASS_ART: [&str; 7] = [
    "centerscion",
    "centermarauder",
    "centerranger",
    "centerwitch",
    "centerduelist",
    "centertemplar",
    "centershadow",
];

impl TypedSession<'_> {
    /// `tree`: the geometry of the tree as the build's spec has it
    pub fn tree(&self) -> Result<J> {
        let spec = &self.build.spec;
        let tree = spec.tree;
        let mut live: Vec<(u32, usize)> = spec.order.iter().map(|(id, &i)| (id, i)).collect();
        live.sort_by_key(|&(id, _)| id);

        let mut nodes = Vec::new();
        for &(id, i) in &live {
            let tn = spec.tree_node(i);
            let (Some(x), Some(y)) = (tn.x, tn.y) else {
                continue;
            };
            let mut n = Map::new();
            n.insert("id".into(), json!(id));
            n.insert("name".into(), json!(spec.dn(i)));
            let kind = match tn.kind {
                NodeType::Socket if tn.expansion_jewel.is_some() => "ClusterSocket",
                k => k.name(),
            };
            n.insert("kind".into(), json!(kind));
            n.insert("x".into(), num(x));
            n.insert("y".into(), num(y));
            if let Some(a) = tn.ascendancy_name {
                n.insert("ascendancy".into(), json!(a.as_str()));
            }
            let stats: Vec<&str> = spec.sd(i).iter().map(|l| &**l).collect();
            n.insert("stats".into(), json!(stats));
            if tn.kind == NodeType::ClassStart
                && let Some(art) = tn
                    .class_start_index
                    .and_then(|c| CLASS_ART.get(usize::try_from(c).ok()?))
            {
                n.insert("classStart".into(), json!(art));
            }
            // A tattoo or timeless passive brings its own art (`ReplaceNode`)
            let icon = match &spec.nodes[i].overlay {
                Some(o) => o.icon.as_deref(),
                None => tn.icon.as_deref(),
            };
            if let Some(icon) = icon {
                n.insert("icon".into(), json!(icon));
            }
            if tn.kind == NodeType::Mastery && !tn.mastery_effects.is_empty() {
                n.insert("mastery".into(), json!(true));
            }
            nodes.push(J::Object(n));
        }

        // The tree's connectors, then the subgraphs', each pair once
        let mut connectors: Vec<(u32, u32, J)> = Vec::new();
        let mut seen = HashSet::new();
        let tree_pairs = tree.connectors.iter().map(|&(a, b)| {
            let asc = tree.node(a).and_then(|n| n.ascendancy_name);
            (a, b, asc)
        });
        let sub_pairs = spec
            .sub_graphs
            .iter()
            .flat_map(|(_, sg)| sg.connectors.iter().map(|&(a, b)| (a, b, None)));
        for (a, b, asc) in tree_pairs.chain(sub_pairs) {
            let (Some(&i1), Some(&i2)) = (spec.index.get(&a), spec.index.get(&b)) else {
                continue;
            };
            if !seen.insert((a.min(b), a.max(b))) {
                continue;
            }
            let mut c = Map::new();
            c.insert("a".into(), json!(a));
            c.insert("b".into(), json!(b));
            if let Some(asc) = asc {
                c.insert("ascendancy".into(), json!(asc.as_str()));
            }
            // `node.g`: a subgraph's nodes have none
            let g = |i: usize| {
                spec.nodes[i]
                    .synth
                    .is_none()
                    .then(|| spec.tree_node(i).group)
            };
            let (n1, n2) = (spec.tree_node(i1), spec.tree_node(i2));
            if g(i1) == g(i2)
                && n1.orbit == n2.orbit
                && let Some(group) = n1.group.and_then(|g| tree.groups.get(&g))
                && let Some(r) = n1
                    .orbit
                    .and_then(|o| tree.orbit_radii.get(usize::try_from(o).ok()?))
            {
                c.insert(
                    "arc".into(),
                    json!({ "x": num(group.x), "y": num(group.y), "r": num(*r) }),
                );
            }
            connectors.push((a, b, J::Object(c)));
        }
        connectors.sort_by_key(|&(a, b, _)| (a, b));

        let classes: Vec<J> = tree
            .classes
            .iter()
            .map(|(id, class)| {
                let mut ascendancies: Vec<(i64, J)> = class
                    .ascendancies
                    .iter()
                    .map(|(ai, a)| (*ai, json!({ "id": ai, "name": &*a.name })))
                    .collect();
                ascendancies.sort_by_key(|(i, _)| *i);
                let ascendancies: Vec<J> = ascendancies.into_iter().map(|(_, a)| a).collect();
                (
                    *id,
                    json!({ "id": id, "name": &*class.name, "ascendancies": ascendancies }),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect();

        let [min_x, min_y, max_x, max_y] = tree.bounds;
        Ok(json!({
            "version": &*tree.version,
            "bounds": { "minX": num(min_x), "minY": num(min_y), "maxX": num(max_x), "maxY": num(max_y) },
            "nodes": nodes,
            "connectors": connectors.into_iter().map(|(_, _, c)| c).collect::<Vec<_>>(),
            "classes": classes,
        }))
    }
}
