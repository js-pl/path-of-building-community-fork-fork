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
//!   pob history <dir> <verb>  a build's history, a git repository of its own in <dir>,
//!                             written with the git the settings name (`POB_GIT` overrides):
//!                               commit <xml> [label]   commits the build in <xml>
//!                               log [n]                the last n versions (20)
//!                               show <version>         the build's XML at a version
//!                               restore <version>      a version again, as a new commit
//!                               push <remote>          a backup, never forced
//!   pob settings [key [value]]
//!                             the settings file (`POB_SETTINGS` picks another): all of
//!                             it and the git it gives, one setting, or sets one
//!   pob host [--headless] [--builds <dir>] [--socket <path>] [--listen <addr>|none]
//!           [--tls-cert <pem> --tls-key <pem>] [--tokens <file>]
//!           [--web <addr>] [--app <dir>] [--build <name>] [--no-open]
//!                             owns the builds in <dir> (`POB_BUILDS`) with their history
//!                             and answers the agent API on the control socket
//!                             (`POB_SOCKET`) and as MCP over HTTP with tokens (on
//!                             127.0.0.1:8731 by default); unless --headless, serves the
//!                             web app (the bundle in <dir>, `POB_APP`, web/dist by
//!                             default) on 127.0.0.1:8730 and opens the browser, showing
//!                             <name>; see docs/agent.md
//!   pob ctl [--socket <path>] [--build <name>]... [--agent <dir>] [--json]
//!           <verb> [key=value]... | <verb> '<json fields>'
//!                             calls one agent API verb on a running host, with a grant
//!                             of the builds named; prints the answer as text (JSON with
//!                             --json); without a verb, lists them
//!   pob mcp [--socket <path>] [--builds <dir>] [--build <name>]... [--agent <dir>]
//!           [--no-start]
//!                             the agent API as an MCP server on stdio, for a harness on
//!                             this machine: relays to the host's control socket with a
//!                             grant of the builds named, and starts a host (`--builds`)
//!                             when none runs, unless --no-start
//!   pob agents <verb>         tokens for MCP over HTTP and the host's connections:
//!                               token [--build <name>]... [--name <n>] [--agent <dir>]
//!                                                      mints a token, shown once
//!                               tokens                 lists them (`POB_TOKENS`, --tokens)
//!                               revoke <id>            revokes one
//!                               connections            the running host's connections
//!                               disconnect <id>        closes one
//!                               install [--build <name>]...
//!                                                      adds `pob mcp` to the harnesses
//!                                                      found (Claude Code, Codex)
//!   pob agent claude|codex [--build <name>]... [--agent <dir>] [--allow <host>]...
//!           [--rebuild] [--reset-login] [--no-container] [--exec] [-- <args>...]
//!                             the harness in the default Podman container, attached to
//!                             this terminal: starts a host if none runs, mints a token
//!                             for the builds named (revoked when it ends), builds the
//!                             images, starts the proxy; --no-container runs the harness
//!                             here with `pob mcp`, unsandboxed; --exec runs the command
//!                             after `--` in the container instead of the harness

#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod container;
mod install;

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
        Some("history") if args.len() >= 3 => history(Path::new(&args[1]), &args[2..]),
        Some("settings") if args.len() <= 3 => settings_cmd(&args[1..]),
        Some("host") => host_cmd(&args[1..]),
        Some("ctl") => ctl(&args[1..]),
        Some("mcp") => mcp(&args[1..]),
        Some("agents") => agents(&args[1..]),
        Some("agent") => agent_cmd(&args[1..]),
        _ => usage(),
    }
}

fn usage() -> ! {
    die(
        "usage: pob serve | pob bundle <out dir> | pob bench <build.xml>\n       \
         pob history <dir> commit <build.xml> [label] | log [n] | show <version> | \
         restore <version> | push <remote>\n       \
         pob settings [key [value]]\n       \
         pob host [--headless] [--builds <dir>] [--socket <path>] [--listen <addr>|none] \
         [--tls-cert <pem> --tls-key <pem>] [--tokens <file>] [--web <addr>] [--app <dir>] \
         [--build <name>] [--no-open]\n       \
         pob ctl [--socket <path>] [--build <name>]... [--agent <dir>] [--json] <verb> [key=value]...\n       \
         pob mcp [--socket <path>] [--builds <dir>] [--build <name>]... [--agent <dir>] [--no-start]\n       \
         pob agents token [--build <name>]... [--name <n>] [--agent <dir>] [--url <url>] | tokens | \
         revoke <id> | connections | disconnect <id> | install [--build <name>]...   [--tokens <file>] [--socket <path>] [--json]\n       \
         pob agent claude|codex [--build <name>]... [--agent <dir>] [--allow <host>]... [--rebuild] \
         [--reset-login] [--no-container] [--exec] [-- <harness args or, with --exec, a command>]",
    )
}

