//! Objectives and constraints: the language of "maximise Full DPS with
//! life at least 5000 and resistances capped". `eval`, `best` and
//! `improve` score and check their variants with them; the searches of
//! step 9 (`dominating`, `front`) are meant to take them as they are.
//!
//! An **objective** is upstream's `WeightedRatioOutputs`
//! (`engine::mod_weights::weighted_ratio`): the sum, by weight, of each
//! stat's ratio of the variant's value to the build's, so a score's units
//! match the trade weights'. A **constraint** is hard: a variant that
//! breaks one is reported as such and never ranked above one that doesn't.

use crate::R;
use crate::eval::{Outcome, Stat};
use engine::calculator::CalcOutput;
use engine::mod_weights::{StatWeight, weighted_ratio};
use serde_json::{Value as J, json};

/// The objective presets
const PRESETS: &[(&str, &[(&str, f64)])] = &[
    // The trade popup's default weights
    ("default", &[("FullDPS", 1.0), ("TotalEHP", 0.5)]),
    ("dps", &[("FullDPS", 1.0)]),
    ("ehp", &[("TotalEHP", 1.0)]),
    ("balanced", &[("FullDPS", 1.0), ("TotalEHP", 1.0)]),
    ("max_hit", &[("SecondMinimalMaximumHitTaken", 1.0)]),
    ("life", &[("Life", 1.0)]),
];

/// A weighted sum of power stats, each by its ratio to the build's
#[derive(Clone, Debug)]
pub(crate) struct Objective {
    pub weights: Vec<StatWeight>,
}

impl Objective {
    /// From a request's `objective`: a preset's name, one stat, or a list
    /// of `{"stat", "weight"}`; default upstream's trade weights
    pub fn of(req: &J) -> R<Objective> {
        let stat_weight = |stat: &str, w: f64| {
            StatWeight::new(stat, w).ok_or_else(|| {
                format!(
                    "no power stat {:?} for an objective; power stats are FullDPS, TotalEHP, Life, EnergyShield, SecondMinimalMaximumHitTaken, EffectiveMovementSpeedMod, ...",
                    crate::text::clean(stat, 60, false)
                )
            })
        };
        let preset = |name: &str| -> R<Vec<StatWeight>> {
            match PRESETS.iter().find(|(p, _)| *p == name) {
                Some((_, list)) => list.iter().map(|(s, w)| stat_weight(s, *w)).collect(),
                None => Ok(vec![stat_weight(name, 1.0).map_err(|_| {
                    format!(
                        "no objective {:?}: a preset ({}), a power stat, or a list of {{\"stat\", \"weight\"}}",
                        crate::text::clean(name, 60, false),
                        PRESETS.iter().map(|(p, _)| *p).collect::<Vec<_>>().join(", ")
                    )
                })?]),
            }
        };
        let one = |v: &J| -> R<StatWeight> {
            let stat = v["stat"]
                .as_str()
                .ok_or("an objective's stat is {\"stat\": \"FullDPS\", \"weight\": 1}")?;
            let weight = match v.get("weight") {
                None | Some(J::Null) => 1.0,
                Some(w) => w.as_f64().ok_or("a weight is a number")?,
            };
            if v.as_object()
                .is_some_and(|o| o.keys().any(|k| k != "stat" && k != "weight"))
            {
                return Err("an objective's stat takes stat and weight".into());
            }
            stat_weight(stat, weight)
        };
        let weights =
            match req.get("objective") {
                None | Some(J::Null) => preset("default")?,
                Some(J::String(s)) => preset(s)?,
                Some(J::Array(list)) if !list.is_empty() && list.len() <= 10 => {
                    list.iter().map(one).collect::<R<_>>()?
                }
                Some(v @ J::Object(_)) => vec![one(v)?],
                Some(_) => return Err(
                    "objective is a preset, a stat, or a list of 1 to 10 {\"stat\", \"weight\"}"
                        .into(),
                ),
            };
        if weights.iter().all(|w| w.weight == 0.0) {
            return Err("an objective needs a weight other than 0".into());
        }
        Ok(Objective { weights })
    }

    /// The variant's score against the build's: the build itself scores
    /// the sum of the weights of its stats that aren't 0
    pub fn score(&self, base: &CalcOutput, new: &CalcOutput) -> f64 {
        weighted_ratio(base, new, &self.weights)
    }

