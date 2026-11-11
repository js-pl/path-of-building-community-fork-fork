//! The typed engine alone (`engine::calcs::build_output`) gives every
//! corpus build the oracle's own outputs (`tests/oracle/<id>.json`,
//! `MAIN`): the player's output with the Full DPS table and the main
//! skill's minion's. Numbers compare bitwise; a missing or extra key fails.
//! Builds that reach a part not ported yet are skipped and listed. The game
//! data still comes from a started upstream host.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::actor::OutVal;
use equivalence::typed_build::TypedData;
use equivalence::{corpus, oracle_dir, upstream_src};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use upstream::host::Host;
use upstream::imported::Imported;

/// A JSON value with numbers parsed exactly
#[derive(Clone, Debug)]
enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(BTreeMap<String, J>),
}

impl PartialEq for J {
    fn eq(&self, other: &J) -> bool {
        match (self, other) {
            (J::Null, J::Null) => true,
            (J::Bool(a), J::Bool(b)) => a == b,
            (J::Num(a), J::Num(b)) => a.to_bits() == b.to_bits(),
            (J::Str(a), J::Str(b)) => a == b,
            (J::Arr(a), J::Arr(b)) => a == b,
            (J::Obj(a), J::Obj(b)) => a == b,
            _ => false,
        }
    }
}

impl std::fmt::Display for J {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            J::Null => write!(f, "null"),
            J::Bool(b) => write!(f, "{b}"),
            J::Num(n) => write!(f, "{}", lua::format::format_g(*n, 17)),
            J::Str(s) => write!(f, "{s:?}"),
            J::Arr(a) => {
                let v: Vec<String> = a.iter().map(J::to_string).collect();
                write!(f, "[{}]", v.join(","))
            }
            J::Obj(o) => {
                let v: Vec<String> = o.iter().map(|(k, v)| format!("{k:?}:{v}")).collect();
                write!(f, "{{{}}}", v.join(","))
            }
        }
    }
}

/// Parses the JSON `oracle/json.lua` writes
fn parse(s: &[u8], i: &mut usize) -> J {
    let ws = |i: &mut usize| {
        while s[*i].is_ascii_whitespace() {
            *i += 1
        }
    };
    ws(i);
    let v = match s[*i] {
        b'{' => {
            *i += 1;
            let mut o = BTreeMap::new();
            ws(i);
            while s[*i] != b'}' {
                let J::Str(k) = parse(s, i) else {
                    panic!("a key at {i}")
                };
                ws(i);
                *i += 1; // ':'
                o.insert(k, parse(s, i));
                ws(i);
                if s[*i] == b',' {
                    *i += 1;
                    ws(i);
                }
            }
            *i += 1;
            J::Obj(o)
        }
        b'[' => {
            *i += 1;
            let mut a = Vec::new();
            ws(i);
            while s[*i] != b']' {
                a.push(parse(s, i));
                ws(i);
                if s[*i] == b',' {
                    *i += 1;
                }
            }
            *i += 1;
            J::Arr(a)
        }
        b'"' => {
            *i += 1;
            let mut out = Vec::new();
            while s[*i] != b'"' {
                if s[*i] == b'\\' {
                    *i += 1;
                    match s[*i] {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let hex = std::str::from_utf8(&s[*i + 1..*i + 5]).unwrap();
                            let c = char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap();
                            out.extend(c.to_string().bytes());
                            *i += 4;
                        }
                        c => out.push(c),
                    }
                } else {
                    out.push(s[*i]);
                }
                *i += 1;
            }
            *i += 1;
            J::Str(String::from_utf8_lossy(&out).into_owned())
        }
        b't' => {
            *i += 4;
            J::Bool(true)
        }
        b'f' => {
            *i += 5;
            J::Bool(false)
        }
        b'n' => {
            *i += 4;
            J::Null
        }
        _ => {
            let start = *i;
            while matches!(s[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                *i += 1;
            }
            let t = std::str::from_utf8(&s[start..*i]).unwrap();
            J::Num(t.parse().unwrap_or_else(|_| panic!("a number: {t}")))
        }
    };
    // Non-finite numbers
    if let J::Obj(o) = &v
        && let (1, Some(J::Str(n))) = (o.len(), o.get("$num"))
    {
        return J::Num(match n.as_str() {
            "inf" => f64::INFINITY,
            "-inf" => f64::NEG_INFINITY,
            _ => f64::NAN,
        });
    }
    v
}

