//! The items of the calculation setup: the part of `calcs.initEnv`
//! (`Modules/CalcSetup.lua`) that equips the items, builds the list of
//! radius jewels and merges the items' mods into `env.itemModDB`, with the
//! conditions and multipliers it sets.
//!
//! Not ported yet: the overrides of the calculations (`override.repItem`),
//! Energy Blade's weapons, the item warnings,
//! and the names of the skills items grant (`nameSpec`).
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::calc_setup::{RadiusJewel, calc_mod, set_source};
use crate::config::Inputs;
use crate::item::{Fields, GrantedSkill, Item};
use crate::items_tab::ItemsTab;
use crate::jewel::{RadiusData, RadiusFunc, RadiusType};
use crate::mods::list::{self, Precision, merge_additive, merge_mod, scale_mod};
use crate::mods::tag::TagKind;
use crate::mods::{Mod, ModType, ModValue, Store, Sym};
use crate::pat;
use crate::skills::SocketGroup;
use crate::spec::{OrderedMap, PassiveSpec};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub type Result<T> = std::result::Result<T, String>;

/// What the items' setup needs
#[derive(Clone, Copy)]
pub struct ItemsInput<'a, 't> {
    pub spec: &'a PassiveSpec<'t>,
    pub tab: &'a ItemsTab,
    /// The active skill set's socket groups
    pub socket_groups: &'a [SocketGroup],
    /// `env.initialNodeModDB`: the mods of the allocated nodes
    pub node_mods: &'a [Mod],
    /// `env.configInput`
    pub config: &'a Inputs,
    pub precision: &'a dyn Precision,
    /// `modDB.conditions.AffectedByEnergyBlade`
    pub energy_blade: bool,
    /// The labels of the jewel radii (`data.jewelRadius`)
    pub jewel_radius: &'a [String],
    /// `override.extraJewelFuncs`: the `ExtraJewelFunc` mods of the first
    /// pass, which give every socketed jewel more functions
    pub extra_jewel_funcs: &'a [Mod],
}

/// A skill an item grants (`env.grantedSkillsItems`)
#[derive(Clone, Debug, PartialEq)]
pub struct ItemSkill {
    pub skill: GrantedSkill,
    pub item_id: f64,
    pub slot_name: String,
}

/// An item's attribute requirements (`env.requirementsTableItems`)
#[derive(Clone, Debug, PartialEq)]
pub struct ItemRequirement {
    pub item_id: f64,
    pub slot_name: String,
    pub str: f64,
    pub dex: f64,
    pub int: f64,
}

/// What equipping the items sets up
#[derive(Debug)]
pub struct ItemsSetup {
    /// `env.itemModDB`
    pub item_db: Store,
    /// The class an item is restricted to, by its title without spaces:
    /// the value of that condition in `env.itemModDB`
    pub class_restrictions: HashMap<String, String>,
    /// `env.player.itemList`: the item in each slot, by id
    pub item_list: BTreeMap<String, f64>,
    /// Multipliers set on `env.modDB`
    pub player_multipliers: HashMap<Sym, f64>,
    /// Mods added to `env.modDB`
    pub player_mods: Vec<Mod>,
    /// `env.radiusJewelList`
    pub radius_jewels: Vec<RadiusJewel>,
    /// `env.extraRadiusNodeList`: unallocated nodes in the radius of a jewel
    /// that is not `Self`
    pub extra_radius_nodes: OrderedMap<()>,
    /// `env.grantedSkillsItems`
    pub granted_skills: Vec<ItemSkill>,
    /// `env.explodeSources`: items whose base mods can explode enemies
    pub explode_sources: Vec<f64>,
    /// `env.flasks` and `env.tinctures`: the active ones, by item id, in
    /// the order they were added
    pub flasks: Vec<i64>,
    pub tinctures: Vec<i64>,
    /// `env.flaskSlotMap`: each flask's slot number, by item id
    pub flask_slots: BTreeMap<i64, f64>,
    pub requirements: Vec<ItemRequirement>,
    /// `env.aegisModList`, `env.theIronMass` and `env.weaponModList1`
    pub aegis_mods: Option<Vec<Mod>>,
    pub iron_mass_mods: Option<Vec<Mod>>,
    pub weapon1_mods: Option<Vec<Mod>>,
}

