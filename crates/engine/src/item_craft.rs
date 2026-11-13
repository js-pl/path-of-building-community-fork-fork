//! Crafting an item from its affixes (`Item:Craft` of `Classes/Item.lua`)
//! and the affixes the item editor offers for each slot
//! (`ItemsTab:UpdateAffixControl`, without the sort by power).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::item::{AffixSlot, Item, ItemContext, ModLine, sort_crafted_mod_lines};
use crate::item_data::ItemData;
use crate::item_tools::{Range, apply_range};
use crate::pat::{self, gsub, is_match};
use std::collections::{HashMap, HashSet};

type Result<T> = std::result::Result<T, String>;

/// An entry of an affix drop-down: `"None"`, or a series of tiers of the
/// same stats
#[derive(Clone, Debug, PartialEq)]
pub struct AffixSeries {
    pub label: String,
    /// The tiers' mod ids, lowest first
    pub mod_list: Vec<String>,
    pub have_range: bool,
    pub stat_order: Vec<f64>,
}

/// The affix slots of one list: `"prefixes"` or `"suffixes"`
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AffixTable {
    Prefixes,
    Suffixes,
}

impl AffixTable {
    pub fn name(self) -> &'static str {
        match self {
            AffixTable::Prefixes => "prefixes",
            AffixTable::Suffixes => "suffixes",
        }
    }
}

impl Item {
    pub fn affix_slots(&self, table: AffixTable) -> &Vec<AffixSlot> {
        match table {
            AffixTable::Prefixes => &self.prefixes.list,
            AffixTable::Suffixes => &self.suffixes.list,
        }
    }

    pub fn affix_slots_mut(&mut self, table: AffixTable) -> &mut Vec<AffixSlot> {
        match table {
            AffixTable::Prefixes => &mut self.prefixes.list,
            AffixTable::Suffixes => &mut self.suffixes.list,
        }
    }

    /// `prefixes.limit or affixLimit / 2`, and the same of the suffixes
    pub fn affix_table_limit(&self, table: AffixTable) -> f64 {
        let list = match table {
            AffixTable::Prefixes => &self.prefixes,
            AffixTable::Suffixes => &self.suffixes,
        };
        list.limit.unwrap_or(self.affix_limit / 2.0)
    }

