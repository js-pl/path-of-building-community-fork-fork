//! Upstream's `Data/ModCache.lua` holds parse results written with 14
//! significant digits. Every entry must equal the engine's live parse of the
//! line, rounded the same way, so preloading it changes nothing but the
//! rounding upstream has too.

use engine::mod_parser::Parsed;
use equivalence::{mod_parser, parser_data, typed_json};
use serde_json::Value as J;

/// Rounds every number like `tostring` (`%.14g`), as writing and loading
/// the cache does.
fn round14(v: &J) -> J {
    match v {
        J::Number(x) => {
            let x = x.as_f64().expect("number");
            let r: f64 = lua::format::format_g(x, 14).parse().expect("number");
            serde_json::Number::from_f64(r).map_or(J::Null, J::Number)
        }
        J::Array(a) => J::Array(a.iter().map(round14).collect()),
        J::Object(o) => J::Object(o.iter().map(|(k, x)| (k.clone(), round14(x))).collect()),
        v => v.clone(),
    }
}

fn json((mods, extra): &Parsed) -> J {
    let mods = match mods {
        Some(m) => J::Array(m.iter().map(typed_json::mod_).collect()),
        None => J::Null,
    };
    J::Array(vec![mods, extra.clone().map_or(J::Null, J::String)])
}

#[test]
fn mod_cache_matches_live_parse() {
    let parser = mod_parser();
    let cache = parser_data().cache;
    let mut lines: Vec<&String> = cache.keys().collect();
    lines.sort();
    let mut failures = Vec::new();
    for line in &lines {
        let live = round14(&json(&parser.parse(line.as_bytes())));
        let cached = json(&cache[*line]);
        if live != cached {
            failures.push(format!("{line}\n  cache {cached}\n  live  {live}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cache entries differ:\n{}",
        failures.len(),
        lines.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}
