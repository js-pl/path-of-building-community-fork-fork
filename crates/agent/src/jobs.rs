//! Jobs: the searches (`front`, `dominating`, `respec`) run as jobs, each
//! with an id, its progress (evaluations done, the best found so far), a
//! time budget, and a cancel.
//!
//! A job works on a copy of the build at the version it started on (its
//! saved XML), so the build can change while it runs; its answer says
//! which version that was. It changes no build: like `best` and `improve`,
//! it answers with changes `apply` takes.
//!
//! With [`Agent::background_jobs`] (the host), a job runs on a thread of
//! its own and the engine thread goes on answering other requests; a call
//! waits for its answer up to its `wait` seconds without holding the
//! engine thread ([`Agent::submit`], [`Agent::poll`]), and then answers
//! with the job's id and progress, which `job` asks for again. Without
//! it (wasm, a caller with data that isn't `'static`), a job runs to its
//! end within the call, still within its budget.

use crate::eval::Budget;
use crate::{Agent, BuildId, Grant, R, Store, check_fields, opt_str};
use serde_json::{Value as J, json};
use session::typed::TypedData;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Jobs running at once, over all connections
pub const MAX_RUNNING: usize = 2;
/// Jobs kept after they finish, the oldest going first
const KEEP: usize = 32;
/// A search's time budget when none is given, and the most it may ask
pub const DEFAULT_SECONDS: f64 = 30.0;
pub const MAX_SECONDS: f64 = 600.0;
/// How long a call waits for a job's answer when it doesn't say, and the
/// most it may wait
pub const DEFAULT_WAIT: f64 = 20.0;
pub const MAX_WAIT: f64 = 120.0;

/// What a job reports while it runs
#[derive(Clone, Debug, Default)]
pub(crate) struct Progress {
    /// What it is doing (`evaluating 40 single moves`, `combinations of
    /// 2 moves`)
    pub phase: String,
    /// The best found so far, short
    pub best: Option<J>,
}

/// A job, shared by the thread running it and the agent answering for it
pub(crate) struct Job {
    pub id: String,
    pub verb: String,
    pub build: BuildId,
    /// The build's name for the connection that started it
    pub name: String,
    /// The build's version it searches
    pub version: u64,
    pub started: Instant,
    /// Its time budget
    pub seconds: f64,
    pub budget: Budget,
    progress: Mutex<Progress>,
    /// Its answer, once it has finished
    result: Mutex<Option<R<J>>>,
    /// When it finished
    finished: Mutex<Option<Instant>>,
}

impl Job {
    pub fn report(&self, phase: impl Into<String>, best: Option<J>) {
        let mut p = self.progress.lock().unwrap();
        p.phase = phase.into();
        if best.is_some() {
            p.best = best;
        }
    }

    pub fn done(&self) -> bool {
        self.result.lock().unwrap().is_some()
    }

    fn finish(&self, r: R<J>) {
        *self.finished.lock().unwrap() = Some(Instant::now());
        *self.result.lock().unwrap() = Some(r);
    }

