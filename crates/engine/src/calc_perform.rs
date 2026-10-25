//! `calcs.perform` (`Modules/CalcPerform.lua`) on the typed setup: merges
//! the keystones, sets up the skills' mod lists, applies flasks, attributes,
//! life and mana, reservations, charges, buffs, curses, auras and ailments,
//! up to the defence and offence calculations.
//!
//! Not ported yet: minions, the party tab's buffs, skills whose stages come
//! from their own cached calculation (Blight, Penance Brand, Earthquake),
//! the overcapped lightning resistance's mana and allies' life pools.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// Lua's `math.min` and `math.max`, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::actor::{Actor, EnvActors, OutVal, ReservingSkill};
use crate::calc::{Calc, ENEMY, PLAYER, m_max, m_min, number, round};
use crate::calc_env::{CalcData, Env};
use crate::calc_skill_mods::Buff;
use crate::config::{Inputs, Val};
use crate::item::Item;
use crate::items_tab::ItemsTab;
use crate::mods::tag::TagKind;
use crate::mods::{Cfg, Mod, ModType, ModValue, Store, StoreId, Sym, Tag, record};
use crate::tree::PassiveTree;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

use ModType::{Base, Flag as FlagT, Inc, List, Max as MaxT, More, Override};

/// What `perform` reads besides the setup
pub struct PerformInput<'a> {
    pub data: &'a CalcData<'a>,
    pub tree: &'a PassiveTree,
    pub tab: &'a ItemsTab,
    /// `env.configInput` and `env.configPlaceholder`
    pub config: &'a Inputs,
    pub placeholder: &'a Inputs,
    /// `build.spectreList`
    pub spectre_list: &'a [String],
    pub class_id: i64,
    /// The class's base attributes (`base_str`, `base_dex`, `base_int`)
    pub class_stats: [f64; 3],
    /// `env.spec.treeVersion`
    pub tree_version: &'a str,
    /// A calculation of the Calcs tab, which builds breakdowns
    pub breakdown: bool,
}

/// An active skill's mod lists during the calculation
#[derive(Clone, Copy, Debug)]
pub struct SkillLists {
    /// `baseSkillModList`
    pub base: StoreId,
    /// `skillModList`, whose parent is the base list
    pub list: StoreId,
}

/// What the buffs mark on an active skill
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillMarks {
    pub buff_skill: bool,
    pub minion_buff_skill: bool,
    pub debuff_skill: bool,
    pub totem_buff_skill: bool,
}

/// A curse to slot (`curses`, `minionCurses`)
#[derive(Clone, Debug)]
struct Curse {
    name: String,
    priority: f64,
    is_mark: bool,
    ignore_hex_limit: bool,
    socketed_curses_hex_limit: bool,
    mod_list: Option<Vec<Mod>>,
    buff_mod_list: Option<Vec<Mod>>,
}

/// Buffs by name, in the order they were first merged
pub(crate) type BuffMap = Vec<(String, Vec<Mod>, bool)>;

/// `mergeBuff(src, destTable, destKey)`: takes the highest value of each
/// mod with the same parameters
pub(crate) fn merge_buff(src: &[Mod], dest: &mut BuffMap, key: &str) -> Result<()> {
    let i = match dest.iter().position(|(k, _, _)| k == key) {
        Some(i) => i,
        None => {
            dest.push((key.to_string(), Vec::new(), false));
            dest.len() - 1
        }
    };
    let list = &mut dest[i].1;
    for m in src {
        let mut matched = false;
        if m.typ != List {
            for d in list.iter_mut() {
                if crate::mods::list::same_params(m, d) {
                    if let ModValue::Number(dv) = d.value
                        && number(&m.value)? > dv
                    {
                        *d = m.clone();
                    }
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            list.push(m.clone());
        }
    }
    Ok(())
}

/// `str:gsub(" ", "")`
pub(crate) fn nospace(s: &str) -> String {
    s.replace(' ', "")
}

/// `str:gsub("%s+", "")`
pub(crate) fn no_whitespace(s: &str) -> String {
    s.chars().filter(|c| !c.is_ascii_whitespace()).collect()
}

/// `calcLib.getConvertedModTags(mod, multiplier, minionMods)`
pub(crate) fn converted_tags(m: &Mod, multiplier: f64, minion_mods: bool) -> Vec<Tag> {
    m.tags
        .iter()
        .map(|t| {
            if minion_mods
                && let TagKind::ActorCondition {
                    actor: Some(a),
                    vars,
                    ..
                } = &t.kind
                && a.as_str() == "parent"
            {
                return Tag {
                    kind: TagKind::Condition {
                        vars: vars.clone().unwrap_or(crate::mods::Names::List(vec![])),
                        neg: false,
                        actor: None,
                    },
                    global_limit: None,
                };
            }
            let mut t = t.clone();
            match &mut t.kind {
                TagKind::Multiplier(x) if x.limit_total => {
                    x.limit = x.limit.map(|l| l * multiplier)
                }
                TagKind::PerStat(x) if x.limit_total => x.limit = x.limit.map(|l| l * multiplier),
                TagKind::PercentStat(x) if x.limit_total => {
                    x.limit = x.limit.map(|l| l * multiplier)
                }
                _ => {}
            }
            t
        })
        .collect()
}

/// A mod with flags and tags (`modLib.createMod(name, type, value,
/// source, flags, keywordFlags, ...)`)
pub(crate) fn new_mod(
    name: &str,
    typ: ModType,
    value: impl Into<ModValue>,
    source: Option<&str>,
    flags: u32,
    keyword_flags: u32,
    tags: Vec<Tag>,
) -> Mod {
    let mut m = Mod::new(name, typ, value, source).flags(flags, keyword_flags);
    for t in tags {
        m = m.tag(t);
    }
    m
}

pub(crate) fn src(name: &str, typ: ModType, value: impl Into<ModValue>, source: &str) -> Mod {
    Mod::new(name, typ, value, Some(source))
}

/// A `GlobalEffect` tag
pub(crate) fn global_effect(fields: &[(&str, ModValue)]) -> Tag {
    let mut t = Tag::other("GlobalEffect", []);
    if let TagKind::Other { fields: f, .. } = &mut t.kind {
        for (k, v) in fields {
            f.fields.push((Sym::new(k), v.clone()));
        }
    }
    t
}

pub(crate) fn multiplier_tag(var: &str, limit: Option<f64>) -> Tag {
    let mut t = Tag::multiplier(var);
    if let Some(l) = limit {
        t = t.limit(l);
    }
    t
}

/// The calculation after `perform`
pub struct Perform<'a> {
    pub c: Calc<'a>,
    pub env: Env,
    pub input: PerformInput<'a>,
    /// Each active skill's lists, in `env.player.activeSkillList` order
    pub lists: Vec<SkillLists>,
    pub marks: Vec<SkillMarks>,
    /// `env.keystonesAdded`
    pub keystones_added: Vec<String>,
    /// `actor.strDmgBonus` of the player
    pub str_dmg_bonus: Option<f64>,
    /// `env.player.reserved_*`, `uncancellable_*`
    pub reserved: [(f64, f64); 2],
    pub uncancellable: [f64; 2],
    /// `actor.appliedEnemyModifiers` of each actor: the mods added
    pub(crate) applied_enemy_modifiers: Vec<Vec<Mod>>,
    /// The curses in their slots (`env.curseSlots`), by name
    pub curse_slots: Vec<String>,
    /// `output.HasBonechill` and whether some skill guarantees it
    pub(crate) has_guaranteed_bonechill: bool,
    /// `env.weaponData1`, `env.player.weaponData1` after Dancing Dervish
    pub weapons_disabled: bool,
}

impl<'a> Perform<'a> {
    pub(crate) fn data(&self) -> &crate::calc_setup::SetupData {
        self.input.data.setup
    }

    pub(crate) fn mf(&self, n: &str) -> u32 {
        self.input.data.setup.mod_flag(n)
    }

    pub(crate) fn kf(&self, n: &str) -> u32 {
        self.input.data.setup.keyword_flag(n)
    }

    pub(crate) fn st(&self, n: &str) -> i64 {
        self.input.data.skills.skill_type(n).unwrap_or(-1)
    }

    pub(crate) fn misc_data(&self, n: &str) -> f64 {
        self.data().misc(n)
    }

    pub(crate) fn pdb(&self) -> StoreId {
        self.c.db(PLAYER)
    }

    pub(crate) fn edb(&self) -> StoreId {
        self.c.db(ENEMY)
    }

    pub(crate) fn main(&self) -> usize {
        self.env.skills.main_skill
    }

    pub(crate) fn skill(&self, i: usize) -> &crate::calc_skills::ActiveSkill {
        &self.env.skills.active_skills[i]
    }

    pub(crate) fn mods(&self, i: usize) -> &crate::calc_skill_mods::SkillMods {
        self.env.skills.active_skills[i]
            .mods
            .as_ref()
            .expect("an active skill with its mod list")
    }

    pub(crate) fn mods_mut(&mut self, i: usize) -> &mut crate::calc_skill_mods::SkillMods {
        self.env.skills.active_skills[i]
            .mods
            .as_mut()
            .expect("an active skill with its mod list")
    }

    pub(crate) fn cfg(&self, i: usize) -> Cfg {
        self.mods(i).skill_cfg.clone()
    }

    pub(crate) fn has_flag(&self, i: usize, f: &str) -> bool {
        self.mods(i).skill_flags.contains(f)
    }

    pub(crate) fn has_type(&self, i: usize, t: &str) -> bool {
        self.skill(i).skill_types.contains(&self.st(t))
    }

    pub(crate) fn skill_name(&self, i: usize) -> &str {
        &self.skill(i).active_effect.granted_effect.name
    }

    pub(crate) fn sd(&self, i: usize, k: &str) -> Option<&ModValue> {
        self.mods(i).skill_data.get(k)
    }

    pub(crate) fn sd_num(&self, i: usize, k: &str) -> Option<f64> {
        self.sd(i, k).and_then(ModValue::as_number)
    }

    pub(crate) fn sd_truthy(&self, i: usize, k: &str) -> bool {
        self.sd(i, k).is_some_and(ModValue::truthy)
    }

    pub(crate) fn item(&self, actor: usize, slot: &str) -> Option<&'a Item> {
        let id = *self.c.actors.actors[actor].item_list.get(slot)?;
        self.input.tab.item(id)
    }

    pub(crate) fn config_num(&self, k: &str) -> Option<f64> {
        match self.input.config.get(k) {
            Some(Val::Number(n)) => Some(*n),
            _ => None,
        }
    }

    pub(crate) fn config_truthy(&self, k: &str) -> bool {
        match self.input.config.get(k) {
            Some(Val::Bool(b)) => *b,
            Some(_) => true,
            None => false,
        }
    }

    pub(crate) fn weapon(&self, actor: usize, n: usize) -> Option<&crate::item::Fields> {
        self.c.actors.actors[actor].weapon_data[n].as_ref()
    }

    pub(crate) fn weapon_str(&self, actor: usize, n: usize, k: &str) -> Option<Rc<str>> {
        match self.weapon(actor, n)?.get(k)? {
            ModValue::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// `modLib.mergeKeystones(env, modDB)`
    fn merge_keystones(&mut self, db: StoreId) -> Result<()> {
        let grants: Vec<(String, Option<Rc<str>>)> = self
            .c
            .tabulate(db, Some(List), None, &["Keystone"])?
            .into_iter()
            .filter_map(|(v, m)| match v {
                ModValue::Str(v) => Some((v.to_string(), m.source.clone())),
                _ => None,
            })
            .collect();
        for (name, source) in grants {
            if self.keystones_added.contains(&name) {
                continue;
            }
            let Some(mods) = self.input.tree.keystone_mods(&name) else {
                continue;
            };
            self.keystones_added.push(name);
            let from_other = source
                .as_ref()
                .is_some_and(|s| !s.to_ascii_lowercase().contains("tree"));
            for m in mods.iter().cloned() {
                let mut m = m;
                if from_other {
                    crate::calc_setup::set_source(&mut m, source.as_deref().unwrap_or(""));
                }
                self.c.add(db, m);
            }
        }
        Ok(())
    }

    /// `applyEnemyModifiers(actor, clearCache)`
    fn apply_enemy_modifiers(&mut self, actor: usize, clear: bool) -> Result<()> {
        if clear {
            self.applied_enemy_modifiers[actor].clear();
        }
        let db = self.c.db(actor);
        let enemy = self.c.actors.actors[actor]
            .enemy
            .ok_or("an actor without an enemy")?;
        let edb = self.c.db(enemy);
        for (v, m) in self.c.tabulate(db, None, None, &["EnemyModifier"])? {
            let ModValue::Record(r) = &v else { continue };
            let Some(ModValue::Mod(inner)) = r.get("mod") else {
                continue;
            };
            if self.applied_enemy_modifiers[actor].contains(inner) {
                continue;
            }
            let source = inner.source.clone().or(m.source.clone());
            let mut added = (**inner).clone();
            crate::calc_setup::set_source(&mut added, source.as_deref().unwrap_or(""));
            if source.is_none() {
                added.source = None;
            }
            self.c.add(edb, added);
            self.applied_enemy_modifiers[actor].push((**inner).clone());
        }
        Ok(())
    }
}

/// `calcs.perform(env)` up to the defence and offence calculations
pub fn perform<'a>(env: Env, input: PerformInput<'a>) -> Result<Perform<'a>> {
    for skill in &env.skills.active_skills {
        if skill.mods.as_ref().is_some_and(|m| m.minion.is_some()) {
            return Err("not ported: minions".into());
        }
    }
    let mut stores = crate::mods::Stores::default();
    let pdb = stores.push(env.player_db.clone());
    let edb = stores.push(env.enemy_db.clone());
    let mut item_list = env.setup.items.item_list.clone();
    if env.setup.items.aegis_mods.is_some() {
        item_list.remove("Weapon 2");
    }
    let weapon_data = env.skills.weapon_data.clone();
    let actors = EnvActors {
        actors: vec![
            Actor {
                store: pdb,
                item_list,
                weapon_data: [Some(weapon_data[0].clone()), Some(weapon_data[1].clone())],
                player: Some(0),
                enemy: Some(1),
                ..Actor::default()
            },
            Actor {
                store: edb,
                player: Some(0),
                enemy: Some(0),
                ..Actor::default()
            },
        ],
        store_actor: [(pdb, 0), (edb, 1)].into_iter().collect(),
        tab: input.tab,
        item_data: input.data.items,
        skill_data: input.data.skills,
    };
    let c = Calc {
        stores,
        actors,
        precision: input.data.items,
        replaced: HashSet::new(),
    };
    let n = env.skills.active_skills.len();
    let mut p = Perform {
        c,
        env,
        input,
        lists: Vec::with_capacity(n),
        marks: vec![SkillMarks::default(); n],
        keystones_added: Vec::new(),
        str_dmg_bonus: None,
        reserved: [(0.0, 0.0); 2],
        uncancellable: [0.0; 2],
        applied_enemy_modifiers: vec![Vec::new(), Vec::new()],
        curse_slots: Vec::new(),
        has_guaranteed_bonechill: false,
        weapons_disabled: false,
    };
    p.run()?;
    Ok(p)
}

impl<'a> Perform<'a> {
    fn run(&mut self) -> Result<()> {
        let pdb = self.pdb();
        let edb = self.edb();
        // Merge keystone modifiers
        self.keystones_added.clear();
        self.merge_keystones(pdb)?;

        // The skills' mod lists
        for i in 0..self.env.skills.active_skills.len() {
            let mut base = Store::list();
            base.parent = Some(pdb);
            for m in &self.mods(i).mods {
                base.add(m.clone());
            }
            let base = self.c.push_store(base, Some(PLAYER));
            let mut list = Store::list();
            list.parent = Some(base);
            let list = self.c.push_store(list, Some(PLAYER));
            self.lists.push(SkillLists { base, list });
        }
        self.update_reserving_skills();

        if self.c.flag(pdb, None, &["AlchemistsGenius"])? {
            let effect = 1.0 + self.c.inc(pdb, None, &["BuffEffectOnSelf"])? / 100.0;
            self.c.add(
                pdb,
                src(
                    "FlaskEffect",
                    Inc,
                    (10.0 * effect).floor(),
                    "Alchemist's Genius",
                ),
            );
            self.c.add(
                pdb,
                src(
                    "FlaskChargesGained",
                    Inc,
                    (20.0 * effect).floor(),
                    "Alchemist's Genius",
                ),
            );
        }

        // Banners
        if self.c.flag(pdb, None, &["Condition:BannerPlanted"])? {
            let max = self.c.base(pdb, None, &["MaximumValour"])?;
            let stacks = self.c.base(pdb, None, &["Multiplier:ValourStacks"])?;
            self.c.add(
                pdb,
                src("Multiplier:BannerValour", Base, m_min(stacks, max), "Base"),
            );
        }
        if self.c.flag(pdb, None, &["CryWolfMinimumPower"])?
            && self.c.base(pdb, None, &["WarcryPower"])? < 10.0
        {
            self.c.add(
                pdb,
                src(
                    "WarcryPower",
                    Override,
                    10.0,
                    "Minimum Warcry Power from CryWolf",
                ),
            );
        }
        if self.c.flag(pdb, None, &["WarcryInfinitePower"])? {
            self.c.add(
                pdb,
                src(
                    "WarcryPower",
                    Override,
                    999999.0,
                    "Warcries have infinite power",
                ),
            );
        }
        let warcry_power = match self.c.override_num(pdb, None, &["WarcryPower"])? {
            Some(v) => v,
            None => self.c.base(pdb, None, &["WarcryPower"])?,
        };
        self.c.set_out(PLAYER, "WarcryPower", warcry_power);
        self.c.set_mult(pdb, "WarcryPower", warcry_power);

        self.apply_enemy_modifiers(PLAYER, true)?;
        self.apply_enemy_modifiers(ENEMY, true)?;

        self.skill_setup()?;

        // Special rarity and quantity for Bisco's
        let q = self.c.inc(pdb, None, &["LootQuantityNormalEnemies"])?;
        let v = if q > 0.0 {
            q + self.c.inc(pdb, None, &["LootQuantity"])?
        } else {
            0.0
        };
        self.c.set_out(PLAYER, "LootQuantityNormalEnemies", v);
        let r = self.c.inc(pdb, None, &["LootRarityMagicEnemies"])?;
        let v = if r > 0.0 {
            r + self.c.inc(pdb, None, &["LootRarity"])?
        } else {
            0.0
        };
        self.c.set_out(PLAYER, "LootRarityMagicEnemies", v);

        if self.c.flag(pdb, None, &["ConvertArmourESToLife"])? {
            for slot in [
                "Helmet",
                "Gloves",
                "Boots",
                "Body Armour",
                "Weapon 2",
                "Weapon 3",
            ] {
                if let Some(es) = self
                    .item(PLAYER, slot)
                    .and_then(|i| i.armour_data.as_ref())
                    .map(|a| a.get("EnergyShield").and_then(ModValue::as_number))
                {
                    let es = es.unwrap_or(0.0);
                    if es > 0.0 {
                        self.c.add(
                            pdb,
                            src("Life", Base, es, &format!("{slot} ES to Life Conversion")),
                        );
                    }
                }
            }
        }

        for element in ["Lightning", "Fire", "Cold", "Chaos", "Physical"] {
            if self.c.flag(
                pdb,
                None,
                &[&format!("{element}DamageAppliesTo{element}AuraEffect")],
            )? {
                let multiplier = self.c.base(
                    pdb,
                    None,
                    &[&format!(
                        "Improved{element}DamageAppliesTo{element}AuraEffect"
                    )],
                )? / 100.0;
                let limit = self.c.max(
                    pdb,
                    None,
                    &[&format!("{element}DamageAppliesTo{element}AuraEffectLimit")],
                )?;
                let mut total = 0.0;
                let cfg = Cfg::default();
                for (_, m) in
                    self.c
                        .tabulate(pdb, Some(Inc), Some(&cfg), &[&format!("{element}Damage")])?
                {
                    let tags = converted_tags(&m, multiplier, true);
                    let mut converted = (number(&m.value)? * multiplier).floor();
                    if let Some(limit) = limit
                        && converted > 0.0
                    {
                        let remaining = limit - total;
                        if remaining <= 0.0 {
                            converted = 0.0;
                        } else {
                            converted = m_min(converted, remaining);
                            total += converted;
                        }
                    }
                    if converted != 0.0 {
                        let kw = self.kf(element);
                        self.c.add(
                            pdb,
                            new_mod(
                                "AuraEffect",
                                m.typ,
                                converted,
                                m.source.as_deref(),
                                m.flags,
                                kw,
                                tags,
                            ),
                        );
                    }
                }
            }
        }

        if self.c.flag(pdb, None, &["MinionLifeAppliesToPlayer"])? {
            let multiplier = self
                .c
                .max(pdb, None, &["ImprovedMinionLifeAppliesToPlayer"])?
                .unwrap_or(100.0)
                / 100.0;
            for v in self.c.list(pdb, None, &["MinionModifier"])? {
                let ModValue::Record(r) = &v else { continue };
                let Some(ModValue::Mod(m)) = r.get("mod") else {
                    continue;
                };
                if m.name.as_str() == "Life" && m.typ == Inc {
                    let tags = converted_tags(m, multiplier, true);
                    let value = (number(&m.value)? * multiplier).floor();
                    let m = new_mod(
                        "Life",
                        Inc,
                        value,
                        m.source.as_deref(),
                        m.flags,
                        m.keyword_flags,
                        tags,
                    );
                    self.c.add(pdb, m);
                }
            }
        }

        // Mageblood
        let left = self.c.base(pdb, None, &["LeftActiveMagicUtilityFlasks"])?;
        let right = self.c.base(pdb, None, &["RightActiveMagicUtilityFlasks"])?;
        let mut flasks: Vec<i64> = self.env.setup.items.flasks.clone();
        if left > 0.0 || right > 0.0 {
            let mut magic = Vec::new();
            for slot in &self.input.tab.ordered_slots {
                let Some(item) = self.input.tab.item(slot.sel_item_id) else {
                    continue;
                };
                let base = item.base_name.as_deref().unwrap_or("");
                if item.typ.as_deref() == Some("Flask")
                    && item.rarity == "MAGIC"
                    && !(base.contains("Life Flask")
                        || base.contains("Mana Flask")
                        || base.contains("Hybrid Flask"))
                {
                    magic.push(slot.sel_item_id as i64);
                }
            }
            let n = magic.len() as f64;
            if left > 0.0 {
                let mut i = 1.0;
                while i <= m_min(left, n) {
                    let id = magic[i as usize - 1];
                    if !flasks.contains(&id) {
                        flasks.push(id);
                    }
                    i += 1.0;
                }
            }
            if right > 0.0 {
                let mut i = n;
                while i >= m_max(n - right + 1.0, 1.0) {
                    let id = magic[i as usize - 1];
                    if !flasks.contains(&id) {
                        flasks.push(id);
                    }
                    i -= 1.0;
                }
            }
        }

        let breakdown = self.input.breakdown;
        let effect_inc = self
            .c
            .inc(pdb, Some(&actor_cfg("player")), &["FlaskEffect"])?;
        if breakdown {
            let charges = self.c.base(pdb, None, &["FlaskChargesGenerated"])?;
            let used = flasks.len() as f64;
            let per_flask = self
                .c
                .base(pdb, None, &["FlaskChargesGeneratedPerEmptyFlask"])?
                * (5.0 - used);
            let total = charges + per_flask;
            let utility = self.c.base(pdb, None, &["UtilityFlaskChargesGenerated"])?;
            let life = self.c.base(pdb, None, &["LifeFlaskChargesGenerated"])?;
            let mana = self.c.base(pdb, None, &["ManaFlaskChargesGenerated"])?;
            self.c.set_out(PLAYER, "FlaskEffect", effect_inc);
            self.c.set_out(PLAYER, "FlaskChargeGen", total);
            self.c.set_out(PLAYER, "LifeFlaskChargeGen", total + life);
            self.c.set_out(PLAYER, "ManaFlaskChargeGen", total + mana);
            self.c
                .set_out(PLAYER, "UtilityFlaskChargeGen", total + utility);
            let crit = self.c.base(pdb, None, &["FlaskChargeOnCritChance"])?;
            self.c
                .set_out(PLAYER, "FlaskChargeOnCritChance", m_min(100.0, crit));
        }

        let mode = self.env.mode;
        if mode.combat {
            self.merge_flasks(&flasks, false, true)?;
            self.merge_keystones(pdb)?;
        }

        // Attributes, life and mana
        self.attribs_conditions(PLAYER)?;
        self.life_mana(PLAYER)?;

        self.reservations()?;
        let mana_lightning = self
            .c
            .flag(pdb, None, &["ManaIncreasedByOvercappedLightningRes"])?;
        self.life_mana_reservation(PLAYER, !mana_lightning)?;

        if mode.combat {
            self.merge_tinctures()?;
        }

        self.attribute_requirements()?;

        // Heralds and auras affecting self
        if mode.buffs {
            let mut heralds: Vec<String> = Vec::new();
            let mut auras: Vec<String> = Vec::new();
            for i in 0..self.lists.len() {
                let name = self
                    .mods(i)
                    .skill_cfg
                    .skill_name
                    .as_deref()
                    .unwrap_or("")
                    .to_string();
                if self.has_type(i, "Herald") && !heralds.contains(&name) {
                    heralds.push(name);
                    let n = self.c.mult(pdb, "Herald").unwrap_or(0.0);
                    self.c.set_mult(pdb, "Herald", n + 1.0);
                    self.c.set_cond(pdb, "AffectedByHerald", true);
                } else if self.has_type(i, "Aura")
                    && !self.has_type(i, "AuraAffectsEnemies")
                    && !self.sd_truthy(i, "auraCannotAffectSelf")
                    && !auras.contains(&name)
                {
                    auras.push(name);
                    let n = self.c.mult(pdb, "AuraAffectingSelf").unwrap_or(0.0);
                    self.c.set_mult(pdb, "AuraAffectingSelf", n + 1.0);
                }
            }
        }

        if self.c.flag(pdb, None, &["ManaAppliesToShockEffect"])? {
            let multiplier = self
                .c
                .max(pdb, None, &["ImprovedManaAppliesToShockEffect"])?
                .unwrap_or(100.0)
                / 100.0;
            for (_, m) in self.c.tabulate(pdb, Some(Inc), None, &["Mana"])? {
                let tags = converted_tags(&m, multiplier, false);
                let value = (number(&m.value)? * multiplier).floor();
                let m = new_mod(
                    "EnemyShockEffect",
                    Inc,
                    value,
                    m.source.as_deref(),
                    m.flags,
                    m.keyword_flags,
                    tags,
                );
                self.c.add(pdb, m);
            }
        }

        self.charges(PLAYER)?;

        self.buffs(&flasks)?;

        // Dancing Dervish
        if self.c.flag(pdb, None, &["DisableWeapons"])? {
            let unarmed = self
                .data()
                .unarmed_weapon_data
                .get(&self.input.class_id)
                .cloned()
                .unwrap_or_default();
            self.c.actors.actors[PLAYER].weapon_data[0] = Some(unarmed);
            self.weapons_disabled = true;
            self.c.set_cond(pdb, "Unarmed", true);
            // `env.player.Gloves` is never set
            self.c.set_cond(pdb, "Unencumbered", true);
        } else if let Some(w) = self.env.setup.items.weapon1_mods.clone() {
            self.c.add_list(pdb, &w);
        }

        self.defence_for_conditionals(PLAYER)?;

        self.charges(PLAYER)?;
        self.misc(PLAYER)?;

        self.ailments()?;

        self.charges(ENEMY)?;
        self.misc(ENEMY)?;

        self.exposures()?;

        if self.c.flag(edb, None, &["Condition:OnConsecratedGround"])? {
            let effect = 1.0 + self.c.inc(pdb, None, &["ConsecratedGroundEffect"])? / 100.0;
            let v = (self.c.inc(edb, None, &["DamageTakenConsecratedGround"])? * effect).floor();
            self.c
                .add(edb, src("DamageTaken", Inc, v, "Consecrated Ground"));
        }

        self.ally_life()?;
        Ok(())
    }
}

