//! Lua values from the oracle's `oracleValue` answers (`oracle/values.lua`'s
//! encoding), so the tests that compare typed structures with upstream's
//! tables read upstream on LuaJIT the way they read Lua values: tables keep
//! their sharing and cycles, functions become [`lua::value::FuncRef`]s
//! named `<file>:<line>`.

use crate::oracle::{J, parse};
use lua::Value;
use lua::value::{FuncRef, TableRef};
use std::collections::HashMap;
use std::rc::Rc;

/// The value of an `{"ok": ...}` answer, or the error of an
/// `{"error": ...}` one
pub fn answer(res: &str) -> Result<Value, String> {
    let J::Obj(mut o) = parse(res.as_bytes(), &mut 0) else {
        return Err(format!("oracle answer {res:.200}"));
    };
    if let Some(e) = o.remove("error") {
        return Err(match e {
            J::Str(e) => e,
            e => e.to_string(),
        });
    }
    decode(&o.remove("ok").ok_or("oracle answer without ok")?)
}

/// Reads one encoded value
pub fn decode(j: &J) -> Result<Value, String> {
    let mut tables = HashMap::new();
    value(j, &mut tables)
}

fn value(j: &J, tables: &mut HashMap<u64, TableRef>) -> Result<Value, String> {
    let id = |n: &J| match n {
        J::Num(n) => Ok(*n as u64),
        _ => Err("table id".to_string()),
    };
    Ok(match j {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Bool(*b),
        J::Num(n) => Value::Number(*n),
        J::Str(s) => Value::str(s),
        J::Arr(_) => return Err("array outside a table".into()),
        J::Obj(o) => {
            if let Some(n) = o.get("#") {
                let t = TableRef::new();
                tables.insert(id(n)?, t.clone());
                let Some(J::Arr(kv)) = o.get("kv") else {
                    return Err("table entries".into());
                };
                for pair in kv.chunks(2) {
                    let k = value(&pair[0], tables)?;
                    let v = value(pair.get(1).ok_or("entry without a value")?, tables)?;
                    t.set(k, v);
                }
                Value::Table(t)
            } else if let Some(n) = o.get("$r") {
                Value::Table(
                    tables
                        .get(&id(n)?)
                        .ok_or("reference to an unknown table")?
                        .clone(),
                )
            } else if let Some(J::Str(h)) = o.get("$bytes") {
                let bytes = (0..h.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&h[i..i + 2], 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .map_err(|e| e.to_string())?;
                Value::Str(bytes.into())
            } else if let Some(J::Str(f)) = o.get("$f") {
                let (file, line) = f.rsplit_once(':').unwrap_or((f, "0"));
                Value::Func(Rc::new(FuncRef {
                    file: file.into(),
                    line: line.parse().unwrap_or(0),
                    offset: 0,
                    key: f.as_str().into(),
                    source_hash: 0,
                }))
            } else {
                return Err(format!("unknown value {j}"));
            }
        }
    })
}

/// `v` with its tables rebuilt as [`decode`] builds the oracle's: entries
/// inserted in `oracle/values.lua`'s order (number keys ascending, then
/// strings, booleans and tables by their `id`), sharing kept, functions
/// named `<file>:<line>`. Tables iterate in the order of their history, so
/// a value made elsewhere (the importer's data) compares with the oracle's
/// only after the same rebuild.
pub fn canonical(v: &Value) -> Value {
    let mut seen = HashMap::new();
    canon(v, &mut seen)
}

fn key_rank(k: &Value) -> (u8, f64, Vec<u8>) {
    match k {
        Value::Number(n) => (0, *n, vec![]),
        Value::Str(s) => (1, 0.0, s.as_bytes().to_vec()),
        Value::Bool(b) => (2, *b as u8 as f64, vec![]),
        Value::Table(t) => (3, 0.0, lua::to_string(&t.get_str("id")).as_bytes().to_vec()),
        _ => (4, 0.0, vec![]),
    }
}

fn canon(v: &Value, seen: &mut HashMap<*const (), TableRef>) -> Value {
    match v {
        Value::Table(t) => {
            let p = Rc::as_ptr(&t.0) as *const ();
            if let Some(c) = seen.get(&p) {
                return Value::Table(c.clone());
            }
            let c = TableRef::new();
            seen.insert(p, c.clone());
            let mut entries: Vec<(Value, Value)> = t
                .entries()
                .into_iter()
                .map(|(k, v)| (k.to_value(), v))
                .filter(|(_, v)| !v.is_nil())
                .collect();
            entries.sort_by(|(a, _), (b, _)| {
                key_rank(a)
                    .partial_cmp(&key_rank(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for (k, v) in entries {
                let k = canon(&k, seen);
                let v = canon(&v, seen);
                c.set(k, v);
            }
            Value::Table(c)
        }
        Value::Func(f) => {
            let name = format!("{}:{}", f.file, f.line);
            Value::Func(Rc::new(FuncRef {
                file: f.file.clone(),
                line: f.line,
                offset: 0,
                key: name.as_str().into(),
                source_hash: 0,
            }))
        }
        v => v.clone(),
    }
}