    /// `Craft()`: the explicit lines again from the affixes, then
    /// `BuildAndParseRaw`
    pub fn craft(&mut self, ctx: &ItemContext) -> Result<()> {
        let affixes = self
            .affixes
            .clone()
            .ok_or("attempt to index field 'affixes' (a nil value)")?;
        let rare_like = self.rare_like_unique.is_some();
        let saved: Vec<ModLine> = self
            .explicit_mod_lines
            .iter()
            .filter(|m| {
                m.flags.crafted
                    || m.flags.custom
                    || (rare_like && !(m.flags.prefix || m.flags.suffix))
            })
            .cloned()
            .collect();
        self.explicit_mod_lines.clear();
        self.name_prefix = String::new();
        self.name_suffix = String::new();
        let base = self.base.clone().ok_or("no base")?;
        self.requirements.level = base.req.level;
        // Stat order to the index of its line
        let mut by_order: HashMap<u64, usize> = HashMap::new();
        for table in [AffixTable::Prefixes, AffixTable::Suffixes] {
            let limit = self.affix_table_limit(table);
            let mut i = 1.0;
            while i <= limit {
                let idx = i as usize - 1;
                i += 1.0;
                let slots = self.affix_slots_mut(table);
                if idx >= slots.len() {
                    // `list[i] = { modId = "None" }`, then indexing the nil
                    if idx == slots.len() {
                        slots.push(AffixSlot {
                            mod_id: "None".into(),
                            range: None,
                            fractured: false,
                        });
                    }
                    return Err("attempt to index local 'affix' (a nil value)".into());
                }
                let affix = slots[idx].clone();
                let Some(m) = affixes.get(&affix.mod_id) else {
                    continue;
                };
                match m.typ.as_deref() {
                    Some("Prefix") => {
                        self.name_prefix =
                            format!("{} {}", m.affix.as_deref().unwrap_or(""), self.name_prefix)
                    }
                    Some("Suffix") => {
                        self.name_suffix =
                            format!("{} {}", self.name_suffix, m.affix.as_deref().unwrap_or(""))
                    }
                    _ => {}
                }
                let level = m
                    .level
                    .ok_or("attempt to perform arithmetic on field 'level' (a nil value)")?;
                self.requirements.level = Some(
                    self.requirements
                        .level
                        .unwrap_or(0.0)
                        .max((level * 0.8).floor()),
                );
                let range = affix.range.clone().unwrap_or(Range::One(0.5));
                for (li, line) in m.lines.iter().enumerate() {
                    let line = apply_range(ctx.data, ctx.parser, line, &range, None, None)?;
                    let order = m.stat_order.get(li).copied();
                    let key = order.map(f64::to_bits);
                    if let Some(&at) = key.as_ref().and_then(|k| by_order.get(k)) {
                        // Combine stats
                        let combined = combine(&self.explicit_mod_lines[at].line, &line)?;
                        self.explicit_mod_lines[at].line = combined;
                    } else {
                        let mut ml = ModLine {
                            line,
                            order,
                            mod_tags: m.mod_tags.clone(),
                            ..ModLine::default()
                        };
                        ml.flags.fractured = affix.fractured;
                        match m.typ.as_deref() {
                            Some("Prefix") => ml.flags.prefix = true,
                            Some("Suffix") => ml.flags.suffix = true,
                            _ => {}
                        }
                        self.explicit_mod_lines.push(ml);
                        if let Some(k) = key {
                            by_order.insert(k, self.explicit_mod_lines.len() - 1);
                        }
                    }
                }
            }
        }
        self.explicit_mod_lines.extend(saved);
        if self.explicit_mod_lines.len() > 1 {
            sort_crafted_mod_lines(&mut self.explicit_mod_lines);
        }
        let raw = self.build_raw();
        self.parse_raw(ctx, &raw, None, false)
    }

