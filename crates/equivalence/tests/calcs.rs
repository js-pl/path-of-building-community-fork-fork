//! Whole builds: upstream's program, translated to Rust, calculates every
//! corpus build exactly as the oracle did (`tests/oracle/<id>.json`).
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use equivalence::{oracle_dir, root, upstream_src};
use lua::rt::{self, first};
use lua::{LResult, LuaError, TableRef, Value};
use std::path::PathBuf;
use std::rc::Rc;
use upstream::host::Host;
use upstream::imported::Imported;

/// Corpus builds as `oracle/run.py` lists them: (id, path).
fn corpus() -> Vec<(String, PathBuf)> {
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

fn s(x: &str) -> Value {
    Value::str(x)
}

/// `actorOutputs` of `oracle/dump.lua`
fn actor_outputs(env: &Value) -> LResult<Value> {
    let player = TableRef::new();
    let out = rt::index(&rt::index(env, &s("player"))?, &s("output"))?;
    let minion = rt::index(env, &s("minion"))?;
    let minion_out = if minion.truthy() {
        rt::index(&minion, &s("output"))?
    } else {
        Value::Nil
    };
    if let Value::Table(t) = &out {
        let mut k = Value::Nil;
        while let Some((key, v)) = rt::next(t, &k)? {
            if !(key.raw_eq(&s("Minion")) && minion.truthy() && v.raw_eq(&minion_out)) {
                player.set(key.clone(), v);
            }
            k = key;
        }
    }
    let r = TableRef::new();
    r.set_str("player", Value::Table(player));
    r.set_str("minion", minion_out);
    Ok(Value::Table(r))
}

fn describe(e: &LuaError) -> String {
    format!(
        "{}\n  {}",
        lua::to_string(e.value()),
        e.traceback().join("\n  ")
    )
}

/// Paths where two JSON documents differ, at most `limit`.
fn diff(
    a: &serde_json::Value,
    b: &serde_json::Value,
    path: &str,
    out: &mut Vec<String>,
    limit: usize,
) {
    use serde_json::Value as J;
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

fn fmt(v: Option<&serde_json::Value>) -> String {
    match v {
        None => "missing".into(),
        Some(v) => {
            let s = v.to_string();
            if s.len() > 80 {
                format!("{}...", &s[..80])
            } else {
                s
            }
        }
    }
}

#[test]
fn calcs() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let json_chunk = upstream::up::chunk("oracle/json.lua").expect("oracle/json.lua is translated");
    let to_json = first(json_chunk(&host.env, &[]).unwrap_or_else(|e| panic!("{}", describe(&e))));
    let mut failures = Vec::new();
    let mut ran = 0;
    for (id, path) in corpus() {
        if !id.contains(&filter) {
            continue;
        }
        ran += 1;
        let run = || -> LResult<String> {
            let text = std::fs::read(&path).unwrap();
            if path.extension().is_some_and(|e| e == "json") {
                host.load_build_json(&text)?;
            } else {
                host.load_build_xml(&text, &id)?;
            }
            let calcs_tab = rt::index(&host.build()?, &s("calcsTab"))?;
            let result = TableRef::new();
            result.set_str(
                "MAIN",
                actor_outputs(&rt::index(&calcs_tab, &s("mainEnv"))?)?,
            );
            result.set_str(
                "CALCS",
                actor_outputs(&rt::index(&calcs_tab, &s("calcsEnv"))?)?,
            );
            Ok(lua::to_string(&first(rt::call(&to_json, &[Value::Table(result)])?)).to_string())
        };
        let got = match run() {
            Ok(j) => j,
            Err(e) => {
                failures.push(format!("{id}: {}", describe(&e)));
                continue;
            }
        };
        let want = std::fs::read_to_string(oracle_dir().join(format!("{id}.json"))).unwrap();
        if got.trim_end() != want.trim_end() {
            let a: serde_json::Value = serde_json::from_str(&got).unwrap();
            let b: serde_json::Value = serde_json::from_str(&want).unwrap();
            let mut d = Vec::new();
            diff(&a, &b, "", &mut d, 15);
            failures.push(format!(
                "{id}: differs from the oracle (got != want):\n  {}",
                d.join("\n  ")
            ));
        }
    }
    assert!(ran > 0, "no build matches CALCS_FILTER={filter}");
    assert!(
        failures.is_empty(),
        "{} of {ran} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
