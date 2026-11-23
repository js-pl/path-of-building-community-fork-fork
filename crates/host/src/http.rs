//! MCP over streamable HTTP, for a harness in a container, a VM or on
//! another machine: one endpoint, `/mcp`, a JSON-RPC message per POST,
//! answered with one JSON body (the host sends no server-initiated
//! messages, so there is no SSE stream: GET is 405).
//!
//! - **Tokens.** Every request carries `Authorization: Bearer <token>`; the
//!   token's grant ([`crate::tokens`]) is the session's. A token missing,
//!   wrong or revoked is 401, checked on every request, so a revoked token
//!   is refused at once.
//! - **Sessions.** `initialize` makes one (`Mcp-Session-Id` in the answer);
//!   later requests carry it, and a session of another token, or one that
//!   ended, is 404. DELETE ends one.
//! - **Origins.** A request from a web page (an `Origin` header) is refused
//!   unless the page is on this machine, against DNS rebinding.
//! - **Limits.** Bodies up to [`crate::control::MAX_LINE`], 64 connections
//!   and 256 sessions at a time (the least recently used one goes).

use crate::connections::Registry;
use crate::control::{Call, MAX_LINE};
use crate::mcp;
use crate::tokens::{Token, Tokens};
use serde_json::{Value as J, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

/// The TLS configuration [`tls_config`] makes
pub use rustls::ServerConfig as TlsConfig;

/// The address the endpoint listens on by default
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8731";
/// The endpoint's path
pub const PATH: &str = "/mcp";
const MAX_CONNECTIONS: usize = 64;
const MAX_SESSIONS: usize = 256;
const MAX_HEADERS: usize = 64 << 10;
/// A connection's idle time before it is closed
const IDLE: Duration = Duration::from_secs(300);

/// Who can reach an address
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// This machine only
    Loopback,
    /// A container network or the WSL adapter: link-local, Podman's
    /// default 10.88.0.0/16, Docker's and WSL's 172.16.0.0/12
    Containers,
    /// Anything else (all interfaces, a LAN, the internet): HTTPS only
    Open,
}

pub fn exposure(ip: IpAddr) -> Exposure {
    if ip.is_loopback() {
        return Exposure::Loopback;
    }
    let containers = match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_link_local()
                || (o[0] == 10 && o[1] == 88)
                || (o[0] == 172 && (16..32).contains(&o[1]))
        }
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    };
    if containers {
        Exposure::Containers
    } else {
        Exposure::Open
    }
}

/// The certificate and key (PEM files) the user supplies, as a TLS config
pub fn tls_config(cert: &Path, key: &Path) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs = CertificateDer::pem_file_iter(cert)
        .and_then(|i| i.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("{}: {e}", cert.display()))?;
    if certs.is_empty() {
        return Err(format!("{}: no certificate", cert.display()));
    }
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| format!("{}: {e}", key.display()))?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| format!("the certificate and key: {e}"))?;
    Ok(Arc::new(config))
}

struct Session {
    token: String,
    server: mcp::Server,
    connection: u64,
    last: Instant,
}

/// The endpoint's state, shared by its connections
pub struct Http {
    tokens: Tokens,
    calls: Mutex<mpsc::Sender<Call>>,
    sessions: Mutex<HashMap<String, Session>>,
    registry: Arc<Registry>,
    active: AtomicUsize,
    me: Weak<Http>,
}

/// A request, as far as the endpoint reads it
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    close: bool,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// An answer: status, headers, body
pub struct Response {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn new(status: u16) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn json(status: u16, v: &J) -> Response {
        let mut r = Response::new(status);
        r.headers.push(("Content-Type", "application/json".into()));
        r.body = v.to_string().into_bytes();
        r
    }