fn history(dir: &Path, args: &[String]) {
    use history::{BuildRepo, Message};
    let (_, settings) = load_settings();
    let git = match settings.history.git() {
        Ok(Some(git)) => git,
        Ok(None) => die("the history is off: pob settings history.enabled true"),
        Err(e) => die(format!("{e} (pob settings history.git <path>)")),
    };
    let repo = BuildRepo::open(git, dir).unwrap_or_else(|e| die(e));
    let version = |rev: &str| repo.resolve(rev).unwrap_or_else(|e| die(e));
    let done = |v: Option<history::Version>| match v {
        Some(v) => println!("{}", v.short()),
        None => println!("unchanged"),
    };
    match (args[0].as_str(), &args[1..]) {
        ("commit", [xml, label @ ..]) if label.len() <= 1 => {
            let text = std::fs::read_to_string(xml).unwrap_or_else(|e| die(format!("{xml}: {e}")));
            let label = label.first().map_or("Edit", String::as_str);
            done(
                repo.commit(&text, &Message::user(label))
                    .unwrap_or_else(|e| die(e)),
            );
        }
        ("log", n) if n.len() <= 1 => {
            let n = n
                .first()
                .map_or(20, |n| n.parse().unwrap_or_else(|_| usage()));
            for e in repo.log(n).unwrap_or_else(|e| die(e)) {
                let trailers: String = e
                    .trailers
                    .iter()
                    .map(|(k, v)| format!("  {k}: {v}"))
                    .collect();
                println!(
                    "{}  {:<8}  {}{trailers}",
                    e.version.short(),
                    e.source.to_string(),
                    e.label
                );
            }
        }
        ("show", [rev]) => print!("{}", repo.read(&version(rev)).unwrap_or_else(|e| die(e))),
        ("restore", [rev]) => done(repo.restore(&version(rev)).unwrap_or_else(|e| die(e))),
        ("push", [remote]) => repo.push(remote).unwrap_or_else(|e| die(e)),
        _ => usage(),
    }
}

fn load_settings() -> (PathBuf, settings::Settings) {
    let path = settings::default_path().unwrap_or_else(|| {
        die(format!(
            "no config directory; set {} to the settings file",
            settings::SETTINGS_ENV
        ))
    });
    let s = settings::Settings::load(&path).unwrap_or_else(|e| die(e));
    (path, s)
}

fn settings_cmd(args: &[String]) {
    let (path, mut s) = load_settings();
    match args {
        [] => {
            println!("{}", path.display());
            for (key, about) in settings::KEYS {
                let value = s.get(key).unwrap_or_else(|e| die(e));
                println!("  {key} = {value:?}\n      {about}");
            }
            match s.history.git() {
                Ok(Some(git)) => {
                    let config = git
                        .config()
                        .map(|c| format!(", gitconfig {}", c.display()))
                        .unwrap_or_default();
                    println!(
                        "history: on, {} ({}{config})",
                        git.version(),
                        git.program().display()
                    )
                }
                Ok(None) => println!("history: off"),
                Err(e) => println!("history: not kept: {e}"),
            }
        }
        [key] => println!("{}", s.get(key).unwrap_or_else(|e| die(e))),
        [key, value] => {
            s.set(key, value).unwrap_or_else(|e| die(e));
            s.save(&path).unwrap_or_else(|e| die(e));
        }
        _ => usage(),
    }
}

/// `--flag value` pairs and the rest of the arguments
fn flags<'a>(
    args: &'a [String],
    with_value: &[&str],
    bare: &[&str],
) -> (Vec<(&'a str, &'a str)>, Vec<&'a str>) {
    let mut out = Vec::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if with_value.contains(&a) {
            let v = args.get(i + 1).unwrap_or_else(|| usage());
            out.push((a, v.as_str()));
            i += 2;
        } else if bare.contains(&a) {
            out.push((a, ""));
            i += 1;
        } else {
            rest.extend(args[i..].iter().map(String::as_str));
            break;
        }
    }
    (out, rest)
}

fn flag<'a>(flags: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    flags.iter().find(|(f, _)| *f == name).map(|(_, v)| *v)
}

fn socket_path(flags: &[(&str, &str)]) -> PathBuf {
    flag(flags, "--socket")
        .map(PathBuf::from)
        .or_else(host::default_socket)
        .unwrap_or_else(|| die(format!("no data directory; set {}", host::SOCKET_ENV)))
}

fn host_cmd(args: &[String]) {
    let (flags, rest) = flags(
        args,
        &[
            "--builds",
            "--socket",
            "--listen",
            "--tls-cert",
            "--tls-key",
            "--tokens",
            "--web",
            "--app",
            "--build",
        ],
        &["--headless", "--no-open"],
    );
    if !rest.is_empty() {
        usage();
    }
    let builds = flag(&flags, "--builds")
        .map(PathBuf::from)
        .or_else(host::default_builds)
        .unwrap_or_else(|| die(format!("no data directory; set {}", host::BUILDS_ENV)));
    let socket = socket_path(&flags);
    let http = http_endpoint(&flags);
    let (settings_path, settings) = load_settings();
    let git = match settings.history.git() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("pob host: the history is not kept: {e}");
            None
        }
    };
    if git.is_none() {
        eprintln!("pob host: no history: builds are saved, their versions aren't");
    }
    std::fs::create_dir_all(&builds).unwrap_or_else(|e| die(format!("{}: {e}", builds.display())));
    let web = if flag(&flags, "--headless").is_some() {
        None
    } else {
        Some(web_app(&flags))
    };
    let tokens = tokens_path(&flags);
    serve_host(builds, socket, git, http, web, tokens, settings_path);
}

/// The web app as the flags ask for it: where its bundle is, the address
/// (loopback only), the browser
struct WebApp {
    listener: std::net::TcpListener,
    port: u16,
    root: PathBuf,
    open: bool,
    build: Option<String>,
}