fn field_num(fields: &Option<Fields>, key: &str) -> Option<f64> {
    fields.as_ref()?.get(key)?.as_number()
}

/// `name:gsub(" ", "")`
fn squash(s: &str) -> String {
    s.replace(' ', "")
}

fn base_name(item: &Item) -> &str {
    item.base_name.as_deref().unwrap_or("")
}

fn typ(item: &Item) -> &str {
    item.typ.as_deref().unwrap_or("")
}

fn sub_type(item: &Item) -> Option<&str> {
    item.base.as_ref()?.sub_type.as_deref()
}

/// The mods of an item in a slot: `item.modList or item.slotModList[n]`
fn slot_mods(item: &Item, slot_num: Option<f64>) -> Option<&[Mod]> {
    if let Some(m) = &item.mod_list {
        return Some(m);
    }
    let n = slot_num?;
    let lists = item.slot_mod_list.as_ref()?;
    (n.fract() == 0.0 && n >= 1.0)
        .then(|| lists.get(n as usize - 1).map(Vec::as_slice))
        .flatten()
}

fn has_socketed_in(m: &Mod) -> bool {
    m.tags
        .iter()
        .any(|t| matches!(t.kind, TagKind::SocketedIn(_)))
}

/// The slot a `DisablesItem` tag disables, unless the item there is of
/// the excluded type: `None` when the mod has no such tag
fn disabled_slot(m: &Mod, items: &HashMap<String, &Item>) -> Option<Option<String>> {
    for t in &m.tags {
        let TagKind::Other { kind, fields } = &t.kind else {
            continue;
        };
        if kind.as_str() != "DisablesItem" {
            continue;
        }
        let slot = match fields.get("slotName") {
            Some(ModValue::Str(s)) => s.to_string(),
            _ => return Some(None),
        };
        if let Some(ModValue::Str(ex)) = fields.get("excludeItemType")
            && items.get(&slot).is_some_and(|i| typ(i) == &**ex)
        {
            return Some(None);
        }
        return Some(Some(slot));
    }
    None
}

impl ItemsSetup {
    fn add_mult(&mut self, name: &str, v: f64) {
        *self
            .item_db
            .multipliers
            .entry(Sym::new(name))
            .or_insert(0.0) += v;
    }

    fn mult(&self, name: &str) -> Option<f64> {
        self.item_db.multipliers.get(&Sym::new(name)).copied()
    }

    fn set_mult(&mut self, name: &str, v: f64) {
        self.item_db.multipliers.insert(Sym::new(name), v);
    }

    fn set_cond(&mut self, name: &str) {
        self.item_db.conditions.insert(Sym::new(name), true);
    }

    fn cond(&self, name: &str) -> bool {
        self.item_db
            .conditions
            .get(&Sym::new(name))
            .copied()
            .unwrap_or(false)
    }

    /// `env.itemModDB:ScaleAddMod(mod, scale)`
    fn scale_add(&mut self, m: &Mod, scale: f64, p: &dyn Precision) {
        self.item_db.add(scale_mod(m, scale, p));
    }

    fn scale_add_list(&mut self, mods: &[Mod], scale: f64, p: &dyn Precision) {
        for m in mods {
            self.scale_add(m, scale, p);
        }
    }
}

/// A function of a jewel function list (`{ type = ..., func = ... }`)
fn radius_func(v: &ModValue) -> Result<(RadiusType, RadiusFunc)> {
    let ModValue::Record(r) = v else {
        return Err(format!("jewel function {v:?}"));
    };
    let typ = match r.get("type") {
        Some(ModValue::Str(t)) => RadiusType::new(t).ok_or(format!("jewel function type {t}"))?,
        t => return Err(format!("jewel function type {t:?}")),
    };
    let func = RadiusFunc::new(r.get("func").ok_or("jewel function without func")?)?;
    Ok((typ, func))
}

