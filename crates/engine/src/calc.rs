//! The state of a typed calculation after the setup: every actor's
//! database, the active skills' mod lists, the actors and their outputs,
//! with the queries `calcs.perform` and what it calls make, named as
//! upstream names them (`Sum`, `More`, `Flag`, `calcLib.mod`, ...).
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::actor::{EnvActors, OutVal};
use crate::mods::list::Precision;
use crate::mods::{Cfg, Mod, ModType, ModValue, Query, Store, StoreId, Stores, Sym};

pub type Result<T> = std::result::Result<T, String>;

/// The actors' places in [`EnvActors::actors`]
pub const PLAYER: usize = 0;
pub const ENEMY: usize = 1;

/// Mod names as symbols
pub fn syms(names: &[&str]) -> Vec<Sym> {
    names.iter().map(|n| Sym::new(n)).collect()
}

/// A value as a number, as upstream's arithmetic takes it
pub fn number(v: &ModValue) -> Result<f64> {
    match v {
        ModValue::Number(n) => Ok(*n),
        ModValue::Str(s) => crate::mods::store::coerce(s)
            .ok_or_else(|| format!("attempt to perform arithmetic on a string value ({s:?})")),
        v => Err(format!("attempt to perform arithmetic on {v:?}")),
    }
}

/// `math.min` and `math.max` as LuaJIT computes them
pub fn m_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

pub fn m_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// `round(val, dec)` of `Modules/Common.lua`
pub fn round(val: f64, dec: f64) -> f64 {
    if dec == 0.0 {
        (val + 0.5).floor()
    } else {
        let p = 10f64.powf(dec);
        (val * p + 0.5).floor() / p
    }
}

/// `floor(val, dec)` of `Modules/Common.lua`
pub fn floor(val: f64, dec: f64) -> f64 {
    let p = 10f64.powf(dec);
    (val * p).floor() / p
}

/// The stores and actors of a calculation
pub struct Calc<'a> {
    pub stores: Stores,
    pub actors: EnvActors<'a>,
    pub precision: &'a dyn Precision,
    /// Mods a `ReplaceMod` put in a ModDB, which later ones pass over
    /// (upstream marks them `replaced`)
    pub replaced: std::collections::HashSet<(StoreId, Sym, usize)>,
}

