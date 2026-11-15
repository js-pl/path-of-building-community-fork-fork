//! The typed game data the `gamedata` crate makes from upstream's data
//! files, written in its binary form and read back, equals what the typed
//! tables' `from_lua` read from the `data` table of upstream started on the
//! oracle (`oracle/values.lua`'s `data`). Values compare in the binary form, so NaN placeholders compare
//! equal; a skill's own stat maps compare by what they resolve to, as
//! upstream copies shared ones in on first use. Prints the data's size
//! and how long making it, reading it and starting the host take.

use engine::calc_setup::SetupData;
use engine::codec::{self, Codec};
use engine::config::ConfigData;
use engine::game_data::GameData;
use engine::item_data::ItemData;
use engine::skill_data::{GrantedEffect, SkillData};
use engine::tree::PassiveTree;
use equivalence::{lua_json, upstream_src};
use lua::rt;
use session::Session;
use std::time::{Duration, Instant};

fn diff_items(got: &ItemData, want: &ItemData) -> Vec<String> {
    let mut out = Vec::new();
    let names = |d: &ItemData| d.bases().iter().map(|b| b.name.clone()).collect::<Vec<_>>();
    if names(got) != names(want) {
        out.push("base names or order".into());
    }
    for b in want.bases() {
        if !same(&got.base(&b.name).cloned(), &Some(b.clone())) {
            out.push(format!("base {}", b.name));
        }
    }
    let mut groups: Vec<_> = want.item_mods.keys().chain(got.item_mods.keys()).collect();
    groups.sort();
    groups.dedup();
    for g in groups {
        match (got.item_mods.get(g), want.item_mods.get(g)) {
            (Some(a), Some(b)) if !same(&a.list, &b.list) => {
                let first = a.list.iter().zip(&b.list).position(|(x, y)| !same(x, y));
                out.push(format!(
                    "itemMods.{g}: {} vs {} affixes, first difference at {first:?}",
                    a.list.len(),
                    b.list.len()
                ));
            }
            (Some(_), Some(_)) => {}
            (a, _) => out.push(format!(
                "itemMods.{g} only in {}",
                if a.is_some() { "got" } else { "want" }
            )),
        }
    }
    for (k, u) in &want.rare_like_uniques {
        if got.rare_like_uniques.get(k) != Some(u) {
            out.push(format!("rareLikeUniques {k}"));
        }
    }
    macro_rules! fields {
        ($($f:ident),*) => {$(
            if !same(&got.$f, &want.$f) {
                out.push(stringify!($f).into());
            }
        )*};
    }
    fields!(
        foulborn_map,
        cluster_jewels,
        cluster_notable_sort_order,
        cluster_orbit_offsets,
        mod_scalability,
        high_precision_mods,
        item_tag_special,
        item_tag_special_exclusion,
        default_high_precision,
        color_codes,
        caster_tag_crucible_uniques,
        minion_tag_crucible_uniques,
        unique_stat_order_exact,
        unique_stat_order_normalised
    );
    out
}

/// Equal in the binary form, so with NaN equal to itself
fn same<T: Codec>(a: &T, b: &T) -> bool {
    codec::encode(a) == codec::encode(b)
}

/// The keys whose values differ between two maps
fn diff_map<K: Ord + std::hash::Hash + std::fmt::Debug, V: Codec>(
    what: &str,
    got: &std::collections::HashMap<K, V>,
    want: &std::collections::HashMap<K, V>,
) -> Vec<String> {
    let mut keys: Vec<_> = got.keys().chain(want.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|k| got.get(k).map(codec::encode) != want.get(k).map(codec::encode))
        .map(|k| format!("{what} {k:?}"))
        .collect()
}

