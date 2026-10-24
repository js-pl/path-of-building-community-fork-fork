//! Lua values as upstream's data and modifiers use them.
//!
//! Strings are byte strings, like Lua's. Tables keep insertion order, so
//! anything that iterates them is deterministic; upstream code that depends on
//! `pairs()` order has to be handled where it is ported.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// Immutable Lua string.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LuaStr(Rc<[u8]>);

impl LuaStr {
    pub fn new(bytes: &[u8]) -> Self {
        LuaStr(Rc::from(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The string as UTF-8, replacing invalid sequences.
    pub fn to_str_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }
}

impl From<&str> for LuaStr {
    fn from(s: &str) -> Self {
        LuaStr::new(s.as_bytes())
    }
}

impl From<String> for LuaStr {
    fn from(s: String) -> Self {
        LuaStr::new(s.as_bytes())
    }
}

impl From<Vec<u8>> for LuaStr {
    fn from(v: Vec<u8>) -> Self {
        LuaStr(Rc::from(v))
    }
}

impl From<&[u8]> for LuaStr {
    fn from(v: &[u8]) -> Self {
        LuaStr::new(v)
    }
}

impl fmt::Debug for LuaStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_str_lossy())
    }
}

impl fmt::Display for LuaStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_str_lossy())
    }
}

/// A function value. Upstream data embeds Lua functions; the importer records
/// where they come from so each can be mapped to its Rust port.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FuncRef {
    /// Upstream file, relative to upstream's `src/`
    pub file: Rc<str>,
    /// First line of the function in that file
    pub line: u32,
    /// Byte offset of the function in that file
    pub offset: u32,
    /// Where the function is stored, e.g. `skills["Fireball"]["preDamageFunc"]`
    pub key: Rc<str>,
    /// [`fnv1a`] of the function's source text, so changes upstream are noticed
    pub source_hash: u64,
}

/// A closure made by ported code: the name of the Rust function that runs
/// it plus its captured values, like a Lua closure's upvalues.
#[derive(Debug)]
pub struct Closure {
    pub name: Rc<str>,
    pub upvalues: Vec<Value>,
}

/// Error raised by Lua code (`error(value)`) or by an operation Lua rejects.
/// Boxed, so results stay small in generated code.
#[derive(Clone)]
pub struct LuaError(Box<ErrorData>);

#[derive(Clone)]
pub struct ErrorData {
    pub value: Value,
    /// Function names from the innermost call outwards, for messages
    pub traceback: Vec<Rc<str>>,
}

impl LuaError {
    pub fn new(value: Value) -> LuaError {
        LuaError(Box::new(ErrorData {
            value,
            traceback: vec![],
        }))
    }

    pub fn msg(msg: impl Into<String>) -> LuaError {
        LuaError::new(Value::Str(LuaStr::from(msg.into())))
    }

    /// The error value, as `pcall` returns it.
    pub fn value(&self) -> &Value {
        &self.0.value
    }

    pub fn into_value(self) -> Value {
        self.0.value
    }

    pub fn traceback(&self) -> &[Rc<str>] {
        &self.0.traceback
    }

    /// Adds a frame to the traceback, innermost first.
    pub fn push_frame(&mut self, name: Rc<str>) {
        if self.0.traceback.len() < 64 {
            self.0.traceback.push(name);
        }
    }
}

