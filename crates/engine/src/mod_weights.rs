//! Mod weights: what each mod that can roll on an item slot is worth to a
//! build, `GenerateModWeights` and `GeneratePassiveNodeWeights` of
//! `Classes/TradeQueryGenerator.lua` with its `WeightedRatioOutputs`, and
//! the trade categories of `Classes/TradeHelpers.lua`'s
//! `getTradeCategory`. For each mod of `Data/QueryMods.lua` ([`QueryMods`])
//! that rolls in the slot's item category, a blank rare of the slot's base
//! with the mod at a mid roll goes into the slot through the misc
//! calculator; the gain in the weighted stats over the blank item, per
//! point of the mod's value, is its weight.
//!
//! Upstream goes over each group of mods in `pairs` order and weighs a
//! trade mod once, the first of the entries sharing its id; here the
//! entries go by key, one of the orders `pairs` can give. The calculations
//! run on threads natively; results do not depend on the thread count.
//!
//! Not ported: Pearl of Tsoatha's weights (its support gem stats need
//! `data.describeStats`), the query itself (`FinishQuery`) and trade.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::calc::m_min;
use crate::calc_override::{Override, OverrideNode, tree_node};
use crate::calc_perform::Result;
use crate::calculator::{CalcOutput, MiscCalculator};
use crate::item::{Item, ItemContext, ModLine};
use crate::parallel::{self, Threads};
use crate::power::PowerStat;
use std::collections::HashSet;
use std::sync::Arc;

/// `Data/QueryMods.lua`: the mods to weigh, by group (`Explicit`,
/// `Implicit`, `Corrupted`, `Enchant`, `Scourge`, `Eater`, `Exarch`,
/// `PassiveNode`, `WatchersEye`), each group's sorted by key
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryMods {
    pub groups: Vec<(String, Vec<QueryMod>)>,
}

/// An entry of `Data/QueryMods.lua`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryMod {
    /// The entry's key (`1922_SkillEffectDuration`)
    pub key: String,
    /// `tradeMod.id`
    pub trade_id: String,
    /// `tradeMod.text`, with `#` for the value
    pub text: String,
    /// `sign`: put before the value (`""`, `"+"`, `"-"`)
    pub sign: Option<String>,
    /// `specialCaseData.overrideModLine`
    pub override_line: Option<String>,
    /// `specialCaseData.overrideModLineSingular`, for a value of 1
    pub override_line_singular: Option<String>,
    /// The mod's rolls by item category (`Helmet`, `1HWeapon`, ...)
    pub ranges: Vec<(String, ModRange)>,
}

/// The rolls of a mod in an item category
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModRange {
    pub min: f64,
    pub max: f64,
    pub sub_type: Option<String>,
}

crate::codec_struct!(QueryMods { groups });
crate::codec_struct!(QueryMod {
    key,
    trade_id,
    text,
    sign,
    override_line,
    override_line_singular,
    ranges,
});
crate::codec_struct!(ModRange { min, max, sub_type });

impl QueryMods {
    /// The mods of a group, none for a group the file does not have
    pub fn group(&self, name: &str) -> &[QueryMod] {
        self.groups
            .iter()
            .find(|(g, _)| g == name)
            .map_or(&[], |(_, m)| m.as_slice())
    }
}

impl QueryMod {
    /// `entry[itemCategory]`
    pub fn range(&self, category: &str) -> Option<&ModRange> {
        self.ranges
            .iter()
            .find(|(c, _)| c == category)
            .map(|(_, r)| r)
    }
}

/// A stat of the weighted sum, as the trade popup's sliders give it:
/// `{ stat, weightMult, transform }`
#[derive(Clone, Debug, PartialEq)]
pub struct StatWeight {
    pub stat: PowerStat,
    pub weight: f64,
}

impl StatWeight {
    /// A stat of `data.powerStatList` by its `stat`, with its transform
    pub fn new(stat: &str, weight: f64) -> Option<StatWeight> {
        let stat = crate::power::power_stats()
            .into_iter()
            .find(|p| p.stat.as_deref() == Some(stat))?;
        Some(StatWeight { stat, weight })
    }
}

