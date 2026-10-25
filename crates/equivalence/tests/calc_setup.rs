//! The typed calculation setup (`engine::calc_setup`) builds the mod
//! databases the translated `calcs.initEnv` builds, for every corpus build:
//! so far the base mods of the player and the enemy, the bandit, pantheon
//! and configuration mods, and the base conditions and multipliers. The
//! translated databases hold more (tree, items, skills); the comparison
//! keeps to the mods of the sources the typed setup builds.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::calc_setup::{self, BaseInput, BuffMode, SetupData};
use engine::config::{Inputs, Val};
use engine::mods::convert::Converter;
use engine::mods::tag::TagKind;
use engine::mods::{Mod, ModValue, Mods, Store, Sym};
use equivalence::{corpus, mod_parser, typed_json, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

fn inputs(v: &Value) -> Inputs {
    let mut out = Inputs::new();
    for (k, x) in entries(v) {
        let Key::Str(k) = k else { continue };
        let val = match x {
            Value::Bool(b) => Val::Bool(b),
            Value::Number(n) => Val::Number(n),
            Value::Str(s) => Val::Str(s.to_str_lossy().into()),
            _ => continue,
        };
        out.insert(k.to_str_lossy().into_owned(), val);
    }
    out
}

fn list_mods(v: &Value) -> Vec<Mod> {
    let conv = Converter::default();
    v.as_table()
        .map(|t| t.array())
        .unwrap_or_default()
        .iter()
        .map(|m| conv.mod_(m).unwrap())
        .collect()
}

/// Takes over from a typed mod what the translation cannot show the same:
/// - the source of a tree mod: upstream's `modLib.setSource` rewrites the
///   tree's shared keystone mods in place when an item or the configuration
///   grants the keystone, so their source depends on the calculations run
///   before; the typed port keeps the tree's mods as they are
/// - functions the translation gives no name and no upvalues, the inline
///   functions of `ModParser.lua`
fn align(typed: &Mod, lua: &mut Mod) {
    if typed
        .source
        .as_deref()
        .is_some_and(|s| s.starts_with("Tree:"))
        && !lua.source.as_deref().is_some_and(|s| s.starts_with("Tree"))
    {
        lua.source = typed.source.clone();
    }
    align_value(&typed.value, &mut lua.value);
    // `CalcPerform` sets the warcry power bonus on the tag the skill data
    // shares with every copy of the mod
    for (t, l) in typed.tags.iter().zip(lua.tags.iter_mut()) {
        if let (TagKind::Other { fields: tf, .. }, TagKind::Other { fields: lf, .. }) =
            (&t.kind, &mut l.kind)
            && tf.get("warcryPowerBonus").is_none()
        {
            lf.fields.retain(|(k, _)| k.as_str() != "warcryPowerBonus");
        }
    }
}

fn align_value(typed: &ModValue, lua: &mut ModValue) {
    match (typed, lua) {
        (ModValue::Func(_), l @ ModValue::Func(_)) => {
            if let ModValue::Func(f) = &*l
                && f.name.contains("<anonymous>")
                && f.args.is_empty()
            {
                *l = typed.clone();
            }
        }
        (ModValue::Mod(t), ModValue::Mod(l)) => align(t, l),
        (ModValue::Record(t), ModValue::Record(l)) => {
            for (k, v) in &t.fields {
                if let Some(lv) = l.get_mut(*k) {
                    align_value(v, lv);
                }
            }
        }
        (ModValue::List(t), ModValue::List(l)) => {
            for (t, l) in t.iter().zip(l.iter_mut()) {
                align_value(t, l);
            }
        }
        _ => {}
    }
}

/// The mods of a ModDB by name
fn db_mods(db: &Value) -> BTreeMap<String, Vec<Mod>> {
    let conv = Converter::default();
    entries(&get(db, "mods"))
        .into_iter()
        .map(|(k, list)| {
            let Key::Str(name) = k else {
                panic!("mod name")
            };
            let mods = list
                .as_table()
                .map(|t| t.array())
                .unwrap_or_default()
                .iter()
                .map(|m| conv.mod_(m).unwrap_or_else(|e| panic!("{e}")))
                .collect();
            (name.to_str_lossy().into_owned(), mods)
        })
        .collect()
}

fn typed_mods(store: &Store) -> BTreeMap<String, Vec<Mod>> {
    match &store.mods {
        Mods::Db(by_name) => by_name
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.clone()))
            .collect(),
        Mods::List(_) => panic!("a list"),
    }
}

