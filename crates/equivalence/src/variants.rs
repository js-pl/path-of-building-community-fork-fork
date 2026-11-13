//! Corpus builds mutated to reach parts of the calculations the corpus
//! does not: each trigger configuration of `CalcTriggers.lua`
//! (`configTable`), by the gems and uniques its keys name.

use crate::typed_build::TypedData;
use crate::{corpus_builds, upstream_src, with_group};
use engine::skill_data::{Gem, GrantedEffect};
use std::sync::Arc;

/// A generated corpus build as a template: its id, XML and main gem
struct Template {
    id: String,
    xml: String,
    gem: Arc<Gem>,
}

/// A gem as a build's `<Gem>` element
pub fn gem_xml(gem: &Gem, attrs: &str) -> String {
    format!(
        "<Gem count=\"1\" enableGlobal1=\"true\" enableGlobal2=\"true\" enabled=\"true\" gemId=\"{}\" level=\"{}\" nameSpec=\"{}\" quality=\"20\" skillId=\"{}\" variantId=\"{}\" {attrs}/>",
        gem.game_id, gem.natural_max_level, gem.name, gem.id, gem.variant_id
    )
}

/// The first generated corpus build of each kind of main skill: melee,
/// bow, wand and spell
fn templates(data: &TypedData) -> Vec<Template> {
    let mut kinds: Vec<(&str, Template)> = Vec::new();
    for (id, xml) in corpus_builds() {
        if !id.starts_with("gen-") {
            continue;
        }
        let Some(gem) = crate::main_skill_id(&xml)
            .and_then(|s| data.skills.gem_for_skill(&s))
            .cloned()
        else {
            continue;
        };
        let ge = &gem.granted_effect;
        let kind = match &ge.weapon_types {
            Some(w) if w.iter().any(|t| t == "Bow") => "bow",
            Some(w) if w.iter().any(|t| t == "Wand") => "wand",
            Some(w) if w.iter().any(|t| t.starts_with("One Handed")) => "melee",
            None if ge.base_flags.iter().any(|f| f == "spell") && ge.minion_list.is_none() => {
                "spell"
            }
            _ => continue,
        };
        if !kinds.iter().any(|(k, _)| *k == kind) {
            kinds.push((kind, Template { id, xml, gem }));
        }
    }
    kinds.into_iter().map(|(_, t)| t).collect()
}

/// The lower case names a trigger configuration can be found by
fn trigger_key(ge: &GrantedEffect) -> Option<String> {
    let name = ge.name.to_lowercase();
    let name = name.strip_prefix("awakened ").unwrap_or(&name).to_string();
    let keys = engine::calc_offence::TRIGGER_CONFIGS;
    [ge.id.clone(), name]
        .into_iter()
        .find(|k| keys.contains(&k.as_str()))
}

/// The number of active skills a socket group of these gems has
fn active_count(gems: &[&Gem]) -> usize {
    gems.iter()
        .map(|g| {
            usize::from(!g.granted_effect.support)
                + usize::from(
                    g.secondary_granted_effect
                        .as_ref()
                        .is_some_and(|s| !s.support),
                )
        })
        .sum()
}

