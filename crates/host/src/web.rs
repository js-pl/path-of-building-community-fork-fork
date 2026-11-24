//! The web app on the host: its static files (the built `web/` bundle)
//! and the WebSocket the UI speaks the engine API on, at `/ws`
//! ([`crate::ui`]). On `127.0.0.1` only.
//!
//! - **The key.** Each launch makes a secret key. The host opens the
//!   browser at `/?key=<key>`, which is answered with an HttpOnly,
//!   SameSite=Strict cookie holding it and sent on to `/`. Every other
//!   request needs the cookie: without it a page says to open the link the
//!   host printed.
//! - **Origins.** The WebSocket's `Origin` must be the host's own
//!   (`http://127.0.0.1:<port>` or `http://localhost:<port>`), against
//!   other pages and DNS rebinding; there are no CORS headers.
//! - **Served by the host.** `index.html` gets `<meta name="pob-host">`,
//!   which makes the app use the WebSocket instead of its workers.

use crate::http::{Request, Response, read_request, reason, write_response};
use crate::ui::Msg;
use crate::ws::{self, Message, Out};
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// The address the web app is served on by default
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8730";
/// The WebSocket's path
pub const WS_PATH: &str = "/ws";
/// The cookie that holds the launch's key
const COOKIE: &str = "pob_key";
/// What marks the app as served by the host
pub const HOST_META: &str = r#"<meta name="pob-host" content="ws">"#;
const MAX_CONNECTIONS: usize = 64;

/// The web app's server
pub struct Web {
    root: PathBuf,
    key: String,
    port: u16,
    msgs: Mutex<mpsc::Sender<Msg>>,
    next: AtomicU64,
    active: AtomicUsize,
}

/// A new launch's key: 32 random bytes in hex
pub fn new_key() -> Result<String, String> {
    Ok(crate::tokens::random(32)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Equal, in time that doesn't depend on where they differ
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

impl Web {
    /// Serves the bundle in `root`, on `port`, passing the UI's messages
    /// to the engine thread
    pub fn new(root: PathBuf, key: String, port: u16, msgs: mpsc::Sender<Msg>) -> Arc<Web> {
        Arc::new(Web {
            root,
            key,
            port,
            msgs: Mutex::new(msgs),
            next: AtomicU64::new(1),
            active: AtomicUsize::new(0),
        })
    }

    /// The link that opens the app
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?key={}", self.port, self.key)
    }

    /// Accepts connections on a thread of their own each
    pub fn serve(self: Arc<Self>, listener: TcpListener) {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let me = self.clone();
            std::thread::spawn(move || me.connection(stream));
        }
    }

    fn connection(&self, stream: TcpStream) {
        let n = self.active.fetch_add(1, Ordering::SeqCst);
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(300)));
        if n < MAX_CONNECTIONS {
            let _ = self.exchange(stream);
        } else {
            let mut s = stream;
            let _ = write_response(&mut s, &Response::text(503, "too many connections"), true);
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
    }

    fn exchange(&self, stream: TcpStream) -> std::io::Result<()> {
        let mut writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        loop {
            let req = match read_request(&mut reader) {
                Ok(Some(r)) => r,
                Ok(None) => return Ok(()),
                Err(e) => {
                    let status = e.to_string().parse().unwrap_or(400);
                    if matches!(status, 400 | 413 | 431) {
                        let _ = write_response(
                            &mut writer,
                            &Response::text(status, reason(status)),
                            true,
                        );
                    }
                    return Ok(());
                }
            };
            if req.path == WS_PATH {
                return match self.upgrade(&req) {
                    Ok(key) => self.websocket(reader, writer, &key),
                    Err(r) => write_response(&mut writer, &r, true),
                };
            }
            let r = self.handle(&req);
            write_response(&mut writer, &r, req.close)?;
            if req.close {
                return Ok(());
            }
        }
    }

    fn has_key(&self, req: &Request) -> bool {
        req.header("cookie").is_some_and(|c| {
            c.split(';').any(|kv| {
                kv.trim()
                    .split_once('=')
                    .is_some_and(|(k, v)| k == COOKIE && same(v, &self.key))
            })
        })
    }

    /// Whether a page's origin is the host's own
    pub fn own_origin(&self, origin: &str) -> bool {
        [
            format!("http://127.0.0.1:{}", self.port),
            format!("http://localhost:{}", self.port),
            format!("http://[::1]:{}", self.port),
        ]
        .iter()
        .any(|o| o.eq_ignore_ascii_case(origin))
    }

    /// Checks a WebSocket handshake; the client's key
    fn upgrade(&self, req: &Request) -> Result<String, Response> {
        if !req.header("origin").is_some_and(|o| self.own_origin(o)) {
            return Err(Response::text(
                403,
                "the WebSocket is for the host's own page",
            ));
        }
        if !self.has_key(req) {
            return Err(Response::text(403, "open the link `pob host` printed"));
        }
        let upgrade = req
            .header("upgrade")
            .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
        match req.header("sec-websocket-key") {
            Some(k) if upgrade && req.method == "GET" => Ok(k.to_string()),
            _ => Err(Response::text(400, "a WebSocket handshake is expected")),
        }
    }

    fn websocket(
        &self,
        mut reader: BufReader<TcpStream>,
        mut writer: TcpStream,
        key: &str,
    ) -> std::io::Result<()> {
        write!(
            writer,
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            ws::accept_key(key)
        )?;
        writer.flush()?;
        reader.get_ref().set_read_timeout(None)?;
        let client = self.next.fetch_add(1, Ordering::SeqCst);
        let (out, frames) = mpsc::channel::<Out>();
        // The frames go out in the order the engine thread sends them
        let mut w = writer.try_clone()?;
        let sender = std::thread::spawn(move || {
            for f in frames {
                let close = f == Out::Close;
                if ws::write(&mut w, &f).is_err() || close {
                    break;
                }
            }
            let _ = w.shutdown(std::net::Shutdown::Both);
        });
        let msgs = self.msgs.lock().unwrap().clone();
        let _ = msgs.send(Msg::Open {
            client,
            out: out.clone(),
        });
        loop {
            match ws::read(&mut reader, false) {
                Ok(Message::Text(text)) => {
                    if msgs.send(Msg::Ui { client, text }).is_err() {
                        break;
                    }
                }
                Ok(Message::Ping(p)) => {
                    let _ = out.send(Out::Pong(p));
                }
                Ok(Message::Pong) => {}
                Ok(Message::Close) | Err(_) => break,
            }
        }
        let _ = msgs.send(Msg::Close { client });
        let _ = out.send(Out::Close);
        drop(out);
        let _ = sender.join();
        Ok(())
    }

    /// Answers a request for a page or file
    pub fn handle(&self, req: &Request) -> Response {
        if req.method != "GET" && req.method != "HEAD" {
            return Response::text(405, "GET only");
        }
        let presented = req.query.split('&').find_map(|kv| kv.strip_prefix("key="));
        if let Some(k) = presented {
            if !same(k, &self.key) {
                return Response::text(
                    403,
                    "a key from another launch: open the link `pob host` printed",
                );
            }
            let mut r = Response::new(303);
            r.headers.push(("Location", req.path.clone()));
            r.headers.push((
                "Set-Cookie",
                format!("{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/", self.key),
            ));
            return r;
        }
        if !self.has_key(req) {
            let mut r = Response::text(
                403,
                "Path of Building: open the link `pob host` printed (it holds this launch's key)",
            );
            r.headers.push(("Cache-Control", "no-store".into()));
            return r;
        }
        let rel = req.path.trim_start_matches('/');
        let rel = if rel.is_empty() { "index.html" } else { rel };
        let ok = rel
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains(['\\', '%', ':']));
        if !ok {
            return Response::text(404, "not found");
        }
        let path = self.root.join(rel);
        let Ok(mut body) = std::fs::read(&path) else {
            return Response::text(404, "not found");
        };
        if rel == "index.html" {
            let html = String::from_utf8_lossy(&body);
            body = html
                .replacen("<head>", &format!("<head>\n    {HOST_META}"), 1)
                .into_bytes();
        }
        let mut r = Response::new(200);
        r.headers.push(("Content-Type", content_type(&path).into()));
        r.headers.push(("Cache-Control", "no-cache".into()));
        if req.method == "GET" {
            r.body = body;
        }
        r
    }
}

