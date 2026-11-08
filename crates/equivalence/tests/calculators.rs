//! The typed calculators: `calcs.getMiscCalculator(build)`'s base pass
//! and its function with overrides (nodes added and removed, an item
//! taken out, a flask switched) give the player's outputs the translated
//! program's do, for every corpus build.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::actor::OutVal;
use engine::calc_env::Override;
use engine::calc_setup::SetupData;
use engine::item::ItemContext;
use engine::item_data::ItemData;
use engine::skill_data::SkillData;
use engine::tree::NodeType;
use equivalence::typed_build::Trees;
use equivalence::{corpus, mod_parser, upstream_src};
use lua::rt;
use lua::{Key, LuaError, TableRef, Value};
use serde_json::{Map, Value as J, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

fn lua_json(v: &Value, key: &str) -> J {
    match v {
        Value::Nil => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(n) => number(*n),
        Value::Str(x) => J::String(x.to_str_lossy().into_owned()),
        Value::Table(_) if key == "sourceGem" => {
            json!({"$gem": lua::to_string(&get(v, "nameSpec")).to_str_lossy()})
        }
        Value::Table(_) if key == "sourceItem" => {
            json!({"$item": lua::to_string(&get(v, "name")).to_str_lossy()})
        }
        Value::Table(_) => {
            let mut m = Map::new();
            for (k, x) in entries(v) {
                let k = lua::to_string(&k.to_value()).to_str_lossy().into_owned();
                let j = lua_json(&x, &k);
                m.insert(k, j);
            }
            J::Object(m)
        }
        _ => json!("$func"),
    }
}

fn typed_json(v: &OutVal) -> J {
    match v {
        OutVal::Num(n) => number(*n),
        OutVal::Bool(b) => J::Bool(*b),
        OutVal::Str(x) => J::String(x.to_string()),
        OutVal::Table(t) => J::Object(t.iter().map(|(k, v)| (k.clone(), typed_json(v))).collect()),
        OutVal::Gem(n) => json!({"$gem": &**n}),
        OutVal::Item(n) => json!({"$item": &**n}),
        OutVal::List(l) => J::Object(
            l.iter()
                .enumerate()
                .map(|(i, v)| ((i + 1).to_string(), typed_json(v)))
                .collect(),
        ),
    }
}

fn compare(
    what: &str,
    got: &HashMap<engine::mods::Sym, OutVal>,
    want: &Value,
    fail: &mut Vec<String>,
) {
    let got: BTreeMap<String, J> = got
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), typed_json(v)))
        .collect();
    let mut want: BTreeMap<String, J> = entries(want)
        .into_iter()
        .map(|(k, v)| {
            let k = lua::to_string(&k.to_value()).to_str_lossy().into_owned();
            let j = lua_json(&v, &k);
            (k, j)
        })
        .collect();
    want.remove("Minion");
    let mut n = 0;
    for k in got.keys().chain(want.keys()).collect::<BTreeSet<_>>() {
        if got.get(k) != want.get(k) {
            n += 1;
            if n <= 8 {
                let f = |v: Option<&J>| {
                    let s = v.map_or("missing".into(), |v| v.to_string());
                    s.chars().take(160).collect::<String>()
                };
                fail.push(format!(
                    "{what} {k}: {} != {}",
                    f(got.get(k)),
                    f(want.get(k))
                ));
            }
        }
    }
    if n > 8 {
        fail.push(format!("{what}: {} more", n - 8));
    }
}

/// `{ [k] = true }` of the values
fn set_of(vals: &[Value]) -> Value {
    let t = TableRef::new();
    for v in vals {
        t.set(v.clone(), Value::Bool(true));
    }
    Value::Table(t)
}

