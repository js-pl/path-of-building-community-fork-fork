//! The unique and rare item databases (`main.uniqueDB`, `main.rareDB` of
//! `Modules/Main.lua`), the uniques `buildTreeDependentUniques` of
//! `Data/Uniques/Special/Generated.lua` makes from the latest tree, the
//! `itemDb` command of `lua/session.lua`, and the Forbidden jewels'
//! counterparts (`ItemsTab:AddForbiddenJewelCounterpart`,
//! `UpdateForbiddenJewelCounterpart`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedData, TypedSession};
use engine::item::{Item, ModLine};
use engine::tree::{NodeType, PassiveTree};
use serde_json::{Map, Value as J, json};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

/// An item database: its items by name (`list`)
pub type ItemDb = BTreeMap<String, Arc<Item>>;

/// `buildForbidden(tree.classNotables)`: Forbidden Flame and Flesh
fn forbidden(tree: &PassiveTree) -> Vec<String> {
    let mut classes: Vec<&str> = tree
        .class_notables
        .keys()
        .map(|c| &**c)
        .filter(|c| *c != "alternate_ascendancies")
        .collect();
    classes.sort();
    let notables = |class: &str| {
        let mut n: Vec<&str> = tree.class_notables[class].iter().map(|n| &**n).collect();
        n.sort();
        n
    };
    let mut out = Vec::new();
    for name in ["Flame", "Flesh"] {
        let mut lines = vec![
            "Rarity: UNIQUE".to_string(),
            format!("Forbidden {name}"),
            format!(
                "{} Jewel",
                if name == "Flame" { "Crimson" } else { "Cobalt" }
            ),
        ];
        for class in &classes {
            for n in notables(class) {
                lines.push(format!("Variant: ({class}) {n}"));
            }
        }
        lines.push(if name == "Flame" {
            "Source: Drops from unique{The Searing Exarch}".into()
        } else {
            "Source: Drops from unique{The Eater of Worlds}".into()
        });
        lines.push("Limited to: 1".into());
        lines.push("Item Level: 83".into());
        let other = if name == "Flame" { "Flesh" } else { "Flame" };
        let mut index = 1;
        for class in &classes {
            for n in notables(class) {
                lines.push(format!("{{variant:{index}}}Requires Class {class}"));
                lines.push(format!(
                    "{{variant:{index}}}Allocates {n} if you have the matching modifier on Forbidden {other}"
                ));
                index += 1;
            }
        }
        lines.push("Corrupted".into());
        out.push(lines.join("\n"));
    }
    out
}

/// `buildKeystoneItems(tree.keystoneMap)`: Skin of the Lords and
/// Impossible Escape
fn keystone_items(tree: &PassiveTree) -> Vec<String> {
    // Keystones added by jewels have no position in the tree data
    let mut seen = HashSet::new();
    let mut native: Vec<&str> = Vec::new();
    for id in tree.keystone_map.values() {
        let Some(node) = tree.node(*id) else { continue };
        if node.kind == NodeType::Keystone
            && !node.is_blighted
            && node.x.is_some()
            && seen.insert(*id)
        {
            native.push(&node.dn);
        }
    }
    native.sort();
    let mut skin = vec![
        "Skin of the Lords".to_string(),
        "Simple Robe".into(),
        "League: Breach".into(),
        "Source: Upgraded from unique{Skin of the Loyal} using currency{Blessing of Chayula}"
            .into(),
    ];
    // Excluded to prevent infinite loops
    let skin_keystones: Vec<&str> = native
        .iter()
        .copied()
        .filter(|n| !["Chaos Inoculation", "Necromantic Aegis"].contains(n))
        .collect();
    for n in &skin_keystones {
        skin.push(format!("Variant: {n}"));
    }
    skin.extend(
        [
            "Implicits: 0",
            "Sockets cannot be modified",
            "+2 to Level of Socketed Gems",
            "100% increased Global Defences",
            "You can only Socket Corrupted Gems in this item",
        ]
        .map(String::from),
    );
    for (i, n) in skin_keystones.iter().enumerate() {
        skin.push(format!("{{variant:{}}}{n}", i + 1));
    }
    skin.push("Corrupted".into());
    let mut escape: Vec<String> = [
        "Impossible Escape",
        "Viridian Jewel",
        "League: Sentinel",
        "Source: Drops from unique{The Maven} (Uber)",
        "Limited to: 1",
        "Radius: Small",
    ]
    .map(String::from)
    .to_vec();
    for n in &native {
        escape.push(format!("Variant: {n}"));
    }
    escape.push("Variant: Everything (QoL Test Variant)".into());
    let count = native.len() + 1;
    for (i, n) in native.iter().enumerate() {
        escape.push(format!(
            "{{variant:{},{count}}}Passive Skills in radius of {n} can be allocated without being connected to your tree",
            i + 1
        ));
    }
    escape.push("Corrupted".into());
    vec![skin.join("\n"), escape.join("\n")]
}

impl TypedData {
    /// `main.uniqueDB.list`
    pub fn unique_db(&self) -> Result<&ItemDb> {
        if let Some(db) = self.item_dbs.0.get() {
            return Ok(db);
        }
        let tree = self.tree(self.game.latest_tree_version())?;
        let mut tree_dependent = forbidden(tree);
        tree_dependent.extend(keystone_items(tree));
        let ctx = self.item_context();
        let mut db = ItemDb::new();
        for (typ, raws) in &self.game.uniques {
            let extra = if typ == "generated" {
                &tree_dependent[..]
            } else {
                &[]
            };
            for raw in raws.iter().chain(extra) {
                let item = Item::new(&ctx, raw, Some("UNIQUE"), true)?;
                if item.base.is_some() {
                    db.insert(item.name.clone(), Arc::new(item));
                }
            }
        }
        Ok(self.item_dbs.0.get_or_init(|| db))
    }

