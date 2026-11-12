//! Node power: `engine::power::node_power` gives the powers the translated
//! `CalcsTab:PowerBuilder()` leaves on every node of the spec, on a sample
//! of the cluster notables and in `powerMax`, for a sample of corpus builds
//! and power stats, with the nodes up to a depth of one point
//! (`nodePowerMaxDepth`). Builds that reach a part not ported yet are
//! skipped and listed.
//!
//! `CALCS_FILTER=<substring>` limits the builds run, and runs each of them;
//! `POWER_FULL=1` calculates every node and cluster notable, for timings.

use engine::calculator::MiscCalculator;
use engine::power::{NodePowers, Power, PowerOptions, cluster_nodes, node_power, power_stats};
use equivalence::outputs::{compare, entries, number};
use equivalence::typed_build::TypedData;
use equivalence::{corpus_builds, upstream_src};
use lua::{LuaError, LuaStr, TableRef, Value, rt};
use serde_json::{Map, Value as J};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;
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

fn key(n: f64) -> String {
    lua::format::format_g(n, 14)
}

fn power_json(p: &Power, m: &mut Map<String, J>) {
    for (k, v) in [
        ("singleStat", p.single_stat),
        ("pathPower", p.path_power),
        ("offence", p.offence),
        ("defence", p.defence),
    ] {
        if let Some(v) = v {
            m.insert(k.into(), number(v));
        }
    }
}

fn typed_json(p: &NodePowers) -> BTreeMap<String, J> {
    let mut out = BTreeMap::new();
    for (id, np) in &p.nodes {
        let mut m = Map::new();
        power_json(&np.power, &mut m);
        if let Some(d) = np.distance {
            m.insert("distance".into(), number(d));
        }
        if let Some(effects) = &np.mastery_effects {
            let e = effects
                .iter()
                .map(|(e, p)| {
                    let mut m = Map::new();
                    power_json(p, &mut m);
                    (key(*e), J::Object(m))
                })
                .collect();
            m.insert("masteryEffects".into(), J::Object(e));
        }
        out.insert(format!("node {id}"), J::Object(m));
    }
    for (id, p) in &p.cluster {
        let mut m = Map::new();
        power_json(p, &mut m);
        out.insert(format!("cluster {id}"), J::Object(m));
    }
    let m = &p.max;
    for (k, v) in [
        ("singleStat", m.single_stat),
        ("offence", m.offence),
        ("offencePerPoint", m.offence_per_point),
        ("defence", m.defence),
        ("defencePerPoint", m.defence_per_point),
    ] {
        out.insert(format!("max {k}"), number(v));
    }
    out
}

fn lua_power(p: &Value) -> Map<String, J> {
    let mut m = Map::new();
    for (k, v) in entries(p) {
        let k = equivalence::outputs::key_str(&k);
        let j = match &v {
            Value::Number(n) => number(*n),
            Value::Table(_) => J::Object(
                entries(&v)
                    .into_iter()
                    .map(|(e, p)| {
                        let e = e.to_value().as_number().map_or("?".into(), key);
                        (e, J::Object(lua_power(&p)))
                    })
                    .collect(),
            ),
            v => J::String(lua::to_string(v).to_str_lossy().into_owned()),
        };
        m.insert(k, j);
    }
    m
}

/// The sampled builds, each with a power stat by label
fn sample() -> Vec<(String, String, &'static str)> {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let stats = ["Offence/Defence", "Full DPS", "Life", "Taken Fire dmg"];
    corpus_builds()
        .into_iter()
        .enumerate()
        .filter(|(i, (id, _))| {
            if filter.is_empty() {
                i % 10 == 3
            } else {
                id.contains(&filter)
            }
        })
        .enumerate()
        .map(|(n, (_, (id, xml)))| (id, xml, stats[n % stats.len()]))
        .collect()
}

