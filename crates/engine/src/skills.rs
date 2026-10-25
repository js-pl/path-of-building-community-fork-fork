//! Socket groups and the gems in them: what upstream's Skills tab
//! (`Classes/SkillsTab.lua`) loads from a build and processes for the
//! calculations, and the gem helpers of `Modules/CalcTools.lua` it uses.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

// `m_max(m_min(x, hi), lo)` as upstream writes it, which `clamp` is not for NaN
#![allow(clippy::manual_clamp)]

use crate::item::sanitise_text;
use crate::pat::{self, gsub, gsub_fn};
use crate::skill_data::{Gem, GrantedEffect, SkillData};
use crate::xml::Element;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// A gem in a socket group.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GemInstance {
    pub name_spec: String,
    pub gem_id: Option<String>,
    pub skill_id: Option<String>,
    /// The gem, for skill gems
    pub gem: Option<Rc<Gem>>,
    /// The granted effect of a skill no gem grants (from an item)
    pub granted_effect: Option<Rc<GrantedEffect>>,
    pub level: Option<f64>,
    pub quality: Option<f64>,
    pub enabled: bool,
    pub enable_global1: bool,
    pub enable_global2: bool,
    pub count: f64,
    pub skill_part: Option<f64>,
    pub skill_part_calcs: Option<f64>,
    pub skill_stage_count: Option<f64>,
    pub skill_stage_count_calcs: Option<f64>,
    pub skill_mine_count: Option<f64>,
    pub skill_mine_count_calcs: Option<f64>,
    pub skill_minion: Option<String>,
    pub skill_minion_calcs: Option<String>,
    pub skill_minion_item_set: Option<f64>,
    pub skill_minion_item_set_calcs: Option<f64>,
    pub skill_minion_skill: Option<f64>,
    pub skill_minion_skill_calcs: Option<f64>,
    pub err_msg: Option<String>,
    /// The colour code of the gem's colour
    pub color: String,
    pub natural_max_level: Option<f64>,
    pub req_level: Option<f64>,
    pub req_str: Option<f64>,
    pub req_dex: Option<f64>,
    pub req_int: Option<f64>,
}

impl GemInstance {
    /// The granted effect of the gem or skill
    pub fn effect(&self) -> Option<&Rc<GrantedEffect>> {
        self.granted_effect
            .as_ref()
            .or(self.gem.as_ref().map(|g| &g.granted_effect))
    }
}

/// A socket group: linked gems, or the skills an item grants.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SocketGroup {
    pub enabled: bool,
    pub include_in_full_dps: bool,
    pub group_count: Option<f64>,
    pub label: Option<String>,
    pub slot: Option<String>,
    pub source: Option<String>,
    pub main_active_skill: f64,
    pub main_active_skill_calcs: f64,
    pub imbued_support: Option<String>,
    pub gem_list: Vec<GemInstance>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillSet {
    pub id: f64,
    pub title: Option<String>,
    pub socket_group_list: Vec<SocketGroup>,
}

/// How gems without a level get one (`defaultGemLevel`)
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DefaultGemLevel {
    NormalMaximum,
    CorruptedMaximum,
    AwakenedMaximum,
    CharacterLevel,
    LevelOne,
}

impl DefaultGemLevel {
    fn from_name(name: &str) -> Option<DefaultGemLevel> {
        Some(match name {
            "normalMaximum" => DefaultGemLevel::NormalMaximum,
            "corruptedMaximum" => DefaultGemLevel::CorruptedMaximum,
            "awakenedMaximum" => DefaultGemLevel::AwakenedMaximum,
            "characterLevel" => DefaultGemLevel::CharacterLevel,
            "levelOne" => DefaultGemLevel::LevelOne,
            _ => return None,
        })
    }
}

/// The Skills tab of a build: its skill sets and gem defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Skills {
    pub default_gem_level: DefaultGemLevel,
    pub default_gem_quality: f64,
    pub skill_sets: BTreeMap<i64, SkillSet>,
    pub skill_set_order_list: Vec<f64>,
    pub active_skill_set_id: f64,
    /// The supports imbued into each slot's gems, by granted effect id
    pub imbued_support_by_slot: HashMap<String, String>,
}

fn num(v: Option<&str>) -> Option<f64> {
    v.and_then(pat::to_number)
}

/// The colour code of a gem colour
fn color_code(data: &SkillData, color: Option<f64>) -> String {
    let i = match color {
        Some(1.0) => 0,
        Some(2.0) => 1,
        Some(3.0) => 2,
        _ => 3,
    };
    data.color_codes[i].clone()
}

