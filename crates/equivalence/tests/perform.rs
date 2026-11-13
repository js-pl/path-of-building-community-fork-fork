//! The typed `calcs.perform` (`engine::calc_perform`) leaves the player,
//! the enemy and the skills as the translated `calcs.perform` does when it
//! reaches the defence calculations: the player's output, every database's
//! mods, conditions and multipliers. The translated state is taken by
//! hooking `calcs.defence`; the same after the defence and the defence
//! estimations, hooking `calcs.buildDefenceEstimations` and
//! `calcs.triggers`.
//!
//! `CALCS_FILTER=<substring>` limits the builds run, `CALCS_SKIP=<a>,<b>`
//! leaves out the builds whose ids contain any.

use engine::actor::OutVal;
use engine::calc_env::CalcMode;
use engine::mods::convert::Converter;
use engine::mods::tag::TagKind;
use engine::mods::{Mod, ModValue, Mods, Store};
use equivalence::typed_build::TypedData;
use equivalence::{corpus_builds, skill_variants, typed_json, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
use serde_json::{Map, Value as J, json};
use std::cell::RefCell;
use std::collections::BTreeMap;
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

fn entries(v: &Value) -> Vec<(Key, Value)> {
    v.as_table().map(|t| t.entries()).unwrap_or_default()
}

fn number(n: f64) -> J {
    if n.is_nan() {
        json!({"$num": "nan"})
    } else if n.is_infinite() {
        json!({"$num": if n > 0.0 { "inf" } else { "-inf" }})
    } else {
        serde_json::Number::from_f64(n).map_or(J::Null, J::Number)
    }
}

/// An output value of the translation as JSON, as `oracle/json.lua`
/// encodes it
fn lua_json(v: &Value, key: &str) -> J {
    match v {
        Value::Nil => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(n) => number(*n),
        Value::Str(x) => J::String(x.to_str_lossy().into_owned()),
        Value::Table(_) if key == "sourceGem" => {
            json!({"$gem": lua::to_string(&get(v, "nameSpec")).to_str_lossy()})
        }
        Value::Table(_) if key == "sourceItem" => {
            json!({"$item": lua::to_string(&get(v, "name")).to_str_lossy()})
        }
        Value::Table(_) => {
            let mut m = Map::new();
            for (k, x) in entries(v) {
                let k = lua::to_string(&k.to_value()).to_str_lossy().into_owned();
                let j = lua_json(&x, &k);
                m.insert(k, j);
            }
            J::Object(m)
        }
        _ => json!("$func"),
    }
}

fn typed_json(v: &OutVal) -> J {
    match v {
        OutVal::Num(n) => number(*n),
        OutVal::Bool(b) => J::Bool(*b),
        OutVal::Str(x) => J::String(x.to_string()),
        OutVal::Table(t) => J::Object(t.iter().map(|(k, v)| (k.clone(), typed_json(v))).collect()),
        OutVal::Gem(n) => json!({"$gem": &**n}),
        OutVal::Item(n) => json!({"$item": &**n}),
        OutVal::List(l) => J::Array(l.iter().map(typed_json).collect()),
    }
}

/// Takes over from a typed mod what the translation cannot show the same,
/// as `calc_setup.rs` does
fn align(typed: &Mod, lua: &mut Mod) {
    if typed
        .source
        .as_deref()
        .is_some_and(|s| s.starts_with("Tree:"))
        && !lua.source.as_deref().is_some_and(|s| s.starts_with("Tree"))
    {
        lua.source = typed.source.clone();
    }
    match (&typed.value, &mut lua.value) {
        (ModValue::Func(_), l @ ModValue::Func(_)) => {
            if let ModValue::Func(f) = &*l
                && f.name.contains("<anonymous>")
                && f.args.is_empty()
            {
                *l = typed.value.clone();
            }
        }
        (ModValue::Record(t), ModValue::Record(l)) => {
            if let (Some(ModValue::Mod(t)), Some(ModValue::Mod(l))) =
                (t.get("mod"), l.get_mut(engine::mods::Sym::new("mod")))
            {
                align(t, l);
            }
        }
        _ => {}
    }
    for l in lua.tags.iter_mut() {
        if let TagKind::Other { fields, .. } = &mut l.kind {
            fields
                .fields
                .retain(|(k, _)| k.as_str() != "warcryPowerBonus");
        }
    }
}

/// A database's state: mods by name, conditions, multipliers
#[derive(Default)]
struct Db {
    mods: BTreeMap<String, Vec<Mod>>,
    conditions: BTreeMap<String, bool>,
    multipliers: BTreeMap<String, f64>,
}

fn lua_db(db: &Value) -> Db {
    let conv = Converter::default();
    let mut out = Db::default();
    let mods = get(db, "mods");
    if mods.is_nil() {
        // A ModList: its own mods
        for (k, m) in entries(db) {
            if !matches!(k, Key::Int(_)) {
                continue;
            }
            if let Ok(m) = conv.mod_(&m) {
                out.mods
                    .entry(m.name.as_str().to_string())
                    .or_default()
                    .push(m);
            }
        }
    } else {
        for (k, list) in entries(&mods) {
            let Key::Str(name) = k else { continue };
            let list: Vec<Mod> = entries(&list)
                .into_iter()
                .map(|(_, m)| conv.mod_(&m).unwrap_or_else(|e| panic!("{e}")))
                .collect();
            if !list.is_empty() {
                out.mods.insert(name.to_str_lossy().into_owned(), list);
            }
        }
    }
    for (k, v) in entries(&get(db, "conditions")) {
        if let Key::Str(k) = k
            && v.truthy()
        {
            out.conditions.insert(k.to_str_lossy().into_owned(), true);
        }
    }
    for (k, v) in entries(&get(db, "multipliers")) {
        if let (Key::Str(k), Some(n)) = (k, v.as_number()) {
            out.multipliers.insert(k.to_str_lossy().into_owned(), n);
        }
    }
    out
}

fn typed_db(store: &Store) -> Db {
    let mut out = Db::default();
    match &store.mods {
        Mods::Db(by_name) => {
            for (k, v) in by_name {
                if !v.is_empty() {
                    out.mods.insert(k.as_str().to_string(), v.clone());
                }
            }
        }
        Mods::List(l) => {
            for m in l {
                out.mods
                    .entry(m.name.as_str().to_string())
                    .or_default()
                    .push(m.clone());
            }
        }
    }
    for (k, v) in &store.conditions {
        if *v {
            out.conditions.insert(k.as_str().to_string(), true);
        }
    }
    for (k, v) in &store.multipliers {
        out.multipliers.insert(k.as_str().to_string(), *v);
    }
    out
}

/// The differences of two databases; mods of a name compare in any order,
/// as upstream adds some from hash tables
fn compare_db(what: &str, t: &Db, l: &mut Db, fail: &mut Vec<String>) {
    let names: std::collections::BTreeSet<String> =
        t.mods.keys().chain(l.mods.keys()).cloned().collect();
    for name in names {
        let tl = t.mods.get(&name).cloned().unwrap_or_default();
        let mut ll = l.mods.get(&name).cloned().unwrap_or_default();
        for lm in ll.iter_mut() {
            if tl.contains(lm) {
                continue;
            }
            for tm in &tl {
                let mut c = lm.clone();
                align(tm, &mut c);
                if c == *tm {
                    *lm = c;
                    break;
                }
            }
        }
        let key = |m: &Mod| typed_json::mod_(m).to_string();
        let mut ts: Vec<String> = tl.iter().map(key).collect();
        let mut ls: Vec<String> = ll.iter().map(key).collect();
        ts.sort();
        ls.sort();
        if ts != ls {
            let only_t: Vec<&String> = ts.iter().filter(|x| !ls.contains(x)).collect();
            let only_l: Vec<&String> = ls.iter().filter(|x| !ts.contains(x)).collect();
            fail.push(format!(
                "{what} {name}:\n      typed only {only_t:?}\n      translated only {only_l:?}"
            ));
        }
    }
    if t.conditions != l.conditions {
        let d: Vec<String> = t
            .conditions
            .keys()
            .chain(l.conditions.keys())
            .filter(|k| t.conditions.get(*k) != l.conditions.get(*k))
            .map(|k| {
                format!(
                    "{k}: {:?} != {:?}",
                    t.conditions.get(k),
                    l.conditions.get(k)
                )
            })
            .collect();
        fail.push(format!("{what} conditions: {}", d.join(", ")));
    }
    if t.multipliers != l.multipliers {
        let d: Vec<String> = t
            .multipliers
            .keys()
            .chain(l.multipliers.keys())
            .filter(|k| t.multipliers.get(*k) != l.multipliers.get(*k))
            .map(|k| {
                format!(
                    "{k}: {:?} != {:?}",
                    t.multipliers.get(k),
                    l.multipliers.get(k)
                )
            })
            .collect();
        fail.push(format!("{what} multipliers: {}", d.join(", ")));
    }
}

/// The translated state at `calcs.defence(env, env.player)`
struct Snapshot {
    output: BTreeMap<String, J>,
    player: Db,
    enemy: Db,
    skills: Vec<Db>,
    /// `env.minion`'s database and skills
    minion: Option<(Db, Vec<Db>)>,
}

fn snapshot(env: &Value) -> Snapshot {
    let player = get(env, "player");
    let output = entries(&get(&player, "output"))
        .into_iter()
        .map(|(k, v)| {
            let k = lua::to_string(&k.to_value()).to_str_lossy().into_owned();
            let j = lua_json(&v, &k);
            (k, j)
        })
        .collect();
    let skills = entries(&get(&player, "activeSkillList"))
        .into_iter()
        .map(|(_, a)| lua_db(&get(&a, "skillModList")))
        .collect();
    let minion = get(env, "minion");
    let minion = (!minion.is_nil()).then(|| {
        let skills = entries(&get(&minion, "activeSkillList"))
            .into_iter()
            .map(|(_, a)| lua_db(&get(&a, "skillModList")))
            .collect();
        (lua_db(&get(&minion, "modDB")), skills)
    });
    Snapshot {
        output,
        player: lua_db(&get(env, "modDB")),
        enemy: lua_db(&get(env, "enemyDB")),
        skills,
        minion,
    }
}

/// Compares the typed calculation with the translated state
fn compare_stage(
    p: &engine::calc_perform::Perform,
    want: &mut Snapshot,
    fail: &mut Vec<String>,
    partial: bool,
) {
    let out_json = |a: usize| -> BTreeMap<String, J> {
        p.c.actors.actors[a]
            .output
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), typed_json(v)))
            .collect()
    };
    let mut got = out_json(0);
    // A mirage's copy's minion's output, which became the player's
    if let Some(J::Object(m)) = got.remove("Minion") {
        for (k, v) in m {
            got.insert(format!("Minion.{k}"), v);
        }
    }
    let mut want_out = want.output.clone();
    // `output.Minion` is the minion's output: compared by key
    if let Some(a) = p.minion_actor() {
        for (k, v) in out_json(a) {
            got.insert(format!("Minion.{k}"), v);
        }
    }
    if let Some(J::Object(m)) = want_out.remove("Minion") {
        for (k, v) in m {
            want_out.insert(format!("Minion.{k}"), v);
        }
    }
    for k in got
        .keys()
        .chain(want_out.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        if got.get(k) != want_out.get(k) && !(partial && !got.contains_key(k)) {
            fail.push(format!(
                "output {k}: {} != {}",
                got.get(k).map_or("missing".into(), |v| v.to_string()),
                want_out.get(k).map_or("missing".into(), |v| v.to_string())
            ));
        }
    }
    compare_db(
        "player",
        &typed_db(p.c.store(p.c.db(0))),
        &mut want.player,
        fail,
    );
    compare_db(
        "enemy",
        &typed_db(p.c.store(p.c.db(1))),
        &mut want.enemy,
        fail,
    );
    if want.skills.len() != p.n_player {
        fail.push(format!("{} skills != {}", p.n_player, want.skills.len()));
    } else {
        for (i, (l, w)) in p.lists.iter().zip(want.skills.iter_mut()).enumerate() {
            compare_db(&format!("skill {i}"), &typed_db(p.c.store(l.list)), w, fail);
        }
    }
    match (p.env_minion, &mut want.minion) {
        (None, None) => {}
        (Some(m), Some((db, skills))) => {
            let m = &p.minions[m];
            compare_db("minion", &typed_db(p.c.store(p.c.db(m.actor))), db, fail);
            if skills.len() != m.skills.len() {
                fail.push(format!(
                    "{} minion skills != {}",
                    m.skills.len(),
                    skills.len()
                ));
            } else {
                for (k, (&i, w)) in m.skills.iter().zip(skills.iter_mut()).enumerate() {
                    let list = p.c.store(p.lists[i].list);
                    compare_db(&format!("minion skill {k}"), &typed_db(list), w, fail);
                }
            }
        }
        (t, _) => fail.push(format!("typed minion {t:?}, translated differs")),
    }
}

