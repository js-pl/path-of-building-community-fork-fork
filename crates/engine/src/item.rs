//! Items: upstream's `Item` class (`Classes/Item.lua`) without its crafting
//! and text building. Parses an item's text into its base, properties and
//! mod lines, and builds the mods it grants in each slot it can occupy.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// `m_max(m_min(x, hi), lo)` as upstream writes it, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::item_data::{Affix, AffixSet, Base, ItemData, normalise_mod_line};
use crate::item_tools::{Range, apply_range, is_zero_value_line, round};
use crate::mod_parser::ModParser;
use crate::mods::convert::Converter;
use crate::mods::store::{Actors, Cfg, Query, Store, Stores};
use crate::mods::tag::{ItemCondition, Names, Tag, TagKind};
use crate::mods::{Mod, ModType, ModValue, Record, Sym};
use crate::pat::{self, capture, captures, gmatch, gmatch1, gsub, gsub_fn, is_match, num};
use lua::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

/// `ModFlag.Attack`, `ModFlag.Spell`, `KeywordFlag.Attack`
const MOD_FLAG_ATTACK: u32 = 0x1;
const MOD_FLAG_SPELL: u32 = 0x2;
const KEYWORD_FLAG_ATTACK: u32 = 0x10000;

const DMG_TYPES: [&str; 5] = ["Physical", "Lightning", "Cold", "Fire", "Chaos"];
const CATALYSTS: [&str; 12] = [
    "Abrasive",
    "Accelerating",
    "Dextral",
    "Fertile",
    "Imbued",
    "Intrinsic",
    "Noxious",
    "Prismatic",
    "Sinistral",
    "Tempering",
    "Turbulent",
    "Unstable",
];
const CATALYST_DESCRIPTORS: [&str; 12] = [
    "Attack",
    "Speed",
    "Suffix",
    "Life and Mana",
    "Caster",
    "Attribute",
    "Physical and Chaos",
    "Resistance",
    "Prefix",
    "Defence",
    "Elemental",
    "Critical",
];
const CATALYST_TAGS: [&[&str]; 12] = [
    &["attack"],
    &["speed"],
    &["suffix"],
    &["life", "mana", "resource"],
    &["caster"],
    &["jewellery_attribute", "attribute"],
    &["physical_damage", "chaos_damage"],
    &["jewellery_resistance", "resistance"],
    &["prefix"],
    &[
        "jewellery_defense",
        "defences",
        "armour",
        "evasion",
        "energyshield",
    ],
    &["jewellery_elemental", "elemental_damage"],
    &["critical"],
];

/// Influences (`itemLib.influenceInfo.all`): key and display name. The
/// first six are `influenceInfo.default`.
pub const INFLUENCES: [(&str, &str); 8] = [
    ("shaper", "Shaper"),
    ("elder", "Elder"),
    ("adjudicator", "Warlord"),
    ("basilisk", "Hunter"),
    ("crusader", "Crusader"),
    ("eyrie", "Redeemer"),
    ("cleansing", "Searing Exarch"),
    ("tangle", "Eater of Worlds"),
];

/// What item parsing needs from outside.
pub struct ItemContext<'a> {
    pub data: &'a ItemData,
    pub parser: &'a ModParser,
    /// The labels of `data.jewelRadius`, the jewel radii of the build's tree
    /// version
    pub jewel_radius: &'a [String],
    /// `main.defaultItemAffixQuality`
    pub default_affix_quality: f64,
}

/// The flags a mod line can carry (`lineFlags` of `Classes/Item.lua`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineFlags {
    pub crafted: bool,
    pub crucible: bool,
    pub custom: bool,
    pub disabled: bool,
    pub eater: bool,
    pub enchant: bool,
    pub exarch: bool,
    pub fractured: bool,
    pub implicit: bool,
    pub scourge: bool,
    pub synthesis: bool,
    pub mutated: bool,
    pub unscalable: bool,
    pub prefix: bool,
    pub suffix: bool,
    pub unveiled: bool,
    pub vestigial: bool,
}

impl LineFlags {
    pub const NAMES: [&'static str; 17] = [
        "crafted",
        "crucible",
        "custom",
        "disabled",
        "eater",
        "enchant",
        "exarch",
        "fractured",
        "implicit",
        "scourge",
        "synthesis",
        "mutated",
        "unscalable",
        "prefix",
        "suffix",
        "unveiled",
        "vestigial",
    ];

    fn slot(&mut self, name: &str) -> Option<&mut bool> {
        Some(match name {
            "crafted" => &mut self.crafted,
            "crucible" => &mut self.crucible,
            "custom" => &mut self.custom,
            "disabled" => &mut self.disabled,
            "eater" => &mut self.eater,
            "enchant" => &mut self.enchant,
            "exarch" => &mut self.exarch,
            "fractured" => &mut self.fractured,
            "implicit" => &mut self.implicit,
            "scourge" => &mut self.scourge,
            "synthesis" => &mut self.synthesis,
            "mutated" => &mut self.mutated,
            "unscalable" => &mut self.unscalable,
            "prefix" => &mut self.prefix,
            "suffix" => &mut self.suffix,
            "unveiled" => &mut self.unveiled,
            "vestigial" => &mut self.vestigial,
            _ => return None,
        })
    }

    /// Sets the flag `name`; false when there is no such flag
    pub fn set(&mut self, name: &str) -> bool {
        self.slot(name).map(|f| *f = true).is_some()
    }

    pub fn get(&self, name: &str) -> bool {
        self.clone().slot(name).is_some_and(|f| *f)
    }
}

/// A mod line of an item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModLine {
    pub line: String,
    pub mod_list: Vec<Mod>,
    /// The part of the line the parser did not understand
    pub extra: Option<String>,
    /// Where the line's ranges roll, 0 to 1
    pub range: Option<f64>,
    pub corrupted_range: Option<f64>,
    pub value_scalar: Option<f64>,
    pub display_value_scalar: Option<f64>,
    pub variant_list: Option<BTreeSet<i64>>,
    pub version_list: Option<BTreeSet<i64>>,
    pub variant_group_list: Option<BTreeSet<i64>>,
    pub mod_tags: Vec<String>,
    pub mod_group: Option<String>,
    pub flags: LineFlags,
    /// The stat order, for sorting crafted lines
    pub order: Option<f64>,
    /// A Foulborn transformation: the line's mod and the one it turns into
    pub mod_id: Option<String>,
    pub new_mod_id: Option<String>,
    pub show_slider: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Socket {
    /// `R`, `G`, `B`, `W` or `A`
    pub color: char,
    pub group: f64,
}

/// An affix slot of a crafted item.
#[derive(Clone, Debug, PartialEq)]
pub struct AffixSlot {
    pub mod_id: String,
    pub range: Option<Range>,
    pub fractured: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Affixes {
    pub list: Vec<AffixSlot>,
    /// The slots allowed, when a mod line changes it
    pub limit: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Requirements {
    pub level: Option<f64>,
    pub str: f64,
    pub dex: f64,
    pub int: f64,
    pub str_mod: f64,
    pub dex_mod: f64,
    pub int_mod: f64,
}

/// A mod line that scales other lines' magnitudes
#[derive(Clone, Debug, PartialEq)]
pub struct ModMagnitude {
    pub tags: Vec<String>,
    pub any_tags: Option<[String; 2]>,
    pub quality: f64,
    pub multiplier: Option<f64>,
    pub mod_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GrantedSkill {
    pub skill_id: Option<Rc<str>>,
    pub level: Option<f64>,
    pub no_supports: bool,
    pub source: String,
    pub triggered: bool,
    pub trigger_chance: Option<f64>,
}

/// Named values, as upstream's `weaponData`, `armourData`, `flaskData`,
/// `tinctureData` and `jewelData` tables: computed ones and those mods set
/// by key
pub type Fields = BTreeMap<String, ModValue>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub id: Option<f64>,
    pub raw: String,
    pub raw_lines: Vec<String>,
    pub name: String,
    pub name_prefix: String,
    pub name_suffix: String,
    pub title: Option<String>,
    pub base_name: Option<String>,
    pub base: Option<Rc<Base>>,
    pub typ: Option<String>,
    pub rarity: String,
    pub quality: Option<f64>,
    pub crafted_quality: Option<f64>,
    pub unique_id: Option<String>,
    pub item_level: Option<f64>,
    pub memory_strands: Option<f64>,
    pub class_restriction: Option<String>,
    pub catalyst: Option<usize>,
    pub catalyst_quality: Option<f64>,
    pub talisman_tier: Option<f64>,
    pub intangibility: Option<f64>,
    pub league: Option<String>,
    pub source: Option<String>,
    pub note: Option<String>,
    pub upgrade_paths: Vec<String>,
    pub unreleased: bool,
    pub hidden_specs: bool,
    pub foil_type: Option<String>,

    pub split: bool,
    pub mirrored: bool,
    pub corrupted: bool,
    pub fractured: bool,
    pub synthesised: bool,
    pub veiled: bool,
    pub crafted: bool,
    pub scourge: bool,
    pub crucible: bool,
    pub implicit: bool,
    /// By `INFLUENCES` index
    pub influences: [bool; 8],
    pub foulborn: bool,
    pub vestigial: bool,
    pub advanced_copy: bool,
    pub corruptible: bool,
    pub can_be_influenced: bool,

    pub implicits_cannot_be_changed: bool,
    pub can_be_anointed: bool,
    pub can_have_two_enchants: bool,
    pub can_have_three_enchants: bool,
    pub can_have_four_enchants: bool,
    pub has_elder_shaper_and_all_conqueror_influences: bool,
    pub can_have_eldritch_influence: bool,
    pub can_have_only_support_skills_crucible_tree: bool,
    pub can_have_shield_crucible_tree: bool,
    pub can_have_two_handed_sword_crucible_tree: bool,
    pub restrict_tag: bool,
    pub no_caster: bool,
    pub no_attack: bool,
    pub restrict_damage_type: bool,
    /// `onlyFireDamage` and the like, by element
    pub only_damage: HashSet<String>,

    pub jewel_radius_label: Option<String>,
    /// 1-based index into the jewel radii
    pub jewel_radius_index: Option<f64>,
    pub limit: Option<f64>,
    pub cluster_jewel_skill: Option<String>,
    pub cluster_jewel_node_count: Option<f64>,

    pub variant: Option<f64>,
    /// `variantAlt` to `variantAlt5`
    pub variant_alt: [Option<f64>; 5],
    /// `hasAltVariant` to `hasAltVariant5`
    pub has_alt_variant: [bool; 5],
    pub variant_list: Option<Vec<String>>,
    pub version_list: Option<Vec<String>>,
    pub selected_version: Option<f64>,
    pub allow_duplicate_variants: bool,
    /// Group to variant to the versions it is in (0: all)
    pub variant_groups: BTreeMap<i64, BTreeMap<i64, BTreeSet<i64>>>,
    pub variant_group_selections: BTreeMap<i64, f64>,
    pub uses_variant_groups: bool,

    pub sockets: Vec<Socket>,
    pub colour_always_matches: bool,
    pub default_socket_color: Option<char>,
    pub abyssal_socket_count: f64,
    pub selectable_socket_count: f64,
    pub socketed_jewel_effect_modifier: f64,

    pub requirements: Requirements,
    pub affixes: Option<Rc<AffixSet>>,
    pub prefixes: Affixes,
    pub suffixes: Affixes,
    pub affix_limit: f64,
    /// `data.rareLikeUniques` key of the item, when it is one
    pub rare_like_unique: Option<String>,
    pub mutated_lines: Option<Vec<(String, String)>>,
    pub mod_magnitude_mods: Vec<ModMagnitude>,

    pub class_requirement_mod_lines: Vec<ModLine>,
    pub buff_mod_lines: Vec<ModLine>,
    pub enchant_mod_lines: Vec<ModLine>,
    pub scourge_mod_lines: Vec<ModLine>,
    pub implicit_mod_lines: Vec<ModLine>,
    pub explicit_mod_lines: Vec<ModLine>,
    pub crucible_mod_lines: Vec<ModLine>,
    pub has_mod_tags: bool,

    pub mod_source: String,
    pub base_mod_list: Vec<Mod>,
    pub buff_mod_list: Vec<Mod>,
    /// The mods of an item with one slot
    pub mod_list: Option<Vec<Mod>>,
    /// The mods of a weapon or ring in each slot (1-based: index 0 is slot 1)
    pub slot_mod_list: Option<Vec<Vec<Mod>>>,
    pub granted_skills: Vec<GrantedSkill>,
    /// By slot number, 1-based
    pub weapon_data: Option<BTreeMap<usize, Fields>>,
    pub armour_data: Option<Fields>,
    pub flask_data: Option<Fields>,
    pub tincture_data: Option<Fields>,
    pub jewel_data: Option<Fields>,
}

/// Which mod line list a line goes to
#[derive(Copy, Clone, PartialEq)]
enum Lines {
    Enchant,
    Scourge,
    ClassRequirement,
    Implicit,
    Crucible,
    Explicit,
}

#[derive(Copy, Clone, PartialEq)]
enum Stage {
    FindImplicit,
    Implicit,
    FindExplicit,
    Explicit,
    Done,
}

#[derive(Copy, Clone, PartialEq)]
enum AffixKind {
    Prefix,
    Suffix,
}

/// `specToNumber`: the number a property value starts with
fn spec_to_number(s: &str) -> Option<f64> {
    capture(s, "^([%+%-]?[%d%.]+)").and_then(|n| pat::to_number(&n))
}

fn tonum(s: &str) -> Result<f64> {
    pat::to_number(s)
        .ok_or_else(|| format!("attempt to perform arithmetic on a string value ({s:?})"))
}

/// Whether a number is a key of an integer set (`t[n]` for a number `n`)
fn has(set: &BTreeSet<i64>, n: Option<f64>) -> bool {
    n.is_some_and(|n| n.fract() == 0.0 && set.contains(&(n as i64)))
}

/// `escapeGGGString` of `Modules/Common.lua`
fn escape_ggg(text: &str) -> String {
    let l = gsub(text, "<[^>]+>{([^}]+)}", "%1");
    let l = gsub(&l, "%[([^|%]]+)%]", "%1");
    gsub(&l, "%[[^|]+|([^|]+)%]", "%1")
}

/// `sanitiseText` of `Modules/Common.lua`
pub fn sanitise_text(text: &str) -> String {
    let v = crate::common::sanitise_text(&Value::str(text));
    v.as_str()
        .map(|s| s.to_str_lossy().into_owned())
        .unwrap_or_default()
}

/// `getCatalystScalar`
fn catalyst_scalar(catalyst: Option<usize>, line: &ModLine, quality: Option<f64>) -> f64 {
    if line.flags.unscalable {
        return 1.0;
    }
    let Some(tags) = catalyst.and_then(|c| CATALYST_TAGS.get(c.wrapping_sub(1))) else {
        return 1.0;
    };
    if line.mod_tags.is_empty() {
        return 1.0;
    }
    let quality = quality.unwrap_or(20.0);
    let has = |t: &str| {
        line.mod_tags.iter().any(|m| m == t)
            || (t == "prefix" && line.flags.prefix)
            || (t == "suffix" && line.flags.suffix)
    };
    if tags.iter().any(|t| has(t)) {
        (100.0 + quality) / 100.0
    } else {
        1.0
    }
}

/// No actors: the queries of an item's own mod lists
struct NoActors;

impl Actors for NoActors {
    fn actor_store(&self, _: usize, _: Sym) -> Option<usize> {
        None
    }
    fn output(&self, _: usize, _: Sym) -> Option<f64> {
        None
    }
    fn special_stat(&self, _: usize, _: Sym, _: Option<&Cfg>) -> Option<f64> {
        None
    }
    fn one_handed_added(&self, _: usize, _: Sym) -> Option<bool> {
        None
    }
    fn monster_tags(&self, _: usize) -> Option<Vec<Rc<str>>> {
        None
    }
    fn item_condition(&self, _: usize, _: &ItemCondition, _: Option<&Cfg>) -> bool {
        false
    }
    fn gem_is_type(&self, _: &Cfg, _: &str) -> bool {
        false
    }
    fn game_id(&self, _: &str) -> Option<Rc<str>> {
        None
    }
    fn high_precision(&self, _: Sym, _: ModType) -> Option<f64> {
        None
    }
}

/// `modList:List(nil, name)` over a plain mod list
fn list_values(mods: &[Mod], name: &str) -> Result<Vec<ModValue>> {
    let mut store = Store::list();
    for m in mods {
        store.add(m.clone());
    }
    let mut stores = Stores::default();
    let id = stores.push(store);
    let q = Query {
        stores: &stores,
        actors: &NoActors,
    };
    q.list(id, None, &[Sym::new(name)])
}

/// `calcLocal`: sums the local mods of a name, type and flags (no tags but
/// `InSlot`, no keyword flags) and removes them from the list
fn calc_local(mods: &mut Vec<Mod>, name: &str, typ: ModType, flags: u32) -> Result<ModValue> {
    let name = Sym::new(name);
    let in_slot = Sym::new("InSlot");
    let mut result = match typ {
        ModType::Flag => ModValue::Bool(false),
        ModType::More => ModValue::Number(1.0),
        _ => ModValue::Number(0.0),
    };
    let mut i = 0;
    while i < mods.len() {
        let m = &mods[i];
        let local_tags = match m.tags.first() {
            None => true,
            Some(t) => matches!(&t.kind, TagKind::Other { kind, .. } if *kind == in_slot),
        };
        if m.name == name && m.typ == typ && m.flags == flags && m.keyword_flags == 0 && local_tags
        {
            result = match (typ, result) {
                (ModType::Flag, r) => {
                    if r.truthy() {
                        r
                    } else {
                        m.value.clone()
                    }
                }
                (ModType::More, ModValue::Number(r)) => {
                    ModValue::Number(r * ((100.0 + value_num(&m.value)?) / 100.0))
                }
                (_, ModValue::Number(r)) => ModValue::Number(r + value_num(&m.value)?),
                (_, r) => return Err(format!("calcLocal of {r:?}")),
            };
            mods.remove(i);
        } else {
            i += 1;
        }
    }
    Ok(result)
}

fn value_num(v: &ModValue) -> Result<f64> {
    match v {
        ModValue::Number(n) => Ok(*n),
        ModValue::Str(s) => crate::mods::store::coerce(s)
            .ok_or_else(|| format!("attempt to perform arithmetic on a string value ({s:?})")),
        v => Err(format!("attempt to perform arithmetic on {v:?}")),
    }
}

fn calc_num(mods: &mut Vec<Mod>, name: &str, typ: ModType, flags: u32) -> Result<f64> {
    value_num(&calc_local(mods, name, typ, flags)?)
}

fn calc_flag(mods: &mut Vec<Mod>, name: &str) -> Result<bool> {
    Ok(calc_local(mods, name, ModType::Flag, 0)?.truthy())
}

/// `modLib.createMod(name, type, value, source, tags...)`
fn new_mod(name: &str, typ: ModType, value: ModValue, source: &str, tags: Vec<Tag>) -> Mod {
    Mod {
        name: Sym::new(name),
        typ,
        value,
        flags: 0,
        keyword_flags: 0,
        source: Some(source.into()),
        source_slot: None,
        tags,
        unreached_tags: vec![],
    }
}

fn condition(var: &str, neg: bool) -> Tag {
    Tag {
        kind: TagKind::Condition {
            vars: Names::One(Sym::new(var)),
            neg,
            actor: None,
        },
        global_limit: None,
    }
}

/// `modLib.setSource`: the mod and the mod it carries get the source
fn set_source(m: &mut Mod, source: &str) {
    m.source = Some(source.into());
    if let ModValue::Record(r) = &mut m.value
        && let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod"))
    {
        inner.source = Some(source.into());
    }
}

fn record_field<'r>(r: &'r Record, name: &str) -> Option<&'r ModValue> {
    r.get(name)
}

