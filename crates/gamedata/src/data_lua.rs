//! Port of what upstream's `Modules/Data.lua` builds on top of the data
//! files the importer reads: its literal tables, evaluated from its source,
//! and the tables its code derives, built as it builds them.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::{Result, Source};
use engine::pat;
use lua::{Key, TableRef, Value};
use std::cmp::Ordering;
use std::path::Path;

/// The literal tables of `Data.lua` the typed data reads
const LITERALS: &[&str] = &[
    "data.misc",
    "data.cursePriority",
    "data.weaponTypeInfo",
    "data.unarmedWeaponData",
    "data.defaultHighPrecision",
    "data.highPrecisionMods",
    "data.itemTagSpecial",
    "data.itemTagSpecialExclusionPattern",
    "data.casterTagCrucibleUniques",
    "data.minionTagCrucibleUniques",
];

pub fn process(src: &Source, src_dir: &Path) -> Result<()> {
    let mut im = importer::Importer::new(src_dir);
    for (k, v) in src.globals.entries() {
        if let Key::Str(name) = k {
            im.env.globals.insert(name.to_str_lossy().into_owned(), v);
        }
    }
    im.env
        .globals
        .insert("data".into(), Value::Table(src.data.clone()));
    let text = im.read("Modules/Data.lua").map_err(|e| e.to_string())?;
    im.env
        .eval_assigns("Modules/Data.lua", &text, LITERALS)
        .map_err(|e| e.to_string())?;
    let data = &src.data;
    boss_stats(data);
    minions(data);
    item_mods(data);
    rare_like_uniques(data)?;
    let map = im
        .read("Data/ModFoulbornMap.jsonc")
        .map_err(|e| e.to_string())?;
    let map = crate::jsonc::decode(&String::from_utf8_lossy(&map))
        .map_err(|e| format!("Data/ModFoulbornMap.jsonc: {e}"))?;
    data.set_str("foulbornMap", map);
    Ok(())
}

fn table(v: &Value) -> TableRef {
    v.as_table().cloned().unwrap_or_default()
}

fn field(t: &Value, k: &str) -> Value {
    t.as_table().map(|t| t.get_str(k)).unwrap_or_default()
}

fn text(v: &Value) -> String {
    v.as_str()
        .map(|s| s.to_str_lossy().into_owned())
        .unwrap_or_default()
}

fn key_text(k: &Key) -> String {
    text(&k.to_value())
}

/// `data.bossStats`: the mean armour and evasion of the bosses and the
/// uber bosses
fn boss_stats(data: &TableRef) {
    let (mut count, mut uber_count) = (0.0, 0.0);
    let (mut armour, mut evasion, mut uber_armour, mut uber_evasion) = (0.0, 0.0, 0.0, 0.0);
    for (_, boss) in table(&data.get_str("bosses")).entries() {
        let num = |k| field(&boss, k).as_number().unwrap_or(f64::NAN);
        if field(&boss, "isUber").truthy() {
            uber_count += 1.0;
            uber_armour += num("armourMult");
            uber_evasion += num("evasionMult");
        }
        count += 1.0;
        armour += num("armourMult");
        evasion += num("evasionMult");
    }
    let stats = TableRef::new();
    stats.set_str("PinnacleArmourMean", 100.0 + armour / count);
    stats.set_str("PinnacleEvasionMean", 100.0 + evasion / count);
    stats.set_str("UberArmourMean", 100.0 + uber_armour / uber_count);
    stats.set_str("UberEvasionMean", 100.0 + uber_evasion / uber_count);
    data.set_str("bossStats", Value::Table(stats));
}

/// The spectres join `data.minions`, and the minions' mods get their source
fn minions(data: &TableRef) {
    let minions = table(&data.get_str("minions"));
    for (name, spectre) in table(&data.get_str("spectres")).entries() {
        table(&spectre).set_str("limit", Value::str("ActiveSpectreLimit"));
        minions.set(name.to_value(), spectre);
    }
    for (_, minion) in minions.entries() {
        let source = format!("Minion:{}", text(&field(&minion, "name")));
        for m in table(&field(&minion, "modList")).array() {
            table(&m).set_str("source", Value::str(&source));
        }
    }
}

