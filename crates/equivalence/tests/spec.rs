//! The typed passive spec (`engine::spec`) equals the translated program's
//! `PassiveSpec` for every spec of every corpus build loaded from XML: each
//! node's allocation, dependencies, connection to the start, path and
//! distance, its stat lines and mods, the mastery selections and counts, and
//! the order of the allocated nodes.
//!
//! Specs with jewels that change the tree (Intuitive Leap and the like,
//! timeless jewels, Impossible Escape, Split Personality), tattoos or
//! cluster jewel subgraphs are not ported yet; they are skipped and counted.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::spec::PassiveSpec;
use engine::tree::{self, PassiveTree};
use equivalence::{corpus, load_data, mod_parser, typed_json, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
use std::collections::HashMap;
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

/// The attributes of every `<Spec>` element, in order
fn spec_attrs(xml: &str) -> Vec<HashMap<String, String>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<Spec ") {
        rest = &rest[i + 6..];
        let end = rest.find('>').unwrap();
        let tag = &rest[..end];
        let mut attrs = HashMap::new();
        let mut t = tag;
        while let Some(eq) = t.find("=\"") {
            let name = t[..eq].trim().to_string();
            let after = &t[eq + 2..];
            let close = after.find('"').unwrap();
            attrs.insert(name, after[..close].replace("&amp;", "&"));
            t = &after[close + 1..];
        }
        out.push(attrs);
    }
    out
}

fn numbers(s: &str) -> Vec<f64> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().unwrap())
        .collect()
}

/// Whether the spec uses what the typed spec does not port yet
fn unsupported(lua_spec: &Value, build: &Value) -> Option<&'static str> {
    if get(lua_spec, "hashOverrides")
        .as_table()
        .is_some_and(|t| !t.is_empty())
    {
        return Some("tattoos");
    }
    if get(lua_spec, "subGraphs")
        .as_table()
        .is_some_and(|t| !t.is_empty())
    {
        return Some("cluster jewels");
    }
    let items = get(&get(build, "itemsTab"), "items");
    for (_, item_id) in get(lua_spec, "jewels")
        .as_table()
        .map(|t| t.entries())
        .unwrap_or_default()
    {
        let item = rt::index(&items, &item_id).unwrap();
        let jd = get(&item, "jewelData");
        for key in [
            "intuitiveLeapLike",
            "conqueredBy",
            "impossibleEscapeKeystone",
            "jewelIncEffectFromClassStart",
        ] {
            if get(&jd, key).truthy() {
                return Some("jewels that change the tree");
            }
        }
    }
    None
}