/// Sets `t[value.key] = value.value` for each `{ key, value }` record
fn set_keyed(fields: &mut Fields, values: Vec<ModValue>) -> Result<()> {
    for v in values {
        let ModValue::Record(r) = v else {
            return Err(format!("keyed value {v:?}"));
        };
        let key = match record_field(&r, "key") {
            Some(ModValue::Str(k)) => k.to_string(),
            k => return Err(format!("key {k:?}")),
        };
        match record_field(&r, "value") {
            Some(v) => {
                fields.insert(key, v.clone());
            }
            None => {
                fields.remove(&key);
            }
        }
    }
    Ok(())
}

fn field_num(fields: &Fields, key: &str) -> Option<f64> {
    fields.get(key).and_then(ModValue::as_number)
}

fn n(x: f64) -> ModValue {
    ModValue::Number(x)
}

impl Item {
    /// `new("Item", raw, rarity, highQuality)`
    pub fn new(
        ctx: &ItemContext,
        raw: &str,
        rarity: Option<&str>,
        high_quality: bool,
    ) -> Result<Item> {
        let mut item = Item::default();
        item.parse_raw(ctx, &sanitise_text(raw), rarity, high_quality)?;
        Ok(item)
    }

    fn lines_mut(&mut self, which: Lines) -> &mut Vec<ModLine> {
        match which {
            Lines::Enchant => &mut self.enchant_mod_lines,
            Lines::Scourge => &mut self.scourge_mod_lines,
            Lines::ClassRequirement => &mut self.class_requirement_mod_lines,
            Lines::Implicit => &mut self.implicit_mod_lines,
            Lines::Crucible => &mut self.crucible_mod_lines,
            Lines::Explicit => &mut self.explicit_mod_lines,
        }
    }

    /// `IsVariantGroupOptionEligible`
    fn variant_group_option_eligible(&self, group: i64, variant: i64) -> bool {
        self.variant_groups
            .get(&group)
            .and_then(|g| g.get(&variant))
            .is_some_and(|versions| {
                versions.contains(&0)
                    || self
                        .selected_version
                        .is_some_and(|v| v.fract() == 0.0 && versions.contains(&(v as i64)))
            })
    }

    /// `GetVariantGroupOptions(groupId, excludeSelected)`
    pub fn variant_group_options(&self, group: i64, exclude_selected: bool) -> Vec<i64> {
        if !self.variant_groups.contains_key(&group) {
            return vec![];
        }
        let mut used = HashSet::new();
        if exclude_selected {
            for &other in self.variant_groups.keys() {
                if other != group
                    && let Some(&v) = self.variant_group_selections.get(&other)
                    && v.fract() == 0.0
                    && self.variant_group_option_eligible(other, v as i64)
                {
                    used.insert(v as i64);
                }
            }
        }
        let count = self.variant_list.as_ref().map_or(0, Vec::len) as i64;
        (1..=count)
            .filter(|v| self.variant_group_option_eligible(group, *v) && !used.contains(v))
            .collect()
    }

    /// `NormaliseVariantSelections`
    fn normalise_variant_selections(&mut self) {
        match &self.version_list {
            Some(list) if !list.is_empty() => {
                let len = list.len() as f64;
                self.selected_version =
                    Some(1f64.max(len.min(self.selected_version.unwrap_or(len))));
            }
            _ => self.selected_version = None,
        }
        let groups = &self.variant_groups;
        self.variant_group_selections
            .retain(|g, _| groups.contains_key(g));
        let mut used = HashSet::new();
        let mut needs = Vec::new();
        for &group in self.variant_groups.keys() {
            if !self.variant_group_options(group, false).is_empty() {
                match self.variant_group_selections.get(&group) {
                    Some(&s)
                        if s.fract() == 0.0
                            && self.variant_group_option_eligible(group, s as i64)
                            && !used.contains(&(s as i64)) =>
                    {
                        used.insert(s as i64);
                    }
                    _ => needs.push(group),
                }
            }
        }
        for group in needs {
            let selected = self
                .variant_group_options(group, false)
                .into_iter()
                .find(|v| !used.contains(v));
            match selected {
                Some(v) => {
                    self.variant_group_selections.insert(group, v as f64);
                    used.insert(v);
                }
                None => {
                    self.variant_group_selections.remove(&group);
                }
            }
        }
    }

    /// `CheckModLineVariant`
    pub fn check_mod_line_variant(&self, line: &ModLine) -> bool {
        self.check_variant(
            &line.variant_list,
            &line.version_list,
            &line.variant_group_list,
        )
    }

    fn check_variant(
        &self,
        variants: &Option<BTreeSet<i64>>,
        versions: &Option<BTreeSet<i64>>,
        groups: &Option<BTreeSet<i64>>,
    ) -> bool {
        if self.uses_variant_groups {
            if let Some(versions) = versions
                && !has(versions, self.selected_version)
            {
                return false;
            }
            if let Some(groups) = groups {
                let Some(variants) = variants else {
                    return false;
                };
                return groups
                    .iter()
                    .any(|g| has(variants, self.variant_group_selections.get(g).copied()));
            }
            return variants.is_none();
        }
        let Some(variants) = variants else {
            return true;
        };
        has(variants, self.variant)
            || (0..5).any(|i| self.has_alt_variant[i] && has(variants, self.variant_alt[i]))
    }

    /// `GetModLineVariantCount`
    pub fn mod_line_variant_count(&self, line: &ModLine) -> usize {
        let Some(variants) = line
            .variant_list
            .as_ref()
            .filter(|_| self.allow_duplicate_variants)
        else {
            return self.check_mod_line_variant(line) as usize;
        };
        let mut count = has(variants, self.variant) as usize;
        for i in 0..5 {
            if self.has_alt_variant[i]
                && self.variant_alt[i].is_some()
                && has(variants, self.variant_alt[i])
            {
                count += 1;
            }
        }
        count
    }

    /// `NormaliseQuality`
    fn normalise_quality(&mut self) {
        let Some(base) = &self.base else {
            return;
        };
        if base.armour.is_some()
            || base.weapon.is_some()
            || base.flask.is_some()
            || base.tincture.is_some()
        {
            match self.quality {
                None => self.quality = Some(0.0),
                Some(q)
                    if self.unique_id.is_none()
                        && !self.corrupted
                        && !self.split
                        && !self.mirrored
                        && q < 20.0 =>
                {
                    self.quality = Some(20.0)
                }
                _ => {}
            }
        }
    }

    /// `GetModSpawnWeight(mod, includeTags, excludeTags, baseTags)`
    fn mod_spawn_weight(
        &self,
        m: &Affix,
        include: &HashSet<String>,
        exclude: &HashSet<String>,
        base_tags: Option<&HashSet<String>>,
    ) -> f64 {
        let Some(base) = &self.base else {
            return 0.0;
        };
        let base_tags = base_tags.unwrap_or(&base.tags);
        let has_influence_tag = |key: &str| {
            base.influence_tags.as_ref().is_some_and(|tags| {
                INFLUENCES
                    .iter()
                    .enumerate()
                    .any(|(i, (k, _))| self.influences[i] && tags.get(*k).is_some_and(|t| t == key))
            })
        };
        if self.restrict_tag {
            for key in &m.mod_tags {
                let mut c = key.chars();
                let flag = match c.next() {
                    Some(f) if f.is_ascii_lowercase() => {
                        format!("{}{}", f.to_ascii_uppercase(), c.as_str())
                    }
                    _ => key.clone(),
                };
                if (flag == "Caster" && self.no_caster) || (flag == "Attack" && self.no_attack) {
                    return 0.0;
                }
            }
        }
        if self.restrict_damage_type {
            let elements = ["fire", "cold", "lightning", "chaos", "physical"];
            let required = elements.iter().any(|e| self.only_damage.contains(*e));
            if required
                && m.mod_tags
                    .iter()
                    .any(|t| elements.contains(&t.as_str()) && !self.only_damage.contains(t))
            {
                return 0.0;
            }
        }
        let mut weight = 0.0;
        for (i, key) in m.weight_key.iter().enumerate() {
            if (base_tags.contains(key) || include.contains(key) || has_influence_tag(key))
                && !exclude.contains(key)
            {
                let maven = m.affix.as_deref().is_some_and(|a| a.contains("Elevated"));
                weight = if has_influence_tag(key) && maven {
                    1000.0
                } else {
                    m.weight_val.get(i).copied().unwrap_or(f64::NAN)
                };
                break;
            }
        }
        for (i, key) in m.weight_multiplier_key.iter().enumerate() {
            if (base_tags.contains(key) || include.contains(key) || has_influence_tag(key))
                && !exclude.contains(key)
            {
                weight =
                    weight * m.weight_multiplier_val.get(i).copied().unwrap_or(f64::NAN) / 100.0;
                break;
            }
        }
        weight
    }

    /// `CanHaveMod(mod)`
    fn can_have_mod(&self, data: &ItemData, m: &Affix) -> bool {
        let mut include = HashSet::new();
        let title = self.title.as_deref().unwrap_or("");
        if data.caster_tag_crucible_uniques.contains(title) {
            include.insert("caster_unique_weapon".to_string());
        }
        if data.minion_tag_crucible_uniques.contains(title) {
            include.insert("minion_unique_weapon".to_string());
        }
        let none = HashSet::new();
        if let Some(valid) = self
            .rare_like_unique
            .as_ref()
            .and_then(|r| data.rare_like_uniques[r].valid_base_tags.as_ref())
        {
            return valid
                .iter()
                .any(|tags| self.mod_spawn_weight(m, &include, &none, Some(tags)) > 0.0);
        }
        let set = |tags: &[&str]| tags.iter().map(|t| t.to_string()).collect::<HashSet<_>>();
        if self.can_have_only_support_skills_crucible_tree {
            return m
                .weight_key
                .iter()
                .rposition(|k| k == "crucible_unique_staff")
                .is_some_and(|i| m.weight_val.get(i) != Some(&0.0));
        } else if self.can_have_shield_crucible_tree {
            return self.mod_spawn_weight(
                m,
                &set(&["crucible_unique_helmet", "shield"]),
                &none,
                None,
            ) > 0.0;
        } else if self.can_have_two_handed_sword_crucible_tree {
            return self.mod_spawn_weight(
                m,
                &set(&["two_hand_weapon"]),
                &set(&["one_hand_weapon"]),
                None,
            ) > 0.0;
        }
        self.mod_spawn_weight(m, &include, &none, None) > 0.0
    }

