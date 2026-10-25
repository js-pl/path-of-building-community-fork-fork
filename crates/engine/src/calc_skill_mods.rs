//! The active skills' mod lists: `calcs.buildActiveSkillModList` and
//! `calcs.mergeSkillInstanceMods` of `Modules/CalcActiveSkill.lua`, with
//! `calcLib.buildSkillInstanceStats` of `Modules/CalcTools.lua`. For each
//! active skill: its flags, weapons, cfg, the mods of its gem and supports,
//! its skill data, its minion and the buffs it grants.
//!
//! Not ported yet: the skills of minions (`calcs.createMinionSkills`).
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::calc_setup::{BuffMode, SetupData};
use crate::calc_skills::{ActiveSkill, PlayerDb, SkillEffect};
use crate::config::Inputs;
use crate::item::Fields;
use crate::items_tab::ItemsTab;
use crate::mods::tag::TagKind;
use crate::mods::{Cfg, Mod, ModType, ModValue, Record, Store, StoreId, Sym, Tag};
use crate::skill_data::{GrantedEffect, SkillData, StatMapEntry};
use crate::skills::{GemInstance, SocketGroup, validate_gem_level};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub type Result<T> = std::result::Result<T, String>;

/// A skill's minion (`activeSkill.minion`) as the mod list sets it up
#[derive(Clone, Debug, PartialEq)]
pub struct Minion {
    /// The minion's id (`minion.type`)
    pub typ: String,
    pub level: f64,
    pub hostile: bool,
    /// Which life table its life comes from (`monsterLifeTable`, ...)
    pub life_table: &'static str,
    /// The keys of `minion.uses`
    pub uses: Option<Vec<String>>,
    /// The item set its items come from, by id
    pub item_set: Option<f64>,
    pub weapon_data: [Fields; 2],
}

/// A buff a skill grants (`activeSkill.buffList`): mods with a
/// `GlobalEffect` tag of one type and name
#[derive(Clone, Debug, PartialEq)]
pub struct Buff {
    pub typ: String,
    pub name: String,
    /// The tag's fields: `allowTotemBuff`, `effectCond`, ...
    pub tag: Record,
    /// Configuration from the active skill, for its own buffs
    pub active_skill_buff: bool,
    pub apply_not_player: bool,
    pub apply_minions: bool,
    pub apply_allies: bool,
    pub allow_totem_buff: bool,
    pub mod_list: Vec<Mod>,
}

/// What building an active skill's mod list sets on it
#[derive(Clone, Debug, Default)]
pub struct SkillMods {
    pub skill_flags: BTreeSet<String>,
    pub skill_part: Option<f64>,
    pub skill_part_name: Option<String>,
    pub disable_reason: Option<String>,
    pub weapon_flags: [Option<u32>; 2],
    pub skill_cfg: Cfg,
    pub weapon_cfgs: [Option<Cfg>; 2],
    /// The mods of `skillModList`, whose parent is the player's database
    pub mods: Vec<Mod>,
    /// `skillData`
    pub skill_data: BTreeMap<String, ModValue>,
    /// The support that triggers the skill, by place in the effect list
    pub triggered_by: Option<usize>,
    pub skill_totem_id: Option<f64>,
    pub extra_skill_mod_list: Vec<Mod>,
    pub active_mine_count: Option<f64>,
    pub active_stage_count: Option<f64>,
    pub minion_list: Vec<String>,
    pub minion: Option<Minion>,
    pub buff_list: Vec<Buff>,
}

/// What building the mod lists needs
pub struct ModsInput<'a> {
    pub setup: &'a SetupData,
    pub skill_data: &'a SkillData,
    pub tab: &'a ItemsTab,
    pub item_data: &'a crate::item_data::ItemData,
    pub item_list: &'a BTreeMap<String, f64>,
    pub weapon_data: &'a [Fields; 2],
    pub mode: BuffMode,
    /// `env.configInput` and `env.configPlaceholder`
    pub config: &'a Inputs,
    pub placeholders: &'a Inputs,
    /// `build.spectreList`
    pub spectre_list: &'a [String],
    pub character_level: f64,
    pub enemy_level: f64,
    /// `env.theIronMass`
    pub iron_mass: bool,
    /// The enemy's database (`env.enemyDB`)
    pub enemy_db: &'a Store,
}

