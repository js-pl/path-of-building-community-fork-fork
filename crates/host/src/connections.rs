//! The host's open connections, control socket and HTTP alike, so the
//! user can list them and close one (`pob agents connections`,
//! `pob agents disconnect`).

use agent::Grant;
use serde_json::{Value as J, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// How a connection is closed from outside it
pub type Closer = Box<dyn Fn() + Send>;

struct Entry {
    kind: &'static str,
    grant: Grant,
    token: Option<String>,
    client: Option<String>,
    since: i64,
    calls: u64,
    close: Closer,
}

/// The open connections
#[derive(Default)]
pub struct Registry {
    inner: Mutex<(u64, BTreeMap<u64, Entry>)>,
}

impl Registry {
    /// Adds a connection: `kind` is `socket` or `http`, `token` the id of
    /// the token it came with; its id
    pub fn add(
        &self,
        kind: &'static str,
        grant: &Grant,
        token: Option<&str>,
        close: Closer,
    ) -> u64 {
        let mut g = self.inner.lock().unwrap();
        g.0 += 1;
        let id = g.0;
        g.1.insert(
            id,
            Entry {
                kind,
                grant: grant.clone(),
                token: token.map(String::from),
                client: None,
                since: crate::control::now(),
                calls: 0,
                close,
            },
        );
        id
    }

    /// Notes a call on a connection
    pub fn touch(&self, id: u64) {
        if let Some(e) = self.inner.lock().unwrap().1.get_mut(&id) {
            e.calls += 1;
        }
    }

    /// Names the MCP client on a connection
    pub fn set_client(&self, id: u64, client: &str) {
        if let Some(e) = self.inner.lock().unwrap().1.get_mut(&id) {
            e.client = Some(client.chars().take(64).collect());
        }
    }

    /// Forgets a connection that ended
    pub fn remove(&self, id: u64) {
        self.inner.lock().unwrap().1.remove(&id);
    }

    /// Closes a connection; whether there was one
    pub fn disconnect(&self, id: u64) -> bool {
        let e = self.inner.lock().unwrap().1.remove(&id);
        match e {
            Some(e) => {
                (e.close)();
                true
            }
            None => false,
        }
    }

    /// Closes the connections made with a token; how many
    pub fn disconnect_token(&self, token: &str) -> usize {
        let gone: Vec<Entry> = {
            let mut g = self.inner.lock().unwrap();
            let ids: Vec<u64> =
                g.1.iter()
                    .filter(|(_, e)| e.token.as_deref() == Some(token))
                    .map(|(id, _)| *id)
                    .collect();
            ids.iter().filter_map(|id| g.1.remove(id)).collect()
        };
        for e in &gone {
            (e.close)();
        }
        gone.len()
    }

    /// The connections, oldest first
    pub fn list(&self) -> J {
        let g = self.inner.lock().unwrap();
        J::Array(
            g.1.iter()
                .map(|(id, e)| {
                    json!({
                        "id": id,
                        "kind": e.kind,
                        "connection": e.grant.connection,
                        "client": e.client,
                        "agent_directory": e.grant.dir,
                        "builds": e.grant.builds,
                        "token": e.token,
                        "since": e.since,
                        "calls": e.calls,
                    })
                })
                .collect(),
        )
    }
}
