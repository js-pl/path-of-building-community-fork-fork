//! Helpers for the equivalence tests in `tests/`: paths to upstream and the
//! oracle outputs, the engine's inputs, and engine values encoded exactly as
//! `oracle/json.lua` encodes upstream's.

#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use engine::data::Data;
use engine::skill_data::{Gem, GrantedEffect, SkillData};
use lua::Value;
use std::path::PathBuf;

pub mod bench;
pub mod bridge;
pub mod outputs;
pub mod typed_build;
pub mod typed_json;
pub mod variants;

/// The repository root.
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Upstream's `src/` directory.
pub fn upstream_src() -> PathBuf {
    root().join("upstream/PathOfBuilding/src")
}

/// `tests/oracle/`
pub fn oracle_dir() -> PathBuf {
    root().join("tests/oracle")
}

/// Corpus builds as `oracle/run.py` lists them: (id, path).
pub fn corpus() -> Vec<(String, PathBuf)> {
    let text = std::fs::read_to_string(root().join("corpus/corpus.toml")).unwrap();
    let value = |line: &str| {
        line.split_once('=')
            .map(|(_, v)| v.trim().trim_matches('"').to_string())
    };
    let mut builds = Vec::new();
    let mut id = None;
    let (mut first_seed, mut count) = (0, 0);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("id =") {
            id = value(line);
        } else if line.starts_with("path =") {
            builds.push((
                id.take().expect("id before path"),
                root().join(value(line).unwrap()),
            ));
        } else if line.starts_with("first_seed =") {
            first_seed = value(line).unwrap().parse().unwrap();
        } else if line.starts_with("count =") {
            count = value(line).unwrap().parse().unwrap();
        }
    }
    for seed in first_seed..first_seed + count {
        builds.push((
            format!("gen-{seed:04}"),
            root().join(format!("corpus/generated/gen-{seed:04}.xml")),
        ));
    }
    builds
}