/// `{ actor = name }`
pub(crate) fn actor_cfg(actor: &str) -> Cfg {
    Cfg {
        actor: Some(Sym::new(actor)),
        ..Cfg::default()
    }
}

/// `{ source = name }`
pub(crate) fn source_cfg(source: &str) -> Cfg {
    Cfg {
        source: Some(source.into()),
        ..Cfg::default()
    }
}

impl<'a> Perform<'a> {
    /// What `GetStat` reads of the player's skills
    fn update_reserving_skills(&mut self) {
        let mut out = Vec::new();
        for i in 0..self.lists.len() {
            let m = self.mods(i);
            if self.has_type(i, "HasReservation")
                && !m.skill_flags.contains("disable")
                && !m.buff_list.is_empty()
            {
                out.push(ReservingSkill {
                    buff_names: m.buff_list.iter().map(|b| b.name.clone()).collect(),
                    mana_reserved_base: self.sd_num(i, "ManaReservedBase"),
                    life_reserved_base: self.sd_num(i, "LifeReservedBase"),
                });
            }
        }
        self.c.actors.actors[PLAYER].reserving_skills = out;
    }

    /// The loop over the active skills before the flasks: brands, totems,
    /// doom, ground effects, minion counts, warcries
    fn skill_setup(&mut self) -> Result<()> {
        let pdb = self.pdb();
        let edb = self.edb();
        let main = self.main();
        let main_list = self.lists[main].list;
        let main_cfg = self.cfg(main);
        // Minion counts by key: (total, nonVaal, permanent)
        let mut minion_counts: Vec<(String, f64, Option<f64>, Option<f64>)> = Vec::new();
        for i in 0..self.lists.len() {
            let list = self.lists[i].list;
            let cfg = self.cfg(i);
            let name = self.skill_name(i).to_string();
            if self.has_type(i, "Brand") {
                let attach_limit = m_min(
                    self.c.base(list, Some(&cfg), &["BrandsAttachedLimit"])?,
                    self.c.base(list, Some(&cfg), &["ActiveBrandLimit"])?,
                );
                let configured =
                    self.c
                        .base(pdb, None, &["Multiplier:ConfigBrandsAttachedToEnemy"])?;
                let attached = if configured > 0.0 {
                    m_min(configured, attach_limit)
                } else {
                    attach_limit
                };
                self.mods_mut(i)
                    .skill_data
                    .insert("attachedBrandCount".into(), ModValue::Number(attached));
                let active = self.c.base(pdb, None, &["Multiplier:ConfigActiveBrands"])?;
                let limit = self.c.base(list, Some(&cfg), &["ActiveBrandLimit"])?;
                let cur = self.c.mult(pdb, "ActiveBrand").unwrap_or(0.0);
                self.c
                    .set_mult(pdb, "ActiveBrand", m_max(m_min(active, limit), cur));
                let cur = self.c.mult(pdb, "BrandsAttachedToEnemy").unwrap_or(0.0);
                self.c
                    .set_mult(pdb, "BrandsAttachedToEnemy", m_max(attached, cur));
                let cur = self.c.mult(edb, "BrandsAttached").unwrap_or(0.0);
                self.c.set_mult(edb, "BrandsAttached", m_max(attached, cur));
            }
            if self.has_flag(i, "totem") {
                let limit = self.c.base(
                    main_list,
                    Some(&main_cfg),
                    &["ActiveTotemLimit", "ActiveBallistaLimit"],
                )?;
                let cur = self.c.out(PLAYER, "ActiveTotemLimit").unwrap_or(0.0);
                let limit = m_max(limit, cur);
                self.c.set_out(PLAYER, "ActiveTotemLimit", limit);
                let summoned = self
                    .c
                    .override_num(pdb, None, &["TotemsSummoned"])?
                    .unwrap_or(limit);
                self.c.set_out(PLAYER, "TotemsSummoned", summoned);
                let cur = self.c.mult(edb, "TotemsSummoned").unwrap_or(0.0);
                self.c.set_mult(edb, "TotemsSummoned", m_max(summoned, cur));
            }
            if self.has_flag(i, "hex")
                && self.has_flag(i, "curse")
                && !self.has_type(i, "TotemCastsWhenNotDetached")
            {
                let hex_doom = self.c.base(pdb, None, &["Multiplier:HexDoomStack"])?;
                let max_doom = self.c.base(list, None, &["MaxDoom"])?;
                let doom_effect = self.c.more(list, None, &["DoomEffect"])?;
                let cur = self.c.out(PLAYER, "HexDoomLimit").unwrap_or(0.0);
                let limit = m_max(max_doom, cur);
                self.c.set_out(PLAYER, "HexDoomLimit", limit);
                self.c.add(
                    list,
                    src(
                        "CurseEffect",
                        Inc,
                        m_min(hex_doom, max_doom) * doom_effect,
                        "Doom",
                    ),
                );
                let cur = self.c.mult(pdb, "HexDoom").unwrap_or(0.0);
                self.c
                    .set_mult(pdb, "HexDoom", m_min(m_max(hex_doom, cur), limit));
            }
            if name == "Vaal Lightning Trap" || name == "Shock Ground" {
                let effect = self.c.base(list, None, &["ShockedGroundEffect"])?
                    * (1.0 + self.c.inc(list, None, &["EnemyShockEffect"])? / 100.0);
                let m = src("ShockOverride", Base, effect, "Shocked Ground")
                    .tag(Tag::actor_condition("enemy", "OnShockedGround"));
                self.c.add(pdb, m);
            }
            let bonechill = self.sd_truthy(i, "supportBonechill");
            if bonechill
                && (self.has_type(i, "ChillingArea")
                    || self.has_type(i, "NonHitChill")
                    || !self.c.flag(list, None, &["CannotChill"])?)
            {
                self.c
                    .set_out_val(PLAYER, "HasBonechill", OutVal::Bool(true));
            }
            let ailment_default = |p: &Self, a: &str| {
                p.data()
                    .non_damaging_ailment
                    .get(a)
                    .and_then(|x| x.default)
                    .unwrap_or(f64::NAN)
            };
            if name == "Summon Skitterbots" {
                let skitter = self.c.inc(list, None, &["SkitterbotAilmentEffect"])?;
                let skill = source_cfg("Skill");
                let affect_player = self.c.flag(list, None, &["SkitterbotAffectPlayer"])?;
                if !self.c.flag(list, None, &["SkitterbotsCannotShock"])? {
                    let effect = ailment_default(self, "Shock")
                        * (1.0
                            + (self.c.inc(list, Some(&skill), &["EnemyShockEffect"])? + skitter)
                                / 100.0);
                    self.c.add(pdb, src("ShockOverride", Base, effect, &name));
                    self.c
                        .add(edb, src("Condition:Shocked", FlagT, true, &name));
                    if affect_player {
                        self.c.add(pdb, src("Shock", FlagT, true, &name));
                        self.c
                            .add(pdb, src("SelfShockOverride", Base, effect, &name));
                    }
                }
                if !self.c.flag(list, None, &["SkitterbotsCannotChill"])? {
                    let effect = ailment_default(self, "Chill")
                        * (1.0
                            + (self.c.inc(list, Some(&skill), &["EnemyChillEffect"])? + skitter)
                                / 100.0);
                    self.c.add(pdb, src("ChillOverride", Base, effect, &name));
                    self.c
                        .add(edb, src("Condition:Chilled", FlagT, true, &name));
                    if affect_player {
                        self.c.add(pdb, src("Chill", FlagT, true, &name));
                        self.c
                            .add(pdb, src("SelfChillOverride", Base, effect, &name));
                    }
                    if bonechill {
                        self.has_guaranteed_bonechill = true;
                        self.c
                            .add(pdb, src("SkitterbotBonechill", FlagT, true, &name));
                    }
                }
                if self.c.flag(list, None, &["ScorchingSkitterbot"])? {
                    let effect = ailment_default(self, "Scorch")
                        * (1.0
                            + (self.c.inc(list, Some(&skill), &["EnemyScorchEffect"])? + skitter)
                                / 100.0);
                    self.c.add(pdb, src("ScorchOverride", Base, effect, &name));
                    self.c
                        .add(edb, src("Condition:Scorched", FlagT, true, &name));
                    if affect_player {
                        self.c.add(pdb, src("Scorch", FlagT, true, &name));
                        self.c
                            .add(pdb, src("SelfScorchOverride", Base, effect, &name));
                    }
                }
            } else if self.has_type(i, "ChillingArea")
                || (self.has_type(i, "NonHitChill")
                    && !self.c.flag(list, None, &["CannotChill"])?)
            {
                let effect = ailment_default(self, "Chill")
                    * self.c.calc_mod(list, Some(&cfg), &["EnemyChillEffect"])?;
                self.c.add(pdb, src("ChillOverride", Base, effect, &name));
                self.c
                    .add(edb, src("Condition:Chilled", FlagT, true, &name));
                if bonechill {
                    self.has_guaranteed_bonechill = true;
                }
            }
            // Active minions; skills without a limit count one
            let minion_list = self.mods(i).minion_list.clone();
            if !self.has_flag(i, "disable") && !minion_list.is_empty() {
                let id = self.skill(i).active_effect.granted_effect.id.clone();
                for typ in &minion_list {
                    let Some(md) = self.data().minions.get(typ) else {
                        continue;
                    };
                    if md.flag("hostile") {
                        continue;
                    }
                    let limit = md.str("limit").map(String::from);
                    let key = limit.clone().unwrap_or_else(|| format!("{id}:{typ}"));
                    let mut count = 1.0;
                    if let Some(limit) = &limit {
                        count = match self.c.override_num(pdb, None, &[limit])? {
                            Some(v) => v,
                            None => {
                                self.c.val(list, limit, None)?
                                    * self.c.more(list, Some(&cfg), &["ActiveMinionLimit"])?
                            }
                        }
                        .floor();
                        let cur = self.c.out(PLAYER, limit).unwrap_or(0.0);
                        self.c.set_out(PLAYER, limit, m_max(count, cur));
                    }
                    let vaal = self.has_type(i, "Vaal");
                    let permanent = self.has_flag(i, "permanentMinion");
                    let pos = match minion_counts.iter().position(|c| c.0 == key) {
                        Some(p) => p,
                        None => {
                            minion_counts.push((key, 0.0, None, None));
                            minion_counts.len() - 1
                        }
                    };
                    let c = &mut minion_counts[pos];
                    c.1 = m_max(count, c.1);
                    if !vaal {
                        c.2 = Some(m_max(count, c.2.unwrap_or(0.0)));
                    }
                    if permanent {
                        c.3 = Some(m_max(count, c.3.unwrap_or(0.0)));
                    }
                }
            }
            if self.has_type(i, "CreatesMinion") && !self.has_type(i, "MinionsAreUndamagable") {
                self.c.add(
                    pdb,
                    Mod::new("Condition:HaveDamageableMinion", FlagT, true, None),
                );
            }
            if self.env.mode.buffs && self.has_flag(i, "warcry") {
                if name == "Rallying Cry"
                    && !self.c.flag(list, None, &["CannotShareWarcryBuffs"])?
                    && !self.c.flag(pdb, None, &["RallyingActive"])?
                {
                    let v = self
                        .c
                        .base(list, Some(&main_cfg), &["RallyingCryExertDamageBonus"])?;
                    self.c.add(
                        pdb,
                        Mod::new("RallyingExertMoreDamagePerAlly", Base, v, None),
                    );
                    self.c
                        .add(pdb, Mod::new("RallyingActive", FlagT, true, None));
                } else if name == "Seismic Cry" && !self.c.flag(pdb, None, &["SeismicActive"])? {
                    let v = self
                        .c
                        .base(list, Some(&main_cfg), &["SeismicAoEMoreMultiplier"])?;
                    self.c.add(pdb, Mod::new("SeismicMoreAoE", Base, v, None));
                    self.c
                        .add(pdb, Mod::new("SeismicActive", FlagT, true, None));
                }
            }
            if self.sd_truthy(i, "triggeredOnDeath") && !self.has_flag(i, "minion") {
                self.mods_mut(i)
                    .skill_data
                    .insert("triggered".into(), ModValue::Bool(true));
                for typ in [Inc, More] {
                    for (_, m) in
                        self.c
                            .tabulate(list, Some(typ), Some(&main_cfg), &["TriggeredDamage"])?
                    {
                        let mut d = m.clone();
                        d.name = Sym::new("Damage");
                        d.source_slot = None;
                        d.unreached_tags.clear();
                        self.c.add(list, d);
                    }
                }
                self.mods_mut(i)
                    .skill_data
                    .insert("triggerTime".into(), ModValue::Number(60.0 * 1000.0));
            }
        }

        let mut summoned = 0.0;
        for (_, total, non_vaal, permanent) in &minion_counts {
            summoned += total;
            let combat = || Tag::condition("Combat");
            self.c.add(
                pdb,
                src("Multiplier:SummonedMinion", Base, *total, "Config").tag(combat()),
            );
            if let Some(n) = non_vaal {
                self.c.add(
                    pdb,
                    src("Multiplier:NonVaalSummonedMinion", Base, *n, "Config").tag(combat()),
                );
            }
            if let Some(n) = permanent {
                self.c.add(
                    pdb,
                    src("Multiplier:PermanentMinion", Base, *n, "Config").tag(combat()),
                );
            }
        }
        // Companionship only works while exactly one minion is summoned
        let over = self
            .config_num("multiplierSummonedMinion")
            .filter(|&n| n != 0.0);
        self.c
            .set_cond(pdb, "OnlyMinion", over.unwrap_or(summoned) == 1.0);
        Ok(())
    }
}

/// `modLib.formatModParams(mod)` as a key: equal for mods with equal params
pub(crate) fn mod_params_key(m: &Mod) -> String {
    format!(
        "{}|{}|{}|{}|{:?}",
        m.name,
        m.typ.name(),
        m.flags,
        m.keyword_flags,
        m.tags
    )
}

pub(crate) fn scaled(mods: &[Mod], scale: f64, p: &dyn crate::mods::list::Precision) -> Vec<Mod> {
    if scale == 1.0 {
        return mods.to_vec();
    }
    mods.iter()
        .map(|m| crate::mods::list::scale_mod(m, scale, p))
        .collect()
}

pub(crate) fn field_num(f: &crate::item::Fields, k: &str) -> Option<f64> {
    f.get(k).and_then(ModValue::as_number)
}

impl<'a> Perform<'a> {
    /// `getFlaskInstantRecovery(item)`
    fn flask_instant_recovery(&self, item: &Item) -> Result<f64> {
        let pdb = self.pdb();
        let fd = item.flask_data.as_ref().ok_or("a flask without data")?;
        let mut perc = field_num(fd, "instantPerc").unwrap_or(0.0);
        if self.c.flag(pdb, None, &["Condition:LowLife"])? {
            perc += field_num(fd, "instantLowLifePerc").unwrap_or(0.0);
        }
        let flask = item.base.as_ref().and_then(|b| b.flask.as_ref());
        if flask.is_some_and(|f| f.life.is_some()) {
            perc += self.c.base(pdb, None, &["LifeFlaskInstantRecovery"])?;
        }
        if flask.is_some_and(|f| f.mana.is_some()) {
            perc += self.c.base(pdb, None, &["ManaFlaskInstantRecovery"])?;
        }
        Ok(m_min(100.0, perc))
    }

    /// `calcFlaskRecovery(type, item)`
    fn flask_recovery(&self, typ: &str, item: &Item, effect_inc: f64) -> Result<Vec<Mod>> {
        let pdb = self.pdb();
        let fd = item.flask_data.as_ref().ok_or("a flask without data")?;
        let l = typ.to_lowercase();
        let mut out = Vec::new();
        if !fd
            .get(&format!("{l}EffectNotRemoved"))
            .is_some_and(ModValue::truthy)
            && !self
                .c
                .flag(pdb, None, &[&format!("{typ}FlaskEffectNotRemoved")])?
        {
            return Ok(out);
        }
        let flask_dur_inc = self.c.inc(pdb, None, &["FlaskDuration"])?;
        let flask_total_rate_inc = self.c.inc(pdb, None, &["FlaskRecoveryRate"])?;
        let base = field_num(fd, &format!("{l}Base")).unwrap_or(f64::NAN);
        let dur = field_num(fd, "duration").unwrap_or(f64::NAN);
        let inst = self.flask_instant_recovery(item)?;
        let rec_inc = self.c.inc(pdb, None, &[&format!("Flask{typ}Recovery")])?;
        let rec_more = self.c.more(pdb, None, &[&format!("Flask{typ}Recovery")])?;
        let rate_inc = self
            .c
            .inc(pdb, None, &[&format!("Flask{typ}RecoveryRate")])?;
        let mut total = base
            * (1.0 - inst / 100.0)
            * (1.0 + rec_inc / 100.0)
            * rec_more
            * (1.0 + flask_dur_inc / 100.0);
        let flask_dur = dur * (1.0 + flask_dur_inc / 100.0)
            / (1.0 + flask_total_rate_inc / 100.0)
            / (1.0 + rate_inc / 100.0);
        let low_life_more = self.c.more(pdb, None, &["FlaskLifeRecoveryLowLife"])?;
        if low_life_more > 1.0 {
            total *= if l == "life" {
                (low_life_more - 1.0) / (1.0 + effect_inc / 100.0) + 1.0
            } else {
                1.0
            };
        }
        let name = item.name.as_str();
        out.push(src(
            &format!("{typ}Recovery"),
            Base,
            total / flask_dur,
            name,
        ));
        if self
            .c
            .flag(pdb, None, &[&format!("{typ}FlaskAppliesToEnergyShield")])?
        {
            out.push(src("EnergyShieldRecovery", Base, total / flask_dur, name));
        }
        if self
            .c
            .flag(pdb, None, &[&format!("{typ}FlaskAppliesToLife")])?
        {
            out.push(src("LifeRecovery", Base, total / flask_dur, name));
        }
        Ok(out)
    }

