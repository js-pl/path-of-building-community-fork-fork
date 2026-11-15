//! The oracle's outputs (`tests/oracle/<id>.json`, or the live oracle's
//! `oracleOutputs`, both `oracle/dump.lua`'s JSON) and the typed engine's
//! outputs compared with them: numbers bitwise, a missing or extra key a
//! difference.

use crate::typed_build::TypedData;
use engine::actor::OutVal;
use engine::calcs::BuildOutput;
use std::collections::BTreeMap;

/// A JSON value with numbers parsed exactly
#[derive(Clone, Debug)]
pub enum J {
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
pub fn parse(s: &[u8], i: &mut usize) -> J {
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

pub fn out_json(v: &OutVal) -> J {
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

pub fn output_json(out: &engine::calcs::Output) -> J {
    J::Obj(
        out.iter()
            .map(|(k, v)| (k.as_str().to_string(), out_json(v)))
            .collect(),
    )
}

/// Paths where two values differ
pub fn diff(got: &J, want: &J, path: &str, out: &mut Vec<String>) {
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

/// What became of a build's calculation
pub enum Outcome {
    Equal,
    Skipped(String),
    Failed(String),
}

pub fn compare(got: Result<BuildOutput, String>, want: J) -> Outcome {
    let got = match got {
        Ok(o) => o,
        Err(e) if e.starts_with("not ported") => return Outcome::Skipped(e),
        Err(e) => return Outcome::Failed(e),
    };
    let mut obj = BTreeMap::new();
    obj.insert("player".to_string(), output_json(&got.player));
    if let Some(m) = &got.minion {
        obj.insert("minion".to_string(), output_json(m));
    }
    let mut d = Vec::new();
    diff(&J::Obj(obj), &want, "", &mut d);
    if d.is_empty() {
        Outcome::Equal
    } else {
        let n = d.len();
        d.truncate(25);
        Outcome::Failed(format!("{n} differences:\n    {}", d.join("\n    ")))
    }
}

/// The outcomes of a build's `MAIN` and `CALCS` calculations against the
/// oracle's outputs of it (`want`, `dump.lua`'s JSON)
pub fn check(data: &mut TypedData, id: &str, xml: &str, want: impl FnOnce() -> J) -> [Outcome; 2] {
    let root = engine::xml::parse(xml).unwrap_or_else(|e| panic!("{id}: {e}"));
    data.load_tree(&root);
    let mut build = match data.load_build(&root) {
        Ok(b) => b,
        Err(e) => return [0, 1].map(|_| Outcome::Failed(format!("build: {e}"))),
    };
    let got = match engine::calcs::build_outputs(&mut build, &data.calc_data()) {
        Ok(o) => o,
        Err(e) if e.starts_with("not ported") => {
            return [0, 1].map(|_| Outcome::Skipped(e.clone()));
        }
        Err(e) => return [0, 1].map(|_| Outcome::Failed(e.clone())),
    };
    let J::Obj(mut want) = want() else {
        panic!("{id}: the oracle's output is not an object")
    };
    let mut want = |k: &str| {
        want.remove(k)
            .unwrap_or_else(|| panic!("the oracle's {k} outputs"))
    };
    [
        compare(Ok(got.main), want("MAIN")),
        compare(got.calcs, want("CALCS")),
    ]
}
