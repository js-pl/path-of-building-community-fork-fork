//! Mod lists in code: what upstream does with a plain `ModList` that no
//! actor owns (`Classes/ModList.lua`, `Classes/ModStore.lua`), such as an
//! item's or a passive node's mods. Queries go through the typed stores
//! with no actors.

use super::store::{Actors, Cfg, Query, Result, Store, StoreId, Stores};
use super::sym::Sym;
use super::tag::ItemCondition;
use super::types::*;
use std::rc::Rc;

/// No actors: the queries of a list nothing owns
pub struct NoActors;

impl Actors for NoActors {
    fn actor_store(&self, _: StoreId, _: Sym) -> Option<StoreId> {
        None
    }
    fn output(&self, _: StoreId, _: Sym) -> Option<f64> {
        None
    }
    fn special_stat(&self, _: StoreId, _: Sym, _: Option<&Cfg>) -> Option<f64> {
        None
    }
    fn one_handed_added(&self, _: StoreId, _: Sym) -> Option<bool> {
        None
    }
    fn monster_tags(&self, _: StoreId) -> Option<Vec<Rc<str>>> {
        None
    }
    fn item_condition(&self, _: StoreId, _: &ItemCondition, _: Option<&Cfg>) -> bool {
        false
    }
    fn gem_is_type(&self, _: &Cfg, _: &str) -> bool {
        false
    }
    fn game_id(&self, _: &str) -> Option<Rc<str>> {
        None
    }
    fn high_precision(&self, _: Sym, _: ModType) -> Option<f64> {
        None
    }
}

/// Runs a query on a plain list of mods
fn query<T>(mods: &[Mod], f: impl FnOnce(&Query, StoreId) -> T) -> T {
    let mut store = Store::list();
    for m in mods {
        store.add(m.clone());
    }
    let mut stores = Stores::default();
    let id = stores.push(store);
    f(
        &Query {
            stores: &stores,
            actors: &NoActors,
        },
        id,
    )
}

/// `modList:Sum(type, nil, name)`
pub fn sum(mods: &[Mod], typ: ModType, name: &str) -> Result<f64> {
    query(mods, |q, id| q.sum(id, typ, None, &[Sym::new(name)]))
}

/// `modList:More(nil, name)`
pub fn more(mods: &[Mod], name: &str) -> Result<f64> {
    query(mods, |q, id| q.more(id, None, &[Sym::new(name)]))
}

/// `modList:Flag(nil, name)`
pub fn flag(mods: &[Mod], name: &str) -> Result<bool> {
    query(mods, |q, id| q.flag(id, None, &[Sym::new(name)]))
}

/// `modList:List(nil, name)`
pub fn list(mods: &[Mod], name: &str) -> Result<Vec<ModValue>> {
    query(mods, |q, id| q.list(id, None, &[Sym::new(name)]))
}

/// `modList:Tabulate(type, nil, name)`: each mod's value with the mod
pub fn tabulate(mods: &[Mod], typ: Option<ModType>, name: &str) -> Result<Vec<(ModValue, Mod)>> {
    query(mods, |q, id| {
        Ok(q.tabulate(id, typ, None, &[Sym::new(name)])?
            .into_iter()
            .map(|r| (r.value, r.m.clone()))
            .collect())
    })
}

/// `modLib.compareModParams(a, b)`: same name, type, flags and tags
pub fn same_params(a: &Mod, b: &Mod) -> bool {
    a.name == b.name
        && a.typ == b.typ
        && a.flags == b.flags
        && a.keyword_flags == b.keyword_flags
        && a.tags == b.tags
}

/// `ModList:MergeMod(mod)`: adds the value to a mod with the same params,
/// for the summed types, else adds the mod
pub fn merge_mod(mods: &mut Vec<Mod>, m: Mod) -> Result<()> {
    if !merge_value(mods, &m)? {
        mods.push(m);
    }
    Ok(())
}

/// `ModList:MergeMod(mod, true)`: adds the value to a mod with the same
/// params, for the summed types; other mods are dropped
pub fn merge_additive(mods: &mut [Mod], m: &Mod) -> Result<()> {
    merge_value(mods, m).map(|_| ())
}

/// Adds the value of a summed mod to one with the same params, if any
fn merge_value(mods: &mut [Mod], m: &Mod) -> Result<bool> {
    if !matches!(m.typ, ModType::Base | ModType::Inc | ModType::More) {
        return Ok(false);
    }
    let Some(existing) = mods.iter_mut().find(|e| same_params(e, m)) else {
        return Ok(false);
    };
    existing.value = match (&existing.value, &m.value) {
        (ModValue::Number(x), ModValue::Number(y)) => ModValue::Number(x + y),
        (a, b) => return Err(format!("attempt to perform arithmetic on {a:?} and {b:?}")),
    };
    Ok(true)
}

/// What scaling needs of the data: `data.highPrecisionMods[name][type]`
/// and `data.defaultHighPrecision`
pub trait Precision {
    fn high_precision(&self, name: Sym, typ: ModType) -> Option<f64>;
    fn default_high_precision(&self) -> f64;
}

/// `round(val, dec)` of `Modules/Common.lua`
fn round(val: f64, dec: f64) -> f64 {
    (val * 10f64.powf(dec) + 0.5).floor() / 10f64.powf(dec)
}

/// `ModStore:ScaleAddMod(mod, scale)`: the mod with its value scaled, as
/// it is added
pub fn scale_mod(m: &Mod, scale: f64, p: &dyn Precision) -> Mod {
    let unscalable = m.tags.iter().any(|t| match &t.kind {
        super::tag::TagKind::Other { fields, .. } => {
            fields.get("unscalable").is_some_and(ModValue::truthy)
        }
        _ => false,
    });
    if scale == 1.0 || unscalable {
        return m.clone();
    }
    let mut scaled = m.clone();
    if let ModValue::Record(r) = &mut scaled.value {
        if r.get("mod").is_some() {
            if let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod")) {
                scale_value(inner, scale, p);
            }
        } else if let Some(ModValue::Str(k)) = r.get("keyOfScaledMod").cloned()
            && let Some(ModValue::Number(v)) = r.get_mut(Sym::new(&k))
        {
            *v = round(*v * scale, 2.0);
        }
        return scaled;
    }
    scale_value(&mut scaled, scale, p);
    scaled
}

/// Scales a mod's number value with the precision upstream keeps for it
fn scale_value(m: &mut Mod, scale: f64, p: &dyn Precision) {
    if let ModValue::Number(v) = m.value {
        let precision = p
            .high_precision(m.name, m.typ)
            .or_else(|| (v.floor() != v).then(|| p.default_high_precision()));
        m.value = ModValue::Number(match precision {
            Some(prec) => {
                let power = 10f64.powf(prec);
                (v * scale * power).floor() / power
            }
            None => round(v * scale, 2.0).trunc(),
        });
    }
}
