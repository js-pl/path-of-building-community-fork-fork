//! The game data the typed engine reads, as the `gamedata` crate makes it
//! from upstream's data files at build time, in the binary form of
//! [`crate::codec`].

use crate::calc_setup::SetupData;
use crate::codec;
use crate::config::ConfigData;
use crate::config_tab::OptionDefault;
use crate::item_data::ItemData;
use crate::skill_data::SkillData;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameData {
    pub items: ItemData,
    pub skills: SkillData,
    pub setup: SetupData,
    pub config: ConfigData,
    /// What a new config set gives each option
    pub option_defaults: Vec<OptionDefault>,
    /// The Configuration tab's sections and options
    pub config_vars: Vec<crate::config_tab::ConfigVar>,
    /// `data.jewelRadius` labels on the latest tree
    pub jewel_radius: Vec<String>,
    /// `treeVersionList`, oldest first; the last is the latest
    pub tree_versions: Vec<String>,
    /// `treeVersions[v]` of each of `tree_versions`
    pub tree_version_info: Vec<TreeVersion>,
    /// `poePlannerVersions`: poeplanner's tree version numbers
    pub poe_planner_versions: Vec<(f64, String)>,
    /// `data.monsterExperienceLevelMap`: each area level's effective
    /// monster level for experience
    pub monster_experience_levels: Vec<f64>,
    /// `data.uniques`: each type's item texts, in `pairs` order, with the
    /// uniques `Generated.lua` adds before a tree loads as `generated`
    pub uniques: Vec<(String, Vec<String>)>,
    /// `data.rares`
    pub rares: Vec<String>,
    /// `data.itemBaseTypeList` and each type's `data.itemBaseLists` (the
    /// bases' names)
    pub item_base_lists: Vec<(String, Vec<String>)>,
    /// `data.enchantments` by key (a base type, a weapon type, `Flask` or
    /// `UtilityFlask`)
    pub enchantments: Vec<(String, Enchantments)>,
    /// `data.vestigialModMappings`: the unique title of each vestigial mod
    /// id, by id
    pub vestigial_mod_mappings: Vec<(String, String)>,
    /// The mods of the Add Modifier popup
    pub craft: crate::item_data::CraftData,
}

/// Enchantment lines by source
pub type SourceLines = Vec<(String, Vec<String>)>;

/// An entry of `data.enchantments`: the lines of each enchantment source
/// (`HARVEST`, `ENDGAME`, ...), or of each skill's sources for helmets
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Enchantments {
    /// Lines by source, in the file's order
    pub sources: SourceLines,
    /// Sources by skill name, sorted by name
    pub skills: Vec<(String, SourceLines)>,
}

crate::codec_struct!(Enchantments { sources, skills });

impl Enchantments {
    /// The lines of a source in `list`
    pub fn lines<'a>(list: &'a [(String, Vec<String>)], source: &str) -> Option<&'a [String]> {
        list.iter()
            .find(|(s, _)| s == source)
            .map(|(_, l)| l.as_slice())
    }
}

crate::codec_struct!(GameData {
    items,
    skills,
    setup,
    config,
    option_defaults,
    config_vars,
    jewel_radius,
    tree_versions,
    tree_version_info,
    poe_planner_versions,
    monster_experience_levels,
    uniques,
    rares,
    item_base_lists,
    enchantments,
    vestigial_mod_mappings,
    craft,
});

impl GameData {
    /// Reads the data from its binary form.
    pub fn decode(bytes: &[u8]) -> codec::Result<GameData> {
        codec::decode(bytes)
    }

    pub fn encode(&self) -> Vec<u8> {
        codec::encode(self)
    }

    /// `data.enchantments[key]`
    pub fn enchantments(&self, key: &str) -> Option<&Enchantments> {
        self.enchantments
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, e)| e)
    }
}

impl GameData {
    pub fn latest_tree_version(&self) -> &str {
        self.tree_versions.last().map_or("", String::as_str)
    }

    /// `defaultTreeVersion`: the version multiple trees came with
    pub fn default_tree_version(&self) -> &str {
        self.tree_versions.get(1).map_or("", String::as_str)
    }

    /// `treeVersions[v]`
    pub fn tree_version(&self, v: &str) -> Option<&TreeVersion> {
        let i = self.tree_versions.iter().position(|x| x == v)?;
        self.tree_version_info.get(i)
    }
}

/// `treeVersions[v]`: a tree version's name, number and the official
/// tree's link prefix
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeVersion {
    pub display: String,
    pub num: f64,
    pub url: String,
}

crate::codec_struct!(TreeVersion { display, num, url });
