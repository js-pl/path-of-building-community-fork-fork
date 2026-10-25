//! The skills of the calculation setup: the part of `calcs.initEnv`
//! (`Modules/CalcSetup.lua`) that adds the skills items and passive nodes
//! grant, and the explosions on kill, to the Skills tab's socket groups,
//! then makes the player's active skills with their supports
//! (`calcs.createActiveSkill` of `Modules/CalcActiveSkill.lua`).
//!
//! Upstream marks the supports items grant as from an item on the shared
//! granted effect (`grantedEffect.fromItem = true`), so it stays marked
//! for every later calculation; here the mark holds for the calculation
//! that makes it. Not ported yet: whether supports are superseded, and
//! the gems' requirements.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::actor::{Actor, EnvActors};
use crate::calc_items::ItemSkill;
use crate::calc_setup::NodeMods;
use crate::item::Fields;
use crate::item_data::ItemData;
use crate::items_tab::ItemsTab;
use crate::mods::tag::TagKind;
use crate::mods::{Cfg, Mod, ModType, ModValue, Mods, Query, Store, StoreId, Stores, Sym};
use crate::skill_data::{Gem, GrantedEffect, SkillData};
use crate::skills::{
    ExplodeSource, GemInstance, SocketGroup, process_socket_group, validate_gem_level,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// A skill an item or a passive node grants (`env.grantedSkills`)
#[derive(Clone, Debug, PartialEq)]
pub struct GrantedSkill {
    pub skill_id: Option<String>,
    pub level: Option<f64>,
    pub no_supports: bool,
    pub source: String,
    pub triggered: bool,
    pub trigger_chance: Option<f64>,
    /// The name of the skill an item grants (`nameSpec`)
    pub name_spec: Option<String>,
    pub slot_name: Option<String>,
    pub source_item: Option<f64>,
    pub source_node: Option<u32>,
}

/// `env.grantedSkills`: the nodes' skills, in allocation order, then the
/// items'
pub fn granted_skills(
    nodes: &[(u32, NodeMods)],
    items: &[ItemSkill],
    data: &SkillData,
) -> Vec<GrantedSkill> {
    let text = |v: &Option<ModValue>| match v {
        Some(ModValue::Str(s)) => Some(s.to_string()),
        _ => None,
    };
    let mut out = Vec::new();
    for (id, node) in nodes {
        for skill in &node.granted_skills {
            out.push(GrantedSkill {
                skill_id: text(&skill.skill_id),
                level: skill.level.as_ref().and_then(ModValue::as_number),
                no_supports: true,
                source: skill.source.clone(),
                triggered: false,
                trigger_chance: None,
                name_spec: None,
                slot_name: None,
                source_item: None,
                source_node: Some(*id),
            });
        }
    }
    for s in items {
        let skill_id = s.skill.skill_id.as_deref().map(String::from);
        out.push(GrantedSkill {
            name_spec: skill_id
                .as_ref()
                .and_then(|id| data.skills.get(id))
                .map(|e| e.name.clone()),
            skill_id,
            level: s.skill.level,
            no_supports: s.skill.no_supports,
            source: s.skill.source.clone(),
            triggered: s.skill.triggered,
            trigger_chance: s.skill.trigger_chance,
            slot_name: Some(s.slot_name.clone()),
            source_item: Some(s.item_id),
            source_node: None,
        });
    }
    out
}

/// The level of a granted skill as a gem's (`getNormalizedSkillLevel`)
fn normalized_level(skill: &GrantedSkill, data: &SkillData) -> Option<f64> {
    let mut gem = GemInstance {
        granted_effect: skill
            .skill_id
            .as_ref()
            .and_then(|id| data.skills.get(id))
            .cloned(),
        level: skill.level,
        ..GemInstance::default()
    };
    validate_gem_level(&mut gem);
    gem.level
}

/// Adds a socket group for each granted skill, or updates the one from
/// before, adds the explosion group, and removes the groups of sources
/// that are gone
pub fn add_granted_groups(
    groups: &mut Vec<SocketGroup>,
    granted: &[GrantedSkill],
    explode_sources: &[ExplodeSource],
    data: &SkillData,
) {
    let mut marked: HashSet<usize> = HashSet::new();
    for skill in granted {
        let found = groups.iter().position(|g| {
            g.source.as_deref() == Some(skill.source.as_str())
                && g.slot == skill.slot_name
                && g.gem_list.first().is_some_and(|gem| {
                    gem.skill_id == skill.skill_id
                        && (gem.level == skill.level || gem.level == normalized_level(skill, data))
                })
        });
        let i = match found {
            Some(i) => i,
            None => {
                groups.push(SocketGroup {
                    label: Some(String::new()),
                    enabled: true,
                    source: Some(skill.source.clone()),
                    slot: skill.slot_name.clone(),
                    main_active_skill: 1.0,
                    main_active_skill_calcs: 1.0,
                    ..SocketGroup::default()
                });
                groups.len() - 1
            }
        };
        marked.insert(i);
        let group = &mut groups[i];
        group.source_item = skill.source_item;
        group.source_node = skill.source_node;
        let mut gem = group
            .gem_list
            .first()
            .cloned()
            .unwrap_or_else(|| GemInstance {
                skill_id: skill.skill_id.clone(),
                name_spec: skill.name_spec.clone().unwrap_or_default(),
                quality: Some(0.0),
                enabled: true,
                ..GemInstance::default()
            });
        gem.from_item = skill.source_item.is_some();
        gem.gem_id = None;
        gem.level = skill.level;
        gem.enable_global1 = true;
        gem.no_supports = skill.no_supports;
        group.no_supports = skill.no_supports;
        gem.triggered = skill.triggered;
        gem.trigger_chance = skill.trigger_chance;
        group.gem_list = vec![gem];
        process_socket_group(group, data);
    }
    if !explode_sources.is_empty() {
        let i = match groups
            .iter()
            .position(|g| g.source.as_deref() == Some("Explode"))
        {
            Some(i) => i,
            None => {
                groups.push(SocketGroup {
                    label: Some("On Kill Monster Explosion".into()),
                    enabled: true,
                    source: Some("Explode".into()),
                    no_supports: true,
                    main_active_skill: 1.0,
                    main_active_skill_calcs: 1.0,
                    ..SocketGroup::default()
                });
                groups.len() - 1
            }
        };
        let group = &mut groups[i];
        group.explode_sources = explode_sources.to_vec();
        let old = std::mem::take(&mut group.gem_list);
        for source in explode_sources {
            let gem = old
                .iter()
                .find(|g| g.explode_source.as_ref() == Some(source))
                .cloned()
                .unwrap_or_else(|| GemInstance {
                    skill_id: Some("EnemyExplode".into()),
                    quality: Some(0.0),
                    enabled: true,
                    level: Some(1.0),
                    triggered: true,
                    explode_source: Some(source.clone()),
                    ..GemInstance::default()
                });
            group.gem_list.push(gem);
        }
        marked.insert(i);
        process_socket_group(group, data);
    }
    let mut i = 0;
    groups.retain(|g| {
        let keep = g.source.is_none() || marked.contains(&i);
        i += 1;
        keep
    });
}

/// A skill's or support's effect as the setup builds it (`activeEffect`,
/// `supportEffect`)
#[derive(Clone, Debug)]
pub struct SkillEffect {
    pub granted_effect: Rc<GrantedEffect>,
    pub level: Option<f64>,
    pub quality: f64,
    pub global_quality: f64,
    pub item_quality: f64,
    pub support_quality: f64,
    pub socket_quality: f64,
    /// The attribute requirement a gem property sets (`req`)
    pub req: Option<f64>,
    /// `gemData`
    pub gem: Option<Rc<Gem>>,
    /// The gem the effect is of (`srcInstance`): its group and place
    pub src: Option<(usize, usize)>,
    /// `gemCfg`: the group's cfg with the gem's socket
    pub gem_cfg: Option<Cfg>,
    /// `gemPropertyInfo`: the gem properties that applied, value and mod
    pub gem_property_info: Vec<(ModValue, Mod)>,
}

impl SkillEffect {
    fn new(granted_effect: Rc<GrantedEffect>, level: Option<f64>, quality: f64) -> SkillEffect {
        SkillEffect {
            granted_effect,
            level,
            quality,
            global_quality: 0.0,
            item_quality: 0.0,
            support_quality: 0.0,
            socket_quality: 0.0,
            req: None,
            gem: None,
            src: None,
            gem_cfg: None,
            gem_property_info: Vec::new(),
        }
    }
}

/// An active skill and the supports that apply to it
/// (`calcs.createActiveSkill`)
#[derive(Clone, Debug)]
pub struct ActiveSkill {
    pub active_effect: SkillEffect,
    pub support_list: Vec<SkillEffect>,
    /// The active effect, then the supports that support it
    pub effect_list: Vec<SkillEffect>,
    pub skill_types: HashSet<i64>,
    pub minion_skill_types: Option<HashSet<i64>>,
    pub skill_flags: BTreeSet<String>,
    /// The socket group, by index
    pub group: Option<usize>,
    pub slot_name: Option<String>,
    /// What building the skill's mod list sets (`buildActiveSkillModList`)
    pub mods: Option<crate::calc_skill_mods::SkillMods>,
}

/// What creating the active skills needs
pub struct SkillsInput<'a> {
    pub groups: &'a [SocketGroup],
    pub tab: &'a ItemsTab,
    pub item_data: &'a ItemData,
    pub skill_data: &'a SkillData,
    /// `env.player.itemList`
    pub item_list: &'a BTreeMap<String, f64>,
    /// `data.unarmedWeaponData[classId]`
    pub unarmed: &'a Fields,
    /// `build.mainSocketGroup`
    pub main_socket_group: Option<f64>,
    /// `build.skillsTab.imbuedSupportBySlot`
    pub imbued_support_by_slot: &'a HashMap<String, String>,
    /// `data.misc.MatchingSocketQualityBonus`
    pub matching_socket_quality_bonus: f64,
    /// The enemy's database (`env.enemyDB`)
    pub enemy_db: &'a Store,
}