fn web_app(flags: &[(&str, &str)]) -> WebApp {
    let root = flag(flags, "--app")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("POB_APP").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/dist"));
    if !root.join("index.html").is_file() {
        die(format!(
            "pob host: no web app in {} (build it with `npm run build` in web/, pass --app <dir>, \
             or run `pob host --headless`)",
            root.display()
        ));
    }
    let addr = flag(flags, "--web").unwrap_or(host::web::DEFAULT_LISTEN);
    let (listener, port) =
        host::web::listen(addr).unwrap_or_else(|e| die(format!("pob host: {e}")));
    WebApp {
        listener,
        port,
        root,
        open: flag(flags, "--no-open").is_none(),
        build: flag(flags, "--build").map(String::from),
    }
}

/// MCP over HTTP as the flags ask for it: the listener, TLS, the tokens
struct HttpEndpoint {
    listener: std::net::TcpListener,
    tls: Option<std::sync::Arc<host::http::TlsConfig>>,
    tokens: host::tokens::Tokens,
}

fn tokens_path(flags: &[(&str, &str)]) -> PathBuf {
    flag(flags, "--tokens")
        .map(PathBuf::from)
        .or_else(host::tokens::default_path)
        .unwrap_or_else(|| {
            die(format!(
                "no data directory; set {}",
                host::tokens::TOKENS_ENV
            ))
        })
}

/// Binds MCP over HTTP: `--listen` (127.0.0.1 by default; `none` for
/// none), HTTPS with `--tls-cert` and `--tls-key`, which an address other
/// than loopback or a container network needs
fn http_endpoint(flags: &[(&str, &str)]) -> Option<HttpEndpoint> {
    use host::http::{self, Exposure};
    let asked = flag(flags, "--listen");
    let addr = asked.unwrap_or(http::DEFAULT_LISTEN);
    if addr == "none" {
        return None;
    }
    let (listener, at) = match http::listen(addr) {
        Ok(l) => l,
        Err(e) if asked.is_some() => die(format!("pob host: MCP over HTTP: {e}")),
        Err(e) => {
            eprintln!("pob host: no MCP over HTTP: {e} (pick another address with --listen)");
            return None;
        }
    };
    let tls = match (flag(flags, "--tls-cert"), flag(flags, "--tls-key")) {
        (Some(c), Some(k)) => {
            Some(http::tls_config(Path::new(c), Path::new(k)).unwrap_or_else(|e| die(e)))
        }
        (None, None) => None,
        _ => die("pob host: --tls-cert and --tls-key go together"),
    };
    let exposure = http::exposure(at.ip());
    if exposure != Exposure::Loopback {
        eprintln!(
            "pob host: warning: MCP on {at} can be reached from other machines or containers; \
             the token is all that guards it"
        );
    }
    if exposure == Exposure::Open && tls.is_none() {
        die(format!(
            "pob host: {at} is neither loopback nor a container network: serve HTTPS with \
             --tls-cert <pem> and --tls-key <pem>"
        ));
    }
    let scheme = if tls.is_some() { "https" } else { "http" };
    eprintln!(
        "pob host: MCP on {scheme}://{at}{}, with tokens from `pob agents token`",
        http::PATH
    );
    Some(HttpEndpoint {
        listener,
        tls,
        tokens: host::tokens::Tokens::new(tokens_path(flags)),
    })
}

fn serve_host(
    builds: PathBuf,
    socket: PathBuf,
    git: Option<history::Git>,
    http: Option<HttpEndpoint>,
    web: Option<WebApp>,
    tokens: PathBuf,
    settings: PathBuf,
) {
    let listener = host::control::local::listen(&socket).unwrap_or_else(|e| die(e));
    let (tx, rx) = std::sync::mpsc::channel();
    let (msgs, inbox) = std::sync::mpsc::channel();
    let registry = std::sync::Arc::new(host::connections::Registry::default());
    if let Some(h) = http {
        let endpoint = host::http::Http::new(h.tokens, tx.clone(), registry.clone());
        std::thread::spawn(move || endpoint.serve(h.listener, h.tls));
    }
    let listed = registry.clone();
    std::thread::spawn(move || host::control::local::accept(listener, tx, registry));
    let to = msgs.clone();
    std::thread::spawn(move || host::ui::forward(rx, to));
    let store = host::FsStore::new(&builds, git);
    let data = data();
    eprintln!(
        "pob host: builds in {}, control socket {}",
        builds.display(),
        socket.display()
    );
    let mut h = host::ui::Host::new(data, store);
    h.registry = Some(listed);
    h.tokens = Some(host::tokens::Tokens::new(tokens));
    h.settings = Some(settings);
    if let Some(w) = web {
        h.default_build = w.build.clone();
        let key = host::web::new_key().unwrap_or_else(|e| die(e));
        let app = host::web::Web::new(w.root, key, w.port, msgs);
        let url = app.url();
        std::thread::spawn(move || app.serve(w.listener));
        eprintln!("pob host: the web app on {url}");
        if w.open
            && let Err(e) = host::web::open_browser(&url)
        {
            eprintln!("pob host: no browser opened ({e}): open the link above");
        }
    } else {
        drop(msgs);
    }
    h.run(inbox);
}

