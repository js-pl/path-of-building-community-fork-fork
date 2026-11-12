//! `ItemClass:BuildRaw` of `Classes/Item.lua`: an item's text as upstream
//! writes it back, which saving a build stores.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::item::{CATALYSTS, INFLUENCES, Item, ModLine};
use crate::item_tools::{Range, round};
use crate::mods::ModValue;
use crate::pat::{is_match, num};
use std::collections::BTreeSet;

fn value(v: &ModValue) -> Option<f64> {
    match v {
        ModValue::Number(n) => Some(*n),
        _ => None,
    }
}

fn id_spec(ids: &BTreeSet<i64>) -> String {
    ids.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// `writeModLine(modLine)`
fn mod_line(m: &ModLine) -> String {
    let mut line = m.line.clone();
    let prepend_all = |line: &str, prefix: &str| {
        format!("{prefix}{}", line.replace('\n', &format!("\n{prefix}")))
    };
    if let Some(r) = m.range
        && is_match(&line, "%(%-?[%d%.]+%-%-?[%d%.]+%)")
    {
        line = format!("{{range:{}}}{line}", num(round(r, Some(6.0))));
    }
    if let Some(r) = m.corrupted_range {
        line = format!("{{corruptedRange:{}}}{line}", num(round(r, Some(2.0))));
    }
    let f = &m.flags;
    for (on, tag) in [
        (f.disabled, "disabled"),
        (f.crafted, "crafted"),
        (f.enchant, "enchant"),
        (f.custom, "custom"),
        (f.scourge, "scourge"),
        (f.crucible, "crucible"),
        (f.mutated, "mutated"),
    ] {
        if on {
            line = format!("{{{tag}}}{line}");
        }
    }
    if let Some(g) = &m.mod_group {
        line = prepend_all(&line, &format!("{{modGroup:{g}}}"));
    }
    for (on, tag) in [
        (f.fractured, "fractured"),
        (f.prefix, "prefix"),
        (f.suffix, "suffix"),
        (f.exarch, "exarch"),
        (f.eater, "eater"),
        (f.synthesis, "synthesis"),
        (f.unscalable, "unscalable"),
        (f.vestigial, "vestigial"),
    ] {
        if on {
            line = format!("{{{tag}}}{line}");
        }
    }
    let new_selection = m.version_list.is_some() || m.variant_group_list.is_some();
    let tags = (!m.mod_tags.is_empty()).then(|| format!("{{tags:{}}}", m.mod_tags.join(",")));
    if new_selection && let Some(t) = &tags {
        line = format!("{t}{line}");
    }
    if let Some(l) = &m.variant_group_list {
        line = prepend_all(&line, &format!("{{group:{}}}", id_spec(l)));
    }
    if let Some(l) = &m.variant_list {
        line = prepend_all(&line, &format!("{{variant:{}}}", id_spec(l)));
    }
    if let Some(l) = &m.version_list {
        line = prepend_all(&line, &format!("{{version:{}}}", id_spec(l)));
    }
    if !new_selection && let Some(t) = &tags {
        line = format!("{t}{line}");
    }
    line
}

const ALT: [&str; 5] = ["", " Two", " Three", " Four", " Five"];

impl Item {
    /// `BuildRaw()`
    pub fn build_raw(&self) -> String {
        let mut raw: Vec<String> = vec![format!("Rarity: {}", self.rarity)];
        let base_name = self.base_name.clone().unwrap_or_default();
        match &self.title {
            Some(t) => {
                raw.push(t.clone());
                raw.push(base_name.clone());
            }
            None => raw.push(format!(
                "{}{base_name}{}",
                self.name_prefix, self.name_suffix
            )),
        }
        if let Some(a) = &self.armour_data {
            for t in ["Armour", "Evasion", "EnergyShield", "Ward"] {
                if let Some(v) = a.get(t).and_then(value).filter(|v| *v > 0.0) {
                    raw.push(format!(
                        "{}: {}",
                        t.replace("EnergyShield", "Energy Shield"),
                        num(v)
                    ));
                    if let Some(p) = a.get(&format!("{t}BasePercentile")).and_then(value) {
                        raw.push(format!("{t}BasePercentile: {}", num(p)));
                    }
                }
            }
        }
        if let Some(i) = self.intangibility {
            raw.push(format!("Intangibility: {}%", i as i64));
        }
        if let Some(u) = &self.unique_id {
            raw.push(format!("Unique ID: {u}"));
        }
        if let Some(l) = &self.league {
            raw.push(format!("League: {l}"));
        }
        if self.unreleased {
            raw.push("Unreleased: true".into());
        }
        for (i, (_, display)) in INFLUENCES.iter().enumerate() {
            if self.influences[i] {
                raw.push(format!("{display} Item"));
            }
        }
        if self.crafted {
            raw.push("Crafted: true".into());
            for (kind, list) in [("Prefix", &self.prefixes), ("Suffix", &self.suffixes)] {
                for a in &list.list {
                    let range = match &a.range {
                        Some(Range::Each(v)) => format!(
                            "{{range:{}}}",
                            v.iter().map(|x| num(*x)).collect::<Vec<_>>().join(",")
                        ),
                        Some(Range::One(r)) => format!("{{range:{}}}", num(round(*r, Some(3.0)))),
                        None => String::new(),
                    };
                    let fractured = if a.fractured { "{fractured}" } else { "" };
                    raw.push(format!("{kind}: {fractured}{range}{}", a.mod_id));
                }
            }
        }
        if let Some(c) = self.catalyst.filter(|c| *c > 0) {
            raw.push(format!("Catalyst: {}", CATALYSTS[c - 1]));
        }
        if let Some(q) = self.catalyst_quality {
            raw.push(format!("CatalystQuality: {}", num(q)));
        }
        if self.cluster_jewel_size_index.is_some() {
            if let Some(s) = &self.cluster_jewel_skill {
                raw.push(format!("Cluster Jewel Skill: {s}"));
            }
            if let Some(n) = self.cluster_jewel_node_count {
                raw.push(format!("Cluster Jewel Node Count: {}", num(n)));
            }
        }
        if let Some(t) = self.talisman_tier {
            raw.push(format!("Talisman Tier: {}", num(t)));
        }
        if let Some(l) = self.item_level {
            raw.push(format!("Item Level: {}", num(l)));
        }
        if let Some(m) = self.memory_strands {
            raw.push(format!("Memory Strands: {}", num(m)));
        }
        if let Some(versions) = &self.version_list {
            for v in versions {
                raw.push(format!("Version: {v}"));
            }
            if let Some(s) = self.selected_version {
                raw.push(format!("Selected Version: {}", num(s)));
            }
        }
        if let Some(variants) = &self.variant_list {
            for v in variants {
                raw.push(format!("Variant: {v}"));
            }
            if self.uses_variant_groups {
                for g in self.variant_groups.keys() {
                    if let Some(v) = self.variant_group_selections.get(g) {
                        raw.push(format!("Selected Variant Group: {g}={}", num(*v)));
                    }
                }
            } else {
                raw.push(format!(
                    "Selected Variant: {}",
                    self.variant.map(num).unwrap_or_default()
                ));
            }
            for b in &self.base_lines {
                if b.variant_list.is_some()
                    || b.version_list.is_some()
                    || b.variant_group_list.is_some()
                {
                    raw.push(mod_line(b));
                }
            }
            if !self.uses_variant_groups {
                for (i, name) in ALT.iter().enumerate() {
                    if self.has_alt_variant[i] {
                        raw.push(format!("Has Alt Variant{name}: true"));
                        raw.push(format!(
                            "Selected Alt Variant{name}: {}",
                            self.variant_alt[i].map(num).unwrap_or_default()
                        ));
                    }
                }
            }
            if self.allow_duplicate_variants {
                raw.push("Allow Duplicate Variants: true".into());
            }
        } else {
            for b in &self.base_lines {
                if b.version_list.is_some() || b.variant_group_list.is_some() {
                    raw.push(mod_line(b));
                }
            }
        }
        if let Some(q) = self.quality {
            raw.push(format!("Quality: {}", num(q)));
        }
        if !self.sockets.is_empty() {
            let mut line = "Sockets: ".to_string();
            for (i, s) in self.sockets.iter().enumerate() {
                line.push(s.color);
                if let Some(next) = self.sockets.get(i + 1) {
                    line.push(if s.group == next.group { '-' } else { ' ' });
                }
            }
            raw.push(line);
        }
        if let Some(l) = self.requirements.level {
            raw.push(format!("LevelReq: {}", num(l)));
        }
        if let Some(r) = &self.jewel_radius_label {
            raw.push(format!("Radius: {r}"));
        }
        if let Some(l) = self.limit {
            raw.push(format!("Limited to: {}", num(l)));
        }
        if let Some(c) = &self.class_restriction {
            raw.push(format!("Requires Class {c}"));
        }
        raw.push(format!(
            "Implicits: {}",
            self.enchant_mod_lines.len()
                + self.implicit_mod_lines.len()
                + self.scourge_mod_lines.len()
        ));
        for list in [
            &self.enchant_mod_lines,
            &self.scourge_mod_lines,
            &self.class_requirement_mod_lines,
            &self.implicit_mod_lines,
            &self.explicit_mod_lines,
            &self.crucible_mod_lines,
        ] {
            raw.extend(list.iter().map(mod_line));
        }
        for (on, line) in [
            (self.split, "Split"),
            (self.mirrored, "Mirrored"),
            (self.fractured, "Fractured Item"),
            (self.corrupted || self.scourge, "Corrupted"),
        ] {
            if on {
                raw.push(line.into());
            }
        }
        if let Some(f) = &self.foil_type {
            raw.push(format!("Foil Unique ({f})"));
        }
        raw.join("\n")
    }
}