/// The active skills of the player and what creating them sets
#[derive(Debug)]
pub struct SkillsSetup {
    /// `env.player.activeSkillList`
    pub active_skills: Vec<ActiveSkill>,
    /// `env.player.mainSkill`, by index in the list
    pub main_skill: usize,
    /// `env.mainSocketGroup`
    pub main_socket_group: usize,
    /// `env.player.weaponData1` and `weaponData2`
    pub weapon_data: [Fields; 2],
    /// Whether each group's slot is in use (`group.slotEnabled`)
    pub slot_enabled: Vec<bool>,
}

/// `calcLib.doesTypeExpressionMatch(checkTypes, skillTypes, minionTypes)`
pub fn type_expression_matches(
    check: &[i64],
    types: &HashSet<i64>,
    minion: Option<&HashSet<i64>>,
    data: &SkillData,
) -> bool {
    let op = |n: &str| data.skill_type(n);
    let (or, and, not) = (op("OR"), op("AND"), op("NOT"));
    let mut stack: Vec<bool> = Vec::new();
    for &t in check {
        if Some(t) == or {
            let other = stack.pop().unwrap_or(false);
            if let Some(top) = stack.last_mut() {
                *top = *top || other;
            }
        } else if Some(t) == and {
            let other = stack.pop().unwrap_or(false);
            if let Some(top) = stack.last_mut() {
                *top = *top && other;
            }
        } else if Some(t) == not {
            if let Some(top) = stack.last_mut() {
                *top = !*top;
            }
        } else {
            stack.push(types.contains(&t) || minion.is_some_and(|m| m.contains(&t)));
        }
    }
    stack.into_iter().any(|v| v)
}

