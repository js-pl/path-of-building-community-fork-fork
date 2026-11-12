//! Interned names: stat, condition, multiplier and skill names, mod types.

use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{OnceLock, RwLock};

/// A fast, deterministic hasher (FxHash) for symbols and short names
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl FxHasher {
    #[inline]
    fn add(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        if !rest.is_empty() {
            let mut b = [0u8; 8];
            b[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(b));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i)
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64)
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;

/// A map keyed by symbols
pub type SymMap<V> = HashMap<Sym, V, FxBuild>;

/// An interned string. Equal strings get equal symbols, so comparing and
/// hashing a symbol is comparing and hashing a number.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sym(u32);

#[derive(Default)]
struct Interner {
    ids: HashMap<&'static str, u32, FxBuild>,
    names: Vec<&'static str>,
}

fn interner() -> &'static RwLock<Interner> {
    static INTERNER: OnceLock<RwLock<Interner>> = OnceLock::new();
    INTERNER.get_or_init(Default::default)
}

/// Names up to this long are cached by [`Sym::new`]
const CACHED_LEN: usize = 56;
const SLOTS: usize = 4096;

/// A recently interned name, with its bytes
#[derive(Clone, Copy)]
struct Slot {
    len: usize,
    id: u32,
    bytes: [u8; CACHED_LEN],
}

thread_local! {
    /// The calculations look the same few names up again and again: a
    /// table of their own skips the shared table's lock and its names'
    /// scattered bytes
    static CACHE: std::cell::RefCell<Vec<Slot>> = std::cell::RefCell::new(vec![
        Slot { len: usize::MAX, id: 0, bytes: [0; CACHED_LEN] };
        SLOTS
    ]);
}

impl Sym {
    pub fn new(name: &str) -> Sym {
        let len = name.len();
        if len > CACHED_LEN {
            return Sym::intern(name);
        }
        let mut h = FxHasher::default();
        h.write(name.as_bytes());
        let k = (h.finish() >> 40) as usize;
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            let slot = &mut c[k % SLOTS];
            if slot.len == len && slot.bytes[..len] == *name.as_bytes() {
                return Sym(slot.id);
            }
            let sym = Sym::intern(name);
            slot.len = len;
            slot.id = sym.0;
            slot.bytes[..len].copy_from_slice(name.as_bytes());
            sym
        })
    }

    fn intern(name: &str) -> Sym {
        if let Some(&id) = interner().read().unwrap().ids.get(name) {
            return Sym(id);
        }
        let mut i = interner().write().unwrap();
        if let Some(&id) = i.ids.get(name) {
            return Sym(id);
        }
        // Names live as long as the program, like the data they name
        let name: &'static str = Box::leak(name.to_owned().into_boxed_str());
        let id = i.names.len() as u32;
        i.names.push(name);
        i.ids.insert(name, id);
        Sym(id)
    }

    pub fn as_str(self) -> &'static str {
        interner().read().unwrap().names[self.0 as usize]
    }
}

impl From<&str> for Sym {
    fn from(s: &str) -> Sym {
        Sym::new(s)
    }
}

impl fmt::Debug for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl fmt::Display for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_names_equal_symbols() {
        let a = Sym::new("Life");
        assert_eq!(a, Sym::new("Life"));
        assert_ne!(a, Sym::new("Mana"));
        assert_eq!(a.as_str(), "Life");
    }
}
