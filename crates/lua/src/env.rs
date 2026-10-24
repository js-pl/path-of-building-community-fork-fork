//! The global environment of translated code and the parts of LuaJIT's
//! standard library upstream uses, with the same semantics.

use crate::pattern::{self, Repl};
use crate::rt::{self, err, first, func};
use crate::value::{LResult, LuaError, LuaStr, Table, TableRef, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// Loads a module for `require` that is not loaded yet.
pub type Loader = Box<dyn Fn(&Rc<Env>, &str) -> LResult<Value>>;

/// Globals of a translated program (Lua's `_G`).
pub struct Env {
    pub globals: TableRef,
    /// `require` falls back to this for names not in `package.loaded`
    pub loader: RefCell<Option<Loader>>,
}

impl Env {
    /// A new environment with the standard library.
    pub fn new() -> Rc<Env> {
        let env = Rc::new(Env {
            globals: TableRef::new(),
            loader: RefCell::new(None),
        });
        install_stdlib(&env);
        // Built-in libraries are loaded modules, as in LuaJIT
        if let Value::Table(loaded) =
            rt::index(&env.global("package"), &Value::str("loaded")).unwrap_or_default()
        {
            for name in [
                "_G",
                "package",
                "coroutine",
                "table",
                "io",
                "os",
                "string",
                "math",
                "debug",
                "bit",
                "jit",
            ] {
                loaded.set_str(name, env.global(name));
            }
        }
        env
    }

    #[inline]
    pub fn get_global(&self, name: &Value) -> Value {
        self.globals.borrow().get(name)
    }

    pub fn global(&self, name: &str) -> Value {
        self.globals.get_str(name)
    }

    #[inline]
    pub fn set_global(&self, name: Value, value: Value) {
        self.globals.borrow_mut().set(name, value);
    }

    pub fn set(&self, name: &str, value: Value) {
        self.globals.set_str(name, value);
    }
}

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or_default()
}

fn check_str(a: &[Value], i: usize, fname: &str) -> LResult<LuaStr> {
    match a.get(i) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(v @ Value::Number(_)) => Ok(crate::to_string(v)),
        v => err(format!(
            "bad argument #{} to '{fname}' (string expected, got {})",
            i + 1,
            v.map(|v| v.type_name()).unwrap_or("no value")
        )),
    }
}

fn check_num(a: &[Value], i: usize, fname: &str) -> LResult<f64> {
    match a.get(i).and_then(crate::to_number) {
        Some(n) => Ok(n),
        None => err(format!(
            "bad argument #{} to '{fname}' (number expected)",
            i + 1
        )),
    }
}

fn opt_num(a: &[Value], i: usize, fname: &str, default: f64) -> LResult<f64> {
    match a.get(i) {
        None | Some(Value::Nil) => Ok(default),
        _ => check_num(a, i, fname),
    }
}

fn check_table(a: &[Value], i: usize, fname: &str) -> LResult<TableRef> {
    match a.get(i) {
        Some(Value::Table(t)) => Ok(t.clone()),
        v => err(format!(
            "bad argument #{} to '{fname}' (table expected, got {})",
            i + 1,
            v.map(|v| v.type_name()).unwrap_or("no value")
        )),
    }
}

fn num(n: f64) -> Value {
    Value::Number(n)
}

fn s(x: impl Into<LuaStr>) -> Value {
    Value::Str(x.into())
}

fn ok1(v: Value) -> LResult<Vec<Value>> {
    Ok(vec![v])
}

fn lib(env: &Env, name: &str, entries: Vec<(&str, Value)>) -> TableRef {
    let t = match env.global(name) {
        Value::Table(t) => t,
        _ => {
            let t = TableRef::new();
            env.set(name, Value::Table(t.clone()));
            t
        }
    };
    for (k, v) in entries {
        t.set_str(k, v);
    }
    t
}

fn install_stdlib(env: &Rc<Env>) {
    env.set("_G", Value::Table(env.globals.clone()));
    env.set("_VERSION", s("Lua 5.1"));
    install_base(env);
    install_math(env);
    install_string(env);
    install_table(env);
    install_bit(env);
    install_os(env);
    install_misc(env);
}

