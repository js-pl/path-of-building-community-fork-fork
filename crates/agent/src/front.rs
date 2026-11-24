//! `front` and `dominating`: searches over several objectives at once.
//!
//! The moves are the generators' (`best`'s); the search evaluates every
//! single move exactly, then combines them by a beam search: each round
//! extends the beam's candidates by one more of the best single moves (the
//! pool), evaluates each new combination exactly as a whole build, and
//! keeps the best of them by non-dominated sorting. A candidate that
//! breaks a constraint but would be on the front is first repaired: the
//! cheapest single move that holds that constraint better is added, and
//! the result evaluated exactly, before it's dropped.
//!
//! Both run as jobs ([`crate::jobs`]) on a copy of the build at its
//! version, within a time budget. Every number of an answer is an exact
//! evaluation; what is the best *found* is said so, and an answer claims
//! an exact front only when the search left nothing out.

use crate::eval::{
    Budget, EXACT, Effort, Evaluator, Outcome, SKIPPED, Stat, limit, row, stats_of, value_json,
};
use crate::generate::{GENERATORS, Move, NOT_YET, generate};
use crate::goal::{Constraint, Objective, broken};
use crate::jobs::{Job, Reply, Start, seconds_of, wait_of};
use crate::search::options;
use crate::{Agent, Grant, Names, R, Store, check_fields, cmd, opt_u64};
use engine::calculator::CalcOutput;
use engine::mod_weights::StatWeight;
use serde_json::{Map, Value as J, json};
use session::Dispatcher;
use session::typed::TypedData;
use std::collections::HashSet;

/// The fields `front` and `dominating` take
pub(crate) const FIELDS: &[&str] = &[
    "build",
    "objectives",
    "constraints",
    "generators",
    "stats",
    "depth",
    "candidates",
    "group",
    "gem",
    "combine",
    "beam",
    "limit",
    "seconds",
    "wait",
    "threads",
];

/// The generators when none are named
pub(crate) const DEFAULT_GENERATORS: &[&str] = &["tree", "masteries", "supports", "gem_levels"];

/// Generators a search never takes: the configuration sets up the
/// environment the numbers are for, and any build looks better under a
/// friendlier one
pub(crate) const NOT_SEARCHED: &[&str] = &["config"];
/// The objectives when none are named
const DEFAULT_OBJECTIVES: &[&str] = &["FullDPS", "TotalEHP"];
/// The most moves a candidate combines, and the default
const MAX_COMBINE: u64 = 6;
const DEFAULT_COMBINE: u64 = 3;
/// The beam's width: the most and the default
const MAX_BEAM: u64 = 100;
const DEFAULT_BEAM: u64 = 12;
/// The single moves a round combines with the beam, at most
const MAX_POOL: usize = 40;
/// Candidates repaired, and fixes tried on each
const REPAIRS: usize = 6;
const FIXES: usize = 4;

/// An objective of a front, each to maximise or minimise
#[derive(Clone, Debug)]
pub(crate) enum Axis {
    /// An output of the engine; whether lower is better
    Stat(Stat, bool),
    /// The passive points the changes add: a cost
    Points,
    /// The moves a candidate makes: a cost
    Moves,
}

impl Axis {
    pub fn label(&self) -> String {
        match self {
            Axis::Stat(s, _) => s.label.clone(),
            Axis::Points => "passive points added".into(),
            Axis::Moves => "moves".into(),
        }
    }

    pub fn json(&self) -> J {
        match self {
            Axis::Stat(s, low) => {
                json!({ "objective": s.label, "stat": s.key, "goal": if *low { "min" } else { "max" } })
            }
            Axis::Points => json!({ "objective": self.label(), "goal": "min", "cost": true }),
            Axis::Moves => json!({ "objective": self.label(), "goal": "min", "cost": true }),
        }
    }

