//! `eval`: variants of a build, each a list of changes, evaluated exactly
//! on copies of the build on all cores, as a short table. Nothing changes
//! in the build. [`Evaluator`] is what `assumptions`, `best` and `improve`
//! evaluate with.
//!
//! Each thread loads the build's saved XML into a session of its own once,
//! and evaluates each of its variants on a clone of that session, with the
//! changes `apply` runs: the numbers are the engine's full calculation, as
//! the sidebar has them, of the build `apply` would make. Variants are
//! independent of each other and of the order they run in, so the answers
//! are the same with any thread count.

use crate::read::{DIFF_STATS, warnings_of};
use crate::text::{LINE_CAP, clean};
use crate::write::{MAX_CHANGES, check_change, num, round};
use crate::{Agent, BuildId, Names, Open, R, Store, check_fields, cmd, opt_u64};
use engine::calculator::CalcOutput;
use engine::parallel::Threads;
use engine::power::{PowerStat, power_stats};
use serde_json::{Value as J, json};
use session::Dispatcher;
use session::typed::TypedData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Variants in one `eval`
pub(crate) const MAX_VARIANTS: usize = 500;
/// Stats in one table
const MAX_STATS: usize = 12;
/// The stats a table shows when none are asked for, at most
const DEFAULT_STATS: usize = 8;

/// What the numbers of a table are
pub(crate) const EXACT: &str = "every value is the engine's full calculation (the sidebar's) of the build with the variant's changes, on a copy of the build; nothing was changed. They rest on the build's configuration, which `assumptions` lists";

/// A stat a table shows: an output of the engine
#[derive(Clone, Debug)]
pub(crate) struct Stat {
    pub key: String,
    pub label: String,
    /// In points (resistances, chances) rather than a ratio
    pub points: bool,
    /// A power stat, read as upstream's power and trade weights read it
    power: Option<PowerStat>,
}

impl Stat {
    /// A stat by its output key (`FullDPS`, `TotalEHP`, `FireResist`,
    /// `MinionLife`), or by its label
    pub fn new(name: &str, base: &CalcOutput) -> R<Stat> {
        let diff = DIFF_STATS
            .iter()
            .find(|(k, l, _)| *k == name || l.eq_ignore_ascii_case(name));
        let power = power_stats().into_iter().find(|p| {
            p.stat.as_deref() == Some(name)
                || (p.stat.is_some() && p.label.eq_ignore_ascii_case(name))
        });
        let key = match (diff, &power) {
            (Some((k, _, _)), _) => k.to_string(),
            (_, Some(p)) => p.stat.clone().unwrap_or_default(),
            _ if base.num(name).is_some() => name.to_string(),
            _ => {
                return Err(format!(
                    "no stat {:?}; a stat is an output of the engine, as FullDPS, TotalEHP, Life, EnergyShield, FireResist, PhysicalMaximumHitTaken or MovementSpeedMod",
                    clean(name, 60, false)
                ));
            }
        };
        let power = power.filter(|p| p.stat.as_deref() == Some(key.as_str()));
        let label = diff
            .map(|(_, l, _)| l.to_string())
            .or_else(|| power.as_ref().map(|p| p.label.clone()))
            .unwrap_or_else(|| key.clone());
        let points =
            diff.is_some_and(|(_, _, p)| *p) || key.ends_with("Resist") || key.ends_with("Chance");
        Ok(Stat {
            key,
            label,
            points,
            power,
        })
    }

    /// Whether lower is better, as upstream's power stats say
    pub fn lower_is_better(&self) -> bool {
        self.power.as_ref().is_some_and(|p| p.negate)
    }

    /// The stat's value in an output; `None` when the engine has none
    pub fn value(&self, out: &CalcOutput) -> Option<f64> {
        match &self.power {
            // Upstream's reading (Full DPS falls back to the combined DPS,
            // minion stats are the minion's), without the transform:
            // tables show the stat itself
            Some(p) => {
                let v = PowerStat {
                    negate: false,
                    ..p.clone()
                }
                .value(out);
                let known = match self.key.strip_prefix("Minion") {
                    Some(m) if !m.is_empty() => out.minion_num(m).is_some(),
                    _ => out.num(&self.key).is_some() || self.key == "FullDPS",
                };
                known.then_some(v)
            }
            None => out.num(&self.key),
        }
    }

