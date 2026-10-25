//! Typed items (`engine::item`) equal the translated program's `Item`: every
//! item of every corpus build loaded from XML, and every unique and rare
//! template of the data as upstream's item databases parse them. Compared are the item's
//! properties, its mod lines and their mods, its sockets and requirements,
//! the mods it grants in each slot and its weapon, armour, flask, tincture
//! and jewel data.
//!
//! `CALCS_FILTER=<substring>` limits the builds run; `ITEM_UNIQUES=0` skips
//! the uniques and rares.

use engine::item::{Fields, INFLUENCES, Item, ItemContext, LineFlags, ModLine};
use engine::item_data::ItemData;
use engine::item_tools::Range;
use engine::mods::convert::Converter;
use engine::mods::{Mod, ModValue};
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

fn text(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_str_lossy().into_owned())
}

fn array(v: &Value) -> Vec<Value> {
    v.as_table().map(|t| t.array()).unwrap_or_default()
}

/// The attributes of every `<Item>` element, in order
fn item_attrs(xml: &str) -> Vec<HashMap<String, String>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<Item ") {
        rest = &rest[i + 6..];
        let end = rest.find('>').unwrap();
        let mut attrs = HashMap::new();
        let mut t = &rest[..end];
        while let Some(eq) = t.find("=\"") {
            let name = t[..eq].trim().to_string();
            let after = &t[eq + 2..];
            let close = after.find('"').unwrap();
            attrs.insert(name, after[..close].to_string());
            t = &after[close + 1..];
        }
        out.push(attrs);
    }
    out
}

fn int_set(v: &Value) -> Option<BTreeSet<i64>> {
    v.as_table().map(|t| {
        t.entries()
            .into_iter()
            .filter(|(_, x)| x.truthy())
            .filter_map(|(k, _)| match k {
                Key::Int(i) => Some(i),
                _ => None,
            })
            .collect()
    })
}

fn lua_mods(v: &Value) -> Result<Vec<Mod>, String> {
    let conv = Converter::default();
    array(v).iter().map(|m| conv.mod_(m)).collect()
}

/// Functions of the two runtimes are named differently; they compare by
/// their place. Record fields compare in any order.
fn no_funcs(v: &mut ModValue) {
    match v {
        ModValue::Func(f) => {
            *f = std::rc::Rc::new(engine::mods::Func {
                name: "".into(),
                args: vec![],
            })
        }
        ModValue::Mod(m) => no_funcs(&mut m.value),
        ModValue::Record(r) => {
            r.fields.iter_mut().for_each(|(_, x)| no_funcs(x));
            r.fields.sort_by_key(|(k, _)| k.as_str());
        }
        ModValue::List(l) => l.iter_mut().for_each(no_funcs),
        _ => {}
    }
}

fn mods_diff(t: &[Mod], l: &Value) -> Option<String> {
    let mut lm = match lua_mods(l) {
        Ok(m) => m,
        Err(e) => return Some(format!("translated mods do not convert: {e}")),
    };
    let mut t = t.to_vec();
    for m in t.iter_mut().chain(lm.iter_mut()) {
        no_funcs(&mut m.value);
    }
    let t = &t[..];
    if t == lm.as_slice() {
        return None;
    }
    if t.len() != lm.len() {
        let names = |m: &[Mod]| m.iter().map(|m| m.name.as_str()).collect::<Vec<_>>();
        return Some(format!(
            "{} mods != {}\n    typed {:?}\n    translated {:?}",
            t.len(),
            lm.len(),
            names(t),
            names(&lm)
        ));
    }
    let i = (0..t.len()).find(|&i| t[i] != lm[i]).unwrap();
    Some(format!(
        "mod {}:\n    typed {}\n    translated {}",
        i + 1,
        typed_json::mod_(&t[i]),
        typed_json::mod_(&lm[i])
    ))
}

