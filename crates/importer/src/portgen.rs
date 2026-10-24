//! Translates upstream Lua functions into Rust that mirrors them line by line
//! on dynamic [`lua::Value`]s, for the many small closures in ModParser's
//! tables. It handles a subset of Lua; functions outside it are ported by
//! hand. The generated code calls the helpers in `engine::rt` and the
//! ModParser helpers through `c` (`engine::mod_parser::Ctx`).

use lua::ast::*;
use std::collections::HashSet;
use std::fmt::Write;

#[derive(Debug)]
pub struct Unsupported(pub String);

type R<T> = std::result::Result<T, Unsupported>;

fn unsupported<T>(msg: impl Into<String>) -> R<T> {
    Err(Unsupported(msg.into()))
}

/// Names a closure may use from its enclosing module, and how they translate.
pub struct Upvalues {
    /// Helper functions, called as `c.<rust name>(args)`; each takes and
    /// returns `Vec<Value>`
    pub helpers: &'static [(&'static str, &'static str)],
    /// Module values read through `c.value("<name>")`
    pub values: &'static [&'static str],
}

/// The upvalues of ModParser's closures.
pub const MOD_PARSER: Upvalues = Upvalues {
    helpers: &[
        ("mod", "mod_"),
        ("flag", "flag"),
        ("firstToUpper", "first_to_upper"),
        ("explodeFunc", "explode_func"),
        ("triggerExtraSkill", "trigger_extra_skill"),
        ("grantedExtraSkill", "granted_extra_skill"),
        ("extraSupport", "extra_support"),
        ("dealNoNonDamageType", "deal_no_non_damage_type"),
        ("getSimpleConv", "get_simple_conv"),
        ("getPerStat", "get_per_stat"),
        ("getThreshold", "get_threshold"),
    ],
    values: &[
        "ModFlag",
        "KeywordFlag",
        "SkillType",
        "data",
        "damageTypeList",
        "conquerorList",
        "gemIdLookup",
    ],
};

pub struct Gen<'a> {
    up: &'a Upvalues,
    out: String,
    indent: usize,
    scopes: Vec<HashSet<String>>,
    tmp: usize,
}

/// Translates a function into the body of
/// `fn(c: &Ctx, a: &[Value]) -> Vec<Value>`.
pub fn translate(f: &FuncBody, up: &Upvalues) -> R<String> {
    if f.vararg {
        return unsupported("varargs");
    }
    let mut g = Gen {
        up,
        out: String::new(),
        indent: 1,
        scopes: vec![HashSet::new()],
        tmp: 0,
    };
    for (i, p) in f.params.iter().enumerate() {
        let v = g.declare(p);
        g.line(&format!("let mut {v} = arg(a, {i});"));
    }
    g.block_body(&f.body)?;
    if !matches!(f.body.last, Some(LastStat::Return(_))) {
        g.line("vec![]");
    }
    Ok(g.out)
}

/// Rust identifier for a Lua local.
fn ident(name: &str) -> String {
    format!("l_{name}")
}

fn str_lit(bytes: &[u8]) -> String {
    let mut s = String::from("sb(b\"");
    for &b in bytes {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7e => s.push(b as char),
            _ => {
                let _ = write!(s, "\\x{b:02x}");
            }
        }
    }
    s.push_str("\")");
    s
}

fn num_lit(x: f64) -> String {
    if x.is_infinite() {
        return if x > 0.0 {
            "n(f64::INFINITY)".into()
        } else {
            "n(f64::NEG_INFINITY)".into()
        };
    }
    let mut s = format!("{x:?}");
    if !s.contains(['.', 'e', 'E']) {
        s.push_str(".0");
    }
    format!("n({s})")
}

