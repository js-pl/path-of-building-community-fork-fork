//! The typed engine's inputs for the tests: its game data and passive
//! trees, made by `gamedata` and read from their binary form, as the engine
//! gets them, and builds loaded on them.

use engine::game_data::GameData;
use engine::mod_parser::ModParser;
use engine::tree::{self, PassiveTree};
use std::collections::HashMap;
use std::rc::Rc;

/// Typed passive trees, by version, made by `gamedata` and read back from
/// their binary form, once each
pub struct Trees {
    src: Rc<gamedata::Source>,
    latest: String,
    trees: HashMap<String, PassiveTree>,
}

impl Trees {
    pub fn new(latest: &str) -> Trees {
        let src = gamedata::Source::load(&crate::upstream_src()).unwrap_or_else(|e| panic!("{e}"));
        Trees::with_source(Rc::new(src), latest)
    }

    fn with_source(src: Rc<gamedata::Source>, latest: &str) -> Trees {
        Trees {
            src,
            latest: latest.to_string(),
            trees: HashMap::new(),
        }
    }

    /// The tree of a version
    pub fn get(&mut self, version: &str, parser: &ModParser) -> &PassiveTree {
        if !self.trees.contains_key(version) {
            let t = self
                .src
                .tree(version, parser, version == self.latest)
                .and_then(|t| PassiveTree::decode(&t.encode()))
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            self.trees.insert(version.to_string(), t);
        }
        &self.trees[version]
    }

    pub fn latest(&self) -> &str {
        &self.latest
    }

    /// The tree of a version [`Trees::get`] loaded
    pub fn loaded(&self, version: &str) -> Option<&PassiveTree> {
        self.trees.get(version)
    }
}

/// The game data the typed engine reads, made by `gamedata` from
/// upstream's data files and read back from its binary form, with the mod
/// parser and the passive trees
pub struct TypedData {
    pub parser: ModParser,
    pub config: engine::config::ConfigData,
    pub option_defaults: Vec<engine::config_tab::OptionDefault>,
    pub setup: engine::calc_setup::SetupData,
    pub items: engine::item_data::ItemData,
    pub skills: engine::skill_data::SkillData,
    /// `data.jewelRadius` labels
    pub radius: Vec<String>,
    /// The jewel sockets of the latest tree
    pub socket_ids: Vec<u32>,
    pub trees: Trees,
    /// The timeless jewels' data, made when a build first needs it
    timeless: std::sync::OnceLock<std::sync::Arc<engine::timeless::Timeless>>,
}

impl TypedData {
    pub fn load() -> TypedData {
        let src = gamedata::Source::load(&crate::upstream_src()).unwrap_or_else(|e| panic!("{e}"));
        let bytes = src.game_data().unwrap_or_else(|e| panic!("{e}")).encode();
        let GameData {
            items,
            skills,
            setup,
            config,
            option_defaults,
            config_vars: _,
            jewel_radius,
            tree_versions,
        } = GameData::decode(&bytes).unwrap_or_else(|e| panic!("{e}"));
        let parser = src.mod_parser().unwrap_or_else(|e| panic!("{e}"));
        let latest = tree_versions.last().cloned().unwrap_or_default();
        let mut trees = Trees::with_source(Rc::new(src), &latest);
        let socket_ids = trees
            .get(&latest, &parser)
            .nodes
            .iter()
            .filter(|n| n.kind == tree::NodeType::Socket)
            .map(|n| n.id)
            .collect();
        TypedData {
            parser,
            config,
            option_defaults,
            setup,
            items,
            skills,
            radius: jewel_radius,
            socket_ids,
            trees,
            timeless: Default::default(),
        }
    }

    /// The timeless jewels' data
    pub fn timeless(&self) -> std::sync::Arc<engine::timeless::Timeless> {
        self.timeless
            .get_or_init(|| {
                let src = gamedata::Source::load(&crate::upstream_src())
                    .unwrap_or_else(|e| panic!("{e}"));
                std::sync::Arc::new(src.timeless(&self.parser).unwrap_or_else(|e| panic!("{e}")))
            })
            .clone()
    }

    /// Loads the tree a build file needs
    pub fn load_tree(&mut self, root: &engine::xml::Element) -> String {
        let version = engine::build::tree_version(root);
        self.trees.get(&version, &self.parser);
        version
    }

    /// What the calculations read
    pub fn calc_data(&self) -> engine::calc_env::CalcData<'_> {
        engine::calc_env::CalcData {
            setup: &self.setup,
            items: &self.items,
            skills: &self.skills,
            parser: &self.parser,
            jewel_radius: &self.radius,
        }
    }

    /// Loads a build file whose tree [`TypedData::load_tree`] loaded
    pub fn load_build(
        &self,
        root: &engine::xml::Element,
    ) -> Result<engine::build::Build<'_>, String> {
        let tree = self
            .trees
            .loaded(&engine::build::tree_version(root))
            .ok_or("the build's tree is not loaded")?;
        let one_hand = |t: &str| self.setup.weapon_type_info.get(t).map(|w| w.one_hand);
        let minions = |m: &str| self.setup.minions.contains_key(m);
        let ctx = engine::build::LoadContext {
            items: engine::item::ItemContext {
                data: &self.items,
                parser: &self.parser,
                jewel_radius: &self.radius,
                default_affix_quality: 0.5,
            },
            skill_data: &self.skills,
            config_data: &self.config,
            option_defaults: &self.option_defaults,
            parser: &self.parser,
            socket_ids: &self.socket_ids,
            one_hand: &one_hand,
            minions: &minions,
            timeless: engine::build::has_timeless_jewel(root).then(|| self.timeless()),
        };
        engine::build::Build::load(root, &ctx, tree, &self.parser)
    }
}
