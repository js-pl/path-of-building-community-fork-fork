//! Calculation overrides (`CalcOverride` of `Modules/Calcs.lua`, applied in
//! `calcs.initEnv` of `Modules/CalcSetup.lua`): the changes a calculator's
//! calculation makes to the build, and the allocation they give.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::item::Item;
use crate::mods::{Mod, ModType, Store};
use crate::spec::{PassiveSpec, SpecNode, copy_table};
use crate::tree::NodeType;
use lua::ljtab::LjMap;
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, String>;

/// The id the items tab holds an override's replacement item under
pub const REP_ITEM_ID: i64 = -100;

/// The id the items tab holds an override's toggled item under
pub const TOGGLE_ITEM_ID: i64 = -101;

/// A node an override allocates
#[derive(Clone, Debug)]
pub enum OverrideNode {
    /// A node of the spec, by index
    Spec(usize),
    /// A node of its own: a mastery's effect, a cluster notable
    Own(Arc<SpecNode>),
}

/// `CalcOverride`
#[derive(Clone, Debug, Default)]
pub struct Override {
    /// `addNodes`, in the order the calculation sees them
    pub add_nodes: Vec<OverrideNode>,
    /// `removeNodes` keyed by node: allocated nodes of the spec, by index
    pub remove_nodes: Vec<usize>,
    /// `removeNodes` keyed by id: the passives items grant that stay off
    pub remove_granted: Vec<u32>,
    /// `repSlotName` and `repItem`: the slot's item, or none
    pub rep_item: Option<(String, Option<Item>)>,
    /// `toggleFlask`, `toggleTincture`: an item of the items tab, by id
    pub toggle_flask: Option<f64>,
    pub toggle_tincture: Option<f64>,
    /// `toggleFlask`, `toggleTincture` of an item outside the items tab (an
    /// item database's), held under [`TOGGLE_ITEM_ID`] and toggled by its
    /// type
    pub toggle_item: Option<Item>,
    /// `conditions` set on the player
    pub conditions: Vec<String>,
}

impl Override {
    /// Allocates a node of the spec
    pub fn add_node(mut self, i: usize) -> Self {
        self.add_nodes.push(OverrideNode::Spec(i));
        self
    }

    /// Deallocates a node of the spec
    pub fn remove_node(mut self, i: usize) -> Self {
        self.remove_nodes.push(i);
        self
    }

    /// The spec with the override's own nodes after its nodes, and each
    /// added node's index in it; `None` when there are none of its own
    pub fn spec_with_nodes<'t>(
        &self,
        spec: &PassiveSpec<'t>,
    ) -> (Option<PassiveSpec<'t>>, Vec<usize>) {
        let mut own: Option<PassiveSpec<'t>> = None;
        let mut added = Vec::with_capacity(self.add_nodes.len());
        for n in &self.add_nodes {
            match n {
                OverrideNode::Spec(i) => added.push(*i),
                OverrideNode::Own(node) => {
                    let s = own.get_or_insert_with(|| spec.clone());
                    added.push(s.nodes.len());
                    s.nodes.push((**node).clone());
                }
            }
        }
        (own, added)
    }
}

/// A mastery's effect as a node of its own (`buildMasteryEffectNode`)
pub fn mastery_effect_node(spec: &PassiveSpec, i: usize, effect: f64) -> Result<SpecNode> {
    let e = spec
        .tree
        .mastery_effects
        .get(&effect.to_bits())
        .ok_or_else(|| format!("no mastery effect {effect}"))?;
    let mut node = spec.nodes[i].clone();
    let mut sd = e.sd.clone();
    let source = format!("Tree:{}", node.id);
    let (mods, mod_list, unknown, _) =
        crate::tree::process_stats(spec.parser, &mut sd, Some(&source))?;
    node.unknown = unknown;
    node.stats = Some(std::sync::Arc::new(crate::spec::NodeStats {
        sd,
        mods,
        mod_list,
    }));
    Ok(node)
}

/// A node of the tree outside the spec (a cluster notable of
/// `clusterNodeMap`) as a node of its own
pub fn tree_node(spec: &PassiveSpec, id: u32) -> Option<SpecNode> {
    Some(SpecNode::synthetic(spec.tree.node(id)?.clone(), vec![]))
}

