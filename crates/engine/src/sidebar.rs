//! What the build mode's sidebar reads of the sidebar's calculation
//! (`calcsTab.mainEnv`) besides the outputs: the main skills' flags and
//! messages, the socket groups' active skills for the skill selection,
//! the item warnings and the aspects in use (`buildMode:RefreshStatList`,
//! `RefreshSkillSelectControls`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::calc::PLAYER;
use crate::calc_items::ItemWarnings;
use crate::calc_perform::{Perform, Result};
use crate::mods::ModValue;
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
    /// `activeSkill.effectList`
    pub effects: Vec<EffectInfo>,
    /// The active effect of its minion's main skill
    pub minion_effect: Option<EffectInfo>,
}

/// An effect of a skill's effect list, as the socket group tooltip
/// shows it
#[derive(Clone, Debug, PartialEq)]
pub struct EffectInfo {
    pub name: String,
    pub color: Option<f64>,
    pub level: Option<f64>,
    pub quality: f64,
    /// The gem it is of (`srcInstance`)
    pub src: Option<(usize, usize)>,
}

impl EffectInfo {
    fn of(e: &crate::calc_skills::SkillEffect) -> EffectInfo {
        EffectInfo {
            name: e.granted_effect.name.clone(),
            color: e.granted_effect.color,
            level: e.level,
            quality: e.quality,
            src: e.src,
        }
    }
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
    /// The gems' `displayEffect`
    pub gem_display: std::collections::HashMap<(usize, usize), crate::calc_skills::GemDisplay>,
    /// The groups' `displayGemList`
    pub display_gem_list: Vec<Option<Vec<(usize, usize)>>>,
    /// The `BASE` strength, dexterity and intelligence of each node the
    /// calculation built a mod list for (`node.finalModList`)
    pub node_attributes: std::collections::HashMap<u32, [f64; 3]>,
    /// What item tooltips read of the player's mod database, by
    /// [`ITEM_QUERIES`]' keys
    pub item_queries: std::collections::HashMap<&'static str, f64>,
    /// The flasks and tinctures in use (`env.flasks`, `env.tinctures`), by
    /// item id
    pub flasks: Vec<i64>,
    pub tinctures: Vec<i64>,
    /// The attributes each radius jewel counted in its socket's radius
    /// (`item.jewelRadiusData[nodeId]`): item id, socket, `Str`, `Dex`,
    /// `Int`
    pub jewel_radius_data: Vec<(f64, u32, [Option<f64>; 3])>,
}

/// The queries of the player's mod database the item tooltips make: how
/// (`BASE`, `INC`, `MORE` sums, `More` products and flags), with the cfg
/// `{ actor = "player" }` or none, and the mod name
pub const ITEM_QUERIES: &[(&str, bool, &str)] = &[
    ("FLAG", false, "OmniscienceRequirements"),
    ("INC", false, "OmniAttributeRequirements"),
    ("INC", false, "FlaskDuration"),
    ("INC", true, "FlaskEffect"),
    ("INC", true, "MagicUtilityFlaskEffect"),
    ("INC", false, "FlaskRecoveryRate"),
    ("INC", false, "FlaskLifeRecovery"),
    ("MORE", false, "FlaskLifeRecovery"),
    ("INC", false, "FlaskLifeRecoveryRate"),
    ("BASE", false, "LifeFlaskInstantRecovery"),
    ("MORE", false, "FlaskLifeRecoveryLowLife"),
    ("FLAG", false, "LifeFlaskAppliesToEnergyShield"),
    ("INC", false, "FlaskManaRecovery"),
    ("INC", false, "FlaskManaRecoveryRate"),
    ("BASE", false, "ManaFlaskInstantRecovery"),
    ("FLAG", false, "ManaFlaskAppliesToLife"),
    ("INC", false, "FlaskChargesUsed"),
    ("INC", false, "FlaskChargesGained"),
    ("BASE", false, "FlaskChargesGenerated"),
    ("BASE", false, "LifeFlaskChargesGenerated"),
    ("BASE", false, "ManaFlaskChargesGenerated"),
    ("BASE", false, "UtilityFlaskChargesGenerated"),
    ("BASE", false, "IronFlaskChargesGeneratedOnWardBreak"),
    ("BASE", false, "FlaskChargesGeneratedPerEmptyFlask"),
    ("BASE", false, "FlaskChanceNotConsumeCharges"),
    ("FLAG", false, "LifeFlaskEffectNotRemoved"),
    ("FLAG", false, "ManaFlaskEffectNotRemoved"),
    ("INC", true, "TinctureEffect"),
    ("INC", true, "MagicTinctureEffect"),
    ("INC", true, "TinctureManaBurnRate"),
    ("SUMMORE", true, "TinctureManaBurnRate"),
    ("BASE", false, "TincturesNotInflictManaBurn"),
    ("INC", true, "TinctureCooldownRecovery"),
];

