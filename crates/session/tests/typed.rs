//! The typed engine's commands give the translated session's JSON, for
//! every corpus build. `SESSION_FILTER=<substring>` limits the builds.

use serde_json::{Value as J, json};
use session::typed::{TypedData, TypedSession};
use session::{Dispatcher, Prefer, Session};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use upstream::imported::Imported;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn src() -> PathBuf {
    root().join("upstream/PathOfBuilding/src")
}

/// The XML builds of `corpus/corpus.toml`, by id
fn corpus() -> Vec<(String, PathBuf)> {
    let text = std::fs::read_to_string(root().join("corpus/corpus.toml")).unwrap();
    let value = |l: &str| {
        l.split_once('=')
            .map(|(_, v)| v.trim().trim_matches('"').to_string())
    };
    let (mut out, mut id, mut first, mut count) = (Vec::new(), None, 0, 0);
    for l in text.lines().map(str::trim) {
        if l.starts_with("id =") {
            id = value(l);
        } else if l.starts_with("path =") {
            out.push((id.take().unwrap(), root().join(value(l).unwrap())));
        } else if l.starts_with("first_seed =") {
            first = value(l).unwrap().parse().unwrap();
        } else if l.starts_with("count =") {
            count = value(l).unwrap().parse().unwrap();
        }
    }
    for seed in first..first + count {
        out.push((
            format!("gen-{seed:04}"),
            root().join(format!("corpus/generated/gen-{seed:04}.xml")),
        ));
    }
    let filter = std::env::var("SESSION_FILTER").unwrap_or_default();
    out.retain(|(id, p)| p.extension().is_some_and(|e| e == "xml") && id.contains(&filter));
    out
}

fn answer(r: Result<String, String>, what: &str) -> J {
    let text = r.unwrap_or_else(|e| panic!("{what}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{what}: {e}: {text}"))
}

/// Where two JSON values differ, as paths
fn diff(a: &J, b: &J, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (J::Object(x), J::Object(y)) => {
            for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                let (u, v) = (x.get(k).unwrap_or(&J::Null), y.get(k).unwrap_or(&J::Null));
                diff(u, v, &format!("{path}.{k}"), out);
            }
        }
        (J::Array(x), J::Array(y)) if x.len() == y.len() => {
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                diff(u, v, &format!("{path}[{i}]"), out);
            }
        }
        _ if a != b => out.push(format!("{path}: typed {a}, translated {b}")),
        _ => {}
    }
}

fn data() -> &'static TypedData {
    let t = Instant::now();
    let data = Box::leak(Box::new(
        TypedData::from_source(&src()).unwrap_or_else(|e| panic!("{e}")),
    ));
    eprintln!("typed data in {:.1?}", t.elapsed());
    data
}

