//! Evaluates the top level of upstream data files: locals, assignments,
//! table constructors, arithmetic and calls to native helpers. Functions are
//! not run; they become [`FuncRef`]s naming where they come from, so the
//! engine can map each to its Rust port. Anything else fails loudly.

use lua::ast::*;
use lua::value::{FuncRef, Key, LuaStr, TableRef, Value};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub struct EvalError {
    pub file: String,
    pub line: u32,
    pub msg: String,
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.msg)
    }
}

impl std::error::Error for EvalError {}

pub type Result<T> = std::result::Result<T, EvalError>;

/// A helper the data files call, such as `mod(...)` in skill files.
pub type Native = Rc<dyn Fn(&[Value]) -> std::result::Result<Vec<Value>, String>>;

/// Natives are passed around as function values whose file is `<native>`.
const NATIVE_FILE: &str = "<native>";

/// Evaluation environment shared by the files of one import.
#[derive(Default, Clone)]
pub struct Env {
    pub globals: HashMap<String, Value>,
    natives: Vec<Native>,
}

impl Env {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a native and returns the value that calls it.
    pub fn native(
        &mut self,
        name: &str,
        f: impl Fn(&[Value]) -> std::result::Result<Vec<Value>, String> + 'static,
    ) -> Value {
        self.natives.push(Rc::new(f));
        Value::Func(Rc::new(FuncRef {
            file: NATIVE_FILE.into(),
            line: self.natives.len() as u32 - 1,
            offset: 0,
            key: name.into(),
            source_hash: 0,
        }))
    }

    /// Registers a native as a global.
    pub fn set_native(
        &mut self,
        name: &str,
        f: impl Fn(&[Value]) -> std::result::Result<Vec<Value>, String> + 'static,
    ) {
        let v = self.native(name, f);
        self.globals.insert(name.to_string(), v);
    }

    /// Evaluates a file (path relative to upstream's `src/`, used in function
    /// references) with the given `...` arguments; returns what it returns.
    pub fn eval_file(&mut self, file: &str, src: &[u8], args: Vec<Value>) -> Result<Vec<Value>> {
        let block = lua::parser::parse(src).map_err(|e| EvalError {
            file: file.into(),
            line: e.line,
            msg: e.msg,
        })?;
        let mut ev = Evaluator {
            env: self,
            file: Rc::from(file),
            src,
            scopes: vec![HashMap::new()],
            varargs: args,
            line: 0,
        };
        ev.block(&block)
    }

    /// Evaluates only the `local name = <expr>` statements named in `names`,
    /// with `locals` predefined; for modules that mix data with code, such as
    /// ModParser. Returns the values by name.
    pub fn eval_locals(
        &mut self,
        file: &str,
        src: &[u8],
        names: &[&str],
        locals: HashMap<String, Value>,
    ) -> Result<HashMap<String, Value>> {
        let block = lua::parser::parse(src).map_err(|e| EvalError {
            file: file.into(),
            line: e.line,
            msg: e.msg,
        })?;
        let mut ev = Evaluator {
            env: self,
            file: Rc::from(file),
            src,
            scopes: vec![locals],
            varargs: vec![],
            line: 0,
        };
        let mut out = HashMap::new();
        for (stat, line) in block.stats.iter().zip(&block.lines) {
            ev.line = *line;
            if let Stat::Local {
                names: lnames,
                values,
            } = stat
                && lnames.len() == 1
                && values.len() == 1
                && names.contains(&lnames[0].as_str())
            {
                let v = ev.expr_path(&values[0], &lnames[0])?;
                ev.scopes[0].insert(lnames[0].clone(), v.clone());
                out.insert(lnames[0].clone(), v);
            }
        }
        for n in names {
            if !out.contains_key(*n) {
                return Err(EvalError {
                    file: file.into(),
                    line: 0,
                    msg: format!("local {n} not found"),
                });
            }
        }
        Ok(out)
    }

