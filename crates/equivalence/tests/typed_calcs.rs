//! Whole builds on the typed engine: `engine::calcs` (`calcs.buildOutput`
//! with Full DPS) calculates every corpus build as the oracle did
//! (`tests/oracle/<id>.json`, `MAIN`). Builds that reach a part not ported
//! yet are skipped and counted.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::actor::OutVal;
use engine::calc_setup::SetupData;
use engine::item::ItemContext;
use engine::item_data::ItemData;
use engine::skill_data::SkillData;
use engine::tree::NodeType;
use equivalence::typed_build::Trees;
use equivalence::{corpus, mod_parser, oracle_dir, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
use serde_json::{Map, Value as J, json};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use upstream::host::Host;
use upstream::imported::Imported;

fn s(x: &str) -> Value {
    Value::str(x)
}

fn get(v: &Value, k: &str) -> Value {
    rt::index(v, &s(k)).unwrap_or_default()
}

fn describe(e: &LuaError) -> String {
    format!(
        "{}\n  {}",
        lua::to_string(e.value()),
        e.traceback().join("\n  ")
    )
}

fn entries(v: &Value) -> Vec<(Key, Value)> {
    v.as_table().map(|t| t.entries()).unwrap_or_default()
}

fn number(n: f64) -> J {
    if n.is_nan() {
        json!({"$num": "nan"})
    } else if n.is_infinite() {
        json!({"$num": if n > 0.0 { "inf" } else { "-inf" }})
    } else {
        json!({"$f": n.to_bits()})
    }
}

/// An output as the oracle's JSON, numbers by their bits
fn typed_json(v: &OutVal) -> J {
    match v {
        OutVal::Num(n) => number(*n),
        OutVal::Bool(b) => J::Bool(*b),
        OutVal::Str(x) => J::String(x.to_string()),
        OutVal::Table(t) => J::Object(t.iter().map(|(k, v)| (k.clone(), typed_json(v))).collect()),
        OutVal::List(l) if l.is_empty() => J::Object(Map::new()),
        OutVal::List(l) => J::Array(l.iter().map(typed_json).collect()),
        OutVal::Gem(n) => json!({"$gem": &**n}),
        OutVal::Item(n) => json!({"$item": &**n}),
    }
}

/// The oracle's JSON with numbers by their bits; empty tables are `{}` or
/// `[]` as the encoder chose, and compare as empty objects
fn oracle_json(v: &J) -> J {
    match v {
        J::Number(n) => number(n.as_f64().unwrap()),
        J::Array(a) if a.is_empty() => J::Object(Map::new()),
        J::Array(a) => J::Array(a.iter().map(oracle_json).collect()),
        J::Object(o) => J::Object(o.iter().map(|(k, v)| (k.clone(), oracle_json(v))).collect()),
        v => v.clone(),
    }
}

fn output_json(o: &HashMap<engine::mods::Sym, OutVal>) -> J {
    let m: BTreeMap<String, J> = o
        .iter()
        .filter(|(k, _)| k.as_str() != "Minion")
        .map(|(k, v)| (k.as_str().to_string(), typed_json(v)))
        .collect();
    J::Object(m.into_iter().collect())
}

fn diff(a: &J, b: &J, path: &str, out: &mut Vec<String>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    match (a, b) {
        (J::Object(x), J::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                match (x.get(k), y.get(k)) {
                    (Some(p), Some(q)) => diff(p, q, &format!("{path}.{k}"), out, limit),
                    (p, q) => out.push(format!("{path}.{k}: {} != {}", fmt(p), fmt(q))),
                }
            }
        }
        (J::Array(x), J::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                diff(p, q, &format!("{path}[{i}]"), out, limit);
            }
        }
        _ if a != b => out.push(format!("{path}: {} != {}", fmt(Some(a)), fmt(Some(b)))),
        _ => {}
    }
}

/// A value for a message: numbers as numbers again
fn fmt(v: Option<&J>) -> String {
    fn plain(v: &J) -> J {
        match v {
            J::Object(o) if o.len() == 1 && o.contains_key("$f") => {
                json!(f64::from_bits(o["$f"].as_u64().unwrap()))
            }
            J::Object(o) => J::Object(o.iter().map(|(k, v)| (k.clone(), plain(v))).collect()),
            J::Array(a) => J::Array(a.iter().map(plain).collect()),
            v => v.clone(),
        }
    }
    match v {
        None => "missing".into(),
        Some(v) => {
            let s = plain(v).to_string();
            if s.len() > 120 {
                format!("{}...", &s[..120])
            } else {
                s
            }
        }
    }
}

