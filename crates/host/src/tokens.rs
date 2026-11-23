//! Tokens for MCP over HTTP: each names the builds it may read and edit,
//! its agent directory and the connection's name. `pob agents token`
//! mints one; the host checks every HTTP request against the file, so a
//! token revoked is refused from the next request on.
//!
//! The file (`tokens.json` in the data directory, only the user's) keeps
//! each token's SHA-256, never the token: a token is shown once, when it
//! is minted.
//!
//! ```json
//! {"tokens": [{"id": "3f9a01c2", "sha256": "...", "connection": "http",
//!   "agent": "http-2026-11-23-101500", "builds": ["RF"], "created": 1795430100}]}
//! ```

use agent::Grant;
use serde_json::{Value as J, json};
use std::path::{Path, PathBuf};

/// The variable that names another tokens file
pub const TOKENS_ENV: &str = "POB_TOKENS";
/// A token's prefix, so it is recognised (and found by secret scanners)
pub const PREFIX: &str = "pob_";

/// The tokens file: `POB_TOKENS`, else `tokens.json` in the data directory
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os(TOKENS_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::data_dir().map(|d| d.join("tokens.json")))
}

/// A token as the file keeps it
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// Short and public: how the user names it to revoke it
    pub id: String,
    sha256: String,
    pub connection: String,
    pub agent: String,
    pub builds: Vec<String>,
    pub created: i64,
}

impl Token {
    /// The grant the token gives
    pub fn grant(&self) -> Grant {
        let mut g = Grant::new(self.connection.clone(), self.agent.clone());
        for b in &self.builds {
            g = g.with_build(b.clone());
        }
        g
    }

    pub fn to_json(&self) -> J {
        json!({
            "id": self.id,
            "connection": self.connection,
            "agent_directory": self.agent,
            "builds": self.builds,
            "created": self.created,
        })
    }

    fn from_json(v: &J) -> Option<Token> {
        Some(Token {
            id: v["id"].as_str()?.into(),
            sha256: v["sha256"].as_str()?.into(),
            connection: v["connection"].as_str()?.into(),
            agent: v["agent"].as_str()?.into(),
            builds: v["builds"]
                .as_array()?
                .iter()
                .map(|b| b.as_str().map(String::from))
                .collect::<Option<_>>()?,
            created: v["created"].as_i64().unwrap_or(0),
        })
    }

    fn file_json(&self) -> J {
        json!({
            "id": self.id,
            "sha256": self.sha256,
            "connection": self.connection,
            "agent": self.agent,
            "builds": self.builds,
            "created": self.created,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(s: &str) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, s.as_bytes()).as_ref())
}

/// `n` random bytes from the system
pub fn random(n: usize) -> Result<Vec<u8>, String> {
    use ring::rand::SecureRandom;
    let mut buf = vec![0; n];
    ring::rand::SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| "no randomness from the system".to_string())?;
    Ok(buf)
}

/// The tokens file
pub struct Tokens {
    path: PathBuf,
}

impl Tokens {
    pub fn new(path: impl Into<PathBuf>) -> Tokens {
        Tokens { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The tokens; none when there is no file
    pub fn list(&self) -> Result<Vec<Token>, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let v: J = serde_json::from_str(&text)
            .map_err(|e| format!("{}: not JSON: {e}", self.path.display()))?;
        Ok(v["tokens"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Token::from_json)
            .collect())
    }

    /// The token a request presents, if the file has it
    pub fn find(&self, presented: &str) -> Result<Option<Token>, String> {
        if !presented.starts_with(PREFIX) {
            return Ok(None);
        }
        let h = sha256(presented);
        Ok(self.list()?.into_iter().find(|t| t.sha256 == h))
    }

    /// Mints a token for `grant`: the token, shown once, and its entry
    pub fn mint(&self, grant: &Grant, now: i64) -> Result<(String, Token), String> {
        agent::grant::check_name(&grant.dir)?;
        for b in &grant.builds {
            agent::grant::check_name(b)?;
        }
        let token = format!("{PREFIX}{}", hex(&random(32)?));
        let h = sha256(&token);
        let entry = Token {
            id: h[..8].to_string(),
            sha256: h,
            connection: grant.connection.clone(),
            agent: grant.dir.clone(),
            builds: grant.builds.clone(),
            created: now,
        };
        let mut all = self.list()?;
        all.push(entry.clone());
        self.save(&all)?;
        Ok((token, entry))
    }

    /// Revokes a token by its id; whether there was one
    pub fn revoke(&self, id: &str) -> Result<bool, String> {
        let mut all = self.list()?;
        let n = all.len();
        all.retain(|t| t.id != id);
        if all.len() == n {
            return Ok(false);
        }
        self.save(&all)?;
        Ok(true)
    }

    /// Writes the file whole, only the user's, then moves it in place
    fn save(&self, all: &[Token]) -> Result<(), String> {
        let err = |e: std::io::Error| format!("{}: {e}", self.path.display());
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(
            &json!({ "tokens": all.iter().map(Token::file_json).collect::<Vec<_>>() }),
        )
        .map_err(|e| e.to_string())?;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        {
            use std::io::Write;
            let mut f = o.open(&tmp).map_err(err)?;
            f.write_all(text.as_bytes()).map_err(err)?;
            f.sync_all().map_err(err)?;
        }
        std::fs::rename(&tmp, &self.path).map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_find_revoke() {
        let dir = std::env::temp_dir().join(format!("pob-tokens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let t = Tokens::new(dir.join("tokens.json"));
        assert!(t.list().unwrap().is_empty());
        let g = Grant::new("http", "http-1").with_build("RF");
        let (token, entry) = t.mint(&g, 5).unwrap();
        assert!(token.starts_with(PREFIX));
        let text = std::fs::read_to_string(t.path()).unwrap();
        assert!(!text.contains(&token[PREFIX.len()..]), "kept hashed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(t.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let found = t.find(&token).unwrap().unwrap();
        assert_eq!(found, entry);
        assert_eq!(found.grant(), g);
        assert!(t.find("pob_00").unwrap().is_none());
        assert!(t.find("x").unwrap().is_none());
        let (other, _) = t.mint(&g, 6).unwrap();
        assert!(t.revoke(&entry.id).unwrap());
        assert!(!t.revoke(&entry.id).unwrap());
        assert!(t.find(&token).unwrap().is_none());
        assert!(t.find(&other).unwrap().is_some());
        assert!(t.mint(&Grant::new("h", "../x"), 0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
