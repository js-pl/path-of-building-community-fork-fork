//! Item, gem and anoint power (`engine::compare`): the Items tab's item
//! database sort, the Skills tab's gem dropdown sort and the anoint
//! popup's sort run the build's calculator over each candidate and sort by
//! a power stat, like node power. Each candidate's power equals the
//! translated `calcFunc`'s on the same candidate, over a sample of corpus
//! builds; the threaded runs equal the sequential one.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::calc_override::Override;
use engine::calculator::MiscCalculator;
use engine::compare::{self, GemCandidate};
use engine::item::Item;
use engine::parallel::Threads;
use engine::power::{PowerStat, power_stats};
use engine::skills::{GemInstance, process_gem_level};
use equivalence::outputs::number;
use equivalence::typed_build::TypedData;
use equivalence::{corpus_builds, upstream_src};
use lua::{LuaError, TableRef, Value, rt};
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

/// A new `Item` from upstream's host (`new("Item"):Item(raw, rarity,
/// true)`)
fn lua_item(host: &Host, raw: &str, rarity: &str) -> Result<Value, String> {
    let new = host.global("new");
    rt::call(&new, &[s("Item")])
        .and_then(|v| {
            rt::method(
                &rt::first(v),
                &"Item".into(),
                &[s(raw), s(rarity), Value::Bool(true)],
            )
        })
        .map(rt::first)
        .map_err(|e| describe(&e))
}

/// `data.powerStatList.GetFromOutput(output, statTable)`
fn lua_value(power_list: &Value, stat_table: &Value, output: &Value) -> f64 {
    let f = get(power_list, "GetFromOutput");
    rt::call(&f, &[output.clone(), stat_table.clone()])
        .ok()
        .and_then(|r| r.into_iter().next())
        .and_then(|v| v.as_number())
        .unwrap_or(f64::NAN)
}

/// The Lua `powerStatList` entry with the label
fn lua_stat(power_list: &Value, label: &str) -> Value {
    equivalence::outputs::entries(power_list)
        .into_iter()
        .map(|(_, v)| v)
        .find(|v| lua::to_string(&get(v, "label")).to_str_lossy() == label)
        .unwrap_or_default()
}

/// The valid, shown slots for an item, as the item database sort iterates
/// (`IsItemValidForSlot` and `slot.shown()`, without the ascendancy-only
/// Ring 3 and the Graft slots)
fn item_slots(
    build: &engine::build::Build,
    item: &Item,
    one_hand: &dyn Fn(&str) -> Option<bool>,
) -> Vec<String> {
    let tab = &build.items;
    let second = tab.active_item_set().use_second_weapon_set;
    tab.ordered_slots
        .iter()
        .filter(|sl| sl.node_id.is_none())
        .filter(|sl| match sl.weapon_set {
            Some(1) => !second,
            Some(_) => second,
            None => true,
        })
        .filter(|sl| !matches!(sl.slot_name.as_str(), "Ring 3" | "Graft 1" | "Graft 2"))
        .filter(|sl| tab.valid_for_slot(item, &sl.slot_name, build.spec.tree, one_hand))
        .map(|sl| sl.slot_name.clone())
        .collect()
}

/// Builds for the sample and the stat to sort them by
fn sample() -> Vec<(String, String, &'static str)> {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let stats = ["Life", "Full DPS", "Armour", "Mana"];
    corpus_builds()
        .into_iter()
        .enumerate()
        .filter(|(i, (id, _))| {
            if filter.is_empty() {
                i % 12 == 5
            } else {
                id.contains(&filter)
            }
        })
        .enumerate()
        .map(|(n, (_, (id, xml)))| (id, xml, stats[n % stats.len()]))
        .collect()
}

