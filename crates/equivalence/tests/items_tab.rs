//! The typed Items tab (`engine::items_tab`) equips what the translated
//! program's does, for every corpus build loaded from XML: each slot's item
//! and whether it is active, the active item set, and the active spec's
//! jewels (`engine::spec::PassiveSpec::load`). Builds whose specs use what
//! the typed spec does not port yet are skipped.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::item::ItemContext;
use engine::item_data::ItemData;
use engine::items_tab::ItemsTab;
use engine::spec::PassiveSpec;
use engine::tree::{self, PassiveTree};
use equivalence::{corpus, load_data, mod_parser, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
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

fn text(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_str_lossy().into_owned())
}

fn array(v: &Value) -> Vec<Value> {
    v.as_table().map(|t| t.array()).unwrap_or_default()
}

#[test]
fn items_tabs_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let (data, _) = load_data();
    let item_data = ItemData::from_lua(&host.global("data"), &host.global("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let latest = lua::to_string(&host.global("latestTreeVersion")).to_string();
    let weapon_types = get(&host.global("data"), "weaponTypeInfo");
    let one_hand = |t: &str| -> Option<bool> {
        let info = get(&weapon_types, t);
        (!info.is_nil()).then(|| get(&info, "oneHand").truthy())
    };
    let mut trees: HashMap<String, PassiveTree> = HashMap::new();
    let mut load_tree = |version: &str, trees: &mut HashMap<String, PassiveTree>| {
        if !trees.contains_key(version) {
            let tree_data = im
                .load1(&format!("TreeData/{version}/tree"), vec![])
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            let radii = tree::jewel_radii(&data.get("jewelRadii"), version).unwrap();
            let t = PassiveTree::new(version, &tree_data, &radii, &parser, version == latest)
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            trees.insert(version.to_string(), t);
        }
    };
    load_tree(&latest, &mut trees);
    let socket_ids: Vec<u32> = trees[&latest]
        .nodes
        .iter()
        .filter(|n| n.kind == tree::NodeType::Socket)
        .map(|n| n.id)
        .collect();
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut skipped = 0;
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
        let radius: Vec<String> = array(&get(&host.global("data"), "jewelRadius"))
            .iter()
            .map(|r| text(&get(r, "label")).unwrap_or_default())
            .collect();
        let ctx = ItemContext {
            data: &item_data,
            parser: &parser,
            jewel_radius: &radius,
            default_affix_quality: 0.5,
        };
        let mut tab = match ItemsTab::load(items_xml, &ctx, &socket_ids) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{id}: typed failed: {e}"));
                continue;
            }
        };
        let specs: Vec<_> = tree_xml.elements().filter(|e| e.elem == "Spec").collect();
        let active = tree_xml
            .attr("activeSpec")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .min(specs.len());
        let spec_xml = specs[active - 1];
        let version = spec_xml.attr("treeVersion").unwrap_or("2_6").to_string();
        load_tree(&version, &mut trees);
        let tree = &trees[&version];
        let mut spec = PassiveSpec::new(tree, &parser).unwrap();
        if let Err(e) = spec.load(spec_xml).and_then(|_| spec.post_load()) {
            eprintln!("{id}: skipped: {e}");
            skipped += 1;
            continue;
        }
        tab.populate_slots(&mut spec, tree, &one_hand);
        compared += 1;
        let mut fail = |what: String| {
            if failures.len() < 60 {
                failures.push(format!("{id}: {what}"));
            }
        };
        let ltab = get(&build, "itemsTab");
        let lactive = get(&ltab, "activeItemSetId").as_number();
        if lactive != Some(tab.active_item_set_id) {
            fail(format!(
                "activeItemSetId: {} != {lactive:?}",
                tab.active_item_set_id
            ));
        }
        let lslots = array(&get(&ltab, "orderedSlots"));
        if lslots.len() != tab.ordered_slots.len() {
            fail(format!(
                "{} slots != {}",
                tab.ordered_slots.len(),
                lslots.len()
            ));
        }
        for (t, l) in tab.ordered_slots.iter().zip(&lslots) {
            let name = text(&get(l, "slotName"));
            let sel = get(l, "selItemId").as_number();
            let active = get(l, "active").truthy();
            let num = get(l, "slotNum").as_number();
            if name.as_deref() != Some(t.slot_name.as_str())
                || sel != Some(t.sel_item_id)
                || active != t.active
                || num != t.slot_num
            {
                fail(format!(
                    "slot {:?} {} active {} num {:?} != {name:?} {sel:?} {active} {num:?}",
                    t.slot_name, t.sel_item_id, t.active, t.slot_num
                ));
            }
        }
        let ljewels: BTreeMap<u32, f64> = get(&get(&build, "spec"), "jewels")
            .as_table()
            .map(|t| t.entries())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| match k {
                Key::Int(i) => Some((i as u32, v.as_number()?)),
                _ => None,
            })
            .collect();
        if ljewels != spec.jewels {
            fail(format!("jewels: {:?} != {ljewels:?}", spec.jewels));
        }
    }
    eprintln!("{compared} items tabs compared, {skipped} skipped");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