/// Binds the web app's address, which must be loopback
pub fn listen(addr: &str) -> Result<(TcpListener, u16), String> {
    let (l, a) = crate::http::listen(addr)?;
    if !a.ip().is_loopback() {
        return Err(format!("{a}: the web app is served on loopback only"));
    }
    Ok((l, a.port()))
}

/// Opens a link in the user's browser
pub fn open_browser(url: &str) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A client of the web app's WebSocket, as the browser is (tests and
/// tools): the handshake with the key's cookie and the host's origin
pub struct WsClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl WsClient {
    pub fn connect(port: u16, key: &str, origin: &str) -> Result<WsClient, String> {
        let s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        let mut writer = s.try_clone().map_err(|e| e.to_string())?;
        write!(
            writer,
            "GET {WS_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nOrigin: {origin}\r\nCookie: {COOKIE}={key}\r\n\r\n"
        )
        .map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(s);
        let mut status = String::new();
        loop {
            let mut line = String::new();
            std::io::BufRead::read_line(&mut reader, &mut line).map_err(|e| e.to_string())?;
            if status.is_empty() {
                status = line.trim().to_string();
            }
            if line.trim().is_empty() {
                break;
            }
        }
        if !status.contains(" 101 ") {
            let mut rest = String::new();
            let _ = reader.read_to_string(&mut rest);
            return Err(format!("{status}: {}", rest.trim()));
        }
        Ok(WsClient { reader, writer })
    }

    pub fn send(&mut self, text: &str) -> Result<(), String> {
        ws::write_client(&mut self.writer, text).map_err(|e| e.to_string())
    }

    /// The next text message, as JSON
    pub fn recv(&mut self) -> Result<serde_json::Value, String> {
        loop {
            match ws::read(&mut self.reader, true).map_err(|e| e.to_string())? {
                Message::Text(t) => return serde_json::from_str(&t).map_err(|e| e.to_string()),
                Message::Close => return Err("closed".into()),
                _ => {}
            }
        }
    }
}