/// `pob agents`: tokens and connections
fn agents(args: &[String]) {
    // The verb and its argument first, then the flags
    let positional = args.iter().take_while(|a| !a.starts_with("--")).count();
    let (rest, args) = args.split_at(positional.min(2));
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let (flags, extra) = flags(
        args,
        &[
            "--tokens", "--socket", "--build", "--agent", "--name", "--url",
        ],
        &["--json"],
    );
    if !extra.is_empty() {
        usage();
    }
    let json_out = flag(&flags, "--json").is_some();
    let print = |v: serde_json::Value| {
        if json_out {
            println!("{v}");
        } else {
            print!("{}", text(&v, 0));
        }
    };
    match rest.as_slice() {
        ["token"] => {
            let tokens = host::tokens::Tokens::new(tokens_path(&flags));
            let name = flag(&flags, "--name").unwrap_or("http");
            let now = host::control::now();
            let dir = flag(&flags, "--agent")
                .map(String::from)
                .unwrap_or_else(|| agent::agent_dir_name(name, now));
            let mut grant = agent::Grant::new(
                name.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .take(32)
                    .collect::<String>(),
                dir,
            );
            if grant.connection.is_empty() {
                die("--name is letters, digits, - and _");
            }
            for (f, b) in &flags {
                if *f == "--build" {
                    if *b == "agents" {
                        die("agents is not a build");
                    }
                    grant = grant.with_build(*b);
                }
            }
            let (token, entry) = tokens.mint(&grant, now).unwrap_or_else(|e| die(e));
            let url = flag(&flags, "--url").map(String::from).unwrap_or_else(|| {
                format!("http://{}{}", host::http::DEFAULT_LISTEN, host::http::PATH)
            });
            if json_out {
                let mut v = entry.to_json();
                v["token"] = serde_json::json!(token);
                v["url"] = serde_json::json!(url);
                println!("{v}");
                return;
            }
            println!("{token}");
            eprintln!(
                "token {} for {} (agent directory {}), shown once; revoke it with `pob agents revoke {}`",
                entry.id,
                if entry.builds.is_empty() {
                    "no build".to_string()
                } else {
                    entry.builds.join(", ")
                },
                entry.agent,
                entry.id
            );
            eprintln!(
                "  claude mcp add --transport http pob {url} --header \"Authorization: Bearer {token}\""
            );
        }
        ["install"] => install_cmd(&flags),
        ["tokens"] => {
            let tokens = host::tokens::Tokens::new(tokens_path(&flags));
            let list = tokens.list().unwrap_or_else(|e| die(e));
            print(serde_json::json!({
                "tokens": list.iter().map(|t| t.to_json()).collect::<Vec<_>>()
            }));
        }
        ["revoke", id] => {
            let tokens = host::tokens::Tokens::new(tokens_path(&flags));
            if !tokens.revoke(id).unwrap_or_else(|e| die(e)) {
                die(format!("no token {id}; `pob agents tokens` lists them"));
            }
            // Refused from the next request on; a running host also closes
            // its sessions now
            let closed = admin(&socket_path(&flags), serde_json::json!({ "revoked": id }))
                .ok()
                .and_then(|v| v["closed"].as_u64())
                .unwrap_or(0);
            println!("revoked {id}; {closed} connection(s) closed");
        }
        ["connections"] => print(
            admin(&socket_path(&flags), serde_json::json!("connections"))
                .unwrap_or_else(|e| die(e)),
        ),
        ["disconnect", id] => {
            let id: u64 = id
                .parse()
                .unwrap_or_else(|_| die("a connection's id is a number"));
            let v = admin(
                &socket_path(&flags),
                serde_json::json!({ "disconnect": id }),
            )
            .unwrap_or_else(|e| die(e));
            if v["closed"] != true {
                die(format!(
                    "no connection {id}; `pob agents connections` lists them"
                ));
            }
            println!("closed {id}");
        }
        _ => usage(),
    }
}

/// `pob agents install`: `pob mcp` into the harnesses found, with the
/// flags given (`--build`, `--builds`, `--socket`, `--agent`)
fn install_cmd(flags: &[(&str, &str)]) {
    let exe = std::env::current_exe()
        .and_then(|e| e.canonicalize())
        .unwrap_or_else(|e| die(format!("where pob is: {e}")));
    let mut args = Vec::new();
    for (f, v) in flags {
        if matches!(*f, "--build" | "--builds" | "--socket" | "--agent") {
            args.push(f.to_string());
            args.push(v.to_string());
        }
    }
    let line = install::command_line(&exe, &args);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let env = install::Env {
        path: &path,
        home: home.as_deref(),
    };
    let mut failed = false;
    for r in install::install(&line, &env) {
        match r.outcome {
            install::Outcome::NotFound => println!("{}: not found", r.harness),
            install::Outcome::Already => println!("{}: has {} already", r.harness, install::NAME),
            install::Outcome::Added(how) => println!("{}: added ({how})", r.harness),
            install::Outcome::Failed(e) => {
                failed = true;
                println!("{}: failed: {e}", r.harness)
            }
        }
    }
    let words = install::shell_words(&line);
    println!(
        "\nBy hand, the server is the command `{words}` on stdio:\n  \
         claude mcp add --scope user {name} -- {words}\n  \
         codex mcp add {name} -- {words}\n  \
         or in a harness's JSON config: {}",
        serde_json::json!({ "mcpServers": { install::NAME: {
            "command": line[0], "args": &line[1..] } } }),
        name = install::NAME,
    );
    if failed {
        std::process::exit(1);
    }
}

fn admin(socket: &Path, req: serde_json::Value) -> Result<serde_json::Value, String> {
    host::control::local::admin(socket, req)
}

