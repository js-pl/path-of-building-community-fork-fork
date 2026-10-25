//! The typed calculation setup (`engine::calc_setup`) builds the mod
//! databases the translated `calcs.initEnv` builds, for every corpus build:
//! so far the base mods of the player and the enemy, the bandit, pantheon
//! and configuration mods, and the base conditions and multipliers. The
//! translated databases hold more (tree, items, skills); the comparison
//! keeps to the mods of the sources the typed setup builds.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::calc_setup::{self, BaseInput, BuffMode, SetupData};
use engine::config::{Inputs, Val};
use engine::mods::convert::Converter;
use engine::mods::{Mod, Mods, Store, Sym};
use equivalence::{corpus, mod_parser, typed_json, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
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

fn inputs(v: &Value) -> Inputs {
    let mut out = Inputs::new();
    for (k, x) in entries(v) {
        let Key::Str(k) = k else { continue };
        let val = match x {
            Value::Bool(b) => Val::Bool(b),
            Value::Number(n) => Val::Number(n),
            Value::Str(s) => Val::Str(s.to_str_lossy().into()),
            _ => continue,
        };
        out.insert(k.to_str_lossy().into_owned(), val);
    }
    out
}

fn list_mods(v: &Value) -> Vec<Mod> {
    let conv = Converter::default();
    v.as_table()
        .map(|t| t.array())
        .unwrap_or_default()
        .iter()
        .map(|m| conv.mod_(m).unwrap())
        .collect()
}

/// The mods of a ModDB by name
fn db_mods(db: &Value) -> BTreeMap<String, Vec<Mod>> {
    let conv = Converter::default();
    entries(&get(db, "mods"))
        .into_iter()
        .map(|(k, list)| {
            let Key::Str(name) = k else {
                panic!("mod name")
            };
            let mods = list
                .as_table()
                .map(|t| t.array())
                .unwrap_or_default()
                .iter()
                .map(|m| conv.mod_(m).unwrap_or_else(|e| panic!("{e}")))
                .collect();
            (name.to_str_lossy().into_owned(), mods)
        })
        .collect()
}

fn typed_mods(store: &Store) -> BTreeMap<String, Vec<Mod>> {
    match &store.mods {
        Mods::Db(by_name) => by_name
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.clone()))
            .collect(),
        Mods::List(_) => panic!("a list"),
    }
}

/// Compares the typed database with the translated one's mods from the
/// typed one's sources
fn compare(what: &str, t: &Store, l: &Value, fail: &mut dyn FnMut(String)) {
    let tm = typed_mods(t);
    let lm = db_mods(l);
    let sources: BTreeSet<Option<String>> = tm
        .values()
        .flatten()
        .map(|m| m.source.as_deref().map(String::from))
        .collect();
    let names: BTreeSet<&String> = tm.keys().chain(lm.keys()).collect();
    for name in names {
        let tl = tm.get(name).cloned().unwrap_or_default();
        let ll: Vec<Mod> = lm
            .get(name)
            .into_iter()
            .flatten()
            // Mods without a source count only where the typed setup has
            // the name, as later parts add some (allocation counts)
            .filter(|m| {
                sources.contains(&m.source.as_deref().map(String::from))
                    && (m.source.is_some() || tm.contains_key(name))
            })
            .cloned()
            .collect();
        if tl != ll {
            let show = |m: &[Mod]| {
                m.iter()
                    .map(|m| typed_json::mod_(m).to_string())
                    .collect::<Vec<_>>()
                    .join("\n      ")
            };
            fail(format!(
                "{what} {name}:\n    typed\n      {}\n    translated\n      {}",
                show(&tl),
                show(&ll)
            ));
        }
    }
    let conditions: HashMap<Sym, bool> = entries(&get(l, "conditions"))
        .into_iter()
        .filter_map(|(k, v)| match k {
            Key::Str(s) => Some((Sym::new(&s.to_str_lossy()), v.truthy())),
            _ => None,
        })
        .collect();
    for (k, v) in &t.conditions {
        if conditions.get(k) != Some(v) {
            fail(format!(
                "{what} condition {}: {v} != {:?}",
                k.as_str(),
                conditions.get(k)
            ));
        }
    }
    let multipliers: HashMap<Sym, f64> = entries(&get(l, "multipliers"))
        .into_iter()
        .filter_map(|(k, v)| match k {
            Key::Str(s) => Some((Sym::new(&s.to_str_lossy()), v.as_number()?)),
            _ => None,
        })
        .collect();
    for (k, v) in &t.multipliers {
        if multipliers.get(k) != Some(v) {
            fail(format!(
                "{what} multiplier {}: {v} != {:?}",
                k.as_str(),
                multipliers.get(k)
            ));
        }
    }
}

#[test]
fn base_dbs_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let data = SetupData::from_lua(
        &host.global("data"),
        &host.global("ModFlag"),
        &host.global("KeywordFlag"),
    )
    .unwrap_or_else(|e| panic!("setup data: {e}"));
    let mut failures = Vec::new();
    let mut compared = 0;
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let calcs = get(&get(&build, "calcsTab"), "calcs");
        let env = rt::call(&get(&calcs, "initEnv"), &[build.clone(), s("MAIN")])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)))
            .into_iter()
            .next()
            .unwrap();
        let spec = get(&env, "spec");
        let tree = get(&spec, "tree");
        let class_id = get(&env, "classId");
        let mut class = rt::index(&get(&tree, "characterData"), &class_id).unwrap_or_default();
        if class.is_nil() {
            class = rt::index(&get(&tree, "classes"), &class_id).unwrap_or_default();
        }
        let stat = |k: &str| get(&class, k).as_number().unwrap_or(f64::NAN);
        let config = get(&build, "configTab");
        let input = inputs(&get(&env, "configInput"));
        let config_mods = list_mods(&get(&config, "modList"));
        let config_enemy_mods = list_mods(&get(&config, "enemyModList"));
        let b = BaseInput {
            class_stats: [stat("base_str"), stat("base_dex"), stat("base_int")],
            character_level: get(&build, "characterLevel").as_number().unwrap(),
            enemy_level: get(&env, "enemyLevel").as_number().unwrap(),
            config_input: &input,
            config_mods: &config_mods,
            config_enemy_mods: &config_enemy_mods,
            mode: BuffMode::new(Some("EFFECTIVE")),
        };
        compared += 1;
        let mut fail = |what: String| {
            if failures.len() < 40 {
                failures.push(format!("{id}: {what}"));
            }
        };
        match calc_setup::base_dbs(&data, &parser, &b) {
            Ok((player, enemy)) => {
                compare("player", &player, &get(&env, "modDB"), &mut fail);
                compare("enemy", &enemy, &get(&env, "enemyDB"), &mut fail);
            }
            Err(e) => fail(format!("typed failed: {e}")),
        }
    }
    eprintln!("{compared} setups compared");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