/// `calcLib.canGrantedEffectSupportActiveSkill(grantedEffect, activeSkill)`
fn can_support(
    support: &GrantedEffect,
    skill: &ActiveSkill,
    from_item: &HashSet<String>,
    groups: &[SocketGroup],
    all_one_hand: bool,
    data: &SkillData,
) -> bool {
    let active = &skill.active_effect;
    let effect = &active.granted_effect;
    if support.unsupported || effect.cannot_be_supported {
        return false;
    }
    if support.support_gems_only && active.gem.is_none() {
        return false;
    }
    let is_from_item = |e: &GrantedEffect| e.from_item || from_item.contains(&e.id);
    let src_from_item = active
        .src
        .and_then(|(g, i)| groups.get(g)?.gem_list.get(i))
        .is_some_and(|g| g.from_item);
    if is_from_item(support)
        && support.support
        && (is_from_item(effect)
            || effect
                .mod_source
                .as_deref()
                .is_some_and(|s| s.starts_with("Item"))
            || src_from_item)
    {
        return false;
    }
    let minion = if support.ignore_minion_types {
        None
    } else {
        skill.minion_skill_types.as_ref()
    };
    if !support.exclude_skill_types.is_empty()
        && type_expression_matches(&support.exclude_skill_types, &skill.skill_types, None, data)
    {
        return false;
    }
    if let Some(weapons) = &support.weapon_types {
        let mut lookup: HashSet<&str> = effect
            .weapon_types
            .iter()
            .flatten()
            .map(String::as_str)
            .collect();
        if lookup.is_empty() {
            return false;
        }
        if all_one_hand {
            lookup.extend([
                "Claw",
                "Dagger",
                "One Handed Axe",
                "One Handed Mace",
                "One Handed Sword",
            ]);
        }
        if !weapons.iter().any(|w| lookup.contains(w.as_str())) {
            return false;
        }
    }
    support.require_skill_types.is_empty()
        || type_expression_matches(
            &support.require_skill_types,
            &skill.skill_types,
            minion,
            data,
        )
}