/// The trade popup's default weights (`initStatSortSelectionList`): Full
/// DPS 1, Effective Hit Pool 0.5
pub fn default_stat_weights() -> Vec<StatWeight> {
    vec![
        StatWeight::new("FullDPS", 1.0).expect("FullDPS"),
        StatWeight::new("TotalEHP", 0.5).expect("TotalEHP"),
    ]
}

/// `data.misc.maxStatIncrease`
const MAX_STAT_INCREASE: f64 = 2.0;

/// `data.powerStatList.GetFromOutput(output, { stat = stat }, true)`
fn stat_value(out: &CalcOutput, stat: &str) -> f64 {
    PowerStat {
        stat: Some(stat.into()),
        label: String::new(),
        negate: false,
        combined_off_def: false,
        ignore_for_nodes: false,
        ignore_for_items: false,
    }
    .value(out)
}

/// `TradeQueryGeneratorClass.WeightedRatioOutputs(baseOutput, newOutput,
/// statWeights)`: the sum of each stat's ratio of new to base (at most
/// `maxStatIncrease`, negated for stats where lower is better), by weight
pub fn weighted_ratio(base: &CalcOutput, new: &CalcOutput, weights: &[StatWeight]) -> f64 {
    let ratio = |stats: &[&str]| {
        let base_sum: f64 = stats.iter().map(|s| stat_value(base, s)).sum();
        let new_sum: f64 = stats.iter().map(|s| stat_value(new, s)).sum();
        if base_sum == f64::INFINITY {
            0.0
        } else if new_sum == f64::INFINITY {
            MAX_STAT_INCREASE
        } else {
            let div = if base_sum != 0.0 { base_sum } else { 1.0 };
            m_min(new_sum / div, MAX_STAT_INCREASE)
        }
    };
    let mut mean = 0.0;
    for w in weights {
        let Some(stat) = w.stat.stat.as_deref() else {
            // `GetFromOutput` of a stat-less entry indexes `output[nil]`
            continue;
        };
        let full = |o: &CalcOutput| o.player.contains_key(&crate::mods::Sym::new("FullDPS"));
        let mut r = if stat == "FullDPS" && !(full(base) && full(new)) {
            ratio(&["TotalDPS", "TotalDotDPS", "CombinedDPS"])
        } else {
            ratio(&[stat])
        };
        if w.stat.negate {
            r = -r;
        }
        mean += r * w.weight;
    }
    mean
}

/// The item category of `getTradeCategory(slotName, item)`: the slot's,
/// or for weapon slots the item's type's
pub fn trade_category(slot: &str, item: Option<&Item>) -> Option<&'static str> {
    let typ = item.and_then(|i| {
        i.typ
            .as_deref()
            .or_else(|| i.base.as_ref().map(|b| b.typ.as_str()))
    });
    let weapon = slot
        .strip_prefix("Weapon ")
        .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()));
    if weapon {
        let Some(t) = typ else {
            return Some("1HWeapon");
        };
        return Some(match t {
            "Shield" => "Shield",
            "Quiver" => "Quiver",
            "Bow" => "Bow",
            "Staff" => "Staff",
            "Two Handed Sword" => "2HSword",
            "Two Handed Axe" => "2HAxe",
            "Two Handed Mace" => "2HMace",
            "Fishing Rod" => "FishingRod",
            "One Handed Sword" => "1HSword",
            "One Handed Axe" => "1HAxe",
            "One Handed Mace" => "1HMace",
            "Sceptre" => "Sceptre",
            "Wand" => "Wand",
            "Dagger" => "Dagger",
            "Claw" => "Claw",
            t if t.contains("Two Handed") => "2HWeapon",
            _ => "1HWeapon",
        });
    }
    Some(match slot {
        "Body Armour" => "Chest",
        "Helmet" => "Helmet",
        "Gloves" => "Gloves",
        "Boots" => "Boots",
        "Amulet" => "Amulet",
        "Ring 1" | "Ring 2" | "Ring 3" => "Ring",
        "Belt" => "Belt",
        s if s.contains("Abyssal") => "AbyssJewel",
        s if s.contains("Jewel") => "Jewel",
        s if s.contains("Flask") => "Flask",
        _ => return None,
    })
}

