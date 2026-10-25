//! The calculation setup of a build: `calcs.initEnv` (`Modules/CalcSetup.lua`)
//! from the typed build, the slices of `calc_setup`, `calc_items`,
//! `calc_tree`, `calc_skills` and `calc_skill_mods` in upstream's order.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::build::Build;
use crate::calc_items::ItemsInput;
use crate::calc_setup::{self, BaseInput, BuffMode, SetupData};
use crate::calc_skill_mods::{self, ModsInput};
use crate::calc_skills::{self, SkillsInput, SkillsSetup};
use crate::calc_tree::{self, EnvSetup};
use crate::item_data::ItemData;
use crate::mod_parser::ModParser;
use crate::mods::{Mod, Store, Sym};
use crate::skill_data::SkillData;
use crate::skills::{ExplodeSource, SocketGroup, update_socket_groups};
use std::collections::HashMap;

pub type Result<T> = std::result::Result<T, String>;

/// The game data the setup reads
pub struct CalcData<'a> {
    pub setup: &'a SetupData,
    pub items: &'a ItemData,
    pub skills: &'a SkillData,
    pub parser: &'a ModParser,
    /// The labels of the jewel radii (`data.jewelRadius`)
    pub jewel_radius: &'a [String],
}

/// A calculation's setup (`env` after `calcs.initEnv`)
#[derive(Debug)]
pub struct Env {
    pub mode: BuffMode,
    pub enemy_level: f64,
    /// `env.modDB` and `env.enemyDB`
    pub player_db: Store,
    pub enemy_db: Store,
    /// `env.initialNodeModDB`
    pub initial_node_mods: Vec<Mod>,
    /// The items and the tree with them
    pub setup: EnvSetup,
    /// The Skills tab's socket groups with the granted ones
    pub groups: Vec<SocketGroup>,
    pub skills: SkillsSetup,
    /// Multipliers set on `env.enemyDB` by the skills
    pub enemy_multipliers: HashMap<Sym, f64>,
}

/// `calcs.initEnv(build, mode)`. The build's Skills tab takes the socket
/// groups with the granted skills and the gems' selections.
pub fn init_env(build: &mut Build, data: &CalcData, mode: BuffMode) -> Result<Env> {
    let spec = &build.spec;
    let tree = spec.tree;
    let class_id = spec.cur_class_id.unwrap_or(0);
    let class = tree
        .classes
        .iter()
        .find(|(id, _)| *id == class_id)
        .map(|(_, c)| c)
        .ok_or_else(|| format!("no class {class_id}"))?;
    let stat = |i: usize| class.base_stats[i].unwrap_or(f64::NAN);
    let character_level = build.info.character_level;
    let enemy_level = build.config_state.enemy_level.unwrap_or_else(|| {
        let max = data
            .setup
            .misc
            .get("MaxEnemyLevel")
            .copied()
            .unwrap_or(f64::NAN);
        if character_level < max {
            character_level
        } else {
            max
        }
    });
    let config = build.config.active().input.clone();
    let base = BaseInput {
        class_stats: [stat(0), stat(1), stat(2)],
        character_level,
        enemy_level,
        config_input: &config,
        config_mods: &build.config_mods,
        config_enemy_mods: &build.config_enemy_mods,
        mode,
    };
    let (mut player_db, enemy_db) = calc_setup::base_dbs(data.setup, data.parser, &base)?;
    calc_setup::allocation_mods(spec, &mut player_db);
    // The tree before the items
    let alloc: Vec<usize> = spec.alloc_nodes.iter().map(|(_, &i)| i).collect();
    let (mut initial_node_mods, _) = calc_setup::build_mod_list_for_node_list(
        spec,
        &alloc,
        &spec.alloc_nodes,
        &mut [],
        &[],
        true,
        data.items,
    )?;
    calc_setup::merge_keystones(&mut initial_node_mods, tree, &mut vec![])?;
    // The items, and the tree with them
    let groups: Vec<SocketGroup> = build
        .skills
        .as_ref()
        .map_or(vec![], |s| s.socket_group_list().to_vec());
    let items_input = ItemsInput {
        spec,
        tab: &build.items,
        socket_groups: &groups,
        node_mods: &initial_node_mods,
        config: &config,
        precision: data.items,
        energy_blade: false,
        jewel_radius: data.jewel_radius,
        extra_jewel_funcs: &[],
    };
    let setup = calc_tree::setup(&player_db, &items_input, data.items)?;
    let mut player_db = setup.mod_db.clone();
    // The granted skills' socket groups
    let granted = calc_skills::granted_skills(
        &setup.tree.node_mods,
        &setup.items.granted_skills,
        data.skills,
    );
    let mut explode: Vec<ExplodeSource> = setup
        .tree
        .node_mods
        .iter()
        .filter(|(_, n)| n.explode)
        .map(|(id, _)| ExplodeSource::Node(*id))
        .collect();
    explode.extend(
        setup
            .items
            .explode_sources
            .iter()
            .map(|&id| ExplodeSource::Item(id)),
    );
    let mut groups = groups;
    calc_skills::add_granted_groups(&mut groups, &granted, &explode, data.skills);
    update_socket_groups(&mut groups, &build.items);
    // The active skills
    let imbued = build
        .skills
        .as_ref()
        .map(|s| s.imbued_support_by_slot.clone())
        .unwrap_or_default();
    let unarmed = data
        .setup
        .unarmed_weapon_data
        .get(&class_id)
        .cloned()
        .unwrap_or_default();
    let skills_input = SkillsInput {
        groups: &groups,
        tab: &build.items,
        item_data: data.items,
        skill_data: data.skills,
        item_list: &setup.items.item_list,
        unarmed: &unarmed,
        main_socket_group: Some(build.info.main_socket_group),
        imbued_support_by_slot: &imbued,
        matching_socket_quality_bonus: data
            .setup
            .misc
            .get("MatchingSocketQualityBonus")
            .copied()
            .unwrap_or(f64::NAN),
        enemy_db: &enemy_db,
    };
    let mut skills = calc_skills::create_active_skills(&skills_input, &mut player_db)?;
    let placeholders = build.config.active().placeholder.clone();
    let weapon_data = skills.weapon_data.clone();
    let mods_input = ModsInput {
        setup: data.setup,
        skill_data: data.skills,
        tab: &build.items,
        item_data: data.items,
        item_list: &setup.items.item_list,
        weapon_data: &weapon_data,
        mode,
        config: &config,
        placeholders: &placeholders,
        spectre_list: &build.info.spectre_list,
        character_level,
        enemy_level,
        iron_mass: setup.items.iron_mass_mods.is_some(),
        enemy_db: &enemy_db,
    };
    let mut enemy_multipliers = HashMap::new();
    calc_skill_mods::build_skill_mod_lists(
        &mut skills.active_skills,
        &mut groups,
        &mut player_db,
        &mut enemy_multipliers,
        &mods_input,
    )?;
    if let Some(list) = build
        .skills
        .as_mut()
        .and_then(|s| s.socket_group_list_mut())
    {
        *list = groups.clone();
    }
    Ok(Env {
        mode,
        enemy_level,
        player_db,
        enemy_db,
        initial_node_mods,
        setup,
        groups,
        skills,
        enemy_multipliers,
    })
}
