//! MCP clients on the host: `pob mcp`'s stdio relay over the control
//! socket, through initialize, the tools, a read and an edit that commits;
//! a grant reaches no other build.
#![cfg(unix)]

use agent::Agent;
use history::Git;
use host::FsStore;
use host::control::{self, unix};
use serde_json::{Value as J, json};
use session::typed::TypedData;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

fn data() -> &'static TypedData {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../upstream/PathOfBuilding/src");
    Box::leak(Box::new(TypedData::from_source(&src).unwrap()))
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pob-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A builds directory with the builds `RF` and `Other`
fn builds(dir: &Path) -> PathBuf {
    let builds = dir.join("builds");
    let xml = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/generated/gen-0003.xml"),
    )
    .unwrap();
    for b in ["RF", "Other"] {
        std::fs::create_dir_all(builds.join(b)).unwrap();
        std::fs::write(builds.join(b).join("build.xml"), &xml).unwrap();
    }
    builds
}

fn last_commit(repo: &Path) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "-1", "--format=%h %s%n%b"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn rpc(id: u64, method: &str, params: J) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

#[test]
fn mcp_on_stdio() {
    let git = Git::find();
    let dir = temp("stdio");
    let builds = builds(&dir);
    let socket = dir.join("run/control.sock");
    let listener = unix::listen(&socket).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let _ = control::connection(stream, tx, 0);
    });

    // The harness's side: its messages, one per line, as `pob mcp` reads them
    let input = [
        rpc(
            1,
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "claude-code", "version": "2" } }),
        ),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        rpc(2, "tools/list", json!({})),
        rpc(
            3,
            "tools/call",
            json!({ "name": "summary", "arguments": {} }),
        ),
        rpc(
            4,
            "tools/call",
            json!({ "name": "allocate", "arguments": { "nodes": [36634] } }),
        ),
        rpc(
            5,
            "tools/call",
            json!({ "name": "summary", "arguments": { "build": "Other" } }),
        ),
        rpc(6, "tools/call", json!({ "name": "notes", "arguments": {} })),
    ]
    .join("\n");
    let s = socket.clone();
    let relay = std::thread::spawn(move || {
        let mut out = Vec::new();
        host::mcp::stdio(input.as_bytes(), &mut out, |harness: &str| {
            let hello = json!({ "hello": { "harness": harness, "builds": ["RF"] } });
            let mut c = unix::Client::connect(&s, &hello)?;
            assert_eq!(c.hello["connection"], "claude-code");
            Ok(Box::new(move |req: &J| c.call(req)) as host::mcp::Relay)
        })
        .unwrap();
        out
    });
    let mut agent = Agent::new(data(), FsStore::new(&builds, git.clone()));
    control::answer(&mut agent, rx);
    let out = relay.join().unwrap();
    let answers: Vec<J> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(answers.len(), 6, "{answers:?}");
    let by_id = |id: u64| answers.iter().find(|a| a["id"] == id).unwrap();

    let init = &by_id(1)["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert!(init["capabilities"]["tools"].is_object());
    assert!(init["instructions"].as_str().unwrap().contains("untrusted"));
    let tools = by_id(2)["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "allocate"));

    let summary = &by_id(3)["result"];
    assert_ne!(summary["isError"], true, "{summary}");
    assert_eq!(summary["structuredContent"]["build"], "RF", "{summary}");

    let edit = &by_id(4)["result"];
    assert_ne!(edit["isError"], true, "{edit}");
    let commit = edit["structuredContent"]["commit"].as_str();
    if git.is_some() {
        let commit = commit.unwrap();
        // Committed before the answer, by the harness's connection
        let log = last_commit(&builds.join("RF"));
        assert!(log.starts_with(&commit[..7]), "{log} {commit}");
        assert!(log.contains("Source: agent claude-code"), "{log}");
    }
    let saved = std::fs::read_to_string(builds.join("RF/build.xml")).unwrap();
    let other = std::fs::read_to_string(builds.join("Other/build.xml")).unwrap();
    assert_ne!(saved, other);

    // Outside the grant
    let refused = &by_id(5)["result"];
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("grant")
    );

    // The author's text stays marked
    let notes = &by_id(6)["result"];
    assert!(
        notes["structuredContent"].get("untrusted").is_some(),
        "{notes}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stdio_without_a_host() {
    let input = [
        rpc(1, "initialize", json!({ "protocolVersion": "2025-11-25" })),
        rpc(2, "tools/call", json!({ "name": "summary" })),
    ]
    .join("\n");
    let mut out = Vec::new();
    host::mcp::stdio(input.as_bytes(), &mut out, |_: &str| {
        Err("no host on /x (start one with `pob host --headless`)".to_string())
    })
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    let last: J = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    assert_eq!(last["result"]["isError"], true);
    assert!(
        last["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("pob host")
    );
}
