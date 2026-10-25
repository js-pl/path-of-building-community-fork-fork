//! Unofficial rewrite of Path of Building Community: the engine.
//!
//! Free-form ports of upstream, replacing its translation piece by piece,
//! see docs/design.md.

pub mod calc_setup;
pub mod common;
pub mod config;
pub mod data;
pub mod item;
pub mod item_data;
pub mod item_tools;
pub mod items_tab;
pub mod jewel;
pub mod mod_parser;
pub mod mod_store;
pub mod mods;
pub mod pat;
pub mod rt;
pub mod skill_data;
pub mod skills;
pub mod spec;
pub mod tree;
pub mod xml;

/// Replaces translated parts of upstream's running program with the
/// engine's native ports. With `POB_TRANSLATED` set, the program stays
/// fully translated, to compare the two.
pub fn install(env: &std::rc::Rc<lua::env::Env>) -> lua::LResult<()> {
    if std::env::var_os("POB_TRANSLATED").is_some() {
        return Ok(());
    }
    mod_store::install(env)
}

/// Upstream's class `name` (`common.classes[name]`), loaded as `getClass`
/// of `Modules/Common.lua` loads it.
pub(crate) fn class(env: &std::rc::Rc<lua::env::Env>, name: &str) -> lua::LResult<lua::TableRef> {
    use lua::{LuaError, Value, rt};
    let classes = rt::index(&env.global("common"), &Value::str("classes"))?;
    let mut class = rt::index(&classes, &Value::str(name))?;
    if class.is_nil() {
        rt::call(
            &env.global("LoadModule"),
            &[Value::str(&format!("Classes/{name}"))],
        )?;
        class = rt::index(&classes, &Value::str(name))?;
    }
    match class {
        Value::Table(t) => Ok(t),
        _ => Err(LuaError::msg(format!("class '{name}' is not defined"))),
    }
}
