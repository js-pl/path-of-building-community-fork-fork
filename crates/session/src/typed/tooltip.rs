//! Tooltips as upstream's `Tooltip` class collects them (`AddLine`,
//! `AddSeparator`), the stat comparisons of `buildMode:CompareStatList`,
//! and the Skills tab's socket group tooltip
//! (`SkillsTab:AddSocketGroupTooltip`): `socketGroupTooltip` of
//! `lua/session.lua`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::display_stats::{Ctx, Stat};
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::actor::OutVal;
use engine::calcs::Output;
use engine::mods::Sym;
use engine::sf;
use engine::sidebar::SkillInfo;
use serde_json::{Value as J, json};

/// `colorCodes` of `Data/Global.lua`
pub const COLORS: &[(&str, &str)] = &[
    ("NORMAL", "^xC8C8C8"),
    ("MAGIC", "^x8888FF"),
    ("RARE", "^xFFFF77"),
    ("UNIQUE", "^xAF6025"),
    ("RELIC", "^x60C060"),
    ("GEM", "^x1AA29B"),
    ("PROPHECY", "^xB54BFF"),
    ("CURRENCY", "^xAA9E82"),
    ("CRAFTED", "^xB8DAF1"),
    ("CUSTOM", "^x5CF0BB"),
    ("SOURCE", "^x88FFFF"),
    ("UNSUPPORTED", "^xF05050"),
    ("DISABLED", "^x7F7F7F"),
    ("WARNING", "^xFF9922"),
    ("TIP", "^x80A080"),
    ("FIRE", "^xB97123"),
    ("COLD", "^x3F6DB3"),
    ("LIGHTNING", "^xADAA47"),
    ("CHAOS", "^xD02090"),
    ("POSITIVE", "^x33FF77"),
    ("NEGATIVE", "^xDD0022"),
    ("HIGHLIGHT", "^xFF0000"),
    ("OFFENCE", "^xE07030"),
    ("DEFENCE", "^x8080E0"),
    ("SCION", "^xFFF0F0"),
    ("MARAUDER", "^xE05030"),
    ("RANGER", "^x70FF70"),
    ("WITCH", "^x7070FF"),
    ("DUELIST", "^xE0E070"),
    ("TEMPLAR", "^xC040FF"),
    ("SHADOW", "^x30C0D0"),
    ("SHAPER", "^x55BBFF"),
    ("ELDER", "^xAA77CC"),
    ("FRACTURED", "^xA29160"),
    ("MUTATED", "^xCD2285"),
    ("ADJUDICATOR", "^xE9F831"),
    ("BASILISK", "^x00CB3A"),
    ("CRUSADER", "^x2946FC"),
    ("EYRIE", "^xAAB7B8"),
    ("CLEANSING", "^xF24141"),
    ("TANGLE", "^x038C8C"),
    ("SCOURGE", "^xFF6E25"),
    ("CRUCIBLE", "^xFFA500"),
    ("SPLITPERSONALITY", "^xFFD62A"),
    ("VESTIGIAL", "^xCBA5F1"),
    ("INTANGIBILITY", "^x9BF4BD"),
    ("MEMORY", "^xBFE2FA"),
    ("STRENGTH", "^xE05030"),
    ("DEXTERITY", "^x70FF70"),
    ("INTELLIGENCE", "^x7070FF"),
    ("LIFE", "^xE05030"),
    ("MANA", "^x7070FF"),
    ("ES", "^x88FFFF"),
    ("WARD", "^xFFFF77"),
    ("ARMOUR", "^xC8C8C8"),
    ("EVASION", "^x33FF77"),
    ("RAGE", "^xFF9922"),
    ("PHYS", "^xC8C8C8"),
];

/// `colorCodes[name]`
pub fn color(name: &str) -> Option<&'static str> {
    COLORS.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

/// `colorCodes[name]` of a name upstream always has
pub fn cc(name: &str) -> &'static str {
    color(name).unwrap_or_else(|| panic!("no colour code {name}"))
}

/// `data.skillColorMap`
pub fn skill_color(c: Option<f64>) -> Option<&'static str> {
    match c? as i64 {
        1 => Some(cc("STRENGTH")),
        2 => Some(cc("DEXTERITY")),
        3 => Some(cc("INTELLIGENCE")),
        4 => Some(cc("NORMAL")),
        _ => None,
    }
}