#[test]
fn typed_calcs_match_the_oracle() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let ldata = host.global("data");
    let config_data = engine::config::ConfigData::from_lua(&ldata, &host.global("SkillType"))
        .unwrap_or_else(|e| panic!("config data: {e}"));
    let var_list = rt::call(&host.global("LoadModule"), &[s("Modules/ConfigOptions")])
        .unwrap_or_else(|e| panic!("{}", describe(&e)))
        .into_iter()
        .next()
        .unwrap();
    let option_defaults = engine::config_tab::option_defaults(&var_list);
    let setup_data =
        SetupData::from_lua(&ldata, &host.global("ModFlag"), &host.global("KeywordFlag"))
            .unwrap_or_else(|e| panic!("setup data: {e}"));
    let item_data = ItemData::from_lua(&ldata, &host.global("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let skill_data = SkillData::from_lua(
        &ldata,
        &host.global("colorCodes"),
        &host.global("SkillType"),
    )
    .unwrap_or_else(|e| panic!("skill data: {e}"));
    let radius: Vec<String> = entries(&get(&ldata, "jewelRadius"))
        .into_iter()
        .map(|(_, r)| {
            lua::to_string(&get(&r, "label"))
                .to_str_lossy()
                .into_owned()
        })
        .collect();
    let one_hand_map: HashMap<String, bool> = setup_data
        .weapon_type_info
        .iter()
        .map(|(k, v)| (k.clone(), v.one_hand))
        .collect();
    let one_hand = |t: &str| one_hand_map.get(t).copied();
    let mut trees = Trees::new(&lua::to_string(&host.global("latestTreeVersion")).to_str_lossy());
    let latest = trees.latest().to_string();
    let socket_ids: Vec<u32> = trees
        .get(&latest, &parser)
        .nodes
        .iter()
        .filter(|n| n.kind == NodeType::Socket)
        .map(|n| n.id)
        .collect();
    let calc_data = engine::calc_env::CalcData {
        setup: &setup_data,
        items: &item_data,
        skills: &skill_data,
        parser: &parser,
        jewel_radius: &radius,
    };
    let mut failures = Vec::new();
    let (mut compared, mut skipped) = (0, 0);
    let mut skip_reasons: BTreeMap<String, usize> = BTreeMap::new();
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        if !entries(&get(&get(&build, "spec"), "subGraphs")).is_empty() {
            *skip_reasons
                .entry("cluster jewel subgraphs".into())
                .or_default() += 1;
            skipped += 1;
            continue;
        }
        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        let version = engine::build::tree_version(&root);
        let tree = trees.get(&version, &parser);
        let load_ctx = engine::build::LoadContext {
            items: ItemContext {
                data: &item_data,
                parser: &parser,
                jewel_radius: &radius,
                default_affix_quality: 0.5,
            },
            skill_data: &skill_data,
            config_data: &config_data,
            option_defaults: &option_defaults,
            parser: &parser,
            socket_ids: &socket_ids,
            one_hand: &one_hand,
            minions: &|m: &str| setup_data.minions.contains_key(m),
        };
        let mut typed_build = match engine::build::Build::load(&root, &load_ctx, tree, &parser) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{id}: build: {e}"));
                continue;
            }
        };
        let calcs = engine::calcs::Calcs::new(&calc_data);
        let out = match calcs
            .build_output_main(&mut typed_build)
            .and_then(|mut o| engine::calcs::refresh_stat_list(&mut o).map(|_| o))
        {
            Ok(o) => o,
            Err(e) if e.contains("not ported") => {
                let e = e[e.find("not ported").unwrap()..].to_string();
                *skip_reasons.entry(e).or_default() += 1;
                skipped += 1;
                continue;
            }
            Err(e) => {
                failures.push(format!("{id}: typed calculation failed: {e}"));
                continue;
            }
        };
        compared += 1;
        let want: J = serde_json::from_str(
            &std::fs::read_to_string(oracle_dir().join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        let want = oracle_json(&want["MAIN"]);
        let mut got = Map::new();
        got.insert("player".into(), output_json(&out.player));
        if let Some(m) = &out.minion {
            got.insert("minion".into(), output_json(m));
        }
        let got = J::Object(got);
        let mut d = Vec::new();
        diff(&got, &want, "", &mut d, 20);
        if !d.is_empty() {
            failures.push(format!(
                "{id}: differs from the oracle (got != want):\n  {}",
                d.join("\n  ")
            ));
        }
    }
    eprintln!("{compared} builds compared, {skipped} skipped: {skip_reasons:?}");
    assert!(
        failures.is_empty(),
        "{} of {compared} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