/// Compares the typed database with the translated one's mods from the
/// typed one's sources
fn compare(what: &str, t: &Store, l: &Value, fail: &mut dyn FnMut(String)) {
    let tm = typed_mods(t);
    let lm = db_mods(l);
    let sources: BTreeSet<Option<String>> = tm
        .values()
        .flatten()
        .map(|m| m.source.as_deref().map(String::from))
        .collect();
    let names: BTreeSet<&String> = tm.keys().chain(lm.keys()).collect();
    for name in names {
        let tl = tm.get(name).cloned().unwrap_or_default();
        let mut ll: Vec<Mod> = lm
            .get(name)
            .into_iter()
            .flatten()
            // Mods without a source count only where the typed setup has
            // the name, as later parts add some (allocation counts)
            .filter(|m| {
                sources.contains(&m.source.as_deref().map(String::from))
                    && (m.source.is_some() || tm.contains_key(name))
            })
            .cloned()
            .collect();
        for (t, l) in tl.iter().zip(ll.iter_mut()) {
            align(t, l);
        }
        if tl != ll {
            let show = |m: &[Mod]| {
                m.iter()
                    .map(|m| typed_json::mod_(m).to_string())
                    .collect::<Vec<_>>()
                    .join("\n      ")
            };
            fail(format!(
                "{what} {name}:\n    typed\n      {}\n    translated\n      {}",
                show(&tl),
                show(&ll)
            ));
        }
    }
    let conditions: HashMap<Sym, bool> = entries(&get(l, "conditions"))
        .into_iter()
        .filter_map(|(k, v)| match k {
            Key::Str(s) => Some((Sym::new(&s.to_str_lossy()), v.truthy())),
            _ => None,
        })
        .collect();
    for (k, v) in &t.conditions {
        if conditions.get(k) != Some(v) {
            fail(format!(
                "{what} condition {}: {v} != {:?}",
                k.as_str(),
                conditions.get(k)
            ));
        }
    }
    let multipliers: HashMap<Sym, f64> = entries(&get(l, "multipliers"))
        .into_iter()
        .filter_map(|(k, v)| match k {
            Key::Str(s) => Some((Sym::new(&s.to_str_lossy()), v.as_number()?)),
            _ => None,
        })
        .collect();
    for (k, v) in &t.multipliers {
        if multipliers.get(k) != Some(v) {
            fail(format!(
                "{what} multiplier {}: {v} != {:?}",
                k.as_str(),
                multipliers.get(k)
            ));
        }
    }
}

#[test]
fn base_dbs_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let data = SetupData::from_lua(
        &host.global("data"),
        &host.global("ModFlag"),
        &host.global("KeywordFlag"),
    )
    .unwrap_or_else(|e| panic!("setup data: {e}"));
    let mut failures = Vec::new();
    let mut compared = 0;
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let calcs = get(&get(&build, "calcsTab"), "calcs");
        let env = rt::call(&get(&calcs, "initEnv"), &[build.clone(), s("MAIN")])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)))
            .into_iter()
            .next()
            .unwrap();
        let spec = get(&env, "spec");
        let tree = get(&spec, "tree");
        let class_id = get(&env, "classId");
        let mut class = rt::index(&get(&tree, "characterData"), &class_id).unwrap_or_default();
        if class.is_nil() {
            class = rt::index(&get(&tree, "classes"), &class_id).unwrap_or_default();
        }
        let stat = |k: &str| get(&class, k).as_number().unwrap_or(f64::NAN);
        let config = get(&build, "configTab");
        let input = inputs(&get(&env, "configInput"));
        let config_mods = list_mods(&get(&config, "modList"));
        let config_enemy_mods = list_mods(&get(&config, "enemyModList"));
        let b = BaseInput {
            class_stats: [stat("base_str"), stat("base_dex"), stat("base_int")],
            character_level: get(&build, "characterLevel").as_number().unwrap(),
            enemy_level: get(&env, "enemyLevel").as_number().unwrap(),
            config_input: &input,
            config_mods: &config_mods,
            config_enemy_mods: &config_enemy_mods,
            mode: BuffMode::new(Some("EFFECTIVE")),
        };
        compared += 1;
        let mut fail = |what: String| {
            if failures.len() < 40 {
                failures.push(format!("{id}: {what}"));
            }
        };
        match calc_setup::base_dbs(&data, &parser, &b) {
            Ok((player, enemy)) => {
                compare("player", &player, &get(&env, "modDB"), &mut fail);
                compare("enemy", &enemy, &get(&env, "enemyDB"), &mut fail);
            }
            Err(e) => fail(format!("typed failed: {e}")),
        }
    }
    eprintln!("{compared} setups compared");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The passive nodes' mods before the items (`env.initialNodeModDB`): the
/// allocated nodes' mods in allocation order, then the keystones they grant
#[test]
fn initial_node_mods_match_upstream() {
    use engine::item_data::ItemData;
    use engine::spec::PassiveSpec;
    use equivalence::typed_build::Trees;
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let item_data = ItemData::from_lua(&host.global("data"), &host.global("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let mut trees = Trees::new(&lua::to_string(&host.global("latestTreeVersion")).to_str_lossy());
    let mut failures = Vec::new();
    let (mut compared, mut skipped) = (0, 0);
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        let Some(tree_xml) = root.child("PathOfBuilding").and_then(|p| p.child("Tree")) else {
            continue;
        };
        let specs: Vec<_> = tree_xml.elements().filter(|e| e.elem == "Spec").collect();
        let active = tree_xml
            .attr("activeSpec")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .min(specs.len());
        let spec_xml = specs[active - 1];
        let version = spec_xml.attr("treeVersion").unwrap_or("2_6").to_string();
        let tree = trees.get(&version, &parser);
        let mut spec = PassiveSpec::new(tree, &parser).unwrap();
        if let Err(e) = spec.load(spec_xml).and_then(|_| spec.post_load()) {
            eprintln!("{id}: skipped: {e}");
            skipped += 1;
            continue;
        }
        let calcs = get(&get(&build, "calcsTab"), "calcs");
        let env = rt::call(&get(&calcs, "initEnv"), &[build.clone(), s("MAIN")])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)))
            .into_iter()
            .next()
            .unwrap();
        if !entries(&get(&get(&build, "spec"), "subGraphs")).is_empty() {
            eprintln!("{id}: skipped: cluster jewel subgraphs are not ported");
            skipped += 1;
            continue;
        }
        let alloc: Vec<usize> = spec.alloc_nodes.iter().map(|(_, &i)| i).collect();
        compared += 1;
        let result = calc_setup::build_mod_list_for_node_list(
            &spec,
            &alloc,
            &spec.alloc_nodes,
            &mut [],
            &[],
            true,
            &item_data,
        )
        .and_then(|(mut mods, _)| {
            calc_setup::merge_keystones(&mut mods, tree, &mut vec![])?;
            Ok(mods)
        });
        match result {
            Ok(mods) => {
                let mut lmods = list_mods(&get(&env, "initialNodeModDB"));
                for (m, l) in mods.iter().zip(lmods.iter_mut()) {
                    align(m, l);
                }
                if mods != lmods {
                    let n = mods.len().min(lmods.len());
                    let i = (0..n).find(|&i| mods[i] != lmods[i]).unwrap_or(n);
                    let show = |m: Option<&Mod>| {
                        m.map_or("none".into(), |m| typed_json::mod_(m).to_string())
                    };
                    failures.push(format!(
                        "{id}: {} mods != {}, first difference at {}:\n    typed {}\n    translated {}",
                        mods.len(),
                        lmods.len(),
                        i + 1,
                        show(mods.get(i)),
                        show(lmods.get(i))
                    ));
                }
            }
            Err(e) => failures.push(format!("{id}: typed failed: {e}")),
        }
    }
    eprintln!("{compared} node lists compared, {skipped} skipped");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn ids(v: &Value) -> BTreeSet<i64> {
    entries(v)
        .into_iter()
        .filter_map(|(k, _)| match k {
            Key::Table(t) => rt::index(&Value::Table(t.0), &s("id"))
                .ok()?
                .as_number()
                .map(|n| n as i64),
            _ => None,
        })
        .collect()
}