fn out_json(v: &OutVal) -> J {
    match v {
        OutVal::Num(n) => J::Num(*n),
        OutVal::Bool(b) => J::Bool(*b),
        OutVal::Str(s) => J::Str(s.to_string()),
        OutVal::Table(t) => J::Obj(t.iter().map(|(k, v)| (k.clone(), out_json(v))).collect()),
        // `{}` for an empty table
        OutVal::List(l) if l.is_empty() => J::Obj(BTreeMap::new()),
        OutVal::List(l) => J::Arr(l.iter().map(out_json).collect()),
        OutVal::Gem(n) => J::Obj([("$gem".into(), J::Str(n.to_string()))].into()),
        OutVal::Item(n) => J::Obj([("$item".into(), J::Str(n.to_string()))].into()),
    }
}

fn output_json(out: &engine::calcs::Output) -> J {
    J::Obj(
        out.iter()
            .map(|(k, v)| (k.as_str().to_string(), out_json(v)))
            .collect(),
    )
}

/// Paths where two values differ
fn diff(got: &J, want: &J, path: &str, out: &mut Vec<String>) {
    match (got, want) {
        (J::Obj(a), J::Obj(b)) => {
            for k in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
                let p = format!("{path}.{k}");
                match (a.get(k), b.get(k)) {
                    (Some(x), Some(y)) => diff(x, y, &p, out),
                    (x, y) => out.push(format!(
                        "{p}: {} != {}",
                        x.map_or("missing".into(), J::to_string),
                        y.map_or("missing".into(), J::to_string)
                    )),
                }
            }
        }
        _ if got != want => out.push(format!("{path}: {got} != {want}")),
        _ => {}
    }
}

/// What became of a build
enum Outcome {
    Equal,
    Skipped(String),
    Failed(String),
}

fn check(data: &mut TypedData, id: &str, path: &PathBuf) -> Outcome {
    let text = std::fs::read_to_string(path).unwrap();
    let root = engine::xml::parse(&text).unwrap_or_else(|e| panic!("{id}: {e}"));
    data.load_tree(&root);
    let mut build = match data.load_build(&root) {
        Ok(b) => b,
        Err(e) => return Outcome::Failed(format!("build: {e}")),
    };
    let got = match engine::calcs::build_output(&mut build, &data.calc_data()) {
        Ok(o) => o,
        Err(e) if e.starts_with("not ported") => return Outcome::Skipped(e),
        Err(e) => return Outcome::Failed(e),
    };
    let mut got_main = BTreeMap::new();
    got_main.insert("player".to_string(), output_json(&got.player));
    if let Some(m) = &got.minion {
        got_main.insert("minion".to_string(), output_json(m));
    }
    let oracle = std::fs::read(oracle_dir().join(format!("{id}.json"))).unwrap();
    let J::Obj(mut want) = parse(&oracle, &mut 0) else {
        panic!("{id}: the oracle's output is not an object")
    };
    let want = want.remove("MAIN").expect("the oracle's MAIN outputs");
    let mut d = Vec::new();
    diff(&J::Obj(got_main), &want, "", &mut d);
    if d.is_empty() {
        Outcome::Equal
    } else {
        let n = d.len();
        d.truncate(25);
        Outcome::Failed(format!("{n} differences:\n    {}", d.join("\n    ")))
    }
}

#[test]
fn build_output_matches_oracle() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let builds: Vec<(String, PathBuf)> = corpus()
        .into_iter()
        .filter(|(id, p)| id.contains(&filter) && p.extension().is_some_and(|e| e == "xml"))
        .collect();
    assert!(!builds.is_empty(), "no build matches CALCS_FILTER={filter}");
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", lua::to_string(e.value())));
    let mut data = TypedData::load(&host);
    let results = builds
        .iter()
        .map(|(id, path)| (id.clone(), check(&mut data, id, path)));
    let mut equal = 0;
    let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut failures = Vec::new();
    for (id, outcome) in results {
        match outcome {
            Outcome::Equal => equal += 1,
            Outcome::Skipped(reason) => skips.entry(reason).or_default().push(id),
            Outcome::Failed(e) => failures.push(format!("{id}: {e}")),
        }
    }
    eprintln!(
        "{equal} builds equal to the oracle, {} skipped: {skips:?}",
        skips.values().map(Vec::len).sum::<usize>()
    );
    assert!(
        failures.is_empty(),
        "{} builds differ from the oracle:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