fn install_base(env: &Rc<Env>) {
    let e = Rc::downgrade(env);
    env.set(
        "type",
        func("type", |a| {
            if a.is_empty() {
                return err("bad argument #1 to 'type' (value expected)");
            }
            ok1(s(a[0].type_name()))
        }),
    );
    env.set(
        "tostring",
        func("tostring", |a| ok1(Value::Str(rt::tostring(&arg(a, 0))))),
    );
    env.set(
        "tonumber",
        func("tonumber", |a| {
            let base = opt_num(a, 1, "tonumber", 10.0)?;
            if base == 10.0 {
                return ok1(crate::to_number(&arg(a, 0)).map(num).unwrap_or_default());
            }
            let st = check_str(a, 0, "tonumber")?;
            let t = st.to_str_lossy();
            let t = t.trim();
            ok1(i64::from_str_radix(t, base as u32)
                .map(|v| num(v as f64))
                .unwrap_or_default())
        }),
    );
    env.set(
        "assert",
        func("assert", |a| {
            if arg(a, 0).truthy() {
                return Ok(a.to_vec());
            }
            let msg = arg(a, 1);
            Err(LuaError::new(if msg.is_nil() {
                s("assertion failed!")
            } else {
                msg
            }))
        }),
    );
    env.set("error", func("error", |a| Err(LuaError::new(arg(a, 0)))));
    env.set(
        "pcall",
        func("pcall", |a| {
            let f = arg(a, 0);
            match rt::call(&f, a.get(1..).unwrap_or(&[])) {
                Ok(mut r) => {
                    r.insert(0, Value::Bool(true));
                    Ok(r)
                }
                Err(e) => Ok(vec![Value::Bool(false), e.into_value()]),
            }
        }),
    );
    env.set(
        "xpcall",
        func("xpcall", |a| {
            let (f, handler) = (arg(a, 0), arg(a, 1));
            match rt::call(&f, &[]) {
                Ok(mut r) => {
                    r.insert(0, Value::Bool(true));
                    Ok(r)
                }
                Err(e) => {
                    let mut r = rt::call(&handler, &[e.into_value()])?;
                    r.insert(0, Value::Bool(false));
                    Ok(r)
                }
            }
        }),
    );
    env.set(
        "select",
        func("select", |a| {
            let n = arg(a, 0);
            if let Value::Str(st) = &n
                && st.as_bytes() == b"#"
            {
                return ok1(num((a.len() - 1) as f64));
            }
            let i = check_num(a, 0, "select")? as i64;
            let count = a.len() as i64 - 1;
            let start = if i < 0 { count + i } else { i - 1 };
            if start < 0 || i == 0 {
                return err("bad argument #1 to 'select' (index out of range)");
            }
            Ok(a.get((start + 1) as usize..)
                .map(|s| s.to_vec())
                .unwrap_or_default())
        }),
    );
    env.set(
        "unpack",
        func("unpack", |a| {
            let t = check_table(a, 0, "unpack")?;
            let i = opt_num(a, 1, "unpack", 1.0)? as i64;
            let j = match a.get(2) {
                None | Some(Value::Nil) => t.borrow().border() as i64,
                _ => check_num(a, 2, "unpack")? as i64,
            };
            let tb = t.borrow();
            Ok((i..=j).map(|k| tb.get_int(k)).collect())
        }),
    );
    env.set(
        "rawget",
        func("rawget", |a| {
            ok1(check_table(a, 0, "rawget")?.get(&arg(a, 1)))
        }),
    );
    env.set(
        "rawset",
        func("rawset", |a| {
            let t = check_table(a, 0, "rawset")?;
            t.set(arg(a, 1), arg(a, 2));
            ok1(Value::Table(t))
        }),
    );
    env.set(
        "rawequal",
        func("rawequal", |a| {
            ok1(Value::Bool(arg(a, 0).raw_eq(&arg(a, 1))))
        }),
    );
    env.set(
        "setmetatable",
        func("setmetatable", |a| {
            let t = check_table(a, 0, "setmetatable")?;
            t.borrow_mut().meta = match arg(a, 1) {
                Value::Table(m) => Some(m),
                Value::Nil => None,
                _ => return err("bad argument #2 to 'setmetatable' (nil or table expected)"),
            };
            ok1(Value::Table(t))
        }),
    );
    env.set(
        "getmetatable",
        func("getmetatable", |a| match arg(a, 0) {
            Value::Table(t) => ok1(t
                .borrow()
                .meta
                .clone()
                .map(Value::Table)
                .unwrap_or_default()),
            _ => ok1(Value::Nil),
        }),
    );
    let next = func("next", |a| {
        let t = check_table(a, 0, "next")?;
        match rt::next(&t, &arg(a, 1))? {
            Some((k, v)) => Ok(vec![k, v]),
            None => ok1(Value::Nil),
        }
    });
    env.set("next", next.clone());
    env.set(
        "pairs",
        func("pairs", move |a| {
            let t = check_table(a, 0, "pairs")?;
            Ok(vec![next.clone(), Value::Table(t), Value::Nil])
        }),
    );
    let ipairs_iter = func("ipairs_iter", |a| {
        let t = check_table(a, 0, "ipairs")?;
        let i = check_num(a, 1, "ipairs")? as i64 + 1;
        let v = t.borrow().get_int(i);
        if v.is_nil() {
            ok1(Value::Nil)
        } else {
            Ok(vec![num(i as f64), v])
        }
    });
    env.set(
        "ipairs",
        func("ipairs", move |a| {
            let t = check_table(a, 0, "ipairs")?;
            Ok(vec![ipairs_iter.clone(), Value::Table(t), num(0.0)])
        }),
    );
    env.set(
        "print",
        func("print", |a| {
            let parts: Vec<String> = a.iter().map(|v| rt::tostring(v).to_string()).collect();
            eprintln!("{}", parts.join("\t"));
            Ok(vec![])
        }),
    );
    env.set("collectgarbage", func("collectgarbage", |_| ok1(num(0.0))));
    let package = TableRef::new();
    package.set_str("loaded", Value::Table(TableRef::new()));
    env.set("package", Value::Table(package.clone()));
    env.set(
        "require",
        func("require", move |a| {
            let name = check_str(a, 0, "require")?;
            let loaded = package.get_str("loaded");
            let cached = rt::index(&loaded, &Value::Str(name.clone()))?;
            if !cached.is_nil() {
                return ok1(cached);
            }
            let env = e
                .upgrade()
                .ok_or_else(|| LuaError::msg("environment dropped"))?;
            let loader = env.loader.borrow();
            let Some(load) = loader.as_ref() else {
                return err(format!("module '{name}' not found"));
            };
            let v = load(&env, &name.to_string())?;
            let v = if v.is_nil() { Value::Bool(true) } else { v };
            rt::set_index(&loaded, Value::Str(name), v.clone())?;
            ok1(v)
        }),
    );
}

