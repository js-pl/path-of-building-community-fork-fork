//! Modifier queries of upstream's `ModStore`, `ModDB` and `ModList` classes
//! (`Classes/ModStore.lua`, `Classes/ModDB.lua`, `Classes/ModList.lua`), in
//! native Rust over the program's own tables. [`install`] replaces the
//! translated methods with these, so every calculation runs them.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.
//!
//! Stores are recognised by their class; anything else is called through
//! its Lua methods, as upstream would. `EvalMod` hands mods with the rare
//! tags it does not port (`ItemCondition`, `SocketedIn`, `SkillName` with
//! `includeTransfigured`) and `GetStat` the reservation stats to the
//! translated methods.

use lua::env::Env;
use lua::rt::{self, first, func};
use lua::{Key, LResult, LuaError, LuaStr, TableRef, Value};
use std::rc::Rc;

/// Keys looked up on every mod, built once.
struct Keys {
    name: Key,
    typ: Key,
    flags: Key,
    keyword_flags: Key,
    source: Key,
    value: Key,
    mods: Key,
    parent: Key,
}

struct Ctx {
    mod_db: TableRef,
    mod_list: TableRef,
    /// Translated methods, for what is not ported
    eval_mod: Value,
    get_stat: Value,
    /// `data.highPrecisionMods`
    high_precision: Value,
    k: Keys,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Db,
    List,
    Other,
}

fn s(x: &str) -> Value {
    Value::str(x)
}

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or_default()
}

fn get(v: &Value, key: &str) -> LResult<Value> {
    rt::index(v, &s(key))
}

/// `t[k]` without metatables, for mods, which are plain tables.
fn raw(t: &TableRef, k: &Key) -> Value {
    t.borrow().get_key(k)
}

fn table(v: &Value) -> LResult<&TableRef> {
    match v {
        Value::Table(t) => Ok(t),
        v => Err(LuaError::msg(format!(
            "attempt to index a {} value",
            v.type_name()
        ))),
    }
}

fn is(v: &Value, x: &str) -> bool {
    matches!(v, Value::Str(s) if s.as_bytes() == x.as_bytes())
}

