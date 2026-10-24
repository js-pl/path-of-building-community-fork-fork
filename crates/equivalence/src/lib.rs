//! Helpers for the equivalence tests in `tests/`: paths to upstream and the
//! oracle outputs, the engine's inputs, and engine values encoded exactly as
//! `oracle/json.lua` encodes upstream's.

use engine::data::Data;
use lua::Value;
use std::path::PathBuf;

/// The repository root.
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Upstream's `src/` directory.
pub fn upstream_src() -> PathBuf {
    root().join("upstream/PathOfBuilding/src")
}

/// `tests/oracle/`
pub fn oracle_dir() -> PathBuf {
    root().join("tests/oracle")
}

/// Imports upstream's data and processes it.
pub fn load_data() -> (Data, importer::Raw) {
    let raw = importer::import(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let data = Data::new(raw.globals.clone(), raw.data.clone());
    (data, raw)
}

/// Builds the mod parser from upstream's source.
pub fn mod_parser() -> engine::mod_parser::ModParser {
    let (data, raw) = load_data();
    let mut im = importer::Importer::new(&upstream_src());
    let tables = importer::mod_parser::import(&mut im, &raw).unwrap_or_else(|e| panic!("{e}"));
    engine::mod_parser::ModParser::new(data, &tables).unwrap_or_else(|e| panic!("{e}"))
}

/// JSON as `oracle/json.lua` writes it: `%.17g` numbers, sorted keys, arrays
/// for tables with keys 1..n, `{}` for empty tables, functions as
/// `{"$func":true}`.
pub fn to_json(v: &Value) -> String {
    let mut out = String::new();
    encode(v, &mut out, &|_| "{\"$func\":true}".to_string());
    out
}

/// Like [`to_json`], with Lua functions as `{"$func":"<file>:<line>"}`, as
/// `oracle/data.lua` writes them.
pub fn to_json_with_funcs(v: &Value) -> String {
    let mut out = String::new();
    encode(v, &mut out, &|v| match v {
        Value::Func(f) => format!("{{\"$func\":\"{}:{}\"}}", f.file, f.line),
        _ => "{\"$func\":true}".to_string(),
    });
    out
}

fn encode_str(s: &[u8], out: &mut String) {
    out.push('"');
    for ch in String::from_utf8_lossy(s).chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn encode(v: &Value, out: &mut String, func: &dyn Fn(&Value) -> String) {
    match v {
        Value::Nil => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_nan() {
                out.push_str("{\"$num\":\"nan\"}")
            } else if n.is_infinite() {
                out.push_str(if *n > 0.0 {
                    "{\"$num\":\"inf\"}"
                } else {
                    "{\"$num\":\"-inf\"}"
                })
            } else {
                out.push_str(&lua::format::format_g(*n, 17))
            }
        }
        Value::Str(s) => encode_str(s.as_bytes(), out),
        Value::Func(_) | Value::Closure(_) | Value::Function(_) => out.push_str(&func(v)),
        Value::Table(t) => {
            let entries = t.entries();
            let n = entries.len();
            let is_array = n > 0 && (1..=n as i64).all(|i| !t.get_int(i).is_nil());
            if is_array {
                out.push('[');
                for i in 1..=n as i64 {
                    if i > 1 {
                        out.push(',');
                    }
                    encode(&t.get_int(i), out, func);
                }
                out.push(']');
            } else {
                let mut keyed: Vec<(Vec<u8>, Value)> = entries
                    .into_iter()
                    .map(|(k, v)| (lua::to_string(&k.to_value()).as_bytes().to_vec(), v))
                    .collect();
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                out.push('{');
                for (i, (k, v)) in keyed.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    encode_str(k, out);
                    out.push(':');
                    encode(v, out, func);
                }
                out.push('}');
            }
        }
    }
}