    /// `UpdateAffixControl`: the drop-down's entries (`"None"` first, as
    /// `None`) for one affix slot, and the entry selected
    pub fn affix_options(
        &self,
        data: &ItemData,
        affix_type: Option<&str>,
        table: AffixTable,
        index: usize,
    ) -> Result<(Vec<Option<AffixSeries>>, usize)> {
        let affixes = self.affixes.clone().ok_or("no affixes")?;
        let rare = self
            .rare_like_unique
            .as_ref()
            .and_then(|r| data.rare_like_uniques.get(r));
        let allow_duplicate_groups = rare.is_some_and(|r| r.allow_duplicate_groups);
        let mut extra_tags: HashSet<String> = HashSet::new();
        let mut exclude_groups: HashSet<String> = HashSet::new();
        for t in [AffixTable::Prefixes, AffixTable::Suffixes] {
            let limit = self.affix_table_limit(t);
            let mut i = 1.0;
            while i <= limit {
                let idx = i as usize;
                i += 1.0;
                if idx == index && t == table {
                    continue;
                }
                let Some(m) = self
                    .affix_slots(t)
                    .get(idx - 1)
                    .and_then(|a| affixes.get(&a.mod_id))
                else {
                    continue;
                };
                if let Some(g) = &m.group
                    && !allow_duplicate_groups
                {
                    exclude_groups.insert(g.clone());
                }
                extra_tags.extend(m.tags.iter().cloned());
            }
        }
        let cluster = self
            .base_name
            .as_ref()
            .and_then(|b| data.cluster_jewels.get(b));
        if let (Some(cj), Some(skill)) = (cluster, &self.cluster_jewel_skill)
            && let Some(tag) = cj.skill_data.get(skill).and_then(|s| s.tag.clone())
        {
            extra_tags.insert(tag);
        }
        let sel = self
            .affix_slots(table)
            .get(index - 1)
            .map(|a| a.mod_id.clone())
            .ok_or("attempt to index field '?' (a nil value)")?;
        let mut list: Vec<&str> = Vec::new();
        let mut retained: HashSet<&str> = HashSet::new();
        for m in &affixes.list {
            let id = m.id.as_str();
            if affixes.get(id).map(|a| std::ptr::eq(a, &**m)) != Some(true) {
                continue;
            }
            if affix_type.is_some_and(|t| m.typ.as_deref() != Some(t))
                || m.group.as_ref().is_some_and(|g| exclude_groups.contains(g))
                || matches!(
                    m.affix.as_deref(),
                    Some("Subterranean" | "of the Underground")
                )
            {
                continue;
            }
            if self.can_have_mod_with(data, m, &extra_tags) {
                list.push(id);
            } else if id == sel {
                list.push(id);
                retained.insert(id);
            }
        }
        list.sort_by(|a, b| {
            use std::cmp::Ordering;
            let (ma, mb) = (affixes.get(a).unwrap(), affixes.get(b).unwrap());
            for i in 0..ma.lines.len().max(mb.lines.len()) {
                if i >= ma.lines.len() {
                    return Ordering::Less;
                } else if i >= mb.lines.len() {
                    return Ordering::Greater;
                }
                let (oa, ob) = (ma.stat_order.get(i), mb.stat_order.get(i));
                if oa != ob {
                    return oa.partial_cmp(&ob).unwrap_or(Ordering::Equal);
                }
            }
            if ma.level != mb.level {
                return ma.level.partial_cmp(&mb.level).unwrap_or(Ordering::Equal);
            }
            a.cmp(b)
        });
        let range_pat = "%(%-?[%d%.]+%-%-?[%d%.]+%)";
        let mut out: Vec<Option<AffixSeries>> = vec![None];
        for id in list {
            let m = affixes.get(id).unwrap();
            let same = matches!(out.last(), Some(Some(s)) if s.stat_order == m.stat_order);
            if !same {
                let label = m.lines.join("/");
                out.push(Some(AffixSeries {
                    have_range: is_match(&label, range_pat),
                    label,
                    mod_list: Vec::new(),
                    stat_order: m.stat_order.clone(),
                }));
            }
            let Some(Some(s)) = out.last_mut() else {
                unreachable!()
            };
            if retained.contains(id) && cluster.is_some() {
                s.label = format!("^8[Retained] {}", s.label);
            }
            s.mod_list.push(id.to_string());
            if s.mod_list.len() == 2 {
                s.label = gsub(&gsub(&s.label, range_pat, "#"), "%-?%d+%.?%d*", "#");
                s.have_range = true;
            }
        }
        let selected = out
            .iter()
            .position(|e| match e {
                Some(s) => s.mod_list.contains(&sel),
                None => sel == "None",
            })
            .unwrap_or(0);
        Ok((out, selected))
    }
}

/// The combined line of two lines with the same stat: each number of the
/// first plus the next one of the second
fn combine(first: &str, second: &str) -> Result<String> {
    let mut start = 1;
    let mut err = None;
    let out = pat::gsub_fn(first, "%d+", |caps| {
        let num = &caps[0];
        match pat::captures_at(second, "()(%d+)()", start) {
            Some(c) => {
                let other = &c[1];
                start = c[2].parse::<usize>().unwrap_or(start);
                let sum = num.parse::<f64>().unwrap_or(0.0) + other.parse::<f64>().unwrap_or(0.0);
                Some(pat::num(sum))
            }
            None => {
                err = Some("attempt to perform arithmetic on local 'e' (a nil value)".to_string());
                None
            }
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(out),
    }
}