/// A number as Lua's `..` writes it
pub fn n(v: f64) -> String {
    lua::value::format_number(v)
}

struct Line {
    size: f64,
    /// `None` for a separator
    text: Option<String>,
    /// A separator with an image, which the next separator does not repeat
    image: bool,
}

/// Upstream's `Tooltip`, as far as the engine API reads it: its lines
#[derive(Default)]
pub struct Tooltip {
    lines: Vec<Line>,
    /// `tooltipHeader`: separators have an image with one
    pub header: bool,
}

impl Tooltip {
    /// `AddLine(size, text)`: one line per line of the text
    pub fn add_line(&mut self, size: f64, text: &str) {
        for l in text.split('\n') {
            self.lines.push(Line {
                size,
                text: Some(l.to_string()),
                image: false,
            });
        }
    }

    /// `AddSeparator(size)`
    pub fn add_separator(&mut self, size: f64) {
        if self.lines.last().is_some_and(|l| l.image) {
            return;
        }
        self.lines.push(Line {
            size,
            text: None,
            image: self.header,
        });
    }

    /// The texts of the lines so far
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().filter_map(|l| l.text.as_deref())
    }

    /// `tooltipLines(tooltip)` of the session
    pub fn json(&self) -> J {
        let lines: Vec<J> = self
            .lines
            .iter()
            .map(|l| match &l.text {
                Some(t) => json!({ "size": num(l.size), "text": t }),
                None => json!({ "separator": true }),
            })
            .collect();
        json!({ "lines": lines })
    }
}

/// `matchFlags(statData.flag, statData.notFlag, skillFlags)`
fn match_flags(s: &Stat, skill: &SkillInfo) -> bool {
    s.flag.is_none_or(|f| skill.flag(f)) && s.not_flag.is_none_or(|f| !skill.flag(f))
}

/// A calculator's output as the comparisons read it (`output`)
pub struct Outputs<'a> {
    pub player: &'a Output,
    pub minion: Option<&'a Output>,
}