#[test]
fn item_power_matches_upstream() {
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = TypedData::load();
    let uniques = equivalence::variants::uniques();
    let mut failures = Vec::new();
    let (mut compared, mut items_compared) = (0, 0);
    for (id, xml, label) in sample() {
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let lbuild = host.build().unwrap();
        if let Some(t) = lbuild.as_table() {
            t.set_str("viewMode", s("ITEMS"));
        }
        let misc = get(&get(&lbuild, "calcsTab"), "miscCalculator");
        let calc_func = rt::index(&misc, &Value::Number(1.0)).unwrap();
        let power_list = get(&host.global("data"), "powerStatList");
        let stat_table = lua_stat(&power_list, label);

        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        data.load_tree(&root);
        let one_hand = |t: &str| data.setup.weapon_type_info.get(t).map(|w| w.one_hand);
        let mut build = match data.load_build(&root) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{id}: build: {e}"));
                continue;
            }
        };
        let calc_data = data.calc_data();
        let calc = match engine::calcs::build_outputs(&mut build, &calc_data)
            .and_then(|_| MiscCalculator::new(&build, &calc_data))
        {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{id}: typed calculator failed: {e}"));
                continue;
            }
        };
        let stat: PowerStat = power_stats()
            .into_iter()
            .find(|s| s.label == label)
            .unwrap();

        // Sample unique items that fit at least one slot
        let ctx = engine::item::ItemContext {
            data: &data.items,
            parser: &data.parser,
            jewel_radius: &data.radius,
            default_affix_quality: 0.5,
        };
        let mut sampled: Vec<(Item, Value, Vec<String>)> = Vec::new();
        for raw in &uniques {
            if sampled.len() >= 6 {
                break;
            }
            let Ok(item) = Item::new(&ctx, raw, Some("UNIQUE"), true) else {
                continue;
            };
            // Flask and tincture DB items toggle by an equipped id, which a
            // database item has none of; the item sort skips them here
            if matches!(item.typ.as_deref(), Some("Flask") | Some("Tincture")) {
                continue;
            }
            let slots = item_slots(&build, &item, &one_hand);
            if slots.is_empty() {
                continue;
            }
            let Ok(li) = lua_item(&host, raw, "UNIQUE") else {
                continue;
            };
            if let Some(t) = li.as_table() {
                t.set_str("id", Value::Number(-100.0));
            }
            sampled.push((item, li, slots));
        }

        // Typed: each item's power over its slots
        let overrides: Vec<Vec<Override>> = sampled
            .iter()
            .map(|(item, _, slots)| {
                slots
                    .iter()
                    .map(|sl| Override {
                        rep_item: Some((sl.clone(), Some(item.clone()))),
                        ..Override::default()
                    })
                    .collect()
            })
            .collect();
        let seq = compare::item_power(
            &calc,
            overrides.clone(),
            &stat,
            compare::uses_full_dps(&stat),
            Threads::Sequential,
        );
        let par = compare::item_power(
            &calc,
            overrides,
            &stat,
            compare::uses_full_dps(&stat),
            Threads::Pool(4),
        );

        let mut typed = BTreeMap::new();
        let mut want = BTreeMap::new();
        let full_dps = stat.stat.as_deref() == Some("FullDPS");
        for (i, (_, li, slots)) in sampled.iter().enumerate() {
            let key = format!("item {i}");
            match &seq[i] {
                Ok(v) => {
                    typed.insert(key.clone(), number(*v));
                }
                Err(e) => {
                    failures.push(format!("{id}: item {i}: typed failed: {e}"));
                    continue;
                }
            }
            if seq[i] != par[i] {
                failures.push(format!("{id}: item {i}: threaded differs from sequential"));
            }
            // Lua: max over the slots
            let mut best = f64::NEG_INFINITY;
            for sl in slots {
                let ovr = TableRef::new();
                ovr.set_str("repSlotName", s(sl));
                ovr.set_str("repItem", li.clone());
                let out = match rt::call(&calc_func, &[Value::Table(ovr), Value::Bool(full_dps)]) {
                    Ok(r) => r.into_iter().next().unwrap_or_default(),
                    Err(e) => {
                        failures.push(format!("{id}: item {i} {sl}: translated: {}", describe(&e)));
                        best = f64::NAN;
                        break;
                    }
                };
                best = best.max(lua_value(&power_list, &stat_table, &out));
            }
            want.insert(key, number(best));
        }
        items_compared += want.len();
        let mut fail = Vec::new();
        equivalence::outputs::compare(label, &typed, &want, &mut fail);
        if !fail.is_empty() {
            failures.push(format!("{id}: {}", fail.join("\n    ")));
        }
        compared += 1;
    }
    eprintln!("{compared} builds, {items_compared} items compared");
    assert!(compared > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Support gems to swap into the display socket group, by name
const SUPPORTS: &[&str] = &[
    "Increased Critical Strikes",
    "Added Fire Damage",
    "Faster Attacks",
    "Faster Casting",
    "Elemental Focus",
    "Increased Area of Effect",
];

#[test]
fn gem_power_matches_upstream() {
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = TypedData::load();
    let fields = ["TotalDPS", "CombinedDPS"];
    let mut failures = Vec::new();
    let (mut compared, mut gems_compared) = (0, 0);
    for (n, (id, xml, _)) in sample().into_iter().enumerate() {
        let field = fields[n % fields.len()];
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let lbuild = host.build().unwrap();
        if let Some(t) = lbuild.as_table() {
            t.set_str("viewMode", s("ITEMS"));
        }
        let lskills = get(&lbuild, "skillsTab");
        let misc = get(&get(&lbuild, "calcsTab"), "miscCalculator");
        let calc_func = rt::index(&misc, &Value::Number(1.0)).unwrap();
        let lgems = get(&host.global("data"), "gems");
        let main = get(&lbuild, "mainSocketGroup").as_number().unwrap_or(1.0);

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
        let calc = match engine::calcs::build_outputs(&mut build, &calc_data)
            .and_then(|_| MiscCalculator::new(&build, &calc_data))
        {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{id}: typed calculator failed: {e}"));
                continue;
            }
        };
        let main_i = main as usize - 1;
        let quality = build.skills.as_ref().map_or(0.0, |s| s.default_gem_quality);
        let level_mode = build.skills.as_ref().map(|s| s.default_gem_level);
        let char_level = build.info.character_level;

        let mut typed_cands = Vec::new();
        let mut lua_cands: Vec<(Value, f64)> = Vec::new();
        for name in SUPPORTS {
            let Some(gem) = data.skills.gems().iter().find(|g| g.name == *name).cloned() else {
                continue;
            };
            let lgem = rt::index(&lgems, &s(&gem.id)).unwrap_or_default();
            if lgem.is_nil() {
                continue;
            }
            let Some(mode) = level_mode else { continue };
            let level = process_gem_level(&gem, false, mode, char_level);

            let gi = GemInstance {
                name_spec: gem.name.clone(),
                gem_id: Some(gem.id.clone()),
                skill_id: Some(gem.granted_effect.id.clone()),
                gem: Some(gem.clone()),
                level: Some(level),
                quality: Some(quality),
                enabled: true,
                enable_global1: true,
                enable_global2: true,
                count: Some(1.0),
                natural_max_level: Some(gem.natural_max_level),
                ..GemInstance::default()
            };
            let mut b = build.clone();
            let groups = b
                .skills
                .as_mut()
                .and_then(|s| s.socket_group_list_mut())
                .unwrap();
            if main_i >= groups.len() {
                break;
            }
            groups[main_i].gem_list.push(gi);
            typed_cands.push(GemCandidate { build: b });

            // Upstream's gemInstance for the same gem
            let t = TableRef::new();
            t.set_str("level", Value::Number(level));
            t.set_str("quality", Value::Number(quality));
            t.set_str("count", Value::Number(1.0));
            t.set_str("enabled", Value::Bool(true));
            t.set_str("enableGlobal1", Value::Bool(true));
            t.set_str("enableGlobal2", Value::Bool(true));
            t.set_str("gemId", s(&gem.id));
            t.set_str("nameSpec", s(&gem.name));
            t.set_str("skillId", s(&gem.granted_effect.id));
            t.set_str("gemData", lgem);
            lua_cands.push((Value::Table(t), level));
        }
        if typed_cands.is_empty() {
            continue;
        }
        let full_dps = field == "FullDPS";

        let seq = compare::gem_power(
            &calc,
            typed_slice(&typed_cands),
            field,
            full_dps,
            Threads::Sequential,
        );
        let par = compare::gem_power(
            &calc,
            typed_slice(&typed_cands),
            field,
            full_dps,
            Threads::Pool(4),
        );

        // Lua: swap each gem into the main group and read the field
        let lsg = get(&lskills, "socketGroupList");
        let group = rt::index(&lsg, &Value::Number(main)).unwrap_or_default();
        let gem_list = get(&group, "gemList");
        let gl = gem_list.as_table().unwrap();
        let base_len = rt::len(&gem_list)
            .unwrap_or_default()
            .as_number()
            .unwrap_or(0.0);
        let idx = base_len + 1.0;

        let mut typed = BTreeMap::new();
        let mut want = BTreeMap::new();
        for (i, (gi, _)) in lua_cands.iter().enumerate() {
            let key = format!("gem {i}");
            match &seq[i] {
                Ok(v) => {
                    typed.insert(key.clone(), number(*v));
                }
                Err(e) => {
                    failures.push(format!("{id}: gem {i}: typed failed: {e}"));
                    continue;
                }
            }
            if seq[i] != par[i] {
                failures.push(format!("{id}: gem {i}: threaded differs from sequential"));
            }
            gl.set(Value::Number(idx), gi.clone());
            let out = match rt::call(&calc_func, &[Value::Nil, Value::Bool(full_dps)]) {
                Ok(r) => r.into_iter().next().unwrap_or_default(),
                Err(e) => {
                    failures.push(format!("{id}: gem {i}: translated: {}", describe(&e)));
                    gl.set(Value::Number(idx), Value::Nil);
                    continue;
                }
            };
            gl.set(Value::Number(idx), Value::Nil);
            want.insert(key, number(gem_field(&out, field)));
        }
        gems_compared += want.len();
        let mut fail = Vec::new();
        equivalence::outputs::compare(field, &typed, &want, &mut fail);
        if !fail.is_empty() {
            failures.push(format!("{id}: {}", fail.join("\n    ")));
        }
        compared += 1;
    }
    eprintln!("{compared} builds, {gems_compared} gems compared");
    assert!(compared > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Borrow the candidates for a run (each run consumes its vector)
fn typed_slice<'a>(cands: &[GemCandidate<'a>]) -> Vec<GemCandidate<'a>> {
    cands
        .iter()
        .map(|c| GemCandidate {
            build: c.build.clone(),
        })
        .collect()
}

