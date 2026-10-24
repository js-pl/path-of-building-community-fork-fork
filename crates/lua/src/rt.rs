//! Lua operations with LuaJIT's semantics, for code translated from Lua:
//! indexing with metatables, calls, arithmetic with string coercion,
//! comparison, concatenation. Errors are returned, as Lua raises them.

use crate::value::{Function, LResult, LuaError, LuaStr, TableRef, Value};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// The string library, which indexing a string reaches (Lua's string metatable)
    static STRING_LIB: RefCell<Option<TableRef>> = const { RefCell::new(None) };
}

/// Sets the table indexing a string reaches, normally the `string` global.
pub fn set_string_lib(t: TableRef) {
    STRING_LIB.with(|s| *s.borrow_mut() = Some(t));
}

fn string_lib() -> Option<TableRef> {
    STRING_LIB.with(|s| s.borrow().clone())
}

pub fn err<T>(msg: impl Into<String>) -> LResult<T> {
    Err(LuaError::msg(msg))
}

/// Wraps a Rust closure as a Lua function value.
pub fn func(name: &str, f: impl Fn(&[Value]) -> LResult<Vec<Value>> + 'static) -> Value {
    Value::Function(Rc::new(Function {
        name: name.into(),
        f: Box::new(f),
    }))
}

/// `obj[key]`, with `__index`.
pub fn index(obj: &Value, key: &Value) -> LResult<Value> {
    let mut cur = obj.clone();
    for _ in 0..100 {
        let handler = match &cur {
            Value::Table(t) => {
                let tb = t.borrow();
                let v = tb.get(key);
                if !v.is_nil() {
                    return Ok(v);
                }
                match &tb.meta {
                    None => return Ok(Value::Nil),
                    Some(m) => {
                        let h = m.get_str("__index");
                        if h.is_nil() {
                            return Ok(Value::Nil);
                        }
                        h
                    }
                }
            }
            Value::Str(_) => match string_lib() {
                Some(lib) => return Ok(lib.get(key)),
                None => return Ok(Value::Nil),
            },
            v => return err(format!("attempt to index a {} value", v.type_name())),
        };
        match handler {
            Value::Function(f) => return Ok(first(f.call(&[cur.clone(), key.clone()])?)),
            h => cur = h,
        }
    }
    err("'__index' chain too long; possible loop")
}

/// `obj.name`
pub fn field(obj: &Value, name: &LuaStr) -> LResult<Value> {
    index(obj, &Value::Str(name.clone()))
}

/// `obj[key] = value`, with `__newindex`.
pub fn set_index(obj: &Value, key: Value, value: Value) -> LResult<()> {
    let mut cur = obj.clone();
    for _ in 0..100 {
        let handler = match &cur {
            Value::Table(t) => {
                let handler = {
                    let tb = t.borrow();
                    match &tb.meta {
                        Some(m) if tb.get(&key).is_nil() => m.get_str("__newindex"),
                        _ => Value::Nil,
                    }
                };
                if handler.is_nil() {
                    if key.is_nil() {
                        return err("table index is nil");
                    }
                    if matches!(key, Value::Number(n) if n.is_nan()) {
                        return err("table index is NaN");
                    }
                    t.borrow_mut().set(key, value);
                    return Ok(());
                }
                handler
            }
            v => return err(format!("attempt to index a {} value", v.type_name())),
        };
        match handler {
            Value::Function(f) => {
                f.call(&[cur.clone(), key, value])?;
                return Ok(());
            }
            h => cur = h,
        }
    }
    err("'__newindex' chain too long; possible loop")
}

/// `f(args...)`, with `__call`.
pub fn call(f: &Value, args: &[Value]) -> LResult<Vec<Value>> {
    match f {
        Value::Function(func) => func.call(args),
        Value::Table(t) => {
            let handler = t
                .borrow()
                .meta
                .as_ref()
                .map(|m| m.get_str("__call"))
                .unwrap_or_default();
            if handler.is_nil() {
                return err("attempt to call a table value");
            }
            let mut a = Vec::with_capacity(args.len() + 1);
            a.push(f.clone());
            a.extend_from_slice(args);
            call(&handler, &a)
        }
        Value::Func(r) => err(format!(
            "attempt to call untranslated function {} ({}:{})",
            r.key, r.file, r.line
        )),
        v => err(format!("attempt to call a {} value", v.type_name())),
    }
}

