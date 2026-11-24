//! `pob-proxy`: the default agent container's only way out (docs/agent.md,
//! "The default container"). It runs in a container of its own, on the
//! agent's internal network and on one with a route out, and does two
//! things:
//!
//! - **An HTTPS proxy** (`--listen`): `CONNECT host:port` to the hosts of
//!   an allowlist, nothing else. Plain HTTP requests, other hosts and ports,
//!   and names that resolve only to private, loopback or link-local
//!   addresses are refused with 403. The harness finds it through
//!   `HTTPS_PROXY`.
//! - **Routes** (`--route <listen>=<upstream>`): plain TCP forwarded to one
//!   upstream, `host:port` or `unix:<path>`; that's how the agent reaches
//!   the host's MCP endpoint, which checks its token.
//!
//! The allowlist (`--allow <file>`, `--allow-host <entry>`): one entry per
//! line, `host`, `host:port` (443 when left out) or `*.domain` (its
//! subdomains, not the domain itself); `#` starts a comment.
//!
//! Std only, a thread per connection, so the source builds anywhere in
//! the image's build stage (`pob agent` ships it in its build context).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A request head's cap
const MAX_HEAD: usize = 16 * 1024;
/// Connections at a time, all listeners together
const MAX_CONNECTIONS: usize = 256;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);

/// One allowlist entry
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Lowercase; for a wildcard, the domain after `*.`
    pub host: String,
    pub wildcard: bool,
    pub port: u16,
}

/// The hosts the proxy lets through
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allowlist {
    pub entries: Vec<Entry>,
}

/// Whether `h` is a host name: letters, digits, `-` and `.`, with labels
/// of 1 to 63 characters
fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

impl Entry {
    pub fn parse(s: &str) -> Result<Entry, String> {
        let s = s.trim().to_ascii_lowercase();
        let (host, port) = match s.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or(format!("{s}: not a port"))?,
            ),
            None => (s.clone(), 443),
        };
        let (host, wildcard) = match host.strip_prefix("*.") {
            Some(h) => (h.to_string(), true),
            None => (host, false),
        };
        // A wildcard needs a domain of two labels at least: no `*.com`
        if !valid_host(&host) || (wildcard && !host.contains('.')) {
            return Err(format!("{s}: not a host name"));
        }
        Ok(Entry {
            host,
            wildcard,
            port,
        })
    }

    fn allows(&self, host: &str, port: u16) -> bool {
        port == self.port
            && if self.wildcard {
                host.strip_suffix(&self.host)
                    .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.'))
            } else {
                host == self.host
            }
    }
}

impl Allowlist {
    /// The entries of a file's text; the line of a bad entry in its error
    pub fn parse(text: &str) -> Result<Allowlist, String> {
        let mut entries = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            entries.push(Entry::parse(line).map_err(|e| format!("line {}: {e}", i + 1))?);
        }
        Ok(Allowlist { entries })
    }

    pub fn allows(&self, host: &str, port: u16) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        valid_host(&host) && self.entries.iter().any(|e| e.allows(&host, port))
    }
}

/// Where a route goes
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Upstream {
    Tcp(String),
    Unix(String),
}

/// `--route <listen>=<upstream>`
pub fn parse_route(s: &str) -> Result<(String, Upstream), String> {
    let (listen, up) = s
        .split_once('=')
        .ok_or(format!("{s}: a route is <listen addr>=<upstream>"))?;
    listen
        .parse::<SocketAddr>()
        .map_err(|_| format!("{listen}: not an address to listen on"))?;
    let up = match up.strip_prefix("unix:") {
        Some(p) if !p.is_empty() => Upstream::Unix(p.to_string()),
        Some(_) => return Err(format!("{s}: unix: needs a path")),
        None => {
            let (h, p) = up
                .rsplit_once(':')
                .ok_or(format!("{up}: the upstream is host:port or unix:<path>"))?;
            if h.is_empty() || p.parse::<u16>().is_err() {
                return Err(format!("{up}: the upstream is host:port or unix:<path>"));
            }
            Upstream::Tcp(up.to_string())
        }
    };
    Ok((listen.to_string(), up))
}

/// Whether an address is one the internet can't route to: what an allowed
/// name must not resolve to only (no way into the host's or the
/// container network's services through the proxy)
pub fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
                || o[0] == 0
                // Carrier-grade NAT, 100.64.0.0/10
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_internal(IpAddr::V4(v4));
            }
            a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                // Unique local fc00::/7, link-local fe80::/10
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

