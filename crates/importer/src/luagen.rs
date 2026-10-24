//! Translates whole upstream Lua modules into Rust that mirrors them
//! statement by statement, running on `lua::rt`. Generated modules keep
//! upstream's structure, function names and evaluation order, so an upstream
//! diff maps to a diff of the generated code. See docs/design.md.
//!
//! A module becomes `pub fn chunk(env: &Rc<Env>, args: &[Value]) ->
//! LResult<Vec<Value>>`. Locals captured by closures become shared cells;
//! other locals are plain variables.

use lua::ast::*;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Unsupported(pub String);

type R<T> = std::result::Result<T, Unsupported>;

fn unsupported<T>(msg: impl Into<String>) -> R<T> {
    Err(Unsupported(msg.into()))
}

/// Identity of a local's declaration: AST node address and position.
type DeclKey = (usize, usize);

fn addr<T>(x: &T) -> usize {
    x as *const T as usize
}

/// Pass 1: which locals are captured by closures, and which outer locals
/// every function needs.
#[derive(Default)]
struct Captures {
    captured: HashSet<DeclKey>,
    /// FuncBody address -> captured outer declarations
    func_captures: HashMap<usize, Vec<DeclKey>>,
    scopes: Vec<Vec<(String, DeclKey, usize)>>,
    func_stack: Vec<usize>,
}

impl Captures {
    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, key: DeclKey) {
        let depth = self.func_stack.len();
        self.scopes
            .last_mut()
            .expect("scope")
            .push((name.to_string(), key, depth));
    }

    fn use_name(&mut self, name: &str) {
        let found = self
            .scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .find(|(n, _, _)| n == name)
            .cloned();
        if let Some((_, key, depth)) = found {
            let cur = self.func_stack.len();
            if depth < cur {
                self.captured.insert(key);
                for f in &self.func_stack[depth..cur] {
                    let list = self.func_captures.entry(*f).or_default();
                    if !list.contains(&key) {
                        list.push(key);
                    }
                }
            }
        }
    }

    fn block(&mut self, b: &Block) {
        self.push_scope();
        for s in &b.stats {
            self.stat(s);
        }
        if let Some(LastStat::Return(es)) = &b.last {
            es.iter().for_each(|e| self.expr(e));
        }
        self.pop_scope();
    }

    fn func(&mut self, f: &FuncBody) {
        self.func_stack.push(addr(f));
        self.push_scope();
        for (i, p) in f.params.iter().enumerate() {
            self.declare(p, (addr(f), i));
        }
        self.block(&f.body);
        self.pop_scope();
        self.func_stack.pop();
    }

    fn stat(&mut self, s: &Stat) {
        match s {
            Stat::Local { names, values } => {
                values.iter().for_each(|e| self.expr(e));
                for (i, n) in names.iter().enumerate() {
                    self.declare(n, (addr(s), i));
                }
            }
            Stat::LocalFunction { name, func } => {
                self.declare(name, (addr(s), 0));
                self.func(func);
            }
            Stat::Assign { targets, values } => {
                targets.iter().chain(values).for_each(|e| self.expr(e));
            }
            Stat::Call(e) => self.expr(e),
            Stat::Do(b) => self.block(b),
            Stat::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            Stat::Repeat { body, cond } => {
                // The condition sees the body's locals
                self.push_scope();
                for s in &body.stats {
                    self.stat(s);
                }
                if let Some(LastStat::Return(es)) = &body.last {
                    es.iter().for_each(|e| self.expr(e));
                }
                self.expr(cond);
                self.pop_scope();
            }
            Stat::If { conds, otherwise } => {
                for (c, b) in conds {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(b) = otherwise {
                    self.block(b);
                }
            }
            Stat::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                self.expr(start);
                self.expr(limit);
                if let Some(st) = step {
                    self.expr(st);
                }
                self.push_scope();
                self.declare(var, (addr(s), 0));
                self.block(body);
                self.pop_scope();
            }
            Stat::GenericFor { vars, exprs, body } => {
                exprs.iter().for_each(|e| self.expr(e));
                self.push_scope();
                for (i, v) in vars.iter().enumerate() {
                    self.declare(v, (addr(s), i));
                }
                self.block(body);
                self.pop_scope();
            }
            Stat::Function { path, func, .. } => {
                self.use_name(&path[0]);
                self.func(func);
            }
            Stat::Goto(_) | Stat::Label(_) => {}
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Name(n) => self.use_name(n),
            Expr::Function(f) => self.func(f),
            Expr::Table(fields) => {
                for f in fields {
                    match f {
                        Field::Keyed(k, v) => {
                            self.expr(k);
                            self.expr(v);
                        }
                        Field::Item(v) => self.expr(v),
                    }
                }
            }
            Expr::Index(a, b) | Expr::Binary(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Call { func, args } => {
                self.expr(func);
                args.iter().for_each(|a| self.expr(a));
            }
            Expr::Method { obj, args, .. } => {
                self.expr(obj);
                args.iter().for_each(|a| self.expr(a));
            }
            Expr::Unary(_, a) | Expr::Paren(a) => self.expr(a),
            _ => {}
        }
    }
}

/// Functions with more statements than this are split into segment
/// functions, so that no generated Rust function gets big: rustc's borrow
/// checker is superlinear in function size.
const BIG_FUNCTION: usize = 60;
/// Statements per segment function.
const SEGMENT: usize = 8;
/// Fields per chunk function of a big table constructor.
const TABLE_CHUNK: usize = 16;

struct Local {
    name: String,
    /// Where the current function finds it: `fr.r[3]` or `fr.c[2]`
    loc: String,
    cell: bool,
    /// Unique name, for upvalue struct fields
    uname: String,
}

/// State of the Lua function being generated.
struct FnCtx {
    id: usize,
    /// Cells reached through `fr.up`, by unique name
    up: HashSet<String>,
    regs: (usize, usize),
    cells: (usize, usize),
    /// Whether blocks are split into segment functions
    big: bool,
    /// Whether code goes into a segment function rather than the main function
    in_segment: bool,
    /// Loops open in the Rust function being written
    loops: usize,
    segments: usize,
}

pub struct Gen<'a> {
    src: &'a [u8],
    caps: Captures,
    out: String,
    indent: usize,
    scopes: Vec<Vec<Local>>,
    /// Unique names of declarations, to name captured cells
    unames: HashMap<DeclKey, String>,
    counter: usize,
    strings: Vec<Vec<u8>>,
    string_index: HashMap<Vec<u8>, usize>,
    /// Labels a `goto` breaks out of, with their Rust labels
    goto_labels: Vec<(String, String)>,
    /// Names of the functions being generated, for function values
    func_names: Vec<String>,
    /// Generated top-level items
    fns: String,
    ctx: Vec<FnCtx>,
}

fn new_gen(src: &[u8]) -> R<(Gen<'_>, Block)> {
    let block = lua::parser::parse(src).map_err(|e| Unsupported(format!("parse error: {e}")))?;
    let mut caps = Captures::default();
    caps.scopes.push(Vec::new());
    caps.func_stack.push(0);
    caps.block(&block);
    let g = Gen {
        src,
        caps,
        out: String::new(),
        indent: 1,
        scopes: vec![Vec::new()],
        unames: HashMap::new(),
        counter: 0,
        strings: Vec::new(),
        string_index: HashMap::new(),
        goto_labels: Vec::new(),
        func_names: vec!["chunk".into()],
        fns: String::new(),
        ctx: Vec::new(),
    };
    Ok((g, block))
}

