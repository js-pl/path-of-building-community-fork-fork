//! Unofficial rewrite of Path of Building Community: the engine.
//!
//! Free-form ports of upstream, see docs/design.md.

pub mod actor;
pub mod breakdown;
pub mod build;
pub mod calc;
pub mod calc_cache;
pub mod calc_defence;
pub mod calc_ehp;
pub mod calc_env;
pub mod calc_items;
pub mod calc_minion;
pub mod calc_mirage;
pub mod calc_offence;
pub mod calc_offence_ailments;
pub mod calc_offence_dot;
pub mod calc_offence_hit;
pub mod calc_override;
mod calc_party;
pub mod calc_perform;
pub mod calc_setup;
pub mod calc_skill_funcs;
pub mod calc_skill_mods;
pub mod calc_skills;
pub mod calc_tree;
pub mod calc_triggers;
pub mod calcs;
pub mod calculator;
pub mod codec;
pub mod common;
pub mod compare;
pub mod config;
pub mod config_tab;
pub mod data;
pub mod game_data;
pub mod item;
pub mod item_craft;
pub mod item_data;
pub mod item_enchant;
mod item_raw;
pub mod item_tools;
pub mod items_tab;
pub mod jewel;
pub mod mod_parser;
pub mod mods;
pub mod parallel;
pub mod party;
pub mod pat;
pub mod power;
pub mod rt;
pub mod sidebar;
pub mod skill_data;
pub mod skills;
pub mod spec;
pub mod spec_cluster;
pub mod stat_describer;
pub mod timeless;
pub mod tree;
pub mod xml;

/// What calculations share can be shared between threads
const _: () = {
    const fn shared<T: Send + Sync>() {}
    shared::<game_data::GameData>();
    shared::<mod_parser::ModParser>();
    shared::<tree::PassiveTree>();
    shared::<build::Build<'static>>();
    shared::<calc_env::CalcData<'static>>();
    shared::<calculator::MiscCalculator<'static, 'static>>();
};
