//! Mod weights: `engine::mod_weights::mod_weights` gives the weights of
//! upstream's `TradeQueryGenerator` (`StartQuery` and `ExecuteQuery`) on
//! the live oracle (`oracleModWeights` of `oracle/session.lua`), for each
//! third of each corpus build's equipped slots, an empty slot, a jewel
//! socket, the special items (Megalomaniac, Watcher's Eye) and a few sets
//! of stat weights and options, rotating by build. Numbers compare bitwise. Builds that reach a part not ported yet
//! are skipped and listed. A build that differs is asked of a fresh
//! oracle, since upstream changes shared skill data for the rest of the
//! process.
//!
//! The corpus runs in shards (`testkit::run_shards`), each with an oracle
//! process of its own. `WEIGHTS_FILTER=<substring>` limits the builds run.

use engine::calculator::MiscCalculator;
use engine::mod_weights::{
    Eldritch, JewelType, ModWeights, Special, StatWeight, WeightOptions, mod_weights,
};
use engine::parallel::Threads;
use equivalence::oracle::{J, diff, parse};
use equivalence::typed_build::TypedData;
use equivalence::{corpus_builds, upstream_src};
use serde_json::{Value, json};
use session::Session;
use std::collections::BTreeMap;

/// The equipment slots weighed, where the build has an item in them
const SLOTS: &[&str] = &[
    "Weapon 1",
    "Weapon 2",
    "Helmet",
    "Body Armour",
    "Gloves",
    "Boots",
    "Amulet",
    "Ring 1",
    "Ring 2",
    "Belt",
];

/// The stat weights and options, by build
fn options(k: usize) -> (WeightOptions, Value) {
    let sets: [&[(&str, f64)]; 4] = [
        &[("FullDPS", 1.0), ("TotalEHP", 0.5)],
        &[("Life", 1.0), ("FireTakenHit", 0.5)],
        &[("CombinedDPS", 1.0), ("Armour", 0.3), ("Evasion", 0.3)],
        &[("FullDPS", 1.0), ("TotalEHP", 0.5)],
    ];
    let set = sets[k % sets.len()];
    let stat_weights = set
        .iter()
        .map(|(s, w)| StatWeight::new(s, *w).unwrap_or_else(|| panic!("no power stat {s}")))
        .collect();
    let rich = k % 4 == 3;
    let typed = WeightOptions {
        stat_weights,
        include_corrupted: rich,
        include_talisman: rich,
        include_scourge: rich,
        include_eldritch: if rich {
            Eldritch::Regular
        } else if k % 4 == 1 {
            Eldritch::WithPresence
        } else {
            Eldritch::None
        },
        jewel_type: JewelType::Base,
        special: None,
    };
    let eldritch = match typed.include_eldritch {
        Eldritch::None => "Remove",
        Eldritch::Regular => "Keep regular",
        Eldritch::WithPresence => "Keep regular+presence",
    };
    let oracle = json!({
        "statWeights": set.iter().map(|(s, w)| json!({"stat": s, "weightMult": w})).collect::<Vec<_>>(),
        "includeCorrupted": rich,
        "includeTalisman": rich,
        "includeScourge": rich,
        "includeEldritch": eldritch,
        "jewelType": "Base",
    });
    (typed, oracle)
}

/// A query of the sample, typed and as the oracle request's case
struct Case {
    name: String,
    slot: Option<String>,
    opts: WeightOptions,
    oracle: Value,
}

