//! The engine's typed game data, made at build time from upstream's data
//! files: the importer evaluates them, [`data_lua`] does what
//! `Modules/Data.lua` does on top of them, and the engine's typed tables
//! are read from the result and encoded in its binary form
//! (`engine::codec`). No Lua runs when the engine loads it.

pub mod data_lua;
mod enchantments;
mod jsonc;
pub mod mod_parser;
mod uniques;

use engine::calc_setup::SetupData;
use engine::config::ConfigData;
use engine::config_tab::OptionDefault;
use engine::game_data::GameData;
use engine::item_data::ItemData;
use engine::mod_parser::{ModParser, ParserData};
use engine::skill_data::SkillData;
use engine::tree::PassiveTree;
use lua::Key;
use lua::{TableRef, Value};
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

/// Upstream's `data` table as `Data.lua` leaves it (the parts ported so
/// far), with the globals of `Data/Global.lua`
pub struct Source {
    pub src_dir: PathBuf,
    pub globals: TableRef,
    pub data: TableRef,
}

impl Source {
    /// Imports upstream's data files from its `src/` directory and
    /// processes them.
    pub fn load(src_dir: &Path) -> Result<Source> {
        let raw = importer::import(src_dir).map_err(|e| e.to_string())?;
        let src = Source {
            src_dir: src_dir.to_path_buf(),
            globals: raw.globals,
            data: raw.data,
        };
        data_lua::process(&src)?;
        engine::data::Data::new(src.globals.clone(), src.data.clone());
        Ok(src)
    }

    /// An importer with the globals and `data` upstream's modules see
    pub fn importer(&self) -> importer::Importer {
        let mut im = importer::Importer::new(&self.src_dir);
        for (k, v) in self.globals.entries() {
            if let Key::Str(name) = k {
                im.env.globals.insert(name.to_str_lossy().into_owned(), v);
            }
        }
        im.env
            .globals
            .insert("data".into(), Value::Table(self.data.clone()));
        im
    }

    /// `treeVersionList` of `GameVersions.lua`, oldest first
    pub fn tree_versions(&self) -> Result<Vec<String>> {
        let mut im = self.importer();
        im.load("GameVersions", vec![]).map_err(|e| e.to_string())?;
        let list = im.global("treeVersionList");
        let list = list.as_table().ok_or("no treeVersionList")?.array();
        let versions: Vec<String> = list
            .iter()
            .map(|v| v.as_str().map(|s| s.to_str_lossy().into_owned()))
            .collect::<Option<_>>()
            .ok_or("treeVersionList holds a non-string")?;
        if versions.is_empty() {
            return Err("treeVersionList is empty".into());
        }
        Ok(versions)
    }

    /// The options' defaults, from `Modules/ConfigOptions.lua` evaluated as
    /// data: its functions are ported by name in `engine::config`
    pub fn option_defaults(&self) -> Result<Vec<OptionDefault>> {
        Ok(engine::config_tab::option_defaults(&self.config_options()?))
    }

    /// The options of the Configuration tab as its controls show them
    pub fn config_vars(&self) -> Result<Vec<engine::config_tab::ConfigVar>> {
        Ok(engine::config_tab::config_vars(&self.config_options()?))
    }

    /// `Modules/ConfigOptions.lua`'s `varList`
    fn config_options(&self) -> Result<Value> {
        let mut im = self.importer();
        for g in ["string", "table"] {
            im.env
                .globals
                .entry(g.into())
                .or_insert_with(|| Value::Table(TableRef::new()));
        }
        im.load1("Modules/ConfigOptions", vec![])
            .map_err(|e| e.to_string())
    }

    /// The labels of `data.jewelRadius` for a tree version
    /// (`data.setJewelRadiiGlobally`)
    pub fn jewel_radius(&self, version: &str) -> Result<Vec<String>> {
        let mut nums = version.split('_').filter_map(|p| p.parse::<u32>().ok());
        let (major, minor) = (nums.next().unwrap_or(0), nums.next().unwrap_or(0));
        let set = if major <= 3 && minor <= 15 {
            "3_15"
        } else {
            "3_16"
        };
        let radii = self.data.get_str("jewelRadii");
        let list = radii.as_table().map(|t| t.get_str(set)).unwrap_or_default();
        list.as_table()
            .ok_or("no jewel radii")?
            .array()
            .iter()
            .map(|r| {
                r.as_table()
                    .and_then(|r| {
                        r.get_str("label")
                            .as_str()
                            .map(|s| s.to_str_lossy().into_owned())
                    })
                    .ok_or_else(|| "a jewel radius without a label".to_string())
            })
            .collect()
    }

