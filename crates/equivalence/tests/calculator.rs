//! Calculators with overrides: `engine::calculator::MiscCalculator` gives
//! the outputs of upstream's `calcs.getMiscCalculator(build)` on the live
//! oracle (upstream on LuaJIT, `oracleCalculator` of `oracle/session.lua`):
//! its base calculation and its `calcFunc(override)`, for a sample of
//! overrides per corpus build: unallocated nodes next to the tree
//! allocated, allocated nodes removed, items replaced or taken off, flasks
//! toggled and a condition set. Numbers compare bitwise. Builds that reach
//! a part not ported yet are skipped and listed. A build that differs is
//! asked of a fresh oracle, since upstream changes shared skill data for
//! the rest of the process.
//!
//! The corpus runs in shards (`testkit::run_shards`), each with an oracle
//! process of its own. `CALCS_FILTER=<substring>` limits the builds run.

use engine::calc_override::Override;
use engine::calculator::{CalcOutput, MiscCalculator};
use engine::tree::NodeType;
use equivalence::oracle::{J, diff, output_json, parse};
use equivalence::typed_build::TypedData;
use equivalence::{corpus_builds, upstream_src};
use serde_json::{Value, json};
use session::Session;
use std::collections::BTreeMap;

/// Nodes per build allocated and removed
const NODES: usize = 3;

/// An override of the sample, typed and as the oracle request's case
struct Case {
    name: String,
    typed: Override,
    oracle: Value,
}

fn cases(b: &engine::build::Build) -> Vec<Case> {
    let spec = &b.spec;
    let lnode = |i: usize| json!(spec.nodes[i].id);
    let litem = |id: f64| json!(id);
    let table = |pairs: Vec<(&str, Value)>| {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    };
    let kind_ok = |i: usize| {
        let n = spec.tree_node(i);
        matches!(
            n.kind,
            NodeType::Normal | NodeType::Notable | NodeType::Keystone
        ) && n.ascendancy_name.is_none()
            && !spec.mod_list(i).is_empty()
    };
    let mut by_id: Vec<usize> = (0..spec.nodes.len()).collect();
    by_id.sort_by_key(|&i| spec.nodes[i].id);
    let mut out = Vec::new();
    let adds = by_id
        .iter()
        .copied()
        .filter(|&i| !spec.nodes[i].alloc && spec.nodes[i].path_dist == 1.0 && kind_ok(i));
    for i in adds.take(NODES) {
        out.push(Case {
            name: format!("add {}", spec.nodes[i].id),
            typed: Override::default().add_node(i),
            oracle: table(vec![("addNodes", json!([lnode(i)]))]),
        });
    }
    let removes: Vec<usize> = by_id
        .iter()
        .copied()
        .filter(|&i| {
            spec.nodes[i].alloc && (kind_ok(i) || spec.tree_node(i).kind == NodeType::Mastery)
        })
        .collect();
    for &i in removes.iter().take(NODES) {
        out.push(Case {
            name: format!("remove {}", spec.nodes[i].id),
            typed: Override::default().remove_node(i),
            oracle: table(vec![("removeNodes", json!([lnode(i)]))]),
        });
    }
    if let [a, b, ..] = removes[..] {
        // Two at once, with a node added
        let mut typed = Override::default().remove_node(a).remove_node(b);
        let mut lua = vec![("removeNodes", json!([lnode(a), lnode(b)]))];
        if let Some(c) = out.first().and_then(|c| c.typed.add_nodes.first()).cloned() {
            let engine::calc_override::OverrideNode::Spec(c) = c else {
                unreachable!()
            };
            typed = typed.add_node(c);
            lua.push(("addNodes", json!([lnode(c)])));
        }
        out.push(Case {
            name: "remove two, add one".into(),
            typed,
            oracle: table(lua),
        });
    }
    let tab = &b.items;
    let selected = |slot: &str| {
        tab.ordered_slots
            .iter()
            .find(|s| s.slot_name == slot)
            .map(|s| s.sel_item_id)
            .filter(|&id| tab.item(id).is_some())
    };
    for slot in ["Helmet", "Body Armour", "Weapon 1", "Amulet"] {
        if selected(slot).is_some() {
            out.push(Case {
                name: format!("take off {slot}"),
                typed: Override {
                    rep_item: Some((slot.into(), None)),
                    ..Override::default()
                },
                oracle: table(vec![("repSlotName", json!(slot))]),
            });
        }
    }
    for (slot, from) in [("Ring 1", "Ring 2"), ("Weapon 1", "Weapon 2")] {
        if let Some(id) = selected(from) {
            out.push(Case {
                name: format!("{from} into {slot}"),
                typed: Override {
                    rep_item: Some((slot.into(), tab.item(id).cloned())),
                    ..Override::default()
                },
                oracle: table(vec![("repSlotName", json!(slot)), ("repItem", litem(id))]),
            });
        }
    }
    for n in 1..=5 {
        let Some(id) = selected(&format!("Flask {n}")) else {
            continue;
        };
        let (typed, key) = match tab.item(id).and_then(|i| i.typ.as_deref()) {
            Some("Flask") => (
                Override {
                    toggle_flask: Some(id),
                    ..Override::default()
                },
                "toggleFlask",
            ),
            Some("Tincture") => (
                Override {
                    toggle_tincture: Some(id),
                    ..Override::default()
                },
                "toggleTincture",
            ),
            _ => continue,
        };
        out.push(Case {
            name: format!("toggle {key} {n}"),
            typed,
            oracle: table(vec![(key, litem(id))]),
        });
    }
    out.push(Case {
        name: "condition".into(),
        typed: Override {
            conditions: vec!["Onslaught".into()],
            ..Override::default()
        },
        oracle: table(vec![("conditions", json!(["Onslaught"]))]),
    });
    out
}