/// `obj:name(args...)`
pub fn method(obj: &Value, name: &LuaStr, args: &[Value]) -> LResult<Vec<Value>> {
    let f = field(obj, name)?;
    if f.is_nil() {
        return err(format!("attempt to call method '{name}' (a nil value)"));
    }
    let mut a = Vec::with_capacity(args.len() + 1);
    a.push(obj.clone());
    a.extend_from_slice(args);
    call(&f, &a)
}

pub fn first(v: Vec<Value>) -> Value {
    v.into_iter().next().unwrap_or_default()
}

fn num(v: &Value, op: &str) -> LResult<f64> {
    match v {
        Value::Number(n) => Ok(*n),
        Value::Str(s) => match crate::str_to_number(s.as_bytes()) {
            Some(n) => Ok(n),
            None => err(format!(
                "attempt to perform arithmetic on a string value ({op})"
            )),
        },
        v => err(format!(
            "attempt to perform arithmetic on a {} value ({op})",
            v.type_name()
        )),
    }
}

#[inline]
pub fn add(a: &Value, b: &Value) -> LResult<Value> {
    if let (Value::Number(x), Value::Number(y)) = (a, b) {
        return Ok(Value::Number(x + y));
    }
    Ok(Value::Number(num(a, "add")? + num(b, "add")?))
}

#[inline]
pub fn sub(a: &Value, b: &Value) -> LResult<Value> {
    if let (Value::Number(x), Value::Number(y)) = (a, b) {
        return Ok(Value::Number(x - y));
    }
    Ok(Value::Number(num(a, "sub")? - num(b, "sub")?))
}

#[inline]
pub fn mul(a: &Value, b: &Value) -> LResult<Value> {
    if let (Value::Number(x), Value::Number(y)) = (a, b) {
        return Ok(Value::Number(x * y));
    }
    Ok(Value::Number(num(a, "mul")? * num(b, "mul")?))
}

#[inline]
pub fn div(a: &Value, b: &Value) -> LResult<Value> {
    if let (Value::Number(x), Value::Number(y)) = (a, b) {
        return Ok(Value::Number(x / y));
    }
    Ok(Value::Number(num(a, "div")? / num(b, "div")?))
}

/// Lua's `%`: `a - floor(a / b) * b`
pub fn modulo(a: &Value, b: &Value) -> LResult<Value> {
    let (x, y) = (num(a, "mod")?, num(b, "mod")?);
    Ok(Value::Number(lua_mod(x, y)))
}

/// Lua's `%` on numbers, as LuaJIT computes it.
pub fn lua_mod(x: f64, y: f64) -> f64 {
    x - (x / y).floor() * y
}

/// `a ^ b`, LuaJIT's `pow`.
pub fn pow(a: &Value, b: &Value) -> LResult<Value> {
    Ok(Value::Number(num(a, "pow")?.powf(num(b, "pow")?)))
}

pub fn unm(a: &Value) -> LResult<Value> {
    Ok(Value::Number(-num(a, "unm")?))
}

/// `a .. b`
pub fn concat(a: &Value, b: &Value) -> LResult<Value> {
    fn part(v: &Value) -> LResult<LuaStr> {
        match v {
            Value::Str(s) => Ok(s.clone()),
            Value::Number(n) => Ok(crate::value::format_number(*n).into()),
            v => err(format!("attempt to concatenate a {} value", v.type_name())),
        }
    }
    let (x, y) = (part(a)?, part(b)?);
    let mut out = Vec::with_capacity(x.len() + y.len());
    out.extend_from_slice(x.as_bytes());
    out.extend_from_slice(y.as_bytes());
    Ok(Value::Str(out.into()))
}

/// `a == b`
#[inline]
pub fn eq(a: &Value, b: &Value) -> bool {
    a.raw_eq(b)
}

fn compare_err<T>(a: &Value, b: &Value) -> LResult<T> {
    if a.type_name() == b.type_name() {
        err(format!("attempt to compare two {} values", a.type_name()))
    } else {
        err(format!(
            "attempt to compare {} with {}",
            a.type_name(),
            b.type_name()
        ))
    }
}

