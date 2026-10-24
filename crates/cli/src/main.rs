//! Native runner of the engine API.
//!
//!   pob serve                 JSON requests on stdin, one per line; responses on stdout
//!   pob bundle <out> [xml..]  writes the data files the engine reads (starting up
//!                             and loading the given builds) as `<out>/core.bin`, and
//!                             every tree version's data under `<out>/files/` for
//!                             builds on other trees, which the browser fetches on demand
//!   pob bench <xml>           times loading, editing and node power for a build

use session::{Recording, Session};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;
use upstream::imported::Imported;

fn src_dir() -> PathBuf {
    std::env::var_os("POB_UPSTREAM_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../upstream/PathOfBuilding/src")
        })
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("pob: {msg}");
    std::process::exit(1)
}

fn ok(s: &Session, req: serde_json::Value) -> serde_json::Value {
    let text = s.request(&req.to_string()).unwrap_or_else(|e| die(e));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| die(e));
    if let Some(e) = v.get("error") {
        die(format!("{}: {e}", req["cmd"]));
    }
    v["ok"].clone()
}

fn serve() {
    let s = Session::new(Rc::new(Imported::new(src_dir()))).unwrap_or_else(|e| die(e));
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap_or_else(|e| die(e));
        if line.trim().is_empty() {
            continue;
        }
        let r = s.request(&line).unwrap_or_else(|e| die(e));
        let mut out = stdout.lock();
        let _ = writeln!(out, "{r}");
        let _ = out.flush();
    }
}

fn copy_tree(from: &Path, to: &Path, rel: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .unwrap_or_else(|e| die(format!("{}: {e}", from.display())))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let rel = format!("{rel}/{name}");
        if p.is_dir() {
            copy_tree(&p, to, &rel, out);
        } else if name.ends_with(".lua") || name.ends_with(".json") {
            let dest = to.join(&rel);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::copy(&p, &dest).unwrap_or_else(|e| die(format!("{}: {e}", p.display())));
            out.push(rel);
        }
    }
}

fn bundle(out: &Path, builds: &[String]) {
    let src = src_dir();
    let rec = Rc::new(Recording {
        inner: Imported::new(&src),
        read: Default::default(),
    });
    let s = Session::new(rec.clone()).unwrap_or_else(|e| die(e));
    for path in builds {
        let xml = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
        ok(&s, serde_json::json!({"cmd": "loadXml", "xml": xml}));
    }
    let read = rec.read.borrow().clone();
    let files: Vec<(String, Vec<u8>)> = read
        .iter()
        .map(|p| {
            let data = std::fs::read(src.join(p)).unwrap_or_else(|e| die(format!("{p}: {e}")));
            (p.clone(), data)
        })
        .collect();
    let packed = session::bundle::pack(files.iter().map(|(p, d)| (p.as_str(), d.as_slice())));
    std::fs::create_dir_all(out).unwrap_or_else(|e| die(e));
    std::fs::write(out.join("core.bin"), &packed).unwrap_or_else(|e| die(e));
    let raw: usize = files.iter().map(|(_, d)| d.len()).sum();
    eprintln!(
        "core.bin: {} files, {} MB, {} MB compressed",
        files.len(),
        raw / 1_000_000,
        packed.len() / 1_000_000
    );
    // Every tree version, for builds on older trees
    let mut lazy = Vec::new();
    for entry in std::fs::read_dir(src.join("TreeData")).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            copy_tree(
                &p,
                &out.join("files"),
                &format!("TreeData/{name}"),
                &mut lazy,
            );
        }
    }
    lazy.sort();
    std::fs::write(
        out.join("files.json"),
        serde_json::to_string(&lazy).unwrap(),
    )
    .unwrap();
    eprintln!("files/: {} files", lazy.len());
}

/// Resident memory (from /proc on Linux) and the cycle collector's work
fn rss() -> String {
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .map(|l| l[6..].trim().to_string())
        })
        .unwrap_or_default();
    let (runs, cleared) = lua::gc::totals();
    format!("{rss}, {runs} collections freed {cleared} objects")
}

fn bench(path: &str) {
    let t = Instant::now();
    let s = Session::new(Rc::new(Imported::new(src_dir()))).unwrap_or_else(|e| die(e));
    eprintln!(
        "start:      {:>8.0} ms  {}",
        t.elapsed().as_secs_f64() * 1000.0,
        rss()
    );
    let xml = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let t = Instant::now();
    ok(&s, serde_json::json!({"cmd": "loadXml", "xml": xml}));
    eprintln!(
        "load:       {:>8.0} ms  {}",
        t.elapsed().as_secs_f64() * 1000.0,
        rss()
    );
    let alloc = ok(&s, serde_json::json!({"cmd": "treeState"}))["alloc"].clone();
    if let Some(id) = alloc.as_array().and_then(|a| a.last()).cloned() {
        let t = Instant::now();
        ok(&s, serde_json::json!({"cmd": "clickNode", "id": id}));
        ok(&s, serde_json::json!({"cmd": "clickNode", "id": id}));
        eprintln!(
            "2 edits:    {:>8.0} ms  {}",
            t.elapsed().as_secs_f64() * 1000.0,
            rss()
        );
    }
    let t = Instant::now();
    ok(&s, serde_json::json!({"cmd": "tree"}));
    eprintln!(
        "tree:       {:>8.0} ms  {}",
        t.elapsed().as_secs_f64() * 1000.0,
        rss()
    );
    if let Some(id) = alloc.as_array().and_then(|a| a.last()).cloned() {
        let t = Instant::now();
        for _ in 0..10 {
            ok(&s, serde_json::json!({"cmd": "clickNode", "id": id}));
            ok(&s, serde_json::json!({"cmd": "clickNode", "id": id}));
        }
        eprintln!(
            "20 edits:   {:>8.0} ms  {}",
            t.elapsed().as_secs_f64() * 1000.0,
            rss()
        );
    }
    let t = Instant::now();
    ok(&s, serde_json::json!({"cmd": "nodePower", "index": 1}));
    eprintln!(
        "node power: {:>8.0} ms  {}",
        t.elapsed().as_secs_f64() * 1000.0,
        rss()
    );
    if std::env::var_os("BENCH_TWICE").is_some() {
        let t = Instant::now();
        ok(&s, serde_json::json!({"cmd": "nodePower", "index": 1}));
        eprintln!(
            "node power: {:>8.0} ms  {}",
            t.elapsed().as_secs_f64() * 1000.0,
            rss()
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => serve(),
        Some("bundle") if args.len() >= 2 => bundle(Path::new(&args[1]), &args[2..]),
        Some("bench") if args.len() == 2 => bench(&args[1]),
        _ => die("usage: pob serve | pob bundle <out dir> [build.xml ...] | pob bench <build.xml>"),
    }
}