    /// `mergeFlasks(flasks, onlyRecovery, checkNonRecoveryFlasksForMinions)`
    /// for the player
    fn merge_flasks(&mut self, flasks: &[i64], only_recovery: bool, _minions: bool) -> Result<()> {
        let pdb = self.pdb();
        let player = actor_cfg("player");
        let effect_inc = self.c.inc(pdb, Some(&player), &["FlaskEffect"])?;
        let effect_inc_magic = self
            .c
            .inc(pdb, Some(&player), &["MagicUtilityFlaskEffect"])?;
        let effect_inc_no_adjacent =
            self.c
                .inc(pdb, Some(&player), &["MagicFlaskNoAdjacentEffect"])?;
        let mut buffs: BuffMap = Vec::new();
        let mut buffs_non_utility: BuffMap = Vec::new();
        let mut conditions: Vec<String> = Vec::new();
        let mut conditions_non_utility: Vec<String> = Vec::new();
        let occupied: HashSet<i64> = self
            .env
            .setup
            .items
            .flask_slots
            .values()
            .map(|&n| n as i64)
            .collect();
        let precision = self.c.precision;
        for &id in flasks {
            let Some(item) = self.input.tab.item(id as f64) else {
                continue;
            };
            let fd = item.flask_data.as_ref().ok_or("a flask without data")?;
            let flask = item
                .base
                .as_ref()
                .and_then(|b| b.flask.clone())
                .ok_or("a flask without a base")?;
            let base_name = item.base_name.clone().unwrap_or_default();
            let instant = self.flask_instant_recovery(item)?;
            let cond = |c: &mut Vec<String>, k: String| {
                if !c.contains(&k) {
                    c.push(k);
                }
            };
            if instant < 100.0 {
                cond(&mut conditions, "UsingFlask".into());
                cond(
                    &mut conditions,
                    format!("Using{}", no_whitespace(&base_name)),
                );
                if flask.life.is_some() || flask.mana.is_some() {
                    cond(&mut conditions_non_utility, "UsingFlask".into());
                    cond(
                        &mut conditions_non_utility,
                        format!("Using{}", no_whitespace(&base_name)),
                    );
                }
                if flask.life.is_some()
                    && !self.c.flag(pdb, None, &["CannotRecoverLifeOutsideLeech"])?
                {
                    cond(&mut conditions, "UsingLifeFlask".into());
                    cond(&mut conditions_non_utility, "UsingLifeFlask".into());
                }
                if flask.mana.is_some() {
                    cond(&mut conditions, "UsingManaFlask".into());
                    cond(&mut conditions_non_utility, "UsingManaFlask".into());
                }
            }
            if base_name == "Iron Flask" {
                let on_break = self
                    .c
                    .base(pdb, None, &["IronFlaskChargesGeneratedOnWardBreak"])?;
                if on_break > 0.0 {
                    let gain = field_num(fd, "gainMod").unwrap_or(f64::NAN)
                        * (1.0 + self.c.inc(pdb, None, &["FlaskChargesGained"])? / 100.0);
                    let used = field_num(fd, "chargesUsed").unwrap_or(f64::NAN)
                        * (1.0 + self.c.inc(pdb, None, &["FlaskChargesUsed"])? / 100.0);
                    if on_break * gain > used {
                        cond(&mut conditions, "UnbrokenWard".into());
                    }
                }
            }
            // calcFlaskMods
            let mut flask_effect_inc = effect_inc + field_num(fd, "effectInc").unwrap_or(0.0);
            if item.rarity == "MAGIC" && !(flask.life.is_some() || flask.mana.is_some()) {
                flask_effect_inc += effect_inc_magic;
            }
            if item.rarity == "MAGIC"
                && effect_inc_no_adjacent != 0.0
                && let Some(&n) = self.env.setup.items.flask_slots.get(&id)
            {
                let n = n as i64;
                if !occupied.contains(&(n - 1)) && !occupied.contains(&(n + 1)) {
                    flask_effect_inc += effect_inc_no_adjacent;
                }
            }
            let effect_mod = 1.0 + flask_effect_inc / 100.0;
            let calc_flask_mods = |base_name: &str,
                                   buff_mods: &[Mod],
                                   mods: &[Mod],
                                   buffs: &mut BuffMap,
                                   non_utility: &mut BuffMap|
             -> Result<()> {
                if !buff_mods.is_empty() {
                    let src_list = scaled(buff_mods, effect_mod, precision);
                    merge_buff(&src_list, buffs, base_name)?;
                }
                if !mods.is_empty() {
                    let src_list = scaled(mods, effect_mod, precision);
                    let key = if item.rarity == "UNIQUE" || item.rarity == "RELIC" {
                        item.title.clone().unwrap_or_default()
                    } else {
                        mods.iter().map(|m| mod_params_key(m) + "&").collect()
                    };
                    if !only_recovery {
                        merge_buff(&src_list, buffs, &key)?;
                        merge_buff(&src_list, non_utility, &key)?;
                    }
                }
                Ok(())
            };
            if only_recovery {
                if flask.life.is_some()
                    && !self.c.flag(pdb, None, &["CannotRecoverLifeOutsideLeech"])?
                {
                    let r = self.flask_recovery("Life", item, effect_inc)?;
                    calc_flask_mods("LifeFlask", &r, &[], &mut buffs, &mut buffs_non_utility)?;
                }
                if flask.mana.is_some() {
                    let r = self.flask_recovery("Mana", item, effect_inc)?;
                    calc_flask_mods("ManaFlask", &r, &[], &mut buffs, &mut buffs_non_utility)?;
                }
            } else {
                let mods = item.mod_list.clone().unwrap_or_default();
                calc_flask_mods(
                    &base_name,
                    &item.buff_mod_list,
                    &mods,
                    &mut buffs,
                    &mut buffs_non_utility,
                )?;
            }
        }
        if self
            .c
            .flag(pdb, None, &["UtilityFlasksDoNotApplyToPlayer"])?
        {
            for c in conditions_non_utility {
                self.c.set_cond(pdb, &c, true);
            }
            for (_, l, _) in buffs_non_utility {
                self.c.add_list(pdb, &l);
            }
        } else if !self.c.flag(pdb, None, &["FlasksDoNotApplyToPlayer"])? {
            for c in conditions {
                self.c.set_cond(pdb, &c, true);
            }
            for (_, l, _) in buffs {
                self.c.add_list(pdb, &l);
            }
        }
        Ok(())
    }

    /// `mergeTinctures(env.tinctures)`
    fn merge_tinctures(&mut self) -> Result<()> {
        let pdb = self.pdb();
        let player = actor_cfg("player");
        let effect_inc = self.c.inc(pdb, Some(&player), &["TinctureEffect"])?;
        let effect_inc_magic = self.c.inc(pdb, Some(&player), &["MagicTinctureEffect"])?;
        let mut limit = self.c.base(pdb, None, &["TinctureLimit"])?;
        if self.input.breakdown {
            self.c.set_out(PLAYER, "TinctureEffect", effect_inc);
            self.c.set_out(PLAYER, "TinctureLimit", limit);
        }
        let not_burn = m_min(
            self.c.base(pdb, None, &["TincturesNotInflictManaBurn"])?,
            100.0,
        );
        let can_burn = self
            .c
            .flag(pdb, None, &["Condition:WeepingWoundsInsteadOfManaBurn"])?
            || (not_burn < 100.0 && self.c.out(PLAYER, "ManaUnreserved").unwrap_or(0.0) > 0.0);
        if !can_burn {
            return Ok(());
        }
        let mut buffs: BuffMap = Vec::new();
        let mut conditions: Vec<String> = Vec::new();
        let precision = self.c.precision;
        let tinctures: Vec<i64> = self.env.setup.items.tinctures.clone();
        for id in tinctures {
            if limit <= 0.0 {
                break;
            }
            limit -= 1.0;
            let Some(item) = self.input.tab.item(id as f64) else {
                continue;
            };
            let base_name = item.base_name.clone().unwrap_or_default();
            for c in [
                "UsingTincture".to_string(),
                format!("Using{}", no_whitespace(&base_name)),
            ] {
                if !conditions.contains(&c) {
                    conditions.push(c);
                }
            }
            let td = item
                .tincture_data
                .as_ref()
                .ok_or("a tincture without data")?;
            let mut inc = effect_inc + field_num(td, "effectInc").unwrap_or(0.0);
            if item.rarity == "MAGIC" {
                inc += effect_inc_magic;
            }
            let effect_mod =
                ((1.0 + inc / 100.0) * (1.0 + item.quality.unwrap_or(0.0) / 100.0) * 100.0).floor()
                    / 100.0;
            if !item.buff_mod_list.is_empty() {
                let l = scaled(&item.buff_mod_list, effect_mod, precision);
                merge_buff(&l, &mut buffs, &base_name)?;
            }
            let mods = item.mod_list.clone().unwrap_or_default();
            if !mods.is_empty() {
                let l = scaled(&mods, effect_mod, precision);
                let key = if item.rarity == "UNIQUE" || item.rarity == "RELIC" {
                    item.title.clone().unwrap_or_default()
                } else {
                    mods.iter().map(|m| mod_params_key(m) + "&").collect()
                };
                merge_buff(&l, &mut buffs, &key)?;
            }
        }
        for c in conditions {
            self.c.set_cond(pdb, &c, true);
        }
        for (_, l, _) in &buffs {
            self.c.add_list(pdb, l);
        }
        if self.c.flag(pdb, None, &["TinctureRangedWeapons"])? {
            let melee = self.mf("WeaponMelee");
            let ranged = self.mf("WeaponRanged");
            for (_, l, _) in &buffs {
                for b in l {
                    if b.flags & melee == melee {
                        let mut m = b.clone();
                        m.flags = (m.flags & !melee) | ranged;
                        self.c.add(pdb, m);
                    }
                }
            }
        }
        Ok(())
    }
}

impl<'a> Perform<'a> {
    /// `doActorLifeMana(actor)`
    pub fn life_mana(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let low = self.c.base(db, None, &["LowLifePercentage"])?;
        let low_pool = self.misc_data("LowPoolThreshold");
        self.c.set_out(
            actor,
            "LowLifePercentage",
            100.0 * if low > 0.0 { low } else { low_pool },
        );
        let full = self.c.base(db, None, &["FullLifePercentage"])?;
        self.c.set_out(
            actor,
            "FullLifePercentage",
            100.0 * if full > 0.0 { full } else { 1.0 },
        );
        // `Flag` gives nil when there is no flag
        let ci = self.c.flag(db, None, &["ChaosInoculation"])?;
        if ci {
            self.c
                .set_out_val(actor, "ChaosInoculation", OutVal::Bool(true));
        } else {
            self.c.clear_out(actor, "ChaosInoculation");
        }
        let life = if ci {
            self.c.set_cond(db, "FullLife", true);
            1.0
        } else {
            let base = self.c.base(db, None, &["Life"])?;
            let inc = self.c.inc(db, None, &["Life"])?;
            let more = self.c.more(db, None, &["Life"])?;
            let over = self.c.override_num(db, None, &["Life"])?;
            let conv = self.c.base(db, None, &["LifeConvertToEnergyShield"])?;
            match over {
                Some(o) => o,
                None => m_max(
                    round(
                        base * (1.0 + inc / 100.0) * more * (1.0 - conv / 100.0),
                        0.0,
                    ),
                    1.0,
                ),
            }
        };
        self.c.set_out(actor, "Life", life);
        let conv = self.c.base(db, None, &["ManaConvertToArmour"])?;
        let mana = round(self.c.val(db, "Mana", None)? * (1.0 - conv / 100.0), 0.0);
        self.c.set_out(actor, "Mana", mana);
        self.c.set_out(
            actor,
            "LowestOfMaximumLifeAndMaximumMana",
            m_min(life, mana),
        );
        Ok(())
    }

    /// The attribute conditions of `doActorAttribsConditions`
    fn attribute_conditions(&mut self, actor: usize) {
        let db = self.c.db(actor);
        let o = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        let (s, d, i) = (o(self, "Str"), o(self, "Dex"), o(self, "Int"));
        let mut stats = [s, d, i];
        stats.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        self.c.set_out(actor, "LowestAttribute", stats[0]);
        let conds = [
            ("TwoHighestAttributesEqual", stats[1] == stats[2]),
            ("DexHigherThanInt", d > i),
            ("StrHigherThanInt", s > i),
            ("IntHigherThanDex", i > d),
            ("StrHigherThanDex", s > d),
            ("IntHigherThanStr", i > s),
            ("DexHigherThanStr", d > s),
            ("StrHighestAttribute", s >= d && s >= i),
            ("IntHighestAttribute", i >= s && i >= d),
            ("DexHighestAttribute", d >= s && d >= i),
            ("IntSingleHighestAttribute", i > s && i > d),
            ("DexSingleHighestAttribute", d > s && d > i),
        ];
        for (k, v) in conds {
            self.c.set_cond(db, k, v);
        }
    }

    /// `doActorAttribsConditions(env, actor)`
    fn attribs_conditions(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let is_player = actor == PLAYER;
        let w2 = self.item(actor, "Weapon 2");
        if w2.is_some_and(|i| i.typ.as_deref() == Some("Shield"))
            || (is_player && self.env.setup.items.aegis_mods.is_some())
        {
            self.c.set_cond(db, "UsingShield", true);
        } else if w2.is_none() {
            self.c.set_cond(db, "OffHandIsEmpty", true);
        }
        let w1_type = self.weapon_str(actor, 0, "type");
        if w1_type.as_deref() == Some("None") {
            self.c.set_cond(db, "Unarmed", true);
            if w2.is_none() && self.item(actor, "Gloves").is_none() {
                self.c.set_cond(db, "Unencumbered", true);
            }
        } else {
            self.weapon_conditions(actor, 0)?;
        }
        for slot in ["Helmet", "Body Armour", "Gloves", "Boots"] {
            if self.item(actor, slot).is_some() {
                self.c.set_cond(db, &format!("Using{slot}"), true);
            }
        }
        for slot in ["Graft 1", "Graft 2"] {
            if let Some(base) = self.item(actor, slot).and_then(|i| i.base_name.as_deref()) {
                let word = crate::pat::capture(base, "%s(%S+)").ok_or("a graft base")?;
                self.c.set_cond(db, &format!("Using{word}"), true);
            }
        }
        let w2_type = self.weapon_str(actor, 1, "type");
        if w2_type.is_some() {
            self.weapon_conditions(actor, 1)?;
        }
        if let (Some(t1), Some(t2)) = (&w1_type, &w2_type) {
            self.c.set_cond(db, "DualWielding", true);
            let all = |p: &Self, n: usize| {
                p.weapon(actor, n)
                    .and_then(|w| w.get("countsAsAll1H"))
                    .is_some_and(ModValue::truthy)
            };
            if (&**t1 == "Claw" || all(self, 0)) && (&**t2 == "Claw" || all(self, 1)) {
                self.c.set_cond(db, "DualWieldingClaws", true);
            }
            if (&**t1 == "Dagger" || all(self, 0)) && (&**t2 == "Dagger" || all(self, 1)) {
                self.c.set_cond(db, "DualWieldingDaggers", true);
            }
            let weapon_type = |p: &Self, n: usize, t: &str| -> String {
                let sub = p.weapon_str(actor, n, "subType");
                if t == "One Handed Sword" && sub.as_deref() == Some("Thrusting") {
                    return "One Handed Sword".into();
                }
                p.data()
                    .weapon_type_info
                    .get(t)
                    .and_then(|i| i.label.clone())
                    .or(sub.map(|s| s.to_string()))
                    .unwrap_or_else(|| t.to_string())
            };
            if weapon_type(self, 0, t1) != weapon_type(self, 1, t2) {
                let info = &self.data().weapon_type_info;
                let one = |t: &str| info.get(t).is_some_and(|i| i.one_hand);
                if one(t1) && one(t2) {
                    self.c.set_cond(db, "WieldingDifferentWeaponTypes", true);
                }
            }
        }
        let mode = self.env.mode;
        let main = self.main();
        let main_cfg = self.cfg(main);
        if mode.combat {
            let f = |p: &Self, k: &str| p.has_flag(main, k);
            if !self.sd_truthy(main, "triggered")
                && !f(self, "trap")
                && !f(self, "mine")
                && !f(self, "totem")
            {
                if f(self, "attack") {
                    self.c.set_cond(db, "AttackedRecently", true);
                } else if f(self, "spell") {
                    self.c.set_cond(db, "CastSpellRecently", true);
                }
                if self.has_type(main, "Movement") {
                    self.c.set_cond(db, "UsedMovementSkillRecently", true);
                }
                if f(self, "minion") && !f(self, "permanentMinion") {
                    self.c.set_cond(db, "UsedMinionSkillRecently", true);
                }
                if self.has_type(main, "Vaal") {
                    self.c.set_cond(db, "UsedVaalSkillRecently", true);
                }
                if self.has_type(main, "Channel") {
                    self.c.set_cond(db, "Channelling", true);
                }
            }
            if f(self, "hit") && !f(self, "trap") && !f(self, "mine") && !f(self, "totem") {
                self.c.set_cond(db, "HitRecently", true);
                if f(self, "spell") {
                    self.c.set_cond(db, "HitSpellRecently", true);
                }
            }
            if f(self, "totem") {
                self.c.set_cond(db, "HaveTotem", true);
                self.c.set_cond(db, "SummonedTotemRecently", true);
                if f(self, "hit") {
                    self.c.set_cond(db, "TotemsHitRecently", true);
                    if f(self, "spell") {
                        self.c.set_cond(db, "TotemsSpellHitRecently", true);
                    }
                }
            }
            if f(self, "mine") {
                self.c.set_cond(db, "DetonatedMinesRecently", true);
            }
            if f(self, "trap") {
                self.c.set_cond(db, "TriggeredTrapsRecently", true);
            }
            let pdb = self.pdb();
            let alt = |p: &Self| -> Result<bool> {
                Ok(p.c.flag(db, None, &["CritAlwaysAltAilments"])?
                    && !p.c.flag(db, Some(&main_cfg), &["NeverCrit"])?)
            };
            let _ = pdb;
            if self.c.base(db, None, &["EnemyScorchChance"])? > 0.0
                || alt(self)?
                || self.c.flag(db, None, &["IgniteCanScorch"])?
            {
                self.c.set_cond(db, "CanInflictScorch", true);
            }
            if self.c.base(db, None, &["EnemyBrittleChance"])? > 0.0 || alt(self)? {
                self.c.set_cond(db, "CanInflictBrittle", true);
            }
            if self.c.base(db, None, &["EnemySapChance"])? > 0.0 || alt(self)? {
                self.c.set_cond(db, "CanInflictSap", true);
            }
            self.shrines(db)?;
        }
        if mode.effective {
            let main_list = self.lists[main].list;
            for e in ["Fire", "Cold", "Lightning"] {
                let k = format!("{e}ExposureChance");
                if self.c.base(main_list, Some(&main_cfg), &[&k])? > 0.0
                    || self.c.base(db, None, &[&k])? > 0.0
                {
                    self.c.set_cond(db, &format!("CanApply{e}Exposure"), true);
                }
            }
        }

        // Attributes
        if self.c.flag(db, None, &["Omniscience"])? {
            self.omniscience(actor)?;
        } else {
            for _ in 0..2 {
                for stat in ["Str", "Dex", "Int"] {
                    let v = m_max(round(self.c.val(db, stat, None)?, 0.0), 0.0);
                    self.c.set_out(actor, stat, v);
                }
                self.attribute_conditions(actor);
            }
        }
        let o = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        let total = o(self, "Str") + o(self, "Dex") + o(self, "Int");
        self.c.set_out(actor, "TotalAttr", total);
        let devotion = self.c.base(db, None, &["Devotion"])?;
        self.c.set_out(actor, "Devotion", devotion);

        if !self.c.flag(db, None, &["NoAttributeBonuses"])? {
            if !self.c.flag(db, None, &["NoStrengthAttributeBonuses"])? {
                let str_ = o(self, "Str");
                if !self.c.flag(db, None, &["NoStrBonusToLife"])? {
                    self.c
                        .add(db, src("Life", Base, (str_ / 2.0).floor(), "Strength"));
                }
                let ratio = self.c.base(db, None, &["StrDmgBonusRatioOverride"])?;
                let extra = self.c.base(db, None, &["DexIntToMeleeBonus"])?;
                let bonus = if ratio > 0.0 {
                    ((str_ + extra) * ratio).floor()
                } else {
                    ((str_ + extra) / 5.0).floor()
                };
                if is_player {
                    self.str_dmg_bonus = Some(bonus);
                }
                let melee = self.mf("Melee");
                self.c.add(
                    db,
                    src("PhysicalDamage", Inc, bonus, "Strength").flags(melee, 0),
                );
            }
            if !self.c.flag(db, None, &["NoDexterityAttributeBonuses"])? {
                let dex = o(self, "Dex");
                let per = match self.c.override_num(db, None, &["DexAccBonusOverride"])? {
                    Some(v) => v,
                    None => self.misc_data("AccuracyPerDexBase"),
                };
                self.c
                    .add(db, src("Accuracy", Base, dex * per, "Dexterity"));
                if !self.c.flag(db, None, &["NoDexBonusToEvasion"])? {
                    self.c
                        .add(db, src("Evasion", Inc, (dex / 5.0).floor(), "Dexterity"));
                }
            }
            if !self.c.flag(db, None, &["NoIntelligenceAttributeBonuses"])? {
                let int = o(self, "Int");
                if !self.c.flag(db, None, &["NoIntBonusToMana"])? {
                    self.c
                        .add(db, src("Mana", Base, (int / 2.0).floor(), "Intelligence"));
                }
                if !self.c.flag(db, None, &["NoIntBonusToES"])? {
                    self.c.add(
                        db,
                        src("EnergyShield", Inc, (int / 10.0).floor(), "Intelligence"),
                    );
                }
            }
        }
        self.life_mana(actor)
    }