/// A request's head: its first line and the rest up to the empty line
fn read_head(r: &mut impl BufRead) -> Result<String, String> {
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = r
            .take((MAX_HEAD - head.len()) as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed before the request's end".into());
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEAD {
            return Err("a request head over the size limit".into());
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&head).to_string())
}

/// The target of a `CONNECT host:port HTTP/1.x` line
pub fn connect_target(line: &str) -> Result<(String, u16), String> {
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err("not an HTTP request".into());
    };
    if !version.starts_with("HTTP/1.") {
        return Err("not an HTTP request".into());
    }
    if method != "CONNECT" {
        return Err(format!("{method}: only CONNECT (HTTPS) is proxied"));
    }
    let (host, port) = target
        .rsplit_once(':')
        .ok_or(format!("{target}: CONNECT takes host:port"))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("{target}: CONNECT takes host:port"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Ok((host.to_ascii_lowercase(), port))
}

/// Copies both ways until either side is done
fn splice(
    a: TcpStream,
    b: impl Read + Write + Send + 'static,
    b2: impl Read + Write + Send + 'static,
) {
    let mut a2 = match a.try_clone() {
        Ok(a2) => a2,
        Err(_) => return,
    };
    let mut a = a;
    let mut b = b;
    let mut b2 = b2;
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut a2, &mut b2);
        let _ = b2.flush();
        drop(b2);
    });
    let _ = std::io::copy(&mut b, &mut a);
    let _ = a.shutdown(Shutdown::Both);
    let _ = t.join();
}

fn deny(mut c: &TcpStream, why: &str) {
    eprintln!("pob-proxy: refused: {why}");
    let body = format!("pob-proxy: {why}\n");
    let _ = write!(
        c,
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// One proxy connection: its CONNECT, checked, then the bytes
fn proxy(c: TcpStream, allow: &Allowlist) {
    let _ = c.set_read_timeout(Some(HEAD_TIMEOUT));
    let Ok(rc) = c.try_clone() else { return };
    let mut r = BufReader::new(rc);
    let head = match read_head(&mut r) {
        Ok(h) => h,
        Err(e) => return deny(&c, &e),
    };
    let first = head.lines().next().unwrap_or_default();
    let (host, port) = match connect_target(first) {
        Ok(t) => t,
        Err(e) => return deny(&c, &e),
    };
    if !allow.allows(&host, port) {
        return deny(&c, &format!("{host}:{port} is not on the allowlist"));
    }
    let addrs: Vec<SocketAddr> = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.filter(|a| !is_internal(a.ip())).collect(),
        Err(e) => return deny(&c, &format!("{host}: {e}")),
    };
    let Some(up) = addrs
        .iter()
        .find_map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).ok())
    else {
        return deny(&c, &format!("{host}:{port}: no public address answers"));
    };
    // Bytes the client sent after its head (TLS's hello) go first
    let early = r.buffer().to_vec();
    let mut cw = &c;
    if cw
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .is_err()
    {
        return;
    }
    let _ = c.set_read_timeout(None);
    let mut upw = &up;
    if !early.is_empty() && upw.write_all(&early).is_err() {
        return;
    }
    let Ok(up2) = up.try_clone() else { return };
    splice(c, up, up2);
}

/// One route connection: the bytes, both ways, to the upstream
fn route(c: TcpStream, up: &Upstream) {
    match up {
        Upstream::Tcp(a) => {
            let Some(s) = a
                .to_socket_addrs()
                .ok()
                .into_iter()
                .flatten()
                .find_map(|a| TcpStream::connect_timeout(&a, CONNECT_TIMEOUT).ok())
            else {
                eprintln!("pob-proxy: route: {a} doesn't answer");
                return;
            };
            let Ok(s2) = s.try_clone() else { return };
            splice(c, s, s2);
        }
        #[cfg(unix)]
        Upstream::Unix(p) => {
            let s = match std::os::unix::net::UnixStream::connect(p) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("pob-proxy: route: {p}: {e}");
                    return;
                }
            };
            let Ok(s2) = s.try_clone() else { return };
            splice(c, s, s2);
        }
        #[cfg(not(unix))]
        Upstream::Unix(p) => eprintln!("pob-proxy: route: {p}: no Unix sockets here"),
    }
}