    /// `+14.9%`, or `+3` for a stat in points; empty when unchanged
    pub fn change(&self, base: Option<f64>, new: Option<f64>) -> String {
        let (b, a) = (base.unwrap_or(0.0), new.unwrap_or(0.0));
        if a == b || (a - b).abs() < 1e-9 * a.abs().max(b.abs()).max(1.0) {
            return String::new();
        }
        if !a.is_finite() || !b.is_finite() {
            return format!("{} → {}", num(b), num(a));
        }
        if self.points {
            format!("{:+}", round(a - b))
        } else if b != 0.0 {
            format!("{:+.1}%", (a - b) / b.abs() * 100.0)
        } else {
            format!("{:+}", num(a - b))
        }
    }
}

/// The stats a request names, else `None`
pub(crate) fn stats_of(req: &J, base: &CalcOutput) -> R<Option<Vec<Stat>>> {
    match req.get("stats") {
        None | Some(J::Null) => Ok(None),
        Some(J::Array(list)) if !list.is_empty() && list.len() <= MAX_STATS => list
            .iter()
            .map(|s| {
                Stat::new(
                    s.as_str()
                        .ok_or("stats is a list of stat names, as \"FullDPS\"")?,
                    base,
                )
            })
            .collect::<R<Vec<_>>>()
            .map(Some),
        Some(_) => Err(format!("stats is a list of 1 to {MAX_STATS} stat names")),
    }
}

/// A variant as a request gives it: a list of changes, an object with
/// `changes`, or one change
pub(crate) fn variant_of(v: &J) -> R<Vec<J>> {
    let changes = match v {
        J::Array(list) => list.clone(),
        J::Object(o) if o.contains_key("changes") => {
            if o.keys().any(|k| k != "changes") {
                return Err("a variant is a list of changes, or {\"changes\": [...]}".into());
            }
            o["changes"]
                .as_array()
                .ok_or("a variant's changes are a list")?
                .clone()
        }
        J::Object(o) if o.contains_key("verb") => vec![v.clone()],
        _ => {
            return Err(
                "a variant is a list of changes, each {\"verb\": ..., ...} as `apply` takes them"
                    .into(),
            );
        }
    };
    if changes.len() > MAX_CHANGES {
        return Err(format!("a variant has at most {MAX_CHANGES} changes"));
    }
    for c in &changes {
        check_change(c)?;
    }
    Ok(changes)
}

/// A variant evaluated
#[derive(Clone, Debug)]
pub(crate) struct Outcome {
    /// Each change's label
    pub labels: Vec<String>,
    pub out: CalcOutput,
    pub warnings: Vec<String>,
    /// The passive points used
    pub points: Option<f64>,
}

impl Outcome {
    pub fn label(&self) -> String {
        if self.labels.is_empty() {
            "the build as it is".into()
        } else {
            clean(&self.labels.join("; "), LINE_CAP, false)
        }
    }
}

/// How much an evaluation did
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Effort {
    pub evaluations: usize,
    pub failed: usize,
    /// Variants left unevaluated when the budget ran out or the job was
    /// cancelled
    pub skipped: usize,
    pub threads: usize,
    pub seconds: f64,
}

impl Effort {
    pub fn add(&mut self, o: Effort) {
        self.evaluations += o.evaluations;
        self.failed += o.failed;
        self.skipped += o.skipped;
        self.threads = self.threads.max(o.threads);
        self.seconds += o.seconds;
    }

    pub fn json(&self) -> J {
        let mut v = json!({
            "evaluations": self.evaluations,
            "failed": self.failed,
            "threads": self.threads,
            "seconds": (self.seconds * 100.0).round() / 100.0,
        });
        if self.skipped > 0 {
            v["skipped"] = json!(self.skipped);
        }
        v
    }
}

