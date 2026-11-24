//! The host on a builds directory with git: a client on the control socket
//! edits a build, each step is committed before its answer, and a host
//! started again reads the saved build.

use agent::{Agent, BuildId, Store};
use history::Git;
use host::FsStore;
use host::control;
use host::local;
use serde_json::json;
use session::typed::TypedData;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

fn data() -> &'static TypedData {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../upstream/PathOfBuilding/src");
    Box::leak(Box::new(TypedData::from_source(&src).unwrap()))
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pob-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Serves the store on `socket` until `client` is done with it
fn serve(
    data: &'static TypedData,
    store: FsStore,
    socket: &Path,
    client: impl FnOnce() + Send + 'static,
) -> FsStore {
    let mut listener = local::listen(socket).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let (tx, rx) = mpsc::channel();
    // One connection, then the senders are gone and `answer` returns
    std::thread::spawn(move || {
        let stream = listener.accept().unwrap();
        let _ = control::connection(stream, tx, 0);
    });
    let c = std::thread::spawn(client);
    let mut agent = Agent::new(data, store);
    control::answer(&mut agent, rx);
    c.join().unwrap();
    let mut store = std::mem::replace(agent.store_mut(), FsStore::new("/nonexistent", None));
    store.close();
    store
}

#[test]
fn host_on_the_control_socket() {
    let Some(git) = Git::find() else {
        eprintln!("no git: skipped");
        return;
    };
    let d = data();
    let dir = temp("socket");
    let builds = dir.join("builds");
    let xml = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/generated/gen-0003.xml"),
    )
    .unwrap();
    std::fs::create_dir_all(builds.join("RF")).unwrap();
    std::fs::write(builds.join("RF/build.xml"), &xml).unwrap();
    let socket = local::endpoint_in(&dir.join("run"), "control.sock");

    let store = FsStore::new(&builds, Some(git.clone()));
    let s = socket.clone();
    let store = serve(d, store, &socket, move || {
        let mut c = local::Client::connect(
            &s,
            &json!({ "hello": { "harness": "ctl", "builds": ["RF"], "agent": "mine" } }),
        )
        .unwrap();
        assert_eq!(c.hello["agent_directory"], "mine");
        let r = c
            .call(&json!({ "verb": "allocate", "nodes": [36634] }))
            .unwrap();
        let commit = r["ok"]["commit"].as_str().unwrap().to_string();
        // Committed before the answer came
        let log = std::process::Command::new("git")
            .args(["-C"])
            .arg(s.parent().unwrap().parent().unwrap().join("builds/RF"))
            .args(["log", "-1", "--format=%h %s%n%b"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout).to_string();
        assert!(log.starts_with(&commit[..7]), "{log} {commit}");
        assert!(log.contains("allocate "), "{log}");
        assert!(log.contains("Source: agent ctl"), "{log}");
        let r = c.call(&json!({ "verb": "copy", "name": "try" })).unwrap();
        assert_eq!(r["ok"]["build"], "agent/try", "{r}");
        let r = c
            .call(&json!({ "verb": "summary", "build": "Other" }))
            .unwrap();
        assert!(r["error"].as_str().unwrap().contains("grant"));
        let r = c.call(&json!({ "verb": "history" })).unwrap();
        // The build as found, then the step
        let versions = r["ok"]["versions"].as_array().unwrap();
        assert_eq!(versions.len(), 2, "{r}");
        assert_eq!(versions[1]["source"], "user", "{r}");
    });
    assert!(builds.join("agents/mine/try/.git").exists());
    assert!(builds.join("agents/mine/try/build.xml").exists());
    let saved = store.read(&BuildId::user("RF")).unwrap().unwrap();
    assert_ne!(saved, xml);

    // Started again: the saved build, and its history
    let store = FsStore::new(&builds, Some(git));
    let s = socket.clone();
    serve(d, store, &socket, move || {
        let mut c = local::Client::connect(
            &s,
            &json!({ "hello": { "harness": "ctl", "builds": ["RF"] } }),
        )
        .unwrap();
        let r = c
            .call(&json!({ "verb": "search", "kind": "nodes", "query": "", "limit": 1 }))
            .unwrap();
        assert!(r.get("ok").is_some(), "{r}");
        let r = c.call(&json!({ "verb": "undo" })).unwrap();
        assert!(r["error"].as_str().unwrap().contains("history"), "{r}");
        let r = c.call(&json!({ "verb": "history" })).unwrap();
        let versions = r["ok"]["versions"].as_array().unwrap().clone();
        assert_eq!(versions.len(), 2, "{r}");
        // A version from before the restart, restored as a new one
        let r = c
            .call(&json!({ "verb": "deallocate", "nodes": [36634] }))
            .unwrap();
        assert!(r.get("ok").is_some(), "{r}");
        let v = versions[0]["version"].as_str().unwrap();
        let r = c.call(&json!({ "verb": "restore", "version": v })).unwrap();
        assert!(
            r["ok"]["label"].as_str().unwrap().starts_with("restore "),
            "{r}"
        );
        let r = c.call(&json!({ "verb": "history" })).unwrap();
        assert_eq!(r["ok"]["versions"][0]["source"], "restore", "{r}");
        assert_eq!(r["ok"]["versions"].as_array().unwrap().len(), 4, "{r}");
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_git_the_files_are_still_written() {
    let dir = temp("nogit");
    let mut store = FsStore::new(&dir, None);
    let step = agent::Step {
        label: "import".into(),
        agent: "t".into(),
        restore: false,
        redo: false,
        trailers: vec![],
    };
    let id = BuildId::agent("a", "x");
    assert_eq!(store.create(&id, "<xml/>", &step).unwrap(), None);
    assert_eq!(store.read(&id).unwrap().as_deref(), Some("<xml/>"));
    assert!(store.create(&id, "<xml/>", &step).is_err());
    assert!(store.log(&id, 5).unwrap().is_empty());
    assert_eq!(store.list(&agent::Owner::Agent("a".into())).unwrap(), ["x"]);
    assert!(store.read(&BuildId::user("agents")).is_err());
    assert!(store.read(&BuildId::user("../x")).is_err());
}