impl Gen<'_> {
    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn declare(&mut self, name: &str) -> String {
        self.scopes
            .last_mut()
            .expect("scope")
            .insert(name.to_string());
        ident(name)
    }

    fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    fn tmp(&mut self) -> String {
        self.tmp += 1;
        format!("t{}", self.tmp)
    }

    fn block_body(&mut self, b: &Block) -> R<()> {
        for s in &b.stats {
            self.stat(s)?;
        }
        match &b.last {
            Some(LastStat::Return(es)) => {
                let v = self.exprlist(es)?;
                self.line(&format!("return {v};"));
            }
            Some(LastStat::Break) => self.line("break;"),
            None => {}
        }
        Ok(())
    }

    fn block(&mut self, b: &Block) -> R<()> {
        self.scopes.push(HashSet::new());
        self.indent += 1;
        let r = self.block_body(b);
        self.indent -= 1;
        self.scopes.pop();
        r
    }

    fn stat(&mut self, s: &Stat) -> R<()> {
        match s {
            Stat::Local { names, values } => {
                if names.len() == 1 && values.len() <= 1 {
                    let v = match values.first() {
                        Some(e) => self.expr(e)?,
                        None => "nil()".into(),
                    };
                    let id = self.declare(&names[0]);
                    self.line(&format!("let mut {id} = {v};"));
                } else {
                    let vals = self.exprlist(values)?;
                    let t = self.tmp();
                    self.line(&format!("let {t} = {vals};"));
                    for (i, n) in names.iter().enumerate() {
                        let id = self.declare(n);
                        self.line(&format!("let mut {id} = nth(&{t}, {i});"));
                    }
                }
                Ok(())
            }
            Stat::Assign { targets, values } => {
                if targets.len() != 1 || values.len() != 1 {
                    return unsupported("multiple assignment");
                }
                let v = self.expr(&values[0])?;
                match &targets[0] {
                    Expr::Name(n) if self.is_local(n) => self.line(&format!("{} = {v};", ident(n))),
                    Expr::Name(n) => return unsupported(format!("assignment to upvalue {n}")),
                    Expr::Index(obj, key) => {
                        let o = self.expr(obj)?;
                        let k = self.expr(key)?;
                        self.line(&format!("set_index(&{o}, {k}, {v});"));
                    }
                    _ => return unsupported("assignment target"),
                }
                Ok(())
            }
            Stat::Call(e) => {
                let v = self.expr_multi(e)?;
                self.line(&format!("let _ = {v};"));
                Ok(())
            }
            Stat::If { conds, otherwise } => {
                for (i, (c, b)) in conds.iter().enumerate() {
                    let cv = self.expr(c)?;
                    if i == 0 {
                        self.line(&format!("if ({cv}).truthy() {{"));
                    } else {
                        // Conditions after the first are evaluated only when reached
                        self.line(&format!("}} else if ({cv}).truthy() {{"));
                    }
                    self.block(b)?;
                }
                if let Some(b) = otherwise {
                    self.line("} else {");
                    self.block(b)?;
                }
                self.line("}");
                Ok(())
            }
            Stat::GenericFor { vars, exprs, body } => {
                let [Expr::Call { func, args }] = exprs.as_slice() else {
                    return unsupported("generic for");
                };
                let iter = match (&**func, args.len()) {
                    (Expr::Name(n), 1) if n == "ipairs" && !self.is_local(n) => "ipairs",
                    (Expr::Name(n), 1) if n == "pairs" && !self.is_local(n) => "pairs",
                    _ => return unsupported("for over a custom iterator"),
                };
                let t = self.expr(&args[0])?;
                let (k, v) = (self.tmp(), self.tmp());
                self.line(&format!("for ({k}, {v}) in {iter}(&{t}) {{"));
                self.indent += 1;
                self.scopes.push(HashSet::new());
                let ids: Vec<String> = vars.iter().map(|n| self.declare(n)).collect();
                if let Some(id) = ids.first() {
                    self.line(&format!("let mut {id} = {k};"));
                }
                if let Some(id) = ids.get(1) {
                    self.line(&format!("let mut {id} = {v};"));
                }
                self.block_body(body)?;
                self.scopes.pop();
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            _ => unsupported(
                format!("statement {s:?}")
                    .chars()
                    .take(80)
                    .collect::<String>(),
            ),
        }
    }

    /// Expressions as a `Vec<Value>`, expanding a trailing call.
    fn exprlist(&mut self, es: &[Expr]) -> R<String> {
        match es {
            [] => Ok("vec![]".into()),
            [single] => self.expr_multi(single),
            _ => {
                let (last, init) = es.split_last().expect("non-empty");
                let mut parts = Vec::new();
                for e in init {
                    parts.push(self.expr(e)?);
                }
                if is_multi(last) {
                    let m = self.expr_multi(last)?;
                    Ok(format!(
                        "{{ let mut v = vec![{}]; v.extend({m}); v }}",
                        parts.join(", ")
                    ))
                } else {
                    parts.push(self.expr(last)?);
                    Ok(format!("vec![{}]", parts.join(", ")))
                }
            }
        }
    }

    /// An expression that may return several values, as `Vec<Value>`.
    fn expr_multi(&mut self, e: &Expr) -> R<String> {
        match e {
            Expr::Call { func, args } => self.call(func, args),
            Expr::Method { obj, name, args } => self.method(obj, name, args),
            Expr::Vararg => unsupported("varargs"),
            _ => Ok(format!("vec![{}]", self.expr(e)?)),
        }
    }

    /// An expression truncated to one value.
    fn expr(&mut self, e: &Expr) -> R<String> {
        Ok(match e {
            Expr::Nil => "nil()".into(),
            Expr::True => "Value::Bool(true)".into(),
            Expr::False => "Value::Bool(false)".into(),
            Expr::Number(x) => num_lit(*x),
            Expr::Str(s) => str_lit(s),
            Expr::Vararg => return unsupported("varargs"),
            Expr::Function(_) => return unsupported("nested function"),
            Expr::Paren(e) => self.expr(e)?,
            Expr::Name(n) => self.name(n)?,
            Expr::Index(obj, key) => {
                let o = self.expr(obj)?;
                match &**key {
                    Expr::Str(s) if s.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_') => {
                        format!("field(&{o}, \"{}\")", String::from_utf8_lossy(s))
                    }
                    k => {
                        let k = self.expr(k)?;
                        format!("index(&{o}, &{k})")
                    }
                }
            }
            Expr::Call { .. } | Expr::Method { .. } => format!("first({})", self.expr_multi(e)?),
            Expr::Table(fields) => self.table(fields)?,
            Expr::Unary(op, a) => {
                let a = self.expr(a)?;
                match op {
                    UnOp::Neg => format!("neg(&{a})"),
                    UnOp::Not => format!("Value::Bool(!({a}).truthy())"),
                    UnOp::Len => format!("len(&{a})"),
                }
            }
            Expr::Binary(op, a, b) => {
                let x = self.expr(a)?;
                let y = self.expr(b)?;
                match op {
                    BinOp::Add => format!("add(&{x}, &{y})"),
                    BinOp::Sub => format!("sub(&{x}, &{y})"),
                    BinOp::Mul => format!("mul(&{x}, &{y})"),
                    BinOp::Div => format!("div(&{x}, &{y})"),
                    BinOp::Mod => format!("rem(&{x}, &{y})"),
                    BinOp::Pow => format!("pow(&{x}, &{y})"),
                    BinOp::Concat => format!("concat(&{x}, &{y})"),
                    BinOp::Eq => format!("Value::Bool(eq(&{x}, &{y}))"),
                    BinOp::Ne => format!("Value::Bool(!eq(&{x}, &{y}))"),
                    BinOp::Lt => format!("Value::Bool(lt(&{x}, &{y}))"),
                    BinOp::Le => format!("Value::Bool(le(&{x}, &{y}))"),
                    BinOp::Gt => format!("Value::Bool(lt(&{y}, &{x}))"),
                    BinOp::Ge => format!("Value::Bool(le(&{y}, &{x}))"),
                    BinOp::And => {
                        format!("{{ let v = {x}; if v.truthy() {{ {y} }} else {{ v }} }}")
                    }
                    BinOp::Or => format!("{{ let v = {x}; if v.truthy() {{ v }} else {{ {y} }} }}"),
                }
            }
        })
    }

    fn name(&mut self, n: &str) -> R<String> {
        if self.is_local(n) {
            return Ok(format!("{}.clone()", ident(n)));
        }
        if n == "m_huge" {
            return Ok("n(f64::INFINITY)".into());
        }
        if self.up.values.contains(&n) {
            return Ok(format!("c.value(\"{n}\")"));
        }
        unsupported(format!("name {n}"))
    }

    fn table(&mut self, fields: &[Field]) -> R<String> {
        let mut keyed = Vec::new();
        let mut items = Vec::new();
        let mut multi = None;
        for (i, f) in fields.iter().enumerate() {
            match f {
                Field::Keyed(k, v) => {
                    // Lua evaluates fields in order; keys and values here have no side effects
                    let k = self.expr(k)?;
                    let v = self.expr(v)?;
                    keyed.push(format!("({k}, {v})"));
                }
                Field::Item(e) => {
                    if i + 1 == fields.len() && is_multi(e) {
                        multi = Some(self.expr_multi(e)?);
                    } else {
                        items.push(self.expr(e)?);
                    }
                }
            }
        }
        let items = match multi {
            Some(m) => format!(
                "{{ let mut v = vec![{}]; v.extend({m}); v }}",
                items.join(", ")
            ),
            None => format!("vec![{}]", items.join(", ")),
        };
        Ok(format!("tbl(vec![{}], {items})", keyed.join(", ")))
    }

    fn args(&mut self, args: &[Expr]) -> R<String> {
        self.exprlist(args)
    }

    fn call(&mut self, func: &Expr, args: &[Expr]) -> R<String> {
        let name = match func {
            Expr::Name(n) if !self.is_local(n) => n.clone(),
            Expr::Index(obj, key) => match (&**obj, &**key) {
                (Expr::Name(o), Expr::Str(k)) if !self.is_local(o) => {
                    format!("{o}.{}", String::from_utf8_lossy(k))
                }
                _ => return unsupported("call of a computed function"),
            },
            _ => return unsupported("call of a local function"),
        };
        if let Some((_, rust)) = self.up.helpers.iter().find(|(l, _)| *l == name) {
            let a = self.args(args)?;
            return Ok(format!("c.{rust}({a})"));
        }
        let one = |g: &mut Self, i: usize| -> R<String> {
            match args.get(i) {
                Some(e) => g.expr(e),
                None => Ok("nil()".into()),
            }
        };
        Ok(match name.as_str() {
            "tonumber" => format!("vec![tonumber(&{})]", one(self, 0)?),
            "tostring" => format!("vec![tostring(&{})]", one(self, 0)?),
            "type" => format!("vec![s({}.type_name())]", one(self, 0)?),
            "unpack" => format!("unpack(&{})", one(self, 0)?),
            "copyTable" => format!("vec![copy_table(&{})]", one(self, 0)?),
            "isValueInArray" => {
                let (t, v) = (one(self, 0)?, one(self, 1)?);
                format!("vec![Value::Bool(is_value_in_array(&{t}, &{v}))]")
            }
            "t_insert" | "table.insert" => {
                if args.len() != 2 {
                    return unsupported("table.insert with a position");
                }
                let (t, v) = (one(self, 0)?, one(self, 1)?);
                format!("{{ t_insert(&{t}, {v}); vec![] }}")
            }
            "bor" | "bit.bor" => format!("vec![bor(&{})]", self.args(args)?),
            "band" | "bit.band" => format!("vec![band(&{})]", self.args(args)?),
            "bnot" | "bit.bnot" => format!("vec![bnot(&{})]", one(self, 0)?),
            "math.floor" => format!("vec![floor(&{})]", one(self, 0)?),
            "math.min" => format!("vec![min(&{})]", self.args(args)?),
            "math.max" => format!("vec![max(&{})]", self.args(args)?),
            "string.format" => {
                let a = self.args(args)?;
                format!("{{ let a = {a}; vec![format(&a[0], &a[1..])] }}")
            }
            "string.gsub" => {
                let (s, p) = (one(self, 0)?, one(self, 1)?);
                self.gsub(&s, &p, args.get(2), args.get(3))?
            }
            _ => return unsupported(format!("call of {name}")),
        })
    }

    fn gsub(&mut self, s: &str, p: &str, repl: Option<&Expr>, n: Option<&Expr>) -> R<String> {
        let n = match n {
            Some(e) => self.expr(e)?,
            None => "nil()".into(),
        };
        Ok(match repl {
            Some(Expr::Index(o, k)) if matches!((&**o, &**k), (Expr::Name(o), Expr::Str(k)) if o == "string" && (k == b"upper" || k == b"lower")) =>
            {
                let Expr::Str(k) = &**k else { unreachable!() };
                let f = if k == b"upper" {
                    "upper_fn"
                } else {
                    "lower_fn"
                };
                format!("gsub_fn(&{s}, &{p}, &mut {f}, &{n})")
            }
            Some(e) => {
                let r = self.expr(e)?;
                format!("gsub(&{s}, &{p}, &{r}, &{n})")
            }
            None => return unsupported("gsub without replacement"),
        })
    }

    fn method(&mut self, obj: &Expr, name: &str, args: &[Expr]) -> R<String> {
        let o = self.expr(obj)?;
        let one = |g: &mut Self, i: usize| -> R<String> {
            match args.get(i) {
                Some(e) => g.expr(e),
                None => Ok("nil()".into()),
            }
        };
        Ok(match name {
            "lower" => format!("vec![lower(&{o})]"),
            "upper" => format!("vec![upper(&{o})]"),
            "sub" => {
                let (i, j) = (one(self, 0)?, one(self, 1)?);
                format!("vec![str_sub(&{o}, &{i}, &{j})]")
            }
            "match" => {
                let (p, i) = (one(self, 0)?, one(self, 1)?);
                format!("str_match(&{o}, &{p}, &{i})")
            }
            "find" => {
                let (p, i, plain) = (one(self, 0)?, one(self, 1)?, one(self, 2)?);
                format!("str_find(&{o}, &{p}, &{i}, &{plain})")
            }
            "gsub" => {
                let p = one(self, 0)?;
                self.gsub(&o, &p, args.get(1), args.get(2))?
            }
            _ => return unsupported(format!("method {name}")),
        })
    }
}