    /// The stat node power ranks the tree by: the heaviest
    pub fn rank_stat(&self) -> &StatWeight {
        self.weights
            .iter()
            .max_by(|a, b| a.weight.abs().total_cmp(&b.weight.abs()))
            .expect("an objective has a stat")
    }

    pub fn json(&self) -> J {
        json!({
            "weights": self.weights.iter().map(|w| json!({
                "stat": w.stat.stat,
                "label": w.stat.label,
                "weight": w.weight,
                "lower_is_better": w.stat.negate,
            })).collect::<Vec<_>>(),
            "gain": "upstream's WeightedRatioOutputs minus the build's own: by weight, each stat's ratio to the build's, less 1 (+0.1 is +10% at weight 1)",
        })
    }
}

/// A hard constraint
#[derive(Clone, Debug)]
pub(crate) enum Constraint {
    /// A stat at least this
    Min(Stat, f64),
    /// A stat at most this
    Max(Stat, f64),
    /// A stat no lower than the build's now
    Keep(Stat),
    /// Fire, cold and lightning resistances at their caps
    ResCapped,
    /// Attribute and level requirements met
    Requirements,
    /// Life and mana reserved within 100%
    Reservation,
    /// No warning the build doesn't have now
    NoNewWarnings,
    /// At most this many passive points more than the build's now
    Points(f64),
}

/// The constraint names a request takes
const NAMED: &[&str] = &[
    "res_capped",
    "requirements",
    "reservation",
    "no_new_warnings",
];

impl Constraint {
    /// A request's `constraints`: a list of names (`res_capped`,
    /// `requirements`, `reservation`, `no_new_warnings`) and objects
    /// (`{"stat", "min"}`, `{"stat", "max"}`, `{"keep": stat}`,
    /// `{"points": n}`)
    pub fn list(req: &J, base: &CalcOutput) -> R<Vec<Constraint>> {
        let list = match req.get("constraints") {
            None | Some(J::Null) => return Ok(Vec::new()),
            Some(J::Array(l)) if l.len() <= 30 => l,
            Some(_) => return Err("constraints is a list of at most 30 constraints".into()),
        };
        list.iter()
            .map(|c| match c {
                J::String(s) => match s.as_str() {
                    "res_capped" => Ok(Constraint::ResCapped),
                    "requirements" => Ok(Constraint::Requirements),
                    "reservation" => Ok(Constraint::Reservation),
                    "no_new_warnings" => Ok(Constraint::NoNewWarnings),
                    s => Err(format!(
                        "no constraint {:?}; the named ones: {}, and {{\"stat\", \"min\"}}, {{\"stat\", \"max\"}}, {{\"keep\": stat}}, {{\"points\": n}}",
                        crate::text::clean(s, 60, false),
                        NAMED.join(", ")
                    )),
                },
                J::Object(o) => {
                    let num = |k: &str| {
                        o.get(k)
                            .map(|v| v.as_f64().ok_or_else(|| format!("a constraint's {k} is a number")))
                            .transpose()
                    };
                    if let Some(k) = o.get("keep") {
                        if o.len() > 1 {
                            return Err("{\"keep\": stat} takes nothing else".into());
                        }
                        let k = k.as_str().ok_or("keep names a stat")?;
                        return Ok(Constraint::Keep(Stat::new(k, base)?));
                    }
                    if let Some(p) = num("points")? {
                        if o.len() > 1 || p < 0.0 {
                            return Err("{\"points\": n} takes a number of points, 0 or more".into());
                        }
                        return Ok(Constraint::Points(p));
                    }
                    let stat = o
                        .get("stat")
                        .and_then(J::as_str)
                        .ok_or("a constraint is a name or {\"stat\": ..., \"min\": ...}")?;
                    let stat = Stat::new(stat, base)?;
                    if o.keys().any(|k| !["stat", "min", "max"].contains(&k.as_str())) {
                        return Err("a stat's constraint takes stat, min and max".into());
                    }
                    match (num("min")?, num("max")?) {
                        (Some(m), None) => Ok(Constraint::Min(stat, m)),
                        (None, Some(m)) => Ok(Constraint::Max(stat, m)),
                        _ => Err("a stat's constraint takes min or max".into()),
                    }
                }
                _ => Err("a constraint is a name or an object".into()),
            })
            .collect()
    }