fn diff_skills(got: &SkillData, want: &SkillData) -> Vec<String> {
    let mut out = Vec::new();
    let mut ids: Vec<_> = got.skills.keys().chain(want.skills.keys()).collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        let (Some(a), Some(b)) = (got.skills.get(id), want.skills.get(id)) else {
            out.push(format!("skill {id} on one side only"));
            continue;
        };
        if same(a, b) {
            continue;
        }
        // Upstream copies shared stat maps into a skill's own on first use
        let own = |e: &GrantedEffect| GrantedEffect {
            stat_map: Default::default(),
            ..e.clone()
        };
        let mut stats: Vec<_> = a.stat_map.keys().chain(b.stat_map.keys()).collect();
        stats.sort();
        stats.dedup();
        let resolve = |d: &SkillData, e, k| d.stat_map(e, k).map(|m| codec::encode(&*m));
        if !same(&own(a), &own(b))
            || stats
                .iter()
                .any(|k| resolve(got, a, k) != resolve(want, b, k))
        {
            out.push(format!("skill {id}"));
        }
    }
    out.extend(diff_map(
        "stat map",
        &got.skill_stat_map,
        &want.skill_stat_map,
    ));
    for g in want.gems() {
        let plain = |g: &engine::skill_data::Gem| engine::skill_data::Gem {
            granted_effect: Default::default(),
            secondary_granted_effect: None,
            ..g.clone()
        };
        let effects = |g: &engine::skill_data::Gem| {
            codec::encode(&(
                (*g.granted_effect).clone(),
                g.secondary_granted_effect.as_deref().cloned(),
            ))
        };
        if got
            .gem(&g.id)
            .is_none_or(|x| plain(x) != plain(g) || effects(x) != effects(g))
        {
            out.push(format!("gem {}", g.id));
        }
    }
    let lookups = |d: &SkillData| {
        let mut d = d.clone();
        d.skills.clear();
        d
    };
    if !same(&lookups(got), &lookups(want)) {
        out.push("gem lookups".into());
    }
    out
}

fn diff_setup(got: &SetupData, want: &SetupData) -> Vec<String> {
    let mut out = diff_map("minion", &got.minions, &want.minions);
    macro_rules! fields {
        ($($f:ident),*) => {$(
            if !same(&got.$f, &want.$f) {
                out.push(stringify!($f).into());
            }
        )*};
    }
    fields!(
        character_constants,
        misc,
        monster_accuracy,
        pantheons,
        mod_flags,
        keyword_flags,
        game_constants,
        weapon_type_info,
        monster_damage_table,
        monster_ally_damage_table,
        unarmed_weapon_data,
        non_damaging_ailment,
        curse_priority,
        monster_constants,
        ailment_types,
        monster_phys_conversion_multi,
        elemental_ailment_types,
        non_damaging_ailment_types,
        non_elemental_ailment_types,
        tables,
        cost_divisors
    );
    out
}

fn diff_config(got: &ConfigData, want: &ConfigData) -> Vec<String> {
    let mut out = diff_map("boss skill", &got.boss_skills, &want.boss_skills);
    if out.is_empty() && !same(got, want) {
        out.push(format!("{got:?}\nvs\n{want:?}"));
    }
    out
}

/// `Data.lua`'s `processMod` sets the source of every mod of a skill (its
/// base, quality and level mods and its stat map's) to `Skill:<id>`, in
/// `pairs(data.skills)` order; a gem and its transfigured variants share
/// some of those tables, so the last skill `pairs` visits names them, which
/// follows LuaJIT's per-process string hash. Shared tables get the source
/// of the first of their skills by id, on both sides.
fn shared_mod_sources(data: &lua::Value) {
    use lua::value::TableRef;
    use std::collections::HashMap;
    let get = |v: &lua::Value, k: &str| rt::index(v, &lua::Value::str(k)).unwrap_or_default();
    let tables = |v: &lua::Value| -> Vec<TableRef> {
        v.as_table()
            .map(|t| t.entries())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(_, v)| v.as_table().cloned())
            .collect()
    };
    let mut owners: HashMap<*const (), (TableRef, Vec<String>)> = HashMap::new();
    let skills = get(data, "skills");
    for (id, ge) in skills.as_table().map(|t| t.entries()).unwrap_or_default() {
        let id = lua::to_string(&id.to_value()).to_str_lossy().into_owned();
        let mut lists: Vec<TableRef> = ["baseMods", "qualityMods", "levelMods"]
            .iter()
            .filter_map(|k| get(&ge, k).as_table().cloned())
            .collect();
        lists.extend(tables(&get(&ge, "statMap")));
        for list in lists {
            for m in tables(&lua::Value::Table(list)) {
                let mods = if m.get_str("name").is_nil() {
                    tables(&lua::Value::Table(m))
                } else {
                    vec![m]
                };
                for m in mods {
                    let inner = get(&m.get_str("value"), "mod");
                    for t in std::iter::once(m).chain(inner.as_table().cloned()) {
                        let p = std::rc::Rc::as_ptr(&t.0) as *const ();
                        owners
                            .entry(p)
                            .or_insert((t, Vec::new()))
                            .1
                            .push(id.clone());
                    }
                }
            }
        }
    }
    for (t, mut ids) in owners.into_values() {
        ids.sort();
        ids.dedup();
        if ids.len() > 1 && !t.get_str("source").is_nil() {
            t.set_str("source", lua::Value::str(&format!("Skill:{}", ids[0])));
        }
    }
}

