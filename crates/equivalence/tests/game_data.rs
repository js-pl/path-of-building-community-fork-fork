//! The typed game data the `gamedata` crate makes from upstream's data
//! files, written in its binary form and read back, equals what the typed
//! tables' `from_lua` read from the `data` table of a started upstream
//! host. Prints the data's size and how long making, reading and the host
//! take.

use engine::game_data::GameData;
use engine::item_data::ItemData;
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
        if got.base(&b.name) != Some(b) {
            out.push(format!("base {}", b.name));
        }
    }
    let mut groups: Vec<_> = want.item_mods.keys().chain(got.item_mods.keys()).collect();
    groups.sort();
    groups.dedup();
    for g in groups {
        match (got.item_mods.get(g), want.item_mods.get(g)) {
            (Some(a), Some(b)) if a != b => {
                let first = a.list.iter().zip(&b.list).position(|(x, y)| x != y);
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
            if got.$f != want.$f {
                out.push(stringify!($f).into());
            }
        )*};
    }
    fields!(
        rare_like_uniques,
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
    assert!(data == built, "the binary form does not read back equal");
    assert_eq!(data.encode(), bytes, "encoding is not deterministic");

    let t = Instant::now();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", lua::to_string(e.value())));
    let start = t.elapsed();
    let t = Instant::now();
    let items = ItemData::from_lua(&host.global("data"), &host.global("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let from_lua = t.elapsed();
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&bytes, 6);
    eprintln!(
        "typed data: {} bytes, {} compressed; made in {make:?}, read in {read:?}; \
         upstream host started in {start:?}, from_lua in {from_lua:?}",
        bytes.len(),
        packed.len()
    );
    let diffs = diff_items(&data.items, &items);
    assert!(diffs.is_empty(), "item data differs: {diffs:#?}");
    assert!(data.items == items);
}
