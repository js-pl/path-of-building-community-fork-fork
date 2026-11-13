//! What the build mode's sidebar reads of the sidebar's calculation
//! (`calcsTab.mainEnv`) besides the outputs: the main skills' flags and
//! messages, the socket groups' active skills for the skill selection,
//! the item warnings and the aspects in use (`buildMode:RefreshStatList`,
//! `RefreshSkillSelectControls`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::calc::{ENEMY, PLAYER};
use crate::calc_items::ItemWarnings;
use crate::calc_perform::{Perform, Result};
use crate::mods::{Mod, ModType, ModValue, Mods, Names, Store, StoreId, Sym, TagKind};
use std::collections::BTreeSet;
use std::sync::Arc;

/// A main skill as the sidebar shows it
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillInfo {
    pub flags: BTreeSet<String>,
    pub info_message: Option<String>,
    pub info_message2: Option<String>,
    pub disable_reason: Option<String>,
}

impl SkillInfo {
    fn of(m: &crate::calc_skill_mods::SkillMods) -> SkillInfo {
        SkillInfo {
            flags: m.skill_flags.clone(),
            info_message: m.info_message.clone(),
            info_message2: m.info_message2.clone(),
            disable_reason: m.disable_reason.clone(),
        }
    }

    pub fn flag(&self, f: &str) -> bool {
        self.flags.contains(f)
    }
}

/// An active skill of a socket group (`displaySkillList`)
#[derive(Clone, Debug, PartialEq)]
pub struct DisplaySkill {
    pub granted_effect: Arc<crate::skill_data::GrantedEffect>,
    /// The gem it comes from (`activeEffect.srcInstance`): group and place
    pub src: Option<(usize, usize)>,
    pub flags: BTreeSet<String>,
    pub stages_max: Option<f64>,
    pub stages_min: Option<f64>,
    /// `activeSkill.minionList`
    pub minion_list: Vec<String>,
    /// The names of its minion's skills (`minion.activeSkillList`)
    pub minion_skills: Option<Vec<String>>,
}

/// What the sidebar reads of the sidebar's calculation
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sidebar {
    pub main_skill: SkillInfo,
    /// `env.minion.mainSkill`
    pub minion_skill: Option<SkillInfo>,
    /// Each socket group's active skills
    pub groups: Vec<Vec<DisplaySkill>>,
    /// `env.mainSocketGroup`
    pub main_socket_group: usize,
    pub item_warnings: ItemWarnings,
    /// The player's `Condition:AspectOfTheSpiderActive`, then Cat's
    /// Agility or Stealth, then Avian's Flight or Might
    pub aspects: [bool; 3],
    /// The same of `env.minion`
    pub minion_aspects: [bool; 3],
    /// `env.grantedPassives`
    pub granted_passives: BTreeSet<u32>,
    /// Whether each socket group's slot is in use (`group.slotEnabled`)
    pub slot_enabled: Vec<bool>,
    /// The player's `AdditionalRingSlot` flag, which shows the third ring
    pub additional_ring_slot: bool,
    /// Whether Saviour's mirage replaced the main skill, whose gem level
    /// and quality outputs are not ported
    pub main_copy: bool,
    /// What the configuration's options are shown by
    pub used: ConfigUsed,
}

/// What `calcs.buildOutput` records of the sidebar's calculation for the
/// Configuration tab (`env.skillsUsed`, `env.conditionsUsed`, ...), and
/// the rest of `mainEnv` the options' `ifFlag`, `ifCondTrue`,
/// `ifSkillFlag` and `ifSkillData` read
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigUsed {
    pub skills: BTreeSet<String>,
    pub conditions: BTreeSet<String>,
    pub minion_conditions: BTreeSet<String>,
    pub enemy_conditions: BTreeSet<String>,
    pub multipliers: BTreeSet<String>,
    pub enemy_multipliers: BTreeSet<String>,
    pub per_stats: BTreeSet<String>,
    pub enemy_per_stats: BTreeSet<String>,
    pub tag_types: BTreeSet<String>,
    pub mods: BTreeSet<String>,
    /// The player's `modDB.conditions` that are set
    pub player_conditions: BTreeSet<String>,
    /// The flags `mainSkill.skillModList:Flag(nil, name)` gives
    pub main_skill_mod_flags: BTreeSet<String>,
    /// The flags and the skill data some active skill of the player has
    pub skill_flags: BTreeSet<String>,
    pub skill_data: BTreeSet<String>,
    /// `env.keystonesAdded`
    pub keystones_added: Vec<String>,
}