    fn text(status: u16, t: &str) -> Response {
        let mut r = Response::new(status);
        r.headers
            .push(("Content-Type", "text/plain; charset=utf-8".into()));
        r.body = t.as_bytes().to_vec();
        r
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// A line of the head, CRLF or LF ended
fn head_line(r: &mut impl BufRead, budget: &mut usize) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = r.take(*budget as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if n > *budget {
        return Err(std::io::Error::other("431"));
    }
    *budget -= n;
    Ok(Some(String::from_utf8_lossy(&buf).trim_end().to_string()))
}

/// Reads one request; `None` when the client closed the connection
pub fn read_request(r: &mut impl BufRead) -> std::io::Result<Option<Request>> {
    let mut budget = MAX_HEADERS;
    let line = loop {
        match head_line(r, &mut budget)? {
            None => return Ok(None),
            Some(l) if l.is_empty() => continue,
            Some(l) => break l,
        }
    };
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(std::io::Error::other("400"));
    };
    let mut headers = Vec::new();
    loop {
        let Some(l) = head_line(r, &mut budget)? else {
            return Ok(None);
        };
        if l.is_empty() {
            break;
        }
        let Some((k, v)) = l.split_once(':') else {
            return Err(std::io::Error::other("400"));
        };
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let mut req = Request {
        method: method.to_string(),
        path: target.split('?').next().unwrap_or("").to_string(),
        headers,
        body: Vec::new(),
        close: version == "HTTP/1.0",
    };
    if let Some(c) = req.header("connection") {
        if c.eq_ignore_ascii_case("close") {
            req.close = true;
        } else if c.eq_ignore_ascii_case("keep-alive") {
            req.close = false;
        }
    }
    let chunked = req
        .header("transfer-encoding")
        .is_some_and(|t| t.to_ascii_lowercase().contains("chunked"));
    if chunked {
        loop {
            let size = head_line(r, &mut budget)?.ok_or(std::io::Error::other("400"))?;
            let size = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16)
                .map_err(|_| std::io::Error::other("400"))?;
            if size == 0 {
                // Trailers, then the blank line
                while head_line(r, &mut budget)?.is_some_and(|l| !l.is_empty()) {}
                break;
            }
            if req.body.len() + size > MAX_LINE {
                return Err(std::io::Error::other("413"));
            }
            let start = req.body.len();
            req.body.resize(start + size, 0);
            r.read_exact(&mut req.body[start..])?;
            let mut crlf = String::new();
            r.read_line(&mut crlf)?;
        }
    } else if let Some(n) = req.header("content-length") {
        let n: usize = n.parse().map_err(|_| std::io::Error::other("400"))?;
        if n > MAX_LINE {
            return Err(std::io::Error::other("413"));
        }
        req.body.resize(n, 0);
        r.read_exact(&mut req.body)?;
    }
    Ok(Some(req))
}

fn write_response(w: &mut impl Write, r: &Response, close: bool) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", r.status, reason(r.status));
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n", r.body.len()));
    if close {
        head.push_str("Connection: close\r\n");
    }
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;
    w.write_all(&r.body)?;
    w.flush()
}

