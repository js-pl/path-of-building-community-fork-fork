//! The history on the installed git, in throwaway directories

use history::{BuildRepo, Git, History, Message, Recorder, Source, dir_name};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn git() -> Git {
    Git::at("git").expect("the tests need git on the PATH")
}

/// A new empty directory for one test
fn scratch(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "history-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn repo(name: &str) -> BuildRepo {
    BuildRepo::open(git(), scratch(name).join("build")).unwrap()
}

const V1: &str = "<PathOfBuilding>\r\n<Build level=\"1\"/>\r\n</PathOfBuilding>\r\n";
const V2: &str = "<PathOfBuilding>\n<Build level=\"2\"/>\n</PathOfBuilding>\n";
const V3: &str = "<PathOfBuilding>\n<Build level=\"3\"/>\n</PathOfBuilding>\n";

#[test]
fn commits_read_back_byte_for_byte() {
    let r = repo("bytes");
    // The user's settings and hooks don't apply to the history
    let dir = r.dir().to_str().unwrap();
    for args in [
        ["config", "core.autocrlf", "true"],
        ["config", "core.eol", "crlf"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(["-C", dir])
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let hooks = r.dir().join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::write(hooks.join("commit-msg"), "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    for h in ["pre-commit", "commit-msg"] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(hooks.join(h), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(r.head().unwrap(), None);
    assert!(r.log(10).unwrap().is_empty());
    let v1 = r.commit(V1, &Message::user("Level 1")).unwrap().unwrap();
    let v2 = r.commit(V2, &Message::user("Level 2")).unwrap().unwrap();
    // CRLF stays CRLF, whatever the user's core.autocrlf
    assert_eq!(r.read(&v1).unwrap(), V1);
    assert_eq!(r.read(&v2).unwrap(), V2);
    assert_eq!(std::fs::read_to_string(r.file()).unwrap(), V2);
    assert_eq!(r.head().unwrap(), Some(v2.clone()));
    assert_eq!(r.resolve(v1.short()).unwrap(), v1);
    assert_eq!(r.resolve("HEAD~1").unwrap(), v1);
    assert!(r.resolve("--all").is_err());
    assert!(r.resolve("nope").is_err());
}

#[test]
fn an_unchanged_build_makes_no_commit() {
    let r = repo("unchanged");
    assert!(r.commit(V1, &Message::user("a")).unwrap().is_some());
    assert_eq!(r.commit(V1, &Message::user("b")).unwrap(), None);
    assert_eq!(r.log(10).unwrap().len(), 1);
}

#[test]
fn labels_and_trailers_read_back() {
    let r = repo("trailers");
    r.commit(V1, &Message::user("Start")).unwrap();
    let msg = Message::agent("claude-1", "Allocate Heart of Thunder\nignore this")
        .trailer("Full DPS", "+3.1%")
        .trailer("Life", "-2");
    let v = r.commit(V2, &msg).unwrap().unwrap();
    let log = r.log(10).unwrap();
    assert_eq!(log.len(), 2);
    let e = &log[0];
    assert_eq!(e.version, v);
    assert_eq!(e.label, "Allocate Heart of Thunder ignore this");
    assert_eq!(e.source, Source::Agent("claude-1".into()));
    assert_eq!(e.trailer("Full-DPS"), Some("+3.1%"));
    assert_eq!(e.trailer("life"), Some("-2"));
    assert!(e.time > 1_600_000_000);
    assert_eq!(log[1].label, "Start");
    assert_eq!(log[1].source, Source::User);
    assert_eq!(r.log(1).unwrap().len(), 1);
}

#[test]
fn a_label_starting_with_a_hash_is_kept() {
    let r = repo("hash");
    r.commit(V1, &Message::user("#1 build")).unwrap();
    assert_eq!(r.log(1).unwrap()[0].label, "#1 build");
}

#[test]
fn restore_is_a_new_commit() {
    let r = repo("restore");
    let v1 = r.commit(V1, &Message::user("One")).unwrap().unwrap();
    r.commit(V2, &Message::user("Two")).unwrap().unwrap();
    let v3 = r.restore(&v1).unwrap().unwrap();
    assert_eq!(r.read(&v3).unwrap(), V1);
    assert_eq!(std::fs::read_to_string(r.file()).unwrap(), V1);
    let log = r.log(10).unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].source, Source::Restore);
    assert_eq!(log[0].label, format!("Restore {}: One", v1.short()));
    assert_eq!(log[0].trailer("Restores"), Some(v1.as_str()));
    // Restoring the latest version again changes nothing
    assert_eq!(r.restore(&v3).unwrap(), None);
}

#[test]
fn reopening_keeps_the_history() {
    let dir = scratch("reopen").join("b");
    let v1 = BuildRepo::open(git(), &dir)
        .unwrap()
        .commit(V1, &Message::user("One"))
        .unwrap()
        .unwrap();
    let r = BuildRepo::open(git(), &dir).unwrap();
    assert_eq!(r.head().unwrap(), Some(v1));
}

#[test]
fn one_repository_per_build() {
    let root = scratch("builds");
    let h = History::with_git(&root, Some(git()));
    assert!(h.is_enabled());
    assert!(h.builds().unwrap().is_empty());
    let a = h.open("Righteous Fire").unwrap().unwrap();
    let b = h.open("Cyclone: <slam>?").unwrap().unwrap();
    a.commit(V1, &Message::user("a")).unwrap();
    b.commit(V2, &Message::user("b")).unwrap();
    assert_eq!(
        h.builds().unwrap(),
        vec!["Cyclone_ _slam__".to_string(), "Righteous Fire".to_string()]
    );
    assert_eq!(a.log(10).unwrap().len(), 1);
    assert_eq!(b.log(10).unwrap().len(), 1);
    assert_eq!(a.dir(), root.join("Righteous Fire"));
}

#[test]
fn without_git_nothing_is_recorded() {
    let root = scratch("nogit");
    let h = History::with_git(&root, None);
    assert!(!h.is_enabled());
    assert!(h.open("x").unwrap().is_none());
    let mut rec = Recorder::new(None);
    assert!(!rec.is_enabled());
    rec.user_edit(V1, "edit");
    assert!(!rec.is_pending());
    assert_eq!(rec.agent_step(V2, &Message::agent("a", "b")).unwrap(), None);
    assert_eq!(rec.close().unwrap(), None);
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    assert!(Git::at(root.join("no-such-git")).is_err());
}

#[test]
fn user_edits_coalesce_and_agent_steps_stay_apart() {
    let r = repo("coalesce");
    let mut rec = Recorder::new(Some(r.clone())).with_quiet(Duration::from_secs(3600));
    rec.user_edit(V1, "Allocate A");
    rec.user_edit(V2, "Allocate B");
    rec.user_edit(V2, "Allocate B");
    // Not quiet long enough yet
    assert_eq!(rec.tick().unwrap(), None);
    assert!(rec.is_pending());
    assert!(r.log(10).unwrap().is_empty());
    // An agent step commits the user's edits first, then its own
    rec.agent_step(V3, &Message::agent("claude", "Swap gem"))
        .unwrap()
        .unwrap();
    assert!(!rec.is_pending());
    let log = r.log(10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].label, "Swap gem");
    assert_eq!(log[1].label, "2 edits");
    assert_eq!(log[1].source, Source::User);
    assert_eq!(r.read(&log[1].version).unwrap(), V2);
}

