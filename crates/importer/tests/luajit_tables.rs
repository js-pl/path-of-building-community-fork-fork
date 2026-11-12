//! Tables built by constructors, stores and the table library visit their
//! number and boolean keys in LuaJIT's `pairs` order and have its `#t`:
//! the cases `oracle/tables.lua` ran on LuaJIT (`tests/oracle/tables.jsonl`).

use importer::eval::Env;
use lua::{Value, rt};

fn key(s: &str) -> Value {
    match s {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        n => Value::Number(n.parse().expect("number key")),
    }
}

fn fmt(v: &Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => lua::format::format_g(*n, 17),
        v => panic!("unexpected key {v:?}"),
    }
}

#[test]
fn pairs_order_matches_luajit() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/oracle/tables.jsonl"
    );
    let cases = std::fs::read_to_string(path).expect("tests/oracle/tables.jsonl");
    let lib = lua::env::Env::new();
    let table = lib.global("table");
    let f = |name: &str| table.as_table().unwrap().get_str(name);
    let (insert, remove) = (f("insert"), f("remove"));
    let mut failed = Vec::new();
    for (i, line) in cases.lines().enumerate() {
        let case: serde_json::Value = serde_json::from_str(line).unwrap();
        let ctor = case["ctor"].as_str().unwrap();
        let args = vec![Value::Number(1.0); case["args"].as_u64().unwrap() as usize];
        let t = Env::new()
            .eval_file("tables.lua", ctor.as_bytes(), args)
            .unwrap();
        let t = t[0].as_table().unwrap().clone();
        let tv = Value::Table(t.clone());
        let ops = case["ops"].as_str().unwrap();
        for op in ops.split(';').filter(|o| !o.is_empty()) {
            let one = Value::Number(1.0);
            let (o, arg) = op.split_at(1);
            match o {
                "s" => {
                    let (k, v) = arg.rsplit_once('=').unwrap();
                    let v = if v == "nil" { Value::Nil } else { one };
                    rt::set_index(&tv, key(k), v).unwrap();
                }
                "a" => drop(rt::call(&insert, &[tv.clone(), one]).unwrap()),
                "i" => drop(rt::call(&insert, &[tv.clone(), key(arg), one]).unwrap()),
                "r" => drop(rt::call(&remove, std::slice::from_ref(&tv)).unwrap()),
                "R" => drop(rt::call(&remove, &[tv.clone(), key(arg)]).unwrap()),
                o => panic!("unknown op {o}"),
            }
        }
        let keys: Vec<String> = t.borrow().iter().map(|(k, _)| fmt(&k.to_value())).collect();
        let len = t.borrow().border() as u64;
        if keys.join(",") != case["keys"].as_str().unwrap() || len != case["len"].as_u64().unwrap()
        {
            failed.push(format!(
                "case {}: {ctor} | {ops}\n  keys {} len {len}\n  want {} len {}",
                i + 1,
                keys.join(","),
                case["keys"].as_str().unwrap(),
                case["len"]
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} cases differ:\n{}",
        failed.len(),
        cases.lines().count(),
        failed[..failed.len().min(5)].join("\n")
    );
}
