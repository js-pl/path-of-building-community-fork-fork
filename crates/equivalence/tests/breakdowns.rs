//! Breakdowns on the typed engine: the Calcs tab's calculation
//! (`calcs.buildOutput(build, "CALCS")`) leaves the player's and the
//! minion's breakdowns (`actor.breakdown`) as the translated program's do,
//! for every corpus build. Breakdowns of parts not ported yet are left out
//! and listed by key with `BREAKDOWN_MISSING=1`.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::breakdown::{Bd, BdVal};
use engine::calc_setup::SetupData;
use engine::item::ItemContext;
use engine::item_data::ItemData;
use engine::skill_data::SkillData;
use engine::tree::NodeType;
use equivalence::typed_build::Trees;
use equivalence::{corpus, mod_parser, upstream_src};
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

/// A translated breakdown value as JSON: tables by key, lines by their
/// index from 1, items by name; functions (the generators the module
/// table carries) are left out
fn lua_json(v: &Value, key: &str, depth: usize) -> Option<J> {
    match v {
        Value::Nil => None,
        Value::Bool(b) => Some(J::Bool(*b)),
        Value::Number(n) => Some(number(*n)),
        Value::Str(s) => Some(J::String(s.to_str_lossy().into_owned())),
        Value::Table(_) if key == "item" => {
            Some(json!({"$item": lua::to_string(&get(v, "name")).to_str_lossy()}))
        }
        Value::Table(_) if key == "mod" => Some(
            engine::mods::convert::Converter::default()
                .mod_(v)
                .map(|m| without_warcry_bonus(equivalence::typed_json::mod_(&m)))
                .unwrap_or_else(|e| J::String(format!("<mod: {e}>"))),
        ),
        Value::Table(_) if depth > 6 => Some(J::String("<deep>".into())),
        Value::Table(_) => {
            let mut m = Map::new();
            for (k, x) in entries(v) {
                let k = match k {
                    Key::Int(i) => i.to_string(),
                    Key::Str(s) => s.to_str_lossy().into_owned(),
                    k => format!("{k:?}"),
                };
                if let Some(j) = lua_json(&x, &k, depth + 1) {
                    m.insert(k, j);
                }
            }
            Some(J::Object(m))
        }
        _ => None,
    }
}

/// `CalcPerform` writes `warcryPowerBonus` onto the tag a skill's data
/// shares with every copy of the mod (an upstream quirk the typed perform
/// keeps local), so the translated mods carry it
fn without_warcry_bonus(mut j: J) -> J {
    if let J::Object(o) = &mut j {
        for (_, t) in o.iter_mut() {
            if let J::Object(tag) = t {
                tag.remove("warcryPowerBonus");
            }
        }
    }
    j
}

fn bd_val_json(v: &BdVal) -> J {
    match v {
        BdVal::Str(s) => J::String(s.to_string()),
        BdVal::Num(n) => number(*n),
        BdVal::Bool(b) => J::Bool(*b),
        BdVal::Table(t) => bd_json(t),
        BdVal::Item(n) => json!({"$item": &**n}),
        BdVal::Mod(m) => equivalence::typed_json::mod_(m),
        BdVal::Value(v) => equivalence::typed_json::value(v),
    }
}

fn bd_json(b: &Bd) -> J {
    let mut m = Map::new();
    for (i, l) in b.lines.iter().enumerate() {
        m.insert((i + 1).to_string(), bd_val_json(l));
    }
    for (k, v) in &b.fields {
        m.insert(k.clone(), bd_val_json(v));
    }
    J::Object(m)
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

/// Compares breakdowns key by key: keys only the translation has are
/// counted as missing (not ported yet), keys only the typed engine has are
/// differences. The weapon passes' breakdowns are compared the same way.
fn compare_keys(
    got: &Map<String, J>,
    want: &Map<String, J>,
    path: &str,
    d: &mut Vec<String>,
    missing: &mut BTreeMap<String, usize>,
) {
    for (k, w) in want {
        let p = format!("{path}.{k}");
        match (got.get(k), w) {
            (Some(J::Object(g)), J::Object(w)) if k == "MainHand" || k == "OffHand" => {
                compare_keys(g, w, &p, d, missing)
            }
            (Some(g), _) => diff(g, w, &p, d, 30),
            (None, _) => *missing.entry(p).or_default() += 1,
        }
    }
    for k in got.keys() {
        if !want.contains_key(k) {
            d.push(format!("{path}.{k}: extra"));
        }
    }
}

#[test]
fn breakdowns_match_upstream() {
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
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let calcs_env = get(&get(&build, "calcsTab"), "calcsEnv");
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
        let out = match calcs.build_outputs(&mut typed_build) {
            Ok(o) => o.1,
            Err(e) if e.contains("not ported") => {
                skipped += 1;
                continue;
            }
            Err(e) => {
                failures.push(format!("{id}: typed calculation failed: {e}"));
                continue;
            }
        };
        compared += 1;
        let mut d = Vec::new();
        for (who, got) in [
            ("player", &out.player_breakdown),
            ("minion", &out.minion_breakdown),
        ] {
            let want = lua_json(&get(&get(&calcs_env, who), "breakdown"), "", 0);
            let (Some(J::Object(want)), Some(got)) = (want, got) else {
                continue;
            };
            let J::Object(got) = bd_json(got) else {
                unreachable!()
            };
            compare_keys(&got, &want, who, &mut d, &mut missing);
        }
        if !d.is_empty() {
            failures.push(format!("{id} (got != want):\n  {}", d.join("\n  ")));
        }
    }
    eprintln!("{compared} builds compared, {skipped} skipped");
    if std::env::var("BREAKDOWN_MISSING").is_ok() {
        let mut m: Vec<_> = missing.into_iter().collect();
        m.sort_by(|a, b| b.1.cmp(&a.1));
        for (k, n) in m {
            eprintln!("missing {k}: {n}");
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
