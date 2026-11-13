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
    /// `data.uniques`: each type's item texts, in `pairs` order, with the
    /// uniques `Generated.lua` adds before a tree loads as `generated`
    pub uniques: Vec<(String, Vec<String>)>,
    /// `data.rares`
    pub rares: Vec<String>,
    /// `data.itemBaseTypeList` and each type's `data.itemBaseLists` (the
    /// bases' names)
    pub item_base_lists: Vec<(String, Vec<String>)>,
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
    uniques,
    rares,
    item_base_lists,
});

impl GameData {
    /// Reads the data from its binary form.
    pub fn decode(bytes: &[u8]) -> codec::Result<GameData> {
        codec::decode(bytes)
    }

    pub fn encode(&self) -> Vec<u8> {
        codec::encode(self)
    }
}

impl GameData {
    pub fn latest_tree_version(&self) -> &str {
        self.tree_versions.last().map_or("", String::as_str)
    }
}