/// A tag's `type`
fn tag_type(t: &TagKind) -> &str {
    match t {
        TagKind::Multiplier(_) => "Multiplier",
        TagKind::MultiplierThreshold(_) => "MultiplierThreshold",
        TagKind::PerStat(_) => "PerStat",
        TagKind::PercentStat(_) => "PercentStat",
        TagKind::StatThreshold(_) => "StatThreshold",
        TagKind::DistanceRamp(_) => "DistanceRamp",
        TagKind::MeleeProximity(_) => "MeleeProximity",
        TagKind::Limit { .. } => "Limit",
        TagKind::Condition { .. } => "Condition",
        TagKind::ActorCondition { .. } => "ActorCondition",
        TagKind::ItemCondition(_) => "ItemCondition",
        TagKind::SocketedIn(_) => "SocketedIn",
        TagKind::SkillName { .. } => "SkillName",
        TagKind::SkillId(_) => "SkillId",
        TagKind::SkillPart { .. } => "SkillPart",
        TagKind::SkillType { .. } => "SkillType",
        TagKind::BaseFlag { .. } => "BaseFlag",
        TagKind::SlotName { .. } => "SlotName",
        TagKind::ModFlagOr(_) => "ModFlagOr",
        TagKind::KeywordFlagAnd(_) => "KeywordFlagAnd",
        TagKind::MonsterTag { .. } => "MonsterTag",
        TagKind::Other { kind, .. } => kind.as_str(),
    }
}

/// `var` of a tag (`varList` is not read)
fn one(n: &Option<Names>) -> Option<Sym> {
    match n {
        Some(Names::One(s)) => Some(*s),
        _ => None,
    }
}

/// Whether a lowercase mod name has four letters, each of one of these
/// sets, in a row (the Lua patterns `[fi][ig][rn][ei]t?e?` and
/// `[cf][or][le][de]z?e?`)
fn has_run(name: &str, sets: [&[u8]; 4]) -> bool {
    let b = name.as_bytes();
    b.windows(4)
        .any(|w| w.iter().zip(sets).all(|(c, s)| s.contains(c)))
}

impl ConfigUsed {
    fn add(set: &mut BTreeSet<String>, m: &Mod, var: Sym) {
        // `Base` mods only describe others
        if m.source.as_deref() != Some("Base") {
            set.insert(var.as_str().to_string());
        }
    }

    /// `addModTags(actor, mod)`
    fn mod_tags(&mut self, player: bool, m: &Mod) {
        Self::add(&mut self.mods, m, m.name);
        let lower = m.name.as_str().to_lowercase();
        if has_run(&lower, [b"fi", b"ig", b"rn", b"ei"]) {
            Self::add(&mut self.enemy_conditions, m, Sym::new("Ignited"));
            Self::add(&mut self.enemy_conditions, m, Sym::new("Burning"));
        }
        if has_run(&lower, [b"cf", b"or", b"le", b"de"]) {
            Self::add(&mut self.enemy_conditions, m, Sym::new("Frozen"));
        }
        for t in &m.tags {
            Self::add(&mut self.tag_types, m, Sym::new(tag_type(&t.kind)));
            match &t.kind {
                TagKind::Other { kind, .. } if kind.as_str() == "IgnoreCond" => break,
                TagKind::Condition { vars, .. } => {
                    let set = if player {
                        &mut self.conditions
                    } else {
                        &mut self.minion_conditions
                    };
                    for v in vars.iter() {
                        Self::add(set, m, v);
                    }
                }
                TagKind::ActorCondition { actor, vars, .. } => {
                    if let Some(v) = one(vars) {
                        if actor.is_some_and(|a| a.as_str() == "enemy") {
                            Self::add(&mut self.enemy_conditions, m, v);
                        } else {
                            Self::add(&mut self.conditions, m, v);
                        }
                    }
                }
                TagKind::Multiplier(crate::mods::tag::Multiplier { vars, actor, .. })
                | TagKind::MultiplierThreshold(crate::mods::tag::MultiplierThreshold {
                    vars,
                    actor,
                    ..
                }) => {
                    let set = match actor {
                        None if player => &mut self.multipliers,
                        Some(a) if a.as_str() == "enemy" => &mut self.enemy_multipliers,
                        _ => continue,
                    };
                    for v in vars.iter().flat_map(Names::iter) {
                        Self::add(set, m, v);
                    }
                }
                TagKind::PerStat(crate::mods::tag::PerStat { stats, actor, .. }) => {
                    self.stat_tag(player, actor.as_ref(), stats, m);
                }
                TagKind::StatThreshold(crate::mods::tag::StatThreshold { stats, .. }) => {
                    self.stat_tag(player, None, stats, m);
                }
                _ => {}
            }
        }
    }