    /// Parses a mod line: its mods (none when the parser does not know it)
    /// and the part not understood
    fn parse_mod(ctx: &ItemContext, line: &str) -> Result<(Option<Vec<Mod>>, Option<String>)> {
        let (list, extra) = ctx.parser.parse(line.as_bytes());
        let conv = Converter::default();
        let list = match list {
            Value::Table(t) => Some(
                t.array()
                    .iter()
                    .map(|m| conv.mod_(m))
                    .collect::<Result<Vec<_>>>()?,
            ),
            _ => None,
        };
        let extra = match extra {
            Value::Str(s) => Some(s.to_str_lossy().into_owned()),
            _ => None,
        };
        Ok((list, extra))
    }

    /// `ParseRaw(raw, rarity, highQuality)`: parses an item's text.
    pub fn parse_raw(
        &mut self,
        ctx: &ItemContext,
        raw: &str,
        rarity: Option<&str>,
        high_quality: bool,
    ) -> Result<()> {
        let data = ctx.data;
        self.raw = raw.to_string();
        self.name = "?".into();
        self.name_prefix = String::new();
        self.name_suffix = String::new();
        self.base = None;
        self.rarity = rarity.unwrap_or("UNIQUE").to_string();
        self.quality = None;
        self.raw_lines = gmatch1(raw, "%s*([^\n]*%S)")
            .iter()
            .map(|l| escape_ggg(l))
            .collect();
        let mut game_mode = rarity.is_some();
        let mut l = 0;
        let mut item_class = None;
        if l < self.raw_lines.len() {
            if is_match(&self.raw_lines[l], "^Item Class:") {
                item_class = Some(gsub(&self.raw_lines[l], "^Item Class: %s+", "%1"));
                l += 1;
            }
            let line = self
                .raw_lines
                .get(l)
                .ok_or("attempt to index a nil value")?;
            if let Some(r) = capture(line, "^Rarity: (%a+)") {
                game_mode = true;
                if data.color_codes.contains(&r.to_ascii_uppercase()) {
                    self.rarity = r.to_ascii_uppercase();
                }
                if self.rarity == "UNIQUE" || self.rarity == "RELIC" {
                    // Hack for relics
                    if let Some(line) = self.raw_lines.iter().find(|l| l.contains("Foil Unique")) {
                        self.rarity = "RELIC".into();
                        self.foil_type =
                            Some(capture(line, "%((.-)%)").unwrap_or_else(|| "Rainbow".into()));
                    }
                }
                l += 1;
            }
        }
        if l < self.raw_lines.len() {
            if self.raw_lines[l] == "--------" {
                l += 1;
            }
            self.name = self.raw_lines.get(l).cloned().unwrap_or_default();
            let unidentified = self.raw_lines.iter().any(|l| l == "Unidentified");
            if !(self.rarity == "NORMAL" || self.rarity == "MAGIC" || unidentified) {
                l += 1;
            }
        }
        let mut check_section = false;
        self.sockets = vec![];
        self.class_requirement_mod_lines = vec![];
        self.buff_mod_lines = vec![];
        self.enchant_mod_lines = vec![];
        self.scourge_mod_lines = vec![];
        self.implicit_mod_lines = vec![];
        self.explicit_mod_lines = vec![];
        self.crucible_mod_lines = vec![];
        self.advanced_copy = false;
        self.mod_magnitude_mods = vec![];
        let mut implicit_lines = 0.0;
        self.variant_list = None;
        self.version_list = None;
        self.allow_duplicate_variants = false;
        self.variant_groups = BTreeMap::new();
        self.uses_variant_groups = false;
        self.prefixes = Affixes::default();
        self.suffixes = Affixes::default();
        self.requirements = Requirements::default();
        self.foulborn = false;
        self.vestigial = false;
        self.mutated_lines = None;
        self.rare_like_unique = None;
        let mut imported_level_req = None;
        let mut flask_buff_lines: Option<HashSet<String>> = None;
        let mut tincture_buff_lines: Option<HashSet<String>> = None;
        let mut defer_jewel_radius = false;
        let mut stage = Stage::FindImplicit;
        let (mut found_explicit, mut found_implicit) = (false, false);
        let mut line_prefix = String::new();
        let mut line_postfix = String::new();
        let mut pending_affixes: Option<Vec<(String, AffixKind)>> = None;

        while l < self.raw_lines.len() {
            let mut line = self.raw_lines[l].clone();
            if line == "Veiled Prefix" || line == "Veiled Suffix" {
                self.veiled = true;
            }
            let influence = INFLUENCES
                .iter()
                .position(|(_, display)| line == format!("{display} Item"));
            // A base's buff lines are its own, not mod lines
            let buff_line = flask_buff_lines.as_mut().is_some_and(|b| b.remove(&line))
                || tincture_buff_lines
                    .as_mut()
                    .is_some_and(|b| b.remove(&line));
            if buff_line {
            } else if line == "--------" {
                line_prefix.clear();
                line_postfix.clear();
                check_section = true;
            } else if line == "Split" {
                self.split = true;
            } else if line == "Mirrored" {
                self.mirrored = true;
            } else if line == "Corrupted" {
                self.corrupted = true;
            } else if line == "Fractured Item" {
                self.fractured = true;
            } else if line == "Synthesised Item" {
                self.synthesised = true;
            } else if line.contains("Foil Unique") {
                self.foil_type =
                    Some(capture(&line, "%((.-)%)").unwrap_or_else(|| "Rainbow".into()));
            } else if let Some(i) = influence {
                self.influences[i] = true;
            } else if line == "Requirements:" {
            } else if is_match(&line, "^%(%a+") || is_match(&line, "^%(%d+%%? of ") {
                // Reminder text
                while l < self.raw_lines.len() && !is_match(&self.raw_lines[l], "%)$") {
                    l += 1;
                }
            } else if self.base.as_ref().is_some_and(|b| b.flask.is_some())
                && (is_match(&line, "^Lasts .+ Seconds$")
                    || is_match(&line, "^Consumes %d+ of %d+ Charges on use$")
                    || is_match(&line, "^Currently has %d+ Charges$"))
            {
                // Flask state, not a mod line
            } else if line.starts_with("{ ") {
                // Advanced copy format
                self.advanced_copy = true;
                line_prefix.clear();
                line_postfix.clear();
                self.crafted = true;
                let (full_name, mod_tags) =
                    if let Some(c) = captures(&line, "^{ (.-) %- (.-)  %- (%d*).*}$") {
                        (c[0].clone(), Some(c[1].clone()))
                    } else if let Some(c) = captures(&line, "^{ (.-) %- (.-) }$") {
                        (c[0].clone(), Some(c[1].clone()))
                    } else if let Some(c) = capture(&line, "^{ (.-) }$") {
                        (c, None)
                    } else {
                        return Err(format!(
                            "attempt to index a nil value (advanced copy line {line:?})"
                        ));
                    };
                let mod_name = capture(&full_name, "^.*Modifier \"(.*)\"");
                match (mod_name.filter(|m| !m.is_empty()), &self.affixes) {
                    (Some(mod_name), Some(affixes)) => {
                        let mut pending = Vec::new();
                        let mut backup = Vec::new();
                        let ignore_type = self
                            .rare_like_unique
                            .as_ref()
                            .is_some_and(|r| data.rare_like_uniques[r].ignore_mod_type);
                        for m in &affixes.list {
                            if m.affix.as_deref() != Some(mod_name.as_str()) {
                                continue;
                            }
                            let target = if self.can_have_mod(data, m) {
                                &mut pending
                            } else {
                                // Conqueror mods can't spawn natively
                                &mut backup
                            };
                            if m.typ.as_deref() == Some("Prefix") || ignore_type {
                                target.push((m.id.clone(), AffixKind::Prefix));
                            } else if m.typ.as_deref() == Some("Suffix") {
                                target.push((m.id.clone(), AffixKind::Suffix));
                            }
                        }
                        if pending.is_empty() && !backup.is_empty() {
                            pending = backup;
                        } else if pending.is_empty() {
                            // A veiled, temple or other custom mod
                            line_prefix = "{custom}".into();
                        }
                        pending_affixes = Some(pending);
                    }
                    _ => {
                        if is_match(&full_name, "(.*)Enhancement.*") {
                            line_postfix = " (enchant)".into();
                        }
                    }
                }
                let flags_part =
                    capture(&gsub(&full_name, "Foulborn", "Mutated"), "(.*)Modifier.*");
                if let Some(part) = flags_part {
                    for flag in gmatch1(&part, "%a+") {
                        let lower = flag.to_ascii_lowercase();
                        if LineFlags::NAMES.contains(&lower.as_str()) {
                            line_prefix.push_str(&format!("{{{lower}}}"));
                        }
                    }
                }
                if full_name.starts_with("Allocated Crucible Passive Skill") {
                    line_prefix.push_str("{crucible}");
                }
                if let Some(tags) = mod_tags.filter(|t| !t.is_empty()) {
                    line_prefix.push_str(&format!(
                        "{{tags:{}}}",
                        gsub(&tags.to_ascii_lowercase(), "%s+", "")
                    ));
                }
            } else {
                line = format!("{line_prefix}{line}{line_postfix}");
                if check_section {
                    match stage {
                        Stage::Implicit => {
                            if found_implicit {
                                stage = Stage::Explicit;
                                found_explicit = true;
                            } else {
                                stage = Stage::FindExplicit;
                            }
                        }
                        Stage::Explicit => stage = Stage::Done,
                        Stage::FindImplicit
                            if self.item_level.is_some()
                                && !is_match(&line, " %(implicit%)")
                                && !is_match(&line, " %(enchant%)")
                                && !line.contains("Talisman Tier") =>
                        {
                            stage = Stage::Explicit;
                            found_explicit = true;
                        }
                        _ => {}
                    }
                    check_section = false;
                }
                let mut spec = captures(&line, "^([%a %(%)]+:?): (.+)$").map(|c| {
                    let name = if c[0] == "Class:" {
                        "Requires Class".to_string()
                    } else {
                        c[0].clone()
                    };
                    (name, c[1].clone())
                });
                if spec.is_none() {
                    spec = captures(&line, "^(Requires %a+) (.+)$")
                        .map(|c| (c[0].clone(), c[1].clone()));
                }
                if let Some((spec_name, spec_val)) = &spec {
                    let mut spec_name = spec_name.clone();
                    let mut spec_val = spec_val.clone();
                    let num = || spec_to_number(&spec_val);
                    match spec_name.as_str() {
                        "Unique ID" => self.unique_id = Some(spec_val.clone()),
                        "Item Level" => self.item_level = num(),
                        "Memory Strands" => self.memory_strands = num(),
                        "Requires Class" => self.class_restriction = Some(spec_val.clone()),
                        _ if is_match(&spec_name, "Quality %([%a%s]+ Modifiers%)") => {
                            self.catalyst_quality =
                                capture(&spec_val, "(%d+)%%").and_then(|v| spec_to_number(&v));
                            let d = capture(&spec_name, "Quality %(([%a%s]+) Modifiers%)");
                            for (i, desc) in CATALYST_DESCRIPTORS.iter().enumerate() {
                                if d.as_deref() == Some(*desc) {
                                    self.catalyst = Some(i + 1);
                                }
                            }
                        }
                        "Quality" => self.quality = num(),
                        "Sockets" => {
                            let mut group = 0.0;
                            for c in spec_val.chars() {
                                if "RGBWA".contains(c) {
                                    self.sockets.push(Socket { color: c, group });
                                } else if c == ' ' {
                                    group += 1.0;
                                }
                            }
                        }
                        "Radius" if self.typ.as_deref() == Some("Jewel") => {
                            let label = capture(&spec_val, "^[%a ]+");
                            self.jewel_radius_label = label.clone();
                            if label.as_deref() == Some("Variable") {
                                // Read from the mods once parsed
                                defer_jewel_radius = true;
                            } else if let Some(i) = ctx
                                .jewel_radius
                                .iter()
                                .position(|r| label.as_deref() == Some(r.as_str()))
                            {
                                self.jewel_radius_index = Some(i as f64 + 1.0);
                            }
                        }
                        "Limited to" if self.typ.as_deref() == Some("Jewel") => self.limit = num(),
                        "Version" => {
                            self.version_list
                                .get_or_insert_with(Vec::new)
                                .push(spec_val.clone());
                            self.uses_variant_groups = true;
                        }
                        "Variant" => {
                            let list = self.variant_list.get_or_insert_with(Vec::new);
                            // Kept for backwards compatibility
                            match captures(&spec_val, "{([%w_]+)}(.+)") {
                                Some(c) => list.push(c[1].clone()),
                                None => list.push(spec_val.clone()),
                            }
                        }
                        "Talisman Tier" => self.talisman_tier = num(),
                        "Armour" | "Evasion Rating" | "Evasion" | "Energy Shield" | "Ward" => {
                            if spec_name == "Evasion Rating" {
                                spec_name = "Evasion".into();
                                if self.base_name.as_deref()
                                    == Some("Two-Toned Boots (Armour/Energy Shield)")
                                {
                                    // Another hack for Two-Toned Boots
                                    self.base_name =
                                        Some("Two-Toned Boots (Armour/Evasion)".into());
                                    self.base =
                                        data.base("Two-Toned Boots (Armour/Evasion)").cloned();
                                }
                            } else if spec_name == "Energy Shield" {
                                spec_name = "EnergyShield".into();
                                if self.base_name.as_deref()
                                    == Some("Two-Toned Boots (Armour/Evasion)")
                                {
                                    // Yet another hack for Two-Toned Boots
                                    self.base_name =
                                        Some("Two-Toned Boots (Evasion/Energy Shield)".into());
                                    self.base = data
                                        .base("Two-Toned Boots (Evasion/Energy Shield)")
                                        .cloned();
                                }
                            }
                            let ad = self.armour_data.get_or_insert_with(Fields::new);
                            match num() {
                                Some(v) => ad.insert(spec_name.clone(), n(v)),
                                None => ad.remove(&spec_name),
                            };
                        }
                        _ if spec_name.contains("BasePercentile") => {
                            let v = num().unwrap_or(0.0);
                            self.armour_data
                                .get_or_insert_with(Fields::new)
                                .insert(spec_name.clone(), n(v));
                        }
                        "Requires Level" => self.requirements.level = num(),
                        // Requirements from imported items can't always be trusted
                        "Level" => imported_level_req = num(),
                        "LevelReq" => self.requirements.level = num(),
                        "Has Alt Variant" => self.has_alt_variant[0] = true,
                        "Has Alt Variant Two" => self.has_alt_variant[1] = true,
                        "Has Alt Variant Three" => self.has_alt_variant[2] = true,
                        "Has Alt Variant Four" => self.has_alt_variant[3] = true,
                        "Has Alt Variant Five" => self.has_alt_variant[4] = true,
                        "Selected Version" => {
                            self.selected_version = num();
                            self.uses_variant_groups = true;
                        }
                        "Selected Variant Group" => {
                            if let Some(c) = captures(&spec_val, "^(%d+)%s*=%s*(%d+)$") {
                                let (g, v) = (tonum(&c[0])?, tonum(&c[1])?);
                                self.variant_group_selections.insert(g as i64, v);
                                self.uses_variant_groups = true;
                            }
                        }
                        "Selected Variant" => self.variant = num(),
                        "Selected Alt Variant" => self.variant_alt[0] = num(),
                        "Selected Alt Variant Two" => self.variant_alt[1] = num(),
                        "Selected Alt Variant Three" => self.variant_alt[2] = num(),
                        "Selected Alt Variant Four" => self.variant_alt[3] = num(),
                        "Selected Alt Variant Five" => self.variant_alt[4] = num(),
                        "Allow Duplicate Variants" => {
                            self.allow_duplicate_variants = spec_val == "true"
                        }
                        // Skipped for builds with the old Watcher's Eye
                        "Has Variants" | "Selected Variants" => l += 1,
                        "League" => self.league = Some(spec_val.clone()),
                        "Crafted" => self.crafted = true,
                        "Scourge" => self.scourge = true,
                        "Crucible" => self.crucible = true,
                        "Implicit" => self.implicit = true,
                        "Prefix" | "Suffix" => {
                            let fractured = spec_val.starts_with("{fractured}");
                            spec_val = gsub(&spec_val, "^{fractured}", "");
                            let c = captures(&spec_val, "{range:([^}]+)}(.+)");
                            let (range_text, affix) = match &c {
                                Some(c) => (Some(c[0].clone()), Some(c[1].clone())),
                                None => (None, None),
                            };
                            let mut range = match &range_text {
                                Some(r) if r.contains(',') => Some(Range::Each(
                                    gmatch1(r, "[^,]+")
                                        .iter()
                                        .map(|v| pat::to_number(v).unwrap_or(f64::NAN))
                                        .collect(),
                                )),
                                Some(r) => pat::to_number(r).map(Range::One),
                                None => None,
                            };
                            let mod_id = affix.unwrap_or_else(|| spec_val.clone());
                            if range.is_none() && mod_id != "None" {
                                range = Some(Range::One(ctx.default_affix_quality));
                            }
                            let slot = AffixSlot {
                                mod_id,
                                range,
                                fractured,
                            };
                            if spec_name == "Prefix" {
                                self.prefixes.list.push(slot);
                            } else {
                                self.suffixes.list.push(slot);
                            }
                        }
                        "Implicits" => {
                            implicit_lines = num().unwrap_or(0.0);
                            stage = Stage::Explicit;
                        }
                        "Unreleased" => self.unreleased = spec_val == "true",
                        "Upgrade" => self.upgrade_paths.push(spec_val.clone()),
                        "Source" => self.source = Some(spec_val.clone()),
                        "Cluster Jewel Skill" => {
                            if let Some(cj) = self
                                .base_name
                                .as_ref()
                                .and_then(|b| data.cluster_jewels.get(b))
                                && cj.skills.contains(&spec_val)
                            {
                                self.cluster_jewel_skill = Some(spec_val.clone());
                            }
                        }
                        "Cluster Jewel Node Count" => {
                            if let Some(cj) = self
                                .base_name
                                .as_ref()
                                .and_then(|b| data.cluster_jewels.get(b))
                            {
                                let v = num().unwrap_or(cj.max_nodes);
                                self.cluster_jewel_node_count =
                                    Some(cj.max_nodes.min(v.max(cj.min_nodes)));
                            }
                        }
                        "Catalyst" => {
                            for (i, c) in CATALYSTS.iter().enumerate() {
                                if spec_val == *c {
                                    self.catalyst = Some(i + 1);
                                }
                            }
                        }
                        "CatalystQuality" => self.catalyst_quality = num(),
                        "Intangibility" => self.intangibility = num(),
                        "Note" => self.note = Some(spec_val.clone()),
                        "Str" | "Strength" => self.requirements.str = num().unwrap_or(f64::NAN),
                        "Dex" | "Dexterity" => self.requirements.dex = num().unwrap_or(f64::NAN),
                        "Int" | "Intelligence" => self.requirements.int = num().unwrap_or(f64::NAN),
                        "Critical Strike Range"
                        | "Attacks per Second"
                        | "Weapon Range"
                        | "Critical Strike Chance"
                        | "Physical Damage"
                        | "Elemental Damage"
                        | "Chaos Damage"
                        | "Chance to Block"
                        | "Block chance" => self.hidden_specs = true,
                        _ => {
                            // An explicit with a colon in it, unless part of the name
                            let in_name = pat::try_find(&self.name, &spec_name)?.is_some()
                                && pat::try_find(&self.name, &spec_val)?.is_some();
                            if !in_name {
                                found_explicit = true;
                                stage = Stage::Explicit;
                            }
                        }
                    }
                }
                if line == "Prefixes:" {
                    found_explicit = true;
                    stage = Stage::Explicit;
                }
                if (spec.is_none() || found_explicit || found_implicit)
                    && let ControlFlow::Base = self.parse_mod_line(
                        ctx,
                        line,
                        &mut l,
                        ParseState {
                            stage: &mut stage,
                            found_explicit: &mut found_explicit,
                            found_implicit: &mut found_implicit,
                            game_mode,
                            implicit_lines,
                            item_class: item_class.as_deref(),
                            pending_affixes: &mut pending_affixes,
                            flask_buff_lines: &mut flask_buff_lines,
                            tincture_buff_lines: &mut tincture_buff_lines,
                        },
                    )?
                {
                    l += 1;
                    continue;
                }
            }
            l += 1;
        }
        self.finish_parse(ctx, imported_level_req, high_quality)?;
        self.build_mod_list(ctx)?;
        if defer_jewel_radius {
            self.jewel_radius_index = self
                .jewel_data
                .as_ref()
                .and_then(|j| j.get("radiusIndex"))
                .and_then(ModValue::as_number);
        }
        Ok(())
    }
}