fn ctl(args: &[String]) {
    let (flags, rest) = flags(args, &["--socket", "--build", "--agent"], &["--json"]);
    let Some((verb, fields)) = rest.split_first() else {
        println!("verbs:");
        for (v, about) in agent::VERBS {
            println!("  {v:<15} {about}");
        }
        return;
    };
    let mut req = serde_json::json!({ "verb": verb });
    match fields {
        [one] if one.trim_start().starts_with('{') => {
            let v: serde_json::Value =
                serde_json::from_str(one).unwrap_or_else(|e| die(format!("not JSON: {e}")));
            for (k, v) in v.as_object().into_iter().flatten() {
                req[k] = v.clone();
            }
        }
        _ => {
            for f in fields {
                let (k, v) = f
                    .split_once('=')
                    .unwrap_or_else(|| die(format!("{f}: fields are key=value")));
                // A value that reads as JSON is JSON (numbers, lists), else a string
                req[k] = serde_json::from_str(v).unwrap_or_else(|_| serde_json::json!(v));
            }
        }
    }
    let builds: Vec<&str> = flags
        .iter()
        .filter(|(f, _)| *f == "--build")
        .map(|(_, v)| *v)
        .collect();
    let mut hello = serde_json::json!({ "hello": { "harness": "ctl", "builds": builds } });
    if let Some(a) = flag(&flags, "--agent") {
        hello["hello"]["agent"] = serde_json::json!(a);
    }
    let answer = call_host(&socket_path(&flags), &hello, &req);
    if flag(&flags, "--json").is_some() {
        println!("{answer}");
    } else if let Some(e) = answer.get("error") {
        die(e.as_str().unwrap_or_default());
    } else {
        print!("{}", text(&answer["ok"], 0));
    }
    if answer.get("error").is_some() {
        std::process::exit(1);
    }
}

fn call_host(
    socket: &Path,
    hello: &serde_json::Value,
    req: &serde_json::Value,
) -> serde_json::Value {
    let mut c = host::control::local::Client::connect(socket, hello).unwrap_or_else(|e| die(e));
    c.call(req).unwrap_or_else(|e| die(e))
}

fn mcp(args: &[String]) {
    let (flags, rest) = flags(
        args,
        &["--socket", "--builds", "--build", "--agent"],
        &["--no-start"],
    );
    if !rest.is_empty() {
        usage();
    }
    let socket = socket_path(&flags);
    let builds: Vec<String> = flags
        .iter()
        .filter(|(f, _)| *f == "--build")
        .map(|(_, v)| v.to_string())
        .collect();
    let agent_dir = flag(&flags, "--agent").map(String::from);
    let host_builds = flag(&flags, "--builds").map(String::from);
    let start = flag(&flags, "--no-start").is_none();
    let connect = move |harness: &str| -> Result<host::mcp::Relay, String> {
        let mut hello = serde_json::json!({ "hello": { "harness": harness, "builds": builds } });
        if let Some(a) = &agent_dir {
            hello["hello"]["agent"] = serde_json::json!(a);
        }
        relay(&socket, &hello, start, host_builds.as_deref())
    };
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    if let Err(e) = host::mcp::stdio(stdin, stdout, connect) {
        die(e);
    }
}

