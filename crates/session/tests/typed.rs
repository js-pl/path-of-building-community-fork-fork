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

fn session() -> Session {
    Session::new(Rc::new(Imported::new(src()))).unwrap_or_else(|e| panic!("{e}"))
}

/// The phases of `typed_session_matches_translated`, with the number of
/// shards each splits the corpus into
const PHASES: [(&str, usize); 9] = [
    ("commands", 2),
    ("geometry", 3),
    ("calcs", 3),
    ("tree", 3),
    ("skills", 3),
    ("save", 3),
    ("items", 3),
    ("config", 3),
    ("new", 1),
];

/// The typed engine's commands against the translated session's: the
/// sidebar and outputs, the tree's geometry, the Calcs tab, tree, skills
/// and item edits and the saved files, each over the corpus. Each phase runs on shards of the
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
            &[" builds", " edits", " trees", "skipped ", "fallback"],
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
        "tree" => tree_edits_match(&mut d),
        "skills" => skills_edits_match(&mut d),
        "save" => saves_match(&mut d),
        "items" => items_edits_match(&mut d),
        "config" => config_edits_match(&mut d),
        "new" => new_build_matches(&mut d),
        _ => panic!("unknown phase {phase}"),
    }
    at_build(&mut d, "");
    report_fallbacks(phase);
}

/// The fallbacks the typed session still has, as substrings of their
/// reasons: any other fails the test. Remove one once it is typed.
const KNOWN_FALLBACKS: &[&str] = &["crafting a cluster jewel"];

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
        return if rng.below(2) == 0 {
            json!({"cmd": "popupButton", "name": "confirm"})
        } else {
            json!({"cmd": "closePopup"})
        };
    }
    let vars: Vec<&J> = config["ok"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["vars"].as_array().unwrap())
        .collect();
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
                break;
            }
            let t = Instant::now();
            answer(d.translated_request(r#"{"cmd":"treeState"}"#), "replay");
            times[1] += t.elapsed();
            let _ = typed;
            compare_views(d, &CONFIG_VIEWS, &label, &mut out);
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
    let steps_per_build: usize = std::env::var("SESSION_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    // Item texts of the builds so far, for adding and editing items
    let mut pool: Vec<J> = Vec::new();
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
        let mut rng = Lcg(seed(&id) + 11);
        let mut out = Vec::new();
        let compare = |d: &mut Dispatcher, req: &J, label: &str, out: &mut Vec<String>| {
            let r = req.to_string();
            let typed = answer(d.request(&r), "typed");
            let tr = answer(d.translated_request(&r), "translated");
            diff(&typed, &tr, &format!("{label} {r}"), out);
            typed
        };
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
            let req = match rng.below(12) {
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
                    let text = &pool[rng.below(pool.len())];
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
                    json!({"cmd": "saveDisplayItem"})
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
            times[0] += t.elapsed();
            if !d.typed_active() {
                let e = d.typed_error.clone().unwrap_or_default();
                assert!(e.starts_with("not ported"), "{id}: {label} {r}: {e}");
                skipped.push(format!("{id}: {r}: {e}"));
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
        "{steps} item and set edits: typed {:.1?}, translated {:.1?} (with its replays)\nskipped:\n{}",
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

/// `saveXml` of both on the dispatcher's build: where they differ
fn compare_saves(d: &mut Dispatcher, label: &str, out: &mut Vec<String>) {
    let typed = answer(d.request(r#"{"cmd":"saveXml"}"#), "typed save");
    let tr = answer(
        d.translated_request(r#"{"cmd":"saveXml"}"#),
        "translated save",
    );
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

/// `loadXml`, `loadCode`, `state` and `outputs`
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
