//! The benchmark (`src/bin/bench.rs`) runs: one build, one run of each
//! operation but node power, typed and translated.

use equivalence::bench::{OPS, Options, run, table};

#[test]
fn bench_runs() {
    let opts = Options {
        filter: "gen-0029".into(),
        reps: 1,
        overrides: 1,
        power: false,
        ..Options::default()
    };
    let rows = run(&opts);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert!(r.error.is_none(), "{:?}", r.error);
    for (k, op) in OPS.iter().enumerate() {
        let power = *op == "node power";
        assert_eq!(r.typed[k].is_some(), !power, "typed {op}");
        assert_eq!(
            r.lua[k].is_some(),
            !power && *op != "load",
            "translated {op}"
        );
    }
    assert!(table(&rows).contains("| gen-0029 |"));
}
