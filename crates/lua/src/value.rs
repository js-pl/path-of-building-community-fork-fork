//! Lua values as upstream's data and modifiers use them.
//!
//! Strings are byte strings, like Lua's. Tables iterate deterministically:
//! number and boolean keys in LuaJIT's order, other keys in insertion order.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::ljtab::{LjTable, NKey, Objs, Slot};

/// Immutable Lua string. Its hash is computed once and kept in front of the
/// bytes, so table lookups do not hash the string again.
#[derive(Clone)]
pub struct LuaStr(Rc<[u8]>);

/// Bytes of the hash in front of a string's bytes
const HASH: usize = 8;

impl LuaStr {
    pub fn new(bytes: &[u8]) -> Self {
        let mut h = FxHasher::default();
        h.write(bytes);
        h.write_usize(bytes.len());
        let hash = h.finish();
        let mut rc = Rc::<[u8]>::new_uninit_slice(HASH + bytes.len());
        let buf = Rc::get_mut(&mut rc).expect("new allocation");
        for (dst, src) in buf.iter_mut().zip(hash.to_le_bytes().iter().chain(bytes)) {
            dst.write(*src);
        }
        // Safety: every byte was written above
        LuaStr(unsafe { rc.assume_init() })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0[HASH..]
    }

    fn hash_value(&self) -> u64 {
        u64::from_le_bytes(self.0[..HASH].try_into().expect("hash prefix"))
    }

    pub fn len(&self) -> usize {
        self.0.len() - HASH
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The string as UTF-8, replacing invalid sequences.
    pub fn to_str_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.as_bytes())
    }
}

impl PartialEq for LuaStr {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
            || (self.hash_value() == other.hash_value() && self.as_bytes() == other.as_bytes())
    }
}

impl Eq for LuaStr {}

impl Hash for LuaStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash_value())
    }
}

impl PartialOrd for LuaStr {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LuaStr {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
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
        LuaStr::new(&v)
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

impl Slot for Value {
    #[inline]
    fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }
}

/// The layout key of a number or boolean key.
#[inline]
fn nkey(k: &Key) -> Option<NKey> {
    match k {
        Key::Int(i) => Some(NKey::int(*i)),
        Key::Float(b) => Some(NKey::Num(*b)),
        Key::Bool(b) => Some(NKey::Bool(*b)),
        _ => None,
    }
}

#[inline]
fn nkey_value(k: NKey) -> Value {
    match k {
        NKey::Num(b) => Value::Number(f64::from_bits(b)),
        NKey::Bool(b) => Value::Bool(b),
    }
}

/// Lua table. Numbers and booleans live in LuaJIT's layout
/// ([`crate::ljtab`]), so `pairs` visits them in LuaJIT's order. Other keys,
/// whose order in LuaJIT depends on a random seed or on addresses, follow
/// them in the order they were first set.
#[derive(Default, Clone)]
pub struct Table {
    lj: LjTable<Value>,
    /// Keys of other types; a removed key keeps its entry with a nil value,
    /// so `next` can continue from it and setting it again puts it back
    entries: Vec<Entry>,
    index: HashMap<Key, u32, std::hash::BuildHasherDefault<FxHasher>>,
    /// Entries with a value
    live: u32,
    /// Entries without a value that still hold a slot of LuaJIT's hash part,
    /// plus `reserved`
    dead: u32,
    /// Slots a constructor's template holds for string keys stored later
    reserved: u32,
    pub meta: Option<TableRef>,
}

#[derive(Clone)]
struct Entry {
    key: Key,
    val: Value,
    /// Resize generation in which a dead entry last held a slot
    epoch: u32,
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

    /// `lj_tab_new`: a table with `asize` array slots (keys `0..asize`) and
    /// `2^hbits` hash slots, as table constructors make them.
    pub fn with_sizes(asize: u32, hbits: u32) -> Self {
        Table {
            lj: LjTable::with_sizes(asize, hbits),
            ..Default::default()
        }
    }

    #[inline]
    fn objs(&self) -> Objs {
        Objs {
            live: self.live,
            slots: self.live + self.dead,
        }
    }

    pub fn get(&self, key: &Value) -> Value {
        match Key::from_value(key) {
            Some(k) => self.get_key(&k),
            None => Value::Nil,
        }
    }

    #[inline]
    pub fn get_key(&self, key: &Key) -> Value {
        match nkey(key) {
            Some(k) => self.lj.get(k).cloned().unwrap_or_default(),
            None => match self.index.get(key) {
                Some(&i) => self.entries[i as usize].val.clone(),
                None => Value::Nil,
            },
        }
    }

