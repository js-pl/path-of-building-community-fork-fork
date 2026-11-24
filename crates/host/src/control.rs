//! The control socket: the user's own processes (`pob ctl`, later `pob
//! mcp`) call the agent API on it, over [`crate::local`]: a Unix socket
//! (a named pipe on Windows) only the user can open;
//! one JSON line per request and per answer.
//!
//! A connection's first line is its hello, which fixes its grant:
//!
//! ```text
//! → {"hello": {"harness": "ctl", "builds": ["RF"], "agent": "ctl-2026-11-23-101500"}}
//! ← {"ok": {"connection": "ctl", "agent_directory": "...", "builds": ["RF"]}}
//! → {"verb": "summary"}
//! ← {"ok": {...}}
//! ```
//!
//! `agent` reuses an agent directory; without it the connection gets a new
//! one, `<harness>-<time>`. The socket is the user's (only they can open
//! it), so a hello may name any of the user's builds; no request after it
//! changes the grant.

use crate::connections::{Closer, Registry};
use agent::{Agent, Grant, Store, agent_dir_name};
use serde_json::{Value as J, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc;

/// A line's cap: a request's, and some room
pub const MAX_LINE: usize = agent::MAX_REQUEST + 1024;

/// A grant from a hello line
pub fn grant_of(hello: &J, now: i64) -> Result<Grant, String> {
    let h = hello
        .get("hello")
        .ok_or("the first line is the hello: {\"hello\": {...}}")?;
    let harness = h["harness"].as_str().unwrap_or("agent");
    let dir = match h["agent"].as_str() {
        Some(d) => {
            agent::grant::check_name(d)?;
            d.to_string()
        }
        None => agent_dir_name(harness, now),
    };
    let connection: String = harness
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    let mut g = Grant::new(
        if connection.is_empty() {
            "agent".into()
        } else {
            connection
        },
        dir,
    );
    for b in h["builds"].as_array().into_iter().flatten() {
        let b = b.as_str().ok_or("builds is a list of build names")?;
        agent::grant::check_name(b)?;
        if crate::store::reserved(b) {
            return Err(format!("{b} is not a build"));
        }
        g = g.with_build(b);
    }
    Ok(g)
}

/// A request from a connection, with where its answer goes
pub struct Call {
    pub grant: Grant,
    pub line: String,
    pub reply: mpsc::Sender<String>,
}

/// Reads one line of at most [`MAX_LINE`] bytes; `None` at the end
pub fn read_line(r: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = r.take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() > MAX_LINE {
        return Err(std::io::Error::other("a line over the size limit"));
    }
    Ok(Some(String::from_utf8_lossy(&buf).trim_end().to_string()))
}

/// Serves one connection: its hello, then its requests through `calls`
pub fn connection(
    stream: impl Read + Write,
    calls: mpsc::Sender<Call>,
    now: i64,
) -> std::io::Result<()> {
    connection_in(stream, calls, now, None)
}

/// An answer to an `admin` hello, which only the host's own registry
/// answers: `"connections"`, `{"disconnect": <id>}` or `{"revoked": <token
/// id>}` (closes the connections made with a token just revoked)
fn admin(req: &J, registry: &Registry) -> J {
    if req == "connections" {
        return json!({ "ok": { "connections": registry.list() } });
    }
    if let Some(id) = req.get("disconnect") {
        return match id.as_u64() {
            Some(id) => json!({ "ok": { "closed": registry.disconnect(id) } }),
            None => json!({ "error": "disconnect takes a connection's id" }),
        };
    }
    if let Some(t) = req.get("revoked").and_then(J::as_str) {
        return json!({ "ok": { "closed": registry.disconnect_token(t) } });
    }
    json!({ "error": "admin is \"connections\", {\"disconnect\": <id>} or {\"revoked\": <token id>}" })
}

/// Serves one connection, listed in `registry` (with how to close it)
/// while it lasts
pub fn connection_in(
    stream: impl Read + Write,
    calls: mpsc::Sender<Call>,
    now: i64,
    registry: Option<(&Registry, Closer)>,
) -> std::io::Result<()> {
    let mut stream = BufReader::new(stream);
    let Some(hello) = read_line(&mut stream)? else {
        return Ok(());
    };
    let hello = serde_json::from_str::<J>(&hello).map_err(|e| format!("not JSON: {e}"));
    if let (Ok(h), Some((reg, _))) = (&hello, &registry)
        && let Some(req) = h.get("admin")
    {
        writeln!(stream.get_mut(), "{}", admin(req, reg))?;
        return Ok(());
    }
    let grant = match hello.and_then(|h| grant_of(&h, now)) {
        Ok(g) => g,
        Err(e) => {
            writeln!(stream.get_mut(), "{}", json!({ "error": e }))?;
            return Ok(());
        }
    };
    writeln!(
        stream.get_mut(),
        "{}",
        json!({ "ok": {
            "connection": grant.connection,
            "agent_directory": grant.dir,
            "builds": grant.builds,
        } })
    )?;
    let listed = registry.map(|(reg, close)| (reg, reg.add("socket", &grant, None, close)));
    let r = serve_lines(&mut stream, &grant, calls, listed);
    if let Some((reg, id)) = listed {
        reg.remove(id);
    }
    r
}

fn serve_lines(
    stream: &mut BufReader<impl Read + Write>,
    grant: &Grant,
    calls: mpsc::Sender<Call>,
    listed: Option<(&Registry, u64)>,
) -> std::io::Result<()> {
    while let Some(line) = read_line(stream)? {
        if line.trim().is_empty() {
            continue;
        }
        if let Some((reg, id)) = listed {
            reg.touch(id);
        }
        let (tx, rx) = mpsc::channel();
        let call = Call {
            grant: grant.clone(),
            line,
            reply: tx,
        };
        if calls.send(call).is_err() {
            break;
        }
        let Ok(answer) = rx.recv() else { break };
        writeln!(stream.get_mut(), "{answer}")?;
        stream.get_mut().flush()?;
    }
    Ok(())
}

/// Answers the connections' calls, one at a time, until every sender is
/// gone
pub fn answer<S: Store>(agent: &mut Agent<S>, calls: mpsc::Receiver<Call>) {
    for c in calls {
        let answer = agent.call_str(&c.grant, &c.line);
        let _ = c.reply.send(answer);
    }
}

/// Seconds since the Unix epoch
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hellos() {
        let g = grant_of(
            &json!({ "hello": { "harness": "ctl", "builds": ["RF"] } }),
            0,
        )
        .unwrap();
        assert_eq!(g.connection, "ctl");
        assert_eq!(g.dir, "ctl-1970-01-01-000000");
        assert_eq!(g.builds, ["RF"]);
        let g = grant_of(&json!({ "hello": { "agent": "mine" } }), 0).unwrap();
        assert_eq!(g.dir, "mine");
        assert!(grant_of(&json!({ "hello": { "agent": "../x" } }), 0).is_err());
        assert!(grant_of(&json!({ "hello": { "builds": ["a/b"] } }), 0).is_err());
        assert!(grant_of(&json!({ "hello": { "builds": ["agents"] } }), 0).is_err());
        assert!(grant_of(&json!({ "verb": "summary" }), 0).is_err());
    }
}
