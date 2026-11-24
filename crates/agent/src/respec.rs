//! `respec`: local search on the tree at a fixed number of points. Each
//! step swaps nodes: it deallocates one of the allocated nodes that node
//! power ranks least worth (with what depends on it) and allocates one of
//! the best paths node power ranks, and takes the swap that improves the
//! objective most while the build uses no more points than now, refunds
//! no more nodes than `points`, and holds the constraints. Every swap is
//! evaluated exactly, as a whole build. It stops where no swap improves
//! the score: a local optimum of these swaps, not a proven best tree.
//!
//! It runs as a job ([`crate::jobs`]) on a copy of the build, and changes
//! nothing: the answer's `apply` makes the swaps one undo step.

use crate::eval::{EXACT, Effort, Evaluator, Outcome, row, value_json};
use crate::generate::{Move, generate};
use crate::goal::{Constraint, Objective, broken};
use crate::jobs::{Job, Reply, Start, seconds_of, wait_of};
use crate::search::{EPS, options, round4, search_stats};
use crate::{Agent, Grant, Names, R, Store, check_fields, cmd, opt_u64};
use serde_json::{Map, Value as J, json};
use session::Dispatcher;
use session::typed::TypedData;

/// The fields `respec` takes
pub(crate) const FIELDS: &[&str] = &[
    "build",
    "objective",
    "constraints",
    "stats",
    "points",
    "depth",
    "candidates",
    "removals",
    "steps",
    "seconds",
    "wait",
    "threads",
];

/// What a `respec` request asks for, checked
struct Spec {
    objective: Objective,
    constraints: Vec<Constraint>,
    stats: Vec<crate::eval::Stat>,
    /// The most nodes refunded
    points: usize,
    opts: crate::generate::Options,
    removals: usize,
    steps: usize,
    threads: Option<usize>,
    /// Whether the fixed point count was added as a constraint
    fixed: bool,
}

impl Spec {
    fn of(req: &J, base: &engine::calculator::CalcOutput) -> R<Spec> {
        check_fields(req, FIELDS)?;
        let objective = Objective::of(req)?;
        let mut constraints = Constraint::list(req, base)?;
        let fixed = !constraints
            .iter()
            .any(|c| matches!(c, Constraint::Points(_)));
        if fixed {
            constraints.push(Constraint::Points(0.0));
        }
        let stats = search_stats(req, base, &objective, &constraints)?;
        let points = opt_u64(req, "points")?.unwrap_or(8);
        if !(1..=60).contains(&points) {
            return Err("points is the most nodes refunded, 1 to 60".into());
        }
        let mut opts = options(req, &objective)?;
        if req.get("candidates").is_none_or(J::is_null) {
            opts.candidates = 12;
        }
        opts.candidates = opts.candidates.min(40);
        let removals = opt_u64(req, "removals")?.unwrap_or(8);
        if !(1..=40).contains(&removals) {
            return Err("removals is 1 to 40 nodes tried for removal each step".into());
        }
        let steps = opt_u64(req, "steps")?.unwrap_or(points);
        if !(1..=60).contains(&steps) {
            return Err("steps is 1 to 60".into());
        }
        let threads = match opt_u64(req, "threads")? {
            None => None,
            Some(n @ 1..=256) => Some(n as usize),
            Some(_) => return Err("threads is 1 to 256".into()),
        };
        Ok(Spec {
            objective,
            constraints,
            stats,
            points: points as usize,
            opts,
            removals: removals as usize,
            steps: steps as usize,
            threads,
            fixed,
        })
    }
}

/// The nodes of the build that a variant no longer allocates
fn refunded(base: &Outcome, r: &Outcome) -> usize {
    base.alloc
        .iter()
        .filter(|n| r.alloc.binary_search(n).is_err())
        .count()
}