fn fields_diff(t: &Fields, l: &Value) -> Option<String> {
    let conv = Converter::default();
    let mut lf = Fields::new();
    for (k, v) in l.as_table().map(|t| t.entries()).unwrap_or_default() {
        let k = match k {
            Key::Str(s) => s.to_str_lossy().into_owned(),
            k => return Some(format!("key {k:?}")),
        };
        // Set by the calculation (CalcSetup), not by the item
        if k == "limitDisabled" {
            continue;
        }
        match conv.value(&v) {
            Ok(mut v) => {
                no_funcs(&mut v);
                lf.insert(k, v);
            }
            Err(e) => return Some(format!("{k}: {e}")),
        }
    }
    let mut t = t.clone();
    t.values_mut().for_each(no_funcs);
    let keys: BTreeSet<&String> = t.keys().chain(lf.keys()).collect();
    let diffs: Vec<String> = keys
        .into_iter()
        .filter(|k| !same_value(t.get(*k), lf.get(*k)))
        .map(|k| format!("{k}: {:?} != {:?}", t.get(k), lf.get(k)))
        .collect();
    (!diffs.is_empty()).then(|| diffs.join("; "))
}

/// Values equal, an empty list and an empty record being the same (both
/// are an empty table upstream)
fn same_value(a: Option<&ModValue>, b: Option<&ModValue>) -> bool {
    let empty = |v: &ModValue| match v {
        ModValue::List(l) => l.is_empty(),
        ModValue::Record(r) => r.fields.is_empty(),
        _ => false,
    };
    match (a, b) {
        (Some(x), Some(y)) => x == y || (empty(x) && empty(y)) || record_eq(x, y),
        (None, None) => true,
        _ => false,
    }
}

/// Records equal in any field order (a record built in Rust has its own)
fn record_eq(a: &ModValue, b: &ModValue) -> bool {
    match (a, b) {
        (ModValue::Record(x), ModValue::Record(y)) => {
            x.fields.len() == y.fields.len()
                && x.fields.iter().all(|(k, v)| {
                    y.get(k.as_str())
                        .is_some_and(|w| same_value(Some(v), Some(w)))
                })
        }
        _ => false,
    }
}

fn opt_num(v: &Value) -> Option<f64> {
    v.as_number()
}

fn range_of(v: &Value) -> Option<Range> {
    match v {
        Value::Number(n) => Some(Range::One(*n)),
        Value::Table(_) => Some(Range::Each(
            array(v)
                .iter()
                .map(|x| x.as_number().unwrap_or(f64::NAN))
                .collect(),
        )),
        _ => None,
    }
}

fn compare_lines(what: &str, t: &[ModLine], l: &Value, fail: &mut dyn FnMut(String)) {
    let ll = array(l);
    if t.len() != ll.len() {
        fail(format!(
            "{what}: {} lines != {}\n    typed {:?}\n    translated {:?}",
            t.len(),
            ll.len(),
            t.iter().map(|m| &m.line).collect::<Vec<_>>(),
            ll.iter().map(|m| text(&get(m, "line"))).collect::<Vec<_>>()
        ));
        return;
    }
    for (i, (tm, lm)) in t.iter().zip(&ll).enumerate() {
        let mut check = |field: &str, ok: bool, detail: String| {
            if !ok {
                fail(format!(
                    "{what} {} ({:?}) {field}: {detail}",
                    i + 1,
                    tm.line
                ));
            }
        };
        let line = text(&get(lm, "line"));
        check(
            "line",
            line.as_deref() == Some(tm.line.as_str()),
            format!("{:?}", line),
        );
        let extra = text(&get(lm, "extra"));
        check(
            "extra",
            extra == tm.extra,
            format!("{:?} != {extra:?}", tm.extra),
        );
        let range = opt_num(&get(lm, "range"));
        check(
            "range",
            range == tm.range,
            format!("{:?} != {range:?}", tm.range),
        );
        let cr = opt_num(&get(lm, "corruptedRange"));
        check(
            "corruptedRange",
            cr == tm.corrupted_range,
            format!("{:?} != {cr:?}", tm.corrupted_range),
        );
        let vs = opt_num(&get(lm, "valueScalar"));
        check(
            "valueScalar",
            vs == tm.value_scalar,
            format!("{:?} != {vs:?}", tm.value_scalar),
        );
        let order = opt_num(&get(lm, "order"));
        check(
            "order",
            order == tm.order,
            format!("{:?} != {order:?}", tm.order),
        );
        for flag in LineFlags::NAMES {
            let lf = get(lm, flag).truthy();
            check(
                flag,
                lf == tm.flags.get(flag),
                format!("{} != {lf}", tm.flags.get(flag)),
            );
        }
        let tags: Vec<String> = array(&get(lm, "modTags")).iter().filter_map(text).collect();
        check(
            "modTags",
            tags == tm.mod_tags,
            format!("{:?} != {tags:?}", tm.mod_tags),
        );
        for (field, tv) in [
            ("variantList", &tm.variant_list),
            ("versionList", &tm.version_list),
            ("variantGroupList", &tm.variant_group_list),
        ] {
            let lv = int_set(&get(lm, field));
            check(field, lv == *tv, format!("{tv:?} != {lv:?}"));
        }
        for (field, tv) in [("modId", &tm.mod_id), ("newModId", &tm.new_mod_id)] {
            let lv = text(&get(lm, field));
            check(field, lv == *tv, format!("{tv:?} != {lv:?}"));
        }
        if let Some(d) = mods_diff(&tm.mod_list, &get(lm, "modList")) {
            check("modList", false, d);
        }
    }
}

