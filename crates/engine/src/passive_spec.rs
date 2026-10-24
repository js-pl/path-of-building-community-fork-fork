//! Path finding of upstream's `PassiveSpec` class (`Classes/PassiveSpec.lua`),
//! in native Rust over the program's own tables. [`install`] replaces the
//! translated methods with these.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use lua::env::Env;
use lua::rt::{self, func};
use lua::{LResult, LuaError, Table, TableRef, Value};
use std::rc::Rc;

fn s(x: &str) -> Value {
    Value::str(x)
}

fn get(v: &Value, key: &str) -> LResult<Value> {
    rt::index(v, &s(key))
}

fn is(v: &Value, x: &str) -> bool {
    matches!(v, Value::Str(s) if s.as_bytes() == x.as_bytes())
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

/// `t[i]` for `i = 1, 2, ...` until nil, as `ipairs` visits them.
fn ipairs_at(t: &Value, i: usize) -> LResult<Value> {
    rt::index(t, &n(i as f64))
}

/// `PassiveSpecClass:BuildPathFromNode(root)`: a breadth-first search from
/// `root`, recording in every node it is the closest allocated start to its
/// distance (`pathDist`) and path (`path`, the node first).
fn build_path_from_node(spec: &Value, root: &Value) -> LResult<()> {
    rt::set_index(root, s("pathDist"), n(0.0))?;
    rt::set_index(root, s("path"), Value::Table(TableRef::new()))?;
    let nodes = get(spec, "nodes")?;
    let mut queue = vec![root.clone()];
    let mut o = 0;
    while o < queue.len() {
        let node = queue[o].clone();
        o += 1;
        let cur_dist = get(&node, "pathDist")?;
        let linked = get(&node, "linked")?;
        let node_type = get(&node, "type")?;
        let node_asc = get(&node, "ascendancyName")?;
        let mut index = 1;
        loop {
            let mut other = ipairs_at(&linked, index)?;
            if other.is_nil() {
                break;
            }
            // Cluster subgraph rebuilds can replace node objects while
            // retaining IDs: normalise stale links to the canonical node
            let id = get(&other, "id")?;
            let canonical = if id.truthy() {
                rt::index(&nodes, &id)?
            } else {
                id
            };
            if !canonical.truthy() {
                other = Value::Nil;
            } else if !canonical.raw_eq(&other) {
                rt::set_index(&linked, n(index as f64), canonical.clone())?;
                other = canonical;
            }
            index += 1;
            if other.is_nil() {
                continue;
            }
            let other_type = get(&other, "type")?;
            let other_dist = get(&other, "pathDist")?;
            let other_dist = if other_dist.truthy() {
                other_dist
            } else {
                n(1000.0)
            };
            let other_asc = get(&other, "ascendancyName")?;
            if !is(&node_type, "Mastery")
                && !is(&other_type, "ClassStart")
                && !is(&other_type, "AscendClassStart")
                && rt::lt(&cur_dist, &other_dist)?
                && (node_asc.raw_eq(&other_asc)
                    || (cur_dist.raw_eq(&n(0.0)) && !other_asc.truthy()))
            {
                // The shortest path to the other node is through this one
                let mut dist = cur_dist.clone();
                if !get(&other, "alloc")?.truthy() {
                    dist = rt::add(&dist, &n(1.0))?;
                }
                rt::set_index(&other, s("pathDist"), dist)?;
                let path = wipe_table(&get(&other, "path")?)?;
                rt::set_index(&other, s("path"), Value::Table(path.clone()))?;
                path.set(n(1.0), other.clone());
                let node_path = get(&node, "path")?;
                let mut i = 1;
                loop {
                    let p = ipairs_at(&node_path, i)?;
                    if p.is_nil() {
                        break;
                    }
                    path.set(n(i as f64 + 1.0), p);
                    i += 1;
                }
                queue.push(other);
            }
        }
    }
    Ok(())
}

/// `wipeTable(tbl)`: the same table emptied, or a new one for nil.
fn wipe_table(t: &Value) -> LResult<TableRef> {
    match t {
        Value::Nil | Value::Bool(false) => Ok(TableRef::new()),
        Value::Table(t) => {
            let mut b = t.borrow_mut();
            let meta = b.meta.take();
            *b = Table::new();
            b.meta = meta;
            drop(b);
            Ok(t.clone())
        }
        v => Err(LuaError::msg(format!(
            "bad argument #1 to 'pairs' (table expected, got {})",
            v.type_name()
        ))),
    }
}

/// Replaces the translated path finding of upstream's PassiveSpec class.
pub fn install(env: &Rc<Env>) -> LResult<()> {
    let spec = crate::class(env, "PassiveSpec")?;
    spec.set_str(
        "BuildPathFromNode",
        func("native BuildPathFromNode", |a| {
            let none = Value::Nil;
            build_path_from_node(a.first().unwrap_or(&none), a.get(1).unwrap_or(&none))?;
            Ok(vec![])
        }),
    );
    Ok(())
}