    /// The weapon conditions of `doActorAttribsConditions` for weapon `n`
    fn weapon_conditions(&mut self, actor: usize, n: usize) -> Result<()> {
        let db = self.c.db(actor);
        let typ = self.weapon_str(actor, n, "type").unwrap_or_default();
        let info = self
            .data()
            .weapon_type_info
            .get(&*typ)
            .cloned()
            .ok_or_else(|| format!("no weapon type info for {typ}"))?;
        self.c.set_cond(db, &format!("Using{}", info.flag), true);
        let all = self
            .weapon(actor, n)
            .and_then(|w| w.get("countsAsAll1H"))
            .is_some_and(ModValue::truthy);
        if all {
            let varunastra = self
                .weapon_str(actor, n, "name")
                .is_some_and(|s| s.contains("Varunastra"));
            for t in ["Axe", "Sword", "Dagger", "Mace", "Claw"] {
                let using = self.c.cond(db, &format!("Using{t}"));
                let added = if t == "Sword" {
                    varunastra || !using
                } else {
                    !using
                };
                if let Some(w) = self.c.actors.actors[actor].weapon_data[n].as_mut() {
                    w.insert(format!("AddedUsing{t}"), ModValue::Bool(added));
                }
                self.c.set_cond(db, &format!("Using{t}"), true);
            }
            self.c.set_cond(db, "WieldingDifferentWeaponTypes", true);
        }
        if info.melee {
            self.c.set_cond(db, "UsingMeleeWeapon", true);
        }
        if info.one_hand {
            self.c.set_cond(db, "UsingOneHandedWeapon", true);
        } else {
            self.c.set_cond(db, "UsingTwoHandedWeapon", true);
        }
        Ok(())
    }

    /// The shrine buffs of `doActorAttribsConditions`
    fn shrines(&mut self, db: StoreId) -> Result<()> {
        let effect = 1.0
            + self
                .c
                .inc(db, None, &["BuffEffectOnSelf", "ShrineBuffEffect"])?
                / 100.0;
        let fl = |x: f64| (x * effect).floor();
        let attack = self.mf("Attack");
        let cast = self.mf("Cast");
        type Shrine = (&'static str, Vec<(&'static str, ModType, f64, u32)>);
        let lesser_accel = self.c.flag(db, None, &["LesserAccelerationShrine"])?
            && !self.c.flag(db, None, &["AccelerationShrine"])?;
        let shrines: Vec<Shrine> = vec![
            (
                "AccelerationShrine",
                vec![
                    ("ActionSpeed", Inc, fl(50.0), 0),
                    ("ProjectileSpeed", Inc, fl(80.0), 0),
                ],
            ),
            (
                "BrutalShrine",
                vec![
                    ("Damage", Inc, fl(50.0), 0),
                    ("EnemyStunDuration", Inc, fl(30.0), 0),
                    ("EnemyKnockbackChance", Inc, 100.0, 0),
                ],
            ),
            ("DiamondShrine", vec![("CritChance", Override, 100.0, 0)]),
            ("DivineShrine", vec![("DamageTaken", More, -100.0, 0)]),
            (
                "EchoingShrine",
                vec![
                    ("Speed", More, fl(100.0), attack),
                    ("Speed", More, fl(100.0), cast),
                    ("RepeatCount", Base, fl(1.0), 0),
                ],
            ),
            (
                "GloomShrine",
                vec![("NonChaosDamageGainAsChaos", Base, fl(10.0), 0)],
            ),
            (
                "GreaterFreezingShrine",
                vec![("PhysicalDamageGainAsCold", Base, fl(30.0), 0)],
            ),
            (
                "GreaterShockingShrine",
                vec![("PhysicalDamageGainAsLightning", Base, fl(30.0), 0)],
            ),
            (
                "GreaterSkeletalShrine",
                vec![("PhysicalDamageGainAsChaos", Base, fl(30.0), 0)],
            ),
            (
                "ImpenetrableShrine",
                vec![
                    ("Armour", Inc, fl(100.0), 0),
                    ("Evasion", Inc, fl(100.0), 0),
                    ("EnergyShield", Inc, fl(100.0), 0),
                ],
            ),
            (
                "MassiveShrine",
                vec![
                    ("Life", Inc, fl(40.0), 0),
                    ("AreaOfEffect", Inc, fl(40.0), 0),
                ],
            ),
            (
                "ReplenishingShrine",
                vec![
                    ("ManaRegen", Inc, 200.0 * effect, 0),
                    ("LifeRegenPercent", Base, 6.7 * effect, 0),
                ],
            ),
            (
                "ResistanceShrine",
                vec![
                    ("ElementalResist", Base, fl(50.0), 0),
                    ("ElementalResistMax", Base, fl(10.0), 0),
                    ("ChaosResistMax", Base, fl(10.0), 0),
                ],
            ),
            (
                "ResonatingShrine",
                vec![
                    ("PowerChargesMax", Base, fl(1.0), 0),
                    ("FrenzyChargesMax", Base, fl(1.0), 0),
                    ("EnduranceChargesMax", Base, fl(1.0), 0),
                ],
            ),
            (
                "LesserAccelerationShrine",
                vec![
                    ("ActionSpeed", Inc, fl(10.0), 0),
                    ("ProjectileSpeed", Inc, fl(30.0), 0),
                ],
            ),
            (
                "LesserBrutalShrine",
                vec![
                    ("Damage", Inc, fl(20.0), 0),
                    ("EnemyStunDuration", Inc, fl(20.0), 0),
                    ("EnemyKnockbackChance", Inc, 100.0, 0),
                ],
            ),
            (
                "LesserImpenetrableShrine",
                vec![
                    ("Armour", Inc, fl(50.0), 0),
                    ("Evasion", Inc, fl(50.0), 0),
                    ("EnergyShield", Inc, fl(50.0), 0),
                ],
            ),
            (
                "LesserMassiveShrine",
                vec![
                    ("Life", Inc, fl(20.0), 0),
                    ("AreaOfEffect", Inc, fl(20.0), 0),
                ],
            ),
            (
                "LesserReplenishingShrine",
                vec![
                    ("ManaRegen", Inc, 100.0 * effect, 0),
                    ("LifeRegenPercent", Base, 3.3 * effect, 0),
                ],
            ),
            (
                "LesserResistanceShrine",
                vec![
                    ("ElementalResist", Base, fl(25.0), 0),
                    ("ElementalResistMax", Base, fl(2.0), 0),
                    ("ChaosResistMax", Base, fl(2.0), 0),
                ],
            ),
        ];
        for (flag, mods) in shrines {
            let on = if flag == "LesserAccelerationShrine" {
                lesser_accel
            } else {
                self.c.flag(db, None, &[flag])?
            };
            if !on {
                continue;
            }
            // "AccelerationShrine" -> "Acceleration Shrine"
            let mut source = String::new();
            for (i, ch) in flag.chars().enumerate() {
                if i > 0 && ch.is_ascii_uppercase() {
                    source.push(' ');
                }
                source.push(ch);
            }
            for (name, typ, v, flags) in mods {
                self.c.add(db, src(name, typ, v, &source).flags(flags, 0));
            }
        }
        Ok(())
    }

    /// `calculateOmniscience()`
    fn omniscience(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let base = self.input.class_stats;
        let source_red = "Reduction from Double/Triple Dipped attributes to Omniscience";
        for pass in 1..=2 {
            if pass != 1 {
                for (k, stat) in ["Str", "Dex", "Int"].into_iter().enumerate() {
                    let v = m_min(round(self.c.val(db, stat, None)?, 0.0), base[k]);
                    self.c.set_out(actor, stat, v);
                    let b = self.c.base(db, None, &[stat])? - base[k];
                    self.c.add(
                        db,
                        src("Omni", Base, b, &format!("{stat} conversion Omniscience")),
                    );
                    let i = self.c.inc(db, None, &[stat])?;
                    self.c.add(db, src("Omni", Inc, i, "Omniscience"));
                    let m = self.c.sum(db, More, None, &[stat])?;
                    self.c.add(db, src("Omni", More, m, "Omniscience"));
                }
            }
            if pass != 2 {
                for typ in [Base, Inc, More] {
                    let mut conv = [0.0; 4];
                    for (k, stat) in ["StrDex", "StrInt", "DexInt", "All"]
                        .into_iter()
                        .enumerate()
                    {
                        conv[k] = self.c.sum(db, typ, None, &[stat])?;
                    }
                    let red = conv[0] + conv[1] + conv[2] + 2.0 * conv[3];
                    self.c.add(db, src("Omni", typ, -red, source_red));
                }
            }
            for (k, stat) in ["Str", "Dex", "Int"].into_iter().enumerate() {
                self.c.set_out(actor, stat, base[k]);
            }
            let omni = m_max(round(self.c.val(db, "Omni", None)?, 0.0), 0.0);
            self.c.set_out(actor, "Omni", omni);
            self.attribute_conditions(actor);
        }
        Ok(())
    }
}

impl<'a> Perform<'a> {
    /// The skills' life and mana reservations
    fn reservations(&mut self) -> Result<()> {
        let pdb = self.pdb();
        let extra_life = self.c.base(pdb, None, &["ExtraLifeReserved"])?;
        let extra_mana = self.c.base(pdb, None, &["ExtraManaReserved"])?;
        // (base, percent) of Life and Mana
        self.reserved = [(0.0, extra_life), (0.0, 0.0)];
        self.uncancellable = [extra_life, extra_mana];
        for i in 0..self.lists.len() {
            if !((self.has_type(i, "HasReservation")
                || self.sd_truthy(i, "triggeredByAutoexertion"))
                && !self.has_type(i, "ReservationBecomesCost"))
            {
                continue;
            }
            let list = self.lists[i].list;
            let cfg = self.cfg(i);
            let mult = crate::calc::floor(
                self.c.more(list, Some(&cfg), &["SupportManaMultiplier"])?,
                4.0,
            );
            let skill = self.skill(i);
            let level = skill
                .active_effect
                .level
                .and_then(|l| skill.active_effect.granted_effect.level(l))
                .cloned()
                .unwrap_or_default();
            let has_cost = !level.cost.is_empty();
            let cost = |k: &str| level.cost.iter().find(|(n, _)| n == k).map(|(_, v)| *v);
            let sd = |p: &Self, k: &str| p.sd_num(i, k);
            let mut mana_flat = sd(self, "manaReservationFlat")
                .or(level.field("manaReservationFlat"))
                .unwrap_or(0.0);
            if self
                .c
                .flag(list, Some(&cfg), &["ManaCostGainAsReservation"])?
                && has_cost
            {
                mana_flat =
                    self.c.base(list, Some(&cfg), &["ManaCostBase"])? + cost("Mana").unwrap_or(0.0);
            }
            let mut mana_pct = sd(self, "manaReservationPercent")
                .or(level.field("manaReservationPercent"))
                .unwrap_or(0.0);
            let mut life_flat = sd(self, "lifeReservationFlat")
                .or(level.field("lifeReservationFlat"))
                .unwrap_or(0.0);
            if self
                .c
                .flag(list, Some(&cfg), &["LifeCostGainAsReservation"])?
                && has_cost
            {
                life_flat =
                    self.c.base(list, Some(&cfg), &["LifeCostBase"])? + cost("Life").unwrap_or(0.0);
            }
            let mut life_pct = sd(self, "lifeReservationPercent")
                .or(level.field("lifeReservationPercent"))
                .unwrap_or(0.0);
            if self.c.flag(list, Some(&cfg), &["BloodMagicReserved"])? {
                life_flat += mana_flat;
                mana_flat = 0.0;
                life_pct += mana_pct;
                mana_pct = 0.0;
                let data = &mut self.mods_mut(i).skill_data;
                for k in ["Flat", "Percent"] {
                    let forced = data.remove(&format!("ManaReservation{k}Forced"));
                    match forced {
                        Some(v) => {
                            data.insert(format!("LifeReservation{k}Forced"), v);
                        }
                        None => {
                            data.remove(&format!("LifeReservation{k}Forced"));
                        }
                    }
                }
            }
            for (pool, name, base_flat, base_pct) in [
                (1, "Mana", mana_flat, mana_pct),
                (0, "Life", life_flat, life_pct),
            ] {
                let names = [format!("{name}Reserved"), "Reserved".to_string()];
                let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                let more = self.c.more(list, Some(&cfg), &names)?;
                let inc = self.c.inc(list, Some(&cfg), &names)?;
                let eff_names = [
                    format!("{name}ReservationEfficiency"),
                    "ReservationEfficiency".to_string(),
                ];
                let eff_names: Vec<&str> = eff_names.iter().map(|s| s.as_str()).collect();
                let eff = m_max(self.c.inc(list, Some(&cfg), &eff_names)?, -100.0);
                let eff_more = self.c.more(list, Some(&cfg), &eff_names)?;
                if name == "Mana" {
                    self.c.actors.actors[PLAYER].mana_efficiency = Some(eff);
                }
                let mut flat = match self.sd(i, &format!("{name}ReservationFlatForced")) {
                    Some(v) => number(v)?,
                    None => {
                        let base = (base_flat * mult).floor();
                        if more > 0.0 && inc > -100.0 && base != 0.0 {
                            m_max(
                                round(
                                    base * (100.0 + inc) / 100.0 * more
                                        / (1.0 + eff / 100.0)
                                        / eff_more,
                                    0.0,
                                ),
                                0.0,
                            )
                        } else {
                            0.0
                        }
                    }
                };
                let mut pct = match self.sd(i, &format!("{name}ReservationPercentForced")) {
                    Some(v) => number(v)?,
                    None => {
                        let base = base_pct * mult;
                        if more > 0.0 && inc > -100.0 && base != 0.0 {
                            m_max(
                                round(
                                    base * (100.0 + inc) / 100.0 * more
                                        / (1.0 + eff / 100.0)
                                        / eff_more,
                                    2.0,
                                ),
                                0.0,
                            )
                        } else {
                            0.0
                        }
                    }
                };
                if let Some(n) = self.mods(i).active_mine_count {
                    flat *= n;
                    pct *= n;
                }
                if cfg.skill_name.as_deref() == Some("Blood Sacrament")
                    && let Some(n) = self.mods(i).active_stage_count
                {
                    flat *= n + 1.0;
                    pct *= n + 1.0;
                }
                if flat != 0.0 {
                    self.mods_mut(i)
                        .skill_data
                        .insert(format!("{name}ReservedBase"), ModValue::Number(flat));
                    self.reserved[pool].0 += flat;
                }
                if pct != 0.0 {
                    let max = self.c.out(PLAYER, name).unwrap_or(f64::NAN);
                    let data = &mut self.mods_mut(i).skill_data;
                    data.insert(format!("{name}ReservedPercent"), ModValue::Number(pct));
                    data.insert(
                        format!("{name}ReservedBase"),
                        ModValue::Number(flat + (max * pct / 100.0).ceil()),
                    );
                    self.reserved[pool].1 += pct;
                }
                if self
                    .c
                    .flag(list, Some(&cfg), &["HasUncancellableReservation"])?
                {
                    self.uncancellable[pool] += pct;
                }
            }
        }
        self.update_reserving_skills();
        Ok(())
    }

    /// `doActorLifeManaReservation(actor, addAura)`
    pub fn life_mana_reservation(&mut self, actor: usize, add_aura: bool) -> Result<()> {
        let db = self.c.db(actor);
        for (pool, name) in [(0, "Life"), (1, "Mana")] {
            let max = self.c.out(actor, name).unwrap_or(f64::NAN);
            let reserved;
            if max > 0.0 {
                let low = self.c.base(db, None, &[&format!("Low{name}Percentage")])?;
                let (base, pct) = if actor == PLAYER {
                    self.reserved[pool]
                } else {
                    (0.0, 0.0)
                };
                reserved = base + (max * pct / 100.0).ceil();
                let unc = if actor == PLAYER {
                    self.uncancellable[pool]
                } else {
                    0.0
                };
                self.c
                    .set_out(actor, &format!("{name}Reserved"), m_min(reserved, max));
                self.c.set_out(
                    actor,
                    &format!("{name}ReservedPercent"),
                    m_min(reserved / max * 100.0, 100.0),
                );
                self.c
                    .set_out(actor, &format!("{name}Unreserved"), max - reserved);
                self.c.set_out(
                    actor,
                    &format!("{name}UnreservedPercent"),
                    (max - reserved) / max * 100.0,
                );
                self.c.set_out(
                    actor,
                    &format!("{name}UncancellableReservation"),
                    m_min(unc, 0.0),
                );
                self.c
                    .set_out(actor, &format!("{name}CancellableReservation"), 100.0 - unc);
                let threshold = if low > 0.0 {
                    low
                } else {
                    self.misc_data("LowPoolThreshold")
                };
                if (max - reserved) / max <= threshold {
                    self.c.set_cond(db, &format!("Low{name}"), true);
                }
            } else {
                reserved = 0.0;
            }
            if add_aura {
                for v in self
                    .c
                    .list(db, None, &[&format!("GrantReserved{name}AsAura")])?
                {
                    let ModValue::Record(r) = &v else { continue };
                    let Some(ModValue::Mod(m)) = r.get("mod") else {
                        continue;
                    };
                    let mut aura = (**m).clone();
                    aura.value =
                        ModValue::Number((number(&aura.value)? * m_min(reserved, max)).floor());
                    self.c.add(
                        db,
                        Mod::new("ExtraAura", List, record([("mod", aura.into())]), None),
                    );
                }
            }
        }
        Ok(())
    }

    /// The attribute requirements (`env.requirementsTable`)
    fn requirements_table(&self) -> Vec<(BTreeMap<&'static str, f64>, OutVal, Option<String>)> {
        let mut out = Vec::new();
        for r in &self.env.setup.items.requirements {
            let name = self
                .input
                .tab
                .item(r.item_id)
                .map(|i| i.name.clone())
                .unwrap_or_default();
            out.push((
                [("Str", r.str), ("Dex", r.dex), ("Int", r.int)]
                    .into_iter()
                    .collect(),
                OutVal::Item(name.into()),
                Some(r.slot_name.clone()),
            ));
        }
        let main = self.env.skills.main_socket_group;
        for (gi, g) in self.env.groups.iter().enumerate() {
            let enabled = self
                .env
                .skills
                .slot_enabled
                .get(gi)
                .copied()
                .unwrap_or(true);
            if !(gi + 1 == main || (g.enabled && enabled)) {
                continue;
            }
            for (k, gem) in g.gem_list.iter().enumerate() {
                if gem.gem.is_none() {
                    continue;
                }
                let over = self
                    .env
                    .skills
                    .active_skills
                    .iter()
                    .flat_map(|s| std::iter::once(&s.active_effect).chain(s.support_list.iter()))
                    .find(|e| e.src == Some((gi, k)) && e.req.is_some())
                    .and_then(|e| e.req);
                let v = |x: Option<f64>| over.or(x).unwrap_or(f64::NAN);
                out.push((
                    [
                        ("Str", v(gem.req_str)),
                        ("Dex", v(gem.req_dex)),
                        ("Int", v(gem.req_int)),
                    ]
                    .into_iter()
                    .collect(),
                    OutVal::Gem(gem.name_spec.as_str().into()),
                    None,
                ));
            }
        }
        out
    }