    /// The candidate's value, turned so that higher is better
    pub fn value(&self, base: &Outcome, r: &Outcome, moves: usize) -> f64 {
        match self {
            Axis::Stat(s, low) => match s.value(&r.out) {
                Some(v) if !v.is_nan() => {
                    if *low {
                        -v
                    } else {
                        v
                    }
                }
                _ => f64::NEG_INFINITY,
            },
            Axis::Points => -(r.points.unwrap_or(0.0) - base.points.unwrap_or(0.0)),
            Axis::Moves => -(moves as f64),
        }
    }
}

/// A request's `objectives`: two or more, each a stat (`"FullDPS"`, or
/// `{"stat", "goal": "max" | "min"}`) or a cost (`"points"`, `"moves"`);
/// default Full DPS and EHP, and the points added when the tree is
/// searched without a points constraint
pub(crate) fn axes_of(
    req: &J,
    base: &CalcOutput,
    tree: bool,
    constraints: &[Constraint],
) -> R<(Vec<Axis>, Vec<String>)> {
    let mut notes = Vec::new();
    let one = |v: &J| -> R<Axis> {
        let (name, goal) = match v {
            J::String(s) => (s.as_str(), None),
            J::Object(o) => {
                if o.keys().any(|k| k != "stat" && k != "goal") {
                    return Err(
                        "an objective is a stat, or {\"stat\", \"goal\": \"max\" | \"min\"}".into(),
                    );
                }
                let goal = match o.get("goal").and_then(J::as_str) {
                    None => None,
                    Some("max") => Some(false),
                    Some("min") => Some(true),
                    Some(_) => return Err("an objective's goal is \"max\" or \"min\"".into()),
                };
                (
                    o.get("stat")
                        .and_then(J::as_str)
                        .ok_or("an objective names its stat")?,
                    goal,
                )
            }
            _ => {
                return Err(
                    "an objective is a stat's name, \"points\", \"moves\" or {\"stat\", \"goal\"}"
                        .into(),
                );
            }
        };
        match name {
            "points" => Ok(Axis::Points),
            "moves" => Ok(Axis::Moves),
            _ => {
                let s = Stat::new(name, base)?;
                let low = goal.unwrap_or_else(|| s.lower_is_better());
                Ok(Axis::Stat(s, low))
            }
        }
    };
    let mut axes = match req.get("objectives") {
        None | Some(J::Null) => DEFAULT_OBJECTIVES
            .iter()
            .map(|s| one(&json!(s)))
            .collect::<R<Vec<_>>>()?,
        Some(J::Array(l)) if (1..=8).contains(&l.len()) => l.iter().map(one).collect::<R<_>>()?,
        Some(_) => return Err("objectives is a list of 2 to 8 objectives".into()),
    };
    let points = axes.iter().any(|a| matches!(a, Axis::Points));
    if tree
        && !points
        && !constraints
            .iter()
            .any(|c| matches!(c, Constraint::Points(_)))
    {
        axes.push(Axis::Points);
        notes.push("the tree is searched, so the passive points added are an objective too (a cost): every allocation adds points; a {\"points\": n} constraint instead caps them".into());
    }
    if axes.len() < 2 {
        return Err("a front needs two or more objectives; `improve` and `best` take one".into());
    }
    if !axes.iter().any(|a| matches!(a, Axis::Stat(..))) {
        return Err("the objectives need a stat besides the costs".into());
    }
    let mut seen = HashSet::new();
    for a in &axes {
        if !seen.insert(a.label()) {
            return Err(format!("the objective {} is named twice", a.label()));
        }
    }
    Ok((axes, notes))
}