/// Imports upstream's data and processes it.
pub fn load_data() -> (Data, importer::Raw) {
    let raw = importer::import(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let data = Data::new(raw.globals.clone(), raw.data.clone());
    (data, raw)
}

/// The typed game data, made by `gamedata` and read from its binary form.
pub fn game_data() -> engine::game_data::GameData {
    let bytes = gamedata::build(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    engine::game_data::GameData::decode(&bytes).unwrap_or_else(|e| panic!("{e}"))
}

/// The mod parser's typed data, made by `gamedata`
pub fn parser_data() -> engine::mod_parser::ParserData {
    gamedata::mod_parser::parser_data(&upstream_src()).unwrap_or_else(|e| panic!("{e}"))
}

/// The mod parser from its binary form, without upstream's mod cache
pub fn mod_parser() -> engine::mod_parser::ModParser {
    let mut data = parser_data();
    data.cache.clear();
    engine::mod_parser::ModParser::decode(&data.encode()).unwrap_or_else(|e| panic!("{e}"))
}

/// The mod parser from its binary form, with upstream's mod cache
pub fn mod_parser_preloaded() -> engine::mod_parser::ModParser {
    engine::mod_parser::ModParser::decode(&parser_data().encode()).unwrap_or_else(|e| panic!("{e}"))
}

/// JSON as `oracle/json.lua` writes it: `%.17g` numbers, sorted keys, arrays
/// for tables with keys 1..n, `{}` for empty tables, functions as
/// `{"$func":true}`.
pub fn to_json(v: &Value) -> String {
    let mut out = String::new();
    encode(v, &mut out, &|_| "{\"$func\":true}".to_string());
    out
}

/// Like [`to_json`], with Lua functions as `{"$func":"<file>:<line>"}`, as
/// `oracle/data.lua` writes them.
pub fn to_json_with_funcs(v: &Value) -> String {
    let mut out = String::new();
    encode(v, &mut out, &|v| match v {
        Value::Func(f) => format!("{{\"$func\":\"{}:{}\"}}", f.file, f.line),
        _ => "{\"$func\":true}".to_string(),
    });
    out
}

fn encode_str(s: &[u8], out: &mut String) {
    out.push('"');
    for ch in String::from_utf8_lossy(s).chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn encode(v: &Value, out: &mut String, func: &dyn Fn(&Value) -> String) {
    match v {
        Value::Nil => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_nan() {
                out.push_str("{\"$num\":\"nan\"}")
            } else if n.is_infinite() {
                out.push_str(if *n > 0.0 {
                    "{\"$num\":\"inf\"}"
                } else {
                    "{\"$num\":\"-inf\"}"
                })
            } else {
                out.push_str(&lua::format::format_g(*n, 17))
            }
        }
        Value::Str(s) => encode_str(s.as_bytes(), out),
        Value::Func(_) | Value::Closure(_) | Value::Function(_) => out.push_str(&func(v)),
        Value::Table(t) => {
            let entries = t.entries();
            let n = entries.len();
            let is_array = n > 0 && (1..=n as i64).all(|i| !t.get_int(i).is_nil());
            if is_array {
                out.push('[');
                for i in 1..=n as i64 {
                    if i > 1 {
                        out.push(',');
                    }
                    encode(&t.get_int(i), out, func);
                }
                out.push(']');
            } else {
                let mut keyed: Vec<(Vec<u8>, Value)> = entries
                    .into_iter()
                    .map(|(k, v)| (lua::to_string(&k.to_value()).as_bytes().to_vec(), v))
                    .collect();
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                out.push('{');
                for (i, (k, v)) in keyed.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    encode_str(k, out);
                    out.push(':');
                    encode(v, out, func);
                }
                out.push('}');
            }
        }
    }
}

/// The corpus builds' XML by id
pub fn corpus_builds() -> Vec<(String, String)> {
    corpus()
        .into_iter()
        .filter(|(_, p)| !p.extension().is_some_and(|e| e == "json"))
        .map(|(id, p)| (id, std::fs::read_to_string(&p).unwrap()))
        .collect()
}

/// The main skill of a build's XML, by its first gem's skill id
fn main_skill_id(xml: &str) -> Option<String> {
    let root = engine::xml::parse(xml).ok()?;
    let pob = root.child("PathOfBuilding")?;
    let main: usize = pob.child("Build")?.attr("mainSocketGroup")?.parse().ok()?;
    let skills = pob.child("Skills")?;
    let active = skills.attr("activeSkillSet").unwrap_or("1");
    let set = skills
        .elements()
        .find(|e| e.elem == "SkillSet" && e.attr("id") == Some(active))?;
    let group = set.elements().filter(|e| e.elem == "Skill").nth(main - 1)?;
    group
        .elements()
        .filter(|e| e.elem == "Gem")
        .find_map(|g| g.attr("skillId").map(String::from))
}

/// Every active gem `keep` selects, in each of its parts, as the main
/// skill (the first socket group) of a generated corpus build whose main
/// skill uses the same weapons or is a spell like it; ids
/// `fn-<skill>-<part>-<template>`
pub fn skill_variants(
    data: &SkillData,
    keep: impl Fn(&GrantedEffect) -> bool,
) -> Vec<(String, String)> {
    gem_variants(
        data,
        |g| keep(&g.granted_effect),
        |g| {
            let parts = g
                .granted_effect
                .parts
                .as_ref()
                .map_or(1, |p| p.len().max(1));
            (1..=parts)
                .map(|part| {
                    (
                        format!("fn-{}-{part}", g.id),
                        "mainActiveSkill=\"1\" mainActiveSkillCalcs=\"1\"".to_string(),
                        format!("skillPart=\"{part}\""),
                    )
                })
                .collect()
        },
    )
}

/// Every active gem `keep` selects as the main skill of a generated corpus
/// build whose main skill uses the same weapons or is a spell like it, once
/// for each of `variants`: the id's prefix, the socket group's attributes
/// and the gem's; ids `<prefix>-<template>`
pub fn gem_variants(
    data: &SkillData,
    keep: impl Fn(&Gem) -> bool,
    variants: impl Fn(&Gem) -> Vec<(String, String, String)>,
) -> Vec<(String, String)> {
    let builds: Vec<(String, String, Option<std::sync::Arc<GrantedEffect>>)> = corpus_builds()
        .into_iter()
        .filter(|(id, _)| id.starts_with("gen-"))
        .map(|(id, xml)| {
            let ge = main_skill_id(&xml).and_then(|s| data.skills.get(&s).cloned());
            (id, xml, ge)
        })
        .collect();
    let spell = |ge: &GrantedEffect| {
        ge.weapon_types.is_none() && ge.base_flags.iter().any(|f| f == "spell")
    };
    let mut out = Vec::new();
    for gem in data.gems() {
        let ge = &gem.granted_effect;
        if !keep(gem) || ge.support || gem.name.contains("Unknown") {
            continue;
        }
        let template = builds.iter().find(|(_, _, t)| {
            t.as_ref()
                .is_some_and(|t| match (&ge.weapon_types, &t.weapon_types) {
                    (Some(w), Some(tw)) => w.first().is_some_and(|f| tw.contains(f)),
                    (None, None) => spell(ge) == spell(t),
                    _ => false,
                })
        });
        let Some((tid, xml, _)) = template else {
            continue;
        };
        for (prefix, group, attrs) in variants(gem) {
            let gem_xml = format!(
                "<Gem count=\"1\" enableGlobal1=\"true\" enableGlobal2=\"true\" enabled=\"true\" gemId=\"{}\" level=\"{}\" nameSpec=\"{}\" quality=\"20\" skillId=\"{}\" variantId=\"{}\" {attrs}/>",
                gem.game_id, gem.natural_max_level, gem.name, gem.id, gem.variant_id
            );
            if let Some(v) = with_group(xml, &group, &gem_xml) {
                out.push((format!("{prefix}-{tid}"), v));
            }
        }
    }
    out
}

/// A build's XML with a socket group of `gems` before its others, as the
/// main skill's; `attrs` are the group's own
pub fn with_group(xml: &str, attrs: &str, gems: &str) -> Option<String> {
    let set = xml.find("<SkillSet id=\"1\">")?;
    let at = set + "<SkillSet id=\"1\">".len();
    let v = format!(
        "{}\n<Skill enabled=\"true\" label=\"Variant\" {attrs}>\n{gems}\n</Skill>\n{}",
        &xml[..at],
        &xml[at..]
    );
    Some(engine::pat::gsub(
        &v,
        "mainSocketGroup=\"%d+\"",
        "mainSocketGroup=\"1\"",
    ))
}

/// `Inflate`, which the timeless jewels' data needs: zlib streams, as
/// SimpleGraphic's (headless upstream has no implementation)
pub fn install_inflate(host: &upstream::host::Host) {
    host.env.set(
        "Inflate",
        lua::rt::func("Inflate", |a| {
            let data = a
                .first()
                .map(|v| lua::to_string(v).as_bytes().to_vec())
                .unwrap_or_default();
            Ok(vec![
                match miniz_oxide::inflate::decompress_to_vec_zlib(&data) {
                    Ok(out) => Value::Str(out.as_slice().into()),
                    Err(_) => Value::Nil,
                },
            ])
        }),
    );
}
