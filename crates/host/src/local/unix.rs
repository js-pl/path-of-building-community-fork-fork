//! The control socket on Unix: a socket file in a directory only the
//! user can enter, itself only the user's

use crate::connections::{Closer, Registry};
use crate::control::{Call, connection_in, now, read_line};
use serde_json::{Value as J, json};
use std::io::{BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

/// A connection's stream
pub type Stream = UnixStream;

/// The socket the host listens on
pub struct Listener(UnixListener);

impl Listener {
    /// Waits for the next client
    pub fn accept(&mut self) -> std::io::Result<Stream> {
        self.0.accept().map(|(s, _)| s)
    }
}

/// Listens at `path`. A socket left by a host that is gone is
/// replaced; one a running host answers on is an error.
pub fn listen(path: &Path) -> Result<Listener, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(format!("a host is running already on {}", path.display()));
        }
        std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let l = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Listener(l))
}

/// Accepts connections on a thread of their own each, passing their
/// calls to `calls` and listing them in `registry`
pub fn serve(
    listener: Listener,
    calls: std::sync::mpsc::Sender<Call>,
    registry: std::sync::Arc<Registry>,
) {
    for stream in listener.0.incoming() {
        let Ok(stream) = stream else { continue };
        let calls = calls.clone();
        let registry = registry.clone();
        std::thread::spawn(move || {
            let close: Closer = match stream.try_clone() {
                Ok(s) => Box::new(move || {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }),
                Err(_) => Box::new(|| {}),
            };
            let _ = connection_in(stream, calls, now(), Some((&registry, close)));
        });
    }
}

/// Whether a host answers on `path`
pub fn answers(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

/// An `admin` request to the host on `path`
pub fn admin(path: &Path, req: J) -> Result<J, String> {
    let s = UnixStream::connect(path).map_err(|e| format!("no host on {}: {e}", path.display()))?;
    let mut w = s.try_clone().map_err(|e| e.to_string())?;
    writeln!(w, "{}", json!({ "admin": req })).map_err(|e| e.to_string())?;
    let line = read_line(&mut BufReader::new(s))
        .map_err(|e| e.to_string())?
        .ok_or("the host closed the connection")?;
    let v: J = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    match v.get("error") {
        Some(e) => Err(e.as_str().unwrap_or_default().to_string()),
        None => Ok(v["ok"].clone()),
    }
}

/// A client of the control socket
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    /// The hello's answer
    pub hello: J,
}

impl Client {
    /// Connects and says hello
    pub fn connect(path: &Path, hello: &J) -> Result<Client, String> {
        let s = UnixStream::connect(path).map_err(|e| {
            format!(
                "no host on {}: {e} (start one with `pob host --headless`)",
                path.display()
            )
        })?;
        let writer = s.try_clone().map_err(|e| e.to_string())?;
        let mut c = Client {
            reader: BufReader::new(s),
            writer,
            hello: J::Null,
        };
        let h = c.call(hello)?;
        if let Some(e) = h.get("error") {
            return Err(e.as_str().unwrap_or_default().to_string());
        }
        c.hello = h["ok"].clone();
        Ok(c)
    }

    /// One request's answer, `{"ok": ...}` or `{"error": ...}`
    pub fn call(&mut self, req: &J) -> Result<J, String> {
        writeln!(self.writer, "{req}").map_err(|e| e.to_string())?;
        let line = read_line(&mut self.reader)
            .map_err(|e| e.to_string())?
            .ok_or("the host closed the connection")?;
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }
}
