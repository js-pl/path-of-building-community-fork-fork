//! The calculation setup of a build: `calcs.initEnv` (`Modules/CalcSetup.lua`)
//! from the typed build, the slices of `calc_setup`, `calc_items`,
//! `calc_tree`, `calc_skills` and `calc_skill_mods` in upstream's order.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::build::Build;
use crate::calc_items::{EnergyBlade, ItemsInput};
use crate::calc_override::{Allocation, Override, REP_ITEM_ID, TOGGLE_ITEM_ID};
use crate::calc_setup::{self, BaseInput, BuffMode, SetupData};
use crate::calc_skill_mods::{self, ModsInput};
use crate::calc_skills::{self, SkillsInput, SkillsSetup};
use crate::calc_tree::{self, EnvSetup};
use crate::config_tab::ConfigTab;
use crate::item::ItemContext;
use crate::item_data::ItemData;
use crate::mod_parser::ModParser;
use crate::mods::{Mod, Store, Sym};
use crate::skill_data::SkillData;
use crate::skills::{ExplodeSource, SocketGroup, update_socket_groups};
use std::sync::{Arc, Mutex};

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

/// Which calculation this is (`env.mode`)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CalcMode {
    /// The sidebar's (`MAIN`)
    Main,
    /// The Calcs tab's (`CALCS`), with its own main skill and buff mode
    Calcs,
    /// A calculator's (`CALCULATOR`), such as the Full DPS skills'
    Calculator,
}

impl CalcMode {
    /// The calculation's buff mode: the Calcs tab's input, or effective
    pub fn buff_mode(self, build: &Build) -> BuffMode {
        match self {
            CalcMode::Calcs => BuffMode::new(Some(&build.calcs.buff_mode)),
            _ => BuffMode::new(Some("EFFECTIVE")),
        }
    }
}

/// A calculation's setup (`env` after `calcs.initEnv`)
#[derive(Clone, Debug)]
pub struct Env {
    pub calc_mode: CalcMode,
    pub mode: BuffMode,
    pub enemy_level: f64,
    /// `env.modDB` and `env.enemyDB`
    pub player_db: Store,
    pub enemy_db: Store,
    /// `env.initialNodeModDB`
    pub initial_node_mods: Vec<Mod>,
    /// The items and the tree with them; its `mod_db` moved to `player_db`
    pub setup: EnvSetup,
    /// The Skills tab's socket groups with the granted ones
    pub groups: Vec<SocketGroup>,
    pub skills: SkillsSetup,
    /// Multipliers set on `env.enemyDB` by the skills
    pub enemy_multipliers: crate::mods::SymMap<f64>,
}

/// The base databases (`calc_setup::base_dbs`) of a build's calculations
/// by their inputs, which the entries hold on to, so that a changed input
/// is a new one
#[derive(Clone, Default)]
pub struct BaseDbs(Arc<Mutex<Vec<BaseEntry>>>);

/// The player's and the enemy's base databases
pub type Bases = (Arc<Store>, Arc<Store>);
type Dbs = Bases;

struct BaseEntry {
    key: BaseKey,
    dbs: Dbs,
}

struct BaseKey {
    config: Arc<ConfigTab>,
    mods: Arc<Vec<Mod>>,
    enemy_mods: Arc<Vec<Mod>>,
    party: Arc<crate::party::Party>,
    /// The setup data, by address
    setup: usize,
    values: [u64; 5],
    mode: BuffMode,
}

impl BaseKey {
    fn same(&self, o: &BaseKey) -> bool {
        Arc::ptr_eq(&self.config, &o.config)
            && Arc::ptr_eq(&self.mods, &o.mods)
            && Arc::ptr_eq(&self.enemy_mods, &o.enemy_mods)
            && Arc::ptr_eq(&self.party, &o.party)
            && self.setup == o.setup
            && self.values == o.values
            && self.mode == o.mode
    }
}

impl std::fmt::Debug for BaseDbs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BaseDbs({})", self.0.lock().expect("base dbs").len())
    }
}