/// Whether an extra jewel function applies to a jewel: its tags are
/// conditions on the jewel's rarity (`{ type = "ItemCondition", itemSlot =
/// "{SlotName}", rarityCond = ... }`), which find the jewel itself
fn extra_func_applies(m: &Mod, item: &Item) -> Result<bool> {
    for t in &m.tags {
        let TagKind::ItemCondition(c) = &t.kind else {
            return Err(format!("extra jewel function tag {t:?}"));
        };
        let Some(rarity) = &c.rarity_cond else {
            return Err(format!("extra jewel function condition {c:?}"));
        };
        if (item.rarity == **rarity) == c.neg {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The radius jewels of a jewel in a socket, and the unallocated nodes in
/// their radius
fn add_radius_jewels(
    out: &mut ItemsSetup,
    input: &ItemsInput,
    node_id: u32,
    item: &Item,
    item_id: f64,
) -> Result<()> {
    if item.jewel_radius_index.is_none() && input.extra_jewel_funcs.is_empty() {
        return Ok(());
    }
    let spec = input.spec;
    let tree = spec.tree;
    let socket = tree.node(node_id).ok_or(format!("no socket {node_id}"))?;
    let in_radius = |index: Option<f64>| -> Option<&Vec<u32>> {
        let index = index.filter(|i| i.fract() == 0.0 && *i >= 1.0)?;
        socket.nodes_in_radius.as_ref()?.get(index as usize - 1)
    };
    let mut funcs: Vec<(RadiusType, RadiusFunc, Option<f64>)> =
        match item.jewel_data.as_ref().and_then(|j| j.get("funcList")) {
            Some(ModValue::List(l)) => l
                .iter()
                .map(|f| radius_func(f).map(|(t, f)| (t, f, item.jewel_radius_index)))
                .collect::<Result<_>>()?,
            // The default function tallies the attributes in the radius
            _ => vec![(
                RadiusType::Self_,
                RadiusFunc::Tally,
                item.jewel_radius_index,
            )],
        };
    for m in input.extra_jewel_funcs {
        if m.name.as_str() != "ExtraJewelFunc" || !extra_func_applies(m, item)? {
            continue;
        }
        let label = match &m.value {
            ModValue::Record(r) => r.get("radius").and_then(|v| match v {
                ModValue::Str(s) => Some(s.clone()),
                _ => None,
            }),
            _ => None,
        };
        let index = label
            .and_then(|l| input.jewel_radius.iter().position(|r| **r == *l))
            .map(|i| i as f64 + 1.0);
        let (typ, func) = radius_func(&m.value)?;
        funcs.push((typ, func, index));
    }
    for (typ, func, index) in funcs {
        let ids = in_radius(index);
        let nodes = ids
            .map(|ids| {
                ids.iter()
                    .filter_map(|&id| Some((id, tree.node(id)?.kind)))
                    .collect()
            })
            .unwrap_or_default();
        if typ != RadiusType::Self_ && socket.nodes_in_radius.is_some() {
            let ids = ids.ok_or("a jewel function without a radius")?;
            for &id in ids {
                if !spec.alloc_nodes.contains(id) {
                    out.extra_radius_nodes.insert(id, ());
                }
            }
        }
        out.radius_jewels.push(RadiusJewel {
            node_id,
            item_id,
            nodes,
            func,
            typ,
            data: RadiusData::default(),
        });
    }
    Ok(())
}

/// Equips the items: the first loop of `initEnv` over the slots, which
/// picks each slot's item, collects the skills items grant and builds the
/// radius jewel list
fn equip<'a>(
    input: &ItemsInput<'a, '_>,
    out: &mut ItemsSetup,
) -> Result<HashMap<String, &'a Item>> {
    let tab = input.tab;
    let spec = input.spec;
    let second_set = tab.active_item_set().use_second_weapon_set;
    let ring3 = list::flag(input.node_mods, "AdditionalRingSlot")?;
    let ignore_limits = input
        .config
        .get("ignoreJewelLimits")
        .is_some_and(|v| v.truthy());
    let mut items = HashMap::new();
    let mut jewel_limits: HashMap<String, f64> = HashMap::new();
    for slot in &tab.ordered_slots {
        let mut slot_name = slot.slot_name.clone();
        if (slot_name == "Graft 1" || slot_name == "Graft 2") && !spec.tree.version.contains("3_27")
        {
            continue;
        }
        if slot_name == "Ring 3" && !ring3 {
            continue;
        }
        let item = tab.item(slot.sel_item_id);
        if let Some(item) = item {
            for skill in &item.granted_skills {
                out.granted_skills.push(ItemSkill {
                    skill: skill.clone(),
                    item_id: slot.sel_item_id,
                    slot_name: slot_name.clone(),
                });
            }
            if list::flag(&item.base_mod_list, "CanExplode")? {
                out.explode_sources.push(slot.sel_item_id);
            }
        }
        if slot
            .weapon_set
            .is_some_and(|s| s != if second_set { 2 } else { 1 })
        {
            continue;
        }
        if slot.weapon_set == Some(2) && second_set {
            slot_name = slot_name.replace(" Swap", "");
        }
        if let Some(node_id) = slot.node_id {
            if !spec.alloc_nodes.contains(node_id) {
                continue;
            }
            if let Some(item) = item {
                if typ(item) == "Jewel" && item.name.contains("The Adorned, Crimson Jewel") {
                    for (key, mult) in [
                        ("corruptedMagicJewelIncEffect", "CorruptedMagicJewelEffect"),
                        ("corruptedRareJewelIncEffect", "CorruptedRareJewelEffect"),
                    ] {
                        if let Some(v) = field_num(&item.jewel_data, key) {
                            out.player_multipliers.insert(Sym::new(mult), v / 100.0);
                        }
                    }
                }
                if let Some(limit) = item.limit
                    && !ignore_limits
                {
                    let key = if sub_type(item) == Some("Timeless") {
                        "Historic".to_string()
                    } else {
                        item.title.clone().unwrap_or_default()
                    };
                    let count = jewel_limits.entry(key).or_insert(0.0);
                    if *count >= limit {
                        continue;
                    }
                    *count += 1.0;
                }
                add_radius_jewels(out, input, node_id, item, slot.sel_item_id)?;
            }
        }
        if let Some(item) = item
            && typ(item) == "Flask"
            && sub_type(item) == Some("Life")
            && item.flask_data.is_some()
        {
            let total = field_num(&item.flask_data, "lifeTotal").unwrap_or(0.0);
            let cur = out.mult("LifeFlaskRecovery").unwrap_or(0.0);
            out.set_mult("LifeFlaskRecovery", lua_max(cur, total));
        }
        match item {
            Some(item) => {
                items.insert(slot_name, item);
            }
            None => {
                items.remove(&slot_name);
            }
        }
    }
    Ok(items)
}

/// `math.max(a, b)`
fn lua_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Removes the items other items disable (`DisablesItem`), every other
/// one along a chain of disablers
fn disable_items(input: &ItemsInput, items: &mut HashMap<String, &Item>) -> Result<()> {
    let mut disablers: BTreeMap<String, String> = BTreeMap::new();
    let mut disabled: HashMap<String, String> = HashMap::new();
    // The tree's disablers, then the items'
    let flag = ModType::new("Flag");
    for m in input.node_mods {
        if m.name.as_str() != "CanNotUseItem"
            || m.typ != flag
            || !m
                .source
                .as_deref()
                .is_some_and(|s| s.split(':').next() == Some("Tree"))
        {
            continue;
        }
        if let Some(Some(slot)) = disabled_slot(m, items) {
            let source = m.source.as_deref().unwrap_or("").to_string();
            disablers.insert(source.clone(), slot.clone());
            disabled.insert(slot, source);
        }
    }
    for slot in &input.tab.ordered_slots {
        let Some(item) = items.get(&slot.slot_name) else {
            continue;
        };
        let mods = slot_mods(item, slot.slot_num).ok_or("item without mods")?;
        for m in mods {
            if let Some(Some(target)) = disabled_slot(m, items) {
                disablers.insert(slot.slot_name.clone(), target.clone());
                disabled.insert(target, slot.slot_name.clone());
            }
        }
    }
    let mut visited = BTreeSet::new();
    let mut true_disabled = BTreeSet::new();
    for start in disablers.keys() {
        if visited.contains(start) {
            continue;
        }
        // Find the start of the chain
        let mut slot = start.clone();
        let mut chain = BTreeSet::new();
        while let Some(prev) = disabled.get(&slot) {
            slot = prev.clone();
            if !chain.insert(slot.clone()) {
                break;
            }
        }
        // Disable every other item along it
        loop {
            visited.insert(slot.clone());
            let Some(next) = disablers.get(&slot) else {
                break;
            };
            slot = next.clone();
            visited.insert(slot.clone());
            true_disabled.insert(slot.clone());
            match disablers.get(&slot) {
                Some(next) if !visited.contains(next) => slot = next.clone(),
                _ => break,
            }
        }
    }
    for slot in true_disabled {
        items.remove(&slot);
    }
    Ok(())
}

/// `rarity:gsub("(%a)(%u*)", ...)`: `MAGIC` is `Magic`
fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_string() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// The influence multipliers and the item property each counts
const INFLUENCE_MULTS: [(&str, usize); 6] = [
    ("ShaperItem", 0),
    ("ElderItem", 1),
    ("WarlordItem", 2),
    ("HunterItem", 3),
    ("CrusaderItem", 4),
    ("RedeemerItem", 5),
];

/// `env.itemModDB` and the rest of the items' setup
pub fn setup_items(input: &ItemsInput) -> Result<ItemsSetup> {
    let mut out = ItemsSetup {
        item_db: Store::db(),
        class_restrictions: HashMap::new(),
        item_list: BTreeMap::new(),
        player_multipliers: HashMap::new(),
        player_mods: Vec::new(),
        radius_jewels: Vec::new(),
        extra_radius_nodes: OrderedMap::default(),
        granted_skills: Vec::new(),
        explode_sources: Vec::new(),
        flasks: Vec::new(),
        tinctures: Vec::new(),
        flask_slots: BTreeMap::new(),
        requirements: Vec::new(),
        aegis_mods: None,
        iron_mass_mods: None,
        weapon1_mods: None,
    };
    let p = input.precision;
    let spec = input.spec;
    let mut items = equip(input, &mut out)?;
    if !input
        .config
        .get("ignoreItemDisablers")
        .is_some_and(|v| v.truthy())
    {
        disable_items(input, &mut items)?;
    }
    for slot in &input.tab.ordered_slots {
        let slot_name = &slot.slot_name;
        let Some(&item) = items.get(slot_name) else {
            continue;
        };
        if typ(item) == "Flask" {
            out.set_cond(&format!(
                "Have{}",
                base_name(item).replace(char::is_whitespace, "")
            ));
            let id = item.id.unwrap_or(0.0) as i64;
            if slot.active && !out.flasks.contains(&id) {
                out.flasks.push(id);
            }
            if let Some(n) = pat::capture(slot_name, "Flask (%d+)").and_then(|n| pat::to_number(&n))
            {
                out.flask_slots.insert(id, n);
            }
            if sub_type(item) == Some("Life") {
                let charges =
                    field_num(&item.flask_data, "chargesMax").ok_or("flask without charges")?;
                if charges > out.mult("LifeFlaskCharges").unwrap_or(0.0) {
                    out.set_mult("LifeFlaskCharges", charges);
                }
            }
            continue;
        }
        if typ(item) == "Tincture" {
            let id = item.id.unwrap_or(0.0) as i64;
            if slot.active && !out.tinctures.contains(&id) {
                out.tinctures.push(id);
            }
            continue;
        }
        let mut scale = 1.0;
        if typ(item) == "Jewel"
            && sub_type(item) == Some("Abyss")
            && let Some(parent) = &slot.parent_slot
        {
            let parent_item = out.item_list.get(parent).and_then(|&id| input.tab.item(id));
            match parent_item {
                Some(pi) if pi.abyssal_socket_count >= slot.slot_num.unwrap_or(f64::NAN) => {
                    scale = pi.socketed_jewel_effect_modifier;
                }
                _ => continue,
            }
        }
        if let Some(node_id) = slot.node_id
            && typ(item) == "Jewel"
            && let Some(per) = field_num(&item.jewel_data, "jewelIncEffectFromClassStart")
            && let Some(dist) = spec
                .index
                .get(&node_id)
                .and_then(|&i| spec.nodes[i].distance_to_class_start)
        {
            scale += dist * (per / 100.0);
        }
        let item_id = item.id.unwrap_or(0.0);
        out.item_list.insert(slot_name.clone(), item_id);
        let src: &[Mod] = slot_mods(item, slot.slot_num).unwrap_or(&[]);
        out.requirements.push(ItemRequirement {
            item_id,
            slot_name: slot_name.clone(),
            str: item.requirements.str_mod,
            dex: item.requirements.dex_mod,
            int: item.requirements.int_mod,
        });
        if typ(item) == "Jewel" && sub_type(item) == Some("Abyss") {
            let mult = squash(base_name(item));
            let cond = format!("Have{mult}");
            if !out.cond(&cond) {
                out.set_cond(&cond);
                out.add_mult("AbyssJewelType", 1.0);
            }
            if let Some(parent) = &slot.parent_slot {
                out.set_cond(&format!("{cond}In{parent}"));
                out.add_mult(&format!("{mult}In{parent}"), 1.0);
            }
            out.add_mult("AbyssJewel", 1.0);
            match item.rarity.as_str() {
                "NORMAL" => out.add_mult("NormalAbyssJewels", 1.0),
                "MAGIC" => out.add_mult("MagicAbyssJewels", 1.0),
                "RARE" => out.add_mult("RareAbyssJewels", 1.0),
                "UNIQUE" | "RELIC" => out.add_mult("UniqueAbyssJewels", 1.0),
                _ => {}
            }
            out.add_mult(&mult, 1.0);
        }
        let aegis = spec.alloc_nodes.contains(45175)
            && spec
                .tree
                .node(45175)
                .is_some_and(|n| &*n.dn == "Necromantic Aegis");
        let weapon = slot_name == "Weapon 1" || slot_name == "Weapon 2";
        if typ(item) == "Shield" && aegis {
            // Necromantic Aegis: the shield's mods go to the minions, but
            // those for socketed gems
            let mut aegis_mods = Vec::new();
            for m in src {
                if has_socketed_in(m) {
                    out.scale_add(m, scale, p);
                } else {
                    aegis_mods.push(scale_mod(m, scale, p));
                }
            }
            out.aegis_mods = Some(aegis_mods);
        } else if weapon && input.energy_blade {
            return Err("Energy Blade is not ported".into());
        } else if slot_name == "Weapon 1" && item.name == "The Iron Mass, Gladius" {
            let mut iron_mass = Vec::new();
            for m in src {
                if !has_socketed_in(m) {
                    iron_mass.push(scale_mod(m, scale, p));
                }
                out.scale_add(m, scale, p);
            }
            out.iron_mass_mods = Some(iron_mass);
        } else if slot_name == "Weapon 1"
            && item
                .granted_skills
                .first()
                .is_some_and(|s| s.skill_id.as_deref() == Some("UniqueAnimateWeapon"))
        {
            // The Dancing Dervish
            let mut weapon_mods = Vec::new();
            for m in src {
                if has_socketed_in(m) {
                    out.scale_add(m, scale, p);
                } else {
                    weapon_mods.push(scale_mod(m, scale, p));
                }
            }
            out.weapon1_mods = Some(weapon_mods);
        } else if item.name.contains("Kalandra's Touch") {
            let other = match slot_name.as_str() {
                "Ring 1" => items.get("Ring 2"),
                "Ring 2" => items.get("Ring 1"),
                _ => None,
            };
            if let Some(other) = other
                && !other.name.contains("Kalandra's Touch")
            {
                for m in slot_mods(other, slot.slot_num).unwrap_or(&[]) {
                    if has_socketed_in(m) {
                        continue;
                    }
                    let mut copy = m.clone();
                    set_source(&mut copy, &item.mod_source);
                    out.scale_add(&copy, scale, p);
                }
                for (mult, i) in INFLUENCE_MULTS {
                    if other.influences[i] {
                        out.add_mult(mult, 1.0);
                        out.add_mult(&format!("Non{mult}"), -1.0);
                    }
                }
                if other.corrupted {
                    out.add_mult("CorruptedItem", 1.0);
                    out.add_mult("NonCorruptedItem", -1.0);
                }
                if other.influences[0] || other.influences[1] {
                    out.add_mult("ShaperOrElderItem", 1.0);
                }
                out.add_mult(&format!("{}Equipped", squash(base_name(other))), 1.0);
            }
            // Only the implicit skills work
            for m in src {
                if m.name.as_str() == "ExtraSkill" {
                    out.scale_add(m, scale, p);
                }
            }
        } else if typ(item) == "Quiver"
            && (items
                .get("Weapon 1")
                .is_some_and(|w| w.name.contains("Widowhail"))
                || list::sum(input.node_mods, ModType::Inc, "EffectOfBonusesFromQuiver")? > 0.0)
        {
            let tree_inc = list::sum(input.node_mods, ModType::Inc, "EffectOfBonusesFromQuiver")?;
            let inc = match items.get("Weapon 1") {
                Some(w) => {
                    list::sum(&w.base_mod_list, ModType::Inc, "EffectOfBonusesFromQuiver")?
                        + tree_inc
                }
                None => 100.0,
            };
            let widowhail = 1.0 + inc / 100.0;
            scale *= widowhail;
            out.player_mods.push(Mod::new(
                "WidowHailMultiplier",
                ModType::Base,
                widowhail,
                Some("Widowhail"),
            ));
            let mut combined = Vec::new();
            for m in src {
                merge_mod(&mut combined, m.clone())?;
            }
            out.scale_add_list(&combined, scale, p);
        } else if let Some(effect) = out_player_mult(
            &out,
            &format!("Corrupted{}JewelEffect", title_case(&item.rarity)),
        ) && typ(item) == "Jewel"
            && item.corrupted
            && slot.node_id.is_some()
            && sub_type(item) != Some("Charm")
        {
            scale += effect;
            let mut combined = Vec::new();
            for m in src {
                merge_mod(&mut combined, m.clone())?;
            }
            out.scale_add_list(&combined, scale, p);
        } else if let Some(name) = match typ(item) {
            "Gloves" => Some("EffectOfBonusesFromGloves"),
            "Boots" => Some("EffectOfBonusesFromBoots"),
            _ => None,
        } && calc_mod(input.node_mods, name)? != 1.0
        {
            let scale = calc_mod(input.node_mods, name)? - 1.0;
            let mut combined = Vec::new();
            for m in src {
                merge_mod(&mut combined, m.clone())?;
            }
            let scaled: Vec<Mod> = combined.iter().map(|m| scale_mod(m, scale, p)).collect();
            for m in &scaled {
                merge_additive(&mut combined, m)?;
            }
            for m in combined {
                out.item_db.add(m);
            }
        } else {
            out.scale_add_list(src, scale, p);
        }
        if let Some(class) = &item.class_restriction {
            let key = squash(item.title.as_deref().unwrap_or(""));
            out.set_cond(&key);
            out.class_restrictions.insert(key, class.clone());
        }
        if !matches!(typ(item), "Jewel" | "Flask" | "Tincture" | "Graft") {
            count_item(&mut out, input, item, slot_name)?;
        }
    }
    for (color, option) in [
        ("Red", "overrideEmptyRedSockets"),
        ("Green", "overrideEmptyGreenSockets"),
        ("Blue", "overrideEmptyBlueSockets"),
        ("White", "overrideEmptyWhiteSockets"),
    ] {
        if let Some(v) = input.config.get(option).filter(|v| v.truthy()) {
            out.set_mult(&format!("Empty{color}SocketsInAnySlot"), v.num());
        }
    }
    Ok(out)
}

fn out_player_mult(out: &ItemsSetup, name: &str) -> Option<f64> {
    out.player_multipliers.get(&Sym::new(name)).copied()
}

/// The item counts and the socket multipliers of an equipped item
fn count_item(
    out: &mut ItemsSetup,
    input: &ItemsInput,
    item: &Item,
    slot_name: &str,
) -> Result<()> {
    let key = match item.rarity.as_str() {
        "UNIQUE" | "RELIC" => {
            if item.foulborn {
                out.add_mult("FoulbornUniqueItem", 1.0);
            }
            "UniqueItem"
        }
        "RARE" => "RareItem",
        "MAGIC" => "MagicItem",
        _ => "NormalItem",
    };
    out.add_mult(key, 1.0);
    out.set_cond(&format!("{key}In{slot_name}"));
    let mut props: Vec<(&str, bool)> = INFLUENCE_MULTS
        .iter()
        .map(|&(m, i)| (m, item.influences[i]))
        .collect();
    props.push(("CorruptedItem", item.corrupted));
    for (mult, has) in props {
        if has {
            out.add_mult(mult, 1.0);
        } else {
            out.add_mult(&format!("Non{mult}"), 1.0);
        }
    }
    if item.influences[0] || item.influences[1] {
        out.add_mult("ShaperOrElderItem", 1.0);
    }
    let kind = pat::gsub(&squash(typ(item)), ".+Handed", "");
    out.add_mult(&format!("{kind}Item"), 1.0);
    if typ(item) == "Ring" {
        out.add_mult(&format!("{}Equipped", squash(base_name(item))), 1.0);
    }
    // The gems socketed in the slot
    let gems: Vec<_> = input
        .socket_groups
        .iter()
        .filter(|g| g.source.is_none() && g.enabled && g.slot.as_deref() == Some(slot_name))
        .flat_map(|g| g.gem_list.iter())
        .filter(|g| g.gem.is_some() && g.enabled)
        .collect();
    let mut empty: HashMap<char, f64> = HashMap::new();
    let mut gem_sockets = 0usize;
    for (i, socket) in item.sockets.iter().enumerate() {
        if matches!(socket.color, 'R' | 'G' | 'B' | 'W') {
            gem_sockets += 1;
            if i + 1 > gems.len() {
                *empty.entry(socket.color).or_insert(0.0) += 1.0;
            }
        }
    }
    let socketed = gem_sockets.min(gems.len());
    let mut colours = [0.0; 3];
    for gem in &gems[..socketed] {
        let tags = &gem.gem.as_ref().unwrap().tags;
        for (i, tag) in ["strength", "dexterity", "intelligence"].iter().enumerate() {
            if tags.contains(*tag) {
                colours[i] += 1.0;
            }
        }
    }
    let e = |c: char| empty.get(&c).copied().unwrap_or(0.0);
    out.set_mult(&format!("SocketedGemsIn{slot_name}"), socketed as f64);
    out.set_mult(&format!("SocketedRedGemsIn{slot_name}"), colours[0]);
    out.set_mult(&format!("SocketedGreenGemsIn{slot_name}"), colours[1]);
    out.set_mult(&format!("SocketedBlueGemsIn{slot_name}"), colours[2]);
    out.set_mult(
        &format!("EmptySocketIn{slot_name}"),
        (gem_sockets as f64).min(e('R') + e('G') + e('B') + e('W')),
    );
    out.add_mult("EmptyRedSocketsInAnySlot", e('R'));
    out.add_mult("EmptyGreenSocketsInAnySlot", e('G'));
    out.add_mult("EmptyBlueSocketsInAnySlot", e('B'));
    out.add_mult("EmptyWhiteSocketsInAnySlot", e('W'));
    Ok(())
}