/// `round(val)` of `Modules/Common.lua`
fn round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// `calcLib.buildSkillInstanceStats(skillInstance, grantedEffect)`: the
/// stats of an effect's level and quality, in the order upstream's table
/// sets them
pub fn build_skill_instance_stats(
    level: Option<f64>,
    quality: f64,
    actor_level: Option<f64>,
    effect: &GrantedEffect,
    setup: &SetupData,
) -> Vec<(String, f64)> {
    let mut stats: Vec<(String, f64)> = Vec::new();
    let add = |stats: &mut Vec<(String, f64)>, k: &str, v: f64| match stats
        .iter_mut()
        .find(|(s, _)| s == k)
    {
        Some((_, x)) => *x += v,
        None => stats.push((k.to_string(), v)),
    };
    if quality > 0.0 {
        for (stat, per) in &effect.quality_stats {
            add(&mut stats, stat, (per * quality).trunc());
        }
    }
    let lvl = level.and_then(|l| effect.level(l));
    let actor_level = actor_level
        .or_else(|| lvl.map(|l| l.level_requirement))
        .unwrap_or(1.0);
    let mut effectiveness = None;
    for (index, stat) in effect.stats.iter().enumerate() {
        let value = lvl.and_then(|l| l.values.get(index).copied());
        let mut stat_value = value.unwrap_or(1.0);
        if let Some(l) = lvl
            && !l.stat_interpolation.is_empty()
        {
            match l.stat_interpolation.get(index).copied() {
                Some(3.0) => {
                    let e = *effectiveness.get_or_insert_with(|| {
                        (setup.game_constant("SkillDamageBaseEffectiveness")
                            + setup.game_constant("SkillDamageIncrementalEffectiveness")
                                * (actor_level - 1.0))
                            * effect.base_effectiveness.unwrap_or(1.0)
                            * (1.0 + effect.incremental_effectiveness.unwrap_or(0.0))
                                .powf(actor_level - 1.0)
                    });
                    stat_value = round(e * value.unwrap_or(f64::NAN));
                }
                Some(2.0) => {
                    let ordered: Vec<i64> = effect.levels.keys().copied().collect();
                    let current = ordered.iter().position(|&n| Some(n as f64) == level);
                    let v = |i: usize| {
                        effect.levels[&ordered[i]]
                            .values
                            .get(index)
                            .copied()
                            .unwrap_or(f64::NAN)
                    };
                    if ordered.len() > 1 {
                        let next = current.map_or(1, |c| (c + 1).min(ordered.len() - 1));
                        let next_req = effect.levels[&ordered[next]].level_requirement;
                        let prev_req = effect.levels[&ordered[next - 1]].level_requirement;
                        let (next_stat, prev_stat) = (v(next), v(next - 1));
                        stat_value = round(
                            prev_stat
                                + (next_stat - prev_stat) * (actor_level - prev_req)
                                    / (next_req - prev_req),
                        );
                    } else if let Some(c) = current {
                        stat_value = round(v(c));
                    }
                }
                _ => {}
            }
        }
        add(&mut stats, stat, stat_value);
    }
    for (stat, v) in &effect.constant_stats {
        add(&mut stats, stat, v.unwrap_or(0.0));
    }
    stats
}

/// `mergeLevelMod(modList, mod, value)`: the mod with the stat's value
fn merge_level_mod(list: &mut Vec<Mod>, m: &Mod, value: f64) {
    let mut m = m.clone();
    match &mut m.value {
        ModValue::Record(r) => {
            if let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod")) {
                inner.value = ModValue::Number(value);
            } else if let Some(v) = r.get_mut(Sym::new("value")) {
                *v = ModValue::Number(value);
            } else {
                r.fields.push((Sym::new("value"), ModValue::Number(value)));
            }
        }
        v => *v = ModValue::Number(value),
    }
    list.push(m);
}

/// `calcs.mergeSkillInstanceMods(env, modList, skillEffect, extraStats)`:
/// the effect's stat mods and base mods
fn merge_skill_instance_mods(
    list: &mut Vec<Mod>,
    effect: &mut SkillEffect,
    extra: &[(String, f64)],
    input: &ModsInput,
) -> Result<()> {
    validate_effect(effect);
    let ge = effect.granted_effect.clone();
    let mut stats =
        build_skill_instance_stats(effect.level, effect.quality, None, &ge, input.setup);
    for (k, v) in extra {
        match stats.iter_mut().find(|(s, _)| s == k) {
            Some((_, x)) => *x += v,
            None => stats.push((k.clone(), *v)),
        }
    }
    for (stat, value) in &stats {
        let Some(map) = input.skill_data.stat_map(&ge, stat) else {
            continue;
        };
        if let Some(e) = &map.error {
            return Err(format!("{} stat {stat}: {e}", ge.id));
        }
        for e in &map.entries {
            match e {
                StatMapEntry::Mod(m) => merge_level_mod(list, m, map.scale.apply(*value)),
                StatMapEntry::Group(mods, scale) => {
                    for m in mods {
                        merge_level_mod(list, m, scale.apply(*value));
                    }
                }
            }
        }
    }
    list.extend(ge.base_mods.iter().cloned());
    Ok(())
}

/// `calcLib.validateGemLevel(effect)`
fn validate_effect(effect: &mut SkillEffect) {
    let mut gem = GemInstance {
        granted_effect: Some(effect.granted_effect.clone()),
        level: effect.level,
        ..GemInstance::default()
    };
    validate_gem_level(&mut gem);
    effect.level = gem.level;
}

/// `getWeaponFlags(env, weaponData, weaponTypes)`: the flags of a weapon
/// usable with the skill, and the weapon type's info
fn weapon_flags<'d>(
    weapon: &Fields,
    weapon_types: &[&Vec<String>],
    input: &'d ModsInput,
) -> (Option<u32>, Option<&'d crate::calc_setup::WeaponTypeInfo>) {
    let typ = match weapon.get("type") {
        Some(ModValue::Str(t)) => t.to_string(),
        _ => return (None, None),
    };
    let Some(info) = input.setup.weapon_type_info.get(&typ) else {
        return (None, None);
    };
    let all_1h = weapon.get("countsAsAll1H").is_some_and(ModValue::truthy);
    for types in weapon_types {
        let has = |t: &str| types.iter().any(|x| x == t);
        if !has(&typ)
            && (!all_1h
                || ![
                    "Claw",
                    "Dagger",
                    "One Handed Axe",
                    "One Handed Mace",
                    "One Handed Sword",
                ]
                .iter()
                .any(|t| has(t)))
        {
            return (None, Some(info));
        }
    }
    let f = |n: &str| input.setup.mod_flag(n);
    let mut flags = f(&info.flag);
    if all_1h {
        flags = f("Axe") | f("Claw") | f("Dagger") | f("Mace") | f("Sword");
    }
    if typ != "None" {
        flags |= f("Weapon");
        flags |= if info.one_hand {
            f("Weapon1H")
        } else {
            f("Weapon2H")
        };
        flags |= if info.melee {
            f("WeaponMelee")
        } else {
            f("WeaponRanged")
        };
    }
    (Some(flags), Some(info))
}

/// A number or a string of a config input or placeholder
fn config_num(input: &ModsInput, k: &str) -> Option<f64> {
    input
        .config
        .get(k)
        .or_else(|| input.placeholders.get(k))
        .map(|v| v.num())
}