/// The typed data's orders that follow LuaJIT's `pairs` over number keys,
/// the same in every run, against the oracle's (`values.lua`'s
/// `pairsOrder`): the comparison above rebuilds every table in sorted order
fn pairs_order_diffs(built: &GameData, order: &lua::Value) -> Vec<String> {
    let get = |v: &lua::Value, k: &str| rt::index(v, &lua::Value::str(k)).unwrap_or_default();
    let strings = |v: &lua::Value| -> Vec<String> {
        v.as_table()
            .map(|t| t.array())
            .unwrap_or_default()
            .iter()
            .map(|s| lua::to_string(s).to_str_lossy().into_owned())
            .collect()
    };
    let mut out = Vec::new();
    let first = get(order, "firstLevels");
    for (id, ge) in &built.skills.skills {
        let want = get(&first, id).as_number().map(|n| n as i64);
        if ge.first_level != want {
            out.push(format!(
                "skill {id} first level {:?} != {want:?}",
                ge.first_level
            ));
        }
    }
    let pantheons = get(order, "pantheonLines");
    for (god, (_, lines)) in &built.setup.pantheons {
        let want: Vec<Vec<String>> = get(&pantheons, god)
            .as_table()
            .map(|t| t.array())
            .unwrap_or_default()
            .iter()
            .map(strings)
            .collect();
        if *lines != want {
            out.push(format!("pantheon {god} mod lines {lines:?} != {want:?}"));
        }
    }
    let flavour = get(order, "flavourIds");
    for (name, list) in &built.items.flavour_text {
        let ids: Vec<String> = list.iter().map(|(id, _)| id.clone()).collect();
        let want = strings(&get(&flavour, name));
        if ids != want {
            out.push(format!("flavour text {name:?} ids {ids:?} != {want:?}"));
        }
    }
    out
}

