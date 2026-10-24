//! The engine API on corpus builds: every command runs, edits behave as in
//! upstream's UI, and builds survive saving and loading.
//!
//! `SESSION_BUILD=<path>` picks the build, relative to the repository root
//! (default `corpus/generated/gen-0001.xml`).

use serde_json::{Value as J, json};
use session::Session;
use std::path::PathBuf;
use std::rc::Rc;
use upstream::imported::Imported;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn session() -> Session {
    let src = root().join("upstream/PathOfBuilding/src");
    Session::new(Rc::new(Imported::new(src))).unwrap_or_else(|e| panic!("{e}"))
}

fn call(s: &Session, req: J) -> J {
    let text = s
        .request(&req.to_string())
        .unwrap_or_else(|e| panic!("{e}"));
    let v: J = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    if let Some(e) = v.get("error") {
        panic!("{} failed: {e}", req["cmd"]);
    }
    v["ok"].clone()
}

fn corpus_xml() -> String {
    let rel =
        std::env::var("SESSION_BUILD").unwrap_or_else(|_| "corpus/generated/gen-0001.xml".into());
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn stats(s: &Session) -> J {
    call(s, json!({"cmd": "state"}))["stats"].clone()
}

#[test]
fn session_commands() {
    let s = session();
    call(
        &s,
        json!({"cmd": "loadXml", "xml": corpus_xml(), "name": "test"}),
    );
    let state = call(&s, json!({"cmd": "state"}));
    let before = state["stats"].clone();
    assert!(
        before.as_array().unwrap().len() > 5,
        "no sidebar stats: {state}"
    );

    // Build codes round trip
    let code = call(&s, json!({"cmd": "saveCode"}))["code"]
        .as_str()
        .unwrap()
        .to_string();
    call(&s, json!({"cmd": "loadCode", "code": code}));
    assert_eq!(
        stats(&s),
        before,
        "stats changed after a build code round trip"
    );

    // Tree: geometry, allocating a node next to the tree and undoing it
    let tree = call(&s, json!({"cmd": "tree"}));
    assert!(tree["nodes"].as_array().unwrap().len() > 1000);
    assert!(tree["connectors"].as_array().unwrap().len() > 1000);
    let ts = call(&s, json!({"cmd": "treeState"}));
    let alloc: Vec<i64> = ts["alloc"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    let mut target = None;
    for c in tree["connectors"].as_array().unwrap() {
        let (a, b) = (c["a"].as_i64().unwrap(), c["b"].as_i64().unwrap());
        let candidate = if alloc.contains(&a) && !alloc.contains(&b) {
            b
        } else if alloc.contains(&b) && !alloc.contains(&a) {
            a
        } else {
            continue;
        };
        let node = tree["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"].as_i64() == Some(candidate));
        if node.is_some_and(|n| n["kind"] == "Normal" || n["kind"] == "Notable") {
            target = Some(candidate);
            break;
        }
    }
    let target = target.expect("a node next to the allocated tree");
    let tip = call(&s, json!({"cmd": "nodeTooltip", "id": target}));
    assert!(!tip["lines"].as_array().unwrap().is_empty());
    call(&s, json!({"cmd": "clickNode", "id": target}));
    let ts2 = call(&s, json!({"cmd": "treeState"}));
    assert!(
        ts2["alloc"].as_array().unwrap().contains(&json!(target)),
        "node {target} was not allocated"
    );
    call(&s, json!({"cmd": "clickNode", "id": target}));
    assert_eq!(
        stats(&s),
        before,
        "stats changed after allocating and deallocating a node"
    );

    // Node power
    let power = call(&s, json!({"cmd": "nodePower", "index": 1}));
    assert!(!power["nodes"].as_object().unwrap().is_empty());

    // Calcs tab and a breakdown
    let calcs = call(&s, json!({"cmd": "calcs"}));
    let sections = calcs["sections"].as_array().unwrap();
    assert!(sections.len() > 5);
    let mut cell = None;
    'find: for sec in sections {
        for sub in sec["subs"].as_array().unwrap() {
            for row in sub["rows"].as_array().unwrap() {
                for c in row["cells"].as_array().unwrap() {
                    if !c["breakdown"].is_null() {
                        cell = Some(c["breakdown"].clone());
                        break 'find;
                    }
                }
            }
        }
    }
    let bd = call(&s, json!({"cmd": "breakdown", "cell": cell.unwrap()}));
    assert!(bd["sections"].is_array());

    // Config: toggle the first checkbox shown and back
    let config = call(&s, json!({"cmd": "config"}));
    let check = config["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|sec| sec["vars"].as_array().unwrap().clone())
        .find(|v| v["class"] == "CheckBoxControl" && v["state"] == false)
        .expect("a config checkbox");
    let path = format!("config.{}", check["var"].as_str().unwrap());
    call(&s, json!({"cmd": "control", "path": path, "state": true}));
    call(&s, json!({"cmd": "control", "path": path, "state": false}));
    assert_eq!(
        stats(&s),
        before,
        "stats changed after toggling a config option twice"
    );

    // Skills and items
    let skills = call(&s, json!({"cmd": "skills"}));
    assert!(!skills["groups"].as_array().unwrap().is_empty());
    let tip = call(&s, json!({"cmd": "socketGroupTooltip", "index": 1}));
    assert!(tip["lines"].is_array());
    let items = call(&s, json!({"cmd": "items"}));
    if let Some(item) = items["items"].as_array().unwrap().first() {
        let tip = call(&s, json!({"cmd": "itemTooltip", "id": item["id"]}));
        assert!(!tip["lines"].as_array().unwrap().is_empty());
    }
    call(&s, json!({"cmd": "setNotes", "text": "notes"}));
    assert_eq!(call(&s, json!({"cmd": "notes"}))["text"], "notes");

    // Saved XML loads back to the same stats
    let xml = call(&s, json!({"cmd": "saveXml"}))["xml"].clone();
    call(&s, json!({"cmd": "loadXml", "xml": xml}));
    assert_eq!(stats(&s), before, "stats changed after an XML round trip");

    // Errors are reported, not thrown
    let r = s.request(r#"{"cmd":"clickNode","id":-1}"#).unwrap();
    assert!(r.contains("\"error\""), "{r}");
}

#[test]
fn bundle_round_trip() {
    let packed = session::bundle::pack([("a.lua", &b"return 1"[..]), ("b/c.txt", b"x")]);
    let files = session::Files::new();
    assert_eq!(session::bundle::unpack(&packed, &files).unwrap(), 2);
    use upstream::host::DataSource;
    assert_eq!(files.read_file("b/c.txt").unwrap(), b"x");
    assert!(files.read_file("missing").is_none());
    assert_eq!(files.take_missing(), vec!["missing".to_string()]);
}

/// Data files the engine lacks are reported, and the request succeeds once
/// they are added, as the browser fetches older tree versions.
#[test]
fn missing_files_retry() {
    let src = root().join("upstream/PathOfBuilding/src");
    let files = Rc::new(session::Files::new());
    // Everything but the 3.13 tree
    fn add(dir: &std::path::Path, rel: &str, files: &session::Files) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if p.is_dir() {
                if rel != "TreeData/3_13" {
                    add(&p, &rel, files);
                }
            } else if !(name.ends_with(".png")
                || name.ends_with(".jpg")
                || name.ends_with(".dds")
                || name.ends_with(".zst"))
            {
                files.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    add(&src, "", &files);
    files.take_missing();
    let s = Session::new(files.clone()).unwrap_or_else(|e| panic!("{e}"));
    let xml = std::fs::read_to_string(
        root().join("upstream/PathOfBuilding/spec/TestBuilds/3.13/OccVortex.xml"),
    )
    .unwrap();
    let req = json!({"cmd": "loadXml", "xml": xml}).to_string();
    let r = s.request(&req).unwrap();
    assert!(r.contains("\"error\""), "{r}");
    let missing = files.take_missing();
    assert!(
        missing.iter().any(|m| m.starts_with("TreeData/3_13/")),
        "{missing:?}"
    );
    for m in &missing {
        if let Ok(data) = std::fs::read(src.join(m)) {
            files.insert(m.clone(), data);
        }
    }
    let r = s.request(&req).unwrap();
    assert!(!r.contains("\"error\""), "{r}");
    let want = session();
    call(&want, json!({"cmd": "loadXml", "xml": xml}));
    assert_eq!(stats(&s), stats(&want));
}