#[test]
fn perform_matches_upstream() {
    run_builds("perform_matches_upstream", &|_| corpus_builds());
}

/// Every gem whose skill has functions of its own (`preDamageFunc`, ...)
#[test]
fn skill_functions_match_upstream() {
    run_builds("skill_functions_match_upstream", &|data| {
        skill_variants(&data.skills, |ge| !ge.funcs.is_empty())
    });
}

/// Every trigger configuration's gems and uniques
/// (`equivalence::variants::trigger_variants`): each gem, item, socket
/// group and main skill in one of its templates, in all of them with
/// `TRIGGERS_FULL=1`
#[test]
fn triggers_match_upstream() {
    run_builds("triggers_match_upstream", &|data| {
        let all = equivalence::variants::trigger_variants(data);
        if std::env::var_os("TRIGGERS_FULL").is_some() {
            all
        } else {
            equivalence::variants::trigger_sample(all)
        }
    });
}

/// Mod lines the corpus does not reach
/// (`equivalence::variants::mod_variants`)
#[test]
fn mods_match_upstream() {
    run_builds("mods_match_upstream", &|_| {
        equivalence::variants::mod_variants()
    });
}

/// Tattoos (`equivalence::variants::tattoo_variants`)
#[test]
fn tattoos_match_upstream() {
    run_builds(
        "tattoos_match_upstream",
        &equivalence::variants::tattoo_variants,
    );
}

