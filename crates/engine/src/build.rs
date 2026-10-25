//! A build file loaded as upstream's build mode loads it
//! (`Modules/Build.lua`): its `<Build>` element, then the Configuration,
//! Items and Skills tabs, and the passive tree, which upstream loads last.
//! The calculations' inputs, without the UI.
//!
//! Not ported yet: the character level's auto mode, timeless jewel data,
//! notes, party and import sections.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

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
}

/// A loaded build: what the calculations read of it
pub struct Build<'t> {
    pub info: BuildInfo,
    pub config: ConfigTab,
    /// The active config set's mods (`configTab.modList`, `enemyModList`)
    pub config_mods: Vec<Mod>,
    pub config_enemy_mods: Vec<Mod>,
    pub config_state: ConfigState,
    pub spec: PassiveSpec<'t>,
    pub items: ItemsTab,
    pub skills: Option<Skills>,
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
        let mut spec = PassiveSpec::new(tree, parser)?;
        if let Some(s) = active_spec(root) {
            spec.load(s)?;
            spec.post_load()?;
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
            config,
            config_mods,
            config_enemy_mods,
            config_state: state,
            spec,
            items,
            skills,
        })
    }
}