/// `calcs.createActiveSkill(activeEffect, supportList, actor, socketGroup)`
fn create_active_skill(
    active: SkillEffect,
    supports: Vec<SkillEffect>,
    group: Option<usize>,
    ctx: &SupportContext,
) -> ActiveSkill {
    let effect = active.granted_effect.clone();
    let mut skill = ActiveSkill {
        skill_types: effect.skill_types.clone(),
        minion_skill_types: effect.minion_skill_types.clone(),
        skill_flags: effect.base_flags.iter().cloned().collect(),
        active_effect: active,
        support_list: supports,
        effect_list: vec![],
        group,
        slot_name: None,
        mods: None,
    };
    let st = |n: &str| ctx.data.skill_type(n);
    if [st("Attack"), st("Damage"), st("Projectile")]
        .iter()
        .any(|t| t.is_some_and(|t| skill.skill_types.contains(&t)))
    {
        skill.skill_flags.insert("hit".into());
    }
    skill.effect_list.push(skill.active_effect.clone());
    // Pass 1: the skill types compatible supports add
    for i in 0..skill.support_list.len() {
        let support = skill.support_list[i].granted_effect.clone();
        if support.support && ctx.can_support(&support, &skill) {
            skill
                .skill_types
                .extend(support.add_skill_types.iter().copied());
        }
    }
    // Pass 2: the compatible supports
    for i in 0..skill.support_list.len() {
        let support = skill.support_list[i].clone();
        let g = &support.granted_effect;
        if g.support && ctx.can_support(g, &skill) {
            if let Some(flags) = &g.add_flags {
                skill.skill_flags.extend(flags.iter().cloned());
            }
            skill.effect_list.push(support);
        }
    }
    skill
}

/// What the support checks read
struct SupportContext<'a> {
    from_item: HashSet<String>,
    groups: &'a [SocketGroup],
    all_one_hand: bool,
    data: &'a SkillData,
}

impl SupportContext<'_> {
    fn can_support(&self, support: &GrantedEffect, skill: &ActiveSkill) -> bool {
        can_support(
            support,
            skill,
            &self.from_item,
            self.groups,
            self.all_one_hand,
            self.data,
        )
    }
}

/// `addBestSupport(supportEffect, appliedSupportList)`: adds a support,
/// unless the list has the same or a better one; a better one replaces it
fn add_best_support(support: &SkillEffect, list: &mut Vec<SkillEffect>) {
    let e = &support.granted_effect;
    for other in list.iter_mut() {
        let o = &other.granted_effect;
        if Rc::ptr_eq(e, o) || e.id == o.id {
            let (l, ol) = (
                support.level.unwrap_or(f64::NAN),
                other.level.unwrap_or(f64::NAN),
            );
            if l > ol || (l == ol && support.quality > other.quality) {
                *other = support.clone();
            }
            return;
        } else if e.plus_version_of.as_deref() == Some(o.id.as_str()) {
            *other = support.clone();
            return;
        } else if o.plus_version_of.as_deref() == Some(e.id.as_str()) {
            return;
        }
    }
    list.push(support.clone());
}

/// `calcLib.validateGemLevel(effect)` on an effect
fn validate_effect_level(effect: &mut SkillEffect) {
    let mut gem = GemInstance {
        granted_effect: Some(effect.granted_effect.clone()),
        level: effect.level,
        ..GemInstance::default()
    };
    validate_gem_level(&mut gem);
    effect.level = gem.level;
}

/// `applyGemMods(effect, modList)`: the gem properties (`GemProperty`) of
/// the gem's keywords
fn apply_gem_mods(
    effect: &mut SkillEffect,
    rows: &[(ModValue, Mod)],
    data: &SkillData,
) -> Result<(), String> {
    for (value, m) in rows {
        let ModValue::Record(v) = value else {
            continue;
        };
        let is_type = |k: &ModValue| match (k, &effect.gem) {
            (ModValue::Str(k), Some(g)) => data.gem_is_type(g, k, true),
            _ => false,
        };
        let matched = match v.get("keywordList") {
            Some(ModValue::List(l)) => l.iter().all(is_type),
            Some(ModValue::Record(r)) if r.fields.is_empty() => true,
            _ => v.get("keyword").is_some_and(is_type),
        };
        if !matched {
            continue;
        }
        let key = match v.get("key") {
            Some(ModValue::Str(k)) => k.to_string(),
            k => return Err(format!("gem property key {k:?}")),
        };
        let add = v
            .get("value")
            .and_then(ModValue::as_number)
            .ok_or("gem property value")?;
        let socketed = m
            .tags
            .iter()
            .any(|t| matches!(t.kind, TagKind::SocketedIn(_)));
        match key.as_str() {
            "quality" => {
                if socketed {
                    effect.item_quality += add;
                } else {
                    effect.global_quality += add;
                }
                effect.quality += add;
            }
            "level" => effect.level = Some(effect.level.unwrap_or(0.0) + add),
            "req" => effect.req = Some(effect.req.unwrap_or(0.0) + add),
            k => return Err(format!("gem property {k}")),
        }
        effect.gem_property_info.push((value.clone(), m.clone()));
    }
    Ok(())
}