/// The jewel types of the trade popup's jewel slots (`jewelType`)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JewelType {
    #[default]
    Base,
    Abyss,
}

/// The eldritch implicits to weigh (`includeEldritch`)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Eldritch {
    /// `Copy Current`, `Remove`: none
    #[default]
    None,
    /// `Keep regular`: without the presence mods
    Regular,
    /// `Keep regular+presence`
    WithPresence,
}

/// The unique items the trade popup weighs in their own way
/// (`options.special`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Special {
    /// Megalomaniac: the cluster notables it can allocate
    Megalomaniac,
    /// Watcher's Eye: its aura mods, any jewel
    WatchersEye,
    /// Pearl of Tsoatha: its support gems (not ported)
    PearlOfTsoatha,
}

/// What to weigh (`StartQuery`'s options)
#[derive(Clone, Debug)]
pub struct WeightOptions {
    pub stat_weights: Vec<StatWeight>,
    pub include_corrupted: bool,
    pub include_talisman: bool,
    pub include_scourge: bool,
    pub include_eldritch: Eldritch,
    /// For jewel sockets (not abyssal ones)
    pub jewel_type: JewelType,
    pub special: Option<Special>,
}

impl Default for WeightOptions {
    fn default() -> Self {
        WeightOptions {
            stat_weights: default_stat_weights(),
            include_corrupted: false,
            include_talisman: false,
            include_scourge: false,
            include_eldritch: Eldritch::None,
            jewel_type: JewelType::Base,
            special: None,
        }
    }
}

/// A mod's weight (an entry of `self.modWeights`)
#[derive(Clone, Debug, PartialEq)]
pub struct ModWeight {
    /// `tradeModId`
    pub trade_id: String,
    /// The `Data/QueryMods.lua` entry weighed
    pub key: String,
    /// The mod line tested
    pub line: String,
    /// The value tested (1 for a passive node)
    pub value: f64,
    /// The weighted gain per point of the value
    pub weight: f64,
    /// The weighted gain (`meanStatDiff`), times 1000
    pub mean_stat_diff: f64,
    /// `invert`: the mod's sign is `-`
    pub invert: bool,
}

/// A slot's mod weights
#[derive(Clone, Debug, PartialEq)]
pub struct ModWeights {
    /// The item category weighed for (`itemCategory`)
    pub category: Option<String>,
    /// The base the mods were tested on
    pub test_base: String,
    /// The blank test item's weighted ratio, times 1000 (`baseStatValue`)
    pub base_stat_value: f64,
    /// The mods worth more than 0.01, in the order weighed
    pub weights: Vec<ModWeight>,
    /// Mod lines the parser made no mods of (upstream logs them, "Failed
    /// to test"); they are weighed anyway
    pub unparsed: Vec<String>,
}

/// A mod to test: its entry and roll
struct Candidate<'m> {
    entry: &'m QueryMod,
    range: &'m ModRange,
}

/// The groups `ExecuteQuery` weighs, in its order
fn groups<'m>(mods: &'m QueryMods, opts: &WeightOptions) -> Vec<Vec<&'m QueryMod>> {
    let all = |g: &str| mods.group(g).iter().collect::<Vec<_>>();
    if opts.special == Some(Special::WatchersEye) {
        let mut out = vec![all("WatchersEye")];
        if opts.include_corrupted {
            out.push(all("Corrupted"));
        }
        return out;
    }
    let mut out = vec![all("Explicit"), all("Implicit")];
    if opts.include_corrupted {
        out.push(all("Corrupted"));
    }
    if opts.include_talisman {
        out.push(all("Enchant"));
    }
    if opts.include_scourge {
        out.push(all("Scourge"));
    }
    let presence =
        |m: &&QueryMod| !(m.key.ends_with("PinnaclePresence") || m.key.ends_with("UniquePresence"));
    match opts.include_eldritch {
        Eldritch::None => {}
        Eldritch::Regular => {
            for g in ["Eater", "Exarch"] {
                out.push(mods.group(g).iter().filter(presence).collect());
            }
        }
        Eldritch::WithPresence => {
            out.push(all("Eater"));
            out.push(all("Exarch"));
        }
    }
    out
}