fn install_math(env: &Env) {
    let f1 = |name: &'static str, f: fn(f64) -> f64| {
        (
            name,
            func(name, move |a| ok1(num(f(check_num(a, 0, name)?)))),
        )
    };
    lib(
        env,
        "math",
        vec![
            ("huge", num(f64::INFINITY)),
            ("pi", num(std::f64::consts::PI)),
            f1("floor", f64::floor),
            f1("ceil", f64::ceil),
            f1("abs", f64::abs),
            f1("sqrt", f64::sqrt),
            f1("sin", f64::sin),
            f1("cos", f64::cos),
            f1("tan", f64::tan),
            f1("asin", f64::asin),
            f1("acos", f64::acos),
            f1("atan", f64::atan),
            f1("exp", f64::exp),
            f1("log10", f64::log10),
            f1("rad", f64::to_radians),
            f1("deg", f64::to_degrees),
            (
                "log",
                func("log", |a| {
                    let x = check_num(a, 0, "log")?;
                    match a.get(1) {
                        None | Some(Value::Nil) => ok1(num(x.ln())),
                        _ => ok1(num(x.ln() / check_num(a, 1, "log")?.ln())),
                    }
                }),
            ),
            (
                "pow",
                func("pow", |a| {
                    ok1(num(check_num(a, 0, "pow")?.powf(check_num(a, 1, "pow")?)))
                }),
            ),
            (
                "fmod",
                func("fmod", |a| {
                    ok1(num(check_num(a, 0, "fmod")? % check_num(a, 1, "fmod")?))
                }),
            ),
            (
                "modf",
                func("modf", |a| {
                    let x = check_num(a, 0, "modf")?;
                    let i = if x.is_infinite() { x } else { x.trunc() };
                    let f = if x.is_infinite() { 0.0 } else { x - i };
                    Ok(vec![num(i), num(f)])
                }),
            ),
            (
                "max",
                func("max", |a| {
                    let mut m = check_num(a, 0, "max")?;
                    for i in 1..a.len() {
                        let x = check_num(a, i, "max")?;
                        if x > m {
                            m = x;
                        }
                    }
                    ok1(num(m))
                }),
            ),
            (
                "min",
                func("min", |a| {
                    let mut m = check_num(a, 0, "min")?;
                    for i in 1..a.len() {
                        let x = check_num(a, i, "min")?;
                        if x < m {
                            m = x;
                        }
                    }
                    ok1(num(m))
                }),
            ),
            (
                "atan2",
                func("atan2", |a| {
                    ok1(num(
                        check_num(a, 0, "atan2")?.atan2(check_num(a, 1, "atan2")?)
                    ))
                }),
            ),
            (
                "random",
                func("random", |a| {
                    // Deterministic: calcs must not depend on it
                    match a.len() {
                        0 => ok1(num(0.5)),
                        1 => ok1(num(1.0)),
                        _ => ok1(num(check_num(a, 0, "random")?)),
                    }
                }),
            ),
            ("randomseed", func("randomseed", |_| Ok(vec![]))),
        ],
    );
}

