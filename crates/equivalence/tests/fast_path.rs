//! Fast path against simple path (`docs/design.md`, layer 7): the
//! calculations that copy one setup for every main skill
//! (`engine::calc_env::reuse_setup`) give exactly what setting each up
//! afresh gives: the sidebar's and Calcs tab's outputs, the calculator's
//! with overrides and Full DPS, and node power by Full DPS, for a sample
//! of corpus builds. Node power on threads equals node power on one,
//! calculated as needed. The Full DPS skills on threads give what
//! calculating them in order gives, for every corpus build.
//!
//! `CALCS_FILTER=<substring>` limits the builds run, and runs each of them.

use engine::calc_env::set_reuse_setup;
use engine::calc_override::Override;
use engine::calculator::{CalcOutput, MiscCalculator};
use engine::parallel::set_full_dps_threads;
use engine::power::{PowerOptions, cluster_nodes, node_power, node_power_with, power_stats};
use equivalence::corpus_builds;
use equivalence::outputs::typed_outputs;
use equivalence::typed_build::TypedData;

/// Everything the build's calculations give, as text that is equal only
/// when every number is
fn results(data: &TypedData, xml: &str) -> Result<Vec<String>, String> {
    let root = engine::xml::parse(xml)?;
    let mut build = data.load_build(&root)?;
    let calc_data = data.calc_data();
    let outs = engine::calcs::build_outputs(&mut build, &calc_data)?;
    let show = |o: &CalcOutput| {
        format!(
            "{:?} {:?}",
            typed_outputs(&o.player),
            o.minion.as_ref().map(typed_outputs)
        )
    };
    let mut r = vec![
        format!("{:?}", typed_outputs(&outs.main.player)),
        format!("{:?}", outs.main.minion.as_ref().map(typed_outputs)),
        format!("{:?}", outs.main.full_dps),
        format!(
            "{:?}",
            outs.calcs.as_ref().map(|c| typed_outputs(&c.player))
        ),
    ];
    let calc = MiscCalculator::new(&build, &calc_data)?;
    r.push(show(calc.base()));
    let spec = &calc.build().spec;
    let nodes = (0..spec.nodes.len())
        .filter(|&i| !spec.nodes[i].alloc && spec.nodes[i].path_dist == 1.0)
        .take(2);
    for i in nodes {
        r.push(show(
            &calc.calculate(&Override::default().add_node(i), true)?,
        ));
    }
    let stat = power_stats().into_iter().find(|s| s.label == "Full DPS");
    let powers = node_power(
        &calc,
        &PowerOptions {
            stat,
            max_depth: Some(1.0),
            cluster_nodes: vec![],
        },
    )?;
    r.push(format!("{:?} {:?}", powers.nodes, powers.max));
    Ok(r)
}

#[test]
fn reused_setup_equals_fresh_setup() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let mut data = TypedData::load();
    let (mut compared, mut failures) = (0, Vec::new());
    for (i, (id, xml)) in corpus_builds().into_iter().enumerate() {
        if !(if filter.is_empty() {
            i % 8 == 1
        } else {
            id.contains(&filter)
        }) {
            continue;
        }
        data.load_tree(&engine::xml::parse(&xml).unwrap());
        set_reuse_setup(true);
        let fast = results(&data, &xml);
        set_reuse_setup(false);
        let simple = results(&data, &xml);
        set_reuse_setup(true);
        match (fast, simple) {
            (Ok(f), Ok(s)) => {
                compared += 1;
                if let Some(k) = (0..f.len()).find(|&k| f[k] != s[k]) {
                    failures.push(format!("{id}: result {k} differs"));
                }
            }
            (Err(f), Err(s)) if f == s => {}
            (f, s) => failures.push(format!("{id}: fast {:?}, simple {:?}", f.err(), s.err())),
        }
    }
    eprintln!("{compared} builds compared");
    assert!(compared > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Node power with its calculations made ahead on 1 and 4 threads gives
/// what calculating each when needed gives, in the same order
#[test]
fn parallel_node_power_equals_sequential() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let mut data = TypedData::load();
    let (mut compared, mut failures) = (0, Vec::new());
    for (i, (id, xml)) in corpus_builds().into_iter().enumerate() {
        if !(if filter.is_empty() {
            i % 8 == 3
        } else {
            id.contains(&filter)
        }) {
            continue;
        }
        let root = engine::xml::parse(&xml).unwrap();
        data.load_tree(&root);
        let run = |threads: Option<usize>| -> Result<Vec<String>, String> {
            let mut build = data.load_build(&root)?;
            let calc_data = data.calc_data();
            engine::calcs::build_outputs(&mut build, &calc_data)?;
            let calc = MiscCalculator::new(&build, &calc_data)?;
            let tree = calc.build().spec.tree;
            let mut out = Vec::new();
            for stat in [None, power_stats().into_iter().find(|s| s.label == "Life")] {
                let p = node_power_with(
                    &calc,
                    &PowerOptions {
                        stat,
                        max_depth: Some(2.0),
                        cluster_nodes: cluster_nodes(tree).into_iter().take(3).collect(),
                    },
                    threads,
                )?;
                out.push(format!("{:?} {:?} {:?}", p.nodes, p.cluster, p.max));
            }
            Ok(out)
        };
        let sequential = run(None);
        for threads in [1, 4] {
            match (&sequential, run(Some(threads))) {
                (Ok(s), Ok(p)) if *s == p => {}
                (Err(s), Err(p)) if *s == p => {}
                (s, p) => failures.push(format!(
                    "{id}, {threads} threads: {:?} / {:?}",
                    s.as_ref().err(),
                    p.err()
                )),
            }
        }
        compared += 1;
    }
    eprintln!("{compared} builds compared");
    assert!(compared > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The Full DPS skills calculated ahead on threads give what calculating
/// them in order gives, with setups reused and afresh
#[test]
fn parallel_full_dps_equals_sequential() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let mut data = TypedData::load();
    let (mut compared, mut failures) = (0, Vec::new());
    for (id, xml) in corpus_builds() {
        if !id.contains(&filter) {
            continue;
        }
        let multi = xml.matches("includeInFullDPS=\"true\"").count() > 1;
        if !multi {
            continue;
        }
        data.load_tree(&engine::xml::parse(&xml).unwrap());
        for reuse in [true, false] {
            set_reuse_setup(reuse);
            set_full_dps_threads(true);
            let threaded = results(&data, &xml);
            set_full_dps_threads(false);
            let sequential = results(&data, &xml);
            set_full_dps_threads(true);
            match (threaded, sequential) {
                (Ok(t), Ok(s)) => {
                    if let Some(k) = (0..t.len()).find(|&k| t[k] != s[k]) {
                        failures.push(format!("{id} (reuse {reuse}): result {k} differs"));
                    }
                }
                (Err(t), Err(s)) if t == s => {}
                (t, s) => failures.push(format!(
                    "{id} (reuse {reuse}): threaded {:?}, sequential {:?}",
                    t.err(),
                    s.err()
                )),
            }
        }
        set_reuse_setup(true);
        compared += 1;
    }
    eprintln!("{compared} builds with Full DPS skills compared");
    assert!(compared > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
