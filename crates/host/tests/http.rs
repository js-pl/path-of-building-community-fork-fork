//! MCP over streamable HTTP with tokens: initialize, the tools, a read and
//! an edit that commits; a missing, wrong or revoked token is refused, a
//! token's grant reaches no other build, and HTTPS serves the same.

use agent::{Agent, Grant};
use host::FsStore;
use host::connections::Registry;
use host::control::Call;
use host::http::{self, Http};
use host::tokens::Tokens;
use serde_json::{Value as J, json};
use session::typed::TypedData;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pob-http-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: J,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One request on a connection of its own
fn send(
    stream: &mut (impl Read + Write),
    method: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let mut req = format!("{method} /mcp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!(
        "Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line.split(' ').nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    let mut len = 0;
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':').unwrap();
        if k.eq_ignore_ascii_case("content-length") {
            len = v.trim().parse().unwrap();
        }
        headers.push((k.to_string(), v.trim().to_string()));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).unwrap();
    let body =
        serde_json::from_slice(&body).unwrap_or_else(|_| json!(String::from_utf8_lossy(&body)));
    Answer {
        status,
        headers,
        body,
    }
}

struct Client {
    addr: SocketAddr,
    token: String,
    session: Option<String>,
    id: u64,
}

impl Client {
    fn post(&mut self, extra: &[(&str, &str)], msg: &J) -> Answer {
        let auth = format!("Bearer {}", self.token);
        let mut h = vec![("Authorization", auth.as_str())];
        let session = self.session.clone().unwrap_or_default();
        if self.session.is_some() {
            h.push(("Mcp-Session-Id", session.as_str()));
            h.push(("MCP-Protocol-Version", "2025-06-18"));
        }
        h.extend_from_slice(extra);
        let mut s = TcpStream::connect(self.addr).unwrap();
        send(&mut s, "POST", &h, &msg.to_string())
    }

    fn rpc(&mut self, method: &str, params: J) -> Answer {
        self.id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": self.id, "method": method, "params": params });
        self.post(&[], &msg)
    }

    fn tool(&mut self, name: &str, args: J) -> J {
        let a = self.rpc("tools/call", json!({ "name": name, "arguments": args }));
        assert_eq!(a.status, 200, "{}", a.body);
        a.body["result"].clone()
    }

    fn initialize(&mut self) -> Answer {
        let a = self.rpc(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "codex", "version": "1" } }),
        );
        if a.status == 200 {
            self.session = a.header("mcp-session-id").map(String::from);
        }
        a
    }
}

