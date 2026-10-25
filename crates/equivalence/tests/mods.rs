//! The typed mod queries (`engine::mods`) answer as upstream's translated
//! ModStore, ModDB and ModList do, on the stores of every corpus build's
//! main calculation: every query for every mod name and type present, with
//! no cfg and with each active skill's cfg.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::mods::convert::Converter;
use engine::mods::*;
use equivalence::bridge::{self, Bridge};
use equivalence::{corpus, upstream_src};
use lua::rt::{self, first};
use lua::{LuaError, Value};
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

/// Numbers compare bitwise; NaN equals NaN
fn same(a: &ModValue, b: &ModValue) -> bool {
    match (a, b) {
        (ModValue::Number(x), ModValue::Number(y)) => {
            x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan())
        }
        // An empty Lua table converts as an empty record
        (ModValue::List(x), ModValue::Record(y)) | (ModValue::Record(y), ModValue::List(x)) => {
            x.is_empty() && y.fields.is_empty()
        }
        (ModValue::List(x), ModValue::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (ModValue::Record(x), ModValue::Record(y)) => {
            x.fields.len() == y.fields.len()
                && x.fields
                    .iter()
                    .all(|(k, v)| y.fields.iter().any(|(k2, v2)| k == k2 && same(v, v2)))
        }
        (ModValue::Mod(x), ModValue::Mod(y)) => {
            x.name == y.name && x.typ == y.typ && same(&x.value, &y.value)
        }
        _ => a == b,
    }
}

struct Run<'a> {
    conv: Converter<'a>,
    failures: Vec<String>,
    queries: usize,
}

impl Run<'_> {
    /// Compares a typed result with the translated one
    fn check(
        &mut self,
        what: &str,
        typed: Result<Option<ModValue>, String>,
        lua: Result<Vec<Value>, LuaError>,
    ) {
        self.queries += 1;
        let lua = lua.map(|r| {
            let v = first(r);
            if v.is_nil() { None } else { Some(v) }
        });
        let ok = match (&typed, &lua) {
            (Err(_), Err(_)) => true,
            (Ok(None), Ok(None)) => true,
            (Ok(Some(t)), Ok(Some(l))) => match self.conv.value(l) {
                Ok(l) => same(t, &l),
                Err(e) => {
                    self.failures
                        .push(format!("{what}: cannot convert the translated result: {e}"));
                    return;
                }
            },
            _ => false,
        };
        if !ok && self.failures.len() < 50 {
            let lua = match lua {
                Ok(Some(v)) => format!("{v:?}"),
                Ok(None) => "nil".into(),
                Err(e) => format!("error {}", describe(&e)),
            };
            self.failures
                .push(format!("{what}: typed {typed:?} != translated {lua}"));
        }
    }
}

fn num(r: Result<f64, String>) -> Result<Option<ModValue>, String> {
    r.map(|n| Some(ModValue::Number(n)))
}

fn boolean(r: Result<bool, String>) -> Result<Option<ModValue>, String> {
    r.map(|b| Some(ModValue::Bool(b)))
}

/// Translated Flag and GetCondition give `true` or nil/false
fn lua_bool(r: Result<Vec<Value>, LuaError>) -> Result<Vec<Value>, LuaError> {
    r.map(|v| vec![Value::Bool(first(v).truthy())])
}

