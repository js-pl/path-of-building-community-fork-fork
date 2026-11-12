//! The values ModParser's closures compute with while they run: what their
//! Lua builds from a line's captures before it becomes typed mods and
//! fragments. Lives only for one call, so it owns its tables. Operations
//! that raise a Lua error panic, as upstream fails on them too.

use crate::mods::{Func, Mod, ModValue};
use lua::pattern::{self, Capture, Repl};
use std::rc::Rc;

pub type Str = Rc<[u8]>;

#[derive(Clone, Debug, Default)]
pub enum V {
    #[default]
    Nil,
    Bool(bool),
    Num(f64),
    Str(Str),
    Tab(Box<Tab>),
    /// A mod made by `mod(...)`
    Mod(Rc<Mod>),
    /// A function of upstream's data, e.g. what `getPerStat` returns
    Func(Rc<Func>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum K {
    Str(Str),
    Int(i64),
}

/// A table: positional slots from 1 (with holes, sized as LuaJIT sizes a
/// constructor), then the other keys in insertion order
#[derive(Clone, Debug, Default)]
pub struct Tab {
    pub arr: Vec<V>,
    pub hash: Vec<(K, V)>,
}

impl V {
    pub fn truthy(&self) -> bool {
        !matches!(self, V::Nil | V::Bool(false))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, V::Nil)
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            V::Nil => "nil",
            V::Bool(_) => "boolean",
            V::Num(_) => "number",
            V::Str(_) => "string",
            V::Tab(_) | V::Mod(_) => "table",
            V::Func(_) => "function",
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            V::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_tab(&self) -> Option<&Tab> {
        match self {
            V::Tab(t) => Some(t),
            _ => None,
        }
    }

    /// `rawequal` for what closures compare: tables have no identity here
    pub fn raw_eq(&self, o: &V) -> bool {
        match (self, o) {
            (V::Nil, V::Nil) => true,
            (V::Bool(a), V::Bool(b)) => a == b,
            (V::Num(a), V::Num(b)) => a == b,
            (V::Str(a), V::Str(b)) => a == b,
            (V::Tab(_) | V::Mod(_) | V::Func(_), _) | (_, V::Tab(_) | V::Mod(_) | V::Func(_)) => {
                panic!("comparing tables in a ModParser closure")
            }
            _ => false,
        }
    }
}

impl From<&str> for V {
    fn from(x: &str) -> V {
        V::Str(x.as_bytes().into())
    }
}

impl From<ModValue> for V {
    fn from(v: ModValue) -> V {
        match v {
            ModValue::Bool(b) => V::Bool(b),
            ModValue::Number(n) => V::Num(n),
            ModValue::Str(s) => V::Str(s.as_bytes().into()),
            ModValue::Mod(m) => V::Mod(Rc::new(*m)),
            ModValue::Func(f) => V::Func(f),
            ModValue::List(l) => tbl(vec![], l.into_iter().map(V::from).collect()),
            ModValue::Record(r) => tbl(
                r.fields
                    .into_iter()
                    .map(|(k, v)| (s(k.as_str()), V::from(v)))
                    .collect(),
                vec![],
            ),
        }
    }
}

fn key(k: &V) -> K {
    match k {
        V::Str(s) => K::Str(s.clone()),
        V::Num(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => K::Int(*n as i64),
        k => panic!("ModParser closure uses a {} key", k.type_name()),
    }
}

impl Tab {
    pub fn get_key(&self, k: &K) -> &V {
        if let K::Int(i) = k
            && *i >= 1
            && (*i as usize) <= self.arr.len()
        {
            return &self.arr[*i as usize - 1];
        }
        self.hash
            .iter()
            .find(|(hk, _)| hk == k)
            .map_or(&V::Nil, |(_, v)| v)
    }

    pub fn get(&self, k: &V) -> &V {
        if k.is_nil() {
            return &V::Nil;
        }
        self.get_key(&key(k))
    }

    pub fn get_str(&self, k: &str) -> &V {
        self.hash
            .iter()
            .find(|(hk, _)| matches!(hk, K::Str(s) if &**s == k.as_bytes()))
            .map_or(&V::Nil, |(_, v)| v)
    }

    pub fn get_int(&self, i: i64) -> &V {
        self.get_key(&K::Int(i))
    }

    pub fn set_key(&mut self, k: K, v: V) {
        if let K::Int(i) = k
            && i >= 1
        {
            let i = i as usize;
            if i <= self.arr.len() {
                self.arr[i - 1] = v;
                return;
            }
            if i == self.arr.len() + 1 && !v.is_nil() {
                self.arr.push(v);
                // Keys after it move into the array, as a rehash would
                while let Some(p) = self
                    .hash
                    .iter()
                    .position(|(hk, _)| *hk == K::Int(self.arr.len() as i64 + 1))
                {
                    let (_, x) = self.hash.remove(p);
                    self.arr.push(x);
                }
                return;
            }
        }
        match self.hash.iter().position(|(hk, _)| *hk == k) {
            Some(p) if v.is_nil() => {
                self.hash.remove(p);
            }
            Some(p) => self.hash[p].1 = v,
            None if v.is_nil() => {}
            None => self.hash.push((k, v)),
        }
    }

    /// `#t` as LuaJIT's `lj_tab_len` finds a border
    pub fn len(&self) -> usize {
        let slot = |k: usize| -> bool { k >= 1 && !self.arr[k - 1].is_nil() };
        let mut j = self.arr.len() + 1;
        if j > 1 && !slot(j - 1) {
            let mut i = 1;
            while j - i > 1 {
                let m = (i + j) >> 1;
                if !slot(m - 1) {
                    j = m;
                } else {
                    i = m;
                }
            }
            return i - 1;
        }
        j -= 1;
        while !self.get_int(j as i64 + 1).is_nil() {
            j += 1;
        }
        j
    }

    pub fn is_empty(&self) -> bool {
        self.len_pairs() == 0
    }

    /// Non-nil entries
    pub fn len_pairs(&self) -> usize {
        self.arr.iter().filter(|v| !v.is_nil()).count() + self.hash.len()
    }

    /// `t[1..#t]`
    pub fn array(&self) -> Vec<V> {
        (1..=self.len() as i64)
            .map(|i| self.get_int(i).clone())
            .collect()
    }

    /// Entries in `pairs` order: the array part, then the other keys
    pub fn entries(&self) -> impl Iterator<Item = (K, &V)> {
        self.arr
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_nil())
            .map(|(i, v)| (K::Int(i as i64 + 1), v))
            .chain(self.hash.iter().map(|(k, v)| (k.clone(), v)))
    }
}

pub fn n(x: f64) -> V {
    V::Num(x)
}

pub fn s(x: &str) -> V {
    V::from(x)
}

pub fn sb(x: &[u8]) -> V {
    V::Str(x.into())
}

pub fn nil() -> V {
    V::Nil
}

/// Argument `i` (0-based); missing arguments are nil.
pub fn arg(a: &[V], i: usize) -> V {
    a.get(i).cloned().unwrap_or_default()
}

/// Value `i` (0-based) of a multiple return.
pub fn nth(v: &[V], i: usize) -> V {
    v.get(i).cloned().unwrap_or_default()
}

pub fn first(v: Vec<V>) -> V {
    v.into_iter().next().unwrap_or_default()
}

pub fn to_number(v: &V) -> Option<f64> {
    match v {
        V::Num(n) => Some(*n),
        V::Str(s) => lua::str_to_number(s),
        _ => None,
    }
}

pub fn tonumber(v: &V) -> V {
    to_number(v).map_or(V::Nil, V::Num)
}

/// `tostring` of a string or number
pub fn to_bytes(v: &V) -> Str {
    match v {
        V::Str(s) => s.clone(),
        V::Num(x) => lua::value::format_number(*x).as_bytes().into(),
        _ => panic!("expected a string, got a {} value", v.type_name()),
    }
}

pub fn tostring(v: &V) -> V {
    match v {
        V::Str(_) | V::Num(_) => V::Str(to_bytes(v)),
        V::Nil => s("nil"),
        V::Bool(b) => s(if *b { "true" } else { "false" }),
        v => s(v.type_name()),
    }
}

fn arith(v: &V, op: &str) -> f64 {
    to_number(v).unwrap_or_else(|| {
        panic!(
            "attempt to perform arithmetic ({op}) on a {} value",
            v.type_name()
        )
    })
}

pub fn add(a: &V, b: &V) -> V {
    n(arith(a, "add") + arith(b, "add"))
}

pub fn sub(a: &V, b: &V) -> V {
    n(arith(a, "sub") - arith(b, "sub"))
}

pub fn mul(a: &V, b: &V) -> V {
    n(arith(a, "mul") * arith(b, "mul"))
}

pub fn div(a: &V, b: &V) -> V {
    n(arith(a, "div") / arith(b, "div"))
}

pub fn rem(a: &V, b: &V) -> V {
    let (x, y) = (arith(a, "mod"), arith(b, "mod"));
    n(x - (x / y).floor() * y)
}

pub fn pow(a: &V, b: &V) -> V {
    n(arith(a, "pow").powf(arith(b, "pow")))
}

pub fn neg(a: &V) -> V {
    n(-arith(a, "unm"))
}

pub fn concat(a: &V, b: &V) -> V {
    let part = |v: &V| match v {
        V::Str(_) | V::Num(_) => to_bytes(v),
        _ => panic!("attempt to concatenate a {} value", v.type_name()),
    };
    V::Str([&*part(a), &*part(b)].concat().into())
}

pub fn eq(a: &V, b: &V) -> bool {
    a.raw_eq(b)
}

fn compare(a: &V, b: &V) -> std::cmp::Ordering {
    match (a, b) {
        (V::Num(x), V::Num(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
        (V::Str(x), V::Str(y)) => x.cmp(y),
        _ => panic!(
            "attempt to compare {} with {}",
            a.type_name(),
            b.type_name()
        ),
    }
}

pub fn lt(a: &V, b: &V) -> bool {
    if let (V::Num(x), V::Num(y)) = (a, b) {
        return x < y;
    }
    compare(a, b) == std::cmp::Ordering::Less
}

pub fn le(a: &V, b: &V) -> bool {
    if let (V::Num(x), V::Num(y)) = (a, b) {
        return x <= y;
    }
    compare(a, b) != std::cmp::Ordering::Greater
}

pub fn len(v: &V) -> V {
    match v {
        V::Str(s) => n(s.len() as f64),
        V::Tab(t) => n(t.len() as f64),
        _ => panic!("attempt to get length of a {} value", v.type_name()),
    }
}

const STRING_LIBRARY: &[&str] = &[
    "byte", "char", "dump", "find", "format", "gmatch", "gsub", "len", "lower", "match", "rep",
    "reverse", "sub", "upper",
];

/// Indexing a string reaches the string library
fn index_string(k: &[u8]) -> V {
    match STRING_LIBRARY.iter().find(|f| f.as_bytes() == k) {
        Some(f) => V::Func(Rc::new(Func {
            name: format!("string.{f}").into(),
            args: vec![],
        })),
        None => V::Nil,
    }
}

pub fn index(obj: &V, k: &V) -> V {
    match obj {
        V::Tab(t) => t.get(k).clone(),
        V::Str(_) => k.as_bytes().map_or(V::Nil, index_string),
        _ => panic!("attempt to index a {} value", obj.type_name()),
    }
}

pub fn field(obj: &V, name: &str) -> V {
    match obj {
        V::Tab(t) => t.get_str(name).clone(),
        V::Str(_) => index_string(name.as_bytes()),
        _ => panic!(
            "attempt to index a {} value (field '{name}')",
            obj.type_name()
        ),
    }
}

pub fn set_index(obj: &mut V, k: V, v: V) {
    match obj {
        V::Tab(t) => t.set_key(key(&k), v),
        _ => panic!("attempt to index a {} value", obj.type_name()),
    }
}

fn str_arg(v: &V, what: &str) -> Str {
    match v {
        V::Str(_) | V::Num(_) => to_bytes(v),
        _ => panic!(
            "bad argument to '{what}' (string expected, got {})",
            v.type_name()
        ),
    }
}

pub fn lower(v: &V) -> V {
    sb(&str_arg(v, "lower").to_ascii_lowercase())
}

pub fn upper(v: &V) -> V {
    sb(&str_arg(v, "upper").to_ascii_uppercase())
}

pub fn str_sub(v: &V, i: &V, j: &V) -> V {
    let x = str_arg(v, "sub");
    let len = x.len() as i64;
    let mut start = to_number(i).unwrap_or(1.0) as i64;
    let mut end = if j.is_nil() {
        -1
    } else {
        to_number(j).expect("sub: end") as i64
    };
    if start < 0 {
        start = (len + start + 1).max(0);
    }
    if end < 0 {
        end = len + end + 1;
    }
    start = start.max(1);
    end = end.min(len);
    if start > end {
        return s("");
    }
    sb(&x[start as usize - 1..end as usize])
}

fn caps(src: &[u8], c: &[Capture]) -> Vec<V> {
    c.iter()
        .map(|c| match *c {
            Capture::Str(a, b) => sb(&src[a..b]),
            Capture::Pos(p) => n((p + 1) as f64),
        })
        .collect()
}

/// Lua's `init` argument as a 0-based start
fn init_pos(init: &V, len: usize) -> usize {
    let len = len as i64;
    let i = to_number(init).unwrap_or(1.0) as i64;
    let i = if i < 0 { len + i + 1 } else { i };
    (i.max(1) - 1) as usize
}

pub fn str_match(v: &V, p: &V, init: &V) -> Vec<V> {
    let (s, p) = (str_arg(v, "match"), str_arg(p, "match"));
    match pattern::find_match(&s, &p, init_pos(init, s.len())).expect("malformed pattern") {
        Some(m) => caps(&s, &m.captures),
        None => vec![V::Nil],
    }
}

pub fn str_find(v: &V, p: &V, init: &V, plain: &V) -> Vec<V> {
    let (s, p) = (str_arg(v, "find"), str_arg(p, "find"));
    let init = init_pos(init, s.len());
    if init > s.len() {
        return vec![V::Nil];
    }
    match pattern::find(&s, &p, init, plain.truthy()).expect("malformed pattern") {
        Some(m) => {
            let mut out = vec![n((m.start + 1) as f64), n(m.end as f64)];
            out.extend(caps(&s, &m.captures));
            out
        }
        None => vec![V::Nil],
    }
}

pub fn gsub(v: &V, p: &V, repl: &V, max_n: &V) -> Vec<V> {
    let r = str_arg(repl, "gsub");
    gsub_with(v, p, Repl::Str(&r), max_n)
}

/// `gsub` with `string.upper` (true) or `string.lower` as the replacement
pub fn gsub_case(v: &V, p: &V, upper: bool, max_n: &V) -> Vec<V> {
    gsub_with(v, p, Repl::Case(upper), max_n)
}

fn gsub_with(v: &V, p: &V, repl: Repl, max_n: &V) -> Vec<V> {
    let (s, p) = (str_arg(v, "gsub"), str_arg(p, "gsub"));
    let max_n = to_number(max_n).map(|x| x as usize);
    let (out, count) = pattern::lua_gsub(&s, &p, repl, max_n).expect("malformed pattern");
    vec![sb(&out), n(count as f64)]
}

pub fn gmatch(v: &V, p: &V) -> Vec<Vec<V>> {
    let (s, p) = (str_arg(v, "gmatch"), str_arg(p, "gmatch"));
    pattern::gmatch(&s, &p)
        .expect("malformed pattern")
        .iter()
        .map(|c| caps(&s, c))
        .collect()
}

pub fn format(fmt: &V, args: &[V]) -> V {
    let f = str_arg(fmt, "format");
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    sb(&crate::rt::format_with(
        &f,
        |i| to_number(&arg(i)),
        |i| String::from_utf8_lossy(&to_bytes(&tostring(&arg(i)))).into_owned(),
    ))
}

pub fn floor(v: &V) -> V {
    n(arith(v, "floor").floor())
}

pub fn min(args: &[V]) -> V {
    let mut m = arith(&args[0], "min");
    for a in &args[1..] {
        let x = arith(a, "min");
        if x < m {
            m = x;
        }
    }
    n(m)
}

pub fn max(args: &[V]) -> V {
    let mut m = arith(&args[0], "max");
    for a in &args[1..] {
        let x = arith(a, "max");
        if x > m {
            m = x;
        }
    }
    n(m)
}

fn bit(v: &V) -> i32 {
    (arith(v, "bit").rem_euclid(4294967296.0) as u64 as u32) as i32
}

pub fn bor(args: &[V]) -> V {
    n(args.iter().fold(0i32, |acc, v| acc | bit(v)) as f64)
}

pub fn band(args: &[V]) -> V {
    n(args
        .iter()
        .skip(1)
        .fold(bit(&args[0]), |acc, v| acc & bit(v)) as f64)
}

pub fn bnot(v: &V) -> V {
    n(!bit(v) as f64)
}

/// `{ ... }` from keyed fields and positional items, sized as LuaJIT sizes
/// the constructor; a positional nil leaves a hole
pub fn tbl(keyed: Vec<(V, V)>, items: Vec<V>) -> V {
    let mut arr = items;
    if !arr.is_empty() && arr.len() < 2 {
        arr.push(V::Nil);
    }
    let mut t = Tab {
        arr,
        hash: Vec::with_capacity(keyed.len()),
    };
    for (k, v) in keyed {
        if !v.is_nil() {
            t.set_key(key(&k), v);
        }
    }
    V::Tab(Box::new(t))
}

pub fn t_insert(t: &mut V, v: V) {
    match t {
        V::Tab(t) => {
            let n = t.len() as i64 + 1;
            t.set_key(K::Int(n), v);
        }
        _ => panic!("bad argument to 'insert' (table expected)"),
    }
}

pub fn unpack(t: &V) -> Vec<V> {
    match t {
        V::Tab(t) => t.array(),
        _ => panic!("bad argument to 'unpack' (table expected)"),
    }
}

pub fn ipairs(t: &V) -> Vec<(V, V)> {
    match t {
        V::Tab(t) => t
            .array()
            .into_iter()
            .enumerate()
            .map(|(i, v)| (n(i as f64 + 1.0), v))
            .collect(),
        _ => panic!("bad argument to 'ipairs' (table expected)"),
    }
}

pub fn pairs(t: &V) -> Vec<(V, V)> {
    match t {
        V::Tab(t) => t
            .entries()
            .map(|(k, v)| {
                let k = match k {
                    K::Str(s) => V::Str(s),
                    K::Int(i) => n(i as f64),
                };
                (k, v.clone())
            })
            .collect(),
        _ => panic!("bad argument to 'pairs' (table expected)"),
    }
}

pub fn copy_table(v: &V) -> V {
    match v {
        V::Tab(_) | V::Mod(_) => v.clone(),
        _ => panic!("copyTable: table expected"),
    }
}

pub fn is_value_in_array(t: &V, v: &V) -> bool {
    match t {
        V::Tab(t) => t.array().iter().any(|x| x.raw_eq(v)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_like_luajit() {
        let t = tbl(vec![], vec![s("a")]);
        assert_eq!(t.as_tab().unwrap().len(), 1);
        let t = tbl(vec![], vec![s("a"), V::Nil, s("c")]);
        assert_eq!(t.as_tab().unwrap().len(), 3);
        let t = tbl(vec![], vec![s("a"), s("b"), V::Nil]);
        assert_eq!(t.as_tab().unwrap().len(), 2);
        let mut t = tbl(vec![], vec![]);
        t_insert(&mut t, s("x"));
        t_insert(&mut t, s("y"));
        assert_eq!(unpack(&t).len(), 2);
    }
}
