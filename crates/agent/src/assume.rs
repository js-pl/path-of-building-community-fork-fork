//! `assumptions`: what the build's numbers rest on (the configuration:
//! enemy, buffs, charges, conditions) and which numbers are exact; for one
//! stat, which options change it and by how much, each option changed on
//! a copy of the build and evaluated exactly.

use crate::eval::{Evaluator, Stat, limit, value_json};
use crate::read::config_option;
use crate::text::{NAME_CAP, clean, plain};
use crate::write::spaced;
use crate::{Agent, BuildId, R, Store, check_fields, cmd, opt_str};
use serde_json::{Map, Value as J, json};

/// The fields `assumptions` takes
pub(crate) const FIELDS: &[&str] = &["build", "stat", "limit"];

/// Which numbers are exact and which are estimates
fn exactness() -> J {
    json!({
        "exact": [
            "summary, eval, assumptions, best and improve, and evaluate_item's `changed`: the engine's full calculation (the sidebar's), which the equivalence tests hold equal to upstream's",
        ],
        "estimate": [
            "evaluate_item's mod weights: each mod at a mid roll on a blank item, linear",
            "best's ranking of tree paths by node power before they are evaluated exactly; the rows best gives are exact",
        ],
        "not_ported": "a number the engine can't give is answered as not ported, never guessed",
    })
}

/// A choice as answers show it: the game's text, its spaces collapsed
fn choice(s: &str) -> String {
    clean(&spaced(&plain(s)), NAME_CAP, false)
}

impl<'d, S: Store> Agent<'d, S> {
    pub(crate) fn assumptions(&mut self, id: &BuildId, name: String, req: &J) -> R<J> {
        check_fields(req, FIELDS)?;
        let stat = opt_str(req, "stat")?;
        let limit = limit(req, 15, 100)?;
        self.with_open(id, |agent, o| {
            let config = cmd(&mut o.session, json!({ "cmd": "config" }))?;
            let sections = config["sections"].as_array().cloned().unwrap_or_default();
            let mut set = Map::new();
            for sec in &sections {
                let mut lines = Vec::new();
                for v in sec["vars"].as_array().into_iter().flatten() {
                    if v.get("value").is_none() {
                        continue;
                    }
                    let opt = config_option(v, sec);
                    let value = match &opt["value"] {
                        J::String(s) => choice(s),
                        v => v.to_string(),
                    };
                    lines.push(format!(
                        "{}: {value}",
                        opt["label"].as_str().unwrap_or_default()
                    ));
                }
                if !lines.is_empty() {
                    let label = plain(sec["label"].as_str().unwrap_or_default());
                    set.insert(label, json!(lines));
                }
            }
            let mut out = Map::new();
            out.insert("build".into(), json!(name));
            out.insert("version".into(), json!(o.version));
            out.insert("config".into(), J::Object(set));
            out.insert(
                "defaults".into(),
                json!("options not listed are at their defaults; `search` with kind `config` finds them, `set_config` sets one"),
            );
            out.insert("numbers".into(), exactness());
            let Some(stat) = stat else {
                return Ok(J::Object(out));
            };
            let live = o.session.typed_output().ok_or("no build is open")?;
            let stat = Stat::new(stat, &live)?;
            // Each shown option changed one way at a time: a check box
            // flipped, each other choice of a list, a number set cleared
            let mut variants = Vec::new();
            let mut tried = Vec::new();
            let mut skipped = 0;
            for sec in &sections {
                for v in sec["vars"].as_array().into_iter().flatten() {
                    if v["shown"] == false || v["enabled"] == false {
                        continue;
                    }
                    let opt = config_option(v, sec);
                    let var = v["var"].as_str().unwrap_or_default();
                    let mut push = |to: J, shown: J| {
                        variants.push(vec![json!({ "verb": "set_config", "option": var, "value": to })]);
                        tried.push((opt.clone(), shown));
                    };
                    match v["class"].as_str() {
                        Some("CheckBoxControl") => {
                            let now = v["state"].as_bool().unwrap_or(false);
                            push(json!(!now), json!(!now));
                        }
                        Some("DropDownControl") => {
                            let sel = v["sel"].as_u64().unwrap_or(1) as usize;
                            for (i, c) in v["list"].as_array().into_iter().flatten().enumerate() {
                                if i + 1 == sel {
                                    continue;
                                }
                                let c = c.as_str().unwrap_or_default();
                                push(json!(plain(c)), json!(choice(c)));
                            }
                        }
                        _ => {
                            if opt["value"].is_null() {
                                skipped += 1;
                            } else {
                                push(J::Null, json!("default"));
                            }
                        }
                    }
                }
            }
            let ev = Evaluator::new(agent, o)?;
            let (base, rows, effort) = ev.run(&variants);
            let base = base.map_err(|e| format!("the build can't be evaluated: {e}"))?;
            let b = stat.value(&base.out);
            let mut changes: Vec<(f64, J)> = Vec::new();
            for ((opt, to), r) in tried.iter().zip(&rows) {
                let Ok(r) = r else { continue };
                let a = stat.value(&r.out);
                let change = stat.change(b, a);
                if change.is_empty() {
                    continue;
                }
                let from = match &opt["value"] {
                    J::String(s) => json!(choice(s)),
                    J::Null => json!("default"),
                    v => v.clone(),
                };
                let size = match (a, b) {
                    (Some(a), Some(b)) if b != 0.0 && !stat.points => ((a - b) / b).abs(),
                    (Some(a), Some(b)) => (a - b).abs() / 100.0,
                    _ => f64::INFINITY,
                };
                changes.push((
                    if size.is_nan() { f64::INFINITY } else { size },
                    json!({
                        "option": opt["option"],
                        "label": opt["label"],
                        "from": from,
                        "to": to,
                        "value": value_json(a),
                        "change": change,
                    }),
                ));
            }
            changes.sort_by(|x, y| y.0.total_cmp(&x.0));
            let total = changes.len();
            out.insert("stat".into(), json!(stat.label));
            out.insert("key".into(), json!(stat.key));
            out.insert("value".into(), value_json(b));
            out.insert(
                "changed_by".into(),
                json!(changes.into_iter().take(limit).map(|(_, j)| j).collect::<Vec<_>>()),
            );
            out.insert(
                "search".into(),
                json!({
                    "options_tried": tried.len(),
                    "change_it": total,
                    "numbers_left_unset": skipped,
                    "effort": effort.json(),
                    "how": "each option shown changed alone on a copy of the build and evaluated exactly: a check box flipped, each other choice of a list, a number set back to its default; numbers not set are not tried",
                }),
            );
            Ok(J::Object(out))
        })
    }
}