#[test]
fn node_power_matches_upstream() {
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = TypedData::load();
    let stats = power_stats();
    let mut failures = Vec::new();
    let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut compared, mut powered, mut t_typed, mut t_lua) = (0, 0, 0.0, 0.0);
    for (id, xml, label) in sample() {
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let lbuild = host.build().unwrap();
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
        let stat = stats.iter().find(|s| s.label == label).unwrap().clone();
        let full = std::env::var_os("POWER_FULL").is_some();
        let cluster: Vec<u32> = cluster_nodes(build.spec.tree)
            .into_iter()
            .take(if full { usize::MAX } else { 3 })
            .collect();
        let depth = (!full).then_some(1.0);
        let opts = PowerOptions {
            stat: Some(stat),
            max_depth: depth,
            cluster_nodes: cluster.clone(),
        };
        let start = Instant::now();
        let got = engine::calcs::build_outputs(&mut build, &calc_data)
            .and_then(|_| MiscCalculator::new(&build, &calc_data))
            .and_then(|c| node_power(&c, &opts));
        let typed_time = start.elapsed().as_secs_f64();
        let got = match got {
            Ok(p) => p,
            Err(e) if e.starts_with("not ported") => {
                skips.entry(e).or_default().push(id.clone());
                continue;
            }
            Err(e) => {
                failures.push(format!("{id}: typed node power failed: {e}"));
                continue;
            }
        };

        // The translated power builder on the same stat, depth and
        // cluster notables
        let calcs_tab = get(&lbuild, "calcsTab");
        let lstat = entries(&get(&host.global("data"), "powerStatList"))
            .into_iter()
            .map(|(_, v)| v)
            .find(|v| lua::to_string(&get(v, "label")).to_str_lossy() == label)
            .unwrap();
        let tab = calcs_tab.as_table().unwrap();
        tab.set_str("powerStat", lstat);
        tab.set_str("nodePowerMaxDepth", depth.map_or(Value::Nil, Value::Number));
        let ltree = get(&get(&lbuild, "spec"), "tree");
        let full_map = get(&ltree, "clusterNodeMap");
        let lnodes = get(&ltree, "nodes");
        let sample_map = TableRef::new();
        for &cid in &cluster {
            let n = rt::index(&lnodes, &Value::Number(cid as f64)).unwrap();
            sample_map.set(get(&n, "dn"), n);
        }
        let ltree_t = ltree.as_table().unwrap();
        ltree_t.set_str("clusterNodeMap", Value::Table(sample_map));
        let lua_before = t_lua;
        let start = Instant::now();
        let r = rt::method(&calcs_tab, &LuaStr::from("PowerBuilder"), &[]);
        t_lua += start.elapsed().as_secs_f64();
        ltree_t.set_str("clusterNodeMap", full_map);
        if let Err(e) = r {
            failures.push(format!(
                "{id}: translated PowerBuilder failed: {}",
                describe(&e)
            ));
            continue;
        }
        t_typed += typed_time;
        let mut want = BTreeMap::new();
        let lspec_nodes = get(&get(&lbuild, "spec"), "nodes");
        for (id, _) in &got.nodes {
            let n = rt::index(&lspec_nodes, &Value::Number(*id as f64)).unwrap();
            want.insert(
                format!("node {id}"),
                J::Object(lua_power(&get(&n, "power"))),
            );
        }
        for &cid in &cluster {
            let n = rt::index(&lnodes, &Value::Number(cid as f64)).unwrap();
            want.insert(
                format!("cluster {cid}"),
                J::Object(lua_power(&get(&n, "power"))),
            );
        }
        let max = get(&calcs_tab, "powerMax");
        for k in [
            "singleStat",
            "offence",
            "offencePerPoint",
            "defence",
            "defencePerPoint",
        ] {
            want.insert(
                format!("max {k}"),
                number(get(&max, k).as_number().unwrap_or(f64::NAN)),
            );
        }
        compared += 1;
        powered += got
            .nodes
            .iter()
            .filter(|(_, p)| p.power.single_stat.is_some())
            .count();
        eprintln!(
            "{id}: typed {typed_time:.2}s, translated {:.2}s",
            t_lua - lua_before
        );
        let mut fail = Vec::new();
        compare(label, &typed_json(&got), &want, &mut fail);
        if !fail.is_empty() {
            let n = fail.len();
            fail.truncate(30);
            failures.push(format!(
                "{id}: {n} differences\n    {}",
                fail.join("\n    ")
            ));
        }
    }
    let skipped: usize = skips.values().map(Vec::len).sum();
    eprintln!(
        "{compared} builds, {powered} nodes with power compared, {skipped} skipped: {skips:?}; typed {t_typed:.1}s (with the build's calculations), translated {t_lua:.1}s"
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