/// The key of a query of [`ITEM_QUERIES`] in [`Sidebar::item_queries`]
pub fn item_query_key(how: &str, player: bool, name: &str) -> String {
    format!("{how}{}:{name}", if player { "@player" } else { "" })
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
                        // The active effect as building the mod list left it
                        effects: std::iter::once(&skill.active_effect)
                            .chain(skill.effect_list.iter().skip(1))
                            .map(EffectInfo::of)
                            .collect(),
                        minion_effect: minion
                            .map(|m| EffectInfo::of(&p.skill(m.main_skill).active_effect)),
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
        gem_display: gem_display(p),
        display_gem_list: p.env.skills.display_gem_list.clone(),
        node_attributes: node_attributes(&p.env.setup.tree.node_mods)?,
        item_queries: item_queries(p)?,
        flasks: p.active_flasks.clone(),
        tinctures: p.env.setup.items.tinctures.clone(),
        jewel_radius_data: p
            .env
            .setup
            .items
            .radius_jewels
            .iter()
            .map(|r| {
                let v = |k: &str| r.data.values.get(k).copied();
                (r.item_id, r.node_id, [v("Str"), v("Dex"), v("Int")])
            })
            .collect(),
    })
}

/// Each node's `finalModList:Sum("BASE", nil, attrib)` of the attributes
fn node_attributes(
    nodes: &[(u32, crate::calc_setup::NodeMods)],
) -> Result<std::collections::HashMap<u32, [f64; 3]>> {
    use crate::mods::{ModType, list};
    let mut out = std::collections::HashMap::new();
    for (id, n) in nodes {
        let mut a = [0.0; 3];
        for (k, name) in ["Str", "Dex", "Int"].iter().enumerate() {
            a[k] = list::sum(&n.mods, ModType::Base, name)?;
        }
        out.insert(*id, a);
    }
    Ok(out)
}

/// The values of [`ITEM_QUERIES`], keyed by their place in it
fn item_queries(p: &Perform) -> Result<std::collections::HashMap<&'static str, f64>> {
    use crate::mods::{Cfg, ModType, Sym};
    let db = p.c.db(PLAYER);
    let player = Cfg {
        actor: Some(Sym::new("player")),
        ..Cfg::default()
    };
    let mut out = std::collections::HashMap::new();
    for (k, (how, on_player, name)) in ITEM_QUERIES.iter().enumerate() {
        let cfg = on_player.then_some(&player);
        let v = match *how {
            "FLAG" => f64::from(u8::from(p.c.flag(db, cfg, &[name])?)),
            "MORE" => p.c.more(db, cfg, &[name])?,
            "SUMMORE" => p.c.sum(db, ModType::More, cfg, &[name])?,
            "INC" => p.c.sum(db, ModType::Inc, cfg, &[name])?,
            _ => p.c.sum(db, ModType::Base, cfg, &[name])?,
        };
        out.insert(ITEM_QUERY_KEYS[k], v);
    }
    Ok(out)
}

/// The keys of [`ITEM_QUERIES`], as [`item_query_key`] makes them
static ITEM_QUERY_KEYS: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
    ITEM_QUERIES
        .iter()
        .map(|(h, p, n)| &*Box::leak(item_query_key(h, *p, n).into_boxed_str()))
        .collect()
});

/// The gems' `displayEffect`: an active effect as building its skill's
/// mod list left it
fn gem_display(
    p: &Perform,
) -> std::collections::HashMap<(usize, usize), crate::calc_skills::GemDisplay> {
    let mut out = p.env.skills.gem_display.clone();
    let mut last: std::collections::HashMap<(usize, usize), &crate::calc_skills::SkillEffect> =
        Default::default();
    for list in &p.env.skills.group_skills {
        for &i in list {
            let e = &p.skill(i).active_effect;
            if let Some(src) = e.src {
                last.insert(src, e);
            }
        }
    }
    for (src, e) in last {
        if let Some(d) = out.get_mut(&src).filter(|d| d.active) {
            d.level = e.level;
            d.quality = e.quality;
        }
    }
    out
}
