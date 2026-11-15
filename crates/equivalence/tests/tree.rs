//! The typed passive tree (`engine::tree`) equals upstream's `PassiveTree`
//! on the oracle (`oracle/values.lua`'s `tree`) for every tree version the
//! corpus uses: every node's fields, position, links, stat parses and mods,
//! the radius lists of jewel sockets and keystones, and the tree's lookup
//! tables. Upstream builds the node list and the connectors in `pairs`
//! order over `tree.nodes`, whose string key `root` makes it follow
//! LuaJIT's per-process string hash, so both compare in id order,
//! connectors undirected and the class notables sorted.

use engine::mods::Mod;
use engine::mods::convert::Converter;
use engine::tree::{self, PassiveTree};
use equivalence::{load_data, lua_json, mod_parser_preloaded, typed_json, upstream_src};
use lua::rt;
use lua::{Key, Value};
use session::Session;
use std::collections::BTreeMap;

const VERSIONS: &[&str] = &["3_29", "3_13", "3_12", "3_6"];

fn s(x: &str) -> Value {
    Value::str(x)
}

fn get(v: &Value, k: &str) -> Value {
    rt::index(v, &s(k)).unwrap_or_default()
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_str_lossy().into_owned())
}

fn ids(v: &Value) -> Vec<u32> {
    v.as_table()
        .map(|t| t.entries())
        .unwrap_or_default()
        .into_iter()
        .map(|(k, _)| match k {
            Key::Int(i) => i as u32,
            k => panic!("node id {k:?}"),
        })
        .collect()
}

fn mods_json(list: &[Mod]) -> String {
    serde_json::Value::Array(list.iter().map(typed_json::mod_).collect()).to_string()
}

fn lua_mods(v: &Value) -> Result<Vec<Mod>, String> {
    let conv = Converter::default();
    v.as_table()
        .map(|t| t.array())
        .unwrap_or_default()
        .iter()
        .map(|m| conv.mod_(m))
        .collect()
}