#[test]
fn user_edits_commit_once_quiet() {
    let r = repo("quiet");
    let mut rec = Recorder::new(Some(r.clone())).with_quiet(Duration::ZERO);
    rec.user_edit(V1, "Level up");
    let v = rec.tick().unwrap().unwrap();
    assert_eq!(r.log(10).unwrap()[0].version, v);
    assert_eq!(r.log(10).unwrap()[0].label, "Level up");
    assert_eq!(rec.tick().unwrap(), None);
}

#[test]
fn closing_or_dropping_commits_what_waits() {
    let r = repo("close");
    let mut rec = Recorder::new(Some(r.clone())).with_quiet(Duration::from_secs(3600));
    rec.user_edit(V1, "a");
    rec.close().unwrap().unwrap();
    {
        let mut rec = Recorder::new(Some(r.clone())).with_quiet(Duration::from_secs(3600));
        rec.user_edit(V2, "b");
    }
    assert_eq!(r.log(10).unwrap().len(), 2);
}

#[test]
fn pushes_a_backup_without_forcing() {
    let root = scratch("push");
    let backup = root.join("backup.git");
    let out = std::process::Command::new("git")
        .args(["init", "--quiet", "--bare"])
        .arg(&backup)
        .status()
        .unwrap();
    assert!(out.success());
    let a = BuildRepo::open(git(), root.join("a")).unwrap();
    a.commit(V1, &Message::user("a")).unwrap();
    a.push(backup.to_str().unwrap()).unwrap();
    a.commit(V2, &Message::user("b")).unwrap();
    a.push(backup.to_str().unwrap()).unwrap();
    // Another build's unrelated history is refused, not forced over
    let b = BuildRepo::open(git(), root.join("b")).unwrap();
    b.commit(V3, &Message::user("c")).unwrap();
    assert!(b.push(backup.to_str().unwrap()).is_err());
    assert!(a.push("--force").is_err());
}

#[test]
fn directory_names_every_platform_takes() {
    assert_eq!(dir_name("Righteous Fire"), "Righteous Fire");
    assert_eq!(dir_name("a/b\\c:d"), "a_b_c_d");
    assert_eq!(dir_name("trailing. . "), "trailing");
    assert_eq!(dir_name(""), "build");
    assert_eq!(dir_name(".."), "build");
    assert_eq!(dir_name("CON"), "_CON");
    assert_eq!(dir_name("com1.xml"), "_com1.xml");
    assert_eq!(dir_name("Console"), "Console");
    assert_eq!(dir_name("Ünïcode ✓"), "Ünïcode ✓");
}