    pub fn get_str(&self, key: &str) -> Value {
        self.get_key(&Key::from(key))
    }

    #[inline]
    pub fn get_int(&self, i: i64) -> Value {
        self.lj.get(NKey::int(i)).cloned().unwrap_or_default()
    }

    /// `t[key] = value`. Panics on a nil or NaN key, which Lua rejects too.
    pub fn set(&mut self, key: Value, value: Value) {
        let k = Key::from_value(&key).expect("table index is nil or NaN");
        self.set_key(k, value);
    }

    /// `t[key] = value`. As in LuaJIT, setting a missing key to nil gives it
    /// a slot too.
    pub fn set_key(&mut self, key: Key, value: Value) {
        match nkey(&key) {
            Some(k) => self.set_n(k, value),
            None => self.set_obj(key, value, true),
        }
    }

    #[inline]
    fn set_n(&mut self, k: NKey, value: Value) {
        let (objs, epoch) = (self.objs(), self.lj.epoch);
        *self.lj.slot(k, objs) = value;
        if self.lj.epoch != epoch {
            self.resized();
        }
    }

    /// Dead keys go with a resize.
    #[cold]
    fn resized(&mut self) {
        self.dead = 0;
        self.reserved = 0;
    }

    /// A new slot for a key outside LuaJIT's layout, one a template
    /// reserved if `reserved`.
    fn new_obj_slot(&mut self, reserved: bool) {
        if reserved && self.reserved > 0 {
            self.reserved -= 1;
            self.dead -= 1;
        } else if self.lj.obj_newkey(self.objs()) {
            self.resized();
        }
    }

    /// A template slot for a string key (see [`crate::ast::Step::Reserve`]).
    pub fn reserve(&mut self) {
        if self.lj.obj_newkey(self.objs()) {
            self.resized();
        }
        self.dead += 1;
        self.reserved += 1;
    }

    /// A reserved string key stored as nil: its slot stays dead.
    pub fn unreserve(&mut self) {
        self.reserved = self.reserved.saturating_sub(1);
    }

    /// `t[key] = value` in a constructor's template, which does not take
    /// the slots the template reserved.
    pub fn set_template(&mut self, key: Key, value: Value) {
        match nkey(&key) {
            Some(k) => self.set_n(k, value),
            None => self.set_obj(key, value, false),
        }
    }

    fn set_obj(&mut self, key: Key, value: Value, reserved: bool) {
        let i = match self.index.get(&key) {
            Some(&i) => i as usize,
            None => {
                self.new_obj_slot(reserved);
                if value.is_nil() {
                    // A dead slot; no entry, so stores of nil stay cheap
                    self.dead += 1;
                    return;
                }
                self.live += 1;
                self.index.insert(key.clone(), self.entries.len() as u32);
                self.entries.push(Entry {
                    key,
                    val: value,
                    epoch: self.lj.epoch,
                });
                return;
            }
        };
        let epoch = self.lj.epoch;
        let e = &mut self.entries[i];
        match (e.val.is_nil(), value.is_nil()) {
            (false, false) => e.val = value,
            (false, true) => {
                e.val = value;
                e.epoch = epoch;
                self.live -= 1;
                self.dead += 1;
            }
            (true, _) if e.epoch == epoch => {
                if !value.is_nil() {
                    e.val = value;
                    self.dead -= 1;
                    self.live += 1;
                }
            }
            (true, nil) => {
                // Its slot went with a resize: a new key to LuaJIT
                self.new_obj_slot(reserved);
                let e = &mut self.entries[i];
                e.epoch = self.lj.epoch;
                e.val = value;
                if nil {
                    self.dead += 1;
                } else {
                    self.live += 1;
                }
            }
        }
    }

    pub fn set_str(&mut self, key: &str, value: impl Into<Value>) {
        self.set_key(Key::from(key), value.into());
    }

    /// Number of array part slots (keys `0..asize`).
    pub fn asize(&self) -> usize {
        self.lj.asize()
    }

    /// `lj_tab_reasize`: an array part for keys up to `n`.
    pub fn reasize(&mut self, n: u32) {
        self.lj.reasize(n, self.live);
        self.resized();
    }

