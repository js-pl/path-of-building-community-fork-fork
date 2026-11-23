//! Grants: what a connection may read and change. A grant names the
//! user's builds it covers and the connection's agent directory; it is
//! fixed when the connection is made, and no verb widens it.

use crate::store::{BuildId, Owner};

/// How the API names a build in a caller's agent directory: `agent/<name>`
pub const AGENT_PREFIX: &str = "agent/";

/// What one connection may read and change
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// The connection's name, in each commit's `Source: agent <name>`
    pub connection: String,
    /// Its agent directory, where it may make and edit builds freely
    pub dir: String,
    /// The user's builds it may read and edit, by name; the first is the
    /// default build
    pub builds: Vec<String>,
}

impl Grant {
    pub fn new(connection: impl Into<String>, dir: impl Into<String>) -> Grant {
        Grant {
            connection: connection.into(),
            dir: dir.into(),
            builds: Vec::new(),
        }
    }

    pub fn with_build(mut self, name: impl Into<String>) -> Grant {
        self.builds.push(name.into());
        self
    }

    /// The build a request names, within the grant: a user's build the
    /// grant names, or `agent/<name>` in the agent directory. Without a
    /// name, the grant's first build.
    pub fn resolve(&self, name: Option<&str>) -> Result<BuildId, String> {
        let name = match name {
            Some(n) => n,
            None => self
                .builds
                .first()
                .ok_or("no build named, and this connection has no build granted")?,
        };
        if let Some(n) = name.strip_prefix(AGENT_PREFIX) {
            check_name(n)?;
            return Ok(BuildId::agent(&self.dir, n));
        }
        if self.builds.iter().any(|b| b == name) {
            return Ok(BuildId::user(name));
        }
        Err(format!(
            "{name:?} is not a build of this connection's grant; `builds` lists them"
        ))
    }

    /// How the API names a build of this grant
    pub fn api_name(&self, id: &BuildId) -> String {
        match &id.owner {
            Owner::User => id.name.clone(),
            Owner::Agent(_) => format!("{AGENT_PREFIX}{}", id.name),
        }
    }
}

/// A build name the agent chooses: a directory name every platform takes
/// as it is, so it reads back the same
pub fn check_name(name: &str) -> Result<(), String> {
    let bad = name.is_empty()
        || name.chars().count() > 64
        || name.starts_with(['.', ' '])
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c));
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if bad || device {
        return Err(format!(
            "{name:?} can't be a build name: 1 to 64 characters, none of <>:\"/\\|?*, \
             not starting or ending with a dot or space"
        ));
    }
    Ok(())
}

/// An agent directory's name: the harness and the time it connected,
/// `claude-2026-11-23-101500` (UTC)
pub fn agent_dir_name(harness: &str, unix_secs: i64) -> String {
    let harness: String = harness
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    let harness = if harness.is_empty() {
        "agent".to_string()
    } else {
        harness
    };
    let days = unix_secs.div_euclid(86400);
    let secs = unix_secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{harness}-{y:04}-{m:02}-{d:02}-{:02}{:02}{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// The date of a day since 1970-01-01 (Howard Hinnant's algorithm)
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_names() {
        assert_eq!(
            agent_dir_name("claude", 1_795_430_100),
            "claude-2026-11-23-103500"
        );
        assert_eq!(agent_dir_name("../x", 0), "x-1970-01-01-000000");
    }

    #[test]
    fn resolves_within_the_grant() {
        let g = Grant::new("c", "c-dir").with_build("RF");
        assert_eq!(g.resolve(None).unwrap(), BuildId::user("RF"));
        assert_eq!(g.resolve(Some("RF")).unwrap(), BuildId::user("RF"));
        assert_eq!(
            g.resolve(Some("agent/copy")).unwrap(),
            BuildId::agent("c-dir", "copy")
        );
        assert!(g.resolve(Some("Other")).is_err());
        assert!(g.resolve(Some("agent/../RF")).is_err());
        assert!(g.resolve(Some("agent/CON")).is_err());
        assert!(Grant::new("c", "d").resolve(None).is_err());
    }
}