enum ControlFlow {
    Base,
    Line,
}

struct ParseState<'s> {
    stage: &'s mut Stage,
    found_explicit: &'s mut bool,
    found_implicit: &'s mut bool,
    game_mode: bool,
    implicit_lines: f64,
    item_class: Option<&'s str>,
    pending_affixes: &'s mut Option<Vec<(String, AffixKind)>>,
    flask_buff_lines: &'s mut Option<HashSet<String>>,
    tincture_buff_lines: &'s mut Option<HashSet<String>>,
}

const RANGE_PAIR: &str = "(%-?%d+%.?%d*)%((%-?%d+%.?%d*%-%-?%d+%.?%d*)%)";

/// The min and max of a `min-max` range, ordered (as strings, as upstream
/// keeps them)
fn min_max(range: &str) -> Result<(String, String)> {
    let c = captures(range, "(%-?%d+%.?%d*)%-(%-?%d+%.?%d*)").ok_or("range without numbers")?;
    let (min, max) = (c[0].clone(), c[1].clone());
    Ok(if tonum(&min)? > tonum(&max)? {
        (max, min)
    } else {
        (min, max)
    })
}

/// `value .. "%(" .. range:gsub("%-", "%%-") .. "%)"`: the pattern of a
/// rolled value and its range
fn rolled_pattern(value: &str, range: &str) -> String {
    format!("{value}%({}%)", gsub(range, "%-", "%%-"))
}

impl Item {
    /// The body of `ParseRaw`'s loop for a line that may be a mod line
    fn parse_mod_line(
        &mut self,
        ctx: &ItemContext,
        mut line: String,
        l: &mut usize,
        st: ParseState,
    ) -> Result<ControlFlow> {
        let data = ctx.data;
        let mut ml = ModLine::default();
        line = gsub_fn(&line, "{(%a*):?([^}]*)}", |c| {
            let (k, val) = (c[0].as_str(), c[1].as_str());
            let ints = |p: &str| -> BTreeSet<i64> {
                gmatch1(val, p)
                    .iter()
                    .filter_map(|v| pat::to_number(v))
                    .map(|v| v as i64)
                    .collect()
            };
            match k {
                "variant" => ml.variant_list = Some(ints("%d+")),
                "version" => {
                    ml.version_list = Some(ints("%d+"));
                    self.uses_variant_groups = true;
                }
                "group" => {
                    ml.variant_group_list =
                        Some(ints("%d+").into_iter().filter(|g| *g > 0).collect());
                    self.uses_variant_groups = true;
                }
                "tags" => ml.mod_tags.extend(gmatch1(val, "[%a_]+")),
                "modGroup" => ml.mod_group = Some(val.to_string()),
                "range" => {
                    self.advanced_copy = true;
                    ml.range = pat::to_number(val);
                }
                "corruptedRange" => ml.corrupted_range = pat::to_number(val),
                k => {
                    ml.flags.set(k);
                }
            }
            Some(String::new())
        });
        if let Some(groups) = &ml.variant_group_list
            && let Some(variants) = &ml.variant_list
        {
            for &g in groups {
                let group = self.variant_groups.entry(g).or_default();
                for &v in variants {
                    let known = self
                        .variant_list
                        .as_ref()
                        .is_some_and(|list| v >= 1 && (v as usize) <= list.len());
                    if !known {
                        continue;
                    }
                    let versions = group.entry(v).or_default();
                    match &ml.version_list {
                        Some(vl) => {
                            let count = self.version_list.as_ref().map_or(0, Vec::len) as i64;
                            versions.extend(vl.iter().filter(|x| **x >= 1 && **x <= count));
                        }
                        None => {
                            versions.insert(0);
                        }
                    }
                }
            }
        }
        line = gsub_fn(&line, " %((%l+)%)", |c| {
            ml.flags.set(&c[0]);
            Some(String::new())
        });
        if ml.flags.enchant {
            ml.flags.crafted = true;
            ml.flags.implicit = true;
        }
        if ml.flags.vestigial {
            self.vestigial = true;
        }

        // Lines with (#-#) ranges have no catalyst or mod magnitude applied
        if is_match(&line, "%((%-?%d+%.?%d*)%-(%-?%d+%.?%d*)%)") {
            self.advanced_copy = true;
        }
        let mut base_name = None;
        if self.base.is_none() && (self.rarity == "NORMAL" || self.rarity == "MAGIC") {
            // Exact match (affix-less magic and normal items)
            if self.name.contains("Energy Blade")
                && let Some(class) = st.item_class
            {
                self.name = if class.contains("One Hand") {
                    "Energy Blade One Handed".into()
                } else {
                    "Energy Blade Two Handed".into()
                };
            }
            if data.base(&self.name).is_some() {
                base_name = Some(self.name.clone());
            } else {
                // Partial match (magic items with affixes): the longest
                let mut best: Option<(&str, usize, usize)> = None;
                let mut best_len: i64 = -1;
                for b in data.bases() {
                    if let Some(s) = self.name.find(b.name.as_str()) {
                        let e = s + b.name.len();
                        let len = e as i64 - 1 - s as i64;
                        if len > best_len {
                            best = Some((&b.name, s, e));
                            best_len = len;
                        }
                    }
                }
                if let Some((m, s, e)) = best {
                    self.name_prefix = self.name[..s].to_string();
                    self.name_suffix = self.name[e..].to_string();
                    base_name = Some(m.to_string());
                }
            }
            if base_name.is_none()
                && let Some(s) = self.name.find("Two-Toned Boots")
            {
                // Hack for Two-Toned Boots
                base_name = Some("Two-Toned Boots".into());
                self.name_prefix = self.name[..s].to_string();
                self.name_suffix = self.name[s + "Two-Toned Boots".len()..].to_string();
            }
            self.name = gsub(&self.name, " %(.+%)", "");
        }
        let mut base_name = base_name.unwrap_or_else(|| {
            let b = gsub(&line, "^Superior ", "");
            let b = gsub(&b, "^Synthesised ", "");
            gsub(&b, "^Vestigial ", "")
        });
        if base_name == "Two-Toned Boots" {
            base_name = "Two-Toned Boots (Armour/Energy Shield)".into();
        }
        if let Some(base) = data.base(&base_name).cloned() {
            if self.uses_variant_groups
                && self.selected_version.is_none()
                && let Some(v) = &self.version_list
            {
                self.selected_version = Some(v.len() as f64);
            }
            // Set the base if the variant matches or there are no variants
            let matches = if self.uses_variant_groups {
                self.check_mod_line_variant(&ml)
            } else {
                self.variant.is_none()
                    || ml.variant_list.is_none()
                    || has(ml.variant_list.as_ref().unwrap(), self.variant)
            };
            if matches {
                self.set_base(
                    ctx,
                    base_name,
                    base,
                    st.flask_buff_lines,
                    st.tincture_buff_lines,
                )?;
            }
            // Base lines are not mod lines
            return Ok(ControlFlow::Base);
        }
        if ml.flags.implicit {
            *st.found_implicit = true;
            *st.stage = Stage::Implicit;
        }
        let mut catalyst = 1.0;
        if is_match(&line, " %- Unscalable Value$") {
            line = gsub(&line, " %- Unscalable Value$", "");
            ml.flags.unscalable = true;
        } else {
            catalyst = catalyst_scalar(self.catalyst, &ml, self.catalyst_quality);
        }
        // Advanced copy gives fixed values as current(base)
        line = gsub(&line, "(%-?%d+%.?%d*)%((%-?%d+%.?%d*)%)", "%1");
        if let Some(pending) = st.pending_affixes.as_mut().filter(|p| !p.is_empty()) {
            let affixes = self
                .affixes
                .clone()
                .ok_or("pending affixes without affixes")?;
            if pending.len() > 1 {
                // Probably a conqueror or essence mod: match the line
                let range_line = gsub(&line, "%-?%d+%.?%d*%(", "(");
                let stripped = gsub(&range_line, "%-?%d+%.?%d*", "#");
                let mut exact = None;
                let mut fallback = None;
                for (i, (id, _)) in pending.iter().enumerate() {
                    let m = affixes.get(id).ok_or("pending affix not found")?;
                    for dl in &m.lines {
                        if line == *dl || range_line == *dl {
                            exact = Some(i);
                            break;
                        }
                        if fallback.is_none() && stripped == gsub(dl, "%-?%d+%.?%d*", "#") {
                            fallback = Some(i);
                        }
                    }
                    if exact.is_some() {
                        break;
                    }
                }
                let keep = pending[exact.or(fallback).unwrap_or(0)].clone();
                *pending = vec![keep];
            }
            let (mod_id, kind) = pending[0].clone();
            // The finest of the ranges, for the most precision
            let mut best_delta = -1.0;
            let mut best_range = -1.0;
            let mut rolls = Vec::new();
            ml.order = affixes
                .get(&mod_id)
                .and_then(|m| m.stat_order.first().copied());
            for c in gmatch(&line.clone(), RANGE_PAIR) {
                let (value, range) = (&c[0], &c[1]);
                let (min, max) = min_max(range)?;
                let (v, lo) = (tonum(value)?, tonum(&min)?);
                let delta = tonum(&max)? - lo;
                rolls.push(if delta > 0.0 {
                    round((v - lo) / delta, Some(6.0))
                } else {
                    0.5
                });
                line = gsub(&line, &rolled_pattern(value, range), value);
                if delta > best_delta {
                    best_range = round((v - lo) / delta, Some(3.0));
                    best_delta = delta;
                }
            }
            let range = if rolls.len() > 1 {
                Range::Each(rolls)
            } else if best_delta > 0.0 {
                Range::One(best_range)
            } else {
                Range::One(0.5)
            };
            let slot = AffixSlot {
                mod_id,
                range: Some(range),
                fractured: ml.flags.fractured,
            };
            match kind {
                AffixKind::Prefix => self.prefixes.list.push(slot),
                AffixKind::Suffix => self.suffixes.list.push(slot),
            }
            *st.pending_affixes = Some(vec![]);
        } else {
            let mut best_delta = -1.0;
            let mut best_range = -1.0;
            let mut first_roll = None;
            let mut independent = false;
            // Advanced copy gives only the ends of enum ranges
            line = gsub_fn(&line, "(%s*)(%b())", |c| {
                let (space, range) = (&c[0], &c[1]);
                if range.contains('-') && !is_match(range, "%d") {
                    Some(String::new())
                } else {
                    Some(format!("{space}{range}"))
                }
            });
            let advanced_copy_line = line.clone();
            for c in gmatch(&line.clone(), RANGE_PAIR) {
                let (value, range) = (&c[0], &c[1]);
                let (min, max) = min_max(range)?;
                let (v, lo) = (tonum(value)?, tonum(&min)?);
                let delta = tonum(&max)? - lo;
                let roll = if delta > 0.0 {
                    round((v - lo) / delta, Some(6.0))
                } else {
                    0.5
                };
                if first_roll.is_some_and(|f| f != roll) {
                    independent = true;
                }
                first_roll.get_or_insert(roll);
                if delta > best_delta {
                    best_range = roll;
                    best_delta = delta;
                }
                let replacement = if !(0.0..=1.0).contains(&best_range) {
                    value.clone()
                } else {
                    format!("{}({min}-{max})", if v < 0.0 { "+" } else { "" })
                };
                line = pat::gsub(&line, &rolled_pattern(value, range), &replacement);
            }
            if independent {
                line = gsub(
                    &advanced_copy_line,
                    "(%-?%d+%.?%d*)%(%-?%d+%.?%d*%-%-?%d+%.?%d*%)",
                    "%1",
                );
            } else if (0.0..=1.0).contains(&best_range) {
                ml.range = Some(best_range);
            }
        }
        let corrupted = ml.corrupted_range;
        let mut ranged = apply_range(
            data,
            ctx.parser,
            &line,
            &Range::One(1.0),
            Some(catalyst),
            corrupted,
        )?;
        let (mut mod_list, mut extra) = Item::parse_mod(ctx, &ranged)?;
        let next = self.raw_lines.get(*l + 1).cloned();
        let next_group = next
            .as_deref()
            .and_then(|n| capture(n, "{modGroup:([^}]+)}"));
        // Only lines of one modifier combine
        if (mod_list.is_none() || extra.is_some())
            && let Some(next) = &next
            && ml.mod_group == next_group
        {
            // Try to combine it with the next line
            let next_line = gsub(&gsub(next, "%b{}", ""), " ?%(%l+%)", "");
            let comb = format!("{line} {next_line}");
            ranged = apply_range(
                data,
                ctx.parser,
                &comb,
                &Range::One(1.0),
                Some(catalyst),
                corrupted,
            )?;
            (mod_list, extra) = Item::parse_mod(ctx, &ranged)?;
            if mod_list.is_some() && extra.is_none() {
                line = format!("{line}\n{next_line}");
                *l += 1;
            } else {
                (mod_list, extra) = Item::parse_mod(ctx, &ranged)?;
            }
        }
        let lower = if ml.flags.disabled {
            String::new()
        } else {
            line.to_ascii_lowercase()
        };
        self.read_line_property(&lower);

        // Mod magnitude lines
        const MAGNITUDE_PATTERNS: [&str; 3] = [
            "(%d+)%% ([ir][ne][cd][ru][ec][ae][sd]e?d?) ?([%a%s]*) modifier magnitudes",
            "(%d+)%% ([ir][ne][cd][ru][ec][ae][sd]e?d?) effect of ([sp][ur][fe]fix)es",
            "([%a%s]*) modifier magnitudes are doubled",
        ];
        let ranged_lower = ranged.to_ascii_lowercase();
        for p in MAGNITUDE_PATTERNS {
            if ml.flags.disabled || !is_match(&ranged_lower, p) {
                continue;
            }
            let r = Range::One(ml.range.unwrap_or(ctx.default_affix_quality));
            let rl = apply_range(data, ctx.parser, &line, &r, Some(catalyst), corrupted)?
                .to_ascii_lowercase();
            let Some(c) = captures(&rl, p) else {
                continue;
            };
            let (amount, inc_dec, tags_text, multiplier) = if c.len() == 1 {
                // "are doubled": 100% increased, doubled
                (
                    "100".to_string(),
                    "increased".to_string(),
                    c[0].clone(),
                    Some(2.0),
                )
            } else {
                (c[0].clone(), c[1].clone(), c[2].clone(), None)
            };
            if inc_dec == "increased" || inc_dec == "reduced" {
                let amount = tonum(&amount)?;
                let quality = if inc_dec == "increased" {
                    amount
                } else {
                    -amount
                };
                if tags_text == "explicit physical and chaos damage" {
                    self.mod_magnitude_mods.push(ModMagnitude {
                        tags: vec!["damage".into()],
                        any_tags: Some(["physical".into(), "chaos".into()]),
                        quality,
                        multiplier: None,
                        mod_type: Some("explicit".into()),
                    });
                } else {
                    let mut tags = Vec::new();
                    let mut mod_type = None;
                    for word in gmatch1(&format!("{tags_text} "), "%S+") {
                        let word = word.to_ascii_lowercase();
                        let word = match word.as_str() {
                            "defence" => "defences".to_string(),
                            "enchantment" => "enchant".to_string(),
                            _ => word,
                        };
                        if word == "implicit" || word == "explicit" || word == "enchant" {
                            mod_type = Some(word);
                        } else {
                            tags.push(word);
                        }
                    }
                    self.mod_magnitude_mods.push(ModMagnitude {
                        tags,
                        any_tags: None,
                        quality,
                        multiplier,
                        mod_type,
                    });
                }
                break;
            }
        }

        let counted = (self.enchant_mod_lines.len() + self.implicit_mod_lines.len()) as f64;
        let which = if ml.flags.enchant || (ml.flags.crafted && counted < st.implicit_lines) {
            Lines::Enchant
        } else if ml.flags.scourge {
            Lines::Scourge
        } else if line.contains("Requires Class") {
            Lines::ClassRequirement
        } else if ml.flags.implicit
            || (!ml.flags.crafted
                && ((self.enchant_mod_lines.len()
                    + self.scourge_mod_lines.len()
                    + self.implicit_mod_lines.len()) as f64)
                    < st.implicit_lines)
        {
            Lines::Implicit
        } else if ml.flags.crucible {
            Lines::Crucible
        } else {
            Lines::Explicit
        };
        ml.line = line.clone();
        if let Some(list) = mod_list {
            ml.mod_list = list;
            ml.extra = extra;
            ml.value_scalar = Some(catalyst);
            ml.range = Some(ml.range.unwrap_or(ctx.default_affix_quality));
            self.lines_mut(which).push(ml);
            if st.game_mode {
                match *st.stage {
                    Stage::FindImplicit => *st.stage = Stage::Implicit,
                    Stage::FindExplicit => {
                        *st.found_explicit = true;
                        *st.stage = Stage::Explicit;
                    }
                    Stage::Explicit => *st.found_explicit = true,
                    _ => {}
                }
            } else {
                *st.found_explicit = true;
            }
        } else if st.game_mode {
            let base_type_line = self.base.as_ref().is_some_and(|b| {
                line == b.typ
                    || b.sub_type
                        .as_ref()
                        .is_some_and(|s| line == format!("{s} {}", b.typ))
            });
            if *st.stage == Stage::Implicit
                || *st.stage == Stage::Explicit
                || (*st.stage == Stage::FindImplicit
                    && data.base(&line).is_none()
                    && self.name != line
                    && !line.contains("Two-Toned")
                    && !base_type_line)
            {
                ml.mod_list = vec![];
                ml.extra = Some(line);
                ml.range = Some(ctx.default_affix_quality);
                self.lines_mut(which).push(ml);
            } else if *st.stage == Stage::FindExplicit {
                *st.stage = Stage::Done;
            }
        } else if *st.found_explicit || *st.stage == Stage::Explicit {
            ml.mod_list = vec![];
            ml.extra = Some(line);
            self.lines_mut(which).push(ml);
        }
        Ok(ControlFlow::Line)
    }

