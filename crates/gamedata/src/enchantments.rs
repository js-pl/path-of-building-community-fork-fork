//! `data.enchantments` as `Modules/Data.lua` leaves it: the files' tables,
//! `Flask` as `UtilityFlask`, and each weapon type's lines of `Weapon`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::{Result, Source};
use engine::game_data::Enchantments;
use engine::item_enchant::ENCHANTMENT_SOURCES;
use lua::{Key, TableRef, Value};

fn key(k: &Key) -> Result<String> {
    match k {
        Key::Str(s) => Ok(s.to_str_lossy().into_owned()),
        k => Err(format!("expected a string key, got {k:?}")),
    }
}

fn table(v: &Value) -> Result<TableRef> {
    v.as_table()
        .cloned()
        .ok_or_else(|| format!("expected a table, got {v:?}"))
}

fn strings(v: &Value) -> Result<Vec<String>> {
    table(v)?
        .array()
        .iter()
        .map(|s| {
            s.as_str()
                .map(|s| s.to_str_lossy().into_owned())
                .ok_or_else(|| format!("expected a string, got {s:?}"))
        })
        .collect()
}

/// A table keyed by enchantment source, in `data.enchantmentSource`'s order
/// (a weapon type's leaves out the sources with no lines in the file, as
/// `Data.lua` does)
fn sources(
    t: &TableRef,
    weapon: bool,
    mut lines: impl FnMut(&Value) -> Result<Vec<String>>,
) -> Result<Vec<(String, Vec<String>)>> {
    let mut out = Vec::new();
    for (k, v) in t.entries() {
        let k = key(&k)?;
        if weapon && table(&v)?.array().is_empty() {
            continue;
        }
        if !ENCHANTMENT_SOURCES.iter().any(|(s, _)| *s == k) {
            return Err(format!("unknown enchantment source {k}"));
        }
        out.push((k, lines(&v)?));
    }
    out.sort_by_key(|(k, _)| ENCHANTMENT_SOURCES.iter().position(|(s, _)| s == k));
    Ok(out)
}

impl Source {
    pub fn enchantments(&self) -> Result<Vec<(String, Enchantments)>> {
        let all = table(&self.data.get_str("enchantments"))?;
        let mut out = Vec::new();
        for (k, v) in all.entries() {
            let k = key(&k)?;
            if k == "Weapon" {
                continue;
            }
            let t = table(&v)?;
            let skill_keyed = t.entries().iter().any(|(k, _)| {
                key(k).is_ok_and(|k| !ENCHANTMENT_SOURCES.iter().any(|(s, _)| *s == k))
            });
            let mut e = Enchantments::default();
            if skill_keyed {
                for (skill, v) in t.entries() {
                    e.skills
                        .push((key(&skill)?, sources(&table(&v)?, false, strings)?));
                }
                e.skills.sort_by(|a, b| a.0.cmp(&b.0));
            } else {
                e.sources = sources(&t, false, strings)?;
            }
            if k == "UtilityFlask" {
                out.push(("Flask".to_string(), e.clone()));
            }
            out.push((k, e));
        }
        // Each weapon type's lines: strings as they are, the pairs of
        // lines for that type joined by "/"
        let weapon = table(&all.get_str("Weapon"))?;
        let types = table(&self.data.get_str("weaponTypeInfo"))?;
        let mut type_names: Vec<String> = types
            .entries()
            .iter()
            .map(|(k, _)| key(k))
            .collect::<Result<_>>()?;
        type_names.sort();
        for typ in type_names {
            let lines = |v: &Value| -> Result<Vec<String>> {
                let mut l = Vec::new();
                for e in table(v)?.array() {
                    match &e {
                        Value::Table(t) => {
                            if table(&t.get_str("types"))?.get_str(&typ).truthy() {
                                l.push(strings(&e)?.join("/"));
                            }
                        }
                        e => l.push(
                            e.as_str()
                                .map(|s| s.to_str_lossy().into_owned())
                                .ok_or_else(|| format!("expected a string, got {e:?}"))?,
                        ),
                    }
                }
                Ok(l)
            };
            out.push((
                typ.clone(),
                Enchantments {
                    sources: sources(&weapon, true, lines)?,
                    skills: vec![],
                },
            ));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}