/// `math.ceil((max - min) * quality + min)`
fn mid_roll(r: &ModRange, quality: f64) -> f64 {
    ((r.max - r.min) * quality + r.min).ceil()
}

/// The mod line of an entry at a value (`modLine:gsub("#", modValueStr)`)
fn mod_line(entry: &QueryMod, value: f64) -> String {
    let line = match (&entry.override_line_singular, &entry.override_line) {
        (Some(s), _) if value == 1.0 => s,
        (_, Some(o)) => o,
        _ => &entry.text,
    };
    let value = format!(
        "{}{}",
        entry.sign.as_deref().unwrap_or(""),
        lua::format::format_g(value, 14)
    );
    line.replace('#', &value)
}

/// `explicitModLines[1] = { line = modLine, custom = true }` and
/// `BuildAndParseRaw()` on the test item. Upstream tests every mod on the
/// one item, which keeps what the earlier lines left on it (the sockets
/// and the other explicit lines a line parsed into), so each test item
/// is made from the one before.
fn set_line(ctx: &ItemContext, item: &mut Item, line: &str) -> Result<()> {
    let mut l = ModLine {
        line: line.to_string(),
        ..ModLine::default()
    };
    l.flags.custom = true;
    match item.explicit_mod_lines.first_mut() {
        Some(first) => *first = l,
        None => item.explicit_mod_lines.push(l),
    }
    let raw = item.build_raw();
    item.parse_raw(ctx, &raw, None, false)
}

