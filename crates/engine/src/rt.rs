//! Lua operations on [`Value`]s with the semantics upstream runs under, for
//! code ported from upstream's dynamically typed modules. Errors that would
//! raise a Lua error panic, as they would abort upstream too.

use lua::pattern::{self, Repl};
use lua::{Key, LuaStr, TableRef, Value};

pub use lua::{list, table};

/// Number value.
pub fn n(x: f64) -> Value {
    Value::Number(x)
}

/// String value.
pub fn s(x: &str) -> Value {
    Value::str(x)
}

/// String value from bytes, which may be any Lua string.
pub fn sb(x: &[u8]) -> Value {
    Value::Str(LuaStr::new(x))
}

pub fn nil() -> Value {
    Value::Nil
}

/// Argument `i` (0-based) of a ported function; missing arguments are nil.
pub fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or_default()
}

/// Value `i` (0-based) of a multiple return; missing values are nil.
pub fn nth(v: &[Value], i: usize) -> Value {
    v.get(i).cloned().unwrap_or_default()
}

/// `tonumber(v)`
pub fn tonumber(v: &Value) -> Value {
    match lua::to_number(v) {
        Some(x) => Value::Number(x),
        None => Value::Nil,
    }
}

/// `tostring(v)`
pub fn tostring(v: &Value) -> Value {
    Value::Str(lua::to_string(v))
}

fn arith_num(v: &Value, op: &str) -> f64 {
    lua::to_number(v).unwrap_or_else(|| {
        panic!(
            "attempt to perform arithmetic ({op}) on a {} value",
            v.type_name()
        )
    })
}

pub fn add(a: &Value, b: &Value) -> Value {
    n(arith_num(a, "add") + arith_num(b, "add"))
}

pub fn sub(a: &Value, b: &Value) -> Value {
    n(arith_num(a, "sub") - arith_num(b, "sub"))
}

pub fn mul(a: &Value, b: &Value) -> Value {
    n(arith_num(a, "mul") * arith_num(b, "mul"))
}

pub fn div(a: &Value, b: &Value) -> Value {
    n(arith_num(a, "div") / arith_num(b, "div"))
}

/// Lua's `%`: `a - floor(a / b) * b`
pub fn rem(a: &Value, b: &Value) -> Value {
    let (x, y) = (arith_num(a, "mod"), arith_num(b, "mod"));
    n(x - (x / y).floor() * y)
}

pub fn pow(a: &Value, b: &Value) -> Value {
    n(arith_num(a, "pow").powf(arith_num(b, "pow")))
}

pub fn neg(a: &Value) -> Value {
    n(-arith_num(a, "unm"))
}

/// `a .. b`
pub fn concat(a: &Value, b: &Value) -> Value {
    let part = |v: &Value| match v {
        Value::Str(_) | Value::Number(_) => lua::to_string(v),
        _ => panic!("attempt to concatenate a {} value", v.type_name()),
    };
    let (x, y) = (part(a), part(b));
    Value::Str([x.as_bytes(), y.as_bytes()].concat().into())
}

/// `a == b`
pub fn eq(a: &Value, b: &Value) -> bool {
    a.raw_eq(b)
}

fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Value::Str(x), Value::Str(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ => panic!(
            "attempt to compare {} with {}",
            a.type_name(),
            b.type_name()
        ),
    }
}

/// `a < b`
pub fn lt(a: &Value, b: &Value) -> bool {
    if let (Value::Number(x), Value::Number(y)) = (a, b) {
        return x < y;
    }
    compare(a, b) == std::cmp::Ordering::Less
}

/// `a <= b`
pub fn le(a: &Value, b: &Value) -> bool {
    if let (Value::Number(x), Value::Number(y)) = (a, b) {
        return x <= y;
    }
    compare(a, b) != std::cmp::Ordering::Greater
}