    /// `main.rareDB.list`: crafted templates with their base's implicits
    pub fn rare_db(&self) -> Result<&ItemDb> {
        if let Some(db) = self.item_dbs.1.get() {
            return Ok(db);
        }
        let ctx = self.item_context();
        let mut db = ItemDb::new();
        for raw in &self.game.rares {
            let mut item = Item::new(&ctx, raw, Some("RARE"), true)?;
            let Some(base) = item.base.clone() else {
                continue;
            };
            if item.crafted {
                if let Some(implicit) = &base.implicit
                    && item.implicit_mod_lines.is_empty()
                {
                    // Automatically add the implicits
                    for (i, line) in implicit.split('\n').filter(|l| !l.is_empty()).enumerate() {
                        let tags = base
                            .implicit_mod_types
                            .as_ref()
                            .and_then(|t| t.get(i).cloned())
                            .unwrap_or_default();
                        item.implicit_mod_lines.push(ModLine {
                            line: line.to_string(),
                            mod_tags: tags,
                            ..ModLine::default()
                        });
                    }
                }
                item.craft(&ctx)?;
            }
            db.insert(item.name.clone(), Arc::new(item));
        }
        Ok(self.item_dbs.1.get_or_init(|| db))
    }
}

impl<'d> TypedSession<'d> {
    /// `itemDb`: the unique or rare templates, by name
    pub fn item_db(&self, rare: bool) -> Result<J> {
        let db = if rare {
            self.data.rare_db()?
        } else {
            self.data.unique_db()?
        };
        let out: Vec<J> = db
            .values()
            .map(|item| {
                let mut o = Map::new();
                o.insert("name".into(), json!(item.name));
                if let Some(t) = &item.typ {
                    o.insert("type".into(), json!(t));
                }
                if let Some(b) = &item.base_name {
                    o.insert("base".into(), json!(b));
                }
                o.insert("raw".into(), json!(item.raw));
                J::Object(o)
            })
            .collect();
        Ok(J::Array(out))
    }

    /// `UpdateForbiddenJewelCounterpart(oldItem, item)`: editing one jewel
    /// moves the other to the same notable. Whether anything changed.
    pub(super) fn update_forbidden_counterpart(
        &mut self,
        old: Option<&Item>,
        item: &Item,
        other_title: &str,
    ) -> Result<bool> {
        let Some(old) = old.filter(|o| o.title == item.title) else {
            return Ok(false);
        };
        let (Some(old_name), Some(new_name)) = (variant_name(old), variant_name(item)) else {
            return Ok(false);
        };
        if old_name == new_name {
            return Ok(false);
        }
        let ctx = self.data.item_context();
        let mut changed = false;
        let ids: Vec<i64> = self.build.items.items.keys().copied().collect();
        for id in ids {
            let other = &self.build.items.items[&id];
            if other.id == item.id
                || other.title.as_deref() != Some(other_title)
                || variant_name(other).as_deref() != Some(&old_name)
            {
                continue;
            }
            let Some(at) = other
                .variant_list
                .as_ref()
                .and_then(|l| l.iter().position(|n| *n == new_name))
            else {
                continue;
            };
            let other = Arc::make_mut(self.build.items.items.get_mut(&id).unwrap());
            other.variant = Some((at + 1) as f64);
            let raw = other.build_raw();
            other.parse_raw(&ctx, &raw, None, false)?;
            other.build_mod_list(&ctx)?;
            changed = true;
        }
        Ok(changed)
    }

    /// `AddForbiddenJewelCounterpart(item)`: the other jewel of the pair
    /// with the same notable, from the unique database, unless the build
    /// has it
    pub(super) fn add_forbidden_counterpart(
        &mut self,
        item: &Item,
        other_title: &str,
    ) -> Result<()> {
        let Some(name) = variant_name(item) else {
            return Ok(());
        };
        if self.build.items.items.values().any(|o| {
            o.title.as_deref() == Some(other_title) && variant_name(o).as_deref() == Some(&name)
        }) {
            return Ok(());
        }
        let Some(db_item) = self
            .data
            .unique_db()?
            .values()
            .find(|u| u.title.as_deref() == Some(other_title))
            .cloned()
        else {
            return Ok(());
        };
        let ctx = self.data.item_context();
        let mut new = Item::new(&ctx, &db_item.raw, None, false)?;
        let Some(at) = new
            .variant_list
            .as_ref()
            .and_then(|l| l.iter().position(|n| *n == name))
        else {
            return Ok(());
        };
        new.variant = Some((at + 1) as f64);
        let raw = new.build_raw();
        new.parse_raw(&ctx, &raw, None, false)?;
        new.normalise_quality();
        self.add_item(new, true)?;
        Ok(())
    }
}

/// `item.variantList[item.variant]`
fn variant_name(item: &Item) -> Option<String> {
    let v = item.variant?;
    if v < 1.0 || v.fract() != 0.0 {
        return None;
    }
    item.variant_list.as_ref()?.get(v as usize - 1).cloned()
}