    /// The item properties a mod line sets (`implicit modifiers cannot be
    /// changed`, affix limits, crafting restrictions)
    fn read_line_property(&mut self, lower: &str) {
        let count = |p: &str| {
            capture(lower, p)
                .and_then(|n| pat::to_number(&n))
                .unwrap_or(0.0)
        };
        match lower {
            "implicit modifiers cannot be changed" => self.implicits_cannot_be_changed = true,
            _ if is_match(lower, " prefix modifiers? allowed") => {
                self.prefixes.limit = Some(
                    self.prefixes.limit.unwrap_or(0.0) + count("%+(%d+) prefix modifiers? allowed")
                        - count("%-(%d+) prefix modifiers? allowed"),
                );
            }
            _ if is_match(lower, " suffix modifiers? allowed") => {
                self.suffixes.limit = Some(
                    self.suffixes.limit.unwrap_or(0.0) + count("%+(%d+) suffix modifiers? allowed")
                        - count("%-(%d+) suffix modifiers? allowed"),
                );
            }
            _ if lower.contains("can be anointed") => self.can_be_anointed = true,
            "can have a second enchantment modifier"
            | "can have 1 additional enchantment modifiers" => self.can_have_two_enchants = true,
            "can have 2 additional enchantment modifiers" => {
                self.can_have_two_enchants = true;
                self.can_have_three_enchants = true;
            }
            "can have 3 additional enchantment modifiers" => {
                self.can_have_two_enchants = true;
                self.can_have_three_enchants = true;
                self.can_have_four_enchants = true;
            }
            "has elder, shaper and all conqueror influences" => {
                self.has_elder_shaper_and_all_conqueror_influences = true;
                for i in 0..6 {
                    self.influences[i] = true;
                }
            }
            _ if lower.contains("if the eater of worlds is dominant") => {
                self.can_have_eldritch_influence = true
            }
            "has a crucible passive skill tree with only support passive skills" => {
                self.can_have_only_support_skills_crucible_tree = true
            }
            "has a crucible passive skill tree" => self.can_have_shield_crucible_tree = true,
            "has a two handed sword crucible passive skill tree" => {
                self.can_have_two_handed_sword_crucible_tree = true
            }
            "cannot roll caster modifiers" => {
                self.restrict_tag = true;
                self.no_caster = true;
            }
            "cannot roll attack modifiers" => {
                self.restrict_tag = true;
                self.no_attack = true;
            }
            _ => {
                if let Some(e) =
                    capture(lower, "^cannot roll modifiers of non%-(%a+) damage types$")
                    && ["cold", "fire", "lightning", "chaos", "physical"].contains(&e.as_str())
                {
                    self.restrict_damage_type = true;
                    self.only_damage.insert(e);
                }
            }
        }
    }

    /// Sets the item's base from a base line
    fn set_base(
        &mut self,
        ctx: &ItemContext,
        base_name: String,
        base: Rc<Base>,
        flask_buff_lines: &mut Option<HashSet<String>>,
        tincture_buff_lines: &mut Option<HashSet<String>>,
    ) -> Result<()> {
        let data = ctx.data;
        self.base_name = Some(base_name.clone());
        if !(self.rarity == "NORMAL" || self.rarity == "MAGIC") {
            self.title = Some(self.name.clone());
        }
        if self.rarity == "UNIQUE"
            && let Some(title) = &self.title
            // Its Foulborn change, a larger radius, is no mod line
            && !title.to_ascii_lowercase().contains("might of the meek")
        {
            self.mutated_lines = data
                .foulborn_map
                .get(&gsub(title, "^[Ff]oulborn ", ""))
                .cloned();
        }
        self.typ = Some(base.typ.clone());
        self.base = Some(base.clone());
        let group = base.sub_type.as_ref().map(|s| format!("{}{s}", base.typ));
        self.affixes = group
            .and_then(|g| data.item_mods.get(&g))
            .or_else(|| data.item_mods.get(&base.typ))
            .or_else(|| data.item_mods.get("Item"))
            .cloned();
        if let Some(title) = &self.title {
            let key = title.to_ascii_lowercase();
            if let Some(r) = data.rare_like_uniques.get(&key) {
                self.rare_like_unique = Some(key);
                self.affixes = Some(r.affixes.clone());
            } else {
                self.rare_like_unique = None;
            }
        }
        self.corruptible = base.typ != "Flask";
        self.can_be_influenced = base.influence_tags.is_some();
        self.requirements.str = base.req.str.unwrap_or(0.0);
        self.requirements.dex = base.req.dex.unwrap_or(0.0);
        self.requirements.int = base.req.int.unwrap_or(0.0);
        let r = &self.requirements;
        let max_req = r.str.max(r.dex).max(r.int);
        self.default_socket_color = Some(if max_req == r.dex {
            'G'
        } else if max_req == r.int {
            'B'
        } else {
            'R'
        });
        let buffs = |lines: &[String],
                     set: &mut Option<HashSet<String>>,
                     out: &mut Vec<ModLine>|
         -> Result<()> {
            let mut s = HashSet::new();
            for line in lines {
                s.insert(line.clone());
                let (list, extra) = Item::parse_mod(ctx, line)?;
                out.push(ModLine {
                    line: line.clone(),
                    extra,
                    mod_list: list.unwrap_or_default(),
                    ..ModLine::default()
                });
            }
            *set = Some(s);
            Ok(())
        };
        if let Some(lines) = base.flask.as_ref().and_then(|f| f.buff.as_ref())
            && flask_buff_lines.is_none()
        {
            buffs(lines, flask_buff_lines, &mut self.buff_mod_lines)?;
        }
        if let Some(lines) = base.tincture.as_ref().and_then(|f| f.buff.as_ref())
            && tincture_buff_lines.is_none()
        {
            buffs(lines, tincture_buff_lines, &mut self.buff_mod_lines)?;
        }
        Ok(())
    }