impl BaseDbs {
    /// The base databases of a build's calculation, set up once for its
    /// inputs with [`reuse_setup`]
    fn get(&self, build: &Build, data: &CalcData, b: &BaseInput) -> Result<Dbs> {
        let make = || {
            calc_setup::base_dbs(data.setup, data.parser, b)
                .map(|(p, e)| (Arc::new(p), Arc::new(e)))
        };
        if !reuse_setup() {
            return make();
        }
        let [s, d, i] = b.class_stats.map(f64::to_bits);
        let key = BaseKey {
            config: build.config.clone(),
            mods: build.config_mods.clone(),
            enemy_mods: build.config_enemy_mods.clone(),
            party: build.party.clone(),
            setup: data.setup as *const SetupData as usize,
            values: [
                s,
                d,
                i,
                b.character_level.to_bits(),
                b.enemy_level.to_bits(),
            ],
            mode: b.mode,
        };
        let mut cache = self.0.lock().expect("base dbs");
        if let Some(e) = cache.iter().find(|e| e.key.same(&key)) {
            return Ok(e.dbs.clone());
        }
        let dbs = make()?;
        cache.push(BaseEntry {
            key,
            dbs: dbs.clone(),
        });
        Ok(dbs)
    }
}

thread_local! {
    static REUSE_SETUP: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Whether calculations that differ only in their main skill (the Full DPS
/// skills, other skills' lookups) copy one setup instead of each setting
/// up afresh; both give the same results
pub fn reuse_setup() -> bool {
    REUSE_SETUP.with(|r| r.get())
}

/// Sets [`reuse_setup`] on this thread, for comparing the two
pub fn set_reuse_setup(reuse: bool) {
    REUSE_SETUP.with(|r| r.set(reuse));
}

/// `calcs.initEnv(build, mode)`. The build's Skills tab takes the socket
/// groups with the granted skills and the gems' selections.
pub fn init_env(build: &mut Build, data: &CalcData, calc_mode: CalcMode) -> Result<Env> {
    init_env_on(build, data, calc_mode, calc_mode.buff_mode(build))
}

/// `calcs.initEnv(build, mode, override, specEnv)` whose base databases
/// (`specEnv.cachedPlayerDB`, `cachedEnemyDB`) come from a calculation in
/// the buff mode `base`: their `Buffed`, `Combat` and `Effective`
/// conditions are its
pub fn init_env_on(
    build: &mut Build,
    data: &CalcData,
    calc_mode: CalcMode,
    base: BuffMode,
) -> Result<Env> {
    init_env_with(build, data, calc_mode, base, &Override::default())
}

/// `calcs.initEnv(build, mode, override, specEnv)` with an override; the
/// base databases as [`init_env_on`]'s
pub fn init_env_with(
    build: &mut Build,
    data: &CalcData,
    calc_mode: CalcMode,
    base: BuffMode,
    ovr: &Override,
) -> Result<Env> {
    init_env_for(build, data, calc_mode, base, ovr, None)
}

/// [`init_env_with`] with `specEnv`'s cached databases: the base
/// databases are the parents of the calculation's own, as in
/// `calcs.calcFullDPS`
/// `calcs.calcFullDPS`; `bases` are the databases an earlier calculation
/// changed (`ReplaceMod` on a base mod), else the build's
pub fn init_env_cached(
    build: &mut Build,
    data: &CalcData,
    calc_mode: CalcMode,
    base: BuffMode,
    ovr: &Override,
    bases: Option<Bases>,
) -> Result<Env> {
    init_env_for(build, data, calc_mode, base, ovr, Some(bases))
}

fn init_env_for(
    build: &mut Build,
    data: &CalcData,
    calc_mode: CalcMode,
    base: BuffMode,
    ovr: &Override,
    cached: Option<Option<Bases>>,
) -> Result<Env> {
    if let Some((_, Some(item))) = &ovr.rep_item {
        let mut item = item.clone();
        item.id = Some(REP_ITEM_ID as f64);
        build
            .items
            .items
            .insert(REP_ITEM_ID, std::sync::Arc::new(item));
    }
    if let Some(item) = &ovr.toggle_item {
        let mut item = item.clone();
        item.id = Some(TOGGLE_ITEM_ID as f64);
        build
            .items
            .items
            .insert(TOGGLE_ITEM_ID, std::sync::Arc::new(item));
    }
    match init_env_pass(build, data, calc_mode, base, ovr, false, cached.clone())? {
        Some(env) => Ok(env),
        // An enabled Energy Blade: everything again, with its condition
        None => init_env_pass(build, data, calc_mode, base, ovr, true, cached)?
            .ok_or_else(|| "Energy Blade asked for a second rerun".to_string()),
    }
}

/// One pass of `calcs.initEnv`, with `override.conditions` holding
/// `AffectedByEnergyBlade` or not; `None` when an enabled Energy Blade
/// asks for the rerun
fn init_env_pass(
    build: &mut Build,
    data: &CalcData,
    calc_mode: CalcMode,
    base_mode: BuffMode,
    ovr: &Override,
    energy_blade: bool,
    cached: Option<Option<Bases>>,
) -> Result<Option<Env>> {
    let calcs = calc_mode == CalcMode::Calcs;
    let mode = calc_mode.buff_mode(build);
    let (own_spec, added) = ovr.spec_with_nodes(&build.spec);
    let spec = own_spec.as_ref().unwrap_or(&build.spec);
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
        party_enemy_mods: &build.party.enemy_mods,
        mode: base_mode,
    };
    let (player_base, enemy_base) = match cached.clone().flatten() {
        Some(b) => b,
        None => build.base_dbs.clone().get(build, data, &base)?,
    };
    let (mut player_db, enemy_db) = if cached.is_some() {
        let child = |b| Store {
            base: Some(b),
            ..Store::db()
        };
        (child(player_base), child(enemy_base))
    } else {
        (
            Arc::unwrap_or_clone(player_base),
            Arc::unwrap_or_clone(enemy_base),
        )
    };
    for c in ovr
        .conditions
        .iter()
        .map(String::as_str)
        .chain(energy_blade.then_some("AffectedByEnergyBlade"))
    {
        player_db.conditions.insert(Sym::new(c), true);
    }
    let item_ctx = ItemContext {
        data: data.items,
        parser: data.parser,
        jewel_radius: data.jewel_radius,
        default_affix_quality: 0.5,
    };
    let allocation = Allocation::new(spec, ovr, &added);
    allocation.add_mods(&mut player_db);
    // The tree before the items
    let alloc: Vec<usize> = allocation.nodes.iter().map(|(_, &i)| i).collect();
    let (mut initial_node_mods, _) = calc_setup::build_mod_list_for_node_list(
        spec,
        &alloc,
        &allocation.nodes,
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
        alloc: &allocation.nodes,
        ovr,
        tab: &build.items,
        socket_groups: &groups,
        node_mods: &initial_node_mods,
        config: &config,
        precision: data.items,
        energy_blade: energy_blade.then_some(EnergyBlade {
            ctx: &item_ctx,
            weapon_types: &data.setup.weapon_type_info,
        }),
        jewel_radius: data.jewel_radius,
        extra_jewel_funcs: &[],
    };
    let mut setup = calc_tree::setup(&player_db, &items_input, data.items)?;
    // Energy Blade's weapons, where the calculations look items up
    build.items.generated = setup.items.generated.clone();
    // The database moves on to the skills
    let mut player_db = std::mem::replace(&mut setup.mod_db, Store::db());
    // The granted skills' socket groups, which only the sidebar's
    // calculation updates
    let mut groups = groups;
    if calc_mode == CalcMode::Main {
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
        calc_skills::add_granted_groups(&mut groups, &granted, &explode, data.skills);
    }
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
        main_socket_group: Some(if calcs {
            build.calcs.skill_number
        } else {
            build.info.main_socket_group
        }),
        calcs,
        imbued_support_by_slot: &imbued,
        matching_socket_quality_bonus: data
            .setup
            .misc
            .get("MatchingSocketQualityBonus")
            .copied()
            .unwrap_or(f64::NAN),
        enemy_db: &enemy_db,
    };
    let Some(mut skills) = calc_skills::create_active_skills(&skills_input, &mut player_db)? else {
        return Ok(None);
    };
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
        calcs,
        config: &config,
        placeholders: &placeholders,
        spectre_list: &build.info.spectre_list,
        character_level,
        enemy_level,
        iron_mass: setup.items.iron_mass_mods.is_some(),
        enemy_db: &enemy_db,
    };
    let mut enemy_multipliers = crate::mods::SymMap::default();
    calc_skill_mods::build_skill_mod_lists(
        &mut skills.active_skills,
        skills.main_skill,
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
    if calcs {
        build.calcs.skill_number = skills.main_socket_group as f64;
    }
    Ok(Some(Env {
        calc_mode,
        mode,
        enemy_level,
        player_db,
        enemy_db,
        initial_node_mods,
        setup,
        groups,
        skills,
        enemy_multipliers,
    }))
}