/// Lints generated code does not follow; set on the module that includes it.
const ALLOW: &str = "#[allow(dead_code, unused_mut, unused_variables, unused_assignments, unreachable_code, unused_imports, non_snake_case, clippy::all)]";

const PRELUDE: &str = "use lua::rt::*;\nuse lua::{LResult, Table, TableRef, Value, LuaStr};\nuse lua::env::Env;\nuse std::rc::Rc;\n\n";

impl Gen<'_> {
    fn names_fn(&self) -> String {
        let mut out = String::from("fn names() -> Rc<Vec<LuaStr>> {\n    Rc::new(vec![\n");
        for s in &self.strings {
            let _ = writeln!(out, "        LuaStr::new({}),", byte_lit(s));
        }
        out.push_str("    ])\n}\n\n");
        out
    }
}

/// Translates a module chunk into Rust source.
pub fn translate_module(src: &[u8], header: &str) -> R<String> {
    let (mut g, block) = new_gen(src)?;
    // The chunk is a vararg function without upvalues
    let chunk = FuncBody {
        params: vec![],
        vararg: true,
        body: block,
        line: 1,
        span: (0, 0),
    };
    let id = g.function_def(&chunk, "chunk", &[])?;
    let mut out = String::new();
    out.push_str(header);
    out.push_str(PRELUDE);
    out.push_str(&g.names_fn());
    let _ = write!(
        out,
        "pub fn chunk(env: &Rc<Env>, args: &[Value]) -> LResult<Vec<Value>> {{\n    let kn = names();\n    let k: Rc<Vec<Value>> = Rc::new(kn.iter().map(|s| Value::Str(s.clone())).collect());\n    let u = U{id} {{ env: env.clone(), k, kn }};\n    f{id}(&u, args)\n}}\n\n"
    );
    out.push_str(&g.fns);
    Ok(out)
}

/// Translates the function literals of a data file, which the importer
/// evaluates as data. Each becomes `make_<offset>(env, locals)`, creating
/// the function with the file's locals it uses (e.g. `mod` in skill files)
/// as upvalues. Returns the module source and the offsets translated.
pub fn translate_data_functions(src: &[u8], header: &str) -> R<(String, Vec<usize>)> {
    let (mut g, block) = new_gen(src)?;
    // A pseudo function for the file's top level, whose locals are cells
    g.counter += 1;
    let top_id = g.counter;
    g.ctx.push(FnCtx {
        id: top_id,
        up: HashSet::new(),
        regs: (0, 0),
        cells: (0, 0),
        big: false,
        in_segment: false,
        loops: 0,
        segments: 0,
    });
    let mut top_locals: Vec<(String, String)> = Vec::new();
    let mut funcs: Vec<(&FuncBody, String)> = Vec::new();
    for s in &block.stats {
        if let Stat::Local { names, .. } = s {
            for (i, n) in names.iter().enumerate() {
                // Force cells, so functions can capture them
                g.caps.captured.insert((addr(s), i));
                let l = g.declare(n, (addr(s), i));
                top_locals.push((n.clone(), l.0));
            }
        }
        collect_functions_stat(s, &mut funcs);
    }
    if let Some(LastStat::Return(es)) = &block.last {
        es.iter()
            .for_each(|e| collect_functions_expr(e, "return", &mut funcs));
    }
    let mut makers = String::new();
    let mut offsets = Vec::new();
    for (f, name) in funcs {
        let creation = g.function(f, &name)?;
        let _ = writeln!(
            makers,
            "pub fn make_{}(env: &Rc<Env>, locals: &dyn Fn(&str) -> Value) -> Value {{",
            f.span.0
        );
        makers.push_str("    let kn = names();\n    let k: Rc<Vec<Value>> = Rc::new(kn.iter().map(|s| Value::Str(s.clone())).collect());\n");
        let _ = writeln!(
            makers,
            "    let u = U{top_id} {{ env: env.clone(), k, kn }};"
        );
        let _ = writeln!(
            makers,
            "    let mut frame = {};",
            g.frame_init(top_id, 0, g.ctx[0].cells.1, "vec![]")
        );
        makers.push_str("    let fr = &mut frame;\n");
        for (n, loc) in &top_locals {
            let _ = writeln!(makers, "    {loc} = cell(locals({n:?}));");
        }
        let _ = writeln!(makers, "    {creation}\n}}\n");
        offsets.push(f.span.0);
    }
    let top = g.ctx.pop().expect("top");
    g.fns.push_str(&format!("struct U{top_id} {{\n    env: Rc<Env>,\n    k: Rc<Vec<Value>>,\n    kn: Rc<Vec<LuaStr>>,\n}}\n\n"));
    g.fns.push_str(&g.frame_struct(top_id, 0, top.cells.1));
    let mut out = String::new();
    out.push_str(header);
    out.push_str(PRELUDE);
    out.push_str(&g.names_fn());
    out.push_str(&makers);
    out.push_str(&g.fns);
    Ok((out, offsets))
}

fn byte_lit(bytes: &[u8]) -> String {
    let mut s = String::from("b\"");
    for &b in bytes {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7e => s.push(b as char),
            _ => {
                let _ = write!(s, "\\x{b:02x}");
            }
        }
    }
    s.push('"');
    s
}

fn num_lit(x: f64) -> String {
    if x.is_infinite() {
        return if x > 0.0 {
            "Value::Number(f64::INFINITY)".into()
        } else {
            "Value::Number(f64::NEG_INFINITY)".into()
        };
    }
    if x.is_nan() {
        return "Value::Number(f64::NAN)".into();
    }
    let mut s = format!("{x:?}");
    if !s.contains(['.', 'e', 'E', 'i', 'N']) {
        s.push_str(".0");
    }
    format!("Value::Number({s})")
}

/// Whether evaluating an expression can have no side effects.
fn pure(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Name(_) | Expr::Number(_) | Expr::Str(_) | Expr::Nil | Expr::True | Expr::False
    )
}

fn is_multi(e: &Expr) -> bool {
    matches!(e, Expr::Call { .. } | Expr::Method { .. } | Expr::Vararg)
}

/// Number of statements, counting nested blocks but not nested functions.
fn size(b: &Block) -> usize {
    b.stats
        .iter()
        .map(|s| {
            1 + match s {
                Stat::Do(b) => size(b),
                Stat::While { body, .. } | Stat::Repeat { body, .. } => size(body),
                Stat::NumericFor { body, .. } | Stat::GenericFor { body, .. } => size(body),
                Stat::If { conds, otherwise } => {
                    conds.iter().map(|(_, b)| size(b)).sum::<usize>()
                        + otherwise.as_ref().map(size).unwrap_or(0)
                }
                _ => 0,
            }
        })
        .sum()
}

/// Labels a statement's gotos jump to outside the statement.
fn escaping_gotos(s: &Stat) -> Vec<String> {
    fn block(b: &Block, out: &mut Vec<String>) {
        let inner: Vec<&String> = b
            .stats
            .iter()
            .filter_map(|s| {
                if let Stat::Label(l) = s {
                    Some(l)
                } else {
                    None
                }
            })
            .collect();
        for s in &b.stats {
            for l in escaping_gotos(s) {
                if !inner.contains(&&l) {
                    out.push(l);
                }
            }
        }
    }
    let mut out = Vec::new();
    match s {
        Stat::Goto(l) if l != "break" => out.push(l.clone()),
        Stat::Do(b) => block(b, &mut out),
        Stat::While { body, .. } | Stat::Repeat { body, .. } => block(body, &mut out),
        Stat::NumericFor { body, .. } | Stat::GenericFor { body, .. } => block(body, &mut out),
        Stat::If { conds, otherwise } => {
            for (_, b) in conds {
                block(b, &mut out);
            }
            if let Some(b) = otherwise {
                block(b, &mut out);
            }
        }
        _ => {}
    }
    out
}

