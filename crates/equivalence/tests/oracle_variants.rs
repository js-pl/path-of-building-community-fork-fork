//! The variant builds (`equivalence::variants`, the builds the corpus does
//! not reach) give the live oracle's outputs: upstream on LuaJIT
//! (`oracle/session.lua`, set up by `oracle/setup.sh`) loads each as
//! `oracle/dump.lua` loads a corpus build, and the typed engine alone
//! (`engine::calcs::build_outputs`) must give its `MAIN` and `CALCS`
//! outputs, numbers bitwise, as `build_output.rs` checks the corpus.
//!
//! Each family runs in shards (`testkit::run_shards`), each shard with an
//! oracle process of its own. `CALCS_FILTER=<substring>` limits the builds;
//! `TRIGGERS_FULL=1` runs every trigger variant instead of the sample.

use equivalence::oracle::{self, J, Outcome, parse};
use equivalence::typed_build::TypedData;
use equivalence::{skill_variants, upstream_src, variants};
use session::Session;
use std::collections::BTreeMap;

/// The variant families, by test name
fn family(name: &str, data: &mut TypedData) -> Vec<(String, String)> {
    match name {
        // Every gem whose skill has functions of its own (`preDamageFunc`,
        // ...), in each of its parts: also the skills whose functions
        // write breakdowns
        "skill_functions" => skill_variants(&data.skills, |ge| !ge.funcs.is_empty()),
        "triggers" => {
            let all = variants::trigger_variants(data);
            if std::env::var_os("TRIGGERS_FULL").is_some() {
                all
            } else {
                variants::trigger_sample(all)
            }
        }
        "mods" => variants::mod_variants(),
        "tattoos" => variants::tattoo_variants(data),
        "timeless" => variants::timeless_variants(data),
        "mirages" => variants::mirage_variants(data),
        "jewels" => variants::jewel_variants(data),
        "calcs_inputs" => variants::calcs_input_variants(data),
        _ => panic!("no variant family {name}"),
    }
}

/// The oracle's outputs of a build, or upstream's error
fn oracle_outputs(oracle: &Session, id: &str, xml: &str) -> Result<J, String> {
    let req = serde_json::json!({"cmd": "oracleOutputs", "id": id, "xml": xml}).to_string();
    let res = oracle.request(&req)?;
    match parse(res.as_bytes(), &mut 0) {
        J::Obj(mut o) => match (o.remove("ok"), o.remove("error")) {
            (Some(ok), _) => Ok(ok),
            (_, Some(J::Str(e))) => Err(e),
            _ => Err(format!("an answer of neither ok nor error: {res}")),
        },
        _ => Err(format!("not an object: {res}")),
    }
}

/// One shard of a family: its builds `k` modulo `n`
fn run_shard(name: &str, k: usize, n: usize) {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let mut data = TypedData::load();
    let builds: Vec<(String, String)> = family(name, &mut data)
        .into_iter()
        .filter(|(id, _)| id.contains(&filter))
        .enumerate()
        .filter(|(i, _)| i % n == k)
        .map(|(_, b)| b)
        .collect();
    let oracle = Session::oracle(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let (mut equal, mut failures, mut aligned) = (0, Vec::new(), Vec::new());
    let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, xml) in &builds {
        let want = match oracle_outputs(&oracle, id, xml) {
            Ok(w) => w,
            Err(e) => {
                // No reference: upstream fails on the build too
                skips
                    .entry(format!("upstream fails: {e}"))
                    .or_default()
                    .push(id.clone());
                continue;
            }
        };
        let mut outcomes = oracle::check(&mut data, id, xml, || want);
        if outcomes.iter().any(|o| matches!(o, Outcome::Failed(_))) {
            // Upstream keeps state from the builds loaded before in the
            // process (a Tempest Shield variant left Bonespire's
            // Intimidating Cry without cost): the reference is the build
            // on a fresh oracle
            let fresh = Session::oracle(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
            if let Ok(w) = oracle_outputs(&fresh, id, xml) {
                let again = oracle::check(&mut data, id, xml, || w);
                if again.iter().all(|o| !matches!(o, Outcome::Failed(_))) {
                    aligned.push(id.clone());
                }
                outcomes = again;
            }
        }
        for (mode, outcome) in ["MAIN", "CALCS"].into_iter().zip(outcomes) {
            match outcome {
                Outcome::Equal => equal += 1,
                Outcome::Skipped(reason) => skips.entry(reason).or_default().push(id.clone()),
                Outcome::Failed(e) => failures.push(format!("{id} ({mode}): {e}")),
            }
        }
    }
    eprintln!(
        "{} builds, {equal} calculations equal to the oracle, skipped {}: {skips:?}; aligned with a fresh oracle {}: {aligned:?}",
        builds.len(),
        skips.values().map(Vec::len).sum::<usize>(),
        aligned.len()
    );
    assert!(
        failures.is_empty(),
        "{} calculations differ from the oracle:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn run(test: &str, name: &str, shards: usize) {
    if let Some(shard) = testkit::shard() {
        let (k, n) = shard.split_once('/').unwrap();
        return run_shard(name, k.parse().unwrap(), n.parse().unwrap());
    }
    let shards: Vec<String> = (0..shards).map(|k| format!("{k}/{shards}")).collect();
    testkit::run_shards(test, &shards, &[" builds"]);
}

#[test]
fn skill_functions_match_oracle() {
    run("skill_functions_match_oracle", "skill_functions", 4);
}

/// Every trigger configuration's gems and uniques
/// (`equivalence::variants::trigger_variants`): a sample, all of them with
/// `TRIGGERS_FULL=1`
#[test]
fn triggers_match_oracle() {
    run("triggers_match_oracle", "triggers", 4);
}

/// Mod lines the corpus does not reach
#[test]
fn mods_match_oracle() {
    run("mods_match_oracle", "mods", 2);
}

#[test]
fn tattoos_match_oracle() {
    run("tattoos_match_oracle", "tattoos", 2);
}

#[test]
fn timeless_match_oracle() {
    run("timeless_match_oracle", "timeless", 2);
}

#[test]
fn mirages_match_oracle() {
    run("mirages_match_oracle", "mirages", 1);
}

/// Jewels that allocate unconnected nodes, and Unnatural Instinct
#[test]
fn jewels_match_oracle() {
    run("jewels_match_oracle", "jewels", 1);
}

/// The Calcs tab's buff modes, main socket groups, skill parts and
/// active skills the corpus leaves at their defaults
#[test]
fn calcs_inputs_match_oracle() {
    run("calcs_inputs_match_oracle", "calcs_inputs", 1);
}