/// The error of a variant a stopped search didn't evaluate
pub(crate) const SKIPPED: &str = "not evaluated: the search was stopped";

/// When a search stops: a time budget, a cancel, and the evaluations done
/// so far, which a job's progress reads. Clones share all three.
#[derive(Clone, Debug, Default)]
pub(crate) struct Budget {
    cancel: Arc<AtomicBool>,
    deadline: Option<Instant>,
    done: Arc<AtomicUsize>,
}

impl Budget {
    /// No limit
    pub fn none() -> Budget {
        Budget::default()
    }

    /// Stops `seconds` from now
    pub fn seconds(seconds: f64) -> Budget {
        Budget {
            deadline: Some(Instant::now() + Duration::from_secs_f64(seconds)),
            ..Budget::default()
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn out_of_time(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    pub fn stopped(&self) -> bool {
        self.cancelled() || self.out_of_time()
    }

    /// Why the search stopped, if it did
    pub fn why(&self) -> Option<&'static str> {
        if self.cancelled() {
            Some("cancelled")
        } else if self.out_of_time() {
            Some("out of time")
        } else {
            None
        }
    }

    /// The evaluations done so far
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }
}

/// Evaluates variants of one version of a build on copies of it
pub(crate) struct Evaluator<'d> {
    data: &'d TypedData,
    /// The build as saved
    pub xml: String,
    names: Names,
    threads: usize,
}

impl<'d> Evaluator<'d> {
    /// The build's open session as it is now
    pub fn new<S: Store>(agent: &mut Agent<'d, S>, o: &mut Open<'d>) -> R<Evaluator<'d>> {
        // Asked for here once, so the threads' copies have them
        agent.names.gems(&mut o.session)?;
        agent.names.uniques(&mut o.session)?;
        let xml = cmd(&mut o.session, json!({ "cmd": "saveXml" }))?["xml"]
            .as_str()
            .ok_or("saveXml gave no XML")?
            .to_string();
        Ok(Evaluator::of(agent.data, xml, agent.names.clone()))
    }

    /// At most `n` threads
    pub fn with_threads(mut self, n: Option<usize>) -> Evaluator<'d> {
        if let Some(n) = n {
            self.threads = self.threads.min(n.max(1));
        }
        self
    }

    /// A build's saved XML
    pub fn of(data: &'d TypedData, xml: String, names: Names) -> Evaluator<'d> {
        let threads = match Threads::available() {
            Threads::Pool(n) => n,
            _ => 1,
        };
        Evaluator {
            data,
            xml,
            names,
            threads,
        }
    }

    /// The build as it is (its outcome), and each variant's
    pub fn run(&self, variants: &[Vec<J>]) -> (R<Outcome>, Vec<R<Outcome>>, Effort) {
        self.run_within(variants, &Budget::none())
    }

    /// As [`Evaluator::run`], until the budget stops it: variants not
    /// begun by then are [`SKIPPED`]. The build itself is always evaluated.
    pub fn run_within(
        &self,
        variants: &[Vec<J>],
        budget: &Budget,
    ) -> (R<Outcome>, Vec<R<Outcome>>, Effort) {
        let start = Instant::now();
        let all: Vec<&[J]> = std::iter::once(&[][..])
            .chain(variants.iter().map(Vec::as_slice))
            .collect();
        let n = self.threads.clamp(1, all.len());
        let work = |w: usize| -> Vec<(usize, R<Outcome>)> {
            let mut names = self.names.clone();
            let mut base = Dispatcher::typed(self.data);
            if let Err(e) = cmd(
                &mut base,
                json!({ "cmd": "loadXml", "xml": self.xml, "name": "copy" }),
            ) {
                return (w..all.len())
                    .step_by(n)
                    .map(|i| (i, Err(e.clone())))
                    .collect();
            }
            (w..all.len())
                .step_by(n)
                .map(|i| {
                    if i > 0 && budget.stopped() {
                        return (i, Err(SKIPPED.to_string()));
                    }
                    let mut s = base.clone();
                    let r = evaluate(&mut names, &mut s, all[i]);
                    budget.done.fetch_add(1, Ordering::Relaxed);
                    (i, r)
                })
                .collect()
        };
        let threads = if n > 1 {
            Threads::Pool(n)
        } else {
            Threads::Sequential
        };
        let mut results: Vec<Option<R<Outcome>>> = vec![None; all.len()];
        for (i, r) in engine::parallel::map((0..n).collect(), threads, work)
            .into_iter()
            .flatten()
        {
            results[i] = Some(r);
        }
        let mut results = results
            .into_iter()
            .map(|r| r.unwrap_or_else(|| Err("not evaluated".into())));
        let base = results.next().unwrap();
        let rest: Vec<R<Outcome>> = results.collect();
        let skipped = rest
            .iter()
            .filter(|r| matches!(r, Err(e) if e == SKIPPED))
            .count();
        let effort = Effort {
            evaluations: all.len() - skipped,
            failed: rest.iter().filter(|r| r.is_err()).count() - skipped,
            skipped,
            threads: n,
            seconds: start.elapsed().as_secs_f64(),
        };
        (base, rest, effort)
    }
}