/// Whether a statement contains a `break` of an enclosing loop (outside
/// loops and functions inside it).
fn has_break(s: &Stat) -> bool {
    fn block(b: &Block) -> bool {
        b.stats.iter().any(has_break) || matches!(b.last, Some(LastStat::Break))
    }
    match s {
        Stat::Goto(l) => l == "break",
        Stat::Do(b) => block(b),
        Stat::If { conds, otherwise } => {
            conds.iter().any(|(_, b)| block(b)) || otherwise.as_ref().is_some_and(block)
        }
        _ => false,
    }
}

impl Gen<'_> {
    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn fresh(&mut self, base: &str) -> String {
        self.counter += 1;
        let clean: String = base
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("u{}_{clean}", self.counter)
    }

    fn tmp(&mut self) -> String {
        self.counter += 1;
        format!("t{}", self.counter)
    }

    fn ctx(&self) -> &FnCtx {
        self.ctx.last().expect("function")
    }

    fn ctx_mut(&mut self) -> &mut FnCtx {
        self.ctx.last_mut().expect("function")
    }

    /// Index of a string constant in the module's pool.
    fn kidx(&mut self, s: &[u8]) -> usize {
        match self.string_index.get(s) {
            Some(&i) => i,
            None => {
                self.strings.push(s.to_vec());
                self.string_index.insert(s.to_vec(), self.strings.len() - 1);
                self.strings.len() - 1
            }
        }
    }

    /// A string constant as an owned `Value`.
    fn kstr(&mut self, s: &[u8]) -> String {
        format!("fr.up.k[{}].clone()", self.kidx(s))
    }

    /// A string constant as `&LuaStr`.
    fn kname(&mut self, s: &[u8]) -> String {
        format!("&fr.up.kn[{}]", self.kidx(s))
    }

    /// How the current function reaches a local's cell.
    fn cell_expr(&self, l: &Local) -> String {
        if self.ctx().up.contains(&l.uname) {
            format!("fr.up.{}", l.uname)
        } else {
            l.loc.clone()
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    /// Ends a scope; its register and cell slots are free again.
    fn pop_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            let regs = scope.iter().filter(|l| !l.cell).count();
            let cells = scope.len() - regs;
            let c = self.ctx_mut();
            c.regs.0 -= regs;
            c.cells.0 -= cells;
        }
    }

    /// Declares a local in the current scope; returns its location and
    /// whether it is a cell.
    fn declare(&mut self, name: &str, key: DeclKey) -> (String, bool) {
        let cell = self.caps.captured.contains(&key);
        let uname = self.fresh(name);
        self.unames.insert(key, uname.clone());
        let c = self.ctx_mut();
        let loc = if cell {
            let slot = c.cells.0;
            c.cells.0 += 1;
            c.cells.1 = c.cells.1.max(c.cells.0);
            format!("fr.c[{slot}]")
        } else {
            let slot = c.regs.0;
            c.regs.0 += 1;
            c.regs.1 = c.regs.1.max(c.regs.0);
            format!("fr.r[{slot}]")
        };
        self.scopes.last_mut().expect("scope").push(Local {
            name: name.to_string(),
            loc: loc.clone(),
            cell,
            uname,
        });
        (loc, cell)
    }

    /// Statement binding a newly declared local.
    fn bind(loc: &str, cell: bool, value: &str) -> String {
        if cell {
            format!("{loc} = cell({value});")
        } else {
            format!("{loc} = {value};")
        }
    }

    fn lookup(&self, name: &str) -> Option<&Local> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .find(|l| l.name == name)
    }

    /// Declares a local and binds it; the value is computed first, as in Lua.
    fn let_local(&mut self, name: &str, key: DeclKey, value: &str) {
        let (loc, cell) = self.declare(name, key);
        self.line(&Self::bind(&loc, cell, value));
    }

    fn return_stat(&self, v: &str) -> String {
        if self.ctx().in_segment {
            format!("return Ok(Flow::Return({v}));")
        } else {
            format!("return Ok({v});")
        }
    }

    fn break_stat(&self) -> &'static str {
        if self.ctx().loops > 0 {
            "break;"
        } else {
            "return Ok(Flow::Break);"
        }
    }

    fn block_body(&mut self, b: &Block) -> R<()> {
        // Forward gotos: the statements from the first one jumping to a label
        // up to the label go in a labeled block, and the gotos break out of
        // it. Lua forbids jumping into the scope of a local, so no local
        // declared in that range is used after the label.
        let mut ranges: Vec<(usize, usize, String)> = Vec::new();
        for (l, st) in b.stats.iter().enumerate() {
            if let Stat::Label(name) = st
                && let Some(g) = b.stats[..l].iter().position(|s| contains_goto(s, name))
            {
                ranges.push((g, l, name.clone()));
            }
        }
        loop {
            let mut changed = false;
            for i in 0..ranges.len() {
                for j in 0..ranges.len() {
                    let (g1, l1) = (ranges[i].0, ranges[i].1);
                    let (g2, l2) = (ranges[j].0, ranges[j].1);
                    if g1 < g2 && g2 < l1 && l1 < l2 {
                        ranges[j].0 = g1;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // Big functions put runs of statements into segment functions; not
        // across labels, and not statements jumping out of themselves
        let segment = self.ctx().big && ranges.is_empty() && b.stats.len() > SEGMENT;
        let mut open: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < b.stats.len() {
            let s = &b.stats[i];
            let mut starting: Vec<usize> =
                (0..ranges.len()).filter(|&r| ranges[r].0 == i).collect();
            starting.sort_by(|&a, &b| ranges[b].1.cmp(&ranges[a].1));
            for r in starting {
                self.counter += 1;
                let rust_label = format!("'g{}", self.counter);
                self.goto_labels
                    .push((ranges[r].2.clone(), rust_label.clone()));
                self.line(&format!("{rust_label}: {{"));
                self.indent += 1;
                open.push(r);
            }
            if let Stat::Label(_) = s {
                while let Some(&r) = open.last() {
                    if ranges[r].1 != i {
                        break;
                    }
                    open.pop();
                    self.goto_labels.pop();
                    self.indent -= 1;
                    self.line("}");
                }
                i += 1;
                continue;
            }
            if segment && escaping_gotos(s).is_empty() {
                let mut j = i;
                while j < b.stats.len() && j - i < SEGMENT && escaping_gotos(&b.stats[j]).is_empty()
                {
                    j += 1;
                }
                self.segment(&b.stats[i..j])?;
                i = j;
                continue;
            }
            self.stat(s)?;
            i += 1;
        }
        if !open.is_empty() {
            return unsupported("goto ranges that do not nest");
        }
        match &b.last {
            Some(LastStat::Return(es)) => {
                let v = self.exprlist(es)?;
                let r = self.return_stat(&v);
                self.line(&r);
            }
            Some(LastStat::Break) => {
                let b = self.break_stat();
                self.line(b);
            }
            None => {}
        }
        Ok(())
    }

    /// Puts statements in a segment function of the current Lua function and
    /// calls it.
    fn segment(&mut self, stats: &[Stat]) -> R<()> {
        let id = self.ctx().id;
        let n = {
            let c = self.ctx_mut();
            c.segments += 1;
            c.segments
        };
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        let saved = {
            let c = self.ctx_mut();
            let s = (c.in_segment, c.loops);
            c.in_segment = true;
            c.loops = 0;
            s
        };
        self.indent = 1;
        let mut r = Ok(());
        for s in stats {
            r = self.stat(s);
            if r.is_err() {
                break;
            }
        }
        self.line("Ok(Flow::Next)");
        let body = std::mem::replace(&mut self.out, saved_out);
        self.indent = saved_indent;
        {
            let c = self.ctx_mut();
            c.in_segment = saved.0;
            c.loops = saved.1;
        }
        r?;
        let _ = write!(
            self.fns,
            "fn f{id}_s{n}(fr: &mut F{id}) -> LResult<Flow> {{\n{body}}}\n\n"
        );
        let brk = if stats.iter().any(has_break) {
            self.break_stat()
        } else {
            "unreachable!(),"
        };
        let brk = if brk.ends_with(';') {
            format!("{{ {brk} }}")
        } else {
            brk.trim_end_matches(',').to_string()
        };
        let ret = if self.ctx().in_segment {
            "return Ok(Flow::Return(v))"
        } else {
            "return Ok(v)"
        };
        self.line(&format!("match f{id}_s{n}(fr)? {{ Flow::Next => {{}} Flow::Break => {brk}, Flow::Return(v) => {ret}, }}"));
        Ok(())
    }

    fn block(&mut self, b: &Block) -> R<()> {
        self.push_scope();
        self.line("{");
        self.indent += 1;
        let r = self.block_body(b);
        self.indent -= 1;
        self.line("}");
        self.pop_scope();
        r
    }

    /// A loop body, with bindings of the loop variables first.
    fn loop_body(&mut self, b: &Block, prologue: &[String]) -> R<()> {
        self.line("{");
        self.indent += 1;
        for p in prologue {
            self.line(p);
        }
        let r = self.block_body(b);
        self.indent -= 1;
        self.line("}");
        r
    }

    fn enter_loop(&mut self) {
        self.ctx_mut().loops += 1;
    }

    fn exit_loop(&mut self) {
        self.ctx_mut().loops -= 1;
    }

    fn stat(&mut self, s: &Stat) -> R<()> {
        match s {
            Stat::Local { names, values } => {
                if names.len() == 1 && values.len() == 1 {
                    let v = self.expr(&values[0])?;
                    self.let_local(&names[0], (addr(s), 0), &v);
                } else if values.is_empty() {
                    for (i, n) in names.iter().enumerate() {
                        self.let_local(n, (addr(s), i), "Value::Nil");
                    }
                } else {
                    let vals = self.exprlist(values)?;
                    let t = self.tmp();
                    self.line(&format!("let {t}: Vec<Value> = {vals};"));
                    for (i, n) in names.iter().enumerate() {
                        self.let_local(
                            n,
                            (addr(s), i),
                            &format!("{t}.get({i}).cloned().unwrap_or_default()"),
                        );
                    }
                }
                Ok(())
            }
            Stat::LocalFunction { name, func } => {
                // Declared first, so the function can call itself
                let (loc, cell) = self.declare(name, (addr(s), 0));
                if cell {
                    self.line(&format!("{loc} = cell(Value::Nil);"));
                    let f = self.function(func, name)?;
                    self.line(&format!("set(&{loc}, {f});"));
                } else {
                    let f = self.function(func, name)?;
                    self.line(&format!("{loc} = {f};"));
                }
                Ok(())
            }
            Stat::Assign { targets, values } => self.assign(targets, values),
            Stat::Call(e) => {
                let v = self.expr_multi(e)?;
                self.line(&format!("{v};"));
                Ok(())
            }
            Stat::Do(b) => self.block(b),
            Stat::While { cond, body } => {
                self.line("loop {");
                self.indent += 1;
                let c = self.expr(cond)?;
                self.line(&format!("if !({c}).truthy() {{ break; }}"));
                self.enter_loop();
                self.push_scope();
                let r = self.loop_body(body, &[]);
                self.pop_scope();
                self.exit_loop();
                r?;
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Stat::Repeat { body, cond } => {
                // The condition sees the body's locals
                self.line("loop {");
                self.indent += 1;
                self.enter_loop();
                self.push_scope();
                let mut r = self.block_body(&Block {
                    stats: body.stats.clone(),
                    lines: body.lines.clone(),
                    last: None,
                });
                if r.is_ok() {
                    match &body.last {
                        Some(LastStat::Return(es)) => match self.exprlist(es) {
                            Ok(v) => {
                                let rs = self.return_stat(&v);
                                self.line(&rs);
                            }
                            Err(e) => r = Err(e),
                        },
                        Some(LastStat::Break) => self.line("break;"),
                        None => match self.expr(cond) {
                            Ok(c) => self.line(&format!("if ({c}).truthy() {{ break; }}")),
                            Err(e) => r = Err(e),
                        },
                    }
                }
                self.pop_scope();
                self.exit_loop();
                r?;
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Stat::If { conds, otherwise } => {
                // Conditions after the first are evaluated only when reached
                let mut opened = 0;
                for (i, (c, b)) in conds.iter().enumerate() {
                    let cv = self.expr(c)?;
                    if i == 0 {
                        self.line(&format!("if ({cv}).truthy() {{"));
                    } else {
                        self.line("} else {");
                        self.indent += 1;
                        opened += 1;
                        self.line(&format!("if ({cv}).truthy() {{"));
                    }
                    self.indent += 1;
                    self.push_scope();
                    let r = self.block_body(b);
                    self.pop_scope();
                    r?;
                    self.indent -= 1;
                }
                if let Some(b) = otherwise {
                    self.line("} else {");
                    self.indent += 1;
                    self.push_scope();
                    let r = self.block_body(b);
                    self.pop_scope();
                    r?;
                    self.indent -= 1;
                }
                self.line("}");
                for _ in 0..opened {
                    self.indent -= 1;
                    self.line("}");
                }
                Ok(())
            }
            Stat::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                let a = self.expr(start)?;
                let b = self.expr(limit)?;
                let st = match step {
                    Some(e) => self.expr(e)?,
                    None => "Value::Number(1.0)".into(),
                };
                let (i, l, sp) = (self.tmp(), self.tmp(), self.tmp());
                self.line("{");
                self.indent += 1;
                self.line(&format!("let mut {i} = for_num(&{a}, \"initial value\")?;"));
                self.line(&format!("let {l} = for_num(&{b}, \"limit\")?;"));
                self.line(&format!("let {sp} = for_num(&{st}, \"step\")?;"));
                self.line(&format!(
                    "while if {sp} > 0.0 {{ {i} <= {l} }} else {{ {i} >= {l} }} {{"
                ));
                self.indent += 1;
                self.enter_loop();
                self.push_scope();
                let (loc, cell) = self.declare(var, (addr(s), 0));
                let decl = Self::bind(&loc, cell, &format!("Value::Number({i})"));
                let r = self.loop_body(body, &[decl]);
                self.pop_scope();
                self.exit_loop();
                r?;
                self.line(&format!("{i} += {sp};"));
                self.indent -= 1;
                self.line("}");
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Stat::GenericFor { vars, exprs, body } => self.generic_for(s, vars, exprs, body),
            Stat::Function { path, method, func } => {
                let mut name = path.join(".");
                if let Some(m) = method {
                    name = format!("{name}:{m}");
                }
                let f = self.function(func, &name)?;
                if path.len() == 1 && method.is_none() {
                    self.assign_name(&path[0], &f);
                    return Ok(());
                }
                let mut obj = self.name(&path[0]);
                let (last, middle): (&str, &[String]) = match method {
                    Some(m) => (m, &path[1..]),
                    None => (&path[path.len() - 1], &path[1..path.len() - 1]),
                };
                for p in middle {
                    let k = self.kname(p.as_bytes());
                    obj = format!("field(&{obj}, {k})?");
                }
                let k = self.kstr(last.as_bytes());
                self.line(&format!("set_index(&{obj}, {k}, {f})?;"));
                Ok(())
            }
            Stat::Goto(label) => {
                if label == "break" {
                    let b = self.break_stat();
                    self.line(b);
                    return Ok(());
                }
                match self.goto_labels.iter().rev().find(|(l, _)| l == label) {
                    Some((_, rust)) => {
                        let rust = rust.clone();
                        self.line(&format!("break {rust};"));
                        Ok(())
                    }
                    None => unsupported(format!("goto {label} backwards or out of scope")),
                }
            }
            Stat::Label(_) => Ok(()),
        }
    }

    fn assign_name(&mut self, name: &str, value: &str) {
        let target = self.lookup(name).map(|l| {
            (
                l.cell,
                if l.cell {
                    self.cell_expr(l)
                } else {
                    l.loc.clone()
                },
            )
        });
        match target {
            Some((true, c)) => self.line(&format!("set(&{c}, {value});")),
            Some((false, loc)) => self.line(&format!("{loc} = {value};")),
            None => {
                let k = self.kstr(name.as_bytes());
                self.line(&format!("fr.up.env.set_global({k}, {value});"));
            }
        }
    }

    fn assign(&mut self, targets: &[Expr], values: &[Expr]) -> R<()> {
        // Lua evaluates the target prefixes, then the values, then assigns
        let mut prepared = Vec::new();
        for t in targets {
            match t {
                Expr::Name(n) => prepared.push((Some(n.clone()), None)),
                Expr::Index(obj, key) => {
                    let o = self.expr(obj)?;
                    let kexpr = self.expr(key)?;
                    let (to, tk) = (self.tmp(), self.tmp());
                    self.line(&format!("let {to} = {o};"));
                    self.line(&format!("let {tk} = {kexpr};"));
                    prepared.push((None, Some((to, tk))));
                }
                _ => return unsupported("assignment target"),
            }
        }
        if targets.len() == 1 && values.len() == 1 {
            let v = self.expr(&values[0])?;
            match &prepared[0] {
                (Some(n), _) => {
                    let n = n.clone();
                    self.assign_name(&n, &v);
                }
                (None, Some((to, tk))) => {
                    let (to, tk) = (to.clone(), tk.clone());
                    self.line(&format!("set_index(&{to}, {tk}, {v})?;"));
                }
                _ => unreachable!(),
            }
            return Ok(());
        }
        let vals = self.exprlist(values)?;
        let t = self.tmp();
        self.line(&format!("let {t}: Vec<Value> = {vals};"));
        for (i, p) in prepared.iter().enumerate() {
            let v = format!("{t}.get({i}).cloned().unwrap_or_default()");
            match p {
                (Some(n), _) => {
                    let n = n.clone();
                    self.assign_name(&n, &v);
                }
                (None, Some((to, tk))) => {
                    let (to, tk) = (to.clone(), tk.clone());
                    self.line(&format!("set_index(&{to}, {tk}, {v})?;"));
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn generic_for(&mut self, s: &Stat, vars: &[String], exprs: &[Expr], body: &Block) -> R<()> {
        // pairs(t) and ipairs(t) with the global functions iterate directly
        if let [Expr::Call { func, args }] = exprs
            && let Expr::Name(f) = &**func
            && (f == "pairs" || f == "ipairs")
            && self.lookup(f).is_none()
            && args.len() == 1
        {
            let t = self.expr(&args[0])?;
            let tv = self.tmp();
            self.line("{");
            self.indent += 1;
            self.line(&format!("let {tv} = {t};"));
            let values: [String; 2] = if f == "ipairs" {
                let (tt, i) = (self.tmp(), self.tmp());
                self.line(&format!("let {tt} = ipairs_table(&{tv})?;"));
                self.line(&format!("let mut {i}: i64 = 0;"));
                self.line("loop {");
                self.indent += 1;
                self.line(&format!("{i} += 1;"));
                let v = self.tmp();
                self.line(&format!("let {v} = {tt}.borrow().get_int({i});"));
                self.line(&format!("if {v}.is_nil() {{ break; }}"));
                [format!("Value::Number({i} as f64)"), v]
            } else {
                let (tt, key) = (self.tmp(), self.tmp());
                self.line(&format!("let {tt} = pairs_table(&{tv})?;"));
                self.line(&format!("let mut {key} = Value::Nil;"));
                self.line("loop {");
                self.indent += 1;
                let (nk, nv) = (self.tmp(), self.tmp());
                self.line(&format!("let ({nk}, {nv}) = match next(&{tt}, &{key})? {{ Some(e) => e, None => break }};"));
                self.line(&format!("{key} = {nk}.clone();"));
                [nk, nv]
            };
            self.enter_loop();
            self.push_scope();
            let mut prologue = Vec::new();
            for (j, name) in vars.iter().enumerate() {
                let (loc, cell) = self.declare(name, (addr(s), j));
                let val = values
                    .get(j)
                    .cloned()
                    .unwrap_or_else(|| "Value::Nil".into());
                prologue.push(Self::bind(&loc, cell, &val));
            }
            let r = self.loop_body(body, &prologue);
            self.pop_scope();
            self.exit_loop();
            r?;
            self.indent -= 1;
            self.line("}");
            self.indent -= 1;
            self.line("}");
            return Ok(());
        }
        // General iterator protocol
        let vals = self.exprlist(exprs)?;
        let (it, f, st, ctl) = (self.tmp(), self.tmp(), self.tmp(), self.tmp());
        self.line("{");
        self.indent += 1;
        self.line(&format!("let {it}: Vec<Value> = {vals};"));
        self.line(&format!(
            "let {f} = {it}.first().cloned().unwrap_or_default();"
        ));
        self.line(&format!(
            "let {st} = {it}.get(1).cloned().unwrap_or_default();"
        ));
        self.line(&format!(
            "let mut {ctl} = {it}.get(2).cloned().unwrap_or_default();"
        ));
        self.line("loop {");
        self.indent += 1;
        let r = self.tmp();
        self.line(&format!(
            "let {r} = call(&{f}, &[{st}.clone(), {ctl}.clone()])?;"
        ));
        self.line(&format!(
            "let {r}0 = {r}.first().cloned().unwrap_or_default();"
        ));
        self.line(&format!("if {r}0.is_nil() {{ break; }}"));
        self.line(&format!("{ctl} = {r}0.clone();"));
        self.enter_loop();
        self.push_scope();
        let mut prologue = Vec::new();
        for (j, name) in vars.iter().enumerate() {
            let (loc, cell) = self.declare(name, (addr(s), j));
            prologue.push(Self::bind(
                &loc,
                cell,
                &format!("{r}.get({j}).cloned().unwrap_or_default()"),
            ));
        }
        let res = self.loop_body(body, &prologue);
        self.pop_scope();
        self.exit_loop();
        res?;
        self.indent -= 1;
        self.line("}");
        self.indent -= 1;
        self.line("}");
        Ok(())
    }

    fn frame_struct(&self, id: usize, regs: usize, cells: usize) -> String {
        format!(
            "struct F{id}<'a> {{\n    r: [Value; {regs}],\n    c: [Cell; {cells}],\n    va: Vec<Value>,\n    up: &'a U{id},\n}}\n\n"
        )
    }

    fn frame_init(&self, id: usize, regs: usize, cells: usize, va: &str) -> String {
        let c = if cells == 0 {
            "[]".to_string()
        } else {
            "{ let ph = cell(Value::Nil); std::array::from_fn(|_| ph.clone()) }".to_string()
        };
        format!("F{id} {{ r: [const {{ Value::Nil }}; {regs}], c: {c}, va: {va}, up: &u }}")
    }

    /// Generates a Lua function's Rust definitions; returns its id.
    /// `captures` are the unique names of the cells it captures.
    fn function_def(&mut self, f: &FuncBody, name: &str, captures: &[String]) -> R<usize> {
        self.counter += 1;
        let id = self.counter;
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        let saved_labels = std::mem::take(&mut self.goto_labels);
        self.indent = 1;
        self.ctx.push(FnCtx {
            id,
            up: captures.iter().cloned().collect(),
            regs: (0, 0),
            cells: (0, 0),
            big: size(&f.body) > BIG_FUNCTION,
            in_segment: false,
            loops: 0,
            segments: 0,
        });
        self.push_scope();
        self.func_names.push(name.to_string());
        for (i, p) in f.params.iter().enumerate() {
            let (loc, cell) = self.declare(p, (addr(f), i));
            let b = Self::bind(
                &loc,
                cell,
                &format!("args.get({i}).cloned().unwrap_or_default()"),
            );
            self.line(&b);
        }
        let r = self.block_body(&f.body);
        if r.is_ok() && !matches!(f.body.last, Some(LastStat::Return(_))) {
            self.line("Ok(vec![])");
        }
        self.func_names.pop();
        self.pop_scope();
        let ctx = self.ctx.pop().expect("function");
        let body = std::mem::replace(&mut self.out, saved_out);
        self.indent = saved_indent;
        self.goto_labels = saved_labels;
        r?;
        let nparams = f.params.len();
        let va = if f.vararg {
            format!("args.get({nparams}..).map(|s| s.to_vec()).unwrap_or_default()")
        } else {
            "vec![]".into()
        };
        let mut def = format!(
            "struct U{id} {{\n    env: Rc<Env>,\n    k: Rc<Vec<Value>>,\n    kn: Rc<Vec<LuaStr>>,\n"
        );
        for c in captures {
            let _ = writeln!(def, "    {c}: Cell,");
        }
        def.push_str("}\n\n");
        def.push_str(&self.frame_struct(id, ctx.regs.1, ctx.cells.1));
        let line = if f.span.1 == 0 {
            1
        } else {
            line_of(self.src, f.span.0)
        };
        let _ = writeln!(def, "// {name}:{line}");
        let init = self
            .frame_init(id, ctx.regs.1, ctx.cells.1, &va)
            .replace("up: &u", "up: u");
        let _ = write!(
            def,
            "fn f{id}(u: &U{id}, args: &[Value]) -> LResult<Vec<Value>> {{\n    let mut frame = {init};\n    let fr = &mut frame;\n{body}}}\n\n"
        );
        self.fns.push_str(&def);
        Ok(id)
    }

    /// A function literal as a `Value` expression.
    fn function(&mut self, f: &FuncBody, name: &str) -> R<String> {
        let keys = self
            .caps
            .func_captures
            .get(&addr(f))
            .cloned()
            .unwrap_or_default();
        let captures: Vec<String> = keys
            .iter()
            .filter_map(|k| self.unames.get(k).cloned())
            .collect();
        // How the enclosing function reaches each captured cell
        let sources: Vec<String> = captures
            .iter()
            .map(|c| {
                let l = self
                    .scopes
                    .iter()
                    .flat_map(|s| s.iter())
                    .find(|l| &l.uname == c)
                    .expect("captured local in scope");
                self.cell_expr(l)
            })
            .collect();
        let id = self.function_def(f, name, &captures)?;
        let line = line_of(self.src, f.span.0);
        let label = format!("{name}:{line}");
        let mut s = format!(
            "{{ let u = Rc::new(U{id} {{ env: fr.up.env.clone(), k: fr.up.k.clone(), kn: fr.up.kn.clone(), "
        );
        for (c, src) in captures.iter().zip(&sources) {
            let _ = write!(s, "{c}: {src}.clone(), ");
        }
        let _ = write!(
            s,
            "}}); func({label:?}, move |args: &[Value]| f{id}(&u, args)) }}"
        );
        Ok(s)
    }

    fn name(&mut self, n: &str) -> String {
        match self.lookup(n) {
            Some(l) if l.cell => format!("get(&{})", self.cell_expr(l)),
            Some(l) => format!("{}.clone()", l.loc),
            None => {
                let k = self.kidx(n.as_bytes());
                format!("fr.up.env.get_global(&fr.up.k[{k}])")
            }
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

    /// Call arguments as a `&[Value]` expression.
    fn args(&mut self, es: &[Expr]) -> R<String> {
        match es.last() {
            Some(last) if is_multi(last) => Ok(format!("&{}", self.exprlist(es)?)),
            _ => {
                let mut parts = Vec::new();
                for e in es {
                    parts.push(self.expr(e)?);
                }
                Ok(format!("&[{}]", parts.join(", ")))
            }
        }
    }

    /// An expression as a `&Value`: plain locals by reference, without a clone.
    fn expr_ref(&mut self, e: &Expr) -> R<String> {
        if let Expr::Name(n) = e
            && let Some(l) = self.lookup(n)
            && !l.cell
        {
            return Ok(format!("&{}", l.loc));
        }
        Ok(format!("&{}", self.expr(e)?))
    }

    /// An expression that may produce several values, as `Vec<Value>`.
    fn expr_multi(&mut self, e: &Expr) -> R<String> {
        match e {
            Expr::Call { func, args } => {
                let f = self.expr_ref(func)?;
                let a = self.args(args)?;
                Ok(format!("call({f}, {a})?"))
            }
            Expr::Method { obj, name, args } => {
                let o = self.expr_ref(obj)?;
                let k = self.kname(name.as_bytes());
                let a = self.args(args)?;
                Ok(format!("method({o}, {k}, {a})?"))
            }
            Expr::Vararg => Ok("fr.va.clone()".into()),
            _ => Ok(format!("vec![{}]", self.expr(e)?)),
        }
    }

    /// An expression truncated to one value.
    fn expr(&mut self, e: &Expr) -> R<String> {
        Ok(match e {
            Expr::Nil => "Value::Nil".into(),
            Expr::True => "Value::Bool(true)".into(),
            Expr::False => "Value::Bool(false)".into(),
            Expr::Number(x) => num_lit(*x),
            Expr::Str(s) => self.kstr(s),
            Expr::Vararg => "fr.va.first().cloned().unwrap_or_default()".into(),
            Expr::Function(f) => {
                let name = format!(
                    "{}.<anonymous>",
                    self.func_names.last().cloned().unwrap_or_default()
                );
                self.function(f, &name)?
            }
            Expr::Paren(e) => self.expr(e)?,
            Expr::Name(n) => self.name(n),
            Expr::Index(obj, key) => {
                let o = self.expr_ref(obj)?;
                match &**key {
                    Expr::Str(s) => {
                        let k = self.kname(s);
                        format!("field({o}, {k})?")
                    }
                    k => {
                        let k = self.expr_ref(k)?;
                        format!("index({o}, {k})?")
                    }
                }
            }
            Expr::Call { .. } | Expr::Method { .. } => format!("first({})", self.expr_multi(e)?),
            Expr::Table(fields) => self.table(fields)?,
            Expr::Unary(op, a) => {
                let r = self.expr_ref(a)?;
                match op {
                    UnOp::Neg => format!("unm({r})?"),
                    UnOp::Not => format!("Value::Bool(!({r}).truthy())"),
                    UnOp::Len => format!("len({r})?"),
                }
            }
            Expr::Binary(op, a, b)
                if !matches!(op, BinOp::Gt | BinOp::Ge | BinOp::And | BinOp::Or) =>
            {
                let x = self.expr_ref(a)?;
                let y = self.expr_ref(b)?;
                match op {
                    BinOp::Add => format!("add({x}, {y})?"),
                    BinOp::Sub => format!("sub({x}, {y})?"),
                    BinOp::Mul => format!("mul({x}, {y})?"),
                    BinOp::Div => format!("div({x}, {y})?"),
                    BinOp::Mod => format!("modulo({x}, {y})?"),
                    BinOp::Pow => format!("pow({x}, {y})?"),
                    BinOp::Concat => format!("concat({x}, {y})?"),
                    BinOp::Eq => format!("Value::Bool(eq({x}, {y}))"),
                    BinOp::Ne => format!("Value::Bool(!eq({x}, {y}))"),
                    BinOp::Lt => format!("Value::Bool(lt({x}, {y})?)"),
                    BinOp::Le => format!("Value::Bool(le({x}, {y})?)"),
                    _ => unreachable!(),
                }
            }
            // a > b is b < a; when evaluating them has no effects, the order does not matter
            Expr::Binary(op @ (BinOp::Gt | BinOp::Ge), a, b) if pure(a) && pure(b) => {
                let x = self.expr_ref(a)?;
                let y = self.expr_ref(b)?;
                let f = if *op == BinOp::Gt { "lt" } else { "le" };
                format!("Value::Bool({f}({y}, {x})?)")
            }
            Expr::Binary(op, a, b) => {
                let x = self.expr(a)?;
                let y = self.expr(b)?;
                match op {
                    // a > b is b < a, with a evaluated first
                    BinOp::Gt => {
                        format!("{{ let x = {x}; let y = {y}; Value::Bool(lt(&y, &x)?) }}")
                    }
                    BinOp::Ge => {
                        format!("{{ let x = {x}; let y = {y}; Value::Bool(le(&y, &x)?) }}")
                    }
                    BinOp::And => {
                        format!("{{ let v = {x}; if v.truthy() {{ {y} }} else {{ v }} }}")
                    }
                    BinOp::Or => format!("{{ let v = {x}; if v.truthy() {{ v }} else {{ {y} }} }}"),
                    _ => unreachable!(),
                }
            }
        })
    }

    fn table(&mut self, fields: &[Field]) -> R<String> {
        if fields.is_empty() {
            return Ok("Value::Table(TableRef::new())".into());
        }
        // Positions of positional fields, in source order
        let mut positions = vec![0usize; fields.len()];
        let mut n = 0usize;
        for (i, f) in fields.iter().enumerate() {
            if let Field::Item(_) = f {
                n += 1;
                positions[i] = n;
            }
        }
        let order = constructor_order(fields);
        let mut s = String::from("{ let mut tb = Table::new(); ");
        if fields.len() <= TABLE_CHUNK {
            s.push_str(&self.table_fields(fields, &order, &positions)?);
        } else {
            // Big constructors fill the table in chunk functions
            let id = self.ctx().id;
            for chunk in order.chunks(TABLE_CHUNK) {
                let body = self.table_fields(fields, chunk, &positions)?;
                let c = {
                    let c = self.ctx_mut();
                    c.segments += 1;
                    c.segments
                };
                let _ = write!(
                    self.fns,
                    "fn f{id}_t{c}(fr: &F{id}, mut tb: &mut Table) -> LResult<()> {{\n    {body}\n    Ok(())\n}}\n\n"
                );
                let _ = write!(s, "f{id}_t{c}(fr, &mut tb)?; ");
            }
        }
        s.push_str("Value::Table(TableRef::from_table(tb)) }");
        Ok(s)
    }

    /// Code storing the constructor fields `which` in `tb`; `positions` are
    /// the positions of positional fields.
    fn table_fields(
        &mut self,
        fields: &[Field],
        which: &[usize],
        positions: &[usize],
    ) -> R<String> {
        let mut s = String::new();
        for &i in which {
            match &fields[i] {
                Field::Keyed(k, v) => {
                    let k = match k {
                        Expr::Str(st) => self.kstr(st),
                        k => self.expr(k)?,
                    };
                    let v = self.expr(v)?;
                    let _ = write!(s, "tset(&mut tb, {k}, {v})?; ");
                }
                Field::Item(e) => {
                    let n = positions[i];
                    if i + 1 == fields.len() && is_multi(e) {
                        let m = self.expr_multi(e)?;
                        let _ = write!(
                            s,
                            "for (j, v) in {m}.into_iter().enumerate() {{ tset(&mut tb, Value::Number(({n} + j) as f64), v)?; }} "
                        );
                    } else {
                        let v = self.expr(e)?;
                        let _ = write!(s, "tset(&mut tb, Value::Number({n}.0), {v})?; ");
                    }
                }
            }
        }
        Ok(s)
    }
}

/// Function literals outside other functions, with a name for messages.
fn collect_functions_stat<'a>(s: &'a Stat, out: &mut Vec<(&'a FuncBody, String)>) {
    match s {
        Stat::Local { values, .. } => values
            .iter()
            .for_each(|e| collect_functions_expr(e, "local", out)),
        Stat::Assign { values, .. } => values
            .iter()
            .for_each(|e| collect_functions_expr(e, "data", out)),
        Stat::Call(e) => collect_functions_expr(e, "data", out),
        Stat::Function { path, func, .. } => out.push((func, path.join("."))),
        Stat::LocalFunction { name, func } => out.push((func, name.clone())),
        _ => {}
    }
}

fn collect_functions_expr<'a>(e: &'a Expr, name: &str, out: &mut Vec<(&'a FuncBody, String)>) {
    match e {
        Expr::Function(f) => out.push((f, name.to_string())),
        Expr::Table(fields) => {
            for f in fields {
                match f {
                    Field::Keyed(k, v) => {
                        let n = match k {
                            Expr::Str(st) => String::from_utf8_lossy(st).to_string(),
                            _ => name.to_string(),
                        };
                        collect_functions_expr(v, &n, out);
                    }
                    Field::Item(v) => collect_functions_expr(v, name, out),
                }
            }
        }
        Expr::Call { func, args } => {
            collect_functions_expr(func, name, out);
            args.iter()
                .for_each(|a| collect_functions_expr(a, name, out));
        }
        Expr::Method { obj, args, .. } => {
            collect_functions_expr(obj, name, out);
            args.iter()
                .for_each(|a| collect_functions_expr(a, name, out));
        }
        Expr::Binary(_, a, b) | Expr::Index(a, b) => {
            collect_functions_expr(a, name, out);
            collect_functions_expr(b, name, out);
        }
        Expr::Unary(_, a) | Expr::Paren(a) => collect_functions_expr(a, name, out),
        _ => {}
    }
}

/// Whether a statement contains `goto <label>`, outside nested functions.
fn contains_goto(s: &Stat, label: &str) -> bool {
    fn block(b: &Block, label: &str) -> bool {
        b.stats.iter().any(|s| contains_goto(s, label))
    }
    match s {
        Stat::Goto(l) => l == label,
        Stat::Do(b) => block(b, label),
        Stat::While { body, .. } | Stat::Repeat { body, .. } => block(body, label),
        Stat::NumericFor { body, .. } | Stat::GenericFor { body, .. } => block(body, label),
        Stat::If { conds, otherwise } => {
            conds.iter().any(|(_, b)| block(b, label))
                || otherwise.as_ref().is_some_and(|b| block(b, label))
        }
        _ => false,
    }
}

fn line_of(src: &[u8], offset: usize) -> usize {
    src[..offset.min(src.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// Data files with code beyond what the importer evaluates; translated whole.
const DATA_MODULES: &[&str] = &["src/Data/Uniques/Special/Generated.lua"];

/// Files that are not translated: data (read by the importer), the desktop
/// launcher and updater, data export tooling.
fn skipped(rel: &str) -> bool {
    (rel.starts_with("src/Data/") && !DATA_MODULES.contains(&rel))
        || rel.starts_with("src/TreeData/")
        || rel.starts_with("src/Export/")
        || matches!(
            rel,
            "src/LaunchInstall.lua" | "src/UpdateApply.lua" | "src/UpdateCheck.lua" | "src/LaunchServer.lua" | "src/HeadlessWrapper.lua"
        )
        // Trade search, not part of the calcs; has a backward goto
        || rel == "src/Classes/TradeQueryGenerator.lua"
        || rel.starts_with("runtime/lua/sha1/")
        || rel == "runtime/lua/lua-profiler.lua"
        || rel == "runtime/lua/socket.lua"
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, root, out)?;
        } else if p.extension().is_some_and(|e| e == "lua") {
            out.push(
                p.strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

/// Rust module name for a path, e.g. src/Modules/CalcSetup.lua -> modules_calc_setup
fn module_name(rel: &str) -> String {
    let stem = rel
        .trim_start_matches("src/")
        .trim_end_matches(".lua")
        .replace("oracle/", "oracle_");
    let mut out = String::new();
    let mut prev_lower = false;
    for c in stem.chars() {
        if c == '/' || c == '-' || c == '.' {
            out.push('_');
            prev_lower = false;
        } else if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

/// What [`generate`] wrote.
pub struct Generated {
    /// Files written, relative to the output directory
    pub files: Vec<String>,
    /// Lua files read, for rebuilding when they change
    pub inputs: Vec<PathBuf>,
    /// Files that could not be translated, with the reason
    pub failed: Vec<(String, String)>,
}

/// Translates upstream's program (`root` is the repository root) into `out`:
/// `up.rs` with every module and `chunk(path)`, `data.rs` with the functions
/// of data files and `make(file, offset, ...)`, each module in its own file
/// under `up/` and `data/`. `out.rs` files are meant to be `include!`d.
pub fn generate(root: &Path, out: &Path) -> std::io::Result<Generated> {
    let upstream = root.join("upstream/PathOfBuilding");
    let mut files = Vec::new();
    collect(&upstream.join("src"), &upstream, &mut files)?;
    collect(&upstream.join("runtime/lua"), &upstream, &mut files)?;
    files.retain(|f| !skipped(f));
    let mut sources: Vec<(String, PathBuf)> = files
        .iter()
        .map(|f| (f.clone(), upstream.join(f)))
        .collect();
    // The oracle's JSON encoder, so the engine's outputs are encoded by the same code
    sources.push(("oracle/json.lua".into(), root.join("oracle/json.lua")));
    for dir in ["up", "data"] {
        let _ = std::fs::remove_dir_all(out.join(dir));
        std::fs::create_dir_all(out.join(dir))?;
    }
    let mut g = Generated {
        files: vec![],
        inputs: vec![],
        failed: vec![],
    };
    let mut registry = Vec::new();
    for (rel, path) in &sources {
        let src = std::fs::read(path)?;
        g.inputs.push(path.clone());
        let header = format!("// @generated by luagen from {rel}; do not edit.\n\n");
        match translate_module(&src, &header) {
            Ok(code) => {
                let name = module_name(rel);
                let file = format!("up/{name}.rs");
                std::fs::write(out.join(&file), code)?;
                g.files.push(file);
                registry.push((rel.clone(), name));
            }
            Err(e) => g.failed.push((rel.clone(), e.0)),
        }
    }
    // Functions of data files, which the importer evaluates
    let mut data_files = Vec::new();
    collect(&upstream.join("src/Data"), &upstream, &mut data_files)?;
    let mut data_registry = Vec::new();
    for rel in &data_files {
        let src = std::fs::read(upstream.join(rel))?;
        g.inputs.push(upstream.join(rel));
        if DATA_MODULES.contains(&rel.as_str()) || !src.windows(8).any(|w| w == b"function") {
            continue;
        }
        let header =
            format!("// @generated by luagen from {rel} (functions only); do not edit.\n\n");
        match translate_data_functions(&src, &header) {
            Ok((code, offsets)) => {
                let name = module_name(rel.trim_start_matches("src/Data/"));
                let file = format!("data/{name}.rs");
                std::fs::write(out.join(&file), code)?;
                g.files.push(file);
                data_registry.push((rel.trim_start_matches("src/").to_string(), name, offsets));
            }
            Err(e) => g.failed.push((rel.clone(), e.0)),
        }
    }
    let include = |dir: &str, name: &str| {
        format!(
            "{ALLOW}\npub mod {name} {{\n    include!(concat!(env!(\"OUT_DIR\"), \"/{dir}/{name}.rs\"));\n}}\n"
        )
    };
    let mut m = String::from("// @generated by luagen; do not edit.\n\n");
    m.push_str("/// A module's top level: runs it with `...` and returns what it returns.\n");
    m.push_str("pub type Chunk = fn(&std::rc::Rc<lua::env::Env>, &[lua::Value]) -> lua::LResult<Vec<lua::Value>>;\n\n");
    for (_, name) in &registry {
        m.push_str(&include("up", name));
    }
    m.push_str(
        "\n/// The translated chunk of a Lua file, by its path in the upstream repository.\n",
    );
    m.push_str("pub fn chunk(path: &str) -> Option<Chunk> {\n    Some(match path {\n");
    for (rel, name) in &registry {
        let _ = writeln!(m, "        {rel:?} => {name}::chunk,");
    }
    m.push_str("        _ => return None,\n    })\n}\n");
    std::fs::write(out.join("up.rs"), m)?;
    g.files.push("up.rs".into());
    let mut m = String::from("// @generated by luagen; do not edit.\n\n");
    for (_, name, _) in &data_registry {
        m.push_str(&include("data", name));
    }
    m.push_str(
        "\n/// Creates the function defined at `offset` in a data file, given the file's locals.\n",
    );
    m.push_str("pub fn make(file: &str, offset: u32, env: &std::rc::Rc<lua::env::Env>, locals: &dyn Fn(&str) -> lua::Value) -> Option<lua::Value> {\n    Some(match (file, offset) {\n");
    for (rel, name, offsets) in &data_registry {
        for o in offsets {
            let _ = writeln!(
                m,
                "        ({rel:?}, {o}) => {name}::make_{o}(env, locals),"
            );
        }
    }
    m.push_str("        _ => return None,\n    })\n}\n");
    std::fs::write(out.join("data.rs"), m)?;
    g.files.push("data.rs".into());
    Ok(g)
}
