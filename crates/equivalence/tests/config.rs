//! The typed configuration (`engine::config`) builds the mods the
//! translated program's Configuration tab builds, for every corpus build
//! loaded from XML: the player's and the enemy's mods, in order, and the
//! inputs and placeholders the options leave.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::config::{self, ConfigData, ConfigState, CustomBlock, Inputs, Val};
use engine::mods::Mod;
use engine::mods::convert::Converter;
use equivalence::{corpus, mod_parser, typed_json, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
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

fn inputs(v: &Value) -> Inputs {
    let mut out = Inputs::new();
    for (k, x) in v.as_table().map(|t| t.entries()).unwrap_or_default() {
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

fn mods_diff(what: &str, t: &[Mod], l: &Value) -> Option<String> {
    let conv = Converter::default();
    let lm: Vec<Mod> = match l
        .as_table()
        .map(|t| t.array())
        .unwrap_or_default()
        .iter()
        .map(|m| conv.mod_(m))
        .collect()
    {
        Ok(m) => m,
        Err(e) => return Some(format!("{what}: translated mods do not convert: {e}")),
    };
    if t == lm.as_slice() {
        return None;
    }
    let n = t.len().min(lm.len());
    let i = (0..n).find(|&i| t[i] != lm[i]).unwrap_or(n);
    let show = |m: Option<&Mod>| m.map_or("none".into(), |m| typed_json::mod_(m).to_string());
    Some(format!(
        "{what}: {} mods != {}, first difference at {}:\n    typed {}\n    translated {}",
        t.len(),
        lm.len(),
        i + 1,
        show(t.get(i)),
        show(lm.get(i))
    ))
}

fn inputs_diff(what: &str, t: &Inputs, l: &Inputs) -> Option<String> {
    let mut keys: Vec<&String> = t.keys().chain(l.keys()).collect();
    keys.sort();
    keys.dedup();
    let diffs: Vec<String> = keys
        .into_iter()
        .filter(|k| t.get(*k) != l.get(*k))
        .map(|k| format!("{k}: {:?} != {:?}", t.get(k), l.get(k)))
        .collect();
    (!diffs.is_empty()).then(|| format!("{what}: {}", diffs.join("; ")))
}

#[test]
fn config_matches_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let data = ConfigData::from_lua(&host.global("data"), &host.global("SkillType"))
        .unwrap_or_else(|e| panic!("config data: {e}"));
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
        let tab = get(&build, "configTab");
        let set = rt::index(&get(&tab, "configSets"), &get(&tab, "activeConfigSetId")).unwrap();
        let mut input = inputs(&get(&set, "input"));
        let mut placeholder = inputs(&get(&set, "placeholder"));
        let custom: Vec<CustomBlock> = get(&set, "customModsList")
            .as_table()
            .map(|t| t.array())
            .unwrap_or_default()
            .iter()
            .map(|b| CustomBlock {
                title: get(b, "title")
                    .as_str()
                    .map(|s| s.to_str_lossy().into_owned()),
                enabled: !matches!(get(b, "enabled"), Value::Bool(false)),
                text: get(b, "text")
                    .as_str()
                    .map(|s| s.to_str_lossy().into_owned())
                    .unwrap_or_default(),
            })
            .collect();
        let level = get(&build, "characterLevel")
            .as_number()
            .unwrap_or(f64::NAN);
        rt::method(&tab, &"BuildModList".into(), &[])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        compared += 1;
        let mut state = ConfigState::default();
        let mut fail = |what: String| {
            if failures.len() < 60 {
                failures.push(format!("{id}: {what}"));
            }
        };
        match config::build_mod_list(
            &data,
            &parser,
            &mut input,
            &mut placeholder,
            &custom,
            &mut state,
            level,
        ) {
            Ok((mods, enemy_mods)) => {
                if let Some(d) = mods_diff("modList", &mods, &get(&tab, "modList")) {
                    fail(d);
                }
                if let Some(d) = mods_diff("enemyModList", &enemy_mods, &get(&tab, "enemyModList"))
                {
                    fail(d);
                }
                if let Some(d) = inputs_diff("input", &input, &inputs(&get(&set, "input"))) {
                    fail(d);
                }
                if let Some(d) = inputs_diff(
                    "placeholder",
                    &placeholder,
                    &inputs(&get(&set, "placeholder")),
                ) {
                    fail(d);
                }
                let level = get(&tab, "enemyLevel").as_number();
                if level != state.enemy_level {
                    fail(format!("enemyLevel: {:?} != {level:?}", state.enemy_level));
                }
            }
            Err(e) => fail(format!("typed failed: {e}")),
        }
    }
    eprintln!("{compared} configurations compared");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Every option applied alone, to each of its list values or to a count,
/// as upstream's apply functions do it on a fresh build
#[test]
fn every_option_matches_upstream() {
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let data = ConfigData::from_lua(&host.global("data"), &host.global("SkillType"))
        .unwrap_or_else(|e| panic!("config data: {e}"));
    host.new_build().unwrap();
    let build = host.build().unwrap();
    let var_list = rt::call(&host.global("LoadModule"), &[s("Modules/ConfigOptions")])
        .unwrap_or_else(|e| panic!("{}", describe(&e)))
        .into_iter()
        .next()
        .unwrap();
    let new = host.global("new");
    let mut failures = Vec::new();
    let mut compared = 0;
    for var_data in var_list.as_table().unwrap().array() {
        let apply = get(&var_data, "apply");
        let Some(var) = get(&var_data, "var")
            .as_str()
            .map(|s| s.to_str_lossy().into_owned())
        else {
            continue;
        };
        if apply.is_nil() {
            continue;
        }
        let typ = get(&var_data, "type")
            .as_str()
            .map(|s| s.to_str_lossy().into_owned())
            .unwrap_or_default();
        let values: Vec<Value> = match typ.as_str() {
            "check" => vec![Value::Bool(true)],
            "list" => get(&var_data, "list")
                .as_table()
                .map(|t| t.array())
                .unwrap_or_default()
                .iter()
                .map(|v| get(v, "val"))
                .collect(),
            "text" => continue,
            _ => vec![Value::Number(3.0), Value::Number(12.0), Value::Number(-2.0)],
        };
        let option = config::OPTIONS
            .iter()
            .find(|o| o.var == var)
            .unwrap_or_else(|| panic!("option {var} not ported"));
        for v in values {
            let val = match &v {
                Value::Bool(b) => Val::Bool(*b),
                Value::Number(n) => Val::Number(*n),
                Value::Str(s) => Val::Str(s.to_str_lossy().into()),
                _ => continue,
            };
            let lists: Vec<Value> = (0..2)
                .map(|_| {
                    rt::call(&new, &[s("ModList")])
                        .unwrap()
                        .into_iter()
                        .next()
                        .unwrap()
                })
                .collect();
            if let Err(e) = rt::call(
                &apply,
                &[v.clone(), lists[0].clone(), lists[1].clone(), build.clone()],
            ) {
                // Options whose apply needs the configuration tab's state
                failures.push(format!(
                    "{var} = {val:?}: translated failed: {}",
                    describe(&e)
                ));
                continue;
            }
            compared += 1;
            let mut input = Inputs::new();
            let mut placeholder = Inputs::new();
            let mut state = ConfigState {
                enemy_level: Some(84.0),
                ..ConfigState::default()
            };
            let mut out = config::Out {
                mods: vec![],
                enemy_mods: vec![],
                input: &mut input,
                placeholder: &mut placeholder,
                data: &data,
                state: &mut state,
                character_level: get(&build, "characterLevel").as_number().unwrap_or(1.0),
            };
            option.apply(&val, &mut out);
            for (what, t, l) in [
                ("modList", &out.mods, &lists[0]),
                ("enemyModList", &out.enemy_mods, &lists[1]),
            ] {
                if let Some(d) = mods_diff(what, t, l) {
                    failures.push(format!("{var} = {val:?}: {d}"));
                }
            }
        }
    }
    eprintln!("{compared} option values compared");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