/// A relay to the host on `socket`, started when none runs and `start`
fn relay(
    socket: &Path,
    hello: &serde_json::Value,
    start: bool,
    builds: Option<&str>,
) -> Result<host::mcp::Relay, String> {
    use host::control::local::Client;
    let client = match Client::connect(socket, hello) {
        Ok(c) => c,
        Err(e) if !start || socket_answers(socket) => return Err(e),
        Err(_) => {
            let (mut child, log) = start_host(socket, builds)?;
            let deadline = Instant::now() + std::time::Duration::from_secs(30);
            loop {
                match Client::connect(socket, hello) {
                    Ok(c) => break c,
                    Err(e) => {
                        if let Ok(Some(status)) = child.try_wait() {
                            let tail = std::fs::read_to_string(&log).unwrap_or_default();
                            let tail = tail.lines().last().unwrap_or_default().to_string();
                            return Err(format!(
                                "started a host, which stopped ({status}): {tail} (its log: {})",
                                log.display()
                            ));
                        }
                        if Instant::now() > deadline {
                            return Err(format!("started a host, but it doesn't answer: {e}"));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        }
    };
    let mut client = client;
    Ok(Box::new(move |req| client.call(req)))
}

/// Whether a host listens on `socket` (then a failed hello is its answer)
fn socket_answers(socket: &Path) -> bool {
    host::control::local::answers(socket)
}

/// Starts `pob host --headless` on `socket`, on its own: its own process
/// group, its output in `host.log` beside the socket (in the data
/// directory on Windows, where the socket is a named pipe)
fn start_host(
    socket: &Path,
    builds: Option<&str>,
) -> Result<(std::process::Child, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|e| format!("starting a host: {e}"))?;
    let dir = if cfg!(windows) {
        host::data_dir().ok_or("no data directory (%LOCALAPPDATA%)")?
    } else {
        socket.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    let dir = dir.as_path();
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let log_path = dir.join("host.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["host", "--headless", "--socket"]).arg(socket);
    if let Some(b) = builds {
        cmd.args(["--builds", b]);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    // A process group of its own, without a console: it outlives the harness
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(
        &mut cmd,
        0x0000_0200 | 0x0800_0000, // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
    );
    let child = cmd
        .spawn()
        .map_err(|e| format!("starting a host: {e} (start one with `pob host --headless`)"))?;
    eprintln!(
        "pob: started a host (pid {}), its log in {}",
        child.id(),
        log_path.display()
    );
    Ok((child, log_path))
}

/// An answer as indented text: `key: value`, lists as `- item`
fn text(v: &serde_json::Value, indent: usize) -> String {
    use serde_json::Value as J;
    let pad = "  ".repeat(indent);
    let scalar = |v: &J| match v {
        J::String(s) => s.replace('\n', &format!("\n{pad}  ")),
        J::Null => "-".into(),
        v => v.to_string(),
    };
    let mut out = String::new();
    match v {
        J::Object(o) => {
            for (k, v) in o {
                match v {
                    J::Object(m) if !m.is_empty() => {
                        out.push_str(&format!("{pad}{k}:\n{}", text(v, indent + 1)))
                    }
                    J::Array(a) if !a.is_empty() => {
                        out.push_str(&format!("{pad}{k}:\n{}", text(v, indent + 1)))
                    }
                    J::Array(_) => out.push_str(&format!("{pad}{k}: none\n")),
                    v => out.push_str(&format!("{pad}{k}: {}\n", scalar(v))),
                }
            }
        }
        J::Array(a) => {
            for v in a {
                match v {
                    J::Object(_) | J::Array(_) => {
                        let inner = text(v, indent + 1);
                        out.push_str(&format!("{pad}-\n{inner}"));
                    }
                    v => out.push_str(&format!("{pad}- {}\n", scalar(v))),
                }
            }
        }
        v => out.push_str(&format!("{pad}{}\n", scalar(v))),
    }
    out
}

/// Revokes the token `pob agent` minted when the run ends, however it ends
/// short of the process being killed
struct TokenGuard {
    tokens: host::tokens::Tokens,
    id: String,
    socket: PathBuf,
}

impl Drop for TokenGuard {
    fn drop(&mut self) {
        match self.tokens.revoke(&self.id) {
            Ok(_) => {
                let _ = admin(&self.socket, serde_json::json!({ "revoked": self.id }));
                eprintln!("pob agent: token {} revoked", self.id);
            }
            Err(e) => eprintln!(
                "pob agent: token {} not revoked: {e} (`pob agents revoke {}`)",
                self.id, self.id
            ),
        }
    }
}

/// Removes the run's proxy container and network when it ends
struct Cleanup {
    podman: PathBuf,
    proxy: String,
    network: String,
    dir: Option<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let quiet = |args: &[&str]| {
            let _ = std::process::Command::new(&self.podman)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        };
        quiet(&["rm", "-f", "-t", "0", &self.proxy]);
        quiet(&["network", "rm", "-f", &self.network]);
        if let Some(d) = &self.dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// `podman <args>`, its output on success, else its error
fn podman_out(podman: &Path, args: &[String]) -> Result<String, String> {
    let out = std::process::Command::new(podman)
        .args(args)
        .output()
        .map_err(|e| format!("{}: {e}", podman.display()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "podman {}: {}",
            args.first().map_or("", |a| a.as_str()),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Copies bytes both ways between a connection and the host's endpoint
fn forward_to(
    endpoint: std::net::SocketAddr,
    mut a: impl std::io::Read + std::io::Write + Send + 'static,
    a2: impl std::io::Read + std::io::Write + Send + 'static,
) {
    let Ok(mut b) = std::net::TcpStream::connect(endpoint) else {
        return;
    };
    let Ok(mut b2) = b.try_clone() else { return };
    let mut a2 = a2;
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut a2, &mut b2);
        let _ = b2.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut b, &mut a);
    drop(a);
    let _ = t.join();
}

/// The route's listener in this process, forwarding to `endpoint`: a Unix
/// socket in `dir` (Linux), or a TCP port on `ip` (Windows); the port
fn start_forwarder(
    route: &container::Route,
    dir: Option<&Path>,
    endpoint: std::net::SocketAddr,
) -> Result<u16, String> {
    match route {
        container::Route::HostAlias => Ok(endpoint.port()),
        #[cfg(unix)]
        container::Route::Unix => {
            use std::os::unix::fs::PermissionsExt;
            let dir = dir.ok_or("no directory for the route's socket")?;
            let path = dir.join("mcp.sock");
            let l = std::os::unix::net::UnixListener::bind(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            // The proxy runs as another uid in its user namespace; what the
            // socket reaches is the endpoint on loopback, which needs a token
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))
                .map_err(|e| e.to_string())?;
            std::thread::spawn(move || {
                for c in l.incoming().flatten() {
                    std::thread::spawn(move || {
                        if let Ok(c2) = c.try_clone() {
                            forward_to(endpoint, c, c2);
                        }
                    });
                }
            });
            Ok(0)
        }
        #[cfg(not(unix))]
        container::Route::Unix => Err(format!(
            "a Unix socket route needs Unix ({:?})",
            dir.map(|d| d.display().to_string())
        )),
        container::Route::Gateway(ip) => {
            let l = std::net::TcpListener::bind((*ip, 0)).map_err(|e| format!("{ip}: {e}"))?;
            let port = l.local_addr().map_err(|e| e.to_string())?.port();
            std::thread::spawn(move || {
                for c in l.incoming().flatten() {
                    std::thread::spawn(move || {
                        if let Ok(c2) = c.try_clone() {
                            forward_to(endpoint, c, c2);
                        }
                    });
                }
            });
            Ok(port)
        }
    }
}

/// The route for this platform; on Windows from the podman machine's view
fn platform_route(podman: &Path) -> Result<container::Route, String> {
    if cfg!(target_os = "macos") {
        return Ok(container::Route::HostAlias);
    }
    if !cfg!(windows) {
        return Ok(container::Route::Unix);
    }
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let wslconfig = home
        .map(|h| h.join(".wslconfig"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    if container::wsl_mirrored(&wslconfig) {
        return Err(
            "WSL's mirrored networking (networkingMode=mirrored in .wslconfig) isn't supported \
             by `pob agent` yet; WSL's default, NAT, is. Use --no-container, or NAT"
                .into(),
        );
    }
    let routes = podman_out(
        podman,
        &["machine", "ssh", "ip route show default"].map(String::from),
    )?;
    container::default_gateway(&routes)
        .map(container::Route::Gateway)
        .ok_or_else(|| format!("no default gateway in the podman machine: {routes}"))
}

/// `pob agent <harness>`: the harness in the default container, with the
/// host's MCP endpoint and a token for the builds named
fn agent_cmd(args: &[String]) {
    use container::Harness;
    let Some((harness, args)) = args.split_first() else {
        usage()
    };
    let harness = Harness::parse(harness).unwrap_or_else(|| {
        die(format!(
            "pob agent: {harness}: the harnesses are claude and codex"
        ))
    });
    // The harness's own arguments come after `--`
    let (args, extra) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let (flags, rest) = flags(
        args,
        &[
            "--build",
            "--agent",
            "--allow",
            "--socket",
            "--builds",
            "--tokens",
            "--endpoint",
        ],
        &["--no-container", "--rebuild", "--reset-login", "--exec"],
    );
    if !rest.is_empty() {
        usage();
    }
    let builds: Vec<&str> = flags
        .iter()
        .filter(|(f, _)| *f == "--build")
        .map(|(_, v)| *v)
        .collect();
    let socket = socket_path(&flags);

    if flag(&flags, "--no-container").is_some() {
        let exe = std::env::current_exe().unwrap_or_else(|e| die(format!("where pob is: {e}")));
        let mut mcp_flags = Vec::new();
        for (f, v) in &flags {
            if matches!(*f, "--build" | "--builds" | "--socket" | "--agent") {
                mcp_flags.extend([f.to_string(), v.to_string()]);
            }
        }
        let line = install::command_line(&exe, &mcp_flags);
        let cmd = harness.local_command(&line, extra);
        eprintln!(
            "pob agent: {} in this terminal, not sandboxed: it runs as you, with your files",
            harness.name()
        );
        let status = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .status()
            .unwrap_or_else(|e| die(format!("{}: {e}", cmd[0])));
        std::process::exit(status.code().unwrap_or(1));
    }

    let podman = std::env::var_os("POB_PODMAN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("podman"));
    if podman_out(&podman, &["--version".into()]).is_err() {
        die(
            "pob agent: no Podman. Install it (https://podman.io/docs/installation; on macOS and \
             Windows then `podman machine init` and `podman machine start`), or run the harness \
             here, unsandboxed, with --no-container",
        );
    }
    let route = platform_route(&podman).unwrap_or_else(|e| die(format!("pob agent: {e}")));

    // The host, and its MCP endpoint over HTTP
    let endpoint: std::net::SocketAddr = flag(&flags, "--endpoint")
        .unwrap_or(host::http::DEFAULT_LISTEN)
        .parse()
        .unwrap_or_else(|_| die("pob agent: --endpoint is an address, 127.0.0.1:8731"));
    let mut started = None;
    if !socket_answers(&socket) {
        started = Some(start_host(&socket, flag(&flags, "--builds")).unwrap_or_else(|e| die(e)));
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    while std::net::TcpStream::connect(endpoint).is_err() {
        if let Some((child, log)) = &mut started
            && let Ok(Some(status)) = child.try_wait()
        {
            let tail = std::fs::read_to_string(&*log).unwrap_or_default();
            die(format!(
                "pob agent: the host stopped ({status}): {} (its log: {})",
                tail.lines().last().unwrap_or_default(),
                log.display()
            ));
        }
        if Instant::now() > deadline {
            die(format!(
                "pob agent: no MCP endpoint on {endpoint}: the host serves MCP over HTTP there \
                 unless started with --listen"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // A token for this run alone
    let tokens = host::tokens::Tokens::new(tokens_path(&flags));
    let now = host::control::now();
    let dir = flag(&flags, "--agent")
        .map(String::from)
        .unwrap_or_else(|| agent::agent_dir_name(harness.connection(), now));
    let mut grant = agent::Grant::new(harness.connection().to_string(), dir);
    for b in &builds {
        if *b == "agents" {
            die("agents is not a build");
        }
        grant = grant.with_build(*b);
    }
    let command = if flag(&flags, "--exec").is_some() {
        if extra.is_empty() {
            die("pob agent --exec: the command goes after --");
        }
        extra.to_vec()
    } else {
        harness.container_command(extra)
    };
    let (token, entry) = tokens.mint(&grant, now).unwrap_or_else(|e| die(e));
    // Everything after the token returns here, so its guards run
    let run = || -> Result<i32, String> {
        let _token = TokenGuard {
            tokens: host::tokens::Tokens::new(tokens_path(&flags)),
            id: entry.id.clone(),
            socket: socket.clone(),
        };

        // The images, built here from the context `pob` ships
        let tag = container::context_tag();
        let (agent_image, proxy_image) = (format!("pob-agent:{tag}"), format!("pob-proxy:{tag}"));
        let rebuild = flag(&flags, "--rebuild").is_some();
        let exists = |image: &str| {
            podman_out(&podman, &["image".into(), "exists".into(), image.into()]).is_ok()
        };
        if rebuild || !exists(&agent_image) || !exists(&proxy_image) {
            let ctx =
                std::env::temp_dir().join(format!("pob-agent-context-{}", std::process::id()));
            for (path, text) in container::CONTEXT {
                let p = ctx.join(path);
                std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::write(&p, text).map_err(|e| e.to_string())?;
            }
            for (target, image) in [("proxy", &proxy_image), ("agent", &agent_image)] {
                eprintln!("pob agent: building {image}");
                let mut cmd = std::process::Command::new(&podman);
                cmd.args(["build", "--target", target, "-t", image]);
                if rebuild {
                    cmd.args(["--pull=always", "--no-cache"]);
                }
                let ok = cmd.arg(&ctx).status().is_ok_and(|s| s.success());
                if !ok {
                    let _ = std::fs::remove_dir_all(&ctx);
                    return Err(format!("building {image} failed"));
                }
            }
            let _ = std::fs::remove_dir_all(&ctx);
        }

        let names = container::Names::new(harness, &std::process::id().to_string());
        if flag(&flags, "--reset-login").is_some() {
            let _ = podman_out(
                &podman,
                &[
                    "volume".into(),
                    "rm".into(),
                    "-f".into(),
                    names.login.clone(),
                ],
            );
            eprintln!(
                "pob agent: removed {}'s login ({})",
                harness.name(),
                names.login
            );
        }

        // The route's end in this process
        let route_dir = match route {
            container::Route::Unix => {
                let base = std::env::var_os("XDG_RUNTIME_DIR")
                    .map(PathBuf::from)
                    .or_else(host::data_dir)
                    .unwrap_or_else(std::env::temp_dir);
                let d = base.join(format!("pob-agent-{}", std::process::id()));
                std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755));
                }
                Some(d)
            }
            _ => None,
        };
        let _cleanup = Cleanup {
            podman: podman.clone(),
            proxy: names.proxy.clone(),
            network: names.network.clone(),
            dir: route_dir.clone(),
        };
        let port = start_forwarder(&route, route_dir.as_deref(), endpoint)
            .map_err(|e| format!("the route to the host: {e}"))?;

        // The network, the proxy, the harness
        let step = |args: Vec<String>| podman_out(&podman, &args);
        step(
            [
                "network",
                "create",
                "--internal",
                "--disable-dns",
                &names.network,
            ]
            .map(String::from)
            .to_vec(),
        )?;
        let allow: Vec<String> = flags
            .iter()
            .filter(|(f, _)| *f == "--allow")
            .map(|(_, v)| v.to_string())
            .collect();
        let dir_s = route_dir.as_ref().map(|d| d.display().to_string());
        step(container::proxy_args(
            &names,
            &proxy_image,
            harness,
            &route,
            port,
            dir_s.as_deref(),
            &allow,
        ))?;
        step(
            ["network", "connect", "podman", &names.proxy]
                .map(String::from)
                .to_vec(),
        )?;
        let ip = step(vec![
            "inspect".into(),
            "-f".into(),
            format!(
                "{{{{(index .NetworkSettings.Networks \"{}\").IPAddress}}}}",
                names.network
            ),
            names.proxy.clone(),
        ])?;
        let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
        eprintln!(
            "pob agent: {} in the container, on {} (token {}, agent directory {})",
            harness.name(),
            if entry.builds.is_empty() {
                "no build of yours".to_string()
            } else {
                entry.builds.join(", ")
            },
            entry.id,
            entry.agent
        );
        let status = std::process::Command::new(&podman)
            .args(container::agent_args(
                &names,
                &agent_image,
                &ip,
                tty,
                &command,
            ))
            .env("POB_TOKEN", &token)
            .status()
            .map_err(|e| format!("{}: {e}", podman.display()))?;
        Ok(status.code().unwrap_or(1))
    };
    let code = run().unwrap_or_else(|e| {
        eprintln!("pob agent: {e}");
        1
    });
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pob agent`'s token: minted for the run, revoked when its guard goes
    #[test]
    fn token_lifecycle() {
        let dir = std::env::temp_dir().join(format!("pob-agent-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tokens.json");
        let tokens = host::tokens::Tokens::new(&file);
        let grant = agent::Grant::new("claude-code".to_string(), "claude-code-x".to_string())
            .with_build("RF");
        let (token, entry) = tokens.mint(&grant, 0).unwrap();
        let other = tokens.mint(&grant, 0).unwrap().1;
        assert!(token.starts_with(host::tokens::PREFIX));
        assert_eq!(tokens.list().unwrap().len(), 2);
        {
            let _guard = TokenGuard {
                tokens: host::tokens::Tokens::new(&file),
                id: entry.id.clone(),
                socket: dir.join("no-host.sock"),
            };
        }
        let left: Vec<String> = tokens.list().unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(left, [other.id]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