#[test]
fn typed_data_matches_upstream() {
    let t = Instant::now();
    let src = gamedata::Source::load(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let built = src.game_data().unwrap_or_else(|e| panic!("{e}"));
    let make = t.elapsed();
    let bytes = built.encode();
    let t = Instant::now();
    let data = GameData::decode(&bytes).unwrap_or_else(|e| panic!("{e}"));
    let read = t.elapsed();
    let rt: Vec<String> = [
        diff_items(&data.items, &built.items),
        diff_skills(&data.skills, &built.skills),
        diff_setup(&data.setup, &built.setup),
        diff_config(&data.config, &built.config),
    ]
    .concat();
    assert!(
        rt.is_empty(),
        "the binary form reads back different: {rt:#?}"
    );
    assert_eq!(data.encode(), bytes, "encoding is not deterministic");

    let t = Instant::now();
    let oracle = Session::oracle(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let req = serde_json::json!({"cmd": "oracleValue", "fn": "data", "params": {}});
    let globals = oracle
        .request(&req.to_string())
        .and_then(|r| lua_json::answer(&r))
        .unwrap_or_else(|e| panic!("{e}"));
    let global = |k: &str| rt::index(&globals, &lua::Value::str(k)).unwrap_or_default();
    let start = t.elapsed();
    let t = Instant::now();
    // The flags as after every skill's first calculation, as the typed
    // data has them; the copied stat maps stay out (values.lua does both)
    let ldata = global("data");
    shared_mod_sources(&ldata);
    let items = ItemData::from_lua(&ldata, &global("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let skills = SkillData::from_lua(&ldata, &global("colorCodes"), &global("SkillType"))
        .unwrap_or_else(|e| panic!("skill data: {e}"));
    let setup = SetupData::from_lua(&ldata, &global("ModFlag"), &global("KeywordFlag"))
        .unwrap_or_else(|e| panic!("setup data: {e}"));
    let config = ConfigData::from_lua(&ldata, &global("SkillType"))
        .unwrap_or_else(|e| panic!("config data: {e}"));
    let from_lua = t.elapsed();
    // The importer's data the typed data is made of, its tables rebuilt as
    // the oracle's are: upstream builds the lists of string-keyed tables
    // (item mods and bases, record fields) in `pairs` order, which follows
    // LuaJIT's per-process string hash
    let idata = lua_json::canonical(&lua::Value::Table(src.data.clone()));
    shared_mod_sources(&idata);
    let iglobal = |k: &str| lua_json::canonical(&src.global(k));
    let got_items = ItemData::from_lua(&idata, &iglobal("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let got_skills = SkillData::from_lua(&idata, &iglobal("colorCodes"), &iglobal("SkillType"))
        .unwrap_or_else(|e| panic!("skill data: {e}"));
    let got_setup = SetupData::from_lua(&idata, &iglobal("ModFlag"), &iglobal("KeywordFlag"))
        .unwrap_or_else(|e| panic!("setup data: {e}"));
    let got_config = ConfigData::from_lua(&idata, &iglobal("SkillType"))
        .unwrap_or_else(|e| panic!("config data: {e}"));
    let var_list = global("configOptions");
    assert_eq!(
        data.option_defaults,
        engine::config_tab::option_defaults(&var_list)
    );
    let radius: Vec<String> = ldata
        .as_table()
        .unwrap()
        .get_str("jewelRadius")
        .as_table()
        .unwrap()
        .array()
        .iter()
        .map(|r| {
            lua::to_string(&r.as_table().unwrap().get_str("label"))
                .to_str_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(data.jewel_radius, radius);
    assert_eq!(
        data.latest_tree_version(),
        lua::to_string(&global("latestTreeVersion")).to_str_lossy()
    );
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&bytes, 6);
    eprintln!(
        "typed data: {} bytes, {} compressed; made in {make:?}, read in {read:?}; \
         upstream's data from the oracle in {start:?}, from_lua in {from_lua:?}",
        bytes.len(),
        packed.len()
    );
    let mut diffs = pairs_order_diffs(&built, &global("pairsOrder"));
    diffs.extend(
        [
            diff_items(&got_items, &items),
            diff_skills(&got_skills, &skills),
            diff_setup(&got_setup, &setup),
            diff_config(&got_config, &config),
        ]
        .concat(),
    );
    assert!(diffs.is_empty(), "typed data differs: {diffs:#?}");
}

/// Every tree version reads back equal from its binary form
#[test]
fn trees_round_trip() {
    let src = gamedata::Source::load(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let parser = src.mod_parser().unwrap_or_else(|e| panic!("{e}"));
    let versions = src.tree_versions().unwrap_or_else(|e| panic!("{e}"));
    let (mut size, mut packed, mut make, mut read) = (0, 0, Duration::ZERO, Duration::ZERO);
    for v in &versions {
        let t = Instant::now();
        let tree = src
            .tree(v, &parser, v == versions.last().unwrap())
            .unwrap_or_else(|e| panic!("{v}: {e}"));
        make += t.elapsed();
        let bytes = tree.encode();
        let t = Instant::now();
        let back = PassiveTree::decode(&bytes).unwrap_or_else(|e| panic!("{v}: {e}"));
        read += t.elapsed();
        assert!(back.encode() == bytes, "{v} reads back different");
        size += bytes.len();
        packed += miniz_oxide::deflate::compress_to_vec_zlib(&bytes, 6).len();
    }
    eprintln!(
        "{} trees: {size} bytes, {packed} compressed; made in {make:?}, read in {read:?}",
        versions.len()
    );
}
