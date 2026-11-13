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
use equivalence::{corpus, load_data, mod_parser_preloaded, typed_json, upstream_src};
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
    let parser = mod_parser_preloaded();
    let mut im = importer::Importer::new(&upstream_src());
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
            if std::env::var_os("SPEC_DEBUG").is_some() {
                eprintln!(
                    "{id} spec {}: translated tree {:?} class {:?} ascend {:?}, xml {:?} {:?} {:?}, {} specs in xml, {} translated",
                    si + 1,
                    get(&lua_spec, "treeVersion"),
                    get(&lua_spec, "curClassId"),
                    get(&lua_spec, "curAscendClassId"),
                    a.get("treeVersion"),
                    a.get("classId"),
                    a.get("ascendClassId"),
                    attrs.len(),
                    spec_list.as_table().map(|t| t.len()).unwrap_or(0),
                );
            }
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
            let effects = engine::spec::mastery_effects(a.get("masteryEffects").map_or("", |s| s));
            let num = |k: &str| a.get(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
            let mut spec = PassiveSpec::new(tree, &parser).unwrap();
            spec.import_from_node_list(
                num("classId"),
                num("ascendClassId"),
                Some(num("secondaryAscendClassId")),
                &hash_list,
                &effects,
            )
            .unwrap_or_else(|e| panic!("{id} spec {si}: {e}"));
            spec.post_load()
                .unwrap_or_else(|e| panic!("{id} spec {si}: {e}"));
            compare_spec(&spec, &lua_spec, &mut |what| {
                if failures.len() < 60 {
                    failures.push(format!("{id} spec {}: {what}", si + 1));
                }
            });
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

/// A build with a jewel socketed in an allocated socket of its active spec
/// and `extra` nodes allocated
fn with_jewel(
    xml: &str,
    text: &str,
    variant: Option<u32>,
    socket: u32,
    extra: &[u32],
) -> Option<String> {
    let id = engine::pat::gmatch(xml, "<Item id=\"(%d+)\"")
        .iter()
        .filter_map(|c| c[0].parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let at = xml.find("<ItemSet ")?;
    let v = variant.map_or(String::new(), |v| format!(" variant=\"{v}\""));
    let mut x = format!(
        "{}<Item id=\"{id}\"{v}>\nRarity: UNIQUE\n{text}\n</Item>\n{}",
        &xml[..at],
        &xml[at..]
    );
    let spec = x.find("<Spec ")?;
    let nodes = spec + x[spec..].find("nodes=\"")? + 7;
    let extra: String = extra.iter().map(|n| format!("{n},")).collect();
    x.insert_str(nodes, &extra);
    let spec_end = spec + x[spec..].find("</Spec>")?;
    let socket_tag = format!("nodeId=\"{socket}\"/>");
    match x[spec..spec_end].find(&socket_tag) {
        Some(k) => {
            let k = spec + k;
            let start = x[..k].rfind("<Socket ")?;
            x.replace_range(
                start..k + socket_tag.len(),
                &format!("<Socket itemId=\"{id}\" {socket_tag}"),
            );
        }
        None => {
            let s_at = spec + x[spec..spec_end].find("<Sockets>")? + 9;
            x.insert_str(s_at, &format!("\n<Socket itemId=\"{id}\" {socket_tag}"));
        }
    }
    Some(x)
}

/// Specs with jewels that let nodes be allocated without a connection
/// (Intuitive Leap, Thread of Hope, Impossible Escape), socketed in an
/// allocated socket of generated corpus builds with unconnected nodes in
/// their radius allocated, loaded with their items
#[test]
fn jewel_specs_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = equivalence::typed_build::TypedData::load();
    let jewels: [(&str, &str, Option<u32>); 4] = [
        (
            "il",
            "Intuitive Leap\nViridian Jewel\nRadius: Small\nPassive Skills in Radius can be Allocated without being connected to your tree",
            None,
        ),
        (
            "toh",
            "Thread of Hope\nCrimson Jewel\nVariant: Small Ring\nVariant: Medium Ring\nVariant: Large Ring\nVariant: Very Large Ring\nVariant: Massive Ring (Uber)\nRadius: Variable\nImplicits: 0\n{variant:1}Only affects Passives in Small Ring\n{variant:2}Only affects Passives in Medium Ring\n{variant:3}Only affects Passives in Large Ring\n{variant:4}Only affects Passives in Very Large Ring\n{variant:5}Only affects Passives in Massive Ring\nPassive Skills in Radius can be Allocated without being connected to your tree",
            Some(3),
        ),
        (
            "ie",
            "Impossible Escape\nViridian Jewel\nLimited to: 1\nRadius: Small\nPassives in Radius of Iron Reflexes can be Allocated without being connected to your tree",
            None,
        ),
        (
            "kl",
            "Unnatural Instinct\nViridian Jewel\nRadius: Small\nAllocated Small Passive Skills in Radius grant nothing\nGrants all bonuses of Unallocated Small Passive Skills in Radius",
            None,
        ),
    ];
    let mut failures = Vec::new();
    let mut compared = 0;
    for (n, (bid, xml)) in equivalence::corpus_builds().into_iter().enumerate() {
        if !bid.starts_with("gen-") || n % 4 != 0 {
            continue;
        }
        let root = engine::xml::parse(&xml).unwrap();
        data.load_tree(&root);
        let Ok(build) = data.load_build(&root) else {
            continue;
        };
        let spec = &build.spec;
        let tree = spec.tree;
        // An allocated, non-cluster socket
        let Some(socket) = tree.sockets.iter().copied().find(|&s| {
            spec.alloc_nodes.contains(s)
                && tree.node(s).is_some_and(|t| t.expansion_jewel.is_none())
        }) else {
            continue;
        };
        for (k, (name, text, variant)) in jewels.iter().enumerate() {
            let id = format!("{bid}-{name}");
            if !id.contains(&filter) {
                continue;
            }
            // Unallocated nodes in the radius: the socket's, or the keystone's
            let centre = if *name == "ie" {
                tree.keystone_map.get("Iron Reflexes").copied().unwrap()
            } else {
                socket
            };
            let ring = if *name == "toh" { 7 } else { 0 };
            let extra: Vec<u32> = tree
                .node(centre)
                .and_then(|c| c.nodes_in_radius.as_ref())
                .map(|r| r[ring].clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|&i| {
                    !spec.alloc_nodes.contains(i)
                        && tree.node(i).is_some_and(|t| {
                            matches!(
                                t.kind,
                                engine::tree::NodeType::Normal | engine::tree::NodeType::Notable
                            ) && t.ascendancy_name.is_none()
                        })
                })
                .skip(k)
                .step_by(3)
                .take(4)
                .collect();
            let Some(v) = with_jewel(&xml, text, *variant, socket, &extra) else {
                continue;
            };
            host.load_build_xml(v.as_bytes(), &id)
                .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
            let lua_build = host.build().unwrap();
            let lua_spec = get(&lua_build, "spec");
            let root = engine::xml::parse(&v).unwrap();
            let typed = match data.load_build(&root) {
                Ok(b) => b,
                Err(e) => {
                    failures.push(format!("{id}: {e}"));
                    continue;
                }
            };
            compared += 1;
            compare_spec(&typed.spec, &lua_spec, &mut |what| {
                if failures.len() < 60 {
                    failures.push(format!("{id}: {what}"));
                }
            });
        }
    }
    eprintln!("{compared} specs compared");
    assert!(compared > 0, "no spec compared");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Specs with tattoos (`equivalence::variants::tattoo_variants`), loaded
/// with their builds
#[test]
fn tattoo_specs_match_upstream() {
    variant_specs_match(&equivalence::variants::tattoo_variants);
}

/// Specs with timeless jewels (`equivalence::variants::timeless_variants`)
#[test]
fn timeless_specs_match_upstream() {
    variant_specs_match(&equivalence::variants::timeless_variants);
}

fn variant_specs_match(
    make: &dyn Fn(&mut equivalence::typed_build::TypedData) -> Vec<(String, String)>,
) {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    equivalence::install_inflate(&host);
    let mut data = equivalence::typed_build::TypedData::load();
    let mut failures = Vec::new();
    let (mut compared, mut changed) = (0, 0);
    for (id, v) in make(&mut data) {
        if !id.contains(&filter) {
            continue;
        }
        host.load_build_xml(v.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let lua_spec = get(&host.build().unwrap(), "spec");
        let root = engine::xml::parse(&v).unwrap();
        let typed = match data.load_build(&root) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{id}: {e}"));
                continue;
            }
        };
        compared += 1;
        changed += typed
            .spec
            .nodes
            .iter()
            .filter(|n| n.conquered.is_some() && n.overlay.is_some())
            .count();
        compare_spec(&typed.spec, &lua_spec, &mut |what| {
            if failures.len() < 60 {
                failures.push(format!("{id}: {what}"));
            }
        });
    }
    eprintln!("{compared} specs compared, {changed} nodes replaced by timeless jewels");
    assert!(compared > 0, "no spec compared");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Compares a typed spec with the translated one
fn compare_spec(spec: &PassiveSpec, lua_spec: &Value, fail: &mut dyn FnMut(String)) {
    // Allocated nodes, in pairs order
    let lalloc: Vec<u32> = get(lua_spec, "allocNodes")
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
    let lnodes = get(lua_spec, "nodes");
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
        let till: Vec<u32> = node.intuitive.iter().map(|&d| spec.nodes[d].id).collect();
        let lill = ids(&get(&ln, "intuitiveLeapLikesAffecting"));
        check(
            "intuitiveLeapLikesAffecting",
            till == lill,
            format!("{till:?} != {lill:?}"),
        );
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
        // Upstream's modLib.mergeKeystones sets the source of a tree
        // keystone's own mods to whatever else grants the keystone (an
        // item, the config), so after a calculation they name that
        let lmods = lmods.map(|mut l| {
            if spec.tree_node(i).kind == engine::tree::NodeType::Keystone {
                for m in &mut l {
                    if m.source
                        .as_deref()
                        .is_some_and(|s| !s.to_lowercase().contains("tree"))
                    {
                        let tree_source: std::sync::Arc<str> = format!("Tree:{}", node.id).into();
                        m.source = Some(tree_source.clone());
                        // setSource rewrites a carried mod's source too
                        if let engine::mods::ModValue::Record(r) = &mut m.value
                            && let Some(engine::mods::ModValue::Mod(inner)) =
                                r.get_mut(engine::mods::Sym::new("mod"))
                        {
                            inner.source = Some(tree_source);
                        }
                    }
                }
            }
            l
        });
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
    let lsel: Vec<(u32, f64)> = get(lua_spec, "masterySelections")
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
        let l = get(lua_spec, field).as_number();
        if l != Some(t as f64) {
            fail(format!("{field} {t} != {l:?}"));
        }
    }
    for (field, t) in [
        ("curClassId", spec.cur_class_id),
        ("curAscendClassId", spec.cur_ascend_class_id),
    ] {
        let l = get(lua_spec, field).as_number().map(|n| n as i64);
        if l != t {
            fail(format!("{field} {t:?} != {l:?}"));
        }
    }
}