fn compare(t: &Item, l: &Value, fail: &mut dyn FnMut(String)) {
    let mut check = |field: &str, ok: bool, detail: String| {
        if !ok {
            fail(format!("{field}: {detail}"));
        }
    };
    for (field, tv) in [
        ("name", Some(t.name.clone())),
        ("namePrefix", Some(t.name_prefix.clone())),
        ("nameSuffix", Some(t.name_suffix.clone())),
        ("title", t.title.clone()),
        ("baseName", t.base_name.clone()),
        ("type", t.typ.clone()),
        ("rarity", Some(t.rarity.clone())),
        ("classRestriction", t.class_restriction.clone()),
        ("uniqueID", t.unique_id.clone()),
        ("jewelRadiusLabel", t.jewel_radius_label.clone()),
        ("clusterJewelSkill", t.cluster_jewel_skill.clone()),
        ("foilType", t.foil_type.clone()),
        (
            "modSource",
            Some(t.mod_source.clone()).filter(|s| !s.is_empty()),
        ),
    ] {
        let lv = text(&get(l, field));
        check(field, lv == tv, format!("{tv:?} != {lv:?}"));
    }
    for (field, tv) in [
        ("quality", t.quality),
        ("itemLevel", t.item_level),
        ("variant", t.variant),
        ("variantAlt", t.variant_alt[0]),
        ("variantAlt2", t.variant_alt[1]),
        ("variantAlt3", t.variant_alt[2]),
        ("variantAlt4", t.variant_alt[3]),
        ("variantAlt5", t.variant_alt[4]),
        ("selectedVersion", t.selected_version),
        ("jewelRadiusIndex", t.jewel_radius_index),
        ("limit", t.limit),
        ("catalystQuality", t.catalyst_quality),
        ("catalyst", t.catalyst.map(|c| c as f64)),
        ("craftedQuality", t.crafted_quality),
        ("clusterJewelNodeCount", t.cluster_jewel_node_count),
        ("talismanTier", t.talisman_tier),
    ] {
        let lv = opt_num(&get(l, field));
        let same = lv == tv || (lv.is_some_and(f64::is_nan) && tv.is_some_and(f64::is_nan));
        check(field, same, format!("{tv:?} != {lv:?}"));
    }
    if t.base.is_some() {
        for (field, tv) in [
            ("abyssalSocketCount", t.abyssal_socket_count),
            ("selectableSocketCount", t.selectable_socket_count),
            (
                "socketedJewelEffectModifier",
                t.socketed_jewel_effect_modifier,
            ),
        ] {
            let lv = opt_num(&get(l, field));
            check(field, lv == Some(tv), format!("{tv:?} != {lv:?}"));
        }
    }
    let la = opt_num(&get(l, "affixLimit"));
    check(
        "affixLimit",
        la == Some(t.affix_limit),
        format!("{} != {la:?}", t.affix_limit),
    );
    for (field, tv) in [
        ("split", t.split),
        ("mirrored", t.mirrored),
        ("corrupted", t.corrupted),
        ("fractured", t.fractured),
        ("synthesised", t.synthesised),
        ("veiled", t.veiled),
        ("crafted", t.crafted),
        ("advancedCopy", t.advanced_copy),
        ("foulborn", t.foulborn),
        ("vestigial", t.vestigial),
        ("usesVariantGroups", t.uses_variant_groups),
        ("allowDuplicateVariants", t.allow_duplicate_variants),
        ("hasModTags", t.has_mod_tags),
        ("implicitsCannotBeChanged", t.implicits_cannot_be_changed),
        ("canBeAnointed", t.can_be_anointed),
        ("restrictTag", t.restrict_tag),
        ("restrictDamageType", t.restrict_damage_type),
        ("hidden_specs", t.hidden_specs),
        ("hasAltVariant", t.has_alt_variant[0]),
        ("hasAltVariant2", t.has_alt_variant[1]),
    ] {
        let lv = get(l, field).truthy();
        check(field, lv == tv, format!("{tv} != {lv}"));
    }
    for (i, (key, _)) in INFLUENCES.iter().enumerate() {
        let lv = get(l, key).truthy();
        check(
            key,
            lv == t.influences[i],
            format!("{} != {lv}", t.influences[i]),
        );
    }
    let lv: Option<Vec<String>> = get(l, "variantList").as_table().map(|_| {
        array(&get(l, "variantList"))
            .iter()
            .filter_map(text)
            .collect()
    });
    check(
        "variantList",
        lv == t.variant_list,
        format!("{:?} != {lv:?}", t.variant_list),
    );
    let lsel: BTreeMap<i64, f64> = get(l, "variantGroupSelections")
        .as_table()
        .map(|t| t.entries())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| match k {
            Key::Int(i) => Some((i, v.as_number()?)),
            _ => None,
        })
        .collect();
    check(
        "variantGroupSelections",
        lsel == t.variant_group_selections,
        format!("{:?} != {lsel:?}", t.variant_group_selections),
    );
    let sockets: Vec<(String, f64)> = array(&get(l, "sockets"))
        .iter()
        .map(|s| {
            (
                text(&get(s, "color")).unwrap_or_default(),
                opt_num(&get(s, "group")).unwrap_or(f64::NAN),
            )
        })
        .collect();
    let ts: Vec<(String, f64)> = t
        .sockets
        .iter()
        .map(|s| (s.color.to_string(), s.group))
        .collect();
    check("sockets", sockets == ts, format!("{ts:?} != {sockets:?}"));
    let cam = get(&get(l, "sockets"), "colourAlwaysMatches").truthy();
    check(
        "colourAlwaysMatches",
        cam == t.colour_always_matches,
        format!("{}", t.colour_always_matches),
    );
    let req = get(l, "requirements");
    let r = &t.requirements;
    for (field, tv) in [
        ("level", r.level),
        ("str", Some(r.str)),
        ("dex", Some(r.dex)),
        ("int", Some(r.int)),
        ("strMod", t.base.is_some().then_some(r.str_mod)),
        ("dexMod", t.base.is_some().then_some(r.dex_mod)),
        ("intMod", t.base.is_some().then_some(r.int_mod)),
    ] {
        let lv = opt_num(&get(&req, field));
        check(
            &format!("requirements.{field}"),
            lv == tv,
            format!("{tv:?} != {lv:?}"),
        );
    }
    for (field, ta) in [("prefixes", &t.prefixes), ("suffixes", &t.suffixes)] {
        let la = get(l, field);
        let ll: Vec<(Option<String>, Option<Range>)> = array(&la)
            .iter()
            .map(|a| (text(&get(a, "modId")), range_of(&get(a, "range"))))
            .collect();
        let tl: Vec<(Option<String>, Option<Range>)> = ta
            .list
            .iter()
            .map(|a| (Some(a.mod_id.clone()), a.range.clone()))
            .collect();
        check(field, ll == tl, format!("{tl:?} != {ll:?}"));
        let limit = opt_num(&get(&la, "limit"));
        check(
            &format!("{field}.limit"),
            limit == ta.limit,
            format!("{:?} != {limit:?}", ta.limit),
        );
    }
    for (what, lines, key) in [
        ("buffModLines", &t.buff_mod_lines, "buffModLines"),
        ("enchantModLines", &t.enchant_mod_lines, "enchantModLines"),
        ("scourgeModLines", &t.scourge_mod_lines, "scourgeModLines"),
        (
            "classRequirementModLines",
            &t.class_requirement_mod_lines,
            "classRequirementModLines",
        ),
        (
            "implicitModLines",
            &t.implicit_mod_lines,
            "implicitModLines",
        ),
        (
            "explicitModLines",
            &t.explicit_mod_lines,
            "explicitModLines",
        ),
        (
            "crucibleModLines",
            &t.crucible_mod_lines,
            "crucibleModLines",
        ),
    ] {
        compare_lines(what, lines, &get(l, key), fail);
    }
    let mut check = |field: &str, d: Option<String>| {
        if let Some(d) = d {
            fail(format!("{field}: {d}"));
        }
    };
    if t.base.is_some() {
        check(
            "baseModList",
            mods_diff(&t.base_mod_list, &get(l, "baseModList")),
        );
    }
    let lbuff = get(l, "buffModList");
    if !lbuff.is_nil() || !t.buff_mod_list.is_empty() {
        check("buffModList", mods_diff(&t.buff_mod_list, &lbuff));
    }
    let lml = get(l, "modList");
    match &t.mod_list {
        Some(m) => check("modList", mods_diff(m, &lml)),
        None if !lml.is_nil() => check("modList", Some("missing".into())),
        None => {}
    }
    let lsl = get(l, "slotModList");
    match &t.slot_mod_list {
        Some(lists) => {
            for (i, m) in lists.iter().enumerate() {
                let li = rt::index(&lsl, &Value::Number(i as f64 + 1.0)).unwrap_or_default();
                check(&format!("slotModList[{}]", i + 1), mods_diff(m, &li));
            }
        }
        None if !lsl.is_nil() => check("slotModList", Some("missing".into())),
        None => {}
    }
    let lwd = get(l, "weaponData");
    match &t.weapon_data {
        Some(wd) => {
            for (slot, f) in wd {
                let li = rt::index(&lwd, &Value::Number(*slot as f64)).unwrap_or_default();
                check(&format!("weaponData[{slot}]"), fields_diff(f, &li));
            }
        }
        None if !lwd.is_nil() => check("weaponData", Some("missing".into())),
        None => {}
    }
    for (field, tf) in [
        ("armourData", &t.armour_data),
        ("flaskData", &t.flask_data),
        ("tinctureData", &t.tincture_data),
        ("jewelData", &t.jewel_data),
    ] {
        let lf = get(l, field);
        match tf {
            Some(f) => check(field, fields_diff(f, &lf)),
            None if !lf.is_nil() => check(field, Some("missing".into())),
            None => {}
        }
    }
    #[allow(clippy::type_complexity)]
    let lskills: Vec<(
        Option<String>,
        Option<f64>,
        bool,
        Option<String>,
        bool,
        Option<f64>,
    )> = array(&get(l, "grantedSkills"))
        .iter()
        .map(|g| {
            (
                text(&get(g, "skillId")),
                opt_num(&get(g, "level")),
                get(g, "noSupports").truthy(),
                text(&get(g, "source")),
                get(g, "triggered").truthy(),
                opt_num(&get(g, "triggerChance")),
            )
        })
        .collect();
    let tskills: Vec<_> = t
        .granted_skills
        .iter()
        .map(|g| {
            (
                g.skill_id.as_ref().map(|s| s.to_string()),
                g.level,
                g.no_supports,
                Some(g.source.clone()),
                g.triggered,
                g.trigger_chance,
            )
        })
        .collect();
    if t.base.is_some() && lskills != tskills {
        check("grantedSkills", Some(format!("{tskills:?} != {lskills:?}")));
    }
}