#[test]
fn mod_queries() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    engine::install(&host.env).unwrap_or_else(|e| panic!("{}", describe(&e)));
    let mut failures = Vec::new();
    let (mut ran, mut queries) = (0, 0);
    for (id, path) in corpus() {
        if !id.contains(&filter) {
            continue;
        }
        ran += 1;
        let text = std::fs::read(&path).unwrap();
        let loaded = if path.extension().is_some_and(|e| e == "json") {
            host.load_build_json(&text)
        } else {
            host.load_build_xml(&text, &id)
        };
        loaded.unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let env = get(&get(&host.build().unwrap(), "calcsTab"), "mainEnv");
        let mut b = Bridge::new(host.env.clone());
        let mut actors = Vec::new();
        for actor in ["player", "enemy", "minion"] {
            let a = get(&env, actor);
            if a.truthy() {
                let db = get(&a, "modDB");
                let sid = b
                    .add(&db)
                    .unwrap_or_else(|e| panic!("{id}: {actor} modDB: {e}"));
                actors.push((actor, sid, db));
            }
        }
        // The cfgs: none, and every active skill's
        let mut cfgs = vec![Value::Nil];
        for skill in get(&get(&env, "player"), "activeSkillList")
            .as_table()
            .map(|t| t.array())
            .unwrap_or_default()
        {
            cfgs.push(get(&skill, "skillCfg"));
        }
        let mut run = Run {
            conv: Converter {
                originals: Some(&b.originals),
                ..Converter::default()
            },
            failures: Vec::new(),
            queries: 0,
        };
        let q = Query {
            stores: &b.stores,
            actors: &b,
        };
        for (actor, sid, db) in &actors {
            // Names and types present in the store and its parents
            let mut present: BTreeMap<Sym, Vec<ModType>> = BTreeMap::new();
            let mut conditions = Vec::new();
            let mut multipliers = Vec::new();
            let mut cur = Some(*sid);
            while let Some(c) = cur {
                let store = &b.stores.stores[c];
                let mods: Vec<&Mod> = match &store.mods {
                    Mods::Db(m) => m.values().flatten().collect(),
                    Mods::List(l) => l.iter().collect(),
                };
                for m in mods {
                    let types = present.entry(m.name).or_default();
                    if !types.contains(&m.typ) {
                        types.push(m.typ);
                    }
                }
                conditions.extend(store.conditions.keys().copied());
                multipliers.extend(store.multipliers.keys().copied());
                cur = store.parent;
            }
            for name in present.keys() {
                if let Some(v) = name.as_str().strip_prefix("Condition:") {
                    conditions.push(Sym::new(v));
                }
                if let Some(v) = name.as_str().strip_prefix("Multiplier:") {
                    multipliers.push(Sym::new(v));
                }
            }
            conditions.sort();
            conditions.dedup();
            multipliers.sort();
            multipliers.dedup();
            let is_db = matches!(b.stores.stores[*sid].mods, Mods::Db(_));
            for (ci, lua_cfg) in cfgs.iter().enumerate() {
                let cfg = bridge::cfg(lua_cfg).unwrap_or_else(|e| panic!("{id}: cfg {ci}: {e}"));
                let cfg = cfg.as_ref();
                *b.lua_cfg.borrow_mut() = lua_cfg.clone();
                for (name, types) in &present {
                    let n = s(name.as_str());
                    let names = [*name];
                    let at = |q: &str| format!("{id} {actor} cfg {ci} {q} {name}");
                    for typ in types {
                        let t = s(typ.name());
                        match typ {
                            ModType::More => run.check(
                                &at("More"),
                                num(q.more(*sid, cfg, &names)),
                                bridge::call(db, "More", &[lua_cfg.clone(), n.clone()]),
                            ),
                            ModType::Flag => run.check(
                                &at("Flag"),
                                boolean(q.flag(*sid, cfg, &names)),
                                lua_bool(bridge::call(db, "Flag", &[lua_cfg.clone(), n.clone()])),
                            ),
                            ModType::Override => run.check(
                                &at("Override"),
                                q.override_value(*sid, cfg, &names),
                                bridge::call(db, "Override", &[lua_cfg.clone(), n.clone()]),
                            ),
                            ModType::List => run.check(
                                &at("List"),
                                q.list(*sid, cfg, &names).map(|l| Some(ModValue::List(l))),
                                bridge::call(db, "List", &[lua_cfg.clone(), n.clone()]),
                            ),
                            ModType::Max | ModType::Min => {
                                let max = *typ == ModType::Max;
                                run.check(
                                    &at(typ.name()),
                                    q.extreme(*sid, cfg, &names, max)
                                        .map(|v| v.map(ModValue::Number)),
                                    bridge::call(
                                        db,
                                        if max { "Max" } else { "Min" },
                                        &[lua_cfg.clone(), n.clone()],
                                    ),
                                );
                            }
                            _ => run.check(
                                &at(&format!("Sum {}", typ.name())),
                                num(q.sum(*sid, *typ, cfg, &names)),
                                bridge::call(db, "Sum", &[t.clone(), lua_cfg.clone(), n.clone()]),
                            ),
                        }
                        if is_db {
                            run.check(
                                &at(&format!("HasMod {}", typ.name())),
                                boolean(q.has_mod(*sid, *typ, cfg, &names)),
                                bridge::call(db, "HasMod", &[t, lua_cfg.clone(), n.clone()]),
                            );
                        }
                    }
                    // Tabulate of every type: values and which mods, in order
                    let typed = q.tabulate(*sid, None, cfg, &names).map(|rows| {
                        Some(ModValue::List(
                            rows.into_iter()
                                .map(|r| {
                                    ModValue::List(vec![
                                        r.value,
                                        ModValue::Str(r.m.typ.name().into()),
                                        ModValue::Str(
                                            r.m.source.clone().unwrap_or_else(|| "".into()),
                                        ),
                                    ])
                                })
                                .collect(),
                        ))
                    });
                    let lua =
                        bridge::call(db, "Tabulate", &[Value::Nil, lua_cfg.clone(), n.clone()])
                            .map(|r| {
                                let rows =
                                    first(r).as_table().map(|t| t.array()).unwrap_or_default();
                                let list = lua::TableRef::new();
                                for row in rows {
                                    let m = get(&row, "mod");
                                    let src = get(&m, "source");
                                    list.push(Value::Table(lua::list![
                                        get(&row, "value"),
                                        get(&m, "type"),
                                        if src.is_nil() { s("") } else { src }
                                    ]));
                                }
                                vec![Value::Table(list)]
                            });
                    run.check(&at("Tabulate"), typed, lua);
                }
                for var in &conditions {
                    let at = format!("{id} {actor} cfg {ci} GetCondition {var}");
                    run.check(
                        &at,
                        boolean(q.condition(*sid, *var, cfg, false)),
                        lua_bool(bridge::call(
                            db,
                            "GetCondition",
                            &[s(var.as_str()), lua_cfg.clone()],
                        )),
                    );
                }
                for var in &multipliers {
                    let at = format!("{id} {actor} cfg {ci} GetMultiplier {var}");
                    run.check(
                        &at,
                        num(q.multiplier(*sid, *var, cfg, false)),
                        bridge::call(db, "GetMultiplier", &[s(var.as_str()), lua_cfg.clone()]),
                    );
                }
            }
        }
        queries += run.queries;
        failures.extend(run.failures);
    }
    assert!(ran > 0, "no build matches CALCS_FILTER={filter}");
    engine::mods::convert::UNKNOWN.with(|u| {
        for (k, n) in u.borrow().iter() {
            eprintln!("unknown {k}: {n}");
        }
    });
    eprintln!("{queries} queries on {ran} builds");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