    /// The end of `ParseRaw`, after the lines
    fn finish_parse(
        &mut self,
        ctx: &ItemContext,
        imported_level_req: Option<f64>,
        high_quality: bool,
    ) -> Result<()> {
        let data = ctx.data;
        let unique = self.rarity == "UNIQUE" || self.rarity == "RELIC";
        if self.advanced_copy && unique {
            for ml in &mut self.explicit_mod_lines {
                let exact = ml.line.to_ascii_lowercase().replace('\n', " ");
                ml.order = data
                    .unique_stat_order_exact
                    .get(&exact)
                    .or_else(|| {
                        data.unique_stat_order_normalised
                            .get(&normalise_mod_line(&ml.line))
                    })
                    .copied();
            }
        }
        if let Some(base) = &self.base {
            if !self.sockets.is_empty() {
                // In-game totals include the socketed gems' requirements
                self.requirements.str = base.req.str.unwrap_or(0.0);
                self.requirements.dex = base.req.dex.unwrap_or(0.0);
                self.requirements.int = base.req.int.unwrap_or(0.0);
            }
            if self.requirements.level.is_none() {
                // Imported requirements are trusted only without sockets
                self.requirements.level = match imported_level_req {
                    Some(r) if self.sockets.is_empty() => Some(r),
                    _ => base.req.level,
                };
            }
        }
        // Hack for Cane of Kulemak
        if self
            .title
            .as_ref()
            .is_some_and(|t| t.eq_ignore_ascii_case("cane of kulemak"))
            && unique
            && self.advanced_copy
        {
            for ml in &mut self.explicit_mod_lines {
                if !ml.line.contains("magnitude") {
                    ml.flags.unveiled = true;
                }
            }
        }
        if (self.advanced_copy || self.crafted) && !self.mod_magnitude_mods.is_empty() {
            self.apply_mod_magnitudes(ctx)?;
        }
        if self.advanced_copy && self.explicit_mod_lines.len() > 1 {
            // sortCraftedModLines
            let group = |m: &ModLine| {
                if m.flags.crafted || m.flags.custom {
                    3
                } else if m.flags.fractured {
                    1
                } else {
                    2
                }
            };
            let mut lines: Vec<(usize, ModLine)> = std::mem::take(&mut self.explicit_mod_lines)
                .into_iter()
                .enumerate()
                .collect();
            lines.sort_by(|(ai, a), (bi, b)| {
                let (ag, bg) = (group(a), group(b));
                if ag != bg {
                    return ag.cmp(&bg);
                }
                if ag < 3 && a.order != b.order {
                    let (ao, bo) = (
                        a.order.unwrap_or(f64::INFINITY),
                        b.order.unwrap_or(f64::INFINITY),
                    );
                    return ao.partial_cmp(&bo).unwrap_or(std::cmp::Ordering::Equal);
                }
                ai.cmp(bi)
            });
            self.explicit_mod_lines = lines.into_iter().map(|(_, m)| m).collect();
        }
        self.affix_limit = 0.0;
        if self.crafted {
            self.finish_affixes(data);
        }
        if let Some(limit) = self.base.as_ref().and_then(|b| b.socket_limit)
            && self.sockets.is_empty()
        {
            let color = self.default_socket_color.unwrap_or('R');
            let mut i = 1.0;
            while i <= limit {
                self.sockets.push(Socket { color, group: 0.0 });
                i += 1.0;
            }
        }
        self.abyssal_socket_count = 0.0;
        if self.uses_variant_groups {
            self.normalise_variant_selections();
        } else if let Some(list) = &self.variant_list {
            let len = list.len() as f64;
            self.variant = Some(len.min(self.variant.unwrap_or(len)));
            for i in 0..5 {
                if self.has_alt_variant[i] {
                    self.variant_alt[i] = Some(len.min(self.variant_alt[i].unwrap_or(len)));
                }
            }
        }
        if let Some(pairs) = self.mutated_lines.clone() {
            // Both ways, so one checkbox can apply or revert the change
            for (orig, foul) in &pairs {
                self.check_mutated_mod(data, orig, foul, false);
                self.check_mutated_mod(data, foul, orig, true);
            }
        }
        if self.explicit_mod_lines.iter().any(|m| m.flags.mutated) {
            self.foulborn = true;
        }
        let has_prefix = self
            .title
            .as_ref()
            .is_some_and(|t| t.to_ascii_lowercase().starts_with("foulborn "));
        if self.foulborn && !has_prefix {
            let title = self
                .title
                .as_ref()
                .ok_or("attempt to concatenate a nil value (title)")?;
            self.title = Some(format!("Foulborn {title}"));
        } else if !self.foulborn && has_prefix {
            self.title = self.title.as_ref().map(|t| gsub(t, "[Ff]oulborn ", ""));
        }
        if let (Some(base), Some(title)) = (&self.base_name, &self.title) {
            self.name = format!("{title}, {}", gsub(base, " %(.+%)", ""));
        }
        if self.quality.is_none() {
            self.normalise_quality();
            if high_quality {
                // Twice, as upstream does
                self.normalise_quality();
            }
        }
        if let Some(base) = self.base.clone()
            && let Some(enchant) = &base.enchant
            && self.enchant_mod_lines.is_empty()
        {
            for (i, line) in gmatch1(enchant, "[^\n]+").into_iter().enumerate() {
                let (list, extra) = Item::parse_mod(ctx, &line)?;
                self.enchant_mod_lines.push(ModLine {
                    line,
                    flags: LineFlags {
                        crafted: true,
                        implicit: true,
                        enchant: true,
                        ..LineFlags::default()
                    },
                    extra,
                    mod_list: list.unwrap_or_default(),
                    mod_tags: base
                        .enchant_mod_types
                        .as_ref()
                        .and_then(|t| t.get(i).cloned())
                        .unwrap_or_default(),
                    ..ModLine::default()
                });
            }
        }
        Ok(())
    }

    /// Applies the mod magnitude lines to the lines they scale
    fn apply_mod_magnitudes(&mut self, ctx: &ItemContext) -> Result<()> {
        for mm in self.mod_magnitude_mods.clone() {
            let lists: Vec<Lines> = match mm.mod_type.as_deref() {
                Some("implicit") => vec![Lines::Implicit],
                Some("explicit") => vec![Lines::Explicit],
                Some("enchant") => vec![Lines::Enchant],
                Some(_) => vec![],
                None => vec![Lines::Implicit, Lines::Explicit, Lines::Enchant],
            };
            for which in lists {
                for i in 0..self.lines_mut(which).len() {
                    let ml = self.lines_mut(which)[i].clone();
                    // Inactive variant lines are not scaled
                    if ml.variant_list.is_some() && self.mod_line_variant_count(&ml) == 0 {
                        continue;
                    }
                    let has_tag = |t: &str| {
                        ml.mod_tags.iter().any(|m| m == t)
                            || (t == "unveiled" && ml.flags.unveiled)
                            || (t == "prefix" && ml.flags.prefix)
                            || (t == "suffix" && ml.flags.suffix)
                    };
                    let mut matched = mm.tags.iter().all(|t| has_tag(t));
                    if let Some([a, b]) = &mm.any_tags
                        && !(has_tag(a) || has_tag(b))
                    {
                        matched = false;
                    }
                    let mut ml = ml;
                    if matched && !ml.flags.unscalable {
                        ml.value_scalar = Some(match mm.multiplier {
                            Some(m) => ml.value_scalar.unwrap_or(1.0) * m,
                            None => ml.value_scalar.unwrap_or(1.0) + mm.quality / 100.0,
                        });
                    }
                    if let Some(vs) = ml.value_scalar.filter(|v| *v != 1.0) {
                        let r = Range::One(ml.range.unwrap_or(1.0));
                        let ranged =
                            apply_range(ctx.data, ctx.parser, &ml.line, &r, Some(vs), Some(1.0))?;
                        let (list, extra) = Item::parse_mod(ctx, &ranged)?;
                        ml.display_value_scalar = Some(1.0);
                        ml.mod_list = list.unwrap_or_default();
                        ml.extra = extra;
                    }
                    self.lines_mut(which)[i] = ml;
                }
            }
        }
        Ok(())
    }

    /// The affix slots of a crafted item
    fn finish_affixes(&mut self, data: &ItemData) {
        let Some(affixes) = self.affixes.clone() else {
            self.crafted = false;
            return;
        };
        if self.rarity == "MAGIC" {
            if self.prefixes.limit.is_some() || self.suffixes.limit.is_some() {
                let p = (self.prefixes.limit.unwrap_or(0.0) + 1.0).min(2.0).max(0.0);
                let s = (self.suffixes.limit.unwrap_or(0.0) + 1.0).min(2.0).max(0.0);
                self.prefixes.limit = Some(p);
                self.suffixes.limit = Some(s);
                self.affix_limit = p + s;
            } else {
                self.affix_limit = 2.0;
            }
        } else if self.rarity == "RARE" {
            let base = self.base.as_ref();
            let small = (self.typ.as_deref() == Some("Jewel")
                && !(base.and_then(|b| b.sub_type.as_deref()) == Some("Abyss") && self.corrupted))
                || self.typ.as_deref() == Some("Graft");
            self.affix_limit = if small { 4.0 } else { 6.0 };
            if self.prefixes.limit.is_some() || self.suffixes.limit.is_some() {
                let half = self.affix_limit / 2.0;
                let p = (self.prefixes.limit.unwrap_or(0.0) + half)
                    .min(self.affix_limit)
                    .max(0.0);
                let s = (self.suffixes.limit.unwrap_or(0.0) + half)
                    .min(self.affix_limit)
                    .max(0.0);
                self.prefixes.limit = Some(p);
                self.suffixes.limit = Some(s);
                self.affix_limit = p + s;
            }
        } else if let Some(r) = self
            .rare_like_unique
            .as_ref()
            .map(|r| &data.rare_like_uniques[r])
        {
            self.prefixes.limit = Some(r.prefix_limit);
            self.suffixes.limit = Some(r.suffix_limit);
            self.affix_limit = r.prefix_limit + r.suffix_limit;
        } else {
            self.crafted = false;
        }
        if !self.crafted {
            return;
        }
        let half = self.affix_limit / 2.0;
        for list in [&mut self.prefixes, &mut self.suffixes] {
            let count = list.limit.unwrap_or(half);
            let mut i = 1.0;
            while i <= count {
                let idx = i as usize - 1;
                if idx >= list.list.len() {
                    // `list[i] = { modId = "None" }`: a gap stays a gap
                    while list.list.len() < idx {
                        list.list.push(AffixSlot {
                            mod_id: "None".into(),
                            range: None,
                            fractured: false,
                        });
                    }
                    list.list.push(AffixSlot {
                        mod_id: "None".into(),
                        range: None,
                        fractured: false,
                    });
                } else {
                    let slot = &mut list.list[idx];
                    if slot.mod_id != "None" && affixes.get(&slot.mod_id).is_none() {
                        if let Some(m) = affixes
                            .list
                            .iter()
                            .find(|m| m.affix.as_deref() == Some(slot.mod_id.as_str()))
                        {
                            slot.mod_id = m.id.clone();
                        }
                        if affixes.get(&slot.mod_id).is_none() {
                            slot.mod_id = "None".into();
                        }
                    }
                }
                i += 1.0;
            }
        }
    }

    /// `checkMod` of `ParseRaw`: marks the explicit lines of a unique mod
    /// that has a Foulborn counterpart
    fn check_mutated_mod(
        &mut self,
        data: &ItemData,
        mod_id: &str,
        new_mod_id: &str,
        mutated: bool,
    ) {
        let group = if mutated { "Foulborn" } else { "ItemExclusive" };
        let Some(original) = data
            .item_mods
            .get(group)
            .and_then(|g| g.get(mod_id))
            .cloned()
        else {
            return;
        };
        let find = |item: &Item, lines: &[String]| -> Option<Vec<usize>> {
            let mut matched: Vec<usize> = Vec::new();
            for line in lines {
                let stat = normalise_mod_line(line);
                for (i, ml) in item.explicit_mod_lines.iter().enumerate() {
                    if !matched.contains(&i)
                        && normalise_mod_line(&ml.line) == stat
                        && item.check_mod_line_variant(ml)
                    {
                        matched.push(i);
                        break;
                    }
                }
            }
            (matched.len() == lines.len()).then_some(matched)
        };
        // Some descriptions are separate lines in PoB
        let joined = [original.lines.join(" ")];
        let Some(matching) = find(self, &joined).or_else(|| find(self, &original.lines)) else {
            return;
        };
        for i in matching {
            let ml = &mut self.explicit_mod_lines[i];
            ml.mod_id = Some(mod_id.to_string());
            ml.new_mod_id = Some(new_mod_id.to_string());
            if mutated {
                ml.flags.mutated = true;
            }
        }
    }

    /// `GetPrimarySlot`
    pub fn primary_slot(&self) -> String {
        let base = self.base.as_ref();
        let typ = self.typ.as_deref().unwrap_or("");
        if base.is_some_and(|b| b.weapon.is_some()) {
            "Weapon 1".into()
        } else if typ == "Quiver" || typ == "Shield" {
            "Weapon 2".into()
        } else if typ == "Ring" {
            "Ring 1".into()
        } else if typ == "Flask" || typ == "Tincture" {
            "Flask 1".into()
        } else if typ == "Graft" {
            "Graft 1".into()
        } else {
            typ.into()
        }
    }

    /// `getRangedModList`: the line's mods rolled at its range, when it has
    /// ranges and the rolled line parses fully
    fn ranged_mod_list(&self, ctx: &ItemContext, ml: &ModLine) -> Result<Option<Vec<Mod>>> {
        let Some(range) = ml.range else {
            return Ok(None);
        };
        if !is_match(&ml.line, "%((%-?%d+%.?%d*)%-(%-?%d+%.?%d*)%)") {
            return Ok(None);
        }
        let line = apply_range(
            ctx.data,
            ctx.parser,
            &ml.line.replace('\n', " "),
            &Range::One(range),
            ml.value_scalar,
            ml.corrupted_range,
        )?;
        let (list, extra) = Item::parse_mod(ctx, &line)?;
        if is_zero_value_line(&line) {
            return Ok(Some(vec![]));
        }
        Ok(if extra.is_none() { list } else { None })
    }

