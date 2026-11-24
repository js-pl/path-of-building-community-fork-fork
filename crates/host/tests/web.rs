//! The web app on the host: its files behind the launch's key, the
//! WebSocket's guards, and the UI and an agent on one build: the UI's
//! edits and the agent's steps get one version sequence, every client is
//! told of each change in order, `changes_since` agrees with the events,
//! and the history has the user's coalesced edits and the agent's steps
//! apart.
#![cfg(unix)]

use agent::Grant;
use history::Git;
use host::FsStore;
use host::control::Call;
use host::http::{Request, read_request};
use host::ui::{Host, Msg};
use host::web::{HOST_META, Web, WsClient};
use serde_json::{Value as J, json};
use session::typed::TypedData;
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

fn data() -> &'static TypedData {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../upstream/PathOfBuilding/src");
    Box::leak(Box::new(TypedData::from_source(&src).unwrap()))
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pob-web-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A plain HTTP GET: status, headers and body
fn get(port: u16, target: &str, headers: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{headers}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head.to_string(), body.to_string())
}

/// An agent API call as the control socket and MCP pass it on
fn agent_call(msgs: &mpsc::Sender<Msg>, grant: &Grant, req: J) -> J {
    let (tx, rx) = mpsc::channel();
    msgs.send(Msg::Agent(Call {
        grant: grant.clone(),
        line: req.to_string(),
        reply: tx,
    }))
    .unwrap();
    serde_json::from_str(&rx.recv().unwrap()).unwrap()
}

fn git_log(dir: &std::path::Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "log",
            "--format=%s|%(trailers:key=Source,valueonly,separator=)",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn ui_and_agent_on_one_build() {
    let Some(git) = Git::find() else {
        eprintln!("no git: skipped");
        return;
    };
    let dir = temp("one");
    let builds = dir.join("builds");
    let app = dir.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("index.html"),
        "<!doctype html><html><head><title>t</title></head></html>",
    )
    .unwrap();
    std::fs::write(app.join("main.js"), "1").unwrap();
    let xml = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/generated/gen-0003.xml"),
    )
    .unwrap();
    std::fs::create_dir_all(builds.join("RF")).unwrap();
    std::fs::write(builds.join("RF/build.xml"), &xml).unwrap();

    let mut store = FsStore::new(&builds, Some(git));
    store.set_quiet(Duration::from_millis(300));
    let (msgs, inbox) = mpsc::channel();
    let (listener, port) = host::web::listen("127.0.0.1:0").unwrap();
    let key = host::web::new_key().unwrap();
    let web = Web::new(app, key.clone(), port, msgs.clone());
    std::thread::spawn(move || web.serve(listener));
    let d = data();
    let to = msgs.clone();
    let b = builds.clone();
    let client = std::thread::spawn(move || ui_client(port, key, to, xml, b));
    let mut h = Host::new(d, store);
    h.default_build = Some("RF".into());
    h.run(inbox);
    client.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

