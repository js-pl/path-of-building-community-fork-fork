//! `best` and `improve`: single moves from the generators, ranked by an
//! objective under constraints, each evaluated exactly. Neither changes
//! the build: they answer with the changes, which `apply` takes as they
//! are. Their answers say how much they searched, and call a result the
//! best only of what was evaluated.

use crate::eval::{EXACT, Evaluator, Outcome, Stat, limit, row, stats_of, value_json};
use crate::generate::{GENERATORS, Generated, Move, NOT_YET, Options, generate};
use crate::goal::{Constraint, Objective, broken};
use crate::{Agent, BuildId, R, Store, check_fields, opt_str, opt_u64};
use serde_json::{Map, Value as J, json};

/// The fields `best` takes
pub(crate) const BEST_FIELDS: &[&str] = &[
    "build",
    "generator",
    "objective",
    "constraints",
    "stats",
    "limit",
    "depth",
    "candidates",
    "group",
    "gem",
];

/// The fields `improve` takes
pub(crate) const IMPROVE_FIELDS: &[&str] = &[
    "build",
    "generators",
    "objective",
    "constraints",
    "stats",
    "steps",
    "depth",
    "candidates",
    "group",
    "gem",
];

/// `improve`'s generators when none are named
const IMPROVE_DEFAULT: &[&str] = &["masteries"];

/// A score better than another by more than rounding
const EPS: f64 = 1e-9;

fn round4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

/// The generators' options from a request
fn options(req: &J, objective: &Objective) -> R<Options> {
    let depth = match req.get("depth") {
        None | Some(J::Null) => 3.0,
        Some(v) => v
            .as_f64()
            .filter(|d| (1.0..=12.0).contains(d))
            .ok_or("depth is 1 to 12 points")?,
    };
    Ok(Options {
        depth,
        candidates: opt_u64(req, "candidates")?
            .unwrap_or(40)
            .clamp(1, crate::eval::MAX_VARIANTS as u64) as usize,
        group: opt_u64(req, "group")?,
        gem: opt_u64(req, "gem")?,
        rank: objective.rank_stat().clone(),
    })
}

/// The stats a search's table shows: those asked for, else the
/// objective's and the constraints'
fn search_stats(
    req: &J,
    base: &engine::calculator::CalcOutput,
    objective: &Objective,
    constraints: &[Constraint],
) -> R<Vec<Stat>> {
    if let Some(s) = stats_of(req, base)? {
        return Ok(s);
    }
    let mut out: Vec<Stat> = Vec::new();
    let mut push = |s: Stat| {
        if !out.iter().any(|o| o.key == s.key) {
            out.push(s);
        }
    };
    for w in &objective.weights {
        if let Some(k) = &w.stat.stat {
            push(Stat::new(k, base)?);
        }
    }
    for c in constraints {
        if let Constraint::Min(s, _) | Constraint::Max(s, _) | Constraint::Keep(s) = c {
            push(s.clone());
        }
    }
    Ok(out)
}

/// The variants evaluated, ranked: those that break no constraint first,
/// each by its gain, ties by the generator's order
fn ranked(
    objective: &Objective,
    constraints: &[Constraint],
    base: &Outcome,
    rows: &[R<Outcome>],
) -> Vec<(usize, f64, Vec<String>)> {
    let base_score = objective.score(&base.out, &base.out);
    let mut out: Vec<(usize, f64, Vec<String>)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let r = r.as_ref().ok()?;
            let gain = objective.score(&base.out, &r.out) - base_score;
            Some((
                i,
                if gain.is_nan() {
                    f64::NEG_INFINITY
                } else {
                    gain
                },
                broken(constraints, base, r),
            ))
        })
        .collect();
    out.sort_by(|a, b| {
        a.2.is_empty()
            .cmp(&b.2.is_empty())
            .reverse()
            .then(b.1.total_cmp(&a.1))
            .then(a.0.cmp(&b.0))
    });
    out
}

/// A ranked row as answers give it
fn ranked_row(
    stats: &[Stat],
    base: &Outcome,
    r: &Outcome,
    changes: &[J],
    gain: f64,
    breaks: &[String],
    estimate: Option<f64>,
) -> J {
    let mut m = row(stats, base, r);
    m.insert("apply".into(), json!(changes));
    m.insert("gain".into(), json!(round4(gain)));
    if let (Some(a), Some(b)) = (r.points, base.points)
        && a != b
    {
        m.insert("points".into(), json!(a - b));
    }
    if let Some(e) = estimate {
        m.insert("estimate".into(), json!(round4(e)));
    }
    if !breaks.is_empty() {
        m.insert("breaks".into(), json!(breaks));
    }
    J::Object(m)
}

