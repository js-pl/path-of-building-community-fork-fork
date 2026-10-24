//! Imports the tables of upstream's `src/Modules/ModParser.lua`.
//!
//! The literal tables (`formList`, `modNameList`, `specialModList`, ...) are
//! evaluated from upstream's source, so their thousands of patterns never
//! have to be copied by hand. Functions in them become [`lua::FuncRef`]s that
//! the engine maps to their Rust ports. The code around the tables is ported
//! in `engine::mod_parser`.

use crate::{Importer, Raw, Result};
use lua::{Closure, TableRef, Value};
use std::collections::HashMap;
use std::rc::Rc;

pub const FILE: &str = "Modules/ModParser.lua";

type NativeFn = dyn Fn(&[Value]) -> std::result::Result<Vec<Value>, String>;

/// The `local` tables of ModParser.lua that are imported, in file order.
pub const TABLES: &[&str] = &[
    "conquerorList",
    "formList",
    "modNameList",
    "modFlagList",
    "preFlagList",
    "modTagList",
    "damageTypeList",
    "gemIdLookup",
    "specialModList",
    "unsupportedModList",
    "suffixTypes",
    "dmgTypes",
    "penTypes",
    "resourceTypes",
    "flagTypes",
    "skillNameList",
    "jewelOtherFuncs",
    "jewelSelfFuncs",
    "jewelSelfUnallocFuncs",
    "jewelThresholdFuncs",
];

/// Closure value for a call of one of ModParser's function factories
/// (`getSimpleConv`, `getPerStat`, `getThreshold`) at table construction.
fn factory(name: &'static str) -> impl Fn(&[Value]) -> std::result::Result<Vec<Value>, String> {
    move |args| {
        Ok(vec![Value::Closure(Rc::new(Closure {
            name: name.into(),
            upvalues: args.to_vec(),
        }))])
    }
}

/// Evaluates the ModParser tables. The `data` constants they use at
/// construction time come from `raw`.
pub fn import(im: &mut Importer, raw: &Raw) -> Result<TableRef> {
    let src = im.read(FILE)?;
    let mut locals = HashMap::new();
    let mut native = |name: &str, f: Box<NativeFn>| {
        let v = im.env.native(name, f);
        locals.insert(name.to_string(), v);
    };
    native(
        "mod",
        Box::new(|args| Ok(vec![Value::Table(engine::mod_tools::create_mod_args(args))])),
    );
    native(
        "flag",
        Box::new(|args| {
            let mut a = vec![
                args.first().cloned().unwrap_or_default(),
                "FLAG".into(),
                true.into(),
            ];
            a.extend(args.iter().skip(1).cloned());
            Ok(vec![Value::Table(engine::mod_tools::create_mod_args(&a))])
        }),
    );
    native("getSimpleConv", Box::new(factory("getSimpleConv")));
    native("getPerStat", Box::new(factory("getPerStat")));
    native("getThreshold", Box::new(factory("getThreshold")));
    let bit = im.global("bit");
    for f in ["bor", "band", "bnot"] {
        locals.insert(f.to_string(), bit.as_table().expect("bit").get_str(f));
    }
    locals.insert("m_huge".into(), Value::Number(f64::INFINITY));
    locals.insert("data".into(), Value::Table(raw.data.clone()));
    for g in ["ModFlag", "KeywordFlag", "SkillType"] {
        locals.insert(g.to_string(), raw.globals.get_str(g));
    }
    let tables = im.env.eval_locals(FILE, &src, TABLES, locals)?;
    let out = TableRef::new();
    for name in TABLES {
        out.set_str(name, tables[*name].clone());
    }
    Ok(out)
}

/// Imports `Data/ModCache.lua`: line -> `{ modList, extra }`, the parse
/// results upstream preloads into `modLib.parseMod`'s memo cache.
pub fn import_mod_cache(im: &mut Importer) -> Result<TableRef> {
    let cache = TableRef::new();
    im.load("Data/ModCache", vec![Value::Table(cache.clone())])?;
    Ok(cache)
}