#[test]
fn specs_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let (data, _) = load_data();
    let latest = lua::to_string(&host.global("latestTreeVersion")).to_string();
    let mut trees: HashMap<String, PassiveTree> = HashMap::new();
    let mut failures = Vec::new();
    let mut skipped: HashMap<&str, usize> = HashMap::new();
    let mut compared = 0;
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(text.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let spec_list = get(&get(&build, "treeTab"), "specList");
        let attrs = spec_attrs(&text);
        for (si, a) in attrs.iter().enumerate() {
            let lua_spec = rt::index(&spec_list, &Value::Number(si as f64 + 1.0)).unwrap();
            if let Some(why) = unsupported(&lua_spec, &build) {
                *skipped.entry(why).or_default() += 1;
                continue;
            }
            compared += 1;
            let version = a
                .get("treeVersion")
                .cloned()
                .unwrap_or_else(|| "2_6".into());
            if !trees.contains_key(&version) {
                let tree_data = im
                    .load1(&format!("TreeData/{version}/tree"), vec![])
                    .unwrap_or_else(|e| panic!("{version}: {e}"));
                let radii = tree::jewel_radii(&data.get("jewelRadii"), &version).unwrap();
                let t = PassiveTree::new(&version, &tree_data, &radii, &parser, version == latest)
                    .unwrap_or_else(|e| panic!("{version}: {e}"));
                trees.insert(version.clone(), t);
            }
            let tree = &trees[&version];
            let hash_list: Vec<u32> = numbers(a.get("nodes").map_or("", |s| s))
                .iter()
                .map(|n| *n as u32)
                .collect();
            let effects: Vec<(u32, f64)> = numbers(a.get("masteryEffects").map_or("", |s| s))
                .chunks(2)
                .map(|p| (p[0] as u32, p[1]))
                .collect();
            let num = |k: &str| a.get(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
            let mut spec = PassiveSpec::new(tree, &parser).unwrap();
            spec.import_from_node_list(
                num("classId"),
                num("ascendClassId"),
                num("secondaryAscendClassId"),
                &hash_list,
                &effects,
            )
            .unwrap_or_else(|e| panic!("{id} spec {si}: {e}"));
            let mut fail = |what: String| {
                if failures.len() < 60 {
                    failures.push(format!("{id} spec {}: {what}", si + 1));
                }
            };
            // Allocated nodes, in pairs order
            let lalloc: Vec<u32> = get(&lua_spec, "allocNodes")
                .as_table()
                .map(|t| t.entries())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(k, _)| match k {
                    Key::Int(i) => Some(i as u32),
                    _ => None,
                })
                .collect();
            let talloc: Vec<u32> = spec.alloc_nodes.iter().map(|(k, _)| k).collect();
            if lalloc != talloc {
                fail(format!(
                    "allocNodes: typed {} {talloc:?}\n  translated {} {lalloc:?}",
                    talloc.len(),
                    lalloc.len()
                ));
            }
            let lnodes = get(&lua_spec, "nodes");
            let ids = |v: &Value| -> Vec<u32> {
                v.as_table()
                    .map(|t| t.array())
                    .unwrap_or_default()
                    .iter()
                    .map(|n| get(n, "id").as_number().unwrap() as u32)
                    .collect()
            };
            for (i, node) in spec.nodes.iter().enumerate() {
                let ln = rt::index(&lnodes, &Value::Number(node.id as f64)).unwrap();
                if ln.is_nil() {
                    fail(format!("node {} missing in the translated spec", node.id));
                    continue;
                }
                let nid = node.id;
                let mut check = |field: &str, ok: bool, detail: String| {
                    if !ok {
                        fail(format!("node {nid} {field}: {detail}"));
                    }
                };
                check(
                    "alloc",
                    get(&ln, "alloc").truthy() == node.alloc,
                    format!("{}", node.alloc),
                );
                let tdeps: Vec<u32> = node.depends.iter().map(|&d| spec.nodes[d].id).collect();
                let ldeps = ids(&get(&ln, "depends"));
                check("depends", tdeps == ldeps, format!("{tdeps:?} != {ldeps:?}"));
                if node.alloc {
                    check(
                        "connectedToStart",
                        get(&ln, "connectedToStart").truthy() == node.connected_to_start,
                        format!("{}", node.connected_to_start),
                    );
                }
                let ld = get(&ln, "pathDist").as_number();
                check(
                    "pathDist",
                    ld == Some(node.path_dist),
                    format!("{} != {ld:?}", node.path_dist),
                );
                let lp = get(&ln, "path");
                let lp = if lp.is_nil() { None } else { Some(ids(&lp)) };
                let tp = spec
                    .path(i)
                    .map(|p| p.iter().map(|&n| spec.nodes[n].id).collect::<Vec<_>>());
                check("path", lp == tp, format!("{tp:?} != {lp:?}"));
                if let Some(d) = node.distance_to_class_start {
                    let l = get(&ln, "distanceToClassStart").as_number();
                    check(
                        "distanceToClassStart",
                        l == Some(d),
                        format!("{d} != {l:?}"),
                    );
                }
                let lsd: Vec<String> = get(&ln, "sd")
                    .as_table()
                    .map(|t| t.array())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_str_lossy().into_owned()))
                    .collect();
                let tsd: Vec<String> = spec.sd(i).iter().map(|s| s.to_string()).collect();
                check("sd", lsd == tsd, format!("{tsd:?} != {lsd:?}"));
                let conv = engine::mods::convert::Converter::default();
                let lmods: Result<Vec<_>, _> = get(&ln, "modList")
                    .as_table()
                    .map(|t| t.array())
                    .unwrap_or_default()
                    .iter()
                    .map(|m| conv.mod_(m))
                    .collect();
                let enc = |l: &[engine::mods::Mod]| {
                    serde_json::Value::Array(l.iter().map(typed_json::mod_).collect()).to_string()
                };
                match lmods {
                    Ok(l) if enc(&l) == enc(spec.mod_list(i)) => {}
                    Ok(l) => check(
                        "modList",
                        false,
                        format!(
                            "\n    typed      {}\n    translated {}",
                            enc(spec.mod_list(i)),
                            enc(&l)
                        ),
                    ),
                    Err(e) => check("modList", false, e),
                }
            }
            // Mastery selections and counts
            let lsel: Vec<(u32, f64)> = get(&lua_spec, "masterySelections")
                .as_table()
                .map(|t| t.entries())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(k, v)| match k {
                    Key::Int(i) => Some((i as u32, v.as_number()?)),
                    _ => None,
                })
                .collect();
            let tsel: Vec<(u32, f64)> = spec
                .mastery_selections
                .iter()
                .map(|(k, v)| (k, *v))
                .collect();
            if lsel != tsel {
                fail(format!("masterySelections {tsel:?} != {lsel:?}"));
            }
            for (field, t) in [
                ("allocatedMasteryCount", spec.allocated_mastery_count),
                ("allocatedNotableCount", spec.allocated_notable_count),
                ("allocatedKeystoneCount", spec.allocated_keystone_count),
                (
                    "allocatedMasteryTypeCount",
                    spec.allocated_mastery_type_count,
                ),
            ] {
                let l = get(&lua_spec, field).as_number();
                if l != Some(t as f64) {
                    fail(format!("{field} {t} != {l:?}"));
                }
            }
            for (field, t) in [
                ("curClassId", spec.cur_class_id),
                ("curAscendClassId", spec.cur_ascend_class_id),
            ] {
                let l = get(&lua_spec, field).as_number().map(|n| n as i64);
                if l != t {
                    fail(format!("{field} {t:?} != {l:?}"));
                }
            }
        }
    }
    eprintln!("{compared} specs compared, skipped: {skipped:?}");
    assert!(compared > 0, "no spec compared");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