    /// Evaluates only the assignments to the given targets (written as
    /// dotted names, e.g. `data.keystones`), in file order; for modules that
    /// mix data with code, such as `Modules/Data.lua`.
    pub fn eval_assigns(&mut self, file: &str, src: &[u8], targets: &[&str]) -> Result<()> {
        let block = lua::parser::parse(src).map_err(|e| EvalError {
            file: file.into(),
            line: e.line,
            msg: e.msg,
        })?;
        let mut ev = Evaluator {
            env: self,
            file: Rc::from(file),
            src,
            scopes: vec![HashMap::new()],
            varargs: vec![],
            line: 0,
        };
        let mut found = 0;
        for (stat, line) in block.stats.iter().zip(&block.lines) {
            ev.line = *line;
            if let Stat::Assign { targets: t, .. } = stat
                && t.len() == 1
                && let Some(name) = dotted_name(&t[0])
                && targets.contains(&name.as_str())
            {
                ev.stat(stat)?;
                found += 1;
            }
        }
        if found < targets.len() {
            return Err(EvalError {
                file: file.into(),
                line: 0,
                msg: format!("not all of {targets:?} assigned"),
            });
        }
        Ok(())
    }

    fn call_native(&self, f: &FuncRef, args: &[Value]) -> std::result::Result<Vec<Value>, String> {
        let native = self.natives.get(f.line as usize).ok_or("unknown native")?;
        native(args)
    }
}

struct Evaluator<'a> {
    env: &'a mut Env,
    file: Rc<str>,
    src: &'a [u8],
    scopes: Vec<HashMap<String, Value>>,
    varargs: Vec<Value>,
    line: u32,
}