fn cases(b: &engine::build::Build, k: usize) -> Vec<Case> {
    let tab = &b.items;
    let filled = |slot: &str| {
        tab.ordered_slots
            .iter()
            .find(|s| s.slot_name == slot)
            .is_some_and(|s| tab.item(s.sel_item_id).is_some())
    };
    let (opts, oracle) = options(k);
    let case = |name: String, slot: Option<&str>, special: Option<(Special, &str)>| {
        let mut opts = opts.clone();
        let mut oracle = oracle.clone();
        if let Some(s) = slot {
            oracle["slot"] = json!(s);
        }
        if let Some((s, name)) = special {
            opts.special = Some(s);
            oracle["special"] = json!(name);
        }
        Case {
            name,
            slot: slot.map(str::to_string),
            opts,
            oracle,
        }
    };
    // A third of the slots per build, rotating
    let mut out: Vec<Case> = SLOTS
        .iter()
        .enumerate()
        .filter(|(j, s)| (j + k).is_multiple_of(3) && filled(s))
        .map(|(_, s)| case(s.to_string(), Some(s), None))
        .collect();
    // An empty slot weighs on the default base
    if let Some(s) = SLOTS.iter().find(|s| !filled(s)) {
        out.push(case(format!("empty {s}"), Some(s), None));
    }
    // A jewel socket of the tree, with the jewel types
    let jewel = tab
        .ordered_slots
        .iter()
        .find(|s| s.node_id.is_some() && s.slot_name.contains("Jewel"))
        .map(|s| s.slot_name.clone());
    if let Some(j) = jewel.as_ref().filter(|_| k % 3 == 1) {
        let mut c = case(format!("{j} abyss"), Some(j), None);
        c.opts.jewel_type = JewelType::Abyss;
        c.oracle["jewelType"] = json!("Abyss");
        out.push(c);
        out.push(case(
            format!("{j} Watcher's Eye"),
            Some(j),
            Some((Special::WatchersEye, "Watcher's Eye")),
        ));
    }
    if k % 3 == 2 {
        out.push(case(
            "Megalomaniac".into(),
            None,
            Some((Special::Megalomaniac, "Megalomaniac")),
        ));
    }
    out
}

/// A result as the oracle gives it, the weights by trade mod id
fn result_json(w: &ModWeights) -> J {
    let mut weights = BTreeMap::new();
    for m in &w.weights {
        let entry = BTreeMap::from([
            ("invert".to_string(), J::Bool(m.invert)),
            ("meanStatDiff".to_string(), J::Num(m.mean_stat_diff)),
            ("weight".to_string(), J::Num(m.weight)),
        ]);
        weights.insert(m.trade_id.clone(), J::Obj(entry));
    }
    let mut obj = BTreeMap::from([
        ("baseStatValue".to_string(), J::Num(w.base_stat_value)),
        ("weights".to_string(), J::Obj(weights)),
    ]);
    if let Some(c) = &w.category {
        obj.insert("category".to_string(), J::Str(c.clone()));
    }
    J::Obj(obj)
}

/// The oracle's result in the form of [`result_json`]
fn oracle_json(j: J) -> J {
    let J::Obj(mut o) = j else { return j };
    let list = match o.remove("weights") {
        Some(J::Arr(a)) => a,
        _ => Vec::new(),
    };
    let mut weights = BTreeMap::new();
    for w in list {
        let J::Obj(mut w) = w else { continue };
        let Some(J::Str(id)) = w.remove("tradeModId") else {
            continue;
        };
        weights.insert(id, J::Obj(w));
    }
    o.insert("weights".into(), J::Obj(weights));
    J::Obj(o)
}

