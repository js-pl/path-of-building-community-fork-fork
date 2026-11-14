//! The typed engine's results depend on its inputs only: every corpus build
//! calculated twice, the second time in reverse order on another thread
//! (fresh thread-locals, other `HashMap` seeds, other allocation addresses,
//! symbols interned in another order), gives the same outputs.

use engine::calculator::MiscCalculator;
use equivalence::corpus_builds;
use equivalence::outputs::typed_outputs;
use equivalence::typed_build::TypedData;
use std::collections::BTreeMap;
use std::fmt::Write;

/// The sidebar's, the Calcs tab's and the calculator's outputs of a build
fn outputs(data: &mut TypedData, xml: &str) -> String {
    let root = engine::xml::parse(xml).unwrap();
    data.load_tree(&root);
    let mut build = match data.load_build(&root) {
        Ok(b) => b,
        Err(e) => return format!("build: {e}"),
    };
    let calc_data = data.calc_data();
    let mut out = String::new();
    match engine::calcs::build_outputs(&mut build, &calc_data) {
        Ok(o) => {
            writeln!(out, "main {:?}", typed_outputs(&o.main.player)).unwrap();
            writeln!(
                out,
                "minion {:?}",
                o.main.minion.as_ref().map(typed_outputs)
            )
            .unwrap();
            match &o.calcs {
                Ok(c) => writeln!(out, "calcs {:?}", typed_outputs(&c.player)).unwrap(),
                Err(e) => writeln!(out, "calcs: {e}").unwrap(),
            }
        }
        Err(e) => return format!("outputs: {e}"),
    }
    match MiscCalculator::new(&build, &calc_data) {
        Ok(c) => writeln!(out, "calculator {:?}", typed_outputs(&c.base().player)).unwrap(),
        Err(e) => writeln!(out, "calculator: {e}").unwrap(),
    }
    out
}

fn run(builds: &[(String, String)], reverse: bool) -> BTreeMap<String, String> {
    let mut data = TypedData::load();
    let mut order: Vec<&(String, String)> = builds.iter().collect();
    if reverse {
        order.reverse();
    }
    order
        .into_iter()
        .map(|(id, xml)| (id.clone(), outputs(&mut data, xml)))
        .collect()
}

#[test]
fn outputs_are_deterministic() {
    let builds = corpus_builds();
    let first = run(&builds, false);
    let again = {
        let builds = builds.clone();
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || run(&builds, true))
            .unwrap()
            .join()
            .unwrap()
    };
    let differ: Vec<&String> = first
        .keys()
        .filter(|id| first.get(*id) != again.get(*id))
        .collect();
    assert!(differ.is_empty(), "outputs differ between runs: {differ:?}");
}
