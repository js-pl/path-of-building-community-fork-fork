//! The control socket on Windows, a named pipe: a client says hello and
//! calls a verb; a second host can't take the pipe; a name that isn't a
//! pipe is refused.
#![cfg(windows)]

use host::connections::Registry;
use host::control::pipe;
use serde_json::{Value as J, json};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

#[test]
fn control_pipe() {
    let path = PathBuf::from(format!(
        r"\\.\pipe\pob-test-{}-{}",
        std::process::id(),
        host::control::now()
    ));
    let listener = pipe::listen(&path).unwrap();
    let again = pipe::listen(&path).err().unwrap();
    assert!(again.contains("running already"), "{again}");
    let (tx, rx) = mpsc::channel::<host::control::Call>();
    let registry = Arc::new(Registry::default());
    let reg = registry.clone();
    std::thread::spawn(move || pipe::accept(listener, tx, reg));
    // The engine's side: echoes each call's grant
    std::thread::spawn(move || {
        for c in rx {
            let _ = c
                .reply
                .send(json!({ "ok": { "builds": c.grant.builds, "line": c.line } }).to_string());
        }
    });

    let hello = json!({ "hello": { "harness": "ctl", "builds": ["RF"] } });
    let mut c = pipe::Client::connect(&path, &hello).unwrap();
    assert_eq!(c.hello["connection"], "ctl");
    let a = c.call(&json!({ "verb": "summary" })).unwrap();
    assert_eq!(a["ok"]["builds"], json!(["RF"]));
    // Two clients at once
    let mut d = pipe::Client::connect(&path, &hello).unwrap();
    assert!(d.call(&json!({ "verb": "tree" })).unwrap()["ok"].is_object());
    assert!(pipe::answers(&path));
    let list = pipe::admin(&path, json!("connections")).unwrap();
    let n = list["connections"].as_array().unwrap().len();
    assert_eq!(n, 2, "{list}");

    // Disconnecting one closes it
    let id = list["connections"][0]["id"].as_u64().unwrap();
    let closed = pipe::admin(&path, json!({ "disconnect": id })).unwrap();
    assert_eq!(closed["closed"], J::Bool(true));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let closed_c = loop {
        let gone = c.call(&json!({ "verb": "summary" })).is_err();
        if gone || std::time::Instant::now() > deadline {
            break gone;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(closed_c, "the disconnected client still gets answers");
    // The other client is untouched
    assert!(d.call(&json!({ "verb": "tree" })).unwrap()["ok"].is_object());

    assert!(pipe::listen(&PathBuf::from(r"C:\control.sock")).is_err());
}