    /// What the variant breaks, if it does
    pub fn broken(&self, base: &Outcome, r: &Outcome) -> Option<String> {
        let shown = |v: Option<f64>| v.map_or("none".to_string(), crate::write::num);
        match self {
            Constraint::Min(s, m) => {
                let v = s.value(&r.out);
                (v.unwrap_or(f64::NEG_INFINITY) < *m)
                    .then(|| format!("{} {} below {}", s.label, shown(v), crate::write::num(*m)))
            }
            Constraint::Max(s, m) => {
                let v = s.value(&r.out);
                (v.unwrap_or(f64::INFINITY) > *m)
                    .then(|| format!("{} {} above {}", s.label, shown(v), crate::write::num(*m)))
            }
            Constraint::Keep(s) => {
                let (b, v) = (s.value(&base.out), s.value(&r.out));
                (v.unwrap_or(f64::NEG_INFINITY) < b.unwrap_or(f64::NEG_INFINITY))
                    .then(|| format!("{} {} → {}", s.label, shown(b), shown(v)))
            }
            Constraint::ResCapped => {
                let low: Vec<String> = ["Fire", "Cold", "Lightning"]
                    .iter()
                    .filter_map(|e| {
                        let m = r.out.num(&format!("Missing{e}Resist")).unwrap_or(0.0);
                        (m > 0.0).then(|| format!("{e} {m} below its cap"))
                    })
                    .collect();
                (!low.is_empty()).then(|| format!("resistances: {}", low.join(", ")))
            }
            Constraint::Requirements => r
                .warnings
                .iter()
                .find(|w| w.contains("requirement"))
                .cloned(),
            Constraint::Reservation => r
                .warnings
                .iter()
                .find(|w| w.contains("reserved over 100%"))
                .cloned(),
            Constraint::NoNewWarnings => r
                .warnings
                .iter()
                .find(|w| !base.warnings.contains(w))
                .map(|w| format!("a new warning: {w}")),
            Constraint::Points(p) => {
                let (b, v) = (base.points.unwrap_or(0.0), r.points.unwrap_or(0.0));
                (v - b > *p).then(|| format!("{} more passive points than the {p} allowed", v - b))
            }
        }
    }

    /// How well the variant holds the constraint, as a number: higher holds
    /// it better; what a repair looks for in a move
    pub fn margin(&self, base: &Outcome, r: &Outcome) -> f64 {
        let v = |s: &Stat, o: &Outcome| s.value(&o.out).unwrap_or(f64::NEG_INFINITY);
        let count =
            |f: &dyn Fn(&String) -> bool| -(r.warnings.iter().filter(|w| f(w)).count() as f64);
        match self {
            Constraint::Min(s, m) => v(s, r) - m,
            Constraint::Max(s, m) => m - s.value(&r.out).unwrap_or(f64::INFINITY),
            Constraint::Keep(s) => v(s, r) - v(s, base),
            Constraint::ResCapped => -["Fire", "Cold", "Lightning"]
                .iter()
                .map(|e| {
                    r.out
                        .num(&format!("Missing{e}Resist"))
                        .unwrap_or(0.0)
                        .max(0.0)
                })
                .sum::<f64>(),
            Constraint::Requirements => count(&|w| w.contains("requirement")),
            Constraint::Reservation => count(&|w| w.contains("reserved over 100%")),
            Constraint::NoNewWarnings => count(&|w| !base.warnings.contains(w)),
            Constraint::Points(p) => p - (r.points.unwrap_or(0.0) - base.points.unwrap_or(0.0)),
        }
    }

    /// The constraint as answers name it
    pub fn describe(&self) -> String {
        match self {
            Constraint::Min(s, m) => format!("{} at least {}", s.label, crate::write::num(*m)),
            Constraint::Max(s, m) => format!("{} at most {}", s.label, crate::write::num(*m)),
            Constraint::Keep(s) => format!("{} no lower than now", s.label),
            Constraint::ResCapped => "fire, cold and lightning resistances capped".into(),
            Constraint::Requirements => "requirements met".into(),
            Constraint::Reservation => "reservation within 100%".into(),
            Constraint::NoNewWarnings => "no new warnings".into(),
            Constraint::Points(p) => format!("at most {p} more passive points"),
        }
    }
}

/// Everything a variant breaks
pub(crate) fn broken(constraints: &[Constraint], base: &Outcome, r: &Outcome) -> Vec<String> {
    constraints
        .iter()
        .filter_map(|c| c.broken(base, r))
        .collect()
}