/// `#v`
pub fn len(v: &Value) -> Value {
    match v {
        Value::Str(s) => n(s.len() as f64),
        Value::Table(t) => n(t.len() as f64),
        _ => panic!("attempt to get length of a {} value", v.type_name()),
    }
}

/// Functions of Lua's string library, which indexing a string reaches
/// through the string metatable.
const STRING_LIBRARY: &[&str] = &[
    "byte", "char", "dump", "find", "format", "gmatch", "gsub", "len", "lower", "match", "rep",
    "reverse", "sub", "upper",
];

/// Indexing a string: the string library function of that name, or nil.
fn index_string(key: &Value) -> Value {
    match key.as_str() {
        Some(k) if STRING_LIBRARY.iter().any(|f| f.as_bytes() == k.as_bytes()) => {
            Value::Closure(std::rc::Rc::new(lua::Closure {
                name: format!("string.{k}").into(),
                upvalues: vec![],
            }))
        }
        _ => Value::Nil,
    }
}

/// `obj[key]`
pub fn index(obj: &Value, key: &Value) -> Value {
    match obj {
        Value::Table(t) => t.get(key),
        Value::Str(_) => index_string(key),
        _ => panic!("attempt to index a {} value", obj.type_name()),
    }
}

/// `obj.name`
pub fn field(obj: &Value, name: &str) -> Value {
    match obj {
        Value::Table(t) => t.get_str(name),
        Value::Str(_) => index_string(&s(name)),
        _ => panic!(
            "attempt to index a {} value (field '{name}')",
            obj.type_name()
        ),
    }
}

/// `obj[key] = value`
pub fn set_index(obj: &Value, key: Value, value: Value) {
    match obj {
        Value::Table(t) => t.set(key, value),
        _ => panic!("attempt to index a {} value", obj.type_name()),
    }
}

fn str_arg(v: &Value, what: &str) -> LuaStr {
    match v {
        Value::Str(s) => s.clone(),
        Value::Number(_) => lua::to_string(v),
        _ => panic!(
            "bad argument to '{what}' (string expected, got {})",
            v.type_name()
        ),
    }
}

/// `s:lower()`
pub fn lower(v: &Value) -> Value {
    Value::Str(lua::lower(str_arg(v, "lower").as_bytes()).into())
}

/// `s:upper()`
pub fn upper(v: &Value) -> Value {
    Value::Str(lua::upper(str_arg(v, "upper").as_bytes()).into())
}

/// `s:sub(i, j)`
pub fn str_sub(v: &Value, i: &Value, j: &Value) -> Value {
    let s = str_arg(v, "sub");
    let len = s.len() as i64;
    let mut start = lua::to_number(i).unwrap_or(1.0) as i64;
    let mut end = if j.is_nil() {
        -1
    } else {
        lua::to_number(j).expect("sub: end") as i64
    };
    if start < 0 {
        start = (len + start + 1).max(0);
    }
    if end < 0 {
        end = len + end + 1;
    }
    if start < 1 {
        start = 1;
    }
    if end > len {
        end = len;
    }
    if start > end {
        return Value::Str("".into());
    }
    Value::Str(LuaStr::new(&s.as_bytes()[start as usize - 1..end as usize]))
}

/// `s:match(p, init)`: all captures (or the whole match), empty if none.
pub fn str_match(v: &Value, p: &Value, init: &Value) -> Vec<Value> {
    let s = str_arg(v, "match");
    let p = str_arg(p, "match");
    let init = lua::to_number(init).unwrap_or(1.0) as i64;
    match pattern::lua_match(s.as_bytes(), p.as_bytes(), init).expect("malformed pattern") {
        Some(caps) => caps,
        None => vec![Value::Nil],
    }
}

