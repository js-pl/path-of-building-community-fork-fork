//! Native runner of the engine API.
//!
//!   pob serve                 JSON requests on stdin, one per line; responses on stdout:
//!                             the typed engine, as the browser ships it, where commands
//!                             it does not have are "not ported" errors
//!   pob bundle <out>          writes the typed engine's data in its binary form (game
//!                             data, mod parser, the latest tree) as `<out>/core.bin`, and
//!                             every tree version under `<out>/files/`, which the browser
//!                             fetches when a build needs it
//!   pob bench <xml>           times making the data, loading, editing and node power for
//!                             a build on the typed engine

#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use session::Dispatcher;
use session::typed::TypedData;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

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

fn ok(s: &mut Dispatcher, req: serde_json::Value) -> serde_json::Value {
    let text = s.request(&req.to_string()).unwrap_or_else(|e| die(e));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| die(e));
    if let Some(e) = v.get("error") {
        die(format!("{}: {e}", req["cmd"]));
    }
    v["ok"].clone()
}

/// The typed engine's data made from upstream's source
fn data() -> &'static TypedData {
    Box::leak(Box::new(
        TypedData::from_source(&src_dir()).unwrap_or_else(|e| die(e)),
    ))
}

fn serve() {
    let mut s = Dispatcher::typed(data());
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

fn mb(n: usize) -> String {
    format!("{:.1} MB", n as f64 / 1e6)
}

use session::bundle::FileList as Files;

/// The typed engine's data, by bundle path: what starts the engine, and
/// what builds need on demand (the tree versions, the timeless jewels')
fn typed_files() -> (Files, Files) {
    let t = Instant::now();
    let (core, lazy) = session::bundle::typed_files(&src_dir()).unwrap_or_else(|e| die(e));
    let size = |prefix: &str| {
        mb(lazy
            .iter()
            .filter(|(p, _)| p.starts_with(prefix))
            .map(|(_, d)| d.len())
            .sum())
    };
    eprintln!(
        "typed data made in {:.1} s: game data {}, mod parser {}; trees {}, timeless jewels {}",
        t.elapsed().as_secs_f64(),
        mb(core[0].1.len()),
        mb(core[1].1.len()),
        size("typed/trees/"),
        size("typed/timeless"),
    );
    (core, lazy)
}

fn bundle(out: &Path) {
    let (core, lazy) = typed_files();
    let packed = session::bundle::pack(core.iter().map(|(p, d)| (p.as_str(), d.as_slice())));
    std::fs::create_dir_all(out).unwrap_or_else(|e| die(e));
    std::fs::write(out.join("core.bin"), &packed).unwrap_or_else(|e| die(e));
    eprintln!(
        "core.bin: {} files, {}, {} compressed",
        core.len(),
        mb(core.iter().map(|(_, d)| d.len()).sum()),
        mb(packed.len())
    );
    let files = out.join("files");
    if files.exists() {
        std::fs::remove_dir_all(&files).unwrap_or_else(|e| die(e));
    }
    for (rel, data) in &lazy {
        let dest = files.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, data).unwrap_or_else(|e| die(format!("{rel}: {e}")));
    }
    let names: Vec<&str> = lazy.iter().map(|(p, _)| p.as_str()).collect();
    std::fs::write(
        out.join("files.json"),
        serde_json::to_string(&names).unwrap(),
    )
    .unwrap();
    eprintln!(
        "files/: {} files, {}",
        lazy.len(),
        mb(lazy.iter().map(|(_, d)| d.len()).sum())
    );
}

/// Resident memory (from /proc on Linux)
fn rss() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .map(|l| l[6..].trim().to_string())
        })
        .unwrap_or_default()
}

fn bench(path: &str) {
    let t = Instant::now();
    let mut s = Dispatcher::typed(data());
    eprintln!(
        "data:       {:>8.0} ms  {}",
        t.elapsed().as_secs_f64() * 1000.0,
        rss()
    );
    let xml = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let t = Instant::now();
    ok(&mut s, serde_json::json!({"cmd": "loadXml", "xml": xml}));
    eprintln!(
        "load:       {:>8.0} ms  {}",
        t.elapsed().as_secs_f64() * 1000.0,
        rss()
    );
    let alloc = ok(&mut s, serde_json::json!({"cmd": "treeState"}))["alloc"].clone();
    if let Some(id) = alloc.as_array().and_then(|a| a.last()).cloned() {
        let t = Instant::now();
        ok(&mut s, serde_json::json!({"cmd": "clickNode", "id": id}));
        ok(&mut s, serde_json::json!({"cmd": "clickNode", "id": id}));
        eprintln!(
            "2 edits:    {:>8.0} ms  {}",
            t.elapsed().as_secs_f64() * 1000.0,
            rss()
        );
    }
    let t = Instant::now();
    ok(&mut s, serde_json::json!({"cmd": "tree"}));
    eprintln!(
        "tree:       {:>8.0} ms  {}",
        t.elapsed().as_secs_f64() * 1000.0,
        rss()
    );
    if let Some(id) = alloc.as_array().and_then(|a| a.last()).cloned() {
        let t = Instant::now();
        for _ in 0..10 {
            ok(&mut s, serde_json::json!({"cmd": "clickNode", "id": id}));
            ok(&mut s, serde_json::json!({"cmd": "clickNode", "id": id}));
        }
        eprintln!(
            "20 edits:   {:>8.0} ms  {}",
            t.elapsed().as_secs_f64() * 1000.0,
            rss()
        );
    }
    let t = Instant::now();
    ok(&mut s, serde_json::json!({"cmd": "nodePower", "index": 1}));
    eprintln!(
        "node power: {:>8.0} ms  {}",
        t.elapsed().as_secs_f64() * 1000.0,
        rss()
    );
    if std::env::var_os("BENCH_TWICE").is_some() {
        let t = Instant::now();
        ok(&mut s, serde_json::json!({"cmd": "nodePower", "index": 1}));
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
        Some("serve") if args.len() == 1 => serve(),
        Some("bundle") if args.len() == 2 => bundle(Path::new(&args[1])),
        Some("bench") if args.len() == 2 => bench(&args[1]),
        _ => usage(),
    }
}

fn usage() -> ! {
    die("usage: pob serve | pob bundle <out dir> | pob bench <build.xml>")
}