    /// The mod parser's typed tables, with upstream's mod cache
    pub fn mod_parser_data(&self) -> Result<ParserData> {
        mod_parser::parser_data(&self.src_dir)
    }

    /// The mod parser, read from its binary form
    pub fn mod_parser(&self) -> Result<ModParser> {
        ModParser::decode(&self.mod_parser_data()?.encode())
    }

    /// A tree version's passive tree, with the timeless jewels' keystones
    pub fn tree(&self, version: &str, parser: &ModParser, latest: bool) -> Result<PassiveTree> {
        let mut im = importer::Importer::new(&self.src_dir);
        let data = im
            .load1(&format!("TreeData/{version}/tree"), vec![])
            .map_err(|e| e.to_string())?;
        let radii = engine::tree::jewel_radii(&self.data.get_str("jewelRadii"), version)?;
        let mut tree = PassiveTree::new(version, &data, &radii, parser, latest)?;
        let legion = im
            .load1("Data/TimelessJewelData/LegionPassives", vec![])
            .map_err(|e| e.to_string())?;
        tree.add_legion_keystones(&legion, parser)?;
        let tattoos = im
            .load1("Data/TattooPassives", vec![])
            .map_err(|e| e.to_string())?;
        tree.add_tattoos(&tattoos, parser)?;
        Ok(tree)
    }

    /// The timeless jewels' data, their look-up tables read from the
    /// source directory when first needed
    pub fn timeless(&self, parser: &ModParser) -> Result<engine::timeless::Timeless> {
        let mut im = importer::Importer::new(&self.src_dir);
        let legion = im
            .load1("Data/TimelessJewelData/LegionPassives", vec![])
            .map_err(|e| e.to_string())?;
        let mapping = im
            .load1("Data/TimelessJewelData/NodeIndexMapping", vec![])
            .map_err(|e| e.to_string())?;
        let src_dir = self.src_dir.clone();
        let loader = move |name: &str| -> Option<Vec<u8>> {
            miniz_oxide::inflate::decompress_to_vec_zlib(&timeless_lut_packed(&src_dir, name)?).ok()
        };
        engine::timeless::Timeless::from_lua(&legion, &mapping, parser, Box::new(loader))
    }

    pub fn global(&self, name: &str) -> Value {
        self.globals.get_str(name)
    }

    pub fn items(&self) -> Result<ItemData> {
        ItemData::from_lua(&self.value(), &self.global("colorCodes"))
            .map_err(|e| format!("item data: {e}"))
    }

    pub fn skills(&self) -> Result<SkillData> {
        SkillData::from_lua(
            &self.value(),
            &self.global("colorCodes"),
            &self.global("SkillType"),
        )
        .map_err(|e| format!("skill data: {e}"))
    }

    pub fn setup(&self) -> Result<SetupData> {
        SetupData::from_lua(
            &self.value(),
            &self.global("ModFlag"),
            &self.global("KeywordFlag"),
        )
        .map_err(|e| format!("setup data: {e}"))
    }

    pub fn config(&self) -> Result<ConfigData> {
        ConfigData::from_lua(&self.value(), &self.global("SkillType"))
            .map_err(|e| format!("config data: {e}"))
    }

    fn value(&self) -> Value {
        Value::Table(self.data.clone())
    }

    pub fn game_data(&self) -> Result<GameData> {
        let versions = self.tree_versions()?;
        Ok(GameData {
            items: self.items()?,
            skills: self.skills()?,
            setup: self.setup()?,
            config: self.config()?,
            option_defaults: self.option_defaults()?,
            config_vars: self.config_vars()?,
            jewel_radius: self.jewel_radius(versions.last().unwrap())?,
            tree_versions: versions,
            uniques: self.uniques()?,
            rares: self.rares()?,
            item_base_lists: self.item_base_lists()?,
            enchantments: self.enchantments()?,
        })
    }
}

/// A timeless jewel's look-up table as upstream ships it, by its name
/// without spaces (`LethalPride`): a zlib stream in `<name>.zip`, or split
/// over `<name>.zip.part0`, `part1`, ...
pub fn timeless_lut_packed(src_dir: &Path, name: &str) -> Option<Vec<u8>> {
    let dir = src_dir.join("Data/TimelessJewelData");
    if let Ok(b) = std::fs::read(dir.join(format!("{name}.zip"))) {
        return Some(b);
    }
    let mut parts = Vec::new();
    for k in 0.. {
        match std::fs::read(dir.join(format!("{name}.zip.part{k}"))) {
            Ok(b) => parts.extend(b),
            Err(_) => break,
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// The typed game data of upstream's `src/` directory, encoded.
pub fn build(src_dir: &Path) -> Result<Vec<u8>> {
    Ok(engine::codec::encode(&Source::load(src_dir)?.game_data()?))
}