    /// `addStatTag`: `stat`, as the tags have no `varList`
    fn stat_tag(&mut self, player: bool, actor: Option<&Sym>, stats: &Option<Names>, m: &Mod) {
        let set = match actor {
            None if player => &mut self.per_stats,
            Some(a) if a.as_str() == "enemy" => &mut self.enemy_per_stats,
            _ => return,
        };
        if let Some(s) = one(stats) {
            Self::add(set, m, s);
        }
    }

    /// The enemy database's tags
    fn enemy_tags(&mut self, m: &Mod) {
        for t in &m.tags {
            match &t.kind {
                TagKind::Other { kind, .. } if kind.as_str() == "IgnoreCond" => break,
                TagKind::Condition { vars, .. } => {
                    for v in vars.iter() {
                        Self::add(&mut self.enemy_conditions, m, v);
                    }
                }
                TagKind::ActorCondition { actor, vars, .. } => {
                    if let Some(v) = one(vars) {
                        if actor.is_some_and(|a| matches!(a.as_str(), "enemy" | "player")) {
                            Self::add(&mut self.conditions, m, v);
                        } else {
                            Self::add(&mut self.enemy_conditions, m, v);
                        }
                    }
                }
                TagKind::Multiplier(crate::mods::tag::Multiplier { vars, actor, .. })
                | TagKind::MultiplierThreshold(crate::mods::tag::MultiplierThreshold {
                    vars,
                    actor,
                    ..
                }) if actor.is_none() => {
                    for v in vars.iter().flat_map(Names::iter) {
                        Self::add(&mut self.enemy_multipliers, m, v);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The mods a store holds itself (`modDB.mods`, a mod list's entries),
/// then those of its cached base
fn own_mods<'s>(s: &'s Store, out: &mut Vec<&'s Mod>) {
    match &s.mods {
        Mods::Db(by_name) => out.extend(by_name.values().flatten()),
        Mods::List(list) => out.extend(list.iter()),
    }
    if let Some(b) = &s.base {
        own_mods(b, out);
    }
}

/// The names of the flag mods a store's queries reach
fn flag_names(p: &Perform, id: StoreId, out: &mut BTreeSet<Sym>) {
    let mut s = Some(id);
    while let Some(id) = s {
        let store = p.c.store(id);
        let mut mods = Vec::new();
        own_mods(store, &mut mods);
        out.extend(
            mods.iter()
                .filter(|m| m.typ == ModType::Flag)
                .map(|m| m.name),
        );
        s = store.parent;
    }
}

/// `env.skillsUsed`, `env.conditionsUsed`, ... of `calcs.buildOutput`
fn config_used(p: &Perform) -> Result<ConfigUsed> {
    let mut u = ConfigUsed::default();
    let skill_flags = |i: usize| {
        let skill = p.skill(i);
        skill
            .mods
            .as_ref()
            .map_or(&skill.skill_flags, |m| &m.skill_flags)
    };
    for i in 0..p.n_player {
        let skill = p.skill(i);
        for e in &skill.effect_list {
            u.skills.insert(e.granted_effect.name.clone());
        }
        u.skill_flags.extend(skill_flags(i).iter().cloned());
        if let Some(m) = &skill.mods {
            for (k, v) in &m.skill_data {
                if !matches!(v, ModValue::Bool(false)) {
                    u.skill_data.insert(k.clone());
                }
            }
        }
        if let Some(m) = p.minions.iter().find(|m| m.skill == i) {
            for &k in &m.skills {
                u.skills
                    .insert(p.skill(k).active_effect.granted_effect.id.clone());
            }
        }
    }
    let mut actors = vec![(true, PLAYER)];
    actors.extend(p.minion_actor().map(|a| (false, a)));
    for (player, a) in actors {
        let mut mods = Vec::new();
        own_mods(p.c.store(p.c.db(a)), &mut mods);
        for m in mods {
            u.mod_tags(player, m);
        }
    }
    for i in 0..p.n_player {
        let lists = &p.lists[i];
        let mut mods = Vec::new();
        own_mods(p.c.store(lists.base), &mut mods);
        for m in mods {
            u.mod_tags(true, m);
        }
        let mut mods = Vec::new();
        own_mods(p.c.store(lists.list), &mut mods);
        for m in mods {
            ConfigUsed::add(&mut u.mods, m, m.name);
            for t in &m.tags {
                ConfigUsed::add(&mut u.tag_types, m, Sym::new(tag_type(&t.kind)));
            }
        }
        if let Some(minion) = p.minions.iter().find(|m| m.skill == i) {
            for &k in &minion.skills {
                let mut mods = Vec::new();
                own_mods(p.c.store(p.lists[k].base), &mut mods);
                for m in mods {
                    u.mod_tags(false, m);
                }
            }
        }
    }
    let mut mods = Vec::new();
    own_mods(p.c.store(p.c.db(ENEMY)), &mut mods);
    for m in mods {
        u.enemy_tags(m);
    }
    let pdb = p.c.store(p.c.db(PLAYER));
    u.player_conditions = pdb
        .conditions
        .iter()
        .filter(|(_, v)| **v)
        .map(|(k, _)| k.as_str().to_string())
        .collect();
    if p.main_copy.is_none() {
        let list = p.lists[p.main()].list;
        let mut names = BTreeSet::new();
        flag_names(p, list, &mut names);
        for n in names {
            if p.c.flag(list, None, &[n.as_str()])? {
                u.main_skill_mod_flags.insert(n.as_str().to_string());
            }
        }
    }
    u.keystones_added = p.keystones_added.clone();
    Ok(u)
}

fn num(v: Option<&ModValue>) -> Option<f64> {
    v.and_then(ModValue::as_number)
}

/// The sidebar's view of a finished calculation
pub(crate) fn collect(p: &Perform) -> Result<Sidebar> {
    let main_skill = match &p.main_copy {
        Some(m) => SkillInfo::of(m),
        None => SkillInfo::of(p.mods(p.main())),
    };
    let minion_skill = p
        .env_minion
        .map(|m| SkillInfo::of(p.mods(p.minions[m].main_skill)));
    let groups = p
        .env
        .skills
        .group_skills
        .iter()
        .map(|list| {
            list.iter()
                .map(|&i| {
                    let skill = p.skill(i);
                    let mods = skill.mods.as_ref();
                    let minion = p.minions.iter().find(|m| m.skill == i);
                    DisplaySkill {
                        granted_effect: skill.active_effect.granted_effect.clone(),
                        src: skill.active_effect.src,
                        flags: mods
                            .map_or_else(|| skill.skill_flags.clone(), |m| m.skill_flags.clone()),
                        stages_max: num(mods.and_then(|m| m.skill_data.get("stagesMax"))),
                        stages_min: num(mods.and_then(|m| m.skill_data.get("stagesMin"))),
                        minion_list: mods.map(|m| m.minion_list.clone()).unwrap_or_default(),
                        minion_skills: minion.map(|m| {
                            m.skills
                                .iter()
                                .map(|&k| p.skill_name(k).to_string())
                                .collect()
                        }),
                    }
                })
                .collect()
        })
        .collect();
    let aspects = |actor: usize| -> Result<[bool; 3]> {
        let db = p.c.db(actor);
        let flag = |names: &[&str]| -> Result<bool> {
            for n in names {
                let name = format!("Condition:{n}");
                if p.c.flag(db, None, &[name.as_str()])? {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        Ok([
            flag(&["AspectOfTheSpiderActive"])?,
            flag(&["CatsAgilityActive", "CatsStealthActive"])?,
            flag(&["AviansFlightActive", "AviansMightActive"])?,
        ])
    };
    Ok(Sidebar {
        main_skill,
        minion_skill,
        groups,
        main_socket_group: p.env.skills.main_socket_group,
        item_warnings: p.env.setup.items.warnings.clone(),
        aspects: aspects(PLAYER)?,
        minion_aspects: match p.minion_actor() {
            Some(a) => aspects(a)?,
            None => [false; 3],
        },
        granted_passives: p.env.setup.tree.granted_passives.clone(),
        slot_enabled: p.env.skills.slot_enabled.clone(),
        additional_ring_slot: p.c.flag(p.c.db(PLAYER), None, &["AdditionalRingSlot"])?,
        main_copy: p.main_copy.is_some(),
        used: config_used(p)?,
    })
}