impl fmt::Debug for LuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl fmt::Display for LuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0.value {
            Value::Str(s) => write!(f, "{s}")?,
            v => write!(f, "{v:?}")?,
        }
        for t in &self.0.traceback {
            write!(f, "\n  in {t}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LuaError {}

pub type LResult<T> = Result<T, LuaError>;

type NativeFn = dyn Fn(&[Value]) -> LResult<Vec<Value>>;

/// `next` was given a key that is not in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKey;

/// An object the cycle collector ([`crate::gc`]) tracks, with its place in
/// the collection running or last run.
pub struct Gc<T> {
    pub(crate) slot: std::cell::Cell<u64>,
    inner: T,
}

impl<T> Gc<T> {
    pub fn new(inner: T) -> Self {
        Gc {
            slot: std::cell::Cell::new(0),
            inner,
        }
    }
}

impl<T> std::ops::Deref for Gc<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

/// A local captured by a closure, shared between them.
pub type CellRef = Rc<Gc<RefCell<Value>>>;

/// A callable Lua function: generated from upstream code or native.
pub struct Function {
    pub name: Rc<str>,
    pub f: Box<NativeFn>,
    /// Cells of the locals a translated closure captures, which its native
    /// closure holds too; the cycle collector ([`crate::gc`]) follows them
    pub upvalues: Vec<CellRef>,
    pub(crate) slot: std::cell::Cell<u64>,
}

impl Function {
    pub fn call(&self, args: &[Value]) -> LResult<Vec<Value>> {
        (self.f)(args).map_err(|mut e| {
            e.push_frame(self.name.clone());
            e
        })
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "function<{}>", self.name)
    }
}

#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Number(f64),
    Str(LuaStr),
    Table(TableRef),
    /// A Lua function from upstream's source, not callable by itself
    Func(Rc<FuncRef>),
    /// A closure built by ported code
    Closure(Rc<Closure>),
    /// A callable function
    Function(Rc<Function>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.into())
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Lua truthiness: everything but nil and false.
    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&LuaStr> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&TableRef> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Func(_) | Value::Closure(_) | Value::Function(_) => "function",
        }
    }

    /// Lua `==`: by value for primitives, by identity for tables.
    pub fn raw_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => Rc::ptr_eq(&a.0, &b.0),
            (Value::Func(a), Value::Func(b)) => Rc::ptr_eq(a, b),
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Structural equality, recursing into tables. For tests.
    pub fn deep_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Table(a), Value::Table(b)) => {
                if Rc::ptr_eq(&a.0, &b.0) {
                    return true;
                }
                let a = a.borrow();
                let b = b.borrow();
                a.len_pairs() == b.len_pairs()
                    && a.iter().all(|(k, v)| v.deep_eq(&b.get(&k.to_value())))
            }
            (Value::Number(a), Value::Number(b)) => a.to_bits() == b.to_bits() || a == b,
            _ => self.raw_eq(other),
        }
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::str(s)
    }
}

impl From<LuaStr> for Value {
    fn from(s: LuaStr) -> Self {
        Value::Str(s)
    }
}

impl From<TableRef> for Value {
    fn from(t: TableRef) -> Self {
        Value::Table(t)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Number(n) => write!(f, "{}", format_number(*n)),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Table(t) => {
                let t = t.borrow();
                f.debug_map()
                    .entries(t.iter().map(|(k, v)| (k.to_value(), v.clone())))
                    .finish()
            }
            Value::Func(r) => write!(f, "function<{}:{} {}>", r.file, r.line, r.key),
            Value::Closure(c) => write!(f, "closure<{}>", c.name),
            Value::Function(func) => write!(f, "function<{}>", func.name),
        }
    }
}

/// 64 bit FNV-1a, a hash that stays the same across Rust versions.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Lua's `tostring` for numbers (`%.14g`).
pub fn format_number(n: f64) -> String {
    crate::format::format_g(n, 14)
}

/// Table key. Numbers with an integer value are stored as integers, so `1`
/// and `1.0` are the same key, as in Lua.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Int(i64),
    /// Bits of a non-integer, non-NaN float
    Float(u64),
    Bool(bool),
    Str(LuaStr),
    Table(TablePtr),
    Func(FuncPtr),
    Closure(ClosurePtr),
    Function(FunctionPtr),
}

#[derive(Clone, Debug)]
pub struct FunctionPtr(pub Rc<Function>);
impl PartialEq for FunctionPtr {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for FunctionPtr {}
impl Hash for FunctionPtr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Rc::as_ptr(&self.0) as *const u8 as usize).hash(state)
    }
}

#[derive(Clone, Debug)]
pub struct ClosurePtr(pub Rc<Closure>);
impl PartialEq for ClosurePtr {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ClosurePtr {}
impl Hash for ClosurePtr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Rc::as_ptr(&self.0) as *const u8 as usize).hash(state)
    }
}

