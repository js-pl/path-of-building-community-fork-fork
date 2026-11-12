//! Times the mod parser: making its typed data, reading it, parsing every
//! oracle line from a fresh parser and again from its cache. With an
//! output path, writes every line's typed result, to compare parsers.
//!
//!   cargo run --release -p equivalence --example parser_bench [-- <out>]

use engine::mod_parser::ModParser;
use equivalence::{oracle_dir, parser_data};
use std::fmt::Write;
use std::time::Instant;

fn main() {
    let out = std::env::args().nth(1);
    let text = std::fs::read_to_string(oracle_dir().join("modparser.jsonl")).unwrap();
    let lines: Vec<String> = text
        .lines()
        .map(|l| {
            let r: serde_json::Value = serde_json::from_str(l).unwrap();
            r["line"].as_str().unwrap().to_string()
        })
        .collect();

    let t = Instant::now();
    let data = parser_data();
    let made = t.elapsed();
    let bytes = data.encode();
    let packed = miniz_oxide::deflate::compress_to_vec(&bytes, 6);
    let t = Instant::now();
    let parser = ModParser::decode(&bytes).unwrap();
    let loaded = t.elapsed();
    println!(
        "made at build time in {made:?}; {} bytes ({} compressed), {} cached lines; read in {loaded:?}",
        bytes.len(),
        packed.len(),
        data.cache.len()
    );

    let mut fresh_data = data.clone();
    fresh_data.cache.clear();
    let fresh = ModParser::new(fresh_data);
    let mut dump = String::new();
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let t = Instant::now();
    for line in &lines {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fresh.parse(line.as_bytes())
        }));
        if out.is_some() {
            match r {
                Err(_) => writeln!(dump, "{line:?} panic").unwrap(),
                Ok((m, e)) => {
                    let m = m.map(Ok::<_, String>);
                    writeln!(dump, "{line:?} {m:?} {e:?}").unwrap();
                }
            }
        }
    }
    let first = t.elapsed();
    let t = Instant::now();
    for line in &lines {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fresh.parse(line.as_bytes())
        }));
    }
    let cached = t.elapsed();
    std::panic::set_hook(quiet);
    let n = lines.len() as f64;
    println!(
        "{} lines: first parse {first:?} ({:.0} lines/s), cached {cached:?} ({:.0} lines/s)",
        lines.len(),
        n / first.as_secs_f64(),
        n / cached.as_secs_f64()
    );
    if let Some(out) = out {
        let mut keys: Vec<&String> = data.cache.keys().collect();
        keys.sort();
        for line in keys {
            let (m, e) = parser.parse(line.as_bytes());
            let m = m.map(Ok::<_, String>);
            writeln!(dump, "cache {line:?} {m:?} {e:?}").unwrap();
        }
        std::fs::write(out, dump).unwrap();
    }
}
