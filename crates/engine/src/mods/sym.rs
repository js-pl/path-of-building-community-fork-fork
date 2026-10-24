//! Interned names: stat, condition, multiplier and skill names, mod types.

use std::collections::HashMap;
use std::fmt;
use std::sync::{OnceLock, RwLock};

/// An interned string. Equal strings get equal symbols, so comparing and
/// hashing a symbol is comparing and hashing a number.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sym(u32);

#[derive(Default)]
struct Interner {
    ids: HashMap<&'static str, u32>,
    names: Vec<&'static str>,
}

fn interner() -> &'static RwLock<Interner> {
    static INTERNER: OnceLock<RwLock<Interner>> = OnceLock::new();
    INTERNER.get_or_init(Default::default)
}

impl Sym {
    pub fn new(name: &str) -> Sym {
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