/// `a < b`
#[inline]
pub fn lt(a: &Value, b: &Value) -> LResult<bool> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Ok(x < y),
        (Value::Str(x), Value::Str(y)) => Ok(x.as_bytes() < y.as_bytes()),
        _ => compare_err(a, b),
    }
}

/// `a <= b`
#[inline]
pub fn le(a: &Value, b: &Value) -> LResult<bool> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Ok(x <= y),
        (Value::Str(x), Value::Str(y)) => Ok(x.as_bytes() <= y.as_bytes()),
        _ => compare_err(a, b),
    }
}

/// `#v`
pub fn len(v: &Value) -> LResult<Value> {
    match v {
        Value::Str(s) => Ok(Value::Number(s.len() as f64)),
        Value::Table(t) => Ok(Value::Number(t.borrow().border() as f64)),
        v => err(format!(
            "attempt to get length of a {} value",
            v.type_name()
        )),
    }
}

/// `tostring(v)`, without addresses for tables and functions.
pub fn tostring(v: &Value) -> LuaStr {
    match v {
        Value::Table(t) => {
            let handler = t
                .borrow()
                .meta
                .as_ref()
                .map(|m| m.get_str("__tostring"))
                .unwrap_or_default();
            if let Value::Function(f) = handler
                && let Ok(r) = f.call(std::slice::from_ref(v))
                && let Value::Str(s) = first(r)
            {
                return s;
            }
            format!("table: {:p}", Rc::as_ptr(&t.0)).into()
        }
        Value::Function(f) => format!("function: {:p}", Rc::as_ptr(f)).into(),
        v => crate::to_string(v),
    }
}

/// Result of a numeric `for` loop's control expressions, as Lua checks them.
pub fn for_num(v: &Value, what: &str) -> LResult<f64> {
    match v {
        Value::Number(n) => Ok(*n),
        Value::Str(s) => crate::str_to_number(s.as_bytes())
            .map(Ok)
            .unwrap_or_else(|| err(format!("'for' {what} must be a number"))),
        _ => err(format!("'for' {what} must be a number")),
    }
}

/// `next(t, k)`
pub fn next(t: &TableRef, k: &Value) -> LResult<Option<(Value, Value)>> {
    t.borrow()
        .next(k)
        .map_err(|_| LuaError::msg("invalid key to 'next'"))
}

/// The table `pairs(v)` iterates.
pub fn pairs_table(v: &Value) -> LResult<TableRef> {
    match v {
        Value::Table(t) => Ok(t.clone()),
        v => err(format!(
            "bad argument #1 to 'pairs' (table expected, got {})",
            v.type_name()
        )),
    }
}

/// The table `ipairs(v)` iterates.
pub fn ipairs_table(v: &Value) -> LResult<TableRef> {
    match v {
        Value::Table(t) => Ok(t.clone()),
        v => err(format!(
            "bad argument #1 to 'ipairs' (table expected, got {})",
            v.type_name()
        )),
    }
}

/// A field of a table constructor: nil values are skipped, nil keys rejected.
#[inline]
pub fn tset(t: &mut crate::value::Table, k: Value, v: Value) -> LResult<()> {
    if v.is_nil() {
        return Ok(());
    }
    match &k {
        Value::Nil => err("table index is nil"),
        Value::Number(n) if n.is_nan() => err("table index is NaN"),
        _ => {
            t.set(k, v);
            Ok(())
        }
    }
}

/// A local captured by a closure, shared between them.
pub type Cell = Rc<RefCell<Value>>;

pub fn cell(v: Value) -> Cell {
    Rc::new(RefCell::new(v))
}

/// How a block of translated statements ended.
pub enum Flow {
    Next,
    Break,
    Return(Vec<Value>),
}

/// Reads a cell, releasing the borrow before the value is used.
#[inline]
pub fn get(c: &Cell) -> Value {
    c.borrow().clone()
}

/// Writes a cell.
#[inline]
pub fn set(c: &Cell, v: Value) {
    *c.borrow_mut() = v;
}
