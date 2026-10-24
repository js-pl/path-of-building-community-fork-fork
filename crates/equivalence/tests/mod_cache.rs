//! Upstream's `Data/ModCache.lua` holds parse results written with 14
//! significant digits. Every entry must equal the engine's live parse of the
//! line, rounded the same way, so preloading it changes nothing but the
//! rounding upstream has too.

use equivalence::{mod_parser, to_json, upstream_src};
use lua::{TableRef, Value};

/// Rounds every number like `tostring` (`%.14g`) and parses it back, as
/// writing and loading the cache does.
fn round14(v: &Value) -> Value {
    match v {
        Value::Number(x) if x.is_finite() => {
            Value::Number(lua::format::format_g(*x, 14).parse().expect("number"))
        }
        Value::Table(t) => {
            let out = TableRef::new();
            for (k, v) in t.entries() {
                out.borrow_mut().set_key(k, round14(&v));
            }
            Value::Table(out)
        }
        v => v.clone(),
    }
}

#[test]
fn mod_cache_matches_live_parse() {
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    let mut entries = cache.entries();
    entries.sort_by(|a, b| {
        lua::to_string(&a.0.to_value())
            .as_bytes()
            .cmp(lua::to_string(&b.0.to_value()).as_bytes())
    });
    let mut failures = Vec::new();
    for (line, entry) in &entries {
        let line = line.to_value();
        let (mods, extra) = parser.parse(line.as_str().unwrap().as_bytes());
        let live = to_json(&round14(&lua::list![mods.clone(), extra.clone()].into()));
        let cached = to_json(
            &lua::list![
                entry.as_table().unwrap().get_int(1),
                entry.as_table().unwrap().get_int(2)
            ]
            .into(),
        );
        // list![] drops trailing nils; compare both the same way
        if live != cached {
            failures.push(format!(
                "{}\n  cache {cached}\n  live  {live}",
                line.as_str().unwrap()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cache entries differ:\n{}",
        failures.len(),
        entries.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}