fn ui_client(port: u16, key: String, msgs: mpsc::Sender<Msg>, xml: String, builds: PathBuf) {
    let stop = Stop(msgs.clone());

    // The files need the key: the link sets the cookie
    let (status, _, _) = get(port, "/", "");
    assert_eq!(status, 403);
    let (status, _, _) = get(port, "/?key=wrong", "");
    assert_eq!(status, 403);
    let (status, head, _) = get(port, &format!("/?key={key}"), "");
    assert_eq!(status, 303);
    assert!(head.contains("HttpOnly; SameSite=Strict"), "{head}");
    let cookie = format!("Cookie: pob_key={key}\r\n");
    let (status, _, body) = get(port, "/", &cookie);
    assert_eq!(status, 200);
    assert!(body.contains(HOST_META), "{body}");
    assert_eq!(get(port, "/main.js", &cookie).2, "1");
    assert_eq!(get(port, "/../x", &cookie).0, 404);

    // The WebSocket wants the host's own origin and the key
    let own = format!("http://127.0.0.1:{port}");
    let e = WsClient::connect(port, &key, "https://evil.example")
        .err()
        .unwrap();
    assert!(e.contains("403"), "{e}");
    let e = WsClient::connect(port, "nope", &own).err().unwrap();
    assert!(e.contains("403"), "{e}");

    let mut ui = WsClient::connect(port, &key, &own).unwrap();
    let mut other = WsClient::connect(port, &key, &format!("http://localhost:{port}")).unwrap();
    let hello = ui.recv().unwrap();
    assert_eq!(hello["event"], "hello", "{hello}");
    assert_eq!(hello["build"], "RF");
    assert_eq!(hello["version"], 0);
    assert_eq!(hello["history"], true);
    assert_eq!(other.recv().unwrap()["event"], "hello");

    // A read: no change
    ui.send(&json!({ "id": 1, "req": { "cmd": "state" } }).to_string())
        .unwrap();
    let a = ui.recv().unwrap();
    assert_eq!(a["id"], 1);
    assert!(a["ok"]["name"].is_string(), "{a}");
    assert_eq!(a["version"], 0);

    // The UI's edit: its answer, then its change event, to every client
    ui.send(&json!({ "id": 2, "req": { "cmd": "nodeTooltip", "id": 36634 } }).to_string())
        .unwrap();
    let a = ui.recv().unwrap();
    assert_eq!(a["id"], 2);
    ui.send(&json!({ "id": 3, "req": { "cmd": "clickNode", "id": 36634 } }).to_string())
        .unwrap();
    let a = ui.recv().unwrap();
    assert_eq!(a["id"], 3, "{a}");
    assert!(a.get("error").is_none(), "{a}");
    assert_eq!(a["version"], 1, "{a}");
    let e = ui.recv().unwrap();
    assert_eq!(e["event"], "change", "{e}");
    assert_eq!(e["version"], 1);
    assert_eq!(e["source"], "user");
    assert!(
        e["label"].as_str().unwrap().starts_with("allocate #36634"),
        "{e}"
    );
    assert_eq!(other.recv().unwrap(), e);

    // An agent's step: pushed to the clients, after the UI's
    let grant = Grant::new("claude", "claude-1").with_build("RF");
    let r = agent_call(&msgs, &grant, json!({ "verb": "set_level", "level": 90 }));
    assert_eq!(r["ok"]["version"], 2, "{r}");
    let e = ui.recv().unwrap();
    assert_eq!(e["version"], 2, "{e}");
    assert_eq!(e["source"], "agent claude");
    assert!(e["commit"].is_string(), "{e}");
    assert_eq!(other.recv().unwrap(), e);

    // The UI undoes the agent's step
    ui.send(&json!({ "id": 4, "req": { "cmd": "undo" } }).to_string())
        .unwrap();
    let a = ui.recv().unwrap();
    assert_eq!(a["version"], 3, "{a}");
    let e = ui.recv().unwrap();
    assert_eq!(e["version"], 3);
    assert_eq!(e["source"], "user");
    assert!(
        e["label"].as_str().unwrap().starts_with("undo level 90"),
        "{e}"
    );
    other.recv().unwrap();

    // `changes_since` gives what the events gave
    let r = agent_call(
        &msgs,
        &grant,
        json!({ "verb": "changes_since", "version": 0 }),
    );
    let changes = r["ok"]["changes"].as_array().unwrap();
    let got: Vec<(u64, &str)> = changes
        .iter()
        .map(|c| {
            (
                c["version"].as_u64().unwrap(),
                c["source"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(got, [(1, "user"), (2, "agent claude"), (3, "user")], "{r}");

    // The history: the build as found, the user's edit committed before
    // the agent's step, the step, and the undo once the user was quiet
    std::thread::sleep(Duration::from_millis(1200));
    let log = git_log(&builds.join("RF"));
    assert_eq!(log.len(), 4, "{log:?}");
    assert!(
        log[0].starts_with("undo level 90") && log[0].ends_with("|user"),
        "{log:?}"
    );
    assert!(
        log[1].starts_with("level 90") && log[1].ends_with("|agent claude"),
        "{log:?}"
    );
    assert!(
        log[2].starts_with("allocate #36634") && log[2].ends_with("|user"),
        "{log:?}"
    );
    assert!(log[3].ends_with("|user"), "{log:?}");

    // The undo list with its sources, the history, and a restore
    ui.send(&json!({ "id": 10, "host": "steps", "args": {} }).to_string())
        .unwrap();
    let a = ui.recv().unwrap();
    let sources = |key: &str| -> Vec<String> {
        a["ok"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["source"].as_str().unwrap().to_string())
            .collect()
    };
    // The undo took the agent's step back
    assert_eq!(sources("steps"), ["user"], "{a}");
    assert_eq!(sources("changes"), ["user", "agent claude", "user"], "{a}");
    ui.send(&json!({ "id": 11, "host": "history", "args": { "n": 10 } }).to_string())
        .unwrap();
    let a = ui.recv().unwrap();
    assert_eq!(a["ok"]["enabled"], true, "{a}");
    assert_eq!(a["ok"]["pending"], false, "{a}");
    let versions = a["ok"]["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 4, "{a}");
    let found = versions[3]["version"].as_str().unwrap();
    ui.send(&json!({ "id": 12, "host": "restore", "args": { "version": found } }).to_string())
        .unwrap();
    let a = ui.recv().unwrap();
    assert!(
        a["ok"]["label"].as_str().unwrap().starts_with("restore "),
        "{a}"
    );
    let e = ui.recv().unwrap();
    assert_eq!(
        (e["version"].as_u64(), e["source"].as_str()),
        (Some(4), Some("user")),
        "{e}"
    );
    other.recv().unwrap();
    // Committed at once, on top: the history only grows
    let log = git_log(&builds.join("RF"));
    assert_eq!(log.len(), 5, "{log:?}");
    assert!(
        log[0].starts_with("restore ") && log[0].ends_with("|restore"),
        "{log:?}"
    );
    // The session's save of that version, as committed
    let restored = std::fs::read_to_string(builds.join("RF/build.xml")).unwrap();
    assert_eq!(restored, xml_at(&builds, "HEAD"));
    // Without the node the UI allocated, which the version before had
    assert!(xml_at(&builds, "HEAD~1").contains("36634"));
    assert!(!restored.contains("36634"));

    // The UI's file is the session's build
    ui.send(&json!({ "id": 5, "req": { "cmd": "saveXml" } }).to_string())
        .unwrap();
    let a = ui.recv().unwrap();
    let saved = std::fs::read_to_string(builds.join("RF/build.xml")).unwrap();
    assert_eq!(a["ok"]["xml"].as_str().unwrap(), saved);

    // A build loaded in the UI is a new build, not a replacement
    ui.send(&json!({ "id": 6, "req": { "cmd": "loadXml", "xml": xml, "name": "RF" } }).to_string())
        .unwrap();
    let e = ui.recv().unwrap();
    assert_eq!(e["event"], "open", "{e}");
    assert_eq!(e["build"], "RF (2)");
    let a = ui.recv().unwrap();
    assert_eq!(a["ok"]["build"], "RF (2)", "{a}");
    assert_eq!(
        std::fs::read_to_string(builds.join("RF/build.xml")).unwrap(),
        saved
    );
    assert!(builds.join("RF (2)/build.xml").exists());

    drop(stop);
}

/// Stops the host when the client is done, or failed
struct Stop(mpsc::Sender<Msg>);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = self.0.send(Msg::Stop);
    }
}

#[test]
fn requests_keep_their_query() {
    let raw = "GET /?key=abc&x=1 HTTP/1.1\r\n\r\n";
    let r: Request = read_request(&mut BufReader::new(raw.as_bytes()))
        .unwrap()
        .unwrap();
    assert_eq!((r.path.as_str(), r.query.as_str()), ("/", "key=abc&x=1"));
}

/// A build's XML at a version, from its repository
fn xml_at(builds: &std::path::Path, version: &str) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(builds.join("RF"))
        .args(["show", &format!("{version}:build.xml")])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}