#[test]
fn calculators_match_upstream() {
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
    'build: for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let mut fresh: Option<Rc<Host>> = None;
        for attempt in 0..2 {
            let h = fresh.as_ref().unwrap_or(&host);
            let xml = std::fs::read_to_string(&path).unwrap();
            h.load_build_xml(xml.as_bytes(), &id)
                .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
            let build = h.build().unwrap();
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
            let mut typed_build = match engine::build::Build::load(&root, &load_ctx, tree, &parser)
            {
                Ok(b) => b,
                Err(e) => {
                    failures.push(format!("{id}: build: {e}"));
                    continue 'build;
                }
            };
            let calcs = engine::calcs::Calcs::new(&calc_data);
            // The build as loaded: after its first `MAIN` calculation
            match calcs.build_output_main(&mut typed_build) {
                Ok(_) => {}
                Err(e) if e.contains("not ported") => {
                    skipped += 1;
                    continue 'build;
                }
                Err(e) => {
                    failures.push(format!("{id}: main: {e}"));
                    continue 'build;
                }
            }
            let lcalcs = get(&get(&build, "calcsTab"), "calcs");
            let r = rt::call(
                &get(&lcalcs, "getMiscCalculator"),
                std::slice::from_ref(&build),
            )
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
            let (calc_func, calc_base) = (r[0].clone(), r[1].clone());
            let (base, used) = match calcs.misc_calculator_base(&typed_build) {
                Ok(b) => b,
                Err(e) if e.contains("not ported") => {
                    skipped += 1;
                    continue 'build;
                }
                Err(e) => {
                    failures.push(format!("{id}: base: {e}"));
                    continue 'build;
                }
            };
            if attempt == 0 {
                compared += 1;
            }
            let mut d = Vec::new();
            compare("base", &base, &calc_base, &mut d);
            // The overrides: a few nodes next to the tree added, a few
            // allocated ones removed, items taken out, a flask switched
            let spec = get(&build, "spec");
            let mut add = Vec::new();
            let mut remove = Vec::new();
            let mut nodes: Vec<(i64, Value)> = entries(&get(&spec, "nodes"))
                .into_iter()
                .filter_map(|(k, v)| match k {
                    Key::Int(i) => Some((i, v)),
                    _ => None,
                })
                .collect();
            nodes.sort_by_key(|(i, _)| *i);
            for (nid, node) in &nodes {
                let typ = lua::to_string(&get(node, "type"))
                    .to_str_lossy()
                    .into_owned();
                if !matches!(typ.as_str(), "Normal" | "Notable" | "Keystone")
                    || !get(node, "ascendancyName").is_nil()
                {
                    continue 'build;
                }
                let alloc = get(node, "alloc").truthy();
                let linked_alloc = entries(&get(node, "linked"))
                    .iter()
                    .any(|(_, l)| get(l, "alloc").truthy());
                if !alloc && linked_alloc && add.len() < 3 {
                    add.push((*nid, node.clone()));
                } else if alloc && remove.len() < 2 {
                    remove.push((*nid, node.clone()));
                }
            }
            let index = |nid: i64| typed_build.spec.index.get(&(nid as u32)).copied();
            let mut cases: Vec<(String, Value, Override)> = Vec::new();
            for (nid, node) in &add {
                let t = TableRef::new();
                t.set(s("addNodes"), set_of(std::slice::from_ref(node)));
                cases.push((
                    format!("add {nid}"),
                    Value::Table(t),
                    Override {
                        add_nodes: index(*nid).into_iter().collect(),
                        ..Override::default()
                    },
                ));
            }
            for (nid, node) in &remove {
                let t = TableRef::new();
                t.set(s("removeNodes"), set_of(std::slice::from_ref(node)));
                cases.push((
                    format!("remove {nid}"),
                    Value::Table(t),
                    Override {
                        remove_nodes: index(*nid).into_iter().collect(),
                        ..Override::default()
                    },
                ));
            }
            for slot in ["Body Armour", "Weapon 1", "Amulet"] {
                let t = TableRef::new();
                t.set(s("repSlotName"), s(slot));
                cases.push((
                    format!("without {slot}"),
                    Value::Table(t),
                    Override {
                        rep_item: Some((slot.to_string(), None)),
                        ..Override::default()
                    },
                ));
            }
            let items = get(&get(&build, "itemsTab"), "items");
            if let Some((fid, flask)) = entries(&items)
                .into_iter()
                .filter(|(_, it)| lua::to_string(&get(it, "type")).to_str_lossy() == "Flask")
                .filter_map(|(k, it)| match k {
                    Key::Int(i) => Some((i, it)),
                    _ => None,
                })
                .min_by_key(|(i, _)| *i)
            {
                let t = TableRef::new();
                t.set(s("toggleFlask"), flask);
                cases.push((
                    format!("flask {fid}"),
                    Value::Table(t),
                    Override {
                        toggle_flask: Some(fid),
                        ..Override::default()
                    },
                ));
            }
            for (what, lua_ov, ov) in cases {
                let want = match rt::call(&calc_func, &[lua_ov]) {
                    Ok(r) => r[0].clone(),
                    Err(e) => {
                        d.push(format!("{what}: translation failed: {}", describe(&e)));
                        continue 'build;
                    }
                };
                match calcs.misc_calculate(&typed_build, ov, None, used, false) {
                    Ok(got) => compare(&what, &got, &want, &mut d),
                    Err(e) if e.contains("not ported") => {}
                    Err(e) => d.push(format!("{what}: {e}")),
                }
            }
            if d.is_empty() {
                break;
            }
            if attempt == 0 {
                // The translated program keeps state across the builds it
                // loads; a build that differs runs again in a fresh one
                let f = Host::new(Rc::new(Imported::new(upstream_src())));
                f.start()
                    .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
                fresh = Some(f);
                continue;
            }
            failures.push(format!("{id} (got != want):\n  {}", d.join("\n  ")));
        }
    }
    eprintln!("{compared} builds compared, {skipped} skipped");
    assert!(
        failures.is_empty(),
        "{} of {compared} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