fn record_str(r: &Record, k: &str) -> Option<String> {
    match r.get(k) {
        Some(ModValue::Str(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Builds every active skill's mod list (`calcs.buildActiveSkillModList`),
/// in order. The skills' gems keep their selections (`skillPart`,
/// `skillMinion`, ...) as upstream writes them back.
pub fn build_skill_mod_lists(
    skills: &mut [ActiveSkill],
    groups: &mut [SocketGroup],
    mod_db: &mut Store,
    enemy_multipliers: &mut std::collections::HashMap<Sym, f64>,
    input: &ModsInput,
) -> Result<()> {
    let mut db = PlayerDb::new(
        std::mem::replace(mod_db, Store::db()),
        input.enemy_db,
        input.item_list,
        input.weapon_data,
        input.tab,
        input.item_data,
        input.skill_data,
    );
    let mut result = Ok(());
    for skill in skills.iter_mut() {
        if let Err(e) = build_skill_mod_list(skill, groups, &mut db, enemy_multipliers, input) {
            result = Err(e);
            break;
        }
    }
    *mod_db = db.finish();
    result
}

fn build_skill_mod_list(
    skill: &mut ActiveSkill,
    groups: &mut [SocketGroup],
    db: &mut PlayerDb,
    enemy_multipliers: &mut std::collections::HashMap<Sym, f64>,
    input: &ModsInput,
) -> Result<()> {
    let data = input.skill_data;
    let st = |n: &str| data.skill_type(n).unwrap_or(-1);
    let mf = |n: &str| input.setup.mod_flag(n);
    let kf = |n: &str| input.setup.keyword_flag(n);
    let mut out = SkillMods::default();
    let mut flags: BTreeSet<String> = skill.skill_flags.clone();
    let has_type = |t: &str| skill.skill_types.contains(&st(t));
    let ge = skill.active_effect.granted_effect.clone();
    let src = skill.active_effect.src;
    let secondary = skill
        .active_effect
        .gem
        .as_ref()
        .is_some_and(|g| g.secondary_granted_effect.is_some());
    if input.mode.buffs {
        flags.insert("buffs".into());
    }
    if input.mode.combat {
        flags.insert("combat".into());
    }
    if input.mode.effective {
        flags.insert("effective".into());
    }
    // Multipart skills
    match &ge.parts {
        Some(parts) if parts.len() > 1 => {
            let gem = src_gem(groups, src).ok_or("a multipart skill without a gem")?;
            let part_index = (parts.len() as f64).min(gem.skill_part.unwrap_or(1.0));
            gem.skill_part = Some(part_index);
            out.skill_part = Some(part_index);
            let part = parts.get(part_index as usize - 1).ok_or("no skill part")?;
            for (k, v) in part {
                match v {
                    ModValue::Bool(true) => {
                        flags.insert(k.clone());
                    }
                    ModValue::Bool(false) => {
                        flags.remove(k);
                    }
                    _ => {}
                }
            }
            out.skill_part_name = match part.get("name") {
                Some(ModValue::Str(s)) => Some(s.to_string()),
                _ => None,
            };
            flags.insert("multiPart".into());
        }
        _ => {
            if !secondary && let Some(g) = src_gem(groups, src) {
                g.skill_part = None;
                g.skill_part_calcs = None;
            }
        }
    }
    let item_type = |slot: &str| {
        input
            .item_list
            .get(slot)
            .and_then(|&id| input.tab.item(id))
            .and_then(|i| i.typ.clone())
    };
    if (has_type("RequiresShield") || flags.contains("shieldAttack"))
        && item_type("Weapon 2").as_deref() != Some("Shield")
    {
        flags.insert("disable".into());
        out.disable_reason = Some("This skill requires a Shield".into());
    }
    if flags.contains("shieldAttack") {
        flags.insert("weapon2Attack".into());
        out.weapon_flags[1] = Some(0);
    } else {
        let group = skill.group.and_then(|g| groups.get(g));
        if flags.contains("forceSourceWeapon")
            && let Some(group) = group.filter(|g| g.source_item.is_some())
        {
            let slot = group.slot.clone().unwrap_or_default();
            if slot.starts_with("Weapon 1") {
                flags.insert("forceMainHand".into());
            } else {
                flags.remove("forceMainHand");
            }
            if slot.starts_with("Weapon 2") {
                flags.insert("forceOffHand".into());
            } else {
                flags.remove("forceOffHand");
            }
        }
        // `ipairs` stops at the active skill's types when it has none
        let mut weapon_types: Vec<&Vec<String>> = Vec::new();
        if let Some(t) = &ge.weapon_types {
            weapon_types.push(t);
            for e in &skill.effect_list {
                if e.granted_effect.support
                    && let Some(t) = &e.granted_effect.weapon_types
                {
                    weapon_types.push(t);
                }
            }
        }
        let attack = flags.contains("attack") || flags.contains("dotFromAttack");
        let (mut w1, mut w1_info) = (None, None);
        if !flags.contains("forceOffHand") {
            (w1, w1_info) = weapon_flags(&input.weapon_data[0], &weapon_types, input);
        }
        if let Some(w1f) = w1 {
            if attack {
                out.weapon_flags[0] = Some(w1f);
                flags.insert("weapon1Attack".into());
                let melee = w1_info.is_some_and(|i| i.melee);
                if melee && flags.contains("melee") {
                    flags.remove("projectile");
                } else if !melee && flags.contains("projectile") {
                    flags.remove("melee");
                }
            }
        } else if has_type("DualWieldOnly") || flags.contains("forceMainHand") || w1_info.is_some()
        {
            flags.insert("disable".into());
            out.disable_reason = Some("Main Hand weapon is not usable with this skill".into());
        }
        if !flags.contains("forceMainHand") {
            let (w2, w2_info) = weapon_flags(&input.weapon_data[1], &weapon_types, input);
            let typ = |w: &Fields| w.get("type").cloned();
            let all_1h = |w: &Fields| w.get("countsAsAll1H").is_some_and(ModValue::truthy);
            if let Some(w2f) = w2 {
                if has_type("DualWieldRequiresDifferentTypes")
                    && typ(&input.weapon_data[0]) == typ(&input.weapon_data[1])
                    && !(all_1h(&input.weapon_data[1]) || all_1h(&input.weapon_data[0]))
                {
                    flags.insert("disable".into());
                    out.disable_reason
                        .get_or_insert_with(|| "Weapon Types Need to be Different".into());
                } else if attack {
                    out.weapon_flags[1] = Some(w2f);
                    flags.insert("weapon2Attack".into());
                }
            } else if has_type("DualWieldOnly") || w2_info.is_some() {
                flags.insert("disable".into());
                out.disable_reason
                    .get_or_insert_with(|| "Off Hand weapon is not usable with this skill".into());
            } else if flags.contains("disable") {
                out.disable_reason = Some("No usable weapon equipped".into());
            }
        }
        if flags.contains("attack")
            && flags.contains("weapon1Attack")
            && flags.contains("weapon2Attack")
        {
            flags.insert("bothWeaponAttack".into());
        }
    }
    // Flags the stat map sets
    let active = &skill.active_effect;
    for (stat, value) in
        build_skill_instance_stats(active.level, active.quality, None, &ge, input.setup)
    {
        if value != 0.0
            && let Some(map) = data.stat_map(&ge, &stat)
            && let Some(f) = &map.skill_flag
        {
            flags.insert(f.clone());
        }
    }
    // The skill's mod and keyword flags
    let has = |f: &BTreeSet<String>, k: &str| f.contains(k);
    let mut mod_flags = 0;
    if has(&flags, "hit") {
        mod_flags |= mf("Hit");
    }
    if has(&flags, "attack") {
        mod_flags |= mf("Attack");
    } else {
        mod_flags |= mf("Cast");
        if has(&flags, "spell") {
            mod_flags |= mf("Spell");
        }
    }
    if has(&flags, "melee") {
        mod_flags |= mf("Melee");
    } else if has(&flags, "projectile") {
        mod_flags |= mf("Projectile");
        flags.insert("chaining".into());
    }
    if has(&flags, "area") {
        mod_flags |= mf("Area");
    }
    let mut kw = 0;
    if has(&flags, "hit") {
        kw |= kf("Hit");
    }
    for (t, k) in [
        ("Aura", "Aura"),
        ("AppliesCurse", "Curse"),
        ("Warcry", "Warcry"),
        ("Movement", "Movement"),
        ("Vaal", "Vaal"),
        ("Lightning", "Lightning"),
        ("Cold", "Cold"),
        ("Fire", "Fire"),
        ("Chaos", "Chaos"),
        ("Physical", "Physical"),
    ] {
        if has_type(t) {
            kw |= kf(k);
        }
    }
    if has(&flags, "weapon1Attack") && out.weapon_flags[0].unwrap_or(0) & mf("Bow") != 0 {
        kw |= kf("Bow");
    }
    if has(&flags, "brand") {
        kw |= kf("Brand");
    }
    if has(&flags, "arrow") {
        kw |= kf("Arrow");
    }
    if has(&flags, "totem") {
        kw |= kf("Totem");
    } else if has(&flags, "trap") {
        kw |= kf("Trap");
    } else if has(&flags, "mine") {
        kw |= kf("Mine");
    } else if !has_type("Triggered") {
        flags.insert("selfCast".into());
    }
    if has_type("Attack") {
        kw |= kf("Attack");
    }
    if has_type("Spell") && !has(&flags, "cast") {
        kw |= kf("Spell");
    }
    if has(&flags, "totem") {
        out.skill_totem_id = Some(ge.skill_totem_id.unwrap_or(match ge.color {
            Some(2.0) => 2.0,
            Some(3.0) => 3.0,
            _ => 1.0,
        }));
    }
    let range = if has(&flags, "melee") {
        config_num(input, "meleeDistance")
    } else {
        config_num(input, "projectileDistance")
    };
    let gem_cfg = skill.active_effect.gem_cfg.clone();
    let cfg = Cfg {
        flags: Some(mod_flags | out.weapon_flags[0].or(out.weapon_flags[1]).unwrap_or(0)),
        keyword_flags: Some(kw),
        skill_name: Some(crate::pat::gsub(&ge.name, "^Vaal ", "").into()),
        summon_skill_name: None,
        skill_id: Some(Some(ge.id.as_str().into())),
        base_flags: Some(ge.base_flags.iter().map(|f| Sym::new(f)).collect()),
        skill_part: out.skill_part,
        skill_types: Some(skill.skill_types.clone()),
        skill_cond: Some(HashSet::new()),
        skill_dist: if input.mode.effective { range } else { None },
        slot_name: skill
            .slot_name
            .clone()
            .or_else(|| {
                gem_cfg
                    .as_ref()
                    .and_then(|c| c.slot_name.map(|s| s.as_str().to_string()))
            })
            .map(|s| Sym::new(&s)),
        socket_color: gem_cfg.as_ref().and_then(|c| c.socket_color.clone()),
        socket_num: gem_cfg.as_ref().and_then(|c| c.socket_num),
        skill_gem: skill.active_effect.gem.clone(),
        ..Cfg::default()
    };
    for (n, (flag, cond)) in [
        ("weapon1Attack", "MainHandAttack"),
        ("weapon2Attack", "OffHandAttack"),
    ]
    .iter()
    .enumerate()
    {
        if has(&flags, flag) {
            let mut c = cfg.clone();
            c.skill_cond = Some([Sym::new(cond)].into_iter().collect());
            c.flags = Some(mod_flags | out.weapon_flags[n].unwrap_or(0));
            out.weapon_cfgs[n] = Some(c);
        }
    }
    // The skill's mod list, under the player's database
    let mut store = Store::list();
    store.parent = Some(db.id);
    let list_id: StoreId = db.stores.push(store);
    db.actors.store_actor.insert(list_id, 0);
    let flag = |db: &PlayerDb, cfg: Option<&Cfg>, name: &str| -> Result<bool> {
        db.query().flag(list_id, cfg, &[Sym::new(name)])
    };
    let curse_application = skill
        .group
        .and_then(|g| groups.get(g))
        .is_some_and(|g| g.source_item.is_some())
        && has(&flags, "curse")
        && src
            .and_then(|(g, i)| groups.get(g)?.gem_list.get(i))
            .is_some_and(|g| g.no_supports && g.triggered);
    if flag(db, Some(&cfg), "DisableSkill")?
        && !(flag(db, Some(&cfg), "EnableSkill")?
            || (curse_application && flag(db, None, "ForceEnableCurseApplication")?))
    {
        flags.insert("disable".into());
        out.disable_reason = Some("Skills of this type are disabled".into());
    }
    out.skill_cfg = cfg.clone();
    if has(&flags, "disable") {
        flags = ["disable".to_string()].into_iter().collect();
        validate_effect(&mut skill.active_effect);
        out.skill_flags = flags;
        skill.mods = Some(out);
        return Ok(());
    }
    let mut mods: Vec<Mod> = Vec::new();
    let new_mod =
        |name: &str, typ: ModType, v: f64, source: Option<&str>, flags: u32, tags: Vec<Tag>| {
            let mut m = Mod::new(name, typ, v, source);
            m.flags = flags;
            m.tags = tags;
            m
        };
    // Support gem mods
    let mut stored_uses: Option<f64> = None;
    for i in 1..skill.effect_list.len() {
        if !skill.effect_list[i].granted_effect.support {
            continue;
        }
        let mut effect = skill.effect_list[i].clone();
        merge_skill_instance_mods(&mut mods, &mut effect, &[], input)?;
        let sge = effect.granted_effect.clone();
        let level = effect
            .level
            .and_then(|l| sge.level(l))
            .ok_or_else(|| format!("{} has no level {:?}", sge.id, effect.level))?;
        let source = sge
            .mod_source
            .clone()
            .unwrap_or_else(|| format!("Skill:{}", sge.id));
        if let Some(m) = level.field("manaMultiplier") {
            mods.push(new_mod(
                "SupportManaMultiplier",
                ModType::More,
                m,
                Some(&source),
                0,
                vec![],
            ));
        }
        if let Some(p) = level.field("manaReservationPercent") {
            out.skill_data
                .insert("manaReservationPercent".into(), ModValue::Number(p));
        }
        if sge.is_trigger && !has(&flags, "disable") {
            if out.triggered_by.is_some() {
                flags.insert("disable".into());
                out.disable_reason =
                    Some("This skill is supported by more than one trigger".into());
            } else {
                out.triggered_by = Some(i);
            }
        }
        if let Some(m) = level.field("PvPDamageMultiplier") {
            mods.push(new_mod(
                "PvpDamageMultiplier",
                ModType::More,
                m,
                Some(&source),
                0,
                vec![],
            ));
        }
        if let Some(u) = level.field("storedUses") {
            stored_uses = Some(u);
        }
        if let Some(u) = level.field("vaalStoredUses") {
            stored_uses = Some(stored_uses.unwrap_or(u));
        }
        skill.effect_list[i] = effect;
    }
    // The gem's level and quality
    let src_instance: Option<GemInstance> = src
        .and_then(|(g, i)| groups.get(g)?.gem_list.get(i))
        .cloned();
    let active = &skill.active_effect;
    let gem_level = src_instance.as_ref().and_then(|g| g.level).or(active.level);
    let gem_quality = src_instance
        .as_ref()
        .and_then(|g| g.quality)
        .or(Some(active.quality));
    mods.push(new_mod(
        "GemLevel",
        ModType::Base,
        gem_level.unwrap_or(f64::NAN),
        Some("Max Level"),
        0,
        vec![],
    ));
    mods.push(new_mod(
        "GemQuality",
        ModType::Base,
        gem_quality.unwrap_or(f64::NAN),
        Some("Max Quality"),
        0,
        vec![],
    ));
    if src_instance.as_ref().is_some_and(|g| g.matches_socket) {
        mods.push(new_mod(
            "GemSocketQuality",
            ModType::Base,
            input
                .setup
                .misc
                .get("MatchingSocketQualityBonus")
                .copied()
                .unwrap_or(f64::NAN),
            Some("Socket Quality"),
            0,
            vec![],
        ));
    }
    // Mods so far are queried with the list
    let set_mods = |db: &mut PlayerDb, mods: &[Mod]| {
        let s = &mut db.stores.stores[list_id];
        *s = Store::list();
        s.parent = Some(db.id);
        for m in mods {
            s.add(m.clone());
        }
    };
    set_mods(db, &mods);
    let rows: Vec<(ModValue, Mod)> = db
        .query()
        .tabulate(
            list_id,
            Some(ModType::List),
            Some(&cfg),
            &[Sym::new("SupportedGemProperty")],
        )?
        .into_iter()
        .map(|r| (r.value, r.m.clone()))
        .collect();
    for (value, m) in rows {
        let ModValue::Record(v) = &value else {
            continue;
        };
        let is_active_gem = skill
            .active_effect
            .gem
            .as_ref()
            .is_some_and(|g| !g.tags.contains("support"));
        if record_str(v, "keyword").as_deref() == Some("grants_active_skill") && is_active_gem {
            let key = record_str(v, "key").unwrap_or_default();
            let add = v
                .get("value")
                .and_then(ModValue::as_number)
                .unwrap_or(f64::NAN);
            let e = &mut skill.active_effect;
            match key.as_str() {
                "quality" => {
                    e.support_quality += add;
                    e.quality += add;
                }
                "level" => e.level = Some(e.level.unwrap_or(f64::NAN) + add),
                k => return Err(format!("supported gem property {k}")),
            }
            let name = format!("GemSupport{}", upper_first(&key));
            mods.push(new_mod(
                &name,
                ModType::Base,
                add,
                m.source.as_deref(),
                0,
                m.tags.first().cloned().into_iter().collect(),
            ));
        }
    }
    for (value, m) in &skill.active_effect.gem_property_info {
        let ModValue::Record(v) = value else { continue };
        let key = record_str(v, "key").unwrap_or_default();
        let add = v
            .get("value")
            .and_then(ModValue::as_number)
            .unwrap_or(f64::NAN);
        let name = format!("GemItem{}", upper_first(&key));
        mods.push(new_mod(
            &name,
            ModType::Base,
            add,
            m.source.as_deref(),
            0,
            m.tags.first().cloned().into_iter().collect(),
        ));
    }
    // The active gem's mods
    set_mods(db, &mods);
    let extra: Vec<(String, f64)> = db
        .query()
        .list(list_id, Some(&cfg), &[Sym::new("ExtraSkillStat")])?
        .iter()
        .filter_map(|v| match v {
            ModValue::Record(r) => Some((record_str(r, "key")?, r.get("value")?.as_number()?)),
            _ => None,
        })
        .collect();
    let mut active = skill.active_effect.clone();
    merge_skill_instance_mods(&mut mods, &mut active, &extra, input)?;
    skill.active_effect = active;
    let level = skill
        .active_effect
        .level
        .and_then(|l| ge.level(l))
        .ok_or_else(|| format!("{} has no level {:?}", ge.id, skill.active_effect.level))?
        .clone();
    let source = ge
        .mod_source
        .clone()
        .unwrap_or_else(|| format!("Skill:{}", ge.id));
    if let Some(c) = level.field("critChance") {
        out.skill_data
            .insert("CritChance".into(), ModValue::Number(c));
    }
    if let Some(m) = level.field("damageMultiplier") {
        mods.push(new_mod(
            "Damage",
            ModType::More,
            m,
            Some(&source),
            mf("Attack"),
            vec![],
        ));
    }
    for (field, key) in [
        ("attackTime", "attackTime"),
        ("attackSpeedMultiplier", "attackSpeedMultiplier"),
        ("cooldown", "cooldown"),
    ] {
        if let Some(v) = level.field(field) {
            out.skill_data.insert(key.into(), ModValue::Number(v));
        }
    }
    if let Some(u) = stored_uses {
        out.skill_data
            .insert("storedUses".into(), ModValue::Number(u));
    }
    if let Some(u) = level.field("storedUses") {
        out.skill_data
            .insert("storedUses".into(), ModValue::Number(u));
    }
    if let Some(u) = level.field("vaalStoredUses") {
        let cur = out
            .skill_data
            .get("storedUses")
            .and_then(ModValue::as_number);
        out.skill_data
            .insert("storedUses".into(), ModValue::Number(cur.unwrap_or(u)));
    }
    if let Some(d) = level.field("soulPreventionDuration") {
        out.skill_data
            .insert("soulPreventionDuration".into(), ModValue::Number(d));
    }
    if let Some(m) = level.field("PvPDamageMultiplier") {
        mods.push(new_mod(
            "PvpDamageMultiplier",
            ModType::More,
            m,
            Some(&source),
            0,
            vec![],
        ));
    }
    // Extra mods from other sources
    set_mods(db, &mods);
    for v in db
        .query()
        .list(list_id, Some(&cfg), &[Sym::new("ExtraSkillMod")])?
    {
        if let ModValue::Record(r) = &v
            && let Some(ModValue::Mod(m)) = r.get("mod")
        {
            mods.push((**m).clone());
            out.extra_skill_mod_list.push((**m).clone());
        }
    }
    if has(&flags, "totem") {
        out.skill_data.insert(
            "totemLevel".into(),
            ModValue::Number(level.level_requirement),
        );
    }
    // Mines and stages
    if has(&flags, "mine") {
        let count = src_gem(groups, src).and_then(|g| g.skill_mine_count);
        out.active_mine_count = count;
        if let Some(c) = count.filter(|c| *c > 0.0) {
            mods.push(new_mod(
                "Multiplier:ActiveMineCount",
                ModType::Base,
                c,
                Some("Base"),
                0,
                vec![],
            ));
            let e = enemy_multipliers
                .entry(Sym::new("ActiveMineCount"))
                .or_insert(0.0);
            *e = e.max(c);
        }
    } else if !secondary && let Some(g) = src_gem(groups, src) {
        g.skill_mine_count_calcs = None;
        g.skill_mine_count = None;
    }
    let no_potential_stage = !ge
        .parts
        .iter()
        .flatten()
        .any(|p| p.get("stages").is_some_and(ModValue::truthy));
    let stage_name = crate::pat::gsub(&ge.name, "%s+", "");
    set_mods(db, &mods);
    let sum = |db: &PlayerDb, name: &str| -> Result<f64> {
        db.query()
            .sum(list_id, ModType::Base, Some(&cfg), &[Sym::new(name)])
    };
    let limit = sum(db, &format!("Multiplier:{stage_name}MaxStages"))?;
    if limit > 0.0 {
        out.skill_data
            .insert("stagesMax".into(), ModValue::Number(limit));
        flags.insert("multiStage".into());
        let chosen = src_gem(groups, src).and_then(|g| g.skill_stage_count);
        let min = 1.0 + sum(db, &format!("Multiplier:{stage_name}MinimumStage"))?;
        let mut count = chosen.unwrap_or(limit).max(min);
        if count > 0.0 {
            mods.push(new_mod(
                &format!("Multiplier:{stage_name}Stage"),
                ModType::Base,
                limit.min(count),
                Some("Base"),
                0,
                vec![],
            ));
            count -= 1.0;
            mods.push(new_mod(
                &format!("Multiplier:{stage_name}StageAfterFirst"),
                ModType::Base,
                (limit - 1.0).min(count),
                Some("Base"),
                0,
                vec![],
            ));
        }
        out.active_stage_count = Some(count);
    } else if no_potential_stage
        && !secondary
        && let Some(g) = src_gem(groups, src)
    {
        g.skill_stage_count_calcs = None;
        g.skill_stage_count = None;
    }
    // Skill data
    set_mods(db, &mods);
    let skill_data_values = {
        let q = db.query();
        let mut v = q.list(db.id, Some(&cfg), &[Sym::new("SkillData")])?;
        v.extend(q.list(list_id, Some(&cfg), &[Sym::new("SkillData")])?);
        v
    };
    for v in skill_data_values {
        if let ModValue::Record(r) = &v
            && let Some(k) = record_str(r, "key")
        {
            match r.get("value") {
                Some(x) => out.skill_data.insert(k, x.clone()),
                None => out.skill_data.remove(&k),
            };
        }
    }
    // The minion
    make_minion(
        &mut out, skill, groups, &mut flags, &level, &cfg, db, list_id, input,
    )?;
    // Global effects become buffs
    set_mods(db, &mods);
    let mut kept = Vec::new();
    for m in mods {
        let effect = m.tags.iter().find_map(|t| match &t.kind {
            TagKind::Other { kind, fields } if kind.as_str() == "GlobalEffect" => {
                Some(fields.clone())
            }
            _ => None,
        });
        let Some(tag) = effect else {
            kept.push(m);
            continue;
        };
        if let Some(cond) = record_str(&tag, "modCond")
            && !db
                .query()
                .condition(list_id, Sym::new(&cond), Some(&cfg), false)?
        {
            continue;
        }
        let typ = record_str(&tag, "effectType").unwrap_or_default();
        let name = record_str(&tag, "effectName").unwrap_or_else(|| ge.name.clone());
        let i = match out
            .buff_list
            .iter()
            .position(|b| b.typ == typ && b.name == name)
        {
            Some(i) => i,
            None => {
                let truthy = |k: &str| tag.get(k).is_some_and(ModValue::truthy);
                let data_flag = |k: &str| out.skill_data.get(k).is_some_and(ModValue::truthy);
                let own = m.source.as_deref() == Some(source.as_str());
                let buff = Buff {
                    typ: typ.clone(),
                    name: name.clone(),
                    active_skill_buff: own,
                    apply_not_player: truthy("applyNotPlayer")
                        || (own && data_flag("buffNotPlayer")),
                    apply_minions: truthy("applyMinions") || (own && data_flag("buffMinions")),
                    apply_allies: own && data_flag("buffAllies"),
                    allow_totem_buff: if own {
                        data_flag("allowTotemBuff")
                    } else {
                        truthy("allowTotemBuff")
                    },
                    tag: tag.clone(),
                    mod_list: vec![],
                };
                out.buff_list.push(buff);
                out.buff_list.len() - 1
            }
        };
        let list = &mut out.buff_list[i].mod_list;
        match list.iter_mut().find(|d| {
            crate::mods::list::same_params(&m, d) && matches!(d.typ, ModType::Base | ModType::Inc)
        }) {
            Some(d) => {
                d.value = match (&d.value, &m.value) {
                    (ModValue::Number(a), ModValue::Number(b)) => ModValue::Number(a + b),
                    (a, b) => {
                        return Err(format!("attempt to perform arithmetic on {a:?} and {b:?}"));
                    }
                };
            }
            None => list.push(m),
        }
    }
    out.mods = kept;
    out.skill_flags = flags;
    skill.mods = Some(out);
    Ok(())
}

/// The gem an effect is of
fn src_gem(groups: &mut [SocketGroup], src: Option<(usize, usize)>) -> Option<&mut GemInstance> {
    let (g, i) = src?;
    groups.get_mut(g)?.gem_list.get_mut(i)
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The skill's minion: its type, level and weapons (`activeSkill.minion`)
#[allow(clippy::too_many_arguments)]
fn make_minion(
    out: &mut SkillMods,
    skill: &ActiveSkill,
    groups: &mut [SocketGroup],
    flags: &mut BTreeSet<String>,
    level: &crate::skill_data::SkillLevel,
    cfg: &Cfg,
    db: &PlayerDb,
    list_id: StoreId,
    input: &ModsInput,
) -> Result<()> {
    let ge = &skill.active_effect.granted_effect;
    let secondary = skill
        .active_effect
        .gem
        .as_ref()
        .is_some_and(|g| g.secondary_granted_effect.is_some());
    let gem = skill
        .active_effect
        .src
        .and_then(|(g, i)| groups.get_mut(g)?.gem_list.get_mut(i));
    let mut spectre = false;
    let mut list: Vec<String> = match &ge.minion_list {
        Some(l) if !l.is_empty() => l.clone(),
        Some(_) => {
            spectre = true;
            input.spectre_list.to_vec()
        }
        None => vec![],
    };
    let mut support_level: Vec<(String, f64)> = Vec::new();
    for e in &skill.effect_list {
        if e.granted_effect.support
            && let Some(add) = &e.granted_effect.add_minion_list
        {
            for t in add {
                if !list.contains(t) {
                    let req = e
                        .level
                        .and_then(|l| e.granted_effect.level(l))
                        .map(|l| l.level_requirement);
                    support_level.push((t.clone(), req.unwrap_or(f64::NAN)));
                    list.push(t.clone());
                }
            }
        }
    }
    out.minion_list = list.clone();
    let Some(gem) = gem else {
        return Ok(());
    };
    if list.is_empty() {
        if !secondary {
            gem.skill_minion = None;
            gem.skill_minion_calcs = None;
            gem.skill_minion_item_set = None;
            gem.skill_minion_item_set_calcs = None;
            gem.skill_minion_skill = None;
            gem.skill_minion_skill_calcs = None;
        }
        return Ok(());
    }
    let index = gem
        .skill_minion
        .as_ref()
        .and_then(|m| list.iter().position(|x| x == m))
        .unwrap_or(0);
    let typ = list[index].clone();
    gem.skill_minion = Some(typ.clone());
    flags.insert("haveMinion".into());
    let md = input
        .setup
        .minions
        .get(&typ)
        .ok_or_else(|| format!("no minion {typ}"))?;
    let hostile = md.flag("hostile");
    let data_num = |k: &str| out.skill_data.get(k).and_then(ModValue::as_number);
    let data_flag = |k: &str| out.skill_data.get(k).is_some_and(ModValue::truthy);
    let mut minion_level = if data_flag("minionLevelIsEnemyLevel") {
        input.enemy_level
    } else if let Some(cap) = data_num("minionLevelIsPlayerLevel") {
        input.character_level.min(cap)
    } else if let Some(l) = support_level
        .iter()
        .find(|(t, _)| *t == typ)
        .map(|(_, l)| *l)
    {
        l
    } else {
        data_num("minionLevel").unwrap_or(level.level_requirement)
    };
    minion_level = minion_level.max(1.0).min(100.0);
    let life_table = if hostile {
        "monsterLifeTable"
    } else {
        match md.str("lifeScaling") {
            Some("AltLife1") => "monsterLifeTable2",
            Some("AltLife2") => "monsterLifeTable3",
            _ if spectre => "monsterLifeTable",
            _ => "monsterAllyLifeTable",
        }
    };
    let attack_time = md.num("attackTime").unwrap_or(f64::NAN);
    let table = if spectre || hostile {
        &input.setup.monster_damage_table
    } else {
        &input.setup.monster_ally_damage_table
    };
    let mut damage = table
        .get(minion_level as usize - 1)
        .copied()
        .unwrap_or(f64::NAN)
        * md.num("damage").unwrap_or(f64::NAN);
    if !md.flag("baseDamageIgnoresAttackSpeed") {
        damage *= attack_time;
    }
    let mut item_set = None;
    if ge.minion_has_item_set {
        let tab = input.tab;
        let valid = gem
            .skill_minion_item_set
            .is_some_and(|id| id.fract() == 0.0 && tab.item_sets.contains_key(&(id as i64)));
        if !valid {
            gem.skill_minion_item_set = tab.item_set_order_list.first().copied();
        }
        item_set = gem.skill_minion_item_set;
    } else if !secondary {
        gem.skill_minion_item_set_calcs = None;
        gem.skill_minion_item_set = None;
    }
    let player = input.weapon_data;
    let player_bow = matches!(player[0].get("type"), Some(ModValue::Str(t)) if &**t == "Bow");
    let mut weapon1 = if (data_flag("minionUseBowAndQuiver") && player_bow)
        || data_flag("minionUseMainHandWeapon")
        || (input.iron_mass && typ == "RaisedSkeleton")
    {
        player[0].clone()
    } else {
        let spread = md.num("damageSpread").unwrap_or(f64::NAN);
        let mut w = Fields::new();
        w.insert(
            "type".into(),
            ModValue::Str(md.str("weaponType1").unwrap_or("None").into()),
        );
        w.insert("AttackRate".into(), ModValue::Number(1.0 / attack_time));
        w.insert("CritChance".into(), ModValue::Number(5.0));
        w.insert(
            "PhysicalMin".into(),
            ModValue::Number(round(damage * (1.0 - spread))),
        );
        w.insert(
            "PhysicalMax".into(),
            ModValue::Number(round(damage * (1.0 + spread))),
        );
        if let Some(r) = md.num("attackRange") {
            w.insert("range".into(), ModValue::Number(r));
        }
        w
    };
    let mut weapon2 = Fields::new();
    if let Some(uses) = &ge.minion_uses {
        let set_item = |slot: &str| -> Option<Fields> {
            let set = input.tab.item_sets.get(&(item_set? as i64))?;
            let name = if set.use_second_weapon_set {
                format!("{slot} Swap")
            } else {
                slot.to_string()
            };
            let item = input.tab.item(set.slots.get(&name)?.sel_item_id)?;
            item.weapon_data
                .as_ref()?
                .get(&if slot == "Weapon 1" { 1 } else { 2 })
                .cloned()
        };
        if uses.iter().any(|u| u == "Weapon 1") {
            if item_set.is_some() {
                if let Some(w) = set_item("Weapon 1") {
                    weapon1 = w;
                }
            } else {
                weapon1 = player[0].clone();
            }
        }
        if uses.iter().any(|u| u == "Weapon 2") {
            if item_set.is_some() {
                if let Some(w) = set_item("Weapon 2") {
                    weapon2 = w;
                }
            } else {
                weapon2 = player[1].clone();
            }
        }
    }
    let rate = player[0].get("AttackRate").and_then(ModValue::as_number);
    if !spectre
        && db.query().flag(
            list_id,
            Some(cfg),
            &[Sym::new("NonSpectreMinionsUseParentMainHandAttackTime")],
        )?
        && let Some(r) = rate.filter(|r| *r > 0.0)
    {
        weapon1.insert("AttackRate".into(), ModValue::Number(r));
    }
    out.minion = Some(Minion {
        typ,
        level: minion_level,
        hostile,
        life_table,
        uses: ge.minion_uses.clone(),
        item_set,
        weapon_data: [weapon1, weapon2],
    });
    Ok(())
}