/// A group's cfg (`groupCfg`): its slot, and the counts of the gems in it by
/// attribute once processed
#[derive(Clone, Debug, Default)]
struct GroupCfg {
    slot_name: Option<String>,
    strength_gems: Option<f64>,
    dexterity_gems: Option<f64>,
    intelligence_gems: Option<f64>,
}

impl GroupCfg {
    fn cfg(&self) -> Cfg {
        Cfg {
            slot_name: self.slot_name.as_deref().map(Sym::new),
            strength_gems: self.strength_gems,
            dexterity_gems: self.dexterity_gems,
            intelligence_gems: self.intelligence_gems,
            ..Cfg::default()
        }
    }
}

/// The player's database during the skills' setup, with its queries
pub(crate) struct PlayerDb<'a> {
    pub stores: Stores,
    pub id: StoreId,
    pub actors: EnvActors<'a>,
}

impl<'a> PlayerDb<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mod_db: Store,
        enemy_db: &Store,
        item_list: &BTreeMap<String, f64>,
        weapon_data: &[Fields; 2],
        tab: &'a ItemsTab,
        item_data: &'a ItemData,
        skill_data: &'a SkillData,
    ) -> PlayerDb<'a> {
        let mut stores = Stores::default();
        let id = stores.push(mod_db);
        let enemy = stores.push(enemy_db.clone());
        PlayerDb {
            stores,
            id,
            actors: EnvActors {
                actors: vec![
                    Actor {
                        store: id,
                        item_list: item_list.clone(),
                        weapon_data: [Some(weapon_data[0].clone()), Some(weapon_data[1].clone())],
                        player: Some(0),
                        enemy: Some(1),
                        ..Actor::default()
                    },
                    Actor {
                        store: enemy,
                        player: Some(0),
                        enemy: Some(0),
                        ..Actor::default()
                    },
                ],
                store_actor: [(id, 0), (enemy, 1)].into_iter().collect(),
                tab,
                item_data,
                skill_data,
            },
        }
    }

    /// Takes the player's database back
    pub fn finish(mut self) -> Store {
        std::mem::replace(&mut self.stores.stores[self.id], Store::db())
    }

    pub fn query(&self) -> Query<'_> {
        Query {
            stores: &self.stores,
            actors: &self.actors,
        }
    }

    pub fn tabulate(&self, cfg: Option<&Cfg>, name: &str) -> Result<Vec<(ModValue, Mod)>, String> {
        Ok(self
            .query()
            .tabulate(self.id, Some(ModType::List), cfg, &[Sym::new(name)])?
            .into_iter()
            .map(|r| (r.value, r.m.clone()))
            .collect())
    }

    pub fn list(&self, cfg: Option<&Cfg>, name: &str) -> Result<Vec<ModValue>, String> {
        self.query().list(self.id, cfg, &[Sym::new(name)])
    }

    pub fn add(&mut self, m: Mod) {
        self.stores.stores[self.id].add(m);
    }
}

/// The value of a record field as a number or a string
fn field_num(v: &ModValue, k: &str) -> Option<f64> {
    match v {
        ModValue::Record(r) => r.get(k).and_then(ModValue::as_number),
        _ => None,
    }
}