fn install_string(env: &Env) {
    let t = lib(
        env,
        "string",
        vec![
            (
                "len",
                func("len", |a| ok1(num(check_str(a, 0, "len")?.len() as f64))),
            ),
            (
                "lower",
                func("lower", |a| {
                    ok1(s(crate::lower(check_str(a, 0, "lower")?.as_bytes())))
                }),
            ),
            (
                "upper",
                func("upper", |a| {
                    ok1(s(crate::upper(check_str(a, 0, "upper")?.as_bytes())))
                }),
            ),
            (
                "sub",
                func("sub", |a| {
                    let st = check_str(a, 0, "sub")?;
                    let len = st.len() as i64;
                    let mut i = opt_num(a, 1, "sub", 1.0)? as i64;
                    let mut j = opt_num(a, 2, "sub", -1.0)? as i64;
                    if i < 0 {
                        i = (len + i + 1).max(0);
                    }
                    if j < 0 {
                        j = len + j + 1;
                    }
                    if i < 1 {
                        i = 1;
                    }
                    if j > len {
                        j = len;
                    }
                    if i > j {
                        return ok1(s(""));
                    }
                    ok1(s(&st.as_bytes()[i as usize - 1..j as usize]))
                }),
            ),
            (
                "rep",
                func("rep", |a| {
                    let st = check_str(a, 0, "rep")?;
                    let n = check_num(a, 1, "rep")?;
                    if n <= 0.0 {
                        return ok1(s(""));
                    }
                    ok1(s(st.as_bytes().repeat(n as usize)))
                }),
            ),
            (
                "reverse",
                func("reverse", |a| {
                    let mut b = check_str(a, 0, "reverse")?.as_bytes().to_vec();
                    b.reverse();
                    ok1(s(b))
                }),
            ),
            (
                "byte",
                func("byte", |a| {
                    let st = check_str(a, 0, "byte")?;
                    let len = st.len() as i64;
                    let mut i = opt_num(a, 1, "byte", 1.0)? as i64;
                    let mut j = match a.get(2) {
                        None | Some(Value::Nil) => i,
                        _ => check_num(a, 2, "byte")? as i64,
                    };
                    if i < 0 {
                        i = len + i + 1;
                    }
                    if j < 0 {
                        j = len + j + 1;
                    }
                    if i < 1 {
                        i = 1;
                    }
                    if j > len {
                        j = len;
                    }
                    Ok((i..=j)
                        .map(|k| num(st.as_bytes()[k as usize - 1] as f64))
                        .collect())
                }),
            ),
            (
                "char",
                func("char", |a| {
                    let mut out = Vec::new();
                    for i in 0..a.len() {
                        out.push(check_num(a, i, "char")? as u8);
                    }
                    ok1(s(out))
                }),
            ),
            (
                "find",
                func("find", |a| {
                    let st = check_str(a, 0, "find")?;
                    let p = check_str(a, 1, "find")?;
                    let init = opt_num(a, 2, "find", 1.0)? as i64;
                    match pattern::lua_find(st.as_bytes(), p.as_bytes(), init, arg(a, 3).truthy())
                        .map_err(pattern_err)?
                    {
                        Some((x, y, caps)) => {
                            let mut r = vec![num(x as f64), num(y as f64)];
                            r.extend(caps);
                            Ok(r)
                        }
                        None => ok1(Value::Nil),
                    }
                }),
            ),
            (
                "match",
                func("match", |a| {
                    let st = check_str(a, 0, "match")?;
                    let p = check_str(a, 1, "match")?;
                    let init = opt_num(a, 2, "match", 1.0)? as i64;
                    match pattern::lua_match(st.as_bytes(), p.as_bytes(), init)
                        .map_err(pattern_err)?
                    {
                        Some(caps) => Ok(caps),
                        None => ok1(Value::Nil),
                    }
                }),
            ),
            (
                "gmatch",
                func("gmatch", |a| {
                    let st = check_str(a, 0, "gmatch")?;
                    let p = check_str(a, 1, "gmatch")?;
                    let all =
                        pattern::lua_gmatch(st.as_bytes(), p.as_bytes()).map_err(pattern_err)?;
                    let state = Rc::new(RefCell::new(all.into_iter()));
                    ok1(func("gmatch_iter", move |_| {
                        match state.borrow_mut().next() {
                            Some(caps) => Ok(caps),
                            None => ok1(Value::Nil),
                        }
                    }))
                }),
            ),
            (
                "gsub",
                func("gsub", |a| {
                    let st = check_str(a, 0, "gsub")?;
                    let p = check_str(a, 1, "gsub")?;
                    let max_n = match a.get(3) {
                        None | Some(Value::Nil) => None,
                        _ => Some(check_num(a, 3, "gsub")? as usize),
                    };
                    let repl = arg(a, 2);
                    let mut call_err = None;
                    let result = match &repl {
                        Value::Str(_) | Value::Number(_) => {
                            let r = crate::to_string(&repl);
                            pattern::lua_gsub(
                                st.as_bytes(),
                                p.as_bytes(),
                                Repl::Str(r.as_bytes()),
                                max_n,
                            )
                        }
                        Value::Table(t) => {
                            let t = t.clone();
                            let mut f = |caps: &[Value]| t.get(&caps[0]);
                            pattern::lua_gsub(
                                st.as_bytes(),
                                p.as_bytes(),
                                Repl::Func(&mut f),
                                max_n,
                            )
                        }
                        Value::Function(_) => {
                            let mut f = |caps: &[Value]| match rt::call(&repl, caps) {
                                Ok(r) => first(r),
                                Err(e) => {
                                    call_err.get_or_insert(e);
                                    Value::Nil
                                }
                            };
                            pattern::lua_gsub(
                                st.as_bytes(),
                                p.as_bytes(),
                                Repl::Func(&mut f),
                                max_n,
                            )
                        }
                        _ => {
                            return err(
                                "bad argument #3 to 'gsub' (string/function/table expected)",
                            );
                        }
                    };
                    if let Some(e) = call_err {
                        return Err(e);
                    }
                    let (out, n) = result.map_err(pattern_err)?;
                    Ok(vec![s(out), num(n as f64)])
                }),
            ),
            ("format", func("format", string_format)),
        ],
    );
    rt::set_string_lib(t);
}

