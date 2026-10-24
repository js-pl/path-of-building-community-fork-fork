//! `new(className, ...)` of upstream's class library (`Modules/Common.lua`),
//! in native Rust. [`install`] replaces the translated global.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.
//!
//! The objects are built exactly as upstream builds them: the object with
//! its class as metatable, `Object`, `_parentInit` and a proxy per parent
//! and superparent class, whose `__index` and `__call` behave as upstream's
//! closures. Those are native functions that hold the object weakly, so the
//! cycle collector sees every reference that keeps it alive, and an object
//! needs no closures and captured cells of its own: most objects upstream
//! creates per calculation are mod lists, and this halves what each costs.

use lua::env::Env;
use lua::rt::{self, func};
use lua::{LResult, LuaError, TableRef, Value};
use std::rc::Rc;

fn s(x: &str) -> Value {
    Value::str(x)
}

fn err(msg: String) -> LuaError {
    LuaError::msg(msg)
}

/// `getClass(className)`
fn get_class(env: &Rc<Env>, classes: &Value, name: &Value) -> LResult<TableRef> {
    let mut class = rt::index(classes, name)?;
    if class.is_nil() {
        let path = rt::concat(&s("Classes/"), name)?;
        rt::call(&env.global("LoadModule"), &[path])?;
        class = rt::index(classes, name)?;
    }
    match class {
        Value::Table(t) => Ok(t),
        _ => Err(err(format!(
            "Class '{}' not defined in class file",
            lua::to_string(name)
        ))),
    }
}

/// `class.name`, which a class inherits from its parents
fn field(class: &TableRef, name: &str) -> LResult<Value> {
    rt::index(&Value::Table(class.clone()), &s(name))
}

/// Parent classes of a class, in `pairs(class._superParents)` order
fn super_parents(class: &TableRef) -> LResult<Vec<TableRef>> {
    let sp = field(class, "_superParents")?;
    let Value::Table(sp) = sp else {
        return Err(err("bad argument #1 to 'pairs' (table expected)".into()));
    };
    Ok(sp
        .entries()
        .into_iter()
        .filter_map(|(k, _)| k.to_value().as_table().cloned())
        .collect())
}

/// The proxy of `parent` in `object`: `object[parent._className]`
fn proxy(object: &TableRef, parent: &TableRef, class_name: &Value) -> Value {
    let weak = Rc::downgrade(&object.0);
    let alive = move || {
        weak.upgrade()
            .map(TableRef)
            .ok_or_else(|| err("object of a class proxy was freed".into()))
    };
    let meta = TableRef::new();
    let index = {
        let alive = alive.clone();
        let parent = parent.clone();
        func("new.<anonymous>", move |a| {
            let key = a.get(1).cloned().unwrap_or_default();
            let v = alive()?.get(&key);
            if !v.is_nil() {
                return Ok(vec![v]);
            }
            Ok(vec![rt::index(&Value::Table(parent.clone()), &key)?])
        })
    };
    let call = {
        let parent = parent.clone();
        let class_name = class_name.clone();
        func("new.<anonymous>", move |a| {
            let object = alive()?;
            let parent_name = lua::to_string(&field(&parent, "_className")?);
            let ctor = field(&parent, "_constructor")?;
            if !ctor.truthy() {
                return Err(err(format!(
                    "Parent class '{parent_name}' of class '{}' has no constructor",
                    lua::to_string(&class_name)
                )));
            }
            let init = object.get_str("_parentInit");
            if rt::index(&init, &Value::Table(parent.clone()))?.truthy() {
                return Err(err(format!(
                    "Parent class '{parent_name}' of class '{}' has already been initialised",
                    lua::to_string(&class_name)
                )));
            }
            rt::call(&ctor, a)?;
            let init = rt::index(&Value::Table(object), &s("_parentInit"))?;
            rt::set_index(&init, Value::Table(parent.clone()), Value::Bool(true))?;
            Ok(vec![])
        })
    };
    meta.set_str("__index", index);
    meta.set_str("__newindex", Value::Table(object.clone()));
    meta.set_str("__call", call);
    meta.borrow_mut().meta = Some(meta.clone());
    Value::Table(meta)
}

/// `new(className, ...)`
fn new(env: &Rc<Env>, a: &[Value]) -> LResult<Value> {
    let name = a.first().cloned().unwrap_or_default();
    let classes = rt::index(&env.global("common"), &s("classes"))?;
    let class = get_class(env, &classes, &name)?;
    let object = TableRef::new();
    object.borrow_mut().meta = Some(class.clone());
    let obj = Value::Table(object.clone());
    rt::set_index(&obj, s("Object"), obj.clone())?;
    let has_parents = field(&class, "_parents")?.truthy();
    let class_name = field(&class, "_className")?;
    if has_parents {
        rt::set_index(&obj, s("_parentInit"), Value::Table(TableRef::new()))?;
        for parent in super_parents(&class)? {
            let key = field(&parent, "_className")?;
            rt::set_index(&obj, key, proxy(&object, &parent, &class_name))?;
        }
    }
    let ctor = field(&class, "_constructor")?;
    if ctor.truthy() {
        let mut args = vec![obj.clone()];
        args.extend_from_slice(a.get(1..).unwrap_or(&[]));
        rt::call(&ctor, &args)?;
    }
    if has_parents {
        for parent in super_parents(&class)? {
            let init = rt::index(&obj, &s("_parentInit"))?;
            if field(&parent, "_constructor")?.truthy()
                && !rt::index(&init, &Value::Table(parent.clone()))?.truthy()
            {
                return Err(err(format!(
                    "Parent class '{}' of class '{}' must be initialised",
                    lua::to_string(&field(&parent, "_className")?),
                    lua::to_string(&name)
                )));
            }
        }
    }
    Ok(obj)
}

/// Replaces the translated `new`.
pub fn install(env: &Rc<Env>) -> LResult<()> {
    let weak = Rc::downgrade(env);
    env.set(
        "new",
        func("native new", move |a| {
            let env = weak
                .upgrade()
                .ok_or_else(|| err("environment dropped".into()))?;
            Ok(vec![new(&env, a)?])
        }),
    );
    Ok(())
}
