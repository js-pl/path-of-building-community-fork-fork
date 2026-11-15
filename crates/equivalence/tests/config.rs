//! The typed configuration (`engine::config`) builds the mods the
//! translated program's Configuration tab builds for every option and
//! value (`every_option_matches_upstream`). The corpus builds' own
//! configurations are covered against the oracle by `build_output.rs` and
//! the session test's config phase (see docs/drop-translation.md).

use engine::config::{self, ConfigState, Inputs, Val};
use engine::mods::Mod;
use engine::mods::convert::Converter;
use equivalence::{typed_json, upstream_src};
use lua::rt;
use lua::{LuaError, Value};
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

#[test]
fn every_option_matches_upstream() {
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let data = equivalence::game_data().config;
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
                    let list = rt::first(rt::call(&new, &[s("ModList")]).unwrap());
                    rt::first(rt::method(&list, &"ModList".into(), &[]).unwrap())
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
