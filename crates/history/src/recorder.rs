//! When a build's versions are committed: an agent's step at once, the
//! user's clicks coalesced.

use crate::{BuildRepo, Message, Result, Version};
use std::time::{Duration, Instant};

/// How long the user's edits wait without another before they are
/// committed together
pub const QUIET: Duration = Duration::from_secs(10);

/// At most this many of the coalesced edits' labels go in the commit
const MAX_LABELS: usize = 30;

/// Records one open build's history:
///
/// - an agent's step is committed before [`Recorder::agent_step`] returns,
///   so a crash never loses it;
/// - the user's edits are held and committed together once [`QUIET`]
///   passes without another ([`Recorder::tick`]), before any agent step
///   (so the agent's commits stay its own), and on [`Recorder::close`] or
///   drop.
///
/// Made without a repository (no git), it records nothing and every
/// method answers `Ok(None)`, so the caller needs no other path.
#[derive(Debug)]
pub struct Recorder {
    repo: Option<BuildRepo>,
    quiet: Duration,
    pending: Option<Pending>,
}

#[derive(Debug)]
struct Pending {
    xml: String,
    labels: Vec<String>,
    last: Instant,
}

impl Recorder {
    /// Records into `repo`; `None` records nothing
    pub fn new(repo: Option<BuildRepo>) -> Recorder {
        Recorder {
            repo,
            quiet: QUIET,
            pending: None,
        }
    }

    /// Waits `quiet` instead of [`QUIET`] before committing the user's edits
    pub fn with_quiet(mut self, quiet: Duration) -> Recorder {
        self.quiet = quiet;
        self
    }

    /// Whether anything is recorded
    pub fn is_enabled(&self) -> bool {
        self.repo.is_some()
    }

    pub fn repo(&self) -> Option<&BuildRepo> {
        self.repo.as_ref()
    }

    /// The user changed the build to `xml`; `label` is the undo step's.
    /// Runs no git: the edit is committed later, with the ones after it.
    pub fn user_edit(&mut self, xml: impl Into<String>, label: impl Into<String>) {
        if self.repo.is_none() {
            return;
        }
        let label = label.into();
        let p = self.pending.get_or_insert_with(|| Pending {
            xml: String::new(),
            labels: Vec::new(),
            last: Instant::now(),
        });
        p.xml = xml.into();
        if p.labels.last() != Some(&label) {
            p.labels.push(label);
        }
        p.last = Instant::now();
    }

    /// Whether the user's edits wait to be committed
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// When [`Recorder::tick`] commits the edits waiting, if any wait
    pub fn due(&self) -> Option<Instant> {
        self.pending.as_ref().map(|p| p.last + self.quiet)
    }

    /// Commits the user's edits if [`QUIET`] passed since the last one;
    /// call it from a timer or the event loop
    pub fn tick(&mut self) -> Result<Option<Version>> {
        match self.due() {
            Some(due) if Instant::now() >= due => self.flush(),
            _ => Ok(None),
        }
    }

    /// Commits the user's edits waiting, now
    pub fn flush(&mut self) -> Result<Option<Version>> {
        let (Some(repo), Some(p)) = (&self.repo, self.pending.take()) else {
            return Ok(None);
        };
        let mut message = match p.labels.as_slice() {
            [one] => Message::user(one.clone()),
            labels => Message::user(format!("{} edits", labels.len())),
        };
        if p.labels.len() > 1 {
            message.body = p.labels.iter().take(MAX_LABELS).cloned().collect();
            if p.labels.len() > MAX_LABELS {
                message
                    .body
                    .push(format!("and {} more", p.labels.len() - MAX_LABELS));
            }
        }
        match repo.commit(&p.xml, &message) {
            Ok(v) => Ok(v),
            Err(e) => {
                // Kept, so the next flush tries again
                self.pending = Some(p);
                Err(e)
            }
        }
    }

    /// An agent changed the build to `xml`: the user's edits waiting are
    /// committed first, then this step, before it returns
    pub fn agent_step(&mut self, xml: &str, message: &Message) -> Result<Option<Version>> {
        self.flush()?;
        match &self.repo {
            Some(repo) => repo.commit(xml, message),
            None => Ok(None),
        }
    }

    /// Commits whatever waits; the build was closed
    pub fn close(mut self) -> Result<Option<Version>> {
        self.flush()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // The last chance not to lose the user's edits; an error here has
        // no one to go to
        let _ = self.flush();
    }
}