impl<'a> Calc<'a> {
    pub fn query(&self) -> Query<'_> {
        Query {
            stores: &self.stores,
            actors: &self.actors,
        }
    }

    /// The store of an actor (`actor.modDB`)
    pub fn db(&self, actor: usize) -> StoreId {
        self.actors.actors[actor].store
    }

    pub fn store(&self, id: StoreId) -> &Store {
        &self.stores.stores[id]
    }

    pub fn store_mut(&mut self, id: StoreId) -> &mut Store {
        &mut self.stores.stores[id]
    }

    /// Adds a store owned by an actor, as its database or one of its lists
    pub fn push_store(&mut self, store: Store, actor: Option<usize>) -> StoreId {
        let id = self.stores.push(store);
        if let Some(a) = actor {
            self.actors.store_actor.insert(id, a);
        }
        id
    }

    pub fn sum(&self, id: StoreId, typ: ModType, cfg: Option<&Cfg>, names: &[&str]) -> Result<f64> {
        self.query().sum(id, typ, cfg, &syms(names))
    }

    pub fn base(&self, id: StoreId, cfg: Option<&Cfg>, names: &[&str]) -> Result<f64> {
        self.sum(id, ModType::Base, cfg, names)
    }

    pub fn inc(&self, id: StoreId, cfg: Option<&Cfg>, names: &[&str]) -> Result<f64> {
        self.sum(id, ModType::Inc, cfg, names)
    }

    pub fn more(&self, id: StoreId, cfg: Option<&Cfg>, names: &[&str]) -> Result<f64> {
        self.query().more(id, cfg, &syms(names))
    }

    pub fn flag(&self, id: StoreId, cfg: Option<&Cfg>, names: &[&str]) -> Result<bool> {
        self.query().flag(id, cfg, &syms(names))
    }

    pub fn override_value(
        &self,
        id: StoreId,
        cfg: Option<&Cfg>,
        names: &[&str],
    ) -> Result<Option<ModValue>> {
        self.query().override_value(id, cfg, &syms(names))
    }

    /// `Override` used as a number
    pub fn override_num(
        &self,
        id: StoreId,
        cfg: Option<&Cfg>,
        names: &[&str],
    ) -> Result<Option<f64>> {
        self.override_value(id, cfg, names)?
            .map(|v| number(&v))
            .transpose()
    }

    pub fn max(&self, id: StoreId, cfg: Option<&Cfg>, names: &[&str]) -> Result<Option<f64>> {
        self.query().extreme(id, cfg, &syms(names), true)
    }

    pub fn min(&self, id: StoreId, cfg: Option<&Cfg>, names: &[&str]) -> Result<Option<f64>> {
        self.query().extreme(id, cfg, &syms(names), false)
    }

    pub fn list(&self, id: StoreId, cfg: Option<&Cfg>, names: &[&str]) -> Result<Vec<ModValue>> {
        self.query().list(id, cfg, &syms(names))
    }

    /// `Tabulate`: each value with its mod
    pub fn tabulate(
        &self,
        id: StoreId,
        typ: Option<ModType>,
        cfg: Option<&Cfg>,
        names: &[&str],
    ) -> Result<Vec<(ModValue, Mod)>> {
        Ok(self
            .query()
            .tabulate(id, typ, cfg, &syms(names))?
            .into_iter()
            .map(|r| (r.value, r.m.clone()))
            .collect())
    }

    pub fn has_mod(
        &self,
        id: StoreId,
        typ: ModType,
        cfg: Option<&Cfg>,
        names: &[&str],
    ) -> Result<bool> {
        self.query().has_mod(id, typ, cfg, &syms(names))
    }

    /// `GetCondition(var, cfg)`
    pub fn get_condition(&self, id: StoreId, var: &str, cfg: Option<&Cfg>) -> Result<bool> {
        self.query().condition(id, Sym::new(var), cfg, false)
    }

    /// `GetMultiplier(var, cfg)`
    pub fn get_multiplier(&self, id: StoreId, var: &str, cfg: Option<&Cfg>) -> Result<f64> {
        self.query().multiplier(id, Sym::new(var), cfg, false)
    }

    /// `calcLib.mod(store, cfg, ...)`
    pub fn calc_mod(&self, id: StoreId, cfg: Option<&Cfg>, names: &[&str]) -> Result<f64> {
        Ok((1.0 + self.inc(id, cfg, names)? / 100.0) * self.more(id, cfg, names)?)
    }

    /// `calcLib.val(store, name, cfg)`
    pub fn val(&self, id: StoreId, name: &str, cfg: Option<&Cfg>) -> Result<f64> {
        let base = self.base(id, cfg, &[name])?;
        if base != 0.0 {
            Ok(base * self.calc_mod(id, cfg, &[name])?)
        } else {
            Ok(0.0)
        }
    }

    /// `AddMod`
    pub fn add(&mut self, id: StoreId, m: Mod) {
        self.stores.stores[id].add(m);
    }

    /// `AddList`
    pub fn add_list(&mut self, id: StoreId, mods: &[Mod]) {
        for m in mods {
            self.add(id, m.clone());
        }
    }

    /// `ScaleAddList(list, scale)`
    pub fn scale_add_list(&mut self, id: StoreId, mods: &[Mod], scale: f64) {
        for m in mods {
            let m = if scale == 1.0 {
                m.clone()
            } else {
                crate::mods::list::scale_mod(m, scale, self.precision)
            };
            self.add(id, m);
        }
    }

    /// `ReplaceMod`: replaces the first mod with the same name, type,
    /// flags and source, in the store or its parents, else adds it. In a
    /// ModDB, a mod put there by a replacement is not replaced again.
    pub fn replace_mod(&mut self, id: StoreId, m: Mod) {
        let mut cur = Some(id);
        while let Some(s) = cur {
            let same = |c: &Mod| {
                c.name == m.name
                    && c.typ == m.typ
                    && c.flags == m.flags
                    && c.keyword_flags == m.keyword_flags
                    && c.source == m.source
            };
            let replaced = &self.replaced;
            let store = &mut self.stores.stores[s];
            match &mut store.mods {
                crate::mods::Mods::Db(by_name) => {
                    let list = by_name.entry(m.name).or_default();
                    if let Some(i) = (0..list.len())
                        .find(|&i| same(&list[i]) && !replaced.contains(&(s, m.name, i)))
                    {
                        list[i] = m.clone();
                        self.replaced.insert((s, m.name, i));
                        return;
                    }
                }
                crate::mods::Mods::List(l) => {
                    if let Some(c) = l.iter_mut().find(|c| same(c)) {
                        *c = m;
                        return;
                    }
                }
            }
            cur = store.parent;
        }
        self.add(id, m);
    }

    /// `store.conditions[var]`
    pub fn cond(&self, id: StoreId, var: &str) -> bool {
        self.store(id)
            .conditions
            .get(&Sym::new(var))
            .copied()
            .unwrap_or(false)
    }

    /// Whether `store.conditions[var]` is set at all
    pub fn cond_set(&self, id: StoreId, var: &str) -> Option<bool> {
        self.store(id).conditions.get(&Sym::new(var)).copied()
    }

    pub fn set_cond(&mut self, id: StoreId, var: &str, v: bool) {
        self.store_mut(id).conditions.insert(Sym::new(var), v);
    }

    /// `store.multipliers[var]`
    pub fn mult(&self, id: StoreId, var: &str) -> Option<f64> {
        self.store(id).multipliers.get(&Sym::new(var)).copied()
    }

    pub fn set_mult(&mut self, id: StoreId, var: &str, v: f64) {
        self.store_mut(id).multipliers.insert(Sym::new(var), v);
    }

    /// `actor.output[k]` as a number
    pub fn out(&self, actor: usize, k: &str) -> Option<f64> {
        self.actors.actors[actor]
            .output
            .get(&Sym::new(k))
            .and_then(OutVal::num)
    }

    pub fn out_val(&self, actor: usize, k: &str) -> Option<&OutVal> {
        self.actors.actors[actor].output.get(&Sym::new(k))
    }

    pub fn set_out(&mut self, actor: usize, k: &str, v: f64) {
        self.actors.actors[actor]
            .output
            .insert(Sym::new(k), OutVal::Num(v));
    }

    pub fn set_out_val(&mut self, actor: usize, k: &str, v: OutVal) {
        self.actors.actors[actor].output.insert(Sym::new(k), v);
    }

    pub fn clear_out(&mut self, actor: usize, k: &str) {
        self.actors.actors[actor].output.remove(&Sym::new(k));
    }
}
