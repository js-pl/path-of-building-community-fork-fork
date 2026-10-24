//! Writes `corpus/synthetic_mod_lines.txt`: one example line for every
//! ModParser rule no real mod line reaches, so the oracle and the engine
//! parse them too and every ported closure is compared with upstream.
//!
//!   cargo run -p importer --bin synthlines
//!
//! Needs the real lines, tests/oracle/modparser.jsonl, to know which rules
//! they reach; regenerate that first after a submodule bump.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Example text for a capture or class: words that upstream's lookups know.
fn class_example(class: u8, repeated: bool) -> &'static str {
    match class {
        b'd' => {
            if repeated {
                "10"
            } else {
                "1"
            }
        }
        b'a' | b'l' | b'w' => {
            if repeated {
                "fire"
            } else {
                "a"
            }
        }
        b's' => " ",
        b'p' => ".",
        b'u' => "A",
        b'x' => "a",
        _ => "",
    }
}

/// An example string the Lua pattern matches, for the pattern features
/// ModParser uses; None when the pattern has something else.
fn example(pattern: &[u8]) -> Option<String> {
    let mut out = String::new();
    let mut i = 0;
    let p = pattern;
    if p.first() == Some(&b'^') {
        i = 1;
    }
    while i < p.len() {
        let c = p[i];
        // One "single" item: its example for one occurrence and for repeats
        let (single, repeated, next): (String, String, usize) = match c {
            b'(' | b')' => {
                i += 1;
                continue;
            }
            b'$' if i + 1 == p.len() => break,
            b'%' => {
                let e = *p.get(i + 1)?;
                if e.is_ascii_alphabetic() {
                    (
                        class_example(e, false).into(),
                        class_example(e, true).into(),
                        i + 2,
                    )
                } else {
                    let ch = (e as char).to_string();
                    (ch.clone(), ch, i + 2)
                }
            }
            b'.' => ("x".into(), "fire".into(), i + 1),
            b'[' => {
                let mut j = i + 1;
                let negate = p.get(j) == Some(&b'^');
                if negate {
                    j += 1;
                }
                let start = j;
                // First char after '[' or '[^' is literal, even ']'
                j += 1;
                while j < p.len() && p[j] != b']' {
                    if p[j] == b'%' {
                        j += 1;
                    }
                    j += 1;
                }
                if j >= p.len() {
                    return None;
                }
                let set = &p[start..j];
                let ch = if negate {
                    if set.contains(&b'a') { "b" } else { "a" }.to_string()
                } else if set[0] == b'%' {
                    let e = *set.get(1)?;
                    if e.is_ascii_alphabetic() {
                        class_example(e, false).to_string()
                    } else {
                        (e as char).to_string()
                    }
                } else {
                    (set[0] as char).to_string()
                };
                let rep = if !negate && set.starts_with(b"%d") {
                    "10".to_string()
                } else {
                    ch.clone()
                };
                (ch, rep, j + 1)
            }
            _ => ((c as char).to_string(), (c as char).to_string(), i + 1),
        };
        match p.get(next) {
            Some(b'+') => {
                out.push_str(&repeated);
                i = next + 1;
            }
            Some(b'*') | Some(b'-') => {
                // Zero repetitions, except inside a capture, which upstream
                // code usually expects to be non-empty
                if p.get(next + 1) == Some(&b')') {
                    out.push_str(&repeated);
                }
                i = next + 1;
            }
            Some(b'?') => {
                out.push_str(&single);
                i = next + 1;
            }
            _ => {
                out.push_str(&single);
                i = next;
            }
        }
    }
    let pattern_matches = lua::pattern::lua_find(out.as_bytes(), pattern, 1, false).ok()??;
    // The example must match from the start, as anchored patterns do
    if pattern.first() == Some(&b'^') && pattern_matches.0 != 1 {
        return None;
    }
    Some(out)
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let src_dir = root.join("upstream/PathOfBuilding/src");
    let raw = importer::import(&src_dir).unwrap_or_else(|e| panic!("{e}"));
    let mut im = importer::Importer::new(&src_dir);
    let tables = importer::mod_parser::import(&mut im, &raw).unwrap_or_else(|e| panic!("{e}"));

    // Lines already in the oracle
    let oracle = std::fs::read_to_string(root.join("tests/oracle/modparser.jsonl"))
        .expect("tests/oracle/modparser.jsonl");
    // The oracle includes the current synthetic lines; only real ones count
    let synthetic_path = root.join("corpus/synthetic_mod_lines.txt");
    let previous: BTreeSet<String> = std::fs::read_to_string(&synthetic_path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let mut known = BTreeSet::new();
    for l in oracle.lines() {
        let rec: serde_json::Value = serde_json::from_str(l).unwrap();
        let line = rec["line"].as_str().unwrap();
        if !previous.contains(line) {
            known.insert(line.to_lowercase());
        }
    }

    let mut lines = BTreeSet::new();
    let mut skipped = 0;
    let keys = |name: &str| -> Vec<Vec<u8>> {
        tables
            .get_str(name)
            .as_table()
            .unwrap()
            .entries()
            .into_iter()
            .filter_map(|(k, _)| k.to_value().as_str().map(|s| s.as_bytes().to_vec()))
            .collect()
    };
    // Special mods are whole lines
    for k in keys("specialModList") {
        let mut anchored = b"^".to_vec();
        anchored.extend_from_slice(&k);
        anchored.push(b'$');
        match example(&anchored) {
            Some(e) => {
                lines.insert(e);
            }
            None => skipped += 1,
        }
    }
    // Tags and flags go around a plain modifier
    for k in keys("modTagList") {
        match example(&k) {
            Some(e) => {
                lines.insert(format!("10% increased damage {}", e.trim()));
            }
            None => skipped += 1,
        }
    }
    for k in keys("preFlagList") {
        match example(&k) {
            Some(e) => {
                lines.insert(format!("{} 10% increased damage", e.trim()));
            }
            None => skipped += 1,
        }
    }
    for k in keys("formList") {
        match example(&k) {
            Some(e) => {
                lines.insert(format!("{} fire damage", e.trim()));
            }
            None => skipped += 1,
        }
    }
    let new: Vec<String> = lines
        .into_iter()
        .filter(|l| !known.contains(&l.to_lowercase()))
        .collect();
    let mut text = String::from(
        "# Generated by `cargo run -p importer --bin synthlines`: example lines for ModParser\n\
         # rules that no real mod line reaches. oracle/modparser.lua parses them too.\n",
    );
    for l in &new {
        text.push_str(l);
        text.push('\n');
    }
    std::fs::write(&synthetic_path, text).unwrap();
    println!(
        "{} synthetic lines, {skipped} patterns without an example",
        new.len()
    );
}