impl Evaluator<'_> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T> {
        Err(EvalError {
            file: self.file.to_string(),
            line: self.line,
            msg: msg.into(),
        })
    }

    fn lookup(&self, name: &str) -> Value {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return v.clone();
            }
        }
        self.env.globals.get(name).cloned().unwrap_or_default()
    }

    fn assign_name(&mut self, name: &str, v: Value) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = v;
                return;
            }
        }
        self.env.globals.insert(name.to_string(), v);
    }

    fn block(&mut self, b: &Block) -> Result<Vec<Value>> {
        self.scopes.push(HashMap::new());
        for (s, line) in b.stats.iter().zip(&b.lines) {
            self.line = *line;
            self.stat(s)?;
        }
        let ret = match &b.last {
            Some(LastStat::Return(exprs)) => self.exprlist(exprs, "")?,
            Some(LastStat::Break) => return self.err("break outside a loop"),
            None => vec![],
        };
        self.scopes.pop();
        Ok(ret)
    }

    fn stat(&mut self, s: &Stat) -> Result<()> {
        match s {
            Stat::Local { names, values } => {
                let vals =
                    self.exprlist(values, names.first().map(String::as_str).unwrap_or(""))?;
                for (i, n) in names.iter().enumerate() {
                    let v = vals.get(i).cloned().unwrap_or_default();
                    self.scopes.last_mut().expect("scope").insert(n.clone(), v);
                }
                Ok(())
            }
            Stat::Assign { targets, values } => {
                let path = targets.first().map(expr_path_name).unwrap_or_default();
                let vals = self.exprlist(values, &path)?;
                for (i, t) in targets.iter().enumerate() {
                    let v = vals.get(i).cloned().unwrap_or_default();
                    match t {
                        Expr::Name(n) => self.assign_name(n, v),
                        Expr::Index(obj, key) => {
                            let obj = self.expr1(obj, "")?;
                            let key = self.expr1(key, "")?;
                            match obj {
                                Value::Table(t) => {
                                    if Key::from_value(&key).is_none() {
                                        return self.err("table index is nil or NaN");
                                    }
                                    t.set(key, v)
                                }
                                _ => {
                                    return self.err(format!(
                                        "attempt to index a {} value",
                                        obj.type_name()
                                    ));
                                }
                            }
                        }
                        _ => return self.err("cannot assign"),
                    }
                }
                Ok(())
            }
            Stat::Call(e) => {
                self.expr(e, "")?;
                Ok(())
            }
            Stat::Do(b) => {
                self.block(b)?;
                Ok(())
            }
            Stat::Function { path, method, func } => {
                let mut name = path.join(".");
                if let Some(m) = method {
                    name = format!("{name}:{m}");
                }
                let f = self.func_ref(func, &name);
                if path.len() == 1 && method.is_none() {
                    self.assign_name(&path[0], f);
                    return Ok(());
                }
                let mut obj = self.lookup(&path[0]);
                let last = method
                    .clone()
                    .unwrap_or_else(|| path[path.len() - 1].clone());
                let middle = if method.is_some() {
                    &path[1..]
                } else {
                    &path[1..path.len() - 1]
                };
                for p in middle {
                    obj = match &obj {
                        Value::Table(t) => t.get_str(p),
                        _ => return self.err(format!("attempt to index field '{p}'")),
                    };
                }
                match obj {
                    Value::Table(t) => t.set_str(&last, f),
                    _ => return self.err("attempt to index a non-table"),
                }
                Ok(())
            }
            Stat::LocalFunction { name, func } => {
                let f = self.func_ref(func, name);
                self.scopes
                    .last_mut()
                    .expect("scope")
                    .insert(name.clone(), f);
                Ok(())
            }
            _ => self.err(format!(
                "statement not supported in data files: {}",
                stat_kind(s)
            )),
        }
    }

    fn func_ref(&mut self, func: &FuncBody, key: &str) -> Value {
        self.line = func.line;
        let text = &self.src[func.span.0..func.span.1];
        Value::Func(Rc::new(FuncRef {
            file: self.file.clone(),
            line: func.line,
            offset: func.span.0 as u32,
            key: key.into(),
            source_hash: lua::value::fnv1a(text),
        }))
    }

    /// Evaluates expressions, expanding the last one if it returns several values.
    fn exprlist(&mut self, exprs: &[Expr], path: &str) -> Result<Vec<Value>> {
        let mut out = Vec::with_capacity(exprs.len());
        for (i, e) in exprs.iter().enumerate() {
            if i + 1 == exprs.len() {
                out.extend(self.expr(e, path)?);
            } else {
                out.push(self.expr1(e, path)?);
            }
        }
        Ok(out)
    }

    fn expr1(&mut self, e: &Expr, path: &str) -> Result<Value> {
        Ok(self.expr(e, path)?.into_iter().next().unwrap_or_default())
    }

    fn expr_path(&mut self, e: &Expr, path: &str) -> Result<Value> {
        self.expr1(e, path)
    }

    /// Evaluates an expression; `path` names where the value is stored, for
    /// function references.
    fn expr(&mut self, e: &Expr, path: &str) -> Result<Vec<Value>> {
        Ok(vec![match e {
            Expr::Nil => Value::Nil,
            Expr::True => Value::Bool(true),
            Expr::False => Value::Bool(false),
            Expr::Number(n) => Value::Number(*n),
            Expr::Str(s) => Value::Str(LuaStr::new(s)),
            Expr::Vararg => return Ok(self.varargs.clone()),
            Expr::Function(f) => self.func_ref(f, path),
            Expr::Table(fields) => Value::Table(self.table(fields, path)?),
            Expr::Name(n) => self.lookup(n),
            Expr::Paren(e) => self.expr1(e, path)?,
            Expr::Index(obj, key) => {
                let o = self.expr1(obj, "")?;
                let k = self.expr1(key, "")?;
                match o {
                    Value::Table(t) => t.get(&k),
                    Value::Str(_) => {
                        return self.err("string methods are not supported in data files");
                    }
                    _ => return self.err(format!("attempt to index a {} value", o.type_name())),
                }
            }
            Expr::Call { func, args } => {
                let f = self.expr1(func, "")?;
                let args = self.exprlist(args, path)?;
                return self.call(&f, &args);
            }
            Expr::Method { .. } => return self.err("method calls are not supported in data files"),
            Expr::Unary(op, e) => {
                let v = self.expr1(e, path)?;
                match op {
                    UnOp::Neg => match lua::to_number(&v) {
                        Some(n) => Value::Number(-n),
                        None => return self.err("attempt to perform arithmetic on a non-number"),
                    },
                    UnOp::Not => Value::Bool(!v.truthy()),
                    UnOp::Len => match v {
                        Value::Str(s) => Value::Number(s.len() as f64),
                        Value::Table(t) => Value::Number(t.len() as f64),
                        _ => return self.err("attempt to get length of a non-table"),
                    },
                }
            }
            Expr::Binary(op, l, r) => self.binary(*op, l, r)?,
        }])
    }

    fn call(&mut self, f: &Value, args: &[Value]) -> Result<Vec<Value>> {
        match f {
            Value::Func(r) if &*r.file == NATIVE_FILE => match self.env.call_native(r, args) {
                Ok(v) => Ok(v),
                Err(msg) => self.err(format!("{}: {msg}", r.key)),
            },
            Value::Func(r) => self.err(format!("cannot call Lua function {} from data", r.key)),
            _ => self.err(format!("attempt to call a {} value", f.type_name())),
        }
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr) -> Result<Value> {
        match op {
            BinOp::And => {
                let lv = self.expr1(l, "")?;
                return if lv.truthy() {
                    self.expr1(r, "")
                } else {
                    Ok(lv)
                };
            }
            BinOp::Or => {
                let lv = self.expr1(l, "")?;
                return if lv.truthy() {
                    Ok(lv)
                } else {
                    self.expr1(r, "")
                };
            }
            _ => {}
        }
        let a = self.expr1(l, "")?;
        let b = self.expr1(r, "")?;
        let num = |v: &Value| lua::to_number(v);
        Ok(match op {
            BinOp::Concat => {
                let s = |v: &Value| match v {
                    Value::Str(_) | Value::Number(_) => Some(lua::to_string(v)),
                    _ => None,
                };
                match (s(&a), s(&b)) {
                    (Some(x), Some(y)) => Value::Str([x.as_bytes(), y.as_bytes()].concat().into()),
                    _ => return self.err("attempt to concatenate a non-string value"),
                }
            }
            BinOp::Eq => Value::Bool(a.raw_eq(&b)),
            BinOp::Ne => Value::Bool(!a.raw_eq(&b)),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let ord = match (&a, &b) {
                    (Value::Number(x), Value::Number(y)) => x.partial_cmp(y),
                    (Value::Str(x), Value::Str(y)) => Some(x.as_bytes().cmp(y.as_bytes())),
                    _ => return self.err("attempt to compare incompatible values"),
                };
                use std::cmp::Ordering::*;
                Value::Bool(match op {
                    BinOp::Lt => ord == Some(Less),
                    BinOp::Le => matches!(ord, Some(Less | Equal)),
                    BinOp::Gt => ord == Some(Greater),
                    _ => matches!(ord, Some(Greater | Equal)),
                })
            }
            _ => {
                let (Some(x), Some(y)) = (num(&a), num(&b)) else {
                    return self.err("attempt to perform arithmetic on a non-number");
                };
                Value::Number(match op {
                    BinOp::Add => x + y,
                    BinOp::Sub => x - y,
                    BinOp::Mul => x * y,
                    BinOp::Div => x / y,
                    BinOp::Mod => x - (x / y).floor() * y,
                    BinOp::Pow => x.powf(y),
                    _ => unreachable!(),
                })
            }
        })
    }

    fn table(&mut self, fields: &[Field], path: &str) -> Result<TableRef> {
        let t = TableRef::new();
        let mut n = 0i64;
        for (i, f) in fields.iter().enumerate() {
            match f {
                Field::Keyed(k, v) => {
                    let key = self.expr1(k, "")?;
                    let sub = format!("{path}[{}]", key_label(&key));
                    let val = self.expr1(v, &sub)?;
                    if Key::from_value(&key).is_none() {
                        return self.err("table index is nil or NaN");
                    }
                    t.set(key, val);
                }
                Field::Item(e) => {
                    let sub = format!("{path}[{}]", n + 1);
                    if i + 1 == fields.len() {
                        for v in self.expr(e, &sub)? {
                            n += 1;
                            t.set(Value::Number(n as f64), v);
                        }
                    } else {
                        n += 1;
                        let v = self.expr1(e, &sub)?;
                        t.set(Value::Number(n as f64), v);
                    }
                }
            }
        }
        Ok(t)
    }
}

