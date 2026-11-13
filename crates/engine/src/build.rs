//! A build file loaded as upstream's build mode loads it
//! (`Modules/Build.lua`): its `<Build>` element, then the Configuration,
//! Items and Skills tabs, and the passive tree, which upstream loads last.
//! The calculations' inputs, without the UI.
//!
//! The character level's auto mode sets the level after a calculation
//! ([`Build::set_character_level`]). Not ported yet: notes, party and import sections, the Calcs tab's inputs other than the
//! calculations'.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::config::{self, ConfigData, ConfigState, Val};
use crate::config_tab::{ConfigTab, OptionDefault};
use crate::item::ItemContext;
use crate::items_tab::{ItemsTab, Sockets};
use crate::mod_parser::ModParser;
use crate::mods::Mod;
use crate::pat;
use crate::skill_data::SkillData;
use crate::skills::Skills;
use crate::spec::PassiveSpec;
use crate::tree::PassiveTree;
use crate::xml::Element;
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, String>;

/// The attributes of the `<Build>` element (`buildMode:Load`)
#[derive(Clone, Debug, PartialEq)]
pub struct BuildInfo {
    pub target_version: Option<String>,
    pub character_level: f64,
    pub character_level_auto_mode: bool,
    pub bandit: String,
    pub pantheon_major_god: String,
    pub pantheon_minor_god: String,
    pub main_socket_group: f64,
    /// The spectres the build has, by minion id
    pub spectre_list: Vec<String>,
}

impl BuildInfo {
    /// `minions` names the known minions; unknown spectres are dropped
    pub fn load(xml: &Element, minions: &dyn Fn(&str) -> bool) -> BuildInfo {
        let attr = |k: &str| xml.attr(k);
        let num = |k: &str| attr(k).and_then(pat::to_number);
        BuildInfo {
            target_version: attr("targetVersion").map(String::from),
            character_level: num("level").unwrap_or(1.0),
            character_level_auto_mode: attr("characterLevelAutoMode") == Some("true"),
            bandit: attr("bandit").unwrap_or("None").to_string(),
            pantheon_major_god: attr("pantheonMajorGod").unwrap_or("None").to_string(),
            pantheon_minor_god: attr("pantheonMinorGod").unwrap_or("None").to_string(),
            main_socket_group: num("mainSkillIndex")
                .or_else(|| num("mainSocketGroup"))
                .unwrap_or(1.0),
            spectre_list: xml
                .elements()
                .filter(|c| c.elem == "Spectre")
                .filter_map(|c| c.attr("id"))
                .filter(|id| minions(id))
                .map(String::from)
                .collect(),
        }
    }
}

/// The Calcs tab's inputs (`calcsTab.input`) the calculations read
#[derive(Clone, Debug, PartialEq)]
pub struct CalcsInput {
    /// `skill_number`: the socket group the tab shows
    pub skill_number: f64,
    /// `misc_buffMode`
    pub buff_mode: String,
    /// `showMinion`
    pub show_minion: bool,
}

impl Default for CalcsInput {
    fn default() -> Self {
        CalcsInput {
            skill_number: 1.0,
            buff_mode: "EFFECTIVE".into(),
            show_minion: false,
        }
    }
}

impl CalcsInput {
    /// `CalcsTab:Load`
    pub fn load(xml: &Element) -> CalcsInput {
        let mut input = CalcsInput::default();
        for e in xml.elements().filter(|e| e.elem == "Input") {
            match (e.attr("name"), e.attr("number"), e.attr("string")) {
                (Some("skill_number"), Some(n), _) => {
                    input.skill_number = pat::to_number(n).unwrap_or(1.0)
                }
                (Some("misc_buffMode"), None, Some(s)) => input.buff_mode = s.into(),
                (Some("showMinion"), None, None) => {
                    input.show_minion = e.attr("boolean") == Some("true")
                }
                _ => {}
            }
        }
        input
    }
}

/// What loading a build reads
pub struct LoadContext<'a> {
    pub items: ItemContext<'a>,
    pub skill_data: &'a SkillData,
    pub config_data: &'a ConfigData,
    pub option_defaults: &'a [OptionDefault],
    pub parser: &'a ModParser,
    /// The jewel sockets of the latest tree
    pub socket_ids: &'a [u32],
    /// `data.weaponTypeInfo[type].oneHand`
    pub one_hand: &'a dyn Fn(&str) -> Option<bool>,
    pub minions: &'a dyn Fn(&str) -> bool,
    /// The timeless jewels' data, without which conquered nodes are not
    /// ported
    pub timeless: Option<Arc<crate::timeless::Timeless>>,
}

/// A loaded build: what the calculations read of it
#[derive(Clone)]
pub struct Build<'t> {
    pub info: BuildInfo,
    /// Shared between a build's copies, as the calculations only read it
    pub config: Arc<ConfigTab>,
    /// The active config set's mods (`configTab.modList`, `enemyModList`)
    pub config_mods: Arc<Vec<Mod>>,
    pub config_enemy_mods: Arc<Vec<Mod>>,
    pub config_state: ConfigState,
    /// Shared between a build's copies, as the calculations only read it
    pub spec: Arc<PassiveSpec<'t>>,
    pub items: ItemsTab,
    pub skills: Option<Skills>,
    pub calcs: CalcsInput,
    /// The base databases of the build's calculations, shared between its
    /// copies
    pub base_dbs: crate::calc_env::BaseDbs,
}