fn pattern_err(e: pattern::PatternError) -> LuaError {
    LuaError::msg(format!("malformed pattern: {e:?}"))
}

/// `string.format` with C printf semantics for the conversions Lua allows.
pub fn string_format(a: &[Value]) -> LResult<Vec<Value>> {
    let f = check_str(a, 0, "format")?;
    let f = f.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut ai = 1;
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
        let flags = std::str::from_utf8(&f[flags_start..i]).unwrap_or("");
        let w_start = i;
        while i < f.len() && f[i].is_ascii_digit() {
            i += 1;
        }
        let width: Option<usize> = std::str::from_utf8(&f[w_start..i])
            .ok()
            .and_then(|w| w.parse().ok());
        let mut prec: Option<usize> = None;
        if f.get(i) == Some(&b'.') {
            i += 1;
            let p_start = i;
            while i < f.len() && f[i].is_ascii_digit() {
                i += 1;
            }
            prec = Some(
                std::str::from_utf8(&f[p_start..i])
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0),
            );
        }
        let Some(&conv) = f.get(i) else {
            return err("invalid option '%' to 'format'");
        };
        i += 1;
        let idx = ai;
        ai += 1;
        let body: Vec<u8> = match conv {
            b'd' | b'i' => {
                let x = check_num(a, idx, "format")?;
                let v = x as i64;
                let mut b = v.abs().to_string();
                if let Some(p) = prec
                    && b.len() < p
                {
                    b = "0".repeat(p - b.len()) + &b;
                }
                let sign = if v < 0 {
                    "-"
                } else if flags.contains('+') {
                    "+"
                } else if flags.contains(' ') {
                    " "
                } else {
                    ""
                };
                pad_num(sign, &b, width, flags, prec.is_none()).into_bytes()
            }
            b'f' | b'F' | b'g' | b'G' | b'e' | b'E' => {
                let x = check_num(a, idx, "format")?;
                let p = prec.unwrap_or(6);
                let mut b = match conv {
                    b'f' | b'F' => crate::format::format_f(x.abs(), p),
                    b'e' | b'E' => crate::format::format_e(x.abs(), p),
                    _ => crate::format::format_g_flags(x.abs(), p, flags.contains('#')),
                };
                if conv.is_ascii_uppercase() {
                    b = b.to_uppercase();
                }
                let neg = x.is_sign_negative() && !x.is_nan();
                let sign = if neg {
                    "-"
                } else if flags.contains('+') {
                    "+"
                } else if flags.contains(' ') {
                    " "
                } else {
                    ""
                };
                pad_num(sign, &b, width, flags, x.is_finite()).into_bytes()
            }
            b'x' | b'X' | b'o' => {
                let x = check_num(a, idx, "format")? as i64 as u64;
                let b = match conv {
                    b'x' => format!("{x:x}"),
                    b'X' => format!("{x:X}"),
                    _ => format!("{x:o}"),
                };
                pad_num("", &b, width, flags, true).into_bytes()
            }
            b'c' => vec![check_num(a, idx, "format")? as u8],
            b's' => {
                let v = arg(a, idx);
                if a.len() <= idx {
                    return err(format!(
                        "bad argument #{} to 'format' (string expected, got no value)",
                        idx + 1
                    ));
                }
                let mut b = rt::tostring(&v).as_bytes().to_vec();
                if let Some(p) = prec {
                    b.truncate(p);
                }
                pad_bytes(b, width, flags.contains('-'))
            }
            b'q' => {
                let st = check_str(a, idx, "format")?;
                let mut b = vec![b'"'];
                for &c in st.as_bytes() {
                    match c {
                        b'"' => b.extend_from_slice(b"\\\""),
                        b'\\' => b.extend_from_slice(b"\\\\"),
                        b'\n' => b.extend_from_slice(b"\\\n"),
                        b'\r' => b.extend_from_slice(b"\\r"),
                        0 => b.extend_from_slice(b"\\0"),
                        c => b.push(c),
                    }
                }
                b.push(b'"');
                b
            }
            c => return err(format!("invalid option '%{}' to 'format'", c as char)),
        };
        out.extend_from_slice(&body);
    }
    ok1(s(out))
}