fn is_multi(e: &Expr) -> bool {
    matches!(e, Expr::Call { .. } | Expr::Method { .. } | Expr::Vararg)
}

/// Collects every function literal in a block, keyed by byte offset.
pub fn functions_by_offset(b: &Block) -> std::collections::HashMap<usize, &FuncBody> {
    fn expr<'a>(e: &'a Expr, out: &mut std::collections::HashMap<usize, &'a FuncBody>) {
        match e {
            Expr::Function(f) => {
                out.insert(f.span.0, f);
                block(&f.body, out);
            }
            Expr::Table(fields) => {
                for f in fields {
                    match f {
                        Field::Keyed(k, v) => {
                            expr(k, out);
                            expr(v, out);
                        }
                        Field::Item(v) => expr(v, out),
                    }
                }
            }
            Expr::Index(a, b) | Expr::Binary(_, a, b) => {
                expr(a, out);
                expr(b, out);
            }
            Expr::Call { func, args } => {
                expr(func, out);
                args.iter().for_each(|a| expr(a, out));
            }
            Expr::Method { obj, args, .. } => {
                expr(obj, out);
                args.iter().for_each(|a| expr(a, out));
            }
            Expr::Unary(_, a) | Expr::Paren(a) => expr(a, out),
            _ => {}
        }
    }
    fn block<'a>(b: &'a Block, out: &mut std::collections::HashMap<usize, &'a FuncBody>) {
        for s in &b.stats {
            match s {
                Stat::Local { values, .. } => values.iter().for_each(|e| expr(e, out)),
                Stat::Assign { targets, values } => {
                    targets.iter().chain(values).for_each(|e| expr(e, out))
                }
                Stat::Call(e) => expr(e, out),
                Stat::Do(b) => block(b, out),
                Stat::While { cond, body } => {
                    expr(cond, out);
                    block(body, out);
                }
                Stat::Repeat { body, cond } => {
                    block(body, out);
                    expr(cond, out);
                }
                Stat::If { conds, otherwise } => {
                    for (c, b) in conds {
                        expr(c, out);
                        block(b, out);
                    }
                    if let Some(b) = otherwise {
                        block(b, out);
                    }
                }
                Stat::NumericFor {
                    start,
                    limit,
                    step,
                    body,
                    ..
                } => {
                    expr(start, out);
                    expr(limit, out);
                    if let Some(s) = step {
                        expr(s, out);
                    }
                    block(body, out);
                }
                Stat::GenericFor { exprs, body, .. } => {
                    exprs.iter().for_each(|e| expr(e, out));
                    block(body, out);
                }
                Stat::Function { func, .. } | Stat::LocalFunction { func, .. } => {
                    out.insert(func.span.0, func);
                    block(&func.body, out);
                }
                _ => {}
            }
        }
        if let Some(LastStat::Return(es)) = &b.last {
            es.iter().for_each(|e| expr(e, out));
        }
    }
    let mut out = std::collections::HashMap::new();
    block(b, &mut out);
    out
}