/// `calcLib.getGemStatRequirement(level, isSupport, multi)`
pub fn gem_stat_requirement(level: f64, support: bool, multi: f64) -> f64 {
    if multi == 0.0 {
        return 0.0;
    }
    let stat_type = if support { 0.5 } else { 0.7 };
    let req = ((20.0 + (level - 3.0) * 3.0) * (multi / 100.0).powf(0.9) * stat_type + 0.5).floor();
    if req < 14.0 { 0.0 } else { req }
}

/// `calcLib.validateGemLevel(gemInstance)`: a level the gem has
pub fn validate_gem_level(gem: &mut GemInstance) {
    let Some(effect) = gem.effect().cloned() else {
        return;
    };
    let has = |level: Option<f64>| level.is_some_and(|l| effect.level(l).is_some());
    if !has(gem.level) {
        // Within the levels of the skill
        let mut level = gem.level.map_or(f64::NAN, |l| 1f64.max(l));
        let count = effect.level_count();
        if count > 0 {
            level = level.min(count as f64);
        }
        gem.level = Some(level);
    }
    if !has(gem.level)
        && let Some(n) = gem.gem.as_ref().map(|g| g.natural_max_level)
    {
        gem.level = Some(n);
    }
    if !has(gem.level) {
        // Any level, the first upstream's `next` finds
        gem.level = effect.levels.keys().next().map(|&l| l as f64);
    }
}

impl Skills {
    /// `SkillsTabClass:Load(xml)`: the skill sets of a build's `<Skills>`.
    pub fn load(xml: &Element, data: &SkillData) -> Skills {
        let default_gem_level = if xml.attr("matchGemLevelToCharacterLevel") == Some("true") {
            DefaultGemLevel::CharacterLevel
        } else {
            match xml.attr("defaultGemLevel") {
                Some(v) if pat::to_number(v).is_none() => {
                    // An unknown value keeps the list's selection, its first
                    DefaultGemLevel::from_name(v).unwrap_or(DefaultGemLevel::NormalMaximum)
                }
                _ => DefaultGemLevel::NormalMaximum,
            }
        };
        let mut skills = Skills {
            default_gem_level,
            default_gem_quality: num(xml.attr("defaultGemQuality"))
                .unwrap_or(0.0)
                .min(23.0)
                .max(0.0),
            skill_sets: BTreeMap::new(),
            skill_set_order_list: vec![],
            active_skill_set_id: 0.0,
            imbued_support_by_slot: HashMap::new(),
        };
        let ctx = Ctx { data };
        for node in xml.elements() {
            if node.elem == "Skill" {
                // The old format: one set
                if skills.skill_set_order_list.is_empty() {
                    skills.skill_set_order_list.push(1.0);
                    skills.new_skill_set(1.0);
                }
                if let Some(group) = load_skill(node, &ctx) {
                    skills.set_mut(1.0).socket_group_list.push(group);
                }
            }
            if node.elem == "SkillSet" {
                let id = num(node.attr("id")).unwrap_or(f64::NAN);
                skills.new_skill_set(id);
                skills.set_mut(id).title = node.attr("title").map(String::from);
                skills.skill_set_order_list.push(id);
                for sub in node.elements() {
                    if let Some(group) = load_skill(sub, &ctx) {
                        skills.set_mut(id).socket_group_list.push(group);
                    }
                }
            }
        }
        let active = num(xml.attr("activeSkillSet")).unwrap_or(1.0);
        skills.set_active_skill_set(active, data);
        skills
    }

    fn key(id: f64) -> i64 {
        id as i64
    }

    fn set_mut(&mut self, id: f64) -> &mut SkillSet {
        self.skill_sets
            .get_mut(&Skills::key(id))
            .expect("skill set")
    }

    /// `NewSkillSet(id)`
    fn new_skill_set(&mut self, id: f64) {
        self.skill_sets.insert(
            Skills::key(id),
            SkillSet {
                id,
                title: None,
                socket_group_list: vec![],
            },
        );
    }

    /// The active set's socket groups (`socketGroupList`)
    pub fn socket_group_list(&self) -> &[SocketGroup] {
        self.skill_sets
            .get(&Skills::key(self.active_skill_set_id))
            .map_or(&[], |s| &s.socket_group_list)
    }

    /// `SetActiveSkillSet(id)`
    fn set_active_skill_set(&mut self, id: f64, data: &SkillData) {
        if self.skill_set_order_list.is_empty() {
            self.skill_set_order_list.push(1.0);
            self.new_skill_set(1.0);
        }
        let id = if self.skill_sets.contains_key(&Skills::key(id)) && id.fract() == 0.0 {
            id
        } else {
            self.skill_set_order_list[0]
        };
        self.active_skill_set_id = id;
        // RebuildImbuedSupportBySlot
        let mut imbued = HashMap::new();
        for group in self.socket_group_list() {
            if let (Some(slot), Some(support)) = (&group.slot, &group.imbued_support)
                && let Some(gem) =
                    data.gem_for_base_name(&format!("{} support", support.to_ascii_lowercase()))
            {
                imbued.insert(slot.clone(), gem.granted_effect.id.clone());
            }
        }
        self.imbued_support_by_slot = imbued;
    }
}

