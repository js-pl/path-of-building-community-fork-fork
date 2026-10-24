//! Modifier stores and their queries: upstream's `ModStore`, `ModDB` and
//! `ModList` (`Classes/ModStore.lua`, `Classes/ModDB.lua`,
//! `Classes/ModList.lua`), with the same arithmetic in the same order, so
//! results are bit for bit upstream's.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use super::sym::Sym;
use super::tag::*;
use super::types::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

/// Index of a store in [`Stores`].
pub type StoreId = usize;

/// The mods of a store: by name (a ModDB) or in one list (a ModList).
#[derive(Clone, Debug)]
pub enum Mods {
    Db(HashMap<Sym, Vec<Mod>>),
    List(Vec<Mod>),
}

#[derive(Clone, Debug)]
pub struct Store {
    pub mods: Mods,
    /// Queries continue into the parent
    pub parent: Option<StoreId>,
    pub conditions: HashMap<Sym, bool>,
    pub multipliers: HashMap<Sym, f64>,
}

impl Store {
    pub fn db() -> Store {
        Store {
            mods: Mods::Db(HashMap::new()),
            parent: None,
            conditions: HashMap::new(),
            multipliers: HashMap::new(),
        }
    }

    pub fn list() -> Store {
        Store {
            mods: Mods::List(Vec::new()),
            ..Store::db()
        }
    }

    /// `AddMod`
    pub fn add(&mut self, m: Mod) {
        match &mut self.mods {
            Mods::Db(by_name) => by_name.entry(m.name).or_default().push(m),
            Mods::List(list) => list.push(m),
        }
    }
}

/// The stores of a calculation: every actor's ModDB and the ModLists made
/// on the way. Parents and actors refer to each other by index.
#[derive(Clone, Debug, Default)]
pub struct Stores {
    pub stores: Vec<Store>,
}

impl Stores {
    pub fn push(&mut self, s: Store) -> StoreId {
        self.stores.push(s);
        self.stores.len() - 1
    }
}

/// What a query is restricted to and evaluated in: upstream's `cfg` tables
/// (`skillCfg` and the like). `None` fields are nil in upstream.
#[derive(Clone, Debug, Default)]
pub struct Cfg {
    pub flags: Option<u32>,
    pub keyword_flags: Option<u32>,
    pub source: Option<Rc<str>>,
    pub skill_name: Option<Rc<str>>,
    pub summon_skill_name: Option<Rc<str>>,
    /// `skillGrantedEffect.id`, when there is a granted effect
    pub skill_id: Option<Option<Rc<str>>>,
    /// The truthy `skillGrantedEffect.baseFlags`, or else `baseFlags`
    pub base_flags: Option<HashSet<Sym>>,
    pub skill_part: Option<f64>,
    pub skill_types: Option<HashSet<i64>>,
    /// The truthy `skillCond` entries
    pub skill_cond: Option<HashSet<Sym>>,
    pub skill_dist: Option<f64>,
    pub slot_name: Option<Sym>,
    pub actor: Option<Sym>,
    pub skill_stats: Option<HashMap<Sym, f64>>,
    pub socket_color: Option<Rc<str>>,
    pub socket_num: Option<f64>,
    pub strength_gems: Option<f64>,
    pub dexterity_gems: Option<f64>,
    pub intelligence_gems: Option<f64>,
}

/// What queries need from outside the stores: actors, their outputs and
/// equipment, game data.
pub trait Actors {
    /// The store of the actor `actor` relative to `from` (`getActor` of
    /// `ModStore.lua`, then `.modDB`); `None` when there is no such actor.
    fn actor_store(&self, from: StoreId, actor: Sym) -> Option<StoreId>;
    /// `actor.output[stat]` of the store's actor, when set
    fn output(&self, store: StoreId, stat: Sym) -> Option<f64>;
    /// `GetStat` for the stats upstream computes from the active skills
    /// (`ManaReservedPercent`, `LifeReservedPercent`, `ManaUnreserved`);
    /// `None` when the stat is none of them
    fn special_stat(&self, store: StoreId, stat: Sym, cfg: Option<&Cfg>) -> Option<f64>;
    /// `allOneH["Added" .. var]` of the store's actor (the weapon counting as
    /// all one handed weapons), when there is one and it is set
    fn one_handed_added(&self, store: StoreId, var: Sym) -> Option<bool>;
    /// `actor.minionData.monsterTags`
    fn monster_tags(&self, store: StoreId) -> Option<Vec<Rc<str>>>;
    /// Whether an `ItemCondition` tag holds
    fn item_condition(&self, store: StoreId, tag: &ItemCondition, cfg: Option<&Cfg>) -> bool;
    /// `calcLib.gemIsType(cfg.skillGem, keyword)`, false without a gem
    fn gem_is_type(&self, cfg: &Cfg, keyword: &str) -> bool;
    /// `calcLib.getGameIdFromGemName(name, true)`
    fn game_id(&self, gem_name: &str) -> Option<Rc<str>>;
    /// `data.highPrecisionMods[name][type]`
    fn high_precision(&self, name: Sym, typ: ModType) -> Option<f64>;
}