impl TypedSession<'_> {
    /// `buildMode:CompareStatList`: the stats that differ, under the header
    #[allow(clippy::too_many_arguments)]
    fn compare_stat_list(
        &self,
        tip: &mut Tooltip,
        stats: &[Stat],
        skill: &SkillInfo,
        base: &Output,
        compare: &Output,
        header: &str,
        node_count: Option<f64>,
    ) -> Result<usize> {
        let cap = (self.data.game.setup.misc)
            .get("DotDpsCap")
            .copied()
            .unwrap_or(f64::NAN);
        let (cb, cc_) = (Ctx { o: base, cap }, Ctx { o: compare, cap });
        let mut count = 0;
        for s in stats {
            let Some(stat) = s.stat else { continue };
            if !match_flags(s, skill) || s.child.is_some() || stat == "SkillDPS" {
                continue;
            }
            let val = |o: &Output| -> Result<f64> {
                match o.get(&Sym::new(stat)) {
                    None => Ok(0.0),
                    Some(OutVal::Num(n)) => Ok(*n),
                    Some(v) if !v.truthy() => Ok(0.0),
                    Some(_) => Err(format!("{NOT_PORTED}: comparing the table {stat}")),
                }
            };
            let (v1, v2) = (val(compare)?, val(base)?);
            let mut diff = v1 - v2;
            if stat == "FullDPS" && compare.get(&Sym::new(stat)).is_none() {
                diff = 0.0;
            }
            let shown = s.cond.is_none_or(|f| f(v1, &cc_) || f(v2, &cb));
            if !(-0.001..=0.001).contains(&diff) && shown {
                if count == 0 {
                    tip.add_line(14.0, header);
                }
                let better = if s.lower_is_better {
                    diff < 0.0
                } else {
                    diff > 0.0
                };
                let color = cc(if better { "POSITIVE" } else { "NEGATIVE" });
                let mult = if s.pc || s.modifier { 100.0 } else { 1.0 };
                let mut val_str = sf!(&format!("%+{}", s.fmt), diff * mult);
                if let Some(c) = engine::pat::captures(&val_str, "^([%+%-]?%d+%.%d+)(%D*)$") {
                    let n = engine::pat::gsub(&engine::pat::gsub(&c[0], "0+$", ""), "%.$", "");
                    val_str = format!("{n}{}", c[1]);
                }
                let val_str = engine::pat::format_num_sep(&val_str);
                let mut line = format!("{color}{val_str} {}", s.label);
                let mut pc_per_pt = String::new();
                if s.comp_percent && v1 != 0.0 && v2 != 0.0 {
                    let pc = v1 / v2 * 100.0 - 100.0;
                    line.push_str(&sf!(" (%+.1f%%)", pc));
                    if let Some(nc) = node_count {
                        pc_per_pt = sf!(" (%+.1f%%)", pc / nc);
                    }
                }
                if let Some(nc) = node_count {
                    let f = format!(" ^8[%+{}%s per point]", s.fmt);
                    line.push_str(&sf!(&f, diff * mult / nc, pc_per_pt.as_str()));
                }
                tip.add_line(14.0, &line);
                count += 1;
            }
        }
        Ok(count)
    }

    /// `buildMode:AddStatComparesToTooltip`: the minion's stats, then the
    /// player's; the number of lines added
    pub(super) fn add_stat_compares(
        &self,
        tip: &mut Tooltip,
        base: &Outputs,
        compare: &Outputs,
        header: &str,
        node_count: Option<f64>,
    ) -> Result<usize> {
        let sb = self.sidebar()?;
        let (stats, minion_stats) = super::sidebar::display_stats();
        let mut count = 0;
        let mut header = header.to_string();
        if let (Some(ms), Some(b), Some(c)) = (&sb.minion_skill, base.minion, compare.minion) {
            let h = format!("{header}\n^7Minion:");
            count += self.compare_stat_list(tip, minion_stats, ms, b, c, &h, node_count)?;
            header = if count > 0 {
                "^7Player:".into()
            } else {
                format!("{header}\n^7Player:")
            };
        }
        count += self.compare_stat_list(
            tip,
            stats,
            &sb.main_skill,
            base.player,
            compare.player,
            &header,
            node_count,
        )?;
        Ok(count)
    }

    /// `socketGroupTooltip`: `SkillsTab:AddSocketGroupTooltip`
    pub fn socket_group_tooltip(&self, index: f64) -> Result<J> {
        let groups = self.groups();
        let gi = (index as usize)
            .checked_sub(1)
            .filter(|&i| i < groups.len() && index.fract() == 0.0)
            .ok_or_else(|| format!("no socket group {}", n(index)))?;
        let group = &groups[gi];
        let sb = self.sidebar()?;
        let mut tip = Tooltip::default();
        let items = &self.build.items;
        let node_name = |id: u32| -> Result<String> {
            let spec = &self.build.spec;
            let i = spec.index.get(&id).ok_or("no source node")?;
            Ok(spec.tree_node(*i).dn.to_string())
        };
        let item_source = |id: f64| -> Result<(String, String)> {
            let item = items.item(id).ok_or("no source item")?;
            Ok((item.rarity.clone(), item.name.clone()))
        };
        if !group.explode_sources.is_empty() {
            for s in &group.explode_sources {
                let (rarity, name) = match s {
                    engine::skills::ExplodeSource::Item(id) => item_source(*id)?,
                    engine::skills::ExplodeSource::Node(id) => ("NORMAL".into(), node_name(*id)?),
                };
                let c = color(&rarity).unwrap_or(cc("NORMAL"));
                tip.add_line(18.0, &format!("^7Source: {c}{name}"));
            }
            return Ok(tip.json());
        }
        let slot_enabled = sb.slot_enabled.get(gi).copied();
        if group.enabled && slot_enabled == Some(false) {
            tip.add_line(
                16.0,
                "^7Note: this group is disabled because it is socketed in the inactive weapon set.",
            );
        }
        let source = match (group.source_item, group.source_node) {
            (Some(id), _) => Some(item_source(id)?),
            (None, Some(id)) => Some(("NORMAL".into(), node_name(id)?)),
            _ => None,
        };
        if let Some((rarity, name)) = source {
            let c = color(&rarity).unwrap_or(cc("NORMAL"));
            tip.add_line(18.0, &format!("^7Source: {c}{name}"));
            tip.add_separator(10.0);
        }
        let gem_at = |(g, k): (usize, usize)| groups.get(g).and_then(|g| g.gem_list.get(k));
        let unported = || format!("{NOT_PORTED}: a socket group tooltip of a gem without a level");
        let magic = cc("MAGIC");
        // "%s%s ^7%d%s/%d%s" of an effect
        let effect_line = |e: &engine::sidebar::EffectInfo| -> Result<String> {
            let col = skill_color(e.color).ok_or_else(unported)?;
            let level = e.level.ok_or_else(unported)?;
            let src = e.src.and_then(gem_at);
            let (sl, sq) = match src {
                Some(g) => (
                    g.level.ok_or_else(unported)?,
                    g.quality.ok_or_else(unported)?,
                ),
                None => (level, e.quality),
            };
            let plus = |v: f64, s: f64| {
                if src.is_some() && v > s {
                    format!("{magic}+{}^7", n(v - s))
                } else {
                    String::new()
                }
            };
            Ok(sf!(
                "%s%s ^7%d%s/%d%s",
                col,
                e.name.as_str(),
                sl,
                plus(level, sl).as_str(),
                sq,
                plus(e.quality, sq).as_str()
            ))
        };
        let display = sb.groups.get(gi).map_or(&[][..], |l| &l[..]);
        let mut shown = std::collections::HashSet::new();
        for (index, skill) in display.iter().enumerate() {
            if index > 0 {
                tip.add_separator(10.0);
            }
            tip.add_line(16.0, &format!("^7Active Skill #{}:", index + 1));
            for e in &skill.effects {
                tip.add_line(20.0, &effect_line(e)?);
                shown.extend(e.src);
            }
            if let Some(e) = &skill.minion_effect {
                tip.add_separator(10.0);
                tip.add_line(
                    16.0,
                    &format!("^7Active Skill #{}'s Main Minion Skill:", index + 1),
                );
                tip.add_line(20.0, &effect_line(e)?);
                shown.extend(e.src);
            }
        }
        let list: Vec<(usize, usize)> = match sb.display_gem_list.get(gi).cloned().flatten() {
            Some(l) => l,
            None => (0..group.gem_list.len()).map(|k| (gi, k)).collect(),
        };
        let mut header = true;
        for at in list {
            if shown.contains(&at) {
                continue;
            }
            let Some(gem) = gem_at(at) else { continue };
            if header {
                header = false;
                tip.add_separator(10.0);
                tip.add_line(16.0, "^7Inactive Gems:");
            }
            let disp = sb.gem_display.get(&at);
            let effect = gem
                .gem
                .as_ref()
                .map(|g| &g.granted_effect)
                .or(gem.granted_effect.as_ref());
            let reason = match effect {
                None => "(Unsupported)",
                Some(_) if !gem.enabled => "(Disabled)",
                Some(_) if !group.enabled || slot_enabled != Some(true) => "",
                Some(e) if e.support => {
                    if disp.is_some_and(|d| d.superseded) {
                        "(Superseded)"
                    } else if !disp.is_some_and(|d| d.supporting) && !display.is_empty() {
                        "(Cannot apply to any of the active skills)"
                    } else {
                        ""
                    }
                }
                Some(_) => "",
            };
            let name = gem
                .granted_effect
                .as_ref()
                .map(|e| e.name.clone())
                .or(gem.gem.as_ref().map(|g| g.name.clone()))
                .unwrap_or_else(|| gem.name_spec.clone());
            let level = gem.level.ok_or_else(unported)?;
            let quality = gem.quality.ok_or_else(unported)?;
            let (dl, dq) = match disp {
                Some(d) => (d.level.ok_or_else(unported)?, d.quality),
                None => (level, quality),
            };
            let plus = |v: f64, s: f64| {
                if v > s {
                    format!("{magic}+{}^7", n(v - s))
                } else {
                    String::new()
                }
            };
            tip.add_line(
                20.0,
                &sf!(
                    "%s%s ^7%d%s/%d%s %s",
                    gem.color.as_str(),
                    name.as_str(),
                    level,
                    plus(dl, level).as_str(),
                    quality,
                    plus(dq, quality).as_str(),
                    reason
                ),
            );
        }
        Ok(tip.json())
    }
}