/// The allocated nodes of a calculation (`env.allocNodes` before the
/// granted passives) and their counts
#[derive(Clone, Debug)]
pub struct Allocation {
    /// Each node's index in the spec, by id
    pub nodes: LjMap<usize>,
    pub notable: i64,
    pub keystone: i64,
    pub mastery: i64,
    pub mastery_type_count: i64,
    /// Allocated masteries by mastery name
    pub mastery_types: Vec<(Arc<str>, i64)>,
    /// Allocated tattoos by type
    pub tattoo_types: Vec<(Arc<str>, i64)>,
}

impl Allocation {
    /// The allocation of `spec` with the override's nodes; `added` are
    /// their indices in it ([`Override::spec_with_nodes`])
    pub fn new(spec: &PassiveSpec, ovr: &Override, added: &[usize]) -> Allocation {
        let mut a = Allocation {
            nodes: LjMap::default(),
            notable: spec.allocated_notable_count as i64,
            keystone: spec.allocated_keystone_count as i64,
            mastery: spec.allocated_mastery_count as i64,
            mastery_type_count: spec.allocated_mastery_type_count as i64,
            mastery_types: spec
                .allocated_mastery_types
                .iter()
                .map(|(n, c)| (n.clone(), *c as i64))
                .collect(),
            tattoo_types: spec
                .allocated_tattoo_types
                .iter()
                .map(|(n, c)| (n.clone(), *c as i64))
                .collect(),
        };
        if ovr.add_nodes.is_empty() && ovr.remove_nodes.is_empty() {
            a.nodes = copy_table(&spec.alloc_nodes);
            return a;
        }
        for &i in added {
            a.count(spec, i, 1);
            a.nodes.insert(spec.nodes[i].id, i);
        }
        for (id, &i) in spec.alloc_nodes.iter() {
            if ovr.remove_nodes.contains(&i) {
                a.count(spec, i, -1);
            } else {
                a.nodes.insert(id, i);
            }
        }
        a
    }

    fn count(&mut self, spec: &PassiveSpec, i: usize, d: i64) {
        if spec.is_tattoo(i)
            && let Some(t) = spec.override_type(i)
        {
            match self.tattoo_types.iter_mut().find(|(n, _)| n == t) {
                Some((_, n)) => *n += d,
                None if d > 0 => self.tattoo_types.push((t.clone(), 1)),
                None => {}
            }
        }
        let node = spec.tree_node(i);
        match node.kind {
            NodeType::Mastery => {
                self.mastery += d;
                let name: Arc<str> = spec.dn(i).into();
                let name = &name;
                match self.mastery_types.iter_mut().find(|(n, _)| n == name) {
                    Some((_, n)) => {
                        let prev = *n;
                        *n += d;
                        if (d > 0 && prev == 0) || (d < 0 && *n == 0) {
                            self.mastery_type_count += d;
                        }
                    }
                    // Upstream fails on removing a mastery it does not count
                    None if d > 0 => {
                        self.mastery_types.push((name.clone(), 1));
                        self.mastery_type_count += 1;
                    }
                    None => {}
                }
            }
            NodeType::Notable => self.notable += d,
            NodeType::Keystone => self.keystone += d,
            _ => {}
        }
    }

    /// The allocation's multipliers on the player
    pub fn add_mods(&self, db: &mut Store) {
        for (name, n) in [
            ("Multiplier:AllocatedNotable", self.notable),
            ("Multiplier:AllocatedKeystone", self.keystone),
            ("Multiplier:AllocatedMastery", self.mastery),
            ("Multiplier:AllocatedMasteryType", self.mastery_type_count),
        ] {
            if n > 0 {
                db.add(Mod::new(name, ModType::Base, n as f64, None));
            }
        }
        if let Some((_, n)) = self
            .mastery_types
            .iter()
            .find(|(name, _)| &**name == "Life Mastery")
            && *n > 0
        {
            db.add(Mod::new(
                "Multiplier:AllocatedLifeMastery",
                ModType::Base,
                *n as f64,
                None,
            ));
        }
        for (t, n) in &self.tattoo_types {
            db.multipliers.insert(crate::mods::Sym::new(t), *n as f64);
        }
    }
}