/// A build with an item added and equipped in `slot` of its active item
/// set; the item's id
fn with_item(xml: &str, text: &str, slot: &str) -> Option<(String, u32)> {
    let id = engine::pat::gmatch(xml, "<Item id=\"(%d+)\"")
        .iter()
        .filter_map(|c| c[0].parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let at = xml.find("<ItemSet ")?;
    let item = format!(
        "<Item id=\"{id}\">\nRarity: UNIQUE\n{}\n</Item>\n",
        text.trim()
    );
    let mut v = format!("{}{item}{}", &xml[..at], &xml[at..]);
    let set_start = v.find("<ItemSet ")?;
    let set_end = set_start + v[set_start..].find("</ItemSet>")?;
    let tag = format!("name=\"{slot}\"/>");
    let slot_at = set_start + v[set_start..set_end].find(&tag)?;
    let line_start = v[..slot_at].rfind("<Slot ")?;
    v.replace_range(
        line_start..slot_at + tag.len(),
        &format!("<Slot itemId=\"{id}\" itemPbURL=\"\" {tag}"),
    );
    Some((v, id))
}

/// Upstream's unique items (`Data/Uniques/*.lua`): each one's text
pub fn uniques() -> Vec<String> {
    let mut out = Vec::new();
    let dir = upstream_src().join("Data/Uniques");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "lua"))
        .collect();
    files.sort();
    for f in files {
        let text = std::fs::read_to_string(f).unwrap();
        let mut rest = text.as_str();
        while let Some(s) = rest.find("[[") {
            let Some(e) = rest[s..].find("]]") else { break };
            out.push(rest[s + 2..s + e].trim().to_string());
            rest = &rest[s + e + 2..];
        }
    }
    out
}

/// Every trigger configuration's gem as the main skill: a support with a
/// spell, hex, mark or warcry it can support, an active skill by itself,
/// each next to the main skill of a melee, bow, wand and spell build; and
/// every unique a configuration names or that triggers skills, equipped in
/// a build, with each socket group it grants and a spell socketed in its
/// slot as the main skill. Ids `trig-<gem or item>-<skill>-<template>`
pub fn trigger_variants(data: &mut TypedData) -> Vec<(String, String)> {
    let templates = templates(data);
    let by_name = |n: &str| data.skills.gems().iter().find(|g| g.name == n).cloned();
    let candidates: Vec<Arc<Gem>> = ["Fireball", "Frostbite", "Sniper's Mark", "Intimidating Cry"]
        .iter()
        .filter_map(|n| by_name(n))
        .collect();
    let cyclone = by_name("Cyclone");
    let mut out = Vec::new();
    for gem in data.skills.gems() {
        if gem.name.contains("Unknown") {
            continue;
        }
        let ge = &gem.granted_effect;
        let key = trigger_key(ge).or_else(|| {
            gem.secondary_granted_effect
                .as_deref()
                .and_then(trigger_key)
        });
        if key.is_none() {
            continue;
        }
        let mut groups: Vec<Vec<Arc<Gem>>> = Vec::new();
        if ge.support {
            for c in &candidates {
                let types = &c.granted_effect.skill_types;
                let m = |check: &[i64]| {
                    engine::calc_skills::type_expression_matches(check, types, None, &data.skills)
                };
                if (ge.require_skill_types.is_empty() || m(&ge.require_skill_types))
                    && !m(&ge.exclude_skill_types)
                {
                    let mut g = vec![gem.clone(), c.clone()];
                    if ge.name == "Cast while Channelling" {
                        g.extend(cyclone.clone());
                    }
                    groups.push(g);
                }
            }
            if groups.is_empty() {
                groups.push(vec![gem.clone()]);
            }
        } else {
            groups.push(vec![gem.clone()]);
        }
        for t in &templates {
            for g in &groups {
                let mut gems: Vec<&Gem> = vec![&t.gem];
                gems.extend(g.iter().map(|g| &**g));
                let xml: String = gems
                    .iter()
                    .map(|g| gem_xml(g, ""))
                    .collect::<Vec<_>>()
                    .join("\n");
                let last = g.last().map_or("", |g| g.id.as_str());
                for main in 1..=active_count(&gems) {
                    let attrs =
                        format!("mainActiveSkill=\"{main}\" mainActiveSkillCalcs=\"{main}\"");
                    if let Some(v) = with_group(&t.xml, &attrs, &xml) {
                        out.push((format!("trig-{}-{last}-{main}-{}", gem.id, t.id), v));
                    }
                }
            }
        }
    }
    let fireball = by_name("Fireball");
    let slam = by_name("Ground Slam");
    let ctx = engine::item::ItemContext {
        data: &data.items,
        parser: &data.parser,
        jewel_radius: &data.radius,
        default_affix_quality: 0.5,
    };
    let keys = engine::calc_offence::TRIGGER_CONFIGS;
    let mut items = Vec::new();
    for text in uniques() {
        let name = text.lines().next().unwrap_or("").to_lowercase();
        if !keys.contains(&name.as_str()) && !text.contains("rigger") {
            continue;
        }
        let Ok(item) = engine::item::Item::new(&ctx, &text, Some("UNIQUE"), false) else {
            continue;
        };
        let slot = item.primary_slot();
        if slot == "Jewel" || slot == "Flask 1" || slot == "Graft 1" {
            continue;
        }
        items.push((item.name.clone(), text, slot));
    }
    let Some(fireball) = fireball else {
        return out;
    };
    for (name, text, slot) in items {
        for t in &templates {
            let Some((xml, id)) = with_item(&t.xml, &text, &slot) else {
                continue;
            };
            let short: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            // A spell socketed in the item's slot
            let attrs = format!("slot=\"{slot}\" mainActiveSkill=\"1\" mainActiveSkillCalcs=\"1\"");
            if let Some(v) = with_group(&xml, &attrs, &gem_xml(&fireball, "")) {
                out.push((format!("trig-{short}-socketed-{}", t.id), v));
            }
            // Each socket group the item grants, next to a slam and a
            // spell of their own
            let Some(xml) = slam.as_ref().and_then(|g| {
                let gems = format!("{}\n{}", gem_xml(g, ""), gem_xml(&fireball, ""));
                with_group(
                    &xml,
                    "mainActiveSkill=\"1\" mainActiveSkillCalcs=\"1\"",
                    &gems,
                )
            }) else {
                continue;
            };
            let root = engine::xml::parse(&xml).unwrap();
            data.load_tree(&root);
            let Ok(mut build) = data.load_build(&root) else {
                continue;
            };
            let Ok(env) = engine::calc_env::init_env(
                &mut build,
                &data.calc_data(),
                engine::calc_env::CalcMode::Main,
            ) else {
                continue;
            };
            let source = format!("Item:{id}:");
            // Numbered among the item's own groups, so a group has the
            // same id in every template
            let mut granted = 0;
            for (k, g) in env.groups.iter().enumerate() {
                if g.source.as_deref().is_some_and(|s| s.starts_with(&source)) {
                    granted += 1;
                    let v = engine::pat::gsub(
                        &xml,
                        "mainSocketGroup=\"%d+\"",
                        &format!("mainSocketGroup=\"{}\"", k + 1),
                    );
                    out.push((format!("trig-{short}-granted{granted}-{}", t.id), v));
                }
            }
        }
    }
    out
}

