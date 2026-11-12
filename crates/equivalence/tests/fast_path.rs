//! Fast path against simple path (`docs/design.md`, layer 7): the
//! calculations that copy one setup for every main skill
//! (`engine::calc_env::reuse_setup`) give exactly what setting each up
//! afresh gives: the sidebar's and Calcs tab's outputs, the calculator's
//! with overrides and Full DPS, and node power by Full DPS, for a sample
//! of corpus builds.
//!
//! `CALCS_FILTER=<substring>` limits the builds run, and runs each of them.

use engine::calc_env::set_reuse_setup;
use engine::calc_override::Override;
use engine::calculator::{CalcOutput, MiscCalculator};
use engine::power::{PowerOptions, node_power, power_stats};
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
