//! The typed `calcs.perform` (`engine::calc_perform`) leaves the player,
//! the enemy and the skills as the translated `calcs.perform` does when it
//! reaches the defence calculations: the player's output, every database's
//! mods, conditions and multipliers. The translated state is taken by
//! hooking `calcs.defence`; the same after the defence and the defence
//! estimations, hooking `calcs.buildDefenceEstimations` and
//! `calcs.triggers`.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::actor::OutVal;
use engine::calc_setup::BuffMode;
use engine::mods::convert::Converter;
use engine::mods::tag::TagKind;
use engine::mods::{Mod, ModValue, Mods, Store};
use engine::skill_data::SkillData;
use equivalence::typed_build::TypedData;
use equivalence::{corpus, typed_json, upstream_src};
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

/// The corpus builds as (id, XML)
fn corpus_builds() -> Vec<(String, String)> {
    corpus()
        .into_iter()
        .filter(|(_, p)| !p.extension().is_some_and(|e| e == "json"))
        .map(|(id, p)| (id, std::fs::read_to_string(&p).unwrap()))
        .collect()
}

#[test]
fn perform_matches_upstream() {
    run_builds(&|_| corpus_builds());
}

/// Every gem whose skill has functions of its own (`preDamageFunc`, ...),
/// in each of its parts, as the main skill of a corpus build whose main
/// skill uses the same weapons or is a spell like it
#[test]
fn skill_functions_match_upstream() {
    run_builds(&skill_variants);
}

/// The main skill of a build's XML, by its first gem's skill id
fn main_skill_id(xml: &str) -> Option<String> {
    let root = engine::xml::parse(xml).ok()?;
    let pob = root.child("PathOfBuilding")?;
    let main: usize = pob.child("Build")?.attr("mainSocketGroup")?.parse().ok()?;
    let skills = pob.child("Skills")?;
    let active = skills.attr("activeSkillSet").unwrap_or("1");
    let set = skills
        .elements()
        .find(|e| e.elem == "SkillSet" && e.attr("id") == Some(active))?;
    let group = set.elements().filter(|e| e.elem == "Skill").nth(main - 1)?;
    group
        .elements()
        .filter(|e| e.elem == "Gem")
        .find_map(|g| g.attr("skillId").map(String::from))
}

fn skill_variants(data: &SkillData) -> Vec<(String, String)> {
    let builds: Vec<(
        String,
        String,
        Option<Rc<engine::skill_data::GrantedEffect>>,
    )> = corpus_builds()
        .into_iter()
        .filter(|(id, _)| id.starts_with("gen-"))
        .map(|(id, xml)| {
            let ge = main_skill_id(&xml).and_then(|s| data.skills.get(&s).cloned());
            (id, xml, ge)
        })
        .collect();
    let spell = |ge: &engine::skill_data::GrantedEffect| {
        ge.weapon_types.is_none() && ge.base_flags.iter().any(|f| f == "spell")
    };
    let mut out = Vec::new();
    for gem in data.gems() {
        let ge = &gem.granted_effect;
        if ge.funcs.is_empty() || ge.support || gem.name.contains("Unknown") {
            continue;
        }
        let template = builds.iter().find(|(_, _, t)| {
            t.as_ref()
                .is_some_and(|t| match (&ge.weapon_types, &t.weapon_types) {
                    (Some(w), Some(tw)) => w.first().is_some_and(|f| tw.contains(f)),
                    (None, None) => spell(ge) == spell(t),
                    _ => false,
                })
        });
        let Some((tid, xml, _)) = template else {
            continue;
        };
        let parts = ge.parts.as_ref().map_or(1, |p| p.len().max(1));
        for part in 1..=parts {
            let gem_xml = format!(
                "<Skill enabled=\"true\" label=\"Variant\" mainActiveSkill=\"1\" mainActiveSkillCalcs=\"1\">\n<Gem count=\"1\" enableGlobal1=\"true\" enableGlobal2=\"true\" enabled=\"true\" gemId=\"{}\" level=\"{}\" nameSpec=\"{}\" quality=\"20\" skillId=\"{}\" variantId=\"{}\" skillPart=\"{part}\"/>\n</Skill>\n",
                gem.game_id, gem.natural_max_level, gem.name, gem.id, gem.variant_id
            );
            let Some(set) = xml.find("<SkillSet id=\"1\">") else {
                continue;
            };
            let at = set + "<SkillSet id=\"1\">".len();
            let mut v = String::new();
            v.push_str(&xml[..at]);
            v.push('\n');
            v.push_str(&gem_xml);
            v.push_str(&xml[at..]);
            let v = engine::pat::gsub(&v, "mainSocketGroup=\"%d+\"", "mainSocketGroup=\"1\"");
            out.push((format!("fn-{}-{part}-{tid}", gem.id), v));
        }
    }
    out
}

fn run_builds(make: &dyn Fn(&SkillData) -> Vec<(String, String)>) {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = TypedData::load(&host);
    let mut failures = Vec::new();
    let (mut compared, mut skipped, mut offence_skipped) = (0, 0, 0);
    // `OFFENCE_PARTIAL=1`: after the offence, only the outputs the typed
    // offence sets
    let partial = std::env::var("OFFENCE_PARTIAL").is_ok();
    let stage_name = |k: usize| ["defence", "buildDefenceEstimations", "triggers", "offence"][k];
    let mut skip_reasons: BTreeMap<String, usize> = BTreeMap::new();
    for (id, xml) in make(&data.skills) {
        if !id.contains(&filter) {
            continue;
        }
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        // The translated perform, up to the defence
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
        let typed_env = match engine::calc_env::init_env(
            &mut typed_build,
            &calc_data,
            BuffMode::new(Some("EFFECTIVE")),
        ) {
            Ok(e) => e,
            Err(e) => {
                failures.push(format!("{id}: init_env: {e}"));
                continue;
            }
        };
        let recalc = engine::calc_cache::Recalculator::new(
            &typed_build,
            &calc_data,
            BuffMode::new(Some("EFFECTIVE")),
        );
        let mut input = engine::calc_perform::PerformInput::new(&typed_build, &calc_data, false)
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
