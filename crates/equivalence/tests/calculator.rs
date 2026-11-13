//! Calculators with overrides: `engine::calculator::MiscCalculator` gives
//! the outputs of the translated `calcs.getMiscCalculator(build)`'s base
//! calculation and of its `calcFunc(override)`, for a sample of overrides
//! per corpus build: unallocated nodes next to the tree allocated,
//! allocated nodes removed, items replaced or taken off, flasks toggled and
//! a condition set. Builds that reach a part not ported yet are skipped and
//! listed. A build that differs is calculated again by a fresh translation,
//! since upstream changes shared skill data for the rest of the process.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::calc_override::Override;
use engine::calculator::{CalcOutput, MiscCalculator};
use engine::tree::NodeType;
use equivalence::outputs::{compare, lua_outputs, typed_outputs};
use equivalence::typed_build::TypedData;
use equivalence::{corpus_builds, upstream_src};
use lua::{LuaError, TableRef, Value, rt};
use std::collections::BTreeMap;
use std::rc::Rc;
use upstream::host::Host;
use upstream::imported::Imported;

fn s(x: &str) -> Value {
    Value::str(x)
}

fn get(v: &Value, k: &str) -> Value {
    rt::index(v, &s(k)).unwrap_or_default()
}

fn describe(e: &LuaError) -> String {
    format!(
        "{}\n  {}",
        lua::to_string(e.value()),
        e.traceback().join("\n  ")
    )
}

/// Nodes per build allocated and removed
const NODES: usize = 3;

/// A set of Lua values, as upstream's override tables hold them
fn set_of(values: &[Value]) -> Value {
    let t = TableRef::new();
    for v in values {
        t.set(v.clone(), Value::Bool(true));
    }
    Value::Table(t)
}

/// An override of the sample, typed and as upstream's table
struct Case {
    name: String,
    typed: Override,
    lua: Value,
}

/// The sample of overrides for a build
fn cases(b: &engine::build::Build, lbuild: &Value) -> Vec<Case> {
    let spec = &b.spec;
    let lnodes = get(&get(lbuild, "spec"), "nodes");
    let lnode = |i: usize| rt::index(&lnodes, &Value::Number(spec.nodes[i].id as f64)).unwrap();
    let litems = get(&get(lbuild, "itemsTab"), "items");
    let litem = |id: f64| rt::index(&litems, &Value::Number(id)).unwrap();
    let table = |pairs: Vec<(&str, Value)>| {
        let t = TableRef::new();
        for (k, v) in pairs {
            t.set_str(k, v);
        }
        Value::Table(t)
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
            lua: table(vec![("addNodes", set_of(&[lnode(i)]))]),
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
            lua: table(vec![("removeNodes", set_of(&[lnode(i)]))]),
        });
    }
    if let [a, b, ..] = removes[..] {
        // Two at once, with a node added
        let mut typed = Override::default().remove_node(a).remove_node(b);
        let mut lua = vec![("removeNodes", set_of(&[lnode(a), lnode(b)]))];
        if let Some(c) = out.first().and_then(|c| c.typed.add_nodes.first()).cloned() {
            let engine::calc_override::OverrideNode::Spec(c) = c else {
                unreachable!()
            };
            typed = typed.add_node(c);
            lua.push(("addNodes", set_of(&[lnode(c)])));
        }
        out.push(Case {
            name: "remove two, add one".into(),
            typed,
            lua: table(lua),
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
                lua: table(vec![("repSlotName", s(slot))]),
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
                lua: table(vec![("repSlotName", s(slot)), ("repItem", litem(id))]),
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
            lua: table(vec![(key, litem(id))]),
        });
    }
    let conditions = TableRef::new();
    conditions.set(Value::Number(1.0), s("Onslaught"));
    out.push(Case {
        name: "condition".into(),
        typed: Override {
            conditions: vec!["Onslaught".into()],
            ..Override::default()
        },
        lua: table(vec![("conditions", Value::Table(conditions))]),
    });
    out
}

fn compare_outputs(what: &str, got: &CalcOutput, want: &Value, fail: &mut Vec<String>) {
    compare(
        &format!("{what}: player"),
        &typed_outputs(&got.player),
        &lua_outputs(want),
        fail,
    );
    let minion = get(want, "Minion");
    match (&got.minion, minion.is_nil()) {
        (None, true) => {}
        (Some(g), false) => compare(
            &format!("{what}: minion"),
            &typed_outputs(g),
            &lua_outputs(&minion),
            fail,
        ),
        (g, _) => fail.push(format!("{what}: minion typed {}", g.is_some())),
    }
}

#[test]
fn overrides_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = TypedData::load();
    let mut failures = Vec::new();
    let (mut compared, mut calcs) = (0, 0);
    let mut aligned: Vec<String> = Vec::new();
    let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, xml) in corpus_builds() {
        if !id.contains(&filter) {
            continue;
        }
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let lbuild = host.build().unwrap();
        let misc = get(&get(&lbuild, "calcsTab"), "miscCalculator");
        let calc_func = rt::index(&misc, &Value::Number(1.0)).unwrap();
        let calc_base = rt::index(&misc, &Value::Number(2.0)).unwrap();

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
        compared += 1;
        let mut fail = Vec::new();
        compare_outputs("base", calc.base(), &calc_base, &mut fail);
        for case in cases(&build, &lbuild) {
            calcs += 1;
            let want = match rt::call(&calc_func, std::slice::from_ref(&case.lua)) {
                Ok(r) => r.into_iter().next().unwrap_or_default(),
                Err(e) => {
                    fail.push(format!(
                        "{}: translated failed: {}",
                        case.name,
                        describe(&e)
                    ));
                    continue;
                }
            };
            match calc.calculate(&case.typed, true) {
                Ok(got) => compare_outputs(&case.name, &got, &want, &mut fail),
                Err(e) if e.starts_with("not ported") => {
                    skips
                        .entry(e)
                        .or_default()
                        .push(format!("{id} {}", case.name));
                }
                Err(e) => fail.push(format!("{}: typed failed: {e}", case.name)),
            }
        }
        // Upstream changes shared skill data for good (`ProcessSocketGroup`
        // clears a triggered granted skill's level cost), so a build loaded
        // earlier can change the translation's answers: a build that
        // differs is calculated again by a fresh translation, and aligned
        // when that one agrees
        if !fail.is_empty() {
            let fresh = Host::new(Rc::new(Imported::new(upstream_src())));
            fresh
                .start()
                .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
            fresh
                .load_build_xml(xml.as_bytes(), &id)
                .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
            let lbuild = fresh.build().unwrap();
            let misc = get(&get(&lbuild, "calcsTab"), "miscCalculator");
            let calc_func = rt::index(&misc, &Value::Number(1.0)).unwrap();
            let calc_base = rt::index(&misc, &Value::Number(2.0)).unwrap();
            let mut again = Vec::new();
            compare_outputs("base", calc.base(), &calc_base, &mut again);
            for case in cases(&build, &lbuild) {
                let Ok(got) = calc.calculate(&case.typed, true) else {
                    continue;
                };
                match rt::call(&calc_func, std::slice::from_ref(&case.lua)) {
                    Ok(r) => {
                        let want = r.into_iter().next().unwrap_or_default();
                        compare_outputs(&case.name, &got, &want, &mut again);
                    }
                    Err(e) => again.push(format!(
                        "{}: translated failed: {}",
                        case.name,
                        describe(&e)
                    )),
                }
            }
            if again.is_empty() {
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
    eprintln!("aligned with a fresh translation: {aligned:?}");
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
