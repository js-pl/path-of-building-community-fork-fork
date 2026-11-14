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

/// Where two JSON values differ, as paths. Against the oracle, in their
/// canonical forms ([`unordered`]).
fn diff(a: &J, b: &J, path: &str, out: &mut Vec<String>) {
    if oracle() {
        let (mut a, mut b) = (a.clone(), b.clone());
        unordered(&mut a);
        unordered(&mut b);
        return diff_values(&a, &b, path, out);
    }
    diff_values(a, b, path, out)
}

/// Whether the reference is the oracle (`SESSION_REFERENCE=oracle`)
fn oracle() -> bool {
    std::env::var("SESSION_REFERENCE").is_ok_and(|r| r == "oracle")
}

/// What upstream gives in `pairs` order over string or table keys, which
/// LuaJIT does not reproduce between runs (its string hash is seeded per
/// process), in a canonical order: breakdown rows, tree connectors, the
/// base-type lines of an item's text (`{variant:..}` runs), the party's
/// enemy modifiers and exported mods (`<value>|Skill:..` runs). Build codes are compared decoded (zlib and miniz
/// compress differently).
fn unordered(v: &mut J) {
    match v {
        J::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "code"
                    && let Some(xml) = x.as_str().and_then(session::dispatch::decode_build_code)
                {
                    *x = J::String(xml);
                }
                unordered(x);
                if (k == "rows" || k == "connectors")
                    && let J::Array(a) = x
                {
                    a.sort_by_key(|r| r.to_string());
                }
            }
        }
        J::Array(a) => a.iter_mut().for_each(unordered),
        J::String(s) if s.contains('\n') => {
            let mut lines: Vec<&str> = s.split('\n').collect();
            let enemy_mods = s.starts_with("^7-----");
            let mut i = 0;
            while i < lines.len() {
                let group = |l: &str| {
                    if enemy_mods {
                        !l.starts_with("---") && !l.starts_with("^7---")
                    } else {
                        l.trim_start().starts_with("{variant:") || l.contains("|Skill:")
                    }
                };
                let start = i;
                while i < lines.len() && group(lines[i]) {
                    i += 1;
                }
                lines[start..i].sort_unstable();
                i += 1;
            }
            *s = lines.join("\n");
        }
        _ => {}
    }
}

