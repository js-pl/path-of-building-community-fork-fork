//! The settings file, in throwaway directories

use settings::{Error, KEYS, Settings};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "settings-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_missing_file_reads_as_the_defaults() {
    let s = Settings::load(&scratch("missing").join("settings.json")).unwrap();
    assert_eq!(s, Settings::default());
    assert!(s.history.enabled);
    assert_eq!(s.history.git_options(), history::GitOptions::default());
}

#[test]
fn saves_and_loads_back_with_its_directory() {
    let path = scratch("roundtrip").join("deep/settings.json");
    let mut s = Settings::default();
    s.set("history.enabled", "off").unwrap();
    s.set("history.git", " C:\\Program Files\\Git\\cmd\\git.exe ")
        .unwrap();
    s.set("history.gitConfig", "/home/me/pob.gitconfig")
        .unwrap();
    s.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("\"gitConfig\": \"/home/me/pob.gitconfig\""),
        "{text}"
    );
    let back = Settings::load(&path).unwrap();
    assert_eq!(back, s);
    assert_eq!(back.get("history.enabled").unwrap(), "false");
    assert_eq!(
        back.get("history.git").unwrap(),
        "C:\\Program Files\\Git\\cmd\\git.exe"
    );
    let o = back.history.git_options();
    assert!(!o.enabled);
    assert_eq!(
        o.program,
        Some(PathBuf::from("C:\\Program Files\\Git\\cmd\\git.exe"))
    );
    assert_eq!(o.config, Some(PathBuf::from("/home/me/pob.gitconfig")));
}

#[test]
fn keys_it_does_not_know_are_kept() {
    let path = scratch("unknown").join("settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"history": {"git": "git", "backup": "/mnt/b"}, "options": {"thousands": true}}"#,
    )
    .unwrap();
    let mut s = Settings::load(&path).unwrap();
    assert!(s.history.enabled, "a key left out keeps its default");
    s.set("history.enabled", "false").unwrap();
    s.save(&path).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["history"]["backup"], "/mnt/b");
    assert_eq!(v["options"]["thousands"], true);
    assert_eq!(v["history"]["enabled"], false);
}

#[test]
fn a_broken_file_is_an_error_and_left_alone() {
    let path = scratch("broken").join("settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"history": {"enabled": "maybe"}}"#).unwrap();
    assert!(matches!(Settings::load(&path), Err(Error::Parse { .. })));
    std::fs::write(&path, "{").unwrap();
    assert!(matches!(Settings::load(&path), Err(Error::Parse { .. })));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{");
}

#[test]
fn every_key_reads_and_writes_and_others_are_refused() {
    let mut s = Settings::default();
    for (key, _) in KEYS {
        let v = s.get(key).unwrap();
        s.set(key, &v).unwrap();
    }
    assert_eq!(s, Settings::default());
    assert!(matches!(
        s.set("history.gti", "x"),
        Err(Error::UnknownKey(_))
    ));
    assert!(matches!(s.get("nope"), Err(Error::UnknownKey(_))));
    assert!(matches!(
        s.set("history.enabled", "maybe"),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn the_git_the_settings_give() {
    let mut s = Settings::default();
    s.set("history.enabled", "false").unwrap();
    assert!(s.history.git().unwrap().is_none());
    s.set("history.enabled", "true").unwrap();
    s.set("history.git", "git").unwrap();
    assert!(s.history.git().unwrap().is_some());
    // Set but not runnable, or a gitconfig that isn't there: told, not ignored
    s.set(
        "history.git",
        scratch("nogit").join("git").to_str().unwrap(),
    )
    .unwrap();
    assert!(s.history.git().is_err());
    s.set("history.git", "git").unwrap();
    s.set(
        "history.gitConfig",
        scratch("nocfg").join("x").to_str().unwrap(),
    )
    .unwrap();
    assert!(s.history.git().is_err());
}
