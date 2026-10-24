//! Layer 2 of docs/design.md: every mod line upstream knows parses to the
//! same mod list as upstream's `modLib.parseMod`.
//!
//! Lines are parsed in the oracle's order, from a fresh parser, as the oracle
//! does: parseMod modifies some of its tables while parsing, so results can
//! depend on earlier lines.

use equivalence::{mod_parser, oracle_dir, to_json};
use lua::Value;

#[test]
fn every_line_matches_upstream() {
    let parser = mod_parser();
    let text = std::fs::read_to_string(oracle_dir().join("modparser.jsonl"))
        .expect("tests/oracle/modparser.jsonl");
    let mut total = 0;
    let mut failures = Vec::new();
    for oracle_line in text.lines() {
        total += 1;
        let record: serde_json::Value = serde_json::from_str(oracle_line).expect("oracle JSON");
        let line = record["line"].as_str().expect("line");
        let (mods, extra) = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parser.parse(line.as_bytes())
        })) {
            Ok(r) => r,
            Err(e) => {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
                failures.push(format!("{line:?}: panicked: {}", msg.unwrap_or_default()));
                continue;
            }
        };
        let or_false = |v: Value| if v.is_nil() { Value::Bool(false) } else { v };
        let t = lua::TableRef::new();
        t.set_str("line", Value::str(line));
        t.set_str("mods", or_false(mods));
        t.set_str("extra", or_false(extra));
        let got = to_json(&Value::Table(t));
        if got != oracle_line {
            failures.push(format!("expected {oracle_line}\n     got {got}"));
        }
    }
    let shown: Vec<&String> = failures.iter().take(30).collect();
    assert!(
        failures.is_empty(),
        "{} of {total} lines differ from upstream:\n{}",
        failures.len(),
        shown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