#[derive(Clone, Debug)]
pub struct TablePtr(pub TableRef);
impl PartialEq for TablePtr {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0.0, &other.0.0)
    }
}
impl Eq for TablePtr {}
impl Hash for TablePtr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Rc::as_ptr(&self.0.0) as usize).hash(state)
    }
}

#[derive(Clone, Debug)]
pub struct FuncPtr(pub Rc<FuncRef>);
impl PartialEq for FuncPtr {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for FuncPtr {}
impl Hash for FuncPtr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Rc::as_ptr(&self.0) as usize).hash(state)
    }
}

impl Key {
    /// The key for a value, or None for nil and NaN, which Lua rejects.
    pub fn from_value(v: &Value) -> Option<Key> {
        Some(match v {
            Value::Nil => return None,
            Value::Bool(b) => Key::Bool(*b),
            Value::Number(n) => {
                if n.is_nan() {
                    return None;
                }
                if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
                    Key::Int(*n as i64)
                } else {
                    Key::Float(n.to_bits())
                }
            }
            Value::Str(s) => Key::Str(s.clone()),
            Value::Table(t) => Key::Table(TablePtr(t.clone())),
            Value::Func(f) => Key::Func(FuncPtr(f.clone())),
            Value::Closure(c) => Key::Closure(ClosurePtr(c.clone())),
            Value::Function(f) => Key::Function(FunctionPtr(f.clone())),
        })
    }

    pub fn to_value(&self) -> Value {
        match self {
            Key::Int(i) => Value::Number(*i as f64),
            Key::Float(b) => Value::Number(f64::from_bits(*b)),
            Key::Bool(b) => Value::Bool(*b),
            Key::Str(s) => Value::Str(s.clone()),
            Key::Table(t) => Value::Table(t.0.clone()),
            Key::Func(f) => Value::Func(f.0.clone()),
            Key::Closure(c) => Value::Closure(c.0.clone()),
            Key::Function(f) => Value::Function(f.0.clone()),
        }
    }
}

impl From<&str> for Key {
    fn from(s: &str) -> Self {
        Key::Str(s.into())
    }
}

impl From<i64> for Key {
    fn from(i: i64) -> Self {
        Key::Int(i)
    }
}

/// Lua table with an array part (`t[1..n]`) and an insertion-ordered hash part.
#[derive(Default, Clone)]
pub struct Table {
    array: Vec<Value>,
    /// Hash part in insertion order; removed entries leave a None
    entries: Vec<Option<(Key, Value)>>,
    /// Slot of every key ever set in the hash part; a removed key keeps its
    /// slot, so `next` can continue from it, as in Lua
    index: HashMap<Key, usize, std::hash::BuildHasherDefault<FxHasher>>,
    /// Number of live entries in the hash part
    live: usize,
    pub meta: Option<TableRef>,
}

/// The fast, non-cryptographic hasher rustc uses.
#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(b));
        }
    }

    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    fn finish(&self) -> u64 {
        self.hash
    }
}

impl FxHasher {
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Table {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &Value) -> Value {
        match Key::from_value(key) {
            Some(k) => self.get_key(&k),
            None => Value::Nil,
        }
    }

    pub fn get_key(&self, key: &Key) -> Value {
        if let Key::Int(i) = key
            && *i >= 1
            && (*i as usize) <= self.array.len()
        {
            return self.array[*i as usize - 1].clone();
        }
        match self.index.get(key) {
            Some(&i) => self.entries[i]
                .as_ref()
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
            None => Value::Nil,
        }
    }

    pub fn get_str(&self, key: &str) -> Value {
        self.get_key(&Key::from(key))
    }

    pub fn get_int(&self, i: i64) -> Value {
        self.get_key(&Key::Int(i))
    }

    /// `t[key] = value`. Panics on a nil or NaN key, which Lua rejects too.
    pub fn set(&mut self, key: Value, value: Value) {
        let k = Key::from_value(&key).expect("table index is nil or NaN");
        self.set_key(k, value);
    }