/// Whether a page's origin is on this machine
fn local_origin(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    let Some(rest) = rest else { return false };
    let host = if let Some(v6) = rest.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        rest.split([':', '/']).next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

impl Http {
    pub fn new(tokens: Tokens, calls: mpsc::Sender<Call>, registry: Arc<Registry>) -> Arc<Http> {
        Arc::new_cyclic(|me| Http {
            tokens,
            calls: Mutex::new(calls),
            sessions: Mutex::new(HashMap::new()),
            registry,
            active: AtomicUsize::new(0),
            me: me.clone(),
        })
    }

    /// Accepts connections on a thread of their own each, TLS first when
    /// `tls` is given
    pub fn serve(self: Arc<Self>, listener: TcpListener, tls: Option<Arc<rustls::ServerConfig>>) {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let me = self.clone();
            let tls = tls.clone();
            std::thread::spawn(move || me.connection(stream, tls));
        }
    }

    fn connection(&self, stream: TcpStream, tls: Option<Arc<rustls::ServerConfig>>) {
        let n = self.active.fetch_add(1, Ordering::SeqCst);
        let _ = stream.set_read_timeout(Some(IDLE));
        let _ = stream.set_nodelay(true);
        if n >= MAX_CONNECTIONS {
            let mut s = stream;
            let r = Response::text(503, "too many connections");
            if tls.is_none() {
                let _ = write_response(&mut s, &r, true);
            }
        } else {
            match tls {
                Some(cfg) => {
                    if let Ok(c) = rustls::ServerConnection::new(cfg) {
                        self.exchange(rustls::StreamOwned::new(c, stream));
                    }
                }
                None => self.exchange(stream),
            }
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
    }

    /// Requests and answers on one connection until it closes
    fn exchange(&self, stream: impl Read + Write) {
        let mut stream = BufReader::new(stream);
        loop {
            let req = match read_request(&mut stream) {
                Ok(Some(r)) => r,
                Ok(None) => return,
                Err(e) => {
                    let status = match e.to_string().as_str() {
                        "413" => 413,
                        "431" => 431,
                        "400" => 400,
                        // A timeout or a broken connection
                        _ => return,
                    };
                    let _ = write_response(
                        stream.get_mut(),
                        &Response::text(status, reason(status)),
                        true,
                    );
                    return;
                }
            };
            let r = self.handle(&req);
            if write_response(stream.get_mut(), &r, req.close).is_err() || req.close {
                return;
            }
        }
    }

    /// Answers one request
    pub fn handle(&self, req: &Request) -> Response {
        if req.path != PATH {
            return Response::text(404, "the MCP endpoint is /mcp");
        }
        if let Some(o) = req.header("origin")
            && !local_origin(o)
        {
            return Response::text(403, "requests from web pages are refused");
        }
        let token = match self.authorize(req) {
            Ok(t) => t,
            Err(r) => return r,
        };
        match req.method.as_str() {
            "POST" => self.post(req, &token),
            "DELETE" => {
                let Some(id) = req.header("mcp-session-id") else {
                    return Response::text(400, "Mcp-Session-Id is missing");
                };
                let gone = {
                    let mut s = self.sessions.lock().unwrap();
                    match s.get(id) {
                        Some(sess) if sess.token == token.id => s.remove(id),
                        _ => None,
                    }
                };
                match gone {
                    Some(sess) => {
                        self.registry.remove(sess.connection);
                        Response::new(204)
                    }
                    None => Response::text(404, "no such session"),
                }
            }
            _ => {
                let mut r = Response::text(405, "POST a JSON-RPC message; there is no SSE stream");
                r.headers.push(("Allow", "POST, DELETE".into()));
                r
            }
        }
    }

    fn authorize(&self, req: &Request) -> Result<Token, Response> {
        let unauthorized = |why: &str| {
            let mut r = Response::text(401, why);
            r.headers
                .push(("WWW-Authenticate", "Bearer realm=\"pob\"".into()));
            r
        };
        let presented = req
            .header("authorization")
            .and_then(|a| {
                a.strip_prefix("Bearer ")
                    .or_else(|| a.strip_prefix("bearer "))
            })
            .map(str::trim)
            .ok_or_else(|| unauthorized("a token is needed: Authorization: Bearer <token>"))?;
        match self.tokens.find(presented) {
            Ok(Some(t)) => Ok(t),
            Ok(None) => Err(unauthorized("the token is wrong or revoked")),
            Err(e) => Err(Response::text(500, &e)),
        }
    }

    fn post(&self, req: &Request, token: &Token) -> Response {
        let msg: J = match serde_json::from_slice(&req.body) {
            Ok(m) => m,
            Err(e) => {
                return Response::json(
                    400,
                    &mcp::error(J::Null, mcp::PARSE_ERROR, format!("not JSON: {e}")),
                );
            }
        };
        if let Some(v) = req.header("mcp-protocol-version")
            && !mcp::VERSIONS.contains(&v)
        {
            return Response::text(400, &format!("protocol version {v} is not served"));
        }
        let initialize = msg["method"] == "initialize";
        let grant = token.grant();
        let (session_id, mut server) = if initialize {
            (None, mcp::Server::default())
        } else {
            let Some(id) = req.header("mcp-session-id") else {
                return Response::text(400, "Mcp-Session-Id is missing; initialize first");
            };
            let mut s = self.sessions.lock().unwrap();
            match s.get_mut(id) {
                Some(sess) if sess.token == token.id => {
                    sess.last = Instant::now();
                    self.registry.touch(sess.connection);
                    (Some(id.to_string()), sess.server.clone())
                }
                _ => return Response::text(404, "no such session; initialize again"),
            }
        };
        let calls = self.calls.lock().unwrap().clone();
        let mut call = |_: &mcp::Server, r: &J| {
            let (tx, rx) = mpsc::channel();
            let c = Call {
                grant: grant.clone(),
                line: r.to_string(),
                reply: tx,
            };
            if calls.send(c).is_err() {
                return json!({ "error": "the host is stopping" });
            }
            match rx.recv() {
                Ok(a) => {
                    serde_json::from_str(&a).unwrap_or_else(|e| json!({ "error": e.to_string() }))
                }
                Err(_) => json!({ "error": "the host is stopping" }),
            }
        };
        let answer = server.handle(&msg, &mut call);
        let mut r = match &answer {
            Some(a) => Response::json(200, a),
            None => Response::new(202),
        };
        if initialize && answer.as_ref().is_some_and(|a| a.get("result").is_some()) {
            let id = match crate::tokens::random(16) {
                Ok(b) => b.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                Err(e) => return Response::text(500, &e),
            };
            let me = self.me.clone();
            let sid = id.clone();
            let connection = self.registry.add(
                "http",
                &grant,
                Some(&token.id),
                Box::new(move || {
                    if let Some(me) = me.upgrade() {
                        me.sessions.lock().unwrap().remove(&sid);
                    }
                }),
            );
            if let Some(c) = &server.client {
                self.registry.set_client(connection, c);
            }
            let mut s = self.sessions.lock().unwrap();
            if s.len() >= MAX_SESSIONS
                && let Some(old) = s.iter().min_by_key(|(_, v)| v.last).map(|(k, _)| k.clone())
                && let Some(gone) = s.remove(&old)
            {
                self.registry.remove(gone.connection);
            }
            s.insert(
                id.clone(),
                Session {
                    token: token.id.clone(),
                    server,
                    connection,
                    last: Instant::now(),
                },
            );
            r.headers.push(("Mcp-Session-Id", id));
        } else if let Some(id) = session_id
            && let Some(sess) = self.sessions.lock().unwrap().get_mut(&id)
        {
            sess.server = server;
        }
        r
    }
}

/// Binds the endpoint's address
pub fn listen(addr: &str) -> Result<(TcpListener, SocketAddr), String> {
    let l = TcpListener::bind(addr).map_err(|e| format!("{addr}: {e}"))?;
    let a = l.local_addr().map_err(|e| e.to_string())?;
    Ok((l, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposures() {
        let e = |s: &str| exposure(s.parse().unwrap());
        assert_eq!(e("127.0.0.1"), Exposure::Loopback);
        assert_eq!(e("::1"), Exposure::Loopback);
        assert_eq!(e("10.88.0.1"), Exposure::Containers);
        assert_eq!(e("172.17.0.1"), Exposure::Containers);
        assert_eq!(e("169.254.1.2"), Exposure::Containers);
        assert_eq!(e("0.0.0.0"), Exposure::Open);
        assert_eq!(e("192.168.1.5"), Exposure::Open);
        assert_eq!(e("::"), Exposure::Open);
    }

    #[test]
    fn origins() {
        assert!(local_origin("http://localhost:3000"));
        assert!(local_origin("http://127.0.0.1"));
        assert!(local_origin("https://[::1]:8443"));
        assert!(!local_origin("https://evil.example"));
        assert!(!local_origin("http://localhost.evil.example"));
        assert!(!local_origin("null"));
    }

    #[test]
    fn requests() {
        let raw = "POST /mcp?x=1 HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\nGET /mcp HTTP/1.0\r\nContent-Length: 2\r\n\r\nhi";
        let mut r = BufReader::new(raw.as_bytes());
        let a = read_request(&mut r).unwrap().unwrap();
        assert_eq!((a.method.as_str(), a.path.as_str()), ("POST", "/mcp"));
        assert_eq!(a.body, b"abcde");
        assert!(!a.close);
        let b = read_request(&mut r).unwrap().unwrap();
        assert_eq!(b.body, b"hi");
        assert!(b.close);
        assert!(read_request(&mut r).unwrap().is_none());
        let big = format!(
            "POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_LINE + 1
        );
        let e = read_request(&mut BufReader::new(big.as_bytes()))
            .err()
            .unwrap();
        assert_eq!(e.to_string(), "413");
    }
}