    /// `BuildModList`: the mods the item grants, for each slot it can take.
    pub fn build_mod_list(&mut self, ctx: &ItemContext) -> Result<()> {
        let Some(base) = self.base.clone() else {
            return Ok(());
        };
        if base.weapon.is_some() {
            self.weapon_data = Some(BTreeMap::new());
        } else if base.armour.is_some() {
            self.armour_data.get_or_insert_with(Fields::new);
        } else if base.flask.is_some() {
            self.flask_data = Some(Fields::new());
            self.buff_mod_list = vec![];
        } else if base.tincture.is_some() {
            self.tincture_data = Some(Fields::new());
            self.buff_mod_list = vec![];
        } else if self.typ.as_deref() == Some("Jewel") {
            self.jewel_data = Some(Fields::new());
        }
        let id = self.id.map_or("-1".to_string(), num);
        self.mod_source = format!("Item:{id}:{}", self.name);
        let source = self.mod_source.clone();
        for i in 0..self.buff_mod_lines.len() {
            let ml = &self.buff_mod_lines[i];
            if ml.extra.is_none() && self.check_mod_line_variant(ml) {
                let mut mods = ml.mod_list.clone();
                for m in &mut mods {
                    m.source = Some(source.as_str().into());
                }
                self.buff_mod_lines[i].mod_list = mods.clone();
                self.buff_mod_list.extend(mods);
            }
        }
        let mut base_list = Vec::new();
        for which in [
            Lines::Enchant,
            Lines::Scourge,
            Lines::ClassRequirement,
            Lines::Implicit,
            Lines::Explicit,
            Lines::Crucible,
        ] {
            for i in 0..self.lines_mut(which).len() {
                let mut ml = self.lines_mut(which)[i].clone();
                self.process_mod_line(ctx, &mut ml, &mut base_list)?;
                self.lines_mut(which)[i] = ml;
            }
        }
        if matches!(
            self.name.as_str(),
            "Tabula Rasa, Simple Robe"
                | "Skin of the Loyal, Simple Robe"
                | "Skin of the Lords, Simple Robe"
                | "The Apostate, Cabalist Regalia"
        ) {
            // Hack to remove the energy shield and base int requirement
            base_list.push(Mod {
                source: None,
                ..new_mod(
                    "ArmourData",
                    ModType::List,
                    ModValue::Record(Record {
                        fields: vec![
                            (Sym::new("key"), ModValue::Str("EnergyShield".into())),
                            (Sym::new("value"), n(0.0)),
                        ],
                    }),
                    "",
                    vec![],
                )
            });
            self.requirements.int = 0.0;
        }
        if calc_flag(&mut base_list, "NoAttributeRequirements")? {
            self.requirements.str_mod = 0.0;
            self.requirements.dex_mod = 0.0;
            self.requirements.int_mod = 0.0;
        } else {
            let mut req = |attr: f64, stat: &str| -> Result<f64> {
                let b = calc_num(&mut base_list, stat, ModType::Base, 0)?;
                let inc = calc_num(&mut base_list, stat, ModType::Inc, 0)?;
                Ok(((attr + b) * (1.0 + inc / 100.0)).floor())
            };
            self.requirements.str_mod = req(self.requirements.str, "StrRequirement")?;
            self.requirements.dex_mod = req(self.requirements.dex, "DexRequirement")?;
            self.requirements.int_mod = req(self.requirements.int, "IntRequirement")?;
        }
        self.granted_skills = vec![];
        for v in list_values(&base_list, "ExtraSkill")? {
            let ModValue::Record(r) = v else {
                return Err(format!("ExtraSkill value {v:?}"));
            };
            if matches!(r.get("name"), Some(ModValue::Str(n)) if &**n == "Unknown") {
                continue;
            }
            let opt_num = |k: &str| r.get(k).and_then(ModValue::as_number);
            self.granted_skills.push(GrantedSkill {
                skill_id: match r.get("skillId") {
                    Some(ModValue::Str(s)) => Some(s.clone()),
                    _ => None,
                },
                level: opt_num("level"),
                no_supports: r.get("noSupports").is_some_and(ModValue::truthy),
                source: source.clone(),
                triggered: r.get("triggered").is_some_and(ModValue::truthy),
                trigger_chance: opt_num("triggerChance"),
            });
        }
        let socket_count = calc_num(&mut base_list, "SocketCount", ModType::Base, 0)?;
        self.abyssal_socket_count =
            calc_num(&mut base_list, "AbyssalSocketCount", ModType::Base, 0)?;
        self.selectable_socket_count = base
            .socket_limit
            .unwrap_or(0.0)
            .max(self.sockets.len() as f64)
            - self.abyssal_socket_count;
        if calc_flag(&mut base_list, "NoSockets")? {
            self.sockets.clear();
            self.selectable_socket_count = 0.0;
            self.abyssal_socket_count = 0.0;
        } else if socket_count > 0.0 {
            // Exactly the stated number of sockets
            self.selectable_socket_count = socket_count;
            let mut group = 0.0;
            let total = socket_count.max(self.sockets.len() as f64) as usize;
            let mut sockets = Vec::new();
            for i in 1..=total {
                if i as f64 > socket_count {
                    break;
                } else if let Some(s) = self.sockets.get(i - 1) {
                    group = s.group;
                    sockets.push(s.clone());
                } else {
                    sockets.push(Socket {
                        color: self.default_socket_color.unwrap_or('R'),
                        group,
                    });
                }
            }
            self.sockets = sockets;
        } else if self.abyssal_socket_count > 0.0 {
            // The right number of abyssal sockets
            let mut new = Vec::new();
            let mut group = 0.0;
            for s in &self.sockets {
                if s.color != 'A' {
                    if new.len() as f64 >= self.selectable_socket_count {
                        break;
                    }
                    new.push(s.clone());
                    group = s.group;
                }
            }
            let mut i = 1.0;
            while i <= self.abyssal_socket_count {
                group += 1.0;
                new.push(Socket { color: 'A', group });
                i += 1.0;
            }
            self.sockets = new;
        }
        if calc_flag(&mut base_list, "SocketAlwaysMatches")? {
            self.colour_always_matches = true;
        }
        self.socketed_jewel_effect_modifier =
            1.0 + calc_num(&mut base_list, "SocketedJewelEffect", ModType::Inc, 0)? / 100.0;
        if base.weapon.is_some() || self.typ.as_deref() == Some("Ring") {
            let mut lists = vec![];
            for slot in 1..=2 {
                lists.push(self.build_mod_list_for_slot_num(ctx, &mut base_list, Some(slot))?);
            }
            if self.typ.as_deref() == Some("Ring") {
                lists.push(self.build_mod_list_for_slot_num(ctx, &mut base_list, Some(3))?);
            }
            self.slot_mod_list = Some(lists);
            self.mod_list = None;
        } else {
            // Upstream passes no slot number here, which reads as slot "nil"
            self.mod_list = Some(self.build_mod_list_for_slot_num(ctx, &mut base_list, None)?);
            self.slot_mod_list = None;
        }
        // The slot lists take local flask flags out of it
        self.base_mod_list = base_list;
        Ok(())
    }

    /// `processModLine` of `BuildModList`
    fn process_mod_line(
        &mut self,
        ctx: &ItemContext,
        ml: &mut ModLine,
        base_list: &mut Vec<Mod>,
    ) -> Result<()> {
        if ml.flags.disabled {
            return Ok(());
        }
        let count = self.mod_line_variant_count(ml);
        if count == 0 {
            return Ok(());
        }
        // Variants can override the class restriction
        if ml.line.contains("Requires Class") {
            self.class_restriction = capture(
                &gsub(&ml.line, "{variant:([%d,]+)}", ""),
                "Requires Class (.+)",
            );
        }
        let ranged = if ml.extra.is_none() {
            self.ranged_mod_list(ctx, ml)?
        } else {
            None
        };
        if let Some(list) = ranged {
            ml.mod_list = list;
            ml.show_slider = true;
        }
        if ml.extra.is_none() {
            for m in &mut ml.mod_list {
                set_source(m, &self.mod_source);
                for _ in 0..count {
                    base_list.push(m.clone());
                }
            }
            if !ml.mod_tags.is_empty() {
                self.has_mod_tags = true;
            }
        }
        Ok(())
    }

