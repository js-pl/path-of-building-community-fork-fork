//! A search job on the host's engine thread: a call waiting for its
//! answer holds up no other request, the answer comes once the job is
//! done, and the UI lists and cancels jobs.

use agent::Grant;
use host::FsStore;
use host::control::Call;
use host::ui::{Host, Msg};
use serde_json::{Value as J, json};
use session::typed::TypedData;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

fn data() -> &'static TypedData {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../upstream/PathOfBuilding/src");
    Box::leak(Box::new(TypedData::from_source(&src).unwrap()))
}

/// Sends an agent call; its answer comes on the receiver
fn call(to: &mpsc::Sender<Msg>, grant: &Grant, req: J) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    to.send(Msg::Agent(Call {
        grant: grant.clone(),
        line: req.to_string(),
        reply: tx,
    }))
    .unwrap();
    rx
}

fn answer(rx: &mpsc::Receiver<String>, secs: u64) -> J {
    let a = rx
        .recv_timeout(Duration::from_secs(secs))
        .expect("an answer");
    serde_json::from_str(&a).unwrap()
}

#[test]
fn jobs_hold_up_nothing() {
    let dir = std::env::temp_dir().join(format!("pob-jobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let builds = dir.join("builds");
    std::fs::create_dir_all(builds.join("RF")).unwrap();
    let xml = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/generated/gen-0003.xml"),
    )
    .unwrap();
    std::fs::write(builds.join("RF/build.xml"), xml).unwrap();
    let d = data();
    let (msgs, inbox) = mpsc::channel();
    let to = msgs.clone();
    let client = std::thread::spawn(move || {
        let g = Grant::new("test", "test-2026-11-24-000000").with_build("RF");
        let long = json!({ "verb": "front", "generators": ["tree", "supports"], "candidates": 200, "combine": 6, "beam": 60, "wait": 60, "seconds": 600 });
        // A long search that waits for its answer
        let search = call(&to, &g, long.clone());
        // Answered while the search runs, its answer still to come
        let s = answer(&call(&to, &g, json!({ "verb": "summary" })), 30);
        assert!(s["ok"].is_object(), "{s}");
        assert!(search.try_recv().is_err(), "the search answered first");
        let jobs = answer(&call(&to, &g, json!({ "verb": "job" })), 30);
        let id = jobs["ok"]["jobs"][0]["id"].as_str().unwrap().to_string();
        let c = answer(
            &call(&to, &g, json!({ "verb": "cancel", "job": id, "wait": 0 })),
            30,
        );
        assert!(c["ok"]["job"].is_object(), "{c}");
        // The waiting call gets the answer once the job stopped
        let r = answer(&search, 90);
        assert_eq!(r["ok"]["job"]["status"], "cancelled", "{r}");
        assert!(r["ok"]["rows"].is_array(), "{r}");
        // A long one, not waited for, listed and cancelled by the UI
        let mut started = long.clone();
        started["wait"] = json!(0);
        let r = answer(&call(&to, &g, started), 30);
        let id = r["ok"]["job"]["id"].as_str().unwrap().to_string();
        assert_eq!(r["ok"]["job"]["status"], "running", "{r}");
        let (out, ui) = mpsc::channel();
        to.send(Msg::Open { client: 1, out }).unwrap();
        let next = |ui: &mpsc::Receiver<host::ws::Out>| loop {
            let host::ws::Out::Text(t) = ui.recv_timeout(Duration::from_secs(30)).unwrap() else {
                continue;
            };
            let v: J = serde_json::from_str(&t).unwrap();
            if v.get("id").is_some() {
                return v;
            }
        };
        let ask = |what: &str, args: J| {
            to.send(Msg::Ui {
                client: 1,
                text: json!({ "id": 1, "host": what, "args": args }).to_string(),
            })
            .unwrap();
        };
        ask("jobs", json!({}));
        let jobs = next(&ui);
        assert!(
            jobs["ok"]["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|j| j["id"] == id),
            "{jobs}"
        );
        ask("cancel_job", json!({ "job": id }));
        assert_eq!(next(&ui)["ok"]["cancelled"], true);
        let r = answer(
            &call(&to, &g, json!({ "verb": "job", "job": id, "wait": 60 })),
            90,
        );
        assert_eq!(r["ok"]["job"]["status"], "cancelled", "{r}");
        to.send(Msg::Stop).unwrap();
    });
    drop(msgs);
    let mut h = Host::new(d, FsStore::new(&builds, None));
    h.agent.background_jobs();
    h.run(inbox);
    client.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