/// A row of `Tabulate`.
#[derive(Clone, Debug)]
pub struct Row<'a> {
    pub value: ModValue,
    pub m: &'a Mod,
}

/// The mod filter all queries share.
struct Filter<'c> {
    flags: u32,
    keyword_flags: u32,
    source: Option<&'c str>,
}

impl<'c> Filter<'c> {
    fn new(cfg: Option<&'c Cfg>) -> Filter<'c> {
        Filter {
            flags: cfg.and_then(|c| c.flags).unwrap_or(0),
            keyword_flags: cfg.and_then(|c| c.keyword_flags).unwrap_or(0),
            source: cfg.and_then(|c| c.source.as_deref()),
        }
    }

    fn matches(&self, m: &Mod) -> bool {
        self.flags & m.flags == m.flags
            && match_keyword_flags(self.keyword_flags, m.keyword_flags)
            && self.source.is_none_or(|s| m.source_name() == Some(s))
    }
}

const MATCH_ALL: u32 = 0x4000_0000;

/// `MatchKeywordFlags` of `Data/Global.lua`
pub fn match_keyword_flags(keyword_flags: u32, mod_flags: u32) -> bool {
    let masked = mod_flags & !MATCH_ALL;
    let k = keyword_flags & !MATCH_ALL;
    if mod_flags & MATCH_ALL != 0 {
        k & masked == masked
    } else {
        masked == 0 || k & masked != 0
    }
}

/// `round(val, dec)` of `Modules/Common.lua`
fn round(val: f64, dec: f64) -> f64 {
    (val * 10f64.powf(dec) + 0.5).floor() / 10f64.powf(dec)
}

/// `math.min` and `math.max` as upstream's runtime computes them
fn min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

fn max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// A value in arithmetic: a number, or a string upstream's runtime coerces
/// (upstream's data has a few numbers as strings, e.g. `"10"`)
fn num(v: &ModValue) -> Result<f64> {
    match v {
        ModValue::Number(n) => Ok(*n),
        ModValue::Str(s) => coerce(s)
            .ok_or_else(|| format!("attempt to perform arithmetic on a string value ({s:?})")),
        v => Err(format!("attempt to perform arithmetic on {v:?}")),
    }
}

/// Lua's string to number coercion: decimal or `0x` hex, surrounding
/// whitespace allowed, no `inf` or `nan`
pub fn coerce(s: &str) -> Option<f64> {
    let t = s.trim_matches(|c: char| c.is_ascii_whitespace());
    let (neg, body) = match t.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, t),
    };
    let v = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        hex.bytes().fold(0.0, |acc, b| {
            acc * 16.0 + (b as char).to_digit(16).unwrap() as f64
        })
    } else {
        if !body
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
            || !body
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_digit() || b == b'.')
        {
            return None;
        }
        body.parse::<f64>().ok()?
    };
    Some(if neg { -v } else { v })
}

/// Queries over a calculation's stores.
pub struct Query<'a> {
    pub stores: &'a Stores,
    pub actors: &'a dyn Actors,
}