/// `s:find(p, init, plain)`: start, end and captures, or nil.
pub fn str_find(v: &Value, p: &Value, init: &Value, plain: &Value) -> Vec<Value> {
    let s = str_arg(v, "find");
    let p = str_arg(p, "find");
    let init = lua::to_number(init).unwrap_or(1.0) as i64;
    match pattern::lua_find(s.as_bytes(), p.as_bytes(), init, plain.truthy())
        .expect("malformed pattern")
    {
        Some((a, b, caps)) => {
            let mut out = vec![n(a as f64), n(b as f64)];
            out.extend(caps);
            out
        }
        None => vec![Value::Nil],
    }
}

/// A Rust function used as a gsub replacement.
pub type ReplFn<'a> = &'a mut dyn FnMut(&[Value]) -> Value;

/// `s:gsub(p, repl, n)`: the new string and the number of replacements.
/// `repl` is a string, a table or a function value given as `func`.
pub fn gsub(v: &Value, p: &Value, repl: &Value, max_n: &Value) -> Vec<Value> {
    gsub_with(v, p, repl, max_n, None)
}

/// `gsub` with a Rust function as replacement, e.g. `string.upper`.
pub fn gsub_fn(
    v: &Value,
    p: &Value,
    f: &mut dyn FnMut(&[Value]) -> Value,
    max_n: &Value,
) -> Vec<Value> {
    gsub_with(v, p, &Value::Nil, max_n, Some(f))
}

fn gsub_with(v: &Value, p: &Value, repl: &Value, max_n: &Value, f: Option<ReplFn>) -> Vec<Value> {
    let s = str_arg(v, "gsub");
    let p = str_arg(p, "gsub");
    let max_n = lua::to_number(max_n).map(|x| x as usize);
    let (out, count) = match (f, repl) {
        (Some(f), _) => pattern::lua_gsub(s.as_bytes(), p.as_bytes(), Repl::Func(f), max_n),
        (None, Value::Table(t)) => {
            let t = t.clone();
            let mut lookup = move |caps: &[Value]| t.get(&caps[0]);
            pattern::lua_gsub(s.as_bytes(), p.as_bytes(), Repl::Func(&mut lookup), max_n)
        }
        (None, r) => {
            let r = str_arg(r, "gsub");
            pattern::lua_gsub(s.as_bytes(), p.as_bytes(), Repl::Str(r.as_bytes()), max_n)
        }
    }
    .expect("malformed pattern");
    vec![Value::Str(out.into()), n(count as f64)]
}

/// `string.upper` as a gsub replacement
pub fn upper_fn(caps: &[Value]) -> Value {
    upper(&caps[0])
}

/// `string.lower` as a gsub replacement
pub fn lower_fn(caps: &[Value]) -> Value {
    lower(&caps[0])
}

/// `s:gmatch(p)`: every match's captures.
pub fn gmatch(v: &Value, p: &Value) -> Vec<Vec<Value>> {
    let s = str_arg(v, "gmatch");
    let p = str_arg(p, "gmatch");
    pattern::lua_gmatch(s.as_bytes(), p.as_bytes()).expect("malformed pattern")
}