#[test]
fn mcp_over_http_with_tokens() {
    let git = history::Git::find();
    let dir = temp("tokens");
    let builds = dir.join("builds");
    let xml = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/generated/gen-0003.xml"),
    )
    .unwrap();
    for b in ["RF", "Other"] {
        std::fs::create_dir_all(builds.join(b)).unwrap();
        std::fs::write(builds.join(b).join("build.xml"), &xml).unwrap();
    }
    let tokens = Tokens::new(dir.join("tokens.json"));
    let (token, entry) = tokens
        .mint(&Grant::new("codex", "codex-1").with_build("RF"), 0)
        .unwrap();
    let (other, _) = tokens
        .mint(&Grant::new("x", "x-1").with_build("Other"), 0)
        .unwrap();

    let (tx, rx) = mpsc::channel::<Call>();
    let registry = Arc::new(Registry::default());
    let endpoint = Http::new(Tokens::new(dir.join("tokens.json")), tx, registry.clone());
    let (listener, addr) = http::listen("127.0.0.1:0").unwrap();
    std::thread::spawn(move || endpoint.serve(listener, None));

    let reg = registry.clone();
    let tokens_path = dir.join("tokens.json");
    let builds_dir = builds.clone();
    let has_git = git.is_some();
    let client = std::thread::spawn(move || {
        // No token, a wrong one
        let mut s = TcpStream::connect(addr).unwrap();
        let a = send(&mut s, "POST", &[], "{}");
        assert_eq!(a.status, 401);
        assert!(a.header("www-authenticate").unwrap().starts_with("Bearer"));
        let mut c = Client {
            addr,
            token: "pob_0000".into(),
            session: None,
            id: 0,
        };
        assert_eq!(c.initialize().status, 401);

        let mut c = Client {
            addr,
            token: token.clone(),
            session: None,
            id: 0,
        };
        let a = c.initialize();
        assert_eq!(a.status, 200, "{}", a.body);
        assert_eq!(a.body["result"]["protocolVersion"], "2025-06-18");
        assert!(c.session.is_some());
        let n = c.post(
            &[],
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        );
        assert_eq!(n.status, 202);
        let list = c.rpc("tools/list", json!({}));
        assert!(
            list.body["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "summary")
        );

        let summary = c.tool("summary", json!({}));
        assert_eq!(summary["structuredContent"]["build"], "RF", "{summary}");
        let edit = c.tool("allocate", json!({ "nodes": [36634] }));
        assert_ne!(edit["isError"], true, "{edit}");
        if has_git {
            let commit = edit["structuredContent"]["commit"].as_str().unwrap();
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(builds_dir.join("RF"))
                .args(["log", "-1", "--format=%h%n%b"])
                .output()
                .unwrap();
            let log = String::from_utf8_lossy(&out.stdout).to_string();
            assert!(log.starts_with(&commit[..7]), "{log}");
            assert!(log.contains("Source: agent codex"), "{log}");
        }
        // The grant reaches no other build
        let refused = c.tool("summary", json!({ "build": "Other" }));
        assert_eq!(refused["isError"], true, "{refused}");
        let refused = c.tool("copy", json!({ "build": "Other", "name": "mine" }));
        assert_eq!(refused["isError"], true, "{refused}");

        // Another token's session is not this one's
        let mut o = Client {
            addr,
            token: other,
            session: c.session.clone(),
            id: 9,
        };
        assert_eq!(o.rpc("tools/list", json!({})).status, 404);
        // No session; a page elsewhere; GET
        let mut n = Client {
            addr,
            token: token.clone(),
            session: None,
            id: 0,
        };
        assert_eq!(n.rpc("tools/list", json!({})).status, 400);
        assert_eq!(
            c.post(
                &[("Origin", "https://evil.example")],
                &json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })
            )
            .status,
            403
        );
        let auth = format!("Bearer {token}");
        let mut s = TcpStream::connect(addr).unwrap();
        assert_eq!(
            send(&mut s, "GET", &[("Authorization", &auth)], "").status,
            405
        );

        // Listed, then revoked: refused at once
        let conns = reg.list();
        let conn = conns
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["token"] == entry.id.as_str())
            .unwrap();
        assert_eq!(conn["kind"], "http");
        assert_eq!(conn["client"], "codex");
        assert_eq!(conn["builds"], json!(["RF"]));
        assert!(Tokens::new(&tokens_path).revoke(&entry.id).unwrap());
        assert_eq!(c.rpc("tools/list", json!({})).status, 401);
        assert_eq!(c.initialize().status, 401);
        assert_eq!(reg.disconnect_token(&entry.id), 1);
    });

    let mut agent = Agent::new(data(), FsStore::new(&builds, git));
    while !client.is_finished() {
        if let Ok(c) = rx.recv_timeout(std::time::Duration::from_millis(20)) {
            let _ = c.reply.send(agent.call_str(&c.grant, &c.line));
        }
    }
    client.join().unwrap();
    assert_eq!(
        std::fs::read_to_string(builds.join("Other/build.xml")).unwrap(),
        xml
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn data() -> &'static TypedData {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../upstream/PathOfBuilding/src");
    Box::leak(Box::new(TypedData::from_source(&src).unwrap()))
}

/// A self-signed certificate for localhost, made with openssl
fn certificate(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    let ok = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:prime256v1",
            "-nodes",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .args([
            "-days",
            "2",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost,IP:127.0.0.1",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
        ])
        .output()
        .is_ok_and(|o| o.status.success());
    ok.then_some((cert, key))
}

#[test]
fn https_with_the_users_certificate() {
    let dir = temp("tls");
    let Some((cert, key)) = certificate(&dir) else {
        eprintln!("no openssl: skipped");
        return;
    };
    let tls = http::tls_config(&cert, &key).unwrap();
    let tokens = Tokens::new(dir.join("tokens.json"));
    let (token, _) = tokens.mint(&Grant::new("t", "t-1"), 0).unwrap();
    let (tx, _rx) = mpsc::channel();
    let endpoint = Http::new(tokens, tx, Arc::new(Registry::default()));
    let (listener, addr) = http::listen("127.0.0.1:0").unwrap();
    std::thread::spawn(move || endpoint.serve(listener, Some(tls)));

    use rustls::pki_types::pem::PemObject;
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls::pki_types::CertificateDer::pem_file_iter(&cert).unwrap() {
        roots.add(c.unwrap()).unwrap();
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let conn =
        rustls::ClientConnection::new(Arc::new(config), "localhost".try_into().unwrap()).unwrap();
    let mut s = rustls::StreamOwned::new(conn, TcpStream::connect(addr).unwrap());
    let auth = format!("Bearer {token}");
    let a = send(
        &mut s,
        "POST",
        &[("Authorization", &auth)],
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-11-25" } }).to_string(),
    );
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(a.body["result"]["protocolVersion"], "2025-11-25");
    assert!(a.header("mcp-session-id").is_some());
    // Plain HTTP on the HTTPS port gets nothing
    let mut plain = TcpStream::connect(addr).unwrap();
    plain
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let _ = plain.write_all(b"POST /mcp HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
    let mut buf = [0u8; 16];
    let n = plain.read(&mut buf).unwrap_or(0);
    assert!(!buf[..n].starts_with(b"HTTP/1.1 200"));
    let _ = std::fs::remove_dir_all(&dir);
}