fn num(v: &Value, what: &str) -> LResult<f64> {
    lua::to_number(v).ok_or_else(|| {
        LuaError::msg(format!(
            "bad argument #1 to '{what}' (number expected, got {})",
            v.type_name()
        ))
    })
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

/// `a or b`
fn or(a: Value, b: impl FnOnce() -> LResult<Value>) -> LResult<Value> {
    if a.truthy() { Ok(a) } else { b() }
}

/// LuaJIT's `bit.tobit`
fn tobit(v: f64) -> i32 {
    (v.rem_euclid(4294967296.0) as u64 as u32) as i32
}

fn band(a: &Value, b: &Value) -> LResult<f64> {
    Ok((tobit(num(a, "band")?) & tobit(num(b, "band")?)) as f64)
}

/// `MatchKeywordFlags` of `Data/Global.lua`, without its cache.
fn match_keyword_flags(keyword_flags: &Value, mod_keyword_flags: &Value) -> LResult<bool> {
    const MATCH_ALL: i32 = 0x40000000;
    let m = tobit(num(mod_keyword_flags, "band")?);
    let k = tobit(num(keyword_flags, "band")?) & !MATCH_ALL;
    let masked = m & !MATCH_ALL;
    Ok(if m & MATCH_ALL != 0 {
        k & masked == masked
    } else {
        masked == 0 || k & masked != 0
    })
}

/// `source:match("[^:]+")`
fn source_name(source: &Value) -> LResult<Option<&[u8]>> {
    let Value::Str(x) = source else {
        return Err(LuaError::msg(format!(
            "attempt to index a {} value",
            source.type_name()
        )));
    };
    let b = x.as_bytes();
    let Some(start) = b.iter().position(|&c| c != b':') else {
        return Ok(None);
    };
    let end = b[start..]
        .iter()
        .position(|&c| c == b':')
        .map_or(b.len(), |i| start + i);
    Ok(Some(&b[start..end]))
}

/// `round(val, dec)` of `Modules/Common.lua`
fn round(val: f64, dec: Option<f64>) -> f64 {
    match dec {
        Some(d) => (val * 10f64.powf(d) + 0.5).floor() / 10f64.powf(d),
        None => (val + 0.5).floor(),
    }
}

/// The mod filter every query shares: flags, keyword flags and source.
struct Filter {
    flags: Value,
    keyword_flags: Value,
    source: Value,
}

impl Filter {
    /// From `cfg`, as `ModStore:Sum` and the other queries read it.
    fn from_cfg(cfg: &Value) -> LResult<Filter> {
        if cfg.truthy() {
            Ok(Filter {
                flags: or(get(cfg, "flags")?, || Ok(n(0.0)))?,
                keyword_flags: or(get(cfg, "keywordFlags")?, || Ok(n(0.0)))?,
                source: get(cfg, "source")?,
            })
        } else {
            Ok(Filter {
                flags: n(0.0),
                keyword_flags: n(0.0),
                source: Value::Nil,
            })
        }
    }

    fn from_args(a: &[Value]) -> Filter {
        Filter {
            flags: arg(a, 0),
            keyword_flags: arg(a, 1),
            source: arg(a, 2),
        }
    }

    fn to_args(&self) -> [Value; 3] {
        [
            self.flags.clone(),
            self.keyword_flags.clone(),
            self.source.clone(),
        ]
    }

    /// `band(flags, mod.flags) == mod.flags and MatchKeywordFlags(...) and
    /// (not source or mod.source:match("[^:]+") == source)`; `lenient_source`
    /// is ModDB's `SumInternal`, which also checks `mod.source` is set.
    fn matches(&self, m: &TableRef, k: &Keys, lenient_source: bool) -> LResult<bool> {
        let mod_flags = raw(m, &k.flags);
        if !matches!(mod_flags, Value::Number(f) if band(&self.flags, &mod_flags)? == f) {
            return Ok(false);
        }
        if !match_keyword_flags(&self.keyword_flags, &raw(m, &k.keyword_flags))? {
            return Ok(false);
        }
        if !self.source.truthy() {
            return Ok(true);
        }
        let src = raw(m, &k.source);
        if lenient_source && !src.truthy() {
            return Ok(false);
        }
        let want = match &self.source {
            Value::Str(x) => x.as_bytes(),
            _ => return Ok(false),
        };
        Ok(source_name(&src)?.is_some_and(|got| got == want))
    }
}

/// Mod lists a query visits: for a ModDB the bucket of each name, for a
/// ModList the whole list once per name, keeping the mods of that name.
fn for_each_mod(
    c: &Ctx,
    store: &TableRef,
    kind: Kind,
    names: &[Value],
    mut f: impl FnMut(usize, &TableRef) -> LResult<bool>,
) -> LResult<bool> {
    match kind {
        Kind::Db => {
            let mods = raw(store, &c.k.mods);
            let mods = table(&mods)?;
            for (i, name) in names.iter().enumerate() {
                let list = mods.get(name);
                if let Value::Table(list) = list {
                    let n = list.borrow().border();
                    for j in 1..=n as i64 {
                        let m = list.get_int(j);
                        if !f(i, table(&m)?)? {
                            return Ok(false);
                        }
                    }
                }
                if !f(i, &EMPTY.with(|e| e.clone()))? {
                    return Ok(false);
                }
            }
        }
        Kind::List => {
            for (i, name) in names.iter().enumerate() {
                let n = store.borrow().border();
                for j in 1..=n as i64 {
                    let m = store.get_int(j);
                    let m = table(&m)?;
                    if raw(m, &c.k.name).raw_eq(name) && !f(i, m)? {
                        return Ok(false);
                    }
                }
                if !f(i, &EMPTY.with(|e| e.clone()))? {
                    return Ok(false);
                }
            }
        }
        Kind::Other => unreachable!(),
    }
    Ok(true)
}

thread_local! {
    /// Marks the end of one name's mods in [`for_each_mod`]
    static EMPTY: TableRef = TableRef::new();
}

fn is_end(m: &TableRef) -> bool {
    EMPTY.with(|e| e.ptr_eq(m))
}

impl Ctx {
    fn kind(&self, v: &Value) -> Kind {
        if let Value::Table(t) = v
            && let Some(meta) = &t.borrow().meta
        {
            if meta.ptr_eq(&self.mod_db) {
                return Kind::Db;
            }
            if meta.ptr_eq(&self.mod_list) {
                return Kind::List;
            }
        }
        Kind::Other
    }

    /// `self.parent`, if it is set
    fn parent(&self, store: &TableRef) -> Option<Value> {
        let p = raw(store, &self.k.parent);
        p.truthy().then_some(p)
    }

    /// `context:EvalMod(mod, cfg, globalLimits)`
    fn eval(
        &self,
        context: &Value,
        m: &TableRef,
        cfg: &Value,
        limits: Option<&TableRef>,
    ) -> LResult<Value> {
        let lim = limits.map_or(Value::Nil, |t| Value::Table(t.clone()));
        if self.kind(context) == Kind::Other {
            return Ok(first(rt::method(
                context,
                &LuaStr::from("EvalMod"),
                &[Value::Table(m.clone()), cfg.clone(), lim],
            )?));
        }
        self.eval_mod(context, m, cfg, limits)
    }

    /// `ModDB:SumInternal` and `ModList:SumInternal`
    fn sum_internal(
        &self,
        store: &Value,
        context: &Value,
        mod_type: &Value,
        cfg: &Value,
        flt: &Filter,
        names: &[Value],
    ) -> LResult<Value> {
        let kind = self.kind(store);
        if kind == Kind::Other {
            let mut a = vec![context.clone(), mod_type.clone(), cfg.clone()];
            a.extend(flt.to_args());
            a.extend_from_slice(names);
            return Ok(first(rt::method(store, &LuaStr::from("SumInternal"), &a)?));
        }
        let t = table(store)?;
        let k = &self.k;
        let mut result = n(0.0);
        // ModDB shares one globalLimits across the names, ModList passes none
        let limits = (kind == Kind::Db).then(TableRef::new);
        for_each_mod(self, t, kind, names, |_, m| {
            if is_end(m) {
                return Ok(true);
            }
            if raw(m, &k.typ).raw_eq(mod_type) && flt.matches(m, k, kind == Kind::Db)? {
                if m.get_int(1).truthy() {
                    let v = or(self.eval(context, m, cfg, limits.as_ref())?, || Ok(n(0.0)))?;
                    result = rt::add(&result, &v)?;
                } else {
                    result = rt::add(&result, &raw(m, &k.value))?;
                }
            }
            Ok(true)
        })?;
        if let Some(p) = self.parent(t) {
            let r = self.sum_internal(&p, context, mod_type, cfg, flt, names)?;
            result = rt::add(&result, &r)?;
        }
        Ok(result)
    }

    /// `ModDB:MoreInternal` and `ModList:MoreInternal`
    fn more_internal(
        &self,
        store: &Value,
        context: &Value,
        cfg: &Value,
        flt: &Filter,
        names: &[Value],
    ) -> LResult<Value> {
        let kind = self.kind(store);
        if kind == Kind::Other {
            let mut a = vec![context.clone(), cfg.clone()];
            a.extend(flt.to_args());
            a.extend_from_slice(names);
            return Ok(first(rt::method(store, &LuaStr::from("MoreInternal"), &a)?));
        }
        let t = table(store)?;
        let k = &self.k;
        let more = s("MORE");
        let mut result = 1.0;
        let mut precision: Option<f64> = None;
        let mut mod_result = 1.0;
        let limits = (kind == Kind::Db).then(TableRef::new);
        for_each_mod(self, t, kind, names, |_, m| {
            if is_end(m) {
                if let Some(p) = precision {
                    let power = 10f64.powf(p);
                    result = (result * mod_result * power).floor() / power;
                } else {
                    result *= round(mod_result, Some(2.0));
                }
                mod_result = 1.0;
                return Ok(true);
            }
            if raw(m, &k.typ).raw_eq(&more) && flt.matches(m, k, false)? {
                let value = if m.get_int(1).truthy() {
                    or(self.eval(context, m, cfg, limits.as_ref())?, || Ok(n(0.0)))?
                } else if kind == Kind::Db {
                    or(raw(m, &k.value), || Ok(n(0.0)))?
                } else {
                    raw(m, &k.value)
                };
                mod_result *= 1.0 + num(&value, "div")? / 100.0;
                let hp = self.high_precision(&raw(m, &k.name), &raw(m, &k.typ))?;
                precision = match precision {
                    Some(p) => Some(match hp {
                        Some(h) => p.max(h),
                        None => p,
                    }),
                    None => hp,
                };
            }
            Ok(true)
        })?;
        let mut result = n(result);
        if let Some(p) = self.parent(t) {
            let r = self.more_internal(&p, context, cfg, flt, names)?;
            result = rt::mul(&result, &r)?;
        }
        Ok(result)
    }

    /// `data.highPrecisionMods[name] and data.highPrecisionMods[name][type]`
    fn high_precision(&self, name: &Value, typ: &Value) -> LResult<Option<f64>> {
        let by_name = rt::index(&self.high_precision, name)?;
        if !by_name.truthy() {
            return Ok(None);
        }
        let p = rt::index(&by_name, typ)?;
        Ok(if p.truthy() {
            Some(num(&p, "max")?)
        } else {
            None
        })
    }

    /// The first matching mod of `mod_type` for which `take` gives a value:
    /// `FlagInternal` and `OverrideInternal`.
    fn first_internal(
        &self,
        method: &str,
        mod_type: &str,
        store: &Value,
        context: &Value,
        cfg: &Value,
        flt: &Filter,
        names: &[Value],
    ) -> LResult<Value> {
        let kind = self.kind(store);
        if kind == Kind::Other {
            let mut a = vec![context.clone(), cfg.clone()];
            a.extend(flt.to_args());
            a.extend_from_slice(names);
            return Ok(first(rt::method(store, &LuaStr::from(method), &a)?));
        }
        let t = table(store)?;
        let k = &self.k;
        let want = s(mod_type);
        let flag = mod_type == "FLAG";
        let mut found = Value::Nil;
        for_each_mod(self, t, kind, names, |_, m| {
            if is_end(m) {
                return Ok(true);
            }
            if raw(m, &k.typ).raw_eq(&want) && flt.matches(m, k, false)? {
                if m.get_int(1).truthy() {
                    let v = self.eval(context, m, cfg, None)?;
                    if v.truthy() {
                        found = if flag { Value::Bool(true) } else { v };
                        return Ok(false);
                    }
                } else {
                    let v = raw(m, &k.value);
                    if v.truthy() {
                        found = if flag { Value::Bool(true) } else { v };
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        })?;
        if found.truthy() {
            return Ok(found);
        }
        match self.parent(t) {
            Some(p) => self.first_internal(method, mod_type, &p, context, cfg, flt, names),
            None => Ok(Value::Nil),
        }
    }

    /// `ModDB:ListInternal` and `ModList:ListInternal`
    fn list_internal(
        &self,
        store: &Value,
        context: &Value,
        result: &TableRef,
        cfg: &Value,
        flt: &Filter,
        names: &[Value],
    ) -> LResult<()> {
        let kind = self.kind(store);
        if kind == Kind::Other {
            let mut a = vec![context.clone(), Value::Table(result.clone()), cfg.clone()];
            a.extend(flt.to_args());
            a.extend_from_slice(names);
            rt::method(store, &LuaStr::from("ListInternal"), &a)?;
            return Ok(());
        }
        let t = table(store)?;
        let k = &self.k;
        let list = s("LIST");
        for_each_mod(self, t, kind, names, |_, m| {
            if is_end(m) {
                return Ok(true);
            }
            if raw(m, &k.typ).raw_eq(&list) && flt.matches(m, k, false)? {
                // `EvalMod(...) or nullValue`, and nullValue is never set
                let v = if m.get_int(1).truthy() {
                    self.eval(context, m, cfg, None)?
                } else {
                    raw(m, &k.value)
                };
                if v.truthy() {
                    result.push(v);
                }
            }
            Ok(true)
        })?;
        if let Some(p) = self.parent(t) {
            self.list_internal(&p, context, result, cfg, flt, names)?;
        }
        Ok(())
    }

    /// `ModDB:TabulateInternal` and `ModList:TabulateInternal`
    #[allow(clippy::too_many_arguments)]
    fn tabulate_internal(
        &self,
        store: &Value,
        context: &Value,
        result: &TableRef,
        mod_type: &Value,
        cfg: &Value,
        flt: &Filter,
        names: &[Value],
    ) -> LResult<()> {
        let kind = self.kind(store);
        if kind == Kind::Other {
            let mut a = vec![
                context.clone(),
                Value::Table(result.clone()),
                mod_type.clone(),
                cfg.clone(),
            ];
            a.extend(flt.to_args());
            a.extend_from_slice(names);
            rt::method(store, &LuaStr::from("TabulateInternal"), &a)?;
            return Ok(());
        }
        let t = table(store)?;
        let k = &self.k;
        let over = s("OVERRIDE");
        let limits = (kind == Kind::Db).then(TableRef::new);
        for_each_mod(self, t, kind, names, |_, m| {
            if is_end(m) {
                return Ok(true);
            }
            let typ = raw(m, &k.typ);
            if (typ.raw_eq(mod_type) || !mod_type.truthy()) && flt.matches(m, k, false)? {
                let v = if m.get_int(1).truthy() {
                    self.eval(context, m, cfg, limits.as_ref())?
                } else {
                    raw(m, &k.value)
                };
                if v.truthy() && (!v.raw_eq(&n(0.0)) || typ.raw_eq(&over)) {
                    let row = TableRef::new();
                    row.set_str("value", v);
                    row.set_str("mod", Value::Table(m.clone()));
                    result.push(Value::Table(row));
                }
            }
            Ok(true)
        })?;
        if let Some(p) = self.parent(t) {
            self.tabulate_internal(&p, context, result, mod_type, cfg, flt, names)?;
        }
        Ok(())
    }

    /// `ModDB:HasModInternal`; ModList has none and inherits nothing, so it
    /// errors there as upstream does.
    fn has_mod_internal(
        &self,
        store: &Value,
        mod_type: &Value,
        flt: &Filter,
        names: &[Value],
    ) -> LResult<bool> {
        if self.kind(store) != Kind::Db {
            let mut a = vec![mod_type.clone()];
            a.extend(flt.to_args());
            a.extend_from_slice(names);
            let r = first(rt::method(store, &LuaStr::from("HasModInternal"), &a)?);
            return Ok(r.raw_eq(&Value::Bool(true)));
        }
        let t = table(store)?;
        let k = &self.k;
        let mut found = false;
        for_each_mod(self, t, Kind::Db, names, |_, m| {
            if is_end(m) {
                return Ok(true);
            }
            if raw(m, &k.typ).raw_eq(mod_type) && flt.matches(m, k, false)? {
                found = true;
                return Ok(false);
            }
            Ok(true)
        })?;
        if found {
            return Ok(true);
        }
        match self.parent(t) {
            Some(p) => self.has_mod_internal(&p, mod_type, flt, names),
            None => Ok(false),
        }
    }

    // ModStore's queries, on any store

    fn sum(&self, store: &Value, mod_type: &Value, cfg: &Value, names: &[Value]) -> LResult<Value> {
        let flt = Filter::from_cfg(cfg)?;
        self.sum_internal(store, store, mod_type, cfg, &flt, names)
    }

    fn more(&self, store: &Value, cfg: &Value, names: &[Value]) -> LResult<Value> {
        let flt = Filter::from_cfg(cfg)?;
        self.more_internal(store, store, cfg, &flt, names)
    }

    fn flag(&self, store: &Value, cfg: &Value, names: &[Value]) -> LResult<Value> {
        let flt = Filter::from_cfg(cfg)?;
        self.first_internal("FlagInternal", "FLAG", store, store, cfg, &flt, names)
    }

    fn override_(&self, store: &Value, cfg: &Value, names: &[Value]) -> LResult<Value> {
        let flt = Filter::from_cfg(cfg)?;
        self.first_internal(
            "OverrideInternal",
            "OVERRIDE",
            store,
            store,
            cfg,
            &flt,
            names,
        )
    }

    fn list(&self, store: &Value, cfg: &Value, names: &[Value]) -> LResult<Value> {
        let flt = Filter::from_cfg(cfg)?;
        let result = TableRef::new();
        self.list_internal(store, store, &result, cfg, &flt, names)?;
        Ok(Value::Table(result))
    }

    fn tabulate(
        &self,
        store: &Value,
        mod_type: &Value,
        cfg: &Value,
        names: &[Value],
    ) -> LResult<TableRef> {
        let flt = Filter::from_cfg(cfg)?;
        let result = TableRef::new();
        self.tabulate_internal(store, store, &result, mod_type, cfg, &flt, names)?;
        Ok(result)
    }

    /// `ModStore:Max` and `ModStore:Min`
    fn extreme(&self, store: &Value, cfg: &Value, names: &[Value], max: bool) -> LResult<Value> {
        let rows = self.tabulate(store, &s(if max { "MAX" } else { "MIN" }), cfg, names)?;
        let mut best = Value::Nil;
        for row in rows.array() {
            let m = get(&row, "mod")?;
            let v = self.eval(store, table(&m)?, cfg, None)?;
            let better = if max {
                let cur = if best.is_nil() { n(0.0) } else { best.clone() };
                rt::lt(&cur, &v)?
            } else {
                best.is_nil() || rt::lt(&v, &best)?
            };
            if better {
                best = v;
            }
        }
        Ok(best)
    }

    /// `ModStore:Combine`
    fn combine(
        &self,
        store: &Value,
        mod_type: &Value,
        cfg: &Value,
        names: &[Value],
    ) -> LResult<Value> {
        match mod_type {
            t if is(t, "MORE") => self.more(store, cfg, names),
            t if is(t, "FLAG") => self.flag(store, cfg, names),
            t if is(t, "OVERRIDE") => self.override_(store, cfg, names),
            t if is(t, "LIST") => self.list(store, cfg, names),
            t if is(t, "MAX") => self.extreme(store, cfg, names, true),
            _ => self.sum(store, mod_type, cfg, names),
        }
    }

    fn call_store(&self, store: &Value, method: &str, args: &[Value]) -> LResult<Value> {
        Ok(first(rt::method(store, &LuaStr::from(method), args)?))
    }

    /// `ModStore:GetCondition(var, cfg, noMod)`
    fn get_condition(
        &self,
        store: &Value,
        var: &Value,
        cfg: &Value,
        no_mod: bool,
    ) -> LResult<Value> {
        if self.kind(store) == Kind::Other {
            return self.call_store(
                store,
                "GetCondition",
                &[var.clone(), cfg.clone(), Value::Bool(no_mod)],
            );
        }
        let v = rt::index(&get(store, "conditions")?, var)?;
        if v.truthy() {
            return Ok(v);
        }
        let parent = get(store, "parent")?;
        if parent.truthy() {
            let v = self.get_condition(&parent, var, cfg, true)?;
            if v.truthy() {
                return Ok(v);
            }
        }
        if no_mod {
            return Ok(Value::Bool(false));
        }
        let name = rt::concat(&s("Condition:"), var)?;
        self.flag(store, cfg, &[name])
    }

    /// `ModStore:GetMultiplier(var, cfg, noMod)`
    fn get_multiplier(
        &self,
        store: &Value,
        var: &Value,
        cfg: &Value,
        no_mod: bool,
    ) -> LResult<Value> {
        if self.kind(store) == Kind::Other {
            return self.call_store(
                store,
                "GetMultiplier",
                &[var.clone(), cfg.clone(), Value::Bool(no_mod)],
            );
        }
        let name = if no_mod {
            Value::Nil
        } else {
            rt::concat(&s("Multiplier:"), var)?
        };
        if !no_mod {
            let o = self.override_(store, cfg, std::slice::from_ref(&name))?;
            if o.truthy() {
                return Ok(o);
            }
        }
        let own = or(rt::index(&get(store, "multipliers")?, var)?, || Ok(n(0.0)))?;
        let parent = get(store, "parent")?;
        let inherited = if parent.truthy() {
            or(self.get_multiplier(&parent, var, cfg, true)?, || Ok(n(0.0)))?
        } else {
            n(0.0)
        };
        let mut r = rt::add(&own, &inherited)?;
        let summed = if no_mod {
            n(0.0)
        } else {
            or(self.sum(store, &s("BASE"), cfg, &[name])?, || Ok(n(0.0)))?
        };
        r = rt::add(&r, &summed)?;
        Ok(r)
    }

    /// `ModStore:GetStat(stat, cfg)`
    fn get_stat(&self, store: &Value, stat: &Value, cfg: &Value) -> LResult<Value> {
        if self.kind(store) == Kind::Other {
            return self.call_store(store, "GetStat", &[stat.clone(), cfg.clone()]);
        }
        if is(stat, "ManaReservedPercent")
            || is(stat, "LifeReservedPercent")
            || is(stat, "ManaUnreserved")
        {
            return Ok(first(rt::call(
                &self.get_stat,
                &[store.clone(), stat.clone(), cfg.clone()],
            )?));
        }
        let output = get(&get(store, "actor")?, "output")?;
        if output.truthy() {
            let v = rt::index(&output, stat)?;
            if v.truthy() {
                return Ok(v);
            }
        }
        if cfg.truthy() {
            let stats = get(cfg, "skillStats")?;
            if stats.truthy() {
                let v = rt::index(&stats, stat)?;
                if v.truthy() {
                    return Ok(v);
                }
            }
        }
        Ok(n(0.0))
    }

    /// `getActor(self, actorType)` of `ModStore.lua`
    fn get_actor(&self, store: &Value, actor_type: &Value) -> LResult<Value> {
        let actor = get(store, "actor")?;
        if is(actor_type, "player") {
            or(get(&actor, "player")?, || {
                let parent = get(&actor, "parent")?;
                let p = if parent.truthy() {
                    get(&parent, "player")?
                } else {
                    parent
                };
                or(p, || {
                    let enemy = get(&actor, "enemy")?;
                    if enemy.truthy() {
                        get(&enemy, "player")
                    } else {
                        Ok(enemy)
                    }
                })
            })
        } else {
            rt::index(&actor, actor_type)
        }
    }

    /// The modDB of the actor a tag names, `None` when that actor is absent
    fn actor_db(&self, store: &Value, actor_type: &Value) -> LResult<Option<Value>> {
        let a = self.get_actor(store, actor_type)?;
        if a.truthy() {
            Ok(Some(get(&a, "modDB")?))
        } else {
            Ok(None)
        }
    }

    /// Sum of `GetMultiplier` over `tag.varList` (in `pairs` order), or of
    /// `tag.var`
    fn multiplier_of(&self, target: &Value, tag: &Value, cfg: &Value) -> LResult<Value> {
        let list = get(tag, "varList")?;
        if list.truthy() {
            let mut base = n(0.0);
            for (_, var) in table(&list)?.entries() {
                base = rt::add(&base, &self.get_multiplier(target, &var, cfg, false)?)?;
            }
            Ok(base)
        } else {
            self.get_multiplier(target, &get(tag, "var")?, cfg, false)
        }
    }

    /// Sum of `GetStat` over `tag.statList` (in `ipairs` order), or `tag.stat`
    fn stat_of(&self, target: &Value, tag: &Value, cfg: &Value) -> LResult<Value> {
        let list = get(tag, "statList")?;
        if list.truthy() {
            let mut base = n(0.0);
            for stat in ipairs(&list)? {
                base = rt::add(&base, &self.get_stat(target, &stat, cfg)?)?;
            }
            Ok(base)
        } else {
            self.get_stat(target, &get(tag, "stat")?, cfg)
        }
    }

    /// Whether `EvalMod` hands this mod to the translated method
    fn needs_translated(&self, m: &TableRef) -> LResult<bool> {
        for tag in ipairs(&Value::Table(m.clone()))? {
            let t = get(&tag, "type")?;
            if is(&t, "ItemCondition")
                || is(&t, "SocketedIn")
                || (is(&t, "SkillName") && get(&tag, "includeTransfigured")?.truthy())
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `ModStore:EvalMod(mod, cfg, globalLimits)`; `None` for a bare
    /// `return`, which callers see as nil.
    fn eval_mod(
        &self,
        store: &Value,
        m: &TableRef,
        cfg: &Value,
        limits: Option<&TableRef>,
    ) -> LResult<Value> {
        if self.needs_translated(m)? {
            let lim = limits.map_or(Value::Nil, |t| Value::Table(t.clone()));
            return Ok(first(rt::call(
                &self.eval_mod,
                &[store.clone(), Value::Table(m.clone()), cfg.clone(), lim],
            )?));
        }
        match self.eval_tags(store, m, cfg)? {
            None => Ok(Value::Nil),
            Some(value) => self.global_limits(m, value, limits),
        }
    }

    fn global_limits(
        &self,
        m: &TableRef,
        mut value: Value,
        limits: Option<&TableRef>,
    ) -> LResult<Value> {
        let Some(limits) = limits else {
            return Ok(value);
        };
        for tag in ipairs(&Value::Table(m.clone()))? {
            let limit = get(&tag, "globalLimit")?;
            let key = get(&tag, "globalLimitKey")?;
            if limit.truthy() && key.truthy() {
                value = or(value, || Ok(n(0.0)))?;
                let cur = or(limits.get(&key), || Ok(n(0.0)))?;
                limits.set(key.clone(), cur.clone());
                if rt::lt(&limit, &rt::add(&cur, &value)?)? {
                    value = rt::sub(&limit, &cur)?;
                }
                limits.set(key.clone(), rt::add(&limits.get(&key), &value)?);
            }
        }
        Ok(value)
    }

    /// Applies a multiplier to the mod's value: a number, or a table value's
    /// `mod.value` or `value`; `ceil` for PercentStat.
    fn scale(
        value: Value,
        mult: &Value,
        tag: &Value,
        limit_total: &Value,
        limit_neg_total: &Value,
        ceil: bool,
    ) -> LResult<Value> {
        let base = or(get(tag, "base")?, || Ok(n(0.0)))?;
        let apply = |v: &Value| -> LResult<Value> {
            let mut r = rt::add(&rt::mul(v, mult)?, &base)?;
            if ceil {
                r = n(num(&r, "ceil")?.ceil());
            }
            if limit_total.truthy() {
                r = n(num(&r, "min")?.min_lua(num(limit_total, "min")?));
            }
            if limit_neg_total.truthy() {
                r = n(num(&r, "max")?.max_lua(num(limit_neg_total, "max")?));
            }
            Ok(r)
        };
        if let Value::Table(t) = &value {
            let copy = Value::Table(t.deep_copy());
            let inner = get(&copy, "mod")?;
            let holder = if inner.truthy() { inner } else { copy.clone() };
            let v = get(&holder, "value")?;
            rt::set_index(&holder, s("value"), apply(&v)?)?;
            Ok(copy)
        } else {
            apply(&value)
        }
    }

    /// The tag loop of `EvalMod`: the mod's value, or `None` when a tag
    /// does not match.
    fn eval_tags(&self, store: &Value, m: &TableRef, cfg: &Value) -> LResult<Option<Value>> {
        let mut value = raw(m, &self.k.value);
        for tag in ipairs(&Value::Table(m.clone()))? {
            let typ = get(&tag, "type")?;
            let Value::Str(typ) = typ else { continue };
            match typ.as_bytes() {
                b"Multiplier" => {
                    let mut target = store.clone();
                    let mut limit_target = store.clone();
                    let limit_actor = get(&tag, "limitActor")?;
                    if limit_actor.truthy() {
                        match self.actor_db(store, &limit_actor)? {
                            Some(db) => limit_target = db,
                            None => return Ok(None),
                        }
                    }
                    let actor = get(&tag, "actor")?;
                    if actor.truthy() {
                        match self.actor_db(store, &actor)? {
                            Some(db) => target = db,
                            None => return Ok(None),
                        }
                    }
                    let base = self.multiplier_of(&target, &tag, cfg)?;
                    let div_var = get(&tag, "divVar")?;
                    if div_var.truthy() {
                        let d = self.get_multiplier(store, &div_var, cfg, false)?;
                        rt::set_index(&tag, s("div"), d)?;
                    }
                    let div = or(get(&tag, "div")?, || Ok(n(1.0)))?;
                    let q = rt::div(&base, &div)?;
                    let mut mult = if get(&tag, "noFloor")?.truthy() {
                        q
                    } else {
                        n((num(&q, "floor")? + 0.0001).floor())
                    };
                    let (mut limit_total, mut limit_neg_total) = (Value::Nil, Value::Nil);
                    let limit = get(&tag, "limit")?;
                    let limit_var = get(&tag, "limitVar")?;
                    let limit_stat = get(&tag, "limitStat")?;
                    if limit.truthy() || limit_var.truthy() || limit_stat.truthy() {
                        let limit = or(limit, || {
                            let v = if limit_var.truthy() {
                                self.get_multiplier(&limit_target, &limit_var, cfg, false)?
                            } else {
                                limit_var.clone()
                            };
                            or(v, || {
                                if limit_stat.truthy() {
                                    self.get_stat(&limit_target, &limit_stat, cfg)
                                } else {
                                    Ok(limit_stat.clone())
                                }
                            })
                        })?;
                        if get(&tag, "limitTotal")?.truthy() {
                            limit_total = limit;
                        } else if get(&tag, "limitNegTotal")?.truthy() {
                            limit_neg_total = limit;
                        } else {
                            mult = n(num(&mult, "min")?.min_lua(num(&limit, "min")?));
                        }
                    }
                    if get(&tag, "invert")?.truthy() && !mult.raw_eq(&n(0.0)) {
                        mult = rt::div(&n(1.0), &mult)?;
                    }
                    value = Self::scale(value, &mult, &tag, &limit_total, &limit_neg_total, false)?;
                }
                b"MultiplierThreshold" => {
                    let mut target = store.clone();
                    let mut threshold_target = store.clone();
                    let threshold_actor = get(&tag, "thresholdActor")?;
                    if threshold_actor.truthy() {
                        match self.actor_db(store, &threshold_actor)? {
                            Some(db) => threshold_target = db,
                            None => return Ok(None),
                        }
                    }
                    let actor = get(&tag, "actor")?;
                    if actor.truthy() {
                        match self.actor_db(store, &actor)? {
                            Some(db) => target = db,
                            None => return Ok(None),
                        }
                    }
                    let mult = self.multiplier_of(&target, &tag, cfg)?;
                    let threshold = or(get(&tag, "threshold")?, || {
                        let t = if threshold_actor.truthy() {
                            &threshold_target
                        } else {
                            &target
                        };
                        self.get_multiplier(t, &get(&tag, "thresholdVar")?, cfg, false)
                    })?;
                    let upper = get(&tag, "upper")?.truthy();
                    if (upper && rt::lt(&threshold, &mult)?)
                        || (get(&tag, "equals")?.truthy() && !mult.raw_eq(&threshold))
                        || (!upper && rt::lt(&mult, &threshold)?)
                    {
                        return Ok(None);
                    }
                }
                b"PerStat" | b"PercentStat" => {
                    let percent_stat = typ.as_bytes() == b"PercentStat";
                    let target = self
                        .actor_db(store, &get(&tag, "actor")?)?
                        .unwrap_or_else(|| store.clone());
                    let base = self.stat_of(&target, &tag, cfg)?;
                    let mut mult = if percent_stat {
                        let percent = or(get(&tag, "percent")?, || {
                            self.get_multiplier(store, &get(&tag, "percentVar")?, cfg, false)
                        })?;
                        let f = if percent.truthy() {
                            rt::div(&percent, &n(100.0))?
                        } else {
                            n(1.0)
                        };
                        let mut mult = rt::mul(&base, &f)?;
                        if get(&tag, "floor")?.truthy() {
                            mult = n(num(&mult, "floor")?.floor());
                        }
                        mult
                    } else {
                        let div_var = get(&tag, "divVar")?;
                        if div_var.truthy() {
                            let d = self.get_multiplier(store, &div_var, cfg, false)?;
                            rt::set_index(&tag, s("div"), d)?;
                        }
                        let div = or(get(&tag, "div")?, || Ok(n(1.0)))?;
                        n((num(&rt::div(&base, &div)?, "floor")? + 0.0001).floor())
                    };
                    let mut limit_total = Value::Nil;
                    let limit = get(&tag, "limit")?;
                    let limit_var = get(&tag, "limitVar")?;
                    if limit.truthy() || limit_var.truthy() {
                        let limit =
                            or(limit, || self.get_multiplier(store, &limit_var, cfg, false))?;
                        if get(&tag, "limitTotal")?.truthy() {
                            limit_total = limit;
                        } else {
                            mult = n(num(&mult, "min")?.min_lua(num(&limit, "min")?));
                        }
                    }
                    value =
                        Self::scale(value, &mult, &tag, &limit_total, &Value::Nil, percent_stat)?;
                }
                b"StatThreshold" => {
                    let list = get(&tag, "statList")?;
                    let stat = if list.truthy() {
                        // Upstream's loop variable shadows the sum, which stays 0
                        for st in ipairs(&list)? {
                            rt::add(&st, &self.get_stat(store, &st, cfg)?)?;
                        }
                        n(0.0)
                    } else {
                        self.get_stat(store, &get(&tag, "stat")?, cfg)?
                    };
                    let mut threshold = or(get(&tag, "threshold")?, || {
                        self.get_stat(store, &get(&tag, "thresholdStat")?, cfg)
                    })?;
                    let tp = get(&tag, "thresholdPercent")?;
                    let tpv = get(&tag, "thresholdPercentVar")?;
                    if tp.truthy() || tpv.truthy() {
                        let p = or(tp, || self.get_multiplier(store, &tpv, cfg, false))?;
                        let f = if p.truthy() {
                            rt::div(&p, &n(100.0))?
                        } else {
                            n(1.0)
                        };
                        threshold = rt::mul(&threshold, &f)?;
                    }
                    let upper = get(&tag, "upper")?.truthy();
                    if (upper && rt::lt(&threshold, &stat)?)
                        || (!upper && rt::lt(&stat, &threshold)?)
                    {
                        return Ok(None);
                    }
                }
                b"DistanceRamp" => {
                    let dist = if cfg.truthy() {
                        get(cfg, "skillDist")?
                    } else {
                        Value::Nil
                    };
                    if !dist.truthy() {
                        return Ok(None);
                    }
                    let ramp = get(&tag, "ramp")?;
                    let points = ipairs(&ramp)?;
                    let at = |p: &Value, i: i64| rt::index(p, &n(i as f64));
                    let first_p = rt::index(&ramp, &n(1.0))?;
                    let last_p = rt::index(&ramp, &rt::len(&ramp)?)?;
                    if rt::le(&dist, &at(&first_p, 1)?)? {
                        value = rt::mul(&value, &at(&first_p, 2)?)?;
                    } else if rt::le(&at(&last_p, 1)?, &dist)? {
                        value = rt::mul(&value, &at(&last_p, 2)?)?;
                    } else {
                        for (i, dat) in points.iter().enumerate() {
                            let next = rt::index(&ramp, &n(i as f64 + 2.0))?;
                            if rt::le(&dist, &at(&next, 1)?)? {
                                let (d1, d2) = (at(dat, 1)?, at(dat, 2)?);
                                let (n1, n2) = (at(&next, 1)?, at(&next, 2)?);
                                let f = rt::add(
                                    &d2,
                                    &rt::div(
                                        &rt::mul(&rt::sub(&n2, &d2)?, &rt::sub(&dist, &d1)?)?,
                                        &rt::sub(&n1, &d1)?,
                                    )?,
                                )?;
                                value = rt::mul(&value, &f)?;
                                break;
                            }
                        }
                    }
                }
                b"MeleeProximity" => {
                    let dist = if cfg.truthy() {
                        get(cfg, "skillDist")?
                    } else {
                        Value::Nil
                    };
                    if !dist.truthy() {
                        return Ok(None);
                    }
                    let r1 = rt::index(&get(&tag, "ramp")?, &n(1.0))?;
                    if rt::le(&dist, &n(15.0))? {
                        value = rt::mul(&value, &r1)?;
                    } else if rt::le(&n(16.0), &dist)? && rt::le(&dist, &n(39.0))? {
                        let f = rt::sub(
                            &r1,
                            &rt::mul(&rt::div(&r1, &n(25.0))?, &rt::sub(&dist, &n(15.0))?)?,
                        )?;
                        value = rt::mul(&value, &f)?;
                    } else if rt::le(&n(40.0), &dist)? {
                        value = n(0.0);
                    }
                }
                b"Limit" => {
                    let limit = or(get(&tag, "limit")?, || {
                        self.get_multiplier(store, &get(&tag, "limitVar")?, cfg, false)
                    })?;
                    value = n(num(&value, "min")?.min_lua(num(&limit, "min")?));
                }
                b"Condition" => {
                    if !self.condition_tag(store, &tag, cfg)? {
                        return Ok(None);
                    }
                }
                b"ActorCondition" => {
                    let mut matched = Value::Bool(false);
                    let mut target = store.clone();
                    let actor = get(&tag, "actor")?;
                    if actor.truthy() {
                        let a = self.get_actor(store, &actor)?;
                        target = if a.truthy() { get(&a, "modDB")? } else { a };
                    }
                    let var = get(&tag, "var")?;
                    let list = get(&tag, "varList")?;
                    if target.truthy() && (var.truthy() || list.truthy()) {
                        if list.truthy() {
                            for (_, v) in table(&list)?.entries() {
                                if self.get_condition(&target, &v, cfg, false)?.truthy() {
                                    matched = Value::Bool(true);
                                    break;
                                }
                            }
                        } else {
                            matched = self.get_condition(&target, &var, cfg, false)?;
                        }
                    } else if actor.truthy() && cfg.truthy() && actor.raw_eq(&get(cfg, "actor")?) {
                        matched = Value::Bool(true);
                    }
                    if get(&tag, "neg")?.truthy() {
                        matched = Value::Bool(!matched.truthy());
                    }
                    if !matched.truthy() {
                        return Ok(None);
                    }
                }
                b"SkillName" => {
                    let summon = get(&tag, "summonSkill")?.truthy();
                    let field = if summon {
                        "summonSkillName"
                    } else {
                        "skillName"
                    };
                    let name = if cfg.truthy() {
                        or(get(cfg, field)?, || Ok(s("")))?
                    } else {
                        s("")
                    };
                    let want = lower(&name)?;
                    let list = get(&tag, "skillNameList")?;
                    let mut matched = if list.truthy() {
                        let mut found = false;
                        for (_, nm) in table(&list)?.entries() {
                            if lower(&nm)? == want {
                                found = true;
                                break;
                            }
                        }
                        found
                    } else {
                        let sn = get(&tag, "skillName")?;
                        sn.truthy() && lower(&sn)? == want
                    };
                    if get(&tag, "neg")?.truthy() {
                        matched = !matched;
                    }
                    if !matched {
                        return Ok(None);
                    }
                }
                b"SkillId" => {
                    if !cfg.truthy() {
                        return Ok(None);
                    }
                    let ge = get(cfg, "skillGrantedEffect")?;
                    if !ge.truthy() || !get(&ge, "id")?.raw_eq(&get(&tag, "skillId")?) {
                        return Ok(None);
                    }
                }
                b"SkillPart" => {
                    if !cfg.truthy() {
                        return Ok(None);
                    }
                    let part = get(cfg, "skillPart")?;
                    let list = get(&tag, "skillPartList")?;
                    let mut matched = if list.truthy() {
                        ipairs(&list)?.iter().any(|p| p.raw_eq(&part))
                    } else {
                        get(&tag, "skillPart")?.raw_eq(&part)
                    };
                    if get(&tag, "neg")?.truthy() {
                        matched = !matched;
                    }
                    if !matched {
                        return Ok(None);
                    }
                }
                b"SkillType" => {
                    let types = if cfg.truthy() {
                        get(cfg, "skillTypes")?
                    } else {
                        Value::Nil
                    };
                    let has = |t: &Value| -> LResult<bool> {
                        Ok(types.truthy() && rt::index(&types, t)?.truthy())
                    };
                    let list = get(&tag, "skillTypeList")?;
                    let mut matched = false;
                    if list.truthy() {
                        for (_, t) in table(&list)?.entries() {
                            if has(&t)? {
                                matched = true;
                                break;
                            }
                        }
                    } else {
                        matched = has(&get(&tag, "skillType")?)?;
                    }
                    if get(&tag, "neg")?.truthy() {
                        matched = !matched;
                    }
                    if !matched {
                        return Ok(None);
                    }
                }
                b"BaseFlag" => {
                    let mut base_flags = Value::Nil;
                    if cfg.truthy() {
                        let ge = get(cfg, "skillGrantedEffect")?;
                        let bf = if ge.truthy() {
                            get(&ge, "baseFlags")?
                        } else {
                            ge
                        };
                        base_flags = or(bf, || get(cfg, "baseFlags"))?;
                    }
                    let mut matched = base_flags.truthy()
                        && rt::index(&base_flags, &get(&tag, "baseFlag")?)?.truthy();
                    if get(&tag, "neg")?.truthy() {
                        matched = !matched;
                    }
                    if !matched {
                        return Ok(None);
                    }
                }
                b"SlotName" => {
                    if !cfg.truthy() {
                        return Ok(None);
                    }
                    let slot = get(cfg, "slotName")?;
                    let list = get(&tag, "slotNameList")?;
                    let mut matched = if list.truthy() {
                        ipairs(&list)?.iter().any(|p| p.raw_eq(&slot))
                    } else {
                        get(&tag, "slotName")?.raw_eq(&slot)
                    };
                    if get(&tag, "neg")?.truthy() {
                        matched = !matched;
                    }
                    if !matched {
                        return Ok(None);
                    }
                }
                b"ModFlagOr" => {
                    let flags = if cfg.truthy() {
                        get(cfg, "flags")?
                    } else {
                        Value::Nil
                    };
                    if !flags.truthy() || band(&flags, &get(&tag, "modFlags")?)? == 0.0 {
                        return Ok(None);
                    }
                }
                b"KeywordFlagAnd" => {
                    let flags = if cfg.truthy() {
                        get(cfg, "keywordFlags")?
                    } else {
                        Value::Nil
                    };
                    if !flags.truthy() {
                        return Ok(None);
                    }
                    let want = get(&tag, "keywordFlags")?;
                    if !n(band(&flags, &want)?).raw_eq(&want) {
                        return Ok(None);
                    }
                }
                b"MonsterTag" => {
                    let actor = get(store, "actor")?;
                    let minion = if actor.truthy() {
                        get(&actor, "minionData")?
                    } else {
                        Value::Nil
                    };
                    let tags = if minion.truthy() {
                        get(&minion, "monsterTags")?
                    } else {
                        Value::Nil
                    };
                    if !tags.truthy() {
                        return Ok(None);
                    }
                    let list = get(&tag, "monsterTagList")?;
                    let mut matched = false;
                    for (_, t) in table(&tags)?.entries() {
                        let want = lower(&t)?;
                        if list.truthy() {
                            for (_, nm) in table(&list)?.entries() {
                                if lower(&nm)? == want {
                                    matched = true;
                                    break;
                                }
                            }
                        } else {
                            let mt = get(&tag, "monsterTag")?;
                            matched = mt.truthy() && lower(&mt)? == want;
                        }
                        if matched {
                            break;
                        }
                    }
                    if get(&tag, "neg")?.truthy() {
                        matched = !matched;
                    }
                    if !matched {
                        return Ok(None);
                    }
                }
                _ => {}
            }
        }
        Ok(Some(value))
    }

    /// The `Condition` tag; false when the mod does not apply.
    fn condition_tag(&self, store: &Value, tag: &Value, cfg: &Value) -> LResult<bool> {
        let actor = get(store, "actor")?;
        let one_handed = |w: &str| -> LResult<Value> {
            let d = get(&actor, w)?;
            if d.truthy() && get(&d, "countsAsAll1H")?.truthy() {
                Ok(d)
            } else {
                Ok(Value::Bool(false))
            }
        };
        let all_one_h = or(one_handed("weaponData1")?, || one_handed("weaponData2"))?;
        let neg = get(tag, "neg")?.truthy();
        let skill_cond = |var: &Value| -> LResult<bool> {
            if !cfg.truthy() {
                return Ok(false);
            }
            let sc = get(cfg, "skillCond")?;
            Ok(sc.truthy() && rt::index(&sc, var)?.truthy())
        };
        let mut matched = false;
        let list = get(tag, "varList")?;
        if list.truthy() {
            for (_, var) in table(&list)?.entries() {
                if neg && all_one_h.truthy() {
                    let added = rt::index(&all_one_h, &rt::concat(&s("Added"), &var)?)?;
                    if !added.is_nil() {
                        if !added.truthy() {
                            return Ok(false);
                        }
                        continue;
                    }
                }
                if self.get_condition(store, &var, cfg, false)?.truthy() || skill_cond(&var)? {
                    matched = true;
                    break;
                }
            }
        } else {
            let var = get(tag, "var")?;
            let mut handled = false;
            if neg && all_one_h.truthy() {
                let added = rt::index(&all_one_h, &rt::concat(&s("Added"), &var)?)?;
                if !added.is_nil() {
                    if !added.truthy() {
                        return Ok(false);
                    }
                    handled = true;
                }
            }
            if !handled {
                matched =
                    self.get_condition(store, &var, cfg, false)?.truthy() || skill_cond(&var)?;
            }
        }
        if neg {
            matched = !matched;
        }
        Ok(matched)
    }
}

/// `string.lower(v)`
fn lower(v: &Value) -> LResult<Vec<u8>> {
    match v {
        Value::Str(x) => Ok(lua::lower(x.as_bytes())),
        Value::Number(_) => Ok(lua::lower(lua::to_string(v).as_bytes())),
        v => Err(LuaError::msg(format!(
            "attempt to index a {} value",
            v.type_name()
        ))),
    }
}

/// The values of `ipairs(t)`
fn ipairs(v: &Value) -> LResult<Vec<Value>> {
    table(v)?;
    let mut out = Vec::new();
    let mut i = 1;
    loop {
        let x = rt::index(v, &n(i as f64))?;
        if x.is_nil() {
            break;
        }
        out.push(x);
        i += 1;
    }
    Ok(out)
}

/// `math.min` and `math.max` as LuaJIT computes them: the first argument
/// unless the second compares smaller (larger)
trait MinMax {
    fn min_lua(self, b: f64) -> f64;
    fn max_lua(self, b: f64) -> f64;
}

impl MinMax for f64 {
    fn min_lua(self, b: f64) -> f64 {
        if b < self { b } else { self }
    }
    fn max_lua(self, b: f64) -> f64 {
        if b > self { b } else { self }
    }
}

fn opt_bool(v: &Value) -> bool {
    v.truthy()
}

/// Replaces the translated query methods of upstream's ModStore, ModDB and
/// ModList classes with the native ones. Run after upstream started, when
/// the classes exist.
pub fn install(env: &Rc<Env>) -> LResult<()> {
    let classes = rt::index(&env.global("common"), &s("classes"))?;
    let class =
        |name: &str| -> LResult<TableRef> { Ok(table(&rt::index(&classes, &s(name))?)?.clone()) };
    let store = class("ModStore")?;
    let mod_db = class("ModDB")?;
    let mod_list = class("ModList")?;
    let high_precision = rt::index(&env.global("data"), &s("highPrecisionMods"))?;
    let key = |x: &str| Key::from(x);
    let c = Rc::new(Ctx {
        mod_db: mod_db.clone(),
        mod_list: mod_list.clone(),
        eval_mod: store.get_str("EvalMod"),
        get_stat: store.get_str("GetStat"),
        high_precision,
        k: Keys {
            name: key("name"),
            typ: key("type"),
            flags: key("flags"),
            keyword_flags: key("keywordFlags"),
            source: key("source"),
            value: key("value"),
            mods: key("mods"),
            parent: key("parent"),
        },
    });

    // Replaces `name` on `class`, and on the classes that copied the old
    // method down from it (newClass caches inherited methods)
    let all: Vec<TableRef> = table(&classes)?
        .entries()
        .into_iter()
        .filter_map(|(_, v)| v.as_table().cloned())
        .collect();
    let replace = |class: &TableRef, name: &str, f: Value| {
        let old = class.get_str(name);
        for other in &all {
            if !other.ptr_eq(class) && !old.is_nil() && other.get_str(name).raw_eq(&old) {
                other.set_str(name, f.clone());
            }
        }
        class.set_str(name, f);
    };
    macro_rules! native {
        ($class:expr, $name:literal, |$c:ident, $a:ident| $body:expr) => {{
            let $c = c.clone();
            replace(
                $class,
                $name,
                func(concat!("native ", $name), move |$a: &[Value]| $body),
            );
        }};
    }
    fn rest(a: &[Value], from: usize) -> &[Value] {
        a.get(from..).unwrap_or(&[])
    }
    fn one(v: LResult<Value>) -> LResult<Vec<Value>> {
        Ok(vec![v?])
    }

    // ModStore
    native!(&store, "Sum", |c, a| one(c.sum(
        &a[0],
        &arg(a, 1),
        &arg(a, 2),
        rest(a, 3)
    )));
    native!(&store, "More", |c, a| one(c.more(
        &a[0],
        &arg(a, 1),
        rest(a, 2)
    )));
    native!(&store, "Flag", |c, a| {
        let v = c.flag(&a[0], &arg(a, 1), rest(a, 2))?;
        Ok(if v.is_nil() { vec![] } else { vec![v] })
    });
    native!(&store, "Override", |c, a| {
        let v = c.override_(&a[0], &arg(a, 1), rest(a, 2))?;
        Ok(if v.is_nil() { vec![] } else { vec![v] })
    });
    native!(&store, "List", |c, a| one(c.list(
        &a[0],
        &arg(a, 1),
        rest(a, 2)
    )));
    native!(&store, "Tabulate", |c, a| {
        one(c
            .tabulate(&a[0], &arg(a, 1), &arg(a, 2), rest(a, 3))
            .map(Value::Table))
    });
    native!(&store, "Max", |c, a| one(c.extreme(
        &a[0],
        &arg(a, 1),
        rest(a, 2),
        true
    )));
    native!(&store, "Min", |c, a| one(c.extreme(
        &a[0],
        &arg(a, 1),
        rest(a, 2),
        false
    )));
    native!(&store, "Combine", |c, a| one(c.combine(
        &a[0],
        &arg(a, 1),
        &arg(a, 2),
        rest(a, 3)
    )));
    native!(&store, "HasMod", |c, a| {
        let flt = Filter::from_cfg(&arg(a, 2))?;
        Ok(vec![Value::Bool(c.has_mod_internal(
            &a[0],
            &arg(a, 1),
            &flt,
            rest(a, 3),
        )?)])
    });
    native!(&store, "GetCondition", |c, a| one(c.get_condition(
        &a[0],
        &arg(a, 1),
        &arg(a, 2),
        opt_bool(&arg(a, 3))
    )));
    native!(&store, "GetMultiplier", |c, a| one(c.get_multiplier(
        &a[0],
        &arg(a, 1),
        &arg(a, 2),
        opt_bool(&arg(a, 3))
    )));
    native!(&store, "GetStat", |c, a| one(c.get_stat(
        &a[0],
        &arg(a, 1),
        &arg(a, 2)
    )));
    native!(&store, "EvalMod", |c, a| {
        let m = arg(a, 1);
        let lim = arg(a, 3);
        let v = c.eval_mod(&a[0], table(&m)?, &arg(a, 2), lim.as_table())?;
        Ok(if v.is_nil() { vec![] } else { vec![v] })
    });

    // ModDB and ModList
    for class in [&mod_db, &mod_list] {
        native!(class, "SumInternal", |c, a| {
            let flt = Filter::from_args(rest(a, 4));
            one(c.sum_internal(&a[0], &arg(a, 1), &arg(a, 2), &arg(a, 3), &flt, rest(a, 7)))
        });
        native!(class, "MoreInternal", |c, a| {
            let flt = Filter::from_args(rest(a, 3));
            one(c.more_internal(&a[0], &arg(a, 1), &arg(a, 2), &flt, rest(a, 6)))
        });
        native!(class, "FlagInternal", |c, a| {
            let flt = Filter::from_args(rest(a, 3));
            let v = c.first_internal(
                "FlagInternal",
                "FLAG",
                &a[0],
                &arg(a, 1),
                &arg(a, 2),
                &flt,
                rest(a, 6),
            )?;
            Ok(if v.is_nil() { vec![] } else { vec![v] })
        });
        native!(class, "OverrideInternal", |c, a| {
            let flt = Filter::from_args(rest(a, 3));
            let v = c.first_internal(
                "OverrideInternal",
                "OVERRIDE",
                &a[0],
                &arg(a, 1),
                &arg(a, 2),
                &flt,
                rest(a, 6),
            )?;
            Ok(if v.is_nil() { vec![] } else { vec![v] })
        });
        native!(class, "ListInternal", |c, a| {
            let flt = Filter::from_args(rest(a, 4));
            let r = arg(a, 2);
            c.list_internal(&a[0], &arg(a, 1), table(&r)?, &arg(a, 3), &flt, rest(a, 7))?;
            Ok(vec![])
        });
        native!(class, "TabulateInternal", |c, a| {
            let flt = Filter::from_args(rest(a, 5));
            let r = arg(a, 2);
            c.tabulate_internal(
                &a[0],
                &arg(a, 1),
                table(&r)?,
                &arg(a, 3),
                &arg(a, 4),
                &flt,
                rest(a, 8),
            )?;
            Ok(vec![])
        });
    }
    native!(&mod_db, "HasModInternal", |c, a| {
        let flt = Filter::from_args(rest(a, 2));
        Ok(vec![Value::Bool(c.has_mod_internal(
            &a[0],
            &arg(a, 1),
            &flt,
            rest(a, 5),
        )?)])
    });
    Ok(())
}