struct Ctx<'a> {
    data: &'a SkillData,
}

/// `SkillsTabClass:LoadSkill(node)`
fn load_skill(node: &Element, ctx: &Ctx) -> Option<SocketGroup> {
    if node.elem != "Skill" {
        return None;
    }
    let a = |k: &str| node.attr(k);
    let mut group = SocketGroup {
        enabled: a("active") == Some("true") || a("enabled") == Some("true"),
        include_in_full_dps: a("includeInFullDPS") == Some("true"),
        group_count: num(a("groupCount")),
        label: a("label").map(String::from),
        slot: a("slot").map(String::from),
        source: a("source").map(String::from),
        main_active_skill: num(a("mainActiveSkill")).unwrap_or(1.0),
        main_active_skill_calcs: num(a("mainActiveSkillCalcs")).unwrap_or(1.0),
        imbued_support: None,
        gem_list: vec![],
    };
    if let (Some(support), Some(_)) = (a("imbuedSupport"), a("slot")) {
        group.imbued_support = Some(support.to_string());
    }
    for child in node.elements() {
        let c = |k: &str| child.attr(k);
        let mut gem = GemInstance {
            name_spec: sanitise_text(c("nameSpec").unwrap_or("")),
            ..GemInstance::default()
        };
        if let Some(game_id) = c("gemId") {
            let found = if let Some(variant) = c("variantId") {
                // Since 3.23 the variant (transfiguration) is stored
                ctx.data.gem_by_game_id(game_id, variant)
            } else if let Some(skill_id) = c("skillId") {
                // Before, the granted effect told the variant
                ctx.data
                    .variants_of(game_id)
                    .and_then(|mut v| v.find(|g| g.granted_effect.id == skill_id))
            } else {
                None
            };
            if let Some(g) = found {
                gem.gem_id = Some(g.id.clone());
                gem.skill_id = Some(g.granted_effect.id.clone());
                gem.name_spec = sanitise_text(&g.name);
            }
        } else if let Some(skill_id) = c("skillId")
            && let Some(effect) = ctx.data.skills.get(skill_id)
        {
            gem.gem_id = ctx.data.gem_for_skill(skill_id).map(|g| g.id.clone());
            gem.skill_id = Some(effect.id.clone());
            gem.name_spec = effect.name.clone();
        }
        gem.level = num(c("level"));
        gem.quality = num(c("quality"));
        gem.name_spec = sanitise_text(&gem.name_spec);
        gem.enabled = c("enabled").is_none_or(|e| e == "true");
        gem.enable_global1 = c("enableGlobal1").is_none_or(|e| e == "true");
        gem.enable_global2 = c("enableGlobal2") == Some("true");
        gem.count = num(c("count")).unwrap_or(1.0);
        gem.skill_part = num(c("skillPart"));
        gem.skill_part_calcs = num(c("skillPartCalcs"));
        gem.skill_stage_count = num(c("skillStageCount"));
        gem.skill_stage_count_calcs = num(c("skillStageCountCalcs"));
        gem.skill_mine_count = num(c("skillMineCount"));
        gem.skill_mine_count_calcs = num(c("skillMineCountCalcs"));
        gem.skill_minion = c("skillMinion").map(String::from);
        gem.skill_minion_calcs = c("skillMinionCalcs").map(String::from);
        gem.skill_minion_item_set = num(c("skillMinionItemSet"));
        gem.skill_minion_item_set_calcs = num(c("skillMinionItemSetCalcs"));
        gem.skill_minion_skill = num(c("skillMinionSkill"));
        gem.skill_minion_skill_calcs = num(c("skillMinionSkillCalcs"));
        group.gem_list.push(gem);
    }
    if let Some(part) = a("skillPart")
        && let Some(first) = group.gem_list.first_mut()
    {
        first.skill_part = pat::to_number(part);
    }
    process_socket_group(&mut group, ctx.data);
    Some(group)
}