/// `string.format(fmt, ...)` for the conversions upstream uses.
pub fn format(fmt: &Value, args: &[Value]) -> Value {
    let f = str_arg(fmt, "format");
    let f = f.as_bytes();
    let mut out = Vec::new();
    let mut ai = 0;
    let mut i = 0;
    while i < f.len() {
        if f[i] != b'%' {
            out.push(f[i]);
            i += 1;
            continue;
        }
        i += 1;
        if f.get(i) == Some(&b'%') {
            out.push(b'%');
            i += 1;
            continue;
        }
        let flags_start = i;
        while i < f.len() && b"-+ #0".contains(&f[i]) {
            i += 1;
        }
        let flags = std::str::from_utf8(&f[flags_start..i]).expect("ascii");
        let rest_start = i;
        while i < f.len() && (f[i].is_ascii_digit() || f[i] == b'.') {
            i += 1;
        }
        let rest = std::str::from_utf8(&f[rest_start..i]).expect("ascii");
        let conv = f[i];
        i += 1;
        let arg = args.get(ai).cloned().unwrap_or_default();
        ai += 1;
        let (width, prec) = match rest.split_once('.') {
            Some((w, p)) => (
                w.parse::<usize>().ok(),
                Some(p.parse::<usize>().unwrap_or(0)),
            ),
            None => (rest.parse::<usize>().ok(), None),
        };
        let body = match conv {
            b'd' | b'i' => {
                let x = lua::to_number(&arg).expect("format: number expected");
                format!("{}", x as i64)
            }
            b'f' => lua::format::format_f(
                lua::to_number(&arg).expect("format: number"),
                prec.unwrap_or(6),
            ),
            b'g' => lua::format::format_g(
                lua::to_number(&arg).expect("format: number"),
                prec.unwrap_or(6),
            ),
            b'e' => lua::format::format_e(
                lua::to_number(&arg).expect("format: number"),
                prec.unwrap_or(6),
            ),
            b's' => {
                let s = lua::to_string(&arg).to_string();
                match prec {
                    Some(p) => s.chars().take(p).collect(),
                    None => s,
                }
            }
            b'x' => format!("{:x}", lua::to_number(&arg).expect("format: number") as i64),
            b'X' => format!("{:X}", lua::to_number(&arg).expect("format: number") as i64),
            c => panic!("format: unsupported conversion '%{}'", c as char),
        };
        let body = if flags.contains('+')
            && matches!(conv, b'd' | b'i' | b'f' | b'g' | b'e')
            && !body.starts_with('-')
        {
            format!("+{body}")
        } else {
            body
        };
        let padded = match width {
            Some(w) if body.len() < w => {
                if flags.contains('-') {
                    format!("{body:<w$}")
                } else if flags.contains('0') {
                    let (sign, digits) = if body.starts_with(['-', '+']) {
                        body.split_at(1)
                    } else {
                        ("", body.as_str())
                    };
                    format!("{sign}{digits:0>width$}", width = w - sign.len())
                } else {
                    format!("{body:>w$}")
                }
            }
            _ => body,
        };
        out.extend_from_slice(padded.as_bytes());
    }
    Value::Str(out.into())
}

/// `math.floor`
pub fn floor(v: &Value) -> Value {
    n(arith_num(v, "floor").floor())
}

/// `math.min`
pub fn min(args: &[Value]) -> Value {
    let mut m = arith_num(&args[0], "min");
    for a in &args[1..] {
        let x = arith_num(a, "min");
        if x < m {
            m = x;
        }
    }
    n(m)
}

/// `math.max`
pub fn max(args: &[Value]) -> Value {
    let mut m = arith_num(&args[0], "max");
    for a in &args[1..] {
        let x = arith_num(a, "max");
        if x > m {
            m = x;
        }
    }
    n(m)
}

fn bit_arg(v: &Value) -> i32 {
    let x = arith_num(v, "bit");
    (x.rem_euclid(4294967296.0) as u64 as u32) as i32
}

/// `bit.bor`
pub fn bor(args: &[Value]) -> Value {
    n(args.iter().fold(0i32, |acc, v| acc | bit_arg(v)) as f64)
}

/// `bit.band`
pub fn band(args: &[Value]) -> Value {
    n(args
        .iter()
        .skip(1)
        .fold(bit_arg(&args[0]), |acc, v| acc & bit_arg(v)) as f64)
}

/// `bit.bnot`
pub fn bnot(v: &Value) -> Value {
    n(!bit_arg(v) as f64)
}

/// `{ ... }` from keyed fields and positional items. A positional nil leaves
/// a hole, as in Lua.
pub fn tbl(keyed: Vec<(Value, Value)>, items: Vec<Value>) -> Value {
    let t = TableRef::new();
    {
        let mut tm = t.borrow_mut();
        for (k, v) in keyed {
            if !v.is_nil() {
                tm.set(k, v);
            }
        }
        for (i, v) in items.into_iter().enumerate() {
            if !v.is_nil() {
                tm.set_key(Key::Int(i as i64 + 1), v);
            }
        }
    }
    Value::Table(t)
}