    /// `BuildModListForSlotNum(baseList, slotNum)`: the mods in a slot, with
    /// local mods applied and quality added. Without a slot number, the
    /// primary slot's `1` becomes `nil`, as upstream's `tostring(nil)`.
    fn build_mod_list_for_slot_num(
        &mut self,
        ctx: &ItemContext,
        base_list: &mut Vec<Mod>,
        slot: Option<usize>,
    ) -> Result<Vec<Mod>> {
        let base = self.base.clone().ok_or("no base")?;
        let mut slot_name = self.primary_slot();
        if slot != Some(1) {
            slot_name = slot_name.replace('1', &slot.map_or("nil".to_string(), |n| n.to_string()));
        }
        let slot_number = Sym::new("SlotNumber");
        let in_slot = Sym::new("InSlot");
        let hand = if slot == Some(1) {
            "MainHand"
        } else {
            "OffHand"
        };
        let other = if slot == Some(1) { "2" } else { "1" };
        let subst = |s: &str| -> Option<String> {
            s.contains('{').then(|| {
                s.replace("{SlotName}", &slot_name)
                    .replace("{Hand}", hand)
                    .replace("{OtherSlotNum}", other)
            })
        };
        let mut mods = Vec::new();
        'mods: for base_mod in base_list.iter() {
            let mut m = base_mod.clone();
            for tag in &mut m.tags {
                if let TagKind::Other { kind, fields } = &tag.kind
                    && (*kind == slot_number || *kind == in_slot)
                    && fields.get("num").and_then(ModValue::as_number) != slot.map(|n| n as f64)
                {
                    continue 'mods;
                }
                tag.map_strings(&subst);
            }
            m.source_slot = Some(slot_name.as_str().into());
            mods.push(m);
        }
        if !self.sockets.is_empty() {
            let multi = |c: char| -> Option<String> {
                let color = match c {
                    'R' => "Red",
                    'G' => "Green",
                    'B' => "Blue",
                    'W' => "White",
                    _ => return None,
                };
                Some(format!("Multiplier:{color}SocketIn{slot_name}"))
            };
            let mut group_counts: Vec<(f64, usize)> = Vec::new();
            for s in &self.sockets {
                match group_counts.iter_mut().find(|(g, _)| *g == s.group) {
                    Some((_, c)) => *c += 1,
                    None => group_counts.push((s.group, 1)),
                }
                if let Some(name) = multi(s.color) {
                    mods.push(new_mod(
                        &name,
                        ModType::Base,
                        n(1.0),
                        "Item Sockets",
                        vec![],
                    ));
                }
            }
            let unlinked = group_counts.iter().filter(|(_, c)| *c == 1).count();
            mods.push(new_mod(
                &format!("Multiplier:UnlinkedSocketIn{slot_name}"),
                ModType::Base,
                n(unlinked as f64),
                "Unlinked Item Sockets",
                vec![],
            ));
        }
        let crafted_quality = calc_num(&mut mods, "Quality", ModType::Base, 0)?;
        if Some(crafted_quality) != self.crafted_quality {
            if let Some(old) = self.crafted_quality {
                self.quality = Some(self.quality.unwrap_or(0.0) - old + crafted_quality);
            }
            self.crafted_quality = Some(crafted_quality);
        }
        if let Some(q) = self.quality {
            mods.push(new_mod(
                &format!("Multiplier:QualityOn{slot_name}"),
                ModType::Base,
                n(q),
                "Quality",
                vec![],
            ));
        }
        let quality = self.quality;
        let qnum = || quality.ok_or("attempt to perform arithmetic on a nil value (quality)");
        if let Some(weapon) = &base.weapon {
            let w = |k: &str| weapon.get(k).copied();
            let attack = MOD_FLAG_ATTACK;
            let mut wd = Fields::new();
            wd.insert("type".into(), ModValue::Str(base.typ.as_str().into()));
            if let Some(s) = &base.sub_type {
                wd.insert("subType".into(), ModValue::Str(s.as_str().into()));
            }
            wd.insert("name".into(), ModValue::Str(self.name.as_str().into()));
            let speed = calc_num(&mut mods, "Speed", ModType::Inc, attack)?
                + (qnum()? / 8.0
                    * calc_num(
                        &mut mods,
                        "AlternateQualityLocalAttackSpeedPer8Quality",
                        ModType::Inc,
                        0,
                    )?)
                .floor();
            wd.insert("AttackSpeedInc".into(), n(speed));
            let rate_base = w("AttackRateBase").ok_or("AttackRateBase")?;
            let rate = round(rate_base * (1.0 + speed / 100.0), Some(2.0));
            wd.insert("AttackRate".into(), n(rate));
            let range_bonus = calc_num(&mut mods, "WeaponRange", ModType::Base, 0)?
                + 10.0 * calc_num(&mut mods, "WeaponRangeMetre", ModType::Base, 0)?
                + (qnum()? / 10.0
                    * calc_num(
                        &mut mods,
                        "AlternateQualityLocalWeaponRangePer10Quality",
                        ModType::Base,
                        0,
                    )?)
                .floor();
            wd.insert("rangeBonus".into(), n(range_bonus));
            wd.insert("range".into(), n(w("Range").ok_or("Range")? + range_bonus));
            let local_inc_ele = calc_num(&mut mods, "LocalElementalDamage", ModType::Inc, 0)?;
            let mut elemental_dps = None;
            for dmg in DMG_TYPES {
                let mut min = w(&format!("{dmg}Min")).unwrap_or(0.0)
                    + calc_num(&mut mods, &format!("{dmg}Min"), ModType::Base, 0)?;
                let mut max = w(&format!("{dmg}Max")).unwrap_or(0.0)
                    + calc_num(&mut mods, &format!("{dmg}Max"), ModType::Base, 0)?;
                if dmg == "Physical" {
                    let phys_inc = calc_num(&mut mods, "PhysicalDamage", ModType::Inc, 0)?;
                    let mut qs = qnum()?;
                    if calc_num(&mut mods, "AlternateQualityWeapon", ModType::Base, 0)? > 0.0 {
                        qs = 0.0;
                    }
                    min = round(min * (1.0 + phys_inc / 100.0) * (1.0 + qs / 100.0), None);
                    max = round(max * (1.0 + phys_inc / 100.0) * (1.0 + qs / 100.0), None);
                } else if dmg != "Chaos" {
                    let inc = calc_num(&mut mods, &format!("Local{dmg}Damage"), ModType::Inc, 0)?
                        + local_inc_ele;
                    min = round(min * (1.0 + inc / 100.0), None);
                    max = round(max * (1.0 + inc / 100.0), None);
                }
                if min > 0.0 && max > 0.0 {
                    wd.insert(format!("{dmg}Min"), n(min));
                    wd.insert(format!("{dmg}Max"), n(max));
                    let dps = (min + max) / 2.0 * rate;
                    wd.insert(format!("{dmg}DPS"), n(dps));
                    if dmg != "Physical" && dmg != "Chaos" {
                        elemental_dps = Some(elemental_dps.unwrap_or(0.0) + dps);
                    }
                }
            }
            if let Some(e) = elemental_dps {
                wd.insert("ElementalDPS".into(), n(e));
            }
            let crit_base = calc_num(&mut mods, "CritChance", ModType::Base, 0)?;
            let crit_inc = calc_num(&mut mods, "CritChance", ModType::Inc, 0)?;
            let crit_alt = (qnum()? / 4.0
                * calc_num(
                    &mut mods,
                    "AlternateQualityLocalCritChancePer4Quality",
                    ModType::Inc,
                    0,
                )?)
            .floor();
            wd.insert(
                "CritChance".into(),
                n(round(
                    (w("CritChanceBase").ok_or("CritChanceBase")? + crit_base)
                        * (1.0 + (crit_inc + crit_alt) / 100.0),
                    Some(2.0),
                )),
            );
            set_keyed(&mut wd, list_values(&mods, "WeaponData")?)?;
            let spell = MOD_FLAG_SPELL;
            let kw_attack = KEYWORD_FLAG_ATTACK;
            let slot_num = slot.ok_or("weapon without a slot number")?;
            let hand_attack = if slot_num == 1 {
                "MainHandAttack"
            } else {
                "OffHandAttack"
            };
            for m in &mut mods {
                let name = m.name.as_str();
                // Accuracy, life and mana on hit and leech become local
                let local = ((name == "Accuracy" && m.flags == 0)
                    || (name == "ImpaleChance" && m.flags != spell)
                    || ((name == "LifeOnHit" || name == "ManaOnHit") && m.flags == attack)
                    || ((name == "PhysicalDamageLifeLeech" || name == "PhysicalDamageManaLeech")
                        && m.flags == attack))
                    && (m.keyword_flags == 0 || m.keyword_flags == kw_attack)
                    && m.tags.is_empty();
                let ailment = (name == "PoisonChance" || name == "BleedChance")
                    && m.flags != spell
                    && (m.tags.is_empty()
                        || (m.tags.len() == 1
                            && matches!(&m.tags[0].kind, TagKind::Condition { vars: Names::One(v), .. } if v.as_str() == "CriticalStrike")));
                if local || ailment {
                    m.tags.push(condition(hand_attack, false));
                }
            }
            let total: f64 = DMG_TYPES
                .iter()
                .map(|d| field_num(&wd, &format!("{d}DPS")).unwrap_or(0.0))
                .fold(0.0, |a, b| a + b);
            wd.insert("TotalDPS".into(), n(total));
            self.weapon_data
                .get_or_insert_with(BTreeMap::new)
                .insert(slot_num, wd);
        } else if let Some(armour) = &base.armour {
            self.local_armour(armour, &mut mods, qnum()?)?;
        } else if let Some(flask) = &base.flask {
            let mut fd = self.flask_data.take().unwrap_or_default();
            let duration_inc = calc_num(&mut mods, "Duration", ModType::Inc, 0)?;
            let duration_more = calc_num(&mut mods, "Duration", ModType::More, 0)?;
            let q = qnum()?;
            if flask.life.is_some() || flask.mana.is_some() {
                // Recovery flask
                let instant = calc_num(&mut mods, "FlaskInstantRecovery", ModType::Base, 0)?;
                fd.insert("instantPerc".into(), n(instant));
                fd.insert(
                    "instantLowLifePerc".into(),
                    n(calc_num(
                        &mut mods,
                        "FlaskLowLifeInstantRecovery",
                        ModType::Base,
                        0,
                    )?),
                );
                let recovery = 1.0 + calc_num(&mut mods, "FlaskRecovery", ModType::Inc, 0)? / 100.0;
                let rate = 1.0 + calc_num(&mut mods, "FlaskRecoveryRate", ModType::Inc, 0)? / 100.0;
                fd.insert(
                    "duration".into(),
                    n(round(
                        flask.duration * (1.0 + duration_inc / 100.0) / rate * duration_more,
                        Some(1.0),
                    )),
                );
                if let Some(life) = flask.life {
                    let b = life * (1.0 + q / 100.0) * recovery;
                    let inst = b * instant / 100.0;
                    let grad = b * (1.0 - instant / 100.0);
                    fd.insert("lifeBase".into(), n(b));
                    fd.insert("lifeInstant".into(), n(inst));
                    fd.insert("lifeGradual".into(), n(grad));
                    fd.insert("lifeTotal".into(), n(inst + grad));
                    fd.insert(
                        "lifeAdditional".into(),
                        n(calc_num(
                            &mut mods,
                            "FlaskAdditionalLifeRecovery",
                            ModType::Base,
                            0,
                        )?),
                    );
                    fd.insert(
                        "lifeEffectNotRemoved".into(),
                        calc_local(base_list, "LifeFlaskEffectNotRemoved", ModType::Flag, 0)?,
                    );
                }
                if let Some(mana) = flask.mana {
                    let b = mana * (1.0 + q / 100.0) * recovery;
                    let inst = b * instant / 100.0;
                    let grad = b * (1.0 - instant / 100.0);
                    fd.insert("manaBase".into(), n(b));
                    fd.insert("manaInstant".into(), n(inst));
                    fd.insert("manaGradual".into(), n(grad));
                    fd.insert("manaTotal".into(), n(inst + grad));
                    fd.insert(
                        "manaEffectNotRemoved".into(),
                        calc_local(base_list, "ManaFlaskEffectNotRemoved", ModType::Flag, 0)?,
                    );
                }
            } else {
                // Utility flask
                fd.insert(
                    "duration".into(),
                    n(round(
                        flask.duration
                            * (1.0 + duration_inc / 100.0)
                            * (1.0 + q / 100.0)
                            * duration_more,
                        Some(1.0),
                    )),
                );
            }
            fd.insert(
                "chargesMax".into(),
                n(flask.charges_max + calc_num(&mut mods, "FlaskCharges", ModType::Base, 0)?),
            );
            fd.insert(
                "chargesUsed".into(),
                n((flask.charges_used
                    * (1.0 + calc_num(&mut mods, "FlaskChargesUsed", ModType::Inc, 0)? / 100.0))
                    .floor()),
            );
            fd.insert(
                "gainMod".into(),
                n(1.0 + calc_num(&mut mods, "FlaskChargeRecovery", ModType::Inc, 0)? / 100.0),
            );
            let effect = calc_num(&mut mods, "FlaskEffect", ModType::Inc, 0)?
                + calc_num(&mut mods, "LocalEffect", ModType::Inc, 0)?;
            fd.insert("effectInc".into(), n(effect));
            set_keyed(&mut fd, list_values(&mods, "FlaskData")?)?;
            self.flask_data = Some(fd);
        } else if let Some(tincture) = &base.tincture {
            let mut td = self.tincture_data.take().unwrap_or_default();
            let burn_inc = calc_num(&mut mods, "TinctureManaBurnRate", ModType::Inc, 0)?;
            let burn_more = calc_num(&mut mods, "TinctureManaBurnRate", ModType::More, 0)?;
            td.insert(
                "manaBurn".into(),
                n((tincture.mana_burn + 0.01)
                    / (1.0 + burn_inc / 100.0)
                    / (1.0 + burn_more / 100.0)),
            );
            let cd_inc = calc_num(&mut mods, "TinctureCooldownRecovery", ModType::Inc, 0)?
                + calc_num(&mut mods, "CooldownRecovery", ModType::Inc, 0)?;
            td.insert("cooldownInc".into(), n(cd_inc));
            td.insert(
                "cooldown".into(),
                n(tincture.cooldown / (1.0 + cd_inc / 100.0)),
            );
            let effect = calc_num(&mut mods, "TinctureEffect", ModType::Inc, 0)?
                + calc_num(&mut mods, "LocalEffect", ModType::Inc, 0)?;
            td.insert("effectInc".into(), n(effect));
            set_keyed(&mut td, list_values(&mods, "TinctureData")?)?;
            self.tincture_data = Some(td);
        } else if self.typ.as_deref() == Some("Jewel") {
            self.local_jewel(ctx, &mut mods)?;
        }
        Ok(mods)
    }

    /// The armour part of `BuildModListForSlotNum`
    fn local_armour(
        &mut self,
        armour: &HashMap<String, f64>,
        mods: &mut Vec<Mod>,
        quality: f64,
    ) -> Result<()> {
        let a = |k: &str| armour.get(k).copied().unwrap_or(0.0);
        let mut c = |name: &str, typ: ModType| calc_num(mods, name, typ, 0);
        let armour_base = c("Armour", ModType::Base)? + a("ArmourBaseMin");
        let armour_var = a("ArmourBaseMax") - a("ArmourBaseMin");
        let armour_evasion_base = c("ArmourAndEvasion", ModType::Base)?;
        let evasion_base = c("Evasion", ModType::Base)? + a("EvasionBaseMin");
        let evasion_var = a("EvasionBaseMax") - a("EvasionBaseMin");
        let evasion_es_base = c("EvasionAndEnergyShield", ModType::Base)?;
        let es_base = c("EnergyShield", ModType::Base)? + a("EnergyShieldBaseMin");
        let es_var = a("EnergyShieldBaseMax") - a("EnergyShieldBaseMin");
        let armour_es_base = c("ArmourAndEnergyShield", ModType::Base)?;
        let ward_base = c("Ward", ModType::Base)? + a("WardBaseMin");
        let ward_var = a("WardBaseMax") - a("WardBaseMin");
        let armour_inc = c("Armour", ModType::Inc)?;
        let armour_evasion_inc = c("ArmourAndEvasion", ModType::Inc)?;
        let evasion_inc = c("Evasion", ModType::Inc)?;
        let evasion_es_inc = c("EvasionAndEnergyShield", ModType::Inc)?;
        let es_inc = c("EnergyShield", ModType::Inc)?;
        let ward_inc = c("Ward", ModType::Inc)?;
        let armour_es_inc = c("ArmourAndEnergyShield", ModType::Inc)?;
        let defences_inc = c("Defences", ModType::Inc)?;
        let mut qs = quality;
        if c("AlternateQualityArmour", ModType::Base)? > 0.0 {
            qs = 0.0;
        }
        let ad = self.armour_data.get_or_insert_with(Fields::new);
        let get = |ad: &Fields, k: &str| field_num(ad, k);
        // Base percentiles differ by defence, which are weighted differently
        let percentile =
            |ad: &mut Fields, stat: &str, inc: f64, base: f64, var: f64| -> Result<()> {
                let key = format!("{stat}BasePercentile");
                if let Some(v) = get(ad, stat)
                    && v > 0.0
                    && !ad.contains_key(&key)
                {
                    let p = ((v / ((1.0 + inc / 100.0) * (1.0 + qs / 100.0))) - base) / var;
                    ad.insert(key, n(round(p.min(1.0).max(0.0), Some(4.0))));
                }
                Ok(())
            };
        let armour_incs = armour_inc + armour_evasion_inc + armour_es_inc + defences_inc;
        let evasion_incs = evasion_inc + armour_evasion_inc + evasion_es_inc + defences_inc;
        let es_incs = es_inc + armour_es_inc + evasion_es_inc + defences_inc;
        let ward_incs = ward_inc + defences_inc;
        percentile(ad, "Armour", armour_incs, armour_base, armour_var)?;
        percentile(ad, "Evasion", evasion_incs, evasion_base, evasion_var)?;
        percentile(ad, "EnergyShield", es_incs, es_base, es_var)?;
        percentile(ad, "Ward", ward_incs, ward_base, ward_var)?;
        let pct =
            |ad: &Fields, stat: &str| get(ad, &format!("{stat}BasePercentile")).unwrap_or(1.0);
        let q = 1.0 + qs / 100.0;
        let armour_v = round(
            (armour_base + armour_evasion_base + armour_es_base + armour_var * pct(ad, "Armour"))
                * (1.0 + armour_incs / 100.0)
                * q,
            None,
        );
        let evasion_v = round(
            (evasion_base
                + armour_evasion_base
                + evasion_es_base
                + evasion_var * pct(ad, "Evasion"))
                * (1.0 + evasion_incs / 100.0)
                * q,
            None,
        );
        let es_v = round(
            (es_base + evasion_es_base + armour_es_base + es_var * pct(ad, "EnergyShield"))
                * (1.0 + es_incs / 100.0)
                * q,
            None,
        );
        let ward_v = round(
            (ward_base + ward_var * pct(ad, "Ward")) * (1.0 + ward_incs / 100.0) * q,
            None,
        );
        for (stat, v) in [
            ("Armour", armour_v),
            ("Evasion", evasion_v),
            ("EnergyShield", es_v),
            ("Ward", ward_v),
        ] {
            ad.insert(stat.into(), n(v));
        }
        for (stat, v) in [
            ("Armour", armour_v),
            ("Evasion", evasion_v),
            ("EnergyShield", es_v),
            ("Ward", ward_v),
        ] {
            let key = format!("{stat}BasePercentile");
            if !ad.contains_key(&key) && v > 0.0 {
                ad.insert(key, n(1.0));
            }
        }
        if let Some(&block) = armour.get("BlockChance") {
            let b = calc_num(mods, "BlockChance", ModType::Base, 0)?;
            let inc = calc_num(mods, "BlockChance", ModType::Inc, 0)?;
            let ad = self.armour_data.as_mut().unwrap();
            ad.insert(
                "BlockChance".into(),
                n(((block + b) * (1.0 + inc / 100.0)).floor()),
            );
        }
        if let Some(&penalty) = armour.get("MovementPenalty") {
            mods.push(new_mod(
                "MovementSpeed",
                ModType::Inc,
                n(-penalty),
                &self.mod_source,
                vec![condition("IgnoreMovementPenalties", true)],
            ));
        }
        let keyed = list_values(mods, "ArmourData")?;
        set_keyed(self.armour_data.as_mut().unwrap(), keyed)?;
        Ok(())
    }

    /// The jewel part of `BuildModListForSlotNum`
    fn local_jewel(&mut self, ctx: &ItemContext, mods: &mut Vec<Mod>) -> Result<()> {
        if self.name.contains("Grand Spectrum") {
            let spectrum = new_mod(
                "Multiplier:GrandSpectrum",
                ModType::Base,
                n(1.0),
                &self.name,
                vec![],
            );
            mods.push(spectrum.clone());
            mods.push(new_mod(
                "MinionModifier",
                ModType::List,
                ModValue::Record(Record {
                    fields: vec![(Sym::new("mod"), ModValue::Mod(Box::new(spectrum)))],
                }),
                &self.name,
                vec![],
            ));
        }
        let mut jd = self.jewel_data.take().unwrap_or_default();
        let funcs = list_values(mods, "JewelFunc")?;
        if !funcs.is_empty() {
            let list = match jd.remove("funcList") {
                Some(ModValue::List(l)) => l,
                _ => vec![],
            };
            jd.insert(
                "funcList".into(),
                ModValue::List(list.into_iter().chain(funcs).collect()),
            );
        }
        set_keyed(&mut jd, list_values(mods, "JewelData")?)?;
        // `List` always gives a table, so upstream always sets it
        let mut keystones = Fields::new();
        set_keyed(
            &mut keystones,
            list_values(mods, "ImpossibleEscapeKeystones")?,
        )?;
        jd.insert(
            "impossibleEscapeKeystones".into(),
            ModValue::Record(Record {
                fields: keystones
                    .into_iter()
                    .map(|(k, v)| (Sym::new(&k), v))
                    .collect(),
            }),
        );
        if let Some(cj) = self
            .base_name
            .as_ref()
            .and_then(|b| ctx.data.cluster_jewels.get(b))
        {
            let notables = list_values(mods, "ClusterJewelNotable")?;
            jd.insert("clusterJewelNotables".into(), ModValue::List(notables));
            let added = list_values(mods, "AddToClusterJewelNode")?;
            jd.insert("clusterJewelAddedMods".into(), ModValue::List(added));
            // Small and medium curse clusters parse the same; the skills differ
            let skill = |jd: &Fields| match jd.get("clusterJewelSkill") {
                Some(ModValue::Str(s)) => Some(s.to_string()),
                _ => None,
            };
            if cj.size == "Small" && skill(&jd).as_deref() == Some("affliction_curse_effect") {
                jd.insert(
                    "clusterJewelSkill".into(),
                    ModValue::Str("affliction_curse_effect_small".into()),
                );
            } else if cj.size == "Medium"
                && skill(&jd).as_deref() == Some("affliction_curse_effect_small")
            {
                jd.insert(
                    "clusterJewelSkill".into(),
                    ModValue::Str("affliction_curse_effect".into()),
                );
            }
            if let Some(c) = field_num(&jd, "clusterJewelNodeCount") {
                jd.insert(
                    "clusterJewelNodeCount".into(),
                    n(cj.max_nodes.min(c.max(cj.min_nodes))),
                );
            }
            if let Some(s) = skill(&jd)
                && !cj.skills.contains(&s)
            {
                jd.remove("clusterJewelSkill");
            }
            // Crafting fields advanced copy leaves out
            if self.cluster_jewel_skill.is_none() {
                self.cluster_jewel_skill = skill(&jd);
            }
            if self.cluster_jewel_node_count.is_none() {
                self.cluster_jewel_node_count = field_num(&jd, "clusterJewelNodeCount");
            }
            // `keystone or ((skill or smalls) and count) or (override and
            // nothingness)`, with Lua's `and`/`or` giving operands
            let get = |k: &str| jd.get(k).cloned();
            let or = |a: Option<ModValue>, b: Option<ModValue>| {
                if a.as_ref().is_some_and(ModValue::truthy) {
                    a
                } else {
                    b
                }
            };
            let and = |a: Option<ModValue>, b: Option<ModValue>| {
                if a.as_ref().is_some_and(ModValue::truthy) {
                    b
                } else {
                    a
                }
            };
            let valid = or(
                or(
                    get("clusterJewelKeystone"),
                    and(
                        or(
                            get("clusterJewelSkill"),
                            get("clusterJewelSmallsAreNothingness"),
                        ),
                        get("clusterJewelNodeCount"),
                    ),
                ),
                and(
                    get("clusterJewelSocketCountOverride"),
                    get("clusterJewelNothingnessCount"),
                ),
            );
            if let Some(v) = valid {
                jd.insert("clusterJewelValid".into(), v);
            }
        }
        self.jewel_data = Some(jd);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_numbers() {
        assert_eq!(spec_to_number("+20% (augmented)"), Some(20.0));
        assert_eq!(spec_to_number("abc"), None);
    }
}