    fn status(&self) -> &'static str {
        match &*self.result.lock().unwrap() {
            None if self.budget.cancelled() => "cancelling",
            None => "running",
            Some(Err(_)) => "failed",
            Some(Ok(_)) => match self.budget.why() {
                Some("cancelled") => "cancelled",
                Some(_) => "out of time",
                _ => "done",
            },
        }
    }

    /// The job's state, short: what `job` lists
    pub fn json(&self) -> J {
        let end = self.finished.lock().unwrap().unwrap_or_else(Instant::now);
        let p = self.progress.lock().unwrap().clone();
        let mut v = json!({
            "id": self.id,
            "verb": self.verb,
            "build": self.name,
            "version": self.version,
            "status": self.status(),
            "seconds": round2(end.duration_since(self.started).as_secs_f64()),
            "budget_seconds": self.seconds,
            "evaluations": self.budget.done(),
        });
        if !self.done() {
            v["phase"] = json!(p.phase);
            if let Some(b) = p.best {
                v["best_so_far"] = b;
            }
        }
        v
    }

    /// The answer a call gets: the search's own when it finished, else
    /// how to wait for it; [`finish_answer`] adds the job's state
    fn answer(&self) -> J {
        match &*self.result.lock().unwrap() {
            Some(Ok(v)) => json!({ "ok": v }),
            Some(Err(e)) => json!({ "error": e }),
            None => json!({
                "ok": {
                    "note": format!(
                        "the search is still running; call `job` with {{\"job\": \"{}\"}} (and `wait`, seconds) for its answer, or `cancel` to stop it and get the best found so far",
                        self.id
                    ),
                },
            }),
        }
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// A call waiting for a job's answer
struct Waiter {
    job: Arc<Job>,
    until: Instant,
    reply: Box<dyn FnOnce(String)>,
}

/// The jobs of an agent
#[derive(Default)]
pub(crate) struct Jobs {
    list: Vec<Arc<Job>>,
    next: u64,
    /// Set by [`Agent::background_jobs`]: the data job threads use
    background: Option<&'static TypedData>,
    waiters: Vec<Waiter>,
}

/// What a call answers: at once, or once a job finishes or the wait is up
pub(crate) enum Reply {
    Now(J),
    Wait(Arc<Job>, Instant),
}

/// What a job runs: the search on a copy of the build
pub(crate) type Work = Box<dyn FnOnce(&TypedData, &Job) -> R<J> + Send>;

/// What a search's job needs from its request
pub(crate) struct Start {
    pub verb: String,
    pub build: BuildId,
    pub name: String,
    pub version: u64,
    pub seconds: f64,
    pub wait: f64,
}

/// A request's time budget, `seconds`
pub(crate) fn seconds_of(req: &J) -> R<f64> {
    match req.get("seconds") {
        None | Some(J::Null) => Ok(DEFAULT_SECONDS),
        Some(v) => v
            .as_f64()
            .filter(|s| *s > 0.0 && *s <= MAX_SECONDS)
            .ok_or_else(|| {
                format!("seconds is the time budget, more than 0 and at most {MAX_SECONDS}")
            }),
    }
}

/// How long a request waits for a job's answer, `wait`
pub(crate) fn wait_of(req: &J) -> R<f64> {
    match req.get("wait") {
        None | Some(J::Null) => Ok(DEFAULT_WAIT),
        Some(v) => v
            .as_f64()
            .filter(|s| (0.0..=MAX_WAIT).contains(s))
            .ok_or_else(|| format!("wait is 0 to {MAX_WAIT} seconds")),
    }
}

/// Whether the grant may see a job: it reaches the build the job searches
fn sees(grant: &Grant, job: &Job) -> bool {
    grant.resolve(Some(&grant.api_name(&job.build))).as_ref() == Ok(&job.build)
}

impl<S: Store> Agent<'static, S> {
    /// Runs jobs on threads of their own, so a search doesn't hold the
    /// thread answering requests; a host turns this on
    pub fn background_jobs(&mut self) {
        self.jobs.background = Some(self.data);
    }
}

impl<'d, S: Store> Agent<'d, S> {
    /// Answers a request as text through `reply`: at once, or, for a call
    /// that waits for a job, once the job finishes or the wait is up;
    /// [`Agent::poll`] sends those. A host calls this on its engine thread.
    pub fn submit(&mut self, grant: &Grant, request: &str, reply: impl FnOnce(String) + 'static) {
        match self.respond_str(grant, request) {
            Reply::Now(v) => reply(v.to_string()),
            Reply::Wait(job, until) => self.jobs.waiters.push(Waiter {
                job,
                until,
                reply: Box::new(reply),
            }),
        }
    }

    /// Sends the answers of the calls whose job finished or whose wait is
    /// up
    pub fn poll(&mut self) {
        let now = Instant::now();
        let (ready, waiting): (Vec<Waiter>, Vec<Waiter>) = std::mem::take(&mut self.jobs.waiters)
            .into_iter()
            .partition(|w| w.job.done() || now >= w.until);
        self.jobs.waiters = waiting;
        for w in ready {
            (w.reply)(finish_answer(&w.job).to_string());
        }
    }

    /// Whether calls wait for jobs, so a host polls soon
    pub fn waiting(&self) -> bool {
        !self.jobs.waiters.is_empty()
    }

    /// A reply, waited for on this thread when it waits for a job
    pub(crate) fn block(&mut self, r: Reply) -> J {
        match r {
            Reply::Now(v) => v,
            Reply::Wait(job, until) => {
                while !job.done() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
                finish_answer(&job)
            }
        }
    }

