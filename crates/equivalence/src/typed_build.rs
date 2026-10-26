//! Typed parts of a build for the tests: passive trees by version, loaded
//! once.

use engine::mod_parser::ModParser;
use engine::tree::{self, PassiveTree};
use std::collections::HashMap;

/// Typed passive trees, by version
pub struct Trees {
    im: importer::Importer,
    jewel_radii: lua::Value,
    legion: Option<lua::Value>,
    latest: String,
    trees: HashMap<String, PassiveTree>,
}

impl Trees {
    pub fn new(latest: &str) -> Trees {
        let (data, _) = crate::load_data();
        Trees {
            im: importer::Importer::new(&crate::upstream_src()),
            jewel_radii: data.get("jewelRadii"),
            legion: None,
            latest: latest.to_string(),
            trees: HashMap::new(),
        }
    }

    /// The tree of a version
    pub fn get(&mut self, version: &str, parser: &ModParser) -> &PassiveTree {
        if !self.trees.contains_key(version) {
            let data = self
                .im
                .load1(&format!("TreeData/{version}/tree"), vec![])
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            let radii = tree::jewel_radii(&self.jewel_radii, version).unwrap();
            let mut t = PassiveTree::new(version, &data, &radii, parser, version == self.latest)
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            if self.legion.is_none() {
                self.legion = Some(
                    self.im
                        .load1("Data/TimelessJewelData/LegionPassives", vec![])
                        .unwrap_or_else(|e| panic!("legion passives: {e}")),
                );
            }
            t.add_legion_keystones(self.legion.as_ref().unwrap(), parser)
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            self.trees.insert(version.to_string(), t);
        }
        &self.trees[version]
    }

    pub fn latest(&self) -> &str {
        &self.latest
    }
}
