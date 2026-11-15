//! The typed engine alone (`engine::calcs::build_outputs`) gives every
//! corpus build the oracle's own outputs (`tests/oracle/<id>.json`), of
//! the sidebar's calculation (`MAIN`) and the Calcs tab's (`CALCS`): the
//! player's output with the Full DPS table and the main skill's minion's. Numbers compare bitwise; a missing or extra key fails.
//! Builds that reach a part not ported yet are skipped and listed. The game
//! data is `gamedata`'s, in its binary form: no Lua runs but the importer's.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use equivalence::oracle::{self, Outcome, parse};
use equivalence::typed_build::TypedData;
use equivalence::{corpus, oracle_dir};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The outcomes of a build's `MAIN` and `CALCS` calculations
fn check(data: &mut TypedData, id: &str, path: &PathBuf) -> [Outcome; 2] {
    let text = std::fs::read_to_string(path).unwrap();
    oracle::check(data, id, &text, || {
        let oracle = std::fs::read(oracle_dir().join(format!("{id}.json"))).unwrap();
        parse(&oracle, &mut 0)
    })
}

#[test]
fn build_output_matches_oracle() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let builds: Vec<(String, PathBuf)> = corpus()
        .into_iter()
        .filter(|(id, p)| id.contains(&filter) && p.extension().is_some_and(|e| e == "xml"))
        .collect();
    assert!(!builds.is_empty(), "no build matches CALCS_FILTER={filter}");
    let mut data = TypedData::load();
    let mut failures = Vec::new();
    let mut summary = Vec::new();
    let mut results: Vec<_> = builds
        .iter()
        .map(|(id, path)| (id, check(&mut data, id, path)))
        .collect();
    for (k, mode) in ["MAIN", "CALCS"].into_iter().enumerate() {
        let mut equal = 0;
        let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, outcomes) in &mut results {
            match std::mem::replace(&mut outcomes[k], Outcome::Equal) {
                Outcome::Equal => equal += 1,
                Outcome::Skipped(reason) => skips.entry(reason).or_default().push(id.to_string()),
                Outcome::Failed(e) => failures.push(format!("{id} ({mode}): {e}")),
            }
        }
        summary.push(format!(
            "{mode}: {equal} builds equal to the oracle, {} skipped: {skips:?}",
            skips.values().map(Vec::len).sum::<usize>()
        ));
    }
    eprintln!("{}", summary.join("\n"));
    assert!(
        failures.is_empty(),
        "{} calculations differ from the oracle:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