/// Accepts on `l`, each connection on a thread of its own, within the cap
fn serve(l: TcpListener, active: Arc<AtomicUsize>, f: Arc<dyn Fn(TcpStream) + Send + Sync>) {
    for c in l.incoming() {
        let Ok(c) = c else { continue };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            eprintln!("pob-proxy: refused: too many connections");
            continue;
        }
        let (active, f) = (active.clone(), f.clone());
        std::thread::spawn(move || {
            f(c);
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("pob-proxy: {msg}");
    std::process::exit(2)
}

fn main() {
    let usage = "usage: pob-proxy [--listen <addr>] [--allow <file>]... [--allow-host <entry>]... \
                 [--route <listen addr>=<host:port | unix:path>]...";
    let mut listen = None;
    let mut allow = Allowlist::default();
    let mut routes = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| die(usage));
        match a.as_str() {
            "--listen" => listen = Some(value()),
            "--allow" => {
                let f = value();
                let text = std::fs::read_to_string(&f).unwrap_or_else(|e| die(format!("{f}: {e}")));
                let list = Allowlist::parse(&text).unwrap_or_else(|e| die(format!("{f}: {e}")));
                allow.entries.extend(list.entries);
            }
            "--allow-host" => allow
                .entries
                .push(Entry::parse(&value()).unwrap_or_else(|e| die(e))),
            "--route" => routes.push(parse_route(&value()).unwrap_or_else(|e| die(e))),
            _ => die(usage),
        }
    }
    if listen.is_none() && routes.is_empty() {
        die(usage);
    }
    let active = Arc::new(AtomicUsize::new(0));
    let mut threads = Vec::new();
    for (at, up) in routes {
        let l = TcpListener::bind(&at).unwrap_or_else(|e| die(format!("{at}: {e}")));
        eprintln!("pob-proxy: route {at} → {up:?}");
        let f: Arc<dyn Fn(TcpStream) + Send + Sync> = Arc::new(move |c| route(c, &up));
        let active = active.clone();
        threads.push(std::thread::spawn(move || serve(l, active, f)));
    }
    if let Some(at) = listen {
        let l = TcpListener::bind(&at).unwrap_or_else(|e| die(format!("{at}: {e}")));
        let hosts: Vec<String> = allow
            .entries
            .iter()
            .map(|e| {
                format!(
                    "{}{}:{}",
                    if e.wildcard { "*." } else { "" },
                    e.host,
                    e.port
                )
            })
            .collect();
        eprintln!("pob-proxy: HTTPS proxy on {at} for {}", hosts.join(", "));
        let allow = Arc::new(allow);
        let f: Arc<dyn Fn(TcpStream) + Send + Sync> = Arc::new(move |c| proxy(c, &allow));
        let active = active.clone();
        threads.push(std::thread::spawn(move || serve(l, active, f)));
    }
    for t in threads {
        let _ = t.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlists() {
        let a = Allowlist::parse(
            "# Claude Code\napi.anthropic.com\n\n*.example.org   # its subdomains\nLOGIN.Example.com:8443\n",
        )
        .unwrap();
        assert_eq!(a.entries.len(), 3);
        assert!(a.allows("api.anthropic.com", 443));
        assert!(a.allows("API.anthropic.com.", 443));
        assert!(!a.allows("api.anthropic.com", 80));
        assert!(!a.allows("anthropic.com", 443));
        assert!(!a.allows("evil-api.anthropic.com", 443));
        assert!(!a.allows("api.anthropic.com.evil.net", 443));
        assert!(a.allows("a.example.org", 443));
        assert!(a.allows("a.b.example.org", 443));
        assert!(!a.allows("example.org", 443));
        assert!(!a.allows("badexample.org", 443));
        assert!(a.allows("login.example.com", 8443));
        assert!(!a.allows("login.example.com", 443));
        assert!(!a.allows("", 443));
        assert!(!a.allows("a b", 443));

        for bad in [
            "*.com",
            "*",
            "a..b",
            "-a.com",
            "a.com:0",
            "a.com:x",
            "a.com:70000",
            "http://a.com",
            "a.com/x",
            "a_b.com",
        ] {
            assert!(Entry::parse(bad).is_err(), "{bad}");
        }
        let e = Allowlist::parse("ok.com\nnot a host\n").unwrap_err();
        assert!(e.starts_with("line 2:"), "{e}");
    }

    #[test]
    fn the_shipped_lists_parse() {
        for f in ["claude.txt", "codex.txt"] {
            let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../container/allow/");
            let text = std::fs::read_to_string(format!("{p}{f}")).unwrap();
            let a = Allowlist::parse(&text).unwrap();
            assert!(!a.entries.is_empty(), "{f}");
            // PyPI for the agent's Python, by default (decided); npm isn't
            assert!(a.allows("pypi.org", 443), "{f}");
            assert!(a.allows("files.pythonhosted.org", 443), "{f}");
            assert!(!a.allows("registry.npmjs.org", 443), "{f}");
        }
    }

    #[test]
    fn connect_lines() {
        assert_eq!(
            connect_target("CONNECT api.anthropic.com:443 HTTP/1.1"),
            Ok(("api.anthropic.com".into(), 443))
        );
        assert_eq!(
            connect_target("CONNECT [::1]:443 HTTP/1.1"),
            Ok(("::1".into(), 443))
        );
        assert!(connect_target("GET http://a.com/ HTTP/1.1").is_err());
        assert!(connect_target("CONNECT a.com HTTP/1.1").is_err());
        assert!(connect_target("CONNECT a.com:443").is_err());
        assert!(connect_target("CONNECT a.com:443 SSH-2.0").is_err());
    }

    #[test]
    fn routes() {
        assert_eq!(
            parse_route("0.0.0.0:8731=unix:/run/pob/mcp.sock"),
            Ok((
                "0.0.0.0:8731".into(),
                Upstream::Unix("/run/pob/mcp.sock".into())
            ))
        );
        assert_eq!(
            parse_route("0.0.0.0:8731=host.containers.internal:8731"),
            Ok((
                "0.0.0.0:8731".into(),
                Upstream::Tcp("host.containers.internal:8731".into())
            ))
        );
        for bad in [
            "0.0.0.0:8731",
            "x=a:1",
            "0.0.0.0:1=a",
            "0.0.0.0:1=unix:",
            "0.0.0.0:1=:5",
        ] {
            assert!(parse_route(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn internal_addresses() {
        for a in [
            "127.0.0.1",
            "10.88.0.1",
            "172.17.0.1",
            "192.168.1.2",
            "169.254.1.2",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_internal(a.parse().unwrap()), "{a}");
        }
        for a in ["1.1.1.1", "160.79.104.10", "2606:4700::1111"] {
            assert!(!is_internal(a.parse().unwrap()), "{a}");
        }
    }

    /// The proxy on loopback: refusals answer 403, a route forwards
    #[test]
    fn proxy_and_route() {
        let allow = Arc::new(Allowlist::parse("api.anthropic.com").unwrap());
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = l.local_addr().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let a = allow.clone();
        let f: Arc<dyn Fn(TcpStream) + Send + Sync> = Arc::new(move |c| proxy(c, &a));
        std::thread::spawn({
            let active = active.clone();
            move || serve(l, active, f)
        });
        let ask = |req: &str| {
            let mut c = TcpStream::connect(at).unwrap();
            c.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            out
        };
        assert!(ask("CONNECT example.com:443 HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403"));
        assert!(ask("GET http://api.anthropic.com/ HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403"));
        assert!(ask("CONNECT api.anthropic.com:80 HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403"));
        let huge = format!(
            "CONNECT a:1 HTTP/1.1\r\nX: {}\r\n\r\n",
            "a".repeat(MAX_HEAD)
        );
        assert!(ask(&huge).starts_with("HTTP/1.1 403"));

        // A route to an echo server
        let echo = TcpListener::bind("127.0.0.1:0").unwrap();
        let echo_at = echo.local_addr().unwrap();
        std::thread::spawn(move || {
            for c in echo.incoming() {
                let mut c = c.unwrap();
                let mut buf = [0; 5];
                c.read_exact(&mut buf).unwrap();
                c.write_all(&buf).unwrap();
            }
        });
        let rl = TcpListener::bind("127.0.0.1:0").unwrap();
        let rat = rl.local_addr().unwrap();
        let up = Upstream::Tcp(echo_at.to_string());
        let f: Arc<dyn Fn(TcpStream) + Send + Sync> = Arc::new(move |c| route(c, &up));
        std::thread::spawn(move || serve(rl, active, f));
        let mut c = TcpStream::connect(rat).unwrap();
        c.write_all(b"hello").unwrap();
        let mut buf = [0; 5];
        c.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
    }
}