/// `data.itemMods.Vestigial`, `.Item` and `.WatchersEye`
fn item_mods(data: &TableRef) {
    let item_mods = table(&data.get_str("itemMods"));
    let group = |k: &str| table(&item_mods.get_str(k));
    let vestigial = TableRef::new();
    for (id, m) in group("ItemExclusive").entries() {
        if key_text(&id).starts_with("Divergent") {
            vestigial.set(id.to_value(), m);
        }
    }
    item_mods.set_str("Vestigial", Value::Table(vestigial));
    let item = TableRef::new();
    for k in [
        "Explicit",
        "ItemExclusive",
        "Corrupted",
        "Delve",
        "Synthesis",
        "Scourge",
        "Eldritch",
        "Mercenary",
    ] {
        for (id, m) in group(k).entries() {
            item.set(id.to_value(), m);
        }
    }
    item_mods.set_str("Item", Value::Table(item));
    item_mods.set_str("WatchersEye", data.get_str("watchersEyeMods"));
}

/// `data.itemBaseLists[type]`: the visible bases of a type (`Jewel: Abyss`)
/// as `{ name, base }`, sorted as `Data.lua` sorts them
fn item_base_list(data: &TableRef, typ: &str) -> TableRef {
    let mut list: Vec<(String, Value)> = table(&data.get_str("itemBases"))
        .entries()
        .into_iter()
        .filter(|(_, b)| !field(b, "hidden").truthy())
        .filter(|(_, b)| {
            let sub = field(b, "subType");
            let t = text(&field(b, "type"));
            (if sub.truthy() {
                format!("{t}: {}", text(&sub))
            } else {
                t
            }) == typ
        })
        .map(|(k, b)| (key_text(&k), b))
        .collect();
    let less = |a: &(String, Value), b: &(String, Value)| {
        let (ar, br) = (field(&a.1, "req"), field(&b.1, "req"));
        match (ar.truthy(), br.truthy()) {
            (true, true) => {
                let (al, bl) = (field(&ar, "level"), field(&br, "level"));
                if al.raw_eq(&bl) {
                    a.0 < b.0
                } else {
                    al.as_number().unwrap_or(1.0) > bl.as_number().unwrap_or(1.0)
                }
            }
            (true, false) => true,
            (false, true) => false,
            (false, false) => a.0 < b.0,
        }
    };
    list.sort_by(|a, b| {
        if less(a, b) {
            Ordering::Less
        } else if less(b, a) {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });
    let out = TableRef::new();
    for (name, base) in list {
        let entry = TableRef::new();
        entry.set_str("name", Value::str(&name));
        entry.set_str("base", base);
        out.push(Value::Table(entry));
    }
    out
}

/// `{ base = copyTable(data.itemBases[name]) }` with extra spawn tags
fn tagged_base(data: &TableRef, name: &str, tags: &[String]) -> Result<Value> {
    let base = table(&data.get_str("itemBases")).get_str(name);
    let base = importer::copy_table(
        base.as_table()
            .ok_or_else(|| format!("no item base {name}"))?,
        false,
    );
    let base_tags = table(&base.get_str("tags"));
    for t in tags {
        base_tags.set_str(t, true);
    }
    let entry = TableRef::new();
    entry.set_str("base", Value::Table(base));
    let list = TableRef::new();
    list.push(Value::Table(entry));
    Ok(Value::Table(list))
}

/// `data.rareLikeUniques`
fn rare_like_uniques(data: &TableRef) -> Result<()> {
    let item_mods = table(&data.get_str("itemMods"));
    let subsume = TableRef::new();
    for (id, m) in table(&item_mods.get_str("JewelAbyss")).entries() {
        if !field(&m, "type").raw_eq(&Value::str("Corrupted")) {
            subsume.set(id.to_value(), m);
        }
    }
    let veiled_mods = data.get_str("veiledMods");
    let mut spawn_tags = Vec::new();
    let [veiled, veiled_suffixes, kulemak, queens_hunger] =
        std::array::from_fn(|_| TableRef::new());
    for (id, m) in table(&veiled_mods).entries() {
        let id = id.to_value();
        let add = |sets: &[&TableRef]| {
            for s in sets {
                s.set(id.clone(), m.clone());
            }
        };
        match text(&field(&m, "affix")).as_str() {
            "Chosen" => add(&[&veiled, &kulemak, &queens_hunger]),
            "Catarina's" => add(&[&kulemak, &queens_hunger]),
            "of the Order" => add(&[&veiled, &veiled_suffixes, &kulemak, &queens_hunger]),
            _ => {}
        }
        for (_, tag) in table(&field(&m, "weightKey")).entries() {
            let tag = text(&tag);
            if pat::find(&tag, "^[%a_]+_veiled_prefix").is_some()
                || pat::find(&tag, "^[%a_]+_veiled_suffix").is_some()
            {
                spawn_tags.push(tag);
            }
        }
    }
    let taken = TableRef::new();
    for (id, m) in table(&item_mods.get_str("JewelCharm")).entries() {
        if pat::find(&key_text(&id), "1$").is_none() {
            taken.set(id.to_value(), m);
        }
    }
    let prefix_tags: Vec<String> = spawn_tags
        .into_iter()
        .filter(|t| t.contains("prefix"))
        .collect();
    let catarina = ["catarina_veiled_prefix".to_string()];
    let uniques = TableRef::new();
    let add = |title: &str,
               valid_bases: Option<Value>,
               affixes: Value,
               prefix: f64,
               suffix: f64,
               extra: &[(&str, Value)]| {
        let u = TableRef::new();
        if let Some(v) = valid_bases {
            u.set_str("validBases", v);
        }
        u.set_str("affixes", affixes);
        u.set_str("prefixLimit", prefix);
        u.set_str("suffixLimit", suffix);
        for (k, v) in extra {
            u.set_str(k, v.clone());
        }
        uniques.set_str(title, Value::Table(u));
    };
    let custom = TableRef::new();
    for k in ["ESSENCE", "VEILED", "CUSTOM"] {
        custom.set_str(k, true);
    }
    add(
        "subsume the source",
        Some(Value::Table(item_base_list(data, "Jewel: Abyss"))),
        Value::Table(subsume),
        4.0,
        0.0,
        &[
            ("ignoreModType", true.into()),
            ("allowDuplicateGroups", true.into()),
        ],
    );
    add(
        "the crimson storm",
        None,
        Value::Table(veiled_suffixes),
        0.0,
        1.0,
        &[],
    );
    add(
        "dread captain's cutlass",
        Some(tagged_base(
            data,
            "Ghostflame Blade",
            &["deepwater_sword".into()],
        )?),
        item_mods.get_str("Explicit"),
        3.0,
        3.0,
        &[("supportsCustomModifiers", Value::Table(custom))],
    );
    add("paradoxica", None, Value::Table(veiled), 1.0, 1.0, &[]);
    add(
        "cane of kulemak",
        Some(tagged_base(data, "Serpentine Staff", &catarina)?),
        Value::Table(kulemak),
        2.0,
        2.0,
        &[],
    );
    add(
        "replica paradoxica",
        Some(tagged_base(data, "Vaal Rapier", &prefix_tags)?),
        veiled_mods,
        3.0,
        3.0,
        &[],
    );
    add(
        "the queen's hunger",
        Some(tagged_base(data, "Vaal Regalia", &catarina)?),
        Value::Table(queens_hunger),
        1.0,
        1.0,
        &[],
    );
    add(
        "that which was taken",
        Some(Value::Table(item_base_list(data, "Jewel: Charm"))),
        Value::Table(taken),
        4.0,
        0.0,
        &[("ignoreModType", true.into())],
    );
    data.set_str("rareLikeUniques", Value::Table(uniques));
    Ok(())
}