/// One variant on a session of its own
fn evaluate(names: &mut Names, s: &mut Dispatcher, changes: &[J]) -> R<Outcome> {
    let mut labels = Vec::new();
    for c in changes {
        let (label, _) = names.change(s, c).map_err(|e| {
            let verb = c["verb"].as_str().unwrap_or("?");
            clean(&format!("{verb}: {e}"), LINE_CAP, false)
        })?;
        labels.push(label);
    }
    let out = s.typed_output().ok_or("no build is open")?;
    let state = cmd(s, json!({ "cmd": "state" }))?;
    let outputs = cmd(s, json!({ "cmd": "outputs" }))?;
    let gems = names.gems(s)?.clone();
    let uniques = names.uniques(s)?.clone();
    let warnings = warnings_of(&state, &outputs["player"], &gems, &uniques);
    Ok(Outcome {
        labels,
        out,
        warnings,
        points: state["points"]["used"].as_f64(),
    })
}

/// A table's columns: the stats asked for, else Full DPS, EHP and the
/// stats of the sidebar's changes any variant changed
pub(crate) fn columns(asked: Option<Vec<Stat>>, base: &Outcome, rows: &[R<Outcome>]) -> Vec<Stat> {
    if let Some(s) = asked {
        return s;
    }
    let mut out = Vec::new();
    for (k, _, _) in DIFF_STATS {
        let Ok(stat) = Stat::new(k, &base.out) else {
            continue;
        };
        let b = stat.value(&base.out);
        let changed = rows
            .iter()
            .flatten()
            .any(|r| !stat.change(b, stat.value(&r.out)).is_empty());
        if changed || *k == "FullDPS" || *k == "TotalEHP" {
            out.push(stat);
        }
        if out.len() == DEFAULT_STATS {
            break;
        }
    }
    out
}

/// A value as tables give it: rounded, `null` when the engine has none
pub(crate) fn value_json(v: Option<f64>) -> J {
    match v {
        Some(v) if v.is_finite() => json!((v * 100.0).round() / 100.0),
        Some(v) => json!(if v > 0.0 { "inf" } else { "-inf" }),
        None => J::Null,
    }
}