#[test]
fn trees_match_upstream() {
    let oracle = Session::oracle(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let parser = mod_parser_preloaded();
    let mut im = importer::Importer::new(&upstream_src());
    let (data, _) = load_data();
    let mut failures = Vec::new();
    for version in VERSIONS {
        let req =
            serde_json::json!({"cmd": "oracleValue", "fn": "tree", "params": {"version": version}});
        let lua_tree = oracle
            .request(&req.to_string())
            .and_then(|r| lua_json::answer(&r))
            .unwrap_or_else(|e| panic!("LoadTree {version}: {e}"));
        let latest = lua::to_string(&get(&lua_tree, "latestTreeVersion")).to_string();
        let tree_data = im
            .load1(&format!("TreeData/{version}/tree"), vec![])
            .unwrap_or_else(|e| panic!("{version}: {e}"));
        let radii = tree::jewel_radii(&data.get("jewelRadii"), version).unwrap();
        let typed = PassiveTree::new(version, &tree_data, &radii, &parser, *version == latest)
            .unwrap_or_else(|e| panic!("{version}: {e}"));
        let mut fail = |what: String| {
            if failures.len() < 60 {
                failures.push(format!("{version}: {what}"));
            }
        };
        let lua_nodes = get(&lua_tree, "nodes");
        let mut lua_ids = ids(&lua_nodes);
        let mut typed_ids: Vec<u32> = typed.nodes.iter().map(|n| n.id).collect();
        lua_ids.sort();
        typed_ids.sort();
        if lua_ids != typed_ids {
            fail(format!(
                "nodes: {} upstream, {} typed",
                lua_ids.len(),
                typed_ids.len()
            ));
        }
        for node in &typed.nodes {
            let ln = rt::index(&lua_nodes, &Value::Number(node.id as f64)).unwrap();
            if ln.is_nil() {
                continue;
            }
            let id = node.id;
            let mut check = |field: &str, ok: bool, detail: String| {
                if !ok {
                    fail(format!("node {id} {field}: {detail}"));
                }
            };
            check(
                "dn",
                text(&get(&ln, "dn")).as_deref() == Some(&*node.dn),
                format!("{:?}", node.dn),
            );
            let lsd: Vec<String> = get(&ln, "sd")
                .as_table()
                .map(|t| t.array())
                .unwrap_or_default()
                .iter()
                .filter_map(text)
                .collect();
            let tsd: Vec<String> = node.sd.iter().map(|s| s.to_string()).collect();
            check("sd", lsd == tsd, format!("{tsd:?} != {lsd:?}"));
            check(
                "type",
                text(&get(&ln, "type")).as_deref() == Some(node.kind.name()),
                node.kind.name().into(),
            );
            check(
                "ascendancyName",
                text(&get(&ln, "ascendancyName"))
                    == node.ascendancy_name.map(|a| a.as_str().to_string()),
                format!("{:?}", node.ascendancy_name),
            );
            let llinked: Vec<u32> = get(&ln, "linkedId")
                .as_table()
                .map(|t| t.array())
                .unwrap_or_default()
                .iter()
                .map(|v| v.as_number().unwrap() as u32)
                .collect();
            check(
                "linkedId",
                llinked == node.linked_id,
                format!("{:?} != {llinked:?}", node.linked_id),
            );
            for (field, t) in [("x", node.x), ("y", node.y), ("angle", node.angle)] {
                let l = get(&ln, field).as_number();
                check(
                    field,
                    l.map(f64::to_bits) == t.map(f64::to_bits),
                    format!("{t:?} != {l:?}"),
                );
            }
            check(
                "icon",
                text(&get(&ln, "icon")).as_deref() == node.icon.as_deref(),
                format!("{:?}", node.icon),
            );
            check(
                "unknown",
                get(&ln, "unknown").truthy() == node.unknown,
                format!("{}", node.unknown),
            );
            check(
                "extra",
                get(&ln, "extra").truthy() == node.extra,
                format!("{}", node.extra),
            );
            // Stat line parses
            let lmods = get(&ln, "mods")
                .as_table()
                .map(|t| t.array())
                .unwrap_or_default();
            check(
                "mods",
                lmods.len() == node.mods.len(),
                format!("{} lines != {}", node.mods.len(), lmods.len()),
            );
            for (i, (lm, tm)) in lmods.iter().zip(&node.mods).enumerate() {
                let list = get(lm, "list");
                let lextra = text(&get(lm, "extra"));
                let ok = match (&tm.list, list.is_nil()) {
                    (None, true) => true,
                    (Some(t), false) => lua_mods(&list)
                        .map(|l| mods_json(&l) == mods_json(t))
                        .unwrap_or(false),
                    _ => false,
                };
                check(
                    &format!("mods[{}]", i + 1),
                    ok && lextra.as_deref() == tm.extra.as_deref(),
                    format!(
                        "typed {:?} list {:?}, upstream {lextra:?} list {}",
                        tm.extra,
                        tm.list.as_ref().map(|l| mods_json(l)),
                        if list.is_nil() {
                            "nil".to_string()
                        } else {
                            lua_mods(&list).map(|l| mods_json(&l)).unwrap_or_else(|e| e)
                        }
                    ),
                );
            }
            // Mods
            match lua_mods(&get(&ln, "modList")) {
                Ok(l) if mods_json(&l) == mods_json(&node.mod_list) => {}
                Ok(l) => check(
                    "modList",
                    false,
                    format!(
                        "\n    typed      {}\n    upstream {}",
                        mods_json(&node.mod_list),
                        mods_json(&l)
                    ),
                ),
                Err(e) => check("modList", false, e),
            }
            let lk = get(&ln, "keystoneMod");
            let lk = if lk.is_nil() {
                None
            } else {
                Some(Converter::default().mod_(&lk).unwrap())
            };
            check(
                "keystoneMod",
                lk.as_ref().map(|m| mods_json(std::slice::from_ref(m)))
                    == node
                        .keystone_mod
                        .as_ref()
                        .map(|m| mods_json(std::slice::from_ref(m))),
                format!("{:?}", node.keystone_mod),
            );
            // Jewel radii
            let lr = get(&ln, "nodesInRadius");
            // Keyed by node id upstream, so in no meaningful order
            let sorted = |v: Vec<Vec<u32>>| -> Vec<Vec<u32>> {
                v.into_iter()
                    .map(|mut r| {
                        r.sort();
                        r
                    })
                    .collect()
            };
            let lr: Option<Vec<Vec<u32>>> = lr
                .as_table()
                .map(|t| sorted(t.array().iter().map(ids).collect()));
            let tr = node.nodes_in_radius.clone().map(sorted);
            let sizes = |r: &Option<Vec<Vec<u32>>>| {
                r.as_ref()
                    .map(|r| r.iter().map(Vec::len).collect::<Vec<_>>())
            };
            check(
                "nodesInRadius",
                lr == tr,
                format!("typed {:?} upstream {:?}", sizes(&tr), sizes(&lr)),
            );
        }
        // Lookup tables
        let map = |name: &str| -> BTreeMap<String, u32> {
            get(&lua_tree, name)
                .as_table()
                .map(|t| t.entries())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(k, v)| match k {
                    Key::Str(k) => Some((
                        k.to_str_lossy().into_owned(),
                        get(&v, "id").as_number()? as u32,
                    )),
                    _ => None,
                })
                .collect()
        };
        let typed_map =
            |m: &std::collections::HashMap<std::sync::Arc<str>, u32>| -> BTreeMap<String, u32> {
                m.iter().map(|(k, v)| (k.to_string(), *v)).collect()
            };
        for (name, t) in [
            ("keystoneMap", &typed.keystone_map),
            ("notableMap", &typed.notable_map),
            ("ascendancyMap", &typed.ascendancy_map),
            ("clusterNodeMap", &typed.cluster_node_map),
        ] {
            let (l, t) = (map(name), typed_map(t));
            if l != t {
                let missing: Vec<_> = l
                    .iter()
                    .filter(|(k, v)| t.get(*k) != Some(v))
                    .take(5)
                    .collect();
                let extra: Vec<_> = t
                    .iter()
                    .filter(|(k, v)| l.get(*k) != Some(v))
                    .take(5)
                    .collect();
                fail(format!(
                    "{name}: upstream only {missing:?}, typed only {extra:?}"
                ));
            }
        }
        // Geometry: connectors (an arc split in two once), orbit radii, bounds
        let mut lconn: Vec<(u32, u32)> = Vec::new();
        for c in get(&lua_tree, "connectors")
            .as_table()
            .map(|t| t.array())
            .unwrap_or_default()
        {
            let id = |k: &str| get(&c, k).as_number().unwrap() as u32;
            let pair = (id("nodeId1"), id("nodeId2"));
            if lconn.last() != Some(&pair) {
                lconn.push(pair);
            }
        }
        let undirected = |c: &[(u32, u32)]| {
            let mut c: Vec<(u32, u32)> = c.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect();
            c.sort();
            c
        };
        if undirected(&lconn) != undirected(&typed.connectors) {
            fail(format!(
                "connectors: {} typed, {} upstream",
                typed.connectors.len(),
                lconn.len()
            ));
        }
        let lradii: Vec<f64> = get(&lua_tree, "orbitRadii")
            .as_table()
            .map(|t| t.array())
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_number)
            .collect();
        if lradii != typed.orbit_radii {
            fail(format!("orbitRadii {:?} != {lradii:?}", typed.orbit_radii));
        }
        let lbounds = ["min_x", "min_y", "max_x", "max_y"].map(|k| get(&lua_tree, k).as_number());
        if lbounds != typed.bounds.map(Some) {
            fail(format!("bounds {:?} != {lbounds:?}", typed.bounds));
        }
        let mut lsockets = ids(&get(&lua_tree, "sockets"));
        let mut tsockets = typed.sockets.clone();
        lsockets.sort();
        tsockets.sort();
        if lsockets != tsockets {
            fail("sockets differ".into());
        }
        // Class notables and class starts
        for (_, class) in &typed.classes {
            let ci = rt::index(&get(&lua_tree, "classNameMap"), &s(&class.name)).unwrap();
            let lc = rt::index(&get(&lua_tree, "classes"), &ci).unwrap();
            let start = get(&lc, "startNodeId").as_number().map(|n| n as u32);
            if start != class.start_node_id {
                fail(format!(
                    "class {} start {:?} != {start:?}",
                    class.name, class.start_node_id
                ));
            }
        }
        // Gathered in `pairs` order over `tree.nodes`, so compared sorted
        let lnot = get(&lua_tree, "classNotables");
        for (class, list) in &typed.class_notables {
            let mut l: Vec<String> = get(&lnot, class)
                .as_table()
                .map(|t| t.array())
                .unwrap_or_default()
                .iter()
                .filter_map(text)
                .collect();
            let mut t: Vec<String> = list.iter().map(|s| s.to_string()).collect();
            l.sort();
            t.sort();
            if l != t {
                fail(format!("classNotables {class}: {t:?} != {l:?}"));
            }
        }
        // Mastery effects
        let lme = get(&lua_tree, "masteryEffects");
        for effect in typed.mastery_effects.values() {
            let le = rt::index(&lme, &Value::Number(effect.id)).unwrap();
            match lua_mods(&get(&le, "modList")) {
                Ok(l) if mods_json(&l) == mods_json(&effect.mod_list) => {}
                _ => fail(format!("masteryEffects[{}].modList", effect.id)),
            }
        }
        eprintln!("{version}: {} nodes", typed.nodes.len());
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