    pub fn set_key(&mut self, key: Key, value: Value) {
        if let Key::Int(i) = key
            && i >= 1
        {
            let i = i as usize;
            if i <= self.array.len() {
                self.array[i - 1] = value;
                if i == self.array.len() {
                    // Keep the array part free of trailing nils
                    while matches!(self.array.last(), Some(Value::Nil)) {
                        self.array.pop();
                    }
                }
                return;
            }
            if i == self.array.len() + 1 && !value.is_nil() {
                self.remove_hash(&Key::Int(i as i64));
                self.array.push(value);
                // Pull following integer keys out of the hash part
                loop {
                    let next = Key::Int(self.array.len() as i64 + 1);
                    match self.remove_hash(&next) {
                        Some(v) => self.array.push(v),
                        None => break,
                    }
                }
                return;
            }
        }
        if value.is_nil() {
            self.remove_hash(&key);
            return;
        }
        match self.index.get(&key) {
            Some(&i) => {
                if self.entries[i].is_none() {
                    self.live += 1;
                }
                self.entries[i] = Some((key, value))
            }
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push(Some((key, value)));
                self.live += 1;
            }
        }
    }

    pub fn set_str(&mut self, key: &str, value: impl Into<Value>) {
        self.set_key(Key::from(key), value.into());
    }

    fn remove_hash(&mut self, key: &Key) -> Option<Value> {
        let i = *self.index.get(key)?;
        let (_, v) = self.entries[i].take()?;
        self.live -= 1;
        Some(v)
    }

    /// `#t`: a border of the table. The array part never ends in nil, so its
    /// length is one.
    pub fn border(&self) -> usize {
        self.array.len()
    }

    /// `#t`: the length of the array part.
    pub fn len(&self) -> usize {
        self.array.len()
    }

    pub fn is_empty(&self) -> bool {
        self.array.is_empty() && self.live == 0
    }

    /// Number of non-nil entries.
    pub fn len_pairs(&self) -> usize {
        self.array.iter().filter(|v| !v.is_nil()).count() + self.live
    }

    /// `t[#t + 1] = value`
    pub fn push(&mut self, value: Value) {
        let n = self.array.len() as i64 + 1;
        self.set_key(Key::Int(n), value);
    }

    /// `table.insert(t, pos, value)`
    pub fn insert(&mut self, pos: usize, value: Value) {
        assert!(
            pos >= 1 && pos <= self.array.len() + 1,
            "table.insert position out of bounds"
        );
        self.array.insert(pos - 1, value);
    }

    /// `table.remove(t, pos)`
    pub fn remove(&mut self, pos: usize) -> Value {
        if pos >= 1 && pos <= self.array.len() {
            self.array.remove(pos - 1)
        } else {
            Value::Nil
        }
    }

    /// The array part, `t[1..#t]`.
    pub fn array(&self) -> &[Value] {
        &self.array
    }

    /// `next(t, key)`: the entry after `key` (the first for nil), array part
    /// first, then the hash part in insertion order. Err for a key that is
    /// not in the table.
    pub fn next(&self, key: &Value) -> Result<Option<(Value, Value)>, InvalidKey> {
        // Position to start scanning: array index, or hash slot + array len
        let mut pos = match key {
            Value::Nil => 0,
            k => {
                let k = Key::from_value(k).ok_or(InvalidKey)?;
                match k {
                    Key::Int(i) if i >= 1 && (i as usize) <= self.array.len() => i as usize,
                    k => match self.index.get(&k) {
                        Some(&slot) => self.array.len() + slot + 1,
                        // An array index past the end: the array part shrank
                        // after the key was set to nil, continue with the hash part
                        None if matches!(k, Key::Int(i) if i >= 1) => self.array.len(),
                        None => return Err(InvalidKey),
                    },
                }
            }
        };
        while pos < self.array.len() {
            if !self.array[pos].is_nil() {
                return Ok(Some((
                    Value::Number((pos + 1) as f64),
                    self.array[pos].clone(),
                )));
            }
            pos += 1;
        }
        let mut h = pos - self.array.len();
        while h < self.entries.len() {
            if let Some((k, v)) = &self.entries[h] {
                return Ok(Some((k.to_value(), v.clone())));
            }
            h += 1;
        }
        Ok(None)
    }

    /// Array part first, then the hash part in insertion order.
    /// Entries of the hash part, without cloning
    pub fn raw_entries(&self) -> impl Iterator<Item = &(Key, Value)> + '_ {
        self.entries.iter().flatten()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Key, Value)> + '_ {
        self.array
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_nil())
            .map(|(i, v)| (Key::Int(i as i64 + 1), v.clone()))
            .chain(self.entries.iter().flatten().cloned())
    }
}