fn pad_bytes(b: Vec<u8>, width: Option<usize>, left: bool) -> Vec<u8> {
    match width {
        Some(w) if b.len() < w => {
            let pad = vec![b' '; w - b.len()];
            if left {
                [b, pad].concat()
            } else {
                [pad, b].concat()
            }
        }
        _ => b,
    }
}

fn pad_num(sign: &str, digits: &str, width: Option<usize>, flags: &str, zero_ok: bool) -> String {
    let body_len = sign.len() + digits.len();
    match width {
        Some(w) if body_len < w => {
            if flags.contains('-') {
                format!("{sign}{digits}{}", " ".repeat(w - body_len))
            } else if flags.contains('0') && zero_ok {
                format!("{sign}{}{digits}", "0".repeat(w - body_len))
            } else {
                format!("{}{sign}{digits}", " ".repeat(w - body_len))
            }
        }
        _ => format!("{sign}{digits}"),
    }
}

fn install_table(env: &Env) {
    lib(
        env,
        "table",
        vec![
            (
                "insert",
                func("insert", |a| {
                    let t = check_table(a, 0, "insert")?;
                    match a.len() {
                        2 => {
                            let n = t.borrow().border();
                            t.borrow_mut().set(num((n + 1) as f64), arg(a, 1));
                        }
                        3 => {
                            let n = t.borrow().border() as i64;
                            let pos = check_num(a, 1, "insert")? as i64;
                            // Shift up, as Lua 5.1 does with raw gets and sets
                            let mut i = n + 1;
                            while i > pos {
                                let v = t.borrow().get_int(i - 1);
                                t.borrow_mut().set(num(i as f64), v);
                                i -= 1;
                            }
                            t.borrow_mut().set(num(pos as f64), arg(a, 2));
                        }
                        _ => return err("wrong number of arguments to 'insert'"),
                    }
                    Ok(vec![])
                }),
            ),
            (
                "remove",
                func("remove", |a| {
                    let t = check_table(a, 0, "remove")?;
                    let n = t.borrow().border() as i64;
                    let pos = opt_num(a, 1, "remove", n as f64)? as i64;
                    if n == 0 {
                        return Ok(vec![]);
                    }
                    let v = t.borrow().get_int(pos);
                    let mut i = pos;
                    while i < n {
                        let next = t.borrow().get_int(i + 1);
                        t.borrow_mut().set(num(i as f64), next);
                        i += 1;
                    }
                    t.borrow_mut().set(num(n as f64), Value::Nil);
                    ok1(v)
                }),
            ),
            (
                "concat",
                func("concat", |a| {
                    let t = check_table(a, 0, "concat")?;
                    let sep = match a.get(1) {
                        None | Some(Value::Nil) => LuaStr::from(""),
                        _ => check_str(a, 1, "concat")?,
                    };
                    let i = opt_num(a, 2, "concat", 1.0)? as i64;
                    let j = match a.get(3) {
                        None | Some(Value::Nil) => t.borrow().border() as i64,
                        _ => check_num(a, 3, "concat")? as i64,
                    };
                    let mut out = Vec::new();
                    for k in i..=j {
                        let v = t.borrow().get_int(k);
                        match &v {
                            Value::Str(_) | Value::Number(_) => {
                                out.extend_from_slice(crate::to_string(&v).as_bytes())
                            }
                            v => {
                                return err(format!(
                                    "invalid value (at index {k}) in table for 'concat' ({} found)",
                                    v.type_name()
                                ));
                            }
                        }
                        if k < j {
                            out.extend_from_slice(sep.as_bytes());
                        }
                    }
                    ok1(s(out))
                }),
            ),
            (
                "sort",
                func("sort", |a| {
                    let t = check_table(a, 0, "sort")?;
                    let cmp = arg(a, 1);
                    sort(&t, &cmp)?;
                    Ok(vec![])
                }),
            ),
            (
                "maxn",
                func("maxn", |a| {
                    ok1(num(check_table(a, 0, "maxn")?.borrow().border() as f64))
                }),
            ),
        ],
    );
    // LuaJIT keeps unpack in table too
    let unpack = env.global("unpack");
    if let Value::Table(t) = env.global("table") {
        t.set_str("unpack", unpack);
    }
}