/// A row of a table: the variant's values, their changes, the warnings it
/// adds and the ones it resolves
pub(crate) fn row(stats: &[Stat], base: &Outcome, r: &Outcome) -> serde_json::Map<String, J> {
    let mut m = serde_json::Map::new();
    m.insert("changes".into(), json!(r.label()));
    m.insert(
        "values".into(),
        J::Array(stats.iter().map(|s| value_json(s.value(&r.out))).collect()),
    );
    m.insert(
        "change".into(),
        J::Array(
            stats
                .iter()
                .map(|s| json!(s.change(s.value(&base.out), s.value(&r.out))))
                .collect(),
        ),
    );
    let added: Vec<&String> = r
        .warnings
        .iter()
        .filter(|w| !base.warnings.contains(w))
        .collect();
    let resolved: Vec<&String> = base
        .warnings
        .iter()
        .filter(|w| !r.warnings.contains(w))
        .collect();
    if !added.is_empty() {
        m.insert("warnings_added".into(), json!(added));
    }
    if !resolved.is_empty() {
        m.insert("warnings_resolved".into(), json!(resolved));
    }
    m
}

/// The fields `eval` takes
pub(crate) const FIELDS: &[&str] = &["build", "variants", "stats", "objective", "constraints"];

impl<'d, S: Store> Agent<'d, S> {
    pub(crate) fn eval(&mut self, id: &BuildId, name: String, req: &J) -> R<J> {
        check_fields(req, FIELDS)?;
        let list = req["variants"]
            .as_array()
            .ok_or("variants is a list of variants, each a list of changes")?;
        if list.is_empty() || list.len() > MAX_VARIANTS {
            return Err(format!("eval takes 1 to {MAX_VARIANTS} variants"));
        }
        let variants = list
            .iter()
            .enumerate()
            .map(|(i, v)| variant_of(v).map_err(|e| format!("variant {}: {e}", i + 1)))
            .collect::<R<Vec<_>>>()?;
        self.with_open(id, |agent, o| {
            let live = o.session.typed_output().ok_or("no build is open")?;
            let asked = stats_of(req, &live)?;
            let scored = req.get("objective").is_some() || req.get("constraints").is_some();
            let objective = crate::goal::Objective::of(req)?;
            let constraints = crate::goal::Constraint::list(req, &live)?;
            let ev = Evaluator::new(agent, o)?;
            let (base, rows, effort) = ev.run(&variants);
            let base = base.map_err(|e| format!("the build can't be evaluated: {e}"))?;
            let stats = columns(asked, &base, &rows);
            let table: Vec<J> = rows
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let mut m = match r {
                        Ok(r) => {
                            let mut m = row(&stats, &base, r);
                            if scored {
                                let gain = objective.score(&base.out, &r.out)
                                    - objective.score(&base.out, &base.out);
                                m.insert("gain".into(), json!((gain * 1e4).round() / 1e4));
                                let b = crate::goal::broken(&constraints, &base, r);
                                if !b.is_empty() {
                                    m.insert("breaks".into(), json!(b));
                                }
                            }
                            m
                        }
                        Err(e) => {
                            let mut m = serde_json::Map::new();
                            m.insert("error".into(), json!(e));
                            m
                        }
                    };
                    m.insert("variant".into(), json!(i + 1));
                    J::Object(m)
                })
                .collect();
            let mut extra = serde_json::Map::new();
            if scored {
                extra.insert("objective".into(), objective.json());
                extra.insert(
                    "constraints".into(),
                    json!(
                        constraints
                            .iter()
                            .map(crate::goal::Constraint::describe)
                            .collect::<Vec<_>>()
                    ),
                );
            }
            let mut answer = json!({
                "build": name,
                "version": o.version,
                "stats": stats.iter().map(|s| s.label.clone()).collect::<Vec<_>>(),
                "keys": stats.iter().map(|s| s.key.clone()).collect::<Vec<_>>(),
                "base": stats.iter().map(|s| value_json(s.value(&base.out))).collect::<Vec<_>>(),
                "base_warnings": base.warnings,
                "rows": table,
                "effort": effort.json(),
                "exact": EXACT,
            });
            answer.as_object_mut().unwrap().extend(extra);
            Ok(answer)
        })
    }
}

/// A request's `limit`, 1 to `max`
pub(crate) fn limit(req: &J, default: u64, max: u64) -> R<usize> {
    Ok(opt_u64(req, "limit")?.unwrap_or(default).clamp(1, max) as usize)
}