#[allow(clippy::too_many_arguments)]
fn run(
    data: &TypedData,
    job: &Job,
    req: &J,
    xml: String,
    mut names: Names,
    name: &str,
    version: u64,
) -> R<J> {
    let budget = &job.budget;
    let mut current = Dispatcher::typed(data);
    cmd(
        &mut current,
        json!({ "cmd": "loadXml", "xml": xml, "name": "copy" }),
    )?;
    let live = current.typed_output().ok_or("no build is open")?;
    let spec = Spec::of(req, &live)?;
    let first = Evaluator::of(data, xml.clone(), names.clone()).with_threads(spec.threads);
    let (base, _, mut effort) = first.run_within(&[], budget);
    let base = base.map_err(|e| format!("the build can't be evaluated: {e}"))?;
    let base_score = spec.objective.score(&base.out, &base.out);
    let mut score = base_score;
    let mut current_xml = xml;
    let mut accepted: Vec<J> = Vec::new();
    let mut taken: Vec<J> = Vec::new();
    let mut last: Option<Outcome> = None;
    let mut stop = format!("stopped after {} steps, the most asked for", spec.steps);
    let mut swaps = 0;
    let mut remove_opts = spec.opts.clone();
    remove_opts.candidates = spec.removals;
    for step in 1..=spec.steps {
        if budget.stopped() {
            break;
        }
        job.report(
            format!("step {step}: ranking nodes"),
            last.as_ref().map(|r| {
                json!({ "changes": r.label(), "gain": round4(score - base_score), "refunded": refunded(&base, r) })
            }),
        );
        let remove = generate("tree_remove", data, &mut names, &mut current, &remove_opts)?;
        let add = generate("tree", data, &mut names, &mut current, &spec.opts)?;
        let node = |m: &Move| m.changes[0]["nodes"][0].clone();
        let mut variants: Vec<Vec<J>> = Vec::new();
        for r in &remove.moves {
            for a in &add.moves {
                if node(r) != node(a) {
                    variants.push(r.changes.iter().chain(&a.changes).cloned().collect());
                }
            }
        }
        if variants.is_empty() {
            stop = format!("stopped at step {step}: no node to swap");
            break;
        }
        job.report(
            format!("step {step}: evaluating {} swaps", variants.len()),
            None,
        );
        let ev = Evaluator::of(data, current_xml.clone(), names.clone()).with_threads(spec.threads);
        let (_, rows, e) = ev.run_within(&variants, budget);
        effort.add(e);
        swaps += rows.iter().filter(|r| r.is_ok()).count();
        let mut best: Option<(usize, f64)> = None;
        for (i, r) in rows.iter().enumerate() {
            let Ok(r) = r else { continue };
            let s = spec.objective.score(&base.out, &r.out);
            if refunded(&base, r) > spec.points
                || !broken(&spec.constraints, &base, r).is_empty()
                || s <= score + EPS
            {
                continue;
            }
            if best.is_none_or(|(_, b)| s > b + EPS) {
                best = Some((i, s));
            }
        }
        let Some((i, s)) = best else {
            stop = if budget.stopped() {
                format!(
                    "stopped at step {step}: {}",
                    budget.why().unwrap_or_default()
                )
            } else {
                format!(
                    "stopped at step {step}: no swap of the {} evaluated improves the score within the points and the constraints",
                    rows.len()
                )
            };
            break;
        };
        let r = rows[i].as_ref().expect("chosen rows are evaluated").clone();
        for c in &variants[i] {
            names.change(&mut current, c)?;
        }
        current_xml = cmd(&mut current, json!({ "cmd": "saveXml" }))?["xml"]
            .as_str()
            .ok_or("saveXml gave no XML")?
            .to_string();
        accepted.extend(variants[i].iter().cloned());
        score = s;
        let mut m = row(&spec.stats, &base, &r);
        m.insert("step".into(), json!(step));
        m.insert("apply".into(), json!(variants[i]));
        m.insert("gain".into(), json!(round4(score - base_score)));
        m.insert("refunded".into(), json!(refunded(&base, &r)));
        m.insert("swaps_evaluated".into(), json!(rows.len()));
        taken.push(J::Object(m));
        last = Some(r);
    }
    if let Some(why) = budget.why() {
        stop = format!(
            "stopped ({why}) after {} evaluations: the best found so far",
            effort.evaluations
        );
    }
    let mut out = Map::new();
    out.insert("build".into(), json!(name));
    out.insert("version".into(), json!(version));
    out.insert("objective".into(), spec.objective.json());
    out.insert(
        "constraints".into(),
        json!(
            spec.constraints
                .iter()
                .map(Constraint::describe)
                .collect::<Vec<_>>()
        ),
    );
    out.insert(
        "stats".into(),
        json!(
            spec.stats
                .iter()
                .map(|s| s.label.clone())
                .collect::<Vec<_>>()
        ),
    );
    out.insert(
        "base".into(),
        json!(
            spec.stats
                .iter()
                .map(|s| value_json(s.value(&base.out)))
                .collect::<Vec<_>>()
        ),
    );
    out.insert(
        "base_breaks".into(),
        json!(broken(&spec.constraints, &base, &base)),
    );
    out.insert("steps".into(), json!(taken));
    if let Some(r) = &last {
        let mut f = row(&spec.stats, &base, r);
        f.remove("changes");
        f.insert("gain".into(), json!(round4(score - base_score)));
        f.insert("refunded".into(), json!(refunded(&base, r)));
        if let (Some(a), Some(b)) = (r.points, base.points) {
            f.insert("points".into(), json!(a - b));
        }
        out.insert("final".into(), J::Object(f));
        out.insert(
            "apply".into(),
            json!({ "verb": "apply", "changes": accepted, "expect": version }),
        );
    }
    let effort: Effort = effort;
    out.insert(
        "search".into(),
        json!({
            "points": spec.points,
            "removals": spec.removals,
            "additions": spec.opts.candidates,
            "depth": spec.opts.depth,
            "swaps_evaluated": swaps,
            "stop": stop,
            "complete": budget.why().is_none(),
            "effort": effort.json(),
        }),
    );
    out.insert(
        "claim".into(),
        json!(format!(
            "local search at a fixed number of points: each step evaluates exactly every swap of one of the {} allocated nodes node power ranks least worth for one of the {} best paths within {} points, and takes the best that improves the score, refunds at most {} nodes in all and holds the constraints. It stops at a local optimum of these swaps, not a proven best tree; node power (an estimate) chose which swaps were tried. Nothing was changed: `apply` makes it one undo step",
            spec.removals, spec.opts.candidates, spec.opts.depth, spec.points
        )),
    );
    out.insert("exact".into(), json!(EXACT));
    if spec.fixed {
        out.insert(
            "notes".into(),
            json!(["the build may use no more passive points than now ({\"points\": n} in constraints allows n more)"]),
        );
    }
    Ok(J::Object(out))
}

impl<'d, S: Store> Agent<'d, S> {
    /// `respec`: checks the request, then starts the job
    pub(crate) fn respec_job(&mut self, grant: &Grant, req: &J) -> R<Reply> {
        check_fields(req, FIELDS)?;
        let id = grant.resolve(req.get("build").and_then(J::as_str))?;
        let name = grant.api_name(&id);
        let seconds = seconds_of(req)?;
        let wait = wait_of(req)?;
        let (xml, version) = self.with_open(&id, |agent, o| {
            let live = o.session.typed_output().ok_or("no build is open")?;
            Spec::of(req, &live)?;
            agent.names.gems(&mut o.session)?;
            agent.names.uniques(&mut o.session)?;
            let xml = cmd(&mut o.session, json!({ "cmd": "saveXml" }))?["xml"]
                .as_str()
                .ok_or("saveXml gave no XML")?
                .to_string();
            Ok((xml, o.version))
        })?;
        let names = self.names.clone();
        let req = req.clone();
        let start = Start {
            verb: "respec".into(),
            build: id,
            name: name.clone(),
            version,
            seconds,
            wait,
        };
        self.start_job(
            start,
            Box::new(move |data, job| run(data, job, &req, xml, names, &name, version)),
        )
    }
}
