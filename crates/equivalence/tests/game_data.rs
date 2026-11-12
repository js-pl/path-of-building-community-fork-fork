//! The typed game data the `gamedata` crate makes from upstream's data
//! files, written in its binary form and read back, equals what the typed
//! tables' `from_lua` read from the `data` table of a started upstream
//! host. Values compare in the binary form, so NaN placeholders compare
//! equal; a skill's own stat maps compare by what they resolve to, as
//! upstream copies shared ones in on first use. Prints the data's size
//! and how long making it, reading it and starting the host take.

use engine::calc_setup::SetupData;
use engine::codec::{self, Codec};
use engine::config::ConfigData;
use engine::game_data::GameData;
use engine::item_data::ItemData;
use engine::skill_data::{GrantedEffect, SkillData};
use equivalence::upstream_src;
use std::rc::Rc;
use std::time::Instant;
use upstream::host::Host;
use upstream::imported::Imported;

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

#[test]
fn typed_data_matches_upstream() {
    let t = Instant::now();
    let built = gamedata::Source::load(&upstream_src())
        .and_then(|s| s.game_data())
        .unwrap_or_else(|e| panic!("{e}"));
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
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", lua::to_string(e.value())));
    let start = t.elapsed();
    let t = Instant::now();
    let ldata = host.global("data");
    let items = ItemData::from_lua(&ldata, &host.global("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let skills = SkillData::from_lua(
        &ldata,
        &host.global("colorCodes"),
        &host.global("SkillType"),
    )
    .unwrap_or_else(|e| panic!("skill data: {e}"));
    let setup = SetupData::from_lua(&ldata, &host.global("ModFlag"), &host.global("KeywordFlag"))
        .unwrap_or_else(|e| panic!("setup data: {e}"));
    let config = ConfigData::from_lua(&ldata, &host.global("SkillType"))
        .unwrap_or_else(|e| panic!("config data: {e}"));
    let from_lua = t.elapsed();
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&bytes, 6);
    eprintln!(
        "typed data: {} bytes, {} compressed; made in {make:?}, read in {read:?}; \
         upstream host started in {start:?}, from_lua in {from_lua:?}",
        bytes.len(),
        packed.len()
    );
    let diffs: Vec<String> = [
        diff_items(&data.items, &items),
        diff_skills(&data.skills, &skills),
        diff_setup(&data.setup, &setup),
        diff_config(&data.config, &config),
    ]
    .concat();
    assert!(diffs.is_empty(), "typed data differs: {diffs:#?}");
}