fn jewel_radius(host: &Host) -> Vec<String> {
    array(&get(&host.global("data"), "jewelRadius"))
        .iter()
        .map(|r| text(&get(r, "label")).unwrap_or_default())
        .collect()
}

#[test]
fn items_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let parser = mod_parser();
    let mut im = importer::Importer::new(&upstream_src());
    let cache = importer::mod_parser::import_mod_cache(&mut im).unwrap_or_else(|e| panic!("{e}"));
    parser.preload(&cache);
    let data = ItemData::from_lua(&host.global("data"), &host.global("colorCodes"))
        .unwrap_or_else(|e| panic!("item data: {e}"));
    let mut failures = Vec::new();
    let mut compared = 0;
    let record = |what: String, item_failures: Vec<String>, failures: &mut Vec<String>| {
        if !item_failures.is_empty() && failures.len() < 300 {
            failures.push(format!("{what}:\n  {}", item_failures.join("\n  ")));
        }
    };

    if std::env::var("ITEM_UNIQUES").map_or(true, |v| v != "0") && filter.is_empty() {
        let radius = jewel_radius(&host);
        let ctx = ItemContext {
            data: &data,
            parser: &parser,
            jewel_radius: &radius,
            default_affix_quality: 0.5,
        };
        let new = host.global("new");
        let data_table = host.global("data");
        let mut texts: Vec<(String, Value, &str)> = Vec::new();
        for (ty, list) in get(&data_table, "uniques").as_table().unwrap().entries() {
            for (_, raw) in list.as_table().map(|t| t.entries()).unwrap_or_default() {
                texts.push((
                    format!("unique {:?}", lua::to_string(&ty.to_value())),
                    raw,
                    "UNIQUE",
                ));
            }
        }
        // The rare templates, as the rare database parses them before
        // crafting them
        for (_, raw) in get(&data_table, "rares").as_table().unwrap().entries() {
            texts.push(("rare".into(), raw, "RARE"));
        }
        {
            for (kind, raw, rarity) in texts {
                let raw_text = text(&raw).unwrap();
                let first_line = raw_text
                    .trim()
                    .lines()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" / ");
                let what = format!("{kind} {first_line}");
                let lua_item = match rt::call(
                    &new,
                    &[s("Item"), raw.clone(), s(rarity), Value::Bool(true)],
                ) {
                    Ok(v) => v.into_iter().next().unwrap_or_default(),
                    Err(e) => {
                        record(
                            what,
                            vec![format!("translated failed: {}", describe(&e))],
                            &mut failures,
                        );
                        continue;
                    }
                };
                compared += 1;
                let mut item_failures = Vec::new();
                match Item::new(&ctx, &raw_text, Some(rarity), true) {
                    Ok(item) => compare(&item, &lua_item, &mut |f| item_failures.push(f)),
                    Err(e) => item_failures.push(format!("typed failed: {e}")),
                }
                record(what, item_failures, &mut failures);
            }
        }
    }

    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let items = get(&get(&build, "itemsTab"), "items");
        let radius = jewel_radius(&host);
        let ctx = ItemContext {
            data: &data,
            parser: &parser,
            jewel_radius: &radius,
            default_affix_quality: 0.5,
        };
        for attrs in item_attrs(&xml) {
            let Some(item_id) = attrs.get("id").and_then(|i| i.parse::<f64>().ok()) else {
                continue;
            };
            let lua_item = rt::index(&items, &Value::Number(item_id)).unwrap_or_default();
            if lua_item.is_nil() {
                continue;
            }
            compared += 1;
            let num = |k: &str| attrs.get(k).and_then(|v| lua::str_to_number(v.as_bytes()));
            let mut item = Item::default();
            let mut item_failures = Vec::new();
            let result = (|| -> Result<(), String> {
                item.parse_raw(&ctx, "", None, false)?;
                item.id = Some(item_id);
                item.variant = num("variant");
                for (i, k) in [
                    "variantAlt",
                    "variantAlt2",
                    "variantAlt3",
                    "variantAlt4",
                    "variantAlt5",
                ]
                .iter()
                .enumerate()
                {
                    if attrs.contains_key(*k) {
                        item.has_alt_variant[i] = true;
                        item.variant_alt[i] = num(k);
                    }
                }
                let raw = text(&get(&lua_item, "raw")).unwrap_or_default();
                item.parse_raw(&ctx, &raw, None, false)?;
                item.build_mod_list(&ctx)
            })();
            match result {
                Ok(()) => compare(&item, &lua_item, &mut |f| item_failures.push(f)),
                Err(e) => item_failures.push(format!("typed failed: {e}")),
            }
            record(format!("{id} item {item_id}"), item_failures, &mut failures);
        }
    }
    eprintln!("{compared} items compared");
    assert!(
        failures.is_empty(),
        "{} items differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
