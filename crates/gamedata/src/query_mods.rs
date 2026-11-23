//! `Data/QueryMods.lua` as `TradeQueryGenerator:InitMods` reads it: each
//! group's entries with their trade mod and rolls by item category.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::{Result, Source};
use engine::mod_weights::{ModRange, QueryMod, QueryMods};
use lua::{Key, TableRef, Value};

fn key(k: &Key) -> Result<String> {
    match k {
        Key::Str(s) => Ok(s.to_str_lossy().into_owned()),
        k => Err(format!("QueryMods: expected a string key, got {k:?}")),
    }
}

fn table(v: &Value, what: &str) -> Result<TableRef> {
    v.as_table()
        .cloned()
        .ok_or_else(|| format!("QueryMods: {what} is not a table"))
}

fn string(t: &TableRef, k: &str) -> Option<String> {
    t.get_str(k).as_str().map(|s| s.to_str_lossy().into_owned())
}

fn entry(name: String, v: &Value) -> Result<QueryMod> {
    let t = table(v, &name)?;
    let trade = table(&t.get_str("tradeMod"), &format!("{name}.tradeMod"))?;
    let special = t.get_str("specialCaseData");
    let special = special.as_table();
    let need = |k: &str| string(&trade, k).ok_or_else(|| format!("QueryMods: {name}.tradeMod.{k}"));
    let mut m = QueryMod {
        trade_id: need("id")?,
        text: need("text")?,
        sign: string(&t, "sign"),
        override_line: special.and_then(|s| string(s, "overrideModLine")),
        override_line_singular: special.and_then(|s| string(s, "overrideModLineSingular")),
        key: name,
        ranges: Vec::new(),
    };
    for (k, v) in t.entries() {
        let k = key(&k)?;
        if matches!(
            k.as_str(),
            "tradeMod" | "specialCaseData" | "sign" | "inverseKey"
        ) {
            continue;
        }
        let r = table(&v, &format!("{}.{k}", m.key))?;
        let num = |f: &str| {
            r.get_str(f)
                .as_number()
                .ok_or_else(|| format!("QueryMods: {}.{k}.{f}", m.key))
        };
        m.ranges.push((
            k.clone(),
            ModRange {
                min: num("min")?,
                max: num("max")?,
                sub_type: string(&r, "subType"),
            },
        ));
    }
    m.ranges.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(m)
}

impl Source {
    /// `Data/QueryMods.lua`, each group's entries by key
    pub fn query_mods(&self) -> Result<QueryMods> {
        let mut im = self.importer();
        let v = im
            .load1("Data/QueryMods", vec![])
            .map_err(|e| e.to_string())?;
        let mut groups = Vec::new();
        for (g, mods) in table(&v, "QueryMods")?.entries() {
            let g = key(&g)?;
            let mut list = table(&mods, &g)?
                .entries()
                .into_iter()
                .map(|(k, v)| entry(key(&k)?, &v))
                .collect::<Result<Vec<_>>>()?;
            list.sort_by(|a, b| a.key.cmp(&b.key));
            groups.push((g, list));
        }
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(QueryMods { groups })
    }
}