/// One build of each of [`trigger_variants`]' gems, items, socket groups
/// and main skills: its templates in turn, so every trigger configuration
/// runs and every template is used. A trigger variant takes over a second
/// in the translated program, and the whole set half an hour
pub fn trigger_sample(all: Vec<(String, String)>) -> Vec<(String, String)> {
    // An id without its template
    let key = |id: &str| id.rsplit_once("-gen-").map_or(id, |(k, _)| k).to_string();
    let mut groups: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (id, xml) in all {
        let k = key(&id);
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, v)) => v.push((id, xml)),
            None => groups.push((k, vec![(id, xml)])),
        }
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(n, (_, mut v))| v.swap_remove(n % v.len()))
        .collect()
}

/// Mod lines the corpus does not reach, each given to a few generated
/// corpus builds as the Configuration tab's custom modifiers; ids
/// `mod-<k>-<template>`
pub fn mod_variants() -> Vec<(String, String)> {
    const LINES: [&str; 1] = [
        "Mana is increased by 50% of Overcapped Lightning Resistance&#10;+60% to Lightning Resistance",
    ];
    let builds: Vec<(String, String)> = corpus_builds()
        .into_iter()
        .filter(|(id, _)| id.starts_with("gen-"))
        .step_by(12)
        .collect();
    let mut out = Vec::new();
    for (k, line) in LINES.iter().enumerate() {
        for (id, xml) in &builds {
            let Some(at) = xml.find("<ConfigSet id=\"1\">") else {
                continue;
            };
            let at = at + "<ConfigSet id=\"1\">".len();
            let v = format!(
                "{}\n<Input name=\"customMods\" string=\"{line}\"/>{}",
                &xml[..at],
                &xml[at..]
            );
            out.push((format!("mod-{k}-{id}"), v));
        }
    }
    out
}

