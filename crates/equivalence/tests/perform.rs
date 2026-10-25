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
use engine::calc_setup::{BuffMode, SetupData};
use engine::item::ItemContext;
use engine::item_data::ItemData;
use engine::mods::convert::Converter;
use engine::mods::tag::TagKind;
use engine::mods::{Mod, ModValue, Mods, Store};
use engine::skill_data::SkillData;
use engine::tree::NodeType;
use equivalence::typed_build::Trees;
use equivalence::{corpus, mod_parser, typed_json, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
use serde_json::{Map, Value as J, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
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
    Snapshot {
        output,
        player: lua_db(&get(env, "modDB")),
        enemy: lua_db(&get(env, "enemyDB")),
        skills,
    }
}

/// Compares the typed calculation with the translated state
fn compare_stage(p: &engine::calc_perform::Perform, want: &mut Snapshot, fail: &mut Vec<String>) {
    let got: BTreeMap<String, J> = p.c.actors.actors[0]
        .output
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), typed_json(v)))
        .collect();
    for k in got
        .keys()
        .chain(want.output.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        if got.get(k) != want.output.get(k) {
            fail.push(format!(
                "output {k}: {} != {}",
                got.get(k).map_or("missing".into(), |v| v.to_string()),
                want.output
                    .get(k)
                    .map_or("missing".into(), |v| v.to_string())
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
    if want.skills.len() != p.lists.len() {
        fail.push(format!("{} skills != {}", p.lists.len(), want.skills.len()));
    } else {
        for (i, (l, w)) in p.lists.iter().zip(want.skills.iter_mut()).enumerate() {
            compare_db(&format!("skill {i}"), &typed_db(p.c.store(l.list)), w, fail);
        }
    }
}

#[test]
fn perform_matches_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let ldata = host.global("data");
    let config_data = engine::config::ConfigData::from_lua(&ldata, &host.global("SkillType"))
        .unwrap_or_else(|e| panic!("config data: {e}"));
    let var_list = rt::call(&host.global("LoadModule"), &[s("Modules/ConfigOptions")])
        .unwrap_or_else(|e| panic!("{}", describe(&e)))
        .into_iter()
        .next()
        .unwrap();
    let option_defaults = engine::config_tab::option_defaults(&var_list);
    let setup_data =
        SetupData::from_lua(&ldata, &host.global("ModFlag"), &host.global("KeywordFlag"))
            .unwrap_or_else(|e| panic!("setup data: {e}"));
    let item_data = ItemData::from_lua(&ldata, &host.global("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let skill_data = SkillData::from_lua(
        &ldata,
        &host.global("colorCodes"),
        &host.global("SkillType"),
    )
    .unwrap_or_else(|e| panic!("skill data: {e}"));
    let radius: Vec<String> = entries(&get(&ldata, "jewelRadius"))
        .into_iter()
        .map(|(_, r)| {
            lua::to_string(&get(&r, "label"))
                .to_str_lossy()
                .into_owned()
        })
        .collect();
    let one_hand_map: HashMap<String, bool> = setup_data
        .weapon_type_info
        .iter()
        .map(|(k, v)| (k.clone(), v.one_hand))
        .collect();
    let one_hand = |t: &str| one_hand_map.get(t).copied();
    let mut trees = Trees::new(&lua::to_string(&host.global("latestTreeVersion")).to_str_lossy());
    let latest = trees.latest().to_string();
    let socket_ids: Vec<u32> = trees
        .get(&latest, &parser)
        .nodes
        .iter()
        .filter(|n| n.kind == NodeType::Socket)
        .map(|n| n.id)
        .collect();
    let calc_data = engine::calc_env::CalcData {
        setup: &setup_data,
        items: &item_data,
        skills: &skill_data,
        parser: &parser,
        jewel_radius: &radius,
    };
    let mut failures = Vec::new();
    let (mut compared, mut skipped) = (0, 0);
    let mut skip_reasons: BTreeMap<String, usize> = BTreeMap::new();
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        if !entries(&get(&get(&build, "spec"), "subGraphs")).is_empty() {
            *skip_reasons
                .entry("cluster jewel subgraphs".into())
                .or_default() += 1;
            skipped += 1;
            continue;
        }
        // The translated perform, up to the defence
        let calcs = get(&get(&build, "calcsTab"), "calcs");
        let env = rt::call(&get(&calcs, "initEnv"), &[build.clone(), s("MAIN")])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)))
            .into_iter()
            .next()
            .unwrap();
        if get(
            &get(&get(&env, "modDB"), "conditions"),
            "AffectedByEnergyBlade",
        )
        .truthy()
        {
            *skip_reasons.entry("Energy Blade".into()).or_default() += 1;
            skipped += 1;
            continue;
        }
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
        if snaps.len() != stages.len() {
            failures.push(format!(
                "{id}: the translated perform reached {} of the stages",
                snaps.len()
            ));
            continue;
        }

        // The typed perform
        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        let version = engine::build::tree_version(&root);
        let tree = trees.get(&version, &parser);
        let load_ctx = engine::build::LoadContext {
            items: ItemContext {
                data: &item_data,
                parser: &parser,
                jewel_radius: &radius,
                default_affix_quality: 0.5,
            },
            skill_data: &skill_data,
            config_data: &config_data,
            option_defaults: &option_defaults,
            parser: &parser,
            socket_ids: &socket_ids,
            one_hand: &one_hand,
            minions: &|m: &str| setup_data.minions.contains_key(m),
        };
        let mut typed_build = match engine::build::Build::load(&root, &load_ctx, tree, &parser) {
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
        let input = engine::calc_perform::PerformInput::new(&typed_build, &calc_data, false)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
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
                _ => p.defence_estimations(0),
            };
            if let Err(e) = result {
                fail.push(format!("typed {} failed: {e}", stages[*stage]));
                break;
            }
            let before = fail.len();
            compare_stage(&p, want, &mut fail);
            if fail.len() > before {
                fail.insert(before, format!("at {}:", stages[*stage]));
                break;
            }
        }
        if !fail.is_empty() {
            fail.truncate(25);
            failures.push(format!("{id}:\n    {}", fail.join("\n    ")));
        }
    }
    eprintln!("{compared} performs compared, {skipped} skipped: {skip_reasons:?}");
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