/// `self.modWeights` of `StartQuery(slot, options)` and `ExecuteQuery`:
/// the weights of the mods that roll in `slot`'s item category (or of a
/// special item's), tested on a blank rare of the slot's item's base
/// through the build's calculator. `slot` is a slot of the items tab;
/// only Megalomaniac's weights need none.
pub fn mod_weights(
    calc: &MiscCalculator,
    ctx: &ItemContext,
    mods: &QueryMods,
    slot: Option<&str>,
    opts: &WeightOptions,
    threads: Threads,
) -> Result<ModWeights> {
    let build = calc.build();
    let tab = &build.items;
    let existing = slot.and_then(|s| {
        let sel = tab.ordered_slots.iter().find(|x| x.slot_name == s)?;
        tab.item(sel.sel_item_id)
    });
    let mut base_name = existing
        .and_then(|i| i.base_name.clone())
        .unwrap_or_else(|| "Unset Amulet".into());
    let category: Option<String> = match opts.special {
        Some(Special::Megalomaniac) => None,
        Some(Special::WatchersEye) => Some("AnyJewel".into()),
        Some(Special::PearlOfTsoatha) => {
            return Err("not ported: Pearl of Tsoatha's mod weights".into());
        }
        None => {
            let slot = slot.ok_or("attempt to index a nil value (local 'slot')")?;
            let mut cat = trade_category(slot, existing).map(str::to_string);
            if slot.contains("Jewel") && !slot.contains("Abyssal") {
                cat = Some(match opts.jewel_type {
                    JewelType::Base => "BaseJewel".into(),
                    JewelType::Abyss => "AbyssJewel".into(),
                });
            }
            Some(cat.ok_or_else(|| {
                format!("'{slot}' is not supported for weighted trade query generation")
            })?)
        }
    };
    if opts.special == Some(Special::PearlOfTsoatha) {
        base_name = "Prismatic Ring".into();
    }
    let raw = format!("Rarity: RARE\nStat Tester\n{base_name}");
    let test_item = Item::new(ctx, &raw, None, false)?;
    let base = calc.base();
    let rep = |item: Item| Override {
        rep_item: slot.map(|s| (s.to_string(), Some(item))),
        ..Override::default()
    };
    let base_item_out = match slot {
        Some(_) => calc.calculate(&rep(test_item.clone()), true)?,
        None => base.clone(),
    };
    let weights = &opts.stat_weights;
    let base_stat_value = weighted_ratio(base, &base_item_out, weights) * 1000.0;
    let gain = |out: &CalcOutput| weighted_ratio(base, out, weights) * 1000.0 - base_stat_value;

    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = ModWeights {
        category: category.clone(),
        test_base: base_name,
        base_stat_value,
        weights: Vec::new(),
        unparsed: Vec::new(),
    };

    if opts.special == Some(Special::Megalomaniac) {
        // `GeneratePassiveNodeWeights(self.modData.PassiveNode)`
        let tree = build.spec.tree;
        let mut nodes = Vec::new();
        for entry in mods.group("PassiveNode") {
            if seen.contains(entry.trade_id.as_str()) {
                continue;
            }
            let name = entry
                .text
                .strip_prefix("1 Added Passive Skill is ")
                .or_else(|| entry.text.strip_prefix("Allocates "));
            let Some(name) = name else { continue };
            let id = tree
                .cluster_node_map
                .get(name)
                .or_else(|| tree.notable_map.get(name))
                .copied()
                .ok_or("table index is nil")?;
            nodes.push((entry, id));
            seen.insert(&entry.trade_id);
        }
        let results = parallel::map(nodes, threads, |(entry, id)| {
            let node = tree_node(&build.spec, id).ok_or("table index is nil")?;
            let ovr = Override {
                add_nodes: vec![OverrideNode::Own(Arc::new(node))],
                ..Override::default()
            };
            let diff = gain(&calc.calculate(&ovr, true)?);
            Ok::<_, String>((entry, diff))
        });
        for r in results {
            let (entry, diff) = r?;
            if diff > 0.01 {
                out.weights.push(ModWeight {
                    trade_id: entry.trade_id.clone(),
                    key: entry.key.clone(),
                    line: entry.text.clone(),
                    value: 1.0,
                    weight: diff,
                    mean_stat_diff: diff,
                    invert: false,
                });
            }
        }
        return Ok(out);
    }

    // `GenerateModWeights` of each group
    let category = category.unwrap_or_default();
    let mut candidates = Vec::new();
    for group in groups(mods, opts) {
        for entry in group {
            let Some(range) = entry.range(&category) else {
                continue;
            };
            if seen.contains(entry.trade_id.as_str())
                || !opts.include_talisman && range.sub_type.as_deref() == Some("Talisman")
            {
                continue;
            }
            candidates.push(Candidate { entry, range });
            seen.insert(&entry.trade_id);
        }
    }
    let slot = slot.ok_or("attempt to index a nil value (field 'slot')")?;
    let quality = ctx.default_affix_quality;
    // The test items one after the other, then their calculations
    let mut item = test_item;
    let mut tests = Vec::with_capacity(candidates.len());
    for c in candidates {
        let value = mid_roll(c.range, quality);
        let line = mod_line(c.entry, value);
        set_line(ctx, &mut item, &line)?;
        tests.push((c.entry, line, value, item.clone()));
    }
    let results = parallel::map(tests, threads, |(entry, line, value, item)| {
        let unparsed = item.mod_list.as_ref().is_some_and(Vec::is_empty)
            || item
                .slot_mod_list
                .as_ref()
                .is_some_and(|s| s.iter().take(2).all(Vec::is_empty));
        let ovr = Override {
            rep_item: Some((slot.to_string(), Some(item))),
            ..Override::default()
        };
        let diff = gain(&calc.calculate(&ovr, true)?);
        Ok::<_, String>((entry, line, value, diff, unparsed))
    });
    for r in results {
        let (entry, line, value, diff, unparsed) = r?;
        if unparsed {
            out.unparsed.push(line.clone());
        }
        if diff > 0.01 {
            out.weights.push(ModWeight {
                trade_id: entry.trade_id.clone(),
                key: entry.key.clone(),
                line,
                value,
                weight: diff / value,
                mean_stat_diff: diff,
                invert: entry.sign.as_deref() == Some("-"),
            });
        }
    }
    Ok(out)
}