/// Upstream's tattoos (`Data/TattooPassives.lua`): name and target type
fn tattoos() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(upstream_src().join("Data/TattooPassives.lua")).unwrap();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        if let Some(c) = engine::pat::captures(line, "^\t\t%[\"(.-)\"%] = {") {
            name = Some(c[0].clone());
        } else if let Some(c) = engine::pat::captures(line, "%[\"targetType\"%] = \"(.-)\"")
            && let Some(n) = name.take()
            && !n.is_empty()
        {
            out.push((n, c[0].clone()));
        }
    }
    out.sort();
    out
}

/// Generated corpus builds with tattoos on allocated nodes of their active
/// spec: a keystone, small attribute nodes and a mastery; ids
/// `tattoo-<template>`
pub fn tattoo_variants(data: &mut TypedData) -> Vec<(String, String)> {
    use engine::tree::NodeType;
    let tattoos = tattoos();
    let mut out = Vec::new();
    for (n, (id, xml)) in corpus_builds().into_iter().enumerate() {
        if !id.starts_with("gen-") || n % 6 != 0 {
            continue;
        }
        let root = engine::xml::parse(&xml).unwrap();
        data.load_tree(&root);
        let Ok(build) = data.load_build(&root) else {
            continue;
        };
        let spec = &build.spec;
        let mut used = Vec::new();
        let mut overrides = String::new();
        for (target, pick) in [
            ("Keystone", None),
            ("Small Strength", Some("Strength")),
            ("Small Dexterity", Some("Dexterity")),
            ("Small Intelligence", Some("Intelligence")),
            ("Small Attribute", None),
            ("Mastery", None),
        ] {
            let choices: Vec<&String> = tattoos
                .iter()
                .filter(|(_, t)| t == target)
                .map(|(n, _)| n)
                .collect();
            let Some(tattoo) = choices.get(n % choices.len().max(1)) else {
                continue;
            };
            let node = spec.alloc_nodes.iter().map(|(_, &i)| i).find(|&i| {
                let t = spec.tree_node(i);
                !used.contains(&i)
                    && t.ascendancy_name.is_none()
                    && match target {
                        "Keystone" => t.kind == NodeType::Keystone,
                        "Mastery" => t.kind == NodeType::Mastery,
                        "Small Attribute" => {
                            t.kind == NodeType::Normal
                                && ["Strength", "Dexterity", "Intelligence"].contains(&&*t.dn)
                        }
                        _ => t.kind == NodeType::Normal && Some(&*t.dn) == pick,
                    }
            });
            if let Some(i) = node {
                used.push(i);
                overrides.push_str(&format!(
                    "<Override nodeId=\"{}\" dn=\"{tattoo}\"/>",
                    spec.nodes[i].id
                ));
            }
        }
        let Some(at) = xml.find("</Spec>") else {
            continue;
        };
        if overrides.is_empty() {
            continue;
        }
        let v = format!(
            "{}<Overrides>{overrides}</Overrides>\n{}",
            &xml[..at],
            &xml[at..]
        );
        out.push((format!("tattoo-{id}"), v));
    }
    out
}

