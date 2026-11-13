//! Native runner of the engine API.
//!
//!   pob serve [--typed | --typed-only]
//!                             JSON requests on stdin, one per line; responses on stdout;
//!                             with --typed, the commands the typed engine has are its;
//!                             with --typed-only, the others are "not ported" errors
//!   pob bundle <out> [--lua [xml..]]
//!                             writes the typed engine's data in its binary form (game
//!                             data, mod parser, the latest tree) as `<out>/core.bin`, and
//!                             every tree version under `<out>/files/`, which the browser
//!                             fetches when a build needs it. With --lua, also the Lua data
//!                             files the translation reads (starting up and loading the
//!                             given builds) and every tree version's Lua data, for an
//!                             engine built with the translation
//!   pob bench <xml>           times loading, editing and node power for a build

#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use session::typed::TypedData;
use session::{Dispatcher, Prefer, Recording, Session};
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

fn serve(mode: Option<&str>) {
    let typed = matches!(mode, Some("--typed" | "--typed-only"));
    let data = typed.then(|| {
        &*Box::leak(Box::new(
            TypedData::from_source(&src_dir()).unwrap_or_else(|e| die(e)),
        ))
    });
    let mut s = match (mode, data) {
        (Some("--typed-only"), Some(data)) => Dispatcher::typed(data),
        _ => {
            let s = Session::new(Rc::new(Imported::new(src_dir()))).unwrap_or_else(|e| die(e));
            let prefer = if typed {
                Prefer::Typed
            } else {
                Prefer::Translated
            };
            Dispatcher::new(s, data, prefer)
        }
    };
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

/// Data files by path, with their bytes
type Files = Vec<(String, Vec<u8>)>;

/// The typed engine's data, by bundle path: what starts the engine, and
/// the tree versions
fn typed_files() -> (Files, Files) {
    let t = Instant::now();
    let src = gamedata::Source::load(&src_dir()).unwrap_or_else(|e| die(e));
    let game = src.game_data().unwrap_or_else(|e| die(e));
    let parser_data = src.mod_parser_data().unwrap_or_else(|e| die(e));
    let parser_bytes = parser_data.encode();
    let parser = engine_parser(&parser_bytes);
    let versions = game.tree_versions.clone();
    let latest = versions.last().cloned().unwrap_or_default();
    let mut core = vec![
        (session::bundle::GAME_DATA.to_string(), game.encode()),
        (session::bundle::MOD_PARSER.to_string(), parser_bytes),
    ];
    let mut trees = Vec::new();
    for v in &versions {
        let tree = src
            .tree(v, &parser, *v == latest)
            .unwrap_or_else(|e| die(format!("tree {v}: {e}")));
        let file = (
            session::bundle::tree_path(v),
            session::bundle::encode_tree(&tree),
        );
        if *v == latest {
            core.push(file.clone());
        }
        trees.push(file);
    }
    eprintln!(
        "typed data made in {:.1} s: game data {}, mod parser {}, {} trees {}",
        t.elapsed().as_secs_f64(),
        mb(core[0].1.len()),
        mb(core[1].1.len()),
        trees.len(),
        mb(trees.iter().map(|(_, d)| d.len()).sum())
    );
    (core, trees)
}

fn engine_parser(bytes: &[u8]) -> engine::mod_parser::ModParser {
    engine::mod_parser::ModParser::decode(bytes).unwrap_or_else(|e| die(e))
}

/// The Lua data files the translation reads starting up and loading the
/// given builds, and every tree version's, by path relative to `src/`
fn lua_files(builds: &[String]) -> (Files, Files) {
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
    let read = |p: &String| {
        let data = std::fs::read(src.join(p)).unwrap_or_else(|e| die(format!("{p}: {e}")));
        (p.clone(), data)
    };
    let core = rec.read.borrow().iter().map(read).collect();
    let mut lazy = Vec::new();
    for entry in std::fs::read_dir(src.join("TreeData")).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            list_tree(&p, &format!("TreeData/{name}"), &mut lazy);
        }
    }
    lazy.sort();
    (core, lazy.iter().map(read).collect())
}

fn list_tree(from: &Path, rel: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .unwrap_or_else(|e| die(format!("{}: {e}", from.display())))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let rel = format!("{rel}/{name}");
        if p.is_dir() {
            list_tree(&p, &rel, out);
        } else if name.ends_with(".lua") || name.ends_with(".json") {
            out.push(rel);
        }
    }
}

fn bundle(out: &Path, args: &[String]) {
    let (mut core, mut lazy) = typed_files();
    if args.first().is_some_and(|a| a == "--lua") {
        let (c, l) = lua_files(&args[1..]);
        core.extend(c);
        lazy.extend(l);
    } else if !args.is_empty() {
        die("builds to bundle the Lua data files for go after --lua");
    }
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
        Some("serve") if args.len() <= 2 => serve(args.get(1).map(String::as_str)),
        Some("bundle") if args.len() >= 2 => bundle(Path::new(&args[1]), &args[2..]),
        Some("bench") if args.len() == 2 => bench(&args[1]),
        _ => die(
            "usage: pob serve [--typed | --typed-only] | pob bundle <out dir> [--lua [build.xml ...]] | pob bench <build.xml>",
        ),
    }
}