/// `table.sort`: Lua 5.1's `auxsort` (which LuaJIT keeps), so elements that
/// compare equal end up in the same order as upstream.
pub fn sort(t: &TableRef, cmp: &Value) -> LResult<()> {
    let n = t.borrow().border();
    let less = |a: &Value, b: &Value| -> LResult<bool> {
        if cmp.is_nil() {
            rt::lt(a, b)
        } else {
            Ok(first(rt::call(cmp, &[a.clone(), b.clone()])?).truthy())
        }
    };
    let get = |i: usize| t.borrow().get_int(i as i64);
    let set = |i: usize, v: Value| t.borrow_mut().set(Value::Number(i as f64), v);
    // Iterative version of auxsort(l, u) with the same comparisons and swaps
    let mut stack = vec![(1usize, n)];
    while let Some((mut l, mut u)) = stack.pop() {
        while l < u {
            // sort elements a[l], a[(l+u)/2] and a[u]
            let (al, au) = (get(l), get(u));
            if less(&au, &al)? {
                set(l, au);
                set(u, al);
            }
            if u - l == 1 {
                break;
            }
            let mut i = (l + u) / 2;
            let (ai, al) = (get(i), get(l));
            if less(&ai, &al)? {
                set(i, al);
                set(l, ai);
            } else {
                let (ai, au) = (get(i), get(u));
                if less(&au, &ai)? {
                    set(i, au);
                    set(u, ai);
                }
            }
            if u - l == 2 {
                break;
            }
            let p = get(i);
            // swap a[i] and a[u-1]
            let au1 = get(u - 1);
            set(i, au1);
            set(u - 1, p.clone());
            // a[l] <= P == a[u-1] <= a[u], only need to sort from l+1 to u-2
            i = l;
            let mut j = u - 1;
            loop {
                // repeat ++i until a[i] >= P
                loop {
                    i += 1;
                    let ai = get(i);
                    if !less(&ai, &p)? {
                        break;
                    }
                    if i > u {
                        return err("invalid order function for sorting");
                    }
                }
                // repeat --j until a[j] <= P
                loop {
                    if j == 0 {
                        return err("invalid order function for sorting");
                    }
                    j -= 1;
                    let aj = get(j);
                    if !less(&p, &aj)? {
                        break;
                    }
                    if j < l {
                        return err("invalid order function for sorting");
                    }
                }
                if j < i {
                    break;
                }
                let (ai, aj) = (get(i), get(j));
                set(i, aj);
                set(j, ai);
            }
            // swap a[u-1] and a[i]
            let (au1, ai) = (get(u - 1), get(i));
            set(u - 1, ai);
            set(i, au1);
            // a[l..i-1] <= a[i] == P <= a[i+1..u]; recurse into the smaller part
            if i - l < u - i {
                stack.push((i + 1, u));
                u = i - 1;
            } else {
                stack.push((l, i - 1));
                l = i + 1;
            }
            // l and u now delimit the smaller part, sorted next iteration
            let _ = &mut l;
            let _ = &mut u;
        }
    }
    Ok(())
}

fn tobit(v: f64) -> i32 {
    // LuaJIT: the number modulo 2^32 as a signed 32 bit integer
    (v.rem_euclid(4294967296.0) as u64 as u32) as i32
}