/// Generated corpus builds with a timeless jewel of each kind and
/// conqueror socketed in the allocated socket with the most allocated
/// nodes in its radius, with a seed of their own; ids
/// `timeless-<jewel>-<variant>-<template>`
pub fn timeless_variants(data: &mut TypedData) -> Vec<(String, String)> {
    const JEWELS: [&str; 6] = [
        "Glorious Vanity",
        "Lethal Pride",
        "Brutal Restraint",
        "Militant Faith",
        "Elegant Hubris",
        "Heroic Tragedy",
    ];
    let texts: Vec<String> = uniques()
        .into_iter()
        .filter(|t| JEWELS.contains(&t.lines().next().unwrap_or("")))
        .collect();
    let mut out = Vec::new();
    for (n, (id, xml)) in corpus_builds().into_iter().enumerate() {
        if !id.starts_with("gen-") || n % 15 != 0 {
            continue;
        }
        let root = engine::xml::parse(&xml).unwrap();
        data.load_tree(&root);
        let Ok(build) = data.load_build(&root) else {
            continue;
        };
        let spec = &build.spec;
        let tree = spec.tree;
        let in_radius = |s: u32| {
            tree.node(s)
                .and_then(|t| t.nodes_in_radius.as_ref())
                .and_then(|r| r.get(2))
                .map_or(0, |r| {
                    r.iter().filter(|&&i| spec.alloc_nodes.contains(i)).count()
                })
        };
        let Some(socket) = tree
            .sockets
            .iter()
            .copied()
            .filter(|&s| {
                spec.alloc_nodes.contains(s)
                    && tree.node(s).is_some_and(|t| t.expansion_jewel.is_none())
            })
            .max_by_key(|&s| in_radius(s))
        else {
            continue;
        };
        for text in &texts {
            let name = text.lines().next().unwrap_or("");
            let variants = text
                .lines()
                .filter(|l| l.starts_with("Variant:"))
                .count()
                .max(1);
            for v in 1..=variants {
                // A seed of its own within the range
                let text = engine::pat::gsub_fn(text, "%((%d+)%-(%d+)%)", |c| {
                    let (lo, hi): (u64, u64) = (c[0].parse().ok()?, c[1].parse().ok()?);
                    let step = if hi > 100_000 { 20 } else { 1 };
                    let k = (n as u64 * 977 + v as u64 * 131) % ((hi - lo) / step);
                    Some((lo + k * step).to_string())
                });
                let Some(x) = with_socketed(&xml, &text, v, socket) else {
                    continue;
                };
                let short: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                out.push((format!("timeless-{short}-{v}-{id}"), x));
            }
        }
    }
    out
}

/// A build with a unique jewel of variant `variant` socketed in an
/// allocated socket of its active spec
fn with_socketed(xml: &str, text: &str, variant: usize, socket: u32) -> Option<String> {
    let id = engine::pat::gmatch(xml, "<Item id=\"(%d+)\"")
        .iter()
        .filter_map(|c| c[0].parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let at = xml.find("<ItemSet ")?;
    let mut x = format!(
        "{}<Item id=\"{id}\" variant=\"{variant}\">\nRarity: UNIQUE\n{text}\n</Item>\n{}",
        &xml[..at],
        &xml[at..]
    );
    let spec = x.find("<Spec ")?;
    let spec_end = spec + x[spec..].find("</Spec>")?;
    let tag = format!("nodeId=\"{socket}\"/>");
    match x[spec..spec_end].find(&tag) {
        Some(k) => {
            let k = spec + k;
            let start = x[..k].rfind("<Socket ")?;
            x.replace_range(
                start..k + tag.len(),
                &format!("<Socket itemId=\"{id}\" {tag}"),
            );
        }
        None => {
            let s_at = spec + x[spec..spec_end].find("<Sockets>")? + 9;
            x.insert_str(s_at, &format!("\n<Socket itemId=\"{id}\" {tag}"));
        }
    }
    Some(x)
}