fn diff_values(a: &J, b: &J, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (J::Object(x), J::Object(y)) => {
            for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                let (u, v) = (x.get(k).unwrap_or(&J::Null), y.get(k).unwrap_or(&J::Null));
                diff_values(u, v, &format!("{path}.{k}"), out);
            }
        }
        (J::Array(x), J::Array(y)) if x.len() == y.len() => {
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                diff_values(u, v, &format!("{path}[{i}]"), out);
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

/// The reference session: the translation, or with
/// `SESSION_REFERENCE=oracle` upstream on LuaJIT (`oracle/session.lua`)
fn session() -> Session {
    if oracle() {
        return Session::oracle(&src()).unwrap_or_else(|e| panic!("{e}"));
    }
    Session::new(Rc::new(Imported::new(src()))).unwrap_or_else(|e| panic!("{e}"))
}

/// The phases of `typed_session_matches_translated`, with the number of
/// shards each splits the corpus into
const PHASES: [(&str, usize); 13] = [
    ("commands", 2),
    ("geometry", 3),
    ("calcs", 3),
    ("tree", 3),
    ("skills", 3),
    ("save", 3),
    ("items", 3),
    ("config", 3),
    ("tooltips", 3),
    ("party", 2),
    ("new", 1),
    ("overlays", 2),
    ("power", 2),
];

/// The typed engine's commands against the translated session's: the
/// sidebar and outputs, the tree's geometry, the Calcs tab, tree, skills
/// and item edits, the saved files and the tooltips, each over the corpus. Each phase runs on shards of the
/// corpus, each in a process of its own with one data and one translated
/// session (`testkit::run_shards`): the translation keeps about 12 MB of
/// every build it loads. `SESSION_PHASES=calcs,tree` runs only these,
/// `SESSION_SHARDS=<n>` splits each phase in `n` shards.
#[test]
fn typed_session_matches_translated() {
    let test = "typed_session_matches_translated";
    let Some(shard) = testkit::shard() else {
        let phases = std::env::var("SESSION_PHASES").unwrap_or_default();
        let run = |p: &str| phases.is_empty() || phases.split(',').any(|x| x == p);
        let count: Option<usize> = std::env::var("SESSION_SHARDS")
            .ok()
            .and_then(|s| s.parse().ok());
        let shards: Vec<String> = PHASES
            .iter()
            .filter(|(p, _)| run(p))
            .flat_map(|&(p, n)| {
                let n = count.unwrap_or(n);
                (0..n).map(move |k| format!("{p} {k}/{n}"))
            })
            .collect();
        return testkit::run_shards(
            test,
            &shards,
            &[
                " builds",
                " edits",
                " trees",
                "skipped ",
                " compared",
                "aligned ",
                "fell back ",
                "fallback",
            ],
        );
    };
    let (phase, part) = shard.split_once(' ').unwrap();
    let (k, n) = part.split_once('/').unwrap();
    SHARD.set((k.parse().unwrap(), n.parse().unwrap())).unwrap();
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    at_build(&mut d, "");
    match phase {
        "commands" => commands_match(data, &mut d),
        "geometry" => geometry_matches(&mut d),
        "calcs" => calcs_match(data, &mut d),
        "tree" => {
            tree_edits_match(&mut d);
            tree_controls_match(&mut d);
        }
        "skills" => skills_edits_match(&mut d),
        "save" => saves_match(&mut d),
        "items" => items_edits_match(&mut d),
        "config" => config_edits_match(&mut d),
        "tooltips" => tooltips_match(data, &mut d),
        "overlays" => overlays_match(data, &mut d),
        "party" => party_match(data, &mut d),
        "new" => new_build_matches(&mut d),
        "power" => power_sorts_match(&mut d),
        _ => panic!("unknown phase {phase}"),
    }
    at_build(&mut d, "");
    report_fallbacks(phase);
}

/// The Skills tab's gem dropdown sorted by DPS (`gemList` of a group's
/// gem slot: `GemSelectControl`'s list, each gem's DPS in the slot) and the
/// Items tab's item databases (`controls` of `items.uniqueDB` sorted by a
/// stat and searched, its inputs, `items.rareDB` searched) give the
/// translation's, on every eighth corpus build
fn power_sorts_match(d: &mut Dispatcher) {
    let (mut failures, mut compared, mut items_compared) = (Vec::new(), 0, 0);
    let (mut t_typed, mut t_tr) = (Duration::ZERO, Duration::ZERO);
    let builds: Vec<_> = corpus().into_iter().step_by(8).collect();
    for (n, (id, path)) in mine(builds).into_iter().enumerate() {
        at_build(d, &id);
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let skills = answer(d.request(r#"{"cmd":"skills"}"#), "skills");
        let group = skills["ok"]["mainSocketGroup"].as_f64().unwrap_or(1.0);
        let gems = skills["ok"]["groups"][group as usize - 1]["gems"]
            .as_array()
            .map_or(0, Vec::len);
        let slot = if n % 2 == 0 && gems >= 2 { 2 } else { gems + 1 };
        let req = json!({"cmd": "gemList", "group": group, "slot": slot}).to_string();
        d.typed_error = None;
        let t = Instant::now();
        let typed = answer(d.request(&req), "typed gemList");
        t_typed += t.elapsed();
        if let Some(e) = d.typed_error.take() {
            failures.push(format!("{id}: gemList fell back: {e}"));
            continue;
        }
        let t = Instant::now();
        let tr = answer(d.translated_request(&req), "translated gemList");
        t_tr += t.elapsed();
        if let Ok(dir) = std::env::var("SESSION_DUMP") {
            for (k, v) in [("typed", &typed), ("translated", &tr)] {
                std::fs::write(format!("{dir}/{id}-{k}.json"), v.to_string()).unwrap();
            }
        }
        let mut out = Vec::new();
        diff(&typed, &tr, &format!("gemList {group}/{slot}"), &mut out);
        if !out.is_empty() {
            let n = out.len();
            out.truncate(10);
            failures.push(format!("{id}: {n} differences\n    {}", out.join("\n    ")));
        }
        compared += 1;
        // The unique database sorted by a stat, its search limiting it to a
        // few items (one a flask), and the rare one searched
        const SEARCHES: [&str; 6] = ["kaom", "atziri", "doryani", "lioneye", "berek", "rigwald"];
        const SORTS: [usize; 5] = [3, 12, 2, 15, 1];
        let edits = [
            json!({"cmd": "control", "path": "items.uniqueDB.search", "text": SEARCHES[n % SEARCHES.len()]}),
            json!({"cmd": "control", "path": "items.uniqueDB.sort", "sel": SORTS[n % SORTS.len()]}),
            json!({"cmd": "control", "path": "items.rareDB.search", "text": "amulet"}),
            // An item type and a league `LoadLeaguesAndTypes` adds
            json!({"cmd": "control", "path": "items.uniqueDB.type", "sel": if n % 4 == 1 { 6 + n % 7 } else { 1 }}),
            json!({"cmd": "control", "path": "items.uniqueDB.league", "sel": if n % 3 == 2 { 3 + n % 5 } else { 1 }}),
            json!({"cmd": "control", "path": "items.rareDB.type", "sel": 6 + n % 9}),
        ];
        for e in &edits {
            let r = answer(d.request(&e.to_string()), "item db control");
            if let Some(err) = d.typed_error.take() {
                failures.push(format!("{id}: {e}: fell back: {err}"));
            }
            assert!(r.get("error").is_none(), "{id}: {e}: {r}");
        }
        let paths = [
            "items.uniqueDB",
            "items.rareDB",
            "items.uniqueDB.sort",
            "items.uniqueDB.search",
            "items.uniqueDB.slot",
            "items.uniqueDB.type",
            "items.uniqueDB.league",
            "items.uniqueDB.requirement",
            "items.uniqueDB.obtainable",
            "items.uniqueDB.searchMode",
            "items.rareDB.search",
            "items.rareDB.sort",
            "items.rareDB.type",
            "items.selectDB",
        ];
        let req = json!({"cmd": "controls", "paths": paths}).to_string();
        // Then with the rare templates selected
        for (k, select) in [None, Some(2)].into_iter().enumerate() {
            if let Some(sel) = select {
                let e = json!({"cmd": "control", "path": "items.selectDB", "sel": sel});
                answer(d.request(&e.to_string()), "selectDB");
                if let Some(err) = d.typed_error.take() {
                    failures.push(format!("{id}: selectDB fell back: {err}"));
                }
            }
            let t = Instant::now();
            let typed = answer(d.request(&req), "typed item db");
            t_typed += t.elapsed();
            if let Some(e) = d.typed_error.take() {
                failures.push(format!("{id}: item db fell back: {e}"));
                break;
            }
            let t = Instant::now();
            let tr = answer(d.translated_request(&req), "translated item db");
            t_tr += t.elapsed();
            let mut out = Vec::new();
            diff(&typed, &tr, &format!("itemDb {k}"), &mut out);
            if !out.is_empty() {
                let n = out.len();
                out.truncate(10);
                failures.push(format!(
                    "{id}: {n} item db differences\n    {}",
                    out.join("\n    ")
                ));
            }
            if k == 0 {
                items_compared += typed["ok"]["items.uniqueDB"]["list"]
                    .as_array()
                    .map_or(0, Vec::len);
            }
        }
    }
    eprintln!(
        "{compared} gem lists and item databases compared ({items_compared} unique rows): typed {t_typed:.1?}, translated {t_tr:.1?}"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Node tooltips of tattooed nodes and of the nodes timeless jewels
/// conquer, which the corpus has none of: the builds of
/// `equivalence::variants` (tattoos; a build with each timeless jewel)
fn overlays_match(data: &'static TypedData, d: &mut Dispatcher) {
    const JEWELS: [&str; 11] = [
        "Glorious Vanity",
        "Lethal Pride",
        "Brutal Restraint",
        "Militant Faith",
        "Elegant Hubris",
        "Heroic Tragedy",
        "Festering Vengeance",
        "Extinguishing Grasp",
        "Baleful Dominion",
        "Destructive Aspiration",
        "Reclaimed Malevolence",
    ];
    let mut eq = equivalence::typed_build::TypedData::load();
    let mut seen = std::collections::HashSet::new();
    let mut builds = equivalence::variants::tattoo_variants(&mut eq);
    builds.extend(
        equivalence::variants::timeless_variants(&mut eq)
            .into_iter()
            .filter(|(id, _)| seen.insert(id.split('-').nth(1).unwrap_or("").to_string())),
    );
    let tree = data.tree(data.game.latest_tree_version()).unwrap();
    let (mut failures, mut compared, mut aligned) = (Vec::new(), 0, Vec::new());
    for (id, xml) in mine(builds) {
        at_build(d, &id);
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let state = answer(d.request(r#"{"cmd":"treeState"}"#), "treeState");
        let alloc: Vec<u32> = state["ok"]["alloc"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_u64().map(|v| v as u32))
            .collect();
        // The tattooed nodes
        let attr = |e: &str, k: &str| -> Option<String> {
            Some(
                e.split(&format!("{k}=\""))
                    .nth(1)?
                    .split('"')
                    .next()?
                    .to_string(),
            )
        };
        let mut nodes: Vec<u32> = xml
            .split("<Override ")
            .skip(1)
            .filter_map(|o| attr(o, "nodeId")?.parse().ok())
            .collect();
        // The nodes in the radius of the sockets with timeless jewels
        let timeless: Vec<String> = xml
            .split("<Item ")
            .skip(1)
            .filter(|i| {
                JEWELS
                    .iter()
                    .any(|j| i.split("</Item>").next().unwrap_or("").contains(j))
            })
            .filter_map(|i| attr(i, "id"))
            .collect();
        for sock in xml.split("<Socket ").skip(1) {
            if !attr(sock, "itemId").is_some_and(|i| timeless.contains(&i)) {
                continue;
            }
            let Some(radius) = attr(sock, "nodeId")
                .and_then(|n| n.parse::<u32>().ok())
                .and_then(|n| tree.node(n)?.nodes_in_radius.as_ref()?.last())
            else {
                continue;
            };
            let (on, off): (Vec<u32>, Vec<u32>) = radius.iter().partition(|n| alloc.contains(n));
            nodes.extend(on);
            nodes.extend(off.into_iter().step_by(8));
        }
        nodes.sort_unstable();
        nodes.dedup();
        let mut out = Vec::new();
        let mut recheck = Vec::new();
        for n in nodes {
            let req = json!({"cmd": "nodeTooltip", "id": n}).to_string();
            let typed = answer(d.request(&req), "typed");
            let tr = answer(d.translated_request(&req), "translated");
            let mut lines = Vec::new();
            diff(&typed, &tr, &req, &mut lines);
            if !lines.is_empty() {
                recheck.push((req, typed, lines));
            }
            compared += 1;
        }
        // `ReplaceNode` wipes the reminder text a node reads from the
        // shared tree node (`wipeTable(old.reminderText)`), for every later
        // build of the process: a tooltip that differs is asked again of a
        // fresh translation, and aligned when that one agrees
        if !recheck.is_empty() {
            let fresh = session();
            let load = json!({"cmd": "loadXml", "xml": xml}).to_string();
            answer(fresh.request(&load), "fresh load");
            for (req, typed, lines) in recheck {
                let tr = answer(fresh.request(&req), "fresh");
                let mut again = Vec::new();
                diff(&typed, &tr, &req, &mut again);
                if again.is_empty() {
                    aligned.push(format!("{id}: {req}"));
                } else {
                    out.extend(lines);
                }
            }
        }
        // The tattooed spec copied in Manage Passive Trees, then switched to
        if xml.contains("<Override ") {
            let specs = answer(d.request(r#"{"cmd":"treeSpecs"}"#), "treeSpecs");
            let active = specs["ok"]["active"].clone();
            for r in [
                json!({"cmd": "control", "path": "items.specButton", "click": true}),
                json!({"cmd": "control", "path": "popup.1", "sel": active, "click": true}),
                json!({"cmd": "control", "path": "popup.1.copy", "click": true}),
                json!({"cmd": "popupButton", "name": "save"}),
            ] {
                answer(d.request(&r.to_string()), "copy tree");
                assert!(d.typed_active(), "{id} {r}: {:?}", d.typed_error);
            }
            let n = specs["ok"]["list"].as_array().map_or(1, Vec::len) + 1;
            let r = json!({"cmd": "selectTreeSpec", "index": n}).to_string();
            answer(d.request(&r), "selectTreeSpec");
            compare_views(
                d,
                &["treeSpecs", "treeState", "state", "outputs"],
                &format!("{id} tattooed copy"),
                &mut out,
            );
            compare_saves(d, &format!("{id} tattooed copy"), &mut out);
            answer(d.request(r#"{"cmd":"closePopup"}"#), "closePopup");
            assert!(d.typed_active(), "{id} closePopup: {:?}", d.typed_error);
        }
        if !out.is_empty() {
            let n = out.len();
            out.truncate(4);
            for l in &mut out {
                if let Some((at, _)) = l.char_indices().nth(600) {
                    l.truncate(at);
                }
            }
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
    }
    spec_versions_match(d, &mut failures);
    eprintln!(
        "{compared} overlay node tooltips compared, aligned with a fresh translation {}:\n{}",
        aligned.len(),
        aligned.join("\n")
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Corpus builds with a copy of their active spec on other tree versions,
/// which the corpus has none of: switching to each and back gives the
/// translation's sidebar, outputs, tree, items and file
fn spec_versions_match(d: &mut Dispatcher, failures: &mut Vec<String>) {
    let builds: Vec<(String, String)> = mine(corpus())
        .into_iter()
        .step_by(8)
        .map(|(id, path)| (id, std::fs::read_to_string(&path).unwrap()))
        .collect();
    for (id, xml) in builds {
        // The active spec, copied on two other versions
        let Some(start) = xml.find("<Spec ") else {
            continue;
        };
        let tag_end = start + xml[start..].find('>').unwrap();
        let end = if xml.as_bytes()[tag_end - 1] == b'/' {
            tag_end + 1
        } else {
            start + xml[start..].find("</Spec>").unwrap() + "</Spec>".len()
        };
        let spec = &xml[start..end];
        let copies: String = ["3_15", "3_25"]
            .iter()
            .map(|v| {
                let mut c = spec.to_string();
                if let Some(at) = c.find("treeVersion=\"") {
                    let e = at + c[at + 13..].find('"').unwrap() + 14;
                    c.replace_range(at..e, &format!("treeVersion=\"{v}\""));
                }
                c
            })
            .collect();
        let xml = format!("{}{copies}{}", &xml[..end], &xml[end..]);
        let id = format!("{id}-spec-versions");
        at_build(d, &id);
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let specs = answer(d.request(r#"{"cmd":"treeSpecs"}"#), "treeSpecs");
        let n = specs["ok"]["list"].as_array().map_or(1, Vec::len);
        let active = specs["ok"]["active"].as_u64().unwrap_or(1) as usize;
        let (mut out, mut broken) = (Vec::new(), false);
        for to in [n - 1, n, active] {
            let req = json!({"cmd": "selectTreeSpec", "index": to}).to_string();
            answer(d.request(&req), "selectTreeSpec");
            if !d.typed_active() {
                break;
            }
            answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
            // A copy upstream cannot show (nodes its tree version lacks)
            let probe = answer(d.translated_request(r#"{"cmd":"state"}"#), "state");
            if probe.get("error").is_some() {
                eprintln!(
                    "{id}: spec {to} fails in the translation: {}",
                    probe["error"]
                );
                broken = true;
                break;
            }
            for c in ["state", "outputs", "treeState", "items", "treeSpecs"] {
                let q = json!({ "cmd": c }).to_string();
                let t = answer(d.request(&q), c);
                let tr = answer(d.translated_request(&q), c);
                diff(&t, &tr, &format!("spec {to}: {c}"), &mut out);
            }
        }
        if broken {
            continue;
        }
        if out.is_empty() && d.typed_active() {
            compare_saves(d, "after the spec switches", &mut out);
        }
        if !out.is_empty() {
            let k = out.len();
            out.truncate(4);
            for l in &mut out {
                if let Some((at, _)) = l.char_indices().nth(600) {
                    l.truncate(at);
                }
            }
            failures.push(format!("{id} ({k}):\n  {}", out.join("\n  ")));
        }
    }
}

/// The fallbacks the typed session still has, as substrings of their
/// reasons: any other fails the test. Remove one once it is typed.
const KNOWN_FALLBACKS: &[&str] = &[
    // Upstream fails the rebuild after two cluster subgraphs shared an id
    // (`Entrance for subGraph not linked to parent socket???`)
    "rebuilding cluster subgraphs that shared an id",
    // The tooltip's stat comparison, where the translation carries another
    // build's change of upstream's shared data (see `compare_display`)
    "aligned with a fresh translation",
    // Party builds upstream fails on as well (`party_match` checks that
    // the translation fails on them too)
    "upstream fails too",
    // A fractional enemy level, which upstream's accuracy sums fail on
    "an enemy level without a monster accuracy",
];

/// The build the dispatcher's fallbacks are recorded for, and each
/// fallback so far: build, command and why
static FALLBACKS: std::sync::Mutex<(String, Vec<String>)> =
    std::sync::Mutex::new((String::new(), Vec::new()));

/// Records the dispatcher's fallbacks for the build so far, then starts
/// recording for `id`
fn at_build(d: &mut Dispatcher, id: &str) {
    let mut f = FALLBACKS.lock().unwrap();
    for (cmd, why) in d.fallbacks.drain(..) {
        let line = format!("{}: {cmd}: {why}", f.0);
        f.1.push(line);
    }
    f.0 = id.to_string();
}

/// A fallback outside the dispatcher, for the current build
fn note_fallback(cmd: &str, why: &str) {
    let mut f = FALLBACKS.lock().unwrap();
    let line = format!("{}: {cmd}: {why}", f.0);
    f.1.push(line);
}

/// Prints every fallback of the phase and fails on unknown ones
fn report_fallbacks(phase: &str) {
    let f = FALLBACKS.lock().unwrap();
    for l in &f.1 {
        eprintln!("fallback {phase} {l}");
    }
    eprintln!("{} fallbacks in phase {phase}", f.1.len());
    let unknown: Vec<&String> =
        f.1.iter()
            .filter(|l| !KNOWN_FALLBACKS.iter().any(|k| l.contains(k)))
            .collect();
    assert!(
        unknown.is_empty(),
        "fallbacks not in KNOWN_FALLBACKS:\n{}",
        unknown
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `newBuild` opens the translation's empty build: its views and file
fn new_build_matches(d: &mut Dispatcher) {
    answer(d.request(r#"{"cmd":"newBuild"}"#), "newBuild");
    assert!(
        d.typed_active(),
        "the typed engine did not open a new build"
    );
    let mut out = Vec::new();
    for cmd in [
        "state",
        "outputs",
        "skills",
        "items",
        "calcs",
        "treeState",
        "treeSpecs",
    ] {
        let req = json!({"cmd": cmd}).to_string();
        let typed = answer(d.request(&req), cmd);
        let tr = answer(d.translated_request(&req), cmd);
        diff(&typed, &tr, cmd, &mut out);
    }
    compare_saves(d, "newBuild", &mut out);
    // Then a skill
    for req in [
        json!({"cmd": "addSocketGroup"}),
        json!({"cmd": "setGem", "group": 1, "gem": 1, "name": "Fireball"}),
        json!({"cmd": "setGem", "group": 1, "gem": 2, "name": "Added Fire Damage"}),
    ] {
        let typed = answer(d.request(&req.to_string()), "typed edit");
        assert!(d.typed_active(), "{req}: {:?}", d.typed_error);
        let tr = answer(
            d.translated_request(r#"{"cmd":"skills"}"#),
            "translated edit",
        );
        assert!(tr.get("error").is_none(), "{req}: {tr}");
        assert!(typed.get("error").is_none(), "{req}: {typed}");
    }
    for cmd in ["state", "outputs", "skills"] {
        let req = json!({"cmd": cmd}).to_string();
        let typed = answer(d.request(&req), cmd);
        let tr = answer(d.translated_request(&req), cmd);
        diff(&typed, &tr, &format!("after a skill: {cmd}"), &mut out);
    }
    compare_saves(d, "newBuild with a skill", &mut out);
    out.truncate(20);
    assert!(out.is_empty(), "newBuild differs:\n{}", out.join("\n"));
}

/// The notes, and the tooltips of every socket group and item and of
/// nodes on and around the tree, give the translation's on every corpus
/// build, also after a few edits; `SESSION_NODES` sets the nodes per build
fn tooltips_match(data: &'static TypedData, d: &mut Dispatcher) {
    let (mut failures, mut fell_back, mut aligned) = (Vec::new(), Vec::new(), Vec::new());
    let mut counts: std::collections::BTreeMap<&str, (usize, Duration, Duration)> =
        Default::default();
    let nodes_per_build: usize = std::env::var("SESSION_NODES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let tree = data.tree(data.game.latest_tree_version()).unwrap();
    for (id, path) in mine(corpus()) {
        at_build(d, &id);
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let mut rng = Lcg(seed(&id) + 23);
        let mut out = Vec::new();
        // Requests that differ, rechecked on a fresh translation, with the
        // edits made before them
        let mut recheck: Vec<(String, J, Vec<String>, Vec<String>)> = Vec::new();
        let edits = std::cell::RefCell::new(Vec::<String>::new());
        let mut compare = |d: &mut Dispatcher, req: J| {
            let r = req.to_string();
            let cmd = req["cmd"].as_str().unwrap().to_string();
            if !d.typed_active() {
                return;
            }
            d.typed_error = None;
            let t = Instant::now();
            let typed = answer(d.request(&r), "typed");
            let tt = t.elapsed();
            let fell = d.typed_error.take();
            let t = Instant::now();
            let tr = answer(d.translated_request(&r), "translated");
            let c = counts.entry(Box::leak(cmd.into_boxed_str())).or_default();
            match fell {
                Some(e) => {
                    let e = e.lines().next().unwrap_or("").to_string();
                    fell_back.push(format!("{id}: {r:.120}: {e:.160}"));
                }
                None => {
                    c.0 += 1;
                    c.1 += tt;
                    c.2 += t.elapsed();
                    let mut lines = Vec::new();
                    diff(&typed, &tr, &format!("{r:.200}"), &mut lines);
                    if !lines.is_empty() {
                        recheck.push((r, typed, lines, edits.borrow().clone()));
                    }
                }
            }
        };
        compare(d, json!({"cmd": "notes"}));
        compare(d, json!({"cmd": "party"}));
        let groups = answer(d.request(r#"{"cmd":"skills"}"#), "skills")["ok"]["groups"]
            .as_array()
            .map_or(0, Vec::len);
        for g in 1..=groups {
            compare(d, json!({"cmd": "socketGroupTooltip", "index": g}));
        }
        let items = answer(d.request(r#"{"cmd":"items"}"#), "items");
        for item in items["ok"]["items"].as_array().into_iter().flatten() {
            compare(d, json!({"cmd": "itemTooltip", "id": item["id"]}));
        }
        // Equipped items compared in their own slot, and the first item's
        // text as a preview
        let slots: Vec<J> = items["ok"]["slots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["item"].is_number())
            .cloned()
            .collect();
        for _ in 0..3.min(slots.len()) {
            let s = &slots[rng.below(slots.len())];
            let req = json!({"cmd": "itemTooltip", "id": s["item"], "slot": s["name"]});
            compare(d, req);
        }
        if let Some(raw) = items["ok"]["items"][0]["raw"].as_str() {
            compare(d, json!({"cmd": "previewItem", "text": raw}));
        }
        // Allocated nodes, their neighbours, and the allocated sockets
        let state = answer(d.request(r#"{"cmd":"treeState"}"#), "treeState");
        let alloc: Vec<u32> = state["ok"]["alloc"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_u64().map(|v| v as u32))
            .collect();
        let mut near: Vec<u32> = alloc
            .iter()
            .filter_map(|n| tree.node_index.get(n))
            .flat_map(|&i| tree.nodes[i].linked_id.iter().copied())
            .filter(|n| !alloc.contains(n))
            .collect();
        near.sort_unstable();
        near.dedup();
        let mut nodes: Vec<u32> = slots
            .iter()
            .filter_map(|s| s["nodeId"].as_u64().map(|n| n as u32))
            .take(2)
            .collect();
        for k in 0..nodes_per_build {
            let pool = if k % 2 == 0 { &alloc } else { &near };
            if !pool.is_empty() {
                nodes.push(pool[rng.below(pool.len())]);
            }
        }
        // Tattooed nodes, and nodes in the radius of socketed jewels,
        // which timeless jewels conquer
        for o in xml.split("<Override ").skip(1) {
            let id = o
                .split("nodeId=\"")
                .nth(1)
                .and_then(|v| v.split('"').next()?.parse::<u32>().ok());
            nodes.extend(id);
        }
        for s in &slots {
            let Some(radius) = s["nodeId"]
                .as_u64()
                .and_then(|n| tree.node(n as u32)?.nodes_in_radius.as_ref()?.last())
            else {
                continue;
            };
            let (on, off): (Vec<u32>, Vec<u32>) = radius.iter().partition(|n| alloc.contains(n));
            for pool in [&on, &on, &off] {
                if !pool.is_empty() {
                    nodes.push(pool[rng.below(pool.len())]);
                }
            }
        }
        nodes.sort_unstable();
        nodes.dedup();
        for n in nodes {
            compare(d, json!({"cmd": "nodeTooltip", "id": n}));
        }
        // Notes edited, and tooltips after a tree edit
        let text = format!("Notes of {id}\n^_1colour ^_xFF0000and ^1codes");
        let req = json!({"cmd": "setNotes", "text": text}).to_string();
        answer(d.request(&req), "setNotes");
        edits.borrow_mut().push(req);
        compare(d, json!({"cmd": "notes"}));
        if let Some(&n) = near.first() {
            let req = json!({"cmd": "clickNode", "id": n}).to_string();
            answer(d.request(&req), "clickNode");
            edits.borrow_mut().push(req);
            compare(d, json!({"cmd": "nodeTooltip", "id": n}));
            for g in 1..=groups.min(2) {
                compare(d, json!({"cmd": "socketGroupTooltip", "index": g}));
            }
        }
        if d.typed_active() {
            compare_saves(d, "after the notes", &mut out);
            compare(d, json!({"cmd": "notes"}));
        }
        // Upstream changes shared skill data for good (`ProcessSocketGroup`
        // clears a triggered granted skill's level cost), so a build loaded
        // earlier in the session can change the translation's answer: a
        // request that differs is asked again of a fresh translation, with
        // the same edits before it, and aligned when that one agrees
        for (r, typed, lines, done) in recheck {
            let fresh = session();
            let load = json!({"cmd": "loadXml", "xml": xml}).to_string();
            answer(fresh.request(&load), "fresh load");
            for e in &done {
                answer(fresh.request(e), "fresh edit");
            }
            let tr = answer(fresh.request(&r), "fresh");
            let mut again = Vec::new();
            diff(&typed, &tr, &r, &mut again);
            if again.is_empty() {
                aligned.push(format!("{id}: {r:.120}"));
            } else {
                out.extend(lines);
            }
        }
        if !out.is_empty() {
            let n = out.len();
            out.truncate(8);
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
    }
    eprintln!(
        "aligned with a fresh translation {}:\n{}",
        aligned.len(),
        aligned.join("\n")
    );
    for (cmd, (n, t, tr)) in &counts {
        eprintln!("{cmd}: {n} compared, typed {t:.1?}, translated {tr:.1?}");
    }
    eprintln!("fell back {}:\n{}", fell_back.len(), fell_back.join("\n"));
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The views compared after a config or class edit
const CONFIG_VIEWS: [&str; 4] = ["config", "state", "outputs", "treeState"];

/// Compares read-only commands of the typed engine and the translation
fn compare_views(d: &mut Dispatcher, cmds: &[&str], label: &str, out: &mut Vec<String>) {
    for cmd in cmds {
        let q = json!({ "cmd": cmd }).to_string();
        let typed = answer(d.request(&q), cmd);
        assert!(d.typed_active(), "{label} {cmd}: {:?}", d.typed_error);
        let tr = answer(d.translated_request(&q), cmd);
        diff(&typed, &tr, &format!("{label} {cmd}"), out);
    }
}

/// A random edit of the Configuration tab's shown options, the class,
/// ascendancy and bloodline drop-downs (with the class change popup),
/// config sets and the tree's undo
fn config_edit(d: &mut Dispatcher, rng: &mut Lcg) -> J {
    let config = answer(d.request(r#"{"cmd":"config"}"#), "config");
    let state = answer(d.request(r#"{"cmd":"state"}"#), "state");
    if let Some(p) = state["ok"]["popups"].as_array().and_then(|p| p.first()) {
        let _ = p;
        return match rng.below(3) {
            0 => json!({"cmd": "popupButton", "name": "confirm"}),
            1 => json!({"cmd": "closePopup"}),
            // A button the class change popup has without a name
            _ => json!({"cmd": "popupButton", "name": "cancel"}),
        };
    }
    if rng.below(12) == 0 {
        // No popup to press
        return json!({"cmd": "popupButton", "name": "confirm"});
    }
    let vars: Vec<&J> = config["ok"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["vars"].as_array().unwrap())
        .collect();
    if rng.below(5) == 0 {
        // The tab's search, Show All Configurations and the set
        // drop-down, which the tab fills when it draws
        return match rng.below(4) {
            0 | 1 => {
                let s = [
                    "",
                    "life",
                    "enemy",
                    "is the",
                    "%",
                    "[",
                    "  ",
                    "(burning|shocked)",
                    "x^7y",
                ][rng.below(9)];
                json!({"cmd": "control", "path": "configTab.search", "text": s})
            }
            2 => json!({"cmd": "control", "path": "configTab.toggleConfigs", "click": true}),
            _ => json!({"cmd": "control", "path": "configTab.setSelect", "sel": rng.below(3) + 1}),
        };
    }
    match rng.below(10) {
        0 | 1 => {
            let path = [
                "build.classDrop",
                "build.ascendDrop",
                "build.secondaryAscendDrop",
            ][rng.below(3)];
            let n = state["ok"]["controls"][path]["list"]
                .as_array()
                .map_or(1, Vec::len);
            json!({"cmd": "control", "path": path, "sel": rng.below(n) + 1})
        }
        2 => json!({"cmd": "undo"}),
        3 => {
            let sets = config["ok"]["configSets"].as_array().unwrap();
            let id = &sets[rng.below(sets.len())]["id"];
            json!({"cmd": "selectConfigSet", "id": id})
        }
        // A search that hides every option: cleared
        _ if vars.is_empty() => json!({"cmd": "control", "path": "configTab.search", "text": ""}),
        _ => {
            let v = vars[rng.below(vars.len())];
            let path = format!("config.{}", v["var"].as_str().unwrap());
            match v["class"].as_str().unwrap() {
                "CheckBoxControl" => {
                    json!({"cmd": "control", "path": path, "state": v["state"] != J::Bool(true)})
                }
                "DropDownControl" => {
                    let n = v["list"].as_array().unwrap().len();
                    json!({"cmd": "control", "path": path, "sel": rng.below(n) + 1})
                }
                _ => {
                    let texts = ["", "0", "1", "5", "50", "100", "-10", "3.5", "1000"];
                    json!({"cmd": "control", "path": path, "text": texts[rng.below(texts.len())]})
                }
            }
        }
    }
}

/// Random sequences of Configuration tab edits, class changes and undo
/// give the translation's options (shown, values, tooltips), sidebar,
/// outputs and tree after each step; then a new build does
fn config_edits_match(d: &mut Dispatcher) {
    let (mut failures, mut steps, mut times) = (Vec::new(), 0, [Duration::ZERO; 2]);
    let mut popup_tooltips = 0;
    let mut skipped = Vec::new();
    let steps_per_build: usize = std::env::var("SESSION_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    for (id, path) in mine(corpus()) {
        at_build(d, &id);
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let mut rng = Lcg(seed(&id) + 3);
        let mut out = Vec::new();
        compare_views(d, &CONFIG_VIEWS, "load", &mut out);
        for step in 0..steps_per_build {
            if !out.is_empty() {
                break;
            }
            let open = answer(d.request(r#"{"cmd":"state"}"#), "state")["ok"]["popups"]
                .as_array()
                .is_some_and(|p| !p.is_empty());
            if !open && rng.below(8) == 0 {
                // Manage Config Sets: sets copied,
                // added, renamed, deleted and switched between edits
                let r = json!({"cmd": "control", "path": "configTab.setManage", "click": true});
                answer(d.request(&r.to_string()), "manage");
                let label = format!("step {step} config sets");
                let (mut log, mut counts) = (Vec::new(), Default::default());
                if !d.typed_active()
                    || drive_popups(d, &mut rng, &label, &mut log, &mut out, &mut counts)
                {
                    let e = d.typed_error.clone().unwrap_or_default();
                    skipped.push(format!("{id}: {e}"));
                    break;
                }
                answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
                compare_views(d, &CONFIG_VIEWS, &label, &mut out);
                steps += 1;
                continue;
            }
            let req = config_edit(d, &mut rng);
            let label = format!("step {step} {req}");
            if std::env::var("SESSION_DUMP").is_ok() {
                eprintln!("{id} {label}");
            }
            let r = req.to_string();
            let t = Instant::now();
            let typed = answer(d.request(&r), "typed edit");
            times[0] += t.elapsed();
            if !d.typed_active() {
                let e = d.typed_error.clone().unwrap_or_default();
                assert!(e.starts_with("not ported"), "{id}: {label}: {e}");
                skipped.push(format!("{id}: {e}"));
                if e.contains("upstream fails") {
                    // Upstream's failure leaves its session broken: a
                    // fresh one for the builds after
                    at_build(d, &id);
                    *d = Dispatcher::new(session(), Some(data()), Prefer::Typed);
                }
                break;
            }
            if typed.get("error").is_some() {
                // An error changes nothing: the translation's own answer
                let tr = answer(d.translated_request(&r), "translated edit");
                diff(&typed, &tr, &label, &mut out);
            }
            let t = Instant::now();
            answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
            times[1] += t.elapsed();
            compare_views(d, &CONFIG_VIEWS, &label, &mut out);
            // An allocated mastery's tooltip with the class change popup
            // open compares reallocating it
            let state = answer(d.request(r#"{"cmd":"state"}"#), "state");
            if state["ok"]["popups"]
                .as_array()
                .is_some_and(|p| !p.is_empty())
            {
                let tree = answer(d.request(r#"{"cmd":"treeState"}"#), "treeState");
                let mastery = tree["ok"]["masteries"]
                    .as_object()
                    .and_then(|m| m.keys().next()?.parse::<u32>().ok());
                if let Some(m) = mastery {
                    // An allocated mastery has no dependent nodes: its path
                    // (`depends`) is itself
                    let p = json!({"cmd": "nodePath", "id": m}).to_string();
                    let path = answer(d.request(&p), "nodePath");
                    assert_eq!(path["ok"]["path"], json!([m]), "{id}: {label}: {p}");
                    let q = json!({"cmd": "nodeTooltip", "id": m}).to_string();
                    let t = answer(d.request(&q), "typed tooltip");
                    let tr = answer(d.translated_request(&q), "translated tooltip");
                    diff(&t, &tr, &format!("{label} {q}"), &mut out);
                    popup_tooltips += 1;
                }
            }
            steps += 1;
        }
        if out.is_empty() && d.typed_active() {
            compare_saves(d, "after the config edits", &mut out);
        }
        if !out.is_empty() {
            let n = out.len();
            out.truncate(8);
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
    }
    // A new build, a class change on it and its undo, in the first shard
    let mut out = Vec::new();
    if SHARD.get().is_none_or(|&(k, _)| k == 0) {
        at_build(d, "newBuild");
        answer(d.request(r#"{"cmd":"newBuild"}"#), "newBuild");
        assert!(d.typed_active(), "newBuild: {:?}", d.typed_error);
        let views = [
            "config",
            "state",
            "outputs",
            "treeState",
            "skills",
            "items",
            "treeSpecs",
        ];
        compare_views(d, &views, "newBuild", &mut out);
        compare_saves(d, "new build", &mut out);
        for req in [
            json!({"cmd": "control", "path": "build.classDrop", "sel": 3}),
            json!({"cmd": "control", "path": "build.ascendDrop", "sel": 2}),
            json!({"cmd": "undo"}),
            json!({"cmd": "undo"}),
            json!({"cmd": "undo"}),
        ] {
            answer(d.request(&req.to_string()), "new build edit");
            compare_views(d, &CONFIG_VIEWS, &format!("new build {req}"), &mut out);
        }
    }
    if !out.is_empty() {
        let n = out.len();
        out.truncate(8);
        failures.push(format!("newBuild ({n}):\n  {}", out.join("\n  ")));
    }
    eprintln!(
        "{steps} edits: typed {:.1?}, translated {:.1?} (with its replays), {popup_tooltips} mastery tooltips compared with a popup open\nskipped:\n{}",
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

/// This process's shard of the corpus: the `k`-th of every `n` builds
static SHARD: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();

/// The builds of this process's shard
fn mine<T>(builds: Vec<T>) -> Vec<T> {
    let (k, n) = SHARD.get().copied().unwrap_or((0, 1));
    builds
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % n == k)
        .map(|(_, b)| b)
        .collect()
}

/// Random sequences of item, set, spec, config set, level and socket
/// group copy edits, items added, edited and saved from the editor give
/// the translation's items, skills, sidebar, outputs and specs after each
/// step, and its file at the end
fn items_edits_match(d: &mut Dispatcher) {
    let (mut failures, mut steps, mut times) = (Vec::new(), 0, [Duration::ZERO; 2]);
    let mut skipped = Vec::new();
    let mut popups = 0;
    let mut popup_inputs: std::collections::BTreeMap<String, usize> = Default::default();
    let steps_per_build: usize = std::env::var("SESSION_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    // Item texts of the builds so far, for adding and editing items
    let mut pool: Vec<J> = Vec::new();
    let mut db_pool: Vec<J> = Vec::new();
    // Forbidden Flame and Flesh, which bring their counterpart along
    let mut forbidden: Vec<J> = Vec::new();
    for (id, path) in mine(corpus()) {
        at_build(d, &id);
        let xml = std::fs::read_to_string(&path).unwrap();
        // The edits since the load, for a fresh translation's recheck
        let mut log = vec![json!({"cmd": "loadXml", "xml": xml}).to_string()];
        answer(d.request(&log[0]), "load");
        if !d.typed_active() {
            continue;
        }
        let mut rng = Lcg(seed(&id) + 11);
        let mut out = Vec::new();
        let compare = |d: &mut Dispatcher, req: &J, label: &str, out: &mut Vec<String>| {
            let r = req.to_string();
            let typed = answer(d.request(&r), "typed");
            let tr = answer(d.translated_request(&r), "translated");
            diff(&typed, &tr, &format!("{label} {r}"), out);
            typed
        };
        if db_pool.is_empty() {
            // The item databases, once: their uniques and templates also
            // go in the pool, the Forbidden jewels always
            for kind in ["unique", "rare"] {
                let db = compare(
                    d,
                    &json!({"cmd": "itemDb", "kind": kind}),
                    "itemDb",
                    &mut out,
                );
                for (i, item) in db["ok"].as_array().into_iter().flatten().enumerate() {
                    let name = item["name"].as_str().unwrap_or("");
                    if name.starts_with("Forbidden Fl") {
                        forbidden.push(item["raw"].clone());
                    } else if i % 37 == 0 {
                        db_pool.push(item["raw"].clone());
                    }
                }
            }
        }
        if seed(&id).is_multiple_of(3) {
            // The active item set shared, the shared set (of several
            // items) dragged back into the build, and both lists reordered
            for r in [
                json!({"cmd": "control", "path": "items.setManage", "click": true}),
                json!({"cmd": "control", "path": "popup.setList", "sel": 1, "dragTo": "popup.sharedList"}),
                json!({"cmd": "control", "path": "popup.sharedList", "sel": 1, "dragTo": "popup.setList"}),
                json!({"cmd": "control", "path": "popup.setList", "sel": 1, "moveTo": 3}),
                json!({"cmd": "control", "path": "popup.sharedList", "sel": 1, "moveTo": 3}),
                json!({"cmd": "closePopup"}),
            ] {
                log.push(r.to_string());
                answer(d.request(&r.to_string()), "shared sets");
                assert!(d.typed_active(), "{id} {r}: {:?}", d.typed_error);
                compare_views(
                    d,
                    &["state", "items"],
                    &format!("shared sets {r}"),
                    &mut out,
                );
            }
        }
        let mut items = compare(d, &json!({"cmd": "items"}), "start", &mut out);
        if let Some(a) = items["ok"]["items"].as_array() {
            pool.extend(a.iter().map(|i| i["raw"].clone()));
        }
        for step in 0..steps_per_build {
            if !out.is_empty() {
                break;
            }
            let pick = |rng: &mut Lcg, a: &J| -> Option<J> {
                let a = a.as_array()?;
                (!a.is_empty()).then(|| a[rng.below(a.len())].clone())
            };
            let slots: Vec<J> = items["ok"]["slots"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s["shown"] == true)
                .collect();
            let specs = answer(d.request(r#"{"cmd":"treeSpecs"}"#), "treeSpecs");
            let n_specs = specs["ok"]["list"].as_array().map_or(1, Vec::len);
            let groups = answer(d.request(r#"{"cmd":"skills"}"#), "skills")["ok"]["groups"]
                .as_array()
                .map_or(0, Vec::len);
            let req = match rng.below(14) {
                0 | 1 if !slots.is_empty() => {
                    let slot = &slots[rng.below(slots.len())];
                    let item = pick(&mut rng, &slot["candidates"])
                        .filter(|_| rng.below(4) != 0)
                        .unwrap_or(json!(0));
                    json!({"cmd": "equipItem", "slot": slot["name"], "id": item})
                }
                2 => match pick(&mut rng, &items["ok"]["items"]) {
                    Some(i) => json!({"cmd": "deleteItem", "id": i["id"]}),
                    None => continue,
                },
                3 => match pick(&mut rng, &items["ok"]["itemSets"]) {
                    Some(s) => json!({"cmd": "selectItemSet", "id": s["id"]}),
                    None => continue,
                },
                4 => json!({"cmd": "selectTreeSpec", "index": rng.below(n_specs) + 1}),
                5 => json!({"cmd": "selectConfigSet", "id": rng.below(2) + 1}),
                6 if groups > 0 => {
                    let c = compare(
                        d,
                        &json!({"cmd": "copySocketGroup", "index": rng.below(groups) + 1}),
                        &format!("step {step}"),
                        &mut out,
                    );
                    match c["ok"]["text"].as_str() {
                        Some(t) => json!({"cmd": "pasteSocketGroup", "text": t}),
                        None => continue,
                    }
                }
                8 => {
                    let text = if rng.below(3) == 0 && !db_pool.is_empty() {
                        &db_pool[rng.below(db_pool.len())]
                    } else if rng.below(4) == 0 && !forbidden.is_empty() {
                        &forbidden[rng.below(forbidden.len())]
                    } else {
                        &pool[rng.below(pool.len())]
                    };
                    match pick(&mut rng, &J::Array(slots.clone())).filter(|_| rng.below(2) == 0) {
                        Some(slot) => json!({"cmd": "addItem", "text": text, "slot": slot["name"]}),
                        None => json!({"cmd": "addItem", "text": text}),
                    }
                }
                9 => match pick(&mut rng, &items["ok"]["items"]) {
                    Some(i) => {
                        let text = &pool[rng.below(pool.len())];
                        json!({"cmd": "editItem", "id": i["id"], "text": text})
                    }
                    None => continue,
                },
                10 | 11 => {
                    let open = match pick(&mut rng, &items["ok"]["items"]) {
                        Some(i) if rng.below(2) == 0 => {
                            json!({"cmd": "setDisplayItem", "id": i["id"]})
                        }
                        _ => json!({"cmd": "setDisplayItem", "text": pool[rng.below(pool.len())]}),
                    };
                    compare(d, &open, &format!("step {step}"), &mut out);
                    log.push(open.to_string());
                    let label = format!("step {step} {open}");
                    compare_display(d, &label, &log, &mut out);
                    json!({"cmd": "saveDisplayItem"})
                }
                13 => {
                    // The item editor: an item opened, random changes to
                    // its controls, then saved
                    let open = match pick(&mut rng, &items["ok"]["items"]) {
                        Some(i) if rng.below(2) == 0 => {
                            json!({"cmd": "setDisplayItem", "id": i["id"]})
                        }
                        _ => json!({"cmd": "setDisplayItem", "text": pool[rng.below(pool.len())]}),
                    };
                    answer(d.request(&open.to_string()), "open");
                    log.push(open.to_string());
                    let mut fell_back = !d.typed_active();
                    for _ in 0..rng.below(4) + 1 {
                        if fell_back {
                            break;
                        }
                        // Shown on both, as the editor is drawn before a click:
                        // upstream's sliders take their values when drawn
                        let shown =
                            compare_display(d, &format!("step {step} {open}"), &log, &mut out);
                        let controls: Vec<J> = shown["ok"]["controls"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|c| {
                                let p = c["path"].as_str().unwrap_or("");
                                c["enabled"] == true
                                    && c["class"] != "LabelControl"
                                    && (c["class"] != "ButtonControl"
                                        || p.contains("AddSocket")
                                        || p.contains("Enchant")
                                        || p.contains("Anoint")
                                        || p.contains("Corrupt")
                                        || p.contains("AddImplicit")
                                        || p.contains("AddCustom")
                                        || p.contains("AddCrucible")
                                        || p.contains("CustomModifierRemove"))
                            })
                            .collect();
                        // The popups more often than the other controls
                        let opens = |p: &str| {
                            p.contains("Enchant")
                                || p.contains("Anoint")
                                || p.contains("Corrupt")
                                || p.contains("AddImplicit")
                                || p.contains("AddCustom")
                                || p.contains("AddCrucible")
                        };
                        let openers: Vec<J> = controls
                            .iter()
                            .filter(|c| c["path"].as_str().is_some_and(opens))
                            .cloned()
                            .collect();
                        let popup =
                            pick(&mut rng, &J::Array(openers)).filter(|_| rng.below(3) == 0);
                        // The modifier sorting now and then too
                        let sorting = controls
                            .iter()
                            .find(|c| c["path"] == "items.craftingSorting")
                            .cloned()
                            .filter(|_| rng.below(6) == 0);
                        let Some(c) = popup
                            .or(sorting)
                            .or_else(|| pick(&mut rng, &J::Array(controls)))
                        else {
                            break;
                        };
                        let path = c["path"].clone();
                        let r = match c["class"].as_str().unwrap_or("") {
                            "DropDownControl" => {
                                let n = c["list"].as_array().map_or(1, Vec::len);
                                json!({"cmd": "control", "path": path, "sel": rng.below(n) + 1})
                            }
                            "EditControl" => {
                                let text = ["0", "20", "7", "", "28"][rng.below(5)];
                                json!({"cmd": "control", "path": path, "text": text})
                            }
                            "CheckBoxControl" => {
                                json!({"cmd": "control", "path": path, "state": c["state"] != true})
                            }
                            "SliderControl" => {
                                let val = rng.below(11) as f64 / 10.0;
                                json!({"cmd": "control", "path": path, "val": val})
                            }
                            _ => json!({"cmd": "control", "path": path, "click": true}),
                        }
                        .to_string();
                        if r.contains("craftingSorting") {
                            *popup_inputs
                                .entry("items.craftingSorting".into())
                                .or_default() += 1;
                        }
                        answer(d.request(&r), "editor control");
                        log.push(r.clone());
                        if !d.typed_active() {
                            fell_back = true;
                            break;
                        }
                        let label = format!("step {step} {r}");
                        if r.contains("click") && opens(&r) {
                            // The popup: random choices, then one of the
                            // buttons that close it
                            popups += 1;
                            compare(d, &json!({"cmd": "state"}), &label, &mut out);
                            let mut closed = false;
                            for _ in 0..rng.below(6) {
                                let st = answer(d.request(r#"{"cmd":"state"}"#), "state");
                                let popup = &st["ok"]["popups"][0];
                                let mut inputs: Vec<J> = popup["controls"]
                                    .as_array()
                                    .cloned()
                                    .unwrap_or_default()
                                    .into_iter()
                                    // Sorting by power only now and then
                                    .filter(|c| c["path"] != "popup.sort" || rng.below(4) == 0)
                                    .collect();
                                for b in popup["buttons"].as_array().into_iter().flatten() {
                                    if b["name"] == "implicits" || b["name"] == "rolls" {
                                        inputs.push(json!({"class": "ButtonControl", "path": format!("popup.{}", b["name"].as_str().unwrap_or(""))}));
                                    }
                                }
                                let Some(c) = pick(&mut rng, &J::Array(inputs)) else {
                                    break;
                                };
                                let r = match c["class"].as_str().unwrap_or("") {
                                    "CheckBoxControl" => {
                                        json!({"cmd": "control", "path": c["path"], "state": c["state"] != true})
                                    }
                                    "SliderControl" => {
                                        let val = rng.below(11) as f64 / 10.0;
                                        json!({"cmd": "control", "path": c["path"], "val": val})
                                    }
                                    "ButtonControl" => {
                                        json!({"cmd": "control", "path": c["path"], "click": true})
                                    }
                                    "EditControl" => {
                                        let text = ["", "life", "(fire|cold) damage", "energy", "z"][rng.below(5)];
                                        json!({"cmd": "control", "path": c["path"], "text": text})
                                    }
                                    _ => {
                                        let n = c["list"].as_array().map_or(1, Vec::len);
                                        json!({"cmd": "control", "path": c["path"], "sel": rng.below(n) + 1})
                                    }
                                }
                                .to_string();
                                *popup_inputs
                                    .entry(c["path"].as_str().unwrap_or("").to_string())
                                    .or_default() += 1;
                                answer(d.request(&r), "popup control");
                                log.push(r.clone());
                                if !d.typed_active() {
                                    closed = true;
                                    break;
                                }
                                compare(
                                    d,
                                    &json!({"cmd": "state"}),
                                    &format!("step {step} {r}"),
                                    &mut out,
                                );
                            }
                            if closed {
                                fell_back = true;
                                break;
                            }
                            let st = answer(d.request(r#"{"cmd":"state"}"#), "state");
                            // Hover tooltips: an entry of an input, and the
                            // save button
                            let inputs = st["ok"]["popups"][0]["controls"].clone();
                            if let Some(c) = pick(&mut rng, &inputs).filter(|c| {
                                c["path"] != "popup.sort" && c["path"] != "popup.notableDB.sort"
                            }) {
                                let n = c["list"].as_array().map_or(1, Vec::len);
                                let req = json!({"cmd": "controlTooltip", "path": c["path"], "index": rng.below(n) + 1});
                                compare(d, &req, &format!("step {step}"), &mut out);
                                *popup_inputs.entry("tooltip".into()).or_default() += 1;
                            }
                            if rng.below(2) == 0 {
                                let req = json!({"cmd": "controlTooltip", "path": "popup.save"});
                                compare(d, &req, &format!("step {step}"), &mut out);
                            }
                            let mut closing: Vec<String> = st["ok"]["popups"][0]["buttons"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|b| b["name"].as_str().map(String::from))
                                .filter(|n| n == "save" || n == "remove" || n == "close")
                                .collect();
                            closing.push("save".into());
                            let button = &closing[rng.below(closing.len())];
                            let r = json!({"cmd": "popupButton", "name": button}).to_string();
                            answer(d.request(&r), "popup button");
                            log.push(r.clone());
                            if !d.typed_active() {
                                fell_back = true;
                                break;
                            }
                        }
                        compare_display(d, &label, &log, &mut out);
                        compare(d, &json!({"cmd": "state"}), &label, &mut out);
                    }
                    if fell_back {
                        json!({"cmd": "state"})
                    } else {
                        json!({"cmd": "saveDisplayItem"})
                    }
                }
                7 => {
                    // The item list: rows selected, clicked (Ctrl to equip,
                    // twice to edit), filtered, sorted and deleted, a
                    // confirmation answered
                    let paths = [
                        "items.itemList",
                        "items.itemList.loadoutFilter",
                        "items.itemList.sort",
                        "items.itemList.deleteUnused",
                        "items.itemList.deleteAll",
                        "items.itemList.delete",
                        "items.sharedItemList",
                        "items.sharedItemList.delete",
                    ];
                    let view = json!({"cmd": "controls", "paths": paths});
                    let mut fell_back = false;
                    for _ in 0..rng.below(4) + 1 {
                        let label = format!("step {step} item list");
                        let c = compare(d, &view, &label, &mut out);
                        let rows = c["ok"]["items.itemList"]["list"]
                            .as_array()
                            .map_or(0, Vec::len);
                        let filters = c["ok"]["items.itemList.loadoutFilter"]["list"]
                            .as_array()
                            .map_or(1, Vec::len);
                        if rows > 0 && rng.below(3) == 0 {
                            let index = rng.below(rows) + 1;
                            let req = json!({"cmd": "controlTooltip", "path": "items.itemList", "index": index});
                            compare(d, &req, &label, &mut out);
                            *popup_inputs
                                .entry("items.itemList tooltip".into())
                                .or_default() += 1;
                        }
                        let click =
                            |path: &str| json!({"cmd": "control", "path": path, "click": true});
                        let shared = c["ok"]["items.sharedItemList"]["list"]
                            .as_array()
                            .map_or(0, Vec::len);
                        let r = match rng.below(16) {
                            15 => {
                                // The tab's own controls: their descriptions,
                                // a set's tooltip, then one of them used
                                let paths = [
                                    "items.setSelect",
                                    "items.specSelect",
                                    "items.weaponSwap1",
                                    "items.weaponSwap2",
                                    "items.craftDisplayItem",
                                    "items.newDisplayItem",
                                    "items.setManage",
                                    "items.specButton",
                                ];
                                let c = compare(
                                    d,
                                    &json!({"cmd": "controls", "paths": paths}),
                                    &label,
                                    &mut out,
                                );
                                let sets = c["ok"]["items.setSelect"]["list"]
                                    .as_array()
                                    .map_or(1, Vec::len);
                                let tip = json!({"cmd": "controlTooltip", "path": "items.setSelect", "index": rng.below(sets) + 1});
                                compare(d, &tip, &label, &mut out);
                                let path = paths[rng.below(6)];
                                *popup_inputs.entry(path.into()).or_default() += 1;
                                match path {
                                    "items.setSelect" => {
                                        json!({"cmd": "control", "path": path, "sel": rng.below(sets + 1) + 1})
                                    }
                                    "items.specSelect" => {
                                        let n =
                                            c["ok"][path]["list"].as_array().map_or(1, Vec::len);
                                        json!({"cmd": "control", "path": path, "sel": rng.below(n + 1) + 1})
                                    }
                                    "items.craftDisplayItem" | "items.newDisplayItem" => {
                                        // A popup opens: driven, then closed
                                        let r =
                                            json!({"cmd": "control", "path": path, "click": true})
                                                .to_string();
                                        answer(d.request(&r), "items control");
                                        log.push(r);
                                        if !d.typed_active()
                                            || drive_popups(
                                                d,
                                                &mut rng,
                                                &label,
                                                &mut log,
                                                &mut out,
                                                &mut popup_inputs,
                                            )
                                        {
                                            fell_back = true;
                                            break;
                                        }
                                        continue;
                                    }
                                    _ => json!({"cmd": "control", "path": path, "click": true}),
                                }
                            }
                            13 if rng.below(3) == 0 => {
                                // A row of the item list or the shared item
                                // list dragged within its list
                                let (src, n) = if rng.below(2) == 0 {
                                    ("items.itemList", rows)
                                } else {
                                    ("items.sharedItemList", shared)
                                };
                                if n == 0 {
                                    continue;
                                }
                                *popup_inputs.entry(format!("move {src}")).or_default() += 1;
                                json!({"cmd": "control", "path": src, "sel": rng.below(n) + 1, "moveTo": rng.below(n + 1) + 1})
                            }
                            13 => {
                                // A row dragged between the item list, the
                                // databases and the shared item list
                                let (src, n, dst) = match rng.below(4) {
                                    0 => ("items.itemList", rows, "items.sharedItemList"),
                                    1 => ("items.sharedItemList", shared, "items.itemList"),
                                    k => {
                                        let db = if rng.below(2) == 0 {
                                            "items.uniqueDB"
                                        } else {
                                            "items.rareDB"
                                        };
                                        let c = compare(
                                            d,
                                            &json!({"cmd": "controls", "paths": [db]}),
                                            &label,
                                            &mut out,
                                        );
                                        let n = c["ok"][db]["list"].as_array().map_or(0, Vec::len);
                                        (
                                            db,
                                            n,
                                            if k == 2 {
                                                "items.itemList"
                                            } else {
                                                "items.sharedItemList"
                                            },
                                        )
                                    }
                                };
                                if n == 0 {
                                    continue;
                                }
                                // Or onto a shown slot, or the minion drop-down
                                if rng.below(3) == 0 {
                                    let dst = match rng.below(6) {
                                        0 => "build.mainSkillMinion".to_string(),
                                        _ if !slots.is_empty() => format!(
                                            "slots.{}",
                                            slots[rng.below(slots.len())]["name"]
                                                .as_str()
                                                .unwrap_or("")
                                        ),
                                        _ => continue,
                                    };
                                    *popup_inputs
                                        .entry(format!("drag {src} {}", &dst[..5]))
                                        .or_default() += 1;
                                    json!({"cmd": "control", "path": src, "sel": rng.below(n) + 1, "dragTo": dst})
                                } else {
                                    let target_len = if dst == "items.itemList" {
                                        rows
                                    } else {
                                        shared
                                    };
                                    let mut r = json!({"cmd": "control", "path": src, "sel": rng.below(n) + 1, "dragTo": dst});
                                    if rng.below(2) == 0 {
                                        r["at"] = json!(rng.below(target_len + 1) + 1);
                                    }
                                    *popup_inputs
                                        .entry(format!("drag {src} {dst}"))
                                        .or_default() += 1;
                                    r
                                }
                            }
                            14 if shared > 0 => {
                                // The shared item list: a row's tooltip,
                                // selected, clicked twice or deleted
                                let index = rng.below(shared) + 1;
                                let req = json!({"cmd": "controlTooltip", "path": "items.sharedItemList", "index": index});
                                compare(d, &req, &label, &mut out);
                                *popup_inputs
                                    .entry("items.sharedItemList".into())
                                    .or_default() += 1;
                                match rng.below(4) {
                                    0 if c["ok"]["items.sharedItemList.delete"]["enabled"]
                                        == true =>
                                    {
                                        click("items.sharedItemList.delete")
                                    }
                                    1 => {
                                        json!({"cmd": "control", "path": "items.sharedItemList", "key": "DELETE"})
                                    }
                                    2 => {
                                        json!({"cmd": "control", "path": "items.sharedItemList", "sel": index, "click": true, "doubleClick": true})
                                    }
                                    _ => {
                                        json!({"cmd": "control", "path": "items.sharedItemList", "sel": index})
                                    }
                                }
                            }
                            12 => {
                                // Manage Item, Skill or Config Sets
                                let path = [
                                    "items.setManage",
                                    "skills.setManage",
                                    "configTab.setManage",
                                    "items.specButton",
                                ][rng.below(4)];
                                let r = json!({"cmd": "control", "path": path, "click": true})
                                    .to_string();
                                *popup_inputs.entry(path.into()).or_default() += 1;
                                answer(d.request(&r), "manage");
                                log.push(r);
                                if !d.typed_active()
                                    || drive_popups(
                                        d,
                                        &mut rng,
                                        &label,
                                        &mut log,
                                        &mut out,
                                        &mut popup_inputs,
                                    )
                                {
                                    fell_back = true;
                                    break;
                                }
                                continue;
                            }
                            10 | 11 => {
                                // A row of an item database
                                let db = if rng.below(2) == 0 {
                                    "items.uniqueDB"
                                } else {
                                    "items.rareDB"
                                };
                                let c = compare(
                                    d,
                                    &json!({"cmd": "controls", "paths": [db]}),
                                    &label,
                                    &mut out,
                                );
                                let n = c["ok"][db]["list"].as_array().map_or(0, Vec::len);
                                if n == 0 {
                                    continue;
                                }
                                let index = rng.below(n) + 1;
                                if rng.below(3) == 0 {
                                    let req = json!({"cmd": "controlTooltip", "path": db, "index": index});
                                    compare(d, &req, &label, &mut out);
                                    *popup_inputs.entry(format!("{db} tooltip")).or_default() += 1;
                                }
                                let mut r = json!({"cmd": "control", "path": db, "sel": index, "click": true});
                                if rng.below(2) == 0 {
                                    r["ctrl"] = json!(true);
                                    r["shift"] = json!(rng.below(2) == 0);
                                } else {
                                    r["doubleClick"] = json!(true);
                                }
                                r
                            }
                            0..=3 if rows > 0 => {
                                let mut r = json!({"cmd": "control", "path": "items.itemList", "sel": rng.below(rows) + 1});
                                match rng.below(4) {
                                    0 => {
                                        r["click"] = json!(true);
                                        r["doubleClick"] = json!(true);
                                    }
                                    1 => {
                                        r["click"] = json!(true);
                                        r["ctrl"] = json!(true);
                                        r["shift"] = json!(rng.below(2) == 0);
                                    }
                                    2 => r["click"] = json!(true),
                                    _ => {}
                                }
                                r
                            }
                            4 | 5 => {
                                json!({"cmd": "control", "path": "items.itemList.loadoutFilter", "sel": rng.below(filters) + 1})
                            }
                            6 => click("items.itemList.sort"),
                            7 => click("items.itemList.deleteUnused"),
                            8 => click("items.itemList.deleteAll"),
                            // A disabled button cannot be clicked
                            _ if c["ok"]["items.itemList.delete"]["enabled"] == true
                                && rng.below(2) == 0 =>
                            {
                                click("items.itemList.delete")
                            }
                            _ => {
                                json!({"cmd": "control", "path": "items.itemList", "key": "DELETE"})
                            }
                        };
                        let key = r["path"].as_str().unwrap_or("").to_string()
                            + if r["doubleClick"] == true {
                                " double"
                            } else if r["ctrl"] == true {
                                " ctrl"
                            } else {
                                ""
                            };
                        *popup_inputs.entry(key).or_default() += 1;
                        let r = r.to_string();
                        answer(d.request(&r), "item list");
                        log.push(r.clone());
                        if !d.typed_active() {
                            fell_back = true;
                            break;
                        }
                        let label = format!("step {step} {r}");
                        let st = compare(d, &json!({"cmd": "state"}), &label, &mut out);
                        let title = st["ok"]["popups"][0]["title"].as_str().unwrap_or("");
                        if title == "Delete Item" || title == "Delete All" {
                            let confirm =
                                rng.below(3) != 0 && (title == "Delete Item" || rng.below(2) == 0);
                            let r = if confirm {
                                json!({"cmd": "popupButton", "name": "confirm"})
                            } else {
                                json!({"cmd": "closePopup"})
                            }
                            .to_string();
                            *popup_inputs
                                .entry(format!("{title} confirm {confirm}"))
                                .or_default() += 1;
                            answer(d.request(&r), "confirmation");
                            log.push(r.clone());
                            if !d.typed_active() {
                                fell_back = true;
                                break;
                            }
                            compare(
                                d,
                                &json!({"cmd": "state"}),
                                &format!("step {step} {r}"),
                                &mut out,
                            );
                        }
                        compare_display(d, &label, &log, &mut out);
                    }
                    if !fell_back {
                        compare(d, &view, &format!("step {step} item list end"), &mut out);
                    }
                    json!({"cmd": "state"})
                }
                12 => {
                    // Craft Item: random choices, Create or Cancel, the
                    // text editor of a unique, then the item saved
                    let types = 70;
                    let mut reqs = vec![json!({"cmd": "craftItem"})];
                    for _ in 0..rng.below(4) {
                        reqs.push(match rng.below(4) {
                            0 => json!({"cmd": "control", "path": "popup.rarity", "sel": rng.below(5) + 1}),
                            1 => json!({"cmd": "control", "path": "popup.type", "sel": rng.below(types) + 1}),
                            2 => json!({"cmd": "control", "path": "popup.base", "sel": rng.below(12) + 1}),
                            _ => {
                                let text = ["", "Doom Crest", " "][rng.below(3)];
                                json!({"cmd": "control", "path": "popup.title", "text": text})
                            }
                        });
                    }
                    let cancel = rng.below(6) == 0;
                    reqs.push(json!({"cmd": "popupButton", "name": if cancel { "cancel" } else { "save" }}));
                    let mut fell_back = false;
                    for r in &reqs {
                        let r = r.to_string();
                        answer(d.request(&r), "craft step");
                        log.push(r.clone());
                        if !d.typed_active() {
                            fell_back = true;
                            break;
                        }
                        let label = format!("step {step} {r}");
                        compare(d, &json!({"cmd": "state"}), &label, &mut out);
                        compare_display(d, &label, &log, &mut out);
                    }
                    if fell_back || cancel {
                        json!({"cmd": "state"})
                    } else {
                        // The text editor a unique opens, saved or closed
                        let st = answer(d.request(r#"{"cmd":"state"}"#), "state");
                        if st["ok"]["popups"][0]["title"] == "Edit Item Text" {
                            let r = if rng.below(4) == 0 {
                                json!({"cmd": "closePopup"})
                            } else {
                                json!({"cmd": "popupButton", "name": "save"})
                            };
                            answer(d.request(&r.to_string()), "edit text");
                            log.push(r.to_string());
                        }
                        json!({"cmd": "saveDisplayItem"})
                    }
                }
                _ => {
                    let text = ["1", "50", "100", "150", "", "37"][rng.below(6)];
                    json!({"cmd": "control", "path": "build.characterLevel", "text": text})
                }
            };
            let label = format!("step {step}");
            let r = req.to_string();
            let t = Instant::now();
            answer(d.request(&r), "typed edit");
            log.push(r.clone());
            times[0] += t.elapsed();
            if !d.typed_active() {
                let e = d.typed_error.clone().unwrap_or_default();
                assert!(e.starts_with("not ported"), "{id}: {label} {r}: {e}");
                skipped.push(format!("{id}: {r}: {e}"));
                if e.contains("upstream fails") {
                    // Upstream's own assert leaves its session broken for
                    // the builds after: a fresh one for them
                    at_build(d, &id);
                    *d = Dispatcher::new(session(), Some(data()), Prefer::Typed);
                }
                break;
            }
            let t = Instant::now();
            answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
            times[1] += t.elapsed();
            for f in ["state", "outputs", "skills", "treeSpecs"] {
                compare(d, &json!({"cmd": f}), &format!("{label} {r}"), &mut out);
            }
            let paths = [
                "build.characterLevel",
                "build.classDrop",
                "build.ascendDrop",
            ];
            compare(
                d,
                &json!({"cmd": "controls", "paths": paths}),
                &format!("{label} {r}"),
                &mut out,
            );
            items = compare(
                d,
                &json!({"cmd": "items"}),
                &format!("{label} {r}"),
                &mut out,
            );
            steps += 1;
        }
        if out.is_empty() && d.typed_active() {
            compare_saves(d, "after the edits", &mut out);
        }
        if !out.is_empty() {
            let n = out.len();
            out.truncate(8);
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
    }
    eprintln!(
        "{steps} item and set edits ({popups} item editor popups, inputs {popup_inputs:?}): typed {:.1?}, translated {:.1?} (with its replays)\nskipped:\n{}",
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

/// `displayItem` of both. The tooltip's stat comparison of the
/// translation can carry another build's mutation of upstream's shared
/// data (`ProcessSocketGroup` clears a triggered granted skill's level
/// cost): lines that differ are asked again of a fresh translation, with
/// the edits of `log` since the load, and aligned when that one agrees
/// Random choices in the front popup and the ones it opens (a list's
/// rows clicked and keyed, enabled buttons, edits' text, a row's tooltip),
/// the state compared after each, then every popup closed; whether the
/// typed session fell back
fn drive_popups(
    d: &mut Dispatcher,
    rng: &mut Lcg,
    label: &str,
    log: &mut Vec<String>,
    out: &mut Vec<String>,
    counts: &mut std::collections::BTreeMap<String, usize>,
) -> bool {
    let cmp = |d: &mut Dispatcher, req: &J, out: &mut Vec<String>| {
        let r = req.to_string();
        let typed = answer(d.request(&r), "typed");
        let tr = answer(d.translated_request(&r), "translated");
        diff(&typed, &tr, &format!("{label} {r}"), out);
        typed
    };
    let choices = rng.below(8) + 1;
    for i in 0..choices + 6 {
        let st = cmp(d, &json!({"cmd": "state"}), out);
        let Some(popup) = st["ok"]["popups"].get(0).cloned() else {
            return false;
        };
        let title = popup["title"].as_str().unwrap_or("").to_string();
        let buttons: Vec<String> = popup["buttons"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["enabled"] == true)
            .filter_map(|b| b["name"].as_str().map(String::from))
            .collect();
        let r = if i >= choices {
            // Closing: Done or Cancel, else the popup closed
            match buttons.iter().find(|b| *b == "close" || *b == "cancel") {
                Some(b) => json!({"cmd": "popupButton", "name": b}),
                None => json!({"cmd": "closePopup"}),
            }
        } else {
            let mut inputs: Vec<J> = popup["controls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c["enabled"] == true)
                .collect();
            for b in &buttons {
                inputs.push(json!({"class": "PopupButton", "name": b}));
            }
            if title == "Manage Passive Trees" {
                // Import Tree, Export Tree and Done have no name
                for k in 2..=4 {
                    inputs.push(json!({"class": "ButtonControl", "path": format!("popup.{k}")}));
                }
            }
            if inputs.is_empty() {
                continue;
            }
            let c = &inputs[rng.below(inputs.len())];
            let path = c["path"].clone();
            let class = c["class"].as_str().unwrap_or("");
            let key = format!(
                "{title}: {}",
                c["path"].as_str().or(c["name"].as_str()).unwrap_or("")
            );
            *counts.entry(key).or_default() += 1;
            if class.ends_with("ListControl") {
                let n = c["list"].as_array().map_or(0, Vec::len);
                if n > 0 && rng.below(4) == 0 {
                    let req =
                        json!({"cmd": "controlTooltip", "path": path, "index": rng.below(n) + 1});
                    cmp(d, &req, out);
                }
                match rng.below(6) {
                    0 if c["sel"].is_number() => {
                        json!({"cmd": "control", "path": path, "key": "DELETE"})
                    }
                    1 if c["sel"].is_number() => {
                        json!({"cmd": "control", "path": path, "key": "F2"})
                    }
                    // Between the build's and the shared item sets
                    2 if n > 0 && (path == "popup.setList" || path == "popup.sharedList") => {
                        let dst = if path == "popup.setList" {
                            "popup.sharedList"
                        } else {
                            "popup.setList"
                        };
                        json!({"cmd": "control", "path": path, "sel": rng.below(n) + 1, "dragTo": dst})
                    }
                    // A row dragged within a mutable list
                    3 if n > 0
                        && matches!(
                            class,
                            "ItemSetListControl"
                                | "SkillSetListControl"
                                | "ConfigSetListControl"
                                | "PassiveSpecListControl"
                                | "SharedItemSetListControl"
                        ) =>
                    {
                        json!({"cmd": "control", "path": path, "sel": rng.below(n) + 1, "moveTo": rng.below(n + 1) + 1})
                    }
                    _ if n > 0 => {
                        let mut r = json!({"cmd": "control", "path": path, "sel": rng.below(n) + 1, "click": true});
                        if rng.below(2) == 0 {
                            r["doubleClick"] = json!(true);
                        }
                        r
                    }
                    _ => continue,
                }
            } else if class == "PopupButton" {
                json!({"cmd": "popupButton", "name": c["name"]})
            } else if class == "ButtonControl" {
                json!({"cmd": "control", "path": path, "click": true})
            } else if class == "EditControl" && title == "Import Tree" && path == "popup.edit" {
                json!({"cmd": "control", "path": path, "text": tree_link(d, rng)})
            } else if class == "EditControl" {
                let text = ["New set", "", " ", "Alt {1}", "Default", "Copy"][rng.below(6)];
                json!({"cmd": "control", "path": path, "text": text})
            } else if class == "CheckBoxControl" {
                json!({"cmd": "control", "path": path, "state": c["state"] != true})
            } else {
                let n = c["list"].as_array().map_or(1, Vec::len);
                json!({"cmd": "control", "path": path, "sel": rng.below(n) + 1})
            }
        };
        let r = r.to_string();
        answer(d.request(&r), "popup step");
        log.push(r);
        if !d.typed_active() {
            return true;
        }
    }
    // Whatever is still open
    for _ in 0..8 {
        let st = cmp(d, &json!({"cmd": "state"}), out);
        if st["ok"]["popups"].as_array().is_none_or(|p| p.is_empty()) {
            break;
        }
        let r = json!({"cmd": "closePopup"}).to_string();
        answer(d.request(&r), "close");
        log.push(r);
        if !d.typed_active() {
            return true;
        }
    }
    false
}

fn compare_display(d: &mut Dispatcher, label: &str, log: &[String], out: &mut Vec<String>) -> J {
    let r = json!({"cmd": "displayItem"}).to_string();
    let mut typed = answer(d.request(&r), "typed");
    let answer_typed = typed.clone();
    let mut tr = answer(d.translated_request(&r), "translated");
    if !typed["ok"].is_object() || !tr["ok"].is_object() {
        diff(&typed, &tr, &format!("{label} {r}"), out);
        return answer_typed;
    }
    let (tl, rl) = (typed["ok"]["lines"].take(), tr["ok"]["lines"].take());
    diff(&typed, &tr, &format!("{label} {r}"), out);
    if tl != rl {
        let fresh = session();
        for e in log {
            answer(fresh.request(e), "fresh edit");
        }
        let fl = answer(fresh.request(&r), "fresh")["ok"]["lines"].take();
        if fl == tl {
            note_fallback("displayItem", "aligned with a fresh translation");
        } else {
            diff(&tl, &rl, &format!("{label} {r}.lines"), out);
        }
    }
    answer_typed
}

/// `saveXml` of both on the dispatcher's build: where they differ
fn compare_saves(d: &mut Dispatcher, label: &str, out: &mut Vec<String>) {
    let mut typed = answer(d.request(r#"{"cmd":"saveXml"}"#), "typed save");
    let mut tr = answer(
        d.translated_request(r#"{"cmd":"saveXml"}"#),
        "translated save",
    );
    if oracle() {
        unordered(&mut typed);
        unordered(&mut tr);
    }
    let (a, b) = (typed["ok"]["xml"].as_str(), tr["ok"]["xml"].as_str());
    if a != b && std::env::var("DUMP_SAVES").is_ok() {
        eprintln!(
            "TYPED\n{}\nTRANSLATED\n{}",
            a.unwrap_or(""),
            b.unwrap_or("")
        );
    }
    if a != b {
        let (a, b) = (a.unwrap_or(""), b.unwrap_or(""));
        let line = a.lines().zip(b.lines()).position(|(x, y)| x != y);
        let at = |s: &str| {
            line.and_then(|l| s.lines().nth(l))
                .unwrap_or("")
                .to_string()
        };
        out.push(format!(
            "{label}: saveXml differs:\n  typed {:?}\n  translated {:?}",
            at(a),
            at(b)
        ));
    }
}

/// `saveXml` and `saveCode` give the translation's file, byte for byte;
/// a file saved by either loads in the other to the same file, sidebar
/// and outputs (design layer 8)
fn saves_match(d: &mut Dispatcher) {
    let (mut failures, mut times, mut n) = (Vec::new(), [Duration::ZERO; 2], 0);
    for (id, path) in mine(corpus()) {
        at_build(d, &id);
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let mut out = Vec::new();
        answer(
            d.translated_request(r#"{"cmd":"treeState"}"#),
            "translated load",
        );
        let t = Instant::now();
        let typed = answer(d.request(r#"{"cmd":"saveXml"}"#), "typed save");
        times[0] += t.elapsed();
        if !d.typed_active() {
            let e = d.typed_error.clone().unwrap_or_default();
            assert!(e.starts_with("not ported"), "{id}: saveXml: {e}");
            continue;
        }
        // The translation replays the typed save first
        answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
        let t = Instant::now();
        let tr = answer(
            d.translated_request(r#"{"cmd":"saveXml"}"#),
            "translated save",
        );
        times[1] += t.elapsed();
        diff(&typed, &tr, "saveXml", &mut out);
        let code = |d: &mut Dispatcher, typed: bool| {
            let r = r#"{"cmd":"saveCode"}"#;
            answer(
                if typed {
                    d.request(r)
                } else {
                    d.translated_request(r)
                },
                "saveCode",
            )
        };
        let (a, b) = (code(d, true), code(d, false));
        diff(&a, &b, "saveCode", &mut out);
        let typed_xml = typed["ok"]["xml"].as_str().unwrap().to_string();
        let tr_xml = tr["ok"]["xml"].as_str().unwrap().to_string();
        // The typed file in the translation
        let views: Vec<J> = ["state", "outputs"]
            .iter()
            .map(|c| answer(d.request(&json!({"cmd": c}).to_string()), c))
            .collect();
        let load = json!({"cmd": "loadXml", "xml": typed_xml}).to_string();
        answer(d.translated_request(&load), "translated load");
        for (c, v) in ["state", "outputs"].iter().zip(&views) {
            let tr = answer(d.translated_request(&json!({"cmd": c}).to_string()), c);
            diff(
                v,
                &tr,
                &format!("typed file in the translation: {c}"),
                &mut out,
            );
        }
        answer(d.request(&load), "typed load");
        compare_saves(d, "typed file", &mut out);
        // The translated file in the typed engine
        let load = json!({"cmd": "loadXml", "xml": tr_xml}).to_string();
        answer(d.request(&load), "typed load");
        compare_saves(d, "translated file", &mut out);
        for c in ["state", "outputs"] {
            let t = answer(d.request(&json!({"cmd": c}).to_string()), c);
            let tr = answer(d.translated_request(&json!({"cmd": c}).to_string()), c);
            diff(&t, &tr, &format!("translated file: {c}"), &mut out);
        }
        n += 1;
        if !out.is_empty() {
            out.truncate(6);
            failures.push(format!("{id}:\n  {}", out.join("\n  ")));
        }
    }
    eprintln!(
        "{n} builds saved: typed saveXml {:.1?}, translated {:.1?}",
        times[0], times[1]
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A build file with each `<Spec>`'s nodes and masteries as a tree link,
/// as `PassiveSpec:EncodeURL` writes it
fn specs_as_urls(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<Spec ") {
        out.push_str(&rest[..at]);
        let end = at + rest[at..].find('>').unwrap();
        let closed = rest[..end].ends_with('/');
        let tag = &rest[at + 6..if closed { end - 1 } else { end }];
        let mut attrs = Vec::new();
        let mut a = std::collections::HashMap::new();
        for part in tag.split('"').collect::<Vec<_>>().chunks(2) {
            if let [k, v] = part {
                let k = k.trim().trim_end_matches('=').to_string();
                a.insert(k.clone(), v.to_string());
                attrs.push((k, v.to_string()));
            }
        }
        let num = |k: &str| a.get(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let mut ids: Vec<u32> = a["nodes"]
            .split(',')
            .filter_map(|n| n.parse().ok())
            .collect();
        ids.sort();
        let masteries: std::collections::HashMap<u32, u32> = a
            .get("masteryEffects")
            .map(|m| {
                m.split('}')
                    .filter_map(|e| {
                        let (n, e) = e.trim_start_matches([',', '{']).split_once(',')?;
                        Some((n.parse().ok()?, e.parse().ok()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut b: Vec<u8> = vec![0, 0, 0, 6, num("classId") as u8];
        b.push((num("secondaryAscendClassId") << 2 | num("ascendClassId")) as u8);
        let (mut nodes, mut clusters, mut mastery) = (Vec::new(), Vec::new(), Vec::new());
        for id in ids {
            if id < 65536 && nodes.len() < 255 * 2 {
                nodes.extend([(id / 256) as u8, (id % 256) as u8]);
                if let Some(e) = masteries.get(&id) {
                    mastery.extend([(e / 256) as u8, (e % 256) as u8]);
                    mastery.extend([(id / 256) as u8, (id % 256) as u8]);
                }
            } else if id >= 65536 {
                let c = id - 65536;
                clusters.extend([(c / 256) as u8, (c % 256) as u8]);
            }
        }
        b.push((nodes.len() / 2) as u8);
        b.extend(nodes);
        b.push((clusters.len() / 2) as u8);
        b.extend(clusters);
        b.push((mastery.len() / 4) as u8);
        b.extend(mastery);
        let url = base64(&b).replace('+', "-").replace('/', "_");
        out.push_str("<Spec");
        for (k, v) in attrs {
            if k != "nodes" && k != "masteryEffects" {
                out.push_str(&format!(" {k}=\"{v}\""));
            }
        }
        out.push_str(&format!(
            "><URL>https://www.pathofexile.com/passive-skill-tree/{url}</URL>"
        ));
        if closed {
            out.push_str("</Spec>");
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Standard base64 with padding
/// A tree link for Import Tree: the build's allocated nodes under another
/// class, on one of the sites and versions the popup reads, or a broken one
fn tree_link(d: &mut Dispatcher, rng: &mut Lcg) -> String {
    let st = answer(d.request(r#"{"cmd":"treeState"}"#), "treeState");
    let nodes: Vec<u64> = st["ok"]["alloc"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_u64)
        .filter(|n| *n < 65536)
        .take(200)
        .collect();
    // The selected masteries of those nodes: (node, effect)
    let masteries: Vec<(u64, u64)> = st["ok"]["masteries"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_u64()?)))
        .filter(|(n, _)| nodes.contains(n))
        .collect();
    let class = rng.below(7) as u8;
    let ascend = rng.below(4) as u8;
    match rng.below(10) {
        // A link PoEURL would resolve
        0 => return "http://poeurl.com/dABz".into(),
        // poeplanner's: little endian, its own version numbers
        1 | 2 => {
            let version: u16 = [51, 47, 33, 99][rng.below(4)];
            let mut b: Vec<u8> = vec![0, 0, 0, 0, 0, 0];
            b.extend(version.to_le_bytes());
            b.extend([class, rng.below(3) as u8, 0]);
            b.extend((nodes.len() as u16).to_le_bytes());
            for n in &nodes {
                b.extend((*n as u16).to_le_bytes());
            }
            b.extend([0, 0, 0, 0]);
            // Masteries on the latest tree only: an old one does not have
            // their nodes, which upstream fails on
            let ms: &[(u64, u64)] = if version == 51 { &masteries } else { &[] };
            b.extend((ms.len() as u16).to_le_bytes());
            for (n, e) in ms {
                b.extend((*n as u16).to_le_bytes());
                b.extend((*e as u16).to_le_bytes());
            }
            if rng.below(8) == 0 {
                b.truncate(10);
            }
            let code = base64(&b).replace('+', "-").replace('/', "_");
            return format!("https://poeplanner.com/{code}");
        }
        _ => {}
    }
    let (version, class) = match rng.below(8) {
        0 => (7, 1),
        1 => (6, 9),
        k => (if k == 2 { 4 } else { 6 }, class),
    };
    let k = rng.below(6);
    let prefix = [
        "https://www.pathofexile.com/passive-skill-tree/",
        "https://www.pathofexile.com/passive-skill-tree/3.15.0/",
        "https://www.pathofexile.com/fullscreen-passive-skill-tree/3.25.0/",
        "https://www.pathofexile.com/passive-skill-tree/2.6.2/",
        "https://poeskilltree.com/?v=3.20.0#",
        "https://www.pathofexile.com/passive-skill-tree/ruthless/",
    ][k];
    let mut b: Vec<u8> = vec![0, 0, 0, version, class, ascend];
    b.push(nodes.len() as u8);
    for n in &nodes {
        b.push((n / 256) as u8);
        b.push((n % 256) as u8);
    }
    b.push(0);
    // Masteries on the latest tree only (the first and the 2.6.2 links)
    let ms: &[(u64, u64)] = if k == 0 || k == 3 { &masteries } else { &[] };
    b.push(ms.len() as u8);
    for (n, e) in ms {
        b.extend([
            (e / 256) as u8,
            (e % 256) as u8,
            (n / 256) as u8,
            (n % 256) as u8,
        ]);
    }
    let code = base64(&b).replace('+', "-").replace('/', "_");
    if rng.below(8) == 0 {
        return format!("{prefix}AAAA");
    }
    format!("{prefix}{code}")
}

/// The active spec as a tree link of the latest tree, with its class and
/// masteries: the official format, or poeplanner's
fn own_tree_link(d: &mut Dispatcher, poe_planner: bool) -> String {
    let st = answer(d.request(r#"{"cmd":"treeState"}"#), "treeState");
    let nodes: Vec<u64> = st["ok"]["alloc"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_u64)
        .filter(|n| *n < 65536)
        .take(250)
        .collect();
    let masteries: Vec<(u64, u64)> = st["ok"]["masteries"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_u64()?)))
        .filter(|(n, _)| nodes.contains(n))
        .collect();
    let class = st["ok"]["classId"].as_u64().unwrap_or(0) as u8;
    let ascend = st["ok"]["ascendClassId"].as_u64().unwrap_or(0) as u8;
    let mut b: Vec<u8> = Vec::new();
    if poe_planner {
        b.extend([0, 0, 0, 0, 0, 0, 51, 0, class, ascend, 0]);
        b.extend((nodes.len() as u16).to_le_bytes());
        for n in &nodes {
            b.extend((*n as u16).to_le_bytes());
        }
        b.extend([0, 0, 0, 0]);
        b.extend((masteries.len() as u16).to_le_bytes());
        for (n, e) in &masteries {
            b.extend((*n as u16).to_le_bytes());
            b.extend((*e as u16).to_le_bytes());
        }
    } else {
        b.extend([0, 0, 0, 6, class, ascend, nodes.len() as u8]);
        for n in &nodes {
            b.extend([(n / 256) as u8, (n % 256) as u8]);
        }
        b.push(0);
        b.push(masteries.len() as u8);
        for (n, e) in &masteries {
            b.extend([
                (e / 256) as u8,
                (e % 256) as u8,
                (n / 256) as u8,
                (n % 256) as u8,
            ]);
        }
    }
    let code = base64(&b).replace('+', "-").replace('/', "_");
    if poe_planner {
        format!("https://poeplanner.com/{code}")
    } else {
        format!("https://www.pathofexile.com/passive-skill-tree/{code}")
    }
}

fn base64(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &x)| n | u32::from(x) << (16 - 8 * i));
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// `loadXml`, `loadCode`, `state`, `outputs` and `treeState`
fn commands_match(data: &'static TypedData, d: &mut Dispatcher) {
    let (mut failures, mut skipped, mut compared) = (Vec::new(), Vec::new(), 0);
    let mut times = [Duration::ZERO; 4];
    let mut builds: Vec<(String, String)> = corpus()
        .into_iter()
        .map(|(id, path)| {
            let xml = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            (id, xml)
        })
        .collect();
    // The character level's auto mode, on every tenth build
    let auto: Vec<(String, String)> = builds
        .iter()
        .step_by(10)
        .filter_map(|(id, xml)| {
            let at = xml.find("characterLevelAutoMode=\"")?;
            let end = at + xml[at..].find("\" ")? + 1;
            let v = format!(
                "{}characterLevelAutoMode=\"true\"{}",
                &xml[..at],
                &xml[end..]
            );
            Some((format!("{id}-auto"), v))
        })
        .collect();
    builds.extend(auto);
    // The specs as tree links (`<URL>`, the old format), on every tenth
    // build from the fifth
    let urls: Vec<(String, String)> = builds
        .iter()
        .skip(4)
        .step_by(10)
        .map(|(id, xml)| (format!("{id}-url"), specs_as_urls(xml)))
        .collect();
    builds.extend(urls);
    let builds = mine(builds);
    for (id, xml) in builds {
        at_build(d, &id);
        if let Err(e) = TypedSession::load(data, &xml, None) {
            skipped.push(format!("{id}: {e}"));
            note_fallback("loadXml", &e);
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
        for cmd in ["state", "outputs", "treeState"] {
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
        for (cmd, typed) in ["state", "outputs", "treeState"].iter().zip(typed) {
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

/// `tree` (the geometry: nodes, cluster jewel subgraphs, connectors,
/// classes) gives the translation's for every corpus build, and for an
/// empty build on each tree version
fn geometry_matches(d: &mut Dispatcher) {
    let mut builds: Vec<(String, String)> = corpus()
        .into_iter()
        .map(|(id, path)| (id, std::fs::read_to_string(&path).unwrap()))
        .collect();
    let src = gamedata::Source::load(&src()).unwrap_or_else(|e| panic!("{e}"));
    if std::env::var("SESSION_FILTER").is_err() {
        for v in src.tree_versions().unwrap_or_else(|e| panic!("{e}")) {
            let xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<PathOfBuilding>
	<Build level="1" targetVersion="3_0" className="Scion" ascendClassName="None" mainSocketGroup="1" viewMode="TREE" characterLevelAutoMode="false" bandit="None" pantheonMajorGod="None" pantheonMinorGod="None"/>
	<Tree activeSpec="1">
		<Spec treeVersion="{v}" classId="0" ascendClassId="0" nodes=""/>
	</Tree>
	<Skills activeSkillSet="1"><SkillSet id="1"/></Skills>
	<Items activeItemSet="1"><ItemSet id="1"/></Items>
	<Config/>
</PathOfBuilding>"#
            );
            builds.push((format!("empty {v}"), xml));
        }
    }
    let builds = mine(builds);
    let (mut failures, mut compared, mut skipped) = (Vec::new(), 0, Vec::new());
    let mut times = [Duration::ZERO; 2];
    for (id, xml) in builds {
        at_build(d, &id);
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            skipped.push(format!("{id}: {:?}", d.typed_error));
            continue;
        }
        let t = Instant::now();
        let typed = answer(d.request(r#"{"cmd":"tree"}"#), "typed tree");
        times[0] += t.elapsed();
        assert!(d.typed_active(), "{id}: {:?}", d.typed_error);
        let t = Instant::now();
        let tr = answer(d.translated_request(r#"{"cmd":"tree"}"#), "translated tree");
        times[1] += t.elapsed();
        assert!(tr.get("ok").is_some(), "{id}: {tr}");
        let mut out = Vec::new();
        diff(&typed, &tr, "tree", &mut out);
        if !out.is_empty() {
            let n = out.len();
            out.truncate(8);
            for l in &mut out {
                if let Some((at, _)) = l.char_indices().nth(400) {
                    l.truncate(at);
                }
            }
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
        compared += 1;
    }
    eprintln!(
        "{compared} trees: typed {:.1?}, translated {:.1?} (with its loads)\nskipped:\n{}",
        times[0],
        times[1],
        skipped.join("\n")
    );
    assert!(
        skipped.is_empty() && failures.is_empty(),
        "{} skipped, {} differ:\n{}",
        skipped.len(),
        failures.len(),
        failures.join("\n")
    );
}

/// `<Import exportParty="true"/>`: the build exports its buffs
fn with_export(xml: &str) -> String {
    match xml.find("exportParty=\"false\"") {
        Some(at) => format!(
            "{}exportParty=\"true\"{}",
            &xml[..at],
            &xml[at + "exportParty=\"false\"".len()..]
        ),
        None if xml.contains("<Import/>") => {
            xml.replace("<Import/>", "<Import exportParty=\"true\"/>")
        }
        None if xml.contains("<Import ") => {
            xml.replacen("<Import ", "<Import exportParty=\"true\" ", 1)
        }
        None => xml.replace(
            "</PathOfBuilding>",
            "<Import exportParty=\"true\"/>\n</PathOfBuilding>",
        ),
    }
}

/// The build with these socket groups added after its others, each gem
/// of a group at its natural maximum level and 20% quality
fn with_groups(data: &TypedData, xml: &str, groups: &[&[&str]]) -> Option<String> {
    let mut add = String::new();
    for names in groups {
        add.push_str("<Skill enabled=\"true\" label=\"Party\" includeInFullDPS=\"false\">\n");
        for name in *names {
            let gem = data
                .game
                .skills
                .gems()
                .iter()
                .find(|g| g.name == *name)
                .unwrap_or_else(|| panic!("no gem {name}"));
            add.push_str(&format!(
                "<Gem count=\"1\" enableGlobal1=\"true\" enableGlobal2=\"true\" enabled=\"true\" gemId=\"{}\" level=\"{}\" nameSpec=\"{}\" quality=\"20\" skillId=\"{}\" variantId=\"{}\"/>\n",
                gem.game_id, gem.natural_max_level, gem.name, gem.id, gem.variant_id
            ));
        }
        add.push_str("</Skill>\n");
    }
    let at = xml.find("</SkillSet>")?;
    Some(format!("{}{add}{}", &xml[..at], &xml[at..]))
}

/// XML text content
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The `ExportedBuffs` of a saved build, by name
fn exported(xml: &str) -> Vec<(String, String)> {
    let root = engine::xml::parse(xml).unwrap();
    let party = root.child("PathOfBuilding").and_then(|p| p.child("Party"));
    party
        .iter()
        .flat_map(|p| p.elements())
        .filter(|e| e.elem == "ExportedBuffs")
        .filter_map(|e| {
            let text = e.children.iter().find_map(|c| match c {
                engine::xml::Node::Text(t) => Some(t.clone()),
                _ => None,
            })?;
            Some((e.attr("name")?.to_string(), text))
        })
        .collect()
}

/// The build with these party buffs imported (its Party tab replaced)
fn with_party(xml: &str, buffs: &[(String, String)]) -> String {
    let mut party =
        String::from("<Party ShowAdvanceTools=\"false\" append=\"false\" destination=\"All\">\n");
    for (name, text) in buffs {
        party.push_str(&format!(
            "<ImportedBuffs name=\"{name}\">{}</ImportedBuffs>\n",
            escape(text)
        ));
    }
    party.push_str("</Party>");
    let start = xml.find("<Party ");
    let end = start.and_then(|s| {
        let rest = &xml[s..];
        let close = rest.find('>')?;
        if rest[..close].ends_with('/') {
            Some(s + close + 1)
        } else {
            rest.find("</Party>").map(|e| s + e + "</Party>".len())
        }
    });
    match (start, end) {
        (Some(s), Some(e)) => format!("{}{party}{}", &xml[..s], &xml[e..]),
        _ => xml.replace("</PathOfBuilding>", &format!("{party}\n</PathOfBuilding>")),
    }
}

/// Compares saved files line by line. Upstream's `modLib.mergeKeystones`
/// gives a keystone's shared mods the source of whatever else grants it,
/// also the copies the tree added (see `align_keystone_sources`): an
/// exported mod line that differs only by the typed source being the
/// tree's is aligned. The number of lines so aligned.
fn compare_party_saves(typed: &str, tr: &str, out: &mut Vec<String>) -> usize {
    let (mut typed, mut tr) = (J::String(typed.into()), J::String(tr.into()));
    if oracle() {
        unordered(&mut typed);
        unordered(&mut tr);
    }
    let (typed, tr) = (typed.as_str().unwrap(), tr.as_str().unwrap());
    let mut aligned = 0;
    if typed.lines().count() != tr.lines().count() {
        out.push("saveXml differs in its number of lines".into());
        return 0;
    }
    for (a, b) in typed.lines().zip(tr.lines()) {
        if a == b {
            continue;
        }
        let (x, y): (Vec<&str>, Vec<&str>) = (a.split('|').collect(), b.split('|').collect());
        let keystone = x.len() == y.len()
            && x.len() > 2
            && x[1].starts_with("Tree:")
            && x.iter()
                .zip(&y)
                .enumerate()
                .all(|(i, (u, v))| i == 1 || u == v);
        if keystone {
            aligned += 1;
        } else {
            out.push(format!(
                "saveXml differs:\n  typed {a:?}\n  translated {b:?}"
            ));
            return aligned;
        }
    }
    aligned
}

/// Party members' stats as a support build exports them, with every flag
/// the calculations read of party members
const PARTY_STATS: &str = "EnemyFireResistEqualToYours
FireResist=40
EnemyChaosResistEqualToYours
ChaosResist=-30
PartyMemberMaximumEnduranceChargesEqualToYours
EnduranceChargesMax=6
PartyMemberFortifyEqualToYours
FortificationStacks=20
MovementSpeedMod|percent|max=150
Life=5000
BlockChanceMax=80
BlockChance=55
MaxLifeLeechRatePercent=25
EnergyShieldRecoveryCap=3000
MainHand.CritChance=12.5";

/// Every link skill's mods as a support build exports them
const PARTY_LINKS: &str = "Destructive Link
130
1|Skill:DestructiveLink|MainHandCritIsEqualToParent|FLAG|-|-|type=GlobalEffect/effectType=Link
---
Protective Link
100
1|Skill:ProtectiveLink|MaximumBlockAttackChanceIsEqualToParent|FLAG|-|-|type=GlobalEffect/effectType=Link
1|Skill:ProtectiveLink|BlockAttackChanceIsEqualToParent|FLAG|-|-|type=GlobalEffect/effectType=Link
---
Soul Link
110
30|Skill:SoulLink|TakenFromParentESBeforeYou|BASE|-|-|type=GlobalEffect/effectType=Link
---
Vampiric Link
100
1|Skill:VampiricLink|MaximumLifeLeechIsEqualToParent|FLAG|-|-|type=GlobalEffect/effectType=Link
---
Flame Link
100
20|Skill:FlameLink|PhysicalDamageGainAsFire|BASE|-|-|type=GlobalEffect/effectType=Link,type=PerStat/actor=parent/div=100/stat=Life
---";

/// Socket groups that give a supporting build buffs to export: a link,
/// an aura, a curse or mark, a warcry, by build
const PARTY_GROUPS: [&[&[&str]]; 8] = [
    &[&["Destructive Link"], &["Determination"], &["Despair"]],
    &[&["Flame Link"], &["Pride"], &["Rallying Cry"]],
    &[&["Protective Link"], &["Vaal Grace"], &["Assassin's Mark"]],
    &[&["Intuitive Link"], &["Wrath"], &["Intimidating Cry"]],
    &[&["Vampiric Link"], &["Malevolence"], &["Punishment"]],
    &[&["Soul Link"], &["Purity of Elements"], &["Ancestral Cry"]],
    &[&["Vaal Haste"], &["Haste"], &["Enfeeble"]],
    &[],
];

/// The Party tab: corpus builds that export their buffs, some with
/// socket groups added that give them buffs to export, and each corpus
/// build with another's exported buffs imported as party members' give
/// the translation's sidebar, outputs, Calcs tab and file.
/// `PARTY_STEP=<n>` takes every nth build (default 2)
fn party_match(data: &'static TypedData, d: &mut Dispatcher) {
    let step: usize = std::env::var("PARTY_STEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let builds: Vec<(String, String)> = mine(corpus())
        .into_iter()
        .step_by(step)
        .map(|(id, path)| (id, std::fs::read_to_string(&path).unwrap()))
        .collect();
    let (mut failures, mut compared, mut skipped) = (Vec::new(), 0, Vec::new());
    let mut aligned = 0;
    let mut check = |d: &mut Dispatcher, id: &str, xml: &str, failures: &mut Vec<String>| {
        at_build(d, id);
        // `PARTY_DUMP=<dir>` writes the builds there
        if let Ok(dir) = std::env::var("PARTY_DUMP") {
            std::fs::write(Path::new(&dir).join(format!("{id}.xml")), xml).unwrap();
        }
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            let e = d.typed_error.clone().unwrap_or_default();
            // Where the typed engine fails, upstream does
            let tr = answer(d.translated_request(r#"{"cmd":"outputs"}"#), "translated");
            if !e.starts_with("not ported") && tr.get("error").is_none() {
                failures.push(format!("{id}: typed fails ({e}), the translation does not"));
            }
            if tr.get("error").is_some() {
                // A build neither loads is not a fallback
                d.fallbacks.clear();
            }
            skipped.push(format!("{id}: {e}"));
            return None;
        }
        let mut out = Vec::new();
        let typed: Vec<J> = ["state", "outputs"]
            .iter()
            .map(|c| answer(d.request(&json!({"cmd": c}).to_string()), c))
            .collect();
        answer(
            d.translated_request(r#"{"cmd":"state"}"#),
            "translated load",
        );
        for (c, t) in ["state", "outputs"].iter().zip(&typed) {
            let tr = answer(d.translated_request(&json!({"cmd": c}).to_string()), c);
            diff(t, &tr, c, &mut out);
        }
        if out.is_empty() {
            let (_, o, _) = compare_calcs(data, d, id, &mut [Duration::ZERO; 2]);
            out.extend(o);
        }
        // The tab's view, twice: its first layout wraps the notes label
        for k in 0..2 {
            let t = answer(d.request(r#"{"cmd":"party"}"#), "party");
            let tr = answer(d.translated_request(r#"{"cmd":"party"}"#), "party");
            diff(&t, &tr, &format!("party view {k}"), &mut out);
        }
        let typed_save = answer(d.request(r#"{"cmd":"saveXml"}"#), "typed save");
        let tr = answer(
            d.translated_request(r#"{"cmd":"saveXml"}"#),
            "translated save",
        );
        if out.is_empty() {
            let a = typed_save["ok"]["xml"].as_str().unwrap_or("");
            let b = tr["ok"]["xml"].as_str().unwrap_or("");
            aligned += compare_party_saves(a, b, &mut out);
        }
        compared += 1;
        if !out.is_empty() {
            let n = out.len();
            out.truncate(8);
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
        tr["ok"]["xml"].as_str().map(String::from)
    };
    let mut exports: Vec<Vec<(String, String)>> = Vec::new();
    let mut codes: Vec<String> = Vec::new();
    for (k, (id, xml)) in builds.iter().enumerate() {
        let groups = PARTY_GROUPS[k % PARTY_GROUPS.len()];
        let xml = with_groups(data, &with_export(xml), groups).unwrap_or_else(|| with_export(xml));
        let saved = check(d, &format!("{id}-export"), &xml, &mut failures);
        if let Some(x) = &saved {
            let z = miniz_oxide::deflate::compress_to_vec_zlib(x.as_bytes(), 9);
            codes.push(base64(&z).replace('+', "-").replace('/', "_"));
        }
        exports.push(saved.map(|x| exported(&x)).unwrap_or_default());
    }
    let n = builds.len();
    for (k, (id, xml)) in builds.iter().enumerate() {
        let from = (k + 1) % n;
        let buffs = &exports[from];
        if buffs.is_empty() {
            continue;
        }
        let xml = with_party(xml, buffs);
        check(
            d,
            &format!("{id}-party-{}", builds[from].0),
            &xml,
            &mut failures,
        );
    }
    // Party members' stats and every link's mods, written by hand
    for (k, (id, xml)) in builds.iter().enumerate().step_by(4) {
        let mut buffs = exports[(k + 2) % n].clone();
        buffs.retain(|(name, _)| name != "PartyMemberStats" && name != "Link Skills");
        buffs.push(("PartyMemberStats".into(), PARTY_STATS.into()));
        buffs.push(("Link Skills".into(), PARTY_LINKS.into()));
        // With the advanced info shown: the texts in their edit boxes
        let xml = with_party(xml, &buffs).replacen(
            "ShowAdvanceTools=\"false\"",
            "ShowAdvanceTools=\"true\"",
            1,
        );
        check(d, &format!("{id}-party-stats"), &xml, &mut failures);
    }
    party_controls_match(d, &builds, &exports, &codes, &mut failures, &mut skipped);
    eprintln!(
        "{compared} party builds compared, {aligned} keystone sources aligned; skipped {}:\n{}",
        skipped.len(),
        skipped.join("\n")
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The Party tab's views after a change
const PARTY_VIEWS: [&str; 3] = ["party", "state", "outputs"];

/// The Party tab's controls (`party.*`) changed at random on corpus builds
/// with a party: import codes (other builds' exports, invalid text, a
/// link) to each destination, appended or replacing, Clear, Disable Party
/// Effects, Rebuild All, Show Advanced Info and the edit boxes' texts.
/// After each change the tab, the sidebar and the outputs, and at the end
/// the saved file, are the translation's.
fn party_controls_match(
    d: &mut Dispatcher,
    builds: &[(String, String)],
    exports: &[Vec<(String, String)>],
    codes: &[String],
    failures: &mut Vec<String>,
    skipped: &mut Vec<String>,
) {
    if codes.is_empty() {
        return;
    }
    let steps_per_build: usize = std::env::var("SESSION_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let n = builds.len();
    let mut steps = 0;
    for (k, (id, xml)) in builds.iter().enumerate() {
        let buffs = &exports[(k + 1) % n];
        let xml = if buffs.is_empty() {
            xml.clone()
        } else {
            with_party(xml, buffs)
        };
        let id = format!("{id}-party-controls");
        at_build(d, &id);
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let mut rng = Lcg(seed(&id));
        let mut out = Vec::new();
        let edits = [
            ("editPartyMemberStats", "PartyMemberStats"),
            ("editAuras", "Aura"),
            ("editCurses", "Curse"),
            ("editWarcries", "Warcry Skills"),
            ("editLinks", "Link Skills"),
            ("enemyCond", "EnemyConditions"),
            ("enemyMods", "EnemyMods"),
        ];
        for step in 0..steps_per_build {
            if !out.is_empty() {
                break;
            }
            let path = |name: &str| format!("party.{name}");
            let req = match rng.below(12) {
                0..=2 => {
                    let code = match rng.below(6) {
                        0 => "not a code".to_string(),
                        1 => String::new(),
                        2 => format!(" {}? ", codes[rng.below(codes.len())]),
                        _ => codes[rng.below(codes.len())].clone(),
                    };
                    json!({"cmd": "control", "path": path("importCodeIn"), "text": code})
                }
                3 => {
                    json!({"cmd": "control", "path": path("importCodeDestination"), "sel": rng.below(9) + 1})
                }
                4 => {
                    json!({"cmd": "control", "path": path("appendNotReplace"), "state": rng.below(2) == 0})
                }
                5 => {
                    json!({"cmd": "control", "path": path("ShowAdvanceTools"), "state": rng.below(2) == 0})
                }
                6 | 7 => json!({"cmd": "control", "path": path("importCodeGo"), "click": true}),
                8 => json!({"cmd": "control", "path": path("clear"), "click": true}),
                9 => json!({"cmd": "control", "path": path("removeEffects"), "click": true}),
                10 => json!({"cmd": "control", "path": path("rebuild"), "click": true}),
                _ => {
                    let (name, buf) = edits[rng.below(edits.len())];
                    let from = &exports[rng.below(n)];
                    let text = from
                        .iter()
                        .find(|(b, _)| b == buf)
                        .map(|(_, t)| t.clone())
                        .unwrap_or_default();
                    json!({"cmd": "control", "path": path(name), "text": text})
                }
            };
            let label = format!("step {step} {}", truncate(&req.to_string()));
            let r = req.to_string();
            let typed = answer(d.request(&r), "typed party edit");
            if !d.typed_active() {
                let e = d.typed_error.clone().unwrap_or_default();
                assert!(
                    e.starts_with("not ported") || e.contains("upstream fails"),
                    "{id}: {label}: {e}"
                );
                skipped.push(format!("{id}: {e}"));
                if e.contains("upstream fails") {
                    // Upstream's failure leaves its session broken: a
                    // fresh one for the builds after
                    at_build(d, &id);
                    *d = Dispatcher::new(session(), Some(data()), Prefer::Typed);
                }
                break;
            }
            if typed.get("error").is_some() {
                let tr = answer(d.translated_request(&r), "translated party edit");
                diff(&typed, &tr, &label, &mut out);
            }
            answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
            compare_views(d, &PARTY_VIEWS, &label, &mut out);
            // A control's hover tooltip
            let names = [
                "importCodeDestination",
                "importCodeGo",
                "rebuild",
                "editAuras",
                "simpleCurses",
            ];
            let q = json!({"cmd": "controlTooltip", "path": format!("party.{}", names[rng.below(names.len())])});
            let q = q.to_string();
            let t = answer(d.request(&q), "typed tooltip");
            let tr = answer(d.translated_request(&q), "translated tooltip");
            diff(&t, &tr, &format!("{label} {q}"), &mut out);
            steps += 1;
        }
        if out.is_empty() {
            compare_saves(d, &format!("{id} after the party edits"), &mut out);
        }
        if !out.is_empty() {
            let m = out.len();
            out.truncate(8);
            failures.push(format!("{id} ({m}):\n  {}", out.join("\n  ")));
        }
    }
    eprintln!("{steps} party control edits compared");
}

/// The Tree tab's controls that open no popup (`tree.*`): the search,
/// the heat map, its node power depth and the power report's toggle, the
/// drop-downs the tab fills when it draws, and the compare checkbox, at
/// random on every fourth corpus build. After each the sidebar (whose
/// unsaved flag counts the search), the outputs and a control's tooltip,
/// then node power within the depth chosen and the saved file, are the
/// translation's.
fn tree_controls_match(d: &mut Dispatcher) {
    let (mut failures, mut steps, mut skipped) = (Vec::new(), 0, Vec::new());
    let names = [
        "specSelect",
        "compareCheck",
        "compareSelect",
        "versionText",
        "treeSearch",
        "treeHeatMap",
        "nodePowerMaxDepthSelect",
        "nodePowerMaxDepthCustom",
        "treeHeatMapStatSelect",
        "powerReport",
    ];
    for (id, path) in mine(corpus()).into_iter().step_by(4) {
        let id = format!("{id}-tree-controls");
        at_build(d, &id);
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let mut rng = Lcg(seed(&id));
        let mut out = Vec::new();
        let mut power_compared = false;
        for step in 0..12 {
            if !out.is_empty() {
                break;
            }
            let path = |name: &str| format!("tree.{name}");
            let state = answer(d.request(r#"{"cmd":"state"}"#), "state");
            let popup = state["ok"]["popups"]
                .as_array()
                .and_then(|p| p.first())
                .cloned();
            let req = if let Some(p) = popup {
                // A button of the popup in front
                let names: Vec<&str> = p["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|b| b["name"].as_str())
                    .collect();
                json!({"cmd": "popupButton", "name": names[rng.below(names.len())]})
            } else if rng.below(3) == 0 {
                // Reset, the version drop-down and the conversions
                match rng.below(4) {
                    0 => json!({"cmd": "control", "path": path("reset"), "click": true}),
                    1 => {
                        json!({"cmd": "control", "path": path("versionSelect"), "sel": rng.below(41) + 1})
                    }
                    2 => json!({"cmd": "control", "path": path("specConvert"), "click": true}),
                    _ => json!({"cmd": "control", "path": path("specConvertAll"), "click": true}),
                }
            } else {
                match rng.below(9) {
                    0 | 1 => {
                        let s = ["", "fire", "Life", "(increased.fire.damage|area)", "%d+"]
                            [rng.below(5)];
                        json!({"cmd": "control", "path": path("treeSearch"), "text": s})
                    }
                    2 => {
                        json!({"cmd": "control", "path": path("treeHeatMap"), "state": rng.below(3) != 0})
                    }
                    3 | 4 => {
                        json!({"cmd": "control", "path": path("nodePowerMaxDepthSelect"), "sel": rng.below(6) + 1})
                    }
                    5 => {
                        let s = ["3", "", "12", "x"][rng.below(4)];
                        json!({"cmd": "control", "path": path("nodePowerMaxDepthCustom"), "text": s})
                    }
                    6 => json!({"cmd": "control", "path": path("powerReport"), "click": true}),
                    7 => {
                        let n =
                            ["specSelect", "compareSelect", "treeHeatMapStatSelect"][rng.below(3)];
                        json!({"cmd": "control", "path": path(n), "sel": rng.below(4) + 1})
                    }
                    _ => json!({"cmd": "control", "path": path("compareCheck"), "state": true}),
                }
            };
            let label = format!("step {step} {req}");
            if std::env::var("SESSION_DUMP").is_ok() {
                eprintln!("{id} {label}");
            }
            // Node power within the depth chosen, before the first
            // conversion (see `docs/progress.md` on converted General's
            // Cry builds)
            let converts = matches!(req["name"].as_str(), Some("convert" | "convertCopy"))
                || req["path"] == "tree.specConvert";
            if converts && !power_compared {
                compare_node_power(d, &mut out);
                power_compared = true;
            }
            let r = req.to_string();
            let typed = answer(d.request(&r), "typed tree control");
            if !d.typed_active() {
                let e = d.typed_error.clone().unwrap_or_default();
                assert!(e.starts_with("not ported"), "{id}: {label}: {e}");
                skipped.push(format!("{id}: {e}"));
                break;
            }
            if typed.get("error").is_some() {
                let tr = answer(d.translated_request(&r), "translated tree control");
                diff(&typed, &tr, &label, &mut out);
            }
            answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
            compare_views(
                d,
                &["state", "outputs", "treeSpecs", "treeState"],
                &label,
                &mut out,
            );
            let q = json!({"cmd": "controlTooltip", "path": path(names[rng.below(names.len())])})
                .to_string();
            let t = answer(d.request(&q), "typed tooltip");
            let tr = answer(d.translated_request(&q), "translated tooltip");
            diff(&t, &tr, &format!("{label} {q}"), &mut out);
            steps += 1;
        }
        // Popups stay open over the next build's load in upstream: closed
        for _ in 0..4 {
            let state = answer(d.request(r#"{"cmd":"state"}"#), "state");
            if state["ok"]["popups"]
                .as_array()
                .is_none_or(|p| p.is_empty())
            {
                break;
            }
            answer(d.request(r#"{"cmd":"closePopup"}"#), "close");
        }
        if out.is_empty() {
            if !power_compared {
                compare_node_power(d, &mut out);
            }
            compare_saves(d, &format!("{id} after the tree controls"), &mut out);
        }
        if !out.is_empty() {
            let m = out.len();
            out.truncate(8);
            failures.push(format!("{id} ({m}):\n  {}", out.join("\n  ")));
        }
    }
    eprintln!(
        "{steps} tree control edits compared; skipped {}:\n{}",
        skipped.len(),
        skipped.join("\n")
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// `nodePower` of the first power stat, typed against translated
fn compare_node_power(d: &mut Dispatcher, out: &mut Vec<String>) {
    let q = r#"{"cmd":"nodePower","index":1}"#;
    let t = answer(d.request(q), "typed nodePower");
    let tr = answer(d.translated_request(q), "translated nodePower");
    diff(&t, &tr, "nodePower", out);
}

/// The first characters of a request, for a failure's label
fn truncate(s: &str) -> String {
    s.chars().take(160).collect()
}

/// The colour codes the sidebar uses are upstream's
#[test]
fn color_codes_match_upstream() {
    let src = gamedata::Source::load(Path::new(&src())).unwrap();
    let codes = src.global("colorCodes");
    let all = session::typed::display_stats::COLOR_CODES.iter();
    for (name, code) in all.chain(session::typed::tooltip::COLORS) {
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
    for (id, path) in mine(corpus()) {
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
fn compare_calcs(
    data: &TypedData,
    d: &mut Dispatcher,
    id: &str,
    times: &mut [Duration; 2],
) -> (usize, Vec<String>, usize) {
    let latest = data.tree(data.game.latest_tree_version()).unwrap();
    let mut aligned = 0;
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
        let mut tr = answer(d.translated_request(&req), "translated");
        times[1] += t.elapsed();
        aligned += align_keystone_sources(&typed, &mut tr, latest);
        let n = out.len();
        diff(&typed, &tr, &req, &mut out);
        if n == 0 && !out.is_empty() && std::env::var("SESSION_DUMP").is_ok() {
            eprintln!("{id} {req}:\ntyped {typed}\ntranslated {tr}");
        }
    }
    (reqs.len() - 2, out, aligned)
}

/// Upstream's tree keeps the source `modLib.mergeKeystones` last gave a
/// keystone's own mods, also from builds loaded before in the session: a
/// tree keystone's mod row that the translation shows as another source's
/// (an item or a configuration of an earlier build that granted the
/// keystone) takes the typed source. The number of rows so aligned.
fn align_keystone_sources(typed: &J, tr: &mut J, tree: &engine::tree::PassiveTree) -> usize {
    let mut n = 0;
    let (Some(ts), Some(rs)) = (
        typed["ok"]["sections"].as_array(),
        tr["ok"]["sections"].as_array_mut(),
    ) else {
        return 0;
    };
    for (t, r) in ts.iter().zip(rs) {
        let (Some(trows), Some(rrows)) = (t["rows"].as_array(), r["rows"].as_array_mut()) else {
            continue;
        };
        for (a, b) in trows.iter().zip(rrows) {
            let keystone = a["sourceName"]
                .as_str()
                .is_some_and(|k| tree.keystone_map.contains_key(k));
            if a["source"] == "Tree" && b["source"] != "Tree" && keystone {
                b["source"] = a["source"].clone();
                b["sourceName"] = a["sourceName"].clone();
                n += 1;
            }
        }
    }
    n
}

/// The Calcs tab gives the translation's JSON, for every corpus build
fn calcs_match(data: &'static TypedData, d: &mut Dispatcher) {
    let (mut failures, mut compared, mut cells, mut aligned) = (Vec::new(), 0, 0, 0);
    let mut times = [Duration::ZERO; 2];
    for (id, path) in mine(corpus()) {
        at_build(d, &id);
        let xml = std::fs::read_to_string(&path).unwrap();
        let load = json!({"cmd": "loadXml", "xml": xml}).to_string();
        answer(d.request(&load), "load");
        if !d.typed_active() {
            continue;
        }
        let (n, mut out, a) = compare_calcs(data, d, &id, &mut times);
        aligned += a;
        cells += n;
        compared += 1;
        if !out.is_empty() {
            let n = out.len();
            out.truncate(6);
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
    }
    eprintln!(
        "{compared} builds, {cells} breakdowns: typed {:.1?}, translated {:.1?} (with its load); {aligned} keystone sources aligned",
        times[0], times[1]
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A build's seed, so that a filtered run takes its steps
fn seed(id: &str) -> u64 {
    id.bytes()
        .fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64))
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
fn tree_edits_match(d: &mut Dispatcher) {
    let (mut failures, mut steps, mut times) = (Vec::new(), 0, [Duration::ZERO; 2]);
    let mut skipped = Vec::new();
    let steps_per_build: usize = std::env::var("SESSION_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    for (id, path) in mine(corpus()) {
        at_build(d, &id);
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
        let mut rng = Lcg(seed(&id) + 1);
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
            if step >= socket_clicks.len() && step < total && rng.below(10) == 0 {
                // Manage Passive Trees: specs copied, added, renamed,
                // deleted and switched between the edits (and their undo)
                let r = json!({"cmd": "control", "path": "items.specButton", "click": true});
                answer(d.request(&r.to_string()), "manage");
                let label = format!("step {step} trees");
                let (mut log, mut counts) = (Vec::new(), Default::default());
                if !d.typed_active()
                    || drive_popups(d, &mut rng, &label, &mut log, &mut out, &mut counts)
                {
                    let e = d.typed_error.clone().unwrap_or_default();
                    skipped.push(format!("{id}: {e}"));
                    break;
                }
                let follow = [
                    json!({"cmd": "treeState"}),
                    json!({"cmd": "treeSpecs"}),
                    json!({"cmd": "state"}),
                    json!({"cmd": "outputs"}),
                ];
                compare(d, &follow, &label, &mut out);
                steps += 1;
                if !out.is_empty() {
                    break;
                }
                continue;
            }
            let req = if let Some(s) = (step < socket_clicks.len()).then(|| socket_clicks[step]) {
                json!({"cmd": "clickNode", "id": s})
            } else if step == total {
                json!({"cmd": "resetTree"})
            } else if rng.below(5) == 0 {
                json!({"cmd": "undo"})
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
            if std::env::var("SESSION_DUMP").is_ok() {
                eprintln!("{id} {label}");
            }
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
            if step < socket_clicks.len() || req["cmd"] == "undo" {
                // The subgraphs taken off and built again
                follow.push(json!({"cmd": "tree"}));
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
            compare(d, &follow, &label, &mut out);
            steps += 1;
            if !out.is_empty() {
                break;
            }
        }
        if out.is_empty() && d.typed_active() {
            compare_saves(d, "after the edits", &mut out);
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

/// Random sequences of Skills tab edits and skill selections (main socket
/// group, groups enabled, in Full DPS, slots, gems added, renamed,
/// levelled and removed, groups added and removed, the sidebar's and the
/// Calcs tab's skill controls) give the translation's skills, sidebar,
/// outputs, Calcs tab and controls after each step
fn skills_edits_match(d: &mut Dispatcher) {
    let (mut failures, mut steps, mut times) = (Vec::new(), 0, [Duration::ZERO; 2]);
    let mut skipped = Vec::new();
    let steps_per_build: usize = std::env::var("SESSION_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let gems = answer(d.translated_request(r#"{"cmd":"gemList"}"#), "gemList");
    let gem_names: Vec<String> = gems["ok"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["name"].as_str().map(String::from))
        .collect();
    for (id, path) in mine(corpus()) {
        at_build(d, &id);
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        if !d.typed_active() {
            continue;
        }
        let mut rng = Lcg(seed(&id) + 7);
        let mut out = Vec::new();
        let compare = |d: &mut Dispatcher, req: &J, label: &str, out: &mut Vec<String>| {
            let r = req.to_string();
            let typed = answer(d.request(&r), "typed");
            let tr = answer(d.translated_request(&r), "translated");
            let n = out.len();
            diff(&typed, &tr, &format!("{label} {r}"), out);
            if n == 0 && !out.is_empty() && std::env::var("SESSION_DUMP").is_ok() {
                eprintln!("{label} {r}:\ntyped {typed}\ntranslated {tr}");
            }
            typed
        };
        compare(d, &json!({"cmd": "skills"}), "start", &mut out);
        compare(d, &json!({"cmd": "gemList"}), "start", &mut out);
        for step in 0..steps_per_build {
            let skills = answer(d.request(r#"{"cmd":"skills"}"#), "skills");
            let groups = skills["ok"]["groups"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let slots = skills["ok"]["slots"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let n = groups.len().max(1);
            let g = rng.below(n) + 1;
            let gem_count = groups
                .get(g - 1)
                .map_or(0, |x| x["gems"].as_array().map_or(0, Vec::len));
            let pick_gem = |rng: &mut Lcg| json!(gem_names[rng.below(gem_names.len())]);
            let req = match rng.below(12) {
                0 => json!({"cmd": "setMainSocketGroup", "index": g}),
                1 => json!({"cmd": "setSocketGroup", "index": g, "enabled": rng.below(2) == 0}),
                2 => {
                    json!({"cmd": "setSocketGroup", "index": g, "includeInFullDPS": rng.below(2) == 0})
                }
                3 => {
                    let slot = if slots.is_empty() || rng.below(3) == 0 {
                        json!("")
                    } else {
                        slots[rng.below(slots.len())].clone()
                    };
                    json!({"cmd": "setSocketGroup", "index": g, "slot": slot})
                }
                4 => json!({"cmd": "setGem", "group": g, "gem": rng.below(gem_count.max(1)) + 1,
                    "level": rng.below(21) + 1, "quality": rng.below(24)}),
                5 => {
                    json!({"cmd": "setGem", "group": g, "gem": gem_count + 1, "name": pick_gem(&mut rng)})
                }
                6 => {
                    json!({"cmd": "setGem", "group": g, "gem": rng.below(gem_count.max(1)) + 1, "name": pick_gem(&mut rng)})
                }
                7 if gem_count > 0 => {
                    json!({"cmd": "removeGem", "group": g, "gem": rng.below(gem_count) + 1})
                }
                8 => {
                    json!({"cmd": "addSocketGroup", "label": if rng.below(2) == 0 { "" } else { "New" }})
                }
                9 if groups.len() > 1 && rng.below(2) == 0 => {
                    // A group dragged within the skills tab's list
                    json!({"cmd": "control", "path": "skills.groupList", "sel": g, "moveTo": rng.below(n + 1) + 1})
                }
                9 if groups.len() > 1 => json!({"cmd": "removeSocketGroup", "index": g}),
                10 => json!({"cmd": "control", "path": "build.mainSkill", "sel": rng.below(3) + 1}),
                _ => {
                    compare(d, &json!({"cmd": "calcs"}), "before", &mut out);
                    match rng.below(4) {
                        0 => json!({"cmd": "control", "path": "calcs.mainSocketGroup", "sel": g}),
                        1 => {
                            json!({"cmd": "control", "path": "calcs.mode", "sel": rng.below(4) + 1})
                        }
                        2 => {
                            json!({"cmd": "control", "path": "calcs.mainSkill", "sel": rng.below(3) + 1})
                        }
                        _ => {
                            json!({"cmd": "control", "path": "build.mainSkillPart", "sel": rng.below(3) + 1})
                        }
                    }
                }
            };
            let label = format!("step {step}");
            let r = req.to_string();
            let t = Instant::now();
            let typed = answer(d.request(&r), "typed edit");
            times[0] += t.elapsed();
            if !d.typed_active() {
                let e = d.typed_error.clone().unwrap_or_default();
                assert!(e.starts_with("not ported"), "{id}: {label} {r}: {e}");
                skipped.push(format!("{id}: {r}: {e}"));
                break;
            }
            let t = Instant::now();
            let tr = answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
            times[1] += t.elapsed();
            let _ = (typed, tr);
            for f in ["skills", "state", "outputs", "calcs"] {
                compare(d, &json!({"cmd": f}), &format!("{label} {r}"), &mut out);
            }
            let paths = [
                "calcs.mainSocketGroup",
                "calcs.mainSkill",
                "calcs.mode",
                "calcs.mainSkillPart",
                "build.mainSkill",
            ];
            compare(
                d,
                &json!({"cmd": "controls", "paths": paths}),
                &format!("{label} {r}"),
                &mut out,
            );
            steps += 1;
            if !out.is_empty() {
                break;
            }
        }
        if out.is_empty() && d.typed_active() {
            compare_saves(d, "after the edits", &mut out);
        }
        // Then every group in Full DPS, one after another
        let groups = answer(d.request(r#"{"cmd":"skills"}"#), "skills")["ok"]["groups"]
            .as_array()
            .map_or(0, Vec::len);
        for g in 1..=groups {
            if !out.is_empty() || !d.typed_active() {
                break;
            }
            let req = json!({"cmd": "setSocketGroup", "index": g, "includeInFullDPS": true});
            answer(d.request(&req.to_string()), "typed edit");
            if d.typed_active() {
                for f in ["state", "outputs"] {
                    compare(d, &json!({"cmd": f}), &format!("full dps {req}"), &mut out);
                }
            }
        }
        if out.is_empty() && d.typed_active() {
            compare_saves(d, "after the edits", &mut out);
        }
        // Then every group in Full DPS, one after another
        let groups = answer(d.request(r#"{"cmd":"skills"}"#), "skills")["ok"]["groups"]
            .as_array()
            .map_or(0, Vec::len);
        for g in 1..=groups {
            if !out.is_empty() || !d.typed_active() {
                break;
            }
            let req = json!({"cmd": "setSocketGroup", "index": g, "includeInFullDPS": true});
            answer(d.request(&req.to_string()), "typed edit");
            if d.typed_active() {
                for f in ["state", "outputs"] {
                    compare(d, &json!({"cmd": f}), &format!("full dps {req}"), &mut out);
                }
            }
        }
        if !out.is_empty() {
            let n = out.len();
            out.truncate(8);
            failures.push(format!("{id} ({n}):\n  {}", out.join("\n  ")));
        }
    }
    eprintln!(
        "{steps} skills edits: typed {:.1?}, translated {:.1?} (with its replays)\nskipped:\n{}",
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

/// Replays `SESSION_SEQ` (a JSON array of requests) on the builds of
/// `SESSION_FILTER`, comparing `skills`, `state` and `outputs` after each;
/// for narrowing down a difference
#[test]
#[ignore]
fn replay_sequence() {
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let seq: Vec<J> = serde_json::from_str(&std::env::var("SESSION_SEQ").unwrap()).unwrap();
    for (id, path) in mine(corpus()) {
        let xml = std::fs::read_to_string(&path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        for req in &seq {
            let r = req.to_string();
            eprintln!("{id} {r}: typed {}", answer(d.request(&r), "typed"));
            answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
            for f in ["skills", "state", "outputs", "treeState"] {
                let q = json!({ "cmd": f }).to_string();
                let (t, tr) = (
                    answer(d.request(&q), f),
                    answer(d.translated_request(&q), f),
                );
                let mut out = Vec::new();
                diff(&t, &tr, f, &mut out);
                out.truncate(4);
                if !out.is_empty() {
                    eprintln!("  {}", out.join("\n  "));
                }
            }
        }
    }
}

/// The shared item list and item sets outlive a build the translation held:
/// the next typed session reads them from it
#[test]
fn shared_lists_survive_a_fallback() {
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let builds: Vec<String> = corpus()
        .into_iter()
        .take(2)
        .map(|(_, p)| std::fs::read_to_string(p).unwrap())
        .collect();
    let load = |d: &mut Dispatcher, xml: &str| {
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        assert!(d.typed_active(), "load: {:?}", d.typed_error);
    };
    load(&mut d, &builds[0]);
    for r in [
        json!({"cmd": "control", "path": "items.setManage", "click": true}),
        json!({"cmd": "control", "path": "popup.setList", "sel": 1, "dragTo": "popup.sharedList"}),
        json!({"cmd": "closePopup"}),
        json!({"cmd": "control", "path": "items.itemList", "sel": 1, "dragTo": "items.sharedItemList"}),
        json!({"cmd": "control", "path": "items.itemList", "sel": 2, "dragTo": "items.sharedItemList"}),
    ] {
        answer(d.request(&r.to_string()), "shared");
        assert!(d.typed_active(), "{r}: {:?}", d.typed_error);
    }
    let mut out = Vec::new();
    compare_views(&mut d, &["sharedLists"], "before the fallback", &mut out);
    // A key the typed session does not have: the translation holds the build
    let r = json!({"cmd": "control", "path": "items.sharedItemList", "key": "F2"});
    answer(d.request(&r.to_string()), "fallback");
    assert!(!d.typed_active());
    load(&mut d, &builds[1]);
    compare_views(&mut d, &["sharedLists"], "after the fallback", &mut out);
    let shared = json!({"cmd": "controls", "paths": ["items.sharedItemList"]}).to_string();
    let (t, tr) = (
        answer(d.request(&shared), "typed"),
        answer(d.translated_request(&shared), "translated"),
    );
    assert!(
        t["ok"]["items.sharedItemList"]["list"]
            .as_array()
            .is_some_and(|l| l.len() == 2),
        "{t}"
    );
    diff(&t, &tr, "shared items", &mut out);
    for r in [
        json!({"cmd": "controlTooltip", "path": "items.sharedItemList", "index": 2}),
        json!({"cmd": "control", "path": "items.setManage", "click": true}),
        json!({"cmd": "control", "path": "popup.sharedList", "sel": 1, "dragTo": "popup.setList"}),
        json!({"cmd": "closePopup"}),
        json!({"cmd": "control", "path": "items.sharedItemList", "sel": 2, "dragTo": "items.itemList"}),
    ] {
        let t = answer(d.request(&r.to_string()), "typed");
        assert!(d.typed_active(), "{r}: {:?}", d.typed_error);
        if r["cmd"] == "controlTooltip" {
            let tr = answer(d.translated_request(&r.to_string()), "translated");
            diff(&t, &tr, &r.to_string(), &mut out);
        }
        compare_views(&mut d, &["state", "items"], &r.to_string(), &mut out);
    }
    compare_saves(&mut d, "shared", &mut out);
    assert!(out.is_empty(), "{}", out.join("\n"));
}

/// Manage Passive Trees' Import Tree with links of each kind the popup
/// reads (and broken ones), and Export Tree, on a few builds
#[test]
fn tree_links_import_and_export() {
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let mut out = Vec::new();
    for (k, (id, path)) in corpus().into_iter().step_by(17).take(4).enumerate() {
        let xml = std::fs::read_to_string(path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        assert!(d.typed_active(), "{id} load: {:?}", d.typed_error);
        let mut rng = Lcg(seed(&id) + 5);
        let step = |d: &mut Dispatcher, r: J, out: &mut Vec<String>| {
            answer(d.request(&r.to_string()), "typed");
            assert!(d.typed_active(), "{id} {r}: {:?}", d.typed_error);
            compare_views(
                d,
                &["state", "treeSpecs", "treeState"],
                &format!("{id} {r}"),
                out,
            );
        };
        step(
            &mut d,
            json!({"cmd": "control", "path": "items.specButton", "click": true}),
            &mut out,
        );
        for i in 0..8 {
            step(
                &mut d,
                json!({"cmd": "control", "path": "popup.2", "click": true}),
                &mut out,
            );
            step(
                &mut d,
                json!({"cmd": "control", "path": "popup.name", "text": format!("Imported {i}")}),
                &mut out,
            );
            // First the build's own tree with its masteries, in both formats
            let link = match i {
                0 | 1 => own_tree_link(&mut d, i == 1),
                _ => tree_link(&mut d, &mut rng),
            };
            step(
                &mut d,
                json!({"cmd": "control", "path": "popup.edit", "text": link}),
                &mut out,
            );
            step(
                &mut d,
                json!({"cmd": "popupButton", "name": "import"}),
                &mut out,
            );
            let st = answer(d.request(r#"{"cmd":"state"}"#), "state");
            if st["ok"]["popups"][0]["title"] == "Import Tree" {
                step(
                    &mut d,
                    json!({"cmd": "popupButton", "name": "cancel"}),
                    &mut out,
                );
            }
        }
        compare_views(&mut d, &["outputs"], &format!("{id} imported"), &mut out);
        step(
            &mut d,
            json!({"cmd": "control", "path": "popup.3", "click": true}),
            &mut out,
        );
        step(
            &mut d,
            json!({"cmd": "popupButton", "name": "shrink"}),
            &mut out,
        );
        step(
            &mut d,
            json!({"cmd": "popupButton", "name": "copy"}),
            &mut out,
        );
        step(
            &mut d,
            json!({"cmd": "popupButton", "name": "done"}),
            &mut out,
        );
        step(
            &mut d,
            json!({"cmd": "control", "path": "popup.4", "click": true}),
            &mut out,
        );
        if k == 0 {
            step(&mut d, json!({"cmd": "undo"}), &mut out);
        }
        compare_saves(&mut d, &id, &mut out);
    }
    assert!(out.is_empty(), "{}", out.join("\n"));
    // A mastery on a node the link's tree does not have: upstream fails,
    // the typed session hands it over
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let (_, path) = corpus().into_iter().next().unwrap();
    let xml = std::fs::read_to_string(path).unwrap();
    answer(
        d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
        "load",
    );
    let link = format!(
        "https://www.pathofexile.com/passive-skill-tree/3.15.0/{}",
        base64(&[0, 0, 0, 6, 2, 0, 0, 0, 1, 0, 1, 0, 1])
    );
    for r in [
        json!({"cmd": "control", "path": "items.specButton", "click": true}),
        json!({"cmd": "control", "path": "popup.2", "click": true}),
        json!({"cmd": "control", "path": "popup.edit", "text": link}),
        json!({"cmd": "popupButton", "name": "import"}),
    ] {
        answer(d.request(&r.to_string()), "request");
    }
    assert!(!d.typed_active());
    let e = d.typed_error.clone().unwrap_or_default();
    assert!(e.contains("(upstream fails)"), "{e}");
}

/// Items dropped on the item slots and on the minion drop-down while it
/// shows an item set (Animate Guardian's Weapon's), from the item list,
/// the databases and the shared item list
#[test]
fn items_dropped_on_slots_and_minion_sets() {
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let Some((id, path)) = corpus().into_iter().find(|(id, _)| id == "gen-0006") else {
        return;
    };
    let xml = std::fs::read_to_string(path).unwrap();
    answer(
        d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
        "load",
    );
    let mut out = Vec::new();
    let step = |d: &mut Dispatcher, r: J, out: &mut Vec<String>| {
        answer(d.request(&r.to_string()), "typed");
        assert!(d.typed_active(), "{id} {r}: {:?}", d.typed_error);
        compare_views(d, &["state", "items", "outputs"], &format!("{id} {r}"), out);
    };
    // A second item set, and the Animate Guardian's Weapon group as the
    // main one, its minion on the second set
    step(
        &mut d,
        json!({"cmd": "control", "path": "items.setManage", "click": true}),
        &mut out,
    );
    step(
        &mut d,
        json!({"cmd": "control", "path": "popup.setList.new", "click": true}),
        &mut out,
    );
    step(
        &mut d,
        json!({"cmd": "popupButton", "name": "save"}),
        &mut out,
    );
    step(&mut d, json!({"cmd": "closePopup"}), &mut out);
    let skills = answer(d.request(r#"{"cmd":"skills"}"#), "skills");
    let group = skills["ok"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .position(|g| g.to_string().contains("Animate Guardian"))
        .expect("an Animate Guardian's Weapon group")
        + 1;
    step(
        &mut d,
        json!({"cmd": "setMainSocketGroup", "index": group}),
        &mut out,
    );
    let rows = |d: &mut Dispatcher, path: &str| {
        let c = answer(
            d.request(&json!({"cmd": "controls", "paths": [path]}).to_string()),
            "controls",
        );
        c["ok"][path]["list"].as_array().map_or(0, Vec::len)
    };
    for sel in [1, 2] {
        step(
            &mut d,
            json!({"cmd": "control", "path": "build.mainSkillMinion", "sel": sel}),
            &mut out,
        );
        // Each row of the item list, and some weapons of the databases
        for k in 1..=rows(&mut d, "items.itemList") {
            step(
                &mut d,
                json!({"cmd": "control", "path": "items.itemList", "sel": k, "dragTo": "build.mainSkillMinion"}),
                &mut out,
            );
        }
        step(
            &mut d,
            json!({"cmd": "control", "path": "items.uniqueDB.type", "sel": 4}),
            &mut out,
        );
        for k in [1, 5, 9] {
            step(
                &mut d,
                json!({"cmd": "control", "path": "items.uniqueDB", "sel": k, "dragTo": "build.mainSkillMinion"}),
                &mut out,
            );
        }
    }
    // Onto the slots: each shown slot gets a row of each list
    let items = answer(d.request(r#"{"cmd":"items"}"#), "items");
    let slots: Vec<String> = items["ok"]["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["shown"] == true)
        .filter_map(|s| s["name"].as_str().map(String::from))
        .collect();
    step(
        &mut d,
        json!({"cmd": "control", "path": "items.itemList", "sel": 1, "dragTo": "items.sharedItemList"}),
        &mut out,
    );
    for (k, slot) in slots.iter().enumerate() {
        let src = [
            "items.itemList",
            "items.uniqueDB",
            "items.rareDB",
            "items.sharedItemList",
        ][k % 4];
        let n = rows(&mut d, src).max(1);
        step(
            &mut d,
            json!({"cmd": "control", "path": src, "sel": k % n + 1, "dragTo": format!("slots.{slot}")}),
            &mut out,
        );
    }
    compare_saves(&mut d, &id, &mut out);
    assert!(out.is_empty(), "{}", out.join("\n"));
}

/// The Items tab's own controls on a few builds: their descriptions, the
/// set drop-down's tooltips, both weapon sets, a second item set and tree
/// picked from the drop-downs, Craft item... and Create custom...
#[test]
fn items_tab_controls_match() {
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let mut out = Vec::new();
    let paths = [
        "items.setSelect",
        "items.specSelect",
        "items.weaponSwap1",
        "items.weaponSwap2",
        "items.craftDisplayItem",
        "items.newDisplayItem",
        "items.setManage",
        "items.specButton",
    ];
    for (id, path) in corpus().into_iter().step_by(11).take(5) {
        let xml = std::fs::read_to_string(path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        assert!(d.typed_active(), "{id} load: {:?}", d.typed_error);
        let step = |d: &mut Dispatcher, r: J, out: &mut Vec<String>| {
            answer(d.request(&r.to_string()), "typed");
            assert!(d.typed_active(), "{id} {r}: {:?}", d.typed_error);
            let views = json!({"cmd": "controls", "paths": paths}).to_string();
            let (t, tr) = (
                answer(d.request(&views), "typed"),
                answer(d.translated_request(&views), "translated"),
            );
            diff(&t, &tr, &format!("{id} {r} controls"), out);
            compare_views(d, &["state", "items", "outputs"], &format!("{id} {r}"), out);
        };
        for r in [
            json!({"cmd": "control", "path": "items.weaponSwap2", "click": true}),
            json!({"cmd": "control", "path": "items.weaponSwap2", "click": true}),
            json!({"cmd": "control", "path": "items.weaponSwap1", "click": true}),
            // A second set, then each picked
            json!({"cmd": "control", "path": "items.setManage", "click": true}),
            json!({"cmd": "control", "path": "popup.setList.new", "click": true}),
            json!({"cmd": "popupButton", "name": "save"}),
            json!({"cmd": "closePopup"}),
            json!({"cmd": "control", "path": "items.setSelect", "sel": 2}),
            json!({"cmd": "control", "path": "items.weaponSwap2", "click": true}),
            json!({"cmd": "control", "path": "items.setSelect", "sel": 9}),
            // A second tree, then each picked
            json!({"cmd": "control", "path": "items.specButton", "click": true}),
            json!({"cmd": "control", "path": "popup.1.new", "click": true}),
            json!({"cmd": "popupButton", "name": "save"}),
            json!({"cmd": "closePopup"}),
            json!({"cmd": "control", "path": "items.specSelect", "sel": 2}),
            json!({"cmd": "control", "path": "items.specSelect", "sel": 1}),
            // A crafted item, and a custom one
            json!({"cmd": "control", "path": "items.craftDisplayItem", "click": true}),
            json!({"cmd": "popupButton", "name": "save"}),
            json!({"cmd": "saveDisplayItem"}),
            json!({"cmd": "control", "path": "items.newDisplayItem", "click": true}),
            json!({"cmd": "control", "path": "popup.edit", "text": "Custom Band\nIron Ring"}),
            json!({"cmd": "popupButton", "name": "save"}),
            json!({"cmd": "control", "path": "items.newDisplayItem", "click": true}),
            json!({"cmd": "popupButton", "name": "cancel"}),
        ] {
            step(&mut d, r, &mut out);
        }
        for index in 1..=3 {
            let r = json!({"cmd": "controlTooltip", "path": "items.setSelect", "index": index});
            let (t, tr) = (
                answer(d.request(&r.to_string()), "typed"),
                answer(d.translated_request(&r.to_string()), "translated"),
            );
            diff(&t, &tr, &format!("{id} {r}"), &mut out);
        }
        compare_saves(&mut d, &id, &mut out);
    }
    assert!(out.is_empty(), "{}", out.join("\n"));
}

/// The build's own controls: their descriptions, the level box's, main
/// socket group's and minion drop-down's tooltips, level scaling, the view
/// modes, and each entry of the loadouts drop-down (New Loadout's popup
/// saved), on a few builds and gen-0006's Animate Guardian's Weapon
#[test]
fn build_controls_match() {
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let mut out = Vec::new();
    let mut paths: Vec<String> = [
        "buildLoadouts",
        "levelScalingButton",
        "back",
        "save",
        "saveAs",
        "mainSkillMinionLibrary",
        "modeImport",
        "modeNotes",
        "modeConfig",
        "modeTree",
        "modeSkills",
        "modeItems",
        "modeCalcs",
        "modeParty",
        "modeCompare",
    ]
    .iter()
    .map(|p| format!("build.{p}"))
    .collect();
    paths.push("build.mainSkillMinion".into());
    let builds: Vec<(String, PathBuf)> = corpus()
        .into_iter()
        .enumerate()
        .filter(|(i, (id, _))| i % 13 == 0 || id == "gen-0006")
        .map(|(_, b)| b)
        .take(6)
        .collect();
    for (id, path) in builds {
        let xml = std::fs::read_to_string(path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        assert!(d.typed_active(), "{id} load: {:?}", d.typed_error);
        let check = |d: &mut Dispatcher, label: &str, out: &mut Vec<String>| {
            let views = json!({"cmd": "controls", "paths": paths}).to_string();
            let (t, tr) = (
                answer(d.request(&views), "typed"),
                answer(d.translated_request(&views), "translated"),
            );
            diff(&t, &tr, &format!("{id} {label} controls"), out);
            compare_views(d, &["state", "outputs"], &format!("{id} {label}"), out);
            for (path, n) in [
                ("build.characterLevel", 1),
                ("build.mainSocketGroup", 12),
                ("build.mainSkillMinion", 3),
            ] {
                for index in 1..=n {
                    let r = json!({"cmd": "controlTooltip", "path": path, "index": index});
                    let (t, tr) = (
                        answer(d.request(&r.to_string()), "typed"),
                        answer(d.translated_request(&r.to_string()), "translated"),
                    );
                    diff(&t, &tr, &format!("{id} {label} {r}"), out);
                }
            }
        };
        let step = |d: &mut Dispatcher, r: J, out: &mut Vec<String>| {
            answer(d.request(&r.to_string()), "typed");
            assert!(d.typed_active(), "{id} {r}: {:?}", d.typed_error);
            check(d, &r.to_string(), out);
        };
        check(&mut d, "load", &mut out);
        if id == "gen-0006" {
            let skills = answer(d.request(r#"{"cmd":"skills"}"#), "skills");
            let group = skills["ok"]["groups"]
                .as_array()
                .unwrap()
                .iter()
                .position(|g| g.to_string().contains("Animate Guardian"))
                .unwrap()
                + 1;
            step(
                &mut d,
                json!({"cmd": "setMainSocketGroup", "index": group}),
                &mut out,
            );
        }
        for r in [
            json!({"cmd": "control", "path": "build.levelScalingButton", "click": true}),
            json!({"cmd": "control", "path": "build.levelScalingButton", "click": true}),
            json!({"cmd": "control", "path": "build.characterLevel", "text": "96"}),
            json!({"cmd": "control", "path": "build.levelScalingButton", "click": true}),
        ] {
            step(&mut d, r, &mut out);
        }
        for m in [
            "Import", "Notes", "Config", "Skills", "Items", "Calcs", "Party", "Compare", "Tree",
        ] {
            step(
                &mut d,
                json!({"cmd": "control", "path": format!("build.mode{m}"), "click": true}),
                &mut out,
            );
        }
        // Each loadout entry but Help, New Loadout's popup saved
        let c = answer(
            d.request(&json!({"cmd": "controls", "paths": ["build.buildLoadouts"]}).to_string()),
            "controls",
        );
        let list: Vec<String> = c["ok"]["build.buildLoadouts"]["list"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        for (k, entry) in list.iter().enumerate() {
            if entry.contains("Help") {
                continue;
            }
            step(
                &mut d,
                json!({"cmd": "control", "path": "build.buildLoadouts", "sel": k + 1}),
                &mut out,
            );
            if entry.contains("New Loadout") {
                step(
                    &mut d,
                    json!({"cmd": "control", "path": "popup.edit", "text": format!("Loadout {k}")}),
                    &mut out,
                );
                step(
                    &mut d,
                    json!({"cmd": "popupButton", "name": "save"}),
                    &mut out,
                );
                // The new loadout picked, if it is listed
                let c = answer(
                    d.request(
                        &json!({"cmd": "controls", "paths": ["build.buildLoadouts"]}).to_string(),
                    ),
                    "controls",
                );
                let name = format!("Loadout {k}");
                let at = c["ok"]["build.buildLoadouts"]["list"]
                    .as_array()
                    .and_then(|l| l.iter().position(|v| v.as_str() == Some(&name)));
                if let Some(at) = at {
                    step(
                        &mut d,
                        json!({"cmd": "control", "path": "build.buildLoadouts", "sel": at + 1}),
                        &mut out,
                    );
                }
            }
        }
        // Whatever popup is left, closed
        while answer(d.request(r#"{"cmd":"state"}"#), "state")["ok"]["popups"]
            .as_array()
            .is_some_and(|p| !p.is_empty())
        {
            step(&mut d, json!({"cmd": "closePopup"}), &mut out);
        }
        compare_saves(&mut d, &id, &mut out);
        // The file saved with the level's auto mode on, loaded again
        let saved = answer(d.request(r#"{"cmd":"saveXml"}"#), "save");
        let xml = saved["ok"]["xml"].as_str().unwrap_or("").to_string();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "reload",
        );
        assert!(d.typed_active(), "{id} reload: {:?}", d.typed_error);
        compare_views(
            &mut d,
            &["state", "outputs"],
            &format!("{id} reloaded"),
            &mut out,
        );
    }
    assert!(out.is_empty(), "{}", out.join("\n"));
}

/// The Skills tab's set drop-down and gem options, in the typed session
/// as in the translated one: their descriptions, the default level's
/// tooltips, the gem list they change, the save, and a reload
#[test]
fn skills_controls_match() {
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let mut out = Vec::new();
    let paths: Vec<String> = [
        "setSelect",
        "setManage",
        "sortGemsByDPS",
        "sortGemsByDPSFieldControl",
        "defaultLevel",
        "defaultQuality",
        "showSupportGemTypes",
        "showLegacyGems",
    ]
    .iter()
    .map(|p| format!("skills.{p}"))
    .collect();
    let mut builds: Vec<(String, Option<PathBuf>)> = vec![("new".into(), None)];
    builds.extend(
        corpus()
            .into_iter()
            .enumerate()
            .filter(|(i, _)| i % 17 == 0)
            .map(|(_, (id, p))| (id, Some(p)))
            .take(4),
    );
    for (id, path) in builds {
        match path {
            Some(path) => {
                let xml = std::fs::read_to_string(path).unwrap();
                answer(
                    d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
                    "load",
                );
            }
            None => {
                answer(d.request(r#"{"cmd":"newBuild"}"#), "newBuild");
            }
        }
        assert!(d.typed_active(), "{id} load: {:?}", d.typed_error);
        let check = |d: &mut Dispatcher, label: &str, out: &mut Vec<String>| {
            let views = json!({"cmd": "controls", "paths": paths}).to_string();
            let (t, tr) = (
                answer(d.request(&views), "typed"),
                answer(d.translated_request(&views), "translated"),
            );
            diff(&t, &tr, &format!("{id} {label} controls"), out);
            compare_views(d, &["state", "skills"], &format!("{id} {label}"), out);
            for index in 0..=6 {
                let r =
                    json!({"cmd": "controlTooltip", "path": "skills.defaultLevel", "index": index});
                let (t, tr) = (
                    answer(d.request(&r.to_string()), "typed"),
                    answer(d.translated_request(&r.to_string()), "translated"),
                );
                diff(&t, &tr, &format!("{id} {label} {r}"), out);
            }
        };
        let step = |d: &mut Dispatcher, r: J, out: &mut Vec<String>| {
            answer(d.request(&r.to_string()), "typed");
            assert!(d.typed_active(), "{id} {r}: {:?}", d.typed_error);
            check(d, &r.to_string(), out);
        };
        // The gem list of the slot after the main group's last gem
        let gem_list = |d: &mut Dispatcher, label: &str, out: &mut Vec<String>| {
            let skills = answer(d.request(r#"{"cmd":"skills"}"#), "skills");
            let group = skills["ok"]["mainSocketGroup"].as_f64().unwrap_or(1.0);
            let Some(gems) = skills["ok"]["groups"][group as usize - 1]["gems"].as_array() else {
                return;
            };
            let r = json!({"cmd": "gemList", "group": group, "slot": gems.len() + 1});
            let (t, tr) = (
                answer(d.request(&r.to_string()), "typed"),
                answer(d.translated_request(&r.to_string()), "translated"),
            );
            diff(&t, &tr, &format!("{id} {label} {r}"), out);
        };
        check(&mut d, "load", &mut out);
        for r in [
            json!({"cmd": "control", "path": "skills.sortGemsByDPS", "state": false}),
            json!({"cmd": "control", "path": "skills.sortGemsByDPSFieldControl", "sel": 4}),
            json!({"cmd": "control", "path": "skills.showSupportGemTypes", "sel": 3}),
            json!({"cmd": "control", "path": "skills.showLegacyGems", "state": true}),
            json!({"cmd": "control", "path": "skills.defaultLevel", "sel": 4}),
            json!({"cmd": "control", "path": "skills.defaultQuality", "text": "15"}),
        ] {
            step(&mut d, r, &mut out);
        }
        // A gem list (its calculations leave selections on the gems), then
        // saved and loaded again
        gem_list(&mut d, "options", &mut out);
        compare_views(
            &mut d,
            &["state", "skills"],
            &format!("{id} gem list"),
            &mut out,
        );
        compare_saves(&mut d, &id, &mut out);
        let saved = answer(d.request(r#"{"cmd":"saveXml"}"#), "save");
        let xml = saved["ok"]["xml"].as_str().unwrap_or("").to_string();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "reload",
        );
        assert!(d.typed_active(), "{id} reload: {:?}", d.typed_error);
        check(&mut d, "reloaded", &mut out);
        for r in [
            json!({"cmd": "control", "path": "skills.sortGemsByDPS", "state": true}),
            json!({"cmd": "control", "path": "skills.sortGemsByDPSFieldControl", "sel": 20}),
            json!({"cmd": "control", "path": "skills.showSupportGemTypes", "sel": 2}),
            json!({"cmd": "control", "path": "skills.defaultLevel", "sel": 0}),
            json!({"cmd": "control", "path": "skills.defaultQuality", "text": "abc"}),
            json!({"cmd": "control", "path": "skills.defaultQuality", "text": "30"}),
            json!({"cmd": "control", "path": "skills.setSelect", "sel": 2}),
            json!({"cmd": "control", "path": "skills.setSelect", "sel": 1}),
        ] {
            step(&mut d, r, &mut out);
        }
        // Not saved: which gem the list calculates last (whose selections
        // stay) is `pairs` order over the slot's gem table, which keeps the
        // order of an earlier, wider fill (random in LuaJIT)
        gem_list(&mut d, "options changed", &mut out);
    }
    assert!(out.is_empty(), "{}", out.join("\n"));
}

/// The Spectre Library (Manage Spectres...), in the typed session as in the
/// translated one: its lists, search, rows' tooltips, adding, dragging,
/// removing, Save and Cancel
#[test]
fn spectre_library_match() {
    let data = data();
    let mut d = Dispatcher::new(session(), Some(data), Prefer::Typed);
    let mut out = Vec::new();
    let builds: Vec<(String, PathBuf)> = corpus()
        .into_iter()
        .enumerate()
        .filter(|(i, (id, _))| i % 23 == 0 || id == "gen-0006")
        .map(|(_, b)| b)
        .take(3)
        .collect();
    for (id, path) in builds {
        let xml = std::fs::read_to_string(path).unwrap();
        answer(
            d.request(&json!({"cmd": "loadXml", "xml": xml}).to_string()),
            "load",
        );
        assert!(d.typed_active(), "{id} load: {:?}", d.typed_error);
        let check = |d: &mut Dispatcher, label: &str, out: &mut Vec<String>| {
            compare_views(d, &["state", "outputs"], &format!("{id} {label}"), out);
            let popups = answer(d.request(r#"{"cmd":"state"}"#), "state")["ok"]["popups"].clone();
            if popups.as_array().is_none_or(|p| p.is_empty()) {
                return;
            }
            for (path, n) in [("popup.list", 4), ("popup.source", 3)] {
                for index in 0..=n {
                    let r = json!({"cmd": "controlTooltip", "path": path, "index": index});
                    let (t, tr) = (
                        answer(d.request(&r.to_string()), "typed"),
                        answer(d.translated_request(&r.to_string()), "translated"),
                    );
                    diff(&t, &tr, &format!("{id} {label} {r}"), out);
                }
            }
        };
        let step = |d: &mut Dispatcher, r: J, out: &mut Vec<String>| {
            answer(d.request(&r.to_string()), "typed");
            assert!(d.typed_active(), "{id} {r}: {:?}", d.typed_error);
            check(d, &r.to_string(), out);
        };
        let c = |path: &str, v: J| {
            let mut r = json!({"cmd": "control", "path": path});
            for (k, x) in v.as_object().unwrap() {
                r[k] = x.clone();
            }
            r
        };
        for r in [
            c("build.mainSkillMinionLibrary", json!({"click": true})),
            c(
                "popup.source",
                json!({"sel": 2, "click": true, "doubleClick": true}),
            ),
            c("popup.source", json!({"sel": 5})),
            c("popup.source.add", json!({"click": true})),
            c("popup.source.add", json!({"click": true})),
            c("popup.source.searchText", json!({"text": "ze"})),
            c(
                "popup.source",
                json!({"sel": 2, "dragTo": "popup.list", "at": 1}),
            ),
            c("popup.source.searchModeDropDown", json!({"sel": 2})),
            c("popup.source.searchText", json!({"text": "fire"})),
            c("popup.source.searchModeDropDown", json!({"sel": 3})),
            c("popup.source", json!({"sel": 1, "dragTo": "popup.list"})),
            c("popup.source.searchText", json!({"text": "zzzz"})),
            c("popup.source.searchText", json!({"text": "("})),
            c("popup.source.searchText", json!({"text": ""})),
            c("popup.list", json!({"sel": 1, "moveTo": 4})),
            c("popup.list", json!({"sel": 2})),
            c("popup.list.delete", json!({"click": true})),
            c("popup.list", json!({"sel": 1})),
            c("popup.list", json!({"key": "DELETE"})),
            c("popup.list", json!({"sel": 9})),
            json!({"cmd": "popupButton", "name": "save"}),
            c("build.mainSkillMinionLibrary", json!({"click": true})),
            c("popup.list", json!({"sel": 1, "key": "DELETE"})),
            c("popup.list", json!({"key": "DELETE"})),
            json!({"cmd": "popupButton", "name": "cancel"}),
            c("build.mainSkillMinionLibrary", json!({"click": true})),
            json!({"cmd": "closePopup"}),
        ] {
            step(&mut d, r, &mut out);
        }
        compare_saves(&mut d, &id, &mut out);
    }
    assert!(out.is_empty(), "{}", out.join("\n"));
}