/// The items and the tree with them (`engine::calc_tree::setup`): the item
/// database, what is equipped, the radius jewels, the allocated nodes and
/// the player's database with the items' and the tree's mods
#[test]
fn items_setup_matches_upstream() {
    use engine::calc_items::ItemsInput;
    use engine::calc_skills;
    use engine::calc_tree;
    use engine::item::ItemContext;
    use engine::item_data::ItemData;
    use engine::items_tab::ItemsTab;
    use engine::skill_data::SkillData;
    use engine::skills::Skills;
    use engine::skills::{ExplodeSource, SocketGroup};
    use engine::spec::PassiveSpec;
    use engine::tree::NodeType;
    use equivalence::typed_build::Trees;
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
    let ctx = ItemContext {
        data: &item_data,
        parser: &parser,
        jewel_radius: &radius,
        default_affix_quality: 0.5,
    };
    let weapon_types = get(&ldata, "weaponTypeInfo");
    let one_hand = |t: &str| -> Option<bool> {
        let info = get(&weapon_types, t);
        (!info.is_nil()).then(|| get(&info, "oneHand").truthy())
    };
    let mut trees = Trees::new(&lua::to_string(&host.global("latestTreeVersion")).to_str_lossy());
    let latest = trees.latest().to_string();
    let socket_ids: Vec<u32> = trees
        .get(&latest, &parser)
        .nodes
        .iter()
        .filter(|n| n.kind == NodeType::Socket)
        .map(|n| n.id)
        .collect();
    let mut failures = Vec::new();
    let (mut compared, mut skipped) = (0, 0);
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        let pob = root.child("PathOfBuilding").unwrap();
        let (Some(items_xml), Some(tree_xml)) = (pob.child("Items"), pob.child("Tree")) else {
            continue;
        };
        if !entries(&get(&get(&build, "spec"), "subGraphs")).is_empty() {
            eprintln!("{id}: skipped: cluster jewel subgraphs are not ported");
            skipped += 1;
            continue;
        }
        let specs: Vec<_> = tree_xml.elements().filter(|e| e.elem == "Spec").collect();
        let active = tree_xml
            .attr("activeSpec")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .min(specs.len());
        let spec_xml = specs[active - 1];
        let version = spec_xml.attr("treeVersion").unwrap_or("2_6").to_string();
        let tree = trees.get(&version, &parser);
        let mut spec = PassiveSpec::new(tree, &parser).unwrap();
        if let Err(e) = spec.load(spec_xml).and_then(|_| spec.post_load()) {
            eprintln!("{id}: skipped: {e}");
            skipped += 1;
            continue;
        }
        let mut tab = match ItemsTab::load(items_xml, &ctx, &socket_ids) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{id}: items tab: {e}"));
                continue;
            }
        };
        tab.populate_slots(&mut spec, tree, &one_hand);
        let mut skills = pob.child("Skills").map(|x| Skills::load(x, &skill_data));
        let groups: Vec<SocketGroup> = skills
            .as_ref()
            .map_or(vec![], |s| s.socket_group_list().to_vec());
        let alloc: Vec<usize> = spec.alloc_nodes.iter().map(|(_, &i)| i).collect();
        let mut node_mods = match calc_setup::build_mod_list_for_node_list(
            &spec,
            &alloc,
            &spec.alloc_nodes,
            &mut [],
            &[],
            true,
            &item_data,
        ) {
            Ok((m, _)) => m,
            Err(e) => {
                failures.push(format!("{id}: node mods: {e}"));
                continue;
            }
        };
        calc_setup::merge_keystones(&mut node_mods, tree, &mut vec![]).unwrap();
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
            eprintln!("{id}: skipped: Energy Blade is not ported");
            skipped += 1;
            continue;
        }
        let config = inputs(&get(&env, "configInput"));
        let input = ItemsInput {
            spec: &spec,
            tab: &tab,
            socket_groups: &groups,
            node_mods: &node_mods,
            config: &config,
            precision: &item_data,
            energy_blade: false,
            jewel_radius: &radius,
            extra_jewel_funcs: &[],
        };
        // The player's database before the items
        let ltree = get(&get(&env, "spec"), "tree");
        let class_id = get(&env, "classId");
        let mut class = rt::index(&get(&ltree, "characterData"), &class_id).unwrap_or_default();
        if class.is_nil() {
            class = rt::index(&get(&ltree, "classes"), &class_id).unwrap_or_default();
        }
        let stat = |k: &str| get(&class, k).as_number().unwrap_or(f64::NAN);
        let config_tab = get(&build, "configTab");
        let config_mods = list_mods(&get(&config_tab, "modList"));
        let config_enemy_mods = list_mods(&get(&config_tab, "enemyModList"));
        let b = BaseInput {
            class_stats: [stat("base_str"), stat("base_dex"), stat("base_int")],
            character_level: get(&build, "characterLevel").as_number().unwrap(),
            enemy_level: get(&env, "enemyLevel").as_number().unwrap(),
            config_input: &config,
            config_mods: &config_mods,
            config_enemy_mods: &config_enemy_mods,
            mode: BuffMode::new(Some("EFFECTIVE")),
        };
        let (mut base, enemy_base) = calc_setup::base_dbs(&setup_data, &parser, &b).unwrap();
        calc_setup::allocation_mods(&spec, &mut base);
        let env_setup = match calc_tree::setup(&base, &input, &item_data) {
            Ok(o) => o,
            Err(e) => {
                failures.push(format!("{id}: typed failed: {e}"));
                continue;
            }
        };
        let out = &env_setup.items;
        compared += 1;
        let mut fail = |what: String| {
            if failures.len() < 80 {
                failures.push(format!("{id}: {what}"));
            }
        };
        // Every mod, condition and multiplier of the item database
        let ldb = get(&env, "itemModDB");
        let (tm, lm) = (typed_mods(&out.item_db), db_mods(&ldb));
        for name in tm.keys().chain(lm.keys()).collect::<BTreeSet<_>>() {
            let (t, mut l) = (
                tm.get(name).cloned().unwrap_or_default(),
                lm.get(name).cloned().unwrap_or_default(),
            );
            for (t, l) in t.iter().zip(l.iter_mut()) {
                align_value(&t.value, &mut l.value);
            }
            if t != l {
                let n = t.len().min(l.len());
                let i = (0..n).find(|&i| t[i] != l[i]).unwrap_or(n);
                let show =
                    |m: Option<&Mod>| m.map_or("none".into(), |m| typed_json::mod_(m).to_string());
                fail(format!(
                    "itemModDB {name}: {} mods != {}, at {}:\n    typed {}\n    translated {}",
                    t.len(),
                    l.len(),
                    i + 1,
                    show(t.get(i)),
                    show(l.get(i))
                ));
            }
        }
        let lconds: BTreeMap<String, Value> = entries(&get(&ldb, "conditions"))
            .into_iter()
            .filter_map(|(k, v)| match k {
                Key::Str(k) => Some((k.to_str_lossy().into_owned(), v)),
                _ => None,
            })
            .collect();
        let tconds: BTreeMap<String, bool> = out
            .item_db
            .conditions
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), *v))
            .collect();
        let lc: BTreeMap<String, bool> = lconds
            .iter()
            .map(|(k, v)| (k.clone(), v.truthy()))
            .collect();
        if tconds != lc {
            fail(format!("conditions {tconds:?} != {lc:?}"));
        }
        for (k, class) in &out.class_restrictions {
            let l = lconds
                .get(k)
                .and_then(|v| v.as_str())
                .map(|s| s.to_str_lossy().into_owned());
            if l.as_deref() != Some(class) {
                fail(format!("condition {k}: {class} != {l:?}"));
            }
        }
        let lm: BTreeMap<String, Option<f64>> = entries(&get(&ldb, "multipliers"))
            .into_iter()
            .filter_map(|(k, v)| match k {
                Key::Str(k) => Some((k.to_str_lossy().into_owned(), v.as_number())),
                _ => None,
            })
            .collect();
        let tmult: BTreeMap<String, Option<f64>> = out
            .item_db
            .multipliers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), Some(*v)))
            .collect();
        if tmult != lm {
            fail(format!("multipliers {tmult:?} != {lm:?}"));
        }
        // The equipped items, flasks and radius jewels
        let litems: BTreeMap<String, f64> = entries(&get(&get(&env, "player"), "itemList"))
            .into_iter()
            .filter_map(|(k, v)| match k {
                Key::Str(k) => Some((k.to_str_lossy().into_owned(), get(&v, "id").as_number()?)),
                _ => None,
            })
            .collect();
        if litems != out.item_list {
            fail(format!("itemList {:?} != {litems:?}", out.item_list));
        }
        if ids(&get(&env, "flasks")) != out.flasks.iter().copied().collect::<BTreeSet<_>>() {
            fail(format!(
                "flasks {:?} != {:?}",
                out.flasks,
                ids(&get(&env, "flasks"))
            ));
        }
        if ids(&get(&env, "tinctures")) != out.tinctures.iter().copied().collect::<BTreeSet<_>>() {
            fail(format!(
                "tinctures {:?} != {:?}",
                out.tinctures,
                ids(&get(&env, "tinctures"))
            ));
        }
        let lrads: Vec<(u32, f64, String, usize)> = entries(&get(&env, "radiusJewelList"))
            .into_iter()
            .map(|(_, r)| {
                (
                    get(&r, "nodeId").as_number().unwrap_or(0.0) as u32,
                    get(&get(&r, "item"), "id").as_number().unwrap_or(0.0),
                    lua::to_string(&get(&r, "type")).to_str_lossy().into_owned(),
                    entries(&get(&r, "nodes")).len(),
                )
            })
            .collect();
        let trads: Vec<(u32, f64, String, usize)> = out
            .radius_jewels
            .iter()
            .map(|r| {
                let typ = match r.typ {
                    engine::jewel::RadiusType::Other => "Other",
                    engine::jewel::RadiusType::Self_ => "Self",
                    engine::jewel::RadiusType::SelfUnalloc => "SelfUnalloc",
                    engine::jewel::RadiusType::Threshold => "Threshold",
                };
                (r.node_id, r.item_id, typ.to_string(), r.nodes.len())
            })
            .collect();
        if trads != lrads {
            fail(format!("radiusJewelList {trads:?} != {lrads:?}"));
        }
        let lextra: BTreeSet<u32> = entries(&get(&env, "extraRadiusNodeList"))
            .into_iter()
            .filter_map(|(k, _)| match k {
                Key::Int(i) => Some(i as u32),
                _ => None,
            })
            .collect();
        let textra: BTreeSet<u32> = out.extra_radius_nodes.iter().map(|(k, _)| k).collect();
        if lextra != textra {
            fail(format!("extraRadiusNodeList {textra:?} != {lextra:?}"));
        }
        // The tree with the items
        let int_keys = |v: &Value| -> Vec<u32> {
            entries(v)
                .into_iter()
                .filter_map(|(k, _)| match k {
                    Key::Int(i) => Some(i as u32),
                    _ => None,
                })
                .collect()
        };
        let lalloc = int_keys(&get(&env, "allocNodes"));
        let talloc: Vec<u32> = env_setup.tree.alloc.iter().map(|(k, _)| k).collect();
        if lalloc != talloc {
            fail(format!("allocNodes {talloc:?} != {lalloc:?}"));
        }
        let lpassives: BTreeSet<u32> = int_keys(&get(&env, "grantedPassives"))
            .into_iter()
            .collect();
        if lpassives != env_setup.tree.granted_passives {
            fail(format!(
                "grantedPassives {:?} != {lpassives:?}",
                env_setup.tree.granted_passives
            ));
        }
        compare("modDB", &env_setup.mod_db, &get(&env, "modDB"), &mut fail);
        // The typed player's item conditions, on every tag of the
        // translated player's database
        {
            use engine::actor::{Actor, EnvActors};
            use engine::mods::Actors;
            use engine::mods::tag::TagKind;
            let mut bridge = equivalence::bridge::Bridge::new(host.env.clone());
            let pid = bridge.add(&get(&env, "modDB")).unwrap();
            let actors = EnvActors {
                actors: vec![Actor {
                    store: pid,
                    item_list: out.item_list.clone(),
                    player: Some(0),
                    ..Actor::default()
                }],
                store_actor: [(pid, 0)].into_iter().collect(),
                tab: &tab,
                item_data: &item_data,
                skill_data: &skill_data,
            };
            if let Mods::Db(by_name) = &bridge.stores.stores[pid].mods {
                for m in by_name.values().flatten() {
                    for t in &m.tags {
                        if let TagKind::ItemCondition(c) = &t.kind {
                            let (tv, lv) = (
                                actors.item_condition(pid, c, None),
                                bridge.item_condition(pid, c, None),
                            );
                            if tv != lv {
                                fail(format!(
                                    "{} item condition {c:?}: {tv} != {lv}",
                                    m.name.as_str()
                                ));
                            }
                        }
                    }
                }
            }
        }
        // The socket groups with the granted skills and explosions
        let granted = calc_skills::granted_skills(
            &env_setup.tree.node_mods,
            &out.granted_skills,
            &skill_data,
        );
        let mut explode: Vec<ExplodeSource> = env_setup
            .tree
            .node_mods
            .iter()
            .filter(|(_, n)| n.explode)
            .map(|(id, _)| ExplodeSource::Node(*id))
            .collect();
        explode.extend(
            out.explode_sources
                .iter()
                .map(|&id| ExplodeSource::Item(id)),
        );
        let mut tgroups = groups.clone();
        calc_skills::add_granted_groups(&mut tgroups, &granted, &explode, &skill_data);
        engine::skills::update_socket_groups(&mut tgroups, &tab);
        if let Some(s) = skills.as_mut().and_then(|s| s.socket_group_list_mut()) {
            *s = tgroups.clone();
        }
        let lgroups: Vec<Value> = entries(&get(&get(&build, "skillsTab"), "socketGroupList"))
            .into_iter()
            .map(|(_, g)| g)
            .collect();
        if lgroups.len() != tgroups.len() {
            fail(format!(
                "{} socket groups != {}",
                tgroups.len(),
                lgroups.len()
            ));
        }
        let text = |v: &Value| v.as_str().map(|s| s.to_str_lossy().into_owned());
        for (i, (t, l)) in tgroups.iter().zip(&lgroups).enumerate() {
            let tg = (
                t.source.clone(),
                t.slot.clone(),
                t.label.clone(),
                t.enabled,
                t.no_supports,
            );
            let lg = (
                text(&get(l, "source")),
                text(&get(l, "slot")),
                text(&get(l, "label")),
                get(l, "enabled").truthy(),
                get(l, "noSupports").truthy(),
            );
            if tg != lg {
                fail(format!("socket group {}: {tg:?} != {lg:?}", i + 1));
            }
            let lgems: Vec<Value> = entries(&get(l, "gemList"))
                .into_iter()
                .map(|(_, g)| g)
                .collect();
            let tgems: Vec<_> = t
                .gem_list
                .iter()
                .map(|g| {
                    (
                        g.skill_id.clone(),
                        g.gem_id.clone(),
                        g.name_spec.clone(),
                        g.level,
                        g.quality,
                        g.count,
                        g.enabled,
                        g.enable_global1,
                        g.triggered,
                        g.no_supports,
                        g.granted_effect.as_ref().map(|e| e.id.clone()),
                    )
                })
                .collect();
            let lgems: Vec<_> = lgems
                .iter()
                .map(|g| {
                    (
                        text(&get(g, "skillId")),
                        text(&get(g, "gemId")),
                        text(&get(g, "nameSpec")).unwrap_or_default(),
                        get(g, "level").as_number(),
                        get(g, "quality").as_number(),
                        get(g, "count").as_number(),
                        get(g, "enabled").truthy(),
                        get(g, "enableGlobal1").truthy(),
                        get(g, "triggered").truthy(),
                        get(g, "noSupports").truthy(),
                        text(&get(&get(g, "grantedEffect"), "id")),
                    )
                })
                .collect();
            if tgems != lgems {
                fail(format!(
                    "socket group {} gems: {tgems:?} != {lgems:?}",
                    i + 1
                ));
            }
        }
        let lmdb = get(&get(&env, "modDB"), "multipliers");
        for (k, v) in &out.player_multipliers {
            if get(&lmdb, k.as_str()).as_number() != Some(*v) {
                fail(format!(
                    "modDB multiplier {}: {v} != {:?}",
                    k.as_str(),
                    get(&lmdb, k.as_str()).as_number()
                ));
            }
        }
        let lgranted: Vec<String> = entries(&get(&env, "grantedSkillsItems"))
            .into_iter()
            .map(|(_, g)| {
                lua::to_string(&get(&g, "slotName"))
                    .to_str_lossy()
                    .into_owned()
            })
            .collect();
        let tgranted: Vec<String> = out
            .granted_skills
            .iter()
            .map(|g| g.slot_name.clone())
            .collect();
        if tgranted != lgranted {
            fail(format!("grantedSkillsItems {tgranted:?} != {lgranted:?}"));
        }
        // The active skills
        let conv = Converter::default();
        let fields = |v: &Value| -> engine::item::Fields {
            entries(v)
                .into_iter()
                .filter_map(|(k, x)| match k {
                    Key::Str(k) => Some((k.to_str_lossy().into_owned(), conv.value(&x).ok()?)),
                    _ => None,
                })
                .collect()
        };
        let unarmed = fields(
            &rt::index(&get(&ldata, "unarmedWeaponData"), &get(&env, "classId"))
                .unwrap_or_default(),
        );
        let imbued = skills
            .as_ref()
            .map(|s| s.imbued_support_by_slot.clone())
            .unwrap_or_default();
        let skills_input = calc_skills::SkillsInput {
            groups: &tgroups,
            tab: &tab,
            item_data: &item_data,
            skill_data: &skill_data,
            item_list: &out.item_list,
            unarmed: &unarmed,
            main_socket_group: pob
                .child("Build")
                .and_then(|b| b.attr("mainSocketGroup"))
                .and_then(|v| v.parse::<f64>().ok()),
            imbued_support_by_slot: &imbued,
            matching_socket_quality_bonus: setup_data.misc["MatchingSocketQualityBonus"],
            enemy_db: &enemy_base,
        };
        let mut player_db = env_setup.mod_db.clone();
        let setup = match calc_skills::create_active_skills(&skills_input, &mut player_db) {
            Ok(s) => s,
            Err(e) => {
                fail(format!("active skills: {e}"));
                continue;
            }
        };
        let lplayer = get(&env, "player");
        let lskills: Vec<Value> = entries(&get(&lplayer, "activeSkillList"))
            .into_iter()
            .map(|(_, v)| v)
            .collect();
        let keys = |v: &Value| -> BTreeSet<String> {
            entries(v)
                .into_iter()
                .filter(|(_, x)| x.truthy())
                .filter_map(|(k, _)| match k {
                    Key::Str(k) => Some(k.to_str_lossy().into_owned()),
                    Key::Int(i) => Some(i.to_string()),
                    _ => None,
                })
                .collect()
        };
        let group_index = |src: &Value| -> Option<(usize, usize)> {
            if src.is_nil() {
                return None;
            }
            for (gi, g) in lgroups.iter().enumerate() {
                for (i, (_, gem)) in entries(&get(g, "gemList")).into_iter().enumerate() {
                    if gem.raw_eq(src) {
                        return Some((gi, i));
                    }
                }
            }
            None
        };
        let effect = |e: &Value| {
            (
                text(&get(&get(e, "grantedEffect"), "id")),
                get(e, "level").as_number(),
                get(e, "quality").as_number(),
            )
        };
        let teffect = |e: &calc_skills::SkillEffect| {
            (Some(e.granted_effect.id.clone()), e.level, Some(e.quality))
        };
        if lskills.len() != setup.active_skills.len() {
            fail(format!(
                "{} active skills != {}",
                setup.active_skills.len(),
                lskills.len()
            ));
        }
        for (i, (t, l)) in setup.active_skills.iter().zip(&lskills).enumerate() {
            let la = get(l, "activeEffect");
            // The active effect's level and quality, and the skill's flags,
            // change as its mod list is built
            let tv = (
                teffect(&t.active_effect).0,
                t.active_effect.src,
                t.slot_name.clone(),
                t.effect_list[1..].iter().map(teffect).collect::<Vec<_>>(),
                t.skill_types
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<BTreeSet<_>>(),
            );
            let leffects: Vec<_> = entries(&get(l, "effectList"))
                .iter()
                .map(|(_, e)| effect(e))
                .collect();
            let lv = (
                effect(&la).0,
                group_index(&get(&la, "srcInstance")),
                text(&get(l, "slotName")),
                leffects.get(1..).unwrap_or_default().to_vec(),
                keys(&get(l, "skillTypes")),
            );
            if tv != lv {
                fail(format!("active skill {}: {tv:?}\n    != {lv:?}", i + 1));
            }
        }
        let lmain = get(&lplayer, "mainSkill");
        let lmain_index = lskills.iter().position(|s| s.raw_eq(&lmain));
        if lmain_index != Some(setup.main_skill) {
            fail(format!(
                "main skill {} != {lmain_index:?}",
                setup.main_skill
            ));
        }
        for (n, w) in ["weaponData1", "weaponData2"].iter().enumerate() {
            let lw = fields(&get(&lplayer, w));
            if lw != setup.weapon_data[n] {
                fail(format!("{w}: {:?} != {lw:?}", setup.weapon_data[n]));
            }
        }
        compare(
            "modDB with skills",
            &player_db,
            &get(&env, "modDB"),
            &mut fail,
        );
        // The skills' mod lists
        let mut setup = setup;
        let placeholders = inputs(&get(&env, "configPlaceholder"));
        let spectres: Vec<String> = entries(&get(&build, "spectreList"))
            .into_iter()
            .filter_map(|(_, v)| text(&v))
            .collect();
        let weapon_data = setup.weapon_data.clone();
        let mods_input = engine::calc_skill_mods::ModsInput {
            setup: &setup_data,
            skill_data: &skill_data,
            tab: &tab,
            item_data: &item_data,
            item_list: &out.item_list,
            weapon_data: &weapon_data,
            mode: BuffMode::new(Some("EFFECTIVE")),
            config: &config,
            placeholders: &placeholders,
            spectre_list: &spectres,
            character_level: get(&build, "characterLevel").as_number().unwrap(),
            enemy_level: get(&env, "enemyLevel").as_number().unwrap(),
            iron_mass: out.iron_mass_mods.is_some(),
            enemy_db: &enemy_base,
        };
        let mut enemy_multipliers = HashMap::new();
        let mut mgroups = tgroups.clone();
        if let Err(e) = engine::calc_skill_mods::build_skill_mod_lists(
            &mut setup.active_skills,
            &mut mgroups,
            &mut player_db,
            &mut enemy_multipliers,
            &mods_input,
        ) {
            fail(format!("skill mod lists: {e}"));
            continue;
        }
        for (i, (t, l)) in setup.active_skills.iter().zip(&lskills).enumerate() {
            let what = format!("skill {} ({})", i + 1, t.active_effect.granted_effect.id);
            let Some(m) = &t.mods else {
                fail(format!("{what}: no mods"));
                continue;
            };
            let la = get(l, "activeEffect");
            let tl = (t.active_effect.level, t.active_effect.quality);
            let ll = (
                get(&la, "level").as_number(),
                get(&la, "quality").as_number().unwrap_or(f64::NAN),
            );
            if tl != ll {
                fail(format!("{what}: level and quality {tl:?} != {ll:?}"));
            }
            let lflags = keys(&get(l, "skillFlags"));
            if m.skill_flags != lflags {
                fail(format!("{what}: flags {:?} != {lflags:?}", m.skill_flags));
            }
            let lcfg = get(l, "skillCfg");
            let tc = &m.skill_cfg;
            let tv = (
                tc.flags.map(|f| f as f64),
                tc.keyword_flags.map(|f| f as f64),
                tc.skill_name.as_deref().map(String::from),
                tc.slot_name.map(|s| s.as_str().to_string()),
                tc.socket_num,
                tc.skill_part,
                tc.skill_dist,
            );
            let lv = (
                get(&lcfg, "flags").as_number(),
                get(&lcfg, "keywordFlags").as_number(),
                text(&get(&lcfg, "skillName")),
                text(&get(&lcfg, "slotName")),
                get(&lcfg, "socketNum").as_number(),
                get(&lcfg, "skillPart").as_number(),
                get(&lcfg, "skillDist").as_number(),
            );
            if tv != lv {
                fail(format!("{what}: cfg {tv:?} != {lv:?}"));
            }
            let lw = (
                get(l, "weapon1Flags").as_number(),
                get(l, "weapon2Flags").as_number(),
            );
            let tw = (
                m.weapon_flags[0].map(|f| f as f64),
                m.weapon_flags[1].map(|f| f as f64),
            );
            if tw != lw {
                fail(format!("{what}: weapon flags {tw:?} != {lw:?}"));
            }
            let mut lmods = list_mods(&get(l, "skillModList"));
            for (a, b) in m.mods.iter().zip(lmods.iter_mut()) {
                align(a, b);
            }
            if m.mods != lmods {
                let n = m.mods.len().min(lmods.len());
                let k = (0..n).find(|&k| m.mods[k] != lmods[k]).unwrap_or(n);
                let show =
                    |x: Option<&Mod>| x.map_or("none".into(), |x| typed_json::mod_(x).to_string());
                fail(format!(
                    "{what}: {} skill mods != {}, at {}:\n    typed {}\n    translated {}",
                    m.mods.len(),
                    lmods.len(),
                    k + 1,
                    show(m.mods.get(k)),
                    show(lmods.get(k))
                ));
            }
            let ldata: BTreeMap<String, ModValue> = entries(&get(l, "skillData"))
                .into_iter()
                .filter_map(|(k, v)| match k {
                    Key::Str(k) => Some((k.to_str_lossy().into_owned(), conv.value(&v).ok()?)),
                    _ => None,
                })
                .collect();
            if m.skill_data != ldata {
                fail(format!("{what}: skillData {:?} != {ldata:?}", m.skill_data));
            }
            let lbuffs: Vec<(String, String, Vec<Mod>)> = entries(&get(l, "buffList"))
                .into_iter()
                .map(|(_, b)| {
                    (
                        text(&get(&b, "type")).unwrap_or_default(),
                        text(&get(&b, "name")).unwrap_or_default(),
                        list_mods(&get(&b, "modList")),
                    )
                })
                .collect();
            let tbuffs: Vec<(String, String, Vec<Mod>)> = m
                .buff_list
                .iter()
                .map(|b| (b.typ.clone(), b.name.clone(), b.mod_list.clone()))
                .collect();
            let mut lbuffs = lbuffs;
            for (t, l) in tbuffs.iter().zip(lbuffs.iter_mut()) {
                for (a, b) in t.2.iter().zip(l.2.iter_mut()) {
                    align(a, b);
                }
            }
            if tbuffs != lbuffs {
                for (t, l) in tbuffs.iter().zip(&lbuffs) {
                    for (a, b) in t.2.iter().zip(&l.2) {
                        if a != b {
                            eprintln!(
                                "buff mod {}\n  != {}",
                                typed_json::mod_(a),
                                typed_json::mod_(b)
                            );
                        }
                    }
                }
                fail(format!(
                    "{what}: buffs {:?} != {:?}",
                    tbuffs
                        .iter()
                        .map(|b| (&b.0, &b.1, b.2.len()))
                        .collect::<Vec<_>>(),
                    lbuffs
                        .iter()
                        .map(|b| (&b.0, &b.1, b.2.len()))
                        .collect::<Vec<_>>()
                ));
            }
            let lminion = get(l, "minion");
            let lm = (!lminion.is_nil()).then(|| {
                (
                    text(&get(&lminion, "type")),
                    get(&lminion, "level").as_number(),
                    fields(&get(&lminion, "weaponData1")),
                )
            });
            let tm = m
                .minion
                .as_ref()
                .map(|x| (Some(x.typ.clone()), Some(x.level), x.weapon_data[0].clone()));
            if tm != lm {
                fail(format!("{what}: minion {tm:?} != {lm:?}"));
            }
        }
        // The same from the typed build alone: its configuration, then the
        // whole setup
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
                fail(format!("build: {e}"));
                continue;
            }
        };
        let lconfig = get(&build, "configTab");
        let lset = rt::index(
            &get(&lconfig, "configSets"),
            &get(&lconfig, "activeConfigSetId"),
        )
        .unwrap();
        let tset = typed_build.config.active();
        for (what, t, l) in [
            ("input", &tset.input, inputs(&get(&lset, "input"))),
            (
                "placeholder",
                &tset.placeholder,
                inputs(&get(&lset, "placeholder")),
            ),
        ] {
            let t: BTreeMap<_, _> = t
                .iter()
                .map(|(k, v)| (k.clone(), format!("{v:?}")))
                .collect();
            let l: BTreeMap<_, _> = l
                .iter()
                .map(|(k, v)| (k.clone(), format!("{v:?}")))
                .collect();
            if t != l {
                let diff: Vec<_> = t
                    .keys()
                    .chain(l.keys())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .filter(|k| t.get(*k) != l.get(*k))
                    .map(|k| format!("{k}: {:?} != {:?}", t.get(k), l.get(k)))
                    .collect();
                fail(format!("config {what}: {}", diff.join(", ")));
            }
        }
        if typed_build.config_mods != config_mods {
            fail(format!(
                "config mods: {} != {}",
                typed_build.config_mods.len(),
                config_mods.len()
            ));
        }
        if typed_build.config_enemy_mods != config_enemy_mods {
            fail("config enemy mods differ".into());
        }
        let calc_data = engine::calc_env::CalcData {
            setup: &setup_data,
            items: &item_data,
            skills: &skill_data,
            parser: &parser,
            jewel_radius: &radius,
        };
        match engine::calc_env::init_env(
            &mut typed_build,
            &calc_data,
            BuffMode::new(Some("EFFECTIVE")),
        ) {
            Ok(typed_env) => {
                let same_db = |a: &Store, b: &Store| {
                    typed_mods(a) == typed_mods(b)
                        && a.conditions == b.conditions
                        && a.multipliers == b.multipliers
                };
                if !same_db(&typed_env.player_db, &player_db) {
                    fail("init_env: the player's database differs".into());
                }
                if !same_db(&typed_env.enemy_db, &enemy_base) {
                    fail("init_env: the enemy's database differs".into());
                }
                let skill_mods = |s: &[engine::calc_skills::ActiveSkill]| -> Vec<_> {
                    s.iter()
                        .map(|a| {
                            let m = a.mods.as_ref();
                            (
                                a.active_effect.granted_effect.id.clone(),
                                m.map(|m| m.mods.clone()),
                                m.map(|m| m.skill_flags.clone()),
                            )
                        })
                        .collect()
                };
                if skill_mods(&typed_env.skills.active_skills) != skill_mods(&setup.active_skills) {
                    fail("init_env: the active skills differ".into());
                }
            }
            Err(e) => fail(format!("init_env: {e}")),
        }
    }
    eprintln!("{compared} item setups compared, {skipped} skipped");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