fn key_label(k: &Value) -> String {
    match k {
        Value::Str(s) => format!("{:?}", s.to_str_lossy()),
        Value::Number(n) => lua::value::format_number(*n),
        v => v.type_name().to_string(),
    }
}

/// `a.b.c` for a chain of names and constant string fields.
fn dotted_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Name(n) => Some(n.clone()),
        Expr::Index(obj, key) => match &**key {
            Expr::Str(s) => Some(format!(
                "{}.{}",
                dotted_name(obj)?,
                std::str::from_utf8(s).ok()?
            )),
            _ => None,
        },
        _ => None,
    }
}

/// Dotted name of an assignment target, for function references.
fn expr_path_name(e: &Expr) -> String {
    match e {
        Expr::Name(n) => n.clone(),
        Expr::Index(obj, key) => {
            let k = match &**key {
                Expr::Str(s) => format!("{:?}", String::from_utf8_lossy(s)),
                Expr::Number(n) => lua::value::format_number(*n),
                _ => "?".into(),
            };
            format!("{}[{k}]", expr_path_name(obj))
        }
        _ => "?".into(),
    }
}

fn stat_kind(s: &Stat) -> &'static str {
    match s {
        Stat::While { .. } => "while",
        Stat::Repeat { .. } => "repeat",
        Stat::If { .. } => "if",
        Stat::NumericFor { .. } | Stat::GenericFor { .. } => "for",
        Stat::Goto(_) => "goto",
        Stat::Label(_) => "label",
        _ => "statement",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_file() {
        let mut env = Env::new();
        let m = env.native("mod", |args| {
            let t = TableRef::new();
            t.set_str("name", args[0].clone());
            t.set_str("value", args[1].clone());
            Ok(vec![Value::Table(t)])
        });
        let src = br#"
local skills, mod = ...
local x = 2 * 3 + 1
skills["A"] = { name = "A" .. x, stats = { mod("Life", -5), "s" }, preDamageFunc = function(a) return a end }
return skills
"#;
        let skills = TableRef::new();
        let ret = env
            .eval_file("Data/Test.lua", src, vec![Value::Table(skills.clone()), m])
            .unwrap();
        assert!(ret[0].raw_eq(&Value::Table(skills.clone())));
        let a = skills.get_str("A");
        let a = a.as_table().unwrap();
        assert_eq!(a.get_str("name").as_str().unwrap().to_string(), "A7");
        let stats = a.get_str("stats");
        let m0 = stats.as_table().unwrap().get_int(1);
        assert_eq!(
            m0.as_table().unwrap().get_str("value").as_number(),
            Some(-5.0)
        );
        let Value::Func(f) = a.get_str("preDamageFunc") else {
            panic!()
        };
        assert_eq!(&*f.key, "skills[\"A\"][\"preDamageFunc\"]");
        assert_eq!(f.line, 4);
    }

    #[test]
    fn rejects_code() {
        let mut env = Env::new();
        let err = env
            .eval_file("x.lua", b"for i = 1, 2 do end", vec![])
            .unwrap_err();
        assert!(err.msg.contains("for"));
    }
}