impl<'d, S: Store> Agent<'d, S> {
    pub(crate) fn best(&mut self, id: &BuildId, name: String, req: &J) -> R<J> {
        check_fields(req, BEST_FIELDS)?;
        let kind = opt_str(req, "generator")?.ok_or_else(|| {
            format!(
                "best takes a generator: {}",
                GENERATORS
                    .iter()
                    .map(|(g, _)| *g)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let objective = Objective::of(req)?;
        let opts = options(req, &objective)?;
        let limit = limit(req, 10, 100)?;
        let data = self.data;
        self.with_open(id, |agent, o| {
            let live = o.session.typed_output().ok_or("no build is open")?;
            let constraints = Constraint::list(req, &live)?;
            let stats = search_stats(req, &live, &objective, &constraints)?;
            let generated = generate(kind, data, &mut agent.names, &mut o.session, &opts)?;
            let ev = Evaluator::new(agent, o)?;
            let variants: Vec<Vec<J>> = generated.moves.iter().map(|m| m.changes.clone()).collect();
            let (base, rows, effort) = ev.run(&variants);
            let base = base.map_err(|e| format!("the build can't be evaluated: {e}"))?;
            let order = ranked(&objective, &constraints, &base, &rows);
            let feasible = order.iter().filter(|r| r.2.is_empty()).count();
            let better = order.iter().filter(|r| r.2.is_empty() && r.1 > EPS).count();
            let table: Vec<J> = order
                .iter()
                .take(limit)
                .map(|(i, gain, breaks)| {
                    let Move { changes, estimate } = &generated.moves[*i];
                    let r = rows[*i].as_ref().expect("ranked rows are evaluated");
                    ranked_row(&stats, &base, r, changes, *gain, breaks, *estimate)
                })
                .collect();
            let failed: Vec<J> = rows
                .iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    r.as_ref()
                        .err()
                        .map(|e| json!({ "apply": generated.moves[i].changes, "error": e }))
                })
                .take(5)
                .collect();
            let claim = claim(&generated, better, rows.len());
            Ok(json!({
                "build": name,
                "version": o.version,
                "generator": kind,
                "objective": objective.json(),
                "constraints": constraints.iter().map(Constraint::describe).collect::<Vec<_>>(),
                "stats": stats.iter().map(|s| s.label.clone()).collect::<Vec<_>>(),
                "base": stats.iter().map(|s| value_json(s.value(&base.out))).collect::<Vec<_>>(),
                "base_breaks": broken(&constraints, &base, &base),
                "rows": table,
                "search": {
                    "moves_possible": generated.possible,
                    "evaluated": rows.len(),
                    "feasible": feasible,
                    "better": better,
                    "how": generated.how,
                    "effort": effort.json(),
                    "failed": failed,
                },
                "claim": claim,
                "exact": EXACT,
                "not_yet": NOT_YET,
            }))
        })
    }

    pub(crate) fn improve(&mut self, id: &BuildId, name: String, req: &J) -> R<J> {
        check_fields(req, IMPROVE_FIELDS)?;
        let objective = Objective::of(req)?;
        let opts = options(req, &objective)?;
        let steps = opt_u64(req, "steps")?.unwrap_or(3).clamp(1, 10) as usize;
        let kinds: Vec<String> = match req.get("generators") {
            None | Some(J::Null) => IMPROVE_DEFAULT.iter().map(|g| g.to_string()).collect(),
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
            if k == "config" {
                return Err(crate::generate::NO_CONFIG.into());
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
        let data = self.data;
        self.with_open(id, |agent, o| {
            let live = o.session.typed_output().ok_or("no build is open")?;
            let constraints = Constraint::list(req, &live)?;
            if kinds.iter().any(|k| k == "tree")
                && !constraints.iter().any(|c| matches!(c, Constraint::Points(_)))
            {
                return Err("improve with the tree generator needs a points constraint, {\"points\": n}: each allocation adds points".into());
            }
            let stats = search_stats(req, &live, &objective, &constraints)?;
            // The build as it is: what every step is scored and checked
            // against
            let first = Evaluator::new(agent, o)?;
            let (base, _, mut effort) = first.run(&[]);
            let base = base.map_err(|e| format!("the build can't be evaluated: {e}"))?;
            let base_score = objective.score(&base.out, &base.out);
            let mut score = base_score;
            let mut current = o.session.clone();
            let mut accepted: Vec<J> = Vec::new();
            let mut taken: Vec<J> = Vec::new();
            let mut last: Option<Outcome> = None;
            let mut stop = format!("stopped after {steps} steps, the most asked for");
            let mut tried = 0;
            for step in 1..=steps {
                // The moves on the build as the steps so far made it
                let mut moves: Vec<Move> = Vec::new();
                let mut hows = Vec::new();
                for k in &kinds {
                    match generate(k, data, &mut agent.names, &mut current, &opts) {
                        Ok(Generated { moves: m, how, .. }) => {
                            moves.extend(m);
                            hows.push(format!("{k}: {how}"));
                        }
                        // A generator with nothing to make at this step
                        Err(e) => hows.push(format!("{k}: {e}")),
                    }
                }
                moves.truncate(crate::eval::MAX_VARIANTS);
                tried += moves.len();
                let xml = crate::cmd(&mut current, json!({ "cmd": "saveXml" }))?["xml"]
                    .as_str()
                    .ok_or("saveXml gave no XML")?
                    .to_string();
                let ev = Evaluator::of(data, xml, agent.names.clone());
                let variants: Vec<Vec<J>> = moves.iter().map(|m| m.changes.clone()).collect();
                let (_, rows, e) = ev.run(&variants);
                effort.add(e);
                let order = ranked(&objective, &constraints, &base, &rows);
                let Some((i, gain, _)) = order
                    .iter()
                    .find(|(_, g, b)| b.is_empty() && base_score + g > score + EPS)
                else {
                    stop = format!(
                        "stopped at step {step}: no move of {} improves the score under the constraints ({} moves evaluated)",
                        kinds.join(", "),
                        moves.len()
                    );
                    break;
                };
                let r = rows[*i].as_ref().expect("ranked rows are evaluated").clone();
                for c in &moves[*i].changes {
                    agent.names.change(&mut current, c)?;
                }
                accepted.extend(moves[*i].changes.iter().cloned());
                score = base_score + gain;
                let mut m = row(&stats, &base, &r);
                m.insert("step".into(), json!(step));
                m.insert("apply".into(), json!(moves[*i].changes));
                m.insert("gain".into(), json!(round4(*gain)));
                m.insert("moves_evaluated".into(), json!(moves.len()));
                m.insert("generated".into(), json!(hows));
                taken.push(J::Object(m));
                last = Some(r);
            }
            let mut out = Map::new();
            out.insert("build".into(), json!(name));
            out.insert("version".into(), json!(o.version));
            out.insert("objective".into(), objective.json());
            out.insert(
                "constraints".into(),
                json!(constraints.iter().map(Constraint::describe).collect::<Vec<_>>()),
            );
            out.insert("stats".into(), json!(stats.iter().map(|s| s.label.clone()).collect::<Vec<_>>()));
            out.insert(
                "base".into(),
                json!(stats.iter().map(|s| value_json(s.value(&base.out))).collect::<Vec<_>>()),
            );
            out.insert("base_breaks".into(), json!(broken(&constraints, &base, &base)));
            out.insert("steps".into(), json!(taken));
            if let Some(r) = &last {
                let mut f = row(&stats, &base, r);
                f.remove("changes");
                f.insert("gain".into(), json!(round4(score - base_score)));
                out.insert("final".into(), J::Object(f));
                out.insert(
                    "apply".into(),
                    json!({ "verb": "apply", "changes": accepted, "expect": o.version }),
                );
            }
            out.insert(
                "search".into(),
                json!({
                    "generators": kinds,
                    "moves_evaluated": tried,
                    "stop": stop,
                    "effort": effort.json(),
                }),
            );
            out.insert(
                "claim".into(),
                json!("greedy: each step takes the best single move of the generators, evaluated exactly on the build as the steps before made it; the final build is evaluated as a whole. It stops where no single move improves the score, a local optimum of these moves, not a proven best build. Nothing was changed: `apply` makes it one undo step"),
            );
            out.insert("exact".into(), json!(EXACT));
            Ok(J::Object(out))
        })
    }
}

/// What a `best` answer may claim
fn claim(g: &Generated, better: usize, evaluated: usize) -> String {
    let found = if better == 0 {
        "no move improves the score under the constraints".to_string()
    } else {
        format!(
            "{better} moves improve the score under the constraints; the first row is the best of them"
        )
    };
    if g.exhaustive() {
        format!(
            "{found}: every move this generator makes ({evaluated}) was evaluated exactly, so this is the best single move of its kind. Combinations of moves are not searched"
        )
    } else {
        format!(
            "{found}, of the {evaluated} moves evaluated out of {} possible: moves the ranking left out were not evaluated, so a better one may exist. Combinations of moves are not searched",
            g.possible
        )
    }
}