/// A request's `generators`, default [`DEFAULT_GENERATORS`]
pub(crate) fn generators_of(req: &J, default: &[&str]) -> R<Vec<String>> {
    let kinds: Vec<String> = match req.get("generators") {
        None | Some(J::Null) => default.iter().map(|g| g.to_string()).collect(),
        Some(J::Array(l)) if !l.is_empty() && l.len() <= GENERATORS.len() => l
            .iter()
            .map(|g| {
                g.as_str()
                    .map(String::from)
                    .ok_or("generators is a list of names")
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("generators is a list of generator names".into()),
    };
    for k in &kinds {
        if NOT_SEARCHED.contains(&k.as_str()) {
            return Err(format!(
                "{k} is never a search move: the configuration sets up the environment the numbers are for, and any build looks better under a friendlier one; `assumptions` shows what it changes"
            ));
        }
        if !GENERATORS.iter().any(|(g, _)| g == k) {
            return Err(format!(
                "no generator {:?}; the generators: {}",
                crate::text::clean(k, 60, false),
                GENERATORS
                    .iter()
                    .map(|(g, _)| *g)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(kinds)
}

/// What a `front` or `dominating` request asks for, checked
struct Spec {
    kinds: Vec<String>,
    axes: Vec<Axis>,
    notes: Vec<String>,
    constraints: Vec<Constraint>,
    stats: Vec<Stat>,
    opts: crate::generate::Options,
    combine: usize,
    beam: usize,
    limit: usize,
    threads: Option<usize>,
}

impl Spec {
    fn of(req: &J, base: &CalcOutput) -> R<Spec> {
        check_fields(req, FIELDS)?;
        let kinds = generators_of(req, DEFAULT_GENERATORS)?;
        let constraints = Constraint::list(req, base)?;
        let (axes, notes) = axes_of(req, base, kinds.iter().any(|k| k == "tree"), &constraints)?;
        // The tree is ranked by node power of the first stat
        let rank = axes
            .iter()
            .find_map(|a| match a {
                Axis::Stat(s, low) => StatWeight::new(&s.key, if *low { -1.0 } else { 1.0 }),
                _ => None,
            })
            .or_else(|| StatWeight::new("FullDPS", 1.0))
            .expect("FullDPS is a power stat");
        let opts = options(
            req,
            &Objective {
                weights: vec![rank],
            },
        )?;
        let mut stats: Vec<Stat> = axes
            .iter()
            .filter_map(|a| match a {
                Axis::Stat(s, _) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let mut extra = stats_of(req, base)?.unwrap_or_default();
        for c in &constraints {
            if let Constraint::Min(s, _) | Constraint::Max(s, _) | Constraint::Keep(s) = c {
                extra.push(s.clone());
            }
        }
        for s in extra {
            if !stats.iter().any(|o| o.key == s.key) {
                stats.push(s);
            }
        }
        stats.truncate(12);
        let combine = opt_u64(req, "combine")?.unwrap_or(DEFAULT_COMBINE);
        if !(1..=MAX_COMBINE).contains(&combine) {
            return Err(format!("combine is 1 to {MAX_COMBINE} moves"));
        }
        let beam = opt_u64(req, "beam")?.unwrap_or(DEFAULT_BEAM);
        if !(1..=MAX_BEAM).contains(&beam) {
            return Err(format!("beam is 1 to {MAX_BEAM} candidates"));
        }
        let threads = match opt_u64(req, "threads")? {
            None => None,
            Some(n @ 1..=256) => Some(n as usize),
            Some(_) => return Err("threads is 1 to 256".into()),
        };
        Ok(Spec {
            kinds,
            axes,
            notes,
            constraints,
            stats,
            opts,
            combine: combine as usize,
            beam: beam as usize,
            limit: limit(req, 12, 50)?,
            threads,
        })
    }
}

/// Whether `a` is better than `b` in some objective, and worse in some
fn compare(a: &[f64], b: &[f64]) -> (bool, bool) {
    let (mut better, mut worse) = (false, false);
    for (x, y) in a.iter().zip(b) {
        let tol = 1e-9 * x.abs().max(y.abs()).max(1.0);
        if x == y || (x.is_finite() && y.is_finite() && (x - y).abs() <= tol) {
            continue;
        }
        if x > y {
            better = true;
        } else {
            worse = true;
        }
    }
    (better, worse)
}

/// Whether `a` is at least as good as `b` in every objective and better
/// in one
pub(crate) fn dominates(a: &[f64], b: &[f64]) -> bool {
    let (better, worse) = compare(a, b);
    better && !worse
}

/// A candidate: a set of moves, evaluated as a whole build
#[derive(Clone, Debug)]
struct Cand {
    /// The moves, by index, in order
    moves: Vec<usize>,
    out: Outcome,
    /// Each objective's value, higher better
    values: Vec<f64>,
    breaks: Vec<String>,
    /// The move a repair added
    repair: Option<usize>,
}

impl Cand {
    fn feasible(&self) -> bool {
        self.breaks.is_empty()
    }
}

/// The order rows come in among equals: the sum of the stat objectives'
/// changes relative to the build's (costs left out)
fn order_score(axes: &[Axis], base: &[f64], values: &[f64]) -> f64 {
    axes.iter()
        .zip(base.iter().zip(values))
        .filter(|(a, _)| matches!(a, Axis::Stat(..)))
        .map(|(_, (b, v))| {
            if !v.is_finite() || !b.is_finite() {
                if v > b {
                    10.0
                } else if v < b {
                    -10.0
                } else {
                    0.0
                }
            } else {
                ((v - b) / b.abs().max(1e-9)).clamp(-10.0, 10.0)
            }
        })
        .sum()
}

/// What a move may not be combined with: a move on the same thing
fn conflict_key(m: &Move) -> Option<String> {
    let c = m.changes.first()?;
    match c["verb"].as_str()? {
        "allocate" => None,
        "deallocate" => Some(format!("node {}", c["nodes"])),
        "set_mastery" => Some(format!("mastery {}", c["node"])),
        "set_gem" => {
            let field = ["name", "level", "quality"]
                .into_iter()
                .find(|f| c.get(*f).is_some())
                .unwrap_or("gem");
            Some(format!("gem {} {} {field}", c["group"], c["gem"]))
        }
        "set_config" => Some(format!("config {}", c["option"])),
        "equip" => Some(format!("slot {}", c["slot"])),
        v => Some(v.to_string()),
    }
}

/// The search's state
struct Search<'a> {
    data: &'a TypedData,
    xml: String,
    names: Names,
    threads: Option<usize>,
    budget: &'a Budget,
    axes: &'a [Axis],
    constraints: &'a [Constraint],
    moves: Vec<Move>,
    keys: Vec<Option<String>>,
    all: Vec<Cand>,
    seen: HashSet<Vec<usize>>,
    base: Option<Outcome>,
    base_values: Vec<f64>,
    effort: Effort,
    errors: Vec<J>,
}

impl Search<'_> {
    /// Evaluates move sets not evaluated yet; the candidates they made
    fn evaluate(&mut self, sets: Vec<(Vec<usize>, Option<usize>)>) -> Vec<usize> {
        let sets: Vec<(Vec<usize>, Option<usize>)> = sets
            .into_iter()
            .filter(|(s, _)| self.seen.insert(s.clone()))
            .collect();
        let variants: Vec<Vec<J>> = sets
            .iter()
            .map(|(s, _)| {
                s.iter()
                    .flat_map(|&i| self.moves[i].changes.clone())
                    .collect()
            })
            .collect();
        let ev = Evaluator::of(self.data, self.xml.clone(), self.names.clone())
            .with_threads(self.threads);
        let (base, rows, e) = ev.run_within(&variants, self.budget);
        self.effort.add(e);
        if self.base.is_none() {
            match base {
                Ok(b) => {
                    self.base_values = self.axes.iter().map(|a| a.value(&b, &b, 0)).collect();
                    self.base = Some(b);
                }
                Err(e) => {
                    self.errors
                        .push(json!({ "error": format!("the build can't be evaluated: {e}") }));
                    return Vec::new();
                }
            }
        }
        let base = self.base.clone().expect("evaluated");
        let mut made = Vec::new();
        for ((set, repair), r) in sets.into_iter().zip(rows) {
            match r {
                Ok(out) => {
                    let values = self
                        .axes
                        .iter()
                        .map(|a| a.value(&base, &out, set.len()))
                        .collect();
                    let breaks = broken(self.constraints, &base, &out);
                    made.push(self.all.len());
                    self.all.push(Cand {
                        moves: set,
                        out,
                        values,
                        breaks,
                        repair,
                    });
                }
                Err(e) if e == SKIPPED => {
                    self.seen.remove(&set);
                }
                Err(e) => {
                    if self.errors.len() < 5 {
                        let changes: Vec<J> = set
                            .iter()
                            .flat_map(|&i| self.moves[i].changes.clone())
                            .collect();
                        self.errors.push(json!({ "apply": changes, "error": e }));
                    }
                }
            }
        }
        made
    }

    fn score(&self, i: usize) -> f64 {
        order_score(self.axes, &self.base_values, &self.all[i].values)
    }

    /// Candidates sorted by the order among equals, ties by their moves
    fn sort(&self, list: &mut [usize]) {
        list.sort_by(|&a, &b| {
            self.score(b)
                .total_cmp(&self.score(a))
                .then_with(|| self.all[a].moves.cmp(&self.all[b].moves))
        });
    }

    /// Those no other of the list dominates
    fn front(&self, list: &[usize]) -> Vec<usize> {
        let mut out: Vec<usize> = list
            .iter()
            .copied()
            .filter(|&i| {
                !list
                    .iter()
                    .any(|&j| j != i && dominates(&self.all[j].values, &self.all[i].values))
            })
            .collect();
        self.sort(&mut out);
        out
    }

    /// The best `k` of the list by non-dominated sorting: the front first,
    /// then the front of the rest, each in the order among equals
    fn select(&self, list: &[usize], k: usize) -> Vec<usize> {
        let mut rest: Vec<usize> = list.to_vec();
        let mut out = Vec::new();
        while out.len() < k && !rest.is_empty() {
            let layer = self.front(&rest);
            rest.retain(|i| !layer.contains(i));
            out.extend(layer.into_iter().take(k - out.len()));
        }
        out
    }

    fn conflicts(&self, set: &[usize], m: usize) -> bool {
        set.contains(&m)
            || self.keys[m]
                .as_ref()
                .is_some_and(|k| set.iter().any(|&i| self.keys[i].as_ref() == Some(k)))
    }

    fn best_so_far(&self, stats: &[Stat]) -> Option<J> {
        let mut feasible: Vec<usize> = (0..self.all.len())
            .filter(|&i| self.all[i].feasible())
            .collect();
        self.sort(&mut feasible);
        let base = self.base.as_ref()?;
        feasible.first().map(|&i| {
            let r = &self.all[i].out;
            json!({
                "changes": r.label(),
                "change": stats.iter().map(|s| s.change(s.value(&base.out), s.value(&r.out))).collect::<Vec<_>>(),
            })
        })
    }
}

/// A `front` or `dominating` search on a copy of the build, as a job
#[allow(clippy::too_many_arguments)]
fn run(
    data: &TypedData,
    job: &Job,
    req: &J,
    xml: String,
    mut names: Names,
    name: &str,
    version: u64,
    dominating: bool,
) -> R<J> {
    let budget = &job.budget;
    let mut s = Dispatcher::typed(data);
    cmd(
        &mut s,
        json!({ "cmd": "loadXml", "xml": xml, "name": "copy" }),
    )?;
    let live = s.typed_output().ok_or("no build is open")?;
    let spec = Spec::of(req, &live)?;
    job.report("generating moves", None);
    let mut moves: Vec<Move> = Vec::new();
    let mut seen_moves = HashSet::new();
    let mut hows = Vec::new();
    let mut possible = 0;
    let mut exhaustive = true;
    for k in &spec.kinds {
        match generate(k, data, &mut names, &mut s, &spec.opts) {
            Ok(g) => {
                possible += g.possible;
                exhaustive &= g.exhaustive();
                hows.push(format!("{k}: {}", g.how));
                for m in g.moves {
                    if seen_moves.insert(J::Array(m.changes.clone()).to_string()) {
                        moves.push(m);
                    }
                }
            }
            Err(e) => hows.push(format!("{k}: {e}")),
        }
    }
    if moves.is_empty() {
        return Err(format!("the generators made no moves: {}", hows.join("; ")));
    }
    moves.truncate(crate::eval::MAX_VARIANTS);
    let keys = moves.iter().map(conflict_key).collect();
    let mut search = Search {
        data,
        xml,
        names,
        threads: spec.threads,
        budget,
        axes: &spec.axes,
        constraints: &spec.constraints,
        moves,
        keys,
        all: Vec::new(),
        seen: HashSet::new(),
        base: None,
        base_values: Vec::new(),
        effort: Effort::default(),
        errors: Vec::new(),
    };
    let n = search.moves.len();
    job.report(format!("evaluating {n} single moves"), None);
    let singles = search.evaluate((0..n).map(|i| (vec![i], None)).collect());
    let Some(base) = search.base.clone() else {
        return Err(search
            .errors
            .first()
            .and_then(|e| e["error"].as_str())
            .unwrap_or("the build can't be evaluated")
            .to_string());
    };
    // Whether the search left any combination out
    let mut pruned = !exhaustive;
    let mut rounds = vec![json!({
        "moves": 1,
        "evaluated": singles.len(),
        "feasible": singles.iter().filter(|&&i| search.all[i].feasible()).count(),
    })];
    let mut feasible: Vec<usize> = singles
        .iter()
        .copied()
        .filter(|&i| search.all[i].feasible())
        .collect();
    // Candidates that break a constraint are not extended
    pruned |= spec.combine > 1 && feasible.len() < singles.len();
    search.sort(&mut feasible);
    let pool_size = (spec.beam * 2).clamp(8, MAX_POOL);
    let pool: Vec<usize> = feasible
        .iter()
        .take(pool_size)
        .map(|&i| search.all[i].moves[0])
        .collect();
    let mut beam = search.select(&feasible, spec.beam);
    // The beam leaves candidates out only when a round follows
    pruned |= spec.combine > 1 && feasible.len() > beam.len();
    pruned |= spec.combine > 1 && feasible.len() > pool.len();
    let mut stop = format!("combined up to {} moves, as asked", spec.combine);
    for size in 2..=spec.combine {
        if budget.stopped() {
            break;
        }
        let mut sets = Vec::new();
        for &c in &beam {
            for &m in &pool {
                if search.conflicts(&search.all[c].moves, m) {
                    continue;
                }
                let mut set = search.all[c].moves.clone();
                set.push(m);
                set.sort();
                if !search.seen.contains(&set) && !sets.iter().any(|(s, _)| *s == set) {
                    sets.push((set, None));
                }
            }
        }
        if sets.is_empty() {
            stop = format!("no combination of {size} moves is left to try");
            break;
        }
        if sets.len() > crate::eval::MAX_VARIANTS {
            sets.truncate(crate::eval::MAX_VARIANTS);
            pruned = true;
        }
        job.report(
            format!("combinations of {size} moves: evaluating {}", sets.len()),
            search.best_so_far(&spec.stats),
        );
        let made = search.evaluate(sets);
        let new: Vec<usize> = made
            .iter()
            .copied()
            .filter(|&i| search.all[i].feasible())
            .collect();
        pruned |= size < spec.combine && new.len() < made.len();
        rounds.push(json!({ "moves": size, "evaluated": made.len(), "feasible": new.len() }));
        if new.is_empty() {
            stop = format!("no combination of {size} moves holds the constraints");
            break;
        }
        beam = search.select(&new, spec.beam);
        pruned |= size < spec.combine && new.len() > beam.len();
    }
    // Repairs: the candidates that break a constraint but no feasible one
    // dominates, each with the cheapest single move that holds what it
    // breaks better
    let mut repaired = 0;
    if !spec.constraints.is_empty() && !budget.stopped() {
        job.report(
            "repairing candidates that break a constraint",
            search.best_so_far(&spec.stats),
        );
        let ok: Vec<usize> = (0..search.all.len())
            .filter(|&i| search.all[i].feasible())
            .collect();
        let mut broken_ones: Vec<usize> = (0..search.all.len())
            .filter(|&i| {
                !search.all[i].feasible()
                    && search.all[i].repair.is_none()
                    && !ok
                        .iter()
                        .any(|&j| dominates(&search.all[j].values, &search.all[i].values))
            })
            .collect();
        search.sort(&mut broken_ones);
        let mut sets = Vec::new();
        for &c in broken_ones.iter().take(REPAIRS) {
            let cand = &search.all[c];
            let which: Vec<&Constraint> = spec
                .constraints
                .iter()
                .filter(|k| k.broken(&base, &cand.out).is_some())
                .collect();
            // The singles that hold a broken constraint better than the
            // build does, cheapest first
            let mut fixes: Vec<(f64, f64, usize)> = singles
                .iter()
                .filter_map(|&i| {
                    let s = &search.all[i];
                    let help: f64 = which
                        .iter()
                        .map(|k| (k.margin(&base, &s.out) - k.margin(&base, &base)).max(0.0))
                        .sum();
                    let cost = s.out.points.unwrap_or(0.0) - base.points.unwrap_or(0.0);
                    (help > 0.0).then_some((cost, help, s.moves[0]))
                })
                .collect();
            fixes.sort_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then(b.1.total_cmp(&a.1))
                    .then(a.2.cmp(&b.2))
            });
            for (_, _, m) in fixes
                .into_iter()
                .filter(|f| !search.conflicts(&cand.moves, f.2))
                .take(FIXES)
            {
                let mut set = cand.moves.clone();
                set.push(m);
                set.sort();
                sets.push((set, Some(m)));
            }
        }
        let made = search.evaluate(sets);
        repaired = made.iter().filter(|&&i| search.all[i].feasible()).count();
        rounds.push(json!({ "repairs": made.len(), "feasible": repaired }));
    }
    let stopped = budget.why();
    if let Some(why) = stopped {
        stop = format!(
            "stopped ({why}) after {} evaluations: the best found so far",
            search.effort.evaluations
        );
        pruned = true;
    }
    // The answer
    let feasible: Vec<usize> = (0..search.all.len())
        .filter(|&i| search.all[i].feasible())
        .collect();
    let base_breaks = broken(&spec.constraints, &base, &base);
    let base_values = search.base_values.clone();
    let (chosen, on_front, total, base_on_front) = if dominating {
        let better: Vec<usize> = feasible
            .iter()
            .copied()
            .filter(|&i| dominates(&search.all[i].values, &base_values))
            .collect();
        let top = search.front(&better);
        let mut rest: Vec<usize> = better
            .iter()
            .copied()
            .filter(|i| !top.contains(i))
            .collect();
        search.sort(&mut rest);
        let n_top = top.len();
        let mut all = top;
        all.extend(rest);
        (all, n_top, better.len(), false)
    } else {
        let top = search.front(&feasible);
        let base_on = base_breaks.is_empty()
            && !top
                .iter()
                .any(|&i| dominates(&search.all[i].values, &base_values));
        let n = top.len();
        (top, n, n, base_on)
    };
    let stats = &spec.stats;
    let mut rows: Vec<J> = Vec::new();
    for (k, &i) in chosen.iter().take(spec.limit).enumerate() {
        let c = &search.all[i];
        let changes: Vec<J> = c
            .moves
            .iter()
            .flat_map(|&m| search.moves[m].changes.clone())
            .collect();
        let mut m = row(stats, &base, &c.out);
        m.insert("apply".into(), json!(changes));
        m.insert("moves".into(), json!(c.moves.len()));
        if let (Some(a), Some(b)) = (c.out.points, base.points)
            && a != b
        {
            m.insert("points".into(), json!(a - b));
        }
        if let Some(r) = c.repair {
            let label = crate::text::clean(
                &search
                    .all
                    .iter()
                    .find(|x| x.moves == vec![r])
                    .map(|x| x.out.label())
                    .unwrap_or_default(),
                crate::text::LINE_CAP,
                false,
            );
            m.insert(
                "repaired".into(),
                json!(format!("by {label}: it broke a constraint without it")),
            );
        }
        if dominating && k < on_front {
            m.insert("on_front".into(), json!(true));
        }
        rows.push(J::Object(m));
    }
    let evaluated = search.all.len();
    let moves_text = format!("{n} single moves of {}", spec.kinds.join(", "));
    let combos = if spec.combine > 1 {
        format!(", and their combinations of up to {} moves", spec.combine)
    } else {
        String::new()
    };
    let claim = match (dominating, total, pruned) {
        (_, _, _) if stopped.is_some() => format!(
            "the search was stopped ({}) before it finished: the rows are the best found so far among the {evaluated} candidates evaluated, not a complete search",
            stopped.unwrap_or_default()
        ),
        (false, _, false) => format!(
            "every combination of up to {} of the {moves_text} was evaluated exactly: this is the exact Pareto front of those combinations under the constraints. Moves these generators don't make were not searched",
            spec.combine
        ),
        (false, _, true) => format!(
            "the best found: the Pareto front of the {evaluated} candidates evaluated ({moves_text}{combos}, by a beam search of {} with the {} best single moves). No row is beaten on every objective by another candidate evaluated; combinations the beam left out were not evaluated, so builds beyond this front may exist",
            spec.beam,
            pool.len()
        ),
        (true, 0, false) => format!(
            "no combination of up to {} of the {moves_text} is better in an objective and worse in none, under the constraints: every one was evaluated exactly. Moves these generators don't make were not searched",
            spec.combine
        ),
        (true, 0, true) => format!(
            "none of the {evaluated} candidates evaluated ({moves_text}{combos}, by a beam search) is better in an objective and worse in none, under the constraints. That doesn't prove nothing is: combinations the beam left out were not evaluated"
        ),
        (true, t, p) => format!(
            "{t} of the {evaluated} candidates evaluated ({moves_text}{combos}) are better than the build in at least one objective and worse in none, under the constraints, each checked by an exact evaluation of the whole build; those marked on_front are not beaten by another of them.{}",
            if p {
                " Combinations the beam left out were not evaluated, so others may exist"
            } else {
                " Every combination was evaluated"
            }
        ),
    };
    let mut out = Map::new();
    out.insert("build".into(), json!(name));
    out.insert("version".into(), json!(version));
    out.insert(
        "objectives".into(),
        json!(spec.axes.iter().map(Axis::json).collect::<Vec<_>>()),
    );
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
        json!(stats.iter().map(|s| s.label.clone()).collect::<Vec<_>>()),
    );
    out.insert(
        "base".into(),
        json!(
            stats
                .iter()
                .map(|s| value_json(s.value(&base.out)))
                .collect::<Vec<_>>()
        ),
    );
    out.insert("base_breaks".into(), json!(base_breaks));
    if dominating {
        out.insert("dominating".into(), json!(total));
    } else {
        out.insert("front".into(), json!(total));
        out.insert("build_on_front".into(), json!(base_on_front));
    }
    out.insert("rows".into(), json!(rows));
    out.insert(
        "search".into(),
        json!({
            "generators": hows,
            "moves": n,
            "moves_possible": possible,
            "combine": spec.combine,
            "beam": spec.beam,
            "pool": pool.len(),
            "rounds": rounds,
            "candidates": evaluated,
            "repaired": repaired,
            "complete": !pruned,
            "stop": stop,
            "effort": search.effort.json(),
            "failed": search.errors,
        }),
    );
    out.insert("claim".into(), json!(claim));
    out.insert("exact".into(), json!(EXACT));
    out.insert("not_yet".into(), json!(NOT_YET));
    if !spec.notes.is_empty() {
        out.insert("notes".into(), json!(spec.notes));
    }
    out.insert(
        "apply".into(),
        json!("each row's `apply` is the changes as `apply` takes them; add \"expect\": version so a build changed since is refused"),
    );
    Ok(J::Object(out))
}

impl<'d, S: Store> Agent<'d, S> {
    /// `front` and `dominating`: checks the request, then starts the job
    pub(crate) fn front_job(&mut self, grant: &Grant, req: &J) -> R<Reply> {
        let verb = req["verb"].as_str().unwrap_or_default().to_string();
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
        let dominating = verb == "dominating";
        let start = Start {
            verb,
            build: id,
            name: name.clone(),
            version,
            seconds,
            wait,
        };
        self.start_job(
            start,
            Box::new(move |data, job| run(data, job, &req, xml, names, &name, version, dominating)),
        )
    }
}
