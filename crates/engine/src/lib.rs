//! Unofficial rewrite of Path of Building Community: the engine.
//!
//! Free-form ports of upstream, replacing its translation piece by piece,
//! see docs/design.md.

pub mod common;
pub mod data;
pub mod mod_parser;
pub mod mod_store;
pub mod mod_tools;
pub mod mods;
pub mod rt;

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