    /// Stores `values` at keys `start..` as a constructor's multiple
    /// results do (`BC_TSETM`).
    pub fn set_multi(&mut self, start: u32, values: Vec<Value>) {
        let need = start as usize + values.len();
        if values.is_empty() {
            return;
        }
        if need > self.asize() {
            self.reasize(need as u32);
        }
        for (i, v) in values.into_iter().enumerate() {
            self.set_n(NKey::int(start as i64 + i as i64), v);
        }
    }

    /// `#t`, the border LuaJIT's interpreter finds.
    #[inline]
    pub fn border(&self) -> usize {
        self.lj.border()
    }

    /// `#t`
    pub fn len(&self) -> usize {
        self.border()
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0 && self.lj.iter().next().is_none()
    }

    /// Number of non-nil entries.
    pub fn len_pairs(&self) -> usize {
        self.lj.iter().count() + self.live as usize
    }

    /// `t[#t + 1] = value`
    pub fn push(&mut self, value: Value) {
        let n = self.border() as i64 + 1;
        self.set_n(NKey::int(n), value);
    }

    /// `t[1..=#t]`
    pub fn array(&self) -> Vec<Value> {
        let n = self.border();
        let arr = self.lj.array_part();
        if n < arr.len() {
            return arr[1..=n].to_vec();
        }
        (1..=n as i64).map(|i| self.get_int(i)).collect()
    }

    /// The first entry at or after iteration position `pos`, with its
    /// position. Positions are what LuaJIT's `pairs` loops step through.
    #[inline]
    pub fn entry_at(&self, pos: usize) -> Option<(usize, Value, Value)> {
        if let Some((p, k, v)) = self.lj.entry_at(pos) {
            return Some((p, nkey_value(k), v.clone()));
        }
        let base = self.lj.positions();
        let from = pos.saturating_sub(base);
        self.entries
            .get(from..)?
            .iter()
            .enumerate()
            .find(|(_, e)| !e.val.is_nil())
            .map(|(i, e)| (base + from + i, e.key.to_value(), e.val.clone()))
    }

    /// `next(t, key)`: the entry after `key` (the first for nil). Err for a
    /// key that is not in the table.
    pub fn next(&self, key: &Value) -> Result<Option<(Value, Value)>, InvalidKey> {
        let pos = match key {
            Value::Nil => 0,
            k => {
                let k = Key::from_value(k).ok_or(InvalidKey)?;
                match nkey(&k) {
                    Some(nk) => self.lj.position_after(nk).ok_or(InvalidKey)?,
                    None => {
                        let i = *self.index.get(&k).ok_or(InvalidKey)?;
                        self.lj.positions() + i as usize + 1
                    }
                }
            }
        };
        Ok(self.entry_at(pos).map(|(_, k, v)| (k, v)))
    }

    /// Calls `f` with every key that is an object and every value, for the
    /// cycle collector.
    pub fn for_each_ref(&self, mut f: impl FnMut(Option<&Key>, &Value)) {
        for v in self.lj.values() {
            f(None, v);
        }
        for e in &self.entries {
            f(Some(&e.key), &e.val);
        }
    }

    /// Entries in `pairs` order.
    pub fn iter(&self) -> impl Iterator<Item = (Key, Value)> + '_ {
        self.lj
            .iter()
            .map(|(k, v)| (Key::from_value(&nkey_value(k)).unwrap(), v.clone()))
            .chain(
                self.entries
                    .iter()
                    .filter(|e| !e.val.is_nil())
                    .map(|e| (e.key.clone(), e.val.clone())),
            )
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
        self.borrow().array()
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
    fn mixed_keys() {
        let mut t = Table::new();
        t.set_str("b", 1.0);
        for k in [70000.0, 2.0, 1.0, 0.5] {
            t.set(Value::Number(k), Value::Bool(true));
        }
        t.set_str("a", 1.0);
        let keys: Vec<_> = t.iter().map(|(k, _)| k).collect();
        // Array part, hash part in LuaJIT's order, then strings as set
        assert_eq!(keys[..2], [Key::Int(1), Key::Int(2)]);
        assert_eq!(keys[4..], [Key::from("b"), Key::from("a")]);
        // A missing key set to nil gets a slot, so next can continue from it
        t.set(Value::Number(5.0), Value::Nil);
        assert!(t.next(&Value::Number(5.0)).is_ok());
        let mut pos = 0;
        let mut n = 0;
        while let Some((p, _, _)) = t.entry_at(pos) {
            pos = p + 1;
            n += 1;
        }
        assert_eq!(n, t.len_pairs());
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
