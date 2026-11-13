//! Actor outputs of the translation and of the typed engine as JSON, to
//! compare them key for key.

use engine::actor::OutVal;
use lua::{Key, Value, rt};
use serde_json::{Map, Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn number(n: f64) -> J {
    if n.is_nan() {
        json!({"$num": "nan"})
    } else if n.is_infinite() {
        json!({"$num": if n > 0.0 { "inf" } else { "-inf" }})
    } else {
        serde_json::Number::from_f64(n).map_or(J::Null, J::Number)
    }
}

/// A list as Lua's: `{}` when empty
pub fn list(v: Vec<J>) -> J {
    if v.is_empty() {
        json!({})
    } else {
        json!({ "#": v })
    }
}

pub fn entries(v: &Value) -> Vec<(Key, Value)> {
    v.as_table().map(|t| t.entries()).unwrap_or_default()
}

pub fn key_str(k: &Key) -> String {
    lua::to_string(&k.to_value()).to_str_lossy().into_owned()
}

fn name_of(v: &Value, k: &str) -> String {
    let x = rt::index(v, &Value::str(k)).unwrap_or_default();
    lua::to_string(&x).to_str_lossy().into_owned()
}

/// An output of the translation as JSON: arrays under `#`, gems and items
/// by name, functions as `$func`
pub fn lua_out(v: &Value, key: &str) -> J {
    match v {
        Value::Nil => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(n) => number(*n),
        Value::Str(x) => J::String(x.to_str_lossy().into_owned()),
        Value::Table(_) if key == "sourceGem" => json!({"$gem": name_of(v, "nameSpec")}),
        Value::Table(_) if key == "sourceItem" => json!({"$item": name_of(v, "name")}),
        Value::Table(_) => {
            let mut m = Map::new();
            let mut ints: BTreeMap<i64, J> = BTreeMap::new();
            for (k, x) in entries(v) {
                match k {
                    Key::Int(i) => {
                        ints.insert(i, lua_out(&x, ""));
                    }
                    k => {
                        let k = key_str(&k);
                        let j = lua_out(&x, &k);
                        m.insert(k, j);
                    }
                }
            }
            if !ints.is_empty() {
                m.insert("#".into(), J::Array(ints.into_values().collect()));
            }
            J::Object(m)
        }
        _ => json!("$func"),
    }
}

/// A typed output as [`lua_out`] encodes the translation's
pub fn typed_out(v: &OutVal) -> J {
    match v {
        OutVal::Num(n) => number(*n),
        OutVal::Bool(b) => J::Bool(*b),
        OutVal::Str(x) => json!(&**x),
        OutVal::Gem(n) => json!({"$gem": &**n}),
        OutVal::Item(n) => json!({"$item": &**n}),
        OutVal::List(l) => list(l.iter().map(typed_out).collect()),
        OutVal::Table(t) => {
            let mut m = Map::new();
            let mut ints: BTreeMap<i64, J> = BTreeMap::new();
            for (k, v) in t.iter() {
                match k.parse::<i64>() {
                    Ok(i) => {
                        ints.insert(i, typed_out(v));
                    }
                    Err(_) => {
                        m.insert(k.clone(), typed_out(v));
                    }
                }
            }
            if !ints.is_empty() {
                m.insert("#".into(), J::Array(ints.into_values().collect()));
            }
            J::Object(m)
        }
    }
}

/// A translated actor's outputs by name, without the minion's (`Minion`),
/// compared as its own
pub fn lua_outputs(out: &Value) -> BTreeMap<String, J> {
    entries(out)
        .into_iter()
        .map(|(k, v)| {
            let k = key_str(&k);
            let j = lua_out(&v, &k);
            (k, j)
        })
        .filter(|(k, _)| k != "Minion")
        .collect()
}

pub fn typed_outputs(out: &engine::calcs::Output) -> BTreeMap<String, J> {
    // `output.Minion`, a mirage's copy's minion's, as `lua_outputs`
    out.iter()
        .filter(|(k, _)| k.as_str() != "Minion")
        .map(|(k, v)| (k.as_str().to_string(), typed_out(v)))
        .collect()
}

/// The differences of two outputs, key for key
pub fn compare(
    what: &str,
    got: &BTreeMap<String, J>,
    want: &BTreeMap<String, J>,
    fail: &mut Vec<String>,
) {
    for k in got.keys().chain(want.keys()).collect::<BTreeSet<_>>() {
        let (g, w) = (got.get(k), want.get(k));
        if g != w {
            let show = |v: Option<&J>| v.map_or("missing".into(), J::to_string);
            fail.push(format!("{what} {k}:\n      {}\n   != {}", show(g), show(w)));
        }
    }
}