impl Build<'_> {
    /// Sets the character level and builds the configuration's mods again
    /// (`configTab:BuildModList()`), as the level's auto mode does
    pub fn set_character_level(
        &mut self,
        level: f64,
        config_data: &crate::config::ConfigData,
        parser: &ModParser,
    ) -> Result<()> {
        self.info.character_level = level;
        let config = Arc::make_mut(&mut self.config);
        let set = config.active_mut();
        let custom = set.custom_mods.clone();
        let mut state = ConfigState::default();
        let (mods, enemy) = config::build_mod_list(
            config_data,
            parser,
            &mut set.input,
            &mut set.placeholder,
            &custom,
            &mut state,
            level,
        )?;
        self.config_mods = Arc::new(mods);
        self.config_enemy_mods = Arc::new(enemy);
        self.config_state = state;
        self.base_dbs = Default::default();
        Ok(())
    }
}

/// Whether a build file has a timeless jewel, whose nodes need the
/// timeless jewels' data
pub fn has_timeless_jewel(root: &Element) -> bool {
    pob(root)
        .and_then(|p| p.child("Items"))
        .is_some_and(|items| {
            items
                .elements()
                .any(|i| i.text().is_some_and(|t| t.contains("Timeless Jewel")))
        })
}

/// The version of the active spec's tree, which loading the build needs
pub fn tree_version(root: &Element) -> String {
    active_spec(root)
        .and_then(|s| s.attr("treeVersion"))
        .unwrap_or("2_6")
        .to_string()
}

fn pob(root: &Element) -> Option<&Element> {
    root.child("PathOfBuilding")
}

fn active_spec(root: &Element) -> Option<&Element> {
    let tree = pob(root)?.child("Tree")?;
    let specs: Vec<&Element> = tree.elements().filter(|e| e.elem == "Spec").collect();
    let active = tree
        .attr("activeSpec")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .min(specs.len());
    specs.get(active.checked_sub(1)?).copied()
}

impl<'t> Build<'t> {
    /// `configTab:BuildModList()` of the active config set
    pub fn build_config_mods(
        &mut self,
        data: &crate::config::ConfigData,
        parser: &ModParser,
    ) -> Result<()> {
        let mut state = ConfigState::default();
        let config = Arc::make_mut(&mut self.config);
        let set = config.active_mut();
        let (mods, enemy_mods) = config::build_mod_list(
            data,
            parser,
            &mut set.input,
            &mut set.placeholder,
            &set.custom_mods.clone(),
            &mut state,
            self.info.character_level,
        )?;
        self.config_mods = Arc::new(mods);
        self.config_enemy_mods = Arc::new(enemy_mods);
        self.config_state = state;
        Ok(())
    }

    /// Loads a build file. `tree` is the tree of [`tree_version`].
    pub fn load(
        root: &Element,
        ctx: &LoadContext,
        tree: &'t PassiveTree,
        parser: &'t ModParser,
    ) -> Result<Build<'t>> {
        let pob = pob(root).ok_or("not a build")?;
        let info = pob
            .child("Build")
            .map(|b| BuildInfo::load(b, ctx.minions))
            .ok_or("no Build element")?;
        let mut config = ConfigTab::new(ctx.option_defaults);
        // Legacy bandit and pantheon choices
        for (k, v) in [
            ("bandit", &info.bandit),
            ("pantheonMajorGod", &info.pantheon_major_god),
            ("pantheonMinorGod", &info.pantheon_minor_god),
        ] {
            config
                .active_mut()
                .input
                .insert(k.to_string(), Val::Str(v.as_str().into()));
        }
        if let Some(c) = pob.child("Config") {
            config = ConfigTab::load(c, ctx.option_defaults)?;
        }
        let mut items = match pob.child("Items") {
            Some(x) => ItemsTab::load(x, &ctx.items, ctx.socket_ids)?,
            None => ItemsTab::load(&Element::default(), &ctx.items, ctx.socket_ids)?,
        };
        let skills = pob.child("Skills").map(|x| Skills::load(x, ctx.skill_data));
        let calcs = pob.child("Calcs").map(CalcsInput::load).unwrap_or_default();
        let mut spec = PassiveSpec::new(tree, parser)?;
        spec.timeless = ctx.timeless.clone();
        if let Some(s) = active_spec(root) {
            spec.load_with(s, Some(&items))?;
            spec.build_cluster_jewel_graphs(&items, ctx.items.data)?;
        }
        items.populate_slots(&mut spec, tree as &dyn Sockets, ctx.one_hand);
        let mut state = ConfigState::default();
        let set = config.active_mut();
        let (config_mods, config_enemy_mods) = config::build_mod_list(
            ctx.config_data,
            ctx.parser,
            &mut set.input,
            &mut set.placeholder,
            &set.custom_mods.clone(),
            &mut state,
            info.character_level,
        )?;
        Ok(Build {
            info,
            config: Arc::new(config),
            config_mods: Arc::new(config_mods),
            config_enemy_mods: Arc::new(config_enemy_mods),
            config_state: state,
            spec: Arc::new(spec),
            items,
            skills,
            calcs,
            base_dbs: Default::default(),
        })
    }
}