/// Timeless jewels (`equivalence::variants::timeless_variants`)
#[test]
fn timeless_match_upstream() {
    run_builds(
        "timeless_match_upstream",
        &equivalence::variants::timeless_variants,
    );
}

/// Mirages of skills with minions
/// (`equivalence::variants::mirage_variants`)
#[test]
fn mirages_match_upstream() {
    run_builds(
        "mirages_match_upstream",
        &equivalence::variants::mirage_variants,
    );
}

/// Builds one process compares
const CHUNK: usize = 25;

/// Compares the builds of the test `test` in processes of their own, `CHUNK`
/// builds each (`testkit::run_shards`): the translated program keeps about
/// 12 MB of every build it loaded, next to the 1.6 GB it starts with, so a
/// thousand builds in one process run out of memory. The processes are
/// this test again, given their builds in the file their shard names
fn run_builds(test: &str, make: &dyn Fn(&mut TypedData) -> Vec<(String, String)>) {
    if let Some(path) = testkit::shard() {
        let builds = std::fs::read(path).unwrap();
        return compare_builds(serde_json::from_slice(&builds).unwrap());
    }
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    // `CALCS_SKIP=<a>,<b>`: leaves out the builds whose ids contain any
    let skip = std::env::var("CALCS_SKIP").unwrap_or_default();
    let skip: Vec<&str> = skip.split(',').filter(|s| !s.is_empty()).collect();
    let builds: Vec<(String, String)> = make(&mut TypedData::load())
        .into_iter()
        .filter(|(id, _)| id.contains(&filter) && !skip.iter().any(|s| id.contains(s)))
        .collect();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("perform-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let shards: Vec<String> = builds
        .chunks(CHUNK)
        .enumerate()
        .map(|(k, chunk)| {
            let path = dir.join(k.to_string());
            std::fs::write(&path, serde_json::to_vec(chunk).unwrap()).unwrap();
            path.to_string_lossy().into_owned()
        })
        .collect();
    let result = std::panic::catch_unwind(|| {
        testkit::run_shards(test, &shards, &["performs compared"]);
    });
    std::fs::remove_dir_all(&dir).unwrap();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

fn compare_builds(builds: Vec<(String, String)>) {
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    equivalence::install_inflate(&host);
    let mut data = TypedData::load();
    // `SkillsTab:ProcessGemInstance` empties the shared cost of the level of
    // a triggered skill an item grants; the typed engine does within a
    // build only, so each build starts from the data's own costs
    let mut costs: Vec<(Value, Value)> = Vec::new();
    for (_, skill) in entries(&get(&host.global("data"), "skills")) {
        for (_, level) in entries(&get(&skill, "levels")) {
            costs.push((level.clone(), get(&level, "cost")));
        }
    }
    let mut failures = Vec::new();
    let (mut compared, mut skipped, mut offence_skipped) = (0, 0, 0);
    // `OFFENCE_PARTIAL=1`: after the offence, only the outputs the typed
    // offence sets
    let partial = std::env::var("OFFENCE_PARTIAL").is_ok();
    let stage_name = |k: usize| ["defence", "buildDefenceEstimations", "triggers", "offence"][k];
    let mut skip_reasons: BTreeMap<String, usize> = BTreeMap::new();
    for (id, xml) in builds {
        for (level, cost) in &costs {
            if let Some(t) = level.as_table() {
                t.set_str("cost", cost.clone());
            }
        }
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        // The translated perform, up to the defence, on a cache of its own
        // as a frame's calculation starts with
        rt::call(&host.global("wipeGlobalCache"), &[])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let calcs = get(&get(&build, "calcsTab"), "calcs");
        let env = rt::call(&get(&calcs, "initEnv"), &[build.clone(), s("MAIN")])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)))
            .into_iter()
            .next()
            .unwrap();
        // The state when the defence, the estimations and the triggers start
        let stages = ["defence", "buildDefenceEstimations", "triggers"];
        let snaps: Rc<RefCell<Vec<(usize, Snapshot)>>> = Rc::new(RefCell::new(Vec::new()));
        let player = get(&env, "player");
        let origs: Vec<Value> = stages.iter().map(|n| get(&calcs, n)).collect();
        for (k, name) in stages.iter().enumerate() {
            let snaps = snaps.clone();
            let target = env.clone();
            let player = player.clone();
            let orig = origs[k].clone();
            let hook = rt::func(name, move |args| {
                if !snaps.borrow().iter().any(|(s, _)| *s == k)
                    && args.first().is_some_and(|a| a.raw_eq(&target))
                    && args.get(1).is_some_and(|a| a.raw_eq(&player))
                {
                    snaps.borrow_mut().push((k, snapshot(&args[0])));
                }
                rt::call(&orig, args)
            });
            calcs.as_table().unwrap().set_str(name, hook);
        }
        let result = rt::call(&get(&calcs, "perform"), std::slice::from_ref(&env));
        for (name, orig) in stages.iter().zip(origs) {
            calcs.as_table().unwrap().set_str(name, orig);
        }
        if let Err(e) = result {
            failures.push(format!("{id}: translated perform failed: {}", describe(&e)));
            continue;
        }
        let mut snaps = std::mem::take(&mut *snaps.borrow_mut());
        // The state after the offence: what `calcs.perform` leaves
        snaps.push((stages.len(), snapshot(&env)));
        if snaps.len() != stages.len() + 1 {
            failures.push(format!(
                "{id}: the translated perform reached {} of the stages",
                snaps.len()
            ));
            continue;
        }

        // The typed perform
        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        data.load_tree(&root);
        let data = &data;
        let calc_data = data.calc_data();
        let mut typed_build = match data.load_build(&root) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{id}: build: {e}"));
                continue;
            }
        };
        let typed_env =
            match engine::calc_env::init_env(&mut typed_build, &calc_data, CalcMode::Main) {
                Ok(e) => e,
                Err(e) => {
                    failures.push(format!("{id}: init_env: {e}"));
                    continue;
                }
            };
        let recalc =
            engine::calc_cache::Recalculator::new(&typed_build, &calc_data, CalcMode::Main);
        let mut input = engine::calc_perform::PerformInput::new(&typed_build, &calc_data, true)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        input.recalc = Some(&recalc);
        let p = match engine::calc_perform::perform(typed_env, input) {
            Ok(p) => p,
            Err(e) if e.starts_with("not ported") => {
                *skip_reasons.entry(e).or_default() += 1;
                skipped += 1;
                continue;
            }
            Err(e) => {
                failures.push(format!("{id}: typed perform failed: {e}"));
                continue;
            }
        };
        compared += 1;
        let mut p = p;
        let mut fail = Vec::new();
        for (stage, want) in snaps.iter_mut() {
            let result = match *stage {
                0 => Ok(()),
                1 => p.defence(0),
                2 => p.defence_estimations(0),
                _ => p.player_offence().and_then(|_| p.minion_calcs()),
            };
            if let Err(e) = result {
                if e.starts_with("not ported") {
                    *skip_reasons.entry(e).or_default() += 1;
                    offence_skipped += 1;
                } else {
                    fail.push(format!("typed {} failed: {e}", stage_name(*stage)));
                }
                break;
            }
            let before = fail.len();
            compare_stage(&p, want, &mut fail, partial && *stage == stages.len());
            if fail.len() > before {
                fail.insert(before, format!("at {}:", stage_name(*stage)));
                break;
            }
        }
        if !fail.is_empty() {
            let lua_main = get(
                &get(
                    &get(&get(&get(&env, "player"), "mainSkill"), "activeEffect"),
                    "grantedEffect",
                ),
                "name",
            );
            fail.insert(
                0,
                format!(
                    "main skill: typed {}, translated {}",
                    p.skill_name(p.main()),
                    lua::to_string(&lua_main).to_str_lossy()
                ),
            );
            fail.truncate(25);
            failures.push(format!("{id}:\n    {}", fail.join("\n    ")));
        }
    }
    eprintln!(
        "{compared} performs compared ({offence_skipped} up to the offence), {skipped} skipped: {skip_reasons:?}"
    );
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
