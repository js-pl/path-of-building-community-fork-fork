//! The passive tree of the calculation with the items: the part of
//! `calcs.initEnv` (`Modules/CalcSetup.lua`) after the items, which merges
//! the item database into the player's, allocates the passives items grant
//! (anoints, Forbidden Flame and Flesh) and adds the mods of the allocated
//! nodes with the radius jewels. When that gives jewels more functions
//! (`ExtraJewelFunc`), upstream runs `initEnv` again with them, and
//! [`setup`] sets the items and the tree up again.
//!
//! Not ported yet: passives granted from the latest tree when the build's
//! tree has no such node.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::calc_items::{ItemsInput, ItemsSetup, setup_items};
use crate::calc_setup::{NodeMods, build_mod_list_for_node_list};
use crate::mods::list::{Precision, on_store};
use crate::mods::tag::TagKind;
use crate::mods::{Mod, ModType, ModValue, Mods, Store, Sym};
use crate::spec::{OrderedMap, PassiveSpec};
use std::collections::{BTreeSet, HashMap};

pub type Result<T> = std::result::Result<T, String>;

/// The allocated nodes and their mods
#[derive(Debug)]
pub struct TreeSetup {
    /// `env.allocNodes`: each node's index in the spec, by id
    pub alloc: OrderedMap<usize>,
    /// `env.grantedPassives`
    pub granted_passives: BTreeSet<u32>,
    /// The allocated nodes' mods, added to `env.modDB`
    pub mods: Vec<Mod>,
    /// Each allocated node's own mods
    pub node_mods: Vec<(u32, NodeMods)>,
}

/// `env.modDB` as the items and the tree leave it, with the items and the
/// tree
#[derive(Debug)]
pub struct EnvSetup {
    pub mod_db: Store,
    pub items: ItemsSetup,
    pub tree: TreeSetup,
}

/// `modDB:List(nil, name)`
fn db_list(db: &Store, name: &str) -> Result<Vec<ModValue>> {
    on_store(db.clone(), |q, id| q.list(id, None, &[Sym::new(name)]))
}

/// What the items add to `env.modDB`, then `mergeDB(env.modDB,
/// env.itemModDB)`
fn merge_items(db: &mut Store, items: &ItemsSetup) {
    for m in &items.player_mods {
        db.add(m.clone());
    }
    for (k, v) in &items.player_multipliers {
        db.multipliers.insert(*k, *v);
    }
    if let Mods::Db(by_name) = &items.item_db.mods {
        for list in by_name.values() {
            for m in list {
                db.add(m.clone());
            }
        }
    }
    for (k, v) in &items.item_db.conditions {
        db.conditions.insert(*k, *v);
    }
    for (k, v) in &items.item_db.multipliers {
        db.multipliers.insert(*k, *v);
    }
}

/// The passives the player's mods grant, and the tree's mods with them
pub fn setup_tree(
    spec: &PassiveSpec,
    db: &Store,
    items: &mut ItemsSetup,
    precision: &dyn Precision,
) -> Result<TreeSetup> {
    let tree = spec.tree;
    let mut alloc = OrderedMap::default();
    for (id, &i) in spec.alloc_nodes.iter() {
        alloc.insert(id, i);
    }
    let mut granted_passives = BTreeSet::new();
    // Anoints and the like
    for passive in db_list(db, "GrantedPassive")? {
        let ModValue::Str(name) = passive else {
            continue;
        };
        let id = tree
            .notable_map
            .get(&*name)
            .or_else(|| tree.ascendancy_map.get(&*name));
        if let Some(&id) = id
            && let Some(&i) = spec.index.get(&id)
        {
            alloc.insert(id, i);
            granted_passives.insert(id);
            items.extra_radius_nodes.remove(id);
        }
    }
    // An ascendancy node both Forbidden Flame and Flesh name, for the
    // class they are both for
    let class_name = spec
        .cur_class_id
        .and_then(|c| tree.classes.iter().find(|(id, _)| *id == c))
        .map(|(_, c)| c.name.to_string());
    let restricted = |k: &str| items.class_restrictions.get(k).cloned();
    let both_for_class = class_name.is_some()
        && restricted("ForbiddenFlesh") == class_name
        && restricted("ForbiddenFlame") == class_name;
    let mut matched: HashMap<String, (Option<ModValue>, bool)> = HashMap::new();
    for v in db_list(db, "GrantedAscendancyNode")? {
        let ModValue::Record(r) = &v else {
            continue;
        };
        let name = match r.get("name") {
            Some(ModValue::Str(s)) => s.to_string(),
            _ => String::new(),
        };
        let side = r.get("side").cloned();
        match matched.get_mut(&name) {
            Some((s, done)) if *s != side && !*done => {
                *done = true;
                if let Some(&id) = tree.ascendancy_map.get(&*name)
                    && let Some(&i) = spec.index.get(&id)
                    && both_for_class
                {
                    alloc.insert(id, i);
                    granted_passives.insert(id);
                }
            }
            _ => {
                matched.insert(name, (side, false));
            }
        }
    }
    let nodes: Vec<usize> = alloc.iter().map(|(_, &i)| i).collect();
    let extra: Vec<usize> = items
        .extra_radius_nodes
        .iter()
        .filter_map(|(id, _)| spec.index.get(&id).copied())
        .collect();
    let (mods, node_mods) = build_mod_list_for_node_list(
        spec,
        &nodes,
        &alloc,
        &mut items.radius_jewels,
        &extra,
        true,
        precision,
    )?;
    Ok(TreeSetup {
        alloc,
        granted_passives,
        mods,
        node_mods,
    })
}

/// `override.extraJewelFuncs`: the mods of `env.modDB:Tabulate("LIST", nil,
/// "ExtraJewelFunc")`. Their conditions are that the jewel is not unique,
/// which hold without a jewel.
fn extra_jewel_funcs(db: &Store) -> Result<Vec<Mod>> {
    let Mods::Db(by_name) = &db.mods else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for m in by_name
        .get(&Sym::new("ExtraJewelFunc"))
        .into_iter()
        .flatten()
    {
        if m.typ != ModType::List {
            continue;
        }
        for t in &m.tags {
            match &t.kind {
                TagKind::ItemCondition(c) if c.neg && !c.all_slots && !c.both_slots => {}
                _ => return Err(format!("extra jewel function tag {t:?}")),
            }
        }
        out.push(m.clone());
    }
    Ok(out)
}

/// The items and the tree on the player's database before them
pub fn setup(base: &Store, input: &ItemsInput, precision: &dyn Precision) -> Result<EnvSetup> {
    let mut items = setup_items(input)?;
    let mut db = base.clone();
    merge_items(&mut db, &items);
    let tree = setup_tree(input.spec, &db, &mut items, precision)?;
    for m in &tree.mods {
        db.add(m.clone());
    }
    if input.extra_jewel_funcs.is_empty() {
        let extra = extra_jewel_funcs(&db)?;
        if !extra.is_empty() {
            let again = ItemsInput {
                extra_jewel_funcs: &extra,
                ..*input
            };
            return setup(base, &again, precision);
        }
    }
    Ok(EnvSetup {
        mod_db: db,
        items,
        tree,
    })
}