/// `SkillsTabClass:FindSkillGem(nameSpec)`: the gem a name or abbreviation
/// means, or why none
pub fn find_skill_gem(name_spec: &str, data: &SkillData) -> Result<Rc<Gem>, String> {
    let any_case = |c: &[String]| {
        Some(format!(
            "[{}{}]",
            c[0].to_ascii_uppercase(),
            c[0].to_ascii_lowercase()
        ))
    };
    let no_space = name_spec.replace(' ', "");
    let patterns = [
        // Exact match, any case
        format!("^ {}$", gsub_fn(name_spec, "%a", any_case)),
        // Simple abbreviation ("CtF" -> "Cold to Fire")
        format!("^{}$", gsub(name_spec, "%a", " %0%%l+")),
        // Abbreviated words ("CldFr" -> "Cold to Fire")
        format!("^ {}%l+$", gsub(&no_space, "%l", "%%l*%0")),
        // Global abbreviation ("CtoF" -> "Cold to Fire")
        format!("^{}", gsub(&no_space, "%a", ".*%0")),
        // Any case global abbreviation ("ctof" -> "Cold to Fire")
        format!(
            "^{}",
            gsub_fn(&no_space, "%a", |c| any_case(c).map(|s| format!(".*{s}")))
        ),
    ];
    for pattern in &patterns {
        let mut found: Option<&Rc<Gem>> = None;
        for gem in data.gems() {
            if pat::try_find(&format!(" {}", gem.name), pattern)
                .ok()
                .flatten()
                .is_some()
            {
                if let Some(f) = found {
                    return Err(format!(
                        "Ambiguous gem name '{name_spec}': matches '{}', '{}'",
                        f.name, gem.name
                    ));
                }
                found = Some(gem);
            }
        }
        if let Some(f) = found {
            return Ok(f.clone());
        }
    }
    Err(format!("Unrecognised gem name '{name_spec}'"))
}

/// `SkillsTabClass:ProcessSocketGroup(socketGroup)`: finds each gem's data
/// and settles its level and requirements.
pub fn process_socket_group(group: &mut SocketGroup, data: &SkillData) {
    for gem in &mut group.gem_list {
        gem.color = "^8".into();
        gem.gem = None;
        gem.granted_effect = None;
        if let Some(id) = gem.gem_id.clone() {
            // By gem id: skills granted by gems
            gem.err_msg = None;
            gem.gem = data.gem(&id).cloned();
            if let Some(g) = &gem.gem {
                gem.name_spec = g.name.clone();
                gem.skill_id = Some(g.granted_effect.id.clone());
            }
        } else if let Some(id) = gem.skill_id.clone() {
            // By skill id: skills granted by items. Upstream looks the gem up
            // by id in a table keyed by granted effects, so never finds one
            gem.err_msg = None;
            gem.granted_effect = data.skills.get(&id).cloned();
        } else if pat::is_match(&gem.name_spec, "%S") {
            // By name: builds from before 1.4.20
            match find_skill_gem(&gem.name_spec, data) {
                Ok(g) => {
                    gem.err_msg = None;
                    gem.gem_id = Some(g.id.clone());
                    gem.skill_id = Some(g.granted_effect.id.clone());
                    gem.name_spec = g.name.clone();
                    gem.gem = Some(g);
                }
                Err(e) => {
                    gem.err_msg = Some(e);
                    gem.gem_id = None;
                    gem.skill_id = None;
                }
            }
        } else {
            gem.err_msg = None;
            gem.skill_id = None;
        }
        if gem
            .gem
            .as_ref()
            .is_some_and(|g| g.granted_effect.unsupported)
        {
            gem.err_msg = Some(format!("{} is not supported yet", gem.name_spec));
            gem.gem = None;
        }
        let Some(effect) = gem.effect().cloned() else {
            continue;
        };
        gem.color = color_code(data, effect.color);
        validate_gem_level(gem);
        if let Some(g) = gem.gem.clone() {
            let req_level = gem
                .level
                .and_then(|l| effect.level(l))
                .map(|l| l.level_requirement);
            gem.req_level = req_level;
            if let Some(r) = req_level {
                gem.req_str = Some(gem_stat_requirement(r, effect.support, g.req_str));
                gem.req_dex = Some(gem_stat_requirement(r, effect.support, g.req_dex));
                gem.req_int = Some(gem_stat_requirement(r, effect.support, g.req_int));
            }
        }
    }
}

/// `SkillsTabClass:ProcessGemLevel(gemData)`: the level a new gem gets
pub fn process_gem_level(
    gem: &Gem,
    imbued: bool,
    default: DefaultGemLevel,
    character_level: f64,
) -> f64 {
    let effect = &gem.granted_effect;
    let natural = gem.natural_max_level;
    match default {
        _ if imbued => 1.0,
        DefaultGemLevel::LevelOne => 1.0,
        DefaultGemLevel::AwakenedMaximum => natural + 1.0,
        DefaultGemLevel::CorruptedMaximum => {
            if effect.plus_version_of.is_some() {
                natural
            } else {
                natural + 1.0
            }
        }
        DefaultGemLevel::NormalMaximum => natural,
        DefaultGemLevel::CharacterLevel => {
            let mut max = natural;
            if effect.level(max).is_none() {
                max = effect.level_count() as f64;
            }
            let mut level = max;
            while level >= 1.0 {
                if effect
                    .level(level)
                    .is_some_and(|l| l.level_requirement <= character_level)
                {
                    return level;
                }
                level -= 1.0;
            }
            1.0
        }
    }
}