    /// The attribute requirements' outputs
    fn attribute_requirements(&mut self) -> Result<()> {
        let pdb = self.pdb();
        let req_mult = self
            .c
            .calc_mod(pdb, None, &["GlobalAttributeRequirements"])?;
        let omni = if self.c.flag(pdb, None, &["OmniscienceRequirements"])? {
            Some(self.c.calc_mod(pdb, None, &["OmniAttributeRequirements"])?)
        } else {
            None
        };
        let ignore = self.c.flag(pdb, None, &["IgnoreAttributeRequirements"])?;
        let attrs: &[&str] = if omni.is_some() {
            &["Omni", "Str", "Dex", "Int"]
        } else {
            &["Str", "Dex", "Int"]
        };
        let table = self.requirements_table();
        for &attr in attrs {
            let battr = if omni.is_some() { "Omni" } else { attr };
            let mut best = 0.0;
            let mut source: Option<usize> = None;
            for (k, (vals, _, _)) in table.iter().enumerate() {
                let Some(&v) = vals.get(attr) else { continue };
                if v > 0.0 {
                    let mut req = (v * req_mult).floor();
                    if let Some(o) = omni {
                        req = ((v * req_mult).floor() * (1.0 / (o - 1.0))).floor();
                    }
                    if req > best {
                        best = req;
                        source = Some(k);
                    }
                }
            }
            if ignore {
                best = 0.0;
            }
            self.c.set_out(PLAYER, &format!("Req{attr}String"), 0.0);
            if best > self.c.out(PLAYER, &format!("Req{battr}")).unwrap_or(0.0) {
                self.c.set_out(PLAYER, &format!("Req{battr}String"), best);
                self.c.set_out(PLAYER, &format!("Req{battr}"), best);
                match source {
                    Some(k) => {
                        let (vals, src, slot) = &table[k];
                        let mut t: BTreeMap<String, OutVal> = vals
                            .iter()
                            .map(|(k, v)| (k.to_string(), OutVal::Num(*v)))
                            .collect();
                        match src {
                            OutVal::Gem(_) => {
                                t.insert("source".into(), OutVal::Str("Gem".into()));
                                t.insert("sourceGem".into(), src.clone());
                            }
                            _ => {
                                t.insert("source".into(), OutVal::Str("Item".into()));
                                t.insert("sourceItem".into(), src.clone());
                                if let Some(s) = slot {
                                    t.insert("sourceSlot".into(), OutVal::Str(s.as_str().into()));
                                }
                            }
                        }
                        self.c.set_out_val(
                            PLAYER,
                            &format!("Req{battr}Item"),
                            OutVal::Table(Rc::new(t)),
                        );
                    }
                    None => self.c.clear_out(PLAYER, &format!("Req{battr}Item")),
                }
                if self.input.breakdown {
                    let have = self.c.out(PLAYER, battr).unwrap_or(0.0);
                    if best > have {
                        self.c.set_out_val(
                            PLAYER,
                            &format!("Req{battr}String"),
                            OutVal::Str(format!("^xDD0022{}", crate::pat::num(best)).into()),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// `doActorCharges(env, actor)`
    pub fn charges(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let b = |p: &Self, k: &str| p.c.base(db, None, &[k]);
        let f = |p: &Self, k: &str| p.c.flag(db, None, &[k]);
        let ov = |p: &Self, k: &str| p.c.override_num(db, None, &[k]);
        let set = |p: &mut Self, k: &str, v: f64| p.c.set_out(actor, k, v);
        let out = |p: &Self, k: &str| p.c.out(actor, k).unwrap_or(f64::NAN);
        let charge_duration = |p: &Self, k: &str| -> Result<f64> {
            Ok((b(p, "ChargeDuration")? * p.c.calc_mod(db, None, &[k, "ChargeDuration"])?).floor())
        };

        let v = m_max(b(self, "PowerChargesMin")?, 0.0);
        set(self, "PowerChargesMin", v);
        let v = match ov(self, "PowerChargesMax")? {
            Some(v) => v,
            None => m_max(b(self, "PowerChargesMax")?, 0.0),
        };
        set(self, "PowerChargesMax", v);
        let v = charge_duration(self, "PowerChargesDuration")?;
        set(self, "PowerChargesDuration", v);
        if f(self, "MaximumFrenzyChargesIsMaximumPowerCharges")? {
            let source = self.first_source(db, "MaximumFrenzyChargesIsMaximumPowerCharges");
            let m = Mod::new(
                "FrenzyChargesMax",
                Override,
                out(self, "PowerChargesMax"),
                source.as_deref(),
            );
            self.c.replace_mod(db, m);
        }
        let v = m_max(b(self, "FrenzyChargesMin")?, 0.0);
        set(self, "FrenzyChargesMin", v);
        let v = match ov(self, "FrenzyChargesMax")? {
            Some(v) => v,
            None => m_max(
                if f(self, "MaximumFrenzyChargesIsMaximumPowerCharges")? {
                    out(self, "PowerChargesMax")
                } else {
                    b(self, "FrenzyChargesMax")?
                },
                0.0,
            ),
        };
        set(self, "FrenzyChargesMax", v);
        let v = charge_duration(self, "FrenzyChargesDuration")?;
        set(self, "FrenzyChargesDuration", v);
        if f(self, "MaximumEnduranceChargesIsMaximumFrenzyCharges")? {
            let source = self.first_source(db, "MaximumEnduranceChargesIsMaximumFrenzyCharges");
            let m = Mod::new(
                "EnduranceChargesMax",
                Override,
                out(self, "FrenzyChargesMax"),
                source.as_deref(),
            );
            self.c.replace_mod(db, m);
        }
        let v = m_max(b(self, "EnduranceChargesMin")?, 0.0);
        set(self, "EnduranceChargesMin", v);
        // The party tab's members have no mods
        let v = match ov(self, "EnduranceChargesMax")? {
            Some(v) => v,
            None => m_max(
                if f(self, "MaximumEnduranceChargesIsMaximumFrenzyCharges")? {
                    out(self, "FrenzyChargesMax")
                } else {
                    b(self, "EnduranceChargesMax")?
                },
                0.0,
            ),
        };
        set(self, "EnduranceChargesMax", v);
        let v = charge_duration(self, "EnduranceChargesDuration")?;
        set(self, "EnduranceChargesDuration", v);
        for k in [
            "SiphoningChargesMax",
            "ChallengerChargesMax",
            "BlitzChargesMax",
            "InspirationChargesMax",
            "CrabBarriersMax",
        ] {
            let v = m_max(b(self, k)?, 0.0);
            set(self, k, v);
        }
        let v = m_max(
            if f(self, "MinimumEnduranceChargesEqualsMinimumBrutalCharges")? {
                if f(self, "MinimumEnduranceChargesIsMaximumEnduranceCharges")? {
                    out(self, "EnduranceChargesMax")
                } else {
                    out(self, "EnduranceChargesMin")
                }
            } else {
                0.0
            },
            0.0,
        );
        set(self, "BrutalChargesMin", v);
        let v = m_max(
            if f(self, "MaximumEnduranceChargesEqualsMaximumBrutalCharges")? {
                out(self, "EnduranceChargesMax")
            } else {
                0.0
            },
            0.0,
        );
        set(self, "BrutalChargesMax", v);
        let v = m_max(out(self, "EnduranceChargesMax"), 0.0);
        set(self, "BrineChargesMax", v);
        let v = m_max(
            if f(self, "MinimumPowerChargesEqualsMinimumAbsorptionCharges")? {
                if f(self, "MinimumPowerChargesIsMaximumPowerCharges")? {
                    out(self, "PowerChargesMax")
                } else {
                    out(self, "PowerChargesMin")
                }
            } else {
                0.0
            },
            0.0,
        );
        set(self, "AbsorptionChargesMin", v);
        let v = m_max(
            if f(self, "MaximumPowerChargesEqualsMaximumAbsorptionCharges")? {
                out(self, "PowerChargesMax")
            } else {
                0.0
            },
            0.0,
        );
        set(self, "AbsorptionChargesMax", v);
        let v = m_max(
            if f(self, "MinimumFrenzyChargesEqualsMinimumAfflictionCharges")? {
                if f(self, "MinimumFrenzyChargesIsMaximumFrenzyCharges")? {
                    out(self, "FrenzyChargesMax")
                } else {
                    out(self, "FrenzyChargesMin")
                }
            } else {
                0.0
            },
            0.0,
        );
        set(self, "AfflictionChargesMin", v);
        let v = m_max(
            if f(self, "MaximumFrenzyChargesEqualsMaximumAfflictionCharges")? {
                out(self, "FrenzyChargesMax")
            } else {
                0.0
            },
            0.0,
        );
        set(self, "AfflictionChargesMax", v);
        for k in ["BloodChargesMax", "SpiritChargesMax"] {
            let v = m_max(b(self, k)?, 0.0);
            set(self, k, v);
        }
        let v = b(self, "SpiritInfusionsMax")?
            + if f(self, "Condition:CanGainSpiritInfusion")? {
                10.0
            } else {
                0.0
            };
        set(self, "SpiritInfusionsMax", v);

        for k in [
            "PowerCharges",
            "FrenzyCharges",
            "EnduranceCharges",
            "SiphoningCharges",
            "ChallengerCharges",
            "BlitzCharges",
            "InspirationCharges",
            "GhostShrouds",
            "BrutalCharges",
            "BrineCharges",
            "AbsorptionCharges",
            "AfflictionCharges",
            "BloodCharges",
            "SpiritCharges",
            "SpiritInfusions",
        ] {
            set(self, k, 0.0);
        }
        if f(self, "MinimumFrenzyChargesIsMaximumFrenzyCharges")? {
            let v = out(self, "FrenzyChargesMax");
            set(self, "FrenzyChargesMin", v);
        }
        if f(self, "MinimumEnduranceChargesIsMaximumEnduranceCharges")? {
            let v = out(self, "EnduranceChargesMax");
            set(self, "EnduranceChargesMin", v);
        }
        if f(self, "MinimumPowerChargesIsMaximumPowerCharges")? {
            let v = out(self, "PowerChargesMax");
            set(self, "PowerChargesMin", v);
        }
        // (charge, use flag, convert flag, converted charge)
        for (charge, use_flag, convert, converted) in [
            (
                "Power",
                "UsePowerCharges",
                "PowerChargesConvertToAbsorptionCharges",
                "Absorption",
            ),
            (
                "Frenzy",
                "UseFrenzyCharges",
                "FrenzyChargesConvertToAfflictionCharges",
                "Affliction",
            ),
            (
                "Endurance",
                "UseEnduranceCharges",
                "EnduranceChargesConvertToBrutalCharges",
                "Brutal",
            ),
        ] {
            let k = format!("{charge}Charges");
            if f(self, use_flag)? {
                let v = ov(self, &k)?.unwrap_or(out(self, &format!("{k}Max")));
                set(self, &k, v);
                if charge == "Endurance" && f(self, "CanGainBrineCharges")? {
                    let v = out(self, "BrineChargesMax");
                    set(self, "BrineCharges", v);
                }
            }
            if f(self, convert)? {
                let v = m_max(
                    out(self, &k),
                    m_min(
                        out(self, &format!("{converted}ChargesMax")),
                        out(self, &format!("{converted}ChargesMin")),
                    ),
                );
                set(self, &format!("{converted}Charges"), v);
                set(self, &k, 0.0);
            } else {
                let v = m_max(
                    out(self, &k),
                    m_min(out(self, &format!("{k}Max")), out(self, &format!("{k}Min"))),
                );
                set(self, &k, v);
            }
            let v = m_max(out(self, &k) - out(self, &format!("{k}Min")), 0.0);
            set(self, &format!("Removable{k}"), v);
        }
        for (k, flag) in [
            ("SiphoningCharges", "UseSiphoningCharges"),
            ("ChallengerCharges", "UseChallengerCharges"),
            ("BlitzCharges", "UseBlitzCharges"),
        ] {
            if f(self, flag)? {
                let v = ov(self, k)?.unwrap_or(out(self, &format!("{k}Max")));
                set(self, k, v);
            }
        }
        if actor == PLAYER {
            let v = ov(self, "InspirationCharges")?.unwrap_or(out(self, "InspirationChargesMax"));
            set(self, "InspirationCharges", v);
        }
        if f(self, "UseGhostShrouds")? {
            let v = ov(self, "GhostShrouds")?.unwrap_or(3.0);
            set(self, "GhostShrouds", v);
        }
        let max = out(self, "BloodChargesMax");
        let v = m_min(ov(self, "BloodCharges")?.unwrap_or(max), max);
        set(self, "BloodCharges", v);
        let v = m_min(
            ov(self, "SpiritCharges")?.unwrap_or(0.0),
            out(self, "SpiritChargesMax"),
        );
        set(self, "SpiritCharges", v);
        let v = m_min(
            ov(self, "SpiritInfusion")?.unwrap_or(0.0),
            out(self, "SpiritInfusionsMax"),
        );
        set(self, "SpiritInfusions", v);
        let max = out(self, "CrabBarriersMax");
        let v = m_min(ov(self, "CrabBarriers")?.unwrap_or(max), max);
        set(self, "CrabBarriers", v);
        for c in ["Power", "Frenzy", "Endurance"] {
            if f(self, &format!("HaveMaximum{c}Charges"))? {
                let v = out(self, &format!("{c}ChargesMax"));
                set(self, &format!("{c}Charges"), v);
            }
        }
        let v =
            out(self, "PowerCharges") + out(self, "FrenzyCharges") + out(self, "EnduranceCharges");
        set(self, "TotalCharges", v);
        let v = out(self, "RemovableEnduranceCharges")
            + out(self, "RemovableFrenzyCharges")
            + out(self, "RemovablePowerCharges");
        set(self, "RemovableTotalCharges", v);
        for (m, o) in [
            ("PowerCharge", "PowerCharges"),
            ("PowerChargeMax", "PowerChargesMax"),
            ("RemovablePowerCharge", "RemovablePowerCharges"),
            ("FrenzyCharge", "FrenzyCharges"),
            ("RemovableFrenzyCharge", "RemovableFrenzyCharges"),
            ("EnduranceCharge", "EnduranceCharges"),
            ("RemovableEnduranceCharge", "RemovableEnduranceCharges"),
            ("TotalCharges", "TotalCharges"),
            ("RemovableTotalCharges", "RemovableTotalCharges"),
            ("SiphoningCharge", "SiphoningCharges"),
            ("ChallengerCharge", "ChallengerCharges"),
            ("BlitzCharge", "BlitzCharges"),
            ("InspirationCharge", "InspirationCharges"),
            ("GhostShroud", "GhostShrouds"),
            ("CrabBarrier", "CrabBarriers"),
            ("BrutalCharge", "BrutalCharges"),
            ("BrineCharge", "BrineCharges"),
            ("AbsorptionCharge", "AbsorptionCharges"),
            ("AfflictionCharge", "AfflictionCharges"),
            ("BloodCharge", "BloodCharges"),
            ("SpiritCharge", "SpiritCharges"),
            ("SpiritInfusion", "SpiritInfusions"),
        ] {
            let v = out(self, o);
            self.c.set_mult(db, m, v);
        }
        Ok(())
    }

    /// `modDB.mods[name][1].source`
    fn first_source(&self, db: StoreId, name: &str) -> Option<String> {
        match &self.c.store(db).mods {
            crate::mods::Mods::Db(m) => m
                .get(&Sym::new(name))
                .and_then(|l| l.first())
                .and_then(|m| m.source.as_deref().map(String::from)),
            crate::mods::Mods::List(_) => None,
        }
    }

    /// `calcs.defenceForConditionals(env, actor)`
    fn defence_for_conditionals(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        for slot in [
            "Helmet",
            "Gloves",
            "Boots",
            "Body Armour",
            "Weapon 2",
            "Weapon 3",
        ] {
            let Some(ad) = self.item(actor, slot).and_then(|i| i.armour_data.as_ref()) else {
                continue;
            };
            for (def, key) in [
                ("Ward", "Ward"),
                ("EnergyShield", "EnergyShield"),
                ("Armour", "Armour"),
                ("Evasion", "Evasion"),
            ] {
                let base = if !self
                    .c
                    .flag(db, None, &[&format!("GainNo{def}From{slot}")])?
                {
                    field_num(ad, key).unwrap_or(0.0)
                } else {
                    0.0
                };
                if base > 0.0 {
                    self.c.set_out(actor, &format!("{def}On{slot}"), base);
                }
            }
        }
        Ok(())
    }
}

impl<'a> Perform<'a> {
    /// `actor.mainSkill`: the player's main skill
    fn actor_main(&self, actor: usize) -> Option<usize> {
        (actor == PLAYER).then(|| self.main())
    }

    /// `doActorMisc(env, actor)`
    pub fn misc(&mut self, actor: usize) -> Result<()> {
        let db = self.c.db(actor);
        let enemy = self.c.actors.actors[actor].enemy.ok_or("no enemy")?;
        let edb = self.c.db(enemy);
        let main = self.actor_main(actor);
        let attack = self.mf("Attack");
        let cast = self.mf("Cast");
        let spell = self.mf("Spell");
        if self.env.mode.combat {
            let player_main_base = self.lists[self.main()].base;
            if self.c.flag(player_main_base, None, &["Cruelty"])? {
                let v = self.c.override_num(db, None, &["Cruelty"])?.unwrap_or(40.0);
                self.c.set_mult(db, "Cruelty", v);
            }
            let min_rage = self.c.base(db, None, &["MinimumRage"])?;
            if min_rage > self.c.mult(db, "Rage").unwrap_or(0.0) {
                self.c.set_mult(db, "Rage", min_rage);
            }
            let allied_fortify = if self.c.flag(db, None, &["YourFortifyEqualToParent"])? {
                let parent = self.c.actors.actors[actor].parent.ok_or("no parent")?;
                self.c
                    .out(parent, "FortificationStacks")
                    .unwrap_or(f64::NAN)
            } else {
                0.0
            };
            if self.c.base(db, None, &["MinimumFortification"])? > 0.0 || allied_fortify > 0.0 {
                self.c.set_cond(db, "Fortified", true);
            }
            if self.c.flag(db, None, &["Fortified"])?
                || self.c.base(db, None, &["Multiplier:Fortification"])? > 0.0
            {
                let skill_list = main.map_or(db, |m| self.lists[m].list);
                let skill_cfg = main.map(|m| self.cfg(m));
                let max_stacks = m_max(
                    match self.c.override_num(db, None, &["MaximumFortification"])? {
                        Some(v) => v,
                        None => self
                            .c
                            .base(db, skill_cfg.as_ref(), &["MaximumFortification"])?,
                    },
                    allied_fortify,
                );
                let min_stacks = m_min(
                    if self.c.flag(db, None, &["Condition:HaveMaxFortification"])? {
                        max_stacks
                    } else {
                        self.c.base(db, None, &["MinimumFortification"])?
                    },
                    max_stacks,
                );
                let over = self.c.override_num(db, None, &["FortificationStacks"])?;
                let stacks = m_min(
                    over.unwrap_or(if min_stacks > 0.0 {
                        min_stacks
                    } else {
                        max_stacks
                    }),
                    max_stacks,
                );
                let inc_dur = self.c.inc(skill_list, None, &["FortifyDuration"])?;
                self.c.set_out(actor, "MaximumFortification", max_stacks);
                self.c.set_out(actor, "MinimumFortification", min_stacks);
                self.c.set_out(
                    actor,
                    "RemovableFortification",
                    m_min(
                        max_stacks - min_stacks,
                        over.unwrap_or(max_stacks) - min_stacks,
                    ),
                );
                self.c.set_out(actor, "FortificationStacks", stacks);
                self.c.set_out(
                    actor,
                    "FortificationStacksOver20",
                    m_min(m_max(0.0, stacks - 20.0), max_stacks - 20.0),
                );
                let dur = self
                    .c
                    .override_num(skill_list, None, &["FortifyDuration"])?
                    .unwrap_or(self.misc_data("FortifyBaseDuration"));
                self.c
                    .set_out(actor, "FortifyDuration", dur * (1.0 + inc_dur / 100.0));
                self.c
                    .set_out_val(actor, "FortificationEffect", OutVal::Str("0".into()));
                if !self
                    .c
                    .flag(db, None, &["Condition:NoFortificationMitigation"])?
                {
                    self.c.set_out(actor, "FortificationEffect", stacks);
                    self.c.add(
                        db,
                        src("DamageTakenWhenHit", More, -stacks, "Fortification"),
                    );
                }
                if stacks >= max_stacks {
                    self.c.add(
                        db,
                        src("Condition:HaveMaximumFortification", FlagT, true, ""),
                    );
                }
                let n = self.c.mult(db, "BuffOnSelf").unwrap_or(0.0);
                self.c.set_mult(db, "BuffOnSelf", n + 1.0);
            }
            if self.c.flag(db, None, &["Onslaught"])? {
                let mut from_flask = false;
                let mut flask_inc = -100.0;
                let player = actor_cfg("player");
                for &id in &self.env.setup.items.flasks.clone() {
                    let Some(item) = self.input.tab.item(id as f64) else {
                        continue;
                    };
                    if item
                        .base_name
                        .as_deref()
                        .is_some_and(|b| b.contains("Silver Flask"))
                    {
                        from_flask = true;
                        let fd = item.flask_data.as_ref().ok_or("a flask without data")?;
                        let mut cur = field_num(fd, "effectInc").unwrap_or(f64::NAN)
                            + self.c.inc(db, Some(&player), &["FlaskEffect"])?;
                        if item.rarity == "MAGIC" {
                            cur += self
                                .c
                                .inc(db, Some(&player), &["MagicUtilityFlaskEffect"])?;
                        }
                        if flask_inc < cur / 100.0 {
                            flask_inc = cur / 100.0;
                        }
                    }
                }
                let onslaught_inc =
                    self.c
                        .inc(db, None, &["OnslaughtEffect", "BuffEffectOnSelf"])?
                        / 100.0;
                let effect = if from_flask {
                    (20.0 * (1.0 + flask_inc + onslaught_inc)).floor()
                } else {
                    (20.0 * (1.0 + onslaught_inc)).floor()
                };
                self.c
                    .add(db, src("Speed", Inc, effect, "Onslaught").flags(attack, 0));
                self.c
                    .add(db, src("Speed", Inc, effect, "Onslaught").flags(cast, 0));
                self.c
                    .add(db, src("MovementSpeed", Inc, effect, "Onslaught"));
            }
            if self.c.cond(db, "AffectedByArcaneSurge")
                || self.c.flag(db, None, &["Condition:ArcaneSurge"])?
            {
                self.c.set_cond(db, "AffectedByArcaneSurge", true);
                let mut effect = 1.0
                    + self
                        .c
                        .inc(db, None, &["ArcaneSurgeEffect", "BuffEffectOnSelf"])?
                        / 100.0;
                effect += self
                    .c
                    .inc(db, Some(&actor_cfg("player")), &["FlaskEffect"])?
                    / 100.0
                    * self.c.base(db, None, &["FlaskEffectToArcaneSurgeEffect"])?
                    / 100.0;
                let regen = self
                    .c
                    .max(db, None, &["ArcaneSurgeManaRegen"])?
                    .unwrap_or(30.0);
                self.c
                    .add(db, src("ManaRegen", Inc, regen * effect, "Arcane Surge"));
                let cs = self
                    .c
                    .max(db, None, &["ArcaneSurgeCastSpeed"])?
                    .unwrap_or(20.0)
                    * effect;
                self.c
                    .add(db, src("Speed", Inc, cs, "Arcane Surge").flags(cast, 0));
                if self
                    .c
                    .flag(db, None, &["ArcaneSurgeCastSpeedToMovementSpeed"])?
                {
                    self.c
                        .add(db, src("MovementSpeed", Inc, cs, "Arcane Surge"));
                }
                let dmg = self.c.max(db, None, &["ArcaneSurgeDamage"])?.unwrap_or(0.0);
                if dmg != 0.0 {
                    self.c.add(
                        db,
                        src("Damage", More, dmg * effect, "Arcane Surge").flags(spell, 0),
                    );
                }
                let regen = self.c.base(db, None, &["ArcaneSurgeAlsoLifeRegen"])?;
                if regen > 0.0 {
                    self.c
                        .add(db, src("LifeRegen", Inc, regen * effect, "Arcane Surge"));
                }
            }
            if self.c.flag(db, None, &["Fanaticism"])?
                && main.is_some_and(|m| self.has_flag(m, "selfCast"))
            {
                let effect =
                    (75.0 * (1.0 + self.c.inc(db, None, &["BuffEffectOnSelf"])? / 100.0)).floor();
                self.c
                    .add(db, src("Speed", More, effect, "Fanaticism").flags(cast, 0));
                self.c
                    .add(db, src("Cost", More, -effect, "Fanaticism").flags(cast, 0));
                self.c.add(
                    db,
                    src("AreaOfEffect", Inc, effect, "Fanaticism").flags(cast, 0),
                );
            }
            if self
                .c
                .flag(db, None, &["Condition:CanGainSpiritInfusion"])?
            {
                let ge = || {
                    global_effect(&[
                        ("effectType", "Buff".into()),
                        ("effectName", "Spirit Infusion".into()),
                        ("unscalable", true.into()),
                    ])
                };
                let mt = || Tag::multiplier("SpiritInfusion");
                let channel = Tag::skill_type(self.st("Channel") as f64);
                let ks = self.kf("Spell");
                self.c.add(
                    db,
                    src("EnergyShieldRechargeFaster", Inc, 15.0, "Spirit Infusion")
                        .tag(mt())
                        .tag(ge()),
                );
                self.c.add(
                    db,
                    src("Damage", More, 5.0, "Spirit Infusion")
                        .flags(0, ks)
                        .tag(channel.clone())
                        .tag(mt())
                        .tag(ge()),
                );
                self.c.add(
                    db,
                    src("Cost", More, 10.0, "Spirit Infusion")
                        .flags(0, ks)
                        .tag(channel)
                        .tag(mt())
                        .tag(ge()),
                );
            }
            if self.c.flag(db, None, &["UnholyMight"])? {
                let effect = 1.0 + self.c.inc(db, None, &["BuffEffectOnSelf"])? / 100.0;
                self.c.add(
                    db,
                    src(
                        "PhysicalDamageConvertToChaos",
                        Base,
                        (100.0 * effect).floor(),
                        "Unholy Might",
                    ),
                );
                self.c
                    .add(db, src("Condition:CanWither", FlagT, true, "Unholy Might"));
            }
            if self.c.flag(db, None, &["ShepherdOfSouls"])? {
                let (vt, at) = (self.st("Vaal") as f64, self.st("Aura") as f64);
                let vaal = || Tag::skill_type(vt);
                let not_aura = || Tag::skill_type(at).negated();
                self.c.add(
                    db,
                    src("SoulCost", More, -80.0, "Shepherd of Souls")
                        .tag(vaal())
                        .tag(not_aura()),
                );
                self.c.add(
                    db,
                    src("SoulCost", Inc, 100.0, "Shepherd of Souls")
                        .tag(vaal())
                        .tag(not_aura())
                        .tag(Tag::multiplier("VaalSkillsUsedInPast8Seconds")),
                );
            }
            if self.c.flag(db, None, &["ChaoticMight"])? {
                let effect =
                    (30.0 * (1.0 + self.c.inc(db, None, &["BuffEffectOnSelf"])? / 100.0)).floor();
                self.c.add(
                    db,
                    src("PhysicalDamageGainAsChaos", Base, effect, "Chaotic Might"),
                );
            }
            if self.c.flag(db, None, &["Tailwind"])? {
                let effect = (8.0
                    * (1.0
                        + self
                            .c
                            .inc(db, None, &["TailwindEffectOnSelf", "BuffEffectOnSelf"])?
                            / 100.0))
                    .floor();
                self.c.add(db, src("ActionSpeed", Inc, effect, "Tailwind"));
            }
            if self.c.flag(db, None, &["Condition:TotemTailwind"])? {
                self.c
                    .add(db, src("TotemActionSpeed", Inc, 8.0, "Tailwind"));
            }
            if self.c.flag(db, None, &["Adrenaline"])? {
                let e = 1.0 + self.c.inc(db, None, &["BuffEffectOnSelf"])? / 100.0;
                self.c
                    .add(db, src("Damage", Inc, (100.0 * e).floor(), "Adrenaline"));
                self.c.add(
                    db,
                    src("Speed", Inc, (25.0 * e).floor(), "Adrenaline").flags(attack, 0),
                );
                self.c.add(
                    db,
                    src("Speed", Inc, (25.0 * e).floor(), "Adrenaline").flags(cast, 0),
                );
                self.c.add(
                    db,
                    src("MovementSpeed", Inc, (25.0 * e).floor(), "Adrenaline"),
                );
                self.c.add(
                    db,
                    src(
                        "PhysicalDamageReduction",
                        Base,
                        (10.0 * e).floor(),
                        "Adrenaline",
                    ),
                );
            }
            if self.c.flag(db, None, &["Condition:WildSavagery"])?
                && self.c.flag(db, None, &["WildSavagery"])?
            {
                self.c
                    .add(db, src("PhysicalDamage", Inc, 100.0, "Wild Savagery"));
                self.c
                    .add(db, src("ActionSpeed", Inc, 10.0, "Wild Savagery"));
                self.c.add(
                    db,
                    src(
                        "IgnoreEnemyPhysicalDamageReduction",
                        FlagT,
                        true,
                        "Wild Savagery",
                    ),
                );
                self.c
                    .add(db, src("StunImmune", FlagT, true, "Wild Savagery"));
            }
            if self.c.flag(db, None, &["Convergence"])? {
                let effect =
                    (30.0 * (1.0 + self.c.inc(db, None, &["BuffEffectOnSelf"])? / 100.0)).floor();
                self.c
                    .add(db, src("ElementalDamage", More, effect, "Convergence"));
            }
            if self.c.flag(db, None, &["HerEmbrace"])? {
                self.c.set_cond(db, "HerEmbrace", true);
                let s = "Her Embrace";
                self.c.add(db, src("AvoidStun", Base, 100.0, s));
                let sword = self.mf("Sword");
                self.c.add(
                    db,
                    src("PhysicalDamageGainAsFire", Base, 123.0, s).flags(sword, 0),
                );
                self.c.add(db, src("AvoidFreeze", Base, 100.0, s));
                self.c.add(db, src("AvoidChill", Base, 100.0, s));
                self.c.add(db, src("AvoidIgnite", Base, 100.0, s));
                self.c.add(db, src("Speed", Inc, 20.0, s).flags(attack, 0));
                self.c.add(db, src("Speed", Inc, 20.0, s).flags(cast, 0));
                self.c.add(db, src("MovementSpeed", Inc, 20.0, s));
            }
            if self.c.flag(db, None, &["Condition:OnConsecratedGround"])? {
                let effect = 1.0 + self.c.inc(db, None, &["ConsecratedGroundEffect"])? / 100.0;
                self.c.add(
                    db,
                    src("LifeRegenPercent", Base, 5.0 * effect, "Consecrated Ground"),
                );
                self.c.add(
                    db,
                    src(
                        "CurseEffectOnSelf",
                        Inc,
                        -50.0 * effect,
                        "Consecrated Ground",
                    ),
                );
                let acc =
                    (self.c.inc(db, None, &["ConsecratedGroundAlsoAccuracy"])? * effect).floor();
                self.c
                    .add(db, src("Accuracy", Inc, acc, "Consecrated Ground"));
            }
            if self.c.flag(db, None, &["Condition:PhantasmalMight"])? {
                let n = self.c.mult(db, "BuffOnSelf").unwrap_or(0.0);
                let limit = self.c.out(actor, "ActivePhantasmLimit").unwrap_or(1.0);
                self.c.set_mult(db, "BuffOnSelf", n + limit - 1.0);
            }
            if self.c.flag(db, None, &["Elusive"])? {
                let max_skill_inc = self
                    .c
                    .max(db, Some(&source_cfg("Skill")), &["ElusiveEffect"])?
                    .unwrap_or(0.0);
                let mut inc = self
                    .c
                    .inc(db, None, &["ElusiveEffect", "BuffEffectOnSelf"])?;
                let m = main.ok_or("Elusive without a main skill")?;
                if self
                    .c
                    .flag(self.lists[m].list, None, &["SupportedByNightblade"])?
                {
                    inc += self
                        .c
                        .inc(db, None, &["NightbladeSupportedElusiveEffect"])?;
                }
                inc += max_skill_inc;
                let mod_ = (1.0 + inc / 100.0)
                    * self
                        .c
                        .more(db, None, &["ElusiveEffect", "BuffEffectOnSelf"])?
                    * 100.0;
                let min_threshold = self
                    .c
                    .override_num(db, None, &["ElusiveEffectMinThreshold"])?
                    .unwrap_or(0.0);
                let inc_dur = self.c.base(db, None, &["ElusiveEffectIncreaseDuration"])?;
                let mut peak = mod_;
                let mut effect_mod = if inc_dur > 0.0 {
                    let rate =
                        20.0 / (1.0 + self.c.inc(db, None, &["ElusiveEffectLossSlower"])? / 100.0);
                    peak = mod_ + rate * inc_dur;
                    let dec_dur = (peak - min_threshold) / rate;
                    let total = inc_dur + dec_dur;
                    let avg_inc = (mod_ + peak) / 2.0;
                    let avg_dec = (peak + min_threshold) / 2.0;
                    (avg_inc * inc_dur + avg_dec * dec_dur) / total
                } else {
                    (mod_ + min_threshold) / 2.0
                };
                self.c.add(
                    db,
                    src("ElusiveEffect", Inc, max_skill_inc, "Max Skill Effect"),
                );
                if let Some(o) = self.c.override_num(db, None, &["ElusiveEffect"])? {
                    effect_mod = m_min(o, peak);
                }
                self.c.set_out(actor, "ElusiveEffectMod", effect_mod);
                let effect = effect_mod / 100.0;
                self.c.set_cond(db, "Elusive", true);
                self.c.add(
                    db,
                    src(
                        "AvoidAllDamageFromHitsChance",
                        Base,
                        (15.0 * effect).floor(),
                        "Elusive",
                    ),
                );
                self.c.add(
                    db,
                    src("MovementSpeed", Inc, (30.0 * effect).floor(), "Elusive"),
                );
            }
            if let Some(effect) = self.c.max(db, None, &["WitherEffectStack"])? {
                self.c
                    .add(db, src("Condition:CanWither", FlagT, true, "Config"));
                self.c.add(
                    edb,
                    src("ChaosDamageTaken", Inc, effect, "Withered")
                        .tag(multiplier_tag("WitheredStack", Some(15.0))),
                );
            }
            if self.c.flag(db, None, &["Condition:CanBeWithered"])? {
                let effect = 6.0 * (100.0 + self.c.inc(db, None, &["WitherEffectOnSelf"])?) / 100.0
                    * self.c.more(db, None, &["WitherEffectOnSelf"])?;
                self.c.add(
                    db,
                    src("ChaosDamageTaken", Inc, effect, "Withered")
                        .tag(multiplier_tag("WitheredStack", Some(15.0))),
                );
            }
            if self.c.flag(db, None, &["Excommunicated"])? {
                self.c
                    .add(db, src("ChaosDamage", More, -100.0, "Excommunicated"));
            }
            if self.c.flag(db, None, &["Blind"])?
                && !self.c.flag(db, None, &["CannotBeBlinded"])?
                && !self.c.flag(db, None, &["UnaffectedByBlind"])?
            {
                let mut effect =
                    1.0 + self.c.inc(db, None, &["BlindEffect", "BuffEffectOnSelf"])? / 100.0;
                if let Some(o) = self.c.override_num(db, None, &["BlindEffect"])? {
                    effect = m_min(o / 100.0, effect);
                }
                self.c
                    .add(db, src("Accuracy", More, (-20.0 * effect).floor(), "Blind"));
                self.c
                    .add(db, src("Evasion", More, (-20.0 * effect).floor(), "Blind"));
            }
            self.self_ailments(db)?;
            if self.c.flag(db, None, &["CanLeechLifeOnFullLife"])?
                && !self.c.flag(db, None, &["GhostReaver"])?
            {
                self.c.set_cond(db, "Leeching", true);
                self.c.set_cond(db, "LeechingLife", true);
            }
            if self
                .c
                .flag(db, None, &["CanLeechEnergyShieldOnFullEnergyShield"])?
            {
                self.c.set_cond(db, "Leeching", true);
                self.c.set_cond(db, "LeechingEnergyShield", true);
            }
            if self.c.flag(db, None, &["Condition:CanGainRage"])?
                || self.c.base(db, None, &["RageRegen"])? > 0.0
            {
                let max_stacks = (self.c.base(db, None, &["MaximumRage"])?
                    * self.c.more(db, None, &["MaximumRage"])?)
                .floor();
                let min_stacks = m_min(self.c.base(db, None, &["MinimumRage"])?, max_stacks);
                let config = self.c.base(db, None, &["Multiplier:RageStack"])?;
                let stacks = m_max(
                    m_min(config, max_stacks),
                    if min_stacks > 0.0 { min_stacks } else { 0.0 },
                );
                let effect = (stacks * self.c.calc_mod(db, None, &["RageEffect"])?).floor();
                self.c.set_out(actor, "RageEffect", effect);
                self.c
                    .add(db, src("Multiplier:RageEffect", Base, effect, "Base"));
                self.c.set_out(actor, "Rage", stacks);
                self.c.set_out(actor, "MaximumRage", max_stacks);
                self.c.add(db, src("Multiplier:Rage", Base, stacks, "Base"));
                let flags = if self.c.flag(db, None, &["Condition:RageSpellDamage"])? {
                    spell
                } else {
                    attack
                };
                self.c
                    .add(db, src("Damage", More, effect, "Rage").flags(flags, 0));
                if stacks == max_stacks {
                    self.c
                        .add(db, src("Condition:HaveMaximumRage", FlagT, true, ""));
                }
                let delay = 2.0 + self.c.base(db, None, &["InherentRageLossDelay"])?;
                self.c.set_out(actor, "InherentRageLossDelay", delay);
                let loss = if !self.c.flag(db, None, &["InherentRageLossIsPrevented"])? {
                    10.0 * (1.0 + self.c.inc(db, None, &["InherentRageLoss"])? / 100.0)
                } else {
                    0.0
                };
                self.c.set_out(actor, "InherentRageLoss", loss);
            }
            let burn = self.config_num("multiplierManaBurnStacks").unwrap_or(0.0);
            if burn > 0.0 {
                let mut max = self.c.base(db, None, &["MaxManaBurnStacks"])?;
                if max == 0.0 {
                    max = 9999.0;
                }
                let mut stacks = m_min(burn, max);
                self.c
                    .add(db, src("Multiplier:ManaBurnStacks", Base, stacks, "Config"));
                stacks +=
                    self.c
                        .base(db, Some(&actor_cfg("player")), &["EffectiveManaBurnStacks"])?;
                let name =
                    if self
                        .c
                        .flag(db, None, &["Condition:WeepingWoundsInsteadOfManaBurn"])?
                    {
                        "Multiplier:WeepingWoundsStacks"
                    } else {
                        "Multiplier:EffectiveManaBurnStacks"
                    };
                self.c.add(db, src(name, Base, stacks, "Config"));
            }
            let ash = self.c.base(db, None, &["CoveredInAshEffect"])?;
            if ash > 0.0 {
                self.c.add(
                    edb,
                    src("FireDamageTaken", Inc, m_min(ash, 20.0), "Covered in Ash"),
                );
            }
            let frost = self.c.base(db, None, &["CoveredInFrostEffect"])?;
            if frost > 0.0 {
                self.c.add(
                    edb,
                    src(
                        "ColdDamageTaken",
                        Inc,
                        m_min(frost, 20.0),
                        "Covered in Frost",
                    ),
                );
            }
            if self.c.flag(db, None, &["HasMalediction"])? {
                self.c.add(db, src("DamageTaken", Inc, 10.0, "Malediction"));
                self.c.add(db, src("Damage", Inc, -10.0, "Malediction"));
            }
            if self.c.flag(db, None, &["HasMaddeningPresence"])? {
                self.c
                    .add(db, src("ActionSpeed", Inc, -10.0, "Maddening Presence"));
                self.c
                    .add(db, src("Damage", Inc, -10.0, "Maddening Presence"));
            }
            if self.c.flag(db, None, &["HasShapersPresence"])? {
                self.c
                    .add(db, src("BuffExpireFaster", More, -20.0, "Shapers Presence"));
            }
            if self.c.flag(db, None, &["Condition:CanHaveSoulEater"])? {
                let max = match self.c.override_num(db, None, &["SoulEaterMax"])? {
                    Some(v) => v,
                    None => self.c.base(db, None, &["SoulEaterMax"])?,
                };
                for f in [attack, cast] {
                    self.c.add(
                        db,
                        src("Speed", Inc, 5.0, "Base")
                            .flags(f, 0)
                            .tag(multiplier_tag("SoulEaterStack", Some(max))),
                    );
                }
            }
        }
        self.apply_enemy_modifiers(actor, false)
    }

    /// Chill, shock, scorch and freeze on the actor (`doActorMisc`)
    fn self_ailments(&mut self, db: StoreId) -> Result<()> {
        let data = self.data().non_damaging_ailment.clone();
        let avoid = |p: &Self, a: &str, shock_applies: bool| -> Result<f64> {
            if p.c
                .flag(db, None, &[&format!("{a}Immune"), "ElementalAilmentImmune"])?
            {
                return Ok(100.0);
            }
            let mut v = p.c.base(
                db,
                None,
                &[
                    &format!("Avoid{a}"),
                    "AvoidAilments",
                    "AvoidElementalAilments",
                ],
            )?;
            if shock_applies
                && p.c
                    .flag(db, None, &["ShockAvoidAppliesToElementalAilments"])?
            {
                v += p.c.base(db, None, &["AvoidShock"])?;
            }
            if p.c
                .flag(db, None, &["SpellSuppressionAppliesToAilmentAvoidance"])?
            {
                v += p.c.base(db, None, &["SpellSuppressionChance"])? / 2.0;
            }
            Ok(m_min(v, 100.0).floor())
        };
        let value = |p: &Self, a: &str| -> Result<f64> {
            let self_over = p.c.base(db, None, &[&format!("Self{a}Override")])?;
            let over = p.c.override_num(db, None, &[&format!("{a}Val")])?;
            // `Override` returns no value when there is none, so `m_max`
            // takes the sum alone and the default is never reached
            Ok(match over {
                Some(o) => m_max(self_over, o),
                None => self_over,
            })
        };
        for (a, shock_applies) in [("Chill", true), ("Shock", false), ("Scorch", true)] {
            if !self.c.flag(db, None, &[a])? {
                continue;
            }
            let ad = data.get(a).cloned().unwrap_or_default();
            let v = value(self, a)?;
            let total = self.c.calc_mod(db, None, &[&format!("Self{a}Effect")])?;
            let av = avoid(self, a, shock_applies)?;
            let max = self
                .c
                .override_num(db, None, &[&format!("{a}Max")])?
                .unwrap_or(ad.max);
            let effect = if av == 100.0 {
                0.0
            } else {
                m_min(m_max((v * total).floor(), 0.0), max)
            };
            match a {
                "Chill" => {
                    let reversed = self.c.flag(db, None, &["SelfChillEffectIsReversed"])?;
                    if self.c.flag(db, None, &["SkitterbotBonechill"])? {
                        self.c.add(
                            db,
                            src(
                                "ColdDamageTaken",
                                Inc,
                                effect * if reversed { -1.0 } else { 1.0 },
                                "Bonechill",
                            ),
                        );
                    }
                    self.c.add(
                        db,
                        src(
                            "ActionSpeed",
                            Inc,
                            effect * if reversed { 1.0 } else { -1.0 },
                            "Chill",
                        ),
                    );
                }
                "Shock" => self.c.add(db, src("DamageTaken", Inc, effect, "Shock")),
                _ => self
                    .c
                    .add(db, src("ElementalResist", Base, -effect, "Scorch")),
            }
        }
        if self.c.flag(db, None, &["Freeze"])? {
            let effect = m_max(
                (70.0 * self.c.calc_mod(db, None, &["SelfChillEffect"])?).floor(),
                0.0,
            );
            self.c.add(db, src("ActionSpeed", Inc, -effect, "Freeze"));
        }
        Ok(())
    }
}

/// A buff's tag field as a string
fn tag_str(b: &Buff, k: &str) -> Option<String> {
    match b.tag.get(k)? {
        ModValue::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

fn tag_num(b: &Buff, k: &str) -> Option<f64> {
    b.tag.get(k).and_then(ModValue::as_number)
}

/// The `GlobalEffect` fields of a mod's first tag
pub(crate) fn first_tag_fields(m: &Mod) -> Option<&crate::mods::Record> {
    match &m.tags.first()?.kind {
        TagKind::Other { fields, .. } => Some(fields),
        _ => None,
    }
}

/// Merges `ExtraAuraEffect`-like values: mods with the same params sum
fn merge_extra(values: Vec<ModValue>) -> Result<Vec<Mod>> {
    let mut out: Vec<Mod> = Vec::new();
    for v in values {
        let ModValue::Record(r) = &v else { continue };
        let Some(ModValue::Mod(m)) = r.get("mod") else {
            continue;
        };
        match out
            .iter_mut()
            .find(|x| crate::mods::list::same_params(x, m))
        {
            Some(x) => x.value = ModValue::Number(number(&x.value)? + number(&m.value)?),
            None => out.push((**m).clone()),
        }
    }
    Ok(out)
}

impl<'a> Perform<'a> {
    /// `calcSkillDuration(skillModList, skillCfg, skillData, env, enemyDB)`
    pub fn skill_duration(&self, list: StoreId, cfg: Option<&Cfg>, skill: usize) -> Result<f64> {
        let mut names = vec!["Duration", "PrimaryDuration"];
        if self.sd_truthy(skill, "mineDurationAppliesToSkill") {
            names.push("MineDuration");
        }
        let dmod = m_max(self.c.calc_mod(list, cfg, &names)?, 0.0);
        let base = self.sd_num(skill, "duration").unwrap_or(0.0)
            + self.c.base(list, cfg, &["Duration", "PrimaryDuration"])?;
        let mut duration = base * dmod;
        let mut debuff_mult = 1.0;
        if self.env.mode.effective {
            debuff_mult = 1.0
                / m_max(
                    self.misc_data("BuffExpirationSlowCap"),
                    self.c.calc_mod(self.edb(), cfg, &["BuffExpireFaster"])?,
                );
        }
        if self.sd_truthy(skill, "debuff") {
            duration *= debuff_mult;
        }
        Ok(duration)
    }

    /// `determineCursePriority(curseName, activeSkill)`
    fn curse_priority(&self, name: &str, skill: Option<usize>) -> f64 {
        let cp = |k: &str| self.data().curse_priority.get(k).copied();
        let mut source = String::new();
        let mut slot = String::new();
        let mut socket = 1.0;
        if let Some(g) = skill
            .and_then(|i| self.skill(i).group)
            .and_then(|g| self.env.groups.get(g))
        {
            source = g.source.clone().unwrap_or_default();
            slot = g.slot.clone().unwrap_or_default();
            for (k, gem) in g.gem_list.iter().enumerate() {
                if gem.gem.as_ref().is_some_and(|d| d.name == name) {
                    socket = m_min(k as f64 + 1.0, 8.0);
                    break;
                }
            }
        }
        let base = cp(name).unwrap_or(0.0);
        let socket_p = socket * cp("SocketPriorityBase").unwrap_or(f64::NAN);
        let mut slot_p = cp(&slot.replace(" Swap", "")).unwrap_or(0.0);
        let source_p = if skill.is_some_and(|i| self.has_type(i, "Aura")) {
            cp("CurseFromAura").unwrap_or(f64::NAN)
        } else if !source.is_empty() {
            cp("CurseFromEquipment").unwrap_or(f64::NAN)
        } else {
            0.0
        };
        if !source.is_empty() && (Some(slot_p) == cp("Ring 2") || Some(slot_p) == cp("Ring 3")) {
            slot_p = cp("Ring 1").unwrap_or(f64::NAN);
        }
        base + socket_p + slot_p + source_p
    }

    /// The totem copies of an aura's defensive mods
    fn totem_aura_mods(&self, lists: [&[Mod]; 2], scale: f64) -> Result<Vec<Mod>> {
        let mut out = Vec::new();
        let scale_v = |v: f64| {
            if v.floor() == v {
                round(v * scale, 2.0).trunc()
            } else {
                v * scale
            }
        };
        for list in lists {
            for m in list {
                let name = m.name.as_str();
                if name == "EnergyShield"
                    || name == "Armour"
                    || name == "Evasion"
                    || crate::pat::is_match(name, "Resist?M?a?x?$")
                {
                    let mut t = m.clone();
                    t.name = Sym::new(&format!("Totem{name}"));
                    if scale != 1.0 {
                        match &mut t.value {
                            ModValue::Number(v) => *v = scale_v(*v),
                            ModValue::Record(r) => {
                                if let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod")) {
                                    inner.value = ModValue::Number(scale_v(number(&inner.value)?));
                                }
                            }
                            _ => {}
                        }
                    }
                    out.push(t);
                }
            }
        }
        Ok(out)
    }

    /// The buffs, debuffs, curses and auras of the active skills, applied
    fn buffs(&mut self, flasks: &[i64]) -> Result<()> {
        let pdb = self.pdb();
        let edb = self.edb();
        let main = self.main();
        let main_cfg = self.cfg(main);
        let mode = self.env.mode;
        let precision = self.c.precision;
        let mut buffs: BuffMap = Vec::new();
        let mut guards: BuffMap = Vec::new();
        let mut debuffs: BuffMap = Vec::new();
        let mut curses: Vec<Curse> = Vec::new();

        // Skills whose stages come from their own calculation
        for i in 0..self.lists.len() {
            if self.has_flag(i, "disable") {
                continue;
            }
            let name = self.skill_name(i).to_string();
            let part2 = self.mods(i).skill_part == Some(2.0);
            if !part2 {
                continue;
            }
            match name.as_str() {
                "Blight"
                | "Blight of Contagion"
                | "Blight of Atrophy"
                | "Penance Brand of Dissipation"
                | "Earthquake of Amplification" => {
                    return Err(format!("not ported: the cached calculation of {name}"));
                }
                "Scorching Ray" | "Scorching Ray of Immolation" => {
                    let list = self.lists[i].list;
                    let n = nospace(&name);
                    self.c.add(
                        list,
                        src(&format!("Multiplier:{n}MaxStages"), Base, 7.0, "Base"),
                    );
                    self.c.add(
                        list,
                        src(&format!("Multiplier:{n}StageAfterFirst"), Base, 7.0, "Base"),
                    );
                }
                _ => {}
            }
        }

        let mut applied_combustion = false;
        let mut warcry_list: Vec<String> = Vec::new();
        for i in 0..self.lists.len() {
            let list = self.lists[i].list;
            let cfg = self.cfg(i);
            let buff_list = self.mods(i).buff_list.clone();
            for buff in &buff_list {
                if let Some(c) = tag_str(buff, "effectCond")
                    && !self.c.get_condition(list, &c, Some(&cfg))?
                {
                    continue;
                }
                if let Some(c) = tag_str(buff, "effectEnemyCond")
                    && !self.c.get_condition(edb, &c, None)?
                {
                    continue;
                }
                let bname = buff.name.as_str();
                match buff.typ.as_str() {
                    "GlobalDB" => self.c.add_list(pdb, &buff.mod_list),
                    "Buff" | "Guard" => {
                        if !mode.buffs || (self.has_flag(i, "totem") && !buff.allow_totem_buff) {
                            continue;
                        }
                        let bcfg = buff.active_skill_buff.then(|| cfg.clone());
                        let store = if buff.active_skill_buff { list } else { pdb };
                        if !buff.apply_not_player {
                            self.marks[i].buff_skill = true;
                            let mut inc = self.c.inc(
                                store,
                                bcfg.as_ref(),
                                &["BuffEffect", "BuffEffectOnSelf", "BuffEffectOnPlayer"],
                            )?;
                            if buff.typ == "Buff" {
                                self.c.set_cond(
                                    pdb,
                                    &format!("AffectedBy{}", nospace(bname)),
                                    true,
                                );
                                inc += self.c.inc(
                                    list,
                                    bcfg.as_ref(),
                                    &[&format!("{}Effect", nospace(bname))],
                                )?;
                            }
                            let more = self.c.more(
                                store,
                                bcfg.as_ref(),
                                &["BuffEffect", "BuffEffectOnSelf"],
                            )?;
                            let l = scaled(&buff.mod_list, (1.0 + inc / 100.0) * more, precision);
                            if buff.typ == "Buff" {
                                merge_buff(&l, &mut buffs, bname)?;
                                if self.sd_truthy(i, "thisIsNotABuff")
                                    && let Some(b) = buffs.iter_mut().find(|b| b.0 == bname)
                                {
                                    b.2 = true;
                                }
                            } else {
                                merge_buff(&l, &mut guards, bname)?;
                            }
                        }
                    }
                    "Warcry" => {
                        if !mode.buffs {
                            continue;
                        }
                        let warcry_name = nospace(&bname.replace(" Cry", "").replace("'s", ""));
                        let base_exerts = self.c.base(
                            list,
                            Some(&main_cfg),
                            &[&format!("{warcry_name}ExertedAttacks")],
                        )?;
                        if base_exerts > 0.0 {
                            let extra = self.c.base(list, None, &["ExtraExertedAttacks"])?;
                            let mult = self.c.more(list, None, &["ExtraExertedAttacks"])?;
                            self.c.add(
                                pdb,
                                Mod::new(
                                    &format!("Num{warcry_name}Exerts"),
                                    Base,
                                    ((base_exerts + extra) * mult).floor(),
                                    None,
                                ),
                            );
                            if !warcry_list.iter().any(|w| w == bname) {
                                self.c.add(
                                    pdb,
                                    src("Multiplier:ExertingWarcryCount", Base, 1.0, bname),
                                );
                                warcry_list.push(bname.to_string());
                            }
                        }
                        if self.c.flag(list, None, &["CannotShareWarcryBuffs"])? {
                            continue;
                        }
                        let power = match self.c.override_num(pdb, None, &["WarcryPower"])? {
                            Some(v) => v,
                            None => m_max(
                                self.c.base(pdb, None, &["WarcryPower"])?
                                    * (1.0 + self.c.inc(pdb, None, &["WarcryPower"])? / 100.0),
                                self.c.base(pdb, None, &["MinimumWarcryPower"])?,
                            ),
                        };
                        let bonus: Vec<Option<f64>> = buff
                            .mod_list
                            .iter()
                            .map(|m| {
                                let f = first_tag_fields(m)?;
                                let warcry = matches!(f.get("effectType"), Some(ModValue::Str(s)) if &**s == "Warcry");
                                let div = f.get("div").and_then(ModValue::as_number)?;
                                if !warcry {
                                    return None;
                                }
                                let limit = f.get("limit").and_then(ModValue::as_number);
                                Some((limit.map_or(power, |l| m_min(power, l)) / div).floor())
                            })
                            .collect();
                        let full_duration = self.skill_duration(list, Some(&cfg), i)?;
                        let cd_over =
                            self.c
                                .override_num(list, Some(&cfg), &["CooldownRecovery"])?;
                        let cooldown = match cd_over {
                            Some(v) => v,
                            None => {
                                let cd = self.sd_num(i, "cooldown").ok_or(
                                    "attempt to perform arithmetic on a nil value (cooldown)",
                                )?;
                                (cd + self.c.base(list, Some(&cfg), &["CooldownRecovery"])?)
                                    / self.c.calc_mod(list, Some(&cfg), &["CooldownRecovery"])?
                            }
                        };
                        let uptime = if self.c.flag(pdb, None, &["Condition:WarcryMaxHit"])? {
                            1.0
                        } else {
                            m_min(full_duration / cooldown, 1.0)
                        };
                        if !self.c.flag(pdb, None, &["CannotGainWarcryBuffs"])?
                            && !buff.apply_not_player
                        {
                            self.marks[i].buff_skill = true;
                            self.c
                                .set_cond(pdb, &format!("AffectedBy{warcry_name}"), true);
                            let inc = self.c.inc(
                                list,
                                Some(&cfg),
                                &["BuffEffect", "BuffEffectOnSelf", "BuffEffectOnPlayer"],
                            )?;
                            let more = self.c.more(
                                list,
                                Some(&cfg),
                                &["BuffEffect", "BuffEffectOnSelf"],
                            )?;
                            let mut l = Vec::new();
                            for (m, b) in buff.mod_list.iter().zip(&bonus) {
                                let mult = (1.0 + inc / 100.0) * more * b.unwrap_or(1.0) * uptime;
                                l.extend(scaled(std::slice::from_ref(m), mult, precision));
                            }
                            merge_buff(&l, &mut buffs, bname)?;
                        }
                    }
                    "Aura" => {
                        if !mode.buffs {
                            continue;
                        }
                        let extra =
                            merge_extra(self.c.list(pdb, Some(&cfg), &["ExtraAuraEffect"])?)?;
                        if !self.sd_truthy(i, "auraCannotAffectSelf") {
                            let names = [
                                "AuraEffect",
                                "BuffEffect",
                                "BuffEffectOnSelf",
                                "AuraEffectOnSelf",
                                "AuraBuffEffect",
                                "SkillAuraEffectOnSelf",
                            ];
                            let inc = self.c.inc(list, Some(&cfg), &names)?;
                            let more = self.c.more(list, Some(&cfg), &names)?;
                            let mult = (1.0 + inc / 100.0) * more;
                            self.marks[i].buff_skill = true;
                            self.c.set_cond(pdb, "AffectedByAura", true);
                            if bname.starts_with("Vaal") {
                                let rest: String = bname.chars().skip(5).collect();
                                self.c.set_cond(
                                    pdb,
                                    &format!("AffectedBy{}", nospace(&rest)),
                                    true,
                                );
                            }
                            self.c
                                .set_cond(pdb, &format!("AffectedBy{}", nospace(bname)), true);
                            let mut l = scaled(&buff.mod_list, mult, precision);
                            l.extend(scaled(&extra, mult, precision));
                            merge_buff(&l, &mut buffs, bname)?;
                        }
                        if !(self.c.flag(pdb, None, &["SelfAurasCannotAffectAllies"])?
                            || self.c.flag(pdb, None, &["SelfAurasOnlyAffectYou"])?
                            || self
                                .c
                                .flag(pdb, None, &["SelfAuraSkillsCannotAffectAllies"])?)
                            && self.c.flag(pdb, None, &["AurasAffectEnemies"])?
                            && !self.c.flag(
                                list,
                                Some(&cfg),
                                &["SelfAurasAffectYouAndLinkedTarget"],
                            )?
                        {
                            let inc =
                                self.c
                                    .inc(list, Some(&cfg), &["AuraEffect", "BuffEffect"])?;
                            let more =
                                self.c
                                    .more(list, Some(&cfg), &["AuraEffect", "BuffEffect"])?;
                            let mult = (1.0 + inc / 100.0) * more;
                            let mut l = scaled(&buff.mod_list, mult, precision);
                            l.extend(scaled(&extra, mult, precision));
                            merge_buff(&l, &mut debuffs, bname)?;
                        }
                        if self.has_flag(main, "totem")
                            && !(self.c.flag(pdb, None, &["SelfAurasCannotAffectAllies"])?
                                || self
                                    .c
                                    .flag(pdb, None, &["SelfAuraSkillsCannotAffectAllies"])?)
                        {
                            self.marks[i].totem_buff_skill = true;
                            let main_list = self.lists[main].list;
                            self.c.set_cond(
                                main_list,
                                &format!("AffectedBy{}", nospace(bname)),
                                true,
                            );
                            self.c.set_cond(main_list, "AffectedByAura", true);
                            let names = ["AuraEffect", "BuffEffect", "AuraBuffEffect"];
                            let inc = self.c.inc(list, Some(&cfg), &names)?;
                            let more = self.c.more(list, Some(&cfg), &names)?;
                            let scale = m_max((1.0 + inc / 100.0) * more, 0.0);
                            let l = self.totem_aura_mods([&extra, &buff.mod_list], scale)?;
                            merge_buff(&l, &mut buffs, &format!("Totem {bname}"))?;
                        }
                        if mode.effective
                            && !self
                                .c
                                .list(pdb, Some(&cfg), &["ExtraAuraDebuffEffect"])?
                                .is_empty()
                            && !(self.c.flag(pdb, None, &["SelfAurasOnlyAffectYou"])?
                                || self.c.flag(
                                    list,
                                    Some(&cfg),
                                    &["SelfAurasAffectYouAndLinkedTarget"],
                                )?)
                            && !buff_list.iter().any(|b| b.typ == "AuraDebuff")
                        {
                            self.marks[i].debuff_skill = true;
                            let extra = merge_extra(self.c.list(
                                pdb,
                                Some(&cfg),
                                &["ExtraAuraDebuffEffect"],
                            )?)?;
                            merge_buff(&extra, &mut debuffs, bname)?;
                        }
                    }
                    "Debuff" | "AuraDebuff" => {
                        let stack_var = tag_str(buff, "effectStackVar");
                        let stack_count = match &stack_var {
                            Some(v) => {
                                let mut n =
                                    self.c
                                        .base(list, Some(&cfg), &[&format!("Multiplier:{v}")])?;
                                if let Some(l) = tag_num(buff, "effectStackLimit") {
                                    n = m_min(n, l);
                                } else if let Some(lv) = tag_str(buff, "effectStackLimitVar") {
                                    n = m_min(
                                        n,
                                        self.c.base(
                                            list,
                                            Some(&cfg),
                                            &[&format!("Multiplier:{lv}")],
                                        )?,
                                    );
                                }
                                n
                            }
                            None => self.sd_num(i, "stackCount").unwrap_or(1.0),
                        };
                        if !(mode.effective && stack_count > 0.0) {
                            continue;
                        }
                        self.marks[i].debuff_skill = true;
                        let cond = format!("AffectedBy{}", nospace(bname));
                        self.c.set_cond(edb, &cond, true);
                        self.c.set_cond(pdb, &cond, true);
                        let mut mult;
                        let mut extra = Vec::new();
                        if buff.typ == "AuraDebuff" {
                            extra = merge_extra(self.c.list(
                                pdb,
                                Some(&cfg),
                                &["ExtraAuraDebuffEffect"],
                            )?)?;
                            mult = 0.0;
                            if !self.c.flag(pdb, None, &["SelfAurasOnlyAffectYou"])? {
                                let names = ["AuraEffect", "BuffEffect", "DebuffEffect"];
                                let inc = self.c.inc(list, Some(&cfg), &names)?;
                                let more = self.c.more(list, Some(&cfg), &names)?;
                                mult = (1.0 + inc / 100.0) * more;
                            }
                        } else {
                            let inc = self.c.inc(list, Some(&cfg), &["DebuffEffect"])?;
                            let more = self.c.more(list, Some(&cfg), &["DebuffEffect"])?;
                            mult = (1.0 + inc / 100.0) * more;
                        }
                        let mut l = scaled(&buff.mod_list, mult * stack_count, precision);
                        l.extend(scaled(&extra, mult * stack_count, precision));
                        if self.sd(i, "stackCount").is_some() || stack_var.is_some() {
                            l.push(src(
                                &format!("Multiplier:{bname}Stack"),
                                Base,
                                stack_count,
                                bname,
                            ));
                        }
                        merge_buff(&l, &mut debuffs, bname)?;
                    }
                    "Curse" | "CurseBuff" => {
                        let mark = self.has_type(i, "Mark");
                        let self_cast =
                            !(self.has_type(i, "Triggered") || self.has_type(i, "Aura"));
                        self.c
                            .set_cond(pdb, &format!("SelfCast{}", nospace(bname)), self_cast);
                        let skip = self
                            .config_truthy(&format!("balanceOfTerrorSelfCast{}", nospace(bname)))
                            && !mark;
                        let applies = (mode.effective
                            && (!self.c.flag(edb, None, &["Hexproof"])?
                                || self.c.flag(pdb, None, &["CursesIgnoreHexproof"])?
                                || self.sd_truthy(i, "ignoreHexLimit")
                                || self.sd_truthy(i, "ignoreHexproof")))
                            || mark;
                        if skip || !applies {
                            continue;
                        }
                        let ignore = (self.c.flag(pdb, Some(&cfg), &["CursesIgnoreHexLimit"])?
                            || self.sd_truthy(i, "ignoreHexLimit"))
                            && !mark;
                        let mut curse = Curse {
                            name: bname.to_string(),
                            priority: self.curse_priority(bname, Some(i)),
                            is_mark: mark,
                            ignore_hex_limit: ignore,
                            socketed_curses_hex_limit: self.c.flag(
                                pdb,
                                Some(&cfg),
                                &["SocketedCursesAdditionalLimit"],
                            )?,
                            mod_list: None,
                            buff_mod_list: None,
                        };
                        let mut inc = self.c.inc(list, Some(&cfg), &["CurseEffect"])?
                            + self.c.inc(edb, None, &["CurseEffectOnSelf"])?;
                        if self.has_type(i, "Aura") && !self.has_type(i, "RemoteMined") {
                            inc += self.c.inc(list, Some(&cfg), &["AuraEffect"])?;
                        }
                        let mut more = self.c.more(list, Some(&cfg), &["CurseEffect"])?;
                        if !mark {
                            more *= self.c.more(edb, None, &["CurseEffectOnSelf"])?;
                        }
                        let mut mult = 0.0;
                        if !((self.c.flag(pdb, None, &["SelfAurasOnlyAffectYou"])?
                            || self.c.flag(
                                list,
                                Some(&cfg),
                                &["SelfAurasAffectYouAndLinkedTarget"],
                            )?)
                            && self.has_type(i, "Aura"))
                        {
                            mult = (1.0 + inc / 100.0) * more;
                        }
                        if buff.typ == "Curse" {
                            curse.mod_list = Some(scaled(&buff.mod_list, mult, precision));
                        } else {
                            let temp = scaled(&buff.mod_list, mult, precision);
                            let binc = self.c.inc(pdb, Some(&cfg), &["BuffEffectOnSelf"])?;
                            let bmore = self.c.more(pdb, Some(&cfg), &["BuffEffectOnSelf"])?;
                            curse.buff_mod_list =
                                Some(scaled(&temp, (1.0 + binc / 100.0) * bmore, precision));
                        }
                        curses.push(curse);
                    }
                    _ => {}
                }
            }
            if self.c.flag(list, None, &["Condition:CanWither"])? {
                let effect =
                    (6.0 * (1.0 + self.c.inc(pdb, None, &["WitherEffect"])? / 100.0)).floor();
                self.c
                    .add(pdb, Mod::new("WitherEffectStack", MaxT, effect, None));
            }
            if self.c.flag(edb, None, &["Condition:Ignited"])?
                && (self.has_type(i, "Damage") || self.has_type(i, "Attack"))
                && !applied_combustion
            {
                let combustion = self
                    .skill(i)
                    .support_list
                    .iter()
                    .any(|s| s.granted_effect.name == "Combustion");
                if combustion && !self.c.flag(list, Some(&cfg), &["CannotIgnite"])? {
                    let v = self.c.base(list, Some(&cfg), &["CombustionFireResist"])?;
                    self.c.add(
                        edb,
                        src("FireResist", Base, v, "Combustion")
                            .tag(global_effect(&[
                                ("effectType", "Debuff".into()),
                                ("effectName", "Combustion".into()),
                            ]))
                            .tag(Tag::condition("Ignited")),
                    );
                    applied_combustion = true;
                }
            }
        }

        if mode.combat {
            self.merge_flasks(flasks, true, false)?;
        }

        // Alternate charges
        if mode.combat {
            if self.c.flag(pdb, None, &["UseEnduranceCharges"])?
                && self
                    .c
                    .flag(pdb, None, &["EnduranceChargesConvertToBrutalCharges"])?
            {
                let v = self.c.base(pdb, None, &["PerBrutalTripleDamageChance"])?;
                let m = Mod::new("TripleDamageChance", Base, v, None)
                    .tag(Tag::multiplier("BrutalCharge"));
                self.c.add(pdb, m);
            }
            if self.c.flag(pdb, None, &["UseFrenzyCharges"])?
                && self
                    .c
                    .flag(pdb, None, &["FrenzyChargesConvertToAfflictionCharges"])?
            {
                let dmg = self.c.base(pdb, None, &["PerAfflictionAilmentDamage"])?;
                let eff = self.c.base(pdb, None, &["PerAfflictionNonDamageEffect"])?;
                let s = "Affliction Charges";
                let mt = || Tag::multiplier("AfflictionCharge");
                let ka = self.kf("Ailment");
                self.c
                    .add(pdb, src("Damage", More, dmg, s).flags(0, ka).tag(mt()));
                for n in [
                    "EnemyChillEffect",
                    "EnemyShockEffect",
                    "EnemyFreezeEffect",
                    "EnemyScorchEffect",
                    "EnemyBrittleEffect",
                    "EnemySapEffect",
                ] {
                    self.c.add(pdb, src(n, More, eff, s).tag(mt()));
                }
            }
        }

        // Extra curses
        for v in self.c.list(pdb, None, &["ExtraCurse"])? {
            let ModValue::Record(r) = &v else { continue };
            let Some(ModValue::Str(id)) = r.get("skillId") else {
                continue;
            };
            let Some(ge) = self.input.data.skills.skills.get(&**id).cloned() else {
                continue;
            };
            let level = r.get("level").and_then(ModValue::as_number);
            let mut effect = crate::calc_skills::SkillEffect::new(ge.clone(), level, 0.0);
            let mut gem_mods = Vec::new();
            crate::calc_skill_mods::merge_skill_instance_mods(
                &mut gem_mods,
                &mut effect,
                &[],
                self.input.data.setup,
                self.input.data.skills,
            )?;
            let curse_mods: Vec<Mod> = gem_mods
                .iter()
                .filter(|m| {
                    m.tags.iter().any(|t| match &t.kind {
                        TagKind::Other { kind, fields } => {
                            kind.as_str() == "GlobalEffect"
                                && matches!(fields.get("effectType"), Some(ModValue::Str(s)) if &**s == "Curse")
                        }
                        _ => false,
                    })
                })
                .cloned()
                .collect();
            if r.get("applyToPlayer").is_some_and(ModValue::truthy) {
                if self.c.base(pdb, None, &["AvoidCurse"])? < 100.0 {
                    self.c.set_cond(pdb, "Cursed", true);
                    let n = self.c.mult(pdb, "CurseOnSelf").unwrap_or(0.0);
                    self.c.set_mult(pdb, "CurseOnSelf", n + 1.0);
                    self.c
                        .set_cond(pdb, &format!("AffectedBy{}", nospace(&ge.name)), true);
                    let cfg = Cfg {
                        skill_name: Some(ge.name.as_str().into()),
                        ..Cfg::default()
                    };
                    let inc = self.c.inc(pdb, Some(&cfg), &["CurseEffectOnSelf"])?
                        + crate::mods::list::sum(&gem_mods, Inc, "CurseEffectAgainstPlayer")?;
                    let more = self.c.more(pdb, Some(&cfg), &["CurseEffectOnSelf"])?
                        * crate::mods::list::more(&gem_mods, "CurseEffectAgainstPlayer")?;
                    self.c
                        .scale_add_list(pdb, &curse_mods, m_max((1.0 + inc / 100.0) * more, 0.0));
                }
            } else if !self.c.flag(edb, None, &["Hexproof"])?
                || self.c.flag(pdb, None, &["CursesIgnoreHexproof"])?
            {
                let scale = (1.0 + self.c.inc(edb, None, &["CurseEffectOnSelf"])? / 100.0)
                    * self.c.more(edb, None, &["CurseEffectOnSelf"])?;
                curses.push(Curse {
                    name: ge.name.clone(),
                    priority: self.curse_priority(&ge.name, None),
                    is_mark: false,
                    ignore_hex_limit: false,
                    socketed_curses_hex_limit: false,
                    mod_list: Some(scaled(&curse_mods, scale, precision)),
                    buff_mod_list: None,
                });
            }
        }

        // Curse limit and slots
        let limit = if self
            .c
            .flag(pdb, None, &["CurseLimitIsMaximumPowerCharges"])?
        {
            self.c.out(PLAYER, "PowerChargesMax").unwrap_or(f64::NAN)
        } else {
            self.c.base(pdb, None, &["EnemyCurseLimit"])?
        };
        self.c.set_out(PLAYER, "EnemyCurseLimit", limit);
        let mut slots: Vec<Curse> = Vec::new();
        let mut mark_slotted = false;
        for curse in &curses {
            if curse.ignore_hex_limit || curse.socketed_curses_hex_limit {
                continue;
            }
            let mut slot: Option<usize> = None;
            let mut skip = false;
            for i in 0..self.lists.len() {
                let bl = &self.mods(i).buff_list;
                if bl.first().is_some_and(|b| b.name == curse.name) && self.has_type(i, "Aura") {
                    if self.c.flag(pdb, None, &["SelfAurasOnlyAffectYou"])?
                        || self.c.flag(
                            self.lists[i].list,
                            Some(&main_cfg),
                            &["SelfAurasAffectYouAndLinkedTarget"],
                        )?
                    {
                        skip = true;
                    }
                    break;
                }
            }
            let mut i = 1.0;
            while i <= limit {
                let k = i as usize - 1;
                if curse.is_mark && mark_slotted {
                    slot = None;
                    break;
                }
                match slots.get(k) {
                    None => {
                        slot = Some(k);
                        break;
                    }
                    Some(s) if s.name == curse.name => {
                        slot = (s.priority < curse.priority).then_some(k);
                        break;
                    }
                    Some(s) if s.priority < curse.priority => slot = Some(k),
                    _ => {}
                }
                i += 1.0;
            }
            if let Some(k) = slot {
                if slots.get(k).is_some_and(|s| s.is_mark) {
                    mark_slotted = false;
                }
                if !skip {
                    if k < slots.len() {
                        slots[k] = curse.clone();
                    } else if k == slots.len() {
                        slots.push(curse.clone());
                    } else {
                        return Err("not ported: a gap in the curse slots".into());
                    }
                }
                if curse.is_mark {
                    mark_slotted = true;
                }
            }
        }
        for curse in &curses {
            if curse.ignore_hex_limit {
                match slots.iter_mut().find(|s| s.name == curse.name) {
                    Some(s) => {
                        if s.priority < curse.priority {
                            *s = curse.clone();
                        }
                    }
                    None => slots.push(curse.clone()),
                }
            }
            if curse.socketed_curses_hex_limit {
                let value = self.c.base(pdb, None, &["SocketedCursesHexLimitValue"])?;
                let mut skip = false;
                for (i, slot) in slots.iter_mut().enumerate() {
                    if slot.name == curse.name {
                        if slot.priority < curse.priority {
                            *slot = curse.clone();
                        }
                        skip = true;
                        break;
                    }
                    if (i + 1) as f64 >= value {
                        skip = true;
                    }
                }
                if !skip {
                    slots.push(curse.clone());
                }
            }
        }

        // Guards
        let mut guard_slots: Vec<(String, Vec<Mod>)> = Vec::new();
        let mut non_vaal = false;
        for (name, l, _) in &guards {
            if name == "Vaal Molten Shell" {
                guard_slots.clear();
                non_vaal = false;
                guard_slots.push((name.clone(), l.clone()));
                break;
            } else if name.starts_with("Vaal") {
                guard_slots.push((name.clone(), l.clone()));
            } else if !non_vaal {
                guard_slots.push((name.clone(), l.clone()));
                non_vaal = true;
            }
        }
        if non_vaal {
            self.c.set_cond(pdb, "AffectedByNonVaalGuardSkill", true);
        }
        for (name, l) in &guard_slots {
            self.c.set_cond(pdb, "AffectedByGuardSkill", true);
            self.c
                .set_cond(pdb, &format!("AffectedBy{}", nospace(name)), true);
            merge_buff(l, &mut buffs, name)?;
        }
        let guard = self.c.cond(pdb, "AffectedByGuardSkill");
        self.c
            .set_out_val(PLAYER, "GuardSkillActive", OutVal::Bool(guard));

        // Apply the buffs and debuffs
        for (_, l, not_buff) in &buffs {
            self.c.add_list(pdb, l);
            if !not_buff {
                let n = self.c.mult(pdb, "BuffOnSelf").unwrap_or(0.0);
                self.c.set_mult(pdb, "BuffOnSelf", n + 1.0);
            }
        }
        for (_, l, _) in &debuffs {
            self.c.add_list(edb, l);
        }
        self.c.set_mult(pdb, "CurseOnEnemy", slots.len() as f64);
        for s in &slots {
            self.c.set_cond(edb, "Cursed", true);
            if s.is_mark {
                self.c.set_cond(edb, "Marked", true);
            }
            if let Some(l) = &s.mod_list {
                self.c.add_list(edb, l);
            }
            if let Some(l) = &s.buff_mod_list {
                self.c.add_list(pdb, l);
            }
        }
        self.curse_slots = slots.iter().map(|s| s.name.clone()).collect();

        // Impale stacks on the enemy
        let max_impale = self.c.base(pdb, None, &["ImpaleStacksMax"])?
            * (1.0
                + self
                    .c
                    .base(pdb, None, &["ImpaleAdditionalDurationChance"])?
                    / 100.0);
        let impale = || {
            src("Multiplier:ImpaleStacks", Base, max_impale, "Config").tag(Tag::condition("Combat"))
        };
        if !self
            .c
            .has_mod(edb, Base, None, &["Multiplier:ImpaleStacks"])?
        {
            self.c.add(edb, impale());
        } else if self.c.base(edb, None, &["Multiplier:ImpaleStacks"])? > max_impale {
            self.c.replace_mod(edb, impale());
        }

        if self
            .c
            .flag(pdb, None, &["ManaIncreasedByOvercappedLightningRes"])?
        {
            return Err("not ported: ManaIncreasedByOvercappedLightningRes".into());
        }

        if self.c.flag(
            pdb,
            Some(&main_cfg),
            &["Condition:CanInflictHallowingFlame"],
        )? {
            let mag = match self
                .c
                .override_num(pdb, None, &["HallowingFlameMagnitude"])?
            {
                Some(v) => v,
                None => self.c.inc(pdb, None, &["HallowingFlameMagnitude"])?,
            };
            let val = (25.0 * (1.0 + mag / 100.0)).floor();
            let inner = src("PhysicalDamageGainAsFire", Base, val, "Hallowing Flame")
                .tag(global_effect(&[
                    ("effectType", "Global".into()),
                    ("unscalable", true.into()),
                ]))
                .tag(Tag::actor_condition("enemy", "HallowingFlame"))
                .tag(Tag::multiplier("HallowingFlame").actor("enemy"));
            self.c.add(
                pdb,
                Mod::new("ExtraAura", List, record([("mod", inner.into())]), None),
            );
        }

        // Extra auras
        for v in self.c.list(pdb, None, &["ExtraAura"])? {
            let ModValue::Record(r) = &v else { continue };
            let Some(ModValue::Mod(m)) = r.get("mod") else {
                continue;
            };
            let flag = |k: &str| r.get(k).is_some_and(ModValue::truthy);
            if !flag("onlyAllies")
                && !(flag("fromAllies")
                    && self.c.flag(pdb, None, &["AlliesAurasCannotAffectSelf"])?)
            {
                let inc = self
                    .c
                    .inc(pdb, None, &["BuffEffectOnSelf", "AuraEffectOnSelf"])?;
                let more = self
                    .c
                    .more(pdb, None, &["BuffEffectOnSelf", "AuraEffectOnSelf"])?;
                self.c
                    .scale_add_list(pdb, std::slice::from_ref(m), (1.0 + inc / 100.0) * more);
                if !flag("notBuff") {
                    let n = self.c.mult(pdb, "BuffOnSelf").unwrap_or(0.0);
                    self.c.set_mult(pdb, "BuffOnSelf", n + 1.0);
                }
            }
            if flag("fromAllies") || !self.c.flag(pdb, None, &["SelfAurasCannotAffectAllies"])? {
                let name = m.name.as_str();
                let blacklist = name == "Speed" || name == "CritMultiplier" || name == "CritChance";
                if self.has_flag(main, "totem") && !blacklist {
                    let mut t = (**m).clone();
                    let (n, matches) =
                        crate::pat::gsub_n(name, "Condition:", "Condition:Totem", None);
                    t.name = Sym::new(&if matches < 1 {
                        format!("Totem{name}")
                    } else {
                        n
                    });
                    self.c.add(pdb, t);
                }
            }
        }

        if !self.c.list(pdb, None, &["AffectedByAuraMod"])?.is_empty() {
            return Err("bad argument #1 to 'pairs' (table expected, got nil)".into());
        }

        self.merge_keystones(pdb)?;
        Ok(())
    }
}

impl<'a> Perform<'a> {
    /// The strongest non-damaging ailments on the enemy
    fn ailments(&mut self) -> Result<()> {
        let pdb = self.pdb();
        let edb = self.edb();
        let main = self.main();
        let main_list = self.lists[main].list;
        let main_hit = self.has_flag(main, "hit");
        let data = self.data().non_damaging_ailment.clone();
        for (ailment, cond) in [
            ("Chill", "Chilled"),
            ("Shock", "Shocked"),
            ("Scorch", "Scorched"),
            ("Brittle", "Brittle"),
            ("Sap", "Sapped"),
        ] {
            // The sum of the player's mods is a number, so always true
            if self
                .c
                .flag(edb, None, &[&format!("Condition:Already{cond}")])?
                || self.c.flag(
                    edb,
                    None,
                    &[&format!("{ailment}Immune"), "ElementalAilmentImmune"],
                )?
                || self.c.base(
                    edb,
                    None,
                    &[
                        &format!("Avoid{ailment}"),
                        "AvoidAilments",
                        "AvoidElementalAilments",
                    ],
                )? >= 100.0
            {
                continue;
            }
            let ad = data.get(ailment).cloned().unwrap_or_default();
            let (n_base, n_over, n_min) = (
                format!("{ailment}Base"),
                format!("{ailment}Override"),
                format!("{ailment}Minimum"),
            );
            let mut over = 0.0;
            let mut minimum = 0.0;
            for (_, m) in self
                .c
                .tabulate(pdb, Some(Base), None, &[&n_base, &n_over, &n_min])?
            {
                let mut effect = number(&m.value)?;
                let name = m.name.as_str();
                let scales = name == n_base || name == n_min;
                if name == n_over {
                    self.c.add(
                        edb,
                        Mod::new(
                            &format!("Condition:{cond}"),
                            FlagT,
                            true,
                            m.source.as_deref(),
                        ),
                    );
                }
                let enemy_effect = format!("Enemy{ailment}Effect");
                if scales {
                    if !matches!(ailment, "Scorch" | "Sap" | "Brittle")
                        && !self
                            .c
                            .flag(main_list, None, &[&format!("Cannot{ailment}")])?
                        && main_hit
                        && self.c.flag(pdb, None, &["ChecksHighestDamage"])?
                    {
                        effect *= self.c.calc_mod(main_list, None, &[&enemy_effect])?;
                    } else {
                        effect *= self.c.calc_mod(pdb, None, &[&enemy_effect])?;
                    }
                }
                effect *= self
                    .c
                    .calc_mod(edb, None, &[&format!("Self{ailment}Effect")])?;
                if scales {
                    let mut o = m.clone();
                    o.name = Sym::new(&n_over);
                    o.value = ModValue::Number(effect);
                    o.source_slot = None;
                    o.unreached_tags.clear();
                    self.c.add(pdb, o);
                    if name == n_min {
                        minimum += effect;
                    }
                }
                over = m_max(m_max(over, effect), minimum);
            }
            let max_over = self
                .c
                .override_num(pdb, None, &[&format!("{ailment}Max")])?;
            let mut max = max_over.unwrap_or(0.0);
            if max_over.is_none() {
                for i in 0..self.lists.len() {
                    let base = self.lists[i].base;
                    let skill_max =
                        ad.max + self.c.base(base, None, &[&format!("{ailment}Max")])?;
                    if skill_max > max {
                        max = skill_max;
                    }
                }
            }
            self.c.set_out(PLAYER, &format!("Maximum{ailment}"), max);
            let val = self.c.base(edb, None, &[&format!("{ailment}Val")])?;
            let p = 10f64.powf(ad.precision);
            let current = (m_min(m_max(over, val), max) * p).floor() / p;
            self.c
                .set_out(PLAYER, &format!("Current{ailment}"), current);
            let num = current;
            let ct = || Tag::condition(cond);
            let mut mods = Vec::new();
            match ailment {
                "Chill" => {
                    mods.push(src("ActionSpeed", Inc, -num, "Chill").tag(ct()));
                    if self.c.flag(pdb, None, &["ChillEffectIncDamageTaken"])? {
                        mods.push(src("DamageTaken", Inc, num, "Ahuana's Bite").tag(ct()));
                    } else if self.c.flag(pdb, None, &["ChillEffectIncColdDamageTaken"])? {
                        mods.push(src("ColdDamageTaken", Inc, num, "Chilled by Hits").tag(ct()));
                    } else if self
                        .c
                        .flag(pdb, None, &["ChillingAreaIncColdDamageTaken"])?
                    {
                        mods.push(src("ColdDamageTaken", Inc, num, "Chilling Area").tag(ct()));
                    } else if matches!(
                        self.c.out_val(PLAYER, "HasBonechill"),
                        Some(OutVal::Bool(true))
                    ) && (self.has_guaranteed_bonechill
                        || self.c.base(edb, None, &["ChillVal"])? > 0.0)
                    {
                        mods.push(src("ColdDamageTaken", Inc, num, "Bonechill").tag(ct()));
                    }
                    if self.c.flag(pdb, None, &["ChillEffectLessDamageDealt"])? {
                        mods.push(src("Damage", More, -num / 2.0, "Shaper of Winter").tag(ct()));
                    }
                }
                "Shock" | "Scorch" => {
                    let (name, value, stack_src, by) = if ailment == "Shock" {
                        ("DamageTaken", num, "Shock Stacks", "DamageTakenByShock")
                    } else {
                        (
                            "ElementalResist",
                            -num,
                            "Scorch Stacks",
                            "ElementalResistByScorch",
                        )
                    };
                    let typ = if ailment == "Shock" { Inc } else { Base };
                    if self.c.flag(pdb, None, &[&format!("{ailment}CanStack")])? {
                        let max_name = format!("{ailment}StacksMax");
                        let limit_v = match self.c.override_num(pdb, None, &[&max_name])? {
                            Some(v) => v,
                            None => self.c.base(pdb, None, &[&max_name])?,
                        };
                        let limit = || -> Result<f64> { Ok(limit_v) };
                        let stacks = format!("{ailment}Stacks");
                        mods.push(
                            src(name, typ, value, ailment)
                                .tag(ct())
                                .tag(multiplier_tag(&stacks, Some(limit()?))),
                        );
                        let cur = num
                            * m_min(
                                self.c.base(edb, None, &[&format!("Multiplier:{stacks}")])?,
                                limit()?,
                            );
                        self.c.set_out(PLAYER, &format!("Current{ailment}"), cur);
                        if self.input.breakdown {
                            mods.push(
                                src(by, typ, value, stack_src)
                                    .tag(ct())
                                    .tag(multiplier_tag(&stacks, Some(limit()?))),
                            );
                        }
                    } else {
                        mods.push(src(name, typ, value, ailment).tag(ct()));
                    }
                }
                "Brittle" => mods.push(src("SelfCritChance", Base, num, "Brittle").tag(ct())),
                _ => mods.push(src("Damage", More, -num, "Sap").tag(ct())),
            }
            for m in mods {
                self.c.add(edb, m);
            }
            self.c.add(
                edb,
                Mod::new(&format!("Condition:Already{cond}"), FlagT, true, None).tag(ct()),
            );
        }
        for (k, m) in [
            ("CurrentChill", "ChillEffect"),
            ("CurrentShock", "ShockEffect"),
        ] {
            let name = format!("Multiplier:{m}");
            let mult = self.c.base(edb, None, &[&name])?;
            if let Some(cur) = self.c.out(PLAYER, k)
                && mult < cur
            {
                self.c.add(edb, src(&name, Base, cur - mult, ""));
            }
        }
        Ok(())
    }

    /// Exposure on the enemy
    fn exposures(&mut self) -> Result<()> {
        let pdb = self.pdb();
        let edb = self.edb();
        let caps = crate::pat::captures(self.input.tree_version, "(%d+)_(%d+)")
            .ok_or("a tree version without a number")?;
        let major: f64 = caps[0].parse().map_err(|_| "tree version")?;
        let minor: f64 = caps[1].parse().map_err(|_| "tree version")?;
        let ee = self.c.flag(pdb, None, &["ElementalEquilibrium"])?;
        for element in ["Fire", "Cold", "Lightning"] {
            if !((major <= 3.0 && minor <= 15.0)
                || !ee
                || !self
                    .c
                    .flag(edb, None, &[&format!("Condition:HitBy{element}Damage")])?)
            {
                continue;
            }
            let mut min = f64::INFINITY;
            let mut source: Option<Rc<str>> = Some("".into());
            for (v, m) in
                self.c
                    .tabulate(edb, Some(Base), None, &[&format!("{element}Exposure")])?
            {
                let v = number(&v)?;
                if v < min {
                    min = v;
                    source = m.source.clone();
                }
            }
            if min != f64::INFINITY {
                for (v, _) in self.c.tabulate(
                    pdb,
                    Some(Base),
                    None,
                    &["ExtraExposure", &format!("Extra{element}Exposure")],
                )? {
                    min += number(&v)?;
                }
                self.c.add(
                    edb,
                    src(&format!("Condition:Has{element}Exposure"), FlagT, true, ""),
                );
                // Without an override, `m_min` takes `min` alone
                let exp_min = self.c.override_num(pdb, None, &["ExposureMin"])?;
                self.c.add(
                    edb,
                    Mod::new(
                        &format!("{element}Resist"),
                        Base,
                        exp_min.map_or(min, |e| m_min(min, e)),
                        source.as_deref(),
                    ),
                );
                self.c.add(
                    pdb,
                    src("Condition:AppliedExposureRecently", FlagT, true, ""),
                );
            }
        }
        Ok(())
    }

    /// The allies' life pools the player's damage can be taken from
    fn ally_life(&mut self) -> Result<()> {
        let pdb = self.pdb();
        let redirects = [
            "takenFromSpectresBeforeYou",
            "takenFromMinionBeforeYou",
            "takenFromRadianceSentinelBeforeYou",
            "takenFromVoidSpawnBeforeYou",
            "takenFromStoneGolemBeforeYou",
            "takenFromTotemsBeforeYou",
            "takenFromVaalRejuvenationTotemsBeforeYou",
        ];
        if self.c.base(pdb, None, &redirects)? != 0.0 {
            return Err("not ported: allies' life pools".into());
        }
        Ok(())
    }
}

impl<'a> PerformInput<'a> {
    /// The input of a build's calculation
    pub fn new<'t: 'a>(
        build: &'a crate::build::Build<'t>,
        data: &'a CalcData<'a>,
        breakdown: bool,
    ) -> Result<PerformInput<'a>> {
        let tree = build.spec.tree;
        let class_id = build.spec.cur_class_id.unwrap_or(0);
        let class = tree
            .classes
            .iter()
            .find(|(id, _)| *id == class_id)
            .map(|(_, c)| c)
            .ok_or_else(|| format!("no class {class_id}"))?;
        let stat = |i: usize| class.base_stats[i].unwrap_or(f64::NAN);
        Ok(PerformInput {
            data,
            tree,
            tab: &build.items,
            config: &build.config.active().input,
            placeholder: &build.config.active().placeholder,
            spectre_list: &build.info.spectre_list,
            class_id,
            class_stats: [stat(0), stat(1), stat(2)],
            tree_version: &tree.version,
            breakdown,
        })
    }
}
