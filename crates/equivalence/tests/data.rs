//! Layer 1 of docs/design.md: every table the importer reads equals the Lua
//! table upstream gets from the same file. `tests/oracle/data.jsonl` holds the
//! FNV-1a hash of each table's canonical JSON; to see a difference, write the
//! tables with `oracle/run.py data-json` and this test's JSON with
//! `DATA_JSON_DIR=<dir> cargo test --release -p equivalence --test data`.

use equivalence::{oracle_dir, to_json_with_funcs, upstream_src};
use lua::{TableRef, Value};

fn path(raw: &importer::Raw, path: &[&str]) -> Value {
    let mut v = Value::Table(raw.data.clone());
    for p in path {
        v = v
            .as_table()
            .unwrap_or_else(|| panic!("{path:?}"))
            .get_str(p);
    }
    v
}

#[test]
fn every_table_matches_upstream() {
    let raw = importer::import(&upstream_src()).unwrap_or_else(|e| panic!("{e}"));
    let mut im = importer::Importer::new(&upstream_src());
    let mut fresh = |file: &str| {
        let t = TableRef::new();
        im.load(&format!("Data/{file}"), vec![Value::Table(t.clone())])
            .unwrap_or_else(|e| panic!("{e}"));
        Value::Table(t)
    };
    let misc = fresh("Misc");
    let mod_cache = fresh("ModCache");
    let item_mods = |k: &str| path(&raw, &["itemMods", k]);
    let lookup = |key: &str| -> Value {
        if let Some(k) = key.strip_prefix("data.") {
            return path(&raw, &[k]);
        }
        if let Some(g) = key.strip_prefix("global.") {
            return raw.globals.get_str(g);
        }
        if let Some(t) = key.strip_prefix("Uniques/") {
            return match t {
                "Special/race" => path(&raw, &["uniques", "race"]),
                "Special/WatchersEye" => path(&raw, &["watchersEyeMods"]),
                t => path(&raw, &["uniques", t]),
            };
        }
        match key {
            "Misc" => misc.clone(),
            "ModCache" => mod_cache.clone(),
            "ModMaster" => path(&raw, &["masterMods"]),
            "EnchantmentHelmet" => path(&raw, &["enchantments", "Helmet"]),
            "EnchantmentBoots" => path(&raw, &["enchantments", "Boots"]),
            "EnchantmentGloves" => path(&raw, &["enchantments", "Gloves"]),
            "EnchantmentBelt" => path(&raw, &["enchantments", "Belt"]),
            "EnchantmentBody" => path(&raw, &["enchantments", "Body Armour"]),
            "EnchantmentWeapon" => path(&raw, &["enchantments", "Weapon"]),
            "EnchantmentFlask" => path(&raw, &["enchantments", "UtilityFlask"]),
            "Essence" => path(&raw, &["essences"]),
            "ModVeiled" => path(&raw, &["veiledMods"]),
            "BeastCraft" => path(&raw, &["beastCraft"]),
            "ModNecropolis" => path(&raw, &["necropolisMods"]),
            "Crucible" => path(&raw, &["crucible"]),
            "Pantheons" => path(&raw, &["pantheons"]),
            "Costs" => path(&raw, &["costs"]),
            "ModMap" => path(&raw, &["mapMods"]),
            "ClusterJewels" => path(&raw, &["clusterJewels"]),
            "TimelessJewelData/LegionTradeIds" => path(&raw, &["timelessJewelTradeIDs"]),
            "TimelessJewelData/NodeIndexMapping" => path(&raw, &["nodeIDList"]),
            "TimelessJewelData/AbyssNotableNames" => path(&raw, &["abyssNotableNames"]),
            "Rares" => path(&raw, &["rares"]),
            "FlavourText" => path(&raw, &["flavourText"]),
            "Bosses" => path(&raw, &["bosses"]),
            "BossSkills.1" => path(&raw, &["bossSkills"]),
            "BossSkills.2" => path(&raw, &["bossSkillsList"]),
            "SkillStatMap" => path(&raw, &["skillStatMap"]),
            "Skills" => path(&raw, &["skills"]),
            "Gems" => path(&raw, &["gems"]),
            "Minions" => path(&raw, &["minions"]),
            "Spectres" => path(&raw, &["spectres"]),
            "Bases" => path(&raw, &["itemBases"]),
            k if k.starts_with("Mod") => item_mods(&k[3..]),
            k => panic!("no importer table for {k}"),
        }
    };
    let json_dir = std::env::var_os("DATA_JSON_DIR").map(std::path::PathBuf::from);
    let text =
        std::fs::read_to_string(oracle_dir().join("data.jsonl")).expect("tests/oracle/data.jsonl");
    let mut failures = Vec::new();
    for line in text.lines() {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        let key = record["key"].as_str().unwrap();
        let json = to_json_with_funcs(&lookup(key));
        if let Some(dir) = &json_dir {
            std::fs::write(
                dir.join(format!("{}.json", key.replace('/', "_"))),
                format!("{json}\n"),
            )
            .unwrap();
        }
        let fnv = format!("{:016x}", lua::value::fnv1a(json.as_bytes()));
        if fnv != record["fnv"].as_str().unwrap()
            || json.len() as u64 != record["bytes"].as_u64().unwrap()
        {
            failures.push(format!(
                "{key}: {} bytes, upstream {} bytes",
                json.len(),
                record["bytes"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "tables differ from upstream:\n{}",
        failures.join("\n")
    );
}