impl<'a> Query<'a> {
    fn store(&self, id: StoreId) -> &'a Store {
        &self.stores.stores[id]
    }

    /// Calls `f` with the mods of each name in turn, and with `None` after
    /// each name; a ModDB visits the name's bucket, a ModList its whole list.
    /// Stops when `f` returns false.
    fn each(
        &self,
        id: StoreId,
        names: &[Sym],
        mut f: impl FnMut(Option<&'a Mod>) -> Result<bool>,
    ) -> Result<bool> {
        let store = self.store(id);
        for name in names {
            match &store.mods {
                Mods::Db(by_name) => {
                    for m in by_name.get(name).map_or(&[][..], |v| &v[..]) {
                        if !f(Some(m))? {
                            return Ok(false);
                        }
                    }
                }
                Mods::List(list) => {
                    for m in list.iter().filter(|m| m.name == *name) {
                        if !f(Some(m))? {
                            return Ok(false);
                        }
                    }
                }
            }
            if !f(None)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn is_db(&self, id: StoreId) -> bool {
        matches!(self.store(id).mods, Mods::Db(_))
    }

    /// `Sum(modType, cfg, ...)`
    pub fn sum(&self, id: StoreId, typ: ModType, cfg: Option<&Cfg>, names: &[Sym]) -> Result<f64> {
        self.sum_in(id, id, typ, cfg, &Filter::new(cfg), names)
    }

    fn sum_in(
        &self,
        id: StoreId,
        context: StoreId,
        typ: ModType,
        cfg: Option<&Cfg>,
        flt: &Filter,
        names: &[Sym],
    ) -> Result<f64> {
        let db = self.is_db(id);
        // A ModDB shares one globalLimits across the names, a ModList none
        let mut limits = db.then(HashMap::new);
        let mut result = 0.0;
        self.each(id, names, |m| {
            let Some(m) = m else {
                return Ok(true);
            };
            if m.typ == typ && flt.matches(m) {
                let v = if m.tags.is_empty() {
                    num(&m.value)?
                } else {
                    match self.eval_mod(context, m, cfg, limits.as_mut())? {
                        Some(v) => num(&v)?,
                        None => 0.0,
                    }
                };
                result += v;
            }
            Ok(true)
        })?;
        if let Some(p) = self.store(id).parent {
            result += self.sum_in(p, context, typ, cfg, flt, names)?;
        }
        Ok(result)
    }

    /// `More(cfg, ...)`
    pub fn more(&self, id: StoreId, cfg: Option<&Cfg>, names: &[Sym]) -> Result<f64> {
        self.more_in(id, id, cfg, &Filter::new(cfg), names)
    }

    fn more_in(
        &self,
        id: StoreId,
        context: StoreId,
        cfg: Option<&Cfg>,
        flt: &Filter,
        names: &[Sym],
    ) -> Result<f64> {
        let db = self.is_db(id);
        let mut limits = db.then(HashMap::new);
        let mut result = 1.0;
        let mut precision: Option<f64> = None;
        let mut mod_result = 1.0;
        self.each(id, names, |m| {
            let Some(m) = m else {
                // Each name's product is rounded before it is applied
                match precision {
                    Some(p) => {
                        let power = 10f64.powf(p);
                        result = (result * mod_result * power).floor() / power;
                    }
                    None => result *= round(mod_result, 2.0),
                }
                mod_result = 1.0;
                return Ok(true);
            };
            if m.typ == ModType::More && flt.matches(m) {
                let value = if m.tags.is_empty() {
                    num(&m.value)?
                } else {
                    match self.eval_mod(context, m, cfg, limits.as_mut())? {
                        Some(v) => num(&v)?,
                        None => 0.0,
                    }
                };
                mod_result *= 1.0 + value / 100.0;
                let hp = self.actors.high_precision(m.name, m.typ);
                precision = match precision {
                    Some(p) => Some(hp.map_or(p, |h| max(p, h))),
                    None => hp,
                };
            }
            Ok(true)
        })?;
        if let Some(p) = self.store(id).parent {
            result *= self.more_in(p, context, cfg, flt, names)?;
        }
        Ok(result)
    }

    /// The first matching mod of `typ` with a value: `Flag` and `Override`.
    fn first_in(
        &self,
        id: StoreId,
        context: StoreId,
        typ: ModType,
        cfg: Option<&Cfg>,
        flt: &Filter,
        names: &[Sym],
    ) -> Result<Option<ModValue>> {
        let mut found = None;
        self.each(id, names, |m| {
            let Some(m) = m else {
                return Ok(true);
            };
            if m.typ == typ && flt.matches(m) {
                let v = if m.tags.is_empty() {
                    Some(m.value.clone())
                } else {
                    self.eval_mod(context, m, cfg, None)?
                };
                if let Some(v) = v.filter(ModValue::truthy) {
                    found = Some(v);
                    return Ok(false);
                }
            }
            Ok(true)
        })?;
        if found.is_some() {
            return Ok(found);
        }
        match self.store(id).parent {
            Some(p) => self.first_in(p, context, typ, cfg, flt, names),
            None => Ok(None),
        }
    }

    /// `Flag(cfg, ...)`
    pub fn flag(&self, id: StoreId, cfg: Option<&Cfg>, names: &[Sym]) -> Result<bool> {
        Ok(self
            .first_in(id, id, ModType::Flag, cfg, &Filter::new(cfg), names)?
            .is_some())
    }

    /// `Override(cfg, ...)`
    pub fn override_value(
        &self,
        id: StoreId,
        cfg: Option<&Cfg>,
        names: &[Sym],
    ) -> Result<Option<ModValue>> {
        self.first_in(id, id, ModType::Override, cfg, &Filter::new(cfg), names)
    }

    /// `List(cfg, ...)`
    pub fn list(&self, id: StoreId, cfg: Option<&Cfg>, names: &[Sym]) -> Result<Vec<ModValue>> {
        let flt = Filter::new(cfg);
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(s) = cur {
            self.each(s, names, |m| {
                let Some(m) = m else {
                    return Ok(true);
                };
                if m.typ == ModType::List && flt.matches(m) {
                    let v = if m.tags.is_empty() {
                        Some(m.value.clone())
                    } else {
                        self.eval_mod(id, m, cfg, None)?
                    };
                    if let Some(v) = v.filter(ModValue::truthy) {
                        out.push(v);
                    }
                }
                Ok(true)
            })?;
            cur = self.store(s).parent;
        }
        Ok(out)
    }

    /// `Tabulate(modType, cfg, ...)`; `None` tabulates every type.
    pub fn tabulate(
        &self,
        id: StoreId,
        typ: Option<ModType>,
        cfg: Option<&Cfg>,
        names: &[Sym],
    ) -> Result<Vec<Row<'a>>> {
        let flt = Filter::new(cfg);
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(s) = cur {
            let mut limits = self.is_db(s).then(HashMap::new);
            self.each(s, names, |m| {
                let Some(m) = m else {
                    return Ok(true);
                };
                if typ.is_none_or(|t| m.typ == t) && flt.matches(m) {
                    let v = if m.tags.is_empty() {
                        Some(m.value.clone())
                    } else {
                        self.eval_mod(id, m, cfg, limits.as_mut())?
                    };
                    if let Some(v) = v.filter(ModValue::truthy)
                        && (v != ModValue::Number(0.0) || m.typ == ModType::Override)
                    {
                        out.push(Row { value: v, m });
                    }
                }
                Ok(true)
            })?;
            cur = self.store(s).parent;
        }
        Ok(out)
    }

    /// `Max(cfg, ...)` and `Min(cfg, ...)`
    pub fn extreme(
        &self,
        id: StoreId,
        cfg: Option<&Cfg>,
        names: &[Sym],
        is_max: bool,
    ) -> Result<Option<f64>> {
        let typ = if is_max { ModType::Max } else { ModType::Min };
        let mut best: Option<f64> = None;
        for row in self.tabulate(id, Some(typ), cfg, names)? {
            let v = match self.eval_mod(id, row.m, cfg, None)? {
                Some(v) => num(&v)?,
                None => return Err("attempt to compare nil with number".into()),
            };
            let better = if is_max {
                best.unwrap_or(0.0) < v
            } else {
                best.is_none_or(|b| v < b)
            };
            if better {
                best = Some(v);
            }
        }
        Ok(best)
    }

    /// `HasMod(modType, cfg, ...)`, of a ModDB
    pub fn has_mod(
        &self,
        id: StoreId,
        typ: ModType,
        cfg: Option<&Cfg>,
        names: &[Sym],
    ) -> Result<bool> {
        let flt = Filter::new(cfg);
        let mut cur = Some(id);
        while let Some(s) = cur {
            if !self.is_db(s) {
                return Err("attempt to call method 'HasModInternal' (a nil value)".into());
            }
            let mut found = false;
            self.each(s, names, |m| {
                let Some(m) = m else {
                    return Ok(true);
                };
                found = m.typ == typ && flt.matches(m);
                Ok(!found)
            })?;
            if found {
                return Ok(true);
            }
            cur = self.store(s).parent;
        }
        Ok(false)
    }

    /// `GetCondition(var, cfg, noMod)`
    pub fn condition(
        &self,
        id: StoreId,
        var: Sym,
        cfg: Option<&Cfg>,
        no_mod: bool,
    ) -> Result<bool> {
        let store = self.store(id);
        if store.conditions.get(&var).copied().unwrap_or(false) {
            return Ok(true);
        }
        if let Some(p) = store.parent
            && self.condition(p, var, cfg, true)?
        {
            return Ok(true);
        }
        if no_mod {
            return Ok(false);
        }
        let name = Sym::new(&format!("Condition:{var}"));
        self.flag(id, cfg, &[name])
    }

    /// `GetMultiplier(var, cfg, noMod)`
    pub fn multiplier(
        &self,
        id: StoreId,
        var: Sym,
        cfg: Option<&Cfg>,
        no_mod: bool,
    ) -> Result<f64> {
        let name = (!no_mod).then(|| Sym::new(&format!("Multiplier:{var}")));
        if let Some(name) = name
            && let Some(o) = self.override_value(id, cfg, &[name])?
        {
            return num(&o);
        }
        let store = self.store(id);
        let own = store.multipliers.get(&var).copied().unwrap_or(0.0);
        let inherited = match store.parent {
            Some(p) => self.multiplier(p, var, cfg, true)?,
            None => 0.0,
        };
        let summed = match name {
            Some(name) => self.sum(id, ModType::Base, cfg, &[name])?,
            None => 0.0,
        };
        Ok(own + inherited + summed)
    }

    /// `GetStat(stat, cfg)`
    pub fn stat(&self, id: StoreId, stat: Sym, cfg: Option<&Cfg>) -> Result<f64> {
        if let Some(v) = self.actors.special_stat(id, stat, cfg) {
            return Ok(v);
        }
        if let Some(v) = self.actors.output(id, stat) {
            return Ok(v);
        }
        Ok(cfg
            .and_then(|c| c.skill_stats.as_ref())
            .and_then(|s| s.get(&stat).copied())
            .unwrap_or(0.0))
    }

    fn actor_db(&self, id: StoreId, actor: Option<Sym>) -> Option<StoreId> {
        actor.and_then(|a| self.actors.actor_store(id, a))
    }

    fn multiplier_of(
        &self,
        target: StoreId,
        vars: &Option<Names>,
        cfg: Option<&Cfg>,
    ) -> Result<f64> {
        match vars {
            Some(Names::List(list)) => {
                let mut base = 0.0;
                for v in list {
                    base += self.multiplier(target, *v, cfg, false)?;
                }
                Ok(base)
            }
            Some(Names::One(v)) => self.multiplier(target, *v, cfg, false),
            None => Err("attempt to concatenate a nil value".into()),
        }
    }

    fn stat_of(&self, target: StoreId, stats: &Option<Names>, cfg: Option<&Cfg>) -> Result<f64> {
        match stats {
            Some(Names::List(list)) => {
                let mut base = 0.0;
                for s in list {
                    base += self.stat(target, *s, cfg)?;
                }
                Ok(base)
            }
            Some(Names::One(s)) => self.stat(target, *s, cfg),
            None => Ok(0.0),
        }
    }

    /// `EvalMod(mod, cfg, globalLimits)`: the mod's value with its tags
    /// applied; `None` when a tag does not hold.
    pub fn eval_mod(
        &self,
        id: StoreId,
        m: &Mod,
        cfg: Option<&Cfg>,
        limits: Option<&mut HashMap<Sym, f64>>,
    ) -> Result<Option<ModValue>> {
        let mut value = m.value.clone();
        for tag in &m.tags {
            if !self.apply(id, &tag.kind, &mut value, cfg)? {
                return Ok(None);
            }
        }
        if let Some(limits) = limits {
            for tag in &m.tags {
                if let Some((limit, key)) = tag.global_limit {
                    let v = num(&value)?;
                    let cur = *limits.entry(key).or_insert(0.0);
                    let v = if cur + v > limit { limit - cur } else { v };
                    value = ModValue::Number(v);
                    limits.insert(key, cur + v);
                }
            }
        }
        Ok(Some(value))
    }

    /// Applies one tag; false when it does not hold.
    fn apply(
        &self,
        id: StoreId,
        tag: &TagKind,
        value: &mut ModValue,
        cfg: Option<&Cfg>,
    ) -> Result<bool> {
        match tag {
            TagKind::Multiplier(t) => {
                let limit_target = match t.limit_actor {
                    Some(a) => match self.actors.actor_store(id, a) {
                        Some(s) => s,
                        None => return Ok(false),
                    },
                    None => id,
                };
                let target = match t.actor {
                    Some(a) => match self.actors.actor_store(id, a) {
                        Some(s) => s,
                        None => return Ok(false),
                    },
                    None => id,
                };
                let base = self.multiplier_of(target, &t.vars, cfg)?;
                let div = match t.div_var {
                    Some(v) => self.multiplier(id, v, cfg, false)?,
                    None => t.div.unwrap_or(1.0),
                };
                let mut mult = if t.no_floor {
                    base / div
                } else {
                    (base / div + 0.0001).floor()
                };
                let (mut limit_total, mut limit_neg_total) = (None, None);
                if t.limit.is_some() || t.limit_var.is_some() || t.limit_stat.is_some() {
                    let limit = match (t.limit, t.limit_var, t.limit_stat) {
                        (Some(l), _, _) => l,
                        (None, Some(v), _) => self.multiplier(limit_target, v, cfg, false)?,
                        (None, None, Some(s)) => self.stat(limit_target, s, cfg)?,
                        (None, None, None) => unreachable!(),
                    };
                    if t.limit_total {
                        limit_total = Some(limit);
                    } else if t.limit_neg_total {
                        limit_neg_total = Some(limit);
                    } else {
                        mult = min(mult, limit);
                    }
                }
                if t.invert && mult != 0.0 {
                    mult = 1.0 / mult;
                }
                scale(value, mult, t.base, limit_total, limit_neg_total, false)?;
            }
            TagKind::MultiplierThreshold(t) => {
                let threshold_target = match t.threshold_actor {
                    Some(a) => match self.actors.actor_store(id, a) {
                        Some(s) => s,
                        None => return Ok(false),
                    },
                    None => id,
                };
                let target = match t.actor {
                    Some(a) => match self.actors.actor_store(id, a) {
                        Some(s) => s,
                        None => return Ok(false),
                    },
                    None => id,
                };
                let mult = self.multiplier_of(target, &t.vars, cfg)?;
                let threshold = match t.threshold {
                    Some(th) => th,
                    None => {
                        let at = if t.threshold_actor.is_some() {
                            threshold_target
                        } else {
                            target
                        };
                        let var = t
                            .threshold_var
                            .ok_or("attempt to concatenate a nil value")?;
                        self.multiplier(at, var, cfg, false)?
                    }
                };
                if (t.upper && mult > threshold)
                    || (t.equals && mult != threshold)
                    || (!t.upper && mult < threshold)
                {
                    return Ok(false);
                }
            }
            TagKind::PerStat(t) => {
                let target = self.actor_db(id, t.actor).unwrap_or(id);
                let base = self.stat_of(target, &t.stats, cfg)?;
                let div = match t.div_var {
                    Some(v) => self.multiplier(id, v, cfg, false)?,
                    None => t.div.unwrap_or(1.0),
                };
                let mut mult = (base / div + 0.0001).floor();
                let mut limit_total = None;
                if t.limit.is_some() || t.limit_var.is_some() {
                    let limit = match (t.limit, t.limit_var) {
                        (Some(l), _) => l,
                        (None, Some(v)) => self.multiplier(id, v, cfg, false)?,
                        (None, None) => unreachable!(),
                    };
                    if t.limit_total {
                        limit_total = Some(limit);
                    } else {
                        mult = min(mult, limit);
                    }
                }
                scale(value, mult, t.base, limit_total, None, false)?;
            }
            TagKind::PercentStat(t) => {
                let target = self.actor_db(id, t.actor).unwrap_or(id);
                let base = self.stat_of(target, &t.stats, cfg)?;
                let percent = match (t.percent, t.percent_var) {
                    (Some(p), _) => Some(p),
                    (None, Some(v)) => Some(self.multiplier(id, v, cfg, false)?),
                    (None, None) => None,
                };
                let mut mult = base * percent.map_or(1.0, |p| p / 100.0);
                if t.floor {
                    mult = mult.floor();
                }
                let mut limit_total = None;
                if t.limit.is_some() || t.limit_var.is_some() {
                    let limit = match (t.limit, t.limit_var) {
                        (Some(l), _) => l,
                        (None, Some(v)) => self.multiplier(id, v, cfg, false)?,
                        (None, None) => unreachable!(),
                    };
                    if t.limit_total {
                        limit_total = Some(limit);
                    } else {
                        mult = min(mult, limit);
                    }
                }
                scale(value, mult, t.base, limit_total, None, true)?;
            }
            TagKind::StatThreshold(t) => {
                let stat = match &t.stats {
                    // Upstream's loop variable shadows the sum, which stays 0
                    Some(Names::List(list)) => {
                        for s in list {
                            self.stat(id, *s, cfg)?;
                        }
                        return Err(
                            "attempt to perform arithmetic on a string value (StatThreshold statList)"
                                .into(),
                        );
                    }
                    Some(Names::One(s)) => self.stat(id, *s, cfg)?,
                    None => self.stat(id, Sym::new(""), cfg)?,
                };
                let mut threshold = match (t.threshold, t.threshold_stat) {
                    (Some(th), _) => th,
                    (None, Some(s)) => self.stat(id, s, cfg)?,
                    (None, None) => 0.0,
                };
                if t.threshold_percent.is_some() || t.threshold_percent_var.is_some() {
                    let p = match (t.threshold_percent, t.threshold_percent_var) {
                        (Some(p), _) => p,
                        (None, Some(v)) => self.multiplier(id, v, cfg, false)?,
                        (None, None) => unreachable!(),
                    };
                    threshold *= p / 100.0;
                }
                if (t.upper && stat > threshold) || (!t.upper && stat < threshold) {
                    return Ok(false);
                }
            }
            TagKind::DistanceRamp(ramp) => {
                let Some(dist) = cfg.and_then(|c| c.skill_dist) else {
                    return Ok(false);
                };
                let (first, last) = match (ramp.first(), ramp.last()) {
                    (Some(f), Some(l)) => (*f, *l),
                    _ => return Err("attempt to index a nil value (ramp)".into()),
                };
                let v = num(value)?;
                let f = if dist <= first.0 {
                    first.1
                } else if dist >= last.0 {
                    last.1
                } else {
                    let mut f = None;
                    for w in ramp.windows(2) {
                        let (dat, next) = (w[0], w[1]);
                        if dist <= next.0 {
                            f = Some(dat.1 + (next.1 - dat.1) * (dist - dat.0) / (next.0 - dat.0));
                            break;
                        }
                    }
                    match f {
                        Some(f) => f,
                        None => return Ok(true),
                    }
                };
                *value = ModValue::Number(v * f);
            }
            TagKind::MeleeProximity(ramp) => {
                let Some(dist) = cfg.and_then(|c| c.skill_dist) else {
                    return Ok(false);
                };
                let max_bonus = *ramp.first().ok_or("attempt to index a nil value (ramp)")?;
                let v = num(value)?;
                if dist <= 15.0 {
                    *value = ModValue::Number(v * max_bonus);
                } else if (16.0..=39.0).contains(&dist) {
                    *value =
                        ModValue::Number(v * (max_bonus - ((max_bonus / 25.0) * (dist - 15.0))));
                } else if dist >= 40.0 {
                    *value = ModValue::Number(0.0);
                }
            }
            TagKind::Limit { limit, limit_var } => {
                let limit = match (limit, limit_var) {
                    (Some(l), _) => *l,
                    (None, Some(v)) => self.multiplier(id, *v, cfg, false)?,
                    (None, None) => return Err("attempt to concatenate a nil value".into()),
                };
                *value = ModValue::Number(min(num(value)?, limit));
            }
            TagKind::Condition { vars, neg } => {
                let skill_cond = |v: Sym| {
                    cfg.and_then(|c| c.skill_cond.as_ref())
                        .is_some_and(|s| s.contains(&v))
                };
                let mut matched = false;
                for var in vars.iter() {
                    if *neg && let Some(added) = self.actors.one_handed_added(id, var) {
                        // A condition the weapon set itself does not count
                        // against a negated tag
                        if !added {
                            return Ok(false);
                        }
                        continue;
                    }
                    if self.condition(id, var, cfg, false)? || skill_cond(var) {
                        matched = true;
                        break;
                    }
                }
                if *neg {
                    matched = !matched;
                }
                if !matched {
                    return Ok(false);
                }
            }
            TagKind::ActorCondition { actor, vars, neg } => {
                let mut matched = false;
                let target = match actor {
                    Some(a) => self.actors.actor_store(id, *a),
                    None => Some(id),
                };
                match (target, vars) {
                    (Some(target), Some(vars)) => {
                        for var in vars.iter() {
                            if self.condition(target, var, cfg, false)? {
                                matched = true;
                                break;
                            }
                        }
                    }
                    _ => {
                        if let (Some(a), Some(c)) = (actor, cfg) {
                            matched = c.actor == Some(*a);
                        }
                    }
                }
                if *neg {
                    matched = !matched;
                }
                if !matched {
                    return Ok(false);
                }
            }
            TagKind::ItemCondition(t) => {
                if !self.actors.item_condition(id, t, cfg) {
                    return Ok(false);
                }
            }
            TagKind::SocketedIn(t) => {
                if !self.socketed_in(t, cfg)? {
                    return Ok(false);
                }
            }
            TagKind::SkillName {
                names,
                include_transfigured,
                summon_skill,
                neg,
                ..
            } => {
                let name = cfg.and_then(|c| {
                    if *summon_skill {
                        c.summon_skill_name.clone()
                    } else {
                        c.skill_name.clone()
                    }
                });
                let mut matched = if *include_transfigured {
                    let want = name
                        .and_then(|n| self.actors.game_id(&n))
                        .unwrap_or_else(|| "".into());
                    match names {
                        Some(names) => names
                            .iter()
                            .any(|n| self.actors.game_id(n.as_str()).as_deref() == Some(&*want)),
                        None => false,
                    }
                } else {
                    let want = name.as_deref().unwrap_or("").to_ascii_lowercase();
                    match names {
                        Some(names) => names
                            .iter()
                            .any(|n| n.as_str().to_ascii_lowercase() == want),
                        None => false,
                    }
                };
                if *neg {
                    matched = !matched;
                }
                if !matched {
                    return Ok(false);
                }
            }
            TagKind::SkillId(want) => {
                let Some(Some(id)) = cfg.map(|c| &c.skill_id) else {
                    return Ok(false);
                };
                if id.as_deref() != want.as_deref() {
                    return Ok(false);
                }
            }
            TagKind::SkillPart { list, part, neg } => {
                let Some(cfg) = cfg else {
                    return Ok(false);
                };
                let mut matched = match list {
                    Some(list) => list.iter().any(|p| Some(*p) == cfg.skill_part),
                    None => *part == cfg.skill_part,
                };
                if *neg {
                    matched = !matched;
                }
                if !matched {
                    return Ok(false);
                }
            }
            TagKind::SkillType {
                list,
                skill_type,
                neg,
            } => {
                let has = |t: f64| {
                    cfg.and_then(|c| c.skill_types.as_ref())
                        .is_some_and(|s| t.fract() == 0.0 && s.contains(&(t as i64)))
                };
                let mut matched = match list {
                    Some(list) => list.iter().any(|t| has(*t)),
                    None => skill_type.is_some_and(has),
                };
                if *neg {
                    matched = !matched;
                }
                if !matched {
                    return Ok(false);
                }
            }
            TagKind::BaseFlag { flag, neg } => {
                let mut matched = cfg
                    .and_then(|c| c.base_flags.as_ref())
                    .is_some_and(|f| f.contains(flag));
                if *neg {
                    matched = !matched;
                }
                if !matched {
                    return Ok(false);
                }
            }
            TagKind::SlotName { names, neg } => {
                let Some(cfg) = cfg else {
                    return Ok(false);
                };
                let mut matched = match names {
                    Some(Names::List(list)) => list.iter().any(|s| Some(*s) == cfg.slot_name),
                    Some(Names::One(s)) => Some(*s) == cfg.slot_name,
                    None => cfg.slot_name.is_none(),
                };
                if *neg {
                    matched = !matched;
                }
                if !matched {
                    return Ok(false);
                }
            }
            TagKind::ModFlagOr(flags) => {
                let Some(f) = cfg.and_then(|c| c.flags) else {
                    return Ok(false);
                };
                if f & flags == 0 {
                    return Ok(false);
                }
            }
            TagKind::KeywordFlagAnd(flags) => {
                let Some(f) = cfg.and_then(|c| c.keyword_flags) else {
                    return Ok(false);
                };
                if f & flags != *flags {
                    return Ok(false);
                }
            }
            TagKind::MonsterTag { names, neg } => {
                let Some(tags) = self.actors.monster_tags(id) else {
                    return Ok(false);
                };
                let mut matched = false;
                for t in &tags {
                    let have = t.to_ascii_lowercase();
                    matched = match names {
                        Some(names) => names
                            .iter()
                            .any(|n| n.as_str().to_ascii_lowercase() == have),
                        None => false,
                    };
                    if matched {
                        break;
                    }
                }
                if *neg {
                    matched = !matched;
                }
                if !matched {
                    return Ok(false);
                }
            }
            TagKind::Other { .. } => {}
        }
        Ok(true)
    }

    /// The `SocketedIn` tag
    fn socketed_in(&self, t: &SocketedIn, cfg: Option<&Cfg>) -> Result<bool> {
        let Some(cfg) = cfg else {
            return Ok(false);
        };
        if t.slot_name.is_none() && t.keyword.is_none() && t.socket_color.is_none() {
            return Ok(false);
        }
        // Checked in upstream's order: slot, keyword or colour, sockets
        let mut checks = Vec::new();
        if let Some(slot) = t.slot_name {
            checks.push(Some(slot) == cfg.slot_name);
        }
        if let Some(k) = &t.keyword {
            checks.push(self.actors.gem_is_type(cfg, k));
        } else if let Some(c) = &t.socket_color
            && t.sockets != Some(Sockets::All)
        {
            checks.push(cfg.socket_color.as_deref() == Some(c));
        }
        if let Some(sockets) = &t.sockets {
            let count = match t.socket_color.as_deref() {
                Some("R") => cfg.strength_gems,
                Some("G") => cfg.dexterity_gems,
                Some("B") => cfg.intelligence_gems,
                _ => None,
            }
            .unwrap_or(0.0);
            checks.push(match sockets {
                Sockets::All => {
                    let total = cfg.intelligence_gems.unwrap_or(0.0)
                        + cfg.dexterity_gems.unwrap_or(0.0)
                        + cfg.strength_gems.unwrap_or(0.0);
                    total == count && total > 0.0
                }
                Sockets::Numbers(nums) => match cfg.socket_num {
                    Some(n) => nums.contains(&n),
                    None => return Ok(false),
                },
                Sockets::Fewer(n) => count < *n,
            });
        }
        Ok(checks.iter().all(|&v| v != t.neg))
    }
}

/// `value * mult + (tag.base or 0)`, capped by `limitTotal` and
/// `limitNegTotal`, on a number or a table value's `mod.value` or `value`;
/// rounded up for PercentStat.
fn scale(
    value: &mut ModValue,
    mult: f64,
    base: Option<f64>,
    limit_total: Option<f64>,
    limit_neg_total: Option<f64>,
    ceil: bool,
) -> Result<()> {
    let apply = |v: &mut ModValue| -> Result<()> {
        let mut r = num(v)? * mult + base.unwrap_or(0.0);
        if ceil {
            r = r.ceil();
        }
        if let Some(l) = limit_total {
            r = min(r, l);
        }
        if let Some(l) = limit_neg_total {
            r = max(r, l);
        }
        *v = ModValue::Number(r);
        Ok(())
    };
    match value {
        ModValue::Record(rec) => {
            let mod_key = Sym::new("mod");
            let value_key = Sym::new("value");
            if let Some(ModValue::Mod(inner)) = rec.get_mut(mod_key) {
                apply(&mut inner.value)
            } else {
                match rec.get_mut(value_key) {
                    Some(v) => apply(v),
                    None => Err("attempt to perform arithmetic on a nil value".into()),
                }
            }
        }
        v => apply(v),
    }
}