/// `(dpsField == "FullDPS" and output[dpsField]) or (output.Minion and
/// output.Minion.CombinedDPS) or output[dpsField] or 0`, on a Lua output
fn gem_field(out: &Value, field: &str) -> f64 {
    let num = |v: &Value, k: &str| get(v, k).as_number();
    if field == "FullDPS"
        && let Some(v) = num(out, field)
    {
        return v;
    }
    let minion = get(out, "Minion");
    if !minion.is_nil()
        && let Some(v) = num(&minion, "CombinedDPS")
    {
        return v;
    }
    num(out, field).unwrap_or(0.0)
}

#[test]
fn anoint_power_matches_upstream() {
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = TypedData::load();
    let mut failures = Vec::new();
    let (mut compared, mut nodes_compared) = (0, 0);
    for (id, xml, label) in sample() {
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let lbuild = host.build().unwrap();
        if let Some(t) = lbuild.as_table() {
            t.set_str("viewMode", s("ITEMS"));
        }
        let litems = get(&lbuild, "itemsTab");
        let misc = get(&get(&lbuild, "calcsTab"), "miscCalculator");
        let calc_func = rt::index(&misc, &Value::Number(1.0)).unwrap();
        let power_list = get(&host.global("data"), "powerStatList");
        let stat_table = lua_stat(&power_list, label);

        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        data.load_tree(&root);
        let mut build = match data.load_build(&root) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{id}: build: {e}"));
                continue;
            }
        };
        // The equipped amulet, which the anoint popup anoints
        let amulet_id = build
            .items
            .ordered_slots
            .iter()
            .find(|sl| sl.slot_name == "Amulet")
            .map(|sl| sl.sel_item_id)
            .filter(|&id| build.items.item(id).is_some());
        let Some(amulet_id) = amulet_id else { continue };
        let rarity = build.items.item(amulet_id).unwrap().rarity.clone();

        let calc_data = data.calc_data();
        let calc = match engine::calcs::build_outputs(&mut build, &calc_data)
            .and_then(|_| MiscCalculator::new(&build, &calc_data))
        {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{id}: typed calculator failed: {e}"));
                continue;
            }
        };
        let stat: PowerStat = power_stats()
            .into_iter()
            .find(|s| s.label == label)
            .unwrap();
        let ctx = engine::item::ItemContext {
            data: &data.items,
            parser: &data.parser,
            jewel_radius: &data.radius,
            default_affix_quality: 0.5,
        };

        // Point the Lua anoint popup at the equipped amulet
        let lamulet = rt::index(&get(&litems, "items"), &Value::Number(amulet_id)).unwrap();
        if let Some(t) = litems.as_table() {
            t.set_str("displayItem", lamulet.clone());
            t.set_str("anointEnchantSlot", Value::Number(1.0));
        }
        // anointItem(node) → an Item; its raw text, parsed typed
        let anoint = |node: Value| -> Option<(Value, Item)> {
            let li = rt::method(&litems, &"anointItem".into(), &[node]).ok()?;
            let li = rt::first(li);
            if let Some(t) = li.as_table() {
                t.set_str("id", Value::Number(-100.0));
            }
            let raw = rt::method(&li, &"BuildRaw".into(), &[]).ok()?;
            let raw = lua::to_string(&rt::first(raw)).to_str_lossy().into_owned();
            let item = Item::new(&ctx, &raw, Some(rarity.as_str()), true).ok()?;
            Some((li, item))
        };
        let Some((_, base_item)) = anoint(Value::Nil) else {
            failures.push(format!("{id}: anointItem(nil) failed"));
            continue;
        };

        // Anoint-able notables (node.recipe) of the tree
        let lnodes = get(&get(&get(&lbuild, "spec"), "tree"), "nodes");
        let mut nodes: Vec<(Value, Item)> = Vec::new();
        for (k, node) in equivalence::outputs::entries(&lnodes) {
            if nodes.len() >= 5 {
                break;
            }
            let _ = k;
            let recipe = get(&node, "recipe");
            if recipe.is_nil() || rt::len(&recipe).unwrap_or_default().as_number() == Some(0.0) {
                continue;
            }
            if let Some(n) = anoint(node) {
                nodes.push(n);
            }
        }
        if nodes.is_empty() {
            continue;
        }

        let base_ovr = Override {
            rep_item: Some(("Amulet".into(), Some(base_item))),
            ..Override::default()
        };
        let node_ovrs: Vec<Override> = nodes
            .iter()
            .map(|(_, item)| Override {
                rep_item: Some(("Amulet".into(), Some(item.clone()))),
                ..Override::default()
            })
            .collect();
        let seq = compare::anoint_power(
            &calc,
            base_ovr.clone(),
            node_ovrs.clone(),
            &stat,
            Threads::Sequential,
        );
        let par = compare::anoint_power(&calc, base_ovr, node_ovrs, &stat, Threads::Pool(4));
        let (seq, par) = match (seq, par) {
            (Ok(s), Ok(p)) => (s, p),
            (s, p) => {
                failures.push(format!(
                    "{id}: anoint base failed: {:?} / {:?}",
                    s.err(),
                    p.err()
                ));
                continue;
            }
        };

        // Lua base and per-node values
        let one_arg = |ovr: Value| rt::call(&calc_func, std::slice::from_ref(&ovr));
        let amulet_ovr = |li: &Value| {
            let t = TableRef::new();
            t.set_str("repSlotName", s("Amulet"));
            t.set_str("repItem", li.clone());
            Value::Table(t)
        };
        let base_li = anoint(Value::Nil).map(|(li, _)| li).unwrap();
        let base_out = one_arg(amulet_ovr(&base_li));
        let base_val = match base_out {
            Ok(r) => lua_value(
                &power_list,
                &stat_table,
                &r.into_iter().next().unwrap_or_default(),
            ),
            Err(e) => {
                failures.push(format!("{id}: translated base: {}", describe(&e)));
                continue;
            }
        };

        let mut typed = BTreeMap::new();
        let mut want = BTreeMap::new();
        for (i, ((li, _), seq_v)) in nodes.iter().zip(&seq).enumerate() {
            let key = format!("node {i}");
            match seq_v {
                Ok(v) => {
                    typed.insert(key.clone(), number(*v));
                }
                Err(e) => {
                    failures.push(format!("{id}: node {i}: typed failed: {e}"));
                    continue;
                }
            }
            if seq[i] != par[i] {
                failures.push(format!("{id}: node {i}: threaded differs from sequential"));
            }
            let out = match one_arg(amulet_ovr(li)) {
                Ok(r) => r.into_iter().next().unwrap_or_default(),
                Err(e) => {
                    failures.push(format!("{id}: node {i}: translated: {}", describe(&e)));
                    continue;
                }
            };
            let v = lua_value(&power_list, &stat_table, &out) - base_val;
            want.insert(key, number(v));
        }
        nodes_compared += want.len();
        let mut fail = Vec::new();
        equivalence::outputs::compare(label, &typed, &want, &mut fail);
        if !fail.is_empty() {
            failures.push(format!("{id}: {}", fail.join("\n    ")));
        }
        compared += 1;
    }
    eprintln!("{compared} builds, {nodes_compared} anoints compared");
    assert!(compared > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