/// An actor's outputs as the oracle gives them (`Minion` left out)
fn actors(out: &CalcOutput) -> J {
    let player = match output_json(&out.player) {
        J::Obj(mut o) => {
            o.remove("Minion");
            J::Obj(o)
        }
        j => j,
    };
    let mut obj = BTreeMap::from([("player".to_string(), player)]);
    if let Some(m) = &out.minion {
        obj.insert("minion".to_string(), output_json(m));
    }
    J::Obj(obj)
}

fn compare_outputs(what: &str, got: &CalcOutput, want: &J, fail: &mut Vec<String>) {
    let mut d = Vec::new();
    diff(&actors(got), want, "", &mut d);
    fail.extend(d.into_iter().map(|d| format!("{what}: {d}")));
}

/// The oracle's answer for a build and its cases: the base outputs and
/// each case's (or upstream's error)
fn ask(
    oracle: &Session,
    id: &str,
    xml: &str,
    cases: &[Case],
) -> Result<(J, Vec<Result<J, String>>), String> {
    // `cmd` first: the oracle tells its own requests by their start
    let rest = json!({
        "id": id,
        "xml": xml,
        "cases": cases.iter().map(|c| c.oracle.clone()).collect::<Vec<_>>(),
    })
    .to_string();
    let req = format!(r#"{{"cmd":"oracleCalculator",{}"#, &rest[1..]);
    let res = oracle.request(&req)?;
    let J::Obj(mut o) = parse(res.as_bytes(), &mut 0) else {
        return Err(format!("not an object: {res}"));
    };
    if let Some(J::Str(e)) = o.remove("error") {
        return Err(e);
    }
    let Some(J::Obj(mut ok)) = o.remove("ok") else {
        return Err(format!("an answer of neither ok nor error: {res}"));
    };
    let base = ok.remove("base").unwrap_or(J::Null);
    let answers = match ok.remove("cases") {
        Some(J::Arr(a)) => a,
        _ => Vec::new(),
    };
    let answers = answers
        .into_iter()
        .map(|a| match a {
            J::Obj(mut a) if a.contains_key("error") => match a.remove("error") {
                Some(J::Str(e)) => Err(e),
                e => Err(format!("{e:?}")),
            },
            a => Ok(a),
        })
        .collect();
    Ok((base, answers))
}

/// The differences of a build's calculator from the oracle's
fn differences(
    calc: &MiscCalculator,
    cases: &[Case],
    base: &J,
    answers: &[Result<J, String>],
) -> Vec<String> {
    let mut fail = Vec::new();
    compare_outputs("base", calc.base(), base, &mut fail);
    for (case, want) in cases.iter().zip(answers) {
        let want = match want {
            Ok(w) => w,
            Err(e) => {
                fail.push(format!("{}: upstream failed: {e}", case.name));
                continue;
            }
        };
        match calc.calculate(&case.typed, true) {
            Ok(got) => compare_outputs(&case.name, &got, want, &mut fail),
            Err(e) => fail.push(format!("{}: typed failed: {e}", case.name)),
        }
    }
    fail
}

/// Shards of the corpus, each with an oracle of its own
const SHARDS: usize = 4;

#[test]
fn overrides_match_upstream() {
    let test = "overrides_match_upstream";
    if let Some(shard) = testkit::shard() {
        let (k, n) = shard.split_once('/').unwrap();
        return run_shard(k.parse().unwrap(), n.parse().unwrap());
    }
    let shards: Vec<String> = (0..SHARDS).map(|k| format!("{k}/{SHARDS}")).collect();
    testkit::run_shards(test, &shards, &[" builds", "aligned "]);
}

fn run_shard(k: usize, n: usize) {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let mut data = TypedData::load();
    let mut oracle = Session::oracle(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let mut failures = Vec::new();
    let (mut compared, mut calcs) = (0, 0);
    let mut aligned: Vec<String> = Vec::new();
    let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let builds = corpus_builds()
        .into_iter()
        .filter(|(id, _)| id.contains(&filter))
        .enumerate()
        .filter(|(i, _)| i % n == k)
        .map(|(_, b)| b);
    for (id, xml) in builds {
        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        data.load_tree(&root);
        let mut build = match data.load_build(&root) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{id}: build: {e}"));
                continue;
            }
        };
        let calc_data = data.calc_data();
        let calc = engine::calcs::build_outputs(&mut build, &calc_data)
            .and_then(|_| MiscCalculator::new(&build, &calc_data));
        let calc = match calc {
            Ok(c) => c,
            Err(e) if e.starts_with("not ported") => {
                skips.entry(e).or_default().push(id.clone());
                continue;
            }
            Err(e) => {
                failures.push(format!("{id}: typed calculator failed: {e}"));
                continue;
            }
        };
        // Typed parts not ported yet leave their cases out
        let cases: Vec<Case> = cases(&build)
            .into_iter()
            .filter(|c| match calc.calculate(&c.typed, true) {
                Err(e) if e.starts_with("not ported") => {
                    skips.entry(e).or_default().push(format!("{id} {}", c.name));
                    false
                }
                _ => true,
            })
            .collect();
        compared += 1;
        calcs += cases.len();
        let (base, answers) =
            ask(&oracle, &id, &xml, &cases).unwrap_or_else(|e| panic!("{id}: oracle: {e}"));
        let mut fail = differences(&calc, &cases, &base, &answers);
        // Upstream changes shared skill data for good (`ProcessSocketGroup`
        // clears a triggered granted skill's level cost), so a build loaded
        // earlier can change the oracle's answers: a build that differs is
        // asked of a fresh oracle, and aligned when that one agrees
        if !fail.is_empty() {
            oracle = Session::oracle(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
            let (base, answers) =
                ask(&oracle, &id, &xml, &cases).unwrap_or_else(|e| panic!("{id}: oracle: {e}"));
            if differences(&calc, &cases, &base, &answers).is_empty() {
                aligned.push(id.clone());
                fail.clear();
            }
        }
        if !fail.is_empty() {
            let n = fail.len();
            fail.truncate(if filter.is_empty() { 20 } else { 500 });
            failures.push(format!(
                "{id}: {n} differences\n    {}",
                fail.join("\n    ")
            ));
        }
    }
    let skipped: usize = skips.values().map(Vec::len).sum();
    eprintln!("{compared} builds, {calcs} overrides compared, {skipped} skipped: {skips:?}");
    eprintln!("aligned with a fresh oracle: {aligned:?}");
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