/// Shared, mutable table, like a Lua table reference. Tables are registered
/// with the cycle collector ([`crate::gc`]).
#[derive(Clone)]
pub struct TableRef(pub Rc<Gc<RefCell<Table>>>);

impl Default for TableRef {
    fn default() -> Self {
        Self::from_table(Table::default())
    }
}

impl TableRef {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_table(t: Table) -> Self {
        let r = Rc::new(Gc::new(RefCell::new(t)));
        crate::gc::register_table(&r);
        TableRef(r)
    }

    pub fn borrow(&self) -> std::cell::Ref<'_, Table> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, Table> {
        self.0.borrow_mut()
    }

    pub fn get(&self, key: &Value) -> Value {
        self.borrow().get(key)
    }

    pub fn get_str(&self, key: &str) -> Value {
        self.borrow().get_str(key)
    }

    pub fn get_int(&self, i: i64) -> Value {
        self.borrow().get_int(i)
    }

    pub fn set(&self, key: Value, value: Value) {
        self.borrow_mut().set(key, value)
    }

    pub fn set_str(&self, key: &str, value: impl Into<Value>) {
        self.borrow_mut().set_str(key, value)
    }

    pub fn push(&self, value: Value) {
        self.borrow_mut().push(value)
    }

    pub fn len(&self) -> usize {
        self.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.borrow().is_empty()
    }

    /// Snapshot of the entries, safe to use while mutating the table.
    pub fn entries(&self) -> Vec<(Key, Value)> {
        self.borrow().iter().collect()
    }

    /// Snapshot of the array part.
    pub fn array(&self) -> Vec<Value> {
        self.borrow().array().to_vec()
    }

    /// Deep copy, like upstream's `copyTable`.
    pub fn deep_copy(&self) -> TableRef {
        let copy = TableRef::new();
        for (k, v) in self.entries() {
            let v = match v {
                Value::Table(t) => Value::Table(t.deep_copy()),
                v => v,
            };
            copy.borrow_mut().set_key(k, v);
        }
        copy
    }

    pub fn ptr_eq(&self, other: &TableRef) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for TableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Value::Table(self.clone()).fmt(f)
    }
}

/// Builds a table from key/value pairs: `table! { "type" => "Condition", "var" => "X" }`.
#[macro_export]
macro_rules! table {
    () => { $crate::TableRef::new() };
    ($($k:expr => $v:expr),+ $(,)?) => {{
        let t = $crate::TableRef::new();
        $( t.set($crate::Value::from($k), $crate::Value::from($v)); )+
        t
    }};
}

/// Builds an array table: `list![a, b, c]`.
#[macro_export]
macro_rules! list {
    () => { $crate::TableRef::new() };
    ($($v:expr),+ $(,)?) => {{
        let t = $crate::TableRef::new();
        $( t.push($crate::Value::from($v)); )+
        t
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_and_hash_parts() {
        let mut t = Table::new();
        t.set(Value::Number(2.0), Value::str("b"));
        t.set(Value::Number(1.0), Value::str("a"));
        assert_eq!(t.len(), 2);
        t.set_str("x", 1.0);
        let keys: Vec<_> = t.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![Key::Int(1), Key::Int(2), Key::from("x")]);
        t.set(Value::Number(2.0), Value::Nil);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn float_keys_normalise() {
        let mut t = Table::new();
        t.set(Value::Number(3.0), Value::Bool(true));
        assert!(t.get(&Value::Number(3.0)).truthy());
        t.set(Value::Number(0.5), Value::Bool(true));
        assert!(t.get(&Value::Number(0.5)).truthy());
    }
}