/// `table.insert(t, v)`
pub fn t_insert(t: &Value, v: Value) {
    match t {
        Value::Table(t) => t.push(v),
        _ => panic!("bad argument to 'insert' (table expected)"),
    }
}

/// `unpack(t)`: the array part.
pub fn unpack(t: &Value) -> Vec<Value> {
    match t {
        Value::Table(t) => t.array(),
        _ => panic!("bad argument to 'unpack' (table expected)"),
    }
}

/// `ipairs(t)` snapshot.
pub fn ipairs(t: &Value) -> Vec<(Value, Value)> {
    match t {
        Value::Table(t) => t
            .array()
            .into_iter()
            .enumerate()
            .map(|(i, v)| (n(i as f64 + 1.0), v))
            .collect(),
        _ => panic!("bad argument to 'ipairs' (table expected)"),
    }
}

/// `pairs(t)` snapshot. Order is the table's insertion order, not LuaJIT's
/// hash order; ported code that depends on the order needs care.
pub fn pairs(t: &Value) -> Vec<(Value, Value)> {
    match t {
        Value::Table(t) => t
            .entries()
            .into_iter()
            .map(|(k, v)| (k.to_value(), v))
            .collect(),
        _ => panic!("bad argument to 'pairs' (table expected)"),
    }
}

/// `copyTable(t)` from upstream's Common.lua.
pub fn copy_table(v: &Value) -> Value {
    match v {
        Value::Table(t) => Value::Table(t.deep_copy()),
        _ => panic!("copyTable: table expected"),
    }
}

/// `isValueInArray(t, v)` from upstream's Common.lua.
pub fn is_value_in_array(t: &Value, v: &Value) -> bool {
    match t {
        Value::Table(t) => t.array().iter().any(|x| x.raw_eq(v)),
        _ => false,
    }
}

/// First value of a multiple return, as Lua truncates in most positions.
pub fn first(v: Vec<Value>) -> Value {
    v.into_iter().next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_like_c() {
        assert_eq!(
            format(&s("%d%%"), &[n(25.0)]).as_str().unwrap().to_string(),
            "25%"
        );
        assert_eq!(
            format(&s("%.2f"), &[n(1.005)])
                .as_str()
                .unwrap()
                .to_string(),
            "1.00"
        );
        assert_eq!(
            format(&s("%s-%s"), &[s("a"), n(2.0)])
                .as_str()
                .unwrap()
                .to_string(),
            "a-2"
        );
        assert_eq!(
            format(&s("%+d"), &[n(5.0)]).as_str().unwrap().to_string(),
            "+5"
        );
        assert_eq!(
            format(&s("%03d"), &[n(5.0)]).as_str().unwrap().to_string(),
            "005"
        );
    }

    #[test]
    fn sub_like_lua() {
        let v = s("hello");
        assert_eq!(
            str_sub(&v, &n(2.0), &n(3.0)).as_str().unwrap().to_string(),
            "el"
        );
        assert_eq!(
            str_sub(&v, &n(-3.0), &nil()).as_str().unwrap().to_string(),
            "llo"
        );
        assert_eq!(
            str_sub(&v, &n(2.0), &nil()).as_str().unwrap().to_string(),
            "ello"
        );
        assert_eq!(
            str_sub(&v, &n(0.0), &n(0.0)).as_str().unwrap().to_string(),
            ""
        );
    }

    #[test]
    fn gsub_upper() {
        let r = gsub_fn(&s(" main hand"), &s("%W%l"), &mut upper_fn, &nil());
        assert_eq!(r[0].as_str().unwrap().to_string(), " Main Hand");
    }
}