fn field_str(v: &ModValue, k: &str) -> Option<String> {
    match v {
        ModValue::Record(r) => match r.get(k) {
            Some(ModValue::Str(s)) => Some(s.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// The mod a property value adds (`value.value`), with a source
fn property_mod(v: &ModValue, source: &str) -> Option<Mod> {
    let ModValue::Record(r) = v else {
        return None;
    };
    let Some(ModValue::Mod(m)) = r.get("value") else {
        return None;
    };
    let mut m = (**m).clone();
    crate::calc_setup::set_source(&mut m, source);
    Some(m)
}

/// The weapons' data (`env.player.weaponData1` and `weaponData2`)
fn weapon_data(input: &SkillsInput, mod_db: &Store) -> [Fields; 2] {
    let item = |slot: &str| input.item_list.get(slot).and_then(|&id| input.tab.item(id));
    let data = |slot: &str, n: usize| -> Option<Fields> {
        item(slot)?.weapon_data.as_ref()?.get(&n).cloned()
    };
    let w1 = data("Weapon 1", 1).unwrap_or_else(|| input.unarmed.clone());
    let w2 = if w1.get("countsAsDualWielding").is_some_and(ModValue::truthy) {
        data("Weapon 1", 2).unwrap_or_default()
    } else if item("Weapon 2").is_none() {
        let hollow_palm =
            item("Weapon 1").is_none() && item("Gloves").is_none() && match &mod_db.mods {
                Mods::Db(by_name) => by_name
                    .get(&Sym::new("Keystone"))
                    .into_iter()
                    .flatten()
                    .any(
                        |m| matches!(&m.value, ModValue::Str(s) if &**s == "Hollow Palm Technique"),
                    ),
                Mods::List(_) => false,
            };
        if hollow_palm {
            input.unarmed.clone()
        } else {
            Fields::new()
        }
    } else {
        data("Weapon 2", 2).unwrap_or_default()
    };
    [w1, w2]
}

/// The player's active skills with their supports: `initEnv` from the
/// weapon data to the main skill. The socket groups are the Skills tab's
/// with the granted ones; `mod_db` is the player's database, which socket
/// and group properties add to.
pub fn create_active_skills(
    input: &SkillsInput,
    mod_db: &mut Store,
) -> Result<SkillsSetup, String> {
    let data = input.skill_data;
    let weapon_data = weapon_data(input, mod_db);
    let all_one_hand = weapon_data
        .iter()
        .any(|d| d.get("countsAsAll1H").is_some_and(ModValue::truthy));
    let mut db = PlayerDb::new(
        std::mem::replace(mod_db, Store::db()),
        input.enemy_db,
        input.item_list,
        &weapon_data,
        input.tab,
        input.item_data,
        data,
    );
    let result = active_skills(input, &mut db, weapon_data, all_one_hand);
    *mod_db = db.finish();
    result
}

fn active_skills(
    input: &SkillsInput,
    db: &mut PlayerDb,
    weapon_data: [Fields; 2],
    all_one_hand: bool,
) -> Result<SkillsSetup, String> {
    let groups = input.groups;
    let data = input.skill_data;
    let main_group = (groups.len() as f64)
        .max(1.0)
        .min(input.main_socket_group.unwrap_or(1.0));
    let main_index = main_group as usize;
    // Supports cross-linked from one slot to others
    let mut cross_linked: Vec<(String, Vec<String>)> = Vec::new();
    for (v, m) in db.tabulate(None, "LinkedSupport")? {
        let source = m.source_slot.as_deref().unwrap_or("").to_string();
        let target = field_str(&v, "targetSlotName").unwrap_or_default();
        match cross_linked.iter_mut().find(|(s, _)| *s == source) {
            Some((_, l)) => l.push(target),
            None => cross_linked.push((source, vec![target])),
        }
    }
    let second_set = input.tab.active_item_set().use_second_weapon_set;
    let slot_enabled: Vec<bool> = groups
        .iter()
        .map(|g| {
            let slot = g
                .slot
                .as_ref()
                .and_then(|s| input.tab.ordered_slots.iter().find(|x| &x.slot_name == s));
            slot.and_then(|s| s.weapon_set)
                .is_none_or(|w| w == if second_set { 2 } else { 1 })
        })
        .collect();
    let socket_bonus = |g: &GemInstance| {
        if g.matches_socket {
            input.matching_socket_quality_bonus
        } else {
            0.0
        }
    };
    let used = |i: usize| i + 1 == main_index || (groups[i].enabled && slot_enabled[i]);
    let slot_of = |g: &SocketGroup| g.slot.as_ref().map(|s| s.replace(" Swap", ""));
    let mut cfgs: Vec<GroupCfg> = groups
        .iter()
        .map(|g| GroupCfg {
            slot_name: slot_of(g),
            ..GroupCfg::default()
        })
        .collect();
    let mut processed: HashSet<(usize, usize)> = HashSet::new();
    let mut from_item: HashSet<String> = HashSet::new();
    // Each group's supports; groups in a slot also by slot, in order
    let mut supports: Vec<Vec<SkillEffect>> = vec![vec![]; groups.len()];
    let mut slot_groups: Vec<(String, Vec<usize>)> = Vec::new();
    let item_name = |slot: &Option<String>| -> Option<String> {
        let id = input.item_list.get(slot.as_deref()?)?;
        Some(input.tab.item(*id)?.name.clone())
    };
    let socket_color = |slot: &Option<String>, n: usize| -> Option<char> {
        let id = input.item_list.get(slot.as_deref()?)?;
        input.tab.item(*id)?.sockets.get(n).map(|s| s.color)
    };
    // The gem properties and socket properties of a gem
    let mut gem_effect = |db: &mut PlayerDb,
                          cfgs: &mut Vec<GroupCfg>,
                          effect: &mut SkillEffect,
                          gi: usize,
                          i: usize|
     -> Result<(), String> {
        let gem = effect.gem.clone().unwrap();
        let group_cfg = cfgs[gi].clone();
        let mut gem_cfg = group_cfg.cfg();
        let color = socket_color(&group_cfg.slot_name, i);
        gem_cfg.socket_color = color.map(|c| Rc::from(c.to_string().as_str()));
        gem_cfg.socket_num = Some(i as f64 + 1.0);
        let rows = if color.is_some() {
            db.tabulate(Some(&gem_cfg), "GemProperty")?
        } else {
            db.tabulate(Some(&group_cfg.cfg()), "GemProperty")?
        };
        effect.gem_cfg = Some(gem_cfg);
        apply_gem_mods(effect, &rows, data)?;
        if processed.insert((gi, i)) {
            let mut socket_cfg = group_cfg.cfg();
            socket_cfg.skill_gem = Some(gem.clone());
            socket_cfg.socket_num = Some(i as f64 + 1.0);
            let source = item_name(&group_cfg.slot_name)
                .or(group_cfg.slot_name.clone())
                .unwrap_or_default();
            for v in db.list(Some(&socket_cfg), "SocketProperty")? {
                if let Some(m) = property_mod(&v, &source) {
                    db.add(m);
                }
            }
            let c = &mut cfgs[gi];
            let count = |n: Option<f64>, tag: &str| {
                Some(n.unwrap_or(0.0) + if gem.tags.contains(tag) { 1.0 } else { 0.0 })
            };
            c.intelligence_gems = count(c.intelligence_gems, "intelligence");
            c.dexterity_gems = count(c.dexterity_gems, "dexterity");
            c.strength_gems = count(c.strength_gems, "strength");
        }
        Ok(())
    };
    // The supports of the groups in use
    for (gi, group) in groups.iter().enumerate() {
        if !used(gi) {
            continue;
        }
        if let Some(slot) = &cfgs[gi].slot_name {
            match slot_groups.iter_mut().find(|(s, _)| s == slot) {
                Some((_, l)) => l.push(gi),
                None => slot_groups.push((slot.clone(), vec![gi])),
            }
        }
        let mut extra =
            |effect: Option<Rc<GrantedEffect>>, value: Option<&ModValue>, level: Option<f64>| {
                let mut effect = effect.or_else(|| {
                    value
                        .and_then(|v| field_str(v, "skillId"))
                        .and_then(|id| data.skills.get(&id).cloned())
                });
                if let (Some(v), Some(e)) = (value, &effect) {
                    from_item.insert(e.id.clone());
                    if !e.support {
                        let id = format!("Support{}", field_str(v, "skillId").unwrap_or_default());
                        effect = data.skills.get(&id).cloned();
                        if let Some(e) = &effect {
                            from_item.insert(e.id.clone());
                        }
                    }
                }
                let Some(e) = effect else {
                    return;
                };
                let name = crate::pat::lower(&e.name);
                let gem = data
                    .gem_for_base_name(&name)
                    .or_else(|| data.gem_for_base_name(&format!("{name} support")))
                    .cloned();
                let mut s = SkillEffect::new(
                    e,
                    level.or_else(|| value.and_then(|v| field_num(v, "level"))),
                    0.0,
                );
                s.gem = gem;
                supports[gi].push(s);
            };
        if group.source.is_none() {
            for v in db.list(Some(&cfgs[gi].cfg()), "ExtraSupport")? {
                extra(None, Some(&v), None);
            }
        }
        if let Some(slot) = &cfgs[gi].slot_name
            && let Some(imbued) = input.imbued_support_by_slot.get(slot)
            && group.imbued_support.is_some()
            && let Some(effect) = data.skills.get(imbued)
        {
            extra(Some(effect.clone()), None, Some(1.0));
            if let Some(g) = data.gem_for_skill(imbued)
                && let Some(second) = &g.secondary_granted_effect
                && second.support
            {
                extra(Some(second.clone()), None, Some(1.0));
            }
        }
        for (i, gem) in group.gem_list.iter().enumerate() {
            if !gem.enabled {
                continue;
            }
            let effects: Vec<Rc<GrantedEffect>> = match &gem.gem {
                Some(g) => std::iter::once(g.granted_effect.clone())
                    .chain(g.secondary_granted_effect.clone())
                    .collect(),
                None => gem.granted_effect.iter().cloned().collect(),
            };
            for e in effects {
                if !e.support {
                    continue;
                }
                let mut s =
                    SkillEffect::new(e, gem.level, gem.quality.unwrap_or(0.0) + socket_bonus(gem));
                s.socket_quality = socket_bonus(gem);
                s.gem = gem.gem.clone();
                s.src = Some((gi, i));
                if s.gem.is_some() {
                    gem_effect(db, &mut cfgs, &mut s, gi, i)?;
                }
                validate_effect_level(&mut s);
                add_best_support(&s, &mut supports[gi]);
            }
        }
    }
    let ctx = SupportContext {
        from_item,
        groups,
        all_one_hand,
        data,
    };
    // The active skills of the groups in use, with their supports
    let mut active_skills = Vec::new();
    let mut group_skills: Vec<Vec<usize>> = vec![vec![]; groups.len()];
    for (gi, group) in groups.iter().enumerate() {
        if !used(gi) {
            continue;
        }
        let slot = cfgs[gi].slot_name.clone();
        for (i, gem) in group.gem_list.iter().enumerate() {
            if !gem.enabled || (gem.gem.is_none() && gem.granted_effect.is_none()) {
                continue;
            }
            let effects: Vec<Option<Rc<GrantedEffect>>> = match &gem.gem {
                Some(g) => vec![
                    Some(g.granted_effect.clone()),
                    g.secondary_granted_effect.clone(),
                ],
                None => vec![gem.granted_effect.clone()],
            };
            // `ipairs` stops at the first nil
            for (index, e) in effects.into_iter().map_while(|e| e).enumerate() {
                let global = if index == 0 {
                    gem.enable_global1
                } else {
                    gem.enable_global2
                };
                if e.support || e.unsupported || (e.has_global_effect && !global) {
                    continue;
                }
                let mut active =
                    SkillEffect::new(e, gem.level, gem.quality.unwrap_or(0.0) + socket_bonus(gem));
                active.socket_quality = socket_bonus(gem);
                active.gem = gem.gem.clone();
                active.src = Some((gi, i));
                if active.gem.is_some() {
                    gem_effect(db, &mut cfgs, &mut active, gi, i)?;
                }
                let mut applied = Vec::new();
                if !group.no_supports {
                    applied = supports[gi].clone();
                    if group.source.is_some()
                        && let Some(slot) = &slot
                        && let Some((_, list)) = slot_groups.iter().find(|(s, _)| s == slot)
                    {
                        for &other in list {
                            for s in &supports[other] {
                                add_best_support(s, &mut applied);
                            }
                        }
                    }
                    for (from, targets) in &cross_linked {
                        for target in targets {
                            if Some(target) == slot.as_ref()
                                && let Some((_, list)) = slot_groups.iter().find(|(s, _)| s == from)
                            {
                                for &other in list {
                                    for s in &supports[other] {
                                        add_best_support(s, &mut applied);
                                    }
                                }
                            }
                        }
                    }
                }
                let has_gem = active.gem.is_some();
                let mut skill = create_active_skill(active, applied, Some(gi), &ctx);
                if has_gem {
                    skill.slot_name = slot.clone();
                }
                group_skills[gi].push(active_skills.len());
                active_skills.push(skill);
            }
        }
    }
    // Group properties and the main skill
    let mut main_skill = None;
    for (gi, group) in groups.iter().enumerate() {
        if used(gi) {
            let source = cfgs[gi].slot_name.clone().unwrap_or_default();
            for v in db.list(Some(&cfgs[gi].cfg()), "GroupProperty")? {
                if let Some(m) = property_mod(&v, &source) {
                    db.add(m);
                }
            }
            if gi + 1 == main_index && !group_skills[gi].is_empty() {
                let n = group_skills[gi].len() as f64;
                let i = n.min(group.main_active_skill) as usize;
                main_skill = group_skills[gi].get(i.saturating_sub(1)).copied();
            }
        }
        if group.enabled
            && slot_enabled[gi]
            && group.gem_list.iter().any(|g| {
                g.enabled
                    && g.effect()
                        .is_some_and(|e| !e.support && e.name == "Energy Blade")
            })
        {
            return Err("Energy Blade is not ported".into());
        }
    }
    let main_skill = match main_skill {
        Some(i) => i,
        None => {
            let melee = data.skills.get("Melee").ok_or("no Melee skill")?.clone();
            let skill =
                create_active_skill(SkillEffect::new(melee, Some(1.0), 0.0), vec![], None, &ctx);
            active_skills.push(skill);
            active_skills.len() - 1
        }
    };
    Ok(SkillsSetup {
        active_skills,
        main_skill,
        main_socket_group: main_index,
        weapon_data,
        slot_enabled,
    })
}