    /// Starts a search as a job, and the call's reply
    pub(crate) fn start_job(&mut self, s: Start, work: Work) -> R<Reply> {
        let running = self.jobs.list.iter().filter(|j| !j.done()).count();
        if running >= MAX_RUNNING {
            return Err(format!(
                "{running} searches are running, the most at once; wait for one (`job`) or `cancel` one"
            ));
        }
        self.jobs.next += 1;
        let job = Arc::new(Job {
            id: format!("j{}", self.jobs.next),
            verb: s.verb,
            build: s.build,
            name: s.name,
            version: s.version,
            started: Instant::now(),
            seconds: s.seconds,
            budget: Budget::seconds(s.seconds),
            progress: Mutex::new(Progress {
                phase: "starting".into(),
                best: None,
            }),
            result: Mutex::new(None),
            finished: Mutex::new(None),
        });
        // Forget the oldest finished jobs
        while self.jobs.list.len() >= KEEP {
            match self.jobs.list.iter().position(|j| j.done()) {
                Some(i) => drop(self.jobs.list.remove(i)),
                None => break,
            }
        }
        self.jobs.list.push(job.clone());
        let until = Instant::now() + Duration::from_secs_f64(s.wait);
        let run = move |data: &TypedData, job: &Job| {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(data, job)))
                .unwrap_or_else(|_| Err("the search failed (a panic in the engine)".into()));
            job.finish(r);
        };
        match self.jobs.background {
            Some(data) => {
                let j = job.clone();
                std::thread::Builder::new()
                    .name(format!("pob job {}", job.id))
                    .spawn(move || run(data, &j))
                    .map_err(|e| format!("no thread for the job: {e}"))?;
            }
            None => run(self.data, &job),
        }
        Ok(Reply::Wait(job, until))
    }

    /// `job`: a job's answer, waited for up to `wait` seconds; without
    /// `job`, the jobs this connection can see
    pub(crate) fn job(&mut self, grant: &Grant, req: &J) -> R<Reply> {
        check_fields(req, &["job", "wait"])?;
        let Some(id) = opt_str(req, "job")? else {
            let list: Vec<J> = self
                .jobs
                .list
                .iter()
                .filter(|j| sees(grant, j))
                .map(|j| j.json())
                .collect();
            return Ok(Reply::Now(json!({ "ok": { "jobs": list } })));
        };
        let job = self.find(grant, id)?;
        let wait = match req.get("wait") {
            None | Some(J::Null) => 0.0,
            _ => wait_of(req)?,
        };
        Ok(Reply::Wait(
            job,
            Instant::now() + Duration::from_secs_f64(wait),
        ))
    }

    /// `cancel`: stops a job; its answer is the best found so far
    pub(crate) fn cancel(&mut self, grant: &Grant, req: &J) -> R<Reply> {
        check_fields(req, &["job", "wait"])?;
        let id = opt_str(req, "job")?.ok_or("cancel names its job, {\"job\": \"j1\"}")?;
        let job = self.find(grant, id)?;
        if !job.done() {
            job.budget.cancel();
        }
        // The search stops at its next evaluation; its answer follows soon
        let wait = match req.get("wait") {
            None | Some(J::Null) => 10.0,
            _ => wait_of(req)?,
        };
        Ok(Reply::Wait(
            job,
            Instant::now() + Duration::from_secs_f64(wait),
        ))
    }

    fn find(&self, grant: &Grant, id: &str) -> R<Arc<Job>> {
        self.jobs
            .list
            .iter()
            .find(|j| j.id == id && sees(grant, j))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "no job {:?} of this connection's builds; `job` without an id lists them",
                    crate::text::clean(id, 20, false)
                )
            })
    }

    /// The host's list of jobs, for the UI
    pub fn jobs_json(&self) -> J {
        J::Array(self.jobs.list.iter().map(|j| j.json()).collect())
    }

    /// Cancels a job by its id, for the UI; whether there was one running
    pub fn cancel_job(&self, id: &str) -> bool {
        match self.jobs.list.iter().find(|j| j.id == id && !j.done()) {
            Some(j) => {
                j.budget.cancel();
                true
            }
            None => false,
        }
    }
}

/// A job's answer with its state under `job`
fn finish_answer(job: &Job) -> J {
    let mut a = job.answer();
    if let Some(ok) = a.get_mut("ok").and_then(J::as_object_mut) {
        ok.insert("job".into(), job.json());
    }
    a
}
