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

    /// The tree of a version [`Trees::get`] loaded
    pub fn loaded(&self, version: &str) -> Option<&PassiveTree> {
        self.trees.get(version)
    }
}

/// The game data the typed engine reads, taken from a started upstream
/// host, with the mod parser and the passive trees
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
}

impl TypedData {
    pub fn load(host: &upstream::host::Host) -> TypedData {
        use lua::{Value, rt};
        let get = |v: &Value, k: &str| rt::index(v, &Value::str(k)).unwrap_or_default();
        let parser = crate::mod_parser();
        let mut im = importer::Importer::new(&crate::upstream_src());
        let cache =
            importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
        parser.preload(&cache);
        let ldata = host.global("data");
        let config = engine::config::ConfigData::from_lua(&ldata, &host.global("SkillType"))
            .unwrap_or_else(|e| panic!("config data: {e}"));
        let var_list = rt::call(
            &host.global("LoadModule"),
            &[Value::str("Modules/ConfigOptions")],
        )
        .unwrap_or_else(|e| panic!("{}", lua::to_string(e.value())))
        .into_iter()
        .next()
        .unwrap();
        let option_defaults = engine::config_tab::option_defaults(&var_list);
        let setup = engine::calc_setup::SetupData::from_lua(
            &ldata,
            &host.global("ModFlag"),
            &host.global("KeywordFlag"),
        )
        .unwrap_or_else(|e| panic!("setup data: {e}"));
        let items = engine::item_data::ItemData::from_lua(&ldata, &host.global("colorCodes"))
            .unwrap_or_else(|e| panic!("item data: {e}"));
        let skills = engine::skill_data::SkillData::from_lua(
            &ldata,
            &host.global("colorCodes"),
            &host.global("SkillType"),
        )
        .unwrap_or_else(|e| panic!("skill data: {e}"));
        let radius = get(&ldata, "jewelRadius")
            .as_table()
            .map(|t| t.entries())
            .unwrap_or_default()
            .into_iter()
            .map(|(_, r)| {
                lua::to_string(&get(&r, "label"))
                    .to_str_lossy()
                    .into_owned()
            })
            .collect();
        let mut trees =
            Trees::new(&lua::to_string(&host.global("latestTreeVersion")).to_str_lossy());
        let latest = trees.latest().to_string();
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
            radius,
            socket_ids,
            trees,
        }
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
        };
        engine::build::Build::load(root, &ctx, tree, &self.parser)
    }
}