fn install_bit(env: &Env) {
    let fold = |name: &'static str, f: fn(i32, i32) -> i32| {
        (
            name,
            func(name, move |a| {
                let mut acc = tobit(check_num(a, 0, name)?);
                for i in 1..a.len() {
                    acc = f(acc, tobit(check_num(a, i, name)?));
                }
                ok1(num(acc as f64))
            }),
        )
    };
    let shift = |name: &'static str, f: fn(i32, u32) -> i32| {
        (
            name,
            func(name, move |a| {
                let x = tobit(check_num(a, 0, name)?);
                let n = (tobit(check_num(a, 1, name)?) & 31) as u32;
                ok1(num(f(x, n) as f64))
            }),
        )
    };
    lib(
        env,
        "bit",
        vec![
            fold("band", |a, b| a & b),
            fold("bor", |a, b| a | b),
            fold("bxor", |a, b| a ^ b),
            (
                "bnot",
                func("bnot", |a| {
                    ok1(num(!tobit(check_num(a, 0, "bnot")?) as f64))
                }),
            ),
            (
                "tobit",
                func("tobit", |a| {
                    ok1(num(tobit(check_num(a, 0, "tobit")?) as f64))
                }),
            ),
            shift("lshift", |x, n| ((x as u32) << n) as i32),
            shift("rshift", |x, n| ((x as u32) >> n) as i32),
            shift("arshift", |x, n| x >> n),
            shift("rol", |x, n| (x as u32).rotate_left(n) as i32),
            shift("ror", |x, n| (x as u32).rotate_right(n) as i32),
            (
                "tohex",
                func("tohex", |a| {
                    let x = tobit(check_num(a, 0, "tohex")?) as u32;
                    ok1(s(format!("{x:08x}")))
                }),
            ),
        ],
    );
}

fn install_os(env: &Env) {
    lib(
        env,
        "os",
        vec![
            // Fixed, so results never depend on the clock
            ("time", func("time", |_| ok1(num(0.0)))),
            ("clock", func("clock", |_| ok1(num(0.0)))),
            ("date", func("date", |_| ok1(s("")))),
            ("getenv", func("getenv", |_| ok1(Value::Nil))),
            (
                "remove",
                func("remove", |_| Ok(vec![Value::Nil, s("not supported")])),
            ),
            (
                "rename",
                func("rename", |_| Ok(vec![Value::Nil, s("not supported")])),
            ),
        ],
    );
    lib(
        env,
        "io",
        vec![
            (
                "open",
                func("open", |_| {
                    Ok(vec![Value::Nil, s("file access is not supported")])
                }),
            ),
            (
                "lines",
                func("lines", |_| err("file access is not supported")),
            ),
        ],
    );
}

fn install_misc(env: &Env) {
    // Calcs never run in a coroutine: coroutine.running() is nil, so upstream
    // never yields; resume runs the function to completion
    lib(
        env,
        "coroutine",
        vec![
            ("running", func("running", |_| ok1(Value::Nil))),
            (
                "create",
                func("create", |a| {
                    let co = TableRef::new();
                    co.set_str("fn", arg(a, 0));
                    ok1(Value::Table(co))
                }),
            ),
            (
                "resume",
                func("resume", |a| {
                    let co = check_table(a, 0, "resume")?;
                    let f = co.get_str("fn");
                    if f.is_nil() {
                        return Ok(vec![Value::Bool(false), s("cannot resume dead coroutine")]);
                    }
                    co.set_str("fn", Value::Nil);
                    match rt::call(&f, a.get(1..).unwrap_or(&[])) {
                        Ok(mut r) => {
                            r.insert(0, Value::Bool(true));
                            Ok(r)
                        }
                        Err(e) => Ok(vec![Value::Bool(false), e.into_value()]),
                    }
                }),
            ),
            (
                "status",
                func("status", |a| {
                    let co = check_table(a, 0, "status")?;
                    ok1(s(if co.get_str("fn").is_nil() {
                        "dead"
                    } else {
                        "suspended"
                    }))
                }),
            ),
            (
                "yield",
                func("yield", |_| {
                    err("attempt to yield from outside a coroutine")
                }),
            ),
        ],
    );
    // The oracle's LuaJIT 2.1 on x64, which libraries probe
    lib(
        env,
        "jit",
        vec![
            ("off", func("off", |_| Ok(vec![]))),
            ("on", func("on", |_| Ok(vec![]))),
            ("version", s("LuaJIT 2.1.0-beta3")),
            ("version_num", num(20100.0)),
            ("arch", s("x64")),
            ("os", s("Linux")),
        ],
    );
    lib(
        env,
        "debug",
        vec![("traceback", func("traceback", |a| ok1(arg(a, 0))))],
    );
}

/// A table for code that builds tables directly.
pub fn table_from(entries: Vec<(Value, Value)>) -> TableRef {
    let mut t = Table::new();
    for (k, v) in entries {
        t.set(k, v);
    }
    TableRef::from_table(t)
}