#[test]
fn typed_commands_match_translated() {
    let data = data();
    let translated = Session::new(Rc::new(Imported::new(src()))).unwrap_or_else(|e| panic!("{e}"));
    let mut d = Dispatcher::new(translated, Some(data), Prefer::Typed);
    let (mut failures, mut skipped, mut compared) = (Vec::new(), Vec::new(), 0);
    let mut times = [Duration::ZERO; 4];
    for (id, path) in corpus() {
        let xml =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if let Err(e) = TypedSession::load(data, &xml, None) {
            skipped.push(format!("{id}: {e}"));
            continue;
        }
        let load = json!({"cmd": "loadXml", "xml": xml}).to_string();
        let t = Instant::now();
        answer(d.request(&load), "typed load");
        times[0] += t.elapsed();
        assert!(
            d.typed_active(),
            "{id}: the typed engine did not load the build"
        );
        let mut typed = Vec::new();
        for cmd in ["state", "outputs"] {
            let t = Instant::now();
            typed.push(answer(d.request(&json!({"cmd": cmd}).to_string()), cmd));
            times[1] += t.elapsed();
        }
        let t = Instant::now();
        answer(
            d.translated_request(r#"{"cmd":"state"}"#),
            "translated load",
        );
        times[2] += t.elapsed();
        for (cmd, typed) in ["state", "outputs"].iter().zip(typed) {
            let t = Instant::now();
            let tr = answer(d.translated_request(&json!({"cmd": cmd}).to_string()), cmd);
            times[3] += t.elapsed();
            let mut out = Vec::new();
            diff(&typed, &tr, cmd, &mut out);
            if !out.is_empty() {
                out.truncate(8);
                failures.push(format!("{id}:\n  {}", out.join("\n  ")));
            }
        }
        if compared == 0 {
            // A build code loads typed to the same sidebar
            let code = answer(d.translated_request(r#"{"cmd":"saveCode"}"#), "saveCode");
            let req = json!({"cmd": "loadCode", "code": code["ok"]["code"]}).to_string();
            answer(d.request(&req), "loadCode");
            assert!(
                d.typed_active(),
                "{id}: the typed engine did not load the build code"
            );
            let typed = answer(d.request(r#"{"cmd":"state"}"#), "state");
            let tr = answer(d.translated_request(r#"{"cmd":"state"}"#), "state");
            assert_eq!(typed, tr, "{id}: state after loadCode");
        }
        compared += 1;
    }
    eprintln!(
        "{compared} builds: typed load {:.1?}, typed state+outputs {:.1?}; translated load and state {:.1?}, state+outputs {:.1?}",
        times[0], times[1], times[2], times[3]
    );
    eprintln!("skipped {}:\n{}", skipped.len(), skipped.join("\n"));
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The colour codes the sidebar uses are upstream's
#[test]
fn color_codes_match_upstream() {
    let src = gamedata::Source::load(Path::new(&src())).unwrap();
    let codes = src.global("colorCodes");
    for (name, code) in session::typed::display_stats::COLOR_CODES {
        let v = codes.as_table().unwrap().get_str(name);
        assert_eq!(
            v.as_str().map(|s| s.to_str_lossy().into_owned()).as_deref(),
            Some(*code),
            "{name}"
        );
    }
}

/// Build codes decode as the import tab decodes them
#[test]
fn build_codes_decode() {
    let xml = "<PathOfBuilding><Build/></PathOfBuilding>";
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(xml.as_bytes(), 9);
    let b64: String = {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut s = String::new();
        for c in packed.chunks(3) {
            let n = c.iter().fold(0u32, |n, &b| n << 8 | b as u32) << (8 * (3 - c.len()));
            for i in 0..=c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        s
    };
    assert_eq!(
        session::dispatch::decode_build_code(&b64).as_deref(),
        Some(xml)
    );
}

/// Node power, the heat map's command, gives the translation's JSON:
/// combined offence and defence and Full DPS, on a build
/// (`SESSION_POWER=<substring>` picks others; it is slow on the translation)
#[test]
fn node_power_matches_translated() {
    let data = data();
    let translated = Session::new(Rc::new(Imported::new(src()))).unwrap_or_else(|e| panic!("{e}"));
    let mut d = Dispatcher::new(translated, Some(data), Prefer::Typed);
    let filter = std::env::var("SESSION_POWER").ok();
    let mut failures = Vec::new();
    for (id, path) in corpus() {
        let sample = id == "gen-0001";
        if !filter.as_deref().map_or(sample, |f| id.contains(f)) {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        assert!(d.typed_active(), "{id}: not loaded typed");
        for index in [1, 2] {
            let req = json!({"cmd": "nodePower", "index": index}).to_string();
            let t = Instant::now();
            let typed = answer(d.request(&req), "typed nodePower");
            let tt = t.elapsed();
            let t = Instant::now();
            let tr = answer(d.translated_request(&req), "translated nodePower");
            eprintln!(
                "{id} stat {index}: typed {tt:.1?}, translated {:.1?}",
                t.elapsed()
            );
            let mut out = Vec::new();
            diff(&typed, &tr, "nodePower", &mut out);
            if !out.is_empty() {
                out.truncate(8);
                failures.push(format!("{id} stat {index}:\n  {}", out.join("\n  ")));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The Calcs tab's cells, its controls and every cell's breakdown on the
/// dispatcher's build: where they differ, as paths
fn compare_calcs(d: &mut Dispatcher, id: &str, times: &mut [Duration; 2]) -> (usize, Vec<String>) {
    let mut reqs = vec![json!({"cmd": "calcs"})];
    let mut paths = vec![json!("build.mainSkill"), json!("build.mainSocketGroup")];
    let typed = answer(d.request(r#"{"cmd":"calcs"}"#), "typed calcs");
    for s in typed["ok"]["sections"].as_array().into_iter().flatten() {
        for u in s["subs"].as_array().into_iter().flatten() {
            for r in u["rows"].as_array().into_iter().flatten() {
                for c in r["cells"].as_array().into_iter().flatten() {
                    if let Some(b) = c.get("breakdown") {
                        reqs.push(json!({"cmd": "breakdown", "cell": b}));
                    }
                    if let Some(p) = c.get("control") {
                        paths.push(p.clone());
                    }
                }
            }
        }
    }
    reqs.insert(1, json!({"cmd": "controls", "paths": paths}));
    let mut out = Vec::new();
    for req in &reqs {
        let req = req.to_string();
        let t = Instant::now();
        let typed = answer(d.request(&req), "typed");
        times[0] += t.elapsed();
        let t = Instant::now();
        let tr = answer(d.translated_request(&req), "translated");
        times[1] += t.elapsed();
        let n = out.len();
        diff(&typed, &tr, &req, &mut out);
        if n == 0 && !out.is_empty() && std::env::var("SESSION_DUMP").is_ok() {
            eprintln!("{id} {req}:\ntyped {typed}\ntranslated {tr}");
        }
    }
    (reqs.len() - 2, out)
}

/// The Calcs tab gives the translation's JSON, for every corpus build.
/// Upstream's tree keeps the source `modLib.mergeKeystones` last gave a
/// keystone's mods, also from builds loaded before: a build that differs
/// is compared again on a fresh translated session.
#[test]
fn calcs_match_translated() {
    let data = data();
    let session = || Session::new(Rc::new(Imported::new(src()))).unwrap_or_else(|e| panic!("{e}"));
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let (mut failures, mut compared, mut cells) = (Vec::new(), 0, 0);
    let mut times = [Duration::ZERO; 2];
    for (id, path) in corpus() {
        let xml = std::fs::read_to_string(&path).unwrap();
        let load = json!({"cmd": "loadXml", "xml": xml}).to_string();
        answer(d.request(&load), "load");
        if !d.typed_active() {
            continue;
        }
        let (n, mut out) = compare_calcs(&mut d, &id, &mut times);
        if !out.is_empty() {
            let mut fresh = Dispatcher::new(session(), Some(data), Prefer::Typed);
            answer(fresh.request(&load), "load");
            out = compare_calcs(&mut fresh, &id, &mut [Duration::ZERO; 2]).1;
        }
        cells += n;
        compared += 1;
        if !out.is_empty() {
            let n = out.len();
            out.truncate(6);
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
    }
    eprintln!(
        "{compared} builds, {cells} breakdowns: typed {:.1?}, translated {:.1?} (with its load)",
        times[0], times[1]
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A small deterministic random number generator for edit sequences
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// Random sequences of tree edits (clicks on allocated and unallocated
/// nodes, ascendancies and masteries with an effect chosen, a reset) give
/// the translation's sidebar, outputs and tree state after each step
#[test]
fn tree_edits_match_translated() {
    let data = data();
    let translated = Session::new(Rc::new(Imported::new(src()))).unwrap_or_else(|e| panic!("{e}"));
    let mut d = Dispatcher::new(translated, Some(data), Prefer::Typed);
    let (mut failures, mut steps, mut times) = (Vec::new(), 0, [Duration::ZERO; 2]);
    let mut skipped = Vec::new();
    let steps_per_build: usize = std::env::var("SESSION_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    for (k, (id, path)) in corpus().into_iter().enumerate() {
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let tree = answer(d.translated_request(r#"{"cmd":"tree"}"#), "tree");
        let ids: Vec<u64> = tree["ok"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_u64())
            .collect();
        // Large cluster sockets, whose edits rebuild the subgraphs
        let sockets: Vec<u64> = tree["ok"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] == "ClusterSocket")
            .filter_map(|n| n["id"].as_u64())
            .filter(|&i| i < 0x10000)
            .collect();
        let mut rng = Lcg(k as u64 + 1);
        let mut out = Vec::new();
        let compare = |d: &mut Dispatcher, reqs: &[J], step: &str, out: &mut Vec<String>| {
            for req in reqs {
                let edit = req["cmd"] == "selectMastery";
                let req = req.to_string();
                let typed = answer(d.request(&req), "typed");
                if edit {
                    // The translation replays the typed edit
                    answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
                    assert!(d.typed_active(), "{step} {req}: {:?}", d.typed_error);
                    continue;
                }
                let tr = answer(d.translated_request(&req), "translated");
                diff(&typed, &tr, &format!("{step} {req}"), out);
            }
        };
        let mut socket_clicks = Vec::new();
        {
            let state = answer(d.request(r#"{"cmd":"treeState"}"#), "treeState");
            let alloc = &state["ok"]["alloc"];
            for s in &sockets {
                if alloc
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| a.as_u64() == Some(*s))
                {
                    socket_clicks.push(*s);
                    socket_clicks.push(*s);
                    break;
                }
            }
        }
        let total = steps_per_build + socket_clicks.len();
        for step in 0..=total {
            let state = answer(d.request(r#"{"cmd":"treeState"}"#), "treeState");
            let alloc: Vec<u64> = state["ok"]["alloc"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(J::as_u64)
                .collect();
            let req = if let Some(s) = (step < socket_clicks.len()).then(|| socket_clicks[step]) {
                json!({"cmd": "clickNode", "id": s})
            } else if step == total {
                json!({"cmd": "resetTree"})
            } else if rng.below(2) == 0 && alloc.len() > 2 {
                json!({"cmd": "clickNode", "id": alloc[rng.below(alloc.len())]})
            } else {
                // A node next to the allocation more often than not
                let mut pick = ids[rng.below(ids.len())];
                for _ in 0..40 {
                    let p = answer(
                        d.request(&json!({"cmd": "nodePath", "id": pick}).to_string()),
                        "nodePath",
                    );
                    let n = p["ok"]["path"].as_array().map_or(0, Vec::len);
                    if (1..=4).contains(&n) {
                        break;
                    }
                    pick = ids[rng.below(ids.len())];
                }
                json!({"cmd": "clickNode", "id": pick})
            };
            let label = format!("step {step} {req}");
            let r = req.to_string();
            let t = Instant::now();
            let typed = answer(d.request(&r), "typed edit");
            times[0] += t.elapsed();
            if !d.typed_active() {
                let e = d.typed_error.clone().unwrap_or_default();
                assert!(e.starts_with("not ported"), "{id}: {label}: {e}");
                skipped.push(format!("{id}: {e}"));
                break;
            }
            let t = Instant::now();
            let tr = answer(
                d.translated_request(r#"{"cmd":"treeState"}"#),
                "translated edit",
            );
            times[1] += t.elapsed();
            let _ = tr;
            let mut follow = vec![
                json!({"cmd": "treeState"}),
                json!({"cmd": "state"}),
                json!({"cmd": "outputs"}),
            ];
            if let Some(i) = req.get("id") {
                follow.push(json!({"cmd": "nodePath", "id": i}));
            }
            if let Some(opts) = typed["ok"]["mastery"].as_array() {
                follow.push(json!({"cmd": "masteryOptions", "id": req["id"]}));
                if !opts.is_empty() {
                    let e = &opts[rng.below(opts.len())]["id"];
                    follow.push(json!({"cmd": "selectMastery", "id": req["id"], "effect": e}));
                    follow.push(json!({"cmd": "treeState"}));
                    follow.push(json!({"cmd": "state"}));
                    follow.push(json!({"cmd": "outputs"}));
                }
            }
            compare(&mut d, &follow, &label, &mut out);
            steps += 1;
            if !out.is_empty() {
                break;
            }
        }
        if !out.is_empty() {
            let n = out.len();
            out.truncate(8);
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
    }
    eprintln!(
        "{steps} edits: typed {:.1?}, translated {:.1?} (with its replays)\nskipped:\n{}",
        times[0],
        times[1],
        skipped.join("\n")
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