/// The oracle's answer for a build: each case's result (or upstream's
/// error)
fn ask(
    oracle: &Session,
    id: &str,
    xml: &str,
    cases: &[Case],
) -> Result<Vec<Result<J, String>>, String> {
    // `cmd` first: the oracle tells its own requests by their start
    let rest = json!({
        "id": id,
        "xml": xml,
        "cases": cases.iter().map(|c| c.oracle.clone()).collect::<Vec<_>>(),
    })
    .to_string();
    let req = format!(r#"{{"cmd":"oracleModWeights",{}"#, &rest[1..]);
    let res = oracle.request(&req)?;
    let J::Obj(mut o) = parse(res.as_bytes(), &mut 0) else {
        return Err(format!("not an object: {res}"));
    };
    if let Some(J::Str(e)) = o.remove("error") {
        return Err(e);
    }
    let Some(J::Obj(mut ok)) = o.remove("ok") else {
        return Err(format!("an answer of neither ok nor error: {res}"));
    };
    let answers = match ok.remove("cases") {
        Some(J::Arr(a)) => a,
        _ => Vec::new(),
    };
    Ok(answers
        .into_iter()
        .map(|a| match a {
            J::Obj(mut a) if a.contains_key("error") => match a.remove("error") {
                Some(J::Str(e)) => Err(e),
                e => Err(format!("{e:?}")),
            },
            a => Ok(oracle_json(a)),
        })
        .collect())
}

/// The differences of a build's weights from the oracle's
fn differences(
    got: &[std::result::Result<ModWeights, String>],
    cases: &[Case],
    answers: &[Result<J, String>],
) -> Vec<String> {
    let mut fail = Vec::new();
    for ((case, got), want) in cases.iter().zip(got).zip(answers) {
        match (got, want) {
            (Ok(g), Ok(w)) => {
                let mut d = Vec::new();
                diff(&result_json(g), w, "", &mut d);
                fail.extend(d.into_iter().map(|d| format!("{}: {d}", case.name)));
            }
            (Err(_), Err(_)) => {}
            (Ok(_), Err(e)) => fail.push(format!("{}: upstream failed: {e}", case.name)),
            (Err(e), Ok(_)) => fail.push(format!("{}: typed failed: {e}", case.name)),
        }
    }
    if cases.len() != answers.len() {
        fail.push(format!("{} cases, {} answers", cases.len(), answers.len()));
    }
    fail
}

/// Shards of the corpus, each with an oracle of its own
const SHARDS: usize = 4;

#[test]
fn mod_weights_match_upstream() {
    let test = "mod_weights_match_upstream";
    if let Some(shard) = testkit::shard() {
        let (k, n) = shard.split_once('/').unwrap();
        return run_shard(k.parse().unwrap(), n.parse().unwrap());
    }
    let shards: Vec<String> = (0..SHARDS).map(|k| format!("{k}/{SHARDS}")).collect();
    testkit::run_shards(test, &shards, &[" builds", "aligned "]);
}

fn run_shard(k: usize, n: usize) {
    let filter = std::env::var("WEIGHTS_FILTER").unwrap_or_default();
    let mut data = TypedData::load();
    let mut oracle = Session::oracle(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let mut failures = Vec::new();
    let (mut compared, mut queries, mut weights) = (0, 0, 0);
    let mut aligned: Vec<String> = Vec::new();
    let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let builds = corpus_builds()
        .into_iter()
        .filter(|(id, _)| id.contains(&filter))
        .enumerate()
        .filter(|(i, _)| i % n == k);
    for (i, (id, xml)) in builds {
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
        let calc = engine::calcs::build_outputs(&mut build, &calc_data)
            .and_then(|_| MiscCalculator::new(&build, &calc_data));
        let calc = match calc {
            Ok(c) => c,
            Err(e) if e.starts_with("not ported") => {
                skips.entry(e).or_default().push(id.clone());
                continue;
            }
            Err(e) => {
                failures.push(format!("{id}: typed calculator failed: {e}"));
                continue;
            }
        };
        let ctx = data.item_context();
        let mut got = Vec::new();
        let mut kept = Vec::new();
        for c in cases(&build, i) {
            let r = mod_weights(
                &calc,
                &ctx,
                &data.query_mods,
                c.slot.as_deref(),
                &c.opts,
                Threads::Sequential,
            );
            match r {
                Err(e) if e.starts_with("not ported") => {
                    skips.entry(e).or_default().push(format!("{id} {}", c.name));
                }
                r => {
                    got.push(r);
                    kept.push(c);
                }
            }
        }
        let cases = kept;
        compared += 1;
        queries += cases.len();
        weights += got
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|w| w.weights.len())
            .sum::<usize>();
        let answers =
            ask(&oracle, &id, &xml, &cases).unwrap_or_else(|e| panic!("{id}: oracle: {e}"));
        let mut fail = differences(&got, &cases, &answers);
        // Upstream changes shared skill data for good, so a build loaded
        // earlier can change the oracle's answers: a build that differs is
        // asked of a fresh oracle, and aligned when that one agrees
        if !fail.is_empty() {
            oracle = Session::oracle(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
            let answers =
                ask(&oracle, &id, &xml, &cases).unwrap_or_else(|e| panic!("{id}: oracle: {e}"));
            if differences(&got, &cases, &answers).is_empty() {
                aligned.push(id.clone());
                fail.clear();
            }
        }
        if !fail.is_empty() {
            let n = fail.len();
            fail.truncate(if filter.is_empty() { 20 } else { 500 });
            failures.push(format!(
                "{id}: {n} differences\n    {}",
                fail.join("\n    ")
            ));
        }
    }
    let skipped: usize = skips.values().map(Vec::len).sum();
    eprintln!(
        "{compared} builds, {queries} queries, {weights} weights compared, {skipped} skipped: {skips:?}"
    );
    eprintln!("aligned with a fresh oracle: {aligned:?}");
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
